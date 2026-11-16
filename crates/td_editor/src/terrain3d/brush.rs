//! The Sculpt tool and the MCP sculpt action on a Terrain3D node: dabs edit the regions the node holds, the next save
//! sends them to Godot.

use egui::Ui;
use td_core::{DVec3, NodeId, Ray};
use td_doc::terrain::{SculptBrush, SculptMode};
use td_doc::terrain3d::reground_scatter;
use td_geom::terrain3d_tile::PaintMode;

use crate::i18n::{tr, trf};
use crate::state::EditorState;

/// The texture a Paint dab sets, and whether it paints the base or sprays the overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Texture {
    pub id: u8,
    pub spray: bool,
}

impl Texture {
    pub fn mode(self) -> PaintMode {
        if self.spray { PaintMode::Spray } else { PaintMode::Base }
    }
}

/// The texture ids Paint can set: the Terrain3D texture assets of the first node the Sculpt tool reaches, with the
/// Paint and Spray switch.
pub fn texture_picker(ui: &mut Ui, state: &mut EditorState, nodes: &[NodeId]) {
    let links = state.terrain3d_links();
    let names: Vec<(u32, String)> = nodes
        .iter()
        .find_map(|id| links.iter().find(|(node, _)| node == id))
        .and_then(|(_, cache)| state.terrain3d.manifest(cache))
        .map(|m| {
            m.textures
                .iter()
                .map(|t| (t.id, if t.name.is_empty() { trf!("Texture {id}", id = t.id) } else { trf!("{id} {name}", id = t.id, name = &t.name) }))
                .collect()
        })
        .unwrap_or_default();
    let current = state.terrain3d_texture.id as u32;
    let shown = names.iter().find(|(id, _)| *id == current).map_or_else(|| trf!("Texture {id}", id = current), |(_, name)| name.clone());
    egui::ComboBox::from_id_salt("terrain3d_texture").selected_text(shown).show_ui(ui, |ui| {
        for (id, name) in &names {
            if ui.selectable_label(current == *id, name).clicked() {
                state.terrain3d_texture.id = (*id).min(31) as u8;
            }
        }
    });
    if names.is_empty() {
        ui.add(egui::DragValue::new(&mut state.terrain3d_texture.id).range(0..=31).prefix(format!("{} ", tr("Texture"))));
    }

    ui.selectable_value(&mut state.terrain3d_texture.spray, false, tr("Paint")).on_hover_text(tr("Paint replaces the base texture under the brush"));
    ui.selectable_value(&mut state.terrain3d_texture.spray, true, tr("Spray"))
        .on_hover_text(tr("Spray blends the texture over the base, a little with every dab"));
}

