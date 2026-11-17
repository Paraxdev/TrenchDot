//! Mirrored linked copies: members of a link set placed as reflections of each other across an axis plane, so an edit
//! in one shows mirrored in the other.

use std::collections::{BTreeSet, HashMap, HashSet};

use td_core::{Aabb, AxisPlane, DVec3, NodeId};

use crate::map::{Map, NodeKind};
use crate::ops::{self, EditOptions};
use crate::selection::Selection;
use crate::symmetry::{MIRROR_TOLERANCE, PlaneChoice};

/// A plane between a linked group and a member of its set that mirrors it, as the views draw it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MirrorPlane {
    pub plane: AxisPlane,
    pub group: NodeId,
    pub partner: NodeId,
    /// Around both halves.
    pub span: Aabb,
}

fn link_of(map: &Map, id: NodeId) -> Option<u64> {
    match map.get(id).map(|n| &n.kind) {
        Some(NodeKind::Group(g)) => g.link_id,
        _ => None,
    }
}

/// Gives each selected group a linked copy reflected across the plane `choice` puts on `axis` for the groups' bounds,
/// and selects the copies. Returns None and changes nothing without a selected group or a plane.
pub fn mirror_linked(map: &mut Map, sel: &mut Selection, axis: usize, choice: &PlaneChoice) -> Option<(Vec<NodeId>, AxisPlane)> {
    let is_group = |id: &NodeId| matches!(map.get(*id).map(|n| &n.kind), Some(NodeKind::Group(_)));
    let groups: Vec<NodeId> = ops::selection_roots(map, sel).into_iter().filter(is_group).collect();
    if groups.is_empty() {
        return None;
    }

    let plane = choice.resolve(axis, &map.bounds_of(groups))?;
    // Sync maps edits between copies with UV lock on, a copy made without it would change its textures on the first edit.
    let opts = EditOptions { uv_lock: true, ..EditOptions::default() };
    let copies = ops::duplicate_linked(map, sel, DVec3::ZERO, opts);
    ops::transform_selection(map, sel, &plane.matrix(), opts);
    Some((copies, plane))
}

/// The other members of `group`'s link set placed as its mirror image, each with the plane between them, by id.
pub fn partners(map: &Map, group: NodeId) -> Vec<(NodeId, AxisPlane)> {
    let Some(NodeKind::Group(g)) = map.get(group).map(|n| &n.kind) else { return Vec::new() };
    let Some(link) = g.link_id else { return Vec::new() };
    let members = map.nodes.iter().filter_map(|(id, node)| match &node.kind {
        NodeKind::Group(other) if other.link_id == Some(link) => Some((*id, other)),
        _ => None,
    });
    mirrors(group, g, members)
}

/// The `members` placed as the mirror image of `group`, which is `g`.
fn mirrors<'a>(group: NodeId, g: &crate::Group, members: impl Iterator<Item = (NodeId, &'a crate::Group)>) -> Vec<(NodeId, AxisPlane)> {
    let (inverse, flipped) = (g.transform.inverse(), g.transform.determinant() < 0.0);
    members
        .filter(|(id, other)| *id != group && (other.transform.determinant() < 0.0) != flipped)
        .filter_map(|(id, other)| AxisPlane::of_reflection(&(other.transform * inverse)).map(|p| (id, p)))
        .collect()
}

/// The members of every link set that holds a mirror image, gathered in one walk of the map. A set of plain linked
/// copies, where every transform keeps its handedness, holds none and is left out.
struct MirrorSets<'a>(HashMap<u64, Vec<(NodeId, &'a crate::Group)>>);

impl<'a> MirrorSets<'a> {
    fn of(map: &'a Map) -> Self {
        let mut sets: HashMap<u64, Vec<(NodeId, &crate::Group)>> = HashMap::new();
        for (id, node) in map.nodes.iter() {
            if let NodeKind::Group(g) = &node.kind
                && let Some(link) = g.link_id
            {
                sets.entry(link).or_default().push((*id, g));
            }
        }

        sets.retain(|_, members| {
            let flipped = members.iter().filter(|(_, g)| g.transform.determinant() < 0.0).count();
            flipped > 0 && flipped < members.len()
        });
        MirrorSets(sets)
    }

