//! Polygon tool: click the corners of an outline in a view, close it and it is extruded into a brush, or into a few
//! when the outline is concave. In wall and trim mode the corners are a path and every segment becomes a wall brush, or
//! a trim profile swept along it, or a raised strip that joins the strips it runs into. Shift on the first corner keeps an outline and the next ones become holes in it.

use egui::{Color32, PointerButton, Pos2, Rect, Response, Stroke, Ui, Vec2};
use td_core::{Aabb, DVec2, DVec3, NodeId, Plane};
use td_geom::brush::Brush;
use td_geom::outline::{self, OutlineError, TrimProfile, WallAlign};
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

/// What the clicked corners make.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PolygonMode {
    #[default]
    Outline,
    Walls,
    Trim,
    Strip,
}

impl PolygonMode {
    pub const ALL: [PolygonMode; 4] = [PolygonMode::Outline, PolygonMode::Walls, PolygonMode::Trim, PolygonMode::Strip];

    fn is_path(self) -> bool {
        self != PolygonMode::Outline
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WallOptions {
    pub thickness: f64,
    pub height: f64,
    pub align: WallAlign,
}

impl Default for WallOptions {
    fn default() -> Self {
        Self { thickness: 16.0, height: 128.0, align: WallAlign::Outside }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TrimOptions {
    pub profile: TrimProfile,
    pub depth: f64,
    pub height: f64,
    pub align: WallAlign,
}

impl Default for TrimOptions {
    fn default() -> Self {
        Self { profile: TrimProfile::Skirting, depth: 4.0, height: 12.0, align: WallAlign::Inside }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StripOptions {
    pub width: f64,
    pub height: f64,
}

impl Default for StripOptions {
    fn default() -> Self {
        Self { width: 64.0, height: 8.0 }
    }
}

/// Name of the group strips go into, a new strip that runs into one joins it.
pub const STRIP_GROUP: &str = "Strip";

/// Corner rounding of the outline, its holes or the path. A radius of 0 keeps the corners sharp.
#[derive(Clone, Copy, Debug)]
pub struct RoundOptions {
    pub radius: f64,
    pub segments: usize,
}

impl Default for RoundOptions {
    fn default() -> Self {
        Self { radius: 0.0, segments: 4 }
    }
}

impl RoundOptions {
    pub fn apply(self, points: &[DVec3], closed: bool) -> Vec<DVec3> {
        outline::round_corners(points, closed, self.radius, self.segments)
    }
}

#[derive(Default)]
pub struct PolygonTool {
    pub points: Vec<DVec3>,
    /// Outlines closed with Shift, the first is the outline and the rest are holes in it.
    pub loops: Vec<Vec<DVec3>>,
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

/// Makes the brushes of an outline with `holes` cut through it as one undo step, inserts them into `parent` and
/// selects them. The points lie on one plane across `normal`, the extrusion runs `height` along it.
pub fn create(
    state: &mut EditorState,
    parent: NodeId,
    points: &[DVec3],
    holes: &[Vec<DVec3>],
    normal: DVec3,
    height: f64,
    material: &str,
) -> Result<Vec<NodeId>, OutlineError> {
    let brushes = outline::extrude_with_holes(points, holes, normal, height, material)?;
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

/// Makes a wall brush per segment of a path in a new group, as one undo step, and selects the group. A closed path
/// also runs a wall from the last point back to the first. Returns the group and its walls.
pub fn create_walls(
    state: &mut EditorState,
    parent: NodeId,
    points: &[DVec3],
    closed: bool,
    normal: DVec3,
    walls: WallOptions,
    material: &str,
) -> Result<(NodeId, Vec<NodeId>), OutlineError> {
    let brushes = outline::walls(points, closed, normal, walls.height, walls.thickness, walls.align, material)?;
    Ok(insert_group(state, parent, brushes, en!("Create Walls"), "Walls"))
}

/// Makes the brushes of a trim `profile` swept along a path in a new group, like [`create_walls`].
#[allow(clippy::too_many_arguments)]
pub fn create_trim(
    state: &mut EditorState,
    parent: NodeId,
    points: &[DVec3],
    closed: bool,
    normal: DVec3,
    profile: &[DVec2],
    align: WallAlign,
    material: &str,
) -> Result<(NodeId, Vec<NodeId>), OutlineError> {
    let brushes = outline::trim(points, closed, normal, profile, align, material)?;
    Ok(insert_group(state, parent, brushes, en!("Create Trim"), "Trim"))
}

/// Makes the brushes of a raised strip along a path. Where it runs into the brushes of an existing strip group it is cut
/// back to meet them flush and joins that group, else it goes into a new group. Either way the group is selected.
/// Returns the group, its new brushes and whether it joined an existing one.
pub fn create_strip(
    state: &mut EditorState,
    parent: NodeId,
    points: &[DVec3],
    closed: bool,
    normal: DVec3,
    strip: StripOptions,
    material: &str,
) -> Result<(NodeId, Vec<NodeId>, bool), OutlineError> {
    let plain = outline::walls(points, closed, normal, strip.height, strip.width, WallAlign::Center, material)?;
    let reach = plain.iter().fold(Aabb::EMPTY, |b, w| b.union(&w.bounds()));
    let map = &state.doc.map;
    let mut joined = None;
    let mut others = Vec::new();
    for (id, brush) in map.brushes() {
        let Some(group) = map.get(id).and_then(|n| n.parent) else { continue };
        let in_strip = matches!(map.get(group).map(|n| &n.kind), Some(td_doc::NodeKind::Group(g)) if g.name == STRIP_GROUP);
        if in_strip && map.is_editable(id) && brush.bounds().overlaps(&reach, 1e-6) && plain.iter().any(|w| w.intersects(brush)) {
            joined.get_or_insert(group);
            others.push(brush);
        }
    }

    let brushes = outline::strip(points, closed, normal, strip.width, strip.height, &others, material)?;
    let Some(group) = joined else {
        let (group, ids) = insert_group(state, parent, brushes, en!("Create Strip"), STRIP_GROUP);
        return Ok((group, ids, false));
    };

    let bounds = brushes.iter().fold(Aabb::EMPTY, |b, brush| b.union(&brush.bounds()));
    let ids = state.doc.edit(en!("Create Strip"), |m, s| {
        s.clear();
        s.select_node(group);
        brushes.into_iter().map(|b| m.insert(group, td_doc::NodeKind::Brush(b))).collect::<Vec<_>>()
    });
    state.last_bounds = bounds;
    Ok((group, ids, true))
}

fn insert_group(state: &mut EditorState, parent: NodeId, brushes: Vec<Brush>, label: &str, name: &str) -> (NodeId, Vec<NodeId>) {
    let bounds = brushes.iter().fold(Aabb::EMPTY, |b, brush| b.union(&brush.bounds()));
    let made = state.doc.edit(label, |m, s| {
        s.clear();
        let group = m.insert(parent, td_doc::NodeKind::Group(td_doc::Group::new(name.to_string())));
        let ids = brushes.into_iter().map(|b| m.insert(group, td_doc::NodeKind::Brush(b))).collect::<Vec<_>>();
        s.select_node(group);
        (group, ids)
    });
    state.last_bounds = bounds;
    made
}

fn path_brushes(state: &EditorState, points: &[DVec3], closed: bool, normal: DVec3, material: &str) -> Result<Vec<Brush>, OutlineError> {
    let (w, t, st) = (state.walls, state.trim, state.strip);
    match state.polygon_mode {
        PolygonMode::Trim => outline::trim(points, closed, normal, &t.profile.points(t.depth, t.height), t.align, material),
        PolygonMode::Strip => outline::walls(points, closed, normal, st.height, st.width, WallAlign::Center, material),
        _ => outline::walls(points, closed, normal, w.height, w.thickness, w.align, material),
    }
}

fn error_msg(e: OutlineError, path: bool) -> Msg {
    match e {
        OutlineError::TooFew if path => msg!("Place at least two corners first"),
        OutlineError::Flat if path => msg!("The walls have no thickness"),
        OutlineError::TooFew => msg!("Place at least three corners first"),
        OutlineError::TooMany => msg!("The outline has too many corners"),
        OutlineError::Flat => msg!("The outline has no area"),
        OutlineError::SelfIntersecting => msg!("The outline crosses itself, press Backspace to remove corners"),
        OutlineError::BadHole => msg!("A hole has to lie inside the outline, apart from the other holes"),
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
            if state.polygon_mode == PolygonMode::Outline && response.ctx.input(|i| i.modifiers.shift) {
                self.loops.push(std::mem::take(&mut self.points));
                state.set_status(msg!("Click the corners of a hole inside the outline, Enter makes the brush"));
            } else {
                self.finish(state, true);
            }

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

    pub fn is_empty(&self) -> bool {
        self.points.is_empty() && self.loops.is_empty()
    }

    /// Removes the last corner, or reopens the last closed outline when no corner of a new one is placed.
    pub fn pop(&mut self) {
        if self.points.is_empty() {
            self.points = self.loops.pop().unwrap_or_default();
        } else {
            self.points.pop();
        }

        if self.is_empty() {
            self.reset();
        }
    }

    /// The outline and its holes, the corners placed so far closing the last hole.
    fn outline_and_holes(&self) -> Result<(&[DVec3], Vec<Vec<DVec3>>), OutlineError> {
        let Some((first, rest)) = self.loops.split_first() else { return Ok((&self.points, Vec::new())) };
        let mut holes = rest.to_vec();
        match self.points.len() {
            0 => {}
            1 | 2 => return Err(OutlineError::TooFew),
            _ => holes.push(self.points.clone()),
        }

        Ok((first, holes))
    }

    /// Makes the brushes. `closed` is whether the path joins back to its first corner, which only matters for walls
    /// since an outline always closes.
    pub fn finish(&mut self, state: &mut EditorState, closed: bool) {
        let mode = state.polygon_mode;
        let Some(frame) = self.frame else {
            state.set_status(error_msg(OutlineError::TooFew, mode.is_path()));
            return;
        };
        let parent = state.insert_parent();
        let material = state.current_material.clone();
        let round = state.polygon_round;
        let mut joined = false;
        let made = match mode {
            PolygonMode::Outline => match self.outline_and_holes() {
                Ok((outer, holes)) => {
                    let holes: Vec<Vec<DVec3>> = holes.iter().map(|h| round.apply(h, true)).collect();
                    create(state, parent, &round.apply(outer, true), &holes, frame.normal, frame.height, &material)
                }
                Err(e) => Err(e),
            },
            PolygonMode::Walls => {
                create_walls(state, parent, &round.apply(&self.points, closed), closed, frame.normal, state.walls, &material).map(|(_, ids)| ids)
            }
            PolygonMode::Trim => {
                let t = state.trim;
                let profile = t.profile.points(t.depth, t.height);
                create_trim(state, parent, &round.apply(&self.points, closed), closed, frame.normal, &profile, t.align, &material).map(|(_, ids)| ids)
            }
            PolygonMode::Strip => {
                let made = create_strip(state, parent, &round.apply(&self.points, closed), closed, frame.normal, state.strip, &material);
                joined = made.as_ref().is_ok_and(|m| m.2);
                made.map(|(_, ids, _)| ids)
            }
        };
        match made {
            Ok(ids) => {
                state.set_status(match mode {
                    PolygonMode::Outline if !self.loops.is_empty() => {
                        msg_n!(ids.len(), "Created {n} polygon brush around the holes", "Created {n} polygon brushes around the holes")
                    }
                    PolygonMode::Outline => msg_n!(ids.len(), "Created {n} polygon brush", "Created {n} polygon brushes"),
                    PolygonMode::Walls => msg_n!(ids.len(), "Created {n} wall in a group", "Created {n} walls in a group"),
                    PolygonMode::Trim => msg_n!(ids.len(), "Created {n} trim brush in a group", "Created {n} trim brushes in a group"),
                    PolygonMode::Strip if joined => {
                        msg_n!(ids.len(), "Joined {n} strip brush to the strip it runs into", "Joined {n} strip brushes to the strip they run into")
                    }
                    PolygonMode::Strip => msg_n!(ids.len(), "Created {n} strip brush in a group", "Created {n} strip brushes in a group"),
                });
                self.reset();
            }
            Err(e) => state.fail_status(error_msg(e, mode.is_path())),
        }
    }

    pub fn lines(&self, out: &mut Vec<LineVertex>, state: &EditorState) {
        let Some(frame) = self.frame else { return };
        if state.polygon_mode.is_path() {
            self.path_lines(out, frame, state);
            return;
        }

        let lift = frame.normal * frame.height;
        for ring in &self.loops {
            let ring = state.polygon_round.apply(ring, true);
            for i in 0..ring.len() {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                line(out, a, b, COLOR);
                line(out, a + lift, b + lift, FAINT);
            }
        }

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

        if state.polygon_round.radius > 0.0 && pts.len() >= 3 {
            let ring = state.polygon_round.apply(pts, true);
            for i in 0..ring.len() {
                line(out, ring[i], ring[(i + 1) % ring.len()], FAINT);
            }
        }
    }

    /// The path, and the walls or trim it would make up to the cursor.
    fn path_lines(&self, out: &mut Vec<LineVertex>, frame: Frame, state: &EditorState) {
        let mut path = self.points.clone();
        path.extend(self.cursor.filter(|c| path.last().is_none_or(|l| (*l - *c).length() > 1e-6)));
        for w in path.windows(2) {
            line(out, w[0], w[1], COLOR);
        }

        let path = state.polygon_round.apply(&path, false);
        let Ok(brushes) = path_brushes(state, &path, false, frame.normal, "") else { return };
        for b in &brushes {
            for (i, j) in b.edges() {
                line(out, b.vertices[i as usize], b.vertices[j as usize], FAINT);
            }
        }
    }

    pub fn paint_overlay(&self, ui: &Ui, cam: &Camera, rect: Rect, mode: PolygonMode) {
        let holes = mode == PolygonMode::Outline && !self.loops.is_empty();
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

        let text = match (mode, self.points.is_empty()) {
            (PolygonMode::Walls, true) => crate::i18n::trf!("Walls: click to place the first corner"),
            (PolygonMode::Walls, false) => crate::i18n::trf!("Walls: click corners, Enter ends the path, the first corner closes it, Backspace undoes"),
            (PolygonMode::Trim, true) => crate::i18n::trf!("Trim: click to place the first corner"),
            (PolygonMode::Trim, false) => crate::i18n::trf!("Trim: click corners, Enter ends the path, the first corner closes it, Backspace undoes"),
            (PolygonMode::Strip, true) => crate::i18n::trf!("Strip: click to place the first corner"),
            (PolygonMode::Strip, false) => crate::i18n::trf!("Strip: click corners, Enter ends the path, the first corner closes it, Backspace undoes"),
            (PolygonMode::Outline, _) if holes => {
                crate::i18n::trf!("Polygon hole: click its corners inside the outline, Shift on its first corner starts another, Enter makes the brush")
            }
            (PolygonMode::Outline, true) => crate::i18n::trf!("Polygon: click to place the first corner"),
            (PolygonMode::Outline, false) => {
                crate::i18n::trf!("Polygon: click corners, the first one or Enter closes, Shift on the first one cuts holes next, Backspace undoes")
            }
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
            [TooFew, TooMany, Flat, SelfIntersecting, BadHole, NotPlanar, NoHeight, BadBrush].into_iter().map(|e| error_msg(e, false).en).collect();
        assert_eq!(texts.len(), 8, "{texts:?}");
        assert_ne!(error_msg(TooFew, true).en, error_msg(TooFew, false).en);
    }
}
