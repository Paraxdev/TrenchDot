//! Finds the faces that only face the void of a sealed map, the way qbsp does. A lattice flood fills the empty space
//! from the point entities, a map whose flood reaches the border of the lattice leaks and keeps every face, otherwise
//! each face that no reached lattice point can see is dropped. The Godot build (trenchdot_void_cull.gd) follows the
//! same rules, so the editor preview matches what the build draws.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

use td_core::{Aabb, DVec3, EPSILON, NodeId, Plane};
use td_doc::{Map, NodeKind};
use td_formats::{EntityKind, GameConfig};
use td_geom::Mesh;

use crate::face_cull::{CullFace, FaceCull, skipped_entity};

/// The key that turns the culling on with 1 on worldspawn, and keeps a brush entity's faces with 0.
pub const SWITCH: &str = "_cull_void_faces";
/// Map units between lattice points, doubled until the lattice fits the budget.
pub const SPACING: f64 = 8.0;
/// Most lattice points the box around the walls may hold, `MAX_POINTS` in trenchdot_void_cull.gd so both halves pick the
/// same spacing.
const BOX_POINTS: f64 = (1u64 << 24) as f64;
/// Most lattice points the flood may reach before the cull is skipped. trenchdot_void_cull.gd budgets `FLOOD_WORK` steps
/// instead, so the two give up at different sizes.
const WORK: usize = 3_000_000;
/// How far a face sample sits in front of the face.
const LIFT: f64 = 0.5;
const NONE: u32 = u32::MAX;

/// Whether a brush entity's `_cull_void_faces` value keeps its faces in the culling. A missing value does.
pub fn switch_on(value: Option<&str>) -> bool {
    !value.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
}

/// Whether the map turns the culling on, which only worldspawn can do. A missing value leaves it off.
pub fn enabled(map: &Map) -> bool {
    map.properties.get(SWITCH).is_some_and(|v| !v.trim().is_empty() && switch_on(Some(v)))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Leak {
    /// The point entity whose flood reached the border.
    pub seed: NodeId,
    pub origin: DVec3,
    /// From the seed to the border of the lattice, straight runs merged.
    pub path: Vec<DVec3>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Worldspawn does not turn it on.
    Off,
    /// No solid walls, or no point entity inside the map to flood from.
    Nothing,
    Sealed,
    Leak(Leak),
    /// The flood outgrew its budget, every face is kept.
    OverBudget,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub outcome: Outcome,
    /// Faces to drop, by node.
    pub dropped: HashMap<NodeId, BTreeSet<usize>>,
    pub candidates: usize,
    pub spacing: f64,
    pub reached: usize,
}

impl Report {
    fn only(outcome: Outcome) -> Report {
        Report { outcome, dropped: HashMap::new(), candidates: 0, spacing: SPACING, reached: 0 }
    }

    pub fn dropped_count(&self) -> usize {
        self.dropped.values().map(BTreeSet::len).sum()
    }

    pub fn leak(&self) -> Option<&Leak> {
        match &self.outcome {
            Outcome::Leak(l) => Some(l),
            _ => None,
        }
    }
}

struct BvhNode {
    bounds: Aabb,
    a: u32,
    b: u32,
    leaf: bool,
}

/// Bounding boxes in a tree, for the walls near a point or segment.
#[derive(Default)]
struct Bvh {
    nodes: Vec<BvhNode>,
    items: Vec<(Aabb, u32)>,
}

impl Bvh {
    fn new(mut items: Vec<(Aabb, u32)>) -> Bvh {
        let mut nodes = Vec::new();
        if !items.is_empty() {
            Self::split(&mut items, 0, &mut nodes);
        }

        Bvh { nodes, items }
    }

    fn split(items: &mut [(Aabb, u32)], base: usize, nodes: &mut Vec<BvhNode>) -> u32 {
        let bounds = items.iter().fold(Aabb::EMPTY, |b, (a, _)| b.union(a));
        let at = nodes.len() as u32;
        if items.len() <= 4 {
            nodes.push(BvhNode { bounds, a: base as u32, b: (base + items.len()) as u32, leaf: true });
            return at;
        }

        let size = bounds.size();
        let axis = if size.x >= size.y && size.x >= size.z {
            0
        } else if size.y >= size.z {
            1
        } else {
            2
        };
        items.sort_by(|p, q| p.0.center()[axis].total_cmp(&q.0.center()[axis]));
        nodes.push(BvhNode { bounds, a: 0, b: 0, leaf: false });
        let mid = items.len() / 2;
        let (l, r) = items.split_at_mut(mid);
        let left = Self::split(l, base, nodes);
        let right = Self::split(r, base + mid, nodes);
        nodes[at as usize] = BvhNode { bounds, a: left, b: right, leaf: false };
        at
    }

    /// Calls `f` with every item overlapping `q` until it returns true, which is then returned.
    fn any(&self, q: &Aabb, mut f: impl FnMut(u32) -> bool) -> bool {
        if self.nodes.is_empty() {
            return false;
        }

        let mut stack = vec![0u32];
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n as usize];
            if !node.bounds.intersects(q) {
                continue;
            }

            if node.leaf {
                for (b, id) in &self.items[node.a as usize..node.b as usize] {
                    if b.intersects(q) && f(*id) {
                        return true;
                    }
                }
            } else {
                stack.push(node.a);
                stack.push(node.b);
            }
        }

        false
    }
}

