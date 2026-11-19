//! Tunnels. The cross section is cut into convex 2D pieces, which are then run along the tunnel's length.

use std::f64::consts::FRAC_PI_2;

use td_core::{Aabb, DVec2, DVec3};

use super::{ARCH_MIN_ANGLE, arch_quads, arch_radius, arch_sweep, hull, ring};
use crate::brush::Brush;

/// How a tunnel is cut across its length.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TunnelProfile {
    /// Two walls, with a ceiling and a floor when [`Tunnel`] asks for them.
    #[default]
    Square,
    /// Vertical walls under an arch, with a floor when [`Tunnel`] asks for one.
    Arched,
    /// A full round tube.
    Round,
}

impl TunnelProfile {
    pub const ALL: [TunnelProfile; 3] = [TunnelProfile::Square, TunnelProfile::Arched, TunnelProfile::Round];

    /// The profile a name means and whether it comes with a floor. `open_bottom` and `closed` are the square tunnel's
    /// names from before the floor was a toggle.
    pub fn parse(s: &str) -> Option<(TunnelProfile, bool)> {
        match s.trim().to_ascii_lowercase().as_str() {
            "square" | "open_bottom" => Some((TunnelProfile::Square, false)),
            "closed" => Some((TunnelProfile::Square, true)),
            "arched" => Some((TunnelProfile::Arched, false)),
            "round" => Some((TunnelProfile::Round, true)),
            _ => None,
        }
    }
}

/// Every option of a tunnel. The default is the tunnel the Shape Generator has always made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tunnel {
    pub profile: TunnelProfile,
    /// Segments of an arched tunnel's arch, sides of a round tunnel.
    pub sides: usize,
    /// Wall thickness, kept by [`tunnel_thickness`] so a hollow remains.
    pub thickness: f64,
    /// Degrees an arched tunnel's arch sweeps. 180 is a half ellipse, less is a flatter circular arch that springs
    /// from both faces of the walls, so its crown is thinner than they are.
    pub angle: f64,
    /// A slab as thick as the walls along the bottom of a square or arched tunnel.
    pub floor: bool,
    /// The slab over a square tunnel, without it the tunnel is an open trench.
    pub ceiling: bool,
    /// Two raised walkways along the walls with a sunken channel between them.
    pub walkway: bool,
    /// How far each walkway reaches in from its wall, at most 40% of the opening.
    pub walkway_width: f64,
    /// How high the walkways stand, at most half the clear height.
    pub walkway_height: f64,
}

impl Default for Tunnel {
    fn default() -> Self {
        Self {
            profile: TunnelProfile::default(),
            sides: 12,
            thickness: 16.0,
            angle: 180.0,
            floor: false,
            ceiling: true,
            walkway: false,
            walkway_width: 32.0,
            walkway_height: 16.0,
        }
    }
}

impl Tunnel {
    /// This tunnel with every option brought into the range `bounds` allows, reading back what [`tunnel`] builds.
    pub fn fitted(&self, bounds: &Aabb) -> Tunnel {
        let thickness = tunnel_thickness(bounds, self.thickness);
        let angle = match self.profile {
            TunnelProfile::Arched => arched_sweep(bounds.size(), thickness, self.angle),
            _ => self.angle,
        };
        let walkway = walkways(bounds, self, thickness).0;
        Tunnel { thickness, angle, walkway_width: walkway.x, walkway_height: walkway.y, ..*self }
    }

    /// How thick an arched tunnel's arch is at its crown when it sweeps under 180 degrees.
    pub fn arch_crown(&self, bounds: &Aabb) -> Option<f64> {
        let fitted = self.fitted(bounds);
        (fitted.profile == TunnelProfile::Arched && fitted.angle < 180.0).then(|| {
            let (radius, _, inner_radius) = segmental(bounds.size().x, fitted.thickness, fitted.angle);
            radius - inner_radius
        })
    }
}

/// One convex piece of a tunnel's cross section, in the XY plane.
enum Piece {
    Rect(DVec2, DVec2),
    Poly(Vec<DVec2>),
}

/// The wall thickness a tunnel really gets, at most 40% of the smaller of its width and height so a hollow always
/// remains.
pub fn tunnel_thickness(bounds: &Aabb, thickness: f64) -> f64 {
    let size = bounds.size();
    thickness.max(1.0).min(size.x.min(size.y) * 0.4)
}

/// Tunnel with its length along Z and its cross section in the XY plane of the bounds. The side walls run the full
/// height, the ceiling and floor sit between them, then come the walkways. An arched tunnel stands its walls up to
/// where the arch begins, the arch rising at most `min(width, height) / 2`.
pub fn tunnel(bounds: &Aabb, params: &Tunnel, material: &str) -> Vec<Brush> {
    section(bounds, params).iter().filter_map(|piece| place_straight(piece, bounds, material)).collect()
}

fn section(bounds: &Aabb, params: &Tunnel) -> Vec<Piece> {
    let (a, b) = (bounds.min, bounds.max);
    let t = tunnel_thickness(bounds, params.thickness);
    let left = |top: f64| Piece::Rect(DVec2::new(a.x, a.y), DVec2::new(a.x + t, top));
    let right = |top: f64| Piece::Rect(DVec2::new(b.x - t, a.y), DVec2::new(b.x, top));
    let floor = || Piece::Rect(DVec2::new(a.x + t, a.y), DVec2::new(b.x - t, a.y + t));
    let mut pieces = match params.profile {
        TunnelProfile::Square => {
            let mut out = vec![left(b.y), right(b.y)];
            out.extend(params.ceiling.then(|| Piece::Rect(DVec2::new(a.x + t, b.y - t), DVec2::new(b.x - t, b.y))));
            out.extend(params.floor.then(floor));
            out
        }
        TunnelProfile::Arched => {
            let (spring, quads) = arched_top(bounds, params.sides, t, params.angle);
            let mut out = vec![left(spring), right(spring)];
            out.extend(quads.into_iter().map(|quad| Piece::Poly(quad.to_vec())));
            out.extend(params.floor.then(floor));
            out
        }
        TunnelProfile::Round => {
            let (outer, inner) = round_ring(bounds, params.sides, t);
            let n = outer.len();
            (0..n).map(|i| Piece::Poly(vec![outer[i], outer[(i + 1) % n], inner[i], inner[(i + 1) % n]])).collect()
        }
    };
    if params.walkway {
        pieces.extend(walkways(bounds, params, t).1);
    }

    pieces
}

