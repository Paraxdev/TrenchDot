//! Find entities by class, key and value, and replace one material with another, over the map or the selection.

use std::collections::{BTreeMap, BTreeSet};

use td_core::NodeId;

use crate::blend::BLEND_MATERIAL;
use crate::entity::Entity;
use crate::map::{Map, NodeKind};
use crate::selection::Selection;

/// What entities have to match. Empty fields are ignored, the others all have to match, case does not matter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntityQuery {
    pub classname: String,
    pub key: String,
    pub value: String,
    /// The whole text has to match instead of a part of it.
    pub exact: bool,
}

impl EntityQuery {
    pub fn is_empty(&self) -> bool {
        [&self.classname, &self.key, &self.value].iter().all(|s| s.trim().is_empty())
    }

    fn text_matches(&self, pattern: &str, text: &str) -> bool {
        let (pattern, text) = (pattern.trim().to_lowercase(), text.to_lowercase());
        if self.exact { text == pattern } else { text.contains(&pattern) }
    }

    /// A key and a value given together have to match on the same property.
    pub fn matches(&self, e: &Entity) -> bool {
        if self.is_empty() {
            return false;
        }

        let class = self.classname.trim();
        if !class.is_empty() && !self.text_matches(class, &e.classname) {
            return false;
        }

        let (key, value) = (self.key.trim(), self.value.trim());
        if key.is_empty() && value.is_empty() {
            return true;
        }

        e.properties.iter().any(|(k, v)| (key.is_empty() || self.text_matches(key, k)) && (value.is_empty() || self.text_matches(value, v)))
    }
}

/// Entities matching the query in outliner order. Hidden and locked ones are left out, they cannot be selected.
pub fn find_entities(map: &Map, query: &EntityQuery) -> Vec<NodeId> {
    if query.is_empty() {
        return Vec::new();
    }

    map.walk().into_iter().filter(|id| map.is_editable(*id) && map.get(*id).and_then(|n| n.entity()).is_some_and(|e| query.matches(e))).collect()
}

/// Where a material replacement reaches.
#[derive(Clone, Copy, Debug)]
pub enum MaterialScope<'a> {
    Map,
    /// The selected faces, or the geometry of the selected nodes when no face is selected.
    Selection(&'a Selection),
}

/// What a replacement changed, or would change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changed {
    pub faces: usize,
    pub terrain_layers: usize,
    /// Material overrides of scatter sets and their palette entries.
    pub scatter_materials: usize,
}

impl Changed {
    pub fn total(&self) -> usize {
        self.faces + self.terrain_layers + self.scatter_materials
    }
}

enum Hit {
    Face(NodeId, usize),
    Layer(NodeId, usize),
    /// A scatter set's own override, or the one of the palette entry.
    Scatter(NodeId, Option<usize>),
}

type FaceSet = BTreeSet<(NodeId, usize)>;

fn is_blend_of(props: &BTreeMap<String, String>, from: &str) -> bool {
    props.get(BLEND_MATERIAL).is_some_and(|m| m == from)
}

/// The faces (their blend material too), terrain layers and scatter overrides in scope that use `from`.
fn hits(map: &Map, from: &str, scope: MaterialScope) -> Vec<Hit> {
    let (nodes, faces_only): (BTreeSet<NodeId>, Option<&FaceSet>) = match scope {
        MaterialScope::Map => (map.nodes.keys().copied().collect(), None),
        MaterialScope::Selection(s) if s.has_faces() => (s.faces.iter().map(|(id, _)| *id).collect(), Some(&s.faces)),
        MaterialScope::Selection(s) => (s.nodes.iter().flat_map(|id| std::iter::once(*id).chain(map.descendants(*id))).collect(), None),
    };
    let uses = |d: &td_geom::FaceData| d.material == from || is_blend_of(&d.props, from);
    let mut out = Vec::new();
    for id in nodes {
        let wanted = |face: usize| faces_only.is_none_or(|f| f.contains(&(id, face)));
        match map.get(id).map(|n| &n.kind) {
            Some(NodeKind::Brush(b)) => out.extend(b.faces.iter().enumerate().filter(|(i, f)| wanted(*i) && uses(&f.data)).map(|(i, _)| Hit::Face(id, i))),
            Some(NodeKind::Mesh(m)) => out.extend(m.faces.iter().enumerate().filter(|(i, f)| wanted(*i) && uses(&f.data)).map(|(i, _)| Hit::Face(id, i))),
            Some(NodeKind::Terrain(t)) if faces_only.is_none() => {
                out.extend(t.layers.iter().enumerate().filter(|(_, l)| l.material == from).map(|(i, _)| Hit::Layer(id, i)))
            }
            Some(NodeKind::Scatter(s)) if faces_only.is_none() => {
                let own = (s.material.as_deref() == Some(from)).then_some(Hit::Scatter(id, None));
                let items = s.items.iter().enumerate().filter(|(_, it)| it.material.as_deref() == Some(from)).map(|(i, _)| Hit::Scatter(id, Some(i)));
                out.extend(own.into_iter().chain(items));
            }
            _ => {}
        }
    }

    out
}

