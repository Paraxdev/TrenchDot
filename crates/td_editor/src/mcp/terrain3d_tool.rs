//! The `terrain3d` MCP tool: link a Terrain3D data folder, look at it, change its regions and settings, and send the
//! edits to Godot.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use td_core::NodeId;
use td_doc::map::NodeKind;
use td_doc::terrain3d::{ChangeKind, Grow, RegionChange, RegionSource, Terrain3DLink};
use td_formats::terrain3d_cache::{self, ManifestMaterial};
use td_geom::terrain3d::{LOC_MAX, LOC_MIN, Loc};

use super::ToolResult;
use super::tools::{require_id, uint};
use crate::app::App;
use crate::state::EditorState;
use crate::terrain3d::bridge::{self, Look, LookChange, Route, Settings};
use crate::terrain3d::inspector::{self, Connection};
use crate::terrain3d::link::{NewFolder, Problem};
use crate::terrain3d::look::look_of;

/// Every op of the tool, the schema text and the unknown op error list them.
pub(crate) const OPS: &[&str] = &["link", "info", "regions", "grow", "settings", "send", "resolve", "probe", "export"];

/// How long `send` waits for Godot when it should, a headless Godot may have to start first.
const SEND_TIMEOUT: Duration = Duration::from_secs(180);

fn loc_list(a: &Value, key: &str) -> Result<Vec<Loc>, String> {
    let list = match &a[key] {
        Value::Null => return Ok(Vec::new()),
        Value::Array(list) => list,
        _ => return Err(format!("{key} must be a list of [x, y] regions")),
    };
    list.iter().map(|v| loc_of(v).map_err(|e| format!("{key}: {e}"))).collect()
}

/// One `[x, y]` region location inside Terrain3D's region map.
fn loc_of(v: &Value) -> Result<Loc, String> {
    let int = |n: &Value| n.as_i64().and_then(|n| i32::try_from(n).ok());
    let pair = v.as_array().filter(|p| p.len() == 2).and_then(|p| Some([int(&p[0])?, int(&p[1])?]));
    let loc = pair.ok_or_else(|| format!("{v} is not an [x, y] region"))?;
    if loc.iter().all(|c| (LOC_MIN..=LOC_MAX).contains(c)) { Ok(loc) } else { Err(format!("region {loc:?} is outside {LOC_MIN} to {LOC_MAX}")) }
}

/// Regions are added blank, which is what Terrain3D's own add does. Filling the new ground from the neighbouring
/// edge comes with the terrain resize.
fn check_fill(a: &Value) -> Result<(), String> {
    match a["fill"].as_str() {
        None | Some("zero") => Ok(()),
        Some(other) => Err(format!("fill {other} is not available, new Terrain3D regions are blank (fill zero)")),
    }
}

fn parse_regions(a: &Value) -> Result<(Vec<Loc>, Vec<Loc>), String> {
    check_fill(a)?;
    let (add, remove) = (loc_list(a, "add")?, loc_list(a, "remove")?);
    if add.is_empty() && remove.is_empty() {
        return Err("add or remove [[x, y], ...] required".into());
    }

    if let Some(both) = add.iter().find(|l| remove.contains(l)) {
        return Err(format!("region {both:?} is in both add and remove"));
    }

    Ok((add, remove))
}

/// Says which asked regions changed nothing, an add on an existing region and a remove of a missing one are skipped.
fn skipped_note(add: &[Loc], remove: &[Loc], added: &[Loc], removed: &[Loc]) -> Option<String> {
    let left = |asked: &[Loc], done: &[Loc]| asked.iter().filter(|l| !done.contains(l)).copied().collect::<Vec<_>>();
    let (exists, missing) = (left(add, added), left(remove, removed));
    let mut parts = Vec::new();
    if !exists.is_empty() {
        parts.push(format!("already have a region: {exists:?}"));
    }

    if !missing.is_empty() {
        parts.push(format!("no region to remove: {missing:?}"));
    }

    (!parts.is_empty()).then(|| format!("skipped, {}", parts.join(", ")))
}

fn parse_grow(a: &Value) -> Result<(Grow, bool), String> {
    check_fill(a)?;
    let side = |key: &str| -> Result<i32, String> {
        match &a[key] {
            Value::Null => Ok(0),
            v => v.as_i64().and_then(|n| i32::try_from(n).ok()).ok_or_else(|| format!("{key} must be a whole number of regions, got {v}")),
        }
    };
    let grow = Grow { north: side("north")?, south: side("south")?, west: side("west")?, east: side("east")? };
    if grow == Grow::default() {
        return Err("north, south, west or east required, a positive number of regions adds, a negative one removes".into());
    }

    Ok((grow, a["dry_run"].as_bool().unwrap_or(false)))
}

#[derive(Debug)]
enum Take {
    Godot,
    Mine,
    Discard,
}

fn parse_resolve(a: &Value) -> Result<(Loc, Take), String> {
    let loc = match &a["region"] {
        Value::Null => return Err("region [x, y] required".into()),
        v => loc_of(v)?,
    };
    let discard = match &a["discard"] {
        Value::Null => false,
        v => v.as_bool().ok_or_else(|| format!("discard must be true or false, got {v}"))?,
    };
    match a["take"].as_str() {
        Some("godot") if discard => Ok((loc, Take::Discard)),
        Some("godot") => Ok((loc, Take::Godot)),
        Some("mine") if discard => Err("discard goes with take godot, take mine sends the edit".into()),
        Some("mine") => Ok((loc, Take::Mine)),
        _ => Err("take must be godot (drop my edit) or mine (send it over Godot's)".into()),
    }
}

/// Why `take` cannot settle `change` on a terrain of `region_size`. Godot refuses an edit made at another region size
/// whatever it is sent over, and a removal made at another size would remove another area, so only dropping either
/// settles the region.
fn resolve_refusal(change: &RegionChange, take: &Take, region_size: u32) -> Option<String> {
    if matches!(take, Take::Godot) && !change.conflict {
        return Some(format!(
            "region {:?} has your unsent {}, which is not a conflict. Send it or take mine to give it to Godot, undo it, or pass discard: true to drop it",
            change.loc,
            kind_name(change.kind)
        ));
    }

    if !change.resized || !matches!(take, Take::Mine) {
        return None;
    }

    let Some(edit) = change.region.as_ref() else {
        return Some(format!(
            "region {:?} was removed at another region size than the {region_size} Godot's terrain uses, so it would remove another area. Take godot to keep Godot's region, or set region_size back with the settings op",
            change.loc
        ));
    };
    Some(format!(
        "region {:?} was made at region size {}, Godot's terrain uses {region_size}. Take godot to drop the edit, or set region_size back to {} with the settings op",
        change.loc, edit.size, edit.size
    ))
}

