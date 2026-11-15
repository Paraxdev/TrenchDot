//! The editor's view of the Terrain3D caches Godot exports. Each manifest is polled once a second, regions load on a
//! worker thread under a memory budget, and the LOD pyramids of regions too far away to load whole are built beside them.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use td_doc::terrain3d::RegionSource;
use td_formats::terrain3d_cache::{self, Applied, CachedRegion, Lod, LodLevel, Manifest, RegionHash};
use td_geom::terrain3d::{EdgeBlend, Grid, Loc, Region3D};
use td_geom::terrain3d_tile;

const POLL: Duration = Duration::from_secs(1);
/// What the LRU keeps of full regions before it lets go of the ones nobody asks for, 1.5 GiB.
pub const DEFAULT_BUDGET: usize = 3 << 29;
/// Terrain3D's `world_background` from this value on (Flat, Noise) blends heights toward the ground level near region edges.
const BACKGROUND_FLAT: u32 = 1;

struct Loaded {
    region: Arc<Region3D>,
    hash: RegionHash,
    used: Cell<u64>,
}

#[derive(Default)]
struct Folder {
    manifest: Option<Manifest>,
    text: String,
    error: Option<String>,
    checked: Option<Instant>,
    /// Goes up whenever the manifest or a loaded region changes.
    generation: u64,
    regions: HashMap<Loc, Loaded>,
    lods: HashMap<Loc, (RegionHash, Arc<Lod>)>,
    want_full: BTreeSet<Loc>,
    want_lod: BTreeSet<Loc>,
    inflight: HashSet<(Loc, bool)>,
    /// What failed to load and the hash it failed with, so a broken file is not asked for again every frame.
    failed: HashMap<(Loc, bool), RegionHash>,
    /// Regions whose hash changed, appeared or went since the last `take_changed`.
    changed: BTreeSet<Loc>,
    /// Like `changed`, but without what the first read of the manifest found.
    news: BTreeSet<Loc>,
    read_before: bool,
    seen_seq: Option<u64>,
    applied: Vec<Applied>,
    refused: BTreeMap<Loc, String>,
}

enum Job {
    Region { cache: PathBuf, loc: Loc, size: u32, hash: RegionHash },
    Lod { cache: PathBuf, loc: Loc, size: u32, hash: RegionHash, grid: Grid },
}

enum Done {
    Region { cache: PathBuf, loc: Loc, size: u32, hash: RegionHash, result: Result<CachedRegion, String> },
    Lod { cache: PathBuf, loc: Loc, size: u32, hash: RegionHash, result: Result<Lod, String> },
}

pub struct Terrain3DStore {
    folders: HashMap<PathBuf, Folder>,
    budget: usize,
    /// Map units per meter, the scale the LOD pyramids are built with.
    pub upm: f64,
    tick: Cell<u64>,
    jobs: Option<Sender<Job>>,
    done: Option<Receiver<Done>>,
    repaint: Option<egui::Context>,
}

impl Default for Terrain3DStore {
    fn default() -> Self {
        Self::new(None)
    }
}

impl Terrain3DStore {
    /// `repaint` is asked to redraw whenever the worker finished a region.
    pub fn new(repaint: Option<egui::Context>) -> Self {
        Self { folders: HashMap::new(), budget: DEFAULT_BUDGET, upm: 32.0, tick: Cell::new(0), jobs: None, done: None, repaint }
    }

    pub fn with_budget(mut self, bytes: usize) -> Self {
        self.budget = bytes;
        self
    }

    /// Looks at the folder's manifest (at most once a second, `recheck` forces it), takes in what the worker finished and
    /// starts what is wanted but not loaded. True when something about the folder changed.
    pub fn refresh(&mut self, cache: &Path) -> bool {
        let before = self.generation(cache);
        self.drain();
        self.folders.entry(cache.to_path_buf()).or_default();
        self.poll_manifest(cache);
        self.pump(cache);
        self.generation(cache) != before
    }

    /// Makes the next `refresh` of every folder read its manifest.
    pub fn recheck(&mut self) {
        self.folders.values_mut().for_each(|f| f.checked = None);
    }

    pub fn forget(&mut self, cache: &Path) {
        self.folders.remove(cache);
    }

    pub fn generation(&self, cache: &Path) -> u64 {
        self.folders.get(cache).map_or(0, |f| f.generation)
    }

    pub fn manifest(&self, cache: &Path) -> Option<&Manifest> {
        self.folders.get(cache)?.manifest.as_ref()
    }

    /// Why the manifest or a region could not be read, while the last good state stays in use.
    pub fn error(&self, cache: &Path) -> Option<&str> {
        self.folders.get(cache)?.error.as_deref()
    }

    pub fn grid(&self, cache: &Path) -> Option<Grid> {
        let m = self.manifest(cache)?;
        Some(Grid { region_size: m.region_size, spacing: m.vertex_spacing, upm: self.upm })
    }

    /// The regions to hold whole and the ones a pyramid is enough for. They load in the background, the ones nobody wants
    /// any more stay until the budget needs their memory.
    pub fn want(&mut self, cache: &Path, full: &[Loc], lod: &[Loc]) {
        let folder = self.folders.entry(cache.to_path_buf()).or_default();
        folder.want_full = full.iter().copied().collect();
        folder.want_lod = lod.iter().copied().filter(|l| !folder.want_full.contains(l)).collect();
        self.evict();
        self.pump(cache);
    }

