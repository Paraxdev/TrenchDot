//! The link to a Terrain3D data folder in the Godot project. Godot reads and writes the folder, the map only names it.

use serde::{Deserialize, Serialize};

use crate::map::{Map, NodeKind};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Terrain3DLink {
    /// A `res://` path to the folder Terrain3D saves its regions in.
    pub data_directory: String,
}

impl Terrain3DLink {
    pub fn new(data_directory: impl Into<String>) -> Self {
        Self { data_directory: data_directory.into() }
    }
}

impl Map {
    pub fn has_terrain3d(&self) -> bool {
        self.nodes.values().any(|n| matches!(n.kind, NodeKind::Terrain3D(_)))
    }
}

#[cfg(test)]
mod tests {
    use td_core::{Aabb, DVec3, NodeId};
    use td_geom::Brush;

    use super::*;
    use crate::format::{self, FormatError};
    use crate::issues;
    use crate::map::Group;
    use crate::ops::{self, DropPlan, EditOptions};
    use crate::{Document, Selection};

    const OPTS: EditOptions = EditOptions { uv_lock: true, grid: 0.0 };

    fn brush_map() -> (Map, NodeId) {
        let mut m = Map::new();
        m.properties.insert("classname".into(), "worldspawn".into());
        let layer = m.default_layer();
        let b = Brush::from_aabb(&Aabb::new(DVec3::new(-200.0, 8.0, 40.0), DVec3::new(-136.0, 72.0, 120.0)), "base/floor").unwrap();
        let brush = m.insert(layer, NodeKind::Brush(b));
        (m, brush)
    }

    /// A brush on the default layer and the link on a second layer, hidden, locked and labelled.
    fn linked_map() -> (Map, NodeId, NodeId) {
        let (mut m, brush) = brush_map();
        let layer = m.add_layer("Ground");
        let link = m.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://levels/terrain3d_data")));
        let node = m.get_mut(link).unwrap();
        node.hidden = true;
        node.locked = true;
        node.label = Some("Valley".into());
        (m, brush, link)
    }

    fn version(value: &serde_json::Value) -> u64 {
        value["version"].as_u64().unwrap()
    }

    fn select(ids: &[NodeId]) -> Selection {
        let mut sel = Selection::default();
        sel.nodes.extend(ids.iter().copied());
        sel
    }

    #[test]
    fn a_map_without_the_link_stays_version_1_in_both_layouts() {
        let (m, _) = brush_map();
        assert!(!m.has_terrain3d());
        assert_eq!(version(&format::to_value(&m)), 1);
        let bytes = format::to_bytes(&m).unwrap();
        assert_eq!(version(&format::file_json_value(&bytes).unwrap().0), 1);
        assert!(format::read_value_up_to(format::to_value(&m), 1).is_ok());
    }

    #[test]
    fn a_map_with_the_link_writes_version_2_and_reads_back() {
        let (m, _, link) = linked_map();
        assert!(m.has_terrain3d());
        let value = format::to_value(&m);
        assert_eq!(version(&value), 2);
        let node = &value["layers"][1]["children"][0];
        assert_eq!(node["type"], "terrain3d");
        assert_eq!(node["data_directory"], "res://levels/terrain3d_data");
        assert_eq!(node["id"], link.0);

        let bytes = format::to_bytes(&m).unwrap();
        assert_eq!(version(&format::file_json_value(&bytes).unwrap().0), 2);
        for loaded in [format::from_bytes(&bytes).unwrap().map, format::from_str(&format::to_string(&m)).unwrap()] {
            assert_eq!(format::to_value(&loaded), value);
            let n = loaded.get(link).unwrap();
            assert_eq!(n.kind, NodeKind::Terrain3D(Terrain3DLink::new("res://levels/terrain3d_data")));
            assert_eq!((n.hidden, n.locked, n.name()), (true, true, "Valley".to_string()));
            assert_eq!(n.parent, Some(loaded.layers[1]));
        }
    }

    #[test]
    fn the_last_release_refuses_a_linked_map_as_too_new() {
        let (m, _, _) = linked_map();
        assert!(matches!(format::read_value_up_to(format::to_value(&m), 1), Err(FormatError::TooNew(2))));
        let (value, _) = format::file_json_value(&format::to_bytes(&m).unwrap()).unwrap();
        assert!(matches!(format::read_value_up_to(value, 1), Err(FormatError::TooNew(2))));
    }

