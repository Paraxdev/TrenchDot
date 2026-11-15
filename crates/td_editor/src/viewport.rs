use egui::{Align2, Color32, CursorIcon, FontId, Key, PointerButton, Pos2, Rect, Response, Sense, Ui, Vec2};
use td_core::{Aabb, DVec3, NodeId, Plane, Ray};
use td_doc::ops;
use td_geom::Brush;
use td_render::{Frame, FrameParams, LineVertex, Renderer, ViewTarget};

use crate::camera::{Camera, ViewKind};
use crate::commands::Action;
use crate::i18n::{en, msg, msg_n, tr, trf};
use crate::mouse_map::{
    Button, CameraDrag, Chord, ClickHit, ClickRule, DoubleClick, DoubleClickHit, DragHit, EmptyDrag, SelectionDrag, click_rule, double_click, empty_drag,
    grabs_slanted_edges, selection_drag, shift_counts_on_click, shift_locks_axis, space_camera,
};
use crate::panels::DndPayload;
use crate::picking::{self, Hit};
use crate::scene::{SceneCache, v3};
use crate::selection_box::SelectionBox;
use crate::state::{EditorState, SelectMode};
use crate::tools::{Drag, ToolKind, line_ray_param};
use crate::toolset::ToolSet;

pub struct ViewCtx<'a> {
    pub state: &'a mut EditorState,
    pub renderer: &'a mut Renderer,
    pub scene: &'a SceneCache,
    pub actions: &'a mut Vec<Action>,
    pub tools: &'a mut ToolSet,
}

pub struct Viewport {
    pub camera: Camera,
    target: Option<ViewTarget>,
    drag: Option<Drag>,
    pub rect: Rect,
    pub hovered: bool,
    /// The id of the view's response, so the app can tell a drag in a view from a drag on a panel widget.
    pub id: egui::Id,
    /// A menu or popup was open when the frame started, see `App::ui`.
    pub(crate) popup_open: bool,
    press_modifiers: egui::Modifiers,
    /// Where and when (egui time) the last plain click in a 2D view landed, a quick click at the same spot cycles.
    last_click: Option<(Pos2, f64)>,
    /// Whether the last click landed on the transform gizmo. A double click it starts only acts on the selection under
    /// the gizmo. The second click is not checked, since the first one may have just put the gizmo under the pointer.
    gizmo_click: bool,
    /// The outer edge of the selected brushes a drag would resize, highlighted before the drag starts.
    hover_edge: Option<[Pos2; 2]>,
    pub(crate) pixels_per_point: f32,
    /// The 3D camera was looked around with this session, so the hint on how to do it is no longer shown.
    looked: bool,
    /// Moving the mouse turns this 3D view with no button held, see `App::drive_mouse_look`.
    pub(crate) mouse_look: bool,
    /// The right button went down on this 3D view and is still held, even before the mouse moves far enough to look.
    right_held: bool,
    /// The fly keys moved the camera during this right button hold, so its release opens no context menu.
    flew: bool,
}

// Linear values, the targets are sRGB.
const BG_LIT: [f64; 4] = [0.18, 0.28, 0.45, 1.0];
/// Depth of the first brush drawn in a 2D view, 4 m. A box drawn in the Top view and hollowed with 16 unit walls
/// leaves 3 m of headroom, enough for a player.
pub(crate) const FIRST_BRUSH_DEPTH: f64 = 128.0;

impl Viewport {
    pub fn new(kind: ViewKind) -> Self {
        Self {
            camera: Camera::new(kind),
            target: None,
            drag: None,
            rect: Rect::NOTHING,
            hovered: false,
            id: egui::Id::NULL,
            popup_open: false,
            press_modifiers: Default::default(),
            last_click: None,
            gizmo_click: false,
            hover_edge: None,
            pixels_per_point: 1.0,
            looked: false,
            mouse_look: false,
            right_held: false,
            flew: false,
        }
    }

    pub fn kind(&self) -> ViewKind {
        self.camera.kind
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut ViewCtx) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        self.rect = rect;
        self.hovered = response.hovered();
        self.id = response.id;
        if rect.width() < 2.0 || rect.height() < 2.0 {
            return;
        }

        let ppp = ui.ctx().pixels_per_point();
        self.pixels_per_point = ppp;
        cx.renderer.ensure_target(&mut self.target, [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32]);

        cx.tools.settle(cx.state);
        if cx.state.tool != ToolKind::Select {
            cx.state.drop_selection_box();
        }

        if self.drag.is_some() && !self.is_camera_drag() {
            // A tool picked mid drag (a shortcut, the toolbar) stops the select handling below, so close its edit here.
            // A drag whose transaction was closed elsewhere (undo, a tab switch) must not go on editing outside it.
            // The Shape Generator's box and the selection box have none, the drag ends with its box.
            let switched = !draws_like_select(cx.state.tool);
            let orphaned = match self.drag {
                Some(Drag::ShapeBounds { .. }) => cx.state.shape_bounds.is_none(),
                Some(Drag::SelectBox) => cx.state.selection_box.is_none(),
                Some(Drag::ScreenBox { .. }) => false,
                _ => !cx.state.doc.in_transaction(),
            };
            if switched || orphaned {
                self.drag = None;
                cx.state.drag_preview = None;
                if switched {
                    cx.state.doc.commit();
                }
            }
        }

        self.handle_camera(ui, &response, cx);

        self.handle_keys(ui, cx);
        self.hover_edge = None;
        // While a menu is open a click in the view only closes it, like in most programs.
        let closing_menu = self.popup_open && self.drag.is_none();
        let passive = closing_menu;
        let extruding = cx.tools.extrude.active();
        if !passive && extruding && !self.is_camera_drag() {
            cx.tools.extrude.input(ui, &response, &self.camera, rect, cx.state);
        } else if !passive && (draws_like_select(cx.state.tool) || self.is_camera_drag()) {
            self.handle_select_tool(ui, &response, cx);
        } else if !passive {
            let hover = response.hover_pos();
            cx.tools.viewport_input(ui, &response, &self.camera, rect, hover, cx.state);
        }

        if self.hovered && self.space_drag(ui, cx.state).is_some() {
            ui.ctx().set_cursor_icon(if self.is_camera_drag() { CursorIcon::Grabbing } else { CursorIcon::Grab });
        }

        self.handle_drop(ui, &response, cx);
        self.update_cursor_world(&response, cx);

        let tool_lines = cx.tools.lines(&self.camera, rect, cx.state);
        self.render(cx, &tool_lines);
        if let Some(target) = &self.target {
            ui.painter().image(target.texture_id, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }

        let layer = ui.layer_id();
        let overlay_start = ui.ctx().graphics_mut(|g| g.entry(layer).next_idx());
        self.paint_overlay(ui, cx);
        if let Some(edge) = self.hover_edge {
            ui.painter_at(rect).line_segment(edge, egui::Stroke::new(3.0, crate::theme::VIEW.yellow));
        }

        cx.tools.paint_overlay(ui, &self.camera, rect, cx.state);
        if let Some(pivot) = crate::theme::colors().flip_pivot().filter(|_| self.camera.kind.is_2d()) {
            flip_shapes(ui.ctx(), layer, overlay_start, pivot);
        }

        // Right click applies materials in the texture tool. The right click that cancels an extrude opens no menu.
        if cx.state.tool != ToolKind::Texture && !extruding && !self.flew {
            response.context_menu(|ui| context_menu(ui, cx.state, cx.actions));
        }
    }

    /// Drops a Select tool drag (move, resize, draw) and puts back what it had changed. Camera drags and the Shape
    /// Generator's box, which is not part of the map, are left alone.
    pub fn cancel_drag(&mut self, state: &mut EditorState) -> bool {
        if self.drag.is_none() || self.is_camera_drag() || matches!(self.drag, Some(Drag::ShapeBounds { .. })) {
            return false;
        }

        if matches!(self.drag, Some(Drag::SelectBox)) {
            self.drag = None;
            state.drop_selection_box();
            return true;
        }

        if matches!(self.drag, Some(Drag::ScreenBox { .. })) {
            self.drag = None;
            return true;
        }

        self.drag = None;
        state.drag_preview = None;
        state.doc.cancel();
        true
    }

    fn is_camera_drag(&self) -> bool {
        matches!(self.drag, Some(Drag::Look | Drag::Pan | Drag::Orbit { .. }))
    }

    /// The right mouse look in the 3D view, or mouse look. While it runs WASD, Q and E fly the camera.
    pub fn is_flying(&self) -> bool {
        matches!(self.drag, Some(Drag::Look)) || self.mouse_look || self.right_held
    }

    /// Holding the right button flies at once. Waiting for the mouse to move first let WASD and Shift+D reach the
    /// tool shortcuts, so a strafe could select all or duplicate the selection.
    fn track_right_button(&mut self, ui: &Ui, response: &Response) {
        let (pressed, down) = ui.input(|i| (i.pointer.button_pressed(PointerButton::Secondary), i.pointer.secondary_down()));
        if pressed && response.contains_pointer() && !self.camera.kind.is_2d() {
            self.right_held = true;
            self.flew = false;
        }

        if !down {
            self.right_held = false;
        }
    }

    pub(crate) fn start_mouse_look(&mut self) {
        self.mouse_look = true;
        self.looked = true;
    }

    /// What a left drag does now that Space is held, Hammer's temporary camera. Off while a text field has the
    /// keyboard, and when plain Space is bound to an action so that binding keeps working.
    fn space_drag(&self, ui: &Ui, state: &EditorState) -> Option<CameraDrag> {
        let drag = space_camera(&state.prefs.keymap_preset, self.camera.kind.is_2d())?;
        let held = ui.input(|i| i.key_down(Key::Space)) && !ui.ctx().egui_wants_keyboard_input();
        (held && !crate::commands::plain_space_bound(&state.prefs)).then_some(drag)
    }

    fn handle_camera(&mut self, ui: &Ui, response: &Response, cx: &mut ViewCtx) {
        let rect = self.rect;
        self.track_right_button(ui, response);
        let space = response.drag_started_by(PointerButton::Primary).then(|| self.space_drag(ui, cx.state)).flatten();
        let (delta, scroll, modifiers, dt, press_origin) =
            ui.input(|i| (i.pointer.delta(), i.smooth_scroll_delta.y, i.modifiers, i.stable_dt as f64, i.pointer.press_origin()));
        let scroll = if cx.tools.mesh.takes_wheel() && cx.state.tool == ToolKind::Mesh { 0.0 } else { scroll };
        let (invert_y, look_sensitivity, fly_speed) = (cx.state.prefs.invert_y, cx.state.prefs.look_sensitivity, cx.state.prefs.fly_speed);
        match self.camera.kind {
            ViewKind::Perspective => {
                let pivot = |view: &Self| {
                    press_origin
                        .and_then(|p| picking::pick(cx.state, &view.camera.ray(rect, p)))
                        .map(|h| h.point)
                        .unwrap_or(view.camera.position + view.camera.forward() * 256.0)
                };
                if response.drag_started_by(PointerButton::Secondary) && modifiers.alt {
                    self.drag = Some(Drag::Orbit { pivot: pivot(self) });
                    self.looked = true;
                } else if response.drag_started_by(PointerButton::Secondary) || space == Some(CameraDrag::Look) {
                    self.drag = Some(Drag::Look);
                    self.looked = true;
                } else if response.drag_started_by(PointerButton::Middle) {
                    self.drag = Some(Drag::Pan);
                } else if response.drag_started_by(PointerButton::Primary) && modifiers.alt && self.alt_drag_orbits(press_origin, cx) {
                    self.drag = Some(Drag::Orbit { pivot: pivot(self) });
                }

                let invert = if invert_y { -1.0 } else { 1.0 };
                if self.mouse_look {
                    // Raw device motion, which keeps coming while the hidden cursor is held in place.
                    let moved = ui.input(|i| i.events.iter().fold(Vec2::ZERO, |sum, e| if let egui::Event::MouseMoved(d) = e { sum + *d } else { sum }));
                    if moved != Vec2::ZERO {
                        self.camera.rotate(Vec2::new(moved.x, moved.y * invert) / self.pixels_per_point, look_sensitivity);
                    }
                }

                match self.drag {
                    Some(Drag::Look) => self.camera.rotate(Vec2::new(delta.x, delta.y * invert), look_sensitivity),
                    Some(Drag::Pan) => self.camera.pan(delta * 1.5, rect),
                    Some(Drag::Orbit { pivot }) => self.camera.orbit(pivot, delta, look_sensitivity),
                    _ => {}
                }

                let brush_tool = matches!(cx.state.tool, ToolKind::Scatter | ToolKind::Blend | ToolKind::Sculpt | ToolKind::Paint);
                // egui turns Ctrl+wheel into zoom and leaves the smoothed scroll at zero, so read the raw notches.
                let resize = if self.hovered && brush_tool { ui.input(|i| wheel_notches(i, |m| m.command)) } else { 0.0 };
                if resize != 0.0 {
                    let factor = 1.08f64.powf(resize as f64);
                    match cx.state.tool {
                        ToolKind::Scatter => cx.state.prefs.scatter.radius = (cx.state.prefs.scatter.radius * factor).clamp(8.0, 16384.0),
                        ToolKind::Blend => cx.state.blend.radius = (cx.state.blend.radius * factor).clamp(2.0, 16384.0),
                        _ => cx.state.sculpt.radius = (cx.state.sculpt.radius * factor).clamp(2.0, 16384.0),
                    }
                } else if self.hovered && scroll != 0.0 && !(brush_tool && modifiers.command) && !(cx.state.tool == ToolKind::Texture && modifiers.alt) {
                    // Near geometry a notch moves a fixed step, from far away it covers a fifth of the distance to
                    // the surface straight ahead, so it closes in quickly without flying through it. With nothing
                    // ahead, the ground under the pointer sets the pace.
                    let step = fly_speed * 0.004;
                    let hit = picking::pick(cx.state, &Ray::new(self.camera.position, self.camera.forward()))
                        .or_else(|| response.hover_pos().and_then(|p| picking::pick(cx.state, &self.camera.ray(rect, p))));
                    let step = hit.map_or(step, |h| step.max(h.distance * WHEEL_DOLLY / NOTCH_POINTS));
                    self.camera.position += self.camera.forward() * (scroll as f64) * step;
                }

                // Like TrenchBroom, the fly keys only steer while the right mouse look is held, otherwise they are tool shortcuts.
                if self.is_flying() && !ui.ctx().egui_wants_keyboard_input() && !modifiers.command {
                    let (up, down) = crate::mouse_map::fly_keys(&cx.state.prefs.keymap_preset);
                    let mut dir = DVec3::ZERO;
                    ui.input(|i| {
                        if i.key_down(Key::W) {
                            dir += self.camera.forward();
                        }

                        if i.key_down(Key::S) {
                            dir -= self.camera.forward();
                        }

                        if i.key_down(Key::D) {
                            dir += self.camera.right();
                        }

                        if i.key_down(Key::A) {
                            dir -= self.camera.right();
                        }

                        if i.key_down(up) {
                            dir += DVec3::Y;
                        }

                        if i.key_down(down) {
                            dir -= DVec3::Y;
                        }
                    });
                    if dir != DVec3::ZERO {
                        let boost = if modifiers.shift { 3.0 } else { 1.0 };
                        self.camera.position += dir.normalize() * fly_speed * boost * dt.min(0.1);
                        self.flew = true;
                        ui.ctx().request_repaint();
                    }
                }
            }
            _ => {
                if response.drag_started_by(PointerButton::Secondary) || response.drag_started_by(PointerButton::Middle) || space == Some(CameraDrag::Pan) {
                    self.drag = Some(Drag::Pan);
                }

                if let Some(Drag::Pan) = self.drag {
                    self.camera.pan(delta, rect);
                }

                if self.hovered
                    && scroll != 0.0
                    && let Some(pos) = response.hover_pos()
                {
                    self.camera.zoom_at(rect, pos, WHEEL_ZOOM.powf(scroll as f64 / NOTCH_POINTS));
                }
            }
        }

        if self.is_camera_drag() && (response.drag_stopped() || !ui.input(|i| i.pointer.any_down())) {
            self.drag = None;
        }
    }