    /// A region held whole, None while it loads or when nobody asked for it.
    pub fn region(&self, cache: &Path, loc: Loc) -> Option<Arc<Region3D>> {
        let loaded = self.folders.get(cache)?.regions.get(&loc)?;
        self.tick.set(self.tick.get() + 1);
        loaded.used.set(self.tick.get());
        Some(loaded.region.clone())
    }

    pub fn lod(&self, cache: &Path, loc: Loc) -> Option<Arc<Lod>> {
        Some(self.folders.get(cache)?.lods.get(&loc)?.1.clone())
    }

    /// The regions whose cache changed since the last call, for the views that draw them.
    pub fn take_changed(&mut self, cache: &Path) -> Vec<Loc> {
        self.folders.get_mut(cache).map(|f| std::mem::take(&mut f.changed).into_iter().collect()).unwrap_or_default()
    }

    /// The regions that changed on disk since the last call, for what must follow them. The regions the first read of the
    /// folder found are not news, so opening a map changes nothing.
    pub fn take_news(&mut self, cache: &Path) -> Vec<Loc> {
        self.folders.get_mut(cache).map(|f| std::mem::take(&mut f.news).into_iter().collect()).unwrap_or_default()
    }

    /// What Godot reported applying since the last call.
    pub fn take_applied(&mut self, cache: &Path) -> Vec<Applied> {
        self.folders.get_mut(cache).map(|f| std::mem::take(&mut f.applied)).unwrap_or_default()
    }

    /// Regions Godot refused to apply and why, until a later apply accepts them or `forget_refusal` drops them.
    pub fn refused(&self, cache: &Path) -> BTreeMap<Loc, String> {
        self.folders.get(cache).map(|f| f.refused.clone()).unwrap_or_default()
    }

    /// Drops the refusals for unsaved changes, which went with the Godot editor that had them.
    pub fn forget_unsaved_refusals(&mut self, cache: &Path) {
        if let Some(f) = self.folders.get_mut(cache) {
            f.refused.retain(|_, why| why != crate::terrain3d::bridge::DIRTY);
        }
    }

    pub fn forget_refusal(&mut self, cache: &Path, loc: Loc) {
        if let Some(f) = self.folders.get_mut(cache) {
            f.refused.remove(&loc);
        }
    }

    pub fn dirty(&self, cache: &Path, loc: Loc) -> bool {
        self.manifest(cache).is_some_and(|m| m.dirty.contains(&loc))
    }

    /// Bytes of the regions held whole across every folder.
    pub fn loaded_bytes(&self) -> usize {
        self.folders.values().flat_map(|f| f.regions.values()).map(|l| bytes_of(&l.region)).sum()
    }

