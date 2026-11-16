//! The Look block of the Terrain3D Inspector: the material, assets and ocean the Godot node has.

use std::io::Read;
use std::path::Path;

use egui::{Rect, RichText, Ui};
use td_core::NodeId;
use td_formats::terrain3d_cache::Manifest;

use super::bridge::{Look, LookChange};
use crate::i18n::tr;
use crate::state::EditorState;
use crate::theme;

pub const MATERIAL: &str = "Terrain3DMaterial";
pub const ASSETS: &str = "Terrain3DAssets";

/// How many folder levels below the data folder a resource is looked for, and how many files are opened at most.
const DEPTH: usize = 3;
const FILES: usize = 400;

/// Where the block drew its controls in the last frame, for MCP driven clicks. `enabled` is false while Godot is out of
/// reach.
#[derive(Clone, Debug, Default)]
pub struct LookUi {
    pub enabled: bool,
    pub rects: Vec<(String, Rect)>,
}

/// A look the Inspector shows for a node ahead of the cache, and the look the cache had when it was asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub node: NodeId,
    pub look: Look,
    pub was: Look,
}

impl Shown {
    /// Whether the cache still lags behind. Any export with another look than before is Godot's word, also when the look
    /// was changed in Godot before this change arrived.
    pub fn pending(&self, now: &Look) -> bool {
        *now == self.was && *now != self.look
    }
}

/// The look the cache says the node has, None for a cache from an addon that does not export it.
pub fn look_of(manifest: &Manifest) -> Option<Look> {
    let resources = manifest.resources.as_ref()?;
    let ocean_enabled = manifest.ocean.as_ref().is_some_and(|o| o.enabled);
    Some(Look { material: resources.material.clone(), assets: resources.assets.clone(), ocean_enabled })
}

/// The `type` of a `.tres` header line like `[gd_resource type="Terrain3DMaterial" format=3]`.
fn header_type(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("[gd_resource")?;
    let value = rest.split(" type=\"").nth(1)?;
    value.split('"').next()
}

fn first_line(path: &Path) -> Option<String> {
    let mut head = Vec::new();
    std::fs::File::open(path).ok()?.take(256).read_to_end(&mut head).ok()?;
    let line = head.split(|b| *b == b'\n').next()?;
    Some(String::from_utf8_lossy(line).into_owned())
}

/// The `.tres` files below `folder` that hold a resource of `class`, as paths relative to it with `/`, sorted.
pub fn folder_resources(folder: &Path, class: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut opened = 0;
    let mut dirs = vec![(folder.to_path_buf(), String::new(), 0)];
    while let Some((dir, prefix, depth)) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if depth + 1 < DEPTH && !name.starts_with('.') {
                    dirs.push((entry.path(), format!("{prefix}{name}/"), depth + 1));
                }
            } else if name.to_ascii_lowercase().ends_with(".tres") && opened < FILES {
                opened += 1;
                if first_line(&entry.path()).as_deref().and_then(header_type) == Some(class) {
                    found.push(format!("{prefix}{name}"));
                }
            }
        }
    }

    found.sort();
    found
}