/// The outer corners of a round tunnel's ring and the corners of its bore, counterclockwise.
fn round_ring(bounds: &Aabb, sides: usize, t: f64) -> (Vec<DVec2>, Vec<DVec2>) {
    let c = DVec2::new(bounds.center().x, bounds.center().y);
    let inner_r = DVec2::new(bounds.size().x * 0.5 - t, bounds.size().y * 0.5 - t);
    let outer = ring(c, DVec2::new(bounds.size().x * 0.5, bounds.size().y * 0.5), sides.max(3), 0.0);
    // Each inner corner sits on the ray from the center through its outer corner, unlike pipe's, so every piece stays
    // convex and inside its own wedge even when few sides span a narrow oval.
    let inner = outer
        .iter()
        .map(|p| {
            let d = *p - c;
            c + d / (d / inner_r).length()
        })
        .collect();
    (outer, inner)
}

/// The width and height a tunnel's walkways really get, and the walkways, left first. In a square or arched tunnel
/// they are boxes against the walls, standing on the floor or the bottom of the box. In a round tunnel they fill the
/// bottom of the bore out to the width, so they follow its curve.
fn walkways(bounds: &Aabb, params: &Tunnel, t: f64) -> (DVec2, [Piece; 2]) {
    let fit = |want: f64, room: f64| want.max(1.0).min(room);
    let (a, b) = (bounds.min, bounds.max);
    if params.profile == TunnelProfile::Round {
        let bore = round_ring(bounds, params.sides, t).1;
        let height = fit(params.walkway_height, bounds.size().y * 0.5 - t);
        let top = bore.iter().map(|p| p.y).fold(f64::INFINITY, f64::min) + height;
        // An odd number of sides is not mirror symmetric, so each side of the bore is found on its own.
        let (left, right) = span_at(&bore, top);
        let width = fit(params.walkway_width, (right - left) * 0.4);
        let under = clip_convex(&bore, DVec2::Y, top);
        let pieces = [Piece::Poly(clip_convex(&under, DVec2::X, left + width)), Piece::Poly(clip_convex(&under, -DVec2::X, width - right))];
        return (DVec2::new(width, height), pieces);
    }

    let base = if params.floor { a.y + t } else { a.y };
    let ceiling = match params.profile {
        TunnelProfile::Arched => arched_top(bounds, params.sides, t, params.angle).0,
        _ if params.ceiling => b.y - t,
        _ => b.y,
    };
    let width = fit(params.walkway_width, (bounds.size().x - 2.0 * t) * 0.4);
    let height = fit(params.walkway_height, (ceiling - base) * 0.5);
    let pieces = [
        Piece::Rect(DVec2::new(a.x + t, base), DVec2::new(a.x + t + width, base + height)),
        Piece::Rect(DVec2::new(b.x - t - width, base), DVec2::new(b.x - t, base + height)),
    ];
    (DVec2::new(width, height), pieces)
}

/// The least and greatest x where the convex polygon `poly` crosses the line at height `y`.
fn span_at(poly: &[DVec2], y: f64) -> (f64, f64) {
    let n = poly.len();
    (0..n)
        .filter_map(|i| {
            let (p, q) = (poly[i], poly[(i + 1) % n]);
            ((p.y - y) * (q.y - y) <= 0.0 && p.y != q.y).then(|| p.x + (q.x - p.x) * ((y - p.y) / (q.y - p.y)))
        })
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| (lo.min(x), hi.max(x)))
}

/// The part of the convex polygon `poly` where `p.dot(normal) <= offset`. Corners within a hair of the line count as on
/// it, so a cut through a corner leaves no sliver of an edge beside it.
fn clip_convex(poly: &[DVec2], normal: DVec2, offset: f64) -> Vec<DVec2> {
    let hair = 1e-9 * normal.length();
    let side = |p: DVec2| {
        let d = p.dot(normal) - offset;
        if d.abs() <= hair { 0.0 } else { d }
    };
    let mut out = Vec::with_capacity(poly.len() + 1);
    for (i, &p) in poly.iter().enumerate() {
        let q = poly[(i + 1) % poly.len()];
        let (dp, dq) = (side(p), side(q));
        if dp <= 0.0 {
            out.push(p);
        }

        if dp * dq < 0.0 {
            out.push(p + (q - p) * (dp / (dp - dq)));
        }
    }

    out
}

/// The sweep in degrees an arched tunnel's arch really gets. Under 180 it is cut back to rise at most half the smaller
/// of the width and height, then raised until the crown is at least 1 thick. When no sweep under 180 does both, the
/// arch is a half ellipse.
fn arched_sweep(size: DVec3, t: f64, angle: f64) -> f64 {
    let (w, cap) = (size.x, size.x.min(size.y) * 0.5);
    let sweep = arch_sweep(w, cap, angle);
    if sweep >= 180.0 {
        return 180.0;
    }

    // The crown is r - sqrt(r^2 - t * (w - t)) thick for an outer radius r, so this is the widest radius leaving 1.
    let widest = (t * (w - t) + 1.0) * 0.5;
    let lo = if widest > w * 0.5 { (2.0 * (w * 0.5 / widest).asin().to_degrees()).max(ARCH_MIN_ANGLE) } else { 180.0 };
    if sweep >= lo {
        sweep
    } else if arch_sweep(w, cap, lo) == lo {
        lo
    } else {
        180.0
    }
}

/// The outer radius, how far its center sits under the springs, and the inner radius of a segmental arch `width` wide
/// that sweeps `sweep` degrees over walls `t` thick.
fn segmental(width: f64, t: f64, sweep: f64) -> (f64, f64, f64) {
    let radius = arch_radius(width, sweep);
    let drop = radius * (sweep.to_radians() * 0.5).cos();
    (radius, drop, (width * 0.5 - t).hypot(drop))
}

