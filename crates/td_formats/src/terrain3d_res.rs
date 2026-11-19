//! Reads Terrain3D region files (`terrain3d_00-01.res`) without Godot, for a data folder the addon has not cached yet.
//! A region read here holds what the addon would export for it, maps and hash alike, so an edit TrenchDot saves passes
//! the same check when Godot applies it.

use crate::godot_res::{self, Object, Resource, SubResource, Value};
use crate::terrain3d_cache::{CacheError, CachedRegion, region_hash};
use std::path::Path;
use td_geom::terrain3d::{BLANK_CONTROL, Loc, MAX_VERTEX_SPACING, MIN_VERTEX_SPACING, REGION_SIZES, Region3D, parse_file_name};

const REGION_TYPE: &str = "Terrain3DRegion";
const HEIGHT: &str = "height_map";
const CONTROL: &str = "control_map";
const COLOR: &str = "color_map";
const COMPRESSED_COLOR: &str = "compressed_color_map";

type Result<T> = std::result::Result<T, CacheError>;

/// A region as its file holds it. `cached.vertex_spacing` is the spacing the region was last saved with, which the
/// other regions of the folder need not share. `data_version` is the Terrain3D data version of the file, 0 without one.
#[derive(Clone, Debug, PartialEq)]
pub struct ResRegion {
    pub cached: CachedRegion,
    pub data_version: f64,
}

pub fn read_region_res(path: &Path) -> Result<ResRegion> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let loc = parse_file_name(name).ok_or_else(|| invalid(format!("{name} is not named like a region file")))?;
    decode_region_res(&std::fs::read(path)?, loc)
}

/// Terrain3D places a region by its file name and ignores the location saved in it, so the caller passes `loc`.
pub fn decode_region_res(bytes: &[u8], loc: Loc) -> Result<ResRegion> {
    let res = godot_res::parse(bytes).map_err(|e| invalid(e.to_string()))?;
    region_from(&res, loc)
}

fn region_from(res: &Resource, loc: Loc) -> Result<ResRegion> {
    let main = res.main();
    if main.type_name != REGION_TYPE {
        return Err(invalid(format!("the file holds a {}, not a {REGION_TYPE}", main.type_name)));
    }

    let height_map = image(res, main, HEIGHT)?;
    let size = match property(main, "region_size")? {
        Some(v) => v.as_int().ok_or_else(|| invalid(format!("the region size is {v:?}")))?,
        None => height_map.as_ref().map(|m| m.width).ok_or_else(|| invalid("has neither a region size nor a height map".into()))?,
    };
    let size =
        u32::try_from(size).ok().filter(|s| REGION_SIZES.contains(s)).ok_or_else(|| invalid(format!("region size {size} is not one of {REGION_SIZES:?}")))?;
    let n = size as usize * size as usize;

    let heights = match &height_map {
        None => vec![0.0; n],
        Some(map) => match map.format {
            "RFloat" => map.base(size, 4)?.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect(),
            "RHalf" => map.base(size, 2)?.as_chunks::<2>().0.iter().map(|b| half_to_f32(u16::from_le_bytes(*b))).collect(),
            _ => return Err(map.stored_as()),
        },
    };

    let control = match image(res, main, CONTROL)? {
        None => vec![BLANK_CONTROL; n],
        Some(map) if map.format == "RFloat" => map.base(size, 4)?.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b)).collect(),
        Some(map) => return Err(map.stored_as()),
    };

    let color = match image(res, main, COLOR)? {
        Some(map) if map.format == "RGBA8" => map.base(size, 4)?.to_vec(),
        Some(map) => return Err(map.stored_as()),
        None if image(res, main, COMPRESSED_COLOR)?.is_some() => {
            return Err(invalid("the color map is only kept compressed, open the folder in Godot once so the addon caches it".into()));
        }
        None => Vec::new(),
    };

    let spacing = number(main, "vertex_spacing")?.unwrap_or(1.0);
    if !(MIN_VERTEX_SPACING..=MAX_VERTEX_SPACING).contains(&spacing) {
        return Err(invalid(format!("vertex spacing {spacing} is outside {MIN_VERTEX_SPACING} to {MAX_VERTEX_SPACING}")));
    }

    let mut region = Region3D { loc, size, heights, control, color };
    if region.color_is_blank() {
        region.color.clear();
    }

    region.check().map_err(invalid)?;
    let hash = region_hash(&region);
    let data_version = number(main, "version")?.unwrap_or(0.0);
    Ok(ResRegion { cached: CachedRegion { region, vertex_spacing: spacing as f32, hash }, data_version })
}