/// A res:// path of a saved `class` file, empty to clear the node's one, None when `key` is absent.
fn resource_path(a: &Value, key: &str, class: &str) -> Result<Option<String>, String> {
    let bad = |v: &Value| format!("{key} must be the res:// path of a saved {class} (.tres or .res), or \"\" to clear it, got {v}");
    let path = match &a[key] {
        Value::Null => return Ok(None),
        Value::String(path) => path,
        v => return Err(bad(v)),
    };
    let saved = path.strip_prefix("res://").is_some_and(|inner| {
        let lower = inner.to_ascii_lowercase();
        (lower.ends_with(".tres") || lower.ends_with(".res")) && !inner.split(['/', '\\']).any(|part| part.is_empty() || part == "..")
    });
    if path.is_empty() || saved { Ok(Some(path.clone())) } else { Err(bad(&a[key])) }
}

/// The new region size, vertex spacing and look, each None when it stays.
fn parse_settings(a: &Value) -> Result<(Option<u32>, Option<f64>, LookChange), String> {
    let size = match &a["region_size"] {
        Value::Null => None,
        v => Some(uint(v).and_then(|n| u32::try_from(n).ok()).ok_or_else(|| format!("region_size must be a whole number, got {v}"))?),
    };
    let spacing = match &a["vertex_spacing"] {
        Value::Null => None,
        v => Some(v.as_f64().ok_or_else(|| format!("vertex_spacing must be a number of meters, got {v}"))?),
    };
    let ocean_enabled = match &a["ocean_enabled"] {
        Value::Null => None,
        v => Some(v.as_bool().ok_or_else(|| format!("ocean_enabled must be true or false, got {v}"))?),
    };
    let look =
        LookChange { material: resource_path(a, "material", "Terrain3DMaterial")?, assets: resource_path(a, "assets", "Terrain3DAssets")?, ocean_enabled };
    if size.is_none() && spacing.is_none() && look.is_empty() {
        return Err("region_size, vertex_spacing, material, assets or ocean_enabled required".into());
    }

    let check = NewFolder { region_size: size.unwrap_or(256), vertex_spacing: spacing.unwrap_or(1.0), ..Default::default() };
    match check.problem() {
        Some(p @ (Problem::RegionSize | Problem::Spacing)) => Err(p.english().into()),
        _ => Ok((size, spacing, look)),
    }
}

/// The settings reply for what Godot answered. An addon that predates the look takes the grid alone: that is a note
/// when the grid changed, else nothing happened and it is an error.
fn settings_reply(id: NodeId, regrid: bool, asked: &LookChange, before: Option<&Look>, answer: &Settings) -> Result<Value, String> {
    let mut reply = json!({ "id": id.0, "changed": regrid, "region_size": answer.region_size, "vertex_spacing": answer.spacing });
    if answer.route == Route::Headless {
        reply["route"] = json!("headless");
        reply["backup"] = json!(answer.backup);
        reply["problems"] = json!(answer.problems);
    } else {
        reply["route"] = json!("live_link");
    }

    match &answer.look {
        Some(now) => {
            reply["changed"] = json!(regrid || before != Some(now));
            reply["material"] = json!(now.material);
            reply["assets"] = json!(now.assets);
            reply["ocean_enabled"] = json!(now.ocean_enabled);
        }
        None if asked.is_empty() => {}
        None if regrid => reply["note"] = json!(bridge::OLD_ADDON_LOOK),
        None => return Err(bridge::OLD_ADDON_LOOK.into()),
    }

    Ok(reply)
}

/// The world background Godot draws beyond the regions, and whether the cache has the noise values to draw it.
fn background_json(material: &ManifestMaterial) -> Value {
    let mode = match material.world_background {
        0 => "none",
        1 => "flat",
        2 => "noise",
        _ => "unknown",
    };
    json!({ "mode": mode, "noise": material.world_noise.is_some() })
}

/// What the node's clipmap holds on the GPU, null before the scene drew it once. The triangles are those every
/// clipmap put on screen in the last 3D frame.
fn clipmap_json(status: Option<&crate::terrain3d::gpu::Status>, triangles: usize) -> Value {
    status.map_or(Value::Null, |s| {
        json!({
            "fine_layers": s.fine_layers,
            "fine_used": s.fine_used,
            "coarse_regions": s.coarse_regions,
            "gpu_bytes": s.gpu_bytes,
            "texture_loads": s.texture_loads,
            "uploads": { "whole": s.uploads.whole, "partial": s.uploads.partial, "partial_cells": s.uploads.partial_cells },
            "triangles": triangles,
        })
    })
}

/// What `link` creates, None when it links a folder Godot made.
fn parse_link(a: &Value) -> Result<(String, Option<NewFolder>), String> {
    let dir = a["data_directory"].as_str().filter(|d| !d.is_empty()).ok_or("data_directory required, a res:// path like res://terrain_data")?;
    let Some(name) = dir.strip_prefix("res://") else { return Err(format!("data_directory must be a res:// path, got {dir}")) };
    let new = match &a["create"] {
        Value::Null => return Ok((dir.to_string(), None)),
        v @ Value::Object(_) => v,
        _ => return Err("create must be an object like { region_size, vertex_spacing, columns, rows }".into()),
    };
    let defaults = NewFolder::default();
    let size = |key: &str, fallback: u32| match &new[key] {
        Value::Null => Ok(fallback),
        v => uint(v).and_then(|n| u32::try_from(n).ok()).ok_or_else(|| format!("create.{key} must be a whole number, got {v}")),
    };
    let vertex_spacing = match &new["vertex_spacing"] {
        Value::Null => defaults.vertex_spacing,
        v => v.as_f64().ok_or_else(|| format!("create.vertex_spacing must be a number of meters, got {v}"))?,
    };
    let folder = NewFolder {
        name: name.to_string(),
        region_size: size("region_size", defaults.region_size)?,
        vertex_spacing,
        columns: size("columns", defaults.columns)?,
        rows: size("rows", defaults.rows)?,
    };
    match folder.problem() {
        Some(p) => Err(p.english().into()),
        None => Ok((dir.to_string(), Some(folder))),
    }
}

