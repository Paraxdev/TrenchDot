//! The Inspector's Edit Regions mode: a click on a square of the Top view adds a region there, or removes the one that
//! is there.

use egui::{Pos2, Rect};
use td_core::{DVec3, NodeId};
use td_geom::terrain3d::{Grid, Loc, loc_in_range};

use super::residency::region_rect;
use crate::camera::{Camera, ViewKind};
use crate::state::EditorState;

/// The region square under a point of the Top view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Square {
    pub loc: Loc,
    pub rect: Rect,
    pub exists: bool,
}

/// The square at screen position `pos`, None outside the 32 x 32 region map or in a view that is not the Top view.
pub fn square_at(camera: &Camera, view: Rect, grid: &Grid, pos: Pos2, exists: impl Fn(Loc) -> bool) -> Option<Square> {
    if camera.kind != ViewKind::Top {
        return None;
    }

    let world = camera.screen_to_plane(view, pos);
    let loc = grid.region_of(grid.vgrid_at(world.x, world.z));
    if !loc_in_range(loc) {
        return None;
    }

    let (min, max) = region_rect(grid, loc);
    let rect = Rect::from_two_pos(camera.project(view, DVec3::new(min.x, 0.0, min.y))?, camera.project(view, DVec3::new(max.x, 0.0, max.y))?);
    Some(Square { loc, rect, exists: exists(loc) })
}

impl EditorState {
    /// The Terrain3D node the Edit Regions mode works on, while the mode is on and that node is the one selection.
    pub fn terrain3d_region_mode(&self) -> Option<NodeId> {
        let nodes = &self.doc.selection.nodes;
        let id = *nodes.iter().next().filter(|_| nodes.len() == 1)?;
        (self.terrain3d_edit_regions && self.doc.map.terrain3d(id).is_some() && self.doc.map.is_editable(id)).then_some(id)
    }

    /// The square of node `id` under `pos` in a Top view.
    pub fn terrain3d_square(&self, id: NodeId, camera: &Camera, view: Rect, pos: Pos2) -> Option<Square> {
        let (_, cache) = self.terrain3d_links().into_iter().find(|(node, _)| *node == id)?;
        let (grid, source, link) = (self.terrain3d.grid(&cache)?, self.terrain3d.source(&cache)?, self.doc.map.terrain3d(id)?);
        square_at(camera, view, &grid, pos, |loc| link.has_region(&source, loc))
    }

    /// Adds a blank region at `loc`, or removes the region there. Returns whether one was added, None when the node
    /// has no cache to size a region from or `loc` is outside the region map.
    pub fn toggle_terrain3d_region(&mut self, id: NodeId, loc: Loc) -> Option<bool> {
        let (_, cache) = self.terrain3d_links().into_iter().find(|(node, _)| *node == id)?;
        let source = self.terrain3d.source(&cache)?;
        self.doc
            .try_edit("Edit Terrain3D regions", |m, _| {
                let link = m.terrain3d_mut(id).ok_or(())?;
                if link.has_region(&source, loc) {
                    link.remove_regions(&source, &[loc]);
                    Ok(false)
                } else {
                    link.add_regions(&source, &[loc]).map(|_| true).map_err(|_| ())
                }
            })
            .ok()
    }

    /// A click at `pos` of the Top view in Edit Regions mode.
    pub fn click_terrain3d_region(&mut self, id: NodeId, camera: &Camera, view: Rect, pos: Pos2) {
        if let Some(square) = self.terrain3d_square(id, camera, view, pos) {
            self.toggle_terrain3d_region(id, square.loc);
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::Vec2;
    use td_core::DVec3;

    use super::*;
    use crate::terrain3d::brush::tests::state_over;

    fn grid() -> Grid {
        Grid { region_size: 64, spacing: 2.5, upm: 32.0 }
    }

    fn view() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(800.0, 600.0))
    }

