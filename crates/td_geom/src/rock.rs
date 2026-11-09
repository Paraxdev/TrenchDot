//! Procedural rocks: an icosphere pushed in and out by layered 3D noise, chipped flat in a few places and cut flat
//! underneath.

use std::f64::consts::TAU;

use td_core::{Aabb, DVec3};

pub const MAX_DETAIL: u32 = 4;
pub const MAX_FACETS: u32 = 12;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rock {
    pub seed: u32,
    /// How far the noise pushes the surface in and out, 0 to 1.
    pub roughness: f64,
    /// Subdivision level of the mesh, which also adds finer noise, 1 to `MAX_DETAIL`.
    pub detail: u32,
    /// Flat chipped faces cut into the surface.
    pub facets: u32,
    /// Share of the lower half cut off flat, so the rock sits on the ground, 0 to 1.
    pub flatten: f64,
}

impl Default for Rock {
    fn default() -> Self {
        Self { seed: 1, roughness: 0.5, detail: 3, facets: 5, flatten: 0.3 }
    }
}

impl Rock {
    /// The rock around the origin with a radius near 1, from an icosphere of the given subdivision level.
    /// Triangles wind counter-clockwise seen from outside.
    pub fn surface(&self, level: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let (mut verts, tris) = icosphere(level.clamp(1, MAX_DETAIL));
        let rough = self.roughness.clamp(0.0, 1.0);
        let octaves = self.detail.clamp(1, MAX_DETAIL) + 1;
        let chips = self.chips();
        for v in &mut verts {
            let d = *v;
            let grain = fbm3(d * 4.0 + DVec3::splat(31.7), self.seed.wrapping_add(101), octaves, 0.55);
            let p = d * (1.0 + rough * (0.6 * fbm3(d * 1.3, self.seed, 2, 0.5) + 0.12 * grain)).max(0.2);
            // Chips come last so their faces stay flat. Pulling the point towards the center onto the tightest plane
            // satisfies every plane at once, where projecting along each normal in turn can undo an earlier cut.
            let pull = chips.iter().filter(|(n, _)| n.dot(p) > 0.0).map(|(n, depth)| depth / n.dot(p)).fold(1.0, f64::min);
            *v = p * pull;
        }

        (verts, tris)
    }

    /// The planes of the chipped faces as an outward normal and its distance from the center.
    fn chips(&self) -> Vec<(DVec3, f64)> {
        (0..self.facets.min(MAX_FACETS))
            .map(|i| {
                let z = rand(self.seed, i * 3) * 2.0 - 1.0;
                let a = rand(self.seed, i * 3 + 1) * TAU;
                let r = (1.0 - z * z).sqrt();
                (DVec3::new(r * a.cos(), z, r * a.sin()), 0.6 + 0.3 * rand(self.seed, i * 3 + 2))
            })
            .collect()
    }

    /// Height of the flat base between the lowest and highest point.
    pub fn floor(&self, min_y: f64, max_y: f64) -> f64 {
        min_y + (max_y - min_y) * self.flatten.clamp(0.0, 1.0) * 0.5
    }
}

/// Stretches the points so their bounds fill `bounds`.
pub fn fit(points: &mut [DVec3], bounds: &Aabb) {
    let src = Aabb::from_points(points.iter().copied());
    let size = src.size().max(DVec3::splat(1e-9));
    for p in points {
        *p = bounds.min + (*p - src.min) / size * bounds.size();
    }
}

pub fn icosphere(level: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let t = (1.0 + 5f64.sqrt()) / 2.0;
    let mut verts: Vec<DVec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .into_iter()
    .map(|(x, y, z)| DVec3::new(x, y, z).normalize())
    .collect();
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..level {
        let mut mid = std::collections::HashMap::new();
        let mut split = |a: u32, b: u32, verts: &mut Vec<DVec3>| -> u32 {
            *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                verts.push(((verts[a as usize] + verts[b as usize]) * 0.5).normalize());
                verts.len() as u32 - 1
            })
        };
        tris = tris
            .iter()
            .flat_map(|&[a, b, c]| {
                let (ab, bc, ca) = (split(a, b, &mut verts), split(b, c, &mut verts), split(c, a, &mut verts));
                [[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]
            })
            .collect();
    }

    (verts, tris)
}

