//! Terrain3D worlds as the editor sees them: the cache Godot exports, and the edits TrenchDot sends back.

pub mod bridge;
pub mod dialog;
pub mod inspector;
pub mod link;
pub mod overlay;
pub mod store;
pub mod tiles;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use td_core::NodeId;
use td_doc::map::NodeKind;
use td_formats::terrain3d_cache;
use td_geom::terrain3d::Loc;

use crate::i18n::msg;
use crate::state::EditorState;

/// The material name the tiles use for Terrain3D texture `id`.
pub fn material_name(id: u8) -> String {
    format!("terrain3d/{id}")
}

/// How long one frame may spend building tiles.
pub const TILE_BUDGET: Duration = Duration::from_millis(12);

/// How often an idle editor looks at the manifests of the linked folders.
pub const POLL_EVERY: Duration = Duration::from_secs(1);

impl EditorState {
    pub(crate) fn terrain3d_links(&self) -> Vec<(NodeId, PathBuf)> {
        let Some(root) = self.game.project_root.as_deref() else { return Vec::new() };
        links_in(&self.doc.map, root)
    }

    /// Keeps every open map that sent to the data folder of an apply Godot never answered unsaved, its edits may only
    /// live in the document now.
    pub(crate) fn keep_unanswered_terrain3d(&mut self, folder: &Path) {
        let Some(root) = self.game.project_root.clone() else { return };
        let sent_there = |map: &td_doc::Map| links_in(map, &root).iter().any(|(_, cache)| cache.parent() == Some(folder));
        if sent_there(&self.doc.map) {
            self.doc.mark_unsaved();
        }

        for tab in &mut self.tabs {
            if sent_there(&tab.doc.map) {
                tab.doc.mark_unsaved();
            }
        }
    }

    /// Writes the regions edited in TrenchDot as pending files and has Godot apply them. Called after a save.
    pub fn send_terrain3d(&mut self, forced: &BTreeSet<Loc>) {
        let Some(project) = self.game.project_root.clone() else { return };
        for (id, cache) in self.terrain3d_links() {
            self.terrain3d.recheck();
            self.terrain3d.refresh(&cache);
            let Some(link) = self.doc.map.terrain3d(id) else { continue };
            let Some(source) = self.terrain3d.source(&cache) else { continue };
            let changes = link.changed_regions(&source);
            let written = bridge::write_pending(&cache, &changes, &td_doc::terrain3d::RegionSource::grid(&source), forced)
                .map_err(|e| e.to_string())
                .and_then(|_| bridge::prune_pending(&cache, link, &changes).map_err(|e| e.to_string()));
            if let Err(problem) = written {
                self.fail_status(msg!("Terrain3D edits were not applied: {problem}", problem));
                continue;
            }

            if terrain3d_cache::pending_locs(&cache).map_or(true, |locs| locs.is_empty()) {
                continue;
            }

            let Some(folder) = cache.parent().map(PathBuf::from) else { continue };
            if self.restricted() {
                continue;
            }

            self.terrain3d_bridge.send(bridge::Apply {
                port: self.prefs.live_link.then_some(self.prefs.live_link_port),
                project: project.clone(),
                godot: self.godot.exe.clone(),
                folder,
                force: !forced.is_empty(),
            });
        }
    }

    /// Has the open Godot editor change the region size and vertex spacing of the Terrain3D node `id` shows.
    pub fn change_terrain3d_settings(&mut self, id: NodeId, region_size: u32, spacing: f64) {
        let Some((_, cache)) = self.terrain3d_links().into_iter().find(|(node, _)| *node == id) else { return };
        let Some(folder) = cache.parent().map(PathBuf::from) else { return };
        if !self.godot_has_project() {
            self.fail_status(msg!("Needs the Godot editor open with this scene"));
            return;
        }

        self.terrain3d_bridge.send_settings(bridge::SettingsRequest { port: self.prefs.live_link_port, folder, region_size, spacing });
    }

