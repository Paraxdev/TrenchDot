//! The symmetry controls of the Sculpt and Blend options bars.

use egui::Ui;
use td_core::{Aabb, DVec3, Mirror, NodeId, Plane};
use td_doc::symmetry::{EditSymmetry, PlaneAt, PlaneChoice, Symmetry, SymmetryAxes, SymmetryCenter, faces_bounds};
use td_geom::Terrain;
use td_render::LineVertex;

use crate::blend_tool::{self, StrokeScope};
use crate::i18n::{en, tr};
use crate::scene::v3;
use crate::state::EditorState;
use crate::tools::ToolKind;
use crate::{mesh_tool, theme, toolset};

/// How far a plane rectangle reaches above and below what it mirrors, one meter.
const PLANE_MARGIN: f64 = 32.0;
const GROUND_SEGMENTS: u32 = 256;
const RING_SEGMENTS: usize = 40;

fn axes_name(axes: SymmetryAxes) -> &'static str {
    match axes {
        SymmetryAxes::Off => en!("off"),
        SymmetryAxes::X => en!("X"),
        SymmetryAxes::Z => en!("Z"),
        SymmetryAxes::Xz => en!("X and Z"),
    }
}

fn axes_summary(axes: SymmetryAxes) -> &'static str {
    match axes {
        SymmetryAxes::Off => en!("symmetry off"),
        SymmetryAxes::X => en!("symmetry X"),
        SymmetryAxes::Z => en!("symmetry Z"),
        SymmetryAxes::Xz => en!("symmetry X and Z"),
    }
}

fn center_name(center: SymmetryCenter) -> &'static str {
    match center {
        SymmetryCenter::Terrain => en!("terrain center"),
        SymmetryCenter::World => en!("world origin"),
        SymmetryCenter::Custom => en!("custom"),
    }
}

/// The terrains and the faces one Sculpt or Blend dab edits.
fn edited(state: &EditorState, blend: bool) -> (Vec<NodeId>, Vec<(NodeId, usize)>) {
    if blend {
        let t = blend_tool::targets(state);
        (t.terrains, [t.displacements, t.faces].concat())
    } else {
        let scope = StrokeScope::of(state);
        let faces = scope.displacements(state);
        (scope.terrains, faces)
    }
}

/// Where the planes were a moment ago, so a custom center starts there: the first terrain the tool edits, else the
/// middle of its faces.
fn planes_in_use(state: &EditorState, center: SymmetryCenter, blend: bool) -> (f64, f64) {
    if center == SymmetryCenter::World {
        return (0.0, 0.0);
    }

    let map = &state.doc.map;
    let (terrains, faces) = edited(state, blend);
    let both = Symmetry { axes: SymmetryAxes::Xz, ..Symmetry::OFF };
    let mirror = match terrains.iter().find_map(|id| map.terrain(*id)) {
        Some(terrain) => both.for_terrain(terrain),
        None => both.for_faces(map, &faces),
    };
    (mirror.x.unwrap_or(0.0), mirror.z.unwrap_or(0.0))
}

/// The symmetry menus, shared by the Sculpt and Blend tools: which planes, where they sit, and their position when typed.
pub fn options(ui: &mut Ui, state: &mut EditorState, blend: bool) {
    let mut s = state.symmetry;
    egui::ComboBox::from_id_salt("symmetry_axes")
        .selected_text(tr(axes_summary(s.axes)))
        .show_ui(ui, |ui| {
            for axes in [SymmetryAxes::Off, SymmetryAxes::X, SymmetryAxes::Z, SymmetryAxes::Xz] {
                ui.selectable_value(&mut s.axes, axes, tr(axes_name(axes)));
            }
        })
        .response
        .on_hover_text(tr("Mirrors every dab across the chosen planes, both sides change in one undo step"));

    if !s.is_off() {
        let before = s.center;
        egui::ComboBox::from_id_salt("symmetry_center")
            .selected_text(tr(center_name(s.center)))
            .show_ui(ui, |ui| {
                for center in [SymmetryCenter::Terrain, SymmetryCenter::World, SymmetryCenter::Custom] {
                    ui.selectable_value(&mut s.center, center, tr(center_name(center)));
                }
            })
            .response
            .on_hover_text(tr("Where the mirror planes sit: each terrain's middle, the world origin or typed positions"));
        if before != SymmetryCenter::Custom && s.center == SymmetryCenter::Custom {
            (s.x, s.z) = planes_in_use(state, before, blend);
        }

        if s.center == SymmetryCenter::Custom {
            let hover = tr("Where this mirror plane sits, in map units. It mirrors evenly on a line of vertices or halfway between two");
            if s.axes.x() {
                ui.add(egui::DragValue::new(&mut s.x).speed(1.0).prefix("x ").suffix(" u")).on_hover_text(hover);
            }

            if s.axes.z() {
                ui.add(egui::DragValue::new(&mut s.z).speed(1.0).prefix("z ").suffix(" u")).on_hover_text(hover);
            }
        }
    }

    state.symmetry = s;
}

