//! Terrain3D region data as the TrenchDot cache holds it, and the grid math Terrain3D uses to address it.
//!
//! Heights are meters, control words are Terrain3D's raw bit fields. Positions are map units unless a function says
//! meters, `Grid::upm` converts.

use td_core::DVec2;

pub const REGION_SIZES: [u32; 6] = [64, 128, 256, 512, 1024, 2048];
pub const LOC_MIN: i32 = -16;
pub const LOC_MAX: i32 = 15;
pub const MIN_VERTEX_SPACING: f64 = 0.25;
pub const MAX_VERTEX_SPACING: f64 = 100.0;
pub const MAX_TEXTURES: u32 = 32;
/// What `add_region_blank` fills a control map with, `enc_auto(true)`.
pub const BLANK_CONTROL: u32 = 1;
/// Color (1, 1, 1, 0.5) through Godot's RGBA8 conversion, which truncates.
pub const BLANK_COLOR: [u8; 4] = [255, 255, 255, 127];

pub type Loc = [i32; 2];

pub fn loc_in_range(loc: Loc) -> bool {
    loc.iter().all(|v| (LOC_MIN..=LOC_MAX).contains(v))
}

/// One control map pixel. Setters mask the value like Terrain3D's `enc_*` functions and leave every other bit as it
/// was, including the reserved ones Terrain3D's own brushes clear.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Control(pub u32);

macro_rules! control_field {
    ($get:ident, $set:ident, $shift:expr, $bits:expr) => {
        pub const fn $get(self) -> u8 {
            ((self.0 >> $shift) & ((1 << $bits) - 1)) as u8
        }

        pub const fn $set(self, value: u8) -> Self {
            let mask = ((1u32 << $bits) - 1) << $shift;
            Self((self.0 & !mask) | (((value as u32) << $shift) & mask))
        }
    };
}

macro_rules! control_flag {
    ($get:ident, $set:ident, $bit:expr) => {
        pub const fn $get(self) -> bool {
            self.0 >> $bit & 1 == 1
        }

        pub const fn $set(self, on: bool) -> Self {
            Self((self.0 & !(1 << $bit)) | ((on as u32) << $bit))
        }
    };
}

impl Control {
    control_field!(base, with_base, 27, 5);
    control_field!(overlay, with_overlay, 22, 5);
    control_field!(blend, with_blend, 14, 8);
    control_field!(uv_angle, with_uv_angle, 10, 4);
    control_field!(uv_scale, with_uv_scale, 7, 3);
    control_field!(reserved, with_reserved, 3, 4);
    control_flag!(hole, with_hole, 2);
    control_flag!(navigation, with_navigation, 1);
    control_flag!(auto, with_auto, 0);
}

/// One Terrain3D region, `size * size` vertices row major (z then x). `color` is RGBA8 or empty when the cache has none.
#[derive(Clone, Debug, PartialEq)]
pub struct Region3D {
    pub loc: Loc,
    pub size: u32,
    pub heights: Vec<f32>,
    pub control: Vec<u32>,
    pub color: Vec<u8>,
}

impl Region3D {
    /// What Terrain3D's `add_region_blank` creates.
    pub fn blank(loc: Loc, size: u32) -> Self {
        let n = size as usize * size as usize;
        Self { loc, size, heights: vec![0.0; n], control: vec![BLANK_CONTROL; n], color: BLANK_COLOR.repeat(n) }
    }

    pub fn check(&self) -> Result<(), String> {
        if !REGION_SIZES.contains(&self.size) {
            return Err(format!("Terrain3D region size {} is not one of {REGION_SIZES:?}", self.size));
        }

        if !loc_in_range(self.loc) {
            return Err(format!("Terrain3D region {:?} is outside {LOC_MIN} to {LOC_MAX}", self.loc));
        }

        let n = self.size as usize * self.size as usize;
        if self.heights.len() != n || self.control.len() != n {
            return Err(format!(
                "Terrain3D region {:?} of size {} needs {n} heights and control words, has {} and {}",
                self.loc,
                self.size,
                self.heights.len(),
                self.control.len()
            ));
        }

        if !self.color.is_empty() && self.color.len() != n * 4 {
            return Err(format!("Terrain3D region {:?} color has {} bytes, expected {}", self.loc, self.color.len(), n * 4));
        }

        Ok(())
    }

