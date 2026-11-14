use crate::heightfield::Terrain;
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
}
