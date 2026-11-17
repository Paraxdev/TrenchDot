use glam::{DMat4, DVec3};

use crate::Aabb;

/// Axis aligned planes a brush dab is mirrored across, at world x = `x` and z = `z`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mirror {
    pub x: Option<f64>,
    pub z: Option<f64>,
}

/// The planes a point is reflected across to reach the dab's side of a [`Mirror`]. Its own inverse.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fold {
    x: Option<f64>,
    z: Option<f64>,
}

impl Mirror {
    pub const NONE: Mirror = Mirror { x: None, z: None };

    pub fn is_none(&self) -> bool {
        self.x.is_none() && self.z.is_none()
    }

    /// The reflections that bring `p` onto the side of each plane `center` is on. A coordinate equal to the plane
    /// counts as the larger side.
    pub fn fold(&self, p: DVec3, center: DVec3) -> Fold {
        let across = |plane: f64, a: f64, b: f64| (a >= plane) != (b >= plane);
        Fold { x: self.x.filter(|&px| across(px, p.x, center.x)), z: self.z.filter(|&pz| across(pz, p.z, center.z)) }
    }

    /// Every fold of these planes, identity first: 1, 2 or 4 of them.
    pub fn folds(&self) -> impl Iterator<Item = Fold> + use<> {
        let (xs, zs) = ([None, self.x], [None, self.z]);
        let (nx, nz) = (1 + self.x.is_some() as usize, 1 + self.z.is_some() as usize);
        zs.into_iter().take(nz).flat_map(move |z| xs.into_iter().take(nx).map(move |x| Fold { x, z }))
    }
}

impl Fold {
    pub const IDENTITY: Fold = Fold { x: None, z: None };

    pub fn is_identity(&self) -> bool {
        self.x.is_none() && self.z.is_none()
    }

    pub fn point(&self, p: DVec3) -> DVec3 {
        DVec3::new(self.x.map_or(p.x, |px| 2.0 * px - p.x), p.y, self.z.map_or(p.z, |pz| 2.0 * pz - p.z))
    }

    pub fn vector(&self, v: DVec3) -> DVec3 {
        DVec3::new(if self.x.is_some() { -v.x } else { v.x }, v.y, if self.z.is_some() { -v.z } else { v.z })
    }
}

/// A plane square to one world axis (0 = x, 1 = y, 2 = z), at coordinate `at` along it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisPlane {
    pub axis: usize,
    pub at: f64,
}

impl AxisPlane {
    pub fn reflect(&self, p: DVec3) -> DVec3 {
        let mut q = p;
        q[self.axis] = 2.0 * self.at - p[self.axis];
        q
    }

    pub fn reflect_vector(&self, v: DVec3) -> DVec3 {
        let mut w = v;
        w[self.axis] = -v[self.axis];
        w
    }

    /// The reflection across this plane.
    pub fn matrix(&self) -> DMat4 {
        let (mut scale, mut at) = (DVec3::ONE, DVec3::ZERO);
        scale[self.axis] = -1.0;
        at[self.axis] = self.at;
        DMat4::from_translation(at) * DMat4::from_scale(scale) * DMat4::from_translation(-at)
    }

    /// Signed, positive on the larger side.
    pub fn distance(&self, p: DVec3) -> f64 {
        p[self.axis] - self.at
    }

    /// Whether the plane passes through `b` grown by `tol` along the axis.
    pub fn meets(&self, b: &Aabb, tol: f64) -> bool {
        b.min[self.axis] - tol <= self.at && self.at <= b.max[self.axis] + tol
    }

