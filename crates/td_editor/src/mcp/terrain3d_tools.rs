//! MCP tools for the Terrain3D node: `terrain_edit` on a Terrain3D id.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{Value, json};
use td_core::{DVec2, DVec3, NodeId};
use td_doc::terrain::{SculptBrush, SculptMode};
use td_doc::terrain3d::{RegionSource, reground_scatter};
use td_geom::terrain3d::{Loc, roughness_percent};
use td_geom::terrain3d_tile::{Flag, Paint, PaintMode};

use super::ToolResult;
use super::content_tools::{point, resample};
use super::tools::uint;
use crate::app::App;
use crate::state::EditorState;
use crate::terrain3d::brush::{color_hex, parse_color};

/// How long a tool waits for the regions it edits to load from the cache.
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a tool waits for the region files of a folder Godot has not exported to be read.
pub(super) const SCAN_TIMEOUT: Duration = Duration::from_secs(30);

fn err(msg: impl Into<String>) -> ToolResult {
    ToolResult::Error(msg.into())
}

fn ok(v: Value) -> ToolResult {
    ToolResult::Json(v)
}

/// Largest map unit distance a terrain_edit position, radius, spacing, margin or width may have, past Terrain3D's whole region map.
const REACH_LIMIT: f64 = 1.0e7;
/// Largest map unit height or strength, 31 km, beyond any ground and where Terrain3D's 32 bit heights lose their detail.
const HEIGHT_LIMIT: f64 = 1.0e6;

/// Most dabs one path may resample into.
const MAX_DABS: f64 = 100_000.0;

fn within(name: &str, v: f64, limit: f64) -> Result<(), String> {
    if v.abs() <= limit { Ok(()) } else { Err(format!("{name} {v} is out of range, it must be within {limit} map units")) }
}

fn positive(name: &str, v: f64) -> Result<(), String> {
    if v > 0.0 { within(name, v, REACH_LIMIT) } else { Err(format!("{name} {v} must be above 0")) }
}

/// Checks a paint_texture id against the texture ids the node's cache manifest lists. A listed set refuses an id outside
/// it, an empty list (an old cache, or a folder with no texture assets yet) accepts any id and returns a note.
fn check_texture_known(id: u8, known: &[u32]) -> Result<Option<String>, String> {
    if known.is_empty() {
        return Ok(Some(format!(
            "the node lists no textures, so texture {id} was not checked and may show nothing until Godot has a texture asset with that id"
        )));
    }

    if known.contains(&u32::from(id)) {
        return Ok(None);
    }

    let mut ids = known.to_vec();
    ids.sort_unstable();
    let list: Vec<String> = ids.iter().map(u32::to_string).collect();
    Err(format!("texture {id} is not one of the texture ids this Terrain3D node has: {}", list.join(", ")))
}

fn xz(p: DVec3) -> DVec2 {
    DVec2::new(p.x, p.z)
}

/// One terrain_edit op on a Terrain3D node, parsed before anything loads or changes.
#[derive(Debug)]
pub(crate) enum Job {
    Dabs { brush: SculptBrush, points: Vec<DVec3> },
    Paint { paint: Paint, strength: f32, radius: f64, points: Vec<DVec3> },
    Flatten { min: DVec2, max: DVec2, height: f64, margin: f64 },
    Ramp { from: DVec3, to: DVec3, width: f64, margin: f64 },
}

impl Job {
    pub(crate) fn parse(op: &str, a: &Value) -> Result<Job, String> {
        let job = Self::parse_unchecked(op, a)?;
        job.check_range()?;
        Ok(job)
    }

