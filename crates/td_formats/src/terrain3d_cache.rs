//! The cache TrenchDot keeps of a Terrain3D data folder, in `<data folder>/.trenchdot/`. Godot writes the manifest and
//! the region files from what Terrain3D holds in memory, TrenchDot writes pending edits for Godot to apply and LOD
//! files for itself. Every writer writes a temporary file and renames it, so a reader never sees half of one.
//!
//! Region and pending files (`.td3r`) are little endian: magic `TD3R`, u32 version, i32 x, i32 y, u32 size, f32 vertex
//! spacing, u32 flags (bit 0 color present), a 32 byte hash, then one zstd frame of `size * size` f32 heights in
//! meters, as many u32 control words and, when flagged, RGBA8 color. A pending file ends in a trailer: `BASE`, the
//! 32 byte hash of the region the edit started from (zero for a new region) and u32 flags (bit 0 force, bit 1 remove).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use td_geom::heightfield::MAX_LAYERS;
use td_geom::terrain3d::{
    LOC_MAX, LOC_MIN, Loc, MAX_TEXTURES, MAX_VERTEX_SPACING, MIN_VERTEX_SPACING, REGION_SIZES, Region3D, file_stem, loc_in_range, parse_file_name,
};
use td_geom::terrain3d_tile::LOD_STRIDES;

pub const DIR_NAME: &str = ".trenchdot";
pub const FORMAT: &str = "trenchdot-terrain3d";
pub const VERSION: u32 = 1;
pub const MANIFEST: &str = "manifest.json";
pub const REGION_EXT: &str = "td3r";
pub const LOD_EXT: &str = "td3l";

const REGION_MAGIC: [u8; 4] = *b"TD3R";
const BASE_MAGIC: [u8; 4] = *b"BASE";
const LOD_MAGIC: [u8; 4] = *b"TD3L";
const TRAILER_LEN: usize = 40;
const FLAG_COLOR: u32 = 1;
const PENDING_FORCE: u32 = 1;
const PENDING_REMOVE: u32 = 2;
const LOD_SPLAT: u8 = 1;
const LOD_HOLES: u8 = 2;
const ZSTD_LEVEL: i32 = 3;

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("not a TrenchDot Terrain3D cache file")]
    Magic,
    #[error("Terrain3D cache version {0} is newer than this editor supports, update TrenchDot")]
    Newer(u32),
    #[error("Terrain3D cache file is truncated")]
    Truncated,
    #[error("Terrain3D cache: {0}")]
    Invalid(String),
    #[error("Terrain3D cache manifest: {0}")]
    Json(#[from] serde_json::Error),
}

type Result<T> = std::result::Result<T, CacheError>;

/// The sha256 Godot computes over a region's height, control and color bytes. TrenchDot only compares and echoes it.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct RegionHash(pub [u8; 32]);

impl RegionHash {
    pub const ZERO: Self = Self([0; 32]);
}

impl std::fmt::Display for RegionHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl std::fmt::Debug for RegionHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::str::FromStr for RegionHash {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        let bad = || format!("{s:?} is not a 64 digit hex hash");
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }

        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| bad())?;
        }

        Ok(Self(out))
    }
}

impl Serialize for RegionHash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RegionHash {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}

/// `manifest.json`, written by the Godot slot node or the headless CLI. Unknown keys are ignored so a newer addon's
/// additions do not break an older editor, a newer `version` is refused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub terrain3d: String,
    #[serde(default)]
    pub godot: String,
    #[serde(default)]
    pub data_version: f64,
    pub region_size: u32,
    pub vertex_spacing: f64,
    #[serde(default)]
    pub material: ManifestMaterial,
    #[serde(default = "yes")]
    pub map_transform_identity: bool,
    #[serde(default)]
    pub editor_open: bool,
    #[serde(default)]
    pub regions: Vec<ManifestRegion>,
    /// Regions with unsaved edits in the Godot editor.
    #[serde(default)]
    pub dirty: Vec<Loc>,
    #[serde(default)]
    pub textures: Vec<ManifestTexture>,
    /// What the last pending apply did with each region.
    #[serde(default)]
    pub applied: Vec<Applied>,
    /// Absent when the Terrain3D node has no ocean or the addon is older than the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocean: Option<ManifestOcean>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ManifestResources>,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManifestMaterial {
    /// Terrain3D's `WorldBackground`: 0 none, 1 flat, 2 noise.
    pub world_background: u32,
    pub ground_level: f64,
    pub region_blend: f64,
    pub auto_shader: bool,
    /// Absent from caches written before the key, a noise background then draws flat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub world_noise: Option<WorldNoise>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto: Option<AutoShader>,
    pub blend_sharpness: f64,
    pub dual_scaling: bool,
    pub macro_variation: bool,
    pub projection: bool,
}

