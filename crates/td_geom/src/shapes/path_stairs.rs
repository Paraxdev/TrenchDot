//! Stairs along a path. Straight flights run between the points, each point's height is the floor there, and every
//! point between the ends gets a landing.

use td_core::DVec3;

use super::hull;
use crate::brush::Brush;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathStairs {
    pub width: f64,
    /// The tallest a step may be. A flight takes as many steps as it needs to stay at or under it.
    pub step: f64,
}

/// What [`path_stairs`] builds, the solid steps and landings and, when asked for, a ramp over each flight.
#[derive(Debug, Default)]
pub struct PathStairsParts {
    pub steps: Vec<Brush>,
    pub ramps: Vec<Brush>,
}

fn flat(v: DVec3) -> DVec3 {
    DVec3::new(v.x, 0.0, v.z)
}

/// The corners of a box `width` wide around the run from `a` to `b` along `dir`, from `bottom` up to `top`.
fn slab(a: DVec3, b: DVec3, dir: DVec3, half: f64, bottom: f64, top: f64) -> Vec<DVec3> {
    let side = DVec3::new(-dir.z, 0.0, dir.x) * half;
    [a + side, a - side, b + side, b - side].into_iter().flat_map(|p| [DVec3::new(p.x, bottom, p.z), DVec3::new(p.x, top, p.z)]).collect()
}

/// Builds the stairs through `points`. Everything is solid down to one step under the lowest point, so the stairs
/// stand on the floor there. A flight that rises takes `ceil(rise / step)` steps, a level one becomes a walkway. With
/// `ramp_material`, each flight also gets a ramp brush touching the step noses, for a clip texture that lets players
/// walk up smoothly.
pub fn path_stairs(points: &[DVec3], spec: &PathStairs, material: &str, ramp_material: Option<&str>) -> PathStairsParts {
    let mut pts: Vec<DVec3> = Vec::with_capacity(points.len());
    for p in points {
        if pts.last().is_none_or(|q| flat(*p - *q).length() > 1e-6) {
            pts.push(*p);
        }
    }

    let mut parts = PathStairsParts::default();
    if pts.len() < 2 || spec.width <= 0.0 || spec.step <= 0.0 {
        return parts;
    }

    let half = spec.width * 0.5;
    let bottom = pts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min) - spec.step;
    let dirs: Vec<DVec3> = pts.windows(2).map(|w| flat(w[1] - w[0]).normalize()).collect();
    let last = pts.len() - 1;
    for i in 1..last {
        let p = pts[i];
        let mut corners = Vec::new();
        for d in [dirs[i - 1], dirs[i]] {
            corners.extend(slab(p - d * half, p + d * half, d, half, bottom, p.y));
        }

        parts.steps.extend(hull(&corners, material));
    }

    for (i, d) in dirs.iter().copied().enumerate() {
        let (from, to) = (pts[i], pts[i + 1]);
        let start = flat(from) + if i > 0 { d * half } else { DVec3::ZERO };
        let end = flat(to) - if i + 1 < last { d * half } else { DVec3::ZERO };
        let run = (end - start).dot(d);
        if run <= 1e-3 {
            continue;
        }

        let rise = to.y - from.y;
        if rise.abs() < 1e-6 {
            parts.steps.extend(hull(&slab(start, end, d, half, bottom, from.y), material));
            continue;
        }

        // Laid out from the low end, so going down a flight looks the same as going up it.
        let (low, high, up) = if rise > 0.0 { (start, end, d) } else { (end, start, -d) };
        let (low_y, high_y) = (from.y.min(to.y), from.y.max(to.y));
        let n = (rise.abs() / spec.step - 1e-9).ceil().max(1.0) as usize;
        let tread = run / n as f64;
        for k in 0..n {
            let top = low_y + rise.abs() * (k + 1) as f64 / n as f64;
            parts.steps.extend(hull(&slab(low + up * (tread * k as f64), low + up * (tread * (k + 1) as f64), up, half, bottom, top), material));
        }

        if let Some(ramp) = ramp_material {
            let side = DVec3::new(-up.z, 0.0, up.x) * half;
            let first = low_y + rise.abs() / n as f64;
            let nose = low + up * (run - tread);
            let profile = [(low, low_y), (low, first), (nose, high_y), (high, high_y), (high, low_y)];
            let corners: Vec<DVec3> = profile.iter().flat_map(|(at, y)| [*at + side, *at - side].map(|p| DVec3::new(p.x, *y, p.z))).collect();
            parts.ramps.extend(hull(&corners, ramp));
        }
    }

    parts
}

