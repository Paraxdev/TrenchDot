//! The Terrain3D cache files, read from fixtures written by tests/terrain3d/make_fixtures.py the way the Godot addon
//! writes them, and round tripped through the Rust writers.

use std::path::{Path, PathBuf};
use td_formats::terrain3d_cache::*;
use td_geom::terrain3d::Region3D;

const SIZE: u32 = 64;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/terrain3d/cache")
}

/// The pattern make_fixtures.py writes.
fn pattern(loc: [i32; 2], lift: f64, color: bool) -> Region3D {
    let mut r = Region3D { loc, size: SIZE, heights: Vec::new(), control: Vec::new(), color: Vec::new() };
    for z in 0..SIZE {
        for x in 0..SIZE {
            let i = z * SIZE + x;
            r.heights.push((loc[0] as f64 * 8.0 + loc[1] as f64 * 0.5 + x as f64 * 0.25 - z as f64 * 0.125 + lift) as f32);
            let c = i.wrapping_mul(2654435761).wrapping_add((loc[0] as u32).wrapping_mul(40503)).wrapping_add((loc[1] as u32).wrapping_mul(977));
            r.control.push(match i {
                0 => 0x7FC0_0001,
                1 => 0xFF80_0001,
                _ => c,
            });
            if color {
                r.color.extend([x as u8 * 4, z as u8 * 4, (x + z) as u8, 255 - x as u8]);
            }
        }
    }

    r
}

fn hash_of(loc: [i32; 2]) -> RegionHash {
    RegionHash(std::array::from_fn(|k| (k as i32 * 37 + loc[0] * 11 + loc[1] * 5) as u8))
}

