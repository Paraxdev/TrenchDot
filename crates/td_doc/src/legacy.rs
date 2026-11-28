//! The names TrenchDot went by as GodotTrench, so maps, projects and installs made back then keep working.

use std::path::Path;

/// The product's old name, as the addon's plugin.cfg and status messages spell it.
pub const NAME: &str = "GodotTrench";

/// The names an old addon's plugin.cfg may carry. Beta builds briefly called it LevelBench, with the GodotTrench names inside.
pub const ADDON_NAMES: [&str; 2] = [NAME, "LevelBench"];

/// Where the addon lived before it moved to `addons/trenchdot`. The original FuncGodot addon installs there too, so a
/// folder there is only ours when its plugin.cfg does not name [`UPSTREAM_ADDON`].
pub const ADDON_DIR: &str = "addons/func_godot";
pub const UPSTREAM_ADDON: &str = "FuncGodot";
/// The project.godot section the old addon kept its map settings in, which the original FuncGodot addon still uses.
pub const SETTINGS_SECTION: &str = "func_godot";

pub const MAP_FORMAT: &str = "godottrench-map";
pub const GAME_FORMAT: &str = "godottrench-game";
pub const GAME_FILE: &str = "godottrench_game.json";

/// Map extensions, the current one first.
pub const MAP_EXTS: [&str; 2] = ["tdm", "gtm"];

/// Whether `path` is a map file, by its extension.
pub fn is_map_path(path: &Path) -> bool {
    path.extension().is_some_and(|e| MAP_EXTS.iter().any(|m| e.eq_ignore_ascii_case(m)))
}

/// The extension that a map saved at `path` and its `.bak`, `.tmp` and `.autosave` files keep, so a map from the old
/// days stays where Godot scenes point at it.
pub fn map_ext(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| MAP_EXTS.iter().find(|m| e.eq_ignore_ascii_case(m)));
    ext.copied().unwrap_or(MAP_EXTS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_paths_by_extension() {
        assert!(is_map_path(Path::new("a/level.tdm")) && is_map_path(Path::new("level.GTM")));
        assert!(!is_map_path(Path::new("level.json")) && !is_map_path(Path::new("level.gtm.bak")) && !is_map_path(Path::new("gtm")));
    }

    #[test]
    fn saves_keep_the_extension_of_the_map() {
        assert_eq!(map_ext(Path::new("level.gtm")), "gtm");
        assert_eq!(map_ext(Path::new("LEVEL.GTM")), "gtm");
        assert_eq!(map_ext(Path::new("level.tdm")), "tdm");
        assert_eq!(map_ext(Path::new("level.json")), "tdm");
        assert_eq!(map_ext(Path::new("level")), "tdm");
    }
}
