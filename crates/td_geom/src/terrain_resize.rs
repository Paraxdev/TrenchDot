use crate::heightfield::{MAX_CELL_SIZE, MIN_CELL_SIZE, Terrain};
use td_core::DVec3;

/// Most cells a terrain may have along one axis, 2049 vertices.
pub const MAX_CELLS: u32 = 2048;

/// Whole cells added (positive) or removed (negative) on each side. North is -Z, west is -X.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sides {
    pub north: i32,
    pub south: i32,
    pub west: i32,
    pub east: i32,
}

impl Sides {
    pub fn is_zero(&self) -> bool {
        *self == Sides::default()
    }
}

/// What new vertices get when a side grows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EdgeFill {
    /// The height and paint of the nearest old edge vertex.
    #[default]
    Edge,
    /// Height 0, painted with the first layer.
    Zero,
}

/// Grows or crops a row major grid of `size` elements, each `stride` values wide, by whole elements per side. New
/// elements repeat the nearest old edge element, or are `fill` when given. Returns an empty Vec when `src` does not
/// hold such a grid, `fill` is not one element wide, or nothing is left.
pub fn resize_grid<T: Copy>(src: &[T], size: [u32; 2], stride: usize, sides: Sides, fill: Option<&[T]>) -> Vec<T> {
    let (w, h) = (size[0] as i64, size[1] as i64);
    let new_w = w + sides.west as i64 + sides.east as i64;
    let new_h = h + sides.north as i64 + sides.south as i64;
    if w < 1 || h < 1 || new_w < 1 || new_h < 1 || stride == 0 || src.len() as i64 != w * h * stride as i64 || fill.is_some_and(|f| f.len() != stride) {
        return Vec::new();
    }

    let mut out = Vec::with_capacity((new_w * new_h) as usize * stride);
    for j in 0..new_h {
        let sj = j - sides.north as i64;
        for i in 0..new_w {
            let si = i - sides.west as i64;
            match fill {
                Some(f) if !(0..w).contains(&si) || !(0..h).contains(&sj) => out.extend_from_slice(f),
                _ => {
                    let k = (sj.clamp(0, h - 1) * w + si.clamp(0, w - 1)) as usize * stride;
                    out.extend_from_slice(&src[k..k + stride]);
                }
            }
        }
    }

    out
}

/// The cell counts after growing or cropping `cells` by `sides`. Refuses a result below 1 or above [`MAX_CELLS`] cells
/// on an axis, and one that keeps none of the old cells.
pub fn resized_cells(cells: [u32; 2], sides: Sides) -> Result<[u32; 2], String> {
    let axes = [("x", cells[0], sides.west, sides.east), ("z", cells[1], sides.north, sides.south)];
    let mut out = [0; 2];
    for (k, (axis, n, before, after)) in axes.into_iter().enumerate() {
        let (n, before, after) = (n as i64, before as i64, after as i64);
        let total = n + before + after;
        if total < 1 {
            return Err(format!("a terrain needs at least 1 cell along each axis, this leaves {} along {axis}", total.max(0)));
        }

        if total > MAX_CELLS as i64 {
            return Err(format!("a terrain has at most {MAX_CELLS} cells along each axis, this makes {total} along {axis}"));
        }

        if n.min(n + after) - (-before).max(0) < 1 {
            return Err(format!("this crops away every cell along {axis}, at least 1 has to stay"));
        }

        out[k] = total as u32;
    }

    Ok(out)
}

impl Terrain {
    /// Grows or crops the grid by whole cells per side. Kept vertices keep their height, paint and world position and
    /// kept cells their holes. The origin moves by whole cells and is not snapped, so an off grid origin stays off grid.
    pub fn resized(&self, sides: Sides, fill: EdgeFill) -> Result<Terrain, String> {
        self.check_data()?;
        if sides.is_zero() {
            return Ok(self.clone());
        }

        let cells = resized_cells(self.cells(), sides)?;
        let zero = fill == EdgeFill::Zero;
        let splat = if self.splat.is_empty() { Vec::new() } else { resize_grid(&self.splat, self.resolution, 4, sides, zero.then_some(&[255, 0, 0, 0][..])) };
        let holes = resize_grid(&self.holes, self.cells(), 1, sides, Some(&[0]));
        Ok(Terrain {
            origin: self.origin - DVec3::new(sides.west as f64 * self.cell_size, 0.0, sides.north as f64 * self.cell_size),
            resolution: [cells[0] + 1, cells[1] + 1],
            cell_size: self.cell_size,
            heights: resize_grid(&self.heights, self.resolution, 1, sides, zero.then_some(&[0.0][..])),
            layers: self.layers.clone(),
            splat,
            holes: if holes.iter().any(|h| *h != 0) { holes } else { Vec::new() },
            chunk_cells: self.chunk_cells,
        })
    }

