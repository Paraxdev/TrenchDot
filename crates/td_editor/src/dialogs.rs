//! Floating windows: command palette, shape generator, terrain generator and keymap editor.

use egui::{Key, RichText};
use td_core::{Aabb, DVec3};
use td_doc::NodeKind;
use td_geom::heightfield::{TerrainGen, TerrainShape};
use td_geom::rock::{self, Rock};
use td_geom::{Brush, Terrain, TerrainLayer, mesh_shapes, shapes};

use crate::commands::{self, Action, ModelImport};
use crate::i18n::{self, Lang, Msg, en, tr, tr_n, trf};
use crate::mesh_tool::MeshOp;
use crate::state::{EditorState, Shade};
use crate::tool_keys::{ToolKeyRow, describe, preset_rows, tool_key_rows, tools_reading};
use crate::tools::ToolKind;
use crate::widgets;

/// Subsequence match score, higher is better. None if `query` is not a subsequence of `text`.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }

    let text_lower = text.to_lowercase();
    let mut score = 0;
    let mut last: Option<usize> = None;
    let chars: Vec<char> = text_lower.chars().collect();
    let mut pos = 0;
    for qc in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = (pos..chars.len()).find(|i| chars[*i] == qc)?;
        score += 10;
        if last.is_some_and(|l| l + 1 == found) {
            score += 8;
        }

        if found == 0 || !chars[found - 1].is_alphanumeric() {
            score += 6;
        }

        score -= (found - pos).min(10) as i32;
        last = Some(found);
        pos = found + 1;
    }

    Some(score)
}

#[derive(Default)]
pub struct CommandPalette {
    pub open: bool,
    query: String,
    selected: usize,
    /// Scrolls the highlighted row into view on the next frame only, so the wheel can still move the list.
    reveal: bool,
    /// Lists the brush entity classes instead of the commands, for Tie to Entity.
    class_picker: bool,
    /// Lists only the tools in a small popup at the pointer, and number keys pick them.
    tools: bool,
    tools_at: Option<egui::Pos2>,
}

/// A command listed as "Category: Name", `category` and `name` are English keys. `en` keeps the English label for
/// searching, `shown` is in the editor's language.
fn entry(category: &str, name: &str, action: Action) -> (Msg, Action) {
    let en = format!("{}: {}", i18n::english(category), i18n::english(name));
    (Msg { en, shown: format!("{}: {}", tr(category), tr(name)) }, action)
}

/// Like [`entry`] for a name that is built at run time, `name` is called again in English for the English label.
fn entry_with(category: &str, name: impl Fn() -> String, action: Action) -> (Msg, Action) {
    let en = {
        let _en = i18n::scope(Lang::En);
        format!("{}: {}", i18n::english(category), name())
    };
    (Msg { en, shown: format!("{}: {}", tr(category), name()) }, action)
}

fn palette_entries(state: &EditorState) -> Vec<(Msg, Action)> {
    let mut out: Vec<(Msg, Action)> = vec![
        entry(en!("File"), en!("New Map"), Action::NewMap),
        entry(en!("File"), en!("Open Map"), Action::OpenMap),
        entry(en!("File"), en!("Save"), Action::Save),
        entry(en!("File"), en!("Save As"), Action::SaveAs),
        entry(en!("File"), en!("Import .map"), Action::ImportQuakeMap),
        entry(en!("File"), en!("Export .map"), Action::ExportQuakeMap),
        entry(en!("File"), en!("Export glTF Binary (.glb) for Blender"), Action::ExportGlb),
        entry(en!("File"), en!("Export Wavefront OBJ (.obj)"), Action::ExportObj),
        entry(en!("Godot"), en!("Open Godot Project"), Action::OpenProject),
        entry(en!("Godot"), en!("Reload Game Config"), Action::ReloadProject),
        entry(en!("Godot"), en!("Open Project in Godot Editor"), Action::OpenGodotEditor),
        entry(en!("Godot"), en!("Run Project"), Action::RunGodotProject),
        entry(en!("Godot"), en!("Build in Godot"), Action::BuildInGodot),
        entry(en!("Godot"), en!("Toggle Live Mode"), Action::ToggleLiveMode),
        entry(en!("Edit"), en!("Find and Replace"), Action::ShowFindDialog),
        entry(en!("Edit"), en!("Undo"), Action::Undo),
        entry(en!("Edit"), en!("Redo"), Action::Redo),
        entry(en!("Edit"), en!("Duplicate"), Action::Duplicate),
        entry(en!("Edit"), en!("Array Copies of the selection"), Action::ShowArrayDialog),
        entry(en!("Transform"), en!("Transform Selection"), Action::ShowTransformDialog),
        entry(en!("Transform"), en!("Flip Horizontal"), Action::FlipHorizontal),
        entry(en!("Transform"), en!("Flip Vertical"), Action::FlipVertical),
        entry(en!("Edit"), en!("Rename"), Action::Rename),
        entry(en!("Edit"), en!("Delete"), Action::Delete),
        entry(en!("Edit"), en!("Copy"), Action::Copy),
        entry(en!("Edit"), en!("Cut"), Action::Cut),
        entry(en!("Select"), en!("All"), Action::SelectAll),
        entry(en!("Select"), en!("Box Select"), Action::ToggleBoxSelect),
        entry(en!("Select"), en!("X-Ray"), Action::ToggleXray),
        entry(en!("Select"), en!("None"), Action::SelectNone),
        entry(en!("Select"), en!("Inverse"), Action::SelectInverse),
        entry(en!("Select"), en!("Brush Faces"), Action::SelectBrushFaces),
        entry(en!("Select"), en!("Object Mode"), Action::ObjectMode),
        entry(en!("Select"), en!("Face Mode"), Action::FaceMode),
        entry(en!("Select"), en!("All in Current Mode"), Action::SelectAllInMode),
        entry(en!("Select"), en!("Touching"), Action::SelectTouching),
        entry(en!("Select"), en!("Inside"), Action::SelectInside),
        entry(en!("Select"), en!("Siblings"), Action::SelectSiblings),
        entry(en!("Select"), en!("Same Material"), Action::SelectSameMaterial),
        entry(en!("Group"), en!("Group Selection"), Action::Group),
        entry(en!("Group"), en!("Ungroup"), Action::Ungroup),
        entry(en!("Group"), en!("Close Group"), Action::CloseGroup),
        entry(en!("Prefab"), en!("Create from Selection"), Action::CreatePrefab),
        entry(en!("Prefab"), en!("Insert"), Action::InsertPrefab),
        entry(en!("Prefab"), en!("Explode Instance"), Action::ExplodeInstances),
        entry(en!("Prefab"), en!("Open"), Action::OpenPrefab),
        entry(en!("View"), en!("Hide Selected"), Action::HideSelected),
        entry(en!("View"), en!("Isolate Selected"), Action::IsolateSelected),
        entry(en!("View"), en!("Show All"), Action::UnhideAll),
        entry(en!("View"), en!("Lock Selected"), Action::LockSelected),
        entry(en!("View"), en!("Unlock All"), Action::UnlockAll),
        entry(en!("View"), en!("Focus Selection"), Action::FocusSelection),
        entry(en!("View"), en!("Cycle Shading"), Action::ToggleTextured),
        entry(en!("View"), en!("Increase Interface Scale"), Action::UiScaleUp),
        entry(en!("View"), en!("Decrease Interface Scale"), Action::UiScaleDown),
        entry(en!("View"), en!("Reset Interface Scale"), Action::UiScaleReset),
        entry(en!("View"), en!("Maximize View"), Action::ToggleMaximizeView),
        entry(en!("View"), en!("Mouse Look"), Action::ToggleMouseLook),
        entry(en!("View"), en!("Single View"), Action::ViewLayout(1)),
        entry(en!("View"), en!("Two Views"), Action::ViewLayout(2)),
        entry(en!("View"), en!("Four Views"), Action::ViewLayout(4)),
        entry(en!("Grid"), en!("Smaller"), Action::GridDown),
        entry(en!("Grid"), en!("Larger"), Action::GridUp),
        entry(en!("Grid"), en!("Toggle Snap"), Action::ToggleSnap),
        entry(en!("Brush"), en!("CSG Subtract"), Action::CsgSubtract),
        entry(en!("Brush"), en!("CSG Convex Merge"), Action::CsgMerge),
        entry(en!("Brush"), en!("CSG Intersect"), Action::CsgIntersect),
        entry(en!("Brush"), en!("Hollow"), Action::CsgHollow),
        entry(en!("Brush"), en!("Extrude Faces"), Action::ExtrudeFaces),
        entry(en!("Brush"), en!("Snap Vertices to Grid"), Action::SnapVertices),
        entry(en!("Brush"), en!("Tie to Entity"), Action::TieToEntity),
        entry(en!("Brush"), en!("Move to World"), Action::MoveToWorld),
        entry(en!("Brush"), en!("Shape Generator"), Action::ShowShapeDialog),
        entry(en!("Brush"), en!("Add Trim (baseboards, molding)"), Action::ShowTrimDialog),
        entry(en!("Brush"), en!("Bevel Edges of brushes, flat or rounded"), Action::ShowBevelDialog),
        entry(en!("Brush"), en!("Add Roof over the selected brushes"), Action::ShowRoofDialog),
        entry(en!("Brush"), en!("Bridge Faces"), Action::BridgeFaces),
        entry(en!("Brush"), en!("Sweep Faces along a line, arc, S-bend or curve"), Action::ShowSweepDialog),
        entry(en!("Brush"), en!("Place Along Path (fences, roads, cables)"), Action::ShowSplineDialog),
        entry(en!("Brush"), en!("Random Fill from the prefab library"), Action::ShowRandomFillDialog),
        entry(en!("Displacement"), en!("Create (power 2)"), Action::CreateDisplacement(2)),
        entry(en!("Displacement"), en!("Create (power 3)"), Action::CreateDisplacement(3)),
        entry(en!("Displacement"), en!("Create (power 4)"), Action::CreateDisplacement(4)),
        entry(en!("Displacement"), en!("Remove"), Action::RemoveDisplacement),
        entry(en!("Displacement"), en!("Sew"), Action::SewDisplacements),
        entry(en!("Texture"), en!("Toggle UV Lock"), Action::ToggleUvLock),
        entry(en!("Texture"), en!("Hotspot Fit"), Action::HotspotTexture),
        entry(en!("Texture"), en!("UV Editor"), Action::ShowUvEditor),
        entry(en!("Texture"), en!("Auto Materials for floors, ceilings and walls"), Action::ShowAutoMaterialsDialog),
        entry(en!("Layer"), en!("Add Layer"), Action::AddLayer),
        entry(en!("Godot"), en!("Bake Lighting"), Action::BakeLighting),
        entry(en!("View"), en!("Set Cordon from Selection"), Action::SetCordonFromSelection),
        entry(en!("View"), en!("Toggle Cordon"), Action::ToggleCordon),
        entry(en!("View"), en!("Clear Cordon"), Action::ClearCordon),
        entry(en!("File"), en!("Import Hammer .vmf"), Action::ImportVmf),
        entry(en!("File"), en!("Convert Textures (VTF/VMT, WAD, WAL)"), Action::ConvertTextures),
        entry(en!("File"), en!("Import Model as Mesh (.bbmodel)"), Action::ImportModel(ModelImport::Mesh)),
        entry(en!("File"), en!("Import Model as Brushes (.bbmodel)"), Action::ImportModel(ModelImport::Brushes)),
        entry(en!("File"), en!("Place Model Prop (.bbmodel, .glb)"), Action::ImportModel(ModelImport::Prop)),
        entry(en!("File"), en!("Export .map, Cordon Only"), Action::ExportQuakeMapCordon),
        entry(en!("File"), en!("New Tab"), Action::NewTab),
        entry(en!("File"), en!("Close Tab"), Action::CloseTab),
        entry(en!("File"), en!("Next Tab"), Action::NextTab),
        entry(en!("Help"), en!("Keyboard Shortcuts"), Action::ShowKeymap),
        entry(en!("Tool"), en!("Tools at Cursor"), Action::ShowToolsPopup),
        entry(en!("Godot"), en!("Reload Models"), Action::ReloadModels),
        entry(en!("Godot"), en!("Add Content to Project"), Action::ShowProjectContent),
        entry(en!("Godot"), en!("Install or Update Addon"), Action::ShowAddonSetup),
        entry(en!("Group"), en!("Duplicate Linked"), Action::DuplicateLinked),
        entry(en!("Group"), en!("Select Linked Groups"), Action::SelectLinkedGroups),
        entry(en!("Group"), en!("Mirror Again"), Action::MirrorAgain),
        entry(en!("Transform"), en!("Drop to Floor"), Action::DropToFloor),
        entry(en!("Transform"), en!("Snap to Wall"), Action::SnapToWall),
        entry(en!("Group"), en!("Separate Linked Groups"), Action::UnlinkGroups),
        entry(en!("Mesh"), en!("Edit Mesh"), Action::EditMesh),
        entry(en!("Edit"), en!("Toggle Edit Mode"), Action::ToggleEditMode),
        entry(en!("Mesh"), en!("Convert Brushes to Mesh"), Action::ConvertToMesh),
        entry(en!("Mesh"), en!("Convert Meshes to Brushes"), Action::ConvertToBrushes),
        entry(en!("Mesh"), en!("Join Meshes"), Action::JoinMeshes),
        entry(en!("Terrain"), en!("Create Terrain"), Action::ShowTerrainDialog),
        entry(en!("Terrain"), en!("Link Terrain3D"), Action::ShowTerrain3DDialog),
        entry(en!("Terrain"), en!("Send Terrain3D to Godot"), Action::SendTerrain3D),
        entry(en!("Terrain"), en!("Flatten"), Action::TerrainFlatten),
        entry(en!("Terrain"), en!("Auto Paint Layers"), Action::TerrainAutoPaint),
        entry(en!("Scatter"), en!("New Empty Set"), Action::NewScatterSet),
        entry(en!("Scatter"), en!("Fill Targets"), Action::ScatterFill),
        entry(en!("Scatter"), en!("Panel"), Action::ShowScatterPanel),
        entry(en!("Scatter"), en!("Install Nature Models"), Action::InstallNatureModels),
        entry(en!("Scatter"), en!("Sets to Entities"), Action::ScatterToEntities),
        entry(en!("Texture"), en!("Set Blend Material"), Action::SetBlendMaterial),
        entry(en!("Texture"), en!("Clear Blend Material"), Action::ClearBlendMaterial),
        entry(en!("Gameplay"), en!("Lift, Moving Platform"), Action::MakePlatform),
        entry(en!("Gameplay"), en!("Install or Update Gameplay Entities"), Action::InstallGameplayEntities),
        entry(en!("Gameplay"), en!("Link Selected Entities"), Action::ShowLinkDialog),
        entry(en!("Gameplay"), en!("Logic Graph"), Action::ShowLogicGraph),
        entry(en!("Help"), en!("Entity and Code Reference"), Action::ShowReference),
        entry(en!("Help"), en!("Check for Updates"), Action::CheckForUpdates),
        entry(en!("File"), en!("Preferences"), Action::ShowPreferences),
        entry(en!("View"), en!("Toggle Transform Gizmo"), Action::ToggleTransformGizmo),
        entry(en!("View"), en!("Toggle Godot Overlays"), Action::ToggleGodotOverlays),
        entry(en!("View"), en!("Preview void culling"), Action::ToggleVoidPreview),
        entry(en!("View"), en!("Toggle Terrain Level of Detail"), Action::ToggleTerrainLod),
        entry(en!("View"), en!("Toggle Walkable Area"), Action::ToggleWalkable),
        entry(en!("Edit"), en!("Repeat Last"), Action::RepeatLast),
        entry(en!("Group"), en!("Open Group"), Action::OpenGroup),
        entry(en!("Brush"), en!("Box from Last Bounds"), Action::CreateBrushFromBounds),
        entry(en!("Godot"), en!("Reload Materials"), Action::ReloadMaterials),
        entry(en!("Texture"), en!("Align to 3D View"), Action::AlignTextureToView),
        entry(en!("Texture"), en!("Reset Alignment"), Action::ResetTexture),
        entry(en!("Texture"), en!("Copy Material and Alignment"), Action::CopyAlignment),
        entry(en!("Texture"), en!("Paste Alignment"), Action::PasteAlignment),
        entry(en!("Texture"), en!("Toggle Treat as One"), Action::ToggleTreatAsOne),
        entry(en!("Texture"), en!("Hotspot Editor"), Action::ShowHotspotEditor),
    ];
    for s in Shade::ALL {
        out.push(entry_with(en!("Shading"), || tr(s.title()).into(), Action::SetShade(s)));
    }

    for v in crate::bake::View::ALL {
        out.push(entry_with(en!("Shading"), || trf!("Baked View, {view}", view = tr(v.label())), Action::SetBakeView(v)));
    }

    for t in crate::state::UiTheme::ALL {
        out.push(entry_with(en!("View"), || trf!("Theme: {theme}", theme = tr(t.label())), Action::SetTheme(t)));
    }

    for l in crate::i18n::Lang::ALL {
        out.push(entry_with(en!("View"), || trf!("Language: {language}", language = l.name()), Action::SetLanguage(l)));
    }

    for j in td_geom::Justify::ALL {
        out.push(entry_with(en!("Texture"), || trf!("Justify {side}", side = tr(j.label())), Action::Justify(j)));
    }

    for d in [0.125, 0.25, 0.5, 1.0, 2.0, 4.0] {
        out.push(entry_with(en!("Texture"), || trf!("Texel Density {d} units per pixel", d), Action::TexelDensity(d)));
    }

    for k in crate::texture_ops::MeshUvKind::ALL {
        out.push(entry_with(en!("Mesh UVs"), || tr(k.label()).into(), Action::MeshUv(k)));
    }

    for preset in td_doc::scatter::PRESETS {
        out.push(entry_with(en!("Scatter"), || trf!("Preset {preset}", preset), Action::ScatterPreset(preset.to_string())));
    }

    for class in crate::volume_tool::VOLUME_CLASSES {
        out.push(entry_with(en!("Gameplay"), || trf!("{class} Around Selection", class), Action::VolumeAroundSelection(class.to_string())));
    }

    {
        use crate::entity_wizards::{DoorKind, HingeSide, SlideDirection};
        for kind in [
            DoorKind::Hinged { side: HingeSide::Left, angle: 95.0 },
            DoorKind::Hinged { side: HingeSide::Right, angle: 95.0 },
            DoorKind::Sliding { direction: SlideDirection::Up, lip: 4.0 },
            DoorKind::Sliding { direction: SlideDirection::Left, lip: 4.0 },
        ] {
            for trigger in [true, false] {
                let action = Action::MakeDoor { kind: kind.clone(), trigger };
                out.push(entry_with(en!("Gameplay"), || action.title(), action.clone()));
            }
        }
    }

    for op in MeshOp::ALL {
        out.push(entry_with(en!("Mesh"), || tr(op.label()).into(), Action::MeshOp(op)));
    }

    for n in 1..=9u8 {
        out.push(entry_with(en!("Camera"), || trf!("Store Bookmark {n}", n), Action::StoreCamera(n)));
        out.push(entry_with(en!("Camera"), || trf!("Go to Bookmark {n}", n), Action::RecallCamera(n)));
    }

    for t in ToolKind::all() {
        out.push(entry_with(en!("Tool"), || tr(t.label()).into(), Action::SetTool(t)));
    }

    let apply = Action::ApplyMirror { distance: td_doc::symmetry::MIRROR_TOLERANCE };
    out.push(entry_with(en!("Group"), || apply.title(), apply.clone()));
    for axis in 0..3 {
        let a = commands::axis_name(axis);
        out.push(entry_with(en!("Transform"), || trf!("Rotate {axis} +90", axis = a), Action::Rotate { axis, degrees: 90.0 }));
        out.push(entry_with(en!("Transform"), || trf!("Rotate {axis} -90", axis = a), Action::Rotate { axis, degrees: -90.0 }));
        out.push(entry_with(en!("Transform"), || trf!("Flip {axis}", axis = a), Action::Flip { axis }));
        out.push(entry_with(en!("Group"), || trf!("Mirror Linked {axis}", axis = a), Action::MirrorLinked { axis, gap: 0.0 }));
        for to in [td_doc::ops::AlignTo::Min, td_doc::ops::AlignTo::Center, td_doc::ops::AlignTo::Max] {
            let action = Action::Align { axis, to };
            out.push(entry_with(en!("Transform"), || action.title(), action.clone()));
        }

        out.push(entry_with(en!("Transform"), || trf!("Distribute {axis}", axis = a), Action::Distribute { axis }));
        for sign in [1.0, -1.0] {
            let mut offset = DVec3::ZERO;
            offset[axis] = sign * state.grid;
            let step = format!("{}{a}", if sign > 0.0 { "+" } else { "-" });
            out.push(entry_with(en!("Transform"), || trf!("Nudge by Grid {step}", step), Action::Nudge(offset)));
        }
    }

    for def in state.game.solid_entities() {
        out.push(entry_with(en!("Brush Entity"), || def.classname.clone(), Action::CreateBrushEntity(def.classname.clone())));
    }

    for def in state.game.point_entities() {
        let class = &def.classname;
        out.push(entry_with(en!("Entity"), || trf!("Place {class}", class), Action::CreatePointEntity { classname: class.clone(), at: None }));
    }

    for layer in &state.doc.map.layers {
        if let Some(n) = state.doc.map.get(*layer) {
            let name = n.name();
            out.push(entry_with(en!("Layer"), || trf!("Move Selection to {name}", name), Action::MoveToLayer(*layer)));
        }
    }

    if !state.prefs.terrain3d {
        out.retain(|(_, a)| !matches!(a, Action::ShowTerrain3DDialog));
    }

    if !state.godot.found() {
        let runs = state.godot_has_project();
        out.retain(|(_, a)| !matches!(a, Action::OpenGodotEditor) && (runs || !matches!(a, Action::RunGodotProject)));
    }

    if td_doc::mirror::drifted(&state.doc.map, &state.doc.selection).is_empty() {
        out.retain(|(_, a)| *a != Action::MirrorAgain);
    }

    out
}

/// The entries that match `query` in the shown label or the English one, best first.
/// Names other editors give a command, so people find it by the word they know. They stay English and are never shown.
fn aliases(action: &Action) -> &'static [&'static str] {
    match action {
        Action::UnlinkGroups => &["Make Unique", "Make Single User", "Unlink Groups"],
        Action::ShowTransformDialog => &["Transformations"],
        Action::TieToEntity => &["tie", "Make Brush Entity"],
        _ => &[],
    }
}

/// The brush entity classes of the game config, each running Create Brush Entity on the selection.
fn class_entries(state: &EditorState) -> Vec<(Msg, Action)> {
    state.game.solid_entities().map(|d| (Msg { en: d.classname.clone(), shown: d.classname.clone() }, Action::CreateBrushEntity(d.classname.clone()))).collect()
}

fn ranked(query: &str, entries: Vec<(Msg, Action)>) -> Vec<(i32, Msg, Action)> {
    let mut matches: Vec<(i32, Msg, Action)> = entries
        .into_iter()
        .filter_map(|(label, action)| {
            let names = [label.shown.as_str(), label.en.as_str()].into_iter().chain(aliases(&action).iter().copied());
            names.filter_map(|name| fuzzy_score(query, name)).max().map(|s| (s, label, action))
        })
        .collect();
    matches.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.shown.cmp(&b.1.shown)));
    matches
}

