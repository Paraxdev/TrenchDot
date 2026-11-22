//! Cutter brushes: the brushes of a `func_cutter` entity carve every brush they overlap when the map is built,
//! while the map keeps the target brushes whole so the cut follows the cutter.

use std::collections::BTreeMap;

use td_core::NodeId;
use td_geom::Brush;
use td_geom::csg::{self, CarveMaterial};

use crate::map::{Map, NodeKind};

pub const CUTTER_CLASS: &str = "func_cutter";

pub fn is_cutter_entity(map: &Map, id: NodeId) -> bool {
    map.entity(id).is_some_and(|e| e.classname == CUTTER_CLASS)
}

/// Whether `id` is a brush of a cutter entity.
pub fn is_cutter_brush(map: &Map, id: NodeId) -> bool {
    map.owning_entity(id).is_some_and(|e| is_cutter_entity(map, e))
}

fn exported(map: &Map, id: NodeId) -> bool {
    !matches!(map.get(map.layer_of(id)).map(|l| &l.kind), Some(NodeKind::Layer(l)) if l.omit_from_export)
}

/// The cutter brushes that get built, with their bounds.
pub fn cutters(map: &Map) -> Vec<(NodeId, &Brush)> {
    map.brushes().filter(|(id, _)| is_cutter_brush(map, *id) && exported(map, *id)).collect()
}

/// Each brush a cutter overlaps, mapped to the convex pieces left after every overlapping cutter carved it.
/// A brush cut away entirely maps to no pieces.
pub fn cut_pieces(map: &Map) -> BTreeMap<NodeId, Vec<Brush>> {
    let cutters: Vec<(&Brush, td_core::Aabb)> = cutters(map).into_iter().map(|(_, b)| (b, b.bounds())).collect();
    let mut out = BTreeMap::new();
    if cutters.is_empty() {
        return out;
    }

    for (id, brush) in map.brushes() {
        if is_cutter_brush(map, id) {
            continue;
        }

        let bounds = brush.bounds();
        let hits: Vec<&Brush> = cutters.iter().filter(|(c, b)| b.intersects(&bounds) && brush.intersects(c)).map(|(c, _)| *c).collect();
        if hits.is_empty() {
            continue;
        }

        let mut pieces = vec![brush.clone()];
        for cutter in hits {
            pieces = pieces.iter().flat_map(|p| csg::subtract_with(p, cutter, CarveMaterial::Target)).collect();
        }

        out.insert(id, pieces);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Entity;
    use td_core::{Aabb, DVec3};

    fn cube(m: &mut Map, parent: NodeId, min: f64, max: f64) -> NodeId {
        m.insert(parent, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::splat(min), DVec3::splat(max)), "wall").unwrap()))
    }

    fn cutter(m: &mut Map, min: DVec3, max: DVec3) -> NodeId {
        let layer = m.default_layer();
        let e = m.insert(layer, NodeKind::Entity(Entity::new(CUTTER_CLASS)));
        m.insert(e, NodeKind::Brush(Brush::from_aabb(&Aabb::new(min, max), "tools/cutter").unwrap()));
        e
    }

    #[test]
    fn a_cutter_carves_the_brushes_it_overlaps_and_leaves_the_rest() {
        let mut m = Map::new();
        let layer = m.default_layer();
        let wall = cube(&mut m, layer, 0.0, 64.0);
        let far = cube(&mut m, layer, 200.0, 264.0);
        cutter(&mut m, DVec3::new(16.0, 16.0, -8.0), DVec3::new(48.0, 48.0, 72.0));

        let cut = cut_pieces(&m);
        assert_eq!(cut.keys().copied().collect::<Vec<_>>(), [wall], "only the overlapped brush, never the cutter or {far:?}");
        let volume: f64 = cut[&wall].iter().map(|p| p.volume()).sum();
        assert!((volume - (64.0f64.powi(3) - 32.0 * 32.0 * 64.0)).abs() < 1e-6, "{volume}");
        assert!(cut[&wall].iter().all(|p| p.faces.iter().all(|f| f.data.material == "wall")), "the hole takes the wall's material");
        assert_eq!(m.brush(wall).unwrap().volume(), 64.0f64.powi(3), "the map keeps the brush whole");
    }

    #[test]
    fn a_save_carries_the_pieces_and_loading_drops_them() {
        let mut m = Map::new();
        let layer = m.default_layer();
        let wall = cube(&mut m, layer, 0.0, 64.0);
        cutter(&mut m, DVec3::new(16.0, 16.0, -8.0), DVec3::new(48.0, 48.0, 72.0));

        let bytes = crate::format::to_bytes(&m).unwrap();
        let (value, _) = crate::format::file_json_value(&bytes).unwrap();
        let node = value["layers"][0]["children"].as_array().unwrap().iter().find(|n| n["id"] == wall.0).unwrap();
        assert_eq!(node["cut"].as_array().unwrap().len(), cut_pieces(&m)[&wall].len());
        assert!(crate::format::to_value(&m)["layers"][0]["children"][0].get("cut").is_none(), "copies and diffs leave it out");

        let back = crate::format::from_bytes(&bytes).unwrap();
        assert!(back.problems.is_empty(), "{:?}", back.problems);
        assert_eq!(back.map.brush(wall).unwrap(), m.brush(wall).unwrap());
        assert_eq!(crate::format::to_bytes(&back.map).unwrap(), bytes);
    }

    #[test]
    fn a_brush_inside_a_cutter_has_no_pieces_and_omitted_layers_do_not_cut() {
        let mut m = Map::new();
        let layer = m.default_layer();
        let small = cube(&mut m, layer, 8.0, 16.0);
        let e = cutter(&mut m, DVec3::ZERO, DVec3::splat(32.0));
        assert_eq!(cut_pieces(&m)[&small], Vec::<Brush>::new());

        let off = m.add_layer("off");
        if let Some(NodeKind::Layer(l)) = m.get_mut(off).map(|n| &mut n.kind) {
            l.omit_from_export = true;
        }

        m.reparent(e, off);
        assert!(cut_pieces(&m).is_empty());
    }
}
