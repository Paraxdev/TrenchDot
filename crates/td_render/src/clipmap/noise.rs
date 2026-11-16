//! Terrain3D's world noise, the hills its Noise background draws past the regions. A port of its backgrounds.glsl,
//! these are the twins of the functions of the same name in clipmap.wgsl.

use glam::{DVec2, DVec3};

/// The most octaves Terrain3D's material offers.
pub const MAX_OCTAVES: u32 = 15;

/// Terrain3D's `world_noise_*` material values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Noise {
    pub scale: f32,
    /// Meters, Terrain3D multiplies it by 10.
    pub height: f32,
    /// x and z in steps of 1024 cells, y in steps of 100 meters.
    pub offset: [f32; 3],
    /// Meters from the eye past which each further step of this length drops an octave.
    pub lod_distance: f32,
    pub max_octaves: u32,
    pub min_octaves: u32,
}

impl Default for Noise {
    fn default() -> Self {
        Self { scale: 5.0, height: 32.0, offset: [0.0; 3], lod_distance: 7500.0, max_octaves: 4, min_octaves: 2 }
    }
}

impl Noise {
    /// The octaves a vertex `dist` meters from the eye sums.
    pub fn octaves(&self, dist: f64) -> u32 {
        let [lo, hi] = [self.min_octaves, self.max_octaves].map(|o| f64::from(o.min(MAX_OCTAVES)));
        let steps = if self.lod_distance > 0.0 { (dist / f64::from(self.lod_distance)).floor() } else { f64::INFINITY };
        (hi - steps).max(lo).min(hi) as u32
    }

    /// The noise height in meters Terrain3D adds at `cells` (vertex grid coordinates), `dist` meters from the eye, over
    /// the ground level blend. `present` says whether a region map location holds a region.
    pub fn height(&self, cells: DVec2, region_size: u32, dist: f64, present: impl Fn([i32; 2]) -> bool) -> f64 {
        let size = f64::from(region_size);
        let uv = cells / size;
        let weight = region_weight(uv, present);
        if weight <= 0.5 {
            return 0.0;
        }

        let [x, y, z] = self.offset.map(f64::from);
        let p = (uv + DVec2::new(x, z) * 1024.0 / size) * f64::from(self.scale) * size / 1024.0 * 0.1;
        let noise = world_noise(p, self.octaves(dist)) * f64::from(self.height) * 10.0 + y * 100.0;
        smoothstep(0.5, 1.0, weight) * noise
    }

    /// The lowest and highest [`Noise::height`] in meters, 0 included.
    pub fn range(&self) -> [f32; 2] {
        let octaves = self.max_octaves.min(MAX_OCTAVES) as i32;
        // world_noise sums octaves that each stay below half the one before.
        let most = 2.0 - 2f32.powi(1 - octaves);
        let lift = self.offset[1] * 100.0;
        let top = most * self.height * 10.0 + lift;
        [lift.min(top).min(0.0), lift.max(top).max(0.0)]
    }
}

