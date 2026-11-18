//! The first steps an empty map shows in its views, and the maps of the open Godot project.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui::{Align2, FontId, Rect, RichText, Stroke, StrokeKind, Ui, Vec2};
use td_doc::Map;

use crate::camera::ViewKind;
use crate::commands::Action;
use crate::i18n::{en, tr, trf};
use crate::state::EditorState;
use crate::theme;
use crate::tools::ToolKind;

pub fn getting_started_url() -> String {
    format!("{}getting-started.html", crate::help::site())
}

/// Set by `discord` in zensical.toml, see build.rs.
pub const DISCORD_URL: &str = env!("TRENCHDOT_DISCORD_URL");

pub const CAMERA_3D_HELP: &str = en!("Hold the right mouse button to look around, WASD flies. Middle drag pans, the wheel moves forward and back.");
pub const CAMERA_2D_HELP: &str = en!("Right or middle drag pans, the wheel zooms. Drag on empty space to draw a box brush.");
const LOOP_STEPS: [&str; 4] = [en!("Block out"), en!("Texture it"), en!("Add entities"), en!("Play in Godot")];

/// The first run card's one line on what a keymap preset changes. A test checks the keys it names against
/// `commands::preset_bindings`.
pub fn preset_line(preset: &str) -> &'static str {
    match preset {
        "trenchbroom" => en!("Q selects, C clips, Ctrl+K subtracts."),
        "hammer" => en!("Shift+S, Shift+X, Shift+V pick Select, Clip, Vertex, H hides."),
        "blender" => en!("A selects all, X deletes, Shift+D duplicates, S scales."),
        _ => "",
    }
}

/// Where the first run card put its parts this frame, for get_state.
#[derive(Clone, Debug, PartialEq)]
pub struct FirstRun {
    pub rect: Rect,
    pub presets: Vec<(&'static str, Rect)>,
    pub skip: Rect,
}

/// The four steps of the loop TrenchDot is for, on one line.
fn loop_strip(ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        for (i, step) in LOOP_STEPS.iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new(">").weak());
            }

            ui.label(RichText::new(format!("{} {}", i + 1, tr(step))).strong());
        }
    });
}

/// The top of a view that its caption, like "3D  Textured", takes and the card stays out of.
const CAPTION_HEIGHT: f32 = 24.0;

const RESCAN: Duration = Duration::from_secs(3);
const MAX_MAPS: usize = 200;

pub fn map_is_empty(map: &Map) -> bool {
    map.layers.iter().all(|l| map.get(*l).is_none_or(|n| n.children.is_empty()))
}

/// The `.tdm` files of the open Godot project. They are scanned on a thread when the project changes and again now and
/// then while something shows them, so a map saved meanwhile turns up without the UI waiting on the disk.
#[derive(Default)]
pub struct ProjectMaps {
    root: Option<PathBuf>,
    scanned: Option<Instant>,
    maps: Vec<PathBuf>,
    pending: Option<mpsc::Receiver<Vec<PathBuf>>>,
}

impl ProjectMaps {
    /// Scans again the next time the maps are shown, for maps that were just added.
    pub fn rescan(&mut self) {
        self.scanned = None;
    }

    pub fn get(&mut self, ctx: &egui::Context, root: Option<&Path>) -> &[PathBuf] {
        if self.root.as_deref() != root {
            *self = Self { root: root.map(Path::to_path_buf), ..Default::default() };
        }

        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(maps) => {
                    self.maps = maps;
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        if let Some(root) = self.root.clone()
            && self.pending.is_none()
            && self.scanned.is_none_or(|t| t.elapsed() > RESCAN)
        {
            self.scanned = Some(Instant::now());
            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                if tx.send(find_maps(&root)).is_ok() {
                    ctx.request_repaint();
                }
            });
            self.pending = Some(rx);
        }

        &self.maps
    }
}

/// The first maps under `root` by path, leaving out the addons' and Godot's own folders.
pub fn find_maps(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < 8 && !name.starts_with('.') && name != "addons" {
                    walk(&path, depth + 1, out);
                }
            } else if td_doc::legacy::is_map_path(&path) {
                out.push(path);
            }
        }
    }

    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out.sort_by(|a, b| a.parent().cmp(&b.parent()).then(a.cmp(b)));
    out.truncate(MAX_MAPS);
    out
}

