//! Terrain3D regions as `Terrain` tiles, so the renderer, picking and scatter use them unchanged, and the brushes that
//! write TrenchDot strokes back into region data.

use crate::heightfield::{MAX_CELL_SIZE, MAX_LAYERS, MIN_CELL_SIZE, Terrain, TerrainLayer, falloff};
use crate::terrain3d::{Control, Grid, Loc, Region3D};
use td_core::{DVec2, DVec3};

/// Vertex strides of the LOD levels, coarsest last.
pub const LOD_STRIDES: [u32; 4] = [1, 4, 16, 64];
/// The coarsest level still has this many cells per side.
pub const LOD_MIN_CELLS: u32 = 16;
const TILE_LAYER_TILE: f64 = 256.0;

/// A region drawn as a terrain. `ids[k]` is the Terrain3D texture id shown by layer `k`.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub terrain: Terrain,
    pub ids: Vec<u8>,
}

/// The strides worth building for a region size.
pub fn lod_strides(region_size: u32) -> Vec<u32> {
    LOD_STRIDES.into_iter().filter(|s| region_size / s >= LOD_MIN_CELLS).collect()
}

/// Up to four texture ids by how much they show, most first, ties to the lower id. Base shows with `255 - blend`,
/// overlay with `blend`, an id that never shows is left out.
pub fn pick_layers(controls: impl IntoIterator<Item = Control>) -> Vec<u8> {
    let mut weight = [0u64; 32];
    for c in controls {
        weight[c.base() as usize] += 255 - c.blend() as u64;
        weight[c.overlay() as usize] += c.blend() as u64;
    }

    let mut ids: Vec<u8> = (0..32u8).filter(|&id| weight[id as usize] > 0).collect();
    ids.sort_by_key(|&id| (std::cmp::Reverse(weight[id as usize]), id));
    ids.truncate(MAX_LAYERS);
    ids
}

/// Splat bytes of one control word over the picked ids. Ids that were not picked drop out.
fn splat_of(c: Control, ids: &[u8]) -> [u8; 4] {
    let mut out = [0u8; 4];
    let blend = c.blend();
    for (id, w) in [(c.base(), 255 - blend), (c.overlay(), blend)] {
        if let Some(slot) = ids.iter().position(|i| *i == id) {
            out[slot] = out[slot].saturating_add(w);
        }
    }

    out
}

/// Cells touching a hole vertex, the triangles Terrain3D discards. Empty when there are none.
fn holes_from_vertices(res: [u32; 2], hole: &[bool]) -> Vec<u8> {
    if !hole.contains(&true) {
        return Vec::new();
    }

    let [w, d] = res.map(|v| v as usize);
    let mut cells = vec![0u8; (w - 1) * (d - 1)];
    for cj in 0..d - 1 {
        for ci in 0..w - 1 {
            let corners = [cj * w + ci, cj * w + ci + 1, (cj + 1) * w + ci, (cj + 1) * w + ci + 1];
            if corners.iter().any(|&k| hole[k]) {
                cells[cj * (w - 1) + ci] = 1;
            }
        }
    }

    cells
}

fn terrain_shell(grid: &Grid, origin_vgrid: [i64; 2], res: [u32; 2], stride: u32) -> Option<Terrain> {
    let cell = grid.cell() * stride as f64;
    if !(MIN_CELL_SIZE..=MAX_CELL_SIZE).contains(&cell) {
        return None;
    }

    let origin = grid.position(origin_vgrid);
    let mut t = Terrain::new(DVec3::new(origin.x, 0.0, origin.y), [2, 2], cell, "");
    t.resolution = res;
    t.layers.clear();
    Some(t)
}

fn to_map(grid: &Grid, meters: f32) -> f32 {
    (meters as f64 * grid.upm) as f32
}

/// `region` as a terrain of `size / stride + 1` vertices per side. The last row and column come from the east, south
/// and south east regions so neighboring tiles share their seam, or repeat the region's own edge where there is none.
/// None for a bad region, a stride that does not divide the size, or a cell size a terrain cannot hold.
pub fn tile<'a>(grid: &Grid, region: &Region3D, regions: impl Fn(Loc) -> Option<&'a Region3D>, stride: u32, material: impl Fn(u8) -> String) -> Option<Tile> {
    region.check().ok()?;
    if region.size != grid.region_size || stride == 0 || !region.size.is_multiple_of(stride) {
        return None;
    }

    let n = region.size / stride + 1;
    let origin = grid.vgrid_of(region.loc, [0, 0]);
    let mut terrain = terrain_shell(grid, origin, [n, n], stride)?;
    let ids = pick_layers(region.control.iter().map(|c| Control(*c)));
    let last = region.size as i64 - 1;
    let lookup = |v: [i64; 2]| {
        if grid.region_of(v) == region.loc { Some((region, region.index(grid.pixel_of(v)))) } else { grid.sample(&regions, v) }
    };
    let count = n as usize * n as usize;
    let mut controls = Vec::with_capacity(count);
    terrain.heights = Vec::with_capacity(count);
    for j in 0..n {
        for i in 0..n {
            let v = [origin[0] + (i * stride) as i64, origin[1] + (j * stride) as i64];
            // A missing south east region falls back to the east or south one, which is what that tile's own edge
            // repeats, so the shared corner vertex agrees.
            let (x, z) = (v[0].min(origin[0] + last), v[1].min(origin[1] + last));
            let (src, k) = [v, [v[0], z], [x, v[1]], [x, z]].into_iter().find_map(lookup).expect("own region");
            terrain.heights.push(to_map(grid, src.heights[k]));
            controls.push(Control(src.control[k]));
        }
    }

    let hole: Vec<bool> = controls.iter().map(|c| c.hole()).collect();
    terrain.holes = holes_from_vertices([n, n], &hole);
    terrain.layers = ids.iter().map(|id| TerrainLayer::new(material(*id), TILE_LAYER_TILE)).collect();
    if ids.len() > 1 {
        terrain.splat = controls.iter().flat_map(|c| splat_of(*c, &ids)).collect();
    }

    Some(Tile { terrain, ids })
}