fn kind_name(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Add => "add",
        ChangeKind::Edit => "edit",
        ChangeKind::Remove => "remove",
    }
}

impl EditorState {
    fn terrain3d_cache_path(&self, id: NodeId) -> Result<PathBuf, String> {
        if self.doc.map.terrain3d(id).is_none() {
            return Err(match self.doc.map.nodes.get(&id) {
                Some(_) => format!("node {id} is not a Terrain3D node, link one with the terrain3d tool"),
                None => format!("no node {id}"),
            });
        }

        let (_, cache) = self.terrain3d_links().into_iter().find(|(node, _)| *node == id).ok_or("Open a Godot project first, Terrain3D data lives in one")?;
        Ok(cache)
    }

    /// The cache folder of Terrain3D node `id`, with its manifest looked at again.
    fn terrain3d_cache_of(&mut self, id: NodeId) -> Result<PathBuf, String> {
        let cache = self.terrain3d_cache_path(id)?;
        self.terrain3d.upm = self.game.units_per_meter;
        self.terrain3d.recheck();
        self.terrain3d.refresh(&cache);
        self.terrain3d.finish_scan(&cache, super::terrain3d_tools::SCAN_TIMEOUT)?;
        if self.terrain3d.source(&cache).is_none() {
            self.export_terrain3d_cache(&cache)?;
        }

        Ok(cache)
    }

    fn terrain3d_editable(&self, id: NodeId) -> Result<(), String> {
        if self.doc.map.is_editable(id) { Ok(()) } else { Err(format!("Terrain3D {id} is hidden or locked")) }
    }

    fn terrain3d_regions_json(&self, id: NodeId, cache: &std::path::Path) -> Value {
        let (Some(link), Some(source)) = (self.doc.map.terrain3d(id), self.terrain3d.source(cache)) else { return json!([]) };
        json!(link.locs(&source))
    }

    /// Adds and removes regions as one undo step. Returns what changed, nothing leaves no undo step.
    fn change_terrain3d_regions(&mut self, id: NodeId, add: &[Loc], remove: &[Loc]) -> Result<(Vec<Loc>, Vec<Loc>), String> {
        let cache = self.terrain3d_cache_of(id)?;
        let source = self.terrain3d.source(&cache).ok_or("no Terrain3D cache")?;
        let done = self.doc.try_edit("Terrain3D regions", |m, _| {
            let link = m.terrain3d_mut(id).ok_or_else(|| "the Terrain3D node vanished".to_string())?;
            let removed = link.remove_regions(&source, remove);
            let added = link.add_regions(&source, add)?;
            if added.is_empty() && removed.is_empty() { Err(String::new()) } else { Ok((added, removed)) }
        });
        match done {
            Err(e) if e.is_empty() => Ok((Vec::new(), Vec::new())),
            other => other,
        }
    }
}

impl App {
    pub(crate) fn tool_terrain3d(&mut self, args: &Value) -> ToolResult {
        let op = args["op"].as_str().unwrap_or_default();
        let done = match op {
            "link" => self.terrain3d_link(args),
            "info" => self.terrain3d_info(args),
            "regions" => self.terrain3d_regions(args),
            "grow" => self.terrain3d_grow(args),
            "settings" => self.terrain3d_settings(args),
            "send" => self.terrain3d_send(args),
            "resolve" => self.terrain3d_resolve(args),
            "probe" => self.terrain3d_probe(args),
            "export" => self.terrain3d_export(args),
            other => Err(format!("unknown op {other:?}, use {}", OPS.join(", "))),
        };
        match done {
            Ok(v) => ToolResult::Json(v),
            Err(e) => ToolResult::Error(e),
        }
    }

    fn terrain3d_link(&mut self, args: &Value) -> Result<Value, String> {
        let (dir, create) = parse_link(args)?;
        let id = self.state.link_terrain3d(&dir, create.as_ref())?;
        let mut reply = json!({ "id": id.0, "data_directory": dir, "created": create.is_some() });
        match self.state.terrain3d_cache_of(id) {
            Ok(cache) => reply["regions"] = self.state.terrain3d_regions_json(id, &cache),
            Err(e) => reply["note"] = json!(format!("linked, but {e}")),
        }

        Ok(reply)
    }

    fn terrain3d_info(&mut self, args: &Value) -> Result<Value, String> {
        let id = require_id(args, "id")?;
        let cache = self.state.terrain3d_cache_of(id)?;
        let state = &self.state;
        let (Some(link), Some(source), Some(manifest)) = (state.doc.map.terrain3d(id), state.terrain3d.source(&cache), state.terrain3d.manifest(&cache)) else {
            return Err("no Terrain3D cache".into());
        };
        let grid = source.grid();
        let range = |loc: Loc| manifest.regions.iter().find(|r| r.loc == loc).map(|r| r.height_range);
        let regions: Vec<Value> = link.locs(&source).into_iter().map(|loc| region_json(link, &source, loc, range(loc))).collect();
        let changes = link.changed_regions(&source);
        let found = inspector::changes(&changes, |loc| inspector::unsaved_in_godot(state, &cache, loc));
        let installed = state.game.project_root.as_deref().is_some_and(|root| root.join("addons").join("terrain_3d").is_dir());
        let connection = inspector::connection(
            installed,
            state.restricted(),
            state.godot_has_project(),
            inspector::godot_busy(state),
            state.godot.exe.is_some(),
            &manifest.terrain3d,
        );
        let godot = match connection {
            Connection::Connected(_) => "connected",
            Connection::Busy => "busy",
            Connection::NotRunning => "not_running",
            Connection::NoGodot => "no_godot",
            Connection::NotInstalled => "not_installed",
            Connection::Restricted => "restricted",
        };
        let refused: serde_json::Map<String, Value> =
            state.terrain3d.refused(&cache).into_iter().map(|(l, why)| (format!("{},{}", l[0], l[1]), json!(why))).collect();
        Ok(json!({
            "id": id.0,
            "data_directory": link.data_directory,
            "source": if state.terrain3d.from_files(&cache) { "region_files" } else { "cache" },
            "settings": { "region_size": grid.region_size, "vertex_spacing": grid.spacing },
            "regions": regions,
            "removed": link.removed.keys().collect::<Vec<_>>(),
            "textures": manifest.textures.iter().map(|t| json!({ "id": t.id, "name": t.name })).collect::<Vec<_>>(),
            "changed": changes.iter().map(|c| json!({ "loc": c.loc, "kind": kind_name(c.kind), "conflict": c.conflict })).collect::<Vec<_>>(),
            "conflicts": found.conflicts,
            "resized": found.resized,
            "unsaved_in_godot": found.unsaved,
            "pending": terrain3d_cache::pending_locs(&cache).unwrap_or_default(),
            "refused": refused,
            "godot": godot,
            "versions": { "terrain3d": manifest.terrain3d, "godot": manifest.godot, "data_version": manifest.data_version },
            "resources": manifest.resources,
            "ocean": manifest.ocean,
            "background": background_json(&manifest.material),
            "clipmap": clipmap_json(state.terrain3d_clipmap.get(&id), self.renderer.clipmap_triangles()),
            "error": state.terrain3d.error(&cache),
        }))
    }

