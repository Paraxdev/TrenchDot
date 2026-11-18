//! Writes godot/tests/first_room/project/entities, the overrides the first room tests build with, the way Override in
//! project writes them. cargo run -p td_editor --example export_override_fixture

use std::path::PathBuf;

use td_editor::entity_override::{FIXTURE_DIR, fixture};

fn main() {
    let godot = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../godot");
    let files = fixture(&godot).expect("make the override fixture");
    let dir = godot.join(FIXTURE_DIR);
    std::fs::create_dir_all(&dir).expect("create fixture folder");
    for (name, text) in &files {
        std::fs::write(godot.join(name), text).expect("write fixture file");
    }

    for entry in std::fs::read_dir(&dir).expect("read fixture folder").flatten() {
        let name = format!("{FIXTURE_DIR}/{}", entry.file_name().to_string_lossy());
        let uid = name.ends_with(".uid") && files.iter().any(|(n, _)| name == format!("{n}.uid"));
        if !uid && !files.iter().any(|(n, _)| *n == name) {
            std::fs::remove_file(entry.path()).expect("remove stale fixture file");
            println!("removed {name}");
        }
    }

    println!("wrote {} files to {}", files.len(), dir.display());
}