impl Default for ManifestMaterial {
    fn default() -> Self {
        Self {
            world_background: 0,
            ground_level: 0.0,
            region_blend: 0.0,
            auto_shader: false,
            world_noise: None,
            auto: None,
            blend_sharpness: 0.5,
            dual_scaling: false,
            macro_variation: false,
            projection: false,
        }
    }
}

/// The `world_noise_*` parameters of the Terrain3D material, defaults are its shader's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldNoise {
    pub scale: f64,
    pub height: f64,
    pub offset: [f64; 3],
    pub lod_distance: f64,
    pub max_octaves: u32,
    pub min_octaves: u32,
    pub fragment_normals: bool,
}

impl Default for WorldNoise {
    fn default() -> Self {
        Self { scale: 5.0, height: 32.0, offset: [0.0; 3], lod_distance: 7500.0, max_octaves: 4, min_octaves: 2, fragment_normals: false }
    }
}

/// The `auto_*` parameters of the Terrain3D material, defaults are its shader's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoShader {
    pub slope: f64,
    pub height_reduction: f64,
    pub base_texture: u32,
    pub overlay_texture: u32,
}

impl Default for AutoShader {
    fn default() -> Self {
        Self { slope: 1.0, height_reduction: 0.1, base_texture: 0, overlay_texture: 1 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManifestOcean {
    pub enabled: bool,
    pub sea_level: f64,
    pub water_color: [f32; 4],
    /// Res path of the ocean material, empty for a built in one.
    pub source: String,
}

impl Default for ManifestOcean {
    fn default() -> Self {
        Self { enabled: false, sea_level: 0.0, water_color: [0.1058, 0.1568, 0.2117, 1.0], source: String::new() }
    }
}

/// Res paths of the resources set on the Terrain3D node, empty for none or a built in one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManifestResources {
    pub material: String,
    pub assets: String,
    pub ocean_material: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestRegion {
    pub loc: Loc,
    pub hash: RegionHash,
    #[serde(default)]
    pub height_range: [f32; 2],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestTexture {
    pub id: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default = "white")]
    pub albedo_color: [f32; 4],
    /// Relative to the cache folder, empty when Godot made none.
    #[serde(default)]
    pub thumbnail: String,
    #[serde(default = "uv_scale")]
    pub uv_scale: f32,
    #[serde(default)]
    pub detiling_rotation: f32,
    #[serde(default)]
    pub detiling_shift: f32,
    /// The full albedo relative to the cache folder, empty when Godot wrote none and the thumbnail stands in.
    #[serde(default)]
    pub albedo: String,
    /// Changes whenever the image or uv values do, empty from caches written before the key.
    #[serde(default)]
    pub hash: String,
}

impl Default for ManifestTexture {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            albedo_color: white(),
            thumbnail: String::new(),
            uv_scale: uv_scale(),
            detiling_rotation: 0.0,
            detiling_shift: 0.0,
            albedo: String::new(),
            hash: String::new(),
        }
    }
}

fn white() -> [f32; 4] {
    [1.0; 4]
}