    fn terrain3d_regions(&mut self, args: &Value) -> Result<Value, String> {
        let id = require_id(args, "id")?;
        let (add, remove) = parse_regions(args)?;
        self.state.terrain3d_cache_of(id)?;
        self.state.terrain3d_editable(id)?;
        let (added, removed) = self.state.change_terrain3d_regions(id, &add, &remove)?;
        let cache = self.state.terrain3d_cache_of(id)?;
        let mut reply = json!({ "id": id.0, "added": added, "removed": removed, "regions": self.state.terrain3d_regions_json(id, &cache) });
        if let Some(note) = skipped_note(&add, &remove, &added, &removed) {
            reply["note"] = json!(note);
        }

        Ok(reply)
    }

    fn terrain3d_grow(&mut self, args: &Value) -> Result<Value, String> {
        let id = require_id(args, "id")?;
        let (grow, dry_run) = parse_grow(args)?;
        let cache = self.state.terrain3d_cache_of(id)?;
        let grid = self.state.terrain3d.grid(&cache).ok_or("no Terrain3D cache")?;
        let plan = {
            let source = self.state.terrain3d.source(&cache).ok_or("no Terrain3D cache")?;
            self.state.doc.map.terrain3d(id).ok_or("not a Terrain3D node")?.plan_grow(&source, grow)?
        };
        if !dry_run {
            self.state.terrain3d_editable(id)?;
            self.state.change_terrain3d_regions(id, &plan.add, &plan.remove)?;
        }

        let size = inspector::footprint(&plan, &grid);
        Ok(json!({
            "id": id.0,
            "dry_run": dry_run,
            "min": plan.min,
            "max": plan.max,
            "add": plan.add,
            "remove": plan.remove,
            "columns": size.columns,
            "rows": size.rows,
            "width_m": size.width,
            "depth_m": size.depth,
            "regions": self.state.terrain3d_regions_json(id, &cache),
        }))
    }

    fn terrain3d_settings(&mut self, args: &Value) -> Result<Value, String> {
        let id = require_id(args, "id")?;
        let (size, spacing, look) = parse_settings(args)?;
        let cache = self.state.terrain3d_cache_of(id)?;
        let grid = self.state.terrain3d.grid(&cache).ok_or("no Terrain3D cache")?;
        let (size, spacing) = (size.unwrap_or(grid.region_size), spacing.unwrap_or(grid.spacing));
        let regrid = inspector::settings_differ(&grid, size, spacing);
        if !regrid && look.is_empty() {
            return Ok(json!({ "id": id.0, "changed": false, "region_size": size, "vertex_spacing": spacing }));
        }

        if let Some(refusal) = inspector::region_size_refusal(&self.state, id, &cache, size) {
            return Err(refusal);
        }

        let before = self.state.terrain3d.manifest(&cache).and_then(look_of);
        let request = self.state.terrain3d_settings_request(id, &cache, size, spacing, look).map_err(|why| why.en)?;
        let answer = bridge::settings(&request)?;
        self.state.terrain3d.recheck();
        let reply = settings_reply(id, regrid, &request.look, before.as_ref(), &answer)?;
        self.state.set_status(crate::terrain3d::settings_status(&answer));
        Ok(reply)
    }

    fn terrain3d_send(&mut self, args: &Value) -> Result<Value, String> {
        let id = require_id(args, "id")?;
        let wait = args["wait"].as_bool().unwrap_or(true);
        let cache = self.state.terrain3d_cache_of(id)?;
        self.state.send_terrain3d(&BTreeSet::new());
        if let Some(reply) = unsent_reply(&self.state, id, &cache) {
            return Ok(reply);
        }

        let pending = || terrain3d_cache::pending_locs(&cache).unwrap_or_default();

        if !wait {
            return Ok(json!({ "id": id.0, "sent": true, "done": false, "pending": pending() }));
        }

        let Some(outcome) = bridge::wait(&mut self.state.terrain3d_bridge, SEND_TIMEOUT) else {
            return Ok(json!({ "id": id.0, "sent": true, "done": false, "pending": pending() }));
        };
        if outcome.unanswered {
            self.state.keep_unanswered_terrain3d(&outcome.folder);
        }

        if outcome.is_ok() {
            self.state.set_status(outcome.status());
        } else {
            self.state.fail_status(outcome.status());
        }

        self.state.terrain3d.recheck();
        if !outcome.is_ok() {
            return Err(format!("Terrain3D edits were not applied: {}", outcome.problems.join("; ")));
        }

        let route = match outcome.route {
            Route::LiveLink => "live_link",
            Route::Headless => "headless",
            Route::Waiting => "waiting",
        };
        Ok(json!({
            "id": id.0,
            "sent": true,
            "done": true,
            "route": route,
            "accepted": outcome.accepted,
            "refused": outcome.refused.iter().map(|(loc, reason)| json!({ "loc": loc, "reason": reason })).collect::<Vec<_>>(),
            "unsaved_in_godot": outcome.unsaved_in_godot(),
            "old_addon": outcome.old_addon,
            "pending": pending(),
        }))
    }

