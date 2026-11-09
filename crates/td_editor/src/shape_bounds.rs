//! The Shape Generator's own box, drawn in the views while it is open with no brush or mesh selected.

use td_core::{Aabb, DVec3, major_axis, snap_to_grid};
use td_render::LineVertex;

use crate::scene::v3;
use crate::state::EditorState;
use crate::theme;
use crate::viewport::cell_span;

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

/// A box dragged from `start` to `current` in a 2D view, over the cells under both ends like a drawn brush, with
/// `base`'s range along the axis the view looks down.
pub fn drawn(start: DVec3, current: DVec3, base: &Aabb, depth_axis: usize, grid: f64, snap: bool) -> Aabb {
    let (mut min, mut max) = (base.min, base.max);
    for i in (0..3).filter(|i| *i != depth_axis) {
        (min[i], max[i]) = cell_span(start[i], current[i], grid, snap);
    }

    Aabb::new(min, max)
}

/// `base` with the side facing `normal` moved to `to`, stopping `min_size` short of the opposite side.
pub fn with_side(base: &Aabb, normal: DVec3, to: f64, min_size: f64) -> Aabb {
    let i = major_axis(normal);
    let mut b = *base;
    if normal[i] > 0.0 {
        b.max[i] = to.max(b.min[i] + min_size);
    } else {
        b.min[i] = to.min(b.max[i] - min_size);
    }

    b
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

    const BASE: ([f64; 3], [f64; 3]) = ([-37.5, 5.25, 22.0], [51.0, 70.5, 99.0]);

    fn base() -> Aabb {
        aabb(BASE.0, BASE.1)
    }

    #[test]
    fn a_drawn_box_covers_the_cells_under_both_ends_and_keeps_the_depth() {
        let (start, current) = (DVec3::new(-100.0, 300.0, -60.0), DVec3::new(20.0, -700.0, 40.0));
        assert_eq!(drawn(start, current, &base(), 1, 16.0, true), aabb([-112.0, 5.25, -64.0], [32.0, 70.5, 48.0]));
    }

    #[test]
    fn a_mirrored_drag_draws_the_same_box() {
        let (a, b) = (DVec3::new(-100.0, 0.0, -60.0), DVec3::new(20.0, 0.0, 40.0));
        let expected = aabb([-112.0, 5.25, -64.0], [32.0, 70.5, 48.0]);
        assert_eq!(drawn(b, a, &base(), 1, 16.0, true), expected);
        let (c, d) = (DVec3::new(20.0, 0.0, -60.0), DVec3::new(-100.0, 0.0, 40.0));
        assert_eq!(drawn(c, d, &base(), 1, 16.0, true), expected, "mirrored along one axis only");
        assert_eq!(drawn(d, c, &base(), 1, 16.0, true), expected);
    }

    #[test]
    fn every_depth_axis_keeps_the_base_range_exactly() {
        let (start, current) = (DVec3::new(-100.0, 37.0, -60.0), DVec3::new(20.0, 90.0, 40.0));
        assert_eq!(drawn(start, current, &base(), 0, 16.0, true), aabb([-37.5, 32.0, -64.0], [51.0, 96.0, 48.0]));
        assert_eq!(drawn(start, current, &base(), 1, 16.0, true), aabb([-112.0, 5.25, -64.0], [32.0, 70.5, 48.0]));
        assert_eq!(drawn(start, current, &base(), 2, 16.0, true), aabb([-112.0, 32.0, 22.0], [32.0, 96.0, 99.0]));
    }

    #[test]
    fn snap_off_keeps_the_raw_ends() {
        let (start, current) = (DVec3::new(20.7, 0.0, -60.2), DVec3::new(-100.3, 0.0, 40.1));
        assert_eq!(drawn(start, current, &base(), 1, 16.0, false), aabb([-100.3, 5.25, -60.2], [20.7, 70.5, 40.1]));
    }

    #[test]
    fn a_click_draws_one_cell() {
        let at = DVec3::new(-29.0, 3.0, 3.0);
        assert_eq!(drawn(at, at, &base(), 1, 16.0, true), aabb([-32.0, 5.25, 0.0], [-16.0, 70.5, 16.0]));
        assert_eq!(drawn(at, at, &base(), 1, 16.0, false), aabb([-29.0, 5.25, 3.0], [-13.0, 70.5, 19.0]));
    }

    #[test]
    fn a_box_drawn_far_off_the_origin_stays_on_the_grid() {
        let (start, current) = (DVec3::new(-130.0, 0.0, -250.0), DVec3::new(-100.0, 0.0, -200.0));
        assert_eq!(drawn(start, current, &base(), 1, 16.0, true), aabb([-144.0, 5.25, -256.0], [-96.0, 70.5, -192.0]));
    }

    fn sided() -> Aabb {
        aabb([-112.0, 16.0, -64.0], [32.0, 80.0, 48.0])
    }

    #[test]
    fn a_side_moves_only_its_own_face() {
        assert_eq!(with_side(&sided(), DVec3::X, 77.0, 16.0), aabb([-112.0, 16.0, -64.0], [77.0, 80.0, 48.0]));
        assert_eq!(with_side(&sided(), DVec3::NEG_Y, -40.0, 16.0), aabb([-112.0, -40.0, -64.0], [32.0, 80.0, 48.0]));
        assert_eq!(with_side(&sided(), DVec3::Z, 100.0, 16.0), aabb([-112.0, 16.0, -64.0], [32.0, 80.0, 100.0]));
        assert_eq!(with_side(&sided(), DVec3::NEG_X, -20.0, 16.0), aabb([-20.0, 16.0, -64.0], [32.0, 80.0, 48.0]));
    }

    #[test]
    fn a_side_stops_min_size_short_of_the_opposite_one() {
        let at = |to| with_side(&sided(), DVec3::X, to, 16.0).max.x;
        assert_eq!(at(-96.0), -96.0, "exactly one cell is accepted");
        assert_eq!(at(-100.0), -96.0);
        assert_eq!(at(-200.0), -96.0, "past the opposite side");
        let at = |to| with_side(&sided(), DVec3::NEG_X, to, 16.0).min.x;
        assert_eq!(at(16.0), 16.0);
        assert_eq!(at(20.0), 16.0);
        assert_eq!(at(200.0), 16.0);
        assert_eq!(with_side(&sided(), DVec3::NEG_X, 200.0, 16.0), aabb([16.0, 16.0, -64.0], [32.0, 80.0, 48.0]));
    }

    #[test]
    fn with_snap_off_a_side_stops_one_unit_short() {
        let at = |to| with_side(&sided(), DVec3::Z, to, 1.0).max.z;
        assert_eq!(at(-62.75), -62.75);
        assert_eq!(at(-63.0), -63.0);
        assert_eq!(at(-63.5), -63.0);
        let at = |to| with_side(&sided(), DVec3::NEG_Y, to, 1.0).min.y;
        assert_eq!(at(79.0), 79.0);
        assert_eq!(at(79.25), 79.0);
    }
}