/// The sha256 make_fixtures.py and the addon compute over a region's uncompressed maps.
fn fixture_hash(loc: [i32; 2]) -> RegionHash {
    match loc {
        [-1, 0] => "b8875e9e03bdb23d5c592f294cc8f6a6f0a1485b831bc20b3413c398b8fb0e69",
        [3, -2] => "7ba9c8194ef04bdcbadd0519fe1623c78e74eed9125822db0f52256d92386191",
        _ => panic!("no fixture region at {loc:?}"),
    }
    .parse()
    .unwrap()
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

fn header(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn manifest_fixture_reads_whole_and_ignores_unknown_keys() {
    let m = read_manifest(&fixture()).unwrap();
    let expected = Manifest {
        format: FORMAT.into(),
        version: 1,
        terrain3d: "1.1.0".into(),
        godot: "4.7.2".into(),
        data_version: 1.1,
        region_size: 64,
        vertex_spacing: 2.5,
        material: ManifestMaterial { world_background: 1, ground_level: -2.0, region_blend: 0.25, auto_shader: true },
        map_transform_identity: false,
        editor_open: true,
        regions: vec![
            ManifestRegion { loc: [-1, 0], hash: fixture_hash([-1, 0]), height_range: [-15.875, 7.75] },
            ManifestRegion { loc: [3, -2], hash: fixture_hash([3, -2]), height_range: [15.125, 38.75] },
        ],
        dirty: vec![[3, -2]],
        textures: vec![
            ManifestTexture { id: 0, name: "Grass".into(), albedo_color: [1.0; 4], thumbnail: "textures/0.png".into() },
            ManifestTexture { id: 5, name: "Rock".into(), albedo_color: [0.5, 0.25, 0.125, 1.0], thumbnail: String::new() },
        ],
        applied: vec![Applied { seq: 7, accepted: vec![[-1, 0]], refused: vec![Refused { loc: [3, -2], reason: "dirty".into() }] }],
    };
    assert_eq!(m, expected);
    assert_eq!(m.regions[0].hash.to_string(), "b8875e9e03bdb23d5c592f294cc8f6a6f0a1485b831bc20b3413c398b8fb0e69");
}

#[test]
fn manifest_round_trips_and_minimal_one_takes_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = read_manifest(&fixture()).unwrap();
    m.dirty.clear();
    m.regions.push(ManifestRegion { loc: [-16, 15], hash: RegionHash::ZERO, height_range: [0.0, 0.0] });
    write_manifest(dir.path(), &m).unwrap();
    assert_eq!(read_manifest(dir.path()).unwrap(), m);
    assert_eq!(files(dir.path()), ["manifest.json"], "no temporary file stays behind");

    let minimal = parse_manifest(r#"{"format":"trenchdot-terrain3d","version":1,"region_size":256,"vertex_spacing":0.25}"#).unwrap();
    assert!(minimal.map_transform_identity && !minimal.editor_open && minimal.regions.is_empty() && minimal.textures.is_empty());
    assert_eq!(minimal.material, ManifestMaterial::default());
}

#[test]
fn manifest_refuses_other_formats_newer_versions_and_bad_values() {
    let text = std::fs::read_to_string(fixture().join("manifest.json")).unwrap();
    let with = |from: &str, to: &str| {
        assert!(text.contains(from), "{from}");
        parse_manifest(&text.replacen(from, to, 1))
    };

    assert!(matches!(with("\"trenchdot-terrain3d\"", "\"trenchdot-other\""), Err(CacheError::Magic)));
    assert!(matches!(with("\"version\": 1", "\"version\": 2"), Err(CacheError::Newer(2))));
    // A newer manifest may drop fields this editor needs, the version still decides the error.
    assert!(matches!(parse_manifest(r#"{"format":"trenchdot-terrain3d","version":3}"#), Err(CacheError::Newer(3))));
    assert!(matches!(with("\"version\": 1", "\"version\": 0"), Err(CacheError::Invalid(_))));
    assert!(matches!(with("\"region_size\": 64", "\"region_size\": 100"), Err(CacheError::Invalid(m)) if m.contains("100")));
    assert!(matches!(with("\"vertex_spacing\": 2.5", "\"vertex_spacing\": 0.2"), Err(CacheError::Invalid(m)) if m.contains("0.2")));
    assert!(with("\"vertex_spacing\": 2.5", "\"vertex_spacing\": 100.0").is_ok());
    assert!(matches!(with("[\n    [\n      3,\n      -2\n    ]\n  ]", "[[16, 0]]"), Err(CacheError::Invalid(m)) if m.contains("[16, 0]")));
    assert!(matches!(with("\"hash\": \"b8", "\"hash\": \"zz"), Err(CacheError::Json(_))));
    assert!(matches!(parse_manifest("{"), Err(CacheError::Json(_))));
}

#[test]
fn region_fixtures_read_whole_and_the_second_keeps_nothing_of_the_first() {
    let first = read_region(&fixture(), [-1, 0], SIZE).unwrap();
    assert_eq!(first, CachedRegion { region: pattern([-1, 0], 0.0, true), vertex_spacing: 2.5, hash: fixture_hash([-1, 0]) });
    assert_eq!(first.region.control[..2], [0x7FC0_0001, 0xFF80_0001], "NaN control patterns keep their bits");

    let second = read_region(&fixture(), [3, -2], SIZE).unwrap();
    assert_eq!(second, CachedRegion { region: pattern([3, -2], 0.0, false), vertex_spacing: 2.5, hash: fixture_hash([3, -2]) });
    assert!(second.region.color.is_empty());
    assert_eq!(read_region(&fixture(), [-1, 0], SIZE).unwrap(), first);
}

#[test]
fn pending_fixture_reads_whole() {
    let p = read_pending(&fixture(), [3, -2], SIZE).unwrap();
    let expected = PendingRegion { region: pattern([3, -2], 1.0, false), vertex_spacing: 2.5, base: fixture_hash([3, -2]), force: true, remove: false };
    assert_eq!(p, expected);
    assert_eq!(pending_locs(&fixture()).unwrap(), [[3, -2]]);
}

#[test]
fn region_round_trips_bit_exact_without_leaving_a_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut region = pattern([-16, 15], 0.5, true);
    region.heights[5] = f32::from_bits(0x7FC0_1234);
    region.heights[6] = -0.0;
    let cached = CachedRegion { region, vertex_spacing: 0.25, hash: hash_of([7, 7]) };
    write_region(dir.path(), &cached).unwrap();
    assert_eq!(files(&dir.path().join("regions")), ["terrain3d-16_15.td3r"]);

    let back = read_region(dir.path(), [-16, 15], SIZE).unwrap();
    let bits = |r: &CachedRegion| r.region.heights.iter().map(|h| h.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&back), bits(&cached));
    assert_eq!((back.region.loc, back.region.size, back.vertex_spacing, back.hash), ([-16, 15], SIZE, 0.25, cached.hash));
    assert_eq!((back.region.control, back.region.color), (cached.region.control, cached.region.color));
}

#[test]
fn pending_round_trips_every_flag() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(pending_locs(dir.path()).unwrap(), Vec::<[i32; 2]>::new(), "no pending folder yet");

    let cases = [
        ([0, 0], false, false, RegionHash::ZERO),
        ([-1, 2], true, false, hash_of([1, 1])),
        ([15, -16], false, true, hash_of([2, 2])),
        ([3, 4], true, true, hash_of([3, 3])),
    ];
    for (loc, force, remove, base) in cases {
        let p = PendingRegion { region: pattern(loc, -3.0, loc[0] < 0), vertex_spacing: 1.0, base, force, remove };
        write_pending(dir.path(), &p).unwrap();
        assert_eq!(read_pending(dir.path(), loc, SIZE).unwrap(), p, "{loc:?}");
    }

    assert_eq!(files(&dir.path().join("pending")).len(), 4, "no temporary file stays behind");
    assert_eq!(pending_locs(dir.path()).unwrap(), [[-1, 2], [0, 0], [3, 4], [15, -16]]);
}

#[test]
fn pending_locs_skip_files_being_written_and_strangers() {
    let dir = tempfile::tempdir().unwrap();
    let pending = dir.path().join("pending");
    std::fs::create_dir_all(&pending).unwrap();
    for name in
        ["terrain3d-01_00.td3r.tdpart", "terrain3d-01_00.res.td3r", "terrain3d_16_00.td3r", "terrain3d_02_03.td3l", "settings.json", "terrain3d-1_00.td3r"]
    {
        std::fs::write(pending.join(name), b"x").unwrap();
    }

    assert!(pending_locs(dir.path()).unwrap().is_empty());
    std::fs::write(pending.join("terrain3d_02-03.td3r"), b"x").unwrap();
    assert_eq!(pending_locs(dir.path()).unwrap(), [[2, -3]]);
}

#[test]
fn damaged_region_files_are_errors() {
    let good = std::fs::read(fixture().join("regions/terrain3d-01_00.td3r")).unwrap();
    assert!(decode_region(&good).is_ok());

    for cut in [0, 3, 59] {
        assert!(matches!(decode_region(&good[..cut]), Err(CacheError::Truncated)), "cut at {cut}");
    }

    assert!(matches!(decode_region(&good[..60]), Err(CacheError::Invalid(_))), "header without maps");
    assert!(matches!(decode_region(&good[..good.len() / 2]), Err(CacheError::Invalid(_))), "half the maps");

    let mut wrong = good.clone();
    wrong[..4].copy_from_slice(b"TD3L");
    assert!(matches!(decode_region(&wrong), Err(CacheError::Magic)));

    let mut newer = good.clone();
    header(&mut newer, 4, 2);
    assert!(matches!(decode_region(&newer), Err(CacheError::Newer(2))));

    let mut size = good.clone();
    header(&mut size, 16, 128);
    assert!(matches!(decode_region(&size), Err(CacheError::Invalid(m)) if m.contains("size 128")), "header bigger than the maps");
    header(&mut size, 16, 100);
    assert!(matches!(decode_region(&size), Err(CacheError::Invalid(m)) if m.contains("100")), "no region size");

    let mut big = encode_region(&CachedRegion { region: Region3D::blank([-1, 0], 128), vertex_spacing: 2.5, hash: RegionHash::ZERO }).unwrap();
    header(&mut big, 16, 64);
    assert!(matches!(decode_region(&big), Err(CacheError::Invalid(m)) if m.contains("size 64")), "maps bigger than the header");

    let mut spacing = good.clone();
    spacing[20..24].copy_from_slice(&0.24f32.to_le_bytes());
    assert!(matches!(decode_region(&spacing), Err(CacheError::Invalid(m)) if m.contains("spacing")));
    spacing[20..24].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(matches!(decode_region(&spacing), Err(CacheError::Invalid(m)) if m.contains("spacing")));

    let mut loc = good.clone();
    header(&mut loc, 8, 16);
    assert!(matches!(decode_region(&loc), Err(CacheError::Invalid(m)) if m.contains("[16, 0]")));

    let pending = std::fs::read(fixture().join("pending/terrain3d_03-02.td3r")).unwrap();
    assert!(decode_region(&pending).is_err(), "a pending file is not a region file");
    assert!(matches!(decode_pending(&good), Err(CacheError::Invalid(m)) if m.contains("BASE")), "a region file is not a pending file");
    assert!(matches!(decode_pending(&pending[..pending.len() - 1]), Err(CacheError::Invalid(_))));
    assert!(matches!(decode_pending(&pending[..99]), Err(CacheError::Truncated)));
}

#[test]
fn reading_checks_the_file_against_its_name_and_the_manifest() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(read_region(&fixture(), [-1, 0], 128), Err(CacheError::Invalid(m)) if m.contains("size 64") && m.contains("128")));
    assert!(matches!(read_pending(&fixture(), [3, -2], 256), Err(CacheError::Invalid(m)) if m.contains("256")));
    assert!(matches!(read_region(dir.path(), [0, 0], SIZE), Err(CacheError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound));
    assert!(matches!(read_region(dir.path(), [16, 0], SIZE), Err(CacheError::Invalid(_))));

    std::fs::create_dir_all(dir.path().join("regions")).unwrap();
    std::fs::copy(fixture().join("regions/terrain3d_03-02.td3r"), dir.path().join("regions/terrain3d_00_00.td3r")).unwrap();
    assert!(matches!(read_region(dir.path(), [0, 0], SIZE), Err(CacheError::Invalid(m)) if m.contains("[3, -2]")));
}

#[test]
fn writers_refuse_bad_regions_and_leave_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut short = pattern([1, 1], 0.0, false);
    short.heights.pop();
    assert!(matches!(write_region(dir.path(), &CachedRegion { region: short, vertex_spacing: 1.0, hash: RegionHash::ZERO }), Err(CacheError::Invalid(_))));

    let mut odd_color = pattern([1, 1], 0.0, true);
    odd_color.color.pop();
    let p = PendingRegion { region: odd_color, vertex_spacing: 1.0, base: RegionHash::ZERO, force: false, remove: false };
    assert!(matches!(write_pending(dir.path(), &p), Err(CacheError::Invalid(_))));

    let spaced = PendingRegion { region: pattern([1, 1], 0.0, false), vertex_spacing: 100.5, ..p };
    assert!(matches!(write_pending(dir.path(), &spaced), Err(CacheError::Invalid(m)) if m.contains("spacing")));
    assert!(files(dir.path()).is_empty());
}

