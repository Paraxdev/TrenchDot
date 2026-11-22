//! Displacement, heightmap terrain and vertex paint editing.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use td_core::{DVec3, Fold, Mirror, NodeId, Ray};
use td_geom::displacement::{self, Displacement};

use crate::map::Map;
use crate::symmetry::Symmetry;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SculptMode {
    Raise,
    Lower,
    Smooth,
    /// Pulls heights towards the height under the stroke start.
    Flatten,
    Noise,
    PaintAlpha,
    EraseAlpha,
    /// Quantizes heights into steps of `terrace_step`.
    Terrace,
    /// Terrains: paints blend layer `layer`.
    PaintLayer,
    /// Terrains: cuts holes.
    Hole,
    /// Terrains: fills holes.
    Unhole,
    /// Terrains: adds the tile under the brush, growing the terrain when it lies outside.
    AddTile,
    /// Terrains: cuts the tile under the brush out and crops empty border tiles.
    RemoveTile,
}

impl SculptMode {
    pub const ALL: [SculptMode; 13] = [
        SculptMode::Raise,
        SculptMode::Lower,
        SculptMode::Smooth,
        SculptMode::Flatten,
        SculptMode::Noise,
        SculptMode::Terrace,
        SculptMode::PaintAlpha,
        SculptMode::EraseAlpha,
        SculptMode::PaintLayer,
        SculptMode::Hole,
        SculptMode::Unhole,
        SculptMode::AddTile,
        SculptMode::RemoveTile,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            SculptMode::Raise => "Raise",
            SculptMode::Lower => "Lower",
            SculptMode::Smooth => "Smooth",
            SculptMode::Flatten => "Flatten",
            SculptMode::Noise => "Noise",
            SculptMode::PaintAlpha => "Paint Alpha",
            SculptMode::EraseAlpha => "Erase Alpha",
            SculptMode::Terrace => "Terrace",
            SculptMode::PaintLayer => "Paint Layer",
            SculptMode::Hole => "Hole",
            SculptMode::Unhole => "Unhole",
            SculptMode::AddTile => "Add Tile",
            SculptMode::RemoveTile => "Remove Tile",
        }
    }

    pub fn is_paint(&self) -> bool {
        matches!(self, SculptMode::PaintAlpha | SculptMode::EraseAlpha | SculptMode::PaintLayer | SculptMode::Hole | SculptMode::Unhole)
    }

    pub fn is_tile(&self) -> bool {
        matches!(self, SculptMode::AddTile | SculptMode::RemoveTile)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SculptBrush {
    pub mode: SculptMode,
    pub radius: f64,
    /// Units per application for heights, 0..1 per application for alpha.
    pub strength: f64,
    /// Target height for flatten, in world units along the face normal.
    pub flatten_height: f64,
    pub terrace_step: f64,
    /// Terrain blend layer painted by `PaintLayer`.
    pub layer: u8,
    /// Picks the `Noise` pattern. Strokes change it per dab, so dabs in one place do not pile the same bumps into spikes.
    pub seed: u32,
}

impl Default for SculptBrush {
    fn default() -> Self {
        Self { mode: SculptMode::Raise, radius: 48.0, strength: 4.0, flatten_height: 0.0, terrace_step: 32.0, layer: 1, seed: 0 }
    }
}

/// Smooth falloff, 1 at the center and 0 at the radius.
pub fn falloff(distance: f64, radius: f64) -> f64 {
    if distance >= radius || radius <= 0.0 {
        return 0.0;
    }

    let t = distance / radius;
    (1.0 - t * t).powi(2)
}

/// Converts quad faces into displacements. Returns how many faces were converted.
pub fn create_displacements(map: &mut Map, faces: &[(NodeId, usize)], power: u8) -> usize {
    let mut count = 0;
    for (id, face) in faces {
        let Some(brush) = map.brush_mut(*id) else { continue };
        let Some(f) = brush.faces.get_mut(*face) else { continue };
        if f.indices.len() != 4 {
            continue;
        }

        f.data.disp = Some(match &f.data.disp {
            Some(existing) if existing.power != power => existing.resample(power),
            Some(existing) => existing.clone(),
            None => Displacement::new(power),
        });
        count += 1;
    }

    count
}

pub fn remove_displacements(map: &mut Map, faces: &[(NodeId, usize)]) {
    for (id, face) in faces {
        if let Some(f) = map.brush_mut(*id).and_then(|b| b.faces.get_mut(*face)) {
            f.data.disp = None;
        }
    }
}

/// Every displacement face of the given brushes (or the whole map when `brushes` is empty).
pub fn displacement_faces(map: &Map, brushes: &[NodeId]) -> Vec<(NodeId, usize)> {
    let mut out = Vec::new();
    let ids: Vec<NodeId> = if brushes.is_empty() { map.brushes().map(|(id, _)| id).collect() } else { brushes.to_vec() };
    for id in ids {
        if let Some(b) = map.brush(id) {
            for (fi, f) in b.faces.iter().enumerate() {
                if f.data.disp.is_some() {
                    out.push((id, fi));
                }
            }
        }
    }

    out
}

/// Nearest hit of a ray on displacement surfaces: (distance, point, brush, face).
pub fn ray_cast(map: &Map, faces: &[(NodeId, usize)], ray: &Ray) -> Option<(f64, DVec3, NodeId, usize)> {
    let mut best: Option<(f64, DVec3, NodeId, usize)> = None;
    for (id, face) in faces {
        if map.is_hidden(*id) {
            continue;
        }

        let Some(brush) = map.brush(*id) else { continue };
        let Some(grid) = displacement::grid(brush, *face) else { continue };
        for (a, b, c) in displacement::triangles(grid.size) {
            if let Some(t) = ray.intersect_triangle(grid.positions[a], grid.positions[b], grid.positions[c])
                && best.is_none_or(|(bt, ..)| t < bt)
            {
                best = Some((t, ray.at(t), *id, *face));
            }
        }
    }

    best
}

fn hash_noise(p: DVec3, seed: u32) -> f64 {
    let v = (p.x * 12.9898 + p.y * 78.233 + p.z * 37.719 + seed as f64 * 7.137).sin() * 43758.5453;
    v.fract() * 2.0 - 1.0
}

/// Applies one sculpt dab at `center` and at its images across `mirror`. Returns true if anything changed.
pub fn sculpt(map: &mut Map, faces: &[(NodeId, usize)], center: DVec3, brush: &SculptBrush, mirror: &Mirror) -> bool {
    let mut changed = false;
    for (id, face) in faces {
        let Some(b) = map.brush(*id) else { continue };
        let Some(grid) = displacement::grid(b, *face) else { continue };
        let normal = b.faces[*face].plane.normal;
        let n = grid.size;
        // A vertex is weighed at its image on the dab's side, so a dab overlapping its own mirror counts once.
        let folds: Vec<Fold> = grid.positions.iter().map(|p| mirror.fold(*p, center)).collect();
        let weights: Vec<f64> = grid.positions.iter().zip(&folds).map(|(p, f)| falloff((f.point(*p) - center).length(), brush.radius)).collect();
        if weights.iter().all(|w| *w <= 0.0) {
            continue;
        }

        let old_heights = b.faces[*face].data.disp.as_ref().unwrap().heights.clone();
        let Some(disp) = map.brush_mut(*id).and_then(|b| b.faces[*face].data.disp.as_mut()) else { continue };
        if matches!(brush.mode, SculptMode::PaintAlpha | SculptMode::PaintLayer | SculptMode::EraseAlpha) && disp.alphas.len() != n * n {
            disp.alphas = vec![0.0; n * n];
        }

        for k in 0..n * n {
            let w = weights[k];
            if w <= 0.0 {
                continue;
            }

            let (i, j) = (k % n, k / n);
            match brush.mode {
                SculptMode::Raise => disp.heights[k] += (brush.strength * w) as f32,
                SculptMode::Lower => disp.heights[k] -= (brush.strength * w) as f32,
                SculptMode::Smooth => {
                    let mut sum = 0.0;
                    let mut cnt = 0.0;
                    for (di, dj) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                        let (ni, nj) = (i as i64 + di, j as i64 + dj);
                        if ni >= 0 && nj >= 0 && (ni as usize) < n && (nj as usize) < n {
                            sum += old_heights[nj as usize * n + ni as usize] as f64;
                            cnt += 1.0;
                        }
                    }

                    let avg = sum / cnt;
                    let t = (w * brush.strength.clamp(0.0, 16.0) / 16.0).clamp(0.0, 1.0);
                    disp.heights[k] = (old_heights[k] as f64 * (1.0 - t) + avg * t) as f32;
                }
                SculptMode::Flatten => {
                    let base_height = folds[k].point(grid.base[k]).dot(folds[k].vector(normal));
                    let target = brush.flatten_height - base_height;
                    let t = (w * brush.strength.clamp(0.0, 16.0) / 16.0).clamp(0.0, 1.0);
                    disp.heights[k] = (old_heights[k] as f64 * (1.0 - t) + target * t) as f32;
                }
                SculptMode::Noise => disp.heights[k] += (hash_noise(folds[k].point(grid.base[k]), brush.seed) * brush.strength * w) as f32,
                SculptMode::PaintAlpha | SculptMode::PaintLayer => disp.alphas[k] = (disp.alphas[k] as f64 + brush.strength * w).clamp(0.0, 1.0) as f32,
                SculptMode::EraseAlpha => disp.alphas[k] = (disp.alphas[k] as f64 - brush.strength * w).clamp(0.0, 1.0) as f32,
                SculptMode::Terrace => {
                    let step = brush.terrace_step.max(1e-3) as f32;
                    let h = old_heights[k];
                    let t = (w * brush.strength.clamp(0.0, 16.0) / 16.0).clamp(0.0, 1.0) as f32;
                    disp.heights[k] = h + ((h / step).round() * step - h) * t;
                }
                SculptMode::Hole | SculptMode::Unhole | SculptMode::AddTile | SculptMode::RemoveTile => {}
            }

            changed = true;
        }
    }

    changed
}

