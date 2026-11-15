//! Linking a map to a Terrain3D data folder, an existing one or a new one.

use std::path::Path;

use td_core::NodeId;
use td_doc::map::NodeKind;
use td_doc::terrain3d::Terrain3DLink;
use td_formats::terrain3d_cache::{self, Manifest};
use td_geom::terrain3d::{LOC_MAX, LOC_MIN, Loc, MAX_VERTEX_SPACING, MIN_VERTEX_SPACING, REGION_SIZES};

use crate::i18n::{en, msg};
use crate::state::EditorState;

pub const DEFAULT_FOLDER: &str = "terrain3d_data";

/// How deep below the project root the folder scan looks.
const SCAN_DEPTH: usize = 8;

/// A data folder TrenchDot creates for a Terrain3D that does not exist yet.
#[derive(Clone, Debug, PartialEq)]
pub struct NewFolder {
    /// Below `res://`.
    pub name: String,
    pub region_size: u32,
    pub vertex_spacing: f64,
    pub columns: u32,
    pub rows: u32,
}

impl Default for NewFolder {
    fn default() -> Self {
        Self { name: DEFAULT_FOLDER.into(), region_size: 256, vertex_spacing: 1.0, columns: 2, rows: 2 }
    }
}

impl NewFolder {
    pub fn data_directory(&self) -> String {
        format!("res://{}", self.name)
    }

    /// Why the folder cannot be created, in English for the MCP tool, the dialog translates the same cases.
    pub fn problem(&self) -> Option<Problem> {
        if self.name.is_empty() {
            return Some(Problem::NoName);
        }

        let plain = self.name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
        if !plain || self.name.starts_with('.') {
            return Some(Problem::BadName);
        }

        if !REGION_SIZES.contains(&self.region_size) {
            return Some(Problem::RegionSize);
        }

        if !(MIN_VERTEX_SPACING..=MAX_VERTEX_SPACING).contains(&self.vertex_spacing) {
            return Some(Problem::Spacing);
        }

        let span = 1..=(LOC_MAX - LOC_MIN + 1) as u32;
        (!span.contains(&self.columns) || !span.contains(&self.rows)).then_some(Problem::Regions)
    }

    /// The regions, centered on the origin: two columns are -1 and 0, three are -1 to 1.
    pub fn locs(&self) -> Vec<Loc> {
        let first = |n: u32| -((n / 2) as i32);
        let (x0, y0) = (first(self.columns), first(self.rows));
        (0..self.rows as i32).flat_map(|y| (0..self.columns as i32).map(move |x| [x0 + x, y0 + y])).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    NoName,
    BadName,
    RegionSize,
    Spacing,
    Regions,
}

impl Problem {
    pub fn english(self) -> &'static str {
        match self {
            Problem::NoName => en!("Type a folder name"),
            Problem::BadName => en!("A folder name uses letters, digits, underscores, hyphens and dots only, and does not start with a dot"),
            Problem::RegionSize => en!("The region size is 64, 128, 256, 512, 1024 or 2048"),
            Problem::Spacing => en!("The vertex spacing is 0.25 to 100 meters"),
            Problem::Regions => en!("Terrain3D holds 1 to 32 regions per side"),
        }
    }
}

/// The folders of the project that hold Terrain3D regions, or a cache TrenchDot made for a new one, as `res://` paths.
pub fn data_folders(project_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    scan(project_root, project_root, 0, &mut out);
    out.sort();
    out
}

fn holds_terrain3d(dir: &Path) -> bool {
    dir.join(terrain3d_cache::DIR_NAME).join(terrain3d_cache::MANIFEST).is_file()
        || std::fs::read_dir(dir).is_ok_and(|entries| entries.flatten().any(|e| is_region_file(&e.file_name().to_string_lossy())))
}

fn is_region_file(name: &str) -> bool {
    name.starts_with("terrain3d") && name.ends_with(".res")
}

fn scan(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut holds = false;
    let mut below = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if name == terrain3d_cache::DIR_NAME {
                holds |= entry.path().join(terrain3d_cache::MANIFEST).is_file();
            } else if !name.starts_with('.') && !(depth == 0 && name == "addons") {
                below.push(entry.path());
            }
        } else if is_region_file(&name) {
            holds = true;
        }
    }

    if holds
        && depth > 0
        && let Ok(rel) = dir.strip_prefix(root)
    {
        out.push(format!("res://{}", rel.to_string_lossy().replace('\\', "/")));
    }

    if depth < SCAN_DEPTH {
        below.into_iter().for_each(|path| scan(root, &path, depth + 1, out));
    }
}