fn fract(x: f64) -> f64 {
    x - x.floor()
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Terrain3D's `hashv2`, a value in 0 to 1 per lattice point.
pub fn hashv2(v: DVec2) -> f64 {
    fract(1e4 * 17f64.mul_add(v.x, v.y * 0.1).sin() * (0.1 + v.y.mul_add(13.0, v.x).sin().abs()))
}

/// Inigo Quilez's value noise at `x` with its x and z derivatives in y and z.
pub fn noise2d(x: DVec2) -> DVec3 {
    let f = x - x.floor();
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let du = 30.0 * f * f * (f - 1.0) * (f - 1.0);
    let p = x.floor();
    let a = hashv2(p);
    let b = hashv2(p + DVec2::X);
    let c = hashv2(p + DVec2::Y);
    let d = hashv2(p + DVec2::ONE);
    let (k1, k2) = (b - a, c - a);
    let k3 = d - (b + k2);
    let value = a + k1 * u.x + k2 * u.y + k3 * u.x * u.y;
    DVec3::new(value, du.x * (k1 + k3 * u.y), du.y * (k2 + k3 * u.x))
}

/// `octaves` of [`noise2d`], each at twice the frequency and turned, damped where the sum so far is steep.
pub fn world_noise(mut p: DVec2, octaves: u32) -> f64 {
    let (mut a, mut b, mut d) = (0.0, 1.0, DVec2::ZERO);
    for _ in 0..octaves {
        let n = noise2d(p);
        d += DVec2::new(n.y, n.z);
        a += b * n.x / (1.0 + d.dot(d));
        b *= 0.5;
        p = DVec2::new(0.8 * p.x + 0.6 * p.y, 0.8 * p.y - 0.6 * p.x) * 2.0;
    }

    a
}

/// Terrain3D's `get_noise_region_blend` at `uv` in regions: 1 far from every region, below 0.5 near one, where no noise
/// is drawn.
pub fn region_weight(uv: DVec2, present: impl Fn([i32; 2]) -> bool) -> f64 {
    // The bias matches Terrain3D's shader.
    let g = uv - 0.5011;
    let at = |dx: f64, dz: f64| f64::from(u8::from(present([(g.x + dx).floor() as i32, (g.y + dz).floor() as i32])));
    let (a, b, c, d) = (at(0.0, 1.0), at(1.0, 1.0), at(1.0, 0.0), at(0.0, 0.0));
    let w = DVec2::new(smoothstep(0.0, 1.0, fract(g.x)), smoothstep(0.0, 1.0, fract(g.y)));
    let lerp = |p: f64, q: f64, t: f64| p + (q - p) * t;
    1.0 - lerp(lerp(d, c, w.x), lerp(a, b, w.x), w.y) * 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    // The expected values come from Terrain3D's backgrounds.glsl formulas evaluated in double precision.
    fn close(got: f64, want: f64) -> bool {
        (got - want).abs() < 1e-9
    }

    /// Region [2, -1] alone, off the origin.
    fn one(loc: [i32; 2]) -> bool {
        loc == [2, -1]
    }

    #[test]
    fn hashes_and_value_noise_match_terrain3d() {
        assert_eq!(hashv2(DVec2::ZERO), 0.0);
        for (at, want) in [([1.0, 0.0], 0.7215652831200714), ([-3.0, 2.0], 0.7078389005455392), ([41.0, -17.0], 0.24026056541606522)] {
            assert!(close(hashv2(DVec2::from(at)), want), "{at:?}");
        }

        let cases = [
            ([-2.3, 1.7], [0.47426162958955176, -0.2675757895579845, 0.20155289234366947]),
            ([5.75, -0.4], [0.4475328723374039, 0.11582166035364494, 0.4132831992625663]),
        ];
        for (at, want) in cases {
            let got = noise2d(DVec2::from(at));
            assert!((0..3).all(|i| close(got[i], want[i])), "{at:?}: {got}");
        }

        // On a lattice point the noise is that point's hash and flat.
        assert_eq!(noise2d(DVec2::new(-3.0, 2.0)), DVec3::new(hashv2(DVec2::new(-3.0, 2.0)), 0.0, 0.0));
        // The derivatives are the slopes of the value.
        let (at, e) = (DVec2::new(5.75, -0.4), 1e-6);
        let slope = |step: DVec2| (noise2d(at + step).x - noise2d(at - step).x) / (2.0 * e);
        let n = noise2d(at);
        assert!((slope(DVec2::X * e) - n.y).abs() < 1e-6 && (slope(DVec2::Y * e) - n.z).abs() < 1e-6);
    }

    #[test]
    fn world_noise_sums_its_octaves_like_terrain3d() {
        let at = DVec2::new(-2.3, 1.7);
        assert_eq!(world_noise(at, 0), 0.0);
        assert_eq!(world_noise(at, 1), noise2d(at).x / (1.0 + noise2d(at).y.powi(2) + noise2d(at).z.powi(2)));
        assert!(close(world_noise(at, 2), 0.5666166895944642));
        assert!(close(world_noise(at, 4), 0.6298967921465936));
        assert!(close(world_noise(DVec2::new(13.1, -7.9), 1), 0.09827208564831261));
    }

    #[test]
    fn octaves_drop_one_per_lod_distance_down_to_the_minimum() {
        let noise = Noise { lod_distance: 600.0, max_octaves: 5, min_octaves: 2, ..Default::default() };
        let got = [0.0, 599.9, 600.0, 1799.0, 1800.0, 2400.0, 1e7].map(|d| noise.octaves(d));
        assert_eq!(got, [5, 5, 4, 3, 2, 2, 2]);
        let none = Noise { lod_distance: 0.0, ..noise };
        assert_eq!(none.octaves(0.0), 2, "no lod distance is past every step");
        let wild = Noise { max_octaves: 99, min_octaves: 40, ..noise };
        assert_eq!((wild.octaves(0.0), wild.octaves(1e7)), (MAX_OCTAVES, MAX_OCTAVES));
        let crossed = Noise { max_octaves: 1, min_octaves: 3, ..noise };
        assert_eq!(crossed.octaves(0.0), 1, "a minimum above the maximum gives the maximum");
    }

    #[test]
    fn the_weight_is_below_half_inside_a_region_and_one_far_from_all() {
        let block = |loc: [i32; 2]| (2..=3).contains(&loc[0]) && (-1..=0).contains(&loc[1]);
        assert_eq!(region_weight(DVec2::new(3.1, 0.2), block), -1.0);
        let center = region_weight(DVec2::new(2.5, -0.5), one);
        assert!(center < -0.99, "{center}");
        assert!(region_weight(DVec2::new(2.6, -0.3), one) < 0.0);
        assert_eq!(region_weight(DVec2::new(-7.2, 5.9), one), 1.0);
        assert_eq!(region_weight(DVec2::new(2.6, -0.3), |_| false), 1.0);
        // Half way along both axes of the cell above the region, a quarter of the blend is the region's.
        assert!(close(region_weight(DVec2::new(3.0011, -0.9989), one), 0.5));
    }

    #[test]
    fn heights_match_terrain3d_and_vanish_at_the_regions() {
        let noise = Noise { scale: 3.5, height: 20.0, offset: [0.25, -0.125, -0.5], lod_distance: 600.0, max_octaves: 5, min_octaves: 2 };
        let far = DVec2::new(-900.0, 1300.0);
        assert!(close(noise.height(far, 256, 100.0, one), 38.576334703831705));
        assert!(close(noise.height(far, 256, 1300.0, one), 32.07507154863993), "two octaves fewer");
        assert_eq!(noise.height(DVec2::new(2.6, -0.3) * 256.0, 256, 0.0, one), 0.0, "inside the region");
        let edge = DVec2::new(3.0011, -0.9989) * 256.0;
        assert!(noise.height(edge, 256, 0.0, one).abs() < 1e-9, "at weight 0.5");
        let past = noise.height(edge - DVec2::new(0.0, 32.0), 256, 0.0, one);
        assert!(past != 0.0 && past.abs() < noise.range()[1].max(-noise.range()[0]) as f64, "{past}");
    }

    #[test]
    fn the_range_holds_every_height_and_zero() {
        let noise = Noise { height: 20.0, offset: [0.0, -0.1, 0.0], max_octaves: 3, ..Default::default() };
        assert_eq!(noise.range(), [-10.0, 1.75 * 200.0 - 10.0]);
        let down = Noise { height: -5.0, offset: [0.0, 0.5, 0.0], max_octaves: 1, ..Default::default() };
        assert_eq!(down.range(), [0.0, 50.0]);
        assert_eq!(Noise { max_octaves: 0, ..down }.range(), [0.0, 50.0]);
        assert_eq!(Noise { offset: [0.0; 3], max_octaves: 2, ..down }.range(), [-75.0, 0.0]);
        let sampled = (0..400).map(|k| noise.height(DVec2::new(-3000.0 + 37.0 * k as f64, 900.0 - 11.0 * k as f64), 256, 0.0, |_| false));
        let [lo, hi] = noise.range().map(f64::from);
        assert!(sampled.into_iter().all(|h| (lo..=hi).contains(&h)));
    }
}
