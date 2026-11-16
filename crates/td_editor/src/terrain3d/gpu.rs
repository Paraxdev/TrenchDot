//! What each linked Terrain3D node's clipmap holds on the GPU: a fine layer for the regions near a camera, least
//! recently used first, and a coarse atlas block for every region, uploaded within a byte budget per frame.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use td_core::NodeId;
use td_doc::terrain3d::{RegionSource, Terrain3DLink};
use td_formats::terrain3d_cache::{LodLevel, Manifest, ManifestMaterial, ManifestOcean, ManifestTexture};
use td_geom::terrain3d::{Control, Loc, Region3D};
use td_render::clipmap::look::{Albedo, ClipmapTexture};
use td_render::clipmap::noise::Noise;
use td_render::clipmap::{AutoShader, Background, ClipmapDesc, ClipmapParams, GpuClipmap, Slot, coarse_stride, fine_capacity, region_index};
use td_render::{Renderer, Water};

use super::residency::{self, Source};
use crate::state::{EditorState, Prefs};

/// Bytes a frame uploads at most, though always one region, so a region of 1024 costs one frame.
pub const UPLOAD_BUDGET: usize = 24 << 20;

/// A region as the clipmap is asked to show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub loc: Loc,
    pub source: Source,
    /// Held whole on the CPU, so it can fill a fine layer and its coarse block.
    pub whole: bool,
    /// Its pyramid holds the coarse stride.
    pub pyramid: bool,
}

