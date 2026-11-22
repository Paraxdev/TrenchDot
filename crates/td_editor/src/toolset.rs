use egui::{Align2, Color32, FontId, Key, PointerButton, Pos2, Rect, Response, Stroke, Ui, Vec2};
use td_core::{Aabb, DMat4, DVec3, NodeId, Plane, Ray};
use td_doc::map::Map;
use td_doc::ops;
use td_doc::selection::Selection;
use td_geom::Brush;
use td_render::LineVertex;

use crate::box_select::BoxOp;
use crate::camera::{Camera, ViewKind};
use crate::i18n::{Msg, en, msg, msg_n, tr, trf};
use crate::picking;
use crate::scene::v3;
use crate::selection_box::SelectionBox;
use crate::state::EditorState;
use crate::theme::LINKED_EDGE;
use crate::tools::ToolKind;
use crate::widgets::corner_hint;

const HANDLE_RADIUS: f32 = 7.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipSide {
    Front,
    Back,
    Both,
}

/// Splits the selected brushes by `plane` as one undo step, keeping the part in front of it (where the normal points),
/// behind it or both, and selects what is left.
pub fn clip_selection(state: &mut EditorState, plane: &Plane, side: ClipSide) {
    let brushes = state.doc.selection.brushes(&state.doc.map);
    state.replaced = state.doc.edit("Clip", |m, s| {
        let mut replaced = td_doc::ops::Replaced::new();
        s.clear();
        for id in brushes {
            let Some(b) = m.brush(id).cloned() else { continue };
            let template = b.planes();
            let cap = td_geom::brush::best_face_data(&plane.flipped(), &template).cloned().unwrap_or_default();
            let (front, back) = b.split(plane, &cap);
            let parent = m.get(id).and_then(|n| n.parent).unwrap_or(m.default_layer());
            let label = m.get(id).and_then(|n| n.label.clone());
            m.remove(id);
            let keep: Vec<Brush> = match side {
                ClipSide::Front => front.into_iter().collect(),
                ClipSide::Back => back.into_iter().collect(),
                ClipSide::Both => front.into_iter().chain(back).collect(),
            };
            let new_ids: Vec<_> = keep.into_iter().map(|k| m.insert_labeled(parent, td_doc::NodeKind::Brush(k), label.clone())).collect();
            s.nodes.extend(new_ids.iter().copied());
            replaced.insert(id, new_ids);
        }

        replaced
    });
}

#[derive(Default)]
pub struct ClipTool {
    pub points: Vec<DVec3>,
    side: Option<ClipSide>,
    dragging: Option<usize>,
    /// Extrusion direction used to make a plane from only two points.
    second_axis: Option<DVec3>,
}

#[derive(Default)]
pub struct VertexTool {
    pub selected: Vec<DVec3>,
    drag: Option<VertexDrag>,
    /// Vertex (or edge/face point) that has a precise-move gizmo, set by double clicking it.
    gizmo: Option<DVec3>,
    gizmo_drag: Option<VGizmoDrag>,
    /// Where the running drag started and where the refused move would have put it, drawn as a red ghost.
    refused: Option<(DVec3, DVec3)>,
    /// The Hammer preset's box over the corners, drawn by a drag off the handles in a 2D view.
    boxing: Option<SelectionBox>,
    /// Box Select's box over the corners, drawn in any view.
    screen_box: Option<CornerBox>,
}

/// A box on the screen of the view at `view`, from `start` to `end`.
#[derive(Clone, Copy, Debug)]
struct CornerBox {
    view: Rect,
    start: Pos2,
    end: Pos2,
    op: BoxOp,
}

struct VertexDrag {
    start: DVec3,
    plane: Plane,
    /// A 3D dot only slides along this line through `start`, see [`pull_along`].
    line: Option<DVec3>,
    base: Vec<DVec3>,
    /// Alt only moves a vertex vertically, a dot keeps its own plane.
    is_dot: bool,
}

/// An edge midpoint or face center of a selected brush. Pulling it away from the brush adds a vertex there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VertexDot {
    pub pos: DVec3,
    /// Normals of the faces it lies on, the same one twice for a face center.
    normals: [DVec3; 2],
}

impl VertexDot {
    /// Direction that leaves the brush: the face normal, or the normalized sum of both normals on an edge.
    pub fn out(&self) -> DVec3 {
        (self.normals[0] + self.normals[1]).try_normalize().unwrap_or(self.normals[0])
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Handle {
    Vertex(DVec3),
    Dot(VertexDot),
}

impl Handle {
    fn pos(&self) -> DVec3 {
        match self {
            Handle::Vertex(v) => *v,
            Handle::Dot(d) => d.pos,
        }
    }
}

/// Half the side of a vertex square in pixels, the size it draws at when hot.
const VERTEX_SQUARE: f32 = 5.0;

/// A dot whose outward direction has less than this length in the view plane is hidden in a 2D view, it can only move
/// along its face there.
const DOT_MIN_SIDEWAYS: f64 = 0.2;

const SELECTED_RED: Color32 = Color32::from_rgb(255, 60, 40);

const DENT_REASON: &str = en!("Pull it outward, brushes cannot have dents.");
const REFUSED_REASON: &str = en!("That move would break the brush, try another spot.");

struct VGizmoDrag {
    axis: usize,
    start: DVec3,
    plane: Plane,
    base: DVec3,
}

#[derive(Default)]
pub struct RotateTool {
    drag: Option<RotateDrag>,
    hover_axis: Option<usize>,
    pub snap_degrees: f64,
}

struct RotateDrag {
    axis: DVec3,
    center: DVec3,
    start_vec: DVec3,
    screen_start: Pos2,
}

#[derive(Default)]
pub struct ScaleTool {
    drag: Option<ScaleDrag>,
}

struct ScaleDrag {
    base: Aabb,
    handle: DVec3,
    plane: Plane,
    start: DVec3,
}

#[derive(Default)]
pub struct ShearTool {
    drag: Option<ShearDrag>,
    hover: Option<ShearSide>,
}

/// A side of the selection's bounds, with a point on it to anchor the drag plane.
#[derive(Clone, Copy)]
struct ShearSide {
    axis: usize,
    max: bool,
    anchor: DVec3,
}

struct ShearDrag {
    base: Aabb,
    side: ShearSide,
    plane: Plane,
    start: DVec3,
    matrix: DMat4,
}

/// Pixels from a side in a 2D view that still picks it.
const SHEAR_PICK_PIXELS: f32 = 10.0;

/// The Sculpt brush ring color in linear space.
pub const SCULPT_RING_COLOR: [f32; 4] = [0.4, 1.0, 0.6, 0.9];

#[derive(Default)]
pub struct BrushStrokeTool {
    /// Surface point and normal under the cursor.
    pub hover: Option<(DVec3, DVec3)>,
    stroking: bool,
    /// The viewport the stroke started in.
    view: Option<egui::Id>,
    last_dab: Option<DVec3>,
    dabs: u32,
    /// Seconds the current stroke has been held.
    held: f64,
}

/// A click or a quick flick is topped up to this much hold time on release, so a single dab always shows.
const MIN_STROKE_SECONDS: f64 = 0.25;

/// Units a raise or lower stroke moves the brush centre per second of holding, per point of strength. It does not
/// depend on the radius, so resizing mid stroke keeps the speed, like the MCP `sculpt` op whose strength is one dab's
/// height whatever the radius.
const SCULPT_UNITS_PER_SECOND: f64 = 8.0;

/// The brush one sculpt dab applies for `seconds` of holding.
pub fn timed_sculpt_brush(mut brush: td_doc::terrain::SculptBrush, seconds: f64) -> td_doc::terrain::SculptBrush {
    brush.strength = if brush.mode.is_paint() { (brush.strength * seconds * 2.0).min(1.0) } else { brush.strength * seconds * SCULPT_UNITS_PER_SECOND };
    brush
}

#[derive(Default)]
pub struct ToolSet {
    pub clip: ClipTool,
    pub polygon: crate::polygon_tool::PolygonTool,
    pub vertex: VertexTool,
    pub rotate: RotateTool,
    pub scale: ScaleTool,
    pub shear: ShearTool,
    pub stroke: BrushStrokeTool,
    pub mesh: crate::mesh_tool::MeshTool,
    pub scatter: crate::scatter_tool::ScatterTool,
    pub blend: crate::blend_tool::BlendTool,
    pub volume: crate::volume_tool::VolumeTool,
    pub path: crate::extra_tools::PathTool,
    pub measure: crate::extra_tools::MeasureTool,
    pub texture: crate::texture_tool::TextureTool,
    pub extrude: crate::face_extrude::FaceExtrude,
    active: Option<ToolKind>,
    /// A tool was switched away from mid drag with its undo transaction open. `sync` cannot reach the document,
    /// so `settle` commits it on the next call that can.
    orphaned: bool,
    /// Set by the app while a viewport's right mouse look runs, read and cleared by `keys`.
    pub flying: bool,
    /// Bounds of the last brush drawn in a view, 2D views give a new brush its depth.
    pub last_brush: Option<Aabb>,
    partners: std::cell::RefCell<PartnerCache>,
}

fn screen_dist(cam: &Camera, rect: Rect, world: DVec3, pos: Pos2) -> f32 {
    cam.project(rect, world).map(|p| (p - pos).length()).unwrap_or(f32::MAX)
}

/// Outline of the object under the pointer in the outliner, so a row can be matched to what it is in the views.
fn outliner_hover_lines(state: &EditorState, id: NodeId, out: &mut Vec<LineVertex>) {
    let map = &state.doc.map;
    if map.layers.contains(&id) {
        return;
    }

    let mut drew = false;
    for n in std::iter::once(id).chain(map.descendants(id)) {
        if let Some(b) = map.brush(n) {
            for (a, c) in b.edges() {
                line(out, b.vertices[a as usize], b.vertices[c as usize], HOVER_COLOR);
            }

            drew = true;
        }
    }

    let bounds = map.bounds(id);
    if !drew && !bounds.is_empty() {
        let c = bounds.corners();
        for (i, j) in Aabb::EDGES {
            line(out, c[i], c[j], HOVER_COLOR);
        }
    }
}

/// What [`ToolSet::linked_partner_lines`] drew for one document state and selection, so the walk over every node runs
/// once per change rather than once per view per frame.
#[derive(Default)]
struct PartnerCache {
    key: Option<(u64, u64, u64)>,
    selection: Selection,
    geometry: Vec<NodeId>,
}

fn linked_partner_geometry(map: &Map, sel: &Selection) -> Vec<NodeId> {
    let touched = td_doc::linked::sets_touched(map, sel);
    if touched.is_empty() {
        return Vec::new();
    }

    let sets = td_doc::linked::link_sets(map);
    let selected: std::collections::BTreeSet<NodeId> = sel.nodes.iter().copied().chain(sel.faces.iter().map(|(id, _)| *id)).collect();
    let mut out = Vec::new();
    for member in touched.iter().filter_map(|link| sets.get(link)).flatten().copied().filter(|m| !map.is_hidden(*m)) {
        let inside: Vec<NodeId> = std::iter::once(member).chain(map.descendants(member)).collect();
        if inside.iter().any(|n| selected.contains(n)) {
            continue;
        }

        out.extend(inside.into_iter().filter(|n| !map.is_hidden(*n) && (map.brush(*n).is_some() || map.mesh(*n).is_some())));
    }

    out
}

pub(crate) const HOVER_COLOR: [f32; 4] = [0.39, 0.76, 0.93, 1.0];
pub(crate) const PREVIEW_COLOR: [f32; 4] = [0.3, 1.0, 1.0, 0.9];

fn line(out: &mut Vec<LineVertex>, a: DVec3, b: DVec3, color: [f32; 4]) {
    out.push(LineVertex { pos: v3(a), color });
    out.push(LineVertex { pos: v3(b), color });
}

impl ToolSet {
    /// Outline of the other members of every linked set the selection touches, so a half can be matched to its partner.
    /// A member that holds a selected object is left out, the selection outline already marks it, and so is a hidden one.
    fn linked_partner_lines(&self, state: &EditorState, out: &mut Vec<LineVertex>) {
        let (map, sel) = (&state.doc.map, &state.doc.selection);
        let (uid, content, loose) = state.doc.content_key();
        // A drag moves nodes without changing which ones are partners, so only a node added or removed redoes the list.
        let key = (uid, content, if state.doc.in_transaction() { map.nodes.len() as u64 } else { loose });
        let mut cache = self.partners.borrow_mut();
        if cache.key != Some(key) || cache.selection != *sel {
            cache.geometry = linked_partner_geometry(map, sel);
            cache.key = Some(key);
            cache.selection = sel.clone();
        }

        for &n in &cache.geometry {
            if let Some(b) = map.brush(n) {
                for (a, c) in b.edges() {
                    line(out, b.vertices[a as usize], b.vertices[c as usize], LINKED_EDGE);
                }
            }

            if let Some(m) = map.mesh(n) {
                for (a, c) in m.edges() {
                    line(out, m.vertices[a as usize], m.vertices[c as usize], LINKED_EDGE);
                }
            }
        }
    }

    /// Resets per-tool state when the active tool changes, and drops the Vertex tool's corners once no object is selected.
    pub fn sync(&mut self, state: &EditorState) {
        if state.doc.selection.nodes.is_empty() {
            self.vertex.selected.clear();
        }

        if self.active != Some(state.tool) {
            self.orphaned |= self.holds_transaction();
            self.clip = ClipTool::default();
            self.polygon.reset();
            self.end_drags();
            self.vertex.gizmo = None;
            if self.rotate.snap_degrees == 0.0 {
                self.rotate.snap_degrees = 15.0;
            }

            self.mesh.reset();
            self.extrude.reset();
            self.path.finish();
            self.texture.reset();
            self.volume.reset();
            self.active = Some(state.tool);
        }
    }

    /// A drag, stroke or mesh modal that edits inside an open undo transaction.
    fn holds_transaction(&self) -> bool {
        self.holds_drag() || self.mesh.holds_transaction() || self.extrude.holds_transaction()
    }

    fn holds_drag(&self) -> bool {
        self.vertex.drag.is_some()
            || self.vertex.gizmo_drag.is_some()
            || self.rotate.drag.is_some()
            || self.scale.drag.is_some()
            || self.shear.drag.is_some()
            || self.stroke.stroking
            || self.scatter.stroking()
            || self.blend.stroking()
    }

    /// Drops drags and strokes without touching the document.
    fn end_drags(&mut self) {
        self.vertex.drag = None;
        self.vertex.gizmo_drag = None;
        self.vertex.boxing = None;
        self.vertex.screen_box = None;
        self.rotate.drag = None;
        self.scale.drag = None;
        self.shear.drag = None;
        self.stroke.stroking = false;
        self.stroke.view = None;
        self.stroke.last_dab = None;
        self.scatter.reset();
        self.blend.reset();
    }

    /// Closes what a tool switch left open, and forgets the old map after a tab switch, New or Open. Runs from
    /// `keys` and the viewports, which can reach the document where `sync` cannot.
    pub fn settle(&mut self, state: &mut EditorState) {
        if state.scene_reset {
            self.reset();
        } else if std::mem::take(&mut self.orphaned) {
            state.doc.commit();
        } else if self.holds_transaction() && !state.doc.in_transaction() {
            // Something else closed the transaction (an undo mid drag), the drag must not go on editing outside it.
            self.end_drags();
            if self.mesh.holds_transaction() {
                self.mesh.reset();
            }

            self.extrude.reset();
        }
    }

    /// Forgets everything that refers to the current map, node ids restart per map. For a tab switch, New or Open,
    /// after the document was swapped: an open drag's transaction belongs to the map that was left, so the document
    /// is not touched.
    pub fn reset(&mut self) {
        self.orphaned = false;
        self.clip = ClipTool::default();
        self.polygon.reset();
        self.end_drags();
        self.vertex.selected.clear();
        self.vertex.gizmo = None;
        self.stroke.hover = None;
        self.mesh.reset();
        self.mesh.selection.clear();
        self.extrude.reset();
        self.path.finish();
        self.measure = Default::default();
        self.texture.reset();
        self.volume.reset();
    }

    /// Enter, Tab and similar keys for the active tool. Returns true if the key was consumed.
    pub fn keys(&mut self, ctx: &egui::Context, state: &mut EditorState) -> bool {
        self.settle(state);
        let flying = std::mem::take(&mut self.flying);
        if ctx.egui_wants_keyboard_input() {
            return false;
        }

        if flying {
            if self.extrude.active() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                self.extrude.cancel(state);
                return true;
            }

            // WASD, Q and E steer the camera during the right mouse look, so letters and other plain keys must not
            // reach tool shortcuts. Only Escape and Ctrl or Alt combinations get through.
            ctx.input_mut(|i| {
                i.events.retain(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => *key == Key::Escape || modifiers.command || modifiers.alt,
                    egui::Event::Text(_) => false,
                    _ => true,
                })
            });
            return false;
        }

        if self.extrude.active() {
            return self.extrude.keys(ctx, state);
        }

        if self.holds_drag() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            state.doc.cancel();
            self.end_drags();
            state.set_status(msg!("Cancelled"));
            return true;
        }

