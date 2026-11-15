//! Terrain3D regions as the terrain tiles the views draw and pick. A planner decides per region how much detail the
//! views need, the builder turns what the store holds into tiles within a time budget.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use td_core::{DVec2, DVec3, NodeId};
use td_formats::terrain3d_cache::Lod;
use td_geom::terrain3d::{Grid, Loc, Region3D};
use td_geom::terrain3d_tile::{self, Level, Tile};

/// Regions this many vertices from the 3D camera load whole. Counted in vertices rather than regions or meters, it caps
/// what the whole regions cost whatever their size and spacing.
pub const FULL_REACH: f64 = 1024.0;
/// A 2D view loads a region whole while a vertex covers more than this many pixels.
pub const FULL_VERTEX_PX: f64 = 1.0;
/// A region that is not whole uses the coarsest level that keeps a vertex under this many pixels.
pub const MAX_VERTEX_PX: f64 = 4.0;

/// What a view looks at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum View {
    /// The 3D camera, `focal` is how many pixels one unit covers at distance one.
    Eye { position: DVec3, focal: f64 },
    /// A 2D view. `min` and `max` are the visible x and z, infinite along the axis the view looks past.
    Flat { min: DVec2, max: DVec2, zoom: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    Full,
    /// The coarsest pyramid level wanted, as its vertex stride.
    Lod(u32),
}

impl Detail {
    /// The finer of two.
    fn finer(self, other: Self) -> Self {
        match (self, other) {
            (Detail::Lod(a), Detail::Lod(b)) => Detail::Lod(a.min(b)),
            _ => Detail::Full,
        }
    }
}

fn coarsest(region_size: u32, px_per_vertex: f64) -> Detail {
    let strides: Vec<u32> = terrain3d_tile::lod_strides(region_size).into_iter().filter(|s| *s > 1).collect();
    let Some(finest) = strides.first().copied() else { return Detail::Full };
    Detail::Lod(strides.iter().rev().copied().find(|s| px_per_vertex * *s as f64 <= MAX_VERTEX_PX).unwrap_or(finest))
}

/// Where on the ground a view looks, for loading the regions nearest to it first.
pub fn focus(view: &View) -> DVec2 {
    match *view {
        View::Eye { position, .. } => DVec2::new(position.x, position.z),
        View::Flat { min, max, .. } => {
            let mid = (min + max) * 0.5;
            DVec2::new(if mid.x.is_finite() { mid.x } else { 0.0 }, if mid.y.is_finite() { mid.y } else { 0.0 })
        }
    }
}

/// The map unit rectangle (x, z) a region covers.
pub fn region_rect(grid: &Grid, loc: Loc) -> (DVec2, DVec2) {
    let min = grid.position(grid.vgrid_of(loc, [0, 0]));
    (min, min + DVec2::splat(grid.region_size as f64 * grid.cell()))
}

fn detail_for(grid: &Grid, loc: Loc, view: &View) -> Option<Detail> {
    let (min, max) = region_rect(grid, loc);
    let cell = grid.cell();
    match *view {
        View::Eye { position, focal } => {
            let nearest = DVec2::new(position.x, position.z).clamp(min, max);
            if nearest.distance(DVec2::new(position.x, position.z)) <= FULL_REACH * cell {
                return Some(Detail::Full);
            }

            let distance = nearest.distance(DVec2::new(position.x, position.z)).hypot(position.y).max(1.0);
            Some(coarsest(grid.region_size, focal * cell / distance))
        }
        View::Flat { min: lo, max: hi, zoom } => {
            if max.x < lo.x || min.x > hi.x || max.y < lo.y || min.y > hi.y {
                return None;
            }

            let px = zoom * cell;
            if px > FULL_VERTEX_PX { Some(Detail::Full) } else { Some(coarsest(grid.region_size, px)) }
        }
    }
}

/// How much of each region the views need. A region no view looks at is left out.
pub fn plan(grid: &Grid, locs: &[Loc], views: &[View]) -> BTreeMap<Loc, Detail> {
    let mut out = BTreeMap::new();
    for &loc in locs {
        let wanted = views.iter().filter_map(|v| detail_for(grid, loc, v)).reduce(Detail::finer);
        if let Some(detail) = wanted {
            out.insert(loc, detail);
        }
    }

    out
}

