//! Primitive generators. All shapes but a sloped or bent tunnel fit inside `bounds`, and they use Y as the height axis.

use std::f64::consts::TAU;

use td_core::{Aabb, DMat4, DVec2, DVec3};

use crate::brush::{Brush, FaceData};
use crate::rock::{self, Rock};
use crate::uv::FaceUv;

mod path_stairs;
mod tunnel;

pub use path_stairs::{PathStairs, PathStairsParts, path_stairs};
pub use tunnel::{Toward, Tunnel, TunnelProfile, tunnel, tunnel_thickness};

/// The world axis a round shape (cylinder, cone) runs along.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Axis {
    X,
    #[default]
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    pub fn parse(s: &str) -> Option<Axis> {
        match s.trim().to_ascii_lowercase().as_str() {
            "x" => Some(Axis::X),
            "y" => Some(Axis::Y),
            "z" => Some(Axis::Z),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Axis::X => "X",
            Axis::Y => "Y",
            Axis::Z => "Z",
        }
    }

    /// The bounds to build an upright shape in, and the turn that lays it along this axis so it fills `bounds`.
    pub fn frame(self, bounds: &Aabb) -> (Aabb, DMat4) {
        let c = bounds.center();
        let s = bounds.size() * 0.5;
        let (half, turn) = match self {
            Axis::Y => return (*bounds, DMat4::IDENTITY),
            Axis::X => (DVec3::new(s.y, s.x, s.z), DMat4::from_rotation_z(-std::f64::consts::FRAC_PI_2)),
            Axis::Z => (DVec3::new(s.x, s.z, s.y), DMat4::from_rotation_x(std::f64::consts::FRAC_PI_2)),
        };
        (Aabb::new(c - half, c + half), DMat4::from_translation(c) * turn * DMat4::from_translation(-c))
    }

    /// Lays a brush built in [`Axis::frame`]'s bounds along this axis, with its textures aligned to the new faces.
    pub fn lay(self, brush: Brush, bounds: &Aabb) -> Brush {
        if self == Axis::Y {
            return brush;
        }

        let mut b = brush.transformed(&self.frame(bounds).1, false);
        for f in &mut b.faces {
            f.data.uv = FaceUv::face_aligned(f.plane.normal, f.data.uv.scale);
        }

        b
    }
}

fn hull(points: &[DVec3], material: &str) -> Option<Brush> {
    Brush::from_points(points, &[], material).ok()
}

fn ring(center: DVec2, radius: DVec2, sides: usize, phase: f64) -> Vec<DVec2> {
    (0..sides)
        .map(|i| {
            let a = phase + TAU * i as f64 / sides as f64;
            center + DVec2::new(a.cos() * radius.x, a.sin() * radius.y)
        })
        .collect()
}

fn xz(bounds: &Aabb) -> (DVec2, DVec2) {
    let c = bounds.center();
    let s = bounds.size() * 0.5;
    (DVec2::new(c.x, c.z), DVec2::new(s.x, s.z))
}

pub fn cylinder(bounds: &Aabb, sides: usize, material: &str) -> Option<Brush> {
    let (c, r) = xz(bounds);
    let pts: Vec<DVec3> =
        ring(c, r, sides.max(3), 0.0).into_iter().flat_map(|p| [DVec3::new(p.x, bounds.min.y, p.y), DVec3::new(p.x, bounds.max.y, p.y)]).collect();
    hull(&pts, material)
}

pub fn cone(bounds: &Aabb, sides: usize, material: &str) -> Option<Brush> {
    let (c, r) = xz(bounds);
    let mut pts: Vec<DVec3> = ring(c, r, sides.max(3), 0.0).into_iter().map(|p| DVec3::new(p.x, bounds.min.y, p.y)).collect();
    pts.push(DVec3::new(c.x, bounds.max.y, c.y));
    hull(&pts, material)
}

pub fn spike(bounds: &Aabb, material: &str) -> Option<Brush> {
    cone(bounds, 4, material)
}

