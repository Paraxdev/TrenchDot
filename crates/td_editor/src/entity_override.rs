//! Override in project, the text of the files it writes. An override is the project's own copy of an addon or pack
//! entity definition with the same classname, whose Script Class is a script of the project that extends the original.
//! The copy is made as text, so everything the definition holds (scene file, collision, base classes) stays as it was.

use crate::entity_pack;

/// Where the files go, relative to the project and as `res://`.
pub const DIR: &str = "entities";
pub const RES_DIR: &str = "res://entities";
pub const FGD: &str = "project_fgd.tres";
pub const MAP_SETTINGS: &str = "project_map_settings.tres";
pub const GAME_CONFIG: &str = "project_game_config.tres";
/// The first person player, the script an override of `info_player_start` gets instead of the template.
pub const PLAYER_SCRIPT: &str = include_str!("../../../godot/tests/first_room/player.gd");
pub const PLAYER_NODE_CLASS: &str = "CharacterBody3D";
/// The metadata key that tells the addon a classname defined twice is meant, see `Clash::intended`.
pub const META: &str = "trenchdot_override";

const SCRIPT_ID: &str = "td_override";
const FGD_FILE_SCRIPT: &str = "res://addons/func_godot/src/fgd/func_godot_fgd_file.gd";
const MAP_SETTINGS_SCRIPT: &str = "res://addons/func_godot/src/map/func_godot_map_settings.gd";
const GAME_CONFIG_SCRIPT: &str = "res://addons/func_godot/src/trenchdot/trenchdot_game_config.gd";

/// What the override's script is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// Extends the original script, or a template when the definition has none.
    Extend,
    /// The first person player, for `info_player_start`.
    Player,
}

pub fn definition_res(classname: &str) -> String {
    format!("{RES_DIR}/{classname}.tres")
}

pub fn script_res(classname: &str) -> String {
    format!("{RES_DIR}/{classname}.gd")
}

pub fn fgd_res() -> String {
    format!("{RES_DIR}/{FGD}")
}

pub fn map_settings_res() -> String {
    format!("{RES_DIR}/{MAP_SETTINGS}")
}

pub fn game_config_res() -> String {
    format!("{RES_DIR}/{GAME_CONFIG}")
}

fn split(text: &str) -> (Vec<String>, bool) {
    let ends = text.ends_with('\n');
    let body = text.strip_suffix('\n').unwrap_or(text);
    (body.split('\n').map(str::to_string).collect(), ends)
}

fn join(lines: &[String], ends: bool) -> String {
    let mut out = lines.join("\n");
    if ends {
        out.push('\n');
    }

    out
}

/// The value of ` name="..."` on a line of a resource header or an ext_resource.
fn attr<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let from = line.find(&key)? + key.len();
    let len = line[from..].find('"')?;
    Some(&line[from..from + len])
}

fn is_ext(line: &str) -> bool {
    line.starts_with("[ext_resource ")
}

fn ext_ref(id: &str) -> String {
    format!("ExtResource(\"{id}\")")
}

fn value_of<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.trim_end().strip_prefix(key)?.strip_prefix(" = ")
}

fn quoted(value: &str) -> Option<&str> {
    value.strip_prefix('"')?.strip_suffix('"')
}

/// The `load_steps` of a header line moved by `delta`, a header without it stays as it is.
fn with_steps(header: &str, delta: i32) -> String {
    let Some(from) = header.find("load_steps=").map(|i| i + "load_steps=".len()) else {
        return header.to_string();
    };
    let len = header[from..].find(|c: char| !c.is_ascii_digit()).unwrap_or(header.len() - from);
    let Ok(steps) = header[from..from + len].parse::<i32>() else {
        return header.to_string();
    };
    format!("{}{}{}", &header[..from], (steps + delta).max(1), &header[from + len..])
}

fn without_uid(header: &str) -> String {
    let Some(from) = header.find(" uid=\"") else {
        return header.to_string();
    };
    let Some(len) = header[from + 6..].find('"') else {
        return header.to_string();
    };
    format!("{}{}", &header[..from], &header[from + 6 + len + 1..])
}

/// The `res://` path of the script a definition's Script Class points at.
pub fn base_script(source: &str) -> Option<String> {
    let (lines, _) = split(source);
    let start = lines.iter().position(|l| l.trim_end() == "[resource]")?;
    let id = lines[start..].iter().find_map(|l| value_of(l, "script_class"))?.strip_prefix("ExtResource(\"")?.strip_suffix("\")")?;
    lines.iter().take(start).filter(|l| is_ext(l)).find(|l| attr(l, "id") == Some(id)).and_then(|l| attr(l, "path")).map(str::to_string)
}