    fn partners(&self, map: &Map, group: NodeId) -> Vec<(NodeId, AxisPlane)> {
        let Some(NodeKind::Group(g)) = map.get(group).map(|n| &n.kind) else { return Vec::new() };
        let Some(members) = g.link_id.and_then(|link| self.0.get(&link)) else { return Vec::new() };
        mirrors(group, g, members.iter().copied())
    }
}

/// The partner on `axis` whose plane is nearest `bounds`. In a row of touching pairs both neighbours of a half are
/// nearest, then a `selected` one wins, then the one that leaves an even number of halves beyond it to pair up among
/// themselves, then the lowest id.
fn nearest(
    found: &[(NodeId, AxisPlane)],
    axis: usize,
    bounds: &Aabb,
    selected: impl Fn(&NodeId) -> bool,
    meeting: impl Fn(NodeId) -> Vec<(NodeId, AxisPlane)>,
) -> Option<(NodeId, AxisPlane)> {
    let gap = |p: &AxisPlane| (bounds.min[axis] - p.at).max(p.at - bounds.max[axis]).max(0.0);
    let on_axis = found.iter().filter(|(_, p)| p.axis == axis);
    let least = on_axis.clone().map(|(_, p)| gap(p)).min_by(f64::total_cmp)?;
    let tied: Vec<(NodeId, AxisPlane)> = on_axis.filter(|(_, p)| gap(p) <= least + 1e-6).copied().collect();
    if let [one] = tied[..] {
        return Some(one);
    }

    tied.into_iter().min_by_key(|(o, p)| (!selected(o), beyond(*o, *p, &meeting) % 2 == 0, *o))
}

/// How many halves line up from `next` on, each meeting the one before across another plane on the axis of `plane`.
fn beyond(next: NodeId, plane: AxisPlane, meeting: impl Fn(NodeId) -> Vec<(NodeId, AxisPlane)>) -> usize {
    let (mut walked, mut across) = (vec![next], plane);
    while let Some((other, p)) =
        meeting(walked[walked.len() - 1]).into_iter().find(|(o, p)| p.axis == across.axis && (p.at - across.at).abs() >= 1e-6 && !walked.contains(o))
    {
        walked.push(other);
        across = p;
    }

    walked.len()
}

/// The `found` partners of `group` whose plane touches both halves within `dist`.
fn touching(map: &Map, group: NodeId, found: Vec<(NodeId, AxisPlane)>, dist: f64) -> Vec<(NodeId, AxisPlane)> {
    let bounds = map.bounds(group);
    found.into_iter().filter(|(other, plane)| plane.meets(&bounds, dist) && plane.meets(&map.bounds(*other), dist)).collect()
}

/// Mirror planes of the selected linked groups and of the linked groups around a selected node. Per group and axis only
/// the plane nearest the group counts, so in a row of mirrored pairs each pair shows its own plane rather than one
/// plane per far copy.
pub fn selection_planes(map: &Map, sel: &Selection) -> Vec<MirrorPlane> {
    // A node seen before had its ancestors walked already, so a big selection visits each node once.
    let (mut seen, mut groups) = (HashSet::new(), BTreeSet::new());
    for id in &sel.nodes {
        let mut at = Some(*id);
        while let Some(node) = at.filter(|n| seen.insert(*n)) {
            if link_of(map, node).is_some() {
                groups.insert(node);
            }

            at = map.get(node).and_then(|n| n.parent);
        }
    }

    // The walk of every map node waits until a selected group could have a mirror image at all.
    let sets = if groups.is_empty() { MirrorSets(HashMap::new()) } else { MirrorSets::of(map) };
    let mut planes: Vec<MirrorPlane> = Vec::new();
    for &group in &groups {
        let found = sets.partners(map, group);
        if found.is_empty() {
            continue;
        }

        let bounds = map.bounds(group);
        let meeting = |g: NodeId| touching(map, g, sets.partners(map, g), MIRROR_TOLERANCE);
        for axis in 0..3 {
            let Some((partner, plane)) = nearest(&found, axis, &bounds, |o| groups.contains(o), meeting) else { continue };
            let span = bounds.union(&map.bounds(partner));
            let drawn = planes.iter().any(|m| (m.group, m.partner) == (partner, group));
            if !span.is_empty() && !drawn {
                planes.push(MirrorPlane { plane, group, partner, span });
            }
        }
    }

    planes
}

