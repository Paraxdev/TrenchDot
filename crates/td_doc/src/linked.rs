//! Linked groups: groups sharing a link id mirror the contents of the one that was edited last.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;
use td_core::{DMat4, DVec3, NodeId};

use crate::format;
use crate::map::{Map, NodeKind};
use crate::ops::{self, EditOptions};
use crate::selection::Selection;

/// Every linked group by link id. Build it once when looking up the members of several sets.
pub fn link_sets(map: &Map) -> BTreeMap<u64, Vec<NodeId>> {
    let mut sets: BTreeMap<u64, Vec<NodeId>> = BTreeMap::new();
    for (id, node) in map.nodes.iter() {
        if let NodeKind::Group(g) = &node.kind
            && let Some(link) = g.link_id
        {
            sets.entry(link).or_default().push(*id);
        }
    }

    sets
}

fn group_transform(map: &Map, id: NodeId) -> DMat4 {
    match map.get(id).map(|n| &n.kind) {
        Some(NodeKind::Group(g)) => g.transform,
        _ => DMat4::IDENTITY,
    }
}

/// Descendants in depth-first order with their depth, leaves expressed in the link set's shared space.
fn normalized(map: &Map, group: NodeId) -> Vec<(usize, NodeKind, Option<String>)> {
    normalized_carried(map, group, &DMat4::IDENTITY)
}

/// [`normalized`] after the group was carried by `carry` with UV lock off, which leaves its world UVs where they were.
fn normalized_carried(map: &Map, group: NodeId, carry: &DMat4) -> Vec<(usize, NodeKind, Option<String>)> {
    let inverse = group_transform(map, group).inverse();
    let carried = *carry != DMat4::IDENTITY;
    let back = (*carry * group_transform(map, group)).inverse();
    let mut out = Vec::new();
    let mut stack: Vec<(NodeId, usize)> = map.get(group).map(|n| n.children.iter().rev().map(|c| (*c, 1)).collect()).unwrap_or_default();
    while let Some((id, depth)) = stack.pop() {
        let Some(node) = map.get(id) else { continue };
        let leaf = node.children.is_empty();
        let kind = match &node.kind {
            NodeKind::Brush(b) if carried => NodeKind::Brush(b.transformed(carry, false).transformed(&back, true)),
            NodeKind::Mesh(m) if carried => NodeKind::Mesh(m.transformed(carry, false).transformed(&back, true)),
            NodeKind::Brush(b) => NodeKind::Brush(b.transformed(&inverse, true)),
            NodeKind::Mesh(m) => NodeKind::Mesh(m.transformed(&inverse, true)),
            NodeKind::Terrain(t) => NodeKind::Terrain(t.transformed(&inverse)),
            NodeKind::Scatter(s) => {
                let mut s = s.clone();
                s.transform(&inverse);
                NodeKind::Scatter(s)
            }
            NodeKind::Entity(e) if leaf => {
                let mut e = e.clone();
                e.transform_by(&inverse);
                NodeKind::Entity(e)
            }
            NodeKind::Instance(i) => {
                let mut e = crate::Entity::new("");
                e.origin = i.origin;
                e.angles = i.angles;
                e.transform_by(&inverse);
                NodeKind::Instance(crate::map::Instance { origin: e.origin, angles: e.angles, ..i.clone() })
            }
            other => other.clone(),
        };
        out.push((depth, kind, node.label.clone()));
        stack.extend(node.children.iter().rev().map(|c| (*c, depth + 1)));
    }

    out
}

fn close(a: DVec3, b: DVec3) -> bool {
    (a - b).abs().max_element() < 1e-3
}

/// Numbers equal up to the rounding a round trip through the group transforms leaves behind.
fn values_close(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() < 1e-3 || (x - y).abs() <= 1e-6 * x.abs().max(y.abs()),
            _ => x == y,
        },
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| values_close(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| values_close(v, w))),
        _ => a == b,
    }
}

/// With `uv` off the texture projections are left out, see [`conflicts`].
fn content<T: Serialize>(v: &T, uv: bool) -> Value {
    let mut value = serde_json::to_value(v).unwrap_or(Value::Null);
    if !uv {
        strip_uv(&mut value);
    }

    value
}

fn strip_uv(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("uv");
            map.remove("uvs");
            map.values_mut().for_each(strip_uv);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_uv),
        _ => {}
    }
}

/// Whole content of two normalized nodes, so any edit (UVs, face properties, paint, entity keys) counts.
fn kinds_match(a: &NodeKind, b: &NodeKind, uv: bool) -> bool {
    match (a, b) {
        (NodeKind::Brush(x), NodeKind::Brush(y)) => values_close(&content(x, uv), &content(y, uv)),
        (NodeKind::Mesh(x), NodeKind::Mesh(y)) => values_close(&content(x, uv), &content(y, uv)),
        (NodeKind::Entity(x), NodeKind::Entity(y)) => values_close(&content(x, true), &content(y, true)),
        (NodeKind::Instance(x), NodeKind::Instance(y)) => values_close(&content(x, true), &content(y, true)),
        (NodeKind::Scatter(x), NodeKind::Scatter(y)) => values_close(&content(x, true), &content(y, true)),
        // Nested groups differ in their placement by design.
        (NodeKind::Group(x), NodeKind::Group(y)) => x.name == y.name && x.link_id == y.link_id,
        // Terrains only move and scale, which leaves everything but origin and cell size exact.
        (NodeKind::Terrain(x), NodeKind::Terrain(y)) => {
            close(x.origin, y.origin)
                && (x.cell_size - y.cell_size).abs() < 1e-6
                && td_geom::Terrain { origin: y.origin, cell_size: y.cell_size, ..x.clone() } == *y
        }
        _ => a == b,
    }
}

type Content = [(usize, NodeKind, Option<String>)];

fn same_content(a: &Content, b: &Content) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|((da, ka, la), (db, kb, lb))| da == db && la == lb && kinds_match(ka, kb, true))
}

