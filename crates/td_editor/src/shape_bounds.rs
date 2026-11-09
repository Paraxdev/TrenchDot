//! The Shape Generator's own box, drawn in the views while it is open with no brush or mesh selected.

use td_core::Aabb;
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