#[cfg(test)]
mod tests {
    use td_core::DMat4;
    use td_geom::Brush;

    use super::*;
    use crate::Document;
    use crate::linked::tests::{assert_copy_maps, wedge};
    use crate::symmetry::PlaneAt;

    const WEDGE_MIN: DVec3 = DVec3::new(8.0, 2.0, 4.0);
    const WEDGE_MAX: DVec3 = DVec3::new(40.0, 34.0, 20.0);

    fn group_of(doc: &mut Document, brush: Brush, name: &str) -> NodeId {
        let layer = doc.map.default_layer();
        doc.edit("add", |m, s| {
            s.clear();
            s.select_node(m.insert(layer, NodeKind::Brush(brush)));
            ops::group_selection(m, s, name, layer).unwrap()
        })
    }

    fn select(doc: &mut Document, ids: &[NodeId]) {
        doc.select(|_, s| {
            s.clear();
            s.nodes.extend(ids);
        });
    }

    fn mirror(doc: &mut Document, ids: &[NodeId], axis: usize, at: PlaneAt) -> (Vec<NodeId>, AxisPlane) {
        select(doc, ids);
        doc.edit("Mirror Linked", |m, s| mirror_linked(m, s, axis, &PlaneChoice { at, custom: 100.25 })).unwrap()
    }

    fn shifted(doc: &mut Document, ids: &[NodeId], offset: DVec3) -> Vec<NodeId> {
        select(doc, ids);
        doc.edit("link", |m, s| ops::duplicate_linked(m, s, offset, EditOptions::default()))
    }

    fn transform(map: &Map, id: NodeId) -> DMat4 {
        match &map.get(id).unwrap().kind {
            NodeKind::Group(g) => g.transform,
            _ => panic!("{id} is no group"),
        }
    }

    fn plane(axis: usize, at: f64) -> AxisPlane {
        AxisPlane { axis, at }
    }

    #[test]
    fn axis_plane_matrix_is_a_flip_about_a_point_on_it() {
        let point = DVec3::new(37.5, -12.25, 3.0);
        for axis in 0..3 {
            let at = AxisPlane { axis, at: point[axis] };
            assert!(at.matrix().abs_diff_eq(ops::flip_about(point, axis), 1e-12), "axis {axis}");
        }
    }

    #[test]
    fn mirror_linked_puts_the_copy_where_each_plane_choice_says() {
        let choices = [
            (PlaneAt::Center, (WEDGE_MIN + WEDGE_MAX) * 0.5),
            (PlaneAt::Max, WEDGE_MAX),
            (PlaneAt::Min, WEDGE_MIN),
            (PlaneAt::World, DVec3::ZERO),
            (PlaneAt::Custom, DVec3::splat(100.25)),
        ];
        for (at, want) in choices {
            for axis in 0..3 {
                let mut doc = Document::new();
                let group = group_of(&mut doc, wedge(), "half");
                let before = doc.map.nodes.clone();
                let (copies, found) = mirror(&mut doc, &[group], axis, at);
                assert_eq!(found, plane(axis, want[axis]), "{at:?}");
                assert_eq!(copies.len(), 1);
                let copy = copies[0];
                assert_eq!(doc.selection.nodes.iter().copied().collect::<Vec<_>>(), [copy]);
                assert!(link_of(&doc.map, group).is_some());
                assert_copy_maps(&doc.map, group, copy, &found.matrix());
                assert_eq!(partners(&doc.map, group), [(copy, found)], "{at:?} on {axis}");
                assert_eq!(partners(&doc.map, copy), [(group, found)], "{at:?} on {axis}");

                assert_eq!(doc.undo().as_deref(), Some("Mirror Linked"));
                assert!(doc.map.nodes == before, "one undo step removes the copy and the link");
            }
        }
    }