impl CommandPalette {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.query.clear();
        self.selected = 0;
        self.reveal = true;
        self.class_picker = false;
        self.tools = false;
        self.tools_at = None;
    }

    pub fn open_tools(&mut self) {
        self.toggle();
        self.open = true;
        self.tools = true;
    }

    /// Opens the list of brush entity classes with `default` highlighted.
    pub fn open_class_picker(&mut self, state: &EditorState, default: &str) {
        self.open = true;
        self.query.clear();
        self.class_picker = true;
        self.tools = false;
        self.selected = ranked("", class_entries(state)).iter().position(|(_, label, _)| label.en == default).unwrap_or(0);
        self.reveal = true;
    }

    fn entries(&self, state: &EditorState) -> Vec<(Msg, Action)> {
        if self.tools {
            return ToolKind::all()
                .into_iter()
                .map(|t| (Msg { en: i18n::english(t.label()).to_string(), shown: tr(t.label()).to_string() }, Action::SetTool(t)))
                .collect();
        }

        if self.class_picker { class_entries(state) } else { palette_entries(state) }
    }

    pub fn show(&mut self, ctx: &egui::Context, state: &EditorState, actions: &mut Vec<Action>) {
        if !self.open {
            return;
        }

        // Shift+Q again moves the popup to the pointer rather than typing a Q into it.
        if self.tools
            && let Some((shortcut, _)) = commands::shortcuts(&state.prefs).into_iter().find(|(_, a)| *a == Action::ShowToolsPopup)
            && ctx.input_mut(|i| {
                let hit = commands::consume_shortcut(i, &shortcut);
                if hit {
                    i.events.retain(|e| !matches!(e, egui::Event::Text(_)));
                }

                hit
            })
        {
            self.query.clear();
            self.selected = 0;
            self.tools_at = None;
        }

        let mut matches = ranked(&self.query, self.entries(state));
        if self.tools {
            let order = ToolKind::all();
            matches.sort_by_key(|(score, _, a)| (-score, order.iter().position(|t| Action::SetTool(*t) == *a)));
        }

        self.selected = self.selected.min(matches.len().saturating_sub(1));

        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.open = false;
            return;
        }

        self.reveal |= up || down;
        if down {
            self.selected = (self.selected + 1).min(matches.len().saturating_sub(1));
        }

        if up {
            self.selected = self.selected.saturating_sub(1);
        }

        let mut run: Option<Action> = None;
        if enter && let Some((_, _, a)) = matches.get(self.selected) {
            run = Some(a.clone());
        }

        let numbered = self.tools && self.query.is_empty();
        if numbered && let Some(n) = ctx.input_mut(|i| (1..=9).find(|&n| i.consume_key(egui::Modifiers::NONE, commands::DIGITS[n - 1]))) {
            if let Some((_, _, a)) = matches.get(n - 1) {
                actions.push(a.clone());
            }

            self.open = false;
            return;
        }

        let frame = egui::Frame::window(&ctx.global_style()).fill(ctx.global_style().visuals.window_fill.gamma_multiply(1.0)).shadow(egui::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: egui::Color32::from_black_alpha(crate::theme::colors().modal_shadow_alpha),
        });
        // Modal, so a click outside closes the palette instead of reaching the view under it.
        if self.tools && self.tools_at.is_none() {
            self.tools_at = Some(ctx.pointer_latest_pos().unwrap_or_else(|| ctx.content_rect().center()));
        }

        let area = match self.tools_at {
            Some(at) => egui::Area::new(egui::Id::new("command_palette"))
                .kind(egui::UiKind::Modal)
                .sense(egui::Sense::hover())
                .order(egui::Order::Foreground)
                .interactable(true)
                .fixed_pos(at)
                .constrain(true),
            None => egui::Modal::default_area(egui::Id::new("command_palette")).anchor(egui::Align2::CENTER_TOP, [0.0, 80.0]),
        };
        let modal =
            egui::Modal::new(egui::Id::new("command_palette")).area(area).frame(frame).backdrop_color(egui::Color32::from_black_alpha(60)).show(ctx, |ui| {
                ui.set_width(if self.tools { 240.0 } else { 460.0 });
                let hint = if self.tools {
                    tr("Type a tool, or press its number…")
                } else if self.class_picker {
                    ui.label(RichText::new(tr("Tie to Entity")).strong());
                    tr("Type a class…")
                } else {
                    tr("Type a command…")
                };
                let edit = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text(hint).desired_width(f32::INFINITY));
                edit.request_focus();
                if edit.changed() {
                    self.selected = 0;
                    self.reveal = true;
                }

                ui.separator();
                let help = matches.get(self.selected).and_then(|(_, _, a)| a.help()).map(tr);
                egui::ScrollArea::vertical().max_height(if help.is_some() { 270.0 } else { 320.0 }).show(ui, |ui| {
                    for (i, (_, label, action)) in matches.iter().enumerate().take(200) {
                        let shortcut = commands::shortcut_text(ctx, &state.prefs, action).unwrap_or_default();
                        let resp = ui.horizontal(|ui| {
                            let text = if numbered && i < 9 { format!("{}  {}", i + 1, label.shown) } else { label.shown.clone() };
                            let r = ui.selectable_label(i == self.selected, text);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(RichText::new(shortcut).weak()));
                            r
                        });
                        if i == self.selected && self.reveal {
                            resp.inner.scroll_to_me(None);
                            self.reveal = false;
                        }

                        if resp.inner.clicked() {
                            run = Some(action.clone());
                        }
                    }

                    if matches.is_empty() {
                        ui.label(RichText::new(tr("No match")).weak());
                    }
                });
                if let Some(help) = help {
                    ui.separator();
                    ui.label(RichText::new(help).weak());
                }
            });
        if modal.should_close() {
            self.open = false;
            return;
        }

        if let Some(a) = run {
            actions.push(a);
            self.open = false;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ShapeKind {
    Cylinder,
    Cone,
    Sphere,
    Wedge,
    Spike,
    Arch,
    Tunnel,
    Pipe,
    Stairs,
    SpiralStairs,
    Torus,
    ArchWall,
    GableRoof,
    Spire,
    Grid,
    Text,
    Rock,
    Pillar,
}

impl ShapeKind {
    const ALL: [ShapeKind; 18] = [
        ShapeKind::Cylinder,
        ShapeKind::Cone,
        ShapeKind::Sphere,
        ShapeKind::Wedge,
        ShapeKind::Spike,
        ShapeKind::Arch,
        ShapeKind::Tunnel,
        ShapeKind::Pipe,
        ShapeKind::Stairs,
        ShapeKind::SpiralStairs,
        ShapeKind::Torus,
        ShapeKind::ArchWall,
        ShapeKind::GableRoof,
        ShapeKind::Spire,
        ShapeKind::Grid,
        ShapeKind::Text,
        ShapeKind::Rock,
        ShapeKind::Pillar,
    ];

    fn label(&self) -> &'static str {
        match self {
            ShapeKind::Cylinder => en!("Cylinder"),
            ShapeKind::Cone => en!("Cone"),
            ShapeKind::Sphere => en!("Sphere"),
            ShapeKind::Wedge => en!("Wedge"),
            ShapeKind::Spike => en!("Spike"),
            ShapeKind::Arch => en!("Arch"),
            ShapeKind::Tunnel => en!("Tunnel"),
            ShapeKind::Pipe => en!("Pipe"),
            ShapeKind::Stairs => en!("Stairs"),
            ShapeKind::SpiralStairs => en!("Spiral Stairs"),
            ShapeKind::Torus => en!("Torus"),
            ShapeKind::ArchWall => en!("Arch Wall"),
            ShapeKind::GableRoof => en!("Gable Roof"),
            ShapeKind::Spire => en!("Spire"),
            ShapeKind::Grid => en!("Grid"),
            ShapeKind::Text => en!("Text"),
            ShapeKind::Rock => en!("Rock"),
            ShapeKind::Pillar => en!("Pillar"),
        }
    }

    /// Shapes that only exist as meshes.
    fn mesh_only(&self) -> bool {
        matches!(self, ShapeKind::Torus | ShapeKind::ArchWall | ShapeKind::GableRoof | ShapeKind::Spire | ShapeKind::Grid)
    }
}

pub struct ShapeDialog {
    pub open: bool,
    kind: ShapeKind,
    sides: usize,
    thickness: f64,
    /// Degrees of a circle an arch sweeps.
    angle: f64,
    /// Options only a tunnel has. Its sides, thickness and angle come from the fields the other shapes share.
    tunnel: shapes::Tunnel,
    steps: usize,
    as_mesh: bool,
    smooth: bool,
    axis: shapes::Axis,
    text: TextShape,
    rock: Rock,
    /// The name the preset row saves under, filled in when a preset is picked.
    preset_name: String,
    /// The editor's grid step, so a tunnel's wall cap lands on it the way its field shows.
    grid: f64,
    /// The span and angle a radius typed out of reach was cut back to, so the dialog says so until either changes.
    radius_cut: Option<(f64, f64)>,
    /// The box kept while the dialog is open, used when nothing is selected.
    bounds: Option<Aabb>,
    /// The generated shape and its triangle count.
    preview: Option<(ShapeKey, Vec<NodeKind>, usize)>,
    /// One entry per Create while the dialog stays open, so an undo, a redo or selecting an earlier shape again still
    /// finds its box.
    made: Vec<Made>,
}

/// Shape Generator settings saved under a name in the preferences, so every map can use them.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ShapePreset {
    pub name: String,
    pub kind: ShapeKind,
    pub sides: usize,
    pub thickness: f64,
    pub angle: f64,
    pub tunnel: shapes::Tunnel,
    pub steps: usize,
    pub as_mesh: bool,
    pub smooth: bool,
    pub axis: shapes::Axis,
    pub text: TextShape,
    pub rock: Rock,
}

impl Default for ShapePreset {
    fn default() -> Self {
        ShapeDialog::default().preset(String::new())
    }
}

/// What a Create made from which box. While it stays selected and unmoved, Create again builds from that box rather
/// than from the made shape, which an arch or a rock leaves smaller.
struct Made {
    ids: Vec<td_doc::NodeId>,
    bounds: Aabb,
    source: Aabb,
}

/// A shape past this many triangles gets its count shown as a warning.
const HEAVY_TRIANGLES: usize = 10_000;

/// Every setting the generated shape depends on, so it is only rebuilt when one of them changes.
type ShapeKey = (ShapeKind, usize, f64, usize, bool, bool, shapes::Axis, TextShape, Rock, Aabb, String, (Option<f64>, Option<shapes::Tunnel>));

/// Block letter settings of the Text shape, laid out from the corner of the bounds.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TextShape {
    pub text: String,
    pub letter_height: f64,
    pub depth: f64,
    /// Gap between letters in font cells.
    pub spacing: f64,
    /// Upright letters reading from +Z, else lying on the floor with their tops towards -Z.
    pub standing: bool,
}

impl Default for TextShape {
    fn default() -> Self {
        Self { text: "TEXT".into(), letter_height: 64.0, depth: 8.0, spacing: 1.0, standing: true }
    }
}

impl TextShape {
    pub fn brushes(&self, corner: DVec3, material: &str) -> Result<Vec<Brush>, String> {
        let bounds = td_geom::block_text::sized_bounds(&self.text, corner, self.letter_height, self.depth, self.standing, self.spacing)?;
        let letters = td_geom::block_text::layout(&self.text, &bounds, self.standing, self.spacing)?;
        Ok(letters.iter().flatten().filter_map(|b| Brush::from_aabb(b, material).ok()).collect())
    }
}

impl Default for ShapeDialog {
    fn default() -> Self {
        Self {
            open: false,
            kind: ShapeKind::Cylinder,
            sides: 16,
            thickness: 16.0,
            angle: 180.0,
            tunnel: shapes::Tunnel::default(),
            steps: 8,
            as_mesh: false,
            smooth: true,
            axis: shapes::Axis::Y,
            text: TextShape::default(),
            rock: Rock::default(),
            preset_name: String::new(),
            grid: 0.0,
            radius_cut: None,
            bounds: None,
            preview: None,
            made: Vec::new(),
        }
    }
}

fn profile_label(profile: shapes::TunnelProfile) -> &'static str {
    match profile {
        shapes::TunnelProfile::Square => en!("Square"),
        shapes::TunnelProfile::Arched => en!("Arched"),
        shapes::TunnelProfile::Round => en!("Round"),
    }
}

fn bounds_label(kind: ShapeKind, bounds: &Aabb, source: &str) -> String {
    let (c, size) = (bounds.min, bounds.size());
    let n = crate::gizmos::fmt_num;
    if kind == ShapeKind::Text {
        trf!("Starts at {x} {y} {z} ({source})", x = n(c.x), y = n(c.y), z = n(c.z), source)
    } else {
        trf!("Bounds {x} x {y} x {z} ({source})", x = n(size.x), y = n(size.y), z = n(size.z), source)
    }
}

/// The Corner and Size rows of the box the shape fills. A size is at least 1 and grows from the corner, and the
/// Text shape only uses the corner. True when the box changed.
fn box_fields(ui: &mut egui::Ui, bounds: &mut Aabb, size: bool, grid: f64) -> bool {
    let mut min = bounds.min.to_array();
    let mut extent = bounds.size().to_array();
    let (mut moved, mut resized) = (false, false);
    egui::Grid::new("shape_box").num_columns(2).show(ui, |ui| {
        ui.label(tr("Corner")).on_hover_text(tr("Lowest corner of the bounds in map units, 32 per meter"));
        moved = widgets::vector_input_snapped(ui, &mut min, 220.0, 1.0, grid);
        ui.end_row();
        if size {
            ui.label(tr("Size")).on_hover_text(tr("Width, height and depth of the bounds. A new size scales from the lowest corner"));
            resized = widgets::vector_input_snapped(ui, &mut extent, 220.0, 1.0, grid);
            ui.end_row();
        }
    });
    if !moved && !resized {
        return false;
    }

    let min = DVec3::from_array(min);
    let extent = if resized { DVec3::from_array(extent.map(|v| v.max(1.0))) } else { bounds.size() };
    let next = Aabb::new(min, min + extent);
    let changed = next != *bounds;
    *bounds = next;
    changed
}

fn preview_lines(kinds: &[NodeKind]) -> Vec<(DVec3, DVec3)> {
    let mut out = Vec::new();
    for kind in kinds {
        match kind {
            NodeKind::Brush(b) => out.extend(b.edges().into_iter().map(|(a, c)| (b.vertices[a as usize], b.vertices[c as usize]))),
            NodeKind::Mesh(m) => out.extend(m.edges().into_iter().map(|(a, c)| (m.vertices[a as usize], m.vertices[c as usize]))),
            _ => {}
        }
    }

    out
}

fn triangles(kinds: &[NodeKind]) -> usize {
    let fan = |n: usize| n.saturating_sub(2);
    kinds
        .iter()
        .map(|kind| match kind {
            NodeKind::Brush(b) => b.faces.iter().map(|f| fan(f.indices.len())).sum(),
            NodeKind::Mesh(m) => m.faces.iter().map(|f| fan(f.indices.len())).sum(),
            _ => 0,
        })
        .sum()
}

/// The selected brushes and meshes, which the shape takes its bounds from and replaces.
fn shape_sources(state: &EditorState) -> Vec<td_doc::NodeId> {
    let (selection, map) = (&state.doc.selection, &state.doc.map);
    let mut ids = selection.brushes(map);
    ids.extend(selection.meshes(map));
    ids
}

impl ShapeDialog {
    fn preset(&self, name: String) -> ShapePreset {
        ShapePreset {
            name,
            kind: self.kind,
            sides: self.sides,
            thickness: self.thickness,
            angle: self.angle,
            tunnel: self.tunnel,
            steps: self.steps,
            as_mesh: self.as_mesh,
            smooth: self.smooth,
            axis: self.axis,
            text: self.text.clone(),
            rock: self.rock,
        }
    }

    fn apply_preset(&mut self, p: &ShapePreset) {
        self.kind = p.kind;
        self.sides = p.sides;
        self.thickness = p.thickness;
        self.angle = p.angle;
        self.tunnel = p.tunnel;
        self.steps = p.steps;
        self.as_mesh = p.as_mesh;
        self.smooth = p.smooth;
        self.axis = p.axis;
        self.text = p.text.clone();
        self.rock = p.rock;
        self.preset_name = p.name.clone();
    }

