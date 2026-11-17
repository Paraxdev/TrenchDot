//! What the 2D views draw over a Terrain3D world: the region borders, each region's location once it is big enough on
//! screen, and the regions with an edit that conflicts with what Godot has since changed.

use std::collections::BTreeSet;

use egui::{Pos2, Rect, Vec2};
use td_core::{DVec2, DVec3};
use td_geom::terrain3d::{Grid, Loc};

use super::residency::region_rect;
use crate::camera::Camera;

/// A region shows its location once its smaller side is at least this many pixels.
pub const LABEL_MIN_PX: f32 = 48.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    /// A region seen from above, with its location in the corner when there is room.
    Region { rect: Rect, conflict: bool, label: Option<(Pos2, String)> },
    /// A region border seen from the side, running the height of the view.
    Border { x: f32, conflict: bool },
    /// A region the Inspector's Regions draft would add, seen from above.
    Added { rect: Rect },
    /// A region the draft would remove, seen from above.
    Removed { rect: Rect },
    /// A border the draft would add or remove, seen from the side.
    PlanBorder { x: f32 },
    /// The draft changes something outside the view on this side of it.
    OffView { edge: Edge },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
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

/// The regions a Regions draft would change: those it adds and removes, and the ones that stay.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub add: Vec<Loc>,
    pub remove: Vec<Loc>,
    pub keep: Vec<Loc>,
}

/// What a Regions draft changes in a view: from above a box for each added and removed region, from the side the
/// borders that appear or go away. Borders shared with a region that stays are left alone. Each side of the view with
/// changes beyond it gets one mark.
pub fn outline(camera: &Camera, view: Rect, grid: &Grid, draft: &Draft) -> Vec<Mark> {
    let (right, up, _) = camera.kind.axes();
    let mut out = Vec::new();
    let mut away = Vec::new();
    let project = |loc: Loc| {
        let (min, max) = region_rect(grid, loc);
        Some((camera.project(view, DVec3::new(min.x, 0.0, min.y))?, camera.project(view, DVec3::new(max.x, 0.0, max.y))?, min, max))
    };
    if up.x != 0.0 || up.z != 0.0 {
        for (locs, added) in [(&draft.add, true), (&draft.remove, false)] {
            for (a, b, ..) in locs.iter().filter_map(|l| project(*l)) {
                let rect = Rect::from_two_pos(a, b);
                match (rect.intersects(view), added) {
                    (true, true) => out.push(Mark::Added { rect }),
                    (true, false) => out.push(Mark::Removed { rect }),
                    (false, _) => away.push(rect.center()),
                }
            }
        }
    } else {
        let along_x = right.x != 0.0;
        let edges = |min: DVec2, max: DVec2, a: Pos2, b: Pos2| if along_x { [(min.x, a.x), (max.x, b.x)] } else { [(min.y, a.x), (max.y, b.x)] };
        let key = |world: f64| (world / grid.cell()).round() as i64;
        let kept: BTreeSet<i64> = draft.keep.iter().flat_map(|l| edges_of(grid, *l, along_x)).collect();
        let mut seen = BTreeSet::new();
        for (a, b, min, max) in draft.add.iter().chain(&draft.remove).filter_map(|l| project(*l)) {
            for (world, x) in edges(min, max, a, b) {
                if kept.contains(&key(world)) || !seen.insert(key(world)) {
                    continue;
                }

                if (view.min.x..=view.max.x).contains(&x) {
                    out.push(Mark::PlanBorder { x });
                } else {
                    away.push(Pos2::new(x, view.center().y));
                }
            }
        }
    }

    let edges: BTreeSet<Edge> = away.iter().map(|p| edge_toward(view, *p)).collect();
    out.extend(edges.into_iter().map(|edge| Mark::OffView { edge }));
    out
}

fn edges_of(grid: &Grid, loc: Loc, along_x: bool) -> [i64; 2] {
    let (min, max) = region_rect(grid, loc);
    let key = |world: f64| (world / grid.cell()).round() as i64;
    if along_x { [key(min.x), key(max.x)] } else { [key(min.y), key(max.y)] }
}