    /// Alt+drag in the 3D view orbits. Only a drag in the Select tool that starts on the selection or one of its gizmos
    /// moves the selection vertically instead, and other tools keep Alt+drag for themselves while something is selected.
    fn alt_drag_orbits(&self, origin: Option<Pos2>, cx: &ViewCtx) -> bool {
        if cx.state.doc.selection.is_empty() || cx.state.tool == ToolKind::Block {
            return true;
        }

        if cx.state.tool != ToolKind::Select {
            return false;
        }

        origin.is_none_or(|p| !self.on_selection(p, cx))
    }

    fn on_selection(&self, pos: Pos2, cx: &ViewCtx) -> bool {
        let state = &*cx.state;
        if crate::gizmos::handle_at(state, &self.camera, self.rect, pos).is_some() || crate::transform_gizmo::hit(state, &self.camera, self.rect, pos).is_some()
        {
            return true;
        }

        self.pick(cx, pos).is_some_and(|h| is_selected(state, h.node))
    }

    fn handle_keys(&mut self, ui: &Ui, cx: &mut ViewCtx) {
        if !self.hovered || ui.ctx().egui_wants_keyboard_input() || !draws_like_select(cx.state.tool) {
            return;
        }

        let grid = cx.state.grid;
        let (right, up, ahead) = match self.camera.kind {
            ViewKind::Perspective => {
                let f = self.camera.forward();
                let flat = DVec3::new(f.x, 0.0, f.z);
                let major = |v: DVec3| {
                    let mut out = DVec3::ZERO;
                    let i = if v.x.abs() > v.z.abs() { 0 } else { 2 };
                    out[i] = v[i].signum();
                    out
                };
                (major(self.camera.right()), major(flat), major(flat))
            }
            k => k.axes(),
        };
        if ui.input(|i| i.modifiers.alt && !i.modifiers.command && !i.modifiers.shift) {
            Self::rotate_keys(ui, cx, right, ahead);
            return;
        }

        ui.input(|i| {
            let mut offset = DVec3::ZERO;
            if i.key_pressed(Key::ArrowLeft) {
                offset -= right;
            }

            if i.key_pressed(Key::ArrowRight) {
                offset += right;
            }

            if i.key_pressed(Key::ArrowUp) {
                offset += up;
            }

            if i.key_pressed(Key::ArrowDown) {
                offset -= up;
            }

            if i.key_pressed(Key::PageUp) {
                offset += if self.camera.kind == ViewKind::Perspective { DVec3::Y } else { -self.camera.forward() };
            }

            if i.key_pressed(Key::PageDown) {
                offset -= if self.camera.kind == ViewKind::Perspective { DVec3::Y } else { -self.camera.forward() };
            }

            if offset != DVec3::ZERO {
                cx.actions.push(Action::Nudge(offset * grid));
            }
        });
    }

    /// TrenchBroom's Alt+arrows turn the selection a quarter turn, left and right about the vertical, up and down
    /// about the view direction, and Page Up and Page Down about the view's right.
    fn rotate_keys(ui: &Ui, cx: &mut ViewCtx, right: DVec3, ahead: DVec3) {
        ui.input(|i| {
            for key in [Key::ArrowLeft, Key::ArrowRight, Key::ArrowUp, Key::ArrowDown, Key::PageUp, Key::PageDown] {
                if i.key_pressed(key)
                    && let Some(action) = quarter_turn(key, right, ahead)
                {
                    cx.actions.push(action);
                }
            }
        });
    }

    fn pick(&self, cx: &ViewCtx, pos: Pos2) -> Option<Hit> {
        picking::pick(cx.state, &self.camera.ray(self.rect, pos))
    }

    /// The objects a click at `pos` can select, nearest first, one hit each. A 2D view sees through all of them.
    fn click_targets(&self, cx: &ViewCtx, pos: Pos2) -> Vec<(NodeId, Hit)> {
        let map = &cx.state.doc.map;
        let mut out: Vec<(NodeId, Hit)> = Vec::new();
        for h in picking::pick_all(cx.state, &self.camera.ray(self.rect, pos)) {
            let target = map.click_target(h.node, &cx.state.open_groups);
            if !out.iter().any(|(t, _)| *t == target) {
                out.push((target, h));
            }
        }

        out
    }

    /// What a double click acts on: the object its first click selected, which in a 2D view may lie under another.
    fn double_click_hit(&self, cx: &ViewCtx, pos: Pos2) -> Option<Hit> {
        let targets = self.click_targets(cx, pos);
        let selection = &cx.state.doc.selection.nodes;
        let chosen = targets.iter().find(|(t, _)| selection.len() == 1 && selection.contains(t)).or(targets.first());
        chosen.map(|(_, h)| *h)
    }

    fn update_cursor_world(&self, response: &Response, cx: &mut ViewCtx) {
        let Some(pos) = response.hover_pos() else { return };
        let world = match self.camera.kind {
            ViewKind::Perspective => {
                let ray = self.camera.ray(self.rect, pos);
                match picking::pick(cx.state, &ray) {
                    Some(h) => h.point + h.normal * 0.01,
                    None => ray.intersect_plane(&Plane::new(DVec3::Y, 0.0)).filter(|t| *t > 0.0 && *t < 8192.0).map(|t| ray.at(t)).unwrap_or(ray.at(256.0)),
                }
            }
            _ => {
                let mut p = self.camera.screen_to_plane(self.rect, pos);
                let axis = self.camera.kind.depth_axis();
                let lb = cx.state.last_bounds;
                p[axis] = if lb.is_empty() { 0.0 } else { lb.min[axis] };
                p
            }
        };
        cx.state.cursor_world = Some(world);
    }

    fn handle_select_tool(&mut self, ui: &Ui, response: &Response, cx: &mut ViewCtx) {
        let rect = self.rect;
        let (modifiers, press_origin, pointer) = ui.input(|i| (i.modifiers, i.pointer.press_origin(), i.pointer.interact_pos()));
        let double_clicked = crate::widgets::double_clicked(response);
        let on_gizmo = response.interact_pointer_pos().is_some_and(|pos| crate::transform_gizmo::hit(cx.state, &self.camera, rect, pos).is_some());
        let pair_on_gizmo = response.clicked() && std::mem::replace(&mut self.gizmo_click, on_gizmo);
        let box_click = response.clicked() && self.click_on_selection_box(response, cx.state);

        if response.clicked()
            && !on_gizmo
            && !box_click
            && let Some(pos) = response.interact_pointer_pos()
        {
            let mut hit = self.pick(cx, pos);
            let plain = !modifiers.command && !(modifiers.shift && shift_counts_on_click(&cx.state.prefs.keymap_preset));
            let now = ui.input(|i| i.time);
            let mut cycle_hint = None;
            if self.camera.kind.is_2d() && plain {
                // Clicking again soon at the same spot, or Alt+click, steps through everything under the cursor, so a
                // wall under its ceiling in the Top view can be picked.
                let again = self.last_click.is_some_and(|(p, t)| p.distance(pos) < 4.0 && now - t < CLICK_CYCLE_SECONDS);
                let targets = self.click_targets(cx, pos);
                let current = targets.iter().position(|(t, _)| cx.state.doc.selection.nodes.len() == 1 && cx.state.doc.selection.nodes.contains(t));
                if double_clicked {
                    // The second click of a double click keeps what the first one picked for the double click below.
                    if let Some(i) = current {
                        hit = Some(targets[i].1);
                    }
                } else if (again || modifiers.alt) && targets.len() > 1 {
                    let next = current.map_or(0, |i| (i + 1) % targets.len());
                    hit = Some(targets[next].1);
                    cycle_hint = Some(msg!("Object {n} of {total} under the cursor, click again for the next", n = next + 1, total = targets.len()));
                } else if targets.len() > 1 {
                    cycle_hint = Some(msg_n!(
                        targets.len(),
                        "{n} object under the cursor, click again or Alt+click to select the next",
                        "{n} objects under the cursor, click again or Alt+click to select the next"
                    ));
                }
            }

            self.last_click = (self.camera.kind.is_2d() && plain).then_some((pos, now));
            let click_hit = match hit {
                None => ClickHit::None,
                Some(h) if h.face.is_some() => ClickHit::Face,
                Some(_) => ClickHit::Object,
            };
            let rule = click_rule(&cx.state.prefs.keymap_preset, cx.state.select_mode, modifiers.command, modifiers.shift, click_hit);
            cx.state.drop_group_status();
            match hit {
                Some(_) if rule == ClickRule::FaceModeMiss => cx.state.set_status(msg!("Face mode picks faces, press 1 for objects")),
                Some(h) => match rule {
                    ClickRule::ReplaceFace | ClickRule::ToggleFace => {
                        let (id, face) = (h.node, h.face.unwrap());
                        cx.state.leave_groups_outside(id);
                        cx.state.doc.select(|_, s| {
                            if rule == ClickRule::ToggleFace {
                                s.toggle_face(id, face);
                            } else {
                                s.clear();
                                s.select_face(id, face);
                            }
                        });
                    }
                    _ => {
                        if cx.state.leave_groups_outside(h.node) && rule == ClickRule::ToggleObject {
                            cx.state.extend_status(", ", msg!("what was selected inside left the selection"));
                        }

                        let map = &cx.state.doc.map;
                        let target = map.click_target(h.node, &cx.state.open_groups);
                        cx.state.last_bounds = map.bounds(target);
                        cx.state.outliner_reveal = Some(target);
                        let deselects = rule == ClickRule::ToggleObject && cx.state.doc.selection.nodes.contains(&target);
                        if !deselects {
                            cx.state.report_group_selected(target);
                        }

                        cx.state.doc.select(|_, s| {
                            if rule == ClickRule::ToggleObject {
                                s.toggle_node(target);
                            } else {
                                s.clear();
                                s.select_node(target);
                            }
                        });
                    }
                },
                None => {
                    if rule == ClickRule::Clear {
                        cx.state.doc.select(|_, s| s.clear());
                    }
                }
            }

            if let Some(hint) = cycle_hint {
                cx.state.show_cycle_hint(hint);
            }
        }

        // A face click pair runs along the faces. A replacing click leaves the face selected, a toggling pair switches it
        // on and off again, so only the toggle is taken by its rule rather than by the selection.
        let mut ran_face_run = false;
        let face_rule = click_rule(&cx.state.prefs.keymap_preset, cx.state.select_mode, modifiers.command, modifiers.shift, ClickHit::Face);
        if double_clicked
            && !box_click
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(h) = picking::pick(cx.state, &self.camera.ray(self.rect, pos))
            && let Some(face) = h.face.filter(|f| {
                cx.state.doc.map.brush(h.node).is_some() && (face_rule == ClickRule::ToggleFace || cx.state.doc.selection.faces.contains(&(h.node, *f)))
            })
        {
            crate::box_select::select_face_run(cx.state, (h.node, face), face_rule == ClickRule::ToggleFace);
            ran_face_run = true;
        }

        if double_clicked
            && !box_click
            && !ran_face_run
            && let Some(pos) = response.interact_pointer_pos()
        {
            let hit = self.double_click_hit(cx, pos);
            // The click above already ran and may have closed or selected groups, so this reads the groups afresh.
            let map = &cx.state.doc.map;
            let group =
                hit.map(|h| (h.node, map.click_target(h.node, &cx.state.open_groups))).and_then(|(node, t)| (t != node && is_group(map, t)).then_some(t));
            let kind = match (hit, group) {
                (None, _) => DoubleClickHit::Nothing,
                (Some(_), Some(_)) => DoubleClickHit::ClosedGroup,
                (Some(h), None) if map.owning_entity(h.node).is_some() => DoubleClickHit::EntityBrush,
                (Some(_), None) => DoubleClickHit::Other,
            };
            let action = if pair_on_gizmo && !hit.is_some_and(|h| is_selected(cx.state, h.node)) {
                DoubleClick::Keep
            } else {
                double_click(kind, cx.state.innermost_open_group().is_some())
            };
            match (action, hit, group) {
                (DoubleClick::OpenGroup, Some(h), Some(group)) => {
                    // Blender's Face mode is a switch of its own, so opening a group there picks the face under the
                    // cursor rather than an object, which would flip the mode to Object.
                    let face_mode = cx.state.prefs.keymap_preset == "blender" && cx.state.select_mode == SelectMode::Face;
                    let face = face_mode
                        .then(|| h.face.or_else(|| picking::pick(cx.state, &self.camera.ray(self.rect, pos)).filter(|p| p.node == h.node).and_then(|p| p.face)))
                        .flatten();
                    cx.state.open_groups_to(group);
                    let map = &cx.state.doc.map;
                    let target = map.click_target(h.node, &cx.state.open_groups);
                    cx.state.last_bounds = map.bounds(target);
                    cx.state.outliner_reveal = Some(target);
                    cx.state.doc.select(|_, s| {
                        s.clear();
                        match face {
                            Some(face) => s.select_face(h.node, face),
                            None if face_mode => {}
                            None => s.select_node(target),
                        }
                    });
                }
                (DoubleClick::CloseGroup, ..) => {
                    if let Some(g) = cx.state.close_innermost_group() {
                        let name = cx.state.doc.map.get(g).map(|n| n.name()).unwrap_or_default();
                        cx.state.set_status(msg!("Closed group {name}", name));
                    }
                }
                (DoubleClick::SelectEntity, Some(h), _) => {
                    if let Some(e) = cx.state.doc.map.owning_entity(h.node) {
                        let brushes = cx.state.doc.map.get(e).map(|n| n.children.clone()).unwrap_or_default();
                        cx.state.outliner_reveal = Some(h.node);
                        cx.state.doc.select(|_, s| {
                            s.clear();
                            s.nodes.extend(brushes);
                        });
                    }
                }
                _ => {}
            }
        }

        if response.drag_started_by(PointerButton::Primary) && !self.is_camera_drag() {
            self.press_modifiers = modifiers;
            self.last_click = None;
            cx.state.drop_selection_box();
            if let Some(origin) = press_origin {
                self.drag = self.begin_primary_drag(origin, modifiers, cx);
            }
        }

        if let (Some(drag), Some(pos)) = (self.drag.clone(), pointer)
            && response.dragged_by(PointerButton::Primary)
        {
            self.update_primary_drag(&drag, pos, modifiers, cx);
        }

        if (response.drag_stopped_by(PointerButton::Primary) || (self.drag.is_some() && !self.is_camera_drag() && !ui.input(|i| i.pointer.primary_down())))
            && let Some(drag) = self.drag.take()
        {
            if let Drag::ScreenBox { start, end, op } = drag {
                crate::box_select::apply(cx.state, &self.camera, self.rect, start, pointer.unwrap_or(end), op);
                return;
            }

            if matches!(drag, Drag::CreateBrush { .. } | Drag::CreateBrush2d { .. }) {
                let b = cx.state.doc.map.bounds_of(cx.state.doc.selection.nodes.iter().copied());
                if !b.is_empty() {
                    cx.state.last_bounds = b;
                    cx.tools.last_brush = Some(b);
                }
            }

            // An orbit around a face extrude must not commit the extrude's open transaction.
            if !matches!(drag, Drag::Look | Drag::Pan | Drag::Orbit { .. }) {
                cx.state.drag_preview = None;
                cx.state.doc.commit();
            }
        }

        if self.drag.is_none()
            && self.camera.kind.is_2d()
            && let Some(pos) = response.hover_pos()
            && let Some((normal, segment)) = self
                .edge_under_cursor(pos, cx.state)
                .filter(|_| side_drag_resizes(&cx.state.prefs.keymap_preset, ui.input(|i| i.modifiers)))
                .map(|e| (e.normal, e.segment))
                .or_else(|| self.terrain_edge_under(pos, cx.state).map(|(_, n, s)| (n, s)))
                .or_else(|| self.box_side_under(pos, cx.state))
        {
            let (r, u, _) = self.camera.kind.axes();
            ui.ctx().set_cursor_icon(resize_cursor(Vec2::new(normal.dot(r) as f32, -normal.dot(u) as f32)));
            self.hover_edge = Some(segment);
        }

        let _ = rect;
    }