/// The res:// path of a file `relative` to the linked data folder.
pub fn res_path(data_directory: &str, relative: &str) -> String {
    format!("{}/{relative}", data_directory.trim_end_matches('/'))
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// A drop down of the resources of `class` in the data folder, with None first and the current one kept even when it
/// lives elsewhere. Returns the res:// path picked, empty for None.
fn picker(ui: &mut Ui, rects: &mut Vec<(String, Rect)>, key: &str, class: &str, folder: &Path, data_directory: &str, current: &str) -> Option<String> {
    let none = tr("None").to_string();
    let selected = if current.is_empty() { none.clone() } else { file_name(current).to_string() };
    let mut picked = None;
    let combo = egui::ComboBox::from_id_salt(("terrain3d_look", key)).selected_text(selected).show_ui(ui, |ui| {
        let mut options: Vec<String> = folder_resources(folder, class).iter().map(|f| res_path(data_directory, f)).collect();
        if !current.is_empty() && !options.iter().any(|o| o == current) {
            options.push(current.to_string());
        }

        for path in std::iter::once(String::new()).chain(options) {
            let text = if path.is_empty() { none.clone() } else { file_name(&path).to_string() };
            let response = ui.add(egui::Button::selectable(path == current, text));
            rects.push((format!("{key}:{path}"), response.rect));
            if response.clicked() && path != current {
                picked = Some(path);
            }
        }
    });
    rects.push((key.to_string(), combo.response.rect));
    if !current.is_empty() {
        combo.response.on_hover_text(current);
    }

    picked
}

/// Draws the block below Region Size and Vertex Spacing. A pick or the Ocean checkbox goes to Godot, which applies it
/// in one undo step and exports again, so the controls keep showing what Godot answered until the cache catches up.
pub fn block(ui: &mut Ui, state: &mut EditorState, id: NodeId, cache: &Path) {
    let Some(manifest) = state.terrain3d.manifest(cache) else { return };
    let (material, ocean, now) = (manifest.material.clone(), manifest.ocean.clone(), look_of(manifest));
    let Some(data_directory) = state.doc.map.terrain3d(id).map(|l| l.data_directory.clone()) else { return };
    let Some(folder) = cache.parent().map(Path::to_path_buf) else { return };
    ui.label(RichText::new(tr("Look")).weak());
    let Some(mut shown) = now.clone() else {
        ui.label(tr("The cache has no material, assets or ocean, update the TrenchDot addon in Godot"));
        return;
    };

    if let Some(ahead) = &state.terrain3d_look_shown
        && ahead.node == id
    {
        if ahead.pending(&shown) {
            shown = ahead.look.clone();
        } else {
            state.terrain3d_look_shown = None;
        }
    }

    let enabled = state.godot_has_project();
    let hint = tr("Needs the Godot editor open with this scene");
    let (mut change, mut rects) = (LookChange::default(), Vec::new());
    ui.add_enabled_ui(enabled, |ui| {
        egui::Grid::new("terrain3d_look").num_columns(2).show(ui, |ui| {
            ui.label(tr("Material"));
            change.material = picker(ui, &mut rects, "material", MATERIAL, &folder, &data_directory, &shown.material);
            ui.end_row();
            ui.label(tr("Assets"));
            change.assets = picker(ui, &mut rects, "assets", ASSETS, &folder, &data_directory, &shown.assets);
            ui.end_row();
            ui.label(tr("Ocean"));
            let mut on = shown.ocean_enabled;
            let response = ui.checkbox(&mut on, "").on_disabled_hover_text(hint);
            rects.push(("ocean".into(), response.rect));
            change.ocean_enabled = (on != shown.ocean_enabled).then_some(on);
            ui.end_row();
        });
    });

    let mode = match material.world_background {
        0 => tr("None").to_string(),
        1 => tr("Flat").to_string(),
        2 => tr("Noise").to_string(),
        other => other.to_string(),
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(tr("World Background")).weak());
        ui.label(mode);
    });
    if material.world_background == 2 && material.world_noise.is_none() {
        ui.colored_label(theme::colors().warning(), tr("This cache has no noise values, the hills draw flat until Godot exports again"));
    }

    if let Some(ocean) = ocean.filter(|o| o.enabled) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tr("Sea Level")).weak());
            ui.label(format!("{:.1} m", ocean.sea_level));
        });
    }

    state.terrain3d_look_ui = Some(LookUi { enabled, rects });
    if !change.is_empty() {
        state.change_terrain3d_look(id, change);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, name: &str, text: &str) {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_shown_look_waits_until_the_cache_has_another_look_than_when_it_was_asked() {
        let look = |m: &str, ocean_enabled: bool| Look { material: format!("res://t/{m}.tres"), assets: "res://t/a.tres".into(), ocean_enabled };
        let (was, asked, other) = (look("rock", false), look("sand", true), look("snow", false));
        let ahead = Shown { node: NodeId(7), look: asked.clone(), was: was.clone() };
        assert!(ahead.pending(&was), "the cache has not caught up yet");
        assert!(!ahead.pending(&asked), "the export of this change");
        assert!(!ahead.pending(&other), "Godot changed the look before this change arrived");
        assert!(!Shown { look: was.clone(), ..ahead }.pending(&was), "a change to what the cache already has");
    }

    fn header(class: &str) -> String {
        format!("[gd_resource type=\"{class}\" load_steps=2 format=3 uid=\"uid://abc\"]\n\n[resource]\n")
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("td_look_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_header_type_is_the_exact_class_of_a_resource_line() {
        assert_eq!(header_type("[gd_resource type=\"Terrain3DMaterial\" format=3]"), Some("Terrain3DMaterial"));
        assert_eq!(header_type("  [gd_resource script_class=\"Foo\" type=\"Resource\" format=3]"), Some("Resource"));
        assert_eq!(header_type("[gd_scene load_steps=2 format=3]"), None);
        assert_eq!(header_type("[gd_resource format=3]"), None);
        assert_eq!(header_type(""), None);
    }

    #[test]
    fn the_listing_keeps_the_files_of_one_class_sorted_with_their_folder() {
        let dir = scratch("listing");
        write(&dir, "M_terrain.tres", &header(MATERIAL));
        write(&dir, "a/B_alt.tres", &header(MATERIAL));
        write(&dir, "assets.tres", &header(ASSETS));
        write(&dir, "MIXED.TRES", &header(MATERIAL));
        write(&dir, "scene.tres", "[gd_scene format=3]\n");
        write(&dir, "near_miss.tres", &header("Terrain3DMaterialExtra"));
        write(&dir, "notes.txt", &header(MATERIAL));
        write(&dir, ".hidden/H.tres", &header(MATERIAL));
        write(&dir, "a/b/c/too_deep.tres", &header(MATERIAL));
        write(&dir, "a/b/deep.tres", &header(MATERIAL));
        assert_eq!(folder_resources(&dir, MATERIAL), ["MIXED.TRES", "M_terrain.tres", "a/B_alt.tres", "a/b/deep.tres"]);
        assert_eq!(folder_resources(&dir, ASSETS), ["assets.tres"]);
        assert!(folder_resources(&dir, "Terrain3DOcean").is_empty());
        assert!(folder_resources(&dir.join("missing"), MATERIAL).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_folder_lists_nothing_of_the_first() {
        let (one, two) = (scratch("one"), scratch("two"));
        write(&one, "m.tres", &header(MATERIAL));
        write(&two, "other.tres", &header(MATERIAL));
        assert_eq!(folder_resources(&one, MATERIAL), ["m.tres"]);
        assert_eq!(folder_resources(&two, MATERIAL), ["other.tres"]);
        let _ = std::fs::remove_dir_all(&one);
        let _ = std::fs::remove_dir_all(&two);
    }

    #[test]
    fn a_res_path_joins_the_data_directory_with_or_without_a_trailing_slash() {
        assert_eq!(res_path("res://data", "a/m.tres"), "res://data/a/m.tres");
        assert_eq!(res_path("res://data/", "m.tres"), "res://data/m.tres");
        assert_eq!(file_name("res://data/a/m.tres"), "m.tres");
        assert_eq!(file_name("m.tres"), "m.tres");
    }
}
