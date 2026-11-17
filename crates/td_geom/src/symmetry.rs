use std::collections::{BTreeSet, HashMap};

use td_core::{AxisPlane, DVec3};

use crate::Brush;

/// Brush vertices closer than this are one point, as in the Vertex tool.
pub(crate) const SAME: f64 = 1e-6;

/// The moves of a symmetric edit across `brushes`. Every `base` point moves by `delta`, then each counterpart that is
/// not in `base` moves by the mirrored delta. A vertex's counterpart is the vertex nearest its mirror image within
/// `tol`, a dot's (an edge midpoint or face center) the nearest dot. A point that is its own nearest counterpart, as
/// on the plane, adds nothing.
pub fn symmetric_moves(brushes: &[&Brush], base: &[DVec3], delta: DVec3, plane: AxisPlane, tol: f64) -> Vec<(DVec3, DVec3)> {
    let vertices: Vec<DVec3> = brushes.iter().flat_map(|b| b.vertices.iter().copied()).collect();
    let dots: Vec<DVec3> = brushes.iter().flat_map(|b| dots(b)).collect();
    let (by_vertex, by_dot, moved) = (Buckets::new(&vertices, tol), Buckets::new(&dots, tol), Buckets::new(base, SAME));
    let mut moves: Vec<(DVec3, DVec3)> = base.iter().map(|p| (*p, delta)).collect();
    let mut taken = BTreeSet::new();
    for p in base {
        let is_vertex = by_vertex.nearest(*p).is_some_and(|i| vertices[i].distance(*p) < SAME);
        let (grid, pool) = if is_vertex { (&by_vertex, &vertices) } else { (&by_dot, &dots) };
        if let Some(c) = grid.nearest(plane.reflect(*p))
            && moved.nearest(pool[c]).is_none()
            && taken.insert((is_vertex, c))
        {
            moves.push((pool[c], plane.reflect_vector(delta)));
        }
    }

    moves
}

/// Edge midpoints and face centers, the Vertex tool's blue dots.
fn dots(b: &Brush) -> impl Iterator<Item = DVec3> {
    let mids = b.edges().into_iter().map(|(a, c)| (b.vertices[a as usize] + b.vertices[c as usize]) * 0.5);
    mids.chain((0..b.faces.len()).map(|f| b.face_center(f)))
}

/// Points sorted into cells as wide as the search distance, so a lookup only visits the 27 cells around it.
pub(crate) struct Buckets<'a> {
    points: &'a [DVec3],
    tol: f64,
    cells: HashMap<(i64, i64, i64), Vec<usize>>,
}

impl<'a> Buckets<'a> {
    pub(crate) fn new(points: &'a [DVec3], tol: f64) -> Self {
        let mut grid = Buckets { points, tol, cells: HashMap::new() };
        for (i, p) in points.iter().enumerate() {
            let key = grid.cell(*p);
            grid.cells.entry(key).or_default().push(i);
        }

        grid
    }

    fn cell(&self, p: DVec3) -> (i64, i64, i64) {
        let c = (p / self.tol.max(1e-9)).floor();
        (c.x as i64, c.y as i64, c.z as i64)
    }

    /// The index of the point nearest `q` within the search distance, the lowest one on a tie.
    pub(crate) fn nearest(&self, q: DVec3) -> Option<usize> {
        let (x, y, z) = self.cell(q);
        let mut best: Option<(f64, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &i in self.cells.get(&(x + dx, y + dy, z + dz)).into_iter().flatten() {
                        let d = self.points[i].distance(q);
                        if d <= self.tol && best.is_none_or(|(bd, bi)| d < bd || (d == bd && i < bi)) {
                            best = Some((d, i));
                        }
                    }
                }
            }
        }

        best.map(|(_, i)| i)
    }
}

#[cfg(test)]
mod tests {
    use td_core::Aabb;

    use super::*;

    const PLANE: AxisPlane = AxisPlane { axis: 0, at: 37.5 };
    const DELTA: DVec3 = DVec3::new(3.0, 2.0, -1.0);
    const BACK: DVec3 = DVec3::new(-3.0, 2.0, -1.0);
    const CORNER: DVec3 = DVec3::new(24.0, 28.0, 20.0);
    const IMAGE: DVec3 = DVec3::new(51.0, 28.0, 20.0);

    fn v(x: f64, y: f64, z: f64) -> DVec3 {
        DVec3::new(x, y, z)
    }

    /// A slanted, lopsided block left of `PLANE`, its profile (-8, 4), (30, 4), (24, 28), (-8, 60) running from z -12
    /// to 20.
    fn block() -> Brush {
        let profile = [(-8.0, 4.0), (30.0, 4.0), (24.0, 28.0), (-8.0, 60.0)];
        let points: Vec<DVec3> = profile.iter().flat_map(|(x, y)| [v(*x, *y, -12.0), v(*x, *y, 20.0)]).collect();
        Brush::from_points(&points, &[], "m").unwrap()
    }

    fn mirror(b: &Brush, plane: AxisPlane) -> Brush {
        b.transformed(&plane.matrix(), true)
    }

    #[track_caller]
    fn assert_moves(got: Vec<(DVec3, DVec3)>, want: &[(DVec3, DVec3)]) {
        let same = got.len() == want.len() && got.iter().zip(want).all(|(a, b)| a.0.distance(b.0) < 1e-9 && a.1 == b.1);
        assert!(same, "got  {got:?}\nwant {want:?}");
    }

    #[test]
    fn a_corner_pulls_its_mirrored_corner_by_the_mirrored_move() {
        let (left, right) = (block(), mirror(&block(), PLANE));
        assert_moves(symmetric_moves(&[&left, &right], &[CORNER], DELTA, PLANE, 0.5), &[(CORNER, DELTA), (IMAGE, BACK)]);
        assert_moves(symmetric_moves(&[&left, &right], &[IMAGE], DELTA, PLANE, 0.5), &[(IMAGE, DELTA), (CORNER, BACK)]);
    }

