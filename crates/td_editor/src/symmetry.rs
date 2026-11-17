//! The symmetry controls of the Sculpt and Blend options bars.

use egui::Ui;
use td_core::NodeId;
use td_doc::symmetry::{Symmetry, SymmetryAxes, SymmetryCenter};

use crate::blend_tool::{self, StrokeScope};
use crate::i18n::{en, tr};
use crate::state::EditorState;

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
