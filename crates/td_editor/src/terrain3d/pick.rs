//! The region data clicks, the Sculpt brush and scatter cast onto for a linked Terrain3D node.

use std::collections::BTreeMap;
use std::sync::Arc;

use td_core::{Aabb, DVec3, NodeId, Ray};
use td_doc::terrain3d::RegionSource;
use td_formats::terrain3d_cache::Lod;
use td_geom::terrain3d::{EdgeBlend, Grid, Loc, Region3D};
use td_geom::terrain3d_pick::{self, Heights, Hit};

use super::store::finest;

use crate::state::EditorState;

enum Held {
    Whole(Arc<Region3D>),
    Lod(Arc<Lod>),
}

impl Held {
    fn heights(&self) -> Option<Heights<'_>> {
        match self {
            Self::Whole(r) => Some(Heights::Whole(r)),
            Self::Lod(lod) => finest(lod).map(|l| Heights::Strided { stride: l.stride, side: l.side, heights: &l.heights, holes: &l.holes }),
        }
    }
}

/// A node's regions as loaded: TrenchDot's edits and the regions loaded whole, else the finest LOD level of the rest.
pub(crate) struct PickRegions {
    grid: Grid,
    edge: Option<EdgeBlend>,
    locs: Vec<Loc>,
    /// The data of a region and its lowest and highest height in meters.
    held: BTreeMap<Loc, (Held, [f32; 2])>,
}

impl PickRegions {
    pub(crate) fn ray_cast(&self, ray: &Ray) -> Option<Hit> {
        let held = |l: Loc| self.held.get(&l);
        terrain3d_pick::ray_cast(&self.grid, self.edge, |l| held(l)?.0.heights(), |l| Some(held(l)?.1), &self.locs, ray)
    }

    /// The loaded regions in map units, empty while none is.
    pub(crate) fn bounds(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        let span = self.grid.region_size as f64 * self.grid.cell();
        for (loc, (_, range)) in &self.held {
            let [mut lo, mut hi] = *range;
            if let Some(edge) = self.edge {
                (lo, hi) = (lo.min(edge.ground_level), hi.max(edge.ground_level));
            }

            if lo > hi {
                continue;
            }

            let corner = self.grid.position(self.grid.vgrid_of(*loc, [0, 0]));
            b.include_point(DVec3::new(corner.x, lo as f64 * self.grid.upm, corner.y));
            b.include_point(DVec3::new(corner.x + span, hi as f64 * self.grid.upm, corner.y + span));
        }

        b
    }
}

impl EditorState {
    /// What a linked Terrain3D node is picked against. None for a node that is hidden, outside the cordon, or whose
    /// folder has not loaded.
    pub(crate) fn terrain3d_regions(&self, id: NodeId) -> Option<PickRegions> {
        let map = &self.doc.map;
        if map.is_hidden(id) || !map.in_cordon(id) {
            return None;
        }

        let (_, cache) = self.terrain3d_links().into_iter().find(|(l, _)| *l == id)?;
        let (grid, link, source) = (self.terrain3d.grid(&cache)?, map.terrain3d(id)?, self.terrain3d.source(&cache)?);
        let locs = link.locs(&source);
        let held = locs
            .iter()
            .filter_map(|loc| {
                let held = match link.region(&source, *loc) {
                    Some(region) => {
                        let range = self.terrain3d.region_range(&region);
                        (Held::Whole(region), range)
                    }
                    None => {
                        let lod = self.terrain3d.lod(&cache, *loc)?;
                        let range = self.terrain3d.lod_range(&lod);
                        (Held::Lod(lod), range)
                    }
                };
                Some((*loc, held))
            })
            .collect();
        Some(PickRegions { grid, edge: source.edge_blend(), locs, held })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use td_core::{DVec3, Ray};
    use td_doc::terrain3d::RegionSource;
    use td_formats::terrain3d_cache::{LodLevel, RegionHash};

    use super::{Held, PickRegions};
    use crate::picking::{SurfaceCaster, pick_all};
    use crate::terrain3d::brush::tests::state_over;
    use crate::terrain3d::store::build_lod;

    // The fixture's heights rise 0.01 m per vertex eastward inside a region, 80 units apart at 32 units per meter.
    fn on_the_slope(p: DVec3, normal: DVec3) {
        assert!((p.y - (32.0 + 0.004 * p.x)).abs() < 1e-3, "{p} lies on the region's slope");
        assert!(normal.distance(DVec3::new(-0.004, 1.0, 0.0).normalize()) < 1e-5, "{normal} is the slope's normal");
    }

    #[test]
    fn clicks_brushes_and_scatter_hit_the_region_data_without_a_tile() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, _) = state_over(dir.path(), &[[-1, 0], [0, 0]]);
        assert_eq!(state.terrain3d_tiles.node(id).count(), 0, "nothing built a tile");
        let ray = Ray::new(DVec3::new(2000.5, 3000.0, 1500.3), DVec3::new(0.2, -1.0, 0.1));

