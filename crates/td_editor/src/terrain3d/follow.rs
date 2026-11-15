//! Scatter that follows a Terrain3D node: its instances are cast onto the tiles TrenchDot draws, which are not quite
//! the surface Godot samples, so they are put back on `surface_height` after a stroke or a fill.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::tiles::{View, region_rect};
use crate::state::EditorState;
use td_core::{DVec2, DVec3, NodeId};
use td_doc::Map;
use td_doc::terrain3d::{RegionSource, reground_scatter};
use td_geom::terrain3d::Loc;

/// A 2D view of the ground from `min` to `max` (x, z) that wants every region it covers whole.
pub(crate) fn ground(min: DVec2, max: DVec2) -> View {
    View::Flat { min, max, zoom: f64::INFINITY }
}

impl EditorState {
    /// Loads and builds the Terrain3D tiles `views` need now. The app does it a little each frame, but a tool call that
    /// casts onto the tiles right after a link or an edit, as a script does, has no frame in between.
    pub(crate) fn build_terrain3d_tiles(&mut self, views: &[View]) {
        if self.terrain3d_links().is_empty() {
            return;
        }

        let start = Instant::now();
        self.terrain3d.recheck();
        while self.update_terrain3d_tiles(views, Duration::from_secs(5)) && start.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The Terrain3D node a set follows and the cache of its folder: the node its targets name, and nothing else.
    pub(crate) fn terrain3d_followed(&self, targets: &[NodeId]) -> Option<(NodeId, PathBuf)> {
        let [node] = targets else { return None };
        self.terrain3d_links().into_iter().find(|(id, _)| id == node)
    }
}

impl EditorState {
    /// Puts the scatter that follows a node back on the regions that changed in Godot, as one undo step, once they are
    /// loaded. A pass that moves nothing leaves no step. While there are steps to redo it waits, a new step would end
    /// them.
    pub(crate) fn reground_owed(&mut self, links: &[(NodeId, PathBuf)], live: &BTreeSet<NodeId>) {
        if self.doc.in_transaction() || self.doc.history.can_redo() {
            return;
        }

        for (id, cache) in links.iter().filter(|(id, _)| live.contains(id)) {
            let (Some(link), Some(source), Some(owed)) = (self.doc.map.terrain3d(*id), self.terrain3d.source(cache), self.terrain3d_owed.get_mut(id)) else {
                continue;
            };
            let exists: HashSet<Loc> = link.locs(&source).into_iter().collect();
            let ready: Vec<Loc> = owed.iter().copied().filter(|l| !exists.contains(l) || link.region(&source, *l).is_some()).collect();
            if ready.is_empty() {
                continue;
            }

            owed.retain(|l| !ready.contains(l));
            let _ = self.doc.try_edit("Follow Terrain3D", |m, _| if reground_scatter(m, *id, &source, &ready) > 0 { Ok(()) } else { Err(()) });
        }
    }
}

/// Puts the instances of the sets that follow `link` back on its surface. `around` is a dab, a center and a radius, and
/// limits the work to the regions it reaches, None checks every region.
pub(crate) fn ground_followers(map: &mut Map, link: NodeId, source: &dyn RegionSource, around: Option<(DVec3, f64)>) -> usize {
    let Some(terrain) = map.terrain3d(link) else { return 0 };
    let mut locs = terrain.locs(source);
    if let Some((center, radius)) = around {
        let grid = source.grid();
        let reach = radius + grid.cell();
        locs.retain(|loc| {
            let (lo, hi) = region_rect(&grid, *loc);
            center.x + reach >= lo.x && center.x - reach <= hi.x && center.z + reach >= lo.y && center.z - reach <= hi.y
        });
    }

    reground_scatter(map, link, source, &locs)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Instant;

    use td_core::{DVec2, DVec3, NodeId};
    use td_doc::scatter::Rng;
    use td_doc::terrain::{SculptBrush, SculptMode};
    use td_doc::terrain3d::RegionSource;
    use td_geom::terrain3d::{Loc, surface_height};

    use super::{ground, ground_followers};
    use crate::scatter_tool;
    use crate::state::EditorState;
    use crate::terrain3d::brush::Texture;
    use crate::terrain3d::brush::tests::state_over;
    use crate::terrain3d::tiles::View;

    const UPM: f64 = 32.0;

    /// A bumpy world: a Raise dab leaves the heights off the plane the fixture starts with, so triangles and the bilinear
    /// surface disagree inside the cells.
    fn bumpy(root: &Path, locs: &[Loc]) -> (EditorState, NodeId, std::path::PathBuf) {
        let (mut state, id, cache) = state_over(root, locs);
        let dab = SculptBrush { mode: SculptMode::Raise, radius: 9.0 * UPM, strength: 40.0, ..Default::default() };
        state.sculpt_terrain3d(&[id], DVec3::new(21.3 * UPM, 0.0, 17.9 * UPM), &dab, Texture { id: 0, spray: false });
        build_tiles(&mut state);
        (state, id, cache)
    }

    fn build_tiles(state: &mut EditorState) {
        let everywhere = [View::Flat { min: DVec2::splat(-1.0e7), max: DVec2::splat(1.0e7), zoom: 1.0 }];
        let start = Instant::now();
        while state.update_terrain3d_tiles(&everywhere, std::time::Duration::from_secs(5)) {
            assert!(start.elapsed().as_secs() < 20, "the tiles were not built");
        }
    }

    fn rocks(state: &mut EditorState, target: NodeId) -> NodeId {
        let id = scatter_tool::create_set(state, "Rocks", td_doc::ScatterKind::Props, vec![td_doc::ScatterItem::new("res://rock.glb")]);
        state.doc.edit("target", |m, _| m.scatter_mut(id).unwrap().targets = vec![target]);
        state.active_scatter = Some(id);
        id
    }

    /// How far each instance of `set` is from the height Godot gives its x and z.
    fn off_surface(state: &EditorState, cache: &Path, link: NodeId, set: NodeId) -> Vec<f64> {
        let source = state.terrain3d.source(cache).unwrap();
        let (grid, edge) = (RegionSource::grid(&source), RegionSource::edge_blend(&source));
        let terrain = state.doc.map.terrain3d(link).unwrap();
        let regions: std::collections::HashMap<Loc, _> = terrain.locs(&source).into_iter().filter_map(|l| Some((l, terrain.region(&source, l)?))).collect();
        state
            .doc
            .map
            .scatter(set)
            .unwrap()
            .instances
            .iter()
            .map(|i| {
                let h = surface_height(&grid, edge, |l| regions.get(&l).map(|r| r.as_ref()), (i.position.x / UPM) as f32, (i.position.z / UPM) as f32);
                (i.position.y - h as f64 * UPM).abs()
            })
            .collect()
    }

    #[test]
    fn a_fill_on_the_node_lands_on_the_height_godot_samples() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = bumpy(dir.path(), &[[0, 0], [1, 0]]);
        let set = rocks(&mut state, id);
        state.prefs.scatter.rules.density = 0.01;
        let undo = state.doc.history.undo_labels().count();
        let placed = scatter_tool::fill(&mut state, &mut Rng::new(3)).unwrap();
        assert!(placed > 50, "the fill placed {placed}");
        assert_eq!(state.doc.history.undo_labels().count(), undo + 1, "the fill is one undo step");
        let off = off_surface(&state, &cache, id, set);
        assert_eq!(off.len(), placed);
        // The last half cell before the world's edge has no surface to sample, NaN there.
        let on: Vec<f64> = off.into_iter().filter(|d| d.is_finite()).collect();
        assert!(on.len() + 8 > placed && on.len() > 50, "{} of {placed} instances are over the surface", on.len());
        assert!(on.iter().all(|d| *d < 0.01 * UPM), "worst miss {} units", on.iter().cloned().fold(0.0, f64::max));
    }