    pub fn index(&self, pixel: [u32; 2]) -> usize {
        pixel[1] as usize * self.size as usize + pixel[0] as usize
    }

    pub fn control_at(&self, pixel: [u32; 2]) -> Control {
        Control(self.control[self.index(pixel)])
    }
}

/// The file stem Terrain3D gives a region, `terrain3d-01_02` for (-1, 2). None outside the region map.
pub fn file_stem(loc: Loc) -> Option<String> {
    if !loc_in_range(loc) {
        return None;
    }

    let part = |v: i32| if v < 0 { format!("{v:03}") } else { format!("_{v:02}") };
    Some(format!("terrain3d{}{}", part(loc[0]), part(loc[1])))
}

/// The location in a region file name of any extension, `terrain3d-01_02.res.remap` included.
pub fn parse_file_name(name: &str) -> Option<Loc> {
    let rest = name.strip_prefix("terrain3d")?;
    let (coords, tail) = (rest.get(..6)?, &rest[6..]);
    if !coords.is_ascii() {
        return None;
    }

    if !tail.is_empty() && !tail.starts_with('.') {
        return None;
    }

    let part = |s: &str| -> Option<i32> {
        let (sign, digits) = s.split_at(1);
        if !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }

        let v: i32 = digits.parse().ok()?;
        match sign {
            "-" => Some(-v),
            "_" => Some(v),
            _ => None,
        }
    };

    let loc = [part(&coords[..3])?, part(&coords[3..])?];
    loc_in_range(loc).then_some(loc)
}

/// How a Terrain3D world lays out its vertices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub region_size: u32,
    /// Meters between vertices.
    pub spacing: f64,
    /// Map units per meter.
    pub upm: f64,
}

impl Grid {
    pub fn check(&self) -> Result<(), String> {
        if !REGION_SIZES.contains(&self.region_size) {
            return Err(format!("Terrain3D region size {} is not one of {REGION_SIZES:?}", self.region_size));
        }

        if !(MIN_VERTEX_SPACING..=MAX_VERTEX_SPACING).contains(&self.spacing) {
            return Err(format!("Terrain3D vertex spacing {} is outside {MIN_VERTEX_SPACING} to {MAX_VERTEX_SPACING}", self.spacing));
        }

        if !(self.upm.is_finite() && self.upm > 0.0) {
            return Err(format!("units per meter {} is not a positive number", self.upm));
        }

        Ok(())
    }

    /// Map units between vertices.
    pub fn cell(&self) -> f64 {
        self.spacing * self.upm
    }

    /// The vertex at or before a map unit position on both axes, Terrain3D's `world_to_vgrid`.
    pub fn vgrid_at(&self, x: f64, z: f64) -> [i64; 2] {
        [(x / self.cell()).floor() as i64, (z / self.cell()).floor() as i64]
    }

    pub fn region_of(&self, vgrid: [i64; 2]) -> Loc {
        let size = self.region_size as i64;
        vgrid.map(|v| v.div_euclid(size).clamp(i32::MIN as i64, i32::MAX as i64) as i32)
    }

    pub fn pixel_of(&self, vgrid: [i64; 2]) -> [u32; 2] {
        vgrid.map(|v| v.rem_euclid(self.region_size as i64) as u32)
    }

    pub fn vgrid_of(&self, loc: Loc, pixel: [u32; 2]) -> [i64; 2] {
        let size = self.region_size as i64;
        [loc[0] as i64 * size + pixel[0] as i64, loc[1] as i64 * size + pixel[1] as i64]
    }

    /// Map unit x, z of a vertex.
    pub fn position(&self, vgrid: [i64; 2]) -> DVec2 {
        DVec2::new(vgrid[0] as f64, vgrid[1] as f64) * self.cell()
    }

