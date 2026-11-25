use td_core::DVec3;

use crate::brush::{Brush, BrushError, FaceData};

/// Where the faces a subtraction carves into a brush take their material and alignment from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CarveMaterial {
    /// The cutter's face that made the cut.
    #[default]
    Cutter,
    /// The target's face that points the most like the new one, as clipping does.
    Target,
}

impl CarveMaterial {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "cutter" => Some(Self::Cutter),
            "target" => Some(Self::Target),
            _ => None,
        }
    }
}

/// `minuend - subtrahend`, as a set of convex pieces. New faces take the subtrahend's surface data.
pub fn subtract(minuend: &Brush, subtrahend: &Brush) -> Vec<Brush> {
    subtract_with(minuend, subtrahend, CarveMaterial::Cutter)
}

/// [`subtract`] with a choice of surface data for the carved faces.
pub fn subtract_with(minuend: &Brush, subtrahend: &Brush, carve: CarveMaterial) -> Vec<Brush> {
    if !minuend.intersects(subtrahend) {
        return vec![minuend.clone()];
    }

    let mut out = Vec::new();
    let mut remaining = minuend.clone();
    // Cutting with the planes that remove the most volume first leaves fewer, larger pieces.
    let mut faces: Vec<&crate::brush::Face> = subtrahend.faces.iter().collect();
    faces.sort_by(|a, b| {
        let cut = |f: &crate::brush::Face| minuend.split(&f.plane, &f.data).0.map(|p| p.volume()).unwrap_or(0.0);
        cut(b).total_cmp(&cut(a))
    });
    let template = minuend.planes();
    for face in faces {
        let data = match carve {
            CarveMaterial::Cutter => &face.data,
            // The visible carved face of a piece faces into the cutter.
            CarveMaterial::Target => crate::brush::best_face_data(&face.plane.flipped(), &template).unwrap_or(&face.data),
        };
        let (front, back) = remaining.split(&face.plane, data);
        if let Some(f) = front {
            out.push(f);
        }

        match back {
            Some(b) => remaining = b,
            None => break,
        }
    }

    merge_pieces(out)
}

/// Joins pieces whose union is still convex, and drops slivers.
pub fn merge_pieces(mut pieces: Vec<Brush>) -> Vec<Brush> {
    pieces.retain(|p| p.volume() > 1e-4);
    loop {
        let mut merged = None;
        'search: for i in 0..pieces.len() {
            for j in i + 1..pieces.len() {
                if !pieces[i].bounds().intersects(&pieces[j].bounds().expanded(1e-3)) {
                    continue;
                }

                let Ok(hull) = convex_merge(&[&pieces[i], &pieces[j]], "") else { continue };
                let sum = pieces[i].volume() + pieces[j].volume();
                if (hull.volume() - sum).abs() <= 1e-4 * sum.max(1.0) {
                    merged = Some((i, j, hull));
                    break 'search;
                }
            }
        }

        match merged {
            Some((i, j, hull)) => {
                pieces.remove(j);
                pieces[i] = hull;
            }
            None => return pieces,
        }
    }
}

/// Joins pieces whose union is convex, whether they only touch or overlap, so identical pieces become one. Unlike
/// [`merge_pieces`] it keeps thin pieces.
pub fn join_convex(mut pieces: Vec<Brush>) -> Vec<Brush> {
    loop {
        let mut joined = None;
        'search: for i in 0..pieces.len() {
            for j in i + 1..pieces.len() {
                if let Some(hull) = join_pair(&pieces[i], &pieces[j]) {
                    joined = Some((i, j, hull));
                    break 'search;
                }
            }
        }

        match joined {
            Some((i, j, hull)) => {
                pieces.remove(j);
                pieces[i] = hull;
            }
            None => return pieces,
        }
    }
}

/// The hull of `a` and `b`, when it holds no more than their union.
fn join_pair(a: &Brush, b: &Brush) -> Option<Brush> {
    if !a.bounds().intersects(&b.bounds().expanded(1e-3)) {
        return None;
    }

    let hull = convex_merge(&[a, b], "").ok()?;
    let both = intersect(a, b).map(|p| p.volume().max(0.0)).unwrap_or(0.0);
    let union = a.volume() + b.volume() - both;
    ((hull.volume() - union).abs() <= 1e-4 * union.max(1.0)).then_some(hull)
}

pub fn intersect(a: &Brush, b: &Brush) -> Result<Brush, BrushError> {
    let mut planes = a.planes();
    planes.extend(b.planes());
    Brush::from_planes(planes)
}

