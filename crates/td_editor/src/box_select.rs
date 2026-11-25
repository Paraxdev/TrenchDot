//! Box Select: with it on, a drag off the selection draws a box on the screen in any view. It takes the objects lying
//! wholly inside, or in Face mode the brush faces whose center is inside. Without X-ray the 3D view only takes faces
//! turned to the camera and not hidden behind something, the 2D views always see through.

use egui::{Modifiers, Pos2, Rect};
use td_core::{DVec3, NodeId};
use td_doc::NodeKind;

use crate::camera::Camera;
use crate::i18n::{msg, msg_n};
use crate::picking;
use crate::state::{EditorState, SelectMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxOp {
    Replace,
    Add,
    Remove,
}

impl BoxOp {
    /// Blender's keys, Shift adds and Ctrl takes away.
    pub fn from_modifiers(m: Modifiers) -> Self {
        if m.command {
            BoxOp::Remove
        } else if m.shift {
            BoxOp::Add
        } else {
            BoxOp::Replace
        }
    }
}

fn projects_inside(cam: &Camera, view: Rect, area: Rect, p: DVec3) -> bool {
    cam.project(view, p).is_some_and(|q| area.contains(q))
}

/// Whether the convex outline `points` overlaps `area`, by the separating axis test.
fn convex_meets(points: &[Pos2], area: Rect) -> bool {
    let corners = [area.left_top(), area.right_top(), area.right_bottom(), area.left_bottom()];
    let separated = |axis: egui::Vec2| {
        let span = |ps: &mut dyn Iterator<Item = &Pos2>| ps.map(|p| p.to_vec2().dot(axis)).fold((f32::MAX, f32::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)));
        let (a, b) = (span(&mut points.iter()), span(&mut corners.iter()));
        a.1 < b.0 || b.1 < a.0
    };
    let edges = points.iter().zip(points.iter().cycle().skip(1)).map(|(p, q)| (*q - *p).rot90());
    !points.is_empty() && ![egui::Vec2::X, egui::Vec2::Y].into_iter().chain(edges).any(separated)
}

/// Whether `id` shows anywhere inside `area` on screen. A brush counts by its faces, anything else by its bounds.
fn touches(state: &EditorState, cam: &Camera, view: Rect, area: Rect, id: NodeId) -> bool {
    let map = &state.doc.map;
    let project = |ps: &[DVec3]| ps.iter().filter_map(|p| cam.project(view, *p)).collect::<Vec<_>>();
    if let Some(NodeKind::Brush(b)) = map.nodes.get(&id).map(|n| &n.kind) {
        return (0..b.faces.len()).any(|f| convex_meets(&project(&b.face_points(f)), area));
    }

    let b = map.bounds(id);
    !b.is_empty() && Rect::from_points(&project(&b.corners())).intersects(area)
}

/// The objects lying wholly inside `area` on screen, or with `touch` every one it reaches, a closed group as one.
pub fn objects(state: &EditorState, cam: &Camera, view: Rect, area: Rect, touch: bool) -> Vec<NodeId> {
    let map = &state.doc.map;
    td_doc::ops::selectable_objects(map, &state.open_groups)
        .into_iter()
        .filter(|id| map.in_cordon(*id))
        .filter(|id| {
            if touch {
                return touches(state, cam, view, area, *id);
            }

            let b = map.bounds(*id);
            !b.is_empty() && b.corners().iter().all(|c| projects_inside(cam, view, area, *c))
        })
        .collect()
}

/// Whether nothing stands between the camera and the point `at` on a surface, a face of `owner` turned to the camera
/// or a corner.
pub fn unoccluded(state: &EditorState, cam: &Camera, view: Rect, at: DVec3, owner: Option<NodeId>) -> bool {
    let Some(screen) = cam.project(view, at) else { return false };
    let ray = cam.ray(view, screen);
    let depth = (at - ray.origin).dot(ray.dir);
    picking::pick(state, &ray).is_none_or(|h| Some(h.node) == owner || h.distance >= depth - 0.5)
}

/// The brush faces whose center lies inside `area` on screen.
pub fn faces(state: &EditorState, cam: &Camera, view: Rect, area: Rect, xray: bool) -> Vec<(NodeId, usize)> {
    let map = &state.doc.map;
    let see_through = xray || cam.kind.is_2d();
    let mut out = Vec::new();
    for (id, node) in map.nodes.iter() {
        let NodeKind::Brush(b) = &node.kind else { continue };
        if !map.is_editable(*id) || !map.in_cordon(*id) {
            continue;
        }

        for f in 0..b.faces.len() {
            let c = b.face_center(f);
            if !projects_inside(cam, view, area, c) {
                continue;
            }

            let facing = b.faces[f].plane.normal.dot(c - cam.eye()) < 0.0;
            if see_through || (facing && unoccluded(state, cam, view, c, Some(*id))) {
                out.push((*id, f));
            }
        }
    }

    out
}