enum Solid<'a> {
    Brush(Vec<Plane>),
    Mesh(&'a Mesh),
}

/// What blocks the flood: the static closed solids that draw opaque, and sky, which draws nothing but seals a map.
struct Walls<'a> {
    solids: Vec<Solid<'a>>,
    solid_tree: Bvh,
    tris: Vec<[DVec3; 3]>,
    tri_tree: Bvh,
    bounds: Aabb,
}

impl<'a> Walls<'a> {
    fn collect(map: &'a Map, game: &GameConfig, opaque: &(dyn Fn(&str) -> bool + Sync)) -> Walls<'a> {
        let sky = game.tool_textures.sky.as_str();
        let solid = |material: &str| material.eq_ignore_ascii_case(sky) || (!game.is_tool_texture(material) && opaque(material));
        let mut solids = Vec::new();
        let mut boxes = Vec::new();
        let mut tris = Vec::new();
        let mut tri_boxes = Vec::new();
        let mut bounds = Aabb::EMPTY;
        for (id, node) in map.nodes.iter() {
            if skipped_entity(map, game, *id) {
                continue;
            }

            let (wall, b) = match &node.kind {
                NodeKind::Brush(b) if b.faces.iter().all(|f| f.data.disp.is_none() && solid(&f.data.material)) => {
                    (Solid::Brush(b.faces.iter().map(|f| f.plane).collect()), b.bounds())
                }
                NodeKind::Mesh(m) if m.edge_faces().values().all(|f| f.len() == 2) && m.faces.iter().all(|f| solid(&f.data.material)) => {
                    for fi in 0..m.faces.len() {
                        for t in m.triangulate_face(fi) {
                            let [a, b, c] = t.map(|i| m.vertices.get(i as usize).copied());
                            if let (Some(a), Some(b), Some(c)) = (a, b, c) {
                                tri_boxes.push((Aabb::from_points([a, b, c]).expanded(EPSILON), tris.len() as u32));
                                tris.push([a, b, c]);
                            }
                        }
                    }

                    (Solid::Mesh(m), m.bounds())
                }
                _ => continue,
            };
            bounds.include(&b);
            boxes.push((b.expanded(EPSILON), solids.len() as u32));
            solids.push(wall);
        }

        Walls { solids, solid_tree: Bvh::new(boxes), tris, tri_tree: Bvh::new(tri_boxes), bounds }
    }

    fn contains(&self, p: DVec3) -> bool {
        self.solid_tree.any(&Aabb::new(p, p), |i| match &self.solids[i as usize] {
            Solid::Brush(planes) => planes.iter().all(|pl| pl.distance(p) <= EPSILON),
            Solid::Mesh(m) => m.contains_point(p),
        })
    }

    /// Whether the segment a to b passes through a wall. Touching a surface or a corner does not count.
    fn blocks(&self, a: DVec3, b: DVec3) -> bool {
        let q = Aabb::from_points([a, b]).expanded(EPSILON);
        self.solid_tree.any(&q, |i| matches!(&self.solids[i as usize], Solid::Brush(planes) if clip_inside(planes, a, b)))
            || self.tri_tree.any(&q, |i| segment_meets_triangle(a, b, &self.tris[i as usize]))
    }
}

/// Whether more than a point of segment a to b lies inside the convex volume.
fn clip_inside(planes: &[Plane], a: DVec3, b: DVec3) -> bool {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for pl in planes {
        let da = pl.distance(a);
        let f0 = da - EPSILON;
        let d = pl.distance(b) - da;
        if d.abs() < 1e-12 {
            if f0 > 0.0 {
                return false;
            }

            continue;
        }

        let t = -f0 / d;
        if d > 0.0 {
            t1 = t1.min(t);
        } else {
            t0 = t0.max(t);
        }

        if t1 - t0 <= 1e-6 {
            return false;
        }
    }

    t1 - t0 > 1e-6
}

/// Segment against triangle, edges included so a segment through the diagonal of a quad is caught.
fn segment_meets_triangle(a: DVec3, b: DVec3, tri: &[DVec3; 3]) -> bool {
    const TOL: f64 = 1e-6;
    let dir = b - a;
    let (e1, e2) = (tri[1] - tri[0], tri[2] - tri[0]);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return false;
    }