        match state.tool {
            ToolKind::Select => {
                if state.selection_box.is_some() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                    crate::selection_box::confirm(state);
                    return true;
                }
            }
            ToolKind::Clip => {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Tab)) {
                    self.clip.side = Some(match self.clip.side.unwrap_or(ClipSide::Front) {
                        ClipSide::Front => ClipSide::Back,
                        ClipSide::Back => ClipSide::Both,
                        ClipSide::Both => ClipSide::Front,
                    });
                    return true;
                }

                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                    self.apply_clip(state);
                    return true;
                }

                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Backspace)) && !self.clip.points.is_empty() {
                    self.clip.points.pop();
                    return true;
                }
            }
            ToolKind::Polygon => {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                    self.polygon.finish(state, state.polygon_mode == crate::polygon_tool::PolygonMode::Outline);
                    return true;
                }

                if !self.polygon.is_empty() {
                    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Backspace)) {
                        self.polygon.pop();
                        return true;
                    }

                    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                        self.polygon.reset();
                        state.set_status(msg!("Cancelled"));
                        return true;
                    }
                }
            }
            ToolKind::Vertex if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Delete)) && !self.vertex.selected.is_empty() => {
                self.delete_vertices(state);
                return true;
            }
            ToolKind::Mesh => return self.mesh.keys(ctx, state),
            ToolKind::Texture => return self.texture.keys(ctx, state),
            ToolKind::Path
                if self.path.last.is_some()
                    && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter) || i.consume_key(egui::Modifiers::NONE, Key::Escape)) =>
            {
                self.path.finish();
                state.set_status(msg!("Path finished"));
                return true;
            }
            _ => {}
        }

        false
    }

    pub fn viewport_input(&mut self, ui: &Ui, response: &Response, cam: &Camera, rect: Rect, hover: Option<Pos2>, state: &mut EditorState) {
        match state.tool {
            ToolKind::Clip => self.clip_input(ui, response, cam, rect, state),
            ToolKind::Polygon => self.polygon.input(response, cam, rect, hover, state, self.last_brush),
            ToolKind::Vertex => self.vertex_input(ui, response, cam, rect, state),
            ToolKind::Rotate => self.rotate_input(ui, response, cam, rect, hover, state),
            ToolKind::Scale => self.scale_input(ui, response, cam, rect, hover, state),
            ToolKind::Shear => self.shear_input(ui, response, cam, rect, hover, state),
            ToolKind::Sculpt | ToolKind::Paint => self.stroke_input(ui, response, cam, rect, hover, state),
            ToolKind::Mesh => self.mesh.viewport_input(ui, response, cam, rect, hover, state),
            ToolKind::Scatter => self.scatter.input(ui, response, cam, rect, hover, state),
            ToolKind::Blend => self.blend.input(ui, response, cam, rect, hover, state),
            ToolKind::Volume => self.volume.input(ui, response, cam, rect, state),
            ToolKind::Path => self.path.input(response, cam, rect, state),
            ToolKind::Measure => self.measure.input(ui, response, cam, rect, state),
            ToolKind::Texture => self.texture.input(ui, response, cam, rect, hover, state),
            ToolKind::Select | ToolKind::Block => {}
        }
    }

    // ------------------------------------------------------- sculpt / paint

    fn stroke_input(&mut self, ui: &Ui, response: &Response, cam: &Camera, rect: Rect, hover: Option<Pos2>, state: &mut EditorState) {
        let scope = crate::blend_tool::StrokeScope::of(state);
        let sculpting = state.tool == ToolKind::Sculpt;
        let tiles = sculpting && state.sculpt.mode.is_tile();
        let faces = if sculpting && !tiles { scope.displacements(state) } else { Vec::new() };
        let (targets, terrains) = (scope.brushes, scope.terrains);
        // Every visible view runs this with the one global pointer, only the view the stroke started in may dab.
        if self.stroke.stroking && self.stroke.view != Some(response.id) {
            return;
        }

        let pointer = if self.stroke.stroking { ui.input(|i| i.pointer.interact_pos()) } else { hover };
        self.stroke.hover = pointer.and_then(|pos| {
            let ray = cam.ray(rect, pos);
            if tiles {
                td_doc::terrain::tile_ray_cast(&state.doc.map, &terrains, &ray).map(|p| (p, DVec3::Y))
            } else if sculpting {
                let disp = td_doc::terrain::ray_cast(&state.doc.map, &faces, &ray)
                    .map(|(d, p, id, face)| (d, p, state.doc.map.brush(id).map(|b| b.faces[face].plane.normal).unwrap_or(DVec3::Y)));
                let terrain = td_doc::terrain::sculpt_ray_cast(&state.doc.map, &terrains, &ray).map(|(d, p, _)| (d, p, DVec3::Y));
                [disp, terrain].into_iter().flatten().min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, p, n)| (p, n))
            } else {
                // Vertex paint only reaches brush faces, a mesh or anything else in front shows no brush ring.
                picking::pick(state, &ray).filter(|h| h.face.is_some() && targets.contains(&h.node)).map(|h| (h.point, h.normal))
            }
        });

        let modifiers = ui.input(|i| i.modifiers);
        // A quick click can press and release within one frame, which only shows up as a click.
        if (response.drag_started_by(PointerButton::Primary)
            || response.clicked_by(PointerButton::Primary)
            || (response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed())))
            && !self.stroke.stroking
            && let Some((p, n)) = self.stroke.hover
        {
            self.stroke.stroking = true;
            self.stroke.view = Some(response.id);
            self.stroke.last_dab = None;
            self.stroke.held = 0.0;
            state.sculpt.flatten_height = p.dot(n);
            state.doc.begin(if sculpting { en!("Sculpt") } else { en!("Vertex Paint") });
            let cell = terrains.iter().filter_map(|id| state.doc.map.terrain(*id)).map(|t| t.cell_size).fold(0.0, f64::max);
            if sculpting && !tiles && state.sculpt.radius < cell {
                state.set_status(msg!(
                    "The brush radius {radius} is smaller than the terrain's {cell} unit cells. Ctrl+wheel makes it bigger",
                    radius = format!("{:.0}", state.sculpt.radius),
                    cell = format!("{cell:.0}")
                ));
            }
        }

        if self.stroke.stroking {
            let dt = ui.input(|i| i.stable_dt as f64).min(0.05);
            let released = !ui.input(|i| i.pointer.primary_down());
            let seconds = if released { dt.max(MIN_STROKE_SECONDS - self.stroke.held) } else { dt };
            self.stroke.held += dt;
            if let Some((p, _)) = self.stroke.hover {
                let spacing = state.sculpt.radius * 0.15;
                let moved_enough = self.stroke.last_dab.is_none_or(|last| (last - p).length() >= spacing);
                if moved_enough || sculpting || seconds > dt {
                    let mut brush = state.sculpt;
                    if modifiers.shift {
                        brush.mode = match brush.mode {
                            td_doc::terrain::SculptMode::Raise => td_doc::terrain::SculptMode::Lower,
                            td_doc::terrain::SculptMode::Lower => td_doc::terrain::SculptMode::Raise,
                            td_doc::terrain::SculptMode::PaintAlpha => td_doc::terrain::SculptMode::EraseAlpha,
                            td_doc::terrain::SculptMode::AddTile => td_doc::terrain::SculptMode::RemoveTile,
                            td_doc::terrain::SculptMode::RemoveTile => td_doc::terrain::SculptMode::AddTile,
                            other => other,
                        };
                    }

                    if modifiers.command && !tiles {
                        brush.mode = td_doc::terrain::SculptMode::Smooth;
                    }

                    // Continuous brushes scale with the time held so the result does not depend on frame rate.
                    self.stroke.dabs = self.stroke.dabs.wrapping_add(1);
                    let brush = td_doc::terrain::SculptBrush { seed: self.stroke.dabs, ..timed_sculpt_brush(brush, seconds) };
                    let color = state.paint_color;
                    let radius = state.sculpt.radius;
                    let strength = state.sculpt.strength;
                    let symmetry = if sculpting { state.symmetry } else { td_doc::symmetry::Symmetry::OFF };
                    state.doc.edit("Stroke", |m, _| {
                        if sculpting {
                            let mirror = symmetry.for_faces(m, &faces);
                            td_doc::terrain::sculpt(m, &faces, p, &brush, &mirror);
                            td_doc::terrain::sculpt_terrains(m, &terrains, p, &brush, &symmetry);
                        } else {
                            td_doc::terrain::paint_vertices(m, &targets, p, radius, color, (strength * seconds).min(1.0));
                        }
                    });
                    self.stroke.last_dab = Some(p);
                    ui.ctx().request_repaint();
                }
            }

            if released {
                self.stroke.stroking = false;
                self.stroke.view = None;
                state.doc.commit();
            }
        }
    }

    // ---------------------------------------------------------------- clip

    fn clip_point_at(&self, cam: &Camera, rect: Rect, pos: Pos2, state: &EditorState) -> Option<(DVec3, Option<DVec3>)> {
        match cam.kind {
            ViewKind::Perspective => {
                let hit = picking::pick(state, &cam.ray(rect, pos))?;
                Some((state.snap(hit.point), Some(hit.normal)))
            }
            k => {
                let mut p = cam.screen_to_plane(rect, pos);
                let b = state.doc.map.bounds_of(state.doc.selection.nodes.iter().copied());
                let axis = k.depth_axis();
                p[axis] = if b.is_empty() { 0.0 } else { b.center()[axis] };
                Some((state.snap(p), Some(k.axes().2)))
            }
        }
    }

    fn clip_input(&mut self, ui: &Ui, response: &Response, cam: &Camera, rect: Rect, state: &mut EditorState) {
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if response.drag_started_by(PointerButton::Primary)
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            self.clip.dragging = self.clip.points.iter().position(|p| screen_dist(cam, rect, *p, origin) < HANDLE_RADIUS);
        }

        if let (Some(i), Some(pos)) = (self.clip.dragging, pointer) {
            if response.dragged_by(PointerButton::Primary)
                && let Some((p, _)) = self.clip_point_at(cam, rect, pos, state)
            {
                self.clip.points[i] = p;
            }

            if response.drag_stopped() {
                self.clip.dragging = None;
            }

            return;
        }

        if response.clicked()
            && self.clip.points.len() < 3
            && let Some(pos) = response.interact_pointer_pos()
            && let Some((p, n)) = self.clip_point_at(cam, rect, pos, state)
        {
            if self.clip.points.is_empty() {
                self.clip.second_axis = n;
            }

            if !self.clip.points.iter().any(|q| (*q - p).length() < 1e-6) {
                self.clip.points.push(p);
            }
        }
    }

    pub fn clip_plane(&self) -> Option<Plane> {
        let pts = &self.clip.points;
        match pts.len() {
            2 => {
                let extra = pts[0] + self.clip.second_axis? * 64.0;
                Plane::from_points(pts[0], pts[1], extra)
            }
            3 => Plane::from_points(pts[0], pts[1], pts[2]),
            _ => None,
        }
    }

    pub fn apply_clip_public(&mut self, state: &mut EditorState) {
        self.apply_clip(state);
    }

    fn apply_clip(&mut self, state: &mut EditorState) {
        let Some(plane) = self.clip_plane() else {
            state.set_status(msg!("Place two or three clip points first"));
            return;
        };
        clip_selection(state, &plane, self.clip.side.unwrap_or(ClipSide::Front));
        self.clip.points.clear();
        state.set_status(msg!("Clipped"));
    }

    // -------------------------------------------------------------- vertex

    fn vertex_handles(state: &EditorState) -> (Vec<DVec3>, Vec<VertexDot>) {
        let mut verts: Vec<DVec3> = Vec::new();
        let mut dots: Vec<VertexDot> = Vec::new();
        for id in state.doc.selection.brushes(&state.doc.map) {
            let Some(b) = state.doc.map.brush(id) else { continue };
            for v in &b.vertices {
                if !verts.iter().any(|q| (*q - *v).length() < 1e-6) {
                    verts.push(*v);
                }
            }

            for (a, c) in b.edges() {
                let mut on_edge = b.faces.iter().filter(|f| f.indices.contains(&a) && f.indices.contains(&c)).map(|f| f.plane.normal);
                let Some(first) = on_edge.next() else { continue };
                let second = on_edge.next().unwrap_or(first);
                dots.push(VertexDot { pos: (b.vertices[a as usize] + b.vertices[c as usize]) * 0.5, normals: [first, second] });
            }

            for (f, face) in b.faces.iter().enumerate() {
                dots.push(VertexDot { pos: b.face_center(f), normals: [face.plane.normal; 2] });
            }
        }

        (verts, dots)
    }

    /// Selects the corners of the selected brushes inside `b` at any depth, the edge and face dots aside. With none
    /// inside the selected corners stay.
    fn box_vertices(&mut self, state: &mut EditorState, b: &SelectionBox) {
        let found: Vec<DVec3> = Self::vertex_handles(state).0.into_iter().filter(|v| b.contains_point(*v)).collect();
        if found.is_empty() {
            state.set_status(msg!("No corner lies inside the box"));
            return;
        }

        if !b.add {
            self.vertex.selected.clear();
        }

        for v in &found {
            if !self.vertex.selected.iter().any(|q| (*q - *v).length() < 1e-6) {
                self.vertex.selected.push(*v);
            }
        }

        state.set_status(msg_n!(found.len(), "Selected {n} corner", "Selected {n} corners"));
    }

    /// Box Select over the corners of the selected brushes. Without X-ray the 3D view skips corners hidden behind
    /// something.
    fn box_corners(&mut self, state: &mut EditorState, cam: &Camera, b: &CornerBox) {
        let area = Rect::from_two_pos(b.start, b.end);
        let see_through = state.prefs.box_xray || cam.kind.is_2d();
        let found: Vec<DVec3> = Self::vertex_handles(state)
            .0
            .into_iter()
            .filter(|v| cam.project(b.view, *v).is_some_and(|q| area.contains(q)))
            .filter(|v| see_through || crate::box_select::unoccluded(state, cam, b.view, *v, None))
            .collect();
        if found.is_empty() {
            state.set_status(msg!("No corner lies inside the box"));
            return;
        }

        let known = |list: &[DVec3], v: DVec3| list.iter().any(|q| (*q - v).length() < 1e-6);
        match b.op {
            BoxOp::Remove => self.vertex.selected.retain(|v| !known(&found, *v)),
            op => {
                if op == BoxOp::Replace {
                    self.vertex.selected.clear();
                }

                for v in &found {
                    if !known(&self.vertex.selected, *v) {
                        self.vertex.selected.push(*v);
                    }
                }
            }
        }

        state.set_status(match b.op {
            BoxOp::Remove => msg_n!(found.len(), "Deselected {n} corner", "Deselected {n} corners"),
            _ => msg_n!(found.len(), "Selected {n} corner", "Selected {n} corners"),
        });
    }

    fn vertex_input(&mut self, ui: &Ui, response: &Response, cam: &Camera, rect: Rect, state: &mut EditorState) {
        let modifiers = ui.input(|i| i.modifiers);
        let (verts, dots) = Self::vertex_handles(state);
        let handle_at = |pos: Pos2| handle_at(cam, rect, &verts, &dots, pos);

        if response.clicked() {
            let Some(pos) = response.interact_pointer_pos() else { return };
            match handle_at(pos) {
                Some(h) => {
                    let v = h.pos();
                    if modifiers.command {
                        if let Some(i) = self.vertex.selected.iter().position(|q| (*q - v).length() < 1e-6) {
                            self.vertex.selected.remove(i);
                        } else {
                            self.vertex.selected.push(v);
                        }
                    } else {
                        self.vertex.selected = vec![v];
                    }

                    if matches!(h, Handle::Dot(_)) {
                        state.set_status(msg!("Drag the dot away from the brush to add a vertex."));
                    }
                }
                None => {
                    if let Some(h) = picking::pick(state, &cam.ray(rect, pos)) {
                        state.leave_groups_outside(h.node);
                        let target = state.doc.map.vertex_click_target(h.node, &state.open_groups);
                        state.doc.select(|_, s| {
                            if !modifiers.command {
                                s.clear();
                            }

                            s.toggle_node(target);
                        });
                    }

                    self.vertex.selected.clear();
                    self.vertex.gizmo = None;
                }
            }
        }

        // Double clicking a vertex (or an edge/face point, which creates a vertex) puts a precise-move
        // gizmo on it. Double clicking elsewhere clears it.
        if response.double_clicked() {
            match response.interact_pointer_pos().and_then(handle_at) {
                Some(h) => {
                    self.vertex.selected = vec![h.pos()];
                    self.vertex.gizmo = Some(h.pos());
                }
                None => self.vertex.gizmo = None,
            }
        }

        if response.drag_started_by(PointerButton::Primary) {
            let Some(origin) = ui.input(|i| i.pointer.press_origin()) else { return };
            // A drag that starts on a gizmo arrow moves the vertex along just that axis.
            if let Some(g) = self.vertex.gizmo.filter(|_| cam.kind == ViewKind::Perspective)
                && let Some(i) = vgizmo_axis_at(cam, rect, g, origin)
            {
                let plane = axis_drag_plane(cam, unit_axis(i), g);
                let ray = cam.ray(rect, origin);
                if let Some(t) = ray.intersect_plane(&plane) {
                    self.vertex.selected = vec![g];
                    state.doc.begin(en!("Move Vertex"));
                    self.vertex.gizmo_drag = Some(VGizmoDrag { axis: i, start: ray.at(t), plane, base: g });
                }

                return;
            }

            if let Some(h) = handle_at(origin) {
                let v = h.pos();
                let is_dot = matches!(h, Handle::Dot(_));
                // Grabbing an unselected handle (or an edge/face point) drags just that one.
                if is_dot || !self.vertex.selected.iter().any(|q| (*q - v).length() < 1e-6) {
                    self.vertex.selected = vec![v];
                }

                let plane = vertex_drag_plane(cam, &h, modifiers.alt);
                let line = vertex_drag_line(cam, &h);
                state.doc.begin(en!("Move Vertices"));
                self.vertex.drag = Some(VertexDrag { start: v, plane, line, base: self.vertex.selected.clone(), is_dot });
            } else if state.prefs.box_select {
                self.vertex.screen_box = Some(CornerBox { view: rect, start: origin, end: origin, op: BoxOp::from_modifiers(modifiers) });
            } else if crate::mouse_map::vertex_box(&state.prefs.keymap_preset, cam.kind.is_2d()) {
                let at = cam.screen_to_plane(rect, origin);
                self.vertex.boxing = Some(SelectionBox { view: cam.kind, a: at, b: at, add: modifiers.command });
            }
        }

        if let Some(b) = self.vertex.boxing.as_mut()
            && (response.dragged_by(PointerButton::Primary) || response.drag_stopped())
            && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
        {
            b.b = cam.screen_to_plane(rect, pos);
        }

        if (response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()))
            && let Some(b) = self.vertex.boxing.take()
        {
            self.box_vertices(state, &b);
        }

        // Every view runs this with the one pointer, only the view the box started in may move or finish it.
        if let Some(b) = self.vertex.screen_box.as_mut().filter(|b| b.view == rect)
            && response.dragged_by(PointerButton::Primary)
            && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
        {
            b.end = pos;
        }

        if self.vertex.screen_box.is_some_and(|b| b.view == rect)
            && (response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()))
            && let Some(b) = self.vertex.screen_box.take()
        {
            self.box_corners(state, cam, &b);
        }

        if let Some(drag) = &self.vertex.drag {
            if response.dragged_by(PointerButton::Primary)
                && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
                && let Some(delta) = vertex_drag_delta(state, cam, rect, drag, pos, modifiers.alt)
            {
                state.doc.reset_transaction();
                let base = drag.base.clone();
                match try_move_vertices(state, &base, delta) {
                    Some(mirrored) => {
                        self.vertex.selected = base.iter().map(|b| *b + delta).collect();
                        self.clear_refusal(state);
                        if mirrored > 0 {
                            state.set_status(moved_mirrored(mirrored));
                        }
                    }
                    None => self.refuse(state, &base, drag.start, delta),
                }
            }

            if response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                self.vertex.drag = None;
                self.vertex.refused = None;
                state.doc.commit();
            }
        }

        if let Some(gd) = &self.vertex.gizmo_drag {
            let (axis, start, plane, base) = (gd.axis, gd.start, gd.plane, gd.base);
            if response.dragged_by(PointerButton::Primary)
                && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            {
                let ray = cam.ray(rect, pos);
                if let Some(t) = ray.intersect_plane(&plane) {
                    let offset = ray.at(t) - start;
                    let delta = unit_axis(axis) * state.snap_scalar(offset[axis]);
                    state.doc.reset_transaction();
                    if let Some(mirrored) = try_move_vertices(state, &[base], delta) {
                        self.vertex.selected = vec![base + delta];
                        self.vertex.gizmo = Some(base + delta);
                        self.vertex.refused = None;
                        let at = base + delta;
                        let mut status = msg!("Vertex {x} {y} {z}", x = format!("{:.3}", at.x), y = format!("{:.3}", at.y), z = format!("{:.3}", at.z));
                        if mirrored > 0 {
                            status = status.then(". ", moved_mirrored(mirrored));
                        }

                        state.set_status(status);
                    } else {
                        self.refuse(state, &[base], base, delta);
                    }
                }
            }

            if response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                self.vertex.gizmo_drag = None;
                self.vertex.refused = None;
                state.doc.commit();
            }
        }
    }

    /// Shows why a drag by `delta` was refused, nothing when it did not move at all.
    fn refuse(&mut self, state: &mut EditorState, base: &[DVec3], from: DVec3, delta: DVec3) {
        if delta.length() < 1e-9 {
            self.clear_refusal(state);
            return;
        }

        self.vertex.refused = Some((from, from + delta));
        state.set_status(Msg::key(refusal_reason(state, base, delta)));
    }

    fn clear_refusal(&mut self, state: &mut EditorState) {
        if self.vertex.refused.take().is_some() {
            state.set_status("");
        }
    }

    fn delete_vertices(&mut self, state: &mut EditorState) {
        self.vertex.gizmo = None;
        let selected = std::mem::take(&mut self.vertex.selected);
        let brushes = state.doc.selection.brushes(&state.doc.map);
        state.doc.edit("Remove Vertices", |m, _| {
            for id in brushes {
                let Some(b) = m.brush(id).cloned() else { continue };
                let pts: Vec<DVec3> = b.vertices.iter().copied().filter(|v| !selected.iter().any(|s| (*s - *v).length() < 1e-6)).collect();
                if pts.len() == b.vertices.len() {
                    continue;
                }

                if let Ok(nb) = Brush::from_points(&pts, &b.planes(), "")
                    && let Some(slot) = m.brush_mut(id)
                {
                    *slot = nb;
                }
            }
        });
    }

    // -------------------------------------------------------------- rotate

    fn rotate_center(state: &EditorState) -> Option<DVec3> {
        let b = state.doc.map.bounds_of(state.doc.selection.nodes.iter().copied());
        (!b.is_empty()).then(|| state.snap(b.center()))
    }

    fn ring_radius(cam: &Camera, center: DVec3) -> f64 {
        match cam.kind {
            ViewKind::Perspective => (cam.position - center).length() * 0.18,
            _ => 90.0 / cam.zoom,
        }
    }

    fn ring_points(center: DVec3, axis: usize, radius: f64) -> Vec<DVec3> {
        let (u, v) = match axis {
            0 => (DVec3::Y, DVec3::Z),
            1 => (DVec3::Z, DVec3::X),
            _ => (DVec3::X, DVec3::Y),
        };
        (0..=48)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / 48.0;
                center + (u * a.cos() + v * a.sin()) * radius
            })
            .collect()
    }

    fn rotate_input(&mut self, ui: &Ui, response: &Response, cam: &Camera, rect: Rect, hover: Option<Pos2>, state: &mut EditorState) {
        let Some(center) = Self::rotate_center(state) else { return };
        let radius = Self::ring_radius(cam, center);
        let axes: Vec<usize> = if cam.kind.is_2d() { vec![cam.kind.depth_axis()] } else { vec![0, 1, 2] };
        if self.rotate.drag.is_none() {
            self.rotate.hover_axis = hover.and_then(|pos| {
                axes.iter()
                    .map(|a| {
                        let pts = Self::ring_points(center, *a, radius);
                        let d = pts.windows(2).map(|w| seg_screen_dist(cam, rect, w[0], w[1], pos)).fold(f32::MAX, f32::min);
                        (d, *a)
                    })
                    .filter(|(d, _)| *d < 8.0 || cam.kind.is_2d())
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, a)| a)
            });
        }

        if response.drag_started_by(PointerButton::Primary)
            && let (Some(axis), Some(origin)) = (self.rotate.hover_axis, ui.input(|i| i.pointer.press_origin()))
        {
            let mut n = DVec3::ZERO;
            n[axis] = 1.0;
            if let Some(p) = ring_plane_point(cam, rect, origin, center, n) {
                state.doc.begin(en!("Rotate"));
                self.rotate.drag = Some(RotateDrag { axis: n, center, start_vec: (p - center).normalize_or_zero(), screen_start: origin });
            }
        }

        if let Some(drag) = &self.rotate.drag {
            // Every visible view runs this with the one global pointer, only the view being dragged may compute the angle.
            if response.dragged_by(PointerButton::Primary)
                && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            {
                if let Some(p) = ring_plane_point(cam, rect, pos, drag.center, drag.axis) {
                    let cur = (p - drag.center).normalize_or_zero();
                    let mut angle = drag.start_vec.cross(cur).dot(drag.axis).atan2(drag.start_vec.dot(cur)).to_degrees();
                    let snap = if ui.input(|i| i.modifiers.shift) { 1.0 } else { self.rotate.snap_degrees.max(1.0) };
                    angle = (angle / snap).round() * snap;
                    state.doc.reset_transaction();
                    if angle.abs() > 1e-9 {
                        let m = ops::rotation_about(drag.center, drag.axis, angle);
                        let opts = state.opts();
                        state.doc.edit("Rotate", |map, s| ops::transform_selection(map, s, &m, opts));
                    }

                    state.set_status(msg!("Rotate {angle}°", angle = format!("{angle:.1}")));
                }

                let _ = drag.screen_start;
            }

            if response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                self.rotate.drag = None;
                state.doc.commit();
            }
        }
    }

    // --------------------------------------------------------------- scale

    fn scale_handles(bounds: &Aabb, cam: &Camera) -> Vec<DVec3> {
        let mut out = Vec::new();
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let h = DVec3::new(x as f64, y as f64, z as f64);
                    if h == DVec3::ZERO {
                        continue;
                    }

                    if cam.kind.is_2d() && h[cam.kind.depth_axis()] != 0.0 {
                        continue;
                    }

                    // 3D shows face handles only to keep the view readable.
                    if !cam.kind.is_2d() && h.abs().element_sum() != 1.0 {
                        continue;
                    }

                    out.push(h);
                }
            }
        }

        let _ = bounds;
        out
    }

    fn handle_pos(bounds: &Aabb, h: DVec3) -> DVec3 {
        bounds.center() + bounds.size() * 0.5 * h
    }

    fn scale_input(&mut self, ui: &Ui, response: &Response, cam: &Camera, rect: Rect, _hover: Option<Pos2>, state: &mut EditorState) {
        let bounds = state.doc.map.bounds_of(state.doc.selection.nodes.iter().copied());
        if bounds.is_empty() {
            return;
        }

        if response.drag_started_by(PointerButton::Primary) {
            let Some(origin) = ui.input(|i| i.pointer.press_origin()) else { return };
            let best = Self::scale_handles(&bounds, cam)
                .into_iter()
                .map(|h| (screen_dist(cam, rect, Self::handle_pos(&bounds, h), origin), h))
                .filter(|(d, _)| *d < HANDLE_RADIUS * 1.5)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, h)) = best {
                let p = Self::handle_pos(&bounds, h);
                let plane = match cam.kind {
                    ViewKind::Perspective => {
                        let f = cam.forward();
                        let n = if h.y != 0.0 { DVec3::new(f.x, 0.0, f.z).normalize_or(DVec3::Z) } else { DVec3::Y };
                        Plane::from_point_normal(p, n)
                    }
                    k => Plane::from_point_normal(p, -k.axes().2),
                };
                state.doc.begin(en!("Scale"));
                self.scale.drag = Some(ScaleDrag { base: bounds, handle: h, plane, start: p });
            }
        }

        if let Some(drag) = &self.scale.drag {
            // Every visible view runs this with the one global pointer, only the view being dragged may compute the scale.
            if response.dragged_by(PointerButton::Primary)
                && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            {
                let ray = cam.ray(rect, pos);
                if let Some(t) = ray.intersect_plane(&drag.plane) {
                    let delta = ray.at(t) - drag.start;
                    let symmetric = ui.input(|i| i.modifiers.alt);
                    let mut new = drag.base;
                    for i in 0..3 {
                        if drag.handle[i] > 0.0 {
                            new.max[i] = state.snap_scalar(drag.base.max[i] + delta[i]);
                            if symmetric {
                                new.min[i] = drag.base.min[i] - (new.max[i] - drag.base.max[i]);
                            }
                        } else if drag.handle[i] < 0.0 {
                            new.min[i] = state.snap_scalar(drag.base.min[i] + delta[i]);
                            if symmetric {
                                new.max[i] = drag.base.max[i] - (new.min[i] - drag.base.min[i]);
                            }
                        }
                    }

                    let valid = (0..3).all(|i| new.max[i] - new.min[i] > 1e-3 || drag.base.size()[i] < 1e-6);
                    state.doc.reset_transaction();
                    if valid && new != drag.base {
                        let m: DMat4 = ops::scale_bounds(&drag.base, &new);
                        let opts = state.opts();
                        state.doc.edit("Scale", |map, s| ops::transform_selection(map, s, &m, opts));
                        let size = new.size();
                        state.set_status(msg!("Scale to {x} x {y} x {z}", x = size.x, y = size.y, z = size.z));
                    }
                }
            }

            if response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                self.scale.drag = None;
                state.doc.commit();
            }
        }
    }

    // --------------------------------------------------------------- shear

    /// The four corners of a side of `bounds`, flattened onto the middle of the view axis in a 2D view.
    fn shear_face(bounds: &Aabb, axis: usize, max: bool, flatten: Option<usize>) -> [DVec3; 4] {
        let mut lo = bounds.min;
        let mut hi = bounds.max;
        let at = if max { bounds.max[axis] } else { bounds.min[axis] };
        lo[axis] = at;
        hi[axis] = at;
        if let Some(d) = flatten {
            lo[d] = bounds.center()[d];
            hi[d] = lo[d];
        }

        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        let corner = |a: bool, b: bool| {
            let mut p = lo;
            p[u] = if a { hi[u] } else { lo[u] };
            p[v] = if b { hi[v] } else { lo[v] };
            p
        };
        [corner(false, false), corner(true, false), corner(true, true), corner(false, true)]
    }

    /// The side of `bounds` under `pos`: the nearest edge in a 2D view, the face the pointer lies over in 3D.
    fn shear_pick(bounds: &Aabb, cam: &Camera, rect: Rect, pos: Pos2) -> Option<ShearSide> {
        if cam.kind.is_2d() {
            let depth = cam.kind.depth_axis();
            return (0..3)
                .filter(|a| *a != depth)
                .flat_map(|axis| [false, true].map(move |max| (axis, max)))
                .map(|(axis, max)| {
                    let along = (0..3).find(|i| *i != axis && *i != depth).unwrap_or(axis);
                    let mut a = Self::shear_face(bounds, axis, max, Some(depth))[0];
                    let mut b = a;
                    a[along] = bounds.min[along];
                    b[along] = bounds.max[along];
                    (seg_screen_dist(cam, rect, a, b, pos), ShearSide { axis, max, anchor: bounds.center() })
                })
                .filter(|(d, _)| *d < SHEAR_PICK_PIXELS)
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, side)| side);
        }

        let ray = cam.ray(rect, pos);
        let mut best: Option<(f64, ShearSide)> = None;
        for axis in 0..3 {
            for max in [false, true] {
                let facing = if max { ray.dir[axis] < 0.0 } else { ray.dir[axis] > 0.0 };
                let f = Self::shear_face(bounds, axis, max, None);
                let mut normal = DVec3::ZERO;
                normal[axis] = 1.0;
                let Some(t) = ray.intersect_plane(&Plane::from_point_normal(f[0], normal)) else { continue };
                let hit = ray.at(t);
                let inside = (0..3).all(|i| i == axis || (hit[i] >= bounds.min[i] && hit[i] <= bounds.max[i]));
                if facing && t > 0.0 && inside && best.is_none_or(|(bt, _)| t < bt) {
                    best = Some((t, ShearSide { axis, max, anchor: hit }));
                }
            }
        }

        best.map(|(_, side)| side)
    }

    /// The box as it is now, the matrix being dragged, and the side to highlight.
    fn shear_view(&self, state: &EditorState) -> Option<(Aabb, DMat4, Option<ShearSide>)> {
        if let Some(d) = &self.shear.drag {
            return Some((d.base, d.matrix, Some(d.side)));
        }

        let bounds = state.doc.map.bounds_of(state.doc.selection.nodes.iter().copied());
        (!bounds.is_empty()).then_some((bounds, DMat4::IDENTITY, self.shear.hover))
    }

    fn shear_input(&mut self, ui: &Ui, response: &Response, cam: &Camera, rect: Rect, hover: Option<Pos2>, state: &mut EditorState) {
        if self.shear.drag.is_none() {
            let bounds = state.doc.map.bounds_of(state.doc.selection.nodes.iter().copied());
            self.shear.hover = hover.filter(|_| !bounds.is_empty()).and_then(|pos| Self::shear_pick(&bounds, cam, rect, pos));
        }

        if response.drag_started_by(PointerButton::Primary)
            && self.shear.drag.is_none()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            let bounds = state.doc.map.bounds_of(state.doc.selection.nodes.iter().copied());
            if !bounds.is_empty()
                && let Some(side) = Self::shear_pick(&bounds, cam, rect, origin)
            {
                let plane = if cam.kind.is_2d() {
                    Plane::from_point_normal(side.anchor, -cam.kind.axes().2)
                } else {
                    let mut normal = DVec3::ZERO;
                    normal[side.axis] = 1.0;
                    Plane::from_point_normal(side.anchor, normal)
                };
                let ray = cam.ray(rect, origin);
                if let Some(t) = ray.intersect_plane(&plane) {
                    state.doc.begin(en!("Shear"));
                    self.shear.drag = Some(ShearDrag { base: bounds, side, plane, start: ray.at(t), matrix: DMat4::IDENTITY });
                }
            }
        }

        let Some(drag) = &mut self.shear.drag else { return };
        // Every visible view runs this with the one global pointer, only the view being dragged may compute the shear.
        if response.dragged_by(PointerButton::Primary)
            && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
        {
            let ray = cam.ray(rect, pos);
            if let Some(t) = ray.intersect_plane(&drag.plane) {
                let delta = ray.at(t) - drag.start;
                let mut offset = DVec3::ZERO;
                let tangents: Vec<usize> = (0..3).filter(|a| *a != drag.side.axis && !(cam.kind.is_2d() && *a == cam.kind.depth_axis())).collect();
                let free = ui.input(|i| i.modifiers.shift);
                let dominant = tangents.iter().copied().max_by(|a, b| delta[*a].abs().total_cmp(&delta[*b].abs()));
                for a in tangents {
                    if free || Some(a) == dominant {
                        offset[a] = state.snap_scalar(delta[a]);
                    }
                }

                state.doc.reset_transaction();
                drag.matrix = DMat4::IDENTITY;
                if offset != DVec3::ZERO
                    && let Some(m) = ops::shear_side(&drag.base, drag.side.axis, drag.side.max, offset)
                {
                    let opts = state.opts();
                    state.doc.edit("Shear", |map, s| ops::shear_selection(map, s, &m, opts));
                    drag.matrix = m;
                    state.set_status(msg!("Shear by {offset}", offset = format!("{} {} {}", offset.x, offset.y, offset.z)));
                }
            }
        }

        if response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            self.shear.drag = None;
            state.doc.commit();
        }
    }

    // ------------------------------------------------------------ drawing

    pub fn lines(&self, cam: &Camera, rect: Rect, state: &EditorState) -> Vec<LineVertex> {
        let mut out = Vec::new();
        match state.tool {
            ToolKind::Clip => {
                let pts = &self.clip.points;
                for w in pts.windows(2) {
                    line(&mut out, w[0], w[1], [0.3, 1.0, 1.0, 1.0]);
                }

                if pts.len() == 3 {
                    line(&mut out, pts[2], pts[0], [0.3, 1.0, 1.0, 1.0]);
                }

                if let Some(plane) = self.clip_plane() {
                    let side = self.clip.side.unwrap_or(ClipSide::Front);
                    for id in state.doc.selection.brushes(&state.doc.map) {
                        let Some(b) = state.doc.map.brush(id) else { continue };
                        let (front, back) = b.split(&plane, &Default::default());
                        let kept = [0.3, 1.0, 0.4, 1.0];
                        let removed = [0.6, 0.6, 0.6, 0.35];
                        for (piece, keep) in [(front, side != ClipSide::Back), (back, side != ClipSide::Front)] {
                            if let Some(p) = piece {
                                for (a, c) in p.edges() {
                                    line(&mut out, p.vertices[a as usize], p.vertices[c as usize], if keep { kept } else { removed });
                                }
                            }
                        }
                    }
                }
            }
            ToolKind::Polygon => self.polygon.lines(&mut out, state),
            ToolKind::Rotate => {
                if let Some(center) = Self::rotate_center(state) {
                    let radius = Self::ring_radius(cam, center);
                    let axes: Vec<usize> = if cam.kind.is_2d() { vec![cam.kind.depth_axis()] } else { vec![0, 1, 2] };
                    for a in axes {
                        let active = self.rotate.hover_axis == Some(a) || self.rotate.drag.as_ref().is_some_and(|d| d.axis[a] == 1.0);
                        let mut color = match a {
                            0 => [1.0, 0.3, 0.3, 0.9],
                            1 => [0.3, 1.0, 0.3, 0.9],
                            _ => [0.35, 0.5, 1.0, 0.9],
                        };
                        if active {
                            color = [1.0, 1.0, 0.3, 1.0];
                        }

                        let pts = Self::ring_points(center, a, radius);
                        for w in pts.windows(2) {
                            line(&mut out, w[0], w[1], color);
                        }
                    }
                }
            }
            ToolKind::Scale => {
                let b = state.doc.map.bounds_of(state.doc.selection.nodes.iter().copied());
                if !b.is_empty() {
                    let c = b.corners();
                    for (i, j) in Aabb::EDGES {
                        line(&mut out, c[i], c[j], [1.0, 0.8, 0.2, 0.9]);
                    }
                }
            }
            ToolKind::Shear => {
                if let Some((bounds, m, side)) = self.shear_view(state) {
                    let c = bounds.corners().map(|p| m.transform_point3(p));
                    for (i, j) in Aabb::EDGES {
                        line(&mut out, c[i], c[j], [1.0, 0.8, 0.2, 0.9]);
                    }

                    if let Some(side) = side {
                        let flatten = cam.kind.is_2d().then(|| cam.kind.depth_axis());
                        let f = Self::shear_face(&bounds, side.axis, side.max, flatten).map(|p| m.transform_point3(p));
                        for k in 0..4 {
                            line(&mut out, f[k], f[(k + 1) % 4], [1.0, 1.0, 0.3, 1.0]);
                        }
                    }
                }
            }
            ToolKind::Vertex => crate::symmetry::edit_lines(state, &mut out),
            ToolKind::Mesh => {
                out.extend(self.mesh.lines(cam, rect, state));
                crate::symmetry::edit_lines(state, &mut out);
            }
            ToolKind::Scatter => self.scatter.lines(state, &mut out),
            ToolKind::Blend => {
                self.blend.lines(state, &mut out);
                crate::symmetry::lines(state, true, self.blend.hover, &mut out);
            }
            ToolKind::Volume => self.volume.lines(state, cam, &mut out),
            ToolKind::Measure => self.measure.lines(&mut out),
            ToolKind::Texture => out.extend(self.texture.lines(state)),
            ToolKind::Sculpt | ToolKind::Paint if state.tool == ToolKind::Sculpt && state.sculpt.mode.is_tile() => {
                let terrains = crate::blend_tool::StrokeScope::of(state).terrains;
                let map = &state.doc.map;
                if let Some((p, t)) =
                    self.stroke.hover.and_then(|(p, _)| td_doc::terrain::tile_terrain(map, &terrains, p).and_then(|id| map.terrain(id)).map(|t| (p, t)))
                {
                    let size = t.tile_size();
                    let [i, j] = t.tile_at(p);
                    let min = t.origin + DVec3::new(i as f64 * size, 0.0, j as f64 * size);
                    let y = DVec3::Y * (p.y - min.y + 0.5);
                    let corners = [DVec3::ZERO, DVec3::X, DVec3::new(1.0, 0.0, 1.0), DVec3::Z, DVec3::ZERO].map(|c| min + y + c * size);
                    for w in corners.windows(2) {
                        line(&mut out, w[0], w[1], SCULPT_RING_COLOR);
                    }
                }
            }
            ToolKind::Sculpt | ToolKind::Paint => {
                if let Some((p, n)) = self.stroke.hover {
                    let basis = Plane::from_point_normal(p, n).basis();
                    let r = state.sculpt.radius;
                    let color = if state.tool == ToolKind::Sculpt { SCULPT_RING_COLOR } else { [1.0, 0.6, 0.9, 0.9] };
                    let ring: Vec<DVec3> = (0..=40)
                        .map(|i| {
                            let a = std::f64::consts::TAU * i as f64 / 40.0;
                            p + (basis.0 * a.cos() + basis.1 * a.sin()) * r + n * 0.5
                        })
                        .collect();
                    for w in ring.windows(2) {
                        line(&mut out, w[0], w[1], color);
                    }

                    line(&mut out, p, p + n * r * 0.4, color);
                }

                if state.tool == ToolKind::Sculpt {
                    crate::symmetry::lines(state, false, self.stroke.hover, &mut out);
                }
            }
            _ => {}
        }

        crate::symmetry::mirror_link_lines(state, cam, &mut out);
        crate::gizmos::lines(state, &mut out);
        for b in &state.sweep_preview {
            for (a, c) in b.edges() {
                line(&mut out, b.vertices[a as usize], b.vertices[c as usize], PREVIEW_COLOR);
            }
        }

        for &(a, b) in state.shape_preview.iter().chain(&state.spline_preview).chain(&state.bevel_preview).chain(&state.roof_preview) {
            line(&mut out, a, b, PREVIEW_COLOR);
        }

        for (a, c) in &state.array_preview {
            line(&mut out, *a, *c, PREVIEW_COLOR);
        }

        crate::shape_bounds::lines(state, &mut out);
        crate::terrain_size::lines(state, &mut out);

        self.linked_partner_lines(state, &mut out);
        if let Some(id) = state.outliner_hover {
            outliner_hover_lines(state, id, &mut out);
        }

        let _ = rect;
        out
    }

    pub fn paint_overlay(&self, ui: &Ui, cam: &Camera, rect: Rect, state: &EditorState) {
        let painter = ui.painter_at(rect);
        if self.extrude.active() {
            self.extrude.paint_overlay(&painter, rect);
            return;
        }

        let hover = ui.input(|i| i.pointer.hover_pos());
        let handle = |p: Pos2, color: Color32, hot: bool| {
            let r = if hot { 5.0 } else { 4.0 };
            painter.rect_filled(Rect::from_center_size(p, Vec2::splat(r * 2.0)), 1.0, color);
            painter.rect_stroke(Rect::from_center_size(p, Vec2::splat(r * 2.0)), 1.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
        };
        match state.tool {
            ToolKind::Clip => {
                for (i, p) in self.clip.points.iter().enumerate() {
                    if let Some(sp) = cam.project(rect, *p) {
                        let hot = hover.is_some_and(|h| (h - sp).length() < HANDLE_RADIUS);
                        handle(sp, Color32::from_rgb(80, 255, 255), hot);
                        painter.text(sp + Vec2::new(8.0, -8.0), Align2::LEFT_BOTTOM, format!("{}", i + 1), FontId::monospace(11.0), Color32::WHITE);
                    }
                }

                let side = match self.clip.side.unwrap_or(ClipSide::Front) {
                    ClipSide::Front => tr("front"),
                    ClipSide::Back => tr("back"),
                    ClipSide::Both => tr("both"),
                };
                corner_hint(&painter, rect, trf!("Clip: click to place points, keep {side} (Tab), Enter to apply", side), Color32::from_rgb(120, 240, 240));
            }
            ToolKind::Polygon => self.polygon.paint_overlay(ui, cam, rect, state.polygon_mode),
            ToolKind::Vertex => {
                let (verts, dots) = Self::vertex_handles(state);
                let hot = hover.and_then(|p| handle_at(cam, rect, &verts, &dots, p));
                let is_selected = |v: DVec3| self.vertex.selected.iter().any(|s| (*s - v).length() < 1e-6);
                for d in dots.iter().filter(|d| dot_visible(cam, d)) {
                    if let Some(sp) = cam.project(rect, d.pos) {
                        let color = match (is_selected(d.pos), hot == Some(Handle::Dot(*d))) {
                            (true, _) => SELECTED_RED,
                            (false, true) => Color32::from_rgb(190, 230, 255),
                            (false, false) => Color32::from_rgb(120, 200, 255),
                        };
                        painter.circle_filled(sp, if hot == Some(Handle::Dot(*d)) { 4.0 } else { 2.5 }, color);
                    }
                }

                for v in verts {
                    if let Some(sp) = cam.project(rect, v) {
                        handle(sp, if is_selected(v) { SELECTED_RED } else { Color32::from_rgb(255, 220, 80) }, hot == Some(Handle::Vertex(v)));
                    }
                }

                if let Some((from, to)) = self.vertex.refused
                    && let (Some(a), Some(b)) = (cam.project(rect, from), cam.project(rect, to))
                {
                    painter.line_segment([a, b], Stroke::new(1.0, SELECTED_RED));
                    painter.circle_stroke(b, 6.0, Stroke::new(2.0, SELECTED_RED));
                }

                if let Some(b) = &self.vertex.boxing {
                    crate::selection_box::paint_box(&painter, cam, rect, b);
                }

                if let Some(b) = self.vertex.screen_box.filter(|b| b.view == rect) {
                    crate::selection_box::paint_rect(&painter, Rect::from_two_pos(b.start, b.end));
                }

                if let Some(g) = self.vertex.gizmo.filter(|_| cam.kind == ViewKind::Perspective) {
                    let len = vgizmo_len(cam, rect, g);
                    let hot_axis = self.vertex.gizmo_drag.as_ref().map(|d| d.axis).or_else(|| hover.and_then(|p| vgizmo_axis_at(cam, rect, g, p)));
                    for i in 0..3 {
                        let a = unit_axis(i);
                        let (Some(from), Some(head), Some(tip)) =
                            (cam.project(rect, g + a * len * 0.15), cam.project(rect, g + a * len * 0.78), cam.project(rect, g + a * len))
                        else {
                            continue;
                        };
                        let c = if hot_axis == Some(i) { crate::theme::VIEW.yellow } else { crate::theme::VIEW.axis()[i] };
                        painter.line_segment([from, head], Stroke::new(if hot_axis == Some(i) { 3.5 } else { 2.5 }, c));
                        let dir = tip - head;
                        if dir.length() > 1.0 {
                            let side = Vec2::new(-dir.y, dir.x).normalized() * 6.0;
                            painter.add(egui::Shape::convex_polygon(vec![tip, head + side, head - side], c, Stroke::NONE));
                        }
                    }

                    if let Some(sp) = cam.project(rect, g) {
                        painter.circle_filled(sp, 3.0, Color32::WHITE);
                    }
                }

                corner_hint(
                    &painter,
                    rect,
                    trf!("Vertex: drag a corner or a blue dot, double click a corner for a gizmo, Del removes"),
                    Color32::from_rgb(255, 220, 120),
                );
            }
            ToolKind::Scale => {
                let b = state.doc.map.bounds_of(state.doc.selection.nodes.iter().copied());
                if !b.is_empty() {
                    for h in Self::scale_handles(&b, cam) {
                        if let Some(sp) = cam.project(rect, Self::handle_pos(&b, h)) {
                            let hot = hover.is_some_and(|p| (p - sp).length() < HANDLE_RADIUS * 1.5);
                            handle(sp, Color32::from_rgb(255, 200, 60), hot);
                        }
                    }
                }

                corner_hint(&painter, rect, trf!("Scale: drag handles, Alt scales symmetrically"), Color32::from_rgb(255, 210, 120));
            }
            ToolKind::Shear => {
                if !cam.kind.is_2d()
                    && let Some((bounds, m, Some(side))) = self.shear_view(state)
                {
                    let f = Self::shear_face(&bounds, side.axis, side.max, None).map(|p| cam.project(rect, m.transform_point3(p)));
                    if let [Some(a), Some(b), Some(c), Some(d)] = f {
                        painter.add(egui::Shape::convex_polygon(vec![a, b, c, d], Color32::from_rgba_unmultiplied(255, 230, 80, 120), Stroke::NONE));
                    }
                }

                corner_hint(&painter, rect, trf!("Shear: drag a side of the box, Shift moves freely in 3D"), Color32::from_rgb(255, 210, 120));
            }
            ToolKind::Rotate => {
                corner_hint(
                    &painter,
                    rect,
                    trf!("Rotate: drag a ring, snaps {degrees}° (Shift for 1°)", degrees = self.rotate.snap_degrees),
                    Color32::from_rgb(200, 255, 160),
                );
            }
            ToolKind::Mesh => self.mesh.paint_overlay(ui, cam, rect, state),
            ToolKind::Measure => self.measure.paint(ui, cam, rect, state),
            ToolKind::Texture => self.texture.paint(ui, rect, state),
            ToolKind::Scatter => self.scatter.paint_overlay(ui, rect, state),
            ToolKind::Blend => self.blend.paint_overlay(ui, rect, state),
            ToolKind::Volume => self.volume.paint_overlay(ui, rect, state),
            ToolKind::Path => {
                corner_hint(
                    &painter,
                    rect,
                    trf!("Path: click to place path corners linked by target, Enter finishes the chain"),
                    Color32::from_rgb(160, 220, 255),
                );
            }
            ToolKind::Sculpt => {
                let has_terrain = state.doc.map.terrains().next().is_some();
                // An empty list is the whole map here: the hint is about the map having nothing to sculpt at all.
                let hint = if !has_terrain && td_doc::terrain::displacement_faces(&state.doc.map, &[]).is_empty() {
                    trf!("Sculpt: no terrain or displacements. Terrain > Create Terrain, or select quad faces and use Brush > Displacement")
                } else if state.sculpt.mode.is_tile() {
                    trf!("Sculpt {mode}: click adds a tile, Shift removes it", mode = tr(state.sculpt.mode.label()))
                } else {
                    trf!("Sculpt {mode}: drag to apply, Shift inverts, Ctrl smooths", mode = tr(state.sculpt.mode.label()))
                };
                corner_hint(&painter, rect, hint, Color32::from_rgb(140, 255, 170));
            }
            ToolKind::Paint => {
                corner_hint(
                    &painter,
                    rect,
                    trf!("Vertex paint: drag over brush faces (the selected brushes, or every brush when nothing is selected)"),
                    Color32::from_rgb(255, 170, 230),
                );
            }
            ToolKind::Select | ToolKind::Block => {}
        }
    }
}