/// Editable terrains among the given nodes (every terrain in the map when `ids` is empty).
pub fn terrain_targets(map: &Map, ids: &[NodeId]) -> Vec<NodeId> {
    let candidates: Vec<NodeId> = if ids.is_empty() { map.terrains().map(|(id, _)| id).collect() } else { ids.to_vec() };
    candidates.into_iter().filter(|id| map.terrain(*id).is_some() && map.is_editable(*id)).collect()
}

/// Nearest terrain hit: (distance, point, terrain).
pub fn terrain_ray_cast(map: &Map, terrains: &[NodeId], ray: &Ray) -> Option<(f64, DVec3, NodeId)> {
    terrains.iter().filter_map(|id| map.terrain(*id).and_then(|t| t.ray_cast(ray)).map(|(d, p)| (d, p, *id))).min_by(|a, b| a.0.total_cmp(&b.0))
}

/// Squared distance along x and z from `p` to the ground of `t`, 0 over it.
fn ground_gap(t: &td_geom::Terrain, p: DVec3) -> f64 {
    let size = DVec3::new(t.cells()[0] as f64, 0.0, t.cells()[1] as f64) * t.cell_size;
    let d = (t.origin - p).max(p - t.origin - size).max(DVec3::ZERO);
    d.x * d.x + d.z * d.z
}