fn tally(hits: &[Hit]) -> Changed {
    let count = |f: fn(&Hit) -> bool| hits.iter().filter(|h| f(h)).count();
    Changed {
        faces: count(|h| matches!(h, Hit::Face(..))),
        terrain_layers: count(|h| matches!(h, Hit::Layer(..))),
        scatter_materials: count(|h| matches!(h, Hit::Scatter(..))),
    }
}

/// How many faces, terrain layers and scatter overrides `replace_material` would change.
pub fn count_material(map: &Map, from: &str, scope: MaterialScope) -> Changed {
    tally(&hits(map, from, scope))
}

/// Replaces `from` by `to` on the faces in scope, their blend material, terrain layers and scatter overrides too. Only
/// the material name changes, so every face keeps its offset, scale and rotation.
pub fn replace_material(map: &mut Map, from: &str, to: &str, scope: MaterialScope) -> Changed {
    let found = hits(map, from, scope);
    for hit in &found {
        match *hit {
            Hit::Face(id, i) => {
                let data = match map.get_mut(id).map(|n| &mut n.kind) {
                    Some(NodeKind::Brush(b)) => &mut b.faces[i].data,
                    Some(NodeKind::Mesh(m)) => &mut m.faces[i].data,
                    _ => continue,
                };
                if data.material == from {
                    data.material = to.to_string();
                }

                if is_blend_of(&data.props, from) {
                    data.props.insert(BLEND_MATERIAL.to_string(), to.to_string());
                }
            }
            Hit::Layer(id, i) => {
                if let Some(NodeKind::Terrain(t)) = map.get_mut(id).map(|n| &mut n.kind) {
                    t.layers[i].material = to.to_string();
                }
            }
            Hit::Scatter(id, item) => {
                if let Some(s) = map.scatter_mut(id) {
                    let slot = match item {
                        Some(i) => &mut s.items[i].material,
                        None => &mut s.material,
                    };
                    *slot = Some(to.to_string());
                }
            }
        }
    }

    tally(&found)
}