/// The Godot node a definition builds, what the template script extends.
pub fn node_class(source: &str) -> String {
    let (lines, _) = split(source);
    let class = lines.iter().skip_while(|l| l.trim_end() != "[resource]").find_map(|l| value_of(l, "node_class").and_then(quoted)).unwrap_or("");
    if !class.is_empty() {
        class.to_string()
    } else if lines.first().and_then(|h| attr(h, "script_class")).is_some_and(|c| c.contains("Solid")) {
        "StaticBody3D".into()
    } else {
        "Node3D".into()
    }
}

/// The definition at `source`, with the classname `classname`, as the project's own copy: no header `uid`, the Script
/// Class pointing at `res://entities/<classname>.gd`, the override marker in its metadata and for the player the node
/// class `CharacterBody3D`. Every other byte stays, so the Scene File, collision and base classes carry over.
pub fn definition_copy(source: &str, classname: &str, script: Script) -> Result<String, String> {
    let (mut lines, _) = split(source);
    let Some(res) = lines.iter().position(|l| l.trim_end() == "[resource]") else {
        return Err("the definition has no [resource] section".into());
    };
    if !lines.first().is_some_and(|h| h.starts_with("[gd_resource ")) {
        return Err("the definition has no resource header".into());
    }

    let named = format!("classname = \"{classname}\"");
    if !lines[res..].iter().any(|l| l.trim_end() == named) {
        return Err(format!("the definition is not for {classname}"));
    }

    let new_ext = format!("[ext_resource type=\"Script\" path=\"{}\" id=\"{SCRIPT_ID}\"]", script_res(classname));
    let new_ref = format!("script_class = {}", ext_ref(SCRIPT_ID));
    let mut delta = 1;
    let ext_at = lines.iter().take(res).rposition(|l| is_ext(l)).map_or(1, |i| i + 1);
    let old = lines.iter().skip(res).position(|l| value_of(l, "script_class").is_some()).map(|i| i + res);
    match old {
        Some(at) => {
            let old_id =
                value_of(&lines[at], "script_class").and_then(|r| r.strip_prefix("ExtResource(\"")).and_then(|r| r.strip_suffix("\")")).map(str::to_string);
            lines[at] = new_ref;
            let line = old_id.as_ref().and_then(|id| lines.iter().take(res).position(|l| is_ext(l) && attr(l, "id") == Some(id)));
            let shared = old_id.is_some_and(|id| lines.iter().filter(|l| !is_ext(l)).any(|l| l.contains(&ext_ref(&id))));
            match line {
                Some(line) if !shared => {
                    lines[line] = new_ext;
                    delta = 0;
                }
                _ => lines.insert(ext_at, new_ext),
            }
        }
        None => {
            lines.insert(ext_at, new_ext);
            append_property(&mut lines, new_ref);
        }
    }

    let res = lines.iter().position(|l| l.trim_end() == "[resource]").unwrap_or(res);
    if script == Script::Player {
        let class = format!("node_class = \"{PLAYER_NODE_CLASS}\"");
        match lines.iter().skip(res).position(|l| value_of(l, "node_class").is_some()) {
            Some(i) => lines[res + i] = class,
            None => append_property(&mut lines, class),
        }
    }

    let meta = format!("metadata/{META} = true");
    if !lines.iter().skip(res).any(|l| l.trim_end() == meta) {
        append_property(&mut lines, meta);
    }

    lines[0] = with_steps(&without_uid(&lines[0]), delta);
    Ok(join(&lines, true))
}

/// Adds a property line at the end of the `[resource]` section, which is the end of the file.
fn append_property(lines: &mut Vec<String>, line: String) {
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }

    lines.push(line);
}

/// The script an override writes. With `Script::Extend` it extends the original script by path (never by class name, so
/// it needs no `class_name` of its own and collides with nothing) and keeps `@tool` when the original has it, otherwise
/// the Godot editor could not run the original's tool code. A definition without a script gets a template that extends
/// its node class. `base_text` is the text of `base_script(source)`.
pub fn script_for(classname: &str, source: &str, base_text: Option<&str>, script: Script) -> Result<String, String> {
    if script == Script::Player {
        return Ok(PLAYER_SCRIPT.to_string());
    }

    let Some(base) = base_script(source) else {
        return Ok(format!("extends {}\n## {classname} for this project, add your own code below.\n", node_class(source)));
    };
    let Some(text) = base_text else {
        return Err(format!("the script {base} is missing"));
    };
    let tool = text.lines().map(str::trim).find(|l| !l.is_empty()) == Some("@tool");
    let mut out = String::new();
    if tool {
        out += "@tool\n";
    }

    out += &format!("extends \"{base}\"\n## {classname} for this project. It runs the original script, add your own code below.\n");
    Ok(out)
}

