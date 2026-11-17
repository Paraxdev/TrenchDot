//! Material blending: terrain layer weights, displacement alpha and per corner blend weights on brush and mesh faces
//! that carry a second material in their `blend_material` face property.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use td_core::{DVec2, DVec3, Fold, Mirror, NodeId};
use td_geom::displacement;
use td_geom::heightfield::{MAX_LAYERS, noise2};

use crate::map::Map;

/// Face property naming the second material that vertex color alpha blends towards.
pub const BLEND_MATERIAL: &str = "blend_material";
/// Face property breaking up the repeat of the blend material, 0 to 1. See `TerrainLayer::detile`.
pub const BLEND_DETILE: &str = "blend_detile";
/// Face property multiplying how often the blend material repeats across the face.
pub const BLEND_UV_SCALE: &str = "blend_uv_scale";
/// Face property setting how crisp the de-tiled blend material stays. See `TerrainLayer::detile_sharpen`.
pub const BLEND_DETILE_SHARPEN: &str = "blend_detile_sharpen";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    /// Pushes weight towards the chosen layer.
    #[default]
    Paint,
    /// Pushes weight back to the base layer.
    Erase,
    /// Averages weights with neighbours, softening transitions.
    Smooth,
    /// Hardens transitions towards the dominant layer.
    Sharpen,
    /// Paints the layer in noise shaped patches.
    Noise,
    /// Paints the layer only where the surface slope is within range.
    Slope,
    /// Paints the layer only within a height band.
    Height,
}

impl BlendMode {
    pub const ALL: [BlendMode; 7] =
        [BlendMode::Paint, BlendMode::Erase, BlendMode::Smooth, BlendMode::Sharpen, BlendMode::Noise, BlendMode::Slope, BlendMode::Height];

