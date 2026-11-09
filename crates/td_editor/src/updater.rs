//! Updates the editor from the rolling beta release. It looks for a newer build at most once a day, downloads the
//! executable for this platform when asked, and swaps the executable, keeping the replaced one beside it to roll back to.

use std::cmp::Ordering;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{OnceLock, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{RichText, Ui};

use crate::content::{self, Asset, Error, Reporter, io_error, megabytes};
use crate::i18n::{self, Msg, en, msg, tr, trf};
use crate::state::{EditorState, Prefs};
use crate::theme;

pub const HEADING: &str = en!("Editor Update");

/// The executable CI builds for this platform, see the package job in ci.yml.
pub const ASSET: Option<&str> = if cfg!(all(windows, target_arch = "x86_64")) {
    Some("trenchdot-windows-x86_64.exe")
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some("trenchdot-linux-x86_64")
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    Some("trenchdot-macos-arm64")
} else {
    None
};

/// How an executable for this platform starts, PE, ELF or 64 bit Mach-O.
const MAGIC: &[u8] = if cfg!(windows) {
    b"MZ"
} else if cfg!(target_os = "macos") {
    &[0xcf, 0xfa, 0xed, 0xfe]
} else {
    b"ELF"
};
const TAG: &str = "beta";
const DAY: u64 = 24 * 3600;
/// A download this big is not the editor.
const MAX_EXE: u64 = 1 << 30;

static RESTART: AtomicBool = AtomicBool::new(false);

/// The executable the editor started from. Read once at startup, since after an update the running process reports the
/// path its file was renamed to.
pub fn exe() -> Option<&'static Path> {
    static EXE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    EXE_PATH.get_or_init(|| std::env::current_exe().ok()).as_deref()
}

/// Why the editor cannot update itself.
#[derive(Clone, Debug, PartialEq)]
pub enum Skip {
    Platform,
    /// Built from source, so it carries no build stamp to compare.
    LocalBuild,
    /// A Flatpak, Snap, AppImage or system install, which its own package manager keeps current.
    Packaged,
    ReadOnly(PathBuf),
}

impl Skip {
    pub fn message(&self) -> Msg {
        match self {
            Skip::Platform => msg!("There is no ready made build for this platform, so the editor does not update itself."),
            Skip::LocalBuild => msg!("This editor was built from source, so it does not update itself. Pull and build it again instead."),
            Skip::Packaged => msg!("This editor came from a package manager, which keeps it up to date."),
            Skip::ReadOnly(dir) => msg!("The editor cannot write to {dir}, so it cannot update itself. Download the new build yourself.", dir = dir.display()),
        }
    }
}

/// Why the editor at `exe` carrying `version` cannot update itself, None when it can. `env` reads an environment
/// variable.
pub fn skip(version: &str, exe: &Path, env: impl Fn(&str) -> Option<String>) -> Option<Skip> {
    if ASSET.is_none() {
        return Some(Skip::Platform);
    }

    // CI stamps every build it publishes, see `stamp` in ci.yml.
    if !version.contains("-beta.") {
        return Some(Skip::LocalBuild);
    }

    let set = |name: &str| env(name).is_some_and(|v| !v.is_empty());
    let system = cfg!(unix) && ["/usr", "/nix", "/opt/homebrew", "/snap"].iter().any(|p| exe.starts_with(p));
    if system || set("FLATPAK_ID") || set("SNAP") || set("APPIMAGE") {
        return Some(Skip::Packaged);
    }

    let dir = exe.parent().unwrap_or(Path::new("."));
    if tempfile::tempfile_in(dir).is_err() {
        return Some(Skip::ReadOnly(dir.to_path_buf()));
    }

    None
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// A build newer than the running one.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub asset: Asset,
}

/// The build stamp in a beta release's title, like `TrenchDot 0.2.0-beta.202611071200.abc1234`.
fn stamp(title: &str) -> Option<&str> {
    title.split_whitespace().find(|w| w.contains("-beta.") && crate::addon_install::compare(w, w).is_some())
}

