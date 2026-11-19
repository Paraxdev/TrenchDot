//! E in the Blender preset: the selected brush faces grow new brushes out along their normals, by a length the mouse
//! or a typed number sets. The old brushes never change.

use egui::{Color32, Key, Modifiers, PointerButton, Pos2, Rect, Response, Ui};
use td_core::{DVec3, NodeId};
use td_doc::map::Map;
use td_geom::Brush;

use crate::camera::{Camera, ViewKind};
use crate::i18n::{en, msg, trf};
use crate::state::EditorState;
use crate::tools::ToolKind;

#[derive(Default)]
pub struct FaceExtrude {
    modal: Option<Modal>,
}

struct Modal {
    faces: Vec<(NodeId, usize)>,
    dir: DVec3,
    anchor: DVec3,
    start: Option<Pos2>,
    view: Option<ViewKind>,
    numeric: String,
    dist: f64,
}

impl FaceExtrude {
    pub fn active(&self) -> bool {
        self.modal.is_some()
    }

    pub fn holds_transaction(&self) -> bool {
        self.modal.is_some()
    }

    /// Forgets the modal without touching the document.
    pub fn reset(&mut self) {
        self.modal = None;
    }

    pub fn start(&mut self, state: &mut EditorState) {
        if self.modal.is_some() || state.tool == ToolKind::Mesh || state.doc.in_transaction() {
            return;
        }

        let map = &state.doc.map;
        let picked: Vec<((NodeId, usize), DVec3, DVec3)> = state
            .doc
            .selection
            .faces
            .iter()
            .filter_map(|&(id, face)| {
                let brush = map.brush(id).filter(|b| face < b.faces.len())?;
                Some(((id, face), brush.faces[face].plane.normal, brush.face_center(face)))
            })
            .collect();
        let Some(&(_, first_normal, _)) = picked.first() else {
            state.fail_status(msg!("Select brush faces to extrude"));
            return;
        };

        let sum: DVec3 = picked.iter().map(|p| p.1).sum();
        let dir = if sum.length() < 1e-6 { first_normal } else { sum.normalize() };
        let anchor = picked.iter().map(|p| p.2).sum::<DVec3>() / picked.len() as f64;
        let faces = picked.iter().map(|p| p.0).collect();
        state.doc.begin(en!("Extrude"));
        self.modal = Some(Modal { faces, dir, anchor, start: None, view: None, numeric: String::new(), dist: 0.0 });
        state.set_status(msg!("Extrude {dist}", dist = format!("{:.3}", 0.0)));
    }

    /// Esc, Enter and the typed length. Every other key is swallowed so the preset's 1, 3, X or S cannot fire mid
    /// extrude. Returns true while the modal is open.
    pub fn keys(&mut self, ctx: &egui::Context, state: &mut EditorState) -> bool {
        let Some(modal) = &mut self.modal else { return false };
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.cancel(state);
            return true;
        }

        let before = modal.numeric.clone();
        crate::mesh_tool::type_number(ctx, &mut modal.numeric);
        if modal.numeric != before {
            let typed = modal.numeric.parse::<f64>().unwrap_or(0.0);
            self.apply(state, typed.max(0.0));
        }

        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            self.commit(state);
        }

        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { pressed: true, .. } | egui::Event::Text(_))));
        true
    }

    /// The mouse in a view. The modal stays in the first view the pointer moves in.
    pub fn input(&mut self, ui: &Ui, response: &Response, cam: &Camera, rect: Rect, state: &mut EditorState) {
        let Some(modal) = &mut self.modal else { return };
        let Some(pos) = response.hover_pos() else { return };
        if modal.view.is_some_and(|v| v != cam.kind) {
            return;
        }

        modal.view = Some(cam.kind);
        let start = *modal.start.get_or_insert(pos);
        if response.clicked_by(PointerButton::Secondary) {
            self.cancel(state);
            return;
        }

        let dist = if modal.numeric.is_empty() {
            let along = |p: Pos2| crate::tools::line_ray_param(modal.anchor, modal.dir, &cam.ray(rect, p));
            let raw = along(pos).zip(along(start)).map_or(0.0, |(a, b)| a - b);
            let snap = state.snap != ui.input(|i| i.modifiers.command);
            if snap { td_core::snap_to_grid(raw, state.grid) } else { raw }
        } else {
            modal.numeric.parse::<f64>().unwrap_or(0.0)
        }
        .max(0.0);
        // Rebuilding restores the whole map from the transaction, so a still mouse must not redo it every frame.
        if dist != modal.dist {
            self.apply(state, dist);
            ui.ctx().request_repaint();
        }

        if response.clicked_by(PointerButton::Primary) || response.drag_stopped_by(PointerButton::Primary) {
            self.commit(state);
        }
    }

    fn apply(&mut self, state: &mut EditorState, dist: f64) {
        let Some(modal) = &mut self.modal else { return };
        modal.dist = dist;
        state.doc.reset_transaction();
        if dist > 0.0 {
            let faces = modal.faces.clone();
            state.doc.edit(en!("Extrude"), |m, s| {
                let far = extrude_faces(m, &faces, dist);
                if !far.is_empty() {
                    s.clear();
                    s.faces.extend(far);
                }
            });
        }

        let shown = if modal.numeric.is_empty() { format!("{dist:.3}") } else { modal.numeric.clone() };
        state.set_status(msg!("Extrude {dist}", dist = shown));
    }

    /// A length of zero makes nothing, so it leaves no undo step.
    fn commit(&mut self, state: &mut EditorState) {
        let Some(modal) = self.modal.take() else { return };
        if modal.dist > 0.0 {
            state.doc.commit();
        } else {
            state.doc.cancel();
        }
    }

    pub fn cancel(&mut self, state: &mut EditorState) {
        if self.modal.take().is_some() {
            state.doc.cancel();
            state.set_status(msg!("Cancelled"));
        }
    }

    pub fn paint_overlay(&self, painter: &egui::Painter, rect: Rect) {
        if self.modal.is_some() {
            crate::widgets::corner_hint(
                painter,
                rect,
                trf!("Extrude: move the mouse or type a length, click or Enter confirms, Esc or right click cancels"),
                Color32::from_rgb(255, 200, 120),
            );
        }
    }
}