/// The terrain a tile at `p` belongs to: the one whose ground lies under `p`, else the nearest one.
pub fn tile_terrain(map: &Map, terrains: &[NodeId], p: DVec3) -> Option<NodeId> {
    let key = |id: &NodeId| map.terrain(*id).map(|t| (ground_gap(t, p), (p.y - t.origin.y).abs(), *id));
    terrains.iter().filter_map(key).min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))).map(|k| k.2)
}

/// Where a tile brush points: the terrain under the ray, or else where the ray meets the ground plane of the terrain
/// it passes closest to, so tiles can be added beside a terrain.
pub fn tile_ray_cast(map: &Map, terrains: &[NodeId], ray: &Ray) -> Option<DVec3> {
    if let Some((_, p, _)) = terrain_ray_cast(map, terrains, ray) {
        return Some(p);
    }

    let on_plane = |id: &NodeId| {
        let t = map.terrain(*id)?;
        let along = (t.origin.y - ray.origin.y) / ray.dir.y;
        let p = ray.origin + ray.dir * along;
        (ray.dir.y.abs() > 1e-9 && along > 0.0).then(|| (ground_gap(t, p), p))
    };
    terrains.iter().filter_map(on_plane).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, p)| p)
}

/// Applies one dab to terrains, mirrored on each by `symmetry`. Returns true if anything changed. Tile modes act on
/// the one terrain [`tile_terrain`] picks and ignore symmetry.
pub fn sculpt_terrains(map: &mut Map, terrains: &[NodeId], center: DVec3, brush: &SculptBrush, symmetry: &Symmetry) -> bool {
    if brush.mode.is_tile() {
        let Some(t) = tile_terrain(map, terrains, center).and_then(|id| map.terrain_mut(id)) else { return false };
        return sculpt_terrains_single(t, center, brush, &Mirror::default());
    }

    let mut changed = false;
    for id in terrains {
        if let Some(t) = map.terrain_mut(*id) {
            let mirror = symmetry.for_terrain(t);
            changed |= sculpt_terrains_single(t, center, brush, &mirror);
        }
    }

    changed
}

