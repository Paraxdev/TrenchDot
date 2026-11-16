use std::collections::BTreeSet;

use td_core::NodeId;

use crate::map::Map;

/// Either objects or faces are selected, never both (TrenchBroom semantics).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub nodes: BTreeSet<NodeId>,
    pub faces: BTreeSet<(NodeId, usize)>,
}

fn face_count(map: &Map, id: NodeId) -> Option<usize> {
    map.brush(id).map(|b| b.faces.len()).or_else(|| map.mesh(id).map(|m| m.faces.len()))
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.faces.is_empty()
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.faces.clear();
    }

    pub fn select_node(&mut self, id: NodeId) {
        self.faces.clear();
        self.nodes.insert(id);
    }

    pub fn toggle_node(&mut self, id: NodeId) {
        self.faces.clear();
        if !self.nodes.remove(&id) {
            self.nodes.insert(id);
        }
    }

    pub fn select_face(&mut self, brush: NodeId, face: usize) {
        self.nodes.clear();
        self.faces.insert((brush, face));
    }

    pub fn toggle_face(&mut self, brush: NodeId, face: usize) {
        self.nodes.clear();
        if !self.faces.remove(&(brush, face)) {
            self.faces.insert((brush, face));
        }
    }

    pub fn has_faces(&self) -> bool {
        !self.faces.is_empty()
    }

    /// Drops ids that no longer exist or no longer point at valid faces.
    pub fn prune(&mut self, map: &Map) {
        self.nodes.retain(|id| map.contains(*id));
        self.faces.retain(|(id, f)| face_count(map, *id).is_some_and(|n| *f < n));
    }

    /// Swaps the selected objects for every face of `objects`, keeping faces that are already selected. Ids that
    /// are not a brush or a mesh add no faces.
    pub fn select_faces_of(&mut self, map: &Map, objects: impl IntoIterator<Item = NodeId>) {
        let faces: Vec<(NodeId, usize)> = objects.into_iter().flat_map(|id| (0..face_count(map, id).unwrap_or(0)).map(move |f| (id, f))).collect();
        self.nodes.clear();
        self.faces.extend(faces);
    }

    /// Selected nodes expanded to all brushes they contain (groups and brush entities recurse).
    pub fn brushes(&self, map: &Map) -> Vec<NodeId> {
        let mut out = BTreeSet::new();
        for id in &self.nodes {
            if map.brush(*id).is_some() {
                out.insert(*id);
            }

            for d in map.descendants(*id) {
                if map.brush(d).is_some() {
                    out.insert(d);
                }
            }
        }

        out.into_iter().collect()
    }

    /// Selected nodes expanded to every leaf that can be transformed: brushes, meshes, terrains, point entities and instances.
    pub fn transformables(&self, map: &Map) -> Vec<NodeId> {
        let mut out = BTreeSet::new();
        for id in &self.nodes {
            let mut consider = vec![*id];
            consider.extend(map.descendants(*id));
            for c in consider {
                let Some(n) = map.get(c) else { continue };
                match &n.kind {
                    crate::NodeKind::Layer(_) | crate::NodeKind::Group(_) => {}
                    _ => {
                        out.insert(c);
                    }
                }
            }
        }

        out.into_iter().collect()
    }

    /// Selected nodes expanded to the meshes they contain.
    pub fn meshes(&self, map: &Map) -> Vec<NodeId> {
        self.expand(map, |n| matches!(n.kind, crate::NodeKind::Mesh(_)))
    }

    pub fn terrains(&self, map: &Map) -> Vec<NodeId> {
        self.expand(map, |n| matches!(n.kind, crate::NodeKind::Terrain(_)))
    }

    /// Brushes, meshes and terrains.
    pub fn geometry(&self, map: &Map) -> Vec<NodeId> {
        self.expand(map, |n| n.kind.is_geometry())
    }

    fn expand(&self, map: &Map, keep: impl Fn(&crate::map::Node) -> bool) -> Vec<NodeId> {
        let mut out = BTreeSet::new();
        for id in &self.nodes {
            for c in std::iter::once(*id).chain(map.descendants(*id)) {
                if map.get(c).is_some_and(&keep) {
                    out.insert(c);
                }
            }
        }

        out.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeKind;
    use td_core::{Aabb, DVec3};
    use td_geom::{Brush, Mesh};

    fn brush(m: &mut Map, min: f64, size: f64) -> NodeId {
        let layer = m.default_layer();
        let min = DVec3::new(min, 7.0, -3.0);
        m.insert(layer, NodeKind::Brush(Brush::from_aabb(&Aabb::new(min, min + DVec3::new(size, size * 2.0, size * 0.5)), "dev/grey").unwrap()))
    }

    fn mesh(m: &mut Map) -> (NodeId, usize) {
        let layer = m.default_layer();
        let b = Brush::from_aabb(&Aabb::new(DVec3::new(-64.0, 5.0, 9.0), DVec3::new(-20.0, 40.0, 21.0)), "dev/grey").unwrap();
        let mesh = Mesh::from_brush(&b);
        let n = mesh.faces.len();
        (m.insert(layer, NodeKind::Mesh(mesh)), n)
    }

    #[test]
    fn selecting_faces_of_objects_swaps_them_for_all_their_faces() {
        let mut m = Map::new();
        let (a, b) = (brush(&mut m, 96.0, 32.0), brush(&mut m, -128.0, 48.0));
        let (mid, mesh_faces) = mesh(&mut m);
        let mut s = Selection::default();
        s.select_node(a);
        s.select_node(b);
        s.select_node(mid);
        s.select_faces_of(&m, s.nodes.clone());
        let mut want: BTreeSet<(NodeId, usize)> = (0..6).map(|f| (a, f)).chain((0..6).map(|f| (b, f))).collect();
        want.extend((0..mesh_faces).map(|f| (mid, f)));
        assert!(s.nodes.is_empty());
        assert_eq!(s.faces, want);
    }

    #[test]
    fn faces_already_selected_on_another_brush_are_kept() {
        let mut m = Map::new();
        let (a, other) = (brush(&mut m, 0.0, 32.0), brush(&mut m, 200.0, 16.0));
        let mut s = Selection::default();
        s.select_face(other, 4);
        s.select_faces_of(&m, [a]);
        let mut want: BTreeSet<(NodeId, usize)> = (0..6).map(|f| (a, f)).collect();
        want.insert((other, 4));
        assert_eq!(s.faces, want);
        assert!(s.nodes.is_empty());
    }

    #[test]
    fn nothing_to_convert_adds_no_faces() {
        let mut m = Map::new();
        let a = brush(&mut m, 0.0, 32.0);
        let mut s = Selection::default();
        s.select_faces_of(&m, []);
        assert!(s.is_empty());
        s.select_face(a, 1);
        s.select_faces_of(&m, [m.default_layer(), NodeId(u64::MAX)]);
        assert_eq!(s.faces, BTreeSet::from([(a, 1)]));
    }
}
