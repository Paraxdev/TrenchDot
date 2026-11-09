//! Sweep Faces: the selected brush faces swept along a straight line, an arc or an S-bend into a run of brushes.

use egui::RichText;
use td_core::NodeId;
use td_doc::{Map, NodeKind, Selection};
use td_geom::sweep::{Bend, MAX_SEGMENTS, Sweep, SweepError, SweepFace, SweepPath};

use crate::i18n::{en, tr, tr_n, trf};
use crate::state::EditorState;

/// What the preview was built from, so it is only rebuilt when one of them changes.
type PreviewKey = (Sweep, Vec<(NodeId, usize)>, u64);

#[derive(Default)]
pub struct SweepDialog {
    pub open: bool,
    pub sweep: Sweep,
    preview: Option<(PreviewKey, Result<usize, SweepError>)>,
}

/// The selected brush faces, each with the brush it belongs to.
pub fn selected_faces(map: &Map, selection: &Selection) -> Vec<(NodeId, SweepFace)> {
    selection
        .faces
        .iter()
        .filter_map(|&(id, face)| {
            let brush = map.brush(id)?;
            (face < brush.faces.len()).then(|| (id, SweepFace { brush: brush.clone(), face }))
        })
        .collect()
}

/// Adds the swept brushes beside their source brushes as one undo step and selects them.
pub fn apply(state: &mut EditorState, picked: &[(NodeId, SweepFace)], sweep: &Sweep) -> Result<Vec<NodeId>, SweepError> {
    let faces: Vec<SweepFace> = picked.iter().map(|(_, f)| f.clone()).collect();
    let brushes = sweep.brushes(&faces)?;
    let per_face = sweep.segments as usize;
    Ok(state.doc.edit(en!("Sweep Faces"), |m, s| {
        s.clear();
        let mut ids = Vec::new();
        for (i, brush) in brushes.into_iter().enumerate() {
            let source = picked[i / per_face].0;
            let parent = m.get(source).and_then(|n| n.parent).unwrap_or(m.default_layer());
            let id = m.insert(parent, NodeKind::Brush(brush));
            s.nodes.insert(id);
            ids.push(id);
        }

        ids
    }))
}

fn error_text(e: SweepError) -> String {
    match e {
        SweepError::Radius(min) => trf!("The radius has to be more than {min} u, or the inner side folds over", min = min.ceil()),
        SweepError::Facing => tr("The faces point opposite ways, pick faces that face the same way").to_string(),
        _ => tr("These settings leave a segment without volume").to_string(),
    }
}

impl SweepDialog {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        if !self.open {
            if self.preview.take().is_some() {
                state.sweep_preview.clear();
            }

            return;
        }

        let s = &mut self.sweep;
        let preview = &mut self.preview;
        let mut open = self.open;
        let mut create = false;
        crate::widgets::window(en!("Sweep Faces")).open(&mut open).resizable(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (path, label) in [(SweepPath::Straight, en!("Straight")), (SweepPath::Arc, en!("Arc")), (SweepPath::SBend, en!("S-bend"))] {
                    ui.selectable_value(&mut s.path, path, tr(label));
                }
            });
            egui::Grid::new("sweep_options").num_columns(2).show(ui, |ui| {
                if s.path == SweepPath::Arc {
                    ui.label(tr("Radius"));
                    ui.add(egui::DragValue::new(&mut s.length).range(1.0..=65536.0).suffix(" u"))
                        .on_hover_text(tr("From the center of the turn to the middle of the faces"));
                    ui.end_row();
                    ui.label(tr("Angle"));
                    ui.add(egui::DragValue::new(&mut s.angle).range(1.0..=720.0).suffix("°"));
                    ui.end_row();
                } else {
                    ui.label(tr("Length"));
                    ui.add(egui::DragValue::new(&mut s.length).range(1.0..=65536.0).suffix(" u"));
                    ui.end_row();
                }

                if s.path == SweepPath::SBend {
                    ui.label(tr("Offset"));
                    ui.add(egui::DragValue::new(&mut s.offset).range(-65536.0..=65536.0).suffix(" u")).on_hover_text(tr("How far the end shifts sideways"));
                    ui.end_row();
                }

                if s.path != SweepPath::Straight {
                    ui.label(tr("Direction"));
                    ui.horizontal(|ui| {
                        for (bend, label) in [(Bend::Up, en!("Up")), (Bend::Down, en!("Down")), (Bend::Left, en!("Left")), (Bend::Right, en!("Right"))] {
                            ui.selectable_value(&mut s.bend, bend, tr(label));
                        }
                    })
                    .response
                    .on_hover_text(tr("Seen looking at the faces, on a floor or ceiling up points along -Z"));
                    ui.end_row();
                }

                ui.label(tr("Segments"));
                ui.add(egui::Slider::new(&mut s.segments, 1..=MAX_SEGMENTS).logarithmic(true));
                ui.end_row();
                ui.label(tr("Rise"));
                ui.add(egui::DragValue::new(&mut s.rise).range(-65536.0..=65536.0).suffix(" u"))
                    .on_hover_text(tr("Height gained over the whole sweep, an arc with rise becomes a spiral"));
                ui.end_row();
                ui.label(tr("Twist"));
                ui.add(egui::DragValue::new(&mut s.twist).range(-720.0..=720.0).suffix("°"))
                    .on_hover_text(tr("Turns the faces around their own normal along the way"));
                ui.end_row();
                ui.label(tr("End scale"));
                ui.add(egui::DragValue::new(&mut s.end_scale).range(0.05..=20.0).speed(0.01))
                    .on_hover_text(tr("Size of the faces at the end, 1 keeps them as they are"));
                ui.end_row();
            });

            if state.doc.selection.faces.is_empty() {
                ui.label(RichText::new(tr("Shift+click a face to sweep, Ctrl+Shift+click adds more")).weak());
                return;
            }

            let key = (*s, state.doc.selection.faces.iter().copied().collect(), state.doc.revision);
            if preview.as_ref().is_none_or(|(k, _)| *k != key) {
                let faces: Vec<SweepFace> = selected_faces(&state.doc.map, &state.doc.selection).into_iter().map(|(_, f)| f).collect();
                let built = s.brushes(&faces);
                let count = built.as_ref().map(Vec::len).map_err(|e| *e);
                state.sweep_preview = built.unwrap_or_default();
                *preview = Some((key, count));
            }

            match preview.as_ref().map(|(_, r)| *r) {
                Some(Ok(n)) => {
                    ui.label(RichText::new(tr_n!(n, "{n} brush", "{n} brushes")).weak());
                    create = ui.button(tr("Create")).clicked();
                }
                Some(Err(e)) => _ = ui.colored_label(ui.visuals().warn_fg_color, error_text(e)),
                None => {}
            }
        });
        if !open || create || state.doc.selection.faces.is_empty() {
            self.preview = None;
            state.sweep_preview.clear();
        }

        self.open = open;
        if create {
            let picked = selected_faces(&state.doc.map, &state.doc.selection);
            if let Err(e) = apply(state, &picked, &self.sweep) {
                state.set_status(error_text(e));
            }
        }
    }
}