/// One upload of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upload {
    Coarse(Loc),
    Fine(Loc, u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layer {
    loc: Loc,
    source: Source,
    used: u64,
}

/// The bookkeeping of one clipmap: which region each fine layer and coarse block holds.
#[derive(Clone, Debug, Default)]
pub struct Held {
    layers: Vec<Option<Layer>>,
    coarse: HashMap<Loc, Source>,
    tick: u64,
}

impl Held {
    pub fn new(layers: u32) -> Self {
        Self { layers: vec![None; layers as usize], ..Default::default() }
    }

    pub fn fine_layer(&self, loc: Loc) -> Option<u32> {
        self.layers.iter().position(|l| l.is_some_and(|l| l.loc == loc)).map(|i| i as u32)
    }

    pub fn fine_used(&self) -> usize {
        self.layers.iter().flatten().count()
    }

    pub fn coarse_count(&self) -> usize {
        self.coarse.len()
    }

    /// Whether some layer or block holds data from `source`.
    pub fn holds(&self, source: Source) -> bool {
        self.layers.iter().flatten().any(|l| l.source == source) || self.coarse.values().any(|s| *s == source)
    }

    /// The source the fine layer of `loc` was filled from.
    pub fn fine_source(&self, loc: Loc) -> Option<Source> {
        self.layers.iter().flatten().find(|l| l.loc == loc).map(|l| l.source)
    }

    /// Records that a patch brought the fine layer of `loc`, and its coarse block when it has one, to `source`. Returns
    /// whether it has a coarse block, which the caller then rewrites.
    pub fn patched(&mut self, loc: Loc, source: Source) -> bool {
        for layer in self.layers.iter_mut().flatten().filter(|l| l.loc == loc) {
            layer.source = source;
        }

        let Some(block) = self.coarse.get_mut(&loc) else { return false };
        *block = source;
        true
    }

    /// This frame's uploads, recorded as done: a coarse block for every region that has none, then fine layers for
    /// `fine` nearest first, then the coarse blocks whose source changed. Stops before the upload that would pass
    /// `budget` bytes, unless it is the first. Returns the uploads and whether some were left for the next frame.
    pub fn plan(&mut self, regions: &[Region], fine: &[Loc], bytes: [usize; 2], budget: usize) -> (Vec<Upload>, bool) {
        self.tick += 1;
        let present: HashMap<Loc, &Region> = regions.iter().map(|r| (r.loc, r)).collect();
        let near: Vec<&Region> = fine.iter().filter_map(|l| present.get(l).copied()).take(self.layers.len()).collect();
        let keep: HashSet<Loc> = near.iter().map(|r| r.loc).collect();
        self.coarse.retain(|loc, _| present.contains_key(loc));
        // Only layers of `near` are refilled, and the shader prefers a layer to the coarse block at any distance.
        for layer in &mut self.layers {
            if layer.is_some_and(|l| present.get(&l.loc).is_none_or(|r| r.source != l.source && !keep.contains(&l.loc))) {
                *layer = None;
            }
        }

        let [coarse_bytes, fine_bytes] = bytes;
        let mut out = Vec::new();
        let mut spent = 0usize;
        let mut room = |cost: usize, out: &[Upload]| {
            let fits = out.is_empty() || spent + cost <= budget;
            if fits {
                spent += cost;
            }

            fits
        };

        let missing: Vec<&Region> = regions.iter().filter(|r| !self.coarse.contains_key(&r.loc) && (r.pyramid || r.whole)).collect();
        for r in missing {
            if !room(coarse_bytes, &out) {
                return (out, true);
            }

            self.coarse.insert(r.loc, r.source);
            out.push(Upload::Coarse(r.loc));
        }

        for r in &near {
            let held = self.fine_layer(r.loc);
            if let Some(i) = held {
                let layer = self.layers[i as usize].as_mut().expect("a held layer");
                layer.used = self.tick;
                if layer.source == r.source || !r.whole {
                    continue;
                }
            }

            if !r.whole {
                continue;
            }

            let free = || self.layers.iter().position(Option::is_none);
            let stale = || {
                let unwanted = self.layers.iter().enumerate().filter(|(_, l)| l.is_some_and(|l| !keep.contains(&l.loc)));
                unwanted.min_by_key(|(_, l)| l.map(|l| l.used)).map(|(i, _)| i)
            };
            let Some(i) = held.map(|i| i as usize).or_else(free).or_else(stale) else { continue };
            if !room(fine_bytes, &out) {
                return (out, true);
            }

            self.layers[i] = Some(Layer { loc: r.loc, source: r.source, used: self.tick });
            out.push(Upload::Fine(r.loc, i as u32));
        }

        for r in regions {
            let stale = self.coarse.get(&r.loc).is_some_and(|s| *s != r.source);
            let ready = r.whole || (r.pyramid && !matches!(r.source, Source::Edited(_)));
            if stale && ready {
                if !room(coarse_bytes, &out) {
                    return (out, true);
                }

                self.coarse.insert(r.loc, r.source);
                out.push(Upload::Coarse(r.loc));
            }
        }

        (out, false)
    }

    /// What the region map should say of each region: its fine layer, else its coarse block, else absent.
    pub fn slots(&self, regions: &[Region]) -> Vec<(Loc, Slot)> {
        regions
            .iter()
            .map(|r| {
                let slot = match self.fine_layer(r.loc) {
                    Some(layer) => Slot::Fine(layer),
                    None if self.coarse.contains_key(&r.loc) => Slot::Coarse,
                    None => Slot::Absent,
                };
                (r.loc, slot)
            })
            .collect()
    }
}

/// The coarse texels of a pyramid level at the clipmap's stride, `region_size / stride` squared row by row. The splat
/// weights become a control word: the strongest texture as base, the second as overlay blended by its share of the
/// two, and the hole bit. None when the level is not at `stride`.
pub fn coarse_from_level(level: &LodLevel, region_size: u32, stride: u32) -> Option<(Vec<f32>, Vec<u32>)> {
    if level.stride != stride || stride == 0 || !region_size.is_multiple_of(stride) || level.side < region_size / stride {
        return None;
    }

    let cs = (region_size / stride) as usize;
    let side = level.side as usize;
    let mut heights = Vec::with_capacity(cs * cs);
    let mut control = Vec::with_capacity(cs * cs);
    for j in 0..cs {
        for i in 0..cs {
            let k = j * side + i;
            heights.push(level.heights[k]);
            let mut c = Control(0).with_base(level.ids.first().copied().unwrap_or(0));
            if let Some(w) = level.splat.get(k * 4..k * 4 + 4) {
                let mut order: Vec<usize> = (0..level.ids.len().min(4)).collect();
                order.sort_by_key(|&ch| std::cmp::Reverse(w[ch]));
                if let [first, rest @ ..] = order.as_slice() {
                    c = c.with_base(level.ids[*first]).with_overlay(level.ids[*first]);
                    if let Some(&second) = rest.first().filter(|&&s| w[s] > 0) {
                        let share = w[second] as f32 / (w[*first] as f32 + w[second] as f32);
                        c = c.with_overlay(level.ids[second]).with_blend((share * 255.0).round() as u8);
                    }
                }
            }

            control.push(c.with_hole(level.holes.get(k).is_some_and(|h| *h != 0)).0);
        }
    }

    Some((heights, control))
}

/// A rectangle of a region's cells, origin and size.
pub type Rect = ([u32; 2], [u32; 2]);

/// The smallest rectangle outside which `new` holds what `old` does, None when nothing changed. Heights compare by
/// their bits, so a NaN left alone counts as unchanged. Regions of another shape differ whole.
pub fn changed_rect(old: &Region3D, new: &Region3D) -> Option<Rect> {
    let side = new.size as usize;
    let n = side * side;
    let shaped = |r: &Region3D| r.size == new.size && r.heights.len() == n && r.control.len() == n && r.color.len() == new.color.len();
    if !shaped(old) || !shaped(new) {
        return Some(([0, 0], [new.size, new.size]));
    }

    let colored = new.color.len() == n * 4;
    let same = |a: usize, b: usize| {
        old.heights[a..b].iter().zip(&new.heights[a..b]).all(|(x, y)| x.to_bits() == y.to_bits())
            && old.control[a..b] == new.control[a..b]
            && (!colored || old.color[a * 4..b * 4] == new.color[a * 4..b * 4])
    };
    let mut out: Option<[usize; 4]> = None;
    for j in 0..side {
        let row = j * side;
        if same(row, row + side) {
            continue;
        }

        let first = (0..side).find(|i| !same(row + i, row + i + 1)).expect("a changed row");
        let last = (0..side).rev().find(|i| !same(row + i, row + i + 1)).expect("a changed row");
        out = Some(out.map_or([first, j, last, j], |[x0, y0, x1, _]| [x0.min(first), y0, x1.max(last), j]));
    }

    out.map(|[x0, y0, x1, y1]| ([x0 as u32, y0 as u32], [(x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32]))
}

/// The values of `rect` in `data` laid out `side` per row, row by row.
fn cut<T: Copy>(data: &[T], side: u32, ([x, y], [w, h]): Rect) -> Vec<T> {
    (y..y + h)
        .flat_map(|j| {
            let at = (j * side + x) as usize;
            data[at..at + w as usize].iter().copied()
        })
        .collect()
}

/// The data the fine layer of `loc` holds when it was filled from `from` and the region is now an edit, so a patch can
/// bring it up to date: the edit uploaded before, or the cached region the edit started from.
fn patch_base(link: &Terrain3DLink, source: &dyn RegionSource, loc: Loc, from: Source, pins: &HashMap<usize, Arc<Region3D>>) -> Option<Arc<Region3D>> {
    let edit = link.live_edit(source, loc)?;
    match from {
        Source::Edited(p) => pins.get(&p).cloned(),
        Source::Cached(hash) => edit.base.clone().filter(|_| edit.base_hash == Some(hash)),
        Source::Absent => None,
    }
}

/// The regions of a linked node the clipmap can show, in the order of `link.locs`.
pub fn regions(state: &EditorState, id: NodeId, cache: &Path) -> Vec<Region> {
    let (Some(link), Some(source), Some(manifest)) = (state.doc.map.terrain3d(id), state.terrain3d.source(cache), state.terrain3d.manifest(cache)) else {
        return Vec::new();
    };
    let size = manifest.region_size;
    let stride = coarse_stride(size);
    let hashes: HashMap<Loc, [u8; 32]> = manifest.regions.iter().map(|r| (r.loc, r.hash.0)).collect();
    let region = |loc: Loc| {
        if let Some(edit) = link.live_edit(&source, loc) {
            return Some(Region { loc, source: Source::Edited(Arc::as_ptr(&edit.region) as usize), whole: true, pyramid: false });
        }

        let hash = *hashes.get(&loc)?;
        let lod = state.terrain3d.lod(cache, loc);
        let pyramid = residency::pyramid_has_coarse(size) && lod.is_some_and(|l| l.hash.0 == hash && l.levels.iter().any(|v| v.stride == stride));
        Some(Region { loc, source: Source::Cached(hash), whole: source.region(loc).is_some(), pyramid })
    };
    link.locs(&source).into_iter().filter(|l| region_index(*l).is_some()).filter_map(region).collect()
}

/// What a node's clipmap holds, for `terrain3d info`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub fine_used: usize,
    pub fine_layers: u32,
    pub coarse_regions: usize,
    pub gpu_bytes: u64,
    pub uploads: Uploads,
}

/// Fine layer uploads since the clipmap was made: whole regions, and patches with the cells they wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Uploads {
    pub whole: u64,
    pub partial: u64,
    pub partial_cells: u64,
}