    let inv = 1.0 / det;
    let s = a - tri[0];
    let u = s.dot(p) * inv;
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    let t = e2.dot(q) * inv;
    u >= -TOL && v >= -TOL && u + v <= 1.0 + TOL && (-TOL..=1.0 + TOL).contains(&t)
}

type Cell = [i32; 3];

struct Flood<'w, 'a> {
    walls: &'w Walls<'a>,
    s: f64,
    lo: Cell,
    hi: Cell,
    seen: HashMap<Cell, u32>,
    blocked: HashSet<Cell>,
    /// Cell and parent of each reached point, parent is NONE for a point seeded by an entity.
    nodes: Vec<(Cell, u32)>,
    seeds: HashMap<u32, (NodeId, DVec3)>,
    queue: VecDeque<u32>,
}

impl Flood<'_, '_> {
    fn pos(&self, c: Cell) -> DVec3 {
        DVec3::new(c[0] as f64, c[1] as f64, c[2] as f64) * self.s
    }

    fn border(&self, c: Cell) -> bool {
        (0..3).any(|k| c[k] <= self.lo[k] || c[k] >= self.hi[k])
    }

    fn inside(&self, c: Cell) -> bool {
        (0..3).all(|k| c[k] >= self.lo[k] && c[k] <= self.hi[k])
    }

    /// Reaches `c` from the point at `from` if it is empty and the way there is clear. Returns the new point.
    fn reach(&mut self, c: Cell, from: DVec3, parent: u32) -> Option<u32> {
        if self.seen.contains_key(&c) || self.blocked.contains(&c) {
            return None;
        }

        let p = self.pos(c);
        if self.walls.contains(p) {
            self.blocked.insert(c);
            return None;
        }

        if self.walls.blocks(from, p) {
            return None;
        }

        let id = self.nodes.len() as u32;
        self.nodes.push((c, parent));
        self.seen.insert(c, id);
        self.queue.push_back(id);
        Some(id)
    }

    fn leak_at(&self, mut at: u32) -> Leak {
        let mut points = Vec::new();
        let seed = loop {
            let (c, parent) = self.nodes[at as usize];
            points.push(self.pos(c));
            if parent == NONE {
                break self.seeds[&at];
            }

            at = parent;
        };
        points.push(seed.1);
        points.reverse();
        let mut path: Vec<DVec3> = Vec::new();
        for p in points {
            if path.last().is_some_and(|l| l.distance_squared(p) < 1e-12) {
                continue;
            }

            if path.len() >= 2 {
                let (a, b) = (path[path.len() - 2], path[path.len() - 1]);
                if (b - a).cross(p - b).length_squared() < 1e-9 {
                    *path.last_mut().unwrap() = p;
                    continue;
                }
            }

            path.push(p);
        }

        Leak { seed: seed.0, origin: seed.1, path }
    }
}

