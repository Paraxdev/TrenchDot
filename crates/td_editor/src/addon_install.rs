//! The TrenchDot addon in a Godot project: whether it is there, matches the editor and is enabled, installing or
//! updating it from the release the content downloads come from, and enabling it in `project.godot`.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;

use td_doc::legacy;

use crate::content::{Added, Download, Error, Progress, Reporter, io_error};
use crate::i18n::{self, Msg, en, msg};

pub const ADDON: Download = Download { asset: "trenchdot-addon.zip", folders: &["addons/trenchdot"], approx_mb: 1 };
/// Where the addon lives in a project.
pub const DIR: &str = "addons/trenchdot";
/// How `project.godot` names the addon among its enabled plugins.
pub const PLUGIN: &str = "res://addons/trenchdot/plugin.cfg";
/// Holds the addon while it is unpacked, and the old addons an update replaced. Godot neither imports nor lists folders
/// whose names start with a dot. A backup under `addons` would still show as a second TrenchDot in Godot's plugin
/// list, even with a `.gdignore` in it.
pub const WORK_DIR: &str = ".trenchdot";
pub const BACKUPS: &str = ".trenchdot/addon-backups";
/// Starts the name of the folder in [`WORK_DIR`] the addon is unpacked into. One a crash left behind goes on the next
/// install.
const UNPACKING: &str = "addon-new-";

const GIT: &str = en!("res://addons/trenchdot is a git checkout, update it with git");
pub const LEGACY_GIT: &str = en!("res://addons/func_godot is a git checkout, remove it with git");
const LEGACY_GIT_MOVE: &str = en!("res://addons/func_godot is a git checkout, move it to res://addons/trenchdot with git");

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Installed {
    #[default]
    Missing,
    /// The folder is there, but its plugin.cfg names no version.
    Unknown,
    Older(String),
    Newer(String),
    Current,
}

/// What is left of a move out of [`legacy::ADDON_DIR`] that happened without the editor, by hand or with git.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leftover {
    /// An older TrenchDot addon is still in the old folder, so Godot finds each of its classes twice.
    Copy { git: bool },
    /// Project files still point into the old folder, which nothing holds.
    Paths,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub installed: Installed,
    /// The addon folder is a git checkout or submodule, which git updates rather than the editor.
    pub git: bool,
    /// The addon is in [`legacy::ADDON_DIR`] and [`DIR`] is missing.
    pub legacy: bool,
    pub leftover: Option<Leftover>,
    /// Listed among the enabled plugins in project.godot.
    pub enabled: bool,
    /// The project has a trenchdot_game.json, so the addon ran in Godot at least once.
    pub exported: bool,
}

impl Status {
    pub fn check(root: &Path) -> Status {
        let dir = root.join(DIR);
        let legacy = td_formats::game::legacy_addon(root).is_some();
        let installed = match td_formats::game::addon_version(root) {
            Some(version) => installed(version, crate::VERSION),
            None if dir.exists() || legacy => Installed::Unknown,
            None => Installed::Missing,
        };
        let project = std::fs::read_to_string(root.join("project.godot")).unwrap_or_default();
        let old = root.join(legacy::ADDON_DIR);
        let leftover = match dir.exists() {
            false => None,
            true if legacy => Some(Leftover::Copy { git: old.join(".git").exists() }),
            true if !old.exists() && project.contains(OLD_RES) => Some(Leftover::Paths),
            true => None,
        };
        let moved = dir.exists() || !legacy;
        let git = if moved { dir.join(".git").exists() } else { old.join(".git").exists() };
        Status { installed, git, legacy: !moved, leftover, enabled: plugin_enabled(&project), exported: td_formats::GameConfig::discover(root).is_some() }
    }

    /// The addon is missing, does not match the editor or was moved out of the old folder only halfway.
    pub fn mismatched(&self) -> bool {
        self.installed != Installed::Current || self.leftover.is_some()
    }

    /// Names what a person dismissed: this editor's version and what the project had.
    pub fn dismiss_key(&self) -> String {
        let had = match &self.installed {
            Installed::Missing => "missing",
            Installed::Unknown => "unknown",
            Installed::Older(v) | Installed::Newer(v) => v,
            Installed::Current => "current",
        };
        let leftover = if self.leftover.is_some() { " leftover" } else { "" };
        format!("{} {had}{leftover}", crate::VERSION)
    }
}

/// How the addon `version` relates to the `editor` version. A beta editor downloads the newest beta, so a newer beta of
/// the same release counts as current, or the editor would ask to replace it with the very same download.
fn installed(version: String, editor: &str) -> Installed {
    fn beta_of(v: &str) -> Option<&str> {
        let (release, stamp) = v.trim().trim_start_matches('v').split_once("-beta.")?;
        stamp.split('.').next()?.parse::<u64>().ok().map(|_| release)
    }

    match compare(&version, editor) {
        Some(Ordering::Equal) => Installed::Current,
        Some(Ordering::Greater) if beta_of(&version).is_some_and(|r| beta_of(editor) == Some(r)) => Installed::Current,
        Some(Ordering::Greater) => Installed::Newer(version),
        _ => Installed::Older(version),
    }
}

/// The addon `version` does not match this editor.
pub fn mismatched(version: &str) -> bool {
    installed(version.to_string(), crate::VERSION) != Installed::Current
}

/// Whether opening the project should bring up the addon part of the setup wizard: the addon is missing or does not
/// match the editor, and nobody dismissed that for this project and version.
pub fn should_ask(root: &Path, dismissed: &BTreeMap<PathBuf, String>) -> bool {
    let status = Status::check(root);
    status.mismatched() && dismissed.get(root) != Some(&status.dismiss_key())
}

