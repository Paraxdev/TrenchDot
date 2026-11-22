//! Blend tool: paints material blends on terrain layers, displacement alpha and faces that have a blend material.

use egui::{Color32, PointerButton, Pos2, Rect, Response, Ui};
use td_core::{DVec3, NodeId, Plane};
use td_doc::blend::{self, BlendMode};
use td_doc::symmetry::Symmetry;
use td_render::LineVertex;

use crate::camera::Camera;
use crate::i18n::{msg, tr, trf};
use crate::picking;
use crate::scene::v3;
use crate::state::EditorState;
use crate::widgets::corner_hint;

/// What one dab edits: terrains, displacement faces and blend faces, from the selection or the whole map.
pub struct BlendTargets {
    pub terrains: Vec<NodeId>,
    pub displacements: Vec<(NodeId, usize)>,
    pub faces: Vec<(NodeId, usize)>,
}

impl BlendTargets {
    pub fn is_empty(&self) -> bool {
        self.terrains.is_empty() && self.displacements.is_empty() && self.faces.is_empty()
    }
}

/// Brushes and terrains a brush tool (sculpt, paint, blend) works on: the selected ones, or every one when nothing
/// is selected. Locked and hidden ones are always left alone.
pub struct StrokeScope {
    pub brushes: Vec<NodeId>,
    pub terrains: Vec<NodeId>,
}

impl StrokeScope {
    pub fn of(state: &EditorState) -> Self {
        let map = &state.doc.map;
        let sel = &state.doc.selection;
        let (brushes, terrains): (Vec<NodeId>, Vec<NodeId>) = if sel.is_empty() {
            (map.brushes().map(|(id, _)| id).collect(), map.terrains().map(|(id, _)| id).collect())
        } else {
            let mut brushes = sel.brushes(map);
            brushes.extend(sel.faces.iter().map(|(id, _)| *id).filter(|id| map.brush(*id).is_some()));
            brushes.sort();
            brushes.dedup();
            (brushes, sel.terrains(map))
        };
        Self {
            brushes: brushes.into_iter().filter(|id| map.is_editable(*id)).collect(),
            terrains: terrains.into_iter().filter(|id| map.is_editable(*id)).collect(),
        }
    }

    /// Displacement faces of the scope's brushes. `displacement_faces` reads an empty list as the whole map.
    pub fn displacements(&self, state: &EditorState) -> Vec<(NodeId, usize)> {
        if self.brushes.is_empty() { Vec::new() } else { td_doc::terrain::displacement_faces(&state.doc.map, &self.brushes) }
    }
}

pub fn targets(state: &EditorState) -> BlendTargets {
    let map = &state.doc.map;
    let sel = &state.doc.selection;
    let scope = StrokeScope::of(state);
    let displacements = scope.displacements(state);
    let faces = if sel.has_faces() {
        sel.faces.iter().copied().filter(|(id, f)| map.is_editable(*id) && blend::blend_faces(map, &[*id]).contains(&(*id, *f))).collect()
    } else {
        let geometry: Vec<NodeId> = sel.geometry(map).into_iter().filter(|id| map.is_editable(*id)).collect();
        // `blend_faces` reads an empty list as the whole map, which is only meant when nothing is selected.
        if geometry.is_empty() && !sel.is_empty() {
            Vec::new()
        } else {
            blend::blend_faces(map, &geometry).into_iter().filter(|(id, _)| map.is_editable(*id)).collect()
        }
    };
    BlendTargets { terrains: scope.terrains, displacements, faces }
}