    /// Refuses values past what the ground can hold, they would write heights of billions of meters into the regions.
    fn check_range(&self) -> Result<(), String> {
        let spot = |p: &DVec3| within("position", p.x, REACH_LIMIT).and_then(|_| within("position", p.z, REACH_LIMIT));
        match self {
            Job::Dabs { brush, points } => {
                positive("radius", brush.radius)?;
                within("strength", brush.strength, HEIGHT_LIMIT)?;
                within("height", brush.flatten_height, HEIGHT_LIMIT)?;
                positive("step", brush.terrace_step)?;
                points.iter().try_for_each(spot)
            }
            Job::Paint { radius, points, .. } => {
                positive("radius", *radius)?;
                points.iter().try_for_each(spot)
            }
            Job::Flatten { min, max, height, margin } => {
                within("height", *height, HEIGHT_LIMIT)?;
                within("margin", *margin, REACH_LIMIT)?;
                [min, max].into_iter().try_for_each(|p| spot(&DVec3::new(p.x, 0.0, p.y)))
            }
            Job::Ramp { from, to, width, margin } => {
                positive("width", *width)?;
                within("margin", *margin, REACH_LIMIT)?;
                [from, to].into_iter().try_for_each(|p| spot(p).and_then(|_| within("height", p.y, HEIGHT_LIMIT)))
            }
        }
    }

    fn parse_unchecked(op: &str, a: &Value) -> Result<Job, String> {
        let radius = a["radius"].as_f64().unwrap_or(256.0);
        let strength = a["strength"].as_f64().unwrap_or(16.0);
        let center = point(&a["center"]);
        let one = || center.map(|c| vec![c]).ok_or_else(|| "center [x, z] required".to_string());
        let many = |spacing: f64| {
            let points: Vec<DVec3> = a["points"].as_array().into_iter().flatten().filter_map(point).collect();
            if points.is_empty() {
                return Err("points [[x, z], ...] required".to_string());
            }

            points.iter().try_for_each(|p| within("position", p.x, REACH_LIMIT).and_then(|_| within("position", p.z, REACH_LIMIT)))?;
            let length: f64 = points.windows(2).map(|w| (w[1] - w[0]).length()).sum();
            if length / spacing.max(1.0) > MAX_DABS {
                return Err(format!("the path would take more than {MAX_DABS} dabs, raise spacing or shorten it"));
            }

            Ok(resample(&points, spacing))
        };
        let mut brush = SculptBrush {
            radius,
            strength,
            flatten_height: a["height"].as_f64().unwrap_or(0.0),
            terrace_step: a["step"].as_f64().unwrap_or(32.0),
            layer: a["layer"].as_u64().unwrap_or(1) as u8,
            ..SculptBrush::default()
        };
        match op {
            "sculpt" | "sculpt_path" => {
                brush.mode = serde_json::from_value(a["mode"].clone())
                    .map_err(|_| "mode must be raise, lower, smooth, flatten, noise, terrace, hole or unhole".to_string())?;
                if matches!(brush.mode, SculptMode::PaintLayer | SculptMode::PaintAlpha | SculptMode::EraseAlpha) {
                    return Err("Terrain3D paints texture ids, use paint_texture".into());
                }

                let points = if op == "sculpt" { one()? } else { many(a["spacing"].as_f64().unwrap_or(radius * 0.5))? };
                Ok(Job::Dabs { brush, points })
            }
            "holes" => {
                if !a["min"].is_null() || !a["max"].is_null() {
                    return Err("Terrain3D holes take center and radius, not a rectangle".into());
                }

                brush.mode = if a["hole"].as_bool().unwrap_or(true) { SculptMode::Hole } else { SculptMode::Unhole };
                Ok(Job::Dabs { brush, points: one()? })
            }
            "paint_texture" | "paint_navigation" | "paint_autoshader" | "paint_color" | "paint_wetness" => {
                let erase = a["erase"].as_bool().unwrap_or(false);
                let paint = match op {
                    "paint_texture" => {
                        let id = uint(&a["texture"]).filter(|t| *t < 32).ok_or("texture, a Terrain3D texture id from 0 to 31, is required")? as u8;
                        let mode = match a["mode"].as_str().unwrap_or("base") {
                            "base" => PaintMode::Base,
                            "spray" => PaintMode::Spray,
                            _ => return Err("mode must be base or spray".into()),
                        };
                        Paint::Texture { id, mode }
                    }
                    "paint_navigation" => Paint::Flag { flag: Flag::Navigation, on: a["on"].as_bool().unwrap_or(true) },
                    "paint_autoshader" => Paint::Flag { flag: Flag::Autoshader, on: a["on"].as_bool().unwrap_or(true) },
                    "paint_color" => {
                        let rgb = match &a["color"] {
                            Value::Null if erase => [255; 3],
                            Value::Null => return Err("color #rrggbb required, or erase true".into()),
                            v => v.as_str().and_then(parse_color).ok_or("color must be #rrggbb")?,
                        };
                        Paint::Color { rgb, erase }
                    }
                    _ => {
                        let roughness = match &a["roughness"] {
                            Value::Null => -50.0,
                            v => v.as_f64().filter(|r| (-100.0..=100.0).contains(r)).ok_or("roughness must be a number from -100 to 100")?,
                        };
                        Paint::Wetness { roughness: roughness.round() as i8, erase }
                    }
                };
                let points = if center.is_some() { one()? } else { many(a["spacing"].as_f64().unwrap_or(radius * 0.4))? };
                Ok(Job::Paint { paint, strength: a["strength"].as_f64().unwrap_or(1.0).clamp(0.0, 1.0) as f32, radius, points })
            }
            "flatten_rect" => {
                let (Some(min), Some(max)) = (point(&a["min"]), point(&a["max"])) else { return Err("min and max [x, z] required".into()) };
                Ok(Job::Flatten { min: xz(min), max: xz(max), height: a["height"].as_f64().unwrap_or(0.0), margin: a["margin"].as_f64().unwrap_or(64.0) })
            }
            "ramp" => {
                let vec3 = |v: &Value| -> Option<DVec3> {
                    let l = v.as_array()?;
                    (l.len() == 3).then(|| DVec3::new(l[0].as_f64().unwrap_or(f64::NAN), l[1].as_f64().unwrap_or(f64::NAN), l[2].as_f64().unwrap_or(f64::NAN)))
                };
                let (Some(from), Some(to)) = (vec3(&a["from"]), vec3(&a["to"])) else { return Err("from and to [x, y, z] required".into()) };
                Ok(Job::Ramp { from, to, width: a["width"].as_f64().unwrap_or(96.0), margin: a["margin"].as_f64().unwrap_or(64.0) })
            }
            other => Err(format!(
                "terrain_edit op {other} does not apply to Terrain3D, use sculpt, sculpt_path, paint_texture, paint_color, paint_wetness, \
                 paint_navigation,paint_autoshader, flatten_rect, ramp, holes, or only probe"
            )),
        }
    }

