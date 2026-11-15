//! Scatter that follows a Terrain3D node: its instances are cast onto the tiles TrenchDot draws, which are not quite
//! the surface Godot samples, so they are put back on `surface_height` after a stroke or a fill.

use std::path::PathBuf;

use super::tiles::region_rect;
use crate::state::EditorState;
use td_core::{DVec3, NodeId};
use td_doc::Map;
use td_doc::terrain3d::{RegionSource, reground_scatter};

impl EditorState {
    /// The Terrain3D node a set follows and the cache of its folder: the node its targets name, and nothing else.
    pub(crate) fn terrain3d_followed(&self, targets: &[NodeId]) -> Option<(NodeId, PathBuf)> {
        let [node] = targets else { return None };
        self.terrain3d_links().into_iter().find(|(id, _)| id == node)
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
}