/// The rolling beta, when its build is newer than `current`.
pub fn check(api: &str, current: &str) -> Result<Option<Release>, Error> {
    let Some(name) = ASSET else { return Ok(None) };
    let agent = content::agent();
    let release = content::release(&agent, api, TAG)?.ok_or(Error::NoRelease)?;
    // A beta published before CI put the stamp in its title cannot be compared.
    let Some(version) = release["name"].as_str().and_then(stamp) else { return Ok(None) };
    if crate::addon_install::compare(version, current) != Some(Ordering::Greater) {
        return Ok(None);
    }

    let asset = content::release_asset(&release, api, name, TAG)?.ok_or(Error::NotPublished(name))?;
    Ok(Some(Release { version: version.to_string(), asset }))
}

/// Where the replaced build stays, `trenchdot-windows-x86_64-previous.exe` beside `trenchdot-windows-x86_64.exe`.
pub fn previous(exe: &Path) -> PathBuf {
    let stem = exe.file_stem().unwrap_or_default().to_string_lossy();
    let ext = exe.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    exe.with_file_name(format!("{stem}-previous{ext}"))
}

/// Downloads `release` and puts its executable in place of `exe`. Returns where the replaced one went.
pub fn install(release: &Release, exe: &Path, report: &Reporter) -> Result<PathBuf, Error> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    let agent = content::agent();
    let download = content::download(&agent, &release.asset, report)?;
    // Beside the executable, so it moves into place with a rename.
    let mut new = tempfile::Builder::new().prefix(".trenchdot-update-").tempfile_in(dir).map_err(|e| io_error(e, dir))?;
    copy(download, new.as_file_mut(), dir)?;
    report.check()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        new.as_file().set_permissions(std::fs::Permissions::from_mode(0o755)).map_err(|e| io_error(e, dir))?;
    }

    new.as_file().sync_all().map_err(|e| io_error(e, dir))?;
    replace(exe, new)
}

fn copy(mut download: std::fs::File, out: &mut std::fs::File, dir: &Path) -> Result<(), Error> {
    let rewind = |f: &mut std::fs::File| std::io::Seek::rewind(f).map_err(|e| io_error(e, dir));
    rewind(&mut download)?;
    let mut head = vec![0u8; MAGIC.len()];
    if download.read_exact(&mut head).is_err() || head != MAGIC {
        return Err(Error::Damaged("it is not an executable for this platform".into()));
    }

    rewind(&mut download)?;
    let n = std::io::copy(&mut download.take(MAX_EXE + 1), out).map_err(|e| io_error(e, dir))?;
    if n > MAX_EXE {
        return Err(Error::Damaged(format!("it has {}", megabytes(n))));
    }

    Ok(())
}

fn replace(exe: &Path, new: tempfile::NamedTempFile) -> Result<PathBuf, Error> {
    let old = previous(exe);
    if std::fs::symlink_metadata(&old).is_ok() {
        std::fs::remove_file(&old)
            .map_err(|e| Error::Io(msg!("Could not remove {path}, the build kept from the last update: {e}", path = old.display(), e)))?;
    }

    // Windows cannot overwrite a running executable, but it can rename it.
    std::fs::rename(exe, &old).map_err(|e| Error::Io(msg!("Could not move the running editor aside: {e}", e)))?;
    if let Err(e) = new.persist(exe) {
        let e = e.error;
        return Err(Error::Io(match std::fs::rename(&old, exe) {
            Ok(()) => msg!("Could not put the new build in place ({e}), the editor is as it was.", e),
            Err(_) => msg!("Could not put the new build in place ({e}). Rename {old} back to {exe}.", e, old = old.display(), exe = exe.display()),
        }));
    }

    Ok(old)
}

/// Swaps `exe` with the build the last update replaced.
pub fn roll_back(exe: &Path) -> Result<(), Error> {
    let old = previous(exe);
    let aside = exe.with_file_name(format!(".trenchdot-swap-{}", std::process::id()));
    let failed = |e: std::io::Error| Error::Io(msg!("Could not swap the builds: {e}", e));
    std::fs::rename(exe, &aside).map_err(failed)?;
    if let Err(e) = std::fs::rename(&old, exe) {
        let _ = std::fs::rename(&aside, exe);
        return Err(failed(e));
    }

    std::fs::rename(&aside, &old).map_err(failed)
}

/// Starts the editor again with the same arguments, called once its window closed.
pub fn restart_if_asked() {
    if !RESTART.load(AtomicOrdering::Relaxed) {
        return;
    }

    let Some(exe) = exe() else { return };
    if let Err(e) = std::process::Command::new(exe).args(std::env::args_os().skip(1)).spawn() {
        eprintln!("TrenchDot: could not start {} again: {e}", exe.display());
    }
}