/// Wedge sloping down towards +Z.
pub fn wedge(bounds: &Aabb, material: &str) -> Option<Brush> {
    let (a, b) = (bounds.min, bounds.max);
    let pts = [
        DVec3::new(a.x, a.y, a.z),
        DVec3::new(b.x, a.y, a.z),
        DVec3::new(a.x, a.y, b.z),
        DVec3::new(b.x, a.y, b.z),
        DVec3::new(a.x, b.y, a.z),
        DVec3::new(b.x, b.y, a.z),
    ];
    hull(&pts, material)
}

/// UV sphere made of one brush per latitude band.
pub fn sphere(bounds: &Aabb, sides: usize, rings: usize, material: &str) -> Vec<Brush> {
    let c = bounds.center();
    let r = bounds.size() * 0.5;
    let rings = rings.max(2);
    let sides = sides.max(3);
    let band = |lat: f64| -> Vec<DVec3> {
        let y = c.y - r.y * lat.cos();
        let rad = lat.sin();
        ring(DVec2::new(c.x, c.z), DVec2::new(r.x * rad, r.z * rad), sides, 0.0).into_iter().map(|p| DVec3::new(p.x, y, p.y)).collect()
    };
    (0..rings)
        .filter_map(|i| {
            let lat0 = std::f64::consts::PI * i as f64 / rings as f64;
            let lat1 = std::f64::consts::PI * (i + 1) as f64 / rings as f64;
            let mut pts = band(lat0);
            pts.extend(band(lat1));
            hull(&pts, material)
        })
        .collect()
}

/// Ring of segments around Y with a square cross section. `thickness` is the radial wall width.
pub fn pipe(bounds: &Aabb, sides: usize, thickness: f64, material: &str) -> Vec<Brush> {
    arc_segments(bounds, sides, thickness, 0.0, 360.0, material)
}

/// The smallest sweep an arch takes, in degrees.
pub const ARCH_MIN_ANGLE: f64 = 10.0;

/// The sweep an arch really gets, in degrees. A segmental arch (under 180) rises `width / 2 * tan(angle / 4)`, so a
/// sweep that would stand taller than `height` is cut back until it fits.
pub fn arch_sweep(width: f64, height: f64, angle: f64) -> f64 {
    let angle = angle.clamp(ARCH_MIN_ANGLE, 180.0);
    if angle >= 180.0 {
        return 180.0;
    }

    angle.min(4.0 * (2.0 * height / width.max(1e-6)).atan().to_degrees()).max(0.5)
}

/// Radius of a circular arch that spans `width` and sweeps `angle` degrees.
pub fn arch_radius(width: f64, angle: f64) -> f64 {
    width * 0.5 / (angle.to_radians() * 0.5).sin()
}

/// The sweep in degrees of a circular arch of `radius` over `width`, the inverse of [`arch_radius`].
pub fn arch_angle_for_radius(width: f64, radius: f64) -> f64 {
    let half = (width * 0.5 / radius.max(1e-6)).clamp((ARCH_MIN_ANGLE * 0.5).to_radians().sin(), 1.0).asin();
    2.0 * half.to_degrees()
}

/// The thickest wall that leaves an arch an opening. A half ellipse stays under 90% of the smaller of its half width
/// and height, a segmental arch under 90% of how far it rises above the floor.
pub fn arch_thickness_cap(bounds: &Aabb, angle: f64) -> f64 {
    let size = bounds.size();
    let sweep = arch_sweep(size.x, size.y, angle);
    if sweep >= 180.0 {
        return (size.x * 0.5).min(size.y) * 0.9;
    }

    let radius = arch_radius(size.x, sweep);
    (radius - radius * (sweep.to_radians() * 0.5).cos()) * 0.9
}