/// What to ask the store for: the regions to hold whole, which include the east, south and south east neighbors a whole
/// region takes its seam from, and the regions a pyramid is enough for.
pub fn wants(plan: &BTreeMap<Loc, Detail>, exists: &dyn Fn(Loc) -> bool) -> (Vec<Loc>, Vec<Loc>) {
    let mut full = BTreeSet::new();
    for (loc, detail) in plan {
        if *detail == Detail::Full {
            full.insert(*loc);
            for n in [[1, 0], [0, 1], [1, 1]] {
                let n = [loc[0].saturating_add(n[0]), loc[1].saturating_add(n[1])];
                if exists(n) {
                    full.insert(n);
                }
            }
        }
    }

    let lod = plan.iter().filter(|(l, d)| matches!(d, Detail::Lod(_)) && !full.contains(*l)).map(|(l, _)| *l).collect();
    (full.into_iter().collect(), lod)
}

/// Where a tile's data came from, so an unchanged source keeps its tile. Edits are told apart by identity, cached
/// regions by the hash Godot gave them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Absent,
    Cached([u8; 32]),
    Edited(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub stride: u32,
    pub lod: bool,
    pub parts: [Source; 4],
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub key: Key,
    pub tile: Arc<Tile>,
}

/// What the builder reads the world through.
pub struct World<'a> {
    pub grid: Grid,
    /// The region with TrenchDot's edits applied, None when it is not loaded.
    pub region: &'a dyn Fn(Loc) -> Option<(Arc<Region3D>, Source)>,
    pub lod: &'a dyn Fn(Loc) -> Option<Arc<Lod>>,
    pub material: &'a dyn Fn(u8) -> String,
}

const SEAM: [[i32; 2]; 3] = [[1, 0], [0, 1], [1, 1]];

fn near(loc: Loc, n: [i32; 2]) -> Loc {
    [loc[0].saturating_add(n[0]), loc[1].saturating_add(n[1])]
}

fn full_tile(world: &World, loc: Loc) -> Option<(Key, Tile)> {
    let (own, source) = (world.region)(loc)?;
    let mut held: HashMap<Loc, Arc<Region3D>> = HashMap::new();
    let mut parts = [source, Source::Absent, Source::Absent, Source::Absent];
    held.insert(loc, own.clone());
    for (k, n) in SEAM.iter().enumerate() {
        if let Some((r, s)) = (world.region)(near(loc, *n)) {
            held.insert(near(loc, *n), r);
            parts[k + 1] = s;
        }
    }

    let tile = terrain3d_tile::tile(&world.grid, &own, |l| held.get(&l).map(|r| &**r), 1, world.material)?;
    Some((Key { stride: 1, lod: false, parts }, tile))
}

fn level_of(lod: &Lod, stride: u32) -> Option<Level<'_>> {
    let v = lod.levels.iter().find(|v| v.stride == stride)?;
    Some(Level { stride, side: v.side, ids: &v.ids, heights: &v.heights, splat: &v.splat, holes: &v.holes })
}

fn lod_tile(world: &World, loc: Loc, want: u32) -> Option<(Key, Tile)> {
    let own = (world.lod)(loc)?;
    let stride = own.levels.iter().map(|l| l.stride).filter(|s| *s <= want).max().or_else(|| own.levels.iter().map(|l| l.stride).min())?;
    let neighbors: Vec<Option<Arc<Lod>>> = SEAM.iter().map(|n| (world.lod)(near(loc, *n))).collect();
    let at = |k: usize| neighbors[k].as_deref().and_then(|l| level_of(l, stride));
    let mut parts = [Source::Cached(own.hash.0), Source::Absent, Source::Absent, Source::Absent];
    for (k, n) in neighbors.iter().enumerate() {
        if let Some(n) = n {
            parts[k + 1] = Source::Cached(n.hash.0);
        }
    }

    let tile = terrain3d_tile::level_tile(&world.grid, loc, level_of(&own, stride)?, at(0), at(1), at(2), world.material)?;
    Some((Key { stride, lod: true, parts }, tile))
}

