//! Sending TrenchDot's Terrain3D edits to Godot. A save writes the edited regions as pending files in the data folder's
//! cache, then Godot applies them: the open editor over the live link as one undo step, or a headless Godot when the
//! editor is not running. Without either the files wait for the next time Godot opens the scene.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use td_doc::terrain3d::{RegionChange, RegionEdit, Removal, Terrain3DLink};
use td_formats::terrain3d_cache::{self, CacheError, PendingRegion, RegionHash};
use td_geom::terrain3d::{Grid, Loc, Region3D};

use crate::i18n::{Msg, en, msg, msg_n};
use crate::live_link;

const CLI_SCRIPT: &str = "res://addons/func_godot/src/trenchdot/terrain3d_cli.gd";
const LIVE_REPLY: Duration = Duration::from_secs(60);
const HEADLESS_TIMEOUT: Duration = Duration::from_secs(120);
const OLD_ADDON: &str = "unknown event";
const NO_SCENE: &str = "no open scene shows the Terrain3D data in ";
/// The reason Godot refuses a region whose scene has unsaved changes there.
pub const DIRTY: &str = "dirty";
/// The refusal reason for an edit made at another region size than the terrain in Godot has.
pub const SIZE: &str = "size";

/// Writes `changes` as the pending files of the cache at `cache`, replacing what an earlier save left for the same
/// regions. `forced` regions are applied even when they changed in Godot meanwhile. Returns the regions written.
pub fn write_pending(cache: &Path, changes: &[RegionChange], grid: &Grid, forced: &BTreeSet<Loc>) -> Result<Vec<Loc>, CacheError> {
    let mut written = Vec::new();
    for change in changes {
        let remove = change.region.is_none();
        let region = change.region.as_ref().map_or_else(|| Region3D::blank(change.loc, grid.region_size), |r| (**r).clone());
        let pending = PendingRegion {
            region,
            vertex_spacing: grid.spacing as f32,
            base: change.base_hash.map_or(RegionHash::ZERO, RegionHash),
            force: forced.contains(&change.loc),
            remove,
        };
        terrain3d_cache::write_pending(cache, &pending)?;
        written.push(change.loc);
    }

    Ok(written)
}

/// Deletes the pending files of regions that no longer differ, the ones an undo took back after the save: regions the
/// link has an opinion about, and the regions in `ours`, whose files this editor wrote. Other files belong to another
/// map and stay.
pub fn prune_pending(cache: &Path, link: &Terrain3DLink, ours: &BTreeSet<Loc>, changes: &[RegionChange]) -> std::io::Result<Vec<Loc>> {
    let mut gone = Vec::new();
    for loc in terrain3d_cache::pending_locs(cache)? {
        let known = link.edits.contains_key(&loc) || link.removed.contains_key(&loc) || ours.contains(&loc);
        if known && !changes.iter().any(|c| c.loc == loc) {
            if let Some(path) = terrain3d_cache::pending_path(cache, loc) {
                std::fs::remove_file(path)?;
            }

            gone.push(loc);
        }
    }

    Ok(gone)
}

/// What `restore_pending` took from the folder.
#[derive(Debug, Default, PartialEq)]
pub struct Restored {
    pub locs: Vec<Loc>,
    /// The regions whose file said to apply them whatever Godot did meanwhile.
    pub forced: BTreeSet<Loc>,
    /// Files that could not be read, each named with its region.
    pub problems: Vec<String>,
}

/// Puts the edits a map saved earlier, and Godot has not applied yet, back into `link`. A region the link already has
/// an opinion about is left alone.
pub fn restore_pending(cache: &Path, region_size: u32, link: &mut Terrain3DLink) -> Restored {
    let mut out = Restored::default();
    let locs = match terrain3d_cache::pending_locs(cache) {
        Ok(locs) => locs,
        Err(e) => {
            out.problems.push(e.to_string());
            return out;
        }
    };

    for loc in locs {
        if link.edits.contains_key(&loc) || link.removed.contains_key(&loc) {
            continue;
        }

        let pending = match terrain3d_cache::read_pending(cache, loc, region_size) {
            Ok(p) => p,
            Err(e) => {
                out.problems.push(format!("pending region {loc:?}: {e}"));
                continue;
            }
        };

        if pending.remove {
            link.removed.insert(loc, Removal { hash: pending.base.0, region_size: pending.region.size });
        } else {
            let base_hash = (pending.base != RegionHash::ZERO).then_some(pending.base.0);
            link.edits.insert(loc, RegionEdit::new(Arc::new(pending.region), base_hash));
        }

        if pending.force {
            out.forced.insert(loc);
        }

        out.locs.push(loc);
    }

    out
}

/// How Godot was reached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Route {
    /// The Godot editor with the scene open, over the live link.
    LiveLink,
    /// A Godot started for the job.
    Headless,
    /// Nobody to ask, the files stay pending.
    #[default]
    Waiting,
}

/// One request to have Godot apply the pending files of a data folder.
#[derive(Clone, Debug)]
pub struct Apply {
    /// The live link port, None when the link is off.
    pub port: Option<u16>,
    pub project: PathBuf,
    pub godot: Option<PathBuf>,
    /// The absolute data folder.
    pub folder: PathBuf,
    /// Applies every pending region whatever changed in Godot, on top of the flag some files carry.
    pub force: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub route: Route,
    pub accepted: Vec<Loc>,
    /// Regions Godot kept as they were, with its reason: `dirty`, `changed` or `size`.
    pub refused: Vec<(Loc, String)>,
    pub problems: Vec<String>,
    /// The addon in the open Godot editor predates the Terrain3D events, so a headless Godot did the job.
    pub old_addon: bool,
    /// Godot gave no answer, it may have taken the pending files before it died, so the edits count as unsent.
    pub unanswered: bool,
    /// The data folder of the request.
    pub folder: PathBuf,
}

impl Outcome {
    fn from_reply(route: Route, reply: &Value) -> Self {
        let locs = |v: &Value| -> Vec<Loc> { v.as_array().into_iter().flatten().filter_map(loc_of).collect() };
        let mut out = Outcome { route, accepted: locs(&reply["accepted"]), ..Default::default() };
        out.refused = reply["refused"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| Some((loc_of(&r["loc"])?, r["reason"].as_str().unwrap_or_default().to_string())))
            .collect();
        out.problems = reply["problems"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_string)).collect();
        if let Some(error) = reply["error"].as_str() {
            out.problems.push(error.to_string());
        }

        if reply["ok"].as_bool() != Some(true) && out.problems.is_empty() {
            out.problems.push(reply_error(reply));
        }