/// One dab. Returns true when anything changed.
pub fn dab(state: &mut EditorState, center: DVec3, brush: &blend::BlendBrush, symmetry: &Symmetry) -> bool {
    let t = targets(state);
    state.doc.edit("Blend", |m, _| {
        let mut changed = false;
        for id in &t.terrains {
            if let Some(terrain) = m.terrain_mut(*id) {
                changed |= blend::blend_terrain(terrain, center, brush, &symmetry.for_terrain(terrain));
            }
        }

        let mut faces = t.displacements.clone();
        faces.extend(t.faces.iter().copied());
        let mirror = symmetry.for_faces(m, &faces);
        changed |= blend::blend_displacements(m, &t.displacements, center, brush, &mirror);
        changed |= blend::blend_face_corners(m, &t.faces, center, brush, &mirror);
        changed
    })
}

/// The selected faces, or every face of the selected geometry when no face is picked.
fn blend_target_faces(state: &EditorState) -> Vec<(NodeId, usize)> {
    let map = &state.doc.map;
    if state.doc.selection.has_faces() {
        return state.doc.selection.faces.iter().copied().collect();
    }

    state
        .doc
        .selection
        .geometry(map)
        .into_iter()
        .flat_map(|id| {
            let n = map.brush(id).map(|b| b.faces.len()).or_else(|| map.mesh(id).map(|m| m.faces.len())).unwrap_or(0);
            (0..n).map(move |f| (id, f))
        })
        .collect()
}

/// Sets the blend material on the selected faces, or on every face of the selected brushes and meshes.
pub fn set_blend_material(state: &mut EditorState, material: Option<&str>) -> usize {
    let faces = blend_target_faces(state);
    let material = material.map(str::to_string);
    state.doc.edit("Blend Material", |m, _| blend::set_blend_material(m, &faces, material.as_deref()))
}

/// Sets the blend material's repeat and de-tiling on the same faces `set_blend_material` would touch.
pub fn set_blend_tiling(state: &mut EditorState, detile: f64, uv_scale: f64, sharpen: f64) -> usize {
    let faces = blend_target_faces(state);
    state.doc.edit("Blend Tiling", |m, _| blend::set_blend_options(m, &faces, detile, uv_scale, sharpen))
}

/// The brush ring color in linear space.
pub const RING_COLOR: [f32; 4] = [0.95, 0.6, 1.0, 0.95];

#[derive(Default)]
pub struct BlendTool {
    pub hover: Option<(DVec3, DVec3)>,
    stroking: bool,
    view: Option<egui::Id>,
    last: Option<DVec3>,
    dabs: u32,
}

impl BlendTool {
    pub fn stroking(&self) -> bool {
        self.stroking
    }

    /// Ends a stroke without touching the document, the caller owns its transaction.
    pub fn reset(&mut self) {
        self.hover = None;
        self.last = None;
        self.stroking = false;
        self.view = None;
    }