/// The property `name` of the main resource, None when the file leaves it at its default. A property the reader did not
/// get to because the file is damaged or newer is an error.
fn property<'a>(main: &'a SubResource, name: &str) -> Result<Option<&'a Value>> {
    match (main.get(name), &main.unread) {
        (Some(v), _) => Ok(Some(v)),
        (None, Some(e)) => Err(invalid(format!("the {} could not be read, {e}", label(name)))),
        (None, None) => Ok(None),
    }
}

fn number(main: &SubResource, name: &str) -> Result<Option<f64>> {
    property(main, name)?.map(|v| v.as_float().ok_or_else(|| invalid(format!("the {} is {v:?}", label(name))))).transpose()
}

/// One map of a region as Godot saves an `Image`.
struct Map<'a> {
    name: &'a str,
    format: &'a str,
    width: i64,
    height: i64,
    mipmaps: bool,
    data: &'a [u8],
}

impl<'a> Map<'a> {
    /// The full size level, mipmaps dropped.
    fn base(&self, size: u32, pixel_bytes: usize) -> Result<&'a [u8]> {
        if self.width != i64::from(size) || self.height != i64::from(size) {
            return Err(invalid(format!("the {} is {} by {}, the region is {size}", label(self.name), self.width, self.height)));
        }

        let n = size as usize * size as usize * pixel_bytes;
        let whole = if self.mipmaps { self.data.len() >= n } else { self.data.len() == n };
        match self.data.get(..n) {
            Some(base) if whole => Ok(base),
            _ => Err(invalid(format!("the {} holds {} bytes, {n} expected", label(self.name), self.data.len()))),
        }
    }

    fn stored_as(&self) -> CacheError {
        invalid(format!("the {} is stored as {}, open the folder in Godot once so the addon caches it", label(self.name), self.format))
    }
}

fn image<'a>(res: &'a Resource, main: &'a SubResource, name: &'a str) -> Result<Option<Map<'a>>> {
    let value = match property(main, name)? {
        None | Some(Value::Nil | Value::Object(Object::Empty)) => return Ok(None),
        Some(v) => v,
    };
    let damaged = |what: &str| invalid(format!("the {} {what}", label(name)));
    if let Value::Object(Object::External(_)) = value {
        return Err(damaged("is a separate file"));
    }

    let image = res.resolve(value).ok_or_else(|| damaged("refers to nothing in the file"))?;
    if image.type_name != "Image" {
        return Err(damaged(&format!("is a {}", image.type_name)));
    }

    let data = image.get("data").ok_or_else(|| damaged("has no data"))?;
    let field = |key: &str| data.get(key).ok_or_else(|| damaged(&format!("has no {key}")));
    Ok(Some(Map {
        name,
        format: field("format")?.as_str().ok_or_else(|| damaged("names no format"))?,
        width: field("width")?.as_int().ok_or_else(|| damaged("has no width"))?,
        height: field("height")?.as_int().ok_or_else(|| damaged("has no height"))?,
        mipmaps: field("mipmaps")?.as_bool().ok_or_else(|| damaged("does not say whether it has mipmaps"))?,
        data: field("data")?.as_bytes().ok_or_else(|| damaged("has no bytes"))?,
    }))
}

fn label(property: &str) -> String {
    property.replace('_', " ")
}

fn invalid(message: String) -> CacheError {
    CacheError::RegionFile(message)
}