/// The wall thickness an arch really gets. A segmental arch keeps it between 1 and [`arch_thickness_cap`]. A half
/// ellipse takes any positive thickness as it always has, filling solid past its half width or height, so maps built
/// before the angle existed come out the same.
pub fn arch_thickness(bounds: &Aabb, angle: f64, thickness: f64) -> f64 {
    let size = bounds.size();
    if arch_sweep(size.x, size.y, angle) >= 180.0 {
        return if thickness > 0.0 { thickness } else { 1.0 };
    }

    thickness.max(1.0).min(arch_thickness_cap(bounds, angle))
}

fn arch_brush(quad: [DVec2; 4], bounds: &Aabb, material: &str) -> Option<Brush> {
    let pts: Vec<DVec3> = quad.iter().flat_map(|p| [DVec3::new(p.x, p.y, bounds.min.z), DVec3::new(p.x, p.y, bounds.max.z)]).collect();
    hull(&pts, material)
}

/// Arch in the XY plane spanning the bounds width, depth along Z. At 180 degrees it is a half ellipse as tall as the
/// bounds. Under 180 it is a circular segment whose ends sit on the bounds floor at the left and right edges, and the
/// bounds height only caps how high it rises.
pub fn arch(bounds: &Aabb, segments: usize, thickness: f64, angle: f64, material: &str) -> Vec<Brush> {
    arch_quads(bounds, segments, thickness, angle).into_iter().filter_map(|quad| arch_brush(quad, bounds, material)).collect()
}

/// The cross section of each segment of [`arch`], its two outer corners then its two inner ones.
fn arch_quads(bounds: &Aabb, segments: usize, thickness: f64, angle: f64) -> Vec<[DVec2; 4]> {
    let segments = segments.max(1);
    let size = bounds.size();
    let sweep = arch_sweep(size.x, size.y, angle);
    let thickness = arch_thickness(bounds, angle, thickness);
    if sweep >= 180.0 {
        let rx = size.x * 0.5;
        let ry = size.y;
        let center = DVec2::new(bounds.center().x, bounds.min.y);
        return (0..segments)
            .map(|i| {
                let a0 = std::f64::consts::PI * i as f64 / segments as f64;
                let a1 = std::f64::consts::PI * (i + 1) as f64 / segments as f64;
                let outer = |a: f64| center + DVec2::new(a.cos() * rx, a.sin() * ry);
                let inner = |a: f64| center + DVec2::new(a.cos() * (rx - thickness).max(0.0), a.sin() * (ry - thickness).max(0.0));
                [outer(a0), outer(a1), inner(a0), inner(a1)]
            })
            .collect();
    }

    let half = sweep.to_radians() * 0.5;
    let radius = arch_radius(size.x, sweep);
    let drop = radius * half.cos();
    let center = DVec2::new(bounds.center().x, bounds.min.y - drop);
    let inner_radius = radius - thickness;
    // The inner arc runs between where its circle meets the floor, so the ends are cut flat instead of radially,
    // which would dip under the floor.
    let inner_half = ((inner_radius * inner_radius - drop * drop).max(0.0).sqrt() / inner_radius).asin();
    let angle_at = |i: usize, half: f64| std::f64::consts::FRAC_PI_2 + half - 2.0 * half * i as f64 / segments as f64;
    let on = |r: f64, a: f64| center + DVec2::new(a.cos() * r, a.sin() * r);
    let inner = |i: usize| on(inner_radius, angle_at(i, inner_half));
    (0..segments).map(|i| [on(radius, angle_at(i, half)), on(radius, angle_at(i + 1, half)), inner(i), inner(i + 1)]).collect()
}

pub fn torus(bounds: &Aabb, sides: usize, thickness: f64, material: &str) -> Vec<Brush> {
    pipe(bounds, sides, thickness, material)
}