    pub fn label(&self) -> &'static str {
        match self {
            BlendMode::Paint => "paint",
            BlendMode::Erase => "erase",
            BlendMode::Smooth => "smooth",
            BlendMode::Sharpen => "sharpen",
            BlendMode::Noise => "noise",
            BlendMode::Slope => "slope",
            BlendMode::Height => "height",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Falloff {
    #[default]
    Smooth,
    Linear,
    Constant,
    /// Random speckles thinning out towards the rim.
    Spray,
}

impl Falloff {
    pub const ALL: [Falloff; 4] = [Falloff::Smooth, Falloff::Linear, Falloff::Constant, Falloff::Spray];

    pub fn label(&self) -> &'static str {
        match self {
            Falloff::Smooth => "smooth",
            Falloff::Linear => "linear",
            Falloff::Constant => "constant",
            Falloff::Spray => "spray",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlendBrush {
    pub mode: BlendMode,
    pub falloff: Falloff,
    pub radius: f64,
    /// 0..1 per application.
    pub strength: f64,
    /// Terrain layer 0..3. Two texture targets treat 0 as the base material and anything else as the blend material.
    pub layer: usize,
    /// Slope range in degrees for `Slope`.
    pub slope: [f64; 2],
    /// World height range for `Height`.
    pub height: [f64; 2],
    /// World size of the noise patches for `Noise`.
    pub noise_scale: f64,
    /// Changes the spray and noise pattern, bump it per dab for fresh speckles.
    pub seed: u32,
}

impl Default for BlendBrush {
    fn default() -> Self {
        Self {
            mode: BlendMode::Paint,
            falloff: Falloff::Smooth,
            radius: 128.0,
            strength: 0.5,
            layer: 1,
            slope: [30.0, 90.0],
            height: [-1.0e5, 1.0e5],
            noise_scale: 256.0,
            seed: 1,
        }
    }
}

fn hash(p: DVec3, seed: u32) -> f64 {
    let k = ((p.x * 7.13).floor() as i64).wrapping_mul(73_856_093)
        ^ ((p.y * 5.71).floor() as i64).wrapping_mul(19_349_663)
        ^ ((p.z * 6.37).floor() as i64).wrapping_mul(83_492_791);
    let mut h = (k as u64 ^ seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 31;
    (h % 10_000) as f64 / 10_000.0
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl BlendBrush {
    /// Brush influence at a point, 0 outside the radius.
    pub fn influence(&self, p: DVec3, center: DVec3, horizontal: bool) -> f64 {
        let d = if horizontal { DVec2::new(p.x - center.x, p.z - center.z).length() } else { (p - center).length() };
        if d >= self.radius || self.radius <= 0.0 {
            return 0.0;
        }

        let t = d / self.radius;
        let w = match self.falloff {
            Falloff::Smooth => (1.0 - t * t).powi(2),
            Falloff::Linear => 1.0 - t,
            Falloff::Constant => 1.0,
            Falloff::Spray => {
                if hash(p, self.seed) < 0.35 * (1.0 - t) {
                    1.0
                } else {
                    0.0
                }
            }
        };
        w * self.strength.clamp(0.0, 1.0)
    }

    /// How much a per point mode applies at a point, 1 for plain painting.
    fn mask(&self, p: DVec3, normal: DVec3) -> f64 {
        match self.mode {
            BlendMode::Noise => {
                let n = noise2(DVec2::new(p.x, p.z) / self.noise_scale.max(1.0), self.seed ^ 0x6e6f) * 0.5 + 0.5;
                smoothstep(0.42, 0.58, n)
            }
            BlendMode::Slope => {
                let s = normal.normalize_or(DVec3::Y).y.clamp(-1.0, 1.0).acos().to_degrees();
                smoothstep(self.slope[0] - 4.0, self.slope[0] + 4.0, s) * (1.0 - smoothstep(self.slope[1] - 4.0, self.slope[1] + 4.0, s))
            }
            BlendMode::Height => {
                let soft = ((self.height[1] - self.height[0]).abs() * 0.1).clamp(1.0, 64.0);
                smoothstep(self.height[0] - soft, self.height[0] + soft, p.y) * (1.0 - smoothstep(self.height[1] - soft, self.height[1] + soft, p.y))
            }
            _ => 1.0,
        }
    }

    fn target_layer(&self) -> usize {
        if self.mode == BlendMode::Erase { 0 } else { self.layer }
    }
}

fn normalize(w: &mut [f64]) {
    let sum: f64 = w.iter().sum();
    if sum <= 1e-9 {
        w.iter_mut().enumerate().for_each(|(i, v)| *v = if i == 0 { 1.0 } else { 0.0 });
    } else {
        w.iter_mut().for_each(|v| *v /= sum);
    }
}

/// New weights for one point. `neighbours` is the average of nearby weights, used by smoothing.
fn apply(brush: &BlendBrush, influence: f64, p: DVec3, normal: DVec3, current: &[f64], neighbours: Option<&[f64]>) -> Vec<f64> {
    let mut w = current.to_vec();
    match brush.mode {
        BlendMode::Smooth => {
            if let Some(avg) = neighbours {
                for (v, a) in w.iter_mut().zip(avg) {
                    *v += (a - *v) * influence;
                }
            }
        }
        BlendMode::Sharpen => {
            let power = 1.0 + influence * 3.0;
            w.iter_mut().for_each(|v| *v = v.max(0.0).powf(power));
            normalize(&mut w);
        }
        _ => {
            let amount = influence * brush.mask(p, normal);
            // `w` holds only the slots that have a layer, so a missing layer goes to the last one, like Terrain::layer_slot.
            let layer = brush.target_layer().min(w.len() - 1);
            for (i, v) in w.iter_mut().enumerate() {
                let target = if i == layer { 1.0 } else { 0.0 };
                *v += (target - *v) * amount;
            }
        }
    }

    normalize(&mut w);
    w
}

/// Applies one dab to terrain layer weights, and its mirror images across `mirror`. Returns true if anything changed.
pub fn blend_terrain(t: &mut td_geom::Terrain, center: DVec3, brush: &BlendBrush, mirror: &Mirror) -> bool {
    let [w, d] = t.resolution;
    let n = t.heights.len();
    if !t.is_valid() {
        return false;
    }

    let window = |c: DVec3| {
        let local = c - t.origin;
        let r = brush.radius / t.cell_size;
        let (x0, x1) = (((local.x / t.cell_size) - r).floor().max(0.0) as u32, (((local.x / t.cell_size) + r).ceil().max(0.0) as u32).min(w - 1));
        let (z0, z1) = (((local.z / t.cell_size) - r).floor().max(0.0) as u32, (((local.z / t.cell_size) + r).ceil().max(0.0) as u32).min(d - 1));
        let off = x0 > x1 || z0 > z1 || local.x + brush.radius < 0.0 || local.z + brush.radius < 0.0;
        (!off).then_some((x0, x1, z0, z1))
    };
    let windows: Vec<(Fold, (u32, u32, u32, u32))> = mirror.folds().filter_map(|f| Some((f, window(f.point(center))?))).collect();
    if windows.is_empty() {
        return false;
    }

    let layers = t.layers.len().clamp(1, MAX_LAYERS);
    if t.splat.len() != n * 4 {
        t.splat = (0..n).flat_map(|_| [255u8, 0, 0, 0]).collect();
    }

    // Every window is read before any is written, so Smooth sees the same weights on both sides of a plane.
    let snapshots: Vec<Vec<[f32; 4]>> = windows
        .iter()
        .map(|(_, (x0, x1, z0, z1))| (*z0..=*z1).flat_map(|j| (*x0..=*x1).map(move |i| (i, j))).map(|(i, j)| t.weights(i, j)).collect())
        .collect();
    let mut changed = false;
    for ((fold, (x0, x1, z0, z1)), old) in windows.iter().zip(&snapshots) {
        let (x0, x1, z0, z1) = (*x0, *x1, *z0, *z1);
        let row = (x1 - x0 + 1) as usize;
        let get = |i: u32, j: u32| old[(j - z0) as usize * row + (i - x0) as usize];
        for j in z0..=z1 {
            for i in x0..=x1 {
                let p = t.vertex(i, j);
                if mirror.fold(p, center) != *fold {
                    continue;
                }

                let at = fold.point(p);
                let influence = brush.influence(at, center, true);
                if influence <= 0.0 {
                    continue;
                }

                let current: Vec<f64> = get(i, j).iter().take(layers).map(|v| *v as f64).collect();
                let neighbours = (brush.mode == BlendMode::Smooth).then(|| {
                    let mut sum = vec![0.0; layers];
                    let mut count = 0.0;
                    for (di, dj) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                        let (ni, nj) = (i as i64 + di, j as i64 + dj);
                        if ni >= x0 as i64 && nj >= z0 as i64 && ni <= x1 as i64 && nj <= z1 as i64 {
                            let nw = get(ni as u32, nj as u32);
                            for (s, v) in sum.iter_mut().zip(nw) {
                                *s += v as f64;
                            }

                            count += 1.0;
                        }
                    }

                    sum.iter().map(|s| s / f64::max(count, 1.0)).collect::<Vec<_>>()
                });
                let new = apply(brush, influence, at, fold.vector(t.normal(i, j)), &current, neighbours.as_deref());
                let k = t.index(i, j) * 4;
                for l in 0..4 {
                    let v = (new.get(l).copied().unwrap_or(0.0) * 255.0).round().clamp(0.0, 255.0) as u8;
                    if t.splat[k + l] != v {
                        t.splat[k + l] = v;
                        changed = true;
                    }
                }
            }
        }
    }

    changed
}

/// Applies one dab and its mirror images across `mirror` to displacement alphas (0 base, 1 blend texture).
pub fn blend_displacements(map: &mut Map, faces: &[(NodeId, usize)], center: DVec3, brush: &BlendBrush, mirror: &Mirror) -> bool {
    let mut changed = false;
    for (id, face) in faces {
        let Some(b) = map.brush(*id) else { continue };
        let Some(grid) = displacement::grid(b, *face) else { continue };
        let n = grid.size;
        let folds: Vec<Fold> = grid.positions.iter().map(|p| mirror.fold(*p, center)).collect();
        let influences: Vec<f64> = grid.positions.iter().zip(&folds).map(|(p, f)| brush.influence(f.point(*p), center, false)).collect();
        if influences.iter().all(|w| *w <= 0.0) {
            continue;
        }

        let normal = b.faces[*face].plane.normal;
        let Some(disp) = map.brush_mut(*id).and_then(|b| b.faces[*face].data.disp.as_mut()) else { continue };
        if disp.alphas.len() != n * n {
            disp.alphas = vec![0.0; n * n];
        }

        let old = disp.alphas.clone();
        for k in 0..n * n {
            if influences[k] <= 0.0 {
                continue;
            }

            let (i, j) = (k % n, k / n);
            let a = old[k] as f64;
            let neighbours = (brush.mode == BlendMode::Smooth).then(|| {
                let mut sum = 0.0;
                let mut count = 0.0;
                for (di, dj) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                    let (ni, nj) = (i as i64 + di, j as i64 + dj);
                    if ni >= 0 && nj >= 0 && (ni as usize) < n && (nj as usize) < n {
                        sum += old[nj as usize * n + ni as usize] as f64;
                        count += 1.0;
                    }
                }

                let avg = sum / f64::max(count, 1.0);
                vec![1.0 - avg, avg]
            });
            let (at, normal) = (folds[k].point(grid.positions[k]), folds[k].vector(grid.normals.get(k).copied().unwrap_or(normal)));
            let new = apply(brush, influences[k], at, normal, &[1.0 - a, a], neighbours.as_deref());
            disp.alphas[k] = new[1] as f32;
            changed |= (new[1] - a).abs() > 1e-6;
        }
    }

    changed
}

/// Brush and mesh faces carrying a blend material among `nodes` (every node when empty).
pub fn blend_faces(map: &Map, nodes: &[NodeId]) -> Vec<(NodeId, usize)> {
    let ids: Vec<NodeId> = if nodes.is_empty() { map.nodes.keys().copied().collect() } else { nodes.to_vec() };
    let mut out = Vec::new();
    for id in ids {
        if let Some(b) = map.brush(id) {
            out.extend(b.faces.iter().enumerate().filter(|(_, f)| f.data.disp.is_none() && f.data.props.contains_key(BLEND_MATERIAL)).map(|(i, _)| (id, i)));
        } else if let Some(m) = map.mesh(id) {
            out.extend(m.faces.iter().enumerate().filter(|(_, f)| f.data.props.contains_key(BLEND_MATERIAL)).map(|(i, _)| (id, i)));
        }
    }

    out
}

/// Per corner blend weights of brush and mesh faces, under a dab and its mirror images across `mirror`. Smoothing averages
/// corners that meet at the same position.
pub fn blend_face_corners(map: &mut Map, faces: &[(NodeId, usize)], center: DVec3, brush: &BlendBrush, mirror: &Mirror) -> bool {
    struct Corner {
        id: NodeId,
        face: usize,
        corner: usize,
        pos: DVec3,
        normal: DVec3,
        alpha: f64,
    }

    let mut corners = Vec::new();
    for (id, face) in faces {
        let (points, normal, colors) = if let Some(b) = map.brush(*id) {
            let Some(f) = b.faces.get(*face) else { continue };
            (f.indices.iter().map(|i| b.vertices[*i as usize]).collect::<Vec<_>>(), f.plane.normal, f.data.colors.clone())
        } else if let Some(m) = map.mesh(*id) {
            let Some(f) = m.faces.get(*face) else { continue };
            (m.face_points(*face), m.face_normal(*face), f.data.colors.clone())
        } else {
            continue;
        };
        for (k, p) in points.iter().enumerate() {
            let alpha = if colors.len() == points.len() { colors[k][3] as f64 } else { 0.0 };
            corners.push(Corner { id: *id, face: *face, corner: k, pos: *p, normal, alpha });
        }
    }

    let key = |p: DVec3| ((p.x * 8.0).round() as i64, (p.y * 8.0).round() as i64, (p.z * 8.0).round() as i64);
    let mut shared: HashMap<(i64, i64, i64), (f64, f64)> = HashMap::new();
    for c in &corners {
        let e = shared.entry(key(c.pos)).or_insert((0.0, 0.0));
        e.0 += c.alpha;
        e.1 += 1.0;
    }

    let mut writes = Vec::new();
    for c in &corners {
        let fold = mirror.fold(c.pos, center);
        let at = fold.point(c.pos);
        let influence = brush.influence(at, center, false);
        if influence <= 0.0 {
            continue;
        }

        let neighbours = (brush.mode == BlendMode::Smooth).then(|| {
            // Neighbouring corners inside the brush, so a brush wide stroke evens out whole faces.
            let (mut sum, mut count) = shared.get(&key(c.pos)).copied().unwrap_or((c.alpha, 1.0));
            for o in &corners {
                if (o.pos - c.pos).length() <= brush.radius * 0.5 && key(o.pos) != key(c.pos) {
                    sum += o.alpha;
                    count += 1.0;
                }
            }

            let avg = sum / count.max(1.0);
            vec![1.0 - avg, avg]
        });
        let new = apply(brush, influence, at, fold.vector(c.normal), &[1.0 - c.alpha, c.alpha], neighbours.as_deref());
        if (new[1] - c.alpha).abs() > 1e-6 {
            writes.push((c.id, c.face, c.corner, new[1] as f32));
        }
    }

    let changed = !writes.is_empty();
    for (id, face, corner, alpha) in writes {
        let data = if let Some(b) = map.brush_mut(id) {
            b.faces.get_mut(face).map(|f| (f.indices.len(), &mut f.data))
        } else {
            map.mesh_mut(id).and_then(|m| m.faces.get_mut(face)).map(|f| (f.indices.len(), &mut f.data))
        };
        let Some((len, data)) = data else { continue };
        if data.colors.len() != len {
            data.colors = vec![[1.0, 1.0, 1.0, 0.0]; len];
        }

        data.colors[corner][3] = alpha;
    }

    changed
}

/// Sets or clears the blend material of faces. Clearing also drops the blend weights stored in vertex alpha.
pub fn set_blend_material(map: &mut Map, faces: &[(NodeId, usize)], material: Option<&str>) -> usize {
    let mut n = 0;
    for (id, face) in faces {
        let data = if let Some(b) = map.brush_mut(*id) {
            b.faces.get_mut(*face).map(|f| &mut f.data)
        } else {
            map.mesh_mut(*id).and_then(|m| m.faces.get_mut(*face)).map(|f| &mut f.data)
        };
        let Some(data) = data else { continue };
        match material {
            Some(m) => {
                data.props.insert(BLEND_MATERIAL.into(), m.to_string());
            }
            None => {
                data.props.remove(BLEND_MATERIAL);
            }
        }

        n += 1;
    }

    n
}

/// Sets how the blend material of `faces` repeats: `detile` breaks up the tiling, `uv_scale` multiplies how
/// often it repeats and `sharpen` keeps the de-tiled result crisp. Defaults drop the property again so plain
/// faces stay plain in the file.
pub fn set_blend_options(map: &mut Map, faces: &[(NodeId, usize)], detile: f64, uv_scale: f64, sharpen: f64) -> usize {
    let mut n = 0;
    for (id, face) in faces {
        let data = if let Some(b) = map.brush_mut(*id) {
            b.faces.get_mut(*face).map(|f| &mut f.data)
        } else {
            map.mesh_mut(*id).and_then(|m| m.faces.get_mut(*face)).map(|f| &mut f.data)
        };
        let Some(data) = data else { continue };
        for (key, value, default) in
            [(BLEND_DETILE, detile.clamp(0.0, 1.0), 0.0), (BLEND_UV_SCALE, uv_scale.max(0.001), 1.0), (BLEND_DETILE_SHARPEN, sharpen.clamp(0.0, 1.0), 0.5)]
        {
            if (value - default).abs() < 1e-6 {
                data.props.remove(key);
            } else {
                data.props.insert(key.into(), format!("{value:.3}"));
            }
        }

        n += 1;
    }

    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeKind;
    use crate::terrain::tests::{P, X, assert_mirrored, mirror_x, mirrored_pair, surface};
    use td_core::Aabb;
    use td_geom::{Brush, Terrain, TerrainLayer};

    fn terrain() -> Terrain {
        let mut t = Terrain::new(DVec3::ZERO, [33, 33], 16.0, "grass");
        t.layers.push(TerrainLayer::new("rock", 128.0));
        t.layers.push(TerrainLayer::new("dirt", 128.0));
        t
    }

    #[test]
    fn paint_erase_smooth_and_sharpen_on_terrain() {
        let mut t = terrain();
        let brush = BlendBrush { layer: 2, strength: 1.0, radius: 64.0, falloff: Falloff::Constant, ..Default::default() };
        assert!(blend_terrain(&mut t, DVec3::new(256.0, 0.0, 256.0), &brush, &Mirror::NONE));
        assert!(t.weights(16, 16)[2] > 0.99);
        assert!(t.weights(0, 0)[0] > 0.99, "outside the brush stays base");

        let smooth = BlendBrush { mode: BlendMode::Smooth, radius: 200.0, strength: 1.0, ..brush };
        blend_terrain(&mut t, DVec3::new(256.0, 0.0, 256.0), &smooth, &Mirror::NONE);
        let edge = t.weights(20, 16)[2];
        assert!(edge > 0.05 && edge < 0.95, "smoothing spreads the edge, got {edge}");

        let sharpen = BlendBrush { mode: BlendMode::Sharpen, ..smooth };
        let before = t.weights(20, 16);
        blend_terrain(&mut t, DVec3::new(256.0, 0.0, 256.0), &sharpen, &Mirror::NONE);
        let after = t.weights(20, 16);
        let dominant = if before[0] > before[2] { 0 } else { 2 };
        assert!(after[dominant] >= before[dominant]);

        let erase = BlendBrush { mode: BlendMode::Erase, radius: 400.0, ..brush };
        blend_terrain(&mut t, DVec3::new(256.0, 0.0, 256.0), &erase, &Mirror::NONE);
        assert!(t.weights(16, 16)[0] > 0.99);
    }

    #[test]
    fn slope_mode_only_paints_steep_ground() {
        let mut t = terrain();
        for j in 0..33 {
            for i in 17..33 {
                let k = t.index(i, j);
                t.heights[k] = (i as f32 - 16.0) * 32.0;
            }
        }

        let brush = BlendBrush {
            mode: BlendMode::Slope,
            layer: 1,
            strength: 1.0,
            radius: 10_000.0,
            falloff: Falloff::Constant,
            slope: [35.0, 90.0],
            ..Default::default()
        };
        blend_terrain(&mut t, DVec3::new(256.0, 0.0, 256.0), &brush, &Mirror::NONE);
        assert!(t.weights(4, 10)[1] < 0.01, "flat ground untouched");
        assert!(t.weights(25, 10)[1] > 0.9, "steep ramp painted");
    }

    #[test]
    fn face_blend_weights_and_material() {
        let mut m = Map::new();
        let l = m.default_layer();
        let id = m.insert(l, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::splat(64.0)), "bricks").unwrap()));
        let top = m.brush(id).unwrap().faces.iter().position(|f| f.plane.normal.y > 0.5).unwrap();
        assert_eq!(set_blend_material(&mut m, &[(id, top)], Some("moss")), 1);
        let faces = blend_faces(&m, &[]);
        assert_eq!(faces, vec![(id, top)]);
        let brush = BlendBrush { strength: 1.0, radius: 20.0, falloff: Falloff::Constant, ..Default::default() };
        assert!(blend_face_corners(&mut m, &faces, DVec3::new(0.0, 64.0, 0.0), &brush, &Mirror::NONE));
        let colors = &m.brush(id).unwrap().faces[top].data.colors;
        assert_eq!(colors.iter().filter(|c| c[3] > 0.99).count(), 1, "only the corner under the brush");
        assert!(colors.iter().all(|c| c[0] == 1.0), "tint stays white");
    }

    #[test]
    fn blend_tiling_options_are_written_and_cleared() {
        let mut m = Map::new();
        let l = m.default_layer();
        let id = m.insert(l, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::splat(64.0)), "bricks").unwrap()));
        let top = m.brush(id).unwrap().faces.iter().position(|f| f.plane.normal.y > 0.5).unwrap();
        let props = |m: &Map| m.brush(id).unwrap().faces[top].data.props.clone();
        assert_eq!(set_blend_options(&mut m, &[(id, top)], 0.75, 4.0, 0.9), 1);
        assert_eq!(props(&m).get(BLEND_DETILE).map(String::as_str), Some("0.750"));
        assert_eq!(props(&m).get(BLEND_UV_SCALE).map(String::as_str), Some("4.000"));
        assert_eq!(props(&m).get(BLEND_DETILE_SHARPEN).map(String::as_str), Some("0.900"));

        // Back to the defaults drops the properties again, so plain faces stay plain in the saved file.
        set_blend_options(&mut m, &[(id, top)], 0.0, 1.0, 0.5);
        assert!(!props(&m).contains_key(BLEND_DETILE) && !props(&m).contains_key(BLEND_UV_SCALE));
        assert!(!props(&m).contains_key(BLEND_DETILE_SHARPEN), "the default crispness is not written either");
    }

    const MX: f64 = 191.5;
    const MZ: f64 = 184.25;
    const CLEAR: DVec3 = DVec3::new(60.25, 0.0, 80.5);
    const STRADDLING: DVec3 = DVec3::new(170.0, 0.0, 160.3);
    const ON_PLANES: DVec3 = DVec3::new(MX, 0.0, MZ);

    /// Off origin, 19 by 10 vertices with a column on its x middle (MX) and no row on its z middle (MZ). Heights and
    /// four layer weights slant within each quarter and mirror across both middles.
    fn symmetric_terrain() -> Terrain {
        let mut t = Terrain::new(DVec3::new(-96.5, 12.0, 40.25), [19, 10], 32.0, "grass");
        for name in ["rock", "dirt", "sand"] {
            t.layers.push(TerrainLayer::new(name, 128.0));
        }

        let [w, d] = t.resolution;
        t.splat = vec![0; (w * d * 4) as usize];
        for j in 0..d {
            for i in 0..w {
                let (a, b) = (i.min(w - 1 - i) as f32, j.min(d - 1 - j) as f32);
                let k = t.index(i, j);
                t.heights[k] = 9.0 * a + 21.0 * b + 1.5 * a * b;
                let raw = [40.0 + 9.0 * a, 30.0 + 13.0 * b, 20.0 + 2.0 * a * b, 5.0 + (11.0 * (a + b)) % 37.0];
                let sum: f32 = raw.iter().sum();
                for (l, r) in raw.iter().enumerate() {
                    t.splat[k * 4 + l] = (r / sum * 255.0).round() as u8;
                }
            }
        }

        t
    }

    fn mode_brush(mode: BlendMode, falloff: Falloff) -> BlendBrush {
        BlendBrush { mode, falloff, layer: 2, radius: 90.0, strength: 0.7, slope: [20.0, 40.0], height: [80.0, 160.0], noise_scale: 70.0, seed: 11 }
    }

    /// The splat of `alone` with every vertex across a mirrored middle taken from its image on the dab's side, which is
    /// what a mirrored dab leaves on a symmetric start.
    fn folded(alone: &Terrain, x: bool, z: bool, center: DVec3) -> Vec<u8> {
        let [w, d] = alone.resolution;
        let side = |v: f64, n: u32| v >= (n - 1) as f64 / 2.0;
        let (ci, cj) = ((center.x - alone.origin.x) / alone.cell_size, (center.z - alone.origin.z) / alone.cell_size);
        let mut out = alone.splat.clone();
        for j in 0..d {
            for i in 0..w {
                let fi = if x && side(i as f64, w) != side(ci, w) { w - 1 - i } else { i };
                let fj = if z && side(j as f64, d) != side(cj, d) { d - 1 - j } else { j };
                let (to, from) = (alone.index(i, j) * 4, alone.index(fi, fj) * 4);
                out[to..to + 4].copy_from_slice(&alone.splat[from..from + 4]);
            }
        }

        out
    }

    #[test]
    fn mirrored_terrain_dabs_repeat_the_dab_side_on_every_side() {
        let start = symmetric_terrain();
        for (x, z) in [(true, false), (false, true), (true, true)] {
            let mirror = Mirror { x: x.then_some(MX), z: z.then_some(MZ) };
            for (place, center) in [("clear", CLEAR), ("straddling", STRADDLING), ("on the planes", ON_PLANES)] {
                for mode in BlendMode::ALL {
                    for falloff in [Falloff::Smooth, Falloff::Spray] {
                        let what = format!("{mode:?} {falloff:?} {place} x {x} z {z}");
                        let brush = mode_brush(mode, falloff);
                        let pattern = mode == BlendMode::Noise || falloff == Falloff::Spray;
                        let mut alone = start.clone();
                        let changed = blend_terrain(&mut alone, center, &brush, &Mirror::NONE);
                        assert!(changed || pattern, "{what}");
                        let mut t = start.clone();
                        assert_eq!(blend_terrain(&mut t, center, &brush, &mirror), t.splat != start.splat, "{what}");
                        assert_eq!(t.splat, folded(&alone, x, z, center), "{what}");
                        if center == ON_PLANES && !pattern {
                            assert_eq!(t.splat, alone.splat, "{what}: a dab on the planes counts once");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_terrain_dab_or_its_image_off_the_terrain_paints_only_what_lands() {
        let start = symmetric_terrain();
        let paint = BlendBrush { radius: 60.0, ..mode_brush(BlendMode::Paint, Falloff::Smooth) };
        let dab = |center: DVec3, mirror: Mirror| {
            let mut t = start.clone();
            let changed = blend_terrain(&mut t, center, &paint, &mirror);
            assert_eq!(changed, t.splat != start.splat, "{center} {mirror:?}");
            t.splat
        };

        let beyond = Mirror { x: Some(600.0), z: None };
        assert_eq!(dab(CLEAR, beyond), dab(CLEAR, Mirror::NONE), "the image falls past the far edge");

        let before = Mirror { x: Some(-120.0), z: None };
        let (off, image) = (DVec3::new(-180.5, 0.0, 150.0), DVec3::new(-59.5, 0.0, 150.0));
        assert_eq!(dab(off, Mirror::NONE), start.splat, "the dab itself misses");
        assert_ne!(dab(off, before), start.splat);
        assert_eq!(dab(off, before), dab(image, Mirror::NONE), "only the image paints");

        // The image's rim ends exactly on a vertex at the edge, then just past it.
        let edge = Mirror { x: Some(-200.0), z: None };
        let constant = BlendBrush { falloff: Falloff::Constant, ..paint };
        let rim = |x: f64| {
            let mut t = start.clone();
            let changed = blend_terrain(&mut t, DVec3::new(x, 0.0, 136.25), &constant, &edge);
            assert_eq!(changed, t.splat != start.splat, "{x}");
            changed
        };
        assert!(!rim(-243.5));
        assert!(rim(-244.0));

        let bare = Terrain::new(start.origin, start.resolution, start.cell_size, "grass");
        let mut t = bare.clone();
        assert!(!blend_terrain(&mut t, off, &paint, &edge));
        assert_eq!(t, bare, "neither the dab nor its image lands, so the splat is not even created");
    }

    /// Alphas that slant across the first face and mirror onto the second.
    fn mirrored_alphas(m: &mut Map, faces: [(NodeId, usize); 2]) {
        for (k, (id, face)) in faces.into_iter().enumerate() {
            let base = displacement::grid(m.brush(id).unwrap(), face).unwrap().base;
            let disp = m.brush_mut(id).unwrap().faces[face].data.disp.as_mut().unwrap();
            disp.alphas = base
                .iter()
                .map(|p| if k == 0 { *p } else { mirror_x(*p) })
                .map(|p| (0.1 + 0.005 * (p.x + 50.0) + 0.003 * (p.z - 40.0) + 0.0001 * (p.z - 60.0).powi(2)) as f32)
                .collect();
        }
    }

    #[test]
    fn mirrored_displacement_dabs_blend_the_mirrored_brush() {
        let floor = Aabb::new(DVec3::new(P - 64.0, -4.0, 40.0), DVec3::new(P, 12.0, 104.0));
        let centers =
            [("clear", DVec3::new(P - 37.0, 12.0, 61.0)), ("straddling", DVec3::new(P - 10.0, 12.0, 61.0)), ("on the plane", DVec3::new(P, 12.0, 61.0))];
        for (place, center) in centers {
            for mode in BlendMode::ALL {
                for falloff in [Falloff::Smooth, Falloff::Spray] {
                    let what = format!("{mode:?} {falloff:?} {place}");
                    let brush = BlendBrush { radius: 30.0, slope: [0.0, 40.0], height: [10.0, 30.0], noise_scale: 12.0, ..mode_brush(mode, falloff) };
                    let (mut alone, faces) = mirrored_pair(floor, DVec3::Y);
                    mirrored_alphas(&mut alone, faces);
                    let start = alone.clone();
                    blend_displacements(&mut alone, &faces, center, &brush, &Mirror::NONE);
                    let mut m = start.clone();
                    blend_displacements(&mut m, &faces, center, &brush, &X);
                    assert_mirrored(&m, faces[0], faces[1], &what);
                    let near = faces[(center.x >= P) as usize];
                    assert_eq!(surface(&m, near), surface(&alone, near), "{what}: the dab's own side is one plain dab");
                    if place == "clear" {
                        assert_eq!(surface(&alone, faces[1]), surface(&start, faces[1]), "{what}: without a mirror");
                        assert_ne!(surface(&m, faces[1]), surface(&start, faces[1]), "{what}");
                    }

                    if place == "on the plane" && mode != BlendMode::Noise && falloff != Falloff::Spray {
                        assert_eq!(surface(&m, faces[0]), surface(&alone, faces[0]), "{what}: counts once");
                    }
                }
            }
        }
    }

    #[test]
    fn mirrored_face_dabs_blend_the_mirrored_corners() {
        let mut start = Map::new();
        let l = start.default_layer();
        let a = Aabb::new(DVec3::new(P - 40.0, -4.0, 40.0), DVec3::new(P - 8.0, 12.0, 72.0));
        let mut faces = Vec::new();
        for bounds in [a, Aabb::new(mirror_x(a.min), mirror_x(a.max))] {
            let id = start.insert(l, NodeKind::Brush(Brush::from_aabb(&bounds, "bricks").unwrap()));
            faces.push((id, start.brush(id).unwrap().faces.iter().position(|f| f.plane.normal.y > 0.5).unwrap()));
        }

        assert_eq!(set_blend_material(&mut start, &faces, Some("moss")), 2);
        let alpha = |p: DVec3| (0.2 + 0.006 * (p.x + 50.0) + 0.004 * (p.z - 30.0)) as f32;
        for (k, (id, face)) in faces.iter().enumerate() {
            let b = start.brush_mut(*id).unwrap();
            let corners: Vec<DVec3> = b.faces[*face].indices.iter().map(|i| b.vertices[*i as usize]).collect();
            b.faces[*face].data.colors = corners.iter().map(|p| [1.0, 1.0, 1.0, alpha(if k == 0 { *p } else { mirror_x(*p) })]).collect();
        }

        let corners = |m: &Map, (id, face): (NodeId, usize)| {
            let b = m.brush(id).unwrap();
            b.faces[face].indices.iter().zip(&b.faces[face].data.colors).map(|(i, c)| (b.vertices[*i as usize], c[3])).collect::<Vec<_>>()
        };
        let centers =
            [("clear", DVec3::new(P - 40.0, 12.0, 40.0)), ("straddling", DVec3::new(P - 6.0, 12.0, 50.0)), ("on the plane", DVec3::new(P, 12.0, 56.0))];
        for (place, center) in centers {
            for mode in BlendMode::ALL {
                for falloff in [Falloff::Smooth, Falloff::Spray] {
                    let what = format!("{mode:?} {falloff:?} {place}");
                    let brush = BlendBrush { radius: 45.0, slope: [0.0, 40.0], height: [0.0, 30.0], noise_scale: 12.0, ..mode_brush(mode, falloff) };
                    let mut alone = start.clone();
                    blend_face_corners(&mut alone, &faces, center, &brush, &Mirror::NONE);
                    let mut m = start.clone();
                    blend_face_corners(&mut m, &faces, center, &brush, &X);
                    let (ca, cb) = (corners(&m, faces[0]), corners(&m, faces[1]));
                    assert_eq!(ca.len(), 4, "{what}");
                    for (p, alpha) in &ca {
                        let (_, mirrored) = cb.iter().find(|(q, _)| (*q - mirror_x(*p)).length() < 1e-9).unwrap_or_else(|| panic!("{what}: no mirror of {p}"));
                        assert!((alpha - mirrored).abs() < 1e-6, "{what} at {p}: {alpha}, mirrored {mirrored}");
                    }

                    let near = faces[(center.x >= P) as usize];
                    assert_eq!(corners(&m, near), corners(&alone, near), "{what}: the dab's own side is one plain dab");
                    if place == "clear" {
                        assert_eq!(corners(&alone, faces[1]), corners(&start, faces[1]), "{what}: without a mirror");
                    }

                    if place == "on the plane" && mode != BlendMode::Noise && falloff != Falloff::Spray {
                        assert_eq!(corners(&m, faces[0]), corners(&alone, faces[0]), "{what}: counts once");
                    }
                }
            }
        }
    }
}
