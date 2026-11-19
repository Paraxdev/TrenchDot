//! Tunnels. The cross section is cut into convex 2D pieces, which are then run along the tunnel's length.

use td_core::{Aabb, DVec2, DVec3};

use super::{arch_quads, hull, ring};
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
    /// Degrees an arched tunnel's arch sweeps, 180 being a half ellipse.
    pub angle: f64,
    /// A slab as thick as the walls along the bottom of a square or arched tunnel.
    pub floor: bool,
    /// The slab over a square tunnel, without it the tunnel is an open trench.
    pub ceiling: bool,
}

impl Default for Tunnel {
    fn default() -> Self {
        Self { profile: TunnelProfile::default(), sides: 12, thickness: 16.0, angle: 180.0, floor: false, ceiling: true }
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
/// height, the ceiling and floor sit between them, the floor last. An arched tunnel stands its walls up to where the
/// arch begins, the arch rising `min(width, height) / 2`.
pub fn tunnel(bounds: &Aabb, params: &Tunnel, material: &str) -> Vec<Brush> {
    section(bounds, params).iter().filter_map(|piece| place_straight(piece, bounds, material)).collect()
}

fn section(bounds: &Aabb, params: &Tunnel) -> Vec<Piece> {
    let (a, b) = (bounds.min, bounds.max);
    let t = tunnel_thickness(bounds, params.thickness);
    let left = |top: f64| Piece::Rect(DVec2::new(a.x, a.y), DVec2::new(a.x + t, top));
    let right = |top: f64| Piece::Rect(DVec2::new(b.x - t, a.y), DVec2::new(b.x, top));
    let floor = || Piece::Rect(DVec2::new(a.x + t, a.y), DVec2::new(b.x - t, a.y + t));
    match params.profile {
        TunnelProfile::Square => {
            let mut out = vec![left(b.y), right(b.y)];
            out.extend(params.ceiling.then(|| Piece::Rect(DVec2::new(a.x + t, b.y - t), DVec2::new(b.x - t, b.y))));
            out.extend(params.floor.then(floor));
            out
        }
        TunnelProfile::Arched => {
            let spring = b.y - bounds.size().x.min(bounds.size().y) * 0.5;
            let top = Aabb::new(DVec3::new(a.x, spring, a.z), b);
            let mut out = vec![left(spring), right(spring)];
            out.extend(arch_quads(&top, params.sides, t, 180.0).into_iter().map(|quad| Piece::Poly(quad.to_vec())));
            out.extend(params.floor.then(floor));
            out
        }
        TunnelProfile::Round => {
            let c = DVec2::new(bounds.center().x, bounds.center().y);
            let inner_r = DVec2::new(bounds.size().x * 0.5 - t, bounds.size().y * 0.5 - t);
            let outer = ring(c, DVec2::new(bounds.size().x * 0.5, bounds.size().y * 0.5), params.sides.max(3), 0.0);
            // Each inner corner sits on the ray from the center through its outer corner, unlike pipe's, so every piece
            // stays convex and inside its own wedge even when few sides span a narrow oval.
            let inner: Vec<DVec2> = outer
                .iter()
                .map(|p| {
                    let d = *p - c;
                    c + d / (d / inner_r).length()
                })
                .collect();
            let n = outer.len();
            (0..n).map(|i| Piece::Poly(vec![outer[i], outer[(i + 1) % n], inner[i], inner[(i + 1) % n]])).collect()
        }
    }
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
}