#[cfg(test)]
mod tests {
    use td_core::Aabb;

    use super::*;

    fn bounds(brushes: &[Brush]) -> Vec<Aabb> {
        brushes.iter().map(|b| b.bounds()).collect()
    }

    const SPEC: PathStairs = PathStairs { width: 64.0, step: 16.0 };

    #[test]
    fn a_straight_flight_rises_in_even_steps_from_the_first_point() {
        let parts = path_stairs(&[DVec3::ZERO, DVec3::new(128.0, 64.0, 0.0)], &SPEC, "dev/grey", None);
        let b = bounds(&parts.steps);
        assert_eq!(b.len(), 4);
        assert!(parts.ramps.is_empty());
        for (k, step) in b.iter().enumerate() {
            assert_eq!((step.min.x, step.max.x), (32.0 * k as f64, 32.0 * (k + 1) as f64));
            assert_eq!((step.min.y, step.max.y), (-16.0, 16.0 * (k + 1) as f64), "solid down to a step under the lowest point");
            assert_eq!((step.min.z, step.max.z), (-32.0, 32.0));
        }
    }

    #[test]
    fn going_down_mirrors_going_up_and_a_step_never_passes_the_limit() {
        let up = bounds(&path_stairs(&[DVec3::ZERO, DVec3::new(100.0, 40.0, 0.0)], &SPEC, "m", None).steps);
        let down = bounds(&path_stairs(&[DVec3::new(100.0, 40.0, 0.0), DVec3::ZERO], &SPEC, "m", None).steps);
        assert_eq!(up.len(), 3, "40 high in steps of at most 16");
        let key = |b: &Aabb| ((b.min.x * 1000.0).round() as i64, (b.max.y * 1000.0).round() as i64);
        let (mut up, mut down): (Vec<_>, Vec<_>) = (up.iter().map(key).collect(), down.iter().map(key).collect());
        up.sort();
        down.sort();
        assert_eq!(up, down);
    }

    #[test]
    fn a_middle_point_gets_a_landing_the_flights_stop_at() {
        let pts = [DVec3::ZERO, DVec3::new(128.0, 32.0, 0.0), DVec3::new(128.0, 64.0, -160.0)];
        let parts = path_stairs(&pts, &SPEC, "m", Some("clip"));
        let b = bounds(&parts.steps);
        let landing = b[0];
        assert_eq!((landing.min.x, landing.max.x, landing.min.z, landing.max.z, landing.max.y), (96.0, 160.0, -32.0, 32.0, 32.0));
        assert!(b[1..3].iter().all(|s| s.max.x <= 96.0 + 1e-9), "the first flight ends at the landing");
        assert!(b[3..].iter().all(|s| s.max.z <= -32.0 + 1e-9 && s.min.x >= 96.0 - 1e-9), "the second flight leaves its far side");
        assert_eq!(b.len(), 5);
        assert_eq!(parts.ramps.len(), 2);
        assert!(parts.ramps.iter().all(|r| r.faces.iter().all(|f| f.data.material == "clip")));
    }

    #[test]
    fn the_ramp_touches_every_step_nose() {
        let parts = path_stairs(&[DVec3::ZERO, DVec3::new(128.0, 64.0, 0.0)], &SPEC, "m", Some("clip"));
        let ramp = &parts.ramps[0];
        for k in 0..4 {
            let nose = DVec3::new(32.0 * k as f64, 16.0 * (k + 1) as f64, 0.0);
            assert!(ramp.faces.iter().all(|f| f.plane.distance(nose) <= 1e-6), "nose {k} inside or on the ramp");
        }

        assert_eq!(ramp.bounds().max.y, 64.0);
    }

    #[test]
    fn a_level_run_is_a_walkway_and_too_few_points_build_nothing() {
        let parts = path_stairs(&[DVec3::ZERO, DVec3::new(0.0, 0.0, 96.0)], &SPEC, "m", Some("clip"));
        assert_eq!(bounds(&parts.steps), [Aabb::new(DVec3::new(-32.0, -16.0, 0.0), DVec3::new(32.0, 0.0, 96.0))]);
        assert!(parts.ramps.is_empty());
        assert!(path_stairs(&[DVec3::ZERO], &SPEC, "m", None).steps.is_empty());
        assert!(path_stairs(&[DVec3::ZERO, DVec3::Y * 64.0], &SPEC, "m", None).steps.is_empty(), "straight up has no run");
    }
}