    #[test]
    fn moving_one_half_keeps_the_plane_halfway_between_them() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        let (copies, found) = mirror(&mut doc, &[group], 0, PlaneAt::Max);
        let copy = copies[0];
        let move_copy = |doc: &mut Document, offset: DVec3| {
            select(doc, &[copy]);
            doc.edit("move", |m, s| ops::translate_selection(m, s, offset, EditOptions::default()));
            partners(&doc.map, group)
        };
        assert_eq!(move_copy(&mut doc, DVec3::new(64.0, 0.0, 0.0)), [(copy, plane(0, found.at + 32.0))]);
        assert_eq!(move_copy(&mut doc, DVec3::new(0.0, 8.0, 0.0)), [], "a slide along the plane is no plain mirror");
    }

    #[test]
    fn mirror_linked_needs_a_selected_group_and_a_plane() {
        let mut doc = Document::new();
        let layer = doc.map.default_layer();
        let loose = doc.edit("add", |m, _| m.insert(layer, NodeKind::Brush(wedge())));
        let empty = doc.edit("empty", |m, _| m.insert(layer, NodeKind::Group(crate::Group::new("empty"))));
        let before = doc.map.nodes.clone();
        for (ids, at) in [(&[][..], PlaneAt::World), (&[loose][..], PlaneAt::World), (&[empty][..], PlaneAt::Max), (&[empty][..], PlaneAt::Center)] {
            select(&mut doc, ids);
            let mut sel = doc.selection.clone();
            assert_eq!(mirror_linked(&mut doc.map, &mut sel, 0, &PlaneChoice { at, custom: 0.0 }), None, "{ids:?} {at:?}");
            assert!(doc.map.nodes == before, "{ids:?} {at:?} changed the map");
            assert_eq!(sel, doc.selection, "{ids:?} {at:?} changed the selection");
        }

        // The world origin needs no bounds, so even an empty group gets its mirrored twin.
        let (copies, found) = mirror(&mut doc, &[empty], 1, PlaneAt::World);
        assert_eq!((copies.len(), found), (1, plane(1, 0.0)));
        assert_eq!(partners(&doc.map, empty), [(copies[0], found)]);
        select(&mut doc, &[empty]);
        assert_eq!(selection_planes(&doc.map, &doc.selection), [], "an empty pair spans nothing to draw");
    }

    #[test]
    fn each_selected_group_gets_a_copy_and_loose_objects_stay() {
        let mut doc = Document::new();
        let layer = doc.map.default_layer();
        let a = group_of(&mut doc, wedge(), "a");
        let post = Brush::from_aabb(&Aabb::new(DVec3::new(-24.0, 0.0, 30.0), DVec3::new(-16.0, 48.0, 38.0)), "trim").unwrap();
        let b = group_of(&mut doc, post, "b");
        let far = Brush::from_aabb(&Aabb::new(DVec3::splat(200.0), DVec3::splat(260.0)), "far").unwrap();
        let loose = doc.edit("add", |m, _| m.insert(layer, NodeKind::Brush(far)));
        let (copies, found) = mirror(&mut doc, &[a, b, loose], 2, PlaneAt::Max);
        assert_eq!(found, plane(2, 38.0), "the loose brush is not mirrored, so it does not move the plane");
        assert_eq!(copies.len(), 2);
        assert_eq!(doc.map.bounds(loose), Aabb::new(DVec3::splat(200.0), DVec3::splat(260.0)));
        let (la, lb) = (link_of(&doc.map, a).unwrap(), link_of(&doc.map, b).unwrap());
        assert_ne!(la, lb);
        assert_eq!([link_of(&doc.map, copies[0]), link_of(&doc.map, copies[1])], [Some(la), Some(lb)]);
        assert_copy_maps(&doc.map, a, copies[0], &found.matrix());
        assert_copy_maps(&doc.map, b, copies[1], &found.matrix());

        // Members of one set mirror into that set, each copy placed as its own original's reflection.
        let shift = DVec3::new(0.0, 0.0, -64.0);
        let moved = shifted(&mut doc, &[a], shift)[0];
        let (copies, found) = mirror(&mut doc, &[a, moved], 0, PlaneAt::Min);
        assert_eq!(found, plane(0, 8.0));
        assert_eq!(copies.iter().map(|c| link_of(&doc.map, *c)).collect::<Vec<_>>(), [Some(la), Some(la)]);
        assert!(transform(&doc.map, copies[0]).abs_diff_eq(found.matrix(), 1e-9));
        assert!(transform(&doc.map, copies[1]).abs_diff_eq(found.matrix() * DMat4::from_translation(shift), 1e-9));
        assert_copy_maps(&doc.map, moved, copies[1], &found.matrix());
    }

    #[test]
    fn partners_are_the_members_placed_as_a_reflection() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        let plain = group_of(&mut doc, wedge(), "plain");
        assert_eq!(partners(&doc.map, group), []);
        assert_eq!(partners(&doc.map, plain), []);
        assert_eq!(partners(&doc.map, doc.map.get(group).unwrap().children[0]), [], "a brush has no partners");
        assert_eq!(partners(&doc.map, NodeId(9999)), []);

        let mirrored = mirror(&mut doc, &[group], 0, PlaneAt::Max).0[0];
        let moved = shifted(&mut doc, &[group], DVec3::new(64.0, 0.0, 0.0))[0];
        let slid = shifted(&mut doc, &[group], DVec3::new(0.0, 0.0, 96.0))[0];
        assert_eq!(partners(&doc.map, group), [(mirrored, plane(0, 40.0))]);
        assert_eq!(partners(&doc.map, moved), [(mirrored, plane(0, 72.0))], "x' = 80 - (x - 64)");
        assert_eq!(partners(&doc.map, slid), [], "a reflection plus a slide along the plane is no mirror");
        assert_eq!(partners(&doc.map, mirrored), [(group, plane(0, 40.0)), (moved, plane(0, 72.0))]);
        assert_eq!(partners(&doc.map, plain), []);

        select(&mut doc, &[mirrored]);
        let turn = ops::rotation_about(DVec3::new(56.0, 10.0, 12.0), DVec3::Y, 30.0);
        doc.edit("turn", |m, s| ops::transform_selection(m, s, &turn, EditOptions::default()));
        assert_eq!(partners(&doc.map, group), []);
        assert_eq!(partners(&doc.map, mirrored), []);
    }

    /// Three mirrored pairs along x, each copy of the first pair 96 units further: halves at 8..40 | 40..72,
    /// 104..136 | 136..168 and 200..232 | 232..264. Every left half also mirrors every right half across a far plane.
    fn facade(doc: &mut Document) -> [[NodeId; 2]; 3] {
        let a = group_of(doc, wedge(), "window");
        let b = mirror(doc, &[a], 0, PlaneAt::Max).0[0];
        let one = shifted(doc, &[a, b], DVec3::new(96.0, 0.0, 0.0));
        let two = shifted(doc, &[a, b], DVec3::new(192.0, 0.0, 0.0));
        [[a, b], [one[0], one[1]], [two[0], two[1]]]
    }

    fn span(min_x: f64, max_x: f64) -> Aabb {
        Aabb::new(DVec3::new(min_x, WEDGE_MIN.y, WEDGE_MIN.z), DVec3::new(max_x, WEDGE_MAX.y, WEDGE_MAX.z))
    }

    /// `count` halves of mirrored pairs placed end to end along x from x 8, each 32 wide, the last one alone when
    /// `count` is odd. Each half also mirrors the touching half of the next pair across their contact plane.
    fn touching_row(doc: &mut Document, count: usize) -> Vec<NodeId> {
        let a = group_of(doc, wedge(), "window");
        let mut row = vec![a, mirror(doc, &[a], 0, PlaneAt::Max).0[0]];
        while row.len() < count {
            let pair = row[..(count - row.len()).min(2)].to_vec();
            row.extend(shifted(doc, &pair, DVec3::new(32.0 * row.len() as f64, 0.0, 0.0)));
        }

        row
    }

    #[test]
    fn selection_planes_keep_the_nearest_partner_per_axis() {
        let mut doc = Document::new();
        let [[a, b], [c, d], [e, f]] = facade(&mut doc);
        assert_eq!(partners(&doc.map, c), [(b, plane(0, 88.0)), (d, plane(0, 136.0)), (f, plane(0, 184.0))]);
        let planes = |doc: &mut Document, ids: &[NodeId]| {
            select(doc, ids);
            selection_planes(&doc.map, &doc.selection)
        };
        assert_eq!(planes(&mut doc, &[c]), [MirrorPlane { plane: plane(0, 136.0), group: c, partner: d, span: span(104.0, 168.0) }]);
        assert_eq!(planes(&mut doc, &[f]), [MirrorPlane { plane: plane(0, 232.0), group: f, partner: e, span: span(200.0, 264.0) }]);
        assert_eq!(planes(&mut doc, &[c, d]), [MirrorPlane { plane: plane(0, 136.0), group: c, partner: d, span: span(104.0, 168.0) }], "a pair draws once");
        assert_eq!(
            planes(&mut doc, &[a, e]),
            [
                MirrorPlane { plane: plane(0, 40.0), group: a, partner: b, span: span(8.0, 72.0) },
                MirrorPlane { plane: plane(0, 232.0), group: e, partner: f, span: span(200.0, 264.0) },
            ]
        );

        let leaf = doc.map.get(d).unwrap().children[0];
        assert_eq!(planes(&mut doc, &[leaf]), [MirrorPlane { plane: plane(0, 136.0), group: d, partner: c, span: span(104.0, 168.0) }], "inside an open half");
        assert_eq!(planes(&mut doc, &[]), []);
        let everything: Vec<NodeId> = doc.map.nodes.keys().copied().collect();
        let pairs = planes(&mut doc, &everything).iter().map(|m| (m.group, m.partner, m.plane)).collect::<Vec<_>>();
        assert_eq!(pairs, [(a, b, plane(0, 40.0)), (c, d, plane(0, 136.0)), (e, f, plane(0, 232.0))], "select all");
    }

    #[test]
    fn selection_planes_pair_touching_halves_the_way_apply_mirror_does() {
        let mut doc = Document::new();
        let [a, b, c, d, e, f]: [NodeId; 6] = touching_row(&mut doc, 6).try_into().unwrap();
        let mut planes = |ids: &[NodeId]| {
            select(&mut doc, ids);
            selection_planes(&doc.map, &doc.selection).iter().map(|m| (m.group, m.partner, m.plane)).collect::<Vec<_>>()
        };
        assert_eq!(planes(&[c]), [(c, d, plane(0, 104.0))]);
        assert_eq!(planes(&[d]), [(d, c, plane(0, 104.0))]);
        assert_eq!(planes(&[b, c]), [(b, c, plane(0, 72.0))], "a selected neighbour wins");
        let all = planes(&[a, b, c, d, e, f]);
        assert_eq!(all, [(a, b, plane(0, 40.0)), (c, d, plane(0, 104.0)), (e, f, plane(0, 168.0))]);
    }

    #[test]
    fn quarters_show_both_planes_and_turned_or_plain_groups_none() {
        let mut doc = Document::new();
        let a = group_of(&mut doc, wedge(), "quarter");
        let b = mirror(&mut doc, &[a], 0, PlaneAt::Max).0[0];
        let [c, d]: [NodeId; 2] = mirror(&mut doc, &[a, b], 2, PlaneAt::Min).0.try_into().unwrap();
        let quarter = |min: DVec3, max: DVec3| Aabb::new(min, max);
        let whole = |doc: &Document| selection_planes(&doc.map, &doc.selection);
        select(&mut doc, &[a]);
        assert_eq!(
            whole(&doc),
            [
                MirrorPlane { plane: plane(0, 40.0), group: a, partner: b, span: quarter(WEDGE_MIN, DVec3::new(72.0, 34.0, 20.0)) },
                MirrorPlane { plane: plane(2, 4.0), group: a, partner: c, span: quarter(DVec3::new(8.0, 2.0, -12.0), WEDGE_MAX) },
            ]
        );
        assert_eq!(partners(&doc.map, d).iter().map(|(id, p)| (*id, p.axis)).collect::<Vec<_>>(), [(b, 2), (c, 0)], "a is half a turn away from d");

        let plain = group_of(&mut doc, wedge(), "plain");
        let turned = shifted(&mut doc, &[plain], DVec3::ZERO)[0];
        let turn = ops::rotation_about(DVec3::new(40.0, 0.0, 4.0), DVec3::Y, 90.0);
        doc.edit("turn", |m, s| ops::transform_selection(m, s, &turn, EditOptions::default()));
        select(&mut doc, &[plain, turned]);
        assert_eq!(whole(&doc), []);
        doc.edit("unlink", |m, s| ops::unlink_groups(m, s));
        assert_eq!(whole(&doc), []);
    }
}