/// Closing was cancelled, so the editor stays and no second one starts later.
pub fn cancel_restart() {
    RESTART.store(false, AtomicOrdering::Relaxed);
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn thread<T: Send + 'static>(repaint: Option<egui::Context>, work: impl FnOnce() -> T + Send + 'static) -> mpsc::Receiver<T> {
    let (tx, rx) = mpsc::channel();
    let lang = i18n::current();
    std::thread::spawn(move || {
        let _lang = i18n::scope(lang);
        let _ = tx.send(work());
        if let Some(ctx) = repaint {
            ctx.request_repaint();
        }
    });
    rx
}

/// Takes what a thread sent, or the error for one that ended without sending.
fn take<T>(rx: &mpsc::Receiver<Result<T, Error>>) -> Option<Result<T, Error>> {
    match rx.try_recv() {
        Ok(result) => Some(result),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(mpsc::TryRecvError::Disconnected) => Some(Err(Error::Io(msg!("The update stopped unexpectedly.")))),
    }
}

/// What an install reports, with the version it put in place.
type JobResult = Result<(Msg, String), Error>;

#[derive(Default)]
pub struct Updater {
    pub open: bool,
    /// Where the release is looked up, [`content::api_url`] unless a test serves it.
    pub api: Option<String>,
    /// The running build and its executable, which tests replace with a stand-in.
    pub running: Option<(String, PathBuf)>,
    checking: Option<mpsc::Receiver<Result<Option<Release>, Error>>>,
    found: Option<Result<Option<Release>, Error>>,
    job: Option<(Reporter, mpsc::Receiver<JobResult>)>,
    /// What installing or rolling back did. Once it worked, only a restart is left to do.
    done: Option<Result<Msg, Error>>,
    installed: Option<String>,
    repaint: Option<egui::Context>,
    /// `skip` writes a file to test the folder, so it runs once rather than every frame.
    skipped: Option<Option<Skip>>,
}

impl Updater {
    fn running(&self) -> (String, PathBuf) {
        self.running.clone().unwrap_or_else(|| (crate::VERSION.to_string(), exe().map(Path::to_path_buf).unwrap_or_default()))
    }

    fn skip(&mut self) -> Option<Skip> {
        let (version, exe) = self.running();
        self.skipped.get_or_insert_with(|| skip(&version, &exe, env_var)).clone()
    }

    pub fn busy(&self) -> bool {
        self.checking.is_some() || self.job.is_some()
    }

    /// Looks for a newer build at startup, when the person allows it and the last look is a day old. Automated runs,
    /// like tests, MCP agents and CI, never look.
    pub fn startup(&mut self, prefs: &mut Prefs, automated: bool, repaint: egui::Context) {
        self.repaint = Some(repaint);
        let now = unix_now();
        if automated || !prefs.update_check || (prefs.update_checked..prefs.update_checked.saturating_add(DAY)).contains(&now) || self.skip().is_some() {
            return;
        }

        self.check();
    }

    pub fn check_from_menu(&mut self) {
        self.open = true;
        if self.skip().is_none() && !matches!(self.done, Some(Ok(_))) {
            self.check();
        }
    }

    fn check(&mut self) {
        if self.busy() {
            return;
        }

        let repaint = self.repaint.clone();
        let api = self.api.clone().unwrap_or_else(content::api_url);
        let (current, _) = self.running();
        self.found = None;
        if matches!(self.done, Some(Err(_))) {
            self.done = None;
        }

        self.checking = Some(thread(repaint, move || check(&api, &current)));
    }

    fn start_install(&mut self, release: Release, exe: PathBuf) {
        let repaint = self.repaint.clone();
        let report = Reporter::new(repaint.clone());
        let thread_report = report.clone();
        let rx = thread(repaint, move || {
            let old = install(&release, &exe, &thread_report)?;
            let name = old.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let text =
                msg!("Installed TrenchDot {version}. Restart to use it, the old build stays as {name} to roll back to.", version = release.version, name);
            Ok((text, release.version))
        });
        self.job = Some((report, rx));
    }