    pub fn input(&mut self, ui: &Ui, response: &Response, cam: &Camera, rect: Rect, hover: Option<Pos2>, state: &mut EditorState) {
        // Every visible view runs this with the one global pointer, only the view the stroke started in may dab.
        if self.stroking && self.view != Some(response.id) {
            return;
        }

        let pointer = if self.stroking { ui.input(|i| i.pointer.interact_pos()) } else { hover };
        self.hover = pointer
            .and_then(|p| {
                picking::pick_all(state, &cam.ray(rect, p))
                    .into_iter()
                    .find(|h| state.doc.map.entity(h.node).is_none() && state.doc.map.scatter(h.node).is_none())
            })
            .map(|h| (h.point, h.normal));
        let modifiers = ui.input(|i| i.modifiers);
        if (response.drag_started_by(PointerButton::Primary) || response.clicked_by(PointerButton::Primary)) && !self.stroking && self.hover.is_some() {
            if targets(state).is_empty() {
                state.set_status(msg!("Nothing to blend: add terrain layers or displacements, or use Terrain > Blend > Set Blend Material on faces"));
                return;
            }

            let layer = state.blend.layer;
            let short = targets(state).terrains.iter().filter_map(|id| state.doc.map.terrain(*id)).map(|t| t.layer_slot(layer)).find(|slot| *slot != layer);
            if let Some(slot) = short.filter(|_| !matches!(state.blend.mode, BlendMode::Erase | BlendMode::Smooth | BlendMode::Sharpen)) {
                state.set_status(msg!("The terrain has no layer {layer}, painting its last layer, {slot}", layer, slot));
            }

            self.stroking = true;
            self.view = Some(response.id);
            self.last = None;
            state.doc.begin("Blend");
        }

        if self.stroking {
            if let Some((p, _)) = self.hover {
                let spacing = (state.blend.radius * 0.2).max(1.0);
                if self.last.is_none_or(|l| (l - p).length() >= spacing) {
                    let mut brush = state.blend;
                    if modifiers.shift {
                        brush.mode = match brush.mode {
                            BlendMode::Paint | BlendMode::Noise | BlendMode::Slope | BlendMode::Height => BlendMode::Erase,
                            BlendMode::Erase => BlendMode::Paint,
                            BlendMode::Sharpen => BlendMode::Smooth,
                            BlendMode::Smooth => BlendMode::Sharpen,
                        };
                    }

                    if modifiers.command {
                        brush.mode = BlendMode::Smooth;
                    }

                    self.dabs = self.dabs.wrapping_add(1);
                    brush.seed = state.blend.seed.wrapping_add(if brush.falloff == blend::Falloff::Spray { self.dabs } else { 0 });
                    let symmetry = state.symmetry;
                    dab(state, p, &brush, &symmetry);
                    self.last = Some(p);
                }
            }

            if !ui.input(|i| i.pointer.primary_down()) {
                self.stroking = false;
                self.view = None;
                state.doc.commit();
            }
        }
    }

    pub fn lines(&self, state: &EditorState, out: &mut Vec<LineVertex>) {
        let Some((p, n)) = self.hover else { return };
        let r = state.blend.radius;
        let basis = Plane::from_point_normal(p, n).basis();
        let color = RING_COLOR;
        let ring = |scale: f64| -> Vec<DVec3> {
            (0..=48)
                .map(|i| {
                    let a = std::f64::consts::TAU * i as f64 / 48.0;
                    p + (basis.0 * a.cos() + basis.1 * a.sin()) * r * scale + n * 0.5
                })
                .collect()
        };
        for w in ring(1.0).windows(2) {
            out.push(LineVertex { pos: v3(w[0]), color });
            out.push(LineVertex { pos: v3(w[1]), color });
        }

        let inner = [color[0], color[1], color[2], 0.4];
        for w in ring(state.blend.strength.clamp(0.05, 1.0)).windows(2) {
            out.push(LineVertex { pos: v3(w[0]), color: inner });
            out.push(LineVertex { pos: v3(w[1]), color: inner });
        }
    }