fn uv_scale() -> f32 {
    0.1
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    pub seq: u64,
    #[serde(default)]
    pub accepted: Vec<Loc>,
    #[serde(default)]
    pub refused: Vec<Refused>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Refused {
    pub loc: Loc,
    #[serde(default)]
    pub reason: String,
}

impl Manifest {
    pub fn check(&self) -> Result<()> {
        check_size(self.region_size)?;
        check_spacing(self.vertex_spacing)?;
        let locs = self.regions.iter().map(|r| r.loc).chain(self.dirty.iter().copied());
        let mut locs = locs.chain(self.applied.iter().flat_map(|a| a.accepted.iter().copied().chain(a.refused.iter().map(|r| r.loc))));
        locs.try_for_each(check_loc)
    }
}

/// Reads a manifest, refusing another format or a newer version before looking at the rest, which a newer addon may
/// have laid out differently.
pub fn parse_manifest(text: &str) -> Result<Manifest> {
    #[derive(Deserialize)]
    struct Head {
        format: String,
        version: u32,
    }

    let mut value: serde_json::Value = serde_json::from_str(text)?;
    whole_floats_to_ints(&mut value);
    let head = Head::deserialize(&value)?;
    if head.format != FORMAT {
        return Err(CacheError::Magic);
    }

    check_version(head.version)?;
    let manifest: Manifest = serde_json::from_value(value)?;
    manifest.check()?;
    Ok(manifest)
}

/// Godot parses every JSON number as a float, so a manifest it read and wrote again holds `7.0` where it held `7`.
fn whole_floats_to_ints(value: &mut serde_json::Value) {
    use serde_json::Value;
    match value {
        Value::Number(n) => {
            if let Some(f) = n.as_f64().filter(|f| n.is_f64() && f.fract() == 0.0 && f.abs() < 9.0e15) {
                *value = Value::from(f as i64);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(whole_floats_to_ints),
        Value::Object(map) => map.values_mut().for_each(whole_floats_to_ints),
        _ => {}
    }
}

/// One region as Godot exported it.
#[derive(Clone, Debug, PartialEq)]
pub struct CachedRegion {
    pub region: Region3D,
    pub vertex_spacing: f32,
    pub hash: RegionHash,
}

/// A region TrenchDot edited, for Godot to apply when the region still has the `base` hash and is not dirty in the
/// Godot editor, or anyway with `force`. With `remove` Godot removes the region and ignores the maps.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingRegion {
    pub region: Region3D,
    pub vertex_spacing: f32,
    pub base: RegionHash,
    pub force: bool,
    pub remove: bool,
}

pub fn encode_region(r: &CachedRegion) -> Result<Vec<u8>> {
    encode_maps(&r.region, r.vertex_spacing, r.hash)
}

pub fn decode_region(bytes: &[u8]) -> Result<CachedRegion> {
    let mut r = Reader::new(bytes);
    let h = read_header(&mut r)?;
    let region = decode_maps(&h, r.rest())?;
    Ok(CachedRegion { region, vertex_spacing: h.spacing, hash: h.hash })
}

/// The header hash of a pending file stays zero, TrenchDot never computes one.
pub fn encode_pending(p: &PendingRegion) -> Result<Vec<u8>> {
    let mut out = encode_maps(&p.region, p.vertex_spacing, RegionHash::ZERO)?;
    let flags = if p.force { PENDING_FORCE } else { 0 } | if p.remove { PENDING_REMOVE } else { 0 };
    out.extend(BASE_MAGIC);
    out.extend(p.base.0);
    out.extend(flags.to_le_bytes());
    Ok(out)
}

pub fn decode_pending(bytes: &[u8]) -> Result<PendingRegion> {
    let mut r = Reader::new(bytes);
    let h = read_header(&mut r)?;
    let rest = r.rest();
    let split = rest.len().checked_sub(TRAILER_LEN).ok_or(CacheError::Truncated)?;
    let (body, trailer) = rest.split_at(split);
    let mut t = Reader::new(trailer);
    if t.take(4)? != BASE_MAGIC {
        return Err(CacheError::Invalid("pending file has no BASE trailer".into()));
    }

    let base = RegionHash(t.array()?);
    let flags = t.u32()?;
    let region = decode_maps(&h, body)?;
    Ok(PendingRegion { region, vertex_spacing: h.spacing, base, force: flags & PENDING_FORCE != 0, remove: flags & PENDING_REMOVE != 0 })
}

struct Header {
    loc: Loc,
    size: u32,
    spacing: f32,
    flags: u32,
    hash: RegionHash,
}

fn read_header(r: &mut Reader) -> Result<Header> {
    if r.take(4)? != REGION_MAGIC {
        return Err(CacheError::Magic);
    }

    check_version(r.u32()?)?;
    let loc = [r.i32()?, r.i32()?];
    let size = r.u32()?;
    let spacing = r.f32()?;
    let flags = r.u32()?;
    let hash = RegionHash(r.array()?);
    check_loc(loc)?;
    check_size(size)?;
    check_spacing(spacing as f64)?;
    Ok(Header { loc, size, spacing, flags, hash })
}

fn encode_maps(region: &Region3D, spacing: f32, hash: RegionHash) -> Result<Vec<u8>> {
    region.check().map_err(CacheError::Invalid)?;
    check_spacing(spacing as f64)?;
    let mut raw = Vec::with_capacity(region.heights.len() * 8 + region.color.len());
    raw.extend(region.heights.iter().flat_map(|h| h.to_le_bytes()));
    raw.extend(region.control.iter().flat_map(|c| c.to_le_bytes()));
    raw.extend(&region.color);

    let flags = if region.color.is_empty() { 0 } else { FLAG_COLOR };
    let mut out = Vec::new();
    out.extend(REGION_MAGIC);
    out.extend(VERSION.to_le_bytes());
    out.extend(region.loc[0].to_le_bytes());
    out.extend(region.loc[1].to_le_bytes());
    out.extend(region.size.to_le_bytes());
    out.extend(spacing.to_le_bytes());
    out.extend(flags.to_le_bytes());
    out.extend(hash.0);
    out.extend(zstd::bulk::compress(&raw, ZSTD_LEVEL)?);
    Ok(out)
}

fn decode_maps(h: &Header, body: &[u8]) -> Result<Region3D> {
    let n = h.size as usize * h.size as usize;
    let expected = n * if h.flags & FLAG_COLOR != 0 { 12 } else { 8 };
    let raw = zstd::bulk::decompress(body, expected).map_err(|e| CacheError::Invalid(format!("the maps do not fit a region of size {}: {e}", h.size)))?;
    if raw.len() != expected {
        return Err(CacheError::Invalid(format!("the maps hold {} bytes, a region of size {} needs {expected}", raw.len(), h.size)));
    }

    let (heights, rest) = raw.split_at(n * 4);
    let (control, color) = rest.split_at(n * 4);
    Ok(Region3D {
        loc: h.loc,
        size: h.size,
        heights: heights.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect(),
        control: control.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b)).collect(),
        color: color.to_vec(),
    })
}