    /// Side of a tile in world units, `chunk_cells` cells.
    pub fn tile_size(&self) -> f64 {
        self.chunk_cells.max(1) as f64 * self.cell_size
    }

    /// The tile under world `p`, on the grid of tiles anchored at the origin. Tiles outside the grid have negative or
    /// past the end indices.
    pub fn tile_at(&self, p: DVec3) -> [i64; 2] {
        let size = self.tile_size();
        [((p.x - self.origin.x) / size).floor() as i64, ((p.z - self.origin.z) / size).floor() as i64]
    }

    /// Adds the tile under `p` as solid ground. A tile outside the grid grows it with flat ground at height 0, and the
    /// cells it gains around the tile become holes, so only the tile shows. Refuses growing past [`MAX_CELLS`].
    pub fn add_tile(&mut self, p: DVec3) -> bool {
        let n = self.chunk_cells.max(1) as i64;
        let [cw, cd] = self.cells().map(|c| c as i64);
        let [ti, tj] = self.tile_at(p);
        let sides = self.tile_sides(p);
        let Ok(mut grown) = self.resized(sides, EdgeFill::Zero) else { return false };
        let [gw, gd] = grown.cells().map(|c| c as i64);
        let (ow, on) = (sides.west as i64, sides.north as i64);
        let (i0, j0) = (ti * n + ow, tj * n + on);
        let mut holes = if grown.holes.is_empty() { vec![0; (gw * gd) as usize] } else { std::mem::take(&mut grown.holes) };
        for j in 0..gd {
            for i in 0..gw {
                let k = (j * gw + i) as usize;
                if (i0..i0 + n).contains(&i) && (j0..j0 + n).contains(&j) {
                    holes[k] = 0;
                } else if !(ow..ow + cw).contains(&i) || !(on..on + cd).contains(&j) {
                    holes[k] = 1;
                }
            }
        }

        grown.holes = if holes.iter().any(|h| *h != 0) { holes } else { Vec::new() };
        if grown == *self {
            return false;
        }

        *self = grown;
        true
    }

    /// Why [`Terrain::add_tile`] cannot add the tile under `p`, when the terrain would grow past [`MAX_CELLS`].
    pub fn add_tile_error(&self, p: DVec3) -> Option<String> {
        self.resized(self.tile_sides(p), EdgeFill::Zero).err()
    }

    fn tile_sides(&self, p: DVec3) -> Sides {
        let n = self.chunk_cells.max(1) as i64;
        let [cw, cd] = self.cells().map(|c| c as i64);
        let [ti, tj] = self.tile_at(p);
        Sides {
            north: (-tj * n).max(0) as i32,
            south: ((tj + 1) * n - cd).max(0) as i32,
            west: (-ti * n).max(0) as i32,
            east: ((ti + 1) * n - cw).max(0) as i32,
        }
    }

    /// Cuts the tile under `p` out as holes, then crops the border tiles that are holes throughout. The last solid
    /// tile stays.
    pub fn remove_tile(&mut self, p: DVec3) -> bool {
        let n = self.chunk_cells.max(1) as i64;
        let [cw, cd] = self.cells().map(|c| c as i64);
        let [ti, tj] = self.tile_at(p);
        if ti < 0 || tj < 0 || ti * n >= cw || tj * n >= cd {
            return false;
        }

        let mut holes = if self.holes.len() == (cw * cd) as usize { self.holes.clone() } else { vec![0; (cw * cd) as usize] };
        for j in tj * n..((tj + 1) * n).min(cd) {
            for i in ti * n..((ti + 1) * n).min(cw) {
                holes[(j * cw + i) as usize] = 1;
            }
        }

        let solid: Vec<(i64, i64)> = (0..cd).flat_map(|j| (0..cw).map(move |i| (i, j))).filter(|(i, j)| holes[(j * cw + i) as usize] == 0).collect();
        if solid.is_empty() || holes == self.holes {
            return false;
        }

        let tile = |c: i64| c.div_euclid(n);
        let (min_i, max_i) = (solid.iter().map(|c| tile(c.0)).min().unwrap(), solid.iter().map(|c| tile(c.0)).max().unwrap());
        let (min_j, max_j) = (solid.iter().map(|c| tile(c.1)).min().unwrap(), solid.iter().map(|c| tile(c.1)).max().unwrap());
        let sides = Sides {
            north: -(min_j * n) as i32,
            south: -(cd - ((max_j + 1) * n).min(cd)) as i32,
            west: -(min_i * n) as i32,
            east: -(cw - ((max_i + 1) * n).min(cw)) as i32,
        };
        let mut cut = self.clone();
        cut.holes = holes;
        match cut.resized(sides, EdgeFill::Edge) {
            Ok(t) => *self = t,
            Err(_) => *self = cut,
        }

        true
    }