fn cell_of(p: DVec3, s: f64) -> Cell {
    [(p.x / s).floor() as i32, (p.y / s).floor() as i32, (p.z / s).floor() as i32]
}

/// Whether `face` is seen from a reached lattice point, so a player can stand where it is visible.
fn face_seen(face: &CullFace, flood: &Flood) -> bool {
    let s = flood.s;
    let n = face.normal;
    let centre = td_geom::polygon::centroid(&face.polygon);
    let mut samples = vec![centre];
    samples.extend(face.polygon.iter().copied());
    let u = if n.x.abs() < 0.9 { n.cross(DVec3::X) } else { n.cross(DVec3::Y) }.normalize_or_zero();
    let v = n.cross(u);
    let flat: Vec<(f64, f64)> = face.polygon.iter().map(|p| (p.dot(u), p.dot(v))).collect();
    let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
    for (x, y) in &flat {
        lo = (lo.0.min(*x), lo.1.min(*y));
        hi = (hi.0.max(*x), hi.1.max(*y));
    }

    let nu = (((hi.0 - lo.0) / s).ceil() as usize).max(1);
    let nv = (((hi.1 - lo.1) / s).ceil() as usize).max(1);
    let (du, dv) = ((hi.0 - lo.0) / nu as f64, (hi.1 - lo.1) / nv as f64);
    for i in 0..nu {
        for j in 0..nv {
            let (x, y) = (lo.0 + (i as f64 + 0.5) * du, lo.1 + (j as f64 + 0.5) * dv);
            if inside_polygon(&flat, x, y) {
                samples.push(centre + u * (x - centre.dot(u)) + v * (y - centre.dot(v)));
            }
        }
    }

    samples.iter().any(|p| {
        let p = *p + n * LIFT;
        let base = cell_of(p, s);
        (0..8).any(|k| {
            let c = [base[0] + (k & 1), base[1] + ((k >> 1) & 1), base[2] + ((k >> 2) & 1)];
            flood.seen.contains_key(&c) && !flood.walls.blocks(flood.pos(c), p)
        })
    })
}

fn inside_polygon(poly: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }

        j = i;
    }

    inside
}

