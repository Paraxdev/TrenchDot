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
    /// A hole is not wholly inside the outline, or it meets another hole.
    BadHole,
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
            OutlineError::BadHole => "a hole reaches outside the outline or meets another hole",
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

/// Most holes one outline may have.
pub const MAX_HOLES: usize = 32;

fn inside(poly: &[DVec2], p: DVec2) -> bool {
    let n = poly.len();
    let mut odd = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            odd = !odd;
        }
    }

    odd
}

fn edges(poly: &[DVec2]) -> impl Iterator<Item = (DVec2, DVec2)> + '_ {
    (0..poly.len()).map(|i| (poly[i], poly[(i + 1) % poly.len()]))
}

/// Like [`convex_pieces`] for an outline with holes cut out of it. Each hole lies wholly inside the outline and apart
/// from the other holes. Every hole is bridged to the outline by a cut from its rightmost corner to the nearest corner
/// it can see, which leaves one polygon that touches itself only along the cuts, and that is cut up as usual.
pub fn convex_pieces_with_holes(points: &[DVec2], holes: &[Vec<DVec2>]) -> Result<Vec<Vec<DVec2>>, OutlineError> {
    if holes.is_empty() {
        return convex_pieces(points);
    }

    if holes.len() > MAX_HOLES {
        return Err(OutlineError::TooMany);
    }

    let outer = clean(points)?;
    let holes: Vec<Vec<DVec2>> = holes.iter().map(|h| clean(h).map_err(|_| OutlineError::BadHole)).collect::<Result<_, _>>()?;
    for (i, hole) in holes.iter().enumerate() {
        let crosses = |other: &[DVec2]| edges(hole).any(|(a, b)| edges(other).any(|(c, d)| segments_meet(a, b, c, d)));
        if !hole.iter().all(|p| inside(&outer, *p)) || crosses(&outer) {
            return Err(OutlineError::BadHole);
        }

        for other in &holes[i + 1..] {
            if crosses(other) || inside(other, hole[0]) || inside(hole, other[0]) {
                return Err(OutlineError::BadHole);
            }
        }
    }

    let mut order: Vec<usize> = (0..holes.len()).collect();
    let right = |h: &[DVec2]| (0..h.len()).max_by(|&a, &b| h[a].x.total_cmp(&h[b].x)).unwrap_or(0);
    order.sort_by(|&a, &b| holes[b][right(&holes[b])].x.total_cmp(&holes[a][right(&holes[a])].x));
    let mut ring = outer.clone();
    let mut cuts: Vec<(DVec2, DVec2)> = Vec::new();
    for &h in &order {
        let hole: Vec<DVec2> = holes[h].iter().rev().copied().collect();
        let m = right(&hole);
        let from = hole[m];
        let mut near: Vec<usize> = (0..ring.len()).collect();
        near.sort_by(|&a, &b| ring[a].distance_squared(from).total_cmp(&ring[b].distance_squared(from)));
        let sees = |to: DVec2| {
            let mid = (from + to) * 0.5;
            let blocked = |(c, d): (DVec2, DVec2)| c != from && d != from && c != to && d != to && segments_meet(from, to, c, d);
            inside(&outer, mid)
                && !holes.iter().any(|o| inside(o, mid))
                && !edges(&outer).chain(holes.iter().flat_map(|o| edges(o))).chain(cuts.iter().copied()).any(blocked)
        };
        let Some(j) = near.into_iter().find(|&j| sees(ring[j])) else { return Err(OutlineError::BadBrush) };
        cuts.push((from, ring[j]));
        let mut joined = ring[..=j].to_vec();
        joined.extend((0..=hole.len()).map(|k| hole[(m + k) % hole.len()]));
        joined.extend_from_slice(&ring[j..]);
        ring = joined;
    }

    let id: Vec<usize> = (0..ring.len()).map(|i| (0..=i).find(|&k| ring[k] == ring[i]).unwrap_or(i)).collect();
    let flat: Vec<DVec3> = ring.iter().map(|p| p.extend(0.0)).collect();
    let mut pieces: Vec<Vec<usize>> = polygon::triangulate(&flat, DVec3::Z)
        .into_iter()
        .filter(|t| cross(ring[t[0]], ring[t[1]], ring[t[2]]) > 1e-9)
        .map(|t| t.iter().map(|&k| id[k]).collect())
        .collect();
    let want = signed_area(&outer) - holes.iter().map(|h| signed_area(h)).sum::<f64>();
    let got: f64 = pieces.iter().map(|t| cross(ring[t[0]], ring[t[1]], ring[t[2]]) * 0.5).sum();
    if (got - want).abs() > 1e-6 * want.max(1.0) {
        return Err(OutlineError::BadBrush);
    }

    loop {
        let mut merged = None;
        'search: for i in 0..pieces.len() {
            for j in i + 1..pieces.len() {
                if let Some(joined) = join(&pieces[i], &pieces[j])
                    && joined.iter().enumerate().all(|(a, k)| !joined[a + 1..].contains(k))
                    && is_convex(&joined.iter().map(|&k| ring[k]).collect::<Vec<_>>())
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

    Ok(pieces.into_iter().map(|piece| piece.into_iter().map(|k| ring[k]).collect()).collect())
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
    extrude_with_holes(points, &[], normal, height, material)
}

/// Like [`extrude`] with `holes` cut through the prism, each an outline on the same plane inside `points`.
pub fn extrude_with_holes(points: &[DVec3], holes: &[Vec<DVec3>], normal: DVec3, height: f64, material: &str) -> Result<Vec<Brush>, OutlineError> {
    let normal = normal.try_normalize().ok_or(OutlineError::NotPlanar)?;
    if height.is_nan() || height.abs() < 1e-3 {
        return Err(OutlineError::NoHeight);
    }

    let Some(first) = points.first() else { return Err(OutlineError::TooFew) };
    if points.iter().chain(holes.iter().flatten()).any(|p| (*p - *first).dot(normal).abs() > 1e-3) {
        return Err(OutlineError::NotPlanar);
    }

    let basis = Plane::new(normal, 0.0).basis();
    let to_flat = |pts: &[DVec3]| pts.iter().map(|p| DVec2::new(p.dot(basis.0), p.dot(basis.1))).collect::<Vec<_>>();
    let flat_holes: Vec<Vec<DVec2>> = holes.iter().map(|h| to_flat(h)).collect();
    let base = normal * first.dot(normal);
    let (origin, height) = if height < 0.0 { (base + normal * height, -height) } else { (base, height) };
    convex_pieces_with_holes(&to_flat(points), &flat_holes)?.iter().map(|piece| prism(piece, origin, basis, normal, height, material)).collect()
}

/// Below this cosine of half the turn a mitre would reach more than four times the width out, so the corner gets
/// square ends instead.
const MITRE_LIMIT: f64 = 0.25;

/// A path of points flattened onto its plane, with repeats dropped.
struct Path {
    flat: Vec<DVec2>,
    normal: DVec3,
    u: DVec3,
    v: DVec3,
    base: DVec3,
}

impl Path {
    fn new(points: &[DVec3], closed: bool, normal: DVec3) -> Result<Path, OutlineError> {
        let normal = normal.try_normalize().ok_or(OutlineError::NotPlanar)?;
        let Some(first) = points.first() else { return Err(OutlineError::TooFew) };
        if points.iter().any(|p| (*p - *first).dot(normal).abs() > 1e-3) {
            return Err(OutlineError::NotPlanar);
        }

        let (u, v) = Plane::new(normal, 0.0).basis();
        let mut flat: Vec<DVec2> = points.iter().map(|p| DVec2::new(p.dot(u), p.dot(v))).collect();
        flat.dedup_by(|a, b| a.distance(*b) < MERGE_DISTANCE);
        if closed && flat.len() > 1 && flat[0].distance(flat[flat.len() - 1]) < MERGE_DISTANCE {
            flat.pop();
        }

        if flat.len() < 2 || (closed && flat.len() < 3) {
            return Err(OutlineError::TooFew);
        }

        if flat.len() > MAX_CORNERS {
            return Err(OutlineError::TooMany);
        }

        Ok(Path { flat, normal, u, v, base: normal * first.dot(normal) })
    }

    fn at(&self, p: DVec2) -> DVec3 {
        self.base + self.u * p.x + self.v * p.y
    }

    fn dir(&self, p: DVec2) -> DVec3 {
        self.u * p.x + self.v * p.y
    }
}

/// 1 when the inside of the path lies to the left of its segments, -1 when to the right. That is the side a closed
/// path winds around, and the side an open one turns toward overall.
fn inside_side(flat: &[DVec2], closed: bool) -> f64 {
    let n = flat.len();
    let turn = if closed { signed_area(flat) } else { (1..n - 1).map(|i| (flat[i] - flat[i - 1]).perp_dot(flat[i + 1] - flat[i])).sum() };
    if turn < 0.0 { -1.0 } else { 1.0 }
}

/// Most straight pieces one rounded corner is made of.
pub const MAX_ROUND_SEGMENTS: usize = 16;

/// The path with every corner replaced by an arc of `segments` straight pieces that touches both edges `radius` from
/// its centre. A corner whose edges are too short for the radius gets the biggest arc that takes at most half of each
/// edge, so neighbouring arcs never overlap. The ends of an open path and straight corners stay as they are.
pub fn round_corners(points: &[DVec3], closed: bool, radius: f64, segments: usize) -> Vec<DVec3> {
    let mut pts = points.to_vec();
    pts.dedup_by(|a, b| a.distance(*b) < MERGE_DISTANCE);
    if closed && pts.len() > 1 && pts[0].distance(pts[pts.len() - 1]) < MERGE_DISTANCE {
        pts.pop();
    }

    let n = pts.len();
    if radius.is_nan() || radius <= 0.0 || segments == 0 || n < 3 {
        return points.to_vec();
    }

    let segments = segments.min(MAX_ROUND_SEGMENTS);
    let mut out: Vec<DVec3> = Vec::with_capacity(n * (segments + 1));
    for i in 0..n {
        let p = pts[i];
        let (da, db) = (pts[(i + n - 1) % n] - p, pts[(i + 1) % n] - p);
        let half = da.angle_between(db) / 2.0;
        if (!closed && (i == 0 || i == n - 1)) || !(1e-3..std::f64::consts::FRAC_PI_2 - 1e-3).contains(&half) {
            out.push(p);
            continue;
        }

        let reach = (radius / half.tan()).min(da.length().min(db.length()) / 2.0);
        let r = reach * half.tan();
        let centre = p + (da.normalize() + db.normalize()).normalize() * (r / half.sin());
        let (s, e) = (p + da.normalize() * reach - centre, p + db.normalize() * reach - centre);
        let sweep = s.angle_between(e);
        for k in 0..=segments {
            let t = k as f64 / segments as f64;
            out.push(centre + (s * ((1.0 - t) * sweep).sin() + e * (t * sweep).sin()) / sweep.sin());
        }
    }

    out.dedup_by(|a, b| a.distance(*b) < MERGE_DISTANCE);
    if closed && out.len() > 1 && out[0].distance(out[out.len() - 1]) < MERGE_DISTANCE {
        out.pop();
    }

    out
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
    if thickness.is_nan() || thickness < 1e-3 {
        return Err(OutlineError::Flat);
    }

    let Path { flat, normal, u, v, base } = Path::new(points, closed, normal)?;
    let n = flat.len();
    let half = thickness / 2.0;
    let side = |i: usize| (flat[(i + 1) % n] - flat[i]).normalize().perp();
    let square = |i: usize| side(i) * half;
    let offset = |corner: usize, into: usize, out: usize, own: usize| {
        let open_end = !closed && (corner == 0 || corner == n - 1);
        let m = side(into) + side(out);
        let cos_half = m.length() / 2.0;
        if open_end || cos_half < MITRE_LIMIT { square(own) } else { m.normalize() * (half / cos_half) }
    };
    let inside = inside_side(&flat, closed);
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

/// Brushes of a raised strip along a path, like a pavement or a road: walls `width` wide centred on the path and
/// `height` tall. Where a segment overlaps an earlier one or one of `others`, the overlap is cut away, so a path that
/// crosses itself or runs into another strip meets it flush at a junction instead of overlapping it.
#[allow(clippy::too_many_arguments)]
pub fn strip(points: &[DVec3], closed: bool, normal: DVec3, width: f64, height: f64, others: &[&Brush], material: &str) -> Result<Vec<Brush>, OutlineError> {
    let segments = walls(points, closed, normal, height, width, WallAlign::Center, material)?;
    let mut out = Vec::with_capacity(segments.len());
    for (i, segment) in segments.iter().enumerate() {
        let mut pieces = vec![segment.clone()];
        for cutter in others.iter().copied().chain(&segments[..i]) {
            pieces = pieces.iter().flat_map(|p| crate::csg::subtract_with(p, cutter, crate::csg::CarveMaterial::Target)).collect();
        }

        out.extend(pieces);
    }

    Ok(out)
}

/// Ready made trim cross sections.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrimProfile {
    #[default]
    Skirting,
    Kerb,
    Cornice,
    Ledge,
}

impl TrimProfile {
    pub const ALL: [TrimProfile; 4] = [TrimProfile::Skirting, TrimProfile::Kerb, TrimProfile::Cornice, TrimProfile::Ledge];

    pub fn parse(name: &str) -> Option<TrimProfile> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            TrimProfile::Skirting => "skirting",
            TrimProfile::Kerb => "kerb",
            TrimProfile::Cornice => "cornice",
            TrimProfile::Ledge => "ledge",
        }
    }

    /// The cross section `depth` out from the path and `height` up, x away from the path and y up.
    pub fn points(self, depth: f64, height: f64) -> Vec<DVec2> {
        let unit: &[(f64, f64)] = match self {
            TrimProfile::Skirting => &[(0.0, 0.0), (1.0, 0.0), (1.0, 0.7), (0.4, 1.0), (0.0, 1.0)],
            TrimProfile::Kerb => &[(0.0, 0.0), (1.0, 0.0), (1.0, 0.75), (0.75, 1.0), (0.0, 1.0)],
            TrimProfile::Cornice => &[(0.0, 0.0), (0.3, 0.0), (0.3, 0.3), (0.6, 0.3), (0.6, 0.6), (1.0, 0.6), (1.0, 1.0), (0.0, 1.0)],
            TrimProfile::Ledge => &[(0.0, 0.0), (0.2, 0.0), (1.0, 0.8), (1.0, 1.0), (0.0, 1.0)],
        };
        unit.iter().map(|(x, y)| DVec2::new(x * depth, y * height)).collect()
    }
}