    #[test]
    fn nothing_moves_without_a_base_and_nothing_follows_without_brushes() {
        let (left, right) = (block(), mirror(&block(), PLANE));
        assert!(symmetric_moves(&[&left, &right], &[], DELTA, PLANE, 0.5).is_empty());
        assert_moves(symmetric_moves(&[], &[CORNER], DELTA, PLANE, 0.5), &[(CORNER, DELTA)]);
    }

    #[test]
    fn selected_on_plane_and_missing_counterparts_add_nothing() {
        let (left, right) = (block(), mirror(&block(), PLANE));
        assert_moves(symmetric_moves(&[&left, &right], &[CORNER, IMAGE], DELTA, PLANE, 0.5), &[(CORNER, DELTA), (IMAGE, DELTA)]);
        assert_moves(symmetric_moves(&[&left], &[CORNER], DELTA, PLANE, 0.5), &[(CORNER, DELTA)]);

        // The ridge runs from the plane to 0.2 past it, so both ends are their own nearest counterparts.
        let corners = [v(29.5, 4.0, -12.0), v(45.5, 4.0, -12.0), v(29.5, 4.0, 20.0), v(45.5, 4.0, 20.0), v(37.5, 30.0, -12.0), v(37.7, 36.0, 20.0)];
        let tent = Brush::from_points(&corners, &[], "m").unwrap();
        let base = [corners[4], corners[5], corners[2]];
        let want = [(base[0], DELTA), (base[1], DELTA), (base[2], DELTA), (corners[3], BACK)];
        assert_moves(symmetric_moves(&[&tent], &base, DELTA, PLANE, 0.5), &want);
    }

    #[test]
    fn a_counterpart_counts_up_to_the_tolerance() {
        let left = block();
        for (lift, found) in [(0.49, true), (0.5, true), (0.51, false)] {
            let right = mirror(&left, PLANE).translated(v(0.0, lift, 0.0), true);
            let mut want = vec![(CORNER, DELTA)];
            if found {
                want.push((IMAGE + v(0.0, lift, 0.0), BACK));
            }

            assert_moves(symmetric_moves(&[&left, &right], &[CORNER], DELTA, PLANE, 0.5), &want);
        }
    }

    #[test]
    fn a_dot_pulls_the_mirrored_dot_only_where_a_brush_has_one() {
        let (left, right) = (block(), mirror(&block(), PLANE));
        let (edge, roof) = (v(24.0, 28.0, 4.0), v(8.0, 44.0, 4.0));
        assert_moves(symmetric_moves(&[&left, &right], &[edge], DELTA, PLANE, 0.5), &[(edge, DELTA), (v(51.0, 28.0, 4.0), BACK)]);
        assert_moves(symmetric_moves(&[&left, &right], &[roof], DELTA, PLANE, 0.5), &[(roof, DELTA), (v(67.0, 44.0, 4.0), BACK)]);
        assert_moves(symmetric_moves(&[&left], &[edge], DELTA, PLANE, 0.5), &[(edge, DELTA)]);

        // The edge's image lies on this box, 0.4 from its corner, yet no dot is there.
        let wall = Brush::from_aabb(&Aabb::new(v(51.0, 4.0, -12.0), v(83.0, 28.4, 4.0)), "m").unwrap();
        assert_moves(symmetric_moves(&[&left, &wall], &[edge], DELTA, PLANE, 0.5), &[(edge, DELTA)]);

        // And a corner's image sits on this box's edge midpoint, which no corner pulls.
        let shelf = Brush::from_aabb(&Aabb::new(v(51.0, 20.0, 20.0), v(83.0, 36.0, 30.0)), "m").unwrap();
        assert_moves(symmetric_moves(&[&left, &shelf], &[CORNER], DELTA, PLANE, 0.5), &[(CORNER, DELTA)]);
    }

    #[test]
    fn many_corners_across_two_brushes_pull_each_counterpart_once() {
        let (left, right) = (block(), mirror(&block(), PLANE));
        let base = [CORNER, v(-8.0, 60.0, -12.0), v(45.0, 4.0, -12.0)];
        let want = [(base[0], DELTA), (base[1], DELTA), (base[2], DELTA), (IMAGE, BACK), (v(83.0, 60.0, -12.0), BACK), (v(30.0, 4.0, -12.0), BACK)];
        assert_moves(symmetric_moves(&[&left, &right], &base, DELTA, PLANE, 0.5), &want);

        // Two corners 0.2 apart share one counterpart, which moves once.
        let lifted = left.translated(v(0.0, 0.2, 0.0), true);
        let base = [CORNER, CORNER + v(0.0, 0.2, 0.0)];
        let want = [(base[0], DELTA), (base[1], DELTA), (IMAGE, BACK)];
        assert_moves(symmetric_moves(&[&left, &lifted, &right], &base, DELTA, PLANE, 0.5), &want);
    }

    #[test]
    fn y_and_z_planes_mirror_their_own_axis() {
        let left = block();
        for (plane, image, back) in [
            (AxisPlane { axis: 1, at: 70.0 }, v(24.0, 112.0, 20.0), v(3.0, -2.0, -1.0)),
            (AxisPlane { axis: 2, at: 31.5 }, v(24.0, 28.0, 43.0), v(3.0, 2.0, 1.0)),
        ] {
            let other = mirror(&left, plane);
            assert_moves(symmetric_moves(&[&left, &other], &[CORNER], DELTA, plane, 0.5), &[(CORNER, DELTA), (image, back)]);
        }
    }
}
