//! Sweeping brush faces along a path into a run of convex brush segments.

use td_core::{Aabb, DMat4, DQuat, DVec3, snap_vec};

use crate::brush::Brush;

pub const MAX_SEGMENTS: u32 = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SweepPath {
    #[default]
    Straight,
    Arc,
    SBend,
    /// Through a list of points, see [`Curve`].
    Curve,
}

/// Which way an arc turns or an S-bend shifts, seen looking at the faces from outside. On a floor or ceiling face up
/// points along -Z.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bend {
    Up,
    Down,
    Left,
    #[default]
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sweep {
    pub path: SweepPath,
    /// How far the faces travel along their normal for Straight and S-bend, the radius of the arc's center line for Arc.
    pub length: f64,
    /// Degrees the arc turns through.
    pub angle: f64,
    /// How far an S-bend shifts sideways.
    pub offset: f64,
    pub bend: Bend,
    pub segments: u32,
    /// Height gained along +Y over the whole sweep, which turns an arc into a spiral.
    pub rise: f64,
    /// Degrees the faces turn around their own normal over the whole sweep.
    pub twist: f64,
    /// Size of the last ring against the first.
    pub end_scale: f64,
}

impl Default for Sweep {
    fn default() -> Self {
        Self { path: SweepPath::Straight, length: 128.0, angle: 90.0, offset: 64.0, bend: Bend::Right, segments: 8, rise: 0.0, twist: 0.0, end_scale: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SweepError {
    NoFaces,
    /// The face normals cancel out, so there is no direction to sweep in.
    Facing,
    Segments,
    EndScale,
    Length,
    Angle,
    /// The arc radius has to be more than this.
    Radius(f64),
    /// A curve needs at least one point away from the faces.
    NoPoints,
    /// A bend in the curve is tighter than the faces reach.
    Bend,
    /// A segment came out without volume.
    Flat,
}

impl std::fmt::Display for SweepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SweepError::NoFaces => write!(f, "pick at least one brush face"),
            SweepError::Facing => write!(f, "the faces point in opposite directions, pick faces that face the same way"),
            SweepError::Segments => write!(f, "segments must be from 1 to {MAX_SEGMENTS}"),
            SweepError::EndScale => write!(f, "end_scale must be more than 0"),
            SweepError::Length => write!(f, "length must be more than 0"),
            SweepError::Angle => write!(f, "angle must be more than 0 and at most 720 degrees, bend picks the side it turns to"),
            SweepError::Radius(min) => write!(f, "the arc radius must be more than {}, or the inner side of the faces folds over", min.ceil()),
            SweepError::NoPoints => write!(f, "a curve needs at least one point away from the faces"),
            SweepError::Bend => {
                write!(f, "a bend in the curve is tighter than the faces reach, so their inner side folds over, move the points further apart")
            }
            SweepError::Flat => write!(f, "a segment has no volume, try fewer segments or less twist"),
        }
    }
}

/// A brush face to sweep, which keeps its brush for the materials of the new faces.
#[derive(Clone, Debug)]
pub struct SweepFace {
    pub brush: Brush,
    pub face: usize,
}

/// The frame every swept face shares, so faces picked side by side bend around one pivot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub center: DVec3,
    pub normal: DVec3,
    pub up: DVec3,
    pub right: DVec3,
}

impl Frame {
    pub fn of(faces: &[SweepFace]) -> Option<Frame> {
        let normal = faces.iter().map(|f| f.brush.faces.get(f.face).map_or(DVec3::ZERO, |face| face.plane.normal)).sum::<DVec3>().try_normalize()?;
        let center = Aabb::from_points(faces.iter().flat_map(|f| f.brush.face_points(f.face))).center();
        let world = if normal.y.abs() > 0.9 { DVec3::NEG_Z } else { DVec3::Y };
        let up = (world - normal * world.dot(normal)).normalize();
        Some(Frame { center, normal, up, right: up.cross(normal) })
    }

    fn towards(&self, bend: Bend) -> DVec3 {
        match bend {
            Bend::Up => self.up,
            Bend::Down => -self.up,
            Bend::Left => -self.right,
            Bend::Right => self.right,
        }
    }
}