/// Convex hull of all input brushes (TrenchBroom's "convex merge").
pub fn convex_merge(brushes: &[&Brush], default_material: &str) -> Result<Brush, BrushError> {
    let points: Vec<DVec3> = brushes.iter().flat_map(|b| b.vertices.iter().copied()).collect();
    let template: Vec<_> = brushes.iter().flat_map(|b| b.planes()).collect();
    Brush::from_points(&points, &template, default_material)
}

/// Replaces the brush by walls of the given thickness around its former volume.
pub fn hollow(brush: &Brush, thickness: f64) -> Vec<Brush> {
    let inner_planes: Vec<_> = brush.planes().into_iter().map(|(p, d)| (p.offset(-thickness), d)).collect();
    match Brush::from_planes(inner_planes) {
        Ok(inner) if inner.volume() > 1e-6 => subtract(brush, &inner),
        _ => vec![brush.clone()],
    }
}

/// Extrudes a face outward by `distance`, creating a new brush attached to it.
pub fn extrude_face(brush: &Brush, face: usize, distance: f64) -> Result<Brush, BrushError> {
    sweep_face(brush, face, distance)
}

/// The volume a face sweeps when it moves `distance` along its normal, inward when negative.
pub fn sweep_face(brush: &Brush, face: usize, distance: f64) -> Result<Brush, BrushError> {
    let f = &brush.faces[face];
    let normal = f.plane.normal;
    let points = brush.face_points(face);
    let mut all = points.clone();
    all.extend(points.iter().map(|p| *p + normal * distance));
    let mut template = brush.planes();
    template.push((f.plane.flipped(), FaceData { ..f.data.clone() }));
    Brush::from_points(&all, &template, &f.data.material)
}

#[cfg(test)]
mod tests {
    use super::*;
    use td_core::{Aabb, AxisPlane, Plane};

    fn boxb(min: DVec3, max: DVec3) -> Brush {
        Brush::from_aabb(&Aabb::new(min, max), "m").unwrap()
    }

    #[test]
    fn a_face_sweeps_outward_and_inward() {
        let b = boxb(DVec3::ZERO, DVec3::splat(64.0));
        let top = b.faces.iter().position(|f| f.plane.normal == DVec3::Y).unwrap();
        let out = sweep_face(&b, top, 16.0).unwrap();
        assert!((out.bounds().min.y - 64.0).abs() < 1e-9 && (out.bounds().max.y - 80.0).abs() < 1e-9);
        let inward = sweep_face(&b, top, -16.0).unwrap();
        assert!((inward.bounds().min.y - 48.0).abs() < 1e-9 && (inward.bounds().max.y - 64.0).abs() < 1e-9);
        inward.validate().unwrap();
    }

    #[test]
    fn subtract_center_hole() {
        let a = boxb(DVec3::splat(-32.0), DVec3::splat(32.0));
        let b = boxb(DVec3::new(-8.0, -64.0, -8.0), DVec3::new(8.0, 64.0, 8.0));
        let pieces = subtract(&a, &b);
        assert_eq!(pieces.len(), 4);
        let vol: f64 = pieces.iter().map(|p| p.volume()).sum();
        assert!((vol - (a.volume() - 16.0 * 16.0 * 64.0)).abs() < 1e-3);
        for p in &pieces {
            p.validate().unwrap();
        }
    }

    #[test]
    fn carved_faces_can_keep_the_target_material() {
        let wall = Brush::from_aabb(&Aabb::new(DVec3::new(-64.0, 0.0, -8.0), DVec3::new(64.0, 128.0, 8.0)), "brick").unwrap();
        let window = Brush::from_aabb(&Aabb::new(DVec3::new(-16.0, 32.0, -16.0), DVec3::new(16.0, 96.0, 16.0)), "cutter").unwrap();
        let materials = |pieces: &[Brush]| pieces.iter().flat_map(|p| p.faces.iter().map(|f| f.data.material.clone())).collect::<Vec<_>>();
        let cutter = subtract(&wall, &window);
        assert!(materials(&cutter).iter().any(|m| m == "cutter"), "by default the reveal takes the cutter's material");
        let target = subtract_with(&wall, &window, CarveMaterial::Target);
        assert!(materials(&target).iter().all(|m| m == "brick"), "{:?}", materials(&target));
        assert_eq!(target.len(), cutter.len());
        assert_eq!(CarveMaterial::from_name("target"), Some(CarveMaterial::Target));
        assert_eq!(CarveMaterial::from_name("nope"), None);
    }