    /// A click while the Hammer selection box waits: inside the box it selects what the box holds, anywhere else it
    /// drops the box and goes on as a normal click. Returns whether the box took the click.
    fn click_on_selection_box(&self, response: &Response, state: &mut EditorState) -> bool {
        let Some(b) = state.selection_box else { return false };
        let inside =
            b.view == self.camera.kind && response.interact_pointer_pos().is_some_and(|pos| b.contains_point(self.camera.screen_to_plane(self.rect, pos)));
        if inside {
            crate::selection_box::confirm(state);
        } else {
            state.drop_selection_box();
        }

        inside
    }

    fn begin_primary_drag(&mut self, origin: Pos2, modifiers: egui::Modifiers, cx: &mut ViewCtx) -> Option<Drag> {
        let block = cx.state.tool == ToolKind::Block;
        if !block
            && let Some(handle) = crate::gizmos::handle_at(cx.state, &self.camera, self.rect, origin)
            && let Some(start) = crate::gizmos::handle_position(cx.state, handle)
        {
            cx.state.doc.begin(en!("Edit Gizmo"));
            return Some(Drag::Gizmo { handle, start });
        }

        if !block && let Some(drag) = crate::transform_gizmo::begin(cx.state, &self.camera, self.rect, origin) {
            return Some(Drag::Transform(drag));
        }

        if let Some(drag) = self.begin_edge_drag(origin, modifiers, cx.state) {
            return Some(drag);
        }

        let preset = cx.state.prefs.keymap_preset.clone();
        if self.camera.kind.is_2d()
            && let Some((id, normal, _)) = self.terrain_edge_under(origin, cx.state)
        {
            cx.state.terrain_resize = None;
            cx.state.doc.begin(en!("Resize Terrain"));
            return Some(Drag::TerrainEdge { id, origin: self.camera.screen_to_plane(self.rect, origin), normal });
        }

        let state = &mut *cx.state;
        let map = &state.doc.map;
        let ray = self.camera.ray(self.rect, origin);
        let selected = |id: NodeId| is_selected(state, id);
        let hit = if self.camera.kind.is_2d() {
            // A 2D view sees through everything, so a selection under other objects (a floor under its ceiling) still drags.
            let hits = picking::pick_all(state, &ray);
            hits.iter().copied().find(|h| selected(h.node)).or(hits.first().copied())
        } else {
            picking::pick(state, &ray)
        };

        if let Some(h) = hit.filter(|_| !block) {
            let face = if self.camera.kind == ViewKind::Perspective && selected(h.node) { h.face.zip(map.brush(h.node)) } else { None };
            let grabbed = if face.is_some() { DragHit::SelectedFace3d } else { DragHit::Selected };
            let action = selection_drag(&preset, Chord::new(Button::Drag, modifiers), grabbed);
            if let (SelectionDrag::PushFace | SelectionDrag::ExtrudeFace, Some((face, brush))) = (action, face) {
                let plane = brush.faces[face].plane;
                let faces = coplanar_selected_faces(state, &plane);
                state.doc.begin(if action == SelectionDrag::ExtrudeFace { en!("Extrude") } else { en!("Resize Brushes") });
                return Some(Drag::FaceResize { faces, origin: h.point, normal: plane.normal, last_ok: 0.0 });
            }

            if selected(h.node) {
                let plane = match self.camera.kind {
                    ViewKind::Perspective => {
                        if modifiers.alt {
                            let f = self.camera.forward();
                            Plane::from_point_normal(h.point, DVec3::new(f.x, 0.0, f.z).normalize_or(DVec3::Z))
                        } else {
                            Plane::from_point_normal(h.point, DVec3::Y)
                        }
                    }
                    k => Plane::from_point_normal(h.point, -k.axes().2),
                };
                let axis = (self.camera.kind == ViewKind::Perspective && modifiers.alt).then_some(DVec3::Y);
                let duplicate = action == SelectionDrag::Copy;
                state.doc.begin(if duplicate { en!("Duplicate Objects") } else { en!("Move Objects") });
                return Some(Drag::Move { start: h.point, plane, axis, duplicate, lock: shift_locks_axis(&preset) });
            }
        }

        if self.camera.kind.is_2d()
            && let Some((normal, _)) = self.box_side_under(origin, state)
        {
            let start = self.camera.screen_to_plane(self.rect, origin);
            return Some(Drag::ShapeBounds { start, side: Some(normal) });
        }

        if self.camera.kind.is_2d() && state.shape_bounds.is_some() {
            return Some(Drag::ShapeBounds { start: self.camera.screen_to_plane(self.rect, origin), side: None });
        }

        if state.prefs.box_select && state.tool == ToolKind::Select {
            return Some(Drag::ScreenBox { start: origin, end: origin, op: crate::box_select::BoxOp::from_modifiers(modifiers) });
        }

        match empty_drag(&preset, state.tool, Chord::new(Button::Drag, modifiers), self.camera.kind.is_2d()) {
            EmptyDrag::DrawBrush => {}
            EmptyDrag::SelectBox { add } => {
                let at = self.camera.screen_to_plane(self.rect, origin);
                let b = SelectionBox { view: self.camera.kind, a: at, b: at, add };
                state.selection_box = Some(b);
                state.set_status(b.pending_status());
                return Some(Drag::SelectBox);
            }
            EmptyDrag::Nothing => {
                state.set_status(crate::selection_box::block_status(&state.prefs));
                return None;
            }
        }

        match self.camera.kind {
            ViewKind::Perspective => {
                let (start, normal) = match hit {
                    Some(h) => (h.point, h.normal),
                    None => {
                        let t = ray.intersect_plane(&Plane::new(DVec3::Y, 0.0)).filter(|t| *t > 0.0)?;
                        (ray.at(t), DVec3::Y)
                    }
                };
                let axis = td_core::major_axis(normal);
                let mut n = DVec3::ZERO;
                n[axis] = normal[axis].signum();
                state.doc.begin(en!("Create Brush"));
                Some(Drag::CreateBrush { start, normal: n, anchor: state.snap_scalar(start[axis]) })
            }
            _ => {
                let start = self.camera.screen_to_plane(self.rect, origin);
                let axis = self.camera.kind.depth_axis();
                let selected = map.bounds_of(state.doc.selection.nodes.iter().copied());
                let depth = (state.prefs.draw_depth_from_selection && !selected.is_empty() && selected.size()[axis] > 1e-6)
                    .then(|| (selected.min[axis], selected.max[axis]));
                state.doc.begin(en!("Create Brush"));
                Some(Drag::CreateBrush2d { start, depth })
            }
        }
    }

    /// 2D views: a press on a side of the selection resizes it, or with the copy modifiers copies the selection and
    /// moves the copy. The side's grab zone reaches a few points past the brushes, where no pick would hit them.
    fn begin_edge_drag(&self, origin: Pos2, modifiers: egui::Modifiers, state: &mut EditorState) -> Option<Drag> {
        if !self.camera.kind.is_2d() {
            return None;
        }

        let edge = self.edge_under_cursor(origin, state)?;
        let at = self.camera.screen_to_plane(self.rect, origin);
        let lock = shift_locks_axis(&state.prefs.keymap_preset);
        match selection_drag(&state.prefs.keymap_preset, Chord::new(Button::Drag, modifiers), DragHit::Edge2d) {
            SelectionDrag::Resize => {
                state.doc.begin(en!("Resize Brushes"));
                Some(Drag::FaceResize { faces: edge.faces, origin: at, normal: edge.normal, last_ok: 0.0 })
            }
            SelectionDrag::Copy if state.tool != ToolKind::Block => {
                state.doc.begin(en!("Duplicate Objects"));
                let plane = Plane::from_point_normal(at, -self.camera.kind.axes().2);
                Some(Drag::Move { start: at, plane, axis: None, duplicate: true, lock })
            }
            _ => None,
        }
    }

    fn update_primary_drag(&mut self, drag: &Drag, pos: Pos2, modifiers: egui::Modifiers, cx: &mut ViewCtx) {
        let rect = self.rect;
        let ray = self.camera.ray(rect, pos);
        let state = &mut *cx.state;
        let opts = state.opts();
        match drag {
            Drag::Gizmo { handle, start } => {
                if let Some(status) = crate::gizmos::drag(state, *handle, *start, &self.camera, rect, pos, modifiers) {
                    state.set_status(status);
                }
            }
            Drag::Transform(drag) => {
                if let Some(status) = crate::transform_gizmo::drag(state, drag, &self.camera, rect, pos, modifiers) {
                    state.set_status(status);
                }
            }
            Drag::Move { start, plane, axis, duplicate, lock } => {
                let Some(t) = ray.intersect_plane(plane) else { return };
                let mut delta = ray.at(t) - *start;
                if let Some(a) = axis {
                    delta = *a * delta.dot(*a);
                }

                if *lock && modifiers.shift {
                    let i = td_core::major_axis(delta);
                    let keep = delta[i];
                    delta = DVec3::ZERO;
                    delta[i] = keep;
                }

                let delta = state.snap(delta);
                state.doc.reset_transaction();
                if *duplicate {
                    state.drag_preview = None;
                    if delta == DVec3::ZERO {
                        return;
                    }

                    state.doc.edit("Duplicate", |m, s| ops::duplicate_selection(m, s, delta, opts));
                } else {
                    // Render the moved geometry from its pre-drag shape translated on the GPU, so heavy
                    // meshes are not re-tessellated every frame.
                    let nodes = state.doc.selection.geometry(&state.doc.map).into_iter().collect();
                    state.drag_preview = Some(crate::state::DragPreview { nodes, offset: delta });
                    if delta == DVec3::ZERO {
                        return;
                    }

                    state.doc.edit("Move", |m, s| ops::translate_selection(m, s, delta, opts));
                }

                state.set_status(msg!("Move {x} {y} {z}", x = format!("{:.3}", delta.x), y = format!("{:.3}", delta.y), z = format!("{:.3}", delta.z)));
            }
            Drag::CreateBrush { start, normal, anchor } => {
                let axis = td_core::major_axis(*normal);
                let mut plane_n = DVec3::ZERO;
                plane_n[axis] = 1.0;
                let Some(t) = ray.intersect_plane(&Plane::new(plane_n, *anchor)) else { return };
                if t < 0.0 {
                    return;
                }

                let current = ray.at(t);
                let grid = state.grid;
                let mut a = state.snap(*start);
                let mut b = state.snap(current);
                for i in 0..3 {
                    if i == axis {
                        a[i] = *anchor;
                        b[i] = *anchor + normal[axis] * grid;
                    } else {
                        // Include the grid cell under the cursor, like TrenchBroom.
                        (a[i], b[i]) = cell_span(start[i], current[i], grid, state.snap);
                    }
                }

                self.replace_created_brush(Aabb::new(a, b), state);
            }
            Drag::CreateBrush2d { start, depth } => {
                let current = self.camera.screen_to_plane(rect, pos);
                let depth_axis = self.camera.kind.depth_axis();
                let grid = state.grid;
                let mut a = DVec3::ZERO;
                let mut b = DVec3::ZERO;
                for i in 0..3 {
                    if i == depth_axis {
                        if let Some(depth) = depth {
                            (a[i], b[i]) = *depth;
                            continue;
                        }

                        // The depth of the last brush drawn, not of whatever was clicked last.
                        let lb = cx.tools.last_brush.unwrap_or(Aabb::new(DVec3::ZERO, DVec3::splat(FIRST_BRUSH_DEPTH)));
                        (a[i], b[i]) = if lb.is_empty() || lb.size()[i] < 1e-6 { (0.0, grid) } else { (lb.min[i], lb.max[i]) };
                    } else {
                        (a[i], b[i]) = cell_span(start[i], current[i], grid, state.snap);
                    }
                }

                self.replace_created_brush(Aabb::new(a, b), state);
            }
            Drag::ShapeBounds { start, side } => {
                let Some(base) = state.shape_bounds else { return };
                let current = self.camera.screen_to_plane(rect, pos);
                state.shape_bounds = Some(match side {
                    Some(normal) => {
                        let min_size = if state.snap { state.grid } else { 1.0 };
                        crate::shape_bounds::with_side(&base, *normal, state.snap_scalar(current[td_core::major_axis(*normal)]), min_size)
                    }
                    None => crate::shape_bounds::drawn(*start, current, &base, self.camera.kind.depth_axis(), state.grid, state.snap),
                });
            }
            Drag::FaceResize { faces, origin, normal, last_ok } => {
                let dist = match self.camera.kind {
                    ViewKind::Perspective => line_ray_param(*origin, *normal, &ray),
                    _ => Some((self.camera.screen_to_plane(rect, pos) - *origin).dot(*normal)),
                };
                let Some(dist) = dist else { return };
                state.doc.reset_transaction();
                let dist = snap_face_move(state, faces, *normal, dist);
                let extrude = self.press_modifiers.command && self.camera.kind == ViewKind::Perspective;
                let applied = if dist.abs() < 1e-9 {
                    0.0
                } else if extrude && dist > 0.0 {
                    let faces = faces.clone();
                    state.doc.edit("Resize", |m, s| {
                        for (id, face) in &faces {
                            let Some(brush) = m.brush(*id) else { continue };
                            if let Ok(nb) = td_geom::csg::extrude_face(brush, *face, dist) {
                                let parent = m.get(*id).and_then(|n| n.parent).unwrap_or(m.default_layer());
                                let new_id = m.insert(parent, td_doc::NodeKind::Brush(nb));
                                s.nodes.insert(new_id);
                            }
                        }
                    });
                    dist
                } else {
                    let mut tries = [dist, *last_ok].into_iter().filter(|d| d.abs() >= 1e-9);
                    match tries.find_map(|d| moved_brushes(&state.doc.map, faces, *normal, d, opts.uv_lock).map(|b| (d, b))) {
                        Some((applied, moved)) => {
                            state.doc.edit("Resize", |m, _| {
                                for (id, brush) in moved {
                                    if let Some(slot) = m.brush_mut(id) {
                                        *slot = brush;
                                    }
                                }
                            });
                            applied
                        }
                        None => 0.0,
                    }
                };
                if let Some(Drag::FaceResize { last_ok, .. }) = &mut self.drag {
                    *last_ok = applied;
                }

                state.set_status(msg!("Resize {dist}", dist = format!("{applied:.3}")));
            }
            Drag::TerrainEdge { id, origin, normal } => {
                let Some(cell) = state.doc.transaction_base().and_then(|m| m.terrain(*id)).map(|t| t.cell_size) else { return };
                let dist = (self.camera.screen_to_plane(rect, pos) - *origin).dot(*normal);
                if let Some([cx, cz]) = crate::terrain_size::drag_edge(state, *id, *normal, (dist / cell).round() as i64) {
                    state.set_status(msg!("Resized the terrain to {cx} x {cz} cells", cx, cz));
                }
            }
            Drag::ScreenBox { .. } => {
                if let Some(Drag::ScreenBox { end, .. }) = self.drag.as_mut() {
                    *end = pos;
                }
            }
            Drag::SelectBox => {
                let at = self.camera.screen_to_plane(rect, pos);
                if let Some(b) = state.selection_box.as_mut() {
                    b.b = at;
                }
            }
            _ => {}
        }
    }

    fn replace_created_brush(&self, bounds: Aabb, state: &mut EditorState) {
        let mat = state.current_material.clone();
        let parent = state.insert_parent();
        state.doc.reset_transaction();
        if let Ok(brush) = Brush::from_aabb(&bounds, &mat) {
            state.doc.edit("Create Brush", |m, s| {
                let id = ops::create_brush(m, parent, brush);
                s.clear();
                s.select_node(id);
            });
            let size = bounds.size();
            state.set_status(msg!("Brush {x} x {y} x {z}", x = size.x, y = size.y, z = size.z));
            state.bind_status_to_selection();
        }
    }