        out
    }

    fn failed(route: Route, problem: impl Into<String>) -> Self {
        Self { route, problems: vec![problem.into()], unanswered: true, ..Default::default() }
    }

    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }

    /// The refused regions Godot has unsaved changes in, which it applies once the scene is saved there.
    pub fn unsaved_in_godot(&self) -> Vec<Loc> {
        self.refused.iter().filter(|(_, reason)| reason == DIRTY).map(|(loc, _)| *loc).collect()
    }

    /// The refused regions made at another region size than the terrain in Godot has, which stay here until the size
    /// is set back in Godot or the edit is dropped.
    pub fn resized(&self) -> Vec<Loc> {
        self.refused.iter().filter(|(_, reason)| reason == SIZE).map(|(loc, _)| *loc).collect()
    }

    /// What the status line says, a failure is the first message when `is_ok` is false.
    pub fn status(&self) -> Msg {
        let dirty = self.unsaved_in_godot();
        let resized = self.resized();
        let (applied, refused) = (self.accepted.len(), self.refused.len() - dirty.len() - resized.len());
        let mut out = if let Some(problem) = self.problems.first() {
            msg!("Terrain3D edits were not applied: {problem}", problem)
        } else if self.route == Route::Waiting {
            msg!("Terrain3D edits wait in the data folder until Godot applies them")
        } else if refused > 0 && applied > 0 {
            msg!("Godot applied {applied} Terrain3D region(s), {refused} changed in Godot and stay here", applied, refused)
        } else if refused > 0 {
            msg!("Godot applied no Terrain3D regions, {refused} changed in Godot and stay here", refused)
        } else if applied > 0 {
            msg!("Godot applied {applied} Terrain3D region(s)", applied)
        } else if !dirty.is_empty() || !resized.is_empty() {
            msg!("Godot applied no Terrain3D regions")
        } else {
            msg!("Godot had no Terrain3D edits to apply")
        };

        if self.is_ok() {
            match dirty[..] {
                [] => {}
                [[x, y]] => out = out.then(". ", msg!("Godot has unsaved changes in Terrain3D region {x}, {y}, save the scene there and send again", x, y)),
                _ => {
                    out = out.then(
                        ". ",
                        msg_n!(
                            dirty.len(),
                            "Godot has unsaved changes in {n} Terrain3D region, save the scene there and send again",
                            "Godot has unsaved changes in {n} Terrain3D regions, save the scene there and send again"
                        ),
                    )
                }
            }
        }

        if self.is_ok() && !resized.is_empty() {
            out = out.then(". ", msg!("Some regions were made at another region size than Godot's terrain has, use Godot's or set the size back in Godot"));
        }

        if self.old_addon {
            out = out.then(". ", msg!("Update the TrenchDot addon in Godot to send Terrain3D edits to the open editor"));
        }

        out
    }
}

fn loc_of(v: &Value) -> Option<Loc> {
    let a = v.as_array()?;
    Some([i32::try_from(a.first()?.as_i64()?).ok()?, i32::try_from(a.get(1)?.as_i64()?).ok()?])
}

fn reply_error(reply: &Value) -> String {
    reply["error"].as_str().map_or_else(|| "unexpected reply".to_string(), str::to_string)
}

/// Asks Godot to apply the pending files, blocking. The open editor gets the job first, a headless Godot takes it when
/// there is no editor, an editor without the scene open, or an addon that does not know the event.
pub fn apply(request: &Apply) -> Outcome {
    Outcome { folder: request.folder.clone(), ..ask(request) }
}

fn ask(request: &Apply) -> Outcome {
    let mut old_addon = false;
    if let Some(port) = request.port {
        let message = json!({ "event": "terrain3d_apply", "dir": live_link::godot_path(&request.folder), "force": request.force });
        match live_link::send_timeout(port, &message, LIVE_REPLY) {
            Ok(reply) => match reply["error"].as_str() {
                Some(OLD_ADDON) => old_addon = true,
                Some(error) if error.starts_with(NO_SCENE) => {}
                _ => return Outcome::from_reply(Route::LiveLink, &reply),
            },
            Err(e) if e == live_link::NOT_RUNNING => {}
            Err(e) => return Outcome::failed(Route::LiveLink, e),
        }
    }

    let mut out = headless(request);
    out.old_addon = old_addon;
    out
}

fn headless(request: &Apply) -> Outcome {
    let Some(exe) = &request.godot else { return Outcome::default() };
    match run_cli(exe, &request.project, "apply", &request.folder, &[]) {
        Err(e) => Outcome::failed(Route::Headless, e),
        Ok(reply) => Outcome::from_reply(Route::Headless, &reply),
    }
}

/// Runs the Terrain3D CLI of the addon with `command` on `folder` and the `extra` arguments after it, and returns the
/// JSON result it printed last.
fn run_cli(exe: &Path, project: &Path, command: &str, folder: &Path, extra: &[String]) -> Result<Value, String> {
    let mut cmd = Command::new(exe);
    cmd.arg("--headless").arg("--path").arg(project).arg("--script").arg(CLI_SCRIPT).arg("--").arg(command).arg(live_link::godot_path(folder)).args(extra);
    let (code, stdout) = run_capture(cmd, HEADLESS_TIMEOUT)?;
    if let Some(reply) = stdout.lines().rev().find(|l| !l.trim().is_empty()).and_then(|l| serde_json::from_str::<Value>(l).ok()) {
        return Ok(reply);
    }

    let how = code.map_or_else(|| "Godot crashed".to_string(), |code| format!("Godot crashed or quit with exit code {code}"));
    let after = match command {
        "apply" => ", so the edits stay unsent",
        "settings" => ", the region files it rewrote have a backup in the data folder's .trenchdot/backups",
        _ => "",
    };
    Err(format!(
        "{how} before it printed a result{after}. Run \"{exe}\" --headless --path \"{project}\" --script {CLI_SCRIPT} -- {command} \"{folder}\"{extra} by hand to see why",
        exe = exe.display(),
        project = live_link::godot_path(project),
        folder = live_link::godot_path(folder),
        extra = extra.iter().map(|a| format!(" {a}")).collect::<String>(),
    ))
}

/// Why a linked data folder shows nothing until Godot writes its cache.
pub const NOT_EXPORTED: &str = "Godot has not exported this Terrain3D folder yet, set the Godot executable in Preferences or open the project in Godot, then click Export from Godot in the Inspector";

/// Has Godot write the cache of the request's data folder again, blocking: the open editor when a scene there shows
/// the folder, else a headless Godot. The pending files and `force` play no part. Returns the regions Godot wrote.
pub fn export(request: &Apply) -> Result<Vec<Loc>, String> {
    let written = |reply: &Value| reply["written"].as_array().into_iter().flatten().filter_map(loc_of).collect();
    if let Some(port) = request.port {
        let message = json!({ "event": "terrain3d_export", "dir": live_link::godot_path(&request.folder) });
        match live_link::send_timeout(port, &message, LIVE_REPLY) {
            Ok(reply) => match reply["error"].as_str() {
                Some(OLD_ADDON) => {}
                Some(error) if error.starts_with(NO_SCENE) => {}
                _ if reply["ok"].as_bool() == Some(true) => return Ok(written(&reply)),
                _ => return Err(reply_error(&reply)),
            },
            Err(e) if e == live_link::NOT_RUNNING => {}
            Err(e) => return Err(e),
        }
    }

    let Some(exe) = &request.godot else { return Err(NOT_EXPORTED.to_string()) };
    let reply = run_cli(exe, &request.project, "export", &request.folder, &[])?;
    if reply["ok"].as_bool() == Some(true) { Ok(written(&reply)) } else { Err(reply_error(&reply)) }
}