    /// Takes back the edits an earlier session saved and Godot has not applied yet, so the map shows them again.
    pub fn restore_terrain3d(&mut self) {
        let mut problems = Vec::new();
        for (id, cache) in self.terrain3d_links() {
            self.terrain3d.recheck();
            self.terrain3d.refresh(&cache);
            let Some(size) = self.terrain3d.grid(&cache).map(|g| g.region_size) else { continue };
            let Some(link) = self.doc.map.terrain3d_mut(id) else { continue };
            problems.extend(bridge::restore_pending(&cache, size, link).problems);
        }

        if !problems.is_empty() {
            let problem = problems.join("; ");
            self.extend_status(". ", msg!("Terrain3D edits saved earlier could not be read: {problem}", problem));
        }
    }

    /// Per frame: the answer of a running apply, and the manifests of the linked folders. True while something wants the
    /// next frame in a second.
    pub fn poll_terrain3d(&mut self) -> bool {
        if let Some(outcome) = self.terrain3d_bridge.poll() {
            if outcome.unanswered {
                self.keep_unanswered_terrain3d(&outcome.folder);
            }

            if outcome.is_ok() {
                self.set_status(outcome.status());
            } else {
                self.fail_status(outcome.status());
            }

            self.terrain3d.recheck();
        }

        self.tell_held_saves();
        if let Some(answer) = self.terrain3d_bridge.poll_settings() {
            match answer {
                Ok(_) => self.set_status(msg!("Godot changed the Terrain3D settings, save the scene in Godot to keep them")),
                Err(problem) => self.fail_status(msg!("Terrain3D settings were not changed: {problem}", problem)),
            }

            self.terrain3d.recheck();
        }

        self.restore_owed_terrain3d();
        let links = self.terrain3d_links();
        for (_, cache) in &links {
            self.terrain3d.refresh(cache);
            let Some(manifest) = self.terrain3d.manifest(cache) else { continue };
            if store::untested_version(manifest) && self.terrain3d_noted.insert(cache.clone()) {
                let note = bridge::untested_version_note(&manifest.terrain3d);
                self.extend_status(". ", note);
            }
        }

        !links.is_empty() || self.terrain3d_bridge.busy()
    }

    /// Per frame: loads and builds the Terrain3D tiles the views need. True when the next frame should go on building.
    pub fn update_terrain3d_tiles(&mut self, views: &[tiles::View], budget: Duration) -> bool {
        let links = self.terrain3d_links();
        let map = &self.doc.map;
        let live: BTreeSet<NodeId> = links.iter().map(|(id, _)| *id).filter(|id| !map.is_hidden(*id) && map.in_cordon(*id)).collect();
        let mut changed = self.terrain3d_tiles.keep_only(&live);
        let mut pending = false;
        for (id, cache) in &links {
            if !live.contains(id) {
                continue;
            }

            self.terrain3d.refresh(cache);
            let (Some(grid), Some(link)) = (self.terrain3d.grid(cache), self.doc.map.terrain3d(*id)) else {
                changed |= self.terrain3d_tiles.clear(*id);
                continue;
            };

            let (locs, edited): (Vec<Loc>, Vec<Loc>) = match self.terrain3d.source(cache) {
                Some(source) => (link.locs(&source), link.edits.keys().copied().filter(|l| link.live_edit(&source, *l).is_some()).collect()),
                None => Default::default(),
            };
            changed |= self.terrain3d_tiles.set_layout(*id, tiles::Layout { grid, locs: locs.clone() });
            let exists: HashSet<Loc> = locs.iter().copied().collect();
            let owed = self.terrain3d_owed.entry(*id).or_default();
            owed.extend(self.terrain3d.take_news(cache));
            let mut plan = tiles::plan(&grid, &locs, views);
            // The scatter that follows the node is put on the new surface once its regions are loaded.
            for loc in owed.iter().filter(|l| exists.contains(*l)) {
                plan.insert(*loc, tiles::Detail::Full);
            }

            for loc in edited.iter().filter(|l| exists.contains(*l)) {
                plan.insert(*loc, tiles::Detail::Full);
            }

            let (full, lod) = tiles::wants(&plan, &|l| exists.contains(&l));
            self.terrain3d.want(cache, &full, &lod);

            let store = &self.terrain3d;
            let source = store.source(cache);
            let hashes: HashMap<Loc, [u8; 32]> = store.manifest(cache).map(|m| m.regions.iter().map(|r| (r.loc, r.hash.0)).collect()).unwrap_or_default();
            let region = |loc: Loc| {
                if let Some(edit) = source.as_ref().and_then(|s| link.live_edit(s, loc)) {
                    return Some((edit.region.clone(), tiles::Source::Edited(Arc::as_ptr(&edit.region) as usize)));
                }

                if link.removed.contains_key(&loc) {
                    return None;
                }

                Some((store.region(cache, loc)?, tiles::Source::Cached(*hashes.get(&loc)?)))
            };
            let lod = |loc: Loc| if link.removed.contains_key(&loc) { None } else { store.lod(cache, loc) };
            let material = material_name;
            let world = tiles::World { grid, region: &region, lod: &lod, material: &material };
            let focus = views.first().map_or(td_core::DVec2::ZERO, tiles::focus);
            let mut order: Vec<Loc> = plan.keys().copied().collect();
            order.sort_by(|a, b| {
                let d = |l: &Loc| {
                    let (lo, hi) = tiles::region_rect(&grid, *l);
                    (lo + hi).distance_squared(focus * 2.0)
                };
                d(a).total_cmp(&d(b))
            });
            let (did, more) = tiles::rebuild(self.terrain3d_tiles.entries(*id), &world, &plan, &order, budget);
            changed |= did;
            pending |= more;
        }

        let key = (links.iter().map(|(_, c)| self.terrain3d.generation(c)).sum::<u64>(), self.doc.revision);
        if self.terrain3d_tiles.conflicts_stale(key) {
            let mut found = BTreeMap::new();
            for (id, cache) in &links {
                let (Some(link), Some(source)) = (self.doc.map.terrain3d(*id), self.terrain3d.source(cache)) else { continue };
                let locs: BTreeSet<Loc> = link.changed_regions(&source).into_iter().filter(|c| c.conflict).map(|c| c.loc).collect();
                if !locs.is_empty() {
                    found.insert(*id, locs);
                }
            }

            self.terrain3d_tiles.set_conflicts(key, found);
            changed = true;
        }

        self.reground_owed(&links, &live);
        if changed {
            self.terrain3d_tiles.generation += 1;
        }

        self.terrain3d_tiles.pending = pending;
        pending
    }
}

