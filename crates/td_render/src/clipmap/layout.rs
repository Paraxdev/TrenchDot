//! Where the clipmap meshes go, after Terrain3D's mesher. Ten square rings surround the camera, each with cells twice
//! as wide as the ring inside it. A ring snaps to a grid of two of its own cells, so a camera moving less than that
//! leaves it in place and the ground does not swim. Positions are in level 0 cells, the Terrain3D vertex grid, with a
//! cell's x along world x and its y along world z.

use std::ops::Range;

use glam::{DVec2, DVec3, DVec4, I64Vec2, Mat4};

/// Cells along a tile side, Terrain3D's default mesh size.
pub const MESH_SIZE: i64 = 48;

/// Rings around the camera. The outermost reaches about 50,000 cells, past any Terrain3D region grid.
pub const LEVELS: u32 = 10;

/// Room in a view's instance buffer. The full ring has 24 parts at level 0 and 20 at every level after it.
pub const MAX_INSTANCES: usize = 256;

/// Furthest a target may be from the origin in cells, keeps every [`ClipmapInstance`] origin inside an i32.
const TARGET_LIMIT: f64 = (1u64 << 30) as f64;

const M: i64 = MESH_SIZE;
const A: i64 = 2 * M + 2;
const B: i64 = 2 * M + 4;
const C: i64 = 2 * M + 6;

/// Tiles of every level after 0, a 4 by 4 ring with a two cell gap on each axis that the fills close.
const TILES: [[i64; 2]; 12] = [
    [2, M + 2],
    [M + 2, M + 2],
    [M + 2, -2],
    [M + 2, -M - 2],
    [M + 2, -2 * M - 2],
    [-2, -2 * M - 2],
    [-M - 2, -2 * M - 2],
    [-2 * M - 2, -2 * M - 2],
    [-2 * M - 2, -M + 2],
    [-2 * M - 2, 2],
    [-2 * M - 2, M + 2],
    [-M + 2, M + 2],
];
const FILLS_A: [[i64; 2]; 2] = [[M - 2, -2 * M - 2], [-M - 2, M + 2]];
const FILLS_B: [[i64; 2]; 2] = [[M + 2, M - 2], [-2 * M - 2, -M - 2]];
const TRIMS_A: [[i64; 2]; 2] = [[2 * M, -2 * M], [-2 * M - 2, -2 * M - 2]];
const TRIMS_B: [[i64; 2]; 2] = [[-2 * M, -2 * M - 2], [-2 * M - 2, 2 * M]];

/// Edge pair positions across a ring, picked per axis by where the level sits in the next one: both after the ring,
/// one on each side, or both before it.
const EDGES: [[i64; 3]; 2] = [[A, A, -B], [B, -B, -C]];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MeshKind {
    Tile,
    /// Two cells wide along x, closes a ring onto the grid of the next level.
    EdgeA,
    /// Two cells wide along z, closes a ring onto the grid of the next level.
    EdgeB,
    /// Four cells wide along x, closes the gap between the tiles of a row.
    FillA,
    /// Four cells wide along z, closes the gap between the tiles of a column.
    FillB,
    /// Two cells wide along x, part of level 0's border around its 4 by 4 tiles.
    TrimA,
    /// Two cells wide along z, part of level 0's border around its 4 by 4 tiles.
    TrimB,
}

impl MeshKind {
    pub const COUNT: usize = 7;
    pub const ALL: [MeshKind; Self::COUNT] =
        [MeshKind::Tile, MeshKind::EdgeA, MeshKind::EdgeB, MeshKind::FillA, MeshKind::FillB, MeshKind::TrimA, MeshKind::TrimB];

    /// Cells along x and z.
    pub const fn size(self) -> [i64; 2] {
        match self {
            MeshKind::Tile => [M, M],
            MeshKind::EdgeA => [2, 4 * M + 8],
            MeshKind::EdgeB => [4 * M + 4, 2],
            MeshKind::FillA => [4, M],
            MeshKind::FillB => [M, 4],
            MeshKind::TrimA => [2, 4 * M + 2],
            MeshKind::TrimB => [4 * M + 2, 2],
        }
    }

    /// Grid vertices row by row, x first, in cells.
    pub fn vertices(self) -> Vec<[u32; 2]> {
        let [w, d] = self.size().map(|n| n as u32);
        (0..=d).flat_map(|z| (0..=w).map(move |x| [x, z])).collect()
    }