    #[test]
    fn a_dab_on_the_node_lands_on_the_height_godot_samples() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = bumpy(dir.path(), &[[0, 0], [1, 0]]);
        let set = rocks(&mut state, id);
        state.prefs.scatter.radius = 12.0 * UPM;
        state.prefs.scatter.rules.density = 2.0;
        let center = DVec3::new(20.0 * UPM, 3.0 * UPM, 18.0 * UPM);
        let placed = scatter_tool::paint(&mut state, center, DVec3::Y, Some(id), &mut Rng::new(5));
        assert!(placed > 5, "the dab placed {placed}");
        let off = off_surface(&state, &cache, id, set);
        assert!(off.iter().all(|d| *d < 0.01 * UPM), "worst miss {} units", off.iter().cloned().fold(0.0, f64::max));
    }

    #[test]
    fn a_dab_leaves_instances_in_other_regions_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, _) = bumpy(dir.path(), &[[0, 0], [1, 0]]);
        let set = rocks(&mut state, id);
        let far = DVec3::new(120.0 * UPM + 5120.0, 999.0, 40.0 * UPM);
        state.doc.edit("seed", |m, _| {
            m.scatter_mut(set).unwrap().instances.push(td_doc::scatter::ScatterInstance { item: 0, position: far, angles: DVec3::ZERO, scale: 1.0 })
        });
        state.prefs.scatter.radius = 6.0 * UPM;
        state.prefs.scatter.rules.density = 2.0;
        let placed = scatter_tool::paint(&mut state, DVec3::new(20.0 * UPM, 3.0 * UPM, 18.0 * UPM), DVec3::Y, Some(id), &mut Rng::new(5));
        assert!(placed > 0);
        let instances = &state.doc.map.scatter(set).unwrap().instances;
        assert_eq!(instances.len(), placed + 1);
        assert_eq!(instances[0].position, far, "an instance a region away from the dab keeps its height");
    }

    #[test]
    fn a_set_with_more_than_the_node_as_target_keeps_the_heights_it_was_cast_at() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = bumpy(dir.path(), &[[0, 0], [1, 0]]);
        let set = rocks(&mut state, id);
        let layer = state.doc.map.default_layer();
        let floor = state.doc.map.insert(
            layer,
            td_doc::NodeKind::Brush(
                td_geom::Brush::from_aabb(&td_core::Aabb::new(DVec3::new(-400.0, -64.0, 0.0), DVec3::new(-100.0, -32.0, 5120.0)), "dev/grey").unwrap(),
            ),
        );
        state.doc.edit("targets", |m, _| m.scatter_mut(set).unwrap().targets = vec![id, floor]);
        state.prefs.scatter.rules.density = 0.01;
        let placed = scatter_tool::fill(&mut state, &mut Rng::new(3)).unwrap();
        assert!(placed > 50);
        let off = off_surface(&state, &cache, id, set);
        assert!(off.iter().any(|d| *d > 0.01 * UPM), "the heights stay where the tiles gave them");
    }

    #[test]
    fn the_eyedropper_takes_the_node_as_target_and_drops_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, _) = bumpy(dir.path(), &[[0, 0]]);
        let set = rocks(&mut state, id);
        state.doc.edit("clear", |m, _| m.scatter_mut(set).unwrap().targets.clear());
        assert_eq!(scatter_tool::eyedrop_target(&mut state, id), Ok(true));
        assert_eq!(state.doc.map.scatter(set).unwrap().targets, vec![id]);
        assert!(state.terrain3d_followed(&[id]).is_some());
        assert!(state.terrain3d_followed(&[id, set]).is_none(), "a second target ends the following");
        assert!(state.terrain3d_followed(&[]).is_none());
        assert!(state.terrain3d_followed(&[set]).is_none(), "only a Terrain3D node is followed");
        assert_eq!(scatter_tool::eyedrop_target(&mut state, id), Ok(false));
        assert!(state.doc.map.scatter(set).unwrap().targets.is_empty());
    }

    /// What Godot does when it saves a region: new heights in the file, a new hash in the manifest.
    fn rewrite(cache: &Path, loc: Loc, lift: f32, hash: u8) {
        use td_formats::terrain3d_cache::{self, RegionHash};
        let mut manifest = terrain3d_cache::read_manifest(cache).unwrap();
        let mut cached = terrain3d_cache::read_region(cache, loc, 64).unwrap();
        cached.region.heights.iter_mut().for_each(|h| *h += lift);
        cached.hash = RegionHash([hash; 32]);
        terrain3d_cache::write_region(cache, &cached).unwrap();
        manifest.regions.iter_mut().find(|r| r.loc == loc).unwrap().hash = cached.hash;
        terrain3d_cache::write_manifest(cache, &manifest).unwrap();
    }

    fn y_of(state: &EditorState, set: NodeId) -> f64 {
        state.doc.map.scatter(set).unwrap().instances[0].position.y
    }

    /// Runs the per frame update until `done`, the way the app does between Godot saving and the user looking.
    fn update_until(state: &mut EditorState, views: &[View], what: &str, done: impl Fn(&EditorState) -> bool) {
        let start = Instant::now();
        while !done(state) {
            assert!(start.elapsed().as_secs() < 10, "{what}");
            state.terrain3d.recheck();
            state.update_terrain3d_tiles(views, std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn rock_at(state: &mut EditorState, id: NodeId, x: f64, y: f64, z: f64) -> NodeId {
        let set = rocks(state, id);
        state.doc.edit("seed", |m, _| {
            m.scatter_mut(set).unwrap().instances.push(td_doc::scatter::ScatterInstance {
                item: 0,
                position: DVec3::new(x, y, z),
                angles: DVec3::ZERO,
                scale: 1.0,
            })
        });
        set
    }

    const LABEL: &str = "Follow Terrain3D";

    #[test]
    fn opening_a_map_does_not_move_instances_or_dirty_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, _) = state_over(dir.path(), &[[0, 0], [1, 0]]);
        let set = rock_at(&mut state, id, 21.3 * UPM, 5.0, 17.9 * UPM);
        let (revision, undo) = (state.doc.revision, state.doc.history.undo_labels().count());
        let everywhere = [View::Flat { min: DVec2::splat(-1.0e7), max: DVec2::splat(1.0e7), zoom: 1.0 }];
        build_tiles(&mut state);
        for _ in 0..20 {
            state.terrain3d.recheck();
            state.update_terrain3d_tiles(&everywhere, std::time::Duration::from_secs(5));
        }

        assert_eq!(y_of(&state, set), 5.0, "the instance is off the surface and stays where the map put it");
        assert_eq!((state.doc.revision, state.doc.history.undo_labels().count()), (revision, undo), "the map is not edited");
    }

    #[test]
    fn a_region_changed_in_godot_puts_following_scatter_back_on_it_in_one_undo_step() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[[0, 0], [1, 0]]);
        let everywhere = [View::Flat { min: DVec2::splat(-1.0e7), max: DVec2::splat(1.0e7), zoom: 1.0 }];
        build_tiles(&mut state);
        let (x, z) = (21.3 * UPM, 17.9 * UPM);
        let set = rock_at(&mut state, id, x, 0.0, z);
        let other = rock_at(&mut state, id, 9120.0, 0.0, 700.0);
        state.doc.edit("ground", |m, _| {
            ground_followers(m, id, &state.terrain3d.source(&cache).unwrap(), None);
        });
        let (before, untouched) = (y_of(&state, set), y_of(&state, other));
        let undo = state.doc.history.undo_labels().count();

        rewrite(&cache, [0, 0], 0.5, 11);
        update_until(&mut state, &everywhere, "the instance never followed the region", |s| y_of(s, set) != before);
        assert!((y_of(&state, set) - (before + 0.5 * UPM)).abs() < 1e-3, "half a meter up, now {} from {before}", y_of(&state, set));
        assert_eq!(y_of(&state, other), untouched, "an instance over the region that stayed does not move");
        assert_eq!(state.doc.history.undo_labels().count(), undo + 1);
        assert_eq!(state.doc.history.undo_labels().next(), Some(LABEL));
        for _ in 0..10 {
            state.terrain3d.recheck();
            state.update_terrain3d_tiles(&everywhere, std::time::Duration::from_secs(5));
        }

        assert_eq!(state.doc.history.undo_labels().count(), undo + 1, "one change in Godot is one step");

        // A save that changed nothing the surface shows moves nothing and leaves no step.
        rewrite(&cache, [0, 0], 0.0, 12);
        for _ in 0..10 {
            state.terrain3d.recheck();
            state.update_terrain3d_tiles(&everywhere, std::time::Duration::from_secs(5));
        }

        assert_eq!(state.doc.history.undo_labels().count(), undo + 1);
    }

    #[test]
    fn a_change_in_godot_waits_for_the_redo_steps_before_it_moves_scatter() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[[0, 0], [1, 0]]);
        let everywhere = [View::Flat { min: DVec2::splat(-1.0e7), max: DVec2::splat(1.0e7), zoom: 1.0 }];
        build_tiles(&mut state);
        let set = rock_at(&mut state, id, 5120.0 + 21.3 * UPM, 0.0, 17.9 * UPM);
        state.doc.edit("ground", |m, _| {
            ground_followers(m, id, &state.terrain3d.source(&cache).unwrap(), None);
        });
        let before = y_of(&state, set);
        state.doc.edit("Rename", |m, _| m.scatter_mut(set).unwrap().name = "Boulders".into());
        state.doc.undo();

        let first = |s: &EditorState| s.terrain3d.region(&cache, [1, 0]).map(|r| r.heights[0]);
        let old = first(&state);
        rewrite(&cache, [1, 0], 0.5, 11);
        update_until(&mut state, &everywhere, "the region never reloaded", |s| first(s).is_some_and(|h| Some(h) != old));
        for _ in 0..10 {
            state.terrain3d.recheck();
            state.update_terrain3d_tiles(&everywhere, std::time::Duration::from_secs(5));
        }

        assert_eq!(y_of(&state, set), before, "the instance waits while a step can be redone");
        assert_eq!(state.doc.history.redo_labels().collect::<Vec<_>>(), ["Rename"]);

        state.doc.redo();
        update_until(&mut state, &everywhere, "the instance never followed the region", |s| y_of(s, set) != before);
        assert!((y_of(&state, set) - (before + 0.5 * UPM)).abs() < 1e-3);
        assert_eq!(state.doc.history.undo_labels().take(2).collect::<Vec<_>>(), [LABEL, "Rename"]);
    }

    #[test]
    fn a_changed_region_nobody_looks_at_is_loaded_for_the_instances_on_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[[0, 0], [1, 0]]);
        // Only region [0, 0] is in view, [1, 0] lies 5120 units east of it.
        let west = [View::Flat { min: DVec2::new(0.0, 0.0), max: DVec2::new(2000.0, 2000.0), zoom: 0.001 }];
        let set = rock_at(&mut state, id, 5120.0 + 21.3 * UPM, 0.0, 17.9 * UPM);
        update_until(&mut state, &west, "the first region never loaded", |s| s.terrain3d.region(&cache, [0, 0]).is_some());
        let before = y_of(&state, set);
        rewrite(&cache, [1, 0], 0.5, 11);
        update_until(&mut state, &west, "the instance never followed the region", |s| y_of(s, set) != before);
        assert!(state.terrain3d.region(&cache, [1, 0]).is_some(), "the region was loaded to read the surface");
    }

    #[test]
    fn building_for_a_patch_of_ground_builds_only_the_regions_under_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, _) = state_over(dir.path(), &[[-1, 0], [0, 0], [3, -2]]);
        let built = |state: &EditorState| state.terrain3d_tiles.node(id).map(|(loc, _)| loc).collect::<Vec<Loc>>();
        state.build_terrain3d_tiles(&[ground(DVec2::new(20000.0, -700.0), DVec2::new(20100.0, -650.0))]);
        assert!(built(&state).is_empty(), "no region lies under the patch");
        state.build_terrain3d_tiles(&[ground(DVec2::new(5000.0, 700.0), DVec2::new(5110.0, 930.0))]);
        assert_eq!(built(&state), [[0, 0]]);
        state.build_terrain3d_tiles(&[
            ground(DVec2::new(-300.0, 700.0), DVec2::new(300.0, 900.0)),
            ground(DVec2::new(16000.0, -9000.0), DVec2::new(16100.0, -8000.0)),
        ]);
        assert_eq!(built(&state), [[-1, 0], [0, 0], [3, -2]]);
    }
}
