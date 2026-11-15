//! Sweep Faces: the selected brush faces swept along a straight line, an arc, an S-bend or a curve through a chain of
//! path points into a run of brushes.

use std::collections::{BTreeMap, BTreeSet};

use egui::RichText;
use td_core::{DVec3, NodeId};
use td_doc::{Map, NodeKind, Selection};
use td_geom::sweep::{Bend, Frame, MAX_SEGMENTS, Sweep, SweepError, SweepFace, SweepPath};

use crate::i18n::{en, tr, tr_n, trf};
use crate::state::EditorState;

/// What the preview was built from, so it is only rebuilt when one of them changes.
type PreviewKey = (Sweep, Vec<(NodeId, usize)>, Option<NodeId>, u64);

/// What the picked faces and the curve were found from, so the brushes are only cloned and the map only walked when one
/// of them changes.
type PickKey = (Vec<(NodeId, usize)>, Option<NodeId>, bool, u64);

struct Picked {
    key: PickKey,
    faces: Vec<(NodeId, SweepFace)>,
    curve: Option<PointChain>,
}

#[cfg(test)]
thread_local! {
    static WALKS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

#[derive(Default)]
pub struct SweepDialog {
    pub open: bool,
    pub sweep: Sweep,
    /// First entity of the chain a curve runs through, None picks the chain that starts nearest the faces.
    pub chain: Option<NodeId>,
    preview: Option<(PreviewKey, Result<usize, SweepError>)>,
    picked: Option<Picked>,
}

/// Point entities linked one to the next by target, like the path_corner chains the Path tool places.
#[derive(Clone, Debug, PartialEq)]
pub struct PointChain {
    pub start: NodeId,
    pub name: String,
    pub points: Vec<DVec3>,
    pub ids: Vec<NodeId>,
}

/// Every chain in the map. A chain is entities of one classname where each targets the next. It starts at one no
/// other entity of its classname targets, and a closed loop starts at its lowest id.
pub fn point_chains(map: &Map) -> Vec<PointChain> {
    #[cfg(test)]
    WALKS.with(|w| w.set((w.get().0 + 1, w.get().1)));
    let entities: BTreeMap<NodeId, &td_doc::Entity> = map.entities().collect();
    let mut by_name: BTreeMap<(&str, &str), NodeId> = BTreeMap::new();
    for (id, e) in &entities {
        if let Some(name) = e.targetname() {
            by_name.entry((e.classname.as_str(), name)).or_insert(*id);
        }
    }

    let next = |id: NodeId| -> Option<NodeId> {
        let e = entities.get(&id)?;
        by_name.get(&(e.classname.as_str(), e.property("target")?)).copied().filter(|n| *n != id)
    };
    let targeted: BTreeSet<NodeId> = entities.keys().filter_map(|id| next(*id)).collect();
    let in_chain = |id: NodeId| entities[&id].classname == "path_corner" || next(id).is_some() || targeted.contains(&id);
    let mut seen = BTreeSet::new();
    let mut chains = Vec::new();
    let starts: Vec<NodeId> =
        entities.keys().copied().filter(|id| in_chain(*id) && !targeted.contains(id)).chain(entities.keys().copied().filter(|id| in_chain(*id))).collect();
    for start in starts {
        if seen.contains(&start) {
            continue;
        }

        let (mut points, mut ids) = (Vec::new(), Vec::new());
        let mut at = Some(start);
        while let Some(id) = at.filter(|id| seen.insert(*id)) {
            points.push(entities[&id].origin);
            ids.push(id);
            at = next(id);
        }

        let e = entities[&start];
        let name = e.targetname().map_or_else(|| format!("{} {}", e.classname, start.0), str::to_string);
        chains.push(PointChain { start, name, points, ids });
    }

    chains
}

/// The chain `chain` names, else the one whose first point is nearest the faces.
pub fn pick_chain(map: &Map, faces: &[SweepFace], chain: Option<NodeId>) -> Option<PointChain> {
    let chains = point_chains(map);
    if let Some(id) = chain {
        return chains.into_iter().find(|c| c.start == id);
    }

    let center = Frame::of(faces)?.center;
    chains.into_iter().min_by(|a, b| a.points[0].distance(center).total_cmp(&b.points[0].distance(center)))
}

/// The selected brush faces, each with the brush it belongs to.
pub fn selected_faces(map: &Map, selection: &Selection) -> Vec<(NodeId, SweepFace)> {
    #[cfg(test)]
    WALKS.with(|w| w.set((w.get().0, w.get().1 + 1)));
    selection
        .faces
        .iter()
        .filter_map(|&(id, face)| {
            let brush = map.brush(id)?;
            (face < brush.faces.len()).then(|| (id, SweepFace { brush: brush.clone(), face }))
        })
        .collect()
}

/// Adds the swept brushes beside their source brushes as one undo step and selects them. `points` are what a curve runs
/// through.
pub fn apply(state: &mut EditorState, picked: &[(NodeId, SweepFace)], sweep: &Sweep, points: &[DVec3]) -> Result<Vec<NodeId>, SweepError> {
    let faces: Vec<SweepFace> = picked.iter().map(|(_, f)| f.clone()).collect();
    let brushes = sweep.brushes_through(&faces, points)?;
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
        SweepError::NoPoints => tr("Place points with the Path tool, the curve runs through them in order").to_string(),
        SweepError::Bend => tr("A bend is tighter than the faces are wide, move the points further apart").to_string(),
        _ => tr("These settings leave a segment without volume").to_string(),
    }
}