    /// Two triangles per cell over [`Self::vertices`], split along the alternating diagonal and wound like td_geom's
    /// `Terrain::cell_triangles`. Every part's origin is an even number of its level's cells, so a cell's parity here
    /// is its parity on the level's grid, and at level 0 the drawn triangles are the ones picking hits.
    pub fn indices(self) -> Vec<u32> {
        let [w, d] = self.size().map(|n| n as u32);
        let at = |x: u32, z: u32| z * (w + 1) + x;
        let mut out = Vec::with_capacity((w * d * 6) as usize);
        for z in 0..d {
            for x in 0..w {
                let [p00, p01, p11, p10] = [at(x, z), at(x, z + 1), at(x + 1, z + 1), at(x + 1, z)];
                if (x + z).is_multiple_of(2) {
                    out.extend([p00, p01, p11, p00, p11, p10]);
                } else {
                    out.extend([p00, p01, p10, p01, p11, p10]);
                }
            }
        }

        out
    }
}

/// One mesh placed in the ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub kind: MeshKind,
    pub level: u32,
    /// Corner with the lowest x and z, in level 0 cells.
    pub origin: I64Vec2,
}

impl Part {
    /// Lowest and highest corner in level 0 cells. Morphing keeps every vertex inside them.
    pub fn cells(&self) -> [I64Vec2; 2] {
        let [w, d] = self.kind.size();
        [self.origin, self.origin + I64Vec2::new(w, d) * (1 << self.level)]
    }
}

/// A part as the vertex shader reads it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ClipmapInstance {
    pub origin: [i32; 2],
    pub level: u32,
    pub pad: u32,
}

impl From<Part> for ClipmapInstance {
    fn from(part: Part) -> Self {
        ClipmapInstance { origin: [part.origin.x as i32, part.origin.y as i32], level: part.level, pad: 0 }
    }
}

/// The level 0 vertex at or before a target in cells, Terrain3D's snapped camera position.
pub fn snap(target: DVec2) -> I64Vec2 {
    let t = target.clamp(DVec2::splat(-TARGET_LIMIT), DVec2::splat(TARGET_LIMIT)).floor();
    I64Vec2::new(t.x as i64, t.y as i64)
}

/// Origin of a level's ring, the snapped target rounded to a multiple of `2^(level + 1)` cells.
pub fn level_origin(snapped: I64Vec2, level: u32) -> I64Vec2 {
    let step = 2i64 << level;
    (snapped + step / 2).div_euclid(I64Vec2::splat(step)) * step
}

/// Every part of the rings around a target in cells, nothing culled.
pub fn ring(target: DVec2) -> Vec<Part> {
    let snapped = snap(target);
    let mut parts = Vec::with_capacity(24 + 20 * (LEVELS as usize - 1));
    for level in 0..LEVELS {
        let origin = level_origin(snapped, level);
        let scale = 1i64 << level;
        let test = ((origin - level_origin(snapped, level + 1)) / (2 * scale) + 1).as_usizevec2();
        let mut put = |kind, [x, z]: [i64; 2]| parts.push(Part { kind, level, origin: origin + I64Vec2::new(x, z) * scale });
        if level == 0 {
            for z in -2..2 {
                for x in -2..2 {
                    put(MeshKind::Tile, [x * M, z * M]);
                }
            }
        } else {
            TILES.into_iter().for_each(|at| put(MeshKind::Tile, at));
        }

        for edge in EDGES {
            put(MeshKind::EdgeA, [edge[test.x], -A - 2 * test.y as i64]);
            put(MeshKind::EdgeB, [-A, edge[test.y]]);
        }

        let [(a, a_at), (b, b_at)] =
            if level == 0 { [(MeshKind::TrimA, TRIMS_A), (MeshKind::TrimB, TRIMS_B)] } else { [(MeshKind::FillA, FILLS_A), (MeshKind::FillB, FILLS_B)] };
        a_at.into_iter().for_each(|at| put(a, at));
        b_at.into_iter().for_each(|at| put(b, at));
    }

    parts
}

/// A ring vertex sliding onto the next level's grid near the outer edge of its ring, the CPU twin of the vertex
/// shader. At the outer edge every vertex sits on a vertex of the next level, so the levels meet without cracks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Morph {
    /// The vertex, in level 0 cells.
    pub start: I64Vec2,
    /// The neighbour it slides onto, a vertex of the next level.
    pub end: I64Vec2,
    /// 0 keeps the vertex at `start`, 1 puts it on `end`.
    pub lerp: f64,
}