/// Every material the map uses with the faces, terrain layers and scatter overrides that use it.
pub fn materials_in_use(map: &Map) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    let mut add = |name: &str| {
        if !name.is_empty() {
            *out.entry(name.to_string()).or_default() += 1;
        }
    };
    for (_, b) in map.brushes() {
        b.faces.iter().for_each(|f| add(&f.data.material));
    }

    for (_, m) in map.meshes() {
        m.faces.iter().for_each(|f| add(&f.data.material));
    }

    for (_, t) in map.terrains() {
        t.layers.iter().for_each(|l| add(&l.material));
    }

    for (_, s) in map.scatters() {
        s.material.iter().chain(s.items.iter().filter_map(|i| i.material.as_ref())).for_each(|m| add(m));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scatter::{Scatter, ScatterItem, ScatterKind};
    use td_core::{Aabb, DVec2, DVec3};
    use td_geom::{Brush, Terrain};

    fn entity(m: &mut Map, class: &str, props: &[(&str, &str)]) -> NodeId {
        let mut e = Entity::new(class);
        e.properties = props.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let layer = m.default_layer();
        m.insert(layer, NodeKind::Entity(e))
    }

    fn query(class: &str, key: &str, value: &str, exact: bool) -> EntityQuery {
        EntityQuery { classname: class.into(), key: key.into(), value: value.into(), exact }
    }

    fn brush(m: &mut Map, material: &str) -> NodeId {
        let layer = m.default_layer();
        m.insert(layer, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::splat(32.0)), material).unwrap()))
    }

    #[test]
    fn entities_match_by_part_of_class_key_and_value_ignoring_case() {
        let mut m = Map::new();
        let door = entity(&mut m, "func_door", &[("targetname", "Hall_Door"), ("speed", "100")]);
        let light = entity(&mut m, "light", &[("color", "1 0.5 0.2"), ("targetname", "lamp")]);
        let button = entity(&mut m, "func_button", &[("target", "hall_door")]);
        assert_eq!(find_entities(&m, &query("FUNC_", "", "", false)), vec![door, button]);
        assert_eq!(find_entities(&m, &query("", "name", "", false)), vec![door, light]);
        assert_eq!(find_entities(&m, &query("", "", "hall_door", false)), vec![door, button]);
        assert_eq!(find_entities(&m, &query("", "target", "hall", false)), vec![door, button]);
        assert_eq!(find_entities(&m, &query("", "targetname", "hall", false)), vec![door]);
        assert!(find_entities(&m, &query("light", "targetname", "100", false)).is_empty());
    }

    #[test]
    fn a_key_and_a_value_have_to_match_on_one_property() {
        let mut m = Map::new();
        let a = entity(&mut m, "info_target", &[("targetname", "x"), ("note", "spawn")]);
        assert!(find_entities(&m, &query("", "targetname", "spawn", false)).is_empty());
        assert_eq!(find_entities(&m, &query("", "note", "spawn", false)), vec![a]);
    }

    #[test]
    fn exact_matching_wants_the_whole_text() {
        let mut m = Map::new();
        let door = entity(&mut m, "func_door", &[("targetname", "door")]);
        entity(&mut m, "func_door_rotating", &[("targetname", "door_2")]);
        assert_eq!(find_entities(&m, &query("func_door", "", "", true)), vec![door]);
        assert_eq!(find_entities(&m, &query("", "", "DOOR", true)), vec![door]);
        assert_eq!(find_entities(&m, &query("func_door", "", "", false)).len(), 2);
    }

    #[test]
    fn an_empty_query_finds_nothing_and_hidden_entities_are_left_out() {
        let mut m = Map::new();
        let a = entity(&mut m, "light", &[]);
        assert!(find_entities(&m, &query(" ", "", "", false)).is_empty());
        assert_eq!(find_entities(&m, &query("light", "", "", false)), vec![a]);
        m.get_mut(a).unwrap().hidden = true;
        assert!(find_entities(&m, &query("light", "", "", false)).is_empty());
    }

    #[test]
    fn replacing_keeps_the_alignment_of_every_face() {
        let mut m = Map::new();
        let id = brush(&mut m, "brick");
        let uv = {
            let b = m.brush_mut(id).unwrap();
            b.faces[0].data.uv.offset = DVec2::new(12.0, -3.0);
            b.faces[0].data.uv.scale = DVec2::new(0.5, 2.0);
            b.faces[0].data.uv.rotation = 30.0;
            b.faces[1].data.material = "wood".into();
            b.faces[0].data.uv.clone()
        };
        let n = replace_material(&mut m, "brick", "stone", MaterialScope::Map);
        assert_eq!(n, Changed { faces: 5, ..Changed::default() });
        let b = m.brush(id).unwrap();
        assert_eq!(b.faces[0].data.uv, uv);
        assert_eq!(b.faces[0].data.material, "stone");
        assert_eq!(b.faces[1].data.material, "wood");
        assert_eq!(count_material(&m, "brick", MaterialScope::Map).total(), 0);
    }

    #[test]
    fn a_selection_limits_the_replacement_to_its_faces_or_nodes() {
        let mut m = Map::new();
        let (a, b) = (brush(&mut m, "brick"), brush(&mut m, "brick"));
        let mut faces = Selection::default();
        faces.select_face(a, 0);
        faces.select_face(a, 2);
        assert_eq!(count_material(&m, "brick", MaterialScope::Selection(&faces)).faces, 2);
        replace_material(&mut m, "brick", "stone", MaterialScope::Selection(&faces));
        let stone = |id| m.brush(id).unwrap().faces.iter().filter(|f| f.data.material == "stone").count();
        assert_eq!((stone(a), stone(b)), (2, 0));

        let mut nodes = Selection::default();
        nodes.select_node(b);
        assert_eq!(replace_material(&mut m, "brick", "stone", MaterialScope::Selection(&nodes)).faces, 6);
        assert_eq!(count_material(&m, "brick", MaterialScope::Map).faces, 4);
        assert_eq!(count_material(&m, "brick", MaterialScope::Selection(&Selection::default())).total(), 0);
    }

    #[test]
    fn terrain_layers_and_blend_materials_follow() {
        let mut m = Map::new();
        let layer = m.default_layer();
        let t = m.insert(layer, NodeKind::Terrain(Terrain::new(DVec3::ZERO, [3, 3], 16.0, "grass")));
        let id = brush(&mut m, "dirt");
        m.brush_mut(id).unwrap().faces[0].data.props.insert(BLEND_MATERIAL.into(), "grass".into());
        let n = replace_material(&mut m, "grass", "moss", MaterialScope::Map);
        assert_eq!(n, Changed { faces: 1, terrain_layers: 1, scatter_materials: 0 });
        assert_eq!(m.terrain(t).unwrap().layers[0].material, "moss");
        assert_eq!(m.brush(id).unwrap().faces[0].data.props[BLEND_MATERIAL], "moss");
        assert_eq!(m.brush(id).unwrap().faces[0].data.material, "dirt");
        let mut sel = Selection::default();
        sel.select_face(id, 1);
        assert_eq!(count_material(&m, "moss", MaterialScope::Selection(&sel)).total(), 0, "a face selection leaves terrain alone");
    }

    #[test]
    fn scatter_material_overrides_follow() {
        let mut m = Map::new();
        let layer = m.default_layer();
        let mut set = Scatter::new("rocks", ScatterKind::Props, vec![ScatterItem::new("res://a.glb"), ScatterItem::new("res://b.glb")]);
        set.material = Some("dev/orange".into());
        set.items[1].material = Some("dev/orange".into());
        let s = m.insert(layer, NodeKind::Scatter(set));
        let plain = m.insert(layer, NodeKind::Scatter(Scatter::new("grass", ScatterKind::Foliage, vec![ScatterItem::new("res://c.glb")])));
        let id = brush(&mut m, "dev/orange");
        let mut faces = Selection::default();
        faces.select_face(id, 0);
        assert_eq!(count_material(&m, "dev/orange", MaterialScope::Selection(&faces)).scatter_materials, 0);
        let mut other = Selection::default();
        other.select_node(plain);
        assert_eq!(count_material(&m, "dev/orange", MaterialScope::Selection(&other)).total(), 0);
        assert_eq!(materials_in_use(&m).get("dev/orange"), Some(&8));

        let mut nodes = Selection::default();
        nodes.select_node(s);
        assert_eq!(count_material(&m, "dev/orange", MaterialScope::Selection(&nodes)), Changed { scatter_materials: 2, ..Changed::default() });
        assert_eq!(replace_material(&mut m, "dev/orange", "dev/dark", MaterialScope::Map), Changed { faces: 6, terrain_layers: 0, scatter_materials: 2 });
        let set = m.scatter(s).unwrap();
        assert_eq!((set.material.as_deref(), set.items[0].material.as_deref(), set.items[1].material.as_deref()), (Some("dev/dark"), None, Some("dev/dark")));
        assert_eq!(m.scatter(plain).unwrap().material, None);
    }

    #[test]
    fn materials_in_use_counts_faces_and_layers() {
        let mut m = Map::new();
        brush(&mut m, "brick");
        let counts = materials_in_use(&m);
        assert_eq!(counts.get("brick"), Some(&6));
        assert_eq!(counts.len(), 1);
    }
}