/// Applies a vertex move to every selected brush containing one of the base positions.
/// Brushes where the move would swallow a moved vertex are left unchanged. Returns true if anything moved.
fn unit_axis(i: usize) -> DVec3 {
    let mut a = DVec3::ZERO;
    a[i] = 1.0;
    a
}

fn seg_dist(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

/// World length at `center` that spans about 64 screen pixels, so the vertex gizmo keeps its size.
fn vgizmo_len(cam: &Camera, rect: Rect, center: DVec3) -> f64 {
    let fallback = (cam.eye() - center).length().max(1.0) * 0.15;
    let (Some(c), Some(p)) = (cam.project(rect, center), cam.project(rect, center + cam.right())) else {
        return fallback;
    };
    let per = (p - c).length() as f64;
    if per > 1e-6 { 64.0 / per } else { fallback }
}

/// The gizmo arrow under the pointer, if any.
fn vgizmo_axis_at(cam: &Camera, rect: Rect, center: DVec3, pos: Pos2) -> Option<usize> {
    let len = vgizmo_len(cam, rect, center);
    (0..3)
        .filter_map(|i| {
            let a = unit_axis(i);
            let from = cam.project(rect, center + a * len * 0.15)?;
            let to = cam.project(rect, center + a * len)?;
            Some((seg_dist(from, to, pos), i))
        })
        .filter(|(d, _)| *d < 8.0)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, i)| i)
}

/// Plane holding `a` and facing the camera, so pointer motion maps cleanly to that direction.
fn axis_drag_plane(cam: &Camera, a: DVec3, center: DVec3) -> Plane {
    let f = cam.forward();
    let n = a.cross(f).cross(a);
    Plane::from_point_normal(center, if n.length_squared() < 1e-9 { f } else { n.normalize() })
}

/// Whether a dot can be seen and grabbed. In 3D that needs one of its faces turned to the camera, in a 2D view its
/// outward direction has to leave the view's depth axis.
pub(crate) fn dot_visible(cam: &Camera, dot: &VertexDot) -> bool {
    if cam.kind.is_2d() {
        let depth = cam.forward();
        let out = dot.out();
        (out - depth * out.dot(depth)).length() >= DOT_MIN_SIDEWAYS
    } else {
        let view = cam.eye() - dot.pos;
        dot.normals.iter().any(|n| n.dot(view) > 0.0)
    }
}

/// The handle a press at `pos` takes: a vertex when the pointer is on its square, else the closest handle in reach.
pub(crate) fn handle_at(cam: &Camera, rect: Rect, verts: &[DVec3], dots: &[VertexDot], pos: Pos2) -> Option<Handle> {
    let on_square = verts
        .iter()
        .filter_map(|v| {
            let d = cam.project(rect, *v)? - pos;
            (d.x.abs() <= VERTEX_SQUARE && d.y.abs() <= VERTEX_SQUARE).then_some((d.length(), Handle::Vertex(*v)))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, h)) = on_square {
        return Some(h);
    }

    let near_vertex = verts.iter().map(|v| (screen_dist(cam, rect, *v, pos), Handle::Vertex(*v)));
    let near_dot = dots.iter().filter(|d| dot_visible(cam, d)).map(|d| (screen_dist(cam, rect, d.pos, pos), Handle::Dot(*d)));
    near_vertex.chain(near_dot).filter(|(d, _)| *d < HANDLE_RADIUS).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, h)| h)
}

pub(crate) fn vertex_drag_plane(cam: &Camera, handle: &Handle, alt: bool) -> Plane {
    let v = handle.pos();
    match (cam.kind, handle) {
        (ViewKind::Perspective, Handle::Dot(d)) => axis_drag_plane(cam, d.out(), v),
        (ViewKind::Perspective, _) if alt => {
            let f = cam.forward();
            Plane::from_point_normal(v, DVec3::new(f.x, 0.0, f.z).normalize_or(DVec3::Z))
        }
        (ViewKind::Perspective, _) => Plane::from_point_normal(v, DVec3::Y),
        (k, _) => Plane::from_point_normal(v, -k.axes().2),
    }
}

/// The line a handle is confined to while dragged: a dot in 3D follows its outward direction.
pub(crate) fn vertex_drag_line(cam: &Camera, handle: &Handle) -> Option<DVec3> {
    match handle {
        Handle::Dot(d) if cam.kind == ViewKind::Perspective => Some(d.out()),
        _ => None,
    }
}

/// A line closer than this (as a sine) to the view direction, or to the pointer ray, reads no pull from the pointer.
const MIN_PULL_SIN: f64 = 0.25;

/// How far along `dir` from `start` the pointer at `pos` pulls, or None while the line runs too close to the
/// direction of sight for the pointer to tell. Holding still there beats a jump, a pixel is a long way along it.
fn pull_along(cam: &Camera, rect: Rect, start: DVec3, dir: DVec3, pos: Pos2) -> Option<f64> {
    if (start - cam.eye()).normalize_or_zero().cross(dir).length() < MIN_PULL_SIN {
        return None;
    }

    closest_pull(&cam.ray(rect, pos), start, dir)
}

/// The parameter along the line `start + s * dir` (unit `dir`) at its closest point to `ray`, None when the ray runs
/// nearly parallel to the line or the closest point lies behind the ray origin.
fn closest_pull(ray: &Ray, start: DVec3, dir: DVec3) -> Option<f64> {
    let ad = dir.dot(ray.dir);
    let den = 1.0 - ad * ad;
    if den < MIN_PULL_SIN * MIN_PULL_SIN {
        return None;
    }

    let w = start - ray.origin;
    let s = (ad * w.dot(ray.dir) - w.dot(dir)) / den;
    (w.dot(ray.dir) + s * ad > 0.0).then_some(s)
}

/// The snapped offset a drag at `pos` asks for, if the pointer ray meets the drag plane or line.
fn vertex_drag_delta(state: &EditorState, cam: &Camera, rect: Rect, drag: &VertexDrag, pos: Pos2, alt: bool) -> Option<DVec3> {
    if let Some(dir) = drag.line {
        return Some(state.snap(dir * pull_along(cam, rect, drag.start, dir, pos)?));
    }

    let ray = cam.ray(rect, pos);
    let t = ray.intersect_plane(&drag.plane)?;
    let mut delta = ray.at(t) - drag.start;
    if cam.kind == ViewKind::Perspective && alt && !drag.is_dot {
        delta = DVec3::new(0.0, delta.y, 0.0);
    }

    // Snaps the offset like the Move tool, so an off grid vertex keeps its position on the other axes.
    Some(state.snap(delta))
}

/// Why `move_vertices` refuses: a target inside or flat on a brush it moves is a dent, and brushes are convex.
fn refusal_reason(state: &EditorState, base: &[DVec3], delta: DVec3) -> &'static str {
    let moves = vertex_moves(state, base, delta);
    let brushes = state.doc.selection.brushes(&state.doc.map);
    let dent =
        brushes.into_iter().filter_map(|id| state.doc.map.brush(id)).any(|b| moves.iter().any(|(p, d)| b.contains_point(*p) && b.contains_point(*p + *d)));
    if dent { DENT_REASON } else { REFUSED_REASON }
}

/// The `(position, delta)` moves of one vertex edit: `base` by `delta`, plus the mirrored counterparts while the edit
/// symmetry is on. Counterparts come from the selected brushes as they are now, so a drag looks them up on the map it
/// started from.
fn vertex_moves(state: &EditorState, base: &[DVec3], delta: DVec3) -> Vec<(DVec3, DVec3)> {
    let map = &state.doc.map;
    let brushes: Vec<&Brush> = state.doc.selection.brushes(map).into_iter().filter_map(|id| map.brush(id)).collect();
    let bounds = brushes.iter().fold(Aabb::EMPTY, |acc, b| acc.union(&b.bounds()));
    match state.edit_symmetry.plane(&bounds) {
        Some(plane) => td_geom::symmetry::symmetric_moves(&brushes, base, delta, plane, td_doc::symmetry::MIRROR_TOLERANCE),
        None => base.iter().map(|p| (*p, delta)).collect(),
    }
}