/// One level of a region's LOD pyramid: `side * side` vertices `stride` apart, heights in meters, `ids` the Terrain3D
/// texture shown by each splat channel. `splat` (four bytes per vertex) and `holes` (one per vertex, or one per cell
/// when `side` is the cell count, then laid out `side` wide) may be empty.
#[derive(Clone, Debug, PartialEq)]
pub struct LodLevel {
    pub stride: u32,
    pub side: u32,
    pub ids: Vec<u8>,
    pub heights: Vec<f32>,
    pub splat: Vec<u8>,
    pub holes: Vec<u8>,
}

/// The LOD pyramid TrenchDot built from the region with `hash`, finest level first.
#[derive(Clone, Debug, PartialEq)]
pub struct Lod {
    pub loc: Loc,
    pub region_size: u32,
    pub hash: RegionHash,
    pub levels: Vec<LodLevel>,
}

impl LodLevel {
    /// A level covers the region with or without the seam row and column it shares with its neighbors.
    pub fn check(&self, region_size: u32) -> std::result::Result<(), String> {
        if !LOD_STRIDES.contains(&self.stride) || !region_size.is_multiple_of(self.stride) {
            return Err(format!("LOD stride {} does not fit a region of size {region_size}", self.stride));
        }

        let cells = region_size / self.stride;
        if self.side != cells && self.side != cells + 1 {
            return Err(format!("LOD level of stride {} has {} vertices per side, expected {cells} or {}", self.stride, self.side, cells + 1));
        }

        if self.ids.len() > MAX_LAYERS || self.ids.iter().any(|id| *id as u32 >= MAX_TEXTURES) {
            return Err(format!("LOD texture ids {:?} are not up to {MAX_LAYERS} ids below {MAX_TEXTURES}", self.ids));
        }

        let n = self.side as usize * self.side as usize;
        if self.heights.len() != n || !(self.splat.is_empty() || self.splat.len() == n * 4) || !(self.holes.is_empty() || self.holes.len() == n) {
            return Err(format!(
                "LOD level of side {} holds {} heights, {} splat and {} hole bytes",
                self.side,
                self.heights.len(),
                self.splat.len(),
                self.holes.len()
            ));
        }

        Ok(())
    }
}