/// Builds a new brush on each face, `dist` out along that face's own normal, next to its source brush. Returns the
/// far face of each new brush.
pub(crate) fn extrude_faces(map: &mut Map, faces: &[(NodeId, usize)], dist: f64) -> Vec<(NodeId, usize)> {
    if dist.is_nan() || dist <= 0.0 {
        return Vec::new();
    }

    let mut far = Vec::new();
    for &(id, face) in faces {
        let Some(brush) = map.brush(id).filter(|b| face < b.faces.len()) else { continue };
        let normal = brush.faces[face].plane.normal;
        let Ok(nb) = td_geom::csg::extrude_face(brush, face, dist) else { continue };
        let parent = map.get(id).and_then(|n| n.parent).unwrap_or(map.default_layer());
        let far_face = far_face(&nb, normal);
        far.push((map.insert(parent, td_doc::NodeKind::Brush(nb)), far_face));
    }

    far
}

/// The sides of an extruded prism run along `normal` and its near cap faces back, so only the far cap points along it.
fn far_face(brush: &Brush, normal: DVec3) -> usize {
    (0..brush.faces.len()).max_by(|&a, &b| brush.faces[a].plane.normal.dot(normal).total_cmp(&brush.faces[b].plane.normal.dot(normal))).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use td_core::Aabb;

    const BOX: (DVec3, DVec3) = (DVec3::new(-56.0, 8.0, -24.0), DVec3::new(40.0, 72.0, 88.0));

    /// The box above in the default layer and wedge W, min (-40, 8, 24) max (24, 72, 152), in a group. W's slanted side
    /// lies on 2y + z = 168.
    fn box_and_wedge() -> (EditorState, NodeId, NodeId, NodeId) {
        let mut state = EditorState::new(Default::default());
        let map = &mut state.doc.map;
        let layer = map.default_layer();
        let group = map.insert(layer, td_doc::NodeKind::Group(td_doc::Group::new("g")));
        let b = map.insert(layer, td_doc::NodeKind::Brush(Brush::from_aabb(&Aabb::new(BOX.0, BOX.1), "dev/grey").unwrap()));
        let wedge = td_geom::shapes::wedge(&Aabb::new(DVec3::new(-40.0, 8.0, 24.0), DVec3::new(24.0, 72.0, 152.0)), "dev/grey").unwrap();
        let w = map.insert(group, td_doc::NodeKind::Brush(wedge));
        (state, b, w, group)
    }

    fn face_along(state: &EditorState, id: NodeId, normal: DVec3) -> usize {
        state.doc.map.brush(id).unwrap().faces.iter().position(|f| (f.plane.normal - normal.normalize()).length() < 1e-9).unwrap()
    }

    fn slanted() -> DVec3 {
        DVec3::new(0.0, 2.0, 1.0).normalize()
    }

    fn brushes(state: &EditorState) -> Vec<(NodeId, Brush)> {
        state.doc.map.brushes().map(|(id, b)| (id, b.clone())).collect()
    }

    fn sorted(points: impl IntoIterator<Item = DVec3>) -> Vec<[i64; 3]> {
        let mut out: Vec<[i64; 3]> = points.into_iter().map(|p| (p * 1000.0).round().as_i64vec3().to_array()).collect();
        out.sort();
        out
    }

    fn select_faces(state: &mut EditorState, faces: &[(NodeId, usize)]) {
        state.doc.edit("select", |_, s| {
            s.clear();
            s.faces.extend(faces.iter().copied());
        });
    }

    fn keys(extrude: &mut FaceExtrude, state: &mut EditorState, events: Vec<egui::Event>) -> (bool, egui::Context) {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput { events, ..Default::default() });
        let taken = extrude.keys(&ctx, state);
        (taken, ctx)
    }

    fn key(key: Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    fn text(t: &str) -> egui::Event {
        egui::Event::Text(t.into())
    }

    #[test]
    fn extrude_faces_grows_a_new_brush_per_face() {
        let (mut state, b, w, group) = box_and_wedge();
        let top = face_along(&state, b, DVec3::Y);
        let side = face_along(&state, w, slanted());
        let before = brushes(&state);
        let wedge_side = state.doc.map.brush(w).unwrap().face_points(side);
        let side_dist = state.doc.map.brush(w).unwrap().faces[side].plane.dist;

        let far = extrude_faces(&mut state.doc.map, &[(b, top), (w, side)], 32.0);
        let map = &state.doc.map;
        assert_eq!(far.len(), 2);
        assert_eq!(map.brushes().count(), 4);
        for (id, brush) in &before {
            assert_eq!(map.brush(*id), Some(brush), "the source brushes stay as they were");
        }

        let (on_box, on_wedge) = (far[0], far[1]);
        assert_eq!(map.get(on_box.0).unwrap().parent, Some(map.default_layer()));
        assert_eq!(map.get(on_wedge.0).unwrap().parent, Some(group), "the new brush sits next to its source in the group");
        assert_eq!(map.brush(on_box.0).unwrap().bounds(), Aabb::new(DVec3::new(-56.0, 72.0, -24.0), DVec3::new(40.0, 104.0, 88.0)));
        let plane = map.brush(on_box.0).unwrap().faces[on_box.1].plane;
        assert!((plane.normal - DVec3::Y).length() < 1e-9 && (plane.dist - 104.0).abs() < 1e-9, "{plane:?}");

        let prism = map.brush(on_wedge.0).unwrap();
        let expected = wedge_side.iter().copied().chain(wedge_side.iter().map(|p| *p + slanted() * 32.0));
        assert_eq!(sorted(prism.vertices.iter().copied()), sorted(expected));
        let plane = prism.faces[on_wedge.1].plane;
        assert!((plane.normal - slanted()).length() < 1e-9 && (plane.dist - (side_dist + 32.0)).abs() < 1e-6, "{plane:?}");

        for dist in [0.0, -16.0, f64::NAN] {
            assert!(extrude_faces(&mut state.doc.map, &[(b, top), (w, side)], dist).is_empty(), "{dist}");
            assert_eq!(state.doc.map.brushes().count(), 4, "{dist} makes nothing");
        }

        assert!(extrude_faces(&mut state.doc.map, &[], 32.0).is_empty());
        assert!(extrude_faces(&mut state.doc.map, &[(b, 99), (group, 0)], 32.0).is_empty(), "a face past the end and a group are skipped");
        assert_eq!(state.doc.map.brushes().count(), 4);
    }

    #[test]
    fn the_far_face_points_the_way_of_the_source() {
        let (state, b, w, _) = box_and_wedge();
        for (id, normal) in [(b, DVec3::Y), (b, DVec3::NEG_X), (w, slanted()), (w, DVec3::NEG_Z)] {
            let face = face_along(&state, id, normal);
            let prism = td_geom::csg::extrude_face(state.doc.map.brush(id).unwrap(), face, 16.0).unwrap();
            let far = far_face(&prism, normal.normalize());
            let plane = prism.faces[far].plane;
            assert!((plane.normal - normal.normalize()).length() < 1e-9, "{normal}: {plane:?}");
            let source = state.doc.map.brush(id).unwrap().faces[face].plane.dist;
            assert!((plane.dist - (source + 16.0)).abs() < 1e-6, "{normal}: the far cap, not the near one");
        }
    }

    #[test]
    fn typed_numbers_take_digits_a_dot_and_backspace() {
        let typed = |events: Vec<egui::Event>| {
            let ctx = egui::Context::default();
            ctx.begin_pass(egui::RawInput { events, ..Default::default() });
            let mut numeric = String::new();
            crate::mesh_tool::type_number(&ctx, &mut numeric);
            numeric
        };
        assert_eq!(typed(vec![text("3"), text("a"), text("2"), text("."), text("5"), key(Key::Backspace)]), "32.");
        assert_eq!(typed(vec![text("-1x6")]), "-16");
        assert_eq!(typed(vec![key(Key::Backspace)]), "");
        assert_eq!(typed(vec![]), "");
    }

    #[test]
    fn escape_cancels_enter_commits_and_zero_leaves_nothing() {
        let (mut state, b, _, _) = box_and_wedge();
        let top = face_along(&state, b, DVec3::Y);
        select_faces(&mut state, &[(b, top)]);
        let before = brushes(&state);
        let undo = |state: &EditorState| state.doc.history.undo_labels().count();
        let undo_before = undo(&state);
        let mut extrude = FaceExtrude::default();

        extrude.start(&mut state);
        assert!(extrude.active() && state.doc.in_transaction());
        assert_eq!(state.status, "Extrude 0.000");
        extrude.apply(&mut state, 32.0);
        assert_eq!(state.doc.map.brushes().count(), 3);
        assert_eq!(state.status, "Extrude 32.000");
        let (taken, _) = keys(&mut extrude, &mut state, vec![key(Key::Escape)]);
        assert!(taken && !extrude.active() && !state.doc.in_transaction());
        assert_eq!(brushes(&state), before);
        assert_eq!(state.doc.selection.faces, std::collections::BTreeSet::from([(b, top)]));
        assert_eq!(undo(&state), undo_before, "Esc leaves no undo step");
        assert_eq!(state.status, "Cancelled");

        extrude.start(&mut state);
        let (taken, ctx) = keys(&mut extrude, &mut state, vec![text("1"), text("6"), key(Key::Num3), key(Key::X)]);
        assert!(taken && extrude.active());
        assert!(ctx.input(|i| i.events.is_empty()), "Blender's 3 and X do not fire mid extrude");
        assert_eq!(state.status, "Extrude 16");
        keys(&mut extrude, &mut state, vec![key(Key::Enter)]);
        assert!(!extrude.active() && !state.doc.in_transaction());
        assert_eq!(undo(&state), undo_before + 1);
        assert_eq!(state.doc.history.undo_labels().next(), Some("Extrude"));
        let new: Vec<(NodeId, Brush)> = brushes(&state).into_iter().filter(|b| !before.contains(b)).collect();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].1.bounds(), Aabb::new(DVec3::new(-56.0, 72.0, -24.0), DVec3::new(40.0, 88.0, 88.0)));
        assert_eq!(
            state.doc.selection.faces,
            std::collections::BTreeSet::from([(new[0].0, far_face(&new[0].1, DVec3::Y))]),
            "the far face is selected for the next E"
        );
        assert!(state.doc.selection.nodes.is_empty());

        let after = brushes(&state);
        let typed_then_erased = vec![vec![text("2")], vec![key(Key::Backspace)]];
        for typed in [vec![], vec![vec![text("-16")]], vec![vec![text("0")]], typed_then_erased] {
            extrude.start(&mut state);
            for frame in &typed {
                keys(&mut extrude, &mut state, frame.clone());
            }

            keys(&mut extrude, &mut state, vec![key(Key::Enter)]);
            assert!(!extrude.active() && !state.doc.in_transaction(), "{typed:?}");
            assert_eq!(brushes(&state), after, "{typed:?} makes nothing");
            assert_eq!(undo(&state), undo_before + 1, "{typed:?} leaves no undo step");
        }
    }

    #[test]
    fn extrude_refuses_without_brush_faces_in_the_mesh_tool_and_inside_a_transaction() {
        let (mut state, b, _, _) = box_and_wedge();
        let mut extrude = FaceExtrude::default();
        state.doc.edit("select", |_, s| s.select_node(b));
        extrude.start(&mut state);
        assert!(!extrude.active() && !state.doc.in_transaction());
        assert_eq!(state.status, "Select brush faces to extrude");
        assert!(!keys(&mut extrude, &mut state, vec![key(Key::Escape)]).0, "a closed extrude takes no keys");

        select_faces(&mut state, &[(b, 0)]);
        state.tool = ToolKind::Mesh;
        extrude.start(&mut state);
        assert!(!extrude.active(), "the Mesh tool owns E");

        state.tool = ToolKind::Select;
        state.doc.begin("drag");
        extrude.start(&mut state);
        assert!(!extrude.active(), "another edit holds the transaction");
        state.doc.cancel();
        extrude.start(&mut state);
        assert!(extrude.active());
    }
}
