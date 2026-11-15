//! MCP tools for the Terrain3D node: `terrain_edit` on a Terrain3D id.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use td_core::{DVec2, DVec3, NodeId};
use td_doc::terrain::{SculptBrush, SculptMode};
use td_doc::terrain3d::{RegionSource, reground_scatter};
use td_geom::terrain3d::Loc;
use td_geom::terrain3d_tile::PaintMode;

use super::ToolResult;
use super::content_tools::{point, resample};
use super::tools::uint;
use crate::app::App;
use crate::state::EditorState;

/// How long a tool waits for the regions it edits to load from the cache.
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

fn err(msg: impl Into<String>) -> ToolResult {
    ToolResult::Error(msg.into())
}

fn ok(v: Value) -> ToolResult {
    ToolResult::Json(v)
}

fn xz(p: DVec3) -> DVec2 {
    DVec2::new(p.x, p.z)
}

/// One terrain_edit op on a Terrain3D node, parsed before anything loads or changes.
#[derive(Debug)]
pub(crate) enum Job {
    Dabs { brush: SculptBrush, points: Vec<DVec3> },
    Paint { texture: u8, mode: PaintMode, strength: f32, radius: f64, points: Vec<DVec3> },
    Flatten { min: DVec2, max: DVec2, height: f64, margin: f64 },
    Ramp { from: DVec3, to: DVec3, width: f64, margin: f64 },
}

impl Job {
    pub(crate) fn parse(op: &str, a: &Value) -> Result<Job, String> {
        let radius = a["radius"].as_f64().unwrap_or(256.0);
        let strength = a["strength"].as_f64().unwrap_or(16.0);
        let center = point(&a["center"]);
        let one = || center.map(|c| vec![c]).ok_or_else(|| "center [x, z] required".to_string());
        let many = |spacing: f64| {
            let points: Vec<DVec3> = a["points"].as_array().into_iter().flatten().filter_map(point).collect();
            if points.is_empty() { Err("points [[x, z], ...] required".to_string()) } else { Ok(resample(&points, spacing)) }
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
            "paint_texture" => {
                let texture = uint(&a["texture"]).filter(|t| *t < 32).ok_or("texture, a Terrain3D texture id from 0 to 31, is required")? as u8;
                let mode = match a["mode"].as_str().unwrap_or("base") {
                    "base" => PaintMode::Base,
                    "spray" => PaintMode::Spray,
                    _ => return Err("mode must be base or spray".into()),
                };
                let points = if center.is_some() { one()? } else { many(radius * 0.4)? };
                Ok(Job::Paint { texture, mode, strength: a["strength"].as_f64().unwrap_or(1.0).clamp(0.0, 1.0) as f32, radius, points })
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
                "terrain_edit op {other} does not apply to Terrain3D, use sculpt, sculpt_path, paint_texture, flatten_rect, ramp, holes, or only probe"
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
        if self.terrain3d.grid(&cache).is_none() {
            self.export_terrain3d_cache(&cache)?;
        }

        let grid = self.terrain3d.grid(&cache).ok_or(crate::terrain3d::bridge::NOT_EXPORTED)?;
        let (lo, hi) = grid.vgrid_rect(min, max, 2);
        let need: Vec<Loc> = {
            let source = self.terrain3d.source(&cache).ok_or("no Terrain3D cache")?;
            grid.regions_in(lo, hi).into_iter().filter(|l| source.hash(*l).is_some()).collect()
        };
        self.terrain3d.want(&cache, &need, &[]);
        let start = Instant::now();
        while need.iter().any(|l| self.terrain3d.region(&cache, *l).is_none()) {
            if start.elapsed() > LOAD_TIMEOUT {
                let why = self.terrain3d.error(&cache).map(|e| format!(": {e}")).unwrap_or_default();
                return Err(format!("the Terrain3D regions did not load in {} s{why}", LOAD_TIMEOUT.as_secs()));
            }

            std::thread::sleep(Duration::from_millis(5));
            self.terrain3d.refresh(&cache);
        }

        Ok(())
    }

    /// Runs `job` on the regions of node `id` as one undo step. Height jobs put the scatter that follows the node back on
    /// the new ground in the same step. Returns the regions changed, none leaves no undo step.
    pub(crate) fn run_terrain3d_job(&mut self, id: NodeId, job: &Job, label: &str, seed: u32) -> Result<Vec<Loc>, String> {
        let (min, max) = job.bounds();
        self.load_terrain3d_rect(id, min, max)?;
        let (_, cache) = self.terrain3d_links().into_iter().find(|(node, _)| *node == id).ok_or("no Terrain3D cache")?;
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
                Job::Paint { texture, mode, strength, radius, points } => {
                    for p in points {
                        locs.extend(link.paint_texture(&source, *p, *radius, *texture, *mode, *strength));
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
            Ok(locs) => Ok(locs),
            Err(e) if e.is_empty() => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// The surface height and texture ids at `x`, `z` (map units) as JSON, after loading what they read.
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
            Ok(locs) => {
                let mut out = json!({ "id": id.0, "changed": !locs.is_empty(), "regions": locs });
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
        let Ok(Job::Paint { texture, mode, strength, .. }) = base(json!({ "texture": 31, "mode": "spray", "strength": 4.0 })) else { panic!("parses") };
        assert_eq!((texture, mode, strength), (31, PaintMode::Spray, 1.0));
        let Ok(Job::Paint { texture, mode, .. }) = base(json!({ "texture": 0 })) else { panic!("parses") };
        assert_eq!((texture, mode), (0, PaintMode::Base));
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
    fn the_bounds_of_a_job_cover_what_it_reads() {
        let ramp = parse("ramp", json!({ "from": [100.0, 5.0, -40.0], "to": [20.0, 9.0, 60.0], "width": 20.0, "margin": 6.0 })).unwrap();
        assert_eq!(ramp.bounds(), (DVec2::new(4.0, -56.0), DVec2::new(116.0, 76.0)));
        let flat = parse("flatten_rect", json!({ "min": [50.0, 10.0], "max": [-30.0, 90.0], "margin": 12.0 })).unwrap();
        assert_eq!(flat.bounds(), (DVec2::new(-42.0, -2.0), DVec2::new(62.0, 102.0)));
        let dab = parse("sculpt", json!({ "center": [7.0, -3.0], "mode": "raise", "radius": 10.0 })).unwrap();
        assert_eq!(dab.bounds(), (DVec2::new(-3.0, -13.0), DVec2::new(17.0, 7.0)));
    }
}