/// Menu entries for `maps`, under a heading per folder.
pub fn maps_menu(ui: &mut Ui, root: Option<&Path>, maps: &[PathBuf], actions: &mut Vec<Action>) {
    if root.is_none() {
        ui.label(RichText::new(tr("Open a Godot project to list its maps")).weak().italics());
        return;
    }

    if maps.is_empty() {
        ui.label(RichText::new(tr("No maps in this project yet")).weak().italics());
        return;
    }

    let mut folder: Option<&Path> = None;
    for path in maps {
        let parent = path.parent();
        if parent != folder {
            folder = parent;
            let shown = parent.zip(root).and_then(|(p, r)| p.strip_prefix(r).ok()).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            ui.label(RichText::new(if shown.is_empty() { "res://".to_string() } else { format!("res://{shown}") }).weak());
        }

        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if ui.button(name).on_hover_text(path.display().to_string()).clicked() {
            actions.push(Action::OpenMapFile(path.clone()));
            ui.close();
        }
    }
}

/// Tells a newcomer where to start while the map is empty. A card in the 3D view names the first step and the help
/// around it, and the 2D views say where to drag. `start_view` is the 2D view the card sends people to, the Top view
/// unless it is closed, and it says so most clearly. On a first start the card asks for a keymap preset instead and
/// reports where it put its parts.
pub fn view_hint(
    ui: &mut Ui,
    rect: Rect,
    kind: ViewKind,
    start_view: Option<ViewKind>,
    state: &mut EditorState,
    maps: &mut ProjectMaps,
    actions: &mut Vec<Action>,
) -> Option<FirstRun> {
    if !map_is_empty(&state.doc.map) {
        return None;
    }

    let first_run_height = ui.id().with("first_run_height");
    // The first run form is taller than the empty map card. A view too short for it shows the card and keeps the
    // question for when the view grows.
    let first_run =
        !state.prefs.keymap_asked && !kind.is_2d() && rect.height() >= ui.data(|d| d.get_temp::<f32>(first_run_height)).unwrap_or(0.0) + CAPTION_HEIGHT;
    if !state.prefs.start_hints && !first_run {
        return None;
    }

    if kind.is_2d() {
        // With the Shape Generator's box showing, a drag draws that box rather than a brush.
        if state.tool == ToolKind::Select && state.shape_bounds.is_none() && rect.width() > 160.0 && rect.height() > 80.0 {
            let start = start_view == Some(kind);
            let (text, color) = if start {
                (tr("Drag here to draw your first box"), theme::colors().yellow)
            } else {
                (tr("Drag here to draw a box"), ui.visuals().text_color())
            };
            let painter = ui.painter_at(rect);
            let galley = painter.layout_no_wrap(text.into(), FontId::proportional(15.0), color);
            let back = Rect::from_center_size(rect.center(), galley.size() + Vec2::new(20.0, 12.0));
            painter.rect_filled(back, 6.0, ui.visuals().extreme_bg_color.gamma_multiply(0.85));
            if start {
                painter.rect_stroke(back, 6.0, Stroke::new(1.5, theme::colors().yellow), StrokeKind::Inside);
            }

            painter.galley(back.center() - galley.size() / 2.0, galley, color);
        }

        return None;
    }

    if rect.width() < 280.0 || rect.height() < 200.0 {
        return None;
    }

    let width = (rect.width() - 32.0).min(420.0);
    let id = ui.id().with("start_hint");
    let size = ui.data(|d| d.get_temp::<Vec2>(id)).unwrap_or(Vec2::new(width, 180.0));
    let card = Align2::CENTER_CENTER.align_size_within_rect(Vec2::new(width, size.y), rect);
    let card = card.translate(Vec2::new(0.0, (rect.min.y + CAPTION_HEIGHT - card.min.y).max(0.0)));
    let root = state.game.project_root.clone();
    let shown = ui.scope_builder(egui::UiBuilder::new().max_rect(card), |ui| {
        egui::Frame::popup(ui.style()).inner_margin(12).show(ui, |ui| {
            // Selectable labels would take the clicks, drags and wheel meant for the camera behind the card.
            ui.style_mut().interaction.selectable_labels = false;
            ui.set_width(ui.available_width());
            if first_run {
                return Some(first_run_form(ui, state));
            }

            ui.label(RichText::new(tr("This is a new, empty map")).strong().size(17.0));
            ui.add_space(2.0);
            loop_strip(ui);
            ui.add_space(4.0);
            let place = match start_view {
                Some(ViewKind::Top) => tr("in the Top view"),
                Some(ViewKind::Front) => tr("in the Front view"),
                Some(ViewKind::Side) => tr("in the Side view"),
                _ => tr("in this view"),
            };
            let first = if state.tool == ToolKind::Select {
                trf!("To start, drag {place} to draw your first box.", place)
            } else {
                match crate::commands::shortcut_text(ui.ctx(), &state.prefs, &Action::SetTool(ToolKind::Select)) {
                    Some(keys) => trf!("To start, pick the Select tool ({keys}), then drag {place} to draw your first box.", keys, place),
                    None => trf!("To start, pick the Select tool, then drag {place} to draw your first box.", place),
                }
            };
            ui.label(RichText::new(first).size(15.0).color(theme::colors().yellow));
            ui.add_space(8.0);
            let hollow = match crate::commands::shortcut_text(ui.ctx(), &state.prefs, &Action::CsgHollow) {
                Some(keys) => trf!("Brush > CSG > Hollow ({keys})", keys),
                None => tr("Brush > CSG > Hollow").into(),
            };
            ui.label(trf!("Then {hollow} turns the box into a room.", hollow));
            ui.label(RichText::new(tr(CAMERA_3D_HELP)).weak());
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                let guide = getting_started_url();
                ui.hyperlink_to(tr("Build Your First Room"), &guide).on_hover_text(&guide);
                let maps = maps.get(ui.ctx(), root.as_deref());
                if !maps.is_empty() {
                    ui.menu_button(tr("Open a map from this project"), |ui| maps_menu(ui, root.as_deref(), maps, actions))
                        .response
                        .on_hover_text(tr("See how a finished map is put together"));
                }

                if ui.button(tr("Hide tips")).on_hover_text(tr("File > Preferences brings them back")).clicked() {
                    state.prefs.start_hints = false;
                }
            });
            None
        })
    });
    ui.data_mut(|d| d.insert_temp(id, shown.response.rect.size()));
    if first_run {
        ui.data_mut(|d| d.insert_temp(first_run_height, shown.response.rect.height()));
    }

    shown.inner.inner.map(|parts| FirstRun { rect: shown.response.rect, ..parts })
}