/// One linked node's clipmap and what it was filled from.
pub struct NodeClipmap {
    clip: GpuClipmap,
    held: Held,
    textures: Option<Vec<ManifestTexture>>,
    /// The edited regions held data came from. Keeping them alive keeps their addresses, which key that data, from going
    /// to a later edit.
    pins: HashMap<usize, Arc<Region3D>>,
    shown: Vec<Loc>,
    uploads: Uploads,
    water: Option<Water>,
}

impl NodeClipmap {
    pub fn clip(&self) -> &GpuClipmap {
        &self.clip
    }

    pub fn water(&self) -> Option<Water> {
        self.water
    }

    pub fn status(&self) -> Status {
        let desc = self.clip.desc();
        let [w, h] = desc.atlas_size();
        let fine = 12 * u64::from(desc.region_size).pow(2) * u64::from(desc.layers);
        Status {
            fine_used: self.held.fine_used(),
            fine_layers: desc.layers,
            coarse_regions: self.held.coarse_count(),
            gpu_bytes: fine + 8 * u64::from(w) * u64::from(h),
            uploads: self.uploads,
        }
    }
}

/// Brings a node's clipmap up to what the store and the edits hold, uploading at most [`UPLOAD_BUDGET`] bytes, and drops
/// it when the node has no region to show. Returns whether uploads are left for the next frame, or why the clipmap
/// could not be made.
pub fn sync(
    renderer: &Renderer,
    slot: &mut Option<NodeClipmap>,
    state: &EditorState,
    id: NodeId,
    cache: &Path,
    selected: bool,
    wire: [f32; 4],
) -> Result<bool, String> {
    let regions = regions(state, id, cache);
    let (Some(link), Some(source), Some(manifest), false) =
        (state.doc.map.terrain3d(id), state.terrain3d.source(cache), state.terrain3d.manifest(cache), regions.is_empty())
    else {
        *slot = None;
        return Ok(false);
    };
    let size = manifest.region_size;
    let stride = coarse_stride(size);
    let lo = [0, 1].map(|i| regions.iter().map(|r| r.loc[i]).min().unwrap_or(0));
    let hi = [0, 1].map(|i| regions.iter().map(|r| r.loc[i]).max().unwrap_or(0));
    let desc = ClipmapDesc { region_size: size, layers: fine_capacity(size, regions.len()), coarse_stride: stride, atlas: [lo, hi] };
    let fits = slot.as_ref().is_some_and(|n| {
        let d = n.clip.desc();
        d.region_size == size && d.coarse_stride == stride && d.layers >= desc.layers && d.in_atlas(lo) && d.in_atlas(hi)
    });
    if !fits {
        *slot = None;
        let clip = renderer.new_clipmap(&desc)?;
        *slot = Some(NodeClipmap {
            clip,
            held: Held::new(desc.layers),
            textures: None,
            pins: HashMap::new(),
            shown: Vec::new(),
            uploads: Uploads::default(),
            water: None,
        });
    }

    let Some(node) = slot.as_mut() else { return Ok(false) };
    if node.textures.as_ref() != Some(&manifest.textures) {
        renderer.set_clipmap_textures(&mut node.clip, &textures(cache, &manifest.textures));
        node.textures = Some(manifest.textures.clone());
    }

    node.water = water(state.terrain3d.upm, manifest.ocean.as_ref());
    let ranges: HashMap<usize, [f32; 2]> = regions
        .iter()
        .filter_map(|r| match r.source {
            Source::Edited(p) => Some((p, state.terrain3d.region_range(&link.region(&source, r.loc)?))),
            _ => None,
        })
        .collect();
    let params = params(state.terrain3d.upm, manifest, &regions, &ranges, selected, wire, &state.prefs);
    if *node.clip.params() != params {
        node.clip.set_params(&renderer.queue, &params);
    }

    let texels = (size / stride) as usize;
    let bytes = [texels * texels * 8, size as usize * size as usize * 12];
    let mut spent = 0;
    for r in &regions {
        let Some(from) = node.held.fine_source(r.loc).filter(|s| *s != r.source) else { continue };
        let (Some(old), Some(new), Some(layer)) =
            (patch_base(link, &source, r.loc, from, &node.pins), link.region(&source, r.loc), node.held.fine_layer(r.loc))
        else {
            continue;
        };
        if let Some(rect @ (origin, [w, h])) = changed_rect(&old, &new) {
            let (heights, control) = (cut(&new.heights, new.size, rect), cut(&new.control, new.size, rect));
            if let Err(e) = node.clip.write_fine(&renderer.queue, layer, origin, [w, h], &heights, &control, &colors(&new, rect)) {
                eprintln!("Terrain3D clipmap: {e}");
                continue;
            }

            node.uploads.partial += 1;
            node.uploads.partial_cells += u64::from(w * h);
            spent += (w * h) as usize * 12;
        }

        node.pins.insert(Arc::as_ptr(&new) as usize, new.clone());
        if node.held.patched(r.loc, r.source)
            && let Some((h, c)) = new.coarse(stride)
        {
            if let Err(e) = node.clip.write_coarse(&renderer.queue, r.loc, &h, &c) {
                eprintln!("Terrain3D clipmap: {e}");
            }

            spent += bytes[0];
        }
    }

    let (uploads, more) = node.held.plan(&regions, state.terrain3d_residency.fine(id), bytes, UPLOAD_BUDGET.saturating_sub(spent));
    for upload in uploads {
        let loc = match upload {
            Upload::Coarse(loc) | Upload::Fine(loc, _) => loc,
        };
        let whole = link.region(&source, loc);
        if let Some(region) = whole.as_ref().filter(|_| link.live_edit(&source, loc).is_some()) {
            node.pins.insert(Arc::as_ptr(region) as usize, region.clone());
        }

        let written = match (upload, whole) {
            (Upload::Coarse(_), Some(region)) => region.coarse(stride).map(|(h, c)| node.clip.write_coarse(&renderer.queue, loc, &h, &c)),
            (Upload::Coarse(_), None) => state
                .terrain3d
                .lod(cache, loc)
                .and_then(|l| l.levels.iter().find(|v| v.stride == stride).and_then(|v| coarse_from_level(v, size, stride)))
                .map(|(h, c)| node.clip.write_coarse(&renderer.queue, loc, &h, &c)),
            (Upload::Fine(_, layer), Some(region)) => {
                let written = node.clip.write_fine(
                    &renderer.queue,
                    layer,
                    [0, 0],
                    [size, size],
                    &region.heights,
                    &region.control,
                    &colors(&region, ([0, 0], [size, size])),
                );
                node.uploads.whole += u64::from(written.is_ok());
                Some(written)
            }
            (Upload::Fine(..), None) => None,
        };
        if let Some(Err(e)) = written {
            eprintln!("Terrain3D clipmap: {e}");
        }
    }

    node.pins.retain(|p, _| node.held.holds(Source::Edited(*p)));
    let present: HashSet<Loc> = regions.iter().map(|r| r.loc).collect();
    let mut slots: Vec<(Loc, Slot)> = node.held.slots(&regions).into_iter().filter(|(l, s)| node.clip.slot(*l) != *s).collect();
    slots.extend(node.shown.iter().filter(|l| !present.contains(*l)).map(|l| (*l, Slot::Absent)));
    if !slots.is_empty()
        && let Err(e) = node.clip.set_slots(&renderer.queue, &slots)
    {
        eprintln!("Terrain3D clipmap: {e}");
    }

    node.shown = regions.iter().map(|r| r.loc).collect();
    Ok(more)
}

