//! The first room tests in Godot build with overrides made by Override in project. This fails when the code writes
//! them differently than the committed fixture; regenerate with `cargo run -p td_editor --example export_override_fixture`.

use std::path::PathBuf;

use td_editor::entity_override::{FIXTURE_DIR, PLAYER_SCRIPT, fixture};

#[test]
fn the_override_fixture_is_what_override_in_project_writes() {
    let godot = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../godot");
    let files = fixture(&godot).unwrap();
    let names: Vec<&str> = files.iter().map(|(n, _)| n.trim_start_matches(FIXTURE_DIR)).collect();
    assert_eq!(
        names,
        [
            "/func_button.gd",
            "/func_button.tres",
            "/info_player_start.gd",
            "/info_player_start.tres",
            "/project_fgd.tres",
            "/project_game_config.tres",
            "/project_map_settings.tres"
        ]
    );
    assert_eq!(files[2].1, PLAYER_SCRIPT);

    for (name, text) in &files {
        let on_disk = std::fs::read_to_string(godot.join(name)).unwrap_or_default().replace("\r\n", "\n");
        assert_eq!(on_disk, *text, "{name} is out of date, run the export_override_fixture example");
    }

    for entry in std::fs::read_dir(godot.join(FIXTURE_DIR)).unwrap().flatten() {
        let name = format!("{FIXTURE_DIR}/{}", entry.file_name().to_string_lossy());
        let known = files.iter().any(|(n, _)| *n == name || name == format!("{n}.uid"));
        assert!(known, "{name} is left over, run the export_override_fixture example");
    }
}
