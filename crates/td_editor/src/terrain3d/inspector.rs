//! The Inspector of a selected Terrain3D node.

use std::collections::BTreeSet;

use egui::{Color32, RichText, Ui};
use td_core::NodeId;
use td_doc::terrain3d::{Grow, GrowPlan, RegionChange, Terrain3DLink};
use td_formats::terrain3d_cache::ManifestTexture;
use td_geom::terrain3d::{Grid, Loc, MAX_VERTEX_SPACING, MIN_VERTEX_SPACING, REGION_SIZES};

use super::bridge;
use super::overlay::Draft;
use crate::commands::Action;
use crate::i18n::{en, tr, tr_n, trf};
use crate::state::EditorState;
use crate::theme;

pub const RESTRICTED_GODOT: &str = en!("Godot: not reached, the project is in Restricted Mode. Click Restricted Mode in the status bar to trust it");

/// How the editor reaches the Godot side of a Terrain3D folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connection {
    /// The Godot editor has the project open, with the Terrain3D version of the folder's manifest, empty when unknown.
    Connected(String),
    NotRunning,
    NoGodot,
    NotInstalled,
    /// The project is in Restricted Mode, so TrenchDot does not reach Godot.
    Restricted,
}

pub fn connection(installed: bool, restricted: bool, godot_open: bool, godot_found: bool, version: &str) -> Connection {
    if !installed {
        Connection::NotInstalled
    } else if restricted {
        Connection::Restricted
    } else if godot_open {
        Connection::Connected(version.to_string())
    } else if godot_found {
        Connection::NotRunning
    } else {
        Connection::NoGodot
    }
}

/// The regions that wait for Godot, the ones Godot changed too, the ones made at another region size than Godot's
/// terrain has and the ones it holds unsaved changes in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub total: usize,
    pub conflicts: Vec<Loc>,
    pub resized: Vec<Loc>,
    pub unsaved: Vec<Loc>,
}

/// Sorts `list` by what Godot does with each region, `unsaved` tells the regions Godot refuses until its scene is saved.
/// A region at another region size stays refused after a save, so it is listed as resized whatever Godot holds.
pub fn changes(list: &[RegionChange], unsaved: impl Fn(Loc) -> bool) -> Changes {
    let (resized, rest): (Vec<_>, Vec<_>) = list.iter().partition(|c| c.resized);
    let (unsaved, rest): (Vec<_>, Vec<_>) = rest.into_iter().partition(|c| unsaved(c.loc));
    Changes {
        total: list.len(),
        conflicts: rest.iter().filter(|c| c.conflict).map(|c| c.loc).collect(),
        resized: resized.iter().map(|c| c.loc).collect(),
        unsaved: unsaved.iter().map(|c| c.loc).collect(),
    }
}

/// Whether Godot holds unsaved changes in region `loc`, or refused it for them, so it waits until the scene is saved.
/// A Godot editor that quit took its unsaved changes along, though its manifest still lists them.
pub fn unsaved_in_godot(state: &EditorState, cache: &std::path::Path, loc: Loc) -> bool {
    state.godot_has_project() && (state.terrain3d.dirty(cache, loc) || state.terrain3d.refused(cache).get(&loc).is_some_and(|r| r == bridge::DIRTY))
}

/// Drops the edit or removal of `loc`, so the region is Godot's again. False when TrenchDot had no change there.
pub fn use_godots(link: &mut Terrain3DLink, loc: Loc) -> bool {
    link.edits.remove(&loc).is_some() | link.removed.remove(&loc).is_some()
}

/// The swatch of a texture: the mean color of its image, multiplied with the tint as the terrain draws it.
/// A texture without an image shows its tint alone.
pub fn swatch(texture: &ManifestTexture, mean: Option<[u8; 3]>) -> Color32 {
    let tint = texture.albedo_color.map(|c| c.clamp(0.0, 1.0));
    let [r, g, b] = mean.unwrap_or([255; 3]).map(f32::from);
    let [tr, tg, tb, _] = tint;
    Color32::from_rgb((r * tr).round() as u8, (g * tg).round() as u8, (b * tb).round() as u8)
}