    pub fn paint_overlay(&self, ui: &Ui, rect: Rect, state: &EditorState) {
        let b = &state.blend;
        let text = trf!(
            "Blend {mode} layer {layer} ({falloff} falloff): drag paints, Shift inverts, Ctrl smooths",
            mode = tr(b.mode.label()),
            layer = b.layer,
            falloff = tr(crate::app::falloff_name(b.falloff))
        );
        corner_hint(&ui.painter_at(rect), rect, text, Color32::from_rgb(240, 170, 255));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use td_core::Aabb;
    use td_doc::NodeKind;
    use td_geom::{Brush, Terrain};

    /// A displacement brush beside a terrain, both editable.
    fn map_with_both() -> (EditorState, NodeId, NodeId) {
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let mut brush = Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::new(128.0, 16.0, 128.0)), "dev/grey").unwrap();
        let top = brush.faces.iter().position(|f| f.plane.normal.y > 0.9).unwrap();
        brush.faces[top].data.disp = Some(td_geom::displacement::Displacement::new(2));
        let terrain = Terrain::new(DVec3::new(128.0, 0.0, 0.0), [5, 5], 32.0, "dev/grey");
        let (disp, ter) = state.doc.edit("setup", |m, _| (m.insert(layer, NodeKind::Brush(brush)), m.insert(layer, NodeKind::Terrain(terrain))));
        (state, disp, ter)
    }

    #[test]
    fn a_selection_limits_the_stroke_to_what_is_selected() {
        let (mut state, disp, ter) = map_with_both();
        let all = targets(&state);
        assert_eq!(all.terrains, vec![ter], "nothing selected reaches everything");
        assert_eq!(all.displacements.len(), 1);

        state.doc.select(|_, s| s.select_node(disp));
        let t = targets(&state);
        assert!(t.terrains.is_empty(), "a selected displacement leaves the terrain alone");
        assert_eq!(t.displacements, vec![(disp, all.displacements[0].1)]);

        state.doc.select(|_, s| {
            s.clear();
            s.select_node(ter);
        });
        let t = targets(&state);
        assert_eq!(t.terrains, vec![ter]);
        assert!(t.displacements.is_empty(), "a selected terrain leaves the displacement alone");

        state.doc.select(|_, s| s.clear());
        state.doc.map.get_mut(ter).unwrap().locked = true;
        assert!(targets(&state).terrains.is_empty(), "a locked terrain is never painted");
    }

    use td_doc::symmetry::{SymmetryAxes, SymmetryCenter};

    /// A 160 by 160 displacement floor and an off origin 9 by 7 vertex terrain with a layer to paint, at the middles
    /// (-10.5, 105.25) and (328.25, 35.5).
    fn symmetric_map() -> (EditorState, NodeId, usize, NodeId) {
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let mut brush = Brush::from_aabb(&Aabb::new(DVec3::new(-90.5, 0.0, 25.25), DVec3::new(69.5, 16.0, 185.25)), "dev/grey").unwrap();
        let top = brush.faces.iter().position(|f| f.plane.normal.y > 0.9).unwrap();
        brush.faces[top].data.disp = Some(td_geom::displacement::Displacement::new(4));
        let mut terrain = Terrain::new(DVec3::new(200.25, 8.0, -60.5), [9, 7], 32.0, "grass");
        terrain.layers.push(td_geom::TerrainLayer::new("rock", 128.0));
        let (id, ter) = state.doc.edit("setup", |m, _| (m.insert(layer, NodeKind::Brush(brush)), m.insert(layer, NodeKind::Terrain(terrain))));
        (state, id, top, ter)
    }

    fn paint(symmetry: Symmetry, spots: &[DVec3]) -> (EditorState, NodeId, usize, NodeId) {
        let (mut state, id, top, ter) = symmetric_map();
        let brush = blend::BlendBrush { radius: 30.0, strength: 0.5, layer: 1, ..Default::default() };
        state.doc.begin("Blend");
        for spot in spots {
            dab(&mut state, *spot, &brush, &symmetry);
        }

        state.doc.commit();
        (state, id, top, ter)
    }

    fn rock(state: &EditorState, ter: NodeId) -> Vec<u8> {
        let t = state.doc.map.terrain(ter).unwrap();
        (0..t.splat.len() / 4).map(|k| t.splat[k * 4 + 1]).collect()
    }

    /// Where each displacement vertex sits and how much of the blend texture it shows.
    fn alphas(state: &EditorState, id: NodeId, top: usize) -> (Vec<DVec3>, Vec<f32>) {
        let b = state.doc.map.brush(id).unwrap();
        let base = td_geom::displacement::grid(b, top).unwrap().base;
        let a = b.faces[top].data.disp.as_ref().unwrap().alphas.clone();
        (base, if a.is_empty() { vec![0.0; 17 * 17] } else { a })
    }

    const TERRAIN_SPOT: DVec3 = DVec3::new(248.25, 8.0, -20.0);
    const FLOOR_SPOT: DVec3 = DVec3::new(-50.0, 16.0, 55.25);

    #[test]
    fn blend_symmetry_mirrors_the_terrain_and_the_displacement_about_their_own_middles() {
        let (plain, id, top, ter) = paint(Symmetry::OFF, &[TERRAIN_SPOT, FLOOR_SPOT]);
        let (_, one) = alphas(&plain, id, top);
        let one_rock = rock(&plain, ter);
        let width = |k: usize| (k % 9, k / 9);
        assert!(one_rock.iter().enumerate().any(|(k, w)| *w > 0 && width(k).0 < 4), "the terrain dab landed on its near side");
        assert!(one_rock.iter().enumerate().all(|(k, w)| *w == 0 || width(k).0 < 4), "and only there without symmetry");
        assert!(one.iter().any(|a| *a > 0.0));

        for symmetry in [sym_x(), Symmetry { axes: SymmetryAxes::Xz, ..sym_x() }] {
            let flip_z = symmetry.axes == SymmetryAxes::Xz;
            let (mut state, id, top, ter) = paint(symmetry, &[TERRAIN_SPOT, FLOOR_SPOT]);
            let r = rock(&state, ter);
            for k in 0..r.len() {
                let (i, j) = width(k);
                let image = if flip_z { (6 - j) * 9 + (8 - i) } else { j * 9 + (8 - i) };
                assert!(r[k].abs_diff(r[image]) <= 1, "{:?}: terrain vertex {i},{j} holds {}, its image {}", symmetry.axes, r[k], r[image]);
            }

            assert!(r.iter().enumerate().any(|(k, w)| *w > 0 && width(k).0 > 4), "the terrain's far side is painted");

            let (base, a) = alphas(&state, id, top);
            for k in 0..base.len() {
                let want = DVec3::new(-21.0 - base[k].x, base[k].y, if flip_z { 210.5 - base[k].z } else { base[k].z });
                let image = (0..base.len()).find(|m| (base[*m] - want).length() < 1e-6).unwrap();
                assert!((a[k] - a[image]).abs() < 1e-4, "{:?}: displacement vertex at {:?} holds {}, its image {}", symmetry.axes, base[k], a[k], a[image]);
            }

            assert!((0..base.len()).any(|k| base[k].x > -10.5 && a[k] > 0.0), "the displacement's far side is painted");

            let steps = state.doc.history.undo_labels().count();
            state.doc.undo();
            assert_eq!(state.doc.history.undo_labels().count(), steps - 1, "one step");
            assert!(rock(&state, ter).iter().all(|w| *w == 0), "one undo restores the terrain");
            assert!(alphas(&state, id, top).1.iter().all(|a| *a == 0.0), "and the displacement");
        }
    }

    fn sym_x() -> Symmetry {
        Symmetry { axes: SymmetryAxes::X, center: SymmetryCenter::Terrain, ..Symmetry::OFF }
    }

    #[test]
    fn blend_symmetry_planes_sit_on_the_world_origin_when_asked() {
        let centroid = |symmetry: Symmetry| {
            let (state, id, top, _) = paint(symmetry, &[DVec3::new(-45.0, 16.0, 105.0)]);
            let (base, a) = alphas(&state, id, top);
            let sum: f64 = a.iter().map(|v| *v as f64).sum();
            base.iter().zip(&a).map(|(p, v)| p.x * *v as f64).sum::<f64>() / sum
        };

        assert!((centroid(Symmetry::OFF) + 45.0).abs() < 3.0, "one dab sits where it was dropped");
        assert!((centroid(sym_x()) + 10.5).abs() < 3.0, "the floor's own middle");
        let world = Symmetry { center: SymmetryCenter::World, ..sym_x() };
        assert!(centroid(world).abs() < 3.0, "the world origin");
        let custom = Symmetry { center: SymmetryCenter::Custom, x: -20.0, ..sym_x() };
        assert!((centroid(custom) + 20.0).abs() < 3.0, "a typed spot");
    }
}
