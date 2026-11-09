//! Polygon tool: click the corners of an outline in a view, close it and it is extruded into a brush, or into a few
//! when the outline is concave.

use egui::{Color32, PointerButton, Pos2, Rect, Response, Stroke, Ui, Vec2};
use td_core::{Aabb, DVec3, NodeId, Plane};
use td_geom::outline::{self, OutlineError};
use td_render::LineVertex;

use crate::camera::{Camera, ViewKind};
use crate::i18n::{Msg, en, msg, msg_n};
use crate::picking;
use crate::scene::v3;
use crate::state::EditorState;
use crate::widgets::corner_hint;

const HANDLE_RADIUS: f32 = 7.0;
const COLOR: [f32; 4] = [0.6, 1.0, 0.3, 1.0];
const FAINT: [f32; 4] = [0.6, 1.0, 0.3, 0.35];

/// Where the outline lies and how far it is extruded, fixed by the first corner.
#[derive(Clone, Copy, Debug)]
struct Frame {
    view: ViewKind,
    origin: DVec3,
    normal: DVec3,
    height: f64,
}

#[derive(Default)]
pub struct PolygonTool {
    pub points: Vec<DVec3>,
    frame: Option<Frame>,
    cursor: Option<DVec3>,
}

/// Where a brush drawn in a 2D view that looks along `axis` starts and ends, the way the box drag picks it: the
/// selection's extent if the preference is on, else the last brush drawn, else one grid cell.
pub fn draw_depth(state: &EditorState, last_brush: Option<Aabb>, axis: usize) -> (f64, f64) {
    let selected = state.doc.map.bounds_of(state.doc.selection.nodes.iter().copied());
    if state.prefs.draw_depth_from_selection && !selected.is_empty() && selected.size()[axis] > 1e-6 {
        return (selected.min[axis], selected.max[axis]);
    }

    let last = last_brush.unwrap_or(Aabb::new(DVec3::ZERO, DVec3::splat(crate::viewport::FIRST_BRUSH_DEPTH)));
    if last.is_empty() || last.size()[axis] < 1e-6 { (0.0, state.grid) } else { (last.min[axis], last.max[axis]) }
}

/// Makes the brushes of an outline as one undo step, inserts them into `parent` and selects them. The points lie on
/// one plane across `normal`, the extrusion runs `height` along it.
pub fn create(state: &mut EditorState, parent: NodeId, points: &[DVec3], normal: DVec3, height: f64, material: &str) -> Result<Vec<NodeId>, OutlineError> {
    let brushes = outline::extrude(points, normal, height, material)?;
    let bounds = brushes.iter().fold(Aabb::EMPTY, |b, brush| b.union(&brush.bounds()));
    let ids = state.doc.edit(en!("Create Polygon Brush"), |m, s| {
        s.clear();
        brushes
            .into_iter()
            .map(|b| {
                let id = m.insert(parent, td_doc::NodeKind::Brush(b));
                s.nodes.insert(id);
                id
            })
            .collect::<Vec<_>>()
    });
    state.last_bounds = bounds;
    Ok(ids)
}

fn error_msg(e: OutlineError) -> Msg {
    match e {
        OutlineError::TooFew => msg!("Place at least three corners first"),
        OutlineError::TooMany => msg!("The outline has too many corners"),
        OutlineError::Flat => msg!("The outline has no area"),
        OutlineError::SelfIntersecting => msg!("The outline crosses itself, press Backspace to remove corners"),
        OutlineError::NotPlanar => msg!("The corners do not lie on one plane"),
        OutlineError::NoHeight => msg!("The extrusion has no height"),
        OutlineError::BadBrush => msg!("That outline does not make a valid brush"),
    }
}

fn screen_dist(cam: &Camera, rect: Rect, world: DVec3, pos: Pos2) -> f32 {
    cam.project(rect, world).map(|p| (p - pos).length()).unwrap_or(f32::MAX)
}

fn line(out: &mut Vec<LineVertex>, a: DVec3, b: DVec3, color: [f32; 4]) {
    out.push(LineVertex { pos: v3(a), color });
    out.push(LineVertex { pos: v3(b), color });
}

impl PolygonTool {
    /// The snapped point under the cursor on the outline's plane, and the plane itself when this click would start the
    /// outline. None in a view that is not the one the outline was started in.
    fn point_at(&self, cam: &Camera, rect: Rect, pos: Pos2, state: &EditorState, last_brush: Option<Aabb>) -> Option<(DVec3, Frame)> {
        if self.frame.is_some_and(|f| f.view != cam.kind) {
            return None;
        }

        if cam.kind.is_2d() {
            let frame = self.frame.unwrap_or_else(|| {
                let axis = cam.kind.depth_axis();
                let (from, to) = draw_depth(state, last_brush, axis);
                let mut normal = DVec3::ZERO;
                normal[axis] = 1.0;
                Frame { view: cam.kind, origin: normal * from, normal, height: to - from }
            });
            let axis = cam.kind.depth_axis();
            let mut p = state.snap(cam.screen_to_plane(rect, pos));
            p[axis] = frame.origin[axis];
            return Some((p, frame));
        }

        let ray = cam.ray(rect, pos);
        let frame = match self.frame {
            Some(f) => f,
            None => {
                let (origin, normal) = match picking::pick(state, &ray) {
                    Some(h) => (h.point, h.normal),
                    None => (ray.intersect_plane(&Plane::new(DVec3::Y, 0.0)).filter(|t| *t > 0.0).map(|t| ray.at(t))?, DVec3::Y),
                };
                Frame { view: cam.kind, origin, normal: normal.normalize_or(DVec3::Y), height: state.grid }
            }
        };
        let t = ray.intersect_plane(&Plane::from_point_normal(frame.origin, frame.normal)).filter(|t| *t > 0.0)?;
        let snapped = state.snap(ray.at(t));
        Some((snapped - frame.normal * (snapped - frame.origin).dot(frame.normal), frame))
    }