    /// The cell counts with cells of `cell_size`, keeping the extent to the nearest cell. Refuses a cell size outside
    /// the range and a result above [`MAX_CELLS`] cells on an axis.
    pub fn rescaled_cells(&self, cell_size: f64) -> Result<[u32; 2], String> {
        if !(MIN_CELL_SIZE..=MAX_CELL_SIZE).contains(&cell_size) {
            return Err(format!("a terrain cell size has to be within {MIN_CELL_SIZE} to {MAX_CELL_SIZE}, got {cell_size}"));
        }

        let mut out = [0; 2];
        for (k, axis) in ["x", "z"].into_iter().enumerate() {
            let n = (self.cells()[k] as f64 * self.cell_size / cell_size).round().max(1.0);
            if n > MAX_CELLS as f64 {
                return Err(format!("a terrain has at most {MAX_CELLS} cells along each axis, this makes {n} along {axis}"));
            }

            out[k] = n as u32;
        }

        Ok(out)
    }

    /// True when cells of `cell_size` are coarser than the current ones, so the ground loses shape on resampling.
    pub fn loses_detail(&self, cell_size: f64) -> bool {
        cell_size > self.cell_size
    }

    /// Changes the cell size and keeps the origin and the extent to the nearest cell. Heights and paint are bilinear at
    /// the new vertices, paint renormalized to 255 per vertex, and every new cell takes the hole of the old cell under
    /// its center. Vertices past the old edge repeat it.
    pub fn rescaled(&self, cell_size: f64) -> Result<Terrain, String> {
        self.check_data()?;
        let cells = self.rescaled_cells(cell_size)?;
        if cell_size == self.cell_size {
            return Ok(self.clone());
        }

        let res = [cells[0] + 1, cells[1] + 1];
        let scale = cell_size / self.cell_size;
        let old = self.cells();
        let corners = |i: u32, j: u32| {
            let u = (i as f64 * scale).min(old[0] as f64);
            let v = (j as f64 * scale).min(old[1] as f64);
            let (i0, j0) = ((u as u32).min(old[0] - 1), (v as u32).min(old[1] - 1));
            let (fu, fv) = (u - i0 as f64, v - j0 as f64);
            [(i0, j0, (1.0 - fu) * (1.0 - fv)), (i0 + 1, j0, fu * (1.0 - fv)), (i0, j0 + 1, (1.0 - fu) * fv), (i0 + 1, j0 + 1, fu * fv)]
        };

        let mut heights = Vec::with_capacity((res[0] * res[1]) as usize);
        let mut splat = Vec::with_capacity(if self.splat.is_empty() { 0 } else { heights.capacity() * 4 });
        for j in 0..res[1] {
            for i in 0..res[0] {
                let c = corners(i, j);
                heights.push(c.iter().map(|&(a, b, w)| self.height(a, b) * w).sum::<f64>() as f32);
                if !self.splat.is_empty() {
                    let mut mix = [0.0f64; 4];
                    for &(a, b, w) in &c {
                        for (m, l) in mix.iter_mut().zip(self.weights(a, b)) {
                            *m += l as f64 * w;
                        }
                    }

                    splat.extend(to_bytes(mix));
                }
            }
        }

        let mut holes = Vec::new();
        if self.holes.iter().any(|h| *h != 0) {
            let near = |c: u32, k: usize| (((c as f64 + 0.5) * scale) as u32).min(old[k] - 1);
            holes = (0..cells[1]).flat_map(|cj| (0..cells[0]).map(move |ci| (ci, cj))).map(|(ci, cj)| self.is_hole(near(ci, 0), near(cj, 1)) as u8).collect();
            if holes.iter().all(|h| *h == 0) {
                holes.clear();
            }
        }

        Ok(Terrain { origin: self.origin, resolution: res, cell_size, heights, layers: self.layers.clone(), splat, holes, chunk_cells: self.chunk_cells })
    }
}