impl EditorState {
    /// The nearest point of the regions of `nodes` along `ray`, the way the Sculpt tool finds its brush center.
    pub fn terrain3d_ray(&self, nodes: &[NodeId], ray: &Ray) -> Option<(f64, DVec3, NodeId)> {
        nodes.iter().filter_map(|id| self.terrain3d_regions(*id)?.ray_cast(ray).map(|h| (h.distance, h.point, *id))).min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// One dab on every Terrain3D node of `nodes`. Height modes put the scatter that follows the node back on the new
    /// surface in the same edit. `PaintLayer` paints the texture instead of a layer. Returns the regions changed.
    pub fn sculpt_terrain3d(&mut self, nodes: &[NodeId], center: DVec3, brush: &SculptBrush, texture: Texture) -> usize {
        let links = self.terrain3d_links();
        let mut changed = 0;
        for id in nodes {
            let Some((_, cache)) = links.iter().find(|(node, _)| node == id) else { continue };
            let Some(source) = self.terrain3d.source(cache) else { continue };
            let label = if brush.mode == SculptMode::PaintLayer { "Paint" } else { "Sculpt" };
            let done = self.doc.try_edit(label, |m, _| {
                let Some(link) = m.terrain3d_mut(*id) else { return Err(()) };
                let locs = match brush.mode {
                    SculptMode::PaintLayer => link.paint_texture(&source, center, brush.radius, texture.id, texture.mode(), brush.strength as f32),
                    _ => link.sculpt(&source, center, brush),
                };
                if !matches!(brush.mode, SculptMode::PaintLayer | SculptMode::Hole | SculptMode::Unhole) {
                    reground_scatter(m, *id, &source, &locs);
                }

                if locs.is_empty() { Err(()) } else { Ok(locs.len()) }
            });
            changed += done.unwrap_or(0);
        }

        changed
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use td_core::DVec3;
    use td_doc::NodeKind;
    use td_doc::terrain::SculptMode;
    use td_doc::terrain3d::RegionSource;
    use td_doc::terrain3d::Terrain3DLink;
    use td_formats::terrain3d_cache::{self, CachedRegion, Manifest, ManifestRegion, RegionHash};
    use td_geom::terrain3d::{Control, Loc, Region3D};

    use super::*;

    const UPM: f64 = 32.0;
    const SPACING: f64 = 2.5;

    fn world(root: &Path, locs: &[Loc]) -> std::path::PathBuf {
        let cache = root.join("data").join(terrain3d_cache::DIR_NAME);
        for loc in locs {
            let mut region = Region3D::blank(*loc, 64);
            region.heights.iter_mut().enumerate().for_each(|(k, h)| *h = 1.0 + 0.01 * (k % 64) as f32);
            region.control.iter_mut().for_each(|c| *c = Control(0).with_base(3).0);
            terrain3d_cache::write_region(&cache, &CachedRegion { region, vertex_spacing: SPACING as f32, hash: RegionHash([9; 32]) }).unwrap();
        }

        let manifest = Manifest {
            format: terrain3d_cache::FORMAT.into(),
            version: terrain3d_cache::VERSION,
            terrain3d: "1.1.0".into(),
            godot: "4.7.2".into(),
            data_version: 1.1,
            region_size: 64,
            vertex_spacing: SPACING,
            material: Default::default(),
            map_transform_identity: true,
            editor_open: false,
            regions: locs.iter().map(|loc| ManifestRegion { loc: *loc, hash: RegionHash([9; 32]), height_range: [0.0, 2.0] }).collect(),
            dirty: Vec::new(),
            textures: Vec::new(),
            applied: Vec::new(),
            ocean: None,
            resources: None,
        };
        terrain3d_cache::write_manifest(&cache, &manifest).unwrap();
        cache
    }

    /// An editor state on a project with one Terrain3D node over `locs`, every region loaded.
    pub(crate) fn state_over(root: &Path, locs: &[Loc]) -> (EditorState, NodeId, std::path::PathBuf) {
        let cache = world(root, locs);
        let mut state = EditorState::new(Default::default());
        state.game.project_root = Some(root.to_path_buf());
        state.game.units_per_meter = UPM;
        state.terrain3d.upm = UPM;
        let layer = state.doc.map.default_layer();
        let id = state.doc.map.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://data")));
        state.terrain3d.refresh(&cache);
        state.terrain3d.want(&cache, locs, &[]);
        let start = Instant::now();
        while locs.iter().any(|l| state.terrain3d.region(&cache, *l).is_none()) {
            assert!(start.elapsed() < Duration::from_secs(10), "the regions did not load");
            state.terrain3d.refresh(&cache);
            std::thread::sleep(Duration::from_millis(5));
        }

        (state, id, cache)
    }

    fn raise(radius: f64) -> SculptBrush {
        SculptBrush { mode: SculptMode::Raise, radius, strength: 40.0, ..Default::default() }
    }

    const PLAIN: Texture = Texture { id: 0, spray: false };

    #[test]
    fn a_dab_on_a_region_corner_raises_both_sides_and_is_one_undo_step_per_dab() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[[0, 0], [1, 0], [0, 1], [1, 1]]);
        // A region is 64 x 2.5 m, the corner of the four sits 160 m east and south of the origin.
        let corner = DVec3::new(160.0 * UPM, 0.0, 160.0 * UPM);
        let source = state.terrain3d.source(&cache).unwrap();
        let before: Vec<_> = [[0, 0], [1, 0], [0, 1], [1, 1]].iter().map(|l| source.region(*l).unwrap().heights.clone()).collect();
        let undo = state.doc.history.undo_labels().count();
        let changed = state.sculpt_terrain3d(&[id], corner, &raise(12.0 * UPM), PLAIN);
        assert_eq!(changed, 4, "all four regions around the corner changed");
        assert_eq!(state.doc.history.undo_labels().count(), undo + 1);
        let link = state.doc.map.terrain3d(id).unwrap();
        for (i, loc) in [[0, 0], [1, 0], [0, 1], [1, 1]].iter().enumerate() {
            let edited = &link.edits[loc].region;
            assert!(edited.heights.iter().zip(&before[i]).any(|(a, b)| a > b), "region {loc:?} rose");
            assert!(edited.heights.iter().zip(&before[i]).all(|(a, b)| a >= b), "a Raise dab never lowers {loc:?}");
        }

        let at = |loc: Loc, px: usize, py: usize| link.edits[&loc].region.heights[py * 64 + px];
        assert!(at([0, 0], 63, 63) > before[0][63 * 64 + 63], "the vertex next to the corner rose");
        assert!(at([1, 1], 0, 0) > before[3][0], "and so did the one on the far side");
    }