/// A project FGD that builds on the addon's default FGD and holds the override of `classname`.
pub fn new_fgd(classname: &str) -> String {
    format!(
        "[gd_resource type=\"Resource\" script_class=\"FuncGodotFGDFile\" load_steps=4 format=3]\n\n\
         [ext_resource type=\"Script\" path=\"{FGD_FILE_SCRIPT}\" id=\"1_fgd\"]\n\
         [ext_resource type=\"Resource\" path=\"{}\" id=\"2_base\"]\n\
         [ext_resource type=\"Resource\" path=\"{}\" id=\"{}\"]\n\n\
         [resource]\n\
         script = ExtResource(\"1_fgd\")\n\
         base_fgd_files = Array[Resource]([ExtResource(\"2_base\")])\n\
         entity_definitions = Array[Resource]([{}])\n",
        entity_pack::DEFAULT_FGD,
        definition_res(classname),
        entity_id(classname, &[]),
        ext_ref(&entity_id(classname, &[])),
    )
}

/// Map settings that build with the FGD at `fgd`, everything else as the addon's default settings.
pub fn new_map_settings(fgd: &str) -> String {
    format!(
        "[gd_resource type=\"Resource\" script_class=\"FuncGodotMapSettings\" load_steps=3 format=3]\n\n\
         [ext_resource type=\"Script\" path=\"{MAP_SETTINGS_SCRIPT}\" id=\"1_ms\"]\n\
         [ext_resource type=\"Resource\" path=\"{fgd}\" id=\"2_fgd\"]\n\n\
         [resource]\n\
         script = ExtResource(\"1_ms\")\n\
         entity_fgd = ExtResource(\"2_fgd\")\n"
    )
}

/// A game config that uses the FGD at `fgd` and the map settings at `map_settings`. Its name and output path stay the
/// defaults, so the editor keeps finding `res://trenchdot_game.json`.
pub fn new_game_config(fgd: &str, map_settings: &str) -> String {
    format!(
        "[gd_resource type=\"Resource\" script_class=\"TrenchDotGameConfig\" load_steps=4 format=3]\n\n\
         [ext_resource type=\"Script\" path=\"{GAME_CONFIG_SCRIPT}\" id=\"1_td\"]\n\
         [ext_resource type=\"Resource\" path=\"{fgd}\" id=\"2_fgd\"]\n\
         [ext_resource type=\"Resource\" path=\"{map_settings}\" id=\"3_ms\"]\n\n\
         [resource]\n\
         script = ExtResource(\"1_td\")\n\
         fgd_file = ExtResource(\"2_fgd\")\n\
         map_settings = ExtResource(\"3_ms\")\n"
    )
}

/// `td_<classname>`, with a number after it when `taken` has it already.
fn entity_id(classname: &str, taken: &[&str]) -> String {
    let base = format!("td_{classname}");
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id.as_str()) {
        id = format!("{base}_{n}");
        n += 1;
    }

    id
}

