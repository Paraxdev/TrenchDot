//! The Hammer preset's selection box. A drag off the selection in a 2D view draws it, it waits until Enter or a click
//! inside it selects every object lying wholly inside on the view's two axes, at any depth. The Vertex tool draws the
//! same box over corners.

use egui::{Color32, Painter, Rect, Stroke, StrokeKind};
use td_core::{Aabb, DVec3, NodeId};

use crate::camera::{Camera, ViewKind};
use crate::commands::Action;
use crate::i18n::{Msg, msg, msg_n};
use crate::state::{EditorState, Prefs};
use crate::tools::ToolKind;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionBox {
    pub view: ViewKind,
    pub a: DVec3,
    pub b: DVec3,
    /// Ctrl was held when the drag began, so the box adds to the selection.
    pub add: bool,
}

impl SelectionBox {
    pub fn min(&self) -> DVec3 {
        self.a.min(self.b)
    }

    pub fn max(&self) -> DVec3 {
        self.a.max(self.b)
    }

    fn holds(&self, min: DVec3, max: DVec3) -> bool {
        let depth = self.view.depth_axis();
        let (lo, hi) = (self.min(), self.max());
        (0..3).filter(|i| *i != depth).all(|i| min[i] >= lo[i] - 1e-6 && max[i] <= hi[i] + 1e-6)
    }

    /// Whether `bounds` lies wholly inside on the view's two axes, the edges included.
    pub fn contains(&self, bounds: &Aabb) -> bool {
        !bounds.is_empty() && self.holds(bounds.min, bounds.max)
    }

    pub fn contains_point(&self, p: DVec3) -> bool {
        self.holds(p, p)
    }

    pub fn pending_status(&self) -> Msg {
        if self.add { msg!("Press Enter or click inside the box to add to the selection") } else { msg!("Press Enter or click inside the box to select") }
    }
}

/// The objects the box would select, a closed group as one.
pub fn targets(state: &EditorState, b: &SelectionBox) -> Vec<NodeId> {
    let map = &state.doc.map;
    td_doc::ops::selectable_objects(map, &state.open_groups).into_iter().filter(|id| b.contains(&map.bounds(*id))).collect()
}

/// Selects what lies inside the waiting box and drops the box. Returns how many objects it took, with none the
/// selection stays as it was.
pub fn confirm(state: &mut EditorState) -> usize {
    let Some(b) = state.selection_box else { return 0 };
    state.drop_selection_box();
    let found = targets(state, &b);
    if found.is_empty() {
        state.set_status(msg!("Nothing lies wholly inside the box"));
        return 0;
    }

    state.last_bounds = state.doc.map.bounds_of(found.iter().copied());
    state.doc.select(|_, s| {
        if !b.add {
            s.clear();
        }

        s.nodes.extend(found.iter().copied());
    });
    state.set_status(msg_n!(found.len(), "Selected {n} object", "Selected {n} objects"));
    state.bind_status_to_selection();
    found.len()
}

/// The status for a drag off the selection in the Hammer 3D view, which leaves drawing to the Block tool.
pub fn block_status(prefs: &Prefs) -> Msg {
    match crate::commands::shortcut_hint(prefs, &Action::SetTool(ToolKind::Block)) {
        Some(keys) => msg!("The Block tool draws brushes ({keys})", keys),
        None => msg!("The Block tool draws brushes"),
    }
}

pub fn paint(painter: &Painter, cam: &Camera, rect: Rect, state: &EditorState) {
    if let Some(b) = &state.selection_box {
        paint_box(painter, cam, rect, b);
    }
}