    fn preset_row(&mut self, ui: &mut egui::Ui, presets: &mut Vec<ShapePreset>) {
        ui.horizontal(|ui| {
            let mut picked = None;
            egui::ComboBox::new("shape_preset", tr("Preset")).selected_text(self.preset_name.as_str()).show_ui(ui, |ui| {
                for p in presets.iter() {
                    if ui.selectable_label(p.name == self.preset_name, &p.name).clicked() {
                        picked = Some(p.clone());
                    }
                }
            });
            if let Some(p) = picked {
                self.apply_preset(&p);
            }

            ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text(tr("Preset name")).desired_width(110.0));
            let name = self.preset_name.trim().to_string();
            let existing = presets.iter().position(|p| p.name == name);
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new(tr("Save Preset")))
                .on_hover_text(tr("Keeps these settings under the name for every map"))
                .clicked()
            {
                let preset = self.preset(name.clone());
                match existing {
                    Some(i) => presets[i] = preset,
                    None => presets.push(preset),
                }

                self.preset_name = name.clone();
            }

            if ui.add_enabled(existing.is_some(), egui::Button::new(tr("Delete"))).clicked() {
                presets.retain(|p| p.name != name);
            }
        });
    }

    fn key(&self, bounds: &Aabb, material: &str) -> ShapeKey {
        (
            self.kind,
            self.sides,
            self.thickness,
            self.steps,
            self.as_mesh,
            self.smooth,
            self.axis,
            self.text.clone(),
            self.rock,
            *bounds,
            material.to_string(),
            (
                matches!(self.kind, ShapeKind::Arch | ShapeKind::ArchWall).then_some(self.angle),
                (self.kind == ShapeKind::Tunnel).then(|| self.tunnel_params(bounds)),
            ),
        )
    }

    fn tunnel_params(&self, bounds: &Aabb) -> shapes::Tunnel {
        // Only an arched tunnel has an arch, so the angle stays out of the key of the others.
        let angle = if self.tunnel.profile == shapes::TunnelProfile::Arched { self.angle } else { 180.0 };
        let thickness = self.thickness.clamp(1.0, tunnel_thickness_max(bounds, self.grid, self.tunnel.axis).max(1.0));
        shapes::Tunnel { sides: self.sides, thickness, angle, ..self.tunnel }
    }

    fn thickness_max(&self, bounds: &Aabb) -> f64 {
        match self.kind {
            ShapeKind::Tunnel => tunnel_thickness_max(bounds, self.grid, self.tunnel.axis),
            ShapeKind::Arch => shapes::arch_thickness_cap(bounds, self.angle),
            _ => 4096.0,
        }
    }

    /// The thickness the shape gets in `bounds`. The fields only show values cut back to the box, so a smaller box or
    /// another shape never lowers what was typed.
    fn thickness_in(&self, bounds: &Aabb) -> f64 {
        let max = self.thickness_max(bounds);
        self.thickness.clamp(1.0_f64.min(max), max)
    }

    pub fn generate(&self, bounds: &Aabb, material: &str) -> Vec<NodeKind> {
        let brushes = |list: Vec<Brush>| -> Vec<NodeKind> { list.into_iter().map(NodeKind::Brush).collect() };
        let smooth = |mut m: td_geom::Mesh| {
            m.smooth_angle = if self.smooth { m.smooth_angle.max(40.0) } else { 0.0 };
            vec![NodeKind::Mesh(m)]
        };
        let as_mesh = self.as_mesh || self.kind.mesh_only();
        let size = bounds.size();
        let thickness = self.thickness_in(bounds);
        let (upright, turn) = self.axis.frame(bounds);
        match (self.kind, as_mesh) {
            (ShapeKind::Cylinder, true) => smooth(mesh_shapes::cylinder(&upright, self.sides, material).transformed(&turn, true)),
            (ShapeKind::Cone, true) => smooth(mesh_shapes::cone(&upright, self.sides, material).transformed(&turn, true)),
            (ShapeKind::Sphere, true) => smooth(mesh_shapes::sphere(bounds, self.sides, (self.sides / 2).max(3), material)),
            (ShapeKind::Torus, _) => {
                let r = size.x.min(size.z) * 0.5;
                smooth(mesh_shapes::torus(
                    bounds.center(),
                    (r - thickness * 0.5).max(1.0),
                    (thickness * 0.5).min(r),
                    self.sides,
                    (self.sides / 2).max(3),
                    material,
                ))
            }
            (ShapeKind::ArchWall, _) => vec![NodeKind::Mesh(mesh_shapes::arch_wall(
                bounds,
                (size.x - thickness * 2.0).max(1.0),
                (size.y - thickness).max(1.0),
                self.angle,
                self.sides,
                material,
            ))],
            (ShapeKind::GableRoof, _) => vec![NodeKind::Mesh(mesh_shapes::gable_roof(bounds, size.z > size.x, thickness, thickness, material))],
            (ShapeKind::Spire, _) => vec![NodeKind::Mesh(mesh_shapes::spire(bounds, 4, material))],
            (ShapeKind::Grid, _) => vec![NodeKind::Mesh(mesh_shapes::grid(bounds, self.steps, self.steps, material))],
            (ShapeKind::Cylinder, false) => brushes(shapes::cylinder(&upright, self.sides, material).map(|b| self.axis.lay(b, bounds)).into_iter().collect()),
            (ShapeKind::Cone, false) => brushes(shapes::cone(&upright, self.sides, material).map(|b| self.axis.lay(b, bounds)).into_iter().collect()),
            (ShapeKind::Sphere, false) => brushes(shapes::sphere(bounds, self.sides, (self.sides / 2).max(3), material)),
            (ShapeKind::Wedge, _) => brushes(shapes::wedge(bounds, material).into_iter().collect()),
            (ShapeKind::Spike, _) => brushes(shapes::spike(bounds, material).into_iter().collect()),
            (ShapeKind::Arch, _) => brushes(shapes::arch(bounds, self.sides, thickness, self.angle, material)),
            (ShapeKind::Tunnel, _) => brushes(shapes::tunnel(bounds, &self.tunnel_params(bounds), material)),
            (ShapeKind::Pipe, _) => brushes(shapes::pipe(bounds, self.sides, thickness, material)),
            (ShapeKind::Stairs, _) => brushes(shapes::stairs(bounds, self.steps, material)),
            (ShapeKind::SpiralStairs, _) => brushes(shapes::spiral_stairs(bounds, self.steps, thickness, 1.0, material)),
            (ShapeKind::Text, _) => brushes(self.text.brushes(bounds.min, material).unwrap_or_default()),
            (ShapeKind::Rock, true) => smooth(mesh_shapes::rock(bounds, &self.rock, material)),
            (ShapeKind::Rock, false) => brushes(shapes::rock(bounds, &self.rock, material).into_iter().collect()),
            (ShapeKind::Pillar, _) => brushes(shapes::pillar(bounds, self.sides, material)),
        }
    }

    /// What the shape replaces and the box it fills. With nothing to replace the dialog keeps a box of its own,
    /// which a view may have edited earlier in the frame.
    fn sources_and_bounds(&mut self, state: &mut EditorState) -> (Vec<td_doc::NodeId>, Aabb) {
        let sources = shape_sources(state);
        if !sources.is_empty() {
            state.shape_bounds = None;
            let bounds = state.doc.map.bounds_of(sources.iter().copied());
            if let Some(m) = self.made.iter().find(|m| m.bounds == bounds && m.ids.len() == sources.len() && sources.iter().all(|id| m.ids.contains(id))) {
                return (sources, m.source);
            }

            return (sources, bounds);
        }

        let bounds = state.shape_bounds.or(self.bounds).unwrap_or_else(|| crate::shape_bounds::seed(state.last_bounds, state.grid));
        state.shape_bounds = Some(bounds);
        self.bounds = Some(bounds);
        (sources, bounds)
    }

    fn arch_fields(&mut self, ui: &mut egui::Ui, bounds: &Aabb) {
        let size = bounds.size();
        let tunnel = self.kind == ShapeKind::Tunnel;
        let params = self.tunnel_params(bounds);
        let (span, height) = match self.kind {
            ShapeKind::Arch => (size.x, size.y),
            ShapeKind::Tunnel => {
                let opening = params.opening(bounds);
                (opening.x, opening.x.min(opening.y) * 0.5)
            }
            _ => {
                let thickness = self.thickness_in(bounds);
                ((size.x - thickness * 2.0).max(1.0), (size.y - thickness).max(1.0))
            }
        };
        ui.horizontal(|ui| {
            let hover = if tunnel {
                tr("How much of a circle the arch sweeps, 180 is a half round and less is flatter, springing from the tops of the walls")
            } else {
                tr("How much of a circle the arch sweeps. At 180 it is a half ellipse as tall as the box, below 180 the box height only caps how high it rises")
            };
            ui.label(tr("Angle")).on_hover_text(hover);
            // Clamped on edits only, else the slider rounds an angle set from a typed radius to its one decimal.
            ui.add(egui::Slider::new(&mut self.angle, shapes::ARCH_MIN_ANGLE..=180.0).suffix("°").max_decimals(1).clamping(egui::SliderClamping::Edits));
        });
        let capped = shapes::arch_sweep(span, height, self.angle);
        let wanted = self.angle.clamp(shapes::ARCH_MIN_ANGLE, 180.0);
        let sweep = if tunnel { params.fitted(bounds).angle } else { capped };
        if tunnel && wanted < 180.0 && sweep >= 180.0 {
            ui.label(RichText::new(tr("Too low and wide for an arch under 180° at this thickness")).weak());
        } else if sweep > capped + 1e-6 {
            ui.label(RichText::new(trf!("The arch needs {angle}° or more to stay 1 u thick", angle = format!("{sweep:.1}"))).weak());
        } else if capped < wanted - 1e-6 {
            ui.label(RichText::new(trf!("The box height caps the sweep at {angle}°", angle = format!("{sweep:.1}"))).weak());
        }

        if let Some(crown) = params.arch_crown(bounds).filter(|_| tunnel) {
            ui.label(RichText::new(trf!("The arch is {thickness} u thick at the top", thickness = format!("{crown:.1}"))).weak());
        }

        if self.kind == ShapeKind::Arch && sweep < 180.0 {
            let rise = arch_rise(span, sweep);
            ui.label(RichText::new(trf!("The arch rises {height} u", height = format!("{rise:.1}"))).weak());
        }

        let (min, max) = (span * 0.5, shapes::arch_radius(span, shapes::ARCH_MIN_ANGLE).max(span * 0.5));
        ui.horizontal(|ui| {
            let mut radius = shapes::arch_radius(span, sweep).clamp(min, max);
            ui.label(tr("Radius")).on_hover_text(tr("Radius of the arch circle in map units, linked to the angle"));
            // No range on the field, it would cut a typed radius back before the dialog could say so.
            let field = ui.add(egui::DragValue::new(&mut radius).suffix(" u"));
            if field.on_hover_text(tr("At least half the span. At 180 the arch is a half ellipse and this shows half the span")).changed() {
                self.angle = shapes::arch_angle_for_radius(span, radius.clamp(min, max));
                self.radius_cut = (radius < min || radius > max).then_some((span, self.angle));
            }
        });
        if self.radius_cut.is_some_and(|cut| cut != (span, self.angle)) {
            self.radius_cut = None;
        }

        if self.radius_cut.is_some() {
            let (min, max) = (format!("{min:.1}"), format!("{max:.1}"));
            ui.label(RichText::new(trf!("The radius stays between {min} and {max} u", min = min, max = max)).weak());
        }
    }

    fn tunnel_fields(&mut self, ui: &mut egui::Ui, bounds: &Aabb) {
        let square = self.tunnel.profile == shapes::TunnelProfile::Square;
        if square || self.tunnel.profile == shapes::TunnelProfile::Arched {
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.tunnel.floor, tr("Floor")).on_hover_text(tr("Closes the bottom with a slab as thick as the walls"));
                if square {
                    ui.checkbox(&mut self.tunnel.ceiling, tr("Ceiling")).on_hover_text(tr("Closes the top, without it a square tunnel is an open trench"));
                }
            });
        }

        ui.checkbox(&mut self.tunnel.walkway, tr("Walkway"))
            .on_hover_text(tr("Raised walkways along both walls with a sunken channel between them, for sewers and culverts"));
        if self.tunnel.walkway {
            // An infinite want makes `fitted` hand back the most the box allows.
            let params = self.tunnel_params(bounds);
            let width_max = shapes::Tunnel { walkway_width: f64::INFINITY, ..params }.fitted(bounds).walkway_width;
            let height_max = shapes::Tunnel { walkway_height: f64::INFINITY, ..params }.fitted(bounds).walkway_height;
            let fitted = params.fitted(bounds);
            let (mut width, mut height) = (fitted.walkway_width, fitted.walkway_height);
            ui.horizontal(|ui| {
                let field = egui::DragValue::new(&mut width).range(1.0_f64.min(width_max)..=width_max);
                if ui.add(field.prefix(format!("{} ", tr(en!("width")))).suffix(" u")).changed() {
                    self.tunnel.walkway_width = width;
                }

                let field = egui::DragValue::new(&mut height).range(1.0_f64.min(height_max)..=height_max);
                if ui.add(field.prefix(format!("{} ", tr(en!("height")))).suffix(" u")).changed() {
                    self.tunnel.walkway_height = height;
                }
            });
        }
    }

    /// The axis, bend and slope rows. The bend rows only show while there is a bend.
    fn tunnel_path_fields(&mut self, ui: &mut egui::Ui, bounds: &Aabb) {
        ui.horizontal(|ui| {
            ui.label(tr("Axis"));
            for a in shapes::Axis::ALL {
                ui.selectable_value(&mut self.tunnel.axis, a, a.label());
            }
        })
        .response
        .on_hover_text(tr("The axis the tunnel runs along, Y makes a vertical shaft"));
        let across = shapes::Toward::across(self.tunnel.axis);
        if !across.contains(&self.tunnel.toward) {
            self.tunnel.toward = shapes::Toward::default_for(self.tunnel.axis);
        }

        ui.add(egui::DragValue::new(&mut self.tunnel.bend).range(0.0..=180.0).prefix(format!("{} ", tr(en!("bend")))).suffix("°"))
            .on_hover_text(tr("Turns the far end in an exact arc, 0 keeps the tunnel straight"));
        let fitted = self.tunnel_params(bounds).fitted(bounds);
        if fitted.bend > 0.0 {
            ui.indent("tunnel_bend", |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr("Direction")).on_hover_text(tr("Which way the far end turns"));
                    for t in across {
                        ui.selectable_value(&mut self.tunnel.toward, t, t.label());
                    }
                });
                ui.add(egui::DragValue::new(&mut self.tunnel.bend_radius).range(0.0..=65536.0).prefix(format!("{} ", tr(en!("inner radius")))).suffix(" u"))
                    .on_hover_text(tr("Distance from the center of the turn to the inside wall"));
                let least = (fitted.bend / 90.0).ceil().max(1.0) as usize;
                let mut segments = fitted.bend_segments;
                if ui.add(egui::Slider::new(&mut segments, least..=64).text(tr(en!("bend segments")))).changed() {
                    self.tunnel.bend_segments = segments;
                }

                if fitted.bend != 90.0 && fitted.bend != 180.0 {
                    ui.label(RichText::new(tr("Only 90° and 180° bends end on the grid")).weak());
                }
            });
        }

        let length = self.tunnel_length(bounds);
        let mut rise = fitted.rise;
        let field = ui.add(egui::DragValue::new(&mut rise).range(-length * 2.0..=length * 2.0).prefix(format!("{} ", tr(en!("slope")))).suffix(" u"));
        if field.on_hover_text(tr("How far the far end climbs over the box length, negative runs downhill")).changed() {
            self.tunnel.rise = rise;
        }

        if fitted.rise != 0.0 && length > 0.0 {
            let angle = (fitted.rise / length).atan().to_degrees();
            ui.label(RichText::new(trf!("The floor climbs at {angle}°", angle = format!("{angle:.1}"))).weak());
        }
    }

    /// The box's size along the tunnel's axis.
    fn tunnel_length(&self, bounds: &Aabb) -> f64 {
        let size = bounds.size();
        match self.tunnel.axis {
            shapes::Axis::X => size.x,
            shapes::Axis::Y => size.y,
            shapes::Axis::Z => size.z,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        if !self.open {
            if self.preview.take().is_some() {
                state.shape_preview.clear();
            }

            self.bounds = None;
            self.made.clear();
            state.shape_bounds = None;
            return;
        }

        let mut open = self.open;
        self.grid = state.effective_grid();
        let (sources, mut bounds) = self.sources_and_bounds(state);
        widgets::top_window(en!("Shape Generator"), ctx).open(&mut open).resizable(false).show(ctx, |ui| {
            // The default menu height shows 7 of the shapes with no sign that more follow.
            let menu_height = ctx.content_rect().height() * 0.7;
            self.preset_row(ui, &mut state.prefs.shape_presets);
            egui::ComboBox::new("shape_kind", tr("Shape")).selected_text(tr(self.kind.label())).height(menu_height).show_ui(ui, |ui| {
                for k in ShapeKind::ALL {
                    ui.selectable_value(&mut self.kind, k, tr(k.label()));
                }
            });
            if self.kind == ShapeKind::Tunnel {
                egui::ComboBox::new("shape_profile", tr("Profile"))
                    .selected_text(tr(profile_label(self.tunnel.profile)))
                    .show_ui(ui, |ui| {
                        for p in shapes::TunnelProfile::ALL {
                            ui.selectable_value(&mut self.tunnel.profile, p, tr(profile_label(p)));
                        }
                    })
                    .response
                    .on_hover_text(tr("How the tunnel is cut across its length"));
            }

            let arched = self.kind == ShapeKind::Tunnel && self.tunnel.profile == shapes::TunnelProfile::Arched;
            let round = self.kind == ShapeKind::Tunnel && self.tunnel.profile == shapes::TunnelProfile::Round;
            if arched
                || round
                || matches!(
                    self.kind,
                    ShapeKind::Cylinder
                        | ShapeKind::Cone
                        | ShapeKind::Sphere
                        | ShapeKind::Arch
                        | ShapeKind::Pipe
                        | ShapeKind::Torus
                        | ShapeKind::ArchWall
                        | ShapeKind::Pillar
                )
            {
                let segments = arched || matches!(self.kind, ShapeKind::Arch | ShapeKind::ArchWall);
                ui.add(egui::Slider::new(&mut self.sides, 3..=96).text(tr(if segments { en!("segments") } else { en!("sides") })));
            }

            if matches!(self.kind, ShapeKind::Arch | ShapeKind::ArchWall) || arched {
                self.arch_fields(ui, &bounds);
            }

            if matches!(
                self.kind,
                ShapeKind::Arch | ShapeKind::Tunnel | ShapeKind::Pipe | ShapeKind::Torus | ShapeKind::ArchWall | ShapeKind::GableRoof | ShapeKind::SpiralStairs
            ) {
                let max = self.thickness_max(&bounds);
                let mut thickness = self.thickness_in(&bounds);
                let label = tr(if self.kind == ShapeKind::SpiralStairs { en!("inner radius") } else { en!("thickness") });
                if ui.add(egui::DragValue::new(&mut thickness).range(1.0_f64.min(max)..=max).prefix(format!("{label} ")).suffix(" u")).changed() {
                    self.thickness = thickness;
                }
            }

            if self.kind == ShapeKind::Tunnel {
                self.tunnel_fields(ui, &bounds);
                self.tunnel_path_fields(ui, &bounds);
            }

            if matches!(self.kind, ShapeKind::Cylinder | ShapeKind::Cone) {
                ui.horizontal(|ui| {
                    ui.label(tr("Axis"));
                    for a in shapes::Axis::ALL {
                        ui.selectable_value(&mut self.axis, a, a.label());
                    }
                })
                .response
                .on_hover_text(tr("The axis the shape runs along, Y stands it upright, X or Z lays it down like a pipe"));
            }

            if matches!(self.kind, ShapeKind::Stairs | ShapeKind::SpiralStairs | ShapeKind::Grid) {
                ui.add(egui::Slider::new(&mut self.steps, 1..=128).text(tr(if self.kind == ShapeKind::Grid { en!("divisions") } else { en!("steps") })));
            }

            if !self.kind.mesh_only() && matches!(self.kind, ShapeKind::Cylinder | ShapeKind::Cone | ShapeKind::Sphere | ShapeKind::Rock) {
                ui.checkbox(&mut self.as_mesh, tr("Create as editable mesh"));
            }

            if self.as_mesh || matches!(self.kind, ShapeKind::Torus) {
                ui.checkbox(&mut self.smooth, tr("Smooth shading"));
            }

            if self.kind == ShapeKind::Rock {
                let r = &mut self.rock;
                egui::Grid::new("shape_rock").num_columns(2).show(ui, |ui| {
                    ui.label(tr("Roughness"));
                    ui.add(egui::Slider::new(&mut r.roughness, 0.0..=1.0));
                    ui.end_row();
                    ui.label(tr("Facets"));
                    ui.add(egui::Slider::new(&mut r.facets, 0..=rock::MAX_FACETS)).on_hover_text(tr("Flat chipped faces cut into the surface"));
                    ui.end_row();
                    ui.label(tr("Flat base"));
                    ui.add(egui::Slider::new(&mut r.flatten, 0.0..=1.0));
                    ui.end_row();
                    if self.as_mesh {
                        ui.label(tr("Detail"));
                        ui.add(egui::Slider::new(&mut r.detail, 1..=rock::MAX_DETAIL));
                        ui.end_row();
                    }

                    ui.label(tr("Seed"));
                    ui.add(egui::DragValue::new(&mut r.seed));
                    ui.end_row();
                });
            }

            if self.kind == ShapeKind::Text {
                let t = &mut self.text;
                egui::Grid::new("shape_text").num_columns(2).show(ui, |ui| {
                    ui.label(tr("Text"));
                    ui.add(egui::TextEdit::multiline(&mut t.text).desired_rows(2).desired_width(180.0))
                        .on_hover_text(tr("A-Z, 0-9, space and . , ! ? ' - + : /, Enter starts a new line"));
                    ui.end_row();
                    ui.label(tr("Letter height"));
                    ui.add(egui::DragValue::new(&mut t.letter_height).range(1.0..=4096.0).suffix(" u"));
                    ui.end_row();
                    ui.label(tr("Depth"));
                    ui.add(egui::DragValue::new(&mut t.depth).range(0.125..=4096.0).suffix(" u"));
                    ui.end_row();
                    ui.label(tr("Spacing"));
                    ui.add(egui::DragValue::new(&mut t.spacing).range(0.0..=10.0).speed(0.05)).on_hover_text(tr("Gap between letters in font cells"));
                    ui.end_row();
                    ui.label(tr("Orientation"));
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut t.standing, true, tr("standing"));
                        ui.selectable_value(&mut t.standing, false, tr("lying"));
                    });
                    ui.end_row();
                });
                if let Err(e) = t.brushes(bounds.min, &state.current_material) {
                    ui.colored_label(ui.visuals().warn_fg_color, e);
                }
            }

            if sources.is_empty() {
                if box_fields(ui, &mut bounds, self.kind != ShapeKind::Text, state.effective_grid()) {
                    state.shape_bounds = Some(bounds);
                    self.bounds = Some(bounds);
                }

                ui.label(RichText::new(tr(en!("Drag in a 2D view to draw the box, or drag its sides"))).weak());
            } else {
                ui.label(bounds_label(self.kind, &bounds, tr(en!("replaces selection"))));
            }

            let key = self.key(&bounds, &state.current_material);
            if self.preview.as_ref().is_none_or(|(k, ..)| *k != key) {
                let kinds = self.generate(&bounds, &state.current_material);
                state.shape_preview = preview_lines(&kinds);
                let tris = triangles(&kinds);
                self.preview = Some((key, kinds, tris));
            }

            let (count, tris) = self.preview.as_ref().map_or((0, 0), |(_, p, t)| (p.len(), *t));
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr_n!(count, "{n} object", "{n} objects")).weak());
                let tris_text = tr_n!(tris, "{n} triangle", "{n} triangles");
                if tris > HEAVY_TRIANGLES {
                    ui.colored_label(ui.visuals().warn_fg_color, tris_text).on_hover_text(tr("A shape this heavy can slow the editor down"));
                } else {
                    ui.label(RichText::new(tris_text).weak());
                }
            });
            if ui.add_enabled(count > 0, egui::Button::new(tr("Create"))).clicked() {
                let kinds = self.preview.as_ref().map(|(_, p, _)| p.clone()).unwrap_or_default();
                let ids = create_shape(state, &sources, kinds);
                self.made.push(Made { bounds: state.doc.map.bounds_of(ids.iter().copied()), ids, source: bounds });
            }
        });
        self.open = open;
    }
}

/// How high an arch under 180 degrees stands above its floor, which is less than its box.
fn arch_rise(span: f64, sweep: f64) -> f64 {
    let radius = shapes::arch_radius(span, sweep);
    radius - radius * (sweep.to_radians() * 0.5).cos()
}

/// The thickest wall a tunnel along `axis` in `bounds` gets, rounded down to the grid while that leaves at least 1.
fn tunnel_thickness_max(bounds: &Aabb, grid: f64, axis: shapes::Axis) -> f64 {
    let cap = shapes::Tunnel { axis, ..Default::default() }.thickness_cap(bounds);
    let snapped = if grid > 0.0 { (cap / grid).floor() * grid } else { 0.0 };
    if snapped >= 1.0 { snapped } else { cap }
}

/// Puts the shape where `sources` were, in their parent, and selects it.
fn create_shape(state: &mut EditorState, sources: &[td_doc::NodeId], kinds: Vec<NodeKind>) -> Vec<td_doc::NodeId> {
    let parent = state.insert_parent();
    state.doc.edit(en!("Create Shape"), |m, s| {
        let target_parent = sources.first().and_then(|b| m.get(*b)).and_then(|n| n.parent).unwrap_or(parent);
        for id in sources {
            m.remove(*id);
        }

        s.clear();
        kinds
            .into_iter()
            .map(|kind| {
                let id = m.insert(target_parent, kind);
                s.nodes.insert(id);
                id
            })
            .collect()
    })
}

pub struct TerrainDialog {
    pub open: bool,
    resolution: u32,
    cell_size: f64,
    params: TerrainGen,
    layers: [String; 4],
    tiles: [f64; 4],
    auto_paint: bool,
    heightmap: Option<std::path::PathBuf>,
    /// Center of the new terrain, its y is the height of the terrain's zero. None places it at the 3D cursor.
    center: Option<[f64; 3]>,
}

impl Default for TerrainDialog {
    fn default() -> Self {
        Self {
            open: false,
            resolution: 129,
            cell_size: 64.0,
            params: TerrainGen::default(),
            layers: ["dev/green".into(), "dev/grey".into(), "dev/blue".into(), "dev/orange".into()],
            tiles: [256.0; 4],
            auto_paint: true,
            heightmap: None,
            center: None,
        }
    }
}

/// The base Auto Paint measures its height bands from: the lowest point, or a sea level the user sets.
pub fn auto_paint_base_ui(ui: &mut egui::Ui, base: &mut Option<f64>) {
    ui.horizontal(|ui| {
        let mut sea = base.is_some();
        if ui
            .checkbox(&mut sea, tr("Bands from sea level"))
            .on_hover_text(tr("Measures the peak and low bands from this height instead of the lowest point"))
            .changed()
        {
            *base = sea.then_some(0.0);
        }

        if let Some(h) = base {
            ui.add(egui::DragValue::new(h).speed(1.0).prefix("y "));
        }
    });
}

/// Builds a terrain from dialog style parameters. `heightmap` values are 0..1.
#[allow(clippy::too_many_arguments)]
pub fn make_terrain(
    origin: DVec3,
    resolution: u32,
    cell_size: f64,
    params: &TerrainGen,
    layers: &[(String, f64)],
    heightmap: Option<(&[f32], [u32; 2])>,
    auto_paint: bool,
    sea_level: Option<f64>,
) -> Terrain {
    let mut t = Terrain::new(origin, [resolution, resolution], cell_size, "");
    t.layers =
        layers.iter().filter(|(m, _)| !m.is_empty()).take(td_geom::heightfield::MAX_LAYERS).map(|(m, tile)| TerrainLayer::new(m.clone(), *tile)).collect();
    match heightmap {
        Some((values, size)) => {
            t.import_heightmap(values, size, params.height);
            t.erode(params.erosion_iterations, cell_size * 0.9);
        }
        None => t.generate(params),
    }

    if auto_paint && t.layers.len() > 1 {
        t.auto_paint_from(sea_level);
    }

    t
}

fn terrain_shape_label(shape: TerrainShape) -> &'static str {
    match shape {
        TerrainShape::Flat => en!("Flat"),
        TerrainShape::Hills => en!("Hills"),
        TerrainShape::Mountain => en!("Mountain"),
        TerrainShape::Island => en!("Island"),
        TerrainShape::Valley => en!("Valley"),
        TerrainShape::Ridges => en!("Ridges"),
    }
}