    pub fn source(&self, cache: &Path) -> Option<FolderSource<'_>> {
        let folder = self.folders.get(cache)?;
        let manifest = folder.manifest.as_ref()?;
        Some(FolderSource { folder, grid: Grid { region_size: manifest.region_size, spacing: manifest.vertex_spacing, upm: self.upm } })
    }

    fn poll_manifest(&mut self, cache: &Path) {
        let folder = self.folders.get_mut(cache).expect("folder");
        if folder.checked.is_some_and(|t| t.elapsed() < POLL) {
            return;
        }

        folder.checked = Some(Instant::now());
        let text = match std::fs::read_to_string(terrain3d_cache::manifest_path(cache)) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if folder.manifest.take().is_some() {
                    folder.text.clear();
                    folder.error = None;
                    folder.changed.extend(folder.regions.keys().copied());
                    folder.regions.clear();
                    folder.lods.clear();
                    folder.inflight.clear();
                    folder.generation += 1;
                }

                return;
            }
            Err(e) => {
                folder.error = Some(e.to_string());
                return;
            }
        };

        if text == folder.text && folder.error.is_none() {
            return;
        }

        folder.text = text;
        match terrain3d_cache::parse_manifest(&folder.text) {
            Ok(manifest) => {
                // A region that failed to load is tried again once the manifest reads clean.
                if folder.error.take().is_some() {
                    folder.failed.clear();
                }

                apply_manifest(folder, manifest);
            }
            Err(e) => folder.error = Some(e.to_string()),
        }
    }

    fn sender(&mut self) -> Sender<Job> {
        self.jobs
            .get_or_insert_with(|| {
                let (job_tx, job_rx) = mpsc::channel();
                let (done_tx, done_rx) = mpsc::channel();
                let repaint = self.repaint.clone();
                std::thread::Builder::new().name("terrain3d-loader".into()).spawn(move || worker(job_rx, done_tx, repaint)).expect("spawn terrain3d loader");
                self.done = Some(done_rx);
                job_tx
            })
            .clone()
    }

    fn pump(&mut self, cache: &Path) {
        let Some(grid) = self.grid(cache) else { return };
        let Some(folder) = self.folders.get_mut(cache) else { return };
        let Some(manifest) = folder.manifest.as_ref() else { return };
        let hashes: HashMap<Loc, RegionHash> = manifest.regions.iter().map(|r| (r.loc, r.hash)).collect();
        let mut jobs = Vec::new();
        for &loc in &folder.want_full {
            let Some(&hash) = hashes.get(&loc) else { continue };
            if !folder.regions.contains_key(&loc) && folder.failed.get(&(loc, true)) != Some(&hash) && folder.inflight.insert((loc, true)) {
                jobs.push(Job::Region { cache: cache.to_path_buf(), loc, size: grid.region_size, hash });
            }
        }

        for &loc in &folder.want_lod {
            let Some(&hash) = hashes.get(&loc) else { continue };
            let current = folder.lods.get(&loc).is_some_and(|(h, _)| *h == hash);
            if !current && folder.failed.get(&(loc, false)) != Some(&hash) && folder.inflight.insert((loc, false)) {
                jobs.push(Job::Lod { cache: cache.to_path_buf(), loc, size: grid.region_size, hash, grid });
            }
        }

        if jobs.is_empty() {
            return;
        }

        let sender = self.sender();
        for job in jobs {
            let _ = sender.send(job);
        }
    }

    fn drain(&mut self) {
        let Some(done) = &self.done else { return };
        let results: Vec<Done> = done.try_iter().collect();
        let mut repump = BTreeSet::new();
        for result in results {
            let (cache, loc, full) = match &result {
                Done::Region { cache, loc, .. } => (cache.clone(), *loc, true),
                Done::Lod { cache, loc, .. } => (cache.clone(), *loc, false),
            };
            let Some(folder) = self.folders.get_mut(&cache) else { continue };
            folder.inflight.remove(&(loc, full));
            repump.insert(cache);
            let current = folder.manifest.as_ref().and_then(|m| Some((m.region_size, m.regions.iter().find(|r| r.loc == loc)?.hash)));
            match result {
                Done::Region { size, hash, result, .. } if current == Some((size, hash)) => match result {
                    Ok(cached) => {
                        folder.regions.insert(loc, Loaded { region: Arc::new(cached.region), hash, used: Cell::new(self.tick.get()) });
                        folder.generation += 1;
                    }
                    Err(e) => {
                        folder.failed.insert((loc, true), hash);
                        folder.error = Some(e);
                    }
                },
                Done::Lod { size, hash, result, .. } if current == Some((size, hash)) => match result {
                    Ok(lod) => {
                        folder.lods.insert(loc, (hash, Arc::new(lod)));
                        folder.generation += 1;
                    }
                    Err(e) => {
                        folder.failed.insert((loc, false), hash);
                        folder.error = Some(e);
                    }
                },
                _ => {}
            }
        }

        self.evict();
        for cache in repump {
            self.pump(&cache);
        }
    }

    fn evict(&mut self) {
        let mut total = self.loaded_bytes();
        if total <= self.budget {
            return;
        }

        let mut idle: Vec<(u64, PathBuf, Loc)> = self
            .folders
            .iter()
            .flat_map(|(cache, f)| f.regions.iter().filter(|(loc, _)| !f.want_full.contains(*loc)).map(move |(loc, l)| (l.used.get(), cache.clone(), *loc)))
            .collect();
        idle.sort();
        for (_, cache, loc) in idle {
            if total <= self.budget {
                break;
            }

            if let Some(folder) = self.folders.get_mut(&cache)
                && let Some(loaded) = folder.regions.remove(&loc)
            {
                total -= bytes_of(&loaded.region);
                folder.generation += 1;
            }
        }
    }
}

fn bytes_of(region: &Region3D) -> usize {
    region.heights.len() * 4 + region.control.len() * 4 + region.color.len()
}

/// Takes the new manifest in: what changed drops out of memory, Godot's report of what it applied is read once.
fn apply_manifest(folder: &mut Folder, manifest: Manifest) {
    let old: HashMap<Loc, RegionHash> = folder.manifest.iter().flat_map(|m| m.regions.iter().map(|r| (r.loc, r.hash))).collect();
    let new: HashMap<Loc, RegionHash> = manifest.regions.iter().map(|r| (r.loc, r.hash)).collect();
    let resized = folder.manifest.as_ref().is_some_and(|m| m.region_size != manifest.region_size || m.vertex_spacing != manifest.vertex_spacing);
    folder.regions.retain(|loc, l| !resized && new.get(loc) == Some(&l.hash));
    folder.lods.retain(|loc, entry| !resized && new.get(loc) == Some(&entry.0));
    let moved: Vec<Loc> = old.keys().chain(new.keys()).copied().filter(|loc| resized || old.get(loc) != new.get(loc)).collect();
    folder.changed.extend(moved.iter().copied());
    if std::mem::replace(&mut folder.read_before, true) {
        folder.news.extend(moved);
    }

    if resized {
        folder.inflight.clear();
        folder.failed.clear();
    }

    let last = manifest.applied.iter().map(|a| a.seq).max();
    if let (Some(seen), Some(last)) = (folder.seen_seq, last) {
        // A lower number than before means Godot started its count again, so everything in the list is new.
        let restarted = last < seen;
        for report in manifest.applied.iter().filter(|a| restarted || a.seq > seen) {
            for loc in &report.accepted {
                folder.refused.remove(loc);
            }

            for r in &report.refused {
                folder.refused.insert(r.loc, r.reason.clone());
            }

            folder.applied.push(report.clone());
        }
    }

    // The list a folder shows the first time is history, only what comes after counts as news.
    folder.seen_seq = last.or(folder.seen_seq).or(Some(0));
    if folder.manifest.as_ref() != Some(&manifest) {
        folder.generation += 1;
    }

    folder.manifest = Some(manifest);
}