/// Brings `tiles` up to `plan`, nearest regions first, stopping once `budget` is spent (one tile is always built). A
/// region wanted whole that is still loading keeps whatever it showed. Returns whether anything changed and whether the
/// budget ran out with tiles left to build.
pub fn rebuild(tiles: &mut BTreeMap<Loc, Entry>, world: &World, plan: &BTreeMap<Loc, Detail>, order: &[Loc], budget: Duration) -> (bool, bool) {
    let started = Instant::now();
    let before = tiles.len();
    let mut changed = false;
    tiles.retain(|l, _| plan.contains_key(l));
    changed |= tiles.len() != before;
    let mut built = 0usize;
    for loc in order {
        let Some(detail) = plan.get(loc) else { continue };
        if built > 0 && started.elapsed() >= budget {
            return (changed, true);
        }

        let wanted = match detail {
            Detail::Full => full_tile(world, *loc).or_else(|| lod_tile(world, *loc, 1)),
            Detail::Lod(stride) => lod_tile(world, *loc, *stride).or_else(|| full_tile(world, *loc)),
        };
        let Some((key, tile)) = wanted else { continue };
        if tiles.get(loc).is_some_and(|e| e.key == key) {
            continue;
        }

        tiles.insert(*loc, Entry { key, tile: Arc::new(tile) });
        changed = true;
        built += 1;
    }

    (changed, false)
}

/// Where the regions of a linked Terrain3D node are, for the 2D views to mark.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub grid: Grid,
    pub locs: Vec<Loc>,
}

/// The tiles of every linked Terrain3D node.
#[derive(Default)]
pub struct Tiles {
    nodes: BTreeMap<NodeId, BTreeMap<Loc, Entry>>,
    /// Counts every change, so a view knows to look again.
    pub generation: u64,
    /// Some tile is still to build or load, the next frame should go on.
    pub pending: bool,
    pub conflicts: BTreeMap<NodeId, BTreeSet<Loc>>,
    pub layouts: BTreeMap<NodeId, Layout>,
    conflicts_for: Option<(u64, u64)>,
}

impl Tiles {
    pub fn node(&self, id: NodeId) -> impl Iterator<Item = (Loc, &Entry)> {
        self.nodes.get(&id).into_iter().flat_map(|m| m.iter().map(|(l, e)| (*l, e)))
    }