/// What `moves` do to one brush.
enum Moved {
    Untouched,
    Refused,
    To(Brush),
}

fn move_brush(b: &Brush, moves: &[(DVec3, DVec3)]) -> Moved {
    let mut pts = b.vertices.clone();
    let mut touched = false;
    for (bp, delta) in moves {
        match pts.iter().position(|v| (*v - *bp).length() < 1e-6) {
            Some(i) => {
                pts[i] += *delta;
                touched = true;
            }
            None => {
                // Edge midpoint or face center: only valid if it lies on this brush's surface.
                if b.faces.iter().any(|f| f.plane.distance(*bp).abs() < 1e-4) && b.contains_point(*bp) && !b.vertices.iter().any(|v| (*v - *bp).length() < 1e-6)
                {
                    pts.push(*bp + *delta);
                    touched = true;
                }
            }
        }
    }

    if !touched {
        return Moved::Untouched;
    }

    let Ok(nb) = Brush::from_points(&pts, &b.planes(), "") else { return Moved::Refused };
    let kept = moves.iter().all(|(bp, delta)| {
        let target = *bp + *delta;
        !b.contains_point(*bp) || nb.vertices.iter().any(|v| (*v - target).length() < 1e-3)
    });
    if kept { Moved::To(nb) } else { Moved::Refused }
}

/// Moves the selected brushes' vertices at `base` (or new ones on an edge or face) by `delta`, like the vertex tool.
/// Brushes that would turn concave or lose a moved vertex are left alone.
#[cfg(test)]
fn move_vertices(state: &mut EditorState, base: &[DVec3], delta: DVec3) -> bool {
    try_move_vertices(state, base, delta).is_some()
}

/// `move_vertices` that says how many counterparts moved with the edit. While the edit symmetry is on the move is all
/// or nothing, so a mirrored half never drifts from its counterpart, and the map is left as it was when it refuses.
pub(crate) fn try_move_vertices(state: &mut EditorState, base: &[DVec3], delta: DVec3) -> Option<usize> {
    let moves = vertex_moves(state, base, delta);
    let mirrored = moves.len() - base.len();
    let atomic = state.edit_symmetry.axis.is_some();
    let brushes: Vec<NodeId> = state.doc.selection.brushes(&state.doc.map);
    let mut moved: Vec<(NodeId, Brush)> = Vec::new();
    for id in brushes {
        let Some(b) = state.doc.map.brush(id) else { continue };
        match move_brush(b, &moves) {
            Moved::To(nb) => moved.push((id, nb)),
            Moved::Refused if atomic => return None,
            Moved::Refused | Moved::Untouched => {}
        }
    }

    if moved.is_empty() {
        return None;
    }

    state.doc.edit("Move Vertices", |m, _| {
        for (id, nb) in moved {
            if let Some(slot) = m.brush_mut(id) {
                *slot = nb;
            }
        }
    });
    Some(mirrored)
}

/// The status after a move that took `n` counterparts across the edit symmetry plane along.
pub(crate) fn moved_mirrored(n: usize) -> Msg {
    msg_n!(n, "Moved {n} mirrored vertex too", "Moved {n} mirrored vertices too")
}