    /// 2D views: the side of the Shape Generator's box under the cursor and its screen segment.
    fn box_side_under(&self, pos: Pos2, state: &EditorState) -> Option<(DVec3, [Pos2; 2])> {
        let b = state.shape_bounds?;
        let (r, u, _) = self.camera.kind.axes();
        side_under(self.camera.project(self.rect, b.min)?, self.camera.project(self.rect, b.max)?, r, u, pos)
    }

    /// 2D views: the side under the cursor that a drag resizes, with the faces lying on it. Outside Hammer that is a
    /// selected brush face, or a face of the one selected brush, seen edge on, else a side of the selection bounds.
    fn edge_under_cursor(&self, pos: Pos2, state: &EditorState) -> Option<EdgeGrab> {
        let (map, sel) = (&state.doc.map, &state.doc.selection);
        if grabs_slanted_edges(&state.prefs.keymap_preset) {
            let selected: Vec<_> = sel.faces.iter().copied().filter(|&(id, _)| map.brush(id).is_some()).collect();
            if !selected.is_empty() {
                return self.face_edge_under(pos, map, &selected);
            }

            let brushes = sel.brushes(map);
            if let [id] = brushes[..] {
                let all: Vec<_> = (0..map.brush(id)?.faces.len()).map(|f| (id, f)).collect();
                return self.face_edge_under(pos, map, &all);
            }
        }

        if sel.nodes.is_empty() {
            return None;
        }

        self.bounds_edge_under(pos, state)
    }

    /// The face among `candidates` seen edge on under the cursor, with every candidate on its plane.
    fn face_edge_under(&self, pos: Pos2, map: &td_doc::Map, candidates: &[(NodeId, usize)]) -> Option<EdgeGrab> {
        let (keys, sides): (Vec<_>, Vec<_>) =
            candidates.iter().filter_map(|&(id, f)| Some(((id, f), edge_on_side(&self.camera, self.rect, map.brush(id)?, f)?))).unzip();
        let i = nearest_side(pos, &sides)?;
        let (id, f) = keys[i];
        let plane = map.brush(id)?.faces[f].plane;
        Some(EdgeGrab { normal: exact_axis(plane.normal), faces: coplanar_with(map, candidates, &plane), segment: sides[i].segment })
    }

    /// 2D views: the side of the selection bounds under the cursor, with the faces lying on it.
    fn bounds_edge_under(&self, pos: Pos2, state: &EditorState) -> Option<EdgeGrab> {
        let brushes = state.doc.selection.brushes(&state.doc.map);
        if brushes.is_empty() {
            return None;
        }

        let bounds = state.doc.map.bounds_of(brushes.iter().copied());
        let (r, u, _) = self.camera.kind.axes();
        let min = self.camera.project(self.rect, bounds.min)?;
        let max = self.camera.project(self.rect, bounds.max)?;
        let (normal, segment) = side_under(min, max, r, u, pos)?;
        let extreme = if normal.element_sum() > 0.0 { bounds.max.dot(normal) } else { bounds.min.dot(normal) };
        let mut faces = Vec::new();
        for id in brushes {
            let Some(b) = state.doc.map.brush(id) else { continue };
            for (fi, f) in b.faces.iter().enumerate() {
                if f.plane.normal.dot(normal) > 0.9999 && (f.plane.dist - extreme).abs() < 0.01 {
                    faces.push((id, fi));
                }
            }
        }

        (!faces.is_empty()).then_some(EdgeGrab { normal, faces, segment })
    }

    /// 2D views: the side of the one selected terrain under the cursor that grows or crops it. The top and bottom of
    /// its height range in the side views are left alone, heights are sculpted.
    fn terrain_edge_under(&self, pos: Pos2, state: &EditorState) -> Option<(NodeId, DVec3, [Pos2; 2])> {
        let sel = &state.doc.selection;
        let mut nodes = sel.nodes.iter();
        let (Some(&id), None) = (nodes.next(), nodes.next()) else { return None };
        let terrain = state.doc.map.terrain(id).filter(|_| sel.faces.is_empty() && state.doc.map.is_editable(id))?;
        let [lo, hi] = crate::terrain_size::height_range(state, id, terrain);
        let [cx, cz] = terrain.cells();
        let min = DVec3::new(terrain.origin.x, lo, terrain.origin.z);
        let max = DVec3::new(terrain.origin.x + cx as f64 * terrain.cell_size, hi, terrain.origin.z + cz as f64 * terrain.cell_size);
        let (r, u, _) = self.camera.kind.axes();
        let (mut a, mut b) = (self.camera.project(self.rect, min)?, self.camera.project(self.rect, max)?);
        // Flat ground is a line in the side views, its ends only get a grab zone once the box has some height.
        let short = 4.0 * EDGE_GRAB - (a.y - b.y).abs();
        if short > 0.0 && self.camera.kind != ViewKind::Top {
            let grow = if a.y <= b.y { short / 2.0 } else { -short / 2.0 };
            (a.y, b.y) = (a.y - grow, b.y + grow);
        }

        side_under_where(a, b, r, u, pos, |normal| normal.y == 0.0).map(|(normal, segment)| (id, normal, segment))
    }

    /// Shows where a dragged payload lands: the faces a material goes on, or the surface point an entity rests on.
    fn paint_drop_target(&self, ui: &Ui, cx: &ViewCtx, payload: &DndPayload, pos: egui::Pos2) {
        let ray = self.camera.ray(self.rect, pos);
        let painter = ui.painter().with_clip_rect(self.rect);
        let drop_color = crate::theme::VIEW.cyan;
        let stroke = egui::Stroke::new(2.0, drop_color);
        match payload {
            DndPayload::Material(_) => {
                let Some(hit) = picking::pick(cx.state, &ray) else { return };
                let map = &cx.state.doc.map;
                let whole = ui.input(|i| i.modifiers.shift);
                let polygons: Vec<Vec<DVec3>> = match (map.brush(hit.node), map.mesh(hit.node), hit.face) {
                    (Some(b), _, Some(face)) if !whole => vec![b.face_points(face)],
                    (Some(b), _, Some(_)) => (0..b.faces.len()).map(|f| b.face_points(f)).collect(),
                    (_, Some(m), Some(face)) => {
                        let faces: Vec<usize> = if whole { (0..m.faces.len()).collect() } else { vec![face] };
                        faces.iter().filter_map(|f| m.faces.get(*f)).map(|f| f.indices.iter().map(|i| m.vertices[*i as usize]).collect()).collect()
                    }
                    _ => Vec::new(),
                };
                for polygon in polygons {
                    let Some(points) = polygon.iter().map(|p| self.camera.project(self.rect, *p)).collect::<Option<Vec<_>>>() else { continue };
                    painter.add(egui::Shape::closed_line(points.clone(), stroke));
                    if map.brush(hit.node).is_some() {
                        painter.add(egui::Shape::convex_polygon(points, drop_color.gamma_multiply(0.25), egui::Stroke::NONE));
                    }
                }
            }
            DndPayload::Entities(_) | DndPayload::Model(_) => {
                let through = ui.input(|i| i.modifiers.alt);
                let Some((at, _)) = crate::commands::drop_target(cx.state, &ray, self.camera.kind, through) else { return };
                let Some(point) = self.camera.project(self.rect, at) else { return };
                painter.circle(point, 5.0, drop_color.gamma_multiply(0.4), stroke);
                // The marker alone shows no depth in a 2D view, so say how deep the drop lands.
                if self.camera.kind.is_2d() {
                    let axis = self.camera.kind.depth_axis();
                    let (axis, depth) = (["X", "Y", "Z"][axis], at[axis].round() + 0.0);
                    let text = if through { trf!("{axis} {depth}", axis, depth) } else { trf!("{axis} {depth}, hold Alt to drop through", axis, depth) };

                    painter.text(point + Vec2::new(10.0, -10.0), Align2::LEFT_BOTTOM, text, FontId::proportional(13.0), ui.visuals().strong_text_color());
                }
            }
        }
    }

    fn handle_drop(&mut self, ui: &Ui, response: &Response, cx: &mut ViewCtx) {
        let Some(payload) = response.dnd_release_payload::<DndPayload>() else {
            if let Some(payload) = response.dnd_hover_payload::<DndPayload>() {
                ui.painter().rect_stroke(self.rect.shrink(1.0), 0.0, egui::Stroke::new(2.0, crate::theme::VIEW.cyan), egui::StrokeKind::Inside);
                ui.ctx().set_cursor_icon(CursorIcon::Copy);
                if let Some(pos) = drop_pos(response) {
                    self.paint_drop_target(ui, cx, &payload, pos);
                }
            }

            return;
        };
        let Some(pos) = drop_pos(response) else { return };
        let ray = self.camera.ray(self.rect, pos);
        let hit = picking::pick(cx.state, &ray);
        match payload.as_ref() {
            DndPayload::Material(name) => {
                let name = name.clone();
                cx.state.current_material = name.clone();
                // Alt while dropping on a surface lays the material as a decal sheet instead of painting it.
                if ui.input(|i| i.modifiers.alt) {
                    if let Some(h) = hit {
                        cx.actions.push(Action::CreateDecal { material: name, at: h.point, normal: h.normal, size: None });
                    }

                    return;
                }

                let apply_whole = ui.input(|i| i.modifiers.shift);
                match hit {
                    Some(Hit { node, face: Some(face), .. }) => {
                        cx.state.doc.edit("Apply Material", |m, _| {
                            if let Some(b) = m.brush_mut(node) {
                                if apply_whole {
                                    b.set_material(&name);
                                } else {
                                    b.faces[face].data.material = name.clone();
                                }
                            } else if let Some(mesh) = m.mesh_mut(node) {
                                if apply_whole {
                                    mesh.faces.iter_mut().for_each(|f| f.data.material = name.clone());
                                } else if let Some(f) = mesh.faces.get_mut(face) {
                                    f.data.material = name.clone();
                                }
                            }
                        });
                    }
                    Some(Hit { node, face: None, .. }) if cx.state.doc.map.terrain(node).is_some() => {
                        // Dropping on a terrain assigns the material to the layer being painted.
                        let blend = cx.state.tool == ToolKind::Blend;
                        let layer = if blend { cx.state.blend.layer } else { cx.state.sculpt.layer as usize };
                        let slot = cx.state.doc.edit("Set Terrain Layer", |m, _| m.terrain_mut(node).map(|t| set_terrain_layer(t, layer, &name)));
                        if let Some(slot) = slot.filter(|s| *s != layer) {
                            if blend {
                                cx.state.blend.layer = slot;
                            } else {
                                cx.state.sculpt.layer = slot as u8;
                            }

                            cx.state.set_status(msg!("The terrain had no layer {layer}, {name} was added as layer {slot}", layer, name, slot));
                        }
                    }
                    _ => {}
                }
            }
            DndPayload::Entities(classnames) => {
                let through = ui.input(|i| i.modifiers.alt);
                let (at, normal) = match (self.camera.kind, crate::commands::drop_target(cx.state, &ray, self.camera.kind, through)) {
                    (_, Some(target)) => target,
                    (ViewKind::Perspective, None) => (cx.state.cursor_world.unwrap_or(ray.at(256.0)), None),
                    (_, None) => (self.camera.screen_to_plane(self.rect, pos), None),
                };
                let right = self.camera.right();
                let axis = right.abs().max_position();
                let row = DVec3::AXES[axis] * right[axis].signum();
                cx.actions.push(Action::PlaceEntities { classnames: classnames.clone(), at: Some(at), normal, row });
            }
            DndPayload::Model(path) => {
                let through = ui.input(|i| i.modifiers.alt);
                let at = match (self.camera.kind, crate::commands::drop_target(cx.state, &ray, self.camera.kind, through)) {
                    (_, Some((p, _))) => p,
                    (ViewKind::Perspective, None) => cx.state.cursor_world.unwrap_or(ray.at(256.0)),
                    (_, None) => self.camera.screen_to_plane(self.rect, pos),
                };
                cx.actions.push(Action::PlaceModel { path: path.clone(), at: cx.state.snap(at) });
            }
        }
    }

    /// Renders this view's camera into a throwaway target of `size` pixels, for screenshots larger than the docked view.
    /// Without `overlays` it is a beauty shot: no grid, entity boxes, volumes, edges, selection outlines or links.
    pub fn render_offscreen(
        &self,
        renderer: &mut Renderer,
        scene: &SceneCache,
        state: &EditorState,
        size: [u32; 2],
        overlays: bool,
    ) -> Option<image::RgbaImage> {
        let mut target = None;
        renderer.ensure_target(&mut target, size);
        let target = target?;
        let size_points = Vec2::new(size[0] as f32, size[1] as f32);
        let mut params = self.frame_params(size_points, state, 1.0);
        let is_2d = self.camera.kind.is_2d();
        let grid =
            if overlays && is_2d { renderer.upload_lines(&grid_lines(&self.camera, Rect::from_min_size(Pos2::ZERO, size_points), state.grid)) } else { None };
        if !overlays {
            params.grid_size = 0.0;
        }

        let lit = state.prefs.shade.is_lit();
        let mut frame = Frame::default();
        frame.overlay_lines.extend(grid.as_ref());
        scene.fill_frame(&mut frame, is_2d, lit, overlays);
        frame.sky = !is_2d && lit;
        renderer.render(&target, &params, &frame);
        let image = renderer.read_target(&target);
        renderer.free_target(target);
        image
    }

    fn frame_params(&self, size: Vec2, state: &EditorState, line_width: f32) -> FrameParams {
        let is_2d = self.camera.kind.is_2d();
        let palette = crate::theme::colors();
        FrameParams {
            view_proj: self.camera.view_proj(size).as_mat4(),
            eye: self.camera.eye().as_vec3(),
            grid_size: if is_2d { 0.0 } else { state.grid as f32 },
            grid_alpha: state.prefs.grid_alpha,
            shade: state.shade_mode(),
            orthographic: is_2d,
            clear: if is_2d {
                crate::theme::linear(palette.view_2d)
            } else if state.prefs.shade.is_lit() {
                BG_LIT
            } else {
                crate::theme::linear(crate::theme::VIEW.gray_1)
            },
            line_width,
            selection_line_width: if is_2d { line_width * palette.selection_line_scale } else { line_width },
            line_flip: if is_2d { palette.flip_pivot() } else { None },
        }
    }

    fn render(&mut self, cx: &mut ViewCtx, tool_lines: &[LineVertex]) {
        let Some(target) = &self.target else { return };
        let rect = self.rect;
        let state = &cx.state;
        let is_2d = self.camera.kind.is_2d();
        let params = self.frame_params(rect.size(), state, self.pixels_per_point);
        let grid = if is_2d { cx.renderer.upload_lines(&grid_lines(&self.camera, rect, state.grid)) } else { None };
        let tools = cx.renderer.upload_lines(tool_lines);
        let scene = cx.scene;
        let mut frame = Frame::default();
        if is_2d {
            frame.overlay_lines.extend(grid.as_ref());
        }

        let lit = state.prefs.shade.is_lit();
        scene.fill_frame(&mut frame, is_2d, lit, true);
        frame.sky = !is_2d && lit;
        frame.overlay_lines.extend(tools.as_ref());
        cx.renderer.render(target, &params, &frame);
    }