    pub fn nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.keys().copied()
    }

    pub(super) fn entries(&mut self, id: NodeId) -> &mut BTreeMap<Loc, Entry> {
        self.nodes.entry(id).or_default()
    }

    pub(super) fn keep_only(&mut self, ids: &BTreeSet<NodeId>) -> bool {
        let before = self.nodes.len();
        self.nodes.retain(|id, _| ids.contains(id));
        self.conflicts.retain(|id, _| ids.contains(id));
        let layouts = self.layouts.len();
        self.layouts.retain(|id, _| ids.contains(id));
        self.nodes.len() != before || self.layouts.len() != layouts
    }

    pub(super) fn clear(&mut self, id: NodeId) -> bool {
        let had = self.nodes.get(&id).is_some_and(|m| !m.is_empty());
        self.nodes.remove(&id);
        self.conflicts.remove(&id);
        had | self.layouts.remove(&id).is_some()
    }

    pub(super) fn set_layout(&mut self, id: NodeId, layout: Layout) -> bool {
        if self.layouts.get(&id) == Some(&layout) {
            return false;
        }

        self.layouts.insert(id, layout);
        true
    }

    pub(super) fn conflicts_stale(&self, key: (u64, u64)) -> bool {
        self.conflicts_for != Some(key)
    }

    pub(super) fn set_conflicts(&mut self, key: (u64, u64), found: BTreeMap<NodeId, BTreeSet<Loc>>) {
        self.conflicts = found;
        self.conflicts_for = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use td_formats::terrain3d_cache::RegionHash;

    fn grid(size: u32) -> Grid {
        Grid { region_size: size, spacing: 2.0, upm: 32.0 }
    }

    fn flat(x: f64, z: f64, w: f64, h: f64, zoom: f64) -> View {
        View::Flat { min: DVec2::new(x, z), max: DVec2::new(x + w, z + h), zoom }
    }

    fn at(grid: &Grid, loc: Loc) -> DVec3 {
        let (min, _) = region_rect(grid, loc);
        DVec3::new(min.x + 100.0, 50.0, min.y + 100.0)
    }

    #[test]
    fn the_3d_camera_loads_whole_the_regions_within_reach_and_the_rest_coarse() {
        let g = grid(256);
        let locs: Vec<Loc> = (-12..=12).map(|x| [x, -3]).collect();
        let eye = View::Eye { position: at(&g, [2, -3]), focal: 800.0 };
        let plan = plan(&g, &locs, &[eye]);
        let full: Vec<i32> = plan.iter().filter(|(_, d)| **d == Detail::Full).map(|(l, _)| l[0]).collect();
        // 1024 vertices are 4 regions of 256, the camera stands 100 units into region 2 so region -3 ends just past them.
        assert_eq!(full, (-2..=6).collect::<Vec<_>>());
        assert!(matches!(plan[&[-12, -3]], Detail::Lod(s) if s > 1));
        assert!(matches!(plan[&[-3, -3]], Detail::Lod(_)), "100 units past the reach");
        assert!(matches!(plan[&[7, -3]], Detail::Lod(_)), "the first region past the reach");
    }

    #[test]
    fn a_world_of_big_regions_loads_only_the_regions_next_to_the_camera_whole() {
        let g = Grid { region_size: 1024, spacing: 1.0, upm: 32.0 };
        let locs: Vec<Loc> = (-8..=8).flat_map(|x| (-8..=8).map(move |z| [x, z])).collect();
        let eye = View::Eye { position: at(&g, [3, -2]), focal: 800.0 };
        let plan = plan(&g, &locs, &[eye]);
        assert_eq!(plan.len(), 289);
        let full: Vec<Loc> = plan.iter().filter(|(_, d)| **d == Detail::Full).map(|(l, _)| *l).collect();
        assert_eq!(full, [[2, -3], [2, -2], [2, -1], [3, -3], [3, -2], [3, -1], [4, -3], [4, -2]], "[4, -1] is a diagonal past the reach");
    }

    #[test]
    fn a_farther_region_takes_a_coarser_level_until_the_coarsest() {
        let g = grid(1024);
        let locs: Vec<Loc> = vec![[10, 0], [60, 0], [400, 0], [30000, 0]];
        let eye = View::Eye { position: DVec3::new(0.0, 10.0, 0.0), focal: 100_000.0 };
        let plan = plan(&g, &locs, &[eye]);
        let strides: Vec<u32> = locs
            .iter()
            .map(|l| match plan[l] {
                Detail::Lod(s) => s,
                Detail::Full => 1,
            })
            .collect();
        assert_eq!(strides, [4, 4, 16, 64]);
    }

    #[test]
    fn a_2d_view_loads_whole_only_above_one_pixel_per_vertex() {
        let g = grid(64);
        let cell = g.cell();
        let one = |zoom| plan(&g, &[[0, 0]], &[flat(-10.0, -10.0, 100.0, 100.0, zoom)]).get(&[0, 0]).copied();
        assert_eq!(one(1.0 / cell + 1e-6), Some(Detail::Full));
        assert_eq!(one(1.0 / cell), Some(Detail::Lod(4)), "exactly one pixel is not above one pixel");
        assert_eq!(one(1.0 / (cell * 4.0)), Some(Detail::Lod(4)));
        assert_eq!(one(0.0001), Some(Detail::Lod(4)), "a 64 region only has the 4 level");
    }

    #[test]
    fn a_2d_view_skips_regions_outside_its_box_and_the_finest_view_wins() {
        let g = grid(64);
        let size = 64.0 * g.cell();
        let locs = [[0, 0], [1, 0], [-1, 0], [0, 1]];
        let top = flat(0.0, 0.0, size, size, 0.5);
        let p = plan(&g, &locs, &[top]);
        assert_eq!(p.keys().copied().collect::<Vec<_>>(), [[-1, 0], [0, 0], [0, 1], [1, 0]], "touching the edge counts");
        let p = plan(&g, &locs, &[flat(0.0, 0.0, 10.0, 10.0, 0.0001), flat(0.0, 0.0, 10.0, 10.0, 1.0)]);
        assert_eq!(p[&[0, 0]], Detail::Full);
        assert!(plan(&g, &locs, &[]).is_empty());
        let side = View::Flat { min: DVec2::new(f64::NEG_INFINITY, 0.0), max: DVec2::new(f64::INFINITY, 10.0), zoom: 1.0 };
        assert_eq!(plan(&g, &locs, &[side]).len(), 3, "the view looks past x, so only z decides");
    }

    #[test]
    fn a_region_of_32_has_no_levels_and_is_always_whole() {
        let g = grid(32);
        let p = plan(&g, &[[0, 0], [5, 0]], &[View::Eye { position: DVec3::new(0.0, 10.0, 0.0), focal: 100.0 }]);
        assert_eq!(p[&[5, 0]], Detail::Full);
        let p = plan(&g, &[[0, 0]], &[flat(-1.0, -1.0, 2.0, 2.0, 0.0001)]);
        assert_eq!(p[&[0, 0]], Detail::Full);
    }

    #[test]
    fn whole_regions_pull_in_their_seam_neighbors_and_never_double_count() {
        let plan = BTreeMap::from([([0, 0], Detail::Full), ([5, 5], Detail::Lod(16)), ([1, 0], Detail::Lod(4)), ([9, 9], Detail::Lod(4))]);
        let exists = |l: Loc| [[1, 0], [0, 1], [5, 5], [1, 1]].contains(&l);
        let (full, lod) = wants(&plan, &exists);
        assert_eq!(full, [[0, 0], [0, 1], [1, 0], [1, 1]]);
        assert_eq!(lod, [[5, 5], [9, 9]]);
    }

    fn region(loc: Loc, bump: f32) -> Region3D {
        let mut r = Region3D::blank(loc, 64);
        for (k, h) in r.heights.iter_mut().enumerate() {
            *h = (k % 64) as f32 * 0.5 + loc[0] as f32 * 3.0 + bump;
        }

        r
    }

    fn lod_of(g: &Grid, r: &Region3D, n: u8) -> Lod {
        crate::terrain3d::store::build_lod(g, r, RegionHash([n; 32])).unwrap()
    }

    #[test]
    fn rebuild_keeps_a_tile_whose_source_did_not_change_and_replaces_one_that_did() {
        let g = grid(64);
        let regions: BTreeMap<Loc, (Arc<Region3D>, u8)> = [[-1, 2], [0, 2]].iter().map(|l| (*l, (Arc::new(region(*l, 0.0)), (l[0] + 2) as u8))).collect();
        let edited = Arc::new(region([0, 2], 9.0));
        let material = |id: u8| format!("t{id}");
        let get = |l: Loc| regions.get(&l).map(|(r, n)| (r.clone(), Source::Cached([*n; 32])));
        let lod = |_: Loc| None;
        let world = World { grid: g, region: &get, lod: &lod, material: &material };
        let plan = BTreeMap::from([([-1, 2], Detail::Full), ([0, 2], Detail::Full)]);
        let order = [[-1, 2], [0, 2]];
        let mut tiles = BTreeMap::new();
        let (changed, pending) = rebuild(&mut tiles, &world, &plan, &order, Duration::from_secs(5));
        assert!(changed && !pending);
        assert_eq!(tiles.len(), 2);
        let first = tiles[&[-1, 2]].tile.clone();

        let (changed, pending) = rebuild(&mut tiles, &world, &plan, &order, Duration::from_secs(5));
        assert!(!changed && !pending);
        assert!(Arc::ptr_eq(&first, &tiles[&[-1, 2]].tile), "nothing changed, nothing rebuilt");

        let edit = edited.clone();
        let get = |l: Loc| {
            if l == [0, 2] {
                Some((edit.clone(), Source::Edited(Arc::as_ptr(&edit) as usize)))
            } else {
                regions.get(&l).map(|(r, n)| (r.clone(), Source::Cached([*n; 32])))
            }
        };
        let world = World { grid: g, region: &get, lod: &lod, material: &material };
        let (changed, _) = rebuild(&mut tiles, &world, &plan, &order, Duration::from_secs(5));
        assert!(changed);
        assert!(!Arc::ptr_eq(&first, &tiles[&[-1, 2]].tile), "the west tile takes its east seam from the edited region");
        assert_eq!(tiles[&[0, 2]].tile.terrain.heights[0], 9.0 * 32.0);
    }

    #[test]
    fn rebuild_stops_at_the_budget_after_one_tile_and_drops_regions_that_left_the_plan() {
        let g = grid(64);
        let locs = [[3, -4], [4, -4], [5, -4]];
        let regions: BTreeMap<Loc, Arc<Region3D>> = locs.iter().map(|l| (*l, Arc::new(region(*l, 1.0)))).collect();
        let get = |l: Loc| regions.get(&l).map(|r| (r.clone(), Source::Cached([l[0] as u8; 32])));
        let lod = |_: Loc| None;
        let material = |id: u8| format!("t{id}");
        let world = World { grid: g, region: &get, lod: &lod, material: &material };
        let plan: BTreeMap<Loc, Detail> = locs.iter().map(|l| (*l, Detail::Full)).collect();
        let mut tiles = BTreeMap::new();
        let (changed, pending) = rebuild(&mut tiles, &world, &plan, &locs, Duration::ZERO);
        assert!(changed && pending);
        assert_eq!(tiles.keys().copied().collect::<Vec<_>>(), [[3, -4]], "the nearest first, one per call");

        let plan = BTreeMap::from([([5, -4], Detail::Full)]);
        let (changed, pending) = rebuild(&mut tiles, &world, &plan, &[[5, -4]], Duration::from_secs(5));
        assert!(changed && !pending);
        assert_eq!(tiles.keys().copied().collect::<Vec<_>>(), [[5, -4]]);
    }

    #[test]
    fn a_region_that_is_not_loaded_yet_draws_its_pyramid_and_keeps_it_until_the_region_arrives() {
        let g = grid(64);
        let (a, b) = (region([0, 0], 0.0), region([1, 0], 2.0));
        let lods: BTreeMap<Loc, Arc<Lod>> = [([0, 0], Arc::new(lod_of(&g, &a, 1))), ([1, 0], Arc::new(lod_of(&g, &b, 2)))].into();
        let none = |_: Loc| None;
        let get_lod = |l: Loc| lods.get(&l).cloned();
        let material = |id: u8| format!("t{id}");
        let world = World { grid: g, region: &none, lod: &get_lod, material: &material };
        let plan = BTreeMap::from([([0, 0], Detail::Full), ([1, 0], Detail::Lod(4))]);
        let mut tiles = BTreeMap::new();
        let (changed, pending) = rebuild(&mut tiles, &world, &plan, &[[0, 0], [1, 0]], Duration::from_secs(5));
        assert!(changed && !pending);
        assert!(tiles.values().all(|e| e.key.lod && e.key.stride == 4));
        assert_eq!(tiles[&[0, 0]].tile.terrain.resolution, [17, 17]);
        let east_seam = tiles[&[0, 0]].tile.terrain.heights[16];
        assert_eq!(east_seam, b.heights[0] * 32.0, "the seam comes from the neighbor's level");

        let full = Arc::new(a.clone());
        let get = |l: Loc| (l == [0, 0]).then(|| (full.clone(), Source::Cached([9; 32])));
        let world = World { grid: g, region: &get, lod: &get_lod, material: &material };
        rebuild(&mut tiles, &world, &plan, &[[0, 0], [1, 0]], Duration::from_secs(5));
        assert!(!tiles[&[0, 0]].key.lod && tiles[&[0, 0]].tile.terrain.resolution == [65, 65]);
        assert!(tiles[&[1, 0]].key.lod);

        let world = World { grid: g, region: &none, lod: &none_lod, material: &material };
        let before = tiles[&[0, 0]].tile.clone();
        let (changed, pending) = rebuild(&mut tiles, &world, &plan, &[[0, 0], [1, 0]], Duration::from_secs(5));
        assert!(!changed && !pending && Arc::ptr_eq(&before, &tiles[&[0, 0]].tile), "nothing to build from, so what is shown stays");
    }

    fn none_lod(_: Loc) -> Option<Arc<Lod>> {
        None
    }
}