fn links_in(map: &td_doc::Map, root: &Path) -> Vec<(NodeId, PathBuf)> {
    map.nodes
        .iter()
        .filter_map(|(id, node)| match &node.kind {
            NodeKind::Terrain3D(link) => Some((*id, terrain3d_cache::cache_dir(root, &link.data_directory)?)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use td_core::DVec3;
    use td_doc::terrain::{SculptBrush, SculptMode};
    use td_doc::terrain3d::ChangeKind;
    use td_formats::terrain3d_cache::{Applied, CachedRegion, ManifestRegion, Refused, RegionHash as FileHash};

    use super::*;
    use crate::terrain3d::brush::tests::state_over;

    const L: Loc = [1, -1];
    const M: Loc = [2, -1];

    /// A Raise dab in region `loc`, which is 5120 units wide.
    fn dab(state: &mut EditorState, id: NodeId, loc: Loc, x: f64) {
        let brush = SculptBrush { mode: SculptMode::Raise, radius: 400.0, strength: 40.0, ..Default::default() };
        let center = DVec3::new(loc[0] as f64 * 5120.0 + x, 0.0, loc[1] as f64 * 5120.0 + 1900.0);
        assert_eq!(state.sculpt_terrain3d(&[id], center, &brush, brush::Texture { id: 0, spray: false }), 1);
    }

    /// Godot applying the pending file of `L` the way the addon does: its round trip leaves other bits, the region gets a
    /// new hash and the manifest reports it.
    fn apply_in_godot(state: &mut EditorState, cache: &Path, seq: u64, salt: u8) {
        let pending = terrain3d_cache::read_pending(cache, L, 64).unwrap();
        std::fs::remove_file(terrain3d_cache::pending_path(cache, L).unwrap()).unwrap();
        let mut region = pending.region;
        region.heights[5] = f32::from_bits(region.heights[5].to_bits() + 1);
        terrain3d_cache::write_region(cache, &CachedRegion { region, vertex_spacing: pending.vertex_spacing, hash: FileHash([salt; 32]) }).unwrap();
        let mut manifest = terrain3d_cache::read_manifest(cache).unwrap();
        manifest.regions.retain(|r| r.loc != L);
        manifest.regions.push(ManifestRegion { loc: L, hash: FileHash([salt; 32]), height_range: [0.0, 0.0] });
        manifest.applied.push(Applied { seq, accepted: vec![L], refused: vec![] });
        terrain3d_cache::write_manifest(cache, &manifest).unwrap();
        state.terrain3d.recheck();
        state.poll_terrain3d();
    }

    /// Godot applying the pending removal of `L`.
    fn remove_in_godot(state: &mut EditorState, cache: &Path, seq: u64) {
        std::fs::remove_file(terrain3d_cache::pending_path(cache, L).unwrap()).unwrap();
        std::fs::remove_file(terrain3d_cache::region_path(cache, L).unwrap()).unwrap();
        let mut manifest = terrain3d_cache::read_manifest(cache).unwrap();
        manifest.regions.retain(|r| r.loc != L);
        manifest.applied.push(Applied { seq, accepted: vec![L], refused: vec![] });
        terrain3d_cache::write_manifest(cache, &manifest).unwrap();
        state.terrain3d.recheck();
        state.poll_terrain3d();
    }

    fn changes(state: &EditorState, id: NodeId, cache: &Path) -> Vec<(Loc, ChangeKind, Option<RegionHash>, bool)> {
        let source = state.terrain3d.source(cache).unwrap();
        state.doc.map.terrain3d(id).unwrap().changed_regions(&source).into_iter().map(|c| (c.loc, c.kind, c.base_hash, c.conflict)).collect()
    }

    #[test]
    fn a_region_godot_applied_sends_cleanly_again_and_undo_past_it_sends_the_old_one_back() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[L, M]);
        state.prefs.live_link = false;
        state.godot.exe = None;
        let original = state.terrain3d.region(&cache, L).unwrap();
        dab(&mut state, id, L, 1000.0);
        state.send_terrain3d(&BTreeSet::new());
        assert_eq!(terrain3d_cache::pending_locs(&cache).unwrap(), vec![L]);
        apply_in_godot(&mut state, &cache, 1, 2);
        assert!(changes(&state, id, &cache).is_empty(), "Godot holds the stroke");

        dab(&mut state, id, L, 3000.0);
        assert_eq!(changes(&state, id, &cache), vec![(L, ChangeKind::Edit, Some([2; 32]), false)], "the second stroke is no conflict");
        state.send_terrain3d(&BTreeSet::new());
        apply_in_godot(&mut state, &cache, 2, 3);
        assert!(changes(&state, id, &cache).is_empty());

        state.doc.undo();
        state.doc.undo();
        let source = state.terrain3d.source(&cache).unwrap();
        assert_eq!(state.doc.map.terrain3d(id).unwrap().region(&source, L).unwrap().heights, original.heights, "back to before the first stroke");
        assert_eq!(changes(&state, id, &cache), vec![(L, ChangeKind::Edit, Some([3; 32]), false)]);
        state.send_terrain3d(&BTreeSet::new());
        assert_eq!(terrain3d_cache::read_pending(&cache, L, 64).unwrap().region.heights, original.heights);
    }

    /// Rebuilds the tiles until `done` holds, the regions they want load in the background.
    fn build_tiles_until(state: &mut EditorState, done: impl Fn(&EditorState) -> bool) {
        let everywhere = [tiles::View::Flat { min: td_core::DVec2::splat(-1.0e7), max: td_core::DVec2::splat(1.0e7), zoom: 1.0 }];
        let start = std::time::Instant::now();
        loop {
            state.update_terrain3d_tiles(&everywhere, Duration::from_secs(5));
            if done(state) {
                return;
            }

            assert!(start.elapsed().as_secs() < 20, "the tiles were not built");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn tile_source(state: &EditorState, id: NodeId, loc: Loc) -> Option<tiles::Source> {
        state.terrain3d_tiles.node(id).find(|(l, _)| *l == loc).map(|(_, e)| e.key.parts[0])
    }

    #[test]
    fn tiles_follow_godot_once_it_changes_or_removes_a_region_it_applied() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[L, M]);
        state.prefs.live_link = false;
        state.godot.exe = None;
        dab(&mut state, id, L, 1000.0);
        state.send_terrain3d(&BTreeSet::new());
        apply_in_godot(&mut state, &cache, 1, 2);
        build_tiles_until(&mut state, |s| matches!(tile_source(s, id, L), Some(tiles::Source::Edited(_))));

        let mut region = (*state.doc.map.terrain3d(id).unwrap().edits[&L].region).clone();
        region.heights[9] = 2560.0;
        terrain3d_cache::write_region(&cache, &CachedRegion { region, vertex_spacing: 2.5, hash: FileHash([7; 32]) }).unwrap();
        let mut manifest = terrain3d_cache::read_manifest(&cache).unwrap();
        manifest.regions.iter_mut().find(|r| r.loc == L).unwrap().hash = FileHash([7; 32]);
        terrain3d_cache::write_manifest(&cache, &manifest).unwrap();
        state.terrain3d.recheck();
        state.poll_terrain3d();
        build_tiles_until(&mut state, |s| tile_source(s, id, L) == Some(tiles::Source::Cached([7; 32])));

        std::fs::remove_file(terrain3d_cache::region_path(&cache, L).unwrap()).unwrap();
        let mut manifest = terrain3d_cache::read_manifest(&cache).unwrap();
        manifest.regions.retain(|r| r.loc != L);
        terrain3d_cache::write_manifest(&cache, &manifest).unwrap();
        state.terrain3d.recheck();
        state.poll_terrain3d();
        build_tiles_until(&mut state, |s| tile_source(s, id, L).is_none());
        assert_eq!(state.terrain3d_tiles.layouts[&id].locs, vec![M]);
    }

    #[test]
    fn a_save_has_godot_build_the_map_only_after_the_terrain3d_apply_answered() {
        use std::io::{BufRead, BufReader, Write};
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, _) = state_over(dir.path(), &[L, M]);
        state.godot.exe = None;
        state.prefs.hot_reload = false;
        state.prefs.live_link = true;
        state.trust_project(dir.path(), false);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        state.prefs.live_link_port = listener.local_addr().unwrap().port();
        let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let seen = seen.clone();
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
                    let message: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                    let event = message["event"].as_str().unwrap_or_default().to_string();
                    seen.lock().unwrap().push(event.clone());
                    if event == "terrain3d_apply" {
                        std::thread::sleep(Duration::from_millis(400));
                        seen.lock().unwrap().push("applied".into());
                        let _ = writeln!(&stream, "{}", serde_json::json!({ "ok": true, "accepted": [L], "refused": [], "problems": [] }));
                    } else {
                        let _ = writeln!(&stream, "{}", serde_json::json!({ "ok": true }));
                    }
                });
            }
        });

        dab(&mut state, id, L, 1000.0);
        state.save_map(&dir.path().join("valley.tdm")).unwrap();
        let start = std::time::Instant::now();
        while log.lock().unwrap().len() < 3 {
            assert!(start.elapsed() < Duration::from_secs(10), "{:?}", log.lock().unwrap());
            state.poll_terrain3d();
            std::thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(log.lock().unwrap()[..3], ["terrain3d_apply", "applied", "map_saved"]);
    }

    #[test]
    fn a_godot_that_dies_after_taking_the_pending_files_leaves_the_edits_unsent() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[L, M]);
        state.prefs.live_link = false;
        state.trust_project(dir.path(), false);
        let pending = terrain3d_cache::pending_path(&cache, L).unwrap();
        let (exe, script) = if cfg!(windows) {
            (
                dir.path().join("godot.cmd"),
                format!(
                    "@echo off
del /q \"{}\"
exit /b 3
",
                    pending.display()
                ),
            )
        } else {
            (
                dir.path().join("godot.sh"),
                format!(
                    "#!/bin/sh
rm -f '{}'
exit 3
",
                    pending.display()
                ),
            )
        };
        std::fs::write(&exe, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        state.godot.exe = Some(exe);
        dab(&mut state, id, L, 1000.0);
        let map = dir.path().join("valley.tdm");
        state.save_map(&map).unwrap();
        let start = std::time::Instant::now();
        while state.terrain3d_bridge.busy() {
            assert!(start.elapsed() < Duration::from_secs(20), "the apply did not finish");
            state.poll_terrain3d();
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(!pending.exists(), "Godot took the file");
        assert!(state.status.contains("exit code 3") && state.status.contains("by hand"), "{}", state.status);
        assert!(state.doc.is_modified(), "closing now would lose the edit, so the map asks to be saved");
        assert!(state.doc.map.terrain3d(id).unwrap().edits.contains_key(&L));

        state.godot.exe = None;
        state.save_map(&map).unwrap();
        assert!(pending.exists(), "the next save writes the edit again");
    }

    #[test]
    fn unsaved_changes_in_godot_stop_counting_once_its_editor_quit() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[L, M]);
        state.prefs.live_link = false;
        state.godot.exe = None;
        state.link_state.connected = true;
        state.link_state.project = Some(crate::live_link::path_key(&crate::live_link::godot_path(dir.path())));
        dab(&mut state, id, L, 1000.0);
        let mut manifest = terrain3d_cache::read_manifest(&cache).unwrap();
        manifest.dirty = vec![M];
        manifest.applied.push(Applied { seq: 1, accepted: vec![], refused: vec![Refused { loc: L, reason: bridge::DIRTY.into() }] });
        terrain3d_cache::write_manifest(&cache, &manifest).unwrap();
        state.terrain3d.recheck();
        state.poll_terrain3d();
        assert!(inspector::unsaved_in_godot(&state, &cache, L) && inspector::unsaved_in_godot(&state, &cache, M));

        state.link_state.connected = false;
        state.poll_terrain3d();
        assert!(!inspector::unsaved_in_godot(&state, &cache, L) && !inspector::unsaved_in_godot(&state, &cache, M));
        assert!(state.terrain3d.refused(&cache).is_empty());

        state.link_state.connected = true;
        assert!(!inspector::unsaved_in_godot(&state, &cache, L), "the refusal went with the editor that quit");
        assert!(inspector::unsaved_in_godot(&state, &cache, M), "a Godot editor open again reports its own unsaved regions");
    }

    #[test]
    fn taking_godots_region_settles_its_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[L, M]);
        state.prefs.live_link = false;
        state.godot.exe = None;
        dab(&mut state, id, L, 1000.0);
        let mut manifest = terrain3d_cache::read_manifest(&cache).unwrap();
        manifest.applied.push(Applied { seq: 1, accepted: vec![], refused: vec![Refused { loc: L, reason: "changed".into() }] });
        terrain3d_cache::write_manifest(&cache, &manifest).unwrap();
        state.terrain3d.recheck();
        state.poll_terrain3d();
        assert_eq!(state.terrain3d.refused(&cache).keys().collect::<Vec<_>>(), vec![&L]);

        state.use_godots_terrain3d(id, L);
        assert!(state.terrain3d.refused(&cache).is_empty());
    }

    #[test]
    fn a_region_added_back_after_godot_removed_it_is_a_plain_add() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[L, M]);
        state.prefs.live_link = false;
        state.godot.exe = None;
        let source = state.terrain3d.source(&cache).unwrap();
        state.doc.edit("Remove", |m, _| m.terrain3d_mut(id).unwrap().remove_regions(&source, &[L]));
        state.send_terrain3d(&BTreeSet::new());
        remove_in_godot(&mut state, &cache, 1);
        assert!(state.doc.map.terrain3d(id).unwrap().removed.is_empty(), "the removal is settled");
        assert!(changes(&state, id, &cache).is_empty());

        let source = state.terrain3d.source(&cache).unwrap();
        state.doc.edit("Add", |m, _| m.terrain3d_mut(id).unwrap().add_regions(&source, &[L]).unwrap());
        assert_eq!(changes(&state, id, &cache), vec![(L, ChangeKind::Add, None, false)]);
        state.send_terrain3d(&BTreeSet::new());
        apply_in_godot(&mut state, &cache, 2, 4);
        assert!(changes(&state, id, &cache).is_empty());
        assert!(state.doc.map.terrain3d(id).unwrap().removed.is_empty());

        state.doc.undo();
        state.doc.undo();
        assert_eq!(changes(&state, id, &cache), vec![(L, ChangeKind::Edit, Some([4; 32]), false)], "undo past the removal sends the region back");
    }

    #[test]
    fn edits_saved_earlier_come_back_once_the_project_and_manifest_load() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, _) = state_over(dir.path(), &[L, M]);
        state.prefs.live_link = false;
        state.godot.exe = None;
        dab(&mut state, id, M, 2000.0);
        let sculpted = state.doc.map.terrain3d(id).unwrap().edits[&M].region.clone();
        state.send_terrain3d(&BTreeSet::new());
        // The map as the next session reads it, the edits are not in the file.
        state.doc.map.terrain3d_mut(id).unwrap().edits.clear();
        let root = state.game.project_root.take();
        state.terrain3d_unrestored.insert((state.doc.uid() + 1, id));

        state.restore_terrain3d();
        state.poll_terrain3d();
        assert!(state.doc.map.terrain3d(id).unwrap().edits.is_empty(), "no project yet");
        assert_eq!(state.terrain3d_unrestored.len(), 2);

        state.game.project_root = root;
        state.poll_terrain3d();
        let link = state.doc.map.terrain3d(id).unwrap();
        assert_eq!(link.edits.keys().copied().collect::<Vec<_>>(), [M]);
        assert_eq!(link.edits[&M].region.heights, sculpted.heights);
        assert_eq!(state.terrain3d_unrestored, HashSet::from([(state.doc.uid() + 1, id)]), "another document's node waits for that document");

        state.doc.map.terrain3d_mut(id).unwrap().edits.clear();
        state.poll_terrain3d();
        assert!(state.doc.map.terrain3d(id).unwrap().edits.is_empty(), "the edits come back once");
    }

    #[test]
    fn a_first_stroke_undone_after_the_save_takes_its_pending_file_back() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[L, M]);
        state.prefs.live_link = false;
        state.godot.exe = None;
        dab(&mut state, id, M, 2000.0);
        state.send_terrain3d(&BTreeSet::new());
        assert_eq!(terrain3d_cache::pending_locs(&cache).unwrap(), vec![M]);

        state.doc.undo();
        assert!(state.doc.map.terrain3d(id).unwrap().edits.is_empty());
        state.send_terrain3d(&BTreeSet::new());
        assert!(terrain3d_cache::pending_locs(&cache).unwrap().is_empty(), "Godot must not apply what was undone");
    }

    #[test]
    fn a_send_in_restricted_mode_leaves_the_file_for_godot_and_says_why_it_went_no_further() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, id, cache) = state_over(dir.path(), &[L, M]);
        state.prefs.live_link = false;
        state.godot.exe = None;
        assert!(state.restricted());
        dab(&mut state, id, M, 2000.0);
        state.send_terrain3d(&BTreeSet::new());
        assert_eq!(terrain3d_cache::pending_locs(&cache).unwrap(), vec![M]);
        assert!(state.status_failed() && state.status.contains("Restricted Mode"), "{}", state.status);
        assert!(!state.terrain3d_bridge.busy());

        state.trust_for_session(dir.path());
        state.set_status("Saved");
        state.send_terrain3d(&BTreeSet::new());
        assert!(!state.status_failed() && state.status == "Saved", "{}", state.status);
        assert!(state.terrain3d_bridge.busy(), "a trusted project reaches Godot");
    }

    #[test]
    fn a_send_before_the_data_loads_keeps_the_map_unsaved_only_when_it_holds_edits() {
        let dir = tempfile::tempdir().unwrap();
        let (mut state, _, _) = state_over(dir.path(), &[L]);
        state.prefs.live_link = false;
        state.godot.exe = None;
        let layer = state.doc.map.default_layer();
        let other = state.doc.map.insert(layer, NodeKind::Terrain3D(td_doc::terrain3d::Terrain3DLink::new("res://elsewhere")));
        state.doc.mark_saved();
        state.set_status("Saved");
        state.send_terrain3d(&BTreeSet::new());
        assert!(!state.doc.is_modified() && state.status == "Saved", "nothing of the unloaded folder to lose: {}", state.status);

        let region = Arc::new(Region3D::blank([-2, 3], 64));
        state.doc.edit("Edit", |m, _| {
            m.terrain3d_mut(other).unwrap().edits.insert([-2, 3], td_doc::terrain3d::RegionEdit::new(region, None));
        });
        state.doc.mark_saved();
        state.send_terrain3d(&BTreeSet::new());
        assert!(state.doc.is_modified(), "closing must still ask");
        assert!(state.status_failed() && state.status.contains("res://elsewhere"), "{}", state.status);
    }
}