/// Compares versions like `0.1.0` or `0.2.0-beta.1` part by part as numbers, a pre-release before its release. None
/// when either is not such a version.
///
/// CI stamps beta builds as `0.1.0-beta.202611041532.d194b57`, the release they follow, the UTC commit time and the
/// commit. Stamps order by their time, and come after the release they follow and after any other pre-release of it.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    fn parse(v: &str) -> Option<(Vec<u64>, Option<&str>)> {
        let v = v.trim().trim_start_matches('v').split('+').next().unwrap_or_default();
        let (core, pre) = v.split_once('-').map_or((v, None), |(c, p)| (c, Some(p)));
        Some((core.split('.').map(|p| p.parse().ok()).collect::<Option<Vec<u64>>>()?, pre))
    }

    fn stamp(pre: Option<&str>) -> Option<u64> {
        pre?.strip_prefix("beta.")?.split('.').next()?.parse().ok()
    }

    let ((a, a_pre), (b, b_pre)) = (parse(a)?, parse(b)?);
    let part = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    let core = (0..a.len().max(b.len())).map(|i| part(&a, i).cmp(&part(&b, i))).find(|o| o.is_ne()).unwrap_or(Ordering::Equal);
    let pre = match (stamp(a_pre), stamp(b_pre)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => match (a_pre, b_pre) {
            (None, None) => Ordering::Equal,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(x), Some(y)) => x.cmp(y),
        },
    };
    Some(core.then(pre))
}

/// Each line of `text` with the byte offset it starts at, without its line ending.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut at = 0;
    text.split_inclusive('\n').map(move |line| {
        let start = at;
        at += line.len();
        (start, line.trim_end_matches(['\n', '\r']))
    })
}

/// Where project.godot enables plugins.
#[derive(Debug, PartialEq)]
enum Plugins {
    NoSection,
    /// `[editor_plugins]` without an `enabled` key. Where its header line ends, before the line break.
    NoKey(usize),
    /// Between the parentheses of `enabled=PackedStringArray(...)`.
    List(Range<usize>),
}

const UNREADABLE: &str = "The editor cannot edit the plugin list in project.godot. Enable TrenchDot in Godot under Project > Project Settings > Plugins.";

fn find_plugins(text: &str) -> Result<Plugins, String> {
    let mut header = None;
    let mut end = text.len();
    for (start, line) in lines(text) {
        let Some(name) = line.trim().strip_prefix('[').and_then(|l| l.split_once(']')).map(|(n, _)| n.trim()) else { continue };
        if header.is_some() {
            end = start;
            break;
        }

        if name == "editor_plugins" {
            header = Some(start + line.len());
        }
    }

    let Some(body) = header else { return Ok(Plugins::NoSection) };
    for (start, line) in lines(&text[..end]).filter(|(start, _)| *start > body) {
        let Some((key, _)) = line.split_once('=') else { continue };
        if key.trim() != "enabled" {
            continue;
        }

        let value = start + key.len() + 1;
        let lead = text[value..end].len() - text[value..end].trim_start().len();
        let open = value + lead + "PackedStringArray(".len();
        if !text[value + lead..end].starts_with("PackedStringArray(") {
            return Err(UNREADABLE.into());
        }

        let (mut quoted, mut escaped) = (false, false);
        for (i, c) in text[open..end].char_indices() {
            match c {
                _ if escaped => escaped = false,
                '\\' if quoted => escaped = true,
                '"' => quoted = !quoted,
                ')' if !quoted => return Ok(Plugins::List(open..open + i)),
                _ => {}
            }
        }

        return Err(UNREADABLE.into());
    }

    Ok(Plugins::NoKey(body))
}

/// Whether the text of a `project.godot` enables the addon.
pub fn plugin_enabled(project: &str) -> bool {
    matches!(find_plugins(project), Ok(Plugins::List(list)) if project[list.clone()].contains(&format!("\"{PLUGIN}\"")))
}

/// `project`, the text of a `project.godot`, with the addon added to its enabled plugins and every other byte as it
/// was. None when the addon is enabled already.
pub fn with_plugin_enabled(project: &str) -> Result<Option<String>, String> {
    let nl = if project.contains("\r\n") { "\r\n" } else { "\n" };
    let entry = format!("\"{PLUGIN}\"");
    let (at, insert) = match find_plugins(project)? {
        Plugins::List(list) if project[list.clone()].contains(&entry) => return Ok(None),
        Plugins::List(list) if project[list.clone()].trim().is_empty() => (list.end, entry),
        Plugins::List(list) => (list.end, format!(", {entry}")),
        Plugins::NoKey(header) => (header, format!("{nl}enabled=PackedStringArray({entry})")),
        Plugins::NoSection => {
            let gap = match project {
                "" => String::new(),
                p if p.ends_with('\n') => nl.to_string(),
                _ => format!("{nl}{nl}"),
            };
            (project.len(), format!("{gap}[editor_plugins]{nl}{nl}enabled=PackedStringArray({entry}){nl}"))
        }
    };

    Ok(Some(format!("{}{insert}{}", &project[..at], &project[at..])))
}

/// Enables the addon in the project's `project.godot`. Returns false when it was enabled already.
pub fn enable(root: &Path) -> Result<bool, String> {
    let path = root.join("project.godot");
    let text = std::fs::read(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let text = String::from_utf8(text).map_err(|_| UNREADABLE.to_string())?;
    let Some(changed) = with_plugin_enabled(&text)? else { return Ok(false) };
    td_formats::write_atomic(&path, &mut changed.as_bytes()).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(true)
}

/// What an install or update did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// What the project had before.
    pub before: Installed,
    /// The version the project has now.
    pub version: Option<String>,
    /// The release the addon came from.
    pub tag: Option<String>,
    /// Where the replaced addon went.
    pub backup: Option<PathBuf>,
    /// How many project files [`migrate`] pointed at the addon's new folder.
    pub migrated: usize,
    pub error: Option<Error>,
}