impl TerrainDialog {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        let mut open = self.open;
        widgets::centered_window(en!("Create Terrain"), ctx).open(&mut open).resizable(false).show(ctx, |ui| {
            egui::Grid::new("terrain_grid").num_columns(2).show(ui, |ui| {
                ui.label(tr("Resolution"));
                egui::ComboBox::from_id_salt("terrain_res").selected_text(format!("{0} x {0}", self.resolution)).show_ui(ui, |ui| {
                    for r in [33, 65, 129, 257, 513, 1025] {
                        ui.selectable_value(&mut self.resolution, r, format!("{r} x {r}"));
                    }
                });
                ui.end_row();
                ui.label(tr("Cell size"));
                ui.add(egui::DragValue::new(&mut self.cell_size).range(4.0..=1024.0).suffix(" u"));
                ui.end_row();
                ui.label(tr("Size"));
                let side = (self.resolution - 1) as f64 * self.cell_size;
                ui.label(trf!("{side} units ({meters} m)", side, meters = format!("{:.0}", side / state.game.units_per_meter)));
                ui.end_row();
                ui.label(tr("Shape"));
                egui::ComboBox::from_id_salt("terrain_shape").selected_text(tr(terrain_shape_label(self.params.shape))).show_ui(ui, |ui| {
                    for s in [TerrainShape::Flat, TerrainShape::Hills, TerrainShape::Mountain, TerrainShape::Island, TerrainShape::Valley, TerrainShape::Ridges]
                    {
                        ui.selectable_value(&mut self.params.shape, s, tr(terrain_shape_label(s)));
                    }
                });
                ui.end_row();
                ui.label(tr("Height"));
                ui.add(egui::DragValue::new(&mut self.params.height).range(0.0..=65536.0).suffix(" u"));
                ui.end_row();
                ui.label(tr("Feature size"));
                ui.add(egui::DragValue::new(&mut self.params.feature_size).range(16.0..=65536.0).suffix(" u"));
                ui.end_row();
                ui.label(tr("Octaves / roughness"));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.params.octaves).range(1..=10));
                    ui.add(egui::Slider::new(&mut self.params.roughness, 0.1..=0.9));
                });
                ui.end_row();
                ui.label(tr("Erosion"));
                ui.add(egui::DragValue::new(&mut self.params.erosion_iterations).range(0..=200));
                ui.end_row();
                ui.label(tr("Seed"));
                ui.add(egui::DragValue::new(&mut self.params.seed));
                ui.end_row();
                for (i, name) in [en!("Base layer"), en!("Slope layer"), en!("Peak layer"), en!("Low layer")].into_iter().enumerate() {
                    ui.label(tr(name));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.layers[i]).desired_width(140.0));
                        if ui.button(tr("Use Current")).clicked() {
                            self.layers[i] = state.current_material.clone();
                        }

                        ui.add(egui::DragValue::new(&mut self.tiles[i]).range(8.0..=8192.0).prefix(format!("{} ", tr("tile"))).suffix(" u"));
                    });
                    ui.end_row();
                }

                ui.label(tr("Center"));
                ui.horizontal(|ui| {
                    let mut at_cursor = self.center.is_none();
                    if ui.checkbox(&mut at_cursor, tr("3D cursor")).changed() {
                        let cursor = state.cursor_world.map(|c| state.snap(c)).unwrap_or(DVec3::ZERO);
                        // Adding 0.0 turns a snapped -0.0 into 0.0, which the field would show as "-0".
                        self.center = (!at_cursor).then(|| cursor.to_array().map(|v| v + 0.0));
                    }

                    if let Some(c) = &mut self.center {
                        for (v, axis) in c.iter_mut().zip(["x ", "y ", "z "]) {
                            ui.add(egui::DragValue::new(v).speed(8.0).prefix(axis));
                        }
                    }
                });
                ui.end_row();
                ui.label(tr("Heightmap"));
                ui.horizontal(|ui| {
                    ui.label(
                        self.heightmap
                            .as_ref()
                            .map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                            .unwrap_or_else(|| tr("procedural").into()),
                    );
                    if ui.button(tr("Import PNG…")).clicked() {
                        self.heightmap = commands::file_dialog(state, |d| d.add_filter(tr("Heightmap"), &["png", "tga", "bmp"]).pick_file());
                    }

                    if self.heightmap.is_some() && ui.button(tr("Clear")).clicked() {
                        self.heightmap = None;
                    }
                });
                ui.end_row();
            });
            ui.checkbox(&mut self.auto_paint, tr("Paint layers from slope and height"));
            if self.auto_paint {
                auto_paint_base_ui(ui, &mut state.auto_paint_base);
            }

            if ui.button(tr("Create")).clicked() {
                let side = (self.resolution - 1) as f64 * self.cell_size;
                let cursor = || state.cursor_world.map(|c| state.snap(c)).unwrap_or(DVec3::ZERO);
                let center = self.center.map(DVec3::from_array).unwrap_or_else(cursor);
                let origin = DVec3::new(center.x - side * 0.5, center.y, center.z - side * 0.5);
                let image = self.heightmap.as_ref().and_then(|p| image::open(p).ok()).map(|img| {
                    let gray = img.to_luma16();
                    let values: Vec<f32> = gray.pixels().map(|p| p.0[0] as f32 / 65535.0).collect();
                    (values, [gray.width(), gray.height()])
                });
                let layers: Vec<(String, f64)> = self.layers.iter().cloned().zip(self.tiles).collect();
                let t = make_terrain(
                    origin,
                    self.resolution,
                    self.cell_size,
                    &self.params,
                    &layers,
                    image.as_ref().map(|(v, s)| (v.as_slice(), *s)),
                    self.auto_paint,
                    state.auto_paint_base,
                );
                let parent = state.insert_parent();
                state.doc.edit(en!("Create Terrain"), |m, s| {
                    let id = m.insert(parent, NodeKind::Terrain(t));
                    s.clear();
                    s.select_node(id);
                });
                state.tool = ToolKind::Sculpt;
                self.open = false;
            }
        });
        if !open {
            self.open = false;
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum KeymapTab {
    #[default]
    Keys,
    Mouse,
    ToolKeys,
}

#[derive(Default)]
pub struct KeymapWindow {
    pub open: bool,
    pub tab: KeymapTab,
    filter: String,
    tool_filter: String,
    recording: Option<String>,
    searching: bool,
    key_search: Option<egui::KeyboardShortcut>,
    taken: Option<TakenChord>,
}

/// The mouse gestures of `preset`, one grid per place they apply. Read only, the rows come from the click rules.
fn mouse_tab(ui: &mut egui::Ui, preset: &str) {
    let rows = crate::mouse_map::mouse_rows(preset);
    let mut contexts: Vec<&str> = Vec::new();
    for row in &rows {
        if !contexts.contains(&row.context) {
            contexts.push(row.context);
        }
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let column = ((ui.available_width() - ui.spacing().item_spacing.x) / 2.0).max(80.0);
        for context in contexts {
            ui.label(RichText::new(tr(context)).strong());
            egui::Grid::new(("mouse", context)).striped(true).num_columns(2).min_col_width(column).max_col_width(column).show(ui, |ui| {
                for row in rows.iter().filter(|r| r.context == context) {
                    ui.label(&row.gesture);
                    ui.label(tr(row.what));
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
        }
    });
}

/// A recorded chord another action holds, waiting for Replace, Swap or Cancel.
struct TakenChord {
    id: String,
    chord: egui::KeyboardShortcut,
    holder: Option<Action>,
    tools: Vec<ToolKind>,
}

/// Everything that reacts to `chord`'s key, with any modifier: the bound actions in the grid's order, then the keys
/// the tools read, the preset's own included. Rows with exactly the pressed chord come first in each list.
pub fn key_uses(prefs: &crate::state::Prefs, chord: &egui::KeyboardShortcut) -> (Vec<Action>, Vec<&'static ToolKeyRow>) {
    let exact = commands::shortcut_to_text(chord);
    let active = commands::shortcuts(prefs);
    let mut actions: Vec<(bool, Action)> = commands::bindable_actions()
        .into_iter()
        .filter_map(|action| {
            let mut uses = active.iter().filter(|(s, a)| *a == action && s.logical_key == chord.logical_key).peekable();
            uses.peek()?;
            Some((uses.any(|(s, _)| commands::shortcut_to_text(s) == exact), action))
        })
        .collect();
    actions.sort_by_key(|(is_exact, _)| !is_exact);
    let mut rows: Vec<&ToolKeyRow> = tool_key_rows().iter().chain(preset_rows(&prefs.keymap_preset)).filter(|r| r.key == chord.logical_key).collect();
    rows.sort_by_key(|r| r.chord() != exact);
    (actions.into_iter().map(|(_, a)| a).collect(), rows)
}

/// Takes the next key press of the frame, and whether Esc was pressed. Nothing else sees the keys that frame.
fn capture_chord(ctx: &egui::Context) -> (Option<egui::KeyboardShortcut>, bool) {
    ctx.input_mut(|i| {
        let found = i.events.iter().find_map(|e| match e {
            egui::Event::Key { key, pressed: true, modifiers, .. } if !matches!(key, Key::Escape) => Some(egui::KeyboardShortcut::new(*modifiers, *key)),
            _ => None,
        });
        let escape = i.consume_key(egui::Modifiers::NONE, Key::Escape);
        i.events.clear();
        (found, escape)
    })
}

impl KeymapWindow {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        if !self.open {
            return;
        }

        if let Some(id) = self.recording.clone() {
            match capture_chord(ctx) {
                (_, true) => self.recording = None,
                (Some(s), _) => {
                    let holder = commands::chord_conflict(&state.prefs, &id, &s);
                    let tools = tools_reading(&s);
                    if holder.is_none() && tools.is_empty() {
                        commands::apply_rebind(&mut state.prefs, &id, &s, commands::Resolve::Replace);
                    } else {
                        self.taken = Some(TakenChord { id, chord: s, holder, tools });
                    }

                    self.recording = None;
                }
                _ => {}
            }
        }

        if self.searching {
            match capture_chord(ctx) {
                (_, true) => self.searching = false,
                (Some(s), _) => {
                    self.key_search = Some(s);
                    self.searching = false;
                }
                _ => {}
            }
        }

        let mut open = self.open;
        widgets::centered_window(en!("Keyboard Shortcuts"), ctx).open(&mut open).default_size([520.0, 520.0]).show(ctx, |ui| {
            let columns = KeyColumns::measure(ui, &state.prefs);
            ui.set_min_width(columns.width(ui).ceil());
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, KeymapTab::Keys, tr("Keys"));
                ui.selectable_value(&mut self.tab, KeymapTab::Mouse, tr("Mouse"));
                ui.selectable_value(&mut self.tab, KeymapTab::ToolKeys, tr("Tool keys"));
            });
            if self.tab == KeymapTab::ToolKeys {
                self.tool_keys_tab(ui, &state.prefs.keymap_preset);
                return;
            }

            if self.tab == KeymapTab::Mouse {
                ui.horizontal(|ui| self.preset_combo(ui, state));
                ui.separator();
                mouse_tab(ui, &state.prefs.keymap_preset);
                return;
            }

            ui.horizontal(|ui| {
                self.preset_combo(ui, state);
                if ui.button(tr("Reset overrides")).clicked() {
                    state.prefs.key_overrides.clear();
                    self.taken = None;
                }

                self.search_controls(ui);
            });
            let prompt = if self.recording.is_some() {
                tr("Press the new shortcut (Esc cancels)")
            } else if self.searching {
                tr("Press the key to look up (Esc cancels)")
            } else {
                ""
            };
            // The line stays while empty, so the rows below do not move under the pointer when a prompt shows.
            ui.label(RichText::new(prompt).color(crate::theme::colors().yellow));
            ui.separator();
            let active = commands::shortcuts(&state.prefs);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.key_search {
                Some(chord) => {
                    let (actions, tool_rows) = key_uses(&state.prefs, &chord);
                    if actions.is_empty() && tool_rows.is_empty() {
                        ui.label(tr("Nothing uses this key"));
                    }

                    self.keys_grid(ui, state, &actions, &tool_rows, &active);
                }
                None => {
                    let actions: Vec<Action> = commands::bindable_actions()
                        .into_iter()
                        .filter(|a| self.filter.is_empty() || fuzzy_score(&self.filter, &a.title()).or_else(|| fuzzy_score(&self.filter, &a.label())).is_some())
                        .collect();
                    self.keys_grid(ui, state, &actions, &[], &active);
                }
            });
        });
        self.open = open;
    }

    fn preset_combo(&mut self, ui: &mut egui::Ui, state: &mut EditorState) {
        ui.label(tr("Preset"));
        let before = state.prefs.keymap_preset.clone();
        egui::ComboBox::from_id_salt("keymap_preset").selected_text(commands::preset_label(&state.prefs.keymap_preset)).show_ui(ui, |ui| {
            for p in commands::PRESETS {
                ui.selectable_value(&mut state.prefs.keymap_preset, p.to_string(), commands::preset_label(p));
            }
        });
        if state.prefs.keymap_preset != before {
            self.taken = None;
        }
    }

    fn search_controls(&mut self, ui: &mut egui::Ui) {
        // The filter and the key chip differ in height, so both sit in a slot of fixed size that the row is laid out
        // around, or swapping them would nudge the Press a key button.
        let (slot, _) = ui.allocate_exact_size(egui::vec2(160.0, ui.spacing().interact_size.y), egui::Sense::hover());
        let mut slot_ui = ui.new_child(egui::UiBuilder::new().max_rect(slot).layout(egui::Layout::left_to_right(egui::Align::Center)));
        match self.key_search {
            Some(chord) => {
                slot_ui.label(RichText::new(commands::shortcut_to_text(&chord)).monospace());
                if slot_ui.small_button("x").on_hover_text(tr("Clear the key search")).clicked() {
                    self.key_search = None;
                }
            }
            None => {
                slot_ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(tr("filter")).desired_width(f32::INFINITY));
            }
        }

        if ui.button(tr("Press a key")).on_hover_text(tr("List every use of the next key you press")).clicked() {
            self.searching = true;
            self.recording = None;
            self.taken = None;
        }
    }

    /// The command rows, then the tool rows of a key search. The prompt for a taken chord sits between two grids so it
    /// can use the full width, and every column keeps the width of its widest entry over all commands.
    fn keys_grid(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut EditorState,
        actions: &[Action],
        tools: &[&ToolKeyRow],
        active: &[(egui::KeyboardShortcut, Action)],
    ) {
        let columns = KeyColumns::measure(ui, &state.prefs);
        let what_width = (ui.available_width() - columns.title - columns.keys - 2.0 * ui.spacing().item_spacing.x).max(columns.buttons);
        let taken_at = self.taken.as_ref().and_then(|t| actions.iter().position(|a| a.binding_id() == t.id));
        let (above, below) = actions.split_at(taken_at.map_or(actions.len(), |i| i + 1));
        egui::Grid::new("keymap").striped(true).num_columns(3).show(ui, |ui| {
            for action in above {
                self.key_row(ui, state, action, active, &columns);
            }
        });
        if taken_at.is_some() {
            self.taken_row(ui, state);
        }

        if below.is_empty() && tools.is_empty() {
            return;
        }

        egui::Grid::new("keymap_below").striped(true).num_columns(3).start_row(above.len()).show(ui, |ui| {
            for action in below {
                self.key_row(ui, state, action, active, &columns);
            }

            if !tools.is_empty() {
                ui.label(RichText::new(tr("Tool keys")).strong());
                ui.end_row();
            }

            for row in tools {
                fixed_cell(ui, columns.title, |ui| {
                    ui.label(tr(row.tool.label()));
                });
                fixed_cell(ui, columns.keys, |ui| {
                    ui.label(RichText::new(row.chord()).monospace());
                });
                // Laid out up front, since the grid centers each cell on its row and a cell that grows as it fills would
                // start at the middle.
                let what = egui::WidgetText::from(describe(row)).into_galley(ui, Some(egui::TextWrapMode::Wrap), what_width, egui::TextStyle::Body);
                ui.label(what);
                ui.end_row();
            }
        });
    }

    fn key_row(&mut self, ui: &mut egui::Ui, state: &mut EditorState, action: &Action, active: &[(egui::KeyboardShortcut, Action)], columns: &KeyColumns) {
        let id = action.binding_id();
        fixed_cell(ui, columns.title, |ui| {
            let title = ui.label(action.title());
            if let Some(help) = action.help() {
                title.on_hover_text(tr(help));
            }
        });
        fixed_cell(ui, columns.keys, |ui| {
            ui.label(RichText::new(keys_text(active, action)).monospace());
        });
        fixed_cell(ui, columns.buttons, |ui| {
            let recording = self.recording.as_deref() == Some(id.as_str());
            if ui.add(egui::Button::new(tr("Set")).small().selected(recording)).clicked() {
                self.recording = Some(id.clone());
                self.searching = false;
                self.taken = None;
            }

            if ui.small_button(tr("Clear")).clicked() {
                state.prefs.key_overrides.insert(id.clone(), String::new());
                self.taken = None;
            }

            if commands::differs_from_preset(&state.prefs, action) && ui.small_button(tr("Default")).clicked() {
                commands::restore_default(&mut state.prefs, action);
                self.taken = None;
            }
        });
        ui.end_row();
    }

    fn taken_row(&mut self, ui: &mut egui::Ui, state: &mut EditorState) {
        let Some(taken) = self.taken.as_ref() else { return };
        let can_swap = commands::swap_chord(&state.prefs, &taken.id).is_some();
        let chord = commands::shortcut_to_text(&taken.chord);
        let mut lines: Vec<String> = taken.holder.iter().map(|a| trf!("{chord} is used by {action}.", chord = chord.as_str(), action = a.title())).collect();
        lines.extend(taken.tools.iter().map(|t| trf!("While the {tool} tool is active it takes {chord} first.", tool = tr(t.label()), chord = chord.as_str())));
        ui.label(RichText::new(lines.join("\n")).color(crate::theme::colors().yellow));
        let mut resolve = None;
        ui.horizontal(|ui| {
            if taken.holder.is_none() {
                if ui.small_button(tr("Set anyway")).clicked() {
                    resolve = Some(Some(commands::Resolve::Replace));
                }
            } else {
                if ui.small_button(tr("Replace")).clicked() {
                    resolve = Some(Some(commands::Resolve::Replace));
                }

                if ui.add_enabled(can_swap, egui::Button::new(tr("Swap")).small()).clicked() {
                    resolve = Some(Some(commands::Resolve::Swap));
                }
            }

            if ui.small_button(tr("Cancel")).clicked() {
                resolve = Some(None);
            }
        });
        if let Some(how) = resolve {
            if let Some(how) = how {
                commands::apply_rebind(&mut state.prefs, &taken.id, &taken.chord, how);
            }

            self.taken = None;
        }
    }

    fn tool_keys_tab(&mut self, ui: &mut egui::Ui, preset: &str) {
        ui.add(egui::TextEdit::singleline(&mut self.tool_filter).hint_text(tr("filter")).desired_width(160.0));
        ui.label(RichText::new(tr("Keys a tool reads while it is the active tool. They are fixed and cannot be rebound.")).weak());
        ui.separator();
        let chord_width = tool_key_rows().iter().chain(preset_rows(preset)).map(|r| text_width(ui, r.chord(), egui::TextStyle::Monospace)).fold(0.0, f32::max);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for tool in ToolKind::all() {
                let rows: Vec<&ToolKeyRow> = tool_key_rows()
                    .iter()
                    .chain(preset_rows(preset))
                    .filter(|r| r.tool == tool)
                    .filter(|r| {
                        self.tool_filter.is_empty()
                            || [tr(tool.label()), r.what(), r.chord().as_str()].iter().any(|t| fuzzy_score(&self.tool_filter, t).is_some())
                    })
                    .collect();
                if rows.is_empty() {
                    continue;
                }

                ui.label(RichText::new(tr(tool.label())).strong());
                egui::Grid::new(("tool_keys", tool.label())).striped(true).num_columns(2).show(ui, |ui| {
                    for row in rows {
                        fixed_cell(ui, chord_width, |ui| {
                            ui.label(RichText::new(row.chord()).monospace());
                        });
                        ui.label(describe(row));
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
            }
        });
    }
}

/// Column widths of the Keys tab, taken over every command and tool key of every preset so a filter, a key search, a
/// prompt or another preset leaves the buttons where they were. Every tab is as wide as these columns, so switching
/// tabs leaves the window as it is.
struct KeyColumns {
    title: f32,
    keys: f32,
    buttons: f32,
}

impl KeyColumns {
    fn measure(ui: &egui::Ui, prefs: &crate::state::Prefs) -> Self {
        let widest = |texts: Vec<String>, style: egui::TextStyle| texts.into_iter().map(|t| text_width(ui, t, style.clone())).fold(0.0, f32::max);
        let actions = commands::bindable_actions();
        let titles: Vec<String> = actions.iter().map(|a| a.title()).chain(ToolKind::all().iter().map(|t| tr(t.label()).to_string())).collect();
        let mut keys: Vec<String> = tool_key_rows().iter().map(|r| r.chord()).collect();
        for preset in commands::PRESETS.iter().copied().chain([prefs.keymap_preset.as_str()]) {
            let active = commands::shortcuts(&crate::state::Prefs { keymap_preset: preset.to_string(), ..prefs.clone() });
            keys.extend(actions.iter().map(|a| keys_text(&active, a)));
            keys.extend(preset_rows(preset).iter().map(|r| r.chord()));
        }

        let padding = 2.0 * ui.spacing().button_padding.x;
        let buttons = [tr("Set"), tr("Clear"), tr("Default")].iter().map(|t| text_width(ui, t.to_string(), egui::TextStyle::Body) + padding).sum::<f32>()
            + 2.0 * ui.spacing().item_spacing.x;
        Self { title: widest(titles, egui::TextStyle::Body), keys: widest(keys, egui::TextStyle::Monospace), buttons }
    }

    fn width(&self, ui: &egui::Ui) -> f32 {
        self.title + self.keys + self.buttons + 2.0 * ui.spacing().item_spacing.x
    }
}

fn text_width(ui: &egui::Ui, text: String, style: egui::TextStyle) -> f32 {
    ui.painter().layout_no_wrap(text, style.resolve(ui.style()), egui::Color32::WHITE).size().x
}

/// A grid cell at least `width` wide, so its column does not change width with the rows shown.
fn fixed_cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_width(width);
        add(ui);
    });
}

fn keys_text(active: &[(egui::KeyboardShortcut, Action)], action: &Action) -> String {
    let keys: Vec<String> = active.iter().filter(|(_, a)| a == action).map(|(s, _)| commands::shortcut_to_text(s)).collect();
    if keys.is_empty() { tr("unbound").to_string() } else { keys.join(", ") }
}
/// Connects an output of one selected entity to an input of the other.
#[derive(Default)]
pub struct LinkDialog {
    pub open: bool,
    from: Option<td_core::NodeId>,
    to: Option<td_core::NodeId>,
    output: String,
    input: String,
    parameter: String,
    delay: f64,
}

impl LinkDialog {
    pub fn open_for(&mut self, state: &EditorState) {
        let entities: Vec<td_core::NodeId> = state.doc.selection.nodes.iter().copied().filter(|id| state.doc.map.entity(*id).is_some()).collect();
        if entities.len() != 2 {
            return;
        }

        self.from = Some(entities[0]);
        self.to = Some(entities[1]);
        self.output.clear();
        self.input.clear();
        self.fit_choices(state);
        self.parameter.clear();
        self.delay = 0.0;
        self.open = true;
    }

    /// Swaps source and target. Outputs belong to the source class and inputs to the target, so choices that the new
    /// pair does not offer go back to the first option.
    pub fn swap(&mut self, state: &EditorState) {
        std::mem::swap(&mut self.from, &mut self.to);
        self.fit_choices(state);
    }