/// The mean color of an image, ignoring the alpha that holds Terrain3D's height. None for an empty image.
pub fn mean_color(image: &image::RgbaImage) -> Option<[u8; 3]> {
    let count = u64::from(image.width()) * u64::from(image.height());
    if count == 0 {
        return None;
    }

    let mut sum = [0u64; 3];
    for pixel in image.pixels() {
        (0..3).for_each(|i| sum[i] += u64::from(pixel[i]));
    }

    Some(sum.map(|s| ((s as f64 / count as f64).round()) as u8))
}

type SwatchKey = (std::path::PathBuf, String, String, String);

/// The mean colors of the textures' images, read once per image since the Inspector draws every frame.
#[derive(Clone, Default)]
struct Means(std::collections::HashMap<SwatchKey, Option<[u8; 3]>>);

/// The mean color of the thumbnail Godot saved for `texture`, else of its full albedo.
fn texture_mean(ui: &Ui, cache: &std::path::Path, texture: &ManifestTexture) -> Option<[u8; 3]> {
    let key = (cache.to_path_buf(), texture.thumbnail.clone(), texture.albedo.clone(), texture.hash.clone());
    let id = egui::Id::new("terrain3d texture means");
    if let Some(known) = ui.data(|d| d.get_temp::<Means>(id)).and_then(|m| m.0.get(&key).copied()) {
        return known;
    }

    let open = |file: &str| (!file.is_empty()).then(|| image::open(cache.join(file)).ok()).flatten();
    let mean = [&texture.thumbnail, &texture.albedo].into_iter().find_map(|file| open(file)).and_then(|i| mean_color(&i.to_rgba8()));
    ui.data_mut(|d| d.get_temp_mut_or_default::<Means>(id).0.insert(key, mean));
    mean
}

/// The size of the terrain a grow plan leaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Footprint {
    pub columns: u32,
    pub rows: u32,
    pub width: f64,
    pub depth: f64,
}

pub fn footprint(plan: &GrowPlan, grid: &Grid) -> Footprint {
    let (columns, rows) = ((plan.max[0] - plan.min[0] + 1) as u32, (plan.max[1] - plan.min[1] + 1) as u32);
    let meters = |regions: u32| regions as f64 * grid.region_size as f64 * grid.spacing;
    Footprint { columns, rows, width: meters(columns), depth: meters(rows) }
}

/// Whether the typed Region Size and Vertex Spacing differ from what the terrain has.
pub fn settings_differ(grid: &Grid, region_size: u32, spacing: f64) -> bool {
    grid.region_size != region_size || (grid.spacing - spacing).abs() > 1e-6
}

/// Why Godot cannot reslice the terrain of node `id` into regions of `region_size`, None when Terrain3D holds the result.
pub fn region_size_refusal(state: &EditorState, id: NodeId, cache: &std::path::Path, region_size: u32) -> Option<String> {
    let link = state.doc.map.terrain3d(id)?;
    let source = state.terrain3d.source(cache)?;
    link.resized_bounds(&source, region_size).err()
}

/// The Region Size and Vertex Spacing block. Godot rewrites every region file for a change, so a change waits for a
/// confirmation, and only the open Godot editor can make it.
fn settings(ui: &mut Ui, state: &mut EditorState, id: NodeId, cache: &std::path::Path) {
    let Some(grid) = state.terrain3d.grid(cache) else { return };
    let files = state.terrain3d.manifest(cache).map_or(0, |m| m.regions.len());
    let (mut size, mut spacing) = match state.terrain3d_settings {
        Some((node, size, spacing)) if node == id => (size, spacing),
        _ => (grid.region_size, grid.spacing),
    };
    let reachable = state.godot_has_project();
    let hint = tr("Needs the Godot editor open with this scene");
    let fits: Vec<bool> = REGION_SIZES.iter().map(|s| region_size_refusal(state, id, cache, *s).is_none()).collect();
    ui.add_enabled_ui(reachable, |ui| {
        egui::Grid::new("terrain3d_settings").num_columns(2).show(ui, |ui| {
            ui.label(tr("Region Size"));
            egui::ComboBox::from_id_salt("terrain3d_region_size")
                .selected_text(size.to_string())
                .show_ui(ui, |ui| {
                    for (option, fits) in REGION_SIZES.into_iter().zip(&fits) {
                        let label = egui::Button::selectable(size == option, option.to_string());
                        if ui.add_enabled(*fits, label).on_disabled_hover_text(tr("Terrain3D holds at most 32 regions per side")).clicked() {
                            size = option;
                        }
                    }
                })
                .response
                .on_disabled_hover_text(hint);
            ui.end_row();
            ui.label(tr("Vertex Spacing"));
            ui.add(egui::DragValue::new(&mut spacing).range(MIN_VERTEX_SPACING..=MAX_VERTEX_SPACING).speed(0.05).max_decimals(3)).on_disabled_hover_text(hint);
            ui.end_row();
        });
    });

    if !settings_differ(&grid, size, spacing) {
        state.terrain3d_settings = None;
        return;
    }

    ui.label(tr_n!(files, "Rewrite the {n} region file?", "Rewrite all {n} region files?"));
    let (mut change, mut cancel) = (false, false);
    ui.horizontal_wrapped(|ui| {
        let refusal = region_size_refusal(state, id, cache, size).map(|e| problem(&e));
        let button = ui.add_enabled(reachable && refusal.is_none(), egui::Button::new(tr("Change Settings")));
        change = button.on_disabled_hover_text(refusal.unwrap_or_else(|| hint.to_string())).clicked();
        cancel = ui.button(tr("Cancel")).clicked();
    });

    state.terrain3d_settings = (!change && !cancel).then_some((id, size, spacing));
    if change {
        state.change_terrain3d_settings(id, size, spacing);
    }
}