impl Outcome {
    pub fn message(&self) -> Msg {
        match &self.error {
            // The addon is unpacked beside the project and only moved in once complete, so nothing half done stays.
            Some(Error::Cancelled) => return msg!("Cancelled, the project is as it was."),
            Some(Error::DiskFull) => return msg!("The disk is full. Free some space and try again, the project is as it was."),
            Some(e) => return e.message(),
            None => {}
        }

        let version = self.version.as_deref().unwrap_or_default();
        if self.migrated > 0 {
            let count = self.migrated;
            return msg!("Moved the addon to res://addons/trenchdot, updated it to {version} and pointed {count} project files at it.", version, count);
        }

        if self.backup.is_none() {
            return msg!("Installed the addon {version}.", version);
        }

        let old = match &self.before {
            Installed::Older(v) | Installed::Newer(v) => msg!(" from {v}", v),
            _ => Msg::default(),
        };
        Msg::filled("Updated the addon{old} to {version}.", &[("old", &old.en, &old.shown), ("version", version, version)])
    }

    /// [`Outcome::message`] in English, for MCP and tests.
    pub fn summary(&self) -> String {
        self.message().en
    }
}

/// Downloads the addon from the release matching the editor, or the rolling beta, into the project at `root`. An addon
/// already there is replaced as a whole and kept in [`BACKUPS`], unless it is a git checkout.
pub fn install(root: &Path, api: &str, report: &Reporter) -> Outcome {
    let before = Status::check(root);
    let mut outcome = Outcome { before: before.installed.clone(), ..Default::default() };
    if before.git {
        outcome.error = Some(Error::Io(Msg::key(if before.legacy { LEGACY_GIT_MOVE } else { GIT })));
        return outcome;
    }

    let work = root.join(WORK_DIR);
    for left in std::fs::read_dir(&work).into_iter().flatten().flatten() {
        if left.file_name().to_string_lossy().starts_with(UNPACKING) {
            let _ = std::fs::remove_dir_all(left.path());
        }
    }

    static NEXT: AtomicU64 = AtomicU64::new(0);
    // Inside the project, so the addon moves into place with a rename rather than a copy.
    let unpacked = work.join(format!("{UNPACKING}{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let result = (|| {
        report.set(|p| *p = Progress { stage: msg!("Looking up {asset} in the releases", asset = ADDON.asset), ..Default::default() });
        let agent = crate::content::agent();
        let asset = crate::content::find_asset(&agent, api, ADDON.asset)?;
        let archive = crate::content::download(&agent, &asset, report)?;
        crate::content::extract(archive, ADDON.asset, &unpacked, ADDON.folders, report, &mut Added::default())?;
        let version = td_formats::game::addon_version(&unpacked).ok_or_else(|| Error::Damaged(format!("it has no {DIR}/plugin.cfg with a version")))?;
        report.check()?;
        let moving = std::fs::symlink_metadata(root.join(DIR)).is_err() && td_formats::game::legacy_addon(root).is_some();
        let backup = swap(root, &unpacked.join(DIR), &before.installed)?;
        let migrated = if moving { migrate(root)? } else { 0 };
        Ok((asset.tag, version, backup, migrated))
    })();
    let _ = std::fs::remove_dir_all(&unpacked);
    // Only goes when empty, that is when there are no backups.
    let _ = std::fs::remove_dir(&work);
    match result {
        Ok((tag, version, backup, migrated)) => {
            outcome.tag = Some(tag);
            outcome.version = Some(version);
            outcome.backup = backup;
            outcome.migrated = migrated;
        }
        Err(e) => outcome.error = Some(e),
    }

    outcome
}

/// Moves the unpacked addon `new` into place. An addon already there, or still in its old folder, moves to a backup
/// folder first, and back when the new one cannot take its place. Returns where the old one went.
fn swap(root: &Path, new: &Path, before: &Installed) -> Result<Option<PathBuf>, Error> {
    let target = root.join(DIR);
    let old = if std::fs::symlink_metadata(&target).is_err() && td_formats::game::legacy_addon(root).is_some() {
        root.join(legacy::ADDON_DIR)
    } else {
        target.clone()
    };
    if std::fs::symlink_metadata(&old).is_err() {
        let addons = root.join("addons");
        std::fs::create_dir_all(&addons).map_err(|e| io_error(e, &addons))?;
        std::fs::rename(new, &target).map_err(|e| io_error(e, &target))?;
        return Ok(None);
    }

    if old.join(".git").exists() {
        return Err(Error::Io(Msg::key(if old == target { GIT } else { LEGACY_GIT_MOVE })));
    }

    let version = match before {
        Installed::Older(v) | Installed::Newer(v) => v.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect(),
        Installed::Current => crate::VERSION.to_string(),
        _ => "unknown".to_string(),
    };
    let backup = backup_dir(root, &format!("trenchdot-{version}"))?;
    std::fs::rename(&old, &backup).map_err(|e| Error::Io(msg!("Could not move the old addon ({e}). Close Godot and try again.", e)))?;
    if let Err(e) = std::fs::rename(new, &target) {
        let back = std::fs::rename(&backup, &old);
        return Err(Error::Io(match back {
            Ok(()) => msg!("Could not put the new addon in place ({e}), the old one is back where it was.", e),
            Err(_) => msg!(
                "Could not put the new addon in place ({e}). The old one is in {backup}, move it back to {target}.",
                e,
                backup = backup.display(),
                target = old.display()
            ),
        }));
    }

    Ok(Some(backup))
}

/// A free folder in [`BACKUPS`] named after `name`.
fn backup_dir(root: &Path, name: &str) -> Result<PathBuf, Error> {
    let backups = root.join(BACKUPS);
    std::fs::create_dir_all(&backups).map_err(|e| io_error(e, &backups))?;
    // In case Godot ever looks into hidden folders, and so a project in git does not pick the backups up.
    for (name, text) in [(".gdignore", ""), (".gitignore", "*\n")] {
        let path = backups.join(name);
        if !path.exists() {
            std::fs::write(&path, text).map_err(|e| io_error(e, &path))?;
        }
    }

    Ok((1..)
        .map(|n| backups.join(if n == 1 { name.to_string() } else { format!("{name}-{n}") }))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .expect("a free backup name"))
}