    fn terrain3d_export(&mut self, args: &Value) -> Result<Value, String> {
        let id = require_id(args, "id")?;
        let cache = self.state.terrain3d_cache_path(id)?;
        self.state.terrain3d.upm = self.state.game.units_per_meter;
        let written = self.state.export_terrain3d_cache(&cache).map_err(|e| format!("Terrain3D cache was not exported: {e}"))?;
        Ok(json!({ "id": id.0, "written": written, "regions": self.state.terrain3d_regions_json(id, &cache) }))
    }

    fn terrain3d_resolve(&mut self, args: &Value) -> Result<Value, String> {
        let id = require_id(args, "id")?;
        let (loc, take) = parse_resolve(args)?;
        let cache = self.state.terrain3d_cache_of(id)?;
        let (change, region_size) = {
            let source = self.state.terrain3d.source(&cache).ok_or("no Terrain3D cache")?;
            let link = self.state.doc.map.terrain3d(id).ok_or("not a Terrain3D node")?;
            (link.changed_regions(&source).into_iter().find(|c| c.loc == loc), source.grid().region_size)
        };
        let Some(change) = change else { return Err(format!("region {loc:?} has no edit of yours that waits for Godot")) };
        if let Some(refusal) = resolve_refusal(&change, &take, region_size) {
            return Err(refusal);
        }

        match take {
            Take::Godot | Take::Discard => {
                self.state.terrain3d_editable(id)?;
                self.state.use_godots_terrain3d(id, loc);
            }
            Take::Mine => self.state.send_terrain3d(&BTreeSet::from([loc])),
        }

        let taken = match take {
            Take::Godot | Take::Discard => "godot",
            Take::Mine => "mine",
        };
        Ok(json!({ "id": id.0, "region": loc, "take": taken, "was_conflict": change.conflict, "kind": kind_name(change.kind) }))
    }

    fn terrain3d_probe(&mut self, args: &Value) -> Result<Value, String> {
        let id = require_id(args, "id")?;
        let points = args["points"].as_array().filter(|p| !p.is_empty()).ok_or("points [[x, z], ...] required")?;
        let points: Vec<(f64, f64)> = points
            .iter()
            .map(|p| match p.as_array().map(Vec::as_slice) {
                Some([x, z]) => Ok((x.as_f64().ok_or("probe points are [x, z] numbers")?, z.as_f64().ok_or("probe points are [x, z] numbers")?)),
                _ => Err(format!("{p} is not an [x, z] point")),
            })
            .collect::<Result<_, String>>()?;
        self.state.terrain3d_cache_of(id)?;
        let probed = points.iter().map(|(x, z)| self.state.probe_terrain3d(id, *x, *z)).collect::<Result<Vec<_>, _>>()?;
        Ok(json!({ "id": id.0, "points": probed }))
    }
}

/// How many regions the Terrain3D node `id` has, for list_nodes.
pub(crate) fn region_count(state: &EditorState, id: NodeId) -> Option<usize> {
    if !matches!(state.doc.map.nodes.get(&id)?.kind, NodeKind::Terrain3D(_)) {
        return None;
    }

    let (_, cache) = state.terrain3d_links().into_iter().find(|(node, _)| *node == id)?;
    inspector::region_count(state, id, &cache)
}

/// The reply of a send that left nothing to wait for. None while a send runs or waits behind an export.
pub(crate) fn unsent_reply(state: &EditorState, id: NodeId, cache: &Path) -> Option<Value> {
    if state.terrain3d_bridge.sending() {
        return None;
    }

    let reason = if state.restricted() { "restricted mode, trust the project in the editor to let it reach Godot" } else { "no Terrain3D edits to send" };
    Some(json!({ "id": id.0, "sent": false, "route": "waiting", "reason": reason, "pending": terrain3d_cache::pending_locs(cache).unwrap_or_default() }))
}

/// One region of info. An edit Godot applied is Godot's region again, so it reads as unedited with Godot's height range.
fn region_json(link: &Terrain3DLink, source: &dyn RegionSource, loc: Loc, height_range: Option<[f32; 2]>) -> Value {
    let edited = link.live_edit(source, loc).filter(|e| !e.applied());
    json!({
        "loc": loc,
        "edited": edited.is_some(),
        "added": edited.is_some_and(|e| e.base_hash.is_none()),
        "height_range": if edited.is_some() { None } else { height_range },
    })
}

#[cfg(test)]
mod tests {
    use td_formats::terrain3d_cache::Manifest;

    use super::*;

    #[test]
    fn regions_need_an_add_or_a_remove_inside_the_region_map() {
        let parse = |v: Value| parse_regions(&v);
        assert_eq!(parse(json!({ "add": [[-3, 2]], "remove": [[4, -1], [0, 0]] })), Ok((vec![[-3, 2]], vec![[4, -1], [0, 0]])));
        assert_eq!(parse(json!({ "add": [[15, -16]] })), Ok((vec![[15, -16]], vec![])));
        assert!(parse(json!({})).unwrap_err().contains("add or remove"));
        assert!(parse(json!({ "add": [] })).is_err(), "an empty list is nothing to do");
        assert!(parse(json!({ "add": [[16, 0]] })).unwrap_err().contains("outside"));
        assert!(parse(json!({ "remove": [[0, -17]] })).unwrap_err().contains("outside"));
        assert!(parse(json!({ "add": [[1, 2, 3]] })).is_err());
        assert!(parse(json!({ "add": [[1.5, 2]] })).is_err());
        assert!(parse(json!({ "add": "0,0" })).unwrap_err().contains("list"));
        assert!(parse(json!({ "add": [[2, 3]], "remove": [[2, 3]] })).unwrap_err().contains("both"));
    }