fn edge_toward(view: Rect, at: Pos2) -> Edge {
    let d = at - view.center();
    match (d.x.abs() / view.width() >= d.y.abs() / view.height(), d.x >= 0.0, d.y >= 0.0) {
        (true, true, _) => Edge::Right,
        (true, false, _) => Edge::Left,
        (false, _, true) => Edge::Bottom,
        (false, _, false) => Edge::Top,
    }
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
            (Mark::Added { rect: r }, Mark::Added { rect: s }) | (Mark::Removed { rect: r }, Mark::Removed { rect: s }) => {
                pos(r.min, s.min) && pos(r.max, s.max)
            }
            (Mark::PlanBorder { x }, Mark::PlanBorder { x: y }) => close(*x, *y),
            (Mark::OffView { edge: e }, Mark::OffView { edge: f }) => e == f,
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

    fn draft(add: &[Loc], remove: &[Loc], keep: &[Loc]) -> Draft {
        Draft { add: add.to_vec(), remove: remove.to_vec(), keep: keep.to_vec() }
    }

    #[test]
    fn a_draft_boxes_the_regions_it_adds_and_removes_from_above() {
        // Regions are 512 units wide, the view's middle is x 700, z 300 at 0.25 pixels per unit.
        let top = camera(ViewKind::Top, DVec3::new(700.0, 0.0, 300.0), 0.25);
        let at = |x: f32, z: f32| Pos2::new(500.0 + (x - 700.0) * 0.25, 350.0 + (z - 300.0) * 0.25);
        let got = outline(&top, view(), &grid(), &draft(&[[2, 0], [2, 1]], &[[0, 0]], &[[1, 0]]));
        let want = [
            Mark::Added { rect: Rect::from_two_pos(at(1024.0, 0.0), at(1536.0, 512.0)) },
            Mark::Added { rect: Rect::from_two_pos(at(1024.0, 512.0), at(1536.0, 1024.0)) },
            Mark::Removed { rect: Rect::from_two_pos(at(0.0, 0.0), at(512.0, 512.0)) },
        ];
        assert_marks(&got, &want);
        assert!(outline(&top, view(), &grid(), &Draft::default()).is_empty());
    }

    #[test]
    fn a_draft_growing_out_of_view_says_which_side() {
        // The view is focused on region [0, 0] like Focus Selection does, then the draft adds [1, 0] to its east.
        let mut focused = Camera::new(ViewKind::Top);
        focused.focus(&td_core::Aabb::new(DVec3::ZERO, DVec3::new(512.0, 1.0, 512.0)), view());
        let beyond = |at: Loc| outline(&focused, view(), &grid(), &draft(&[at], &[], &[[0, 0]]));
        assert_marks(&beyond([2, 0]), &[Mark::OffView { edge: Edge::Right }]);
        assert_marks(&beyond([-2, 0]), &[Mark::OffView { edge: Edge::Left }]);
        assert_marks(&beyond([0, -3]), &[Mark::OffView { edge: Edge::Top }]);
        assert_marks(&outline(&focused, view(), &grid(), &draft(&[], &[[0, 3]], &[[0, 0]])), &[Mark::OffView { edge: Edge::Bottom }]);
        // The region next door reaches into the view, so it is boxed from its near edge and no hint shows.
        assert!(matches!(beyond([1, 0])[..], [Mark::Added { .. }]));

        let mut wide = focused.clone();
        wide.zoom = 0.25;
        let got = outline(&wide, view(), &grid(), &draft(&[[1, 0]], &[], &[[0, 0]]));
        let at = |x: f32, z: f32| Pos2::new(500.0 + (x - 256.0) * 0.25, 350.0 + (z - 256.0) * 0.25);
        assert_marks(&got, &[Mark::Added { rect: Rect::from_two_pos(at(512.0, 0.0), at(1024.0, 512.0)) }]);
    }

    #[test]
    fn a_region_counts_as_in_view_up_to_its_exact_edge() {
        // Region [1, 0] starts at x 512, which is 900 pixels with the view centered on x -1088, the view's right edge.
        let touching = outline(&camera(ViewKind::Top, DVec3::new(-1088.0, 0.0, 256.0), 0.25), view(), &grid(), &draft(&[[1, 0]], &[], &[]));
        assert!(matches!(touching[..], [Mark::Added { .. }]), "{touching:?}");
        let past = outline(&camera(ViewKind::Top, DVec3::new(-1092.0, 0.0, 256.0), 0.25), view(), &grid(), &draft(&[[1, 0]], &[], &[]));
        assert_marks(&past, &[Mark::OffView { edge: Edge::Right }]);
    }

    #[test]
    fn a_region_in_view_still_points_to_the_rest() {
        let top = camera(ViewKind::Top, DVec3::new(256.0, 0.0, 256.0), 0.25);
        let got = outline(&top, view(), &grid(), &draft(&[[40, 0]], &[[0, 0]], &[[1, 0]]));
        assert!(matches!(got[..], [Mark::Removed { .. }, Mark::OffView { edge: Edge::Right }]), "{got:?}");
    }

    #[test]
    fn changes_out_of_view_on_several_sides_get_one_hint_each() {
        // The view shows x 300..1100 and z 0..600, the draft grows two regions east, one south and drops one west.
        let top = camera(ViewKind::Top, DVec3::new(700.0, 0.0, 300.0), 1.0);
        let got = outline(&top, view(), &grid(), &draft(&[[3, 0], [4, 1], [1, 5]], &[[-1, 0]], &[[0, 0], [1, 0], [2, 0]]));
        let want = [Mark::OffView { edge: Edge::Left }, Mark::OffView { edge: Edge::Right }, Mark::OffView { edge: Edge::Bottom }];
        assert_marks(&got, &want);

        let front = camera(ViewKind::Front, DVec3::new(700.0, 0.0, 0.0), 1.0);
        let got = outline(&front, view(), &grid(), &draft(&[[3, 0]], &[[-1, 0]], &[[0, 0], [1, 0], [2, 0]]));
        assert_marks(&got, &[Mark::OffView { edge: Edge::Left }, Mark::OffView { edge: Edge::Right }]);
    }

    #[test]
    fn a_side_view_borders_only_the_sides_that_move() {
        // [1, 0] joins [0, 0], so the border at x 512 stays and only the one at 1024 appears.
        let front = camera(ViewKind::Front, DVec3::new(300.0, 0.0, 0.0), 0.25);
        let x = |world: f32| 500.0 + (world - 300.0) * 0.25;
        assert_marks(&outline(&front, view(), &grid(), &draft(&[[1, 0]], &[], &[[0, 0]])), &[Mark::PlanBorder { x: x(1024.0) }]);
        assert_marks(&outline(&front, view(), &grid(), &draft(&[], &[[1, 0]], &[[0, 0]])), &[Mark::PlanBorder { x: x(1024.0) }]);
        let both = outline(&front, view(), &grid(), &draft(&[[-1, 2]], &[[1, 0]], &[[0, 0]]));
        assert_marks(&both, &[Mark::PlanBorder { x: x(-512.0) }, Mark::PlanBorder { x: x(1024.0) }]);

        let side = camera(ViewKind::Side, DVec3::new(0.0, 0.0, 500.0), 0.25);
        let z = |world: f32| 500.0 - (world - 500.0) * 0.25;
        assert_marks(&outline(&side, view(), &grid(), &draft(&[[3, 1]], &[], &[[0, 0]])), &[Mark::PlanBorder { x: z(1024.0) }]);
    }

    #[test]
    fn a_side_view_says_which_side_the_moving_border_is_on() {
        let front = camera(ViewKind::Front, DVec3::new(300.0, 0.0, 0.0), 1.0);
        assert_marks(&outline(&front, view(), &grid(), &draft(&[[1, 0]], &[], &[[0, 0]])), &[Mark::OffView { edge: Edge::Right }]);
        // The side view looks along x with z pointing left.
        let side = camera(ViewKind::Side, DVec3::new(0.0, 0.0, 300.0), 1.0);
        assert_marks(&outline(&side, view(), &grid(), &draft(&[[0, 1]], &[], &[[0, 0]])), &[Mark::OffView { edge: Edge::Left }]);
    }
}