/// The color map of a region within `rect`, Terrain3D's blank color where it has none.
fn colors(region: &Region3D, rect: Rect) -> Vec<[u8; 4]> {
    let [w, h] = rect.1;
    if region.color.len() != region.heights.len() * 4 {
        return vec![td_geom::terrain3d::BLANK_COLOR; (w * h) as usize];
    }

    cut(region.color.as_chunks::<4>().0, region.size, rect)
}

/// The manifest's textures as the clipmap draws them: the exported albedo, else the thumbnail, else the color alone.
fn textures(cache: &Path, list: &[ManifestTexture]) -> Vec<ClipmapTexture> {
    let open = |file: &str| (!file.is_empty()).then(|| image::open(cache.join(file)).ok()).flatten().map(|i| i.to_rgba8());
    list.iter()
        .map(|t| {
            let albedo = match (open(&t.albedo), open(&t.thumbnail)) {
                (Some(full), _) => Albedo::Full(full),
                (None, Some(small)) => Albedo::Thumbnail(small),
                (None, None) => Albedo::None,
            };
            let [r, g, b, _] = t.albedo_color;
            ClipmapTexture { id: t.id, albedo, color: [r, g, b], uv_scale: t.uv_scale, detiling: [t.detiling_rotation, t.detiling_shift] }
        })
        .collect()
}

