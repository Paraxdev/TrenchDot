//! The editor's view of the Terrain3D caches Godot exports. Each manifest is polled once a second, regions load on a
//! worker thread under a memory budget, and the LOD pyramids of regions too far away to load whole are built beside them.
//! A data folder Godot has not exported yet is read from Terrain3D's own region files instead, writing nothing.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant, SystemTime};

use td_doc::terrain3d::RegionSource;
use td_formats::terrain3d_cache::{self, Applied, CacheError, CachedRegion, Lod, LodLevel, Manifest, ManifestRegion, RegionHash};
use td_formats::terrain3d_res::{self, ResRegion};
use td_geom::terrain3d::{EdgeBlend, Grid, Loc, Region3D, file_stem, parse_file_name};
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
    /// The Godot editor that wrote the manifest is gone, so its list of unsaved regions and their hashes is dead.
    editor_gone: bool,
    /// The manifest was built from the region files of a data folder with no cache.
    from_files: bool,
    /// The region files the last read of the data folder found.
    listing: Listing,
    scan: Option<Arc<Progress>>,
}

/// Region file names with their size and modification time.
type Listing = BTreeMap<String, (u64, SystemTime)>;

struct Progress {
    done: AtomicUsize,
    total: usize,
}

enum Job {
    Region { cache: PathBuf, loc: Loc, size: u32, hash: RegionHash, from_files: bool },
    Lod { cache: PathBuf, loc: Loc, size: u32, hash: RegionHash, grid: Grid, from_files: bool },
    Scan { cache: PathBuf, listing: Listing, progress: Arc<Progress> },
}

enum Done {
    Region { cache: PathBuf, loc: Loc, size: u32, hash: RegionHash, result: Result<CachedRegion, String> },
    Lod { cache: PathBuf, loc: Loc, size: u32, hash: RegionHash, result: Result<Lod, String> },
    Scan { cache: PathBuf, listing: Listing, progress: Arc<Progress>, manifest: Option<Box<Manifest>>, error: Option<String> },
}

/// Region data and its lowest and highest height in meters.
type Measured = (Weak<dyn Any + Send + Sync>, [f32; 2]);

pub struct Terrain3DStore {
    folders: HashMap<PathBuf, Folder>,
    budget: usize,
    /// Map units per meter, the scale the LOD pyramids are built with.
    pub upm: f64,
    tick: Cell<u64>,
    jobs: Option<Sender<Job>>,
    done: Option<Receiver<Done>>,
    repaint: Option<egui::Context>,
    /// Height ranges of region data, keyed by its address. The Weak keeps the address from going to other data, and
    /// `Arc::make_mut` moves data a Weak still points at, so an edited region gets a new address and a new range.
    ranges: RefCell<HashMap<usize, Measured>>,
}

impl Default for Terrain3DStore {
    fn default() -> Self {
        Self::new(None)
    }
}

