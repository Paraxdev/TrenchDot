//! Bevel Edges: the picked edges of the selected brushes cut off flat or rounded.

use egui::RichText;
use td_core::{DVec3, NodeId};
use td_doc::{Map, NodeKind, Selection};
use td_geom::Brush;
use td_geom::bevel::{BevelError, bevel_edges};

use crate::i18n::{en, tr, tr_n};
use crate::state::EditorState;

pub const MAX_SEGMENTS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BevelSpec {
    pub width: f64,
    pub segments: usize,
}

impl Default for BevelSpec {
    fn default() -> Self {
        Self { width: 8.0, segments: 1 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BevelProblem {
    NoEdge,
    TooWide,
    Broken,
}

impl BevelProblem {
    pub fn message(self) -> &'static str {
        match self {
            BevelProblem::NoEdge => "select brushes, or pick both corners of each edge to bevel with the Vertex tool",
            BevelProblem::TooWide => "the bevel is wider than a face beside the edge",
            BevelProblem::Broken => "the bevelled brush came out invalid, try a smaller width",
        }
    }

    fn text(self) -> String {
        match self {
            BevelProblem::NoEdge => tr("Select brushes, or pick both corners of each edge with the Vertex tool").to_string(),
            BevelProblem::TooWide => tr("The bevel is wider than a face beside the edge").to_string(),
            BevelProblem::Broken => tr("The bevelled brush came out invalid, try a smaller width").to_string(),
        }
    }
}

/// The edges to bevel on each selected brush. With `corners`, only edges with both ends among them, else every edge.
pub fn picked_edges(map: &Map, sel: &Selection, corners: &[DVec3]) -> Vec<(NodeId, Vec<(DVec3, DVec3)>)> {
    let picked = |p: DVec3| corners.iter().any(|c| c.distance(p) < 1e-6);
    let mut out = Vec::new();
    for id in sel.geometry(map) {
        let Some(NodeKind::Brush(b)) = map.get(id).map(|n| &n.kind) else { continue };
        let edges: Vec<(DVec3, DVec3)> = b
            .edges()
            .into_iter()
            .map(|(a, c)| (b.vertices[a as usize], b.vertices[c as usize]))
            .filter(|(a, c)| corners.is_empty() || (picked(*a) && picked(*c)))
            .collect();
        if !edges.is_empty() {
            out.push((id, edges));
        }
    }

    out
}

pub fn build(map: &Map, picked: &[(NodeId, Vec<(DVec3, DVec3)>)], spec: &BevelSpec) -> Result<Vec<(NodeId, Brush)>, BevelProblem> {
    if picked.is_empty() {
        return Err(BevelProblem::NoEdge);
    }

    picked
        .iter()
        .filter_map(|(id, edges)| map.brush(*id).map(|b| (*id, b, edges)))
        .map(|(id, b, edges)| match bevel_edges(b, edges, spec.width, spec.segments) {
            Ok(out) => Ok((id, out)),
            Err(BevelError::NoEdge) => Err(BevelProblem::NoEdge),
            Err(BevelError::TooWide) => Err(BevelProblem::TooWide),
            Err(BevelError::Brush(_)) => Err(BevelProblem::Broken),
        })
        .collect()
}

/// Bevels the picked edges in one undo step and returns how many brushes changed.
pub fn apply(state: &mut EditorState, picked: &[(NodeId, Vec<(DVec3, DVec3)>)], spec: &BevelSpec) -> Result<usize, BevelProblem> {
    let brushes = build(&state.doc.map, picked, spec)?;
    let n = brushes.len();
    state.doc.edit(en!("Bevel Edges"), |m, _| {
        for (id, b) in brushes {
            if let Some(slot) = m.brush_mut(id) {
                *slot = b;
            }
        }
    });
    Ok(n)
}

type PreviewKey = (BevelSpec, Vec<(NodeId, Vec<(DVec3, DVec3)>)>, u64);

#[derive(Default)]
pub struct BevelDialog {
    pub open: bool,
    spec: BevelSpec,
    preview: Option<(PreviewKey, Result<usize, BevelProblem>)>,
}

impl BevelDialog {
    /// `corners` are the Vertex tool's picked corners while it is the active tool.
    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState, corners: &[DVec3]) {
        if !self.open {
            if self.preview.take().is_some() {
                state.bevel_preview.clear();
            }

            return;
        }

        let spec = &mut self.spec;
        let preview = &mut self.preview;
        let mut open = self.open;
        let mut bevel = None;
        crate::widgets::centered_window(en!("Bevel Edges"), ctx).open(&mut open).resizable(false).show(ctx, |ui| {
            egui::Grid::new("bevel_options").num_columns(2).show(ui, |ui| {
                ui.label(tr("Width"));
                ui.add(egui::DragValue::new(&mut spec.width).range(0.5..=1024.0).speed(0.5).suffix(" u"));
                ui.end_row();
                ui.label(tr("Segments"));
                ui.add(egui::Slider::new(&mut spec.segments, 1..=MAX_SEGMENTS)).on_hover_text(tr("One segment cuts the edge off flat, more round it"));
                ui.end_row();
            });

            let picked = picked_edges(&state.doc.map, &state.doc.selection, corners);
            let key = (*spec, picked, state.doc.revision);
            if preview.as_ref().map(|(k, _)| k) != Some(&key) {
                let built = build(&state.doc.map, &key.1, spec);
                state.bevel_preview =
                    built.iter().flatten().flat_map(|(_, b)| b.edges().into_iter().map(|(a, c)| (b.vertices[a as usize], b.vertices[c as usize]))).collect();
                *preview = Some((key, built.map(|b| b.len())));
            }

            let Some((key, result)) = preview.as_ref() else { return };
            match result {
                Ok(_) => {
                    let edges: usize = key.1.iter().map(|(_, e)| e.len()).sum();
                    ui.label(RichText::new(tr_n!(edges, "{n} edge", "{n} edges")).weak());
                    if ui.button(tr("Bevel")).clicked() {
                        bevel = Some(key.1.clone());
                    }
                }
                Err(e) => _ = ui.colored_label(ui.visuals().warn_fg_color, e.text()),
            }
        });
        if !open || bevel.is_some() {
            self.preview = None;
            state.bevel_preview.clear();
        }

        self.open = open;
        if let Some(picked) = bevel
            && let Err(e) = apply(state, &picked, &self.spec)
        {
            state.set_status(e.text());
        }
    }
}

#[cfg(test)]
mod tests {
    use td_core::Aabb;

