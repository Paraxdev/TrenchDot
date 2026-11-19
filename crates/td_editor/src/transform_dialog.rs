//! Transform Selection: the selection moved, turned or scaled by typed numbers, for the exact steps the gizmo cannot give.

use egui::RichText;
use td_core::DVec3;
use td_doc::ops;

use crate::i18n::{Msg, en, msg, tr};
use crate::state::EditorState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Move,
    Rotate,
    Scale,
}

impl Mode {
    const ALL: [Mode; 3] = [Mode::Move, Mode::Rotate, Mode::Scale];

    fn index(self) -> usize {
        self as usize
    }

    fn title(self) -> &'static str {
        match self {
            Mode::Move => en!("Move"),
            Mode::Rotate => en!("Rotate"),
            Mode::Scale => en!("Scale"),
        }
    }

    /// The values that change nothing.
    fn neutral(self) -> DVec3 {
        match self {
            Mode::Scale => DVec3::ONE,
            Mode::Move | Mode::Rotate => DVec3::ZERO,
        }
    }
}

pub struct TransformDialog {
    pub open: bool,
    mode: Mode,
    values: [DVec3; 3],
    focus: bool,
}

impl Default for TransformDialog {
    fn default() -> Self {
        Self { open: false, mode: Mode::Move, values: Mode::ALL.map(Mode::neutral), focus: false }
    }
}

/// Why `values` cannot be applied to the selection, if they cannot.
pub fn refusal(state: &EditorState, mode: Mode, values: DVec3) -> Option<Msg> {
    let map = &state.doc.map;
    let movable = |id: &td_doc::NodeId| map.get(*id).is_some_and(|n| !matches!(n.kind, td_doc::NodeKind::Layer(_) | td_doc::NodeKind::Group(_)));
    if !state.doc.selection.nodes.iter().any(|id| movable(id) || map.descendants(*id).iter().any(movable)) {
        return Some(msg!("Select objects to transform"));
    }

    if mode == Mode::Scale && values.to_array().contains(&0.0) {
        return Some(msg!("A scale factor cannot be 0"));
    }

    (values == mode.neutral()).then(|| msg!("The values change nothing"))
}

/// Moves by `values` units, turns about the selection center by `values` degrees (Y, X, Z euler, like Array Copies) or
/// scales about it by `values` factors, as one undo step.
pub fn apply(state: &mut EditorState, mode: Mode, values: DVec3) -> Result<(), Msg> {
    if let Some(why) = refusal(state, mode, values) {
        return Err(why);
    }

    let opts = state.opts();
    let center = ops::selection_center(&state.doc.map, &state.doc.selection, opts.grid);
    state.doc.edit(mode.title(), |map, sel| match mode {
        Mode::Move => ops::translate_selection(map, sel, values, opts),
        Mode::Rotate => ops::transform_selection(map, sel, &ops::turn_about(center, values), opts),
        Mode::Scale => ops::transform_selection(map, sel, &ops::scale_about(center, values), opts),
    });
    Ok(())
}

impl TransformDialog {
    pub fn open(&mut self) {
        self.open = true;
        self.focus = true;
    }

    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        if !self.open {
            return;
        }