/// One level of a region's LOD pyramid as the cache stores it: `side * side` vertices `stride` apart that the region
/// owns, heights in meters, `splat` (four bytes per vertex) over `ids`, and one hole byte per cell laid out `side` wide.
/// `splat` and `holes` may be empty.
#[derive(Clone, Copy, Debug)]
pub struct Level<'a> {
    pub stride: u32,
    pub side: u32,
    pub ids: &'a [u8],
    pub heights: &'a [f32],
    pub splat: &'a [u8],
    pub holes: &'a [u8],
}

impl Level<'_> {
    fn fits(&self, size: u32) -> bool {
        let n = self.side as usize * self.side as usize;
        self.stride > 0
            && self.side > 0
            && self.side * self.stride == size
            && self.heights.len() == n
            && (self.splat.is_empty() || self.splat.len() == n * 4)
            && (self.holes.is_empty() || self.holes.len() == n)
    }

    fn weights(&self, k: usize) -> impl Iterator<Item = (u8, u8)> + '_ {
        let single = (self.splat.is_empty()).then(|| self.ids.first().map(|id| (*id, 255u8))).flatten();
        let spread = self.splat.get(k * 4..k * 4 + 4).into_iter().flat_map(|w| self.ids.iter().copied().zip(w.iter().copied()));
        single.into_iter().chain(spread)
    }
}

/// A level of the region at `loc` as a terrain of `side + 1` vertices per side, the way [`tile`] draws a region. The seam
/// row and column come from the same level of the east, south and south east neighbors, or repeat the level's own edge
/// where a neighbor has none or another stride. None for a level that does not fit the grid.
pub fn level_tile<'a>(
    grid: &Grid,
    loc: Loc,
    own: Level<'a>,
    east: Option<Level<'a>>,
    south: Option<Level<'a>>,
    south_east: Option<Level<'a>>,
    material: impl Fn(u8) -> String,
) -> Option<Tile> {
    if !own.fits(grid.region_size) || own.ids.is_empty() {
        return None;
    }

    let same = |l: Option<Level<'a>>| l.filter(|l| l.stride == own.stride && l.side == own.side && l.fits(grid.region_size));
    let (east, south, south_east) = (same(east), same(south), same(south_east));
    let side = own.side as usize;
    let n = side + 1;
    let mut terrain = terrain_shell(grid, grid.vgrid_of(loc, [0, 0]), [n as u32, n as u32], own.stride)?;
    let multiple = own.ids.len() > 1;
    let mut splat = Vec::with_capacity(if multiple { n * n * 4 } else { 0 });
    terrain.heights = Vec::with_capacity(n * n);
    let last = side - 1;
    for j in 0..n {
        for i in 0..n {
            let (a, b) = (i == side, j == side);
            let candidates: [(Option<Level<'a>>, usize, usize); 4] = match (a, b) {
                (false, false) => [(Some(own), i, j); 4],
                (true, false) => [(east, 0, j), (Some(own), last, j), (Some(own), last, j), (Some(own), last, j)],
                (false, true) => [(south, i, 0), (Some(own), i, last), (Some(own), i, last), (Some(own), i, last)],
                (true, true) => [(south_east, 0, 0), (east, 0, last), (south, last, 0), (Some(own), last, last)],
            };
            let (level, x, z) = candidates.into_iter().find_map(|(l, x, z)| l.map(|l| (l, x, z)))?;
            let k = z * side + x;
            terrain.heights.push(to_map(grid, level.heights[k]));
            if multiple {
                let mut out = [0u8; 4];
                for (id, w) in level.weights(k) {
                    if let Some(slot) = own.ids.iter().position(|i| *i == id) {
                        out[slot] = out[slot].saturating_add(w);
                    }
                }

                splat.extend_from_slice(&out);
            }
        }
    }

    terrain.holes = if own.holes.iter().any(|h| *h != 0) { own.holes.to_vec() } else { Vec::new() };
    terrain.layers = own.ids.iter().map(|id| TerrainLayer::new(material(*id), TILE_LAYER_TILE)).collect();
    terrain.splat = splat;
    Some(Tile { terrain, ids: own.ids.to_vec() })
}

/// Every LOD level of a region that a terrain can hold, finest first.
pub fn pyramid<'a>(
    grid: &Grid,
    region: &Region3D,
    regions: impl Fn(Loc) -> Option<&'a Region3D> + Copy,
    material: impl Fn(u8) -> String + Copy,
) -> Vec<(u32, Tile)> {
    lod_strides(region.size).into_iter().filter_map(|s| Some((s, tile(grid, region, regions, s, material)?))).collect()
}