    /// The vertices from `min` to `max` (map units) widened by `margin` vertices on every side, inclusive.
    pub fn vgrid_rect(&self, min: DVec2, max: DVec2, margin: i64) -> ([i64; 2], [i64; 2]) {
        let lo = min.min(max) / self.cell();
        let hi = min.max(max) / self.cell();
        ([lo.x.floor() as i64 - margin, lo.y.floor() as i64 - margin], [hi.x.ceil() as i64 + margin, hi.y.ceil() as i64 + margin])
    }

    /// The region map locations whose vertices meet a vertex rectangle.
    pub fn regions_in(&self, min: [i64; 2], max: [i64; 2]) -> Vec<Loc> {
        let (lo, hi) = (self.region_of(min), self.region_of(max));
        let mut out = Vec::new();
        for y in lo[1].max(LOC_MIN)..=hi[1].min(LOC_MAX) {
            for x in lo[0].max(LOC_MIN)..=hi[0].min(LOC_MAX) {
                out.push([x, y]);
            }
        }

        out
    }

    /// The region holding a vertex and the vertex's index in it.
    pub fn sample<'a>(&self, regions: impl Fn(Loc) -> Option<&'a Region3D>, vgrid: [i64; 2]) -> Option<(&'a Region3D, usize)> {
        let region = regions(self.region_of(vgrid))?;
        if region.size != self.region_size {
            return None;
        }

        Some((region, region.index(self.pixel_of(vgrid))))
    }
}

/// The material values Terrain3D blends heights with near region edges. Pass one only when the world background is
/// Flat or Noise and the shader has both uniforms, as `get_modified_height` checks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeBlend {
    pub ground_level: f32,
    pub region_blend: f32,
}

/// Terrain3D's `get_surface_height` at a position in meters: bilinear over the four vertices around it, NaN when one
/// is a hole or has no region. Kept in f32 like Terrain3D so both sides agree to the last bit where they can.
pub fn surface_height<'a>(grid: &Grid, edge: Option<EdgeBlend>, regions: impl Fn(Loc) -> Option<&'a Region3D>, x: f32, z: f32) -> f32 {
    let step = grid.spacing as f32;
    let snap = |v: f32| (v / step + 0.5).floor() * step;
    let vgrid = |x: f32, z: f32| [(x / step).floor() as i64, (z / step).floor() as i64];
    let height = |v: [i64; 2]| modified_height(grid, edge, &regions, v);
    let (rx, rz) = (snap(x), snap(z));
    if (x - rx) * (x - rx) + (z - rz) * (z - rz) < 0.0001 {
        return height(vgrid(rx, rz));
    }

    let v00 = vgrid(x, z);
    let h00 = height(v00);
    let h01 = height([v00[0], v00[1] + 1]);
    let h10 = height([v00[0] + 1, v00[1]]);
    let h11 = height([v00[0] + 1, v00[1] + 1]);
    let (x1, y1) = (v00[0] as f32, v00[1] as f32);
    let (x2, y2) = ((v00[0] + 1) as f32, (v00[1] + 1) as f32);
    let (px, py) = (x / step, z / step);
    (h00 * (x2 - px) * (y2 - py) + h01 * (x2 - px) * (py - y1) + h10 * (px - x1) * (y2 - py) + h11 * (px - x1) * (py - y1)) / ((x2 - x1) * (y2 - y1))
}

fn modified_height<'a>(grid: &Grid, edge: Option<EdgeBlend>, regions: &impl Fn(Loc) -> Option<&'a Region3D>, vgrid: [i64; 2]) -> f32 {
    let Some((region, k)) = grid.sample(regions, vgrid) else { return f32::NAN };
    if Control(region.control[k]).hole() {
        return f32::NAN;
    }

    let height = region.heights[k];
    let Some(edge) = edge else { return height };
    let texel = 1.0 / grid.region_size as f32;
    let blend = region_blend(edge.region_blend, regions, vgrid[0] as f32 * texel, vgrid[1] as f32 * texel);
    let s = blend.clamp(0.0, 1.0);
    height + s * s * (3.0 - 2.0 * s) * (edge.ground_level - height)
}