impl Lod {
    pub fn check(&self) -> Result<()> {
        check_loc(self.loc)?;
        check_size(self.region_size)?;
        if self.levels.len() > LOD_STRIDES.len() {
            return Err(CacheError::Invalid(format!("{} LOD levels, at most {}", self.levels.len(), LOD_STRIDES.len())));
        }

        self.levels.iter().try_for_each(|l| l.check(self.region_size)).map_err(CacheError::Invalid)
    }
}

pub fn encode_lod(lod: &Lod) -> Result<Vec<u8>> {
    lod.check()?;
    let mut raw = Vec::new();
    for l in &lod.levels {
        let flags = if l.splat.is_empty() { 0 } else { LOD_SPLAT } | if l.holes.is_empty() { 0 } else { LOD_HOLES };
        raw.extend(l.stride.to_le_bytes());
        raw.extend(l.side.to_le_bytes());
        raw.push(l.ids.len() as u8);
        raw.extend(&l.ids);
        raw.push(flags);
        raw.extend(l.heights.iter().flat_map(|h| h.to_le_bytes()));
        raw.extend(&l.splat);
        raw.extend(&l.holes);
    }

    let mut out = Vec::new();
    out.extend(LOD_MAGIC);
    out.extend(VERSION.to_le_bytes());
    out.extend(lod.loc[0].to_le_bytes());
    out.extend(lod.loc[1].to_le_bytes());
    out.extend(lod.region_size.to_le_bytes());
    out.extend(lod.hash.0);
    out.extend((lod.levels.len() as u32).to_le_bytes());
    out.extend(zstd::bulk::compress(&raw, ZSTD_LEVEL)?);
    Ok(out)
}

pub fn decode_lod(bytes: &[u8]) -> Result<Lod> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != LOD_MAGIC {
        return Err(CacheError::Magic);
    }

    check_version(r.u32()?)?;
    let loc = [r.i32()?, r.i32()?];
    let region_size = r.u32()?;
    let hash = RegionHash(r.array()?);
    let count = r.u32()? as usize;
    check_loc(loc)?;
    check_size(region_size)?;
    if count > LOD_STRIDES.len() {
        return Err(CacheError::Invalid(format!("{count} LOD levels, at most {}", LOD_STRIDES.len())));
    }

    let side = region_size as usize + 1;
    let cap = count * (10 + MAX_LAYERS + side * side * 9);
    let raw = zstd::bulk::decompress(r.rest(), cap).map_err(|e| CacheError::Invalid(format!("LOD levels: {e}")))?;
    let mut r = Reader::new(&raw);
    let mut levels = Vec::with_capacity(count);
    for _ in 0..count {
        let (stride, side) = (r.u32()?, r.u32()?);
        let id_count = r.u8()? as usize;
        let ids = r.take(id_count)?.to_vec();
        let flags = r.u8()?;
        // Bounded before it sizes a read, a junk side overflows the byte count.
        if side > region_size + 1 {
            return Err(CacheError::Invalid(format!("LOD level has {side} vertices per side, a region of size {region_size} has at most {}", region_size + 1)));
        }

        let n = side as usize * side as usize;
        let level = LodLevel { stride, side, ids, heights: Vec::new(), splat: Vec::new(), holes: Vec::new() };
        let heights = r.take(n * 4)?.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        let splat = if flags & LOD_SPLAT != 0 { r.take(n * 4)?.to_vec() } else { Vec::new() };
        let holes = if flags & LOD_HOLES != 0 { r.take(n)?.to_vec() } else { Vec::new() };
        let level = LodLevel { heights, splat, holes, ..level };
        level.check(region_size).map_err(CacheError::Invalid)?;
        levels.push(level);
    }

    if !r.rest().is_empty() {
        return Err(CacheError::Invalid(format!("{} bytes after the last LOD level", r.rest().len())));
    }

    Ok(Lod { loc, region_size, hash, levels })
}