/// Draws `b` in the view it was drawn in, with the Mesh tool's box colors.
pub fn paint_box(painter: &Painter, cam: &Camera, rect: Rect, b: &SelectionBox) {
    if b.view != cam.kind {
        return;
    }

    let (Some(p), Some(q)) = (cam.project(rect, b.a), cam.project(rect, b.b)) else { return };
    let r = Rect::from_two_pos(p, q);
    painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 160, 60, 20));
    painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_rgb(255, 160, 60)), StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use td_doc::NodeKind;
    use td_geom::Brush;

    use super::*;

    fn cuboid(min: [f64; 3], max: [f64; 3]) -> NodeKind {
        NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::from(min), DVec3::from(max)), "dev/grey").unwrap())
    }

    fn wedge(min: [f64; 3], max: [f64; 3]) -> NodeKind {
        NodeKind::Brush(td_geom::shapes::wedge(&Aabb::new(DVec3::from(min), DVec3::from(max)), "dev/grey").unwrap())
    }

    fn add(state: &mut EditorState, kind: NodeKind) -> NodeId {
        let layer = state.doc.map.default_layer();
        state.doc.edit("add", |m, _| m.insert(layer, kind))
    }

    fn top(a: [f64; 3], b: [f64; 3], add: bool) -> SelectionBox {
        SelectionBox { view: ViewKind::Top, a: DVec3::from(a), b: DVec3::from(b), add }
    }

    /// A slanted wedge and a tall box inside the Top box, one box over its edge and one far away.
    fn room() -> (EditorState, [NodeId; 4]) {
        let mut state = EditorState::new(Default::default());
        let slanted = add(&mut state, wedge([-300.0, 8.0, 200.0], [-236.0, 72.0, 296.0]));
        let tall = add(&mut state, cuboid([-200.0, 0.0, 210.0], [-168.0, 500.0, 250.0]));
        let straddling = add(&mut state, cuboid([-160.0, 8.0, 200.0], [-100.0, 40.0, 260.0]));
        let away = add(&mut state, cuboid([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]));
        (state, [slanted, tall, straddling, away])
    }

    #[test]
    fn the_box_takes_what_lies_wholly_inside_on_the_view_axes() {
        let (state, [slanted, tall, ..]) = room();
        assert_eq!(targets(&state, &top([-120.0, 0.0, 300.0], [-310.0, 0.0, 190.0], false)), [slanted, tall], "any depth counts in Top");
        assert_eq!(targets(&state, &top([-300.0, 0.0, 200.0], [-236.0, 0.0, 296.0], false)), [slanted], "a corner on the edge is inside");
        assert!(targets(&state, &top([-299.0, 0.0, 200.0], [-236.0, 0.0, 296.0], false)).is_empty());
        assert!(targets(&state, &top([400.0, 0.0, 400.0], [500.0, 0.0, 500.0], false)).is_empty());

        let front = SelectionBox { view: ViewKind::Front, a: DVec3::new(-310.0, 0.0, 9000.0), b: DVec3::new(-120.0, 80.0, 9000.0), add: false };
        assert_eq!(targets(&state, &front), [slanted], "the Front view reads X and Y, the tall box sticks out the top");
    }

    #[test]
    fn hidden_locked_and_closed_groups_follow_the_click_rules() {
        let (mut state, [slanted, tall, ..]) = room();
        let layer = state.doc.map.default_layer();
        let hidden = add(&mut state, cuboid([-290.0, 0.0, 280.0], [-280.0, 8.0, 290.0]));
        let locked = add(&mut state, cuboid([-270.0, 0.0, 280.0], [-260.0, 8.0, 290.0]));
        state.doc.edit("hide", |m, _| {
            td_doc::ops::set_hidden(m, &[hidden], true);
            td_doc::ops::set_locked(m, &[locked], true);
        });
        let group = state.doc.edit("group", |m, s| {
            s.clear();
            s.nodes.extend([slanted, tall]);
            td_doc::ops::group_selection(m, s, "pair", layer).unwrap()
        });
        let b = top([-120.0, 0.0, 300.0], [-310.0, 0.0, 190.0], false);
        assert_eq!(targets(&state, &b), [group], "the closed group once, nothing hidden or locked");

        state.open_groups.push(group);
        assert_eq!(targets(&state, &b), [slanted, tall], "an open group gives its members");
    }

    #[test]
    fn confirm_replaces_or_adds_and_an_empty_box_keeps_the_selection() {
        let (mut state, [slanted, tall, straddling, away]) = room();
        state.doc.select(|_, s| {
            s.clear();
            s.select_node(away);
            s.select_face(straddling, 0);
        });
        state.selection_box = Some(top([-120.0, 0.0, 300.0], [-310.0, 0.0, 190.0], false));
        assert_eq!(confirm(&mut state), 2);
        assert_eq!(state.doc.selection.nodes.iter().copied().collect::<Vec<_>>(), [slanted, tall]);
        assert!(!state.doc.selection.has_faces(), "a new box replaces faces too");
        assert_eq!(state.status, "Selected 2 objects");
        assert!(state.selection_box.is_none());

        state.doc.select(|_, s| {
            s.clear();
            s.select_node(away);
        });
        state.selection_box = Some(top([-300.0, 0.0, 200.0], [-236.0, 0.0, 296.0], true));
        assert_eq!(confirm(&mut state), 1);
        assert_eq!(state.doc.selection.nodes.iter().copied().collect::<Vec<_>>(), [slanted, away]);
        assert_eq!(state.status, "Selected 1 object");

        state.selection_box = Some(top([400.0, 0.0, 400.0], [500.0, 0.0, 500.0], false));
        assert_eq!(confirm(&mut state), 0);
        assert_eq!(state.doc.selection.nodes.len(), 2, "nothing inside keeps the selection");
        assert_eq!(state.status, "Nothing lies wholly inside the box");
        assert!(state.selection_box.is_none());
        assert_eq!(confirm(&mut state), 0, "no box, nothing to confirm");
    }

    #[test]
    fn the_3d_drag_names_the_block_tool_by_its_live_key() {
        let mut prefs = Prefs { keymap_preset: "hammer".into(), ..Default::default() };
        assert_eq!(block_status(&prefs).en, "The Block tool draws brushes (Shift+B)");
        prefs.key_overrides.insert(Action::SetTool(ToolKind::Block).binding_id(), String::new());
        assert_eq!(block_status(&prefs).en, "The Block tool draws brushes");
    }

    #[test]
    fn escape_leaves_no_box_status_behind_once_it_moved_on() {
        use crate::commands::{Action, execute};
        let ctx = egui::Context::default();
        let (mut state, [_, _, _, away]) = room();
        let b = top([-120.0, 0.0, 300.0], [-310.0, 0.0, 190.0], false);
        for selected in [false, true] {
            state.doc.select(|_, s| {
                s.clear();
                if selected {
                    s.select_node(away);
                }
            });
            state.selection_box = Some(b);
            execute(&mut state, Action::Cancel, &ctx);
            assert_eq!(state.status, "Dropped the selection box");
            execute(&mut state, Action::Cancel, &ctx);
            state.expire_selection_status();
            assert_eq!((state.status.as_str(), state.doc.selection.nodes.is_empty()), ("", true), "selected {selected}");
        }

        state.selection_box = Some(b);
        assert_eq!(confirm(&mut state), 2);
        state.expire_selection_status();
        assert_eq!(state.status, "Selected 2 objects");
        execute(&mut state, Action::Cancel, &ctx);
        state.expire_selection_status();
        assert_eq!((state.status.as_str(), state.doc.selection.nodes.is_empty()), ("", true), "the count goes with the selection");
    }

    #[test]
    fn dropping_the_box_clears_only_its_own_status() {
        let (mut state, _) = room();
        let b = top([0.0, 0.0, 0.0], [10.0, 0.0, 10.0], true);
        state.selection_box = Some(b);
        state.set_status(b.pending_status());
        assert!(state.drop_selection_box());
        assert!(state.status.is_empty());
        assert!(!state.drop_selection_box());

        state.selection_box = Some(b);
        state.set_status(msg!("Saved"));
        assert!(state.drop_selection_box());
        assert_eq!(state.status, "Saved");
    }
}
