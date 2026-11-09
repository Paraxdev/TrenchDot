//! Array Copies: the selection copied in a run, each copy offset, turned and scaled from the one before.

use egui::RichText;
use td_core::{Aabb, DVec3, NodeId};
use td_doc::ops::{self, ArraySpec, MAX_COPIES};
use td_doc::{Map, NodeKind, Selection};

use crate::i18n::{en, tr, tr_n};
use crate::state::EditorState;

/// Keeps a huge array from filling the viewports with lines.
const MAX_PREVIEW_SEGMENTS: usize = 200_000;

/// What the preview was built from, so it is only rebuilt when one of them changes.
type PreviewKey = (ArraySpec, Vec<NodeId>, u64);

pub struct ArrayDialog {
    pub open: bool,
    spec: ArraySpec,
    preview: Option<PreviewKey>,
}

impl Default for ArrayDialog {
    fn default() -> Self {
        Self { open: false, spec: ArraySpec { count: 3, offset: DVec3::new(64.0, 0.0, 0.0), ..Default::default() }, preview: None }
    }
}

/// The outline of everything the selection holds, as the segments to draw for each copy.
fn outline(map: &Map, sel: &Selection) -> Vec<(DVec3, DVec3)> {
    let mut out = Vec::new();
    for root in ops::selection_roots(map, sel) {
        for id in std::iter::once(root).chain(map.descendants(root)) {
            let Some(node) = map.get(id) else { continue };
            match &node.kind {
                NodeKind::Brush(b) => out.extend(b.edges().into_iter().map(|(a, c)| (b.vertices[a as usize], b.vertices[c as usize]))),
                NodeKind::Mesh(m) => out.extend(m.edges().into_iter().map(|(a, c)| (m.vertices[a as usize], m.vertices[c as usize]))),
                NodeKind::Group(_) => {}
                _ if !node.children.is_empty() => {}
                _ => {
                    let b = map.bounds(id);
                    if !b.is_empty() {
                        out.extend(box_edges(&b));
                    }
                }
            }
        }
    }

    out
}

fn box_edges(b: &Aabb) -> impl Iterator<Item = (DVec3, DVec3)> {
    let c = b.corners();
    const EDGES: [(usize, usize); 12] = [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)];
    EDGES.into_iter().map(move |(i, j)| (c[i], c[j]))
}

fn preview_lines(map: &Map, sel: &Selection, spec: &ArraySpec) -> Vec<(DVec3, DVec3)> {
    let base = outline(map, sel);
    let center = map.bounds_of(sel.nodes.iter().copied()).center();
    let mut out = Vec::new();
    for m in spec.matrices(center) {
        if out.len() + base.len() > MAX_PREVIEW_SEGMENTS {
            break;
        }

        out.extend(base.iter().map(|(a, b)| (m.transform_point3(*a), m.transform_point3(*b))));
    }

    out
}

fn vec3_row(ui: &mut egui::Ui, v: &mut DVec3, range: std::ops::RangeInclusive<f64>, suffix: &str, speed: f64) {
    ui.horizontal(|ui| {
        for (axis, name) in [(0, "X "), (1, "Y "), (2, "Z ")] {
            ui.add(egui::DragValue::new(&mut v[axis]).range(range.clone()).speed(speed).prefix(name).suffix(suffix));
        }
    });
}

impl ArrayDialog {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        if !self.open {
            if self.preview.take().is_some() {
                state.array_preview.clear();
            }

            return;
        }

        let spec = &mut self.spec;
        let preview = &mut self.preview;
        let mut open = self.open;
        let mut create = false;
        crate::widgets::window(en!("Array Copies")).open(&mut open).resizable(false).show(ctx, |ui| {
            egui::Grid::new("array_options").num_columns(2).show(ui, |ui| {
                ui.label(tr("Copies"));
                ui.add(egui::Slider::new(&mut spec.count, 1..=MAX_COPIES).logarithmic(true));
                ui.end_row();
                ui.label(tr("Offset"));
                vec3_row(ui, &mut spec.offset, -65536.0..=65536.0, " u", 1.0);
                ui.end_row();
                ui.label(tr("Rotate"));
                vec3_row(ui, &mut spec.rotate, -360.0..=360.0, "°", 0.5);
                ui.end_row();
                ui.label(tr("Scale"));
                vec3_row(ui, &mut spec.scale, 0.05..=20.0, "", 0.01);
                ui.end_row();
                ui.label(tr("Pivot"));
                ui.horizontal(|ui| {
                    let custom = spec.pivot.is_some();
                    if ui.selectable_label(!custom, tr("Center")).clicked() {
                        spec.pivot = None;
                    }

                    if ui.selectable_label(custom, tr("Custom")).clicked() && !custom {
                        spec.pivot = Some(state.doc.map.bounds_of(state.doc.selection.nodes.iter().copied()).center());
                    }
                })
                .response
                .on_hover_text(tr("Where each copy turns and scales, Center is the middle of the selection"));
                ui.end_row();
                if let Some(p) = &mut spec.pivot {
                    ui.label("");
                    vec3_row(ui, p, -65536.0..=65536.0, " u", 1.0);
                    ui.end_row();
                }
            });
            ui.checkbox(&mut spec.linked, tr("Linked")).on_hover_text(tr("Linked copies update together when one is edited"));

            if state.doc.selection.nodes.is_empty() {
                ui.label(RichText::new(tr("Select the objects to copy")).weak());
                return;
            }

            let key = (*spec, state.doc.selection.nodes.iter().copied().collect(), state.doc.revision);
            if preview.as_ref() != Some(&key) {
                state.array_preview = match spec.validate() {
                    Ok(()) => preview_lines(&state.doc.map, &state.doc.selection, spec),
                    Err(_) => Vec::new(),
                };
                *preview = Some(key);
            }

            if spec.validate().is_err() {
                ui.colored_label(ui.visuals().warn_fg_color, tr("The scale grows or shrinks the copies too much"));
                return;
            }

            ui.label(RichText::new(tr_n!(spec.count, "{n} copy", "{n} copies")).weak());
            create = ui.button(tr("Create")).clicked();
        });
        if !open || create || state.doc.selection.nodes.is_empty() {
            self.preview = None;
            state.array_preview.clear();
        }

        self.open = open && !create;
        if create {
            let parent = state.insert_parent();
            let opts = ops::EditOptions { uv_lock: state.uv_lock, grid: 0.0 };
            let spec = self.spec;
            state.doc.edit(en!("Array Copies"), |m, s| ops::array_copies(m, s, &spec, parent, opts));
        }
    }
}