    #[test]
    fn a_skipped_region_is_named_in_the_reply() {
        assert_eq!(skipped_note(&[[1, 2]], &[[-3, 4]], &[[1, 2]], &[[-3, 4]]), None);
        assert_eq!(skipped_note(&[], &[], &[], &[]), None);
        let note = skipped_note(&[[1, 2], [5, -6]], &[[-3, 4], [0, 9]], &[[5, -6]], &[[0, 9]]).unwrap();
        assert!(note.contains("already have a region: [[1, 2]]") && note.contains("no region to remove: [[-3, 4]]"), "{note}");
        assert!(!note.contains("[5, -6]") && !note.contains("[0, 9]"), "{note}");
        let only_add = skipped_note(&[[7, 7]], &[], &[], &[]).unwrap();
        assert!(only_add.contains("[[7, 7]]") && !only_add.contains("remove"), "{only_add}");
    }

    #[test]
    fn only_a_blank_fill_is_offered() {
        assert!(parse_regions(&json!({ "add": [[0, 0]], "fill": "zero" })).is_ok());
        assert!(parse_regions(&json!({ "add": [[0, 0]], "fill": "edge" })).unwrap_err().contains("edge"));
        assert!(parse_grow(&json!({ "east": 1, "fill": "edge" })).is_err());
    }

    #[test]
    fn grow_takes_signed_sides_and_refuses_none() {
        let (grow, dry) = parse_grow(&json!({ "north": 2, "south": -1, "west": 0, "east": 3 })).unwrap();
        assert_eq!(grow, Grow { north: 2, south: -1, west: 0, east: 3 });
        assert!(!dry);
        assert!(parse_grow(&json!({ "west": -1, "dry_run": true })).unwrap().1);
        assert_eq!(parse_grow(&json!({ "east": 1 })).unwrap().0, Grow { east: 1, ..Default::default() });
        assert!(parse_grow(&json!({})).is_err());
        assert!(parse_grow(&json!({ "north": 0 })).is_err());
        assert!(parse_grow(&json!({ "north": 1.5 })).unwrap_err().contains("north"));
        assert!(parse_grow(&json!({ "south": "a" })).is_err());
    }

    #[test]
    fn resolve_needs_a_region_and_a_side() {
        assert_eq!(parse_resolve(&json!({ "region": [-2, 5], "take": "godot" })).map(|(l, t)| (l, matches!(t, Take::Godot))), Ok(([-2, 5], true)));
        assert_eq!(parse_resolve(&json!({ "region": [0, 1], "take": "mine" })).map(|(l, t)| (l, matches!(t, Take::Mine))), Ok(([0, 1], true)));
        assert!(parse_resolve(&json!({ "take": "mine" })).unwrap_err().contains("region"));
        assert!(parse_resolve(&json!({ "region": [0, 1] })).unwrap_err().contains("take"));
        assert!(parse_resolve(&json!({ "region": [0, 1], "take": "both" })).is_err());
        assert!(parse_resolve(&json!({ "region": [16, 1], "take": "mine" })).is_err());
        let discard = |take: &str, discard: Value| parse_resolve(&json!({ "region": [3, -7], "take": take, "discard": discard }));
        assert!(matches!(discard("godot", json!(true)), Ok(([3, -7], Take::Discard))));
        assert!(matches!(discard("godot", json!(false)), Ok(([3, -7], Take::Godot))));
        assert!(matches!(discard("mine", json!(false)), Ok(([3, -7], Take::Mine))));
        assert!(discard("mine", json!(true)).unwrap_err().contains("discard goes with take godot"));
        assert!(discard("godot", json!("yes")).unwrap_err().contains("discard must be true or false, got \"yes\""));
    }

    #[test]
    fn a_region_made_at_another_region_size_can_only_be_dropped() {
        let edit = |loc: Loc, size: u32, resized: bool| RegionChange {
            loc,
            kind: ChangeKind::Edit,
            region: Some(std::sync::Arc::new(td_geom::terrain3d::Region3D::blank(loc, size))),
            base_hash: None,
            conflict: true,
            resized,
        };
        let refusal = resolve_refusal(&edit([-3, 7], 256, true), &Take::Mine, 128).unwrap();
        for fact in ["[-3, 7]", "region size 256", "uses 128", "take godot", "region_size back to 256"] {
            assert!(refusal.to_lowercase().contains(fact), "{fact} missing from {refusal}");
        }

        assert_eq!(resolve_refusal(&edit([-3, 7], 256, true), &Take::Godot, 128), None);
        assert_eq!(resolve_refusal(&edit([5, -2], 128, false), &Take::Mine, 128), None);
        let removal = |resized| RegionChange { region: None, kind: ChangeKind::Remove, ..edit([-6, 2], 64, resized) };
        assert_eq!(resolve_refusal(&removal(false), &Take::Mine, 128), None);
        assert_eq!(resolve_refusal(&removal(true), &Take::Godot, 128), None);
        let refusal = resolve_refusal(&removal(true), &Take::Mine, 128).unwrap();
        for fact in ["[-6, 2]", "removed at another region size", "the 128", "take godot"] {
            assert!(refusal.to_lowercase().contains(fact), "{fact} missing from {refusal}");
        }
    }

    #[test]
    fn take_godot_settles_only_a_conflict() {
        let change = |kind: ChangeKind, conflict: bool| RegionChange {
            loc: [-4, 9],
            kind,
            region: (kind != ChangeKind::Remove).then(|| std::sync::Arc::new(td_geom::terrain3d::Region3D::blank([-4, 9], 64))),
            base_hash: None,
            conflict,
            resized: false,
        };
        for kind in [ChangeKind::Add, ChangeKind::Edit, ChangeKind::Remove] {
            let refusal = resolve_refusal(&change(kind, false), &Take::Godot, 64).unwrap_or_default();
            for fact in ["[-4, 9]".to_string(), format!("unsent {}", kind_name(kind)), "not a conflict".into(), "discard: true".into()] {
                assert!(refusal.contains(&fact), "{fact} missing from {refusal:?}");
            }

            assert_eq!(resolve_refusal(&change(kind, true), &Take::Godot, 64), None, "{kind:?} conflict");
            assert_eq!(resolve_refusal(&change(kind, false), &Take::Mine, 64), None, "{kind:?} sent as mine");
            assert_eq!(resolve_refusal(&change(kind, false), &Take::Discard, 64), None, "{kind:?} discarded");
        }
    }

