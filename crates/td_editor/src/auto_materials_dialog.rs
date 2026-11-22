//! Auto Materials: one material each for the floors, ceilings, inside walls and outside walls of the selected brushes.

use td_core::NodeId;
use td_doc::Map;
use td_geom::face_class::{FaceClass, classify};

use crate::i18n::{en, tr, tr_n};
use crate::state::EditorState;

/// The material per face class, an empty one leaves those faces as they are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoMaterials {
    pub floor: String,
    pub ceiling: String,
    pub inside_wall: String,
    pub outside_wall: String,
}

impl AutoMaterials {
    fn of(&self, class: FaceClass) -> &str {
        match class {
            FaceClass::Floor => &self.floor,
            FaceClass::Ceiling => &self.ceiling,
            FaceClass::InsideWall => &self.inside_wall,
            FaceClass::OutsideWall => &self.outside_wall,
        }
    }
}

/// Faces painted per class, in the order floor, ceiling, inside wall, outside wall.
pub type Counts = [usize; 4];

fn slot(class: FaceClass) -> usize {
    match class {
        FaceClass::Floor => 0,
        FaceClass::Ceiling => 1,
        FaceClass::InsideWall => 2,
        FaceClass::OutsideWall => 3,
    }
}

fn plan(map: &Map, ids: &[NodeId], materials: &AutoMaterials) -> (Vec<(NodeId, usize, String)>, Counts) {
    let world: Vec<_> = map.brushes().map(|(_, b)| b).collect();
    let mut out = Vec::new();
    let mut counts = [0; 4];
    for &id in ids {
        let Some(brush) = map.brush(id) else { continue };
        for face in 0..brush.faces.len() {
            let class = classify(brush, face, &world);
            let material = materials.of(class);
            if !material.is_empty() {
                counts[slot(class)] += 1;
                out.push((id, face, material.to_string()));
            }
        }
    }

    (out, counts)
}

/// Paints the faces of brushes `ids` by class in one undo step.
pub fn apply(state: &mut EditorState, ids: &[NodeId], materials: &AutoMaterials) -> Counts {
    let (faces, counts) = plan(&state.doc.map, ids, materials);
    if !faces.is_empty() {
        state.doc.edit(en!("Auto Materials"), |m, _| {
            for (id, face, material) in faces {
                if let Some(f) = m.brush_mut(id).and_then(|b| b.faces.get_mut(face)) {
                    f.data.material = material;
                }
            }
        });
    }

    counts
}

#[derive(Default)]
pub struct AutoMaterialsDialog {
    pub open: bool,
    materials: AutoMaterials,
}

impl AutoMaterialsDialog {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        if !self.open {
            return;
        }

        let materials = &mut self.materials;
        let mut open = self.open;
        let mut run = false;
        crate::widgets::centered_window(en!("Auto Materials"), ctx).open(&mut open).resizable(false).show(ctx, |ui| {
            egui::Grid::new("auto_materials").num_columns(3).show(ui, |ui| {
                for (label, value) in [
                    (tr("Floor"), &mut materials.floor),
                    (tr("Ceiling"), &mut materials.ceiling),
                    (tr("Inside walls"), &mut materials.inside_wall),
                    (tr("Outside walls"), &mut materials.outside_wall),
                ] {
                    ui.label(label);
                    ui.add_sized([180.0, ui.spacing().interact_size.y], egui::TextEdit::singleline(value).hint_text(tr("Unchanged")));
                    if ui.button(tr("Use current")).on_hover_text(tr("Takes the material picked in the Materials panel")).clicked() {
                        value.clone_from(&state.current_material);
                    }

                    ui.end_row();
                }
            });

            let count = state.doc.selection.brushes(&state.doc.map).len();
            if count == 0 {
                ui.colored_label(ui.visuals().warn_fg_color, tr("Select the brushes to paint"));
                return;
            }

            if ui
                .button(tr_n!(count, "Apply to {n} brush", "Apply to {n} brushes"))
                .on_hover_text(tr("A wall faces inside when walls stand ahead of it and to both sides"))
                .clicked()
            {
                run = true;
            }
        });
        self.open = open;
        if run {
            let ids = state.doc.selection.brushes(&state.doc.map);
            apply(state, &ids, &self.materials);
        }
    }
}

#[cfg(test)]
mod tests {
    use td_core::{Aabb, DVec3};
    use td_doc::NodeKind;
    use td_geom::Brush;

    use super::*;
    use crate::state::Prefs;

    #[test]
    fn a_room_gets_its_floor_ceiling_and_both_wall_sides_painted_in_one_undo_step() {
        let mut state = EditorState::new(Prefs::default());
        let layer = state.doc.map.default_layer();
        let mut ids = Vec::new();
        for (min, max) in [
            ([0.0, -16.0, 0.0], [256.0, 0.0, 256.0]),
            ([0.0, 128.0, 0.0], [256.0, 144.0, 256.0]),
            ([0.0, 0.0, 0.0], [16.0, 128.0, 256.0]),
            ([240.0, 0.0, 0.0], [256.0, 128.0, 256.0]),
            ([16.0, 0.0, 0.0], [240.0, 128.0, 16.0]),
            ([16.0, 0.0, 240.0], [240.0, 128.0, 256.0]),
        ] {
            let b = Brush::from_aabb(&Aabb::new(DVec3::from(min), DVec3::from(max)), "old").unwrap();
            ids.push(state.doc.map.insert(layer, NodeKind::Brush(b)));
        }

        let materials = AutoMaterials { floor: "tile".into(), inside_wall: "plaster".into(), outside_wall: "brick".into(), ..Default::default() };
        let counts = apply(&mut state, &ids, &materials);
        assert_eq!(counts[1], 0, "an empty ceiling material leaves ceilings alone");
        assert!(counts[0] > 0 && counts[2] >= 4 && counts[3] >= 4, "{counts:?}");

        let west = state.doc.map.brush(ids[2]).unwrap();
        let face = |n: DVec3| &west.faces.iter().find(|f| f.plane.normal.dot(n) > 0.99).unwrap().data.material;
        assert_eq!((face(DVec3::X).as_str(), face(-DVec3::X).as_str()), ("plaster", "brick"));
        assert_eq!(face(-DVec3::Y), "old");

        state.doc.undo();
        assert!(state.doc.map.brush(ids[2]).unwrap().faces.iter().all(|f| f.data.material == "old"));
    }
}
