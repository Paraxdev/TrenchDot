//! `td_geom::terrain3d::surface_height` against Terrain3D's own `get_surface_height`, sampled by
//! godot/tests/terrain3d_parity_tests.gd over a cache the addon exported. Reading that cache also proves Rust parses a
//! manifest and regions Godot wrote.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use td_formats::terrain3d_cache::*;
use td_geom::terrain3d::{EdgeBlend, Grid, Region3D, surface_height};

fn parity() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/terrain3d/parity")
}

fn samples(mode: &str) -> Vec<(f32, f32, Option<f32>)> {
    let text = std::fs::read_to_string(parity().join("samples.json")).unwrap();
    let all: HashMap<String, Vec<(f64, f64, Option<f64>)>> = serde_json::from_str(&text).unwrap();
    all[mode].iter().map(|&(x, z, h)| (x as f32, z as f32, h.map(|h| h as f32))).collect()
}

fn check(mode: &str, edge: Option<EdgeBlend>) {
    let manifest = read_manifest(&parity()).unwrap();
    let regions: HashMap<[i32; 2], Region3D> =
        manifest.regions.iter().map(|r| (r.loc, read_region(&parity(), r.loc, manifest.region_size).unwrap().region)).collect();
    assert_eq!(regions.len(), 3);
    let grid = Grid { region_size: manifest.region_size, spacing: manifest.vertex_spacing, upm: 32.0 };
    let points = samples(mode);
    assert_eq!(points.len(), 200);
    let wrong: Vec<_> = points
        .iter()
        .filter_map(|&(x, z, want)| {
            let got = surface_height(&grid, edge, |loc| regions.get(&loc), x, z);
            let same = match want {
                None => got.is_nan(),
                Some(want) => (got - want).abs() <= 1e-4,
            };
            (!same).then_some((x, z, want, got))
        })
        .collect();
    assert!(wrong.is_empty(), "{} of 200 {mode} samples differ from Terrain3D, (x, z, want, got) {:?}", wrong.len(), &wrong[..wrong.len().min(8)]);
}

#[test]
fn surface_height_matches_terrain3d_without_a_background() {
    check("none", None);
}

#[test]
fn surface_height_matches_terrain3d_blending_toward_the_ground_level() {
    let material = read_manifest(&parity()).unwrap().material;
    assert_eq!(material.world_background, 1);
    let edge = EdgeBlend { ground_level: material.ground_level as f32, region_blend: material.region_blend as f32 };
    assert_eq!(edge, EdgeBlend { ground_level: -3.0, region_blend: 0.9 });
    check("flat", Some(edge));
}

#[test]
fn the_samples_cover_holes_edges_and_both_sides_of_the_vertex_snap() {
    let none = samples("none");
    let flat = samples("flat");
    let holes = none.iter().filter(|p| p.2.is_none()).count();
    assert!((20..180).contains(&holes), "{holes} of 200 samples are holes or outside");
    let blended = none.iter().zip(&flat).filter(|(a, b)| a.2 != b.2).count();
    assert!(blended >= 20, "only {blended} samples change with the Flat background");
    assert!(none.iter().zip(&flat).all(|(a, b)| a.2.is_none() == b.2.is_none() && (a.0, a.1) == (b.0, b.1)));
}