fn plane_name(at: PlaneAt) -> &'static str {
    match at {
        PlaneAt::Center => en!("selection center"),
        PlaneAt::Max => en!("max side"),
        PlaneAt::Min => en!("min side"),
        PlaneAt::World => en!("world origin"),
        PlaneAt::Custom => en!("custom"),
    }
}

const PLANE_CHOICES: [PlaneAt; 5] = [PlaneAt::Center, PlaneAt::Max, PlaneAt::Min, PlaneAt::World, PlaneAt::Custom];

fn custom_plane(ui: &mut Ui, choice: &mut PlaneChoice) {
    if choice.at == PlaneAt::Custom {
        ui.add(egui::DragValue::new(&mut choice.custom).speed(1.0).suffix(" u")).on_hover_text(tr("Where the mirror plane sits along the axis, in map units"));
    }
}

/// Radio rows for where Mirror Linked puts its plane, with the typed position under them while custom is picked.
pub fn plane_rows(ui: &mut Ui, choice: &mut PlaneChoice) {
    for at in PLANE_CHOICES {
        ui.radio_value(&mut choice.at, at, tr(plane_name(at)));
    }

    custom_plane(ui, choice);
}

/// The same choice as a drop-down with the typed position beside it, for the Inspector.
pub fn plane_combo(ui: &mut Ui, choice: &mut PlaneChoice) {
    egui::ComboBox::from_id_salt("mirror_plane")
        .selected_text(tr(plane_name(choice.at)))
        .show_ui(ui, |ui| {
            for at in PLANE_CHOICES {
                ui.selectable_value(&mut choice.at, at, tr(plane_name(at)));
            }
        })
        .response
        .on_hover_text(tr("Where the plane sits: selection middle, larger or smaller side, world origin or a typed position"));
    custom_plane(ui, choice);
}

fn edit_axis_name(axis: Option<usize>) -> &'static str {
    match axis {
        Some(0) => en!("X"),
        Some(1) => en!("Y"),
        Some(2) => en!("Z"),
        _ => en!("off"),
    }
}

fn edit_axis_summary(axis: Option<usize>) -> &'static str {
    match axis {
        Some(0) => en!("symmetry X"),
        Some(1) => en!("symmetry Y"),
        Some(2) => en!("symmetry Z"),
        _ => en!("symmetry off"),
    }
}

/// The box around what the Vertex tool (selected brushes) or the Mesh tool (meshes in edit) works on.
fn edit_bounds(state: &EditorState) -> Aabb {
    let map = &state.doc.map;
    if state.tool == ToolKind::Mesh {
        mesh_tool::edit_bounds(state)
    } else {
        state.doc.selection.brushes(map).into_iter().filter_map(|id| map.brush(id)).fold(Aabb::EMPTY, |acc, b| acc.union(&b.bounds()))
    }
}

