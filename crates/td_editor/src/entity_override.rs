//! Override in project, the text of the files it writes. An override is the project's own copy of an addon or pack
//! entity definition with the same classname, whose Script Class is a script of the project that extends the original.
//! The copy is made as text, so everything the definition holds (scene file, collision, base classes) stays as it was.

use std::path::{Path, PathBuf};

use td_formats::GameConfig;

use crate::i18n::Msg;
use crate::{code_refs, entity_pack, godot_settings, msg};

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
    definition_res_in(RES_DIR, classname)
}

pub fn definition_res_in(res_dir: &str, classname: &str) -> String {
    format!("{res_dir}/{classname}.tres")
}

pub fn script_res(classname: &str) -> String {
    script_res_in(RES_DIR, classname)
}

pub fn script_res_in(res_dir: &str, classname: &str) -> String {
    format!("{res_dir}/{classname}.gd")
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
    definition_copy_in(RES_DIR, source, classname, script)
}

/// [`definition_copy`] for a project folder other than `res://entities`.
pub fn definition_copy_in(res_dir: &str, source: &str, classname: &str, script: Script) -> Result<String, String> {
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

    let new_ext = format!("[ext_resource type=\"Script\" path=\"{}\" id=\"{SCRIPT_ID}\"]", script_res_in(res_dir, classname));
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
    new_fgd_in(RES_DIR, classname)
}

/// [`new_fgd`] for a project folder other than `res://entities`.
pub fn new_fgd_in(res_dir: &str, classname: &str) -> String {
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
        definition_res_in(res_dir, classname),
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
    add_to_fgd_in(RES_DIR, fgd, classname)
}