    /// Takes in what the threads found. Only a look that reached GitHub counts as the daily one, so an offline start
    /// tries again next time.
    pub fn poll(&mut self, prefs: &mut Prefs) {
        if let Some(found) = self.checking.as_ref().and_then(take) {
            if found.is_ok() {
                prefs.update_checked = unix_now();
            }

            self.checking = None;
            self.found = Some(found);
        }

        if let Some(done) = self.job.as_ref().and_then(|(_, rx)| take(rx)) {
            self.job = None;
            self.installed = done.as_ref().ok().map(|(_, version)| version.clone());
            self.done = Some(done.map(|(text, _)| text));
        }
    }

    /// The newer build the status bar offers, until it is installed.
    pub fn available(&self) -> Option<&str> {
        match (&self.found, &self.done) {
            (Some(Ok(Some(release))), None | Some(Err(_))) => Some(&release.version),
            _ => None,
        }
    }

    /// Draws the window while it is open, returning where it is.
    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) -> Option<egui::Rect> {
        self.poll(&mut state.prefs);
        if !self.open {
            return None;
        }

        let mut open = true;
        let shown = crate::widgets::window(HEADING).open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
            ui.set_max_width(440.0);
            self.ui(ui, state);
        });
        self.open &= open;
        shown.map(|s| s.response.rect)
    }

    fn ui(&mut self, ui: &mut Ui, state: &mut EditorState) {
        let (current, exe) = self.running();
        self.repaint = Some(ui.ctx().clone());
        if let Some((report, _)) = &self.job {
            let progress = report.progress();
            ui.label(if report.cancelled() {
                tr("Cancelling…")
            } else if progress.stage.en.is_empty() {
                tr("Starting")
            } else {
                progress.stage.shown.as_str()
            });
            ui.add(egui::ProgressBar::new(progress.fraction()).text(progress.amount()).animate(progress.total == 0));
            if ui.add_enabled(!report.cancelled(), egui::Button::new(tr("Cancel"))).on_hover_text(tr("Stops the update, the editor stays as it is")).clicked() {
                report.cancel();
            }

            return;
        }

        if let Some(skip) = self.skip() {
            ui.label(skip.message().shown);
            if skip != Skip::LocalBuild {
                download_links(ui);
            }

            return;
        }

        match &self.done {
            Some(Ok(text)) => {
                ui.label(RichText::new(&text.shown).color(theme::colors().success()));
                if let Some(installed) = &self.installed
                    && state.game.addon_version.as_deref().is_some_and(|v| crate::addon_install::compare(v, installed) == Some(Ordering::Less))
                {
                    ui.label(tr("The addon in this project is older. Update it after the restart from Godot > Install or Update Addon."));
                }

                if ui.button(RichText::new(tr("Restart Now")).strong()).on_hover_text(tr("Closes the editor and starts the new build")).clicked() {
                    RESTART.store(true, AtomicOrdering::Relaxed);
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }

                return;
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e.message().shown).color(theme::colors().warning()));
                if e.is_download() {
                    download_links(ui);
                }
            }
            None => {}
        }

        let mut install = None;
        if self.checking.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Looking for a newer build…"));
            });
        } else {
            match &self.found {
                None => {}
                Some(Ok(None)) => {
                    ui.label(trf!("TrenchDot {version} is the newest build.", version = current));
                }
                Some(Ok(Some(release))) => {
                    ui.label(trf!("TrenchDot {new} is out, you have {current}.", new = release.version, current));
                    let button = egui::Button::new(RichText::new(tr("Download and Install")).strong());
                    if ui.add(button).on_hover_text(tr("Downloads the new build and keeps this one to roll back to")).clicked() {
                        install = Some(release.clone());
                    }
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e.message().shown).color(theme::colors().warning()));
                    if e.is_download() {
                        download_links(ui);
                    }
                }
            }
        }

        if let Some(release) = install {
            self.start_install(release, exe.clone());
            return;
        }

        ui.horizontal(|ui| {
            if ui.add_enabled(self.checking.is_none(), egui::Button::new(tr("Check Again"))).clicked() {
                self.check();
            }

            if previous(&exe).is_file() && ui.button(tr("Roll Back")).on_hover_text(tr("Puts back the build the last update replaced")).clicked() {
                self.installed = None;
                self.done = Some(roll_back(&exe).map(|()| msg!("Rolled back to the previous build. Restart to use it.")));
            }
        });
        ui.separator();
        ui.checkbox(&mut state.prefs.update_check, tr("Look for a newer build once a day"));
    }
}

