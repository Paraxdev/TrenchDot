//! Place Along Path: copies of a piece repeated along a curve through a chain of path points, the piece bent to follow
//! it, or a cable hanging between the points.

use egui::RichText;
use td_core::{Aabb, DMat4, DVec3, NodeId};
use td_doc::{Map, NodeKind, Selection, ops};
use td_geom::FaceData;
use td_geom::mesh::Mesh;
use td_geom::spline::{self, Piece};
use td_geom::sweep::Curve;
use td_geom::uv::FaceUv;

use crate::i18n::{en, tr, tr_n};
use crate::state::EditorState;
use crate::sweep_dialog::{PointChain, point_chains};

/// Keeps a long run of detailed pieces from filling the viewports with lines.
const MAX_PREVIEW_SEGMENTS: usize = 200_000;

/// Points per span of a cable.
const CABLE_STEPS: u32 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplineMode {
    /// Rigid copies of the piece, like fence posts.
    #[default]
    Repeat,
    /// The piece laid end to end and bent, like a road, a trim or a pipe.
    Bend,
    /// A round cable hanging between the points.
    Cable,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplineSpec {
    pub mode: SplineMode,
    pub spacing: f64,
    pub upright: bool,
    pub sag: f64,
    pub radius: f64,
    pub sides: u32,
}

impl Default for SplineSpec {
    fn default() -> Self {
        Self { mode: SplineMode::Repeat, spacing: 64.0, upright: true, sag: 32.0, radius: 2.0, sides: 6 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplineError {
    NoPoints,
    NoPiece,
    NothingToBend,
    TooDetailed,
}

impl SplineError {
    pub fn message(self) -> &'static str {
        match self {
            SplineError::NoPoints => "place at least two points with the Path tool, the curve runs through them in order",
            SplineError::NoPiece => "select what to place along the path first",
            SplineError::NothingToBend => "only brushes and meshes bend, select one of those",
            SplineError::TooDetailed => "the piece has too many faces to bend along a path this long",
        }
    }

    fn text(self) -> String {
        match self {
            SplineError::NoPoints => tr("Place at least two points with the Path tool, the curve runs through them in order").to_string(),
            SplineError::NoPiece => tr("Select what to place along the path, then click Use Selection").to_string(),
            SplineError::NothingToBend => tr("Only brushes and meshes bend, select one of those").to_string(),
            SplineError::TooDetailed => tr("The piece has too many faces to bend along a path this long").to_string(),
        }
    }
}

/// What a placement adds: copies of the piece at each place, or new meshes beside the nodes they came from.
pub enum Placement {
    Copies(Vec<DMat4>),
    Meshes(Vec<(Option<NodeId>, Mesh)>),
}

/// The piece's roots, leaving out points of any chain so picking a path along with the piece still works.
pub fn piece_of(map: &Map, sel: &Selection) -> Vec<NodeId> {
    let points: std::collections::BTreeSet<NodeId> = point_chains(map).iter().flat_map(|c| c.ids.iter().copied()).collect();
    ops::selection_roots(map, sel).into_iter().filter(|id| !points.contains(id) && map.get(*id).is_some()).collect()
}

/// The chain `chain` names, else the one whose first point is nearest the piece, else the first.
pub fn pick_chain(map: &Map, piece: &[NodeId], chain: Option<NodeId>) -> Option<PointChain> {
    let chains = point_chains(map);
    if let Some(id) = chain {
        return chains.into_iter().find(|c| c.start == id);
    }

    let b = map.bounds_of(piece.iter().copied());
    let center = if b.is_empty() { DVec3::ZERO } else { b.center() };
    chains.into_iter().min_by(|a, b| a.points[0].distance(center).total_cmp(&b.points[0].distance(center)))
}

fn piece_bounds(map: &Map, piece: &[NodeId]) -> Option<Aabb> {
    let b = map.bounds_of(piece.iter().copied());
    (!b.is_empty()).then_some(b)
}

/// Brushes and meshes under the piece's roots, as meshes, with the node each came from.
fn bendable(map: &Map, piece: &[NodeId]) -> Vec<(NodeId, Mesh)> {
    piece
        .iter()
        .flat_map(|root| std::iter::once(*root).chain(map.descendants(*root)))
        .filter_map(|id| match &map.get(id)?.kind {
            NodeKind::Brush(b) => Some((id, Mesh::from_brush(b))),
            NodeKind::Mesh(m) => Some((id, m.clone())),
            _ => None,
        })
        .collect()
}

/// Works out what `spec` places along `points` without touching the map. A cable takes `material`.
pub fn build(state: &EditorState, spec: &SplineSpec, piece: &[NodeId], points: &[DVec3], material: &str) -> Result<Placement, SplineError> {
    let map = &state.doc.map;
    if spec.mode == SplineMode::Cable {
        let data = FaceData::new(material.to_string(), FaceUv::default());
        let mesh = spline::cable(points, spec.sag, spec.radius, spec.sides, CABLE_STEPS, &data).ok_or(SplineError::NoPoints)?;
        return Ok(Placement::Meshes(vec![(None, mesh)]));
    }

    let curve = Curve::through(points).ok_or(SplineError::NoPoints)?;
    let bounds = piece_bounds(map, piece).ok_or(SplineError::NoPiece)?;
    let shape = Piece::of(&bounds);
    match spec.mode {
        SplineMode::Repeat => Ok(Placement::Copies(spline::repeat(&curve, &shape, spec.spacing, spec.upright))),
        _ => {
            let size = |material: &str| crate::texture_ops::tex_size(state, material);
            let sources = bendable(map, piece);
            let cuts = spline::bend_cuts(&curve, &shape, sources.iter().map(|(_, m)| m.faces.len()).sum()).ok_or(SplineError::TooDetailed)?;
            let meshes: Vec<(Option<NodeId>, Mesh)> =
                sources.into_iter().filter_map(|(id, mesh)| spline::bend(&mesh, &shape, &curve, cuts, &size).map(|m| (Some(id), m))).collect();
            if meshes.is_empty() {
                return Err(SplineError::NothingToBend);
            }

            Ok(Placement::Meshes(meshes))
        }
    }
}

/// Adds what `spec` places along `points` as one undo step and selects it.
pub fn apply(state: &mut EditorState, spec: &SplineSpec, piece: &[NodeId], points: &[DVec3], material: &str) -> Result<Vec<NodeId>, SplineError> {
    let placement = build(state, spec, piece, points, material)?;
    let opts = state.opts();
    let fallback = state.insert_parent();
    Ok(state.doc.edit(en!("Place Along Path"), |m, s| {
        let mut ids = Vec::new();
        match placement {
            Placement::Copies(places) => {
                let mut source = Selection::default();
                source.nodes.extend(piece.iter().copied());
                for place in places {
                    let mut copy = source.clone();
                    ids.extend(ops::duplicate_selection(m, &mut copy, DVec3::ZERO, opts));
                    ops::transform_selection(m, &copy, &place, opts);
                }
            }
            Placement::Meshes(meshes) => {
                for (source, mesh) in meshes {
                    let parent = source.and_then(|id| m.get(id)).and_then(|n| n.parent).unwrap_or(fallback);
                    ids.push(m.insert(parent, NodeKind::Mesh(mesh)));
                }
            }
        }

        s.clear();
        s.nodes.extend(ids.iter().copied());
        ids
    }))
}

fn mesh_lines(mesh: &Mesh, out: &mut Vec<(DVec3, DVec3)>) {
    out.extend(mesh.edges().into_iter().map(|(a, b)| (mesh.vertices[a as usize], mesh.vertices[b as usize])));
}

/// The outline of everything under the piece's roots.
fn piece_lines(map: &Map, piece: &[NodeId]) -> Vec<(DVec3, DVec3)> {
    let mut out = Vec::new();
    for id in piece.iter().flat_map(|root| std::iter::once(*root).chain(map.descendants(*root))) {
        let Some(node) = map.get(id) else { continue };
        match &node.kind {
            NodeKind::Brush(b) => out.extend(b.edges().into_iter().map(|(a, c)| (b.vertices[a as usize], b.vertices[c as usize]))),
            NodeKind::Mesh(m) => mesh_lines(m, &mut out),
            NodeKind::Group(_) | NodeKind::Layer(_) => {}
            _ if !node.children.is_empty() => {}
            _ => {
                let b = map.bounds(id);
                if !b.is_empty() {
                    let c = b.corners();
                    const EDGES: [(usize, usize); 12] = [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)];
                    out.extend(EDGES.iter().map(|&(i, j)| (c[i], c[j])));
                }
            }
        }
    }

    out
}

fn preview_lines(map: &Map, piece: &[NodeId], placement: &Placement) -> Vec<(DVec3, DVec3)> {
    let mut out = Vec::new();
    match placement {
        Placement::Copies(places) => {
            let base = piece_lines(map, piece);
            for m in places {
                if out.len() + base.len() > MAX_PREVIEW_SEGMENTS {
                    break;
                }

                out.extend(base.iter().map(|(a, b)| (m.transform_point3(*a), m.transform_point3(*b))));
            }
        }
        Placement::Meshes(meshes) => {
            for (_, mesh) in meshes {
                mesh_lines(mesh, &mut out);
            }

            out.truncate(MAX_PREVIEW_SEGMENTS);
        }
    }

    out
}

/// What the preview was built from, so it is only rebuilt when one of them changes.
type PreviewKey = (SplineSpec, Vec<NodeId>, Option<NodeId>, u64);

#[derive(Default)]
pub struct SplineDialog {
    pub open: bool,
    spec: SplineSpec,
    /// Roots of what is placed, taken from the selection when the dialog opens or on Use Selection, so selecting and
    /// moving the path's points leaves it alone.
    piece: Vec<NodeId>,
    /// First entity of the chain to follow, None picks the chain that starts nearest the piece.
    chain: Option<NodeId>,
    was_open: bool,
    preview: Option<(PreviewKey, Result<usize, SplineError>)>,
}

impl SplineDialog {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        if !self.open {
            self.was_open = false;
            if self.preview.take().is_some() {
                state.spline_preview.clear();
            }

            return;
        }

        if !self.was_open {
            self.was_open = true;
            self.piece = piece_of(&state.doc.map, &state.doc.selection);
        }

        self.piece.retain(|id| state.doc.map.get(*id).is_some());
        let spec = &mut self.spec;
        let piece = &mut self.piece;
        let chain = &mut self.chain;
        let preview = &mut self.preview;
        let mut open = self.open;
        let mut create = None;
        crate::widgets::window(en!("Place Along Path")).open(&mut open).resizable(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (mode, label, tip) in [
                    (SplineMode::Repeat, en!("Repeat"), en!("Copies of the selection along the path, like fence posts")),
                    (SplineMode::Bend, en!("Bend"), en!("The selection laid end to end and bent along the path, like a road or a pipe")),
                    (SplineMode::Cable, en!("Cable"), en!("A cable hanging between the points, in the current material")),
                ] {
                    ui.selectable_value(&mut spec.mode, mode, tr(label)).on_hover_text(tr(tip));
                }
            });

            let curve = pick_chain(&state.doc.map, piece, *chain);
            egui::Grid::new("spline_options").num_columns(2).show(ui, |ui| {
                let label = |c: &PointChain| format!("{}, {}", c.name, tr_n!(c.points.len(), "{n} point", "{n} points"));
                ui.label(tr("Points"));
                let selected = curve.as_ref().map(label).unwrap_or_default();
                egui::ComboBox::from_id_salt("spline_chain").selected_text(selected).show_ui(ui, |ui| {
                    for c in point_chains(&state.doc.map) {
                        if ui.selectable_label(curve.as_ref().is_some_and(|k| k.start == c.start), label(&c)).clicked() {
                            *chain = Some(c.start);
                        }
                    }
                });
                ui.end_row();

                if spec.mode != SplineMode::Cable {
                    ui.label(tr("Piece"));
                    ui.horizontal(|ui| {
                        ui.label(tr_n!(piece.len(), "{n} object", "{n} objects"));
                        if ui.button(tr("Use Selection")).clicked() {
                            *piece = piece_of(&state.doc.map, &state.doc.selection);
                        }
                    });
                    ui.end_row();
                }

                match spec.mode {
                    SplineMode::Repeat => {
                        ui.label(tr("Spacing"));
                        ui.add(egui::DragValue::new(&mut spec.spacing).range(1.0..=65536.0).suffix(" u"))
                            .on_hover_text(tr("Evened out so the first and last copies sit on the ends of the path"));
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut spec.upright, tr("Upright")).on_hover_text(tr("Copies stand straight up on a slope instead of leaning with it"));
                        ui.end_row();
                    }
                    SplineMode::Bend => {}
                    SplineMode::Cable => {
                        ui.label(tr("Sag"));
                        ui.add(egui::DragValue::new(&mut spec.sag).range(-65536.0..=65536.0).suffix(" u"))
                            .on_hover_text(tr("How far the middle of each span hangs below its ends"));
                        ui.end_row();
                        ui.label(tr("Radius"));
                        ui.add(egui::DragValue::new(&mut spec.radius).range(0.1..=1024.0).speed(0.1).suffix(" u"));
                        ui.end_row();
                        ui.label(tr("Sides"));
                        ui.add(egui::Slider::new(&mut spec.sides, 3..=64));
                        ui.end_row();
                    }
                }
            });

            let points = curve.map(|c| c.points).unwrap_or_default();
            let key = (*spec, piece.clone(), *chain, state.doc.revision);
            if preview.as_ref().is_none_or(|(k, _)| *k != key) {
                let built = build(state, spec, piece, &points, &state.current_material);
                state.spline_preview = built.as_ref().map(|p| preview_lines(&state.doc.map, piece, p)).unwrap_or_default();
                let count = built.map(|p| match p {
                    Placement::Copies(c) => c.len(),
                    Placement::Meshes(m) => m.len(),
                });
                *preview = Some((key, count));
            }

            match preview.as_ref().map(|(_, r)| *r) {
                Some(Ok(n)) => {
                    let text = match spec.mode {
                        SplineMode::Repeat => tr_n!(n, "{n} copy", "{n} copies"),
                        _ => tr_n!(n, "{n} mesh", "{n} meshes"),
                    };
                    ui.label(RichText::new(text).weak());
                    if ui.button(tr("Create")).clicked() {
                        create = Some(points);
                    }
                }
                Some(Err(e)) => _ = ui.colored_label(ui.visuals().warn_fg_color, e.text()),
                None => {}
            }
        });
        if !open || create.is_some() {
            self.preview = None;
            state.spline_preview.clear();
        }

        self.open = open;
        let material = state.current_material.clone();
        if let Some(points) = create
            && let Err(e) = apply(state, &self.spec, &self.piece, &points, &material)
        {
            state.set_status(e.text());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Prefs;
    use td_doc::Entity;

    fn corner(state: &mut EditorState, name: &str, target: &str, origin: DVec3) -> NodeId {
        let mut e = Entity::new("path_corner");
        e.origin = origin;
        e.properties.insert("targetname".into(), name.into());
        if !target.is_empty() {
            e.properties.insert("target".into(), target.into());
        }

        let layer = state.doc.map.default_layer();
        state.doc.map.insert(layer, NodeKind::Entity(e))
    }

    fn setup() -> (EditorState, NodeId, Vec<DVec3>) {
        let mut state = EditorState::new(Prefs::default());
        let brush = td_geom::Brush::from_aabb(&Aabb::new(DVec3::new(-8.0, 0.0, -8.0), DVec3::new(8.0, 64.0, 8.0)), "dev/grey").unwrap();
        let layer = state.doc.map.default_layer();
        let post = state.doc.map.insert(layer, NodeKind::Brush(brush));
        let a = corner(&mut state, "p1", "p2", DVec3::new(100.0, 0.0, 0.0));
        corner(&mut state, "p2", "", DVec3::new(356.0, 0.0, 0.0));
        state.doc.selection.nodes.extend([post, a]);
        (state, post, vec![DVec3::new(100.0, 0.0, 0.0), DVec3::new(356.0, 0.0, 0.0)])
    }

    #[test]
    fn the_piece_leaves_out_path_points_and_finds_the_nearest_chain() {
        let (state, post, points) = setup();
        let piece = piece_of(&state.doc.map, &state.doc.selection);
        assert_eq!(piece, vec![post]);
        assert_eq!(pick_chain(&state.doc.map, &piece, None).unwrap().points, points);
    }

    #[test]
    fn repeat_bend_and_cable_add_one_undo_step_each() {
        let (mut state, post, points) = setup();
        let start = state.doc.map.walk();
        let undo_once = |state: &mut EditorState| {
            assert_eq!(state.undo().as_deref(), Some("Place Along Path"));
            assert!(!state.doc.history.can_undo());
            assert_eq!(state.doc.map.walk(), start);
        };

        let spec = SplineSpec::default();
        let ids = apply(&mut state, &spec, &[post], &points, "dev/grey").unwrap();
        assert_eq!(ids.len(), 5);
        let first = state.doc.map.bounds(ids[0]);
        assert!(first.center().distance(DVec3::new(100.0, 32.0, 0.0)) < 0.5, "{first:?}");
        undo_once(&mut state);

        let ids = apply(&mut state, &SplineSpec { mode: SplineMode::Bend, ..spec }, &[post], &points, "dev/grey").unwrap();
        assert!(matches!(state.doc.map.get(ids[0]).map(|n| &n.kind), Some(NodeKind::Mesh(_))));
        undo_once(&mut state);

        let ids = apply(&mut state, &SplineSpec { mode: SplineMode::Cable, ..spec }, &[], &points, "dev/grey").unwrap();
        assert_eq!(ids.len(), 1);
        undo_once(&mut state);
        assert_eq!(apply(&mut state, &spec, &[], &points, "dev/grey"), Err(SplineError::NoPiece));
        assert_eq!(apply(&mut state, &spec, &[post], &points[..1], "dev/grey"), Err(SplineError::NoPoints));
    }

    #[test]
    fn a_piece_too_detailed_to_bend_along_a_long_path_is_refused() {
        let (mut state, _, points) = setup();
        let data = FaceData::new("dev/grey", FaceUv::default());
        let rope = spline::cable(&[DVec3::ZERO, DVec3::new(64.0, 0.0, 0.0)], 0.0, 4.0, 64, 8, &data).unwrap();
        let layer = state.doc.map.default_layer();
        let rope = state.doc.map.insert(layer, NodeKind::Mesh(rope));
        let spec = SplineSpec { mode: SplineMode::Bend, ..Default::default() };
        assert!(build(&state, &spec, &[rope], &points, "dev/grey").is_ok());
        let far = [points[0], points[0] + DVec3::X * 65536.0];
        assert_eq!(build(&state, &spec, &[rope], &far, "dev/grey").err(), Some(SplineError::TooDetailed));
    }
}