    /// The map unit rectangle the job reads and writes.
    pub(crate) fn bounds(&self) -> (DVec2, DVec2) {
        let around = |points: &[DVec3], reach: f64| {
            let first = points.first().map_or(DVec2::ZERO, |p| xz(*p));
            let (lo, hi) = points.iter().fold((first, first), |(lo, hi), p| (lo.min(xz(*p)), hi.max(xz(*p))));
            (lo - DVec2::splat(reach), hi + DVec2::splat(reach))
        };
        match self {
            Job::Dabs { brush, points } => around(points, brush.radius),
            Job::Paint { radius, points, .. } => around(points, *radius),
            Job::Flatten { min, max, margin, .. } => (min.min(*max) - DVec2::splat(*margin), min.max(*max) + DVec2::splat(*margin)),
            Job::Ramp { from, to, width, margin } => around(&[*from, *to], width * 0.5 + margin),
        }
    }
}

impl EditorState {
    /// Loads the regions of node `id` that meet a map unit rectangle (and the ring around it the edits read), waiting for
    /// the cache worker. Tools need whole regions, the views only load what they draw.
    pub(crate) fn load_terrain3d_rect(&mut self, id: NodeId, min: DVec2, max: DVec2) -> Result<(), String> {
        let (_, cache) = self.terrain3d_links().into_iter().find(|(node, _)| *node == id).ok_or("Open a Godot project first, Terrain3D data lives in one")?;
        self.terrain3d.upm = self.game.units_per_meter;
        self.terrain3d.recheck();
        self.terrain3d.refresh(&cache);
        self.terrain3d.finish_scan(&cache, SCAN_TIMEOUT)?;
        if self.terrain3d.grid(&cache).is_none() {
            self.export_terrain3d_cache(&cache)?;
        }

        let grid = self.terrain3d.grid(&cache).ok_or(crate::terrain3d::bridge::NOT_EXPORTED)?;
        let (lo, hi) = grid.vgrid_rect(min, max, 2);
        let need: Vec<Loc> = {
            let source = self.terrain3d.source(&cache).ok_or("no Terrain3D cache")?;
            grid.regions_in(lo, hi).into_iter().filter(|l| source.hash(*l).is_some()).collect()
        };
        self.terrain3d.load_whole(&cache, &need, LOAD_TIMEOUT)
    }