fn lod(hash: RegionHash, lift: f32) -> Lod {
    let level = |stride: u32, side: u32, ids: Vec<u8>, splat: bool, holes: bool| {
        let n = (side * side) as usize;
        LodLevel {
            stride,
            side,
            ids,
            heights: (0..n).map(|i| i as f32 * 0.5 - 3.0 + lift).collect(),
            splat: if splat { (0..n * 4).map(|i| (i * 7) as u8).collect() } else { Vec::new() },
            holes: if holes { (0..n).map(|i| (i % 5 == 0) as u8).collect() } else { Vec::new() },
        }
    };
    Lod {
        loc: [-2, 3],
        region_size: 256,
        hash,
        levels: vec![level(1, 257, vec![0, 31, 4, 9], true, true), level(4, 64, vec![2], false, false), level(16, 17, vec![], false, true)],
    }
}

#[test]
fn lod_files_are_kept_per_region_hash() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (lod(hash_of([1, 0]), 0.0), lod(hash_of([0, 1]), 9.0));
    write_lod(dir.path(), &a).unwrap();
    write_lod(dir.path(), &b).unwrap();
    assert_eq!(read_lod(dir.path(), [-2, 3], 256, a.hash).unwrap(), a);
    assert_eq!(read_lod(dir.path(), [-2, 3], 256, b.hash).unwrap(), b);
    assert!(matches!(read_lod(dir.path(), [-2, 3], 256, hash_of([5, 5])), Err(CacheError::Io(_))));
    assert!(matches!(read_lod(dir.path(), [-2, 3], 512, a.hash), Err(CacheError::Invalid(_))));

    let renamed = lod_path(dir.path(), [-2, 3], hash_of([5, 5])).unwrap();
    std::fs::copy(lod_path(dir.path(), [-2, 3], a.hash).unwrap(), &renamed).unwrap();
    assert!(matches!(read_lod(dir.path(), [-2, 3], 256, hash_of([5, 5])), Err(CacheError::Invalid(_))), "a file under another hash's name");
    std::fs::remove_file(renamed).unwrap();

    let other = Lod { loc: [0, 0], ..a.clone() };
    write_lod(dir.path(), &other).unwrap();
    std::fs::write(dir.path().join("lod/terrain3d-02_03.notahash.td3l"), b"x").unwrap();
    assert_eq!(prune_lod(dir.path(), [-2, 3], b.hash).unwrap(), 1);
    let mut left = vec![format!("terrain3d-02_03.{}.td3l", b.hash), "terrain3d-02_03.notahash.td3l".into(), format!("terrain3d_00_00.{}.td3l", a.hash)];
    left.sort();
    assert_eq!(files(&dir.path().join("lod")), left, "other regions and strangers stay");
    assert_eq!(prune_lod(dir.path(), [-2, 3], b.hash).unwrap(), 0);
    assert_eq!(prune_lod(tempfile::tempdir().unwrap().path(), [-2, 3], b.hash).unwrap(), 0, "no lod folder");
}