impl Morph {
    pub fn new(vertex: I64Vec2, level: u32, target: DVec2) -> Self {
        let scale = 1i64 << level;
        let away = (vertex.as_dvec2() - target).abs().max_element() / scale as f64;
        let m = MESH_SIZE as f64;
        let t = ((away - m - 4.0) / (m - 4.0)).clamp(0.0, 1.0);
        // Odd vertices slide onto the neighbour two past a multiple of four, the corner the next level's alternating
        // diagonal runs through, so the collapsed cells split the way the next level splits them.
        let shift = (vertex / scale).rem_euclid(I64Vec2::splat(4)).map(|r| match r {
            1 => 1,
            3 => -1,
            _ => 0,
        });
        Morph { start: vertex, end: vertex + shift * scale, lerp: t * t * (3.0 - 2.0 * t) }
    }

    pub fn position(&self) -> DVec2 {
        self.start.as_dvec2().lerp(self.end.as_dvec2(), self.lerp)
    }
}

/// The six planes of a view projection in wgpu clip space, each with the visible side positive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum([DVec4; 6]);

impl Frustum {
    /// An infinite far plane comes out with a zero normal and keeps everything.
    pub fn new(view_proj: Mat4) -> Self {
        let m = view_proj.as_dmat4();
        let [x, y, z, w] = [0, 1, 2, 3].map(|i| m.row(i));
        Frustum([w + x, w - x, w + y, w - y, z, w - z])
    }

    /// Whether a box may be on screen, a box touching a plane counts.
    pub fn intersects(&self, min: DVec3, max: DVec3) -> bool {
        self.0.iter().all(|p| {
            let n = p.truncate();
            n.dot(DVec3::select(n.cmpge(DVec3::ZERO), max, min)) + p.w >= 0.0
        })
    }
}

/// What a view draws of the ring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cull {
    /// The view, None keeps parts off screen.
    pub frustum: Option<Frustum>,
    /// Map units between vertices.
    pub cell: f64,
    /// Lowest and highest world y the ground reaches, background and noise included.
    pub heights: [f64; 2],
    /// Lowest and highest corner of the regions in cells, set when nothing draws beyond them.
    pub area: Option<[I64Vec2; 2]>,
}

impl Cull {
    /// A part sharing no more than an edge with the area is dropped, one touching the frustum is kept.
    pub fn keeps(&self, part: &Part) -> bool {
        let [min, max] = part.cells();
        if let Some([lo, hi]) = self.area
            && !min.max(lo).cmplt(max.min(hi)).all()
        {
            return false;
        }

        let world = |c: I64Vec2, y: f64| DVec3::new(c.x as f64 * self.cell, y, c.y as f64 * self.cell);
        self.frustum.is_none_or(|f| f.intersects(world(min, self.heights[0]), world(max, self.heights[1])))
    }
}

/// The parts a view draws, grouped by kind so each kind is one instanced draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Instances {
    pub parts: Vec<Part>,
    /// Range of `parts` per kind, in [`MeshKind::ALL`] order.
    pub ranges: [Range<u32>; MeshKind::COUNT],
}