const CURVE_STEPS: usize = 32;

/// A smooth curve that leaves its first point along a start direction and passes through each point in turn, as cubic
/// Hermite spans whose tangents bisect the neighbouring chords. Each span scales its tangents to its own length, which
/// keeps the curve from overshooting where the points are spaced unevenly.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    /// Position, rotation from the start direction and share of the curve's length at each sample.
    samples: Vec<(DVec3, DQuat, f64)>,
    start: DVec3,
    length: f64,
}

impl Curve {
    /// Leaves the faces' center along their normal.
    pub fn new(frame: &Frame, points: &[DVec3]) -> Option<Curve> {
        Self::build(std::iter::once(frame.center).chain(points.iter().copied()), Some(frame.normal))
    }

    /// Starts at the first point, heading for the second.
    pub fn through(points: &[DVec3]) -> Option<Curve> {
        Self::build(points.iter().copied(), None)
    }

    fn build(points: impl Iterator<Item = DVec3>, start: Option<DVec3>) -> Option<Curve> {
        let mut pts: Vec<DVec3> = Vec::new();
        for p in points {
            if pts.last().is_none_or(|l| p.distance(*l) > 1e-6) {
                pts.push(p);
            }
        }

        if pts.len() < 2 {
            return None;
        }

        let start = start.unwrap_or((pts[1] - pts[0]).normalize());
        let last = pts.len() - 1;
        let direction = |i: usize| -> DVec3 {
            if i == 0 {
                start
            } else if i == last {
                (pts[i] - pts[i - 1]).normalize()
            } else {
                ((pts[i] - pts[i - 1]).normalize() + (pts[i + 1] - pts[i]).normalize()).try_normalize().unwrap_or((pts[i + 1] - pts[i]).normalize())
            }
        };
        let mut samples: Vec<(DVec3, DVec3)> = vec![(pts[0], start)];
        for i in 0..last {
            let (a, b) = (pts[i], pts[i + 1]);
            let len = a.distance(b);
            let (ma, mb) = (direction(i) * len, direction(i + 1) * len);
            for k in 1..=CURVE_STEPS {
                let u = k as f64 / CURVE_STEPS as f64;
                let (u2, u3) = (u * u, u * u * u);
                let pos = a * (2.0 * u3 - 3.0 * u2 + 1.0) + ma * (u3 - 2.0 * u2 + u) + b * (3.0 * u2 - 2.0 * u3) + mb * (u3 - u2);
                let tangent = a * (6.0 * u2 - 6.0 * u) + ma * (3.0 * u2 - 4.0 * u + 1.0) + b * (6.0 * u - 6.0 * u2) + mb * (3.0 * u2 - 2.0 * u);
                samples.push((pos, tangent.try_normalize().unwrap_or(samples.last()?.1)));
            }
        }

        // Each sample turns the last one's frame by the smallest rotation onto its own tangent, so the faces never
        // spin around the curve on their own.
        let mut out = Vec::with_capacity(samples.len());
        let (mut rot, mut length) = (DQuat::IDENTITY, 0.0);
        for (i, &(pos, tangent)) in samples.iter().enumerate() {
            if i > 0 {
                let (prev, prev_tangent) = samples[i - 1];
                rot = (DQuat::from_rotation_arc(prev_tangent, tangent) * rot).normalize();
                length += prev.distance(pos);
            }

            out.push((pos, rot, length));
        }

        if length <= 1e-9 {
            return None;
        }

        for s in &mut out {
            s.2 /= length;
        }

        Some(Curve { samples: out, start, length })
    }

    pub fn length(&self) -> f64 {
        self.length
    }

    /// Position and unit tangent at `t` from 0 to 1 of the curve's length.
    pub fn point(&self, t: f64) -> (DVec3, DVec3) {
        let (pos, rot) = self.at(t);
        (pos, (rot * self.start).normalize())
    }

    /// Position and rotation at `t` from 0 to 1, spaced evenly along the curve's length.
    fn at(&self, t: f64) -> (DVec3, DQuat) {
        let i = self.samples.partition_point(|s| s.2 < t).clamp(1, self.samples.len() - 1);
        let (a, b) = (self.samples[i - 1], self.samples[i]);
        let f = if b.2 > a.2 { ((t - a.2) / (b.2 - a.2)).clamp(0.0, 1.0) } else { 0.0 };
        (a.0.lerp(b.0, f), a.1.slerp(b.1, f))
    }
}