/// Trim brushes along a path of points in 3D: the `profile` cross section, x away from the path and y along `normal`,
/// swept along every segment and mitred at the corners like [`walls`]. Inside and outside put the profile's x on that
/// side of the path, center centres its x range on the path facing outside. A concave profile makes a brush per
/// convex piece and segment. A closed path also joins the last point back to the first.
pub fn trim(points: &[DVec3], closed: bool, normal: DVec3, profile: &[DVec2], align: WallAlign, material: &str) -> Result<Vec<Brush>, OutlineError> {
    let path = Path::new(points, closed, normal)?;
    let mut pieces = convex_pieces(profile)?;
    if align == WallAlign::Center {
        let (lo, hi) = pieces.iter().flatten().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.x), hi.max(p.x)));
        pieces.iter_mut().flatten().for_each(|p| p.x -= (lo + hi) / 2.0);
    }

    let flat = &path.flat;
    let n = flat.len();
    let out_side = if align == WallAlign::Inside { 1.0 } else { -1.0 } * inside_side(flat, closed);
    let dir = |i: usize| (flat[(i + 1) % n] - flat[i]).normalize();
    let cap = |corner: usize, into: usize, out: usize, own: DVec2| {
        let open_end = !closed && (corner == 0 || corner == n - 1);
        let m = dir(into) + dir(out);
        if open_end || m.length() / 2.0 < MITRE_LIMIT { own } else { m.normalize() }
    };
    let segments = if closed { n } else { n - 1 };
    let mut out = Vec::with_capacity(segments * pieces.len());
    for i in 0..segments {
        let j = (i + 1) % n;
        let d = dir(i);
        let (s, e, side) = (path.at(flat[i]), path.at(flat[j]), path.dir(d.perp() * out_side));
        let caps = [(cap(i, (i + n - 1) % n, i, d), cap(j, i, j, d)), (d, d)];
        for piece in &pieces {
            let brush = |(start, end): (DVec2, DVec2)| {
                let mut planes = Vec::with_capacity(piece.len() + 2);
                let mut face = |n: DVec3, through: DVec3| planes.push((Plane::from_point_normal(through, n), FaceData::with_normal(material, n)));
                face(-path.dir(start), s);
                face(path.dir(end), e);
                for k in 0..piece.len() {
                    let (a, b) = (piece[k], piece[(k + 1) % piece.len()]);
                    face((side * (b.y - a.y) - path.normal * (b.x - a.x)).normalize(), s + side * a.x + path.normal * a.y);
                }

                Brush::from_planes(planes).ok().filter(|b| b.volume() > 1e-6)
            };
            out.push(caps.into_iter().find_map(brush).ok_or(OutlineError::BadBrush)?);
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use td_core::Aabb;

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

    fn square(x: f64, y: f64, size: f64) -> Vec<DVec2> {
        pts(&[(x, y), (x + size, y), (x + size, y + size), (x, y + size)])
    }

    #[test]
    fn holes_cut_through_and_keep_the_volume_of_what_is_left() {
        let outer = square(0.0, 0.0, 256.0);
        let one = vec![square(96.0, 96.0, 64.0)];
        let two = vec![square(32.0, 32.0, 64.0), pts(&[(160.0, 160.0), (224.0, 192.0), (176.0, 224.0)])];
        for holes in [one, two] {
            let pieces = convex_pieces_with_holes(&outer, &holes).unwrap();
            assert!(pieces.iter().all(|p| is_convex(p) && signed_area(p) > 0.0));
            let area = 256.0 * 256.0 - holes.iter().map(|h| signed_area(&clean(h).unwrap())).sum::<f64>();
            assert!((pieces.iter().map(|p| signed_area(p)).sum::<f64>() - area).abs() < 1e-6);
            let flat_holes: Vec<Vec<DVec3>> = holes.iter().map(|h| lift(h, 0.0)).collect();
            let brushes = extrude_with_holes(&lift(&outer, 0.0), &flat_holes, DVec3::Z, 16.0, "m").unwrap();
            assert!((volume(&brushes) - area * 16.0).abs() < 1e-3);
            for hole in &holes {
                let centre = hole.iter().fold(DVec2::ZERO, |a, p| a + *p) / hole.len() as f64;
                assert!(!brushes.iter().any(|b| b.contains_point(centre.extend(8.0))), "the hole at {centre} is filled");
            }
        }

        let ring = convex_pieces_with_holes(&outer, &[square(1.0, 1.0, 254.0)]).unwrap();
        assert!((ring.iter().map(|p| signed_area(p)).sum::<f64>() - (256.0 * 256.0 - 254.0 * 254.0)).abs() < 1e-6);
    }

    #[test]
    fn strips_meet_flush_at_junctions() {
        let disjoint = |b: &[Brush]| (0..b.len()).all(|i| (i + 1..b.len()).all(|j| !b[i].intersects(&b[j])));
        let road = strip(&lift(&pts(&[(0.0, 0.0), (256.0, 0.0)]), 0.0), false, DVec3::Z, 32.0, 8.0, &[], "m").unwrap();
        let others: Vec<&Brush> = road.iter().collect();
        let branch = strip(&lift(&pts(&[(128.0, 0.0), (128.0, 128.0)]), 0.0), false, DVec3::Z, 32.0, 8.0, &others, "m").unwrap();
        assert!((volume(&branch) - 112.0 * 32.0 * 8.0).abs() < 1e-6, "the branch stops at the road's side");
        let crossing = strip(&lift(&pts(&[(64.0, -128.0), (64.0, 128.0)]), 0.0), false, DVec3::Z, 32.0, 8.0, &others, "m").unwrap();
        assert_eq!(crossing.len(), 2, "a crossing path is split around the road");
        assert!(disjoint(&[road.clone(), branch, crossing].concat()));

        let looped =
            strip(&lift(&pts(&[(0.0, 0.0), (128.0, 0.0), (128.0, 128.0), (64.0, 128.0), (64.0, -64.0)]), 0.0), false, DVec3::Z, 16.0, 8.0, &[], "m").unwrap();
        assert!(disjoint(&looped), "a path crossing itself does not overlap");
    }

    #[test]
    fn rounded_corners_follow_an_arc_and_never_overlap() {
        let r = 32.0;
        let rounded = round_corners(&lift(&square(0.0, 0.0, 128.0), 0.0), true, r, 8);
        assert_eq!(rounded.len(), 36);
        let flat: Vec<DVec2> = rounded.iter().map(|p| p.truncate()).collect();
        let cut = r * r - 4.0 * r * r * (std::f64::consts::PI / 16.0).sin();
        assert!((signed_area(&flat) - (128.0 * 128.0 - 4.0 * cut)).abs() < 1e-6);
        let brushes = extrude(&rounded, DVec3::Z, 16.0, "m").unwrap();
        assert_eq!(brushes.len(), 1);

        let circle = round_corners(&lift(&square(0.0, 0.0, 128.0), 0.0), true, 1000.0, 8);
        assert_eq!(circle.len(), 32);
        assert!(circle.iter().all(|p| (p.truncate().distance(DVec2::splat(64.0)) - 64.0).abs() < 1e-9));

        let path = lift(&pts(&[(0.0, 0.0), (128.0, 0.0), (128.0, 128.0)]), 8.0);
        let bent = round_corners(&path, false, 16.0, 4);
        assert_eq!((bent.len(), bent[0], bent[bent.len() - 1]), (7, path[0], path[2]));
        assert!(bent.iter().all(|p| p.z == 8.0));
        assert_eq!(walls(&bent, false, DVec3::Z, 64.0, 8.0, WallAlign::Center, "m").unwrap().len(), 6);
        assert_eq!(round_corners(&path, false, 0.0, 4), path);
    }

    #[test]
    fn holes_must_sit_inside_and_apart() {
        let outer = square(0.0, 0.0, 128.0);
        for holes in [
            vec![square(100.0, 32.0, 64.0)],
            vec![square(200.0, 32.0, 16.0)],
            vec![square(16.0, 16.0, 64.0), square(48.0, 48.0, 64.0)],
            vec![square(16.0, 16.0, 96.0), square(48.0, 48.0, 16.0)],
            vec![square(0.0, 32.0, 32.0)],
        ] {
            assert_eq!(convex_pieces_with_holes(&outer, &holes), Err(OutlineError::BadHole), "{holes:?}");
        }
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

    #[test]
    fn a_square_trim_profile_matches_the_walls() {
        let path = lift(&pts(&[(0.0, 0.0), (128.0, 0.0), (128.0, 96.0), (32.0, 160.0)]), 0.0);
        let square = pts(&[(0.0, 0.0), (16.0, 0.0), (16.0, 64.0), (0.0, 64.0)]);
        for closed in [false, true] {
            for align in WallAlign::ALL {
                let t = trim(&path, closed, DVec3::Z, &square, align, "t").unwrap();
                let w = walls(&path, closed, DVec3::Z, 64.0, 16.0, align, "w").unwrap();
                assert_eq!(t.len(), w.len());
                for (a, b) in t.iter().zip(&w) {
                    a.validate().unwrap();
                    assert!((a.volume() - b.volume()).abs() < 1e-3, "{align:?} closed {closed}: {} against {}", a.volume(), b.volume());
                    assert!((a.bounds().min - b.bounds().min).length() < 1e-3 && (a.bounds().max - b.bounds().max).length() < 1e-3);
                }
            }
        }
    }

    #[test]
    fn a_concave_trim_around_a_room_keeps_its_volume() {
        let room = lift(&pts(&[(0.0, 0.0), (256.0, 0.0), (256.0, 256.0), (0.0, 256.0)]), 32.0);
        let profile = TrimProfile::Cornice.points(10.0, 20.0);
        let pieces = convex_pieces(&profile).unwrap();
        assert!(pieces.len() > 1);
        let (area, moment) = pieces.iter().flat_map(|p| (0..p.len()).map(|i| (p[i], p[(i + 1) % p.len()]))).fold((0.0, 0.0), |(a, m), (p, q)| {
            let c = p.perp_dot(q);
            (a + c / 2.0, m + (p.x + q.x) * c / 6.0)
        });
        for (align, centroid) in [(WallAlign::Inside, -moment / area), (WallAlign::Outside, moment / area)] {
            let brushes = trim(&room, true, DVec3::Z, &profile, align, "t").unwrap();
            assert_eq!(brushes.len(), 4 * pieces.len());
            brushes.iter().for_each(|b| b.validate().unwrap());
            let expected = area * 4.0 * (256.0 + 2.0 * centroid);
            assert!((volume(&brushes) - expected).abs() < 1e-3, "{align:?}: {} against {expected}", volume(&brushes));
            let b = brushes.iter().fold(Aabb::EMPTY, |b, brush| b.union(&brush.bounds()));
            assert_eq!((b.min.z, b.max.z), (32.0, 52.0));
            let reach = if align == WallAlign::Inside { 0.0 } else { 10.0 };
            assert!((b.min.x + reach).abs() < 1e-6 && (b.max.x - 256.0 - reach).abs() < 1e-6, "{align:?} {b:?}");
        }
    }

    #[test]
    fn trims_reject_bad_profiles_and_paths() {
        let two = lift(&pts(&[(0.0, 0.0), (64.0, 0.0)]), 0.0);
        let profile = TrimProfile::Skirting.points(4.0, 8.0);
        assert_eq!(trim(&two, false, DVec3::Z, &profile, WallAlign::Inside, "t").unwrap().len(), 1);
        assert_eq!(trim(&two[..1], false, DVec3::Z, &profile, WallAlign::Inside, "t").unwrap_err(), OutlineError::TooFew);
        assert_eq!(trim(&two, false, DVec3::Z, &pts(&[(0.0, 0.0), (4.0, 4.0), (8.0, 8.0)]), WallAlign::Inside, "t").unwrap_err(), OutlineError::Flat);
        for p in TrimProfile::ALL {
            assert_eq!(TrimProfile::parse(p.name()), Some(p));
            assert!(signed_area(&p.points(1.0, 1.0)) > 0.0);
        }
    }
}
