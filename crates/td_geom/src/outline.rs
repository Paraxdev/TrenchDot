//! Brushes from a flat outline: the corners are cleaned up, a concave outline is cut into convex pieces and each piece
//! is extruded into a prism.

use td_core::{DVec2, DVec3, Plane};

use crate::brush::{Brush, FaceData};
use crate::polygon;

/// Most corners an outline may have, the decomposition is cubic in it.
pub const MAX_CORNERS: usize = 256;

/// Corners closer together than this are one corner.
const MERGE_DISTANCE: f64 = 1e-4;
/// Relative cross product below which three corners count as lying on one line.
const ON_LINE: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineError {
    /// Fewer than three distinct corners are left.
    TooFew,
    TooMany,
    /// Every corner lies on one line, so the outline has no area.
    Flat,
    /// Two edges cross, touch or lie on top of each other.
    SelfIntersecting,
    /// The corners do not lie on one plane across the extrusion direction.
    NotPlanar,
    /// The extrusion is shorter than a hair.
    NoHeight,
    /// A piece could not be turned into a valid brush.
    BadBrush,
}

impl std::fmt::Display for OutlineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OutlineError::TooFew => "an outline needs at least three different corners",
            OutlineError::TooMany => "the outline has too many corners",
            OutlineError::Flat => "the outline has no area, its corners lie on one line",
            OutlineError::SelfIntersecting => "the outline crosses itself",
            OutlineError::NotPlanar => "the corners do not lie on one plane across the extrusion direction",
            OutlineError::NoHeight => "the extrusion height is zero",
            OutlineError::BadBrush => "a piece of the outline did not make a valid brush",
        })
    }
}

impl std::error::Error for OutlineError {}

fn cross(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    (b - a).perp_dot(c - a)
}

fn signed_area(poly: &[DVec2]) -> f64 {
    (0..poly.len()).map(|i| poly[i].perp_dot(poly[(i + 1) % poly.len()])).sum::<f64>() * 0.5
}

/// Whether the corner `p` between `prev` and `next` lies on the line through them.
fn straight(prev: DVec2, p: DVec2, next: DVec2) -> bool {
    cross(prev, p, next).abs() <= ON_LINE * (p - prev).length().max(1.0) * (next - p).length().max(1.0)
}

fn on_segment(a: DVec2, b: DVec2, p: DVec2) -> bool {
    let slack = MERGE_DISTANCE;
    p.x >= a.x.min(b.x) - slack && p.x <= a.x.max(b.x) + slack && p.y >= a.y.min(b.y) - slack && p.y <= a.y.max(b.y) + slack
}