/// [`same_content`] without the texture projections, which [`sync`] settles on its own, and with the order of the
/// entries and of brush faces left open: the pieces of a carve come out in a different order in a reflected half.
fn same_shape(a: &Content, b: &Content) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let in_order = a.iter().zip(b).all(|((da, ka, la), (db, kb, lb))| da == db && la == lb && kinds_match(ka, kb, false));
    if in_order {
        return true;
    }

    let mut used = vec![false; b.len()];
    a.iter().all(|(da, ka, la)| {
        let found = (0..b.len()).find(|k| !used[*k] && b[*k].0 == *da && b[*k].2 == *la && alike(ka, &b[*k].1));
        found.map(|k| used[k] = true).is_some()
    })
}

/// [`kinds_match`] without texture projections, where brushes match when they hold the same planes in any order.
fn alike(a: &NodeKind, b: &NodeKind) -> bool {
    let (NodeKind::Brush(x), NodeKind::Brush(y)) = (a, b) else { return kinds_match(a, b, false) };
    if x.faces.len() != y.faces.len() || x.vertices.len() != y.vertices.len() {
        return false;
    }

    let mut used = vec![false; y.faces.len()];
    x.faces.iter().all(|f| {
        let found = y.faces.iter().enumerate().find(|(k, g)| {
            !used[*k]
                && close(f.plane.normal, g.plane.normal)
                && (f.plane.dist - g.plane.dist).abs() < 1e-3
                && values_close(&content(&f.data, false), &content(&g.data, false))
        });
        found.map(|(k, _)| used[k] = true).is_some()
    })
}

fn subtree_changed(before: &Map, after: &Map, group: NodeId) -> bool {
    let now: BTreeSet<NodeId> = std::iter::once(group).chain(after.descendants(group)).collect();
    let then: BTreeSet<NodeId> = if before.contains(group) { std::iter::once(group).chain(before.descendants(group)).collect() } else { BTreeSet::new() };
    if now != then {
        return true;
    }

    now.iter().any(|id| match (before.get(*id), after.get(*id)) {
        (Some(a), Some(b)) => a.kind != b.kind || a.children != b.children || a.label != b.label,
        _ => true,
    })
}

/// Whether the edit changed what `group` holds, rather than only carrying it along with its placement.
fn content_edited(before: &Map, after: &Map, group: NodeId) -> bool {
    if !before.contains(group) || !subtree_changed(before, after, group) {
        return false;
    }

    let now = normalized(after, group);
    if same_content(&now, &normalized(before, group)) {
        return false;
    }

    let carry = group_transform(after, group) * group_transform(before, group).inverse();
    carry == DMat4::IDENTITY || !same_content(&now, &normalized_carried(before, group, &carry))
}

fn link_of(map: &Map, id: NodeId) -> Option<u64> {
    match map.get(id).map(|n| &n.kind) {
        Some(NodeKind::Group(g)) => g.link_id,
        _ => None,
    }
}

/// The link sets of the selected linked groups, and of the nearest linked group around each other selected object.
pub fn sets_touched(map: &Map, sel: &Selection) -> BTreeSet<u64> {
    let objects = sel.nodes.iter().copied().chain(sel.faces.iter().map(|(id, _)| *id));
    objects.filter_map(|id| std::iter::once(id).chain(map.ancestors(id)).find_map(|g| link_of(map, g))).collect()
}

pub fn members(map: &Map, link: u64) -> Vec<NodeId> {
    link_sets(map).remove(&link).unwrap_or_default()
}

/// Whether moving `id` under `dest` would put a linked group inside a copy of itself, or carry an object from one copy
/// of a set into another. [`sync`] cannot settle either: the first nests deeper on every edit, the second loses the
/// object in every copy.
pub fn crosses_copies(map: &Map, id: NodeId, dest: NodeId) -> bool {
    let dest_links: Vec<(NodeId, u64)> = std::iter::once(dest).chain(map.ancestors(dest)).filter_map(|g| link_of(map, g).map(|l| (g, l))).collect();
    if dest_links.is_empty() {
        return false;
    }

    let inside = std::iter::once(id).chain(map.descendants(id)).filter_map(|n| link_of(map, n));
    if inside.into_iter().any(|l| dest_links.iter().any(|(_, d)| *d == l)) {
        return true;
    }

    map.ancestors(id).into_iter().any(|a| link_of(map, a).is_some_and(|l| dest_links.iter().any(|(g, d)| *d == l && *g != a)))
}

/// Link sets in which an edit changed the contents of two or more members that agreed before and now disagree. [`sync`]
/// would keep one of them and drop what the edit did to the others. Texture projections do not count, a UV edit of the
/// same face in several copies lands differently in each placement and [`sync`] recomputes the others from one. A member
/// the edit created does not count, it is a copy rather than an edit, and neither does one only moved as a whole.
pub fn conflicts(before: &Map, after: &Map) -> Vec<u64> {
    link_sets(after)
        .into_iter()
        .filter(|(_, members)| {
            let edited: Vec<NodeId> = members.iter().copied().filter(|g| content_edited(before, after, *g)).collect();
            let [first, rest @ ..] = edited.as_slice() else { return false };
            let content = normalized(after, *first);
            if rest.iter().all(|g| same_shape(&normalized(after, *g), &content)) {
                return false;
            }

            // Copies that disagreed already are settled by sync the way they always were, or they could never be edited again.
            let then = normalized(before, *first);
            rest.iter().all(|g| same_shape(&normalized(before, *g), &then))
        })
        .map(|(link, _)| link)
        .collect()
}

