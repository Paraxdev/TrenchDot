use glam::DVec3;

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
}
