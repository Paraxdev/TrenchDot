//! What the 2D views draw over a Terrain3D world: the region borders, each region's location once it is big enough on
//! screen, and the regions with an edit that conflicts with what Godot has since changed.

use std::collections::BTreeSet;

use egui::{Pos2, Rect, Vec2};
use td_core::DVec3;
use td_geom::terrain3d::{Grid, Loc};

use super::tiles::region_rect;
use crate::camera::Camera;

/// A region shows its location once its smaller side is at least this many pixels.
pub const LABEL_MIN_PX: f32 = 48.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    /// A region seen from above, with its location in the corner when there is room.
    Region { rect: Rect, conflict: bool, label: Option<(Pos2, String)> },
    /// A region border seen from the side, running the height of the view.
    Border { x: f32, conflict: bool },
    /// The footprint the Inspector's Regions draft would leave, seen from above.
    Plan { rect: Rect },
    /// An outer border of that footprint seen from the side.
    PlanBorder { x: f32 },
}

/// The marks of `locs` and of the conflicting regions in `conflicts` in a view whose screen area is `view`. Top views get
/// one box per region, front and side views only the vertical borders.
pub fn marks(camera: &Camera, view: Rect, grid: &Grid, locs: &[Loc], conflicts: &BTreeSet<Loc>) -> Vec<Mark> {
    let all: BTreeSet<Loc> = locs.iter().chain(conflicts).copied().collect();
    let (_, up, _) = camera.kind.axes();
    let from_above = up.x != 0.0 || up.z != 0.0;
    let mut out = Vec::new();
    // Neighbors share a border, which counts as a conflict when either region does.
    let mut borders: std::collections::BTreeMap<i64, (f32, bool)> = Default::default();
    for loc in all {
        let conflict = conflicts.contains(&loc);
        let (min, max) = region_rect(grid, loc);
        let (lo, hi) = (DVec3::new(min.x, 0.0, min.y), DVec3::new(max.x, 0.0, max.y));
        let (Some(a), Some(b)) = (camera.project(view, lo), camera.project(view, hi)) else { continue };
        if from_above {
            let rect = Rect::from_two_pos(a, b);
            if !rect.intersects(view) {
                continue;
            }

            let label = (rect.width().min(rect.height()) >= LABEL_MIN_PX).then(|| {
                let corner = rect.min.max(view.min) + Vec2::new(4.0, 3.0);
                (corner, format!("{}, {}", loc[0], loc[1]))
            });
            out.push(Mark::Region { rect, conflict, label });
            continue;
        }

        let (right, _, _) = camera.kind.axes();
        let edges = if right.x != 0.0 { [(lo.x, a.x), (hi.x, b.x)] } else { [(lo.z, a.x), (hi.z, b.x)] };
        for (world, x) in edges {
            if x < view.min.x || x > view.max.x {
                continue;
            }

            let key = (world / grid.cell()).round() as i64;
            let entry = borders.entry(key).or_insert((x, false));
            entry.1 |= conflict;
        }
    }

    out.extend(borders.into_values().map(|(x, conflict)| Mark::Border { x, conflict }));
    out
}

