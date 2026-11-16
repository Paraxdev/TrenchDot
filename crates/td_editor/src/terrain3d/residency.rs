//! Which Terrain3D regions the editor holds and how: whole near a camera, where the clipmap draws them from fine data
//! and picking is exact, and as a pyramid everywhere else.

use std::collections::{BTreeMap, BTreeSet};

use td_core::{DVec2, DVec3};
use td_geom::terrain3d::{Grid, Loc};
use td_geom::terrain3d_tile;
use td_render::clipmap::{coarse_stride, layout::MESH_SIZE};

/// A 2D view loads a region whole while a vertex covers more than this many pixels.
pub const FULL_VERTEX_PX: f64 = 1.0;

/// What a view looks at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum View {
    /// The 3D camera.
    Eye { position: DVec3 },
    /// A 2D view. `min` and `max` are the visible x and z, infinite along the axis the view looks past.
    Flat { min: DVec2, max: DVec2, zoom: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    Full,
    Lod,
}

/// Cells from an eye within which a region is held whole: as far as the clipmap levels reach whose vertices lie closer
/// together than the coarse stride, the ones that need fine data.
pub fn fine_reach(region_size: u32) -> i64 {
    (2 * MESH_SIZE + 8) * (coarse_stride(region_size) as i64 / 2).max(1)
}

/// Whether a region's pyramid holds the clipmap's coarse stride. A region whose pyramid does not is always held whole.
pub fn pyramid_has_coarse(region_size: u32) -> bool {
    terrain3d_tile::lod_strides(region_size).contains(&coarse_stride(region_size))
}

/// Where on the ground a view looks.
pub fn focus(view: &View) -> DVec2 {
    match *view {
        View::Eye { position } => DVec2::new(position.x, position.z),
        View::Flat { min, max, .. } => {
            let mid = (min + max) * 0.5;
            DVec2::new(if mid.x.is_finite() { mid.x } else { 0.0 }, if mid.y.is_finite() { mid.y } else { 0.0 })
        }
    }
}

/// The map unit rectangle (x, z) a region covers.
pub fn region_rect(grid: &Grid, loc: Loc) -> (DVec2, DVec2) {
    let min = grid.position(grid.vgrid_of(loc, [0, 0]));
    (min, min + DVec2::splat(grid.region_size as f64 * grid.cell()))
}

/// Map units from `p` to the nearest point of a region along the farther of x and z, 0 inside it.
fn gap(grid: &Grid, loc: Loc, p: DVec2) -> f64 {
    let (min, max) = region_rect(grid, loc);
    (min - p).max(p - max).max(DVec2::ZERO).max_element()
}

fn whole_for(grid: &Grid, loc: Loc, view: &View) -> bool {
    let cell = grid.cell();
    match *view {
        View::Eye { position } => gap(grid, loc, DVec2::new(position.x, position.z)) <= fine_reach(grid.region_size) as f64 * cell,
        View::Flat { min: lo, max: hi, zoom } => {
            let (min, max) = region_rect(grid, loc);
            let inside = max.x >= lo.x && min.x <= hi.x && max.y >= lo.y && min.y <= hi.y;
            inside && zoom * cell > FULL_VERTEX_PX
        }
    }
}

/// How each region is held: whole when a view needs it whole, as a pyramid otherwise.
pub fn plan(grid: &Grid, locs: &[Loc], views: &[View]) -> BTreeMap<Loc, Detail> {
    let pyramid = pyramid_has_coarse(grid.region_size);
    locs.iter().map(|&loc| (loc, if !pyramid || views.iter().any(|v| whole_for(grid, loc, v)) { Detail::Full } else { Detail::Lod })).collect()
}

/// What to ask the store for: the regions to hold whole, which include the east, south and south east neighbors whose
/// seam a whole region shares, and the regions a pyramid is enough for.
pub fn wants(plan: &BTreeMap<Loc, Detail>, exists: &dyn Fn(Loc) -> bool) -> (Vec<Loc>, Vec<Loc>) {
    let mut full = BTreeSet::new();
    for (loc, detail) in plan {
        if *detail == Detail::Full {
            full.insert(*loc);
            for n in [[1, 0], [0, 1], [1, 1]] {
                let n = [loc[0].saturating_add(n[0]), loc[1].saturating_add(n[1])];
                if exists(n) {
                    full.insert(n);
                }
            }
        }
    }

    let lod = plan.keys().filter(|l| !full.contains(*l)).copied().collect();
    (full.into_iter().collect(), lod)
}

/// The regions held whole, nearest to an eye first (else to the first view), the order the clipmap gives out fine
/// layers in.
pub fn by_distance(grid: &Grid, full: &[Loc], views: &[View]) -> Vec<Loc> {
    let eyes: Vec<DVec2> = views.iter().filter(|v| matches!(v, View::Eye { .. })).map(focus).collect();
    let points = if eyes.is_empty() { views.first().map(focus).into_iter().collect() } else { eyes };
    let distance = |l: &Loc| points.iter().map(|p| gap(grid, *l, *p)).fold(f64::INFINITY, f64::min);
    let mut out = full.to_vec();
    out.sort_by(|a, b| distance(a).total_cmp(&distance(b)).then(a.cmp(b)));
    out
}