    fn fit_choices(&mut self, state: &EditorState) {
        let (Some(from), Some(to)) = (self.from, self.to) else { return };
        let (outs, ins) = crate::entity_wizards::link_options(state, from, to);
        if !outs.contains(&self.output) {
            self.output = outs.first().cloned().unwrap_or_default();
        }

        if !ins.contains(&self.input) {
            self.input = ins.first().cloned().unwrap_or_default();
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        let (Some(from), Some(to)) = (self.from, self.to) else { return };
        if !self.open {
            return;
        }

        if state.doc.map.entity(from).is_none() || state.doc.map.entity(to).is_none() {
            self.open = false;
            return;
        }

        let name = |id| state.doc.map.get(id).map(|n| n.name()).unwrap_or_default();
        let (from_name, to_name) = (name(from), name(to));
        let (outs, ins) = crate::entity_wizards::link_options(state, from, to);
        let mut open = self.open;
        let mut apply = false;
        let mut swap = false;
        widgets::centered_window(en!("Link Entities"), ctx).open(&mut open).resizable(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&from_name).strong());
                swap = ui.small_button("⇄").on_hover_text(tr("Swap")).clicked();

                ui.label(RichText::new(&to_name).strong());
            });
            egui::Grid::new("link_grid").num_columns(2).show(ui, |ui| {
                ui.label(tr("When"));
                egui::ComboBox::from_id_salt("link_out").selected_text(&self.output).show_ui(ui, |ui| {
                    for o in outs {
                        ui.selectable_value(&mut self.output, o.clone(), o);
                    }
                });
                ui.end_row();
                ui.label(tr("call"));
                egui::ComboBox::from_id_salt("link_in").selected_text(&self.input).show_ui(ui, |ui| {
                    for i in ins {
                        ui.selectable_value(&mut self.input, i.clone(), i);
                    }
                });
                ui.end_row();
                ui.label(tr("parameter"));
                ui.text_edit_singleline(&mut self.parameter);
                ui.end_row();
                ui.label(tr("delay"));
                ui.add(egui::DragValue::new(&mut self.delay).range(0.0..=3600.0).speed(0.05).suffix(" s"));
                ui.end_row();
            });
            apply = ui.button(tr("Link")).clicked();
        });
        if swap {
            self.swap(state);
        }

        if apply {
            if let (Some(from), Some(to)) = (self.from, self.to) {
                match crate::entity_wizards::link(state, from, to, &self.output, &self.input, &self.parameter, self.delay) {
                    Ok(c) => state.set_status(format!("{} > {}.{}", c.output, c.target, c.input)),
                    Err(e) => state.set_status(e),
                }
            }

            open = false;
        }

        self.open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shape_bounds_label_is_rounded() {
        let bounds = Aabb::new(DVec3::new(-12.3456789, 3.1417, 40.0), DVec3::new(383.0962, 500.7383, 121.17647058825241));
        assert_eq!(bounds_label(ShapeKind::Rock, &bounds, "last brush"), "Bounds 395.442 x 497.597 x 81.176 (last brush)");
        assert_eq!(bounds_label(ShapeKind::Text, &bounds, "last brush"), "Starts at -12.346 3.142 40 (last brush)");

        let whole = Aabb::new(DVec3::new(32.0, -64.0, 0.0), DVec3::new(96.0, 0.0, 16.5));
        assert_eq!(bounds_label(ShapeKind::Rock, &whole, "replaces selection"), "Bounds 64 x 64 x 16.5 (replaces selection)");

        let tiny = Aabb::new(DVec3::new(-0.0004, -0.0, 1.0004), DVec3::new(10.0, 10.0, 10.0));
        assert_eq!(bounds_label(ShapeKind::Text, &tiny, "x"), "Starts at 0 0 1 (x)");
    }

    struct BoxFixture {
        bounds: Aabb,
        size: bool,
        changes: usize,
    }

    fn box_harness(size: bool) -> egui_kittest::Harness<'static, BoxFixture> {
        let fixture = BoxFixture { bounds: Aabb::new(DVec3::new(-40.0, 8.0, 24.0), DVec3::new(88.0, 72.0, 120.0)), size, changes: 0 };
        let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(420.0, 160.0)).build_ui_state(
            |ui, f: &mut BoxFixture| {
                if box_fields(ui, &mut f.bounds, f.size, 0.0) {
                    f.changes += 1;
                }
            },
            fixture,
        );
        harness.run();
        harness
    }

    fn spin_buttons(harness: &egui_kittest::Harness<'static, BoxFixture>) -> usize {
        use egui_kittest::kittest::Queryable;
        harness.get_all_by_role(egui::accesskit::Role::SpinButton).count()
    }

    // Digit by digit like a person types, nothing may change before Enter.
    fn type_into(harness: &mut egui_kittest::Harness<'static, BoxFixture>, field: usize, text: &str) {
        use egui_kittest::kittest::Queryable;
        harness.get_all_by_role(egui::accesskit::Role::SpinButton).nth(field).unwrap().click();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.run();
        let before = harness.state().bounds;
        for c in text.chars() {
            harness.get_all_by_role(egui::accesskit::Role::SpinButton).nth(field).unwrap().type_text(&c.to_string());
            harness.run();
            assert_eq!(harness.state().bounds, before, "typing {text} applied early");
        }
    }

    fn enter(harness: &mut egui_kittest::Harness<'static, BoxFixture>) {
        harness.key_press(egui::Key::Enter);
        harness.run();
    }

    #[test]
    fn the_box_fields_type_the_corner_and_size() {
        let mut harness = box_harness(true);
        assert_eq!(spin_buttons(&harness), 6);

        type_into(&mut harness, 3, "100");
        enter(&mut harness);
        assert_eq!(harness.state().bounds, Aabb::new(DVec3::new(-40.0, 8.0, 24.0), DVec3::new(60.0, 72.0, 120.0)), "the size grows from the corner");
        assert_eq!(harness.state().changes, 1);

        type_into(&mut harness, 1, "-40");
        enter(&mut harness);
        assert_eq!(harness.state().bounds, Aabb::new(DVec3::new(-40.0, -40.0, 24.0), DVec3::new(60.0, 24.0, 120.0)), "moving the corner keeps the size");

        type_into(&mut harness, 5, "0");
        enter(&mut harness);
        assert_eq!(harness.state().bounds.size(), DVec3::new(100.0, 64.0, 1.0), "a size of 0 clamps to 1");

        type_into(&mut harness, 5, "2");
        enter(&mut harness);
        assert_eq!(harness.state().bounds.size().z, 2.0);
        type_into(&mut harness, 5, "1");
        enter(&mut harness);
        assert_eq!(harness.state().bounds.size().z, 1.0, "a size of 1 is allowed");

        let before = harness.state().bounds;
        let changes = harness.state().changes;
        type_into(&mut harness, 4, "55");
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert_eq!(harness.state().bounds, before, "Escape keeps the old size");
        assert_eq!(harness.state().changes, changes);
    }

    #[test]
    fn the_box_fields_of_the_text_shape_have_only_the_corner() {
        let mut harness = box_harness(false);
        assert_eq!(spin_buttons(&harness), 3);
        type_into(&mut harness, 2, "-8");
        enter(&mut harness);
        assert_eq!(harness.state().bounds, Aabb::new(DVec3::new(-40.0, 8.0, -8.0), DVec3::new(88.0, 72.0, 88.0)));
    }

    fn palette_frame(ctx: &egui::Context, palette: &mut CommandPalette, state: &EditorState, time: &mut f64, events: Vec<egui::Event>) -> Vec<Action> {
        *time += 0.05;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        ctx.begin_pass(egui::RawInput { events, time: Some(*time), screen_rect: Some(screen), ..Default::default() });
        let mut actions = Vec::new();
        palette.show(ctx, state, &mut actions);
        ctx.end_pass().drop_without_applying_deltas();
        actions
    }

    #[test]
    fn the_class_picker_lists_brush_classes_runs_the_highlighted_one_and_never_leaks_into_the_palette() {
        let class =
            |name: &str, kind: &str| -> td_formats::EntityDef { serde_json::from_value(serde_json::json!({ "classname": name, "type": kind })).unwrap() };
        let (ctx, mut state, mut time) = (egui::Context::default(), EditorState::new(Default::default()), 0.0);
        state.game.entities = vec![class("trigger_once", "solid"), class("func_door", "solid"), class("light", "point"), class("func_detail", "solid")];
        let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let mut palette = CommandPalette::default();
        let mut run = |palette: &mut CommandPalette, events: Vec<egui::Event>| {
            for _ in 0..3 {
                palette_frame(&ctx, palette, &state, &mut time, vec![]);
            }

            palette_frame(&ctx, palette, &state, &mut time, events)
        };

        palette.open_class_picker(&state, "trigger_once");
        assert_eq!(
            run(&mut palette, vec![key(egui::Key::Enter)]),
            vec![Action::CreateBrushEntity("trigger_once".into())],
            "the default is highlighted, not the first row"
        );
        assert!(!palette.open);

        palette.open_class_picker(&state, "func_detail");
        assert_eq!(
            run(&mut palette, vec![key(egui::Key::ArrowDown), key(egui::Key::Enter)]),
            vec![Action::CreateBrushEntity("func_door".into())],
            "arrows move from it"
        );

        palette.open_class_picker(&state, "func_detail");
        assert!(run(&mut palette, vec![egui::Event::Text("trig".into())]).is_empty());
        assert_eq!(run(&mut palette, vec![key(egui::Key::Enter)]), vec![Action::CreateBrushEntity("trigger_once".into())], "typing filters the classes");

        palette.open_class_picker(&state, "missing");
        assert_eq!(
            run(&mut palette, vec![key(egui::Key::Enter)]),
            vec![Action::CreateBrushEntity("func_detail".into())],
            "an unknown default falls back to the first row"
        );

        palette.open_class_picker(&state, "func_detail");
        assert!(run(&mut palette, vec![key(egui::Key::Escape)]).is_empty() && !palette.open, "Escape runs nothing");
        palette.toggle();
        assert!(palette.open && !palette.class_picker, "the next palette lists commands again");
        assert!(run(&mut palette, vec![egui::Event::Text("tie to".into())]).is_empty());
        let ran = run(&mut palette, vec![key(egui::Key::Enter)]);
        assert_eq!(ran, vec![Action::TieToEntity], "{ran:?}");
    }

    #[test]
    fn the_wheel_scrolls_the_palette_away_from_the_highlighted_row() {
        let (ctx, state, mut time) = (egui::Context::default(), EditorState::new(Default::default()), 0.0);
        let first = ranked("", palette_entries(&state))[0].1.shown.clone();
        let at = egui::pos2(600.0, 300.0);
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -120.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        };
        let mut palette = CommandPalette::default();
        let mut top = |palette: &mut CommandPalette, events: Vec<egui::Event>| {
            time += 0.05;
            ctx.begin_pass(egui::RawInput {
                events,
                time: Some(time),
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                ..Default::default()
            });
            palette.show(&ctx, &state, &mut Vec::new());
            let mut output = ctx.end_pass();
            let mut shapes: Vec<_> = std::mem::take(&mut output.shapes).into_iter().map(|c| c.shape).collect();
            output.drop_without_applying_deltas();
            let mut y = None;
            while let Some(shape) = shapes.pop() {
                match shape {
                    egui::Shape::Vec(inner) => shapes.extend(inner),
                    egui::Shape::Text(t) if t.galley.text() == first => y = Some(t.pos.y),
                    _ => {}
                }
            }

            y
        };

        palette.toggle();
        for events in [vec![], vec![], vec![egui::Event::PointerMoved(at)]] {
            top(&mut palette, events);
        }

        let before = top(&mut palette, vec![]).expect("the first row is drawn");
        top(&mut palette, vec![wheel]);
        let mut after = Some(before);
        for _ in 0..20 {
            after = top(&mut palette, vec![]);
        }

        assert!(after.is_none_or(|y| y < before - 60.0), "the list stays where the wheel left it, {before} then {after:?}");
    }

    #[test]
    fn the_tools_popup_picks_a_tool_by_number_or_by_name() {
        let (ctx, state, mut time) = (egui::Context::default(), EditorState::new(Default::default()), 0.0);
        let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let mut palette = CommandPalette::default();
        let mut run = |palette: &mut CommandPalette, events: Vec<egui::Event>| {
            for _ in 0..3 {
                palette_frame(&ctx, palette, &state, &mut time, vec![]);
            }

            palette_frame(&ctx, palette, &state, &mut time, events)
        };

        palette.open_tools();
        assert_eq!(run(&mut palette, vec![key(egui::Key::Num1)]), vec![Action::SetTool(ToolKind::all()[0])], "numbered in toolbar order");
        palette.open_tools();
        assert_eq!(run(&mut palette, vec![key(egui::Key::Num2)]), vec![Action::SetTool(ToolKind::all()[1])]);
        assert!(!palette.open);

        palette.open_tools();
        assert!(run(&mut palette, vec![egui::Event::Text("clip".into())]).is_empty());
        assert!(run(&mut palette, vec![key(egui::Key::Num1)]).is_empty(), "with a query typed, numbers are text");
        assert_eq!(run(&mut palette, vec![key(egui::Key::Enter)]), vec![Action::SetTool(ToolKind::Clip)]);

        palette.open_tools();
        palette.tools_at = Some(egui::pos2(5.0, 5.0));
        assert!(run(&mut palette, vec![egui::Event::Text("zzz".into())]).is_empty());
        assert!(run(&mut palette, vec![key(egui::Key::Enter)]).is_empty() && palette.open, "Enter with no match runs nothing");
        let shift_q = egui::Event::Key { key: egui::Key::Q, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::SHIFT };
        run(&mut palette, vec![shift_q, egui::Event::Text("Q".into())]);
        assert!(palette.open && palette.query.is_empty(), "Shift+Q again types nothing: {:?}", palette.query);
        assert_ne!(palette.tools_at, Some(egui::pos2(5.0, 5.0)), "Shift+Q again moves the popup");
        assert!(run(&mut palette, vec![key(egui::Key::Escape)]).is_empty() && !palette.open);

        palette.toggle();
        assert!(palette.open && !palette.tools, "the next palette lists commands again");
    }

    #[test]
    fn escape_or_a_click_outside_closes_the_palette_without_running_anything() {
        let (ctx, state, mut time) = (egui::Context::default(), EditorState::new(Default::default()), 0.0);
        let mut palette = CommandPalette::default();
        let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        palette.toggle();
        for _ in 0..3 {
            palette_frame(&ctx, &mut palette, &state, &mut time, vec![]);
        }

        assert!(palette_frame(&ctx, &mut palette, &state, &mut time, vec![key(egui::Key::Escape)]).is_empty());
        assert!(!palette.open, "Escape closes it");

        palette.toggle();
        for _ in 0..3 {
            palette_frame(&ctx, &mut palette, &state, &mut time, vec![]);
        }

        let outside = egui::pos2(40.0, 700.0);
        let press = |pressed| egui::Event::PointerButton { pos: outside, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let mut ran = palette_frame(&ctx, &mut palette, &state, &mut time, vec![egui::Event::PointerMoved(outside)]);
        ran.extend(palette_frame(&ctx, &mut palette, &state, &mut time, vec![press(true)]));
        ran.extend(palette_frame(&ctx, &mut palette, &state, &mut time, vec![press(false)]));
        assert!(ran.is_empty() && !palette.open, "a click outside closes it, ran {ran:?}");
    }

    #[test]
    fn fuzzy_ranks_word_starts() {
        assert!(fuzzy_score("csg", "Brush: CSG Subtract").is_some());
        assert!(fuzzy_score("xyz", "Brush: CSG Subtract").is_none());
        let a = fuzzy_score("sub", "Brush: CSG Subtract").unwrap();
        let b = fuzzy_score("sub", "Select: Same Material but").unwrap_or(i32::MIN);
        assert!(a > b);
    }

    #[test]
    fn the_shape_dialog_builds_rocks_as_a_brush_or_a_mesh() {
        let bounds = Aabb::new(DVec3::new(0.0, 0.0, 0.0), DVec3::new(96.0, 48.0, 64.0));
        let mut dialog = ShapeDialog { kind: ShapeKind::Rock, ..Default::default() };
        let brush = dialog.generate(&bounds, "m");
        let [NodeKind::Brush(b)] = brush.as_slice() else { panic!("one rock brush") };
        assert!((b.bounds().size() - bounds.size()).abs().max_element() < 1e-6);
        dialog.as_mesh = true;
        let mesh = dialog.generate(&bounds, "m");
        let [NodeKind::Mesh(m)] = mesh.as_slice() else { panic!("one rock mesh") };
        assert!(m.is_closed() && m.faces.len() > b.faces.len());
    }

    /// One frame of the dialog, returning each text it drew with its screen rect and color.
    fn shape_frame(
        ctx: &egui::Context,
        dialog: &mut ShapeDialog,
        state: &mut EditorState,
        time: &mut f64,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Rect, egui::Color32)> {
        *time += 0.05;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
        ctx.begin_pass(egui::RawInput { events, time: Some(*time), screen_rect: Some(screen), ..Default::default() });
        dialog.show(ctx, state);
        let mut output = ctx.end_pass();
        let mut shapes: Vec<_> = std::mem::take(&mut output.shapes).into_iter().map(|c| c.shape).collect();
        output.drop_without_applying_deltas();
        let mut texts = Vec::new();
        while let Some(shape) = shapes.pop() {
            match shape {
                egui::Shape::Vec(inner) => shapes.extend(inner),
                egui::Shape::Text(t) => {
                    let color = t.galley.job.sections.first().map_or(egui::Color32::PLACEHOLDER, |s| s.format.color);
                    let color = t.override_text_color.unwrap_or(if color == egui::Color32::PLACEHOLDER { t.fallback_color } else { color });
                    texts.push((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2()), color));
                }
                _ => {}
            }
        }

        texts
    }

    fn click_create(ctx: &egui::Context, dialog: &mut ShapeDialog, state: &mut EditorState, time: &mut f64) {
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts = shape_frame(ctx, dialog, state, time, vec![]);
        }

        let at = texts.iter().find(|(text, ..)| text == "Create").expect("the dialog draws its Create button").1.center();
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        for event in [egui::Event::PointerMoved(at), press(true), press(false)] {
            shape_frame(ctx, dialog, state, time, vec![event]);
        }
    }

    #[test]
    fn a_selected_terrain_is_never_replaced() {
        let (ctx, mut state, mut time) = (egui::Context::default(), EditorState::new(crate::state::Prefs::default()), 0.0);
        let parent = state.insert_parent();
        let terrain = state.doc.map.insert(parent, NodeKind::Terrain(Terrain::new(DVec3::new(512.0, 0.0, 0.0), [5, 5], 32.0, "m")));
        state.doc.selection.select_node(terrain);
        assert!(shape_sources(&state).is_empty());
        let mut dialog = ShapeDialog { open: true, ..Default::default() };
        click_create(&ctx, &mut dialog, &mut state, &mut time);
        let made = state.doc.selection.brushes(&state.doc.map);
        assert!(!made.is_empty(), "Create made a shape");
        let made_bounds = state.doc.map.bounds_of(made.iter().copied());
        assert!(
            state.last_bounds.expanded(0.01).contains(&made_bounds),
            "a terrain alone leaves the shape at the last brush bounds, it went to {made_bounds:?}"
        );

        state.doc.selection.select_node(terrain);
        assert_eq!(shape_sources(&state), made);
        click_create(&ctx, &mut dialog, &mut state, &mut time);
        assert!(matches!(state.doc.map.get(terrain).map(|n| &n.kind), Some(NodeKind::Terrain(_))), "Create keeps the terrain");
        assert!(made.iter().all(|id| state.doc.map.get(*id).is_none()), "and replaces the shape selected with it");
    }

    #[test]
    fn creating_a_shape_again_replaces_the_mesh_it_made() {
        let (ctx, mut state, mut time) = (egui::Context::default(), EditorState::new(crate::state::Prefs::default()), 0.0);
        let parent = state.insert_parent();
        let block = Brush::from_aabb(&Aabb::new(DVec3::ZERO, DVec3::new(96.0, 48.0, 64.0)), "m").unwrap();
        let block = state.doc.map.insert(parent, NodeKind::Brush(block));
        state.doc.selection.select_node(block);
        let mut dialog = ShapeDialog { open: true, kind: ShapeKind::Rock, as_mesh: true, ..Default::default() };
        for _ in 0..3 {
            click_create(&ctx, &mut dialog, &mut state, &mut time);
        }

        assert!(state.doc.map.brush(block).is_none());
        assert_eq!(state.doc.map.meshes().count(), 1, "Create on the selected rock swaps it rather than stacking another");
    }

    fn made_bounds(state: &EditorState) -> Aabb {
        let made = state.doc.selection.brushes(&state.doc.map);
        assert!(!made.is_empty(), "Create made a shape");
        state.doc.map.bounds_of(made.iter().copied())
    }

    fn fresh_bounds(dialog: ShapeDialog, source: &Aabb) -> Aabb {
        let (ctx, mut state, mut time) = (egui::Context::default(), EditorState::new(crate::state::Prefs::default()), 0.0);
        let block = insert_block(&mut state, source);
        state.doc.selection.select_node(block);
        click_create(&ctx, &mut ShapeDialog { open: true, ..dialog }, &mut state, &mut time);
        made_bounds(&state)
    }

    #[test]
    fn creating_again_builds_from_the_box_the_first_shape_replaced() {
        let source = box_of([0.0, 0.0, 0.0], [256.0, 192.0, 32.0]);
        let (ctx, mut state, mut time) = (egui::Context::default(), EditorState::new(crate::state::Prefs::default()), 0.0);
        let block = insert_block(&mut state, &source);
        state.doc.selection.select_node(block);
        let mut dialog = ShapeDialog { open: true, kind: ShapeKind::Arch, angle: 60.0, ..Default::default() };
        click_create(&ctx, &mut dialog, &mut state, &mut time);
        assert!(made_bounds(&state).size().y < 40.0, "a 60 degree arch is flat");

        for angle in [120.0, 180.0] {
            dialog.angle = angle;
            click_create(&ctx, &mut dialog, &mut state, &mut time);
            let fresh = fresh_bounds(ShapeDialog { kind: ShapeKind::Arch, angle, ..Default::default() }, &source);
            assert_eq!(made_bounds(&state), fresh, "{angle} degrees again matches a fresh one from the box");
        }

        dialog.kind = ShapeKind::Tunnel;
        click_create(&ctx, &mut dialog, &mut state, &mut time);
        assert_eq!(made_bounds(&state), source, "the tunnel fills the whole box");

        let other = insert_block(&mut state, &box_of([512.0, 0.0, 0.0], [576.0, 64.0, 64.0]));
        state.doc.selection.clear();
        state.doc.selection.select_node(other);
        dialog.kind = ShapeKind::Wedge;
        click_create(&ctx, &mut dialog, &mut state, &mut time);
        assert_eq!(made_bounds(&state), box_of([512.0, 0.0, 0.0], [576.0, 64.0, 64.0]), "another selection builds from its own box");
    }

    #[test]
    fn undo_and_redo_back_to_a_recreated_arch_keep_its_box() {
        let source = box_of([0.0, 0.0, 0.0], [192.0, 128.0, 128.0]);
        let (ctx, mut state, mut time) = (egui::Context::default(), EditorState::new(crate::state::Prefs::default()), 0.0);
        let block = insert_block(&mut state, &source);
        state.doc.selection.select_node(block);
        let mut dialog = ShapeDialog { open: true, kind: ShapeKind::Arch, ..Default::default() };
        for angle in [120.0, 60.0, 180.0] {
            dialog.angle = angle;
            click_create(&ctx, &mut dialog, &mut state, &mut time);
        }

        let label = "Bounds 192 x 128 x 128 (replaces selection)";
        let shows_box = |dialog: &mut ShapeDialog, state: &mut EditorState, time: &mut f64| {
            shape_frame(&ctx, dialog, state, time, vec![]).iter().any(|(text, ..)| text == label)
        };
        assert_eq!(made_bounds(&state), source);
        state.undo();
        assert!(made_bounds(&state).size().y < 40.0, "the undo brings back the 60 degree arch");
        assert!(shows_box(&mut dialog, &mut state, &mut time), "and the dialog still offers the box it came from");
        state.redo();
        assert!(shows_box(&mut dialog, &mut state, &mut time), "so does a redo");
        state.undo();
        click_create(&ctx, &mut dialog, &mut state, &mut time);
        assert_eq!(made_bounds(&state), source, "Create after the undo builds the 180 degree arch from the box");

        state.undo();
        let arch = state.doc.selection.brushes(&state.doc.map);
        assert!(made_bounds(&state).size().y < 40.0, "the 60 degree arch is back");
        let select = |state: &mut EditorState, ids: &[td_doc::NodeId]| {
            state.doc.selection.clear();
            for id in ids {
                state.doc.selection.select_node(*id);
            }
        };
        let other = insert_block(&mut state, &box_of([512.0, 0.0, 0.0], [576.0, 64.0, 64.0]));
        select(&mut state, &[other]);
        assert!(!shows_box(&mut dialog, &mut state, &mut time), "another selection shows its own box");
        select(&mut state, &arch);
        assert!(shows_box(&mut dialog, &mut state, &mut time), "selecting the arch again brings its box back");
        select(&mut state, &[other]);
        click_create(&ctx, &mut dialog, &mut state, &mut time);
        state.undo();
        select(&mut state, &arch);
        assert!(shows_box(&mut dialog, &mut state, &mut time), "so does undoing a Create on another object");
        select(&mut state, &[]);
        shape_frame(&ctx, &mut dialog, &mut state, &mut time, vec![]);
        select(&mut state, &arch);
        assert!(shows_box(&mut dialog, &mut state, &mut time), "so does selecting it after selecting nothing");
        dialog.open = false;
        shape_frame(&ctx, &mut dialog, &mut state, &mut time, vec![]);
        dialog.open = true;
        assert!(!shows_box(&mut dialog, &mut state, &mut time), "closing the dialog forgets the boxes");
    }

    #[test]
    fn the_arch_thickness_field_stops_at_the_arch_cap() {
        let source = box_of([0.0, 0.0, 0.0], [192.0, 128.0, 256.0]);
        let (ctx, mut state, mut time) = (egui::Context::default(), EditorState::new(crate::state::Prefs::default()), 0.0);
        let block = insert_block(&mut state, &source);
        state.doc.selection.select_node(block);
        for angle in [90.0, 180.0] {
            let mut dialog = ShapeDialog { open: true, kind: ShapeKind::Arch, angle, thickness: 500.0, ..Default::default() };
            shape_frame(&ctx, &mut dialog, &mut state, &mut time, vec![]);
            assert_eq!(dialog.thickness_in(&source), shapes::arch_thickness_cap(&source, angle), "{angle}");
        }

        let mut dialog = ShapeDialog { open: true, kind: ShapeKind::Arch, thickness: 500.0, ..Default::default() };
        shape_frame(&ctx, &mut dialog, &mut state, &mut time, vec![]);
        dialog.angle = 60.0;
        shape_frame(&ctx, &mut dialog, &mut state, &mut time, vec![]);
        assert_eq!(dialog.thickness_in(&source), shapes::arch_thickness_cap(&source, 60.0), "lowering the angle lowers a thick wall with it");
        assert_eq!(dialog.thickness, 500.0, "the typed thickness waits for a box it fits");
    }

    #[test]
    fn a_capped_shape_keeps_the_typed_values() {
        let small = box_of([0.0, 0.0, 0.0], [64.0, 64.0, 32.0]);
        let mut dialog = ShapeDialog { kind: ShapeKind::Pipe, thickness: 100.0, ..Default::default() };
        for kind in [ShapeKind::Arch, ShapeKind::Tunnel, ShapeKind::Pipe] {
            dialog.kind = kind;
            shape_texts(&mut dialog, &small);
        }

        assert_eq!(dialog.thickness, 100.0, "looking at the Arch and Tunnel left the pipe's thickness alone");

        let round = box_of([0.0, 0.0, 0.0], [128.0, 128.0, 256.0]);
        let tunnel = shapes::Tunnel { profile: shapes::TunnelProfile::Round, walkway: true, walkway_width: 32.0, ..Default::default() };
        let mut dialog = ShapeDialog { kind: ShapeKind::Tunnel, tunnel, ..Default::default() };
        let texts = shape_texts(&mut dialog, &round);
        let fitted = dialog.tunnel_params(&round).fitted(&round);
        assert!(fitted.walkway_width < 32.0 && texts.iter().any(|t| t == "28.19"), "{texts:?}");
        dialog.tunnel.profile = shapes::TunnelProfile::Square;
        shape_texts(&mut dialog, &round);
        assert_eq!(dialog.tunnel_params(&round).fitted(&round).walkway_width, 32.0, "a square tube has room for the typed width");
    }

    #[test]
    fn the_dialog_shows_how_high_an_arch_under_180_rises() {
        let source = box_of([0.0, 0.0, 0.0], [192.0, 128.0, 256.0]);
        let (ctx, mut state, mut time) = (egui::Context::default(), EditorState::new(crate::state::Prefs::default()), 0.0);
        let block = insert_block(&mut state, &source);
        state.doc.selection.select_node(block);
        let rises = |dialog: &mut ShapeDialog, state: &mut EditorState, time: &mut f64| {
            shape_frame(&ctx, dialog, state, time, vec![]).into_iter().map(|(text, ..)| text).find(|t| t.starts_with("The arch rises"))
        };
        let mut dialog = ShapeDialog { open: true, kind: ShapeKind::ArchWall, angle: 90.0, ..Default::default() };
        assert_eq!(rises(&mut dialog, &mut state, &mut time), None, "an arch wall fills its box");
        dialog.kind = ShapeKind::Arch;
        dialog.angle = 180.0;
        assert_eq!(rises(&mut dialog, &mut state, &mut time), None, "a half ellipse is as tall as its box");
        dialog.angle = 90.0;
        assert_eq!(rises(&mut dialog, &mut state, &mut time).as_deref(), Some("The arch rises 39.8 u"));
        click_create(&ctx, &mut dialog, &mut state, &mut time);
        assert!((made_bounds(&state).size().y - arch_rise(192.0, 90.0)).abs() < 1e-6);
    }

    #[test]
    fn only_the_shapes_using_angle_or_tunnel_options_rebuild_on_them() {
        let bounds = box_of([0.0, 0.0, 0.0], [192.0, 128.0, 256.0]);
        let key = |kind, angle, tunnel| ShapeDialog { kind, angle, tunnel, ..Default::default() }.key(&bounds, "m");
        let a = shapes::Tunnel::default();
        let b = shapes::Tunnel { profile: shapes::TunnelProfile::Arched, ..a };
        assert_eq!(key(ShapeKind::Wedge, 90.0, a), key(ShapeKind::Wedge, 180.0, b));
        assert_eq!(key(ShapeKind::Tunnel, 90.0, a), key(ShapeKind::Tunnel, 180.0, a));
        assert_ne!(key(ShapeKind::Tunnel, 90.0, a), key(ShapeKind::Tunnel, 90.0, b));
        assert_ne!(key(ShapeKind::Tunnel, 90.0, a), key(ShapeKind::Tunnel, 90.0, shapes::Tunnel { floor: true, ..a }));
        assert_ne!(key(ShapeKind::Tunnel, 90.0, a), key(ShapeKind::Tunnel, 90.0, shapes::Tunnel { ceiling: false, ..a }));
        assert_ne!(key(ShapeKind::Tunnel, 90.0, b), key(ShapeKind::Tunnel, 120.0, b), "the arch angle rebuilds an arched tunnel");
        let walkway = shapes::Tunnel { walkway: true, ..b };
        assert_ne!(key(ShapeKind::Tunnel, 90.0, b), key(ShapeKind::Tunnel, 90.0, walkway));
        assert_ne!(
            key(ShapeKind::Tunnel, 90.0, walkway),
            key(ShapeKind::Tunnel, 90.0, shapes::Tunnel { walkway_width: walkway.walkway_width + 8.0, ..walkway }),
            "the walkway width rebuilds"
        );
        assert_ne!(
            key(ShapeKind::Tunnel, 90.0, walkway),
            key(ShapeKind::Tunnel, 90.0, shapes::Tunnel { walkway_height: walkway.walkway_height + 8.0, ..walkway }),
            "the walkway height rebuilds"
        );
        let bent = shapes::Tunnel { bend: 90.0, ..a };
        let others = [
            shapes::Tunnel { toward: shapes::Toward::MinusX, ..bent },
            shapes::Tunnel { bend_radius: bent.bend_radius + 8.0, ..bent },
            shapes::Tunnel { bend_segments: bent.bend_segments + 1, ..bent },
            shapes::Tunnel { rise: 48.0, ..bent },
            shapes::Tunnel { axis: shapes::Axis::X, ..bent },
            shapes::Tunnel { bend: 45.0, ..bent },
        ];
        for other in others {
            assert_ne!(key(ShapeKind::Tunnel, 90.0, bent), key(ShapeKind::Tunnel, 90.0, other), "{other:?}");
        }

        for kind in [ShapeKind::Arch, ShapeKind::ArchWall] {
            assert_eq!(key(kind, 90.0, a), key(kind, 90.0, b));
            assert_ne!(key(kind, 90.0, a), key(kind, 180.0, a));
        }
    }

    #[test]
    fn heavy_shapes_are_flagged_by_their_triangles() {
        let bounds = Aabb::new(DVec3::ZERO, DVec3::splat(256.0));
        assert_eq!(triangles(&[NodeKind::Brush(Brush::from_aabb(&bounds, "m").unwrap())]), 12);
        let mut dialog = ShapeDialog { kind: ShapeKind::Rock, as_mesh: true, ..Default::default() };
        dialog.rock.detail = rock::MAX_DETAIL;
        assert!(triangles(&dialog.generate(&bounds, "m")) <= HEAVY_TRIANGLES, "the most detailed rock is fine on its own");
        let grid = ShapeDialog { kind: ShapeKind::Grid, steps: 128, ..Default::default() };
        assert!(triangles(&grid.generate(&bounds, "m")) > HEAVY_TRIANGLES);
    }

    fn show_frame(dialog: &mut ShapeDialog, ctx: &egui::Context, state: &mut EditorState) {
        ctx.begin_pass(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0))), ..Default::default() });
        dialog.show(ctx, state);
        ctx.end_pass().drop_without_applying_deltas();
    }

    fn open_shape_dialog(last: Aabb) -> (ShapeDialog, egui::Context, EditorState) {
        let mut state = EditorState::new(crate::state::Prefs::default());
        state.grid = 16.0;
        state.last_bounds = last;
        (ShapeDialog { open: true, ..Default::default() }, egui::Context::default(), state)
    }

    fn box_of(min: [f64; 3], max: [f64; 3]) -> Aabb {
        Aabb::new(DVec3::from(min), DVec3::from(max))
    }

    fn insert_block(state: &mut EditorState, bounds: &Aabb) -> td_doc::NodeId {
        let parent = state.insert_parent();
        let brush = Brush::from_aabb(bounds, "m").unwrap();
        state.doc.map.insert(parent, NodeKind::Brush(brush))
    }

    #[test]
    fn with_nothing_selected_the_dialog_seeds_a_box_around_the_last_brush() {
        let last = box_of([-37.0, 5.0, 22.0], [51.0, 70.0, 99.0]);
        let (mut dialog, ctx, mut state) = open_shape_dialog(last);
        assert_eq!(state.shape_bounds, None);
        show_frame(&mut dialog, &ctx, &mut state);
        let seeded = box_of([-32.0, 0.0, 16.0], [48.0, 64.0, 96.0]);
        assert_eq!(state.shape_bounds, Some(seeded));
        assert_eq!(dialog.sources_and_bounds(&mut state), (Vec::new(), seeded));
        assert_eq!(state.last_bounds, last, "the box never writes the last brush");
    }

    #[test]
    fn a_selected_brush_or_mesh_replaces_the_box() {
        let (mut dialog, ctx, mut state) = open_shape_dialog(box_of([-37.0, 5.0, 22.0], [51.0, 70.0, 99.0]));
        show_frame(&mut dialog, &ctx, &mut state);
        assert!(state.shape_bounds.is_some());

        let block_bounds = box_of([100.0, -8.0, 40.0], [180.0, 56.0, 104.0]);
        let block = insert_block(&mut state, &block_bounds);
        state.doc.selection.select_node(block);
        show_frame(&mut dialog, &ctx, &mut state);
        assert_eq!(state.shape_bounds, None);
        assert_eq!(dialog.sources_and_bounds(&mut state), (vec![block], block_bounds));

        let mesh_bounds = box_of([-200.0, 0.0, -90.0], [-120.0, 64.0, -10.0]);
        let mesher = ShapeDialog { kind: ShapeKind::Rock, as_mesh: true, ..Default::default() };
        let kinds = mesher.generate(&mesh_bounds, "m");
        assert!(matches!(kinds[..], [NodeKind::Mesh(_)]));
        let parent = state.insert_parent();
        let mesh = state.doc.map.insert(parent, kinds.into_iter().next().unwrap());
        state.doc.selection.clear();
        state.doc.selection.select_node(mesh);
        show_frame(&mut dialog, &ctx, &mut state);
        let (sources, bounds) = dialog.sources_and_bounds(&mut state);
        assert_eq!(sources, vec![mesh]);
        assert_eq!(bounds, state.doc.map.bounds_of([mesh]));

        state.doc.selection.select_node(block);
        let (sources, bounds) = dialog.sources_and_bounds(&mut state);
        assert_eq!(sources.len(), 2);
        assert_eq!(bounds, state.doc.map.bounds_of([block, mesh]));
        assert_ne!(bounds, block_bounds);
        assert_eq!(state.shape_bounds, None);
    }

    #[test]
    fn a_point_entity_alone_still_shows_the_box() {
        let (mut dialog, ctx, mut state) = open_shape_dialog(box_of([-37.0, 5.0, 22.0], [51.0, 70.0, 99.0]));
        let layer = state.doc.map.default_layer();
        let light = td_doc::ops::create_point_entity(&mut state.doc.map, layer, "light", DVec3::new(300.0, 4.0, -9.0));
        state.doc.selection.select_node(light);
        show_frame(&mut dialog, &ctx, &mut state);
        assert_eq!(state.shape_bounds, Some(box_of([-32.0, 0.0, 16.0], [48.0, 64.0, 96.0])));
    }

    #[test]
    fn the_box_is_remembered_while_a_selection_hides_it() {
        let (mut dialog, ctx, mut state) = open_shape_dialog(box_of([-37.0, 5.0, 22.0], [51.0, 70.0, 99.0]));
        show_frame(&mut dialog, &ctx, &mut state);
        let edited = box_of([-40.0, 8.0, 24.0], [88.0, 72.0, 120.0]);
        state.shape_bounds = Some(edited);
        show_frame(&mut dialog, &ctx, &mut state);
        assert_eq!(state.shape_bounds, Some(edited), "a view edit wins over the stored box");

        let block = insert_block(&mut state, &box_of([0.0; 3], [32.0; 3]));
        state.doc.selection.select_node(block);
        state.last_bounds = box_of([500.0, 500.0, 500.0], [600.0, 600.0, 600.0]);
        show_frame(&mut dialog, &ctx, &mut state);
        assert_eq!(state.shape_bounds, None);

        state.doc.selection.clear();
        show_frame(&mut dialog, &ctx, &mut state);
        assert_eq!(state.shape_bounds, Some(edited), "deselecting brings the box back, it is not seeded again");
    }

    #[test]
    fn closing_forgets_the_box_and_reopening_seeds_again() {
        let (mut dialog, ctx, mut state) = open_shape_dialog(box_of([-37.0, 5.0, 22.0], [51.0, 70.0, 99.0]));
        show_frame(&mut dialog, &ctx, &mut state);
        assert!(state.shape_bounds.is_some() && dialog.bounds.is_some());

        dialog.open = false;
        show_frame(&mut dialog, &ctx, &mut state);
        assert_eq!(state.shape_bounds, None);
        assert_eq!(dialog.bounds, None);

        state.last_bounds = box_of([130.0, 0.0, -70.0], [190.0, 41.0, -9.0]);
        dialog.open = true;
        show_frame(&mut dialog, &ctx, &mut state);
        assert_eq!(state.shape_bounds, Some(box_of([128.0, 0.0, -64.0], [192.0, 48.0, -16.0])));
    }

    #[test]
    fn closing_clears_a_box_that_never_had_a_preview() {
        let (mut dialog, ctx, mut state) = open_shape_dialog(Aabb::EMPTY);
        state.shape_bounds = Some(box_of([0.0; 3], [16.0; 3]));
        dialog.bounds = state.shape_bounds;
        dialog.open = false;
        assert!(dialog.preview.is_none());
        show_frame(&mut dialog, &ctx, &mut state);
        assert_eq!(state.shape_bounds, None);
        assert_eq!(dialog.bounds, None);
    }

    #[test]
    fn the_preview_follows_the_box_between_frames() {
        let (mut dialog, ctx, mut state) = open_shape_dialog(box_of([-37.0, 5.0, 22.0], [51.0, 70.0, 99.0]));
        let inside =
            |lines: &[(DVec3, DVec3)], b: &Aabb| lines.iter().all(|(p, q)| [p, q].iter().all(|v| v.cmpge(b.min - 1e-6).all() && v.cmple(b.max + 1e-6).all()));
        let (first, second) = (box_of([-40.0, 8.0, 24.0], [88.0, 72.0, 120.0]), box_of([100.0, -16.0, -48.0], [164.0, 48.0, 16.0]));
        state.shape_bounds = Some(first);
        show_frame(&mut dialog, &ctx, &mut state);
        let lines_first = state.shape_preview.clone();
        assert!(!lines_first.is_empty() && inside(&lines_first, &first));

        state.shape_bounds = Some(second);
        show_frame(&mut dialog, &ctx, &mut state);
        assert!(!state.shape_preview.is_empty() && inside(&state.shape_preview, &second));
        assert_ne!(state.shape_preview, lines_first);

        state.shape_bounds = Some(first);
        show_frame(&mut dialog, &ctx, &mut state);
        assert_eq!(state.shape_preview, lines_first);
    }

    #[test]
    fn creating_from_the_box_adds_the_shape_and_hands_the_selection_over() {
        let (mut dialog, ctx, mut state) = open_shape_dialog(box_of([-37.0, 5.0, 22.0], [51.0, 70.0, 99.0]));
        dialog.kind = ShapeKind::Stairs;
        let layer = state.doc.map.default_layer();
        let group = state.doc.map.insert(layer, NodeKind::Group(td_doc::Group::new("g")));
        state.open_groups.push(group);
        let target = box_of([-40.0, 8.0, 24.0], [88.0, 72.0, 120.0]);
        state.shape_bounds = Some(target);
        show_frame(&mut dialog, &ctx, &mut state);
        let kinds = dialog.preview.as_ref().map(|(_, k, _)| k.clone()).unwrap();
        assert!(kinds.len() > 1, "stairs are many objects");

        create_shape(&mut state, &[], kinds.clone());
        assert_eq!(state.doc.map.brushes().count(), kinds.len());
        let created: Vec<_> = state.doc.selection.nodes.iter().copied().collect();
        assert_eq!(created.len(), kinds.len());
        assert!(created.iter().all(|id| state.doc.map.get(*id).and_then(|n| n.parent) == Some(group)));
        assert_eq!(state.doc.map.bounds_of(created.iter().copied()), target);

        let (sources, bounds) = dialog.sources_and_bounds(&mut state);
        assert_eq!(sources.len(), created.len());
        assert_eq!(bounds, target);
        assert_eq!(state.shape_bounds, None, "the new shape drives the dialog now");
    }

    #[test]
    fn the_dialog_shows_a_heavy_shapes_triangles_in_the_warning_color() {
        let (ctx, mut state, mut time) = (egui::Context::default(), EditorState::new(crate::state::Prefs::default()), 0.0);
        state.last_bounds = Aabb::new(DVec3::ZERO, DVec3::splat(256.0));
        let warn = ctx.global_style().visuals.warn_fg_color;
        for (steps, heavy) in [(8, false), (128, true)] {
            let mut dialog = ShapeDialog { open: true, kind: ShapeKind::Grid, steps, ..Default::default() };
            let tris = triangles(&dialog.generate(&state.last_bounds, &state.current_material));
            assert_eq!(tris > HEAVY_TRIANGLES, heavy);
            let mut texts = Vec::new();
            for _ in 0..3 {
                texts = shape_frame(&ctx, &mut dialog, &mut state, &mut time, vec![]);
            }

            let (_, _, color) = texts.iter().find(|(text, ..)| *text == format!("{tris} triangles")).expect("the dialog shows the shape's triangle count");
            assert_eq!(*color == warn, heavy, "{tris} triangles drawn in {color:?}");
        }
    }

    #[test]
    fn the_shape_dialog_builds_block_text() {
        let mut dialog = ShapeDialog { kind: ShapeKind::Text, ..Default::default() };
        dialog.text = TextShape { text: "HI".into(), letter_height: 70.0, depth: 4.0, spacing: 1.0, standing: false };
        let corner = Aabb::new(DVec3::new(100.0, 8.0, 0.0), DVec3::new(164.0, 72.0, 64.0));
        let nodes = dialog.generate(&corner, "dev/orange");
        assert!(nodes.len() >= 4, "H is three boxes, I at least one: {}", nodes.len());
        let mut bounds = Aabb::EMPTY;
        for n in &nodes {
            let NodeKind::Brush(b) = n else { panic!("text is brushes") };
            bounds = bounds.union(&b.bounds());
        }

        assert_eq!(bounds.min.y, 8.0, "letters start at the corner of the bounds");
        assert!(
            (bounds.size().y - 4.0).abs() < 1e-9 && (bounds.size().z - 70.0).abs() < 1e-9,
            "lying letters are depth thick and letter height long: {bounds:?}"
        );
        dialog.text.standing = true;
        let upright = dialog.generate(&corner, "m").iter().fold(Aabb::EMPTY, |acc, n| if let NodeKind::Brush(b) = n { acc.union(&b.bounds()) } else { acc });
        assert!((upright.size().y - 70.0).abs() < 1e-9);
        dialog.text.text = "Ö".into();
        assert!(dialog.generate(&corner, "m").is_empty(), "unsupported letters create nothing");
        assert!(dialog.text.brushes(DVec3::ZERO, "m").unwrap_err().contains("no block letter"));
    }

    #[test]
    fn the_palette_lists_every_language() {
        let state = EditorState::new(crate::state::Prefs::default());
        let entries = palette_entries(&state);
        let languages: Vec<_> = entries.iter().filter(|(_, a)| matches!(a, Action::SetLanguage(_))).collect();
        assert_eq!(languages.len(), crate::i18n::Lang::ALL.len());
        for l in crate::i18n::Lang::ALL {
            assert!(languages.iter().any(|(label, a)| *a == Action::SetLanguage(l) && label.en == format!("View: Language: {}", l.name())));
        }

        let hits = |q: &str| entries.iter().filter(|(label, _)| fuzzy_score(q, &label.en).is_some() && label.en.starts_with("View: Language")).count();
        assert_eq!(hits("language deutsch"), 1);
        assert_eq!(hits("language"), 8);
    }

    #[test]
    fn the_palette_lists_extrude_faces_under_brush() {
        let state = EditorState::new(crate::state::Prefs::default());
        let entries = palette_entries(&state);
        let hits: Vec<_> = entries.iter().filter(|(_, a)| *a == Action::ExtrudeFaces).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.en, "Brush: Extrude Faces");
        let matches = ranked("extrude faces", entries);
        assert_eq!(matches[0].2, Action::ExtrudeFaces);
    }

    #[test]
    fn palette_ranks_grid_larger_first() {
        let state = EditorState::new(crate::state::Prefs::default());
        let matches = ranked("grid larger", palette_entries(&state));
        assert_eq!(matches[0].2, Action::GridUp);
        assert_eq!(matches[0].1.en, "Grid: Larger");
    }

    #[test]
    fn the_palette_finds_a_command_by_its_english_or_its_translated_name() {
        let _ja =
            crate::i18n::test_catalogue(Lang::Ja, r#"{ "Grid": "グリッド", "Larger": "大きく", "Smaller": "小さく", "Select": "選択", "All": "すべて" }"#);
        let state = EditorState::new(crate::state::Prefs::default());
        let _scope = i18n::scope(Lang::Ja);
        let entries = palette_entries(&state);
        let up = entries.iter().find(|(_, a)| *a == Action::GridUp).unwrap().0.clone();
        assert_eq!((up.en.as_str(), up.shown.as_str()), ("Grid: Larger", "グリッド: 大きく"));
        for query in ["grid larger", "グリッド: 大きく", "グリッド 大きく"] {
            let matches = ranked(query, entries.clone());
            assert_eq!(matches[0].2, Action::GridUp, "{query}");
        }

        let down = ranked("グリッド小", entries.clone());
        assert_eq!(down[0].2, Action::GridDown);
        assert!(ranked("zzzz", entries.clone()).is_empty());
        assert_eq!(ranked("", entries.clone()).len(), entries.len(), "an empty query lists everything");
    }

    #[test]
    fn palette_finds_separate_linked_groups_by_its_other_names() {
        let state = EditorState::new(crate::state::Prefs::default());
        let entries = palette_entries(&state);
        for query in ["make unique", "make single user", "unlink", "separate linked"] {
            let matches = ranked(query, entries.clone());
            assert_eq!(matches[0].2, Action::UnlinkGroups, "{query}");
            assert_eq!(matches[0].1.en, "Group: Separate Linked Groups", "{query} shows the real name");
        }

        assert_eq!(ranked("make unique", entries.clone()).iter().filter(|m| m.2 == Action::UnlinkGroups).count(), 1, "an alias adds no row of its own");
        assert_ne!(ranked("grid larger", entries)[0].2, Action::UnlinkGroups, "other queries are not pulled to it");
    }

    #[test]
    fn command_help_stays_short() {
        use crate::i18n::{Lang, scope, tr};
        let state = EditorState::new(crate::state::Prefs::default());
        let helps: Vec<_> = palette_entries(&state).into_iter().filter_map(|(label, action)| action.help().map(|h| (label.en, h))).collect();
        assert!(helps.len() > 30, "{} commands explain themselves", helps.len());
        for lang in Lang::ALL {
            let _scope = scope(lang);
            let limit = if matches!(lang, Lang::ZhHans | Lang::Ja) { 60 } else { 100 };
            for (label, help) in &helps {
                let shown = tr(help);
                assert!(shown.chars().count() <= limit, "{} {label}: {} characters", lang.code(), shown.chars().count());
            }
        }
    }

    #[test]
    fn the_mirror_commands_sit_in_the_group_category() {
        let state = EditorState::new(crate::state::Prefs::default());
        let entries = palette_entries(&state);
        let label = |action: Action| entries.iter().find(|(_, a)| *a == action).map(|(l, _)| l.en.clone());
        for (axis, name) in ["X", "Y", "Z"].into_iter().enumerate() {
            assert_eq!(label(Action::MirrorLinked { axis, gap: 0.0 }), Some(format!("Group: Mirror Linked {name}")));
        }

        let apply = Action::ApplyMirror { distance: td_doc::symmetry::MIRROR_TOLERANCE };
        assert_eq!(label(apply), Some("Group: Apply Mirror".to_string()));
        assert!(!entries.iter().any(|(l, _)| l.en.starts_with("Transform: Mirror") || l.en == "Transform: Apply Mirror"));
    }

    #[test]
    fn the_palette_offers_mirror_again_only_for_a_slid_half() {
        let ctx = egui::Context::default();
        let mut state = EditorState::new(crate::state::Prefs::default());
        let offered = |state: &EditorState| palette_entries(state).iter().any(|(_, a)| *a == Action::MirrorAgain);
        let first = |state: &EditorState| ranked("mirror", palette_entries(state))[0].1.en.clone();
        let layer = state.doc.map.default_layer();
        state.doc.edit("add", |m, s| {
            let id = m.insert(layer, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::new(8.0, 0.0, 0.0), DVec3::new(40.0, 32.0, 32.0)), "wall").unwrap()));
            s.select_node(id);
        });
        assert!(!offered(&state), "nothing is mirrored");
        assert!(first(&state).starts_with("Group: Mirror Linked"), "{}", first(&state));

        commands::execute(&mut state, Action::MirrorLinked { axis: 0, gap: 0.0 }, &ctx);
        assert!(!state.status_failed(), "{}", state.status);
        assert!(!offered(&state), "the halves sit across their plane");
        state.doc.edit("slide", |m, s| td_doc::ops::translate_selection(m, s, DVec3::new(0.0, 0.0, 12.0), td_doc::ops::EditOptions::default()));
        assert!(offered(&state), "a slid half can be mirrored again");
    }

    #[test]
    fn palette_lists_each_command_once() {
        let state = EditorState::new(crate::state::Prefs::default());
        let entries = palette_entries(&state);
        for (i, (label, action)) in entries.iter().enumerate() {
            let twin = entries[i + 1..].iter().find(|(_, other)| other == action);
            assert!(twin.is_none(), "{} and {} run the same command", label.en, twin.unwrap().0.en);
        }

        assert!(entries.iter().any(|(_, a)| *a == Action::ToggleEditMode));
    }

    #[test]
    fn link_terrain3d_is_in_the_palette_only_with_the_experimental_bridge_on() {
        let mut state = EditorState::new(crate::state::Prefs::default());
        let has = |state: &EditorState| palette_entries(state).iter().any(|(_, a)| *a == Action::ShowTerrain3DDialog);
        assert!(!has(&state), "off by default");
        state.prefs.terrain3d = true;
        assert!(has(&state));
    }

    #[test]
    fn the_generator_and_keymap_windows_open_under_their_english_ids_in_german() {
        let _de = crate::i18n::test_catalogue(
            Lang::De,
            r#"{ "Shape Generator": "Formgenerator", "Create Terrain": "Gelände erstellen", "Keyboard Shortcuts": "Tastenkürzel", "Link Entities": "Entitäten verknüpfen" }"#,
        );
        let _scope = i18n::scope(Lang::De);
        let mut state = EditorState::new(crate::state::Prefs::default());
        let layer = state.doc.map.default_layer();
        state.doc.edit("add", |m, s| {
            let a = td_doc::ops::create_point_entity(m, layer, "logic_branch", DVec3::ZERO);
            let b = td_doc::ops::create_point_entity(m, layer, "light", DVec3::X);
            s.select_node(a);
            s.select_node(b);
        });
        let (mut shape, mut terrain, mut keymap, mut link) = (ShapeDialog::default(), TerrainDialog::default(), KeymapWindow::default(), LinkDialog::default());
        (shape.open, terrain.open, keymap.open) = (true, true, true);
        link.open_for(&state);
        assert!(link.open);
        let ctx = egui::Context::default();
        for time in [0.0, 0.1, 0.2] {
            ctx.begin_pass(egui::RawInput {
                time: Some(time),
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0))),
                ..Default::default()
            });
            shape.show(&ctx, &mut state);
            terrain.show(&ctx, &mut state);
            keymap.show(&ctx, &mut state);
            link.show(&ctx, &mut state);
            ctx.end_pass().drop_without_applying_deltas();
        }

        for english in ["Shape Generator", "Create Terrain", "Keyboard Shortcuts", "Link Entities"] {
            let id = egui::Id::new(Some(english));
            assert!(ctx.memory(|m| m.area_rect(id)).is_some(), "{english} is open under its English id");
            assert!(ctx.memory(|m| m.area_rect(egui::Id::new(Some(tr(english))))).is_none(), "{english} is not under its German id");
        }
    }

    #[test]
    fn no_two_commands_have_the_same_name_in_any_language() {
        let state = EditorState::new(crate::state::Prefs::default());
        for lang in Lang::ALL {
            let _scope = i18n::scope(lang);
            let entries = palette_entries(&state);
            let mut seen = std::collections::HashMap::new();
            for (label, action) in &entries {
                if let Some(other) = seen.insert(label.shown.clone(), action) {
                    panic!("{} {:?} and {:?} are both called {}", lang.code(), other, action, label.shown);
                }
            }
        }
    }

    #[test]
    fn a_connected_godot_editor_runs_the_project_without_an_executable() {
        let mut state = EditorState::new(crate::state::Prefs::default());
        state.godot.exe = None;
        let runs = |state: &EditorState| palette_entries(state).iter().any(|(_, a)| *a == Action::RunGodotProject);
        assert!(!runs(&state));
        let root = std::path::PathBuf::from("C:/game");
        state.game.project_root = Some(root.clone());
        state.link_state.connected = true;
        state.link_state.project = Some(crate::live_link::path_key(&crate::live_link::godot_path(&root)));
        assert!(runs(&state) && !palette_entries(&state).iter().any(|(_, a)| *a == Action::OpenGodotEditor));
    }

    #[test]
    fn jargon_commands_explain_themselves() {
        use crate::i18n::{Lang, scope, tr};
        let state = EditorState::new(crate::state::Prefs::default());
        for (label, action) in palette_entries(&state) {
            let jargon = ["CSG", "Hollow", "UV Lock", "Displacement", "Cordon", "Prefab", "Duplicate Linked", "Convex"].iter().any(|j| label.en.contains(j));
            let help = action.help();
            assert!(!jargon || help.is_some(), "{} needs a help line", label.en);
            for lang in Lang::ALL {
                let _scope = scope(lang);
                let shown = help.map(tr);
                assert!(
                    shown.is_none_or(|h| !h.contains(['\u{2013}', '\u{2014}']) && !h.contains(" - ")),
                    "{} {}: no dashes as punctuation",
                    lang.code(),
                    label.en
                );
            }
        }
    }

    #[test]
    fn swapping_a_link_picks_output_and_input_of_the_new_pair() {
        let mut state = EditorState::new(crate::state::Prefs::default());
        let layer = state.doc.map.default_layer();
        let (branch, light) = state.doc.edit("add", |m, s| {
            let branch = td_doc::ops::create_point_entity(m, layer, "logic_branch", DVec3::ZERO);
            let light = td_doc::ops::create_point_entity(m, layer, "light", DVec3::X);
            s.select_node(branch);
            s.select_node(light);
            (branch, light)
        });
        let mut dialog = LinkDialog::default();
        dialog.open_for(&state);
        let (from, to) = (dialog.from.unwrap(), dialog.to.unwrap());
        assert_eq!((from, to), (branch.min(light), branch.max(light)));
        for _ in 0..2 {
            dialog.swap(&state);
            let (outs, ins) = crate::entity_wizards::link_options(&state, dialog.from.unwrap(), dialog.to.unwrap());
            assert_eq!(dialog.output, outs.first().cloned().unwrap_or_default());
            assert!(ins.contains(&dialog.input), "{} is not an input of the target", dialog.input);
        }
    }

    #[test]
    fn shape_dialog_generates_meshes_and_spirals() {
        let bounds = Aabb::new(DVec3::new(-64.0, 0.0, -64.0), DVec3::new(64.0, 256.0, 64.0));
        let mut d = ShapeDialog { kind: ShapeKind::SpiralStairs, steps: 16, thickness: 16.0, ..Default::default() };
        let stairs = d.generate(&bounds, "m");
        assert_eq!(stairs.len(), 16);
        d.kind = ShapeKind::Cylinder;
        d.as_mesh = true;
        assert!(matches!(d.generate(&bounds, "m").as_slice(), [NodeKind::Mesh(m)] if m.smooth_angle > 0.0));
        for k in [ShapeKind::Torus, ShapeKind::ArchWall, ShapeKind::GableRoof, ShapeKind::Spire, ShapeKind::Grid] {
            d.kind = k;
            assert!(!d.generate(&bounds, "m").is_empty(), "{k:?}");
        }

        d.kind = ShapeKind::Pillar;
        let pillar = d.generate(&bounds, "m");
        assert!(pillar.len() > 3 && pillar.iter().all(|k| matches!(k, NodeKind::Brush(_))), "a pillar is brushes even after a mesh shape");
    }

    #[test]
    fn shape_dialog_arch_angle_changes_the_curve() {
        let bounds = Aabb::new(DVec3::new(-64.0, 0.0, -8.0), DVec3::new(64.0, 128.0, 8.0));
        let top = |angle: f64, kind: ShapeKind| {
            let d = ShapeDialog { kind, angle, sides: 12, thickness: 16.0, ..Default::default() };
            d.generate(&bounds, "m")
                .iter()
                .map(|n| match n {
                    NodeKind::Brush(b) => b.bounds().max.y,
                    NodeKind::Mesh(m) => m.vertices.iter().map(|v| v.y).fold(f64::MIN, f64::max),
                    _ => panic!("brush or mesh"),
                })
                .fold(f64::MIN, f64::max)
        };
        let (full, flat) = (top(180.0, ShapeKind::Arch), top(90.0, ShapeKind::Arch));
        assert!((full - 128.0).abs() < 1e-6 && flat < 40.0, "{full} {flat}");
        for kind in [ShapeKind::Arch, ShapeKind::ArchWall] {
            assert!(!ShapeDialog { kind, angle: 60.0, ..Default::default() }.generate(&bounds, "m").is_empty());
        }

        let radius = shapes::arch_radius(128.0, 90.0);
        assert!((shapes::arch_angle_for_radius(128.0, radius) - 90.0).abs() < 1e-9);
    }

    fn shape_texts(dialog: &mut ShapeDialog, source: &Aabb) -> Vec<String> {
        let (ctx, mut state, mut time) = (egui::Context::default(), EditorState::new(crate::state::Prefs::default()), 0.0);
        state.grid = 16.0;
        let block = insert_block(&mut state, source);
        state.doc.selection.select_node(block);
        dialog.open = true;
        (0..2).flat_map(|_| shape_frame(&ctx, dialog, &mut state, &mut time, vec![])).map(|(text, ..)| text).collect()
    }

    #[test]
    fn the_arch_says_when_the_box_height_caps_its_sweep() {
        let capped = |texts: &[String]| texts.iter().find(|t| t.starts_with("The box height caps the sweep at")).cloned();
        let low = box_of([0.0, 0.0, 0.0], [256.0, 80.0, 32.0]);
        let mut dialog = ShapeDialog { kind: ShapeKind::Arch, angle: 150.0, ..Default::default() };
        assert_eq!(capped(&shape_texts(&mut dialog, &low)).as_deref(), Some("The box height caps the sweep at 128.0°"));
        dialog.angle = 100.0;
        assert_eq!(capped(&shape_texts(&mut dialog, &low)), None, "100 degrees fits");
        dialog.angle = 180.0;
        assert_eq!(capped(&shape_texts(&mut dialog, &low)), None, "the half ellipse takes the box height");
    }

    #[test]
    fn the_tunnel_thickness_field_shows_the_capped_wall() {
        let source = box_of([0.0, 0.0, 0.0], [128.0, 96.0, 32.0]);
        let z = shapes::Axis::Z;
        assert_eq!(tunnel_thickness_max(&source, 16.0, z), 32.0);
        assert!((tunnel_thickness_max(&source, 0.0, z) - 38.4).abs() < 1e-9);
        assert!((tunnel_thickness_max(&source, 64.0, z) - 38.4).abs() < 1e-9, "a grid step too big to fit keeps the cap");

        let long = box_of([-8.0, 4.0, 10.0], [292.0, 204.0, 74.0]);
        assert_eq!(tunnel_thickness_max(&long, 0.0, shapes::Axis::X), 25.6, "an X tunnel's cross section is 200 by 64");
        assert_eq!(tunnel_thickness_max(&long, 0.0, shapes::Axis::Y), 25.6, "a Y tunnel's cross section is 300 by 64");
        assert_eq!(tunnel_thickness_max(&long, 0.0, z), 80.0, "a Z tunnel's cross section is 300 by 200");
        assert_eq!(tunnel_thickness_max(&long, 16.0, shapes::Axis::X), 16.0, "rounded down to the grid");

        let mut dialog = ShapeDialog { kind: ShapeKind::Tunnel, thickness: 100.0, ..Default::default() };
        let texts = shape_texts(&mut dialog, &source);
        assert_eq!(dialog.thickness_in(&source), 32.0);
        assert!(texts.iter().any(|t| t == "32"), "the field shows 32, {texts:?}");
        let wall = |n: &NodeKind| matches!(n, NodeKind::Brush(b) if b.bounds().min.x == 0.0 && b.bounds().max.x == 32.0);
        assert!(dialog.generate(&source, "m").iter().any(wall), "the left wall is as thick as the field says");

        let mut pipe = ShapeDialog { kind: ShapeKind::Pipe, thickness: 100.0, ..Default::default() };
        shape_texts(&mut pipe, &source);
        assert_eq!(pipe.thickness, 100.0, "a pipe leaves the field alone");
    }

    #[test]
    fn a_typed_arch_radius_stays_exact() {
        use egui_kittest::kittest::Queryable;
        let bounds = box_of([0.0, 0.0, 0.0], [128.0, 96.0, 32.0]);
        let dialog = ShapeDialog { kind: ShapeKind::Arch, angle: 90.0, ..Default::default() };
        let mut harness =
            egui_kittest::Harness::builder().with_size(egui::vec2(480.0, 160.0)).build_ui_state(|ui, d: &mut ShapeDialog| d.arch_fields(ui, &bounds), dialog);
        harness.run();
        let radius = |d: &ShapeDialog| shapes::arch_radius(128.0, shapes::arch_sweep(128.0, 96.0, d.angle));
        for typed in [100.0, 200.0, 64.0] {
            harness.get_all_by_role(egui::accesskit::Role::SpinButton).last().unwrap().click();
            harness.run();
            harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
            harness.run();
            harness.get_all_by_role(egui::accesskit::Role::SpinButton).last().unwrap().type_text(&typed.to_string());
            harness.run();
            harness.key_press(egui::Key::Enter);
            for _ in 0..3 {
                harness.run();
            }

            assert!((radius(harness.state()) - typed).abs() < 1e-6, "typed {typed}, got {}", radius(harness.state()));
        }
    }

    #[test]
    fn a_radius_out_of_reach_says_where_it_stops() {
        use egui_kittest::kittest::Queryable;
        let bounds = box_of([0.0, 0.0, 0.0], [128.0, 96.0, 32.0]);
        let dialog = ShapeDialog { kind: ShapeKind::Arch, angle: 90.0, ..Default::default() };
        let mut harness =
            egui_kittest::Harness::builder().with_size(egui::vec2(480.0, 200.0)).build_ui_state(|ui, d: &mut ShapeDialog| d.arch_fields(ui, &bounds), dialog);
        harness.run();
        let radius = harness.query_all_by_role(egui::accesskit::Role::SpinButton).count() - 1;
        let note = |h: &egui_kittest::Harness<'_, ShapeDialog>| h.query_by_label_contains("The radius stays between 64.0 and").is_some();
        type_into_field(&mut harness, radius, "10");
        assert_eq!(harness.state().angle, 180.0, "a radius under half the span gives a half round");
        assert!(note(&harness), "the cut radius is named");
        type_into_field(&mut harness, radius, "100");
        assert!(!note(&harness), "a radius in reach clears the note");
        type_into_field(&mut harness, radius, "100000");
        assert_eq!(harness.state().angle, shapes::ARCH_MIN_ANGLE);
        assert!(note(&harness));
        harness.state_mut().angle = 120.0;
        harness.run();
        assert!(!note(&harness), "moving the angle clears it too");
    }

    #[test]
    fn shape_presets_save_pick_and_delete_through_the_preferences() {
        use egui_kittest::kittest::Queryable;
        let dialog = ShapeDialog { kind: ShapeKind::Rock, rock: Rock { seed: 7, ..Default::default() }, preset_name: " boulder ".into(), ..Default::default() };
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(560.0, 240.0))
            .build_ui_state(|ui, (d, presets): &mut (ShapeDialog, Vec<ShapePreset>)| d.preset_row(ui, presets), (dialog, Vec::new()));
        h.run();
        h.get_by_label("Save Preset").click();
        h.run();
        let saved = h.state().1.clone();
        assert_eq!(saved.len(), 1);
        assert_eq!((saved[0].name.as_str(), saved[0].kind, saved[0].rock.seed), ("boulder", ShapeKind::Rock, 7));

        let mut prefs = crate::state::Prefs { shape_presets: saved, ..Default::default() };
        prefs = serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
        assert_eq!(prefs.shape_presets, h.state().1, "presets survive a restart");
        let older: crate::state::Prefs = serde_json::from_str("{}").unwrap();
        assert!(older.shape_presets.is_empty());

        h.state_mut().0 = ShapeDialog::default();
        h.run();
        h.get_by_role(egui::accesskit::Role::ComboBox).click();
        h.run();
        h.get_by_label("boulder").click();
        h.run();
        assert_eq!((h.state().0.kind, h.state().0.rock.seed, h.state().0.preset_name.as_str()), (ShapeKind::Rock, 7, "boulder"));

        h.get_by_label("Delete").click();
        h.run();
        assert!(h.state().1.is_empty());
    }

    fn type_into_field(harness: &mut egui_kittest::Harness<'_, ShapeDialog>, index: usize, text: &str) {
        use egui_kittest::kittest::Queryable;
        let role = egui::accesskit::Role::SpinButton;
        harness.get_all_by_role(role).nth(index).unwrap().click();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.run();
        harness.get_all_by_role(role).nth(index).unwrap().type_text(text);
        harness.run();
        harness.key_press(egui::Key::Enter);
        for _ in 0..3 {
            harness.run();
        }
    }

    #[test]
    fn a_typed_tunnel_arch_radius_stays_exact() {
        use egui_kittest::kittest::Queryable;
        let bounds = box_of([0.0, 0.0, 0.0], [128.0, 96.0, 32.0]);
        let tunnel = shapes::Tunnel { profile: shapes::TunnelProfile::Arched, ..Default::default() };
        let dialog = ShapeDialog { kind: ShapeKind::Tunnel, tunnel, angle: 90.0, ..Default::default() };
        let mut harness =
            egui_kittest::Harness::builder().with_size(egui::vec2(480.0, 200.0)).build_ui_state(|ui, d: &mut ShapeDialog| d.arch_fields(ui, &bounds), dialog);
        harness.run();
        let radius = |d: &ShapeDialog| shapes::arch_radius(128.0, d.tunnel_params(&bounds).fitted(&bounds).angle);
        for typed in [100.0, 200.0, 64.0] {
            let last = harness.get_all_by_role(egui::accesskit::Role::SpinButton).count() - 1;
            type_into_field(&mut harness, last, &typed.to_string());
            assert!((radius(harness.state()) - typed).abs() < 1e-6, "typed {typed}, got {}", radius(harness.state()));
        }
    }

    #[test]
    fn the_arched_tunnel_notes_show_on_both_sides_of_each_guard() {
        let note = |dialog: &mut ShapeDialog, source: &Aabb, start: &str| {
            let found: Vec<String> = shape_texts(dialog, source).into_iter().filter(|t| t.starts_with(start)).collect();
            assert!(found.len() <= 1, "{start} shows once, {found:?}");
            found.into_iter().next()
        };
        let tunnel = shapes::Tunnel { profile: shapes::TunnelProfile::Arched, ..Default::default() };
        let dialog = |angle| ShapeDialog { kind: ShapeKind::Tunnel, tunnel, thickness: 8.0, angle, ..Default::default() };
        let notes = |d: &mut ShapeDialog, b: &Aabb| {
            [
                note(d, b, "The box height caps the sweep at"),
                note(d, b, "The arch is "),
                note(d, b, "The arch needs "),
                note(d, b, "Too low and wide for an arch under 180"),
            ]
        };
        let low = box_of([0.0, 0.0, 0.0], [256.0, 64.0, 32.0]);

        // The box height caps a 100 degree sweep at 56.1, and 40 fits.
        assert_eq!(notes(&mut dialog(100.0), &low)[0].as_deref(), Some("The box height caps the sweep at 56.1°"));
        assert_eq!(notes(&mut dialog(40.0), &low)[0], None);

        // A crown under 1 u thick raises 14 to 14.8, and 15 is already thick enough.
        let at_14 = notes(&mut dialog(14.0), &low);
        assert_eq!(at_14[2].as_deref(), Some("The arch needs 14.8° or more to stay 1 u thick"));
        assert_eq!((at_14[0].as_deref(), at_14[3].as_deref()), (None, None), "a raised arch is not also capped or a fallback");
        let at_15 = notes(&mut dialog(15.0), &low);
        assert_eq!(at_15[2], None);
        assert!(at_15[1].as_deref().is_some_and(|t| t.contains("1.0 u")), "{at_15:?}");

        // Exactly the angle that keeps the crown 1 u thick is not raised.
        let least = dialog(10.0).tunnel_params(&low).fitted(&low).angle;
        assert!(least > 14.0 && least < 15.0, "{least}");
        assert_eq!(note(&mut dialog(least), &low, "The arch needs "), None);

        // Under 180 the crown is thinner than the walls, at 180 it is not.
        let crown = shapes::Tunnel { thickness: 8.0, angle: 40.0, ..tunnel }.arch_crown(&low).unwrap();
        let at_40 = notes(&mut dialog(40.0), &low);
        assert!(at_40[1].as_deref().is_some_and(|t| t.contains(&format!("{crown:.1} u"))), "{at_40:?}");
        assert_eq!(notes(&mut dialog(180.0), &low), [None, None, None, None]);

        // A box 18 high cannot hold a segmental arch at this thickness, so the arch stays a half ellipse.
        let flat = box_of([0.0, 0.0, 0.0], [256.0, 18.0, 32.0]);
        let got = notes(&mut dialog(40.0), &flat);
        assert!(got[3].is_some(), "{got:?}");
        assert_eq!((got[0].as_deref(), got[1].as_deref(), got[2].as_deref()), (None, None, None));
        assert_eq!(notes(&mut dialog(180.0), &flat)[3], None, "no fallback when no arch was asked for");
        assert_eq!(notes(&mut dialog(40.0), &box_of([0.0, 0.0, 0.0], [256.0, 20.0, 32.0]))[3], None, "20 high holds the arch");

        for profile in [shapes::TunnelProfile::Square, shapes::TunnelProfile::Round] {
            let mut d =
                ShapeDialog { kind: ShapeKind::Tunnel, tunnel: shapes::Tunnel { profile, ..tunnel }, thickness: 8.0, angle: 40.0, ..Default::default() };
            assert_eq!(notes(&mut d, &flat), [None, None, None, None], "{profile:?} has no arch notes");
        }
    }

    #[test]
    fn the_tunnel_walkway_fields_show_only_when_ticked_and_stop_at_the_cap() {
        use egui::accesskit::Role::SpinButton;
        use egui_kittest::kittest::Queryable;
        let bounds = box_of([0.0, 0.0, 0.0], [128.0, 96.0, 32.0]);
        for profile in shapes::TunnelProfile::ALL {
            let dialog = ShapeDialog { kind: ShapeKind::Tunnel, tunnel: shapes::Tunnel { profile, ..Default::default() }, ..Default::default() };
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(480.0, 160.0))
                .build_ui_state(|ui, d: &mut ShapeDialog| d.tunnel_fields(ui, &bounds), dialog);
            harness.run();
            assert_eq!(harness.query_all_by_role(SpinButton).count(), 0, "{profile:?} unticked");
            harness.get_by_label("Walkway").click();
            harness.run();
            assert!(harness.state().tunnel.walkway, "{profile:?}");
            assert_eq!(harness.query_all_by_role(SpinButton).count(), 2, "{profile:?} ticked");

            for index in 0..2 {
                type_into_field(&mut harness, index, "1000");
            }

            let (d, fitted) = (harness.state(), harness.state().tunnel_params(&bounds).fitted(&bounds));
            assert_eq!(
                (d.tunnel.walkway_width, d.tunnel.walkway_height),
                (fitted.walkway_width, fitted.walkway_height),
                "{profile:?} the fields stop at the cap"
            );
            assert!(fitted.walkway_width > 1.0 && fitted.walkway_height > 1.0);
            if profile == shapes::TunnelProfile::Square {
                assert!(
                    (d.tunnel.walkway_width - 38.4).abs() < 1e-9 && d.tunnel.walkway_height == 40.0,
                    "{} by {}",
                    d.tunnel.walkway_width,
                    d.tunnel.walkway_height
                );
            }

            type_into_field(&mut harness, 0, "0");
            assert_eq!(harness.state().tunnel.walkway_width, 1.0, "{profile:?} the width stops at 1");
            harness.get_by_label("Walkway").click();
            harness.run();
            assert_eq!(harness.query_all_by_role(SpinButton).count(), 0, "{profile:?} unticked again");
        }
    }

    fn path_harness(dialog: ShapeDialog, bounds: Aabb) -> egui_kittest::Harness<'static, ShapeDialog> {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(480.0, 320.0))
            .build_ui_state(move |ui, d: &mut ShapeDialog| d.tunnel_path_fields(ui, &bounds), dialog);
        harness.run();
        harness
    }

    #[test]
    fn the_tunnel_axis_is_its_own_and_defaults_to_z() {
        use egui_kittest::kittest::Queryable;
        let dialog = ShapeDialog::default();
        assert_eq!((dialog.tunnel.axis, dialog.axis), (shapes::Axis::Z, shapes::Axis::Y), "tunnels run along Z, cylinders stand on Y");
        let mut harness = path_harness(ShapeDialog { kind: ShapeKind::Tunnel, ..Default::default() }, box_of([0.0, 0.0, 0.0], [128.0, 96.0, 256.0]));
        harness.get_by_label("X").click();
        harness.run();
        assert_eq!((harness.state().tunnel.axis, harness.state().axis), (shapes::Axis::X, shapes::Axis::Y), "the row sets the tunnel only");
    }

    #[test]
    fn the_bend_group_shows_only_while_bent_and_lists_the_crossing_directions() {
        use egui::accesskit::Role::{Slider, SpinButton};
        use egui_kittest::kittest::Queryable;
        let bounds = box_of([0.0, 0.0, 0.0], [128.0, 96.0, 256.0]);
        let tunnel = |axis, bend| shapes::Tunnel { axis, bend, ..Default::default() };
        let straight = path_harness(ShapeDialog { tunnel: tunnel(shapes::Axis::Z, 0.0), ..Default::default() }, bounds);
        assert_eq!(straight.query_all_by_role(SpinButton).count(), 2, "bend and slope");
        assert_eq!(straight.query_all_by_role(Slider).count(), 0);
        for label in ["Direction", "+X", "-Y"] {
            assert!(straight.query_by_label(label).is_none(), "{label} stays hidden at 0");
        }

        let all = ["+X", "-X", "+Y", "-Y", "+Z", "-Z"];
        for (axis, shown) in
            [(shapes::Axis::Z, ["+X", "-X", "+Y", "-Y"]), (shapes::Axis::X, ["+Y", "-Y", "+Z", "-Z"]), (shapes::Axis::Y, ["+X", "-X", "+Z", "-Z"])]
        {
            let bent = path_harness(ShapeDialog { tunnel: tunnel(axis, 0.1), ..Default::default() }, bounds);
            assert_eq!(bent.query_all_by_role(SpinButton).count(), 4, "{axis:?} bend, inner radius, the segments slider's number box and slope");
            assert_eq!(bent.query_all_by_role(Slider).count(), 1, "{axis:?} bend segments");
            assert!(bent.query_by_label("Direction").is_some());
            for label in all {
                assert_eq!(bent.query_by_label(label).is_some(), shown.contains(&label), "{axis:?} {label}");
            }
        }
    }

    #[test]
    fn the_direction_snaps_when_the_axis_changes() {
        use egui_kittest::kittest::Queryable;
        let bounds = box_of([0.0, 0.0, 0.0], [128.0, 96.0, 256.0]);
        let tunnel = shapes::Tunnel { bend: 90.0, toward: shapes::Toward::MinusY, ..Default::default() };
        let mut harness = path_harness(ShapeDialog { tunnel, ..Default::default() }, bounds);
        harness.get_by_label("Y").click();
        harness.run();
        assert_eq!(harness.state().tunnel.toward, shapes::Toward::PlusX, "-Y runs along a Y tunnel, so it falls back");
        harness.get_by_label("-Z").click();
        harness.run();
        harness.get_by_label("Z").click();
        harness.run();
        assert_eq!(harness.state().tunnel.toward, shapes::Toward::PlusX, "-Z runs along a Z tunnel");
        harness.get_by_label("X").click();
        harness.run();
        assert_eq!(harness.state().tunnel.toward, shapes::Toward::PlusZ, "+X runs along an X tunnel, so it goes to +Z");
        harness.get_by_label("-Y").click();
        harness.get_by_label("Z").click();
        harness.run();
        assert_eq!(harness.state().tunnel.toward, shapes::Toward::MinusY, "a direction that still crosses is kept");
    }

    #[test]
    fn the_grid_note_shows_for_bends_that_miss_it() {
        use egui_kittest::kittest::Queryable;
        let bounds = box_of([0.0, 0.0, 0.0], [128.0, 96.0, 256.0]);
        for (bend, shown) in [(45.0, true), (89.9, true), (90.0, false), (90.1, true), (180.0, false), (200.0, false)] {
            let harness = path_harness(ShapeDialog { tunnel: shapes::Tunnel { bend, ..Default::default() }, ..Default::default() }, bounds);
            assert_eq!(harness.query_by_label("Only 90° and 180° bends end on the grid").is_some(), shown, "{bend}");
        }
    }

    #[test]
    fn the_slope_note_shows_the_climb_angle() {
        use egui_kittest::kittest::Queryable;
        let bounds = box_of([0.0, 0.0, 0.0], [128.0, 96.0, 256.0]);
        let note = |rise| {
            let harness = path_harness(ShapeDialog { tunnel: shapes::Tunnel { rise, ..Default::default() }, ..Default::default() }, bounds);
            ["14.0", "-14.0", "63.4", "-63.4"].into_iter().find(|a| harness.query_by_label(&format!("The floor climbs at {a}°")).is_some())
        };
        assert_eq!(note(0.0), None);
        assert_eq!(note(64.0), Some("14.0"));
        assert_eq!(note(-64.0), Some("-14.0"));
        assert_eq!(note(900.0), Some("63.4"), "twice the length is the most");
        assert_eq!(note(-900.0), Some("-63.4"));
    }

    #[test]
    fn the_slope_field_stops_at_twice_the_length_along_the_axis() {
        use egui::accesskit::Role::SpinButton;
        use egui_kittest::kittest::Queryable;
        let bounds = box_of([0.0, 0.0, 0.0], [128.0, 96.0, 256.0]);
        for (axis, most) in [(shapes::Axis::Z, 512.0), (shapes::Axis::X, 256.0), (shapes::Axis::Y, 192.0)] {
            let mut harness = path_harness(ShapeDialog { tunnel: shapes::Tunnel { axis, ..Default::default() }, ..Default::default() }, bounds);
            for text in ["5000", "-5000"] {
                type_into_field(&mut harness, 1, text);
                assert_eq!(harness.state().tunnel.rise.abs(), most, "{axis:?} {text}");
            }

            assert_eq!(harness.get_all_by_role(SpinButton).count(), 2);
        }
    }

    #[test]
    fn shape_dialog_builds_a_tunnel_per_profile() {
        let bounds = Aabb::new(DVec3::new(-24.0, 8.0, 40.0), DVec3::new(104.0, 104.0, 296.0));
        for (profile, floor, ceiling, count) in [
            (shapes::TunnelProfile::Square, false, false, 2),
            (shapes::TunnelProfile::Square, false, true, 3),
            (shapes::TunnelProfile::Square, true, false, 3),
            (shapes::TunnelProfile::Square, true, true, 4),
            (shapes::TunnelProfile::Arched, false, true, 10),
            (shapes::TunnelProfile::Arched, true, false, 11),
            (shapes::TunnelProfile::Round, false, true, 8),
            (shapes::TunnelProfile::Round, true, false, 8),
        ] {
            let tunnel = shapes::Tunnel { profile, floor, ceiling, ..Default::default() };
            let d = ShapeDialog { kind: ShapeKind::Tunnel, tunnel, sides: 8, ..Default::default() };
            assert_eq!(d.generate(&bounds, "m").len(), count, "{profile:?} floor {floor} ceiling {ceiling}");
        }

        let arched = |angle| {
            let tunnel = shapes::Tunnel { profile: shapes::TunnelProfile::Arched, ..Default::default() };
            ShapeDialog { kind: ShapeKind::Tunnel, tunnel, angle, ..Default::default() }.generate(&bounds, "m")
        };
        assert_ne!(arched(90.0), arched(180.0), "the Angle row bends an arched tunnel");
        let square = |angle| ShapeDialog { kind: ShapeKind::Tunnel, angle, ..Default::default() }.generate(&bounds, "m");
        assert_eq!(square(90.0), square(180.0), "a square tunnel has no arch to bend");
    }

    #[test]
    fn the_tunnel_toggles_follow_the_profile() {
        use egui_kittest::kittest::Queryable;
        let bounds = box_of([0.0, 0.0, 0.0], [128.0, 96.0, 32.0]);
        for (profile, floor, ceiling) in
            [(shapes::TunnelProfile::Square, true, true), (shapes::TunnelProfile::Arched, true, false), (shapes::TunnelProfile::Round, false, false)]
        {
            let dialog = ShapeDialog { kind: ShapeKind::Tunnel, tunnel: shapes::Tunnel { profile, ..Default::default() }, ..Default::default() };
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(480.0, 80.0))
                .build_ui_state(|ui, d: &mut ShapeDialog| d.tunnel_fields(ui, &bounds), dialog);
            harness.run();
            assert_eq!(harness.query_by_label("Floor").is_some(), floor, "{profile:?} Floor");
            assert_eq!(harness.query_by_label("Ceiling").is_some(), ceiling, "{profile:?} Ceiling");
            if floor {
                harness.get_by_label("Floor").click();
                harness.run();
                assert!(harness.state().tunnel.floor, "{profile:?} Floor ticks the floor on");
            }

            if ceiling {
                harness.get_by_label("Ceiling").click();
                harness.run();
                assert!(!harness.state().tunnel.ceiling, "{profile:?} Ceiling takes the ceiling off");
            }
        }
    }

    #[test]
    fn shape_dialog_lays_a_cylinder_down_along_its_axis() {
        let bounds = Aabb::new(DVec3::new(-100.0, 70.0, -4.0), DVec3::new(100.0, 78.0, 4.0));
        for as_mesh in [false, true] {
            let d = ShapeDialog { kind: ShapeKind::Cylinder, axis: shapes::Axis::X, as_mesh, sides: 10, ..Default::default() };
            let got = match d.generate(&bounds, "m").as_slice() {
                [NodeKind::Brush(b)] => b.bounds(),
                [NodeKind::Mesh(m)] => m.bounds(),
                other => panic!("one shape, got {}", other.len()),
            };
            assert!((got.size().x - 200.0).abs() < 1e-6 && got.size().y <= 8.0 + 1e-6, "runs along X, got {got:?}");
        }
    }

    #[test]
    fn terrain_from_parameters() {
        let params = TerrainGen { shape: TerrainShape::Island, height: 300.0, ..Default::default() };
        let layers = vec![("grass".to_string(), 256.0), ("rock".to_string(), 128.0)];
        let t = make_terrain(DVec3::ZERO, 33, 32.0, &params, &layers, None, true, None);
        assert_eq!(t.layers.len(), 2);
        assert_eq!(t.splat.len(), 33 * 33 * 4);
        let hm: Vec<f32> = (0..16).map(|i| i as f32 / 15.0).collect();
        let from_image = make_terrain(DVec3::ZERO, 17, 32.0, &TerrainGen { erosion_iterations: 0, ..params }, &layers, Some((&hm, [4, 4])), false, None);
        assert!((from_image.height(16, 16) - 300.0).abs() < 1e-3);
    }

    fn keymap_harness() -> egui_kittest::Harness<'static, (KeymapWindow, EditorState)> {
        let window = KeymapWindow { open: true, ..Default::default() };
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(700.0, 900.0)).build_ui_state(
            |ui, (window, state): &mut (KeymapWindow, EditorState)| window.show(&ui.ctx().clone(), state),
            (window, EditorState::new(crate::state::Prefs::default())),
        );
        h.run();
        h
    }

    #[test]
    fn the_tool_keys_tab_lists_what_the_keys_do_and_leaves_the_keys_tab_behind() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        assert_eq!(h.state().0.tab, KeymapTab::Keys);
        assert!(h.query_by_label("Preset").is_some());
        assert!(h.query_by_label("Bevel vertices (with vertices, edges or faces selected)").is_none());

        h.get_by_label("Tool keys").click();
        h.run();
        assert_eq!(h.state().0.tab, KeymapTab::ToolKeys);
        assert!(h.query_by_label("Preset").is_none(), "the preset belongs to the Keys tab");
        assert!(h.query_by_label("Ctrl+Shift+B").is_some());
        assert!(h.query_by_label("Bevel vertices (with vertices, edges or faces selected)").is_some());
        assert!(h.query_by_label("Close the outline and extrude it").is_some());

        h.get_by_label("Keys").click();
        h.run();
        assert!(h.query_by_label("Preset").is_some());
    }

    #[test]
    fn the_mouse_tab_shows_the_clicks_of_the_chosen_preset() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        assert!(h.query_by_label("Select the face under the cursor, replacing the selection").is_none());
        h.get_by_label("Mouse").click();
        h.run();
        assert_eq!(h.state().0.tab, KeymapTab::Mouse);
        assert!(h.query_by_label("Preset").is_some(), "the preset decides the clicks");
        assert!(h.query_by_label("Shift+click").is_some());
        assert!(h.query_by_label("Select the face under the cursor, replacing the selection").is_some());
        assert!(h.query_by_label("Shift+click a row").is_some());
        assert!(h.query_by_label("Alt+right click a face").is_some());
        assert!(h.query_by_label("Nothing changes, it has no faces to pick").is_none());
        assert!(h.query_by_label("Double click a group").is_some());

        h.get_by_role(egui::accesskit::Role::ComboBox).click();
        h.run();
        h.get_by_label(commands::preset_label("blender")).click();
        h.run();
        assert_eq!(h.state().1.prefs.keymap_preset, "blender");
        assert!(h.query_by_label("Shift+click").is_none(), "Blender has no Shift+click outside its modes");
        let command = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };
        assert!(h.query_by_label(&format!("Shift+click or {command}+click (Face mode)")).is_some());
        assert!(h.query_by_label(&format!("Shift+click or {command}+click (Object mode)")).is_some());
        assert!(h.query_by_label("Nothing changes, it has no faces to pick").is_some());
        for label in ["Double click a group", "Double click an entity brush", "Double click empty space"] {
            assert!(h.query_by_label(label).is_some(), "{label} in the Blender preset");
        }

        h.get_by_role(egui::accesskit::Role::ComboBox).click();
        h.run();
        h.get_by_label(commands::preset_label("hammer")).click();
        h.run();
        assert_eq!(h.state().1.prefs.keymap_preset, "hammer");
        assert!(h.query_by_label("Double click a group").is_some(), "the Hammer preset names it too");
    }

    fn switch_preset(h: &mut egui_kittest::Harness<'static, (KeymapWindow, EditorState)>, preset: &str) {
        use egui_kittest::kittest::Queryable;
        h.get_by_role(egui::accesskit::Role::ComboBox).click();
        h.run();
        h.get_by_label(commands::preset_label(preset)).click();
        h.run();
        assert_eq!(h.state().1.prefs.keymap_preset, preset);
    }

    #[test]
    fn the_mouse_tab_lists_the_copy_drag_for_hammer_and_the_face_click_for_trenchbroom() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        h.get_by_label("Mouse").click();
        h.run();
        assert!(h.query_by_label("Shift+click").is_some());
        assert!(h.query_by_label("Shift+drag the selection").is_none());
        assert!(h.query_by_label("Ctrl+drag the selection").is_none());
        assert!(h.query_by_label("Move it. Ctrl copies, Shift locks the axis").is_some());
        assert!(h.query_by_label("Shift+drag a selected face (3D)").is_some());

        switch_preset(&mut h, "hammer");
        assert!(h.query_by_label("Shift+drag the selection").is_some());
        assert!(h.query_by_label("Ctrl+drag the selection").is_some());
        assert!(h.query_by_label("Shift+click").is_none());
        assert!(h.query_by_label("Ctrl+Shift+click").is_none());
        assert!(h.query_by_label("Shift+drag a selected face (3D)").is_none());
        assert!(h.query_by_label("Ctrl+Shift+drag a selected face (3D)").is_none());

        switch_preset(&mut h, "trenchbroom");
        assert!(h.query_by_label("Shift+click").is_some());
        assert!(h.query_by_label("Shift+drag the selection").is_none());
    }

    #[test]
    fn the_tool_keys_tab_lists_the_texture_tool_select_all_only_for_hammer() {
        use egui_kittest::kittest::Queryable;
        const ROW: &str = "Select every face of the selected brushes (with faces or brushes selected)";
        let mut h = keymap_harness();
        h.get_by_label("Tool keys").click();
        h.run();
        const SLIDE: &str = "Slide the texture one grid step (with faces or brushes selected)";
        assert_eq!(h.query_all_by_label(SLIDE).count(), 4);
        assert!(h.query_by_label(ROW).is_none());

        h.get_by_label("Keys").click();
        h.run();
        switch_preset(&mut h, "hammer");
        h.get_by_label("Tool keys").click();
        h.run();
        assert!(h.query_by_label(ROW).is_some());
        assert_eq!(h.query_all_by_label(SLIDE).count(), 4);

        h.state_mut().0.tool_filter = "every face".into();
        h.run();
        assert!(h.query_by_label(ROW).is_some());
        assert_eq!(h.query_all_by_label(SLIDE).count(), 0);

        h.state_mut().1.prefs.keymap_preset = "blender".into();
        h.run();
        assert!(h.query_by_label(ROW).is_none());
    }

    #[test]
    fn the_tool_keys_filter_keeps_matching_rows_of_every_tool() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        h.get_by_label("Tool keys").click();
        h.run();
        h.state_mut().0.tool_filter = "backspace".into();
        h.run();
        assert!(h.query_by_label("Remove the last point (once a point is placed)").is_some());
        assert!(h.query_by_label("Remove the last corner (once a corner is placed)").is_some());
        assert!(h.query_by_label("Ctrl+Shift+B").is_none());
        assert!(h.query_by_label("Tab").is_none());
    }

    #[test]
    fn each_tab_has_its_own_filter_and_the_window_keeps_its_size() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        let rect =
            |h: &egui_kittest::Harness<'static, (KeymapWindow, EditorState)>| h.ctx.memory(|m| m.area_rect(egui::Id::new(Some("Keyboard Shortcuts")))).unwrap();
        let full = rect(&h);
        h.state_mut().0.filter = "Select Inside".into();
        h.run();
        assert!(h.query_by_label("Select Inside").is_some());
        assert!(h.query_by_label("Undo").is_none());
        assert_eq!(rect(&h), full, "a short list leaves the window where it was");

        h.get_by_label("Tool keys").click();
        h.run();
        assert!(h.query_by_label("Ctrl+Shift+B").is_some(), "the Keys filter stays on its tab");
        h.state_mut().0.tool_filter = "zzzz".into();
        h.run();
        assert!(h.query_by_label("Ctrl+Shift+B").is_none());
        assert_eq!(rect(&h).min, full.min);

        h.get_by_label("Keys").click();
        h.run();
        assert_eq!(h.state().0.filter, "Select Inside");
        assert!(h.query_by_label("Select Inside").is_some());

        h.get_by_label("Mouse").click();
        h.state_mut().1.prefs.keymap_preset = "blender".into();
        h.run();
        assert!(h.query_by_label("Click on something without faces (Face mode)").is_some());
        assert_eq!(rect(&h), full, "long gestures wrap instead of widening the window");
    }

    #[test]
    fn the_set_buttons_stay_put_through_a_filter_a_recording_and_a_prompt() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        let window =
            |h: &egui_kittest::Harness<'static, (KeymapWindow, EditorState)>| h.ctx.memory(|m| m.area_rect(egui::Id::new(Some("Keyboard Shortcuts")))).unwrap();
        let full = window(&h);
        let column: Vec<f32> = h.get_all_by_label("Set").map(|n| n.rect().min.x).collect();
        assert!(column.iter().all(|x| *x == column[0]), "{column:?}");

        h.state_mut().0.filter = Action::CsgSubtract.title();
        h.run();
        let (set, clear) = (h.get_by_label("Set").rect(), h.get_by_label("Clear").rect());
        assert_eq!(set.min.x, column[0], "a short list keeps the column widths");

        h.get_by_label("Set").click();
        h.run();
        h.get_by_label("Keys").hover();
        h.run();
        assert!(h.state().0.recording.is_some());
        assert_eq!((h.get_by_label("Set").rect(), h.get_by_label("Clear").rect()), (set, clear), "the button keeps its label and the row its place");

        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::E);
        h.run();
        assert!(h.query_by_label("Replace").is_some());
        assert_eq!(h.get_by_label("Set").rect(), set, "the prompt opens below the row");
        assert_eq!(window(&h), full, "the prompt wraps instead of widening the window");
    }

    #[test]
    fn a_key_search_lines_its_tool_rows_up_with_the_command_rows() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        let press = h.get_by_label("Press a key").rect();
        press_key_search(&mut h, Key::A);
        assert_eq!(h.get_by_label("Press a key").rect(), press, "the key chip takes the filter's place");
        let mesh = h.get_all_by_label("Mesh").next().unwrap().rect();
        assert_eq!(h.get_by_label(Action::SelectAll.title().as_str()).rect().min.x, mesh.min.x);
        let chords: Vec<f32> = h.get_all_by_label("Ctrl+A").map(|n| n.rect().min.x).collect();
        assert!(chords.len() >= 2 && chords.iter().all(|x| *x == chords[0]), "{chords:?}");
        let rows: Vec<f32> = h.get_all_by_label("Mesh").map(|n| n.rect().center().y).collect();
        let what: Vec<f32> = h.get_all_by_label("Select all in the current mode").map(|n| n.rect().center().y).collect();
        assert!(what.len() == 2 && what.iter().all(|w| rows.iter().any(|r| (w - r).abs() < 1.0)), "descriptions {what:?} beside rows {rows:?}");
    }

    #[test]
    fn the_tool_keys_tab_gives_every_tool_the_same_chord_column() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        h.get_by_label("Tool keys").click();
        h.run();
        let vertex = h.get_by_label("Bevel vertices (with vertices, edges or faces selected)").rect();
        let polygon = h.get_by_label("Close the outline and extrude it").rect();
        assert_eq!(vertex.min.x, polygon.min.x);
    }

    fn chord(modifiers: egui::Modifiers, key: Key) -> egui::KeyboardShortcut {
        egui::KeyboardShortcut::new(modifiers, key)
    }

    #[test]
    fn key_uses_lists_the_exact_chord_first_in_actions_and_tool_rows() {
        let prefs = crate::state::Prefs::default();
        let ctrl = egui::Modifiers::COMMAND;
        let shift = egui::Modifiers::SHIFT;
        let uses = |m, key| {
            let (actions, rows) = key_uses(&prefs, &chord(m, key));
            (actions, rows.iter().map(|r| r.chord()).collect::<Vec<_>>())
        };

        let (actions, rows) = uses(egui::Modifiers::NONE, Key::R);
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[0], Action::SetTool(ToolKind::Rotate));
        assert!(actions.contains(&Action::RepeatLast) && actions.contains(&Action::SetTool(ToolKind::Shear)));
        assert_eq!(rows, ["R", "Ctrl+R"]);

        let (actions, rows) = uses(ctrl, Key::R);
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[0], Action::RepeatLast);
        assert_eq!(rows, ["Ctrl+R", "R"]);

        let (actions, _) = uses(shift, Key::R);
        assert_eq!(actions[0], Action::SetTool(ToolKind::Shear));
        assert_eq!(actions.len(), 3);

        // The browser sends ctrl and command together, the keymap only command.
        let both = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        assert_eq!(uses(both, Key::R).0[0], Action::RepeatLast);
    }

    #[test]
    fn key_uses_finds_keys_only_a_tool_reads_and_keys_nothing_reads() {
        let prefs = crate::state::Prefs::default();
        let (actions, rows) = key_uses(&prefs, &chord(egui::Modifiers::NONE, Key::Enter));
        assert!(actions.is_empty());
        assert_eq!(rows.iter().map(|r| r.tool).collect::<Vec<_>>(), [ToolKind::Polygon, ToolKind::Clip, ToolKind::Mesh, ToolKind::Path]);

        let (actions, rows) = key_uses(&prefs, &chord(egui::Modifiers::NONE, Key::F20));
        assert!(actions.is_empty() && rows.is_empty());
    }

    #[test]
    fn key_uses_follows_the_preset_and_the_overrides() {
        let mut prefs = crate::state::Prefs::default();
        let alt_a = chord(egui::Modifiers::ALT, Key::A);
        assert_eq!(key_uses(&prefs, &alt_a).0[0], Action::SelectAll, "no Alt+A in TrenchBroom, so the grid order stands");
        assert_eq!(key_uses(&prefs, &chord(egui::Modifiers::COMMAND, Key::A)).0[0], Action::SelectAll);
        assert_eq!(key_uses(&prefs, &chord(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::A)).0[0], Action::SelectNone);

        let ctrl_a = chord(egui::Modifiers::COMMAND, Key::A);
        let texture_rows = |prefs: &crate::state::Prefs| key_uses(prefs, &ctrl_a).1.iter().filter(|r| r.tool == ToolKind::Texture).count();
        assert_eq!(texture_rows(&prefs), 0);
        prefs.keymap_preset = "hammer".into();
        assert_eq!(texture_rows(&prefs), 1, "the Hammer Texture tool row");

        prefs.keymap_preset = "blender".into();
        assert_eq!(key_uses(&prefs, &alt_a).0[0], Action::SelectNone);
        assert_eq!(key_uses(&prefs, &chord(egui::Modifiers::NONE, Key::A)).0[0], Action::SelectAllInMode);

        let ctrl_e = chord(egui::Modifiers::COMMAND, Key::E);
        let mut prefs = crate::state::Prefs::default();
        assert!(key_uses(&prefs, &ctrl_e).0.contains(&Action::SelectInside));
        prefs.key_overrides.insert(Action::SelectInside.binding_id(), "Ctrl+Shift+Y".into());
        assert!(!key_uses(&prefs, &ctrl_e).0.contains(&Action::SelectInside));
        assert_eq!(key_uses(&prefs, &chord(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::Y)).0[0], Action::SelectInside);
    }

    fn press_key_search(h: &mut egui_kittest::Harness<'static, (KeymapWindow, EditorState)>, key: Key) {
        use egui_kittest::kittest::Queryable;
        h.get_by_label("Press a key").click();
        h.run();
        assert!(h.state().0.searching);
        h.key_press(key);
        h.run();
    }

    #[test]
    fn pressing_a_key_lists_its_key_rows_and_tool_rows() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        press_key_search(&mut h, Key::A);
        assert!(!h.state().0.searching);
        assert_eq!(h.state().0.key_search.map(|c| c.logical_key), Some(Key::A));
        assert!(h.query_by_label(Action::SelectAll.title().as_str()).is_some());
        assert!(h.query_by_label(Action::SelectNone.title().as_str()).is_some());
        assert!(h.query_by_label(Action::Undo.title().as_str()).is_none(), "Undo is on Z, not A");
        assert_eq!(h.query_all_by_label("Mesh").count(), 3, "one Mesh row for A, Ctrl+A and Alt+A");
        assert_eq!(h.query_all_by_label("Select all in the current mode").count(), 2);
        assert!(h.query_by_label("Preset").is_some(), "the Keys tab stays");
        assert!(h.query_by_label("Nothing uses this key").is_none());
    }

    #[test]
    fn a_key_nothing_uses_says_so_and_the_x_brings_the_list_back() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        press_key_search(&mut h, Key::F20);
        assert!(h.query_by_label("Nothing uses this key").is_some());
        assert!(h.query_by_label(Action::SelectAll.title().as_str()).is_none());

        h.get_by_label("x").click();
        h.run();
        assert_eq!(h.state().0.key_search, None);
        assert!(h.query_by_label("Nothing uses this key").is_none());
        assert!(h.query_by_label(Action::SelectAll.title().as_str()).is_some());
    }

    #[test]
    fn escape_ends_the_key_search_without_filtering() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        h.get_by_label("Press a key").click();
        h.run();
        h.key_press(Key::Escape);
        h.run();
        let window = &h.state().0;
        assert!(!window.searching);
        assert_eq!(window.key_search, None);
        assert!(window.filter.is_empty());
        assert!(h.query_by_label(Action::SelectAll.title().as_str()).is_some());
        assert!(h.query_by_label("Nothing uses this key").is_none());
    }

    fn record_chord(h: &mut egui_kittest::Harness<'static, (KeymapWindow, EditorState)>, action: &Action, modifiers: egui::Modifiers, key: Key) {
        use egui_kittest::kittest::Queryable;
        h.state_mut().0.filter = action.title();
        h.run();
        h.get_by_label("Set").click();
        h.run();
        assert!(h.state().0.recording.is_some());
        h.key_press_modifiers(modifiers, key);
        h.run();
    }

    fn overrides(h: &egui_kittest::Harness<'static, (KeymapWindow, EditorState)>) -> Vec<(String, String)> {
        h.state().1.prefs.key_overrides.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    #[test]
    fn setting_a_free_chord_writes_straight_away() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        record_chord(&mut h, &Action::CsgSubtract, egui::Modifiers::ALT | egui::Modifiers::SHIFT, Key::F9);
        assert_eq!(overrides(&h), [(Action::CsgSubtract.binding_id(), "Shift+Alt+F9".to_string())]);
        assert!(h.state().0.taken.is_none());
        assert!(h.query_by_label("Replace").is_none());
    }

    #[test]
    fn setting_a_used_chord_asks_and_cancel_writes_nothing() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        record_chord(&mut h, &Action::CsgSubtract, egui::Modifiers::COMMAND, Key::E);
        assert!(overrides(&h).is_empty());
        assert!(h.state().0.recording.is_none());
        let taken = h.state().0.taken.as_ref().unwrap();
        assert_eq!((taken.id.as_str(), taken.holder.clone()), (Action::CsgSubtract.binding_id().as_str(), Some(Action::SelectInside)));
        assert!(h.query_by_label_contains("Ctrl+E is used by Select Inside").is_some());
        assert!(h.query_by_label("Replace").is_some() && h.query_by_label("Swap").is_some());

        h.get_by_label("Cancel").click();
        h.run();
        assert!(overrides(&h).is_empty());
        assert!(h.state().0.taken.is_none());
        assert!(h.query_by_label("Replace").is_none());
    }

    #[test]
    fn replace_in_the_prompt_takes_the_chord() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        record_chord(&mut h, &Action::CsgSubtract, egui::Modifiers::COMMAND, Key::E);
        h.get_by_label("Replace").click();
        h.run();
        assert_eq!(overrides(&h), [(Action::CsgSubtract.binding_id(), "Ctrl+E".to_string())]);
        assert!(h.state().0.taken.is_none());
        assert!(h.query_by_label("Replace").is_none());
        let prefs = &h.state().1.prefs;
        assert!(commands::shortcuts(prefs).iter().all(|(_, a)| *a != Action::SelectInside));
    }

    #[test]
    fn swap_in_the_prompt_gives_the_other_action_the_old_chord() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        record_chord(&mut h, &Action::CsgSubtract, egui::Modifiers::COMMAND, Key::E);
        h.get_by_label("Swap").click();
        h.run();
        let want: std::collections::BTreeMap<String, String> =
            [(Action::CsgSubtract.binding_id(), "Ctrl+E".to_string()), (Action::SelectInside.binding_id(), "Ctrl+K".to_string())].into();
        assert_eq!(h.state().1.prefs.key_overrides, want);
        assert!(h.state().0.taken.is_none());
    }

    #[test]
    fn a_chord_a_tool_reads_asks_before_it_is_set() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        let chord = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::B);
        assert!(commands::chord_conflict(&h.state().1.prefs, &Action::CsgSubtract.binding_id(), &chord).is_none(), "no action holds it");
        record_chord(&mut h, &Action::CsgSubtract, chord.modifiers, chord.logical_key);
        assert!(overrides(&h).is_empty());
        assert!(h.query_by_label_contains("While the Mesh tool is active it takes Ctrl+B first.").is_some());
        assert!(h.query_by_label("Replace").is_none() && h.query_by_label("Swap").is_none());

        h.get_by_label("Set anyway").click();
        h.run();
        assert_eq!(overrides(&h), [(Action::CsgSubtract.binding_id(), "Ctrl+B".to_string())]);
        assert!(h.state().0.taken.is_none());
    }

    #[test]
    fn swap_is_greyed_out_when_the_action_had_no_chord() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        h.state_mut().1.prefs.key_overrides.insert(Action::CsgSubtract.binding_id(), String::new());
        record_chord(&mut h, &Action::CsgSubtract, egui::Modifiers::COMMAND, Key::E);
        assert!(h.state().0.taken.is_some());
        h.get_by_label("Swap").click();
        h.run();
        assert!(h.state().0.taken.is_some(), "a disabled Swap does nothing");
        assert!(!h.state().1.prefs.key_overrides.contains_key(&Action::SelectInside.binding_id()), "the holder is untouched");
        h.get_by_label("Replace").click();
        h.run();
        assert_eq!(h.state().1.prefs.key_overrides[&Action::CsgSubtract.binding_id()], "Ctrl+E");
    }

    #[test]
    fn the_prompt_follows_a_new_recording() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        record_chord(&mut h, &Action::CsgSubtract, egui::Modifiers::COMMAND, Key::E);
        assert!(h.state().0.taken.is_some());
        h.get_by_label("Set").click();
        h.run();
        assert!(h.state().0.taken.is_none(), "starting another recording drops the question");
        assert!(h.query_by_label("Replace").is_none());
    }

    #[test]
    fn a_command_name_says_what_it_does_on_hover() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        let help = Action::SelectAllInMode.help().unwrap();
        h.state_mut().0.filter = Action::SelectAllInMode.title();
        h.run();
        assert!(h.query_by_label(help).is_none());
        h.get_by_label(&Action::SelectAllInMode.title()).hover();
        h.run();
        assert!(h.query_by_label(help).is_some(), "an unbound command in the TrenchBroom preset explains itself");
    }

    #[test]
    fn the_prompt_goes_when_the_row_changes() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        record_chord(&mut h, &Action::CsgSubtract, egui::Modifiers::COMMAND, Key::E);
        h.get_by_label("Clear").click();
        h.run();
        assert!(h.state().0.taken.is_none(), "Clear");
        assert_eq!(overrides(&h), [(Action::CsgSubtract.binding_id(), String::new())]);

        record_chord(&mut h, &Action::CsgSubtract, egui::Modifiers::COMMAND, Key::E);
        h.get_by_label("Default").click();
        h.run();
        assert!(h.state().0.taken.is_none(), "Default");
        assert!(overrides(&h).is_empty());
    }

    #[test]
    fn a_command_that_lost_its_key_to_replace_takes_it_back_with_default() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        let ctrl_e = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::E);
        commands::apply_rebind(&mut h.state_mut().1.prefs, &Action::CsgSubtract.binding_id(), &ctrl_e, commands::Resolve::Replace);
        assert!(!h.state().1.prefs.key_overrides.contains_key(&Action::SelectInside.binding_id()));
        h.state_mut().0.filter = Action::SelectInside.title();
        h.run();
        h.get_by_label("Default").click();
        h.run();
        assert!(overrides(&h).is_empty(), "Ctrl+E goes back to Select Inside and Ctrl+K to CSG Subtract");
        assert!(!commands::differs_from_preset(&h.state().1.prefs, &Action::SelectInside));
    }

    #[test]
    fn the_prompt_goes_when_the_preset_changes() {
        use egui_kittest::kittest::Queryable;
        let mut h = keymap_harness();
        record_chord(&mut h, &Action::CsgSubtract, egui::Modifiers::COMMAND, Key::E);
        assert!(h.state().0.taken.is_some());
        h.get_by_role(egui::accesskit::Role::ComboBox).click();
        h.run();
        h.get_by_label(commands::preset_label("hammer")).click();
        h.run();
        assert_eq!(h.state().1.prefs.keymap_preset, "hammer");
        assert!(h.state().0.taken.is_none());
        assert!(h.query_by_label_contains("is used by").is_none());
    }

    #[test]
    fn a_key_pressed_outside_the_search_is_not_taken() {
        let mut h = keymap_harness();
        h.key_press(Key::A);
        h.run();
        assert_eq!(h.state().0.key_search, None);
    }
}