/// Heights from `min` to `max` (vertices, inclusive) across regions as one terrain for the sculpt brushes. Vertices
/// with no region read 0 and are marked absent, their cells are holes so rays pass through.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub terrain: Terrain,
    pub min: [i64; 2],
    pub present: Vec<bool>,
}

pub fn window<'a>(grid: &Grid, regions: impl Fn(Loc) -> Option<&'a Region3D>, min: [i64; 2], max: [i64; 2]) -> Option<Window> {
    let res = [(max[0] - min[0] + 1).max(0), (max[1] - min[1] + 1).max(0)];
    if res[0] < 2 || res[1] < 2 || res[0] > u32::MAX as i64 / res[1] {
        return None;
    }

    let res = res.map(|v| v as u32);
    let mut terrain = terrain_shell(grid, min, res, 1)?;
    terrain.layers.push(TerrainLayer::new("", TILE_LAYER_TILE));
    let count = res[0] as usize * res[1] as usize;
    terrain.heights = Vec::with_capacity(count);
    let mut present = Vec::with_capacity(count);
    let mut blocked = Vec::with_capacity(count);
    for z in min[1]..=max[1] {
        for x in min[0]..=max[0] {
            match grid.sample(&regions, [x, z]) {
                Some((r, k)) => {
                    terrain.heights.push(to_map(grid, r.heights[k]));
                    present.push(true);
                    blocked.push(Control(r.control[k]).hole());
                }
                None => {
                    terrain.heights.push(0.0);
                    present.push(false);
                    blocked.push(true);
                }
            }
        }
    }

    if !present.contains(&true) {
        return None;
    }

    terrain.holes = holes_from_vertices(res, &blocked);
    Some(Window { terrain, min, present })
}

/// Writes the window heights that changed back into `region`. A vertex keeps its exact stored bits unless the brush
/// moved it, so a window that comes back untouched leaves the region byte equal. Returns true if anything changed.
pub fn write_window(grid: &Grid, window: &Window, region: &mut Region3D) -> bool {
    if region.size != grid.region_size || region.check().is_err() {
        return false;
    }

    let [w, _] = window.terrain.resolution;
    let origin = grid.vgrid_of(region.loc, [0, 0]);
    let mut changed = false;
    for (k, h) in window.terrain.heights.iter().enumerate() {
        if !window.present[k] {
            continue;
        }

        let (x, z) = (window.min[0] + (k % w as usize) as i64, window.min[1] + (k / w as usize) as i64);
        let (px, pz) = (x - origin[0], z - origin[1]);
        if px < 0 || pz < 0 || px >= region.size as i64 || pz >= region.size as i64 {
            continue;
        }

        let i = region.index([px as u32, pz as u32]);
        if to_map(grid, region.heights[i]).to_bits() != h.to_bits() {
            region.heights[i] = (*h as f64 / grid.upm) as f32;
            changed = true;
        }
    }

    changed
}

/// Runs `f` on each control word of `region` whose vertex is within `radius` of `center` (map units), with the
/// brush falloff there. Returns true if any word changed.
fn brush_controls(grid: &Grid, region: &mut Region3D, center: DVec3, radius: f64, mut f: impl FnMut(Control, f32) -> Control) -> bool {
    if region.size != grid.region_size || region.check().is_err() {
        return false;
    }

    let c = DVec2::new(center.x, center.z);
    let (min, max) = grid.vgrid_rect(c - DVec2::splat(radius), c + DVec2::splat(radius), 0);
    let origin = grid.vgrid_of(region.loc, [0, 0]);
    let size = region.size as i64;
    let mut changed = false;
    for z in min[1].max(origin[1])..=max[1].min(origin[1] + size - 1) {
        for x in min[0].max(origin[0])..=max[0].min(origin[0] + size - 1) {
            let d = (grid.position([x, z]) - c).length();
            if d > radius {
                continue;
            }

            let k = region.index([(x - origin[0]) as u32, (z - origin[1]) as u32]);
            let next = f(Control(region.control[k]), falloff(d, radius) as f32);
            if next.0 != region.control[k] {
                region.control[k] = next.0;
                changed = true;
            }
        }
    }

    changed
}