/// Where a region's data came from, so an unchanged source is not uploaded again. Edits are told apart by identity,
/// cached regions by the hash Godot gave them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Absent,
    Cached([u8; 32]),
    Edited(usize),
}

/// Where the regions of a linked Terrain3D node are, for the 2D views to mark.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub grid: Grid,
    pub locs: Vec<Loc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(size: u32) -> Grid {
        Grid { region_size: size, spacing: 0.5, upm: 32.0 }
    }

    fn eye(x: f64, z: f64) -> View {
        View::Eye { position: DVec3::new(x, 900.0, z) }
    }

    #[test]
    fn a_region_is_whole_up_to_the_fine_reach_of_an_eye_and_a_pyramid_past_it() {
        let g = grid(256);
        assert_eq!((fine_reach(256), fine_reach(512), fine_reach(1024), fine_reach(2048)), (208, 208, 832, 832));
        let reach = 208.0 * g.cell();
        let (min, max) = region_rect(&g, [-2, 1]);
        let mid_z = min.y * 0.3 + max.y * 0.7;
        let at = |x: f64, z: f64| plan(&g, &[[-2, 1]], &[eye(x, z)])[&[-2, 1]];
        assert_eq!(at(min.x - reach, mid_z), Detail::Full, "exactly at the reach to the west");
        assert_eq!(at(min.x - reach - 0.01, mid_z), Detail::Lod);
        assert_eq!(at(max.x + reach, max.y + reach), Detail::Full, "the reach is along the farther axis, a corner counts");
        assert_eq!(at(max.x + reach, max.y + reach + 0.01), Detail::Lod);
        assert_eq!(at(min.x + 3.0, mid_z), Detail::Full, "the eye inside the region");
    }

    #[test]
    fn every_region_is_held_and_only_the_near_ones_whole() {
        let g = grid(1024);
        let locs = [[-1, 0], [0, 0], [3, -2]];
        let p = plan(&g, &locs, &[eye(-200.0, 4000.5)]);
        assert_eq!(p.values().copied().collect::<Vec<_>>(), vec![Detail::Full, Detail::Full, Detail::Lod]);
        assert_eq!(plan(&g, &locs, &[]).values().filter(|d| **d == Detail::Lod).count(), 3, "no view still holds every pyramid");
        assert!(plan(&g, &[], &[eye(0.0, 0.0)]).is_empty());
    }

    #[test]
    fn a_region_whose_pyramid_lacks_the_coarse_stride_is_always_whole() {
        assert!(!pyramid_has_coarse(32) && pyramid_has_coarse(64) && pyramid_has_coarse(2048));
        let far = plan(&grid(32), &[[7, 7]], &[eye(-90_000.0, 0.0)]);
        assert_eq!(far[&[7, 7]], Detail::Full);
    }

    #[test]
    fn a_2d_view_holds_whole_only_past_one_pixel_per_vertex_and_inside_its_box() {
        let g = grid(256);
        let (min, max) = region_rect(&g, [1, -1]);
        let flat = |zoom: f64, lo: DVec2| View::Flat { min: lo, max: lo + DVec2::splat(500.0), zoom };
        let at = |v: View| plan(&g, &[[1, -1]], &[v])[&[1, -1]];
        let px = 1.0 / g.cell();
        assert_eq!(at(flat(px * 1.001, min)), Detail::Full);
        assert_eq!(at(flat(px, min)), Detail::Lod, "exactly one pixel per vertex is not enough");
        assert_eq!(at(flat(4.0, max + DVec2::splat(0.01))), Detail::Lod, "outside the box");
        assert_eq!(at(flat(4.0, max)), Detail::Full, "touching the box");
    }

    #[test]
    fn whole_regions_pull_in_their_seam_neighbors_and_the_nearest_comes_first() {
        let g = grid(256);
        let plan: BTreeMap<Loc, Detail> = [([0, 0], Detail::Full), ([1, 0], Detail::Lod), ([1, 1], Detail::Lod), ([-4, 3], Detail::Lod)].into();
        let exists = |l: Loc| plan.contains_key(&l);
        let (full, lod) = wants(&plan, &exists);
        assert_eq!((full.clone(), lod), (vec![[0, 0], [1, 0], [1, 1]], vec![[-4, 3]]));

        let (_, max) = region_rect(&g, [1, 1]);
        let views = [View::Flat { min: DVec2::ZERO, max: DVec2::ONE, zoom: 1.0 }, eye(max.x + 10.0, max.y + 10.0)];
        assert_eq!(by_distance(&g, &full, &views), vec![[1, 1], [0, 0], [1, 0]], "the eye wins over the 2D view, ties by location");
        assert_eq!(by_distance(&g, &full, &views[..1]), vec![[0, 0], [1, 0], [1, 1]]);
        assert!(by_distance(&g, &[], &views).is_empty());
    }
}