#[test]
fn lod_levels_are_checked() {
    let good = lod(RegionHash::ZERO, 0.0);
    assert!(decode_lod(&encode_lod(&good).unwrap()).is_ok());
    let bad = |edit: &dyn Fn(&mut Lod)| {
        let mut l = good.clone();
        edit(&mut l);
        encode_lod(&l)
    };

    assert!(bad(&|l| l.levels[1].stride = 2).is_err(), "stride outside the pyramid");
    assert!(bad(&|l| l.levels[1].side = 66).is_err(), "side neither cells nor cells plus one");
    assert!(bad(&|l| l.levels[0].ids.push(1)).is_err(), "five layers");
    assert!(bad(&|l| l.levels[1].ids = vec![32]).is_err(), "texture id 32");
    assert!(bad(&|l| l.levels[1].heights.truncate(10)).is_err());
    assert!(bad(&|l| l.levels[2].holes.push(0)).is_err());
    assert!(bad(&|l| l.levels[0].splat.truncate(4)).is_err());
    assert!(bad(&|l| l.levels.extend(good.levels.clone())).is_err(), "more levels than strides");
    assert!(bad(&|l| l.region_size = 100).is_err());

    let bytes = encode_lod(&good).unwrap();
    let mut wrong = bytes.clone();
    wrong[..4].copy_from_slice(b"TD3R");
    assert!(matches!(decode_lod(&wrong), Err(CacheError::Magic)));
    let mut newer = bytes.clone();
    header(&mut newer, 4, 2);
    assert!(matches!(decode_lod(&newer), Err(CacheError::Newer(2))));
    assert!(matches!(decode_lod(&bytes[..40]), Err(CacheError::Truncated)));
    assert!(decode_lod(&bytes[..bytes.len() - 10]).is_err());

    let raw = |levels: &[u8]| {
        let mut out = bytes[..56].to_vec();
        header(&mut out, 52, 1);
        out.extend(zstd::bulk::compress(levels, 3).unwrap());
        decode_lod(&out)
    };
    let mut junk_side = 4u32.to_le_bytes().to_vec();
    junk_side.extend(u32::MAX.to_le_bytes());
    junk_side.extend([0, 0]);
    assert!(matches!(raw(&junk_side), Err(CacheError::Invalid(m)) if m.contains("4294967295")), "a junk side never sizes a read");
    let mut extra = 16u32.to_le_bytes().to_vec();
    extra.extend(16u32.to_le_bytes());
    extra.extend([0, 0]);
    extra.extend(vec![0u8; 16 * 16 * 4 + 1]);
    assert!(matches!(raw(&extra), Err(CacheError::Invalid(m)) if m.contains("1 bytes after")));
    extra.pop();
    assert_eq!(raw(&extra).unwrap().levels[0].heights, vec![0.0; 256]);
}

#[test]
fn cache_dir_stays_inside_the_project() {
    let root = Path::new("proj");
    assert_eq!(cache_dir(root, "res://terrain3d_data"), Some(root.join("terrain3d_data").join(".trenchdot")));
    assert_eq!(cache_dir(root, "res://worlds/a/"), Some(root.join("worlds").join("a").join(".trenchdot")));
    assert_eq!(cache_dir(root, "res://"), None);
    assert_eq!(cache_dir(root, "res://."), None);
    assert_eq!(cache_dir(root, "res://../other"), None);
    assert_eq!(region_path(root, [16, 0]), None);
    assert_eq!(pending_path(root, [-1, 2]), Some(root.join("pending").join("terrain3d-01_02.td3r")));
}

#[test]
fn hashes_read_as_hex_either_case() {
    let h = hash_of([-1, 0]);
    assert_eq!(h.to_string().to_uppercase().parse::<RegionHash>(), Ok(h));
    assert!("f5".parse::<RegionHash>().is_err());
    assert!(format!("{}g", &h.to_string()[1..]).parse::<RegionHash>().is_err());
    assert!(format!("{}é", &h.to_string()[2..]).parse::<RegionHash>().is_err());
}