/// Selects what the box from `a` to `b` holds, or with `touch` every object it reaches. With nothing found the
/// selection stays as it was.
pub fn apply(state: &mut EditorState, cam: &Camera, view: Rect, a: Pos2, b: Pos2, op: BoxOp, touch: bool) -> usize {
    let area = Rect::from_two_pos(a, b);
    let xray = state.prefs.box_xray;
    if state.select_mode == SelectMode::Face {
        let found = faces(state, cam, view, area, xray);
        if found.is_empty() {
            state.set_status(msg!("No face lies inside the box"));
            return 0;
        }

        state.doc.select(|_, s| match op {
            BoxOp::Remove => {
                for f in &found {
                    s.faces.remove(f);
                }
            }
            _ => {
                if op == BoxOp::Replace {
                    s.clear();
                }

                s.nodes.clear();
                s.faces.extend(found.iter().copied());
            }
        });
        state.set_status(match op {
            BoxOp::Remove => msg_n!(found.len(), "Deselected {n} face", "Deselected {n} faces"),
            _ => msg_n!(found.len(), "Selected {n} face", "Selected {n} faces"),
        });
        return found.len();
    }

    let found = objects(state, cam, view, area, touch);
    if found.is_empty() {
        state.set_status(if touch { msg!("The box touches nothing") } else { msg!("Nothing lies wholly inside the box") });
        return 0;
    }

    if op != BoxOp::Remove {
        state.last_bounds = state.doc.map.bounds_of(found.iter().copied());
    }

    state.doc.select(|_, s| match op {
        BoxOp::Remove => {
            for id in &found {
                s.nodes.remove(id);
            }
        }
        _ => {
            if op == BoxOp::Replace {
                s.clear();
            }

            s.nodes.extend(found.iter().copied());
        }
    });
    state.set_status(match op {
        BoxOp::Remove => msg_n!(found.len(), "Deselected {n} object", "Deselected {n} objects"),
        _ => msg_n!(found.len(), "Selected {n} object", "Selected {n} objects"),
    });
    state.bind_status_to_selection();
    found.len()
}

/// How far one face may turn from the next for a double click to follow it.
const RUN_ANGLE: f64 = 45.0;

/// A double click on a face in Face mode: selects the faces running on from it across shared edges, on its brush and
/// on touching brushes, as long as each turns by at most 45 degrees. `add` keeps the faces already selected.
pub fn select_face_run(state: &mut EditorState, start: (NodeId, usize), add: bool) -> usize {
    let found = td_doc::ops::connected_faces(&state.doc.map, start, RUN_ANGLE);
    state.doc.select(|_, s| {
        if !add {
            s.clear();
        }

        s.nodes.clear();
        s.faces.extend(found.iter().copied());
    });
    state.set_status(msg_n!(found.len(), "Selected {n} face", "Selected {n} faces"));
    found.len()
}

#[cfg(test)]
mod tests {
    use td_core::Aabb;
    use td_geom::Brush;

    use super::*;
    use crate::camera::ViewKind;

    fn cuboid(state: &mut EditorState, min: [f64; 3], max: [f64; 3]) -> NodeId {
        let layer = state.doc.map.default_layer();
        let brush = Brush::from_aabb(&Aabb::new(DVec3::from(min), DVec3::from(max)), "dev/grey").unwrap();
        state.doc.edit("add", |m, _| m.insert(layer, NodeKind::Brush(brush)))
    }