fn seg_screen_dist(cam: &Camera, rect: Rect, a: DVec3, b: DVec3, pos: Pos2) -> f32 {
    let (Some(pa), Some(pb)) = (cam.project(rect, a), cam.project(rect, b)) else { return f32::MAX };
    let ab = pb - pa;
    let t = ((pos - pa).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (pa + ab * t - pos).length()
}

fn ring_plane_point(cam: &Camera, rect: Rect, pos: Pos2, center: DVec3, axis: DVec3) -> Option<DVec3> {
    let ray = cam.ray(rect, pos);
    let plane = Plane::from_point_normal(center, axis);
    match ray.intersect_plane(&plane) {
        Some(t) if t > 0.0 || cam.kind.is_2d() => Some(ray.at(t)),
        _ => {
            // Ring seen edge-on: fall back to projecting the closest point on the view ray.
            let (_, t) = ray.distance_to_point(center);
            Some(plane.project_point(ray.at(t)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_brush() -> (EditorState, NodeId) {
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let brush = Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::splat(64.0)), "dev/grey").unwrap();
        let id = state.doc.edit("brush", |m, s| {
            let id = m.insert(layer, td_doc::NodeKind::Brush(brush));
            s.select_node(id);
            id
        });
        (state, id)
    }

    /// Starts a rotate drag the way `rotate_input` does and applies a quarter turn inside it.
    fn rotate_mid_drag(tools: &mut ToolSet, state: &mut EditorState) {
        state.tool = ToolKind::Rotate;
        tools.sync(state);
        state.doc.begin("Rotate");
        tools.rotate.drag = Some(RotateDrag { axis: DVec3::Y, center: DVec3::splat(32.0), start_vec: DVec3::X, screen_start: Pos2::ZERO });
        let m = ops::rotation_about(DVec3::splat(32.0), DVec3::Y, 90.0);
        let opts = state.opts();
        state.doc.edit("Rotate", |map, s| ops::transform_selection(map, s, &m, opts));
    }

    /// A slanted, skewed wedge away from the origin, shaped like the one in the bug report: a sloped top over a
    /// footprint that is not a rectangle.
    fn wedge() -> Brush {
        let pts = [
            DVec3::new(100.0, 64.0, -150.0),
            DVec3::new(260.0, 64.0, -150.0),
            DVec3::new(280.0, 64.0, -30.0),
            DVec3::new(110.0, 64.0, -20.0),
            DVec3::new(100.0, 160.0, -150.0),
            DVec3::new(260.0, 96.0, -150.0),
            DVec3::new(280.0, 88.0, -30.0),
            DVec3::new(110.0, 156.0, -20.0),
        ];
        Brush::from_points(&pts, &[], "dev/grey").unwrap()
    }

    fn state_with_wedge() -> (EditorState, NodeId) {
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let id = state.doc.edit("wedge", |m, s| {
            let id = m.insert(layer, td_doc::NodeKind::Brush(wedge()));
            s.select_node(id);
            id
        });
        (state, id)
    }

    fn view_rect() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))
    }

    #[test]
    fn the_vertex_box_takes_the_corners_inside_at_any_depth() {
        let (mut state, _) = state_with_wedge();
        let mut tools = ToolSet::default();
        let top = |a: [f64; 2], b: [f64; 2], add| SelectionBox { view: ViewKind::Top, a: DVec3::new(a[0], 0.0, a[1]), b: DVec3::new(b[0], 0.0, b[1]), add };
        let corners = |tools: &ToolSet| {
            let mut all: Vec<[i64; 3]> = tools.vertex.selected.iter().map(|v| v.to_array().map(|c| c.round() as i64)).collect();
            all.sort();
            all
        };

        tools.box_vertices(&mut state, &top([90.0, -160.0], [270.0, -140.0], false));
        assert_eq!(corners(&tools), [[100, 64, -150], [100, 160, -150], [260, 64, -150], [260, 96, -150]], "both of each stacked pair, no dot");
        assert_eq!(state.status, "Selected 4 corners");

        tools.box_vertices(&mut state, &top([400.0, 0.0], [500.0, 100.0], false));
        assert_eq!(corners(&tools).len(), 4, "an empty box keeps the corners");
        assert_eq!(state.status, "No corner lies inside the box");

        tools.box_vertices(&mut state, &top([270.0, -40.0], [290.0, -25.0], true));
        assert_eq!(corners(&tools).len(), 6, "Ctrl adds");
        assert_eq!(state.status, "Selected 2 corners");

        tools.box_vertices(&mut state, &top([110.0, -20.0], [280.0, -30.0], false));
        assert_eq!(corners(&tools), [[110, 64, -20], [110, 156, -20], [280, 64, -30], [280, 88, -30]], "corners on the edges count");

        tools.vertex.boxing = Some(top([0.0, 0.0], [1.0, 1.0], false));
        state.tool = ToolKind::Select;
        tools.sync(&state);
        assert!(tools.vertex.boxing.is_none(), "a tool switch drops the box");
        tools.vertex.boxing = Some(top([0.0, 0.0], [1.0, 1.0], false));
        tools.reset();
        assert!(tools.vertex.boxing.is_none());
    }

    #[test]
    fn box_select_takes_the_corners_in_sight_unless_xray_is_on() {
        let (mut state, _) = state_with_wedge();
        let mut tools = ToolSet::default();
        let cam = perspective();
        let all = |op| CornerBox { view: view_rect(), start: Pos2::ZERO, end: view_rect().max, op };
        let corners = ToolSet::vertex_handles(&state).0.len();

        tools.box_corners(&mut state, &cam, &all(BoxOp::Replace));
        let seen = tools.vertex.selected.len();
        assert!(seen > 0 && seen < corners, "the corners on the far side stay out, {seen} of {corners}");
        state.prefs.box_xray = true;
        tools.box_corners(&mut state, &cam, &all(BoxOp::Add));
        assert_eq!(tools.vertex.selected.len(), corners, "X-ray adds the hidden ones");
        tools.box_corners(&mut state, &cam, &all(BoxOp::Remove));
        assert!(tools.vertex.selected.is_empty());
        assert_eq!(state.status, format!("Deselected {corners} corners"));

        tools.vertex.screen_box = Some(all(BoxOp::Replace));
        tools.reset();
        assert!(tools.vertex.screen_box.is_none());
    }

    fn perspective() -> Camera {
        let mut cam = Camera::new(ViewKind::Perspective);
        cam.position = DVec3::new(620.0, 520.0, -640.0);
        cam.look_at(DVec3::new(190.0, 100.0, -85.0));
        cam
    }

    fn view_2d(kind: ViewKind) -> Camera {
        let mut cam = Camera::new(kind);
        cam.center = DVec3::new(190.0, 100.0, -85.0);
        cam.zoom = 1.5;
        cam
    }

    fn brush_of(state: &EditorState, id: NodeId) -> &Brush {
        state.doc.map.brush(id).unwrap()
    }

    fn top_normal() -> DVec3 {
        DVec3::new(0.4, 1.0, 0.0).normalize()
    }

    #[test]
    fn the_shape_box_is_drawn_in_every_view() {
        let b = Aabb::new(DVec3::new(-40.0, 8.0, 24.0), DVec3::new(88.0, 72.0, 120.0));
        let corners = b.corners();
        let mut state = EditorState::new(Default::default());
        let tools = ToolSet::default();
        let cams = [perspective(), view_2d(ViewKind::Top), view_2d(ViewKind::Front), view_2d(ViewKind::Side)];
        let of_color = |lines: &[LineVertex], c: [f32; 4]| lines.iter().filter(|v| v.color == c).map(|v| v.pos).collect::<Vec<_>>();

        for cam in &cams {
            assert!(of_color(&tools.lines(cam, view_rect(), &state), crate::shape_bounds::color()).is_empty(), "no box without bounds");
        }

        state.shape_bounds = Some(b);
        state.shape_preview = vec![(DVec3::new(0.0, 8.0, 24.0), DVec3::new(10.0, 20.0, 30.0))];
        let expected: Vec<[f32; 3]> = Aabb::EDGES.iter().flat_map(|&(i, j)| [crate::scene::v3(corners[i]), crate::scene::v3(corners[j])]).collect();
        for cam in &cams {
            let lines = tools.lines(cam, view_rect(), &state);
            assert_eq!(of_color(&lines, crate::shape_bounds::color()), expected, "{:?}", cam.kind);
            assert_eq!(of_color(&lines, PREVIEW_COLOR).len(), 2, "the preview keeps its own color in {:?}", cam.kind);
        }
    }

    fn state_with_terrains() -> (EditorState, NodeId, NodeId) {
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let (a, b) = state.doc.edit("terrains", |m, s| {
            let a = m.insert(layer, td_doc::NodeKind::Terrain(td_geom::Terrain::new(DVec3::new(-96.5, 12.0, 40.25), [17, 9], 32.0, "dev/grey")));
            let b = m.insert(layer, td_doc::NodeKind::Terrain(td_geom::Terrain::new(DVec3::new(500.0, 0.0, 500.0), [5, 5], 64.0, "dev/grey")));
            s.select_node(a);
            (a, b)
        });
        (state, a, b)
    }

    fn draft(id: NodeId, resize: td_doc::terrain_resize::TerrainResize) -> Option<crate::terrain_size::TerrainResizeDraft> {
        Some(crate::terrain_size::TerrainResizeDraft { id, resize })
    }

    #[test]
    fn a_pending_terrain_size_is_outlined_in_every_view() {
        use td_doc::terrain_resize::TerrainResize;
        use td_geom::terrain_resize::Sides;
        let (mut state, a, b) = state_with_terrains();
        let tools = ToolSet::default();
        let cams = [perspective(), view_2d(ViewKind::Top), view_2d(ViewKind::Front), view_2d(ViewKind::Side)];
        let color = crate::shape_bounds::color();
        let outline =
            |state: &EditorState, cam: &Camera| tools.lines(cam, view_rect(), state).iter().filter(|v| v.color == color).map(|v| v.pos).collect::<Vec<_>>();
        let nothing = |state: &EditorState, why: &str| {
            for cam in &cams {
                assert!(outline(state, cam).is_empty(), "{why} in {:?}", cam.kind);
            }
        };

        nothing(&state, "no draft");
        state.terrain_resize = draft(a, TerrainResize::default());
        nothing(&state, "a draft that changes nothing");
        state.terrain_resize = draft(a, TerrainResize { cell_size: Some(32.0), ..Default::default() });
        nothing(&state, "the cell size it already has");

        let wide = TerrainResize { sides: Sides { north: -2, south: 0, west: 1, east: 3 }, ..Default::default() };
        let old = state.doc.map.terrain(a).unwrap().bounds();
        let new = Aabb::new(DVec3::new(-128.5, old.min.y, 104.25), DVec3::new(-128.5 + 640.0, old.max.y, 104.25 + 192.0));
        let corners = new.corners();
        let expected: Vec<[f32; 3]> = Aabb::EDGES.iter().flat_map(|&(i, j)| [crate::scene::v3(corners[i]), crate::scene::v3(corners[j])]).collect();
        state.terrain_resize = draft(a, wide);
        for cam in &cams {
            assert_eq!(outline(&state, cam), expected, "{:?}", cam.kind);
        }

        let rescaled = TerrainResize { cell_size: Some(64.0), ..Default::default() };
        state.terrain_resize = draft(a, rescaled);
        let half = Aabb::new(DVec3::new(-96.5, old.min.y, 40.25), DVec3::new(-96.5 + 512.0, old.max.y, 40.25 + 256.0));
        let corners = half.corners();
        let expected: Vec<[f32; 3]> = Aabb::EDGES.iter().flat_map(|&(i, j)| [crate::scene::v3(corners[i]), crate::scene::v3(corners[j])]).collect();
        assert_eq!(outline(&state, &cams[1]), expected, "a new cell size keeps the extent");

        state.terrain_resize = draft(b, wide);
        nothing(&state, "a draft of a terrain that is not the selection");
        state.doc.selection.clear();
        state.terrain_resize = draft(a, wide);
        nothing(&state, "a draft with nothing selected");
        state.doc.selection.select_node(a);
        state.doc.selection.select_node(b);
        nothing(&state, "a draft over two selected nodes");
    }

    #[test]
    fn a_refused_terrain_size_draws_nothing() {
        use td_doc::terrain_resize::TerrainResize;
        use td_geom::terrain_resize::{MAX_CELLS, Sides};
        let (mut state, a, _) = state_with_terrains();
        let tools = ToolSet::default();
        let cam = view_2d(ViewKind::Top);
        let color = crate::shape_bounds::color();
        let drawn = |state: &EditorState| tools.lines(&cam, view_rect(), state).iter().filter(|v| v.color == color).count();
        let with = |sides: Sides| TerrainResize { sides, ..Default::default() };

        // The terrain is 16 x 8 cells, so these sit on both sides of every limit.
        for (sides, kept) in [
            (Sides { north: -7, ..Default::default() }, true),
            (Sides { north: -8, ..Default::default() }, false),
            (Sides { west: -8, east: -8, ..Default::default() }, false),
            (Sides { east: MAX_CELLS as i32 - 16, ..Default::default() }, true),
            (Sides { east: MAX_CELLS as i32 - 15, ..Default::default() }, false),
        ] {
            state.terrain_resize = draft(a, with(sides));
            assert_eq!(drawn(&state), if kept { 24 } else { 0 }, "{sides:?}");
        }

        state.terrain_resize = draft(a, with(Sides { east: 4, ..Default::default() }));
        assert_eq!(drawn(&state), 24);
        state.doc.edit("lock", |m, _| td_doc::ops::set_locked(m, &[a], true));
        assert_eq!(drawn(&state), 0, "a locked terrain cannot be resized");
        state.doc.edit("unlock", |m, _| {
            td_doc::ops::set_locked(m, &[a], false);
            td_doc::ops::set_hidden(m, &[a], true);
        });
        assert_eq!(drawn(&state), 0, "neither can a hidden one");
    }

    #[test]
    fn dots_carry_the_direction_that_leaves_the_brush() {
        let (state, id) = state_with_wedge();
        let (verts, dots) = ToolSet::vertex_handles(&state);
        let b = brush_of(&state, id);
        assert_eq!(verts.len(), 8);
        assert_eq!(dots.len(), 6 + 12, "one dot per face and per edge");

        let top = dots.iter().find(|d| (d.out() - top_normal()).length() < 1e-6).expect("the sloped top face has a dot");
        let face = b.faces.iter().position(|f| (f.plane.normal - top_normal()).length() < 1e-6).unwrap();
        assert!((top.pos - b.face_center(face)).length() < 1e-9);

        // The edge between the sloped top and the z = -150 side leaves along the sum of both normals.
        let edge_out = (top_normal() + DVec3::NEG_Z).normalize();
        let edge = dots.iter().find(|d| (d.out() - edge_out).length() < 1e-6).expect("top front edge dot");
        let mid = (DVec3::new(100.0, 160.0, -150.0) + DVec3::new(260.0, 96.0, -150.0)) * 0.5;
        assert!((edge.pos - mid).length() < 1e-9, "edge dot sits at the midpoint, got {:?}", edge.pos);
        assert!(dots.iter().all(|d| (d.out().length() - 1.0).abs() < 1e-9));
    }

    #[test]
    fn dots_on_faces_turned_away_are_hidden_in_3d() {
        let (state, _) = state_with_wedge();
        let (_, dots) = ToolSet::vertex_handles(&state);
        let cam = perspective();
        let facing = |d: &VertexDot| d.normals.iter().any(|n| n.dot(cam.eye() - d.pos) > 0.0);
        assert!(dots.iter().any(|d| dot_visible(&cam, d)) && dots.iter().any(|d| !dot_visible(&cam, d)));
        for d in &dots {
            assert_eq!(dot_visible(&cam, d), facing(d), "dot at {:?}", d.pos);
        }

        // From above, in front and to the right: the bottom face center is hidden, the top and front are not.
        let face_dot = |n: DVec3| dots.iter().find(|d| d.normals[0] == d.normals[1] && (d.normals[0] - n).length() < 1e-6).unwrap();
        assert!(dot_visible(&cam, face_dot(top_normal())));
        assert!(dot_visible(&cam, face_dot(DVec3::NEG_Z)));
        assert!(!dot_visible(&cam, face_dot(DVec3::NEG_Y)));

        // A silhouette edge shows when only one of its two faces looks at the camera.
        let silhouette = dots.iter().filter(|d| d.normals[0] != d.normals[1]).any(|d| {
            let toward = d.normals.iter().filter(|n| n.dot(cam.eye() - d.pos) > 0.0).count();
            toward == 1 && dot_visible(&cam, d)
        });
        assert!(silhouette, "the wedge has silhouette edges from this camera");
    }

    #[test]
    fn dots_facing_along_a_2d_views_depth_are_hidden() {
        let cam = view_2d(ViewKind::Top);
        let at = |sideways: f64| {
            let n = DVec3::new(sideways, (1.0 - sideways * sideways).sqrt(), 0.0);
            VertexDot { pos: DVec3::new(100.0, 50.0, 0.0), normals: [n; 2] }
        };
        assert!(dot_visible(&cam, &at(0.21)), "tilted just enough to leave along the view plane");
        assert!(!dot_visible(&cam, &at(0.19)), "nearly along the depth axis");
        assert!(!dot_visible(&cam, &at(0.0)), "straight along the depth axis");
        assert!(dot_visible(&cam, &at(1.0)));

        let (state, _) = state_with_wedge();
        let (_, dots) = ToolSet::vertex_handles(&state);
        let bottom = dots.iter().find(|d| (d.out() - DVec3::NEG_Y).length() < 1e-6).unwrap();
        assert!(!dot_visible(&cam, bottom), "the bottom face center can never leave its face in the top view");
        assert!(dot_visible(&view_2d(ViewKind::Front), bottom));
    }

    #[test]
    fn a_vertex_square_beats_a_nearby_dot_and_a_dot_beats_a_far_vertex() {
        let cam = view_2d(ViewKind::Front);
        let rect = view_rect();
        let vert = DVec3::new(100.0, 80.0, 0.0);
        let sp = cam.project(rect, vert).unwrap();
        let px = |dx: f32, dy: f32| Pos2::new(sp.x + dx, sp.y + dy);
        // One pixel on screen is 1 / zoom units.
        let dot_at = |dx: f32, dy: f32| VertexDot { pos: vert + DVec3::new(dx as f64 / 1.5, -dy as f64 / 1.5, 0.0), normals: [DVec3::X; 2] };
        let verts = [vert];

        // The pointer is on the dot, 3 px from the vertex and so inside its square: the vertex wins.
        let near = dot_at(3.0, 0.0);
        assert_eq!(handle_at(&cam, rect, &verts, &[near], px(3.0, 0.0)), Some(Handle::Vertex(vert)));
        // The dot is 6 px away, the pointer on it is outside the square, so the dot wins.
        let far = dot_at(6.0, 0.0);
        assert_eq!(handle_at(&cam, rect, &verts, &[far], px(6.0, 0.0)), Some(Handle::Dot(far)));
        // The square edge is inclusive, half a pixel past it the closest handle wins.
        assert_eq!(handle_at(&cam, rect, &verts, &[far], px(5.0, 0.0)), Some(Handle::Vertex(vert)));
        assert_eq!(handle_at(&cam, rect, &verts, &[far], px(5.5, 0.0)), Some(Handle::Dot(far)));
        // Outside the square but closer to the vertex than to the dot, the vertex still wins.
        assert_eq!(handle_at(&cam, rect, &verts, &[far], px(-1.0, 6.5)), Some(Handle::Vertex(vert)));
        // Reach ends at HANDLE_RADIUS for both kinds.
        assert_eq!(handle_at(&cam, rect, &verts, &[], px(0.0, 6.9)), Some(Handle::Vertex(vert)));
        assert_eq!(handle_at(&cam, rect, &verts, &[], px(0.0, 7.1)), None);
        let lone = dot_at(0.0, 30.0);
        assert_eq!(handle_at(&cam, rect, &[], &[lone], px(0.0, 36.9)), Some(Handle::Dot(lone)));
        assert_eq!(handle_at(&cam, rect, &[], &[lone], px(0.0, 37.1)), None);
    }

    #[test]
    fn a_hidden_dot_is_not_picked() {
        let cam = perspective();
        let rect = view_rect();
        let (state, _) = state_with_wedge();
        let (_, dots) = ToolSet::vertex_handles(&state);
        let hidden = dots.iter().find(|d| !dot_visible(&cam, d)).unwrap();
        let sp = cam.project(rect, hidden.pos).unwrap();
        assert_eq!(handle_at(&cam, rect, &[], std::slice::from_ref(hidden), sp), None);
        let shown = dots.iter().find(|d| dot_visible(&cam, d)).unwrap();
        let sp = cam.project(rect, shown.pos).unwrap();
        assert_eq!(handle_at(&cam, rect, &[], std::slice::from_ref(shown), sp), Some(Handle::Dot(*shown)));
    }

    #[test]
    fn a_3d_dot_drags_in_a_camera_facing_plane_that_holds_its_direction() {
        let (state, _) = state_with_wedge();
        let (_, dots) = ToolSet::vertex_handles(&state);
        let cam = perspective();
        let mut checked = 0;
        for d in dots.iter().filter(|d| dot_visible(&cam, d)) {
            let plane = vertex_drag_plane(&cam, &Handle::Dot(*d), false);
            assert!(plane.normal.dot(d.out()).abs() < 1e-9, "the plane holds the outward direction");
            assert!(plane.distance(d.pos).abs() < 1e-9, "and passes through the dot");
            assert!(plane.normal.dot(cam.forward()).abs() > 0.05, "and faces the camera");
            checked += 1;
        }

        assert!(checked >= 9, "checked {checked} visible dots");
        let top = dots.iter().find(|d| (d.out() - top_normal()).length() < 1e-6).unwrap();
        let vertex_plane = vertex_drag_plane(&cam, &Handle::Vertex(top.pos), false);
        assert_eq!(vertex_plane.normal, DVec3::Y, "a vertex still drags in the horizontal plane");
        assert!(vertex_plane.normal.dot(top.out()).abs() > 0.5, "which a top dot could not leave");
        assert_eq!(vertex_drag_plane(&view_2d(ViewKind::Front), &Handle::Dot(*top), false).normal, DVec3::Z);
    }

    fn dot_drag(cam: &Camera, d: &VertexDot) -> VertexDrag {
        let handle = Handle::Dot(*d);
        VertexDrag { start: d.pos, plane: vertex_drag_plane(cam, &handle, false), line: vertex_drag_line(cam, &handle), base: vec![d.pos], is_dot: true }
    }

    /// A camera `phi` degrees off the dot's normal, looking at the dot from 400 units, then turned by `yaw_off`
    /// degrees so the dot sits away from the screen center.
    fn camera_off_normal(dot: &VertexDot, phi: f64, yaw_off: f64) -> Camera {
        let out = dot.out();
        let side = out.cross(DVec3::Y).normalize();
        let mut cam = Camera::new(ViewKind::Perspective);
        cam.position = dot.pos + (out * phi.to_radians().cos() + side * phi.to_radians().sin()) * 400.0;
        cam.look_at(dot.pos);
        cam.yaw += yaw_off.to_radians();
        cam
    }

    fn top_dot() -> VertexDot {
        let (state, _) = state_with_wedge();
        let (_, dots) = ToolSet::vertex_handles(&state);
        *dots.iter().find(|d| (d.out() - top_normal()).length() < 1e-6).unwrap()
    }

    /// The pointer that sits `px` pixels from the dot, `deg` degrees turned from the projected normal.
    fn pointer_from_dot(cam: &Camera, dot: &VertexDot, px: f32, deg: f32) -> Pos2 {
        let rect = view_rect();
        let at = cam.project(rect, dot.pos).unwrap();
        let along = (cam.project(rect, dot.pos + dot.out() * 10.0).unwrap() - at).normalized();
        let (sin, cos) = deg.to_radians().sin_cos();
        at + Vec2::new(along.x * cos - along.y * sin, along.x * sin + along.y * cos) * px
    }

    fn unsnapped() -> EditorState {
        let mut state = state_with_wedge().0;
        state.snap = false;
        state
    }

    #[test]
    fn a_3d_face_dot_follows_its_normal_at_a_grazing_angle() {
        let dot = top_dot();
        let state = unsnapped();
        let cam = camera_off_normal(&dot, 30.0, -38.0);
        let drag = dot_drag(&cam, &dot);
        let rect = view_rect();
        assert!(rect.contains(cam.project(rect, dot.pos).unwrap()), "the dot is on screen");

        // Whatever way the pointer drifts off the projected normal, the pull stays on the normal line and short.
        let mut longest: f64 = 0.0;
        for deg in [0.0, 15.0, -15.0, 90.0, -90.0, 135.0, 180.0] {
            let delta = vertex_drag_delta(&state, &cam, rect, &drag, pointer_from_dot(&cam, &dot, 40.0, deg), false).unwrap();
            assert!((delta - dot.out() * delta.dot(dot.out())).length() < 1e-6, "{deg} degrees off the normal leaves the normal line: {delta:?}");
            longest = longest.max(delta.length());
        }

        let px_per_unit = ((cam.project(rect, dot.pos + dot.out() * 10.0).unwrap() - cam.project(rect, dot.pos).unwrap()).length() / 10.0) as f64;
        assert!(longest < 1.5 * 40.0 / px_per_unit, "40 px moved the dot {longest} units at {px_per_unit} px per unit");
        let on_normal = vertex_drag_delta(&state, &cam, rect, &drag, pointer_from_dot(&cam, &dot, 40.0, 0.0), false).unwrap();
        assert!(on_normal.dot(dot.out()) > 30.0, "40 px along the normal pulls {on_normal:?}");
    }

    fn edge_dot() -> VertexDot {
        let (state, _) = state_with_wedge();
        let (_, dots) = ToolSet::vertex_handles(&state);
        *dots.iter().filter(|d| d.normals[0] != d.normals[1]).max_by(|a, b| a.out().y.total_cmp(&b.out().y)).unwrap()
    }

    #[test]
    fn a_3d_edge_dot_follows_its_outward_direction_at_a_grazing_angle() {
        let dot = edge_dot();
        let state = unsnapped();
        let cam = camera_off_normal(&dot, 30.0, -38.0);
        let drag = dot_drag(&cam, &dot);
        let rect = view_rect();
        assert!(rect.contains(cam.project(rect, dot.pos).unwrap()), "the dot is on screen");

        let mut longest: f64 = 0.0;
        for deg in [0.0, 15.0, -15.0, 90.0, -90.0, 135.0, 180.0] {
            let delta = vertex_drag_delta(&state, &cam, rect, &drag, pointer_from_dot(&cam, &dot, 40.0, deg), false).unwrap();
            assert!((delta - dot.out() * delta.dot(dot.out())).length() < 1e-6, "{deg} degrees off the direction leaves its line: {delta:?}");
            longest = longest.max(delta.length());
        }

        let px_per_unit = ((cam.project(rect, dot.pos + dot.out() * 10.0).unwrap() - cam.project(rect, dot.pos).unwrap()).length() / 10.0) as f64;
        assert!(longest < 1.5 * 40.0 / px_per_unit, "40 px moved the dot {longest} units at {px_per_unit} px per unit");
        for want in [7.0, 64.0, -30.0] {
            let pointer = cam.project(rect, dot.pos + dot.out() * want).unwrap();
            let delta = vertex_drag_delta(&state, &cam, rect, &drag, pointer, false).unwrap();
            assert!((delta - dot.out() * want).length() < 1e-3, "wanted {want}, got {delta:?}");
        }
    }

    #[test]
    fn a_3d_edge_dot_holds_still_when_the_view_looks_along_its_direction() {
        let dot = edge_dot();
        let state = unsnapped();
        let cam = camera_off_normal(&dot, 3.0, 0.0);
        let drag = dot_drag(&cam, &dot);
        let pointer = pointer_from_dot(&cam, &dot, 120.0, 45.0);
        assert_eq!(vertex_drag_delta(&state, &cam, view_rect(), &drag, pointer, false), None);
    }

    #[test]
    fn a_3d_face_dot_pull_matches_the_pointer_head_on_and_off_center() {
        let dot = top_dot();
        let state = unsnapped();
        let rect = view_rect();
        for (phi, yaw_off) in [(60.0, 0.0), (60.0, -38.0), (-45.0, 20.0)] {
            let cam = camera_off_normal(&dot, phi, yaw_off);
            let drag = dot_drag(&cam, &dot);
            for want in [7.0, 20.0, 64.0, 150.0] {
                let pointer = cam.project(rect, dot.pos + dot.out() * want).unwrap();
                let delta = vertex_drag_delta(&state, &cam, rect, &drag, pointer, false).unwrap();
                assert!((delta - dot.out() * want).length() < 1e-3, "phi {phi} yaw {yaw_off}: wanted {want}, got {delta:?}");
            }

            let sideways = vertex_drag_delta(&state, &cam, rect, &drag, pointer_from_dot(&cam, &dot, 40.0, 90.0), false).unwrap();
            assert!(sideways.length() < 25.0, "phi {phi} yaw {yaw_off}: a sideways move pulled {sideways:?}");
        }
    }

    #[test]
    fn a_3d_face_dot_pull_snaps_to_the_grid() {
        let dot = top_dot();
        let mut state = state_with_wedge().0;
        state.snap = true;
        state.grid = 16.0;
        let cam = camera_off_normal(&dot, 60.0, -20.0);
        let drag = dot_drag(&cam, &dot);
        let pointer = cam.project(view_rect(), dot.pos + dot.out() * 50.0).unwrap();
        let delta = vertex_drag_delta(&state, &cam, view_rect(), &drag, pointer, false).unwrap();
        assert_eq!(delta, state.snap(dot.out() * 50.0));
        assert!([delta.x, delta.y, delta.z].iter().all(|c| (c / 16.0 - (c / 16.0).round()).abs() < 1e-9), "{delta:?} is on the grid");
    }

    #[test]
    fn a_3d_face_dot_holds_still_when_the_view_looks_along_its_normal() {
        let dot = top_dot();
        let state = unsnapped();
        let rect = view_rect();
        for phi in [0.0, 3.0, 10.0, 14.0] {
            let cam = camera_off_normal(&dot, phi, 0.0);
            let drag = dot_drag(&cam, &dot);
            for px in [1.0, 40.0, 120.0, 240.0] {
                for deg in (0..360).step_by(45) {
                    let pointer = pointer_from_dot(&cam, &dot, px, deg as f32);
                    assert_eq!(vertex_drag_delta(&state, &cam, rect, &drag, pointer, false), None, "phi {phi}, {px} px at {deg} degrees jumped");
                }
            }
        }

        // Past the limit (asin 0.25 is 14.5 degrees) the pull reads again.
        let cam = camera_off_normal(&dot, 16.0, 0.0);
        let drag = dot_drag(&cam, &dot);
        let pointer = cam.project(rect, dot.pos + dot.out() * 40.0).unwrap();
        assert!(vertex_drag_delta(&state, &cam, rect, &drag, pointer, false).is_some());
    }

    #[test]
    fn a_3d_face_dot_holds_still_when_the_pointer_ray_runs_along_its_normal() {
        let dot = top_dot();
        let state = unsnapped();
        let rect = view_rect();
        let cam = camera_off_normal(&dot, 30.0, 0.0);
        let drag = dot_drag(&cam, &dot);
        // The vanishing point of the normal line, where the pointer ray is parallel to it.
        let vanish = cam.project(rect, cam.position - dot.out() * 1000.0).unwrap();
        assert!(rect.contains(vanish), "{vanish:?} is on screen");
        assert_eq!(vertex_drag_delta(&state, &cam, rect, &drag, vanish, false), None);
        let near = vanish + Vec2::new(8.0, 0.0);
        assert_eq!(vertex_drag_delta(&state, &cam, rect, &drag, near, false), None, "still within the limit of a parallel ray");
    }

    #[test]
    fn the_closest_pull_rejects_a_point_behind_the_ray_and_reads_one_in_front() {
        let start = DVec3::new(0.0, 0.0, -100.0);
        let ahead = Ray::new(DVec3::ZERO, DVec3::new(1.0, 0.0, -0.5));
        assert!((closest_pull(&ahead, start, DVec3::X).unwrap() - 200.0).abs() < 1e-9);
        let behind = Ray::new(DVec3::ZERO, DVec3::new(1.0, 0.0, 0.5));
        assert_eq!(closest_pull(&behind, start, DVec3::X), None);
        let inward = Ray::new(DVec3::ZERO, DVec3::new(-1.0, 0.0, -0.5));
        assert!((closest_pull(&inward, start, DVec3::X).unwrap() + 200.0).abs() < 1e-9);
        let nearly_parallel = Ray::new(DVec3::new(0.0, 0.0, 10.0), DVec3::new(1.0, 0.0, -0.1));
        assert_eq!(closest_pull(&nearly_parallel, start, DVec3::X), None);
    }

    #[test]
    fn only_a_3d_dot_drags_along_a_line() {
        let (state, _) = state_with_wedge();
        let (_, dots) = ToolSet::vertex_handles(&state);
        let persp = perspective();
        assert!(dots.iter().any(|d| d.normals[0] == d.normals[1]) && dots.iter().any(|d| d.normals[0] != d.normals[1]));
        for d in &dots {
            let line = vertex_drag_line(&persp, &Handle::Dot(*d));
            assert_eq!(line, Some(d.out()), "dot at {:?}", d.pos);
            assert_eq!(vertex_drag_line(&view_2d(ViewKind::Top), &Handle::Dot(*d)), None);
            assert_eq!(vertex_drag_line(&persp, &Handle::Vertex(d.pos)), None);
        }
    }

    #[test]
    fn pulling_a_3d_face_dot_into_the_brush_is_refused() {
        let dot = top_dot();
        let mut state = unsnapped();
        let cam = camera_off_normal(&dot, 60.0, -10.0);
        let drag = dot_drag(&cam, &dot);
        let pointer = cam.project(view_rect(), dot.pos - dot.out() * 40.0).unwrap();
        let delta = vertex_drag_delta(&state, &cam, view_rect(), &drag, pointer, false).unwrap();
        assert!((delta + dot.out() * 40.0).length() < 1e-3, "{delta:?}");
        state.doc.begin("Move Vertices");
        assert!(!move_vertices(&mut state, &[dot.pos], delta));
        ToolSet::default().refuse(&mut state, &[dot.pos], dot.pos, delta);
        state.doc.commit();
        assert_eq!(state.status, DENT_REASON);
    }

    /// Pulls every dot visible in `cam` outward with a pointer drag and checks that a vertex appears at the snapped
    /// target. `pull` is how far the pointer goes from the dot along its outward direction.
    fn pull_every_visible_dot(cam: &Camera, pull: f64) -> usize {
        let rect = view_rect();
        let (base_state, id) = state_with_wedge();
        let before = brush_of(&base_state, id).vertices.clone();
        let (_, dots) = ToolSet::vertex_handles(&base_state);
        let mut pulled = 0;
        for d in dots.iter().filter(|d| dot_visible(cam, d)) {
            let mut state = state_with_wedge().0;
            let drag = dot_drag(cam, d);
            // The pointer travels in the drag plane: along the outward direction, or its part along the view plane in 2D.
            let dir = if cam.kind.is_2d() { (d.out() - cam.forward() * d.out().dot(cam.forward())).normalize() } else { d.out() };
            let pointer = cam.project(rect, d.pos + dir * pull).unwrap();
            let delta = vertex_drag_delta(&state, cam, rect, &drag, pointer, false).unwrap();
            assert!(delta.length() > 0.0, "dot at {:?} asks for no move", d.pos);
            assert!(delta.dot(d.out()) > 0.0, "snapping kept the pull outward for the dot at {:?}", d.pos);

            state.doc.begin("Move Vertices");
            assert!(move_vertices(&mut state, &[d.pos], delta), "dot at {:?} pulled by {delta:?} was refused", d.pos);
            state.doc.commit();
            let grown = brush_of(&state, id);
            assert!(grown.vertices.iter().any(|v| (*v - (d.pos + delta)).length() < 1e-3), "a vertex sits at the snapped target {:?}", d.pos + delta);
            // The hull may swallow a corner next to a steep pull, but it never loses volume.
            assert!(before.iter().all(|b| grown.contains_point(*b)), "the old corners stay inside for the dot at {:?}", d.pos);
            assert!(grown.vertices.len() >= 5);
            pulled += 1;
        }

        pulled
    }

    #[test]
    fn pulling_any_visible_dot_outward_adds_a_vertex_in_every_view() {
        let (state, _) = state_with_wedge();
        let (_, dots) = ToolSet::vertex_handles(&state);
        let persp = perspective();
        let pulled = pull_every_visible_dot(&persp, 40.0);
        assert_eq!(pulled, dots.iter().filter(|d| dot_visible(&persp, d)).count());
        assert!(pulled >= 9);

        for kind in [ViewKind::Top, ViewKind::Front] {
            let cam = view_2d(kind);
            let pulled = pull_every_visible_dot(&cam, 80.0);
            assert_eq!(pulled, dots.iter().filter(|d| dot_visible(&cam, d)).count());
            assert!(pulled >= 8, "{kind:?} reaches {pulled} dots");
            assert!(pulled < dots.len(), "{kind:?} hides the dots that face along its depth");
        }
    }

    #[test]
    fn pulling_a_dot_inward_is_refused_with_a_reason() {
        let (mut state, id) = state_with_wedge();
        let (_, dots) = ToolSet::vertex_handles(&state);
        let before = brush_of(&state, id).vertices.clone();
        let mut tools = ToolSet::default();
        for d in &dots {
            let delta = d.out() * -48.0;
            state.doc.begin("Move Vertices");
            assert!(!move_vertices(&mut state, &[d.pos], delta), "dot at {:?} pulled inward moved", d.pos);
            tools.refuse(&mut state, &[d.pos], d.pos, delta);
            state.doc.commit();
            assert_eq!(state.status, DENT_REASON);
            assert_eq!(tools.vertex.refused, Some((d.pos, d.pos + delta)));
            assert_eq!(brush_of(&state, id).vertices, before, "a refused move leaves the brush alone");
        }

        // Sliding along the face stays flat on it, which is a dent too.
        let top = dots.iter().find(|d| (d.out() - top_normal()).length() < 1e-6).unwrap();
        let slide = DVec3::new(16.0, -0.4 * 16.0, 0.0);
        assert_eq!(refusal_reason(&state, &[top.pos], slide), DENT_REASON);
        assert!(!move_vertices(&mut state, &[top.pos], slide), "sliding along the face adds nothing");
    }

    #[test]
    fn a_refusal_outside_the_brush_gets_the_generic_reason_and_a_zero_move_none() {
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let tetra = [DVec3::new(50.0, 10.0, 70.0), DVec3::new(114.0, 10.0, 70.0), DVec3::new(50.0, 10.0, 134.0), DVec3::new(50.0, 74.0, 70.0)];
        let brush = Brush::from_points(&tetra, &[], "dev/grey").unwrap();
        state.doc.edit("tetra", |m, s| {
            let id = m.insert(layer, td_doc::NodeKind::Brush(brush));
            s.select_node(id);
        });

        // Dropping the apex onto the base plane outside the footprint flattens the brush, no dent involved.
        let apex = tetra[3];
        let delta = DVec3::new(64.0, -64.0, 64.0);
        state.doc.begin("Move Vertices");
        assert!(!move_vertices(&mut state, &[apex], delta));
        let mut tools = ToolSet::default();
        tools.refuse(&mut state, &[apex], apex, delta);
        state.doc.commit();
        assert_eq!(state.status, REFUSED_REASON);
        assert!(tools.vertex.refused.is_some());

        // A move that snaps to nothing is not a refusal, and clears an earlier one.
        tools.refuse(&mut state, &[apex], apex, DVec3::ZERO);
        assert_eq!(tools.vertex.refused, None);
        assert_eq!(state.status, "");
    }

    #[test]
    fn clip_pieces_keep_the_brush_name() {
        let (mut state, id) = state_with_brush();
        state.doc.edit("name", |m, _| m.rename(id, "ramp"));
        clip_selection(&mut state, &Plane::from_point_normal(DVec3::splat(32.0), DVec3::Z), ClipSide::Both);
        let pieces = &state.replaced[&id];
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(|p| state.doc.map.get(*p).unwrap().name() == "ramp"));
    }

    #[test]
    fn clip_selection_keeps_the_side_the_normal_points_to() {
        let (mut state, _) = state_with_brush();
        let slanted = Plane::from_point_normal(DVec3::new(32.0, 32.0, 32.0), DVec3::new(1.0, 1.0, 0.0).normalize());
        let undo_before = state.doc.history.undo_labels().count();
        clip_selection(&mut state, &slanted, ClipSide::Front);
        let kept = state.doc.selection.brushes(&state.doc.map);
        assert_eq!(kept.len(), 1);
        let b = state.doc.map.brush(kept[0]).unwrap().bounds();
        assert_eq!((b.min.x, b.min.y, b.max.x, b.max.y), (0.0, 0.0, 64.0, 64.0), "a diagonal cut keeps the corner's full reach");
        assert!(state.doc.map.brush(kept[0]).unwrap().vertices.iter().all(|v| v.x + v.y >= 64.0 - 1e-6), "only the +x+y half is left");
        assert_eq!(state.doc.history.undo_labels().count(), undo_before + 1);

        clip_selection(&mut state, &Plane::from_point_normal(DVec3::splat(48.0), DVec3::Z), ClipSide::Both);
        assert_eq!(state.doc.selection.brushes(&state.doc.map).len(), 2, "both halves stay and are selected");
    }

    #[test]
    fn move_vertices_sags_a_corner_and_refuses_concave_results() {
        let (mut state, id) = state_with_brush();
        assert!(move_vertices(&mut state, &[DVec3::new(64.0, 64.0, 64.0)], DVec3::new(0.0, -24.0, 0.0)));
        let b = state.doc.map.brush(id).unwrap();
        assert!(b.vertices.iter().any(|v| (*v - DVec3::new(64.0, 40.0, 64.0)).length() < 1e-6));
        assert!(!move_vertices(&mut state, &[DVec3::new(1.0, 2.0, 3.0)], DVec3::X), "a point off the brush moves nothing");
    }

    fn hull(points: &[DVec3]) -> Brush {
        Brush::from_points(points, &[], "dev/grey").unwrap()
    }

    fn insert_brushes(state: &mut EditorState, brushes: Vec<Brush>) -> Vec<NodeId> {
        let layer = state.doc.map.default_layer();
        state.doc.edit("brushes", |m, s| {
            brushes
                .into_iter()
                .map(|b| {
                    let id = m.insert(layer, td_doc::NodeKind::Brush(b));
                    s.select_node(id);
                    id
                })
                .collect()
        })
    }

    fn corners(state: &EditorState, id: NodeId) -> Vec<DVec3> {
        let mut v = state.doc.map.brush(id).unwrap().vertices.clone();
        v.sort_by(|a, b| a.to_array().partial_cmp(&b.to_array()).unwrap());
        v
    }

    fn with_corners(mut v: Vec<DVec3>, edits: &[(DVec3, DVec3)]) -> Vec<DVec3> {
        for (from, to) in edits {
            let i = v.iter().position(|p| p.distance(*from) < 1e-6).unwrap_or_else(|| panic!("no corner at {from:?}"));
            v[i] = *to;
        }

        v.sort_by(|a, b| a.to_array().partial_cmp(&b.to_array()).unwrap());
        v
    }

    fn assert_corners(got: &[DVec3], want: &[DVec3]) {
        assert_eq!(got.len(), want.len(), "{got:?} against {want:?}");
        for (g, w) in got.iter().zip(want) {
            assert!(g.distance(*w) < 1e-6, "{got:?} against {want:?}");
        }
    }

    fn symmetry_x(at: td_doc::symmetry::PlaneAt, custom: f64) -> td_doc::symmetry::EditSymmetry {
        td_doc::symmetry::EditSymmetry { axis: Some(0), plane: td_doc::symmetry::PlaneChoice { at, custom } }
    }

    /// A trapezoid prism off the origin, mirror symmetric across x 37.5 and slanted in z at the top.
    fn trapezoid() -> Brush {
        hull(&[
            DVec3::new(-20.0, 0.0, 10.0),
            DVec3::new(95.0, 0.0, 10.0),
            DVec3::new(95.0, 0.0, 60.0),
            DVec3::new(-20.0, 0.0, 60.0),
            DVec3::new(10.0, 40.0, 25.0),
            DVec3::new(65.0, 40.0, 25.0),
            DVec3::new(65.0, 40.0, 50.0),
            DVec3::new(10.0, 40.0, 50.0),
        ])
    }

    #[test]
    fn a_symmetric_corner_move_takes_its_mirror_along_in_one_undo_step() {
        use td_doc::symmetry::PlaneAt;
        let mut state = EditorState::new(Default::default());
        let id = insert_brushes(&mut state, vec![trapezoid()])[0];
        let before = corners(&state, id);
        let steps = state.doc.history.undo_labels().count();
        state.edit_symmetry = symmetry_x(PlaneAt::Center, 0.0);

        let (left, right) = (DVec3::new(10.0, 40.0, 25.0), DVec3::new(65.0, 40.0, 25.0));
        assert_eq!(try_move_vertices(&mut state, &[left], DVec3::new(3.0, 2.0, -1.0)), Some(1));
        assert_corners(&corners(&state, id), &with_corners(before.clone(), &[(left, DVec3::new(13.0, 42.0, 24.0)), (right, DVec3::new(62.0, 42.0, 24.0))]));
        assert_eq!(state.doc.history.undo_labels().count(), steps + 1);
        assert_eq!(state.doc.history.undo_labels().next(), Some("Move Vertices"));

        state.doc.undo();
        assert_corners(&corners(&state, id), &before);
    }

    #[test]
    fn symmetry_off_and_a_missing_counterpart_move_the_corner_alone() {
        use td_doc::symmetry::PlaneAt;
        let mut state = EditorState::new(Default::default());
        let id = insert_brushes(&mut state, vec![trapezoid()])[0];
        let before = corners(&state, id);
        let (left, right) = (DVec3::new(10.0, 40.0, 25.0), DVec3::new(65.0, 40.0, 25.0));
        let delta = DVec3::new(3.0, 2.0, -1.0);

        assert_eq!(try_move_vertices(&mut state, &[left], delta), Some(0), "off moves one corner");
        assert_corners(&corners(&state, id), &with_corners(before.clone(), &[(left, left + delta)]));

        state.doc.undo();
        state.edit_symmetry = symmetry_x(PlaneAt::World, 0.0);
        assert_eq!(try_move_vertices(&mut state, &[right], delta), Some(0), "no counterpart across x 0, so it moves alone");
        assert_corners(&corners(&state, id), &with_corners(before.clone(), &[(right, right + delta)]));

        state.doc.undo();
        state.edit_symmetry = symmetry_x(PlaneAt::Custom, 37.5);
        assert_eq!(try_move_vertices(&mut state, &[right], delta), Some(1), "a typed plane finds it");
        state.doc.undo();
        state.edit_symmetry = symmetry_x(PlaneAt::Custom, 38.5);
        assert_eq!(try_move_vertices(&mut state, &[right], delta), Some(0), "a plane a unit off is out of reach");
    }

    #[test]
    fn a_selected_counterpart_moves_as_selected() {
        use td_doc::symmetry::PlaneAt;
        let mut state = EditorState::new(Default::default());
        let id = insert_brushes(&mut state, vec![trapezoid()])[0];
        let before = corners(&state, id);
        state.edit_symmetry = symmetry_x(PlaneAt::Center, 0.0);

        let (left, right) = (DVec3::new(10.0, 40.0, 25.0), DVec3::new(65.0, 40.0, 25.0));
        let delta = DVec3::new(3.0, 2.0, -1.0);
        assert_eq!(try_move_vertices(&mut state, &[left, right], delta), Some(0), "both selected, nothing follows twice");
        assert_corners(&corners(&state, id), &with_corners(before, &[(left, left + delta), (right, right + delta)]));
    }

    #[test]
    fn mirrored_pillars_follow_across_brushes_only_when_both_are_selected() {
        use td_doc::symmetry::PlaneAt;
        let pillar = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(32.0, 0.0, 0.0),
            DVec3::new(32.0, 0.0, 48.0),
            DVec3::new(0.0, 0.0, 48.0),
            DVec3::new(4.0, 64.0, 6.0),
            DVec3::new(30.0, 64.0, 2.0),
            DVec3::new(28.0, 64.0, 44.0),
            DVec3::new(2.0, 64.0, 50.0),
        ];
        let image: Vec<DVec3> = pillar.iter().map(|p| DVec3::new(200.0 - p.x, p.y, p.z)).collect();
        let mut state = EditorState::new(Default::default());
        let ids = insert_brushes(&mut state, vec![hull(&pillar), hull(&image)]);
        let (a, b) = (corners(&state, ids[0]), corners(&state, ids[1]));
        state.edit_symmetry = symmetry_x(PlaneAt::Custom, 100.0);

        let (top, top_image) = (DVec3::new(4.0, 64.0, 6.0), DVec3::new(196.0, 64.0, 6.0));
        assert_eq!(try_move_vertices(&mut state, &[top], DVec3::new(3.0, 5.0, -2.0)), Some(1));
        assert_corners(&corners(&state, ids[0]), &with_corners(a.clone(), &[(top, DVec3::new(7.0, 69.0, 4.0))]));
        assert_corners(&corners(&state, ids[1]), &with_corners(b.clone(), &[(top_image, DVec3::new(193.0, 69.0, 4.0))]));

        state.doc.undo();
        state.doc.select(|_, s| {
            s.clear();
            s.select_node(ids[0]);
        });
        assert_eq!(try_move_vertices(&mut state, &[top], DVec3::new(3.0, 5.0, -2.0)), Some(0), "an unselected brush has nothing to mirror");
        assert_corners(&corners(&state, ids[1]), &b);
    }

    /// Two boxes mirrored across x 72, the second one thin in z, so a pull that sticks out of the first lands inside
    /// the second.
    fn lopsided_pair(state: &mut EditorState) -> (NodeId, NodeId) {
        let boxes = [
            Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::splat(64.0)), "dev/grey").unwrap(),
            Brush::from_aabb(&Aabb::new(DVec3::new(80.0, 0.0, 0.0), DVec3::new(144.0, 64.0, 16.0)), "dev/grey").unwrap(),
        ];
        let ids = insert_brushes(state, boxes.to_vec());
        (ids[0], ids[1])
    }

    #[test]
    fn a_move_that_dents_the_mirrored_brush_is_refused_for_both() {
        use td_doc::symmetry::PlaneAt;
        let mut state = EditorState::new(Default::default());
        let (a, b) = lopsided_pair(&mut state);
        let (a_before, b_before) = (corners(&state, a), corners(&state, b));
        let (corner, delta) = (DVec3::new(64.0, 64.0, 0.0), DVec3::new(-24.0, -24.0, 10.0));

        assert_eq!(try_move_vertices(&mut state, &[corner], delta), Some(0), "alone the first box takes it");
        assert!(corners(&state, a) != a_before);
        assert_corners(&corners(&state, b), &b_before);
        state.doc.undo();

        state.edit_symmetry = symmetry_x(PlaneAt::Center, 0.0);
        let steps = state.doc.history.undo_labels().count();
        assert_eq!(try_move_vertices(&mut state, &[corner], delta), None, "the thin box would dent, so neither moves");
        assert_corners(&corners(&state, a), &a_before);
        assert_corners(&corners(&state, b), &b_before);
        assert_eq!(state.doc.history.undo_labels().count(), steps, "a refusal adds no undo step");
        assert_eq!(refusal_reason(&state, &[corner], delta), DENT_REASON, "the reason names the mirrored dent");

        assert!(!move_vertices(&mut state, &[DVec3::new(1.0, 2.0, 3.0)], DVec3::X), "a point off every brush still moves nothing");
    }

    #[test]
    fn pulling_a_dot_adds_the_mirrored_vertex_too() {
        use td_doc::symmetry::PlaneAt;
        let mut state = EditorState::new(Default::default());
        let id =
            insert_brushes(&mut state, vec![Brush::from_aabb(&Aabb::new(DVec3::new(10.0, 5.0, -30.0), DVec3::new(90.0, 45.0, 20.0)), "dev/grey").unwrap()])[0];
        state.edit_symmetry = symmetry_x(PlaneAt::Center, 0.0);

        assert_eq!(try_move_vertices(&mut state, &[DVec3::new(10.0, 45.0, -5.0)], DVec3::new(-6.0, 8.0, 0.0)), Some(1));
        let got = corners(&state, id);
        assert_eq!(got.len(), 10);
        for want in [DVec3::new(4.0, 53.0, -5.0), DVec3::new(96.0, 53.0, -5.0)] {
            assert!(got.iter().any(|v| v.distance(want) < 1e-6), "no corner at {want:?} in {got:?}");
        }
    }

    #[test]
    fn escape_that_clears_the_objects_drops_the_vertex_corners() {
        let (mut state, _) = state_with_brush();
        let mut tools = ToolSet::default();
        state.tool = ToolKind::Vertex;
        tools.vertex.selected = vec![DVec3::ZERO, DVec3::new(64.0, 0.0, 0.0)];
        tools.sync(&state);
        assert_eq!(tools.vertex.selected.len(), 2, "the corners of a selected brush stay");

        crate::commands::execute(&mut state, crate::commands::Action::Cancel, &egui::Context::default());
        tools.sync(&state);
        assert!(state.doc.selection.nodes.is_empty());
        assert_eq!((tools.vertex.selected.as_slice(), state.tool), (&[][..], ToolKind::Vertex));
    }

    #[test]
    fn switching_tools_mid_drag_closes_the_undo_step() {
        let (mut state, _) = state_with_brush();
        let mut tools = ToolSet::default();
        rotate_mid_drag(&mut tools, &mut state);
        let undo_before = state.doc.history.undo_labels().count();

        state.tool = ToolKind::Select;
        tools.sync(&state);
        assert!(tools.rotate.drag.is_none());
        tools.settle(&mut state);
        assert!(!state.doc.in_transaction(), "the rotate transaction is not left open");
        assert_eq!(state.doc.history.undo_labels().count(), undo_before + 1, "the rotation so far is kept as its own undo step");
    }

    /// Starts a shear drag the way `shear_input` does and slides the top side 32 along X inside it.
    fn shear_mid_drag(tools: &mut ToolSet, state: &mut EditorState) {
        state.tool = ToolKind::Shear;
        tools.sync(state);
        let base = Aabb::new(DVec3::ZERO, DVec3::splat(64.0));
        let plane = Plane::from_point_normal(DVec3::splat(32.0), DVec3::Z);
        let m = ops::shear_side(&base, 1, true, DVec3::new(32.0, 0.0, 0.0)).unwrap();
        state.doc.begin("Shear");
        tools.shear.drag = Some(ShearDrag { base, side: ShearSide { axis: 1, max: true, anchor: DVec3::splat(32.0) }, plane, start: DVec3::ZERO, matrix: m });
        let opts = state.opts();
        state.doc.edit("Shear", |map, s| ops::shear_selection(map, s, &m, opts));
    }

    #[test]
    fn escape_mid_shear_cancels_it() {
        let (mut state, id) = state_with_brush();
        let before = state.doc.map.brush(id).unwrap().clone();
        let mut tools = ToolSet::default();
        shear_mid_drag(&mut tools, &mut state);
        assert_ne!(state.doc.map.brush(id), Some(&before));
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        ctx.begin_pass(input);
        assert!(tools.keys(&ctx, &mut state), "Escape is taken by the drag");
        assert!(tools.shear.drag.is_none());
        assert!(!state.doc.in_transaction());
        assert_eq!(state.doc.map.brush(id), Some(&before), "the brush is back where it was");
        assert_eq!(state.tool, ToolKind::Shear, "the tool stays, only the drag ends");
    }

    #[test]
    fn switching_tools_mid_shear_closes_the_undo_step() {
        let (mut state, id) = state_with_brush();
        let mut tools = ToolSet::default();
        shear_mid_drag(&mut tools, &mut state);
        let undo_before = state.doc.history.undo_labels().count();

        state.tool = ToolKind::Select;
        tools.sync(&state);
        assert!(tools.shear.drag.is_none());
        tools.settle(&mut state);
        assert!(!state.doc.in_transaction());
        assert_eq!(state.doc.history.undo_labels().count(), undo_before + 1);
        assert!(state.doc.map.brush(id).unwrap().vertices.iter().any(|v| v.abs_diff_eq(DVec3::new(96.0, 64.0, 0.0), 1e-6)));
    }

    #[test]
    fn escape_mid_drag_cancels_it() {
        let (mut state, id) = state_with_brush();
        let before = state.doc.map.brush(id).unwrap().clone();
        let mut tools = ToolSet::default();
        rotate_mid_drag(&mut tools, &mut state);
        assert!(press(&mut tools, &mut state, Key::Escape), "Escape is taken by the drag");
        assert!(!state.doc.in_transaction());
        assert_eq!(state.doc.map.brush(id), Some(&before), "the brush is back where it was");
        assert_eq!(state.tool, ToolKind::Rotate, "the tool stays, only the drag ends");
    }

    #[test]
    fn escape_cancels_an_extrude_also_while_the_view_flies() {
        for flying in [false, true] {
            let (mut state, id) = state_with_brush();
            state.doc.edit("select", |_, s| s.select_face(id, 0));
            let mut tools = ToolSet::default();
            tools.extrude.start(&mut state);
            assert!(tools.extrude.active() && state.doc.in_transaction());
            tools.flying = flying;
            assert!(press(&mut tools, &mut state, Key::Escape), "flying {flying}: Escape is taken by the extrude");
            assert!(!tools.extrude.active() && !state.doc.in_transaction(), "flying {flying}");
            assert_eq!(state.doc.selection.faces, std::collections::BTreeSet::from([(id, 0)]), "flying {flying}");
        }
    }

    #[test]
    fn undo_or_a_tool_switch_mid_extrude_closes_it() {
        let (mut state, id) = state_with_brush();
        state.doc.edit("select", |_, s| s.select_face(id, 0));
        let mut tools = ToolSet::default();
        tools.sync(&state);
        tools.extrude.start(&mut state);
        state.doc.cancel();
        tools.settle(&mut state);
        assert!(!tools.extrude.active(), "the transaction was closed under it");

        tools.extrude.start(&mut state);
        state.tool = ToolKind::Clip;
        tools.sync(&state);
        assert!(!tools.extrude.active());
        tools.settle(&mut state);
        assert!(!state.doc.in_transaction());
    }

    #[test]
    fn a_document_swap_forgets_node_ids() {
        let (mut state, id) = state_with_brush();
        let mut tools = ToolSet::default();
        tools.path.last = Some(id);
        tools.mesh.selection.insert(id, Default::default());
        state.scene_reset = true;
        tools.settle(&mut state);
        assert!(tools.path.last.is_none(), "the next path corner does not link into another map's node");
        assert!(tools.mesh.selection.is_empty());
    }

    /// A flat 4096 unit terrain around the origin and a camera looking straight down at it.
    fn terrain_under_camera() -> (EditorState, NodeId, Camera) {
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let terrain = td_geom::Terrain::new(DVec3::new(-2048.0, 0.0, -2048.0), [65, 65], 64.0, "dev/grey");
        let id = state.doc.edit("terrain", |m, _| m.insert(layer, td_doc::NodeKind::Terrain(terrain)));
        state.tool = ToolKind::Sculpt;
        let mut cam = Camera::new(ViewKind::Perspective);
        cam.position = DVec3::new(0.0, 3000.0, 0.0);
        cam.pitch = (-89.0f64).to_radians();
        (state, id, cam)
    }

    fn input_rect() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.0))
    }

    #[test]
    fn only_the_view_a_stroke_started_in_dabs() {
        let (mut state, id, cam) = terrain_under_camera();
        state.sculpt.radius = 48.0;
        let mut top = Camera::new(ViewKind::Top);
        top.center = DVec3::new(900.0, 0.0, -700.0);
        top.zoom = 0.5;
        let mut tools = ToolSet::default();
        let ctx = egui::Context::default();
        let left = input_rect();
        let right = left.translate(egui::vec2(left.width(), 0.0));
        let pos = left.center();
        for pressed in [None, Some(true), None, None, None, None, Some(false)] {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if let Some(pressed) = pressed {
                events.push(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            }

            let raw = egui::RawInput { screen_rect: Some(left.union(right)), events, ..Default::default() };
            let mut output = ctx.run_ui(raw, |ui| {
                for (cam, rect) in [(&cam, left), (&top, right)] {
                    let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
                    let hover = response.hover_pos();
                    tools.viewport_input(ui, &response, cam, rect, hover, &mut state);
                }
            });
            output.textures_delta.clear();
        }

        let terrain = state.doc.map.terrain(id).unwrap();
        let [w, d] = terrain.resolution;
        let raised: Vec<DVec3> = (0..d).flat_map(|j| (0..w).map(move |i| (i, j))).map(|(i, j)| terrain.vertex(i, j)).filter(|v| v.y > 0.0).collect();
        assert!(!raised.is_empty(), "the stroke raised nothing");
        assert!(raised.iter().all(|v| v.x.abs() < 200.0 && v.z.abs() < 200.0), "the top view dabbed too: {raised:?}");
    }

    #[test]
    fn an_unhole_stroke_can_start_over_a_hole() {
        let (mut state, id, cam) = terrain_under_camera();
        state.doc.edit("cut", |m, _| m.terrain_mut(id).unwrap().set_holes_rect(glam::DVec2::splat(-300.0), glam::DVec2::splat(300.0), true));
        assert!(state.doc.map.terrain(id).unwrap().is_hole(32, 32));
        state.sculpt.mode = td_doc::terrain::SculptMode::Unhole;
        state.sculpt.radius = 100.0;
        let mut tools = ToolSet::default();
        let ctx = egui::Context::default();
        let pos = input_rect().center();
        for pressed in [None, Some(true), None, Some(false)] {
            view_frame(&ctx, &mut tools, &mut state, &cam, pos, pressed);
        }

        let t = state.doc.map.terrain(id).unwrap();
        assert!(!t.is_hole(32, 32) && t.is_hole(36, 36), "the cells under the brush are filled, the rest of the hole stays");
    }

    #[test]
    fn add_tile_clicks_beside_a_terrain_grow_it_and_shift_clicks_take_the_tile_back() {
        let (mut state, id, _) = terrain_under_camera();
        state.sculpt.mode = td_doc::terrain::SculptMode::AddTile;
        let mut top = Camera::new(ViewKind::Top);
        top.center = DVec3::new(2300.0, 0.0, 100.0);
        let mut tools = ToolSet::default();
        let ctx = egui::Context::default();
        let pos = input_rect().center();
        for modifiers in [egui::Modifiers::NONE, egui::Modifiers::SHIFT] {
            for pressed in [None, Some(true), None, Some(false)] {
                view_frame_with(&ctx, &mut tools, &mut state, &top, pos, pressed, modifiers);
            }

            let t = state.doc.map.terrain(id).unwrap();
            if modifiers.shift {
                assert_eq!((t.cells(), t.holes.is_empty()), ([64, 64], true));
            } else {
                assert_eq!(t.cells(), [96, 64]);
                assert!(t.is_hole(80, 10) && !t.is_hole(80, 40));
            }
        }

        assert!(state.doc.undo().is_some() && state.doc.map.terrain(id).unwrap().cells() == [96, 64], "each click is its own undo step");
    }

    /// Runs one frame of the viewport tools over a 400 by 400 view with the pointer at `pos`.
    fn view_frame(ctx: &egui::Context, tools: &mut ToolSet, state: &mut EditorState, cam: &Camera, pos: Pos2, pressed: Option<bool>) {
        view_frame_with(ctx, tools, state, cam, pos, pressed, egui::Modifiers::NONE);
    }

    fn view_frame_with(
        ctx: &egui::Context,
        tools: &mut ToolSet,
        state: &mut EditorState,
        cam: &Camera,
        pos: Pos2,
        pressed: Option<bool>,
        modifiers: egui::Modifiers,
    ) {
        let rect = input_rect();
        let mut events = vec![egui::Event::ModifiersChanged(modifiers), egui::Event::PointerMoved(pos)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers });
        }

        let raw = egui::RawInput { screen_rect: Some(rect), events, ..Default::default() };
        let mut output = ctx.run_ui(raw, |ui| {
            let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
            let hover = response.hover_pos();
            tools.viewport_input(ui, &response, cam, rect, hover, state);
        });
        output.textures_delta.clear();
    }

    fn click_at(tools: &mut ToolSet, state: &mut EditorState, cam: &Camera, pos: Pos2) {
        let ctx = egui::Context::default();
        for pressed in [None, Some(true), Some(false), None] {
            view_frame(&ctx, tools, state, cam, pos, pressed);
        }
    }

    fn click(tools: &mut ToolSet, state: &mut EditorState, cam: &Camera) {
        click_at(tools, state, cam, input_rect().center());
    }

    fn press(tools: &mut ToolSet, state: &mut EditorState, key: Key) -> bool {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        ctx.begin_pass(input);
        tools.keys(&ctx, state)
    }

    fn press_chord(tools: &mut ToolSet, state: &mut EditorState, key: Key, modifiers: egui::Modifiers) -> bool {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
        ctx.begin_pass(input);
        let returned = tools.keys(&ctx, state);
        returned || !ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Key { key: k, pressed: true, .. } if *k == key)))
    }

    /// An editor with the tool active and what the row's condition asks for, on an off-origin brush and mesh.
    fn tool_key_setup(tool: ToolKind, when: crate::tool_keys::When) -> (ToolSet, EditorState) {
        use crate::tool_keys::When;
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let mesh = td_geom::mesh_shapes::cuboid(&Aabb::new(DVec3::new(-300.5, 4.0, 90.0), DVec3::new(-220.5, 36.0, 118.0)), "dev/grey");
        let (brush, mesh) = state.doc.edit("add", |m, s| {
            s.clear();
            (m.insert(layer, td_doc::NodeKind::Brush(trapezoid())), m.insert(layer, td_doc::NodeKind::Mesh(mesh)))
        });
        state.tool = tool;
        let mut tools = ToolSet::default();
        tools.sync(&state);
        match when {
            When::Always => {}
            When::ClipPoint => tools.clip.points.push(DVec3::new(10.0, 4.0, -7.0)),
            When::PolygonPoint => tools.polygon.points.push(DVec3::new(10.0, 4.0, -7.0)),
            When::VertexSelected => tools.vertex.selected.push(DVec3::new(-20.0, 0.0, 10.0)),
            When::PathStarted => tools.path.last = Some(brush),
            When::FacesInReach => select_only(&mut state, &[brush]),
            When::SelectionBox => {
                let (a, b) = (DVec3::splat(-1000.0), DVec3::splat(1000.0));
                state.selection_box = Some(crate::selection_box::SelectionBox { view: crate::camera::ViewKind::Top, a, b, add: false });
            }
            When::MeshSelected | When::MeshGizmo | When::MeshTransform | When::MeshLoopCut => {
                select_only(&mut state, &[mesh]);
                tools.mesh.component = crate::mesh_tool::Component::Face;
                tools.mesh.select_all(&state);
                match when {
                    When::MeshGizmo => tools.mesh.gizmo = true,
                    When::MeshTransform => tools.mesh.start_transform(crate::mesh_tool::TransformKind::Grab, None, &mut state),
                    When::MeshLoopCut => tools.mesh.modal = Some(crate::mesh_tool::Modal::LoopCut { hover: None, cuts: 2 }),
                    _ => {}
                }
            }
        }

        (tools, state)
    }

    #[test]
    fn every_tool_key_row_is_read_by_its_tool() {
        use crate::tool_keys::tool_key_rows;
        assert!(tool_key_rows().len() > 40);
        for row in tool_key_rows() {
            let (mut tools, mut state) = tool_key_setup(row.tool, row.when);
            assert!(
                press_chord(&mut tools, &mut state, row.key, row.modifiers),
                "{:?} {} {:?} is listed but the tool leaves it",
                row.tool,
                row.chord(),
                row.when
            );
        }
    }

    #[test]
    fn enter_confirms_a_waiting_selection_box() {
        use crate::tool_keys::{When, preset_rows};
        let row = preset_rows("hammer").iter().find(|r| r.when == When::SelectionBox).expect("the Hammer preset lists Enter");
        let (mut tools, mut state) = tool_key_setup(row.tool, row.when);
        assert!(press_chord(&mut tools, &mut state, row.key, row.modifiers));
        assert!(state.selection_box.is_none());
        assert_eq!(state.doc.selection.nodes.len(), 2, "the brush and the mesh lie inside");
    }

    #[test]
    fn a_plain_key_the_tool_does_not_list_is_left_alone() {
        use crate::tool_keys::{When, tool_key_rows};
        let ready = |tool| match tool {
            ToolKind::Clip => When::ClipPoint,
            ToolKind::Polygon => When::PolygonPoint,
            ToolKind::Vertex => When::VertexSelected,
            ToolKind::Path => When::PathStarted,
            ToolKind::Texture => When::FacesInReach,
            ToolKind::Mesh => When::MeshSelected,
            _ => When::Always,
        };
        let plain = |k: &Key| {
            k.name().len() == 1 || matches!(k, Key::Enter | Key::Tab | Key::Backspace | Key::Delete | Key::Escape | Key::Space | Key::Plus | Key::Minus)
        };
        let arrows = [Key::ArrowLeft, Key::ArrowRight, Key::ArrowUp, Key::ArrowDown];
        for tool in ToolKind::all() {
            let listed = |key: Key, extra: When| {
                tool_key_rows().iter().any(|r| {
                    r.tool == tool
                        && r.key == key
                        && r.modifiers == egui::Modifiers::NONE
                        && (r.when == ready(tool) || r.when == When::Always || r.when == extra)
                })
            };
            for key in Key::ALL.iter().copied().filter(plain).chain(arrows) {
                let (mut tools, mut state) = tool_key_setup(tool, ready(tool));
                let taken = press_chord(&mut tools, &mut state, key, egui::Modifiers::NONE);
                assert_eq!(taken, listed(key, When::Always), "{tool:?} {key:?}");
            }
        }

        let (mut tools, mut state) = tool_key_setup(ToolKind::Clip, When::ClipPoint);
        assert!(!press_chord(&mut tools, &mut state, Key::P, egui::Modifiers::NONE), "P belongs to the Mesh tool");
        assert_eq!(tools.clip.points.len(), 1);
    }

    fn polygon_tools(state: &mut EditorState) -> ToolSet {
        let mut tools = ToolSet::default();
        state.tool = ToolKind::Polygon;
        tools.sync(state);
        tools
    }

    fn centered_view(kind: ViewKind) -> Camera {
        let mut cam = Camera::new(kind);
        cam.center = DVec3::ZERO;
        cam.zoom = 1.0;
        cam
    }

    fn corner(tools: &mut ToolSet, state: &mut EditorState, cam: &Camera, world: [f64; 3]) {
        let pos = cam.project(input_rect(), DVec3::from(world)).expect("the corner is in view");
        click_at(tools, state, cam, pos);
    }

    #[test]
    fn a_concave_polygon_drawn_in_the_top_view_is_one_undo_step() {
        let mut state = EditorState::new(Default::default());
        let mut tools = polygon_tools(&mut state);
        let top = centered_view(ViewKind::Top);
        let undo_before = state.doc.history.undo_labels().count();
        corner(&mut tools, &mut state, &top, [-64.0, 0.0, -64.0]);
        corner(&mut tools, &mut state, &top, [64.0, 0.0, -64.0]);
        corner(&mut tools, &mut state, &top, [32.0, 0.0, 32.0]);
        assert!(press(&mut tools, &mut state, Key::Backspace), "Backspace is taken with corners placed");
        assert_eq!(tools.polygon.points.len(), 2, "Backspace removes the last corner");
        for c in [[64.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 64.0], [-64.0, 0.0, 64.0]] {
            corner(&mut tools, &mut state, &top, c);
        }

        assert!(press(&mut tools, &mut state, Key::Enter));

        let made: Vec<NodeId> = state.doc.selection.nodes.iter().copied().collect();
        assert_eq!(made.len(), 2, "the L is cut into two brushes, both selected");
        let b = state.doc.map.bounds_of(made.iter().copied());
        assert_eq!((b.min, b.max), (DVec3::new(-64.0, 0.0, -64.0), DVec3::new(64.0, crate::viewport::FIRST_BRUSH_DEPTH, 64.0)));
        assert_eq!(state.doc.history.undo_labels().count(), undo_before + 1);
        assert_eq!(state.doc.history.undo_labels().next(), Some("Create Polygon Brush"));
        assert!(tools.polygon.points.is_empty(), "the tool is ready for the next outline");
    }

    #[test]
    fn shift_on_the_first_corner_turns_the_next_outlines_into_holes() {
        let mut state = EditorState::new(Default::default());
        let mut tools = polygon_tools(&mut state);
        let top = centered_view(ViewKind::Top);
        let shift_corner = |tools: &mut ToolSet, state: &mut EditorState, world: [f64; 3]| {
            let pos = top.project(input_rect(), DVec3::from(world)).expect("the corner is in view");
            let ctx = egui::Context::default();
            for pressed in [None, Some(true), Some(false), None] {
                view_frame_with(&ctx, tools, state, &top, pos, pressed, egui::Modifiers::SHIFT);
            }
        };
        for c in [[-96.0, 0.0, -96.0], [96.0, 0.0, -96.0], [96.0, 0.0, 96.0], [-96.0, 0.0, 96.0]] {
            corner(&mut tools, &mut state, &top, c);
        }

        shift_corner(&mut tools, &mut state, [-96.0, 0.0, -96.0]);
        assert_eq!((tools.polygon.loops.len(), tools.polygon.points.len()), (1, 0), "the outline is kept for holes");
        for c in [[-32.0, 0.0, -32.0], [32.0, 0.0, -32.0], [32.0, 0.0, 32.0]] {
            corner(&mut tools, &mut state, &top, c);
        }

        assert!(press(&mut tools, &mut state, Key::Backspace));
        assert!(press(&mut tools, &mut state, Key::Backspace));
        assert!(press(&mut tools, &mut state, Key::Backspace));
        assert!(press(&mut tools, &mut state, Key::Backspace), "Backspace reopens the outline");
        assert_eq!((tools.polygon.loops.len(), tools.polygon.points.len()), (0, 4));
        shift_corner(&mut tools, &mut state, [-96.0, 0.0, -96.0]);
        for c in [[-32.0, 0.0, -32.0], [32.0, 0.0, -32.0], [32.0, 0.0, 32.0], [-32.0, 0.0, 32.0]] {
            corner(&mut tools, &mut state, &top, c);
        }

        let undo_before = state.doc.history.undo_labels().count();
        corner(&mut tools, &mut state, &top, [-32.0, 0.0, -32.0]);
        assert!(!state.status_failed(), "{}", state.status);
        let made: Vec<NodeId> = state.doc.selection.nodes.iter().copied().collect();
        assert!(made.len() >= 4, "a square ring takes at least four convex brushes, got {}", made.len());
        let volume: f64 = made.iter().map(|id| state.doc.map.brush(*id).unwrap().volume()).sum();
        assert!((volume - (192.0 * 192.0 - 64.0 * 64.0) * crate::viewport::FIRST_BRUSH_DEPTH).abs() < 1e-3);
        assert_eq!(state.doc.history.undo_labels().count(), undo_before + 1);
        assert!(tools.polygon.is_empty());
    }

    #[test]
    fn wall_mode_turns_the_corners_into_walls_open_on_enter_and_closed_on_the_first_corner() {
        use td_geom::outline::WallAlign;
        let mut state = EditorState::new(Default::default());
        state.polygon_mode = crate::polygon_tool::PolygonMode::Walls;
        let mut tools = polygon_tools(&mut state);
        let top = centered_view(ViewKind::Top);
        corner(&mut tools, &mut state, &top, [-64.0, 0.0, -64.0]);
        corner(&mut tools, &mut state, &top, [64.0, 0.0, -64.0]);
        assert!(press(&mut tools, &mut state, Key::Enter));
        let group = state.doc.selection.nodes.iter().copied().collect::<Vec<_>>();
        assert!(
            matches!(group.as_slice(), [g] if matches!(state.doc.map.get(*g).map(|n| &n.kind), Some(td_doc::NodeKind::Group(_)))),
            "the walls come in a selected group"
        );
        assert_eq!(state.doc.map.get(group[0]).unwrap().children.len(), 1, "two corners make one wall");
        let b = state.doc.map.bounds_of(group.iter().copied());
        assert_eq!((b.max.x, b.max.y - b.min.y), (64.0, 128.0), "the open ends are square and the walls 128 tall");
        assert_eq!(b.max.z - b.min.z, 16.0);
        assert_eq!(state.doc.history.undo_labels().next(), Some("Create Walls"));
        state.walls.align = WallAlign::Center;

        for c in [[-64.0, 0.0, -64.0], [64.0, 0.0, -64.0], [64.0, 0.0, 64.0], [-64.0, 0.0, -64.0]] {
            corner(&mut tools, &mut state, &top, c);
        }

        let group = *state.doc.selection.nodes.iter().next().unwrap();
        assert_eq!(state.doc.map.get(group).unwrap().children.len(), 3, "clicking the first corner closes the path into three walls");
        assert!(tools.polygon.points.is_empty());

        state.polygon_mode = crate::polygon_tool::PolygonMode::Trim;
        for c in [[-64.0, 0.0, -64.0], [64.0, 0.0, -64.0], [64.0, 0.0, 64.0], [-64.0, 0.0, 64.0], [-64.0, 0.0, -64.0]] {
            corner(&mut tools, &mut state, &top, c);
        }

        let group = *state.doc.selection.nodes.iter().next().unwrap();
        assert_eq!(state.doc.map.get(group).unwrap().children.len(), 4, "a closed skirting has a brush per side");
        assert_eq!(state.doc.history.undo_labels().next(), Some("Create Trim"));
        let b = state.doc.map.bounds_of([group]);
        assert_eq!((b.min.x, b.max.x, b.max.y - b.min.y), (-64.0, 64.0, 12.0), "the skirting stays inside the clicked room");

        state.polygon_mode = crate::polygon_tool::PolygonMode::Strip;
        state.strip.width = 16.0;
        for c in [[-128.0, 0.0, 128.0], [128.0, 0.0, 128.0]] {
            corner(&mut tools, &mut state, &top, c);
        }

        assert!(press(&mut tools, &mut state, Key::Enter));
        let road = *state.doc.selection.nodes.iter().next().unwrap();
        for c in [[0.0, 0.0, 128.0], [0.0, 0.0, 192.0]] {
            corner(&mut tools, &mut state, &top, c);
        }

        assert!(press(&mut tools, &mut state, Key::Enter));
        assert_eq!(state.doc.selection.nodes.iter().copied().collect::<Vec<_>>(), vec![road], "the branch joins the road's group");
        assert_eq!(state.doc.map.get(road).unwrap().children.len(), 2);
        assert_eq!(state.doc.map.bounds_of([road]).max.z, 192.0);
        assert_eq!(state.doc.history.undo_labels().next(), Some("Create Strip"));
    }

    #[test]
    fn the_first_corner_closes_a_polygon_and_escape_drops_it() {
        let mut state = EditorState::new(Default::default());
        let mut tools = polygon_tools(&mut state);
        let top = centered_view(ViewKind::Top);
        for c in [[0.0, 0.0, 0.0], [64.0, 0.0, 0.0], [0.0, 0.0, 64.0], [0.0, 0.0, 0.0]] {
            corner(&mut tools, &mut state, &top, c);
        }

        assert_eq!(state.doc.selection.nodes.len(), 1, "clicking the first corner again closes the triangle");
        assert!(tools.polygon.points.is_empty());

        let brushes = state.doc.map.brush_count();
        corner(&mut tools, &mut state, &top, [128.0, 0.0, 0.0]);
        corner(&mut tools, &mut state, &top, [160.0, 0.0, 0.0]);
        assert!(press(&mut tools, &mut state, Key::Escape));
        assert!(tools.polygon.points.is_empty());
        assert_eq!(state.status, "Cancelled");
        assert!(!press(&mut tools, &mut state, Key::Escape), "with nothing placed Escape is left to the rest of the editor");
        assert!(press(&mut tools, &mut state, Key::Enter));
        assert_eq!(state.status, "Place at least three corners first");
        assert_eq!(state.doc.map.brush_count(), brushes);
    }

    #[test]
    fn a_polygon_stays_in_the_view_it_was_started_in() {
        let mut state = EditorState::new(Default::default());
        let mut tools = polygon_tools(&mut state);
        let (top, front) = (centered_view(ViewKind::Top), centered_view(ViewKind::Front));
        corner(&mut tools, &mut state, &top, [0.0, 0.0, 0.0]);
        corner(&mut tools, &mut state, &front, [64.0, 64.0, 0.0]);
        assert_eq!(tools.polygon.points.len(), 1);
        assert_eq!(state.status, "Finish the outline in the view where you started it");

        assert!(press(&mut tools, &mut state, Key::Backspace));
        corner(&mut tools, &mut state, &front, [64.0, 64.0, 0.0]);
        assert_eq!(tools.polygon.points.len(), 1, "removing the last corner frees the outline to start in another view");
    }

    #[test]
    fn a_3d_polygon_starts_on_the_surface_under_the_pointer() {
        let (mut state, block) = state_with_brush();
        let mut tools = polygon_tools(&mut state);
        let mut cam = Camera::new(ViewKind::Perspective);
        cam.position = DVec3::new(32.0, 600.0, 32.0);
        cam.pitch = (-89.0f64).to_radians();
        for c in [[16.0, 64.0, 16.0], [48.0, 64.0, 16.0], [48.0, 64.0, 48.0]] {
            corner(&mut tools, &mut state, &cam, c);
        }

        assert!(press(&mut tools, &mut state, Key::Enter));

        let made: Vec<NodeId> = state.doc.selection.nodes.iter().copied().collect();
        assert!(made.len() == 1 && made[0] != block);
        let b = state.doc.map.bounds_of(made.iter().copied());
        assert_eq!((b.min, b.max), (DVec3::new(16.0, 64.0, 16.0), DVec3::new(48.0, 64.0 + state.grid, 48.0)), "it stands on the block's top");
    }

    #[test]
    fn a_3d_polygon_click_on_nothing_says_so() {
        let mut state = EditorState::new(Default::default());
        let mut tools = polygon_tools(&mut state);
        let mut cam = Camera::new(ViewKind::Perspective);
        cam.position = DVec3::new(0.0, 256.0, 0.0);
        cam.pitch = 80.0f64.to_radians();
        click(&mut tools, &mut state, &cam);
        assert!(tools.polygon.points.is_empty());
        assert_eq!(state.status, "Click a surface or the ground to start the outline");
    }

    #[test]
    fn a_sculpt_click_raises_a_visible_bump_and_a_no_op_stroke_leaves_no_undo_step() {
        let (mut state, id, cam) = terrain_under_camera();
        let mut tools = ToolSet::default();
        state.sculpt.radius = 256.0;
        click(&mut tools, &mut state, &cam);
        let top = state.doc.map.terrain(id).unwrap().heights.iter().copied().fold(0.0f32, f32::max);
        let expected = timed_sculpt_brush(state.sculpt, MIN_STROKE_SECONDS).strength as f32;
        assert!(top >= expected * 0.9, "a click raises the ground by a quarter second of holding, {top} of {expected}");
        assert!(expected >= 8.0, "{expected}");
        assert_eq!(state.doc.history.undo_labels().next(), Some("Sculpt"));

        let steps = state.doc.history.undo_labels().count();
        state.sculpt.mode = td_doc::terrain::SculptMode::Flatten;
        state.doc.edit("level", |m, _| m.terrain_mut(id).unwrap().heights.fill(0.0));
        let steps = steps + 1;
        click(&mut tools, &mut state, &cam);
        assert_eq!(state.doc.history.undo_labels().count(), steps, "flattening flat ground changes nothing and records nothing");
    }

    use td_doc::symmetry::{Symmetry, SymmetryAxes, SymmetryCenter};

    /// An off origin 65 by 65 vertex terrain with a slanted start, its middle at x 547.5 and z 1047.75, under a camera
    /// that sculpts about 400 units short of the middle in x and 750 in z.
    fn slanted_terrain_under_camera() -> (EditorState, NodeId, Camera) {
        let (mut state, id, mut cam) = terrain_under_camera();
        state.doc.edit("move", |m, _| {
            let t = m.terrain_mut(id).unwrap();
            t.origin = DVec3::new(-1500.5, 20.0, -1000.25);
            for j in 0..65 {
                for i in 0..65 {
                    let k = t.index(i, j);
                    t.heights[k] = 3.0 + 0.5 * i as f32 + 0.25 * j as f32;
                }
            }
        });
        state.sculpt.radius = 128.0;
        cam.position = DVec3::new(-400.0, 3000.0, 300.0);
        (state, id, cam)
    }

    fn heights_of(state: &EditorState, id: NodeId) -> Vec<f32> {
        state.doc.map.terrain(id).unwrap().heights.clone()
    }

    /// Raise per vertex, its total and the weighted middle of where it landed in x and z.
    fn raised(state: &EditorState, id: NodeId, start: &[f32]) -> (Vec<f32>, f64, DVec3) {
        let t = state.doc.map.terrain(id).unwrap();
        let delta: Vec<f32> = t.heights.iter().zip(start).map(|(a, b)| a - b).collect();
        let (mut sum, mut mid) = (0.0f64, DVec3::ZERO);
        for j in 0..65 {
            for i in 0..65 {
                let w = delta[t.index(i, j)] as f64;
                sum += w;
                mid += DVec3::new(t.origin.x + i as f64 * 64.0, 0.0, t.origin.z + j as f64 * 64.0) * w;
            }
        }

        (delta, sum, mid / sum)
    }

    fn sculpt_click_with(symmetry: Symmetry) -> (EditorState, NodeId, Vec<f32>) {
        let (mut state, id, cam) = slanted_terrain_under_camera();
        state.symmetry = symmetry;
        let start = heights_of(&state, id);
        click(&mut ToolSet::default(), &mut state, &cam);
        (state, id, start)
    }

    fn sym(axes: SymmetryAxes, center: SymmetryCenter) -> Symmetry {
        Symmetry { axes, center, ..Symmetry::OFF }
    }

    #[test]
    fn a_sculpt_click_with_symmetry_raises_every_side_in_one_undo_step() {
        let (off, id, start) = sculpt_click_with(Symmetry::OFF);
        let (_, total_one, at) = raised(&off, id, &start);
        assert!(total_one > 0.0);

        for (axes, flip_x, flip_z) in [(SymmetryAxes::X, true, false), (SymmetryAxes::Z, false, true), (SymmetryAxes::Xz, true, true)] {
            let (mut state, id, start) = sculpt_click_with(sym(axes, SymmetryCenter::Terrain));
            let (delta, total, mid) = raised(&state, id, &start);
            let count = 1 << (flip_x as u32 + flip_z as u32);
            assert!((total - total_one * count as f64).abs() < 1e-3 * total, "{axes:?}: {count} dabs of {total_one} raise {total}");
            for j in 0..65usize {
                for i in 0..65usize {
                    let (a, b) = (if flip_x { 64 - i } else { i }, if flip_z { 64 - j } else { j });
                    let (p, q) = (delta[j * 65 + i], delta[b * 65 + a]);
                    assert!((p - q).abs() < 1e-3, "{axes:?}: vertex {i},{j} raised {p}, its image {a},{b} raised {q}");
                }
            }

            assert!((mid.x - if flip_x { 547.5 } else { at.x }).abs() < 1.0, "{axes:?}: x middle {}", mid.x);
            assert!((mid.z - if flip_z { 1047.75 } else { at.z }).abs() < 1.0, "{axes:?}: z middle {}", mid.z);
            let before = state.doc.history.undo_labels().count();
            assert_eq!(state.doc.history.undo_labels().next(), Some("Sculpt"));
            state.doc.undo();
            assert_eq!(state.doc.history.undo_labels().count(), before - 1, "one step");
            assert_eq!(heights_of(&state, id), start, "{axes:?}: one undo restores every side");
        }
    }

    #[test]
    fn sculpt_symmetry_off_changes_one_side_only() {
        let (state, id, start) = sculpt_click_with(Symmetry::OFF);
        let (delta, _, at) = raised(&state, id, &start);
        assert!(at.x < 547.5 && at.z < 1047.75, "the click lands on the near corner side");
        let far = (0..65usize).flat_map(|j| (0..65usize).map(move |i| (i, j))).filter(|(i, j)| *i > 32 || *j > 32);
        assert!(far.into_iter().all(|(i, j)| delta[j * 65 + i] == 0.0), "nothing past the middle moves");
    }

    #[test]
    fn symmetry_planes_sit_on_the_terrain_middle_the_world_origin_or_a_typed_spot() {
        let custom = Symmetry { axes: SymmetryAxes::Xz, center: SymmetryCenter::Custom, x: 210.5, z: -64.25 };
        let cases = [
            (sym(SymmetryAxes::Xz, SymmetryCenter::Terrain), DVec3::new(547.5, 0.0, 1047.75)),
            (sym(SymmetryAxes::Xz, SymmetryCenter::World), DVec3::ZERO),
            (custom, DVec3::new(210.5, 0.0, -64.25)),
        ];
        for (symmetry, want) in cases {
            let (state, id, start) = sculpt_click_with(symmetry);
            let (_, _, mid) = raised(&state, id, &start);
            assert!((mid.x - want.x).abs() < 2.0 && (mid.z - want.z).abs() < 2.0, "{:?}: the four dabs center on {mid:?}", symmetry.center);
        }
    }

    #[test]
    fn a_sculpt_click_mirrors_across_the_middle_of_a_displacement() {
        let mut brush = Brush::from_aabb(&Aabb::new(DVec3::new(-40.5, 0.0, 25.25), DVec3::new(87.5, 16.0, 153.25)), "dev/grey").unwrap();
        let top = brush.faces.iter().position(|f| f.plane.normal.y > 0.9).unwrap();
        brush.faces[top].data.disp = Some(td_geom::displacement::Displacement::new(3));
        let make = || {
            let mut state = EditorState::new(Default::default());
            let layer = state.doc.map.default_layer();
            let id = state.doc.edit("brush", |m, _| m.insert(layer, td_doc::NodeKind::Brush(brush.clone())));
            state.tool = ToolKind::Sculpt;
            state.sculpt.radius = 24.0;
            (state, id)
        };
        let mut cam = Camera::new(ViewKind::Perspective);
        cam.position = DVec3::new(-16.5, 600.0, 89.25);
        cam.pitch = (-89.0f64).to_radians();

        let raise = |state: &mut EditorState, id: NodeId, axes| {
            state.symmetry = sym(axes, SymmetryCenter::Terrain);
            click(&mut ToolSet::default(), state, &cam);
            let b = state.doc.map.brush(id).unwrap();
            (td_geom::displacement::grid(b, top).unwrap().base, b.faces[top].data.disp.as_ref().unwrap().heights.clone())
        };

        let (mut plain, plain_id) = make();
        let (base, one) = raise(&mut plain, plain_id, SymmetryAxes::Off);
        let left: Vec<usize> = (0..base.len()).filter(|k| base[*k].x < 23.5).collect();
        let right: Vec<usize> = (0..base.len()).filter(|k| base[*k].x > 23.5).collect();
        assert!(left.iter().any(|k| one[*k] > 0.0));
        assert!(right.iter().all(|k| one[*k] == 0.0), "without symmetry only the dab's side rises");

        let (mut state, id) = make();
        let (_, both) = raise(&mut state, id, SymmetryAxes::X);
        for k in 0..base.len() {
            let want = DVec3::new(47.0 - base[k].x, base[k].y, base[k].z);
            let image = (0..base.len()).find(|m| (base[*m] - want).length() < 1e-6).unwrap();
            assert!((both[k] - both[image]).abs() < 1e-4, "vertex {k} at {:?}: {} against its image {}", base[k], both[k], both[image]);
        }

        assert!(right.iter().any(|k| both[*k] > 0.0), "the far side rises too");
        assert!(left.iter().all(|k| (both[*k] - one[*k]).abs() < 1e-4), "the dab's own side is the plain dab");
        state.doc.undo();
        let b = state.doc.map.brush(id).unwrap();
        assert!(b.faces[top].data.disp.as_ref().unwrap().heights.iter().all(|h| *h == 0.0), "one undo restores both sides");
    }

    #[test]
    fn vertex_paint_ignores_symmetry() {
        let run = |symmetry: Symmetry| {
            let (mut state, id) = state_with_brush();
            state.tool = ToolKind::Paint;
            state.paint_color = [1.0, 0.0, 0.0, 1.0];
            state.sculpt.radius = 20.0;
            state.symmetry = symmetry;
            let mut cam = Camera::new(ViewKind::Perspective);
            cam.position = DVec3::new(8.0, 600.0, 8.0);
            cam.pitch = (-89.0f64).to_radians();
            click(&mut ToolSet::default(), &mut state, &cam);
            state.doc.map.brush(id).unwrap().faces.iter().map(|f| f.data.colors.clone()).collect::<Vec<_>>()
        };

        let plain = run(Symmetry::OFF);
        assert!(plain.iter().flatten().any(|c| c[1] < 1.0), "the click painted something");
        assert!(plain.iter().flatten().any(|c| c[1] == 1.0), "and left the far corners alone");
        let planes = Symmetry { axes: SymmetryAxes::Xz, center: SymmetryCenter::Custom, x: 32.0, z: 32.0 };
        assert_eq!(run(planes), plain);
    }

    #[test]
    fn sculpt_speed_keeps_the_default_brush_fast_and_ignores_the_radius() {
        let default = td_doc::terrain::SculptBrush::default();
        assert!(timed_sculpt_brush(default, 1.0).strength >= 32.0, "the default brush raises at least 32 units a second");
        let big = td_doc::terrain::SculptBrush { radius: 2800.0, ..default };
        assert_eq!(timed_sculpt_brush(big, 0.5).strength, timed_sculpt_brush(default, 0.5).strength, "a big brush is not faster");
    }

    type Segment = ([f32; 3], [f32; 3]);

    fn axis_color(axis: usize) -> [f32; 4] {
        let [r, g, b, _] = crate::theme::linear(crate::theme::VIEW.axis()[axis]);
        [r as f32, g as f32, b as f32, 0.9]
    }

    fn sorted(mut segments: Vec<Segment>) -> Vec<Segment> {
        for s in &mut segments {
            if s.0 > s.1 {
                *s = (s.1, s.0);
            }
        }

        segments.sort_by(|a, b| a.partial_cmp(b).unwrap());
        segments
    }

    fn segments_of(lines: &[LineVertex], color: [f32; 4]) -> Vec<Segment> {
        sorted(lines.chunks(2).filter(|pair| pair[0].color == color).map(|pair| (pair[0].pos, pair[1].pos)).collect())
    }

    /// The outline of a plane at `at` on `axis` (0 is x, 2 is z), spanning `along` on the other horizontal axis and `y`.
    fn plane_outline(axis: usize, at: f32, along: (f32, f32), y: (f32, f32)) -> Vec<Segment> {
        let p = |a: f32, h: f32| if axis == 0 { [at, h, a] } else { [a, h, at] };
        sorted(vec![
            (p(along.0, y.0), p(along.1, y.0)),
            (p(along.1, y.0), p(along.1, y.1)),
            (p(along.1, y.1), p(along.0, y.1)),
            (p(along.0, y.1), p(along.0, y.0)),
        ])
    }

    /// `state_with_terrains` with the first terrain tilted 3 units a cell along x and 2 along z, so its heights run
    /// from 12 to 76 and its middle sits at x 159.5, z 168.25.
    fn tilted_terrains() -> (EditorState, NodeId, NodeId) {
        let (mut state, a, b) = state_with_terrains();
        state.doc.edit("tilt", |m, _| {
            let t = m.terrain_mut(a).unwrap();
            for j in 0..9 {
                for i in 0..17 {
                    let k = t.index(i, j);
                    t.heights[k] = (3 * i + 2 * j) as f32;
                }
            }
        });
        (state, a, b)
    }

    fn all_cameras() -> [Camera; 4] {
        [perspective(), view_2d(ViewKind::Top), view_2d(ViewKind::Front), view_2d(ViewKind::Side)]
    }

    fn axis_drawn(tools: &ToolSet, state: &EditorState, axis: usize) -> Vec<Vec<Segment>> {
        all_cameras().iter().map(|cam| segments_of(&tools.lines(cam, view_rect(), state), axis_color(axis))).collect()
    }

    #[test]
    fn the_x_plane_is_drawn_in_every_view_with_the_ground_line_under_it() {
        let (mut state, _, _) = tilted_terrains();
        state.tool = ToolKind::Sculpt;
        state.symmetry = sym(SymmetryAxes::X, SymmetryCenter::Terrain);
        let tools = ToolSet::default();
        let mut expected = plane_outline(0, 159.5, (40.25, 296.25), (-20.0, 108.0));
        let y = |j: i32| 36.0 + 2.0 * j as f32;
        let z = |j: i32| 40.25 + 32.0 * j as f32;
        for j in 0..8 {
            expected.push(([159.5, y(j), z(j)], [159.5, y(j + 1), z(j + 1)]));
        }

        let expected = sorted(expected);
        for (cam, drawn) in all_cameras().iter().zip(axis_drawn(&tools, &state, 0)) {
            assert_eq!(drawn, expected, "{:?}", cam.kind);
        }

        for drawn in axis_drawn(&tools, &state, 2) {
            assert!(drawn.is_empty(), "no Z plane for X");
        }
    }

    #[test]
    fn the_z_plane_and_both_planes_take_their_own_axis_colors() {
        let (mut state, _, _) = tilted_terrains();
        state.tool = ToolKind::Sculpt;
        let tools = ToolSet::default();
        let mut z_plane = plane_outline(2, 168.25, (-96.5, 415.5), (-20.0, 108.0));
        let y = |i: i32| 20.0 + 3.0 * i as f32;
        let x = |i: i32| -96.5 + 32.0 * i as f32;
        for i in 0..16 {
            z_plane.push(([x(i), y(i), 168.25], [x(i + 1), y(i + 1), 168.25]));
        }

        let z_plane = sorted(z_plane);
        state.symmetry = sym(SymmetryAxes::Z, SymmetryCenter::Terrain);
        for drawn in axis_drawn(&tools, &state, 2) {
            assert_eq!(drawn, z_plane);
        }

        for drawn in axis_drawn(&tools, &state, 0) {
            assert!(drawn.is_empty(), "no X plane for Z");
        }

        state.symmetry = sym(SymmetryAxes::Xz, SymmetryCenter::Terrain);
        for (x, z) in axis_drawn(&tools, &state, 0).into_iter().zip(axis_drawn(&tools, &state, 2)) {
            assert_eq!((x.len(), z), (12, z_plane.clone()), "the X plane has its outline and eight ground segments");
        }
    }

    #[test]
    fn the_ground_line_stops_at_256_segments() {
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        state.doc.edit("wide", |m, s| {
            let id = m.insert(layer, td_doc::NodeKind::Terrain(td_geom::Terrain::new(DVec3::new(-10.5, 3.0, 7.25), [301, 4], 4.0, "dev/grey")));
            s.select_node(id);
        });
        state.tool = ToolKind::Sculpt;
        state.symmetry = sym(SymmetryAxes::Z, SymmetryCenter::Terrain);
        let tools = ToolSet::default();
        for drawn in axis_drawn(&tools, &state, 2) {
            assert_eq!(drawn.len(), 4 + 256, "the outline and the capped ground line");
        }

        state.symmetry = sym(SymmetryAxes::X, SymmetryCenter::Terrain);
        for drawn in axis_drawn(&tools, &state, 0) {
            assert_eq!(drawn.len(), 4 + 3, "a short axis gets one segment per cell");
        }
    }

    #[test]
    fn symmetry_planes_follow_the_center_choice() {
        let (mut state, _, _) = tilted_terrains();
        state.tool = ToolKind::Sculpt;
        let tools = ToolSet::default();
        let has_plane = |state: &EditorState, at: f32| {
            let drawn = segments_of(&tools.lines(&view_2d(ViewKind::Top), view_rect(), state), axis_color(0));
            plane_outline(0, at, (40.25, 296.25), (-20.0, 108.0)).iter().all(|s| drawn.contains(s))
        };

        state.symmetry = sym(SymmetryAxes::X, SymmetryCenter::World);
        assert!(has_plane(&state, 0.0), "the world origin puts the plane at x 0");
        state.symmetry = Symmetry { axes: SymmetryAxes::X, center: SymmetryCenter::Custom, x: -50.5, z: 99.0 };
        assert!(has_plane(&state, -50.5), "a custom plane sits where it was typed");
        let drawn = segments_of(&tools.lines(&view_2d(ViewKind::Top), view_rect(), &state), axis_color(2));
        assert!(drawn.is_empty(), "the typed z is ignored without a Z plane");
    }

    #[test]
    fn each_target_in_reach_gets_its_own_plane() {
        let (mut state, _, _) = tilted_terrains();
        state.tool = ToolKind::Sculpt;
        state.symmetry = sym(SymmetryAxes::X, SymmetryCenter::Terrain);
        let tools = ToolSet::default();
        for drawn in axis_drawn(&tools, &state, 0) {
            assert_eq!(drawn.len(), 12, "only the selected terrain");
        }

        state.doc.selection.clear();
        let want = plane_outline(0, 628.0, (500.0, 756.0), (-32.0, 32.0));
        for drawn in axis_drawn(&tools, &state, 0) {
            assert_eq!(drawn.len(), 12 + 4 + 4, "with nothing selected both terrains are in reach");
            assert!(want.iter().all(|s| drawn.contains(s)), "the second terrain mirrors about its own middle");
        }
    }

    #[test]
    fn the_planes_are_drawn_for_blend_without_a_hover_and_never_for_paint_or_off() {
        let (mut state, _, _) = tilted_terrains();
        state.symmetry = sym(SymmetryAxes::Xz, SymmetryCenter::Terrain);
        let tools = ToolSet::default();
        for tool in [ToolKind::Sculpt, ToolKind::Blend] {
            state.tool = tool;
            assert!(tools.stroke.hover.is_none() && tools.blend.hover.is_none());
            for (x, z) in axis_drawn(&tools, &state, 0).into_iter().zip(axis_drawn(&tools, &state, 2)) {
                assert_eq!((x.len(), z.len()), (12, 20), "{tool:?}");
            }
        }

        for tool in [ToolKind::Paint, ToolKind::Select, ToolKind::Scatter] {
            state.tool = tool;
            for axis in [0, 2] {
                for drawn in axis_drawn(&tools, &state, axis) {
                    assert!(drawn.is_empty(), "{tool:?} draws no plane");
                }
            }
        }

        state.symmetry = Symmetry { center: SymmetryCenter::World, ..Symmetry::OFF };
        for tool in [ToolKind::Sculpt, ToolKind::Blend] {
            state.tool = tool;
            for axis in [0, 2] {
                for drawn in axis_drawn(&tools, &state, axis) {
                    assert!(drawn.is_empty(), "{tool:?} with symmetry off draws no plane");
                }
            }
        }
    }

    #[test]
    fn a_displacement_gets_a_plane_through_the_middle_of_its_face_and_no_ground_line() {
        let mut brush = Brush::from_aabb(&Aabb::new(DVec3::new(-40.5, 0.0, 25.25), DVec3::new(87.5, 16.0, 153.25)), "dev/grey").unwrap();
        let top = brush.faces.iter().position(|f| f.plane.normal.y > 0.9).unwrap();
        brush.faces[top].data.disp = Some(td_geom::displacement::Displacement::new(3));
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        state.doc.edit("brush", |m, _| m.insert(layer, td_doc::NodeKind::Brush(brush)));
        state.symmetry = sym(SymmetryAxes::X, SymmetryCenter::Terrain);
        let tools = ToolSet::default();
        for tool in [ToolKind::Sculpt, ToolKind::Blend] {
            state.tool = tool;
            let want = plane_outline(0, 23.5, (25.25, 153.25), (-16.0, 48.0));
            for drawn in axis_drawn(&tools, &state, 0) {
                assert_eq!(drawn, want, "{tool:?}");
            }
        }
    }

    /// The mean of each ring of 40 segments drawn in `color`, from the start points of its segments.
    fn ring_centers(lines: &[LineVertex], color: [f32; 4]) -> Vec<[f32; 3]> {
        let points: Vec<[f32; 3]> = lines.chunks(2).filter(|pair| pair[0].color == color).map(|pair| pair[0].pos).collect();
        assert_eq!(points.len() % 40, 0, "rings of 40 segments");
        let mut centers: Vec<[f32; 3]> = points
            .chunks(40)
            .map(|ring| {
                let sum = ring.iter().fold([0.0f32; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]);
                [sum[0] / 40.0, sum[1] / 40.0, sum[2] / 40.0]
            })
            .collect();
        centers.sort_by(|a, b| a.partial_cmp(b).unwrap());
        centers
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3)
    }

    /// A hover on the tilted terrain at x 50, z 100 (height 29.46875) and the surface normal there.
    fn hover_on_tilt() -> (DVec3, DVec3) {
        (DVec3::new(50.0, 29.46875, 100.0), DVec3::new(-3.0 / 32.0, 1.0, -2.0 / 32.0).normalize())
    }

    #[test]
    fn a_sculpt_hover_gets_a_dimmer_ring_at_every_mirrored_spot() {
        let (mut state, _, _) = tilted_terrains();
        state.tool = ToolKind::Sculpt;
        state.sculpt.radius = 40.0;
        let mut tools = ToolSet::default();
        let (p, n) = hover_on_tilt();
        tools.stroke.hover = Some((p, n));
        let ghost = [0.4, 1.0, 0.6, 0.45];
        let at = |x: f64, y: f64, z: f64, nx: f64, nz: f64| {
            let c = DVec3::new(x, y, z) + DVec3::new(n.x * nx, n.y, n.z * nz) * 0.5;
            [c.x as f32, c.y as f32, c.z as f32]
        };
        let cases = [
            (SymmetryAxes::Off, vec![]),
            (SymmetryAxes::X, vec![at(269.0, 50.0, 100.0, -1.0, 1.0)]),
            (SymmetryAxes::Z, vec![at(50.0, 38.0, 236.5, 1.0, -1.0)]),
            (SymmetryAxes::Xz, vec![at(50.0, 38.0, 236.5, 1.0, -1.0), at(269.0, 50.0, 100.0, -1.0, 1.0), at(269.0, 58.53125, 236.5, -1.0, -1.0)]),
        ];
        for (axes, mut want) in cases {
            want.sort_by(|a, b| a.partial_cmp(b).unwrap());
            state.symmetry = sym(axes, SymmetryCenter::Terrain);
            for cam in all_cameras() {
                let got = ring_centers(&tools.lines(&cam, view_rect(), &state), ghost);
                assert_eq!(got.len(), want.len(), "{axes:?} in {:?}", cam.kind);
                assert!(got.iter().zip(&want).all(|(g, w)| near(*g, *w)), "{axes:?} in {:?}: {got:?} vs {want:?}", cam.kind);
            }
        }

        state.symmetry = sym(SymmetryAxes::X, SymmetryCenter::Terrain);
        let real = tools.lines(&perspective(), view_rect(), &state).iter().filter(|v| v.color == [0.4, 1.0, 0.6, 0.9]).count();
        assert_eq!(real, 82, "the real ring and its normal tick stay");
    }

    #[test]
    fn a_ghost_ring_needs_a_target_under_the_hover_and_a_spot_that_is_not_the_hover() {
        let (mut state, _, _) = tilted_terrains();
        state.tool = ToolKind::Sculpt;
        state.sculpt.radius = 64.0;
        let mut tools = ToolSet::default();
        let ghosts =
            |tools: &ToolSet, state: &EditorState| ring_centers(&tools.lines(&view_2d(ViewKind::Top), view_rect(), state), [0.4, 1.0, 0.6, 0.45]).len();
        let hover = |x: f64| Some((DVec3::new(x, 40.0, 100.0), DVec3::Y));

        state.symmetry = sym(SymmetryAxes::X, SymmetryCenter::Terrain);
        tools.stroke.hover = hover(159.5);
        assert_eq!(ghosts(&tools, &state), 0, "a dab on the plane lands on itself");
        tools.stroke.hover = hover(159.75);
        assert_eq!(ghosts(&tools, &state), 1, "a quarter unit off the plane is a mirror");
        tools.stroke.hover = hover(1000.0);
        assert_eq!(ghosts(&tools, &state), 0, "a hover past the terrain mirrors nothing");

        state.symmetry = sym(SymmetryAxes::X, SymmetryCenter::World);
        tools.stroke.hover = hover(160.5);
        assert_eq!(ghosts(&tools, &state), 1, "the mirrored brush still reaches the terrain edge by its radius");
        tools.stroke.hover = hover(160.75);
        assert_eq!(ghosts(&tools, &state), 0, "past the radius it reaches nothing");

        state.doc.selection.clear();
        state.symmetry = sym(SymmetryAxes::X, SymmetryCenter::Terrain);
        tools.stroke.hover = hover(50.0);
        assert_eq!(ghosts(&tools, &state), 1, "the second terrain in reach adds no ring of its own");
    }

    #[test]
    fn a_blend_hover_gets_dimmer_rings_in_the_blend_color() {
        let (mut state, _, _) = tilted_terrains();
        state.tool = ToolKind::Blend;
        state.blend.radius = 40.0;
        let mut tools = ToolSet::default();
        let (p, n) = hover_on_tilt();
        tools.blend.hover = Some((p, n));
        state.symmetry = sym(SymmetryAxes::Z, SymmetryCenter::Terrain);
        let ghost = [0.95, 0.6, 1.0, 0.95 * 0.5];
        let centers = ring_centers(&tools.lines(&perspective(), view_rect(), &state), ghost);
        let c = DVec3::new(50.0, 38.0, 236.5) + DVec3::new(n.x, n.y, -n.z) * 0.5;
        assert_eq!(centers.len(), 1);
        assert!(near(centers[0], [c.x as f32, c.y as f32, c.z as f32]), "{centers:?}");

        state.symmetry = Symmetry::OFF;
        assert!(ring_centers(&tools.lines(&perspective(), view_rect(), &state), ghost).is_empty());
    }

    /// The wedge grouped, with a mirrored linked copy across `axis` at the selection's max side, the copy selected.
    fn mirrored_wedge(axis: usize) -> (EditorState, NodeId, NodeId) {
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let group = state.doc.edit("add", |m, s| {
            s.clear();
            s.select_node(m.insert(layer, td_doc::NodeKind::Brush(wedge())));
            ops::group_selection(m, s, "half", layer).unwrap()
        });
        let choice = td_doc::symmetry::PlaneChoice::default();
        let (copies, _) = state.doc.edit("Mirror Linked", |m, s| td_doc::mirror::mirror_linked(m, s, axis, &choice, 0.0)).unwrap();
        (state, group, copies[0])
    }

    /// The outline of the plane at `at` on `axis` over `lo` to `hi` on the other axes, grown by one meter.
    fn rect_outline(axis: usize, at: f32, lo: [f32; 3], hi: [f32; 3]) -> Vec<Segment> {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        let p = |a: f32, b: f32| {
            let mut out = [0.0; 3];
            out[axis] = at;
            out[u] = a;
            out[v] = b;
            out
        };
        let (u0, u1, v0, v1) = (lo[u] - 32.0, hi[u] + 32.0, lo[v] - 32.0, hi[v] + 32.0);
        sorted(vec![(p(u0, v0), p(u1, v0)), (p(u1, v0), p(u1, v1)), (p(u1, v1), p(u0, v1)), (p(u0, v1), p(u0, v0))])
    }

    /// The plane at `at` on `axis` as a 2D view looking along `depth` at `zoom` draws it, three lines a point apart over
    /// `lo` to `hi` grown by one meter.
    fn bar_outline(axis: usize, at: f32, lo: [f32; 3], hi: [f32; 3], depth: usize, zoom: f64) -> Vec<Segment> {
        let along = 3 - axis - depth;
        let point = 1.0 / zoom;
        sorted(
            [-point, 0.0, point]
                .map(|o| {
                    let mut a = [0.0; 3];
                    a[depth] = (lo[depth] + hi[depth]) * 0.5;
                    a[axis] = (at as f64 + o) as f32;
                    a[along] = lo[along] - 32.0;
                    let mut b = a;
                    b[along] = hi[along] + 32.0;
                    (a, b)
                })
                .to_vec(),
        )
    }

    fn select_only(state: &mut EditorState, ids: &[NodeId]) {
        state.doc.select(|_, s| {
            s.clear();
            s.nodes.extend(ids);
        });
    }

    fn plane_segments(tools: &ToolSet, state: &EditorState, cam: &Camera) -> Vec<Segment> {
        (0..3).flat_map(|axis| segments_of(&tools.lines(cam, view_rect(), state), axis_color(axis))).collect()
    }

    #[test]
    fn a_selected_mirrored_half_draws_its_plane_over_both_halves_on_each_axis() {
        let tools = ToolSet::default();
        // The wedge spans x 100..280, y 64..160, z -150..-20, and its copy sits on the max side of the chosen axis.
        let (lo, hi) = ([100.0, 64.0, -150.0], [280.0, 160.0, -20.0]);
        for axis in 0..3 {
            let (mut state, group, copy) = mirrored_wedge(axis);
            let mut both = hi;
            both[axis] = hi[axis] + (hi[axis] - lo[axis]);
            let expected = rect_outline(axis, hi[axis], lo, both);
            for id in [copy, group] {
                select_only(&mut state, &[id]);
                for cam in all_cameras() {
                    let depth = cam.kind.depth_axis();
                    let want = if cam.kind.is_2d() && depth != axis { bar_outline(axis, hi[axis], lo, both, depth, cam.zoom) } else { expected.clone() };
                    assert_eq!(plane_segments(&tools, &state, &cam), want, "axis {axis} {:?}", cam.kind);
                }
            }

            let leaf = state.doc.map.get(copy).unwrap().children[0];
            select_only(&mut state, &[leaf]);
            assert_eq!(plane_segments(&tools, &state, &perspective()), expected, "axis {axis}: a brush inside a half shows it too");
        }
    }

    fn edit_plane_segments(tools: &ToolSet, state: &EditorState, axis: usize) -> Vec<Segment> {
        segments_of(&tools.lines(&perspective(), view_rect(), state), axis_color(axis))
    }

    #[test]
    fn the_vertex_tool_draws_its_symmetry_plane_over_the_selected_brushes() {
        use td_doc::symmetry::{EditSymmetry, PlaneAt, PlaneChoice};
        let tools = ToolSet::default();
        let mut state = EditorState::new(Default::default());
        insert_brushes(&mut state, vec![trapezoid()]);
        state.tool = ToolKind::Vertex;
        let (lo, hi) = ([-20.0, 0.0, 10.0], [95.0, 40.0, 60.0]);
        assert!(edit_plane_segments(&tools, &state, 0).is_empty(), "off draws nothing");

        let cases = [
            (0, PlaneAt::Center, 0.0, 37.5),
            (0, PlaneAt::World, 55.0, 0.0),
            (0, PlaneAt::Custom, 140.25, 140.25),
            (1, PlaneAt::Center, 0.0, 20.0),
            (2, PlaneAt::Custom, -12.5, -12.5),
        ];
        for (axis, at, custom, plane_at) in cases {
            state.edit_symmetry = EditSymmetry { axis: Some(axis), plane: PlaneChoice { at, custom } };
            assert_eq!(edit_plane_segments(&tools, &state, axis), rect_outline(axis, plane_at, lo, hi), "axis {axis} {at:?}");
        }

        state.tool = ToolKind::Select;
        assert!(edit_plane_segments(&tools, &state, 2).is_empty(), "only the Vertex and Mesh tools draw it");
        state.tool = ToolKind::Vertex;
        select_only(&mut state, &[]);
        assert!(edit_plane_segments(&tools, &state, 2).is_empty(), "nothing selected, nothing to draw over");
    }

    #[test]
    fn the_vertex_plane_spans_every_selected_brush_and_the_mesh_plane_the_meshes_in_edit() {
        use td_doc::symmetry::{EditSymmetry, PlaneAt, PlaneChoice};
        let tools = ToolSet::default();
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let far = hull(&[DVec3::new(200.0, 10.0, -50.0), DVec3::new(260.0, 10.0, -50.0), DVec3::new(230.0, 70.0, -20.0), DVec3::new(230.0, 10.0, -20.0)]);
        let mesh = td_geom::mesh_shapes::cuboid(&Aabb::new(DVec3::new(-300.5, 4.0, 90.0), DVec3::new(-220.5, 36.0, 118.0)), "dev/grey");
        let (brushes, mesh_id) = state.doc.edit("add", |m, s| {
            s.clear();
            let a = m.insert(layer, td_doc::NodeKind::Brush(trapezoid()));
            let b = m.insert(layer, td_doc::NodeKind::Brush(far));
            let c = m.insert(layer, td_doc::NodeKind::Mesh(mesh));
            (vec![a, b], c)
        });
        state.edit_symmetry = EditSymmetry { axis: Some(0), plane: PlaneChoice { at: PlaneAt::Center, custom: 0.0 } };

        select_only(&mut state, &brushes);
        state.tool = ToolKind::Vertex;
        assert_eq!(edit_plane_segments(&tools, &state, 0), rect_outline(0, 120.0, [-20.0, 0.0, -50.0], [260.0, 70.0, 60.0]));
        state.tool = ToolKind::Mesh;
        assert!(edit_plane_segments(&tools, &state, 0).is_empty(), "no mesh in edit, brushes do not count");

        select_only(&mut state, &[mesh_id]);
        assert_eq!(edit_plane_segments(&tools, &state, 0), rect_outline(0, -260.5, [-300.5, 4.0, 90.0], [-220.5, 36.0, 118.0]));
        state.tool = ToolKind::Vertex;
        assert!(edit_plane_segments(&tools, &state, 0).is_empty(), "no brush selected, meshes do not count");
    }

    #[test]
    fn mirror_planes_show_in_every_tool() {
        let tools = ToolSet::default();
        let (mut state, _, _) = mirrored_wedge(0);
        for tool in [ToolKind::Select, ToolKind::Vertex, ToolKind::Mesh, ToolKind::Clip, ToolKind::Sculpt, ToolKind::Texture] {
            state.tool = tool;
            let drawn = segments_of(&tools.lines(&perspective(), view_rect(), &state), axis_color(0));
            assert_eq!(drawn.len(), 4, "{tool:?}");
        }
    }

    #[test]
    fn nothing_selected_or_a_plain_linked_copy_draws_no_plane() {
        let tools = ToolSet::default();
        let (mut state, group, copy) = mirrored_wedge(0);
        select_only(&mut state, &[]);
        assert!(plane_segments(&tools, &state, &perspective()).is_empty());

        let linked = state.doc.edit("link", |m, s| {
            s.clear();
            s.select_node(group);
            ops::duplicate_linked(m, s, DVec3::new(0.0, 0.0, 400.0), Default::default())
        });
        select_only(&mut state, &linked);
        assert!(plane_segments(&tools, &state, &perspective()).is_empty(), "a copy slid along the plane is no mirror of its source");
        select_only(&mut state, &[copy]);
        assert_eq!(plane_segments(&tools, &state, &perspective()).len(), 4, "the real mirror still draws, so the slid copy is the only reason for nothing");

        let (mut plain, only, _) = mirrored_wedge(0);
        plain.doc.edit("unlink", |m, s| {
            s.clear();
            s.select_node(only);
            ops::unlink_groups(m, s);
        });
        select_only(&mut plain, &[only]);
        assert!(plane_segments(&tools, &plain, &perspective()).is_empty(), "an unlinked group has no partner");
    }

    #[test]
    fn a_facade_of_mirrored_pairs_draws_one_plane_per_selected_pair() {
        let tools = ToolSet::default();
        let (mut state, a, b) = mirrored_wedge(0);
        let shifted = state.doc.edit("link", |m, s| {
            s.clear();
            s.nodes.extend([a, b]);
            ops::duplicate_linked(m, s, DVec3::new(0.0, 0.0, 300.0), Default::default())
        });
        select_only(&mut state, &[shifted[0]]);
        let drawn = plane_segments(&tools, &state, &perspective());
        assert_eq!(drawn.len(), 4, "the nearest partner only, not one plane per far copy");
        assert_eq!(drawn[0].0[0], 280.0);
        select_only(&mut state, &[a, shifted[0]]);
        assert_eq!(plane_segments(&tools, &state, &perspective()).len(), 8, "two pairs, two planes");
    }

    fn linked_segments(tools: &ToolSet, state: &EditorState, cam: &Camera) -> Vec<Segment> {
        segments_of(&tools.lines(cam, view_rect(), state), crate::theme::LINKED_EDGE)
    }

    /// Every brush edge under `groups`, as the lines draw them.
    fn edges_under(state: &EditorState, groups: &[NodeId]) -> Vec<Segment> {
        let map = &state.doc.map;
        let mut out = Vec::new();
        for g in groups {
            for n in map.descendants(*g) {
                if let Some(b) = map.brush(n) {
                    out.extend(b.edges().into_iter().map(|(a, c)| (v3(b.vertices[a as usize]), v3(b.vertices[c as usize]))));
                }
            }
        }

        sorted(out)
    }

    fn brush_in(state: &EditorState, group: NodeId) -> NodeId {
        state.doc.map.descendants(group).into_iter().find(|n| state.doc.map.brush(*n).is_some()).unwrap()
    }

    #[test]
    fn a_selected_linked_half_outlines_only_its_partners() {
        let tools = ToolSet::default();
        for cam in [perspective(), view_2d(ViewKind::Top), view_2d(ViewKind::Front)] {
            let (mut state, group, copy) = mirrored_wedge(0);
            let (own, partner) = (edges_under(&state, &[group]), edges_under(&state, &[copy]));
            assert_eq!(own.len(), 12);
            assert_ne!(own, partner, "the halves are mirrored, so their edges differ");

            select_only(&mut state, &[copy]);
            assert_eq!(linked_segments(&tools, &state, &cam), own, "the pair outlines the half that is not selected");
            select_only(&mut state, &[group]);
            assert_eq!(linked_segments(&tools, &state, &cam), partner);

            select_only(&mut state, &[group, copy]);
            assert!(linked_segments(&tools, &state, &cam).is_empty(), "both halves are selected, so none is a partner");
            select_only(&mut state, &[]);
            assert!(linked_segments(&tools, &state, &cam).is_empty());

            let inner = brush_in(&state, group);
            select_only(&mut state, &[inner]);
            assert_eq!(linked_segments(&tools, &state, &cam), partner, "a brush inside a half stands for the half");
            state.doc.select(|_, s| {
                s.clear();
                s.select_face(inner, 2);
            });
            assert_eq!(linked_segments(&tools, &state, &cam), partner, "so does a selected face");

            let third = state.doc.edit("link", |m, s| {
                s.clear();
                s.select_node(group);
                ops::duplicate_linked(m, s, DVec3::new(0.0, 0.0, 400.0), Default::default())
            })[0];
            select_only(&mut state, &[third]);
            assert_eq!(linked_segments(&tools, &state, &cam), edges_under(&state, &[group, copy]), "a set of three outlines the other two");
            select_only(&mut state, &[group, third]);
            assert_eq!(linked_segments(&tools, &state, &cam), partner, "one member left unselected");

            state.doc.edit("unlink", |m, s| {
                s.clear();
                s.select_node(third);
                ops::unlink_groups(m, s);
            });
            select_only(&mut state, &[third]);
            assert!(linked_segments(&tools, &state, &cam).is_empty(), "an unlinked group has no partner");
            select_only(&mut state, &[group]);
            assert_eq!(linked_segments(&tools, &state, &cam), partner, "the other two stay linked");

            state.doc.edit("hide", |m, _| ops::set_hidden(m, &[copy], true));
            assert!(linked_segments(&tools, &state, &cam).is_empty(), "a hidden partner is not outlined");
        }
    }
}