    /// Runs `job` on the regions of node `id` as one undo step. Height jobs put the scatter that follows the node back on
    /// the new ground in the same step. Returns the regions changed, none leaves no undo step, and a note for the reply
    /// when a paint could not be checked against the node's textures.
    pub(crate) fn run_terrain3d_job(&mut self, id: NodeId, job: &Job, label: &str, seed: u32) -> Result<(Vec<Loc>, Option<String>), String> {
        let (min, max) = job.bounds();
        self.load_terrain3d_rect(id, min, max)?;
        let (_, cache) = self.terrain3d_links().into_iter().find(|(node, _)| *node == id).ok_or("no Terrain3D cache")?;
        let mut note = None;
        if let Job::Paint { paint: Paint::Texture { id: texture, .. }, .. } = job {
            let known: Vec<u32> = self.terrain3d.manifest(&cache).map(|m| m.textures.iter().map(|t| t.id).collect()).unwrap_or_default();
            note = check_texture_known(*texture, &known)?;
        }

        let source = self.terrain3d.source(&cache).ok_or("no Terrain3D cache")?;
        let done = self.doc.try_edit(label, |m, _| {
            let Some(link) = m.terrain3d_mut(id) else { return Err("the Terrain3D node vanished".to_string()) };
            let mut locs = BTreeSet::new();
            match job {
                Job::Dabs { brush, points } => {
                    let mut brush = *brush;
                    for (k, p) in points.iter().enumerate() {
                        brush.seed = seed.wrapping_add(k as u32);
                        locs.extend(link.sculpt(&source, *p, &brush));
                    }
                }
                Job::Paint { paint, strength, radius, points } => {
                    for p in points {
                        locs.extend(link.paint(&source, *p, *radius, paint, *strength));
                    }
                }
                Job::Flatten { min, max, height, margin } => locs.extend(link.flatten_rect(&source, *min, *max, *height, *margin)),
                Job::Ramp { from, to, width, margin } => locs.extend(link.ramp(&source, *from, *to, *width, *margin)),
            }

            let locs: Vec<Loc> = locs.into_iter().collect();
            let heights = match job {
                Job::Dabs { brush, .. } => !matches!(brush.mode, SculptMode::Hole | SculptMode::Unhole),
                Job::Paint { .. } => false,
                Job::Flatten { .. } | Job::Ramp { .. } => true,
            };
            if heights {
                reground_scatter(m, id, &source, &locs);
            }

            if locs.is_empty() { Err(String::new()) } else { Ok(locs) }
        });
        match done {
            Ok(locs) => Ok((locs, note)),
            Err(e) if e.is_empty() => Ok((Vec::new(), note)),
            Err(e) => Err(e),
        }
    }

    /// The surface height, texture ids, flags, color and wetness at `x`, `z` (map units) as JSON, after loading what they
    /// read.
    pub(crate) fn probe_terrain3d(&mut self, id: NodeId, x: f64, z: f64) -> Result<Value, String> {
        self.load_terrain3d_rect(id, DVec2::new(x, z), DVec2::new(x, z))?;
        let (_, cache) = self.terrain3d_links().into_iter().find(|(node, _)| *node == id).ok_or("no Terrain3D cache")?;
        let source = self.terrain3d.source(&cache).ok_or("no Terrain3D cache")?;
        let link = self.doc.map.terrain3d(id).ok_or("not a Terrain3D node")?;
        let p = link.probe(&source, x, z);
        let mut out = json!({ "x": x, "z": z, "height": p.height, "region": p.region });
        if let Some(c) = p.control {
            out["base"] = json!(c.base());
            out["overlay"] = json!(c.overlay());
            out["blend"] = json!(c.blend());
            out["hole"] = json!(c.hole());
            out["navigation"] = json!(c.navigation());
            out["auto"] = json!(c.auto());
        }

        if let Some([r, g, b, a]) = p.color {
            out["color"] = json!(color_hex([r, g, b]));
            out["roughness"] = json!(roughness_percent(a));
        }

        Ok(out)
    }
}