/// Godot's `halfbits_to_floatbits`, which Terrain3D converts a 16 bit height map with.
fn half_to_f32(h: u16) -> f32 {
    let sign = u32::from(h & 0x8000) << 16;
    let significand = u32::from(h & 0x03ff);
    f32::from_bits(match h & 0x7c00 {
        0 if significand == 0 => sign,
        0 => {
            let shift = significand.leading_zeros() - 21;
            sign + ((113 - shift) << 23) + (((significand << shift) & 0x03ff) << 13)
        }
        0x7c00 => sign + 0x7f80_0000 + (significand << 13),
        _ => sign + ((u32::from(h & 0x7fff) + 0x1_c000) << 13),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::godot_res::ResError;
    use crate::terrain3d_cache::decode_region;
    use std::path::PathBuf;
    use td_geom::terrain3d::{BLANK_COLOR, file_stem};

    fn godot() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../godot")
    }

    fn fixture(path: &str) -> Vec<u8> {
        std::fs::read(godot().join("tests/terrain3d/res_fixtures").join(path)).unwrap()
    }

    /// The region file at `res` and the `.td3r` the addon exported for it.
    fn with_export(res: &Path, td3r: &Path) -> (ResRegion, CachedRegion) {
        let read = read_region_res(res).unwrap_or_else(|e| panic!("{}: {e}", res.display()));
        let exported = decode_region(&std::fs::read(td3r).unwrap()).unwrap();
        assert_eq!(region_hash(&exported.region), exported.hash, "{}", td3r.display());
        (read, exported)
    }

    #[test]
    fn region_files_read_as_the_addon_exports_them() {
        let valley = godot().join("terrain3d_valley");
        let mut seen = 0;
        for entry in std::fs::read_dir(&valley).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap();
            let Some(loc) = parse_file_name(name).filter(|_| name.ends_with(".res")) else { continue };
            let td3r = valley.join(".trenchdot/regions").join(file_stem(loc).unwrap() + ".td3r");
            let (read, exported) = with_export(&path, &td3r);
            assert_eq!(read.cached.region, exported.region, "{name}");
            assert_eq!(read.cached.hash, exported.hash, "{name}");
            assert_eq!(read.data_version as f32, 1.1, "{name}");
            seen += 1;
        }

        assert_eq!(seen, 6);

        let fixtures = godot().join("tests/terrain3d/res_fixtures");
        let (painted, exported) = with_export(&fixtures.join("painted/terrain3d-03_02.res"), &fixtures.join("painted/terrain3d-03_02.td3r"));
        assert_eq!(painted.cached, exported, "16 bit heights and a mipmapped color map read like Godot converts them");
        assert_eq!((painted.cached.region.loc, painted.cached.vertex_spacing), ([-3, 2], 2.5));
        assert!(!painted.cached.region.color.is_empty());

        let (instances, exported) = with_export(&fixtures.join("instances/terrain3d_01_00.res"), &fixtures.join("instances/terrain3d_01_00.td3r"));
        assert_eq!(instances.cached, exported, "a blank color map is left out, a spacing at its default is 1");
        assert!(instances.cached.region.color.is_empty());
    }

    #[test]
    fn the_file_name_places_a_region() {
        let old = godot().join("tests/terrain3d/v1_0_2/terrain3d_02-01.res");
        let read = read_region_res(&old).unwrap();
        assert_eq!(read.cached.region.loc, [2, -1], "Terrain3D 1.0 saved no location");
        assert_eq!((read.cached.region.size, read.cached.vertex_spacing, read.data_version as f32), (64, 2.0, 0.93));

        let moved = decode_region_res(&fixture("painted/terrain3d-03_02.res"), [5, -7]).unwrap();
        assert_eq!(moved.cached.region.loc, [5, -7], "not the location saved in the file");
        let err = read_region_res(&old.with_file_name("terrain3d_02-01 copy.res")).unwrap_err();
        assert!(err.to_string().contains("not named like a region file"), "{err}");
    }

    fn painted() -> Resource {
        godot_res::parse(&fixture("painted/terrain3d-03_02.res")).unwrap()
    }

    fn main_mut(res: &mut Resource) -> &mut SubResource {
        res.internal_mut().last_mut().unwrap()
    }

    fn set_image(res: &mut Resource, map: &str, key: &str, value: Value) {
        let Some(Value::Object(Object::Internal(i))) = res.main().get(map).cloned() else { panic!("no {map}") };
        let image = &mut res.internal_mut()[i];
        let Some((_, Value::Dictionary(data))) = image.properties.iter_mut().find(|(n, _)| n == "data") else { panic!("{map} has no data") };
        data.iter_mut().find(|(k, _)| k.as_str() == Some(key)).unwrap().1 = value;
    }

    fn refused(res: &Resource, needle: &str) {
        let err = region_from(res, [0, 0]).unwrap_err();
        assert!(matches!(err, CacheError::RegionFile(_)) && err.to_string().contains(needle), "{err} lacks {needle}");
    }

    #[test]
    fn maps_godot_would_convert_first_are_refused() {
        let mut compressed = painted();
        let color = main_mut(&mut compressed).properties.iter_mut().find(|(n, _)| n == COLOR).unwrap();
        color.0 = COMPRESSED_COLOR.into();
        refused(&compressed, "only kept compressed");

        let mut dxt = painted();
        set_image(&mut dxt, COLOR, "format", Value::String("DXT5 RGBA8".into()));
        refused(&dxt, "stored as DXT5 RGBA8");

        let mut half_control = painted();
        set_image(&mut half_control, CONTROL, "format", Value::String("RHalf".into()));
        refused(&half_control, "control map is stored as RHalf");

        let mut small = painted();
        set_image(&mut small, HEIGHT, "width", Value::Int(32));
        refused(&small, "is 32 by 64");

        let mut short = painted();
        set_image(&mut short, CONTROL, "data", Value::Bytes(vec![0; 100]));
        refused(&short, "holds 100 bytes");

        let mut other = painted();
        main_mut(&mut other).type_name = "Resource".into();
        refused(&other, "not a Terrain3DRegion");

        for (spacing, says) in [(0.1, "vertex spacing 0.1 is outside"), (f64::NAN, "vertex spacing NaN is outside")] {
            let mut odd = painted();
            main_mut(&mut odd).properties.iter_mut().find(|(n, _)| n == "vertex_spacing").unwrap().1 = Value::Float(spacing);
            refused(&odd, says);
        }
    }

    #[test]
    fn a_missing_map_is_blank_unless_the_file_stopped_before_it() {
        let mut res = painted();
        main_mut(&mut res).properties.retain(|(n, _)| n != HEIGHT && n != COLOR);
        let read = region_from(&res, [0, 0]).unwrap().cached;
        assert!(read.region.heights.iter().all(|h| *h == 0.0));
        assert!(read.region.color.is_empty());
        assert_eq!(read.region.control, region_from(&painted(), [0, 0]).unwrap().cached.region.control);

        main_mut(&mut res).unread = Some(ResError::Truncated);
        refused(&res, "height map could not be read, the resource ends early");
    }

    #[test]
    fn a_blank_color_map_is_left_out_of_the_hash() {
        let mut region = Region3D::blank([0, 0], 64);
        let with_blank = region_hash(&region);
        region.color.clear();
        assert_eq!(region_hash(&region), with_blank);
        region.color = [BLANK_COLOR, [0, 0, 0, 255]].concat().repeat(64 * 32);
        assert_ne!(region_hash(&region), with_blank);
    }

    #[test]
    fn halves_convert_exactly() {
        let cases = [
            (0x3c00, 1.0),
            (0xc000, -2.0),
            (0x0001, 2f32.powi(-24)),
            (0x03ff, 1023.0 * 2f32.powi(-24)),
            (0x0400, 2f32.powi(-14)),
            (0x7bff, 65504.0),
            (0x7c00, f32::INFINITY),
            (0xfc00, f32::NEG_INFINITY),
        ];
        for (half, float) in cases {
            assert_eq!(half_to_f32(half), float, "{half:#06x}");
        }

        assert_eq!(half_to_f32(0x8000).to_bits(), (-0.0f32).to_bits());
        assert_eq!(half_to_f32(0x7e01).to_bits(), 0x7fc0_2000, "a NaN keeps its payload");
    }
}