    /// A Top view centered on a slanted spot so no square lines up with the screen origin.
    fn top() -> Camera {
        let mut camera = Camera::new(ViewKind::Top);
        camera.center = DVec3::new(-3000.0, 0.0, 6100.0);
        camera.zoom = 0.05;
        camera
    }

    #[test]
    fn a_square_is_found_from_the_pixel_under_the_pointer_on_both_sides_of_zero() {
        let exists = |l: Loc| l == [-1, 1];
        // A region is 5120 units, 256 pixels at this zoom. Screen (500, 350) is the world point (-3000, 6100).
        let hit = square_at(&top(), view(), &grid(), Pos2::new(500.0, 350.0), exists).unwrap();
        assert_eq!((hit.loc, hit.exists), ([-1, 1], true));
        assert!((hit.rect.width() - 256.0).abs() < 0.01 && (hit.rect.height() - 256.0).abs() < 0.01);
        assert!(hit.rect.contains(Pos2::new(500.0, 350.0)));

        let far = square_at(&top(), view(), &grid(), Pos2::new(hit.rect.max.x + 4.0, 350.0 + 100.0), exists).unwrap();
        assert_eq!((far.loc, far.exists), ([0, 1], false), "east of the edge is the next square");
        assert!((far.rect.min.x - hit.rect.max.x).abs() < 0.01);
    }

    #[test]
    fn nothing_is_found_outside_the_region_map_or_in_other_views() {
        let mut camera = top();
        camera.center = DVec3::new(16.0 * 5120.0 + 100.0, 0.0, 100.0);
        assert!(square_at(&camera, view(), &grid(), view().center(), |_| false).is_none(), "region 16 is past the last one");
        camera.center = DVec3::new(16.0 * 5120.0 - 100.0, 0.0, 100.0);
        assert_eq!(square_at(&camera, view(), &grid(), view().center(), |_| false).unwrap().loc, [15, 0], "region 15 is the last one");
        camera.center = DVec3::new(-16.0 * 5120.0 - 100.0, 0.0, 100.0);
        assert!(square_at(&camera, view(), &grid(), view().center(), |_| false).is_none(), "region -17 is past the first one");
        let mut front = top();
        front.kind = ViewKind::Front;
        assert!(square_at(&front, view(), &grid(), view().center(), |_| false).is_none());
    }

    #[test]
    fn a_click_adds_an_empty_square_and_removes_a_filled_one_as_undo_steps() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, _) = state_over(dir.path(), &[[0, 0], [1, 0]]);
        assert_eq!(state.terrain3d_region_mode(), None, "the mode is off");
        state.terrain3d_edit_regions = true;
        assert_eq!(state.terrain3d_region_mode(), None, "and needs the node selected");
        state.doc.select(|_, s| {
            s.nodes.insert(id);
        });
        assert_eq!(state.terrain3d_region_mode(), Some(id));

        let undo = state.doc.history.undo_labels().count();
        assert_eq!(state.toggle_terrain3d_region(id, [-2, 3]), Some(true));
        assert_eq!(state.toggle_terrain3d_region(id, [1, 0]), Some(false));
        assert_eq!(state.doc.history.undo_labels().count(), undo + 2);
        let link = state.doc.map.terrain3d(id).unwrap();
        assert!(link.edits.contains_key(&[-2, 3]) && link.removed.contains_key(&[1, 0]));
        assert_eq!(state.toggle_terrain3d_region(id, [-2, 3]), Some(false), "a region TrenchDot just added goes again");
        assert!(state.doc.map.terrain3d(id).unwrap().edits.is_empty());

        assert_eq!(state.toggle_terrain3d_region(id, [16, 0]), None, "a square outside the map changes nothing");
        assert_eq!(state.doc.history.undo_labels().count(), undo + 3);
        state.doc.undo();
        assert!(state.doc.map.terrain3d(id).unwrap().edits.contains_key(&[-2, 3]), "undo brings the added region back");
        state.terrain3d_edit_regions = false;
        assert_eq!(state.terrain3d_region_mode(), None);
    }
}