impl SweepDialog {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        if !self.open {
            self.picked = None;
            if self.preview.take().is_some() {
                state.sweep_preview.clear();
            }

            return;
        }

        let s = &mut self.sweep;
        let chain = &mut self.chain;
        let preview = &mut self.preview;
        let cached = &mut self.picked;
        let mut open = self.open;
        let mut create = None;
        crate::widgets::window(en!("Sweep Faces")).open(&mut open).resizable(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (path, label) in
                    [(SweepPath::Straight, en!("Straight")), (SweepPath::Arc, en!("Arc")), (SweepPath::SBend, en!("S-bend")), (SweepPath::Curve, en!("Curve"))]
                {
                    let r = ui.selectable_value(&mut s.path, path, tr(label));
                    if path == SweepPath::Curve {
                        r.on_hover_text(tr("Through a chain of points from the Path tool, move or add points to reshape it"));
                    }
                }
            });
            let pick_key = (state.doc.selection.faces.iter().copied().collect(), *chain, s.path == SweepPath::Curve, state.doc.revision);
            if cached.as_ref().is_none_or(|p| p.key != pick_key) {
                let faces = selected_faces(&state.doc.map, &state.doc.selection);
                let curve = if pick_key.2 {
                    let sweep_faces: Vec<SweepFace> = faces.iter().map(|(_, f)| f.clone()).collect();
                    pick_chain(&state.doc.map, &sweep_faces, *chain)
                } else {
                    None
                };
                *cached = Some(Picked { key: pick_key, faces, curve });
            }

            let Some(Picked { faces: picked, curve, .. }) = cached.as_ref() else { return };
            egui::Grid::new("sweep_options").num_columns(2).show(ui, |ui| {
                match s.path {
                    SweepPath::Arc => {
                        ui.label(tr("Radius"));
                        ui.add(egui::DragValue::new(&mut s.length).range(1.0..=65536.0).suffix(" u"))
                            .on_hover_text(tr("From the center of the turn to the middle of the faces"));
                        ui.end_row();
                        ui.label(tr("Angle"));
                        ui.add(egui::DragValue::new(&mut s.angle).range(1.0..=720.0).suffix("°"));
                        ui.end_row();
                    }
                    SweepPath::Curve => {
                        let label = |c: &PointChain| format!("{}, {}", c.name, tr_n!(c.points.len(), "{n} point", "{n} points"));
                        ui.label(tr("Points"));
                        let selected = curve.as_ref().map(label).unwrap_or_default();
                        egui::ComboBox::from_id_salt("sweep_chain").selected_text(selected).show_ui(ui, |ui| {
                            for c in point_chains(&state.doc.map) {
                                if ui.selectable_label(curve.as_ref().is_some_and(|k| k.start == c.start), label(&c)).clicked() {
                                    *chain = Some(c.start);
                                }
                            }
                        });
                        ui.end_row();
                    }
                    _ => {
                        ui.label(tr("Length"));
                        ui.add(egui::DragValue::new(&mut s.length).range(1.0..=65536.0).suffix(" u"));
                        ui.end_row();
                    }
                }

                if s.path == SweepPath::SBend {
                    ui.label(tr("Offset"));
                    ui.add(egui::DragValue::new(&mut s.offset).range(-65536.0..=65536.0).suffix(" u")).on_hover_text(tr("How far the end shifts sideways"));
                    ui.end_row();
                }

                if matches!(s.path, SweepPath::Arc | SweepPath::SBend) {
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

            let points = curve.as_ref().map(|c| c.points.as_slice()).unwrap_or_default();
            let key = (*s, state.doc.selection.faces.iter().copied().collect(), *chain, state.doc.revision);
            if preview.as_ref().is_none_or(|(k, _)| *k != key) {
                let faces: Vec<SweepFace> = picked.iter().map(|(_, f)| f.clone()).collect();
                let built = s.brushes_through(&faces, points);
                let count = built.as_ref().map(Vec::len).map_err(|e| *e);
                state.sweep_preview = built.unwrap_or_default();
                *preview = Some((key, count));
            }

            match preview.as_ref().map(|(_, r)| *r) {
                Some(Ok(n)) => {
                    ui.label(RichText::new(tr_n!(n, "{n} brush", "{n} brushes")).weak());
                    if ui.button(tr("Create")).clicked() {
                        create = Some((picked.clone(), points.to_vec()));
                    }
                }
                Some(Err(e)) => _ = ui.colored_label(ui.visuals().warn_fg_color, error_text(e)),
                None => {}
            }
        });
        if !open || create.is_some() || state.doc.selection.faces.is_empty() {
            self.preview = None;
            state.sweep_preview.clear();
        }

        self.open = open;
        if let Some((picked, points)) = create
            && let Err(e) = apply(state, &picked, &self.sweep, &points)
        {
            state.set_status(error_text(e));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corner(map: &mut Map, name: &str, target: &str, origin: DVec3) -> NodeId {
        let mut e = td_doc::Entity::new("path_corner");
        e.origin = origin;
        e.properties.insert("targetname".into(), name.into());
        if !target.is_empty() {
            e.properties.insert("target".into(), target.into());
        }

        let layer = map.default_layer();
        map.insert(layer, NodeKind::Entity(e))
    }

    #[test]
    fn chains_follow_targets_from_their_start_and_loops_start_at_the_lowest_id() {
        let mut map = Map::new();
        let c = corner(&mut map, "c", "", DVec3::new(0.0, 0.0, 300.0));
        let a = corner(&mut map, "a", "b", DVec3::new(0.0, 0.0, 100.0));
        corner(&mut map, "b", "c", DVec3::new(0.0, 0.0, 200.0));
        let x = corner(&mut map, "x", "y", DVec3::new(500.0, 0.0, 0.0));
        corner(&mut map, "y", "x", DVec3::new(600.0, 0.0, 0.0));
        let mut light = td_doc::Entity::new("light");
        light.properties.insert("targetname".into(), "lamp".into());
        let layer = map.default_layer();
        map.insert(layer, NodeKind::Entity(light));

        let chains = point_chains(&map);
        assert_eq!(chains.len(), 2);
        assert_eq!((chains[0].start, chains[0].name.as_str(), chains[0].points.len()), (a, "a", 3));
        assert_eq!(chains[0].points[2].z, 300.0);
        assert_eq!((chains[1].start, chains[1].points.len()), (x, 2));
        assert!(chains.iter().all(|ch| ch.start != c));
    }

    fn frame(dialog: &mut SweepDialog, ctx: &egui::Context, state: &mut EditorState) -> (usize, usize) {
        WALKS.with(|w| w.set((0, 0)));
        ctx.begin_pass(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() });
        dialog.show(ctx, state);
        ctx.end_pass().drop_without_applying_deltas();
        WALKS.with(|w| w.get())
    }

    #[test]
    fn faces_and_chains_are_only_looked_up_when_the_selection_or_the_document_changes() {
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let bounds = td_core::Aabb::new(DVec3::new(40.0, -24.0, 8.0), DVec3::new(168.0, 40.0, 72.0));
        let brush = state.doc.map.insert(layer, NodeKind::Brush(td_geom::Brush::from_aabb(&bounds, "dev/grey").unwrap()));
        let first = corner(&mut state.doc.map, "a", "b", DVec3::new(300.0, 10.0, 20.0));
        corner(&mut state.doc.map, "b", "", DVec3::new(500.0, 90.0, 60.0));
        let second = corner(&mut state.doc.map, "x", "y", DVec3::new(-300.0, 50.0, 10.0));
        corner(&mut state.doc.map, "y", "", DVec3::new(-500.0, 70.0, 30.0));
        state.doc.selection.faces.insert((brush, 0));

        let ctx = egui::Context::default();
        let mut dialog = SweepDialog { open: true, ..Default::default() };
        dialog.sweep.path = SweepPath::Curve;
        assert_eq!(frame(&mut dialog, &ctx, &mut state), (1, 1), "the first frame looks everything up");
        let built = state.sweep_preview.len();
        assert!(built > 0);
        assert_eq!(dialog.picked.as_ref().and_then(|p| p.curve.as_ref()).map(|c| c.start), Some(first), "the chain nearest the faces");

        assert_eq!(frame(&mut dialog, &ctx, &mut state), (0, 0), "a cache hit looks nothing up");
        assert_eq!(frame(&mut dialog, &ctx, &mut state), (0, 0));
        assert_eq!(state.sweep_preview.len(), built);

        state.doc.selection.faces.insert((brush, 1));
        assert_eq!(frame(&mut dialog, &ctx, &mut state), (1, 1), "another face is a miss");
        assert_eq!(frame(&mut dialog, &ctx, &mut state), (0, 0));

        dialog.chain = Some(second);
        assert_eq!(frame(&mut dialog, &ctx, &mut state), (1, 1), "another chain is a miss");
        assert_eq!(dialog.picked.as_ref().and_then(|p| p.curve.as_ref()).map(|c| c.start), Some(second));
        assert_eq!(frame(&mut dialog, &ctx, &mut state), (0, 0));

        state.doc.revision += 1;
        assert_eq!(frame(&mut dialog, &ctx, &mut state), (1, 1), "an edit is a miss");
        assert_eq!(frame(&mut dialog, &ctx, &mut state), (0, 0));

        dialog.sweep.path = SweepPath::Straight;
        assert_eq!(frame(&mut dialog, &ctx, &mut state), (0, 1), "a straight sweep looks up no chain");
        assert!(dialog.picked.as_ref().is_some_and(|p| p.curve.is_none() && p.faces.len() == 2));
        assert_eq!(frame(&mut dialog, &ctx, &mut state), (0, 0));
    }
}