fn region_blend<'a>(region_blend: f32, regions: &impl Fn(Loc) -> Option<&'a Region3D>, u: f32, v: f32) -> f32 {
    let present = |u: f32, v: f32| {
        let loc = [u.floor() as i32, v.floor() as i32];
        if loc_in_range(loc) && regions(loc).is_some() { 1.0 } else { 0.0 }
    };

    // The bias matches Terrain3D's shader.
    let (u, v) = (u - 0.5011, v - 0.5011);
    let a = present(u, v + 1.0);
    let b = present(u + 1.0, v + 1.0);
    let c = present(u + 1.0, v);
    let d = present(u, v);
    let factor = 2.0 + 126.0 * (1.0 - region_blend);
    let weight = |f: f32| {
        let f = f.clamp(1e-8, 1.0 - 1e-8);
        1.0 / (1.0 + (factor * ((1.0 - f) / f).ln()).exp())
    };

    let (wx, wy) = (weight(u - u.floor()), weight(v - v.floor()));
    let lerp = |a: f32, b: f32, t: f32| a + t * (b - a);
    (1.0 - lerp(lerp(d, c, wx), lerp(a, b, wx), wy)) * 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATTERNS: [u32; 2] = [0xA5C3_96E1, !0xA5C3_96E1];

    type Field = (fn(Control) -> u8, fn(Control, u8) -> Control, u32, u8);
    type Flag = (fn(Control) -> bool, fn(Control, bool) -> Control, u32);

    #[test]
    fn every_control_field_round_trips_and_leaves_the_other_bits() {
        let fields: [Field; 6] = [
            (Control::base, Control::with_base, 27, 31),
            (Control::overlay, Control::with_overlay, 22, 31),
            (Control::blend, Control::with_blend, 14, 255),
            (Control::uv_angle, Control::with_uv_angle, 10, 15),
            (Control::uv_scale, Control::with_uv_scale, 7, 7),
            (Control::reserved, Control::with_reserved, 3, 15),
        ];
        for (get, set, shift, max) in fields {
            let mask = (max as u32) << shift;
            for bg in PATTERNS {
                for v in [0, 1, max] {
                    let c = set(Control(bg), v);
                    assert_eq!(get(c), v, "shift {shift} value {v}");
                    assert_eq!(c.0 & !mask, bg & !mask, "shift {shift} touched other bits");
                }

                assert_eq!(get(set(Control(bg), max.wrapping_add(1))), 0, "shift {shift} masks like enc_*");
            }
        }

        let flags: [Flag; 3] =
            [(Control::hole, Control::with_hole, 2), (Control::navigation, Control::with_navigation, 1), (Control::auto, Control::with_auto, 0)];
        for (get, set, bit) in flags {
            for bg in PATTERNS {
                for on in [false, true] {
                    let c = set(Control(bg), on);
                    assert_eq!(get(c), on);
                    assert_eq!(c.0 & !(1 << bit), bg & !(1 << bit));
                }
            }
        }

        let c = Control(0xF8E0_6ADD);
        assert_eq!((c.base(), c.overlay(), c.blend(), c.uv_angle(), c.uv_scale(), c.reserved()), (31, 3, 129, 10, 5, 11));
        assert_eq!((c.hole(), c.navigation(), c.auto()), (true, false, true));
    }

    #[test]
    fn nan_control_patterns_survive_field_edits() {
        for bits in [0x7FC0_0004u32, 0x7F80_0001, 0xFFFF_FFFF, 0xFF80_0003] {
            assert!(f32::from_bits(bits).is_nan());
            let c = Control(bits);
            assert_eq!(c.with_hole(!c.hole()).with_hole(c.hole()).0, bits);
            assert_eq!(c.with_base(c.base()).with_blend(c.blend()).with_auto(c.auto()).0, bits);
            let mut region = Region3D::blank([0, 0], 64);
            region.control[5] = bits;
            assert_eq!(region.clone().control[5], bits);
        }
    }

    #[test]
    fn file_names_follow_terrain3d_both_ways() {
        let cases = [([0, 0], "terrain3d_00_00"), ([-1, 2], "terrain3d-01_02"), ([-16, -16], "terrain3d-16-16"), ([15, 15], "terrain3d_15_15")];
        for (loc, stem) in cases {
            assert_eq!(file_stem(loc).as_deref(), Some(stem));
            for ext in ["", ".res", ".td3r", ".res.remap"] {
                assert_eq!(parse_file_name(&format!("{stem}{ext}")), Some(loc), "{stem}{ext}");
            }
        }

        for loc in [[16, 0], [0, -17], [i32::MIN, 0]] {
            assert_eq!(file_stem(loc), None);
        }

        for junk in [
            "terrain3d_16_00.res",
            "terrain3d-17_00.res",
            "terrain3d_0a_00.res",
            "terrain3d+01_02.res",
            "terrain3d_01_02x.res",
            "terrain3d_01_0",
            "Terrain3d_00_00.res",
            "terrain3d_00_00res",
            "terrain3d_0\u{e9}_0.res",
            "terrain3d__1_00.res",
            "",
        ] {
            assert_eq!(parse_file_name(junk), None, "{junk}");
        }
    }

    #[test]
    fn world_region_and_pixel_math_matches_terrain3d_off_the_origin() {
        for size in REGION_SIZES {
            for spacing in [0.25, 1.0, 2.5] {
                let grid = Grid { region_size: size, spacing, upm: 32.0 };
                let (s, n) = (size as f64, size as i64);
                let m = |meters: f64| meters * 32.0;
                let cases = [
                    (m(-(s + 0.6) * spacing), m((2.0 * s + 3.3) * spacing), [-n - 1, 2 * n + 3], [-2, 2], [size - 1, 3]),
                    (m(-s * spacing), m(-0.01 * spacing), [-n, -1], [-1, -1], [0, size - 1]),
                    (m(0.0), m(s * spacing - 0.001), [0, n - 1], [0, 0], [0, size - 1]),
                ];
                for (x, z, vgrid, loc, pixel) in cases {
                    let v = grid.vgrid_at(x, z);
                    assert_eq!(v, vgrid, "size {size} spacing {spacing} at {x}, {z}");
                    assert_eq!(grid.region_of(v), loc);
                    assert_eq!(grid.pixel_of(v), pixel);
                    assert_eq!(grid.vgrid_of(loc, pixel), v);
                    let p = grid.position(v);
                    assert_eq!(grid.vgrid_at(p.x, p.y), v);
                }
            }
        }
    }

    #[test]
    fn grid_and_region_checks_refuse_what_terrain3d_cannot_hold() {
        let ok = Grid { region_size: 64, spacing: 0.25, upm: 32.0 };
        assert!(ok.check().is_ok());
        assert!(Grid { spacing: 100.0, ..ok }.check().is_ok());
        for bad in [
            Grid { region_size: 100, ..ok },
            Grid { region_size: 4096, ..ok },
            Grid { spacing: 0.24, ..ok },
            Grid { spacing: 100.01, ..ok },
            Grid { spacing: f64::NAN, ..ok },
            Grid { upm: 0.0, ..ok },
            Grid { upm: f64::NAN, ..ok },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }

        let blank = Region3D::blank([-1, 2], 64);
        assert!(blank.check().is_ok());
        assert_eq!((blank.heights[0], blank.control[4095], &blank.color[..4]), (0.0, BLANK_CONTROL, &BLANK_COLOR[..]));
        assert!(Region3D { color: Vec::new(), ..blank.clone() }.check().is_ok());
        assert!(Region3D { size: 100, ..blank.clone() }.check().is_err());
        assert!(Region3D { loc: [16, 0], ..blank.clone() }.check().is_err());
        assert!(Region3D { heights: vec![0.0; 4095], ..blank.clone() }.check().is_err());
        assert!(Region3D { control: vec![0; 4097], ..blank.clone() }.check().is_err());
        assert!(Region3D { color: vec![0; 4096 * 3], ..blank }.check().is_err());
    }

    #[test]
    fn regions_in_a_rect_stop_at_the_region_map() {
        let grid = Grid { region_size: 64, spacing: 1.0, upm: 32.0 };
        assert_eq!(grid.regions_in([-65, 10], [0, 70]), vec![[-2, 0], [-1, 0], [0, 0], [-2, 1], [-1, 1], [0, 1]]);
        assert_eq!(grid.regions_in([5, 5], [6, 6]), vec![[0, 0]]);
        assert_eq!(grid.regions_in([-17 * 64 - 3, 15 * 64], [-16 * 64, 17 * 64]), vec![[-16, 15]]);
        assert!(grid.regions_in([16 * 64, 0], [17 * 64, 3]).is_empty());
    }

    fn slanted(loc: Loc, size: u32) -> Region3D {
        let mut r = Region3D::blank(loc, size);
        for pz in 0..size {
            for px in 0..size {
                r.heights[(pz * size + px) as usize] = 5.0 + 0.375 * px as f32 - 0.25 * pz as f32;
            }
        }

        r
    }

    #[test]
    fn surface_height_matches_terrain3d_on_and_between_vertices_and_at_holes() {
        let grid = Grid { region_size: 64, spacing: 2.5, upm: 32.0 };
        let mut region = slanted([-1, 2], 64);
        region.control[21 * 64 + 11] = Control(BLANK_CONTROL).with_hole(true).0;
        let regions = |loc: Loc| (loc == region.loc).then_some(&region);
        let h = |x: f32, z: f32| surface_height(&grid, None, regions, x, z);
        let at = |px: f32, pz: f32| 5.0 + 0.375 * px - 0.25 * pz;

        assert_eq!(h(-135.0, 370.0), region.heights[20 * 64 + 10]);
        assert!((h(-135.0 + 0.4 * 2.5, 370.0 - 0.7 * 2.5) - at(10.4, 19.3)).abs() < 1e-4);
        assert!((h(-150.0 + 1.25, 360.0 + 0.5) - at(4.5, 16.2)).abs() < 1e-4);

        assert!(h(-135.0 + 1.0, 370.0 + 1.0).is_nan(), "cell touching the hole");
        assert!(h(-142.5 + 1.0, 370.0 + 1.0).is_finite(), "cell west of it");
        assert_eq!(h(-135.0 + 0.009, 370.0), region.heights[20 * 64 + 10], "inside the snap distance");
        assert!(h(-135.0 + 0.011, 370.0).is_nan(), "past the snap distance the hole row joins in");
        assert_eq!(h(-137.5, 372.5), region.heights[21 * 64 + 9]);
        assert!(h(-132.5, 372.5).is_nan(), "on the hole vertex");
        assert!(h(10.0, 370.0).is_nan(), "no region");
    }

    #[test]
    fn surface_height_blends_to_ground_level_at_an_open_region_edge() {
        let grid = Grid { region_size: 256, spacing: 1.0, upm: 32.0 };
        let region = slanted([0, 0], 256);
        let west = slanted([-1, 0], 256);
        let edge = Some(EdgeBlend { ground_level: -3.0, region_blend: 0.25 });
        let alone = |loc: Loc| (loc == [0, 0]).then_some(&region);
        let joined = |loc: Loc| match loc {
            [0, 0] => Some(&region),
            [-1, 0] => Some(&west),
            _ => None,
        };

        let stored = |px: usize, pz: usize| region.heights[pz * 256 + px];
        assert_eq!(surface_height(&grid, edge, alone, 128.0, 128.0), stored(128, 128));
        assert!((surface_height(&grid, edge, alone, 0.0, 128.0) + 3.0).abs() < 1e-5, "open west edge");
        assert!((surface_height(&grid, edge, alone, 0.0, 0.0) + 3.0).abs() < 1e-5, "open corner");
        assert_eq!(surface_height(&grid, edge, joined, 0.0, 128.0), stored(0, 128), "west neighbor closes the edge");
        assert_eq!(surface_height(&grid, None, alone, 0.0, 128.0), stored(0, 128), "no world background");
        let partway = surface_height(&grid, edge, alone, 1.0, 128.0);
        assert!(partway < -3.5 && partway > stored(1, 128) + 0.5, "{partway} lies between the ground level and the stored height");
    }
}