/// Where an arched tunnel's walls stop and the cross section of each segment of its arch. At 180 degrees the arch is
/// the half ellipse it always was. Under 180 its inner arc shares the outer one's center and springs from the inner
/// faces of the walls, so the walls end flush under it.
fn arched_top(bounds: &Aabb, segments: usize, t: f64, angle: f64) -> (f64, Vec<[DVec2; 4]>) {
    let (a, b, w) = (bounds.min, bounds.max, bounds.size().x);
    let sweep = arched_sweep(bounds.size(), t, angle);
    if sweep >= 180.0 {
        let spring = b.y - w.min(bounds.size().y) * 0.5;
        return (spring, arch_quads(&Aabb::new(DVec3::new(a.x, spring, a.z), b), segments, t, 180.0));
    }

    let n = segments.max(1);
    let (radius, drop, inner_radius) = segmental(w, t, sweep);
    let spring = b.y - (radius - drop);
    let center = DVec2::new(bounds.center().x, spring - drop);
    let (half, inner_half) = (sweep.to_radians() * 0.5, ((w * 0.5 - t) / inner_radius).asin());
    let at = |r: f64, half: f64, i: usize| {
        let a = FRAC_PI_2 + half - 2.0 * half * i as f64 / n as f64;
        center + DVec2::new(a.cos() * r, a.sin() * r)
    };
    // The ends lie on the springs, written out so the walls meet them without rounding.
    let outer = |i: usize| match i {
        0 => DVec2::new(a.x, spring),
        _ if i == n => DVec2::new(b.x, spring),
        _ => at(radius, half, i),
    };
    let inner = |i: usize| match i {
        0 => DVec2::new(a.x + t, spring),
        _ if i == n => DVec2::new(b.x - t, spring),
        _ => at(inner_radius, inner_half, i),
    };
    (spring, (0..n).map(|i| [outer(i), outer(i + 1), inner(i), inner(i + 1)]).collect())
}