impl App {
    /// terrain_edit on a Terrain3D node.
    pub(crate) fn terrain_edit_terrain3d(&mut self, id: NodeId, args: &Value) -> ToolResult {
        let op = args["op"].as_str().unwrap_or_default();
        let probe = match &args["probe"] {
            Value::Null => None,
            v => match point(v) {
                Some(p) => Some(p),
                None => return err("probe must be [x, z]"),
            },
        };
        if op.is_empty() || op == "probe" {
            let Some(p) = probe else { return err("op required, or probe [x, z] to read the height") };
            return match self.state.probe_terrain3d(id, p.x, p.z) {
                Ok(v) => ok(json!({ "id": id.0, "probe_height": v["height"], "probe": v })),
                Err(e) => err(e),
            };
        }

        if !self.state.doc.map.is_editable(id) {
            return err(format!("Terrain3D {id} is hidden or locked"));
        }

        let job = match Job::parse(op, args) {
            Ok(j) => j,
            Err(e) => return err(e),
        };
        let seed = args["seed"].as_u64().unwrap_or(7) as u32;
        match self.state.run_terrain3d_job(id, &job, &format!("Terrain {op}"), seed) {
            Ok((locs, note)) => {
                let mut out = json!({ "id": id.0, "changed": !locs.is_empty(), "regions": locs });
                if let Some(note) = note {
                    out["note"] = json!(note);
                }

                if let Some(p) = probe {
                    match self.state.probe_terrain3d(id, p.x, p.z) {
                        Ok(v) => out["probe_height"] = v["height"].clone(),
                        Err(e) => return err(e),
                    }
                }

                ok(out)
            }
            Err(e) => err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(op: &str, a: Value) -> Result<Job, String> {
        Job::parse(op, &a)
    }

    #[test]
    fn a_sculpt_needs_a_center_and_a_mode_that_terrain3d_has() {
        assert!(parse("sculpt", json!({ "mode": "raise" })).unwrap_err().contains("center"));
        assert!(parse("sculpt", json!({ "center": [1.0, 2.0] })).unwrap_err().contains("mode"));
        for mode in ["paint_layer", "paint_alpha", "erase_alpha"] {
            assert!(parse("sculpt", json!({ "center": [1.0, 2.0], "mode": mode })).unwrap_err().contains("paint_texture"), "{mode}");
        }

        let Ok(Job::Dabs { brush, points }) =
            parse("sculpt", json!({ "center": [-130.5, 77.0], "mode": "terrace", "radius": 90.0, "step": 8.0, "strength": 3.0 }))
        else {
            panic!("a sculpt parses");
        };
        assert_eq!((brush.mode, brush.radius, brush.terrace_step, brush.strength), (SculptMode::Terrace, 90.0, 8.0, 3.0));
        assert_eq!(points, vec![DVec3::new(-130.5, 0.0, 77.0)]);
    }

    #[test]
    fn a_path_is_resampled_by_its_spacing_and_keeps_both_ends() {
        let Ok(Job::Dabs { points, .. }) =
            parse("sculpt_path", json!({ "points": [[0.0, 0.0], [100.0, 0.0], [100.0, 50.0]], "mode": "lower", "radius": 40.0, "spacing": 25.0 }))
        else {
            panic!("a path parses");
        };
        assert_eq!((points.first().copied(), points.last().copied()), (Some(DVec3::ZERO), Some(DVec3::new(100.0, 0.0, 50.0))));
        assert_eq!(points.len(), 7);
        assert!(parse("sculpt_path", json!({ "points": [], "mode": "raise" })).unwrap_err().contains("points"));
    }

    #[test]
    fn paint_texture_checks_the_id_at_its_boundary_and_the_mode() {
        let base = |extra: Value| {
            let mut a = json!({ "center": [10.0, 20.0], "radius": 16.0 });
            a.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            parse("paint_texture", a)
        };
        assert!(base(json!({})).unwrap_err().contains("texture"));
        assert!(base(json!({ "texture": 32 })).unwrap_err().contains("0 to 31"));
        assert!(base(json!({ "texture": -1 })).unwrap_err().contains("0 to 31"));
        assert!(base(json!({ "texture": 2, "mode": "smear" })).unwrap_err().contains("base or spray"));
        let Ok(Job::Paint { paint, strength, .. }) = base(json!({ "texture": 31, "mode": "spray", "strength": 4.0 })) else { panic!("parses") };
        assert_eq!((paint, strength), (Paint::Texture { id: 31, mode: PaintMode::Spray }, 1.0));
        let Ok(Job::Paint { paint, .. }) = base(json!({ "texture": 0 })) else { panic!("parses") };
        assert_eq!(paint, Paint::Texture { id: 0, mode: PaintMode::Base });
    }

    fn paint_of(op: &str, a: Value) -> Result<(Paint, f32, Vec<DVec3>), String> {
        match parse(op, a)? {
            Job::Paint { paint, strength, points, .. } => Ok((paint, strength, points)),
            other => panic!("{op} parsed as {other:?}"),
        }
    }

    #[test]
    fn every_paint_op_takes_a_center_or_points_and_needs_one() {
        let extra = |op: &str| match op {
            "paint_texture" => json!({ "texture": 3 }),
            "paint_color" => json!({ "color": "#102030" }),
            _ => json!({}),
        };
        for op in ["paint_texture", "paint_navigation", "paint_autoshader", "paint_color", "paint_wetness"] {
            let with = |more: Value| {
                let mut a = extra(op);
                a.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
                paint_of(op, a)
            };
            assert_eq!(with(json!({ "center": [-700.5, 310.0] })).unwrap().2, vec![DVec3::new(-700.5, 0.0, 310.0)], "{op}");
            assert_eq!(with(json!({ "points": [[-40.0, 12.0]] })).unwrap().2, vec![DVec3::new(-40.0, 0.0, 12.0)], "{op}");
            let path = with(json!({ "points": [[-40.0, 12.0], [60.0, 12.0]], "radius": 50.0 })).unwrap().2;
            assert_eq!((path.len(), path.last().copied()), (6, Some(DVec3::new(60.0, 0.0, 12.0))), "{op} spaces dabs radius * 0.4 apart");
            assert_eq!(with(json!({ "points": [[-40.0, 12.0], [60.0, 12.0]], "radius": 50.0, "spacing": 50.0 })).unwrap().2.len(), 3, "{op}");
            assert!(with(json!({})).unwrap_err().contains("points"), "{op}");
        }
    }

    #[test]
    fn flag_color_and_wetness_ops_check_their_values() {
        let at = |op: &str, more: Value| {
            let mut a = json!({ "center": [5.0, -9.0], "radius": 30.0 });
            a.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
            paint_of(op, a).map(|(paint, strength, _)| (paint, strength))
        };
        assert_eq!(at("paint_navigation", json!({})).unwrap().0, Paint::Flag { flag: Flag::Navigation, on: true });
        assert_eq!(at("paint_navigation", json!({ "on": false })).unwrap().0, Paint::Flag { flag: Flag::Navigation, on: false });
        assert_eq!(at("paint_autoshader", json!({})).unwrap().0, Paint::Flag { flag: Flag::Autoshader, on: true });
        assert_eq!(at("paint_autoshader", json!({ "on": false })).unwrap().0, Paint::Flag { flag: Flag::Autoshader, on: false });

        assert_eq!(at("paint_color", json!({ "color": "#B4966E" })).unwrap(), (Paint::Color { rgb: [0xb4, 0x96, 0x6e], erase: false }, 1.0));
        for bad in [json!("#12345"), json!("red"), json!("#gggggg"), json!("b4966e"), json!(0xb4966e)] {
            assert!(at("paint_color", json!({ "color": bad })).unwrap_err().contains("#rrggbb"), "{bad}");
        }

        assert!(at("paint_color", json!({})).unwrap_err().contains("required"));
        assert_eq!(at("paint_color", json!({ "erase": true })).unwrap().0, Paint::Color { rgb: [255; 3], erase: true });
        assert_eq!(at("paint_color", json!({ "color": "#000000", "strength": 3.0 })).unwrap().1, 1.0);
        assert_eq!(at("paint_color", json!({ "color": "#000000", "strength": -2.0 })).unwrap().1, 0.0);
        assert_eq!(at("paint_color", json!({ "color": "#000000", "strength": 0.25 })).unwrap().1, 0.25);

        assert_eq!(at("paint_wetness", json!({})).unwrap(), (Paint::Wetness { roughness: -50, erase: false }, 1.0));
        for (given, kept) in [(-100.0, -100), (100.0, 100), (0.0, 0), (33.4, 33), (-0.6, -1)] {
            assert_eq!(at("paint_wetness", json!({ "roughness": given })).unwrap().0, Paint::Wetness { roughness: kept, erase: false }, "{given}");
        }

        for bad in [json!(-101), json!(101), json!(100.5), json!("wet")] {
            assert!(at("paint_wetness", json!({ "roughness": bad })).unwrap_err().contains("-100 to 100"), "{bad}");
        }

        assert_eq!(at("paint_wetness", json!({ "roughness": 20, "erase": true })).unwrap().0, Paint::Wetness { roughness: 20, erase: true });
        assert!(at("paint_wetness", json!({ "radius": 1.0e9 })).unwrap_err().contains("radius"));
    }

    #[test]
    fn a_texture_id_must_be_one_the_node_lists_unless_it_lists_none() {
        assert_eq!(check_texture_known(0, &[0, 1]), Ok(None));
        assert_eq!(check_texture_known(1, &[0, 1]), Ok(None));
        let refused = check_texture_known(2, &[1, 0]).unwrap_err();
        assert!(refused.contains("texture 2") && refused.ends_with("0, 1"), "{refused}");
        assert_eq!(check_texture_known(5, &[2, 5, 9]), Ok(None));
        assert!(check_texture_known(4, &[2, 5, 9]).unwrap_err().ends_with("2, 5, 9"));
        assert!(check_texture_known(31, &[31]).unwrap().is_none());
        assert!(check_texture_known(30, &[31]).is_err());
        let note = check_texture_known(17, &[]).unwrap().expect("an empty list notes it");
        assert!(note.contains("17") && note.contains("no textures"), "{note}");
        assert!(check_texture_known(0, &[]).unwrap().is_some());
        assert!(check_texture_known(31, &[]).unwrap().is_some());
    }

    #[test]
    fn holes_ramps_and_rectangles_check_their_arguments() {
        assert!(parse("holes", json!({ "min": [0.0, 0.0], "max": [4.0, 4.0] })).unwrap_err().contains("center and radius"));
        let Ok(Job::Dabs { brush, .. }) = parse("holes", json!({ "center": [4.0, 4.0], "hole": false })) else { panic!("parses") };
        assert_eq!(brush.mode, SculptMode::Unhole);
        assert!(parse("ramp", json!({ "from": [0.0, 0.0], "to": [1.0, 1.0, 1.0] })).unwrap_err().contains("[x, y, z]"));
        assert!(parse("flatten_rect", json!({ "min": [0.0, 0.0] })).unwrap_err().contains("min and max"));
        for op in ["erode", "auto_paint", "set_layers", "clear_layer", "paint_path", "bogus"] {
            assert!(parse(op, json!({})).unwrap_err().contains("does not apply to Terrain3D"), "{op}");
        }
    }

    #[test]
    fn absurd_numbers_are_refused_at_their_limit() {
        let sculpt = |extra: Value| {
            let mut a = json!({ "center": [-130.5, 77.0], "mode": "raise", "radius": 90.0 });
            a.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            parse("sculpt", a)
        };
        assert!(sculpt(json!({})).is_ok());
        assert!(sculpt(json!({ "radius": 1.0e7 })).is_ok());
        assert!(sculpt(json!({ "radius": 1.0e9 })).unwrap_err().contains("radius"));
        assert!(sculpt(json!({ "radius": -5.0 })).unwrap_err().contains("above 0"));
        assert!(sculpt(json!({ "radius": 0.0 })).unwrap_err().contains("above 0"));
        assert!(sculpt(json!({ "center": [1.0e12, 3.0] })).unwrap_err().contains("position"));
        assert!(sculpt(json!({ "center": [3.0, -1.0e7 - 1.0] })).unwrap_err().contains("position"));
        assert!(sculpt(json!({ "center": [-1.0e7, 1.0e7] })).is_ok());
        assert!(sculpt(json!({ "strength": 1.0e6 })).is_ok());
        assert!(sculpt(json!({ "strength": -1.0e9 })).unwrap_err().contains("strength"));
        assert!(sculpt(json!({ "mode": "flatten", "height": 1.0e12 })).unwrap_err().contains("height"));
        assert!(sculpt(json!({ "mode": "flatten", "height": -1.0e6 })).is_ok());
        assert!(sculpt(json!({ "mode": "terrace", "step": 0.0 })).unwrap_err().contains("step"));
    }

    #[test]
    fn rectangles_ramps_paths_and_paint_check_their_range_too() {
        assert!(parse("flatten_rect", json!({ "min": [0.0, 0.0], "max": [1.0e12, 4.0] })).unwrap_err().contains("position"));
        assert!(parse("flatten_rect", json!({ "min": [-9.0, 3.0], "max": [40.0, 4.0], "height": 1.0e12 })).unwrap_err().contains("height"));
        assert!(parse("flatten_rect", json!({ "min": [-9.0, 3.0], "max": [40.0, 4.0], "height": 12.5 })).is_ok());
        assert!(parse("ramp", json!({ "from": [0.0, 5.0, 0.0], "to": [10.0, 1.0e12, 3.0] })).unwrap_err().contains("height"));
        assert!(parse("ramp", json!({ "from": [0.0, 5.0, 0.0], "to": [10.0, 9.0, 3.0], "width": -1.0 })).unwrap_err().contains("width"));
        assert!(parse("ramp", json!({ "from": [0.0, 5.0, 0.0], "to": [10.0, 9.0, 3.0] })).is_ok());
        assert!(parse("paint_texture", json!({ "texture": 1, "center": [4.0, 5.0], "radius": 1.0e9 })).unwrap_err().contains("radius"));
        let path = |points: Value, spacing: f64| parse("sculpt_path", json!({ "points": points, "mode": "raise", "radius": 10.0, "spacing": spacing }));
        assert!(path(json!([[0.0, 0.0], [1.0e12, 0.0]]), 25.0).unwrap_err().contains("position"));
        assert!(path(json!([[0.0, 0.0], [5.0e6, 0.0]]), 1.0).unwrap_err().contains("dabs"));
        assert!(path(json!([[0.0, 0.0], [99_999.0, 0.0]]), 1.0).is_ok());
        assert!(path(json!([[0.0, 0.0], [100_001.0, 0.0]]), 1.0).unwrap_err().contains("dabs"));
    }

    #[test]
    fn the_bounds_of_a_job_cover_what_it_reads() {
        let ramp = parse("ramp", json!({ "from": [100.0, 5.0, -40.0], "to": [20.0, 9.0, 60.0], "width": 20.0, "margin": 6.0 })).unwrap();
        assert_eq!(ramp.bounds(), (DVec2::new(4.0, -56.0), DVec2::new(116.0, 76.0)));
        let flat = parse("flatten_rect", json!({ "min": [50.0, 10.0], "max": [-30.0, 90.0], "margin": 12.0 })).unwrap();
        assert_eq!(flat.bounds(), (DVec2::new(-42.0, -2.0), DVec2::new(62.0, 102.0)));
        let dab = parse("sculpt", json!({ "center": [7.0, -3.0], "mode": "raise", "radius": 10.0 })).unwrap();
        assert_eq!(dab.bounds(), (DVec2::new(-3.0, -13.0), DVec2::new(17.0, 7.0)));
        let wet = parse("paint_wetness", json!({ "points": [[-300.0, 40.0], [-260.0, -15.0]], "radius": 25.0 })).unwrap();
        assert_eq!(wet.bounds(), (DVec2::new(-325.0, -40.0), DVec2::new(-235.0, 65.0)));
    }
}