/// Whether two segments share a point, by crossing, touching or overlapping.
fn segments_meet(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool {
    let side = |p: DVec2, q: DVec2, r: DVec2| {
        let x = cross(p, q, r);
        if x.abs() <= ON_LINE * (q - p).length().max(1.0) {
            0
        } else if x > 0.0 {
            1
        } else {
            -1
        }
    };
    let (o1, o2, o3, o4) = (side(a, b, c), side(a, b, d), side(c, d, a), side(c, d, b));
    (o1 * o2 < 0 && o3 * o4 < 0)
        || (o1 == 0 && on_segment(a, b, c))
        || (o2 == 0 && on_segment(a, b, d))
        || (o3 == 0 && on_segment(c, d, a))
        || (o4 == 0 && on_segment(c, d, b))
}

/// The outline without repeated corners (the closing one too) and without corners on a straight run, wound
/// counter-clockwise whichever way it was given. Refuses an outline that crosses or touches itself.
pub fn clean(points: &[DVec2]) -> Result<Vec<DVec2>, OutlineError> {
    if points.len() > MAX_CORNERS {
        return Err(OutlineError::TooMany);
    }

    let mut poly: Vec<DVec2> = Vec::with_capacity(points.len());
    for p in points {
        if poly.last().is_none_or(|q| (*q - *p).length() > MERGE_DISTANCE) {
            poly.push(*p);
        }
    }

    while poly.len() > 1 && (poly[0] - poly[poly.len() - 1]).length() <= MERGE_DISTANCE {
        poly.pop();
    }

    if poly.len() < 3 {
        return Err(OutlineError::TooFew);
    }

    let n = poly.len();
    let run = |poly: &[DVec2], i: usize| straight(poly[(i + poly.len() - 1) % poly.len()], poly[i], poly[(i + 1) % poly.len()]);
    if (0..n).all(|i| run(&poly, i)) {
        return Err(OutlineError::Flat);
    }

    for i in 0..n {
        let (a, b, next) = (poly[i], poly[(i + 1) % n], poly[(i + 2) % n]);
        // An edge that doubles back along its neighbour folds the outline onto itself.
        if straight(a, b, next) && (b - a).dot(next - b) < 0.0 {
            return Err(OutlineError::SelfIntersecting);
        }

        for j in i + 2..n {
            if (j + 1) % n != i && segments_meet(a, b, poly[j], poly[(j + 1) % n]) {
                return Err(OutlineError::SelfIntersecting);
            }
        }
    }

    while let Some(i) = (0..poly.len()).find(|&i| run(&poly, i)) {
        poly.remove(i);
    }

    let area = signed_area(&poly);
    if poly.len() < 3 || area.abs() < 1e-6 {
        return Err(OutlineError::Flat);
    }

    if area < 0.0 {
        poly.reverse();
    }

    Ok(poly)
}

fn is_convex(poly: &[DVec2]) -> bool {
    let n = poly.len();
    (0..n).all(|i| cross(poly[i], poly[(i + 1) % n], poly[(i + 2) % n]) >= -ON_LINE)
}

/// The outline cut into convex pieces, each wound counter-clockwise. A convex outline stays whole. Ear clipping makes
/// triangles, then Hertel-Mehlhorn merges neighbours while the result stays convex, which leaves at most four times
/// the fewest possible pieces.
pub fn convex_pieces(points: &[DVec2]) -> Result<Vec<Vec<DVec2>>, OutlineError> {
    let poly = clean(points)?;
    if is_convex(&poly) {
        return Ok(vec![poly]);
    }

    let flat: Vec<DVec3> = poly.iter().map(|p| p.extend(0.0)).collect();
    let mut pieces: Vec<Vec<usize>> =
        polygon::triangulate(&flat, DVec3::Z).into_iter().filter(|t| cross(poly[t[0]], poly[t[1]], poly[t[2]]).abs() > 1e-9).map(|t| t.to_vec()).collect();
    loop {
        let mut merged = None;
        'search: for i in 0..pieces.len() {
            for j in i + 1..pieces.len() {
                if let Some(joined) = join(&pieces[i], &pieces[j])
                    && is_convex(&joined.iter().map(|&k| poly[k]).collect::<Vec<_>>())
                {
                    merged = Some((i, j, joined));
                    break 'search;
                }
            }
        }

        let Some((i, j, joined)) = merged else { break };
        pieces[i] = joined;
        pieces.remove(j);
    }

    Ok(pieces.into_iter().map(|piece| piece.into_iter().map(|k| poly[k]).collect()).collect())
}

/// Two counter-clockwise polygons glued along an edge they share, if they have one.
fn join(p: &[usize], q: &[usize]) -> Option<Vec<usize>> {
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        let Some(k) = (0..q.len()).find(|&k| q[k] == b && q[(k + 1) % q.len()] == a) else { continue };
        let mut out: Vec<usize> = (1..=p.len()).map(|s| p[(i + s) % p.len()]).collect();
        out.extend((2..q.len()).map(|s| q[(k + s) % q.len()]));
        return Some(out);
    }

    None
}