impl EditorState {
    /// Adds the map's Terrain3D node for `data_directory` as one undo step. `create` first makes the folder's cache with
    /// the size and spacing it names, and the node gets its blank regions as edits that the next save sends to Godot.
    pub fn link_terrain3d(&mut self, data_directory: &str, create: Option<&NewFolder>) -> Result<NodeId, String> {
        let root = self.game.project_root.clone().ok_or("Open a Godot project first, Terrain3D data lives in one")?;
        if let Some(existing) = self.doc.map.nodes.iter().find(|(_, n)| matches!(n.kind, NodeKind::Terrain3D(_))).map(|(id, _)| *id) {
            return Err(format!("The map links Terrain3D already (node {existing}), delete that node to link another folder"));
        }

        let cache = terrain3d_cache::cache_dir(&root, data_directory).ok_or_else(|| format!("{data_directory} is not a folder inside the project"))?;
        let folder = cache.parent().ok_or("no folder")?;
        let mut locs = Vec::new();
        if create.is_none() && !holds_terrain3d(folder) {
            return Err(format!("{data_directory} holds no Terrain3D data, check the folder name or link a new folder"));
        }

        if let Some(new) = create {
            if let Some(problem) = new.problem() {
                return Err(problem.english().into());
            }

            if folder.read_dir().is_ok_and(|mut d| d.next().is_some()) {
                return Err(format!("{} exists already, pick it from the list", new.data_directory()));
            }

            let manifest = Manifest {
                format: terrain3d_cache::FORMAT.into(),
                version: terrain3d_cache::VERSION,
                terrain3d: String::new(),
                godot: String::new(),
                data_version: 0.0,
                region_size: new.region_size,
                vertex_spacing: new.vertex_spacing,
                material: Default::default(),
                map_transform_identity: true,
                editor_open: false,
                regions: Vec::new(),
                dirty: Vec::new(),
                textures: Vec::new(),
                applied: Vec::new(),
            };
            terrain3d_cache::write_manifest(&cache, &manifest).map_err(|e| e.to_string())?;
            locs = new.locs();
        }

        self.terrain3d.recheck();
        self.terrain3d.refresh(&cache);
        let layer = self.valid_layer();
        let source = self.terrain3d.source(&cache);
        let mut failed = None;
        let id = self.doc.edit("Link Terrain3D", |m, s| {
            let id = m.insert(layer, NodeKind::Terrain3D(Terrain3DLink::new(data_directory)));
            if let (Some(source), Some(link)) = (&source, m.terrain3d_mut(id))
                && let Err(e) = link.add_regions(source, &locs)
            {
                failed = Some(e);
            }

            s.clear();
            s.select_node(id);
            id
        });
        if let Some(e) = failed {
            self.doc.undo();
            return Err(e);
        }

        let folder = data_directory.to_string();
        self.set_status(msg!("Linked the Terrain3D data in {folder}", folder));
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn regions_center_on_the_origin() {
        let at = |columns, rows| NewFolder { columns, rows, ..Default::default() }.locs();
        assert_eq!(at(1, 1), vec![[0, 0]]);
        assert_eq!(at(2, 2), vec![[-1, -1], [0, -1], [-1, 0], [0, 0]]);
        assert_eq!(at(3, 2), vec![[-1, -1], [0, -1], [1, -1], [-1, 0], [0, 0], [1, 0]]);
        assert_eq!(at(4, 1), vec![[-2, 0], [-1, 0], [0, 0], [1, 0]]);
        let all = at(32, 32);
        assert_eq!(all.len(), 1024);
        assert_eq!((all[0], all[1023]), ([LOC_MIN, LOC_MIN], [LOC_MAX, LOC_MAX]));
    }

    #[test]
    fn a_new_folder_is_checked_at_every_boundary() {
        let base = NewFolder::default();
        assert_eq!(base.problem(), None);
        let with = |f: &dyn Fn(&mut NewFolder)| {
            let mut n = base.clone();
            f(&mut n);
            n.problem()
        };
        assert_eq!(with(&|n| n.name.clear()), Some(Problem::NoName));
        for bad in [".hidden", "a b", "a/b", "..", "tér", "a\\b"] {
            assert_eq!(with(&|n| n.name = bad.into()), Some(Problem::BadName), "{bad}");
        }

        assert_eq!(with(&|n| n.name = "levels-1.v2_x".into()), None);
        assert_eq!(with(&|n| n.region_size = 100), Some(Problem::RegionSize));
        assert_eq!(with(&|n| n.region_size = 2048), None);
        assert_eq!(with(&|n| n.vertex_spacing = 0.24), Some(Problem::Spacing));
        assert_eq!(with(&|n| n.vertex_spacing = 0.25), None);
        assert_eq!(with(&|n| n.vertex_spacing = 100.0), None);
        assert_eq!(with(&|n| n.vertex_spacing = 100.5), Some(Problem::Spacing));
        assert_eq!(with(&|n| n.vertex_spacing = f64::NAN), Some(Problem::Spacing));
        assert_eq!(with(&|n| n.columns = 0), Some(Problem::Regions));
        assert_eq!(with(&|n| n.columns = 32), None);
        assert_eq!(with(&|n| n.rows = 33), Some(Problem::Regions));
    }

    #[test]
    fn the_scan_lists_folders_with_regions_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("terrain3d_data/terrain3d_-01_02.res"));
        touch(&root.join("levels/forest/terrain3d_00_00.res"));
        touch(&root.join("fresh/.trenchdot/manifest.json"));
        touch(&root.join("addons/terrain_3d/terrain3d_x.res"));
        touch(&root.join(".godot/terrain3d_x.res"));
        touch(&root.join("empty/readme.txt"));
        touch(&root.join("other/terrain.res"));
        touch(&root.join("terrain3d_root.res"));
        assert_eq!(data_folders(root), ["res://fresh", "res://levels/forest", "res://terrain3d_data"]);