/// Copies the contents of edited linked groups into the other members of their sets. Returns how many groups were rewritten.
pub fn sync(before: &Map, after: &mut Map) -> usize {
    let mut updated = 0;
    let links: Vec<u64> = link_sets(after).into_keys().collect();
    for link in links {
        // Rebuilding an outer set replaces the linked groups nested in its copies, so members are looked up fresh.
        let members = link_sets(after).remove(&link).unwrap_or_default();
        if members.len() < 2 {
            continue;
        }

        let edited = members.iter().find(|g| content_edited(before, after, **g));
        let Some(&source) = edited.or_else(|| members.iter().find(|g| subtree_changed(before, after, **g))) else { continue };
        let source_content = normalized(after, source);
        let source_transform = group_transform(after, source);
        let children = after.get(source).map(|n| n.children.clone()).unwrap_or_default();
        let text = format::nodes_to_string(after, &children);
        for &target in &members {
            if target == source {
                continue;
            }

            if same_content(&normalized(after, target), &source_content) {
                continue;
            }

            for c in after.get(target).map(|n| n.children.clone()).unwrap_or_default() {
                after.remove(c);
            }

            let Ok(ids) = format::paste_nodes(after, target, &text) else { continue };
            let mut sel = Selection::default();
            sel.nodes.extend(ids);
            let m = group_transform(after, target) * source_transform.inverse();
            let leaves = sel.transformables(after);
            ops::transform_nodes(after, &leaves, &m, EditOptions { uv_lock: true, grid: 0.0 });
            // Pasted nested linked groups carry the source's placement, which would pull them back onto the source
            // copy when their own set syncs next.
            ops::update_group_transforms(after, &sel, &m);
            updated += 1;
        }
    }

    updated
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::Document;
    use td_core::{Aabb, DMat3, DVec2};
    use td_geom::{Brush, Mesh};

    #[test]
    fn edits_propagate_to_linked_copies() {
        let mut doc = Document::new();
        let layer = doc.map.default_layer();
        let brush = doc.edit("add", |m, s| {
            let id = m.insert(layer, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::splat(16.0)), "a").unwrap()));
            s.select_node(id);
            id
        });
        let group = doc.edit("group", |m, s| ops::group_selection(m, s, "g", layer)).unwrap();
        let copies = doc.edit("link", |m, s| ops::duplicate_linked(m, s, DVec3::new(100.0, 0.0, 0.0), EditOptions::default()));
        assert_eq!(copies.len(), 1);
        let copy = copies[0];
        assert!((doc.map.bounds(copy).min.x - 100.0).abs() < 1e-9);

        // Moving a linked group as a whole does not touch the others.
        doc.select(|_, s| {
            s.clear();
            s.select_node(copy);
        });
        doc.edit("move", |m, s| ops::translate_selection(m, s, DVec3::new(0.0, 0.0, 50.0), EditOptions::default()));
        assert!((doc.map.bounds(group).min.z).abs() < 1e-9);

        // Editing inside the first group rebuilds the copy at its own placement.
        doc.select(|_, s| {
            s.clear();
            s.select_node(brush);
        });
        doc.edit("scale", |m, s| {
            let b = m.brush(brush).unwrap().bounds();
            let bigger = Aabb::new(b.min, b.max + DVec3::new(16.0, 0.0, 0.0));
            ops::transform_selection(m, s, &ops::scale_bounds(&b, &bigger), EditOptions::default());
        });
        let cb = doc.map.bounds(copy);
        assert!((cb.size().x - 32.0).abs() < 1e-6, "copy resized: {cb:?}");
        assert!((cb.min.x - 100.0).abs() < 1e-6 && (cb.min.z - 50.0).abs() < 1e-6, "copy kept its placement: {cb:?}");
    }

    fn linked_pair(doc: &mut Document) -> (NodeId, NodeId, NodeId) {
        let layer = doc.map.default_layer();
        let brush = doc.edit("add", |m, s| {
            let id = m.insert(layer, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::splat(16.0)), "a").unwrap()));
            s.select_node(id);
            id
        });
        let group = doc.edit("group", |m, s| ops::group_selection(m, s, "window", layer)).unwrap();
        let copy = doc.edit("link", |m, s| ops::duplicate_linked(m, s, DVec3::new(100.0, 0.0, 0.0), EditOptions::default()))[0];
        (brush, group, copy)
    }

    #[test]
    fn drops_never_cross_copies_of_a_linked_set() {
        let mut doc = Document::new();
        let (brush, group, copy) = linked_pair(&mut doc);
        let layer = doc.map.default_layer();
        assert_eq!(ops::drop_plan(&doc.map, &[group], copy, |_| true), None, "a linked group into its own copy");
        assert_eq!(ops::drop_plan(&doc.map, &[brush], copy, |_| true), None, "the only object of a copy would empty it");

        let extra =
            doc.edit("add", |m, _| m.insert(layer, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::splat(200.0), DVec3::splat(216.0)), "b").unwrap())));
        let plan = ops::drop_plan(&doc.map, &[extra], brush, |_| true).unwrap();
        doc.edit("drop", |m, s| ops::drop_nodes(m, s, &[extra], plan));
        assert_eq!(doc.map.get(copy).unwrap().children.len(), 2, "the copy got the object too");
        assert_eq!(ops::drop_plan(&doc.map, &[extra], copy, |_| true), None, "an object from one copy into another");
        assert!(crosses_copies(&doc.map, group, copy));
        assert!(!crosses_copies(&doc.map, extra, group), "staying inside its own copy is fine");

        let inner = doc.edit("inner", |m, _| m.insert(group, NodeKind::Group(crate::map::Group::new("inner"))));
        assert!(ops::drop_plan(&doc.map, &[extra], inner, |_| true).is_some(), "a move inside one copy still works");
        assert_eq!(ops::drop_plan(&doc.map, &[extra], layer, |_| true), Some(ops::DropPlan::Into(layer)), "out of a copy that keeps an object");
    }

    #[test]
    fn a_drop_never_empties_a_linked_copy_through_a_nested_group() {
        let mut doc = Document::new();
        let (brush, group, _) = linked_pair(&mut doc);
        let layer = doc.map.default_layer();
        let inner = doc.edit("nest", |m, _| {
            let inner = m.insert(group, NodeKind::Group(crate::map::Group::new("inner")));
            m.reparent(brush, inner);
            inner
        });
        assert_eq!(doc.map.get(group).unwrap().children, vec![inner]);
        assert_eq!(ops::drop_plan(&doc.map, &[brush], layer, |_| true), None);
    }

    #[test]
    fn face_edits_propagate() {
        let mut doc = Document::new();
        let (brush, _, copy) = linked_pair(&mut doc);
        doc.edit("retexture", |m, _| m.brush_mut(brush).unwrap().faces[0].data.material = "glass".into());
        let other = doc.map.get(copy).unwrap().children[0];
        assert_eq!(doc.map.brush(other).unwrap().faces[0].data.material, "glass");

        let offset = |doc: &Document| doc.map.brush(doc.map.get(copy).unwrap().children[0]).unwrap().faces[1].data.uv.offset;
        let before = offset(&doc);
        doc.edit("shift", |m, _| m.brush_mut(brush).unwrap().faces[1].data.uv.offset.x += 8.0);
        assert!((offset(&doc) - before - td_core::DVec2::new(8.0, 0.0)).length() < 1e-6, "{:?} then {:?}", before, offset(&doc));

        doc.edit("props", |m, _| {
            m.brush_mut(brush).unwrap().faces[2].data.props.insert("surface".into(), "metal".into());
        });
        let other = doc.map.get(copy).unwrap().children[0];
        assert_eq!(doc.map.brush(other).unwrap().faces[2].data.props.get("surface").map(String::as_str), Some("metal"));

        let before = doc.map.get(copy).unwrap().children.clone();
        doc.edit("move", |m, s| {
            s.clear();
            s.select_node(copy);
            ops::translate_selection(m, s, DVec3::new(0.0, 32.0, 0.0), EditOptions::default());
        });
        assert_eq!(doc.map.get(copy).unwrap().children, before, "moving a whole linked group rebuilds nothing");
    }

    #[test]
    fn names_propagate_but_each_copy_keeps_its_own() {
        let mut doc = Document::new();
        let (brush, group, copy) = linked_pair(&mut doc);
        doc.edit("rename", |m, _| m.rename(brush, "sill"));
        let other = doc.map.get(copy).unwrap().children[0];
        assert_eq!(doc.map.get(other).unwrap().name(), "sill");

        doc.edit("rename", |m, _| m.rename(other, ""));
        let rebuilt = doc.map.get(group).unwrap().children[0];
        assert_eq!(doc.map.get(rebuilt).unwrap().label, None, "clearing a name in a copy clears it in the others");

        doc.edit("rename", |m, _| m.rename(copy, "east window"));
        assert_eq!(doc.map.get(copy).unwrap().editable_name(), "east window");
        assert_eq!(doc.map.get(group).unwrap().editable_name(), "window", "a copy is named on its own");
    }

    fn assert_tree_consistent(map: &Map) {
        for (id, node) in map.nodes.iter() {
            match node.parent {
                Some(p) => assert!(map.get(p).is_some_and(|p| p.children.contains(id)), "{id} is an orphan"),
                None => assert!(map.layers.contains(id)),
            }
        }
    }

    #[test]
    fn nested_linked_groups_leave_no_orphans() {
        let mut doc = Document::new();
        let layer = doc.map.default_layer();
        let (brush, window, _) = linked_pair(&mut doc);
        let house = doc.edit("group", |m, s| {
            s.clear();
            s.nodes.extend(m.get(layer).unwrap().children.clone());
            ops::group_selection(m, s, "house", layer)
        });
        let house = house.unwrap();
        let house_copy = doc.edit("link", |m, s| ops::duplicate_linked(m, s, DVec3::new(0.0, 0.0, 200.0), EditOptions::default()))[0];
        // The outer set syncs first when its link id is the lower one, which is when stale inner members bit.
        let groups: Vec<NodeId> = doc.map.nodes.keys().copied().collect();
        for g in groups {
            if let Some(NodeKind::Group(group)) = doc.map.get_mut(g).map(|n| &mut n.kind)
                && group.link_id == Some(1)
            {
                group.link_id = Some(9);
            }
        }

        assert_tree_consistent(&doc.map);

        doc.edit("retexture", |m, _| m.brush_mut(brush).unwrap().faces[0].data.material = "glass".into());
        assert_tree_consistent(&doc.map);
        let materials: Vec<String> = doc.map.brushes().map(|(_, b)| b.faces[0].data.material.clone()).collect();
        assert_eq!(materials.len(), 4, "two windows in each of two houses");
        assert!(materials.iter().all(|m| m == "glass"), "{materials:?}");
        assert!(doc.map.contains(window) && doc.map.contains(house) && doc.map.contains(house_copy));
        assert_eq!(format::from_str(&format::to_string(&doc.map)).unwrap().nodes.len(), doc.map.nodes.len(), "every node is saved");
    }

    pub(crate) fn wedge() -> Brush {
        let points = [
            DVec3::new(8.0, 2.0, 4.0),
            DVec3::new(40.0, 2.0, 4.0),
            DVec3::new(8.0, 2.0, 20.0),
            DVec3::new(40.0, 2.0, 20.0),
            DVec3::new(8.0, 30.0, 4.0),
            DVec3::new(8.0, 34.0, 20.0),
            DVec3::new(40.0, 14.0, 4.0),
            DVec3::new(40.0, 18.0, 20.0),
        ];
        Brush::from_points(&points, &[], "wall").unwrap()
    }

    fn mirror() -> DMat4 {
        ops::flip_about(DVec3::new(40.0, 0.0, 0.0), 0)
    }

    /// Groups `kind` and adds a linked copy mirrored across x = 40, the wedge's max side, so the halves meet.
    fn mirrored_pair(doc: &mut Document, kind: NodeKind) -> (NodeId, NodeId) {
        let layer = doc.map.default_layer();
        doc.edit("add", |m, s| {
            let id = m.insert(layer, kind);
            s.select_node(id);
        });
        let group = doc.edit("group", |m, s| ops::group_selection(m, s, "half", layer)).unwrap();
        let copy = doc.edit("mirror", |m, s| {
            let copy = ops::duplicate_linked(m, s, DVec3::ZERO, EditOptions::default())[0];
            ops::transform_selection(m, s, &mirror(), EditOptions::default());
            copy
        });
        (group, copy)
    }

    fn child(doc: &Document, group: NodeId) -> NodeId {
        let children = &doc.map.get(group).unwrap().children;
        assert_eq!(children.len(), 1, "{children:?}");
        children[0]
    }

    fn facing(b: &Brush, dir: DVec3) -> usize {
        (0..b.faces.len()).max_by(|x, y| b.faces[*x].plane.normal.dot(dir).total_cmp(&b.faces[*y].plane.normal.dot(dir))).unwrap()
    }

    /// Moves one corner, retextures the face looking most along -Z and shifts and turns the top face's texture.
    fn reshape(map: &mut Map, id: NodeId, corner: DVec3, delta: DVec3) {
        let b = map.brush(id).unwrap();
        let mut points = b.vertices.clone();
        let k = points.iter().position(|p| close(*p, corner)).unwrap_or_else(|| panic!("no corner at {corner}"));
        points[k] += delta;
        let mut moved = Brush::from_points(&points, &b.planes(), "wall").unwrap();
        let back = facing(&moved, DVec3::NEG_Z);
        moved.faces[back].data.material = "brick".into();
        let top = facing(&moved, DVec3::Y);
        moved.faces[top].data.uv.offset += DVec2::new(5.0, 3.0);
        moved.faces[top].data.uv.rotate(30.0);
        *map.brush_mut(id).unwrap() = moved;
    }

    /// `copy` is `source` under the rigid or mirroring `m`: the same corners moved, a valid brush wound outward with the
    /// same volume, and every face keeping its data with each texel staying on the moved surface.
    fn assert_brush_maps(source: &Brush, copy: &Brush, m: &DMat4) {
        copy.validate().unwrap();
        assert_eq!(copy.vertices.len(), source.vertices.len());
        for v in &source.vertices {
            let want = m.transform_point3(*v);
            assert!(copy.vertices.iter().any(|p| close(*p, want)), "{want} missing from {:?}", copy.vertices);
        }

        assert!(copy.volume() > 0.0 && (copy.volume() - source.volume()).abs() < 1e-2, "volume {} vs {}", copy.volume(), source.volume());
        let centroid = copy.vertices.iter().sum::<DVec3>() / copy.vertices.len() as f64;
        let linear = DMat3::from_mat4(*m);
        assert_eq!(copy.faces.len(), source.faces.len());
        for (fi, face) in source.faces.iter().enumerate() {
            let center = m.transform_point3(source.face_center(fi));
            let ci = (0..copy.faces.len()).find(|c| close(copy.face_center(*c), center)).unwrap_or_else(|| panic!("no face at {center}"));
            let moved = &copy.faces[ci];
            assert!((moved.plane.normal - linear * face.plane.normal).length() < 1e-4, "face {ci} normal {}", moved.plane.normal);
            assert!(moved.plane.normal.dot(copy.face_center(ci) - centroid) > 0.0, "face {ci} faces inward");
            assert_eq!(moved.data.material, face.data.material);
            assert_eq!(moved.data.props, face.data.props);
            for p in source.face_points(fi) {
                let (a, b) = (face.data.uv.texel(p), moved.data.uv.texel(m.transform_point3(p)));
                assert!((a - b).length() < 1e-3, "face {fi} texel {a} at {p} became {b}");
            }
        }
    }

    /// `copy` is `source` reflected by `m` corner for corner, each face with its winding, corner UVs and colors reversed.
    fn assert_mesh_mirrors(source: &Mesh, copy: &Mesh, m: &DMat4) {
        copy.validate().unwrap();
        assert!(copy.is_closed());
        assert_eq!(copy.vertices.len(), source.vertices.len());
        for (v, p) in source.vertices.iter().zip(&copy.vertices) {
            assert!(close(m.transform_point3(*v), *p), "{v} became {p}");
        }

        assert!(copy.volume() > 0.0 && (copy.volume() - source.volume()).abs() < 1e-6, "volume {} vs {}", copy.volume(), source.volume());
        let centroid = copy.vertices.iter().sum::<DVec3>() / copy.vertices.len() as f64;
        let size = DVec2::new(64.0, 32.0);
        assert_eq!(copy.faces.len(), source.faces.len());
        for (fi, (face, mirrored)) in source.faces.iter().zip(&copy.faces).enumerate() {
            assert_eq!(mirrored.indices, face.indices.iter().rev().copied().collect::<Vec<_>>());
            assert_eq!(mirrored.data.colors, face.data.colors.iter().rev().copied().collect::<Vec<_>>());
            assert_eq!(mirrored.data.material, face.data.material);
            assert!(copy.face_normal(fi).dot(copy.face_center(fi) - centroid) > 0.0, "face {fi} faces inward");
            let n = face.indices.len();
            for k in 0..n {
                let (a, b) = (source.corner_uv(fi, k, size), copy.corner_uv(fi, n - 1 - k, size));
                assert!((a - b).length() < 1e-6, "face {fi} corner {k} uv {a} became {b}");
            }
        }
    }

    #[test]
    fn a_mirrored_copy_follows_vertex_material_and_uv_edits() {
        let mut doc = Document::new();
        let (group, copy) = mirrored_pair(&mut doc, NodeKind::Brush(wedge()));
        assert_brush_maps(doc.map.brush(child(&doc, group)).unwrap(), doc.map.brush(child(&doc, copy)).unwrap(), &mirror());

        let source = child(&doc, group);
        doc.edit("reshape", |m, _| reshape(m, source, DVec3::new(8.0, 2.0, 20.0), DVec3::new(-4.0, -4.0, 4.0)));
        let src = doc.map.brush(source).unwrap();
        assert!(src.vertices.iter().any(|p| close(*p, DVec3::new(4.0, -2.0, 24.0))), "{:?}", src.vertices);
        let mirrored = doc.map.brush(child(&doc, copy)).unwrap();
        assert!(mirrored.vertices.iter().any(|p| close(*p, DVec3::new(76.0, -2.0, 24.0))), "{:?}", mirrored.vertices);
        assert_eq!(mirrored.faces[facing(mirrored, DVec3::NEG_Z)].data.material, "brick");
        assert_brush_maps(src, mirrored, &mirror());
    }

    #[test]
    fn edits_in_the_mirrored_copy_flow_back_mirrored() {
        let mut doc = Document::new();
        let (group, copy) = mirrored_pair(&mut doc, NodeKind::Brush(wedge()));
        let mirrored = child(&doc, copy);
        doc.edit("reshape", |m, _| reshape(m, mirrored, DVec3::new(72.0, 2.0, 4.0), DVec3::new(5.0, -3.0, -2.0)));
        let edited = doc.map.brush(mirrored).unwrap();
        assert!(edited.vertices.iter().any(|p| close(*p, DVec3::new(77.0, -1.0, 2.0))), "{:?}", edited.vertices);
        let original = doc.map.brush(child(&doc, group)).unwrap();
        assert!(original.vertices.iter().any(|p| close(*p, DVec3::new(3.0, -1.0, 2.0))), "{:?}", original.vertices);
        assert_eq!(original.faces[facing(original, DVec3::NEG_Z)].data.material, "brick");
        assert_brush_maps(edited, original, &mirror().inverse());
    }

    #[test]
    fn a_mirrored_mesh_copy_keeps_faces_outward_after_a_vertex_edit() {
        let mut doc = Document::new();
        let mut mesh = Mesh::from_brush(&wedge());
        mesh.faces[2].uvs = vec![[0.0, 0.0], [0.5, 0.0], [0.75, 1.0], [0.0, 0.25]];
        mesh.faces[2].data.colors = vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], [1.0, 1.0, 1.0, 0.5]];
        let (group, copy) = mirrored_pair(&mut doc, NodeKind::Mesh(mesh));
        assert_mesh_mirrors(doc.map.mesh(child(&doc, group)).unwrap(), doc.map.mesh(child(&doc, copy)).unwrap(), &mirror());

        let source = child(&doc, group);
        doc.edit("drag", |m, _| {
            let mesh = m.mesh_mut(source).unwrap();
            let k = mesh.vertices.iter().position(|p| close(*p, DVec3::new(8.0, 2.0, 20.0))).unwrap();
            mesh.vertices[k] += DVec3::new(-4.0, -4.0, 4.0);
        });
        let src = doc.map.mesh(source).unwrap();
        let mirrored = doc.map.mesh(child(&doc, copy)).unwrap();
        assert!(mirrored.vertices.iter().any(|p| close(*p, DVec3::new(76.0, -2.0, 24.0))), "{:?}", mirrored.vertices);
        assert_mesh_mirrors(src, mirrored, &mirror());
    }

    #[test]
    fn a_set_of_three_follows_one_edit() {
        let mut doc = Document::new();
        let (group, mirrored) = mirrored_pair(&mut doc, NodeKind::Brush(wedge()));
        let shift = DVec3::new(12.0, 0.0, 96.0);
        let moved = doc.edit("link", |m, s| {
            s.clear();
            s.select_node(group);
            ops::duplicate_linked(m, s, shift, EditOptions::default())[0]
        });
        let source = child(&doc, group);
        doc.edit("reshape", |m, _| reshape(m, source, DVec3::new(40.0, 14.0, 4.0), DVec3::new(3.0, 5.0, -2.0)));
        let src = doc.map.brush(source).unwrap();
        assert!(src.vertices.iter().any(|p| close(*p, DVec3::new(43.0, 19.0, 2.0))), "{:?}", src.vertices);
        assert_brush_maps(src, doc.map.brush(child(&doc, moved)).unwrap(), &DMat4::from_translation(shift));
        assert_brush_maps(src, doc.map.brush(child(&doc, mirrored)).unwrap(), &mirror());

        let turn = ops::rotation_about(DVec3::new(56.0, 10.0, 12.0), DVec3::Y, 30.0);
        doc.edit("turn", |m, s| {
            s.clear();
            s.select_node(mirrored);
            ops::transform_selection(m, s, &turn, EditOptions::default());
        });
        let source = child(&doc, group);
        doc.edit("reshape", |m, _| reshape(m, source, DVec3::new(8.0, 30.0, 4.0), DVec3::new(-2.0, 6.0, -3.0)));
        let src = doc.map.brush(source).unwrap();
        assert!(src.vertices.iter().any(|p| close(*p, DVec3::new(6.0, 36.0, 1.0))), "{:?}", src.vertices);
        assert_brush_maps(src, doc.map.brush(child(&doc, moved)).unwrap(), &DMat4::from_translation(shift));
        assert_brush_maps(src, doc.map.brush(child(&doc, mirrored)).unwrap(), &(turn * mirror()));
    }

    fn pane() -> Brush {
        let points = [
            DVec3::new(14.0, 10.0, 20.0),
            DVec3::new(22.0, 10.0, 20.0),
            DVec3::new(14.0, 10.0, 23.0),
            DVec3::new(22.0, 10.0, 23.0),
            DVec3::new(14.0, 18.0, 20.0),
            DVec3::new(14.0, 19.0, 23.0),
            DVec3::new(22.0, 16.0, 20.0),
            DVec3::new(22.0, 17.0, 23.0),
        ];
        Brush::from_points(&points, &[], "frame").unwrap()
    }

    const SECOND_WINDOW: DVec3 = DVec3::new(14.0, 2.0, 0.0);

    /// House A holds a wall, a plain trim group and a window group. The house is linked first with its copy placed by
    /// `outer`, then the window gets a linked twin inside house A, so the outer set has the lower link id.
    fn houses(doc: &mut Document, outer: &DMat4) -> (NodeId, NodeId, NodeId) {
        let layer = doc.map.default_layer();
        let window = doc.edit("window", |m, s| {
            s.select_node(m.insert(layer, NodeKind::Brush(pane())));
            ops::group_selection(m, s, "window", layer).unwrap()
        });
        let trim = doc.edit("trim", |m, s| {
            let post = Brush::from_aabb(&Aabb::new(DVec3::new(6.0, 0.0, 2.0), DVec3::new(10.0, 4.0, 22.0)), "trim").unwrap();
            s.clear();
            s.select_node(m.insert(layer, NodeKind::Brush(post)));
            ops::group_selection(m, s, "trim", layer).unwrap()
        });
        let house = doc.edit("house", |m, s| {
            let wall = m.insert(layer, NodeKind::Brush(wedge()));
            s.clear();
            s.nodes.extend([wall, trim, window]);
            ops::group_selection(m, s, "house", layer).unwrap()
        });
        let copy = doc.edit("link house", |m, s| {
            let copy = ops::duplicate_linked(m, s, DVec3::ZERO, EditOptions::default())[0];
            ops::transform_selection(m, s, outer, EditOptions::default());
            copy
        });
        doc.edit("link window", |m, s| {
            s.clear();
            s.select_node(window);
            ops::duplicate_linked(m, s, SECOND_WINDOW, EditOptions::default());
        });
        (house, window, copy)
    }

    fn windows(map: &Map, house: NodeId) -> Vec<NodeId> {
        map.get(house).unwrap().children.iter().copied().filter(|c| link_of(map, *c).is_some()).collect()
    }

    /// `b` is `a` placed by `m`: the same tree, linked groups placed by `m` on top of their counterpart's placement,
    /// plain groups left unplaced and every brush moved by `m`.
    pub(crate) fn assert_copy_maps(map: &Map, a: NodeId, b: NodeId, m: &DMat4) {
        let (x, y) = (map.get(a).unwrap(), map.get(b).unwrap());
        match (&x.kind, &y.kind) {
            (NodeKind::Group(g), NodeKind::Group(h)) => {
                assert_eq!((&g.name, g.link_id), (&h.name, h.link_id));
                let want = if g.link_id.is_some() { *m * g.transform } else { DMat4::IDENTITY };
                assert!(h.transform.abs_diff_eq(want, 1e-6), "{} {b} placed at {:?}, wants {want:?}", h.name, h.transform);
                assert_eq!(x.children.len(), y.children.len(), "{} children", g.name);
                for (c, d) in x.children.iter().zip(&y.children) {
                    assert_copy_maps(map, *c, *d, m);
                }
            }
            (NodeKind::Brush(p), NodeKind::Brush(q)) => assert_brush_maps(p, q, m),
            _ => panic!("{a} and {b} differ in kind"),
        }
    }

    /// Checks the houses right after the window twin is linked and again after the first window's brush is edited,
    /// since both edits rebuild house B and then sync the window set inside it.
    fn nested_windows_follow(outer: DMat4, moved_corner_in_b: DVec3) {
        let mut doc = Document::new();
        let (house, window, copy) = houses(&mut doc, &outer);
        assert!(link_of(&doc.map, house) < link_of(&doc.map, window), "the outer set syncs first");
        for edit in [false, true] {
            if edit {
                let pane = child(&doc, window);
                doc.edit("reshape", |m, _| reshape(m, pane, DVec3::new(22.0, 10.0, 20.0), DVec3::new(2.0, -3.0, -1.0)));
            }

            let (a, b) = (windows(&doc.map, house), windows(&doc.map, copy));
            assert_eq!((a.len(), b.len(), a[0]), (2, 2, window), "two windows in each house, the edited one kept");
            assert_copy_maps(&doc.map, house, copy, &outer);
            assert_copy_maps(&doc.map, a[0], a[1], &DMat4::from_translation(SECOND_WINDOW));
        }

        let first = doc.map.brush(child(&doc, window)).unwrap();
        assert!(first.vertices.iter().any(|p| close(*p, DVec3::new(24.0, 7.0, 19.0))), "{:?}", first.vertices);
        let pane = doc.map.brush(child(&doc, windows(&doc.map, copy)[0])).unwrap();
        assert!(pane.vertices.iter().any(|p| close(*p, moved_corner_in_b)), "{:?}", pane.vertices);
        assert_tree_consistent(&doc.map);
    }

    #[test]
    fn nested_linked_groups_keep_their_place_when_the_outer_copy_is_rebuilt() {
        nested_windows_follow(DMat4::from_translation(DVec3::new(160.0, 8.0, 96.0)), DVec3::new(184.0, 15.0, 115.0));
    }

    #[test]
    fn nested_linked_groups_keep_their_place_in_a_mirrored_copy() {
        nested_windows_follow(mirror(), DVec3::new(56.0, 7.0, 19.0));
    }

    fn moved(map: &Map, ids: &[NodeId], by: DVec3) -> Map {
        moved_with(map, ids, by, true)
    }

    fn moved_with(map: &Map, ids: &[NodeId], by: DVec3, uv_lock: bool) -> Map {
        let mut map = map.clone();
        let mut sel = Selection::default();
        sel.nodes.extend(ids.iter().copied());
        ops::translate_selection(&mut map, &sel, by, EditOptions { uv_lock, ..EditOptions::default() });
        map
    }

    #[test]
    fn conflicts_name_only_sets_whose_changed_members_disagree() {
        let mut doc = Document::new();
        let (group, mirrored) = mirrored_pair(&mut doc, NodeKind::Brush(wedge()));
        let third = doc.edit("link", |m, s| {
            s.clear();
            s.select_node(group);
            ops::duplicate_linked(m, s, DVec3::new(12.0, 0.0, 96.0), EditOptions::default())[0]
        });
        doc.select(|_, s| s.clear());
        let (_, window, window_copy) = linked_pair(&mut doc);
        let (half, other, third_brush) = (child(&doc, group), child(&doc, mirrored), child(&doc, third));
        let (pane, pane_copy) = (child(&doc, window), child(&doc, window_copy));
        let mirror_link = link_of(&doc.map, group).unwrap();
        let window_link = link_of(&doc.map, window).unwrap();
        assert_ne!(mirror_link, window_link);
        let before = doc.map.clone();
        let slant = DVec3::new(5.0, -3.0, 7.0);

        assert_eq!(conflicts(&before, &before), Vec::<u64>::new(), "nothing changed");

        let mut one = before.clone();
        reshape(&mut one, half, DVec3::new(40.0, 14.0, 4.0), DVec3::new(3.0, 5.0, -2.0));
        assert_eq!(conflicts(&before, &one), Vec::<u64>::new(), "one member changed");

        let mut agree = before.clone();
        for id in [half, other] {
            agree.brush_mut(id).unwrap().faces[2].data.material = "brick".into();
        }

        assert_eq!(conflicts(&before, &agree), Vec::<u64>::new(), "the same face of both halves retextured");

        assert_eq!(conflicts(&before, &moved(&before, &[half, other], slant)), vec![mirror_link], "a nudge of both halves reflects differently");
        assert_eq!(conflicts(&before, &moved(&before, &[half, third_brush], slant)), Vec::<u64>::new(), "two translated copies agree");
        assert_eq!(conflicts(&before, &moved(&before, &[other, third_brush], slant)), vec![mirror_link], "two of a set of three disagree");

        let both_sets = moved(&moved(&before, &[pane, pane_copy], slant), &[half, other], slant);
        assert_eq!(conflicts(&before, &both_sets), vec![mirror_link], "the window copies agree, the mirrored halves do not");

        let wholes = moved(&moved(&before, &[group], slant), &[mirrored], DVec3::new(-9.0, 4.0, 2.0));
        assert_eq!(conflicts(&before, &wholes), Vec::<u64>::new(), "halves moved as wholes keep their contents");
        for (by, how) in [(slant, "slanted"), (DVec3::new(16.0, 0.0, 0.0), "along the mirror axis"), (DVec3::new(0.0, 0.0, -24.0), "along the plane")] {
            let unlocked = moved_with(&before, &[group, mirrored, third], by, false);
            assert_eq!(conflicts(&before, &unlocked), Vec::<u64>::new(), "all three moved {how} with UV lock off");
        }

        let mut whole_and_edit = moved(&before, &[group], slant);
        reshape(&mut whole_and_edit, other, DVec3::new(40.0, 14.0, 4.0), DVec3::new(3.0, 5.0, -2.0));
        assert_eq!(conflicts(&before, &whole_and_edit), Vec::<u64>::new(), "one half moved, the other reshaped");
        let unlocked_and_nudged = moved(&moved_with(&before, &[group], slant, false), &[other], slant);
        assert_eq!(conflicts(&before, &unlocked_and_nudged), Vec::<u64>::new(), "one half moved with UV lock off, the other nudged");
        let both_nudged = moved(&moved_with(&before, &[group], slant, false), &[half, other], slant);
        assert_eq!(conflicts(&before, &both_nudged), vec![mirror_link], "a whole move does not hide what moving a brush did");

        assert_eq!(conflicts(&one, &moved(&one, &[half, other], slant)), Vec::<u64>::new(), "copies that disagreed already");
        assert_eq!(conflicts(&one, &moved(&one, &[half, third_brush], slant)), Vec::<u64>::new());
        assert_eq!(conflicts(&agree, &moved(&agree, &[half, other], slant)), vec![mirror_link], "copies that agreed on more than the original");

        let mut created = before.clone();
        reshape(&mut created, half, DVec3::new(40.0, 14.0, 4.0), DVec3::new(3.0, 5.0, -2.0));
        let layer = created.default_layer();
        let extra = created.insert(layer, NodeKind::Group(crate::map::Group { link_id: Some(mirror_link), ..crate::map::Group::new("half") }));
        created.insert(extra, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::splat(200.0), DVec3::splat(216.0)), "b").unwrap()));
        assert_eq!(conflicts(&before, &created), Vec::<u64>::new(), "a member the edit created is not an edit");
        let mut copied = before.clone();
        let mut source = Selection::default();
        source.nodes.insert(group);
        let fourth = ops::duplicate_linked(&mut copied, &mut source, DVec3::new(-200.0, 8.0, 0.0), EditOptions::default())[0];
        let mut edited = copied.clone();
        reshape(&mut edited, half, DVec3::new(40.0, 14.0, 4.0), DVec3::new(1.0, 1.0, 1.0));
        edited.brush_mut(child_in(&edited, fourth)).unwrap().faces[0].data.material = "glass".into();
        assert_eq!(conflicts(&copied, &edited), vec![mirror_link], "once it exists, it counts");
    }

    fn child_in(map: &Map, group: NodeId) -> NodeId {
        map.get(group).unwrap().children[0]
    }

    /// Applies `f` to the projection of the face looking most along +Y, with that face's corners, in the brush of each of
    /// `groups`. A sync replaces the contents of the copies, so the brushes are looked up on every call.
    fn retexture_tops(doc: &mut Document, groups: &[NodeId], f: impl Fn(&mut td_geom::FaceUv, &[DVec3])) {
        let ids: Vec<NodeId> = groups.iter().map(|g| child(doc, *g)).collect();
        doc.edit("uv", |m, _| {
            for id in ids {
                let b = m.brush_mut(id).unwrap();
                let top = facing(b, DVec3::Y);
                let points = b.face_points(top);
                f(&mut b.faces[top].data.uv, &points);
            }
        });
    }

    type UvEdit = Box<dyn Fn(&mut td_geom::FaceUv, &[DVec3])>;

    fn assert_uv_edits_stay(doc: &mut Document, groups: [NodeId; 2]) {
        let tex = DVec2::splat(64.0);
        let edits: [(&str, UvEdit); 4] = [
            ("scale", Box::new(|uv, _| uv.scale *= 2.0)),
            ("fields", Box::new(|uv, _| (uv.offset, uv.rotation) = (DVec2::new(5.0, 3.0), 30.0))),
            ("justify", Box::new(move |uv, p| uv.justify(p, tex, td_geom::Justify::Fit))),
            ("fit", Box::new(move |uv, p| uv.fit(p, tex, DVec2::new(2.0, 1.0)))),
        ];
        for (how, f) in edits {
            let steps = doc.history.undo_labels().count();
            retexture_tops(doc, &groups, f);
            assert!(!doc.take_link_conflict(), "{how} was refused");
            assert_eq!(doc.history.undo_labels().count(), steps + 1, "{how} was not recorded");
        }
    }

    #[test]
    fn uv_edits_across_translated_copies_are_not_conflicts() {
        let mut doc = Document::new();
        let (_, group, copy) = linked_pair(&mut doc);
        assert_uv_edits_stay(&mut doc, [group, copy]);
        let (x, y) = (doc.map.brush(child(&doc, group)).unwrap(), doc.map.brush(child(&doc, copy)).unwrap());
        assert_eq!(x.faces[facing(x, DVec3::Y)].data.uv.scale, y.faces[facing(y, DVec3::Y)].data.uv.scale);
    }

    #[test]
    fn uv_edits_across_mirrored_halves_are_not_conflicts() {
        let mut doc = Document::new();
        let (group, mirrored) = mirrored_pair(&mut doc, NodeKind::Brush(wedge()));
        assert_uv_edits_stay(&mut doc, [group, mirrored]);
    }

    #[test]
    fn a_carve_across_the_mirror_joint_is_not_a_conflict() {
        let mut doc = Document::new();
        let (group, mirrored) = mirrored_pair(&mut doc, NodeKind::Brush(wedge()));
        let layer = doc.map.default_layer();
        let cutter = doc.edit("cutter", |m, s| {
            let id = m.insert(layer, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::new(30.0, 8.0, 8.0), DVec3::new(50.0, 24.0, 16.0)), "cut").unwrap()));
            s.clear();
            s.select_node(id);
            id
        });
        let (cut, _) = doc.edit("carve", |m, s| ops::csg_subtract(m, s, td_geom::csg::CarveMaterial::Target));
        assert_eq!(cut, 2);
        assert!(!doc.take_link_conflict(), "the carve was refused");
        assert!(!doc.map.contains(cutter));
        let (a, b) = (&doc.map.get(group).unwrap().children, &doc.map.get(mirrored).unwrap().children);
        assert_eq!(a.len(), b.len());
        assert!(a.len() > 1);
    }
}