impl Terrain3DStore {
    /// `repaint` is asked to redraw whenever the worker finished a region.
    pub fn new(repaint: Option<egui::Context>) -> Self {
        Self {
            folders: HashMap::new(),
            budget: DEFAULT_BUDGET,
            upm: 32.0,
            tick: Cell::new(0),
            jobs: None,
            done: None,
            repaint,
            ranges: RefCell::new(HashMap::new()),
        }
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

    /// Holds the regions `need` whole and waits until they loaded, for tools that edit them. The next manifest read clears
    /// the error to try a failed region again, so the wait keeps the last error it saw to name when it gives up.
    pub fn load_whole(&mut self, cache: &Path, need: &[Loc], timeout: Duration) -> Result<(), String> {
        self.want(cache, need, &[]);
        let start = Instant::now();
        let mut why = None;
        loop {
            let missing: Vec<Loc> = need.iter().copied().filter(|l| self.region(cache, *l).is_none()).collect();
            if missing.is_empty() {
                return Ok(());
            }

            if let Some(e) = self.error(cache) {
                why = Some(e.to_string());
            }

            if start.elapsed() >= timeout {
                let mut names = missing.iter().take(4).map(|l| format!("{l:?}")).collect::<Vec<_>>().join(", ");
                if missing.len() > 4 {
                    names += &format!(" and {} more", missing.len() - 4);
                }

                let why = why.map(|e| format!(", the last error was: {e}")).unwrap_or_default();
                return Err(format!("the Terrain3D regions {names} did not load in {} s{why}", timeout.as_secs()));
            }

            std::thread::sleep(Duration::from_millis(5));
            self.refresh(cache);
        }
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

    /// Lowest and highest finite height of a region in meters, an empty range for none.
    pub fn region_range(&self, region: &Arc<Region3D>) -> [f32; 2] {
        self.range(Arc::<Region3D>::downgrade(region), || height_range(&region.heights))
    }

    /// The range of the finest level of a pyramid.
    pub fn lod_range(&self, lod: &Arc<Lod>) -> [f32; 2] {
        self.range(Arc::<Lod>::downgrade(lod), || finest(lod).map_or([f32::INFINITY, f32::NEG_INFINITY], |l| height_range(&l.heights)))
    }

    fn range(&self, data: Weak<dyn Any + Send + Sync>, measure: impl FnOnce() -> [f32; 2]) -> [f32; 2] {
        let key = data.as_ptr() as *const () as usize;
        let mut ranges = self.ranges.borrow_mut();
        if let Some((_, range)) = ranges.get(&key) {
            return *range;
        }

        ranges.retain(|_, (held, _)| held.strong_count() > 0);
        let range = measure();
        ranges.insert(key, (data, range));
        range
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

    /// Notes whether the Godot editor of the folder is gone. The regions it listed unsaved stop counting as conflicts
    /// meanwhile, the hash they have in the manifest never reached the disk.
    pub fn set_editor_gone(&mut self, cache: &Path, gone: bool) {
        if let Some(f) = self.folders.get_mut(cache)
            && std::mem::replace(&mut f.editor_gone, gone) != gone
        {
            f.generation += 1;
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

    /// Whether a region or pyramid of the folder is still loading.
    pub fn loading(&self, cache: &Path) -> bool {
        self.folders.get(cache).is_some_and(|f| !f.inflight.is_empty())
    }

    /// Whether the folder shows what its region files hold because Godot has not exported a cache for it.
    pub fn from_files(&self, cache: &Path) -> bool {
        self.folders.get(cache).is_some_and(|f| f.from_files && f.manifest.is_some())
    }

    /// The region files read so far and how many there are, while the folder's files are read.
    pub fn scanning(&self, cache: &Path) -> Option<(usize, usize)> {
        let progress = self.folders.get(cache)?.scan.as_ref()?;
        Some((progress.done.load(Ordering::Relaxed), progress.total))
    }

    /// Waits for a read of the folder's region files to finish, so a tool sees the folder before it asks Godot for a cache.
    pub fn finish_scan(&mut self, cache: &Path, timeout: Duration) -> Result<(), String> {
        let start = Instant::now();
        while let Some((done, total)) = self.scanning(cache) {
            if start.elapsed() >= timeout {
                return Err(format!("the Terrain3D region files are still being read, {done} of {total} after {} s", timeout.as_secs()));
            }

            std::thread::sleep(Duration::from_millis(5));
            self.refresh(cache);
        }

        Ok(())
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
                if !folder.from_files {
                    clear(folder);
                }

                self.poll_files(cache);
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

                if std::mem::take(&mut folder.from_files) {
                    folder.listing.clear();
                }

                folder.scan = None;
                apply_manifest(folder, manifest);
            }
            Err(e) => folder.error = Some(e.to_string()),
        }
    }

    /// Reads the region files of a data folder without a cache again whenever their names, sizes or times change.
    fn poll_files(&mut self, cache: &Path) {
        let folder = self.folders.get_mut(cache).expect("folder");
        if folder.scan.is_some() {
            return;
        }

        let listing = cache.parent().map(region_files).unwrap_or_default();
        if listing == folder.listing {
            return;
        }

        if listing.is_empty() {
            folder.listing.clear();
            folder.from_files = false;
            clear(folder);
            return;
        }

        let progress = Arc::new(Progress { done: AtomicUsize::new(0), total: listing.len() });
        folder.scan = Some(progress.clone());
        folder.generation += 1;
        let _ = self.sender().send(Job::Scan { cache: cache.to_path_buf(), listing, progress });
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
        let from_files = folder.from_files;
        let mut jobs = Vec::new();
        for &loc in &folder.want_full {
            let Some(&hash) = hashes.get(&loc) else { continue };
            if !folder.regions.contains_key(&loc) && folder.failed.get(&(loc, true)) != Some(&hash) && folder.inflight.insert((loc, true)) {
                jobs.push(Job::Region { cache: cache.to_path_buf(), loc, size: grid.region_size, hash, from_files });
            }
        }

        for &loc in &folder.want_lod {
            let Some(&hash) = hashes.get(&loc) else { continue };
            let current = folder.lods.get(&loc).is_some_and(|(h, _)| *h == hash);
            if !current && folder.failed.get(&(loc, false)) != Some(&hash) && folder.inflight.insert((loc, false)) {
                jobs.push(Job::Lod { cache: cache.to_path_buf(), loc, size: grid.region_size, hash, grid, from_files });
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
            let (cache, loc, full) = match result {
                Done::Region { ref cache, loc, .. } => (cache.clone(), loc, true),
                Done::Lod { ref cache, loc, .. } => (cache.clone(), loc, false),
                Done::Scan { cache, listing, progress, manifest, error } => {
                    if let Some(folder) = self.folders.get_mut(&cache)
                        && folder.scan.as_ref().is_some_and(|p| Arc::ptr_eq(p, &progress))
                    {
                        take_scan(folder, listing, manifest, error);
                        repump.insert(cache);
                    }

                    continue;
                }
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

/// Forgets the manifest and everything read under it.
fn clear(folder: &mut Folder) {
    if folder.manifest.take().is_some() {
        folder.text.clear();
        folder.error = None;
        folder.changed.extend(folder.regions.keys().copied());
        folder.regions.clear();
        folder.lods.clear();
        folder.inflight.clear();
        // The reads failed against files that went with the manifest, and a re-export brings back the same hashes.
        folder.failed.clear();
        folder.generation += 1;
    }
}

/// Takes in what a read of the region files found. A read that found no region keeps the last good manifest.
fn take_scan(folder: &mut Folder, listing: Listing, manifest: Option<Box<Manifest>>, error: Option<String>) {
    folder.scan = None;
    folder.listing = listing;
    folder.error = error;
    folder.failed.clear();
    folder.generation += 1;
    if let Some(manifest) = manifest {
        folder.from_files = true;
        apply_manifest(folder, *manifest);
    }
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
            Job::Region { cache, loc, size, hash, from_files } => {
                let result = read_region(&cache, loc, size, from_files)
                    .and_then(|r| if r.hash == hash { Ok(r) } else { Err(format!("region {loc:?} changed while loading")) });
                Done::Region { cache, loc, size, hash, result }
            }
            Job::Lod { cache, loc, size, hash, grid, from_files } => {
                let result = load_lod(&cache, loc, size, hash, &grid, from_files);
                Done::Lod { cache, loc, size, hash, result }
            }
            Job::Scan { cache, listing, progress } => {
                let (manifest, error) = scan(&cache, &listing, &progress, repaint.as_ref());
                Done::Scan { cache, listing, progress, manifest: manifest.map(Box::new), error }
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

/// The cached pyramid of the region, or one built from its file and kept in the cache for the next time. A folder read
/// from its region files has no cache to keep it in.
fn load_lod(cache: &Path, loc: Loc, size: u32, hash: RegionHash, grid: &Grid, from_files: bool) -> Result<Lod, String> {
    if !from_files && let Ok(lod) = terrain3d_cache::read_lod(cache, loc, size, hash) {
        return Ok(lod);
    }

    let cached = read_region(cache, loc, size, from_files)?;
    if cached.hash != hash {
        return Err(format!("region {loc:?} changed while loading"));
    }

    let lod = build_lod(grid, &cached.region, hash).ok_or_else(|| format!("region {loc:?} has no LOD for size {size}"))?;
    if !from_files && terrain3d_cache::write_lod(cache, &lod).is_ok() {
        let _ = terrain3d_cache::prune_lod(cache, loc, hash);
    }

    Ok(lod)
}

/// Region `loc` from the cache, or from Terrain3D's own file in the data folder the cache would sit in.
fn read_region(cache: &Path, loc: Loc, size: u32, from_files: bool) -> Result<CachedRegion, String> {
    if !from_files {
        return terrain3d_cache::read_region(cache, loc, size).map_err(|e| e.to_string());
    }

    let name = format!("{}.res", file_stem(loc).ok_or_else(|| format!("region {loc:?} is outside the region map"))?);
    let read = terrain3d_res::read_region_res(&data_dir(cache).join(&name)).map_err(|e| file_error(&name, e))?;
    Ok(read.cached)
}

fn data_dir(cache: &Path) -> &Path {
    cache.parent().unwrap_or(cache)
}

fn file_error(name: &str, e: CacheError) -> String {
    let why = match e {
        CacheError::RegionFile(why) => why,
        e => e.to_string(),
    };
    format!("Terrain3D region file {name}: {why}")
}

/// The `.res` region files of a data folder, the ones Terrain3D loads.
fn region_files(dir: &Path) -> Listing {
    let Ok(entries) = std::fs::read_dir(dir) else { return Listing::new() };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let res = Path::new(&name).extension().is_some_and(|e| e == "res") && parse_file_name(&name).is_some();
            let meta = entry.metadata().ok().filter(|m| res && m.is_file())?;
            Some((name, (meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH))))
        })
        .collect()
}

/// What `scan` keeps of a region file. The maps are dropped as soon as they are measured, a folder can hold gigabytes.
struct Scanned {
    region: ManifestRegion,
    size: u32,
    data_version: f64,
    vertex_spacing: f32,
}

/// Reads every region file of `listing` and builds the manifest the addon would export for them. The spacing and data
/// version come from the newest file, the first by name among equals, as the addon takes them. A file that does not
/// read is left out and named in the error. No manifest when no region read or the regions differ in size.
fn scan(cache: &Path, listing: &Listing, progress: &Progress, repaint: Option<&egui::Context>) -> (Option<Manifest>, Option<String>) {
    let mut errors = Vec::new();
    let mut read: Vec<Scanned> = Vec::new();
    let mut newest: Option<(SystemTime, usize)> = None;
    for (name, (_, modified)) in listing {
        match terrain3d_res::read_region_res(&data_dir(cache).join(name)) {
            Ok(ResRegion { cached, data_version }) => {
                if newest.is_none_or(|(at, _)| *modified > at) {
                    newest = Some((*modified, read.len()));
                }

                let region = ManifestRegion { loc: cached.region.loc, hash: cached.hash, height_range: height_range(&cached.region.heights) };
                read.push(Scanned { region, size: cached.region.size, data_version, vertex_spacing: cached.vertex_spacing });
            }
            Err(e) => errors.push(file_error(name, e)),
        }

        progress.done.fetch_add(1, Ordering::Relaxed);
        if let Some(ctx) = repaint {
            ctx.request_repaint();
        }
    }

    let Some((_, at)) = newest else { return (None, joined(errors)) };
    let size = read[at].size;
    if read.iter().any(|r| r.size != size) {
        errors.insert(0, "the region files have different sizes, open the folder in Godot once to make them match".into());
        return (None, joined(errors));
    }

    let manifest = Manifest {
        format: terrain3d_cache::FORMAT.into(),
        version: terrain3d_cache::VERSION,
        terrain3d: String::new(),
        godot: String::new(),
        data_version: read[at].data_version,
        region_size: size,
        vertex_spacing: read[at].vertex_spacing as f64,
        material: Default::default(),
        map_transform_identity: true,
        editor_open: false,
        regions: read.into_iter().map(|r| r.region).collect(),
        dirty: Vec::new(),
        textures: Vec::new(),
        applied: Vec::new(),
        ocean: None,
        resources: None,
    };
    (Some(manifest), joined(errors))
}

/// The first few errors in one line.
fn joined(mut errors: Vec<String>) -> Option<String> {
    const SHOWN: usize = 3;
    if errors.len() > SHOWN {
        let more = errors.len() - SHOWN;
        errors.truncate(SHOWN);
        errors.push(format!("{more} more did not read"));
    }

    (!errors.is_empty()).then(|| errors.join(", "))
}

pub fn finest(lod: &Lod) -> Option<&LodLevel> {
    lod.levels.iter().min_by_key(|l| l.stride)
}

fn height_range(heights: &[f32]) -> [f32; 2] {
    heights.iter().filter(|h| h.is_finite()).fold([f32::INFINITY, f32::NEG_INFINITY], |[lo, hi], h| [lo.min(*h), hi.max(*h)])
}

/// The pyramid of a region without its finest level, which is the region itself. A level holds the vertices the region
/// owns and one byte per cell for holes, the seam row and column come from the neighbor's own level where two sit
/// side by side. Heights and flag bits are the vertex's own, the color the average of the `stride` square around it,
/// so a far tint does not hinge on the one pixel a vertex lands on. None for a region too small for any level.
pub fn build_lod(grid: &Grid, region: &Region3D, hash: RegionHash) -> Option<Lod> {
    const FLAG_BITS: u32 = 3;
    let colored = region.color.len() == region.heights.len() * 4 && !region.color_is_blank();
    let flagged = region.control.iter().any(|c| c & FLAG_BITS != 0);
    let levels: Vec<LodLevel> = terrain3d_tile::lod_strides(region.size)
        .into_iter()
        .filter(|&s| s > 1)
        .filter_map(|stride| {
            let tile = terrain3d_tile::tile(grid, region, |_| None, stride, |_| String::new())?;
            let side = region.size / stride;
            let n = side as usize + 1;
            let (mut heights, mut splat, mut holes, mut colors, mut flags) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for j in 0..side {
                for i in 0..side {
                    let k = ((j * stride) * region.size + i * stride) as usize;
                    heights.push(region.heights[k]);
                    if !tile.terrain.splat.is_empty() {
                        let k = (j as usize * n + i as usize) * 4;
                        splat.extend_from_slice(&tile.terrain.splat[k..k + 4]);
                    }

                    holes.push(tile.terrain.holes.get((j * side + i) as usize).copied().unwrap_or(0));
                    if colored {
                        colors.extend(average_color(region, [i * stride, j * stride], stride));
                    }

                    if flagged {
                        flags.push((region.control[k] & FLAG_BITS) as u8);
                    }
                }
            }

            Some(LodLevel { stride, side, ids: tile.ids, heights, splat, holes, colors, flags })
        })
        .collect();
    (!levels.is_empty()).then_some(Lod { loc: region.loc, region_size: region.size, hash, levels })
}

/// The colors of an edited region's coarse block, the way its pyramid level of `stride` keeps them. None for a region
/// without a color map or with a blank one.
pub fn coarse_color(region: &Region3D, stride: u32) -> Option<Vec<u8>> {
    let colored = region.color.len() == region.heights.len() * 4 && !region.color_is_blank();
    if !colored || stride == 0 || !region.size.is_multiple_of(stride) {
        return None;
    }

    let side = region.size / stride;
    Some((0..side).flat_map(|j| (0..side).map(move |i| [i * stride, j * stride])).flat_map(|at| average_color(region, at, stride)).collect())
}

/// The mean color of the `stride` square of pixels centered on `at`, cut to the region.
fn average_color(region: &Region3D, at: [u32; 2], stride: u32) -> [u8; 4] {
    let span = |v: u32| v.saturating_sub(stride / 2)..(v + stride.div_ceil(2)).min(region.size);
    let (mut sum, mut count) = ([0u32; 4], 0u32);
    for z in span(at[1]) {
        for x in span(at[0]) {
            let k = (z * region.size + x) as usize * 4;
            for (s, c) in sum.iter_mut().zip(&region.color[k..k + 4]) {
                *s += *c as u32;
            }

            count += 1;
        }
    }

    sum.map(|s| ((s + count / 2) / count) as u8)
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

    fn unsure(&self, loc: Loc) -> bool {
        self.folder.editor_gone && self.folder.manifest.as_ref().is_some_and(|m| m.dirty.contains(&loc))
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
    use td_geom::terrain3d::{BLANK_COLOR, Control};

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
            ocean: None,
            resources: None,
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

    fn settle(store: &mut Terrain3DStore, cache: &Path, mut done: impl FnMut(&Terrain3DStore) -> bool) {
        let start = Instant::now();
        while !done(store) {
            assert!(start.elapsed() < Duration::from_secs(10), "the worker did not finish");
            store.refresh(cache);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn height_ranges_follow_the_data_they_were_measured_on() {
        let store = Terrain3DStore::default();
        let (mut a, b) = (Arc::new(region(A, 4.0)), Arc::new(region(B, -20.0)));
        assert_eq!(store.region_range(&a), [-11.75, 35.5]);
        assert_eq!(store.region_range(&b), [-35.75, 11.5], "a second region does not get the first one's range");
        assert_eq!(store.region_range(&a), [-11.75, 35.5]);

        let shared = a.clone();
        Arc::make_mut(&mut a).heights[70] = 90.0;
        assert_eq!((store.region_range(&a), store.region_range(&shared)), ([-11.75, 90.0], [-11.75, 35.5]), "an edit next to its undo step");
        drop(shared);
        Arc::make_mut(&mut a).heights[71] = f32::NAN;
        Arc::make_mut(&mut a).heights[0] = -50.0;
        assert_eq!(store.region_range(&a), [-50.0, 90.0], "an edit nothing else holds, NaN left out");

        let mut holes = Region3D::blank(A, 64);
        holes.heights.fill(f32::NAN);
        let empty = store.region_range(&Arc::new(holes));
        assert!(empty[0] > empty[1], "no height is an empty range: {empty:?}");
        assert_eq!(store.ranges.borrow().len(), 3, "dead data is dropped when a range is measured: {:?}", store.ranges.borrow().keys());

        let level = |stride: u32, fill: f32| LodLevel {
            stride,
            side: 64 / stride,
            ids: Vec::new(),
            heights: vec![fill; (64 / stride).pow(2) as usize],
            splat: Vec::new(),
            holes: Vec::new(),
            colors: Vec::new(),
            flags: Vec::new(),
        };
        let mut lod = Lod { loc: A, region_size: 64, hash: hash(1), levels: vec![level(16, -400.0), level(4, 7.0)] };
        lod.levels[1].heights[5] = 12.5;
        assert_eq!(store.lod_range(&Arc::new(lod)), [7.0, 12.5], "a pyramid's range is its finest level's");
        let bare = Lod { loc: B, region_size: 64, hash: hash(2), levels: Vec::new() };
        assert_eq!(store.lod_range(&Arc::new(bare)), [f32::INFINITY, f32::NEG_INFINITY]);
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
    fn a_pyramid_keeps_each_vertex_flag_bits_and_averages_the_color_around_it() {
        let grid = Grid { region_size: 64, spacing: 2.5, upm: 32.0 };
        let plain = build_lod(&grid, &region(A, 3.0), hash(1)).unwrap();
        assert!(plain.levels.iter().all(|l| l.colors.is_empty() && l.flags.is_empty()), "nothing to keep, nothing stored");

        let mut r = region(A, 3.0);
        let at = |x: usize, z: usize| z * 64 + x;
        r.control[at(0, 0)] = Control(r.control[at(0, 0)]).with_auto(true).0;
        r.control[at(8, 4)] = Control(r.control[at(8, 4)]).with_navigation(true).with_hole(true).0;
        r.control[at(9, 4)] = Control(r.control[at(9, 4)]).with_auto(true).0;
        r.fill_color();
        let red = [200, 40, 90, 255];
        for z in 2..6 {
            for x in 6..12 {
                r.color[at(x, z) * 4..at(x, z) * 4 + 4].copy_from_slice(&red);
            }
        }

        let lod = build_lod(&grid, &r, hash(1)).unwrap();
        let level = &lod.levels[0];
        assert_eq!((level.stride, level.colors.len(), level.flags.len()), (4, 256 * 4, 256));
        let v = |i: usize, j: usize| j * 16 + i;
        assert_eq!((level.flags[v(0, 0)], level.flags[v(2, 1)]), (1, 2), "the vertex's own bits, the hole bit left out");
        assert_eq!(level.flags.iter().filter(|f| **f != 0).count(), 2, "a bit beside a vertex does not count");
        let color = |i, j| level.colors[v(i, j) * 4..v(i, j) * 4 + 4].to_vec();
        assert_eq!(color(2, 1), red, "x 6 to 9 and z 2 to 5 are all red");
        let half: Vec<u8> = (0..4).map(|c| ((red[c] as u32 * 2 + BLANK_COLOR[c] as u32 * 2 + 2) / 4) as u8).collect();
        assert_eq!(color(3, 1), half, "x 10 and 11 red, 12 and 13 blank");
        assert_eq!((color(0, 0), color(1, 1)), (BLANK_COLOR.to_vec(), BLANK_COLOR.to_vec()));
        assert_eq!(coarse_color(&r, 4).as_ref(), Some(&level.colors), "an edited region's coarse block matches its pyramid");
        assert_eq!(coarse_color(&r, 16).map(|c| c.len()), Some(4 * 4 * 4));
        assert_eq!((coarse_color(&r, 0), coarse_color(&r, 5)), (None, None));
        r.color = BLANK_COLOR.repeat(64 * 64);
        assert_eq!(coarse_color(&r, 4), None, "a blank color map tints nothing");
        assert_eq!(coarse_color(&region(A, 3.0), 4), None);
    }

    #[test]
    fn a_pyramid_file_of_an_older_version_is_built_again_and_a_current_one_read() {
        let grid = Grid { region_size: 64, spacing: 2.5, upm: 32.0 };
        let (old_dir, new_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (old, new) = (cache_of(&old_dir), cache_of(&new_dir));
        write_world(&old, &[(A, 1)]);
        write_world(&new, &[(A, 1)]);
        let built = load_lod(&old, A, 64, hash(1), &grid, false).unwrap();
        let mut marked = built.clone();
        marked.levels[0].heights[0] = -999.0;
        for cache in [&old, &new] {
            terrain3d_cache::write_lod(cache, &marked).unwrap();
        }

        let path = terrain3d_cache::lod_path(&old, A, hash(1)).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(load_lod(&new, A, 64, hash(1), &grid, false).unwrap(), marked, "a current file is read, not built");
        assert_eq!(load_lod(&old, A, 64, hash(1), &grid, false).unwrap(), built, "a version 1 file is built again");
        assert_eq!(terrain3d_cache::read_lod(&old, A, 64, hash(1)).unwrap(), built, "and written back at the current version");
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
    fn what_failed_while_the_cache_was_gone_loads_once_it_is_back() {
        let (dir, other_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (cache, other) = (cache_of(&dir), cache_of(&other_dir));
        write_world(&cache, &[(A, 1), (B, 2)]);
        write_world(&other, &[(A, 1)]);
        std::fs::write(terrain3d_cache::region_path(&other, A).unwrap(), b"broken").unwrap();
        let mut store = Terrain3DStore::default();
        store.refresh(&other);
        store.want(&other, &[A], &[]);
        settle(&mut store, &other, |s| s.error(&other).is_some() && !s.loading(&other));

        // The views asked for regions and pyramids, then the folder went away before the loader read them.
        store.refresh(&cache);
        std::fs::remove_dir_all(&cache).unwrap();
        store.want(&cache, &[A], &[B]);
        settle(&mut store, &cache, |s| s.error(&cache).is_some() && !s.loading(&cache));
        assert_eq!(store.folders[&cache].failed.len(), 2, "both reads failed under the old manifest");
        store.recheck();
        store.refresh(&cache);
        assert!(store.manifest(&cache).is_none() && store.error(&cache).is_none());

        // Godot exports the same data again, so every hash is the one the reads failed with.
        write_world(&cache, &[(A, 1), (B, 2)]);
        store.recheck();
        store.refresh(&cache);
        store.want(&cache, &[A], &[B]);
        settle(&mut store, &cache, |s| s.region(&cache, A).is_some() && s.lod(&cache, B).is_some());
        assert_eq!(*store.region(&cache, A).unwrap(), region(A, -7.0 + 6.0 + 1.0));
        let lod = store.lod(&cache, B).unwrap();
        assert_eq!((lod.loc, lod.hash, lod.levels.len()), (B, hash(2), 1));
        assert!(store.region(&cache, B).is_none() && store.lod(&cache, A).is_none());
        assert!(store.error(&cache).is_none());

        let broken = &store.folders[&other];
        assert!(broken.regions.is_empty() && broken.inflight.is_empty());
        assert!(broken.failed.contains_key(&(A, true)), "a broken file in another folder is still not asked for again");
    }

    #[test]
    fn a_tool_waits_for_whole_regions_and_names_what_did_not_load() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        write_world(&cache, &[(A, 1), (B, 2)]);
        let mut store = Terrain3DStore::default();
        assert_eq!(store.load_whole(&cache, &[], Duration::ZERO), Ok(()));
        store.refresh(&cache);
        let early = store.load_whole(&cache, &[B], Duration::ZERO).unwrap_err();
        assert!(early.contains("regions [3, -2] did not load") && !early.contains("error"), "{early}");

        store.load_whole(&cache, &[A, B], Duration::from_secs(10)).unwrap();
        assert_eq!(*store.region(&cache, A).unwrap(), region(A, -7.0 + 6.0 + 1.0));
        assert_eq!(*store.region(&cache, B).unwrap(), region(B, 21.0 - 6.0 + 2.0));
        assert_eq!(store.load_whole(&cache, &[B, A], Duration::ZERO), Ok(()), "what is loaded needs no wait");

        let broken_dir = tempfile::tempdir().unwrap();
        let broken = cache_of(&broken_dir);
        write_world(&broken, &[(A, 1), (B, 2)]);
        std::fs::write(terrain3d_cache::region_path(&broken, A).unwrap(), b"broken").unwrap();
        let read = terrain3d_cache::read_region(&broken, A, 64).unwrap_err().to_string();
        store.refresh(&broken);
        let start = Instant::now();
        let failed = store.load_whole(&broken, &[B, A], Duration::from_millis(1500)).unwrap_err();
        assert!(start.elapsed() >= Duration::from_millis(1500), "a failed region is tried again until the wait ends");
        assert!(failed.contains("regions [-1, 2] did not load") && failed.ends_with(&format!("the last error was: {read}")), "{failed}");
        assert!(store.region(&broken, B).is_some() && store.region(&cache, A).is_some());

        let many: Vec<Loc> = (0..5).map(|k| [k - 2, 7 - k]).collect();
        let five = store.load_whole(&cache, &many, Duration::ZERO).unwrap_err();
        assert!(five.contains("regions [-2, 7], [-1, 6], [0, 5], [1, 4] and 1 more did"), "{five}");
        let four = store.load_whole(&cache, &many[..4], Duration::ZERO).unwrap_err();
        assert!(four.contains("regions [-2, 7], [-1, 6], [0, 5], [1, 4] did"), "{four}");
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

    fn godot_dir(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../godot").join(path)
    }

    /// The cache Godot exported for the valley's region files.
    fn valley_cache() -> PathBuf {
        godot_dir("terrain3d_valley").join(terrain3d_cache::DIR_NAME)
    }

    /// Copies `files` into `dir/data` and returns the cache path of that folder, which has no cache.
    fn data_folder(dir: &tempfile::TempDir, files: &[PathBuf]) -> PathBuf {
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        for file in files {
            std::fs::copy(file, data.join(file.file_name().unwrap())).unwrap();
        }

        data.join(terrain3d_cache::DIR_NAME)
    }

    fn valley_files(dir: &tempfile::TempDir) -> PathBuf {
        let files: Vec<PathBuf> = std::fs::read_dir(godot_dir("terrain3d_valley"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "res"))
            .collect();
        assert_eq!(files.len(), 6);
        data_folder(dir, &files)
    }

    fn set_modified(path: &Path, modified: SystemTime) {
        std::fs::File::options().write(true).open(path).unwrap().set_modified(modified).unwrap();
    }

    #[test]
    fn a_folder_without_a_cache_shows_its_region_files_beside_a_cached_one() {
        let (files_dir, cached_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (files, cached) = (valley_files(&files_dir), cache_of(&cached_dir));
        write_world(&cached, &[(A, 1)]);
        let mut store = Terrain3DStore::default();
        store.refresh(&files);
        store.refresh(&cached);
        assert!(store.manifest(&files).is_none(), "the files are read in the background");
        settle(&mut store, &files, |s| s.manifest(&files).is_some());
        assert!(store.from_files(&files) && !store.from_files(&cached));
        assert!(store.error(&files).is_none(), "{:?}", store.error(&files));

        let exported = terrain3d_cache::read_manifest(&valley_cache()).unwrap();
        let read = store.manifest(&files).unwrap();
        assert_eq!((read.region_size, read.vertex_spacing), (exported.region_size, exported.vertex_spacing));
        assert!((read.data_version - exported.data_version).abs() < 1e-6, "{} {}", read.data_version, exported.data_version);
        assert!(read.textures.is_empty() && read.material.world_background == 0);
        let sorted = |m: &Manifest| {
            let mut regions: Vec<_> = m.regions.iter().map(|r| (r.loc, r.hash, r.height_range)).collect();
            regions.sort_by_key(|r| r.0);
            regions
        };
        assert_eq!(sorted(read), sorted(&exported), "the hashes and height ranges Godot exported");

        let (near, far) = ([0, 0], [1, -1]);
        store.want(&files, &[near], &[far]);
        store.want(&cached, &[A], &[]);
        settle(&mut store, &files, |s| s.region(&files, near).is_some() && s.lod(&files, far).is_some() && s.region(&cached, A).is_some());
        assert_eq!(*store.region(&files, near).unwrap(), terrain3d_cache::read_region(&valley_cache(), near, 256).unwrap().region);
        let far_region = terrain3d_cache::read_region(&valley_cache(), far, 256).unwrap();
        assert_eq!(*store.lod(&files, far).unwrap(), build_lod(&store.grid(&files).unwrap(), &far_region.region, far_region.hash).unwrap());
        assert_eq!(*store.region(&cached, A).unwrap(), region(A, -7.0 + 6.0 + 1.0));
        assert!(!files.exists(), "nothing is written to a folder without a cache");
    }

    #[test]
    fn the_region_files_are_read_with_progress_and_shown_once_all_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let files = valley_files(&dir);
        let mut store = Terrain3DStore::default();
        store.refresh(&files);
        let mut seen = Vec::new();
        settle(&mut store, &files, |s| {
            if let Some(progress) = s.scanning(&files) {
                assert!(s.manifest(&files).is_none(), "no half read folder shows");
                seen.push(progress);
            }

            s.manifest(&files).is_some()
        });
        assert!(!seen.is_empty() && seen.iter().all(|&(done, total)| total == 6 && done <= 6), "{seen:?}");
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0), "{seen:?}");
        assert_eq!(store.manifest(&files).unwrap().regions.len(), 6);
        assert!(store.scanning(&files).is_none());

        store.recheck();
        assert!(!store.refresh(&files), "files that did not change are not read again");
        assert!(store.scanning(&files).is_none());
        assert_eq!(store.finish_scan(&files, Duration::ZERO), Ok(()));

        set_modified(&dir.path().join("data/terrain3d_00_00.res"), SystemTime::now() + Duration::from_secs(60));
        store.recheck();
        store.refresh(&files);
        let late = store.finish_scan(&files, Duration::ZERO).unwrap_err();
        assert!(late.starts_with("the Terrain3D region files are still being read, ") && late.ends_with(" of 6 after 0 s"), "{late}");
        store.finish_scan(&files, Duration::from_secs(10)).unwrap();
        assert_eq!(store.manifest(&files).unwrap().regions.len(), 6);
        assert!(store.take_news(&files).is_empty(), "a file saved again with the same data is no news");
    }

    #[test]
    fn a_damaged_region_file_leaves_out_that_region_only() {
        let dir = tempfile::tempdir().unwrap();
        let files = valley_files(&dir);
        std::fs::write(dir.path().join("data/terrain3d_00_00.res"), b"RSRC broken").unwrap();
        let mut store = Terrain3DStore::default();
        store.refresh(&files);
        settle(&mut store, &files, |s| s.manifest(&files).is_some());
        let locs: BTreeSet<Loc> = store.manifest(&files).unwrap().regions.iter().map(|r| r.loc).collect();
        assert_eq!(locs, BTreeSet::from([[-1, -1], [-1, 0], [0, -1], [1, -1], [1, 0]]));
        let error = store.error(&files).unwrap();
        assert!(error.starts_with("Terrain3D region file terrain3d_00_00.res: ") && !error.contains("Terrain3D region file:"), "{error}");
        store.want(&files, &[[1, 0]], &[]);
        settle(&mut store, &files, |s| s.region(&files, [1, 0]).is_some());

        let broken = tempfile::tempdir().unwrap();
        let none = data_folder(&broken, &[dir.path().join("data/terrain3d_00_00.res")]);
        store.refresh(&none);
        settle(&mut store, &none, |s| s.scanning(&none).is_none());
        assert!(store.manifest(&none).is_none() && !store.from_files(&none));
        assert!(store.error(&none).unwrap().starts_with("Terrain3D region file terrain3d_00_00.res: "));
    }

    #[test]
    fn a_region_file_that_comes_or_goes_reads_the_folder_again() {
        let dir = tempfile::tempdir().unwrap();
        let files = valley_files(&dir);
        let data = dir.path().join("data");
        let mut store = Terrain3DStore::default();
        store.refresh(&files);
        settle(&mut store, &files, |s| s.manifest(&files).is_some());
        assert!(store.take_news(&files).is_empty(), "the first read is no news");

        std::fs::remove_file(data.join("terrain3d_01_00.res")).unwrap();
        std::fs::write(data.join("terrain3d_01_00.res.import"), "").unwrap();
        std::fs::write(data.join("terrain3d_assets.res"), "").unwrap();
        store.recheck();
        store.refresh(&files);
        assert_eq!(store.scanning(&files).map(|p| p.1), Some(5), "only region files count");
        settle(&mut store, &files, |s| s.scanning(&files).is_none());
        assert_eq!(store.manifest(&files).unwrap().regions.len(), 5);
        assert_eq!(store.take_news(&files), vec![[1, 0]]);
        assert!(store.error(&files).is_none());

        for entry in std::fs::read_dir(&data).unwrap().flatten() {
            if entry.file_name().to_string_lossy().starts_with("terrain3d_0") {
                std::fs::remove_file(entry.path()).unwrap();
            }
        }

        store.recheck();
        store.refresh(&files);
        assert_eq!(store.manifest(&files).unwrap().regions.len(), 5, "the last read shows while the files are read again");
        settle(&mut store, &files, |s| s.scanning(&files).is_none());
        assert_eq!(store.manifest(&files).unwrap().regions.iter().map(|r| r.loc).collect::<Vec<_>>(), vec![[-1, -1], [-1, 0]]);

        for loc in [[-1, -1], [-1, 0]] {
            std::fs::remove_file(data.join(format!("{}.res", file_stem(loc).unwrap()))).unwrap();
        }

        store.recheck();
        store.refresh(&files);
        assert!(store.manifest(&files).is_none() && !store.from_files(&files) && store.scanning(&files).is_none());
    }

    #[test]
    fn a_cache_godot_exports_later_takes_over_without_reading_a_region_again() {
        let dir = tempfile::tempdir().unwrap();
        let files = valley_files(&dir);
        let mut store = Terrain3DStore::default();
        store.refresh(&files);
        settle(&mut store, &files, |s| s.manifest(&files).is_some());
        let (near, far) = ([0, 0], [1, -1]);
        store.want(&files, &[near], &[far]);
        settle(&mut store, &files, |s| s.region(&files, near).is_some() && s.lod(&files, far).is_some());
        let (region, lod) = (store.region(&files, near).unwrap(), store.lod(&files, far).unwrap());

        std::fs::create_dir_all(&files).unwrap();
        std::fs::copy(terrain3d_cache::manifest_path(&valley_cache()), terrain3d_cache::manifest_path(&files)).unwrap();
        store.recheck();
        store.refresh(&files);
        assert!(!store.from_files(&files));
        assert_eq!(store.manifest(&files).unwrap().terrain3d, "1.1.0");
        assert!(Arc::ptr_eq(&region, &store.region(&files, near).unwrap()), "the region read from its file stays");
        assert!(Arc::ptr_eq(&lod, &store.lod(&files, far).unwrap()));
        assert!(store.take_news(&files).is_empty(), "the same hashes are no news");

        // A read of the files still running when the cache appears is dropped.
        let racing_dir = tempfile::tempdir().unwrap();
        let racing = valley_files(&racing_dir);
        store.refresh(&racing);
        assert!(store.scanning(&racing).is_some());
        std::fs::create_dir_all(&racing).unwrap();
        terrain3d_cache::write_manifest(&racing, &manifest(&[(A, 1)])).unwrap();
        store.recheck();
        store.refresh(&racing);
        assert!(store.scanning(&racing).is_none());
        std::thread::sleep(Duration::from_millis(300));
        store.refresh(&racing);
        assert!(!store.from_files(&racing));
        assert_eq!(store.manifest(&racing).unwrap().regions.len(), 1);
    }

    #[test]
    fn the_newest_region_file_gives_the_spacing_and_the_sizes_must_match() {
        let fixtures = godot_dir("tests/terrain3d/res_fixtures");
        let dir = tempfile::tempdir().unwrap();
        let files = data_folder(&dir, &[fixtures.join("painted/terrain3d-03_02.res"), fixtures.join("instances/terrain3d_01_00.res")]);
        let data = dir.path().join("data");
        let (painted, instances) = (data.join("terrain3d-03_02.res"), data.join("terrain3d_01_00.res"));
        let then = SystemTime::now() - Duration::from_secs(600);
        set_modified(&painted, then);
        set_modified(&instances, then);
        let mut store = Terrain3DStore::default();
        store.refresh(&files);
        settle(&mut store, &files, |s| s.manifest(&files).is_some());
        let m = store.manifest(&files).unwrap();
        assert_eq!((m.region_size, m.vertex_spacing, m.regions.len()), (64, 2.5, 2), "the first file by name when both are as new");

        store.want(&files, &[[-3, 2]], &[]);
        settle(&mut store, &files, |s| s.region(&files, [-3, 2]).is_some());
        set_modified(&instances, then + Duration::from_secs(1));
        store.recheck();
        store.refresh(&files);
        settle(&mut store, &files, |s| s.scanning(&files).is_none());
        assert_eq!(store.manifest(&files).unwrap().vertex_spacing, 1.0, "the newer file");
        assert!(store.region(&files, [-3, 2]).is_none(), "a new spacing drops what was read");

        std::fs::copy(godot_dir("terrain3d_valley/terrain3d_00_00.res"), data.join("terrain3d_00_00.res")).unwrap();
        store.recheck();
        store.refresh(&files);
        settle(&mut store, &files, |s| s.scanning(&files).is_none());
        assert!(store.error(&files).unwrap().starts_with("the region files have different sizes"), "{:?}", store.error(&files));
        assert_eq!(store.manifest(&files).unwrap().regions.len(), 2, "the last good read stays");
    }
}