/// Blend weights as bytes summing to exactly 255. Rounding the running sum keeps every byte within 1 of its weight.
fn to_bytes(weights: [f64; 4]) -> [u8; 4] {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 || total.is_nan() {
        return [255, 0, 0, 0];
    }

    let mut out = [0u8; 4];
    let (mut run, mut prev) = (0.0, 0u32);
    for (k, w) in weights.iter().enumerate() {
        run += w;
        let at = if k == 3 { 255 } else { ((run / total * 255.0).round() as u32).clamp(prev, 255) };
        out[k] = (at - prev) as u8;
        prev = at;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heightfield::TerrainLayer;

    fn sides(north: i32, south: i32, west: i32, east: i32) -> Sides {
        Sides { north, south, west, east }
    }

    fn bumpy(i: u32, j: u32) -> f32 {
        3.5 * i as f32 - 2.25 * j as f32 + 0.5 * (i * j) as f32 + 1.0
    }

    fn paint(i: u32, j: u32) -> [u8; 4] {
        [(i * 10 + j) as u8, 100 + i as u8, 200 - j as u8, 3]
    }

    /// A slanted, non planar terrain of `cells` off the origin, painted per vertex, with holes at `holes`.
    fn slanted(cells: [u32; 2], holes: &[(u32, u32)]) -> Terrain {
        let mut t = Terrain::new(DVec3::new(-96.5, 12.0, 40.25), [cells[0] + 1, cells[1] + 1], 32.0, "grass");
        t.layers.push(TerrainLayer::new("rock", 2.0));
        t.chunk_cells = 16;
        for j in 0..=cells[1] {
            for i in 0..=cells[0] {
                let k = t.index(i, j);
                t.heights[k] = bumpy(i, j);
                t.splat.extend_from_slice(&paint(i, j));
            }
        }

        if !holes.is_empty() {
            t.holes = vec![0; (cells[0] * cells[1]) as usize];
            for (ci, cj) in holes {
                t.holes[(cj * cells[0] + ci) as usize] = 1;
            }
        }

        t
    }

    /// Every vertex of `new` at (i, j) is the old vertex at (i - di, j - dj) clamped onto the old grid, with world x and
    /// z on the new grid, and every kept old vertex sits at its old world position.
    fn assert_edge_copy(old: &Terrain, new: &Terrain, di: i64, dj: i64) {
        new.check_data().unwrap();
        let [w, d] = old.resolution.map(|r| r as i64);
        let mut heights = Vec::new();
        let mut splat = Vec::new();
        for j in 0..new.resolution[1] as i64 {
            for i in 0..new.resolution[0] as i64 {
                let (si, sj) = ((i - di).clamp(0, w - 1) as u32, (j - dj).clamp(0, d - 1) as u32);
                heights.push(bumpy(si, sj));
                splat.extend_from_slice(&paint(si, sj));
                if (0..w).contains(&(i - di)) && (0..d).contains(&(j - dj)) {
                    let (a, b) = (new.vertex(i as u32, j as u32), old.vertex(si, sj));
                    assert!((a - b).length() < 1e-9, "vertex {i},{j} moved from {b} to {a}");
                }
            }
        }

        assert_eq!(new.heights, heights);
        assert_eq!(new.splat, splat);
        assert_eq!((new.cell_size, &new.layers, new.chunk_cells), (old.cell_size, &old.layers, old.chunk_cells));
    }

    fn holes_at(cells: [u32; 2], at: &[(u32, u32)]) -> Vec<u8> {
        let mut v = vec![0; (cells[0] * cells[1]) as usize];
        for (ci, cj) in at {
            v[(cj * cells[0] + ci) as usize] = 1;
        }

        v
    }

    #[test]
    fn grow_three_sides_keeps_every_old_vertex_in_place() {
        let old = slanted([4, 3], &[(1, 0), (3, 2)]);
        let new = old.resized(sides(3, 0, 2, 1), EdgeFill::Edge).unwrap();
        assert_eq!(new.resolution, [8, 7]);
        assert_eq!(new.origin, DVec3::new(-160.5, 12.0, -55.75));
        assert_edge_copy(&old, &new, 2, 3);
        assert_eq!(new.height(0, 0), bumpy(0, 0) as f64, "the north west corner repeats the old corner");
        assert_eq!(new.height(7, 0), bumpy(4, 0) as f64, "the north east corner repeats the old corner");
        assert_eq!(new.height(0, 6), bumpy(0, 3) as f64);
        assert_eq!(new.holes, holes_at([7, 6], &[(3, 3), (5, 5)]));
    }

    #[test]
    fn zero_fill_writes_flat_first_layer_ground() {
        let old = slanted([4, 3], &[(2, 1)]);
        let new = old.resized(sides(0, 2, 0, 1), EdgeFill::Zero).unwrap();
        new.check_data().unwrap();
        assert_eq!(new.resolution, [6, 6]);
        assert_eq!(new.origin, old.origin);
        for j in 0..6 {
            for i in 0..6 {
                let k = new.index(i, j);
                let kept = i <= 4 && j <= 3;
                let (h, p) = if kept { (bumpy(i, j), paint(i, j)) } else { (0.0, [255, 0, 0, 0]) };
                assert_eq!((new.heights[k], &new.splat[k * 4..k * 4 + 4]), (h, &p[..]), "vertex {i},{j}");
            }
        }

        assert_eq!(new.holes, holes_at([5, 5], &[(2, 1)]));

        let mut bare = slanted([4, 3], &[]);
        bare.splat.clear();
        let grown = bare.resized(sides(1, 1, 1, 1), EdgeFill::Zero).unwrap();
        grown.check_data().unwrap();
        assert!(grown.splat.is_empty() && grown.holes.is_empty());
        assert_eq!(grown.height(0, 0), 0.0);
        assert_eq!(grown.height(1, 1), bumpy(0, 0) as f64);
    }

    #[test]
    fn crop_north_and_grow_east_keeps_the_window() {
        let old = slanted([4, 3], &[(1, 0), (3, 2)]);
        let new = old.resized(sides(-1, 0, 0, 2), EdgeFill::Edge).unwrap();
        assert_eq!(new.resolution, [7, 3]);
        assert_eq!(new.origin, DVec3::new(-96.5, 12.0, 72.25));
        assert_edge_copy(&old, &new, 0, -1);
        assert_eq!(new.holes, holes_at([6, 2], &[(3, 1)]));

        let top_row = slanted([4, 3], &[(0, 0), (3, 0)]);
        let cropped = top_row.resized(sides(-1, 0, 0, 2), EdgeFill::Edge).unwrap();
        cropped.check_data().unwrap();
        assert!(cropped.holes.is_empty(), "all holes cropped away leaves no hole data");
    }

    #[test]
    fn crop_every_side() {
        let old = slanted([6, 5], &[(0, 0), (2, 2), (2, 1), (3, 1), (5, 4)]);
        let new = old.resized(sides(-1, -2, -1, -3), EdgeFill::Zero).unwrap();
        assert_eq!(new.resolution, [3, 3]);
        assert_eq!(new.origin, DVec3::new(-64.5, 12.0, 72.25));
        assert_edge_copy(&old, &new, -1, -1);
        assert_eq!(new.holes, holes_at([2, 2], &[(1, 1), (1, 0)]));
    }

    #[test]
    fn zero_one_and_many_cells() {
        let old = slanted([4, 3], &[(1, 0)]);
        assert_eq!(old.resized(Sides::default(), EdgeFill::Zero).unwrap(), old);

        let one = old.resized(sides(0, 0, 0, 1), EdgeFill::Edge).unwrap();
        assert_eq!(one.resolution, [6, 4]);
        assert_eq!(one.origin, old.origin);
        assert_edge_copy(&old, &one, 0, 0);
        assert_eq!(one.holes, holes_at([5, 3], &[(1, 0)]));

        let many = old.resized(sides(20, 0, 37, 0), EdgeFill::Edge).unwrap();
        assert_eq!(many.resolution, [42, 24]);
        assert_eq!(many.origin, DVec3::new(-96.5 - 37.0 * 32.0, 12.0, 40.25 - 20.0 * 32.0));
        assert_edge_copy(&old, &many, 37, 20);
        assert_eq!(many.holes, holes_at([41, 23], &[(38, 20)]));
    }

    #[test]
    fn crop_down_to_one_cell_and_no_further() {
        let old = slanted([4, 3], &[(3, 2)]);
        let one = old.resized(sides(-2, 0, -3, 0), EdgeFill::Edge).unwrap();
        assert_eq!(one.resolution, [2, 2]);
        assert_edge_copy(&old, &one, -3, -2);
        assert_eq!(one.holes, vec![1]);

        let err = old.resized(sides(0, 0, -4, 0), EdgeFill::Edge).unwrap_err();
        assert!(err.contains("at least 1 cell") && err.contains("leaves 0 along x"), "{err}");
        let err = old.resized(sides(-2, -1, 0, 0), EdgeFill::Edge).unwrap_err();
        assert!(err.contains("leaves 0 along z"), "{err}");
        let err = old.resized(sides(0, 0, -9, 0), EdgeFill::Edge).unwrap_err();
        assert!(err.contains("leaves 0 along x"), "{err}");
    }

    #[test]
    fn grow_up_to_the_cell_limit_and_no_further() {
        let strip = slanted([10, 1], &[]);
        let full = strip.resized(sides(0, 0, 0, 2038), EdgeFill::Edge).unwrap();
        assert_eq!(full.resolution, [MAX_CELLS + 1, 2]);
        assert_edge_copy(&strip, &full, 0, 0);

        let err = strip.resized(sides(0, 0, 0, 2039), EdgeFill::Edge).unwrap_err();
        assert!(err.contains("at most 2048") && err.contains("2049 along x"), "{err}");
        assert_eq!(resized_cells([1, 10], sides(2000, 38, 0, 0)), Ok([1, 2048]));
        let err = resized_cells([1, 10], sides(2000, 39, 0, 0)).unwrap_err();
        assert!(err.contains("2049 along z"), "{err}");
        assert!(resized_cells([1, 10], sides(i32::MAX, i32::MAX, 0, 0)).is_err());
    }

    #[test]
    fn a_crop_has_to_keep_an_old_cell() {
        assert_eq!(resized_cells([4, 3], sides(0, 0, -3, 3)), Ok([4, 3]));
        let err = resized_cells([4, 3], sides(0, 0, -4, 4)).unwrap_err();
        assert!(err.contains("crops away every cell along x"), "{err}");
        assert_eq!(resized_cells([4, 3], sides(5, -2, 0, 0)), Ok([4, 6]));
        let err = resized_cells([4, 3], sides(5, -3, 0, 0)).unwrap_err();
        assert!(err.contains("crops away every cell along z"), "{err}");
    }

    #[test]
    fn a_broken_terrain_is_refused() {
        let mut t = slanted([4, 3], &[]);
        t.heights.pop();
        let err = t.resized(sides(1, 0, 0, 0), EdgeFill::Edge).unwrap_err();
        assert!(err.contains("needs 20 heights"), "{err}");
        assert!(t.resized(Sides::default(), EdgeFill::Edge).is_err());
    }

    #[test]
    fn resize_grid_by_stride_and_fill() {
        let src = [1, 2, 3, 4, 5, 6];
        let s = sides(1, -1, 1, 0);
        assert_eq!(resize_grid(&src, [3, 2], 1, s, None), vec![1, 1, 2, 3, 1, 1, 2, 3]);
        assert_eq!(resize_grid(&src, [3, 2], 1, s, Some(&[9])), vec![9, 9, 9, 9, 9, 1, 2, 3]);
        assert_eq!(resize_grid(&src, [3, 2], 1, sides(0, 0, -1, -1), None), vec![2, 5]);

        let wide = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(resize_grid(&wide, [2, 1], 4, sides(0, 0, 0, 1), None), vec![1, 2, 3, 4, 5, 6, 7, 8, 5, 6, 7, 8]);
        assert_eq!(resize_grid(&wide, [2, 1], 4, sides(0, 1, -1, 0), Some(&[0, 0, 0, 9])), vec![5, 6, 7, 8, 0, 0, 0, 9]);

        assert!(resize_grid(&src, [3, 3], 1, s, None).is_empty(), "too short a source");
        assert!(resize_grid(&wide, [2, 1], 4, s, Some(&[0])).is_empty(), "a fill narrower than one element");
        assert!(resize_grid(&src, [3, 2], 1, sides(0, 0, -3, 0), None).is_empty(), "nothing left");
        assert!(resize_grid::<u8>(&[], [0, 0], 1, s, Some(&[0])).is_empty());
    }

    #[test]
    fn odd_west_crop_keeps_a_plane_and_the_vertex_heights() {
        let mut plane = slanted([5, 4], &[]);
        for j in 0..5 {
            for i in 0..6 {
                let k = plane.index(i, j);
                plane.heights[k] = 1.5 * i as f32 - 0.625 * j as f32;
            }
        }

        let cropped = plane.resized(sides(0, 0, -1, 0), EdgeFill::Edge).unwrap();
        cropped.check_data().unwrap();
        for sj in 0..=16 {
            for si in 0..=16 {
                let (x, z) = (-64.5 + si as f64 * 8.0 + 0.3, 40.25 + sj as f64 * 8.0 + 0.2);
                let inside = (-64.5..=63.5).contains(&x) && (40.25..=168.25).contains(&z);
                let (a, b) = (cropped.height_at(x, z), plane.height_at(x, z));
                match (a, b) {
                    (Some(a), Some(b)) => assert!((a - b).abs() < 1e-6, "at {x},{z}: {a} vs {b}"),
                    _ => assert!(!inside && a.is_none(), "at {x},{z}"),
                }
            }
        }

        let bumpy_terrain = slanted([5, 4], &[]);
        let cropped = bumpy_terrain.resized(sides(0, 0, -1, 0), EdgeFill::Edge).unwrap();
        assert_edge_copy(&bumpy_terrain, &cropped, -1, 0);
    }

    /// A slanted plane of `cells` off the origin, unpainted so it can be long.
    fn planar(cells: [u32; 2]) -> Terrain {
        let mut t = Terrain::new(DVec3::new(-96.5, 12.0, 40.25), [cells[0] + 1, cells[1] + 1], 32.0, "grass");
        t.layers.push(TerrainLayer::new("rock", 2.0));
        t.chunk_cells = 16;
        for j in 0..=cells[1] {
            for i in 0..=cells[0] {
                let k = t.index(i, j);
                t.heights[k] = 2.5 * i as f32 - 1.25 * j as f32 + 7.0;
            }
        }

        t
    }

    #[test]
    fn halving_the_cell_doubles_the_cells_on_the_plane() {
        let old = planar([4, 3]);
        let new = old.rescaled(16.0).unwrap();
        new.check_data().unwrap();
        assert_eq!((new.origin, new.resolution, new.cell_size), (old.origin, [9, 7], 16.0));
        assert_eq!((&new.layers, new.chunk_cells), (&old.layers, old.chunk_cells));
        assert_eq!(new.size(), old.size());
        let plane: Vec<f32> = (0..7).flat_map(|j| (0..9).map(move |i| 1.25 * i as f32 - 0.625 * j as f32 + 7.0)).collect();
        assert_eq!(new.heights, plane);

        let bumpy_terrain = slanted([4, 3], &[]);
        let fine = bumpy_terrain.rescaled(16.0).unwrap();
        for j in 0..=3 {
            for i in 0..=4 {
                assert_eq!(fine.height(2 * i, 2 * j), bumpy(i, j) as f64, "old vertex {i},{j} keeps its height");
                assert!((fine.vertex(2 * i, 2 * j) - bumpy_terrain.vertex(i, j)).length() < 1e-9);
            }
        }

        let mid = (bumpy(1, 2) + bumpy(2, 2) + bumpy(1, 3) + bumpy(2, 3)) as f64 / 4.0;
        assert!((fine.height(3, 5) - mid).abs() < 1e-5, "a cell center is the mean of its corners");
    }

    #[test]
    fn doubling_the_cell_rounds_half_away_from_zero() {
        let old = planar([3, 5]);
        assert_eq!(old.rescaled_cells(64.0), Ok([2, 3]));
        let new = old.rescaled(64.0).unwrap();
        new.check_data().unwrap();
        assert_eq!((new.origin, new.resolution, new.cell_size), (old.origin, [3, 4], 64.0));
        let expected: Vec<f32> = (0..4).flat_map(|j| (0..3).map(move |i| 2.5 * (2 * i).min(3) as f32 - 1.25 * (2 * j).min(5) as f32 + 7.0)).collect();
        assert_eq!(new.heights, expected, "vertices past the old edge repeat it");
        assert_eq!(old.rescaled_cells(1e5), Ok([1, 1]), "never below one cell");
    }

    #[test]
    fn paint_mixes_bilinearly_and_sums_to_255() {
        let mut old = slanted([2, 1], &[]);
        let row = [[255, 0, 0, 0], [0, 0, 255, 0], [0, 100, 0, 155]];
        let bottom = [[100, 50, 0, 0], [0, 0, 0, 0], [10, 20, 30, 40]];
        old.splat = row.iter().chain(bottom.iter()).flatten().copied().collect();
        let new = old.rescaled(16.0).unwrap();
        new.check_data().unwrap();
        assert_eq!(new.resolution, [5, 3]);
        let px = |i: u32, j: u32| {
            let k = new.index(i, j) * 4;
            [new.splat[k], new.splat[k + 1], new.splat[k + 2], new.splat[k + 3]]
        };

        assert_eq!(px(0, 0), [255, 0, 0, 0]);
        assert_eq!(px(1, 0), [128, 0, 127, 0], "midway between two painted vertices");
        assert_eq!(px(4, 0), [0, 100, 0, 155]);
        assert_eq!(px(0, 2), [170, 85, 0, 0], "an old vertex comes back renormalized");
        assert_eq!(px(2, 2), [255, 0, 0, 0], "an unpainted vertex reads as the first layer");
        for j in 0..3 {
            for i in 0..5 {
                let p = px(i, j);
                assert_eq!(p.iter().map(|b| *b as u32).sum::<u32>(), 255, "vertex {i},{j}: {p:?}");
                let (u, v) = (i as f64 / 2.0, j as f64 / 2.0);
                let (i0, j0) = ((u as u32).min(1), 0);
                let (fu, fv) = (u - i0 as f64, v - j0 as f64);
                for l in 0..4 {
                    let w = |a: u32, b: u32| old.weights(a, b)[l] as f64;
                    let mix = (w(i0, j0) * (1.0 - fu) + w(i0 + 1, j0) * fu) * (1.0 - fv) + (w(i0, j0 + 1) * (1.0 - fu) + w(i0 + 1, j0 + 1) * fu) * fv;
                    assert!((p[l] as f64 - mix * 255.0).abs() <= 1.0, "vertex {i},{j} layer {l}: {p:?} vs {mix}");
                }
            }
        }

        let bare = slanted([2, 1], &[]);
        let bare = Terrain { splat: Vec::new(), ..bare };
        assert!(bare.rescaled(16.0).unwrap().splat.is_empty(), "no paint stays no paint");
    }

    #[test]
    fn holes_take_the_nearest_old_cell() {
        let old = slanted([4, 3], &[(1, 0), (3, 2)]);
        let fine = old.rescaled(16.0).unwrap();
        assert_eq!(fine.holes, holes_at([8, 6], &[(2, 0), (3, 0), (2, 1), (3, 1), (6, 4), (7, 4), (6, 5), (7, 5)]));

        let coarse = old.rescaled(64.0).unwrap();
        assert_eq!(coarse.resolution, [3, 3]);
        assert_eq!(coarse.holes, holes_at([2, 2], &[(1, 1)]));

        let lost = slanted([4, 3], &[(0, 0)]).rescaled(64.0).unwrap();
        assert!(lost.holes.is_empty(), "no hole left gives no holes");
    }

    #[test]
    fn the_same_cell_size_changes_nothing() {
        let old = slanted([4, 3], &[(1, 0)]);
        assert_eq!(old.rescaled(32.0).unwrap(), old);
        assert!(!old.loses_detail(32.0));
        assert!(old.loses_detail(32.000001));
        assert!(!old.loses_detail(31.5));
    }

    #[test]
    fn the_cell_size_has_to_stay_in_range() {
        let mut tiny = slanted([1, 1], &[]);
        tiny.cell_size = 0.002;
        assert_eq!(tiny.rescaled(MIN_CELL_SIZE).unwrap().resolution, [3, 3]);
        let mut huge = slanted([2, 2], &[]);
        huge.cell_size = 50000.0;
        assert_eq!(huge.rescaled(MAX_CELL_SIZE).unwrap().resolution, [2, 2]);

        for bad in [0.000999, 100000.1, 0.0, -32.0, f64::NAN, f64::INFINITY] {
            let err = tiny.rescaled(bad).unwrap_err();
            assert!(err.contains("0.001 to 100000"), "{bad}: {err}");
            assert!(tiny.rescaled_cells(bad).is_err());
        }
    }

    #[test]
    fn the_cell_count_has_to_stay_under_the_limit() {
        let strip = planar([683, 1]);
        let full = strip.rescaled(10.671875).unwrap();
        full.check_data().unwrap();
        assert_eq!(full.resolution, [MAX_CELLS + 1, 4]);
        let err = strip.rescaled(32.0 / 3.0).unwrap_err();
        assert!(err.contains("at most 2048") && err.contains("2049 along x"), "{err}");
        let err = planar([1, 683]).rescaled_cells(32.0 / 3.0).unwrap_err();
        assert!(err.contains("2049 along z"), "{err}");
    }

    #[test]
    fn a_broken_terrain_is_not_rescaled() {
        let mut t = slanted([4, 3], &[]);
        t.splat.pop();
        assert!(t.rescaled(16.0).unwrap_err().contains("splat"));
        assert!(t.rescaled(32.0).is_err());
    }

    #[test]
    fn adding_a_tile_beside_the_terrain_grows_it_with_only_that_tile_solid() {
        let mut t = slanted([16, 16], &[]);
        let size = t.tile_size();
        assert_eq!(size, 512.0);
        let corner = t.origin;
        assert!(!t.add_tile(corner + DVec3::new(100.0, 0.0, 100.0)), "the tile is already there");

        let beside = corner + DVec3::new(size * 1.5, 0.0, size * -0.5);
        assert!(t.add_tile(beside));
        assert_eq!(t.cells(), [32, 32]);
        assert_eq!(t.origin, corner - DVec3::new(0.0, 0.0, size));
        assert!(!t.is_hole(20, 3) && !t.is_hole(5, 20));
        assert!(t.is_hole(3, 3) && t.is_hole(20, 20), "the corner the grid gained around the tile is a hole");
        assert_eq!(t.height(0, 16), bumpy(0, 0) as f64, "old heights stay at their place");
        assert_eq!(t.height(24, 8), 0.0);
        assert!(!t.add_tile(beside));

        assert!(t.add_tile(corner + DVec3::new(size * 1.5, 0.0, size * 0.5)));
        assert!(!t.is_hole(20, 20), "a tile cut out inside the grid comes back solid");
    }

    #[test]
    fn removing_tiles_cuts_holes_and_crops_empty_border_tiles_but_keeps_the_last() {
        let mut t = slanted([32, 16], &[]);
        let size = t.tile_size();
        let corner = t.origin;
        let west = corner + DVec3::new(size * 0.5, 0.0, size * 0.5);
        let east = corner + DVec3::new(size * 1.5, 0.0, size * 0.5);
        assert!(!t.remove_tile(corner - DVec3::new(10.0, 0.0, 10.0)), "nothing to remove outside the grid");

        assert!(t.remove_tile(west));
        assert_eq!(t.cells(), [16, 16]);
        assert_eq!(t.origin, corner + DVec3::new(size, 0.0, 0.0));
        assert!(t.holes.is_empty());
        assert_eq!(t.height(0, 0), bumpy(16, 0) as f64);

        assert!(!t.remove_tile(east), "the last tile stays");
        assert!(t.add_tile(west + DVec3::new(0.0, 0.0, size)));
        assert_eq!(t.cells(), [32, 32]);
        assert!(t.remove_tile(east));
        assert_eq!((t.cells(), t.origin), ([16, 16], corner + DVec3::new(0.0, 0.0, size)));
        assert!(t.holes.is_empty());
    }
}
