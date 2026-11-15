//! The link to a Terrain3D data folder in the Godot project. Godot reads and writes the folder, the map only names it
//! and holds the regions TrenchDot edited until they are sent.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use td_core::{DVec2, DVec3, NodeId};
use td_geom::terrain3d::{EdgeBlend, Grid, LOC_MAX, LOC_MIN, Loc, Region3D, loc_in_range, surface_height};
use td_geom::terrain3d_tile::{self, PaintMode};

use crate::map::{Map, NodeKind};
use crate::terrain::{SculptBrush, SculptMode, sculpt_terrains_single};

/// The sha256 Godot computes over a region's height, control and color bytes. TrenchDot only echoes it back.
pub type RegionHash = [u8; 32];

/// What Godot last exported for a linked folder. The editor's cache store implements it.
pub trait RegionSource {
    fn grid(&self) -> Grid;
    fn edge_blend(&self) -> Option<EdgeBlend>;
    fn locs(&self) -> Vec<Loc>;
    fn hash(&self, loc: Loc) -> Option<RegionHash>;
    /// None for a region that does not exist or is not loaded at full resolution.
    fn region(&self, loc: Loc) -> Option<Arc<Region3D>>;
}

#[derive(Clone, Debug)]
pub struct RegionEdit {
    pub region: Arc<Region3D>,
    /// The hash of the cached region the edit started from, None for a region TrenchDot added.
    pub base_hash: Option<RegionHash>,
    /// The region as it was at `base_hash`, when known. An edit whose region is this very Arc is one Godot applied, it
    /// differs from nothing and gives way to the cache once Godot changes the region again.
    pub base: Option<Arc<Region3D>>,
}

impl RegionEdit {
    pub fn new(region: Arc<Region3D>, base_hash: Option<RegionHash>) -> Self {
        Self { region, base_hash, base: None }
    }

    pub fn applied(&self) -> bool {
        self.base.as_ref().is_some_and(|b| Arc::ptr_eq(b, &self.region))
    }
}

impl PartialEq for RegionEdit {
    fn eq(&self, other: &Self) -> bool {
        self.base_hash == other.base_hash
            && self.base.as_ref().map(Arc::as_ptr) == other.base.as_ref().map(Arc::as_ptr)
            && (Arc::ptr_eq(&self.region, &other.region) || same_bits(&self.region, &other.region))
    }
}