/// One dab on one terrain, and its images across `mirror`.
pub fn sculpt_terrains_single(t: &mut td_geom::Terrain, center: DVec3, brush: &SculptBrush, mirror: &Mirror) -> bool {
    let r = brush.radius;
    let soft = (brush.strength.clamp(0.0, 16.0) / 16.0).clamp(0.0, 1.0);
    match brush.mode {
        SculptMode::Raise => t.raise(center, r, brush.strength, mirror),
        SculptMode::Lower => t.raise(center, r, -brush.strength, mirror),
        SculptMode::Smooth => t.smooth(center, r, soft, mirror),
        SculptMode::Flatten => t.flatten(center, r, brush.flatten_height, soft, mirror),
        SculptMode::Noise => t.add_noise(center, r, brush.strength, brush.seed, mirror),
        SculptMode::Terrace => t.terrace(center, r, brush.terrace_step, soft, mirror),
        SculptMode::PaintLayer | SculptMode::PaintAlpha => t.paint_layer(center, r, brush.layer as usize, brush.strength.min(1.0), mirror),
        SculptMode::EraseAlpha => t.paint_layer(center, r, 0, brush.strength.min(1.0), mirror),
        SculptMode::Hole => t.set_holes(center, r, true, mirror),
        SculptMode::Unhole => t.set_holes(center, r, false, mirror),
        SculptMode::AddTile => t.add_tile(center),
        SculptMode::RemoveTile => t.remove_tile(center),
    }
}

/// A boundary vertex of a displacement: (face slot, grid index, base position, position, normal).
type SeamVertex = (usize, usize, DVec3, DVec3, DVec3);

/// Makes coincident edge and corner vertices of neighbouring displacements meet at their average position.
pub fn sew(map: &mut Map, faces: &[(NodeId, usize)]) -> usize {
    let key = |p: DVec3| ((p.x * 16.0).round() as i64, (p.y * 16.0).round() as i64, (p.z * 16.0).round() as i64);
    let mut groups: HashMap<(i64, i64, i64), Vec<SeamVertex>> = HashMap::new();
    for (fi, (id, face)) in faces.iter().enumerate() {
        let Some(b) = map.brush(*id) else { continue };
        let Some(grid) = displacement::grid(b, *face) else { continue };
        let normal = b.faces[*face].plane.normal;
        let n = grid.size;
        for k in 0..n * n {
            let (i, j) = (k % n, k / n);
            if i == 0 || j == 0 || i == n - 1 || j == n - 1 {
                groups.entry(key(grid.base[k])).or_default().push((fi, k, grid.base[k], grid.positions[k], normal));
            }
        }
    }

    let mut sewn = 0;
    for entries in groups.values().filter(|e| e.len() > 1) {
        let avg = entries.iter().map(|e| e.3).sum::<DVec3>() / entries.len() as f64;
        for (fi, k, base, _, normal) in entries {
            let (id, face) = faces[*fi];
            if let Some(d) = map.brush_mut(id).and_then(|b| b.faces[face].data.disp.as_mut()) {
                d.heights[*k] = (avg - *base).dot(*normal) as f32;
            }
        }

        sewn += 1;
    }

    sewn
}

