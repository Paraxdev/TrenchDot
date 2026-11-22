//! Bridges between two brush faces: one convex brush wrapped around both.

use crate::brush::{Brush, BrushError};

/// The convex brush around faces `fa` of `a` and `fb` of `b`, for a ramp between two platforms, a beam between two
/// walls or a plug that closes a gap. Each new face takes the surface of the face of either brush it lies most like.
pub fn bridge_faces(a: &Brush, fa: usize, b: &Brush, fb: usize) -> Result<Brush, BrushError> {
    let points: Vec<_> = a.face_points(fa).into_iter().chain(b.face_points(fb)).collect();
    let template: Vec<_> = a.planes().into_iter().chain(b.planes()).collect();
    let material = a.faces.get(fa).map(|f| f.data.material.clone()).unwrap_or_default();
    Brush::from_points(&points, &template, &material)
}

#[cfg(test)]
mod tests {
    use td_core::{Aabb, DVec3};

    use super::*;

    fn top(b: &Brush) -> usize {
        b.faces.iter().position(|f| f.plane.normal.y > 0.99).unwrap()
    }

    fn side(b: &Brush, normal: DVec3) -> usize {
        b.faces.iter().position(|f| f.plane.normal.dot(normal) > 0.99).unwrap()
    }

    #[test]
    fn facing_walls_get_a_beam_and_two_platform_tops_a_ramp() {
        let left = Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::new(32.0, 128.0, 64.0)), "wall").unwrap();
        let right = Brush::from_aabb(&Aabb::new(DVec3::new(160.0, 0.0, 0.0), DVec3::new(192.0, 128.0, 64.0)), "wall").unwrap();
        let beam = bridge_faces(&left, side(&left, DVec3::X), &right, side(&right, -DVec3::X)).unwrap();
        assert!((beam.volume() - 128.0 * 128.0 * 64.0).abs() < 1e-6, "{}", beam.volume());
        assert_eq!(beam.faces.len(), 6);

        let low = Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::new(64.0, 16.0, 64.0)), "floor").unwrap();
        let high = Brush::from_aabb(&Aabb::new(DVec3::new(128.0, 0.0, 0.0), DVec3::new(192.0, 64.0, 64.0)), "floor").unwrap();
        let ramp = bridge_faces(&low, top(&low), &high, top(&high)).unwrap();
        ramp.validate().unwrap();
        assert!(ramp.faces.iter().all(|f| f.data.material == "floor"));
        assert!(ramp.vertices.iter().any(|v| v.distance(DVec3::new(64.0, 16.0, 0.0)) < 1e-6));
        assert!(ramp.vertices.iter().any(|v| v.distance(DVec3::new(128.0, 64.0, 64.0)) < 1e-6));

        assert!(bridge_faces(&low, top(&low), &low, top(&low)).is_err(), "one flat face has no volume");
    }
}