        std::fs::remove_dir_all(root.join("levels")).unwrap();
        touch(&root.join("z/terrain3d_01_01.res"));
        assert_eq!(data_folders(root), ["res://fresh", "res://terrain3d_data", "res://z"]);
    }

    fn state_in(dir: &Path) -> EditorState {
        let mut state = EditorState::new(Default::default());
        state.game.project_root = Some(dir.to_path_buf());
        state
    }

    #[test]
    fn linking_a_new_folder_adds_its_blank_regions_in_one_undo_step() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_in(dir.path());
        let new = NewFolder { name: "world".into(), region_size: 128, vertex_spacing: 2.5, columns: 3, rows: 2 };
        let id = state.link_terrain3d(&new.data_directory(), Some(&new)).unwrap();
        let cache = terrain3d_cache::cache_dir(dir.path(), "res://world").unwrap();
        let manifest = terrain3d_cache::read_manifest(&cache).unwrap();
        assert_eq!((manifest.region_size, manifest.vertex_spacing, manifest.regions.len()), (128, 2.5, 0));
        let link = state.doc.map.terrain3d(id).unwrap();
        assert_eq!(link.data_directory, "res://world");
        let mut want = new.locs();
        want.sort();
        assert_eq!(link.edits.keys().copied().collect::<Vec<_>>(), want);
        assert!(link.edits.values().all(|e| e.base_hash.is_none() && e.region.size == 128));
        assert_eq!(state.doc.selection.nodes.iter().copied().collect::<Vec<_>>(), vec![id]);

        state.doc.undo();
        assert!(!state.doc.map.has_terrain3d());
        state.doc.redo();
        assert_eq!(state.doc.map.terrain3d(id).unwrap().edits.len(), 6);
        state.doc.undo();
        assert!(state.link_terrain3d("res://world", None).is_ok(), "a folder holding only its cache links");
    }

    #[test]
    fn a_second_link_and_a_used_folder_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_in(dir.path());
        touch(&dir.path().join("taken/terrain3d_00_00.res"));
        let taken = NewFolder { name: "taken".into(), ..Default::default() };
        assert!(state.link_terrain3d("res://taken", Some(&taken)).unwrap_err().contains("exists already"));
        assert!(!state.doc.map.has_terrain3d());
        std::fs::create_dir(dir.path().join("empty")).unwrap();
        touch(&dir.path().join("empty/terrain3d.tres"));
        for missing in ["res://nope", "res://empty"] {
            assert!(state.link_terrain3d(missing, None).unwrap_err().contains("holds no Terrain3D data"), "{missing}");
            assert!(!state.doc.map.has_terrain3d() && !state.doc.history.can_undo(), "{missing} linked nothing");
        }

        assert!(state.link_terrain3d("res://taken", None).is_ok());
        let again = NewFolder { name: "second".into(), ..Default::default() };
        assert!(state.link_terrain3d("res://second", Some(&again)).unwrap_err().contains("links Terrain3D already"));
        assert!(!dir.path().join("second").exists());
        assert!(state.link_terrain3d("res://../out", None).is_err());
    }

    #[test]
    fn linking_without_a_project_or_a_valid_folder_changes_nothing() {
        let mut state = EditorState::new(Default::default());
        assert!(state.link_terrain3d("res://x", None).unwrap_err().contains("Open a Godot project"));
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_in(dir.path());
        let bad = NewFolder { name: "a b".into(), ..Default::default() };
        assert_eq!(state.link_terrain3d("res://a b", Some(&bad)).unwrap_err(), Problem::BadName.english());
        assert!(!state.doc.map.has_terrain3d());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