        let mode = &mut self.mode;
        let values = &mut self.values;
        let focus = std::mem::take(&mut self.focus);
        let mut open = self.open;
        let mut applied = None;
        crate::widgets::centered_window(en!("Transform Selection"), ctx).open(&mut open).resizable(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for m in Mode::ALL {
                    ui.radio_value(mode, m, tr(m.title()));
                }
            });
            let (range, speed, suffix) = match *mode {
                Mode::Move => (-65536.0..=65536.0, 1.0, " u"),
                Mode::Rotate => (-360.0..=360.0, 0.5, "°"),
                Mode::Scale => (-100.0..=100.0, 0.01, ""),
            };
            let mut enter = false;
            let v = &mut values[mode.index()];
            ui.horizontal(|ui| {
                for (axis, name) in [(0, "X "), (1, "Y "), (2, "Z ")] {
                    let field = ui.add(egui::DragValue::new(&mut v[axis]).range(range.clone()).speed(speed).prefix(name).suffix(suffix));
                    if axis == 0 && focus {
                        field.request_focus();
                    }

                    enter |= field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                }
            });
            let why = refusal(state, *mode, *v);
            if let Some(why) = &why {
                ui.label(RichText::new(&why.shown).weak());
            }

            let click = ui.add_enabled(why.is_none(), egui::Button::new(tr("Apply"))).clicked();
            if (click || enter) && why.is_none() {
                applied = Some((*mode, *v));
            }
        });
        self.open = open;
        if let Some((mode, v)) = applied {
            let _ = apply(state, mode, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};
    use td_core::{Aabb, NodeId};
    use td_doc::NodeKind;
    use td_geom::Brush;

    const A: (DVec3, DVec3) = (DVec3::new(10.0, -20.0, 5.0), DVec3::new(74.0, 44.0, 69.0));
    const B: (DVec3, DVec3) = (DVec3::new(200.0, 0.0, 0.0), DVec3::new(232.0, 96.0, 160.0));

    // The union of both brushes is (10, -20, 0) to (232, 96, 160), its center (121, 38, 80) snaps to (128, 32, 80).
    const PIVOT: DVec3 = DVec3::new(128.0, 32.0, 80.0);

    fn two_brushes() -> (EditorState, [NodeId; 2]) {
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let ids = [A, B].map(|(min, max)| {
            let brush = Brush::from_aabb(&Aabb::new(min, max), "dev/grey").unwrap();
            state.doc.edit("add", |m, _| m.insert(layer, NodeKind::Brush(brush)))
        });
        for id in ids {
            state.doc.selection.select_node(id);
        }

        (state, ids)
    }

    fn bounds(state: &EditorState, id: NodeId) -> Aabb {
        state.doc.map.bounds(id)
    }

    fn near(a: &Aabb, b: &Aabb) -> bool {
        a.min.abs_diff_eq(b.min, 1e-6) && a.max.abs_diff_eq(b.max, 1e-6)
    }

    fn undo_steps(state: &EditorState) -> usize {
        state.doc.history.undo_labels().count()
    }

    fn unmoved(state: &EditorState, id: NodeId, from: (DVec3, DVec3)) -> bool {
        near(&bounds(state, id), &Aabb::new(from.0, from.1))
    }

    /// The bounds of the corners of `b` through `f`.
    fn mapped(b: (DVec3, DVec3), f: impl Fn(DVec3) -> DVec3) -> Aabb {
        let mut corners = Vec::new();
        for x in [b.0.x, b.1.x] {
            for y in [b.0.y, b.1.y] {
                for z in [b.0.z, b.1.z] {
                    corners.push(f(DVec3::new(x, y, z)));
                }
            }
        }

        Aabb::from_points(corners)
    }

    #[test]
    fn move_offsets_every_brush_by_the_typed_units_without_snapping() {
        let (mut state, [a, b]) = two_brushes();
        let steps = undo_steps(&state);
        let by = DVec3::new(16.0, -8.0, 40.5);
        apply(&mut state, Mode::Move, by).unwrap();
        assert!(near(&bounds(&state, a), &Aabb::new(A.0 + by, A.1 + by)));
        assert!(near(&bounds(&state, b), &Aabb::new(B.0 + by, B.1 + by)));
        assert_eq!(undo_steps(&state), steps + 1);
        assert_eq!(state.doc.history.undo_labels().next(), Some("Move"));
        state.doc.undo();
        assert!(unmoved(&state, a, A) && unmoved(&state, b, B));
    }

    #[test]
    fn rotate_turns_both_brushes_about_the_snapped_selection_center() {
        let (mut state, [a, b]) = two_brushes();
        let steps = undo_steps(&state);
        apply(&mut state, Mode::Rotate, DVec3::new(0.0, 30.0, 0.0)).unwrap();
        let (s, c) = 30f64.to_radians().sin_cos();
        let turn = |p: DVec3| {
            let d = p - PIVOT;
            PIVOT + DVec3::new(d.x * c + d.z * s, d.y, -d.x * s + d.z * c)
        };
        assert!(near(&bounds(&state, a), &mapped(A, turn)), "{:?}", bounds(&state, a));
        assert!(near(&bounds(&state, b), &mapped(B, turn)), "{:?}", bounds(&state, b));
        assert_eq!(undo_steps(&state), steps + 1);
        assert_eq!(state.doc.history.undo_labels().next(), Some("Rotate"));
    }

    #[test]
    fn rotate_uses_the_exact_center_when_snapping_is_off() {
        let (mut state, [a, _]) = two_brushes();
        state.snap = false;
        apply(&mut state, Mode::Rotate, DVec3::new(0.0, 0.0, 90.0)).unwrap();
        let center = DVec3::new(121.0, 38.0, 80.0);
        let turn = |p: DVec3| {
            let d = p - center;
            center + DVec3::new(-d.y, d.x, d.z)
        };
        assert!(near(&bounds(&state, a), &mapped(A, turn)), "{:?}", bounds(&state, a));
    }

    #[test]
    fn scale_multiplies_about_the_center_and_a_negative_factor_mirrors() {
        let (mut state, [a, b]) = two_brushes();
        let steps = undo_steps(&state);
        let factors = DVec3::new(2.0, 1.0, 0.5);
        apply(&mut state, Mode::Scale, factors).unwrap();
        let grow = |p: DVec3| PIVOT + (p - PIVOT) * factors;
        assert!(near(&bounds(&state, a), &mapped(A, grow)), "{:?}", bounds(&state, a));
        assert!(near(&bounds(&state, b), &mapped(B, grow)), "{:?}", bounds(&state, b));
        assert_eq!(undo_steps(&state), steps + 1);
        assert_eq!(state.doc.history.undo_labels().next(), Some("Scale"));

        state.doc.undo();
        let flip = DVec3::new(1.0, -1.0, 1.0);
        apply(&mut state, Mode::Scale, flip).unwrap();
        assert!(near(&bounds(&state, a), &mapped(A, |p| PIVOT + (p - PIVOT) * flip)));
        let Some(NodeKind::Brush(mirrored)) = state.doc.map.get(a).map(|n| &n.kind) else { panic!("a brush") };
        assert!(mirrored.validate().is_ok(), "a mirrored brush stays a valid solid");
    }

    #[test]
    fn only_the_selected_brush_moves() {
        let (mut state, [a, b]) = two_brushes();
        state.doc.selection.clear();
        state.doc.selection.select_node(a);
        let by = DVec3::new(0.0, 0.0, 8.0);
        apply(&mut state, Mode::Move, by).unwrap();
        assert!(near(&bounds(&state, a), &Aabb::new(A.0 + by, A.1 + by)));
        assert!(unmoved(&state, b, B));
    }

    #[test]
    fn nothing_selected_is_refused_in_every_mode_and_leaves_no_undo_step() {
        let (mut state, [a, b]) = two_brushes();
        state.doc.selection.clear();
        let steps = undo_steps(&state);
        for (mode, v) in [(Mode::Move, DVec3::X), (Mode::Rotate, DVec3::Y), (Mode::Scale, DVec3::splat(2.0))] {
            assert_eq!(apply(&mut state, mode, v).unwrap_err().en, "Select objects to transform", "{mode:?}");
        }

        state.doc.selection.select_face(a, 0);
        assert!(apply(&mut state, Mode::Move, DVec3::X).is_err(), "a selected face is not an object to transform");
        assert_eq!(undo_steps(&state), steps);
        assert!(unmoved(&state, a, A) && unmoved(&state, b, B));
    }

    #[test]
    fn values_that_change_nothing_are_refused() {
        let (mut state, [a, _]) = two_brushes();
        let steps = undo_steps(&state);
        for (mode, v) in [(Mode::Move, DVec3::ZERO), (Mode::Rotate, DVec3::ZERO), (Mode::Scale, DVec3::ONE)] {
            assert_eq!(apply(&mut state, mode, v).unwrap_err().en, "The values change nothing", "{mode:?}");
        }

        assert_eq!(undo_steps(&state), steps);
        // 1 is nothing to Scale but a real offset to Move.
        apply(&mut state, Mode::Move, DVec3::ONE).unwrap();
        assert_eq!(bounds(&state, a).min, A.0 + DVec3::ONE);
    }

    #[test]
    fn a_zero_scale_factor_on_any_one_axis_is_refused_and_a_tiny_one_is_not() {
        let (mut state, [a, b]) = two_brushes();
        let steps = undo_steps(&state);
        for axis in 0..3 {
            let mut v = DVec3::splat(2.0);
            v[axis] = 0.0;
            assert_eq!(apply(&mut state, Mode::Scale, v).unwrap_err().en, "A scale factor cannot be 0", "axis {axis}");
            v[axis] = 0.001;
            assert!(refusal(&state, Mode::Scale, v).is_none(), "axis {axis}");
        }

        assert_eq!(undo_steps(&state), steps);
        assert!(unmoved(&state, a, A) && unmoved(&state, b, B));
    }

    #[test]
    fn apply_honours_uv_lock() {
        let uvs = |state: &EditorState, id: NodeId| match &state.doc.map.get(id).unwrap().kind {
            NodeKind::Brush(b) => b.faces.iter().map(|f| f.data.uv.clone()).collect::<Vec<_>>(),
            _ => unreachable!(),
        };
        let by = DVec3::new(16.0, -8.0, 40.0);
        let (mut locked, [id, _]) = two_brushes();
        let (mut unlocked, _) = two_brushes();
        let before = uvs(&locked, id);
        unlocked.uv_lock = false;
        apply(&mut locked, Mode::Move, by).unwrap();
        apply(&mut unlocked, Mode::Move, by).unwrap();
        assert_eq!(uvs(&unlocked, id), before, "without the lock the texture slides with the brush");
        assert_ne!(uvs(&locked, id), before, "with the lock the offsets follow the move");
    }

    struct Fixture {
        dialog: TransformDialog,
        state: EditorState,
    }

    fn harness(state: EditorState) -> egui_kittest::Harness<'static, Fixture> {
        let mut dialog = TransformDialog::default();
        dialog.open();
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(700.0, 400.0))
            .build_ui_state(|ui, f: &mut Fixture| f.dialog.show(ui.ctx(), &mut f.state), Fixture { dialog, state });
        harness.run();
        harness
    }

    fn spin<'a>(harness: &'a egui_kittest::Harness<'static, Fixture>, field: usize) -> egui_kittest::Node<'a> {
        harness.get_all_by_role(egui::accesskit::Role::SpinButton).nth(field).unwrap()
    }

    fn type_into(harness: &mut egui_kittest::Harness<'static, Fixture>, field: usize, text: &str) {
        spin(harness, field).click();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.run();
        spin(harness, field).type_text(text);
        harness.run();
    }

    #[test]
    fn typing_an_offset_and_pressing_apply_moves_the_brush() {
        let (state, [a, _]) = two_brushes();
        let mut harness = harness(state);
        type_into(&mut harness, 0, "24");
        type_into(&mut harness, 2, "-12");
        assert!(unmoved(&harness.state().state, a, A), "nothing moves before Apply");
        harness.get_by_label("Apply").click();
        harness.run();
        let by = DVec3::new(24.0, 0.0, -12.0);
        assert!(near(&bounds(&harness.state().state, a), &Aabb::new(A.0 + by, A.1 + by)));
        assert!(harness.state().dialog.open, "Apply keeps the window open");

        harness.get_by_label("Apply").click();
        harness.run();
        assert!(near(&bounds(&harness.state().state, a), &Aabb::new(A.0 + by * 2.0, A.1 + by * 2.0)), "the same step applies again");
    }

    #[test]
    fn enter_in_a_field_applies_and_each_mode_keeps_its_own_values() {
        let (state, [a, _]) = two_brushes();
        let mut harness = harness(state);
        type_into(&mut harness, 1, "8");
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(bounds(&harness.state().state, a).min.y, A.0.y + 8.0);

        harness.get_by_label("Scale").click();
        harness.run();
        assert_eq!(harness.state().dialog.mode, Mode::Scale);
        harness.get_by_label("Move").click();
        harness.run();
        assert_eq!(harness.state().dialog.values[Mode::Move.index()], DVec3::new(0.0, 8.0, 0.0), "Move keeps what was typed");
        assert_eq!(harness.state().dialog.values[Mode::Scale.index()], DVec3::ONE);
    }

    #[test]
    fn apply_is_disabled_with_the_reason_shown() {
        let (mut state, [a, _]) = two_brushes();
        state.doc.selection.clear();
        let mut harness = harness(state);
        assert!(harness.query_by_label_contains("Select objects to transform").is_some());
        assert!(harness.get_by_label("Apply").accesskit_node().is_disabled());
        harness.get_by_label("Scale").click();
        harness.state_mut().state.doc.selection.select_node(a);
        harness.run();
        assert!(harness.query_by_label_contains("The values change nothing").is_some());
        assert!(harness.get_by_label("Apply").accesskit_node().is_disabled());
        type_into(&mut harness, 0, "0");
        assert!(harness.query_by_label_contains("A scale factor cannot be 0").is_some());
        type_into(&mut harness, 0, "2");
        assert!(!harness.get_by_label("Apply").accesskit_node().is_disabled());
    }

    #[test]
    fn the_x_field_has_focus_when_the_window_opens() {
        let (state, _) = two_brushes();
        let harness = harness(state);
        assert!(spin(&harness, 0).accesskit_node().is_focused());
        assert!(!spin(&harness, 1).accesskit_node().is_focused());
    }
}