    #[test]
    fn subtract_disjoint_is_identity() {
        let a = boxb(DVec3::ZERO, DVec3::splat(8.0));
        let b = boxb(DVec3::splat(16.0), DVec3::splat(32.0));
        assert_eq!(subtract(&a, &b).len(), 1);
    }

    #[test]
    fn hollow_volume() {
        let a = boxb(DVec3::splat(-32.0), DVec3::splat(32.0));
        let walls = hollow(&a, 4.0);
        let vol: f64 = walls.iter().map(|p| p.volume()).sum();
        assert!((vol - (64.0f64.powi(3) - 56.0f64.powi(3))).abs() < 1e-3);
    }

    #[test]
    fn merge_two_boxes() {
        let a = boxb(DVec3::ZERO, DVec3::splat(16.0));
        let b = boxb(DVec3::new(16.0, 0.0, 0.0), DVec3::new(32.0, 16.0, 16.0));
        let m = convex_merge(&[&a, &b], "m").unwrap();
        assert_eq!(m.faces.len(), 6);
        assert!((m.volume() - 32.0 * 16.0 * 16.0).abs() < 1e-6);
    }

    const SEAM: AxisPlane = AxisPlane { axis: 0, at: 96.0 };

    fn face(normal: DVec3, point: DVec3, material: &str) -> (Plane, FaceData) {
        let n = normal.normalize();
        (Plane::new(n, n.dot(point)), FaceData::with_normal(material, n))
    }

    /// The left half of a hut whose roof rises to a ridge on `SEAM`.
    fn half_hut() -> Brush {
        Brush::from_planes(vec![
            face(DVec3::X, DVec3::new(96.0, 0.0, 0.0), "seam"),
            face(DVec3::NEG_X, DVec3::new(40.0, 0.0, 0.0), "outer"),
            face(DVec3::NEG_Y, DVec3::new(0.0, 16.0, 0.0), "floor"),
            face(DVec3::new(-32.0, 56.0, 0.0), DVec3::new(40.0, 48.0, 0.0), "roof"),
            face(DVec3::Z, DVec3::new(0.0, 0.0, 40.0), "front"),
            face(DVec3::NEG_Z, DVec3::new(0.0, 0.0, -24.0), "back"),
        ])
        .unwrap()
    }

    fn assert_box(b: &Brush, min: DVec3, max: DVec3) {
        b.validate().unwrap();
        assert_eq!(b.bounds(), Aabb::new(min, max));
        assert_eq!(b.faces.len(), 6);
        let size = max - min;
        assert!((b.volume() - size.x * size.y * size.z).abs() < 1e-6, "{}", b.volume());
    }

    #[test]
    fn join_convex_passes_no_piece_and_one_piece_through() {
        assert!(join_convex(Vec::new()).is_empty());
        assert_eq!(join_convex(vec![half_hut()]), vec![half_hut()]);
    }

    #[test]
    fn join_convex_joins_a_slanted_half_and_its_mirror() {
        let half = half_hut();
        let mirror = half.transformed(&SEAM.matrix(), true);
        let joined = join_convex(vec![half.clone(), mirror.clone()]);
        assert_eq!(joined.len(), 1);
        let hut = &joined[0];
        hut.validate().unwrap();
        assert_eq!(hut.bounds(), Aabb::new(DVec3::new(40.0, 16.0, -24.0), DVec3::new(152.0, 80.0, 40.0)));
        assert!((hut.volume() - 2.0 * 56.0 * 48.0 * 64.0).abs() < 1e-6, "{}", hut.volume());

        // Planes both halves have keep the first half's face, the mirror's own faces keep its mirrored alignment.
        let expected = [
            (face(DVec3::NEG_X, DVec3::new(40.0, 0.0, 0.0), "outer"), &half),
            (face(DVec3::X, DVec3::new(152.0, 0.0, 0.0), "outer"), &mirror),
            (face(DVec3::NEG_Y, DVec3::new(0.0, 16.0, 0.0), "floor"), &half),
            (face(DVec3::new(-32.0, 56.0, 0.0), DVec3::new(40.0, 48.0, 0.0), "roof"), &half),
            (face(DVec3::new(32.0, 56.0, 0.0), DVec3::new(152.0, 48.0, 0.0), "roof"), &mirror),
            (face(DVec3::Z, DVec3::new(0.0, 0.0, 40.0), "front"), &half),
            (face(DVec3::NEG_Z, DVec3::new(0.0, 0.0, -24.0), "back"), &half),
        ];
        assert_eq!(hut.faces.len(), expected.len());
        for ((plane, data), source) in &expected {
            let on = |b: &Brush| b.faces.iter().find(|f| f.plane.approx_eq(plane, 1e-9, 1e-6)).cloned();
            let got = on(hut).unwrap_or_else(|| panic!("no face on {plane:?}"));
            assert_eq!(got.data.material, data.material);
            assert_eq!(got.data, on(source).unwrap().data, "{plane:?}");
        }
    }

