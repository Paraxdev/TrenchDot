//! Add Roof: a roof over the top faces of the selected brushes, each brush one wing of the footprint.

use egui::RichText;
use td_core::{DVec2, NodeId};
use td_doc::{Map, NodeKind, Selection};
use td_geom::Brush;
use td_geom::shapes::{RoofEnd, Wing, roof};

use crate::i18n::{en, tr, tr_n};
use crate::state::EditorState;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoofSpec {
    pub pitch: f64,
    pub ends: RoofEnd,
}

impl Default for RoofSpec {
    fn default() -> Self {
        Self { pitch: 30.0, ends: RoofEnd::Gable }
    }
}

/// The wings the selected brushes' footprints make, and the height their tallest top reaches.
pub fn wings(map: &Map, sel: &Selection) -> Option<(Vec<Wing>, f64)> {
    let mut out = Vec::new();
    let mut base = f64::MIN;
    for id in sel.geometry(map) {
        let Some(NodeKind::Brush(b)) = map.get(id).map(|n| &n.kind) else { continue };
        let bounds = b.bounds();
        out.push(Wing::new(DVec2::new(bounds.min.x, bounds.min.z), DVec2::new(bounds.max.x, bounds.max.z)));
        base = base.max(bounds.max.y);
    }

    (!out.is_empty()).then_some((out, base))
}

pub fn build(map: &Map, sel: &Selection, spec: &RoofSpec, material: &str) -> Vec<Brush> {
    wings(map, sel).map(|(w, base)| roof(&w, base, spec.pitch, spec.ends, material)).unwrap_or_default()
}

/// Adds the roof brushes in one undo step and selects them.
pub fn apply(state: &mut EditorState, brushes: Vec<Brush>) -> Vec<NodeId> {
    let parent = state.insert_parent();
    state.doc.edit(en!("Add Roof"), |m, s| {
        s.clear();
        brushes
            .into_iter()
            .map(|b| {
                let id = m.insert(parent, NodeKind::Brush(b));
                s.nodes.insert(id);
                id
            })
            .collect()
    })
}

type PreviewKey = (RoofSpec, String, u64, Vec<NodeId>);

#[derive(Default)]
pub struct RoofDialog {
    pub open: bool,
    spec: RoofSpec,
    preview: Option<(PreviewKey, Vec<Brush>)>,
}

impl RoofDialog {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        if !self.open {
            if self.preview.take().is_some() {
                state.roof_preview.clear();
            }

            return;
        }

        let spec = &mut self.spec;
        let preview = &mut self.preview;
        let mut open = self.open;
        let mut add = None;
        crate::widgets::centered_window(en!("Add Roof"), ctx).open(&mut open).resizable(false).show(ctx, |ui| {
            egui::Grid::new("roof_options").num_columns(2).show(ui, |ui| {
                ui.label(tr("Pitch"));
                ui.add(egui::Slider::new(&mut spec.pitch, 5.0..=75.0).suffix("°"));
                ui.end_row();
                ui.label(tr("Ends"));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut spec.ends, RoofEnd::Gable, tr("Gable"));
                    ui.selectable_value(&mut spec.ends, RoofEnd::Hip, tr("Hip"));
                })
                .response
                .on_hover_text(tr("How a ridge end that runs into no other wing stops"));
                ui.end_row();
            });

            let key = (*spec, state.current_material.clone(), state.doc.revision, state.doc.selection.nodes.iter().copied().collect());
            if preview.as_ref().map(|(k, _)| k) != Some(&key) {
                let built = build(&state.doc.map, &state.doc.selection, spec, &state.current_material);
                state.roof_preview = built.iter().flat_map(|b| b.edges().into_iter().map(|(a, c)| (b.vertices[a as usize], b.vertices[c as usize]))).collect();
                *preview = Some((key, built));
            }

            let Some((_, built)) = preview.as_ref() else { return };
            if built.is_empty() {
                ui.colored_label(ui.visuals().warn_fg_color, tr("Select the brushes the roof covers, one per wing"));
                return;
            }

            ui.label(RichText::new(tr_n!(built.len(), "{n} brush", "{n} brushes")).weak());
            if ui.button(tr("Add Roof")).clicked() {
                add = Some(built.clone());
            }
        });
        if !open || add.is_some() {
            self.preview = None;
            state.roof_preview.clear();
        }

        self.open = open;
        if let Some(brushes) = add {
            apply(state, brushes);
        }
    }
}

#[cfg(test)]
mod tests {
    use td_core::{Aabb, DVec3};

    use super::*;
    use crate::state::Prefs;

    #[test]
    fn two_selected_walls_get_one_l_roof_in_one_undo_step() {
        let mut state = EditorState::new(Prefs::default());
        let layer = state.doc.map.default_layer();
        for (min, max) in [(DVec3::ZERO, DVec3::new(256.0, 96.0, 96.0)), (DVec3::ZERO, DVec3::new(96.0, 128.0, 256.0))] {
            let id = state.doc.map.insert(layer, NodeKind::Brush(Brush::from_aabb(&Aabb::new(min, max), "wall").unwrap()));
            state.doc.selection.nodes.insert(id);
        }

        let (w, base) = wings(&state.doc.map, &state.doc.selection).unwrap();
        assert_eq!((w.len(), base), (2, 128.0));
        let brushes = build(&state.doc.map, &state.doc.selection, &RoofSpec { pitch: 45.0, ends: RoofEnd::Hip }, "roof");
        assert!(brushes.len() >= 2);
        assert!(brushes.iter().all(|b| b.bounds().min.y >= 128.0 - 1e-6 && b.faces.iter().all(|f| f.data.material == "roof")));

        let before = state.doc.map.brushes().count();
        let ids = apply(&mut state, brushes);
        assert_eq!(state.doc.map.brushes().count(), before + ids.len());
        assert_eq!(state.doc.selection.nodes.len(), ids.len());
        state.doc.undo();
        assert_eq!(state.doc.map.brushes().count(), before);
    }
}