    #[test]
    fn settings_check_each_value_at_its_edge() {
        let grid = |a: Value| parse_settings(&a).map(|(size, spacing, look)| (size, spacing, look.is_empty()));
        assert_eq!(grid(json!({ "region_size": 64 })), Ok((Some(64), None, true)));
        assert_eq!(grid(json!({ "vertex_spacing": 0.25 })), Ok((None, Some(0.25), true)));
        assert_eq!(grid(json!({ "region_size": 2048, "vertex_spacing": 100.0 })), Ok((Some(2048), Some(100.0), true)));
        assert!(parse_settings(&json!({})).unwrap_err().contains("material, assets or ocean_enabled required"));
        assert!(parse_settings(&json!({ "region_size": 100 })).unwrap_err().contains("64"));
        assert!(parse_settings(&json!({ "region_size": 4096 })).is_err());
        assert!(parse_settings(&json!({ "vertex_spacing": 0.2 })).unwrap_err().contains("spacing"));
        assert!(parse_settings(&json!({ "vertex_spacing": 100.5 })).is_err());
        assert!(parse_settings(&json!({ "vertex_spacing": "wide" })).is_err());
    }

    #[test]
    fn settings_pick_saved_resources_and_refuse_other_paths() {
        let look = |a: Value| parse_settings(&a).map(|(_, _, look)| look);
        let material = "res://terrain/look/M_terrain.tres";
        assert_eq!(look(json!({ "material": material })), Ok(LookChange { material: Some(material.into()), ..Default::default() }));
        assert_eq!(
            parse_settings(&json!({ "assets": "res://a.RES", "material": "", "ocean_enabled": false, "region_size": 512 })),
            Ok((Some(512), None, LookChange { material: Some(String::new()), assets: Some("res://a.RES".into()), ocean_enabled: Some(false) }))
        );
        assert_eq!(look(json!({ "ocean_enabled": true })), Ok(LookChange { ocean_enabled: Some(true), ..Default::default() }));
        let bad = [
            ("material", json!("C:/proj/terrain/m.tres")),
            ("material", json!("terrain/m.tres")),
            ("material", json!("user://m.tres")),
            ("material", json!("res://")),
            ("material", json!("res://terrain/../../m.tres")),
            ("material", json!("res://terrain//m.tres")),
            ("material", json!("res://terrain\\..\\..\\m.tres")),
            ("material", json!("res://terrain/m.gdshader")),
            ("assets", json!("res://terrain/")),
            ("assets", json!(7)),
            ("assets", json!(["res://a.tres"])),
        ];
        for (key, path) in bad {
            let refused = look(json!({ key: path.clone() })).unwrap_err();
            assert!(refused.starts_with(key) && refused.contains("res://"), "{key} {path}: {refused}");
        }

        assert!(look(json!({ "ocean_enabled": "yes" })).unwrap_err().contains("true or false"));
        assert!(look(json!({ "material": material, "region_size": 100 })).is_err(), "a bad grid value refuses the whole change");
    }

    #[test]
    fn the_settings_reply_says_what_godot_has_and_when_an_old_addon_left_the_look() {
        let id = NodeId(7);
        let look = Look { material: "res://t/m.tres".into(), assets: String::new(), ocean_enabled: true };
        let asked = LookChange { material: Some("res://t/m.tres".into()), ocean_enabled: Some(true), ..Default::default() };
        let answer = Settings { region_size: 128, spacing: 0.5, look: Some(look.clone()), route: Route::LiveLink, ..Default::default() };
        assert_eq!(
            settings_reply(id, false, &asked, None, &answer),
            Ok(
                json!({ "id": 7, "changed": true, "region_size": 128, "vertex_spacing": 0.5, "route": "live_link", "material": "res://t/m.tres", "assets": "", "ocean_enabled": true })
            )
        );
        let changed = |regrid, before: &Look| settings_reply(id, regrid, &asked, Some(before), &answer).unwrap()["changed"].clone();
        assert_eq!(changed(false, &look), json!(false), "Godot had that look already");
        assert_eq!(changed(true, &look), json!(true));
        assert_eq!(changed(false, &Look { ocean_enabled: false, ..look.clone() }), json!(true));

        let old = Settings { region_size: 128, spacing: 0.5, route: Route::LiveLink, ..Default::default() };
        assert_eq!(settings_reply(id, false, &asked, Some(&look), &old), Err(bridge::OLD_ADDON_LOOK.to_string()));
        assert_eq!(
            settings_reply(id, true, &asked, None, &old),
            Ok(json!({ "id": 7, "changed": true, "region_size": 128, "vertex_spacing": 0.5, "route": "live_link", "note": bridge::OLD_ADDON_LOOK }))
        );
        assert_eq!(
            settings_reply(id, true, &LookChange::default(), None, &old),
            Ok(json!({ "id": 7, "changed": true, "region_size": 128, "vertex_spacing": 0.5, "route": "live_link" }))
        );

        let backup = "C:/game/terrain/.trenchdot/backups/20261119T101500";
        let headless = Settings {
            region_size: 64,
            spacing: 2.0,
            route: Route::Headless,
            backup: backup.into(),
            regions: 4,
            problems: vec!["slow".into()],
            ..Default::default()
        };
        assert_eq!(
            settings_reply(id, true, &LookChange::default(), None, &headless),
            Ok(json!({ "id": 7, "changed": true, "region_size": 64, "vertex_spacing": 2.0, "route": "headless", "backup": backup, "problems": ["slow"] }))
        );
    }