/// A refusal of `plan_grow` in the language of the editor.
fn problem(error: &str) -> String {
    if error.contains("no regions to grow from") {
        tr("The Terrain3D data folder has no regions to grow from").into()
    } else if error.contains("every Terrain3D region") {
        tr("Shrinking by that much would remove every Terrain3D region").into()
    } else if error.contains("at most 32 regions") {
        tr("Terrain3D holds at most 32 regions per side").into()
    } else {
        error.to_string()
    }
}

/// How many regions the terrain has with TrenchDot's edits, unsent ones included. None until the cache loads.
pub fn region_count(state: &EditorState, id: NodeId, cache: &std::path::Path) -> Option<usize> {
    Some(state.doc.map.terrain3d(id)?.locs(&state.terrain3d.source(cache)?).len())
}

/// The Regions block, a draft of regions to add or remove on each side that Apply turns into one undo step.
fn regions(ui: &mut Ui, state: &mut EditorState, id: NodeId, cache: &std::path::Path) {
    let mut draft = match state.terrain3d_draft {
        Some((node, grow)) if node == id => grow,
        _ => Grow::default(),
    };
    ui.label(RichText::new(tr("Regions")).weak());
    match region_count(state, id, cache) {
        Some(0) => _ = ui.label(tr("No regions yet")),
        Some(n) => _ = ui.label(tr_n!(n, "{n} region", "{n} regions")),
        None => {}
    }

    egui::Grid::new("terrain3d_grow").num_columns(2).show(ui, |ui| {
        let sides = [
            (tr("North (-Z)"), &mut draft.north),
            (tr("South (+Z)"), &mut draft.south),
            (tr("West (-X)"), &mut draft.west),
            (tr("East (+X)"), &mut draft.east),
        ];
        for (label, count) in sides {
            ui.label(label);
            ui.add(egui::DragValue::new(count).range(-32..=32));
            ui.end_row();
        }
    });

    let (plan, current) = match (state.doc.map.terrain3d(id), state.terrain3d.source(cache)) {
        (Some(link), Some(source)) if draft != Grow::default() => (Some(link.plan_grow(&source, draft)), link.locs(&source)),
        _ => (None, Vec::new()),
    };
    state.terrain3d_outline = None;
    let mut apply = false;
    if let Some(plan) = &plan {
        match (plan, state.terrain3d.grid(cache)) {
            (Ok(plan), Some(grid)) => {
                let size = footprint(plan, &grid);
                let (w, d) = (format!("{:.0}", size.width), format!("{:.0}", size.depth));
                ui.label(trf!("{c} x {r} regions, {w} x {d} meters", c = size.columns, r = size.rows, w, d));
                let keep = current.into_iter().filter(|l| !plan.remove.contains(l)).collect();
                state.terrain3d_outline = Some((id, Draft { add: plan.add.clone(), remove: plan.remove.clone(), keep }));
            }
            (Err(error), _) => {
                ui.colored_label(theme::colors().warning(), problem(error));
            }
            _ => {}
        }
    }

    ui.horizontal_wrapped(|ui| {
        apply = ui.add_enabled(matches!(plan, Some(Ok(_))), egui::Button::new(tr("Apply"))).clicked();
        if ui.add_enabled(draft != Grow::default(), egui::Button::new(tr("Reset"))).clicked() {
            draft = Grow::default();
        }
    });

    if apply && let Some(source) = state.terrain3d.source(cache) {
        state.doc.edit("Grow Terrain3D", |m, _| {
            if let Some(link) = m.terrain3d_mut(id) {
                let _ = link.grow(&source, draft);
            }
        });
        draft = Grow::default();
        state.terrain3d_outline = None;
    }

    state.terrain3d_draft = (draft != Grow::default()).then_some((id, draft));
}