impl Instances {
    pub fn new(target: DVec2, cull: &Cull) -> Self {
        let mut parts: Vec<Part> = ring(target).into_iter().filter(|p| cull.keeps(p)).collect();
        parts.sort_by_key(|p| p.kind);
        let mut ranges: [Range<u32>; MeshKind::COUNT] = Default::default();
        let mut start = 0;
        for (range, kind) in ranges.iter_mut().zip(MeshKind::ALL) {
            let end = start + parts[start..].iter().take_while(|p| p.kind == kind).count();
            *range = start as u32..end as u32;
            start = end;
        }

        Instances { parts, ranges }
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use glam::camera::rh::proj::directx::{orthographic, perspective_infinite_reverse};
    use glam::camera::rh::view::look_to_mat4;

    use super::*;

    /// Off the origin, asymmetric, and on rounding ties: -2 at level 1, -64 at level 6, 1536 at level 9.
    const TARGETS: [[f64; 2]; 5] = [[-12345.75, 6789.25], [-1.5, -333.9], [-64.0, 1536.25], [40961.3, -7.8], [-524288.5, 99999.99]];

    const OPEN: Cull = Cull { frustum: None, cell: 32.0, heights: [-100.0, 300.0], area: None };

    /// Lowest and highest corner of a level and everything inside it, centred on the next level's origin.
    fn outer(snapped: I64Vec2, level: u32) -> [I64Vec2; 2] {
        let o = level_origin(snapped, level + 1);
        [o - (B << level), o + (B << level)]
    }

    /// Looks straight down from (3000, 5000, -7000) and sees x 1976 to 4024, y 804 to 4900 and z -7512 to -6488.
    fn top_down() -> Frustum {
        let view = look_to_mat4(Vec3::new(3000.0, 5000.0, -7000.0), Vec3::NEG_Y, Vec3::NEG_Z);
        Frustum::new(orthographic(-1024.0, 1024.0, -512.0, 512.0, 100.0, 4196.0) * view)
    }

    #[test]
    fn level_origins_are_multiples_of_their_step() {
        assert_eq!(level_origin(I64Vec2::new(-64, 1536), 6), I64Vec2::new(0, 1536));
        assert_eq!(level_origin(I64Vec2::new(-65, 1535), 6), I64Vec2::new(-128, 1536));
        assert_eq!(level_origin(I64Vec2::new(-64, 1536), 9), I64Vec2::new(0, 2048));
        for t in TARGETS {
            let target = DVec2::from(t);
            let snapped = snap(target);
            assert_eq!(snapped.as_dvec2(), target.floor());
            for level in 0..=LEVELS {
                let step = 2i64 << level;
                let origin = level_origin(snapped, level);
                assert_eq!(origin.rem_euclid(I64Vec2::splat(step)), I64Vec2::ZERO, "{t:?} level {level}");
                let off = snapped - origin;
                assert!(off.cmpge(I64Vec2::splat(-step / 2)).all() && off.cmplt(I64Vec2::splat(step / 2)).all(), "{t:?} level {level} {off}");
            }

            for part in ring(target) {
                assert_eq!(part.origin.rem_euclid(I64Vec2::splat(2 << part.level)), I64Vec2::ZERO, "{t:?} {part:?}");
            }
        }
    }

    #[test]
    fn ring_covers_every_cell_once() {
        for t in TARGETS {
            let parts = ring(DVec2::from(t));
            let snapped = snap(DVec2::from(t));
            for level in 0..LEVELS {
                let scale = 1i64 << level;
                let [lo, hi] = outer(snapped, level);
                let inner = (level > 0).then(|| outer(snapped, level - 1));
                let side = ((hi.x - lo.x) / scale) as usize;
                let mut hits = vec![0u8; side * side];
                for part in parts.iter().filter(|p| p.level == level) {
                    let [w, d] = part.kind.size();
                    for z in 0..d {
                        for x in 0..w {
                            let cell = part.origin + I64Vec2::new(x, z) * scale;
                            assert!(cell.cmpge(lo).all() && cell.cmplt(hi).all(), "{t:?} {part:?} leaves level {level}");
                            let i = (cell - lo) / scale;
                            hits[i.y as usize * side + i.x as usize] += 1;
                        }
                    }
                }

                for (i, &n) in hits.iter().enumerate() {
                    let cell = lo + I64Vec2::new((i % side) as i64, (i / side) as i64) * scale;
                    let hole = inner.is_some_and(|[a, b]| cell.cmpge(a).all() && cell.cmplt(b).all());
                    assert_eq!(n, u8::from(!hole), "{t:?} level {level} cell {cell}");
                }
            }
        }
    }

    #[test]
    fn outer_edge_lands_on_the_next_level_at_full_morph() {
        let on = |v: I64Vec2, [a, b]: [I64Vec2; 2]| (v.x == a.x || v.x == b.x || v.y == a.y || v.y == b.y) && v.cmpge(a).all() && v.cmple(b).all();
        for t in TARGETS {
            let target = DVec2::from(t);
            let snapped = snap(target);
            let (mut outer_seen, mut slid, mut inner_seen) = (0, 0, 0);
            for part in ring(target) {
                let scale = 1i64 << part.level;
                let edge = outer(snapped, part.level);
                let inner = (part.level > 0).then(|| outer(snapped, part.level - 1));
                for [x, z] in part.kind.vertices() {
                    let v = part.origin + I64Vec2::new(x as i64, z as i64) * scale;
                    let morph = Morph::new(v, part.level, target);
                    if on(v, edge) {
                        assert_eq!(morph.lerp, 1.0, "{t:?} {part:?} {v}");
                        assert_eq!(morph.end.rem_euclid(I64Vec2::splat(2 * scale)), I64Vec2::ZERO, "{t:?} {part:?} {v}");
                        assert!(on(morph.end, edge), "{t:?} {part:?} {v} leaves the edge");
                        assert_eq!(morph.position(), morph.end.as_dvec2());
                        outer_seen += 1;
                        slid += usize::from(morph.end != v);
                    }

                    if let Some(inner) = inner
                        && on(v, inner)
                    {
                        assert_eq!(morph.lerp, 0.0, "{t:?} {part:?} {v}");
                        inner_seen += 1;
                    }
                }
            }

            assert!(outer_seen > 0 && slid > 0 && inner_seen > 0, "{t:?} {outer_seen} {slid} {inner_seen}");
        }
    }

    #[test]
    fn morph_never_folds_a_triangle() {
        let meshes = MeshKind::ALL.map(|k| (k.vertices(), k.indices()));
        for t in [[-12345.75, 6789.25], [-1.5, -333.9], [40961.3, -7.8]] {
            let target = DVec2::from(t);
            let mut partial = 0;
            for part in ring(target) {
                let scale = 1i64 << part.level;
                let (vertices, indices) = &meshes[part.kind as usize];
                let moved: Vec<DVec2> = vertices
                    .iter()
                    .map(|&[x, z]| {
                        let morph = Morph::new(part.origin + I64Vec2::new(x as i64, z as i64) * scale, part.level, target);
                        partial += usize::from(morph.lerp > 0.0 && morph.lerp < 1.0 && morph.end != morph.start);
                        morph.position()
                    })
                    .collect();
                for tri in indices.chunks(3) {
                    let [a, b, c] = [0, 1, 2].map(|i| moved[tri[i] as usize]);
                    assert!((b - a).perp_dot(c - a) <= 1e-9 * (scale * scale) as f64, "{t:?} {part:?} folds {a} {b} {c}");
                }
            }

            assert!(partial > 0, "{t:?}");
        }
    }

    #[test]
    fn meshes_split_cells_like_terrain_cells() {
        for kind in MeshKind::ALL {
            let [w, d] = kind.size();
            let vertices = kind.vertices();
            let indices = kind.indices();
            assert_eq!((vertices.len() as i64, indices.len() as i64), ((w + 1) * (d + 1), w * d * 6), "{kind:?}");
            for (cell, tris) in indices.chunks(6).enumerate() {
                let [x, z] = [cell as u32 % w as u32, cell as u32 / w as u32];
                let got: Vec<[u32; 2]> = tris.iter().map(|&i| vertices[i as usize]).collect();
                let [p00, p01, p11, p10] = [[x, z], [x, z + 1], [x + 1, z + 1], [x + 1, z]];
                let want = if (x + z).is_multiple_of(2) { [p00, p01, p11, p00, p11, p10] } else { [p00, p01, p10, p01, p11, p10] };
                assert_eq!(got, want, "{kind:?} cell {x} {z}");
            }
        }
    }

    #[test]
    fn frustum_keeps_a_box_touching_each_plane() {
        let frustum = top_down();
        let base = (DVec3::new(2500.0, 2000.0, -7100.0), DVec3::new(2600.0, 2100.0, -7000.0));
        assert!(frustum.intersects(base.0, base.1));
        for (axis, limit, out) in [(0, 4024.0, 1.0), (0, 1976.0, -1.0), (1, 4900.0, 1.0), (1, 804.0, -1.0), (2, -6488.0, 1.0), (2, -7512.0, -1.0)] {
            let from = |edge: f64| {
                let (mut min, mut max) = base;
                if out > 0.0 {
                    (min[axis], max[axis]) = (edge, edge + 50.0);
                } else {
                    (min[axis], max[axis]) = (edge - 50.0, edge);
                }

                frustum.intersects(min, max)
            };
            assert!(from(limit), "axis {axis} at {limit}");
            assert!(!from(limit + out), "axis {axis} past {limit}");
        }
    }

    #[test]
    fn frustum_culls_parts_out_of_view() {
        let target = DVec2::new(-3001.6, 777.3);
        let all = ring(target);
        let eye = Vec3::new(target.x as f32 * 32.0, 2000.0, target.y as f32 * 32.0);
        let view = look_to_mat4(eye, Vec3::X, Vec3::Y);
        let ahead = Cull { frustum: Some(Frustum::new(perspective_infinite_reverse(1.0, 16.0 / 9.0, 4.0) * view)), ..OPEN };
        let seen = Instances::new(target, &ahead).parts;
        let behind: Vec<&Part> = all.iter().filter(|p| (p.cells()[1].x as f32 * 32.0) < eye.x - 4.0).collect();
        assert!(!behind.is_empty() && behind.iter().all(|p| !seen.contains(p)));
        let far = all.iter().filter(|p| p.level == LEVELS - 1 && (p.cells()[0].y..p.cells()[1].y).contains(&777)).max_by_key(|p| p.origin.x).unwrap();
        assert!(far.cells()[0].x > 40000 && seen.contains(far), "{far:?}");

        let below = |top: f64| Instances::new(target, &Cull { frustum: Some(top_down()), heights: [-100.0, top], ..OPEN }).parts.len();
        assert_eq!(below(803.0), 0);
        assert!(below(804.0) > 0);
    }

    #[test]
    fn area_drops_parts_that_only_touch_it() {
        let target = DVec2::new(-3001.6, 777.3);
        let all = Instances::new(target, &OPEN);
        assert_eq!(all.parts.len(), ring(target).len());
        let tile = all.parts[0];
        assert_eq!((tile.kind, tile.level), (MeshKind::Tile, 0));
        let [min, max] = tile.cells();
        let far = I64Vec2::splat(1 << 40);
        let kept = |lo: I64Vec2, hi: I64Vec2| Instances::new(target, &Cull { area: Some([lo, hi]), ..OPEN }).parts;
        for (touch, into) in [
            ([-far, I64Vec2::new(min.x, far.y)], [-far, I64Vec2::new(min.x + 1, far.y)]),
            ([I64Vec2::new(max.x, -far.y), far], [I64Vec2::new(max.x - 1, -far.y), far]),
            ([-far, I64Vec2::new(far.x, min.y)], [-far, I64Vec2::new(far.x, min.y + 1)]),
            ([I64Vec2::new(-far.x, max.y), far], [I64Vec2::new(-far.x, max.y - 1), far]),
        ] {
            assert!(!kept(touch[0], touch[1]).contains(&tile), "{touch:?}");
            assert!(kept(into[0], into[1]).contains(&tile), "{into:?}");
        }

        assert_eq!(kept(min + 1, min + 2), vec![tile]);
        assert_eq!(kept(min + 1, min + 1), vec![]);
        assert_eq!(kept(far, far + 10), vec![]);
        assert_eq!(kept(-far, far), all.parts);
    }

    #[test]
    fn full_ring_fits_the_instance_buffer() {
        for t in TARGETS {
            let all = Instances::new(DVec2::from(t), &OPEN);
            assert!(all.parts.len() <= MAX_INSTANCES);
            assert_eq!(all.ranges.clone().map(|r| r.len()), [16 + 12 * 9, 20, 20, 18, 18, 2, 2], "{t:?}");
            for (range, kind) in all.ranges.iter().zip(MeshKind::ALL) {
                assert!(all.parts[range.start as usize..range.end as usize].iter().all(|p| p.kind == kind), "{t:?} {kind:?}");
            }

            assert_eq!(all.ranges[MeshKind::COUNT - 1].end as usize, all.parts.len());
        }
    }

    #[test]
    fn far_target_keeps_origins_in_i32() {
        let limit = 1i64 << 30;
        assert_eq!(snap(DVec2::new(limit as f64 - 0.5, -(limit as f64) + 1.5)), I64Vec2::new(limit - 1, -limit + 1));
        assert_eq!(snap(DVec2::new(limit as f64 + 0.5, -(limit as f64) - 5.0)), I64Vec2::new(limit, -limit));
        for t in [[1e15, -1e15], [f64::INFINITY, -3.5]] {
            let parts = ring(DVec2::from(t));
            assert_eq!(parts.len(), 24 + 20 * (LEVELS as usize - 1));
            for part in parts {
                let gpu = ClipmapInstance::from(part);
                assert_eq!(I64Vec2::new(gpu.origin[0] as i64, gpu.origin[1] as i64), part.origin, "{t:?}");
            }
        }
    }
}