    fn paint_overlay(&self, ui: &Ui, cx: &ViewCtx) {
        let painter = ui.painter_at(self.rect);
        let label_color = Color32::from_rgb(200, 200, 210);
        let text = match self.camera.kind {
            ViewKind::Perspective => format!("{}  {}", tr(self.camera.kind.label()), tr(cx.state.prefs.shade.title())),
            // Two significant digits, so a notch of the wheel shows when zoomed far out too.
            k => trf!(
                "{view}  zoom {zoom}",
                view = tr(k.label()),
                zoom = format!(
                    "{:.*}",
                    if self.camera.zoom < 0.01 {
                        4
                    } else if self.camera.zoom < 0.1 {
                        3
                    } else {
                        2
                    },
                    self.camera.zoom
                )
            ),
        };
        let label = painter.text(self.rect.min + Vec2::new(8.0, 6.0), Align2::LEFT_TOP, text, FontId::proportional(12.0), label_color);
        // Until the first right drag. An empty map's card says the same, and Hide tips turns both off.
        if self.camera.kind == ViewKind::Perspective
            && self.hovered
            && !self.looked
            && cx.state.prefs.start_hints
            && !crate::welcome::map_is_empty(&cx.state.doc.map)
        {
            let hint = painter.layout(
                crate::i18n::tr(crate::welcome::camera_help(&cx.state.prefs.keymap_preset, false)).into(),
                FontId::proportional(12.0),
                label_color.gamma_multiply(0.6),
                self.rect.width() - 16.0,
            );
            painter.galley(label.left_bottom() + Vec2::new(0.0, 4.0), hint, label_color);
        }

        if self.mouse_look {
            let center = self.rect.center();
            for (width, color) in [(3.0, Color32::from_black_alpha(160)), (1.0, label_color)] {
                for arm in [Vec2::X * 7.0, Vec2::Y * 7.0] {
                    painter.line_segment([center - arm, center + arm], egui::Stroke::new(width, color));
                }
            }
        }

        let state = &cx.state;
        let active = match &self.drag {
            Some(Drag::Gizmo { handle, .. }) => Some(*handle),
            _ => None,
        };
        crate::gizmos::paint_overlay(ui, &self.camera, self.rect, state, active);
        let transforming = match &self.drag {
            Some(Drag::Transform(drag)) => Some(drag.part),
            _ => None,
        };
        crate::transform_gizmo::paint(ui, &self.camera, self.rect, state, transforming);
        if self.camera.kind.is_2d() {
            if !state.doc.selection.nodes.is_empty() {
                let b = state.doc.map.bounds_of(state.doc.selection.nodes.iter().copied());
                size_labels(&painter, &self.camera, self.rect, b, Color32::from_rgb(255, 120, 100), false);
            }

            if let Some(b) = state.shape_bounds {
                size_labels(&painter, &self.camera, self.rect, b, crate::theme::VIEW.shape_bounds(), false);
            }

            crate::selection_box::paint(&painter, &self.camera, self.rect, state);
            if let Some(Drag::ScreenBox { start, end, .. }) = self.drag {
                crate::selection_box::paint_rect(&painter, Rect::from_two_pos(start, end));
            }

            // The pending size outlines the selected terrain, so its labels go on the other sides.
            if let Some(b) = crate::terrain_size::footprint(state) {
                size_labels(&painter, &self.camera, self.rect, b, crate::theme::VIEW.shape_bounds(), true);
            }

            paint_terrain3d_marks(&painter, &self.camera, self.rect, state);
        }

        if state.prefs.godot_overlays {
            let ghost = Color32::from_rgba_unmultiplied(115, 184, 255, 230);
            for item in &state.overlay_ghosts.items {
                let points: Vec<Pos2> = item.drawn_bounds().corners().iter().filter_map(|c| self.camera.project(self.rect, *c)).collect();
                if points.len() < 8 {
                    continue;
                }

                // Only boxes big enough on screen get a name, an overview would drown in labels.
                let screen = Rect::from_points(&points);
                if screen.width().max(screen.height()) < 28.0 || !self.rect.intersects(screen) {
                    continue;
                }

                painter.text(Pos2::new(screen.center().x, screen.min.y - 2.0), Align2::CENTER_BOTTOM, item.label(), FontId::proportional(11.0), ghost);
            }
        }

        if self.camera.kind == ViewKind::Perspective {
            for (id, e) in state.doc.map.entities() {
                let selected = state.doc.selection.nodes.contains(&id);
                if !selected || state.doc.map.is_hidden(id) {
                    continue;
                }

                let pos = if state.doc.map.is_point_entity(id) { e.origin } else { state.doc.map.bounds(id).center() };
                if let Some(p) = self.camera.project(self.rect, pos) {
                    painter.text(p + Vec2::new(0.0, -18.0), Align2::CENTER_BOTTOM, e.classname.as_str(), FontId::proportional(12.0), Color32::WHITE);
                }
            }
        }
    }

    pub fn focus(&mut self, bounds: &Aabb) {
        if self.rect.width() > 1.0 {
            self.camera.focus(bounds, self.rect);
        } else {
            self.camera.focus(bounds, Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)));
        }
    }

    pub fn ray_at(&self, pos: Pos2) -> Ray {
        self.camera.ray(self.rect, pos)
    }

    pub fn target(&self) -> Option<&ViewTarget> {
        self.target.as_ref()
    }
}

/// How soon a second click at the same spot in a 2D view has to follow to select the next object under the cursor.
/// Longer than a double click, short enough that clicking the selection again later does not cycle away from it.
const CLICK_CYCLE_SECONDS: f64 = 1.5;
/// Points of `smooth_scroll_delta` egui makes of one mouse wheel notch. Trackpads send smaller point deltas.
const NOTCH_POINTS: f64 = 40.0;
/// Zoom factor of one wheel notch in the 2D views.
const WHEEL_ZOOM: f64 = 1.2;
/// Share of the distance to the surface straight ahead, or with nothing there the one under the pointer, that one
/// wheel notch moves the 3D camera.
const WHEEL_DOLLY: f64 = 0.2;

/// Wheel movement this frame in notches, from the raw events whose modifiers pass `keep`. egui spreads one notch
/// over several frames of `smooth_scroll_delta` and turns Ctrl+wheel into zoom, so tools that step or resize per
/// notch read the events instead. Positive is away from the user.
pub fn wheel_notches(input: &egui::InputState, keep: impl Fn(egui::Modifiers) -> bool) -> f32 {
    input
        .raw
        .events
        .iter()
        .map(|e| match e {
            egui::Event::MouseWheel { unit, delta, modifiers, .. } if keep(*modifiers) => match unit {
                egui::MouseWheelUnit::Line => delta.y,
                egui::MouseWheelUnit::Point => delta.y / 50.0,
                egui::MouseWheelUnit::Page => delta.y * 3.0,
            },
            _ => 0.0,
        })
        .sum()
}

/// The distance to move `faces` along `normal` so that an axis aligned face lands on the grid, not just a grid step
/// away from where it was. Slanted faces still move in grid steps.
fn snap_face_move(state: &EditorState, faces: &[(NodeId, usize)], normal: DVec3, dist: f64) -> f64 {
    let plane = faces.first().and_then(|&(id, face)| state.doc.map.brush(id)?.faces.get(face).map(|f| f.plane));
    let Some(plane) = plane.filter(|_| state.snap && normal.abs().max_element() > 1.0 - 1e-6) else { return state.snap_scalar(dist) };
    let facing = plane.normal.dot(normal);
    if facing.abs() < 1.0 - 1e-6 {
        return state.snap_scalar(dist);
    }

    let at = plane.dist * facing;
    state.snap_scalar(at + dist) - at
}

/// The outward normal and screen segment of the side of a box within `EDGE_GRAB` of `pos` in a 2D view, given where
/// the box's min and max corners land on screen and the view's right and up axes.
pub(crate) fn side_under(min: Pos2, max: Pos2, r: DVec3, u: DVec3, pos: Pos2) -> Option<(DVec3, [Pos2; 2])> {
    side_under_where(min, max, r, u, pos, |_| true)
}

/// [`side_under`] among the sides whose normal passes `keep`.
fn side_under_where(min: Pos2, max: Pos2, r: DVec3, u: DVec3, pos: Pos2, keep: impl Fn(DVec3) -> bool) -> Option<(DVec3, [Pos2; 2])> {
    let screen = Rect::from_two_pos(min, max);
    if !screen.expand(EDGE_GRAB).contains(pos) {
        return None;
    }

    // View axes can point along negative world axes, so the side of the screen the bounds minimum lands on picks the face.
    let (r, u) = (r.abs(), u.abs());
    let candidates = [
        ((pos.x - screen.min.x).abs(), screen.width(), if min.x <= max.x { -r } else { r }, [screen.left_top(), screen.left_bottom()]),
        ((pos.x - screen.max.x).abs(), screen.width(), if min.x <= max.x { r } else { -r }, [screen.right_top(), screen.right_bottom()]),
        ((pos.y - screen.min.y).abs(), screen.height(), if min.y <= max.y { -u } else { u }, [screen.left_top(), screen.right_top()]),
        ((pos.y - screen.max.y).abs(), screen.height(), if min.y <= max.y { u } else { -u }, [screen.left_bottom(), screen.right_bottom()]),
    ];
    let (dist, extent, normal, segment) = candidates.into_iter().filter(|c| keep(c.2)).min_by(|a, b| a.0.total_cmp(&b.0))?;
    // Inside a thin box the edge zones leave its middle third for moving it.
    let grab = if screen.contains(pos) { EDGE_GRAB.min(extent / 3.0) } else { EDGE_GRAB };
    (dist <= grab).then_some((normal, segment))
}

/// A grid aligned span covering `a` to `b` and the cells under both ends, at least one cell wide. An end a hair
/// past a grid line (pointer to world rounding) counts as on it.
pub(crate) fn cell_span(a: f64, b: f64, grid: f64, snap: bool) -> (f64, f64) {
    let (lo, hi) = (a.min(b), a.max(b));
    let eps = grid * 1e-3;
    let (lo, hi) = if snap { (((lo + eps) / grid).floor() * grid, ((hi - eps) / grid).ceil() * grid) } else { (lo, hi) };
    if (hi - lo).abs() < 1e-6 { (lo, lo + grid) } else { (lo, hi) }
}

fn paint_terrain3d_marks(painter: &egui::Painter, camera: &Camera, rect: Rect, state: &EditorState) {
    use crate::terrain3d::overlay::{Mark, marks, outline};

    let border = Color32::from_rgba_unmultiplied(150, 150, 150, 120);
    let warning = crate::theme::colors().warning();
    let none = std::collections::BTreeSet::new();
    for (id, layout) in &state.terrain3d_tiles.layouts {
        let conflicts = state.terrain3d_tiles.conflicts.get(id).unwrap_or(&none);
        for mark in marks(camera, rect, &layout.grid, &layout.locs, conflicts) {
            match mark {
                Mark::Region { rect: area, conflict, label } => {
                    let (color, width) = if conflict { (warning, 2.0) } else { (border, 1.0) };
                    painter.rect_stroke(area, 0.0, egui::Stroke::new(width, color), egui::StrokeKind::Inside);
                    if let Some((pos, text)) = label {
                        painter.text(pos, Align2::LEFT_TOP, text, FontId::monospace(11.0), color);
                    }
                }
                Mark::Border { x, conflict } => {
                    let (color, width) = if conflict { (warning, 2.0) } else { (border, 1.0) };
                    painter.line_segment([Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)], egui::Stroke::new(width, color));
                }
                Mark::Plan { .. } | Mark::PlanBorder { .. } => {}
            }
        }
    }

    let Some((id, min, max)) = state.terrain3d_outline else { return };
    let Some(layout) = state.terrain3d_tiles.layouts.get(&id) else { return };
    let stroke = egui::Stroke::new(2.0, crate::theme::colors().info());
    for mark in outline(camera, rect, &layout.grid, min, max) {
        match mark {
            Mark::Plan { rect: area } => {
                painter.rect_stroke(area, 0.0, stroke, egui::StrokeKind::Inside);
            }
            Mark::PlanBorder { x } => {
                painter.line_segment([Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)], stroke);
            }
            _ => {}
        }
    }
}

/// Width and height of `b` beside it in a 2D view, above and to the right, or below and to the left when `opposite`.
fn size_labels(painter: &egui::Painter, camera: &Camera, rect: Rect, b: Aabb, color: Color32, opposite: bool) {
    if b.is_empty() {
        return;
    }

    let (r, u, _) = camera.kind.axes();
    let size = b.size();
    let w = (size * r.abs()).element_sum();
    let h = (size * u.abs()).element_sum();
    if let (Some(p0), Some(p1)) = (camera.project(rect, b.min), camera.project(rect, b.max)) {
        let sr = Rect::from_two_pos(p0, p1);
        let (width_at, height_at) = if opposite {
            ((Pos2::new(sr.center().x, sr.max.y + 4.0), Align2::CENTER_TOP), (Pos2::new(sr.min.x - 6.0, sr.center().y), Align2::RIGHT_CENTER))
        } else {
            ((Pos2::new(sr.center().x, sr.min.y - 4.0), Align2::CENTER_BOTTOM), (Pos2::new(sr.max.x + 6.0, sr.center().y), Align2::LEFT_CENTER))
        };
        painter.text(width_at.0, width_at.1, crate::gizmos::fmt_num(w), FontId::monospace(11.0), color);
        painter.text(height_at.0, height_at.1, crate::gizmos::fmt_num(h), FontId::monospace(11.0), color);
    }
}

fn coplanar_selected_faces(state: &EditorState, plane: &Plane) -> Vec<(NodeId, usize)> {
    let mut out = Vec::new();
    for id in state.doc.selection.brushes(&state.doc.map) {
        if let Some(b) = state.doc.map.brush(id)
            && let Some(f) = b.find_face_by_plane(plane)
        {
            out.push((id, f));
        }
    }

    out
}

/// The faces among `faces` that lie on `plane`.
fn coplanar_with(map: &td_doc::Map, faces: &[(NodeId, usize)], plane: &Plane) -> Vec<(NodeId, usize)> {
    let on_plane = |&(id, f): &(NodeId, usize)| map.brush(id).and_then(|b| b.faces.get(f)).is_some_and(|f| f.plane.approx_eq(plane, 1e-6, 1e-4));
    faces.iter().copied().filter(on_plane).collect()
}

/// The brushes holding `faces` with those faces moved `dist` along `normal`, or None when any brush would lose a face,
/// so faces that move together never come apart.
fn moved_brushes(map: &td_doc::Map, faces: &[(NodeId, usize)], normal: DVec3, dist: f64, uv_lock: bool) -> Option<Vec<(NodeId, Brush)>> {
    faces.iter().filter_map(|&(id, f)| Some(map.brush(id)?.move_face(f, normal * dist, uv_lock).ok().map(|b| (id, b)))).collect()
}

/// The world axis `n` lies along within 1e-6, else `n`. An axis aligned side then lands on grid lines when snapped.
fn exact_axis(n: DVec3) -> DVec3 {
    let mut axis = DVec3::ZERO;
    let i = td_core::major_axis(n);
    axis[i] = n[i].signum();
    if (n - axis).abs().max_element() <= 1e-6 { axis } else { n }
}

/// A brush face seen edge on in a 2D view, a side of the brush outline there.
struct FaceSide {
    segment: [Pos2; 2],
    /// Outward on screen, where y grows down.
    screen_normal: Vec2,
    /// How far the brush reaches on screen along `screen_normal`.
    thickness: f32,
}

fn edge_on_side(cam: &Camera, rect: Rect, brush: &Brush, face: usize) -> Option<FaceSide> {
    let n = brush.faces.get(face)?.plane.normal;
    let (r, u, forward) = cam.kind.axes();
    if n.dot(forward).abs() >= 1e-4 {
        return None;
    }

    let screen_normal = Vec2::new(n.dot(r) as f32, -n.dot(u) as f32).normalized();
    let along = screen_normal.rot90();
    let points = brush.face_points(face).into_iter().map(|p| cam.project(rect, p)).collect::<Option<Vec<_>>>()?;
    let key = |p: &&Pos2| p.to_vec2().dot(along);
    let a = *points.iter().min_by(|p, q| key(p).total_cmp(&key(q)))?;
    let b = *points.iter().max_by(|p, q| key(p).total_cmp(&key(q)))?;
    let depths = brush.vertices.iter().filter_map(|&v| cam.project(rect, v)).map(|p| p.to_vec2().dot(screen_normal));
    let (lo, hi) = depths.fold((f32::MAX, f32::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)));
    Some(FaceSide { segment: [a, b], screen_normal, thickness: hi - lo })
}