/// The cache folder of the Terrain3D data folder `data_directory` (a res:// path) in the project at `project_root`.
/// None for the project root itself or a path that leaves the project.
pub fn cache_dir(project_root: &Path, data_directory: &str) -> Option<PathBuf> {
    let dir = crate::game::project_file(project_root, data_directory)?;
    (dir != project_root).then(|| dir.join(DIR_NAME))
}

pub fn manifest_path(cache: &Path) -> PathBuf {
    cache.join(MANIFEST)
}

pub fn region_path(cache: &Path, loc: Loc) -> Option<PathBuf> {
    Some(cache.join("regions").join(format!("{}.{REGION_EXT}", file_stem(loc)?)))
}

pub fn pending_path(cache: &Path, loc: Loc) -> Option<PathBuf> {
    Some(cache.join("pending").join(format!("{}.{REGION_EXT}", file_stem(loc)?)))
}

/// LOD files carry the hash of the region they were built from, so an edited region never reads a stale pyramid.
pub fn lod_path(cache: &Path, loc: Loc, hash: RegionHash) -> Option<PathBuf> {
    Some(cache.join("lod").join(format!("{}.{hash}.{LOD_EXT}", file_stem(loc)?)))
}

pub fn read_manifest(cache: &Path) -> Result<Manifest> {
    parse_manifest(&std::fs::read_to_string(manifest_path(cache))?)
}

pub fn write_manifest(cache: &Path, manifest: &Manifest) -> Result<()> {
    manifest.check()?;
    write_file(&manifest_path(cache), &serde_json::to_vec_pretty(manifest)?)
}

/// Region `loc` of a cache whose manifest says `region_size`.
pub fn read_region(cache: &Path, loc: Loc, region_size: u32) -> Result<CachedRegion> {
    let path = region_path(cache, loc).ok_or_else(|| outside(loc))?;
    let r = decode_region(&std::fs::read(&path)?)?;
    expect(&r.region, loc, region_size)?;
    Ok(r)
}

pub fn write_region(cache: &Path, r: &CachedRegion) -> Result<()> {
    let path = region_path(cache, r.region.loc).ok_or_else(|| outside(r.region.loc))?;
    write_file(&path, &encode_region(r)?)
}

pub fn read_pending(cache: &Path, loc: Loc, region_size: u32) -> Result<PendingRegion> {
    let path = pending_path(cache, loc).ok_or_else(|| outside(loc))?;
    let p = decode_pending(&std::fs::read(&path)?)?;
    expect(&p.region, loc, region_size)?;
    Ok(p)
}

pub fn write_pending(cache: &Path, p: &PendingRegion) -> Result<()> {
    let path = pending_path(cache, p.region.loc).ok_or_else(|| outside(p.region.loc))?;
    write_file(&path, &encode_pending(p)?)
}

/// The regions with a pending file, sorted. Files still being written and anything else in the folder are skipped.
pub fn pending_locs(cache: &Path) -> std::io::Result<Vec<Loc>> {
    let entries = match std::fs::read_dir(cache.join("pending")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        entries => entries?,
    };

    let mut locs = Vec::new();
    for entry in entries {
        let name = entry?.file_name();
        let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(&format!(".{REGION_EXT}"))) else { continue };
        if let Some(loc) = parse_file_name(stem).filter(|loc| file_stem(*loc).as_deref() == Some(stem)) {
            locs.push(loc);
        }
    }

    locs.sort();
    Ok(locs)
}