/// The faces of `map` that only face the void, or the leak that stops the cull. `opaque` says whether a material draws
/// opaque and single sided. Stops early with `Outcome::Cancelled` once `cancel` is set.
pub fn analyze(map: &Map, game: &GameConfig, opaque: &(dyn Fn(&str) -> bool + Sync), cancel: &AtomicBool) -> Report {
    if !enabled(map) {
        return Report::only(Outcome::Off);
    }

    let walls = Walls::collect(map, game, opaque);
    let seeds: Vec<(NodeId, DVec3)> = map
        .entities()
        .filter(|(id, e)| {
            map.is_point_entity(*id) && e.classname != "worldspawn" && game.entity(&e.classname).is_none_or(|d| d.kind != EntityKind::Solid)
        })
        .map(|(id, e)| (id, e.origin))
        .collect();
    if walls.solids.is_empty() || seeds.is_empty() {
        return Report::only(Outcome::Nothing);
    }

    let mut s = SPACING;
    let (lo, hi) = loop {
        let b = walls.bounds.expanded(2.0 * s);
        let lo = cell_of(b.min, s);
        let mut hi = cell_of(b.max, s);
        for k in 0..3 {
            if (hi[k] as f64) * s < b.max[k] {
                hi[k] += 1;
            }
        }

        let count: f64 = (0..3).map(|k| (hi[k] - lo[k] + 1) as f64).product();
        if count <= BOX_POINTS {
            break (lo, hi);
        }

        s *= 2.0;
    };

    let mut flood =
        Flood { walls: &walls, s, lo, hi, seen: HashMap::new(), blocked: HashSet::new(), nodes: Vec::new(), seeds: HashMap::new(), queue: VecDeque::new() };
    let finish = |flood: &Flood, outcome: Outcome| Report { outcome, dropped: HashMap::new(), candidates: 0, spacing: s, reached: flood.nodes.len() };

    let mut any_seed = false;
    for (entity, origin) in seeds {
        let cell = cell_of(origin, s);
        if !flood.inside(cell) || !flood.inside([cell[0] + 1, cell[1] + 1, cell[2] + 1]) {
            return finish(&flood, Outcome::Leak(Leak { seed: entity, origin, path: vec![origin] }));
        }

        if walls.contains(origin) {
            continue;
        }

        for k in 0..8 {
            let c = [cell[0] + (k & 1), cell[1] + ((k >> 1) & 1), cell[2] + ((k >> 2) & 1)];
            let Some(id) = flood.reach(c, origin, NONE) else { continue };
            flood.seeds.insert(id, (entity, origin));
            any_seed = true;
            if flood.border(c) {
                return finish(&flood, Outcome::Leak(flood.leak_at(id)));
            }
        }
    }

    if !any_seed {
        return finish(&flood, Outcome::Nothing);
    }

    let mut steps = 0u32;
    while let Some(at) = flood.queue.pop_front() {
        steps += 1;
        if steps % 4096 == 0 && cancel.load(Ordering::Relaxed) {
            return finish(&flood, Outcome::Cancelled);
        }

        if flood.nodes.len() > WORK {
            return finish(&flood, Outcome::OverBudget);
        }

        let (c, _) = flood.nodes[at as usize];
        let from = flood.pos(c);
        for (axis, step) in [(0, 1), (0, -1), (1, 1), (1, -1), (2, 1), (2, -1)] {
            let mut n = c;
            n[axis] += step;
            if let Some(id) = flood.reach(n, from, at)
                && flood.border(n)
            {
                return finish(&flood, Outcome::Leak(flood.leak_at(id)));
            }
        }
    }

    let ids: Vec<NodeId> = map
        .nodes
        .iter()
        .filter(|(_, n)| matches!(n.kind, NodeKind::Brush(_) | NodeKind::Mesh(_)))
        .filter(|(id, _)| map.owning_entity(**id).and_then(|e| map.entity(e)).is_none_or(|e| switch_on(e.property(SWITCH))))
        .map(|(id, _)| *id)
        .collect();
    let threads = std::thread::available_parallelism().map(|t| t.get()).unwrap_or(1).min(ids.len().max(1));
    let chunk = ids.len().div_ceil(threads).max(1);
    let flood_ref = &flood;
    let results: Vec<(usize, Vec<(NodeId, usize)>)> = std::thread::scope(|scope| {
        ids.chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    let mut count = 0;
                    let mut dropped = Vec::new();
                    for id in part {
                        for face in FaceCull::node_faces(map, game, opaque, &|_| true, *id) {
                            count += 1;
                            if !face_seen(&face, flood_ref) {
                                dropped.push((*id, face.face));
                            }
                        }
                    }

                    (count, dropped)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect()
    });

    let mut report = finish(&flood, Outcome::Sealed);
    for (count, dropped) in results {
        report.candidates += count;
        for (id, face) in dropped {
            report.dropped.entry(id).or_default().insert(face);
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use td_geom::Brush;

    fn bx(map: &mut Map, parent: NodeId, min: [f64; 3], max: [f64; 3]) -> NodeId {
        let b = Brush::from_aabb(&Aabb::new(DVec3::from(min), DVec3::from(max)), "dev/grey").unwrap();
        map.insert(parent, NodeKind::Brush(b))
    }

    fn start(map: &mut Map, at: [f64; 3]) -> NodeId {
        let layer = map.default_layer();
        let mut e = td_doc::Entity::new("info_player_start");
        e.origin = DVec3::from(at);
        map.insert(layer, NodeKind::Entity(e))
    }

    /// Slabs around the room x, y, z in 0..128, 16 thick, meeting in seams. The ceiling is left out.
    fn room_without_ceiling(map: &mut Map) -> Vec<NodeId> {
        let layer = map.default_layer();
        vec![
            bx(map, layer, [-16.0, -16.0, -16.0], [144.0, 0.0, 144.0]),
            bx(map, layer, [-16.0, 0.0, -16.0], [0.0, 128.0, 144.0]),
            bx(map, layer, [128.0, 0.0, -16.0], [144.0, 128.0, 144.0]),
            bx(map, layer, [0.0, 0.0, -16.0], [128.0, 128.0, 0.0]),
            bx(map, layer, [0.0, 0.0, 128.0], [128.0, 128.0, 144.0]),
        ]
    }

    fn ceiling(map: &mut Map) -> NodeId {
        let layer = map.default_layer();
        bx(map, layer, [-16.0, 128.0, -16.0], [144.0, 144.0, 144.0])
    }

    /// Analyzes with the culling switched on, unless the map says otherwise.
    fn run_with(map: &Map, game: &GameConfig) -> Report {
        let mut map = map.clone();
        map.properties.entry(SWITCH.into()).or_insert("1".into());
        analyze(&map, game, &|_| true, &AtomicBool::new(false))
    }

    fn run(map: &Map) -> Report {
        run_with(map, &GameConfig::default())
    }

    fn face(map: &Map, id: NodeId, normal: DVec3) -> usize {
        map.brush(id).unwrap().faces.iter().position(|f| f.plane.normal.dot(normal) > 0.999).unwrap()
    }

    fn dropped(r: &Report, map: &Map, id: NodeId, normal: DVec3) -> bool {
        r.dropped.get(&id).is_some_and(|f| f.contains(&face(map, id, normal)))
    }

    #[test]
    fn a_sealed_room_drops_the_faces_that_look_outwards() {
        let mut map = Map::new();
        let walls = room_without_ceiling(&mut map);
        let top = ceiling(&mut map);
        start(&mut map, [64.0, 64.0, 64.0]);
        let r = run(&map);
        assert_eq!(r.outcome, Outcome::Sealed);
        let [floor, left, right, back, front] = walls[..] else { unreachable!() };
        assert!(dropped(&r, &map, floor, DVec3::NEG_Y) && dropped(&r, &map, floor, DVec3::X) && dropped(&r, &map, floor, DVec3::NEG_Z));
        assert!(!dropped(&r, &map, floor, DVec3::Y));
        assert!(dropped(&r, &map, left, DVec3::NEG_X) && !dropped(&r, &map, left, DVec3::X));
        assert!(dropped(&r, &map, right, DVec3::X) && !dropped(&r, &map, right, DVec3::NEG_X));
        assert!(dropped(&r, &map, back, DVec3::NEG_Z) && !dropped(&r, &map, back, DVec3::Z));
        assert!(dropped(&r, &map, front, DVec3::Z) && !dropped(&r, &map, front, DVec3::NEG_Z));
        assert!(dropped(&r, &map, top, DVec3::Y) && !dropped(&r, &map, top, DVec3::NEG_Y));
        assert!(r.dropped_count() > 20);
    }

    #[test]
    fn a_hole_leaks_and_the_path_runs_from_the_seed_to_the_border() {
        let mut map = Map::new();
        room_without_ceiling(&mut map);
        let layer = map.default_layer();
        for (min, max) in [
            ([-16.0, 128.0, -16.0], [144.0, 144.0, 48.0]),
            ([-16.0, 128.0, 80.0], [144.0, 144.0, 144.0]),
            ([-16.0, 128.0, 48.0], [48.0, 144.0, 80.0]),
            ([80.0, 128.0, 48.0], [144.0, 144.0, 80.0]),
        ] {
            bx(&mut map, layer, min, max);
        }

        let seed = start(&mut map, [64.0, 64.0, 64.0]);
        let r = run(&map);
        let leak = r.leak().expect("the hole leaks");
        assert_eq!(leak.seed, seed);
        assert_eq!(leak.path[0], DVec3::new(64.0, 64.0, 64.0));
        let end = leak.path.last().unwrap();
        assert!(end.y > 144.0, "the path ends above the roof, at {end}");
        assert!(leak.path.len() < 8, "straight runs are merged: {:?}", leak.path);
        assert!(r.dropped.is_empty(), "a leaking map keeps every face");
    }

    #[test]
    fn the_switch_turns_it_off_and_an_entity_can_keep_its_faces() {
        let mut map = Map::new();
        room_without_ceiling(&mut map);
        ceiling(&mut map);
        start(&mut map, [64.0, 64.0, 64.0]);
        let layer = map.default_layer();
        let detail = map.insert(layer, NodeKind::Entity(td_doc::Entity::new("func_detail")));
        let outer = bx(&mut map, detail, [200.0, 0.0, 0.0], [232.0, 32.0, 32.0]);
        assert!(run(&map).dropped.contains_key(&outer), "a pillar outside the room only faces the void");

        map.entity_mut(detail).unwrap().properties.insert(SWITCH.into(), "0".into());
        let r = run(&map);
        assert_eq!(r.outcome, Outcome::Sealed);
        assert!(!r.dropped.contains_key(&outer), "the entity keeps its faces");
        assert!(r.dropped_count() > 20, "the rest of the map is still culled");

        map.properties.insert(SWITCH.into(), "0".into());
        map.entity_mut(detail).unwrap().properties.insert(SWITCH.into(), "1".into());
        let r = run(&map);
        assert_eq!(r.outcome, Outcome::Off);
        assert!(r.dropped.is_empty(), "the entity cannot turn it back on");

        map.properties.remove(SWITCH);
        let off = analyze(&map, &GameConfig::default(), &|_| true, &AtomicBool::new(false));
        assert_eq!(off.outcome, Outcome::Off, "a map without the key is not culled");
    }

    #[test]
    fn two_brushes_meeting_in_a_seam_do_not_leak() {
        let mut map = Map::new();
        let layer = map.default_layer();
        bx(&mut map, layer, [-16.0, -16.0, -16.0], [144.0, 0.0, 144.0]);
        ceiling(&mut map);
        bx(&mut map, layer, [-16.0, 0.0, -16.0], [0.0, 128.0, 144.0]);
        bx(&mut map, layer, [128.0, 0.0, -16.0], [144.0, 128.0, 144.0]);
        // Cut at x = 60, and at the lattice plane x = 64.
        bx(&mut map, layer, [0.0, 0.0, 128.0], [60.0, 128.0, 144.0]);
        bx(&mut map, layer, [60.0, 0.0, 128.0], [128.0, 128.0, 144.0]);
        bx(&mut map, layer, [0.0, 0.0, -16.0], [64.0, 128.0, 0.0]);
        bx(&mut map, layer, [64.0, 0.0, -16.0], [128.0, 128.0, 0.0]);
        start(&mut map, [64.0, 64.0, 64.0]);
        assert_eq!(run(&map).outcome, Outcome::Sealed);
    }

    #[test]
    fn a_closed_mesh_seals_and_a_moving_body_does_not() {
        let mut map = Map::new();
        let walls = room_without_ceiling(&mut map);
        ceiling(&mut map);
        start(&mut map, [64.0, 64.0, 64.0]);
        let layer = map.default_layer();
        map.remove(walls[4]);
        let c = DVec3::new;
        let (x0, y0, z0, x1, y1, z1) = (0.0, 0.0, 128.0, 128.0, 128.0, 144.0);
        let quad = |a: DVec3, b: DVec3, d: DVec3, e: DVec3| (vec![a, b, d, e], td_geom::FaceData::new("dev/grey", Default::default()));
        let mesh = Mesh::from_polygons(vec![
            quad(c(x0, y0, z0), c(x0, y1, z0), c(x1, y1, z0), c(x1, y0, z0)),
            quad(c(x0, y0, z1), c(x1, y0, z1), c(x1, y1, z1), c(x0, y1, z1)),
            quad(c(x0, y0, z0), c(x0, y0, z1), c(x0, y1, z1), c(x0, y1, z0)),
            quad(c(x1, y0, z0), c(x1, y1, z0), c(x1, y1, z1), c(x1, y0, z1)),
            quad(c(x0, y0, z0), c(x1, y0, z0), c(x1, y0, z1), c(x0, y0, z1)),
            quad(c(x0, y1, z0), c(x0, y1, z1), c(x1, y1, z1), c(x1, y1, z0)),
        ]);
        let wall = map.insert(layer, NodeKind::Mesh(mesh));
        assert!(map.mesh(wall).unwrap().edge_faces().values().all(|f| f.len() == 2));
        assert_eq!(run(&map).outcome, Outcome::Sealed, "the mesh closes the room");

        map.remove(wall);
        let lift = map.insert(layer, NodeKind::Entity(td_doc::Entity::new("func_lift")));
        bx(&mut map, lift, [0.0, 0.0, 128.0], [128.0, 128.0, 144.0]);
        let mut game = GameConfig::default();
        game.entities.push(serde_json::from_str(r#"{"classname": "func_lift", "type": "solid", "node_class": "AnimatableBody3D"}"#).unwrap());
        let r = run_with(&map, &game);
        assert!(r.leak().is_some(), "a body that moves seals nothing");
    }

    #[test]
    fn a_face_seen_through_a_narrow_alcove_stays() {
        let mut map = Map::new();
        let layer = map.default_layer();
        bx(&mut map, layer, [-16.0, -16.0, -16.0], [176.0, 0.0, 144.0]);
        bx(&mut map, layer, [-16.0, 128.0, -16.0], [176.0, 144.0, 144.0]);
        bx(&mut map, layer, [-16.0, 0.0, -16.0], [0.0, 128.0, 144.0]);
        bx(&mut map, layer, [0.0, 0.0, -16.0], [176.0, 128.0, 0.0]);
        bx(&mut map, layer, [0.0, 0.0, 128.0], [176.0, 128.0, 144.0]);
        // A thick right wall with an alcove 16 wide cut into it and a plate closing its back.
        bx(&mut map, layer, [128.0, 0.0, 0.0], [176.0, 32.0, 128.0]);
        bx(&mut map, layer, [128.0, 64.0, 0.0], [176.0, 128.0, 128.0]);
        bx(&mut map, layer, [128.0, 32.0, 0.0], [176.0, 64.0, 48.0]);
        bx(&mut map, layer, [128.0, 32.0, 64.0], [176.0, 64.0, 128.0]);
        let back = bx(&mut map, layer, [160.0, 32.0, 48.0], [176.0, 64.0, 64.0]);
        start(&mut map, [64.0, 64.0, 64.0]);
        let r = run(&map);
        assert_eq!(r.outcome, Outcome::Sealed);
        assert!(!dropped(&r, &map, back, DVec3::NEG_X), "the back of the alcove is visible");
        assert!(dropped(&r, &map, back, DVec3::X));
    }

    fn sealed_room(map: &mut Map) {
        room_without_ceiling(map);
        ceiling(map);
        start(map, [64.0, 64.0, 64.0]);
    }

    #[test]
    fn a_brush_entity_without_brushes_is_not_a_seed() {
        let mut map = Map::new();
        sealed_room(&mut map);
        let layer = map.default_layer();
        let mut empty = td_doc::Entity::new("func_wall");
        empty.origin = DVec3::new(2000.0, 0.0, 0.0);
        map.insert(layer, NodeKind::Entity(empty));
        let mut game = GameConfig::default();
        game.entities.push(serde_json::from_str(r#"{"classname": "func_wall", "type": "solid"}"#).unwrap());
        let r = run_with(&map, &game);
        assert_eq!(r.outcome, Outcome::Sealed);
        assert!(run(&map).leak().is_some(), "an entity the game does not know is a point entity");
    }
}