fn worker(jobs: Receiver<Job>, done: Sender<Done>, repaint: Option<egui::Context>) {
    for job in jobs {
        let result = match job {
            Job::Region { cache, loc, size, hash } => {
                let result = terrain3d_cache::read_region(&cache, loc, size)
                    .map_err(|e| e.to_string())
                    .and_then(|r| if r.hash == hash { Ok(r) } else { Err(format!("region {loc:?} changed while loading")) });
                Done::Region { cache, loc, size, hash, result }
            }
            Job::Lod { cache, loc, size, hash, grid } => {
                let result = load_lod(&cache, loc, size, hash, &grid);
                Done::Lod { cache, loc, size, hash, result }
            }
        };

        if done.send(result).is_err() {
            return;
        }

        if let Some(ctx) = &repaint {
            ctx.request_repaint();
        }
    }
}

/// The cached pyramid of the region, or one built from its file and kept in the cache for the next time.
fn load_lod(cache: &Path, loc: Loc, size: u32, hash: RegionHash, grid: &Grid) -> Result<Lod, String> {
    if let Ok(lod) = terrain3d_cache::read_lod(cache, loc, size, hash) {
        return Ok(lod);
    }

    let cached = terrain3d_cache::read_region(cache, loc, size).map_err(|e| e.to_string())?;
    if cached.hash != hash {
        return Err(format!("region {loc:?} changed while loading"));
    }

    let lod = build_lod(grid, &cached.region, hash).ok_or_else(|| format!("region {loc:?} has no LOD for size {size}"))?;
    if terrain3d_cache::write_lod(cache, &lod).is_ok() {
        let _ = terrain3d_cache::prune_lod(cache, loc, hash);
    }

    Ok(lod)
}

/// The pyramid of a region without its finest level, which is the region itself. A level holds the vertices the region
/// owns and one byte per cell for holes, the seam row and column come from the neighbor's own level where two sit
/// side by side. None for a region too small for any level.
pub fn build_lod(grid: &Grid, region: &Region3D, hash: RegionHash) -> Option<Lod> {
    let levels: Vec<LodLevel> = terrain3d_tile::lod_strides(region.size)
        .into_iter()
        .filter(|&s| s > 1)
        .filter_map(|stride| {
            let tile = terrain3d_tile::tile(grid, region, |_| None, stride, |_| String::new())?;
            let side = region.size / stride;
            let n = side as usize + 1;
            let (mut heights, mut splat, mut holes) = (Vec::new(), Vec::new(), Vec::new());
            for j in 0..side {
                for i in 0..side {
                    heights.push(region.heights[((j * stride) * region.size + i * stride) as usize]);
                    if !tile.terrain.splat.is_empty() {
                        let k = (j as usize * n + i as usize) * 4;
                        splat.extend_from_slice(&tile.terrain.splat[k..k + 4]);
                    }

                    holes.push(tile.terrain.holes.get((j * side + i) as usize).copied().unwrap_or(0));
                }
            }

            Some(LodLevel { stride, side, ids: tile.ids, heights, splat, holes })
        })
        .collect();
    (!levels.is_empty()).then_some(Lod { loc: region.loc, region_size: region.size, hash, levels })
}

/// True for a Terrain3D this editor was not tested with, which keeps working because Godot does every file access.
pub fn untested_version(manifest: &Manifest) -> bool {
    let mut parts = manifest.terrain3d.split('.').map(|p| p.parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(major), Some(minor)) => (major, minor) < (1, 0) || (major, minor) > (1, 1),
        _ => false,
    }
}

/// A folder's last exported state, what `Terrain3DLink` compares its edits against.
pub struct FolderSource<'a> {
    folder: &'a Folder,
    grid: Grid,
}

