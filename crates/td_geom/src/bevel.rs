//! Bevels and chamfers on brush edges. Each bevelled edge adds clip planes, so the brush stays convex.

use td_core::{DVec3, Plane};

use crate::brush::{Brush, BrushError, FaceData};
use crate::uv::FaceUv;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum BevelError {
    #[error("no edge of the brush runs between the given points")]
    NoEdge,
    #[error("the bevel is wider than the faces beside the edge")]
    TooWide,
    #[error(transparent)]
    Brush(#[from] BrushError),
}

const EPS: f64 = 1e-4;

fn slerp(a: DVec3, b: DVec3, t: f64) -> DVec3 {
    let angle = a.dot(b).clamp(-1.0, 1.0).acos();
    if angle < 1e-9 {
        return a;
    }

    (a * ((1.0 - t) * angle).sin() + b * (t * angle).sin()) / angle.sin()
}

/// Bevels the edges of `brush` that run between the point pairs in `edges`, cutting `width` into both faces beside
/// each edge. One segment is a flat chamfer, more round the corner off in that many facets. The new faces continue the
/// texture of the face they lean towards. An edge is found by its end points, in either order.
pub fn bevel_edges(brush: &Brush, edges: &[(DVec3, DVec3)], width: f64, segments: usize) -> Result<Brush, BevelError> {
    let index = |p: DVec3| brush.vertices.iter().position(|v| v.distance(p) < EPS);
    let mut planes = brush.planes();
    let mut found = 0;
    for &(pa, pb) in edges {
        let (Some(a), Some(b)) = (index(pa), index(pb)) else { continue };
        let sides: Vec<usize> = (0..brush.faces.len())
            .filter(|&f| {
                let ids = &brush.faces[f].indices;
                ids.contains(&(a as u32)) && ids.contains(&(b as u32))
            })
            .collect();
        let [f1, f2] = sides[..] else { continue };
        found += 1;

        let (p, e) = (brush.vertices[a], (brush.vertices[b] - brush.vertices[a]).normalize());
        let (n1, n2) = (brush.faces[f1].plane.normal, brush.faces[f2].plane.normal);
        let inward = |n: DVec3, f: usize| {
            let t = n.cross(e).normalize();
            if t.dot(brush.face_center(f) - p) < 0.0 { -t } else { t }
        };
        let (t1, t2) = (inward(n1, f1), inward(n2, f2));
        let reach = |t: DVec3, f: usize| brush.face_points(f).iter().map(|v| (*v - p).dot(t)).fold(0.0, f64::max);
        if width >= reach(t1, f1) - EPS || width >= reach(t2, f2) - EPS {
            return Err(BevelError::TooWide);
        }

        // The arc touches both faces `width` from the edge, its center lies `radius` inside each of them.
        let interior = t1.dot(t2).clamp(-1.0, 1.0).acos();
        let radius = width * (interior * 0.5).tan();
        let center = p + t1 * width - n1 * radius;
        let n = segments.max(1);
        for k in 0..n {
            let normal = slerp(n1, n2, (k as f64 + 0.5) / n as f64).normalize();
            let at = center + slerp(n1, n2, k as f64 / n as f64) * radius;
            let plane = Plane::from_point_normal(at, normal);
            let face = &brush.faces[if 2 * k < n { f1 } else { f2 }];
            let data = FaceData { uv: FaceUv::wrapped(&face.data.uv, &face.plane, &plane), disp: None, colors: Vec::new(), ..face.data.clone() };
            planes.push((plane, data));
        }
    }

    if found == 0 {
        return Err(BevelError::NoEdge);
    }

    Ok(Brush::from_planes(planes)?)
}

#[cfg(test)]
mod tests {
    use td_core::Aabb;

    use super::*;

    fn cube() -> Brush {
        Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::splat(64.0)), "wall").unwrap()
    }

    const TOP_FRONT: (DVec3, DVec3) = (DVec3::new(0.0, 64.0, 64.0), DVec3::new(64.0, 64.0, 64.0));

    #[test]
    fn a_chamfer_cuts_the_same_width_into_both_faces() {
        let b = bevel_edges(&cube(), &[TOP_FRONT], 16.0, 1).unwrap();
        assert_eq!(b.faces.len(), 7);
        assert!((b.volume() - (64.0f64.powi(3) - 0.5 * 16.0 * 16.0 * 64.0)).abs() < 1e-6, "{}", b.volume());
        for v in [DVec3::new(0.0, 48.0, 64.0), DVec3::new(0.0, 64.0, 48.0), DVec3::new(64.0, 48.0, 64.0)] {
            assert!(b.vertices.iter().any(|w| w.distance(v) < 1e-6), "{v} is a corner of the chamfer");
        }

        assert!(b.faces.iter().all(|f| f.data.material == "wall"));
    }

    #[test]
    fn a_round_bevel_stays_on_the_arc_and_the_points_can_come_in_either_order() {
        let b = bevel_edges(&cube(), &[(TOP_FRONT.1, TOP_FRONT.0)], 16.0, 4).unwrap();
        assert_eq!(b.faces.len(), 10);
        let center = DVec3::new(0.0, 48.0, 48.0);
        let on_arc: Vec<_> = b.vertices.iter().filter(|v| v.x == 0.0 && v.y > 48.0 - 1e-9 && v.z > 48.0 - 1e-9).collect();
        assert_eq!(on_arc.len(), 5);
        for v in on_arc {
            let d = DVec3::new(0.0, v.y, v.z).distance(center);
            assert!((d - 16.0).abs() < 1e-6, "{v} lies on the arc");
        }

        let quarter_circle_cut = 16.0 * 16.0 * (1.0 - std::f64::consts::FRAC_PI_4) * 64.0;
        assert!(b.volume() < 64.0f64.powi(3) - quarter_circle_cut + 1e-6);
    }

    #[test]
    fn two_edges_at_once_and_the_refusals() {
        let side = (DVec3::new(64.0, 0.0, 64.0), DVec3::new(64.0, 64.0, 64.0));
        assert_eq!(bevel_edges(&cube(), &[TOP_FRONT, side], 8.0, 1).unwrap().faces.len(), 8);
        assert_eq!(bevel_edges(&cube(), &[(DVec3::ZERO, DVec3::splat(64.0))], 8.0, 1), Err(BevelError::NoEdge), "a diagonal is no edge");
        assert_eq!(bevel_edges(&cube(), &[TOP_FRONT], 64.0, 1), Err(BevelError::TooWide));
    }
}