    #[test]
    fn info_reads_the_look_and_the_background_from_the_cache() {
        let manifest = |extra: Value| {
            let mut m = json!({ "format": terrain3d_cache::FORMAT, "version": 1, "region_size": 256, "vertex_spacing": 1.0 });
            m.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            serde_json::from_value::<Manifest>(m).unwrap()
        };
        let old = manifest(json!({}));
        assert_eq!((look_of(&old), background_json(&old.material)), (None, json!({ "mode": "none", "noise": false })));

        let resources = json!({ "material": "res://data/M_terrain.tres", "assets": "", "ocean_material": "res://ocean.tres" });
        let without_ocean = manifest(json!({ "resources": resources, "material": { "world_background": 1 } }));
        let want = Look { material: "res://data/M_terrain.tres".into(), assets: String::new(), ocean_enabled: false };
        assert_eq!(look_of(&without_ocean), Some(want.clone()));
        assert_eq!(background_json(&without_ocean.material), json!({ "mode": "flat", "noise": false }));

        let noise = json!({ "world_background": 2, "world_noise": { "height": 64.0 } });
        let full = manifest(json!({ "resources": resources, "ocean": { "enabled": true, "sea_level": 5.0 }, "material": noise }));
        assert_eq!(look_of(&full), Some(Look { ocean_enabled: true, ..want }));
        assert_eq!(background_json(&full.material), json!({ "mode": "noise", "noise": true }));
        let flat_noise = manifest(json!({ "material": { "world_background": 2 } }));
        assert_eq!(background_json(&flat_noise.material), json!({ "mode": "noise", "noise": false }), "an old cache has no noise values");
        assert_eq!(background_json(&manifest(json!({ "material": { "world_background": 5 } })).material)["mode"], "unknown");
    }

    #[test]
    fn link_names_a_res_folder_and_checks_what_it_creates() {
        assert_eq!(parse_link(&json!({ "data_directory": "res://world" })), Ok(("res://world".into(), None)));
        let (dir, new) =
            parse_link(&json!({ "data_directory": "res://my_world", "create": { "region_size": 128, "vertex_spacing": 0.5, "columns": 3, "rows": 1 } }))
                .unwrap();
        assert_eq!(dir, "res://my_world");
        assert_eq!(new, Some(NewFolder { name: "my_world".into(), region_size: 128, vertex_spacing: 0.5, columns: 3, rows: 1 }));
        let defaults = parse_link(&json!({ "data_directory": "res://w", "create": {} })).unwrap().1.unwrap();
        assert_eq!(defaults, NewFolder { name: "w".into(), ..Default::default() });
        assert!(parse_link(&json!({})).is_err());
        assert!(parse_link(&json!({ "data_directory": "world" })).unwrap_err().contains("res://"));
        assert!(parse_link(&json!({ "data_directory": "res://" , "create": {} })).is_err(), "no folder name");
        assert!(parse_link(&json!({ "data_directory": "res://a/b", "create": {} })).is_err(), "a nested folder");
        assert!(parse_link(&json!({ "data_directory": "res://w", "create": { "region_size": 100 } })).is_err());
        assert!(parse_link(&json!({ "data_directory": "res://w", "create": { "columns": 33 } })).is_err());
        assert!(parse_link(&json!({ "data_directory": "res://w", "create": { "columns": 32, "rows": 1 } })).is_ok());
        assert!(parse_link(&json!({ "data_directory": "res://w", "create": { "rows": 0 } })).is_err());
        assert!(parse_link(&json!({ "data_directory": "res://w", "create": "yes" })).is_err());
    }

    #[test]
    fn info_shows_an_edit_godot_applied_as_godots_region() {
        use std::sync::Arc;
        use td_doc::terrain3d::RegionEdit;

        let dir = tempfile::tempdir().unwrap();
        let (state, _, cache) = crate::terrain3d::brush::tests::state_over(dir.path(), &[[1, -1]]);
        let source = state.terrain3d.source(&cache).unwrap();
        let mut link = Terrain3DLink::new("res://t3d");
        let entry = |link: &Terrain3DLink| region_json(link, &source, [1, -1], Some([2.0, 5.0]));
        let mine = Arc::new((*state.terrain3d.region(&cache, [1, -1]).unwrap()).clone());
        link.edits.insert([1, -1], RegionEdit::new(mine.clone(), source.hash([1, -1])));
        assert_eq!(entry(&link), json!({ "loc": [1, -1], "edited": true, "added": false, "height_range": null }));

        link.edits.insert([1, -1], RegionEdit { region: mine.clone(), base_hash: source.hash([1, -1]), base: Some(mine.clone()) });
        assert_eq!(entry(&link), json!({ "loc": [1, -1], "edited": false, "added": false, "height_range": [2.0, 5.0] }));

        link.edits.insert([1, -1], RegionEdit { region: mine.clone(), base_hash: None, base: Some(mine) });
        assert_eq!(entry(&link)["edited"], json!(false), "Godot changed the region since it applied the edit");
    }

    #[test]
    fn a_region_size_past_32_regions_per_side_is_refused() {
        use td_formats::terrain3d_cache;
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = crate::terrain3d::brush::tests::state_over(dir.path(), &[[1, -1], [2, -1]]);
        let mut manifest = terrain3d_cache::read_manifest(&cache).unwrap();
        manifest.region_size = 256;
        terrain3d_cache::write_manifest(&cache, &manifest).unwrap();
        state.terrain3d.recheck();
        state.poll_terrain3d();
        let source = state.terrain3d.source(&cache).unwrap();
        state.doc.edit("Add", |m, _| m.terrain3d_mut(id).unwrap().add_regions(&source, &[[-8, -8]]).unwrap());
        assert_eq!(inspector::region_size_refusal(&state, id, &cache, 128), None);
        assert_eq!(inspector::region_size_refusal(&state, id, &cache, 2048), None);
        let refusal = inspector::region_size_refusal(&state, id, &cache, 64).unwrap();
        assert!(refusal.contains("at most 32 regions") && refusal.contains("[-32, -32] to [11, -1]"), "{refusal}");
    }

    #[test]
    fn the_region_count_holds_regions_not_in_godot_yet() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = crate::terrain3d::brush::tests::state_over(dir.path(), &[[1, -1]]);
        assert_eq!(inspector::region_count(&state, id, &cache), Some(1));
        let source = state.terrain3d.source(&cache).unwrap();
        state.doc.edit("Add", |m, _| m.terrain3d_mut(id).unwrap().add_regions(&source, &[[2, -1], [3, -1]]).unwrap());
        assert_eq!(inspector::region_count(&state, id, &cache), Some(3));
    }

    #[test]
    fn the_op_list_has_every_op_once() {
        let set: BTreeSet<_> = OPS.iter().collect();
        assert_eq!(set.len(), OPS.len());
        assert_eq!(OPS.len(), 9);
    }
}