pub fn inspector(ui: &mut Ui, state: &mut EditorState, id: NodeId, actions: &mut Vec<Action>) {
    let Some(folder) = state.doc.map.terrain3d(id).map(|l| l.data_directory.clone()) else { return };
    let heading = ui.heading(tr("Terrain3D"));
    crate::panels::section_help(&heading, "Terrain3D", "");
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(tr("Data Folder")).weak());
        ui.label(&folder);
    });

    let Some((_, cache)) = state.terrain3d_links().into_iter().find(|(node, _)| *node == id) else {
        ui.label(tr("Open a Godot project first, Terrain3D data lives in one"));
        return;
    };

    state.terrain3d.refresh(&cache);
    let manifest = state.terrain3d.manifest(&cache).cloned();
    let installed = state.game.project_root.as_deref().is_some_and(|root| root.join("addons").join("terrain_3d").is_dir());
    let version = manifest.as_ref().map_or("", |m| m.terrain3d.as_str());
    match connection(installed, state.restricted(), state.godot_has_project(), state.godot.exe.is_some(), version) {
        Connection::Connected(version) if version.is_empty() => ui.label(tr("Godot: connected")),
        Connection::Connected(version) => ui.label(trf!("Godot: connected, Terrain3D {version}", version)),
        Connection::NotRunning => ui.label(tr("Godot: not running, a save applies edits with a headless Godot")),
        Connection::NoGodot => ui.label(tr("No Godot found, edits wait in the data folder")),
        Connection::NotInstalled => ui.label(tr("Terrain3D is not installed in this project")),
        Connection::Restricted => ui.colored_label(theme::colors().warning(), tr(RESTRICTED_GODOT)),
    };

    if let Some(error) = state.terrain3d.error(&cache) {
        ui.colored_label(theme::colors().warning(), error.to_string());
    }

    if manifest.is_none() {
        ui.colored_label(theme::colors().warning(), tr("Godot has not exported this folder yet, so TrenchDot shows none of it"));
        if ui.add_enabled(!state.restricted(), egui::Button::new(tr("Export from Godot"))).clicked()
            && let Err(e) = state.export_terrain3d_cache(&cache)
        {
            state.fail_status(e);
        }
    }

    ui.separator();
    settings(ui, state, id, &cache);
    super::look::block(ui, state, id, &cache);
    regions(ui, state, id, &cache);
    ui.toggle_value(&mut state.terrain3d_edit_regions, tr("Edit Regions")).on_hover_text(tr("Click regions in the Top view to add or remove them"));
    if let Some(manifest) = &manifest {
        ui.label(RichText::new(tr("Textures")).weak());
        if manifest.textures.is_empty() {
            ui.label(tr("No textures"));
        }

        for texture in &manifest.textures {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.0, swatch(texture, texture_mean(ui, &cache, texture)));
                let name =
                    if texture.name.is_empty() { trf!("Texture {id}", id = texture.id) } else { trf!("{id} {name}", id = texture.id, name = &texture.name) };
                ui.label(name);
            });
        }
    }

    let found = match (state.doc.map.terrain3d(id), state.terrain3d.source(&cache)) {
        (Some(link), Some(source)) => changes(&link.changed_regions(&source), |loc| unsaved_in_godot(state, &cache, loc)),
        _ => Changes::default(),
    };
    if found.total == 0 {
        return;
    }

    ui.separator();
    ui.label(trf!("{regions} changed here, not in Godot yet", regions = tr_n!(found.total, "{n} region", "{n} regions")));
    if ui.button(tr("Send to Godot")).clicked() {
        actions.push(Action::SendTerrain3D);
    }

    let (mut godots, mut mine): (Option<Loc>, Option<Loc>) = (None, None);
    for loc in &found.conflicts {
        ui.label(RichText::new(trf!("Region {x}, {y} changed in Godot too", x = loc[0], y = loc[1])).color(theme::colors().warning()));
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("Use Godot's")).clicked() {
                godots = Some(*loc);
            }

            if ui.button(tr("Keep Mine")).clicked() {
                mine = Some(*loc);
            }
        });
    }

    for loc in &found.resized {
        let warning =
            trf!("Region {x}, {y} was made at another region size than Godot's terrain has, use Godot's or set the size back in Godot", x = loc[0], y = loc[1]);
        ui.label(RichText::new(warning).color(theme::colors().warning()));
        if ui.button(tr("Use Godot's")).clicked() {
            godots = Some(*loc);
        }
    }

    for loc in &found.unsaved {
        let warning = trf!("Godot has unsaved changes in region {x}, {y}, save the scene there and send again", x = loc[0], y = loc[1]);
        ui.label(RichText::new(warning).color(theme::colors().warning()));
        if ui.button(tr("Send Again")).clicked() {
            actions.push(Action::SendTerrain3D);
        }
    }

    if let Some(loc) = godots {
        state.use_godots_terrain3d(id, loc);
    }

    if let Some(loc) = mine {
        state.send_terrain3d(&BTreeSet::from([loc]));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use td_doc::terrain3d::{ChangeKind, RegionEdit, Removal};
    use td_geom::terrain3d::Region3D;

    use super::*;

    fn change(loc: Loc, conflict: bool) -> RegionChange {
        RegionChange { loc, kind: ChangeKind::Edit, region: None, base_hash: None, conflict, resized: false }
    }

    fn resized(loc: Loc, conflict: bool) -> RegionChange {
        RegionChange { resized: true, ..change(loc, conflict) }
    }

    #[test]
    fn the_connection_follows_install_then_godot_then_the_executable() {
        let cases = [
            (false, false, true, true, Connection::NotInstalled),
            (false, true, false, false, Connection::NotInstalled),
            (true, true, true, true, Connection::Restricted),
            (true, true, false, false, Connection::Restricted),
            (true, false, true, false, Connection::Connected("1.1.0".into())),
            (true, false, true, true, Connection::Connected("1.1.0".into())),
            (true, false, false, true, Connection::NotRunning),
            (true, false, false, false, Connection::NoGodot),
        ];
        for (installed, restricted, open, found, want) in cases {
            assert_eq!(
                connection(installed, restricted, open, found, "1.1.0"),
                want,
                "installed {installed}, restricted {restricted}, open {open}, found {found}"
            );
        }

        assert_eq!(connection(true, false, true, true, ""), Connection::Connected(String::new()));
    }

    #[test]
    fn changes_count_every_region_and_list_the_conflicts_in_order() {
        let none = |_| false;
        assert_eq!(changes(&[], none), Changes::default());
        let list = [change([2, -1], false), change([-3, 4], true), change([0, 0], false), change([5, 5], true)];
        assert_eq!(changes(&list, none), Changes { total: 4, conflicts: vec![[-3, 4], [5, 5]], unsaved: vec![], ..Default::default() });
        assert_eq!(changes(&list[..1], none), Changes { total: 1, ..Default::default() });
        let unsaved = |loc: Loc| loc == [5, 5] || loc == [2, -1] || loc == [7, 7];
        assert_eq!(
            changes(&list, unsaved),
            Changes { total: 4, conflicts: vec![[-3, 4]], unsaved: vec![[2, -1], [5, 5]], ..Default::default() },
            "unsaved wins over conflict"
        );
    }

    #[test]
    fn a_region_at_another_region_size_is_listed_apart_and_never_a_conflict() {
        let none = |_| false;
        let list = [change([2, -1], true), resized([-3, 4], true), resized([8, 8], false), change([0, 0], false), resized([5, -5], true)];
        let found = changes(&list, none);
        assert_eq!(found, Changes { total: 5, conflicts: vec![[2, -1]], resized: vec![[-3, 4], [8, 8], [5, -5]], unsaved: vec![] });
        let unsaved = |loc: Loc| loc == [8, 8] || loc == [2, -1];
        assert_eq!(
            changes(&list, unsaved),
            Changes { total: 5, conflicts: vec![], resized: vec![[-3, 4], [8, 8], [5, -5]], unsaved: vec![[2, -1]] },
            "resized wins over unsaved, a save in Godot would not let it through"
        );
        assert_eq!(changes(&list[..1], none).resized, Vec::<Loc>::new());
    }

    #[test]
    fn using_godots_drops_only_that_regions_edit_or_removal() {
        let mut link = Terrain3DLink::new("res://terrain");
        let edit = |loc: Loc| RegionEdit::new(Arc::new(Region3D::blank(loc, 64)), Some([3; 32]));
        link.edits.insert([1, -2], edit([1, -2]));
        link.edits.insert([4, 0], edit([4, 0]));
        link.removed.insert([-5, 2], Removal { hash: [9; 32], region_size: 64 });
        assert!(use_godots(&mut link, [1, -2]));
        assert!(use_godots(&mut link, [-5, 2]));
        assert!(!use_godots(&mut link, [1, -2]), "nothing left to drop");
        assert!(!use_godots(&mut link, [9, 9]));
        assert_eq!(link.edits.keys().copied().collect::<Vec<_>>(), [[4, 0]]);
        assert!(link.removed.is_empty());
    }

    #[test]
    fn a_swatch_is_the_mean_of_the_image_times_the_tint_and_the_tint_alone_without_one() {
        let texture = |albedo_color| ManifestTexture { id: 3, albedo_color, ..Default::default() };
        let mean = Some([200, 100, 50]);
        assert_eq!(swatch(&texture([1.0; 4]), mean), Color32::from_rgb(200, 100, 50), "a white tint leaves the image");
        assert_eq!(swatch(&texture([0.5, 1.0, 0.2, 0.3]), mean), Color32::from_rgb(100, 100, 10));
        assert_eq!(swatch(&texture([-1.0, 2.0, 1.0, 1.0]), mean), Color32::from_rgb(0, 100, 50), "the tint is clamped");
        assert_eq!(swatch(&texture([0.1, 0.2, 0.9, 0.5]), None), Color32::from_rgb(26, 51, 230));
    }

    #[test]
    fn the_mean_color_of_an_image_ignores_the_alpha_and_an_empty_image_has_none() {
        let mut image = image::RgbaImage::from_pixel(3, 1, image::Rgba([10, 20, 30, 0]));
        image.put_pixel(1, 0, image::Rgba([40, 50, 60, 255]));
        image.put_pixel(2, 0, image::Rgba([100, 20, 0, 7]));
        assert_eq!(mean_color(&image), Some([50, 30, 30]));
        assert_eq!(mean_color(&image::RgbaImage::new(0, 4)), None);
    }

    #[test]
    fn a_footprint_counts_regions_and_meters_from_the_slanted_grid() {
        let grid = Grid { region_size: 128, spacing: 2.5, upm: 32.0 };
        let plan = |min, max| GrowPlan { min, max, add: Vec::new(), remove: Vec::new() };
        assert_eq!(footprint(&plan([0, 0], [0, 0]), &grid), Footprint { columns: 1, rows: 1, width: 320.0, depth: 320.0 });
        assert_eq!(footprint(&plan([-3, 2], [-1, 3]), &grid), Footprint { columns: 3, rows: 2, width: 960.0, depth: 640.0 });
    }

    #[test]
    fn a_grow_refusal_is_translated_by_what_it_says_and_an_unknown_one_stays() {
        use crate::i18n::{Lang, scope};
        let _german = scope(Lang::De);
        assert_ne!(problem("the Terrain3D data folder has no regions to grow from"), "the Terrain3D data folder has no regions to grow from");
        assert_ne!(problem("shrinking by that much would remove every Terrain3D region"), "shrinking by that much would remove every Terrain3D region");
        assert_ne!(problem("Terrain3D holds at most 32 regions per side (-16 to 15), the terrain would span [0, 0] to [40, 0]"), "");
        assert_eq!(problem("something else"), "something else");
    }

    #[test]
    fn settings_differ_by_region_size_or_by_more_than_rounding_of_the_spacing() {
        let grid = Grid { region_size: 128, spacing: 2.5, upm: 32.0 };
        assert!(!settings_differ(&grid, 128, 2.5));
        assert!(!settings_differ(&grid, 128, 2.5 + 1e-7));
        assert!(settings_differ(&grid, 128, 2.5 + 1e-5));
        assert!(settings_differ(&grid, 128, 0.25));
        assert!(settings_differ(&grid, 64, 2.5));
        assert!(settings_differ(&grid, 2048, 100.0));
    }
}