/// Vertex paint on regular faces: blends face corner colors towards `color` around `center`.
pub fn paint_vertices(map: &mut Map, brushes: &[NodeId], center: DVec3, radius: f64, color: [f32; 4], strength: f64) -> bool {
    let mut changed = false;
    for id in brushes {
        let Some(b) = map.brush(*id).cloned() else { continue };
        let Some(bm) = map.brush_mut(*id) else { continue };
        for (fi, f) in b.faces.iter().enumerate() {
            if f.data.disp.is_some() {
                continue;
            }

            for (k, vi) in f.indices.iter().enumerate() {
                let w = falloff((b.vertices[*vi as usize] - center).length(), radius) * strength;
                if w <= 0.0 {
                    continue;
                }

                let face = &mut bm.faces[fi];
                // On blended faces alpha is the blend weight, unpainted means none of the blend material.
                let blended = face.data.props.contains_key(crate::blend::BLEND_MATERIAL);
                if face.data.colors.len() != face.indices.len() {
                    face.data.colors = vec![[1.0, 1.0, 1.0, if blended { 0.0 } else { 1.0 }]; face.indices.len()];
                }

                let c = &mut face.data.colors[k];
                for ch in 0..if blended { 3 } else { 4 } {
                    c[ch] += (color[ch] - c[ch]) * w.clamp(0.0, 1.0) as f32;
                }

                changed = true;
            }
        }
    }

    changed
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::NodeKind;
    use crate::symmetry::{SymmetryAxes, SymmetryCenter};
    use td_core::Aabb;
    use td_geom::{Brush, Terrain};

    fn terrain_map() -> (Map, NodeId, NodeId) {
        let mut m = Map::new();
        let l = m.default_layer();
        let a = m.insert(l, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::new(0.0, -16.0, 0.0), DVec3::new(64.0, 0.0, 64.0)), "m").unwrap()));
        let b = m.insert(l, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::new(64.0, -16.0, 0.0), DVec3::new(128.0, 0.0, 64.0)), "m").unwrap()));
        let top = |m: &Map, id| m.brush(id).unwrap().faces.iter().position(|f| f.plane.normal.y > 0.5).unwrap();
        let faces = vec![(a, top(&m, a)), (b, top(&m, b))];
        assert_eq!(create_displacements(&mut m, &faces, 3), 2);
        (m, a, b)
    }

    #[test]
    fn raise_then_sew_closes_seam() {
        let (mut m, _, _) = terrain_map();
        let faces = displacement_faces(&m, &[]);
        let brush = SculptBrush { mode: SculptMode::Raise, radius: 40.0, strength: 10.0, flatten_height: 0.0, ..Default::default() };
        // Dab near the left brush only, so its edge rises while the neighbour stays flat.
        assert!(sculpt(&mut m, &faces[..1], DVec3::new(60.0, 0.0, 32.0), &brush, &Mirror::NONE));
        let hit = ray_cast(&m, &faces, &Ray::new(DVec3::new(62.0, 100.0, 32.0), DVec3::NEG_Y)).unwrap();
        assert!(hit.1.y > 5.0, "surface raised: {:?}", hit.1);
        assert!(sew(&mut m, &faces) > 0);
        let grids: Vec<_> = faces.iter().map(|(id, f)| displacement::grid(m.brush(*id).unwrap(), *f).unwrap()).collect();
        for (k, p) in grids[0].positions.iter().enumerate() {
            if (grids[0].base[k].x - 64.0).abs() < 1e-6 {
                let other = grids[1].base.iter().position(|q| (*q - grids[0].base[k]).length() < 1e-6).unwrap();
                assert!((grids[1].positions[other] - *p).length() < 1e-4);
            }
        }
    }

    #[test]
    fn alpha_paint_and_smooth() {
        let (mut m, a, _) = terrain_map();
        let faces = displacement_faces(&m, &[a]);
        let paint = SculptBrush { mode: SculptMode::PaintAlpha, radius: 20.0, strength: 0.5, flatten_height: 0.0, ..Default::default() };
        sculpt(&mut m, &faces, DVec3::new(32.0, 0.0, 32.0), &paint, &Mirror::NONE);
        let d = m.brush(a).unwrap().faces[faces[0].1].data.disp.clone().unwrap();
        assert!(d.alphas.iter().any(|v| *v > 0.4) && d.alphas.contains(&0.0));

        let raise = SculptBrush { mode: SculptMode::Raise, radius: 8.0, strength: 16.0, ..paint };
        sculpt(&mut m, &faces, DVec3::new(32.0, 0.0, 32.0), &raise, &Mirror::NONE);
        let peak = |m: &Map| m.brush(a).unwrap().faces[faces[0].1].data.disp.as_ref().unwrap().heights.iter().cloned().fold(0.0f32, f32::max);
        let before = peak(&m);
        let smooth = SculptBrush { mode: SculptMode::Smooth, radius: 32.0, strength: 16.0, ..paint };
        sculpt(&mut m, &faces, DVec3::new(32.0, 0.0, 32.0), &smooth, &Mirror::NONE);
        assert!(peak(&m) < before);
    }

    #[test]
    fn vertex_paint_blends_corners() {
        let mut m = Map::new();
        let l = m.default_layer();
        let id = m.insert(l, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::splat(32.0)), "m").unwrap()));
        assert!(paint_vertices(&mut m, &[id], DVec3::ZERO, 8.0, [1.0, 0.0, 0.0, 1.0], 1.0));
        let b = m.brush(id).unwrap();
        let painted = b.faces.iter().filter(|f| f.data.colors.iter().any(|c| c[1] < 0.5)).count();
        assert_eq!(painted, 3, "the three faces meeting at the origin corner");
    }

    #[test]
    fn vertex_paint_keeps_blend_weights_of_blended_faces() {
        let mut m = Map::new();
        let l = m.default_layer();
        let id = m.insert(l, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::splat(32.0)), "m").unwrap()));
        let faces: Vec<(NodeId, usize)> = (0..6).map(|f| (id, f)).collect();
        crate::blend::set_blend_material(&mut m, &faces, Some("rock"));
        assert!(paint_vertices(&mut m, &[id], DVec3::ZERO, 8.0, [1.0, 0.0, 0.0, 1.0], 1.0));
        let b = m.brush(id).unwrap();
        assert!(b.faces.iter().all(|f| f.data.colors.iter().all(|c| c[3] == 0.0)), "tinting does not switch faces to the blend material");
        assert!(b.faces.iter().any(|f| f.data.colors.iter().any(|c| c[1] < 0.5)));
    }

    #[test]
    fn layer_paint_on_unpainted_displacement() {
        let (mut m, a, _) = terrain_map();
        let faces = displacement_faces(&m, &[a]);
        let paint = SculptBrush { mode: SculptMode::PaintLayer, radius: 20.0, strength: 0.5, ..Default::default() };
        assert!(sculpt(&mut m, &faces, DVec3::new(32.0, 0.0, 32.0), &paint, &Mirror::NONE));
        let d = m.brush(a).unwrap().faces[faces[0].1].data.disp.clone().unwrap();
        assert!(d.alphas.iter().any(|v| *v > 0.4));
    }

    pub(crate) const P: f64 = 21.5;
    pub(crate) const X: Mirror = Mirror { x: Some(P), z: None };

    pub(crate) fn mirror_x(p: DVec3) -> DVec3 {
        DVec3::new(2.0 * P - p.x, p.y, p.z)
    }

    /// Base position, height and alpha of every vertex of a displacement.
    pub(crate) fn surface(m: &Map, (id, face): (NodeId, usize)) -> Vec<(DVec3, f32, f32)> {
        let b = m.brush(id).unwrap();
        let d = b.faces[face].data.disp.as_ref().unwrap();
        let base = displacement::grid(b, face).unwrap().base;
        base.iter().enumerate().map(|(k, p)| (*p, d.heights[k], d.alphas.get(k).copied().unwrap_or(0.0))).collect()
    }

    pub(crate) fn assert_mirrored(m: &Map, a: (NodeId, usize), b: (NodeId, usize), what: &str) {
        let (sa, sb) = (surface(m, a), surface(m, b));
        assert_eq!(sa.len(), sb.len(), "{what}");
        for (p, h, alpha) in &sa {
            let (_, hb, ab) = sb.iter().find(|(q, ..)| (*q - mirror_x(*p)).length() < 1e-6).unwrap_or_else(|| panic!("{what}: no mirror of {p}"));
            assert!((h - hb).abs() < 1e-4 && (alpha - ab).abs() < 1e-5, "{what} at {p}: {h} and {alpha}, mirrored {hb} and {ab}");
        }
    }

    /// A displaced face of the brush `a` facing along `normal`, and its mirror image across x = P, with mirrored slanted
    /// start heights.
    pub(crate) fn mirrored_pair(a: Aabb, normal: DVec3) -> (Map, [(NodeId, usize); 2]) {
        let mut m = Map::new();
        let l = m.default_layer();
        let mut place = |bounds: Aabb, normal: DVec3| {
            let brush = Brush::from_aabb(&bounds, "m").unwrap();
            let face = brush.faces.iter().position(|f| f.plane.normal.dot(normal) > 0.5).unwrap();
            (m.insert(l, NodeKind::Brush(brush)), face)
        };
        let faces = [place(a, normal), place(Aabb::new(mirror_x(a.min), mirror_x(a.max)), DVec3::new(-normal.x, normal.y, normal.z))];
        assert_eq!(create_displacements(&mut m, &faces, 3), 2);
        let g = |p: DVec3| (0.07 * (p.x + 50.0) + 0.031 * (p.z - 40.0) + 0.0021 * p.y * p.z) as f32;
        for (k, face) in faces.into_iter().enumerate() {
            let base = displacement::grid(m.brush(face.0).unwrap(), face.1).unwrap().base;
            let d = m.brush_mut(face.0).unwrap().faces[face.1].data.disp.as_mut().unwrap();
            for (h, p) in d.heights.iter_mut().zip(base) {
                *h = g(if k == 0 { p } else { mirror_x(p) });
            }
        }

        (m, faces)
    }

    #[test]
    fn mirrored_dabs_on_displacements_land_on_the_mirrored_brush() {
        let floor = Aabb::new(DVec3::new(P - 64.0, -4.0, 40.0), DVec3::new(P, 12.0, 104.0));
        // More than the radius from the plane, so only the mirrored dab reaches the second brush.
        let dab = DVec3::new(P - 37.0, 12.0, 61.0);
        for mode in [SculptMode::Raise, SculptMode::Smooth, SculptMode::Noise, SculptMode::Terrace, SculptMode::PaintAlpha] {
            let brush = SculptBrush { mode, radius: 30.0, strength: 6.0, terrace_step: 1.5, seed: 7, ..Default::default() };
            let (mut alone, faces) = mirrored_pair(floor, DVec3::Y);
            let start = surface(&alone, faces[1]);
            assert!(sculpt(&mut alone, &faces, dab, &brush, &Mirror::NONE));
            assert_eq!(surface(&alone, faces[1]), start, "{mode:?} without a mirror");

            let (mut m, faces) = mirrored_pair(floor, DVec3::Y);
            assert!(sculpt(&mut m, &faces, dab, &brush, &X));
            assert_ne!(surface(&m, faces[1]), start, "{mode:?}");
            assert_mirrored(&m, faces[0], faces[1], &format!("{mode:?}"));
        }
    }

    #[test]
    fn a_displacement_dab_on_the_plane_counts_once() {
        let floor = Aabb::new(DVec3::new(P - 64.0, -4.0, 40.0), DVec3::new(P, 12.0, 104.0));
        let raise = SculptBrush { mode: SculptMode::Raise, radius: 30.0, strength: 6.0, ..Default::default() };
        let dab = DVec3::new(P, 12.0, 61.0);
        let (mut once, faces) = mirrored_pair(floor, DVec3::Y);
        let (mut m, _) = mirrored_pair(floor, DVec3::Y);
        assert!(sculpt(&mut once, &faces, dab, &raise, &Mirror::NONE));
        assert!(sculpt(&mut m, &faces, dab, &raise, &X));
        for face in faces {
            for ((p, h, _), (_, want, _)) in surface(&m, face).iter().zip(surface(&once, face)) {
                assert!((h - want).abs() < 1e-5, "at {p}: {h}, one dab gives {want}");
            }
        }

        assert_mirrored(&m, faces[0], faces[1], "on the plane");
    }

    #[test]
    fn flatten_on_a_wall_flattens_the_facing_wall_to_the_mirrored_height() {
        let wall = Aabb::new(DVec3::new(P - 56.0, 12.0, 40.0), DVec3::new(P - 40.0, 76.0, 104.0));
        let (mut m, faces) = mirrored_pair(wall, DVec3::X);
        // Ten units out of the wall facing +X, like a stroke started on a vertex raised by ten.
        let flatten = SculptBrush { mode: SculptMode::Flatten, radius: 40.0, strength: 16.0, flatten_height: P - 40.0 + 10.0, ..Default::default() };
        assert!(sculpt(&mut m, &faces, DVec3::new(P - 38.0, 44.0, 72.0), &flatten, &X));
        assert_mirrored(&m, faces[0], faces[1], "walls");
        for (face, x) in [(faces[0], P - 40.0), (faces[1], P + 40.0)] {
            let (_, h, _) = surface(&m, face).into_iter().find(|(p, ..)| (*p - DVec3::new(x, 44.0, 72.0)).length() < 1e-6).unwrap();
            assert!((h - 10.0).abs() < 0.5, "the wall at x {x} is {h} out");
        }
    }

    /// Two 17 by 9 cell terrains with slanted heights, mirror images of each other across x = 0.
    fn twin_terrains() -> (Map, NodeId, NodeId) {
        let mut west = Terrain::new(DVec3::new(-300.5, 12.0, 40.25), [18, 10], 16.0, "m");
        for (k, h) in west.heights.iter_mut().enumerate() {
            *h = (k % 18) as f32 * 0.7 + (k / 18) as f32 * 0.3 + ((k * 7) % 5) as f32 * 0.1;
        }

        let mut east = Terrain::new(DVec3::new(28.5, 12.0, 40.25), [18, 10], 16.0, "m");
        east.heights = flip_x(&west.heights);
        let mut m = Map::new();
        let l = m.default_layer();
        let (w, e) = (m.insert(l, NodeKind::Terrain(west)), m.insert(l, NodeKind::Terrain(east)));
        (m, w, e)
    }

    fn flip_x(heights: &[f32]) -> Vec<f32> {
        (0..heights.len()).map(|k| heights[k / 18 * 18 + 17 - k % 18]).collect()
    }

    fn raised(m: &Map, start: &Map, id: NodeId) -> Vec<f32> {
        m.terrain(id).unwrap().heights.iter().zip(&start.terrain(id).unwrap().heights).map(|(a, b)| a - b).collect()
    }

    fn assert_close(got: &[f32], want: &[f32], what: &str) {
        for (k, (g, w)) in got.iter().zip(want).enumerate() {
            assert!((g - w).abs() < 1e-4, "{what}: vertex {k} is {g}, want {w}");
        }
    }

    #[test]
    fn terrains_mirror_around_their_own_middle_or_one_world_plane() {
        let raise = SculptBrush { mode: SculptMode::Raise, radius: 40.0, strength: 5.0, ..Default::default() };
        // East of the middle of the west terrain at x -164.5, by more than the radius.
        let dab = DVec3::new(-110.5, 20.0, 90.25);
        let west_half_still = |d: &[f32]| (0..d.len()).filter(|k| k % 18 < 9).all(|k| d[k] == 0.0);
        let (start, w, e) = twin_terrains();

        let mut m = start.clone();
        assert!(sculpt_terrains(&mut m, &[w, e], dab, &raise, &Symmetry::OFF));
        let (dw, de) = (raised(&m, &start, w), raised(&m, &start, e));
        assert!(dw.iter().any(|d| *d > 4.0) && west_half_still(&dw) && de.iter().all(|d| *d == 0.0), "off: {dw:?}");

        let mut m = start.clone();
        let own = Symmetry { axes: SymmetryAxes::X, ..Symmetry::OFF };
        assert!(sculpt_terrains(&mut m, &[w, e], dab, &raise, &own));
        let (dw, de) = (raised(&m, &start, w), raised(&m, &start, e));
        assert!(dw.iter().any(|d| *d > 4.0) && !west_half_still(&dw), "terrain center: {dw:?}");
        assert_close(&dw, &flip_x(&dw), "the west terrain around its middle");
        assert!(de.iter().all(|d| *d == 0.0), "the east terrain mirrors around its own middle, out of reach");

        let mut m = start.clone();
        let world = Symmetry { axes: SymmetryAxes::X, center: SymmetryCenter::World, ..Symmetry::OFF };
        assert!(sculpt_terrains(&mut m, &[w, e], dab, &raise, &world));
        let (dw, de) = (raised(&m, &start, w), raised(&m, &start, e));
        assert!(dw.iter().any(|d| *d > 4.0) && west_half_still(&dw), "world: {dw:?}");
        assert_close(&de, &flip_x(&dw), "the east terrain across x 0");

        let mut m = start.clone();
        assert!(sculpt_terrains(&mut m, &[w], dab, &raise, &world));
        assert!(raised(&m, &start, e).iter().all(|d| *d == 0.0), "a terrain left out of the stroke stays as it was");
    }

    #[test]
    fn a_tile_beside_two_terrains_grows_only_the_nearest() {
        let (start, w, e) = twin_terrains();
        let ray = Ray::new(DVec3::new(-200.0, 500.0, 0.0), DVec3::NEG_Y);
        let at = tile_ray_cast(&start, &[w, e], &ray).unwrap();
        assert_eq!(at, DVec3::new(-200.0, 12.0, 0.0), "off the terrains the ray meets their ground plane");
        assert_eq!(tile_terrain(&start, &[w, e], at), Some(w));

        let mut m = start.clone();
        let add = SculptBrush { mode: SculptMode::AddTile, ..Default::default() };
        assert!(sculpt_terrains(&mut m, &[w, e], at, &add, &Symmetry::OFF));
        assert_eq!(m.terrain(w).unwrap().cells(), [32, 41]);
        assert_eq!(m.terrain(e), start.terrain(e));
    }
}