fn arc_segments(bounds: &Aabb, sides: usize, thickness: f64, start_deg: f64, end_deg: f64, material: &str) -> Vec<Brush> {
    let (c, r) = xz(bounds);
    let sides = sides.max(3);
    let span = (end_deg - start_deg).to_radians();
    let inner_r = (r - DVec2::splat(thickness)).max(DVec2::ZERO);
    (0..sides)
        .filter_map(|i| {
            let a0 = start_deg.to_radians() + span * i as f64 / sides as f64;
            let a1 = start_deg.to_radians() + span * (i + 1) as f64 / sides as f64;
            let at = |a: f64, rad: DVec2| c + DVec2::new(a.cos() * rad.x, a.sin() * rad.y);
            let quad = [at(a0, r), at(a1, r), at(a0, inner_r), at(a1, inner_r)];
            let pts: Vec<DVec3> = quad.iter().flat_map(|p| [DVec3::new(p.x, bounds.min.y, p.y), DVec3::new(p.x, bounds.max.y, p.y)]).collect();
            hull(&pts, material)
        })
        .collect()
}

/// Staircase rising towards -Z (Godot forward).
pub fn stairs(bounds: &Aabb, steps: usize, material: &str) -> Vec<Brush> {
    let steps = steps.max(1);
    let size = bounds.size();
    let step_h = size.y / steps as f64;
    let step_d = size.z / steps as f64;
    (0..steps)
        .filter_map(|i| {
            let min = DVec3::new(bounds.min.x, bounds.min.y, bounds.max.z - step_d * (i + 1) as f64);
            let max = DVec3::new(bounds.max.x, bounds.min.y + step_h * (i + 1) as f64, bounds.max.z - step_d * i as f64);
            Brush::from_aabb(&Aabb::new(min, max), material).ok()
        })
        .collect()
}

/// Spiral staircase of wedge shaped steps around the bounds' vertical axis, `turns` full turns high.
pub fn spiral_stairs(bounds: &Aabb, steps: usize, inner_radius: f64, turns: f64, material: &str) -> Vec<Brush> {
    let steps = steps.max(2);
    let c = bounds.center();
    let outer = bounds.size().x.min(bounds.size().z) * 0.5;
    let step_h = bounds.size().y / steps as f64;
    let arc = TAU * turns / steps as f64;
    (0..steps)
        .filter_map(|i| {
            let a0 = arc * i as f64;
            let a1 = a0 + arc * 1.05;
            let y0 = bounds.min.y + step_h * i as f64;
            let y1 = y0 + step_h;
            let at = |a: f64, r: f64, y: f64| DVec3::new(c.x + a.cos() * r, y, c.z + a.sin() * r);
            let pts = [
                at(a0, inner_radius, y0),
                at(a1, inner_radius, y0),
                at(a0, outer, y0),
                at(a1, outer, y0),
                at(a0, inner_radius, y1),
                at(a1, inner_radius, y1),
                at(a0, outer, y1),
                at(a1, outer, y1),
            ];
            Brush::from_points(&pts, &[], material).ok()
        })
        .collect()
}

/// Low poly rock: the convex hull of a coarse noisy rock filling the bounds.
pub fn rock(bounds: &Aabb, params: &Rock, material: &str) -> Option<Brush> {
    let (mut pts, _) = params.surface(1);
    let b = Aabb::from_points(pts.iter().copied());
    let floor = params.floor(b.min.y, b.max.y);
    for p in &mut pts {
        p.y = p.y.max(floor);
    }

    rock::fit(&mut pts, bounds);
    hull(&pts, material)
}