    /// A camera looking down -Z at the origin from 512 units away, over an 800 by 600 view.
    fn front_camera() -> (Camera, Rect) {
        let mut cam = Camera::new(ViewKind::Perspective);
        cam.position = DVec3::new(0.0, 0.0, 512.0);
        cam.look_at(DVec3::ZERO);
        (cam, Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0)))
    }

    fn screen(cam: &Camera, view: Rect, p: [f64; 3]) -> Pos2 {
        cam.project(view, DVec3::from(p)).unwrap()
    }

    #[test]
    fn the_box_takes_whole_objects_and_leaves_ones_over_its_edge() {
        let mut state = EditorState::new(Default::default());
        let near = cuboid(&mut state, [-32.0, -32.0, -32.0], [32.0, 32.0, 32.0]);
        let wide = cuboid(&mut state, [100.0, -16.0, -16.0], [300.0, 16.0, 16.0]);
        let (cam, view) = front_camera();
        let (a, b) = (screen(&cam, view, [-80.0, 80.0, 0.0]), screen(&cam, view, [200.0, -80.0, 0.0]));

        assert_eq!(objects(&state, &cam, view, Rect::from_two_pos(a, b), false), [near]);
        assert_eq!(apply(&mut state, &cam, view, a, b, BoxOp::Replace, false), 1);
        assert!(state.doc.selection.nodes.contains(&near));

        let all = Rect::from_min_size(Pos2::ZERO, view.size());
        assert_eq!(apply(&mut state, &cam, view, all.min, all.max, BoxOp::Add, false), 2);
        assert!(state.doc.selection.nodes.contains(&wide));
        apply(&mut state, &cam, view, a, b, BoxOp::Remove, false);
        assert_eq!(state.doc.selection.nodes.iter().copied().collect::<Vec<_>>(), [wide]);
    }

    #[test]
    fn without_xray_only_faces_turned_to_the_camera_and_in_plain_sight_count() {
        let mut state = EditorState::new(Default::default());
        let front = cuboid(&mut state, [-64.0, -64.0, 0.0], [64.0, 64.0, 32.0]);
        let hidden = cuboid(&mut state, [-16.0, -16.0, -128.0], [16.0, 16.0, -96.0]);
        let (cam, view) = front_camera();
        let all = Rect::from_min_size(Pos2::ZERO, view.size());

        let seen = faces(&state, &cam, view, all, false);
        assert_eq!(seen.len(), 1, "only the front face of the near box: {seen:?}");
        assert_eq!(seen[0].0, front);
        let face = &state.doc.map.brush(front).unwrap().faces[seen[0].1];
        assert!(face.plane.normal.z > 0.9);

        let through = faces(&state, &cam, view, all, true);
        assert_eq!(through.iter().filter(|(id, _)| *id == hidden).count(), 6, "X-ray takes every face of the hidden box");

        state.select_mode = SelectMode::Face;
        state.prefs.box_xray = false;
        assert_eq!(apply(&mut state, &cam, view, all.min, all.max, BoxOp::Replace, false), 1);
        assert!(state.doc.selection.nodes.is_empty());
        assert_eq!(state.doc.selection.faces.len(), 1);
    }

    #[test]
    fn a_touching_box_takes_what_it_reaches_by_the_brush_outline() {
        let mut state = EditorState::new(Default::default());
        let points = [[0.0, 0.0, 0.0], [64.0, 0.0, 0.0], [0.0, 0.0, 64.0], [0.0, 32.0, 0.0], [64.0, 32.0, 0.0], [0.0, 32.0, 64.0]];
        let wedge = Brush::from_points(&points.map(DVec3::from), &[], "dev/grey").unwrap();
        let layer = state.doc.map.default_layer();
        let wedge = state.doc.edit("add", |m, _| m.insert(layer, NodeKind::Brush(wedge)));
        let wide = cuboid(&mut state, [100.0, 0.0, 0.0], [300.0, 32.0, 32.0]);
        let mut cam = Camera::new(ViewKind::Top);
        cam.center = DVec3::ZERO;
        cam.zoom = 1.0;
        let view = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let area = |a: [f64; 2], b: [f64; 2]| Rect::from_two_pos(screen(&cam, view, [a[0], 0.0, a[1]]), screen(&cam, view, [b[0], 0.0, b[1]]));

        assert!(objects(&state, &cam, view, area([48.0, 48.0], [60.0, 60.0]), true).is_empty(), "past the slope, inside the bounds only");
        assert_eq!(objects(&state, &cam, view, area([20.0, 20.0], [40.0, 40.0]), true), [wedge], "over the slope");
        assert_eq!(objects(&state, &cam, view, area([90.0, 8.0], [120.0, 24.0]), true), [wide], "over one end");
        assert!(objects(&state, &cam, view, area([90.0, 8.0], [120.0, 24.0]), false).is_empty(), "not wholly inside");

        let (a, b) = (screen(&cam, view, [20.0, 0.0, 20.0]), screen(&cam, view, [120.0, 0.0, 24.0]));
        assert_eq!(apply(&mut state, &cam, view, a, b, BoxOp::Replace, true), 2);
    }
}
