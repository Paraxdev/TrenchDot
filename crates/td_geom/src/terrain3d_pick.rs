//! Ray casts against Terrain3D region data, on the triangles the clipmap draws: whole regions, or a strided level of
//! them where only the LOD pyramid is loaded.

use crate::heightfield::ray_cast_cells;
use crate::terrain3d::{Control, EdgeBlend, Grid, LOC_MIN, Loc, Region3D, edge_blended, loc_in_range};
use td_core::{Aabb, DVec3, Ray};

/// What a region is picked against. `Strided` is a LOD level: every `stride`th vertex the region owns, `side * side`
/// heights in meters, and one hole byte per cell laid out `side` wide, which may be empty.
#[derive(Clone, Copy, Debug)]
pub enum Heights<'a> {
    Whole(&'a Region3D),
    Strided { stride: u32, side: u32, heights: &'a [f32], holes: &'a [u8] },
}

impl Heights<'_> {
    fn stride(&self) -> u32 {
        match self {
            Self::Whole(_) => 1,
            Self::Strided { stride, .. } => *stride,
        }
    }

    fn fits(&self, size: u32) -> bool {
        match self {
            Self::Whole(r) => r.size == size && r.check().is_ok(),
            Self::Strided { stride, side, heights, holes } => {
                let n = *side as usize * *side as usize;
                *side > 0 && side.checked_mul(*stride) == Some(size) && heights.len() == n && (holes.is_empty() || holes.len() == n)
            }
        }
    }

    /// Height in meters at a pixel of the region and whether that vertex is a hole. A strided level interpolates
    /// between its samples, which only the seam of a finer neighbor asks for.
    fn sample(&self, pixel: [u32; 2]) -> (f32, bool) {
        match self {
            Self::Whole(r) => {
                let k = r.index(pixel);
                (r.heights[k], Control(r.control[k]).hole())
            }
            Self::Strided { stride, side, heights, .. } => {
                let last = *side as usize - 1;
                let axis = |p: u32| {
                    let f = p as f32 / *stride as f32;
                    let i = (f.floor() as usize).min(last);
                    (i, (i + 1).min(last), f - i as f32)
                };
                let ((x0, x1, tx), (z0, z1, tz)) = (axis(pixel[0]), axis(pixel[1]));
                let at = |i: usize, j: usize| heights[j * *side as usize + i];
                let lerp = |a: f32, b: f32, t: f32| a + t * (b - a);
                (lerp(lerp(at(x0, z0), at(x1, z0), tx), lerp(at(x0, z1), at(x1, z1), tx), tz), false)
            }
        }
    }

    fn cell_hole(&self, cell: [u32; 2]) -> bool {
        match self {
            Self::Whole(_) => false,
            Self::Strided { side, holes, .. } => holes.get(cell[1] as usize * *side as usize + cell[0] as usize).is_some_and(|h| *h != 0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub distance: f64,
    pub point: DVec3,
    /// The normal of the triangle hit, pointing up.
    pub normal: DVec3,
}

const MAP_SIDE: usize = (-LOC_MIN * 2) as usize;

/// The nearest hit of `ray` on the regions at `locs`, in map units. `heights` gives the data of a region, None while it
/// is not loaded. A cell belongs to the region holding its first vertex and takes its far corners from the neighbors,
/// so a cell next to a missing or unloaded region is not hit, the way `surface_height` is NaN there. Neither is a
/// triangle with a hole vertex, and `edge` blends the heights toward the ground level as Terrain3D draws them.
/// `range` gives the lowest and highest height in meters of the data `heights` gives, which bounds the walk over the
/// cells of a region. None leaves that region, and the regions whose last cells it closes, unbounded.
pub fn ray_cast<'a>(
    grid: &Grid,
    edge: Option<EdgeBlend>,
    heights: impl Fn(Loc) -> Option<Heights<'a>>,
    range: impl Fn(Loc) -> Option<[f32; 2]>,
    locs: &[Loc],
    ray: &Ray,
) -> Option<Hit> {
    grid.check().ok()?;
    let slot = |loc: Loc| loc_in_range(loc).then(|| (loc[1] - LOC_MIN) as usize * MAP_SIDE + (loc[0] - LOC_MIN) as usize);
    let mut map = [false; MAP_SIDE * MAP_SIDE];
    for k in locs.iter().filter_map(|l| slot(*l)) {
        map[k] = true;
    }

    let present = |loc: Loc| slot(loc).is_some_and(|k| map[k]);
    let data = |loc: Loc| heights(loc).filter(|h| present(loc) && h.fits(grid.region_size));
    let span = grid.region_size as f64 * grid.cell();
    // The last row and column of a region take their far corners from the neighbors, and the edge blend pulls toward
    // the ground level.
    let heights_of = |loc: Loc| -> Option<[f32; 2]> {
        let mut all = edge.map_or([f32::INFINITY, f32::NEG_INFINITY], |e| [e.ground_level; 2]);
        for d in [[0, 0], [1, 0], [0, 1], [1, 1]] {
            let near = [loc[0].saturating_add(d[0]), loc[1].saturating_add(d[1])];
            if data(near).is_some() {
                let [lo, hi] = range(near)?;
                all = [all[0].min(lo), all[1].max(hi)];
            }
        }

        Some(all)
    };
    let mut order: Vec<([f64; 2], Loc, Heights<'a>)> = locs
        .iter()
        .filter_map(|loc| {
            let own = data(*loc)?;
            let [lo, hi] = heights_of(*loc).map_or([-1e12, 1e12], |[lo, hi]| [lo as f64 * grid.upm, hi as f64 * grid.upm]);
            if lo > hi {
                return None;
            }

            let corner = grid.position(grid.vgrid_of(*loc, [0, 0]));
            // Padded so a ray along an axis exactly on the region edge does not meet the slab test's 0 * inf.
            let rect = Aabb::new(DVec3::new(corner.x, lo, corner.y), DVec3::new(corner.x + span, hi, corner.y + span)).expanded(1.0);
            Some((ray.aabb_span(&rect)?, *loc, own))
        })
        .collect();
    order.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));

    let mut best: Option<Hit> = None;
    for ([t0, t_end], loc, own) in order {
        if best.is_some_and(|b| t0 > b.distance) {
            break;
        }

        let near = [[1, 0], [0, 1], [1, 1]].map(|d| data([loc[0].saturating_add(d[0]), loc[1].saturating_add(d[1])]));
        let stride = own.stride() as i64;
        let n = grid.region_size / own.stride();
        let base = grid.vgrid_of(loc, [0, 0]);
        let vertex = |v: [i64; 2]| -> Option<(DVec3, bool)> {
            let r = grid.region_of(v);
            let h = match [r[0] as i64 - loc[0] as i64, r[1] as i64 - loc[1] as i64] {
                [0, 0] => own,
                [1, 0] => near[0]?,
                [0, 1] => near[1]?,
                [1, 1] => near[2]?,
                _ => return None,
            };
            let (raw, hole) = h.sample(grid.pixel_of(v));
            let y = edge.map_or(raw, |e| edge_blended(grid, e, present, v, raw));
            let p = grid.position(v);
            Some((DVec3::new(p.x, y as f64 * grid.upm, p.y), hole))
        };
        let test = |ci: i64, cj: i64| -> Option<(f64, DVec3)> {
            if ci < 0 || cj < 0 || ci >= n as i64 || cj >= n as i64 || own.cell_hole([ci as u32, cj as u32]) {
                return None;
            }

            let corner = |a: i64, b: i64| vertex([base[0] + (ci + a) * stride, base[1] + (cj + b) * stride]);
            let (p00, p01, p11, p10) = (corner(0, 0)?, corner(0, 1)?, corner(1, 1)?, corner(1, 0)?);
            // The diagonal of `Terrain::cell_triangles` over global cells, so every stride splits cells alike.
            let even = (base[0] / stride + ci + base[1] / stride + cj).rem_euclid(2) == 0;
            let tris = if even { [[p00, p01, p11], [p00, p11, p10]] } else { [[p00, p01, p10], [p01, p11, p10]] };
            let mut hit: Option<(f64, DVec3)> = None;
            for tri in tris.iter().filter(|tri| tri.iter().all(|(_, hole)| !hole)) {
                let [a, b, c] = tri.map(|(p, _)| p);
                if let Some(t) = ray.intersect_triangle(a, b, c)
                    && hit.is_none_or(|(ht, _)| t < ht)
                {
                    let normal = (b - a).cross(c - a).normalize_or(DVec3::Y);
                    hit = Some((t, if normal.y < 0.0 { -normal } else { normal }));
                }
            }

            hit
        };
        let lo = grid.position(base);
        let found = ray_cast_cells(ray, [t0, t_end], lo, grid.cell() * stride as f64, [n, n], test);
        if let Some((t, normal)) = found
            && best.is_none_or(|b| t < b.distance)
        {
            best = Some(Hit { distance: t, point: ray.at(t), normal });
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::heightfield::Terrain;
    use crate::terrain3d::surface_height;

    const SIZE: u32 = 64;
    const UPM: f64 = 32.0;

    fn grid() -> Grid {
        Grid { region_size: SIZE, spacing: 1.5, upm: UPM }
    }

    fn bumpy(v: [i64; 2]) -> f32 {
        let (x, z) = (v[0] as f32, v[1] as f32);
        3.0 + 0.07 * x - 0.11 * z + 0.5 * (x * 0.3).sin() * (z * 0.2).cos()
    }

    fn slope(v: [i64; 2]) -> f32 {
        9.0 + 0.13 * v[0] as f32 - 0.05 * v[1] as f32
    }

    fn world(locs: &[Loc], h: impl Fn([i64; 2]) -> f32) -> BTreeMap<Loc, Region3D> {
        let g = grid();
        let region = |loc: Loc| {
            let mut r = Region3D::blank(loc, SIZE);
            for pz in 0..SIZE {
                for px in 0..SIZE {
                    let k = r.index([px, pz]);
                    r.heights[k] = h(g.vgrid_of(loc, [px, pz]));
                }
            }

            r
        };
        locs.iter().map(|l| (*l, region(*l))).collect()
    }

    fn range_of(heights: &[f32]) -> [f32; 2] {
        heights.iter().fold([f32::INFINITY, f32::NEG_INFINITY], |[lo, hi], h| [lo.min(*h), hi.max(*h)])
    }

    fn cast(world: &BTreeMap<Loc, Region3D>, edge: Option<EdgeBlend>, ray: &Ray) -> Option<Hit> {
        let locs: Vec<Loc> = world.keys().copied().collect();
        ray_cast(&grid(), edge, |l| world.get(&l).map(Heights::Whole), |l| world.get(&l).map(|r| range_of(&r.heights)), &locs, ray)
    }

    /// The vertices of `count` by `count` regions from `min` as one terrain in map units.
    fn terrain_of(min: Loc, count: u32, h: impl Fn([i64; 2]) -> f32) -> Terrain {
        let g = grid();
        let base = g.vgrid_of(min, [0, 0]);
        let n = SIZE * count;
        let lo = g.position(base);
        let mut t = Terrain::new(DVec3::new(lo.x, 0.0, lo.y), [n, n], g.cell(), "");
        t.heights = (0..n).flat_map(|j| (0..n).map(move |i| [base[0] + i as i64, base[1] + j as i64])).map(|v| (h(v) as f64 * UPM) as f32).collect();
        t
    }

    fn triangle_normal(t: &Terrain, p: DVec3) -> DVec3 {
        let local = (p - t.origin) / t.cell_size;
        let cross = |o: DVec3, a: DVec3| (a.x - o.x) * (p.z - o.z) - (a.z - o.z) * (p.x - o.x);
        for tri in Terrain::cell_triangles(local.x.floor() as u32, local.z.floor() as u32) {
            let [a, b, c] = tri.map(|(i, j)| t.vertex(i, j));
            let d = [cross(a, b), cross(b, c), cross(c, a)];
            if d.iter().all(|v| *v >= -1e-6) || d.iter().all(|v| *v <= 1e-6) {
                let n = (b - a).cross(c - a).normalize();
                return if n.y < 0.0 { -n } else { n };
            }
        }

        panic!("no triangle under {p}")
    }

    fn assert_hit(got: Option<Hit>, want: Option<(f64, DVec3)>, normal: DVec3, what: &str) {
        let (got, (t, p)) = (got.unwrap_or_else(|| panic!("{what}: no hit")), want.unwrap_or_else(|| panic!("{what}: the terrain has no hit")));
        assert!((got.distance - t).abs() < 1e-6, "{what}: distance {} against {t}", got.distance);
        assert!(got.point.distance(p) < 1e-6, "{what}: point {} against {p}", got.point);
        assert!(got.normal.distance(normal) < 1e-9, "{what}: normal {} against {normal}", got.normal);
    }

    const MIN: Loc = [2, -3];

    fn block() -> [Loc; 4] {
        [MIN, [MIN[0] + 1, MIN[1]], [MIN[0], MIN[1] + 1], [MIN[0] + 1, MIN[1] + 1]]
    }

    fn corner() -> DVec3 {
        let g = grid();
        let lo = g.position(g.vgrid_of(MIN, [0, 0]));
        DVec3::new(lo.x, 0.0, lo.y)
    }

    #[test]
    fn a_hit_is_the_terrain_hit_on_the_same_heights() {
        let w = world(&block(), bumpy);
        let t = terrain_of(MIN, 2, bumpy);
        let c = corner();
        let rays = [
            Ray::new(c + DVec3::new(700.3, 2000.0, 1300.7), DVec3::new(0.31, -0.8, 0.17)),
            Ray::new(c + DVec3::new(5000.1, 1500.0, 300.2), DVec3::new(-0.6, -0.35, 0.72)),
            Ray::new(c + DVec3::new(3071.3, 2900.0, 2999.1), DVec3::new(0.05, -1.0, -0.02)),
            Ray::new(c + DVec3::new(1234.5, 5000.0, 4321.0), DVec3::NEG_Y),
            Ray::new(c + DVec3::new(-800.4, 1800.0, 2500.6), DVec3::new(1.0, -0.12, 0.1)),
        ];
        for (k, ray) in rays.iter().enumerate() {
            let want = t.ray_cast(ray);
            let normal = triangle_normal(&t, want.expect("terrain hit").1);
            assert_hit(cast(&w, None, ray), want, normal, &format!("ray {k}"));
        }
    }

    #[test]
    fn a_slanted_ray_crosses_a_region_border_off_the_origin() {
        let g = grid();
        let w = world(&block(), bumpy);
        let t = terrain_of(MIN, 2, bumpy);
        let c = corner();
        let east = [MIN[0] + 1, MIN[1]];
        let across = Ray::new(c + DVec3::new(2000.7, 2600.0, 900.3), DVec3::new(0.7, -0.45, 0.08));
        let hit = cast(&w, None, &across).expect("hit across the border");
        assert_eq!(g.region_of(g.vgrid_at(hit.point.x, hit.point.z)), east, "the ray lands past the border");
        assert_hit(Some(hit), t.ray_cast(&across), triangle_normal(&t, hit.point), "across");

        // The last cell of the west region takes its far corners from the east one.
        let seam_x = c.x + (SIZE as f64 - 0.4) * g.cell();
        let seam = Ray::new(DVec3::new(seam_x, 3000.0, c.z + 1000.9), DVec3::new(0.002, -1.0, 0.001));
        let want = t.ray_cast(&seam);
        let hit = cast(&w, None, &seam);
        assert_hit(hit, want, triangle_normal(&t, want.unwrap().1), "seam cell");
        assert_eq!(g.region_of(g.vgrid_at(hit.unwrap().point.x, hit.unwrap().point.z)), MIN);

        let alone = world(&[MIN], bumpy);
        let below = cast(&alone, None, &seam);
        assert!(below.is_none(), "without the east region the seam cell has no far corners: {below:?}");
    }

    #[test]
    fn triangles_with_a_hole_vertex_are_missed_and_the_rest_of_the_cell_is_hit() {
        let g = grid();
        let loc = [-2, 1];
        let mut w = world(&[loc], slope);
        let hole = [10u32, 7u32];
        let r = w.get_mut(&loc).unwrap();
        let k = r.index(hole);
        r.control[k] = r.control_at(hole).with_hole(true).0;

        let base = g.vgrid_of(loc, [0, 0]);
        let (mut hit, mut missed) = (0, 0);
        for (ci, cj) in [(9, 6), (10, 6), (9, 7), (10, 7)] {
            for tri in Terrain::cell_triangles(ci, cj) {
                let at = |(i, j): (u32, u32)| g.position([base[0] + i as i64, base[1] + j as i64]);
                let centroid = tri.iter().map(|v| at(*v)).sum::<td_core::DVec2>() / 3.0;
                let got = cast(&w, None, &Ray::new(DVec3::new(centroid.x, 5000.0, centroid.y), DVec3::NEG_Y));
                if tri.contains(&(hole[0], hole[1])) {
                    assert!(got.is_none(), "triangle {tri:?} touches the hole: {got:?}");
                    missed += 1;
                } else {
                    let v = DVec3::new(centroid.x, 0.0, centroid.y);
                    let want = tri.iter().map(|&(i, j)| slope([base[0] + i as i64, base[1] + j as i64]) as f64 * UPM).sum::<f64>() / 3.0;
                    let got = got.unwrap_or_else(|| panic!("triangle {tri:?} is away from the hole"));
                    assert!((got.point.y - want).abs() < 1e-6 && td_core::DVec2::new(got.point.x - v.x, got.point.z - v.z).length() < 1e-9, "{tri:?}: {got:?}");
                    hit += 1;
                }
            }
        }

        assert_eq!((hit, missed), (4, 4), "the diagonals leave four of eight triangles clear of the hole vertex");
    }

    fn strided(r: &Region3D, stride: u32) -> Vec<f32> {
        let side = SIZE / stride;
        (0..side).flat_map(|j| (0..side).map(move |i| r.heights[r.index([i * stride, j * stride])])).collect()
    }

    fn cast_strided(w: &BTreeMap<Loc, Region3D>, levels: &BTreeMap<Loc, Vec<f32>>, holes: &[u8], ray: &Ray) -> Option<Hit> {
        let locs: Vec<Loc> = w.keys().copied().collect();
        let heights = |l: Loc| match levels.get(&l) {
            Some(h) => Some(Heights::Strided { stride: 4, side: SIZE / 4, heights: h, holes }),
            None => w.get(&l).map(Heights::Whole),
        };
        let range = |l: Loc| Some(range_of(levels.get(&l).or(w.get(&l).map(|r| &r.heights))?));
        ray_cast(&grid(), None, heights, range, &locs, ray)
    }

    #[test]
    fn strided_heights_hit_within_the_stride_error() {
        let locs = [[-1, 4], [0, 4]];
        let c = {
            let g = grid();
            let lo = g.position(g.vgrid_of(locs[0], [0, 0]));
            DVec3::new(lo.x, 0.0, lo.y)
        };
        let rays = [
            Ray::new(c + DVec3::new(900.2, 3000.0, 700.9), DVec3::new(0.4, -0.85, 0.2)),
            Ray::new(c + DVec3::new(4100.6, 2500.0, 2600.1), DVec3::new(-0.3, -0.7, -0.5)),
            Ray::new(c + DVec3::new(3020.4, 2000.0, 1500.8), DVec3::new(0.15, -1.0, 0.05)),
            Ray::new(c + DVec3::new(3050.3, 3000.0, 1500.8), DVec3::NEG_Y),
        ];

        // A plane is exact at any stride up to the f32 rounding of its heights, also where a whole region meets a
        // strided one at the seam.
        let plane = world(&locs, slope);
        let all: BTreeMap<Loc, Vec<f32>> = plane.iter().map(|(l, r)| (*l, strided(r, 4))).collect();
        let east: BTreeMap<Loc, Vec<f32>> = all.iter().filter(|(l, _)| **l == locs[1]).map(|(l, h)| (*l, h.clone())).collect();
        for (k, ray) in rays.iter().enumerate() {
            let whole = cast(&plane, None, ray).expect("whole hit");
            for (what, levels) in [("strided", &all), ("whole beside strided", &east)] {
                let got = cast_strided(&plane, levels, &[], ray).unwrap_or_else(|| panic!("ray {k} {what}: no hit"));
                assert!((got.distance - whole.distance).abs() < 1e-3 && got.normal.distance(whole.normal) < 1e-5, "ray {k} {what}: {got:?} {whole:?}");
            }
        }

        // The bumps reach 0.5 m either way, so the strided and the whole surface stay within 1 m of each other.
        let bumps = world(&locs, bumpy);
        let levels: BTreeMap<Loc, Vec<f32>> = bumps.iter().map(|(l, r)| (*l, strided(r, 4))).collect();
        let t = terrain_of(locs[0], 2, bumpy);
        let mut worst = 0.0f64;
        let down = (0..11).flat_map(|i| (0..7).map(move |j| Ray::new(c + DVec3::new(100.3 + i as f64 * 517.1, 4000.0, 90.7 + j as f64 * 431.9), DVec3::NEG_Y)));
        for (k, ray) in rays.iter().copied().chain(down).enumerate() {
            let got = cast_strided(&bumps, &levels, &[], &ray).unwrap_or_else(|| panic!("ray {k}: no strided hit"));
            let under = t.height_at(got.point.x, got.point.z).expect("on the terrain");
            worst = worst.max((got.point.y - under).abs());
            assert!((got.point.y - under).abs() <= UPM + 1e-6, "ray {k}: {} against {under}", got.point.y);
        }

        assert!(worst > 1.0, "the bumps show at stride 4, worst {worst}");

        let mut holes = vec![0u8; (SIZE / 4 * SIZE / 4) as usize];
        let center = |ci: f64, cj: f64| Ray::new(c + DVec3::new((ci + 0.3) * 4.0 * grid().cell(), 4000.0, (cj + 0.6) * 4.0 * grid().cell()), DVec3::NEG_Y);
        holes[5 * 16 + 3] = 1;
        assert!(cast_strided(&bumps, &levels, &holes, &center(3.0, 5.0)).is_none(), "a hole byte drops its cell");
        assert!(cast_strided(&bumps, &levels, &holes, &center(4.0, 5.0)).is_some(), "the next cell stays");
    }

    #[test]
    fn the_edge_blend_pulls_open_edges_to_the_ground_level_only_when_given() {
        let g = grid();
        let locs = [[5, -1], [6, -1]];
        let w = world(&locs, |_| 40.0);
        let edge = EdgeBlend { ground_level: -3.0, region_blend: 0.25 };
        let height = |edge: Option<EdgeBlend>, loc: Loc, pixel: [u32; 2]| {
            let p = g.position(g.vgrid_of(loc, pixel));
            cast(&w, edge, &Ray::new(DVec3::new(p.x, 5000.0, p.y), DVec3::NEG_Y)).expect("vertex hit").point.y
        };
        let godot = |loc: Loc, pixel: [u32; 2]| {
            let p = g.position(g.vgrid_of(loc, pixel)) / UPM;
            surface_height(&g, Some(edge), |l| w.get(&l), p.x as f32, p.y as f32) as f64 * UPM
        };

        let open = (locs[0], [0, 20]);
        let inner = (locs[0], [32, 30]);
        let border = (locs[1], [0, 30]);
        for (what, (loc, pixel)) in [("open edge", open), ("inside", inner), ("shared border", border)] {
            assert!((height(None, loc, pixel) - 40.0 * UPM).abs() < 1e-6, "{what} without a blend");
            assert!((height(Some(edge), loc, pixel) - godot(loc, pixel)).abs() < 1e-3, "{what} matches surface_height");
        }

        assert!(height(Some(edge), open.0, open.1) < 10.0 * UPM, "the open edge sinks toward -3 m");
        assert!((height(Some(edge), inner.0, inner.1) - 40.0 * UPM).abs() < 1e-3, "the middle keeps its height");
        assert!((height(Some(edge), border.0, border.1) - 40.0 * UPM).abs() < 1e-3, "a border between two regions keeps its height");
    }

    #[test]
    fn the_height_range_bounds_the_walk_and_counts_the_neighbors_that_close_the_last_cells() {
        let g = grid();
        let blocks = block();
        let east = |v: [i64; 2]| g.region_of(v)[0] > MIN[0];
        let steps = |v: [i64; 2]| (if east(v) { 50.0 } else { 10.0 }) + 0.01 * v[1] as f32;
        let w = world(&blocks, steps);
        let t = terrain_of(MIN, 2, steps);
        let exact = |l: Loc| w.get(&l).map(|r| range_of(&r.heights));
        let cast_with = |range: &dyn Fn(Loc) -> Option<[f32; 2]>, ray: &Ray| ray_cast(&g, None, |l| w.get(&l).map(Heights::Whole), range, &blocks, ray);

        // Low over the west region, the ray first meets ground on the last cells of it, which rise to the east region.
        let c = corner();
        let low = Ray::new(c + DVec3::new(100.3, 45.0 * UPM, 700.7), DVec3::new(1.0, -0.01, 0.07));
        let want = t.ray_cast(&low);
        let hit = cast_with(&exact, &low);
        assert_hit(hit, want, triangle_normal(&t, want.expect("terrain hit").1), "the east neighbor's heights widen the west box");
        assert_eq!(g.region_of(g.vgrid_at(hit.unwrap().point.x, hit.unwrap().point.z)), MIN, "the hit is on a seam cell of the west region");
        assert_eq!(cast_with(&|_| None, &low), hit, "no range is unbounded");

        let above = |_: Loc| Some([60.0, 61.0]);
        assert!(cast_with(&above, &low).is_none(), "a range above the data rules the ground out");
        let down = Ray::new(c + DVec3::new(1500.3, 4000.0, 1200.7), DVec3::new(0.1, -1.0, 0.05));
        assert!(cast_with(&exact, &down).is_some() && cast_with(&above, &down).is_none(), "a steep ray alike");
        let one_unbounded = |l: Loc| if l == MIN { None } else { above(l) };
        assert_eq!(cast_with(&one_unbounded, &down), cast_with(&exact, &down), "only the region without a range is walked whole");
    }

    #[test]
    fn rays_miss_where_there_is_no_usable_region() {
        let g = grid();
        let w = world(&[MIN], bumpy);
        let c = corner();
        let down = Ray::new(c + DVec3::new(1500.3, 4000.0, 1200.7), DVec3::new(0.1, -1.0, 0.05));
        assert!(cast(&w, None, &down).is_some(), "the ray hits the region");

        let up = Ray::new(c + DVec3::new(1500.3, 4000.0, 1200.7), DVec3::new(0.1, 1.0, 0.05));
        assert!(cast(&w, None, &up).is_none(), "pointing away");
        let beside = Ray::new(c + DVec3::new(-2000.0, 4000.0, 1200.7), DVec3::new(-0.1, -1.0, 0.05));
        assert!(cast(&w, None, &beside).is_none(), "over a missing region");

        let r = &w[&MIN];
        let none: [Loc; 0] = [];
        assert!(ray_cast(&g, None, |_| Some(Heights::Whole(r)), |_| None, &none, &down).is_none(), "no regions");
        assert!(ray_cast(&g, None, |_| None, |_| None, &[MIN], &down).is_none(), "a region not loaded yet");
        assert!(
            ray_cast(&g, None, |l| (l == MIN).then_some(Heights::Whole(r)), |_| None, &[[MIN[0] + 1, MIN[1]]], &down).is_none(),
            "data for a region the map lacks"
        );
        let other = Grid { region_size: 128, ..g };
        assert!(ray_cast(&other, None, |_| Some(Heights::Whole(r)), |_| None, &[MIN], &down).is_none(), "a region of another size");
        let bad = Grid { spacing: 0.0, ..g };
        assert!(ray_cast(&bad, None, |_| Some(Heights::Whole(r)), |_| None, &[MIN], &down).is_none(), "a grid Terrain3D cannot hold");
        let short = [1.0f32; 15];
        let level = Heights::Strided { stride: 4, side: 4, heights: &short, holes: &[] };
        assert!(ray_cast(&g, None, |_| Some(level), |_| None, &[MIN], &down).is_none(), "a level that does not cover the region");
    }
}