/// The card of a first start: the loop, a row per keymap preset and a way out. It is the only question the editor asks.
fn first_run_form(ui: &mut Ui, state: &mut EditorState) -> FirstRun {
    // The 3D view of the default layout is about 475 by 246 points, so the form has to stay short.
    ui.spacing_mut().item_spacing.y = 3.0;
    ui.label(RichText::new(tr("Welcome to TrenchDot")).strong().size(17.0));
    loop_strip(ui);
    ui.add_space(3.0);
    ui.label(tr("Use the keys of the editor you know:"));
    let mut presets = Vec::new();
    for id in crate::commands::PRESETS {
        let style = ui.style().clone();
        let mut job = egui::text::LayoutJob::default();
        RichText::new(format!("{}  ", crate::commands::preset_label(id))).strong().append_to(
            &mut job,
            &style,
            egui::FontSelection::Default,
            egui::Align::Center,
        );
        RichText::new(tr(preset_line(id))).weak().small().append_to(&mut job, &style, egui::FontSelection::Default, egui::Align::Center);
        let row = ui.add(egui::Button::selectable(state.prefs.keymap_preset == id, job).wrap().min_size(Vec2::new(ui.available_width(), 0.0)));
        presets.push((id, row.rect));
        if row.clicked() {
            state.prefs.keymap_preset = id.into();
            state.prefs.keymap_asked = true;
            state.set_status(crate::msg!("Keymap preset set to {preset}", preset = crate::commands::preset_label(id)));
        }
    }

    ui.add_space(3.0);
    let mut skip = Rect::NOTHING;
    ui.horizontal(|ui| {
        let guide = getting_started_url();
        ui.hyperlink_to(tr("Build Your First Room"), &guide).on_hover_text(&guide);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let button = ui.button(tr("Skip")).on_hover_text(tr("File > Preferences > Keymap preset changes it later"));
            skip = button.rect;
            if button.clicked() {
                state.prefs.keymap_asked = true;
            }
        });
    });
    FirstRun { rect: Rect::NOTHING, presets, skip }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readmes_link_the_discord_invite_in_zensical_toml() {
        fn pages(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pages(&path, out);
                } else if path.extension().is_some_and(|e| e == "md") {
                    out.push(path);
                }
            }
        }

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = vec![root.join("README.md")];
        pages(&root.join("docs"), &mut files);
        let stale: Vec<String> = files
            .iter()
            .filter(|f| {
                let text = std::fs::read_to_string(f).unwrap();
                text.match_indices("https://discord.gg/").any(|(at, _)| text[at..].split([')', '"', '>', ' ', '\n']).next() != Some(DISCORD_URL))
            })
            .map(|f| f.strip_prefix(&root).unwrap().display().to_string())
            .collect();
        assert!(stale.is_empty(), "these still link an old Discord invite, change it to {DISCORD_URL}: {stale:?}");
    }

    #[test]
    fn finds_project_maps_but_not_the_addons_or_godot_folders() {
        let dir = std::env::temp_dir().join(format!("td_welcome_maps_{}", std::process::id()));
        for sub in ["maps/showcase", "addons/func_godot", ".godot"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }

        let old = format!("maps/c.{}", td_doc::legacy::MAP_EXTS[1]);
        for file in ["maps/b.tdm", "maps/z.tdm", "maps/showcase/a.tdm", "addons/func_godot/test.tdm", ".godot/cache.tdm", "maps/notes.txt", &old] {
            std::fs::write(dir.join(file), "").unwrap();
        }

        let found: Vec<String> = find_maps(&dir).iter().map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/")).collect();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(found, ["maps/b.tdm", &old, "maps/z.tdm", "maps/showcase/a.tdm"], "a folder's maps stay together, old ones too");
    }

    #[test]
    fn maps_added_meanwhile_show_up_after_a_rescan() {
        let dir = std::env::temp_dir().join(format!("td_welcome_rescan_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
        let mut maps = ProjectMaps::default();
        let wait = |maps: &mut ProjectMaps| {
            for _ in 0..500 {
                if maps.pending.is_none() && maps.scanned.is_some() {
                    break;
                }

                std::thread::sleep(Duration::from_millis(5));
                maps.get(&ctx, Some(&dir));
            }

            maps.get(&ctx, Some(&dir)).to_vec()
        };
        assert!(wait(&mut maps).is_empty());
        std::fs::create_dir_all(dir.join("demo/maps")).unwrap();
        std::fs::write(dir.join("demo/maps/demo.tdm"), "").unwrap();
        assert!(wait(&mut maps).is_empty(), "the list is kept for a few seconds");
        maps.rescan();
        assert_eq!(wait(&mut maps), [dir.join("demo/maps/demo.tdm")], "an install asks for a new scan");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_long_map_list_keeps_the_first_maps_by_path() {
        let dir = std::env::temp_dir().join(format!("td_welcome_many_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("a/first.tdm"), "").unwrap();
        for i in 0..MAX_MAPS + 20 {
            std::fs::write(dir.join(format!("b/{i:03}.tdm")), "").unwrap();
        }

        let found = find_maps(&dir);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(found.len(), MAX_MAPS);
        assert_eq!(found[0], dir.join("a/first.tdm"));
        assert_eq!(found[MAX_MAPS - 1], dir.join(format!("b/{:03}.tdm", MAX_MAPS - 2)));
    }

    struct Fixture {
        state: EditorState,
        maps: ProjectMaps,
        actions: Vec<Action>,
        start_view: Option<ViewKind>,
        view_hovered: bool,
        opened: Vec<String>,
        first_run: Option<FirstRun>,
    }

    impl Fixture {
        /// A map whose keymap question was answered, the card most tests are about.
        fn new(mut state: EditorState) -> Self {
            state.prefs.keymap_asked = true;
            Self::first_start(state)
        }

        fn first_start(state: EditorState) -> Self {
            Self {
                state,
                maps: ProjectMaps::default(),
                actions: Vec::new(),
                start_view: Some(ViewKind::Top),
                view_hovered: false,
                opened: Vec::new(),
                first_run: None,
            }
        }
    }

    /// The 3D view with its card, and the links it opens taken the way the app takes them.
    fn card_harness(fixture: Fixture) -> egui_kittest::Harness<'static, Fixture> {
        egui_kittest::Harness::builder().with_size(egui::vec2(800.0, 600.0)).build_ui_state(
            |ui, f: &mut Fixture| {
                let rect = ui.max_rect();
                f.view_hovered = ui.allocate_rect(rect, egui::Sense::click_and_drag()).hovered();
                f.first_run = view_hint(ui, rect, ViewKind::Perspective, f.start_view, &mut f.state, &mut f.maps, &mut f.actions);
                f.opened.extend(crate::panels::take_open_urls(ui.ctx()));
            },
            fixture,
        )
    }

    #[test]
    fn the_3d_view_card_says_where_to_start_and_lets_the_camera_through() {
        use egui_kittest::kittest::Queryable;

        let dir = std::env::temp_dir().join(format!("td_welcome_hint_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("maps")).unwrap();
        std::fs::write(dir.join("maps/church.tdm"), "").unwrap();
        let mut state = EditorState::new(Default::default());
        state.game.project_root = Some(dir.clone());
        let mut harness = card_harness(Fixture::new(state));
        harness.run();
        assert!(harness.query_by_label("To start, drag in the Top view to draw your first box.").is_some());
        assert!(harness.query_by_label_contains("Brush > CSG > Hollow (").is_some(), "names the menu and the shortcut");
        let title = harness.get_by_label("This is a new, empty map").rect().center();
        harness.hover_at(title);
        harness.run();
        assert!(harness.state().view_hovered, "the card's text lets the camera behind it take the mouse");

        harness.get_by_label("Build Your First Room").click();
        harness.run();
        assert_eq!(harness.state().opened, [getting_started_url()], "the link reaches the app, which opens it in the browser");

        // The scan runs on a thread, the menu shows up once it is done.
        for _ in 0..500 {
            if harness.query_by_label("Open a map from this project").is_some() {
                break;
            }

            std::thread::sleep(Duration::from_millis(10));
            harness.step();
        }

        harness.get_by_label("Open a map from this project").click();
        harness.run();
        assert!(harness.query_by_label("res://maps").is_some());
        harness.get_by_label("church").click();
        harness.run();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(harness.state().actions, vec![Action::OpenMapFile(dir.join("maps/church.tdm"))]);

        harness.get_by_label("Hide tips").click();
        harness.run();
        assert!(!harness.state().state.prefs.start_hints);
        assert!(harness.query_by_label("This is a new, empty map").is_none());
    }

    #[test]
    fn the_first_room_link_opens_the_guide_in_the_editor_language() {
        use egui_kittest::kittest::Queryable;

        for (lang, label, url) in [
            (crate::i18n::Lang::De, "Baue deinen ersten Raum", "https://paraxdev.github.io/TrenchDot/de/getting-started.html"),
            (crate::i18n::Lang::Ja, "最初の部屋を作る", "https://paraxdev.github.io/TrenchDot/ja/getting-started.html"),
            (crate::i18n::Lang::En, "Build Your First Room", "https://paraxdev.github.io/TrenchDot/getting-started.html"),
        ] {
            let _scope = crate::i18n::scope(lang);
            let mut harness = card_harness(Fixture::new(EditorState::new(Default::default())));
            harness.run();
            harness.get_by_label(label).click();
            harness.run();
            assert_eq!(harness.state().opened, [url], "{}", lang.code());
        }
    }

    #[test]
    fn the_first_room_link_reads_like_the_title_of_the_page_it_opens() {
        let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/translations");
        for lang in crate::i18n::Lang::ALL.into_iter().filter(|l| *l != crate::i18n::Lang::En) {
            let site = std::fs::read_to_string(docs.join(lang.code()).join("site.toml")).unwrap();
            let title = site.lines().find_map(|l| l.strip_prefix("\"Build your first room\" = ")).unwrap().trim_matches('"');
            let _scope = crate::i18n::scope(lang);
            assert_eq!(tr("Build Your First Room"), title, "{}", lang.code());
        }
    }

    /// Where the card's title and its last row sit in a 3D view at `view`.
    fn card_in(view: Rect) -> (Rect, Rect) {
        use egui_kittest::kittest::Queryable;
        let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(900.0, 900.0)).build_ui_state(
            |ui, f: &mut Fixture| {
                view_hint(ui, view, ViewKind::Perspective, f.start_view, &mut f.state, &mut f.maps, &mut f.actions);
            },
            Fixture::new(EditorState::new(Default::default())),
        );
        harness.run();
        let title = harness.get_by_label(tr("This is a new, empty map")).rect();
        let last = harness.get_by_label(tr("Hide tips")).rect();
        (title, last)
    }

    #[test]
    fn the_card_stays_below_the_view_caption() {
        let _german = crate::i18n::scope(crate::i18n::Lang::De);
        let short = Rect::from_min_size(egui::pos2(37.0, 61.0), egui::vec2(492.0, 230.0));
        let (title, _) = card_in(short);
        assert!(title.min.y >= short.min.y + CAPTION_HEIGHT + 12.0, "the card leaves the caption free, title at {title:?} in {short:?}");

        let tall = Rect::from_min_size(egui::pos2(37.0, 61.0), egui::vec2(492.0, 470.0));
        let (title, last) = card_in(tall);
        let above = title.min.y - 12.0 - tall.min.y;
        let below = tall.max.y - (last.max.y + 12.0);
        assert!((above - below).abs() < 4.0, "a card with room stays centered, {above} above and {below} below");
    }

    #[test]
    fn the_first_step_fits_the_open_views_and_the_tool() {
        use egui_kittest::kittest::Queryable;

        let mut harness = card_harness(Fixture { start_view: Some(ViewKind::Front), ..Fixture::new(EditorState::new(Default::default())) });
        harness.run();
        assert!(harness.query_by_label("To start, drag in the Front view to draw your first box.").is_some(), "the Top view is closed");
        harness.state_mut().start_view = None;
        harness.run();
        assert!(harness.query_by_label("To start, drag in this view to draw your first box.").is_some(), "only the 3D view is showing");

        harness.state_mut().state.tool = ToolKind::Clip;
        harness.run();
        assert!(harness.query_by_label_contains("pick the Select tool (Q), then drag in this view").is_some(), "only the Select tool draws boxes");

        let layer = harness.state().state.doc.map.default_layer();
        let brush = td_geom::Brush::from_aabb(&td_core::Aabb::new(td_core::DVec3::ZERO, td_core::DVec3::splat(64.0)), "dev/grey").unwrap();
        harness.state_mut().state.doc.map.insert(layer, td_doc::NodeKind::Brush(brush));
        harness.run();
        assert!(harness.query_by_label("This is a new, empty map").is_none(), "the card goes once the map has something in it");
    }

    #[test]
    fn the_2d_hints_step_aside_for_the_shape_generator_box() {
        let ctx = egui::Context::default();
        let mut fixture = Fixture::new(EditorState::new(Default::default()));
        let hints = |f: &mut Fixture| {
            ctx.begin_pass(egui::RawInput::default());
            let rect = Rect::from_min_size(egui::pos2(13.0, 41.0), egui::vec2(410.0, 270.0));
            let mut ui = Ui::new(ctx.clone(), egui::Id::new("views"), egui::UiBuilder::new().max_rect(rect));
            for kind in [ViewKind::Top, ViewKind::Front] {
                view_hint(&mut ui, rect, kind, f.start_view, &mut f.state, &mut f.maps, &mut f.actions);
            }

            let out = ctx.end_pass();
            let texts: Vec<String> =
                out.shapes.iter().filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.galley.text().to_owned()) } else { None }).collect();
            out.drop_without_applying_deltas();
            texts
        };

        let both = [tr("Drag here to draw your first box"), tr("Drag here to draw a box")];
        assert_eq!(hints(&mut fixture), both);
        fixture.state.shape_bounds = Some(td_core::Aabb::new(td_core::DVec3::new(-40.0, 8.0, 24.0), td_core::DVec3::new(88.0, 72.0, 56.0)));
        assert_eq!(hints(&mut fixture), Vec::<String>::new(), "a drag draws the Shape Generator's box, not a brush");
        fixture.state.shape_bounds = None;
        assert_eq!(hints(&mut fixture), both, "closing the dialog brings them back");
    }

    fn unanswered() -> Fixture {
        Fixture::first_start(EditorState::new(Default::default()))
    }

    #[test]
    fn the_empty_map_card_names_the_loop() {
        use egui_kittest::kittest::Queryable;

        let mut harness = card_harness(Fixture::new(EditorState::new(Default::default())));
        harness.run();
        for step in ["1 Block out", "2 Texture it", "3 Add entities", "4 Play in Godot"] {
            assert!(harness.query_by_label(step).is_some(), "{step}");
        }
    }

    #[test]
    fn the_first_run_card_leads_with_the_loop_and_offers_every_preset() {
        use egui_kittest::kittest::Queryable;

        let mut harness = card_harness(unanswered());
        harness.run();
        assert!(harness.query_by_label("Welcome to TrenchDot").is_some());
        for step in ["1 Block out", "2 Texture it", "3 Add entities", "4 Play in Godot"] {
            assert!(harness.query_by_label(step).is_some(), "{step}");
        }

        for id in crate::commands::PRESETS {
            let name = crate::commands::preset_label(id);
            assert!(harness.query_by_label_contains(name).is_some(), "{name}");
            assert!(harness.query_by_label_contains(preset_line(id)).is_some(), "{name} carries its line");
        }

        assert!(harness.query_by_label("Skip").is_some());
        assert!(harness.query_by_label("Build Your First Room").is_some());
        assert!(harness.query_by_label("This is a new, empty map").is_none(), "the first run form replaces the normal card");

        let parts = harness.state().first_run.clone().expect("the card reports its parts");
        assert_eq!(parts.presets.iter().map(|(id, _)| *id).collect::<Vec<_>>(), crate::commands::PRESETS);
        for rect in parts.presets.iter().map(|(_, r)| r).chain([&parts.skip]) {
            assert!(parts.rect.contains_rect(*rect), "{rect:?} lies in the card {:?}", parts.rect);
        }

        assert!(parts.presets.windows(2).all(|w| w[0].1.max.y <= w[1].1.min.y), "one row below the other");
        assert_eq!(harness.state().state.prefs.keymap_preset, "trenchbroom", "TrenchBroom is the preselected row");
        assert!(!harness.state().state.prefs.keymap_asked, "showing the card answers nothing");
    }

    #[test]
    fn picking_a_preset_applies_it_and_ends_the_first_run() {
        use egui_kittest::kittest::Queryable;

        for (id, from) in [("hammer", "trenchbroom"), ("blender", "hammer"), ("trenchbroom", "blender")] {
            let mut fixture = unanswered();
            fixture.state.prefs.keymap_preset = from.into();
            let mut harness = card_harness(fixture);
            harness.run();
            harness.get_by_label_contains(crate::commands::preset_label(id)).click();
            harness.run();
            harness.run();
            let prefs = &harness.state().state.prefs;
            assert_eq!((prefs.keymap_preset.as_str(), prefs.keymap_asked), (id, true), "{id}");
            assert!(harness.query_by_label("Welcome to TrenchDot").is_none(), "{id}");
            assert!(harness.query_by_label("This is a new, empty map").is_some(), "{id} ends in the normal card");
            assert!(harness.state().first_run.is_none(), "{id}");
            assert_eq!(harness.state().state.status, format!("Keymap preset set to {}", crate::commands::preset_label(id)), "{id}");
        }
    }

    #[test]
    fn skip_keeps_the_preset() {
        use egui_kittest::kittest::Queryable;

        let mut fixture = unanswered();
        fixture.state.prefs.keymap_preset = "blender".into();
        let mut harness = card_harness(fixture);
        harness.run();
        let before = harness.state().state.status.clone();
        harness.get_by_label("Skip").click();
        harness.run();
        harness.run();
        let prefs = &harness.state().state.prefs;
        assert_eq!((prefs.keymap_preset.as_str(), prefs.keymap_asked), ("blender", true));
        assert!(harness.query_by_label("This is a new, empty map").is_some());
        assert!(harness.state().first_run.is_none());
        assert_eq!(harness.state().state.status, before, "skip says nothing");
    }

    #[test]
    fn the_first_run_card_waits_for_an_empty_map() {
        use egui_kittest::kittest::Queryable;

        let mut harness = card_harness(unanswered());
        harness.run();
        assert!(harness.query_by_label("Welcome to TrenchDot").is_some());
        let layer = harness.state().state.doc.map.default_layer();
        let brush = td_geom::Brush::from_aabb(&td_core::Aabb::new(td_core::DVec3::ZERO, td_core::DVec3::splat(64.0)), "dev/grey").unwrap();
        harness.state_mut().state.doc.map.insert(layer, td_doc::NodeKind::Brush(brush));
        harness.run();
        assert!(harness.query_by_label("Welcome to TrenchDot").is_none(), "a brush in the map hides it");
        assert!(!harness.state().state.prefs.keymap_asked, "and leaves the question for the next empty map");

        let mut tips_off = unanswered();
        tips_off.state.prefs.start_hints = false;
        let mut harness = card_harness(tips_off);
        harness.run();
        assert!(harness.query_by_label("Welcome to TrenchDot").is_some(), "the question does not depend on the tips setting");

        let mut answered = Fixture::new(EditorState::new(Default::default()));
        answered.state.prefs.start_hints = false;
        let mut harness = card_harness(answered);
        harness.run();
        assert!(harness.query_by_label("Welcome to TrenchDot").is_none());
        assert!(harness.query_by_label("This is a new, empty map").is_none(), "answered with the tips off shows nothing");
        assert!(harness.state().first_run.is_none());
    }

    #[test]
    fn the_first_run_card_waits_for_a_view_it_fits_in() {
        let ctx = egui::Context::default();
        let mut fixture = unanswered();
        let card = |f: &mut Fixture, kind: ViewKind, size: egui::Vec2| {
            ctx.begin_pass(egui::RawInput::default());
            let rect = Rect::from_min_size(egui::pos2(13.0, 41.0), size);
            let mut ui = Ui::new(ctx.clone(), egui::Id::new("view"), egui::UiBuilder::new().max_rect(rect));
            let parts = view_hint(&mut ui, rect, kind, f.start_view, &mut f.state, &mut f.maps, &mut f.actions);
            ctx.end_pass().drop_without_applying_deltas();
            parts.is_some()
        };
        assert!(card(&mut fixture, ViewKind::Perspective, egui::vec2(280.0, 200.0)), "the smallest view that takes it before the form is measured");
        assert!(!card(&mut fixture, ViewKind::Perspective, egui::vec2(279.0, 200.0)));
        assert!(!card(&mut fixture, ViewKind::Perspective, egui::vec2(280.0, 199.0)));
        assert!(!card(&mut fixture, ViewKind::Top, egui::vec2(500.0, 280.0)), "the 2D views keep their drag hint");
        assert!(!fixture.state.prefs.keymap_asked);
    }

    #[test]
    fn the_first_run_card_waits_for_a_view_tall_enough_for_the_form() {
        // German in a narrow view wraps the preset rows, so the form is taller than the 200 point minimum allows.
        let _scope = crate::i18n::scope(crate::i18n::Lang::De);
        let ctx = egui::Context::default();
        let mut fixture = unanswered();
        let mut card = |height: f32| {
            ctx.begin_pass(egui::RawInput::default());
            let rect = Rect::from_min_size(egui::pos2(13.0, 41.0), egui::vec2(280.0, height));
            let mut ui = Ui::new(ctx.clone(), egui::Id::new("view"), egui::UiBuilder::new().max_rect(rect));
            let parts = view_hint(&mut ui, rect, ViewKind::Perspective, fixture.start_view, &mut fixture.state, &mut fixture.maps, &mut fixture.actions);
            ctx.end_pass().drop_without_applying_deltas();
            parts.map(|p| p.rect.height())
        };
        let form = card(500.0).expect("a tall view shows the form");
        assert!(form + CAPTION_HEIGHT - 1.0 >= 200.0, "the form is {form} high, the minimum view height would hide it anyway");
        assert_eq!(card(form + CAPTION_HEIGHT), Some(form), "the shortest view that takes it");
        assert_eq!(card(form + CAPTION_HEIGHT - 1.0), None, "a view that would clip its bottom row");
        assert_eq!(card(500.0), Some(form), "the question waits for the view to grow");
        assert!(!fixture.state.prefs.keymap_asked);
    }

    #[test]
    fn the_first_run_card_fits_the_default_3d_view() {
        use egui_kittest::kittest::Queryable;

        let view = Rect::from_min_size(egui::pos2(37.0, 61.0), egui::vec2(475.0, 246.0));
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::De, crate::i18n::Lang::Ru, crate::i18n::Lang::Fr, crate::i18n::Lang::Ja] {
            let _scope = crate::i18n::scope(lang);
            let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(900.0, 900.0)).build_ui_state(
                |ui, f: &mut Fixture| f.first_run = view_hint(ui, view, ViewKind::Perspective, f.start_view, &mut f.state, &mut f.maps, &mut f.actions),
                unanswered(),
            );
            harness.run();
            harness.run();
            let card = harness.state().first_run.clone().unwrap().rect;
            let title = harness.get_by_label(tr("Welcome to TrenchDot")).rect();
            assert!(view.contains_rect(card), "{}: the card {card:?} lies inside the view {view:?}", lang.code());
            assert!(card.min.y >= view.min.y + CAPTION_HEIGHT, "{}: the card stays below the caption, at {card:?}", lang.code());
            assert!(title.min.y >= card.min.y, "{}", lang.code());
            assert!(card.height() < 222.0, "{}: the card is {} high", lang.code(), card.height());
        }
    }

    #[test]
    fn every_preset_has_a_line_naming_keys_it_binds() {
        use crate::commands::{Action, parse_shortcut, preset_bindings};
        let names = |preset: &str, chord: &str, action: &Action| {
            preset_line(preset).contains(chord) && parse_shortcut(chord).is_some_and(|s| preset_bindings(preset).contains(&(s, action.clone())))
        };
        let lines: [(&str, Vec<(&str, Action)>); 3] = [
            ("trenchbroom", vec![("Q", Action::SetTool(ToolKind::Select)), ("C", Action::SetTool(ToolKind::Clip)), ("Ctrl+K", Action::CsgSubtract)]),
            (
                "hammer",
                vec![
                    ("Shift+S", Action::SetTool(ToolKind::Select)),
                    ("Shift+X", Action::SetTool(ToolKind::Clip)),
                    ("Shift+V", Action::SetTool(ToolKind::Vertex)),
                    ("H", Action::HideSelected),
                ],
            ),
            ("blender", vec![("A", Action::SelectAll), ("X", Action::Delete), ("Shift+D", Action::Duplicate), ("S", Action::SetTool(ToolKind::Scale))]),
        ];
        assert_eq!(lines.iter().map(|(id, _)| *id).collect::<Vec<_>>(), crate::commands::PRESETS, "a preset added later needs its line and its check");
        for (preset, chords) in &lines {
            assert!(!preset_line(preset).is_empty());
            for (chord, action) in chords {
                assert!(names(preset, chord, action), "{preset}: {chord} should do {action:?}");
            }
        }

        assert!(!names("trenchbroom", "A", &Action::SelectAll), "a chord the preset does not bind is caught");
        assert!(!names("hammer", "Shift+S", &Action::SetTool(ToolKind::Clip)), "so is one bound to something else");
        assert_eq!(preset_line("nonsense"), "", "an unknown id has no line");
    }

    #[test]
    fn a_map_with_only_its_layers_is_empty() {
        let mut map = Map::new();
        assert!(map_is_empty(&map));
        let layer = map.default_layer();
        let brush = td_geom::Brush::from_aabb(&td_core::Aabb::new(td_core::DVec3::ZERO, td_core::DVec3::splat(64.0)), "dev/grey").unwrap();
        map.insert(layer, td_doc::NodeKind::Brush(brush));
        assert!(!map_is_empty(&map));
    }
}