/// The pyramid of region `loc` built from the region with `hash`.
pub fn read_lod(cache: &Path, loc: Loc, region_size: u32, hash: RegionHash) -> Result<Lod> {
    let path = lod_path(cache, loc, hash).ok_or_else(|| outside(loc))?;
    let lod = decode_lod(&std::fs::read(&path)?)?;
    if lod.loc != loc || lod.region_size != region_size || lod.hash != hash {
        return Err(CacheError::Invalid(format!("{} holds the LOD of region {:?} size {} hash {}", path.display(), lod.loc, lod.region_size, lod.hash)));
    }

    Ok(lod)
}

pub fn write_lod(cache: &Path, lod: &Lod) -> Result<()> {
    let path = lod_path(cache, lod.loc, lod.hash).ok_or_else(|| outside(lod.loc))?;
    write_file(&path, &encode_lod(lod)?)
}

/// Deletes the LOD files of `loc` built from any region other than `keep`. Returns how many went.
pub fn prune_lod(cache: &Path, loc: Loc, keep: RegionHash) -> std::io::Result<usize> {
    let Some(stem) = file_stem(loc) else { return Ok(0) };
    let entries = match std::fs::read_dir(cache.join("lod")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        entries => entries?,
    };

    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(hash) = name.to_str().and_then(|n| n.strip_prefix(&format!("{stem}."))?.strip_suffix(&format!(".{LOD_EXT}"))) else { continue };
        if hash.parse::<RegionHash>().is_ok_and(|h| h != keep) {
            std::fs::remove_file(entry.path())?;
            removed += 1;
        }
    }

    Ok(removed)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }

    crate::write_atomic(path, &mut &bytes[..])?;
    Ok(())
}

fn expect(region: &Region3D, loc: Loc, region_size: u32) -> Result<()> {
    if region.loc != loc {
        return Err(CacheError::Invalid(format!("the file of region {loc:?} holds region {:?}", region.loc)));
    }

    if region.size != region_size {
        return Err(CacheError::Invalid(format!("region {loc:?} has size {}, the manifest says {region_size}", region.size)));
    }

    Ok(())
}

fn outside(loc: Loc) -> CacheError {
    CacheError::Invalid(format!("region {loc:?} is outside {LOC_MIN} to {LOC_MAX}"))
}

fn check_version(version: u32) -> Result<()> {
    match version {
        0 => Err(CacheError::Invalid("version 0".into())),
        v if v > VERSION => Err(CacheError::Newer(v)),
        _ => Ok(()),
    }
}

fn check_loc(loc: Loc) -> Result<()> {
    if loc_in_range(loc) { Ok(()) } else { Err(outside(loc)) }
}

fn check_size(size: u32) -> Result<()> {
    if REGION_SIZES.contains(&size) { Ok(()) } else { Err(CacheError::Invalid(format!("region size {size} is not one of {REGION_SIZES:?}"))) }
}

fn check_spacing(spacing: f64) -> Result<()> {
    if (MIN_VERTEX_SPACING..=MAX_VERTEX_SPACING).contains(&spacing) {
        Ok(())
    } else {
        Err(CacheError::Invalid(format!("vertex spacing {spacing} is outside {MIN_VERTEX_SPACING} to {MAX_VERTEX_SPACING}")))
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|end| *end <= self.bytes.len()).ok_or(CacheError::Truncated)?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("N bytes"))
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        self.array().map(u32::from_le_bytes)
    }

    fn i32(&mut self) -> Result<i32> {
        self.array().map(i32::from_le_bytes)
    }

    fn f32(&mut self) -> Result<f32> {
        self.array().map(f32::from_le_bytes)
    }

    fn rest(&self) -> &'a [u8] {
        &self.bytes[self.at..]
    }
}