impl Sweep {
    /// Twist and scale around the faces' center at `t`.
    fn local(&self, frame: &Frame, t: f64) -> DMat4 {
        let Frame { center: c, normal: n, .. } = *frame;
        DMat4::from_translation(c)
            * DMat4::from_axis_angle(n, self.twist.to_radians() * t)
            * DMat4::from_scale(DVec3::splat(1.0 + (self.end_scale - 1.0) * t))
            * DMat4::from_translation(-c)
    }

    /// Where the faces sit at `t` from 0 to 1 along the curve.
    pub fn transform_along(&self, frame: &Frame, curve: &Curve, t: f64) -> DMat4 {
        let (pos, rot) = curve.at(t);
        DMat4::from_translation(DVec3::Y * self.rise * t)
            * DMat4::from_translation(pos)
            * DMat4::from_quat(rot)
            * DMat4::from_translation(-frame.center)
            * self.local(frame, t)
    }

    /// Where the faces sit at `t` from 0 to 1 along the path. A curve takes its points, see [`Self::transform_along`].
    pub fn transform(&self, frame: &Frame, t: f64) -> DMat4 {
        let Frame { center: c, normal: n, .. } = *frame;
        let local = self.local(frame, t);
        let dir = frame.towards(self.bend);
        let path = match self.path {
            SweepPath::Straight | SweepPath::Curve => DMat4::from_translation(n * self.length * t),
            SweepPath::SBend => DMat4::from_translation(n * self.length * t + dir * self.offset * t * t * (3.0 - 2.0 * t)),
            SweepPath::Arc => {
                let pivot = c + dir * self.length;
                DMat4::from_translation(pivot) * DMat4::from_axis_angle(n.cross(dir), self.angle.to_radians() * t) * DMat4::from_translation(-pivot)
            }
        };
        DMat4::from_translation(DVec3::Y * self.rise * t) * path * local
    }

    pub fn check(&self, frame: &Frame, faces: &[SweepFace]) -> Result<(), SweepError> {
        if !(1..=MAX_SEGMENTS).contains(&self.segments) {
            return Err(SweepError::Segments);
        }

        if self.end_scale.is_nan() || self.end_scale <= 0.0 {
            return Err(SweepError::EndScale);
        }

        match self.path {
            SweepPath::Straight | SweepPath::SBend if self.length.is_nan() || self.length <= 0.0 => Err(SweepError::Length),
            // A negative angle would start the run backwards into the source brush.
            SweepPath::Arc if !(self.angle > 0.0 && self.angle <= 720.0) => Err(SweepError::Angle),
            SweepPath::Arc => {
                let dir = frame.towards(self.bend);
                let n = frame.normal;
                // Twist swings every in-plane extent into dir somewhere along the way.
                let reach = |p: DVec3| {
                    let d = p - frame.center;
                    if self.twist == 0.0 { d.dot(dir) } else { (d - n * d.dot(n)).length() }
                };
                let reach = faces.iter().flat_map(|f| f.brush.face_points(f.face)).map(reach).fold(0.0, f64::max);
                let reach = reach * self.end_scale.max(1.0);
                if self.length > reach + 1e-6 { Ok(()) } else { Err(SweepError::Radius(reach)) }
            }
            _ => Ok(()),
        }
    }

    /// Refuses a curve whose tightest bend is smaller than how far the faces reach towards its inside there.
    fn check_curve(&self, frame: &Frame, faces: &[SweepFace], curve: &Curve) -> Result<(), SweepError> {
        let offsets: Vec<DVec3> = faces.iter().flat_map(|f| f.brush.face_points(f.face)).map(|p| p - frame.center).collect();
        let tangent = |rot: DQuat| rot * frame.normal;
        for w in curve.samples.windows(2) {
            let ((p0, r0, _), (p1, r1, t)) = (w[0], w[1]);
            let (t0, t1) = (tangent(r0), tangent(r1));
            let angle = t0.angle_between(t1);
            if angle < 1e-9 {
                continue;
            }

            let radius = p0.distance(p1) / angle;
            let inward = (t1 - t0).normalize();
            let local = self.local(frame, t);
            let reach = offsets.iter().map(|d| (r1 * local.transform_vector3(*d)).dot(inward)).fold(0.0, f64::max);
            if reach >= radius - 1e-6 {
                return Err(SweepError::Bend);
            }
        }

        Ok(())
    }