    use super::*;
    use crate::state::Prefs;

    #[test]
    fn picked_corners_pick_one_edge_and_the_bevel_is_one_undo_step() {
        let mut state = EditorState::new(Prefs::default());
        let layer = state.doc.map.default_layer();
        let cube = Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::splat(64.0)), "wall").unwrap();
        let id = state.doc.map.insert(layer, NodeKind::Brush(cube));
        state.doc.selection.nodes.insert(id);

        assert_eq!(picked_edges(&state.doc.map, &state.doc.selection, &[])[0].1.len(), 12, "no corners picks every edge");
        let corners = [DVec3::new(0.0, 64.0, 64.0), DVec3::new(64.0, 64.0, 64.0)];
        let picked = picked_edges(&state.doc.map, &state.doc.selection, &corners);
        assert_eq!(picked.len(), 1);
        let [(a, b)] = picked[0].1[..] else { panic!("one edge, got {picked:?}") };
        assert!([a, b].contains(&corners[0]) && [a, b].contains(&corners[1]));

        let spec = BevelSpec { width: 64.0, segments: 1 };
        assert_eq!(apply(&mut state, &picked, &spec), Err(BevelProblem::TooWide));
        let none = picked_edges(&state.doc.map, &state.doc.selection, &[DVec3::ZERO]);
        assert_eq!(apply(&mut state, &none, &spec), Err(BevelProblem::NoEdge));

        assert_eq!(apply(&mut state, &picked, &BevelSpec { width: 16.0, segments: 3 }), Ok(1));
        assert_eq!(state.doc.map.brush(id).unwrap().faces.len(), 9);
        state.doc.undo();
        assert_eq!(state.doc.map.brush(id).unwrap().faces.len(), 6);
    }

    #[test]
    fn every_edge_of_a_box_rounds_off() {
        let mut state = EditorState::new(Prefs::default());
        let layer = state.doc.map.default_layer();
        let cube = Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::splat(64.0)), "wall").unwrap();
        let id = state.doc.map.insert(layer, NodeKind::Brush(cube));
        state.doc.selection.nodes.insert(id);
        let picked = picked_edges(&state.doc.map, &state.doc.selection, &[]);
        assert_eq!(apply(&mut state, &picked, &BevelSpec { width: 8.0, segments: 2 }), Ok(1));
        let b = state.doc.map.brush(id).unwrap();
        assert_eq!(b.faces.len(), 6 + 12 * 2);
        assert!(b.validate().is_ok());
    }
}
