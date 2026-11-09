//! The Shape Generator's own box, drawn in the views while it is open with no brush or mesh selected.

use td_core::{Aabb, DVec3, snap_to_grid};
use td_render::LineVertex;

use crate::scene::v3;
use crate::state::EditorState;
use crate::theme;

/// The box's line color in linear space, the UI accent so it stays apart from every other line in the views.
pub fn color() -> [f32; 4] {
    let [r, g, b, _] = theme::linear(theme::VIEW.shape_bounds());
    [r as f32, g as f32, b as f32, 0.9]
}

pub fn lines(state: &EditorState, out: &mut Vec<LineVertex>) {
    let Some(b) = state.shape_bounds else { return };
    let corners = b.corners();
    for (i, j) in Aabb::EDGES {
        out.push(LineVertex { pos: v3(corners[i]), color: color() });
        out.push(LineVertex { pos: v3(corners[j]), color: color() });
    }
}

/// The box the dialog starts with, around the last brush: every side on the nearest grid line and every axis at
/// least one cell, an empty `last` falls back to a 64 unit cube at the origin.
pub fn seed(last: Aabb, grid: f64) -> Aabb {
    let last = if last.is_empty() { Aabb::new(DVec3::ZERO, DVec3::splat(64.0)) } else { last };
    let axis = |lo: f64, hi: f64| {
        let (lo, hi) = (snap_to_grid(lo, grid), snap_to_grid(hi, grid));
        (lo, if grid > 0.0 && hi < lo + grid { lo + grid } else { hi })
    };
    let (x, y, z) = (axis(last.min.x, last.max.x), axis(last.min.y, last.max.y), axis(last.min.z, last.max.z));
    Aabb::new(DVec3::new(x.0, y.0, z.0), DVec3::new(x.1, y.1, z.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aabb(min: [f64; 3], max: [f64; 3]) -> Aabb {
        Aabb::new(DVec3::from(min), DVec3::from(max))
    }

    #[test]
    fn the_seed_rounds_every_side_to_the_nearest_grid_line() {
        let last = aabb([-37.0, 5.0, 22.0], [51.0, 70.0, 99.0]);
        assert_eq!(seed(last, 16.0), aabb([-32.0, 0.0, 16.0], [48.0, 64.0, 96.0]));
        assert_eq!(seed(last, 8.0), aabb([-40.0, 8.0, 24.0], [48.0, 72.0, 96.0]));
    }

    #[test]
    fn an_empty_last_brush_seeds_the_origin_cube() {
        assert_eq!(seed(Aabb::EMPTY, 16.0), aabb([0.0; 3], [64.0; 3]));
        assert_eq!(seed(Aabb::EMPTY, 128.0), aabb([0.0; 3], [128.0; 3]));
    }

    #[test]
    fn a_flat_or_sub_cell_axis_gets_one_cell() {
        let flat = seed(aabb([-37.0, 5.0, 22.0], [51.0, 5.0, 99.0]), 16.0);
        assert_eq!(flat, aabb([-32.0, 0.0, 16.0], [48.0, 16.0, 96.0]));
        let thin = seed(aabb([-37.0, 2.0, 22.0], [51.0, 6.0, 99.0]), 16.0);
        assert_eq!(thin, aabb([-32.0, 0.0, 16.0], [48.0, 16.0, 96.0]));
        let negative = seed(aabb([-37.0, -22.0, 22.0], [51.0, -18.0, 99.0]), 16.0);
        assert_eq!(negative, aabb([-32.0, -16.0, 16.0], [48.0, 0.0, 96.0]));
    }

    #[test]
    fn exactly_one_cell_is_kept() {
        assert_eq!(seed(aabb([3.0, 3.0, 3.0], [21.0, 20.0, 19.0]), 16.0), aabb([0.0; 3], [16.0, 16.0, 16.0]));
        let one = aabb([-48.0, 16.0, 32.0], [-32.0, 32.0, 48.0]);
        assert_eq!(seed(one, 16.0), one);
    }

    #[test]
    fn half_a_cell_rounds_as_snap_to_grid_does() {
        let s = seed(aabb([8.0, -8.0, 24.0], [40.0, 24.0, 56.0]), 16.0);
        assert_eq!(s.min.x, snap_to_grid(8.0, 16.0));
        assert_eq!(s.min.y, snap_to_grid(-8.0, 16.0));
        assert_eq!(s.min, DVec3::new(16.0, -16.0, 32.0));
    }

    #[test]
    fn a_zero_grid_keeps_the_input() {
        let last = aabb([-37.3, 5.1, 22.9], [51.7, 5.1, 99.5]);
        assert_eq!(seed(last, 0.0), last);
        assert_eq!(seed(Aabb::EMPTY, 0.0), aabb([0.0; 3], [64.0; 3]));
    }
}