/// Sets or clears the hole bit of every vertex within `radius`. Nothing else in the control word changes.
pub fn set_hole_vertices(grid: &Grid, region: &mut Region3D, center: DVec3, radius: f64, hole: bool) -> bool {
    brush_controls(grid, region, center, radius, |c, _| c.with_hole(hole))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintMode {
    /// Terrain3D's Paint: replaces base and overlay under the brush core.
    Base,
    /// Terrain3D's Spray: blends the id in a little per dab.
    Spray,
}

/// One texture dab, ported from Terrain3D's texture tool (terrain_3d_editor.cpp, REPLACE and ADD). `strength` is
/// Terrain3D's 0 to 1. Where the texture takes hold the autoshader bit is cleared like Terrain3D does, or the auto
/// shader would keep drawing over it. Angle and scale are not painted, they and the reserved, hole and navigation
/// bits stay as they were.
pub fn paint_texture(grid: &Grid, region: &mut Region3D, center: DVec3, radius: f64, id: u8, mode: PaintMode, strength: f32) -> bool {
    let id = id & 0x1F;
    brush_controls(grid, region, center, radius, |c, alpha| match mode {
        PaintMode::Base if alpha > 0.5 => c.with_base(id).with_overlay(id).with_blend(0).with_auto(false),
        PaintMode::Base => c,
        PaintMode::Spray => spray(c, alpha, id, strength),
    })
}

fn spray(c: Control, alpha: f32, id: u8, strength: f32) -> Control {
    if alpha * strength * 11.0 <= 0.1 {
        return c;
    }

    let (mut base, mut overlay, mut auto) = (c.base(), c.overlay(), c.auto());
    let mut blend = c.blend() as f32 / 255.0;
    let value = (alpha * (strength * 0.05).clamp(0.004, 0.25)).clamp(0.0, 1.0);
    if id != base && id != overlay {
        blend = if blend >= 0.5 { (blend + value).clamp(0.0, 1.0) } else { (blend - value).clamp(0.0, 1.0) };
        if blend <= 1.0 / 254.0 {
            overlay = id;
        } else if blend >= 1.0 - 1.0 / 254.0 {
            base = id;
        }
    }

    if base == id {
        blend = (blend - value).clamp(0.0, 1.0);
        if alpha > 0.5 && blend < 0.5 {
            auto = false;
        }
    }

    if overlay == id {
        blend = (blend + value).clamp(0.0, 1.0);
        if alpha > 0.5 && blend >= 0.5 {
            auto = false;
        }
    }

    c.with_base(base).with_overlay(overlay).with_blend((blend * 255.0).round().clamp(0.0, 255.0) as u8).with_auto(auto)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain3d::BLANK_CONTROL;
    use std::collections::BTreeMap;

    fn grid(size: u32) -> Grid {
        Grid { region_size: size, spacing: 1.0, upm: 32.0 }
    }

    fn height(loc: Loc, px: u32, pz: u32) -> f32 {
        loc[0] as f32 * 7.0 - loc[1] as f32 * 3.0 + 0.375 * px as f32 - 0.125 * pz as f32
    }

    fn slanted(loc: Loc, size: u32) -> Region3D {
        let mut r = Region3D::blank(loc, size);
        for pz in 0..size {
            for px in 0..size {
                r.heights[(pz * size + px) as usize] = height(loc, px, pz);
            }
        }

        r
    }

    fn world(locs: &[Loc], size: u32) -> BTreeMap<Loc, Region3D> {
        locs.iter().map(|l| (*l, slanted(*l, size))).collect()
    }

    fn name(id: u8) -> String {
        format!("t{id}")
    }

    #[test]
    fn tiles_share_seams_with_their_neighbors_and_repeat_the_edge_without_one() {
        let g = grid(64);
        let w = world(&[[-1, 0], [0, 0], [-1, 1], [0, 1]], 64);
        let get = |l: Loc| w.get(&l);

        let t = tile(&g, &w[&[-1, 0]], get, 1, name).unwrap().terrain;
        assert_eq!((t.resolution, t.cell_size, t.origin), ([65, 65], 32.0, DVec3::new(-64.0 * 32.0, 0.0, 0.0)));
        let mut expected = Vec::new();
        for j in 0..65u32 {
            for i in 0..65u32 {
                let (loc, px, pz) = match (i == 64, j == 64) {
                    (false, false) => ([-1, 0], i, j),
                    (true, false) => ([0, 0], 0, j),
                    (false, true) => ([-1, 1], i, 0),
                    (true, true) => ([0, 1], 0, 0),
                };
                expected.push(height(loc, px, pz) * 32.0);
            }
        }

        assert_eq!(t.heights, expected);
        assert!(t.holes.is_empty() && t.splat.is_empty());
        assert_eq!(t.layers.iter().map(|l| l.material.as_str()).collect::<Vec<_>>(), ["t0"]);

        let t = tile(&g, &w[&[0, 1]], get, 1, name).unwrap().terrain;
        let mut expected = Vec::new();
        for j in 0..65u32 {
            for i in 0..65u32 {
                expected.push(height([0, 1], i.min(63), j.min(63)) * 32.0);
            }
        }

        assert_eq!(t.heights, expected);
    }

    fn hole_cells(t: &Terrain) -> Vec<(u32, u32)> {
        let [cw, _] = t.cells();
        t.holes.iter().enumerate().filter(|(_, h)| **h != 0).map(|(k, _)| (k as u32 % cw, k as u32 / cw)).collect()
    }

    #[test]
    fn a_hole_vertex_makes_exactly_the_cells_around_it_holes() {
        let g = grid(64);
        let mut w = world(&[[2, -3], [3, -3]], 64);
        let r = w.get_mut(&[2, -3]).unwrap();
        for (px, pz) in [(0, 0), (5, 0), (10, 10)] {
            r.control[pz * 64 + px] = Control(BLANK_CONTROL).with_hole(true).0;
        }

        w.get_mut(&[3, -3]).unwrap().control[7 * 64] = Control(0).with_hole(true).0;

        let t = tile(&g, &w[&[2, -3]], |l| w.get(&l), 1, name).unwrap().terrain;
        let mut expected = vec![(0, 0), (4, 0), (5, 0), (9, 9), (10, 9), (63, 6), (9, 10), (10, 10), (63, 7)];
        expected.sort_by_key(|(i, j)| (*j, *i));
        assert_eq!(hole_cells(&t), expected);
    }

    fn mixed(base: u8, overlay: u8, blend: u8) -> Control {
        Control(0).with_base(base).with_overlay(overlay).with_blend(blend)
    }

    #[test]
    fn the_four_most_shown_texture_ids_become_layers() {
        assert!(pick_layers([]).is_empty());
        assert_eq!(pick_layers([mixed(3, 9, 0), mixed(3, 9, 0)]), [3]);
        assert_eq!(pick_layers([mixed(1, 2, 100), mixed(4, 5, 30)]), [4, 1, 2, 5]);
        let six = [mixed(6, 7, 128), mixed(8, 9, 128), mixed(10, 10, 200), mixed(31, 0, 0)];
        assert_eq!(pick_layers(six), [10, 31, 7, 9]);
        assert_eq!(pick_layers([mixed(2, 2, 77)]), [2]);
    }

    #[test]
    fn tile_weights_follow_base_overlay_and_blend() {
        let g = grid(64);
        let mut r = slanted([-1, -1], 64);
        r.control.fill(mixed(4, 4, 0).0);
        r.control[0] = mixed(4, 9, 100).0;
        r.control[1] = mixed(9, 12, 255).0;
        r.control[2] = mixed(30, 31, 10).0;
        let t = tile(&g, &r, |_| None, 1, name).unwrap();
        assert_eq!(t.ids, [4, 12, 30, 9]);
        assert_eq!(t.terrain.layers.iter().map(|l| l.material.clone()).collect::<Vec<_>>(), ["t4", "t12", "t30", "t9"]);
        assert_eq!(t.terrain.splat.len(), 65 * 65 * 4);
        assert_eq!(&t.terrain.splat[..16], &[155, 0, 0, 100, 0, 255, 0, 0, 0, 0, 245, 0, 255, 0, 0, 0]);
        assert_eq!(&t.terrain.splat[64 * 4..65 * 4], &[255, 0, 0, 0], "edge column repeats");
    }

    #[test]
    fn a_window_across_four_regions_round_trips_byte_equal() {
        let g = Grid { region_size: 64, spacing: 2.5, upm: 40.0 };
        let mut w = world(&[[-1, -1], [0, -1], [-1, 0], [0, 0]], 64);
        for r in w.values_mut() {
            for (k, h) in r.heights.iter_mut().enumerate() {
                *h += (k as f32 * 0.137).sin() * 0.3;
            }
        }

        w.get_mut(&[0, 0]).unwrap().heights[3] = f32::from_bits(0x7FC0_1234);
        let before = w.clone();

        let win = window(&g, |l| w.get(&l), [-5, -6], [7, 4]).unwrap();
        assert_eq!((win.terrain.resolution, win.terrain.origin), ([13, 11], DVec3::new(-5.0 * 100.0, 0.0, -6.0 * 100.0)));
        assert!(win.present.iter().all(|p| *p) && win.terrain.holes.is_empty());
        for r in w.values_mut() {
            assert!(!write_window(&g, &win, r));
        }

        for (loc, r) in &w {
            let bits = |r: &Region3D| r.heights.iter().map(|h| h.to_bits()).collect::<Vec<_>>();
            assert_eq!(bits(r), bits(&before[loc]), "{loc:?}");
        }
    }

    #[test]
    fn a_window_marks_vertices_without_a_region_absent() {
        let g = grid(64);
        let w = world(&[[0, 0]], 64);
        let win = window(&g, |l| w.get(&l), [-2, 1], [1, 2]).unwrap();
        assert_eq!(win.present, [false, false, true, true, false, false, true, true]);
        assert_eq!(
            win.terrain.heights,
            [0.0, 0.0, height([0, 0], 0, 1) * 32.0, height([0, 0], 1, 1) * 32.0, 0.0, 0.0, height([0, 0], 0, 2) * 32.0, height([0, 0], 1, 2) * 32.0]
        );
        assert_eq!(win.terrain.holes, [1, 1, 0]);
        assert!(window(&g, |l| w.get(&l), [-9, -9], [-1, -1]).is_none(), "nothing present");
        assert!(window(&g, |l| w.get(&l), [3, 3], [3, 9]).is_none(), "one vertex wide");
        assert!(window(&g, |l| w.get(&l), [3, 3], [2, 9]).is_none(), "inverted");
    }

    #[test]
    fn a_raise_across_a_region_corner_changes_every_side_equally() {
        let g = grid(64);
        let mut w: BTreeMap<Loc, Region3D> = [[-1, -1], [0, -1], [-1, 0], [0, 0]]
            .into_iter()
            .map(|l| {
                let mut r = Region3D::blank(l, 64);
                r.heights.fill(2.0);
                (l, r)
            })
            .collect();
        let (min, max) = g.vgrid_rect(DVec2::splat(-96.0), DVec2::splat(96.0), 1);
        let mut win = window(&g, |l| w.get(&l), min, max).unwrap();
        assert!(win.terrain.raise(DVec3::ZERO, 96.0, 64.0, &td_core::Mirror::NONE));
        for r in w.values_mut() {
            assert!(write_window(&g, &win, r), "{:?}", r.loc);
        }

        let h = |v: [i64; 2]| {
            let (r, k) = g.sample(|l| w.get(&l), v).unwrap();
            r.heights[k]
        };
        assert_eq!(h([0, 0]), 4.0);
        for (dx, dz) in [(1, 0), (1, 1), (2, 1), (0, 2)] {
            let ring = [[dx, dz], [-dx, dz], [dx, -dz], [-dx, -dz], [dz, dx], [-dz, -dx]].map(h);
            assert!(ring.iter().all(|v| *v == ring[0] && *v > 2.0), "{dx} {dz}: {ring:?}");
        }

        assert_eq!(h([3, 0]), 2.0);
        assert_eq!(h([-3, 0]), 2.0);
    }

    fn patterned() -> Control {
        Control(0).with_base(2).with_overlay(2).with_uv_angle(9).with_uv_scale(5).with_reserved(0b1011).with_navigation(true).with_hole(true).with_auto(true)
    }

    fn untouched_bits(c: Control) -> (u8, u8, u8, bool, bool) {
        (c.uv_angle(), c.uv_scale(), c.reserved(), c.navigation(), c.hole())
    }

    #[test]
    fn base_paint_replaces_ids_in_the_core_and_keeps_every_other_bit() {
        let g = Grid { region_size: 64, spacing: 2.0, upm: 32.0 };
        let mut r = slanted([-2, 1], 64);
        r.control.fill(patterned().0);
        let center = g.position(g.vgrid_of([-2, 1], [20, 30]));
        let before = r.clone();
        assert!(paint_texture(&g, &mut r, DVec3::new(center.x, 0.0, center.y), 64.0 * 3.0, 7, PaintMode::Base, 1.0));

        let at = |r: &Region3D, dx: i32, dz: i32| r.control_at([(20 + dx) as u32, (30 + dz) as u32]);
        let core = at(&r, 1, 1);
        assert_eq!((core.base(), core.overlay(), core.blend(), core.auto()), (7, 7, 0, false));
        assert_eq!(untouched_bits(core), untouched_bits(patterned()));
        assert_eq!(at(&r, 2, 2), patterned(), "falloff below 0.5");
        assert_eq!(at(&r, 3, 0), patterned(), "on the radius");
        let changed: Vec<usize> = (0..r.control.len()).filter(|k| r.control[*k] != before.control[*k]).collect();
        assert_eq!(changed.len(), 9);
        assert_eq!(r.heights, before.heights);
    }

    #[test]
    fn spray_blends_an_id_in_and_drops_auto_once_it_shows() {
        let g = grid(64);
        let mut r = Region3D::blank([5, 5], 64);
        r.control.fill(patterned().0);
        let p = g.position(g.vgrid_of([5, 5], [10, 10]));
        let center = DVec3::new(p.x, 3.0, p.y);
        assert!(paint_texture(&g, &mut r, center, 40.0, 5, PaintMode::Spray, 1.0));
        let c = r.control_at([10, 10]);
        assert_eq!((c.base(), c.overlay(), c.blend(), c.auto()), (2, 5, 13, true));
        assert_eq!(untouched_bits(c), untouched_bits(patterned()));

        for _ in 0..9 {
            paint_texture(&g, &mut r, center, 40.0, 5, PaintMode::Spray, 1.0);
        }

        let c = r.control_at([10, 10]);
        assert_eq!((c.base(), c.overlay(), c.blend(), c.auto()), (2, 5, 130, false));
        assert_eq!(untouched_bits(c), untouched_bits(patterned()));

        let mut weak = Region3D::blank([5, 5], 64);
        assert!(!paint_texture(&g, &mut weak, center, 40.0, 5, PaintMode::Spray, 0.009), "alpha * strength * 11 = 0.099");
        assert!(paint_texture(&g, &mut weak, center, 40.0, 5, PaintMode::Spray, 0.01));
    }

    #[test]
    fn hole_vertices_set_and_clear_only_the_hole_bit() {
        let g = grid(64);
        let mut r = slanted([-1, 3], 64);
        let plain = patterned().with_hole(false);
        r.control.fill(plain.0);
        let before = r.clone();
        let p = g.position(g.vgrid_of([-1, 3], [0, 63]));
        let center = DVec3::new(p.x, 0.0, p.y);
        assert!(set_hole_vertices(&g, &mut r, center, 48.0, true));
        let holes: Vec<[u32; 2]> = (0..64u32).flat_map(|z| (0..64u32).map(move |x| [x, z])).filter(|px| r.control_at(*px).hole()).collect();
        assert_eq!(holes, vec![[0, 62], [1, 62], [0, 63], [1, 63]]);
        assert_eq!(r.control_at([0, 63]), plain.with_hole(true));
        assert!(!set_hole_vertices(&g, &mut r, center, 48.0, true), "already holes");
        assert!(set_hole_vertices(&g, &mut r, center, 48.0, false));
        assert_eq!(r, before);
        assert!(!set_hole_vertices(&g, &mut r, DVec3::new(0.0, 0.0, 0.0), 48.0, true), "brush outside the region");
    }

    #[test]
    fn brushes_refuse_a_region_of_another_size() {
        let g = grid(64);
        let mut r = Region3D::blank([0, 0], 128);
        assert!(!set_hole_vertices(&g, &mut r, DVec3::ZERO, 100.0, true));
        assert!(!paint_texture(&g, &mut r, DVec3::ZERO, 100.0, 3, PaintMode::Base, 1.0));
        let w = world(&[[0, 0]], 64);
        let win = window(&g, |l| w.get(&l), [0, 0], [3, 3]).unwrap();
        assert!(!write_window(&g, &win, &mut r));
        assert!(tile(&g, &r, |_| None, 1, name).is_none());
        let mut broken = Region3D::blank([0, 0], 64);
        broken.heights.pop();
        assert!(tile(&g, &broken, |_| None, 1, name).is_none());
    }

    #[test]
    fn lod_tiles_pick_exact_vertices() {
        assert_eq!(lod_strides(64), [1, 4]);
        assert_eq!(lod_strides(256), [1, 4, 16]);
        assert_eq!(lod_strides(2048), [1, 4, 16, 64]);

        let g = Grid { region_size: 64, spacing: 0.25, upm: 32.0 };
        let w = world(&[[-3, 4], [-2, 4]], 64);
        let get = |l: Loc| w.get(&l);
        let t = tile(&g, &w[&[-3, 4]], get, 4, name).unwrap().terrain;
        assert_eq!((t.resolution, t.cell_size, t.origin), ([17, 17], 32.0, DVec3::new(-3.0 * 64.0 * 8.0, 0.0, 4.0 * 64.0 * 8.0)));
        for j in 0..17u32 {
            for i in 0..17u32 {
                let want = match i {
                    16 => height([-2, 4], 0, (j * 4).min(63)),
                    _ => height([-3, 4], i * 4, (j * 4).min(63)),
                };
                assert_eq!(t.heights[(j * 17 + i) as usize], want * 32.0, "{i} {j}");
            }
        }

        for bad in [0, 3, 128] {
            assert!(tile(&g, &w[&[-3, 4]], get, bad, name).is_none(), "stride {bad}");
        }

        let far = Grid { region_size: 1024, spacing: 100.0, upm: 64.0 };
        let big = Region3D::blank([0, 0], 1024);
        assert!(tile(&far, &big, |_| None, 16, name).is_none(), "102400 units per cell");
        assert_eq!(pyramid(&far, &big, |_| None, name).iter().map(|(s, _)| *s).collect::<Vec<_>>(), [1, 4]);
    }

    struct Sampled {
        ids: Vec<u8>,
        heights: Vec<f32>,
        splat: Vec<u8>,
        holes: Vec<u8>,
    }

    impl Sampled {
        fn level(&self, stride: u32, side: u32) -> Level<'_> {
            Level { stride, side, ids: &self.ids, heights: &self.heights, splat: &self.splat, holes: &self.holes }
        }
    }

    /// What the cache stores for a region at `stride`: the owned vertices of its own tile.
    fn sampled(g: &Grid, r: &Region3D, stride: u32) -> Sampled {
        let t = tile(g, r, |_| None, stride, name).unwrap();
        let side = r.size / stride;
        let n = side as usize + 1;
        let (mut heights, mut splat, mut holes) = (Vec::new(), Vec::new(), Vec::new());
        for j in 0..side {
            for i in 0..side {
                heights.push(r.heights[((j * stride) * r.size + i * stride) as usize]);
                if !t.terrain.splat.is_empty() {
                    let k = (j as usize * n + i as usize) * 4;
                    splat.extend_from_slice(&t.terrain.splat[k..k + 4]);
                }

                holes.push(t.terrain.holes.get((j * side + i) as usize).copied().unwrap_or(0));
            }
        }

        Sampled { ids: t.ids, heights, splat, holes }
    }

    fn painted(locs: &[Loc]) -> BTreeMap<Loc, Region3D> {
        let mut w = world(locs, 64);
        for (n, r) in w.values_mut().enumerate() {
            for k in 0..r.control.len() {
                let (base, overlay) = ((k * 7 + n * 3) % 9, (k * 5 + n) % 4 + 10);
                r.control[k] = mixed(base as u8, overlay as u8, ((k * 13 + n * 40) % 256) as u8).0;
            }

            r.control[4 * 64 + 8] = Control(r.control[4 * 64 + 8]).with_hole(true).0;
        }

        w
    }

    #[test]
    fn a_level_tile_matches_the_tile_of_the_region_it_was_sampled_from() {
        let g = Grid { region_size: 64, spacing: 1.5, upm: 32.0 };
        let w = painted(&[[-1, 0], [0, 0], [-1, 1], [0, 1]]);
        let stride = 4;
        let s: BTreeMap<Loc, Sampled> = w.iter().map(|(l, r)| (*l, sampled(&g, r, stride))).collect();
        let lv = |l: Loc| s.get(&l).map(|s| s.level(stride, 16));

        let loc = [-1, 0];
        let full = tile(&g, &w[&loc], |l| w.get(&l), stride, name).unwrap();
        assert_eq!(level_tile(&g, loc, lv(loc).unwrap(), lv([0, 0]), lv([-1, 1]), lv([0, 1]), name), Some(full));

        // Without east, south and south east neighbors the level repeats its last owned vertex.
        let loc = [0, 1];
        let alone = level_tile(&g, loc, lv(loc).unwrap(), lv([1, 1]), lv([0, 2]), lv([1, 2]), name).unwrap().terrain;
        let own = &s[&loc];
        let expected: Vec<f32> = (0..17).flat_map(|j| (0..17).map(move |i| (i, j))).map(|(i, j)| own.heights[j.min(15) * 16 + i.min(15)] * 32.0).collect();
        assert_eq!(alone.heights, expected);
        assert_eq!(alone.holes, own.holes);

        let full = tile(&g, &w[&[-1, 0]], |l| w.get(&l), stride, name).unwrap().terrain;
        assert_eq!((full.resolution, full.cell_size), ([17, 17], 192.0));
        assert!(full.holes.iter().any(|h| *h != 0) && full.splat.len() == 17 * 17 * 4);
    }

    #[test]
    fn a_level_tile_repeats_its_edge_without_neighbors_and_ignores_another_stride() {
        let g = grid(64);
        let w = painted(&[[2, -3], [3, -3]]);
        let a = sampled(&g, &w[&[2, -3]], 4);
        let alone = level_tile(&g, [2, -3], a.level(4, 16), None, None, None, name).unwrap();
        let expected: Vec<f32> = (0..17).flat_map(|j| (0..17).map(move |i| (i, j))).map(|(i, j)| a.heights[j.min(15) * 16 + i.min(15)] * 32.0).collect();
        assert_eq!(alone.terrain.heights, expected);
        assert_eq!(&alone.terrain.splat[16 * 4..17 * 4], &alone.terrain.splat[15 * 4..16 * 4], "the edge column repeats");

        let coarse = sampled(&g, &w[&[3, -3]], 16);
        let mismatched = level_tile(&g, [2, -3], a.level(4, 16), Some(coarse.level(16, 4)), None, None, name);
        assert_eq!(mismatched, Some(alone), "a neighbor at another stride is not a seam");
    }

    #[test]
    fn a_level_that_does_not_fit_the_grid_makes_no_tile() {
        let g = grid(64);
        let w = painted(&[[0, 0]]);
        let a = sampled(&g, &w[&[0, 0]], 4);
        let ok = a.level(4, 16);
        assert!(level_tile(&g, [0, 0], ok, None, None, None, name).is_some());
        assert!(level_tile(&g, [0, 0], Level { side: 15, ..ok }, None, None, None, name).is_none(), "side times stride is not the region size");
        assert!(level_tile(&g, [0, 0], Level { heights: &a.heights[1..], ..ok }, None, None, None, name).is_none());
        assert!(level_tile(&g, [0, 0], Level { splat: &a.splat[1..], ..ok }, None, None, None, name).is_none());
        assert!(level_tile(&g, [0, 0], Level { holes: &a.holes[1..], ..ok }, None, None, None, name).is_none());
        assert!(level_tile(&g, [0, 0], Level { ids: &[], ..ok }, None, None, None, name).is_none());
        assert!(level_tile(&g, [0, 0], Level { stride: 0, ..ok }, None, None, None, name).is_none());
        assert!(level_tile(&Grid { region_size: 128, ..g }, [0, 0], ok, None, None, None, name).is_none());
    }
}