/// The prism of a convex counter-clockwise polygon in the plane spanned by `u` and `v` (`u` x `v` = `normal`) through
/// `origin`, running `height` along `normal`. Faces get the default alignment.
fn prism(poly: &[DVec2], origin: DVec3, (u, v): (DVec3, DVec3), normal: DVec3, height: f64, material: &str) -> Result<Brush, OutlineError> {
    let at = |p: DVec2| origin + u * p.x + v * p.y;
    let mut planes = Vec::with_capacity(poly.len() + 2);
    let mut face = |n: DVec3, through: DVec3| planes.push((Plane::from_point_normal(through, n), FaceData::with_normal(material, n)));
    face(-normal, origin);
    face(normal, origin + normal * height);
    for i in 0..poly.len() {
        let (a, b) = (at(poly[i]), at(poly[(i + 1) % poly.len()]));
        face((b - a).cross(normal).normalize(), a);
    }

    Brush::from_planes(planes).map_err(|_| OutlineError::BadBrush)
}

/// Brushes for an outline of points in 3D. The points lie on one plane across `normal`, and the prism runs from that
/// plane `height` units along `normal`, or against it for a negative height. A convex outline makes one brush, a
/// concave one a brush per convex piece. The winding of the points does not matter.
pub fn extrude(points: &[DVec3], normal: DVec3, height: f64, material: &str) -> Result<Vec<Brush>, OutlineError> {
    let normal = normal.try_normalize().ok_or(OutlineError::NotPlanar)?;
    if height.is_nan() || height.abs() < 1e-3 {
        return Err(OutlineError::NoHeight);
    }

    let Some(first) = points.first() else { return Err(OutlineError::TooFew) };
    if points.iter().any(|p| (*p - *first).dot(normal).abs() > 1e-3) {
        return Err(OutlineError::NotPlanar);
    }

    let basis = Plane::new(normal, 0.0).basis();
    let flat: Vec<DVec2> = points.iter().map(|p| DVec2::new(p.dot(basis.0), p.dot(basis.1))).collect();
    let base = normal * first.dot(normal);
    let (origin, height) = if height < 0.0 { (base + normal * height, -height) } else { (base, height) };
    convex_pieces(&flat)?.iter().map(|piece| prism(piece, origin, basis, normal, height, material)).collect()
}

/// Which side of the drawn path a wall lies on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WallAlign {
    Center,
    /// On the side the path turns toward, inside the room for a closed path.
    Inside,
    #[default]
    Outside,
}

impl WallAlign {
    pub const ALL: [WallAlign; 3] = [WallAlign::Outside, WallAlign::Center, WallAlign::Inside];

    pub fn parse(name: &str) -> Option<WallAlign> {
        Self::ALL.into_iter().find(|a| a.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            WallAlign::Center => "center",
            WallAlign::Inside => "inside",
            WallAlign::Outside => "outside",
        }
    }
}