    #[test]
    fn join_convex_joins_overlapping_and_identical_pieces() {
        // A half that runs 8 u past the plane overlaps its mirror by 16 u, which merge_pieces refuses.
        let (min, max) = (DVec3::new(40.0, 16.0, -24.0), DVec3::new(104.0, 80.0, 40.0));
        let half = boxb(min, max);
        let mirror = half.transformed(&SEAM.matrix(), true);
        assert_eq!(merge_pieces(vec![half.clone(), mirror.clone()]).len(), 2);
        let joined = join_convex(vec![half.clone(), mirror]);
        assert_eq!(joined.len(), 1);
        assert_box(&joined[0], min, DVec3::new(152.0, 80.0, 40.0));

        let same = join_convex(vec![half.clone(), half.clone(), half.clone()]);
        assert_eq!(same.len(), 1);
        assert_box(&same[0], min, max);

        let inside = boxb(DVec3::new(50.0, 20.0, -10.0), DVec3::new(70.0, 30.0, 0.0));
        let swallowed = join_convex(vec![inside, half]);
        assert_eq!(swallowed.len(), 1);
        assert_box(&swallowed[0], min, max);
    }

    #[test]
    fn join_convex_keeps_an_l_shape_and_an_edge_touch_apart() {
        let base = boxb(DVec3::new(40.0, 16.0, -24.0), DVec3::new(104.0, 48.0, 40.0));
        let upright = boxb(DVec3::new(40.0, 48.0, -24.0), DVec3::new(72.0, 112.0, 40.0));
        assert_eq!(join_convex(vec![base.clone(), upright.clone()]), vec![base.clone(), upright]);

        let corner = boxb(DVec3::new(104.0, 48.0, -24.0), DVec3::new(150.0, 90.0, 40.0));
        assert_eq!(join_convex(vec![base.clone(), corner.clone()]), vec![base, corner]);
    }

    #[test]
    fn join_convex_joins_four_quarters_and_leaves_a_far_piece() {
        let along_z = AxisPlane { axis: 2, at: 8.0 };
        let quarter = boxb(DVec3::new(40.0, 16.0, -24.0), DVec3::new(96.0, 80.0, 8.0));
        let across = quarter.transformed(&SEAM.matrix(), true);
        let far = boxb(DVec3::new(400.0, 16.0, -24.0), DVec3::new(430.0, 50.0, 0.0));
        // The first pair tried only touches at an edge.
        let pieces = vec![quarter.clone(), across.transformed(&along_z.matrix(), true), far.clone(), across, quarter.transformed(&along_z.matrix(), true)];
        let joined = join_convex(pieces);
        assert_eq!(joined.len(), 2);
        assert_box(&joined[0], DVec3::new(40.0, 16.0, -24.0), DVec3::new(152.0, 80.0, 40.0));
        assert_eq!(joined[1], far);
    }

    #[test]
    fn join_convex_allows_the_volume_tolerance_and_no_more() {
        // The hull over a step of height h holds a wedge of 2048 h more than the two boxes, the tolerance is 1e-4 of
        // their 524288 + 4096 h, so the tallest step that still joins is about 0.0256.
        let pieces = |h: f64| {
            let low = boxb(DVec3::new(40.0, 16.0, -24.0), DVec3::new(104.0, 80.0, 40.0));
            vec![low, boxb(DVec3::new(104.0, 16.0, -24.0), DVec3::new(168.0, 80.0 + h, 40.0))]
        };
        assert_eq!(join_convex(pieces(0.0)).len(), 1);
        assert_eq!(join_convex(pieces(0.0255)).len(), 1);
        assert_eq!(join_convex(pieces(0.0257)), pieces(0.0257));
    }

    #[test]
    fn extrude_adds_volume() {
        let a = boxb(DVec3::ZERO, DVec3::splat(16.0));
        let top = a.faces.iter().position(|f| f.plane.normal.y > 0.5).unwrap();
        let e = extrude_face(&a, top, 8.0).unwrap();
        assert!((e.volume() - 16.0 * 16.0 * 8.0).abs() < 1e-6);
        assert!(e.bounds().min.y > 15.9);
    }
}