impl crate::state::EditorState {
    /// What reaches Godot for the data folder of the cache at `cache`, refused in Restricted Mode or without a project.
    pub(crate) fn terrain3d_export_request(&self, cache: &Path) -> Result<Apply, String> {
        if self.restricted() {
            return Err("the project is in Restricted Mode, trust it in the editor to let it reach Godot".to_string());
        }

        let project = self.game.project_root.clone().ok_or("Open a Godot project first, Terrain3D data lives in one")?;
        let folder = cache.parent().ok_or("no Terrain3D data folder")?.to_path_buf();
        Ok(Apply { port: self.prefs.live_link.then_some(self.prefs.live_link_port), project, godot: self.godot.exe.clone(), folder, force: false })
    }

    /// Has Godot write the missing cache of a linked folder, blocking, and reads its manifest.
    pub(crate) fn export_terrain3d_cache(&mut self, cache: &Path) -> Result<Vec<Loc>, String> {
        let written = export(&self.terrain3d_export_request(cache)?)?;
        self.terrain3d.recheck();
        self.terrain3d.refresh(cache);
        if self.terrain3d.manifest(cache).is_none() || self.terrain3d.from_files(cache) {
            return Err(self.terrain3d.error(cache).map_or_else(|| NOT_EXPORTED.to_string(), |e| format!("Godot exported the Terrain3D folder, but {e}")));
        }

        Ok(written)
    }
}

/// Runs `cmd` and returns what it printed, killing it after `timeout`. A failing exit code is the caller's to read from
/// the printed result.
/// The exit code, None when a signal ended the process, and what it printed.
fn run_capture(mut cmd: Command, timeout: Duration) -> Result<(Option<i32>, String), String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = cmd.spawn().map_err(|e| format!("could not start Godot: {e}"))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    });

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok((status.code(), reader.join().map_err(|_| "Godot's output could not be read".to_string())?)),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                // A child of the killed process may still hold the pipe, so the reader is left to finish alone.
                return Err(format!("Godot did not finish within {} seconds", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// The material, assets and ocean switch a settings change asks Godot for, None keeps the node's own. A resource is
/// the res:// path of a saved Terrain3DMaterial or Terrain3DAssets, an empty one clears it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LookChange {
    pub material: Option<String>,
    pub assets: Option<String>,
    pub ocean_enabled: Option<bool>,
}

impl LookChange {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The material, assets and ocean switch the Terrain3D node has, a resource built into the scene has an empty path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub material: String,
    pub assets: String,
    pub ocean_enabled: bool,
}

/// One request to change the region size, vertex spacing and look of a Terrain3D.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsRequest {
    /// The live link port, None when the link is off.
    pub port: Option<u16>,
    pub project: PathBuf,
    /// The Godot that rewrites the region files when no Godot editor runs, None leaves the change to the editor.
    pub godot: Option<PathBuf>,
    pub folder: PathBuf,
    pub region_size: u32,
    pub spacing: f64,
    pub look: LookChange,
}

/// What the Terrain3D has after a settings change. `look` is None from an addon that predates picking it, and from a
/// headless Godot, which changes the grid alone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub region_size: u32,
    pub spacing: f64,
    pub look: Option<Look>,
    pub route: Route,
    /// The folder a headless Godot copied the region files and manifest to first, empty on the live link.
    pub backup: String,
    /// How many regions a headless Godot rewrote.
    pub regions: usize,
    pub problems: Vec<String>,
}

const OLD_ADDON_SETTINGS: &str = "Update the TrenchDot addon in Godot to change Terrain3D settings";
pub const OLD_ADDON_LOOK: &str = "Update the TrenchDot addon in Godot to pick the Terrain3D material, assets or ocean";
const NO_SCENE_SETTINGS: &str = "Open the scene that shows this Terrain3D in the Godot editor first";
pub const LINK_OFF_SETTINGS: &str = en!("Turn on the live link, TrenchDot only rewrites the region files when it can tell no Godot editor has them open");
const HEADLESS_LOOK: &str = "Open the scene that shows this Terrain3D in the Godot editor to pick its material, assets or ocean";

fn settings_message(request: &SettingsRequest) -> Value {
    let mut message = json!({
        "event": "terrain3d_settings",
        "dir": live_link::godot_path(&request.folder),
        "region_size": request.region_size,
        "vertex_spacing": request.spacing,
    });
    let look = &request.look;
    for (key, path) in [("material", &look.material), ("assets", &look.assets)] {
        if let Some(path) = path {
            message[key] = json!(path);
        }
    }

    if let Some(on) = look.ocean_enabled {
        message["ocean_enabled"] = json!(on);
    }

    message
}

/// Changes the Terrain3D settings, blocking. The open Godot editor does it when it shows the folder. A headless Godot
/// rewrites the region files only when no Godot editor runs at all, since an editor may hold the regions in an open
/// scene and save them back over the change. The answer is what Godot has now.
pub fn settings(request: &SettingsRequest) -> Result<Settings, String> {
    let Some(port) = request.port else { return Err(LINK_OFF_SETTINGS.to_string()) };
    match live_link::send_timeout(port, &settings_message(request), LIVE_REPLY) {
        Ok(reply) => live_settings(&reply),
        Err(e) if e == live_link::NOT_RUNNING => match &request.godot {
            Some(exe) => headless_settings(exe, request),
            None => Err(e),
        },
        Err(e) => Err(e),
    }
}

fn live_settings(reply: &Value) -> Result<Settings, String> {
    if reply["ok"].as_bool() == Some(true)
        && let (Some(region_size), Some(spacing)) = (reply["region_size"].as_u64().and_then(|n| u32::try_from(n).ok()), reply["vertex_spacing"].as_f64())
    {
        let look = match (reply["material"].as_str(), reply["assets"].as_str(), reply["ocean_enabled"].as_bool()) {
            (Some(material), Some(assets), Some(ocean_enabled)) => Some(Look { material: material.into(), assets: assets.into(), ocean_enabled }),
            _ => None,
        };
        return Ok(Settings { region_size, spacing, look, route: Route::LiveLink, ..Default::default() });
    }

    Err(match reply["error"].as_str() {
        Some(OLD_ADDON) => OLD_ADDON_SETTINGS.to_string(),
        Some(error) if error.starts_with(NO_SCENE) => NO_SCENE_SETTINGS.to_string(),
        _ => reply_error(reply),
    })
}