/// A region Godot accepted from a send, see [`Terrain3DLink::settle`].
#[derive(Clone, Debug)]
pub struct Settled {
    pub loc: Loc,
    /// What was sent, None for a removal.
    pub region: Option<Arc<Region3D>>,
    /// The base hash the send carried.
    pub base_hash: Option<RegionHash>,
    /// The region before the edit that was sent, when known.
    pub before: Option<Arc<Region3D>>,
    /// The hash Godot exported after applying it.
    pub now: RegionHash,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Terrain3DLink {
    /// A `res://` path to the folder Terrain3D saves its regions in.
    pub data_directory: String,
    /// Regions edited in TrenchDot. A map save sends them as pending patches, the map file never holds them.
    #[serde(skip)]
    pub edits: imbl::OrdMap<Loc, RegionEdit>,
    /// Cached regions TrenchDot removed, with the hash each had.
    #[serde(skip)]
    pub removed: imbl::OrdMap<Loc, RegionHash>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Add,
    Edit,
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegionChange {
    pub loc: Loc,
    pub kind: ChangeKind,
    /// None for a removal.
    pub region: Option<Arc<Region3D>>,
    pub base_hash: Option<RegionHash>,
    /// Godot changed the region since the edit started from it, or the world's region size no longer fits the edit.
    pub conflict: bool,
}

/// Regions to add (positive) or remove (negative) on each side of the region map. North is toward -z.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grow {
    pub north: i32,
    pub south: i32,
    pub west: i32,
    pub east: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrowPlan {
    /// Region map bounds after the change, inclusive.
    pub min: Loc,
    pub max: Loc,
    pub add: Vec<Loc>,
    pub remove: Vec<Loc>,
}

impl Terrain3DLink {
    pub fn new(data_directory: impl Into<String>) -> Self {
        Self { data_directory: data_directory.into(), edits: imbl::OrdMap::new(), removed: imbl::OrdMap::new() }
    }

    /// The edit at `loc`, unless it is an applied one that Godot changed since.
    pub fn live_edit(&self, source: &dyn RegionSource, loc: Loc) -> Option<&RegionEdit> {
        self.edits.get(&loc).filter(|e| !e.applied() || source.hash(loc) == e.base_hash)
    }

    /// The region at `loc` with TrenchDot's edits applied.
    pub fn region(&self, source: &dyn RegionSource, loc: Loc) -> Option<Arc<Region3D>> {
        if let Some(edit) = self.live_edit(source, loc) {
            return Some(edit.region.clone());
        }

        if self.removed.contains_key(&loc) { None } else { source.region(loc) }
    }

    pub fn has_region(&self, source: &dyn RegionSource, loc: Loc) -> bool {
        self.live_edit(source, loc).is_some() || (!self.removed.contains_key(&loc) && source.hash(loc).is_some())
    }

    /// Every region with TrenchDot's edits applied, row by row from the north west.
    pub fn locs(&self, source: &dyn RegionSource) -> Vec<Loc> {
        let mut out: Vec<Loc> = source.locs().into_iter().filter(|l| !self.removed.contains_key(l)).collect();
        out.extend(self.edits.keys().copied().filter(|l| self.live_edit(source, *l).is_some()));
        out.sort_by_key(|l| (l[1], l[0]));
        out.dedup();
        out
    }

    /// Region map bounds, inclusive. None without regions.
    pub fn bounds(&self, source: &dyn RegionSource) -> Option<(Loc, Loc)> {
        let locs = self.locs(source);
        let first = *locs.first()?;
        Some(locs.iter().fold((first, first), |(lo, hi), l| ([lo[0].min(l[0]), lo[1].min(l[1])], [hi[0].max(l[0]), hi[1].max(l[1])])))
    }

    /// Runs `f` on the region at `loc` and keeps the result as an edit when it reports a change.
    fn modify(&mut self, source: &dyn RegionSource, loc: Loc, f: impl FnOnce(&mut Region3D) -> bool) -> bool {
        if self.live_edit(source, loc).is_some() {
            let edit = self.edits.get_mut(&loc).expect("a live edit");
            // Clones only when an undo step or the base still holds the region, so the dabs of one stroke share one copy.
            return f(Arc::make_mut(&mut edit.region));
        }

        if self.removed.contains_key(&loc) {
            return false;
        }

        let (Some(base), Some(hash)) = (source.region(loc), source.hash(loc)) else { return false };
        let mut region = (*base).clone();
        if !f(&mut region) {
            return false;
        }

        self.edits.insert(loc, RegionEdit { region: Arc::new(region), base_hash: Some(hash), base: Some(base) });
        true
    }

    fn brush_regions(&mut self, source: &dyn RegionSource, center: DVec3, radius: f64, f: impl Fn(&Grid, &mut Region3D) -> bool) -> Vec<Loc> {
        let grid = source.grid();
        let c = DVec2::new(center.x, center.z);
        let (min, max) = grid.vgrid_rect(c - DVec2::splat(radius), c + DVec2::splat(radius), 0);
        grid.regions_in(min, max).into_iter().filter(|loc| self.modify(source, *loc, |r| f(&grid, r))).collect()
    }

    /// One sculpt dab at `center` (map units). Height modes run the terrain brushes on a window gathered across
    /// regions, so strokes cross region borders without a seam. Hole and Unhole set vertex hole bits. Layer and alpha
    /// modes do nothing, Terrain3D paints texture ids through `paint_texture`. Returns the regions that changed.
    pub fn sculpt(&mut self, source: &dyn RegionSource, center: DVec3, brush: &SculptBrush) -> Vec<Loc> {
        match brush.mode {
            SculptMode::Hole | SculptMode::Unhole => {
                let hole = brush.mode == SculptMode::Hole;
                return self.brush_regions(source, center, brush.radius, |g, r| terrain3d_tile::set_hole_vertices(g, r, center, brush.radius, hole));
            }
            SculptMode::PaintAlpha | SculptMode::EraseAlpha | SculptMode::PaintLayer => return Vec::new(),
            _ => {}
        }

        let grid = source.grid();
        let c = DVec2::new(center.x, center.z);
        // Smooth and Terrace read one ring of neighbors past the brush.
        let (min, max) = grid.vgrid_rect(c - DVec2::splat(brush.radius), c + DVec2::splat(brush.radius), 2);
        let locs = grid.regions_in(min, max);
        let regions: HashMap<Loc, Arc<Region3D>> = locs.iter().filter_map(|l| Some((*l, self.region(source, *l)?))).collect();
        let Some(mut window) = terrain3d_tile::window(&grid, |l| regions.get(&l).map(|r| r.as_ref()), min, max) else { return Vec::new() };
        // Holding these Arcs would make every dab of a stroke copy its regions again.
        drop(regions);
        let before = window.terrain.heights.clone();
        if !sculpt_terrains_single(&mut window.terrain, center, brush) {
            return Vec::new();
        }

        let w = window.terrain.resolution[0] as usize;
        let touched: HashSet<Loc> = (0..before.len())
            .filter(|k| window.present[*k] && before[*k].to_bits() != window.terrain.heights[*k].to_bits())
            .map(|k| grid.region_of([min[0] + (k % w) as i64, min[1] + (k / w) as i64]))
            .collect();
        locs.into_iter().filter(|loc| touched.contains(loc) && self.modify(source, *loc, |r| terrain3d_tile::write_window(&grid, &window, r))).collect()
    }

    /// One texture dab, Terrain3D's Paint or Spray. Returns the regions that changed.
    pub fn paint_texture(&mut self, source: &dyn RegionSource, center: DVec3, radius: f64, id: u8, mode: PaintMode, strength: f32) -> Vec<Loc> {
        self.brush_regions(source, center, radius, |g, r| terrain3d_tile::paint_texture(g, r, center, radius, id, mode, strength))
    }

    /// Adds blank regions like Terrain3D's `add_region_blank`. A location that has a region already is left alone.
    /// One location outside the region map refuses the whole call. Returns the regions added.
    pub fn add_regions(&mut self, source: &dyn RegionSource, locs: &[Loc]) -> Result<Vec<Loc>, String> {
        if let Some(bad) = locs.iter().find(|l| !loc_in_range(**l)) {
            return Err(format!("Terrain3D region {bad:?} is outside {LOC_MIN} to {LOC_MAX}"));
        }

        let size = source.grid().region_size;
        let mut added = Vec::new();
        for &loc in locs {
            if self.has_region(source, loc) {
                continue;
            }

            let base_hash = self.removed.remove(&loc);
            self.edits.insert(loc, RegionEdit::new(Arc::new(Region3D::blank(loc, size)), base_hash));
            added.push(loc);
        }

        Ok(added)
    }

    /// Removes regions. A region TrenchDot added just goes, a cached one is remembered so the send deletes it in
    /// Godot. Returns the regions removed.
    pub fn remove_regions(&mut self, source: &dyn RegionSource, locs: &[Loc]) -> Vec<Loc> {
        let mut gone = Vec::new();
        for &loc in locs {
            if !self.has_region(source, loc) {
                continue;
            }

            let live = self.live_edit(source, loc).is_some();
            let base = match self.edits.remove(&loc) {
                Some(edit) if live => edit.base_hash,
                _ => source.hash(loc),
            };
            if let Some(hash) = base {
                self.removed.insert(loc, hash);
            }

            gone.push(loc);
        }

        gone
    }

    /// What `grow` would do. Refused without regions, when a side shrinks past the other, or past Terrain3D's 32 regions.
    pub fn plan_grow(&self, source: &dyn RegionSource, grow: Grow) -> Result<GrowPlan, String> {
        let (lo, hi) = self.bounds(source).ok_or("the Terrain3D data folder has no regions to grow from")?;
        let min = [lo[0] as i64 - grow.west as i64, lo[1] as i64 - grow.north as i64];
        let max = [hi[0] as i64 + grow.east as i64, hi[1] as i64 + grow.south as i64];
        if min[0] > max[0] || min[1] > max[1] {
            return Err("shrinking by that much would remove every Terrain3D region".into());
        }

        let range = LOC_MIN as i64..=LOC_MAX as i64;
        if !(range.contains(&min[0]) && range.contains(&min[1]) && range.contains(&max[0]) && range.contains(&max[1])) {
            return Err(format!("Terrain3D holds at most 32 regions per side ({LOC_MIN} to {LOC_MAX}), the terrain would span {min:?} to {max:?}"));
        }

        let (min, max) = (min.map(|v| v as i32), max.map(|v| v as i32));
        let inside = |l: Loc, lo: Loc, hi: Loc| (lo[0]..=hi[0]).contains(&l[0]) && (lo[1]..=hi[1]).contains(&l[1]);
        let mut add = Vec::new();
        for y in min[1]..=max[1] {
            for x in min[0]..=max[0] {
                if !inside([x, y], lo, hi) {
                    add.push([x, y]);
                }
            }
        }

        let remove = self.locs(source).into_iter().filter(|l| !inside(*l, min, max)).collect();
        Ok(GrowPlan { min, max, add, remove })
    }

    /// Adds blank regions on the sides that grow and removes the rows and columns of the sides that shrink. The gaps of
    /// a sparse layout stay gaps.
    pub fn grow(&mut self, source: &dyn RegionSource, grow: Grow) -> Result<GrowPlan, String> {
        let plan = self.plan_grow(source, grow)?;
        self.add_regions(source, &plan.add)?;
        self.remove_regions(source, &plan.remove);
        Ok(plan)
    }

    /// Region map bounds, inclusive, once Godot reslices the terrain into regions of `region_size` vertices. Terrain3D
    /// keeps every vertex where it is, so the bounds follow from the vertex span. None without regions.
    pub fn resized_bounds(&self, source: &dyn RegionSource, region_size: u32) -> Result<Option<(Loc, Loc)>, String> {
        let Some((lo, hi)) = self.bounds(source) else { return Ok(None) };
        let (old, new) = (source.grid().region_size as i64, region_size.max(1) as i64);
        let min = lo.map(|v| (v as i64 * old).div_euclid(new));
        let max = hi.map(|v| ((v as i64 + 1) * old - 1).div_euclid(new));
        let range = LOC_MIN as i64..=LOC_MAX as i64;
        if !min.iter().chain(&max).all(|v| range.contains(v)) {
            return Err(format!(
                "Terrain3D holds at most 32 regions per side ({LOC_MIN} to {LOC_MAX}), at region size {region_size} the terrain would span {min:?} to {max:?}"
            ));
        }

        Ok(Some((min.map(|v| v as i32), max.map(|v| v as i32))))
    }

    /// The surface height and the texture ids at `x`, `z` (map units) with TrenchDot's edits applied.
    pub fn probe(&self, source: &dyn RegionSource, x: f64, z: f64) -> Probe {
        let grid = source.grid();
        let near = grid.vgrid_at(x, z);
        let regions: HashMap<Loc, Arc<Region3D>> =
            grid.regions_in([near[0] - 1, near[1] - 1], [near[0] + 2, near[1] + 2]).into_iter().filter_map(|l| Some((l, self.region(source, l)?))).collect();
        let find = |l: Loc| regions.get(&l).map(|r| r.as_ref());
        let h = surface_height(&grid, source.edge_blend(), find, (x / grid.upm) as f32, (z / grid.upm) as f32);
        let cell = grid.cell();
        let nearest = [(x / cell).round() as i64, (z / cell).round() as i64];
        let loc = grid.region_of(near);
        Probe {
            height: h.is_finite().then_some(h as f64 * grid.upm),
            region: find(loc).map(|_| loc),
            control: grid.sample(find, nearest).map(|(r, k)| Control(r.control[k])),
        }
    }

    /// The regions a send has to write, row by row from the north west. An edit that already matches the cached
    /// region is left out, which is also how an applied send shows once Godot has exported again.
    pub fn changed_regions(&self, source: &dyn RegionSource) -> Vec<RegionChange> {
        let size = source.grid().region_size;
        let mut out = Vec::new();
        for (loc, edit) in &self.edits {
            if edit.applied() || source.region(*loc).is_some_and(|base| same_bits(&base, &edit.region)) {
                continue;
            }

            out.push(RegionChange {
                loc: *loc,
                kind: if edit.base_hash.is_some() { ChangeKind::Edit } else { ChangeKind::Add },
                region: Some(edit.region.clone()),
                base_hash: edit.base_hash,
                conflict: source.hash(*loc) != edit.base_hash || edit.region.size != size,
            });
        }

        for (loc, hash) in &self.removed {
            let Some(now) = source.hash(*loc) else { continue };
            out.push(RegionChange { loc: *loc, kind: ChangeKind::Remove, region: None, base_hash: Some(*hash), conflict: now != *hash });
        }

        out.sort_by_key(|c| (c.loc[1], c.loc[0]));
        out
    }

    /// Takes in that Godot applied a send, on the current map and on every undo step alike. The edit that was sent
    /// becomes the applied state, other edits of the region that started where the send did now start from what Godot
    /// has, and a state from before the first edit gets the old region back as an edit, so an undo past the send still
    /// sends it back on the next save. Returns whether the region reads differently now.
    pub fn settle(&mut self, s: &Settled) -> bool {
        let now = Some(s.now);
        let sent = |r: &Arc<Region3D>| s.region.as_ref().is_some_and(|sent| Arc::ptr_eq(sent, r) || same_bits(sent, r));
        if let Some(edit) = self.edits.get_mut(&s.loc) {
            if sent(&edit.region) {
                edit.base_hash = now;
                edit.base = Some(edit.region.clone());
            } else if edit.base_hash == s.base_hash && s.region.is_some() {
                edit.base_hash = now;
                edit.base = s.region.clone();
            }

            return false;
        }

        if let Some(hash) = self.removed.get_mut(&s.loc) {
            if Some(*hash) == s.base_hash && s.region.is_some() {
                *hash = s.now;
            }

            return false;
        }

        let Some(region) = &s.region else { return false };
        match (s.base_hash, &s.before) {
            (None, _) => {
                self.removed.insert(s.loc, s.now);
            }
            (Some(_), Some(before)) => {
                self.edits.insert(s.loc, RegionEdit { region: before.clone(), base_hash: now, base: Some(region.clone()) });
            }
            (Some(_), None) => return false,
        }

        true
    }
}

fn same_bits(a: &Region3D, b: &Region3D) -> bool {
    a.loc == b.loc
        && a.size == b.size
        && a.control == b.control
        && a.color == b.color
        && a.heights.len() == b.heights.len()
        && a.heights.iter().zip(&b.heights).all(|(x, y)| x.to_bits() == y.to_bits())
}

impl Map {
    pub fn has_terrain3d(&self) -> bool {
        self.nodes.values().any(|n| matches!(n.kind, NodeKind::Terrain3D(_)))
    }

    pub fn terrain3d(&self, id: NodeId) -> Option<&Terrain3DLink> {
        match &self.get(id)?.kind {
            NodeKind::Terrain3D(link) => Some(link),
            _ => None,
        }
    }

    pub fn terrain3d_mut(&mut self, id: NodeId) -> Option<&mut Terrain3DLink> {
        match &mut self.get_mut(id)?.kind {
            NodeKind::Terrain3D(link) => Some(link),
            _ => None,
        }
    }
}

/// Puts the instances of every scatter set that follows the link back on its surface where one of the `changed`
/// regions lies under them. A set follows the link when its targets are exactly that node, the rule the Godot build
/// uses too. Instances over a hole or outside every region keep their height. Returns how many instances moved.
pub fn reground_scatter(map: &mut Map, link: NodeId, source: &dyn RegionSource, changed: &[Loc]) -> usize {
    let Some(terrain) = map.terrain3d(link) else { return 0 };
    if changed.is_empty() {
        return 0;
    }

    let grid = source.grid();
    let edge = source.edge_blend();
    let changed: HashSet<Loc> = changed.iter().copied().collect();
    // The edge blend looks at the regions around the one under an instance, which may itself only neighbor a changed one.
    let mut regions: HashMap<Loc, Arc<Region3D>> = HashMap::new();
    for c in &changed {
        for dy in -2..=2 {
            for dx in -2..=2 {
                let loc = [c[0] + dx, c[1] + dy];
                if !regions.contains_key(&loc)
                    && let Some(r) = terrain.region(source, loc)
                {
                    regions.insert(loc, r);
                }
            }
        }
    }

    let followers: Vec<NodeId> = map.scatters().filter(|(_, s)| s.targets == [link]).map(|(id, _)| id).collect();
    let mut moved = 0;
    for id in followers {
        let Some(set) = map.scatter(id) else { continue };
        let mut ys = Vec::new();
        for (i, inst) in set.instances.iter().enumerate() {
            let v = grid.vgrid_at(inst.position.x, inst.position.z);
            let under = [v, [v[0] + 1, v[1]], [v[0], v[1] + 1], [v[0] + 1, v[1] + 1]];
            if !under.iter().any(|v| changed.contains(&grid.region_of(*v))) {
                continue;
            }

            let (x, z) = (inst.position.x / grid.upm, inst.position.z / grid.upm);
            let h = surface_height(&grid, edge, |l| regions.get(&l).map(|r| r.as_ref()), x as f32, z as f32);
            let y = h as f64 * grid.upm;
            if h.is_finite() && y != inst.position.y {
                ys.push((i, y));
            }
        }

        if !ys.is_empty()
            && let Some(set) = map.scatter_mut(id)
        {
            moved += ys.len();
            for (i, y) in ys {
                set.instances[i].position.y = y;
            }
        }
    }

    moved
}

#[cfg(test)]
mod tests {
    use td_core::{Aabb, DVec3, NodeId};
    use td_geom::Brush;

    use super::*;
    use crate::format::{self, FormatError};
    use crate::issues;
    use crate::map::Group;
    use crate::ops::{self, DropPlan, EditOptions};
    use crate::{Document, Selection};

    const OPTS: EditOptions = EditOptions { uv_lock: true, grid: 0.0 };

    fn brush_map() -> (Map, NodeId) {
        let mut m = Map::new();
        m.properties.insert("classname".into(), "worldspawn".into());
        let layer = m.default_layer();
        let b = Brush::from_aabb(&Aabb::new(DVec3::new(-200.0, 8.0, 40.0), DVec3::new(-136.0, 72.0, 120.0)), "base/floor").unwrap();
        let brush = m.insert(layer, NodeKind::Brush(b));
        (m, brush)
    }

    /// A brush on the default layer and the link on a second layer, hidden, locked and labelled.
    fn linked_map() -> (Map, NodeId, NodeId) {
        let (mut m, brush) = brush_map();
        let layer = m.add_layer("Ground");
        let link = m.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://levels/terrain3d_data")));
        let node = m.get_mut(link).unwrap();
        node.hidden = true;
        node.locked = true;
        node.label = Some("Valley".into());
        (m, brush, link)
    }

    fn version(value: &serde_json::Value) -> u64 {
        value["version"].as_u64().unwrap()
    }

    fn select(ids: &[NodeId]) -> Selection {
        let mut sel = Selection::default();
        sel.nodes.extend(ids.iter().copied());
        sel
    }

    #[test]
    fn a_map_without_the_link_stays_version_1_in_both_layouts() {
        let (m, _) = brush_map();
        assert!(!m.has_terrain3d());
        assert_eq!(version(&format::to_value(&m)), 1);
        let bytes = format::to_bytes(&m).unwrap();
        assert_eq!(version(&format::file_json_value(&bytes).unwrap().0), 1);
        assert!(format::read_value_up_to(format::to_value(&m), 1).is_ok());
    }

    #[test]
    fn a_map_with_the_link_writes_version_2_and_reads_back() {
        let (m, _, link) = linked_map();
        assert!(m.has_terrain3d());
        let value = format::to_value(&m);
        assert_eq!(version(&value), 2);
        let node = &value["layers"][1]["children"][0];
        assert_eq!(node["type"], "terrain3d");
        assert_eq!(node["data_directory"], "res://levels/terrain3d_data");
        assert_eq!(node["id"], link.0);

        let bytes = format::to_bytes(&m).unwrap();
        assert_eq!(version(&format::file_json_value(&bytes).unwrap().0), 2);
        for loaded in [format::from_bytes(&bytes).unwrap().map, format::from_str(&format::to_string(&m)).unwrap()] {
            assert_eq!(format::to_value(&loaded), value);
            let n = loaded.get(link).unwrap();
            assert_eq!(n.kind, NodeKind::Terrain3D(Terrain3DLink::new("res://levels/terrain3d_data")));
            assert_eq!((n.hidden, n.locked, n.name()), (true, true, "Valley".to_string()));
            assert_eq!(n.parent, Some(loaded.layers[1]));
        }
    }

    #[test]
    fn the_last_release_refuses_a_linked_map_as_too_new() {
        let (m, _, _) = linked_map();
        assert!(matches!(format::read_value_up_to(format::to_value(&m), 1), Err(FormatError::TooNew(2))));
        let (value, _) = format::file_json_value(&format::to_bytes(&m).unwrap()).unwrap();
        assert!(matches!(format::read_value_up_to(value, 1), Err(FormatError::TooNew(2))));
    }

    #[test]
    fn the_clipboard_is_version_2_only_when_it_holds_the_link() {
        let (m, brush, link) = linked_map();
        let clip = |ids: &[NodeId]| version(&serde_json::from_str(&format::nodes_to_string(&m, ids)).unwrap());
        assert_eq!(clip(&[]), 1);
        assert_eq!(clip(&[brush]), 1);
        assert_eq!(clip(&[link]), 2);
        assert_eq!(clip(&[brush, link]), 2);
        assert_eq!(clip(&[m.layers[1]]), 2);

        let (mut other, _) = brush_map();
        let layer = other.default_layer();
        let pasted = format::paste_nodes(&mut other, layer, &format::nodes_to_string(&m, &[link])).unwrap();
        assert_eq!(pasted.len(), 1);
        assert_eq!(other.get(pasted[0]).unwrap().kind, m.get(link).unwrap().kind);
    }

    #[test]
    fn the_link_survives_undo_and_a_save_and_load() {
        let (m, _) = brush_map();
        let mut doc = Document::from_map(m, None);
        let layer = doc.map.default_layer();
        let link = doc.edit("Link Terrain3D", |m, _| m.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://t3d"))));
        let linked = format::to_value(&doc.map);
        doc.undo();
        assert!(!doc.map.has_terrain3d());
        assert_eq!(version(&format::to_value(&doc.map)), 1);
        doc.redo();
        assert_eq!(format::to_value(&doc.map), linked);

        let dir = std::env::temp_dir().join(format!("td_terrain3d_link_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("linked.tdm");
        format::save(&doc.map, &path).unwrap();
        let loaded = format::load(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(loaded.problems.is_empty());
        assert_eq!(format::to_value(&loaded.map), linked);
        assert_eq!(loaded.map.get(link).unwrap().kind, NodeKind::Terrain3D(Terrain3DLink::new("res://t3d")));
    }

    #[test]
    fn moving_and_rotating_leave_the_link_alone() {
        let (mut m, brush, link) = linked_map();
        let before_link = m.get(link).cloned();
        let before_brush = m.bounds(brush);
        let sel = select(&[brush, link]);
        assert_eq!(sel.transformables(&m), vec![brush]);
        assert!(select(&[m.layers[1]]).transformables(&m).is_empty());

        ops::translate_selection(&mut m, &sel, DVec3::new(32.0, -16.0, 8.0), OPTS);
        assert_eq!(m.bounds(brush).min, before_brush.min + DVec3::new(32.0, -16.0, 8.0));
        ops::transform_selection(&mut m, &sel, &ops::rotation_about(DVec3::new(10.0, 0.0, -30.0), DVec3::Y, 30.0), OPTS);
        assert_eq!(m.get(link).cloned(), before_link);
    }

    #[test]
    fn duplicating_skips_the_link() {
        let (mut m, brush, link) = linked_map();
        let count = m.nodes.len();
        let mut sel = select(&[link]);
        assert!(ops::duplicate_selection(&mut m, &mut sel, DVec3::ZERO, OPTS).is_empty());
        assert_eq!(m.nodes.len(), count);

        let mut sel = select(&[brush, link]);
        let copies = ops::duplicate_selection(&mut m, &mut sel, DVec3::new(64.0, 0.0, 0.0), OPTS);
        assert_eq!(copies.len(), 1);
        assert!(m.get(copies[0]).unwrap().brush().is_some());
        assert_eq!(m.nodes.len(), count + 1);
        assert_eq!(m.nodes.values().filter(|n| n.kind.is_pinned()).count(), 1);
        let layer = m.default_layer();
        assert_eq!(m.duplicate_subtree(link, layer), None);
    }

    #[test]
    fn grouping_leaves_the_link_in_its_layer() {
        let (mut m, brush, link) = linked_map();
        let layer = m.default_layer();
        assert_eq!(ops::group_selection(&mut m, &mut select(&[link]), "g", layer), None);
        assert_eq!(m.get(link).unwrap().parent, Some(m.layers[1]));

        let group = ops::group_selection(&mut m, &mut select(&[brush, link]), "g", layer).unwrap();
        assert_eq!(m.get(group).unwrap().children, vec![brush]);
        assert_eq!(m.get(link).unwrap().parent, Some(m.layers[1]));
    }

    #[test]
    fn dropping_moves_the_link_only_between_layers() {
        let (mut m, brush, link) = linked_map();
        m.get_mut(link).unwrap().locked = false;
        let layer = m.default_layer();
        let group = m.insert(layer, NodeKind::Group(Group::new("room")));
        let inner = m.insert(group, NodeKind::Brush(m.brush(brush).unwrap().clone()));
        let yes = |_| true;

        assert_eq!(ops::drop_plan(&m, &[link], layer, yes), Some(DropPlan::Into(layer)));
        assert_eq!(ops::drop_plan(&m, &[link], group, yes), None);
        assert_eq!(ops::drop_plan(&m, &[link], inner, yes), None);
        assert_eq!(ops::drop_plan(&m, &[link], brush, yes), None);
        assert_eq!(ops::drop_plan(&m, &[link, brush], m.layers[1], yes), Some(DropPlan::Into(m.layers[1])));
        assert_eq!(ops::drop_plan(&m, &[brush], link, yes), None);
        assert_eq!(ops::drop_plan(&m, &[brush], inner, yes), Some(DropPlan::Into(group)));
    }

    #[test]
    fn holds_pinned_looks_inside_containers() {
        let (mut m, brush, link) = linked_map();
        assert!(!ops::holds_pinned(&m, &[]));
        assert!(!ops::holds_pinned(&m, &[brush]));
        assert!(ops::holds_pinned(&m, &[link]));
        assert!(ops::holds_pinned(&m, &[brush, link]));
        assert!(ops::holds_pinned(&m, &[m.layers[1]]));
        let layer = m.default_layer();
        let group = m.insert(layer, NodeKind::Group(Group::new("hand edited")));
        m.reparent(link, group);
        assert!(ops::holds_pinned(&m, &[group]));
    }

    type Found = (&'static str, Option<NodeId>, Vec<(&'static str, String)>);

    fn found(m: &Map) -> Vec<Found> {
        let mut out: Vec<Found> = issues::check(m).into_iter().filter(|i| i.code.starts_with("terrain3d")).map(|i| (i.code, i.node, i.args)).collect();
        out.sort();
        out
    }

    #[test]
    fn one_link_with_a_folder_in_a_layer_has_no_issues() {
        let (m, _, _) = linked_map();
        assert!(found(&m).is_empty());
    }

    #[test]
    fn an_empty_folder_is_an_error() {
        for folder in ["", "   "] {
            let (mut m, _, link) = linked_map();
            m.get_mut(link).unwrap().kind = NodeKind::Terrain3D(Terrain3DLink::new(folder));
            assert_eq!(found(&m), vec![("terrain3d_no_folder", Some(link), vec![])]);
        }
    }

    #[test]
    fn a_link_inside_a_group_or_entity_is_an_error() {
        let (mut m, _, link) = linked_map();
        let ground = m.layers[1];
        let group = m.insert(ground, NodeKind::Group(Group::new("hand edited")));
        m.reparent(link, group);
        assert_eq!(found(&m), vec![("terrain3d_nested", Some(link), vec![])]);

        let entity = m.insert(ground, NodeKind::Entity(crate::Entity::new("func_group")));
        m.reparent(link, entity);
        assert_eq!(found(&m), vec![("terrain3d_nested", Some(link), vec![])]);
    }

    #[test]
    fn every_link_after_the_first_is_an_error() {
        let (mut m, _, first) = linked_map();
        let layer = m.default_layer();
        let second = m.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://other")));
        let other = vec![("other", first.to_string())];
        assert_eq!(found(&m), vec![("terrain3d_second", Some(second), other.clone())]);

        let ground = m.layers[1];
        let third = m.insert(ground, NodeKind::Terrain3D(Terrain3DLink::new("res://levels/terrain3d_data")));
        assert_eq!(found(&m), vec![("terrain3d_second", Some(second), other.clone()), ("terrain3d_second", Some(third), other)]);
        let loaded = format::from_bytes(&format::to_bytes(&m).unwrap()).unwrap().map;
        assert_eq!(loaded.nodes.values().filter(|n| n.kind.is_pinned()).count(), 3);
    }
}

#[cfg(test)]
mod edit_tests {
    use td_geom::terrain3d::Control;

    use super::*;
    use crate::scatter::{Scatter, ScatterInstance, ScatterItem, ScatterKind};
    use crate::terrain::falloff;
    use crate::{Document, format};

    const SIZE: u32 = 64;
    const SPACING: f64 = 0.5;
    const UPM: f64 = 32.0;
    const CELL: f64 = SPACING * UPM;

    fn ground(v: [i64; 2]) -> f32 {
        3.0 + 0.05 * v[0] as f32 - 0.02 * v[1] as f32
    }

    fn control() -> u32 {
        Control(0).with_base(2).with_overlay(5).with_blend(40).with_uv_angle(3).with_uv_scale(2).with_reserved(9).with_navigation(true).with_auto(true).0
    }

    fn grid() -> Grid {
        Grid { region_size: SIZE, spacing: SPACING, upm: UPM }
    }

    fn slanted(loc: Loc) -> Region3D {
        let mut r = Region3D::blank(loc, SIZE);
        for pz in 0..SIZE {
            for px in 0..SIZE {
                let k = r.index([px, pz]);
                r.heights[k] = ground(grid().vgrid_of(loc, [px, pz]));
                r.control[k] = control();
            }
        }

        r
    }

    fn hash(loc: Loc, salt: u8) -> RegionHash {
        let mut h = [salt; 32];
        h[0] = loc[0] as u8;
        h[1] = loc[1] as u8;
        h
    }

    struct Fake {
        grid: Grid,
        regions: HashMap<Loc, (Arc<Region3D>, RegionHash)>,
    }

    impl Fake {
        fn new(locs: &[Loc]) -> Self {
            Self { grid: grid(), regions: locs.iter().map(|l| (*l, (Arc::new(slanted(*l)), hash(*l, 1)))).collect() }
        }

        /// What Godot exports after the region changed on its side.
        fn export(&mut self, region: Region3D, salt: u8) {
            self.regions.insert(region.loc, (Arc::new(region.clone()), hash(region.loc, salt)));
        }
    }

    impl RegionSource for Fake {
        fn grid(&self) -> Grid {
            self.grid
        }

        fn edge_blend(&self) -> Option<EdgeBlend> {
            None
        }

        fn locs(&self) -> Vec<Loc> {
            self.regions.keys().copied().collect()
        }

        fn hash(&self, loc: Loc) -> Option<RegionHash> {
            self.regions.get(&loc).map(|r| r.1)
        }

        fn region(&self, loc: Loc) -> Option<Arc<Region3D>> {
            self.regions.get(&loc).map(|r| r.0.clone())
        }
    }

    fn brush(mode: SculptMode, radius: f64, strength: f64) -> SculptBrush {
        SculptBrush { mode, radius, strength, ..SculptBrush::default() }
    }

    fn vertices(loc: Loc) -> impl Iterator<Item = ([u32; 2], [i64; 2])> {
        (0..SIZE).flat_map(move |pz| (0..SIZE).map(move |px| ([px, pz], grid().vgrid_of(loc, [px, pz]))))
    }

    fn distance(v: [i64; 2], center: DVec3) -> f64 {
        (grid().position(v) - DVec2::new(center.x, center.z)).length()
    }

    /// The height after one Raise dab of `amount` map units, worked out the way the window and the heightfield do it.
    fn raised(v: [i64; 2], center: DVec3, radius: f64, amount: f64) -> f32 {
        let w = falloff(distance(v, center), radius);
        let map = (ground(v) as f64 * UPM) as f32;
        if w > 0.0 { ((map + (amount * w) as f32) as f64 / UPM) as f32 } else { ground(v) }
    }

    fn assert_regions(link: &Terrain3DLink, source: &Fake, locs: &[Loc], height: impl Fn([i64; 2]) -> f32, control: impl Fn([i64; 2]) -> u32) {
        for &loc in locs {
            let r = link.region(source, loc).unwrap();
            for (pixel, v) in vertices(loc) {
                let k = r.index(pixel);
                assert_eq!(r.heights[k].to_bits(), height(v).to_bits(), "height at {v:?}");
                assert_eq!(r.control[k], control(v), "control at {v:?}");
            }
        }
    }

    #[test]
    fn a_dab_across_four_region_corners_raises_all_four_seamlessly() {
        let corner = [[-2, -1], [-1, -1], [-2, 0], [-1, 0]];
        let source = Fake::new(&[corner[0], corner[1], corner[2], corner[3], [3, 2]]);
        let mut link = Terrain3DLink::new("res://t3d");
        let center = DVec3::new(-1013.0, 77.0, 9.5);
        let changed = link.sculpt(&source, center, &brush(SculptMode::Raise, 100.0, 4.0));
        assert_eq!(changed, corner.to_vec());
        assert_eq!(link.edits.keys().copied().collect::<Vec<_>>(), vec![[-2, -1], [-2, 0], [-1, -1], [-1, 0]]);
        assert!(link.edits.values().all(|e| e.base_hash == Some(hash(e.region.loc, 1))));
        assert_regions(&link, &source, &corner, |v| raised(v, center, 100.0, 4.0), |_| control());
        assert_regions(&link, &source, &[[3, 2]], ground, |_| control());
    }

    #[test]
    fn a_dab_needs_a_region_under_it_and_stays_inside_the_regions_it_has() {
        let empty = Fake::new(&[]);
        let mut link = Terrain3DLink::new("res://t3d");
        assert!(link.sculpt(&empty, DVec3::new(40.0, 0.0, -70.0), &brush(SculptMode::Raise, 100.0, 4.0)).is_empty());
        assert!(link.edits.is_empty());

        let source = Fake::new(&[[3, 2]]);
        let center = DVec3::new(3.0 * 1024.0 + 20.0, 0.0, 2.0 * 1024.0 - 30.0);
        assert_eq!(link.sculpt(&source, center, &brush(SculptMode::Raise, 120.0, 6.0)), vec![[3, 2]]);
        assert_eq!(link.edits.keys().copied().collect::<Vec<_>>(), vec![[3, 2]]);
        assert_regions(&link, &source, &[[3, 2]], |v| raised(v, center, 120.0, 6.0), |_| control());
        assert!(link.region(&source, [3, 1]).is_none() && link.region(&source, [2, 2]).is_none());
    }

    #[test]
    fn layer_and_alpha_modes_leave_terrain3d_alone() {
        let source = Fake::new(&[[0, 0]]);
        let mut link = Terrain3DLink::new("res://t3d");
        for mode in [SculptMode::PaintLayer, SculptMode::PaintAlpha, SculptMode::EraseAlpha] {
            assert!(link.sculpt(&source, DVec3::new(300.0, 0.0, 300.0), &brush(mode, 100.0, 1.0)).is_empty());
        }

        assert!(link.edits.is_empty());
    }

    #[test]
    fn the_dabs_of_one_stroke_share_a_copy_and_an_undo_restores_the_region_arc() {
        let source = Fake::new(&[[0, 0]]);
        let center = DVec3::new(333.0, 0.0, 517.0);
        let raise = brush(SculptMode::Raise, 64.0, 2.0);

        let mut alone = Terrain3DLink::new("res://t3d");
        alone.sculpt(&source, center, &raise);
        let ptr = Arc::as_ptr(&alone.edits[&[0, 0]].region);
        alone.sculpt(&source, center, &raise);
        assert_eq!(Arc::as_ptr(&alone.edits[&[0, 0]].region), ptr);

        let mut map = Map::new();
        let layer = map.default_layer();
        let id = map.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://t3d")));
        let mut doc = Document::from_map(map, None);
        let dab = |m: &mut Map| m.terrain3d_mut(id).unwrap().sculpt(&source, center, &raise);
        doc.edit("Sculpt", |m, _| dab(m));
        let first = doc.map.terrain3d(id).unwrap().edits[&[0, 0]].region.clone();
        doc.edit("Sculpt", |m, _| dab(m));
        let second = doc.map.terrain3d(id).unwrap().edits[&[0, 0]].region.clone();
        assert!(!Arc::ptr_eq(&first, &second));
        let k = first.index([21, 32]);
        assert!(second.heights[k] > first.heights[k] && first.heights[k] > ground([21, 32]));

        doc.undo();
        assert!(Arc::ptr_eq(&doc.map.terrain3d(id).unwrap().edits[&[0, 0]].region, &first));
        doc.undo();
        assert!(doc.map.terrain3d(id).unwrap().edits.is_empty());
        doc.redo();
        assert!(Arc::ptr_eq(&doc.map.terrain3d(id).unwrap().edits[&[0, 0]].region, &first));
    }

    #[test]
    fn holes_flip_only_the_hole_bit_and_unhole_puts_the_region_back() {
        let source = Fake::new(&[[0, 0], [1, 0], [1, 1]]);
        let mut link = Terrain3DLink::new("res://t3d");
        let center = DVec3::new(1030.0, 50.0, 400.0);
        assert_eq!(link.sculpt(&source, center, &brush(SculptMode::Hole, 70.0, 1.0)), vec![[0, 0], [1, 0]]);
        let holed = |v: [i64; 2]| if distance(v, center) <= 70.0 { Control(control()).with_hole(true).0 } else { control() };
        assert_regions(&link, &source, &[[0, 0], [1, 0], [1, 1]], ground, holed);
        assert_eq!(link.changed_regions(&source).len(), 2);

        assert_eq!(link.sculpt(&source, center, &brush(SculptMode::Unhole, 70.0, 1.0)), vec![[0, 0], [1, 0]]);
        assert_regions(&link, &source, &[[0, 0], [1, 0]], ground, |_| control());
        assert!(link.changed_regions(&source).is_empty());
    }

    #[test]
    fn painting_a_texture_crosses_the_border_and_keeps_the_other_bits() {
        let source = Fake::new(&[[-1, 0], [0, 0]]);
        let mut link = Terrain3DLink::new("res://t3d");
        let center = DVec3::new(-10.0, 0.0, 650.0);
        assert_eq!(link.paint_texture(&source, center, 90.0, 7, PaintMode::Base, 1.0), vec![[-1, 0], [0, 0]]);
        let painted = |v: [i64; 2]| {
            let d = distance(v, center);
            if d <= 90.0 && falloff(d, 90.0) as f32 > 0.5 {
                Control(control()).with_base(7).with_overlay(7).with_blend(0).with_auto(false).0
            } else {
                control()
            }
        };
        assert_regions(&link, &source, &[[-1, 0], [0, 0]], ground, painted);
        assert!(link.paint_texture(&source, DVec3::new(5000.0, 0.0, 5000.0), 90.0, 7, PaintMode::Base, 1.0).is_empty());
    }

    #[test]
    fn a_sparse_negative_layout_lists_row_by_row() {
        let source = Fake::new(&[[15, 15], [3, -2], [-16, -16], [0, 0]]);
        let mut link = Terrain3DLink::new("res://t3d");
        assert_eq!(link.locs(&source), vec![[-16, -16], [3, -2], [0, 0], [15, 15]]);
        assert_eq!(link.bounds(&source), Some(([-16, -16], [15, 15])));
        assert_eq!(link.remove_regions(&source, &[[0, 0]]), vec![[0, 0]]);
        assert_eq!(link.add_regions(&source, &[[-16, 15]]), Ok(vec![[-16, 15]]));
        assert_eq!(link.locs(&source), vec![[-16, -16], [3, -2], [-16, 15], [15, 15]]);
        assert!(link.region(&source, [0, 0]).is_none());
        assert_eq!(Terrain3DLink::new("res://t3d").bounds(&Fake::new(&[])), None);
    }

    #[test]
    fn adding_regions_skips_taken_spots_and_refuses_the_whole_call_outside_the_map() {
        let source = Fake::new(&[[2, -3]]);
        let mut link = Terrain3DLink::new("res://t3d");
        assert_eq!(link.add_regions(&source, &[]), Ok(vec![]));
        assert_eq!(link.add_regions(&source, &[[-16, 0], [15, 0], [2, -3], [-16, 0]]), Ok(vec![[-16, 0], [15, 0]]));
        for loc in [[-16, 0], [15, 0]] {
            assert_eq!(link.edits[&loc], RegionEdit::new(Arc::new(Region3D::blank(loc, SIZE)), None));
        }

        for bad in [[-17, 0], [16, 0], [0, -17], [0, 16]] {
            let err = link.add_regions(&source, &[[1, 1], bad]).unwrap_err();
            assert!(err.contains(&format!("{bad:?}")), "{err}");
        }

        assert_eq!(link.locs(&source), vec![[2, -3], [-16, 0], [15, 0]]);
    }

    #[test]
    fn removing_remembers_cached_regions_and_forgets_added_ones() {
        let source = Fake::new(&[[2, -3], [4, -3]]);
        let mut link = Terrain3DLink::new("res://t3d");
        link.sculpt(&source, DVec3::new(2.0 * 1024.0 + 500.0, 0.0, -3.0 * 1024.0 + 500.0), &brush(SculptMode::Raise, 60.0, 3.0));
        link.add_regions(&source, &[[1, 1]]).unwrap();
        assert_eq!(link.remove_regions(&source, &[[2, -3], [9, 9], [1, 1], [4, -3]]), vec![[2, -3], [1, 1], [4, -3]]);
        assert!(link.edits.is_empty());
        assert_eq!(link.removed.iter().map(|(l, h)| (*l, *h)).collect::<Vec<_>>(), vec![([2, -3], hash([2, -3], 1)), ([4, -3], hash([4, -3], 1))]);
        assert!(link.remove_regions(&source, &[[2, -3]]).is_empty());

        assert_eq!(link.add_regions(&source, &[[2, -3]]), Ok(vec![[2, -3]]));
        assert_eq!(link.edits[&[2, -3]].base_hash, Some(hash([2, -3], 1)));
        let changes: Vec<_> = link.changed_regions(&source).into_iter().map(|c| (c.loc, c.kind, c.base_hash, c.conflict)).collect();
        assert_eq!(changes, vec![([2, -3], ChangeKind::Edit, Some(hash([2, -3], 1)), false), ([4, -3], ChangeKind::Remove, Some(hash([4, -3], 1)), false)]);
    }

    fn locs_in(min: Loc, max: Loc, skip: (Loc, Loc)) -> Vec<Loc> {
        let inside = |l: Loc| (skip.0[0]..=skip.1[0]).contains(&l[0]) && (skip.0[1]..=skip.1[1]).contains(&l[1]);
        (min[1]..=max[1]).flat_map(|y| (min[0]..=max[0]).map(move |x| [x, y])).filter(|l| !inside(*l)).collect()
    }

    #[test]
    fn growing_and_shrinking_each_side_keeps_the_gaps_of_a_sparse_layout() {
        let source = Fake::new(&[[-2, 1], [1, 3]]);
        let mut link = Terrain3DLink::new("res://t3d");
        let old = ([-2, 1], [1, 3]);
        let cases = [
            (Grow { north: 1, ..Grow::default() }, [-2, 0], [1, 3], vec![]),
            (Grow { south: 2, ..Grow::default() }, [-2, 1], [1, 5], vec![]),
            (Grow { west: 1, east: 1, ..Grow::default() }, [-3, 1], [2, 3], vec![]),
            (Grow { west: -1, ..Grow::default() }, [-1, 1], [1, 3], vec![[-2, 1]]),
            (Grow { east: -3, ..Grow::default() }, [-2, 1], [-2, 3], vec![[1, 3]]),
            (Grow { north: -2, ..Grow::default() }, [-2, 3], [1, 3], vec![[-2, 1]]),
            (Grow { south: -1, west: 1, ..Grow::default() }, [-3, 1], [1, 2], vec![[1, 3]]),
            (Grow::default(), [-2, 1], [1, 3], vec![]),
        ];
        for (grow, min, max, remove) in cases {
            let plan = link.plan_grow(&source, grow).unwrap();
            assert_eq!(plan, GrowPlan { min, max, add: locs_in(min, max, old), remove }, "{grow:?}");
        }

        assert!(link.edits.is_empty() && link.removed.is_empty());
        link.grow(&source, Grow { north: 1, west: -1, ..Grow::default() }).unwrap();
        assert_eq!(link.locs(&source), vec![[-1, 0], [0, 0], [1, 0], [1, 3]]);
        assert!(!link.has_region(&source, [0, 2]));
        assert_eq!(link.edits[&[0, 0]].region.as_ref(), &Region3D::blank([0, 0], SIZE));
        assert_eq!(link.removed.keys().copied().collect::<Vec<_>>(), vec![[-2, 1]]);
    }

    #[test]
    fn growing_stops_at_32_regions_per_side() {
        let source = Fake::new(&[[0, 0]]);
        let link = Terrain3DLink::new("res://t3d");
        let side = |west, east, north, south| Grow { north, south, west, east };
        for g in [side(16, 0, 0, 0), side(0, 15, 0, 0), side(0, 0, 16, 0), side(0, 0, 0, 15)] {
            assert!(link.plan_grow(&source, g).is_ok(), "{g:?}");
        }

        for g in [side(17, 0, 0, 0), side(0, 16, 0, 0), side(0, 0, 17, 0), side(0, 0, 0, 16)] {
            assert!(link.plan_grow(&source, g).unwrap_err().contains("32 regions"), "{g:?}");
        }

        assert_eq!(link.plan_grow(&source, side(16, 15, 16, 15)).unwrap().add.len(), 32 * 32 - 1);
        assert!(link.plan_grow(&source, side(i32::MAX, i32::MAX, 0, 0)).is_err());
        assert!(link.plan_grow(&source, side(-1, 0, 0, 0)).unwrap_err().contains("every"));
        assert!(link.plan_grow(&Fake::new(&[]), side(1, 0, 0, 0)).unwrap_err().contains("no regions"));
    }

    #[test]
    fn a_region_godot_changed_since_the_edit_is_a_conflict() {
        let mut source = Fake::new(&[[0, 0], [1, 0], [2, 0], [3, 0]]);
        let mut link = Terrain3DLink::new("res://t3d");
        let dab = |x: f64| DVec3::new(x, 0.0, 512.0);
        link.sculpt(&source, dab(500.0), &brush(SculptMode::Raise, 60.0, 3.0));
        link.sculpt(&source, dab(1524.0), &brush(SculptMode::Raise, 60.0, 3.0));
        link.remove_regions(&source, &[[2, 0], [3, 0]]);
        link.add_regions(&source, &[[5, 0], [6, 0]]).unwrap();

        let mut godot = slanted([0, 0]);
        godot.heights[9] += 1.0;
        source.export(godot, 2);
        source.export(link.edits[&[1, 0]].region.as_ref().clone(), 3);
        source.export(slanted([2, 0]), 4);
        source.regions.remove(&[3, 0]);
        source.export(slanted([5, 0]), 5);

        let changes: Vec<_> = link.changed_regions(&source).into_iter().map(|c| (c.loc, c.kind, c.base_hash, c.conflict, c.region.is_some())).collect();
        assert_eq!(
            changes,
            vec![
                ([0, 0], ChangeKind::Edit, Some(hash([0, 0], 1)), true, true),
                ([2, 0], ChangeKind::Remove, Some(hash([2, 0], 1)), true, false),
                ([5, 0], ChangeKind::Add, None, true, true),
                ([6, 0], ChangeKind::Add, None, false, true),
            ]
        );

        source.grid.region_size = 128;
        assert!(link.changed_regions(&source).iter().all(|c| c.conflict));
    }

    #[test]
    fn edits_never_reach_the_map_file() {
        let source = Fake::new(&[[0, 0]]);
        let mut map = Map::new();
        let layer = map.default_layer();
        let id = map.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://t3d")));
        let clean = format::to_value(&map);
        let link = map.terrain3d_mut(id).unwrap();
        link.sculpt(&source, DVec3::new(300.0, 0.0, 300.0), &brush(SculptMode::Raise, 60.0, 3.0));
        link.remove_regions(&source, &[[0, 0]]);
        link.add_regions(&source, &[[1, 1]]).unwrap();
        assert!(!link.edits.is_empty() && !link.removed.is_empty());
        assert_eq!(format::to_value(&map), clean);
        let loaded = format::from_bytes(&format::to_bytes(&map).unwrap()).unwrap().map;
        assert_eq!(loaded.terrain3d(id), Some(&Terrain3DLink::new("res://t3d")));
    }

    fn scatter_at(targets: Vec<NodeId>, points: &[DVec3]) -> NodeKind {
        let mut s = Scatter::new("Rocks", ScatterKind::Props, vec![ScatterItem::new("res://rock.glb")]);
        s.targets = targets;
        s.instances = points.iter().map(|p| ScatterInstance { item: 0, position: *p, angles: DVec3::ZERO, scale: 1.0 }).collect();
        NodeKind::Scatter(s)
    }

    #[test]
    fn regrounding_moves_only_followers_over_changed_ground() {
        let source = Fake::new(&[[0, 0], [1, 0]]);
        let mut map = Map::new();
        let layer = map.default_layer();
        let link = map.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://t3d")));
        let other = map.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://other")));
        let points = [DVec3::new(301.0, 5.0, 297.0), DVec3::new(1500.0, 5.0, 300.0), DVec3::new(600.0, 5.0, 100.0), DVec3::new(-500.0, 5.0, 300.0)];
        let follow = map.insert(layer, scatter_at(vec![link], &points));
        let both = map.insert(layer, scatter_at(vec![link, other], &points));
        let every = map.insert(layer, scatter_at(vec![], &points));
        let elsewhere = map.insert(layer, scatter_at(vec![other], &points));

        let t = map.terrain3d_mut(link).unwrap();
        let mut changed = t.sculpt(&source, DVec3::new(300.0, 0.0, 300.0), &brush(SculptMode::Raise, 200.0, 8.0));
        changed.extend(t.sculpt(&source, DVec3::new(600.0, 0.0, 100.0), &brush(SculptMode::Hole, 2.0 * CELL, 1.0)));
        assert_eq!(changed, vec![[0, 0], [0, 0]]);
        assert_eq!(reground_scatter(&mut map, link, &source, &[]), 0);
        assert_eq!(reground_scatter(&mut map, follow, &source, &changed), 0);

        let t = map.terrain3d(link).unwrap();
        let regions: HashMap<Loc, Arc<Region3D>> = [[0, 0], [1, 0]].into_iter().map(|l| (l, t.region(&source, l).unwrap())).collect();
        let y = surface_height(&grid(), None, |l| regions.get(&l).map(|r| r.as_ref()), (301.0 / UPM) as f32, (297.0 / UPM) as f32) as f64 * UPM;
        assert!(y > ground([19, 19]) as f64 * UPM + 1.0);

        assert_eq!(reground_scatter(&mut map, link, &source, &changed), 1);
        let positions = |m: &Map, id: NodeId| m.scatter(id).unwrap().instances.iter().map(|i| i.position).collect::<Vec<_>>();
        assert_eq!(positions(&map, follow), vec![DVec3::new(301.0, y, 297.0), points[1], points[2], points[3]]);
        for id in [both, every, elsewhere] {
            assert_eq!(positions(&map, id), points.to_vec());
        }

        assert_eq!(reground_scatter(&mut map, link, &source, &changed), 0);
    }

    const L: Loc = [2, -3];

    fn dab_on_l(link: &mut Terrain3DLink, source: &Fake, x: f64) -> Vec<Loc> {
        link.sculpt(source, DVec3::new(2.0 * 1024.0 + x, 0.0, -3.0 * 1024.0 + 300.0), &brush(SculptMode::Raise, 60.0, 3.0))
    }

    /// What Godot exports for a region it applied: the same terrain, but its round trip leaves other bits.
    fn round_trip(region: &Region3D) -> Region3D {
        let mut r = region.clone();
        r.heights[7] = f32::from_bits(r.heights[7].to_bits() + 1);
        r
    }

    /// Godot applying every change of the link, the way the editor learns of it from the manifest.
    fn apply_in_godot(link: &Terrain3DLink, source: &mut Fake, salt: u8) -> Vec<Settled> {
        let mut out = Vec::new();
        for change in link.changed_regions(source) {
            let region = change.region.clone().unwrap();
            source.export(round_trip(&region), salt);
            let before = link.edits[&change.loc].base.clone();
            out.push(Settled { loc: change.loc, region: Some(region), base_hash: change.base_hash, before, now: hash(change.loc, salt) });
        }

        out
    }

    fn changes(link: &Terrain3DLink, source: &Fake) -> Vec<(Loc, ChangeKind, Option<RegionHash>, bool)> {
        link.changed_regions(source).into_iter().map(|c| (c.loc, c.kind, c.base_hash, c.conflict)).collect()
    }

    #[test]
    fn an_applied_edit_sends_nothing_and_the_next_stroke_starts_from_godots_hash() {
        let mut source = Fake::new(&[L, [3, -3]]);
        let mut link = Terrain3DLink::new("res://t3d");
        assert_eq!(dab_on_l(&mut link, &source, 400.0), vec![L]);
        let sent = apply_in_godot(&link, &mut source, 2);
        assert_eq!(sent.len(), 1);
        assert!(!link.settle(&sent[0]), "the region looks the same as before");
        assert!(changes(&link, &source).is_empty(), "Godot holds the edit");
        assert!(Arc::ptr_eq(&link.region(&source, L).unwrap(), sent[0].region.as_ref().unwrap()));

        dab_on_l(&mut link, &source, 700.0);
        assert_eq!(changes(&link, &source), vec![(L, ChangeKind::Edit, Some(hash(L, 2)), false)]);
        assert_eq!(apply_in_godot(&link, &mut source, 3).len(), 1, "the second stroke is sent like the first");
    }

    #[test]
    fn added_regions_godot_accepted_are_no_conflict_for_later_sends() {
        let mut source = Fake::new(&[L]);
        let mut link = Terrain3DLink::new("res://t3d");
        link.add_regions(&source, &[[5, 1], [6, 1]]).unwrap();
        for s in apply_in_godot(&link, &mut source, 4) {
            link.settle(&s);
        }

        assert!(changes(&link, &source).is_empty());
        link.add_regions(&source, &[[7, 1]]).unwrap();
        assert_eq!(changes(&link, &source), vec![([7, 1], ChangeKind::Add, None, false)]);
        link.sculpt(&source, DVec3::new(5.0 * 1024.0 + 500.0, 0.0, 1024.0 + 500.0), &brush(SculptMode::Raise, 60.0, 3.0));
        assert_eq!(changes(&link, &source), vec![([5, 1], ChangeKind::Edit, Some(hash([5, 1], 4)), false), ([7, 1], ChangeKind::Add, None, false)]);
    }

    #[test]
    fn an_applied_edit_gives_way_when_godot_changes_the_region_but_an_unsent_one_conflicts() {
        let mut source = Fake::new(&[L, [3, -3]]);
        let mut link = Terrain3DLink::new("res://t3d");
        dab_on_l(&mut link, &source, 400.0);
        link.sculpt(&source, DVec3::new(3.0 * 1024.0 + 500.0, 0.0, -3.0 * 1024.0 + 500.0), &brush(SculptMode::Raise, 60.0, 3.0));
        let sent = apply_in_godot(&link, &mut source, 2);
        link.settle(&sent[0]);
        assert_eq!(changes(&link, &source).len(), 1, "only the first region was settled");

        let mut reloaded = slanted(L);
        reloaded.heights[3] = 40.0;
        source.export(reloaded.clone(), 5);
        let mut godots = slanted([3, -3]);
        godots.heights[3] = 41.0;
        source.export(godots, 5);
        assert!(same_bits(&link.region(&source, L).unwrap(), &reloaded), "the cache shows once Godot changed the region");
        assert!(link.has_region(&source, L));
        assert_eq!(changes(&link, &source), vec![([3, -3], ChangeKind::Edit, Some(hash([3, -3], 1)), true)]);

        dab_on_l(&mut link, &source, 400.0);
        assert_eq!(link.edits[&L].base_hash, Some(hash(L, 5)));
        assert_eq!(link.region(&source, L).unwrap().heights[3], 40.0, "the stroke started from what Godot has");
        assert_eq!(changes(&link, &source), vec![(L, ChangeKind::Edit, Some(hash(L, 5)), false), ([3, -3], ChangeKind::Edit, Some(hash([3, -3], 1)), true)]);
    }

    #[test]
    fn a_smaller_region_size_refuses_a_span_past_32_regions_per_side() {
        let mut source = Fake::new(&[[-8, -8], [7, 7]]);
        source.grid.region_size = 256;
        let link = Terrain3DLink::new("res://t3d");
        assert_eq!(link.resized_bounds(&source, 128), Ok(Some(([-16, -16], [15, 15]))));
        assert_eq!(link.resized_bounds(&source, 512), Ok(Some(([-4, -4], [3, 3]))));
        let err = link.resized_bounds(&source, 64).unwrap_err();
        assert!(err.contains("at most 32 regions") && err.contains("[-32, -32] to [31, 31]"), "{err}");
        assert_eq!(Terrain3DLink::new("res://t3d").resized_bounds(&Fake::new(&[]), 64), Ok(None));
    }

    #[test]
    fn a_region_godot_removed_after_applying_an_edit_leaves_the_locs() {
        let mut source = Fake::new(&[L, [3, -3]]);
        let mut link = Terrain3DLink::new("res://t3d");
        dab_on_l(&mut link, &source, 400.0);
        link.settle(&apply_in_godot(&link, &mut source, 2)[0]);
        assert!(link.locs(&source).contains(&L));

        source.regions.remove(&L);
        assert_eq!(link.locs(&source), vec![[3, -3]]);
        assert!(!link.has_region(&source, L));
        assert_eq!(link.bounds(&source), Some(([3, -3], [3, -3])));
    }

    #[test]
    fn undo_past_an_applied_send_brings_the_old_region_back_to_send() {
        let mut source = Fake::new(&[L]);
        let mut map = Map::new();
        let layer = map.default_layer();
        let id = map.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://t3d")));
        let mut doc = Document::from_map(map, None);
        doc.edit("Sculpt", |m, _| dab_on_l(m.terrain3d_mut(id).unwrap(), &source, 400.0));
        doc.edit("Sculpt", |m, _| dab_on_l(m.terrain3d_mut(id).unwrap(), &source, 600.0));
        doc.mark_saved();
        let sent = apply_in_godot(doc.map.terrain3d(id).unwrap(), &mut source, 2);
        assert!(!doc.rewrite_history(|m| m.terrain3d_mut(id).is_some_and(|l| l.settle(&sent[0]))));
        assert!(!doc.is_modified());
        assert!(changes(doc.map.terrain3d(id).unwrap(), &source).is_empty());

        doc.undo();
        assert_eq!(changes(doc.map.terrain3d(id).unwrap(), &source), vec![(L, ChangeKind::Edit, Some(hash(L, 2)), false)], "one stroke back");
        doc.undo();
        let link = doc.map.terrain3d(id).unwrap();
        assert!(same_bits(&link.region(&source, L).unwrap(), &slanted(L)), "the region as it was before the first stroke");
        assert_eq!(changes(link, &source), vec![(L, ChangeKind::Edit, Some(hash(L, 2)), false)]);

        doc.redo();
        doc.redo();
        assert!(changes(doc.map.terrain3d(id).unwrap(), &source).is_empty(), "redo is the applied state again");
    }

    #[test]
    fn undoing_an_added_region_after_godot_took_it_removes_it_there() {
        let mut source = Fake::new(&[L]);
        let mut map = Map::new();
        let layer = map.default_layer();
        let id = map.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new("res://t3d")));
        let mut doc = Document::from_map(map, None);
        doc.edit("Add", |m, _| m.terrain3d_mut(id).unwrap().add_regions(&source, &[[-4, 2]]).unwrap());
        let sent = apply_in_godot(doc.map.terrain3d(id).unwrap(), &mut source, 6);
        doc.undo();
        assert!(doc.rewrite_history(|m| m.terrain3d_mut(id).is_some_and(|l| l.settle(&sent[0]))), "the current map lost the region");
        assert!(doc.is_modified());
        assert_eq!(changes(doc.map.terrain3d(id).unwrap(), &source), vec![([-4, 2], ChangeKind::Remove, Some(hash([-4, 2], 6)), false)]);
        doc.redo();
        assert!(changes(doc.map.terrain3d(id).unwrap(), &source).is_empty());
    }

    #[test]
    fn settling_a_removal_or_another_send_leaves_unrelated_edits_alone() {
        let source = Fake::new(&[L, [3, -3]]);
        let mut link = Terrain3DLink::new("res://t3d");
        dab_on_l(&mut link, &source, 400.0);
        let before = link.clone();
        let other = Settled { loc: L, region: Some(Arc::new(slanted(L))), base_hash: Some(hash(L, 9)), before: None, now: hash(L, 2) };
        assert!(!link.settle(&other));
        assert_eq!(link, before, "an edit from another base is not this send");
        let removal = Settled { loc: [3, -3], region: None, base_hash: Some(hash([3, -3], 1)), before: None, now: hash([3, -3], 2) };
        assert!(!link.settle(&removal));
        assert_eq!(link, before);
    }
}