fn place_straight(piece: &Piece, bounds: &Aabb, material: &str) -> Option<Brush> {
    let (near, far) = (bounds.min.z, bounds.max.z);
    match piece {
        Piece::Rect(min, max) => Brush::from_aabb(&Aabb::new(min.extend(near), max.extend(far)), material).ok(),
        Piece::Poly(points) => hull(&points.iter().flat_map(|p| [p.extend(near), p.extend(far)]).collect::<Vec<_>>(), material),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The profile names [`TunnelProfile`] had before the floor and ceiling were toggles.
    const LEGACY: [&str; 4] = ["open_bottom", "closed", "arched", "round"];

    fn build(bounds: &Aabb, profile: TunnelProfile, sides: usize, thickness: f64) -> Vec<Brush> {
        tunnel(bounds, &Tunnel { profile, sides, thickness, ..Tunnel::default() }, "m")
    }

    fn legacy(bounds: &Aabb, name: &str, sides: usize, thickness: f64) -> Vec<Brush> {
        let (profile, floor) = TunnelProfile::parse(name).unwrap();
        tunnel(bounds, &Tunnel { profile, floor, sides, thickness, ..Tunnel::default() }, "m")
    }

    /// [`tunnel`] as it was before the section model, with the half ellipse arch it called written out, for the
    /// legacy profile `name`.
    fn old_tunnel(bounds: &Aabb, name: &str, sides: usize, thickness: f64) -> Vec<Brush> {
        let (a, b) = (bounds.min, bounds.max);
        let t = thickness.max(1.0).min(bounds.size().x.min(bounds.size().y) * 0.4);
        let boxed = |min: DVec3, max: DVec3| Brush::from_aabb(&Aabb::new(min, max), "m").ok();
        let extruded = |quad: [DVec2; 4]| {
            let pts: Vec<DVec3> = quad.iter().flat_map(|p| [DVec3::new(p.x, p.y, a.z), DVec3::new(p.x, p.y, b.z)]).collect();
            Brush::from_points(&pts, &[], "m").ok()
        };
        let left = |top: f64| boxed(a, DVec3::new(a.x + t, top, b.z));
        let right = |top: f64| boxed(DVec3::new(b.x - t, a.y, a.z), DVec3::new(b.x, top, b.z));
        match name {
            "open_bottom" | "closed" => {
                let mut out = vec![left(b.y), right(b.y), boxed(DVec3::new(a.x + t, b.y - t, a.z), DVec3::new(b.x - t, b.y, b.z))];
                if name == "closed" {
                    out.push(boxed(DVec3::new(a.x + t, a.y, a.z), DVec3::new(b.x - t, a.y + t, b.z)));
                }

                out.into_iter().flatten().collect()
            }
            "arched" => {
                let spring = b.y - bounds.size().x.min(bounds.size().y) * 0.5;
                let top = Aabb::new(DVec3::new(a.x, spring, a.z), b);
                let mut out: Vec<Brush> = [left(spring), right(spring)].into_iter().flatten().collect();
                let segments = sides.max(1);
                let wall = if t > 0.0 { t } else { 1.0 };
                let (rx, ry) = (top.size().x * 0.5, top.size().y);
                let center = DVec2::new(top.center().x, top.min.y);
                out.extend((0..segments).filter_map(|i| {
                    let a0 = std::f64::consts::PI * i as f64 / segments as f64;
                    let a1 = std::f64::consts::PI * (i + 1) as f64 / segments as f64;
                    let outer = |a: f64| center + DVec2::new(a.cos() * rx, a.sin() * ry);
                    let inner = |a: f64| center + DVec2::new(a.cos() * (rx - wall).max(0.0), a.sin() * (ry - wall).max(0.0));
                    extruded([outer(a0), outer(a1), inner(a0), inner(a1)])
                }));
                out
            }
            _ => {
                let c = DVec2::new(bounds.center().x, bounds.center().y);
                let inner_r = DVec2::new(bounds.size().x * 0.5 - t, bounds.size().y * 0.5 - t);
                let outer = ring(c, DVec2::new(bounds.size().x * 0.5, bounds.size().y * 0.5), sides.max(3), 0.0);
                let inner: Vec<DVec2> = outer
                    .iter()
                    .map(|p| {
                        let d = *p - c;
                        c + d / (d / inner_r).length()
                    })
                    .collect();
                let n = outer.len();
                (0..n).filter_map(|i| extruded([outer[i], outer[(i + 1) % n], inner[i], inner[(i + 1) % n]])).collect()
            }
        }
    }

    fn tunnel_bounds() -> Aabb {
        Aabb::new(DVec3::new(-40.0, 10.0, 200.0), DVec3::new(88.0, 110.0, 360.0))
    }

    fn off_origin() -> Aabb {
        Aabb::new(DVec3::new(-37.5, 10.25, 203.0), DVec3::new(91.75, 117.0, 360.5))
    }

    fn test_boxes() -> [Aabb; 4] {
        let tall = Aabb::new(DVec3::new(13.5, -20.0, -64.0), DVec3::new(45.25, 300.0, 100.0));
        let low = Aabb::new(DVec3::new(-300.0, 4.0, 7.0), DVec3::new(212.5, 40.5, 99.0));
        [tunnel_bounds(), off_origin(), tall, low]
    }

    fn assert_apart(pieces: &[Brush], what: &str) {
        for (i, a) in pieces.iter().enumerate() {
            a.validate().unwrap();
            for other in &pieces[i + 1..] {
                assert!(!a.intersects(other), "{what} overlaps");
            }
        }
    }

    #[test]
    fn a_default_tunnel_matches_the_old_one() {
        for bounds in test_boxes() {
            let made = tunnel(&bounds, &Tunnel::default(), "m");
            assert_eq!(made.len(), 3, "{bounds:?}");
            assert_eq!(format!("{made:?}"), format!("{:?}", old_tunnel(&bounds, "open_bottom", 12, 16.0)), "{bounds:?}");
        }
    }

    #[test]
    fn a_legacy_profile_builds_what_it_built() {
        for bounds in test_boxes() {
            for name in LEGACY {
                for sides in [1, 2, 3, 8, 96] {
                    for thickness in [0.0, 1.0, 16.0, 1e6] {
                        let (new, old) = (legacy(&bounds, name, sides, thickness), old_tunnel(&bounds, name, sides, thickness));
                        assert!(old.len() >= 2, "{name} {sides} {thickness} on {bounds:?} builds pieces");
                        assert_eq!(format!("{new:?}"), format!("{old:?}"), "{name} {sides} {thickness} on {bounds:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn old_profile_names_still_parse() {
        for (name, want) in [
            ("square", (TunnelProfile::Square, false)),
            ("open_bottom", (TunnelProfile::Square, false)),
            ("closed", (TunnelProfile::Square, true)),
            ("arched", (TunnelProfile::Arched, false)),
            ("round", (TunnelProfile::Round, true)),
            (" Closed ", (TunnelProfile::Square, true)),
            ("Open_Bottom", (TunnelProfile::Square, false)),
            ("SQUARE", (TunnelProfile::Square, false)),
        ] {
            assert_eq!(TunnelProfile::parse(name), Some(want), "{name:?}");
        }

        for name in ["box", "", "open bottom", "tube", "squared"] {
            assert_eq!(TunnelProfile::parse(name), None, "{name:?}");
        }
    }

    #[test]
    fn a_square_tunnel_takes_floor_and_ceiling() {
        let bounds = off_origin();
        let (a, b, c, t) = (bounds.min, bounds.max, bounds.center(), 16.0);
        let slab = |x0: f64, x1: f64, y0: f64, y1: f64| Aabb::new(DVec3::new(x0, y0, a.z), DVec3::new(x1, y1, b.z));
        let (left, right) = (slab(a.x, a.x + t, a.y, b.y), slab(b.x - t, b.x, a.y, b.y));
        let (ceiling, floor) = (slab(a.x + t, b.x - t, b.y - t, b.y), slab(a.x + t, b.x - t, a.y, a.y + t));
        for (has_floor, has_ceiling, count) in [(false, false, 2), (false, true, 3), (true, false, 3), (true, true, 4)] {
            let what = format!("floor {has_floor} ceiling {has_ceiling}");
            let pieces = tunnel(&bounds, &Tunnel { floor: has_floor, ceiling: has_ceiling, ..Tunnel::default() }, "m");
            assert_eq!(pieces.len(), count, "{what}");
            let mut want = vec![left, right];
            want.extend(has_ceiling.then_some(ceiling));
            want.extend(has_floor.then_some(floor));
            assert_eq!(pieces.iter().map(Brush::bounds).collect::<Vec<_>>(), want, "{what}");
            assert_apart(&pieces, &what);
            let filled = |p: DVec3| pieces.iter().any(|piece| piece.contains_point(p));
            assert_eq!(filled(DVec3::new(c.x + 9.0, a.y + t * 0.5, c.z - 11.0)), has_floor, "{what} floor slab");
            assert_eq!(filled(DVec3::new(c.x - 13.0, b.y - t * 0.5, c.z + 7.0)), has_ceiling, "{what} ceiling slab");
            assert!(!filled(c), "{what} leaves the middle empty");
        }
    }

    #[test]
    fn an_arched_tunnel_takes_a_floor() {
        let bounds = off_origin();
        let (a, b, t) = (bounds.min, bounds.max, 16.0);
        let arched = |sides: usize, floor: bool, ceiling: bool| {
            tunnel(&bounds, &Tunnel { profile: TunnelProfile::Arched, sides, floor, ceiling, ..Tunnel::default() }, "m")
        };
        for sides in [2, 5, 12] {
            let (open, floored) = (arched(sides, false, true), arched(sides, true, true));
            assert_eq!(open.len(), 2 + sides, "{sides}");
            assert_eq!(floored.len(), 2 + sides + 1, "{sides}");
            assert_eq!(format!("{:?}", &floored[..open.len()]), format!("{open:?}"), "{sides} the floor leaves the rest as it was");
            assert_eq!(floored.last().unwrap().bounds(), Aabb::new(DVec3::new(a.x + t, a.y, a.z), DVec3::new(b.x - t, a.y + t, b.z)), "{sides}");
            assert_apart(&floored, &format!("{sides} sides"));
            assert!(!floored.iter().any(|p| p.contains_point(bounds.center())), "{sides} leaves the middle empty");
            assert_eq!(format!("{:?}", arched(sides, true, false)), format!("{floored:?}"), "{sides} has no ceiling to take away");
        }
    }

    #[test]
    fn a_round_tunnel_ignores_floor_and_ceiling() {
        let bounds = off_origin();
        let round = |floor: bool, ceiling: bool| tunnel(&bounds, &Tunnel { profile: TunnelProfile::Round, sides: 7, floor, ceiling, ..Tunnel::default() }, "m");
        let plain = round(false, true);
        assert_eq!(plain.len(), 7);
        for (floor, ceiling) in [(false, false), (true, false), (true, true)] {
            assert_eq!(format!("{:?}", round(floor, ceiling)), format!("{plain:?}"), "floor {floor} ceiling {ceiling}");
        }
    }

    #[test]
    fn a_tunnel_has_its_pieces_inside_the_bounds_without_overlap() {
        let bounds = tunnel_bounds();
        for (name, count) in [("open_bottom", 3), ("closed", 4), ("arched", 2 + 6), ("round", 6)] {
            let pieces = legacy(&bounds, name, 6, 16.0);
            assert_eq!(pieces.len(), count, "{name}");
            for p in &pieces {
                let got = p.bounds();
                assert!(got.min.cmpge(bounds.min - 1e-6).all() && got.max.cmple(bounds.max + 1e-6).all(), "{name} {got:?}");
                assert!((got.size().z - bounds.size().z).abs() < 1e-6, "{name} runs the full length");
            }

            assert_apart(&pieces, name);
        }
    }

    #[test]
    fn a_tunnel_interior_is_empty() {
        let bounds = tunnel_bounds();
        let c = bounds.center();
        for (name, probe) in [("open_bottom", DVec3::new(c.x, bounds.min.y + 5.0, c.z)), ("open_bottom", c), ("closed", c), ("arched", c), ("round", c)] {
            for thickness in [16.0, 4000.0] {
                for p in legacy(&bounds, name, 8, thickness) {
                    assert!(!p.contains_point(probe), "{name} at {thickness} fills {probe:?}");
                }
            }
        }

        let open = build(&bounds, TunnelProfile::Square, 8, 16.0);
        assert!(open.iter().all(|p| p.bounds().min.y <= bounds.min.y + 1e-6 || p.bounds().min.y > bounds.min.y + 16.0), "no floor");
    }

    #[test]
    fn a_round_tunnel_with_few_sides_has_no_overlapping_pieces() {
        for size in [DVec3::new(20.0, 200.0, 64.0), DVec3::new(64.0, 256.0, 64.0), DVec3::new(256.0, 24.0, 64.0), DVec3::new(128.0, 128.0, 64.0)] {
            let bounds = Aabb::new(DVec3::new(-30.0, 8.0, 0.0), DVec3::new(-30.0, 8.0, 0.0) + size);
            for sides in [0, 3, 4, 5, 6, 9] {
                for thickness in [4.0, 16.0, 1e6] {
                    let pieces = build(&bounds, TunnelProfile::Round, sides, thickness);
                    assert_eq!(pieces.len(), sides.max(3), "{size} {sides} {thickness}");
                    assert!(!pieces.iter().any(|a| a.contains_point(bounds.center())), "{size} {sides} {thickness} fills the middle");
                    assert_apart(&pieces, &format!("{size} {sides} {thickness}"));
                }
            }
        }

        let square = Aabb::new(DVec3::ZERO, DVec3::new(128.0, 128.0, 64.0));
        let t = 16.0;
        for v in build(&square, TunnelProfile::Round, 8, t).iter().flat_map(|p| p.vertices.clone()) {
            let d = DVec2::new(v.x - 64.0, v.y - 64.0).length();
            assert!((d - 64.0).abs() < 1e-6 || (d - (64.0 - t)).abs() < 1e-6, "a round box keeps a circular ring: {d}");
        }
    }

    #[test]
    fn the_tunnel_thickness_is_clamped() {
        let bounds = tunnel_bounds();
        assert_eq!(tunnel_thickness(&bounds, 16.0), 16.0);
        assert_eq!(tunnel_thickness(&bounds, 0.0), 1.0);
        assert!((tunnel_thickness(&bounds, 1e6) - 40.0).abs() < 1e-9, "40 percent of the 100 unit height");
        let walls = legacy(&bounds, "closed", 8, 1e6);
        assert!(walls.iter().all(|w| w.volume() > 0.0) && walls.len() == 4);
    }

    fn arched(angle: f64, thickness: f64) -> Tunnel {
        Tunnel { profile: TunnelProfile::Arched, angle, thickness, ..Tunnel::default() }
    }

    fn rect(piece: &Piece) -> (DVec2, DVec2) {
        match piece {
            Piece::Rect(min, max) => (*min, *max),
            Piece::Poly(_) => panic!("a box was expected"),
        }
    }

    fn poly(piece: &Piece) -> &[DVec2] {
        match piece {
            Piece::Poly(points) => points,
            Piece::Rect(..) => panic!("a polygon was expected"),
        }
    }

    /// How far a circular arch `width` wide that sweeps `angle` degrees rises above its ends.
    fn rise(width: f64, angle: f64) -> f64 {
        let radius = width * 0.5 / (angle.to_radians() * 0.5).sin();
        radius - (radius * radius - width * width * 0.25).sqrt()
    }

    #[test]
    fn a_flat_arched_tunnel_springs_from_the_walls() {
        let bounds = Aabb::new(DVec3::new(-37.5, 10.25, 203.0), DVec3::new(90.5, 110.25, 360.5));
        let (a, b, c, t) = (bounds.min, bounds.max, bounds.center(), 16.0);
        let params = arched(120.0, t);
        assert_eq!(params.fitted(&bounds).angle, 120.0, "a 128 by 100 box leaves room for 120");
        let pieces = section(&bounds, &params);
        assert_eq!(pieces.len(), 2 + 12);
        let spring = rect(&pieces[0]).1.y;
        assert!((spring - (b.y - rise(128.0, 120.0))).abs() < 1e-9, "the walls stop where a 120 degree arch springs, not at {spring}");
        assert_eq!(rect(&pieces[0]), (DVec2::new(a.x, a.y), DVec2::new(a.x + t, spring)));
        assert_eq!(rect(&pieces[1]), (DVec2::new(b.x - t, a.y), DVec2::new(b.x, spring)));
        let (first, last) = (poly(&pieces[2]), poly(&pieces[13]));
        assert_eq!([first[0], first[2]], [DVec2::new(a.x, spring), DVec2::new(a.x + t, spring)], "the arch springs from both faces of the left wall");
        assert_eq!([last[1], last[3]], [DVec2::new(b.x, spring), DVec2::new(b.x - t, spring)], "and of the right wall");

        let made = tunnel(&bounds, &params, "m");
        assert_eq!(made.len(), 2 + 12);
        assert_apart(&made, "a 120 degree arch");
        let top = made.iter().flat_map(|p| p.vertices.iter().map(|v| v.y)).fold(f64::MIN, f64::max);
        assert!((top - b.y).abs() < 1e-9, "the crown reaches the top of the box, not {top}");
        assert!(made[2..].iter().all(|p| p.bounds().min.y >= spring - 1e-9), "the arch sits on the walls");
        let filled = |p: DVec3| made.iter().any(|piece| piece.contains_point(p));
        assert!(!filled(DVec3::new(a.x + t + 0.5, spring + 0.1, c.z - 30.0)), "no ledge inside the left wall");
        assert!(!filled(DVec3::new(b.x - t - 0.5, spring + 0.1, c.z + 30.0)), "no ledge inside the right wall");
        assert!(filled(DVec3::new(c.x + 3.0, b.y - 0.5, c.z - 20.0)), "the crown is solid");
        assert!(!filled(c));
    }

    #[test]
    fn the_arched_angle_is_capped_by_the_box() {
        let bounds = Aabb::new(DVec3::new(-100.5, 7.25, -30.0), DVec3::new(155.5, 71.25, 98.0));
        let fit = |angle: f64| arched(angle, 16.0).fitted(&bounds).angle;
        let cap = fit(120.0);
        assert!((rise(256.0, cap) - 32.0).abs() < 1e-9, "120 is cut back to the sweep rising half the height, not {cap}");
        assert_eq!(fit(cap), cap, "the cap itself is kept");
        assert_eq!(fit(cap + 1e-6), cap);
        assert_eq!(fit(cap - 1e-6), cap - 1e-6);
        assert_eq!(fit(180.0), 180.0);
        assert_eq!(fit(400.0), 180.0);
        for angle in [120.0, cap] {
            let made = tunnel(&bounds, &arched(angle, 16.0), "m");
            let fitted = tunnel(&bounds, &arched(angle, 16.0).fitted(&bounds), "m");
            assert_eq!(format!("{made:?}"), format!("{fitted:?}"), "{angle} builds its fitted tunnel");
            assert_eq!(made.len(), 2 + 12);
            assert!((made[0].bounds().max.y - (bounds.max.y - 32.0)).abs() < 1e-9, "{angle} springs half the height down");
            assert_apart(&made, &format!("{angle}"));
        }
    }

    #[test]
    fn the_arched_angle_keeps_the_crown_one_unit_thick() {
        let wide = Aabb::new(DVec3::new(-77.0, -12.5, 40.0), DVec3::new(179.0, 87.5, 250.0));
        let (b, c) = (wide.max, wide.center());
        let fit = |angle: f64| arched(angle, 4.0).fitted(&wide).angle;
        let crown = |angle: f64| arched(angle, 4.0).arch_crown(&wide).unwrap();
        // 504.5 is the radius whose circle through the outer springs passes 1 unit above the one through the inner springs.
        let lo = 2.0 * (128.0f64 / 504.5).asin().to_degrees();
        let raised = fit(10.0);
        assert!((raised - lo).abs() < 1e-9, "10 is raised to {lo}, not {raised}");
        assert!((crown(10.0) - 1.0).abs() < 1e-9, "{}", crown(10.0));
        assert_eq!(fit(raised), raised, "the lowest angle itself is kept");
        assert_eq!(fit(raised - 1e-6), raised);
        assert_eq!(fit(raised + 1e-6), raised + 1e-6);
        assert!(crown(raised + 1e-6) > 1.0);

        let made = tunnel(&wide, &arched(10.0, 4.0), "m");
        assert_apart(&made, "a 1 unit crown");
        let filled = |p: DVec3| made.iter().any(|piece| piece.contains_point(p));
        assert!(filled(DVec3::new(c.x + 0.25, b.y - 0.5, c.z)) && !filled(DVec3::new(c.x + 0.25, b.y - 1.5, c.z)), "the crown is 1 thick");

        let narrow = Aabb::new(DVec3::new(5.0, 3.5, -8.0), DVec3::new(69.0, 67.5, 120.0));
        assert_eq!(arched(10.0, 16.0).fitted(&narrow).angle, 10.0, "a 16 thick wall keeps the crown over 1 at 10");
        assert!(arched(10.0, 16.0).arch_crown(&narrow).unwrap() > 1.0);
        assert_eq!(arched(4.0, 16.0).fitted(&narrow).angle, 10.0, "nothing sweeps under 10");

        assert!(arched(10.0, 1.5).fitted(&wide).angle < 180.0, "a wall just over 1 thick still makes a flat arch");
        for thickness in [1.0, 0.0, -5.0] {
            for angle in [10.0, 45.0, 120.0, 179.9, 180.0] {
                let thin = arched(angle, thickness);
                assert_eq!(thin.fitted(&wide).angle, 180.0, "{thickness} {angle}");
                assert_eq!(thin.arch_crown(&wide), None, "{thickness} {angle}");
                let half_round = tunnel(&wide, &arched(180.0, thickness), "m");
                assert_eq!(format!("{:?}", tunnel(&wide, &thin, "m")), format!("{half_round:?}"), "{thickness} {angle}");
            }
        }

        assert_eq!(Tunnel { angle: 120.0, ..Tunnel::default() }.fitted(&wide).angle, 120.0, "only an arch has an angle to fit");
        assert_eq!(Tunnel { angle: 120.0, ..Tunnel::default() }.arch_crown(&wide), None);
        assert_eq!(arched(180.0, 16.0).arch_crown(&wide), None);
    }

    #[test]
    fn a_low_wide_arched_tunnel_falls_back_to_a_half_ellipse() {
        let low = Aabb::new(DVec3::new(-200.0, 16.0, 8.0), DVec3::new(312.0, 48.0, 72.0));
        let flat = arched(60.0, 4.0);
        assert_eq!(flat.fitted(&low).angle, 180.0, "no sweep both fits 512 by 32 and keeps the crown 1 thick");
        assert_eq!(flat.arch_crown(&low), None);
        assert_eq!(format!("{:?}", tunnel(&low, &flat, "m")), format!("{:?}", tunnel(&low, &arched(180.0, 4.0), "m")));

        let taller = Aabb::new(DVec3::new(-200.0, 16.0, 8.0), DVec3::new(312.0, 86.0, 72.0));
        let lo = 2.0 * (256.0f64 / 1016.5).asin().to_degrees();
        let raised = arched(20.0, 4.0).fitted(&taller).angle;
        assert!((raised - lo).abs() < 1e-9, "70 high leaves room for the 1 unit crown at {lo}, not {raised}");
        let capped = flat.fitted(&taller).angle;
        assert!((rise(512.0, capped) - 35.0).abs() < 1e-9, "{capped}");
        assert!(capped > raised);
    }

    #[test]
    fn a_flat_arched_tunnel_has_no_overlapping_pieces() {
        for bounds in test_boxes() {
            for angle in [10.0, 60.0, 120.0, 179.9] {
                for sides in [2, 3, 12, 96] {
                    let sweep = arched(angle, 16.0).fitted(&bounds).angle;
                    let what = format!("{angle} as {sweep} with {sides} sides on {bounds:?}");
                    let made = tunnel(&bounds, &Tunnel { sides, ..arched(angle, 16.0) }, "m");
                    assert_eq!(made.len(), 2 + sides, "{what}");
                    assert_apart(&made, &what);
                    assert!(!made.iter().any(|p| p.contains_point(bounds.center())), "{what} fills the middle");
                    for p in &made {
                        let got = p.bounds();
                        assert!(got.min.cmpge(bounds.min - 1e-6).all() && got.max.cmple(bounds.max + 1e-6).all(), "{what} {got:?}");
                    }

                    if sweep < 180.0 {
                        let spring = bounds.max.y - rise(bounds.size().x, sweep);
                        assert!((made[0].bounds().max.y - spring).abs() < 1e-9, "{what} springs where its arch does");
                    }
                }
            }
        }
    }

    fn walkway(width: f64, height: f64) -> Tunnel {
        Tunnel { walkway: true, walkway_width: width, walkway_height: height, ..Tunnel::default() }
    }

    #[test]
    fn a_walkway_lines_both_walls() {
        let bounds = off_origin();
        let (a, b, c, t) = (bounds.min, bounds.max, bounds.center(), 16.0);
        let (width, height) = (20.5, 12.25);
        for (profile, floor, ceiling, count) in [
            (TunnelProfile::Square, false, true, 3),
            (TunnelProfile::Square, true, false, 3),
            (TunnelProfile::Square, true, true, 4),
            (TunnelProfile::Arched, false, true, 2 + 6),
            (TunnelProfile::Arched, true, true, 2 + 6 + 1),
        ] {
            let what = format!("{profile:?} floor {floor} ceiling {ceiling}");
            let plain = Tunnel { profile, floor, ceiling, sides: 6, angle: 120.0, ..Tunnel::default() };
            let without = tunnel(&bounds, &plain, "m");
            let with = tunnel(&bounds, &Tunnel { profile, floor, ceiling, sides: 6, angle: 120.0, ..walkway(width, height) }, "m");
            assert_eq!(without.len(), count, "{what}");
            assert_eq!(with.len(), count + 2, "{what}");
            assert_eq!(format!("{:?}", &with[..count]), format!("{without:?}"), "{what} keeps the rest as it was");
            let base = if floor { a.y + t } else { a.y };
            let slab = |x0: f64, x1: f64| Aabb::new(DVec3::new(x0, base, a.z), DVec3::new(x1, base + height, b.z));
            assert_eq!(with[count].bounds(), slab(a.x + t, a.x + t + width), "{what} left");
            assert_eq!(with[count + 1].bounds(), slab(b.x - t - width, b.x - t), "{what} right");
            assert_apart(&with, &what);
            let filled = |p: DVec3| with.iter().any(|piece| piece.contains_point(p));
            let mid = base + height * 0.5;
            assert!(filled(DVec3::new(a.x + t + width - 0.5, mid, c.z - 31.0)), "{what} the left walkway is solid");
            assert!(filled(DVec3::new(b.x - t - width + 0.5, mid, c.z + 17.0)), "{what} the right walkway is solid");
            assert!(!filled(DVec3::new(a.x + t + width + 0.5, mid, c.z + 5.0)), "{what} the channel is empty");
            assert!(!filled(DVec3::new(b.x - t - width - 0.5, mid, c.z - 9.0)), "{what} the channel is empty");
            assert!(!filled(DVec3::new(a.x + t + 0.5, base + height + 0.5, c.z)), "{what} nothing stands on the walkway");
        }
    }

    #[test]
    fn the_walkway_is_clamped() {
        let bounds = Aabb::new(DVec3::new(-21.5, 6.0, -40.0), DVec3::new(170.5, 102.0, 88.0));
        let (a, b) = (bounds.min, bounds.max);
        let floored = |width: f64, height: f64| Tunnel { floor: true, ..walkway(width, height) };
        let fit = |params: Tunnel| {
            let fitted = params.fitted(&bounds);
            (fitted.walkway_width, fitted.walkway_height)
        };
        // The opening is 192 - 2 * 16 wide and 96 - 2 * 16 high between the floor and the ceiling.
        for (want, got) in [
            ((0.0, 0.0), (1.0, 1.0)),
            ((-7.5, -3.0), (1.0, 1.0)),
            ((1.0 - 1e-9, 1.0 - 1e-9), (1.0, 1.0)),
            ((1.0, 1.0), (1.0, 1.0)),
            ((1.0 + 1e-9, 1.0 + 1e-9), (1.0 + 1e-9, 1.0 + 1e-9)),
            ((63.5, 31.75), (63.5, 31.75)),
            ((64.0 - 1e-9, 32.0 - 1e-9), (64.0 - 1e-9, 32.0 - 1e-9)),
            ((64.0, 32.0), (64.0, 32.0)),
            ((64.0 + 1e-9, 32.0 + 1e-9), (64.0, 32.0)),
            ((1e6, 1e6), (64.0, 32.0)),
        ] {
            assert_eq!(fit(floored(want.0, want.1)), got, "{want:?}");
        }

        for (params, high) in [
            (Tunnel { floor: false, ..walkway(1e6, 1e6) }, 40.0),
            (Tunnel { floor: true, ceiling: false, ..walkway(1e6, 1e6) }, 40.0),
            (Tunnel { floor: false, ceiling: false, ..walkway(1e6, 1e6) }, 48.0),
            (Tunnel { profile: TunnelProfile::Arched, floor: true, ..walkway(1e6, 1e6) }, 16.0),
            (Tunnel { profile: TunnelProfile::Arched, floor: false, ..walkway(1e6, 1e6) }, 24.0),
            (Tunnel { profile: TunnelProfile::Arched, floor: true, angle: 120.0, ..walkway(1e6, 1e6) }, 16.0),
        ] {
            let (width, height) = fit(params);
            assert!(width == 64.0 && (height - high).abs() < 1e-9, "{params:?} stays under half of what is left under the top: {height}");
        }

        let thick = fit(Tunnel { thickness: 1e6, ..floored(1e6, 1e6) });
        assert!((thick.0 - (192.0 - 2.0 * 38.4) * 0.4).abs() < 1e-9 && (thick.1 - (96.0 - 2.0 * 38.4) * 0.5).abs() < 1e-9, "{thick:?}");

        let widest = tunnel(&bounds, &floored(1e6, 1e6), "m");
        assert_eq!(widest.len(), 6);
        assert_eq!(widest[4].bounds(), Aabb::new(DVec3::new(a.x + 16.0, a.y + 16.0, a.z), DVec3::new(a.x + 80.0, a.y + 48.0, b.z)));
        assert_eq!(widest[5].bounds(), Aabb::new(DVec3::new(b.x - 80.0, a.y + 16.0, a.z), DVec3::new(b.x - 16.0, a.y + 48.0, b.z)));
        assert_eq!(format!("{widest:?}"), format!("{:?}", tunnel(&bounds, &floored(1e6, 1e6).fitted(&bounds), "m")));
        assert_apart(&widest, "the widest walkways");
        assert!(!widest.iter().any(|p| p.contains_point(DVec3::new(bounds.center().x, a.y + 47.0, 3.0))), "a channel is left");

        let off = Tunnel { walkway: false, ..floored(20.0, 10.0) };
        let made = tunnel(&bounds, &off, "m");
        assert_eq!(made.len(), 4, "no walkways when it is off");
        assert_eq!(format!("{made:?}"), format!("{:?}", tunnel(&bounds, &Tunnel { floor: true, ..Tunnel::default() }, "m")));
    }

    #[test]
    fn a_round_tunnel_walkway_sits_in_the_bore() {
        let tall = Aabb::new(DVec3::new(13.5, -20.0, -64.0), DVec3::new(141.25, 300.0, 100.0));
        for bounds in [off_origin(), tall] {
            let t = 16.0;
            for sides in [3, 7, 8] {
                for (width, height) in [(9.5, 13.25), (0.0, 0.0), (1e6, 1e6)] {
                    let what = format!("{sides} sides, {width} by {height} on {bounds:?}");
                    let params = Tunnel { profile: TunnelProfile::Round, sides, ..walkway(width, height) };
                    let pieces = section(&bounds, &params);
                    assert_eq!(pieces.len(), sides + 2, "{what}");
                    let bore: Vec<DVec2> = pieces[..sides].iter().map(|p| poly(p)[2]).collect();
                    let edges = || (0..sides).map(|i| (bore[i], bore[(i + 1) % sides] - bore[i]));
                    let low = bore.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
                    let fitted = params.fitted(&bounds);
                    let (fw, fh) = (fitted.walkway_width, fitted.walkway_height);
                    assert!((fh - height.max(1.0).min(bounds.size().y * 0.5 - t)).abs() < 1e-9, "{what} height {fh}");
                    let top = low + fh;
                    let tops: Vec<(f64, f64)> = pieces[sides..]
                        .iter()
                        .map(|piece| {
                            let points = poly(piece);
                            assert!(points.iter().all(|&p| edges().all(|(q, d)| d.perp_dot(p - q) >= -1e-9)), "{what} stays in the bore");
                            assert!(points.iter().all(|p| p.y <= top + 1e-9), "{what} stays under the top");
                            let at_top: Vec<f64> = points.iter().filter(|p| (p.y - top).abs() < 1e-9).map(|p| p.x).collect();
                            assert_eq!(at_top.len(), 2, "{what} has a flat top at {top}: {points:?}");
                            (at_top[0].min(at_top[1]), at_top[0].max(at_top[1]))
                        })
                        .collect();
                    let ((l0, l1), (r0, r1)) = (tops[0], tops[1]);
                    for x in [l0, r1] {
                        assert!(edges().any(|(q, d)| (d.perp_dot(DVec2::new(x, top) - q) / d.length()).abs() < 1e-9), "{what} {x} reaches the bore");
                    }

                    assert!((l1 - l0 - fw).abs() < 1e-9 && (r1 - r0 - fw).abs() < 1e-9, "{what} both tops are {fw} wide");
                    assert!((fw - width.max(1.0).min((r1 - l0) * 0.4)).abs() < 1e-9, "{what} width {fw}");
                    assert!(l1 < r0, "{what} leaves a channel");

                    let made = tunnel(&bounds, &params, "m");
                    assert_eq!(made.len(), sides + 2, "{what}");
                    assert_apart(&made, &what);
                    let filled = |x: f64, y: f64| made.iter().any(|p| p.contains_point(DVec3::new(x, y, bounds.center().z + 3.0)));
                    assert!(!filled((l1 + r0) * 0.5, top - 0.01), "{what} the channel is empty");
                    assert!(!filled((l0 + l1) * 0.5, top + 0.01) && !filled((r0 + r1) * 0.5, top + 0.01), "{what} nothing over the walkways");
                    for (piece, side) in pieces[sides..].iter().zip(["left", "right"]) {
                        let points = poly(piece);
                        let mid = points.iter().copied().sum::<DVec2>() / points.len() as f64;
                        assert!(filled(mid.x, mid.y), "{what} the {side} walkway is solid");
                    }
                }
            }
        }
    }

    #[test]
    fn clip_convex_keeps_the_inside() {
        let quad = [DVec2::new(10.5, -3.0), DVec2::new(40.0, 2.5), DVec2::new(33.0, 21.0), DVec2::new(5.0, 14.5)];
        assert_eq!(clip_convex(&quad, DVec2::Y, 21.0), quad, "all in, the top corner on the line");
        assert_eq!(clip_convex(&quad, DVec2::new(0.6, 0.8), 100.0), quad, "all in, slanted");
        assert!(clip_convex(&quad, DVec2::Y, -3.5).is_empty(), "all out");
        assert!(clip_convex(&quad, DVec2::new(-0.6, -0.8), -40.0).is_empty(), "all out, slanted");
        assert!(clip_convex(&[], DVec2::Y, 0.0).is_empty());

        let cut = clip_convex(&quad, DVec2::X, 20.0);
        let want = [quad[0], DVec2::new(20.0, -3.0 + 5.5 * 9.5 / 29.5), DVec2::new(20.0, 21.0 - 6.5 * 13.0 / 28.0), quad[3]];
        assert_eq!(cut.len(), 4, "{cut:?}");
        assert!(cut.iter().zip(want).all(|(got, want)| got.distance(want) < 1e-9), "through two edges: {cut:?}");

        let slanted = clip_convex(&quad, DVec2::new(1.0, 1.0), 30.0);
        assert_eq!(slanted.len(), 4, "{slanted:?}");
        assert!(slanted.iter().all(|p| p.x + p.y <= 30.0 + 1e-9) && slanted.contains(&quad[0]) && slanted.contains(&quad[3]), "{slanted:?}");

        // The edge from the last corner to the first lies on 17.5 x + 5.5 y = 167.25, the others on its far side.
        assert_eq!(clip_convex(&quad, DVec2::new(-17.5, -5.5), -167.25), quad, "a cut along an edge adds no points");
        assert_eq!(clip_convex(&quad, DVec2::new(17.5, 5.5), 167.25), vec![quad[0], quad[3]], "the far side keeps only the edge");
        // The diagonal from the second corner to the last lies on 12 x + 35 y = 567.5.
        assert_eq!(clip_convex(&quad, DVec2::new(12.0, 35.0), 567.5), vec![quad[0], quad[1], quad[3]], "a cut through two corners");
    }
}