fn headless_settings(exe: &Path, request: &SettingsRequest) -> Result<Settings, String> {
    if !request.look.is_empty() {
        return Err(HEADLESS_LOOK.to_string());
    }

    let extra = [request.region_size.to_string(), request.spacing.to_string()];
    let reply = run_cli(exe, &request.project, "settings", &request.folder, &extra)?;
    let problems: Vec<String> = reply["problems"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    if reply["ok"].as_bool() != Some(true) {
        return Err(match reply["error"].as_str().filter(|e| !e.is_empty()) {
            Some(error) => error.to_string(),
            None if !problems.is_empty() => problems.join("; "),
            None => reply_error(&reply),
        });
    }

    Ok(Settings {
        region_size: request.region_size,
        spacing: request.spacing,
        look: None,
        route: Route::Headless,
        backup: reply["backup"].as_str().unwrap_or_default().to_string(),
        regions: reply["written"].as_array().map_or(0, Vec::len),
        problems,
    })
}

/// The regions Godot wrote for an export, or why it did not.
pub type ExportAnswer = Result<Vec<Loc>, String>;

/// Runs the applies on a thread so a save never waits for Godot. A request that arrives while one runs replaces the
/// one waiting, only the latest state matters.
#[derive(Default)]
pub struct Bridge {
    running: Option<Receiver<Outcome>>,
    queued: Option<Apply>,
    told_old_addon: bool,
    settings_running: Option<Receiver<Result<Settings, String>>>,
    settings_queued: Option<SettingsRequest>,
    export_running: Option<(PathBuf, Receiver<ExportAnswer>)>,
    exported: Vec<(PathBuf, ExportAnswer)>,
}

impl Bridge {
    pub fn busy(&self) -> bool {
        self.running.is_some() || self.settings_running.is_some() || self.export_running.is_some()
    }

    /// Whether a send is running or waits for the export in front of it, which `busy` does not say.
    pub fn sending(&self) -> bool {
        self.running.is_some() || self.queued.is_some()
    }

    pub fn applying(&self) -> bool {
        self.running.is_some()
    }

    pub fn send(&mut self, request: Apply) {
        if self.running.is_some() || self.export_running.is_some() {
            self.queued = Some(request);
            return;
        }

        let (tx, rx) = mpsc::channel();
        self.running = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(apply(&request));
        });
    }

    /// Has Godot write the cache at `cache` again on a thread, without waiting. A send that comes meanwhile waits for it.
    pub fn export(&mut self, cache: PathBuf, request: Apply) {
        let (tx, rx) = mpsc::channel();
        self.export_running = Some((cache, rx));
        std::thread::spawn(move || {
            let _ = tx.send(export(&request));
        });
    }

    /// The cache and the answer of an export that finished, and starts the send that waited for it.
    pub fn poll_export(&mut self) -> Option<(PathBuf, ExportAnswer)> {
        self.settle_export();
        (!self.exported.is_empty()).then(|| self.exported.remove(0))
    }

    /// Takes the answer of a finished export and starts the send that waited for it. `poll` does this too, so a caller
    /// that only waits for the send, like the MCP tool, still sees it start.
    fn settle_export(&mut self) {
        let Some(answer) = self.export_running.as_ref().and_then(|(_, rx)| rx.try_recv().ok()) else { return };
        let Some((cache, _)) = self.export_running.take() else { return };
        self.exported.push((cache, answer));
        if self.running.is_none()
            && let Some(next) = self.queued.take()
        {
            self.send(next);
        }
    }

    pub fn send_settings(&mut self, request: SettingsRequest) {
        if self.settings_running.is_some() {
            self.settings_queued = Some(request);
            return;
        }

        let (tx, rx) = mpsc::channel();
        self.settings_running = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(settings(&request));
        });
    }

    /// The answer of the settings change that finished, and starts the one that waited.
    pub fn poll_settings(&mut self) -> Option<Result<Settings, String>> {
        let answer = self.settings_running.as_ref()?.try_recv().ok()?;
        self.settings_running = None;
        if let Some(next) = self.settings_queued.take() {
            self.send_settings(next);
        }

        Some(answer)
    }

    /// The outcome of the apply that finished, and starts the one that waited. The addon notice is on the first outcome
    /// that has it only, the fallback itself works every time.
    pub fn poll(&mut self) -> Option<Outcome> {
        self.settle_export();
        let mut outcome = self.running.as_ref()?.try_recv().ok()?;
        self.running = None;
        if let Some(next) = self.queued.take() {
            self.send(next);
        }

        if outcome.old_addon && std::mem::replace(&mut self.told_old_addon, true) {
            outcome.old_addon = false;
        }

        Some(outcome)
    }
}

/// The note on a folder whose Terrain3D this editor was not tested with.
pub fn untested_version_note(version: &str) -> Msg {
    msg!("Terrain3D {version} was not tested with this editor, edits still go through Godot", version)
}