/// The symmetry menus of the Vertex and Mesh tools: which axis moves follow across, where the plane sits and its
/// position when typed. The plane choices leave out the bounds sides, an object lies wholly on one side of its own.
pub fn edit_options(ui: &mut Ui, state: &mut EditorState) {
    let mut s = state.edit_symmetry;
    egui::ComboBox::from_id_salt("edit_symmetry_axis")
        .selected_text(tr(edit_axis_summary(s.axis)))
        .show_ui(ui, |ui| {
            for axis in [None, Some(0), Some(1), Some(2)] {
                ui.selectable_value(&mut s.axis, axis, tr(edit_axis_name(axis)));
            }
        })
        .response
        .on_hover_text(tr("Moving a vertex also moves its counterpart across the plane, both sides change in one undo step"));

    if s.axis.is_some() {
        let before = s.plane.at;
        ui.label(tr("Symmetry Plane"));
        egui::ComboBox::from_id_salt("edit_symmetry_plane")
            .selected_text(tr(plane_name(s.plane.at)))
            .show_ui(ui, |ui| {
                for at in [PlaneAt::Center, PlaneAt::World, PlaneAt::Custom] {
                    ui.selectable_value(&mut s.plane.at, at, tr(plane_name(at)));
                }
            })
            .response
            .on_hover_text(tr("Where the symmetry plane sits: the middle of what you edit, the world origin or a typed position"));
        if before != PlaneAt::Custom && s.plane.at == PlaneAt::Custom {
            let here = EditSymmetry { plane: PlaneChoice { at: before, ..s.plane }, ..s };
            if let Some(plane) = here.plane(&edit_bounds(state)) {
                s.plane.custom = plane.at;
            }
        }

        custom_plane(ui, &mut s.plane);
    }

    state.edit_symmetry = s;
}

/// What one dab edits: the mirror its planes resolve to and the footprint it covers.
struct Target<'a> {
    mirror: Mirror,
    bounds: Aabb,
    terrain: Option<&'a Terrain>,
}

fn targets(state: &EditorState, blend: bool) -> Vec<Target<'_>> {
    let map = &state.doc.map;
    let (terrains, faces) = edited(state, blend);
    let mut out: Vec<Target> = terrains
        .iter()
        .filter_map(|id| map.terrain(*id))
        .map(|t| Target { mirror: state.symmetry.for_terrain(t), bounds: t.bounds(), terrain: Some(t) })
        .collect();
    let bounds = faces_bounds(map, &faces);
    if !bounds.is_empty() {
        out.push(Target { mirror: state.symmetry.for_bounds(&bounds), bounds, terrain: None });
    }

    out
}

fn plane_color(axis: usize) -> [f32; 4] {
    let [r, g, b, _] = theme::linear(theme::VIEW.axis()[axis]);
    [r as f32, g as f32, b as f32, 0.9]
}

fn segment(out: &mut Vec<LineVertex>, a: DVec3, b: DVec3, color: [f32; 4]) {
    out.push(LineVertex { pos: v3(a), color });
    out.push(LineVertex { pos: v3(b), color });
}

/// The plane at `at` on `axis` (0 is x, 2 is z) as a rectangle over the target, and for a terrain the line where it
/// meets the ground.
fn plane_lines(target: &Target, axis: usize, at: f64, out: &mut Vec<LineVertex>) {
    let color = plane_color(axis);
    let along = 2 - axis;
    let (lo, hi) = (target.bounds.min[along], target.bounds.max[along]);
    let point = |a: f64, y: f64| {
        let mut p = DVec3::new(0.0, y, 0.0);
        p[axis] = at;
        p[along] = a;
        p
    };
    let (low, high) = (target.bounds.min.y - PLANE_MARGIN, target.bounds.max.y + PLANE_MARGIN);
    let corners = [point(lo, low), point(hi, low), point(hi, high), point(lo, high)];
    for i in 0..4 {
        segment(out, corners[i], corners[(i + 1) % 4], color);
    }

    let Some(terrain) = target.terrain else { return };
    let n = terrain.cells()[along / 2].clamp(1, GROUND_SEGMENTS);
    let ground = |i: u32| {
        let a = lo + (hi - lo) * i as f64 / n as f64;
        let p = point(a, 0.0);
        terrain.height_at(p.x, p.z).map(|y| point(a, y))
    };
    let mut prev = ground(0);
    for i in 1..=n {
        let next = ground(i);
        if let (Some(a), Some(b)) = (prev, next) {
            segment(out, a, b, color);
        }

        prev = next;
    }
}

