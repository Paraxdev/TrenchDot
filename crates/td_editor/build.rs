//! Hashes the addon in godot/addons/trenchdot, which the release zip is made of, so the editor knows the bytes of the
//! addon and the Gameplay pack it installs, see `code_scan.rs`. On Windows it also embeds the exe icon.

use std::path::{Path, PathBuf};

fn main() {
    // Read at run time, since worktrees sharing a target dir share this script's binary and env! would bake in the path
    // of whichever tree compiled it first.
    let crate_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let addon = crate_dir.join("../../godot/addons/trenchdot");
    println!("cargo::rerun-if-changed={}", addon.display());
    let mut files = Vec::new();
    walk(&addon, &addon, &mut files);
    files.sort();
    let mut out = String::from("&[\n");
    for (rel, hash) in files {
        out += &format!("    ({rel:?}, {hash:?}),\n");
    }

    out += "]\n";
    let path = Path::new(&std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("addon_files.rs");
    std::fs::write(&path, out).unwrap_or_else(|e| panic!("could not write {}: {e}", path.display()));

    let site = crate_dir.join("../../zensical.toml");
    println!("cargo::rerun-if-changed={}", site.display());
    let config = std::fs::read_to_string(&site).unwrap_or_else(|e| panic!("could not read {}: {e}", site.display()));
    let discord = config.lines().find_map(|l| l.strip_prefix("discord = \"")?.strip_suffix('"')).expect("zensical.toml sets discord under [project.extra]");
    println!("cargo::rustc-env=TRENCHDOT_DISCORD_URL={discord}");

    // Explorer, the Start menu entry and .tdm files show the exe's own icon, which egui's window icon does not set.
    #[cfg(windows)]
    embed_resource::compile("assets/trenchdot.rc", embed_resource::NONE).manifest_optional().expect("could not embed the exe icon");
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, [u8; 32])>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        // The release zip leaves these out, see ci.yml.
        if dir == root && rel.starts_with(".git") {
            continue;
        }

        match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk(root, &path, out),
            Ok(kind) if kind.is_file() => {
                let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
                let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
                out.push((rel, digest.as_ref().try_into().expect("SHA-256 has 32 bytes")));
            }
            _ => {}
        }
    }
}