/// Finishes a move into [`DIR`] done without the editor: an older copy left in [`legacy::ADDON_DIR`] goes to the
/// backups, then [`migrate`] points the project at the new folder. Returns how many files changed and the backup.
pub fn finish_move(root: &Path) -> Result<(usize, Option<PathBuf>), Error> {
    let old = root.join(legacy::ADDON_DIR);
    let mut backup = None;
    if root.join(DIR).exists() && td_formats::game::legacy_addon(root).is_some() {
        if old.join(".git").exists() {
            return Err(Error::Io(Msg::key(LEGACY_GIT)));
        }

        let to = backup_dir(root, "func_godot")?;
        std::fs::rename(&old, &to).map_err(|e| Error::Io(msg!("Could not move the old addon ({e}). Close Godot and try again.", e)))?;
        backup = Some(to);
    }

    Ok((migrate(root)?, backup))
}

const OLD_RES: &str = "res://addons/func_godot/";
const NEW_RES: &str = "res://addons/trenchdot/";
/// The project settings the addon kept in [`legacy::SETTINGS_SECTION`].
const MOVED_SETTINGS: [&str; 2] = ["default_map_settings", "default_inverse_scale_factor"];

/// Points a project whose addon was in [`legacy::ADDON_DIR`] at [`DIR`]: its scenes, resources and scripts, the plugin
/// list and the addon's project settings. The addon's files have other uids than before the move, so references into
/// it lose theirs and Godot finds them by path, writing the new uids the next time it saves them. Folders starting
/// with a dot and the other addons stay as they are. Returns how many files changed.
pub fn migrate(root: &Path) -> Result<usize, Error> {
    let mut changed = 0;
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|e| io_error(e, &dir))?.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if !name.starts_with('.') && path != root.join("addons") {
                    dirs.push(path);
                }

                continue;
            }

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
            let project = dir == root && name == "project.godot";
            if !project && !["tscn", "tres", "gd", "gdshader", "cs", "cfg", "json"].contains(&ext) {
                continue;
            }

            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let mut new = repointed(&text, ext == "tscn" || ext == "tres");
            if project {
                new = MOVED_SETTINGS.iter().fold(new, |t, key| crate::godot_settings::moved_setting(&t, legacy::SETTINGS_SECTION, "trenchdot", key));
            }

            if new != text {
                td_formats::write_atomic(&path, &mut new.as_bytes()).map_err(|e| io_error(e, &path))?;
                changed += 1;
            }
        }
    }

    Ok(changed)
}

/// `text` with its paths into the old addon folder moved to the new one. In a scene or resource those references also
/// drop their uid, which named the file before the move.
fn repointed(text: &str, resource: bool) -> String {
    if !text.contains(OLD_RES) {
        return text.to_string();
    }

    let text = text.replace(OLD_RES, NEW_RES);
    if !resource {
        return text;
    }

    let into_addon = format!("path=\"{NEW_RES}");
    text.split_inclusive('\n')
        .map(|line| match line.find(" uid=\"") {
            Some(at) if line.starts_with("[ext_resource") && line.contains(&into_addon) => {
                let end = line[at + 6..].find('"').map_or(line.len(), |e| at + 7 + e);
                format!("{}{}", &line[..at], &line[end..])
            }
            _ => line.to_string(),
        })
        .collect()
}

/// An install running on its own thread.
pub struct Job {
    report: Reporter,
    rx: mpsc::Receiver<Outcome>,
}

impl Job {
    pub fn start(root: PathBuf, api: String, repaint: Option<egui::Context>) -> Job {
        let report = Reporter::new(repaint.clone());
        let thread_report = report.clone();
        let (tx, rx) = mpsc::channel();
        let lang = i18n::current();
        std::thread::spawn(move || {
            let _lang = i18n::scope(lang);
            let _ = tx.send(install(&root, &api, &thread_report));
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
        });
        Job { report, rx }
    }

    pub fn progress(&self) -> Progress {
        self.report.progress()
    }

    pub fn cancel(&self) {
        self.report.cancel();
    }

    pub fn cancelling(&self) -> bool {
        self.report.cancelled()
    }