    /// One convex brush per face and segment, face by face. Each ring of points is worked out once, so neighbouring
    /// segments meet exactly.
    pub fn brushes(&self, faces: &[SweepFace]) -> Result<Vec<Brush>, SweepError> {
        self.brushes_through(faces, &[])
    }

    /// Like [`Self::brushes`], with the points a [`SweepPath::Curve`] passes through.
    pub fn brushes_through(&self, faces: &[SweepFace], points: &[DVec3]) -> Result<Vec<Brush>, SweepError> {
        let faces: Vec<SweepFace> = faces.iter().filter(|f| f.face < f.brush.faces.len()).cloned().collect();
        if faces.is_empty() {
            return Err(SweepError::NoFaces);
        }

        let frame = Frame::of(&faces).ok_or(SweepError::Facing)?;
        self.check(&frame, &faces)?;
        let curve = match self.path {
            SweepPath::Curve => {
                let curve = Curve::new(&frame, points).ok_or(SweepError::NoPoints)?;
                self.check_curve(&frame, &faces, &curve)?;
                Some(curve)
            }
            _ => None,
        };
        let n = self.segments;
        let at = |i: f64| match &curve {
            Some(c) => self.transform_along(&frame, c, i / n as f64),
            None => self.transform(&frame, i / n as f64),
        };
        let rings: Vec<DMat4> = (0..=n).map(|i| at(i as f64)).collect();
        let mut out = Vec::new();
        for f in &faces {
            let points = f.brush.face_points(f.face);
            let ring = |m: &DMat4| points.iter().map(|p| snap_vec(m.transform_point3(*p))).collect::<Vec<_>>();
            let rings: Vec<Vec<DVec3>> = rings.iter().map(ring).collect();
            for i in 0..n as usize {
                // Materials and alignment come from the source brush moved to the middle of the segment.
                let moved = f.brush.transformed(&at(i as f64 + 0.5), true);
                let face = &moved.faces[f.face];
                let mut template = moved.planes();
                template.push((face.plane.flipped(), face.data.clone()));
                let hull: Vec<DVec3> = rings[i].iter().chain(&rings[i + 1]).copied().collect();
                let brush = Brush::from_points(&hull, &template, &face.data.material).map_err(|_| SweepError::Flat)?;
                out.push(brush);
            }
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_face(min: DVec3, max: DVec3, normal: DVec3) -> SweepFace {
        let brush = Brush::from_aabb(&Aabb::new(min, max), "wall").unwrap();
        let face = brush.faces.iter().position(|f| f.plane.normal.dot(normal) > 0.99).unwrap();
        SweepFace { brush, face }
    }

    fn total_volume(brushes: &[Brush]) -> f64 {
        brushes.iter().map(Brush::volume).sum()
    }

    #[test]
    fn a_straight_sweep_matches_an_extrusion() {
        let face = box_face(DVec3::ZERO, DVec3::splat(64.0), DVec3::X);
        let out = Sweep { length: 128.0, segments: 4, ..Sweep::default() }.brushes(&[face]).unwrap();
        assert_eq!(out.len(), 4);
        assert!((total_volume(&out) - 64.0 * 64.0 * 128.0).abs() < 1e-6);
        let b = Aabb::from_points(out.iter().flat_map(|b| b.vertices.iter().copied()));
        assert_eq!((b.min, b.max), (DVec3::new(64.0, 0.0, 0.0), DVec3::new(192.0, 64.0, 64.0)));
        assert!(out.iter().all(|b| b.validate().is_ok()));
    }

    #[test]
    fn an_arc_turns_the_faces_and_neighbouring_segments_share_their_rings() {
        let face = box_face(DVec3::ZERO, DVec3::splat(64.0), DVec3::X);
        let sweep = Sweep { path: SweepPath::Arc, length: 96.0, angle: 90.0, bend: Bend::Right, segments: 6, ..Sweep::default() };
        let out = sweep.brushes(&[face]).unwrap();
        assert_eq!(out.len(), 6);
        assert!(out.iter().all(|b| b.validate().is_ok()));
        // Looking at a +X face from outside, right is -Z, so a right turn ends facing -Z.
        let last = out.last().unwrap();
        assert!(last.faces.iter().any(|f| f.plane.normal.dot(DVec3::NEG_Z) > 0.999));
        for w in out.windows(2) {
            let shared = w[0].vertices.iter().filter(|v| w[1].vertices.iter().any(|u| u.distance(**v) < 1e-9)).count();
            assert_eq!(shared, 4);
        }
    }

    #[test]
    fn faces_side_by_side_bend_around_one_pivot() {
        let a = box_face(DVec3::ZERO, DVec3::new(64.0, 64.0, 32.0), DVec3::X);
        let b = box_face(DVec3::new(0.0, 0.0, 32.0), DVec3::splat(64.0), DVec3::X);
        let sweep = Sweep { path: SweepPath::Arc, length: 96.0, segments: 4, ..Sweep::default() };
        let out = sweep.brushes(&[a, b]).unwrap();
        let whole = sweep.brushes(&[box_face(DVec3::ZERO, DVec3::splat(64.0), DVec3::X)]).unwrap();
        assert!((total_volume(&out) - total_volume(&whole)).abs() < 1e-3);
    }

    #[test]
    fn an_arc_tighter_than_the_faces_is_refused() {
        let face = box_face(DVec3::ZERO, DVec3::splat(64.0), DVec3::X);
        let err = Sweep { path: SweepPath::Arc, length: 20.0, ..Sweep::default() }.brushes(&[face]).unwrap_err();
        assert_eq!(err, SweepError::Radius(32.0));
    }

    #[test]
    fn a_twisted_arc_counts_the_extents_the_twist_turns_into_the_bend() {
        let face = box_face(DVec3::ZERO, DVec3::new(16.0, 64.0, 16.0), DVec3::X);
        let arc = Sweep { path: SweepPath::Arc, length: 12.0, ..Sweep::default() };
        assert!(arc.brushes(std::slice::from_ref(&face)).is_ok());
        let err = Sweep { twist: 90.0, ..arc }.brushes(&[face]).unwrap_err();
        assert!(matches!(err, SweepError::Radius(r) if r > 32.0), "{err:?}");
    }

    #[test]
    fn a_negative_angle_or_faces_facing_apart_are_refused() {
        let face = box_face(DVec3::ZERO, DVec3::splat(64.0), DVec3::X);
        let back = box_face(DVec3::ZERO, DVec3::splat(64.0), DVec3::NEG_X);
        let arc = Sweep { path: SweepPath::Arc, length: 96.0, angle: -90.0, ..Sweep::default() };
        assert_eq!(arc.brushes(std::slice::from_ref(&face)).unwrap_err(), SweepError::Angle);
        assert_eq!(Sweep::default().brushes(&[face, back]).unwrap_err(), SweepError::Facing);
        assert_eq!(Sweep::default().brushes(&[]).unwrap_err(), SweepError::NoFaces);
    }

    #[test]
    fn new_faces_take_the_materials_of_the_source_brush() {
        let mut face = box_face(DVec3::ZERO, DVec3::splat(64.0), DVec3::X);
        for f in &mut face.brush.faces {
            if f.plane.normal.y > 0.9 {
                f.data.material = "ceiling".into();
            }
        }

        let out = Sweep { path: SweepPath::Arc, length: 96.0, segments: 3, ..Sweep::default() }.brushes(&[face]).unwrap();
        for b in &out {
            let top = b.faces.iter().find(|f| f.plane.normal.y > 0.9).unwrap();
            assert_eq!(top.data.material, "ceiling");
        }
    }

    fn curve() -> Sweep {
        Sweep { path: SweepPath::Curve, segments: 16, ..Sweep::default() }
    }

    #[test]
    fn a_curve_through_one_point_ahead_matches_an_extrusion() {
        let face = box_face(DVec3::ZERO, DVec3::splat(64.0), DVec3::X);
        let out = curve().brushes_through(&[face], &[DVec3::new(192.0, 32.0, 32.0)]).unwrap();
        assert!((total_volume(&out) - 64.0 * 64.0 * 128.0).abs() < 1e-3);
        let b = Aabb::from_points(out.iter().flat_map(|b| b.vertices.iter().copied()));
        assert!(b.min.distance(DVec3::new(64.0, 0.0, 0.0)) < 1e-6 && b.max.distance(DVec3::new(192.0, 64.0, 64.0)) < 1e-6, "{b:?}");
    }

    #[test]
    fn a_curve_passes_through_its_points_and_ends_along_the_last_chord() {
        let face = box_face(DVec3::ZERO, DVec3::new(16.0, 64.0, 64.0), DVec3::X);
        let points = [DVec3::new(208.0, 32.0, 32.0), DVec3::new(400.0, 32.0, -160.0)];
        let sweep = curve();
        let frame = Frame::of(std::slice::from_ref(&face)).unwrap();
        let path = Curve::new(&frame, &points).unwrap();
        assert!(path.samples.iter().any(|s| s.0.distance(points[0]) < 1e-9));
        let end = sweep.transform_along(&frame, &path, 1.0);
        assert!(end.transform_point3(frame.center).distance(points[1]) < 1e-6);
        let along = end.transform_vector3(frame.normal).normalize();
        assert!(along.dot(DVec3::new(1.0, 0.0, -1.0).normalize()) > 0.999, "{along:?}");

        let out = sweep.brushes_through(&[face], &points).unwrap();
        assert_eq!(out.len(), 16);
        assert!(out.iter().all(|b| b.validate().is_ok() && b.volume() > 0.0));
        for w in out.windows(2) {
            let shared = w[0].vertices.iter().filter(|v| w[1].vertices.iter().any(|u| u.distance(**v) < 1e-9)).count();
            assert_eq!(shared, 4);
        }
    }

    #[test]
    fn a_flat_curve_keeps_the_faces_upright() {
        let face = box_face(DVec3::ZERO, DVec3::new(16.0, 64.0, 64.0), DVec3::X);
        let points = [DVec3::new(160.0, 32.0, -64.0), DVec3::new(96.0, 32.0, -256.0), DVec3::new(320.0, 32.0, -384.0)];
        let out = curve().brushes_through(&[face], &points).unwrap();
        for b in &out {
            assert!(b.faces.iter().any(|f| f.plane.normal.y > 0.999), "a segment rolled over");
        }
    }

    #[test]
    fn a_bend_tighter_than_the_faces_or_a_curve_without_points_is_refused() {
        let face = box_face(DVec3::ZERO, DVec3::splat(64.0), DVec3::X);
        let tight = [DVec3::new(72.0, 32.0, 32.0), DVec3::new(72.0, 32.0, 24.0)];
        assert_eq!(curve().brushes_through(std::slice::from_ref(&face), &tight).unwrap_err(), SweepError::Bend);
        assert_eq!(curve().brushes_through(std::slice::from_ref(&face), &[]).unwrap_err(), SweepError::NoPoints);
        assert_eq!(curve().brushes_through(&[face], &[DVec3::new(64.0, 32.0, 32.0)]).unwrap_err(), SweepError::NoPoints);
    }

    #[test]
    fn rise_twist_and_scale_still_give_valid_brushes() {
        let face = box_face(DVec3::ZERO, DVec3::new(16.0, 64.0, 64.0), DVec3::X);
        for path in [SweepPath::Straight, SweepPath::Arc, SweepPath::SBend] {
            let sweep = Sweep { path, length: 160.0, angle: 270.0, rise: 96.0, twist: 45.0, end_scale: 0.5, segments: 12, ..Sweep::default() };
            let out = sweep.brushes(std::slice::from_ref(&face)).unwrap();
            assert_eq!(out.len(), 12);
            assert!(out.iter().all(|b| b.volume() > 0.0), "{path:?}");
        }
    }
}