        let hits = pick_all(&state, &ray);
        let [hit] = hits.as_slice() else { panic!("one hit, got {hits:?}") };
        assert_eq!((hit.node, hit.face), (id, None));
        assert!((hit.point - ray.at(hit.distance)).length() < 1e-9);
        on_the_slope(hit.point, hit.normal);

        let (d, p, node) = state.terrain3d_ray(&[id], &ray).expect("the Sculpt brush finds the ground");
        assert_eq!((d, p, node), (hit.distance, hit.point, id));
        let caster = SurfaceCaster::new(&state, &td_core::Aabb::from_center_size(ray.origin, DVec3::splat(1.0e6)));
        let ground = caster.cast(ray.origin, ray.dir).expect("scatter finds the ground");
        assert_eq!((ground.node, ground.normal), (id, hit.normal));
        assert!(ground.point.distance(hit.point) < 1e-9, "{} against {}", ground.point, hit.point);
        let b = caster.bounds_of(&[id]);
        assert!(b.min.x <= -5120.0 && b.max.x >= 5120.0 && b.min.y <= 32.0 && b.max.y >= 52.0, "the bounds hold both regions: {b:?}");

        state.doc.map.get_mut(id).unwrap().hidden = true;
        assert!(pick_all(&state, &ray).is_empty() && state.terrain3d_ray(&[id], &ray).is_none(), "a hidden node is not hit");
        assert!(SurfaceCaster::new(&state, &td_core::Aabb::from_center_size(ray.origin, DVec3::splat(1.0e6))).is_empty());
    }

    #[test]
    fn a_region_without_its_whole_data_is_hit_on_its_finest_lod_level() {
        let dir = tempfile::tempdir().unwrap();
        let (state, id, cache) = state_over(dir.path(), &[[0, 0]]);
        let source = state.terrain3d.source(&cache).unwrap();
        let grid = source.grid();
        let region = source.region([0, 0]).unwrap();
        let mut lod = build_lod(&grid, &region, RegionHash([9; 32])).expect("a pyramid");
        assert_eq!(lod.levels.iter().map(|l| l.stride).collect::<Vec<_>>(), [4]);
        lod.levels.insert(0, LodLevel { stride: 16, side: 4, ids: vec![3], heights: vec![-50.0; 16], splat: Vec::new(), holes: Vec::new() });
        let lod = Arc::new(lod);
        let range = state.terrain3d.lod_range(&lod);
        let regions = PickRegions { grid, edge: None, locs: vec![[0, 0]], held: BTreeMap::from([([0, 0], (Held::Lod(lod), range))]) };

        let ray = Ray::new(DVec3::new(2000.5, 3000.0, 1500.3), DVec3::new(0.2, -1.0, 0.1));
        let hit = regions.ray_cast(&ray).expect("the level is hit");
        on_the_slope(hit.point, hit.normal);
        let whole = state.terrain3d_regions(id).unwrap().ray_cast(&ray).unwrap();
        assert!((hit.distance - whole.distance).abs() < 1e-3, "the stride 4 level of a plane is the plane");
        assert!(regions.bounds().min.y > 0.0, "the coarser level does not count: {:?}", regions.bounds());
    }
}