fn hash(x: i64, y: i64, z: i64, seed: u32) -> u32 {
    let mut h =
        (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ (z as u32).wrapping_mul(0xcb1a_b31f) ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^ (h >> 15)
}

fn rand(seed: u32, i: u32) -> f64 {
    hash(i as i64, 7, 13, seed) as f64 / u32::MAX as f64
}

/// Seeded gradient noise in roughly -1..1.
pub fn noise3(p: DVec3, seed: u32) -> f64 {
    const GRAD: [DVec3; 12] = [
        DVec3::new(1.0, 1.0, 0.0),
        DVec3::new(-1.0, 1.0, 0.0),
        DVec3::new(1.0, -1.0, 0.0),
        DVec3::new(-1.0, -1.0, 0.0),
        DVec3::new(1.0, 0.0, 1.0),
        DVec3::new(-1.0, 0.0, 1.0),
        DVec3::new(1.0, 0.0, -1.0),
        DVec3::new(-1.0, 0.0, -1.0),
        DVec3::new(0.0, 1.0, 1.0),
        DVec3::new(0.0, -1.0, 1.0),
        DVec3::new(0.0, 1.0, -1.0),
        DVec3::new(0.0, -1.0, -1.0),
    ];
    let i = p.floor();
    let f = p - i;
    let (ix, iy, iz) = (i.x as i64, i.y as i64, i.z as i64);
    let corner = |dx: i64, dy: i64, dz: i64| {
        let g = GRAD[(hash(ix + dx, iy + dy, iz + dz, seed) % 12) as usize];
        g.dot(f - DVec3::new(dx as f64, dy as f64, dz as f64))
    };
    let fade = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (u, v, w) = (fade(f.x), fade(f.y), fade(f.z));
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let x00 = lerp(corner(0, 0, 0), corner(1, 0, 0), u);
    let x10 = lerp(corner(0, 1, 0), corner(1, 1, 0), u);
    let x01 = lerp(corner(0, 0, 1), corner(1, 0, 1), u);
    let x11 = lerp(corner(0, 1, 1), corner(1, 1, 1), u);
    lerp(lerp(x00, x10, v), lerp(x01, x11, v), w)
}

pub fn fbm3(p: DVec3, seed: u32, octaves: u32, gain: f64) -> f64 {
    let (mut sum, mut amp, mut norm, mut q) = (0.0, 1.0, 0.0, p);
    for o in 0..octaves.max(1) {
        sum += noise3(q, seed.wrapping_add(o * 7919)) * amp;
        norm += amp;
        amp *= gain;
        q = q * 2.03 + DVec3::new(17.1, -9.7, 4.3);
    }

    sum / norm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icosphere_is_closed_and_winds_outwards() {
        for level in 0..=2 {
            let (verts, tris) = icosphere(level);
            assert_eq!(tris.len(), 20 * 4usize.pow(level));
            let volume: f64 = tris.iter().map(|[a, b, c]| verts[*a as usize].dot(verts[*b as usize].cross(verts[*c as usize])) / 6.0).sum();
            assert!(volume > 2.5, "level {level} volume {volume}");
        }
    }

    #[test]
    fn chipped_faces_stay_flat() {
        for seed in 0..20 {
            let rock = Rock { seed, roughness: 1.0, facets: MAX_FACETS, ..Rock::default() };
            for p in rock.surface(3).0 {
                for (n, depth) in &rock.chips() {
                    assert!(n.dot(p) <= depth + 1e-6, "seed {seed}: a point sticks out through a chip by {}", n.dot(p) - depth);
                }
            }
        }
    }

    #[test]
    fn rocks_follow_their_seed() {
        let a = Rock::default().surface(2).0;
        assert_eq!(a, Rock::default().surface(2).0);
        assert_ne!(a, Rock { seed: 2, ..Rock::default() }.surface(2).0);
        let smooth = Rock { roughness: 0.0, facets: 0, ..Rock::default() }.surface(2).0;
        assert!(smooth.iter().all(|p| (p.length() - 1.0).abs() < 1e-9));
    }
}