/// The ocean's water at its sea level, its color made linear. None when the node has no ocean or it is off.
fn water(upm: f64, ocean: Option<&ManifestOcean>) -> Option<Water> {
    let ocean = ocean.filter(|o| o.enabled)?;
    let [r, g, b, a] = ocean.water_color;
    Some(Water { level: (ocean.sea_level * upm) as f32, color: [td_render::srgb_to_linear(r), td_render::srgb_to_linear(g), td_render::srgb_to_linear(b), a] })
}

/// A cache from before the noise values were exported draws a Noise background flat.
fn background(m: &ManifestMaterial) -> Background {
    match (m.world_background, &m.world_noise) {
        (2, Some(n)) => Background::Noise(Noise {
            scale: n.scale as f32,
            height: n.height as f32,
            offset: n.offset.map(|v| v as f32),
            lod_distance: n.lod_distance as f32,
            max_octaves: n.max_octaves,
            min_octaves: n.min_octaves,
        }),
        (1 | 2, _) => Background::Flat,
        _ => Background::None,
    }
}

fn params(
    upm: f64,
    manifest: &Manifest,
    regions: &[Region],
    ranges: &HashMap<usize, [f32; 2]>,
    selected: bool,
    wire: [f32; 4],
    prefs: &Prefs,
) -> ClipmapParams {
    let m = &manifest.material;
    let background = background(m);
    let cached: HashMap<Loc, [f32; 2]> = manifest.regions.iter().map(|r| (r.loc, r.height_range)).collect();
    let mut range = [f32::INFINITY, f32::NEG_INFINITY];
    let mut take = |[lo, hi]: [f32; 2]| range = [range[0].min(lo), range[1].max(hi)];
    for r in regions {
        let known = match r.source {
            Source::Edited(p) => ranges.get(&p),
            _ => cached.get(&r.loc),
        };
        known.copied().into_iter().for_each(&mut take);
    }

    if background != Background::None {
        take([m.ground_level as f32; 2]);
    }

    if let Background::Noise(noise) = background {
        let [lo, hi] = noise.range();
        range = [range[0] + lo, range[1] + hi];
    }

    let upm = upm as f32;
    let heights = if range[0] <= range[1] { range.map(|h| h * upm) } else { [0.0; 2] };
    let auto_shader = m.auto_shader.then(|| {
        let a = m.auto.clone().unwrap_or_default();
        AutoShader { slope: a.slope as f32, height_reduction: a.height_reduction as f32, base: a.base_texture, overlay: a.overlay_texture }
    });
    ClipmapParams {
        cell: (manifest.vertex_spacing * upm as f64) as f32,
        upm,
        background,
        background_shown: prefs.terrain3d_background,
        ground_level: m.ground_level as f32,
        region_blend: m.region_blend as f32,
        heights,
        selected,
        wire,
        textures: prefs.terrain3d_textures,
        auto_shader,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(loc: Loc, hash: u8) -> Region {
        Region { loc, source: Source::Cached([hash; 32]), whole: true, pyramid: true }
    }

    #[test]
    fn the_background_follows_the_material_and_widens_the_cull_range() {
        use td_formats::terrain3d_cache::{self, ManifestRegion, RegionHash, WorldNoise};

        let ranges = [([-2, 3], [4.0, 20.0]), ([5, -1], [-6.0, 1.5])];
        let mut manifest = Manifest {
            format: terrain3d_cache::FORMAT.into(),
            version: terrain3d_cache::VERSION,
            terrain3d: "1.1.0".into(),
            godot: "4.7.2".into(),
            data_version: 1.1,
            region_size: 64,
            vertex_spacing: 0.5,
            material: ManifestMaterial { ground_level: 30.0, ..Default::default() },
            map_transform_identity: true,
            editor_open: false,
            regions: ranges.iter().map(|&(loc, height_range)| ManifestRegion { loc, hash: RegionHash([1; 32]), height_range }).collect(),
            dirty: Vec::new(),
            textures: Vec::new(),
            applied: Vec::new(),
            ocean: None,
            resources: None,
        };
        let regions = [region([-2, 3], 1), region([5, -1], 1)];
        let drawn = |manifest: &Manifest| {
            let p = params(32.0, manifest, &regions, &HashMap::new(), false, [1.0; 4], &Prefs::default());
            (p.background, p.heights)
        };
        assert_eq!(drawn(&manifest), (Background::None, [-6.0 * 32.0, 20.0 * 32.0]), "no background leaves out the ground level");
        manifest.material.world_background = 1;
        assert_eq!(drawn(&manifest), (Background::Flat, [-6.0 * 32.0, 30.0 * 32.0]));
        manifest.material.world_background = 2;
        assert_eq!(drawn(&manifest), (Background::Flat, [-6.0 * 32.0, 30.0 * 32.0]), "an old cache without noise values draws flat");

        let offset = [0.25, 0.5, -1.0];
        manifest.material.world_noise =
            Some(WorldNoise { scale: 3.5, height: -20.0, offset, lod_distance: 600.0, max_octaves: 3, min_octaves: 1, fragment_normals: true });
        let noise = Noise { scale: 3.5, height: -20.0, offset: offset.map(|v| v as f32), lod_distance: 600.0, max_octaves: 3, min_octaves: 1 };
        // The noise reaches from 1.75 * -20 * 10 + 50 = -300 meters to the 50 of its offset, on top of -6 to 30.
        assert_eq!(drawn(&manifest), (Background::Noise(noise), [-306.0 * 32.0, 80.0 * 32.0]));
        manifest.material.world_background = 0;
        assert_eq!(drawn(&manifest), (Background::None, [-6.0 * 32.0, 20.0 * 32.0]), "noise values without the mode draw nothing");
        let shown = |prefs: &Prefs| {
            let p = params(32.0, &manifest, &regions, &HashMap::new(), false, [1.0; 4], prefs);
            (p.textures, p.background_shown)
        };
        let off = |t: bool, b: bool| Prefs { terrain3d_textures: t, terrain3d_background: b, ..Prefs::default() };
        assert_eq!(shown(&Prefs::default()), (true, true));
        assert_eq!((shown(&off(false, true)), shown(&off(true, false)), shown(&off(false, false))), ((false, true), (true, false), (false, false)));
    }

    #[test]
    fn an_ocean_that_is_on_gives_water_at_its_sea_level_in_linear_color() {
        let ocean =
            |enabled: bool, sea_level: f64| ManifestOcean { enabled, sea_level, water_color: [0.5, 0.04045, 1.0, 0.75], source: "res://sea.tres".into() };
        assert_eq!(water(32.0, None), None, "no ocean");
        assert_eq!(water(32.0, Some(&ocean(false, 5.0))), None, "an ocean that is off");
        let sea = water(32.0, Some(&ocean(true, -2.5))).unwrap();
        assert_eq!(sea.level, -80.0);
        let want = [0.21404114, 0.04045 / 12.92, 1.0, 0.75];
        assert!(sea.color.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6), "{:?}", sea.color);
        assert_eq!(water(40.0, Some(&ocean(true, 5.0))).unwrap().level, 200.0, "the level follows the units per meter");
        assert_eq!(water(32.0, Some(&ocean(true, 0.0))).unwrap().level, 0.0);
    }

    #[test]
    fn coarse_blocks_come_first_then_fine_layers_nearest_first() {
        let regions = [region([-2, 1], 1), region([3, -1], 2), region([0, 0], 3)];
        let mut held = Held::new(2);
        let (ups, more) = held.plan(&regions, &[[0, 0], [-2, 1], [3, -1]], [10, 100], usize::MAX);
        use Upload::*;
        assert_eq!(ups, vec![Coarse([-2, 1]), Coarse([3, -1]), Coarse([0, 0]), Fine([0, 0], 0), Fine([-2, 1], 1)]);
        assert!(!more);
        assert_eq!(held.slots(&regions), vec![([-2, 1], Slot::Fine(1)), ([3, -1], Slot::Coarse), ([0, 0], Slot::Fine(0))]);
        assert_eq!(held.plan(&regions, &[[0, 0], [-2, 1]], [10, 100], usize::MAX), (vec![], false), "nothing changed, nothing uploads");
        assert_eq!((held.fine_used(), held.coarse_count()), (2, 3));
    }

    #[test]
    fn with_one_region_more_than_layers_the_least_recently_used_layer_is_reused() {
        let regions: Vec<Region> = (0..4).map(|k| region([k - 1, 2 * k - 3], k as u8)).collect();
        let locs: Vec<Loc> = regions.iter().map(|r| r.loc).collect();
        let mut held = Held::new(3);
        held.plan(&regions, &locs[..3], [10, 100], usize::MAX);
        held.plan(&regions, &[locs[2], locs[0]], [10, 100], usize::MAX);
        let (ups, _) = held.plan(&regions, &[locs[3], locs[2], locs[0]], [10, 100], usize::MAX);
        assert_eq!(ups, vec![Upload::Fine(locs[3], 1)], "region 1 was used least recently");
        assert_eq!(held.fine_layer(locs[1]), None);
        assert_eq!(held.slots(&regions)[1], (locs[1], Slot::Coarse), "an evicted region falls back to its coarse block");

        let (ups, _) = held.plan(&regions, &[locs[1], locs[3], locs[2], locs[0]], [10, 100], usize::MAX);
        assert_eq!(ups, vec![Upload::Fine(locs[1], 0)], "only the three nearest wanted regions keep layers");
        let (ups, _) = held.plan(&regions, &[locs[0], locs[1], locs[3]], [10, 100], usize::MAX);
        assert_eq!(ups, vec![Upload::Fine(locs[0], 2)], "region 2 is no longer wanted, so its layer goes");
    }

    #[test]
    fn the_budget_stops_before_the_upload_that_passes_it_but_never_before_the_first() {
        let regions: Vec<Region> = (0..3).map(|k| region([2 * k - 1, 1 - k], k as u8)).collect();
        let locs: Vec<Loc> = regions.iter().map(|r| r.loc).collect();
        let mut held = Held::new(3);
        let (ups, more) = held.plan(&regions, &locs, [10, 100], 5);
        assert_eq!((ups, more), (vec![Upload::Coarse(locs[0])], true), "one upload even over the budget");
        let (ups, more) = held.plan(&regions, &locs, [10, 100], 120);
        assert_eq!((ups, more), (vec![Upload::Coarse(locs[1]), Upload::Coarse(locs[2]), Upload::Fine(locs[0], 0)], true));
        let (ups, more) = held.plan(&regions, &locs, [10, 100], 200);
        assert_eq!((ups, more), (vec![Upload::Fine(locs[1], 1), Upload::Fine(locs[2], 2)], false), "exactly the budget fits");
    }

    #[test]
    fn a_changed_source_reuploads_its_layer_and_block_and_a_removed_region_frees_both() {
        let mut regions = vec![region([5, -4], 1), region([-6, 7], 2)];
        let mut held = Held::new(2);
        held.plan(&regions, &[[5, -4], [-6, 7]], [10, 100], usize::MAX);
        regions[0].source = Source::Edited(0x40);
        regions[1].whole = false;
        regions[1].source = Source::Cached([9; 32]);
        let (ups, _) = held.plan(&regions, &[[5, -4], [-6, 7]], [10, 100], usize::MAX);
        assert_eq!(ups, vec![Upload::Fine([5, -4], 0), Upload::Coarse([5, -4]), Upload::Coarse([-6, 7])]);
        assert_eq!(held.slots(&regions)[1], ([-6, 7], Slot::Fine(1)), "a stale layer still shows until the region loads whole");

        let gone = [regions[1]];
        assert_eq!(held.plan(&gone, &[[5, -4], [-6, 7]], [10, 100], usize::MAX).0, vec![]);
        assert_eq!((held.fine_layer([5, -4]), held.fine_used(), held.coarse_count()), (None, 1, 1));
        assert_eq!(held.slots(&[]), vec![]);
    }

    #[test]
    fn a_changed_region_that_gets_no_new_layer_this_frame_falls_back_to_its_coarse_block() {
        let mut regions = vec![Region { source: Source::Edited(0x50), ..region([7, -2], 1) }, region([-3, 4], 2), region([1, 6], 3)];
        let mut held = Held::new(2);
        held.plan(&regions, &[[7, -2], [-3, 4]], [10, 100], usize::MAX);
        regions[0].source = Source::Cached([1; 32]);
        assert_eq!(held.plan(&regions, &[], [10, 100], usize::MAX), (vec![Upload::Coarse([7, -2])], false), "an undone edit far away");
        assert_eq!(held.slots(&regions), vec![([7, -2], Slot::Coarse), ([-3, 4], Slot::Fine(1)), ([1, 6], Slot::Coarse)], "an unchanged layer stays");

        regions[1].source = Source::Edited(0x60);
        regions[2].whole = false;
        let (ups, _) = held.plan(&regions, &[[7, -2], [1, 6], [-3, 4]], [10, 100], usize::MAX);
        assert_eq!(ups, vec![Upload::Fine([7, -2], 0), Upload::Coarse([-3, 4])]);
        assert_eq!(held.slots(&regions)[1], ([-3, 4], Slot::Coarse), "a changed region past the layer count loses its layer too");
    }

    #[test]
    fn a_region_with_neither_whole_data_nor_a_pyramid_stays_absent() {
        let r = Region { loc: [1, 1], source: Source::Cached([1; 32]), whole: false, pyramid: false };
        let mut held = Held::new(1);
        assert_eq!(held.plan(&[r], &[[1, 1]], [10, 100], usize::MAX), (vec![], false));
        assert_eq!(held.slots(&[r]), vec![([1, 1], Slot::Absent)]);
        let edited = Region { source: Source::Edited(8), pyramid: true, ..r };
        held.plan(&[Region { pyramid: true, ..r }], &[], [10, 100], usize::MAX);
        assert_eq!(held.plan(&[edited], &[], [10, 100], usize::MAX).0, vec![], "an edit not held whole keeps the block it had");
    }

    #[test]
    fn a_patch_moves_the_layer_and_block_to_the_new_source_and_lets_the_old_one_go() {
        let regions = [region([4, -3], 1), region([-5, 2], 2)];
        let mut held = Held::new(1);
        held.plan(&regions, &[[4, -3]], [10, 100], usize::MAX);
        assert_eq!((held.fine_source([4, -3]), held.fine_source([-5, 2])), (Some(Source::Cached([1; 32])), None));

        assert!(held.patched([4, -3], Source::Edited(0x70)), "the patched region has a coarse block to rewrite");
        assert_eq!(held.fine_source([4, -3]), Some(Source::Edited(0x70)));
        assert!(!held.holds(Source::Cached([1; 32])) && held.holds(Source::Edited(0x70)));
        let edited = [Region { source: Source::Edited(0x70), ..regions[0] }, regions[1]];
        assert_eq!(held.plan(&edited, &[[4, -3]], [10, 100], usize::MAX), (vec![], false), "a patched region needs no upload");

        let mut lone = Held::new(1);
        lone.plan(&regions[..1], &[], [10, 100], 5);
        assert!(!lone.patched([-5, 2], Source::Edited(0x80)), "nothing held, nothing patched");
        assert!(!lone.holds(Source::Edited(0x80)));
    }

    /// A slanted region off the origin with a color map, different in every cell.
    fn ground(size: u32) -> Region3D {
        let mut r = Region3D::blank([-3, 2], size);
        for (k, (h, c)) in r.heights.iter_mut().zip(r.control.iter_mut()).enumerate() {
            let (i, j) = ((k as u32 % size) as f32, (k as u32 / size) as f32);
            *h = 1.5 + 0.25 * i - 0.125 * j;
            *c = Control(0).with_base((k % 7) as u8).0;
        }

        r.color = (0..r.heights.len() * 4).map(|b| (b * 37 % 251) as u8).collect();
        r
    }

    fn at(size: u32, i: u32, j: u32) -> usize {
        (j * size + i) as usize
    }

    #[test]
    fn one_changed_cell_is_a_rectangle_of_one() {
        let old = ground(16);
        let mut new = old.clone();
        new.heights[at(16, 5, 9)] += 0.5;
        assert_eq!(changed_rect(&old, &new), Some(([5, 9], [1, 1])));
    }

    #[test]
    fn a_slanted_stroke_is_bounded_by_its_first_and_last_cell_whatever_changed() {
        let old = ground(16);
        let mut new = old.clone();
        for t in 0..8 {
            new.heights[at(16, 2 + t, 3 + t * 4 / 7)] -= 0.75;
        }

        new.control[at(16, 11, 4)] = Control(0).with_base(12).0;
        new.color[at(16, 6, 10) * 4 + 2] ^= 0xff;
        assert_eq!(changed_rect(&old, &new), Some(([2, 3], [10, 8])), "heights reach 2..=9 and 3..=7, control 11, color row 10");
    }

    #[test]
    fn an_unchanged_region_has_no_rectangle_even_with_nan_heights() {
        let mut old = ground(16);
        old.heights[at(16, 7, 7)] = f32::NAN;
        assert_eq!(changed_rect(&old, &old.clone()), None);
        let mut plain = ground(1);
        plain.color.clear();
        assert_eq!(changed_rect(&plain, &plain.clone()), None, "a region of one vertex without color");
    }

    #[test]
    fn a_region_changed_everywhere_or_reshaped_is_one_whole_rectangle() {
        let old = ground(16);
        let mut new = old.clone();
        new.heights.iter_mut().for_each(|h| *h += 1.0);
        assert_eq!(changed_rect(&old, &new), Some(([0, 0], [16, 16])));

        let mut uncolored = old.clone();
        uncolored.color.clear();
        assert_eq!(changed_rect(&uncolored, &old), Some(([0, 0], [16, 16])), "a color map gained");
        assert_eq!(changed_rect(&ground(8), &old), Some(([0, 0], [16, 16])), "another size");
    }

    #[test]
    fn cells_on_opposite_borders_span_the_region_between_them() {
        let old = ground(16);
        let mut new = old.clone();
        new.heights[at(16, 0, 4)] = 9.0;
        new.control[at(16, 15, 11)] ^= 1;
        assert_eq!(changed_rect(&old, &new), Some(([0, 4], [16, 8])), "west and east");

        let mut new = old.clone();
        new.heights[at(16, 7, 0)] = 9.0;
        new.color[at(16, 3, 15) * 4] ^= 1;
        assert_eq!(changed_rect(&old, &new), Some(([3, 0], [5, 16])), "north and south");

        let mut new = old.clone();
        new.heights[at(16, 15, 15)] = -9.0;
        assert_eq!(changed_rect(&old, &new), Some(([15, 15], [1, 1])), "the last cell");
    }

    #[test]
    fn a_rectangle_is_cut_row_by_row_with_the_blank_color_for_a_region_without_color() {
        let mut r = ground(16);
        let rect = ([13, 6], [3, 2]);
        let want: Vec<f32> = [(13, 6), (14, 6), (15, 6), (13, 7), (14, 7), (15, 7)].iter().map(|(i, j)| r.heights[at(16, *i, *j)]).collect();
        assert_eq!(cut(&r.heights, 16, rect), want);
        let k = at(16, 15, 7) * 4;
        assert_eq!(colors(&r, rect)[5], [r.color[k], r.color[k + 1], r.color[k + 2], r.color[k + 3]]);
        r.color.clear();
        assert_eq!(colors(&r, rect), vec![td_geom::terrain3d::BLANK_COLOR; 6]);
    }

    fn level(stride: u32, side: u32) -> LodLevel {
        let n = (side * side) as usize;
        LodLevel { stride, side, ids: vec![3, 9, 17], heights: (0..n).map(|k| k as f32 * 0.5 - 7.0).collect(), splat: vec![0; n * 4], holes: vec![0; n] }
    }

    #[test]
    fn splat_weights_become_base_overlay_and_blend_and_holes_the_hole_bit() {
        let mut l = level(4, 17);
        l.splat[..4].copy_from_slice(&[60, 200, 0, 0]);
        l.splat[4..8].copy_from_slice(&[0, 0, 255, 0]);
        l.splat[(17 + 2) * 4..(17 + 2) * 4 + 4].copy_from_slice(&[90, 90, 30, 0]);
        l.holes[17 * 3 + 5] = 1;
        let (heights, control) = coarse_from_level(&l, 64, 4).unwrap();
        assert_eq!((heights.len(), control.len()), (256, 256));
        assert_eq!(heights[16 * 3 + 5], l.heights[17 * 3 + 5], "the seam column is skipped");

        let c = |k: usize| Control(control[k]);
        assert_eq!((c(0).base(), c(0).overlay(), c(0).blend(), c(0).hole()), (9, 3, 59, false), "200 beats 60, blend 60 / 260");
        assert_eq!((c(1).base(), c(1).overlay(), c(1).blend()), (17, 17, 0), "one texture alone");
        assert_eq!((c(18).base(), c(18).overlay(), c(18).blend()), (3, 9, 128), "a tie goes to the first channel");
        assert!(c(16 * 3 + 5).hole() && control.iter().filter(|w| Control(**w).hole()).count() == 1);
        assert_eq!((c(2).base(), c(2).overlay(), c(2).blend()), (3, 3, 0), "no weight at all draws the first texture");
    }

    #[test]
    fn a_level_without_splat_or_at_another_stride_converts_plainly_or_not_at_all() {
        let mut l = level(16, 64);
        l.splat.clear();
        l.holes.clear();
        let (heights, control) = coarse_from_level(&l, 1024, 16).unwrap();
        assert_eq!(heights, l.heights);
        assert!(control.iter().all(|w| *w == Control(0).with_base(3).0));
        assert_eq!(coarse_from_level(&l, 1024, 4), None);
        assert_eq!(coarse_from_level(&level(16, 63), 1024, 16), None, "too few vertices");
    }
}