/// [`add_to_fgd`] for a project folder other than `res://entities`.
pub fn add_to_fgd_in(res_dir: &str, fgd: &str, classname: &str) -> Result<String, String> {
    let crlf = fgd.contains("\r\n");
    let text = fgd.replace("\r\n", "\n");
    let (mut lines, ends) = split(&text);
    let Some(res) = lines.iter().position(|l| l.trim_end() == "[resource]") else {
        return Err("the FGD has no [resource] section".into());
    };
    if !lines.first().is_some_and(|h| h.starts_with("[gd_resource ")) {
        return Err("the FGD has no resource header".into());
    }

    let path = definition_res_in(res_dir, classname);
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

/// What `plan` found in the project: the files to write, the project settings to set and where the FGD is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub classname: String,
    /// `res://` paths of the override's definition, script and the FGD that lists it.
    pub definition: String,
    pub script: String,
    pub fgd: String,
    /// In the order they are written. A new file never replaces one, an edit does.
    pub writes: Vec<Write>,
    /// `project.godot` settings as (section, key, value), only the ones that change.
    pub settings: Vec<(&'static str, &'static str, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Write {
    pub path: PathBuf,
    pub text: String,
    pub replace: bool,
}

const ADDON_RES: &str = "res://addons/";
const MAP_SETTINGS_KEY: (&str, &str) = ("func_godot", "default_map_settings");
const GAME_CONFIG_KEY: (&str, &str) = ("trenchdot", "game_config");
const LEGACY_GAME_CONFIG_KEY: (&str, &str) = ("godottrench", "game_config");
/// Definitions FuncGodot ships itself, next to the TrenchDot ones in the addon's `fgd` folder.
const FUNC_GODOT_ENTITIES: [&str; 5] = ["worldspawn", "func_geo", "func_detail", "func_detail_illusionary", "func_illusionary"];

/// The text of `path` with LF line ends, `shown` is how a refusal names it.
fn read(path: &Path, shown: &str) -> Result<String, Msg> {
    std::fs::read_to_string(path).map(|t| t.replace("\r\n", "\n")).map_err(|e| msg!("{path} cannot be read: {error}", path = shown, error = e))
}

fn not_a_file(path: &str) -> Msg {
    msg!("The project names {path}, which is not a file inside it", path)
}

fn project_path(root: &Path, res: &str) -> Result<PathBuf, Msg> {
    let rel = res.strip_prefix("res://").filter(|r| !r.is_empty() && !r.split(['/', '\\']).any(|p| p == "..")).ok_or_else(|| not_a_file(res))?;
    Ok(root.join(rel))
}

/// The definition `classname` is copied from: the installed pack's, then the addon's own, then FuncGodot's.
pub fn source(root: &Path, classname: &str) -> Result<(PathBuf, String), Msg> {
    let pack = format!("{}/definitions/{classname}.tres", entity_pack::DIR);
    let found = if entity_pack::in_pack(classname) && root.join(&pack).is_file() {
        Some(pack)
    } else if code_refs::CORE_ENTITIES.contains(&classname) {
        Some(format!("{}/fgd/trenchdot/{classname}.tres", crate::addon_install::DIR))
    } else if FUNC_GODOT_ENTITIES.contains(&classname) {
        Some(format!("{}/fgd/{classname}.tres", crate::addon_install::DIR))
    } else {
        None
    };
    match found {
        Some(rel) if root.join(&rel).is_file() => Ok((root.join(&rel), format!("res://{rel}"))),
        Some(rel) => Err(msg!("The addon in this project has no definition at {path}, update the addon", path = format!("res://{rel}"))),
        None if entity_pack::in_pack(classname) && entity_pack::installed(root) => Err(msg!(
            "The Gameplay entities pack in this project has no {path}, so there is no definition of {classname} to copy",
            path = format!("{}/definitions/{classname}.tres", entity_pack::RES_DIR),
            classname
        )),
        None if entity_pack::in_pack(classname) => {
            Err(msg!("{classname} comes with the Gameplay entities pack. Install it with Gameplay > Install Gameplay Entities, then override it", classname))
        }
        None => Err(msg!("Override in project has no definition to copy for {classname}, it copies core, FuncGodot and Gameplay entities", classname)),
    }
}

/// The FGD a game config or map settings resource of the project builds with, None for the addon's default. `prop` is
/// the property that holds it, `fgd_file` or `entity_fgd`.
fn fgd_property(text: &str, prop: &str) -> Result<Option<String>, String> {
    let (lines, _) = split(text);
    let start = lines.iter().position(|l| l.trim_end() == "[resource]").ok_or("it has no [resource] section")?;
    let Some(value) = lines[start..].iter().find_map(|l| value_of(l, prop)) else {
        return Ok(None);
    };
    if value == "null" {
        return Ok(None);
    }

    let id = value.strip_prefix("ExtResource(\"").and_then(|r| r.strip_suffix("\")")).ok_or("its FGD is not a file of the project")?;
    lines[..start]
        .iter()
        .filter(|l| is_ext(l))
        .find(|l| attr(l, "id") == Some(id))
        .and_then(|l| attr(l, "path"))
        .map(|p| Some(p.to_string()))
        .ok_or_else(|| format!("its FGD {id} has no path"))
}

/// The resource a project setting names when it is the project's own: None for an unset setting and for the addon's
/// defaults. A uid that no `.tres` of the project carries refuses, it may well be the project's own binary resource.
fn own_setting(root: &Path, project: &str, keys: &[(&str, &str)]) -> Result<Option<String>, Msg> {
    let found = keys.iter().find_map(|(section, key)| {
        godot_settings::get(project, section, key).filter(|v| !v.trim().is_empty()).map(|v| (format!("{section}/{key}"), v.trim().to_string()))
    });
    let Some((setting, value)) = found else {
        return Ok(None);
    };
    match godot_settings::resolve(root, &value).map_err(|_| not_a_file(&value))? {
        Some(res) => Ok(Some(res).filter(|r| !r.starts_with(ADDON_RES))),
        None => Err(msg!("Cannot find {uid} of {setting} in the project. Set {setting} to its res:// path, then try again", uid = value, setting)),
    }
}

/// The project FGD that the project's own resource `res` holds in `prop`. Err when it builds with the addon's FGD,
/// `addon` is the refusal for that.
fn own_fgd_of(root: &Path, res: &str, prop: &str, addon: fn(&str) -> Msg) -> Result<String, Msg> {
    let text = read(&project_path(root, res)?, res)?;
    match fgd_property(&text, prop) {
        Ok(Some(fgd)) if !fgd.starts_with(ADDON_RES) => Ok(fgd),
        Ok(_) => Err(addon(res)),
        Err(e) => Err(msg!("{path} cannot be read: {error}", path = res, error = e)),
    }
}

fn game_config_has_addon_fgd(path: &str) -> Msg {
    msg!("Your game config {path} builds with the addon's FGD. Point its FGD File at an FGD of your own, then try again", path)
}

fn map_settings_have_addon_fgd(path: &str) -> Msg {
    msg!("Your map settings {path} build with the addon's FGD. Point their Entity Fgd at an FGD of your own, then try again", path)
}

fn defined_by(classname: &str, path: &str) -> Msg {
    msg!("{classname} is defined by {path} in this project already, edit that definition instead", classname, path)
}

/// The definition at `res` is an override TrenchDot wrote.
pub fn is_override(root: &Path, res: &str) -> bool {
    let meta = format!("metadata/{META} = true");
    project_path(root, res).ok().and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|t| t.lines().any(|l| l.trim_end() == meta))
}

fn has_scene_file(source: &str) -> bool {
    source.lines().skip_while(|l| l.trim_end() != "[resource]").any(|l| value_of(l, "scene_file").is_some_and(|v| v != "null"))
}

/// The project's own definition of `classname` that the project FGD `fgd` lists, which no clash reports when the FGD
/// does not build on the addon's.
fn own_definition(root: &Path, fgd: &str, classname: &str) -> Option<String> {
    let named = format!("classname = \"{classname}\"");
    let pack = format!("{}/", entity_pack::RES_DIR);
    fgd.lines()
        .filter(|l| is_ext(l))
        .filter_map(|l| attr(l, "path"))
        .filter(|p| p.ends_with(".tres") && !p.starts_with(ADDON_RES) && !p.starts_with(&pack))
        .find(|p| project_path(root, p).ok().and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|t| t.lines().any(|l| l.trim_end() == named)))
        .map(str::to_string)
}

/// The project's own resources, None where a setting is unset or the addon's default.
struct Wiring {
    config: Option<String>,
    settings: Option<String>,
    fgd: Option<String>,
}

/// The own game config, map settings and FGD of the project, read from `project.godot`. Refuses when the two name
/// different FGDs or when one of them builds with the addon's FGD, which the person has to change in the resource.
fn own_wiring(root: &Path, project: &str) -> Result<Wiring, Msg> {
    let config = own_setting(root, project, &[GAME_CONFIG_KEY, LEGACY_GAME_CONFIG_KEY])?;
    let settings = own_setting(root, project, &[MAP_SETTINGS_KEY])?;
    let config_fgd = config.as_deref().map(|r| own_fgd_of(root, r, "fgd_file", game_config_has_addon_fgd)).transpose()?;
    let settings_fgd = settings.as_deref().map(|r| own_fgd_of(root, r, "entity_fgd", map_settings_have_addon_fgd)).transpose()?;
    let fgd = match (config_fgd, settings_fgd) {
        (Some(a), Some(b)) if a != b => {
            return Err(msg!("Your game config and your map settings use two FGDs, {a} and {b}. Point both at one of them, then try again", a, b));
        }
        (a, b) => a.or(b),
    };
    Ok(Wiring { config, settings, fgd })
}