/// The plane at `at` on `axis` as a rectangle over the other two axes of `span`, grown by the plane margin.
pub fn plane_rect(axis: usize, at: f64, span: &Aabb, out: &mut Vec<LineVertex>) {
    let color = plane_color(axis);
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let point = |a: f64, b: f64| {
        let mut p = DVec3::ZERO;
        p[axis] = at;
        p[u] = a;
        p[v] = b;
        p
    };
    let (u0, u1) = (span.min[u] - PLANE_MARGIN, span.max[u] + PLANE_MARGIN);
    let (v0, v1) = (span.min[v] - PLANE_MARGIN, span.max[v] + PLANE_MARGIN);
    let corners = [point(u0, v0), point(u1, v0), point(u1, v1), point(u0, v1)];
    for i in 0..4 {
        segment(out, corners[i], corners[(i + 1) % 4], color);
    }
}

/// The Vertex and Mesh tools' symmetry plane over what they edit, nothing while it is off or in another tool.
pub fn edit_lines(state: &EditorState, out: &mut Vec<LineVertex>) {
    if !matches!(state.tool, ToolKind::Vertex | ToolKind::Mesh) {
        return;
    }

    let bounds = edit_bounds(state);
    if let Some(plane) = state.edit_symmetry.plane(&bounds).filter(|_| !bounds.is_empty()) {
        plane_rect(plane.axis, plane.at, &bounds, out);
    }
}

/// The mirror planes between the selected linked groups and their mirrored partners.
pub fn mirror_link_lines(state: &EditorState, out: &mut Vec<LineVertex>) {
    for m in td_doc::mirror::selection_planes(&state.doc.map, &state.doc.selection) {
        plane_rect(m.plane.axis, m.plane.at, &m.span, out);
    }
}

/// Where the brush at `hover` would also dab: its mirror through each target it is over, when that lands within brush
/// reach of any target. Each spot is a center and the mirrored surface normal.
fn mirrored_spots(targets: &[Target], hover: (DVec3, DVec3), radius: f64) -> Vec<(DVec3, DVec3)> {
    let reaches = |t: &Target, p: DVec3| {
        (t.bounds.min.x - radius..=t.bounds.max.x + radius).contains(&p.x) && (t.bounds.min.z - radius..=t.bounds.max.z + radius).contains(&p.z)
    };
    let (p, normal) = hover;
    let mut out: Vec<(DVec3, DVec3)> = Vec::new();
    for target in targets.iter().filter(|t| reaches(t, p)) {
        for fold in target.mirror.folds().filter(|f| !f.is_identity()) {
            let mut spot = fold.point(p);
            if (spot.x - p.x).hypot(spot.z - p.z) < 1e-6 || !targets.iter().any(|t| reaches(t, spot)) {
                continue;
            }

            if let Some(y) = targets.iter().find_map(|t| t.terrain?.height_at(spot.x, spot.z)) {
                spot.y = y;
            }

            if !out.iter().any(|(q, _)| (q.x - spot.x).hypot(q.z - spot.z) < 1e-6) {
                out.push((spot, fold.vector(normal)));
            }
        }
    }

    out
}

/// The mirror planes of the Sculpt or Blend tool over everything it edits, and a dimmer brush ring at each mirrored
/// spot of `hover`. Draws nothing with symmetry off.
pub fn lines(state: &EditorState, blend: bool, hover: Option<(DVec3, DVec3)>, out: &mut Vec<LineVertex>) {
    if state.symmetry.is_off() {
        return;
    }

    let targets = targets(state, blend);
    for target in &targets {
        for (axis, at) in [(0, target.mirror.x), (2, target.mirror.z)] {
            if let Some(at) = at {
                plane_lines(target, axis, at, out);
            }
        }
    }

    let Some(hover) = hover else { return };
    let (radius, mut color) = if blend { (state.blend.radius, blend_tool::RING_COLOR) } else { (state.sculpt.radius, toolset::SCULPT_RING_COLOR) };
    color[3] *= 0.5;
    for (center, normal) in mirrored_spots(&targets, hover, radius) {
        let basis = Plane::from_point_normal(center, normal).basis();
        let ring = |i: usize| {
            let a = std::f64::consts::TAU * i as f64 / RING_SEGMENTS as f64;
            center + (basis.0 * a.cos() + basis.1 * a.sin()) * radius + normal * 0.5
        };
        for i in 0..RING_SEGMENTS {
            segment(out, ring(i), ring(i + 1), color);
        }
    }
}