impl RegionSource for FolderSource<'_> {
    fn grid(&self) -> Grid {
        self.grid
    }

    fn edge_blend(&self) -> Option<EdgeBlend> {
        let m = &self.folder.manifest.as_ref()?.material;
        (m.world_background >= BACKGROUND_FLAT).then_some(EdgeBlend { ground_level: m.ground_level as f32, region_blend: m.region_blend as f32 })
    }

    fn locs(&self) -> Vec<Loc> {
        self.folder.manifest.iter().flat_map(|m| m.regions.iter().map(|r| r.loc)).collect()
    }

    fn hash(&self, loc: Loc) -> Option<td_doc::terrain3d::RegionHash> {
        Some(self.folder.manifest.as_ref()?.regions.iter().find(|r| r.loc == loc)?.hash.0)
    }

    fn region(&self, loc: Loc) -> Option<Arc<Region3D>> {
        Some(self.folder.regions.get(&loc)?.region.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use td_doc::terrain3d::{ChangeKind, RegionEdit, Terrain3DLink};
    use td_formats::terrain3d_cache::{ManifestRegion, Refused};
    use td_geom::terrain3d::Control;

    const A: Loc = [-1, 2];
    const B: Loc = [3, -2];

    fn hash(n: u8) -> RegionHash {
        RegionHash([n; 32])
    }

    /// Heights slanted and different in every region and vertex, controls naming a different texture per column.
    fn region(loc: Loc, seed: f32) -> Region3D {
        let mut r = Region3D::blank(loc, 64);
        for (k, h) in r.heights.iter_mut().enumerate() {
            *h = seed + (k % 64) as f32 * 0.5 - (k / 64) as f32 * 0.25;
        }

        for (k, c) in r.control.iter_mut().enumerate() {
            *c = Control(0).with_base((k % 3) as u8).0;
        }

        r
    }

    fn manifest(regions: &[(Loc, u8)]) -> Manifest {
        Manifest {
            format: terrain3d_cache::FORMAT.into(),
            version: terrain3d_cache::VERSION,
            terrain3d: "1.1.0".into(),
            godot: "4.7.2".into(),
            data_version: 1.1,
            region_size: 64,
            vertex_spacing: 2.5,
            material: Default::default(),
            map_transform_identity: true,
            editor_open: false,
            regions: regions.iter().map(|(loc, h)| ManifestRegion { loc: *loc, hash: hash(*h), height_range: [0.0, 1.0] }).collect(),
            dirty: Vec::new(),
            textures: Vec::new(),
            applied: Vec::new(),
        }
    }

    /// Writes region files and a manifest for `regions` (location, hash byte).
    fn write_world(cache: &Path, regions: &[(Loc, u8)]) {
        for (loc, h) in regions {
            let seed = loc[0] as f32 * 7.0 + loc[1] as f32 * 3.0 + *h as f32;
            terrain3d_cache::write_region(cache, &CachedRegion { region: region(*loc, seed), vertex_spacing: 2.5, hash: hash(*h) }).unwrap();
        }

        terrain3d_cache::write_manifest(cache, &manifest(regions)).unwrap();
    }

    fn cache_of(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("data").join(terrain3d_cache::DIR_NAME)
    }

    fn settle(store: &mut Terrain3DStore, cache: &Path, done: impl Fn(&Terrain3DStore) -> bool) {
        let start = Instant::now();
        while !done(store) {
            assert!(start.elapsed() < Duration::from_secs(10), "the worker did not finish");
            store.refresh(cache);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn loads_the_wanted_regions_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        write_world(&cache, &[(A, 1), (B, 2), ([0, 0], 3)]);
        let mut store = Terrain3DStore::default();
        assert!(store.refresh(&cache));
        assert_eq!(store.manifest(&cache).unwrap().regions.len(), 3);
        assert!(store.region(&cache, A).is_none(), "nothing loads before it is wanted");
        store.want(&cache, &[A, B], &[]);
        settle(&mut store, &cache, |s| s.region(&cache, A).is_some() && s.region(&cache, B).is_some());
        let (a, b) = (store.region(&cache, A).unwrap(), store.region(&cache, B).unwrap());
        assert_eq!((a.loc, b.loc), (A, B));
        assert_eq!(*a, region(A, -7.0 + 6.0 + 1.0), "the second file did not leak into the first");
        assert_eq!(*b, region(B, 21.0 - 6.0 + 2.0));
        assert!(store.region(&cache, [0, 0]).is_none());
        assert_eq!(store.take_changed(&cache), vec![[-1, 2], [0, 0], [3, -2]]);
        assert!(store.take_changed(&cache).is_empty());
    }

    #[test]
    fn a_poll_reloads_only_the_regions_that_changed() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        write_world(&cache, &[(A, 1), (B, 2)]);
        let mut store = Terrain3DStore::default();
        store.refresh(&cache);
        store.want(&cache, &[A, B], &[]);
        settle(&mut store, &cache, |s| s.region(&cache, A).is_some() && s.region(&cache, B).is_some());
        let (a, b) = (store.region(&cache, A).unwrap(), store.region(&cache, B).unwrap());
        store.take_changed(&cache);
        let generation = store.generation(&cache);

        assert!(!store.refresh(&cache), "a second poll within the second does not even read the manifest");
        write_world(&cache, &[(A, 9), (B, 2)]);
        store.recheck();
        assert!(store.refresh(&cache));
        assert!(store.region(&cache, A).is_none(), "the stale copy is dropped at once");
        settle(&mut store, &cache, |s| s.region(&cache, A).is_some());
        let (a2, b2) = (store.region(&cache, A).unwrap(), store.region(&cache, B).unwrap());
        assert_eq!(*a2, region(A, -7.0 + 6.0 + 9.0));
        assert_ne!(a2.heights, a.heights);
        assert!(Arc::ptr_eq(&b, &b2), "the untouched region is not read again");
        assert_eq!(store.take_changed(&cache), vec![A]);
        assert!(store.generation(&cache) > generation);

        store.recheck();
        assert!(!store.refresh(&cache), "an unchanged manifest changes nothing");
    }

    #[test]
    fn the_news_leave_out_what_the_first_read_listed_and_keep_each_folder_apart() {
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (dir.path().join("one").join(terrain3d_cache::DIR_NAME), dir.path().join("two").join(terrain3d_cache::DIR_NAME));
        write_world(&one, &[(A, 1), (B, 2)]);
        write_world(&two, &[(A, 5), (B, 6)]);
        let mut store = Terrain3DStore::default();
        assert!(store.take_news(&one).is_empty(), "a folder nobody read has no news");
        store.refresh(&one);
        store.refresh(&two);
        assert!(store.take_news(&one).is_empty(), "the regions the first read found are not news");
        assert!(store.take_news(&one).is_empty());

        write_world(&one, &[(A, 9), (B, 2), ([0, 0], 4)]);
        write_world(&two, &[(A, 5), (B, 7)]);
        store.recheck();
        store.refresh(&one);
        store.refresh(&two);
        assert_eq!(store.take_news(&one), vec![A, [0, 0]], "a changed and an added region, not the one that stayed");
        assert!(store.take_news(&one).is_empty(), "news come once");
        assert_eq!(store.take_news(&two), vec![B], "the second folder has its own news");
    }

    #[test]
    fn two_folders_keep_separate_caches() {
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (dir.path().join("one").join(terrain3d_cache::DIR_NAME), dir.path().join("two").join(terrain3d_cache::DIR_NAME));
        write_world(&one, &[(A, 1)]);
        write_world(&two, &[(A, 5), (B, 6)]);
        let mut store = Terrain3DStore::default();
        for cache in [&one, &two] {
            store.refresh(cache);
            store.want(cache, &[A], &[]);
        }

        settle(&mut store, &one, |s| s.region(&one, A).is_some() && s.region(&two, A).is_some());
        assert_ne!(store.region(&one, A).unwrap().heights, store.region(&two, A).unwrap().heights);
        assert_eq!(store.manifest(&one).unwrap().regions.len(), 1);
        assert_eq!(store.manifest(&two).unwrap().regions.len(), 2);

        write_world(&two, &[(A, 8)]);
        store.recheck();
        store.refresh(&two);
        assert_eq!(*store.region(&one, A).unwrap(), region(A, -7.0 + 6.0 + 1.0), "the other folder keeps its region");
        assert!(store.region(&two, A).is_none());
        store.forget(&two);
        assert!(store.manifest(&two).is_none() && store.manifest(&one).is_some());
    }

    #[test]
    fn a_manifest_hash_change_marks_an_edited_region_as_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        write_world(&cache, &[(A, 1), (B, 2)]);
        let mut store = Terrain3DStore::default();
        store.refresh(&cache);
        store.want(&cache, &[A, B], &[]);
        settle(&mut store, &cache, |s| s.region(&cache, A).is_some() && s.region(&cache, B).is_some());

        let mut link = Terrain3DLink::new("res://terrain");
        let (a, b) = (store.region(&cache, A).unwrap(), store.region(&cache, B).unwrap());
        for (loc, base) in [(A, a), (B, b)] {
            let mut edited = (*base).clone();
            edited.heights[100] += 1.5;
            link.edits.insert(loc, RegionEdit::new(Arc::new(edited), store.source(&cache).unwrap().hash(loc)));
        }

        let changes = link.changed_regions(&store.source(&cache).unwrap());
        assert_eq!(
            changes.iter().map(|c| (c.loc, c.kind, c.conflict)).collect::<Vec<_>>(),
            vec![(B, ChangeKind::Edit, false), (A, ChangeKind::Edit, false)],
            "row by row from the north west"
        );

        write_world(&cache, &[(A, 1), (B, 7)]);
        store.recheck();
        store.refresh(&cache);
        let changes = link.changed_regions(&store.source(&cache).unwrap());
        assert_eq!(changes.iter().map(|c| (c.loc, c.conflict)).collect::<Vec<_>>(), vec![(B, true), (A, false)], "only the region Godot changed conflicts");
    }

    #[test]
    fn the_budget_lets_go_of_the_least_recently_used_idle_region() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        let c: Loc = [0, 0];
        write_world(&cache, &[(A, 1), (B, 2), (c, 3)]);
        let one = bytes_of(&region(A, 0.0));
        let mut store = Terrain3DStore::default().with_budget(one * 2);
        store.refresh(&cache);
        store.want(&cache, &[A, B], &[]);
        settle(&mut store, &cache, |s| s.region(&cache, A).is_some() && s.region(&cache, B).is_some());
        assert_eq!(store.loaded_bytes(), one * 2);

        // B was touched last, so A is the one to go once C is wanted instead of both.
        store.region(&cache, A);
        store.region(&cache, B);
        store.want(&cache, &[B, c], &[]);
        settle(&mut store, &cache, |s| s.region(&cache, c).is_some());
        assert!(store.region(&cache, A).is_none(), "the idle region went");
        assert!(store.region(&cache, B).is_some() && store.region(&cache, c).is_some());
        assert_eq!(store.loaded_bytes(), one * 2);

        store.want(&cache, &[A, B, c], &[]);
        settle(&mut store, &cache, |s| s.region(&cache, A).is_some());
        assert_eq!(store.loaded_bytes(), one * 3, "wanted regions stay even over the budget");
    }

    #[test]
    fn a_far_region_gets_its_pyramid_from_the_cache_file_once_built() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        write_world(&cache, &[(A, 1)]);
        let mut store = Terrain3DStore::default();
        store.refresh(&cache);
        store.want(&cache, &[], &[A]);
        settle(&mut store, &cache, |s| s.lod(&cache, A).is_some());
        assert!(store.region(&cache, A).is_none(), "a pyramid does not load the whole region");
        let lod = store.lod(&cache, A).unwrap();
        let source = region(A, -7.0 + 6.0 + 1.0);
        assert_eq!((lod.loc, lod.region_size, lod.hash), (A, 64, hash(1)));
        assert_eq!(lod.levels.len(), 1, "a 64 region keeps one level above the full one");
        let level = &lod.levels[0];
        assert_eq!((level.stride, level.side, level.heights.len()), (4, 16, 256));
        for (j, i) in [(0usize, 0usize), (0, 15), (7, 3), (15, 0), (15, 15)] {
            assert_eq!(level.heights[j * 16 + i], source.heights[j * 4 * 64 + i * 4], "vertex {i}, {j}");
        }

        assert_eq!(level.ids, vec![0, 1, 2], "the three textures that show, most used first");
        assert_eq!(level.splat.len(), 256 * 4);
        assert_eq!(level.holes.len(), 256);
        assert!(terrain3d_cache::lod_path(&cache, A, hash(1)).unwrap().is_file());

        // A fresh store finds the pyramid in the cache even though the region file is gone.
        std::fs::remove_file(terrain3d_cache::region_path(&cache, A).unwrap()).unwrap();
        let mut again = Terrain3DStore::default();
        again.refresh(&cache);
        again.want(&cache, &[], &[A]);
        settle(&mut again, &cache, |s| s.lod(&cache, A).is_some());
        assert_eq!(*again.lod(&cache, A).unwrap(), *lod);
    }

    #[test]
    fn a_changed_region_replaces_its_pyramid_and_prunes_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        write_world(&cache, &[(A, 1), (B, 2)]);
        let mut store = Terrain3DStore::default();
        store.refresh(&cache);
        store.want(&cache, &[], &[A, B]);
        settle(&mut store, &cache, |s| s.lod(&cache, A).is_some() && s.lod(&cache, B).is_some());
        let old = store.lod(&cache, A).unwrap();

        write_world(&cache, &[(A, 4), (B, 2)]);
        store.recheck();
        store.refresh(&cache);
        assert!(store.lod(&cache, A).is_none() && store.lod(&cache, B).is_some());
        settle(&mut store, &cache, |s| s.lod(&cache, A).is_some());
        let new = store.lod(&cache, A).unwrap();
        assert_eq!(new.hash, hash(4));
        assert_ne!(new.levels[0].heights, old.levels[0].heights);
        assert!(!terrain3d_cache::lod_path(&cache, A, hash(1)).unwrap().exists(), "the pyramid of the old hash is gone");
        assert!(terrain3d_cache::lod_path(&cache, A, hash(4)).unwrap().is_file());
        assert!(terrain3d_cache::lod_path(&cache, B, hash(2)).unwrap().is_file(), "another region's file stays");
    }

    #[test]
    fn a_broken_region_file_is_reported_and_not_asked_for_again() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        write_world(&cache, &[(A, 1), (B, 2)]);
        let path = terrain3d_cache::region_path(&cache, A).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        let mut store = Terrain3DStore::default();
        store.refresh(&cache);
        store.want(&cache, &[A, B], &[]);
        settle(&mut store, &cache, |s| s.region(&cache, B).is_some() && s.error(&cache).is_some());
        assert!(store.region(&cache, A).is_none());
        store.refresh(&cache);
        assert!(store.folders[&cache].inflight.is_empty(), "no retry while the hash stays the same");

        std::fs::write(&path, &bytes).unwrap();
        write_world(&cache, &[(A, 1), (B, 2)]);
        store.recheck();
        store.want(&cache, &[A, B], &[]);
        settle(&mut store, &cache, |s| s.region(&cache, A).is_some());
        assert!(store.error(&cache).is_none(), "a good manifest read clears the error");
    }

    #[test]
    fn an_unreadable_manifest_keeps_the_last_good_one_and_a_missing_one_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        let mut store = Terrain3DStore::default();
        assert!(!store.refresh(&cache), "no cache yet");
        assert!(store.manifest(&cache).is_none() && store.error(&cache).is_none());

        write_world(&cache, &[(A, 1)]);
        store.recheck();
        assert!(store.refresh(&cache));
        std::fs::write(terrain3d_cache::manifest_path(&cache), "{ not json").unwrap();
        store.recheck();
        store.refresh(&cache);
        assert!(store.error(&cache).is_some());
        assert_eq!(store.manifest(&cache).unwrap().regions.len(), 1);

        std::fs::write(terrain3d_cache::manifest_path(&cache), r#"{"format":"trenchdot-terrain3d","version":99,"region_size":64,"vertex_spacing":1.0}"#)
            .unwrap();
        store.recheck();
        store.refresh(&cache);
        assert!(store.error(&cache).unwrap().contains("newer"), "{:?}", store.error(&cache));

        store.want(&cache, &[A], &[]);
        write_world(&cache, &[(A, 1)]);
        store.recheck();
        store.refresh(&cache);
        settle(&mut store, &cache, |s| s.region(&cache, A).is_some());
        std::fs::remove_dir_all(&cache).unwrap();
        store.recheck();
        assert!(store.refresh(&cache));
        assert!(store.manifest(&cache).is_none() && store.region(&cache, A).is_none());
    }

    #[test]
    fn a_new_region_size_drops_every_loaded_region() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        write_world(&cache, &[(A, 1), (B, 2)]);
        let mut store = Terrain3DStore::default();
        store.refresh(&cache);
        store.want(&cache, &[A, B], &[]);
        settle(&mut store, &cache, |s| s.region(&cache, A).is_some() && s.region(&cache, B).is_some());

        let mut m = manifest(&[(A, 1), (B, 2)]);
        m.region_size = 128;
        terrain3d_cache::write_manifest(&cache, &m).unwrap();
        store.recheck();
        store.refresh(&cache);
        assert!(store.region(&cache, A).is_none() && store.region(&cache, B).is_none());
        assert_eq!(store.grid(&cache).unwrap().region_size, 128);
    }

    #[test]
    fn godots_apply_report_is_news_once_and_refusals_stick_until_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        let mut m = manifest(&[(A, 1), (B, 2)]);
        m.applied = vec![Applied { seq: 4, accepted: vec![A], refused: vec![] }];
        terrain3d_cache::write_manifest(&cache, &m).unwrap();
        let mut store = Terrain3DStore::default();
        store.refresh(&cache);
        assert!(store.take_applied(&cache).is_empty(), "what the folder said before we looked is history");

        m.applied.push(Applied { seq: 5, accepted: vec![A], refused: vec![Refused { loc: B, reason: "dirty".into() }] });
        terrain3d_cache::write_manifest(&cache, &m).unwrap();
        store.recheck();
        store.refresh(&cache);
        let news = store.take_applied(&cache);
        assert_eq!(news.len(), 1);
        assert_eq!(news[0].seq, 5);
        assert_eq!(store.refused(&cache), BTreeMap::from([(B, "dirty".to_string())]));
        store.recheck();
        store.refresh(&cache);
        assert!(store.take_applied(&cache).is_empty(), "the same report is not news twice");

        m.applied.push(Applied { seq: 6, accepted: vec![B], refused: vec![] });
        terrain3d_cache::write_manifest(&cache, &m).unwrap();
        store.recheck();
        store.refresh(&cache);
        assert_eq!(store.take_applied(&cache).len(), 1);
        assert!(store.refused(&cache).is_empty(), "an accepted region is no longer refused");

        m.applied = vec![Applied { seq: 1, accepted: vec![], refused: vec![Refused { loc: A, reason: "changed".into() }] }];
        terrain3d_cache::write_manifest(&cache, &m).unwrap();
        store.recheck();
        store.refresh(&cache);
        assert_eq!(store.take_applied(&cache).len(), 1, "a count that starts again is read as new");
        assert_eq!(store.refused(&cache), BTreeMap::from([(A, "changed".to_string())]));
        store.forget_refusal(&cache, A);
        assert!(store.refused(&cache).is_empty());
    }

    #[test]
    fn dirty_regions_come_from_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        let mut m = manifest(&[(A, 1), (B, 2)]);
        m.dirty = vec![B];
        terrain3d_cache::write_manifest(&cache, &m).unwrap();
        let mut store = Terrain3DStore::default();
        store.refresh(&cache);
        assert!(store.dirty(&cache, B) && !store.dirty(&cache, A) && !store.dirty(&cache, [9, 9]));
    }

    #[test]
    fn the_source_reports_the_grid_hashes_and_edge_blend() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        let mut m = manifest(&[(A, 1)]);
        terrain3d_cache::write_manifest(&cache, &m).unwrap();
        let mut store = Terrain3DStore::default();
        assert!(store.source(&cache).is_none());
        store.refresh(&cache);
        store.upm = 16.0;
        let source = store.source(&cache).unwrap();
        assert_eq!(source.grid(), Grid { region_size: 64, spacing: 2.5, upm: 16.0 });
        assert_eq!((source.locs(), source.hash(A), source.hash(B)), (vec![A], Some([1; 32]), None));
        assert_eq!(source.edge_blend(), None, "no world background, no blending");

        m.material.world_background = 2;
        m.material.ground_level = -3.5;
        m.material.region_blend = 0.25;
        terrain3d_cache::write_manifest(&cache, &m).unwrap();
        store.recheck();
        store.refresh(&cache);
        assert_eq!(store.source(&cache).unwrap().edge_blend(), Some(EdgeBlend { ground_level: -3.5, region_blend: 0.25 }));
    }

    #[test]
    fn terrain3d_versions_outside_one_zero_to_one_one_are_untested() {
        let of = |v: &str| {
            let mut m = manifest(&[]);
            m.terrain3d = v.into();
            untested_version(&m)
        };
        assert!(!of("1.0.0") && !of("1.0.2") && !of("1.1.0") && !of("1.1.9"));
        assert!(of("0.9.3") && of("1.2.0") && of("2.0.0") && of("0.99.0"));
        assert!(!of("") && !of("dev") && !of("1"), "a version nobody can read is not called untested");
    }
}