/// The side whose grab zone holds `pos`, the nearest when zones overlap, and on a tie the one `pos` lies further
/// outside of. A zone reaches `EDGE_GRAB` out from its side and past both ends, like [`side_under`] for a box, but
/// inside the brush only a third of its thickness, so a thin brush keeps its middle for moving.
fn nearest_side(pos: Pos2, sides: &[FaceSide]) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, s) in sides.iter().enumerate() {
        let [a, b] = s.segment;
        let along = s.screen_normal.rot90();
        let t = (pos - a).dot(along);
        let out = (pos - a).dot(s.screen_normal);
        let grab = if out < 0.0 { EDGE_GRAB.min(s.thickness / 3.0) } else { EDGE_GRAB };
        if t < -EDGE_GRAB || t > (b - a).dot(along) + EDGE_GRAB || out.abs() > grab {
            continue;
        }

        if best.is_none_or(|(_, o)| out.abs() < o.abs() || (out.abs() == o.abs() && out > o)) {
            best = Some((i, out));
        }
    }

    best.map(|(i, _)| i)
}

fn draws_like_select(tool: ToolKind) -> bool {
    matches!(tool, ToolKind::Select | ToolKind::Block)
}

/// Whether a press with `modifiers` on a side of the selection resizes it, or lets the press copy the selection.
fn side_drag_resizes(preset: &str, modifiers: egui::Modifiers) -> bool {
    selection_drag(preset, Chord::new(Button::Drag, modifiers), DragHit::Edge2d) == SelectionDrag::Resize
}

/// The resize arrow closest to a side's outward screen normal, straight within 22.5 degrees of a screen axis.
fn resize_cursor(screen_normal: Vec2) -> CursorIcon {
    let n = screen_normal.normalized();
    let tan = std::f32::consts::FRAC_PI_8.tan();
    if n.y.abs() <= n.x.abs() * tan {
        CursorIcon::ResizeHorizontal
    } else if n.x.abs() <= n.y.abs() * tan {
        CursorIcon::ResizeVertical
    } else if n.x * n.y < 0.0 {
        CursorIcon::ResizeNeSw
    } else {
        CursorIcon::ResizeNwSe
    }
}

/// Redraws the shapes painted since `start` for the paper of a light 2D view, the same flip the line shader applies.
fn flip_shapes(ctx: &egui::Context, layer: egui::LayerId, start: egui::layers::ShapeIdx, pivot: f32) {
    ctx.graphics_mut(|g| {
        let list = g.entry(layer);
        for i in start.0..list.next_idx().0 {
            list.mutate_shape(egui::layers::ShapeIdx(i), |clipped| {
                egui::epaint::shape_transform::adjust_colors(&mut clipped.shape, move |c| *c = crate::theme::flip_color(*c, pivot));
            });
        }
    });
}

fn grid_lines(cam: &Camera, rect: Rect, grid: f64) -> Vec<LineVertex> {
    let (r, u, _) = cam.kind.axes();
    let (r0, r1, u0, u1) = cam.visible_range(rect);
    let mut spacing = grid.max(0.125);
    while spacing * cam.zoom < 10.0 {
        spacing *= 2.0;
    }

    let depth = cam.kind.depth_axis();
    let mut base = DVec3::ZERO;
    base[depth] = 0.0;
    let mut out = Vec::new();
    let axis_color = |a: DVec3| -> [f32; 4] {
        match td_core::major_axis(a) {
            0 => [0.45, 0.08, 0.08, 0.8],
            1 => [0.08, 0.4, 0.08, 0.8],
            _ => [0.08, 0.12, 0.5, 0.8],
        }
    };
    let mut push = |along: DVec3, across: DVec3, value: f64, from: f64, to: f64, index: i64| {
        let color = if index == 0 {
            axis_color(across)
        } else if index % 8 == 0 {
            [0.16, 0.16, 0.19, 0.6]
        } else {
            [0.09, 0.09, 0.11, 0.45]
        };
        let p0 = base + along * value + across * from;
        let p1 = base + along * value + across * to;
        out.push(LineVertex { pos: v3(p0), color });
        out.push(LineVertex { pos: v3(p1), color });
    };
    let (i0, i1) = ((r0 / spacing).floor() as i64, (r1 / spacing).ceil() as i64);
    for i in i0..=i1 {
        push(r, u, i as f64 * spacing, u0, u1, (i as f64 * spacing / grid).round() as i64);
    }

    let (j0, j1) = ((u0 / spacing).floor() as i64, (u1 / spacing).ceil() as i64);
    for j in j0..=j1 {
        push(u, r, j as f64 * spacing, r0, r1, (j as f64 * spacing / grid).round() as i64);
    }

    out
}

/// Screen distance in points within which a drag grabs an outer edge of the selected brushes in a 2D view.
const EDGE_GRAB: f32 = 8.0;

struct EdgeGrab {
    normal: DVec3,
    faces: Vec<(NodeId, usize)>,
    /// The edge on screen, highlighted while the pointer is near it.
    segment: [Pos2; 2],
}

fn is_group(map: &td_doc::Map, id: NodeId) -> bool {
    matches!(map.get(id).map(|n| &n.kind), Some(td_doc::NodeKind::Group(_)))
}

/// Whether clicking `id` would land on something selected: the object a click picks, or a group or entity around it.
fn is_selected(state: &EditorState, id: NodeId) -> bool {
    let map = &state.doc.map;
    let target = map.click_target(id, &state.open_groups);
    let nodes = &state.doc.selection.nodes;
    nodes.contains(&id) || nodes.contains(&target) || map.ancestors(id).iter().any(|a| nodes.contains(a))
}

fn context_menu(ui: &mut Ui, state: &EditorState, actions: &mut Vec<Action>) {
    let has_sel = !state.doc.selection.nodes.is_empty();
    let cursor = state.cursor_world;
    let point_classes: Vec<String> = state.game.point_entities().map(|e| e.classname.clone()).collect();
    let solid_classes: Vec<String> = state.game.solid_entities().map(|e| e.classname.clone()).collect();
    let layers: Vec<(NodeId, String)> = state.doc.map.layers.iter().filter_map(|l| state.doc.map.get(*l).map(|n| (*l, n.name()))).collect();
    let mut out: Vec<Action> = Vec::new();

    let brush_faces = state.doc.selection.faces.iter().any(|&(id, _)| state.doc.map.brush(id).is_some());
    if state.prefs.keymap_preset == "blender" && brush_faces {
        let enabled = state.tool != ToolKind::Mesh;
        let mut resp = ui.add_enabled(enabled, egui::Button::new(tr(en!("Extrude Faces"))));
        if let Some(help) = Action::ExtrudeFaces.help() {
            resp = resp.on_hover_text(tr(help));
        }

        if resp.clicked() {
            out.push(Action::ExtrudeFaces);
            ui.close();
        }

        ui.separator();
    }

    ui.menu_button(tr("Create Point Entity"), |ui| {
        for c in point_classes {
            if ui.button(&c).clicked() {
                out.push(Action::CreatePointEntity { classname: c, at: cursor });
                ui.close();
            }
        }
    });
    let has_brushes = !td_doc::ops::brush_entity_geometry(&state.doc.map, &state.doc.selection).is_empty();
    ui.add_enabled_ui(has_brushes, |ui| {
        ui.menu_button(tr("Create Brush Entity"), |ui| {
            for c in solid_classes {
                if ui.button(&c).clicked() {
                    out.push(Action::CreateBrushEntity(c));
                    ui.close();
                }
            }
        });
    });
    let mut entries: Vec<Option<(&str, Action)>> = vec![
        Some((en!("Rename"), Action::Rename)),
        Some((en!("Move Brushes to World"), Action::MoveToWorld)),
        None,
        Some((en!("Create Prefab from Selection…"), Action::CreatePrefab)),
        Some((en!("Explode Instance"), Action::ExplodeInstances)),
        Some((en!("Open Prefab"), Action::OpenPrefab)),
        Some((en!("Group"), Action::Group)),
        Some((en!("Ungroup"), Action::Ungroup)),
        Some((en!("Hide"), Action::HideSelected)),
        Some((en!("Isolate"), Action::IsolateSelected)),
        None,
        Some((en!("CSG Subtract"), Action::CsgSubtract)),
        Some((en!("CSG Convex Merge"), Action::CsgMerge)),
        Some((en!("CSG Intersect"), Action::CsgIntersect)),
        Some((en!("Hollow"), Action::CsgHollow)),
        None,
    ];
    if !td_doc::linked::sets_touched(&state.doc.map, &state.doc.selection).is_empty() {
        let at = entries.iter().position(|e| matches!(e, Some((_, Action::Ungroup)))).map_or(0, |i| i + 1);
        entries.insert(at, Some((en!("Select Linked Groups"), Action::SelectLinkedGroups)));
        entries.insert(at + 1, Some((en!("Separate Linked Groups"), Action::UnlinkGroups)));
    }

    for entry in entries {
        match entry {
            Some((label, action)) => {
                let mut resp = ui.add_enabled(has_sel, egui::Button::new(tr(label)));
                if let Some(help) = action.help() {
                    resp = resp.on_hover_text(tr(help));
                }

                if resp.clicked() {
                    out.push(action);
                    ui.close();
                }
            }
            None => {
                ui.separator();
            }
        }
    }

    ui.add_enabled_ui(has_sel, |ui| {
        ui.menu_button(tr("Move to Layer"), |ui| {
            for (id, name) in layers {
                if ui.button(name).clicked() {
                    out.push(Action::MoveToLayer(id));
                    ui.close();
                }
            }
        });
    });
    actions.extend(out);
}

/// Puts `material` into terrain layer `layer`. A layer past the last one adds a single new layer instead of filling
/// the gap. Returns the slot that was set.
pub fn set_terrain_layer(t: &mut td_geom::Terrain, layer: usize, material: &str) -> usize {
    if layer < t.layers.len() {
        t.layers[layer].material = material.to_string();
        return layer;
    }

    if t.layers.len() >= td_geom::heightfield::MAX_LAYERS {
        let last = t.layers.len() - 1;
        t.layers[last].material = material.to_string();
        return last;
    }

    let tile = t.layers.last().map(|l| l.tile).unwrap_or(256.0);
    t.layers.push(td_geom::TerrainLayer::new(material, tile));
    t.layers.len() - 1
}

/// Where a payload dragged from another widget is over the view. `hover_pos` is always None then, egui never counts a
/// widget as hovered while another one is dragged.
fn drop_pos(response: &Response) -> Option<Pos2> {
    if !response.contains_pointer() {
        return None;
    }

    let pos = response.ctx.input(|i| i.pointer.hover_pos())?;
    Some(response.ctx.layer_transform_from_global(response.layer_id).map_or(pos, |t| t * pos))
}