    pub fn input(&mut self, response: &Response, cam: &Camera, rect: Rect, hover: Option<Pos2>, state: &mut EditorState, last_brush: Option<Aabb>) {
        if let Some(pos) = hover
            && let Some((p, _)) = self.point_at(cam, rect, pos, state, last_brush)
        {
            self.cursor = Some(p);
        }

        if !response.clicked_by(PointerButton::Primary) {
            return;
        }

        let Some(pos) = response.interact_pointer_pos() else { return };
        if self.points.len() >= 3 && screen_dist(cam, rect, self.points[0], pos) < HANDLE_RADIUS {
            self.close(state);
            return;
        }

        let Some((p, frame)) = self.point_at(cam, rect, pos, state, last_brush) else {
            state.set_status(match self.frame {
                Some(f) if f.view != cam.kind => msg!("Finish the outline in the view where you started it"),
                Some(_) => msg!("Click on the outline's plane"),
                None => msg!("Click a surface or the ground to start the outline"),
            });
            return;
        };
        if self.points.iter().any(|q| (*q - p).length() < 1e-6) {
            return;
        }

        self.frame = Some(frame);
        self.points.push(p);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn pop(&mut self) {
        self.points.pop();
        if self.points.is_empty() {
            self.reset();
        }
    }

    pub fn close(&mut self, state: &mut EditorState) {
        let Some(frame) = self.frame else {
            state.set_status(error_msg(OutlineError::TooFew));
            return;
        };
        let parent = state.insert_parent();
        let material = state.current_material.clone();
        match create(state, parent, &self.points, frame.normal, frame.height, &material) {
            Ok(ids) => {
                state.set_status(msg_n!(ids.len(), "Created {n} polygon brush", "Created {n} polygon brushes"));
                self.reset();
            }
            Err(e) => state.fail_status(error_msg(e)),
        }
    }

    pub fn lines(&self, out: &mut Vec<LineVertex>) {
        let Some(frame) = self.frame else { return };
        let lift = frame.normal * frame.height;
        let pts = &self.points;
        for w in pts.windows(2) {
            line(out, w[0], w[1], COLOR);
            line(out, w[0] + lift, w[1] + lift, FAINT);
        }

        for p in pts {
            line(out, *p, *p + lift, FAINT);
        }

        if let (Some(last), Some(cursor)) = (pts.last(), self.cursor) {
            line(out, *last, cursor, COLOR);
            line(out, *last + lift, cursor + lift, FAINT);
            if pts.len() >= 2 {
                line(out, cursor, pts[0], FAINT);
            }
        } else if pts.len() >= 3 {
            line(out, pts[pts.len() - 1], pts[0], FAINT);
        }
    }

    pub fn paint_overlay(&self, ui: &Ui, cam: &Camera, rect: Rect) {
        let painter = ui.painter_at(rect);
        let hover = ui.input(|i| i.pointer.hover_pos());
        for (i, p) in self.points.iter().enumerate() {
            let Some(sp) = cam.project(rect, *p) else { continue };
            let closing = i == 0 && self.points.len() >= 3 && hover.is_some_and(|h| (h - sp).length() < HANDLE_RADIUS);
            let color = if closing { Color32::from_rgb(255, 255, 120) } else { Color32::from_rgb(150, 255, 80) };
            let r = if closing { 6.0 } else { 4.0 };
            let square = Rect::from_center_size(sp, Vec2::splat(r * 2.0));
            painter.rect_filled(square, 1.0, color);
            painter.rect_stroke(square, 1.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
        }

        let text = if self.points.is_empty() {
            crate::i18n::trf!("Polygon: click to place the first corner")
        } else {
            crate::i18n::trf!("Polygon: click corners, the first one or Enter closes, Backspace undoes")
        };
        corner_hint(&painter, rect, text, Color32::from_rgb(170, 255, 110));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_outline_error_has_its_own_message() {
        use OutlineError::*;
        let texts: std::collections::HashSet<String> =
            [TooFew, TooMany, Flat, SelfIntersecting, NotPlanar, NoHeight, BadBrush].into_iter().map(|e| error_msg(e).en).collect();
        assert_eq!(texts.len(), 7, "{texts:?}");
    }
}
