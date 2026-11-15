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
use td_doc::terrain3d::{RegionChange, RegionEdit, Terrain3DLink};
use td_formats::terrain3d_cache::{self, CacheError, PendingRegion, RegionHash};
use td_geom::terrain3d::{Grid, Loc, Region3D};

use crate::i18n::{Msg, msg, msg_n};
use crate::live_link;

const CLI_SCRIPT: &str = "res://addons/func_godot/src/trenchdot/terrain3d_cli.gd";
const LIVE_REPLY: Duration = Duration::from_secs(60);
const HEADLESS_TIMEOUT: Duration = Duration::from_secs(120);
const OLD_ADDON: &str = "unknown event";
const NO_SCENE: &str = "no open scene shows the Terrain3D data in ";
/// The reason Godot refuses a region whose scene has unsaved changes there.
pub const DIRTY: &str = "dirty";

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

/// Deletes the pending files of regions the link has an opinion about but no longer differs in, the ones an undo took
/// back after the save. Files for regions the link knows nothing of belong to another map and stay.
pub fn prune_pending(cache: &Path, link: &Terrain3DLink, changes: &[RegionChange]) -> std::io::Result<Vec<Loc>> {
    let mut gone = Vec::new();
    for loc in terrain3d_cache::pending_locs(cache)? {
        let known = link.edits.contains_key(&loc) || link.removed.contains_key(&loc);
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
            link.removed.insert(loc, pending.base.0);
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

    /// What the status line says, a failure is the first message when `is_ok` is false.
    pub fn status(&self) -> Msg {
        let dirty = self.unsaved_in_godot();
        let (applied, refused) = (self.accepted.len(), self.refused.len() - dirty.len());
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
        } else if !dirty.is_empty() {
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
    let mut cmd = Command::new(exe);
    cmd.arg("--headless")
        .arg("--path")
        .arg(&request.project)
        .arg("--script")
        .arg(CLI_SCRIPT)
        .arg("--")
        .arg("apply")
        .arg(live_link::godot_path(&request.folder));
    let (code, stdout) = run_capture(cmd, HEADLESS_TIMEOUT)?;
    if let Some(reply) = stdout.lines().rev().find(|l| !l.trim().is_empty()).and_then(|l| serde_json::from_str::<Value>(l).ok()) {
        return Ok(reply);
    }

    let how = code.map_or_else(|| "Godot crashed".to_string(), |code| format!("Godot crashed or quit with exit code {code}"));
    let unsent = if command == "apply" { ", so the edits stay unsent" } else { "" };
    Err(format!(
        "{how} before it printed a result{unsent}. Run \"{exe}\" --headless --path \"{project}\" --script {CLI_SCRIPT} -- {command} \"{folder}\" by hand to see why",
        exe = exe.display(),
        project = live_link::godot_path(&request.project),
        folder = live_link::godot_path(&request.folder),
    ))
}

/// Why a linked data folder shows nothing until Godot writes its cache.
pub const NOT_EXPORTED: &str = "Godot has not exported this Terrain3D folder yet, set the Godot executable in Preferences or open the project in Godot, then click Export from Godot in the Inspector";

/// Has Godot write the cache of the request's data folder again, blocking: the open editor when a scene there shows
/// the folder, else a headless Godot. The pending files and `force` play no part.
pub fn export(request: &Apply) -> Result<(), String> {
    if let Some(port) = request.port {
        let message = json!({ "event": "terrain3d_export", "dir": live_link::godot_path(&request.folder) });
        match live_link::send_timeout(port, &message, LIVE_REPLY) {
            Ok(reply) => match reply["error"].as_str() {
                Some(OLD_ADDON) => {}
                Some(error) if error.starts_with(NO_SCENE) => {}
                _ if reply["ok"].as_bool() == Some(true) => return Ok(()),
                _ => return Err(reply_error(&reply)),
            },
            Err(e) if e == live_link::NOT_RUNNING => {}
            Err(e) => return Err(e),
        }
    }

    let Some(exe) = &request.godot else { return Err(NOT_EXPORTED.to_string()) };
    let reply = run_cli(exe, request, "export")?;
    if reply["ok"].as_bool() == Some(true) { Ok(()) } else { Err(reply_error(&reply)) }
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
    pub(crate) fn export_terrain3d_cache(&mut self, cache: &Path) -> Result<(), String> {
        export(&self.terrain3d_export_request(cache)?)?;
        self.terrain3d.recheck();
        self.terrain3d.refresh(cache);
        if self.terrain3d.manifest(cache).is_none() {
            return Err(self.terrain3d.error(cache).map_or_else(|| NOT_EXPORTED.to_string(), |e| format!("Godot exported the Terrain3D folder, but {e}")));
        }

        Ok(())
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

/// Runs the applies on a thread so a save never waits for Godot. A request that arrives while one runs replaces the
/// one waiting, only the latest state matters.
#[derive(Default)]
pub struct Bridge {
    running: Option<Receiver<Outcome>>,
    queued: Option<Apply>,
    told_old_addon: bool,
}

impl Bridge {
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    pub fn applying(&self) -> bool {
        self.running.is_some()
    }

    pub fn send(&mut self, request: Apply) {
        if self.running.is_some() {
            self.queued = Some(request);
            return;
        }

        let (tx, rx) = mpsc::channel();
        self.running = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(apply(&request));
        });
    }

    /// The outcome of the apply that finished, and starts the one that waited. The addon notice is on the first outcome
    /// that has it only, the fallback itself works every time.
    pub fn poll(&mut self) -> Option<Outcome> {
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
        RegionChange { loc, kind, region: (kind != ChangeKind::Remove).then(|| region(loc, seed)), base_hash: base.map(|b| [b; 32]), conflict: false }
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
        let all = [change(A, ChangeKind::Edit, Some(1), 1.0), change(B, ChangeKind::Edit, Some(2), 2.0), change([5, 5], ChangeKind::Edit, Some(3), 3.0)];
        write_pending(&cache, &all, &grid(), &BTreeSet::new()).unwrap();

        // The link still edits A and B, B was undone since the save, [5, 5] belongs to another map's link.
        let mut link = Terrain3DLink::new("res://terrain");
        link.edits.insert(A, RegionEdit::new(region(A, 1.0), Some([1; 32])));
        link.edits.insert(B, RegionEdit::new(region(B, 2.0), Some([2; 32])));
        let gone = prune_pending(&cache, &link, &all[..1]).unwrap();
        assert_eq!(gone, vec![B]);
        assert_eq!(terrain3d_cache::pending_locs(&cache).unwrap(), vec![A, [5, 5]]);

        assert!(prune_pending(&dir.path().join("none"), &link, &[]).unwrap().is_empty(), "no folder, nothing to prune");
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
        assert_eq!(link.removed[&[0, 0]], [9; 32]);

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
        assert_eq!(
            out(vec![], vec![([0, 0], "size".into()), ([1, 0], "changed".into())]).status().en,
            "Godot applied no Terrain3D regions, 2 changed in Godot and stay here"
        );
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
}