    pub fn poll(&self) -> Option<Outcome> {
        match self.rx.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Outcome { error: Some(Error::Io(msg!("The install stopped unexpectedly."))), ..Default::default() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::test_server::{Server, zip};

    fn project(name: &str, godot: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("td_addon_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("project.godot"), godot).unwrap();
        dir
    }

    fn addon(root: &Path, version: &str) {
        std::fs::create_dir_all(root.join(DIR)).unwrap();
        std::fs::write(root.join(DIR).join("plugin.cfg"), format!("[plugin]\n\nname=\"TrenchDot\"\nversion=\"{version}\"\n")).unwrap();
    }

    fn release_zip(version: &str) -> Vec<u8> {
        zip(&[
            ("addons/trenchdot/plugin.cfg", format!("[plugin]\nname=\"TrenchDot\"\nversion=\"{version}\"\n").as_bytes()),
            ("addons/trenchdot/src/func_godot_plugin.gd", b"@tool\nextends EditorPlugin\n"),
        ])
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(compare("0.1.0", "0.1.0"), Some(Ordering::Equal));
        assert_eq!(compare("0.1", "0.1.0"), Some(Ordering::Equal));
        assert_eq!(compare("0.10.0", "0.9.0"), Some(Ordering::Greater));
        assert_eq!(compare("0.1.0-beta", "0.1.0"), Some(Ordering::Less));
        assert_eq!(compare("v1.0.0", "0.9.9"), Some(Ordering::Greater));
        assert_eq!(compare("dev", "0.1.0"), None);
    }

    #[test]
    fn beta_stamps_order_by_their_time() {
        let (older, newer) = ("0.1.0-beta.202611031000.aaaaaaa", "0.1.0-beta.202611041532.d194b57");
        assert_eq!(compare(older, newer), Some(Ordering::Less));
        assert_eq!(compare(newer, older), Some(Ordering::Greater));
        assert_eq!(compare(newer, "0.1.0-beta.202611041532.d194b57"), Some(Ordering::Equal));
        assert_eq!(compare("0.1.0", newer), Some(Ordering::Less), "an addon from before the stamps is older than a stamped editor");
        assert_eq!(compare(newer, "0.1.0"), Some(Ordering::Greater), "a beta comes after the release whose number it carries");
        assert_eq!(compare("0.1.0-beta", newer), Some(Ordering::Less));
        assert_eq!(compare("0.2.0", newer), Some(Ordering::Greater), "the release number counts first");

        assert_eq!(installed(newer.into(), older), Installed::Current, "a beta editor downloads the newest beta");
        assert_eq!(installed(older.into(), newer), Installed::Older(older.into()));
        assert_eq!(installed(newer.into(), "0.1.0"), Installed::Newer(newer.into()), "a release editor downloads its release");
        assert_eq!(installed("0.2.0-beta.202611031000.aaaaaaa".into(), older), Installed::Newer("0.2.0-beta.202611031000.aaaaaaa".into()));
        assert!(!mismatched(crate::VERSION) && mismatched("0.0.1"));
    }

    #[test]
    fn detects_what_the_project_has() {
        let root = project("detect", "config_version=5\n");
        let status = Status::check(&root);
        assert_eq!(status, Status { installed: Installed::Missing, git: false, legacy: false, leftover: None, enabled: false, exported: false });
        assert!(should_ask(&root, &BTreeMap::new()));
        assert!(!should_ask(&root, &BTreeMap::from([(root.clone(), status.dismiss_key())])), "dismissed for this version");
        assert!(should_ask(&root, &BTreeMap::from([(root.clone(), "0.0.1 missing".to_string())])), "a new editor asks again");

        addon(&root, "0.0.1");
        assert_eq!(Status::check(&root).installed, Installed::Older("0.0.1".into()));
        assert!(should_ask(&root, &BTreeMap::from([(root.clone(), status.dismiss_key())])), "another addon asks again");
        addon(&root, "999.0.0");
        assert_eq!(Status::check(&root).installed, Installed::Newer("999.0.0".into()));
        std::fs::write(root.join(DIR).join("plugin.cfg"), "[plugin]\nname=\"Something\"\n").unwrap();
        assert_eq!(Status::check(&root).installed, Installed::Unknown);

        // The addon of an install from before the rename has the editor's own version number and is still older.
        for (version, name) in [crate::VERSION, "0.0.1", "999.0.0"].into_iter().flat_map(|v| td_doc::legacy::ADDON_NAMES.map(|n| (v, n))) {
            let cfg = format!("[plugin]\nname=\"{name}\"\nversion=\"{version}\"\n");
            std::fs::write(root.join(DIR).join("plugin.cfg"), cfg).unwrap();
            let old = format!("{version} ({name})");
            assert_eq!(Status::check(&root).installed, Installed::Older(old.clone()));
            assert!(mismatched(&old) && should_ask(&root, &BTreeMap::new()));
        }

        addon(&root, crate::VERSION);
        std::fs::write(root.join(DIR).join(".git"), "gitdir: ../../.git/modules/func_godot\n").unwrap();
        std::fs::write(root.join("project.godot"), format!("[editor_plugins]\n\nenabled=PackedStringArray(\"{PLUGIN}\")\n")).unwrap();
        std::fs::write(root.join(td_formats::game::GAME_FILE_NAME), "{}").unwrap();
        let status = Status::check(&root);
        assert_eq!(status, Status { installed: Installed::Current, git: true, legacy: false, leftover: None, enabled: true, exported: true });
        assert!(!status.mismatched() && !should_ask(&root, &BTreeMap::new()));
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn enabled(text: &str) -> String {
        with_plugin_enabled(text).unwrap().expect("a change")
    }

    #[test]
    fn enabling_changes_only_the_plugin_list() {
        let godot = "; Engine configuration file.\n\nconfig_version=5\n\n[application]\n\nconfig/name=\"My Game\"\n";
        let added = format!("{godot}\n[editor_plugins]\n\nenabled=PackedStringArray(\"{PLUGIN}\")\n");
        assert_eq!(enabled(godot), added);
        assert!(plugin_enabled(&added) && !plugin_enabled(godot));
        assert_eq!(with_plugin_enabled(&added), Ok(None), "enabled already");
        assert_eq!(enabled("config_version=5"), format!("config_version=5\n\n[editor_plugins]\n\nenabled=PackedStringArray(\"{PLUGIN}\")\n"));

        let crlf = godot.replace('\n', "\r\n");
        assert_eq!(enabled(&crlf), added.replace('\n', "\r\n"), "CRLF files stay CRLF");

        let other = "[editor_plugins]\r\n\r\nenabled=PackedStringArray(\"res://addons/other/plugin.cfg\")\r\n\r\n[rendering]\r\n\r\nx=1\r\n";
        assert_eq!(
            enabled(other),
            format!("[editor_plugins]\r\n\r\nenabled=PackedStringArray(\"res://addons/other/plugin.cfg\", \"{PLUGIN}\")\r\n\r\n[rendering]\r\n\r\nx=1\r\n")
        );
        let empty = "[editor_plugins]\n\nenabled=PackedStringArray()\n";
        assert_eq!(enabled(empty), format!("[editor_plugins]\n\nenabled=PackedStringArray(\"{PLUGIN}\")\n"));

        // A tricky name, a list over two lines and the key of another section do not throw it off.
        let tricky = "[editor_plugins]\n\nenabled=PackedStringArray(\"res://addons/a)b/plugin.cfg\",\n\"res://addons/c/plugin.cfg\")\n[z]\nenabled=1\n";
        assert_eq!(
            enabled(tricky),
            format!(
                "[editor_plugins]\n\nenabled=PackedStringArray(\"res://addons/a)b/plugin.cfg\",\n\"res://addons/c/plugin.cfg\", \"{PLUGIN}\")\n[z]\nenabled=1\n"
            )
        );
        let no_key = "[editor_plugins]\n\nsomething=1\n\n[z]\n";
        assert_eq!(enabled(no_key), format!("[editor_plugins]\nenabled=PackedStringArray(\"{PLUGIN}\")\n\nsomething=1\n\n[z]\n"));
        assert!(!plugin_enabled("[other]\nenabled=PackedStringArray(\"res://addons/trenchdot/plugin.cfg\")\n"), "only in [editor_plugins]");

        assert_eq!(with_plugin_enabled("[editor_plugins]\nenabled=PoolStringArray(\"x\")\n"), Err(UNREADABLE.to_string()));
        assert_eq!(with_plugin_enabled("[editor_plugins]\nenabled=PackedStringArray(\"x\"\n"), Err(UNREADABLE.to_string()));
    }

    #[test]
    fn enable_writes_project_godot_byte_for_byte() {
        let godot = "; Engine configuration file.\r\n\r\nconfig_version=5\r\n\r\n[application]\r\n\r\nconfig/name=\"Über Spiel\"\r\n";
        let root = project("enable", godot);
        assert_eq!(enable(&root), Ok(true));
        let written = std::fs::read(root.join("project.godot")).unwrap();
        let expected = format!("{godot}\r\n[editor_plugins]\r\n\r\nenabled=PackedStringArray(\"{PLUGIN}\")\r\n");
        assert_eq!(written, expected.as_bytes());
        assert_eq!(enable(&root), Ok(false));
        assert_eq!(std::fs::read(root.join("project.godot")).unwrap(), expected.as_bytes());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1, "no temporary file stays");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn installs_a_missing_addon_from_the_release() {
        let server = Server::start();
        server.release(&format!("v{}", crate::VERSION), &[(ADDON.asset, &release_zip(crate::VERSION))], true);
        let root = project("install", "config_version=5\n");
        std::fs::create_dir_all(root.join(WORK_DIR).join("addon-new-1-0/addons")).unwrap();
        let outcome = install(&root, &server.url, &Reporter::default());
        assert_eq!(outcome.error, None);
        assert_eq!((outcome.version.as_deref(), outcome.backup.as_ref()), (Some(crate::VERSION), None));
        assert_eq!(Status::check(&root).installed, Installed::Current);
        assert!(root.join(DIR).join("src/func_godot_plugin.gd").is_file());
        assert!(!root.join(WORK_DIR).exists(), "nothing is left beside the addon, not even what a crash left");
        assert!(outcome.summary().starts_with("Installed the addon"), "{}", outcome.summary());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_update_replaces_the_folder_and_keeps_the_old_one() {
        let server = Server::start();
        server.release("beta", &[(ADDON.asset, &release_zip(crate::VERSION))], true);
        let root = project("update", "config_version=5\n");
        addon(&root, "0.0.1");
        std::fs::write(root.join(DIR).join("removed_since.gd"), "old").unwrap();
        let outcome = install(&root, &server.url, &Reporter::default());
        assert_eq!(outcome.error, None);
        let backup = root.join(BACKUPS).join("trenchdot-0.0.1");
        assert_eq!(outcome.backup.as_ref(), Some(&backup));
        assert!(!root.join(DIR).join("removed_since.gd").exists(), "the folder is replaced as a whole");
        assert_eq!(std::fs::read_to_string(backup.join("removed_since.gd")).unwrap(), "old");
        assert!(root.join(BACKUPS).join(".gdignore").is_file() && root.join(BACKUPS).join(".gitignore").is_file());
        assert_eq!(Status::check(&root).installed, Installed::Current);
        let summary = outcome.summary();
        assert!(summary.starts_with("Updated the addon from 0.0.1 to"), "{summary}");
        assert_eq!(std::fs::read_dir(root.join(WORK_DIR)).unwrap().count(), 1, "only the backups stay");

        addon(&root, "0.0.1");
        assert_eq!(install(&root, &server.url, &Reporter::default()).backup, Some(root.join(BACKUPS).join("trenchdot-0.0.1-2")), "an older backup stays");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_addon_in_the_old_folder_moves_and_the_project_follows() {
        let server = Server::start();
        server.release("beta", &[(ADDON.asset, &release_zip(crate::VERSION))], true);
        let godot = "config_version=5\n\n[editor_plugins]\n\nenabled=PackedStringArray(\"res://addons/func_godot/plugin.cfg\")\n\n[func_godot]\n\ndefault_map_settings=\"res://ms.tres\"\ndefault_inverse_scale_factor=16.0\n";
        let root = project("move", godot);
        let old = root.join(legacy::ADDON_DIR);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("plugin.cfg"), "[plugin]\n\nname=\"TrenchDot\"\nversion=\"0.0.1\"\n").unwrap();
        assert!(matches!(Status::check(&root).installed, Installed::Older(_)), "the old folder counts as an older addon");
        let scene = "[gd_scene format=3 uid=\"uid://scene\"]\n\n[ext_resource type=\"Script\" uid=\"uid://old\" path=\"res://addons/func_godot/src/map/func_godot_map.gd\" id=\"1\"]\n[ext_resource type=\"Script\" uid=\"uid://mine\" path=\"res://mine.gd\" id=\"2\"]\n";
        std::fs::create_dir_all(root.join("levels")).unwrap();
        std::fs::write(root.join("levels/level.tscn"), scene).unwrap();
        std::fs::write(root.join("mine.gd"), "extends \"res://addons/func_godot/src/map/func_godot_map.gd\"\n").unwrap();
        std::fs::create_dir_all(root.join("addons/other")).unwrap();
        std::fs::write(root.join("addons/other/x.gd"), "# res://addons/func_godot/\n").unwrap();

        let outcome = install(&root, &server.url, &Reporter::default());
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.migrated, 3, "the scene, the script and project.godot");
        assert!(outcome.summary().starts_with("Moved the addon to res://addons/trenchdot"), "{}", outcome.summary());
        assert!(!old.exists() && outcome.backup.as_ref().is_some_and(|b| b.join("plugin.cfg").is_file()), "the old folder went to the backups");
        assert_eq!(Status::check(&root).installed, Installed::Current);
        assert_eq!(
            std::fs::read_to_string(root.join("levels/level.tscn")).unwrap(),
            scene.replace("uid=\"uid://old\" path=\"res://addons/func_godot/", "path=\"res://addons/trenchdot/"),
            "references into the addon follow it by path, others keep their uid"
        );
        assert_eq!(std::fs::read_to_string(root.join("mine.gd")).unwrap(), "extends \"res://addons/trenchdot/src/map/func_godot_map.gd\"\n");
        assert_eq!(std::fs::read_to_string(root.join("addons/other/x.gd")).unwrap(), "# res://addons/func_godot/\n", "other addons stay as they are");
        let project = std::fs::read_to_string(root.join("project.godot")).unwrap();
        assert!(plugin_enabled(&project), "{project}");
        assert_eq!(crate::godot_settings::get(&project, "trenchdot", "default_map_settings").as_deref(), Some("res://ms.tres"));
        assert_eq!(crate::godot_settings::get(&project, "trenchdot", "default_inverse_scale_factor").as_deref(), Some("16.0"));
        assert_eq!(crate::godot_settings::get(&project, legacy::SETTINGS_SECTION, "default_map_settings"), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_original_func_godot_addon_is_left_alone() {
        let server = Server::start();
        server.release("beta", &[(ADDON.asset, &release_zip(crate::VERSION))], true);
        let godot = "config_version=5\n\n[func_godot]\n\ndefault_map_settings=\"res://addons/func_godot/func_godot_default_map_settings.tres\"\n";
        let root = project("upstream", godot);
        let upstream = root.join(legacy::ADDON_DIR);
        std::fs::create_dir_all(&upstream).unwrap();
        std::fs::write(upstream.join("plugin.cfg"), "[plugin]\n\nname=\"FuncGodot\"\nversion=\"2025.1\"\n").unwrap();
        std::fs::write(
            root.join("level.tscn"),
            "[ext_resource type=\"Script\" uid=\"uid://up\" path=\"res://addons/func_godot/src/map/func_godot_map.gd\" id=\"1\"]\n",
        )
        .unwrap();
        assert_eq!(Status::check(&root).installed, Installed::Missing);

        let outcome = install(&root, &server.url, &Reporter::default());
        assert_eq!((outcome.error, outcome.backup, outcome.migrated), (None, None, 0));
        assert!(upstream.join("plugin.cfg").is_file() && root.join(DIR).join("plugin.cfg").is_file(), "both addons sit side by side");
        assert_eq!(std::fs::read_to_string(root.join("project.godot")).unwrap(), godot);
        assert!(std::fs::read_to_string(root.join("level.tscn")).unwrap().contains("uid://up"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_move_done_without_the_editor_is_finished() {
        let godot = "config_version=5\n\n[editor_plugins]\n\nenabled=PackedStringArray(\"res://addons/func_godot/plugin.cfg\")\n\n[func_godot]\n\ndefault_map_settings=\"res://ms.tres\"\n";
        let root = project("by_hand", godot);
        addon(&root, crate::VERSION);
        std::fs::write(
            root.join("level.tscn"),
            "[ext_resource type=\"Script\" uid=\"uid://old\" path=\"res://addons/func_godot/src/map/func_godot_map.gd\" id=\"1\"]\n",
        )
        .unwrap();
        let status = Status::check(&root);
        assert_eq!((status.installed.clone(), status.leftover), (Installed::Current, Some(Leftover::Paths)));
        assert!(status.mismatched() && should_ask(&root, &BTreeMap::new()));

        let old = root.join(legacy::ADDON_DIR);
        std::fs::create_dir_all(old.join(".git")).unwrap();
        std::fs::write(old.join("plugin.cfg"), "[plugin]\n\nname=\"TrenchDot\"\nversion=\"0.0.1\"\n").unwrap();
        assert_eq!(Status::check(&root).leftover, Some(Leftover::Copy { git: true }));
        assert!(matches!(finish_move(&root), Err(Error::Io(why)) if why.en.contains("git")));
        assert!(old.join("plugin.cfg").is_file() && root.join("level.tscn").is_file());

        std::fs::remove_dir_all(old.join(".git")).unwrap();
        assert_eq!(Status::check(&root).leftover, Some(Leftover::Copy { git: false }));
        let (changed, backup) = finish_move(&root).unwrap();
        assert_eq!(changed, 2, "the scene and project.godot");
        assert!(!old.exists() && backup.is_some_and(|b| b.join("plugin.cfg").is_file()), "the old copy went to the backups");
        assert_eq!(
            std::fs::read_to_string(root.join("level.tscn")).unwrap(),
            "[ext_resource type=\"Script\" path=\"res://addons/trenchdot/src/map/func_godot_map.gd\" id=\"1\"]\n"
        );
        let project = std::fs::read_to_string(root.join("project.godot")).unwrap();
        assert!(plugin_enabled(&project) && !project.contains(OLD_RES), "{project}");
        assert_eq!(crate::godot_settings::get(&project, "trenchdot", "default_map_settings").as_deref(), Some("res://ms.tres"));
        let status = Status::check(&root);
        assert_eq!(status.leftover, None);
        assert!(!status.mismatched());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_addon_in_the_old_folder_under_git_is_left_to_git() {
        let server = Server::start();
        server.release("beta", &[(ADDON.asset, &release_zip(crate::VERSION))], true);
        let root = project("legacy_git", "config_version=5\n");
        let old = root.join(legacy::ADDON_DIR);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("plugin.cfg"), "[plugin]\n\nname=\"TrenchDot\"\nversion=\"0.0.1\"\n").unwrap();
        std::fs::write(old.join(".git"), "gitdir: ../../.git/modules/func_godot\n").unwrap();
        let status = Status::check(&root);
        assert!(status.git && status.legacy && status.leftover.is_none(), "{status:?}");
        let outcome = install(&root, &server.url, &Reporter::default());
        assert!(matches!(&outcome.error, Some(Error::Io(why)) if why.en.contains("move it to res://addons/trenchdot")), "{outcome:?}");
        assert!(old.join("plugin.cfg").is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_git_checkout_is_never_replaced() {
        let server = Server::start();
        server.release("beta", &[(ADDON.asset, &release_zip(crate::VERSION))], true);
        let root = project("git", "config_version=5\n");
        addon(&root, "0.0.1");
        std::fs::create_dir_all(root.join(DIR).join(".git")).unwrap();
        let outcome = install(&root, &server.url, &Reporter::default());
        assert!(matches!(&outcome.error, Some(Error::Io(why)) if why.en.contains("git")), "{outcome:?}");
        assert!(server.requests.lock().unwrap().is_empty(), "nothing is downloaded");
        assert_eq!(Status::check(&root).installed, Installed::Older("0.0.1".into()));
        assert!(!root.join(WORK_DIR).exists());

        std::fs::remove_dir_all(root.join(DIR).join(".git")).unwrap();
        let unpacked = root.join("new");
        std::fs::create_dir_all(unpacked.join(DIR)).unwrap();
        std::fs::create_dir_all(root.join(DIR).join(".git")).unwrap();
        assert!(matches!(swap(&root, &unpacked.join(DIR), &Installed::Older("0.0.1".into())), Err(Error::Io(_))), "checked again right before the swap");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn failed_downloads_leave_the_project_alone() {
        let server = Server::start();
        let root = project("failed", "config_version=5\n");
        addon(&root, "0.0.1");
        assert_eq!(install(&root, &server.url, &Reporter::default()).error, Some(Error::NoRelease));
        server.release("beta", &[], true);
        assert_eq!(install(&root, &server.url, &Reporter::default()).error, Some(Error::NotPublished(ADDON.asset)));

        server.release("beta", &[(ADDON.asset, &zip(&[("addons/trenchdot/readme.txt", b"no plugin.cfg")]))], true);
        assert!(matches!(install(&root, &server.url, &Reporter::default()).error, Some(Error::Damaged(_))));
        server.release("beta", &[(ADDON.asset, &zip(&[("addons/trenchdot/plugin.cfg", b"x"), ("project.godot", b"x")]))], true);
        assert!(matches!(install(&root, &server.url, &Reporter::default()).error, Some(Error::Unsafe(_))));

        let cancelled = Reporter::default();
        cancelled.cancel();
        server.release("beta", &[(ADDON.asset, &release_zip(crate::VERSION))], true);
        let outcome = install(&root, &server.url, &cancelled);
        assert_eq!(outcome.error, Some(Error::Cancelled));
        assert_eq!(outcome.summary(), "Cancelled, the project is as it was.");

        assert_eq!(Status::check(&root).installed, Installed::Older("0.0.1".into()));
        assert_eq!(std::fs::read(root.join("project.godot")).unwrap(), b"config_version=5\n");
        assert!(!root.join(WORK_DIR).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_release_zip_holds_the_addon_folder() {
        let ci = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/ci.yml")).unwrap();
        let line = ci.lines().find(|l| l.contains(&format!("{}\"", ADDON.asset)) && l.contains("zip -qr")).expect("ci.yml zips the addon");
        assert!(line.contains(&format!("cd godot && zip -qr \"../dist/{}\" {DIR} ", ADDON.asset)), "{line}");
        assert_eq!(ADDON.folders, [DIR]);
    }

    #[test]
    fn a_worker_words_its_stage_and_outcome_in_the_language_it_started_under() {
        let closed = std::net::SocketAddr::from(([127, 0, 0, 1], crate::test_ports::CLOSED));
        for (lang, stage, error) in [(i18n::Lang::De, "Suche ", "GitHub ist nicht erreichbar"), (i18n::Lang::En, "Looking up ", "Could not reach GitHub")] {
            let _lang = i18n::scope(lang);
            let root = project(&format!("lang_{}", lang.code()), "config_version=5\n");
            let job = Job::start(root.clone(), format!("http://{closed}"), None);
            let outcome = loop {
                if let Some(o) = job.poll() {
                    break o;
                }

                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            let progress = job.progress().stage;
            assert!(progress.en.starts_with("Looking up ") && progress.shown.starts_with(stage), "{lang:?}: {progress:?}");
            let message = outcome.message();
            assert!(message.en.starts_with("Could not reach GitHub"), "{message:?}");
            assert!(message.shown.starts_with(error), "{lang:?}: {message:?}");
            assert!(outcome.summary().starts_with("Could not reach GitHub"));
            std::fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn an_outcome_words_a_new_install_and_an_update_per_language() {
        let _lang = i18n::scope(i18n::Lang::De);
        let fresh = Outcome { version: Some("1.2.0".into()), tag: Some("beta".into()), ..Default::default() }.message();
        assert_eq!(fresh.en, "Installed the addon 1.2.0.");
        assert_eq!(fresh.shown, "Addon 1.2.0 installiert.");
        let updated = Outcome {
            before: Installed::Older("1.0.0".into()),
            version: Some("1.2.0".into()),
            backup: Some("/project/backup/func_godot".into()),
            ..Default::default()
        }
        .message();
        assert_eq!(updated.en, "Updated the addon from 1.0.0 to 1.2.0.");
        assert_eq!(updated.shown, "Addon von 1.0.0 auf 1.2.0 aktualisiert.");
    }

    #[test]
    fn a_job_installs_on_its_own_thread() {
        let server = Server::start();
        server.release("beta", &[(ADDON.asset, &release_zip(crate::VERSION))], false);
        let root = project("job", "config_version=5\n");
        let job = Job::start(root.clone(), server.url.clone(), None);
        let outcome = loop {
            if let Some(o) = job.poll() {
                break o;
            }

            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!((outcome.error, outcome.tag.as_deref()), (None, Some("beta")));
        assert_eq!(job.progress().done, 2, "both files were counted");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