pub fn default_face(material: &str, normal: DVec3) -> FaceData {
    FaceData::with_normal(material, normal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b() -> Aabb {
        Aabb::new(DVec3::splat(-32.0), DVec3::splat(32.0))
    }

    #[test]
    fn cylinder_faces() {
        let c = cylinder(&b(), 8, "m").unwrap();
        assert_eq!(c.faces.len(), 10);
        c.validate().unwrap();
    }

    #[test]
    fn generators_produce_valid_brushes() {
        cone(&b(), 12, "m").unwrap().validate().unwrap();
        wedge(&b(), "m").unwrap().validate().unwrap();
        for s in sphere(&b(), 12, 6, "m") {
            s.validate().unwrap();
        }

        assert_eq!(pipe(&b(), 12, 8.0, "m").len(), 12);
        assert_eq!(arch(&b(), 8, 8.0, 180.0, "m").len(), 8);
        assert_eq!(stairs(&b(), 4, "m").len(), 4);
        for seed in 0..20 {
            rock(&b(), &Rock { seed, ..Rock::default() }, "m").unwrap().validate().unwrap();
        }
    }

    #[test]
    fn a_cylinder_lies_along_the_axis_it_is_given() {
        let bounds = Aabb::new(DVec3::new(-100.0, 70.0, 20.0), DVec3::new(100.0, 78.0, 28.0));
        for (axis, along) in [(Axis::X, DVec3::X), (Axis::Z, DVec3::Z)] {
            let bounds = if axis == Axis::Z { Aabb::new(DVec3::new(20.0, 70.0, -100.0), DVec3::new(28.0, 78.0, 100.0)) } else { bounds };
            let (local, _) = axis.frame(&bounds);
            let c = axis.lay(cylinder(&local, 10, "m").unwrap(), &bounds);
            c.validate().unwrap();
            let got = c.bounds();
            let inside = got.min.cmpge(bounds.min - 1e-6).all() && got.max.cmple(bounds.max + 1e-6).all();
            let full_length = (got.size().dot(along) - bounds.size().dot(along)).abs() < 1e-6;
            assert!(inside && full_length, "{axis:?} runs the length of its bounds, got {got:?}");
            let caps = c.faces.iter().filter(|f| f.plane.normal.dot(along).abs() > 0.999).count();
            assert_eq!(caps, 2, "{axis:?} has its round ends facing along the axis");
            assert!(c.faces.iter().all(|f| !f.data.uv.is_degenerate_for(f.plane.normal)));
        }

        let (local, _) = Axis::X.frame(&bounds);
        let tip = Axis::X.lay(cone(&local, 8, "m").unwrap(), &bounds);
        assert!(tip.vertices.iter().any(|v| (v.x - 100.0).abs() < 1e-6), "a cone along X points to +X");
    }

    fn old_arch(bounds: &Aabb, segments: usize, thickness: f64) -> Vec<Brush> {
        let size = bounds.size();
        let (rx, ry) = (size.x * 0.5, size.y);
        let center = DVec2::new(bounds.center().x, bounds.min.y);
        (0..segments)
            .filter_map(|i| {
                let a0 = std::f64::consts::PI * i as f64 / segments as f64;
                let a1 = std::f64::consts::PI * (i + 1) as f64 / segments as f64;
                let outer = |a: f64| center + DVec2::new(a.cos() * rx, a.sin() * ry);
                let inner = |a: f64| center + DVec2::new(a.cos() * (rx - thickness).max(0.0), a.sin() * (ry - thickness).max(0.0));
                let quad = [outer(a0), outer(a1), inner(a0), inner(a1)];
                let pts: Vec<DVec3> = quad.iter().flat_map(|p| [DVec3::new(p.x, p.y, bounds.min.z), DVec3::new(p.x, p.y, bounds.max.z)]).collect();
                hull(&pts, "m")
            })
            .collect()
    }

    fn arch_bounds() -> Aabb {
        Aabb::new(DVec3::new(100.0, 40.0, -20.0), DVec3::new(228.0, 140.0, 20.0))
    }

    #[test]
    fn a_half_circle_arch_is_unchanged() {
        for bounds in [b(), arch_bounds()] {
            for thickness in [0.5, 12.0, 70.0, 1e6] {
                let (new, old) = (arch(&bounds, 9, thickness, 180.0, "m"), old_arch(&bounds, 9, thickness));
                assert_eq!(new.len(), old.len(), "{thickness}");
                for (n, o) in new.iter().zip(&old) {
                    assert_eq!(n.vertices, o.vertices, "{thickness}");
                }
            }
        }

        assert_eq!(arch(&b(), 8, 8.0, 400.0, "m")[0].vertices, arch(&b(), 8, 8.0, 180.0, "m")[0].vertices, "angles clamp to 180");
    }

    #[test]
    fn a_segmental_arch_stays_in_bounds_and_stands_on_the_floor() {
        let bounds = arch_bounds();
        let (width, thickness) = (bounds.size().x, 12.0);
        for angle in [30.0, 90.0, 140.0] {
            let brushes = arch(&bounds, 10, thickness, angle, "m");
            assert_eq!(brushes.len(), 10);
            let radius = arch_radius(width, angle);
            let center = DVec2::new(bounds.center().x, bounds.min.y - radius * (angle.to_radians() * 0.5).cos());
            let drop = radius * (angle.to_radians() * 0.5).cos();
            let wall = thickness.min((radius - drop) * 0.9);
            let mut ends = [f64::MAX, f64::MIN];
            for brush in &brushes {
                brush.validate().unwrap();
                for v in &brush.vertices {
                    assert!(v.x >= bounds.min.x - 1e-6 && v.x <= bounds.max.x + 1e-6, "{angle}: x {v:?}");
                    assert!(v.y >= bounds.min.y - 1e-6 && v.y <= bounds.max.y + 1e-6, "{angle}: y {v:?}");
                    assert!(v.z >= bounds.min.z - 1e-6 && v.z <= bounds.max.z + 1e-6);
                    let d = DVec2::new(v.x, v.y).distance(center);
                    assert!((d - radius).abs() < 1e-6 || (d - (radius - wall)).abs() < 1e-6, "{angle}: radius {d} against {radius}");
                    if (v.y - bounds.min.y).abs() < 1e-6 && (d - radius).abs() < 1e-6 {
                        ends = [ends[0].min(v.x), ends[1].max(v.x)];
                    }
                }
            }

            assert!((ends[0] - bounds.min.x).abs() < 1e-6 && (ends[1] - bounds.max.x).abs() < 1e-6, "{angle}: ends {ends:?}");
            for (i, piece) in brushes.iter().enumerate() {
                assert!(brushes[i + 1..].iter().all(|other| !piece.intersects(other)), "{angle}: pieces overlap");
            }

            let thin = brushes.iter().flat_map(|b| b.vertices.clone()).any(|v| (DVec2::new(v.x, v.y).distance(center) - (radius - thickness)).abs() < 1e-6);
            assert_eq!(thin, wall == thickness, "{angle}: the wall is thinned only when the arch is too low for it");
        }
    }

    #[test]
    fn the_arch_radius_follows_the_angle() {
        assert!((arch_radius(64.0, 180.0) - 32.0).abs() < 1e-9);
        assert!((arch_radius(64.0, 90.0) - 32.0 * 2f64.sqrt()).abs() < 1e-9);
        assert!((arch_radius(64.0, 60.0) - 64.0).abs() < 1e-9);
        for angle in [10.0, 45.0, 90.0, 179.0] {
            assert!((arch_angle_for_radius(64.0, arch_radius(64.0, angle)) - angle).abs() < 1e-6);
        }

        assert!((arch_angle_for_radius(64.0, 10.0) - 180.0).abs() < 1e-9, "a radius under half the width is the half circle");
    }

    #[test]
    fn an_arch_never_rises_above_its_bounds() {
        let flat = Aabb::new(DVec3::new(0.0, 0.0, 0.0), DVec3::new(128.0, 20.0, 8.0));
        assert!(arch_sweep(128.0, 20.0, 170.0) < 170.0);
        let top = arch(&flat, 8, 4.0, 170.0, "m").iter().flat_map(|b| b.vertices.clone()).map(|v| v.y).fold(f64::MIN, f64::max);
        assert!((top - 20.0).abs() < 1.0 && top <= 20.0 + 1e-6, "rises {top}");
    }

    /// [`arch`] as it was before [`arch_quads`] was split out of it.
    fn old_angled_arch(bounds: &Aabb, segments: usize, thickness: f64, angle: f64) -> Vec<Brush> {
        let segments = segments.max(1);
        let size = bounds.size();
        let sweep = arch_sweep(size.x, size.y, angle);
        let thickness = arch_thickness(bounds, angle, thickness);
        if sweep >= 180.0 {
            return old_arch(bounds, segments, thickness);
        }

        let half = sweep.to_radians() * 0.5;
        let radius = arch_radius(size.x, sweep);
        let drop = radius * half.cos();
        let center = DVec2::new(bounds.center().x, bounds.min.y - drop);
        let inner_radius = radius - thickness;
        let inner_half = ((inner_radius * inner_radius - drop * drop).max(0.0).sqrt() / inner_radius).asin();
        let angle_at = |i: usize, half: f64| std::f64::consts::FRAC_PI_2 + half - 2.0 * half * i as f64 / segments as f64;
        let on = |r: f64, a: f64| center + DVec2::new(a.cos() * r, a.sin() * r);
        let inner = |i: usize| on(inner_radius, angle_at(i, inner_half));
        (0..segments)
            .filter_map(|i| {
                let quad = [on(radius, angle_at(i, half)), on(radius, angle_at(i + 1, half)), inner(i), inner(i + 1)];
                let pts: Vec<DVec3> = quad.iter().flat_map(|p| [DVec3::new(p.x, p.y, bounds.min.z), DVec3::new(p.x, p.y, bounds.max.z)]).collect();
                hull(&pts, "m")
            })
            .collect()
    }

    #[test]
    fn an_arch_matches_the_old_one() {
        let fractional = Aabb::new(DVec3::new(-37.5, 10.25, 203.0), DVec3::new(91.75, 117.0, 360.5));
        let low = Aabb::new(DVec3::new(-300.0, 4.0, 7.0), DVec3::new(212.5, 40.5, 99.0));
        for bounds in [b(), arch_bounds(), fractional, low] {
            for angle in [10.0, 45.0, 120.0, 179.9, 180.0] {
                for segments in [1, 2, 12] {
                    for thickness in [0.0, 1.0, 16.0, 1e6] {
                        let (new, old) = (arch(&bounds, segments, thickness, angle, "m"), old_angled_arch(&bounds, segments, thickness, angle));
                        assert!(segments == 1 || !old.is_empty());
                        assert_eq!(format!("{new:?}"), format!("{old:?}"), "{bounds:?} {angle} {segments} {thickness}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_arch_thickness_keeps_a_wall_and_an_opening() {
        let bounds = arch_bounds();
        assert!((arch_thickness_cap(&bounds, 180.0) - 57.6).abs() < 1e-9, "90 percent of the 64 unit half width");
        let rise = arch_radius(128.0, 90.0) - 64.0;
        assert!((arch_thickness_cap(&bounds, 90.0) - rise * 0.9).abs() < 1e-9);
        assert!((arch_thickness(&bounds, 90.0, 1e6) - rise * 0.9).abs() < 1e-9);
        assert_eq!(arch_thickness(&bounds, 90.0, 0.5), 1.0);
        let probe = DVec3::new(bounds.center().x, bounds.min.y + 0.5, 0.0);
        for thickness in [0.0, 1e6] {
            let brushes = arch(&bounds, 8, thickness, 90.0, "m");
            assert_eq!(brushes.len(), 8, "{thickness}");
            assert!(brushes.iter().all(|b| b.validate().is_ok() && !b.contains_point(probe)), "{thickness}");
        }

        for thickness in [0.5, 12.0, 1e6] {
            assert_eq!(arch_thickness(&bounds, 180.0, thickness), thickness, "a half ellipse keeps its old thickness");
        }

        assert_eq!(arch_thickness(&bounds, 180.0, 0.0), 1.0);
        assert_eq!(arch(&bounds, 8, 0.0, 180.0, "m").len(), 8, "a zero wall still makes an arch");
        assert!(arch(&bounds, 8, 1e6, 180.0, "m").iter().any(|b| b.contains_point(probe)), "a wall past the half width fills it, as before");
    }
}