/// Waits for the outcome, for tests and scripts that need the result before they go on.
pub fn wait(bridge: &mut Bridge, timeout: Duration) -> Option<Outcome> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(outcome) = bridge.poll() {
            return Some(outcome);
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use td_doc::terrain3d::ChangeKind;

    const A: Loc = [-1, 2];
    const B: Loc = [3, -2];

    fn grid() -> Grid {
        Grid { region_size: 64, spacing: 2.5, upm: 32.0 }
    }

    fn region(loc: Loc, seed: f32) -> Arc<Region3D> {
        let mut r = Region3D::blank(loc, 64);
        for (k, h) in r.heights.iter_mut().enumerate() {
            *h = seed + (k % 64) as f32 * 0.5 - (k / 64) as f32 * 0.25;
        }

        Arc::new(r)
    }

    fn change(loc: Loc, kind: ChangeKind, base: Option<u8>, seed: f32) -> RegionChange {
        RegionChange {
            loc,
            kind,
            region: (kind != ChangeKind::Remove).then(|| region(loc, seed)),
            base_hash: base.map(|b| [b; 32]),
            conflict: false,
            resized: false,
        }
    }

    fn cache_of(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("terrain").join(terrain3d_cache::DIR_NAME)
    }

    #[test]
    fn pending_files_carry_the_region_its_base_hash_and_the_flags() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        let changes = [change(A, ChangeKind::Edit, Some(7), 1.5), change(B, ChangeKind::Add, None, -4.0), change([0, 0], ChangeKind::Remove, Some(9), 0.0)];
        let forced = BTreeSet::from([B]);
        let written = write_pending(&cache, &changes, &grid(), &forced).unwrap();
        assert_eq!(written, vec![A, B, [0, 0]]);
        assert_eq!(
            terrain3d_cache::pending_locs(&cache).unwrap(),
            vec![[0, 0], A, B].into_iter().collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>()
        );

        let edit = terrain3d_cache::read_pending(&cache, A, 64).unwrap();
        assert_eq!((edit.base, edit.force, edit.remove, edit.vertex_spacing), (RegionHash([7; 32]), false, false, 2.5));
        assert_eq!(edit.region, *region(A, 1.5));
        let add = terrain3d_cache::read_pending(&cache, B, 64).unwrap();
        assert_eq!((add.base, add.force, add.remove), (RegionHash::ZERO, true, false), "a new region has no base and was forced");
        assert_eq!(add.region, *region(B, -4.0));
        let gone = terrain3d_cache::read_pending(&cache, [0, 0], 64).unwrap();
        assert_eq!((gone.base, gone.force, gone.remove), (RegionHash([9; 32]), false, true));
        assert!(
            !dir.path().join("terrain").join(terrain3d_cache::DIR_NAME).join("pending").read_dir().unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("tdpart"))
        );

        write_pending(&cache, &changes[..1], &grid(), &BTreeSet::new()).unwrap();
        assert_eq!(terrain3d_cache::read_pending(&cache, A, 64).unwrap().region, *region(A, 1.5), "a second save replaces the file");
    }

    #[test]
    fn pruning_drops_only_regions_the_link_took_back() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        let c: Loc = [-2, 3];
        let all = [
            change(A, ChangeKind::Edit, Some(1), 1.0),
            change(B, ChangeKind::Edit, Some(2), 2.0),
            change(c, ChangeKind::Edit, Some(4), 4.0),
            change([5, 5], ChangeKind::Edit, Some(3), 3.0),
        ];
        write_pending(&cache, &all, &grid(), &BTreeSet::new()).unwrap();

        // The link still edits A and B, B was undone since the save to what Godot has, the first stroke on c was undone
        // so the link lost it, [5, 5] belongs to another map's link.
        let mut link = Terrain3DLink::new("res://terrain");
        link.edits.insert(A, RegionEdit::new(region(A, 1.0), Some([1; 32])));
        link.edits.insert(B, RegionEdit::new(region(B, 2.0), Some([2; 32])));
        let ours = BTreeSet::from([A, B, c]);
        assert_eq!(prune_pending(&cache, &link, &BTreeSet::new(), &all[..1]).unwrap(), vec![B], "without ours only what the link knows");
        assert_eq!(prune_pending(&cache, &link, &ours, &all[..1]).unwrap(), vec![c]);
        assert_eq!(terrain3d_cache::pending_locs(&cache).unwrap(), vec![A, [5, 5]]);
        assert!(prune_pending(&cache, &link, &ours, &all[..1]).unwrap().is_empty(), "a second save finds nothing left to take back");

        assert!(prune_pending(&dir.path().join("none"), &link, &ours, &[]).unwrap().is_empty(), "no folder, nothing to prune");
    }

    #[test]
    fn restoring_puts_saved_edits_back_without_touching_what_the_link_has() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_of(&dir);
        let changes = [
            change(A, ChangeKind::Edit, Some(7), 1.5),
            change(B, ChangeKind::Add, None, -4.0),
            change([0, 0], ChangeKind::Remove, Some(9), 0.0),
            change([2, 2], ChangeKind::Edit, Some(4), 8.0),
        ];
        write_pending(&cache, &changes, &grid(), &BTreeSet::from([[0, 0]])).unwrap();
        // A file of another region size cannot be restored, and says which one.
        let other = PendingRegion { region: Region3D::blank([6, 6], 128), vertex_spacing: 1.0, base: RegionHash::ZERO, force: false, remove: false };
        terrain3d_cache::write_pending(&cache, &other).unwrap();

        let mut link = Terrain3DLink::new("res://terrain");
        link.edits.insert([2, 2], RegionEdit::new(region([2, 2], 99.0), Some([4; 32])));
        let restored = restore_pending(&cache, 64, &mut link);
        assert_eq!(restored.locs, vec![A, [0, 0], B]);
        assert_eq!(restored.forced, BTreeSet::from([[0, 0]]));
        assert_eq!(restored.problems.len(), 1);
        assert!(restored.problems[0].contains("[6, 6]"), "{:?}", restored.problems);
        assert_eq!(link.edits.len(), 3);
        assert_eq!(*link.edits[&A].region, *region(A, 1.5));
        assert_eq!(link.edits[&A].base_hash, Some([7; 32]));
        assert_eq!(link.edits[&B].base_hash, None, "a region TrenchDot added has no base");
        assert_eq!(*link.edits[&[2, 2]].region, *region([2, 2], 99.0), "the link's own edit wins over the file");
        assert_eq!(link.removed.len(), 1);
        assert_eq!(link.removed[&[0, 0]], Removal { hash: [9; 32], region_size: 64 });

        let mut empty = Terrain3DLink::new("res://terrain");
        assert_eq!(restore_pending(&dir.path().join("none"), 64, &mut empty), Restored::default());
    }

    /// Answers each event like the addon would, remembers what it was asked.
    fn fake_editor(answers: Vec<(&'static str, Value)>) -> (u16, Arc<Mutex<Vec<Value>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut line = String::new();
                if BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }

                let message: Value = serde_json::from_str(&line).unwrap();
                let event = message["event"].as_str().unwrap_or_default().to_string();
                let reply = answers.iter().find(|(e, _)| *e == event).map_or(json!({ "ok": false, "error": "unknown event" }), |(_, r)| r.clone());
                log.lock().unwrap().push(message);
                let _ = writeln!(&stream, "{reply}");
            }
        });
        (port, seen)
    }

    /// A script that stands in for the Godot executable: it records its arguments and prints `result`.
    fn fake_godot(dir: &Path, result: &str) -> (PathBuf, PathBuf) {
        let args = dir.join("args.txt");
        let (path, script) = if cfg!(windows) {
            (dir.join("godot.cmd"), format!("@echo off\r\necho %* > \"{}\"\r\necho {result}\r\n", args.display()))
        } else {
            (dir.join("godot.sh"), format!("#!/bin/sh\necho \"$@\" > '{}'\necho '{result}'\n", args.display()))
        };
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        (path, args)
    }

    fn request(dir: &Path, port: Option<u16>, godot: Option<PathBuf>) -> Apply {
        Apply { port, project: dir.join("game"), godot, folder: dir.join("game").join("terrain"), force: false }
    }

    #[test]
    fn the_open_editor_gets_the_apply_event_and_its_answer_is_the_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let reply = json!({ "ok": true, "accepted": [[-1, 2], [3, -2]], "refused": [{ "loc": [0, 5], "reason": "dirty" }], "problems": [] });
        let (port, seen) = fake_editor(vec![("terrain3d_apply", reply)]);
        let mut req = request(dir.path(), Some(port), None);
        req.force = true;
        let out = apply(&req);
        assert_eq!(out.route, Route::LiveLink);
        assert_eq!(out.accepted, vec![A, B]);
        assert_eq!(out.refused, vec![([0, 5], "dirty".to_string())]);
        assert!(out.is_ok() && !out.old_addon);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!((seen[0]["event"].as_str(), seen[0]["force"].as_bool()), (Some("terrain3d_apply"), Some(true)));
        assert_eq!(seen[0]["dir"].as_str(), Some(live_link::godot_path(&req.folder).as_str()));
    }

    #[test]
    fn an_error_from_the_editor_is_reported_and_no_headless_godot_starts() {
        let dir = tempfile::tempdir().unwrap();
        let (port, _) = fake_editor(vec![("terrain3d_apply", json!({ "ok": false, "error": "Terrain3D 1.1.0 lacks update_maps" }))]);
        let (exe, args) = fake_godot(dir.path(), r#"{"ok":true}"#);
        let out = apply(&request(dir.path(), Some(port), Some(exe)));
        assert_eq!((out.route, out.problems), (Route::LiveLink, vec!["Terrain3D 1.1.0 lacks update_maps".to_string()]));
        assert!(!args.exists(), "the editor answered, so nothing else ran");
    }

    #[test]
    fn an_editor_without_the_scene_open_leaves_the_job_to_a_headless_godot() {
        let dir = tempfile::tempdir().unwrap();
        let (port, _) = fake_editor(vec![("terrain3d_apply", json!({ "ok": false, "error": format!("{NO_SCENE}C:/game/terrain") }))]);
        let (exe, _) = fake_godot(dir.path(), r#"{"ok":true,"accepted":[[3,-2]],"refused":[],"problems":[]}"#);
        let out = apply(&request(dir.path(), Some(port), Some(exe)));
        assert_eq!((out.route, out.accepted.clone(), out.old_addon), (Route::Headless, vec![B], false), "{out:?}");
    }

    #[test]
    fn the_headless_run_gets_the_cli_script_and_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (exe, args) = fake_godot(dir.path(), r#"{"ok":true,"accepted":[[-1,2]],"refused":[{"loc":[3,-2],"reason":"changed"}],"problems":[]}"#);
        let req = request(dir.path(), None, Some(exe));
        let out = apply(&req);
        assert_eq!(out.route, Route::Headless);
        assert_eq!((out.accepted.clone(), out.refused.clone()), (vec![A], vec![(B, "changed".to_string())]));
        let args = std::fs::read_to_string(&args).unwrap();
        for part in ["--headless", "--path", "terrain3d_cli.gd", " -- apply "] {
            assert!(args.contains(part), "{part} is missing from {args}");
        }

        assert!(args.contains(&live_link::godot_path(&req.project)) && args.trim_end().ends_with(&live_link::godot_path(&req.folder)), "{args}");
    }

    #[test]
    fn an_export_goes_to_the_open_editor_first_then_to_a_headless_godot() {
        let dir = tempfile::tempdir().unwrap();
        let (port, seen) = fake_editor(vec![("terrain3d_export", json!({ "ok": true, "written": [], "problems": [] }))]);
        assert_eq!(export(&request(dir.path(), Some(port), None)), Ok(vec![]));
        assert_eq!(seen.lock().unwrap()[0]["event"], "terrain3d_export");

        let (port, _) = fake_editor(vec![("terrain3d_export", json!({ "ok": false, "error": format!("{NO_SCENE}res://terrain") }))]);
        let (exe, args) = fake_godot(dir.path(), r#"{"ok":true,"written":[[0,0]],"removed":0,"problems":[]}"#);
        let req = request(dir.path(), Some(port), Some(exe));
        assert_eq!(export(&req), Ok(vec![[0, 0]]));
        let args = std::fs::read_to_string(&args).unwrap();
        assert!(args.contains(" -- export ") && args.trim_end().ends_with(&live_link::godot_path(&req.folder)), "{args}");

        let (exe, _) = fake_godot(dir.path(), r#"{"ok":false,"error":"Terrain3D is not installed in this project"}"#);
        assert_eq!(export(&request(dir.path(), None, Some(exe))), Err("Terrain3D is not installed in this project".to_string()));
        assert_eq!(export(&request(dir.path(), None, None)), Err(NOT_EXPORTED.to_string()));
    }

    #[test]
    fn an_old_addon_falls_back_to_a_headless_godot_and_says_so_once() {
        let dir = tempfile::tempdir().unwrap();
        let (port, seen) = fake_editor(Vec::new());
        let (exe, _) = fake_godot(dir.path(), r#"{"ok":true,"accepted":[[-1,2]],"refused":[],"problems":[]}"#);
        let mut bridge = Bridge::default();
        bridge.send(request(dir.path(), Some(port), Some(exe.clone())));
        assert!(bridge.busy());
        let first = wait(&mut bridge, Duration::from_secs(20)).expect("first outcome");
        assert_eq!((first.route, first.accepted.clone(), first.old_addon), (Route::Headless, vec![A], true));
        assert!(first.status().en.contains("Update the TrenchDot addon in Godot"), "{}", first.status().en);

        bridge.send(request(dir.path(), Some(port), Some(exe)));
        let second = wait(&mut bridge, Duration::from_secs(20)).expect("second outcome");
        assert_eq!((second.route, second.accepted.clone(), second.old_addon), (Route::Headless, vec![A], false), "the notice is not repeated");
        assert!(!second.status().en.contains("Update the TrenchDot addon"));
        assert_eq!(seen.lock().unwrap().len(), 2, "the editor is still asked first every time");
        assert!(!bridge.busy());
    }

    #[test]
    fn without_an_editor_or_godot_the_edits_wait() {
        let dir = tempfile::tempdir().unwrap();
        let out = apply(&request(dir.path(), None, None));
        assert_eq!(out.route, Route::Waiting);
        assert!(out.is_ok());
        assert_eq!(out.status().en, "Terrain3D edits wait in the data folder until Godot applies them");
        // A port nobody listens on is the same as no editor.
        let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(apply(&request(dir.path(), Some(free), None)).route, Route::Waiting);
    }

    #[test]
    fn a_godot_that_prints_nothing_useful_is_a_problem() {
        let dir = tempfile::tempdir().unwrap();
        let (exe, _) = fake_godot(dir.path(), "not json at all");
        let out = apply(&request(dir.path(), None, Some(exe)));
        assert_eq!((out.route, out.unanswered), (Route::Headless, true));
        let problem = &out.problems[0];
        assert!(problem.starts_with("Godot crashed or quit with exit code 0 before it printed a result, so the edits stay unsent"), "{problem}");
        assert!(problem.contains("terrain3d_cli.gd -- apply") && problem.contains("by hand"), "{problem}");

        let crash = if cfg!(windows) { dir.path().join("crash.cmd") } else { dir.path().join("crash.sh") };
        std::fs::write(
            &crash,
            if cfg!(windows) {
                "@echo off
exit /b 3
"
            } else {
                "#!/bin/sh
exit 3
"
            },
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&crash, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let out = apply(&request(dir.path(), None, Some(crash.clone())));
        assert!(out.unanswered && out.problems[0].starts_with("Godot crashed or quit with exit code 3 "), "{:?}", out.problems);
        assert_eq!(out.folder, request(dir.path(), None, None).folder);
        let export = export(&request(dir.path(), None, Some(crash))).unwrap_err();
        assert!(export.starts_with("Godot crashed or quit with exit code 3 before it printed a result. Run"), "{export}");

        let (exe, _) = fake_godot(dir.path(), r#"{"ok":false,"error":"no Terrain3D data folder at C:/x"}"#);
        let out = apply(&request(dir.path(), None, Some(exe)));
        assert_eq!((out.problems.clone(), out.unanswered), (vec!["no Terrain3D data folder at C:/x".to_string()], false));
        assert!(out.status().en.starts_with("Terrain3D edits were not applied: no Terrain3D data folder"));

        let missing = apply(&request(dir.path(), None, Some(dir.path().join("no-such-godot"))));
        assert!(missing.problems[0].starts_with("could not start Godot"), "{:?}", missing.problems);
    }

    #[test]
    fn a_godot_that_hangs_is_killed_after_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let (path, script) = if cfg!(windows) {
            (dir.path().join("hang.cmd"), "@echo off\r\nping -n 20 127.0.0.1 > nul\r\n".to_string())
        } else {
            (dir.path().join("hang.sh"), "#!/bin/sh\nsleep 20\n".to_string())
        };
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let start = Instant::now();
        let result = run_capture(Command::new(&path), Duration::from_millis(300));
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(result.unwrap_err().contains("did not finish"));
    }

    #[test]
    fn a_request_that_arrives_while_one_runs_replaces_the_waiting_one() {
        let dir = tempfile::tempdir().unwrap();
        let (port, seen) = fake_editor(vec![("terrain3d_apply", json!({ "ok": true, "accepted": [[1, 1]] }))]);
        let mut bridge = Bridge::default();
        let mut req = request(dir.path(), Some(port), None);
        bridge.send(req.clone());
        for force in [false, true] {
            req.force = force;
            bridge.send(req.clone());
        }

        let mut outcomes = 0;
        while let Some(out) = wait(&mut bridge, Duration::from_secs(5)) {
            assert_eq!(out.accepted, vec![[1, 1]]);
            outcomes += 1;
        }

        assert_eq!(outcomes, 2, "the one that ran and the latest one that waited");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1]["force"].as_bool(), Some(true), "the newer request won");
    }

    #[test]
    fn the_status_names_what_happened() {
        let out = |accepted: Vec<Loc>, refused: Vec<(Loc, String)>| Outcome { route: Route::LiveLink, accepted, refused, ..Default::default() };
        assert_eq!(out(vec![A], vec![]).status().en, "Godot applied 1 Terrain3D region(s)");
        assert_eq!(out(vec![A, B], vec![([0, 0], "changed".into())]).status().en, "Godot applied 2 Terrain3D region(s), 1 changed in Godot and stay here");
        let size_note = "Some regions were made at another region size than Godot's terrain has, use Godot's or set the size back in Godot";
        assert_eq!(
            out(vec![], vec![([0, 0], "size".into()), ([1, 0], "changed".into())]).status().en,
            format!("Godot applied no Terrain3D regions, 1 changed in Godot and stay here. {size_note}")
        );
        assert_eq!(out(vec![], vec![([0, 0], "size".into()), ([1, 0], "size".into())]).status().en, format!("Godot applied no Terrain3D regions. {size_note}"));
        assert_eq!(out(vec![A], vec![([0, 0], "size".into())]).status().en, format!("Godot applied 1 Terrain3D region(s). {size_note}"));
        assert_eq!(out(vec![], vec![([0, 0], "size".into())]).resized(), vec![[0, 0]]);
        assert!(out(vec![], vec![([0, 0], "changed".into())]).resized().is_empty());
        assert_eq!(
            out(vec![A], vec![([-3, 2], "dirty".into())]).status().en,
            "Godot applied 1 Terrain3D region(s). Godot has unsaved changes in Terrain3D region -3, 2, save the scene there and send again"
        );
        assert_eq!(
            out(vec![], vec![([-3, 2], "dirty".into())]).status().en,
            "Godot applied no Terrain3D regions. Godot has unsaved changes in Terrain3D region -3, 2, save the scene there and send again"
        );
        let mixed = out(vec![], vec![([-3, 2], "dirty".into()), ([4, 0], "changed".into()), ([5, -1], "dirty".into())]);
        assert_eq!(mixed.unsaved_in_godot(), vec![[-3, 2], [5, -1]]);
        assert_eq!(
            mixed.status().en,
            "Godot applied no Terrain3D regions, 1 changed in Godot and stay here. Godot has unsaved changes in 2 Terrain3D regions, save the scene there and send again"
        );
        assert_eq!(out(vec![], vec![]).status().en, "Godot had no Terrain3D edits to apply");
        assert!(untested_version_note("1.2.0").en.starts_with("Terrain3D 1.2.0 was not tested"));
    }

    fn settings_request(port: u16, folder: PathBuf, region_size: u32, spacing: f64, look: LookChange) -> SettingsRequest {
        let project = folder.parent().map(Path::to_path_buf).unwrap_or_default();
        SettingsRequest { port: Some(port), project, godot: None, folder, region_size, spacing, look }
    }

    #[test]
    fn the_settings_event_names_the_folder_and_only_the_look_it_changes() {
        let folder = PathBuf::from("D:/proj/terrain");
        let dir = live_link::godot_path(&folder);
        let grid = settings_request(1, folder.clone(), 128, 2.5, LookChange::default());
        assert_eq!(settings_message(&grid), json!({ "event": "terrain3d_settings", "dir": dir, "region_size": 128, "vertex_spacing": 2.5 }));

        let look = LookChange { material: Some("res://terrain/look.tres".into()), assets: Some(String::new()), ocean_enabled: Some(false) };
        assert_eq!(
            settings_message(&settings_request(1, folder.clone(), 512, 0.75, look)),
            json!({ "event": "terrain3d_settings", "dir": dir, "region_size": 512, "vertex_spacing": 0.75, "material": "res://terrain/look.tres", "assets": "", "ocean_enabled": false })
        );
        let ocean = LookChange { ocean_enabled: Some(true), ..Default::default() };
        assert!(!ocean.is_empty() && LookChange::default().is_empty());
        assert_eq!(
            settings_message(&settings_request(1, folder, 64, 3.0, ocean)),
            json!({ "event": "terrain3d_settings", "dir": dir, "region_size": 64, "vertex_spacing": 3.0, "ocean_enabled": true })
        );
    }

    #[test]
    fn the_open_editor_changes_the_settings_and_answers_what_it_has_now() {
        let dir = tempfile::tempdir().unwrap();
        let answer = json!({ "ok": true, "region_size": 512, "vertex_spacing": 0.75, "material": "res://terrain/m.tres", "assets": "", "ocean_enabled": true });
        let (port, seen) = fake_editor(vec![("terrain3d_settings", answer)]);
        let look = LookChange { material: Some("res://terrain/m.tres".into()), assets: None, ocean_enabled: Some(true) };
        let got = settings(&settings_request(port, dir.path().join("terrain"), 512, 0.75, look));
        let want = Look { material: "res://terrain/m.tres".into(), assets: String::new(), ocean_enabled: true };
        assert_eq!(got, Ok(Settings { region_size: 512, spacing: 0.75, look: Some(want), route: Route::LiveLink, ..Default::default() }));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!((seen[0]["region_size"].as_u64(), seen[0]["vertex_spacing"].as_f64()), (Some(512), Some(0.75)));
        assert_eq!((seen[0]["material"].as_str(), seen[0].get("assets"), seen[0]["ocean_enabled"].as_bool()), (Some("res://terrain/m.tres"), None, Some(true)));
    }

    #[test]
    fn an_addon_that_predates_the_look_answers_the_grid_alone() {
        let dir = tempfile::tempdir().unwrap();
        let look = LookChange { assets: Some("res://terrain/a.tres".into()), ..Default::default() };
        let answers = [
            json!({ "ok": true, "region_size": 256, "vertex_spacing": 2.0 }),
            json!({ "ok": true, "region_size": 256, "vertex_spacing": 2.0, "material": "", "assets": "res://terrain/a.tres" }),
            json!({ "ok": true, "region_size": 256, "vertex_spacing": 2.0, "material": "", "assets": 3, "ocean_enabled": false }),
        ];
        for answer in answers {
            let (port, _) = fake_editor(vec![("terrain3d_settings", answer.clone())]);
            let got = settings(&settings_request(port, dir.path().join("terrain"), 256, 2.0, look.clone()));
            assert_eq!(got, Ok(Settings { region_size: 256, spacing: 2.0, route: Route::LiveLink, ..Default::default() }), "{answer}");
        }
    }

    #[test]
    fn a_refused_settings_change_says_why_in_plain_words() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("terrain");
        let request = |port| settings_request(port, folder.clone(), 64, 1.0, LookChange { material: Some("res://x.tres".into()), ..Default::default() });
        let refused = |error: &'static str| {
            let (port, _) = fake_editor(vec![("terrain3d_settings", json!({ "ok": false, "error": error }))]);
            settings(&request(port)).unwrap_err()
        };
        assert_eq!(refused("unknown event"), OLD_ADDON_SETTINGS);
        assert_eq!(refused("no open scene shows the Terrain3D data in res://terrain"), NO_SCENE_SETTINGS);
        assert_eq!(refused("Terrain3D has no region size 100, use 64"), "Terrain3D has no region size 100, use 64");
        assert_eq!(refused("res://x.tres is not a Terrain3DMaterial"), "res://x.tres is not a Terrain3DMaterial");
        let (port, _) = fake_editor(vec![("terrain3d_settings", json!({ "ok": true, "material": "res://x.tres", "assets": "", "ocean_enabled": false }))]);
        assert_eq!(settings(&request(port)), Err("unexpected reply".to_string()), "a reply without the grid is not a success");
        assert_eq!(settings(&request(CLOSED)), Err(live_link::NOT_RUNNING.to_string()), "with no Godot to run the change waits for the editor");
    }

    /// A port nobody listens on, so the live link says no Godot editor runs.
    const CLOSED: u16 = 1;

    #[test]
    fn without_a_godot_editor_a_headless_godot_rewrites_the_region_files() {
        let dir = tempfile::tempdir().unwrap();
        let reply = r#"{"ok":true,"error":"","backup":"C:/game/terrain/.trenchdot/backups/20261119T101500","written":[[0,0],[1,0],[0,1]],"removed":[],"problems":["kept the spacing"]}"#;
        let (exe, args) = fake_godot(dir.path(), reply);
        let folder = dir.path().join("game").join("terrain");
        let request = SettingsRequest { godot: Some(exe), ..settings_request(CLOSED, folder.clone(), 128, 2.5, LookChange::default()) };
        let got = settings(&request).unwrap();
        assert_eq!((got.route, got.region_size, got.spacing, got.look.clone()), (Route::Headless, 128, 2.5, None));
        assert_eq!(
            (got.backup.as_str(), got.regions, got.problems),
            ("C:/game/terrain/.trenchdot/backups/20261119T101500", 3, vec!["kept the spacing".to_string()])
        );
        let args = std::fs::read_to_string(&args).unwrap();
        for part in ["--headless", "--path", "terrain3d_cli.gd", " -- settings "] {
            assert!(args.contains(part), "{part} is missing from {args}");
        }

        assert!(args.trim_end().ends_with(&format!("{} 128 2.5", live_link::godot_path(&folder))), "{args}");
    }

    #[test]
    fn a_headless_refusal_is_the_settings_error() {
        let dir = tempfile::tempdir().unwrap();
        let refused = |reply: &str| {
            let (exe, _) = fake_godot(dir.path(), reply);
            settings(&SettingsRequest { godot: Some(exe), ..settings_request(CLOSED, dir.path().join("terrain"), 64, 1.0, LookChange::default()) }).unwrap_err()
        };
        let held = "the regions at (0, 0) hold Terrain3D instances, which only their scene can move";
        assert_eq!(refused(&format!(r#"{{"ok":false,"error":"{held}"}}"#)), held);
        assert_eq!(refused(r#"{"ok":false,"error":"","problems":["could not save","could not export"]}"#), "could not save; could not export");
        assert_eq!(refused(r#"{"ok":false}"#), "unexpected reply");
    }

    #[test]
    fn a_running_godot_editor_or_a_look_change_never_starts_a_headless_godot() {
        let dir = tempfile::tempdir().unwrap();
        let (exe, args) = fake_godot(dir.path(), r#"{"ok":true}"#);
        let request = |port, look| SettingsRequest { godot: Some(exe.clone()), ..settings_request(port, dir.path().join("terrain"), 64, 1.0, look) };
        for (error, want) in [("unknown event", OLD_ADDON_SETTINGS), ("no open scene shows the Terrain3D data in res://terrain", NO_SCENE_SETTINGS)] {
            let (port, _) = fake_editor(vec![("terrain3d_settings", json!({ "ok": false, "error": error }))]);
            assert_eq!(settings(&request(port, LookChange::default())), Err(want.to_string()));
        }

        let look = LookChange { ocean_enabled: Some(true), ..Default::default() };
        assert_eq!(settings(&request(CLOSED, look)), Err(HEADLESS_LOOK.to_string()));
        assert_eq!(settings(&SettingsRequest { port: None, ..request(CLOSED, LookChange::default()) }), Err(LINK_OFF_SETTINGS.to_string()));
        assert!(!args.exists(), "no Godot ran");
    }

    #[test]
    fn settings_run_on_a_thread_and_the_latest_waiting_request_wins() {
        let dir = tempfile::tempdir().unwrap();
        let (port, seen) = fake_editor(vec![("terrain3d_settings", json!({ "ok": true, "region_size": 64, "vertex_spacing": 1.0 }))]);
        let mut bridge = Bridge::default();
        let request = |region_size| settings_request(port, dir.path().join("terrain"), region_size, 1.0, LookChange::default());
        assert!(bridge.poll_settings().is_none());
        for size in [64, 128, 512] {
            bridge.send_settings(request(size));
        }

        assert!(bridge.busy());
        let mut answers = 0;
        let start = Instant::now();
        while answers < 2 && start.elapsed() < Duration::from_secs(5) {
            answers += usize::from(bridge.poll_settings().is_some());
            std::thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(answers, 2);
        let sizes: Vec<u64> = seen.lock().unwrap().iter().filter_map(|m| m["region_size"].as_u64()).collect();
        assert_eq!(sizes, vec![64, 512], "the middle request was replaced");
        assert!(!bridge.busy());
    }
}