    #[test]
    fn the_clipboard_is_version_2_only_when_it_holds_the_link() {
        let (m, brush, link) = linked_map();
        let clip = |ids: &[NodeId]| version(&serde_json::from_str(&format::nodes_to_string(&m, ids)).unwrap());
        assert_eq!(clip(&[]), 1);
        assert_eq!(clip(&[brush]), 1);
        assert_eq!(clip(&[link]), 2);
        assert_eq!(clip(&[brush, link]), 2);
        assert_eq!(clip(&[m.layers[1]]), 2);

        let (mut other, _) = brush_map();
        let layer = other.default_layer();
        let pasted = format::paste_nodes(&mut other, layer, &format::nodes_to_string(&m, &[link])).unwrap();
        assert_eq!(pasted.len(), 1);
        assert_eq!(other.get(pasted[0]).unwrap().kind, m.get(link).unwrap().kind);
    }

    #[test]
    fn the_link_survives_undo_and_a_save_and_load() {
        let (m, _) = brush_map();
        let mut doc = Document::from_map(m, None);
        let layer = doc.map.default_layer();
        let link = doc.edit("Link Terrain3D", |m, _| m.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://t3d"))));
        let linked = format::to_value(&doc.map);
        doc.undo();
        assert!(!doc.map.has_terrain3d());
        assert_eq!(version(&format::to_value(&doc.map)), 1);
        doc.redo();
        assert_eq!(format::to_value(&doc.map), linked);

        let dir = std::env::temp_dir().join(format!("td_terrain3d_link_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("linked.tdm");
        format::save(&doc.map, &path).unwrap();
        let loaded = format::load(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(loaded.problems.is_empty());
        assert_eq!(format::to_value(&loaded.map), linked);
        assert_eq!(loaded.map.get(link).unwrap().kind, NodeKind::Terrain3D(Terrain3DLink::new("res://t3d")));
    }

    #[test]
    fn moving_and_rotating_leave_the_link_alone() {
        let (mut m, brush, link) = linked_map();
        let before_link = m.get(link).cloned();
        let before_brush = m.bounds(brush);
        let sel = select(&[brush, link]);
        assert_eq!(sel.transformables(&m), vec![brush]);
        assert!(select(&[m.layers[1]]).transformables(&m).is_empty());

        ops::translate_selection(&mut m, &sel, DVec3::new(32.0, -16.0, 8.0), OPTS);
        assert_eq!(m.bounds(brush).min, before_brush.min + DVec3::new(32.0, -16.0, 8.0));
        ops::transform_selection(&mut m, &sel, &ops::rotation_about(DVec3::new(10.0, 0.0, -30.0), DVec3::Y, 30.0), OPTS);
        assert_eq!(m.get(link).cloned(), before_link);
    }

    #[test]
    fn duplicating_skips_the_link() {
        let (mut m, brush, link) = linked_map();
        let count = m.nodes.len();
        let mut sel = select(&[link]);
        assert!(ops::duplicate_selection(&mut m, &mut sel, DVec3::ZERO, OPTS).is_empty());
        assert_eq!(m.nodes.len(), count);

        let mut sel = select(&[brush, link]);
        let copies = ops::duplicate_selection(&mut m, &mut sel, DVec3::new(64.0, 0.0, 0.0), OPTS);
        assert_eq!(copies.len(), 1);
        assert!(m.get(copies[0]).unwrap().brush().is_some());
        assert_eq!(m.nodes.len(), count + 1);
        assert_eq!(m.nodes.values().filter(|n| n.kind.is_pinned()).count(), 1);
        let layer = m.default_layer();
        assert_eq!(m.duplicate_subtree(link, layer), None);
    }

    #[test]
    fn grouping_leaves_the_link_in_its_layer() {
        let (mut m, brush, link) = linked_map();
        let layer = m.default_layer();
        assert_eq!(ops::group_selection(&mut m, &mut select(&[link]), "g", layer), None);
        assert_eq!(m.get(link).unwrap().parent, Some(m.layers[1]));

        let group = ops::group_selection(&mut m, &mut select(&[brush, link]), "g", layer).unwrap();
        assert_eq!(m.get(group).unwrap().children, vec![brush]);
        assert_eq!(m.get(link).unwrap().parent, Some(m.layers[1]));
    }

    #[test]
    fn dropping_moves_the_link_only_between_layers() {
        let (mut m, brush, link) = linked_map();
        m.get_mut(link).unwrap().locked = false;
        let layer = m.default_layer();
        let group = m.insert(layer, NodeKind::Group(Group::new("room")));
        let inner = m.insert(group, NodeKind::Brush(m.brush(brush).unwrap().clone()));
        let yes = |_| true;

        assert_eq!(ops::drop_plan(&m, &[link], layer, yes), Some(DropPlan::Into(layer)));
        assert_eq!(ops::drop_plan(&m, &[link], group, yes), None);
        assert_eq!(ops::drop_plan(&m, &[link], inner, yes), None);
        assert_eq!(ops::drop_plan(&m, &[link], brush, yes), None);
        assert_eq!(ops::drop_plan(&m, &[link, brush], m.layers[1], yes), Some(DropPlan::Into(m.layers[1])));
        assert_eq!(ops::drop_plan(&m, &[brush], link, yes), None);
        assert_eq!(ops::drop_plan(&m, &[brush], inner, yes), Some(DropPlan::Into(group)));
    }

    #[test]
    fn holds_pinned_looks_inside_containers() {
        let (mut m, brush, link) = linked_map();
        assert!(!ops::holds_pinned(&m, &[]));
        assert!(!ops::holds_pinned(&m, &[brush]));
        assert!(ops::holds_pinned(&m, &[link]));
        assert!(ops::holds_pinned(&m, &[brush, link]));
        assert!(ops::holds_pinned(&m, &[m.layers[1]]));
        let layer = m.default_layer();
        let group = m.insert(layer, NodeKind::Group(Group::new("hand edited")));
        m.reparent(link, group);
        assert!(ops::holds_pinned(&m, &[group]));
    }

    type Found = (&'static str, Option<NodeId>, Vec<(&'static str, String)>);

    fn found(m: &Map) -> Vec<Found> {
        let mut out: Vec<Found> = issues::check(m).into_iter().filter(|i| i.code.starts_with("terrain3d")).map(|i| (i.code, i.node, i.args)).collect();
        out.sort();
        out
    }

    #[test]
    fn one_link_with_a_folder_in_a_layer_has_no_issues() {
        let (m, _, _) = linked_map();
        assert!(found(&m).is_empty());
    }

    #[test]
    fn an_empty_folder_is_an_error() {
        for folder in ["", "   "] {
            let (mut m, _, link) = linked_map();
            m.get_mut(link).unwrap().kind = NodeKind::Terrain3D(Terrain3DLink::new(folder));
            assert_eq!(found(&m), vec![("terrain3d_no_folder", Some(link), vec![])]);
        }
    }

    #[test]
    fn a_link_inside_a_group_or_entity_is_an_error() {
        let (mut m, _, link) = linked_map();
        let ground = m.layers[1];
        let group = m.insert(ground, NodeKind::Group(Group::new("hand edited")));
        m.reparent(link, group);
        assert_eq!(found(&m), vec![("terrain3d_nested", Some(link), vec![])]);

        let entity = m.insert(ground, NodeKind::Entity(crate::Entity::new("func_group")));
        m.reparent(link, entity);
        assert_eq!(found(&m), vec![("terrain3d_nested", Some(link), vec![])]);
    }

    #[test]
    fn every_link_after_the_first_is_an_error() {
        let (mut m, _, first) = linked_map();
        let layer = m.default_layer();
        let second = m.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://other")));
        let other = vec![("other", first.to_string())];
        assert_eq!(found(&m), vec![("terrain3d_second", Some(second), other.clone())]);

        let ground = m.layers[1];
        let third = m.insert(ground, NodeKind::Terrain3D(Terrain3DLink::new("res://levels/terrain3d_data")));
        assert_eq!(found(&m), vec![("terrain3d_second", Some(second), other.clone()), ("terrain3d_second", Some(third), other)]);
        let loaded = format::from_bytes(&format::to_bytes(&m).unwrap()).unwrap().map;
        assert_eq!(loaded.nodes.values().filter(|n| n.kind.is_pinned()).count(), 3);
    }
}