/// The project FGD at `fgd` with the override of `classname` added to its Entity Definitions. Only the plain form
/// Godot writes is edited, anything else is an error and the caller tells the person to add the definition by hand.
pub fn add_to_fgd(fgd: &str, classname: &str) -> Result<String, String> {
    let crlf = fgd.contains("\r\n");
    let text = fgd.replace("\r\n", "\n");
    let (mut lines, ends) = split(&text);
    let Some(res) = lines.iter().position(|l| l.trim_end() == "[resource]") else {
        return Err("the FGD has no [resource] section".into());
    };
    if !lines.first().is_some_and(|h| h.starts_with("[gd_resource ")) {
        return Err("the FGD has no resource header".into());
    }

    let path = definition_res(classname);
    if lines.iter().take(res).any(|l| is_ext(l) && attr(l, "path") == Some(&path)) {
        return Err(format!("the FGD lists {path} already"));
    }

    let taken: Vec<&str> = lines.iter().take(res).filter(|l| is_ext(l)).filter_map(|l| attr(l, "id")).collect();
    let id = entity_id(classname, &taken);
    let item = ext_ref(&id);
    let ext_line = format!("[ext_resource type=\"Resource\" path=\"{path}\" id=\"{id}\"]");

    let defs = lines.iter().skip(res).position(|l| l.starts_with("entity_definitions")).map(|i| i + res);
    match defs {
        Some(at) => {
            let line = lines[at].trim_end();
            let inner = line
                .strip_prefix("entity_definitions = Array[Resource]([")
                .and_then(|r| r.strip_suffix("])"))
                .ok_or("its entity_definitions line is not the plain Godot form")?;
            let plain = inner.is_empty()
                || inner.split(", ").all(|i| {
                    i.strip_prefix("ExtResource(\"")
                        .or_else(|| i.strip_prefix("SubResource(\""))
                        .is_some_and(|r| r.strip_suffix("\")").is_some_and(|id| !id.contains('"')))
                });
            if !plain {
                return Err("its entity_definitions line is not the plain Godot form".into());
            }

            let list = if inner.is_empty() { item.clone() } else { format!("{inner}, {item}") };
            lines[at] = format!("entity_definitions = Array[Resource]([{list}])");
        }
        None => append_property(&mut lines, format!("entity_definitions = Array[Resource]([{item}])")),
    }

    let at = lines.iter().take(res).rposition(|l| is_ext(l)).map_or(1, |i| i + 1);
    lines.insert(at, ext_line);
    lines[0] = with_steps(&lines[0], 1);
    let out = join(&lines, ends);
    Ok(if crlf { out.replace('\n', "\r\n") } else { out })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUTTON: &str = include_str!("../../../godot/addons/func_godot/gameplay_pack/definitions/func_button.tres");
    const LIGHT: &str = include_str!("../../../godot/addons/func_godot/fgd/trenchdot/light.tres");
    const START: &str = include_str!("../../../godot/addons/func_godot/fgd/trenchdot/info_player_start.tres");
    const DETAIL: &str = include_str!("../../../godot/addons/func_godot/fgd/func_detail.tres");
    const TD_BUTTON: &str = include_str!("../../../godot/addons/func_godot/gameplay_pack/scripts/td_button.gd");
    const STREAMER: &str = include_str!("../../../godot/addons/func_godot/gameplay_pack/scripts/td_section_streamer.gd");
    const PACK_FGD: &str = include_str!("../../../godot/addons/func_godot/gameplay_pack/gameplay_fgd.tres");
    const CORE_FGD: &str = include_str!("../../../godot/addons/func_godot/fgd/trenchdot/trenchdot_fgd.tres");
    const DEFAULT_MAP_SETTINGS: &str = include_str!("../../../godot/addons/func_godot/func_godot_default_map_settings.tres");
    const DEFAULT_GAME_CONFIG: &str = include_str!("../../../godot/addons/func_godot/game_config/trenchdot/trenchdot_game_config.tres");

    fn lines(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    #[test]
    fn a_definition_with_a_script_keeps_everything_but_the_uid_and_the_script() {
        let copy = definition_copy(BUTTON, "func_button", Script::Extend).unwrap();
        let mut want = BUTTON
            .replace("path=\"res://trenchdot/entities/scripts/td_button.gd\" id=\"2_script\"", "path=\"res://entities/func_button.gd\" id=\"td_override\"");
        want = want.replace("script_class = ExtResource(\"2_script\")", "script_class = ExtResource(\"td_override\")");
        want.push_str("metadata/trenchdot_override = true\n");
        assert_eq!(copy, want);
        assert_eq!(lines(&copy)[0], lines(BUTTON)[0], "no uid in this header, same load_steps as the script line was replaced");
    }

    #[test]
    fn the_header_uid_goes_and_the_uids_of_the_resources_stay() {
        let copy = definition_copy(DETAIL, "func_detail", Script::Extend).unwrap();
        assert!(DETAIL.lines().next().unwrap().contains("uid=\"uid://cxy7jnh6d7msn\""));
        assert_eq!(lines(&copy)[0], "[gd_resource type=\"Resource\" script_class=\"FuncGodotFGDSolidClass\" load_steps=6 format=3]");
        for uid in ["uid://5cow84q03m6a", "uid://nayxb8n7see2", "uid://doo4ly322b4jc", "uid://bcdsueg5pysfq"] {
            assert!(copy.contains(uid), "{uid}");
        }

        assert!(!copy.contains("uid://cxy7jnh6d7msn"));
    }

    #[test]
    fn a_definition_without_a_script_gets_one_appended_and_the_header_counts_it() {
        let copy = definition_copy(DETAIL, "func_detail", Script::Extend).unwrap();
        let want = [
            "[gd_resource type=\"Resource\" script_class=\"FuncGodotFGDSolidClass\" load_steps=6 format=3]",
            "",
            "[ext_resource type=\"Script\" uid=\"uid://5cow84q03m6a\" path=\"res://addons/func_godot/src/fgd/func_godot_fgd_solid_class.gd\" id=\"1_0fsmp\"]",
            "[ext_resource type=\"Resource\" uid=\"uid://nayxb8n7see2\" path=\"res://addons/func_godot/fgd/phong_base.tres\" id=\"1_c3bns\"]",
            "[ext_resource type=\"Resource\" uid=\"uid://doo4ly322b4jc\" path=\"res://addons/func_godot/fgd/vertex_merge_distance_base.tres\" id=\"2_c03gr\"]",
            "[ext_resource type=\"Resource\" uid=\"uid://bcdsueg5pysfq\" path=\"res://addons/func_godot/fgd/cull_interior_faces.tres\" id=\"3_wuxhx\"]",
            "[ext_resource type=\"Script\" path=\"res://entities/func_detail.gd\" id=\"td_override\"]",
            "",
            "[resource]",
        ];
        assert_eq!(&lines(&copy)[..want.len()], &want);
        let tail = &lines(&copy)[want.len()..];
        assert_eq!(tail[0], "script = ExtResource(\"1_0fsmp\")");
        assert_eq!(
            &tail[tail.len() - 3..],
            ["node_class = \"StaticBody3D\"", "script_class = ExtResource(\"td_override\")", "metadata/trenchdot_override = true"]
        );
        assert_eq!(copy.matches("script_class = ").count(), 1);
    }

    #[test]
    fn a_core_script_is_replaced_in_place_and_the_other_lines_are_untouched() {
        let copy = definition_copy(LIGHT, "light", Script::Extend).unwrap();
        let mut want = LIGHT.replace(
            "path=\"res://addons/func_godot/src/trenchdot/entities/td_light.gd\" id=\"2_script\"",
            "path=\"res://entities/light.gd\" id=\"td_override\"",
        );
        want = want.replace("script_class = ExtResource(\"2_script\")", "script_class = ExtResource(\"td_override\")");
        want.push_str("metadata/trenchdot_override = true\n");
        assert_eq!(copy, want);
        assert!(!copy.contains("td_light.gd"));
    }

    #[test]
    fn only_the_player_changes_the_node_class() {
        let start = definition_copy(START, "info_player_start", Script::Player).unwrap();
        assert!(start.contains("node_class = \"CharacterBody3D\"\n") && !start.contains("Marker3D"), "{start}");
        assert_eq!(start.matches("node_class = ").count(), 1);
        let template = definition_copy(START, "info_player_start", Script::Extend).unwrap();
        assert!(template.contains("node_class = \"Marker3D\"\n"), "{template}");

        let light = definition_copy(LIGHT, "light", Script::Player).unwrap();
        assert!(light.contains("node_class = \"CharacterBody3D\"\n") && !light.contains("OmniLight3D"));
    }

    #[test]
    fn a_player_start_copy_is_whole() {
        let copy = definition_copy(START, "info_player_start", Script::Player).unwrap();
        let want = "\
[gd_resource type=\"Resource\" script_class=\"FuncGodotFGDPointClass\" load_steps=3 format=3]

[ext_resource type=\"Script\" path=\"res://addons/func_godot/src/fgd/func_godot_fgd_point_class.gd\" id=\"1_class\"]
[ext_resource type=\"Script\" path=\"res://entities/info_player_start.gd\" id=\"td_override\"]

[resource]
script = ExtResource(\"1_class\")
classname = \"info_player_start\"
description = \"Player spawn point\"
class_properties = Dictionary[String, Variant]({

})
class_property_descriptions = Dictionary[String, Variant]({

})
meta_properties = Dictionary[String, Variant]({
\"color\": Color(0.2, 1, 0.2, 1),
\"size\": AABB(-16, -16, 0, 16, 16, 56)
})
node_class = \"CharacterBody3D\"
script_class = ExtResource(\"td_override\")
metadata/trenchdot_override = true
";
        assert_eq!(copy, want);
    }

    #[test]
    fn the_marker_is_added_once_and_a_copy_of_a_copy_is_the_same() {
        let once = definition_copy(BUTTON, "func_button", Script::Extend).unwrap();
        assert_eq!(once.matches("metadata/trenchdot_override").count(), 1);
        let marked = format!("{BUTTON}metadata/trenchdot_override = true\n");
        let copy = definition_copy(&marked, "func_button", Script::Extend).unwrap();
        assert_eq!(copy, once);
    }

    #[test]
    fn a_definition_the_copy_cannot_trust_is_refused() {
        assert!(definition_copy(BUTTON, "func_door", Script::Extend).unwrap_err().contains("not for func_door"));
        assert!(definition_copy("[gd_resource type=\"Resource\"]\n", "x", Script::Extend).unwrap_err().contains("[resource]"));
        assert!(definition_copy("classname = \"x\"\n[resource]\n", "x", Script::Extend).unwrap_err().contains("header"));
        assert!(definition_copy(BUTTON, "func_butto", Script::Extend).is_err(), "a prefix of the classname is another class");
    }

    #[test]
    fn a_script_id_shared_with_another_line_is_kept() {
        let source = "[gd_resource type=\"Resource\" load_steps=3 format=3]\n\n[ext_resource type=\"Script\" path=\"res://a.gd\" id=\"1\"]\n\n[resource]\nclassname = \"x\"\nscript_class = ExtResource(\"1\")\nother = ExtResource(\"1\")\n";
        let copy = definition_copy(source, "x", Script::Extend).unwrap();
        assert!(copy.contains("path=\"res://a.gd\" id=\"1\"") && copy.contains("path=\"res://entities/x.gd\" id=\"td_override\""), "{copy}");
        assert_eq!(lines(&copy)[0], "[gd_resource type=\"Resource\" load_steps=4 format=3]");
    }

    #[test]
    fn the_base_script_and_the_node_class_come_from_the_definition() {
        assert_eq!(base_script(BUTTON).as_deref(), Some("res://trenchdot/entities/scripts/td_button.gd"));
        assert_eq!(base_script(LIGHT).as_deref(), Some("res://addons/func_godot/src/trenchdot/entities/td_light.gd"));
        assert_eq!(base_script(START), None);
        assert_eq!(base_script(DETAIL), None);
        assert_eq!(node_class(BUTTON), "AnimatableBody3D");
        assert_eq!(node_class(START), "Marker3D");
        assert_eq!(node_class(DETAIL), "StaticBody3D");
        assert_eq!(node_class("[gd_resource script_class=\"FuncGodotFGDSolidClass\"]\n[resource]\n"), "StaticBody3D");
        assert_eq!(node_class("[gd_resource script_class=\"FuncGodotFGDPointClass\"]\n[resource]\nnode_class = \"\"\n"), "Node3D");
    }

    #[test]
    fn a_script_extends_the_original_by_path_and_keeps_tool() {
        let script = script_for("func_button", BUTTON, Some(TD_BUTTON), Script::Extend).unwrap();
        let want = "@tool\nextends \"res://trenchdot/entities/scripts/td_button.gd\"\n## func_button for this project. It runs the original script, add your own code below.\n";
        assert_eq!(script, want);
        assert!(!script.contains("class_name"));
    }

    #[test]
    fn a_base_without_tool_gets_no_tool() {
        assert!(!STREAMER.trim_start().starts_with("@tool"));
        let source = BUTTON.replace("td_button.gd", "td_section_streamer.gd");
        let script = script_for("func_button", &source, Some(STREAMER), Script::Extend).unwrap();
        assert_eq!(
            script,
            "extends \"res://trenchdot/entities/scripts/td_section_streamer.gd\"\n## func_button for this project. It runs the original script, add your own code below.\n"
        );
        let late = "# comment\n@tool\nextends Node\n";
        assert!(!script_for("func_button", BUTTON, Some(late), Script::Extend).unwrap().starts_with("@tool"));
        let blank = "\n\n  @tool  \nextends Node\n";
        assert!(script_for("func_button", BUTTON, Some(blank), Script::Extend).unwrap().starts_with("@tool\n"));
    }

    #[test]
    fn a_definition_without_a_script_gets_a_template_of_its_node_class() {
        let start = script_for("info_player_start", START, None, Script::Extend).unwrap();
        assert_eq!(start, "extends Marker3D\n## info_player_start for this project, add your own code below.\n");
        let detail = script_for("func_detail", DETAIL, None, Script::Extend).unwrap();
        assert_eq!(detail, "extends StaticBody3D\n## func_detail for this project, add your own code below.\n");
    }

    #[test]
    fn the_player_script_is_the_embedded_file_and_a_missing_base_is_an_error() {
        assert_eq!(script_for("info_player_start", START, None, Script::Player).unwrap(), PLAYER_SCRIPT);
        assert!(PLAYER_SCRIPT.starts_with("extends CharacterBody3D"));
        assert!(script_for("func_button", BUTTON, None, Script::Extend).unwrap_err().contains("td_button.gd is missing"));
    }

    #[test]
    fn a_new_fgd_builds_on_the_addon_fgd_and_lists_the_override() {
        let want = "\
[gd_resource type=\"Resource\" script_class=\"FuncGodotFGDFile\" load_steps=4 format=3]

[ext_resource type=\"Script\" path=\"res://addons/func_godot/src/fgd/func_godot_fgd_file.gd\" id=\"1_fgd\"]
[ext_resource type=\"Resource\" path=\"res://addons/func_godot/fgd/trenchdot_default_fgd.tres\" id=\"2_base\"]
[ext_resource type=\"Resource\" path=\"res://entities/func_door.tres\" id=\"td_func_door\"]

[resource]
script = ExtResource(\"1_fgd\")
base_fgd_files = Array[Resource]([ExtResource(\"2_base\")])
entity_definitions = Array[Resource]([ExtResource(\"td_func_door\")])
";
        assert_eq!(new_fgd("func_door"), want);
        assert_eq!(definition_res("func_door"), "res://entities/func_door.tres");
        assert_eq!(fgd_res(), "res://entities/project_fgd.tres");
    }

    #[test]
    fn new_settings_and_config_read_like_the_addon_defaults_with_the_project_fgd() {
        let settings = new_map_settings("res://entities/project_fgd.tres");
        let want = "\
[gd_resource type=\"Resource\" script_class=\"FuncGodotMapSettings\" load_steps=3 format=3]

[ext_resource type=\"Script\" path=\"res://addons/func_godot/src/map/func_godot_map_settings.gd\" id=\"1_ms\"]
[ext_resource type=\"Resource\" path=\"res://entities/project_fgd.tres\" id=\"2_fgd\"]

[resource]
script = ExtResource(\"1_ms\")
entity_fgd = ExtResource(\"2_fgd\")
";
        assert_eq!(settings, want);
        assert!(DEFAULT_MAP_SETTINGS.contains("path=\"res://addons/func_godot/src/map/func_godot_map_settings.gd\""));
        assert!(DEFAULT_MAP_SETTINGS.lines().last().unwrap().starts_with("entity_fgd = "));

        let config = new_game_config("res://entities/project_fgd.tres", "res://entities/project_map_settings.tres");
        let want = "\
[gd_resource type=\"Resource\" script_class=\"TrenchDotGameConfig\" load_steps=4 format=3]

[ext_resource type=\"Script\" path=\"res://addons/func_godot/src/trenchdot/trenchdot_game_config.gd\" id=\"1_td\"]
[ext_resource type=\"Resource\" path=\"res://entities/project_fgd.tres\" id=\"2_fgd\"]
[ext_resource type=\"Resource\" path=\"res://entities/project_map_settings.tres\" id=\"3_ms\"]

[resource]
script = ExtResource(\"1_td\")
fgd_file = ExtResource(\"2_fgd\")
map_settings = ExtResource(\"3_ms\")
";
        assert_eq!(config, want);
        let default_props: Vec<&str> = DEFAULT_GAME_CONFIG.lines().skip_while(|l| *l != "[resource]").skip(1).map(|l| l.split(" = ").next().unwrap()).collect();
        let new_props: Vec<&str> = config.lines().skip_while(|l| *l != "[resource]").skip(1).map(|l| l.split(" = ").next().unwrap()).collect();
        assert_eq!(new_props, default_props, "only the properties the addon default sets");
    }

    #[test]
    fn an_entry_goes_into_an_empty_array_one_entry_and_many() {
        let empty = new_fgd("a").replace("[ExtResource(\"td_a\")]", "[]");
        let one = add_to_fgd(&empty, "func_door").unwrap();
        assert!(one.contains("entity_definitions = Array[Resource]([ExtResource(\"td_func_door\")])\n"), "{one}");
        assert!(one.contains("[ext_resource type=\"Resource\" path=\"res://entities/func_door.tres\" id=\"td_func_door\"]\n\n[resource]"), "{one}");
        assert_eq!(lines(&one)[0], "[gd_resource type=\"Resource\" script_class=\"FuncGodotFGDFile\" load_steps=5 format=3]");

        let two = add_to_fgd(&new_fgd("func_door"), "light").unwrap();
        assert!(two.contains("entity_definitions = Array[Resource]([ExtResource(\"td_func_door\"), ExtResource(\"td_light\")])\n"), "{two}");
        assert_eq!(two.matches("[ext_resource").count(), 4);

        let three = add_to_fgd(&two, "func_button").unwrap();
        assert!(three.contains("([ExtResource(\"td_func_door\"), ExtResource(\"td_light\"), ExtResource(\"td_func_button\")])\n"), "{three}");
        assert_eq!(three.matches("[ext_resource").count(), 5);
        assert_eq!(lines(&three)[0], "[gd_resource type=\"Resource\" script_class=\"FuncGodotFGDFile\" load_steps=6 format=3]");
        let tail: Vec<&str> = lines(&three).into_iter().rev().take(3).collect();
        assert_eq!(tail[1], "base_fgd_files = Array[Resource]([ExtResource(\"2_base\")])", "{three}");
    }

    #[test]
    fn a_real_fgd_gets_the_entry_after_its_last_resource_and_keeps_every_other_line() {
        let out = add_to_fgd(CORE_FGD, "func_button").unwrap();
        let src: Vec<&str> = lines(CORE_FGD);
        let got: Vec<&str> = lines(&out);
        assert_eq!(got.len(), src.len() + 1);
        assert_eq!(got[9], "[ext_resource type=\"Resource\" path=\"res://entities/func_button.tres\" id=\"td_func_button\"]");
        assert_eq!(got[0], "[gd_resource type=\"Resource\" script_class=\"TrenchDotFGDFile\" load_steps=9 format=3]");
        assert_eq!(got[1..9], src[1..9]);
        assert_eq!(got[10..got.len() - 1], src[9..src.len() - 1]);
        assert_eq!(
            got[got.len() - 1],
            src[src.len() - 1].replace("ExtResource(\"7_env_voxel_gi\")]", "ExtResource(\"7_env_voxel_gi\"), ExtResource(\"td_func_button\")]")
        );

        let pack = add_to_fgd(PACK_FGD, "light").unwrap();
        assert_eq!(pack.matches("[ext_resource").count(), 46);
        assert!(pack.starts_with("[gd_resource type=\"Resource\" script_class=\"FuncGodotFGDFile\" load_steps=47 format=3]"));
    }

    #[test]
    fn a_missing_entity_definitions_line_is_added_and_an_id_clash_gets_a_number() {
        let bare = "[gd_resource type=\"Resource\" script_class=\"FuncGodotFGDFile\" load_steps=2 format=3]\n\n[ext_resource type=\"Script\" path=\"res://f.gd\" id=\"td_light\"]\n\n[resource]\nscript = ExtResource(\"td_light\")\n";
        let out = add_to_fgd(bare, "light").unwrap();
        assert_eq!(
            out,
            "[gd_resource type=\"Resource\" script_class=\"FuncGodotFGDFile\" load_steps=3 format=3]\n\n[ext_resource type=\"Script\" path=\"res://f.gd\" id=\"td_light\"]\n[ext_resource type=\"Resource\" path=\"res://entities/light.tres\" id=\"td_light_2\"]\n\n[resource]\nscript = ExtResource(\"td_light\")\nentity_definitions = Array[Resource]([ExtResource(\"td_light_2\")])\n"
        );
        let twice = add_to_fgd(&out.replace("res://entities/light.tres", "res://entities/other.tres"), "light").unwrap();
        assert!(twice.contains("id=\"td_light_3\"") && twice.contains("ExtResource(\"td_light_2\"), ExtResource(\"td_light_3\")"), "{twice}");
    }

    #[test]
    fn an_fgd_it_cannot_edit_is_refused_and_so_is_a_double_entry() {
        let fgd = new_fgd("func_door");
        assert!(add_to_fgd(&fgd, "func_door").unwrap_err().contains("lists res://entities/func_door.tres already"));
        let spread = fgd.replace("Array[Resource]([ExtResource(\"td_func_door\")])", "Array[Resource]([\nExtResource(\"td_func_door\")\n])");
        assert!(add_to_fgd(&spread, "light").unwrap_err().contains("plain Godot form"));
        let typed = fgd.replace("Array[Resource]([ExtResource(\"td_func_door\")])", "Array[Resource](ExtResource(\"td_func_door\"))");
        assert!(add_to_fgd(&typed, "light").is_err());
        let odd = fgd.replace("ExtResource(\"td_func_door\")])", "Foo(1)])");
        assert!(add_to_fgd(&odd, "light").is_err());
        assert!(add_to_fgd("\u{0}\u{0}binary", "light").is_err());
        assert!(add_to_fgd("[gd_resource type=\"Resource\"]\n", "light").unwrap_err().contains("[resource]"));
        let sub = fgd.replace("([ExtResource(\"td_func_door\")])\n", "([SubResource(\"1\"), ExtResource(\"td_func_door\")])\n");
        assert!(add_to_fgd(&sub, "light").unwrap().contains("SubResource(\"1\"), ExtResource(\"td_func_door\"), ExtResource(\"td_light\")"));
    }

    #[test]
    fn a_crlf_fgd_stays_crlf() {
        let fgd = new_fgd("func_door").replace('\n', "\r\n");
        let out = add_to_fgd(&fgd, "light").unwrap();
        assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
        assert_eq!(out.replace("\r\n", "\n"), add_to_fgd(&new_fgd("func_door"), "light").unwrap());
        let no_end = new_fgd("func_door").trim_end().to_string();
        assert!(!add_to_fgd(&no_end, "light").unwrap().ends_with('\n'));
    }
}