    /// The plane `m` reflects across when it is a pure reflection on one axis. Group transforms gather rounding as
    /// they are composed, so the slide along the plane is allowed more slack than the linear part.
    pub fn of_reflection(m: &DMat4) -> Option<AxisPlane> {
        const LINEAR: f64 = 1e-6;
        const SLIDE: f64 = 1e-3;
        (0..3).map(|axis| AxisPlane { axis, at: m.w_axis[axis] * 0.5 }).find(|plane| {
            let r = plane.matrix();
            let linear = [(m.x_axis, r.x_axis), (m.y_axis, r.y_axis), (m.z_axis, r.z_axis)];
            linear.iter().all(|(a, b)| (*a - *b).abs().max_element() <= LINEAR) && (m.w_axis - r.w_axis).abs().max_element() <= SLIDE
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const X: f64 = 37.5;
    const Z: f64 = -12.25;
    const XZ: Mirror = Mirror { x: Some(X), z: Some(Z) };

    #[test]
    fn folds_list_identity_first_then_every_combination() {
        let only_x = Mirror { x: Some(X), z: None };
        let only_z = Mirror { x: None, z: Some(Z) };
        assert_eq!(Mirror::NONE.folds().collect::<Vec<_>>(), [Fold::IDENTITY]);
        assert_eq!(only_x.folds().collect::<Vec<_>>(), [Fold::IDENTITY, Fold { x: Some(X), z: None }]);
        assert_eq!(only_z.folds().collect::<Vec<_>>(), [Fold::IDENTITY, Fold { x: None, z: Some(Z) }]);
        assert_eq!(
            XZ.folds().collect::<Vec<_>>(),
            [Fold::IDENTITY, Fold { x: Some(X), z: None }, Fold { x: None, z: Some(Z) }, Fold { x: Some(X), z: Some(Z) }]
        );
        assert!(Mirror::NONE.is_none() && Mirror::default().is_none());
        assert!(!only_x.is_none() && !only_z.is_none());
    }

    #[test]
    fn fold_reflects_across_the_planes_between_point_and_center() {
        let center = DVec3::new(50.0, 7.0, -30.0);
        let fold = |m: &Mirror, x: f64, z: f64| m.fold(DVec3::new(x, -3.0, z), center);
        assert!(Mirror::NONE.fold(DVec3::new(-900.0, 0.0, 900.0), center).is_identity());
        assert!(fold(&XZ, 40.0, -20.0).is_identity(), "same side of both planes");
        assert_eq!(fold(&XZ, 10.0, -20.0), Fold { x: Some(X), z: None });
        assert_eq!(fold(&XZ, 40.0, -5.0), Fold { x: None, z: Some(Z) });
        assert_eq!(fold(&XZ, 10.0, -5.0), Fold { x: Some(X), z: Some(Z) });

        // A point on the plane counts as the larger side.
        assert!(fold(&XZ, X, -20.0).is_identity());
        assert_eq!(fold(&XZ, X - 1e-9, -20.0), Fold { x: Some(X), z: None });
        assert_eq!(fold(&XZ, 40.0, Z), Fold { x: None, z: Some(Z) });
        assert!(fold(&XZ, 40.0, Z - 1e-9).is_identity());

        // So does a center on the plane.
        let on = DVec3::new(X, 0.0, Z);
        assert!(XZ.fold(DVec3::new(X + 3.0, 0.0, Z), on).is_identity());
        assert!(XZ.fold(on, on).is_identity());
        assert_eq!(XZ.fold(DVec3::new(X - 3.0, 0.0, Z - 0.5), on), Fold { x: Some(X), z: Some(Z) });
    }

    #[test]
    fn point_and_vector_reflect_horizontally_and_undo_themselves() {
        let p = DVec3::new(10.5, -4.0, 3.75);
        let v = DVec3::new(0.6, -0.48, -0.64);
        let x = Fold { x: Some(X), z: None };
        let z = Fold { x: None, z: Some(Z) };
        let xz = Fold { x: Some(X), z: Some(Z) };
        assert_eq!(Fold::IDENTITY.point(p), p);
        assert_eq!(Fold::IDENTITY.vector(v), v);
        assert_eq!(x.point(p), DVec3::new(64.5, -4.0, 3.75));
        assert_eq!(z.point(p), DVec3::new(10.5, -4.0, -28.25));
        assert_eq!(xz.point(p), DVec3::new(64.5, -4.0, -28.25));
        assert_eq!(x.vector(v), DVec3::new(-0.6, -0.48, -0.64));
        assert_eq!(z.vector(v), DVec3::new(0.6, -0.48, 0.64));
        assert_eq!(xz.vector(v), DVec3::new(-0.6, -0.48, 0.64));
        for f in XZ.folds() {
            assert_eq!(f.point(f.point(p)), p, "{f:?}");
            assert_eq!(f.vector(f.vector(v)), v, "{f:?}");
        }
    }

    const P: DVec3 = DVec3::new(37.5, -12.25, 3.0);

    fn planes() -> [AxisPlane; 3] {
        [AxisPlane { axis: 0, at: X }, AxisPlane { axis: 1, at: Z }, AxisPlane { axis: 2, at: 3.0 }]
    }

    #[test]
    fn axis_planes_reflect_points_and_vectors_and_undo_themselves() {
        let p = DVec3::new(10.5, -4.0, 9.75);
        let v = DVec3::new(0.6, -0.48, -0.64);
        let [x, y, z] = planes();
        assert_eq!(x.reflect(p), DVec3::new(64.5, -4.0, 9.75));
        assert_eq!(y.reflect(p), DVec3::new(10.5, -20.5, 9.75));
        assert_eq!(z.reflect(p), DVec3::new(10.5, -4.0, -3.75));
        assert_eq!(x.reflect_vector(v), DVec3::new(-0.6, -0.48, -0.64));
        assert_eq!(y.reflect_vector(v), DVec3::new(0.6, 0.48, -0.64));
        assert_eq!(z.reflect_vector(v), DVec3::new(0.6, -0.48, 0.64));
        assert_eq!((x.distance(p), y.distance(p), z.distance(p)), (-27.0, 8.25, 6.75));
        for plane in planes() {
            assert_eq!(plane.reflect(plane.reflect(p)), p, "{plane:?}");
            assert_eq!(plane.reflect_vector(plane.reflect_vector(v)), v, "{plane:?}");
            let m = plane.matrix();
            assert!(m.transform_point3(p).abs_diff_eq(plane.reflect(p), 1e-12), "{plane:?}");
            assert!(m.transform_vector3(v).abs_diff_eq(plane.reflect_vector(v), 1e-12), "{plane:?}");
            assert!((m * m).abs_diff_eq(DMat4::IDENTITY, 1e-12), "{plane:?}");
            assert!((m.determinant() + 1.0).abs() < 1e-12, "{plane:?}");
        }
    }

    #[test]
    fn meets_counts_the_tolerance_and_the_exact_edge() {
        let b = Aabb::new(DVec3::new(8.0, -30.0, 4.0), DVec3::new(40.0, 14.0, 20.0));
        let at = |axis, at: f64| AxisPlane { axis, at };
        assert!(at(0, 8.0).meets(&b, 0.0) && at(0, 40.0).meets(&b, 0.0) && at(0, 21.5).meets(&b, 0.0));
        assert!(!at(0, 7.9).meets(&b, 0.0) && !at(0, 40.1).meets(&b, 0.0));
        assert!(at(0, 40.5).meets(&b, 0.5) && at(0, 7.5).meets(&b, 0.5));
        assert!(!at(0, 40.75).meets(&b, 0.5) && !at(0, 7.25).meets(&b, 0.5));
        assert!(at(1, -30.0).meets(&b, 0.0) && !at(1, 15.0).meets(&b, 0.5) && at(1, 14.5).meets(&b, 0.5));
        assert!(at(2, 20.0).meets(&b, 0.0) && !at(2, 3.0).meets(&b, 0.5) && at(2, 3.5).meets(&b, 0.5));
        assert!(!at(0, 0.0).meets(&Aabb::EMPTY, 1e9));
    }

    #[test]
    fn of_reflection_finds_the_plane_of_a_pure_reflection_only() {
        for plane in planes() {
            assert_eq!(AxisPlane::of_reflection(&plane.matrix()), Some(plane), "{plane:?}");
            let mut slide = DVec3::ZERO;
            slide[(plane.axis + 1) % 3] = 0.9e-3;
            slide[(plane.axis + 2) % 3] = -0.9e-3;
            assert_eq!(AxisPlane::of_reflection(&(DMat4::from_translation(slide) * plane.matrix())), Some(plane), "slid {slide}");
            slide[(plane.axis + 1) % 3] = 1.1e-3;
            assert_eq!(AxisPlane::of_reflection(&(DMat4::from_translation(slide) * plane.matrix())), None, "{plane:?} slid {slide}");
            slide[(plane.axis + 1) % 3] = 1e-2;
            assert_eq!(AxisPlane::of_reflection(&(DMat4::from_translation(slide) * plane.matrix())), None, "{plane:?} slid {slide}");

            let mut skew = plane.matrix();
            skew.y_axis.x += 0.9e-6;
            assert_eq!(AxisPlane::of_reflection(&skew), Some(plane));
            skew.y_axis.x += 0.2e-6;
            assert_eq!(AxisPlane::of_reflection(&skew), None, "{plane:?}");
            let mut scale = DVec3::splat(1.0);
            scale[plane.axis] = 2.0;
            assert_eq!(AxisPlane::of_reflection(&(DMat4::from_scale(scale) * plane.matrix())), None, "{plane:?} scaled");
        }

        let turn = |axis: DVec3, degrees: f64| {
            DMat4::from_translation(P) * DMat4::from_quat(glam::DQuat::from_axis_angle(axis, f64::to_radians(degrees))) * DMat4::from_translation(-P)
        };
        assert_eq!(AxisPlane::of_reflection(&DMat4::IDENTITY), None);
        assert_eq!(AxisPlane::of_reflection(&DMat4::from_translation(DVec3::new(64.0, 0.0, 0.0))), None);
        assert_eq!(AxisPlane::of_reflection(&turn(DVec3::Y, 30.0)), None);
        assert_eq!(AxisPlane::of_reflection(&turn(DVec3::Y, 180.0)), None, "a half turn flips two axes but keeps handedness");
        assert_eq!(AxisPlane::of_reflection(&(turn(DVec3::Y, 30.0) * planes()[0].matrix())), None);
        assert_eq!(AxisPlane::of_reflection(&DMat4::from_scale(DVec3::splat(-1.0))), None);
        assert_eq!(AxisPlane::of_reflection(&(planes()[0].matrix() * planes()[2].matrix())), None);
    }
}