/// Wall brushes along a path of points in 3D, one per segment, `thickness` wide on the side of the path `align` picks.
/// The points lie on one plane across `normal` and the walls run `height` along it. Corners are mitred so neighbouring
/// walls meet without gaps or overlap, a corner sharper than about 30 degrees gets square ends instead. A closed path
/// also joins the last point back to the first.
pub fn walls(points: &[DVec3], closed: bool, normal: DVec3, height: f64, thickness: f64, align: WallAlign, material: &str) -> Result<Vec<Brush>, OutlineError> {
    // Below this cosine of half the turn the mitre would reach more than four half thicknesses out.
    const MITRE_LIMIT: f64 = 0.25;
    let normal = normal.try_normalize().ok_or(OutlineError::NotPlanar)?;
    let Some(first) = points.first() else { return Err(OutlineError::TooFew) };
    if points.iter().any(|p| (*p - *first).dot(normal).abs() > 1e-3) {
        return Err(OutlineError::NotPlanar);
    }

    if thickness.is_nan() || thickness < 1e-3 {
        return Err(OutlineError::Flat);
    }

    let (u, v) = Plane::new(normal, 0.0).basis();
    let base = normal * first.dot(normal);
    let mut flat: Vec<DVec2> = points.iter().map(|p| DVec2::new(p.dot(u), p.dot(v))).collect();
    flat.dedup_by(|a, b| a.distance(*b) < MERGE_DISTANCE);
    if closed && flat.len() > 1 && flat[0].distance(flat[flat.len() - 1]) < MERGE_DISTANCE {
        flat.pop();
    }

    let n = flat.len();
    if n < 2 || (closed && n < 3) {
        return Err(OutlineError::TooFew);
    }

    if n > MAX_CORNERS {
        return Err(OutlineError::TooMany);
    }

    let half = thickness / 2.0;
    let side = |i: usize| (flat[(i + 1) % n] - flat[i]).normalize().perp();
    let square = |i: usize| side(i) * half;
    let offset = |corner: usize, into: usize, out: usize, own: usize| {
        let open_end = !closed && (corner == 0 || corner == n - 1);
        let m = side(into) + side(out);
        let cos_half = m.length() / 2.0;
        if open_end || cos_half < MITRE_LIMIT { square(own) } else { m.normalize() * (half / cos_half) }
    };
    let turn = if closed { signed_area(&flat) } else { (1..n - 1).map(|i| (flat[i] - flat[i - 1]).perp_dot(flat[i + 1] - flat[i])).sum() };
    let inside = if turn < 0.0 { -1.0 } else { 1.0 };
    let shift = match align {
        WallAlign::Center => 0.0,
        WallAlign::Inside => inside,
        WallAlign::Outside => -inside,
    };
    let at = |p: DVec2| base + u * p.x + v * p.y;
    let segments = if closed { n } else { n - 1 };
    let mut out = Vec::with_capacity(segments);
    for i in 0..segments {
        let j = (i + 1) % n;
        let (s, e) = (flat[i], flat[j]);
        let (os, oe) = (offset(i, (i + n - 1) % n, i, i), offset(j, i, j, i));
        let quad = |os: DVec2, oe: DVec2| [s - os * (1.0 - shift), e - oe * (1.0 - shift), e + oe * (1.0 + shift), s + os * (1.0 + shift)].map(at);
        let brushes = extrude(&quad(os, oe), normal, height, material).or_else(|e| match e {
            OutlineError::NoHeight => Err(e),
            _ => extrude(&quad(square(i), square(i)), normal, height, material),
        })?;
        out.extend(brushes);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(list: &[(f64, f64)]) -> Vec<DVec2> {
        list.iter().map(|(x, y)| DVec2::new(*x, *y)).collect()
    }

    fn lift(points: &[DVec2], z: f64) -> Vec<DVec3> {
        points.iter().map(|p| DVec3::new(p.x, p.y, z)).collect()
    }

    fn l_shape() -> Vec<DVec2> {
        pts(&[(0.0, 0.0), (64.0, 0.0), (64.0, 32.0), (32.0, 32.0), (32.0, 64.0), (0.0, 64.0)])
    }

    fn u_shape() -> Vec<DVec2> {
        pts(&[(0.0, 0.0), (96.0, 0.0), (96.0, 64.0), (64.0, 64.0), (64.0, 32.0), (32.0, 32.0), (32.0, 64.0), (0.0, 64.0)])
    }

    fn volume(brushes: &[Brush]) -> f64 {
        brushes.iter().map(|b| b.volume()).sum()
    }

    #[test]
    fn convex_outline_is_one_brush() {
        let square = pts(&[(0.0, 0.0), (64.0, 0.0), (64.0, 64.0), (0.0, 64.0)]);
        let brushes = extrude(&lift(&square, 0.0), DVec3::Z, 32.0, "m").unwrap();
        assert_eq!(brushes.len(), 1);
        brushes[0].validate().unwrap();
        assert_eq!(brushes[0].faces.len(), 6);
        assert!((brushes[0].volume() - 64.0 * 64.0 * 32.0).abs() < 1e-6);
        assert_eq!(brushes[0].bounds().min, DVec3::ZERO);
        assert_eq!(brushes[0].bounds().max, DVec3::new(64.0, 64.0, 32.0));
    }

    #[test]
    fn concave_shapes_split_into_convex_pieces_of_the_same_volume() {
        for (shape, area, most) in [(l_shape(), 64.0 * 64.0 - 32.0 * 32.0, 2), (u_shape(), 96.0 * 64.0 - 32.0 * 32.0, 3)] {
            let pieces = convex_pieces(&shape).unwrap();
            assert!(pieces.len() > 1 && pieces.len() <= most, "{} pieces", pieces.len());
            assert!(pieces.iter().all(|p| is_convex(p)));
            let brushes = extrude(&lift(&shape, 0.0), DVec3::Z, 16.0, "m").unwrap();
            assert_eq!(brushes.len(), pieces.len());
            brushes.iter().for_each(|b| b.validate().unwrap());
            assert!((volume(&brushes) - area * 16.0).abs() < 1e-6, "{} against {}", volume(&brushes), area * 16.0);
        }
    }

    #[test]
    fn winding_does_not_matter() {
        for shape in [l_shape(), u_shape()] {
            let mut reversed = shape.clone();
            reversed.reverse();
            let a = extrude(&lift(&shape, 0.0), DVec3::Z, 16.0, "m").unwrap();
            let b = extrude(&lift(&reversed, 0.0), DVec3::Z, 16.0, "m").unwrap();
            assert_eq!(a.len(), b.len());
            assert!((volume(&a) - volume(&b)).abs() < 1e-6);
            b.iter().for_each(|b| b.validate().unwrap());
        }
    }

    #[test]
    fn repeated_and_collinear_corners_are_cleaned() {
        let messy = pts(&[(0.0, 0.0), (32.0, 0.0), (32.0, 0.0), (64.0, 0.0), (64.0, 32.0), (64.0, 64.0), (0.0, 64.0), (0.0, 0.0)]);
        let clean = clean(&messy).unwrap();
        assert_eq!(clean.len(), 4);
        assert!(signed_area(&clean) > 0.0);
        let brushes = extrude(&lift(&messy, 0.0), DVec3::Z, 8.0, "m").unwrap();
        assert_eq!(brushes.len(), 1);
        assert!((volume(&brushes) - 64.0 * 64.0 * 8.0).abs() < 1e-6);
    }

    #[test]
    fn degenerate_outlines_are_refused() {
        assert_eq!(clean(&pts(&[(0.0, 0.0), (1.0, 1.0)])), Err(OutlineError::TooFew));
        assert_eq!(clean(&pts(&[(0.0, 0.0), (0.0, 0.0), (8.0, 8.0), (8.0, 8.0)])), Err(OutlineError::TooFew));
        assert_eq!(clean(&pts(&[(0.0, 0.0), (16.0, 16.0), (32.0, 32.0)])), Err(OutlineError::Flat));
        assert_eq!(clean(&vec![DVec2::ZERO; MAX_CORNERS + 1]), Err(OutlineError::TooMany));
    }

    #[test]
    fn self_intersection_is_refused() {
        let bow_tie = pts(&[(0.0, 0.0), (64.0, 64.0), (64.0, 0.0), (0.0, 64.0)]);
        assert_eq!(clean(&bow_tie), Err(OutlineError::SelfIntersecting));
        assert_eq!(extrude(&lift(&bow_tie, 0.0), DVec3::Z, 16.0, "m"), Err(OutlineError::SelfIntersecting));
        let touching = pts(&[(0.0, 0.0), (64.0, 0.0), (64.0, 64.0), (32.0, 0.0), (0.0, 64.0)]);
        assert_eq!(clean(&touching), Err(OutlineError::SelfIntersecting));
        let spike = pts(&[(0.0, 0.0), (64.0, 0.0), (32.0, 0.0), (32.0, 64.0)]);
        assert_eq!(clean(&spike), Err(OutlineError::SelfIntersecting));
    }

    #[test]
    fn extrudes_along_any_axis_and_both_ways() {
        let tri = pts(&[(0.0, 0.0), (64.0, 0.0), (0.0, 64.0)]);
        let on_y: Vec<DVec3> = tri.iter().map(|p| DVec3::new(p.x, 32.0, p.y)).collect();
        for (normal, height, lo, hi) in [(DVec3::Y, 16.0, 32.0, 48.0), (DVec3::Y, -16.0, 16.0, 32.0), (DVec3::NEG_Y, 16.0, 16.0, 32.0)] {
            let brushes = extrude(&on_y, normal, height, "m").unwrap();
            assert_eq!(brushes.len(), 1);
            let bounds = brushes[0].bounds();
            assert!((bounds.min.y - lo).abs() < 1e-6 && (bounds.max.y - hi).abs() < 1e-6, "{bounds:?}");
            assert!((brushes[0].volume() - 64.0 * 64.0 / 2.0 * 16.0).abs() < 1e-6);
        }

        let tilted = DVec3::new(1.0, 1.0, 0.0).normalize();
        let (u, v) = Plane::new(tilted, 0.0).basis();
        let on_plane: Vec<DVec3> = tri.iter().map(|p| tilted * 10.0 + u * p.x + v * p.y).collect();
        assert!((extrude(&on_plane, tilted, 8.0, "m").unwrap()[0].volume() - 64.0 * 64.0 / 2.0 * 8.0).abs() < 1e-6);
    }

    #[test]
    fn off_plane_points_and_flat_prisms_are_refused() {
        let mut tri = lift(&pts(&[(0.0, 0.0), (64.0, 0.0), (0.0, 64.0)]), 0.0);
        assert_eq!(extrude(&tri, DVec3::Z, 0.0, "m"), Err(OutlineError::NoHeight));
        tri[1].z = 8.0;
        assert_eq!(extrude(&tri, DVec3::Z, 16.0, "m"), Err(OutlineError::NotPlanar));
    }

    #[test]
    fn many_cornered_concave_outline_keeps_its_area() {
        // A comb with ten teeth.
        let mut comb = vec![(0.0, 0.0), (320.0, 0.0)];
        for i in (0..10).rev() {
            let x = i as f64 * 32.0;
            comb.extend([(x + 32.0, 64.0), (x + 24.0, 64.0), (x + 24.0, 16.0), (x + 8.0, 16.0), (x + 8.0, 64.0), (x, 64.0)]);
        }

        let shape = clean(&pts(&comb)).unwrap();
        let brushes = extrude(&lift(&shape, 0.0), DVec3::Z, 8.0, "m").unwrap();
        brushes.iter().for_each(|b| b.validate().unwrap());
        assert!((volume(&brushes) - signed_area(&shape) * 8.0).abs() < 1e-6);
    }

    #[test]
    fn walls_follow_an_open_path_with_mitred_corners() {
        let path = lift(&pts(&[(0.0, 0.0), (128.0, 0.0), (128.0, 128.0)]), 0.0);
        let brushes = walls(&path, false, DVec3::Z, 64.0, 16.0, WallAlign::Center, "wall").unwrap();
        assert_eq!(brushes.len(), 2);
        assert!((volume(&brushes) - 2.0 * 128.0 * 16.0 * 64.0).abs() < 1e-6, "the mitred walls meet without overlap");
        let b = brushes.iter().fold(td_core::Aabb::EMPTY, |b, w| b.union(&w.bounds()));
        assert_eq!((b.min, b.max), (DVec3::new(0.0, -8.0, 0.0), DVec3::new(136.0, 128.0, 64.0)), "open ends stay square");
    }

    #[test]
    fn a_closed_path_makes_a_ring_of_walls() {
        let ring = lift(&pts(&[(0.0, 0.0), (128.0, 0.0), (128.0, 128.0), (0.0, 128.0), (0.0, 0.0)]), 32.0);
        let brushes = walls(&ring, true, DVec3::Z, -32.0, 8.0, WallAlign::Center, "wall").unwrap();
        assert_eq!(brushes.len(), 4, "the repeated first point is dropped");
        assert!((volume(&brushes) - 4.0 * 128.0 * 8.0 * 32.0).abs() < 1e-6);
        assert!(brushes.iter().all(|w| w.bounds().min.z == 0.0 && w.bounds().max.z == 32.0));
    }

    #[test]
    fn walls_go_inside_or_outside_a_room_whatever_its_winding() {
        let mut room = pts(&[(0.0, 0.0), (128.0, 0.0), (128.0, 128.0), (0.0, 128.0)]);
        for _ in 0..2 {
            let ring = lift(&room, 0.0);
            let outside = walls(&ring, true, DVec3::Z, 64.0, 8.0, WallAlign::Outside, "w").unwrap();
            let b = outside.iter().fold(td_core::Aabb::EMPTY, |b, w| b.union(&w.bounds()));
            assert_eq!((b.min.x, b.max.x, b.min.y, b.max.y), (-8.0, 136.0, -8.0, 136.0));
            let inside = walls(&ring, true, DVec3::Z, 64.0, 8.0, WallAlign::Inside, "w").unwrap();
            let b = inside.iter().fold(td_core::Aabb::EMPTY, |b, w| b.union(&w.bounds()));
            assert_eq!((b.min.x, b.max.x), (0.0, 128.0));
            assert!((volume(&inside) - (128.0 * 128.0 - 112.0 * 112.0) * 64.0).abs() < 1e-6);
            room.reverse();
        }

        let l = lift(&pts(&[(0.0, 0.0), (128.0, 0.0), (128.0, 128.0)]), 0.0);
        let b = walls(&l, false, DVec3::Z, 64.0, 16.0, WallAlign::Outside, "w").unwrap()[0].bounds();
        assert_eq!((b.min.y, b.max.y), (-16.0, 0.0), "an open path's outside is away from its turn");
    }

    #[test]
    fn a_hairpin_corner_gets_square_ends() {
        let path = lift(&pts(&[(0.0, 0.0), (128.0, 0.0), (0.0, 16.0)]), 0.0);
        let brushes = walls(&path, false, DVec3::Z, 64.0, 16.0, WallAlign::Center, "wall").unwrap();
        assert_eq!(brushes.len(), 2);
        assert!(brushes.iter().all(|w| w.bounds().max.x < 129.0), "no spike out past the corner");
    }

    #[test]
    fn walls_reject_bad_paths() {
        let one = lift(&pts(&[(0.0, 0.0), (0.0, 0.0)]), 0.0);
        let two = lift(&pts(&[(0.0, 0.0), (64.0, 0.0)]), 0.0);
        assert_eq!(walls(&one, false, DVec3::Z, 64.0, 16.0, WallAlign::Center, "w").unwrap_err(), OutlineError::TooFew);
        assert_eq!(walls(&two, true, DVec3::Z, 64.0, 16.0, WallAlign::Center, "w").unwrap_err(), OutlineError::TooFew);
        assert_eq!(walls(&two, false, DVec3::Z, 64.0, 0.0, WallAlign::Center, "w").unwrap_err(), OutlineError::Flat);
        assert_eq!(walls(&two, false, DVec3::Z, 0.0, 16.0, WallAlign::Center, "w").unwrap_err(), OutlineError::NoHeight);
        assert_eq!(
            walls(&[DVec3::ZERO, DVec3::new(64.0, 0.0, 5.0)], false, DVec3::Z, 64.0, 16.0, WallAlign::Center, "w").unwrap_err(),
            OutlineError::NotPlanar
        );
    }
}