fn download_links(ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(tr("Download it yourself from"));
        ui.hyperlink_to(tr("the GitHub releases"), crate::addon_wizard::RELEASES_PAGE).on_hover_text(crate::addon_wizard::RELEASES_PAGE);
        ui.label(tr("or"));
        ui.hyperlink_to(tr("itch.io"), crate::addon_wizard::ITCH_PAGE).on_hover_text(crate::addon_wizard::ITCH_PAGE);
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::content::test_server::{Server, zip};

    const OLD: &str = "0.2.0-beta.202611010000.aaaaaaa";
    const NEW: &str = "0.2.0-beta.202611071200.bbbbbbb";
    const EXE: &str = if cfg!(windows) { "trenchdot.exe" } else { "trenchdot" };

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("td_updater_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A file that passes for an executable of this platform.
    fn exe_bytes(tail: &[u8]) -> Vec<u8> {
        [MAGIC, tail].concat()
    }

    fn publish(server: &Server, title: &str, exe: &[u8]) {
        let exe = exe_bytes(exe);
        server.titled_release(TAG, title, &[(ASSET.unwrap(), &exe), ("func_godot-trenchdot-addon.zip", b"addon")], true);
    }

    fn release_of(server: &Server) -> Release {
        check(&server.url, OLD).unwrap().unwrap()
    }

    #[test]
    fn only_a_published_build_in_a_folder_it_can_write_updates_itself() {
        if ASSET.is_none() {
            assert_eq!(skip(NEW, Path::new("trenchdot"), |_| None), Some(Skip::Platform));
            return;
        }

        let dir = folder("skip");
        let exe = dir.join(EXE);
        assert_eq!(skip(crate::RELEASE, &exe, |_| None), Some(Skip::LocalBuild));
        assert_eq!(skip(NEW, &exe, |_| None), None);
        assert_eq!(skip(NEW, &exe, |v| (v == "FLATPAK_ID").then(|| "dev.trenchdot".into())), Some(Skip::Packaged));
        assert_eq!(skip(NEW, &exe, |v| (v == "APPIMAGE").then(String::new)), None, "an empty variable is not set");
        assert_eq!(skip(NEW, &dir.join("gone").join(EXE), |_| None), Some(Skip::ReadOnly(dir.join("gone"))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_beta_is_offered_only_when_its_build_stamp_is_newer() {
        let Some(asset) = ASSET else { return };
        let server = Server::start();
        assert_eq!(check(&server.url, OLD), Err(Error::NoRelease));
        publish(&server, &format!("TrenchDot {NEW}"), b"new");
        let found = release_of(&server);
        assert_eq!(found.version, NEW);
        assert!(found.asset.url.ends_with(asset) && found.asset.sha256.is_some());
        assert_eq!(check(&server.url, NEW), Ok(None));
        assert_eq!(check(&server.url, "0.3.0"), Ok(None), "a newer release is no downgrade");

        publish(&server, "TrenchDot rolling beta", b"new");
        assert_eq!(check(&server.url, OLD), Ok(None), "a beta from before the stamp was in its title");
        server.titled_release(TAG, &format!("TrenchDot {NEW}"), &[], true);
        assert_eq!(check(&server.url, OLD), Err(Error::NotPublished(asset)));
    }

    #[test]
    fn an_update_swaps_the_executable_and_rolls_back() {
        if ASSET.is_none() {
            return;
        }

        let server = Server::start();
        let dir = folder("swap");
        let exe = dir.join(EXE);
        std::fs::write(&exe, "old").unwrap();
        publish(&server, &format!("TrenchDot {NEW}"), b"new");
        let kept = install(&release_of(&server), &exe, &Reporter::default()).unwrap();
        assert_eq!(kept, previous(&exe));
        assert_eq!((std::fs::read(&exe).unwrap(), std::fs::read(&kept).unwrap()), (exe_bytes(b"new"), b"old".to_vec()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777, 0o755);
        }

        roll_back(&exe).unwrap();
        assert_eq!((std::fs::read(&exe).unwrap(), std::fs::read(&kept).unwrap()), (b"old".to_vec(), exe_bytes(b"new")));

        publish(&server, &format!("TrenchDot {NEW}"), b"newer");
        install(&release_of(&server), &exe, &Reporter::default()).unwrap();
        assert_eq!((std::fs::read(&exe).unwrap(), std::fs::read(&kept).unwrap()), (exe_bytes(b"newer"), b"old".to_vec()), "only one old build is kept");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "nothing else is left beside it");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_download_that_is_not_the_editor_changes_nothing() {
        if ASSET.is_none() {
            return;
        }

        let server = Server::start();
        let dir = folder("not_exe");
        let exe = dir.join(EXE);
        std::fs::write(&exe, "old").unwrap();
        for body in [&b"<html>rate limited</html>"[..], &zip(&[(EXE, &exe_bytes(b"zipped"))])] {
            server.titled_release(TAG, &format!("TrenchDot {NEW}"), &[(ASSET.unwrap(), body)], true);
            assert!(matches!(install(&release_of(&server), &exe, &Reporter::default()), Err(Error::Damaged(_))));
        }

        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    struct Fixture {
        state: EditorState,
        updater: Updater,
    }

    fn wait(harness: &mut egui_kittest::Harness<'static, Fixture>) {
        harness.step();
        for _ in 0..500 {
            if !harness.state().updater.busy() {
                break;
            }

            std::thread::sleep(Duration::from_millis(10));
            harness.step();
        }

        harness.run();
    }

    #[test]
    fn the_window_offers_the_new_build_and_a_restart_once_it_is_in_place() {
        if ASSET.is_none() {
            return;
        }

        let server = Server::start();
        publish(&server, &format!("TrenchDot {NEW}"), b"new");
        let dir = folder("window");
        let exe = dir.join(EXE);
        std::fs::write(&exe, "old").unwrap();
        let updater = Updater { api: Some(server.url.clone()), running: Some((OLD.into(), exe.clone())), ..Default::default() };
        let fixture = Fixture { state: EditorState::new(Prefs::default()), updater };
        let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(700.0, 500.0)).build_ui_state(
            |ui, f: &mut Fixture| {
                let ctx = ui.ctx().clone();
                f.updater.show(&ctx, &mut f.state);
            },
            fixture,
        );
        harness.state_mut().updater.check_from_menu();
        wait(&mut harness);
        assert!(harness.state().state.prefs.update_checked > 0, "a look that reached GitHub counts as the daily one");
        assert!(harness.query_by_label(&format!("TrenchDot {NEW} is out, you have {OLD}.")).is_some());
        assert_eq!(harness.state().updater.available(), Some(NEW));
        assert!(harness.query_by_label("Roll Back").is_none(), "there is no old build yet");

        harness.get_by_label("Download and Install").click();
        wait(&mut harness);
        assert!(harness.query_by_label_contains(&format!("Installed TrenchDot {NEW}")).is_some());
        assert!(harness.query_by_label("Restart Now").is_some());
        assert_eq!(harness.state().updater.available(), None, "the status bar stops offering it");
        assert_eq!(std::fs::read(&exe).unwrap(), exe_bytes(b"new"));

        harness.state_mut().updater = Updater { api: Some(server.url.clone()), running: Some((NEW.into(), exe.clone())), ..Default::default() };
        harness.state_mut().updater.check_from_menu();
        wait(&mut harness);
        assert!(harness.query_by_label(&format!("TrenchDot {NEW} is the newest build.")).is_some());
        harness.get_by_label("Roll Back").click();
        harness.run();
        assert!(harness.query_by_label("Rolled back to the previous build. Restart to use it.").is_some());
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn automated_runs_and_a_recent_look_skip_the_startup_check() {
        let mut prefs = Prefs::default();
        let dir = folder("startup");
        let mut updater = Updater { running: Some((NEW.into(), dir.join(EXE))), api: Some("http://127.0.0.1:9".into()), ..Default::default() };
        updater.startup(&mut prefs, true, egui::Context::default());
        assert!(!updater.busy() && prefs.update_checked == 0);
        prefs.update_check = false;
        updater.startup(&mut prefs, false, egui::Context::default());
        assert!(!updater.busy());
        prefs.update_check = true;
        prefs.update_checked = unix_now() - 3600;
        updater.startup(&mut prefs, false, egui::Context::default());
        assert!(!updater.busy(), "it looked an hour ago");
        if ASSET.is_some() {
            prefs.update_checked = unix_now() - DAY - 1;
            updater.startup(&mut prefs, false, egui::Context::default());
            assert!(updater.busy());
            for _ in 0..500 {
                if !updater.busy() {
                    break;
                }

                std::thread::sleep(Duration::from_millis(10));
                updater.poll(&mut prefs);
            }

            assert!(prefs.update_checked < unix_now() - DAY, "a look that failed does not use up the day");
        }

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