/// The project FGD `project.godot` leads to, None when the project has none yet.
pub fn own_fgd(root: &Path) -> Result<Option<String>, Msg> {
    let project = read(&root.join("project.godot"), "project.godot")?;
    own_wiring(root, &project).map(|w| w.fgd)
}

/// Everything Override in project would do for `classname`, or why it will not. Reads the project and writes nothing.
pub fn plan(root: &Path, game: &GameConfig, classname: &str, script: Script) -> Result<Plan, Msg> {
    plan_in(root, game, classname, script, DIR)
}

/// [`plan`] with the override's files in the project folder `dir` instead of `entities`.
pub fn plan_in(root: &Path, game: &GameConfig, classname: &str, script: Script, dir: &str) -> Result<Plan, Msg> {
    let dir = dir.trim_matches('/');
    let res_dir = format!("res://{dir}");
    project_path(root, &res_dir)?;
    if format!("{res_dir}/").starts_with(ADDON_RES) {
        return Err(msg!("Overrides are never written inside the addons folder, and {path} is in it", path = res_dir));
    }

    let valid = !classname.is_empty() && classname.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(msg!("{classname} is not a classname Override in project can use", classname));
    }

    if script == Script::Player && classname != "info_player_start" {
        return Err(msg!("The first person player is the script of info_player_start only"));
    }

    let definition = definition_res_in(&res_dir, classname);
    let script_path = script_res_in(&res_dir, classname);
    if is_override(root, &definition) {
        return Err(msg!("{classname} is overridden already. Change it in {definition} and {script}", classname, definition, script = script_path));
    }

    let (source_path, source_res) = source(root, classname)?;
    if let Some(clash) = game.clashes.iter().find(|c| c.classname == classname) {
        let winner = clash.winner();
        if !winner.starts_with(ADDON_RES) && !winner.starts_with(&format!("{}/", entity_pack::RES_DIR)) {
            return Err(defined_by(classname, winner));
        }
    }

    let source_text = read(&source_path, &source_res)?;
    let base = base_script(&source_text);
    if base.as_deref().is_some_and(|b| !b.ends_with(".gd")) || (base.is_none() && has_scene_file(&source_text)) {
        return Err(msg!(
            "TrenchDot cannot copy {classname}, its script is not GDScript or it builds a scene without a script. Copy its definition by hand",
            classname
        ));
    }

    let fail = |e: String| msg!("{classname} could not be copied: {error}", classname, error = e);
    let base_text = base.filter(|_| script == Script::Extend).and_then(|base| project_path(root, &base).ok()).and_then(|p| read(&p, "").ok());
    let copy = definition_copy_in(&res_dir, &source_text, classname, script).map_err(fail)?;
    let script_text = script_for(classname, &source_text, base_text.as_deref(), script).map_err(fail)?;

    let project = read(&root.join("project.godot"), "project.godot")?;
    let Wiring { config, settings, fgd: own } = own_wiring(root, &project)?;

    let mut plan = Plan {
        classname: classname.to_string(),
        definition: definition.clone(),
        script: script_path.clone(),
        fgd: String::new(),
        writes: Vec::new(),
        settings: Vec::new(),
    };
    let add_new = |res: &str, text: String, plan: &mut Plan| -> Result<(), Msg> {
        let path = project_path(root, res)?;
        if path.exists() {
            return Err(msg!("{path} is in the way, it exists already. Rename or move it, then try again", path = res));
        }

        plan.writes.push(Write { path, text, replace: false });
        Ok(())
    };
    add_new(&definition, copy, &mut plan)?;
    add_new(&script_path, script_text, &mut plan)?;

    match own {
        Some(fgd) => {
            let path = project_path(root, &fgd)?;
            let text = read(&path, &fgd)?;
            if let Some(own) = own_definition(root, &text, classname) {
                return Err(defined_by(classname, &own));
            }

            let text = add_to_fgd_in(&res_dir, &text, classname).map_err(|e| {
                msg!("{path} could not be extended: {error}. Add {classname} to its Entity Definitions yourself", path = fgd, error = e, classname)
            })?;
            plan.writes.push(Write { path, text, replace: true });
            plan.fgd = fgd;
        }
        None => {
            plan.fgd = format!("{res_dir}/{FGD}");
            add_new(&plan.fgd.clone(), new_fgd_in(&res_dir, classname), &mut plan)?;
        }
    }

    let settings_res = match settings {
        Some(res) => res,
        None => {
            let res = format!("{res_dir}/{MAP_SETTINGS}");
            add_new(&res, new_map_settings(&plan.fgd), &mut plan)?;
            plan.settings.push((MAP_SETTINGS_KEY.0, MAP_SETTINGS_KEY.1, res.clone()));
            res
        }
    };
    if config.is_none() {
        let res = format!("{res_dir}/{GAME_CONFIG}");
        add_new(&res, new_game_config(&plan.fgd, &settings_res), &mut plan)?;
        plan.settings.push((GAME_CONFIG_KEY.0, GAME_CONFIG_KEY.1, res));
    }

    Ok(plan)
}

/// Where the overrides Godot's first room tests build with live, from the root of the Godot project in this repository.
pub const FIXTURE_DIR: &str = "tests/first_room/project/entities";

