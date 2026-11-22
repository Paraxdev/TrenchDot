use td_core::{DVec2, DVec3};

use crate::brush::Brush;
use crate::csg::{CarveMaterial, subtract_with};

/// One rectangular part of a roof footprint on the XZ plane, `min` and `max` holding world X and Z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wing {
    pub min: DVec2,
    pub max: DVec2,
    /// The ridge runs along X, else along Z.
    pub ridge_x: bool,
}

impl Wing {
    /// A wing whose ridge runs along its longer side.
    pub fn new(min: DVec2, max: DVec2) -> Self {
        let size = max - min;
        Self { min, max, ridge_x: size.x >= size.y }
    }

    fn long(&self) -> (f64, f64) {
        if self.ridge_x { (self.min.x, self.max.x) } else { (self.min.y, self.max.y) }
    }

    fn across(&self) -> (f64, f64) {
        if self.ridge_x { (self.min.y, self.max.y) } else { (self.min.x, self.max.x) }
    }

    /// This wing's span along the axis `other` has its ridge on.
    fn span_on(&self, other: &Wing) -> (f64, f64) {
        if other.ridge_x { (self.min.x, self.max.x) } else { (self.min.y, self.max.y) }
    }

    fn span_across(&self, other: &Wing) -> (f64, f64) {
        if other.ridge_x { (self.min.y, self.max.y) } else { (self.min.x, self.max.x) }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoofEnd {
    /// A free end of a ridge stops in an upright triangle.
    #[default]
    Gable,
    /// A free end slopes down at the roof's pitch.
    Hip,
}

const EPS: f64 = 1e-6;

/// Where the ridge of `wings[i]` ends at its low (`hi = false`) or high end: the center line of a wing it runs into,
/// so the two roofs meet in valleys, or `None` for a free end.
fn joined_end(wings: &[Wing], i: usize, hi: bool) -> Option<f64> {
    let w = &wings[i];
    let (lo, up) = w.long();
    let (c0, c1) = w.across();
    wings.iter().enumerate().filter(|&(j, _)| j != i).find_map(|(_, o)| {
        let (a, b) = o.span_on(w);
        let (d0, d1) = o.span_across(w);
        let covers = d0 <= c0 + EPS && d1 >= c1 - EPS;
        let beyond = if hi { b > up + EPS && a <= up + EPS } else { a < lo - EPS && b >= lo - EPS };
        (covers && beyond).then_some((a + b) * 0.5)
    })
}

fn wing_prism(wings: &[Wing], i: usize, base: f64, slope: f64, ends: RoofEnd, material: &str) -> Option<Brush> {
    let w = &wings[i];
    let (c0, c1) = w.across();
    let half = (c1 - c0) * 0.5;
    let rise = half * slope;
    let (lo_join, hi_join) = (joined_end(wings, i, false), joined_end(wings, i, true));
    let (lo, hi) = (lo_join.unwrap_or(w.long().0), hi_join.unwrap_or(w.long().1));
    let hip = |joined: Option<f64>| if joined.is_none() && ends == RoofEnd::Hip { half } else { 0.0 };
    let (mut r0, mut r1) = (lo + hip(lo_join), hi - hip(hi_join));
    if r0 > r1 {
        let mid = (lo + hi) * 0.5;
        (r0, r1) = (mid, mid);
    }

    let at = |l: f64, c: f64, y: f64| if w.ridge_x { DVec3::new(l, y, c) } else { DVec3::new(c, y, l) };
    let mid = (c0 + c1) * 0.5;
    let points = [at(lo, c0, base), at(hi, c0, base), at(hi, c1, base), at(lo, c1, base), at(r0, mid, base + rise), at(r1, mid, base + rise)];
    Brush::from_points(&points, &[], material).ok()
}

/// A roof over a footprint of rectangular wings resting at height `base`, every slope at `pitch` degrees. A wing whose
/// ridge end runs into another wing stretches to that wing's center line, so an L or T meets in valleys, and the free
/// ends become gables or hips. The pieces do not overlap.
pub fn roof(wings: &[Wing], base: f64, pitch: f64, ends: RoofEnd, material: &str) -> Vec<Brush> {
    let slope = pitch.clamp(1.0, 89.0).to_radians().tan();
    let prisms: Vec<Brush> = (0..wings.len()).filter_map(|i| wing_prism(wings, i, base, slope, ends, material)).collect();
    let mut out = Vec::new();
    for (i, prism) in prisms.iter().enumerate() {
        let mut pieces = vec![prism.clone()];
        for earlier in &prisms[..i] {
            pieces = pieces.iter().flat_map(|p| subtract_with(p, earlier, CarveMaterial::Target)).collect();
        }

        out.extend(pieces.into_iter().filter(|p| p.volume() > 1e-3));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wing(x0: f64, z0: f64, x1: f64, z1: f64) -> Wing {
        Wing::new(DVec2::new(x0, z0), DVec2::new(x1, z1))
    }

    fn volume(brushes: &[Brush]) -> f64 {
        brushes.iter().map(Brush::volume).sum()
    }

    fn has_vertex(brushes: &[Brush], v: DVec3) -> bool {
        brushes.iter().any(|b| b.vertices.iter().any(|w| w.distance(v) < 1e-6))
    }

    #[test]
    fn one_wing_is_a_gable_or_a_hip_roof() {
        let w = [wing(0.0, 0.0, 256.0, 64.0)];
        let gable = roof(&w, 100.0, 45.0, RoofEnd::Gable, "roof");
        assert_eq!(gable.len(), 1);
        assert!((volume(&gable) - 256.0 * 64.0 * 32.0 / 2.0).abs() < 1e-6);
        assert!(has_vertex(&gable, DVec3::new(0.0, 132.0, 32.0)) && has_vertex(&gable, DVec3::new(256.0, 132.0, 32.0)));

        let hip = roof(&w, 100.0, 45.0, RoofEnd::Hip, "roof");
        assert!(has_vertex(&hip, DVec3::new(32.0, 132.0, 32.0)) && has_vertex(&hip, DVec3::new(224.0, 132.0, 32.0)));
        let pyramid = roof(&[wing(0.0, 0.0, 64.0, 64.0)], 0.0, 45.0, RoofEnd::Hip, "roof");
        assert!((volume(&pyramid) - 64.0 * 64.0 * 32.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn a_t_stem_runs_to_the_bar_ridge_and_meets_it_in_valleys() {
        let wings = [wing(0.0, 0.0, 320.0, 96.0), wing(112.0, 96.0, 208.0, 320.0)];
        assert!(!wings[1].ridge_x);
        let pieces = roof(&wings, 0.0, 45.0, RoofEnd::Gable, "roof");
        assert!(has_vertex(&pieces, DVec3::new(160.0, 48.0, 48.0)), "the stem ridge reaches the bar ridge");
        assert!(has_vertex(&pieces, DVec3::new(112.0, 0.0, 96.0)) && has_vertex(&pieces, DVec3::new(208.0, 0.0, 96.0)), "valleys start at the inner corners");
        let bar = 320.0 * 96.0 * 48.0 / 2.0;
        let stem = 96.0 * 224.0 * 48.0 / 2.0;
        let above_bar = 96.0 * 48.0 * 48.0 / 6.0;
        assert!((volume(&pieces) - (bar + stem + above_bar)).abs() < 1e-3, "{}", volume(&pieces));
        assert!(pieces.iter().all(|p| p.validate().is_ok()));
    }

    #[test]
    fn an_l_of_overlapping_wings_crosses_its_gables_without_overlap() {
        let wings = [wing(0.0, 0.0, 256.0, 96.0), wing(0.0, 0.0, 96.0, 256.0)];
        let pieces = roof(&wings, 0.0, 45.0, RoofEnd::Gable, "roof");
        let each = 256.0 * 96.0 * 48.0 / 2.0;
        // Under both roofs in the corner square lies a pyramid.
        let shared = 96.0 * 96.0 * 48.0 / 3.0;
        assert!((volume(&pieces) - (2.0 * each - shared)).abs() < 1e-3, "{}", volume(&pieces));
        assert!(has_vertex(&pieces, DVec3::new(96.0, 0.0, 96.0)) && has_vertex(&pieces, DVec3::new(48.0, 48.0, 48.0)));
    }
}