    #[test]
    fn a_dab_off_every_region_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, _) = state_over(dir.path(), &[[0, 0]]);
        let undo = state.doc.history.undo_labels().count();
        let far = DVec3::new(-900.0 * UPM, 0.0, 900.0 * UPM);
        assert_eq!(state.sculpt_terrain3d(&[id], far, &raise(4.0 * UPM), PLAIN), 0);
        assert!(state.doc.map.terrain3d(id).unwrap().edits.is_empty());
        assert_eq!(state.sculpt_terrain3d(&[], far, &raise(4.0 * UPM), PLAIN), 0, "no nodes, nothing to do");
        assert_eq!(state.doc.history.undo_labels().count(), undo, "a dab that changes nothing leaves no undo step");
    }

    #[test]
    fn paint_sets_the_texture_in_the_base_and_spray_blends_the_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, _) = state_over(dir.path(), &[[0, 0], [1, 0]]);
        let paint = SculptBrush { mode: SculptMode::PaintLayer, radius: 6.0 * UPM, strength: 1.0, ..Default::default() };
        let center = DVec3::new(64.0 * 2.5 * UPM + 3.0 * UPM, 0.0, 40.0 * UPM);
        assert!(state.sculpt_terrain3d(&[id], center, &paint, Texture { id: 9, spray: false }) >= 1);
        let link = state.doc.map.terrain3d(id).unwrap();
        let centre = |loc: Loc| Control(link.edits[&loc].region.control[16 * 64 + 1]);
        assert_eq!((centre([1, 0]).base(), centre([1, 0]).overlay()), (9, 9));
        let heights_before = link.edits[&[1, 0]].region.heights.clone();
        assert!(heights_before.iter().all(|h| *h >= 1.0), "painting leaves the heights alone");

        let spray = SculptBrush { strength: 0.4, ..paint };
        state.sculpt_terrain3d(&[id], center, &spray, Texture { id: 12, spray: true });
        let after = Control(state.doc.map.terrain3d(id).unwrap().edits[&[1, 0]].region.control[16 * 64 + 1]);
        assert_eq!(after.overlay(), 12, "spray sets the overlay");
        assert_eq!(after.base(), 9, "and keeps the base");
        assert!(after.blend() > 0);
    }

    #[test]
    fn holes_are_cut_and_filled_again() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, _) = state_over(dir.path(), &[[0, 0]]);
        let center = DVec3::new(30.0 * UPM, 0.0, 33.0 * UPM);
        let hole = SculptBrush { mode: SculptMode::Hole, radius: 3.0 * UPM, strength: 1.0, ..Default::default() };
        assert_eq!(state.sculpt_terrain3d(&[id], center, &hole, PLAIN), 1);
        let holes = |s: &EditorState| s.doc.map.terrain3d(id).unwrap().edits[&[0, 0]].region.control.iter().filter(|c| Control(**c).hole()).count();
        assert!(holes(&state) > 0);
        state.sculpt_terrain3d(&[id], center, &SculptBrush { mode: SculptMode::Unhole, ..hole }, PLAIN);
        assert_eq!(holes(&state), 0);
    }

    fn rocks(targets: Vec<NodeId>, at: DVec3) -> NodeKind {
        let mut set = td_doc::Scatter::new("Rocks", td_doc::ScatterKind::Props, vec![td_doc::ScatterItem::new("res://rock.glb")]);
        set.targets = targets;
        set.instances = vec![td_doc::scatter::ScatterInstance { item: 0, position: at, angles: DVec3::ZERO, scale: 1.0 }];
        NodeKind::Scatter(set)
    }

    #[test]
    fn a_dab_puts_following_scatter_on_the_new_surface_in_the_same_undo_step() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[[0, 0], [1, 0]]);
        let layer = state.doc.map.default_layer();
        let rock = DVec3::new(21.3 * UPM, 5.0, 17.9 * UPM);
        let follower = state.doc.map.insert(layer, rocks(vec![id], rock));
        let other = state.doc.map.insert(layer, rocks(vec![id, follower], rock));
        let mixed = state.doc.map.insert(layer, rocks(vec![], rock));
        let y_of = |s: &EditorState, set: NodeId| s.doc.map.scatter(set).unwrap().instances[0].position.y;

        let undo = state.doc.history.undo_labels().count();
        assert_eq!(state.sculpt_terrain3d(&[id], DVec3::new(20.0 * UPM, 0.0, 18.0 * UPM), &raise(8.0 * UPM), PLAIN), 1);
        let source = state.terrain3d.source(&cache).unwrap();
        let link = state.doc.map.terrain3d(id).unwrap();
        let regions: Vec<_> = [[0, 0], [1, 0]].iter().map(|l| (*l, link.region(&source, *l).unwrap())).collect();
        let want = td_geom::terrain3d::surface_height(&source.grid(), None, |l| regions.iter().find(|(k, _)| *k == l).map(|(_, r)| r.as_ref()), 21.3, 17.9);
        assert!(want.is_finite());
        assert_eq!(y_of(&state, follower), want as f64 * UPM);
        assert_eq!((y_of(&state, other), y_of(&state, mixed)), (5.0, 5.0), "only a set that targets the node alone follows it");
        assert_eq!(state.doc.history.undo_labels().count(), undo + 1);
        state.doc.undo();
        assert_eq!(y_of(&state, follower), 5.0, "one undo puts the dab and the scatter back");
        assert!(state.doc.map.terrain3d(id).unwrap().edits.is_empty());
    }
}