/// The footprint of the regions `min` to `max` (inclusive) a draft would leave, one box from above and its two outer
/// borders from the side.
pub fn outline(camera: &Camera, view: Rect, grid: &Grid, min: Loc, max: Loc) -> Vec<Mark> {
    let (lo, hi) = (region_rect(grid, min).0, region_rect(grid, max).1);
    let (Some(a), Some(b)) = (camera.project(view, DVec3::new(lo.x, 0.0, lo.y)), camera.project(view, DVec3::new(hi.x, 0.0, hi.y))) else {
        return Vec::new();
    };
    let (_, up, _) = camera.kind.axes();
    if up.x != 0.0 || up.z != 0.0 {
        return vec![Mark::Plan { rect: Rect::from_two_pos(a, b) }];
    }

    [a.x, b.x].into_iter().filter(|x| (view.min.x..=view.max.x).contains(x)).map(|x| Mark::PlanBorder { x }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::ViewKind;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    fn same(a: &Mark, b: &Mark) -> bool {
        let pos = |p: Pos2, q: Pos2| close(p.x, q.x) && close(p.y, q.y);
        match (a, b) {
            (Mark::Region { rect: r, conflict: c, label: l }, Mark::Region { rect: s, conflict: d, label: m }) => {
                pos(r.min, s.min)
                    && pos(r.max, s.max)
                    && c == d
                    && match (l, m) {
                        (Some((p, t)), Some((q, u))) => pos(*p, *q) && t == u,
                        (None, None) => true,
                        _ => false,
                    }
            }
            (Mark::Border { x, conflict: c }, Mark::Border { x: y, conflict: d }) => close(*x, *y) && c == d,
            (Mark::Plan { rect: r }, Mark::Plan { rect: s }) => pos(r.min, s.min) && pos(r.max, s.max),
            (Mark::PlanBorder { x }, Mark::PlanBorder { x: y }) => close(*x, *y),
            _ => false,
        }
    }

    fn assert_marks(got: &[Mark], want: &[Mark]) {
        assert!(got.len() == want.len() && got.iter().zip(want).all(|(a, b)| same(a, b)), "{got:?} is not {want:?}");
    }

    fn grid() -> Grid {
        Grid { region_size: 64, spacing: 0.25, upm: 32.0 }
    }

    fn view() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(800.0, 600.0))
    }

    fn camera(kind: ViewKind, center: DVec3, zoom: f64) -> Camera {
        let mut camera = Camera::new(kind);
        camera.center = center;
        camera.zoom = zoom;
        camera
    }

    #[test]
    fn a_top_view_boxes_each_region_with_its_location() {
        // Regions are 512 units wide, at zoom 0.25 that is 128 pixels, the view's middle is x 700, z 300.
        let camera = camera(ViewKind::Top, DVec3::new(700.0, 0.0, 300.0), 0.25);
        let got = marks(&camera, view(), &grid(), &[[1, 0], [0, 1]], &BTreeSet::new());
        let at = |x: f32, z: f32| Pos2::new(500.0 + (x - 700.0) * 0.25, 350.0 + (z - 300.0) * 0.25);
        let want = [
            Mark::Region {
                rect: Rect::from_two_pos(at(0.0, 512.0), at(512.0, 1024.0)),
                conflict: false,
                label: Some((at(0.0, 512.0) + Vec2::new(4.0, 3.0), "0, 1".into())),
            },
            Mark::Region {
                rect: Rect::from_two_pos(at(512.0, 0.0), at(1024.0, 512.0)),
                conflict: false,
                label: Some((at(512.0, 0.0) + Vec2::new(4.0, 3.0), "1, 0".into())),
            },
        ];
        assert_marks(&got, &want);
    }

    #[test]
    fn the_location_appears_exactly_at_the_label_size() {
        let at_zoom = |zoom: f64| {
            let camera = camera(ViewKind::Top, DVec3::new(256.0, 0.0, 256.0), zoom);
            match &marks(&camera, view(), &grid(), &[[0, 0]], &BTreeSet::new())[0] {
                Mark::Region { label, .. } => label.is_some(),
                other => panic!("{other:?}"),
            }
        };
        assert!(at_zoom(48.0 / 512.0 + 1e-6));
        assert!(!at_zoom(48.0 / 512.0 - 1e-4));
    }

    #[test]
    fn the_location_stays_in_view_when_the_corner_is_off_screen() {
        let camera = camera(ViewKind::Top, DVec3::new(1000.0, 0.0, 700.0), 0.5);
        let got = marks(&camera, view(), &grid(), &[[0, 0]], &BTreeSet::new());
        let Mark::Region { label: Some((pos, text)), .. } = &got[0] else { panic!("{got:?}") };
        assert_eq!(text, "0, 0");
        assert_eq!(*pos, Pos2::new(100.0 + 4.0, 50.0 + 3.0));
    }

    #[test]
    fn regions_out_of_view_get_no_mark_and_a_conflict_without_a_region_still_shows() {
        let camera = camera(ViewKind::Top, DVec3::new(256.0, 0.0, 256.0), 0.25);
        assert!(marks(&camera, view(), &grid(), &[[40, 40]], &BTreeSet::new()).is_empty());
        let removed = BTreeSet::from([[-1, 0]]);
        let got = marks(&camera, view(), &grid(), &[[0, 0]], &removed);
        let flags: Vec<(bool, bool)> = got
            .iter()
            .map(|m| match m {
                Mark::Region { conflict, label, .. } => (*conflict, label.is_some()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(flags, [(true, true), (false, true)]);
    }

    #[test]
    fn a_front_view_draws_the_shared_borders_once_and_a_conflict_wins() {
        let camera = camera(ViewKind::Front, DVec3::new(512.0, 0.0, 0.0), 0.25);
        let conflicts = BTreeSet::from([[1, 3]]);
        let got = marks(&camera, view(), &grid(), &[[0, 0], [1, 3], [1, 0]], &conflicts);
        let at = |x: f32| 500.0 + (x - 512.0) * 0.25;
        let want =
            [Mark::Border { x: at(0.0), conflict: false }, Mark::Border { x: at(512.0), conflict: true }, Mark::Border { x: at(1024.0), conflict: true }];
        assert_marks(&got, &want);
    }

    #[test]
    fn a_side_view_runs_along_z_and_leaves_out_borders_off_screen() {
        // The side view looks along x with z pointing left.
        let camera = camera(ViewKind::Side, DVec3::new(0.0, 0.0, 300.0), 0.25);
        let got = marks(&camera, view(), &grid(), &[[5, 0], [0, 1], [0, 40]], &BTreeSet::new());
        let at = |z: f32| 500.0 - (z - 300.0) * 0.25;
        let want = [0.0, 512.0, 1024.0].map(|z| Mark::Border { x: at(z), conflict: false });
        assert_marks(&got, &want);
    }

    #[test]
    fn the_outline_boxes_the_footprint_from_above_and_borders_it_from_the_side() {
        // Regions are 512 units wide, so [-1, 0] to [1, 2] spans x -512 to 1024 and z 0 to 1536.
        let top = camera(ViewKind::Top, DVec3::new(300.0, 0.0, 500.0), 0.25);
        let at = |x: f32, z: f32| Pos2::new(500.0 + (x - 300.0) * 0.25, 350.0 + (z - 500.0) * 0.25);
        let got = outline(&top, view(), &grid(), [-1, 0], [1, 2]);
        assert_marks(&got, &[Mark::Plan { rect: Rect::from_two_pos(at(-512.0, 0.0), at(1024.0, 1536.0)) }]);

        let front = camera(ViewKind::Front, DVec3::new(300.0, 0.0, 0.0), 0.25);
        let got = outline(&front, view(), &grid(), [-1, 0], [1, 2]);
        let x = |world: f32| 500.0 + (world - 300.0) * 0.25;
        assert_marks(&got, &[Mark::PlanBorder { x: x(-512.0) }, Mark::PlanBorder { x: x(1024.0) }]);

        let side = camera(ViewKind::Side, DVec3::new(0.0, 0.0, 500.0), 0.25);
        let got = outline(&side, view(), &grid(), [-1, 0], [1, 2]);
        let z = |world: f32| 500.0 - (world - 500.0) * 0.25;
        assert_marks(&got, &[Mark::PlanBorder { x: z(0.0) }, Mark::PlanBorder { x: z(1536.0) }]);
    }

    #[test]
    fn the_outline_leaves_out_a_side_border_off_screen_and_covers_one_region() {
        let front = camera(ViewKind::Front, DVec3::new(300.0, 0.0, 0.0), 0.25);
        let got = outline(&front, view(), &grid(), [0, 0], [40, 0]);
        assert_marks(&got, &[Mark::PlanBorder { x: 500.0 + (0.0 - 300.0) * 0.25 }]);
        let top = camera(ViewKind::Top, DVec3::new(300.0, 0.0, 500.0), 0.25);
        let one = outline(&top, view(), &grid(), [1, 1], [1, 1]);
        let Mark::Plan { rect } = &one[0] else { panic!("{one:?}") };
        assert!(close(rect.width(), 128.0) && close(rect.height(), 128.0), "{rect:?}");
    }
}