fn quarter_turn(key: Key, right: DVec3, ahead: DVec3) -> Option<Action> {
    let (axis, degrees) = match key {
        Key::ArrowLeft => (DVec3::Y, -90.0),
        Key::ArrowRight => (DVec3::Y, 90.0),
        Key::ArrowUp => (ahead, 90.0),
        Key::ArrowDown => (ahead, -90.0),
        Key::PageUp => (right, 90.0),
        Key::PageDown => (right, -90.0),
        _ => return None,
    };
    let i = (0..3).max_by(|a, b| axis[*a].abs().total_cmp(&axis[*b].abs())).unwrap_or(1);
    Some(Action::Rotate { axis: i, degrees: degrees * axis[i].signum() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_arrows_turn_about_the_view_axes() {
        let (right, _, ahead) = ViewKind::Top.axes();
        assert_eq!(quarter_turn(Key::ArrowRight, right, ahead), Some(Action::Rotate { axis: 1, degrees: 90.0 }));
        assert_eq!(quarter_turn(Key::ArrowLeft, right, ahead), Some(Action::Rotate { axis: 1, degrees: -90.0 }));
        assert_eq!(quarter_turn(Key::ArrowUp, right, ahead), Some(Action::Rotate { axis: 1, degrees: -90.0 }), "the Top view looks down");
        assert_eq!(quarter_turn(Key::PageUp, DVec3::NEG_Z, ahead), Some(Action::Rotate { axis: 2, degrees: -90.0 }));
        assert_eq!(quarter_turn(Key::A, right, ahead), None);
    }

    #[test]
    fn holding_the_right_button_flies_before_the_mouse_moves() {
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, view: &mut Viewport| {
                let response = ui.allocate_rect(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.0)), Sense::click_and_drag());
                view.track_right_button(ui, &response);
            },
            Viewport::new(ViewKind::Perspective),
        );
        let at = Pos2::new(100.0, 100.0);
        harness.hover_at(at);
        harness.run();
        let button = |pressed| egui::Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed, modifiers: Default::default() };
        harness.input_mut().events.push(button(true));
        harness.run();
        assert!(harness.state().is_flying(), "the fly keys steer from the press on, not after the first mouse move");

        harness.input_mut().events.push(button(false));
        harness.run();
        assert!(!harness.state().is_flying());
    }

    #[test]
    fn a_view_knows_where_a_drag_from_another_widget_is() {
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, seen: &mut Option<Option<Pos2>>| {
                let source = ui.allocate_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(60.0, 20.0)), Sense::drag());
                source.dnd_set_drag_payload(DndPayload::Model("fir.glb".into()));
                let view = ui.allocate_rect(Rect::from_min_size(Pos2::new(0.0, 40.0), Vec2::splat(200.0)), Sense::click_and_drag());
                if view.dnd_hover_payload::<DndPayload>().is_some() {
                    *seen = Some(drop_pos(&view));
                }
            },
            None,
        );
        harness.hover_at(Pos2::new(10.0, 10.0));
        harness.run();
        harness.drag_at(Pos2::new(10.0, 10.0));
        harness.run();
        for pos in [Pos2::new(40.0, 60.0), Pos2::new(100.0, 120.0)] {
            harness.hover_at(pos);
            harness.run();
        }

        assert_eq!(*harness.state(), Some(Some(Pos2::new(100.0, 120.0))));
    }

    #[test]
    fn the_context_menu_speaks_german_and_still_sends_the_same_actions() {
        use crate::i18n::{Lang, scope};
        use egui_kittest::kittest::Queryable;
        struct Probe {
            state: EditorState,
            actions: Vec<Action>,
        }

        let mut state = EditorState::new(Default::default());
        let parent = state.insert_parent();
        let min = DVec3::new(96.0, -32.0, 8.0);
        let brush = td_geom::Brush::from_aabb(&td_core::Aabb::new(min, min + DVec3::new(32.0, 64.0, 16.0)), "dev/grey").unwrap();
        let id = state.doc.edit("Create Brush", |m, _| td_doc::ops::create_brush(m, parent, brush));
        let _de = scope(Lang::De);
        let mut harness = egui_kittest::Harness::builder().with_size(Vec2::new(400.0, 500.0)).build_ui_state(
            |ui, p: &mut Probe| {
                crate::i18n::set(Lang::De);
                context_menu(ui, &p.state, &mut p.actions);
            },
            Probe { state, actions: Vec::new() },
        );
        harness.run();
        for english in ["Rename", "Group", "Hide", "CSG Subtract", "Move to Layer", "Create Point Entity"] {
            assert!(harness.query_by_label_contains(english).is_none(), "{english} is still English");
        }

        for german in ["Umbenennen", "Gruppieren", "Ausblenden", "CSG subtrahieren", "In Ebene verschieben", "Punkt-Entität erstellen"] {
            assert!(harness.query_by_label_contains(german).is_some(), "{german} is missing");
        }

        assert!(harness.state().actions.is_empty());
        harness.state_mut().state.doc.selection.nodes.insert(id);
        harness.run();
        harness.get_by_label("Gruppieren").click();
        harness.run();
        assert_eq!(harness.state().actions, [Action::Group]);
        assert!(harness.query_by_label("Verknüpfte Gruppen auswählen").is_none(), "only a linked selection offers it");
        assert!(harness.query_by_label("Verknüpfte Gruppen trennen").is_none(), "only a linked selection offers it");

        let doc = &mut harness.state_mut().state.doc;
        doc.edit("Duplicate Linked", |m, s| {
            td_doc::ops::group_selection(m, s, "Group", parent);
            td_doc::ops::duplicate_linked(m, s, DVec3::new(0.0, 0.0, 80.0), Default::default());
        });
        doc.select(|_, s| {
            s.clear();
            s.nodes.insert(id);
        });
        assert!(doc.map.get(id).unwrap().parent != Some(parent), "the brush sits inside the linked group");
        harness.run();
        harness.get_by_label("Verknüpfte Gruppen auswählen").click();
        harness.run();
        assert_eq!(harness.state().actions, [Action::Group, Action::SelectLinkedGroups]);
        harness.get_by_label("Verknüpfte Gruppen trennen").click();
        harness.run();
        assert_eq!(harness.state().actions, [Action::Group, Action::SelectLinkedGroups, Action::UnlinkGroups]);
    }

    #[test]
    fn the_context_menu_lists_extrude_faces_for_selected_brush_faces_in_blender_only() {
        use egui_kittest::kittest::Queryable;
        struct Probe {
            state: EditorState,
            actions: Vec<Action>,
        }

        let mut state = EditorState::new(Default::default());
        let parent = state.insert_parent();
        let min = DVec3::new(96.0, -32.0, 8.0);
        let brush = td_geom::Brush::from_aabb(&td_core::Aabb::new(min, min + DVec3::new(32.0, 64.0, 16.0)), "dev/grey").unwrap();
        let id = state.doc.edit("Create Brush", |m, _| td_doc::ops::create_brush(m, parent, brush));
        let mut harness = egui_kittest::Harness::builder()
            .with_size(Vec2::new(400.0, 500.0))
            .build_ui_state(|ui, p: &mut Probe| context_menu(ui, &p.state, &mut p.actions), Probe { state, actions: Vec::new() });
        let shows = |harness: &mut egui_kittest::Harness<'_, Probe>, preset: &str, tool: ToolKind, faces: &[(NodeId, usize)], nodes: &[NodeId]| {
            let state = &mut harness.state_mut().state;
            state.prefs.keymap_preset = preset.into();
            state.tool = tool;
            state.doc.select(|_, s| {
                s.clear();
                s.faces.extend(faces.iter().copied());
                s.nodes.extend(nodes.iter().copied());
            });
            harness.run();
            harness.query_by_label("Extrude Faces").is_some()
        };

        assert!(!shows(&mut harness, "blender", ToolKind::Select, &[], &[id]), "an object selection has no faces to extrude");
        assert!(!shows(&mut harness, "blender", ToolKind::Select, &[], &[]), "nothing selected");
        assert!(!shows(&mut harness, "trenchbroom", ToolKind::Select, &[(id, 2)], &[]), "TrenchBroom leaves E unbound");
        assert!(!shows(&mut harness, "hammer", ToolKind::Select, &[(id, 2)], &[]), "Hammer leaves E unbound");
        assert!(shows(&mut harness, "blender", ToolKind::Select, &[(id, 2)], &[]));
        assert!(harness.state().actions.is_empty());
        harness.get_by_label("Extrude Faces").click();
        harness.run();
        assert_eq!(harness.state().actions, [Action::ExtrudeFaces]);

        assert!(shows(&mut harness, "blender", ToolKind::Select, &[(id, 0), (id, 4)], &[]), "several faces");
        assert!(shows(&mut harness, "blender", ToolKind::Select, &[(id, 2)], &[id]), "faces beside a node");
        assert!(shows(&mut harness, "blender", ToolKind::Mesh, &[(id, 2)], &[]));
        harness.get_by_label("Extrude Faces").click();
        harness.run();
        assert_eq!(harness.state().actions, [Action::ExtrudeFaces], "the Mesh tool owns E, the entry is disabled there");
    }

    #[test]
    fn dropping_past_the_last_terrain_layer_adds_one_layer() {
        let mut t = td_geom::Terrain::new(DVec3::ZERO, [3, 3], 32.0, "grass");
        assert_eq!(set_terrain_layer(&mut t, 3, "sand"), 1);
        assert_eq!(t.layers.iter().map(|l| l.material.as_str()).collect::<Vec<_>>(), ["grass", "sand"]);
        assert_eq!(set_terrain_layer(&mut t, 0, "moss"), 0);
        assert_eq!(t.layers[0].material, "moss");
        for m in ["a", "b", "c"] {
            set_terrain_layer(&mut t, 9, m);
        }

        assert_eq!(t.layers.len(), 4, "never more than four layers");
        assert_eq!(t.layers[3].material, "c", "a drop past four replaces the last layer");
    }

    #[test]
    fn size_labels_drop_trailing_zeros() {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        let painter = ctx.layer_painter(egui::LayerId::background());
        let rect = Rect::from_min_size(Pos2::new(20.0, 30.0), Vec2::new(600.0, 400.0));
        let b = Aabb::new(DVec3::new(-21.5, 3.0, 7.0), DVec3::new(15.8, 51.0, 19.25));
        size_labels(&painter, &Camera::new(ViewKind::Front), rect, b, Color32::WHITE, false);
        let out = ctx.end_pass();
        let texts: Vec<String> =
            out.shapes.iter().filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.galley.text().to_owned()) } else { None }).collect();
        out.drop_without_applying_deltas();
        assert_eq!(texts, ["37.3", "48"], "like the Shape Generator's fields");
    }

    #[test]
    fn opposite_size_labels_keep_clear_of_the_usual_ones() {
        let rect = Rect::from_min_size(Pos2::new(20.0, 30.0), Vec2::new(600.0, 400.0));
        let camera = Camera::new(ViewKind::Side);
        let b = Aabb::new(DVec3::new(5.0, -40.0, -90.5), DVec3::new(9.0, 61.0, 130.0));
        let screen = Rect::from_two_pos(camera.project(rect, b.min).unwrap(), camera.project(rect, b.max).unwrap());
        let labels = |opposite: bool| {
            let ctx = egui::Context::default();
            ctx.begin_pass(egui::RawInput::default());
            size_labels(&ctx.layer_painter(egui::LayerId::background()), &camera, rect, b, Color32::WHITE, opposite);
            let out = ctx.end_pass();
            let texts: Vec<(String, Rect)> = out
                .shapes
                .iter()
                .filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some((t.galley.text().to_owned(), t.visual_bounding_rect())) } else { None })
                .collect();
            out.drop_without_applying_deltas();
            texts
        };

        let (usual, opposite) = (labels(false), labels(true));
        let names = |l: &[(String, Rect)]| l.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>();
        assert_eq!(names(&usual), ["220.5", "101"]);
        assert_eq!(names(&opposite), names(&usual));
        assert!(usual[0].1.max.y <= screen.min.y && usual[1].1.min.x >= screen.max.x);
        assert!(opposite[0].1.min.y >= screen.max.y && opposite[1].1.max.x <= screen.min.x);
        for (a, b) in usual.iter().zip(&opposite) {
            assert!(!a.1.intersects(b.1), "{} overlaps", a.0);
        }
    }

    #[test]
    fn a_drawn_brush_covers_the_cells_under_both_ends() {
        assert_eq!(cell_span(10.0, 20.0, 16.0, true), (0.0, 32.0));
        assert_eq!(cell_span(20.0, 10.0, 16.0, true), (0.0, 32.0), "dragging backwards gives the same span");
        assert_eq!(cell_span(16.0, 48.0, 16.0, true), (16.0, 48.0), "ends on grid lines stay put");
        assert_eq!(cell_span(15.9999, 48.0001, 16.0, true), (16.0, 48.0), "so do ends a rounding error off them");
        assert_eq!(cell_span(3.0, 3.0, 16.0, true), (0.0, 16.0), "a click still makes one cell");
        assert_eq!(cell_span(3.0, 3.0, 16.0, false), (3.0, 19.0));
    }

    // The top view's axes: min lands left and above max on screen.
    const TOP_MIN: Pos2 = Pos2::new(300.0, 250.0);
    const TOP_MAX: Pos2 = Pos2::new(500.0, 400.0);

    fn top_side(pos: Pos2) -> Option<(DVec3, [Pos2; 2])> {
        side_under(TOP_MIN, TOP_MAX, DVec3::X, DVec3::NEG_Z, pos)
    }

    #[test]
    fn each_side_of_a_box_picks_its_normal() {
        let (lt, rb) = (Pos2::new(300.0, 250.0), Pos2::new(500.0, 400.0));
        let (rt, lb) = (Pos2::new(500.0, 250.0), Pos2::new(300.0, 400.0));
        assert_eq!(top_side(Pos2::new(300.0, 320.0)), Some((DVec3::NEG_X, [lt, lb])));
        assert_eq!(top_side(Pos2::new(503.0, 320.0)), Some((DVec3::X, [rt, rb])));
        assert_eq!(top_side(Pos2::new(400.0, 247.0)), Some((DVec3::NEG_Z, [lt, rt])));
        assert_eq!(top_side(Pos2::new(410.0, 396.0)), Some((DVec3::Z, [lb, rb])));
    }

    #[test]
    fn a_side_grabs_up_to_eight_points_away() {
        assert_eq!(top_side(Pos2::new(292.1, 320.0)).map(|s| s.0), Some(DVec3::NEG_X));
        assert_eq!(top_side(Pos2::new(292.0, 320.0)).map(|s| s.0), Some(DVec3::NEG_X));
        assert_eq!(top_side(Pos2::new(291.9, 320.0)), None);
        assert_eq!(top_side(Pos2::new(400.0, 408.0)).map(|s| s.0), Some(DVec3::Z));
        assert_eq!(top_side(Pos2::new(400.0, 408.1)), None);
        assert_eq!(top_side(Pos2::new(308.0, 320.0)).map(|s| s.0), Some(DVec3::NEG_X), "inside a big box too");
        assert_eq!(top_side(Pos2::new(308.1, 320.0)), None);
    }

    #[test]
    fn far_from_every_side_grabs_nothing() {
        assert_eq!(top_side(Pos2::new(400.0, 320.0)), None);
        assert_eq!(top_side(Pos2::new(10.0, 10.0)), None);
        assert_eq!(top_side(Pos2::new(900.0, 320.0)), None);
    }

    #[test]
    fn inside_a_thin_box_only_the_outer_thirds_grab() {
        let thin = |x: f32| side_under(Pos2::new(300.0, 250.0), Pos2::new(312.0, 400.0), DVec3::X, DVec3::NEG_Z, Pos2::new(x, 320.0)).map(|s| s.0);
        assert_eq!(thin(303.0), Some(DVec3::NEG_X));
        assert_eq!(thin(304.0), Some(DVec3::NEG_X), "a third of 12 points is still the side");
        assert_eq!(thin(305.0), None);
        assert_eq!(thin(306.0), None);
        assert_eq!(thin(307.0), None);
        assert_eq!(thin(309.0), Some(DVec3::X));
        assert_eq!(thin(293.0), Some(DVec3::NEG_X), "outside it the full eight points grab");
        assert_eq!(thin(319.0), Some(DVec3::X));
    }

    #[test]
    fn a_flipped_projection_picks_the_opposite_normal() {
        // The side view looks down -X with -Z to the right and +Y up, so min lands right of and below max.
        let side = |min: Pos2, max: Pos2, pos: Pos2| side_under(min, max, DVec3::NEG_Z, DVec3::Y, pos).map(|s| s.0);
        let (min, max) = (Pos2::new(500.0, 400.0), Pos2::new(300.0, 250.0));
        assert_eq!(side(min, max, Pos2::new(300.0, 320.0)), Some(DVec3::Z));
        assert_eq!(side(min, max, Pos2::new(500.0, 320.0)), Some(DVec3::NEG_Z));
        assert_eq!(side(min, max, Pos2::new(400.0, 250.0)), Some(DVec3::Y));
        assert_eq!(side(min, max, Pos2::new(400.0, 400.0)), Some(DVec3::NEG_Y));
        let (min, max) = (Pos2::new(300.0, 250.0), Pos2::new(500.0, 400.0));
        assert_eq!(side(min, max, Pos2::new(300.0, 320.0)), Some(DVec3::NEG_Z));
        assert_eq!(side(min, max, Pos2::new(400.0, 250.0)), Some(DVec3::NEG_Y));
        let segment = side_under(Pos2::new(500.0, 400.0), Pos2::new(300.0, 250.0), DVec3::NEG_Z, DVec3::Y, Pos2::new(300.0, 320.0)).map(|s| s.1);
        assert_eq!(segment, Some([Pos2::new(300.0, 250.0), Pos2::new(300.0, 400.0)]), "the segment is in screen order");
    }

    #[test]
    fn the_end_of_a_tall_terrain_grabs_right_up_to_its_corner() {
        use td_doc::map::NodeKind;
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let id = state.doc.map.insert(layer, NodeKind::Terrain(td_geom::Terrain::new(DVec3::ZERO, [8, 8], 32.0, "")));
        state.doc.select(|_, s| _ = s.nodes.insert(id));
        state.terrain_heights.set(Some((id, state.doc.revision, [0.0, 128.0])));
        let mut view = Viewport::new(ViewKind::Front);
        view.rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        (view.camera.center, view.camera.zoom) = (DVec3::new(128.0, 64.0, 0.0), 1.0);
        let corner = view.camera.project(view.rect, DVec3::new(0.0, 128.0, 0.0)).unwrap();
        let normal = |dx: f32, dy: f32| view.terrain_edge_under(corner + Vec2::new(dx, dy), &state).map(|e| e.1);
        assert_eq!(normal(3.0, 1.0), Some(DVec3::NEG_X), "the top is nearer, but heights are not dragged");
        assert_eq!(normal(-3.0, -1.0), Some(DVec3::NEG_X));
        assert_eq!(normal(128.0, 1.0), None, "the top itself grabs nothing");
    }

    /// A Side view 800 by 600 at zoom 2 looking at wedge W, min (-40, 8, 24) max (24, 72, 152), whose slanted side
    /// lies on 2y + z = 168.
    fn wedge_in_side_view() -> (EditorState, Viewport, NodeId) {
        let mut state = EditorState::new(Default::default());
        let w = add_wedge(&mut state, DVec3::new(-40.0, 8.0, 24.0), DVec3::new(24.0, 72.0, 152.0));
        let mut view = Viewport::new(ViewKind::Side);
        view.rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        (view.camera.center, view.camera.zoom) = (DVec3::new(0.0, 40.0, 88.0), 2.0);
        (state, view, w)
    }

    fn add_wedge(state: &mut EditorState, min: DVec3, max: DVec3) -> NodeId {
        let layer = state.doc.map.default_layer();
        let brush = td_geom::shapes::wedge(&Aabb::new(min, max), "dev/grey").unwrap();
        state.doc.map.insert(layer, td_doc::map::NodeKind::Brush(brush))
    }

    fn face_along(state: &EditorState, id: NodeId, normal: DVec3) -> usize {
        state.doc.map.brush(id).unwrap().faces.iter().position(|f| (f.plane.normal - normal.normalize()).length() < 1e-9).unwrap()
    }

    fn grabbed(view: &Viewport, state: &EditorState, pos: Pos2) -> Option<(Vec<(NodeId, usize)>, DVec3)> {
        view.edge_under_cursor(pos, state).map(|g| (g.faces, g.normal))
    }

    const SLANT: DVec3 = DVec3::new(0.0, 2.0, 1.0);

    #[test]
    fn a_slanted_side_seen_edge_on_grabs_along_its_own_normal() {
        let (mut state, view, w) = wedge_in_side_view();
        state.doc.select(|_, s| _ = s.nodes.insert(w));
        let at = |p: DVec3| view.camera.project(view.rect, p).unwrap();
        let mid = at(DVec3::new(-8.0, 40.0, 88.0));
        let grab = view.edge_under_cursor(mid, &state).unwrap();
        assert_eq!(grab.faces, vec![(w, face_along(&state, w, SLANT))]);
        assert!((grab.normal - SLANT.normalize()).length() < 1e-12, "{}", grab.normal);
        let (toe, apex) = (at(DVec3::new(0.0, 8.0, 152.0)), at(DVec3::new(0.0, 72.0, 24.0)));
        assert!(grab.segment[0].distance(apex) < 1e-3 && grab.segment[1].distance(toe) < 1e-3, "{:?}", grab.segment);

        let out = Vec2::new(-1.0, -2.0).normalized();
        let faces = |p: Pos2| grabbed(&view, &state, p).map(|g| g.0);
        assert_eq!(faces(mid + out * 7.99), Some(grab.faces.clone()));
        assert_eq!(faces(mid + out * 8.01), None);
        assert_eq!(faces(mid - out * 7.9), Some(grab.faces.clone()), "W is thick enough for the whole zone inside");
        assert_eq!(faces(at(DVec3::new(0.0, 88.0 / 3.0, 200.0 / 3.0))), None, "the middle moves W");
    }

    #[test]
    fn hammer_and_several_brushes_grab_only_the_bounds_sides() {
        let (mut state, view, w) = wedge_in_side_view();
        state.doc.select(|_, s| _ = s.nodes.insert(w));
        state.prefs.keymap_preset = "hammer".into();
        let at = |p: DVec3| view.camera.project(view.rect, p).unwrap();
        let mid = at(DVec3::new(0.0, 40.0, 88.0));
        assert_eq!(grabbed(&view, &state, mid), None, "Hammer moves a brush by its slanted wireframe");
        let back = face_along(&state, w, DVec3::NEG_Z);
        assert_eq!(grabbed(&view, &state, at(DVec3::new(0.0, 40.0, 22.0))), Some((vec![(w, back)], DVec3::NEG_Z)));

        state.prefs.keymap_preset = "trenchbroom".into();
        let layer = state.doc.map.default_layer();
        let cube = Brush::from_aabb(&Aabb::new(DVec3::new(40.0, 0.0, 40.0), DVec3::new(72.0, 24.0, 104.0)), "dev/grey").unwrap();
        let b = state.doc.map.insert(layer, td_doc::map::NodeKind::Brush(cube));
        state.doc.select(|_, s| _ = s.nodes.insert(b));
        assert_eq!(grabbed(&view, &state, mid), None, "two brushes grab their bounds only");
        let bottom = face_along(&state, b, DVec3::NEG_Y);
        assert_eq!(grabbed(&view, &state, at(DVec3::new(0.0, -2.0, 88.0))), Some((vec![(b, bottom)], DVec3::NEG_Y)));
    }

    #[test]
    fn a_face_selection_grabs_only_its_own_edge_on_faces() {
        let (mut state, view, w) = wedge_in_side_view();
        let at = |p: DVec3| view.camera.project(view.rect, p).unwrap();
        let (slant, bottom, side) = (face_along(&state, w, SLANT), face_along(&state, w, DVec3::NEG_Y), face_along(&state, w, DVec3::X));
        let (mid, floor, back) = (at(DVec3::new(0.0, 40.0, 88.0)), at(DVec3::new(0.0, 8.0, 88.0)), at(DVec3::new(0.0, 40.0, 24.0)));
        let spots = [mid, floor, back, at(DVec3::new(0.0, 8.0, 152.0)), at(DVec3::new(0.0, 72.0, 24.0)), at(DVec3::new(0.0, 30.0, 60.0))];
        let select = |state: &mut EditorState, faces: &[(NodeId, usize)]| {
            state.doc.select(|_, s| {
                s.clear();
                s.faces.extend(faces.iter().copied());
            })
        };

        assert!(spots.iter().all(|&p| grabbed(&view, &state, p).is_none()), "nothing selected");

        select(&mut state, &[(w, slant)]);
        assert_eq!(grabbed(&view, &state, mid).map(|g| g.0), Some(vec![(w, slant)]));
        assert_eq!(grabbed(&view, &state, floor), None, "an unselected face of the brush stays put");

        select(&mut state, &[(w, bottom)]);
        assert_eq!(grabbed(&view, &state, mid), None);
        assert_eq!(grabbed(&view, &state, floor), Some((vec![(w, bottom)], DVec3::NEG_Y)));

        select(&mut state, &[(w, side)]);
        assert!(spots.iter().all(|&p| grabbed(&view, &state, p).is_none()), "a face looking at the view is no side");

        let layer = state.doc.map.default_layer();
        let mesh = td_geom::Mesh::from_brush(state.doc.map.brush(w).unwrap());
        let m = state.doc.map.insert(layer, td_doc::map::NodeKind::Mesh(mesh));
        select(&mut state, &[(m, 0), (m, 1), (m, 2)]);
        assert!(spots.iter().all(|&p| grabbed(&view, &state, p).is_none()), "mesh faces are not resized here");
    }

    #[test]
    fn coplanar_selected_faces_are_grabbed_together() {
        let (mut state, view, w) = wedge_in_side_view();
        let w2 = add_wedge(&mut state, DVec3::new(56.0, 40.0, 24.0), DVec3::new(88.0, 72.0, 88.0));
        let (slant, slant2, bottom) = (face_along(&state, w, SLANT), face_along(&state, w2, SLANT), face_along(&state, w, DVec3::NEG_Y));
        state.doc.select(|_, s| s.faces.extend([(w, slant), (w2, slant2), (w, bottom)]));
        let shared = view.camera.project(view.rect, DVec3::new(0.0, 56.0, 56.0)).unwrap();
        assert_eq!(grabbed(&view, &state, shared).map(|g| g.0), Some(vec![(w, slant), (w2, slant2)]));
    }

    #[test]
    fn at_the_apex_the_nearer_side_wins() {
        let (mut state, view, w) = wedge_in_side_view();
        state.doc.select(|_, s| _ = s.nodes.insert(w));
        let apex = view.camera.project(view.rect, DVec3::new(0.0, 72.0, 24.0)).unwrap();
        let (out, down_slope) = (Vec2::new(-1.0, -2.0).normalized(), Vec2::new(-2.0, 1.0).normalized());
        let faces = |p: Pos2| grabbed(&view, &state, p).map(|g| g.0);
        assert_eq!(faces(apex + down_slope * 4.0 + out * 3.0), Some(vec![(w, face_along(&state, w, SLANT))]));
        assert_eq!(faces(apex + Vec2::new(2.0, 2.0)), Some(vec![(w, face_along(&state, w, DVec3::NEG_Z))]));
    }

    #[test]
    fn a_thin_slanted_brush_keeps_its_middle_for_moving() {
        let (mut state, view, _) = wedge_in_side_view();
        let corners = [(0.0, 0.0), (0.0, 12.0), (64.0, 32.0), (64.0, 44.0)];
        let points: Vec<DVec3> = [-16.0, 16.0].into_iter().flat_map(|x| corners.map(|(y, z)| DVec3::new(x, y, z))).collect();
        let layer = state.doc.map.default_layer();
        let slab = state.doc.map.insert(layer, td_doc::map::NodeKind::Brush(Brush::from_points(&points, &[], "dev/grey").unwrap()));
        state.doc.select(|_, s| _ = s.nodes.insert(slab));
        // 10.7 units thick, 21.5 points at zoom 2, so the zone inside each slanted side is 7.2 points deep.
        let mid = view.camera.project(view.rect, DVec3::new(0.0, 32.0, 28.0)).unwrap();
        let out = Vec2::new(-2.0, 1.0).normalized();
        let side = Some(vec![(slab, face_along(&state, slab, DVec3::new(0.0, -1.0, 2.0)))]);
        let faces = |p: Pos2| grabbed(&view, &state, p).map(|g| g.0);
        assert_eq!(faces(mid + out * 7.9), side);
        assert_eq!(faces(mid - out * 7.0), side);
        assert_eq!(faces(mid - out * 7.5), None);
    }

    #[test]
    fn side_zones_end_exactly_at_their_limits() {
        let mut segment = [Pos2::new(100.0, 100.0), Pos2::new(100.0, 200.0)];
        segment.sort_by(|a, b| a.to_vec2().dot(Vec2::X.rot90()).total_cmp(&b.to_vec2().dot(Vec2::X.rot90())));
        let side = |thickness: f32| [FaceSide { segment, screen_normal: Vec2::X, thickness }];
        let grabs = |thickness: f32, x: f32, y: f32| nearest_side(Pos2::new(x, y), &side(thickness)).is_some();
        assert!(grabs(30.0, 108.0, 150.0) && !grabs(30.0, 108.01, 150.0), "outside");
        assert!(grabs(30.0, 92.0, 150.0) && !grabs(30.0, 91.99, 150.0), "inside a thick brush");
        assert!(grabs(21.0, 93.0, 150.0) && !grabs(21.0, 92.99, 150.0), "inside a thin brush, a third of it");
        assert!(grabs(30.0, 100.0, 92.0) && !grabs(30.0, 100.0, 91.99), "before the first end");
        assert!(grabs(30.0, 100.0, 208.0) && !grabs(30.0, 100.0, 208.01), "past the last end");
        assert_eq!(nearest_side(Pos2::new(100.0, 150.0), &[]), None);
    }

    #[test]
    fn moved_brushes_moves_all_or_none() {
        let (mut state, _, w) = wedge_in_side_view();
        let w2 = add_wedge(&mut state, DVec3::new(56.0, 40.0, 24.0), DVec3::new(88.0, 72.0, 88.0));
        let both = [(w, face_along(&state, w, SLANT)), (w2, face_along(&state, w2, SLANT))];
        let n = SLANT.normalize();
        let slant_dist = |b: &Brush| b.faces.iter().find(|f| (f.plane.normal - n).length() < 1e-9).unwrap().plane.dist;
        let before = 168.0 / 5f64.sqrt();
        for dist in [-16.0, 16.0] {
            let moved = moved_brushes(&state.doc.map, &both, n, dist, false).unwrap();
            assert_eq!(moved.iter().map(|m| m.0).collect::<Vec<_>>(), vec![w, w2]);
            assert!(moved.iter().all(|(_, b)| (slant_dist(b) - (before + dist)).abs() < 1e-9 && b.faces.len() == 5), "{dist}");
        }

        assert!(moved_brushes(&state.doc.map, &both, n, -32.0, false).is_none(), "W2's corner is 28.6 inside, so neither moves");
        assert!(moved_brushes(&state.doc.map, &both[..1], n, -32.0, false).is_some());
        assert!(moved_brushes(&state.doc.map, &both[..1], n, -64.0, false).is_none(), "W's corner is 57.2 inside");
        assert!(moved_brushes(&state.doc.map, &[], n, -64.0, false).is_some_and(|m| m.is_empty()));
    }

    #[test]
    fn the_resize_cursor_follows_the_side_on_screen() {
        let deg = |a: f32| resize_cursor(Vec2::angled(a.to_radians()) * 3.0);
        assert_eq!(resize_cursor(Vec2::new(1.0, 0.0)), CursorIcon::ResizeHorizontal);
        assert_eq!(resize_cursor(Vec2::new(0.0, 1.0)), CursorIcon::ResizeVertical);
        assert_eq!(resize_cursor(Vec2::new(0.7, -0.7)), CursorIcon::ResizeNeSw, "up and right on screen");
        assert_eq!(resize_cursor(Vec2::new(0.7, 0.7)), CursorIcon::ResizeNwSe);
        for (a, icon) in [
            (22.4, CursorIcon::ResizeHorizontal),
            (22.6, CursorIcon::ResizeNwSe),
            (-22.4, CursorIcon::ResizeHorizontal),
            (-22.6, CursorIcon::ResizeNeSw),
            (180.0 + 22.4, CursorIcon::ResizeHorizontal),
            (90.0 - 22.4, CursorIcon::ResizeVertical),
            (90.0 - 22.6, CursorIcon::ResizeNwSe),
            (-90.0 + 22.6, CursorIcon::ResizeNeSw),
        ] {
            assert_eq!(deg(a), icon, "{a} degrees");
        }
    }

    #[test]
    fn a_side_within_a_hair_of_an_axis_takes_the_axis() {
        assert_eq!(exact_axis(DVec3::new(1e-7, -1.0, 0.0).normalize()), DVec3::NEG_Y);
        let off = DVec3::new(1e-5, 1.0, 0.0).normalize();
        assert_eq!(exact_axis(off), off);
        assert_eq!(exact_axis(SLANT.normalize()), SLANT.normalize());
    }

    #[test]
    fn a_hit_counts_as_selected_when_it_or_a_group_around_it_is() {
        let mut state = EditorState::new(Default::default());
        let layer = state.insert_parent();
        let (crate_, lid, side, loose) = state.doc.edit("add", |m, s| {
            let cube = |m: &mut td_doc::Map, min: DVec3, size: DVec3| {
                td_doc::ops::create_brush(m, layer, td_geom::Brush::from_aabb(&td_core::Aabb::new(min, min + size), "dev/grey").unwrap())
            };
            let lid = cube(m, DVec3::new(96.0, -32.0, 8.0), DVec3::new(32.0, 64.0, 16.0));
            let side = cube(m, DVec3::new(140.0, -20.0, 30.0), DVec3::new(8.0, 40.0, 72.0));
            let loose = cube(m, DVec3::new(-300.0, 50.0, -12.0), DVec3::new(24.0, 24.0, 56.0));
            s.clear();
            s.nodes.extend([lid, side]);
            let crate_ = td_doc::ops::group_selection(m, s, "crate", layer).unwrap();
            s.clear();
            (crate_, lid, side, loose)
        });
        let selected = |state: &EditorState| [crate_, lid, side, loose].map(|id| is_selected(state, id));
        assert_eq!(selected(&state), [false; 4]);

        state.doc.select(|_, s| s.select_node(lid));
        assert_eq!(selected(&state), [false, true, false, false], "a brush selected by id inside a closed group still drags");

        state.doc.select(|_, s| {
            s.clear();
            s.select_node(crate_);
        });
        assert_eq!(selected(&state), [true, true, true, false]);

        state.doc.select(|_, s| {
            s.clear();
            s.select_node(loose);
        });
        assert_eq!(selected(&state), [false, false, false, true]);
    }

    #[test]
    fn a_copy_chord_on_a_side_copies_even_just_past_the_brush() {
        use egui::Modifiers;
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let cube = Brush::from_aabb(&Aabb::new(DVec3::new(160.0, 0.0, -32.0), DVec3::new(224.0, 64.0, 32.0)), "dev/grey").unwrap();
        let id = state.doc.map.insert(layer, td_doc::map::NodeKind::Brush(cube));
        state.doc.select(|_, s| _ = s.nodes.insert(id));
        let mut view = Viewport::new(ViewKind::Top);
        view.rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        (view.camera.center, view.camera.zoom) = (DVec3::new(192.0, 32.0, 0.0), 1.0);
        let at = |x: f64| view.camera.project(view.rect, DVec3::new(x, 32.0, 0.0)).unwrap();
        let begin = |state: &mut EditorState, preset: &str, x: f64, modifiers: Modifiers| {
            state.prefs.keymap_preset = preset.into();
            let drag = view.begin_edge_drag(at(x), modifiers, state);
            state.doc.commit();
            drag
        };

        for (preset, chord) in [("hammer", Modifiers::SHIFT), ("hammer", Modifiers::COMMAND), ("trenchbroom", Modifiers::COMMAND)] {
            for x in [220.0, 224.0, 228.0] {
                let Some(Drag::Move { start, plane, axis: None, duplicate: true, .. }) = begin(&mut state, preset, x, chord) else {
                    panic!("{preset} {chord:?} at {x} does not copy");
                };
                let ray = view.camera.ray(view.rect, at(x) + Vec2::new(32.0, 0.0));
                let delta = ray.at(ray.intersect_plane(&plane).unwrap()) - start;
                assert!(td_core::vec_approx_eq(delta, DVec3::new(32.0, 0.0, 0.0)), "{preset} {chord:?} at {x}: {delta}");
            }
        }

        let resize = begin(&mut state, "hammer", 228.0, Modifiers::NONE);
        assert!(matches!(resize, Some(Drag::FaceResize { normal, .. }) if normal == DVec3::X), "a plain drag still resizes");
        assert!(begin(&mut state, "hammer", 240.0, Modifiers::SHIFT).is_none(), "past the grab zone a selection box starts");
        state.tool = ToolKind::Block;
        assert!(begin(&mut state, "hammer", 228.0, Modifiers::SHIFT).is_none(), "the Block tool draws instead");
    }
}