/// The files of that fixture as (path from the Godot project root, text): the first person player and `func_button`,
/// overridden one after the other in a fresh project that has the addon and the pack of `godot`.
pub fn fixture(godot: &Path) -> Result<Vec<(String, String)>, String> {
    let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
    let root = scratch.path();
    let copy = |res: &str| -> Result<String, String> {
        let rel = res.trim_start_matches("res://");
        let text = std::fs::read_to_string(godot.join(rel)).map_err(|e| format!("{res}: {e}"))?;
        std::fs::create_dir_all(root.join(rel).parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(root.join(rel), &text).map_err(|e| e.to_string())?;
        Ok(text)
    };
    std::fs::write(root.join("project.godot"), "config_version=5\n").map_err(|e| e.to_string())?;

    let game = GameConfig::builtin();
    for (classname, script) in [("info_player_start", Script::Player), ("func_button", Script::Extend)] {
        let (_, res) = source(godot, classname).map_err(|e| e.en)?;
        if let Some(base) = base_script(&copy(&res)?) {
            copy(&base)?;
        }

        let plan = plan_in(root, &game, classname, script, FIXTURE_DIR).map_err(|e| e.en)?;
        for write in &plan.writes {
            std::fs::create_dir_all(write.path.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&write.path, &write.text).map_err(|e| e.to_string())?;
        }

        let project = root.join("project.godot");
        let mut text = std::fs::read_to_string(&project).map_err(|e| e.to_string())?;
        for (section, key, value) in &plan.settings {
            text = godot_settings::with_setting(&text, section, key, value);
        }

        std::fs::write(&project, text).map_err(|e| e.to_string())?;
    }

    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join(FIXTURE_DIR)).map_err(|e| e.to_string())?.flatten() {
        let name = format!("{FIXTURE_DIR}/{}", entry.file_name().to_string_lossy());
        files.push((name, std::fs::read_to_string(entry.path()).map_err(|e| e.to_string())?));
    }

    files.sort();
    Ok(files)
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

#[cfg(test)]
mod plan_tests {
    use super::*;
    use td_formats::Clash;

    const GODOT: &str = "config_version=5\n\n[application]\n\nconfig/name=\"Room\"\n";
    const MAP_SETTINGS_WITHOUT_FGD: &str = "[gd_resource type=\"Resource\" format=3]\n\n[resource]\n";

    fn addon() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../godot/addons/func_godot")
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().map(Result::unwrap) {
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn put(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A project with the addon's definitions and the pack installed, as Install Gameplay Entities leaves it.
    fn project(godot: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        copy_dir(&addon().join("fgd"), &root.join("addons/func_godot/fgd"));
        copy_dir(&addon().join("gameplay_pack/definitions"), &root.join("trenchdot/entities/definitions"));
        copy_dir(&addon().join("gameplay_pack/scripts"), &root.join("trenchdot/entities/scripts"));
        copy_dir(&addon().join("src/trenchdot/entities"), &root.join("addons/func_godot/src/trenchdot/entities"));
        put(root, "project.godot", godot);
        dir
    }

    fn settings_text(lines: &[(&str, &str, &str)]) -> String {
        lines.iter().fold(GODOT.to_string(), |text, (section, key, value)| godot_settings::with_setting(&text, section, key, value))
    }

    fn own_settings() -> String {
        settings_text(&[("func_godot", "default_map_settings", "res://mine/ms.tres"), ("trenchdot", "game_config", "res://mine/gc.tres")])
    }

    fn game() -> GameConfig {
        GameConfig::builtin()
    }

    fn refusal(plan: Result<Plan, Msg>) -> String {
        plan.unwrap_err().en
    }

    fn plan_of(root: &Path, classname: &str) -> Plan {
        plan(root, &game(), classname, Script::Extend).unwrap()
    }

    fn refused(root: &Path, classname: &str) -> String {
        refusal(plan(root, &game(), classname, Script::Extend))
    }

    /// What applying a plan leaves in the project, without the editor.
    fn apply(root: &Path, plan: &Plan) {
        for write in &plan.writes {
            assert_eq!(write.path.exists(), write.replace, "{}", write.path.display());
            std::fs::create_dir_all(write.path.parent().unwrap()).unwrap();
            std::fs::write(&write.path, &write.text).unwrap();
        }

        let mut text = std::fs::read_to_string(root.join("project.godot")).unwrap();
        for (section, key, value) in &plan.settings {
            text = godot_settings::with_setting(&text, section, key, value);
        }

        std::fs::write(root.join("project.godot"), text).unwrap();
    }

    fn rels(root: &Path, plan: &Plan) -> Vec<String> {
        plan.writes.iter().map(|w| w.path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/")).collect()
    }

    fn own_resources(root: &Path) {
        put(root, "mine/fgd.tres", &new_fgd_in("res://mine", "my_thing"));
        put(root, "mine/ms.tres", &new_map_settings("res://mine/fgd.tres"));
        put(root, "mine/gc.tres", &new_game_config("res://mine/fgd.tres", "res://mine/ms.tres"));
    }

    fn tres_with_uid(uid: &str) -> String {
        format!("[gd_resource type=\"Resource\" format=3 uid=\"{uid}\"]\n\n[resource]\n")
    }

    fn text_of(root: &Path, rel: &str) -> String {
        std::fs::read_to_string(root.join(rel)).unwrap().replace("\r\n", "\n")
    }

    #[test]
    fn a_fresh_project_gets_the_override_the_three_resources_and_both_settings() {
        let dir = project(GODOT);
        let root = dir.path();
        let plan = plan_of(root, "func_button");
        assert_eq!(
            rels(root, &plan),
            [
                "entities/func_button.tres",
                "entities/func_button.gd",
                "entities/project_fgd.tres",
                "entities/project_map_settings.tres",
                "entities/project_game_config.tres"
            ]
        );
        let source = text_of(root, "trenchdot/entities/definitions/func_button.tres");
        let base = text_of(root, "trenchdot/entities/scripts/td_button.gd");
        assert_eq!(plan.writes[0].text, definition_copy(&source, "func_button", Script::Extend).unwrap());
        assert_eq!(plan.writes[1].text, script_for("func_button", &source, Some(&base), Script::Extend).unwrap());
        assert_eq!(plan.writes[2].text, new_fgd("func_button"));
        assert_eq!(plan.writes[3].text, new_map_settings("res://entities/project_fgd.tres"));
        assert_eq!(plan.writes[4].text, new_game_config("res://entities/project_fgd.tres", "res://entities/project_map_settings.tres"));
        assert!(plan.writes.iter().all(|w| !w.replace));
        assert_eq!(
            plan.settings,
            [
                ("func_godot", "default_map_settings", "res://entities/project_map_settings.tres".to_string()),
                ("trenchdot", "game_config", "res://entities/project_game_config.tres".to_string())
            ]
        );
        assert_eq!(plan.definition, "res://entities/func_button.tres");
        assert_eq!(plan.script, "res://entities/func_button.gd");
        assert_eq!(plan.fgd, "res://entities/project_fgd.tres");
        assert!(!root.join("entities").exists(), "a plan writes nothing");
        assert_eq!(text_of(root, "project.godot"), GODOT);
    }

    #[test]
    fn a_second_override_only_adds_to_the_fgd() {
        let dir = project(GODOT);
        let root = dir.path();
        apply(root, &plan_of(root, "func_button"));
        let second = plan_of(root, "light");
        assert_eq!(rels(root, &second), ["entities/light.tres", "entities/light.gd", "entities/project_fgd.tres"]);
        assert!(second.settings.is_empty());
        assert!(second.writes[2].replace);
        assert_eq!(second.writes[2].text, add_to_fgd(&new_fgd("func_button"), "light").unwrap());
        apply(root, &second);
        let third = plan_of(root, "func_door");
        assert_eq!(third.writes[2].text, add_to_fgd(&add_to_fgd(&new_fgd("func_button"), "light").unwrap(), "func_door").unwrap());
        assert_eq!(third.fgd, "res://entities/project_fgd.tres");
    }

    #[test]
    fn own_resources_sharing_an_fgd_get_only_that_fgd_edited() {
        let dir = project(&own_settings());
        let root = dir.path();
        own_resources(root);
        let plan = plan_of(root, "func_door");
        assert_eq!(rels(root, &plan), ["entities/func_door.tres", "entities/func_door.gd", "mine/fgd.tres"]);
        assert_eq!(plan.writes[2].text, add_to_fgd(&new_fgd_in("res://mine", "my_thing"), "func_door").unwrap());
        assert!(plan.settings.is_empty());
        assert_eq!(plan.fgd, "res://mine/fgd.tres");
        assert_eq!(own_fgd(root).unwrap().as_deref(), Some("res://mine/fgd.tres"));
    }

    #[test]
    fn a_missing_piece_is_created_and_pointed_at_the_own_fgd() {
        let config_only = project(&settings_text(&[("trenchdot", "game_config", "res://mine/gc.tres")]));
        let root = config_only.path();
        own_resources(root);
        let first = plan_of(root, "func_door");
        assert_eq!(rels(root, &first)[2..], ["mine/fgd.tres", "entities/project_map_settings.tres"]);
        assert_eq!(first.writes[3].text, new_map_settings("res://mine/fgd.tres"));
        assert_eq!(first.settings, [("func_godot", "default_map_settings", "res://entities/project_map_settings.tres".to_string())]);

        let settings_only = project(&settings_text(&[("func_godot", "default_map_settings", "res://mine/ms.tres")]));
        let root = settings_only.path();
        own_resources(root);
        let second = plan_of(root, "func_door");
        assert_eq!(rels(root, &second)[2..], ["mine/fgd.tres", "entities/project_game_config.tres"]);
        assert_eq!(second.writes[3].text, new_game_config("res://mine/fgd.tres", "res://mine/ms.tres"));
        assert_eq!(second.settings, [("trenchdot", "game_config", "res://entities/project_game_config.tres".to_string())]);
    }

    #[test]
    fn the_legacy_game_config_setting_counts_until_the_new_one_is_set() {
        let legacy = project(&settings_text(&[("godottrench", "game_config", "res://mine/gc.tres")]));
        own_resources(legacy.path());
        let plan = plan_of(legacy.path(), "func_door");
        assert_eq!(plan.fgd, "res://mine/fgd.tres");
        assert!(plan.settings.iter().all(|(_, key, _)| *key != "game_config"), "an own legacy game config is kept");

        let both = project(&settings_text(&[("godottrench", "game_config", "res://mine/gone.tres"), ("trenchdot", "game_config", "res://mine/gc.tres")]));
        own_resources(both.path());
        assert_eq!(plan_of(both.path(), "func_door").fgd, "res://mine/fgd.tres", "the new setting wins over the legacy one");
    }

    #[test]
    fn settings_that_name_the_addon_or_nothing_count_as_unset() {
        let addon_names = settings_text(&[
            ("func_godot", "default_map_settings", "res://addons/func_godot/func_godot_default_map_settings.tres"),
            ("trenchdot", "game_config", "res://addons/func_godot/game_config/trenchdot/trenchdot_game_config.tres"),
        ]);
        let dir = project(&addon_names);
        let plan = plan_of(dir.path(), "func_door");
        assert_eq!(plan.writes.len(), 5);
        assert_eq!(plan.settings.len(), 2);

        let empty = project(&settings_text(&[("func_godot", "default_map_settings", ""), ("trenchdot", "game_config", "")]));
        assert_eq!(plan_of(empty.path(), "func_door").settings.len(), 2);
    }

    #[test]
    fn uid_settings_resolve_to_the_project_and_addon_files_and_an_unknown_uid_refuses() {
        let dir = project(&settings_text(&[("func_godot", "default_map_settings", "uid://mine_ms"), ("trenchdot", "game_config", "uid://mine_gc")]));
        let root = dir.path();
        own_resources(root);
        put(root, "mine/ms.tres", &new_map_settings("res://mine/fgd.tres").replacen("format=3", "format=3 uid=\"uid://mine_ms\"", 1));
        put(root, "mine/gc.tres", &new_game_config("res://mine/fgd.tres", "res://mine/ms.tres").replacen("format=3", "format=3 uid=\"uid://mine_gc\"", 1));
        let plan = plan_of(root, "func_door");
        assert_eq!(plan.fgd, "res://mine/fgd.tres");
        assert!(plan.settings.is_empty());

        let addon_uid = project(&settings_text(&[("func_godot", "default_map_settings", "uid://addon_ms")]));
        put(addon_uid.path(), "addons/func_godot/func_godot_default_map_settings.tres", &tres_with_uid("uid://addon_ms"));
        let plan = plan_of(addon_uid.path(), "func_door");
        assert_eq!(plan.settings.len(), 2);

        for (section, key) in [("func_godot", "default_map_settings"), ("trenchdot", "game_config"), ("godottrench", "game_config")] {
            let unknown = project(&settings_text(&[(section, key, "uid://binary_res")]));
            put(unknown.path(), "mine/settings.res", "RSRC binary");
            let text = refused(unknown.path(), "func_door");
            assert_eq!(text, format!("Cannot find uid://binary_res of {section}/{key} in the project. Set {section}/{key} to its res:// path, then try again"));
            assert!(!unknown.path().join("entities").exists());
        }
    }

    #[test]
    fn an_empty_game_config_setting_leaves_the_legacy_one_in_charge() {
        let dir = project(&settings_text(&[("trenchdot", "game_config", ""), ("godottrench", "game_config", "res://mine/gc.tres")]));
        own_resources(dir.path());
        let plan = plan_of(dir.path(), "func_door");
        assert_eq!(plan.fgd, "res://mine/fgd.tres");
        assert!(plan.settings.iter().all(|(_, key, _)| *key != "game_config"), "{:?}", plan.settings);
    }

    #[test]
    fn a_crlf_project_file_and_resources_still_plan() {
        let dir = project(&own_settings().replace('\n', "\r\n"));
        let root = dir.path();
        own_resources(root);
        for name in ["fgd", "ms", "gc"] {
            let path = root.join(format!("mine/{name}.tres"));
            std::fs::write(&path, std::fs::read_to_string(&path).unwrap().replace('\n', "\r\n")).unwrap();
        }

        let plan = plan_of(root, "func_door");
        assert_eq!(plan.fgd, "res://mine/fgd.tres");
        assert!(!plan.writes[2].text.contains('\r'));
    }

    #[test]
    fn resources_that_build_with_the_addon_fgd_refuse() {
        let ms = project(&settings_text(&[("func_godot", "default_map_settings", "res://mine/ms.tres")]));
        put(ms.path(), "mine/ms.tres", &new_map_settings(entity_pack::DEFAULT_FGD));
        assert!(refused(ms.path(), "func_door").starts_with("Your map settings res://mine/ms.tres build with the addon's FGD"));
        put(ms.path(), "mine/ms.tres", MAP_SETTINGS_WITHOUT_FGD);
        assert!(refused(ms.path(), "func_door").starts_with("Your map settings res://mine/ms.tres"), "no Entity Fgd line is the default");

        let gc = project(&settings_text(&[("trenchdot", "game_config", "res://mine/gc.tres")]));
        put(gc.path(), "mine/gc.tres", &new_game_config(entity_pack::DEFAULT_FGD, "res://mine/ms.tres"));
        assert!(refused(gc.path(), "func_door").starts_with("Your game config res://mine/gc.tres builds with the addon's FGD"));

        let mixed = project(&own_settings());
        put(mixed.path(), "mine/ms.tres", &new_map_settings("res://mine/fgd.tres"));
        put(mixed.path(), "mine/gc.tres", &new_game_config(entity_pack::DEFAULT_FGD, "res://mine/ms.tres"));
        assert!(refused(mixed.path(), "func_door").starts_with("Your game config res://mine/gc.tres"), "one own FGD does not excuse the other");
        assert!(!mixed.path().join("entities").exists());
    }

    #[test]
    fn two_own_fgds_refuse_and_the_same_one_does_not() {
        let dir = project(&own_settings());
        let root = dir.path();
        own_resources(root);
        put(root, "mine/other.tres", &new_fgd_in("res://mine", "other_thing"));
        put(root, "mine/gc.tres", &new_game_config("res://mine/other.tres", "res://mine/ms.tres"));
        let text = refused(root, "func_door");
        assert!(text.contains("use two FGDs, res://mine/other.tres and res://mine/fgd.tres"), "{text}");
        assert!(own_fgd(root).is_err());
        put(root, "mine/gc.tres", &new_game_config("res://mine/fgd.tres", "res://mine/ms.tres"));
        assert!(plan(root, &game(), "func_door", Script::Extend).is_ok());
    }

    #[test]
    fn a_setting_for_a_file_that_is_not_there_refuses() {
        let missing = project(&settings_text(&[("trenchdot", "game_config", "res://mine/nothing.tres")]));
        assert!(refused(missing.path(), "func_door").contains("res://mine/nothing.tres cannot be read"));
        let odd = project(&settings_text(&[("func_godot", "default_map_settings", "C:/maps/ms.tres")]));
        assert!(refused(odd.path(), "func_door").contains("C:/maps/ms.tres, which is not a file"));
        let up = project(&settings_text(&[("func_godot", "default_map_settings", "res://../ms.tres")]));
        assert!(refused(up.path(), "func_door").contains("res://../ms.tres, which is not a file"));
        let no_project = project(GODOT);
        std::fs::remove_file(no_project.path().join("project.godot")).unwrap();
        assert!(refused(no_project.path(), "func_door").contains("project.godot cannot be read"));
    }

    #[test]
    fn an_existing_file_refuses_and_nothing_is_planned() {
        for rel in [
            "entities/func_door.tres",
            "entities/func_door.gd",
            "entities/project_fgd.tres",
            "entities/project_map_settings.tres",
            "entities/project_game_config.tres",
        ] {
            let dir = project(GODOT);
            put(dir.path(), rel, "mine");
            let text = refused(dir.path(), "func_door");
            assert!(text.starts_with(&format!("res://{rel} is in the way")), "{rel}: {text}");
            assert_eq!(text_of(dir.path(), rel), "mine");
        }

        let other = project(GODOT);
        put(other.path(), "entities/light.tres", "mine");
        assert!(plan(other.path(), &game(), "func_door", Script::Extend).is_ok(), "another classname is no obstacle");
    }

    #[test]
    fn a_class_the_project_defines_itself_refuses() {
        let dir = project(GODOT);
        let mut game = game();
        let clash = |winner: &str, intended: bool| Clash {
            classname: "func_door".into(),
            definitions: vec!["res://trenchdot/entities/definitions/func_door.tres".into(), winner.into()],
            intended,
        };
        game.clashes = vec![clash("res://mine/door.tres", false)];
        let text = refusal(plan(dir.path(), &game, "func_door", Script::Extend));
        assert!(text.starts_with("func_door is defined by res://mine/door.tres"), "{text}");
        game.clashes = vec![clash("res://mine/door.tres", true)];
        assert!(plan(dir.path(), &game, "func_door", Script::Extend).is_err(), "an intended winner outside the addon is still the project's own");

        game.clashes = vec![clash("res://trenchdot/entities/definitions/func_door.tres", false)];
        assert!(plan(dir.path(), &game, "func_door", Script::Extend).is_ok());
        game.clashes = vec![clash("res://mine/door.tres", true)];
        assert!(plan(dir.path(), &game, "light", Script::Extend).is_ok(), "a clash of another class is no obstacle");
    }

    #[test]
    fn a_class_the_own_fgd_lists_without_a_clash_refuses() {
        let dir = project(&own_settings());
        let root = dir.path();
        own_resources(root);
        put(root, "mine/fgd.tres", &new_fgd_in("res://mine", "func_door").replace("[ExtResource(\"2_base\")]", "[]"));
        put(root, "mine/func_door.tres", "[gd_resource type=\"Resource\" format=3]\n\n[resource]\nclassname = \"func_door\"\n");
        assert_eq!(refused(root, "func_door"), "func_door is defined by res://mine/func_door.tres in this project already, edit that definition instead");
        assert!(plan(root, &game(), "func_button", Script::Extend).is_ok(), "the own FGD defines no func_button");

        put(root, "mine/func_door.tres", "[gd_resource type=\"Resource\" format=3]\n\n[resource]\nclassname = \"func_doorway\"\n");
        assert!(plan(root, &game(), "func_door", Script::Extend).is_ok(), "a classname that only starts the same is another class");

        let pack = project(&own_settings());
        own_resources(pack.path());
        let fgd = new_fgd_in("res://mine", "func_door").replace("res://mine/func_door.tres", "res://trenchdot/entities/definitions/func_door.tres");
        put(pack.path(), "mine/fgd.tres", &fgd);
        assert!(plan(pack.path(), &game(), "func_door", Script::Extend).is_ok(), "the pack definition is what gets overridden");
    }

    #[test]
    fn a_definition_with_a_csharp_script_or_a_scene_without_a_script_refuses() {
        let cannot = "TrenchDot cannot copy light, its script is not GDScript or it builds a scene without a script. Copy its definition by hand";
        let dir = project(GODOT);
        let root = dir.path();
        let light = text_of(root, "addons/func_godot/fgd/trenchdot/light.tres");
        put(root, "addons/func_godot/fgd/trenchdot/light.tres", &light.replace("td_light.gd", "TdLight.cs"));
        assert_eq!(refused(root, "light"), cannot);

        let without_script: String = light.lines().filter(|l| !l.starts_with("script_class = ")).map(|l| format!("{l}\n")).collect();
        put(root, "addons/func_godot/fgd/trenchdot/light.tres", &format!("{without_script}scene_file = ExtResource(\"9_scene\")\n"));
        assert_eq!(refused(root, "light"), cannot);
        put(root, "addons/func_godot/fgd/trenchdot/light.tres", &format!("{without_script}scene_file = null\n"));
        assert!(plan(root, &game(), "light", Script::Extend).is_ok(), "no scene and no script gets the template");
        put(root, "addons/func_godot/fgd/trenchdot/light.tres", &format!("{light}scene_file = ExtResource(\"9_scene\")\n"));
        assert!(plan(root, &game(), "light", Script::Extend).is_ok(), "a scene with a GDScript is extended");
        assert!(!root.join("entities").exists());
    }

    #[test]
    fn an_override_is_reported_as_one_and_another_file_is_in_the_way() {
        let dir = project(GODOT);
        let root = dir.path();
        let first = plan_of(root, "func_door");
        apply(root, &first);
        assert_eq!(refused(root, "func_door"), "func_door is overridden already. Change it in res://entities/func_door.tres and res://entities/func_door.gd");
        assert!(is_override(root, "res://entities/func_door.tres"));
        assert!(!is_override(root, "res://entities/func_door.gd") && !is_override(root, "res://entities/light.tres"));

        put(root, "entities/light.tres", "[gd_resource type=\"Resource\" format=3]\n\n[resource]\nmetadata/trenchdot_override = false\n");
        assert!(refused(root, "light").starts_with("res://entities/light.tres is in the way"));
    }

    #[test]
    fn an_installed_pack_without_the_definition_says_so() {
        let dir = project(GODOT);
        let root = dir.path();
        std::fs::remove_file(root.join("trenchdot/entities/definitions/func_door.tres")).unwrap();
        std::fs::copy(addon().join("gameplay_pack/pack.json"), root.join("trenchdot/entities/pack.json")).unwrap();
        assert_eq!(
            refused(root, "func_door"),
            "The Gameplay entities pack in this project has no res://trenchdot/entities/definitions/func_door.tres, so there is no definition of func_door to copy"
        );
        assert!(plan(root, &game(), "func_button", Script::Extend).is_ok());
    }

    #[test]
    fn the_source_is_the_installed_pack_then_the_addon() {
        let dir = project(GODOT);
        let root = dir.path();
        let pack = root.join("trenchdot/entities/definitions/func_button.tres");
        assert_eq!(source(root, "func_button").unwrap(), (pack, "res://trenchdot/entities/definitions/func_button.tres".to_string()));
        put(root, "trenchdot/entities/definitions/func_button.tres", &(text_of(root, "trenchdot/entities/definitions/func_button.tres") + "# edited\n"));
        assert!(plan_of(root, "func_button").writes[0].text.contains("# edited"), "the installed pack copy is the source, edits included");

        assert_eq!(source(root, "light").unwrap().1, "res://addons/func_godot/fgd/trenchdot/light.tres");
        assert_eq!(source(root, "func_detail").unwrap().1, "res://addons/func_godot/fgd/func_detail.tres");
        assert_eq!(source(root, "worldspawn").unwrap().1, "res://addons/func_godot/fgd/worldspawn.tres");
    }

    #[test]
    fn classnames_without_a_source_refuse() {
        let dir = project(GODOT);
        let root = dir.path();
        assert!(refused(root, "my_thing").starts_with("Override in project has no definition to copy for my_thing"));

        std::fs::remove_file(root.join("addons/func_godot/fgd/trenchdot/light.tres")).unwrap();
        assert!(refused(root, "light").contains("no definition at res://addons/func_godot/fgd/trenchdot/light.tres"));

        std::fs::remove_file(root.join("trenchdot/entities/definitions/func_door.tres")).unwrap();
        let text = refused(root, "func_door");
        assert!(text.starts_with("func_door comes with the Gameplay entities pack") && text.contains("Install Gameplay Entities"), "{text}");
        assert!(!root.join("entities").exists());
    }

    #[test]
    fn a_missing_base_script_refuses() {
        let dir = project(GODOT);
        std::fs::remove_file(dir.path().join("trenchdot/entities/scripts/td_button.gd")).unwrap();
        let text = refused(dir.path(), "func_button");
        assert!(text.starts_with("func_button could not be copied") && text.contains("td_button.gd is missing"), "{text}");
    }

    #[test]
    fn bad_classnames_and_a_misplaced_player_refuse() {
        let dir = project(GODOT);
        for name in ["", "../light", "light/x", "a b", "light\"", "func_door.tres"] {
            assert!(refused(dir.path(), name).contains("is not a classname"), "{name:?}");
        }

        assert!(refusal(plan(dir.path(), &game(), "light", Script::Player)).contains("info_player_start only"));
        let player = plan(dir.path(), &game(), "info_player_start", Script::Player).unwrap();
        assert_eq!(player.writes[1].text, PLAYER_SCRIPT);
        assert!(player.writes[0].text.contains("node_class = \"CharacterBody3D\""));
    }

    #[test]
    fn nothing_is_ever_planned_inside_addons() {
        let own = project(&own_settings());
        own_resources(own.path());
        let fresh = project(GODOT);
        for root in [own.path(), fresh.path()] {
            for class in ["info_player_start", "light", "light_spot", "func_detail", "worldspawn", "func_door", "func_button", "func_platform"] {
                for write in &plan_of(root, class).writes {
                    assert!(!write.path.strip_prefix(root).unwrap().starts_with("addons"), "{class}: {}", write.path.display());
                }
            }
        }

        for dir in ["addons", "addons/func_godot/entities", "/addons/x/"] {
            let text = refusal(plan_in(fresh.path(), &game(), "light", Script::Extend, dir));
            assert!(text.starts_with("Overrides are never written inside the addons folder"), "{dir}: {text}");
        }

        assert!(plan_in(fresh.path(), &game(), "light", Script::Extend, "addons_mine").is_ok());
        assert!(refusal(plan_in(fresh.path(), &game(), "light", Script::Extend, "../out")).contains("not a file inside it"));
    }

    #[test]
    fn another_folder_puts_every_path_in_it() {
        let dir = project(GODOT);
        let root = dir.path();
        let plan = plan_in(root, &game(), "light", Script::Extend, "tests/first_room/project/entities").unwrap();
        let res = "res://tests/first_room/project/entities";
        assert_eq!(rels(root, &plan)[0], "tests/first_room/project/entities/light.tres");
        assert_eq!(plan.fgd, format!("{res}/project_fgd.tres"));
        assert!(plan.writes[0].text.contains(&format!("path=\"{res}/light.gd\"")));
        assert!(plan.writes[2].text.contains(&format!("path=\"{res}/light.tres\"")));
        assert!(plan.writes[3].text.contains("project_fgd.tres"));
        assert_eq!(plan.settings[1].2, format!("{res}/project_game_config.tres"));
    }

    #[test]
    fn a_second_project_does_not_inherit_the_first() {
        let first = project(GODOT);
        apply(first.path(), &plan_of(first.path(), "func_door"));
        let second = project(GODOT);
        let plan = plan_of(second.path(), "light");
        assert_eq!(plan.writes.len(), 5);
        assert_eq!(plan.writes[2].text, new_fgd("light"));
    }
}
