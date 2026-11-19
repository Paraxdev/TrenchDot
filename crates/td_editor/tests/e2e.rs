//! End-to-end tests: launch the real editor with its MCP server and drive it with simulated user input.
//! They need a GPU and a desktop session, so they are ignored by default:
//!   cargo test -p td_editor --test e2e -- --ignored --test-threads=1
//! Screenshots are written to target/e2e/.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};

struct Editor {
    child: Child,
    port: u16,
    /// What the editor wants as a bearer token, when it wants one.
    token: Option<String>,
    name: &'static str,
}

impl Drop for Editor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The editor with its MCP HTTP server on `port` and the settings every test starts from.
fn editor_command(port: u16) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_trenchdot"));
    command.arg(format!("--mcp-http={port}")).arg("--default-prefs");
    // Nothing listens on these, so saves never reach a Godot editor or game open on the default ports.
    command.arg(format!("--live-link-port={}", free_port())).arg(format!("--hot-reload-port={}", free_port()));
    // TRENCHDOT_E2E_VISIBLE=1 shows the windows, to watch a test drive the editor. CI runs under Xvfb, which shows
    // nothing anyway.
    if cfg!(windows) && std::env::var_os("TRENCHDOT_E2E_VISIBLE").is_none() {
        command.arg("--offscreen");
    }

    // A token or switch from the shell that runs the tests would lock the harness out.
    for var in ["MCP_TOKEN", "DISABLE_MCP"] {
        command.env_remove(format!("TRENCHDOT_{var}")).env_remove(format!("{}{var}", td_editor::legacy::ENV_PREFIX));
    }

    command
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn artifacts() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/e2e");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Makes `link` a link to the folder `target`, a junction on Windows, which needs no rights to make.
fn link_dir(link: &std::path::Path, target: &std::path::Path) -> bool {
    #[cfg(windows)]
    let made = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(std::path::absolute(link).unwrap())
        .arg(std::path::absolute(target).unwrap())
        .output()
        .is_ok_and(|o| o.status.success());
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(target, link).is_ok();
    made
}

/// Every file below `dir`, following no links.
fn files_below(dir: &std::path::Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
    entries.flat_map(|e| if e.file_type().is_ok_and(|t| t.is_dir()) { files_below(&e.path()) } else { vec![e.path()] }).collect()
}

impl Editor {
    fn launch(name: &'static str) -> Editor {
        Self::launch_with(name, &[])
    }

    fn launch_with(name: &'static str, args: &[&str]) -> Editor {
        Self::launch_env(name, args, &[])
    }

    fn launch_env(name: &'static str, args: &[&str], env: &[(&str, &str)]) -> Editor {
        let port = free_port();
        let token = args
            .iter()
            .find_map(|a| a.strip_prefix("--mcp-token="))
            .or_else(|| env.iter().find(|(k, _)| *k == "TRENCHDOT_MCP_TOKEN").map(|(_, v)| v.trim()).filter(|v| !v.is_empty()))
            .map(String::from);
        let child = editor_command(port).args(args).envs(env.iter().copied()).spawn().expect("launch editor");
        let editor = Editor { child, port, token, name };
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if editor
                .try_rpc("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "e2e", "version": "1" } }))
                .is_some()
            {
                break;
            }

            assert!(Instant::now() < deadline, "editor did not start");
            std::thread::sleep(Duration::from_millis(250));
        }

        // Let the first frames lay out the viewports.
        std::thread::sleep(Duration::from_millis(500));
        editor
    }

    /// Posts a JSON body with extra header lines and returns the whole response.
    fn post(&self, headers: &str, body: &str) -> Option<String> {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(180))).ok()?;
        write!(s, "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\n\r\n{body}", body.len()).ok()?;
        let mut out = String::new();
        s.read_to_string(&mut out).ok()?;
        Some(out)
    }

    fn try_rpc(&self, method: &str, params: Value) -> Option<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
        let auth = self.token.as_ref().map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
        let out = self.post(&auth, &body)?;
        let (_, body) = out.split_once("\r\n\r\n")?;
        serde_json::from_str(body).ok()
    }

    /// Calls a tool and returns its structured result, panicking on tool errors.
    fn call(&self, tool: &str, args: Value) -> Value {
        let r = self.try_rpc("tools/call", json!({ "name": tool, "arguments": args })).unwrap_or_else(|| panic!("{tool}: no response"));
        let result = &r["result"];
        assert!(!result["isError"].as_bool().unwrap_or(true), "{tool} failed: {}", result["content"][0]["text"]);
        result["structuredContent"].clone()
    }

    fn call_err(&self, tool: &str, args: Value) -> String {
        let r = self.try_rpc("tools/call", json!({ "name": tool, "arguments": args })).unwrap();
        assert!(r["result"]["isError"].as_bool().unwrap_or(false), "{tool} unexpectedly succeeded");
        r["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string()
    }

    fn input(&self, target: &str, events: Value) -> Value {
        self.call("simulate_input", json!({ "target": target, "events": events }))["state"].clone()
    }

    fn state(&self) -> Value {
        self.call("get_state", json!({}))
    }

    fn brushes(&self) -> u64 {
        self.state()["map"]["brushes"].as_u64().unwrap()
    }

    fn selection_bounds(&self) -> (Vec<f64>, Vec<f64>) {
        let b = &self.state()["selection"]["bounds"];
        let v = |k: &str| b[k].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect::<Vec<_>>();
        (v("min"), v("max"))
    }

    /// Saves a screenshot and returns (width, height, distinct colors sampled).
    fn screenshot(&self, target: &str, label: &str) -> (u32, u32, usize) {
        let r = self.try_rpc("tools/call", json!({ "name": "screenshot", "arguments": { "target": target } })).unwrap();
        let data = r["result"]["content"][0]["data"].as_str().expect("image content");
        let png = base64::engine::general_purpose::STANDARD.decode(data).unwrap();
        std::fs::write(artifacts().join(format!("{}_{label}.png", self.name)), &png).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        let mut colors = std::collections::HashSet::new();
        for (x, y, p) in img.enumerate_pixels() {
            if x % 7 == 0 && y % 7 == 0 {
                colors.insert(p.0);
            }
        }

        (img.width(), img.height(), colors.len())
    }

    fn box_brush(&self, min: [f64; 3], max: [f64; 3]) -> u64 {
        self.call("create_brush", json!({ "min": min, "max": max }))["ids"][0].as_u64().unwrap()
    }
}

fn approx(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
}

#[test]
#[ignore]
fn map_file_save_creates_its_folder() {
    let ed = Editor::launch("save_folder");
    let dir = artifacts().join("save_folder");
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("levels/crypt.tdm");
    ed.call("map_file", json!({ "op": "save", "path": path }));
    assert!(path.is_file(), "saved into a folder that did not exist");

    let scripts = dir.join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    let outside = dir.join("elsewhere/new/crypt.tdm");
    let script = json!({ "steps": [{ "tool": "map_file", "args": { "op": "save", "path": outside } }] });
    std::fs::write(scripts.join("save.json"), script.to_string()).unwrap();
    let summary = ed.call("run_script", json!({ "path": scripts.join("save.json"), "continue_on_error": true }));
    assert_eq!(summary["errors"].as_array().map(Vec::len), Some(1), "{summary}");
    assert!(!dir.join("elsewhere").exists(), "a script makes no folder outside the places it may write");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore]
fn gameplay_place_refuses_unknown_classnames() {
    let ed = Editor::launch("place_unknown");
    let e = ed.call_err("gameplay", json!({ "op": "place", "classname": "ligth", "origin": [0, 0, 0] }));
    assert!(e.contains("no entity definition for 'ligth'") && e.contains("light"), "{e}");
    assert_eq!(ed.call("gameplay", json!({ "op": "place", "classname": "light", "origin": [0, 0, 0] }))["classname"], "light");
}

#[test]
#[ignore]
fn create_brush_entity_wants_its_classname_in_args() {
    let ed = Editor::launch("create_brush_entity");
    let brush = ed.box_brush([40.0, 16.0, -72.0], [104.0, 112.0, -56.0]);
    ed.call("select", json!({ "ids": [brush] }));
    let e = ed.call_err("run_action", json!({ "action": "create_brush_entity", "classname": "func_door" }));
    assert!(e.contains("args.classname") && e.contains("inside args"), "{e}");
    assert_eq!(ed.call("list_nodes", json!({ "type": "entity" }))["total"], 0);
    ed.call("run_action", json!({ "action": "create_brush_entity", "args": { "classname": "func_door" } }));
    let second = ed.box_brush([-104.0, 16.0, 56.0], [-40.0, 112.0, 72.0]);
    ed.call("select", json!({ "ids": [second] }));
    ed.call("run_action", json!({ "action": "create_brush_entity" }));
    let nodes = ed.call("list_nodes", json!({ "type": "entity" }));
    let mut classnames: Vec<_> = nodes["nodes"].as_array().unwrap().iter().map(|n| n["classname"].as_str().unwrap().to_string()).collect();
    classnames.sort();
    assert_eq!(classnames, ["func_detail", "func_door"], "no classname at all keeps the old default");
}

#[test]
#[ignore]
fn place_entities_wants_its_classnames_in_args() {
    let ed = Editor::launch("place_entities");
    let e = ed.call_err("run_action", json!({ "action": "place_entities", "classnames": ["infodecal"], "at": [0, 0, 0] }));
    assert!(e.contains("args.classnames") && e.contains("inside args"), "{e}");
    let e = ed.call_err("run_action", json!({ "action": "place_entities", "args": { "classnames": [], "at": [0, 0, 0] } }));
    assert!(e.contains("args.classnames") && !e.contains("inside args"), "{e}");
    assert_eq!(ed.call("list_nodes", json!({ "type": "entity" }))["total"], 0);
    ed.call("run_action", json!({ "action": "place_entities", "args": { "classnames": ["info_null"], "at": [64, 0, 0], "normal": [0, 1, 0] } }));
    assert_eq!(ed.call("list_nodes", json!({ "type": "entity" }))["total"], 1);
}

/// A decal projects along its local -Y, so placing one with a normal turns it to face out of the surface, where other
/// entities keep their angles.
#[test]
#[ignore]
fn an_infodecal_placed_with_a_normal_turns_to_the_surface() {
    let infodecal = td_formats::GameConfig::with_gameplay_pack().entity("infodecal").cloned().expect("the Gameplay pack has infodecal");
    let dir = linked_project("infodecal_normal", json!([infodecal]));
    let ed = Editor::launch_with("infodecal_normal", &["--project", dir.to_str().unwrap()]);

    let place = |classname: &str, at: [f64; 3], normal: [f64; 3]| -> Value {
        ed.call("run_action", json!({ "action": "place_entities", "args": { "classnames": [classname], "at": at, "normal": normal } }));
        let nodes = ed.call("list_nodes", json!({ "type": "entity" }));
        let id = nodes["nodes"].as_array().unwrap().iter().filter(|n| n["classname"] == classname).map(|n| n["id"].as_u64().unwrap()).max().unwrap();
        ed.call("get_node", json!({ "id": id }))
    };
    let facing = |node: &Value| -> td_core::DVec3 {
        let a: Vec<f64> = node["angles"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let q = td_core::DQuat::from_euler(td_core::EulerRot::YXZ, a[1].to_radians(), a[0].to_radians(), a[2].to_radians());
        q * td_core::DVec3::Y
    };

    for (at, normal) in [([96.0, 40.0, -48.0], [2.0, 3.0, 6.0]), ([-160.0, 8.0, 72.0], [-3.0, -6.0, 2.0])] {
        let decal = place("infodecal", at, normal);
        assert_eq!(decal["origin"], json!(at), "{decal}");
        let want = td_core::DVec3::from(normal).normalize();
        assert!((facing(&decal) - want).length() < 1e-4, "{decal} faces {} not {want}", facing(&decal));
    }

    let straight = place("infodecal", [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    assert_eq!(straight["angles"], json!([0.0, 0.0, 0.0]), "{straight}");

    let marker = place("info_null", [64.0, 0.0, 16.0], [2.0, 3.0, 6.0]);
    assert_eq!(marker["angles"], json!([0.0, 0.0, 0.0]), "{marker}");
}

#[test]
#[ignore]
fn a_drag_held_open_ends_with_a_later_release() {
    let ed = Editor::launch("held_drag");
    let reply =
        ed.call("simulate_input", json!({ "target": "top", "events": [{ "type": "drag", "world": [-64, 0, -64], "to_world": [0, 0, 0], "release": false }] }));
    assert_eq!(reply["held"], true);
    ed.screenshot("window", "held_drag");
    // The brush being drawn follows a move in a later call, so the button stayed down in between.
    ed.input("top", json!([{ "type": "move", "world": [64, 0, 32] }]));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&min, &[-64.0, 0.0, -64.0]) && approx(&max, &[64.0, 128.0, 32.0]), "{min:?} {max:?}");
    let e = ed.call_err("simulate_input", json!({ "target": "top", "events": [{ "type": "click", "world": [0, 0, 0] }] }));
    assert!(e.contains("release"), "{e}");

    let reply = ed.call("simulate_input", json!({ "target": "top", "events": [{ "type": "release" }] }));
    assert_eq!(reply["held"], false);
    ed.input("top", json!([{ "type": "move", "world": [128, 0, 96] }]));
    let (_, max) = ed.selection_bounds();
    assert!(approx(&max, &[64.0, 128.0, 32.0]) && ed.brushes() == 1, "moves after the release draw nothing, {max:?}");
    assert!(ed.call_err("simulate_input", json!({ "events": [{ "type": "release" }] })).contains("no drag is held"));
}

#[test]
#[ignore]
fn escape_cancels_a_move_drag_then_clears_then_leaves_the_tool() {
    let ed = Editor::launch("escape_ladder");
    let brush = ed.box_brush([64.0, 0.0, 32.0], [128.0, 64.0, 96.0]);
    ed.call("select", json!({ "ids": [brush] }));
    ed.call("set_camera", json!({ "view": "top", "center": [96, 0, 64], "zoom": 1.5 }));
    let undo_before = ed.state()["undo"].as_array().unwrap().len();
    let drag = json!({ "type": "drag", "world": [96, 0, 56], "to_world": [160, 0, 120], "release": false });
    assert_eq!(ed.call("simulate_input", json!({ "target": "top", "events": [drag] }))["held"], true);
    std::thread::sleep(Duration::from_millis(300));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0], min[2], max[2]], &[128.0, 192.0, 96.0, 160.0]), "the held drag moved the brush {min:?} {max:?}");

    ed.input("top", json!([{ "type": "key", "key": "Escape" }]));
    let state = ed.state();
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0], min[2], max[2]], &[64.0, 128.0, 32.0, 96.0]), "Escape put the brush back {min:?} {max:?}");
    assert_eq!(state["selection"]["nodes"], json!([brush]), "and kept the selection");
    assert_eq!(state["undo"].as_array().unwrap().len(), undo_before, "the cancelled move leaves no undo step");

    // The button is still down, a move after the cancel must not drag anything.
    ed.input("top", json!([{ "type": "move", "world": [150, 0, 110] }]));
    let (min, _) = ed.selection_bounds();
    assert!(approx(&[min[0], min[2]], &[64.0, 32.0]), "the cancelled drag stays cancelled {min:?}");
    ed.call("simulate_input", json!({ "target": "top", "events": [{ "type": "release" }] }));
    let (min, _) = ed.selection_bounds();
    assert!(approx(&[min[0], min[2]], &[64.0, 32.0]), "the release changes nothing {min:?}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before);

    ed.call("run_action", json!({ "action": "set_tool", "args": { "tool": "vertex" } }));
    ed.input("top", json!([{ "type": "key", "key": "Escape" }]));
    let state = ed.state();
    assert_eq!((state["selection"]["nodes"].clone(), state["editor"]["tool"].clone()), (json!([]), json!("Vertex")), "the next Escape clears the objects");
    ed.input("top", json!([{ "type": "key", "key": "Escape" }]));
    assert_eq!(ed.state()["editor"]["tool"], "Select", "and the one after leaves the tool");
}

#[test]
#[ignore]
fn a_held_drag_keeps_its_modifiers_until_the_release() {
    let ed = Editor::launch("held_alt_drag");
    let brush = ed.box_brush([-32.0, 0.0, -32.0], [32.0, 64.0, 32.0]);
    ed.call("select", json!({ "ids": [brush] }));
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 1.5 }));
    ed.call("run_action", json!({ "action": "set_tool", "args": { "tool": "scale" } }));
    // The scale drag reads Alt on every frame and then scales from the center.
    let drag = json!({ "type": "drag", "world": [32, 0, 0], "to_world": [48, 0, 0], "modifiers": ["alt"], "release": false });
    assert_eq!(ed.call("simulate_input", json!({ "target": "top", "events": [drag] }))["held"], true);
    std::thread::sleep(Duration::from_millis(500));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0]], &[-48.0, 48.0]), "Alt stays down between calls {min:?} {max:?}");
    assert_eq!(ed.state()["ui"]["modifiers"], json!(["alt"]));
    ed.input("top", json!([{ "type": "move", "world": [64, 0, 0] }]));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0]], &[-64.0, 64.0]), "a later move still scales from the center {min:?} {max:?}");
    let reply = ed.call("simulate_input", json!({ "target": "top", "events": [{ "type": "release" }] }));
    assert_eq!(reply["held"], false);
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0]], &[-64.0, 64.0]), "the release keeps the scale under the pointer {min:?} {max:?}");
    // Every simulate_input step sets its own modifiers, so only a call between scripts sees what the release left down.
    assert_eq!(ed.state()["ui"]["modifiers"], json!([]), "Alt was let go with the release");

    let drag = json!({ "type": "drag", "world": [64, 0, 0], "to_world": [80, 0, 0], "release": false });
    assert_eq!(ed.call("simulate_input", json!({ "target": "top", "events": [drag] }))["held"], true);
    std::thread::sleep(Duration::from_millis(500));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0]], &[-64.0, 80.0]), "a drag without Alt scales from the opposite side {min:?} {max:?}");
    ed.call("simulate_input", json!({ "target": "top", "events": [{ "type": "release" }] }));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0]], &[-64.0, 80.0]), "a plain release keeps the scale too {min:?} {max:?}");
}

#[test]
#[ignore]
fn a_held_rotate_drag_shows_its_angle_and_keeps_it_on_release() {
    let ed = Editor::launch("held_rotate_drag");
    let brush = ed.box_brush([32.0, 0.0, 0.0], [160.0, 64.0, 64.0]);
    ed.call("select", json!({ "ids": [brush] }));
    ed.call("set_camera", json!({ "view": "top", "center": [96, 0, 32], "zoom": 1.5 }));
    ed.call("run_action", json!({ "action": "set_tool", "args": { "tool": "rotate" } }));
    let drag = json!({ "type": "drag", "world": [160, 0, 32], "to_world": [96, 0, 96], "release": false });
    assert_eq!(ed.call("simulate_input", json!({ "target": "top", "events": [drag] }))["held"], true);
    std::thread::sleep(Duration::from_millis(500));
    let status = ed.state()["status"].as_str().unwrap().to_string();
    assert!(status.contains("-90.0"), "the held status shows the live angle, got {status}");
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0], min[2], max[2]], &[64.0, 128.0, -32.0, 96.0]), "the held drag already turned the brush {min:?} {max:?}");

    let reply = ed.call("simulate_input", json!({ "target": "top", "events": [{ "type": "release" }] }));
    assert_eq!(reply["held"], false);
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0], min[2], max[2]], &[64.0, 128.0, -32.0, 96.0]), "the release keeps the angle under the pointer {min:?} {max:?}");
    assert!(approx(&[min[1], max[1]], &[0.0, 64.0]), "a turn about the up axis keeps the height {min:?} {max:?}");
    ed.call("run_action", json!({ "action": "undo" }));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0], min[2], max[2]], &[32.0, 160.0, 0.0, 64.0]), "one undo reverts the whole drag {min:?} {max:?}");
}

#[test]
#[ignore]
fn shear_over_mcp_slides_a_side_and_keeps_the_brush_valid() {
    let ed = Editor::launch("shear_mcp");
    let brush = ed.box_brush([0.0, 0.0, 0.0], [64.0, 32.0, 64.0]);
    ed.call("select", json!({ "ids": [brush] }));
    let undo_before = ed.state()["undo"].as_array().unwrap().len();
    let reply = ed.call("transform", json!({ "shear": { "side": "+y", "along": "x", "amount": 16 } }));
    let (min, max) = (&reply["bounds"]["min"], &reply["bounds"]["max"]);
    assert_eq!((min[0].as_f64(), max[0].as_f64()), (Some(0.0), Some(80.0)), "the top side slid 16 along X {reply}");
    assert_eq!((min[1].as_f64(), max[1].as_f64()), (Some(0.0), Some(32.0)), "the height is unchanged {reply}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before + 1, "one undo step");
    assert_eq!(ed.call("validate_map", json!({}))["count"], 0, "the sheared brush is still valid");
    let vertices = ed.call("get_node", json!({ "id": brush }))["vertices"].clone();
    assert_eq!(vertices.as_array().unwrap().len(), 8, "{vertices}");
    let low: Vec<f64> = vertices.as_array().unwrap().iter().filter(|v| v[1].as_f64() == Some(0.0)).map(|v| v[0].as_f64().unwrap()).collect();
    assert!(low.iter().all(|x| *x == 0.0 || *x == 64.0), "the fixed bottom side stayed put {low:?}");

    ed.call("transform", json!({ "shear": { "side": "-x", "along": "z", "amount": -8 } }));
    assert_eq!(ed.call("validate_map", json!({}))["count"], 0);
    assert!(ed.call_err("transform", json!({ "shear": { "side": "y", "along": "x", "amount": 1 } })).contains("shear.side"));
    assert!(ed.call_err("transform", json!({ "shear": { "side": "+y", "along": "y", "amount": 1 } })).contains("shear.along"));
    assert!(ed.call_err("transform", json!({ "shear": { "side": "+y", "along": "x" } })).contains("shear.amount"));
    assert!(ed.call_err("transform", json!({ "shear": { "side": "é+x", "along": "y", "amount": 1 } })).contains("shear.side"));
    ed.call("run_action", json!({ "action": "undo", "args": { "steps": 2 } }));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0], min[2], max[2]], &[0.0, 64.0, 0.0, 64.0]), "undo restores the box {min:?} {max:?}");

    let flat = ed.call("create_terrain", json!({ "origin": [0, 0, 0], "resolution": 3, "cell_size": 32, "shape": "flat" }))["id"].clone();
    ed.call("select", json!({ "ids": [flat] }));
    let undo_before = ed.state()["undo"].as_array().unwrap().len();
    let rejected = json!({ "translate": [32, 0, 0], "shear": { "side": "+y", "along": "x", "amount": 16 } });
    assert!(ed.call_err("transform", rejected).contains("no extent"), "a flat selection has no side to slide");
    let (min, _) = ed.selection_bounds();
    assert_eq!(min[0], 0.0, "the move before the shear was taken back");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before);
}

#[test]
#[ignore]
fn transform_scales_by_factors_about_the_center() {
    let ed = Editor::launch("transform_scale");
    ed.call("set_editor", json!({ "snap": false }));
    let brush = ed.box_brush([40.0, 8.0, -72.0], [104.0, 72.0, -20.0]);
    ed.call("select", json!({ "ids": [brush] }));
    let undo_before = ed.state()["undo"].as_array().unwrap().len();
    let reply = ed.call("transform", json!({ "rotate": { "axis": "y", "degrees": 90 }, "scale": [2, 0.5, -1] }));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&min, &[20.0, 24.0, -78.0]) && approx(&max, &[124.0, 56.0, -14.0]), "turned, then scaled about the turned center {reply}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before + 1);
    assert_eq!(ed.state()["undo"][0], "MCP: Transform");
    assert_eq!(ed.call("validate_map", json!({}))["count"], 0, "the mirrored brush is still valid");

    let undo_before = ed.state()["undo"].clone();
    for zero in [json!([1, 0, 1]), json!([0, 0, 0])] {
        let e = ed.call_err("transform", json!({ "translate": [32, 0, 0], "scale": zero }));
        assert!(e.contains("a scale factor cannot be 0"), "{e}");
    }

    let (min, max) = ed.selection_bounds();
    assert!(approx(&min, &[20.0, 24.0, -78.0]) && approx(&max, &[124.0, 56.0, -14.0]), "a refused scale moves nothing either");
    assert_eq!(ed.state()["undo"], undo_before);

    ed.call("transform", json!({ "scale": [0.5, 2, 1] }));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&min, &[46.0, 8.0, -78.0]) && approx(&max, &[98.0, 72.0, -14.0]), "{min:?} {max:?}");
}

#[test]
#[ignore]
fn the_shear_tool_drags_a_side_as_one_undo_step() {
    let ed = Editor::launch("shear_drag");
    let brush = ed.box_brush([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]);
    ed.call("select", json!({ "ids": [brush] }));
    ed.call("set_camera", json!({ "view": "front", "center": [32, 32, 32], "zoom": 1.5 }));
    ed.call("run_action", json!({ "action": "set_tool", "args": { "tool": "shear" } }));
    let undo_before = ed.state()["undo"].as_array().unwrap().len();
    ed.input("front", json!([{ "type": "drag", "world": [32, 64, 32], "to_world": [80, 64, 32] }]));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0], min[1], max[1]], &[0.0, 112.0, 0.0, 64.0]), "the top side slid 48 along X {min:?} {max:?}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before + 1, "the whole drag is one undo step");
    assert_eq!(ed.call("validate_map", json!({}))["count"], 0);
    ed.call("run_action", json!({ "action": "undo" }));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0]], &[0.0, 64.0]), "one undo reverts the drag {min:?} {max:?}");

    ed.input("front", json!([{ "type": "drag", "world": [32, 24, 32], "to_world": [32, 24, 100] }]));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0]], &[0.0, 64.0]), "a drag that misses every side changes nothing {min:?} {max:?}");
}

#[test]
#[ignore]
fn a_one_shot_rotate_drag_commits_the_angle_it_ended_on() {
    let ed = Editor::launch("one_shot_rotate_drag");
    let brush = ed.box_brush([32.0, 0.0, 0.0], [160.0, 64.0, 64.0]);
    ed.call("select", json!({ "ids": [brush] }));
    ed.call("set_camera", json!({ "view": "top", "center": [96, 0, 32], "zoom": 1.5 }));
    ed.call("run_action", json!({ "action": "set_tool", "args": { "tool": "rotate" } }));
    ed.input("top", json!([{ "type": "drag", "world": [160, 0, 32], "to_world": [160, 0, 96] }]));
    let status = ed.state()["status"].as_str().unwrap().to_string();
    assert!(status.contains("-45.0"), "the status shows the angle, got {status}");
    let (min, max) = ed.selection_bounds();
    let half = 96.0 / 2f64.sqrt();
    assert!(
        approx(&[min[0], max[0], min[2], max[2]], &[96.0 - half, 96.0 + half, 32.0 - half, 32.0 + half]),
        "the brush turned 45 degrees about its center {min:?} {max:?}"
    );
}

#[test]
#[ignore]
fn a_release_with_only_one_axis_is_refused_and_keeps_the_drag() {
    let ed = Editor::launch("held_half_release");
    let drag = json!({ "type": "drag", "world": [-96, 0, -32], "to_world": [32, 0, 64], "release": false });
    assert_eq!(ed.call("simulate_input", json!({ "target": "top", "events": [drag] }))["held"], true);
    for half in [json!({ "x": 40 }), json!({ "y": 70 })] {
        let mut release = json!({ "type": "release" });
        release.as_object_mut().unwrap().extend(half.as_object().unwrap().clone());
        let e = ed.call_err("simulate_input", json!({ "target": "top", "events": [release] }));
        assert!(e.contains("both x and y"), "{e}");
    }

    let (min, max) = ed.selection_bounds();
    assert!(approx(&min, &[-96.0, 0.0, -32.0]) && approx(&max, &[32.0, 128.0, 64.0]), "the refused releases moved nothing {min:?} {max:?}");
    let reply = ed.call("simulate_input", json!({ "target": "top", "events": [{ "type": "release" }] }));
    assert_eq!(reply["held"], false, "the drag was still held for the real release");
}

#[test]
#[ignore]
fn new_map_and_close_tab_let_go_of_a_held_drag() {
    let ed = Editor::launch("held_drag_new_map");
    let hold = || {
        let drag = json!({ "type": "drag", "world": [-96, 0, -32], "to_world": [32, 0, 64], "release": false });
        assert_eq!(ed.call("simulate_input", json!({ "target": "top", "events": [drag] }))["held"], true);
    };
    let release = json!({ "target": "top", "events": [{ "type": "release" }] });

    hold();
    ed.call("run_action", json!({ "action": "new_map" }));
    assert!(ed.call_err("simulate_input", release.clone()).contains("no drag is held"));
    ed.input("top", json!([{ "type": "move", "world": [128, 0, 96] }]));
    assert_eq!(ed.brushes(), 0, "the new map draws nothing from the old drag");

    // The held drag drew a brush, so closing the tab without discard is refused and keeps the drag.
    hold();
    assert!(ed.call_err("run_action", json!({ "action": "close_tab" })).contains("unsaved"));
    assert_eq!(ed.call("simulate_input", release.clone())["held"], false, "a refused close_tab keeps the drag");

    hold();
    ed.call("run_action", json!({ "action": "close_tab", "args": { "discard": true } }));
    assert!(ed.call_err("simulate_input", release).contains("no drag is held"));
}

#[test]
#[ignore]
fn draw_select_move_delete_undo() {
    let ed = Editor::launch("basic");
    // Draw a brush by dragging in the top view. The first brush is 128 units tall, a room once hollowed.
    ed.input("top", json!([{ "type": "drag", "world": [-64, 0, -64], "to_world": [64, 0, 32] }]));
    assert_eq!(ed.brushes(), 1);
    let (min, max) = ed.selection_bounds();
    assert!(approx(&min, &[-64.0, 0.0, -64.0]) && approx(&max, &[64.0, 128.0, 32.0]), "{min:?} {max:?}");

    // Click empty space deselects, clicking the brush selects it again.
    ed.input("top", json!([{ "type": "click", "world": [200, 0, 120] }]));
    assert!(ed.state()["selection"]["nodes"].as_array().unwrap().is_empty());
    ed.input("front", json!([{ "type": "click", "world": [0, 32, 0] }]));
    assert_eq!(ed.state()["selection"]["nodes"].as_array().unwrap().len(), 1);

    // Move by dragging the selection in the front view.
    ed.input("front", json!([{ "type": "drag", "world": [0, 32, 0], "to_world": [96, 32, 0] }]));
    let (min, _) = ed.selection_bounds();
    assert!(approx(&min, &[32.0, 0.0, -64.0]), "moved min {min:?}");

    // Ctrl+drag duplicates.
    ed.input("front", json!([{ "type": "drag", "world": [96, 32, 0], "to_world": [96, 128, 0], "modifiers": ["ctrl"] }]));
    assert_eq!(ed.brushes(), 2);

    // Delete key removes the selection, Ctrl+Z brings it back.
    ed.input("top", json!([{ "type": "move", "world": [0, 0, 0] }, { "type": "key", "key": "Delete" }]));
    assert_eq!(ed.brushes(), 1);
    ed.input("window", json!([{ "type": "key", "key": "Z", "modifiers": ["ctrl"] }]));
    assert_eq!(ed.brushes(), 2);

    let (w, h, colors) = ed.screenshot("window", "final");
    assert!(w > 800 && h > 500 && colors > 20);
}

#[test]
#[ignore]
fn resizing_puts_an_off_grid_face_on_the_grid() {
    let ed = Editor::launch("resize_onto_grid");
    ed.box_brush([-30.0, 0.0, -32.0], [30.0, 64.0, 32.0]);
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 1.5 }));
    ed.input("top", json!([{ "type": "drag", "world": [30, 0, 0], "to_world": [70, 0, 0] }]));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0]], &[-30.0, 64.0]), "the dragged face lands on the grid line, the other keeps its offset {min:?} {max:?}");
}

#[test]
#[ignore]
fn a_new_2d_brush_can_take_the_depth_of_the_selection() {
    let ed = Editor::launch("draw_depth_from_selection");
    ed.call("set_camera", json!({ "view": "top", "center": [96, 0, 32], "zoom": 1.0 }));
    ed.box_brush([0.0, 16.0, 0.0], [64.0, 32.0, 64.0]);
    ed.input("top", json!([{ "type": "drag", "world": [128, 0, 0], "to_world": [184, 0, 56] }]));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&min, &[128.0, 16.0, 0.0]) && approx(&max, &[192.0, 32.0, 64.0]), "on by default, the new brush spans the floor's height {min:?} {max:?}");

    ed.call("set_editor", json!({ "draw_depth_from_selection": false }));
    ed.box_brush([0.0, 48.0, 0.0], [64.0, 96.0, 64.0]);
    ed.input("top", json!([{ "type": "drag", "world": [128, 0, 80], "to_world": [184, 0, 136] }]));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[1], max[1]], &[16.0, 32.0]), "turned off, the last drawn brush's depth wins over the selection {min:?} {max:?}");
}

#[test]
#[ignore]
fn resize_edges_and_faces() {
    let ed = Editor::launch("resize");
    ed.box_brush([-32.0, 0.0, -32.0], [32.0, 64.0, 32.0]);
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 1.5 }));
    // Drag the right edge (+X) in the top view.
    ed.input("top", json!([{ "type": "drag", "world": [32, 0, 0], "to_world": [96, 0, 0] }]));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&min, &[-32.0, 0.0, -32.0]) && approx(&max, &[96.0, 64.0, 32.0]), "edge resize {min:?} {max:?}");

    // The grabbed edge moves in every view, whichever way its axes point on screen.
    ed.input("top", json!([{ "type": "drag", "world": [32, 0, -32], "to_world": [32, 0, -64] }]));
    ed.call("set_camera", json!({ "view": "front", "center": [32, 32, 0], "zoom": 1.5 }));
    ed.input("front", json!([{ "type": "drag", "world": [32, 64, 0], "to_world": [32, 32, 0] }]));
    ed.input("front", json!([{ "type": "drag", "world": [32, 0, 0], "to_world": [32, 16, 0] }]));
    ed.call("set_camera", json!({ "view": "side", "center": [0, 24, 0], "zoom": 1.5 }));
    ed.input("side", json!([{ "type": "drag", "world": [0, 24, 32], "to_world": [0, 24, 64] }]));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&min, &[-32.0, 16.0, -64.0]) && approx(&max, &[96.0, 32.0, 64.0]), "edge resize in all views {min:?} {max:?}");

    // Shift+drag the top face upwards in the 3D view.
    ed.call("set_camera", json!({ "view": "3d", "position": [-200, 250, 220], "look_at": [32, 32, 0] }));
    ed.input("3d", json!([{ "type": "drag", "world": [32, 64, 0], "to_world": [32, 128, 0], "modifiers": ["shift"], "steps": 12 }]));
    let (_, max) = ed.selection_bounds();
    assert!(max[1] > 64.0, "face drag should raise the top, got {max:?}");
    ed.screenshot("3d", "after_face_drag");
}

#[test]
#[ignore]
fn a_slanted_side_resizes_in_2d_and_a_selected_face_drags_alone() {
    let ed = Editor::launch("slanted_side_drag");
    let wedge = ed.call("create_brush", json!({ "shape": "wedge", "min": [0, 0, 0], "max": [64, 64, 128] }))["ids"][0].as_u64().unwrap();
    ed.call("set_camera", json!({ "view": "side", "center": [32, 48, 64], "zoom": 1.5 }));
    let bounds = || {
        let nodes = ed.call("list_nodes", json!({ "type": "brush" }))["nodes"].clone();
        let b = &nodes[0]["bounds"];
        let v = |k: &str| b[k].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect::<Vec<_>>();
        (nodes.as_array().unwrap().len(), v("min"), v("max"))
    };
    let undo = || ed.call("run_action", json!({ "action": "undo" }));
    let drag = json!([{ "type": "drag", "world": [32, 32, 64], "to_world": [32, 48, 72] }]);
    // The slanted side lies on 2y + z = 128, the drag reaches 17.9 along its normal and snaps to 16.
    let resized = (1, vec![0.0, 0.0, 0.0], vec![64.0, 128.0 / 2.0 + 8.0 * 5f64.sqrt(), 128.0 + 16.0 * 5f64.sqrt()]);
    let same = |a: &(usize, Vec<f64>, Vec<f64>), b: &(usize, Vec<f64>, Vec<f64>)| a.0 == b.0 && approx(&a.1, &b.1) && approx(&a.2, &b.2);

    ed.call("select", json!({ "ids": [wedge] }));
    let undo_before = ed.state()["undo"].as_array().unwrap().len();
    ed.input("side", drag.clone());
    let state = ed.state();
    assert!(same(&bounds(), &resized), "{:?}", bounds());
    assert!(state["status"].as_str().unwrap().starts_with("Resize 16.000"), "{}", state["status"]);
    assert_eq!(state["undo"][0], "Resize Brushes");
    assert_eq!(state["undo"].as_array().unwrap().len(), undo_before + 1);

    undo();
    ed.call("set_editor", json!({ "keymap": "hammer" }));
    ed.input("side", drag.clone());
    assert!(same(&bounds(), &(1, vec![0.0, 16.0, 16.0], vec![64.0, 80.0, 144.0])), "Hammer moves by the slanted outline {:?}", bounds());

    undo();
    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    let faces = ed.call("texture", json!({ "op": "get", "faces": (0..5).map(|f| json!([wedge, f])).collect::<Vec<_>>() }));
    let slanted = faces["faces"].as_array().unwrap().iter().find(|f| f["normal"][1].as_f64().unwrap() > 0.1 && f["normal"][2].as_f64().unwrap() > 0.1).unwrap()
        ["face"][1]
        .as_u64()
        .unwrap();
    ed.call("select", json!({ "faces": [[wedge, slanted]] }));
    ed.input("side", drag.clone());
    assert!(same(&bounds(), &resized), "a selected face drags on its own {:?}", bounds());
    let selection = &ed.state()["selection"];
    assert_eq!(selection["faces"], json!([[wedge, slanted]]));
    assert_eq!(selection["nodes"], json!([]));

    undo();
    ed.call("select", json!({ "ids": [wedge] }));
    ed.input("side", json!([{ "type": "drag", "world": [32, 32, 64], "to_world": [32, -64, -128], "steps": 12 }]));
    let (count, _, max) = bounds();
    assert_eq!(count, 1);
    assert!(max[1] > 0.0 && max[1] < 64.0 && max[2] > 0.0 && max[2] < 128.0, "past the collapse the wedge keeps its last valid shape {max:?}");
}

/// Ctrl on a side copies the selection instead of resizing it, and so does Shift in the Hammer preset. TrenchBroom and
/// Blender keep Shift for the resize.
#[test]
#[ignore]
fn ctrl_or_hammer_shift_on_a_side_copies_instead_of_resizing() {
    let ed = Editor::launch("side_drag_copies");
    let id = ed.box_brush([-40.0, 8.0, -24.0], [88.0, 72.0, 40.0]);
    ed.call("set_editor", json!({ "grid": 16, "snap": true }));
    ed.call("set_camera", json!({ "view": "front", "center": [24, 40, 0], "zoom": 1.5 }));
    let bounds = || -> Vec<[[i64; 3]; 2]> {
        let nodes = ed.call("list_nodes", json!({ "type": "brush" }));
        let mut all: Vec<_> = nodes["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| ["min", "max"].map(|end| [0, 1, 2].map(|i| n["bounds"][end][i].as_f64().unwrap().round() as i64)))
            .collect();
        all.sort();
        all
    };
    let home = [[-40, 8, -24], [88, 72, 40]];
    let resized = [[-40, 8, -24], [128, 72, 40]];
    let try_drag = |keymap: &str, mods: &[&str]| {
        ed.call("set_editor", json!({ "keymap": keymap }));
        ed.call("select", json!({ "ids": [id] }));
        ed.input("front", json!([{ "type": "drag", "world": [85, 40, 0], "to_world": [121, 40, 0], "modifiers": mods }]));
        let after = bounds();
        let top = ed.state()["undo"][0].as_str().unwrap().to_string();
        ed.call("run_action", json!({ "action": "undo" }));
        assert_eq!(bounds(), vec![home], "undo restores the single brush after {keymap} {mods:?}");
        (after, top)
    };
    let expect_resize = |keymap: &str, mods: &[&str]| {
        let (after, top) = try_drag(keymap, mods);
        assert_eq!(after, vec![resized], "{keymap} {mods:?} resizes the side");
        assert_eq!(top, "Resize Brushes", "{keymap} {mods:?}");
    };
    let expect_copy = |keymap: &str, mods: &[&str]| {
        let (after, top) = try_drag(keymap, mods);
        assert_eq!(after.len(), 2, "{keymap} {mods:?} adds a brush {after:?}");
        let copy = after.iter().find(|b| **b != home).unwrap_or_else(|| panic!("{keymap} {mods:?} left only the original {after:?}"));
        assert!(after.contains(&home), "{keymap} {mods:?} leaves the original at max x 88 {after:?}");
        let shift = copy[0][0] - home[0][0];
        assert!(shift > 0 && shift % 16 == 0, "{keymap} {mods:?} moves the copy along +X in grid steps {after:?}");
        assert_eq!(*copy, [[home[0][0] + shift, 8, -24], [home[1][0] + shift, 72, 40]], "{keymap} {mods:?} keeps the size");
        assert_eq!(top, "Duplicate Objects", "{keymap} {mods:?}");
    };

    expect_resize("trenchbroom", &[]);
    expect_resize("trenchbroom", &["shift"]);
    expect_copy("trenchbroom", &["ctrl"]);
    expect_copy("trenchbroom", &["ctrl", "shift"]);
    expect_resize("hammer", &[]);
    expect_copy("hammer", &["shift"]);
    expect_copy("hammer", &["ctrl"]);
    expect_resize("blender", &[]);
    expect_copy("blender", &["ctrl"]);
}

/// E in the Blender preset grows a new brush out of each selected brush face, by a typed or moved length. Esc and a
/// right click put everything back, a length of zero or less makes nothing, and TrenchBroom leaves E unbound.
#[test]
#[ignore]
fn blender_e_extrudes_selected_faces_into_new_brushes() {
    let ed = Editor::launch("face_extrude");
    let b = ed.box_brush([-56.0, 8.0, -24.0], [40.0, 72.0, 88.0]);
    let w = ed.call("create_brush", json!({ "shape": "wedge", "min": [-40, 8, 24], "max": [24, 72, 152] }))["ids"][0].as_u64().unwrap();
    ed.call("set_editor", json!({ "keymap": "blender", "grid": 16, "snap": true }));
    ed.call("set_camera", json!({ "view": "front", "center": [0, 96, 0], "zoom": 1 }));
    let brushes = || -> Vec<(u64, [[i64; 3]; 2])> {
        let nodes = ed.call("list_nodes", json!({ "type": "brush" }));
        let mut all: Vec<_> = nodes["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| (n["id"].as_u64().unwrap(), ["min", "max"].map(|end| [0, 1, 2].map(|i| n["bounds"][end][i].as_f64().unwrap().round() as i64))))
            .collect();
        all.sort();
        all
    };
    let face_toward = |id: u64, count: usize, normal: [f64; 3]| -> u64 {
        let faces = ed.call("texture", json!({ "op": "get", "faces": (0..count).map(|f| json!([id, f])).collect::<Vec<_>>() }));
        let len = normal.iter().map(|v| v * v).sum::<f64>().sqrt();
        let along = |f: &Value| (0..3).map(|i| f["normal"][i].as_f64().unwrap() * normal[i] / len).sum::<f64>();
        faces["faces"].as_array().unwrap().iter().find(|f| along(f) > 0.999).unwrap()["face"][1].as_u64().unwrap()
    };
    let undo_len = || ed.state()["undo"].as_array().unwrap().len();
    let keys = |events: Value| {
        let mut all = vec![json!({ "type": "move", "world": [0, 110, 0] })];
        all.extend(events.as_array().unwrap().iter().cloned());
        ed.input("front", Value::Array(all));
    };
    let extrude = |length: &str| keys(json!([{ "type": "key", "key": "E" }, { "type": "text", "text": length }, { "type": "key", "key": "Enter" }]));
    let top = face_toward(b, 6, [0.0, 1.0, 0.0]);
    let slanted = face_toward(w, 5, [0.0, 2.0, 1.0]);
    let home = brushes();

    keys(json!([{ "type": "key", "key": "E" }]));
    assert_eq!(brushes(), home);
    assert_eq!(ed.state()["status"], "Select brush faces to extrude");

    ed.call("select", json!({ "faces": [[b, top]] }));
    let undo_before = undo_len();
    extrude("32");
    let after = brushes();
    assert_eq!(after.len(), 3, "E, 32 and Enter add a brush {after:?}");
    let first = after.iter().find(|n| !home.contains(n)).unwrap();
    assert_eq!(first.1, [[-56, 72, -24], [40, 104, 88]]);
    let state = ed.state();
    assert_eq!(state["selection"]["nodes"], json!([]));
    let selected = state["selection"]["faces"].as_array().unwrap().clone();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0][0], first.0, "the new brush's far face is selected");
    assert_eq!(state["editor"]["select_mode"], "face");
    assert_eq!(state["undo"][0], "Extrude");
    assert_eq!(undo_len(), undo_before + 1);

    extrude("16");
    let chained = brushes();
    let second = *chained.iter().find(|n| !after.contains(n)).unwrap();
    assert_eq!(second.1, [[-56, 104, -24], [40, 120, 88]], "a second E extrudes the far face again {chained:?}");
    let far = ed.state()["selection"]["faces"].clone();
    assert_eq!(far[0][0], second.0);

    for (cancel, how) in
        [(json!({ "type": "key", "key": "Escape" }), "Esc"), (json!({ "type": "click", "world": [0, 146, 0], "button": "right" }), "right click")]
    {
        keys(json!([{ "type": "key", "key": "E" }, { "type": "move", "world": [0, 146, 0] }]));
        let state = ed.state();
        assert!(state["status"].as_str().unwrap().starts_with("Extrude 32.000"), "{how}: {}", state["status"]);
        assert_eq!(brushes().len(), chained.len() + 1, "{how}: the extrude shows while the mouse moves");
        ed.input("front", json!([cancel]));
        let state = ed.state();
        assert_eq!(brushes(), chained, "{how} puts the map back");
        assert_eq!(state["selection"]["faces"], far, "{how} puts the far face selection back");
        assert_eq!(state["status"], "Cancelled", "{how}");
        assert_eq!(undo_len(), undo_before + 2, "{how} leaves no undo step");
    }

    keys(json!([{ "type": "key", "key": "E" }, { "type": "key", "key": "Enter" }]));
    extrude("-16");
    extrude("0");
    assert_eq!(brushes(), chained, "a length of zero or less makes nothing");
    assert_eq!(undo_len(), undo_before + 2);

    ed.call("select", json!({ "faces": [[b, top], [w, slanted]] }));
    let undo_two = undo_len();
    extrude("16");
    let both = brushes();
    assert_eq!(both.len(), chained.len() + 2, "one brush per face {both:?}");
    assert!(both.iter().any(|n| n.1 == [[-56, 72, -24], [40, 88, 88]]), "{both:?}");
    assert_eq!(undo_len(), undo_two + 1, "both brushes are one undo step");
    assert_eq!(ed.state()["selection"]["faces"].as_array().unwrap().len(), 2);

    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    ed.call("select", json!({ "faces": [[b, top]] }));
    let (undo_tb, status) = (undo_len(), ed.state()["status"].clone());
    keys(json!([{ "type": "key", "key": "E" }, { "type": "move", "world": [0, 146, 0] }]));
    assert_eq!(brushes(), both, "TrenchBroom leaves E unbound");
    assert_eq!(ed.state()["status"], status);
    assert_eq!(undo_len(), undo_tb);
}

fn selected(ed: &Editor) -> Vec<u64> {
    ed.state()["selection"]["nodes"].as_array().unwrap().iter().map(|n| n.as_u64().unwrap()).collect()
}

#[test]
#[ignore]
fn the_shape_generator_shows_its_own_box() {
    let ed = Editor::launch("shape_box");
    let a = ed.box_brush([-37.0, 5.0, 22.0], [51.0, 70.0, 99.0]);
    ed.call("run_action", json!({ "action": "select_none" }));
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 1 }));
    let bounds = |ed: &Editor| ed.state()["ui"]["shape_bounds"].clone();
    assert_eq!(bounds(&ed), Value::Null, "no box while the dialog is closed");

    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "B", "modifiers": ["ctrl", "shift"] }]));
    ed.input("top", json!([{ "type": "move" }]));
    let want = json!({ "min": [-32.0, 0.0, 16.0], "max": [48.0, 64.0, 96.0] });
    assert_eq!(bounds(&ed), want, "the box starts on the grid lines nearest the last brush");
    ed.screenshot("top", "top");
    ed.screenshot("3d", "3d");

    ed.call("select", json!({ "ids": [a] }));
    ed.input("top", json!([{ "type": "move" }]));
    assert_eq!(bounds(&ed), Value::Null, "a selected brush takes over");

    ed.call("run_action", json!({ "action": "select_none" }));
    ed.input("top", json!([{ "type": "move" }]));
    assert_eq!(bounds(&ed), want, "deselecting brings the same box back");
    assert_eq!(ed.brushes(), 1, "the box is not part of the map");
    assert_eq!(selected(&ed), Vec::<u64>::new());
}

/// Where a world point lands in a 2D view, in window points.
fn screen_of(ed: &Editor, view: &str, world: [f64; 3]) -> (f64, f64) {
    let state = ed.state();
    let cam = state["cameras"].as_array().unwrap().iter().find(|c| c["view"] == view).expect("view open");
    let n = |v: &Value| v.as_f64().unwrap();
    let (rect, zoom) = (&cam["rect"], n(&cam["zoom"]));
    let at = |i: usize| n(&cam["center"][i]);
    let (dx, dy) = match view {
        "top" => (world[0] - at(0), world[2] - at(2)),
        "front" => (world[0] - at(0), at(1) - world[1]),
        _ => (at(2) - world[2], at(1) - world[1]),
    };
    (n(&rect[0]) + n(&rect[2]) / 2.0 + dx * zoom, n(&rect[1]) + n(&rect[3]) / 2.0 + dy * zoom)
}

/// Asserts that a press at each world point lands on the view and not on the Shape Generator's window.
fn assert_clear_of_the_dialog(ed: &Editor, view: &str, points: &[[f64; 3]]) {
    let state = ed.state();
    let r: Vec<f64> = state["ui"]["windows"].as_array().unwrap().iter().find(|w| w["title"] == "Shape Generator").expect("Shape Generator open")["rect"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    for p in points {
        let (x, y) = screen_of(ed, view, *p);
        assert!(x < r[0] - 4.0 || x > r[2] + 4.0 || y < r[1] - 4.0 || y > r[3] + 4.0, "{p:?} at ({x}, {y}) is under the dialog {r:?}");
    }
}

#[test]
#[ignore]
fn the_shape_generator_box_is_drawn_and_resized_in_2d() {
    let ed = Editor::launch("shape_box_2d");
    let a = ed.box_brush([-37.0, 5.0, 22.0], [51.0, 70.0, 99.0]);
    ed.call("run_action", json!({ "action": "select_none" }));
    ed.call("set_camera", json!({ "view": "top", "center": [-40, 0, 0], "zoom": 1 }));
    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "B", "modifiers": ["ctrl", "shift"] }]));
    ed.input("top", json!([{ "type": "move" }]));
    let bounds = |ed: &Editor| ed.state()["ui"]["shape_bounds"].clone();
    let box_of = |min: [f64; 3], max: [f64; 3]| json!({ "min": min, "max": max });
    assert_eq!(bounds(&ed), box_of([-32.0, 0.0, 16.0], [48.0, 64.0, 96.0]), "the seeded box");
    let undo = ed.state()["undo"].clone();
    let drag = |ed: &Editor, view: &str, from: [f64; 3], to: [f64; 3]| {
        assert_clear_of_the_dialog(ed, view, &[from]);
        ed.input(view, json!([{ "type": "drag", "world": from, "to_world": to }]));
    };

    drag(&ed, "top", [-100.0, 0.0, -60.0], [20.0, 0.0, 40.0]);
    assert_eq!(bounds(&ed), box_of([-112.0, 0.0, -64.0], [32.0, 64.0, 48.0]), "drawn over the cells under both ends, keeping the depth");
    assert_eq!(ed.brushes(), 1, "no brush was made");
    assert_eq!(selected(&ed), Vec::<u64>::new());
    assert_eq!(ed.state()["undo"], undo, "the box is not an undo step");

    drag(&ed, "top", [32.0, 0.0, 0.0], [77.0, 0.0, 0.0]);
    assert_eq!(bounds(&ed), box_of([-112.0, 0.0, -64.0], [80.0, 64.0, 48.0]), "the +X side follows the pointer on the grid");

    drag(&ed, "top", [-112.0, 0.0, 0.0], [200.0, 0.0, 0.0]);
    assert_eq!(bounds(&ed), box_of([64.0, 0.0, -64.0], [80.0, 64.0, 48.0]), "the -X side stops one cell short of the other");

    ed.call("set_camera", json!({ "view": "front", "center": [0, 50, 0], "zoom": 1 }));
    drag(&ed, "front", [0.0, 100.0, 0.0], [40.0, 30.0, 0.0]);
    assert_eq!(bounds(&ed), box_of([0.0, 16.0, -64.0], [48.0, 112.0, 48.0]), "a front drag keeps the depth from the top drag");
    assert_eq!((ed.brushes(), ed.state()["undo"].clone()), (1, undo), "still nothing in the map");

    // Selecting a brush hands the dialog over to it, which ends a box drag in flight for good.
    ed.call("set_camera", json!({ "view": "top", "center": [-40, 0, 0], "zoom": 1 }));
    let before = bounds(&ed);
    assert_clear_of_the_dialog(&ed, "top", &[[-100.0, 0.0, -60.0]]);
    ed.input("top", json!([{ "type": "drag", "world": [-100, 0, -60], "to_world": [20, 0, 40], "release": false }]));
    let held = bounds(&ed);
    assert_ne!(held, before, "the drag was drawing");
    ed.call("select", json!({ "ids": [a] }));
    ed.input("top", json!([{ "type": "move", "world": [90, 0, 70] }, { "type": "move", "world": [100, 0, 80] }]));
    assert_eq!(bounds(&ed), Value::Null, "a selected brush takes over");
    ed.call("run_action", json!({ "action": "select_none" }));
    ed.input("top", json!([{ "type": "move", "world": [110, 0, 90] }, { "type": "move", "world": [120, 0, 100] }, { "type": "release" }]));
    assert_eq!(bounds(&ed), held, "the box comes back, the drag does not");
    assert_eq!(ed.brushes(), 1);

    // A selected light on a side of the box moves, the box stays.
    let light = ed.call("create_entity", json!({ "classname": "light", "origin": [32, 64, 0] }))["id"].as_u64().unwrap();
    assert_eq!(selected(&ed), [light]);
    let (min, max) = ed.selection_bounds();
    drag(&ed, "top", [32.0, 0.0, 0.0], [64.0, 0.0, 32.0]);
    let moved = |v: &[f64]| vec![v[0] + 32.0, v[1], v[2] + 32.0];
    assert_eq!(ed.selection_bounds(), (moved(&min), moved(&max)), "the light follows the pointer");
    assert_eq!(bounds(&ed), held, "the side under the light is left alone");

    ed.screenshot("top", "top");

    // The 3D view still draws real brushes, which the dialog then replaces.
    ed.call("set_camera", json!({ "view": "3d", "position": [0, 300, 300], "look_at": [0, 0, 0] }));
    ed.input("3d", json!([{ "type": "drag", "world": [150, 0, 100], "to_world": [250, 0, 160], "steps": 8 }]));
    ed.input("top", json!([{ "type": "move" }]));
    assert_eq!(ed.brushes(), 2, "a 3D drag makes a brush");
    assert_eq!(selected(&ed).len(), 1);
    assert_eq!(bounds(&ed), Value::Null);
    ed.call("set_camera", json!({ "view": "top", "center": [-40, 0, 0], "zoom": 1 }));
    ed.input("top", json!([{ "type": "drag", "world": [-100, 0, -60], "to_world": [-60, 0, -20] }]));
    assert_eq!(ed.brushes(), 3, "with a brush selected a 2D drag draws a brush as before");
}

#[test]
#[ignore]
fn keys_stay_with_windows_menus_and_the_palette() {
    let ed = Editor::launch("key_scope");
    let a = ed.box_brush([-64.0, 0.0, -64.0], [64.0, 32.0, 64.0]);
    ed.box_brush([128.0, 0.0, -64.0], [192.0, 32.0, 64.0]);
    ed.call("select", json!({ "ids": [a] }));
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 0.5 }));

    // With the pointer over a floating window, keys meant for it must not delete, convert or reselect anything.
    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "B", "modifiers": ["ctrl", "shift"] }]));
    // A new window is laid out on its first frame and shown on the next, and a centered one settles on the one after.
    ed.input("top", json!([{ "type": "move" }]));
    ed.input("top", json!([{ "type": "move" }]));
    let state = ed.state();
    let window = state["ui"]["windows"].as_array().unwrap().iter().find(|w| w["title"] == "Shape Generator").expect("Shape Generator open")["rect"].clone();
    let r: Vec<f64> = window.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let keys = ["Delete", "Backspace", "Tab", "Escape"].map(|k| json!({ "type": "key", "key": k }));
    let mut events = vec![json!({ "type": "move", "x": (r[0] + r[2]) / 2.0, "y": (r[1] + r[3]) / 2.0 })];
    events.extend(keys);
    events.push(json!({ "type": "key", "key": "A", "modifiers": ["ctrl"] }));
    ed.input("window", Value::Array(events));
    let state = ed.state();
    assert_eq!((state["map"]["brushes"].as_u64(), state["map"]["meshes"].as_u64()), (Some(2), Some(0)), "nothing deleted or converted");
    assert_eq!(state["editor"]["tool"], "Select");
    assert_eq!(selected(&ed), vec![a], "the selection is untouched");

    // Escape or a click outside closes the command palette without running anything or reaching the view.
    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "F1" }, { "type": "text", "text": "grid larger" }, { "type": "key", "key": "Escape" }]));
    assert_eq!(selected(&ed), vec![a], "Escape only closed the palette");
    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "F1" }]));
    ed.call("set_camera", json!({ "view": "front", "center": [0, 0, 0], "zoom": 0.5 }));
    ed.input("front", json!([{ "type": "click", "world": [-300, 100, 0] }]));
    assert_eq!(selected(&ed), vec![a], "the click outside only closed the palette");
    assert_eq!(ed.state()["editor"]["grid"], 16.0);
    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "CloseBracket" }]));
    assert_eq!(ed.state()["editor"]["grid"], 32.0, "keys reach the map again");

    // Dismissing a context menu keeps the selection.
    ed.input("top", json!([{ "type": "click", "world": [0, 0, 0], "button": "right" }, { "type": "key", "key": "Escape" }]));
    assert_eq!(selected(&ed), vec![a], "Escape only closed the menu");
    ed.input("top", json!([{ "type": "click", "world": [0, 0, 0], "button": "right" }]));
    ed.input("top", json!([{ "type": "click", "world": [-300, 0, 150] }]));
    assert_eq!(selected(&ed), vec![a], "the click outside only closed the menu");
    ed.input("top", json!([{ "type": "click", "world": [-300, 0, 150] }]));
    assert!(selected(&ed).is_empty(), "a click on empty space deselects again");

    // Tab edits brushes in the Vertex tool and never converts them.
    ed.call("select", json!({ "ids": [a] }));
    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "Tab" }]));
    let state = ed.state();
    assert_eq!((state["editor"]["tool"].as_str(), state["map"]["meshes"].as_u64()), (Some("Vertex"), Some(0)));
    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "Tab" }]));
    assert_eq!(ed.state()["editor"]["tool"], "Select");
}

#[test]
#[ignore]
fn number_fields_in_the_tool_options_bar_take_ctrl_a() {
    let ed = Editor::launch("bar_ctrl_a");
    let a = ed.box_brush([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]);
    ed.box_brush([128.0, 0.0, 0.0], [192.0, 64.0, 64.0]);
    ed.call("select", json!({ "ids": [a] }));
    ed.call("run_action", json!({ "action": "set_tool", "args": { "tool": "sculpt" } }));
    ed.input("window", json!([{ "type": "move", "x": 10, "y": 10 }]));
    let bar: Vec<f64> = ed.state()["ui"]["tool_options"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    // The radius field, after the tool name and the mode drop down.
    let (x, y) = (bar[0] + 241.0, (bar[1] + bar[3]) / 2.0);
    for events in [
        json!([{ "type": "click", "x": x, "y": y }, { "type": "key", "key": "A", "modifiers": ["ctrl"] }, { "type": "text", "text": "150" }, { "type": "key", "key": "Enter" }]),
        json!([{ "type": "double_click", "x": x, "y": y }, { "type": "key", "key": "A", "modifiers": ["ctrl"] }, { "type": "text", "text": "96" }, { "type": "key", "key": "Enter" }]),
    ] {
        let typed = events[2]["text"].as_str().unwrap().parse::<f64>().unwrap();
        ed.input("window", events);
        let state = ed.state();
        assert_eq!(state["editor"]["sculpt"]["radius"], typed, "Ctrl+A selected the field's text, so the typed value replaced it");
        assert_eq!(selected(&ed), vec![a], "not Select All");
    }
}

#[test]
#[ignore]
fn alt_orbit_click_cycling_and_edge_grab() {
    let ed = Editor::launch("picking");
    let floor = ed.box_brush([-64.0, 0.0, -64.0], [64.0, 16.0, 64.0]);
    let ceiling = ed.box_brush([-64.0, 112.0, -64.0], [64.0, 128.0, 64.0]);
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 1.0 }));

    // In the Top view the ceiling covers the floor, clicking again soon at the same spot or Alt+click reaches it. The
    // pauses keep separate clicks from reading as a double click, or later ones from cycling.
    let pause = |seconds: f64| std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
    ed.input("top", json!([{ "type": "click", "world": [0, 0, 0] }]));
    let first = selected(&ed);
    pause(0.5);
    ed.input("top", json!([{ "type": "click", "world": [0, 0, 0] }]));
    let second = selected(&ed);
    assert!(first.len() == 1 && second.len() == 1 && first != second, "{first:?} then {second:?}");
    assert!([floor, ceiling].contains(&first[0]) && [floor, ceiling].contains(&second[0]));
    pause(0.5);
    ed.input("top", json!([{ "type": "click", "world": [0, 0, 0], "modifiers": ["alt"] }]));
    assert_eq!(selected(&ed), first, "Alt+click steps on and wraps around");
    pause(2.0);
    ed.input("top", json!([{ "type": "click", "world": [0, 0, 0] }]));
    assert_eq!(selected(&ed), first, "a click long after the last one picks the nearest object again");
    pause(2.0);
    ed.input("top", json!([{ "type": "double_click", "world": [0, 0, 0] }]));
    assert_eq!(selected(&ed), first, "the second click of a double click does not cycle");

    // A drag a few pixels outside the edge still resizes instead of drawing a new brush.
    ed.call("select", json!({ "ids": [floor] }));
    ed.input("top", json!([{ "type": "drag", "world": [70, 0, 0], "to_world": [102, 0, 0] }]));
    assert_eq!(ed.brushes(), 2, "no stray brush");
    let (_, max) = ed.selection_bounds();
    assert!(approx(&max, &[96.0, 16.0, 64.0]), "edge grabbed from 6 pixels off {max:?}");

    // Alt+drag in 3D orbits unless it starts on the selection, which then moves vertically.
    ed.call("select", json!({ "ids": [ceiling] }));
    ed.call("set_camera", json!({ "view": "3d", "position": [-300, 250, 300], "look_at": [0, 64, 0] }));
    let camera = |ed: &Editor| ed.state()["cameras"].as_array().unwrap().iter().find(|c| c["view"] == "3d").unwrap()["position"].clone();
    let before = camera(&ed);
    ed.input("3d", json!([{ "type": "drag", "x": 30, "y": 30, "to": [90, 40], "modifiers": ["alt"] }]));
    assert_ne!(camera(&ed), before, "Alt+drag off the selection orbits");
    let (min, _) = ed.selection_bounds();
    assert!(approx(&min, &[-64.0, 112.0, -64.0]), "the selection stays put {min:?}");
    assert_eq!(ed.brushes(), 2);
    // A wide slab, so the drag can start on it well away from the transform gizmo in its middle.
    let slab = ed.box_brush([-384.0, 112.0, -384.0], [384.0, 128.0, 384.0]);
    ed.call("select", json!({ "ids": [slab] }));
    ed.call("set_camera", json!({ "view": "3d", "position": [-300, 250, 300], "look_at": [0, 64, 0] }));
    ed.input("3d", json!([{ "type": "drag", "world": [-200, 128, 200], "to_world": [-200, 200, 200], "modifiers": ["alt"], "steps": 12 }]));
    let (min, _) = ed.selection_bounds();
    assert!(min[1] > 112.0 && approx(&[min[0], min[2]], &[-384.0, -384.0]), "Alt+drag on the selection moves it up {min:?}");

    // The wheel step follows the wall straight ahead, not the far ground under a pointer off to the side.
    ed.box_brush([-20000.0, -16.0, -20000.0], [20000.0, -8.0, 20000.0]);
    ed.box_brush([1936.0, 0.0, -8.0], [2064.0, 256.0, 8.0]);
    ed.call("set_camera", json!({ "view": "3d", "position": [2000, 128, 400], "look_at": [2000, 128, 0] }));
    ed.input("3d", json!([{ "type": "scroll", "world": [-2000, -8, -8000], "delta": 120 }]));
    pause(0.5);
    let z = camera(&ed)[2].as_f64().unwrap();
    assert!(z > 8.0 && z < 400.0, "moved towards the wall without passing it, z {z}");
}

/// Where to click for the group gesture tests: the top face of each brush and a spot with nothing under it.
struct GroupSpots {
    a: [f64; 3],
    b: [f64; 3],
    c: [f64; 3],
    empty: [f64; 3],
}

fn open_groups(ed: &Editor) -> Value {
    ed.state()["editor"]["open_groups"].clone()
}

fn selected_sorted(ed: &Editor) -> Vec<u64> {
    let mut ids = selected(ed);
    ids.sort();
    ids
}

fn group_gestures_walk(ed: &Editor, view: &str, spots: GroupSpots) {
    let GroupSpots { a, b, c, empty } = spots;
    // A click within 0.3 s of an unpaired click at the same spot makes a double click with it.
    let pause = || std::thread::sleep(std::time::Duration::from_secs_f64(0.7));
    let click = |at: [f64; 3], modifiers: &[&str]| {
        ed.input(view, json!([{ "type": "click", "world": at, "modifiers": modifiers }]));
        pause();
    };
    let double_click = |at: [f64; 3]| {
        ed.input(view, json!([{ "type": "double_click", "world": at }]));
        pause();
    };
    let status = || ed.state()["status"].as_str().unwrap().to_string();
    let brush_a = ed.box_brush([40.0, 0.0, -64.0], [104.0, 32.0, 0.0]);
    let brush_b = ed.box_brush([140.0, 0.0, 24.0], [172.0, 64.0, 88.0]);
    let brush_c = ed.box_brush([-130.0, 0.0, 40.0], [-70.0, 48.0, 112.0]);
    ed.call("select", json!({ "ids": [brush_a, brush_b] }));
    ed.call("run_action", json!({ "action": "group" }));
    let group = selected(ed)[0];
    ed.call("hierarchy", json!({ "op": "close_group", "all": true }));
    ed.call("select", json!({ "ids": [] }));

    // A click on a brush of a closed group selects the group whole and opens nothing.
    click(a, &[]);
    assert_eq!(selected(ed), [group]);
    assert_eq!(open_groups(ed), json!([]));
    assert!(status().starts_with("Selected group"), "{}", status());
    click(b, &[]);
    assert_eq!(selected(ed), [group], "the other brush of the group gives the same group");
    click(c, &["ctrl"]);
    assert_eq!(selected_sorted(ed), [brush_c.min(group), brush_c.max(group)], "Ctrl+click adds the loose brush next to the group");
    click(b, &["ctrl"]);
    assert_eq!(selected(ed), [brush_c], "Ctrl+click on a brush of the selected group removes the whole group");

    // A double click opens the group one level and selects what was clicked.
    double_click(a);
    assert_eq!(open_groups(ed), json!([group]));
    assert_eq!(selected(ed), [brush_a]);
    click(b, &[]);
    assert_eq!(selected(ed), [brush_b], "inside the open group a click picks one brush");
    assert_eq!(open_groups(ed), json!([group]));
    click(c, &["ctrl"]);
    assert_eq!(open_groups(ed), json!([]), "a click outside the open group closes it");
    assert_eq!(selected(ed), [brush_c], "and drops the brush picked inside it");
    assert!(status().starts_with("Closed group"), "{}", status());

    // A double click on empty space closes the innermost open group and clears the selection.
    double_click(a);
    assert_eq!(open_groups(ed), json!([group]));
    double_click(empty);
    assert_eq!(open_groups(ed), json!([]));
    assert_eq!(selected(ed), Vec::<u64>::new());
    double_click(empty);
    assert_eq!(open_groups(ed), json!([]), "with no group open it changes nothing");

    // Esc clears the selection first, the next one closes the open group, and one more changes nothing.
    let escape = || ed.input(view, json!([{ "type": "key", "key": "Escape" }]));
    double_click(a);
    assert_eq!((open_groups(ed), selected(ed)), (json!([group]), vec![brush_a]));
    escape();
    assert_eq!((open_groups(ed), selected(ed)), (json!([group]), Vec::<u64>::new()), "the first Esc clears the selection");
    escape();
    assert_eq!(open_groups(ed), json!([]), "the next Esc closes the group");
    ed.call("select", json!({ "ids": [group] }));
    click(b, &["ctrl"]);
    assert_eq!(selected(ed), Vec::<u64>::new());
    assert!(!status().contains(" group "), "a Ctrl+click that removes the group neither says it selected it nor keeps the old message: {}", status());
    escape();
    assert_eq!(open_groups(ed), json!([]));
    assert_eq!(selected(ed), Vec::<u64>::new());

    // A double click on a loose brush opens nothing.
    double_click(c);
    assert_eq!(open_groups(ed), json!([]));
    assert_eq!(selected(ed), [brush_c]);

    // Nested groups take one double click per level, and the first one selects the inner group.
    ed.call("select", json!({ "ids": [group, brush_c] }));
    ed.call("run_action", json!({ "action": "group" }));
    let outer = selected(ed)[0];
    ed.call("hierarchy", json!({ "op": "close_group", "all": true }));
    ed.call("select", json!({ "ids": [] }));
    click(a, &[]);
    assert_eq!(selected(ed), [outer]);
    double_click(a);
    assert_eq!(open_groups(ed), json!([outer]));
    assert_eq!(selected(ed), [group], "one level opened, the inner group is what a click reaches now");
    double_click(a);
    assert_eq!(open_groups(ed), json!([outer, group]));
    assert_eq!(selected(ed), [brush_a]);
    double_click(empty);
    assert_eq!(open_groups(ed), json!([outer]), "empty space closes one level");
    assert_eq!(selected(ed), Vec::<u64>::new());
    double_click(empty);
    assert_eq!(open_groups(ed), json!([]));

    // The Blender preset selects a closed group whole on a click too.
    ed.call("set_editor", json!({ "keymap": "blender" }));
    click(b, &[]);
    assert_eq!(selected(ed), [outer]);
    assert_eq!(open_groups(ed), json!([]));
    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));

    // The Vertex tool reshapes brushes, so its click picks one brush inside a closed group and opens nothing.
    ed.call("select", json!({ "ids": [] }));
    ed.call("set_editor", json!({ "tool": "vertex" }));
    click(a, &[]);
    assert_eq!(selected(ed), [brush_a]);
    click(b, &["ctrl"]);
    assert_eq!(selected_sorted(ed), [brush_a.min(brush_b), brush_a.max(brush_b)]);
    assert_eq!(open_groups(ed), json!([]));
    ed.call("set_editor", json!({ "tool": "select" }));
}

#[test]
#[ignore]
fn a_group_click_in_top_keeps_the_cycling_hint() {
    let ed = Editor::launch("group_cycle_hint");
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 1.0 }));
    let p = ed.box_brush([40.0, 64.0, -64.0], [104.0, 96.0, 0.0]);
    let q = ed.box_brush([140.0, 0.0, 24.0], [172.0, 64.0, 88.0]);
    let under = ed.box_brush([24.0, -40.0, -80.0], [120.0, -8.0, 16.0]);
    ed.call("select", json!({ "ids": [p, q] }));
    ed.call("run_action", json!({ "action": "group" }));
    let group = selected(&ed)[0];
    ed.call("hierarchy", json!({ "op": "close_group", "all": true }));
    ed.call("select", json!({ "ids": [] }));
    let click = |modifiers: &[&str]| {
        ed.input("top", json!([{ "type": "click", "world": [72.0, 96.0, -32.0], "modifiers": modifiers }]));
        // Two clicks less than 0.3 s apart at one spot make a double click, which opens the group instead.
        std::thread::sleep(std::time::Duration::from_secs_f64(0.7));
        ed.state()["status"].as_str().unwrap().to_string()
    };

    let status = click(&[]);
    assert_eq!(selected(&ed), [group], "the group on top comes first");
    assert!(status.starts_with("Selected group"), "{status}");
    assert!(status.contains("2 objects under the cursor"), "the hint to reach the brush below stays: {status}");
    let status = click(&["alt"]);
    assert_eq!(selected(&ed), [under]);
    assert!(status.starts_with("Object 2 of 2"), "{status}");

    // A click that closes the open group still says so beside the hint.
    let low = ed.box_brush([108.0, -80.0, -40.0], [118.0, -60.0, -20.0]);
    ed.call("hierarchy", json!({ "op": "open_group", "id": group }));
    ed.input("top", json!([{ "type": "click", "world": [112.0, 96.0, -32.0], "modifiers": [] }]));
    let status = ed.state()["status"].as_str().unwrap().to_string();
    assert!([under, low].contains(&selected(&ed)[0]), "{:?}", selected(&ed));
    assert!(status.starts_with("Closed group"), "{status}");
    assert!(status.contains("2 objects under the cursor"), "{status}");
}

#[test]
#[ignore]
fn double_click_opens_a_group_and_empty_space_closes_it_in_top() {
    let ed = Editor::launch("group_gestures_top");
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 1.0 }));
    group_gestures_walk(&ed, "top", GroupSpots { a: [72.0, 32.0, -32.0], b: [156.0, 64.0, 56.0], c: [-100.0, 48.0, 76.0], empty: [0.0, 0.0, -110.0] });
}

#[test]
#[ignore]
fn double_click_opens_a_group_and_empty_space_closes_it_in_3d() {
    let ed = Editor::launch("group_gestures_3d");
    ed.call("set_camera", json!({ "view": "3d", "position": [20, 280, 150], "look_at": [20, 0, 20] }));
    group_gestures_walk(&ed, "3d", GroupSpots { a: [72.0, 32.0, -32.0], b: [156.0, 64.0, 56.0], c: [-100.0, 48.0, 76.0], empty: [-200.0, 0.0, -200.0] });
}

#[test]
#[ignore]
fn double_click_on_the_transform_gizmo_keeps_the_open_group() {
    let ed = Editor::launch("group_gizmo_double_click");
    ed.call("set_camera", json!({ "view": "3d", "position": [0, 200, 250], "look_at": [0, 8, 0] }));
    let small = ed.box_brush([-8.0, 0.0, -8.0], [8.0, 16.0, 8.0]);
    let other = ed.box_brush([-120.0, 0.0, -40.0], [-88.0, 32.0, -8.0]);
    ed.call("select", json!({ "ids": [small, other] }));
    ed.call("run_action", json!({ "action": "group" }));
    let group = selected(&ed)[0];
    ed.call("hierarchy", json!({ "op": "close_group", "all": true }));
    ed.call("select", json!({ "ids": [] }));
    let pause = || std::thread::sleep(std::time::Duration::from_secs_f64(0.7));
    ed.input("3d", json!([{ "type": "double_click", "world": [0.0, 16.0, 0.0] }]));
    pause();
    assert_eq!((open_groups(&ed), selected(&ed)), (json!([group]), vec![small]));

    // The X arrow of the gizmo reaches past the small brush over empty space.
    let arrow = [20.0, 8.0, 0.0];
    ed.input("3d", json!([{ "type": "double_click", "world": arrow }]));
    pause();
    assert_eq!((open_groups(&ed), selected(&ed)), (json!([group]), vec![small]), "a double click on the gizmo neither closes nor opens a group");
    let (before, _) = ed.selection_bounds();
    ed.input("3d", json!([{ "type": "drag", "world": arrow, "to_world": [52.0, 8.0, 0.0], "steps": 10 }]));
    let (after, _) = ed.selection_bounds();
    assert!((after[0] - before[0] - 32.0).abs() < 2.0 && (after[2] - before[2]).abs() < 1e-3, "the spot is on the X arrow, before {before:?} after {after:?}");
}

#[test]
#[ignore]
fn two_quick_double_clicks_open_and_close_two_group_levels() {
    let ed = Editor::launch("group_quick_double_clicks");
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 1.0 }));
    let a = ed.box_brush([40.0, 0.0, -64.0], [104.0, 32.0, 0.0]);
    let b = ed.box_brush([140.0, 0.0, 24.0], [172.0, 64.0, 88.0]);
    let c = ed.box_brush([-130.0, 0.0, 40.0], [-70.0, 48.0, 112.0]);
    ed.call("select", json!({ "ids": [a, b] }));
    ed.call("run_action", json!({ "action": "group" }));
    let inner = selected(&ed)[0];
    ed.call("select", json!({ "ids": [inner, c] }));
    ed.call("run_action", json!({ "action": "group" }));
    let outer = selected(&ed)[0];
    ed.call("hierarchy", json!({ "op": "close_group", "all": true }));
    ed.call("select", json!({ "ids": [] }));

    // Four clicks well inside the 0.6 s that egui counts towards a triple click.
    let twice = |at: [f64; 3]| ed.input("top", json!([{ "type": "double_click", "world": at }, { "type": "double_click", "world": at }]));
    twice([72.0, 32.0, -32.0]);
    assert_eq!(open_groups(&ed), json!([outer, inner]), "the second double click opens the inner group too");
    assert_eq!(selected(&ed), [a]);
    std::thread::sleep(std::time::Duration::from_secs_f64(0.7));
    twice([0.0, 0.0, -110.0]);
    assert_eq!(open_groups(&ed), json!([]), "two double clicks on empty space close both levels");
}

#[test]
#[ignore]
fn double_click_on_an_entity_brush_still_selects_the_entity() {
    let ed = Editor::launch("group_gestures_entity");
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 1.0 }));
    let p = ed.box_brush([40.0, 0.0, -64.0], [104.0, 32.0, 0.0]);
    let q = ed.box_brush([130.0, 0.0, -64.0], [170.0, 48.0, 0.0]);
    let loose = ed.box_brush([-130.0, 0.0, 40.0], [-70.0, 48.0, 112.0]);
    ed.call("select", json!({ "ids": [p, q] }));
    ed.call("run_action", json!({ "action": "create_brush_entity", "args": { "classname": "func_door" } }));
    let entity = selected(&ed)[0];
    ed.call("select", json!({ "ids": [] }));
    let double_click = |at: [f64; 3]| {
        ed.input("top", json!([{ "type": "double_click", "world": at }]));
        std::thread::sleep(std::time::Duration::from_secs_f64(0.7));
    };

    double_click([72.0, 32.0, -32.0]);
    assert_eq!(selected_sorted(&ed), [p.min(q), p.max(q)], "a loose entity brush widens to every brush of the entity");
    double_click([-100.0, 48.0, 76.0]);
    assert_eq!(selected(&ed), [loose], "a plain brush stays as it is");

    ed.call("select", json!({ "ids": [entity] }));
    ed.call("run_action", json!({ "action": "group" }));
    let group = selected(&ed)[0];
    ed.call("hierarchy", json!({ "op": "close_group", "all": true }));
    ed.call("select", json!({ "ids": [] }));
    double_click([72.0, 32.0, -32.0]);
    assert_eq!(open_groups(&ed), json!([group]), "inside a closed group the first double click opens it");
    assert_eq!(selected(&ed), [p]);
    double_click([72.0, 32.0, -32.0]);
    assert_eq!(selected_sorted(&ed), [p.min(q), p.max(q)], "and the next one selects the entity");
    assert_eq!(open_groups(&ed), json!([group]));
}

#[test]
#[ignore]
fn clip_vertex_rotate_tools() {
    let ed = Editor::launch("tools");
    ed.box_brush([-64.0, 0.0, -32.0], [64.0, 64.0, 32.0]);
    ed.call("set_camera", json!({ "view": "front", "center": [0, 32, 0], "zoom": 1.5 }));

    // Clip tool: two points on the x=16 line in the front view, Enter keeps the front side.
    ed.input("window", json!([{ "type": "key", "key": "C" }]));
    assert_eq!(ed.state()["editor"]["tool"], "Clip");
    ed.input("front", json!([{ "type": "click", "world": [16, -16, 0] }, { "type": "click", "world": [16, 96, 0] }]));
    assert_eq!(ed.state()["editor"]["clip_points"].as_array().unwrap().len(), 2);
    ed.screenshot("front", "clip_preview");
    ed.input("front", json!([{ "type": "key", "key": "Enter" }]));
    assert_eq!(ed.brushes(), 1);
    let (min, max) = ed.selection_bounds();
    let width = max[0] - min[0];
    assert!((width - 48.0).abs() < 1e-3 || (width - 80.0).abs() < 1e-3, "clip width {width}");

    // Vertex tool: drag the top-right vertex up by 32 in the front view.
    ed.call("set_editor", json!({ "tool": "vertex" }));
    let (min, max) = ed.selection_bounds();
    let corner = [max[0], max[1], max[2]];
    ed.input("front", json!([{ "type": "drag", "world": corner, "to_world": [corner[0], corner[1] + 32.0, corner[2]] }]));
    let (_, new_max) = ed.selection_bounds();
    assert!((new_max[1] - (max[1] + 32.0)).abs() < 1e-3, "vertex move {new_max:?}");
    let node = ed.call("list_nodes", json!({ "type": "brush" }))["nodes"][0]["id"].as_u64().unwrap();
    let brush = ed.call("get_node", json!({ "id": node }));
    assert!(brush["faces"].as_array().unwrap().len() >= 6);
    let _ = min;

    // Rotate 90 degrees about Y with the menu action swaps the X and Z extents.
    ed.call("set_editor", json!({ "tool": "select" }));
    let (min, max) = ed.selection_bounds();
    let (sx, sz) = (max[0] - min[0], max[2] - min[2]);
    ed.call("run_action", json!({ "action": "rotate", "args": { "axis": "y", "degrees": 90 } }));
    let (min, max) = ed.selection_bounds();
    assert!(((max[0] - min[0]) - sz).abs() < 1e-3 && ((max[2] - min[2]) - sx).abs() < 1e-3);
    assert_eq!(ed.call("validate_map", json!({}))["count"], 0, "map should have no issues");
}

#[test]
#[ignore]
fn csg_entities_io_and_files() {
    let ed = Editor::launch("csg");
    ed.box_brush([-64.0, 0.0, -64.0], [64.0, 128.0, 64.0]);
    let cutter = ed.box_brush([-16.0, -16.0, -128.0], [16.0, 64.0, 128.0]);
    ed.call("select", json!({ "ids": [cutter] }));
    ed.call("run_action", json!({ "action": "csg_subtract" }));
    assert_eq!(ed.brushes(), 3, "door cut leaves left, right and top pieces");

    ed.call("run_action", json!({ "action": "select_all" }));
    ed.call("run_action", json!({ "action": "group" }));
    assert_eq!(ed.call("list_nodes", json!({ "type": "group" }))["total"], 1);
    ed.call("run_action", json!({ "action": "ungroup" }));
    assert_eq!(ed.call("list_nodes", json!({ "type": "group" }))["total"], 0);

    // Brush entity with I/O pointing at a light that does not exist yet.
    let door = ed.box_brush([-16.0, 0.0, -4.0], [16.0, 64.0, 4.0]);
    ed.call("select", json!({ "ids": [door] }));
    ed.call("run_action", json!({ "action": "create_brush_entity", "args": { "classname": "func_door" } }));
    let door_entity = ed.state()["selection"]["nodes"][0].as_u64().unwrap();
    ed.call("update_entity", json!({ "id": door_entity, "properties": { "targetname": "door1" }, "outputs": [{ "output": "opened", "target": "lamp", "input": "turn_on", "delay": 0.5 }] }));
    let renamed = ed.call("hierarchy", json!({ "op": "rename", "id": door_entity, "name": "front door" }));
    assert_eq!(renamed["name"], "front door");
    let node = ed.call("get_node", json!({ "id": door_entity }));
    assert_eq!(
        (&node["label"], &node["properties"]["targetname"]),
        (&json!("front door"), &json!("door1")),
        "renaming over MCP names it like the Outliner does"
    );
    let revision = ed.state()["map"]["revision"].clone();
    assert_eq!(ed.call("hierarchy", json!({ "op": "rename", "id": door_entity, "name": " front door " }))["name"], "front door");
    assert_eq!(ed.state()["map"]["revision"], revision, "the same name again is no edit");
    assert_eq!(ed.call("hierarchy", json!({ "op": "rename", "id": door_entity, "name": "" }))["name"], "func_door (door1)");
    let layer = ed.state()["editor"]["current_layer"].clone();
    assert!(ed.call_err("hierarchy", json!({ "op": "rename", "id": layer, "name": " " })).contains("need a name"));
    let issues = ed.call("validate_map", json!({}));
    assert!(issues["issues"].as_array().unwrap().iter().any(|i| i["code"] == "io_missing_target"));
    ed.call("create_entity", json!({ "classname": "light", "origin": [0, 96, 96], "properties": { "targetname": "lamp" } }));
    let issues = ed.call("validate_map", json!({}));
    assert!(!issues["issues"].as_array().unwrap().iter().any(|i| i["code"] == "io_missing_target"), "{issues}");

    // Save, clear, reopen.
    let path = artifacts().join("csg_roundtrip.tdm");
    ed.call("map_file", json!({ "op": "save", "path": path }));
    let before = ed.state()["map"].clone();
    ed.call("map_file", json!({ "op": "new" }));
    assert_eq!(ed.brushes(), 0);
    let opened = ed.call("map_file", json!({ "op": "open", "path": path }));
    assert_eq!(opened["problems"], json!([]));
    let after = ed.state()["map"].clone();
    assert_eq!(before["brushes"], after["brushes"]);
    assert_eq!(before["entities"], after["entities"]);
    let bytes = std::fs::read(&path).unwrap();
    assert!(td_doc::binary::is_binary(&bytes), "maps are saved in the binary container");
    let (text, _) = td_doc::format::file_to_json(&bytes).unwrap();
    assert!(text.contains("\"format\": \"trenchdot-map\"") && text.contains("\"opened\""));

    let glb = artifacts().join("csg_roundtrip.glb");
    let exported = ed.call("map_file", json!({ "op": "export_glb", "path": glb, "markers": true }));
    assert!(exported["objects"].as_u64().unwrap() > 1 && exported["triangles"].as_u64().unwrap() > 0, "{exported}");
    let (doc, _, _) = gltf::import(&glb).expect("the export reads back as glTF");
    assert!(doc.nodes().any(|n| n.name() == Some("light (lamp)") && n.mesh().is_none()), "the light is an empty marker");
    assert!(ed.call_err("run_action", json!({ "action": "export_glb" })).contains("map_file {op: export_glb or export_obj, path}"));

    // A damaged save still opens, with what was lost reported.
    let damaged = artifacts().join("csg_damaged.tdm");
    std::fs::write(&damaged, &bytes[..bytes.len() - 8]).unwrap();
    let opened = ed.call("map_file", json!({ "op": "open", "path": damaged }));
    assert!(opened["problems"].as_array().is_some_and(|p| !p.is_empty()), "{opened}");
    assert_eq!(ed.state()["map"]["brushes"], after["brushes"]);

    assert!(ed.call_err("get_node", json!({ "id": 999999 })).contains("no node"));
    let (_, _, colors) = ed.screenshot("3d", "csg");
    assert!(colors > 8, "3d view should show geometry");
}

#[test]
#[ignore]
fn prefabs_and_command_palette() {
    use td_doc::{Entity, Map, NodeKind, format};
    let ed = Editor::launch("prefabs");
    let dir = artifacts().join("prefabs");
    std::fs::create_dir_all(&dir).unwrap();

    let mut prefab = Map::new();
    let layer = prefab.default_layer();
    prefab.insert(
        layer,
        NodeKind::Brush(td_geom::Brush::from_aabb(&td_core::Aabb::new(td_core::DVec3::ZERO, td_core::DVec3::new(64.0, 32.0, 16.0)), "dev/blue").unwrap()),
    );
    let mut light = Entity::new("light");
    light.origin = td_core::DVec3::new(32.0, 48.0, 0.0);
    light.properties.insert("targetname".into(), "plight".into());
    prefab.insert(layer, NodeKind::Entity(light));
    format::save(&prefab, &dir.join("crate.tdm")).unwrap();

    ed.call("map_file", json!({ "op": "save", "path": dir.join("main.tdm") }));
    let inserted = ed.call(
        "run_action",
        json!({ "action": "insert_prefab", "args": { "path": dir.join("crate.tdm"), "origin": [100, 0, 0], "angles": [0, 90, 0], "fixup": "a" } }),
    );
    assert_eq!(inserted["path"], "crate.tdm", "prefab path stored relative to the map");
    // The scene cache resolves instance bounds on the next frame.
    ed.input("window", json!([{ "type": "move", "x": 10, "y": 10 }]));
    let inst = &ed.call("list_nodes", json!({ "type": "instance" }))["nodes"][0];
    let min: Vec<f64> = inst["bounds"]["min"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let max: Vec<f64> = inst["bounds"]["max"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    // Brush rotated 90° spans x 100..116, z -64..0. The light's 16 unit box at (100, 48, -32) widens x and y.
    assert!(approx(&min, &[92.0, 0.0, -64.0]) && approx(&max, &[116.0, 56.0, 0.0]), "rotated instance bounds {min:?} {max:?}");
    ed.call("select", json!({ "ids": [inst["id"]] }));
    let (sel_min, sel_max) = ed.selection_bounds();
    assert!(approx(&sel_min, &min) && approx(&sel_max, &max), "selection bounds cover the prefab content, got {sel_min:?} {sel_max:?}");
    ed.screenshot("3d", "instance");

    ed.call("run_action", json!({ "action": "explode_instances" }));
    assert_eq!(ed.brushes(), 1);
    let lights = ed.call("list_nodes", json!({ "classname": "light" }));
    assert_eq!(lights["nodes"][0]["name"], "light (a-plight)", "fixup applied to exploded names");

    // Command palette: F1, type, Enter.
    ed.input("window", json!([{ "type": "key", "key": "F1" }, { "type": "text", "text": "grid larger" }, { "type": "key", "key": "Enter" }]));
    assert_eq!(ed.state()["editor"]["grid"], 32.0);
}

#[test]
#[ignore]
fn mesh_tool_extrude_with_keys() {
    let ed = Editor::launch("mesh");
    let id = ed.call("create_mesh", json!({ "shape": "cuboid", "min": [-32, 0, -32], "max": [32, 64, 32] }))["id"].as_u64().unwrap();
    assert_eq!(ed.state()["map"]["meshes"], 1);
    ed.call("set_editor", json!({ "tool": "mesh", "mesh_component": "face" }));
    ed.call("set_camera", json!({ "view": "3d", "position": [-120, 200, 160], "look_at": [0, 32, 0] }));

    // Blender style: click the top face, E extrudes into a grab, Y constrains, typed distance, Enter confirms.
    ed.input("3d", json!([{ "type": "click", "world": [0, 64, 0] }]));
    ed.input("3d", json!([{ "type": "key", "key": "E" }, { "type": "key", "key": "Y" }, { "type": "text", "text": "32" }, { "type": "key", "key": "Enter" }]));
    let mesh = ed.call("get_node", json!({ "id": id }));
    assert_eq!(mesh["faces"].as_array().unwrap().len(), 10, "extruding one face of a cube adds four side faces");
    let (_, max) = ed.selection_bounds();
    assert!((max[1] - 96.0).abs() < 1e-3, "extruded top {max:?}");
    ed.screenshot("3d", "extruded");

    // Undo restores the cube, the topology API works on the same node.
    ed.input("window", json!([{ "type": "key", "key": "Z", "modifiers": ["ctrl"] }]));
    ed.input("window", json!([{ "type": "key", "key": "Z", "modifiers": ["ctrl"] }]));
    assert_eq!(ed.call("get_node", json!({ "id": id }))["faces"].as_array().unwrap().len(), 6);
    let cut = ed.call("mesh_edit", json!({ "id": id, "op": "bisect", "point": [0, 16, 0], "normal": [0, 1, 0], "delete": "back", "fill": true }));
    assert_eq!(cut["closed"], true, "bisect with fill keeps the mesh closed");
    let r = ed.call("mesh_edit", json!({ "id": id, "op": "solidify", "thickness": 4 }));
    assert!(r["face_count"].as_u64().unwrap() >= 12);
    assert!(ed.call_err("mesh_edit", json!({ "id": id, "op": "nope" })).contains("unknown"));

    let cube = ed.call("create_mesh", json!({ "shape": "cuboid", "min": [100, 0, 0], "max": [164, 64, 64] }))["id"].as_u64().unwrap();
    ed.call("select", json!({ "ids": [cube] }));
    let brushes = ed.brushes();
    ed.call("run_action", json!({ "action": "convert_to_brushes" }));
    assert_eq!(ed.brushes(), brushes + 1);
    assert_eq!(ed.state()["map"]["meshes"], 1, "only the convex cube was converted");

    let r = ed.call(
        "run_action",
        json!({ "action": "create_decal", "args": { "material": "base/wall", "at": [100, 32, 32], "normal": [-1, 0, 0], "size": [48, 24] } }),
    );
    let decal = ed.call("get_node", json!({ "id": r["selection"][0] }));
    assert_eq!(decal["decal"], true);
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[max[1] - min[1], max[2] - min[2]], &[24.0, 48.0]), "decal spans its size on the wall {min:?} {max:?}");
    assert!(ed.call_err("run_action", json!({ "action": "create_decal", "args": { "at": [0, 0, 0] } })).contains("material"));
}

#[test]
#[ignore]
fn terrain_sprinkle_cordon_tabs_bookmarks() {
    let ed = Editor::launch("terrain");
    let t = ed.call("create_terrain", json!({ "origin": [-512, 0, -512], "resolution": 33, "cell_size": 32, "shape": "flat" }));
    let id = t["id"].as_u64().unwrap();
    assert_eq!(ed.state()["map"]["terrains"], 1);
    let sculpt = ed.call("run_action", json!({ "action": "sculpt", "args": { "center": [0, 0, 0], "mode": "raise", "radius": 128, "strength": 64 } }));
    assert_eq!(sculpt["changed"], true);
    let (_, max) = ed.selection_bounds();
    assert!(max[1] > 32.0, "raised terrain {max:?}");

    // Sculpt tool with real mouse drags over the terrain.
    ed.call("set_editor", json!({ "tool": "sculpt", "sculpt_mode": "raise", "brush_radius": 96 }));
    ed.call("set_camera", json!({ "view": "3d", "position": [-600, 500, 600], "look_at": [200, 0, -200] }));
    let rev = ed.state()["map"]["revision"].as_u64().unwrap();
    ed.input("3d", json!([{ "type": "drag", "world": [200, 0, -200], "to_world": [300, 0, -300], "steps": 10 }]));
    assert!(ed.state()["map"]["revision"].as_u64().unwrap() > rev, "sculpt drag edits the terrain");
    ed.call("run_action", json!({ "action": "select_none" }));
    assert_eq!(ed.state()["selection"]["nodes"], json!([]), "select_none empties the selection with the Sculpt tool active");
    assert_eq!(ed.state()["editor"]["tool"], "Sculpt", "and leaves the tool alone");
    ed.call("terrain_edit", json!({ "id": id, "op": "set_layers", "layers": [["dev/green", 256], ["dev/grey", 256], ["dev/blue", 256], ["dev/orange", 256]] }));
    let low_weight = |sea_level: Value| {
        ed.call("terrain_edit", json!({ "id": id, "op": "auto_paint", "sea_level": sea_level }));
        ed.call("blend", json!({ "op": "weights", "id": id, "center": [-500, 0, -500] }))["weights"][3].as_f64().unwrap()
    };
    let (from_floor, from_sea) = (low_weight(Value::Null), low_weight(json!(40)));
    assert!(from_floor < 0.5 && from_sea > 0.8, "flat ground below sea level joins the low band only when bands start at sea level: {from_floor} {from_sea}");

    // clear_layer wipes a layer's paint everywhere, so a script that repaints an area gives the same result however
    // many times it runs.
    let weight_at = |id: u64, p: [f64; 3]| ed.call("blend", json!({ "op": "weights", "id": id, "center": p }))["weights"][3].as_f64().unwrap();
    assert_eq!(ed.call("terrain_edit", json!({ "id": id, "op": "clear_layer", "layer": 3 }))["changed"], true);
    assert_eq!(weight_at(id, [-500.0, 0.0, -500.0]), 0.0, "layer 3 cleared everywhere");

    // A rectangular hole for scripts that need a precise cut rather than a circular brush.
    let holed = ed.call("terrain_edit", json!({ "id": id, "op": "holes", "min": [-40, -40], "max": [40, 40], "hole": true }));
    assert_eq!(holed["changed"], true);
    ed.call("terrain_edit", json!({ "id": id, "op": "holes", "min": [-40, -40], "max": [40, 40], "hole": false }));

    ed.call("set_editor", json!({ "shade": "lit", "tool": "select" }));
    let (_, _, colors) = ed.screenshot("3d", "terrain_lit");
    assert!(colors > 8);

    let placed = ed.call("run_action", json!({ "action": "sprinkle", "args": { "center": [-300, 0, 300], "items": ["info_player_start"], "radius": 160, "density": 2, "min_spacing": 24, "seed": 3 } }));
    let count = placed["ids"].as_array().unwrap().len();
    assert!(count >= 3, "sprinkle placed {count}");
    let first = ed.call("get_node", json!({ "id": placed["ids"][0] }));
    let y = first["origin"][1].as_f64().unwrap();
    assert!(y.abs() < 1.0, "sprinkled onto the flat part of the terrain, got {y}");

    ed.call("select", json!({ "ids": [id] }));
    ed.call("run_action", json!({ "action": "set_cordon" }));
    assert_eq!(ed.state()["map"]["cordon_enabled"], true);
    ed.call("run_action", json!({ "action": "clear_cordon" }));
    assert!(ed.state()["map"]["cordon"].is_null());

    ed.call("run_action", json!({ "action": "store_camera", "args": { "slot": 2 } }));
    assert_eq!(ed.state()["map"]["cameras"], json!([2]));
    ed.call("set_camera", json!({ "view": "3d", "position": [0, 50, 0], "look_at": [10, 50, 0] }));
    ed.call("run_action", json!({ "action": "recall_camera", "args": { "slot": 2 } }));
    let cameras = ed.state()["cameras"].clone();
    let cam = cameras.as_array().unwrap().iter().find(|v| v["view"] == "3d").unwrap().clone();
    assert!((cam["position"][0].as_f64().unwrap() + 600.0).abs() < 1.0, "recalled camera {cam}");

    ed.call("run_action", json!({ "action": "new_tab" }));
    let tabs = ed.state()["tabs"].clone();
    assert_eq!(tabs["titles"].as_array().unwrap().len(), 2);
    assert_eq!(ed.state()["map"]["terrains"], 0, "new tab starts empty");
    ed.call("run_action", json!({ "action": "next_tab" }));
    assert_eq!(ed.state()["map"]["terrains"], 1, "switching back keeps the first map");
}

#[test]
#[ignore]
fn terrain_edges_drag_whole_cells_in_the_2d_views() {
    let ed = Editor::launch("terrain_edge_drag");
    let id = ed.call("create_terrain", json!({ "origin": [0, 0, 0], "resolution": 9, "cell_size": 32, "shape": "flat" }))["id"].as_u64().unwrap();
    ed.call("select", json!({ "ids": [id] }));
    let node = |ed: &Editor| ed.call("get_node", json!({ "id": id, "compact": true }));
    let undo = |ed: &Editor| ed.state()["undo"].as_array().unwrap().len();
    let undo_before = undo(&ed);

    ed.call("set_camera", json!({ "view": "top", "center": [128, 0, 128], "zoom": 0.5 }));
    ed.input("top", json!([{ "type": "drag", "world": [256, 0, 128], "to_world": [326, 0, 128] }]));
    assert_eq!(node(&ed)["resolution"], json!([11, 9]), "70 units east rounds to 2 cells");

    // Flat ground is a line in the side views, its ends still grab.
    ed.call("set_camera", json!({ "view": "front", "center": [128, 0, 128], "zoom": 0.5 }));
    ed.input("front", json!([{ "type": "drag", "world": [0, 0, 128], "to_world": [-40, 0, 128] }]));
    assert_eq!(node(&ed)["resolution"], json!([12, 9]));
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0]], &[-32.0, 320.0]), "growing west moves the west edge {min:?} {max:?}");

    ed.call("set_camera", json!({ "view": "top", "center": [128, 0, 128], "zoom": 0.5 }));
    ed.input("top", json!([{ "type": "drag", "world": [64, 0, 0], "to_world": [64, 0, 400] }]));
    assert_eq!(node(&ed)["resolution"], json!([12, 2]), "a crop past the far side keeps one cell");
    assert_eq!(undo(&ed), undo_before + 3, "each drag is one undo step");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(node(&ed)["resolution"], json!([12, 9]));

    ed.call("set_camera", json!({ "view": "side", "center": [128, 0, 128], "zoom": 0.5 }));
    ed.input("side", json!([{ "type": "drag", "world": [0, 0, 256], "to_world": [0, 0, 200] }]));
    assert_eq!(node(&ed)["resolution"], json!([12, 7]), "56 units back from the south edge rounds to 2 cells");
    assert_eq!(ed.call("validate_map", json!({}))["count"], 0);

    let steps = undo(&ed);
    ed.call("set_camera", json!({ "view": "top", "center": [128, 0, 128], "zoom": 0.5 }));
    ed.input("top", json!([{ "type": "drag", "world": [320, 0, 64], "to_world": [384, 0, 64], "release": false }]));
    assert_eq!(node(&ed)["resolution"], json!([14, 7]), "a held drag shows its cells");
    ed.input("top", json!([{ "type": "release", "world": [320, 0, 64] }]));
    assert_eq!(node(&ed)["resolution"], json!([12, 7]));
    assert_eq!(undo(&ed), steps, "a drag back to where it started leaves no undo step");

    // With a brush selected too, the shared edge resizes the brush and leaves the terrain alone.
    let brush = ed.box_brush([256.0, 0.0, 64.0], [320.0, 32.0, 128.0]);
    ed.call("select", json!({ "ids": [id, brush] }));
    ed.input("top", json!([{ "type": "drag", "world": [320, 0, 96], "to_world": [384, 0, 96] }]));
    assert_eq!(node(&ed)["resolution"], json!([12, 7]));
    let (_, max) = ed.selection_bounds();
    assert!(approx(&[max[0]], &[384.0]), "the brush grew {max:?}");
    ed.call("select", json!({ "ids": [brush] }));
    ed.call("run_action", json!({ "action": "delete" }));

    ed.call("hierarchy", json!({ "op": "set_flags", "ids": [id], "locked": true }));
    ed.call("select", json!({ "ids": [id] }));
    ed.input("top", json!([{ "type": "drag", "world": [320, 0, 64], "to_world": [384, 0, 64] }]));
    assert_eq!(node(&ed)["resolution"], json!([12, 7]), "a locked terrain keeps its cells");
    ed.call("hierarchy", json!({ "op": "set_flags", "ids": [id], "locked": false }));

    // Once the terrain has height, its top and bottom in the side views are sculpted, not dragged.
    ed.call("terrain_edit", json!({ "id": id, "op": "flatten_rect", "min": [-32, 0], "max": [96, 192], "height": 128, "margin": 0 }));
    ed.call("select", json!({ "ids": [id] }));
    let steps = undo(&ed);
    ed.call("set_camera", json!({ "view": "front", "center": [128, 64, 96], "zoom": 0.5 }));
    ed.input("front", json!([{ "type": "drag", "world": [224, 128, 96], "to_world": [224, 256, 96] }]));
    assert_eq!(node(&ed)["resolution"], json!([12, 7]));
    assert_eq!(undo(&ed), steps, "dragging the top does nothing");
    ed.call("select", json!({ "ids": [id] }));
    ed.input("front", json!([{ "type": "drag", "world": [314, 126, 96], "to_world": [378, 126, 96] }]));
    assert_eq!(node(&ed)["resolution"], json!([14, 7]), "the end grabs even where the top is nearer");
}

#[test]
#[ignore]
fn terrain_resize_over_mcp() {
    let ed = Editor::launch("terrain_resize");
    let t = ed.call("create_terrain", json!({ "origin": [-100.5, 7, 40.25], "resolution": 17, "cell_size": 32, "shape": "flat" }));
    let id = t["id"].as_u64().unwrap();
    ed.call("terrain_edit", json!({ "id": id, "op": "ramp", "from": [-90, 40, 60], "to": [380, 150, 480], "width": 200, "margin": 96 }));
    let set = ed.call("scatter", json!({ "op": "new_set", "preset": "grass", "name": "grass", "targets": [id] }))["id"].as_u64().unwrap();
    ed.call("scatter", json!({ "op": "fill", "density": 3, "seed": 5 }));
    let compact = |ed: &Editor, id: u64| ed.call("get_node", json!({ "id": id, "compact": true }));
    let instances = |ed: &Editor| compact(ed, set)["instances"].as_u64().unwrap();
    let before_count = instances(&ed);
    assert!(before_count > 40, "scatter on the terrain, got {before_count}");

    let height = |ed: &Editor, p: [f64; 2]| ed.call("terrain_edit", json!({ "id": id, "probe": p }))["probe_height"].as_f64().unwrap();
    // Vertex positions, since an odd shift flips the cells' diagonals and moves the surface between vertices.
    let probes = [[187.5, 296.25], [379.5, 488.25], [91.5, 72.25], [27.5, 232.25]];
    let heights = |ed: &Editor| -> Vec<f64> { probes.iter().map(|p| height(ed, *p)).collect() };
    let close = |a: &[f64], b: &[f64]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
    let slope = heights(&ed);
    assert!(slope[0] > slope[2] + 20.0, "the ramp made a slope {slope:?}");
    let north_edge = height(&ed, [91.5, 40.25]);
    let node_before = ed.call("get_node", json!({ "id": id }));
    assert_eq!(compact(&ed, id)["resolution"], json!([17, 17]));
    let revision = || ed.state()["map"]["revision"].clone();
    let (rev_before, undo_before) = (revision(), ed.state()["undo"].clone());

    // A dry run reports the plan and touches nothing.
    let args = json!({ "id": id, "op": "resize", "north": 4, "west": -3 });
    let mut dry_args = args.clone();
    dry_args["dry_run"] = json!(true);
    let dry = ed.call("terrain_edit", dry_args);
    assert_eq!((dry["changed"].clone(), dry["dry_run"].clone()), (json!(false), json!(true)));
    assert_eq!((dry["resolution"].clone(), dry["cells"].clone(), dry["cell_size"].clone()), (json!([14, 21]), json!([13, 20]), json!(32.0)));
    assert_eq!(dry["origin"], json!([-4.5, 7.0, -87.75]));
    assert_eq!((dry["bounds"]["min"][0].clone(), dry["bounds"]["max"][0].clone()), (json!(-4.5), json!(-4.5 + 13.0 * 32.0)));
    assert_eq!((dry["bounds"]["min"][2].clone(), dry["bounds"]["max"][2].clone()), (json!(-87.75), json!(-87.75 + 20.0 * 32.0)));
    assert_eq!(dry["detail_lost"], false);
    assert!(dry.get("scatter_moved").is_none(), "a dry run builds nothing to move");
    let removed = dry["scatter_removed"].as_u64().unwrap();
    assert!(removed > 0 && removed < before_count, "the removed strip holds some of the {before_count} plants, got {removed}");
    assert_eq!((revision(), ed.state()["undo"].clone(), instances(&ed)), (rev_before.clone(), undo_before.clone(), before_count));
    assert_eq!(ed.call("get_node", json!({ "id": id })), node_before);

    // The same call for real, with a probe in the grown strip.
    let mut real_args = args.clone();
    real_args["probe"] = json!([91.5, -50.0]);
    let done = ed.call("terrain_edit", real_args);
    assert_eq!((done["changed"].clone(), done["scatter_removed"].clone()), (json!(true), json!(removed)));
    assert_eq!((done["resolution"].clone(), done["origin"].clone(), done["id"].clone()), (dry["resolution"].clone(), dry["origin"].clone(), json!(id)));
    assert!(done["scatter_moved"].is_u64());
    assert!((done["probe_height"].as_f64().unwrap() - north_edge).abs() < 1e-3, "new ground repeats the edge height, {done}");
    assert_eq!(instances(&ed), before_count - removed);
    let (rev_after, undo_after) = (revision(), ed.state()["undo"].clone());
    assert_eq!((undo_after[0].clone(), undo_after[1].clone()), (json!("MCP: Resize Terrain"), undo_before[0].clone()), "one undo step");
    let node_after = ed.call("get_node", json!({ "id": id }));
    let shape = compact(&ed, id);
    assert_eq!((shape["resolution"].clone(), shape["bounds"]["min"][0].clone()), (json!([14, 21]), json!(-4.5)));
    let kept = heights(&ed);
    assert!(close(&kept, &slope), "kept vertices keep their height, {kept:?} vs {slope:?}");

    // Undo and redo are exact.
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!((ed.call("get_node", json!({ "id": id })), instances(&ed)), (node_before.clone(), before_count));
    assert!(close(&heights(&ed), &slope));
    assert_eq!(ed.state()["undo"], undo_before);
    ed.call("run_action", json!({ "action": "redo" }));
    assert_eq!((ed.call("get_node", json!({ "id": id })), instances(&ed)), (node_after.clone(), before_count - removed));
    assert!(close(&heights(&ed), &kept));
    assert_eq!(ed.state()["undo"], undo_after);
    assert_ne!(rev_after, rev_before);

    // cells resizes around the center and gives the odd cell to east or south.
    let shrunk = ed.call("terrain_edit", json!({ "id": id, "op": "resize", "cells": [10, 15] }));
    assert_eq!((shrunk["cells"].clone(), shrunk["origin"].clone()), (json!([10, 15]), json!([27.5, 7.0, -23.75])));
    let grown = ed.call("terrain_edit", json!({ "id": id, "op": "resize", "cells": [13, 15], "fill": "zero", "scatter": "keep" }));
    assert_eq!((grown["cells"].clone(), grown["origin"].clone(), grown["scatter_removed"].clone()), (json!([13, 15]), json!([-4.5, 7.0, -23.75]), json!(0)));
    ed.call("run_action", json!({ "action": "undo", "args": { "steps": 2 } }));
    assert_eq!(compact(&ed, id)["resolution"], json!([14, 21]));
    ed.call("run_action", json!({ "action": "redo", "args": { "steps": 2 } }));
    assert_eq!(compact(&ed, id)["resolution"], json!([14, 16]));

    // A cell size change keeps the origin and rounds the counts half away from zero.
    let finer = ed.call("terrain_edit", json!({ "id": id, "op": "resize", "cell_size": 16, "dry_run": true }));
    assert_eq!((finer["cells"].clone(), finer["detail_lost"].clone(), finer["origin"].clone()), (json!([26, 30]), json!(false), json!([-4.5, 7.0, -23.75])));
    let coarser = ed.call("terrain_edit", json!({ "id": id, "op": "resize", "cell_size": 64 }));
    assert_eq!((coarser["cells"].clone(), coarser["cell_size"].clone(), coarser["detail_lost"].clone()), (json!([7, 8]), json!(64.0), json!(true)));
    assert_eq!(coarser["origin"], json!([-4.5, 7.0, -23.75]));
    assert!(coarser["scatter_moved"].is_u64());

    // Nothing to change is not an edit.
    let (rev, undo) = (revision(), ed.state()["undo"].clone());
    for same in [json!({ "north": 0 }), json!({ "cell_size": 64 }), json!({ "cells": [7, 8] }), json!({ "east": 0, "south": 0, "cell_size": 64.0 })] {
        let mut a = same.clone();
        a["id"] = json!(id);
        a["op"] = json!("resize");
        let r = ed.call("terrain_edit", a);
        assert_eq!((r["changed"].clone(), r["scatter_removed"].clone()), (json!(false), json!(0)), "{same}");
        assert_eq!(r["cells"], json!([7, 8]), "{same}");
    }

    assert_eq!((revision(), ed.state()["undo"].clone()), (rev.clone(), undo.clone()), "no op leaves no undo step");

    // Every guard refuses and leaves the map alone.
    let refuse = |args: Value, text: &str| {
        let mut a = args.clone();
        a["id"] = json!(id);
        a["op"] = json!("resize");
        let e = ed.call_err("terrain_edit", a.clone());
        assert!(e.contains(text), "{args}: {e}");
        if args.get("dry_run").is_none() {
            a["dry_run"] = json!(true);
            let e = ed.call_err("terrain_edit", a);
            assert!(e.contains(text), "dry run {args}: {e}");
        }

        assert_eq!((revision(), ed.state()["undo"].clone()), (rev.clone(), undo.clone()), "{args} changed the map");
    };
    refuse(json!({}), "resize needs north, south, west, east, cells or cell_size");
    refuse(json!({ "cells": [8, 8], "north": 1 }), "cells cannot be combined");
    refuse(json!({ "cells": [14, 16], "cell_size": 32 }), "cannot be combined with cell_size");
    refuse(json!({ "west": 1.5 }), "west must be a whole number of cells");
    refuse(json!({ "north": "two" }), "north must be a whole number of cells");
    refuse(json!({ "south": 3000000000i64 }), "south must be a whole number of cells");
    refuse(json!({ "cells": [8] }), "cells must be [x, z]");
    refuse(json!({ "cells": [8.5, 8] }), "cells must be [x, z]");
    refuse(json!({ "cells": [0, 8] }), "cells must be [x, z]");
    refuse(json!({ "east": 1, "fill": "mid" }), "fill must be edge or zero");
    refuse(json!({ "east": 1, "scatter": "move" }), "scatter must be drop or keep");
    refuse(json!({ "east": 1, "dry_run": "yes" }), "dry_run must be true or false");
    refuse(json!({ "west": -7 }), "at least 1 cell along each axis, this leaves 0 along x");
    refuse(json!({ "south": -8 }), "at least 1 cell along each axis, this leaves 0 along z");
    refuse(json!({ "east": 2042 }), "at most 2048 cells along each axis, this makes 2049 along x");
    refuse(json!({ "north": 2041 }), "at most 2048 cells along each axis, this makes 2049 along z");
    refuse(json!({ "north": -8, "south": 3 }), "crops away every cell along z");
    refuse(json!({ "cells": [2049, 8] }), "this makes 2049 along x");
    refuse(json!({ "cell_size": 0.0009 }), "0.001 to 100000");
    refuse(json!({ "cell_size": 100001 }), "0.001 to 100000");
    refuse(json!({ "cell_size": -1 }), "0.001 to 100000");
    refuse(json!({ "cell_size": "big" }), "cell_size must be a number");
    refuse(json!({ "cell_size": 0.001 }), "at most 2048 cells along each axis");

    // The exact boundaries pass.
    let widest = ed.call("terrain_edit", json!({ "id": id, "op": "resize", "cells": [2048, 8], "scatter": "keep" }));
    assert_eq!(widest["resolution"], json!([2049, 9]));
    assert!(ed.call_err("terrain_edit", json!({ "id": id, "op": "resize", "east": 1 })).contains("this makes 2049 along x"));
    let narrowest = ed.call("terrain_edit", json!({ "id": id, "op": "resize", "cells": [1, 1] }));
    assert_eq!(narrowest["resolution"], json!([2, 2]));
    let tiny = ed.call("terrain_edit", json!({ "id": id, "op": "resize", "cell_size": 100000 }));
    assert_eq!((tiny["cells"].clone(), tiny["cell_size"].clone()), (json!([1, 1]), json!(100000.0)));

    let e = ed.call_err("terrain_edit", json!({ "id": set, "op": "resize", "east": 1 }));
    assert!(e.contains(&format!("node {set} is not a terrain")), "{e}");
    let e = ed.call_err("terrain_edit", json!({ "id": 987654, "probe": [0, 0] }));
    assert!(e.contains("no node 987654"), "{e}");

    // A locked terrain refuses, the same as the other terrain ops.
    ed.call("hierarchy", json!({ "op": "set_flags", "ids": [id], "locked": true }));
    let e = ed.call_err("terrain_edit", json!({ "id": id, "op": "resize", "east": 1 }));
    assert!(e.contains("hidden or locked"), "{e}");
    ed.call("hierarchy", json!({ "op": "set_flags", "ids": [id], "locked": false }));
    assert_eq!(ed.call("terrain_edit", json!({ "id": id, "op": "resize", "east": 1 }))["changed"], true);
}

#[test]
#[ignore]
fn terrain_symmetry_mirrors_strokes_and_mcp_calls() {
    let ed = Editor::launch("terrain_symmetry");
    let height = |ed: &Editor, id: u64, p: [f64; 2]| ed.call("terrain_edit", json!({ "id": id, "probe": p }))["probe_height"].as_f64().unwrap();
    let close = |a: f64, b: f64| (a - b).abs() < 1e-3;
    let symmetry = |ed: &Editor| ed.state()["editor"]["symmetry"].clone();
    let revision = |ed: &Editor| ed.state()["map"]["revision"].clone();

    // Terrain A starts at an odd corner, so its middle is x 411.5, z 552.25 and mirrored vertices are vertices.
    let a = ed.call("create_terrain", json!({ "origin": [-100.5, 7, 40.25], "resolution": 33, "cell_size": 32, "shape": "flat" }))["id"].as_u64().unwrap();
    let base = height(&ed, a, [-100.5, 40.25]);
    let st = symmetry(&ed);
    assert_eq!((st["axes"].clone(), st["center"].clone()), (json!("off"), json!("terrain")), "a fresh editor starts with symmetry off");

    ed.call("set_editor", json!({ "tool": "sculpt", "sculpt_mode": "raise", "brush_radius": 96, "symmetry": { "axes": "x" } }));
    let st = symmetry(&ed);
    assert_eq!((st["axes"].clone(), st["center"].clone(), st["x"].as_f64(), st["z"].as_f64()), (json!("x"), json!("terrain"), Some(0.0), Some(0.0)));
    ed.call("set_camera", json!({ "view": "3d", "position": [411.5, 900, -300], "look_at": [300, 7, 300] }));
    let undo_before = ed.state()["undo"].clone();
    ed.input("3d", json!([{ "type": "drag", "world": [200.5, 7, 300.25], "to_world": [260.5, 7, 380.25], "steps": 10 }]));
    let near = [[187.5, 296.25], [251.5, 360.25]];
    let mirrored = [[635.5, 296.25], [571.5, 360.25]];
    for (p, m) in near.iter().zip(&mirrored) {
        let (hp, hm) = (height(&ed, a, *p), height(&ed, a, *m));
        assert!(hp > base + 0.05, "the drag raised {p:?}: {hp} over {base}");
        assert!(close(hp, hm), "{p:?} and {m:?} match: {hp} vs {hm}");
    }

    let across_z = [187.5, 808.25];
    assert!(close(height(&ed, a, across_z), base), "x symmetry leaves the z mirror alone");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.state()["undo"], undo_before, "the whole stroke is one undo step");
    for p in near.iter().chain(&mirrored) {
        assert!(close(height(&ed, a, *p), base), "undo restored {p:?}");
    }

    // run_action sculpt takes the bar's setting unless it is given one. The vertices mirror across x 411.5.
    let sculpt = |center: [f64; 3], extra: Value| {
        let mut args = json!({ "center": center, "mode": "raise", "radius": 24, "strength": 16 });
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        ed.call("run_action", json!({ "action": "sculpt", "args": args }))
    };
    let (left, right, far_left, far_right) = ([379.5, 808.25], [443.5, 808.25], [-68.5, 808.25], [891.5, 808.25]);
    sculpt([left[0], 0.0, left[1]], json!({}));
    assert!(height(&ed, a, left) > base + 1.0 && close(height(&ed, a, left), height(&ed, a, right)), "without symmetry the bar's x applies");
    sculpt([far_left[0], 0.0, far_left[1]], json!({ "symmetry": "off" }));
    assert!(height(&ed, a, far_left) > base + 1.0 && close(height(&ed, a, far_right), base), "off in the call beats the bar");
    ed.call("set_editor", json!({ "symmetry": "off" }));
    let st = symmetry(&ed);
    assert_eq!((st["axes"].clone(), st["x"].as_f64()), (json!("off"), Some(0.0)));
    let (low_left, low_right) = ([27.5, 296.25], [795.5, 296.25]);
    sculpt([low_left[0], 0.0, low_left[1]], json!({}));
    assert!(height(&ed, a, low_left) > base + 1.0 && close(height(&ed, a, low_right), base), "the bar is off");
    sculpt([low_left[0], 0.0, 456.25], json!({ "symmetry": "x" }));
    assert!(
        close(height(&ed, a, [27.5, 456.25]), height(&ed, a, [795.5, 456.25])) && height(&ed, a, [795.5, 456.25]) > base + 1.0,
        "the call mirrors with the bar off"
    );

    // A value replaces the whole setting, so a custom center does not outlive a string.
    ed.call("set_editor", json!({ "symmetry": { "axes": "xz", "center": "custom", "x": 100.5, "z": -20.25 } }));
    let st = symmetry(&ed);
    assert_eq!((st["axes"].clone(), st["center"].clone(), st["x"].as_f64(), st["z"].as_f64()), (json!("xz"), json!("custom"), Some(100.5), Some(-20.25)));
    ed.call("set_editor", json!({ "symmetry": "z" }));
    let st = symmetry(&ed);
    assert_eq!((st["axes"].clone(), st["center"].clone(), st["x"].as_f64(), st["z"].as_f64()), (json!("z"), json!("terrain"), Some(0.0), Some(0.0)));
    ed.call("set_editor", json!({ "grid": 16 }));
    assert_eq!(symmetry(&ed)["axes"], "z", "other settings leave it alone");

    // Terrain B, far from A, is edited by id. Its middle is x 2256.5, z -744.25.
    let b = ed.call("create_terrain", json!({ "origin": [2000.5, 0, -1000.25], "resolution": 17, "cell_size": 32, "shape": "flat" }))["id"].as_u64().unwrap();
    ed.call("terrain_edit", json!({ "id": b, "op": "set_layers", "layers": [["dev/green", 256], ["dev/grey", 256], ["dev/blue", 256], ["dev/orange", 256]] }));
    let base_b = height(&ed, b, [2000.5, -1000.25]);
    let raise = |ed: &Editor, op: &str, extra: Value| {
        let mut args = json!({ "id": b, "op": op, "mode": "raise", "radius": 24, "strength": 16 });
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        ed.call("terrain_edit", args)
    };

    // No symmetry, even with the bar on z, so scripts replay the same.
    raise(&ed, "sculpt", json!({ "center": [2096.5, 0, -904.25] }));
    assert!(height(&ed, b, [2096.5, -904.25]) > base_b + 1.0 && close(height(&ed, b, [2096.5, -584.25]), base_b), "terrain_edit ignores the bar");

    raise(&ed, "sculpt", json!({ "center": [2096.5, 0, -840.25], "symmetry": "z" }));
    assert!(
        height(&ed, b, [2096.5, -840.25]) > base_b + 1.0 && close(height(&ed, b, [2096.5, -840.25]), height(&ed, b, [2096.5, -648.25])),
        "z mirrors across the terrain's middle"
    );
    assert!(close(height(&ed, b, [2416.5, -840.25]), base_b), "and not across x");

    // Four copies around a custom cross, off the middle and between vertices: x 2208.5, z -792.25.
    let cross = json!({ "axes": "xz", "center": "custom", "x": 2208.5, "z": -792.25 });
    raise(&ed, "sculpt", json!({ "center": [2128.5, 0, -872.25], "symmetry": cross }));
    let four = [[2128.5, -872.25], [2288.5, -872.25], [2128.5, -712.25], [2288.5, -712.25]];
    let h0 = height(&ed, b, four[0]);
    assert!(h0 > base_b + 1.0);
    for p in &four[1..] {
        assert!(close(height(&ed, b, *p), h0), "{p:?} matches the first dab");
    }

    // A path mirrors every dab.
    let path = raise(&ed, "sculpt_path", json!({ "points": [[2064.5, -616.25], [2064.5, -552.25]], "spacing": 32, "symmetry": { "axes": "x" } }));
    assert_eq!(path["changed"], true);
    for z in [-616.25, -584.25, -552.25] {
        let (l, r) = (height(&ed, b, [2064.5, z]), height(&ed, b, [2448.5, z]));
        assert!(l > base_b + 1.0 && close(l, r), "path vertex {z}: {l} vs {r}");
    }

    // A world plane far from the terrain reflects the dab off it, the real dab still lands.
    let world = raise(&ed, "sculpt", json!({ "center": [2096.5, 0, -1000.25], "symmetry": { "axes": "x", "center": "world" } }));
    assert_eq!(world["changed"], true);
    assert!(height(&ed, b, [2096.5, -1000.25]) > base_b + 1.0);

    // Paint and holes mirror too. The weights are those of the nearest vertex.
    let weights = |p: [f64; 3]| ed.call("blend", json!({ "op": "weights", "id": b, "center": p }))["weights"].clone();
    ed.call(
        "terrain_edit",
        json!({ "id": b, "op": "paint_path", "points": [[2352.5, -936.25], [2352.5, -904.25]], "layer": 1, "radius": 24, "symmetry": "x" }),
    );
    let (painted, twin, bare) = (weights([2352.5, 0.0, -936.25]), weights([2160.5, 0.0, -936.25]), weights([2352.5, 0.0, -600.25]));
    assert!(painted[1].as_f64().unwrap() > 0.1, "painted {painted}");
    assert_eq!(painted, twin, "paint lands at the mirrored vertex");
    assert_ne!(painted, bare);
    assert_eq!(weights([2160.5, 0.0, -904.25]), weights([2352.5, 0.0, -904.25]));

    let holed = ed.call("terrain_edit", json!({ "id": b, "op": "holes", "center": [2112.5, -888.25], "radius": 20, "symmetry": "x" }));
    assert_eq!(holed["changed"], true);
    let holes = |ed: &Editor| -> Vec<usize> {
        let node = ed.call("get_node", json!({ "id": b }));
        let bytes = base64::engine::general_purpose::STANDARD.decode(node["holes"].as_str().expect("holes are saved")).unwrap();
        bytes.iter().enumerate().filter(|(_, h)| **h != 0).map(|(k, _)| k).collect()
    };
    assert_eq!(holes(&ed), vec![3 * 16 + 3, 3 * 16 + 12], "cell (3, 3) and its mirror in a 16 cell wide terrain");

    // Blend: a stroke follows the bar and an explicit value wins. The bar is z, the middle z is -744.25.
    let stroke = |extra: Value| {
        let mut args =
            json!({ "op": "stroke", "ids": [b], "mode": "paint", "layer": 2, "radius": 24, "strength": 1, "points": [[2480.5, -840.25], [2480.5, -808.25]] });
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        ed.call("blend", args)
    };
    stroke(json!({}));
    let (p5, p6, p10, p11) =
        (weights([2480.5, 0.0, -840.25]), weights([2480.5, 0.0, -808.25]), weights([2480.5, 0.0, -680.25]), weights([2480.5, 0.0, -648.25]));
    assert!(p5[2].as_f64().unwrap() > 0.1, "blend painted {p5}");
    assert_eq!((p5, p6), (p11, p10), "the bar's z mirrors the stroke");
    stroke(json!({ "points": [[2288.5, -936.25], [2288.5, -904.25]], "symmetry": "off" }));
    assert!(weights([2288.5, 0.0, -936.25])[2].as_f64().unwrap() > 0.1);
    assert_eq!(weights([2288.5, 0.0, -552.25])[2].as_f64().unwrap(), 0.0, "off in the call beats the bar");

    // Guards. A refused call changes nothing, not even the undo list.
    let (rev, undo) = (revision(&ed), ed.state()["undo"].clone());
    let refused = |args: Value, want: &[&str]| {
        let msg = ed.call_err("terrain_edit", args.clone());
        for w in want {
            assert!(msg.contains(w), "{args} gave {msg}");
        }
    };
    let rect = json!({ "id": b, "op": "flatten_rect", "min": [2100, -900], "max": [2200, -800], "height": 20 });
    for (op, extra) in [
        ("flatten_rect", json!({})),
        ("ramp", json!({ "from": [2100, 0, -900], "to": [2200, 40, -800] })),
        ("erode", json!({})),
        ("auto_paint", json!({})),
        ("clear_layer", json!({ "layer": 1 })),
        ("resize", json!({ "north": 1 })),
        ("holes", json!({ "min": [2100, -900], "max": [2200, -800] })),
    ] {
        let mut args = rect.clone();
        args["op"] = json!(op);
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        args["symmetry"] = json!("x");
        refused(args, &["symmetry works with sculpt, sculpt_path, paint_path and holes by center", op]);
    }

    let sculpt_args = |symmetry: Value| json!({ "id": b, "op": "sculpt", "mode": "raise", "center": [2096.5, 0, -904.25], "symmetry": symmetry });
    refused(sculpt_args(json!("diagonal")), &["off, x, z or xz"]);
    refused(sculpt_args(json!({ "axes": "x", "plane": 3 })), &["symmetry must be", "plane"]);
    refused(sculpt_args(json!({ "axes": "x", "center": "middle" })), &["symmetry must be", "terrain, world or custom"]);
    refused(sculpt_args(json!(3)), &["off, x, z or xz"]);
    refused(json!({ "id": b, "op": "sculpt_paht", "symmetry": "x" }), &["unknown terrain op sculpt_paht"]);
    refused(json!({ "id": b, "probe": [2096.5, -904.25], "symmetry": "diagonal" }), &["off, x, z or xz"]);
    assert_eq!((revision(&ed), ed.state()["undo"].clone()), (rev, undo), "refused calls leave the map and the undo list alone");
    assert!(ed.call("terrain_edit", json!({ "id": b, "probe": [2096.5, -904.25], "symmetry": "x" }))["probe_height"].is_number());

    let msg = ed.call_err("set_editor", json!({ "symmetry": "diagonal", "grid": 8 }));
    assert!(msg.contains("off, x, z or xz"), "{msg}");
    ed.call("select", json!({ "ids": [a] }));
    let msg = ed.call_err("blend", json!({ "op": "dab", "ids": [b], "center": [2096.5, 0, -904.25], "symmetry": "diagonal" }));
    assert!(msg.contains("off, x, z or xz"), "{msg}");
    assert_eq!(ed.state()["selection"]["nodes"], json!([a]), "a refused blend keeps the selection");
    let msg = ed.call_err("run_action", json!({ "action": "sculpt", "args": { "center": [0, 0, 0], "mode": "raise", "symmetry": "q" } }));
    assert!(msg.contains("off, x, z or xz"), "{msg}");
    assert_eq!(symmetry(&ed)["axes"], "z", "a refused set_editor changed nothing");
    assert_ne!(ed.state()["editor"]["grid"], json!(8), "not even the grid");

    // The edge of the guard: off in any spelling goes through on the ops that refuse the rest.
    for off in [json!("off"), json!({ "axes": "off", "center": "custom", "x": 5, "z": 6 })] {
        let mut args = rect.clone();
        args["symmetry"] = off;
        assert_eq!(ed.call("terrain_edit", args)["changed"], true);
        ed.call("terrain_edit", json!({ "id": b, "op": "flatten_rect", "min": [2100, -900], "max": [2200, -800], "height": 0 }));
    }

    // One call is one undo step, mirrored dabs and all.
    let before = ed.state()["undo"].clone();
    let corners = [[2352.5, -584.25], [2160.5, -584.25], [2352.5, -904.25], [2160.5, -904.25]];
    let flat: Vec<f64> = corners.iter().map(|p| height(&ed, b, *p)).collect();
    raise(&ed, "sculpt", json!({ "center": [2352.5, 0, -584.25], "symmetry": "xz" }));
    let raised: Vec<f64> = corners.iter().map(|p| height(&ed, b, *p)).collect();
    assert!(raised.iter().zip(&flat).all(|(r, f)| *r > f + 1.0), "all four corners rose: {raised:?} over {flat:?}");
    let after = ed.state()["undo"].clone();
    assert_eq!((after[0].clone(), after[1].clone()), (json!("MCP: Terrain sculpt"), before[0].clone()));
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.state()["undo"], before);
    for (p, f) in corners.iter().zip(&flat) {
        assert!(close(height(&ed, b, *p), *f), "undone {p:?}");
    }
}

fn texel(uv: &Value, p: [f64; 3]) -> [f64; 2] {
    let axis = |k: &str| -> [f64; 3] {
        let a = uv[k].as_array().unwrap();
        [a[0].as_f64().unwrap(), a[1].as_f64().unwrap(), a[2].as_f64().unwrap()]
    };
    let (u, v) = (axis("u_axis"), axis("v_axis"));
    let s = |i: usize| uv["scale"][i].as_f64().unwrap();
    let o = |i: usize| uv["offset"][i].as_f64().unwrap();
    let dot = |a: [f64; 3]| a[0] * p[0] + a[1] * p[1] + a[2] * p[2];
    [dot(u) / s(0) + o(0), dot(v) / s(1) + o(1)]
}

/// Builds a box, selects its top face and picks the texture tool, returns the editor, the brush and the face index.
fn top_face_for_sliding(name: &'static str) -> (Editor, u64, u64) {
    let ed = Editor::launch(name);
    let brush = ed.box_brush([32.0, 0.0, 0.0], [160.0, 64.0, 64.0]);
    let all = ed.call("texture", json!({ "op": "get", "faces": (0..6).map(|f| json!([brush, f])).collect::<Vec<_>>() }));
    let top = all["faces"].as_array().unwrap().iter().find(|f| f["normal"][1].as_f64().unwrap() > 0.99).unwrap()["face"][1].as_u64().unwrap();
    ed.call("set_camera", json!({ "view": "top", "center": [96, 0, 32], "zoom": 1.5 }));
    ed.call("set_editor", json!({ "tool": "texture" }));
    ed.call("select", json!({ "faces": [[brush, top]] }));
    (ed, brush, top)
}

fn face_offset(ed: &Editor, brush: u64, face: u64) -> [f64; 2] {
    let uv = &ed.call("texture", json!({ "op": "get", "faces": [[brush, face]] }))["faces"][0]["uv"]["offset"];
    [uv[0].as_f64().unwrap(), uv[1].as_f64().unwrap()]
}

#[test]
#[ignore]
fn a_held_slide_drag_in_the_top_view_slides_by_what_was_dragged() {
    let (ed, brush, top) = top_face_for_sliding("held_slide_drag");
    let start = face_offset(&ed, brush, top);
    let drag = json!({ "type": "drag", "world": [64, 0, 16], "to_world": [96, 0, 32], "release": false });
    assert_eq!(ed.call("simulate_input", json!({ "target": "top", "events": [drag] }))["held"], true);
    std::thread::sleep(Duration::from_millis(500));
    let held = face_offset(&ed, brush, top);
    let mut moved = [(held[0] - start[0]).abs(), (held[1] - start[1]).abs()];
    moved.sort_by(f64::total_cmp);
    assert!(approx(&moved, &[16.0, 32.0]), "the held drag already slid the texture {start:?} {held:?}");

    ed.call("simulate_input", json!({ "target": "top", "events": [{ "type": "release" }] }));
    assert!(approx(&face_offset(&ed, brush, top), &held), "the release keeps the slide under the pointer");
}

#[test]
#[ignore]
fn a_one_shot_slide_drag_in_the_top_view_slides_by_what_was_dragged() {
    let (ed, brush, top) = top_face_for_sliding("one_shot_slide_drag");
    let start = face_offset(&ed, brush, top);
    ed.input("top", json!([{ "type": "drag", "world": [64, 0, 16], "to_world": [96, 0, 32] }]));
    let end = face_offset(&ed, brush, top);
    let mut moved = [(end[0] - start[0]).abs(), (end[1] - start[1]).abs()];
    moved.sort_by(f64::total_cmp);
    assert!(approx(&moved, &[16.0, 32.0]), "{start:?} {end:?}");
}

#[test]
#[ignore]
fn texture_tool_apply_wrap_slide_scale() {
    let ed = Editor::launch("texture");
    let brush = ed.box_brush([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]);
    let all = ed.call("texture", json!({ "op": "get", "faces": (0..6).map(|f| json!([brush, f])).collect::<Vec<_>>() }));
    let face_with = |n: [f64; 3]| {
        let found = all["faces"].as_array().unwrap().iter().find(|f| (0..3).all(|i| (f["normal"][i].as_f64().unwrap() - n[i]).abs() < 1e-6)).unwrap();
        found["face"][1].as_u64().unwrap()
    };
    let (front, left) = (face_with([0.0, 0.0, -1.0]), face_with([-1.0, 0.0, 0.0]));
    ed.call("set_camera", json!({ "view": "3d", "position": [-110, 110, -150], "look_at": [32, 32, 32] }));
    ed.call("set_editor", json!({ "tool": "texture", "material": "dev/orange" }));
    assert_eq!(ed.state()["editor"]["tool"], "Texture");

    // Click selects the front face, right click on the left face applies the current material there.
    ed.input("3d", json!([{ "type": "click", "world": [32, 32, 0] }]));
    assert_eq!(ed.state()["selection"]["faces"], json!([[brush, front]]));
    ed.input("3d", json!([{ "type": "click", "world": [0, 32, 32], "button": "right" }]));
    let get = |f: u64| ed.call("texture", json!({ "op": "get", "faces": [[brush, f]] }))["faces"][0].clone();
    assert_eq!(get(left)["material"], "dev/orange");
    assert_ne!(get(front)["material"], "dev/orange", "right click does not touch the selection");

    // Dragging the selected face slides its texture by the dragged distance.
    let before = get(front)["uv"]["offset"][0].as_f64().unwrap();
    ed.input("3d", json!([{ "type": "drag", "world": [32, 32, 0], "to_world": [48, 32, 0], "steps": 10 }]));
    let after = get(front)["uv"]["offset"][0].as_f64().unwrap();
    assert!(((after - before).abs() - 16.0).abs() <= 1.0, "slid by {}", after - before);

    // Alt+right click wraps the selected face's alignment around the corner.
    ed.input("3d", json!([{ "type": "click", "world": [0, 32, 32], "button": "right", "modifiers": ["alt"] }]));
    let (f_uv, l_uv) = (get(front)["uv"].clone(), get(left)["uv"].clone());
    for y in [0.0, 20.0, 64.0] {
        let (a, b) = (texel(&f_uv, [0.0, y, 0.0]), texel(&l_uv, [0.0, y, 0.0]));
        assert!((a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6, "seamless at the corner: {a:?} {b:?}");
    }

    // Ctrl+wheel doubles the texture size of the selected face.
    let scale = f_uv["scale"][0].as_f64().unwrap().abs();
    ed.input("3d", json!([{ "type": "scroll", "world": [32, 32, 0], "delta": 120, "modifiers": ["ctrl"] }]));
    assert!((get(front)["uv"]["scale"][0].as_f64().unwrap().abs() - scale * 2.0).abs() < 1e-9);

    // Alt+click picks material and alignment, justify aligns the texture to the face edge.
    ed.input("3d", json!([{ "type": "click", "world": [0, 32, 32], "modifiers": ["alt"] }]));
    assert_eq!(ed.state()["editor"]["material"], "dev/orange");
    let justified = ed.call("texture", json!({ "op": "justify", "mode": "left", "faces": [[brush, front]] }));
    assert_eq!(justified["changed"], 1);
    let uv = get(front)["uv"].clone();
    let min_u = [[0.0, 0.0, 0.0], [64.0, 0.0, 0.0]].iter().map(|p| texel(&uv, *p)[0]).fold(f64::MAX, f64::min);
    assert!(min_u.abs() < 1e-6, "left justified, min u {min_u}");
    ed.screenshot("3d", "texture_tool");

    // Mesh UV projections write explicit UVs, hotspots need a texture file.
    let mesh = ed.call("create_mesh", json!({ "shape": "cylinder", "min": [128, 0, 0], "max": [192, 64, 64] }))["id"].as_u64().unwrap();
    let projected = ed.call("texture", json!({ "op": "mesh_uv", "kind": "cylinder" }));
    assert!(projected["changed"].as_u64().unwrap() >= 16);
    assert!(ed.call("texture", json!({ "op": "get", "faces": [[mesh, 2]] }))["faces"][0]["explicit"].is_array());
    assert!(ed.call_err("texture", json!({ "op": "set_hotspots", "material": "dev/grey", "rects": [[0, 0, 64, 64]] })).contains("no texture file"));
    ed.call("set_editor", json!({ "shade": "lit" }));
    let (_, _, colors) = ed.screenshot("3d", "lit_sky");
    assert!(colors > 20, "lit view with sky");
}

#[test]
#[ignore]
fn texture_tool_wheel_acts_on_selected_brushes() {
    let ed = Editor::launch("texture_brushes");
    let a = ed.call("create_brush", json!({ "min": [96, 32, -160], "max": [224, 96, -64], "shape": "wedge" }))["ids"][0].as_u64().unwrap();
    let b = ed.call("create_brush", json!({ "min": [-288, 64, 128], "max": [-160, 192, 224], "shape": "wedge" }))["ids"][0].as_u64().unwrap();
    ed.call("set_camera", json!({ "view": "3d", "position": [0, 300, 400], "look_at": [0, 64, 0] }));
    ed.call("set_editor", json!({ "tool": "texture" }));
    let scales = |id: u64| -> Vec<(u64, f64, f64)> {
        let got = ed.call("texture", json!({ "op": "get", "faces": (0..16).map(|f| json!([id, f])).collect::<Vec<_>>() }));
        got["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f["face"][1].as_u64().unwrap(), f["uv"]["scale"][0].as_f64().unwrap(), f["uv"]["scale"][1].as_f64().unwrap()))
            .collect()
    };
    let wheel = || ed.input("3d", json!([{ "type": "scroll", "world": [0, 64, 0], "delta": 120, "modifiers": ["ctrl"] }]));
    let (base_a, base_b) = (scales(a), scales(b));
    assert!(base_a.len() >= 5 && base_b.len() >= 5);

    // Nothing selected, nothing changes.
    ed.call("select", json!({ "mode": "clear" }));
    wheel();
    assert_eq!(scales(a), base_a);
    assert_eq!(scales(b), base_b);

    // Two brushes selected without any faces, every face of both doubles in one undo step.
    ed.call("select", json!({ "ids": [a, b] }));
    wheel();
    let twice = |base: &[(u64, f64, f64)]| base.iter().map(|(f, x, y)| (*f, x * 2.0, y * 2.0)).collect::<Vec<_>>();
    assert_eq!(scales(a), twice(&base_a));
    assert_eq!(scales(b), twice(&base_b));
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(scales(a), base_a);
    assert_eq!(scales(b), base_b);

    // One face selected, only that face changes.
    ed.call("select", json!({ "faces": [[a, 1]] }));
    wheel();
    let mut want = base_a.clone();
    want[1] = (want[1].0, want[1].1 * 2.0, want[1].2 * 2.0);
    assert_eq!(scales(a), want);
    assert_eq!(scales(b), base_b);
}

/// Replays examples/mcp/logic_playground.json and follows the wiring of its math and logic nodes with the simulation.
fn replay_logic_playground(ed: &Editor, root: &std::path::Path, out: &std::path::Path) {
    let path = root.join("examples/mcp/logic_playground.json").canonicalize().unwrap();
    let summary = ed.call("run_script", json!({ "path": path, "vars": { "save_dir": out } }));
    assert_eq!(summary["errors"], json!([]), "logic_playground");
    assert_eq!(summary["ran"], summary["steps"], "logic_playground");
    assert_eq!(ed.state()["map"]["cameras"], json!([1, 2, 3]), "logic_playground");
    let issues = ed.call("validate_map", json!({}));
    let errors: Vec<&Value> = issues["issues"].as_array().unwrap().iter().filter(|i| i["severity"] == "error").collect();
    assert!(errors.is_empty(), "logic_playground: {errors:?}");

    let saved = td_doc::format::load(&out.join("logic_playground.tdm")).unwrap();
    assert!(saved.problems.is_empty(), "logic_playground: {:?}", saved.problems);
    let map = saved.map;
    for class in ["math_value", "math_calc", "math_compare", "logic_gate", "logic_random", "logic_case", "logic_flipflop"] {
        assert!(map.entities().any(|(_, e)| e.classname == class), "the playground uses {class}");
    }

    let coin = map.find_by_targetname("coin_1")[0];
    let cascade = td_editor::logic_sim::simulate(&map, &td_formats::GameConfig::with_gameplay_pack(), coin, "pressed");
    let links: Vec<String> = cascade.events.iter().map(|e| format!("{}.{}>{}.{}", e.source_name, e.output, e.target, e.input)).collect();
    for link in [
        "coin_1.pressed>score.add",
        "score.changed>points.calculate",
        "points.result>points_text.set_text",
        "score.changed>enough.set_and_compare",
        "enough.on_greater>door_gate.set_a",
        "door_gate.on_true>vault_door.open",
        "door_gate.on_false>vault_door.close",
    ] {
        assert!(links.iter().any(|l| l == link), "pressing a coin reaches {link}: {links:?}");
    }

    assert_eq!(cascade.broken(), 0, "every connection of the playground reaches an entity");
    let draw = map.find_by_targetname("draw_button")[0];
    let lottery = td_editor::logic_sim::simulate(&map, &td_formats::GameConfig::with_gameplay_pack(), draw, "pressed");
    let links: Vec<String> = lottery.events.iter().map(|e| format!("{}.{}>{}.{}", e.source_name, e.output, e.target, e.input)).collect();
    for link in ["picker.out_2>green_light.turn_on", "picker.picked>color_case.in_value", "color_case.on_case_3>lottery_text.set_text"] {
        assert!(links.iter().any(|l| l == link), "drawing a light reaches {link}: {links:?}");
    }

    assert_eq!(lottery.broken(), 0);
}

#[test]
#[ignore]
fn logic_playground_script_replays() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let project = root.join("godot").canonicalize().unwrap();
    let ed = Editor::launch_with("logic_playground", &["--project", project.to_str().unwrap()]);
    let out = artifacts().join("logic_playground");
    std::fs::create_dir_all(&out).unwrap();
    let path = root.join("examples/mcp/sealed_rooms.json").canonicalize().unwrap();
    let summary = ed.call("run_script", json!({ "path": path, "vars": { "save_dir": out } }));
    assert_eq!(summary["errors"], json!([]), "sealed_rooms");
    assert_eq!(summary["ran"], summary["steps"], "sealed_rooms");
    ed.call("validate_map", json!({}));
    let cull = ed.state()["map"]["void_cull"].clone();
    assert_eq!(cull["state"], "sealed", "sealed_rooms: {cull}");
    assert!(cull["dropped_faces"].as_u64().unwrap() > 0, "sealed_rooms: {cull}");

    replay_logic_playground(&ed, &root, &out);
}

/// A script shared next to a project of its own edits and saves the map, and nothing else: it does not switch to that
/// project, start Godot, touch the clipboard, close tabs or undo the person's work in them, or read and write outside
/// the open project, its own folder and the folders the caller passes in, also through links.
#[test]
#[ignore]
fn shared_scripts_only_edit_the_map() {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../godot").canonicalize().unwrap();
    let shared = artifacts().join("shared_script");
    let out = artifacts().join("shared_script_out");
    for dir in [&shared, &out] {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
    }

    std::fs::write(shared.join("project.godot"), "config_version=5\n").unwrap();
    td_doc::format::save(&td_doc::Map::new(), &shared.join("pack.tdm")).unwrap();
    let escaped = artifacts().join("shared_script_escaped.tdm");
    let _ = std::fs::remove_file(&escaped);
    let script = json!({ "format": "trenchdot-mcp-script", "steps": [
        { "tool": "create_brush", "args": { "min": [0, 0, 0], "max": [64, 64, 64] } },
        { "tool": "open_project", "args": { "path": "${script_dir}" } },
        { "tool": "map_file", "args": { "op": "open", "path": "${script_dir}/pack.tdm" } },
        { "tool": "run_action", "args": { "action": "run_godot_project" } },
        { "tool": "run_action", "args": { "action": "build_in_godot" } },
        { "tool": "run_action", "args": { "action": "reload_game_config" } },
        { "tool": "set_editor", "args": { "live_mode": true } },
        { "tool": "map_file", "args": { "op": "save", "path": "${script_dir}/../shared_script_escaped.tdm" } },
        { "tool": "map_file", "args": { "op": "export_map", "path": escaped.with_extension("map") } },
        { "tool": "map_file", "args": { "op": "save", "path": "${script_dir}/castle.tdm" } },
        { "tool": "map_file", "args": { "op": "export_map", "path": "${out}/castle.map" } }
    ] });
    std::fs::write(shared.join("castle.json"), script.to_string()).unwrap();

    let ed = Editor::launch_with("shared_script", &["--project", project.to_str().unwrap()]);
    let summary = ed.call("run_script", json!({ "path": shared.join("castle.json"), "vars": { "out": out }, "continue_on_error": true }));
    let refused: Vec<u64> = summary["errors"].as_array().unwrap().iter().map(|e| e["step"].as_u64().unwrap()).collect();
    assert_eq!(refused, [1, 2, 3, 4, 5, 6, 7, 8], "{}", summary["errors"]);
    assert_eq!(summary["ran"], 3);
    let error = |step: u64| summary["errors"].as_array().unwrap().iter().find(|e| e["step"] == step).unwrap()["error"].as_str().unwrap().to_string();
    assert!(error(2).contains("another project"), "{}", error(2));
    assert!(error(3).contains("run_godot_project cannot run inside scripts"), "{}", error(3));
    assert!(error(7).contains("cannot write"), "{}", error(7));
    assert!(shared.join("castle.tdm").is_file() && out.join("castle.map").is_file(), "the script's folder and a folder passed in vars take writes");
    assert!(!escaped.exists() && !escaped.with_extension("map").exists(), "nothing lands outside them");
    let state = ed.state();
    assert_eq!(std::path::Path::new(state["game"]["project_root"].as_str().unwrap()).canonicalize().unwrap(), project, "the project stays");
    assert_eq!(state["godot"]["live_mode"], false);

    // A save without a path goes back to the map the script started on, never to another map that is open.
    let (mine, theirs) = (artifacts().join("shared_script_mine.tdm"), artifacts().join("shared_script_theirs.tdm"));
    for path in [&mine, &theirs] {
        td_doc::format::save(&td_doc::Map::new(), path).unwrap();
        let _ = std::fs::remove_file(path.with_extension("tdm.bak"));
    }

    ed.call("map_file", json!({ "op": "open", "path": theirs }));
    ed.call("map_file", json!({ "op": "open_tab", "path": mine }));
    let brush = json!({ "tool": "create_brush", "args": { "min": [0, 0, 0], "max": [64, 64, 64] } });
    let script = json!({ "steps": [
        brush,
        { "tool": "map_file", "args": { "op": "save" } },
        { "tool": "run_action", "args": { "action": "next_tab" } },
        brush,
        { "tool": "map_file", "args": { "op": "save" } },
        { "tool": "run_action", "args": { "action": "save" } },
        { "tool": "map_file", "args": { "op": "open", "path": theirs } },
        { "tool": "run_action", "args": { "action": "next_tab" } },
        { "tool": "map_file", "args": { "op": "export_map", "path": mine } }
    ] });
    std::fs::write(shared.join("tabs.json"), script.to_string()).unwrap();
    let summary = ed.call("run_script", json!({ "path": shared.join("tabs.json"), "continue_on_error": true }));
    let refused: Vec<u64> = summary["errors"].as_array().unwrap().iter().map(|e| e["step"].as_u64().unwrap()).collect();
    assert_eq!(refused, [4, 5, 6, 8], "{}", summary["errors"]);
    let brushes = |path: &PathBuf| td_doc::format::load(path).unwrap().map.nodes.values().filter(|n| matches!(n.kind, td_doc::NodeKind::Brush(_))).count();
    assert_eq!((brushes(&mine), brushes(&theirs)), (1, 0));
    assert!(!theirs.with_extension("tdm.bak").exists());

    // Reads stay in the same folders. What an imported map points to elsewhere is left out, and the clipboard and the
    // other tabs are left alone.
    let outside = artifacts().join("shared_script_outside");
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    td_doc::format::save(&td_doc::Map::new(), &outside.join("secret.tdm")).unwrap();
    std::fs::write(outside.join("far.wad"), "WAD2").unwrap();
    std::fs::write(outside.join("box.obj"), "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
    std::fs::write(outside.join("inst.vmf"), "world { \"id\" \"1\" \"classname\" \"worldspawn\" }\nentity { \"id\" \"2\" \"classname\" \"info_target\" }\n")
        .unwrap();
    let instance = "entity { \"id\" \"2\" \"classname\" \"func_instance\" \"file\" \"../shared_script_outside/inst.vmf\" \"origin\" \"0 0 0\" }";
    std::fs::write(shared.join("level.vmf"), format!("world {{ \"id\" \"1\" \"classname\" \"worldspawn\" }}\n{instance}\n")).unwrap();
    std::fs::write(shared.join("wads.map"), "{\n\"classname\" \"worldspawn\"\n\"wad\" \"../shared_script_outside/far.wad\"\n}\n").unwrap();
    let away = "${script_dir}/../shared_script_outside";
    let tabs = ed.state()["tabs"]["titles"].as_array().unwrap().len();
    let script = json!({ "steps": [
        { "tool": "map_file", "args": { "op": "import_map", "path": format!("{away}/x.map") } },
        { "tool": "map_file", "args": { "op": "import_map", "path": "${script_dir}/wads.map", "textures": format!("{away}/far.wad") } },
        { "tool": "map_file", "args": { "op": "convert_textures", "path": format!("{away}/far.wad") } },
        { "tool": "import_model", "args": { "path": format!("{away}/box.obj") } },
        { "tool": "validate_map", "args": { "path": format!("{away}/secret.tdm") } },
        { "tool": "changes_since", "args": { "file": format!("{away}/secret.tdm") } },
        { "tool": "run_action", "args": { "action": "insert_prefab", "args": { "path": outside.join("secret.tdm") } } },
        { "tool": "map_file", "args": { "op": "open", "path": "//td-no-such-host/share/x.tdm" } },
        { "tool": "run_action", "args": { "action": "copy" } },
        { "tool": "run_action", "args": { "action": "close_tab", "args": { "discard": true } } },
        { "tool": "run_action", "args": { "action": "close_tab", "args": { "close_others": true } } },
        { "tool": "map_file", "args": { "op": "import_vmf", "path": "${script_dir}/level.vmf" }, "save": "vmf" },
        { "tool": "map_file", "args": { "op": "import_map", "path": "${script_dir}/wads.map" }, "save": "wads" },
        { "tool": "map_file", "args": { "op": "import_vmf", "path": format!("{away}/inst.vmf") } }
    ] });
    std::fs::write(shared.join("reads.json"), script.to_string()).unwrap();
    let summary = ed.call("run_script", json!({ "path": shared.join("reads.json"), "continue_on_error": true }));
    let refused: Vec<u64> = summary["errors"].as_array().unwrap().iter().map(|e| e["step"].as_u64().unwrap()).collect();
    assert_eq!(refused, (0..=10).chain([13]).collect::<Vec<_>>(), "{}", summary["errors"]);
    let error = |step: u64| summary["errors"].as_array().unwrap().iter().find(|e| e["step"] == step).unwrap()["error"].as_str().unwrap().to_string();
    for step in (0..=6).chain([13]) {
        assert!(error(step).contains("cannot read"), "{}", error(step));
    }

    assert!(error(7).contains("network path"), "{}", error(7));
    assert!(error(8).contains("clipboard") && error(9).contains("discard") && error(10).contains("other tabs"), "{}", summary["errors"]);
    assert_eq!(summary["vars"]["vmf"]["missing_instances"], json!(["../shared_script_outside/inst.vmf"]), "an instance elsewhere is left out");
    let sources = summary["vars"]["wads"]["texture_sources"].to_string();
    assert!(!sources.contains("far.wad"), "a WAD elsewhere is not read: {sources}");
    assert!(ed.state()["tabs"]["titles"].as_array().unwrap().len() >= tabs, "no tab was closed");

    // A texture folder that links out of the project takes no textures from a script.
    let linked = artifacts().join("shared_script_linked");
    let elsewhere = artifacts().join("shared_script_linked_textures");
    for dir in [&linked, &elsewhere] {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
    }

    std::fs::write(linked.join("project.godot"), "config_version=5\n").unwrap();
    assert!(link_dir(&linked.join("textures"), &elsewhere), "link the texture folder out of the project");
    std::fs::copy(outside.join("far.wad"), linked.join("evil.wad")).unwrap();
    ed.call("open_project", json!({ "path": linked }));
    let script = json!({ "steps": [
        { "tool": "map_file", "args": { "op": "convert_textures", "path": "${script_dir}/evil.wad", "overwrite": true } },
        { "tool": "map_file", "args": { "op": "import_map", "path": "${script_dir}/wads.map", "textures": "auto" } },
        { "tool": "map_file", "args": { "op": "import_vmf", "path": "${script_dir}/level.vmf", "textures": "auto" } }
    ] });
    std::fs::write(linked.join("textures.json"), script.to_string()).unwrap();
    std::fs::copy(shared.join("wads.map"), linked.join("wads.map")).unwrap();
    std::fs::copy(shared.join("level.vmf"), linked.join("level.vmf")).unwrap();
    let summary = ed.call("run_script", json!({ "path": linked.join("textures.json"), "continue_on_error": true }));
    let errors = summary["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 3, "{}", summary["errors"]);
    assert!(errors.iter().all(|e| e["error"].as_str().unwrap().contains("cannot write textures into")), "{}", summary["errors"]);
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&linked);

    // Links inside the folders do not lead out of them either: not to a prefab, not to a file a model names, and not
    // for the files an import or export writes. Prefabs that the script's maps and pasted text name are left unread.
    let links = artifacts().join("shared_script_links");
    let away = artifacts().join("shared_script_links_away");
    for dir in [&links, &away] {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
    }

    let pack = links.join("pack");
    for dir in ["models", "prefabs", "obj", "files"].map(|d| away.join(d)).iter().chain([&pack, &links.join("maps"), &links.join("textures/models")]) {
        std::fs::create_dir_all(dir).unwrap();
    }

    std::fs::write(links.join("project.godot"), "config_version=5\n").unwrap();
    assert!(link_dir(&pack.join("junc"), &away.join("files")));
    let png = |size: u32, rgba: [u8; 4], path: &std::path::Path| image::RgbaImage::from_pixel(size, size, image::Rgba(rgba)).save(path).unwrap();
    png(8, [1, 2, 3, 255], &away.join("files/secret.png"));
    png(4, [200, 80, 40, 255], &links.join("textures/brick.png"));
    std::fs::write(pack.join("lnk.mtl"), "newmtl m\nKd 1 1 1\nmap_Kd junc/secret.png\n").unwrap();
    std::fs::write(pack.join("lnk.obj"), "mtllib lnk.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nusemtl m\nf 1/1 2/2 3/3\n").unwrap();
    let mut secret = td_doc::Map::new();
    let layer = secret.default_layer();
    let cube = td_geom::Brush::from_aabb(&td_core::Aabb::new(td_core::DVec3::ZERO, td_core::DVec3::splat(64.0)), "brick").unwrap();
    secret.insert(layer, td_doc::NodeKind::Brush(cube));
    td_doc::format::save(&secret, &away.join("files/secret.tdm")).unwrap();
    let outside_prefab = away.join("files/secret.tdm").to_string_lossy().replace('\\', "/");
    let mut refers = td_doc::Map::new();
    let layer = refers.default_layer();
    let instance = td_doc::map::Instance { path: outside_prefab.clone(), origin: td_core::DVec3::ZERO, angles: td_core::DVec3::ZERO, fixup: String::new() };
    refers.insert(layer, td_doc::NodeKind::Instance(instance));
    td_doc::format::save(&refers, &pack.join("refers.tdm")).unwrap();
    let pasted = json!({ "format": "trenchdot-clipboard", "version": 1, "nodes": [
        { "id": 2, "type": "instance", "path": outside_prefab, "origin": [0, 0, 0], "angles": [0, 0, 0] }
    ] });
    ed.call("open_project", json!({ "path": links }));
    let script = json!({ "steps": [
        { "tool": "run_action", "args": { "action": "paste", "args": { "text": pasted.to_string() } } },
        { "tool": "map_file", "args": { "op": "open", "path": "${script_dir}/refers.tdm" } },
        { "tool": "run_action", "args": { "action": "select_all" } },
        { "tool": "run_action", "args": { "action": "explode_instances" } },
        { "tool": "map_file", "args": { "op": "save", "path": "${script_dir}/loot.tdm" } },
        { "tool": "import_model", "args": { "path": "${script_dir}/lnk.obj", "mode": "mesh" } }
    ] });
    std::fs::write(pack.join("reads.json"), script.to_string()).unwrap();
    let summary = ed.call("run_script", json!({ "path": pack.join("reads.json"), "continue_on_error": true }));
    let refused: Vec<u64> = summary["errors"].as_array().unwrap().iter().map(|e| e["step"].as_u64().unwrap()).collect();
    assert_eq!(refused, [0, 3], "{}", summary["errors"]);
    let error = |step: u64| summary["errors"].as_array().unwrap().iter().find(|e| e["step"] == step).unwrap()["error"].as_str().unwrap().to_string();
    assert!(error(0).contains("cannot paste an instance"), "{}", error(0));
    assert!(error(3).contains("Cannot load prefab"), "{}", error(3));
    let loot = td_doc::format::load(&pack.join("loot.tdm")).unwrap().map;
    assert!(!loot.nodes.values().any(|n| matches!(n.kind, td_doc::NodeKind::Brush(_))), "nothing of the prefab outside got into the map");
    let texture = image::open(links.join("textures/models/lnk/tex0.png")).unwrap();
    assert_eq!(texture.width(), 4, "the model's color, not the image its material names through the link");

    // The person's unsaved work in another tab stays, undo and redo only reach what the script did there.
    ed.call("run_action", json!({ "action": "close_tab", "args": { "close_others": true, "discard": true } }));
    ed.call("run_action", json!({ "action": "close_tab", "args": { "discard": true } }));
    let start = links.join("maps/start.tdm");
    td_doc::format::save(&td_doc::Map::new(), &start).unwrap();
    ed.call("map_file", json!({ "op": "open", "path": start }));
    for x in [0, 128] {
        ed.call("create_brush", json!({ "min": [x, 0, 0], "max": [x + 64, 64, 64] }));
    }

    ed.call("map_file", json!({ "op": "new" }));
    let script = json!({ "steps": [
        { "tool": "run_action", "args": { "action": "next_tab" } },
        { "tool": "run_action", "args": { "action": "undo", "args": { "steps": 50 } } },
        { "tool": "run_action", "args": { "action": "undo" } },
        { "tool": "create_brush", "args": { "min": [256, 0, 0], "max": [320, 64, 64] } },
        { "tool": "run_action", "args": { "action": "undo" } },
        { "tool": "run_action", "args": { "action": "redo" } },
        { "tool": "run_action", "args": { "action": "redo" } },
        { "tool": "run_action", "args": { "action": "undo", "args": { "steps": "2" } } }
    ] });
    std::fs::write(pack.join("undo.json"), script.to_string()).unwrap();
    let summary = ed.call("run_script", json!({ "path": pack.join("undo.json"), "continue_on_error": true }));
    let refused: Vec<u64> = summary["errors"].as_array().unwrap().iter().map(|e| e["step"].as_u64().unwrap()).collect();
    assert_eq!(refused, [1, 2, 6, 7], "{}", summary["errors"]);
    assert_eq!(ed.brushes(), 3);
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.brushes(), 2, "the person's two brushes are still there");

    // Writes through links: the model's textures, a prefab library entry and the files an OBJ export puts next to it.
    std::fs::remove_dir_all(links.join("textures/models")).unwrap();
    for (link, target) in [("textures/models", "models"), ("prefab_library", "prefabs"), ("pack/out_textures", "obj")] {
        assert!(link_dir(&links.join(link), &away.join(target)), "{link}");
    }

    ed.call("run_action", json!({ "action": "reload_materials" }));
    let script = json!({ "steps": [
        { "tool": "import_model", "args": { "path": "${script_dir}/lnk.obj", "mode": "mesh" } },
        { "tool": "create_brush", "args": { "min": [0, 0, 0], "max": [64, 64, 64], "material": "brick" } },
        { "tool": "run_action", "args": { "action": "select_all" } },
        { "tool": "prefab_library", "args": { "op": "add", "name": "loot", "category": "x" } },
        { "tool": "map_file", "args": { "op": "export_obj", "path": "${script_dir}/out.obj" } }
    ] });
    std::fs::write(pack.join("writes.json"), script.to_string()).unwrap();
    let summary = ed.call("run_script", json!({ "path": pack.join("writes.json"), "continue_on_error": true }));
    let refused: Vec<u64> = summary["errors"].as_array().unwrap().iter().map(|e| e["step"].as_u64().unwrap()).collect();
    assert_eq!(refused, [3, 4], "{}", summary["errors"]);
    assert!(summary["errors"].as_array().unwrap().iter().all(|e| e["error"].as_str().unwrap().contains("scripts cannot write")), "{}", summary["errors"]);
    let mut left = files_below(&away);
    left.sort();
    assert_eq!(left, [away.join("files/secret.png"), away.join("files/secret.tdm")], "nothing was written through the links");
    assert!(!pack.join("out.obj").exists() && !pack.join("out.mtl").exists());

    // Converted textures named by where they lie keep their folders, also one that links out of the texture folder.
    std::fs::create_dir_all(away.join("sub")).unwrap();
    assert!(link_dir(&links.join("textures/sub"), &away.join("sub")));
    std::fs::create_dir_all(pack.join("src/textures/sub")).unwrap();
    png(4, [9, 9, 9, 255], &pack.join("src/textures/sub/x.png"));
    let script = json!({ "steps": [
        { "tool": "map_file", "args": { "op": "convert_textures", "path": "${script_dir}/src", "overwrite": true }, "save": "converted" }
    ] });
    std::fs::write(pack.join("convert.json"), script.to_string()).unwrap();
    let summary = ed.call("run_script", json!({ "path": pack.join("convert.json"), "continue_on_error": true }));
    let failed = summary["vars"]["converted"]["textures"]["failed"].to_string();
    assert!(failed.contains("sub/x") && failed.contains("scripts cannot write"), "{failed}");
    assert!(files_below(&away.join("sub")).is_empty(), "the texture stays out of the linked folder");
    let _ = std::fs::remove_dir_all(&links);
}

/// A fresh project whose folder `link` links to a fresh folder outside it, which is returned second.
fn project_with_link(name: &str, link: &str) -> (PathBuf, PathBuf) {
    let (project, away) = (artifacts().join(name), artifacts().join(format!("{name}_away")));
    for dir in [&project, &away] {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
    }

    std::fs::write(project.join("project.godot"), "config_version=5\n").unwrap();
    let link = project.join(link);
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    assert!(link_dir(&link, &away), "link {} out of the project", link.display());
    (project, away)
}

/// Runs `steps` as a script saved in `project`, going on past errors.
fn run_steps(ed: &Editor, project: &std::path::Path, steps: Value) -> Value {
    std::fs::write(project.join("steps.json"), json!({ "steps": steps }).to_string()).unwrap();
    ed.call("run_script", json!({ "path": project.join("steps.json"), "continue_on_error": true }))
}

#[test]
#[ignore]
fn script_random_fill_takes_no_pieces_through_a_link() {
    let (project, away) = project_with_link("script_fill_linked", "prefab_library");
    std::fs::create_dir_all(away.join("kit")).unwrap();
    let mut piece = td_doc::Map::new();
    let layer = piece.default_layer();
    let cube = td_geom::Brush::from_aabb(&td_core::Aabb::new(td_core::DVec3::ZERO, td_core::DVec3::splat(64.0)), "brick").unwrap();
    piece.insert(layer, td_doc::NodeKind::Brush(cube));
    td_doc::format::save(&piece, &away.join("kit/block.tdm")).unwrap();
    let ed = Editor::launch("script_fill_linked");
    ed.call("open_project", json!({ "path": project }));
    let fill = json!({ "pieces": [{ "name": "Kit/Block" }], "cells": [2, 2], "cell": 64 });
    let summary = run_steps(&ed, &project, json!([{ "tool": "random_fill", "args": fill }]));
    let errors = summary["errors"].to_string();
    assert!(errors.contains("outside the folders the running script may read"), "{errors}");
    assert_eq!(ed.brushes(), 0);
    ed.call("random_fill", fill);
    assert!(ed.brushes() > 0, "outside a script the linked library fills");
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
#[ignore]
fn script_scatter_preset_writes_no_models_through_a_link() {
    let (project, away) = project_with_link("script_preset_linked", "trenchdot/nature");
    let ed = Editor::launch("script_preset_linked");
    ed.call("open_project", json!({ "path": project }));
    let preset = json!({ "op": "preset", "preset": "low-poly trees" });
    let summary = run_steps(&ed, &project, json!([{ "tool": "scatter", "args": preset }]));
    let errors = summary["errors"].to_string();
    assert!(errors.contains("scripts cannot write"), "{errors}");
    assert!(files_below(&away).is_empty(), "no model was written through the link");
    ed.call("scatter", preset);
    assert!(away.join("pine.bbmodel").is_file(), "outside a script the preset adds its models");
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
#[ignore]
fn script_material_reads_stay_out_of_linked_textures() {
    let (project, away) = project_with_link("script_material_linked", "textures");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255])).save(away.join("wall.png")).unwrap();
    std::fs::write(away.join("wall.hotspots.json"), r#"{"rects": [[0, 0, 4, 4]]}"#).unwrap();
    std::fs::write(
        away.join("wall.tres"),
        "[gd_resource type=\"StandardMaterial3D\" format=3]\n[resource]\ntransparency = 1\nmetadata/texture_size = Vector2(128, 64)\n",
    )
    .unwrap();
    let ed = Editor::launch("script_material_linked");
    ed.call("open_project", json!({ "path": project }));
    let info = json!({ "op": "material_info", "material": "wall" });
    let direct = ed.call("texture", info.clone());
    assert_eq!((&direct["size"], &direct["hotspots"]), (&json!([8, 8]), &json!([[0.0, 0.0, 4.0, 4.0]])), "{direct}");
    assert_eq!((&direct["transparent"], &direct["world_size"]), (&json!(true), &json!([128.0, 64.0])), "{direct}");
    let summary = run_steps(
        &ed,
        &project,
        json!([
            { "tool": "texture", "args": info, "save": "info" },
            { "tool": "command", "args": { "action": "reload_materials" } },
            { "tool": "texture", "args": info, "save": "rescanned" },
        ]),
    );
    let fenced = &summary["vars"]["info"];
    assert_eq!((&fenced["size"], &fenced["hotspots"]), (&Value::Null, &json!([])), "{summary}");
    assert_eq!((&fenced["transparent"], &fenced["world_size"]), (&json!(false), &Value::Null), "the material file is not read either");
    assert_eq!(summary["vars"]["rescanned"]["world_size"], Value::Null, "a rescan inside the script reads no linked material");
    assert!(summary["unread_textures"].as_array().is_some_and(|u| !u.is_empty()), "the summary names what went unread: {summary}");
    assert_eq!(ed.call("texture", info)["world_size"], json!([128.0, 64.0]), "the library reads the linked folder again after the script");
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
#[ignore]
fn script_prefab_library_lists_no_linked_entries() {
    let (project, away) = project_with_link("script_library_linked", "prefab_library/kit");
    let mut piece = td_doc::Map::new();
    let layer = piece.default_layer();
    let slab = td_core::Aabb::new(td_core::DVec3::new(-48.0, 0.0, -16.0), td_core::DVec3::new(48.0, 24.0, 40.0));
    piece.insert(layer, td_doc::NodeKind::Brush(td_geom::Brush::from_aabb(&slab, "brick").unwrap()));
    td_doc::format::save(&piece, &away.join("block.tdm")).unwrap();
    std::fs::create_dir_all(project.join("prefab_library/kept")).unwrap();
    td_doc::format::save(&piece, &project.join("prefab_library/kept/arch.tdm")).unwrap();
    let ed = Editor::launch("script_library_linked");
    ed.call("open_project", json!({ "path": project }));
    let list = json!({ "op": "list" });
    let names = |reply: &Value| -> Vec<String> {
        let entries = reply["entries"].as_array().unwrap_or_else(|| panic!("{reply}"));
        entries.iter().filter(|e| e["source"] == "project").map(|e| e["name"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(names(&ed.call("prefab_library", list.clone())), ["Kept/Arch", "Kit/Block"]);
    let summary = run_steps(
        &ed,
        &project,
        json!([
            { "tool": "prefab_library", "args": list, "save": "before" },
            { "tool": "create_brush", "args": { "min": [32, 0, 72], "max": [104, 40, 88] }, "save": "ledge" },
            { "tool": "prefab_library", "args": { "op": "add", "ids": "$ledge.ids", "category": "kept", "name": "ledge" } },
            { "tool": "prefab_library", "args": list, "save": "after" },
        ]),
    );
    assert_eq!(summary["errors"], json!([]), "{summary}");
    assert_eq!(names(&summary["vars"]["before"]), ["Kept/Arch"], "{summary}");
    assert_eq!(names(&summary["vars"]["after"]), ["Kept/Arch", "Kept/Ledge"], "the rescan after the add loads no linked entry");
    assert_eq!(names(&ed.call("prefab_library", list)), ["Kept/Arch", "Kept/Ledge", "Kit/Block"], "the library scans the link again after the script");
    let _ = std::fs::remove_dir_all(&project);
}

/// Replays the showcase maps in examples/mcp against the demo project, the same way a user or an LLM would build them.
#[test]
#[ignore]
fn example_mcp_scripts_replay() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let project = root.join("godot").canonicalize().unwrap();
    let ed = Editor::launch_with("examples", &["--project", project.to_str().unwrap()]);
    let out = artifacts().join("examples");
    std::fs::create_dir_all(&out).unwrap();

    // Array duplicate around a pivot places each copy one step further.
    let id = ed.box_brush([96.0, 0.0, -8.0], [112.0, 16.0, 8.0]);
    let copies = ed.call("duplicate", json!({ "ids": [id], "offset": [0, 0, 0], "rotate_y": 90, "pivot": [0, 0, 0], "count": 3 }));
    let brushes = ed.call("list_nodes", json!({ "type": "brush" }));
    let centers: Vec<[i64; 2]> = copies["copies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let node = brushes["nodes"].as_array().unwrap().iter().find(|n| n["id"] == c[0]).expect("copy listed");
            let b = node["bounds"].clone();
            let mid = |i: usize| ((b["min"][i].as_f64().unwrap() + b["max"][i].as_f64().unwrap()) * 0.5).round() as i64;
            [mid(0), mid(2)]
        })
        .collect();
    assert_eq!(centers, vec![[0, -104], [-104, 0], [0, 104]], "copies rotate 90, 180 and 270 degrees");

    for (script, min_brushes, min_entities) in [
        ("mountain_house", 60, 10),
        ("church_school", 250, 20),
        ("lighthouse_forest", 100, 20),
        ("night_district", 150, 30),
        ("withered_city", 600, 40),
        ("sea_island", 16, 6),
    ] {
        // Outputs may target Godot overlay nodes, which the editor only knows from the sidecar next to the saved map.
        let sidecar = root.join("godot/demo/maps/showcase").join(format!("{script}.overlay.json"));
        if sidecar.is_file() {
            std::fs::copy(&sidecar, out.join(format!("{script}.overlay.json"))).unwrap();
        }

        let path = root.join("examples/mcp").join(format!("{script}.json")).canonicalize().unwrap();
        let summary = ed.call("run_script", json!({ "path": path, "vars": { "save_dir": out } }));
        assert_eq!(summary["errors"], json!([]), "{script}");
        assert_eq!(summary["ran"], summary["steps"], "{script}");
        let map = ed.state()["map"].clone();
        assert!(map["brushes"].as_u64().unwrap() >= min_brushes, "{script}: {map}");
        assert!(map["entities"].as_u64().unwrap() >= min_entities, "{script}: {map}");
        assert_eq!(map["cameras"], json!([1, 2, 3]), "{script}");

        let scatters = ed.call("list_nodes", json!({ "type": "scatter" }));
        assert!(scatters["total"].as_u64().unwrap() >= 2, "{script}: scatter sets");
        let issues = ed.call("validate_map", json!({}));
        let errors: Vec<&Value> = issues["issues"].as_array().unwrap().iter().filter(|i| i["severity"] == "error").collect();
        assert!(errors.is_empty(), "{script}: {errors:?}");

        let saved = out.join(format!("{script}.tdm"));
        let reloaded = td_doc::format::load(&saved).unwrap_or_else(|e| panic!("{script}: {e}"));
        assert!(reloaded.problems.is_empty(), "{script}: {:?}", reloaded.problems);
        assert_eq!(reloaded.map.scatters().count() as u64, scatters["total"].as_u64().unwrap());

        ed.call("run_action", json!({ "action": "recall_camera", "args": { "slot": 1 } }));
        ed.call("set_editor", json!({ "shade": "lit" }));
        ed.input("window", json!([{ "type": "move", "x": 10, "y": 10 }]));
        let (_, _, colors) = ed.screenshot("3d", script);
        assert!(colors > 40, "{script}: lit overview has {colors} colors");
    }

    replay_logic_playground(&ed, &root, &out);

    // The night map's roller door keeps the beacon outputs passed to make_door next to the ones the wizard wires.
    let night = td_doc::format::load(&out.join("night_district.tdm")).unwrap().map;
    let (_, garage) = night.entities().find(|(_, e)| e.targetname() == Some("garage_door")).expect("garage door in the night map");
    assert_eq!(garage.outputs.iter().filter(|o| o.target == "garage_beacon").count(), 4, "{:?}", garage.outputs);

    // A bad scatter item is reported instead of leaving an empty palette.
    let e = ed.call_err("scatter", json!({ "op": "new_set", "name": "bad", "items": [{ "source": "res://x.glb", "align": true }] }));
    assert!(e.contains("items[0]"), "{e}");

    // Screenshots can render the 3D camera larger than its docked pane.
    let r = ed.try_rpc("tools/call", json!({ "name": "screenshot", "arguments": { "target": "3d", "width": 640, "height": 360 } })).unwrap();
    let png = base64::engine::general_purpose::STANDARD.decode(r["result"]["content"][0]["data"].as_str().expect("image content")).unwrap();
    assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8().dimensions(), (640, 360));
}

#[test]
#[ignore]
fn keyboard_shortcuts_and_views() {
    let ed = Editor::launch("keys");
    ed.input("window", json!([{ "type": "key", "key": "CloseBracket" }]));
    assert_eq!(ed.state()["editor"]["grid"], 32.0);
    ed.input("window", json!([{ "type": "key", "key": "OpenBracket" }, { "type": "key", "key": "OpenBracket" }]));
    assert_eq!(ed.state()["editor"]["grid"], 8.0);
    let first = ed.box_brush([0.0, 0.0, 0.0], [32.0, 32.0, 32.0]);
    ed.box_brush([64.0, 0.0, 0.0], [96.0, 32.0, 32.0]);
    ed.call("select", json!({ "ids": [first] }));
    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "F2" }, { "type": "text", "text": "north wall" }, { "type": "key", "key": "Enter" }]));
    assert_eq!(ed.call("get_node", json!({ "id": first }))["label"], "north wall", "F2 opens the name field in the Outliner");
    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "A", "modifiers": ["ctrl"] }]));
    assert_eq!(ed.state()["selection"]["nodes"].as_array().unwrap().len(), 2);
    ed.input("window", json!([{ "type": "key", "key": "H", "modifiers": ["ctrl"] }]));
    assert!(ed.call("list_nodes", json!({ "type": "brush" }))["nodes"].as_array().unwrap().iter().all(|n| n["hidden"] == true));
    ed.input("window", json!([{ "type": "key", "key": "H", "modifiers": ["ctrl", "shift"] }]));
    assert!(ed.call("list_nodes", json!({ "type": "brush" }))["nodes"].as_array().unwrap().iter().all(|n| n["hidden"] == false));
    for view in ["3d", "top", "front", "side"] {
        let (w, h, _) = ed.screenshot(view, view);
        assert!(w > 50 && h > 50);
    }

    let ppp = |ed: &Editor| ed.state()["ui"]["pixels_per_point"].as_f64().unwrap();
    let base = ppp(&ed);
    ed.input("window", json!([{ "type": "key", "key": "Equals", "modifiers": ["ctrl"] }, { "type": "key", "key": "Equals", "modifiers": ["ctrl"] }]));
    ed.input("window", json!([{ "type": "move", "x": 5, "y": 5 }]));
    assert_eq!(ed.state()["ui"]["scale"], 1.2);
    assert!((ppp(&ed) / base - 1.2).abs() < 0.01, "pixels per point {} from {base}", ppp(&ed));
    ed.input("window", json!([{ "type": "key", "key": "0", "modifiers": ["ctrl"] }]));
    assert_eq!(ed.state()["ui"]["scale"], 1.0);
}

#[test]
#[ignore]
fn vertex_gizmo_moves_a_vertex_along_one_axis() {
    let ed = Editor::launch("vertex_gizmo");
    let id = ed.box_brush([-32.0, -32.0, -32.0], [32.0, 32.0, 32.0]);
    ed.call("select", json!({ "ids": [id] }));
    ed.call("set_editor", json!({ "tool": "vertex" }));
    ed.call("set_camera", json!({ "view": "3d", "position": [190.0, 150.0, 210.0], "look_at": [0.0, 0.0, 0.0] }));

    // Double click the +x+y+z corner to attach the precise-move gizmo, then drag its x arrow by +32.
    ed.input("3d", json!([{ "type": "double_click", "world": [32.0, 32.0, 32.0] }]));
    ed.screenshot("3d", "vertex_gizmo");
    let (_, before) = ed.selection_bounds();
    ed.input("3d", json!([{ "type": "drag", "world": [50.0, 32.0, 32.0], "to_world": [82.0, 32.0, 32.0], "steps": 10 }]));
    let (_, after) = ed.selection_bounds();
    assert!((after[0] - 64.0).abs() < 2.0, "x corner should move +32 to 64 via the gizmo, before {before:?} after {after:?}");
    assert!((after[1] - before[1]).abs() < 1e-3 && (after[2] - before[2]).abs() < 1e-3, "only x should change, after {after:?}");
}

#[test]
#[ignore]
fn vertex_tool_pulls_a_face_dot_up_to_add_a_vertex() {
    let ed = Editor::launch("vertex_face_dot");
    let id = ed.box_brush([-32.0, -32.0, -32.0], [32.0, 32.0, 32.0]);
    ed.call("select", json!({ "ids": [id] }));
    ed.call("set_editor", json!({ "tool": "vertex" }));
    ed.call("set_camera", json!({ "view": "3d", "position": [190.0, 150.0, 210.0], "look_at": [0.0, 0.0, 0.0] }));
    let corners = |ed: &Editor| ed.call("get_node", json!({ "id": id }))["vertices"].as_array().unwrap().len();
    assert_eq!(corners(&ed), 8);

    // The top face center sits on a horizontal face, a drag in the horizontal plane could never leave it.
    ed.input("3d", json!([{ "type": "drag", "world": [0.0, 32.0, 0.0], "to_world": [0.0, 80.0, 0.0], "steps": 10 }]));
    let (_, after) = ed.selection_bounds();
    assert_eq!(corners(&ed), 9, "the pulled dot became a vertex");
    assert!((after[1] - 80.0).abs() < 1.0, "the new vertex sits 48 above the old top, got {after:?}");
    assert_eq!(ed.state()["selection"]["nodes"], json!([id]), "the brush stays selected");
}

#[test]
#[ignore]
fn vertex_tool_click_selects_a_dot_and_an_inward_pull_says_why() {
    let ed = Editor::launch("vertex_dot_click");
    ed.box_brush([-512.0, -32.0, -512.0], [512.0, 0.0, 512.0]);
    let id = ed.box_brush([-32.0, 0.0, -32.0], [32.0, 64.0, 32.0]);
    ed.call("select", json!({ "ids": [id] }));
    ed.call("set_editor", json!({ "tool": "vertex" }));
    ed.call("set_camera", json!({ "view": "3d", "position": [190.0, 150.0, 210.0], "look_at": [0.0, 32.0, 0.0] }));

    // The midpoint of a bottom edge lies on the floor behind it. The click selects the dot instead of picking the floor.
    ed.input("3d", json!([{ "type": "click", "world": [32.0, 0.0, 0.0] }]));
    let state = ed.state();
    assert_eq!(state["selection"]["nodes"], json!([id]), "the click kept the brush selected");
    assert!(state["status"].as_str().unwrap().contains("away"), "the click on a dot says what to do, got {}", state["status"]);

    // Pulling the top face center down is a dent: the brush stays as it was and the status says why.
    let (_, before) = ed.selection_bounds();
    ed.input("3d", json!([{ "type": "drag", "world": [0.0, 64.0, 0.0], "to_world": [0.0, 16.0, 0.0], "steps": 10 }]));
    let (_, after) = ed.selection_bounds();
    assert_eq!(before, after, "a refused pull changes nothing");
    assert_eq!(ed.call("get_node", json!({ "id": id }))["vertices"].as_array().unwrap().len(), 8);
    let status = ed.state()["status"].as_str().unwrap().to_string();
    assert!(status.contains("dents"), "the status says why, got {status}");
}

#[test]
#[ignore]
fn scatter_materials_and_scattering_onto_a_scatter() {
    let ed = Editor::launch("scatter_materials");
    ed.box_brush([-512.0, -32.0, -512.0], [512.0, 0.0, 512.0]);
    let rocks = ed.call("scatter", json!({ "op": "paint", "preset": "rocks", "center": [0, 0], "radius": 420, "density": 3, "seed": 4 }));
    let rocks_id = rocks["set"]["id"].as_u64().unwrap();
    assert!(rocks["placed"].as_u64().unwrap() > 0, "rocks were painted");

    // A material on the set retextures every model in it.
    ed.call("scatter", json!({ "op": "material", "id": rocks_id, "material": "showcase/snow" }));
    let node = ed.call("get_node", json!({ "id": rocks_id }));
    assert_eq!(node["material"], "showcase/snow");
    assert!(node["items"][0].get("material").is_none(), "no entry override yet");

    // A material on one palette entry wins for that entry only.
    ed.call("scatter", json!({ "op": "material", "id": rocks_id, "item": 0, "material": "showcase/gold" }));
    let node = ed.call("get_node", json!({ "id": rocks_id }));
    assert_eq!(node["items"][0]["material"], "showcase/gold", "entry 0 is retextured");
    assert!(node["items"][1].get("material").is_none(), "entry 1 still follows the set");
    assert_eq!(node["material"], "showcase/snow", "the set keeps its own material");
    let entries = rocks["set"]["items"].as_array().unwrap().len();
    assert_eq!(ed.call_err("scatter", json!({ "op": "material", "id": rocks_id, "item": 9 })), format!("the set has no palette entry 9, it has {entries}"));

    // Grass that targets the rock set lands on the rocks, above the floor they stand on.
    let grass = ed.call("scatter", json!({ "op": "new_set", "preset": "grass", "name": "on_rocks", "targets": [rocks_id] }));
    let grass_id = grass["id"].as_u64().unwrap();
    let painted = ed.call("scatter", json!({ "op": "paint", "id": grass_id, "center": [0, 0], "radius": 420, "density": 12, "seed": 7 }));
    assert!(painted["placed"].as_u64().unwrap() > 0, "grass landed on the rocks");
    let blades = ed.call("get_node", json!({ "id": grass_id }));
    let above = blades["instances"].as_array().unwrap().iter().filter(|i| i[2].as_f64().unwrap() > 0.0).count();
    assert_eq!(above, blades["instances"].as_array().unwrap().len(), "every blade sits on a rock, not on the floor");
}

#[test]
#[ignore]
fn the_scatter_tool_paints_grass_onto_scattered_boulders_under_the_mouse() {
    let ed = Editor::launch("scatter_on_scatter_mouse");
    let floor = ed.box_brush([-1024.0, -32.0, -1024.0], [1024.0, 0.0, 1024.0]);
    ed.call("scatter", json!({ "op": "new_set", "name": "boulders", "preset": "boulders", "targets": [floor] }));
    let boulders = ed.call("scatter", json!({ "op": "paint", "center": [0, 0], "radius": 600, "density": 2, "seed": 3 }));
    let boulders_id = boulders["set"]["id"].as_u64().unwrap();
    let rock = ed.call("get_node", json!({ "id": boulders_id }))["instances"][0].clone();
    let (x, z) = (rock[1].as_f64().unwrap(), rock[3].as_f64().unwrap());

    // What a user does: a new grass set from the preset, the scatter tool, and a drag across a boulder.
    let grass = ed.call("scatter", json!({ "op": "new_set", "name": "moss", "preset": "grass" }));
    let grass_id = grass["id"].as_u64().unwrap();
    ed.call("set_editor", json!({ "tool": "scatter" }));
    ed.call("set_camera", json!({ "view": "3d", "position": [x, 420.0, z + 260.0], "look_at": [x, 20.0, z] }));
    ed.input("3d", json!([{ "type": "drag", "world": [x - 20.0, 30.0, z], "to_world": [x + 20.0, 30.0, z], "steps": 10 }]));
    ed.screenshot("3d", "grass_on_boulders");

    let node = ed.call("get_node", json!({ "id": grass_id }));
    assert_eq!(node["targets"], json!([boulders_id]), "the stroke started on a boulder, so the boulders are the target");
    let blades = node["instances"].as_array().unwrap();
    assert!(!blades.is_empty(), "grass was painted");
    assert!(blades.iter().all(|i| i[2].as_f64().unwrap() > 0.0), "every blade sits on a boulder, above the floor");
}

#[test]
#[ignore]
fn mesh_uv_projections_adjustments_and_the_mesh_vertex_gizmo() {
    let ed = Editor::launch("mesh_uv_adjust");
    let mesh = ed.call("create_mesh", json!({ "shape": "cuboid", "min": [0, 0, 0], "max": [64, 64, 64] }))["id"].as_u64().unwrap();
    let uvs = |face: u64| ed.call("texture", json!({ "op": "get", "faces": [[mesh, face]] }))["faces"][0]["explicit"].clone();
    let floats = |v: &Value| v.as_array().unwrap().iter().flat_map(|c| c.as_array().unwrap().iter().map(|x| x.as_f64().unwrap())).collect::<Vec<_>>();
    assert!(uvs(0).is_null(), "a new mesh uses planar projections");
    let err = ed.call_err("texture", json!({ "op": "uv_adjust", "adjust": "flip_u" }));
    assert!(err.contains("no explicit UV corners"), "{err}");

    // Bake writes the planar look as corner UVs, every projection then relays them.
    assert_eq!(ed.call("texture", json!({ "op": "mesh_uv", "kind": "bake" }))["changed"], 6);
    assert!(uvs(0).is_array());
    for kind in ["planar", "planar_x", "planar_y", "planar_z", "box", "cylinder_x", "cylinder", "cylinder_z", "sphere", "view", "unfold", "world"] {
        assert_eq!(ed.call("texture", json!({ "op": "mesh_uv", "kind": kind }))["changed"], 6, "{kind}");
    }

    let start = floats(&uvs(0));
    for _ in 0..2 {
        ed.call("texture", json!({ "op": "uv_adjust", "adjust": "flip_u", "faces": [[mesh, 0]] }));
    }

    assert!(approx(&floats(&uvs(0)), &start), "flipping twice is the identity");
    for _ in 0..4 {
        ed.call("texture", json!({ "op": "uv_adjust", "adjust": "rotate", "degrees": 90, "faces": [[mesh, 0]] }));
    }

    assert!(approx(&floats(&uvs(0)), &start), "four quarter turns are the identity");

    ed.call("texture", json!({ "op": "uv_adjust", "adjust": "fit", "faces": [[mesh, 0]] }));
    let fitted = floats(&uvs(0));
    assert!(fitted.iter().all(|x| (-1e-4..=1.0001).contains(x)), "fit spans the texture once: {fitted:?}");
    assert!(fitted.iter().any(|x| *x < 1e-4) && fitted.iter().any(|x| *x > 0.9999));

    let moved = ed.call("texture", json!({ "op": "uv_adjust", "adjust": "move", "texels": [8, 0], "corners": [[mesh, 0, 0]] }));
    assert_eq!(moved["changed"], 1, "a corner stitched to nothing moves alone");
    ed.call("texture", json!({ "op": "uv_adjust", "adjust": "align_horizontal", "corners": [[mesh, 0, 0], [mesh, 0, 1]] }));
    let aligned = floats(&uvs(0));
    assert!((aligned[1] - aligned[3]).abs() < 1e-5, "corners 0 and 1 share v: {aligned:?}");
    ed.call("texture", json!({ "op": "uv_adjust", "adjust": "snap", "faces": [[mesh, 0]] }));
    assert!(ed.call_err("texture", json!({ "op": "uv_adjust", "adjust": "wobble", "faces": [[mesh, 0]] })).contains("unknown adjust"));
    assert_eq!(ed.call("texture", json!({ "op": "mesh_uv", "kind": "clear" }))["changed"], 6);
    assert!(uvs(0).is_null());

    // In the mesh tool a double click puts the transform gizmo on a vertex, its X arrow moves along X only.
    ed.call("select", json!({ "ids": [mesh] }));
    ed.call("set_editor", json!({ "tool": "mesh" }));
    ed.call("set_camera", json!({ "view": "front", "center": [64, 64, 0], "zoom": 2.0 }));
    ed.input("front", json!([{ "type": "double_click", "world": [64.0, 64.0, 64.0] }]));
    ed.screenshot("front", "mesh_vertex_gizmo");
    let before = ed.call("get_node", json!({ "id": mesh }))["vertices"].clone();
    ed.input("front", json!([{ "type": "drag", "world": [80.0, 64.0, 64.0], "to_world": [112.0, 72.0, 64.0], "steps": 10 }]));
    let after = ed.call("get_node", json!({ "id": mesh }))["vertices"].clone();
    let changed: Vec<(Vec<f64>, Vec<f64>)> = before
        .as_array()
        .unwrap()
        .iter()
        .zip(after.as_array().unwrap())
        .map(|(a, b)| (floats(&json!([a])), floats(&json!([b]))))
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(changed.len(), 1, "one vertex moved: {changed:?}");
    let (a, b) = &changed[0];
    assert_eq!((b[0] - a[0], b[1] - a[1], b[2] - a[2]), (32.0, 0.0, 0.0), "moved +32 along X only");
    assert_eq!(ed.state()["undo"][0], "Move Vertices");
}

#[test]
#[ignore]
fn rocks_from_create_brush_and_create_mesh() {
    let ed = Editor::launch("rocks");
    let brush = ed.call("create_brush", json!({ "shape": "rock", "min": [0, 0, 0], "max": [96, 48, 64], "seed": 7, "facets": 3 }));
    assert_eq!(brush["ids"].as_array().map(Vec::len), Some(1), "one convex brush: {brush}");
    let low = ed.call("create_mesh", json!({ "shape": "rock", "min": [200, 0, 0], "max": [296, 48, 64], "detail": 1 }))["faces"].as_u64().unwrap();
    let high =
        ed.call("create_mesh", json!({ "shape": "rock", "min": [400, 0, 0], "max": [496, 48, 64], "detail": 3, "flatten": 0 }))["faces"].as_u64().unwrap();
    assert!(high > low * 8, "detail subdivides the mesh: {low} vs {high}");
    assert!(ed.call_err("create_brush", json!({ "shape": "rock", "min": [0, 0, 0], "max": [64, 64, 64], "seed": -1 })).contains("seed must be"));
    assert!(ed.call_err("create_mesh", json!({ "shape": "rock", "min": [0, 0, 0], "max": [64, 64, 64], "roughness": 2 })).contains("roughness must be"));
}

#[test]
#[ignore]
fn an_arch_takes_an_angle_over_mcp() {
    let ed = Editor::launch("arch_angle");
    let count = |args: Value| ed.call("create_brush", args)["ids"].as_array().map(Vec::len);
    assert_eq!(count(json!({ "shape": "arch", "min": [0, 0, 0], "max": [128, 64, 16], "sides": 6 })), Some(6));
    assert_eq!(count(json!({ "shape": "arch", "min": [0, 0, 0], "max": [128, 64, 16], "sides": 6, "angle": 90 })), Some(6));
    let arch = |max: [i32; 3], angle: f64| ed.call("create_brush", json!({ "shape": "arch", "min": [0, 0, 0], "max": max, "angle": angle }));
    assert!(arch([128, 64, 16], 90.0)["note"].is_null(), "an angle that fits is not reported");
    for (asked, used) in [(0.0, 10.0), (5.0, 10.0), (200.0, 180.0)] {
        let reply = arch([128, 64, 16], asked);
        assert_eq!(reply["angle"].as_f64(), Some(used), "{reply}");
        assert!(reply["note"].as_str().unwrap().contains("outside 10 to 180"), "{reply}");
    }

    let low = arch([256, 32, 16], 90.0);
    assert!(low["angle"].as_f64().unwrap() < 60.0, "a low box caps the sweep: {low}");
    assert!(low["note"].as_str().unwrap().contains("rise above the bounds"), "{low}");
    for angle in [90, 180] {
        let one = json!({ "shape": "arch", "sides": 1, "angle": angle, "min": [0, 0, 400], "max": [256, 192, 432] });
        assert!(ed.call_err("create_brush", one).contains("at least 2 sides"), "{angle}");
    }

    let walled = |angle: f64, thickness: f64| {
        let args = json!({ "shape": "arch", "min": [0, 0, 600], "max": [256, 192, 632], "angle": angle, "thickness": thickness });
        ed.call("create_brush", args)
    };
    for angle in [90.0, 180.0] {
        assert!(walled(angle, 16.0)["note"].is_null(), "a thickness that fits is not reported");
        let thin = walled(angle, 0.0);
        assert_eq!(thin["thickness"].as_f64(), Some(1.0), "{thin}");
        assert!(thin["note"].as_str().unwrap().contains("is under 1"), "{thin}");
    }

    let thick = walled(90.0, 500.0);
    assert!(thick["thickness"].as_f64().unwrap() < 500.0, "{thick}");
    assert!(thick["note"].as_str().unwrap().contains("the most this arch can take"), "{thick}");
    assert!(walled(180.0, 500.0)["note"].is_null(), "a half ellipse keeps a thick wall as it always has");
    assert!(ed.call_err("create_brush", json!({ "shape": "arch", "min": [0, 0, 0], "max": [128, 64, 16], "angle": "90" })).contains("angle must be a number"));

    let both = ed.call("create_brush", json!({ "shape": "arch", "min": [0, 0, 700], "max": [256, 192, 732], "angle": 300, "thickness": 0 }));
    assert_eq!((both["angle"].as_f64(), both["thickness"].as_f64()), (Some(180.0), Some(1.0)), "{both}");

    let wall = |angle: f64| {
        let args = json!({ "shape": "arch_wall", "min": [1200, 0, 700], "max": [1456, 192, 732], "opening_width": 128, "opening_height": 128, "angle": angle });
        ed.call("create_mesh", args)
    };
    assert!(wall(90.0)["note"].is_null(), "an angle that fits is not reported");
    let wide = wall(300.0);
    assert_eq!(wide["angle"].as_f64(), Some(180.0), "{wide}");
    assert!(wide["note"].as_str().unwrap().contains("outside 10 to 180"), "{wide}");
    let low = ed.call(
        "create_mesh",
        json!({ "shape": "arch_wall", "min": [1200, 0, 800], "max": [1456, 192, 832], "opening_width": 128, "opening_height": 8, "angle": 90 }),
    );
    assert!(low["note"].as_str().unwrap().contains("rise above"), "{low}");
    let text = json!({ "shape": "arch_wall", "min": [1200, 0, 900], "max": [1456, 192, 932], "angle": "flat" });
    assert!(ed.call_err("create_mesh", text).contains("angle must be a number"));

    let flat = ed.call("create_mesh", json!({ "shape": "arch_wall", "min": [0, 0, 0], "max": [128, 128, 16], "angle": 90 }))["faces"].as_u64().unwrap();
    let round = ed.call("create_mesh", json!({ "shape": "arch_wall", "min": [200, 0, 0], "max": [328, 128, 16] }))["faces"].as_u64().unwrap();
    assert_eq!(flat, round, "the angle only moves the opening's corners");
}

#[test]
#[ignore]
fn a_tunnel_makes_its_pieces_for_every_profile() {
    let ed = Editor::launch("tunnel");
    let count = |args: Value| ed.call("create_brush", args)["ids"].as_array().map(Vec::len);
    let tunnel = |x: i32, profile: Option<&str>| {
        let mut args = json!({ "shape": "tunnel", "min": [x, 0, 0], "max": [x + 128, 128, 256], "sides": 6 });
        if let Some(profile) = profile {
            args["profile"] = json!(profile);
        }

        args
    };
    assert_eq!(count(tunnel(0, None)), Some(3), "square without a floor by default");
    for (i, (profile, pieces)) in [("open_bottom", 3), ("closed", 4), ("arched", 8), ("round", 6), ("square", 3)].into_iter().enumerate() {
        assert_eq!(count(tunnel(200 * (i as i32 + 1), Some(profile))), Some(pieces), "{profile}");
    }

    let toggled = |profile: &str, toggles: Value| {
        let mut args = tunnel(-300, Some(profile));
        args.as_object_mut().unwrap().extend(toggles.as_object().unwrap().clone());
        count(args)
    };
    assert_eq!(toggled("closed", json!({ "floor": false })), Some(3), "an explicit floor overrides the profile");
    assert_eq!(toggled("open_bottom", json!({ "floor": true })), Some(4));
    assert_eq!(toggled("square", json!({ "ceiling": false })), Some(2), "an open trench");
    assert_eq!(toggled("square", json!({ "floor": true, "ceiling": false })), Some(3));
    assert_eq!(toggled("arched", json!({ "floor": true, "ceiling": false })), Some(9), "an arch keeps its roof");
    assert_eq!(toggled("round", json!({ "floor": false, "ceiling": false })), Some(6), "a round tube ignores both");
    for wrong in ["triangle", "tube", ""] {
        assert!(ed.call_err("create_brush", tunnel(0, Some(wrong))).contains("profile is square, arched, round"), "{wrong:?}");
    }

    for sides in [0, 1] {
        let mut flat = tunnel(0, Some("arched"));
        flat["sides"] = json!(sides);
        assert!(ed.call_err("create_brush", flat).contains("at least 2 sides"), "{sides}");
    }

    let mut low = tunnel(1200, Some("round"));
    low["sides"] = json!(1);
    assert_eq!(count(low), Some(3), "a round tunnel takes at least 3 sides");
    let thick = |thickness: f64| ed.call("create_brush", json!({ "shape": "tunnel", "min": [0, 0, 400], "max": [192, 128, 656], "thickness": thickness }));
    assert!(thick(16.0)["note"].is_null(), "a thickness that fits is not reported");
    let capped = thick(200.0);
    assert_eq!(capped["thickness"].as_f64().map(|t| (t * 100.0).round()), Some(5120.0), "40 percent of the 128 high tunnel: {capped}");
    assert!(capped["note"].as_str().unwrap().contains("40 percent"), "{capped}");
    assert_eq!(thick(0.25)["thickness"].as_f64(), Some(1.0));
}

fn tunnel_vertices(ed: &Editor, reply: &Value) -> Vec<[f64; 3]> {
    reply["ids"].as_array().unwrap().iter().flat_map(|id| mesh_vertices(ed, id.as_u64().unwrap())).collect()
}

#[test]
#[ignore]
fn a_tunnel_takes_its_options_over_mcp() {
    let ed = Editor::launch("tunnel_options");
    let make = |max: [i32; 3], extra: Value| {
        let mut args = json!({ "shape": "tunnel", "min": [0, 0, 0], "max": max });
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let reply = ed.call("create_brush", args);
        let bounds = ed.selection_bounds();
        (reply, bounds)
    };
    let note = |reply: &Value| reply["note"].as_str().unwrap_or_default().to_string();
    let cube = [128, 128, 256];

    let (plain, along_z) = make(cube, json!({}));
    assert!(plain["note"].is_null(), "{plain}");
    assert_eq!(along_z, (vec![0.0, 0.0, 0.0], vec![128.0, 128.0, 256.0]), "no axis keeps the tunnel along Z");
    assert_eq!(tunnel_vertices(&ed, &make(cube, json!({ "axis": "z" })).0), tunnel_vertices(&ed, &plain));
    let shaft = make([128, 256, 128], json!({ "axis": "y" })).0;
    for id in shaft["ids"].as_array().unwrap() {
        let ys: Vec<f64> = mesh_vertices(&ed, id.as_u64().unwrap()).iter().map(|v| v[1]).collect();
        assert_eq!((ys.iter().cloned().fold(f64::INFINITY, f64::min), ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max)), (0.0, 256.0), "a shaft runs up Y");
    }

    let walked = make(cube, json!({ "walkway": true })).0;
    assert_eq!(walked["ids"].as_array().unwrap().len(), plain["ids"].as_array().unwrap().len() + 2, "two walkways: {walked}");
    let wide = make(cube, json!({ "walkway": true, "walkway_width": 90, "walkway_height": 200 })).0;
    assert!((wide["walkway_width"].as_f64().unwrap() - 38.4).abs() < 1e-9 && wide["walkway_height"].as_f64() == Some(56.0), "{wide}");
    assert!(note(&wide).contains("40 percent of the opening") && note(&wide).contains("half the clear height"), "{wide}");
    assert!(make(cube, json!({ "walkway_width": 90 })).0["note"].is_null(), "walkway sizes without walkways are not reported");

    let arched = |max: [i32; 3], angle: f64, thickness: f64| make(max, json!({ "profile": "arched", "angle": angle, "thickness": thickness })).0;
    let flatter = arched(cube, 120.0, 16.0);
    assert!(flatter["note"].is_null(), "{flatter}");
    assert_ne!(tunnel_vertices(&ed, &flatter), tunnel_vertices(&ed, &arched(cube, 180.0, 16.0)), "the angle reshapes the arch");
    let outside = arched(cube, 200.0, 16.0);
    assert_eq!(outside["angle"].as_f64(), Some(180.0), "{outside}");
    assert!(note(&outside).contains("outside 10 to 180"), "{outside}");
    let low = arched([256, 96, 256], 150.0, 16.0);
    assert!(low["angle"].as_f64().unwrap() < 150.0 && note(&low).contains("rise above half"), "{low}");
    let thin = arched(cube, 30.0, 2.0);
    assert!(thin["angle"].as_f64().unwrap() > 30.0 && note(&thin).contains("under 1 u thick"), "{thin}");
    let flat = arched([128, 24, 256], 30.0, 2.0);
    assert_eq!(flat["angle"].as_f64(), Some(180.0), "{flat}");
    assert!(note(&flat).contains("half ellipse"), "{flat}");
    assert!(make(cube, json!({ "angle": 30 })).0["note"].is_null(), "a square tunnel ignores the angle");

    let steep = make(cube, json!({ "rise": 900 })).0;
    assert_eq!(steep["rise"].as_f64(), Some(512.0), "{steep}");
    assert!(note(&steep).contains("twice the length"), "{steep}");
    let (_, sloped) = make(cube, json!({ "rise": 64 }));
    assert_eq!(sloped.1[1], 192.0, "the far end climbs 64");

    let over = make(cube, json!({ "bend_angle": 270, "bend_segments": 1 })).0;
    assert_eq!((over["bend_angle"].as_f64(), over["bend_segments"].as_u64()), (Some(180.0), Some(2)), "{over}");
    assert!(note(&over).contains("outside 0 to 180, the tunnel bends 180") && note(&over).contains("one per quarter turn"), "{over}");
    let (back, straight) = make(cube, json!({ "bend_angle": -10 }));
    assert!(note(&back).contains("stays straight"), "{back}");
    assert_eq!(straight, along_z);
    let (_, bent) = make(cube, json!({ "bend_angle": 90 }));
    assert_eq!(bent, (vec![0.0, 0.0, 0.0], vec![192.0, 128.0, 448.0]), "a quarter turn toward +x round a 64 radius");
    let (_, left) = make(cube, json!({ "bend_angle": 90, "bend_toward": "-x" }));
    assert_eq!(left, (vec![-64.0, 0.0, 0.0], vec![128.0, 128.0, 448.0]));

    for (args, error) in [
        (json!({ "bend_angle": 90, "bend_toward": "up" }), "bend_toward is +x, -x, +y, -y, +z or -z"),
        (json!({ "bend_angle": 90, "bend_toward": "+z" }), "a tunnel along z turns toward x or y"),
        (json!({ "axis": "x", "bend_angle": 90, "bend_toward": "x" }), "a tunnel along x turns toward y or z"),
        (json!({ "walkway": "yes" }), "walkway must be true or false"),
        (json!({ "rise": "high" }), "rise must be a number"),
        (json!({ "bend_segments": 2.5 }), "bend_segments must be a whole number"),
    ] {
        let mut full = json!({ "shape": "tunnel", "min": [0, 0, 0], "max": cube });
        full.as_object_mut().unwrap().extend(args.as_object().unwrap().clone());
        assert!(ed.call_err("create_brush", full).contains(error), "{args}");
    }
}

#[test]
#[ignore]
fn a_bent_tunnel_ends_square_on_the_grid_over_mcp() {
    let ed = Editor::launch("tunnel_bend");
    for profile in ["arched", "square", "round"] {
        let args = json!({ "shape": "tunnel", "min": [0, 0, 0], "max": [128, 128, 256], "profile": profile, "bend_angle": 90, "bend_toward": "+x", "bend_radius": 64 });
        let reply = ed.call("create_brush", args);
        let vertices = tunnel_vertices(&ed, &reply);
        let end: Vec<&[f64; 3]> = vertices.iter().filter(|v| v[0] > 191.0).collect();
        assert!(end.iter().all(|v| v[0] == 192.0), "{profile} far end leaves x 192: {end:?}");
        let zs = end.iter().map(|v| v[2]);
        assert_eq!(
            (zs.clone().fold(f64::INFINITY, f64::min), zs.fold(f64::NEG_INFINITY, f64::max)),
            (320.0, 448.0),
            "{profile} far end spans the tunnel width"
        );
        assert!(vertices.iter().all(|v| v[2] >= 0.0 && v[2] <= 448.0), "{profile} start stays on z 0");
    }
}

#[test]
#[ignore]
fn sweep_faces_runs_a_corridor_round_a_bend() {
    let ed = Editor::launch("sweep");
    let floor = ed.call("create_brush", json!({ "min": [0, 0, 0], "max": [64, 16, 128], "material": "dev/floor" }))["ids"][0].as_u64().unwrap();
    let faces = ed.call("texture", json!({ "op": "get", "faces": (0..6).map(|f| json!([floor, f])).collect::<Vec<_>>() }));
    let end = faces["faces"].as_array().unwrap().iter().find(|f| f["normal"][0].as_f64().unwrap() > 0.99).unwrap()["face"][1].as_u64().unwrap();

    let straight = ed.call("sweep_faces", json!({ "faces": [[floor, end]], "length": 96, "segments": 3 }));
    assert_eq!(straight["ids"].as_array().unwrap().len(), 3);
    assert_eq!(straight["bounds"]["min"][0].as_f64(), Some(64.0));
    assert_eq!(straight["bounds"]["max"][0].as_f64(), Some(160.0));

    ed.call("select", json!({ "faces": [[floor, end]] }));
    let arc = ed.call("sweep_faces", json!({ "path": "arc", "radius": 128, "angle": 90, "bend": "left", "segments": 6, "rise": 32 }));
    assert_eq!(arc["ids"].as_array().unwrap().len(), 6, "the selected face is used: {arc}");
    assert_eq!(selected(&ed).len(), 6, "the new brushes are selected");
    assert!(ed.call_err("sweep_faces", json!({ "faces": [[floor, end]], "path": "arc", "radius": 10 })).contains("radius must be more than"));
    assert!(ed.call_err("sweep_faces", json!({ "faces": [[floor, end]], "path": "spiral" })).contains("unknown path"));
    assert!(ed.call_err("sweep_faces", json!({ "faces": [[floor, 99]] })).contains("existing brush face"));
    assert!(ed.call_err("sweep_faces", json!({ "faces": [[floor, end]], "path": "arc", "angle": -90 })).contains("angle must be more than 0"));
    let back = faces["faces"].as_array().unwrap().iter().find(|f| f["normal"][0].as_f64().unwrap() < -0.99).unwrap()["face"][1].as_u64().unwrap();
    assert!(ed.call_err("sweep_faces", json!({ "faces": [[floor, end], [floor, back]] })).contains("opposite directions"));
}

#[test]
#[ignore]
fn place_along_path_repeats_bends_and_hangs_a_cable() {
    let ed = Editor::launch("spline");
    let post = ed.call("create_brush", json!({ "min": [-4, 0, -4], "max": [4, 48, 4], "material": "dev/grey" }))["ids"][0].as_u64().unwrap();
    for (name, target, x) in [("fence1", "fence2", 0), ("fence2", "fence3", 256), ("fence3", "", 512)] {
        let mut props = json!({ "targetname": name });
        if !target.is_empty() {
            props["target"] = json!(target);
        }

        ed.call("create_entity", json!({ "classname": "path_corner", "origin": [x, 0, 300], "properties": props }));
    }

    let fence = ed.call("place_along_path", json!({ "ids": [post], "spacing": 64 }));
    assert_eq!(fence["ids"].as_array().unwrap().len(), 9, "{fence}");
    let x = |v: &Value| v.as_f64().unwrap();
    assert!((x(&fence["bounds"]["min"][0]) + 4.0).abs() < 0.01 && (x(&fence["bounds"]["max"][0]) - 516.0).abs() < 0.01, "{fence}");
    assert_eq!(selected(&ed).len(), 9, "the copies are selected");

    let rail = ed.call("create_brush", json!({ "min": [0, 40, -2], "max": [128, 44, 2], "material": "dev/grey" }))["ids"][0].as_u64().unwrap();
    let bent = ed.call("place_along_path", json!({ "mode": "bend", "ids": [rail], "chain": "fence1" }));
    assert_eq!(bent["ids"].as_array().unwrap().len(), 1, "{bent}");

    let cable = ed.call("place_along_path", json!({ "mode": "cable", "points": [[0, 200, 0], [256, 200, 0]], "sag": 40, "radius": 1 }));
    assert!((cable["bounds"]["min"][1].as_f64().unwrap() - 159.0).abs() < 0.5, "{cable}");
    assert!(ed.call_err("place_along_path", json!({ "mode": "wave" })).contains("unknown mode"));
    assert!(ed.call_err("place_along_path", json!({ "ids": [post], "points": [[0, 0, 0]] })).contains("at least two points"));
    assert!(ed.call_err("place_along_path", json!({ "ids": [post], "chain": "nope" })).contains("no chain"));
}

/// The Array Copies dialog and the duplicate tool share one core: copies turn and scale step by step, and the whole run
/// is a single undo step.
#[test]
#[ignore]
fn array_copies_turn_scale_and_undo_in_one_step() {
    let ed = Editor::launch("array");
    let bar = ed.box_brush([-32.0, 0.0, -8.0], [32.0, 16.0, 8.0]);
    let node = |id: u64| -> Value {
        let nodes = ed.call("list_nodes", json!({}));
        nodes["nodes"].as_array().unwrap().iter().find(|n| n["id"] == id).expect("copy listed").clone()
    };
    let bounds_of = |id: u64| -> [[i64; 3]; 2] {
        let b = node(id)["bounds"].clone();
        ["min", "max"].map(|end| [0, 1, 2].map(|i| b[end][i].as_f64().unwrap().round() as i64))
    };
    let size_of = |id: u64| -> [i64; 3] {
        let [min, max] = bounds_of(id);
        [0, 1, 2].map(|i| max[i] - min[i])
    };

    let turned = ed.call("duplicate", json!({ "ids": [bar], "count": 2, "offset": [0, 100, 0], "rotate": [0, 0, 90] }));
    let ids: Vec<u64> = turned["copies"].as_array().unwrap().iter().map(|c| c[0].as_u64().unwrap()).collect();
    assert_eq!(size_of(ids[0]), [16, 64, 16], "a quarter turn about Z stands the bar up");
    assert_eq!(size_of(ids[1]), [64, 16, 16], "two quarter turns lay it down again");
    assert_eq!(ed.brushes(), 3);
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.brushes(), 1, "one undo removes every copy");

    let shrunk = ed.call("duplicate", json!({ "ids": [bar], "count": 2, "offset": [0, 0, 100], "scale": 0.5 }));
    let ids: Vec<u64> = shrunk["copies"].as_array().unwrap().iter().map(|c| c[0].as_u64().unwrap()).collect();
    assert_eq!(size_of(ids[0]), [32, 8, 8]);
    assert_eq!(size_of(ids[1]), [16, 4, 4], "the scale compounds");
    ed.call("run_action", json!({ "action": "undo" }));
    let stretched = ed.call("duplicate", json!({ "ids": [bar], "scale": [1, 2, 1], "pivot": [0, 0, 0], "offset": [0, 0, 0] }));
    assert_eq!(size_of(stretched["copies"][0][0].as_u64().unwrap()), [64, 32, 16]);

    assert!(ed.call_err("duplicate", json!({ "ids": [bar], "scale": 0 })).contains("scale has to be more than 0"));
    assert!(ed.call_err("duplicate", json!({ "ids": [bar], "scale": "big" })).contains("scale must be"));
    assert!(ed.call_err("duplicate", json!({ "ids": [bar], "scale": 2, "count": 512 })).contains("beyond"));
    assert!(ed.call_err("duplicate", json!({ "ids": [bar], "rotate": [0, 90] })).contains("rotate"));

    ed.call("select", json!({ "ids": [bar] }));
    ed.input("window", json!([{ "type": "move", "x": 10, "y": 10 }, { "type": "key", "key": "F1" }, { "type": "text", "text": "array copies" }, { "type": "key", "key": "Enter" }]));
    ed.input("window", json!([{ "type": "move", "x": 10, "y": 10 }]));
    assert!(window_titles(&ed).contains(&"Array Copies".to_string()), "the palette opens the dialog: {:?}", window_titles(&ed));
    assert_eq!(ed.brushes(), 2, "opening the dialog only previews");
    let state = ed.state();
    let window = state["ui"]["windows"].as_array().unwrap().iter().find(|w| w["title"] == "Array Copies").unwrap()["rect"].clone();
    let r: Vec<f64> = window.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    // Create is the last row, at the window's left edge.
    ed.input("window", json!([{ "type": "click", "x": r[0] + 24.0, "y": r[3] - 16.0 }]));
    assert!(!window_titles(&ed).contains(&"Array Copies".to_string()), "Create closes the dialog");
    assert_eq!(ed.brushes(), 5, "three copies by default");
    assert_eq!(bounds_of(selected(&ed)[0]), [[160, 0, -8], [224, 16, 8]], "the last copy is three steps along X and stays selected");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.brushes(), 2, "one undo removes every copy the dialog made");

    let linked = ed.call("duplicate", json!({ "ids": [bar], "count": 2, "offset": [0, 0, 0], "rotate_y": 90, "pivot": [100, 0, 0], "linked": true }));
    let groups: Vec<u64> = linked["copies"].as_array().unwrap().iter().map(|c| c[0].as_u64().unwrap()).collect();
    for g in &groups {
        assert!(node(*g)["name"].as_str().unwrap().ends_with("(linked)"), "{}", node(*g));
    }

    assert_eq!(bounds_of(groups[0]), [[92, 0, 68], [108, 16, 132]], "rotate_y still turns about the pivot");
    assert_eq!(bounds_of(groups[1]), [[168, 0, -8], [232, 16, 8]]);
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.brushes(), 2);
    assert_eq!(ed.call("list_nodes", json!({ "type": "group" }))["nodes"], json!([]), "the same undo takes the wrapping group back");
}

#[test]
#[ignore]
fn find_entities_and_replace_material_share_the_dialog_core() {
    let ed = Editor::launch("find_replace");
    let a = ed.box_brush([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]);
    let b = ed.box_brush([128.0, 0.0, 0.0], [192.0, 64.0, 64.0]);
    for id in [a, b] {
        ed.call("texture", json!({ "op": "apply", "material": "dev/orange", "faces": (0..6).map(|f| json!([id, f])).collect::<Vec<_>>() }));
    }

    ed.call("texture", json!({ "op": "shift", "texels": [5, 3], "faces": [[a, 0]] }));
    let before = ed.call("texture", json!({ "op": "get", "faces": [[a, 0]] }));
    let door = ed.call("create_entity", json!({ "classname": "func_door", "origin": [0, 0, 200], "properties": { "targetname": "Hall_Door" } }))["id"]
        .as_u64()
        .unwrap();
    let light = ed
        .call("create_entity", json!({ "classname": "light", "origin": [64, 96, 200], "properties": { "targetname": "lamp", "target": "hall_door" } }))["id"]
        .as_u64()
        .unwrap();

    let found = ed.call("find_entities", json!({ "classname": "FUNC_" }));
    assert_eq!(found["total"], 1);
    assert_eq!(found["entities"][0]["id"].as_u64(), Some(door));
    let by_value = ed.call("find_entities", json!({ "value": "hall_door", "select": true }));
    assert_eq!(by_value["total"], 2, "the value matches targetname and target: {by_value}");
    let mut picked = selected(&ed);
    picked.sort_unstable();
    assert_eq!(picked, vec![door, light]);
    let none = ed.call("find_entities", json!({ "classname": "typo", "select": true }));
    assert_eq!((none["total"].as_u64(), none["selected"].as_u64()), (Some(0), Some(0)));
    assert_eq!(selected(&ed).len(), 2, "a search that finds nothing keeps the selection");
    assert_eq!(ed.call("find_entities", json!({ "key": "target", "value": "hall", "exact": true }))["total"], 0);
    assert_eq!(ed.call("find_entities", json!({ "key": "target", "value": "hall_door", "exact": true }))["total"], 1);
    assert!(ed.call_err("find_entities", json!({})).contains("at least one"));
    assert!(ed.call_err("find_entities", json!({ "key": 3 })).contains("must be text"));

    ed.call("select", json!({ "ids": [] }));
    let selection_only = ed.call_err("replace_material", json!({ "from": "dev/orange", "to": "dev/dark", "scope": "selection" }));
    assert!(selection_only.contains("nothing is selected"), "{selection_only}");
    ed.call("select", json!({ "ids": [a] }));
    let dry = ed.call("replace_material", json!({ "from": "dev/orange", "to": "dev/dark", "scope": "selection", "dry_run": true }));
    assert_eq!(dry["changed"], 6);
    let done = ed.call("replace_material", json!({ "from": "dev/orange", "to": "dev/dark", "scope": "selection" }));
    assert_eq!((done["faces"].as_u64(), done["changed"].as_u64()), (Some(6), Some(6)));
    let after = ed.call("texture", json!({ "op": "get", "faces": [[a, 0]] }));
    assert_eq!(after["faces"][0]["material"], "dev/dark");
    assert_eq!(after["faces"][0]["uv"], before["faces"][0]["uv"], "the alignment stays: {after}");
    assert_eq!(ed.call("list_nodes", json!({ "material": "dev/orange" }))["total"], 1, "the other brush keeps its material");

    let rest = ed.call("replace_material", json!({ "from": "dev/orange", "to": "dev/dark" }));
    assert_eq!(rest["changed"], 6);
    assert_eq!(ed.call("replace_material", json!({ "from": "dev/orange", "to": "dev/dark" }))["changed"], 0);
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.call("list_nodes", json!({ "material": "dev/orange" }))["total"], 1, "one undo step brings the whole replacement back");
    assert!(ed.call_err("replace_material", json!({ "from": "dev/orange", "to": "dev/orange" })).contains("the same"));
    assert!(ed.call_err("replace_material", json!({ "from": "dev/orange" })).contains("to is required"));
    assert!(ed.call_err("replace_material", json!({ "from": "a", "to": "b", "scope": "room" })).contains("unknown scope"));

    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "F", "modifiers": ["ctrl"] }]));
    ed.input("top", json!([{ "type": "move" }]));
    assert!(ed.state()["ui"]["windows"].as_array().unwrap().iter().any(|w| w["title"] == "Find and Replace"), "Ctrl+F opens the dialog");
}

#[test]
#[ignore]
fn polygon_brush_extrudes_convex_and_concave_outlines() {
    let ed = Editor::launch("polygon_brush");
    let square = ed.call("create_polygon_brush", json!({ "points": [[0, 0, 0], [64, 0, 0], [64, 0, 64], [0, 0, 64]], "axis": "y", "height": 32 }));
    assert_eq!(square["ids"].as_array().unwrap().len(), 1, "a convex outline is one brush: {square}");
    assert_eq!(square["bounds"], json!({ "min": [0.0, 0.0, 0.0], "max": [64.0, 32.0, 64.0] }));

    let l = json!([[200, 0, 0], [328, 0, 0], [328, 0, 64], [264, 0, 64], [264, 0, 128], [200, 0, 128]]);
    let made = ed.call("create_polygon_brush", json!({ "points": l, "height": 16 }));
    assert_eq!(made["ids"].as_array().unwrap().len(), 2, "an L is split into two convex brushes: {made}");
    assert_eq!(made["bounds"], json!({ "min": [200.0, 0.0, 0.0], "max": [328.0, 16.0, 128.0] }));
    assert_eq!(selected(&ed).len(), 2, "the pieces are selected");
    assert_eq!(ed.state()["undo"][0], "MCP: Create Polygon Brush");

    let mut reversed = l.as_array().unwrap().clone();
    reversed.reverse();
    let flipped = ed.call("create_polygon_brush", json!({ "points": reversed, "height": 16 }));
    assert_eq!(flipped["ids"].as_array().unwrap().len(), 2, "winding does not matter");

    let before = ed.brushes();
    let bowtie = json!([[0, 0, 300], [64, 0, 364], [64, 0, 300], [0, 0, 364]]);
    assert!(ed.call_err("create_polygon_brush", json!({ "points": bowtie, "axis": "y" })).contains("crosses itself"));
    assert!(ed.call_err("create_polygon_brush", json!({ "points": [[0, 0, 0], [64, 0, 0]], "axis": "y" })).contains("at least three"));
    assert!(ed.call_err("create_polygon_brush", json!({ "points": [[0, 0, 0], [32, 0, 0], [64, 0, 0]], "axis": "y" })).contains("no area"));
    assert!(ed.call_err("create_polygon_brush", json!({ "points": [[0, 0, 0], [64, 0, 0], [64, 8, 64]], "axis": "y" })).contains("one plane"));
    let slanted = json!([[0, 0, 0], [64, 0, 0], [0, 64, 64]]);
    assert!(ed.call_err("create_polygon_brush", json!({ "points": slanted })).contains("do not share exactly one coordinate"));
    let both = json!({ "points": [[0, 0, 0], [64, 0, 0], [0, 0, 64]], "axis": "y", "normal": [0, 1, 0] });
    assert!(ed.call_err("create_polygon_brush", both).contains("not both"));
    assert_eq!(ed.brushes(), before, "refused outlines change nothing");

    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.brushes(), before - 2, "one undo removes both pieces");
}

/// Scripted building blocks the withered city showcase needs: block letters, window grids, plane clips, vertex moves and
/// named wizard entities, all without mouse input.
#[test]
#[ignore]
fn block_text_window_grids_clips_and_named_wizards() {
    let ed = Editor::launch("block_text");
    let text = ed.call("create_brush", json!({ "shape": "text", "text": "HI", "min": [0, 0, 0], "max": [300, 4, 140], "material": "dev/orange" }));
    let letters = text["letters"].as_array().unwrap();
    assert_eq!(letters.len(), 2);
    assert_eq!(letters[0].as_array().unwrap().len(), 3, "H is two posts and a bar");
    assert_eq!(text["ids"].as_array().unwrap().len(), 3 + letters[1].as_array().unwrap().len());
    assert!(ed.call_err("create_brush", json!({ "shape": "text", "text": "Ö", "min": [0, 0, 0], "max": [64, 8, 64] })).contains("no block letter"));

    let solid = ed.call("create_brush", json!({ "min": [0, 0, 200], "max": [512, 128, 216] }))["ids"].as_array().unwrap().len();
    let wall = ed.call(
        "create_brush",
        json!({ "min": [0, 0, 300], "max": [512, 128, 316], "openings": [{ "min": [32, 32, 296], "max": [96, 96, 320], "count": 4, "step": [128, 0, 0] }] }),
    );
    assert!(wall["ids"].as_array().unwrap().len() > solid + 3, "four windows cut the wall into pieces: {wall}");

    ed.call("select", json!({ "ids": wall["ids"] }));
    let clipped = ed.call("run_action", json!({ "action": "clip_apply", "args": { "point": [0, 64, 0], "normal": [0, 1, 0], "keep": "back" } }));
    assert!(!clipped["ids"].as_array().unwrap().is_empty());
    assert_eq!(ed.selection_bounds().1[1], 64.0, "only the part below the plane is left");
    assert!(ed.call_err("run_action", json!({ "action": "clip_apply", "args": { "point": [0, 0, 0] } })).contains("normal"));

    let block = ed.call("create_brush", json!({ "min": [600, 0, 0], "max": [664, 64, 64] }))["ids"][0].clone();
    ed.call("select", json!({ "ids": [block] }));
    ed.call("run_action", json!({ "action": "move_vertices", "args": { "vertices": [[664, 64, 64]], "offset": [0, -32, 0] } }));
    let node = ed.call("get_node", json!({ "id": block }));
    assert!(node.to_string().contains("32"), "the corner sags: {node}");
    assert!(ed.call_err("run_action", json!({ "action": "move_vertices", "args": { "vertices": [[1, 2, 3]], "offset": [0, 8, 0] } })).contains("vertices"));

    let lift = ed.call("create_brush", json!({ "min": [800, 0, 0], "max": [864, 8, 64] }))["ids"].clone();
    let platform =
        ed.call("gameplay", json!({ "op": "make_platform", "ids": lift, "travel": [0, 256, 0], "properties": { "targetname": "lift", "speed": "3" } }));
    assert_eq!(platform["properties"]["targetname"], "lift");
    assert_eq!(platform["properties"]["speed"], "3");
    let button = ed.call("create_brush", json!({ "min": [900, 0, 0], "max": [908, 8, 8] }))["ids"].clone();
    let made = ed.call("gameplay", json!({ "op": "make_button", "ids": button, "target": "lift", "input": "toggle", "properties": { "targetname": "call" } }));
    assert_eq!(made["properties"]["targetname"], "call");
    assert_eq!(made["outputs"][0]["target"], "lift");
}

fn shot(ed: &Editor, args: Value, label: &str) -> image::RgbaImage {
    let r = ed.try_rpc("tools/call", json!({ "name": "screenshot", "arguments": args })).unwrap();
    let data = r["result"]["content"][0]["data"].as_str().unwrap_or_else(|| panic!("{label}: no image in {r}"));
    let png = base64::engine::general_purpose::STANDARD.decode(data).unwrap();
    std::fs::write(artifacts().join(format!("{}_{label}.png", ed.name)), &png).unwrap();
    image::load_from_memory(&png).unwrap().to_rgba8()
}

/// Mean absolute difference per channel, 0 for identical pictures.
fn image_difference(a: &image::RgbaImage, b: &image::RgbaImage) -> f64 {
    assert_eq!(a.dimensions(), b.dimensions());
    let sum: u64 = a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    sum as f64 / a.as_raw().len() as f64
}

#[test]
#[ignore]
fn offscreen_beauty_shots_leave_out_editor_overlays() {
    let ed = Editor::launch("beauty");
    let floor = ed.box_brush([-256.0, -16.0, -256.0], [256.0, 0.0, 256.0]);
    ed.call("set_camera", json!({ "view": "3d", "position": [0, 96, 192], "look_at": [0, 16, 0] }));
    ed.call("run_action", json!({ "action": "select_none" }));
    let size = json!({ "target": "3d", "width": 320, "height": 200 });
    let empty = shot(&ed, size.clone(), "empty");
    ed.call("create_entity", json!({ "classname": "info_player_start", "origin": [0, 0, 0] }));
    ed.call("run_action", json!({ "action": "select_none" }));
    let beauty = shot(&ed, size.clone(), "beauty");
    let mut with_overlays = size.clone();
    with_overlays["overlays"] = json!(true);
    let overlays = shot(&ed, with_overlays, "overlays");
    assert!(image_difference(&beauty, &empty) < 0.5, "the player start box shows in a beauty shot");
    assert!(image_difference(&overlays, &empty) > 1.0, "overlays: true still draws the entity box and edges");

    ed.call("select", json!({ "ids": [floor] }));
    let selected = shot(&ed, size.clone(), "selected_beauty");
    assert!(image_difference(&selected, &beauty) < 0.1, "a selected brush keeps its selection tint, difference {}", image_difference(&selected, &beauty));
    let mut tinted = size.clone();
    tinted["overlays"] = json!(true);
    let tinted = shot(&ed, tinted, "selected_overlays");
    assert!(image_difference(&tinted, &overlays) > 1.0, "the overlay shot right after still shows the selection");
}

/// Lamps that start off with `fixture_off dark` keep their fixture in the lit preview, drawn without its glow like the
/// Godot addon does, and glow again once the lamp starts on.
#[test]
#[ignore]
fn dark_fixtures_lose_their_glow_in_the_lit_preview() {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../godot").canonicalize().unwrap();
    let ed = Editor::launch_with("dark_fixture", &["--project", project.to_str().unwrap()]);
    ed.call("set_map_properties", json!({ "properties": { "sun_energy": "0", "ambient_energy": "0.1", "sky_energy": "0.1" } }));
    let panel = ed.call("create_brush", json!({ "min": [-64, 0, -8], "max": [64, 96, 0], "material": "showcase/lamp" }))["ids"][0].as_u64().unwrap();
    ed.call("gameplay", json!({ "op": "brush_entity", "ids": [panel], "classname": "func_illusionary", "properties": { "targetname": "panel" } }));
    let lamp = ed.call(
        "create_entity",
        json!({ "classname": "light", "origin": [0, 48, -64], "properties": { "start_on": "0", "fixture": "panel", "fixture_off": "dark" } }),
    )["id"]
        .as_u64()
        .unwrap();
    ed.call("run_action", json!({ "action": "select_none" }));
    ed.call("set_camera", json!({ "view": "3d", "position": [0, 48, 48], "look_at": [0, 48, 0] }));
    ed.call("set_editor", json!({ "shade": "lit" }));
    let args = json!({ "target": "3d", "width": 160, "height": 120 });
    let luma = |img: &image::RgbaImage| {
        let p = img.get_pixel(80, 60).0;
        (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3
    };
    let dark = shot(&ed, args.clone(), "off");
    ed.call("update_entity", json!({ "id": lamp, "properties": { "start_on": "1" } }));
    let on = shot(&ed, args.clone(), "on");
    ed.call("update_entity", json!({ "id": lamp, "properties": { "start_on": "0", "fixture_off": "hide" } }));
    let hidden = shot(&ed, args, "hidden");
    assert!(luma(&on) > 200 && luma(&dark) + 80 < luma(&on), "the glow goes with the lamp: on {} dark {}", luma(&on), luma(&dark));
    assert!(image_difference(&dark, &hidden) > 5.0, "a dark fixture is still drawn, unlike a hidden one");
}

/// A lamp in a closed room is baked into the light map, the Baked view shows it, the bake goes with undo and the
/// saved file, and moving the lamp marks the bake out of date.
#[test]
#[ignore]
fn baked_lighting_shows_saves_and_goes_stale() {
    let ed = Editor::launch("bake");
    ed.call("set_map_properties", json!({ "properties": { "sun_energy": "0", "ambient_energy": "0", "sky_energy": "0" } }));
    for (min, max) in [
        ([-256.0, -16.0, -256.0], [256.0, 0.0, 256.0]),
        ([-256.0, 192.0, -256.0], [256.0, 208.0, 256.0]),
        ([-272.0, 0.0, -256.0], [-256.0, 192.0, 256.0]),
        ([256.0, 0.0, -256.0], [272.0, 192.0, 256.0]),
        ([-256.0, 0.0, -272.0], [256.0, 192.0, -256.0]),
        ([-256.0, 0.0, 256.0], [256.0, 192.0, 272.0]),
    ] {
        ed.box_brush(min, max);
    }

    let lamp =
        ed.call("create_entity", json!({ "classname": "light", "origin": [-128, 96, 0], "properties": { "light_energy": "2", "omni_range": "12" } }))["id"]
            .as_u64()
            .unwrap();
    ed.call("run_action", json!({ "action": "select_none" }));
    let baked = ed.call("bake_lighting", json!({ "quality": "preview", "texel_size": 16, "view": "light" }));
    assert!(baked["has_bake"].as_bool().unwrap() && !baked["out_of_date"].as_bool().unwrap(), "{baked}");
    assert_eq!(baked["baked_lights"], 1, "{baked}");
    assert!(baked["surfaces"].as_u64().unwrap() >= 6 && baked["width"].as_u64().unwrap() > 0, "{baked}");
    assert_eq!(ed.state()["editor"]["shade"], "baked");

    ed.call("set_camera", json!({ "view": "3d", "position": [0, 160, 240], "look_at": [0, 0, 0] }));
    let args = json!({ "target": "3d", "width": 160, "height": 120 });
    let luma = |img: &image::RgbaImage, x: u32| {
        let p = img.get_pixel(x, 90).0;
        (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3
    };
    let light = shot(&ed, args.clone(), "light_map");
    assert!(luma(&light, 40) > luma(&light, 120) + 20, "the floor under the lamp is brighter: {} against {}", luma(&light, 40), luma(&light, 120));
    ed.call("bake_lighting", json!({ "bake": false, "view": "occlusion" }));
    let ao = shot(&ed, args.clone(), "occlusion");
    assert!(image_difference(&ao, &light) > 5.0, "the view switches maps");

    ed.call("run_action", json!({ "action": "undo" }));
    assert!(!ed.call("bake_lighting", json!({ "bake": false }))["has_bake"].as_bool().unwrap(), "undo drops the bake");
    ed.call("run_action", json!({ "action": "redo" }));

    let path = artifacts().join("bake.tdm");
    ed.call("map_file", json!({ "op": "save", "path": path }));
    ed.call("map_file", json!({ "op": "open", "path": path }));
    let reopened = ed.call("bake_lighting", json!({ "bake": false }));
    assert!(reopened["has_bake"].as_bool().unwrap() && !reopened["out_of_date"].as_bool().unwrap(), "{reopened}");

    ed.call("select", json!({ "ids": [lamp] }));
    ed.call("transform", json!({ "translate": [256, 0, 0] }));
    assert!(ed.call("bake_lighting", json!({ "bake": false }))["out_of_date"].as_bool().unwrap(), "moving the lamp marks the bake out of date");
}

#[test]
#[ignore]
fn texture_settings_retile_every_face_and_save() {
    let ed = Editor::launch("texture_settings");
    let wall = ed.box_brush([0.0, 0.0, 0.0], [256.0, 128.0, 16.0]);
    let side = ed.box_brush([256.0, 0.0, 0.0], [512.0, 128.0, 16.0]);
    let faces = |b: u64| (0..6).map(|f| json!([b, f])).collect::<Vec<_>>();
    ed.call("texture", json!({ "op": "apply", "material": "dev/orange", "faces": ([faces(wall), faces(side)].concat()) }));
    ed.call("texture", json!({ "op": "shift", "texels": [37, 11], "faces": faces(side) }));
    ed.call("set_camera", json!({ "view": "3d", "position": [256, 64, -100], "look_at": [256, 64, 0] }));
    let args = json!({ "target": "3d", "width": 320, "height": 200 });
    let before = shot(&ed, args.clone(), "per_face");

    let set = ed.call("texture", json!({ "op": "settings", "material": "dev/orange", "projection": "world", "size": [64, 64], "bake": false }));
    assert_eq!((set["projection"].as_str(), set["size"].clone(), set["bake"].as_bool()), (Some("world"), json!([64.0, 64.0]), Some(false)), "{set}");
    let world = shot(&ed, args.clone(), "world");
    assert!(image_difference(&before, &world) > 1.0, "the new repeat size and projection show in the view");
    assert!(ed.call_err("texture", json!({ "op": "settings", "material": "dev/orange", "projection": "sideways" })).contains("projection"));

    let path = artifacts().join("texture_settings.tdm");
    ed.call("map_file", json!({ "op": "save", "path": path }));
    ed.call("map_file", json!({ "op": "open", "path": path }));
    let kept = ed.call("texture", json!({ "op": "settings", "material": "dev/orange" }));
    assert_eq!(kept, set, "saved with the map");

    let cleared = ed.call("texture", json!({ "op": "settings", "material": "dev/orange", "size": null, "projection": "face", "bake": true }));
    assert!(cleared["size"].is_null() && cleared["projection"] == "face", "{cleared}");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.call("texture", json!({ "op": "settings", "material": "dev/orange" })), set, "undo brings them back");
}

#[test]
#[ignore]
fn a_camera_inside_a_trigger_sees_out_of_it() {
    let ed = Editor::launch("trigger_inside");
    ed.box_brush([-512.0, -16.0, -512.0], [512.0, 0.0, 512.0]);
    ed.box_brush([-64.0, 0.0, -300.0], [64.0, 128.0, -268.0]);
    ed.call("set_camera", json!({ "view": "3d", "position": [0, 64, 0], "look_at": [0, 48, -300] }));
    let args = json!({ "target": "3d", "width": 320, "height": 200, "overlays": true });
    let outside = shot(&ed, args.clone(), "no_trigger");
    let volume = ed.box_brush([-128.0, 0.0, -128.0], [128.0, 128.0, 128.0]);
    ed.call("select", json!({ "ids": [volume] }));
    ed.call("run_action", json!({ "action": "create_brush_entity", "args": { "classname": "trigger_once" } }));
    ed.call("run_action", json!({ "action": "select_none" }));
    let inside = shot(&ed, args, "inside_trigger");
    assert!(image_difference(&inside, &outside) < 2.0, "the trigger's back faces tint the whole view, difference {}", image_difference(&inside, &outside));
}

#[test]
#[ignore]
fn emissive_model_materials_glow_in_the_lit_preview() {
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut bin: Vec<u8> = Vec::new();
    for f in [-1.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 2.0, 0.0, -1.0, 0.0, 0.0, 1.0, 2.0, 0.0, -1.0, 2.0, 0.0] {
        bin.extend(f.to_le_bytes());
    }

    // A black panel that only its emission can light up.
    let gltf = json!({
        "asset": { "version": "2.0" },
        "extensionsUsed": ["KHR_materials_emissive_strength"],
        "scene": 0, "scenes": [{ "nodes": [0] }], "nodes": [{ "mesh": 0 }],
        "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 }, "material": 0 }] }],
        "materials": [{ "pbrMetallicRoughness": { "baseColorFactor": [0.0, 0.0, 0.0, 1.0] }, "emissiveFactor": [1.0, 0.2, 0.0],
                        "extensions": { "KHR_materials_emissive_strength": { "emissiveStrength": 4.0 } } }],
        "buffers": [{ "byteLength": bin.len(), "uri": format!("data:application/octet-stream;base64,{}", b64.encode(&bin)) }],
        "bufferViews": [{ "buffer": 0, "byteLength": bin.len() }],
        "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 6, "type": "VEC3", "min": [-1.0, 0.0, 0.0], "max": [1.0, 2.0, 0.0] }]
    });
    let dir = artifacts().join("emissive_project");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.godot"), "config_version=5\n").unwrap();
    std::fs::write(dir.join("emissive_panel.gltf"), gltf.to_string()).unwrap();

    let ed = Editor::launch_with("emissive_model", &["--project", dir.to_str().unwrap()]);
    ed.call("create_entity", json!({ "classname": "prop_model", "origin": [0, 0, 0], "properties": { "model": "res://emissive_panel.gltf" } }));
    ed.call("run_action", json!({ "action": "select_none" }));
    ed.call("set_camera", json!({ "view": "3d", "position": [0, 32, 96], "look_at": [0, 32, 0] }));
    ed.call("set_editor", json!({ "shade": "lit" }));
    let img = shot(&ed, json!({ "target": "3d", "width": 200, "height": 200 }), "lit");
    let center = img.get_pixel(100, 100).0;
    assert!(center[0] > 180 && center[2] < 120, "the emissive panel glows orange instead of rendering black, got {center:?}");

    let listed = ed.call("list_nodes", json!({ "classname": "prop_model" }));
    let bounds = &listed["nodes"][0]["bounds"];
    let (min, max) = (bounds["min"].as_array().unwrap(), bounds["max"].as_array().unwrap());
    assert!(max[1].as_f64().unwrap() > 60.0 && min[0].as_f64().unwrap() < -30.0, "list_nodes gives the 2 m panel's bounds, not the entity box: {bounds}");
}

#[test]
#[ignore]
fn map_file_absolute_open_close_others_and_script_notes() {
    let ed = Editor::launch("mcp_small");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let name = "e2e_relative_room.tdm";
    let abs = manifest.join(name);
    ed.call("map_file", json!({ "op": "save", "path": abs.clone() }));

    // A relative path resolves against the process directory, opening it should not leave a later save exposed
    // to that changing: the path in the open map is made absolute.
    ed.call("map_file", json!({ "op": "open", "path": name }));
    let opened_path = ed.state()["map"]["path"].as_str().unwrap().to_string();
    assert!(std::path::Path::new(&opened_path).is_absolute(), "opened path should be absolute: {opened_path}");
    let _ = std::fs::remove_file(&abs);

    // A dirty tab left behind by map_file new does not linger: close_others clears it.
    ed.call("map_file", json!({ "op": "new" }));
    ed.box_brush([0.0, 0.0, 0.0], [16.0, 16.0, 16.0]);
    ed.call("map_file", json!({ "op": "new" }));
    assert_eq!(ed.state()["tabs"]["titles"].as_array().unwrap().len(), 2, "the dirty tab stayed open in the background");
    let closed = ed.call("run_action", json!({ "action": "close_tab", "args": { "close_others": true, "discard": true } }));
    assert_eq!(closed["closed"], 1);
    assert_eq!(ed.state()["tabs"]["titles"].as_array().unwrap().len(), 1);

    // run_script reports note steps separately instead of dropping them from the count.
    let steps = json!([
        { "note": "first" },
        { "tool": "create_brush", "args": { "min": [0, 0, 0], "max": [8, 8, 8] } },
        { "note": "second" },
        { "tool": "create_brush", "args": { "min": [8, 0, 0], "max": [16, 8, 8] } }
    ]);
    let summary = ed.call("run_script", json!({ "steps": steps }));
    assert_eq!(summary["steps"], 2);
    assert_eq!(summary["notes"], 2);

    // An unbraced $name/... is an error rather than a path nobody meant to write passing through unexpanded.
    let project = manifest.join("../../godot").canonicalize().unwrap();
    ed.call("open_project", json!({ "path": project }));
    let err = ed.call_err("run_script", json!({ "steps": [{ "tool": "map_file", "args": { "op": "open", "path": "$project/does_not_exist.tdm" } }] }));
    assert!(err.contains("${project}"), "{err}");
}

#[test]
#[ignore]
fn view_panes_maximize_close_and_come_back() {
    let ed = Editor::launch("view_panes");
    let width = |ed: &Editor| ed.screenshot("front", "front").0;
    let docked = width(&ed);
    ed.input("front", json!([{ "type": "move" }, { "type": "key", "key": "Space", "modifiers": ["shift"] }]));
    let maximized = width(&ed);
    assert!(maximized > docked * 3 / 2, "Shift+Space fills the view area with the front view, {docked} to {maximized}");
    ed.input("front", json!([{ "type": "move" }, { "type": "key", "key": "Space", "modifiers": ["shift"] }]));
    assert_eq!(width(&ed), docked, "Shift+Space again brings the four views back");

    // Passing over a view on the way to a menu does not make it the one Single View keeps, clicking in it does.
    let palette = |ed: &Editor, command: &str| {
        ed.input("window", json!([{ "type": "move", "x": 10, "y": 10 }, { "type": "key", "key": "F1" }, { "type": "text", "text": command }, { "type": "key", "key": "Enter" }]));
    };
    ed.input("front", json!([{ "type": "move" }]));
    palette(&ed, "single view");
    assert!(ed.screenshot("3d", "single_3d").0 > docked * 3 / 2, "Single View starts with the 3D view");
    palette(&ed, "four views");
    assert_eq!(width(&ed), docked);
    ed.input("front", json!([{ "type": "click", "button": "middle" }]));
    palette(&ed, "single view");
    assert!(width(&ed) > docked * 3 / 2, "Single View keeps the view worked in last");
    palette(&ed, "four views");
}

/// Rounded rectangle of a 3D view in window pixels, shrunk so the dock card edge stays out of it.
fn view_crop(ed: &Editor, window: &image::RgbaImage) -> image::RgbaImage {
    let state = ed.state();
    let ppp = state["ui"]["pixels_per_point"].as_f64().unwrap();
    let rect = state["cameras"].as_array().unwrap().iter().find(|c| c["view"] == "3d").unwrap()["rect"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap() * ppp)
        .collect::<Vec<_>>();
    let (x0, y0) = (rect[0] as u32 + 4, rect[1] as u32 + 4);
    let (x1, y1) = (rect[2] as u32 - 4, rect[3] as u32 - 4);
    image::imageops::crop_imm(window, x0, y0, x1 - x0, y1 - y0).to_image()
}

fn theme_active(ed: &Editor, want: &str) {
    for _ in 0..20 {
        if ed.state()["ui"]["theme_active"] == want {
            return;
        }
    }

    panic!("the theme never became {want}: {}", ed.state()["ui"]);
}

#[test]
#[ignore]
fn theme_switches_without_a_restart() {
    let ed = Editor::launch("theme_switch");
    ed.box_brush([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]);
    // Xvfb starts the pointer over a 2D view, where it lights up a brush edge, so it waits where the palette leaves it.
    ed.input("window", json!([{ "type": "move", "x": 10, "y": 10 }]));
    let window = |label: &str| shot(&ed, json!({ "target": "window" }), label);
    let dark = window("dark");
    assert_eq!(image_difference(&dark, &window("dark_twice")), 0.0, "a still editor draws the same picture twice");
    let dark_view = shot(&ed, json!({ "target": "3d" }), "dark_3d");
    let ui = ed.state()["ui"].clone();
    assert_eq!((ui["theme"].as_str(), ui["theme_active"].as_str()), (Some("dark"), Some("dark")));

    ed.call("set_editor", json!({ "theme": "light" }));
    theme_active(&ed, "light");
    assert_eq!(ed.state()["ui"]["theme"], "light");
    let light = window("light");
    let chrome = td_editor::theme::LIGHT.chrome;
    let backdrop = td_editor::theme::LIGHT.backdrop;
    assert_eq!(light.get_pixel(light.width() / 2, 2).0[..3], [chrome.r(), chrome.g(), chrome.b()], "menu bar");
    assert_eq!(light.get_pixel(1, light.height() / 2).0[..3], [backdrop.r(), backdrop.g(), backdrop.b()], "dock gap");
    assert!(image_difference(&dark, &light) > 20.0, "the window changes colors");
    // egui anti-aliases text and round shapes with a different curve in light mode, which moves their edge pixels a little.
    let (dark_crop, light_crop) = (view_crop(&ed, &dark), view_crop(&ed, &light));
    let worst = dark_crop.as_raw().iter().zip(light_crop.as_raw()).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
    assert!(worst < 60 && image_difference(&dark_crop, &light_crop) < 0.5, "the 3D view stays dark, worst {worst}");
    assert_eq!(image_difference(&dark_view, &shot(&ed, json!({ "target": "3d" }), "light_3d")), 0.0);

    let before = ed.state()["ui"].clone();
    assert!(ed.call_err("set_editor", json!({ "theme": "sepia" })).contains("dark, light or system"));
    assert!(ed.call_err("set_editor", json!({ "theme": 3 })).contains("dark, light or system"));
    assert_eq!(ed.state()["ui"]["theme"], before["theme"]);
    assert_eq!(ed.state()["ui"]["theme_active"], "light");

    ed.input("window", json!([{ "type": "move", "x": 10, "y": 10 }, { "type": "key", "key": "F1" }, { "type": "text", "text": "theme: dark" }, { "type": "key", "key": "Enter" }]));
    theme_active(&ed, "dark");
    assert_eq!(ed.state()["ui"]["theme"], "dark");
    assert_eq!(ed.state()["status"], "Theme: Dark");
    // The status line now names the command that ran, so it is left out.
    let again = window("dark_again");
    let body = |img: &image::RgbaImage| image::imageops::crop_imm(img, 0, 0, img.width(), img.height() - 40).to_image();
    assert_eq!(image_difference(&body(&dark), &body(&again)), 0.0, "the palette command brings the dark window back exactly");
}

/// An editor whose system opener is replaced by a script that writes down what it was asked to open, so no browser
/// starts. The closure waits for that many lines in the log, 0 for what is there now.
#[cfg(unix)]
fn editor_with_opener(name: &'static str) -> (Editor, impl Fn(usize) -> Vec<String>) {
    use std::os::unix::fs::PermissionsExt;

    let bin = artifacts().join("opener");
    std::fs::create_dir_all(&bin).unwrap();
    let log = bin.join("opened.txt");
    let _ = std::fs::remove_file(&log);
    for opener in ["xdg-open", "open"] {
        let script = bin.join(opener);
        std::fs::write(&script, format!("#!/bin/sh\necho \"$1\" >> '{}'\n", log.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let ed = Editor::launch_env(name, &[], &[("PATH", &path)]);
    let opened = move |count: usize| {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let lines: Vec<String> = std::fs::read_to_string(&log).unwrap_or_default().lines().map(str::to_string).collect();
            if lines.len() >= count || Instant::now() > deadline {
                return lines;
            }

            std::thread::sleep(Duration::from_millis(100));
        }
    };
    (ed, opened)
}

/// The middle of the Clip tool button in window points.
#[cfg(unix)]
fn clip_button_center(ed: &Editor) -> (f64, f64) {
    let state = ed.state();
    let button = state["ui"]["tool_buttons"].as_array().unwrap().iter().find(|b| b["tool"] == "Clip").expect("the Clip tool button")["rect"].clone();
    let r: Vec<f64> = button.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

/// F1 over a toolbar tool and a Ctrl+click on it open the tool's page of the manual.
#[test]
#[ignore]
#[cfg(unix)]
fn f1_and_ctrl_click_on_a_tool_open_its_manual_page() {
    let (ed, opened) = editor_with_opener("manual");
    let (x, y) = clip_button_center(&ed);
    let page = "https://paraxdev.github.io/TrenchDot/editor/brushes.html#reshape";

    ed.input("window", json!([{ "type": "move", "x": x, "y": y }, { "type": "key", "key": "F1" }, { "type": "text", "text": "grid larger" }, { "type": "key", "key": "Enter" }]));
    let state = ed.state();
    assert_eq!(state["editor"]["grid"], 16.0, "F1 over a tool opens the manual, not the command palette");
    assert!(state["status"].as_str().unwrap().contains(page), "{}", state["status"]);
    assert_eq!(opened(1), [page]);

    ed.input("window", json!([{ "type": "click", "x": x, "y": y, "modifiers": ["ctrl"] }]));
    assert_eq!(ed.state()["editor"]["tool"], "Select", "a Ctrl+click opens the page instead of picking the tool");
    assert_eq!(opened(2), [page, page]);
    ed.input("window", json!([{ "type": "click", "x": x, "y": y }]));
    assert_eq!(ed.state()["editor"]["tool"], "Clip");

    // Over a view F1 keeps its binding.
    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "F1" }, { "type": "text", "text": "grid larger" }, { "type": "key", "key": "Enter" }]));
    assert_eq!(ed.state()["editor"]["grid"], 32.0, "the palette ran the command");
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(opened(0).len(), 2, "nothing else was opened");
}

/// The manual opens in the language of the editor, English at the root of the site and every other language in its own
/// folder.
#[test]
#[ignore]
#[cfg(unix)]
fn f1_opens_the_manual_in_the_editor_language() {
    let (ed, opened) = editor_with_opener("manual_language");
    let page = |folder: &str| format!("https://paraxdev.github.io/TrenchDot/{folder}editor/brushes.html#reshape");

    ed.call("set_editor", json!({ "language": "de" }));
    let (x, y) = clip_button_center(&ed);
    ed.input("window", json!([{ "type": "move", "x": x, "y": y }, { "type": "key", "key": "F1" }]));
    assert!(ed.state()["status"].as_str().unwrap().contains(&page("de/")), "{}", ed.state()["status"]);
    assert_eq!(opened(1), [page("de/")]);

    ed.call("set_editor", json!({ "language": "zh-Hans" }));
    let (x, y) = clip_button_center(&ed);
    ed.input("window", json!([{ "type": "click", "x": x, "y": y, "modifiers": ["ctrl"] }]));
    assert_eq!(opened(2), [page("de/"), page("zh-Hans/")]);

    ed.call("set_editor", json!({ "language": "en" }));
    let (x, y) = clip_button_center(&ed);
    ed.input("window", json!([{ "type": "move", "x": x, "y": y }, { "type": "key", "key": "F1" }]));
    assert_eq!(opened(3), [page("de/"), page("zh-Hans/"), page("")]);

    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(opened(0).len(), 3, "nothing else was opened");
}

fn open_uv_editor(ed: &Editor) {
    ed.input("window", json!([{ "type": "move", "x": 10, "y": 10 }, { "type": "key", "key": "F1" }, { "type": "text", "text": "texture: uv editor" }, { "type": "key", "key": "Enter" }]));
}

fn selection_of(ed: &Editor) -> (Vec<u64>, Vec<(u64, u64)>) {
    let state = ed.state();
    let mut nodes: Vec<u64> = state["selection"]["nodes"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    let mut faces: Vec<(u64, u64)> = state["selection"]["faces"].as_array().unwrap().iter().map(|p| (p[0].as_u64().unwrap(), p[1].as_u64().unwrap())).collect();
    nodes.sort_unstable();
    faces.sort_unstable();
    (nodes, faces)
}

/// Ctrl+click toggles whole objects in the views whether or not the UV Editor tab is showing.
#[test]
#[ignore]
fn ctrl_click_toggles_objects_with_the_uv_editor_showing() {
    let ed = Editor::launch("ctrl_click_uv");
    let a = ed.box_brush([8.0, -16.0, 0.0], [72.0, 48.0, 40.0]);
    let b = ed.box_brush([136.0, 4.0, 12.0], [200.0, 52.0, 76.0]);
    let light = ed.call("create_entity", json!({ "classname": "light", "origin": [250, 30, 100] }))["id"].as_u64().unwrap();
    ed.call("select", json!({ "ids": [] }));
    open_uv_editor(&ed);
    let ctrl_click = |at: [f64; 3]| {
        ed.call("set_camera", json!({ "view": "top", "center": at, "zoom": 1.0 }));
        ed.input("top", json!([{ "type": "click", "world": at, "modifiers": ["ctrl"] }]));
    };

    ctrl_click([40.0, 48.0, 20.0]);
    assert_eq!(selection_of(&ed), (vec![a], vec![]));
    ctrl_click([168.0, 52.0, 44.0]);
    assert_eq!(selection_of(&ed), (vec![a, b], vec![]));
    ctrl_click([250.0, 30.0, 100.0]);
    assert_eq!(selection_of(&ed), (vec![a, b, light], vec![]));
    ctrl_click([40.0, 48.0, 20.0]);
    assert_eq!(selection_of(&ed), (vec![b, light], vec![]));
    ctrl_click([168.0, 52.0, 44.0]);
    assert_eq!(selection_of(&ed), (vec![light], vec![]));
}

/// Showing the UV Editor tab edits what is selected and never swaps objects for their faces.
#[test]
#[ignore]
fn opening_the_uv_editor_keeps_the_selection() {
    for (name, faces) in [("uv_keeps_objects", false), ("uv_keeps_faces", true)] {
        let ed = Editor::launch(name);
        let a = ed.box_brush([8.0, -16.0, 0.0], [72.0, 48.0, 40.0]);
        let wedge = ed.call("create_brush", json!({ "min": [-72, 8, 200], "max": [-8, 72, 296], "shape": "wedge" }))["ids"][0].as_u64().unwrap();
        let light = ed.call("create_entity", json!({ "classname": "light", "origin": [250, 30, 100] }))["id"].as_u64().unwrap();
        let expected = if faces {
            ed.call("select", json!({ "faces": [[a, 2], [wedge, 4]] }));
            (vec![], vec![(a, 2), (wedge, 4)])
        } else {
            ed.call("select", json!({ "ids": [a, wedge, light] }));
            (vec![a, wedge, light], vec![])
        };
        assert_eq!(selection_of(&ed), expected);
        open_uv_editor(&ed);
        ed.input("window", json!([{ "type": "move", "x": 12, "y": 12 }]));
        assert_eq!(selection_of(&ed), expected, "{name}");
    }
}

/// Ctrl+A selects every object in each preset and tool, except in Hammer's Texture tool, where it selects the faces of
/// the selected brushes, or of the brushes that own the selected faces.
#[test]
#[ignore]
fn hammer_ctrl_l_and_ctrl_i_flip_along_the_view_under_the_pointer() {
    let ed = Editor::launch("hammer_flip");
    let a = ed.box_brush([40.0, 8.0, -72.0], [104.0, 72.0, -20.0]);
    let far = ed.box_brush([300.0, 0.0, 0.0], [332.0, 32.0, 32.0]);
    ed.call("select", json!({ "ids": [a] }));
    ed.call("transform", json!({ "shear": { "side": "+y", "along": "x", "amount": 16 } }));
    ed.call("set_editor", json!({ "keymap": "hammer", "snap": false }));
    let verts = || -> Vec<[i64; 3]> {
        let node = ed.call("get_node", json!({ "id": a }));
        let mut all: Vec<[i64; 3]> = node["vertices"].as_array().unwrap().iter().map(|v| [0, 1, 2].map(|i| v[i].as_f64().unwrap().round() as i64)).collect();
        all.sort();
        all
    };
    let mirrored = |from: &[[i64; 3]], axis: usize, sum: i64| -> Vec<[i64; 3]> {
        let mut all: Vec<[i64; 3]> = from.iter().map(|v| std::array::from_fn(|i| if i == axis { sum - v[i] } else { v[i] })).collect();
        all.sort();
        all
    };
    let undo_len = || ed.state()["undo"].as_array().unwrap().len();
    let press = |view: &str, key: &str, mods: &[&str]| ed.input(view, json!([{ "type": "move" }, { "type": "key", "key": key, "modifiers": mods }]));
    let slanted = verts();
    assert_eq!(slanted.len(), 8);
    // The top side slid to x 56 to 120, so the bounds are 40 to 120 on X, 8 to 72 on Y and -72 to -20 on Z.
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0], min[1], max[1], min[2], max[2]], &[40.0, 120.0, 8.0, 72.0, -72.0, -20.0]), "{min:?} {max:?}");

    ed.call("set_camera", json!({ "view": "front", "center": [80, 40, -46], "zoom": 1.5 }));
    ed.call("set_camera", json!({ "view": "top", "center": [80, 0, -46], "zoom": 1.5 }));
    ed.call("set_camera", json!({ "view": "side", "center": [0, 40, -46], "zoom": 1.5 }));
    ed.call("set_camera", json!({ "view": "3d", "position": [300, 50, 0], "look_at": [0, 50, 0] }));
    let before = undo_len();
    press("front", "L", &["ctrl"]);
    let flipped_x = mirrored(&slanted, 0, 160);
    assert_eq!(verts(), flipped_x, "Ctrl+L in Front mirrors X across the center, one corner side swapped");
    assert_ne!(verts(), slanted);
    assert_eq!(undo_len(), before + 1, "one undo step");
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0], min[1], max[1], min[2], max[2]], &[40.0, 120.0, 8.0, 72.0, -72.0, -20.0]), "the bounds stay put {min:?} {max:?}");

    press("front", "I", &["ctrl"]);
    let flipped_xy = mirrored(&flipped_x, 1, 80);
    assert_eq!(verts(), flipped_xy, "Ctrl+I in Front mirrors Y");
    press("top", "I", &["ctrl"]);
    let flipped_xyz = mirrored(&flipped_xy, 2, -92);
    assert_eq!(verts(), flipped_xyz, "Ctrl+I in Top mirrors Z");
    press("front", "R", &["ctrl"]);
    assert_eq!(verts(), flipped_xy, "Ctrl+R repeats the last flip on the same world axis, whatever view it is pressed in");
    assert_eq!(undo_len(), before + 4);

    press("side", "L", &["ctrl"]);
    let side_z = mirrored(&flipped_xy, 2, -92);
    assert_eq!(verts(), side_z, "Ctrl+L in Side mirrors Z");
    press("side", "I", &["ctrl"]);
    assert_eq!(verts(), mirrored(&side_z, 1, 80), "Ctrl+I in Side mirrors Y");
    press("side", "I", &["ctrl"]);
    press("side", "L", &["ctrl"]);
    assert_eq!(verts(), flipped_xy);

    press("3d", "L", &["ctrl"]);
    assert_eq!(verts(), mirrored(&flipped_xy, 2, -92), "Ctrl+L in 3D flips Z when the camera looks along X");
    press("3d", "L", &["ctrl"]);
    press("3d", "I", &["ctrl"]);
    assert_eq!(verts(), mirrored(&flipped_xy, 1, 80), "Ctrl+I in 3D flips Y");
    press("3d", "I", &["ctrl"]);
    ed.call("set_camera", json!({ "view": "3d", "position": [0, 50, 300], "look_at": [0, 50, 0] }));
    press("3d", "L", &["ctrl"]);
    assert_eq!(verts(), mirrored(&flipped_xy, 0, 160), "Ctrl+L in 3D flips X when the camera looks along Z");
    press("3d", "L", &["ctrl"]);
    assert_eq!(verts(), flipped_xy);

    press("top", "R", &["ctrl", "shift"]);
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0], min[2], max[2]], &[54.0, 106.0, -86.0, -6.0]), "Ctrl+Shift+R turns the selection 90 degrees about Y {min:?} {max:?}");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(verts(), flipped_xy);

    ed.call("select", json!({ "ids": [a] }));
    press("top", "I", &["ctrl", "shift"]);
    assert_eq!(selected(&ed), vec![far], "Ctrl+Shift+I selects the inverse");

    ed.call("run_action", json!({ "action": "select_none" }));
    let before = undo_len();
    press("front", "L", &["ctrl"]);
    press("front", "I", &["ctrl"]);
    assert_eq!(undo_len(), before, "nothing selected, nothing flipped, no undo step");
    assert_eq!(verts(), flipped_xy);

    let e = ed.call_err("run_action", json!({ "action": "flip_horizontal" }));
    assert!(e.contains("flip {axis}"), "{e}");
    assert!(ed.call_err("run_action", json!({ "action": "flip_vertical" })).contains("flip {axis}"));

    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    ed.call("select", json!({ "ids": [a] }));
    press("top", "I", &["ctrl"]);
    assert_eq!(selected(&ed), vec![far], "TrenchBroom keeps Select Inverse on Ctrl+I");
    press("top", "L", &["ctrl"]);
    assert_eq!(verts(), flipped_xy, "and Ctrl+L stays a CSG intersect, which leaves a lone brush alone");
}

/// Ctrl+M opens the Transform Selection window in the Hammer preset and still merges brushes in TrenchBroom, where the
/// window has no key.
#[test]
#[ignore]
fn ctrl_m_opens_transform_selection_in_hammer_and_merges_in_trenchbroom() {
    let ed = Editor::launch("hammer_transform_dialog");
    let a = ed.box_brush([40.0, 8.0, -72.0], [104.0, 72.0, -20.0]);
    let b = ed.box_brush([104.0, 8.0, -72.0], [136.0, 72.0, -20.0]);
    ed.call("select", json!({ "ids": [a, b] }));
    let press = |mods: &[&str]| ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "M", "modifiers": mods }]));

    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    press(&["ctrl"]);
    assert!(!window_titles(&ed).contains(&"Transform Selection".to_string()), "{:?}", window_titles(&ed));
    assert_eq!(selected(&ed).len(), 1, "TrenchBroom Ctrl+M merges the two brushes into one");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.state()["selection"]["nodes"].as_array().unwrap().len(), 2);

    ed.call("set_editor", json!({ "keymap": "hammer" }));
    ed.call("select", json!({ "ids": [a, b] }));
    press(&["ctrl", "shift"]);
    assert_eq!(selected(&ed).len(), 1, "the merge moved to Ctrl+Shift+M in Hammer");
    ed.call("run_action", json!({ "action": "undo" }));
    ed.call("select", json!({ "ids": [a, b] }));
    press(&["ctrl"]);
    assert!(window_titles(&ed).contains(&"Transform Selection".to_string()), "{:?}", window_titles(&ed));
    assert_eq!(selected(&ed).len(), 2, "Hammer Ctrl+M leaves the brushes alone");

    assert!(ed.call_err("run_action", json!({ "action": "transform_selection" })).contains("transform {translate, rotate, scale}"));
}

/// Ctrl+T ties the selected brushes to a brush entity through the class picker in the Hammer preset, where Select
/// Touching moved to Ctrl+Shift+T. TrenchBroom keeps Select Touching on Ctrl+T.
#[test]
#[ignore]
fn hammer_ctrl_t_ties_brushes_to_an_entity_and_trenchbroom_keeps_select_touching() {
    let ed = Editor::launch("hammer_tie");
    let a = ed.box_brush([40.0, 8.0, -72.0], [104.0, 72.0, -20.0]);
    let b = ed.box_brush([200.0, 8.0, -72.0], [232.0, 72.0, -20.0]);
    ed.call("select", json!({ "ids": [a, b] }));
    let press = |key: &str, mods: &[&str]| ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": key, "modifiers": mods }]));
    let entities = || ed.call("list_nodes", json!({ "type": "entity" }));
    let parent_of = |id: u64| {
        let brushes = ed.call("list_nodes", json!({ "type": "brush" }));
        brushes["nodes"].as_array().unwrap().iter().find(|n| n["id"] == id).map(|n| n["parent"].as_u64().unwrap()).unwrap()
    };
    let layer = parent_of(a);

    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    press("T", &["ctrl"]);
    assert_eq!(entities()["total"], 0, "TrenchBroom Ctrl+T does not tie");
    assert_eq!(ed.state()["map"]["brushes"], 0, "it selected touching, which deleted the two selecting brushes");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(parent_of(a), layer);

    ed.call("set_editor", json!({ "keymap": "hammer" }));
    ed.call("select", json!({ "ids": [a, b] }));
    press("T", &["ctrl"]);
    ed.input("top", json!([{ "type": "key", "key": "Escape" }]));
    assert_eq!((entities()["total"].as_u64(), parent_of(a), parent_of(b)), (Some(0), layer, layer), "Escape changes nothing");
    assert_eq!(selected_sorted(&ed), vec![a, b]);

    // The picker must not stay behind in the command palette.
    ed.input(
        "top",
        json!([{ "type": "key", "key": "P", "modifiers": ["ctrl", "shift"] }, { "type": "text", "text": "grid larger" }, { "type": "key", "key": "Enter" }]),
    );
    assert_eq!(ed.state()["editor"]["grid"], 32.0, "the palette lists commands again");

    press("T", &["ctrl"]);
    ed.input("top", json!([{ "type": "key", "key": "Enter" }]));
    let nodes = entities();
    assert_eq!(nodes["total"], 1, "{nodes}");
    assert_eq!(nodes["nodes"][0]["classname"], "func_detail");
    let entity = nodes["nodes"][0]["id"].as_u64().unwrap();
    assert_eq!((parent_of(a), parent_of(b)), (entity, entity));
    assert_eq!(selected(&ed), vec![entity], "the new entity is selected");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!((entities()["total"].as_u64(), parent_of(a), parent_of(b)), (Some(0), layer, layer), "one undo unties both");

    ed.call("select", json!({ "ids": [a, b] }));
    press("T", &["ctrl"]);
    ed.input("top", json!([{ "type": "text", "text": "func_geo" }]));
    ed.input("top", json!([{ "type": "key", "key": "Enter" }]));
    let made = entities();
    assert_eq!(made["nodes"][0]["classname"], "func_geo", "a typed class is the one that is made: {made}");
    ed.call("select", json!({ "ids": [a, b] }));
    press("T", &["ctrl"]);
    assert_eq!(selected(&ed), vec![entity_of(&entities())], "tying a whole entity again selects it and changes nothing");

    ed.call("select", json!({ "ids": [a] }));
    press("T", &["ctrl", "shift"]);
    assert_eq!(ed.state()["map"]["brushes"], 1, "Hammer Ctrl+Shift+T selects touching, which deletes the selecting brush");

    assert!(ed.call_err("run_action", json!({ "action": "tie_to_entity" })).contains("create_brush_entity {classname}"));
}

fn entity_of(entities: &Value) -> u64 {
    entities["nodes"][0]["id"].as_u64().unwrap()
}

#[test]
#[ignore]
fn the_mesh_tool_keeps_ctrl_l_for_select_linked_in_the_hammer_preset() {
    let ed = Editor::launch("hammer_mesh_ctrl_l");
    ed.call("set_editor", json!({ "keymap": "hammer" }));
    ed.call("create_mesh", json!({ "shape": "cuboid", "min": [-32, 0, -32], "max": [32, 64, 32] }));
    ed.call("set_editor", json!({ "tool": "mesh", "mesh_component": "face" }));
    ed.call("set_camera", json!({ "view": "3d", "position": [-120, 200, 160], "look_at": [0, 32, 0] }));
    ed.input("3d", json!([{ "type": "click", "world": [0, 64, 0] }]));
    let faces = || ed.state()["editor"]["mesh_selection"][0]["faces"].as_array().unwrap().len();
    assert_eq!(faces(), 1);
    let labels = || ed.state()["undo"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect::<Vec<_>>();
    let before = labels();
    ed.input("3d", json!([{ "type": "move" }, { "type": "key", "key": "L", "modifiers": ["ctrl"] }]));
    assert_eq!(faces(), 6, "Ctrl+L selected the whole linked mesh");
    assert!(!labels().iter().any(|l| l == "Flip") && labels().len() <= before.len() + 1, "and flipped nothing {:?}", labels());
}

#[test]
#[ignore]
fn ctrl_a_in_the_hammer_texture_tool_selects_faces() {
    let ed = Editor::launch("hammer_ctrl_a");
    let a = ed.box_brush([8.0, -16.0, 0.0], [72.0, 48.0, 40.0]);
    let wedge = ed.call("create_brush", json!({ "min": [-72, 8, 200], "max": [-8, 72, 296], "shape": "wedge" }))["ids"][0].as_u64().unwrap();
    let c = ed.box_brush([300.0, 4.0, 12.0], [332.0, 52.0, 76.0]);
    let light = ed.call("create_entity", json!({ "classname": "light", "origin": [250, 30, 100] }))["id"].as_u64().unwrap();
    let ctrl_a = || ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "A", "modifiers": ["ctrl"] }]));
    let faces_of = |id: u64, n: u64| (0..n).map(|f| (id, f)).collect::<Vec<_>>();

    assert_eq!(ed.state()["editor"]["keymap"], "trenchbroom");
    ed.call("set_editor", json!({ "tool": "texture" }));
    ed.call("select", json!({ "faces": [[c, 1]] }));
    ctrl_a();
    assert_eq!(selection_of(&ed), (vec![a, wedge, c, light], vec![]), "TrenchBroom: every object, faces clear, even in the Texture tool");

    let e = ed.call_err("set_editor", json!({ "keymap": "maya", "grid": 8 }));
    assert!(e.contains("trenchbroom, hammer, blender"), "{e}");
    assert_eq!(ed.state()["editor"]["keymap"], "trenchbroom");
    assert_ne!(ed.state()["editor"]["grid"], 8, "a refused call changes nothing");

    assert_eq!(ed.call("set_editor", json!({ "keymap": "hammer" }))["keymap"], "hammer");
    assert_eq!(ed.state()["editor"]["keymap"], "hammer");
    ed.call("select", json!({ "ids": [a, wedge] }));
    ctrl_a();
    assert_eq!(selection_of(&ed), (vec![], [faces_of(a, 6), faces_of(wedge, 5)].concat()), "a box and a wedge, 11 faces");

    ed.call("select", json!({ "faces": [[c, 1], [wedge, 3]] }));
    ctrl_a();
    assert_eq!(selection_of(&ed), (vec![], [faces_of(wedge, 5), faces_of(c, 6)].concat()), "the owners of the selected faces");

    ed.call("select", json!({ "ids": [light] }));
    ctrl_a();
    assert_eq!(selection_of(&ed), (vec![light], vec![]), "an entity has no faces, nothing changes");
    assert!(ed.state()["status"].as_str().unwrap_or_default().contains("brush or mesh"), "{}", ed.state()["status"]);

    ed.call("set_editor", json!({ "tool": "select" }));
    ed.call("select", json!({ "faces": [[c, 1]] }));
    ctrl_a();
    assert_eq!(selection_of(&ed), (vec![a, wedge, c, light], vec![]), "Hammer outside the Texture tool selects objects");
}

/// The Block tool draws a box wherever a drag starts, also on the selection the Select tool would move. A drag on a 2D
/// edge of the selection still resizes it, and Alt+drag in 3D orbits even when it starts on the selection.
#[test]
#[ignore]
fn the_block_tool_draws_over_the_selection() {
    let ed = Editor::launch("block_tool");
    let a = ed.box_brush([40.0, 8.0, -72.0], [104.0, 72.0, -20.0]);
    ed.call("set_editor", json!({ "grid": 8, "snap": true, "keymap": "hammer" }));
    ed.call("set_camera", json!({ "view": "top", "center": [72, 0, -46], "zoom": 1.5 }));
    let bounds = || -> Vec<[[i64; 3]; 2]> {
        let nodes = ed.call("list_nodes", json!({ "type": "brush" }));
        let mut all: Vec<_> = nodes["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| ["min", "max"].map(|end| [0, 1, 2].map(|i| n["bounds"][end][i].as_f64().unwrap().round() as i64)))
            .collect();
        all.sort();
        all
    };
    let tool = || ed.state()["editor"]["tool"].clone();
    let undo = || ed.call("run_action", json!({ "action": "undo" }));
    let shift_b = || ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "B", "modifiers": ["shift"] }]));
    let drag = |from: [i64; 3], to: [i64; 3]| {
        ed.call("select", json!({ "ids": [a] }));
        ed.input("top", json!([{ "type": "drag", "world": from, "to_world": to }]));
    };
    let home = [[40, 8, -72], [104, 72, -20]];

    shift_b();
    assert_eq!(tool(), "Block");
    drag([60, 0, -50], [97, 0, -29]);
    let after = bounds();
    assert_eq!(after.len(), 2, "{after:?}");
    assert!(after.contains(&home), "the selection stays put {after:?}");
    let drawn = after.iter().find(|b| **b != home).unwrap();
    assert_eq!((drawn[0][1], drawn[1][1]), (8, 72), "the box takes the selection's depth {drawn:?}");
    assert_eq!(ed.state()["undo"][0], "Create Brush");
    undo();

    drag([104, 0, -46], [120, 0, -46]);
    assert_eq!(bounds(), vec![[[40, 8, -72], [120, 72, -20]]], "a 2D edge drag still resizes");
    undo();

    shift_b();
    assert_eq!(tool(), "Select", "Shift+B again goes back to Select");
    drag([60, 0, -50], [97, 0, -29]);
    assert_eq!(bounds(), vec![[[80, 8, -48], [144, 72, 4]]], "the Select tool moves the selection instead");
    undo();

    ed.call("run_action", json!({ "action": "set_tool", "args": { "tool": "block" } }));
    let slab = ed.box_brush([-384.0, 112.0, -384.0], [384.0, 128.0, 384.0]);
    ed.call("select", json!({ "ids": [slab] }));
    ed.call("set_camera", json!({ "view": "3d", "position": [-300, 250, 300], "look_at": [0, 64, 0] }));
    let camera = || ed.state()["cameras"].as_array().unwrap().iter().find(|c| c["view"] == "3d").unwrap()["position"].clone();
    let before = camera();
    ed.input("3d", json!([{ "type": "drag", "world": [-200, 128, 200], "to_world": [-200, 200, 200], "modifiers": ["alt"], "steps": 12 }]));
    assert_ne!(camera(), before, "Alt+drag on the selection orbits");
    assert_eq!(ed.brushes(), 2);
    assert!(bounds().contains(&[[-384, 112, -384], [384, 128, 384]]), "the slab stays put {:?}", bounds());
}

#[test]
#[ignore]
fn hammer_select_box_waits_for_enter_or_a_click_inside() {
    let ed = Editor::launch("hammer_select_box");
    let a = ed.box_brush([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]);
    let b = ed.box_brush([96.0, 0.0, 0.0], [160.0, 64.0, 64.0]);
    let c = ed.box_brush([0.0, 0.0, 192.0], [64.0, 64.0, 256.0]);
    ed.call("set_editor", json!({ "grid": 8, "snap": true, "keymap": "hammer" }));
    ed.call("set_camera", json!({ "view": "top", "center": [160, 0, 160], "zoom": 0.5 }));
    ed.call("select", json!({ "ids": [] }));
    let pending = || ed.state()["editor"]["selection_box"].clone();
    let drag = |from: [i64; 3], to: [i64; 3], modifiers: Value| {
        ed.input("top", json!([{ "type": "drag", "world": from, "to_world": to, "modifiers": modifiers }]));
    };
    let key = |key: &str| ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": key }]));

    drag([-48, 0, -48], [208, 0, 112], json!([]));
    let waiting = pending();
    assert_eq!((waiting["view"].clone(), waiting["add"].clone()), (json!("top"), json!(false)), "{waiting}");
    assert!(selected(&ed).is_empty(), "the box waits");
    key("Enter");
    assert!(pending().is_null());
    assert_eq!(selected_sorted(&ed), [a, b], "c sticks out below the box");

    drag([-48, 0, 160], [96, 0, 288], json!(["ctrl"]));
    assert_eq!(pending()["add"], true);
    ed.input("top", json!([{ "type": "click", "world": [80, 0, 280] }]));
    assert!(pending().is_null());
    assert_eq!(selected_sorted(&ed), [a, b, c], "a click inside adds what the box holds");

    drag([300, 0, 300], [360, 0, 360], json!([]));
    assert!(!pending().is_null());
    key("Escape");
    assert!(pending().is_null(), "Esc drops the box");
    assert_eq!(selected_sorted(&ed), [a, b, c], "and keeps the selection");

    ed.call("select", json!({ "ids": [] }));
    drag([-48, 0, -48], [80, 0, 80], json!([]));
    ed.input("top", json!([{ "type": "click", "world": [128, 0, 32] }]));
    assert!(pending().is_null());
    assert_eq!(selected(&ed), [b], "a click outside drops the box and clicks as usual");

    ed.call("set_camera", json!({ "view": "3d", "position": [-300, 250, 300], "look_at": [0, 0, 0] }));
    ed.input("3d", json!([{ "type": "drag", "world": [200, 0, -200], "to_world": [300, 0, -150] }]));
    assert_eq!(ed.brushes(), 3, "a 3D drag off the selection draws nothing");
    assert_eq!(selected(&ed), [b]);
    assert!(pending().is_null());
    let status = ed.state()["status"].as_str().unwrap().to_string();
    assert!(status.starts_with("The Block tool draws brushes"), "{status}");

    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    drag([300, 0, 300], [360, 0, 360], json!([]));
    assert!(pending().is_null());
    assert_eq!(ed.brushes(), 4, "TrenchBroom draws a brush");
}

#[test]
#[ignore]
fn hammer_space_drag_pans_and_looks() {
    let ed = Editor::launch("hammer_space_drag");
    let a = ed.box_brush([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]);
    ed.call("set_editor", json!({ "grid": 8, "snap": true, "keymap": "hammer" }));
    ed.call("set_camera", json!({ "view": "top", "center": [100, 0, 120], "zoom": 1.0 }));
    ed.call("select", json!({ "ids": [a] }));
    let camera = |view: &str| ed.state()["cameras"].as_array().unwrap().iter().find(|c| c["view"] == view).unwrap().clone();
    let space_drag = |view: &str, from: [i64; 3], to: [i64; 3]| {
        ed.input(view, json!([{ "type": "drag", "world": from, "to_world": to, "keys": ["Space"], "steps": 12 }]));
    };

    let before = camera("top")["center"].clone();
    space_drag("top", [200, 0, 200], [280, 0, 160]);
    let after = camera("top")["center"].clone();
    let (dx, dz) = (after[0].as_f64().unwrap() - before[0].as_f64().unwrap(), after[2].as_f64().unwrap() - before[2].as_f64().unwrap());
    assert!((-81.0..=-40.0).contains(&dx) && (20.0..=41.0).contains(&dz), "Space+drag pans the 2D view by the drag, {dx} {dz}");
    assert_eq!(ed.brushes(), 1);
    assert_eq!(selected(&ed), [a], "and leaves the selection alone");
    assert!(ed.state()["editor"]["selection_box"].is_null());

    ed.call("set_camera", json!({ "view": "3d", "position": [-300, 250, 300], "look_at": [0, 0, 0] }));
    let before = camera("3d");
    space_drag("3d", [200, 0, -200], [300, 0, -150]);
    let after = camera("3d");
    assert_ne!(after["forward"], before["forward"], "Space+drag looks around in 3D");
    assert_eq!(after["position"], before["position"]);
    assert_eq!(ed.brushes(), 1);

    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    space_drag("top", [200, 0, 200], [280, 0, 280]);
    assert_eq!(ed.brushes(), 2, "TrenchBroom leaves Space alone and draws a brush");
}

#[test]
#[ignore]
fn hammer_vertex_tool_boxes_corners_in_2d() {
    let ed = Editor::launch("hammer_vertex_box");
    let w = ed.call("create_brush", json!({ "shape": "wedge", "min": [-40, 8, 24], "max": [24, 72, 152] }))["ids"][0].as_u64().unwrap();
    ed.call("set_editor", json!({ "grid": 8, "snap": true, "keymap": "hammer", "tool": "vertex" }));
    ed.call("select", json!({ "ids": [w] }));
    ed.call("set_camera", json!({ "view": "front", "center": [-8, 40, 0], "zoom": 0.5 }));
    let corners = || -> Vec<[i64; 3]> {
        let mut all: Vec<[i64; 3]> =
            ed.state()["editor"]["selected_vertices"].as_array().unwrap().iter().map(|v| [0, 1, 2].map(|i| v[i].as_f64().unwrap().round() as i64)).collect();
        all.sort();
        all
    };
    let drag = |view: &str, from: [i64; 3], to: [i64; 3], modifiers: Value| {
        ed.input(view, json!([{ "type": "drag", "world": from, "to_world": to, "modifiers": modifiers }]));
    };

    drag("front", [-72, 48, 0], [40, 96, 0], json!([]));
    assert_eq!(corners(), [[-40, 72, 24], [24, 72, 24]], "the box takes the top edge of the slope");
    assert_eq!(ed.state()["status"], "Selected 2 corners");

    drag("front", [-40, 72, 24], [-40, 88, 24], json!([]));
    assert_eq!(corners(), [[-40, 88, 24], [24, 88, 24]], "a drag on one of them moves both");
    let (_, max) = ed.selection_bounds();
    assert!((max[1] - 88.0).abs() < 1e-3, "{max:?}");

    drag("front", [-72, -24, 0], [-24, 24, 0], json!(["ctrl"]));
    assert_eq!(corners(), [[-40, 8, 24], [-40, 8, 152], [-40, 88, 24], [24, 88, 24]], "Ctrl adds both stacked bottom corners");

    drag("front", [80, 100, 0], [110, 130, 0], json!([]));
    assert_eq!(corners().len(), 4, "an empty box keeps the corners");
    assert_eq!(ed.state()["status"], "No corner lies inside the box");

    ed.call("set_camera", json!({ "view": "3d", "position": [-300, 250, 300], "look_at": [0, 0, 0] }));
    ed.input("3d", json!([{ "type": "drag", "world": [200, 0, -200], "to_world": [300, 0, -150] }]));
    assert_eq!(corners().len(), 4, "the 3D view draws no box");

    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    drag("front", [-72, 48, 0], [40, 96, 0], json!([]));
    assert_eq!(corners().len(), 4, "TrenchBroom draws no box");
}

#[test]
#[ignore]
fn hammer_mouse_look_toggles_with_z() {
    let ed = Editor::launch("hammer_mouse_look");
    let a = ed.box_brush([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]);
    ed.call("set_editor", json!({ "keymap": "hammer" }));
    ed.call("set_camera", json!({ "view": "3d", "position": [-300, 250, 300], "look_at": [32, 32, 32] }));
    ed.call("run_action", json!({ "action": "select_none" }));
    let camera = || ed.state()["cameras"].as_array().unwrap().iter().find(|c| c["view"] == "3d").unwrap().clone();
    let looking = || ed.state()["editor"]["mouse_look"].as_bool().unwrap();
    let z = |view: &str| ed.input(view, json!([{ "type": "move", "world": [32, 32, 32] }, { "type": "key", "key": "Z" }]));
    let motion = |dx: i64, dy: i64| ed.input("3d", json!([{ "type": "motion", "dx": dx, "dy": dy }]));

    z("3d");
    assert!(looking(), "Z over the 3D view starts mouse look");
    let tool = ed.state()["editor"]["tool"].clone();
    let before = camera();
    motion(40, -15);
    let turned = camera();
    assert_ne!(turned["forward"], before["forward"], "moving the mouse turns the camera");
    assert_eq!(turned["position"], before["position"]);
    motion(40, 0);
    let turned_again = camera();
    assert_ne!(turned_again["forward"], turned["forward"], "and keeps turning it");

    ed.input("3d", json!([{ "type": "key", "key": "W" }]));
    assert_ne!(camera()["position"], turned_again["position"], "W flies");
    assert_eq!(ed.state()["editor"]["tool"], tool, "and picks no tool");

    ed.input("3d", json!([{ "type": "key", "key": "Z" }]));
    assert!(!looking(), "Z again stops it");
    let still = camera();
    motion(40, 0);
    assert_eq!(camera()["forward"], still["forward"], "then the mouse no longer turns the view");

    z("3d");
    ed.input("3d", json!([{ "type": "key", "key": "Escape" }]));
    assert!(!looking(), "Esc stops it");

    z("3d");
    ed.input("3d", json!([{ "type": "click", "world": [32, 32, 32] }]));
    assert!(!looking(), "a click stops it");
    assert!(selected(&ed).is_empty(), "and selects nothing");
    ed.input("3d", json!([{ "type": "click", "world": [32, 32, 32] }]));
    assert_eq!(selected(&ed), [a], "the next click selects again");

    // On Windows the hidden cursor sits in the middle of the window, which can lie over another view.
    ed.call("run_action", json!({ "action": "select_none" }));
    z("3d");
    ed.input("top", json!([{ "type": "click", "world": [32, 32, 32] }]));
    assert!(!looking(), "a click over another view stops it");
    assert!(selected(&ed).is_empty(), "and that view sees no click");
    assert!(ed.state()["editor"]["selection_box"].is_null());
    ed.input("top", json!([{ "type": "click", "world": [32, 32, 32] }]));
    assert_eq!(selected(&ed), [a], "the next click there selects");

    z("top");
    assert!(!looking(), "a 2D view has no mouse look");
    assert_eq!(ed.state()["status"], "Mouse look works in the 3D view");

    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    z("3d");
    assert!(!looking(), "TrenchBroom binds no key to it");
}

/// The Hammer preset clones on Shift+drag, Ctrl+drag too, and has no Shift axis lock. TrenchBroom keeps Shift as the
/// axis lock and as the face push in 3D.
#[test]
#[ignore]
fn hammer_shift_drag_clones_the_selection_and_trenchbroom_locks_the_axis() {
    let ed = Editor::launch("hammer_shift_drag");
    let a = ed.box_brush([40.0, 8.0, -72.0], [104.0, 72.0, -20.0]);
    ed.call("set_editor", json!({ "grid": 8, "snap": true }));
    ed.call("set_camera", json!({ "view": "top", "center": [72, 0, -46], "zoom": 1.5 }));
    let bounds = || -> Vec<[[i64; 3]; 2]> {
        let nodes = ed.call("list_nodes", json!({ "type": "brush" }));
        let mut all: Vec<_> = nodes["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| ["min", "max"].map(|end| [0, 1, 2].map(|i| n["bounds"][end][i].as_f64().unwrap().round() as i64)))
            .collect();
        all.sort();
        all
    };
    let undo_len = || ed.state()["undo"].as_array().unwrap().len();
    let drag = |mods: Vec<&str>| {
        ed.call("select", json!({ "ids": [a] }));
        ed.input("top", json!([{ "type": "drag", "world": [60, 0, -50], "to_world": [97, 0, -29], "modifiers": mods }]));
    };
    let home = [[40, 8, -72], [104, 72, -20]];
    let moved = [[80, 8, -48], [144, 72, 4]];

    ed.call("set_editor", json!({ "keymap": "hammer" }));
    let before = undo_len();
    drag(vec!["shift"]);
    assert_eq!(bounds(), vec![home, moved], "Hammer Shift+drag leaves the brush and moves the copy on both axes");
    assert_eq!(undo_len(), before + 1, "one undo step");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(bounds(), vec![home]);

    drag(vec!["ctrl"]);
    assert_eq!(bounds(), vec![home, moved], "Hammer Ctrl+drag copies too");
    ed.call("run_action", json!({ "action": "undo" }));

    drag(vec![]);
    assert_eq!(bounds(), vec![moved], "a plain Hammer drag moves it on both axes");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(bounds(), vec![home]);

    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    drag(vec!["shift"]);
    assert_eq!(bounds(), vec![[[80, 8, -72], [144, 72, -20]]], "TrenchBroom Shift+drag moves, locked to the main axis, and copies nothing");
    ed.call("run_action", json!({ "action": "undo" }));

    drag(vec!["ctrl"]);
    assert_eq!(bounds(), vec![home, moved], "Ctrl+drag copies in TrenchBroom");
    ed.call("run_action", json!({ "action": "undo" }));

    // A light far from the brush drags the gizmo's center away, so the drag starts on a face and not on a handle.
    ed.call("select", json!({ "ids": [a] }));
    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "Delete" }]));
    let big = ed.box_brush([240.0, 0.0, -300.0], [560.0, 64.0, -60.0]);
    let home = [[240, 0, -300], [560, 64, -60]];
    ed.call("set_camera", json!({ "view": "3d", "position": [160, 260, 260], "look_at": [400, 30, -180] }));
    let light = ed.call("create_entity", json!({ "classname": "light", "origin": [1000, 200, -600] }))["id"].as_u64().unwrap();
    let face_drag = || {
        ed.call("select", json!({ "ids": [big, light] }));
        ed.input("3d", json!([{ "type": "drag", "world": [270, 64, -90], "to_world": [270, 120, -90], "modifiers": ["shift"], "steps": 12 }]));
    };
    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    face_drag();
    let tb = bounds();
    assert_eq!(tb.len(), 1, "TrenchBroom Shift+drag on a selected face resizes, it does not copy {tb:?}");
    assert!(tb[0][1][1] > 64 && tb[0][0] == home[0], "{tb:?}");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(bounds(), vec![home]);

    ed.call("set_editor", json!({ "keymap": "hammer" }));
    face_drag();
    let hammer = bounds();
    assert_eq!(hammer.len(), 2, "Hammer Shift+drag on a selected face copies {hammer:?}");
    assert!(hammer.contains(&home), "the brush keeps its size {hammer:?}");
    let copy = hammer.iter().find(|b| **b != home).unwrap();
    assert_eq!([0, 1, 2].map(|i| copy[1][i] - copy[0][i]), [320, 64, 240], "the copy keeps its size {copy:?}");
}

/// The Blender preset's 1, 3, A and Alt+A, pressed in the 3D view: the keys switch between Object and Face mode and A
/// selects everything in the current mode.
#[test]
#[ignore]
fn blender_one_and_three_switch_modes_and_a_selects_in_the_mode() {
    let ed = Editor::launch("blender_modes");
    let a = ed.box_brush([8.0, -16.0, 0.0], [72.0, 48.0, 40.0]);
    let wedge = ed.call("create_brush", json!({ "min": [-72, 8, 200], "max": [-8, 72, 296], "shape": "wedge" }))["ids"][0].as_u64().unwrap();
    let c = ed.box_brush([300.0, 4.0, 12.0], [332.0, 52.0, 76.0]);
    let light = ed.call("create_entity", json!({ "classname": "light", "origin": [250, 30, 100] }))["id"].as_u64().unwrap();
    let key = |key: &str, modifiers: Vec<&str>| ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": key, "modifiers": modifiers }]));
    let faces_of = |id: u64, n: u64| (0..n).map(|f| (id, f)).collect::<Vec<_>>();
    let mode = || ed.state()["editor"]["select_mode"].as_str().unwrap_or_default().to_string();

    assert_eq!(mode(), "object");
    ed.call("set_editor", json!({ "keymap": "blender" }));
    ed.call("select", json!({ "ids": [a, wedge, light] }));
    key("3", vec![]);
    assert_eq!(
        (mode(), selection_of(&ed)),
        ("face".into(), (vec![], [faces_of(a, 6), faces_of(wedge, 5)].concat())),
        "3: faces of the brushes, the light drops out"
    );
    key("1", vec![]);
    assert_eq!((mode(), selection_of(&ed)), ("object".into(), (vec![a, wedge], vec![])), "1: back to the owners");

    ed.call("run_action", json!({ "action": "select_none" }));
    key("3", vec![]);
    assert_eq!((mode(), selection_of(&ed)), ("face".into(), (vec![], vec![])), "3 with nothing selected");
    key("A", vec![]);
    assert_eq!(
        selection_of(&ed),
        (vec![], [faces_of(a, 6), faces_of(wedge, 5), faces_of(c, 6)].concat()),
        "A in Face mode: every face of every brush, no light"
    );
    key("A", vec!["alt"]);
    assert_eq!((mode(), selection_of(&ed)), ("face".into(), (vec![], vec![])), "Alt+A clears and keeps the mode");

    key("1", vec![]);
    key("A", vec![]);
    assert_eq!((mode(), selection_of(&ed)), ("object".into(), (vec![a, wedge, c, light], vec![])), "A in Object mode: every object");

    ed.call("select", json!({ "faces": [[c, 2]] }));
    assert_eq!(mode(), "face", "a selection written by a tool call moves the mode");
    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    key("3", vec![]);
    assert_eq!(selection_of(&ed), (vec![], vec![(c, 2)]), "3 means nothing in the TrenchBroom preset");
}

/// The TrenchBroom click rules in the 3D view: a click replaces the selection, Ctrl+click toggles an object, Shift+click
/// picks one face and Ctrl+Shift+click toggles a face. Shift on something without faces acts like a click.
#[test]
#[ignore]
fn shift_click_picks_one_face_and_ctrl_shift_click_toggles_faces() {
    let ed = Editor::launch("click_rules");
    let a = ed.box_brush([8.0, -16.0, 0.0], [72.0, 48.0, 40.0]);
    let b = ed.box_brush([136.0, 4.0, 12.0], [200.0, 52.0, 76.0]);
    let wedge = ed.call("create_brush", json!({ "min": [-72, 8, 200], "max": [-8, 72, 296], "shape": "wedge" }))["ids"][0].as_u64().unwrap();
    let light = ed.call("create_entity", json!({ "classname": "light", "origin": [250, 30, 100] }))["id"].as_u64().unwrap();
    ed.call("select", json!({ "ids": [] }));

    // The 3D view puts a transform gizmo over a selected object that takes clicks on small boxes, so the clicks go
    // through the top view for top and slanted faces and the front view for the front face.
    let click = |view: &str, at: [f64; 3], modifiers: &[&str]| {
        ed.call("set_camera", json!({ "view": view, "center": at, "zoom": 1.0 }));
        ed.input(view, json!([{ "type": "click", "world": at, "modifiers": modifiers }]));
    };
    let nodes = || {
        let mut n = selected(&ed);
        n.sort_unstable();
        n
    };
    let faces = || {
        let state = ed.state();
        let mut f: Vec<(u64, u64)> = state["selection"]["faces"].as_array().unwrap().iter().map(|p| (p[0].as_u64().unwrap(), p[1].as_u64().unwrap())).collect();
        f.sort_unstable();
        f
    };
    let normal = |(id, face): (u64, u64)| -> Vec<f64> {
        let got = ed.call("texture", json!({ "op": "get", "faces": [[id, face]] }));
        got["faces"][0]["normal"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()
    };
    let top_a = [14.0, 48.0, 4.0];
    let top_b = [192.0, 52.0, 70.0];
    let front_b = [192.0, 10.0, 76.0];
    let slant = [-14.0, 59.2, 219.2];
    let empty = [150.0, 20.0, 250.0];

    click("top", top_a, &[]);
    assert_eq!((nodes(), faces()), (vec![a], vec![]), "a click selects the object");
    click("top", top_a, &["shift"]);
    let first = faces();
    assert_eq!((nodes(), first.len()), (vec![], 1), "Shift+click leaves the object for one face of it");
    assert_eq!(first[0].0, a);
    assert!(approx(&normal(first[0]), &[0.0, 1.0, 0.0]), "the face under the cursor is the top");

    click("top", top_b, &["ctrl", "shift"]);
    let both = faces();
    assert_eq!(both.len(), 2, "Ctrl+Shift+click adds a face on another brush: {both:?}");
    assert_eq!(both.iter().map(|f| f.0).collect::<Vec<_>>(), vec![a, b]);
    assert!(nodes().is_empty());

    click("front", front_b, &["ctrl", "shift"]);
    assert_eq!(faces().len(), 3, "a second face of the same brush is added");
    click("front", front_b, &["ctrl", "shift"]);
    assert_eq!(faces(), both, "Ctrl+Shift+click on a selected face removes just that face");

    click("top", slant, &["ctrl", "shift"]);
    let with_slant = faces();
    assert_eq!(with_slant.len(), 3);
    let slanted = *with_slant.iter().find(|f| f.0 == wedge).expect("the wedge face is selected");
    let n = normal(slanted);
    assert!(n[0].abs() < 1e-3 && (n[1] - 0.8321).abs() < 1e-3 && (n[2] - 0.5547).abs() < 1e-3, "the slanted face {n:?}");

    click("front", front_b, &["shift"]);
    let only = faces();
    assert_eq!(only.len(), 1, "Shift+click replaces the face selection: {only:?}");
    assert_eq!(only[0].0, b);
    assert!(approx(&normal(only[0]), &[0.0, 0.0, 1.0]), "the front face of B");

    click("top", top_a, &[]);
    assert_eq!((nodes(), faces()), (vec![a], vec![]), "a plain click on an object leaves face selection");
    click("top", top_b, &["ctrl"]);
    assert_eq!((nodes(), faces()), (vec![a, b], vec![]), "Ctrl+click adds an object");
    click("top", top_a, &["ctrl"]);
    assert_eq!((nodes(), faces()), (vec![b], vec![]), "Ctrl+click on a selected object removes it");

    click("top", top_a, &["shift"]);
    assert_eq!(faces().len(), 1);
    click("top", [250.0, 30.0, 100.0], &["shift"]);
    assert_eq!((nodes(), faces()), (vec![light], vec![]), "Shift+click on an entity selects it like a click");
    click("top", top_a, &["shift"]);
    click("top", [250.0, 30.0, 100.0], &["ctrl", "shift"]);
    assert_eq!((nodes(), faces()), (vec![light], vec![]), "Ctrl+Shift on an entity toggles the object, and objects and faces never mix");

    click("top", top_a, &["shift"]);
    click("top", empty, &["ctrl"]);
    assert_eq!((nodes(), faces().len()), (vec![], 1), "Ctrl+click on empty space keeps the selection");
    click("top", empty, &["ctrl", "shift"]);
    assert_eq!((nodes(), faces().len()), (vec![], 1), "so does Ctrl+Shift+click");
    click("top", empty, &["shift"]);
    assert_eq!((nodes(), faces()), (vec![], vec![]), "Shift+click on empty space clears like a click");
    click("top", top_a, &[]);
    click("top", empty, &[]);
    assert_eq!((nodes(), faces()), (vec![], vec![]), "a click on empty space clears");
}

/// The Hammer preset keeps Shift free for cloning, so Shift+click selects the object like a click and Ctrl+Shift+click
/// toggles it. TrenchBroom picks the face for the same clicks.
#[test]
#[ignore]
fn hammer_shift_click_selects_objects_and_trenchbroom_picks_faces() {
    let ed = Editor::launch("hammer_click_rules");
    let a = ed.box_brush([8.0, -16.0, 0.0], [72.0, 48.0, 40.0]);
    let b = ed.box_brush([136.0, 4.0, 12.0], [200.0, 52.0, 76.0]);
    ed.call("select", json!({ "ids": [] }));
    let click = |at: [f64; 3], modifiers: &[&str]| {
        ed.call("set_camera", json!({ "view": "top", "center": at, "zoom": 1.0 }));
        ed.input("top", json!([{ "type": "click", "world": at, "modifiers": modifiers }]));
    };
    let top_a = [14.0, 48.0, 4.0];
    let top_b = [192.0, 52.0, 70.0];
    let empty = [150.0, 20.0, 250.0];

    ed.call("set_editor", json!({ "keymap": "hammer" }));
    click(top_a, &["shift"]);
    assert_eq!(selection_of(&ed), (vec![a], vec![]), "Hammer Shift+click selects the brush, not a face");
    click(top_b, &["ctrl", "shift"]);
    assert_eq!(selection_of(&ed), (vec![a, b], vec![]), "Ctrl+Shift+click adds the brush");
    click(top_a, &["ctrl", "shift"]);
    assert_eq!(selection_of(&ed), (vec![b], vec![]), "Ctrl+Shift+click on a selected brush removes it");
    click(top_a, &["shift"]);
    click(empty, &["shift"]);
    assert_eq!(selection_of(&ed), (vec![], vec![]), "Shift+click on empty space clears like a click");

    ed.call("set_editor", json!({ "keymap": "trenchbroom" }));
    click(top_a, &["shift"]);
    let (nodes, faces) = selection_of(&ed);
    assert_eq!((nodes, faces.len()), (vec![], 1), "TrenchBroom Shift+click picks one face of the brush");
    assert_eq!(faces[0].0, a);
    click(top_b, &["ctrl", "shift"]);
    assert_eq!(selection_of(&ed).1.iter().map(|f| f.0).collect::<Vec<_>>(), vec![a, b], "Ctrl+Shift+click adds a face");
}

/// The Blender preset's clicks in the 3D view: Shift+click and Ctrl+click toggle in the current mode, an object in Object
/// mode and a face in Face mode, and Face mode leaves an entity alone.
#[test]
#[ignore]
fn blender_shift_click_toggles_in_the_current_mode() {
    let ed = Editor::launch("blender_clicks");
    let a = ed.box_brush([8.0, -16.0, 0.0], [72.0, 48.0, 40.0]);
    let b = ed.box_brush([136.0, 4.0, 12.0], [200.0, 52.0, 76.0]);
    let wedge = ed.call("create_brush", json!({ "min": [-72, 8, 200], "max": [-8, 72, 296], "shape": "wedge" }))["ids"][0].as_u64().unwrap();
    let light = ed.call("create_entity", json!({ "classname": "light", "origin": [250, 30, 100] }))["id"].as_u64().unwrap();
    ed.call("set_editor", json!({ "keymap": "blender" }));
    ed.call("select", json!({ "ids": [] }));
    let click = |view: &str, at: [f64; 3], modifiers: &[&str]| {
        ed.call("set_camera", json!({ "view": view, "center": at, "zoom": 1.0 }));
        ed.input(view, json!([{ "type": "click", "world": at, "modifiers": modifiers }]));
    };
    let key = |key: &str| ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": key, "modifiers": [] }]));
    let mode = || ed.state()["editor"]["select_mode"].as_str().unwrap_or_default().to_string();
    let normal = |(id, face): (u64, u64)| -> Vec<f64> {
        let got = ed.call("texture", json!({ "op": "get", "faces": [[id, face]] }));
        got["faces"][0]["normal"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()
    };
    let top_a = [14.0, 48.0, 4.0];
    let top_b = [192.0, 52.0, 70.0];
    let front_b = [192.0, 10.0, 76.0];
    let slant = [-14.0, 59.2, 219.2];
    let empty = [150.0, 20.0, 250.0];
    let on_light = [250.0, 30.0, 100.0];

    click("top", top_a, &[]);
    assert_eq!((mode(), selection_of(&ed)), ("object".into(), (vec![a], vec![])), "a click picks the object");
    click("top", top_b, &["shift"]);
    assert_eq!(selection_of(&ed), (vec![a, b], vec![]), "Shift+click adds an object in Object mode");
    click("top", top_a, &["shift"]);
    assert_eq!(selection_of(&ed), (vec![b], vec![]), "Shift+click on a selected object removes it");
    click("top", top_a, &["ctrl"]);
    assert_eq!(selection_of(&ed), (vec![a, b], vec![]), "Ctrl+click toggles the same way");
    click("top", empty, &["shift"]);
    assert_eq!(selection_of(&ed), (vec![a, b], vec![]), "Shift+click on empty space keeps the selection");
    click("top", slant, &[]);
    assert_eq!(selection_of(&ed), (vec![wedge], vec![]), "a click replaces the selection");

    key("3");
    ed.call("run_action", json!({ "action": "select_none" }));
    assert_eq!(mode(), "face");
    click("top", top_a, &[]);
    let (nodes, faces) = selection_of(&ed);
    assert_eq!((nodes, faces.len(), faces[0].0), (vec![], 1, a), "a click in Face mode picks one face");
    assert!(approx(&normal(faces[0]), &[0.0, 1.0, 0.0]), "the top face");
    click("top", top_b, &["shift"]);
    assert_eq!(selection_of(&ed).1.iter().map(|f| f.0).collect::<Vec<_>>(), vec![a, b], "Shift+click adds a face on another brush");
    click("front", front_b, &["shift"]);
    assert_eq!(selection_of(&ed).1.len(), 3, "a second face of the same brush");
    click("front", front_b, &["shift"]);
    assert_eq!(selection_of(&ed).1.len(), 2, "Shift+click on a selected face removes just that face");
    click("top", top_b, &["ctrl"]);
    assert_eq!(selection_of(&ed).1.iter().map(|f| f.0).collect::<Vec<_>>(), vec![a], "Ctrl+click toggles a face too");
    click("top", slant, &["shift"]);
    let slanted = *selection_of(&ed).1.iter().find(|f| f.0 == wedge).expect("the wedge face is selected");
    let n = normal(slanted);
    assert!(n[0].abs() < 1e-3 && (n[1] - 0.8321).abs() < 1e-3 && (n[2] - 0.5547).abs() < 1e-3, "the slanted face {n:?}");

    let before = selection_of(&ed);
    click("top", on_light, &[]);
    assert_eq!((mode(), selection_of(&ed)), ("face".into(), before.clone()), "an entity has no faces, so nothing changes");
    assert!(ed.state()["status"].as_str().unwrap().contains("press 1"), "the status points at Object mode: {}", ed.state()["status"]);
    click("top", on_light, &["shift"]);
    assert_eq!(selection_of(&ed), before, "Shift does not turn it into an object pick");
    click("top", empty, &["shift"]);
    assert_eq!(selection_of(&ed), before, "Shift+click on empty space keeps the faces");
    click("top", empty, &[]);
    assert_eq!((mode(), selection_of(&ed)), ("face".into(), (vec![], vec![])), "a click on empty space clears and stays in Face mode");

    key("1");
    click("top", on_light, &[]);
    assert_eq!((mode(), selection_of(&ed)), ("object".into(), (vec![light], vec![])), "back in Object mode the entity is picked");
}

/// Ctrl+Shift+A is Select None as in TrenchBroom, the Hammer preset keeps it for Select Brush Faces, and the MCP action
/// does the same as that menu item in every preset.
#[test]
#[ignore]
fn ctrl_shift_a_selects_none_and_hammer_keeps_it_for_brush_faces() {
    let ed = Editor::launch("ctrl_shift_a");
    let a = ed.box_brush([8.0, -16.0, 0.0], [72.0, 48.0, 40.0]);
    let b = ed.box_brush([136.0, 4.0, 12.0], [200.0, 52.0, 76.0]);
    let c = ed.box_brush([300.0, 0.0, 0.0], [332.0, 32.0, 32.0]);
    let light = ed.call("create_entity", json!({ "classname": "light", "origin": [32, 64, 0] }))["id"].as_u64().unwrap();
    let chord = || ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "A", "modifiers": ["ctrl", "shift"] }]));
    let selection = || {
        let state = ed.state();
        let mut nodes: Vec<u64> = state["selection"]["nodes"].as_array().unwrap().iter().map(|n| n.as_u64().unwrap()).collect();
        nodes.sort();
        (nodes, state["selection"]["faces"].as_array().unwrap().len())
    };

    assert_eq!(ed.state()["editor"]["keymap"], "trenchbroom");
    ed.call("select", json!({ "ids": [a, b] }));
    chord();
    assert_eq!(selection(), (vec![], 0), "TrenchBroom: the objects are deselected, no faces appear");

    ed.call("select", json!({ "ids": [c] }));
    ed.call("run_action", json!({ "action": "select_brush_faces" }));
    assert_eq!(selection(), (vec![], 6), "the menu action still selects the faces");
    chord();
    assert_eq!(selection(), (vec![], 0), "Ctrl+Shift+A clears faces too");
    chord();
    assert_eq!(selection(), (vec![], 0), "nothing selected stays so");

    ed.call("select", json!({ "ids": [a] }));
    ed.call("run_action", json!({ "action": "set_tool", "args": { "tool": "sculpt" } }));
    chord();
    assert_eq!(selection(), (vec![], 0), "Select None clears with another tool active");
    assert_eq!(ed.state()["editor"]["tool"], "Sculpt", "and does not leave it");
    ed.input("top", json!([{ "type": "move" }, { "type": "key", "key": "Escape" }]));
    assert_eq!(ed.state()["editor"]["tool"], "Select", "Escape is Cancel, it leaves the tool");

    ed.call("set_editor", json!({ "keymap": "hammer" }));
    ed.call("select", json!({ "ids": [] }));
    chord();
    assert_eq!(selection(), (vec![], 0), "nothing selected changes nothing");
    ed.call("select", json!({ "ids": [a, b] }));
    chord();
    assert_eq!(selection(), (vec![], 12), "Hammer: Ctrl+Shift+A turns the selected brushes into their faces");

    for keymap in ["trenchbroom", "hammer", "blender"] {
        ed.call("set_editor", json!({ "keymap": keymap }));
        ed.call("select", json!({ "ids": [] }));
        let e = ed.call_err("run_action", json!({ "action": "select_brush_faces" }));
        assert!(e.contains("Select a brush or mesh first"), "{keymap}: {e}");
        ed.call("select", json!({ "ids": [light] }));
        ed.call_err("run_action", json!({ "action": "select_brush_faces" }));
        assert_eq!(selection(), (vec![light], 0), "{keymap}: an entity alone is refused and stays selected");
        ed.call("select", json!({ "ids": [b] }));
        ed.call("run_action", json!({ "action": "select_brush_faces" }));
        assert_eq!(selection(), (vec![], 6), "{keymap}: one brush");
        ed.call("select", json!({ "ids": [a, b, c] }));
        ed.call("run_action", json!({ "action": "select_brush_faces" }));
        assert_eq!(selection(), (vec![], 18), "{keymap}: three brushes");
        ed.call("run_action", json!({ "action": "select_none" }));
        assert_eq!(selection(), (vec![], 0), "{keymap}: select_none over MCP clears faces too");
    }
}

/// Script ergonomics found building the night maps: ids that follow CSG, joined id lists, carve materials, full mesh
/// rotation, exposed-only scatter fills, forward slash paths, lamp fixtures and Node methods as inputs.
#[test]
#[ignore]
fn script_ids_follow_csg_and_night_map_options() {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../godot").canonicalize().unwrap();
    let ed = Editor::launch_with("night_options", &["--project", project.to_str().unwrap()]);
    let steps = json!([
        { "tool": "create_brush", "args": { "min": [0, 0, 0], "max": [256, 128, 16], "material": "dev/grey" }, "save": "wall" },
        { "tool": "create_brush", "args": { "min": [512, 0, 0], "max": [576, 64, 64], "material": "dev/grey" }, "save": "block" },
        { "tool": "create_brush", "args": { "min": [96, 32, -8], "max": [160, 96, 24], "material": "dev/orange" }, "save": "window" },
        { "tool": "select", "args": { "ids": "$window.ids" } },
        { "tool": "run_action", "args": { "action": "csg_subtract", "args": { "carve_material": "target" } }, "save": "cut" },
        { "tool": "select", "args": { "ids": "$wall.ids + $block.ids" }, "save": "both" },
        { "tool": "select", "args": { "ids": ["$wall.ids", "$block.ids"] }, "save": "nested" }
    ]);
    let summary = ed.call("run_script", json!({ "steps": steps }));
    assert_eq!(summary["errors"], json!([]));
    let vars = &summary["vars"];
    let pieces = vars["wall"]["ids"].as_array().unwrap();
    assert_eq!(pieces.len(), 4, "the saved wall ids now name the four pieces around the window: {vars}");
    assert_eq!(vars["window"]["ids"], json!([]), "the cutter is gone");
    assert_eq!(vars["cut"]["replaced"].as_object().unwrap().len(), 2, "{}", vars["cut"]);
    assert_eq!(vars["both"]["selected"].as_array().unwrap().len(), 5, "joined lists select the pieces and the block");
    assert_eq!(vars["nested"]["selected"], vars["both"]["selected"]);
    for id in pieces {
        let node = ed.call("get_node", json!({ "id": id }));
        assert!(!node.to_string().contains("dev/orange"), "carved faces kept the wall's material: {node}");
    }

    let flat = ed.call("create_mesh", json!({ "shape": "cylinder", "min": [0, 0, 300], "max": [16, 64, 316], "rotate": [0, 0, 90] }));
    ed.call("select", json!({ "ids": [flat["id"]] }));
    let (min, max) = ed.selection_bounds();
    assert!((max[0] - min[0] - 64.0).abs() < 1e-3 && max[1] - min[1] < 17.0, "the cylinder lies along X: {min:?} {max:?}");

    let ground = ed.box_brush([-512.0, -16.0, 600.0], [512.0, 0.0, 1600.0]);
    ed.box_brush([-512.0, 200.0, 600.0], [0.0, 204.0, 1600.0]);
    ed.call("select", json!({ "ids": [ground] }));
    ed.call("scatter", json!({ "op": "new_set", "name": "weeds", "items": ["res://weed.glb"], "kind": "foliage", "targets": [ground] }));
    let filled = ed.call("scatter", json!({ "op": "fill", "density": 3, "exposed_only": true, "seed": 3 }));
    assert!(filled["placed"].as_u64().unwrap() > 0, "{filled}");
    assert!(filled["set"]["bounds"]["min"][0].as_f64().unwrap() > -64.0, "no weed under the slab over negative X: {filled}");

    let info = ed.call("texture", json!({ "op": "material_info", "material": "night/apartment_lit" }));
    let file = info["material_file"].as_str().expect("the material has a file");
    assert!(!file.contains('\\') && file.ends_with("night/apartment_lit.tres"), "{file}");
    assert!(!ed.state()["game"]["project_root"].as_str().unwrap().contains('\\'));

    ed.call(
        "create_entity",
        json!({ "classname": "light", "origin": [0, 64, 0], "properties": { "targetname": "lamp", "start_on": "0", "fixture": "bulb" },
        "outputs": [{ "output": "switched", "target": "bulb", "input": "set_visible" }, { "output": "switched", "target": "bulb", "input": "set_visibel" }] }),
    );
    let bulb = ed.box_brush([0.0, 70.0, 0.0], [8.0, 78.0, 8.0]);
    ed.call("gameplay", json!({ "op": "brush_entity", "ids": [bulb], "classname": "func_illusionary", "properties": { "targetname": "bulb" } }));
    let issues = ed.call("validate_map", json!({}));
    let inputs: Vec<String> =
        issues["issues"].as_array().unwrap().iter().filter(|i| i["code"] == "io_unknown_input").map(|i| i["message"].to_string()).collect();
    assert_eq!(inputs.len(), 1, "{inputs:?}");
    assert!(inputs[0].contains("set_visibel"));
    ed.call("set_editor", json!({ "shade": "lit" }));
    ed.screenshot("3d", "fixture_hidden_while_off");
}

/// An agent edits a saved map: every call and every script is one labelled undo step, and the person can read what
/// changed and take the whole batch back.
#[test]
#[ignore]
fn agent_edits_are_one_undo_step_and_can_be_reviewed() {
    let ed = Editor::launch("review");
    let floor = ed.box_brush([0.0, 0.0, 0.0], [256.0, 16.0, 256.0]);
    assert_eq!(ed.state()["undo"][0], "MCP: Create Brush");
    let path = artifacts().join("review.tdm");
    ed.call("map_file", json!({ "op": "save", "path": path }));
    let undo_before = ed.state()["undo"].as_array().unwrap().len();

    let run = ed.call(
        "run_script",
        json!({ "label": "Add crates", "steps": [
            { "tool": "create_brush", "args": { "min": [0, 16, 0], "max": [32, 48, 32], "material": "crate" } },
            { "tool": "create_brush", "args": { "min": [64, 16, 0], "max": [96, 48, 32], "material": "crate" } },
            { "tool": "create_entity", "args": { "classname": "light", "origin": [128, 64, 128], "properties": { "energy": "2" } } },
            { "tool": "select", "args": { "ids": [floor] } },
            { "tool": "transform", "args": { "translate": [0, -8, 0] } }
        ] }),
    );
    assert_eq!(run["undo"], "MCP: Add crates, 5 steps");
    let state = ed.state();
    assert_eq!(state["undo"][0], "MCP: Add crates, 5 steps");
    assert_eq!(state["undo"].as_array().unwrap().len(), undo_before + 1);

    let changes = ed.call("changes_since", json!({}));
    assert_eq!(changes["summary"], "3 added, 0 removed, 1 changed", "{changes}");
    assert_eq!(changes["since"], "the last save");
    assert_eq!(changes["changed"][0]["id"].as_u64(), Some(floor));
    assert_eq!(changes["changed"][0]["changes"], json!(["moved"]));
    assert_eq!(changes["changed"][0]["offset"], json!([0.0, -8.0, 0.0]));
    assert_eq!(ed.call("changes_since", json!({ "undo_steps": 1 }))["summary"], changes["summary"]);
    let from_file = ed.call("changes_since", json!({ "file": path }));
    assert_eq!(from_file["summary"], changes["summary"], "{from_file}");
    assert!(ed.call_err("changes_since", json!({ "undo_steps": 99 })).contains("only"));

    let summary = ed.call("summarize_map", json!({}));
    assert_eq!(summary["counts"]["brushes"], 3);
    assert_eq!(summary["entities"]["light"], 1);
    assert_eq!(summary["materials"]["crate"], 12);
    let lights = ed.call("list_nodes", json!({ "property": { "energy": "2" } }));
    assert_eq!(lights["total"], 1);
    assert_eq!(lights["nodes"][0]["properties"]["energy"], "2");
    let near = ed.call("list_nodes", json!({ "type": "brush", "box": { "min": [60, 20, 0], "max": [70, 30, 10] } }));
    assert_eq!(near["total"], 1);
    assert_eq!(ed.call("list_nodes", json!({ "material": "crate", "layer": "Default" }))["total"], 2);
    let compact = ed.call("get_node", json!({ "id": floor, "compact": true }));
    assert!(compact.get("vertices").is_none() && compact["faces"] == 6, "{compact}");

    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.brushes(), 1);
    assert_eq!(ed.call("changes_since", json!({}))["summary"], "0 added, 0 removed, 0 changed");
    assert!(!ed.state()["map"]["modified"].as_bool().unwrap());
    ed.call("run_action", json!({ "action": "redo" }));
    assert_eq!(ed.brushes(), 3);
    ed.call("run_action", json!({ "action": "undo", "args": { "steps": 2 } }));
    assert_eq!(ed.brushes(), 0);
}

/// A 4 x 4 BGRA8888 VTF 7.2 without mipmaps or thumbnail.
fn tiny_vtf(bgra: [u8; 4]) -> Vec<u8> {
    let mut b = vec![0u8; 80];
    b[0..4].copy_from_slice(b"VTF\0");
    b[4..8].copy_from_slice(&7u32.to_le_bytes());
    b[8..12].copy_from_slice(&2u32.to_le_bytes());
    b[12..16].copy_from_slice(&80u32.to_le_bytes());
    b[16..18].copy_from_slice(&4u16.to_le_bytes());
    b[18..20].copy_from_slice(&4u16.to_le_bytes());
    b[24..26].copy_from_slice(&1u16.to_le_bytes());
    b[52..56].copy_from_slice(&12i32.to_le_bytes());
    b[56] = 1;
    b[57..61].copy_from_slice(&(-1i32).to_le_bytes());
    b[63..65].copy_from_slice(&1u16.to_le_bytes());
    b.extend(bgra.repeat(16));
    b
}

/// A Quake WAD2 holding one 8 x 8 texture.
fn tiny_wad(name: &str) -> Vec<u8> {
    let mut b = b"WAD2".to_vec();
    b.extend(1u32.to_le_bytes());
    b.extend((12u32 + 40 + 85).to_le_bytes());
    let mut header = [0u8; 40];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[16..20].copy_from_slice(&8u32.to_le_bytes());
    header[20..24].copy_from_slice(&8u32.to_le_bytes());
    for (k, off) in [40u32, 104, 120, 124].iter().enumerate() {
        header[24 + k * 4..28 + k * 4].copy_from_slice(&off.to_le_bytes());
    }

    b.extend(header);
    b.extend([7u8; 85]);
    let mut entry = [0u8; 32];
    entry[0..4].copy_from_slice(&12u32.to_le_bytes());
    entry[12] = 0x44;
    entry[16..16 + name.len()].copy_from_slice(name.as_bytes());
    b.extend(entry);
    b
}

fn write_material(textures: &std::path::Path, name: &str, size: [u32; 2]) {
    let file = textures.join(name);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    image::RgbaImage::from_pixel(64, 32, image::Rgba([200, 40, 40, 255])).save(file.with_extension("png")).unwrap();
    std::fs::write(
        file.with_extension("tres"),
        format!(
            "[gd_resource type=\"StandardMaterial3D\" format=3]
[ext_resource type=\"Texture2D\" path=\"res://textures/{name}.png\" id=\"1\"]
[resource]
albedo_texture = ExtResource(\"1\")
metadata/texture_size = Vector2({}, {})
",
            size[0], size[1]
        ),
    )
    .unwrap();
}

/// Materials written into the project while the editor runs are found without a restart: validate_map does not
/// report them missing and justify fit uses their texture_size.
#[test]
#[ignore]
fn materials_added_while_running_are_found() {
    let dir = artifacts().join("late_materials");
    let _ = std::fs::remove_dir_all(&dir);
    let textures = dir.join("textures");
    std::fs::create_dir_all(&textures).unwrap();
    std::fs::write(
        dir.join("project.godot"),
        "config_version=5
",
    )
    .unwrap();
    let ed = Editor::launch_with("late_materials", &["--project", dir.to_str().unwrap()]);
    let codes = |ed: &Editor, code: &str| -> Vec<Value> {
        ed.call("validate_map", json!({}))["issues"].as_array().unwrap().iter().filter(|i| i["code"] == code).cloned().collect()
    };

    write_material(&textures, "megaplex/word", [80, 48]);
    let decal = ed.call("create_brush", json!({ "min": [0, 0, 0], "max": [80, 48, 4], "material": "megaplex/word" }))["ids"][0].as_u64().unwrap();
    assert_eq!(codes(&ed, "missing_material"), Vec::<Value>::new(), "the new material is found");
    let faces = ed.call("texture", json!({ "op": "get", "faces": (0..6).map(|f| json!([decal, f])).collect::<Vec<_>>() }));
    let front = faces["faces"].as_array().unwrap().iter().find(|f| f["normal"][2].as_f64().unwrap() > 0.99).unwrap()["face"][1].as_u64().unwrap();
    ed.call("texture", json!({ "op": "justify", "mode": "fit", "faces": [[decal, front]] }));
    let uv = ed.call("texture", json!({ "op": "get", "faces": [[decal, front]] }))["faces"][0]["uv"].clone();
    let corners: Vec<[f64; 2]> = [[0.0, 0.0, 4.0], [80.0, 48.0, 4.0]].iter().map(|p| texel(&uv, *p)).collect();
    let span = [(corners[1][0] - corners[0][0]).abs(), (corners[1][1] - corners[0][1]).abs()];
    assert!(approx(&span, &[80.0, 48.0]), "fit covers one 80 by 48 repeat, not the image's pixel size: {span:?}");

    let before = ed.state()["game"]["materials"].as_u64().unwrap_or_default();
    write_material(&textures, "megaplex/later", [64, 64]);
    let reloaded = ed.call("run_action", json!({ "action": "reload_materials" }));
    assert!(reloaded["materials"].as_u64().unwrap() > before, "{reloaded}");
    ed.call("create_brush", json!({ "min": [0, 64, 0], "max": [64, 128, 16], "material": "megaplex/later" }));
    assert_eq!(codes(&ed, "missing_material"), Vec::<Value>::new());
}

/// A game config exported from Godot while the editor runs is used by the next MCP call, without open_project.
#[test]
#[ignore]
fn game_config_changes_are_picked_up() {
    let dir = artifacts().join("game_config_watch");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.godot"), "config_version=5\n").unwrap();
    let export = |classes: &[&str]| {
        let entities: Vec<Value> = classes.iter().map(|c| json!({ "classname": c, "type": "point" })).collect();
        std::fs::write(dir.join("trenchdot_game.json"), json!({ "format": "trenchdot-game", "entities": entities }).to_string()).unwrap();
    };
    export(&["game_locker"]);
    let ed = Editor::launch_with("game_config_watch", &["--project", dir.to_str().unwrap()]);
    assert!(ed.call_err("get_game_config", json!({ "classname": "clue_spot" })).contains("no definition"));

    export(&["game_locker", "clue_spot"]);
    assert_eq!(ed.call("get_game_config", json!({ "classname": "clue_spot" }))["classname"], "clue_spot");
    let reloaded = ed.call("run_action", json!({ "action": "reload_game_config" }));
    assert_eq!(reloaded["entities"], 2, "{reloaded}");
}

/// validate_map names a prop whose model file does not exist, gives every issue its bounds, and checks saved maps
/// one by one or all at once without opening them.
#[test]
#[ignore]
fn validate_map_reports_missing_models_across_the_project() {
    let dir = artifacts().join("validate_project");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("props")).unwrap();
    std::fs::create_dir_all(dir.join("maps")).unwrap();
    std::fs::write(dir.join("project.godot"), "config_version=5\n").unwrap();
    std::fs::write(dir.join("props/chair.gltf"), "{}").unwrap();
    let ed = Editor::launch_with("validate_project", &["--project", dir.to_str().unwrap()]);
    let missing = |result: &Value| -> Vec<Value> { result["issues"].as_array().unwrap().iter().filter(|i| i["code"] == "missing_model").cloned().collect() };

    let chair =
        ed.call("create_entity", json!({ "classname": "prop_model", "origin": [64, 0, 32], "properties": { "model": "res://props/chair.glb" } }))["id"].clone();
    let found = missing(&ed.call("validate_map", json!({})));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["node"], chair);
    assert!(found[0]["message"].as_str().unwrap().contains("res://props/chair.gltf does"), "{}", found[0]["message"]);
    let bounds = &found[0]["bounds"];
    assert!(bounds["min"][0].as_f64().unwrap() <= 64.0 && bounds["max"][0].as_f64().unwrap() >= 64.0, "{bounds}");

    let saved = dir.join("maps/block_a.tdm");
    ed.call("map_file", json!({ "op": "save", "path": saved }));
    ed.call("map_file", json!({ "op": "new" }));
    assert_eq!(missing(&ed.call("validate_map", json!({}))).len(), 0, "the new map is clean");
    assert_eq!(missing(&ed.call("validate_map", json!({ "path": "res://maps/block_a.tdm" }))).len(), 1, "a saved map is checked without opening it");
    let project = ed.call("validate_map", json!({ "project": true }));
    let maps = project["maps"].as_array().unwrap();
    assert_eq!(maps.len(), 1, "{project}");
    assert!(maps[0]["path"].as_str().unwrap().ends_with("maps/block_a.tdm"));
    assert_eq!(missing(&maps[0]).len(), 1);
    assert!(ed.call_err("validate_map", json!({ "path": "maps/nope.tdm" })).contains("nope.tdm"));
}

/// validate_map reports faces of two brushes that overlap on one plane with different textures, which z-fight once
/// built, and leaves out the same texture lined up.
#[test]
#[ignore]
fn validate_map_reports_coplanar_faces() {
    let ed = Editor::launch("coplanar");
    let fights = |ed: &Editor| -> Vec<Value> {
        ed.call("validate_map", json!({}))["issues"].as_array().unwrap().iter().filter(|i| i["code"] == "coplanar_faces").cloned().collect()
    };
    ed.call("create_brush", json!({ "min": [0, 0, 0], "max": [64, 64, 16], "material": "dev/grey" }));
    let aligned = ed.call("create_brush", json!({ "min": [32, 16, 0], "max": [96, 48, 16], "material": "dev/grey" }))["ids"][0].as_u64().unwrap();
    assert_eq!(fights(&ed), Vec::<Value>::new(), "one texture lined up draws the same pixels");

    ed.call("texture", json!({ "op": "apply", "material": "dev/orange", "faces": (0..6).map(|f| json!([aligned, f])).collect::<Vec<_>>() }));
    let found = fights(&ed);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["node"].as_u64(), Some(aligned));
    assert_eq!(found[0]["severity"], "warning");
    assert!(found[0]["message"].as_str().unwrap().contains("as do 1 more"), "{}", found[0]["message"]);
}

/// A Hammer map and a TrenchBroom map, imported as a user would, find their textures next to them and bring them into
/// the project, after which no face is missing its material.
#[test]
#[ignore]
fn imports_convert_valve_and_quake_textures() {
    let dir = artifacts().join("import_textures");
    let _ = std::fs::remove_dir_all(&dir);
    let project = dir.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("project.godot"), "config_version=5\n").unwrap();
    let game = dir.join("mod");
    std::fs::create_dir_all(game.join("materials/brick")).unwrap();
    std::fs::create_dir_all(game.join("maps")).unwrap();
    std::fs::write(game.join("materials/brick/wall.vtf"), tiny_vtf([40, 80, 160, 255])).unwrap();
    std::fs::write(game.join("materials/brick/wall.vmt"), "LightmappedGeneric { $basetexture brick/wall $surfaceprop brick }").unwrap();
    let side = |plane: &str, material: &str| {
        format!("side {{ \"plane\" \"{plane}\" \"material\" \"{material}\" \"uaxis\" \"[1 0 0 0] 0.25\" \"vaxis\" \"[0 -1 0 0] 0.25\" }}")
    };
    let solid = [
        side("(-64 64 64) (64 64 64) (64 -64 64)", "BRICK/WALL"),
        side("(-64 -64 0) (64 -64 0) (64 64 0)", "TOOLS/TOOLSNODRAW"),
        side("(-64 64 64) (-64 -64 64) (-64 -64 0)", "BRICK/WALL"),
        side("(64 64 0) (64 -64 0) (64 -64 64)", "BRICK/WALL"),
        side("(64 64 64) (-64 64 64) (-64 64 0)", "BRICK/WALL"),
        side("(64 -64 0) (-64 -64 0) (-64 -64 64)", "BRICK/WALL"),
    ]
    .join("\n");
    let vmf = game.join("maps/room.vmf");
    std::fs::write(&vmf, format!("world {{ \"classname\" \"worldspawn\" solid {{ {solid} }} }}")).unwrap();

    let ed = Editor::launch_with("import_textures", &["--project", project.to_str().unwrap()]);
    let offered = ed.call("map_file", json!({ "op": "import_vmf", "path": vmf }));
    assert_eq!((offered["missing_materials"].as_u64(), offered["convertible"].as_u64()), (Some(1), Some(1)), "{offered}");
    let missing = |ed: &Editor| {
        let issues = ed.call("validate_map", json!({}));
        issues["issues"].as_array().unwrap().iter().filter(|i| i["code"] == "missing_material").count()
    };
    assert_eq!(missing(&ed), 1, "brick/wall is listed until it is converted");

    let imported = ed.call("map_file", json!({ "op": "import_vmf", "path": vmf, "textures": "auto" }));
    assert_eq!(imported["textures"]["converted"], 1, "{imported}");
    assert!(project.join("textures/brick/wall.png").is_file() && project.join("textures/brick/wall.tres").is_file());
    assert_eq!(missing(&ed), 0);

    let quake = dir.join("quake");
    std::fs::create_dir_all(quake.join("wads")).unwrap();
    std::fs::write(quake.join("wads/base.wad"), tiny_wad("wall1")).unwrap();
    std::fs::write(quake.join("sky.wad"), tiny_wad("sky4")).unwrap();
    let map = quake.join("room.map");
    let brush = "( -64 -64 -16 ) ( -64 -63 -16 ) ( -64 -64 -15 ) WALL1 0 0 0 1 1\n\
( -64 -64 -16 ) ( -64 -64 -15 ) ( -63 -64 -16 ) wall1 0 0 0 1 1\n\
( -64 -64 -16 ) ( -63 -64 -16 ) ( -64 -63 -16 ) skip 0 0 0 1 1\n\
( 64 64 16 ) ( 64 65 16 ) ( 65 64 16 ) sky4 0 0 0 1 1\n\
( 64 64 16 ) ( 65 64 16 ) ( 64 64 17 ) wall1 0 0 0 1 1\n\
( 64 64 16 ) ( 64 64 17 ) ( 64 65 16 ) wall1 0 0 0 1 1\n";
    std::fs::write(&map, format!("{{\n\"classname\" \"worldspawn\"\n\"wad\" \"/somewhere/else/base.wad;sky.wad\"\n{{\n{brush}}}\n}}\n")).unwrap();
    let imported = ed.call("map_file", json!({ "op": "import_map", "path": map, "textures": "auto" }));
    assert_eq!(imported["textures"]["converted"], 1, "the WAD is found by name one folder down: {imported}");
    assert!(project.join("textures/wall1.png").is_file());
    assert_eq!(missing(&ed), 0, "WALL1 finds wall1 and skip is the project's skip texture");
    let brush = ed.call("list_nodes", json!({ "type": "brush" }))["nodes"][0]["id"].clone();
    let faces = ed.call("get_node", json!({ "id": brush })).to_string();
    assert!(!faces.contains("WALL1") && faces.contains("\"wall1\"") && faces.contains("special/skip"), "{faces}");
    assert!(faces.contains("special/sky") && !faces.contains("sky4"), "{faces}");
    let sky = &imported["textures"]["sky"];
    assert!(sky["sky_top_color"].is_string() && sky["sky_horizon_color"].is_string(), "sky colors from sky4: {imported}");

    let again = ed.call("map_file", json!({ "op": "convert_textures", "path": game }));
    assert_eq!((again["found"].as_u64(), again["textures"]["existing"].as_u64()), (Some(1), Some(1)), "{again}");
    ed.call("set_editor", json!({ "shade": "textured" }));
    let (_, _, colors) = ed.screenshot("3d", "quake_room");
    assert!(colors > 2);
}

type Sent = std::sync::Arc<std::sync::Mutex<Vec<Value>>>;

fn fake_godot_editor(project: &std::path::Path, map: &std::path::Path) -> (u16, Sent) {
    fake_godot_editor_with(project, &[map], json!({}))
}

fn fake_godot_editor_with(project: &std::path::Path, maps: &[&std::path::Path], answers: Value) -> (u16, Sent) {
    fake_godot(project, maps, answers, true)
}

/// Answers the live link like a Godot editor with `maps` open: status lists them, a capture returns a small red PNG and
/// a walkable request a floor with a small island up on a wall. Any other event in `answers` gets `{"ok": true}` with
/// the fields given for it, the rest are unknown events as an older addon answers them. Every message but status is
/// kept. Like the addon it serves several connections at once. Captures go unanswered when `captures` is false.
fn fake_godot(project: &std::path::Path, maps: &[&std::path::Path], answers: Value, captures: bool) -> (u16, Sent) {
    use std::io::BufRead;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let slash = |p: &std::path::Path| p.to_string_lossy().replace('\\', "/");
    let project = slash(project);
    let listed: Vec<Value> = maps.iter().map(|m| json!({ "path": slash(m), "epoch": 1 })).collect();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(8, 4, image::Rgba([220, 30, 30, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let png = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let (project, listed, log, png, answers) = (project.clone(), listed.clone(), log.clone(), png.clone(), answers.clone());
            std::thread::spawn(move || {
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    let msg: Value = serde_json::from_str(&line).unwrap();
                    line.clear();
                    let event = msg["event"].as_str().unwrap_or_default();
                    if event != "status" {
                        log.lock().unwrap().push(msg.clone());
                    }

                    let mut reply = match event {
                        "status" => json!({ "ok": true, "project": project, "godot": "4.7", "pid": 1, "maps": listed.clone() }),
                        "capture" if !captures => continue,
                        "capture" => json!({ "ok": true, "png": png, "width": 8, "height": 4, "warnings": ["warning: prop model res://gone.glb not found"] }),
                        "walkable" => {
                            json!({
                                "ok": true, "units_per_meter": 32.0, "agent": msg["agent"], "msec": 5,
                                "vertices": [8, 16.5, 8, 8, 16.5, 248, 248, 16.5, 248, 248, 16.5, 8, 300, 128, 0, 300, 128, 32, 332, 128, 32, 332, 128, 0],
                                "polygons": [[0, 1, 2, 3], [4, 5, 6, 7]],
                                "islands": [[0], [1]]
                            })
                        }
                        other => match answers[other].as_object() {
                            Some(fields) => {
                                let mut reply = json!({ "ok": true });
                                reply.as_object_mut().unwrap().extend(fields.clone());
                                reply
                            }
                            None => json!({ "ok": false, "error": "unknown event" }),
                        },
                    };
                    reply["seq"] = msg["seq"].clone();
                    if writeln!(&stream, "{reply}").is_err() {
                        break;
                    }
                }
            });
        }
    });
    (port, seen)
}

/// The messages of one event the fake Godot editor got.
fn sent_events(sent: &Sent, event: &str) -> Vec<Value> {
    sent.lock().unwrap().iter().filter(|m| m["event"] == event).cloned().collect()
}

/// Waits until `what` holds for the editor's state.
fn wait_state(ed: &Editor, label: &str, what: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let state = ed.state();
        if what(&state) {
            return state;
        }

        assert!(Instant::now() < deadline, "timed out waiting for {label}: {state}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A project folder for a test against the fake Godot editor, with a game config that defines `entities`.
fn linked_project(name: &str, entities: Value) -> PathBuf {
    let dir = artifacts().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("maps")).unwrap();
    std::fs::write(dir.join("project.godot"), "config_version=5\n").unwrap();
    let config = json!({ "format": "trenchdot-game", "version": 1, "entities": entities });
    std::fs::write(dir.join("trenchdot_game.json"), config.to_string()).unwrap();
    // The editor compares project paths with Godot's, which have no `..` and no verbatim prefix.
    PathBuf::from(dir.canonicalize().unwrap().to_string_lossy().trim_start_matches(r"\\?\").to_string())
}

/// Editors under test get ports of their own, so their saves never reach a Godot editor or game the person running the
/// tests has open on the default ports.
#[test]
#[ignore]
fn saves_stay_off_the_default_godot_ports() {
    use std::io::BufRead;
    let dir = linked_project("default_godot_ports", json!([]));
    let map = dir.join("maps/hall.tdm");
    let sent: Sent = Default::default();
    // A port a running Godot already holds cannot be listened on, and that Godot then shows nothing here either.
    let mut listened = 0;
    for port in [7842, 7843] {
        let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", port)) else { continue };
        listened += 1;
        let log = sent.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                if std::io::BufReader::new(stream).read_line(&mut line).is_ok()
                    && let Ok(message) = serde_json::from_str::<Value>(&line)
                {
                    log.lock().unwrap().push(message);
                }
            }
        });
    }

    if listened == 0 {
        eprintln!("skipped: both default Godot ports are in use, nothing here could see a save reach them");
        return;
    }

    let ed = Editor::launch_with("default_godot_ports", &["--project", dir.to_str().unwrap()]);
    ed.box_brush([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]);
    ed.call("map_file", json!({ "op": "save", "path": map }));
    wait_state(&ed, "the save to reach the live link", |s| s["status"].as_str().is_some_and(|t| t.starts_with("Live link:")));
    let godot_path = map.to_string_lossy().replace('\\', "/");
    // Editors of other checkouts may still talk to the default ports, only this map counts.
    let ours: Vec<Value> = sent.lock().unwrap().iter().filter(|m| m["path"] == godot_path.as_str()).cloned().collect();
    assert!(ours.is_empty(), "the save reached a default port: {ours:?}");
}

/// Godot names the entity classes it built without a definition and the map settings it used, and the editor shows them
/// in the status bar and the Issues panel.
#[test]
#[ignore]
fn godot_names_the_classes_it_built_without_a_definition() {
    let dir = linked_project("godot_missing_classes", json!([{ "classname": "func_door", "type": "solid" }]));
    let map = dir.join("maps/hall.tdm");
    let warning = "func_crusher: Node Class AnimatedBody3D not found, building it as a Node3D";
    let answers = json!({ "map_saved": { "rebuilt": 1, "missing_classes": ["func_door"], "settings": ["res://my_settings.tres"], "warnings": [warning] } });
    let (godot, sent) = fake_godot_editor_with(&dir, &[&map], answers);
    let ed = Editor::launch_with("godot_missing", &["--project", dir.to_str().unwrap()]);
    ed.call("set_editor", json!({ "live_link_port": godot }));
    wait_state(&ed, "the live link", |s| s["godot"]["project_open"] == true);

    let leaf = ed.box_brush([0.0, 0.0, 0.0], [8.0, 96.0, 64.0]);
    ed.call("gameplay", json!({ "op": "brush_entity", "ids": [leaf], "classname": "func_door" }));
    ed.call("map_file", json!({ "op": "save", "path": map }));
    let state = wait_state(&ed, "the save reply", |s| s["status"].as_str().is_some_and(|t| t.contains("no definition")));
    assert_eq!(
        state["status"],
        "Live link: Godot rebuilt 1 map node(s), but res://my_settings.tres has no definition for func_door, so it builds as a plain node. See the Issues panel"
    );
    assert_eq!(state["godot"]["missing_classes"], json!(["func_door"]));
    assert_eq!(sent_events(&sent, "map_saved").len(), 1);
    let issues = ed.call("validate_map", json!({}));
    let godot_issues: Vec<&Value> = issues["issues"].as_array().unwrap().iter().filter(|i| i["code"].as_str().unwrap().starts_with("godot_")).collect();
    assert_eq!(godot_issues.len(), 2, "{issues}");
    assert!(
        godot_issues[0]["message"].as_str().unwrap().contains("Godot built func_door as a plain node")
            && godot_issues[0]["message"].as_str().unwrap().contains("res://my_settings.tres")
    );
    assert_eq!(godot_issues[1]["message"], format!("Godot: {warning}"));
}

/// With the editor in German the live link status and the Godot issues still read in English over MCP, and the shown
/// words are German once the language is set.
#[test]
#[ignore]
fn godot_statuses_stay_english_for_mcp_in_german() {
    let dir = linked_project("godot_missing_de", json!([{ "classname": "func_door", "type": "solid" }]));
    let map = dir.join("maps/hall.tdm");
    let answers = json!({ "map_saved": { "rebuilt": 1, "missing_classes": ["func_door"], "settings": ["res://a.tres", "res://b.tres"], "warnings": [] } });
    let (godot, _sent) = fake_godot_editor_with(&dir, &[&map], answers);
    let ed = Editor::launch_with("godot_missing_de", &["--project", dir.to_str().unwrap()]);
    ed.call("set_editor", json!({ "live_link_port": godot, "language": "de" }));
    wait_state(&ed, "the live link", |s| s["godot"]["project_open"] == true);

    let leaf = ed.box_brush([0.0, 0.0, 0.0], [8.0, 96.0, 64.0]);
    ed.call("gameplay", json!({ "op": "brush_entity", "ids": [leaf], "classname": "func_door" }));
    ed.call("map_file", json!({ "op": "save", "path": map }));
    let state = wait_state(&ed, "the save reply", |s| s["status"].as_str().is_some_and(|t| t.contains("no definition")));
    assert_eq!(
        state["status"],
        "Live link: Godot rebuilt 1 map node(s), but res://a.tres and res://b.tres have no definition for func_door, so it builds as a plain node. See the Issues panel"
    );
    let issues = ed.call("validate_map", json!({}));
    assert!(
        issues["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["message"].as_str().unwrap().contains("there is no definition for it in res://a.tres, res://b.tres")),
        "{issues}"
    );
}

/// In live mode Godot names the classes an edit built without a definition, and the status bar says so.
#[test]
#[ignore]
fn live_mode_tells_about_classes_godot_lacks() {
    let dir = linked_project("godot_unresolved", json!([{ "classname": "func_door", "type": "solid" }]));
    let map = dir.join("maps/hall.tdm");
    let answers = json!({
        "map_saved": { "rebuilt": 1, "settings": ["res://my_settings.tres"] },
        "live_begin": { "epoch": 1 },
        "live_delta": { "epoch": 1, "unresolved": ["func_door"], "settings": ["res://live_settings.tres"] },
    });
    let (godot, sent) = fake_godot_editor_with(&dir, &[&map], answers);
    let ed = Editor::launch_with("godot_unresolved", &["--project", dir.to_str().unwrap()]);
    ed.call("set_editor", json!({ "live_link_port": godot, "live_mode": true }));
    wait_state(&ed, "the live link", |s| s["godot"]["project_open"] == true);
    let leaf = ed.box_brush([0.0, 0.0, 0.0], [8.0, 96.0, 64.0]);
    ed.call("gameplay", json!({ "op": "brush_entity", "ids": [leaf], "classname": "func_door" }));
    ed.call("map_file", json!({ "op": "save", "path": map }));
    wait_state(&ed, "the live session", |s| s["godot"]["live_session"] == true);
    assert_eq!(ed.state()["godot"]["missing_classes"], json!([]), "the save built every class");

    ed.box_brush([64.0, 0.0, 0.0], [128.0, 16.0, 64.0]);
    let state = wait_state(&ed, "the live reply", |s| s["status"].as_str().is_some_and(|t| t.starts_with("Godot has no definition")));
    assert_eq!(
        state["status"],
        "Godot has no definition for func_door, it shows a plain node. Install the Gameplay entities pack or add the class to the FGD of res://live_settings.tres"
    );
    assert_eq!(state["godot"]["missing_classes"], json!(["func_door"]));
    assert!(!sent_events(&sent, "live_delta").is_empty());
    let issues = ed.call("validate_map", json!({}));
    assert!(issues["issues"].as_array().unwrap().iter().any(|i| i["code"] == "godot_missing_class"), "{issues}");
}

/// Puts the Gameplay entities pack template into the project at `dir`, where an addon that has the pack carries it.
fn add_pack_template(dir: &std::path::Path) {
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    copy(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../godot/addons/func_godot/gameplay_pack"), &dir.join("addons/func_godot/gameplay_pack"));
}

/// The door wizard installs the Gameplay entities pack into a project that lacks it, so the door it makes has its
/// definition, and has the connected Godot scan for the new files.
#[test]
#[ignore]
fn the_door_wizard_installs_the_gameplay_pack_first() {
    let dir = linked_project("door_wizard_pack", json!([]));
    add_pack_template(&dir);
    let (godot, sent) = fake_godot_editor_with(&dir, &[&dir.join("maps/hall.tdm")], json!({ "scan": {} }));
    let ed = Editor::launch_with("door_wizard_pack", &["--project", dir.to_str().unwrap()]);
    ed.call("set_editor", json!({ "live_link_port": godot }));
    wait_state(&ed, "the live link", |s| s["godot"]["project_open"] == true);

    let leaf = ed.box_brush([0.0, 0.0, -4.0], [48.0, 96.0, 4.0]);
    let door = ed.call("gameplay", json!({ "op": "make_door", "ids": [leaf], "trigger": true }));
    assert_eq!(door["classname"], "func_door_rotating");
    let summary = door["gameplay_pack"].as_str().unwrap_or_else(|| panic!("the pack was installed: {door}"));
    assert!(summary.starts_with("Added ") && summary.ends_with(td_editor::entity_pack::SCAN_NOTE), "{summary}");
    assert!(dir.join("trenchdot/entities/pack.json").is_file() && dir.join("trenchdot/entities/scripts/td_door.gd").is_file());
    let issues = ed.call("validate_map", json!({}));
    let unknown: Vec<&Value> = issues["issues"].as_array().unwrap().iter().filter(|i| i["code"] == "unknown_class").collect();
    assert!(unknown.is_empty(), "{unknown:?}");
    assert_eq!(sent_events(&sent, "scan").len(), 1, "Godot scans for the new files");

    let lift = ed.box_brush([200.0, 0.0, 0.0], [264.0, 8.0, 64.0]);
    let again = ed.call("gameplay", json!({ "op": "make_platform", "ids": [lift] }));
    assert!(again["gameplay_pack"].is_null(), "installed once: {again}");
}

/// Saving a prefab has the connected Godot build the open maps that instance it, from the map as shown in the editor.
#[test]
#[ignore]
fn saving_a_prefab_rebuilds_the_maps_that_use_it() {
    let dir = linked_project("godot_prefab_users", json!([]));
    let (prefab, level) = (dir.join("maps/crate.tdm"), dir.join("maps/level.tdm"));
    let answers = json!({ "map_saved": { "rebuilt": 0 }, "build": { "rebuilt": 1 } });
    let (godot, sent) = fake_godot_editor_with(&dir, &[&level], answers);
    let ed = Editor::launch_with("godot_prefab_users", &["--project", dir.to_str().unwrap()]);
    ed.call("set_editor", json!({ "live_link_port": godot }));
    wait_state(&ed, "the live link", |s| s["godot"]["project_open"] == true);

    ed.box_brush([0.0, 0.0, 0.0], [32.0, 32.0, 32.0]);
    ed.call("map_file", json!({ "op": "save", "path": prefab }));
    ed.call("run_action", json!({ "action": "new_tab" }));
    ed.call("run_action", json!({ "action": "insert_prefab", "args": { "path": prefab, "origin": [128, 0, 0] } }));
    ed.call("map_file", json!({ "op": "save", "path": level }));
    assert!(sent_events(&sent, "build").is_empty(), "nothing instances the level");

    ed.call("map_file", json!({ "op": "open_tab", "path": prefab }));
    ed.box_brush([0.0, 32.0, 0.0], [16.0, 48.0, 16.0]);
    ed.call("run_action", json!({ "action": "save" }));
    let state = wait_state(&ed, "the build", |s| s["status"].as_str().is_some_and(|t| t.contains("which uses")));
    assert_eq!(state["status"], "Live link: Godot rebuilt level.tdm, which uses crate.tdm");
    let builds = sent_events(&sent, "build");
    assert_eq!(builds.len(), 1, "{builds:?}");
    assert!(builds[0]["path"].as_str().unwrap().ends_with("maps/level.tdm"), "{}", builds[0]["path"]);
    let text = builds[0]["text"].as_str().unwrap();
    assert!(text.contains("trenchdot-map") && text.contains("crate.tdm"), "the level as shown, with its instance: {text}");
}

/// Run Project has the connected Godot editor save its scenes and play, so the game has what Godot built. With an addon
/// that has no play event the editor starts the game itself and says the game runs the scene saved in Godot.
#[test]
#[ignore]
fn run_project_plays_in_the_connected_godot() {
    let dir = linked_project("godot_play", json!([]));
    let map = dir.join("maps/hall.tdm");
    let (godot, sent) = fake_godot_editor_with(&dir, &[&map], json!({ "play": {} }));
    let (old_godot, old_sent) = fake_godot_editor(&dir, &map);
    // Stands in for Godot when the editor starts the game itself, it exits right away.
    let stand_in = std::env::current_exe().unwrap();
    let ed = Editor::launch_env("godot_play", &["--project", dir.to_str().unwrap()], &[("GODOT", stand_in.to_str().unwrap())]);
    ed.call("set_editor", json!({ "live_link_port": godot }));
    wait_state(&ed, "the live link", |s| s["godot"]["project_open"] == true);
    ed.call("run_action", json!({ "action": "run_godot_project" }));
    let state = wait_state(&ed, "the play reply", |s| s["status"].as_str().is_some_and(|t| t.starts_with("Godot saved")));
    assert_eq!(state["status"], "Godot saved its open scenes and runs the main scene");
    assert_eq!(sent_events(&sent, "play").len(), 1);

    ed.call("set_editor", json!({ "live_link_port": old_godot }));
    wait_state(&ed, "the older addon", |s| s["godot"]["project_open"] == true);
    ed.call("run_action", json!({ "action": "run_godot_project" }));
    let state = wait_state(&ed, "the fallback", |s| s["status"].as_str().is_some_and(|t| t.starts_with("Launched")));
    let status = state["status"].as_str().unwrap();
    assert!(status.ends_with(td_editor::commands::UNSAVED_IN_GODOT), "{status}");
    assert_eq!(sent_events(&old_sent, "play").len(), 1, "asked first, then started the game itself");
}

/// `screenshot {source: godot}` asks the connected Godot editor for a picture of the map as shown in TrenchDot and
/// returns it with what Godot logged, and says what is missing when it cannot.
#[test]
#[ignore]
fn godot_captures_come_over_the_live_link() {
    let dir = artifacts().join("godot_capture");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("maps")).unwrap();
    std::fs::write(dir.join("project.godot"), "config_version=5\n").unwrap();
    let map = dir.join("maps/yard.tdm");
    let (godot, captures) = fake_godot_editor(&dir, &map);

    let ed = Editor::launch_with("godot_capture", &["--project", dir.to_str().unwrap()]);
    ed.call("set_editor", json!({ "live_link_port": godot }));
    ed.call("create_brush", json!({ "min": [0, 0, 0], "max": [256, 16, 256] }));
    assert!(ed.call_err("screenshot", json!({ "source": "godot" })).contains("save the map"));
    ed.call("open_project", json!({ "path": dir }));
    ed.call("map_file", json!({ "op": "save", "path": map }));
    let e = ed.call_err("screenshot", json!({ "source": "godot", "position": [0, 64, 0], "look_at": [0, 64, 0] }));
    assert!(e.contains("look_at"), "{e}");

    ed.call("set_camera", json!({ "view": "3d", "position": [0, 64, 512], "look_at": [0, 64, 0] }));
    let r = ed.try_rpc("tools/call", json!({ "name": "screenshot", "arguments": { "source": "godot", "width": 320, "height": 200 } })).unwrap();
    let content = &r["result"]["content"];
    let png = base64::engine::general_purpose::STANDARD.decode(content[0]["data"].as_str().unwrap_or_else(|| panic!("no image in {r}"))).unwrap();
    assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8().get_pixel(0, 0).0, [220, 30, 30, 255]);
    let note = content[1]["text"].as_str().unwrap();
    assert!(note.contains("yard.tdm") && note.contains("built from the map") && note.contains("res://gone.glb"), "{note}");

    let sent = sent_events(&captures, "capture");
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0]["width"].as_u64(), sent[0]["height"].as_u64()), (Some(320), Some(200)));
    let forward: Vec<f64> = sent[0]["camera"]["forward"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!(approx(&forward, &[0.0, 0.0, -1.0]), "the 3d view's camera looks down -Z: {forward:?}");
    assert!(sent[0]["text"].as_str().is_some_and(|t| t.contains("trenchdot-map")), "the map as shown goes along to build");

    ed.call("screenshot", json!({ "source": "godot", "build": false, "position": [512, 64, 0], "look_at": [0, 64, 0] }));
    let sent = sent_events(&captures, "capture");
    assert!(sent[1]["text"].is_null() && sent[1]["camera"]["position"] == json!([512.0, 64.0, 0.0]), "{}", sent[1]);

    ed.call("set_editor", json!({ "live_link_port": free_port() }));
    let e = ed.call_err("screenshot", json!({ "source": "godot" }));
    assert!(e.contains("not running"), "{e}");
}

/// The walkability tool asks the connected Godot editor for the walkable area, answers with the islands, draws them
/// over the views and bakes again after an edit. Without Godot it says so.
#[test]
#[ignore]
fn the_walkable_area_comes_from_godot() {
    let dir = artifacts().join("godot_walkable");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("maps")).unwrap();
    std::fs::write(
        dir.join("project.godot"),
        "config_version=5
",
    )
    .unwrap();
    let map = dir.join("maps/yard.tdm");
    let (godot, sent) = fake_godot_editor(&dir, &map);

    let ed = Editor::launch_with("walkable", &["--project", dir.to_str().unwrap()]);
    ed.call("set_editor", json!({ "live_link_port": godot }));
    ed.call("create_brush", json!({ "min": [0, 0, 0], "max": [256, 16, 256] }));
    assert!(ed.call_err("walkability", json!({})).contains("save the map"));
    ed.call("open_project", json!({ "path": dir }));
    ed.call("map_file", json!({ "op": "save", "path": map }));
    ed.call("run_action", json!({ "action": "select_none" }));
    ed.call("set_camera", json!({ "view": "3d", "position": [128, 400, 400], "look_at": [128, 16, 128] }));
    let args = json!({ "target": "3d", "width": 320, "height": 200, "overlays": true });
    let before = shot(&ed, args.clone(), "before");

    let r = ed.call("walkability", json!({ "radius": 0.5 }));
    assert_eq!(r["islands"], 2);
    assert_eq!(r["area_m2"], 57.3, "{r}");
    assert_eq!(r["main"]["bounds"]["min"], json!([8.0, 16.5, 8.0]));
    assert_eq!(r["cut_off"][0]["center"], json!([316.0, 128.0, 16.0]));
    let first = sent_events(&sent, "walkable")[0].clone();
    assert_eq!(first["agent"]["radius"], 0.5);
    assert!(first["text"].as_str().is_some_and(|t| t.contains("trenchdot-map")), "the map as shown goes along to build");

    let greenish = |p: &image::Rgba<u8>, by: i32| p.0[1] as i32 > p.0[0] as i32 + by;
    let shown = shot(&ed, args.clone(), "shown");
    assert!(greenish(shown.get_pixel(160, 110), 20), "the connected floor is tinted bluish green, got {:?}", shown.get_pixel(160, 110));
    assert!(image_difference(&shown, &before) > 1.0);
    let top = shot(&ed, json!({ "target": "top", "width": 320, "height": 200, "overlays": true }), "top");
    assert!(top.pixels().any(|p| greenish(p, 40)), "the top view shows the walkable area too");

    // On paper the fill flips with the outlines. 0.4 is walkable::FILL_ALPHA.
    ed.call("set_editor", json!({ "theme": "light" }));
    theme_active(&ed, "light");
    ed.call("set_camera", json!({ "view": "top", "center": [128, 0, 128], "zoom": 1.0 }));
    let paper = shot(&ed, json!({ "target": "top", "width": 320, "height": 200, "overlays": true }), "top_light");
    let light = td_editor::theme::LIGHT;
    let green = egui::Rgba::from(td_editor::theme::DARK.walkable());
    let fill = [green.r(), green.g(), green.b(), 0.4];
    let flipped = over(td_render::ink::flip_linear(fill, light.flip_pivot().unwrap()), light.view_2d, 1);
    let plain = over(fill, light.view_2d, 1);
    assert!(flipped[1] + 8 < plain[1], "flipped {flipped:?} reads darker than {plain:?}");
    for (x, y) in [(131, 97), (203, 150), (60, 40)] {
        let at = paper.get_pixel(x, y).0;
        assert!((0..3).all(|i| at[i].abs_diff(flipped[i]) <= 3), "fill pixel {x},{y} is {at:?}, flipped it is {flipped:?}");
    }

    ed.call("set_editor", json!({ "theme": "dark" }));
    theme_active(&ed, "dark");

    ed.box_brush([64.0, 16.0, 64.0], [96.0, 64.0, 96.0]);
    let start = Instant::now();
    while sent_events(&sent, "walkable").len() < 2 {
        assert!(start.elapsed() < Duration::from_secs(10), "an edit bakes the overlay again");
        std::thread::sleep(Duration::from_millis(100));
    }

    ed.call("run_action", json!({ "action": "toggle_walkable" }));
    ed.call("run_action", json!({ "action": "select_none" }));
    let hidden = shot(&ed, args, "hidden");
    assert!(!greenish(hidden.get_pixel(160, 110), 20), "toggle_walkable hides the overlay, got {:?}", hidden.get_pixel(160, 110));

    ed.call("set_editor", json!({ "live_link_port": free_port() }));
    let e = ed.call_err("walkability", json!({}));
    assert!(e.contains("not running"), "{e}");
    std::thread::sleep(Duration::from_millis(1500));
    let e = ed.call_err("run_action", json!({ "action": "toggle_walkable" }));
    assert!(e.contains("Cannot show the walkable area") && e.contains("not running"), "{e}");
}

/// The selected entity is outlined two pixels thick on paper, where one pixel washes out to pink, and one pixel as before on dark.
#[test]
#[ignore]
fn selected_entity_outline_reads_on_paper() {
    let ed = Editor::launch("selection_outline");
    let id = ed.call("create_entity", json!({ "classname": "info_player_start", "origin": [7, 0, 5] }))["id"].clone();
    ed.call("create_entity", json!({ "classname": "info_player_start", "origin": [-50, 0, -30] }));
    ed.call("select", json!({ "ids": [id] }));
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 4 }));
    let args = json!({ "target": "top", "width": 400, "height": 300, "overlays": true });
    let count_near = |img: &image::RgbaImage, want: [u8; 3]| img.pixels().filter(|p| (0..3).all(|i| p.0[i].abs_diff(want[i]) <= 3)).count();
    let ratio = |a: [u8; 3], b: egui::Color32| {
        let lum = |c: [u8; 3]| {
            let l = |v: u8| egui::Rgba::from(egui::Color32::from_rgb(v, v, v)).r();
            0.2126 * l(c[0]) + 0.7152 * l(c[1]) + 0.0722 * l(c[2])
        };
        let (a, b) = (lum(a) + 0.05, lum([b.r(), b.g(), b.b()]) + 0.05);
        a.max(b) / a.min(b)
    };

    let selected = td_editor::scene::SELECTED_COLOR;
    let dark = shot(&ed, args.clone(), "dark");
    let half = [188, 79, 66];
    assert_eq!(count_near(&dark, half), 1300, "the dark outline keeps its one pixel look");
    assert_eq!(count_near(&dark, over(selected, td_editor::theme::DARK.view_2d, 1)), 0);

    ed.call("set_editor", json!({ "theme": "light" }));
    theme_active(&ed, "light");
    let light = td_editor::theme::LIGHT;
    let paper = shot(&ed, args.clone(), "light");
    let ink = over(td_render::ink::flip_linear(selected, light.flip_pivot().unwrap()), light.view_2d, 1);
    assert_eq!(ink, [192, 47, 34]);
    assert!(ratio(ink, light.view_2d) >= 3.0);
    let full = count_near(&paper, ink);
    // The entity box is about 128 px a side, and a two pixel line covers one full row of it at any offset.
    assert!(full >= 4 * 120, "only {full} pixels of the full outline color on paper");
    assert_eq!(count_near(&paper, [217, 179, 180]), 0, "no half covered pink outline is left");

    ed.call("set_editor", json!({ "theme": "dark" }));
    theme_active(&ed, "dark");
    assert_eq!(count_near(&shot(&ed, args, "dark_again"), half), 1300, "switching back restores the dark outline");
}

/// A selected terrain is outlined two pixels thick on paper like a selected entity, its chunk lines stay thin.
#[test]
#[ignore]
fn selected_terrain_outline_reads_on_paper() {
    let ed = Editor::launch("terrain_outline");
    let id = ed.call("create_terrain", json!({ "origin": [-9, 0, -11], "resolution": 3, "cell_size": 16, "shape": "flat" }))["id"].clone();
    ed.call("select", json!({ "ids": [id] }));
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 4 }));
    ed.call("set_editor", json!({ "theme": "light" }));
    theme_active(&ed, "light");
    let paper = shot(&ed, json!({ "target": "top", "width": 400, "height": 300, "overlays": true }), "light");
    let count_near = |want: [u8; 3]| paper.pixels().filter(|p| (0..3).all(|i| p.0[i].abs_diff(want[i]) <= 3)).count();
    let light = td_editor::theme::LIGHT;
    let ink = over(td_render::ink::flip_linear(td_editor::scene::SELECTED_COLOR, light.flip_pivot().unwrap()), light.view_2d, 1);
    let full = count_near(ink);
    assert!(full >= 4 * 120, "only {full} pixels of the full outline color on paper");
    assert_eq!(count_near([217, 179, 180]), 0, "no half covered pink outline is left");
}

/// An unselected I/O link is a translucent line, which washed out to gray on paper. It keeps its contrast there now,
/// and the dark view blends it as before.
#[test]
#[ignore]
fn io_links_read_on_paper() {
    let ed = Editor::launch("io_links_paper");
    ed.call("create_entity", json!({ "classname": "info_player_start", "origin": [170, 0, 60], "properties": { "targetname": "dst" } }));
    ed.call(
        "create_entity",
        json!({ "classname": "info_player_start", "origin": [-180, 0, -40], "outputs": [{ "output": "OnTrigger", "target": "dst", "input": "Trigger" }] }),
    );
    ed.call("run_action", json!({ "action": "select_none" }));
    ed.call("set_camera", json!({ "view": "top", "center": [0, 0, 0], "zoom": 1 }));
    let args = json!({ "target": "top", "width": 400, "height": 300, "overlays": true });
    let lum = |c: [u8; 3]| {
        let l = |v: u8| egui::Rgba::from(egui::Color32::from_rgb(v, v, v)).r();
        0.2126 * l(c[0]) + 0.7152 * l(c[1]) + 0.0722 * l(c[2])
    };
    let ratio = |a: [u8; 3], b: egui::Color32| {
        let (a, b) = (lum(a) + 0.05, lum([b.r(), b.g(), b.b()]) + 0.05);
        a.max(b) / a.min(b)
    };
    // Pixels the slanted line covers fully, away from the grid lines that cross it.
    let count_near = |img: &image::RgbaImage, want: [u8; 3]| img.pixels().filter(|p| (0..3).all(|i| p.0[i].abs_diff(want[i]) <= 3)).count();

    let (dark, light) = (td_editor::theme::DARK, td_editor::theme::LIGHT);
    let io = td_editor::scene::IO_LINK;
    let unselected = [io[0], io[1], io[2], io[3] * 0.5 * 1.6];
    let twin = over(unselected, dark.view_2d, 1);
    let on_dark = count_near(&shot(&ed, args.clone(), "dark"), twin);
    assert!(on_dark >= 10, "only {on_dark} pixels of the dark link color {twin:?}");

    ed.call("set_editor", json!({ "theme": "light" }));
    theme_active(&ed, "light");
    let c = td_render::ink::flip_linear(unselected, light.flip_pivot().unwrap());
    let ink = over([c[0], c[1], c[2], td_render::ink::paper_alpha(unselected[3])], light.view_2d, 1);
    assert!(ratio(ink, light.view_2d) >= 3.0, "the link color {ink:?} is too faint on paper");
    assert!(ink.iter().max().unwrap() - ink.iter().min().unwrap() >= 20, "the link color {ink:?} is gray");
    let on_paper = count_near(&shot(&ed, args, "light"), ink);
    assert!(on_paper >= 10, "only {on_paper} pixels of the paper link color {ink:?}");
}

/// Answers the GitHub release API with a rolling beta that holds `asset`, and serves its download. Returns the API url.
fn fake_release(asset: &'static str, bytes: Vec<u8>) -> String {
    use std::io::BufRead;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let release = json!({ "tag_name": "beta", "assets": [{ "name": asset, "size": bytes.len(), "browser_download_url": format!("{url}/download/{asset}") }] });
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            while reader.read_line(&mut String::new()).unwrap_or(0) > 2 {}
            let path = line.split(' ').nth(1).unwrap_or_default();
            let (status, body) = match path {
                "/releases/tags/beta" => (200, release.to_string().into_bytes()),
                p if p.ends_with(asset) => (200, bytes.clone()),
                _ => (404, b"{}".to_vec()),
            };
            let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = stream.write_all(&body);
        }
    });
    url
}

/// Dialogs open in the middle of the editor window instead of at its top left corner over the menu bar. The Shape
/// Generator keeps its top in place as its rows change, so it opens with its top a little above the middle instead.
#[test]
#[ignore]
fn dialogs_open_in_the_middle_of_the_window() {
    let ed = Editor::launch("dialogs_centered");
    let titles = ["Shape Generator", "Find and Replace", "Keyboard Shortcuts"];
    for title in titles {
        // F1 over an open dialog opens its manual page instead of the palette.
        ed.input(
            "top",
            json!([{ "type": "move" }, { "type": "key", "key": "F1" }, { "type": "text", "text": title.to_lowercase() }, { "type": "key", "key": "Enter" }]),
        );
        wait_state(&ed, title, |s| s["ui"]["windows"].as_array().is_some_and(|w| w.iter().any(|w| w["title"] == title)));
    }

    std::thread::sleep(Duration::from_millis(500));
    let state = ed.state();
    let ppp = state["ui"]["pixels_per_point"].as_f64().unwrap();
    let (width, height, _) = ed.screenshot("window", "dialogs_centered");
    let screen = (width as f64 / ppp / 2.0, height as f64 / ppp / 2.0);
    let windows = state["ui"]["windows"].as_array().unwrap();
    for title in titles {
        let window = windows.iter().find(|w| w["title"] == title).unwrap_or_else(|| panic!("{title} is not open: {:?}", window_titles(&ed)));
        let r: Vec<f64> = window["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let center = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        let middle = if title == "Shape Generator" { (r[1] - screen.1 * 0.7).abs() } else { (center.1 - screen.1).abs() };
        assert!((center.0 - screen.0).abs() < 3.0 && middle < 3.0, "{title} at {r:?} is not centered on {screen:?}");
    }
}

fn window_titles(ed: &Editor) -> Vec<String> {
    ed.state()["ui"]["windows"].as_array().unwrap().iter().map(|w| w["title"].as_str().unwrap().to_string()).collect()
}

/// A project opened for the first time asks what content it should get, and the question gets out of an agent's way on
/// its first edit. project_content downloads the same content, here from a local stand-in for the release.
#[test]
#[ignore]
fn a_new_project_asks_about_content_but_not_an_agent_at_work() {
    let dir = artifacts().join("content_project");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.godot"), "config_version=5\n").unwrap();
    let mut pack = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in [
        ("trenchdot/nature/trees/pack.json", &b"{}"[..]),
        ("trenchdot/nature/trees_detailed/pack.json", b"{}"),
        ("trenchdot/nature/bushes/pack.json", b"{}"),
        ("trenchdot/nature/trees/oak.glb", include_bytes!("../../../godot/trenchdot/nature/trees/oak.glb")),
    ] {
        pack.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        pack.write_all(bytes).unwrap();
    }

    let api = fake_release("trenchdot-nature-pack.zip", pack.finish().unwrap().into_inner());
    let ed = Editor::launch_env("content", &["--project", dir.to_str().unwrap()], &[("TRENCHDOT_RELEASES_API", &api)]);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !window_titles(&ed).contains(&"Set up this project".to_string()) {
        assert!(Instant::now() < deadline, "the content question shows for a new project: {:?}", window_titles(&ed));
        std::thread::sleep(Duration::from_millis(200));
    }

    ed.input("window", json!([{ "type": "move", "x": 20, "y": 20 }]));
    ed.screenshot("window", "wizard");
    assert!(window_titles(&ed).contains(&"Set up this project".to_string()), "looking and moving the mouse leave it open");
    ed.box_brush([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]);
    assert_eq!(window_titles(&ed), Vec::<String>::new(), "an agent's first edit closes it");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "the question alone writes nothing into the project");

    let added = ed.call("project_content", json!({ "install": "nature" }));
    assert_eq!(added["releases"], json!(["beta"]), "{added}");
    assert!(dir.join("trenchdot/nature/trees/oak.glb").is_file());
    let status = ed.call("project_content", json!({}));
    assert_eq!(
        (status["nature_pack"].as_bool(), status["demo"].as_bool(), status["wizard_open"].as_bool()),
        (Some(true), Some(false), Some(false)),
        "{status}"
    );
    assert!(ed.call_err("scatter", json!({ "op": "new_set", "preset": "bushes" })).contains("project_content"), "the pack here lacks the bushes");
    drop(ed);

    let demo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../godot").canonicalize().unwrap();
    let ed = Editor::launch_with("content_demo", &["--project", demo.to_str().unwrap()]);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(window_titles(&ed), Vec::<String>::new(), "the demo project has its content and its addon matches");
    drop(ed);

    // A project with content but an old addon is offered the update alone, which also gets out of an agent's way.
    let old = artifacts().join("content_old_addon");
    let _ = std::fs::remove_dir_all(&old);
    std::fs::create_dir_all(old.join("trenchdot/nature")).unwrap();
    std::fs::create_dir_all(old.join("addons/func_godot")).unwrap();
    std::fs::write(old.join("project.godot"), "config_version=5\n").unwrap();
    std::fs::write(old.join("addons/func_godot/plugin.cfg"), "[plugin]\nversion=\"0.0.1\"\n").unwrap();
    let ed = Editor::launch_with("content_addon", &["--project", old.to_str().unwrap()]);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !window_titles(&ed).contains(&"Set up this project".to_string()) {
        assert!(Instant::now() < deadline, "an old addon brings up the wizard: {:?}", window_titles(&ed));
        std::thread::sleep(Duration::from_millis(200));
    }

    ed.screenshot("window", "addon_wizard");
    ed.box_brush([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]);
    assert_eq!(window_titles(&ed), Vec::<String>::new(), "an agent's first edit closes it");
    assert_eq!(std::fs::read_to_string(old.join("addons/func_godot/plugin.cfg")).unwrap(), "[plugin]\nversion=\"0.0.1\"\n");
    assert!(!old.join(".trenchdot").exists(), "nothing changes without a click");
}

/// Waits for the trust prompt to show or go away.
fn wait_for_trust_prompt(ed: &Editor, shown: bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while window_titles(ed).contains(&td_editor::trust::QUESTION.to_string()) != shown {
        assert!(Instant::now() < deadline, "the trust prompt should be {}: {:?}", if shown { "shown" } else { "gone" }, window_titles(ed));
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The middle of the Restricted Mode chip in the status bar.
fn trust_chip(ed: &Editor) -> Value {
    let chip = ed.state()["ui"]["trust_chip"].clone();
    let r: Vec<f64> = chip.as_array().unwrap_or_else(|| panic!("no Restricted Mode chip: {chip}")).iter().map(|v| v.as_f64().unwrap()).collect();
    json!({ "type": "click", "x": (r[0] + r[2]) / 2.0, "y": (r[1] + r[3]) / 2.0 })
}

/// A project nobody trusted opens in Restricted Mode when it holds code. An agent opening it gets no prompt in its way,
/// the Restricted Mode chip in the status bar brings the prompt up, and closing it leaves the project restricted.
#[test]
#[ignore]
fn an_untrusted_project_opens_in_restricted_mode() {
    let dir = artifacts().join("restricted_project");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("trenchdot/nature")).unwrap();
    std::fs::write(dir.join("project.godot"), "config_version=5\n").unwrap();
    std::fs::write(dir.join("main.gd"), "extends Node\n").unwrap();
    let ed = Editor::launch("restricted");
    let opened = ed.call("open_project", json!({ "path": dir }));
    assert_eq!(opened["restricted"], true, "{opened}");
    assert_eq!(ed.state()["game"]["restricted"], true);
    assert_eq!(window_titles(&ed), Vec::<String>::new(), "no prompt stands in an agent's way");

    // Nothing has Godot run or build the project, and an agent is told why.
    ed.call("map_file", json!({ "op": "save", "path": dir.join("yard.tdm") }));
    for (tool, args) in [
        ("run_action", json!({ "action": "run_godot_project" })),
        ("run_action", json!({ "action": "open_godot_editor" })),
        ("run_action", json!({ "action": "build_in_godot" })),
        ("run_action", json!({ "action": "toggle_live_mode" })),
        ("set_editor", json!({ "live_mode": true })),
        ("screenshot", json!({ "source": "godot" })),
        ("walkability", json!({})),
    ] {
        let e = ed.call_err(tool, args.clone());
        assert!(e.contains("Restricted Mode"), "{tool} {args}: {e}");
    }

    assert_eq!(ed.state()["godot"]["live_mode"], false);
    assert_eq!(window_titles(&ed), Vec::<String>::new());
    ed.input("window", json!([trust_chip(&ed)]));
    wait_for_trust_prompt(&ed, true);
    ed.screenshot("window", "trust_prompt");
    ed.input("window", json!([{ "type": "key", "key": "Escape" }]));
    wait_for_trust_prompt(&ed, false);
    assert_eq!(ed.state()["game"]["restricted"], true, "closing the prompt keeps Restricted Mode");
    assert!(ed.state()["status"].as_str().unwrap().contains("Restricted Mode"));

    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../godot").canonicalize().unwrap();
    let ed = Editor::launch_with("trusted_by_argument", &["--project", project.to_str().unwrap()]);
    let state = ed.state();
    assert_eq!((state["game"]["restricted"].as_bool(), state["ui"]["trust_chip"].is_null()), (Some(false), true), "--project is trusted for the session");

    // A map of the untrusted project leaves the trusted one open and says which project it belongs to.
    let opened = ed.call("map_file", json!({ "op": "open", "path": dir.join("yard.tdm") }));
    let other = opened["other_project"].as_str().unwrap_or_else(|| panic!("{opened}"));
    assert_eq!(std::path::Path::new(other).canonicalize().unwrap(), dir.canonicalize().unwrap());
    let state = ed.state();
    assert_eq!(std::path::Path::new(state["game"]["project_root"].as_str().unwrap()).canonicalize().unwrap(), project);
    assert_eq!(state["game"]["restricted"], false);
    assert!(state["status"].as_str().unwrap().contains("a project you have not trusted"), "{}", state["status"]);
    assert_eq!(window_titles(&ed), Vec::<String>::new());
    let tab = ed.call("map_file", json!({ "op": "open_tab", "path": project.join("demo/maps/showcase/mountain_house.tdm") }));
    assert!(tab["other_project"].is_null(), "{tab}");
}

/// A project without code opens trusted with no prompt and no chip, for as long as it is open. An agent's saves store
/// no trust, so once the project has a script it is asked about again. A save from the keyboard stores it.
#[test]
#[ignore]
fn a_project_without_code_opens_trusted() {
    let folder = artifacts().join("code_free_projects");
    let _ = std::fs::remove_dir_all(&folder);
    let (castle, keep, other) = (folder.join("castle"), folder.join("keep"), folder.join("other"));
    for dir in [&castle, &keep, &other] {
        std::fs::create_dir_all(dir.join("trenchdot/nature")).unwrap();
        std::fs::write(dir.join("project.godot"), "config_version=5\n").unwrap();
    }

    std::fs::write(other.join("main.gd"), "extends Node\n").unwrap();
    let ed = Editor::launch("code_free");
    ed.call("set_editor", json!({ "live_link_port": free_port() }));
    let opened = ed.call("open_project", json!({ "path": castle }));
    assert_eq!((&opened["restricted"], &opened["trust"]), (&json!(false), &json!("no_code")), "{opened}");
    let state = ed.state();
    assert_eq!((&state["game"]["trust"], &state["ui"]["trust_chip"]), (&json!("no_code"), &Value::Null));
    assert!(!window_titles(&ed).contains(&td_editor::trust::QUESTION.to_string()));

    ed.call("map_file", json!({ "op": "save", "path": castle.join("yard.tdm") }));
    ed.call("run_action", json!({ "action": "save" }));
    let ran = ed.call("run_script", json!({ "steps": [{ "tool": "map_file", "args": { "op": "save", "path": castle.join("court.tdm") } }] }));
    assert_eq!(ran["errors"], json!([]), "{ran}");
    assert_eq!(ed.state()["game"]["trust"], "no_code", "saves an agent makes store no trust");

    std::fs::write(castle.join("main.gd"), "extends Node\n").unwrap();
    ed.call("open_project", json!({ "path": castle }));
    assert_eq!(ed.state()["game"]["trust"], "no_code", "it stays trusted while it is open");
    ed.call("open_project", json!({ "path": other }));
    assert_eq!(ed.state()["game"]["restricted"], true);
    let opened = ed.call("open_project", json!({ "path": castle }));
    assert_eq!((&opened["restricted"], &opened["trust"]), (&json!(true), &Value::Null), "opened again with a script: {opened}");

    ed.call("open_project", json!({ "path": keep }));
    ed.call("map_file", json!({ "op": "save", "path": keep.join("yard.tdm") }));
    ed.input("window", json!([{ "type": "move", "x": 400, "y": 300 }, { "type": "key", "key": "S", "modifiers": ["ctrl"] }]));
    assert_eq!(ed.state()["game"]["trust"], "stored", "Ctrl+S is the person's own save");
    std::fs::write(keep.join("main.gd"), "extends Node\n").unwrap();
    ed.call("open_project", json!({ "path": other }));
    let opened = ed.call("open_project", json!({ "path": keep }));
    assert_eq!((&opened["restricted"], &opened["trust"]), (&json!(false), &json!("stored")), "{opened}");
}

#[test]
#[ignore]
fn malformed_arguments_are_errors_not_defaults() {
    let ed = Editor::launch("strict_args");
    let brush = ed.box_brush([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]);
    assert!(ed.call_err("create_entity", json!({ "classname": "info_null", "origin": [1, 2] })).contains("origin must be [x, y, z]"));
    assert!(ed.call_err("set_face", json!({ "id": brush, "face": 0, "scale": "big" })).contains("scale must be [x, y]"));
    assert!(ed.call_err("set_camera", json!({ "view": "3d", "position": [0, "a", 0] })).contains("position"));
    assert!(ed.call_err("duplicate", json!({ "ids": [brush], "offset": 64 })).contains("offset"));
    assert!(ed.call_err("texture", json!({ "op": "shift", "faces": [[brush]], "texels": [8, 0] })).contains("pairs"));
    assert!(ed.call_err("texture", json!({ "op": "wrap", "faces": [[brush, 1]], "source": [brush] })).contains("source"));
    let shifted = ed.call("texture", json!({ "op": "shift", "faces": [[brush.to_string(), "0"]], "texels": [8, 0] }));
    assert_eq!(shifted["changed"], json!(1), "text ids work like elsewhere");
    assert!(ed.call_err("simulate_input", json!({ "events": [{ "type": "click", "x": 5, "y": 5, "modifiers": ["meta"] }] })).contains("modifier"));
    assert!(ed.call_err("gameplay", json!({ "op": "make_door", "ids": [brush], "side": "up" })).contains("side must be left or right"));
    assert!(ed.call_err("map_file", json!({})).starts_with("op required"));
    assert!(ed.call_err("open_project", json!({})).contains("path required"));

    let mesh = ed.call("create_mesh", json!({}))["id"].as_u64().unwrap();
    assert!(ed.call_err("create_mesh", json!({ "min": [0, 0, 0] })).contains("min and max"));
    assert!(ed.call_err("mesh_edit", json!({ "id": mesh, "op": "set_material", "faces": [0] })).contains("material required"));
    assert!(ed.call_err("mesh_edit", json!({ "id": mesh, "op": "translate", "verts": [0] })).contains("offset"));
    assert!(ed.call_err("mesh_edit", json!({ "id": mesh, "op": "bisect", "point": [0, 0, 0], "normal": [0, 0, 0] })).contains("normal"));

    let e = ed.call("create_entity", json!({ "classname": "info_null", "properties": { "targetname": "a", "gone": null } }))["id"].clone();
    let props = ed.call("get_node", json!({ "id": e }))["properties"].clone();
    assert_eq!(props["targetname"], "a");
    assert!(props.get("gone").is_none(), "null leaves the key out: {props}");
    assert_eq!(ed.call("hierarchy", json!({ "op": "rename", "id": e, "name": "door" }))["changed"], json!(true));
    assert_eq!(ed.call("hierarchy", json!({ "op": "rename", "id": e, "name": "door" }))["changed"], json!(false));

    ed.call("hierarchy", json!({ "op": "set_flags", "ids": [brush], "locked": true }));
    assert!(ed.call_err("texture", json!({ "op": "shift", "faces": [[brush, 0]], "texels": [8, 0] })).contains("locked"));
    assert!(ed.call_err("trim", json!({ "ids": [brush] })).contains("locked"));
    let read = ed.call("texture", json!({ "op": "get", "faces": [[brush, 0]] }));
    assert_eq!(read["faces"].as_array().map(Vec::len), Some(1), "reading a locked face stays allowed");

    let views: Vec<Value> = ed.state()["cameras"].as_array().unwrap().iter().map(|c| c["view"].clone()).collect();
    assert!(views.contains(&json!("top")), "{views:?}");
    for v in views {
        ed.call("set_camera", json!({ "view": v }));
    }
}

#[test]
#[ignore]
fn the_mcp_http_server_wants_its_token() {
    let ed = Editor::launch_env("mcp_token", &["--mcp-token=e2e-from-the-flag"], &[("TRENCHDOT_MCP_TOKEN", "e2e-from-the-env")]);
    assert_eq!(ed.token.as_deref(), Some("e2e-from-the-flag"), "the flag wins over the environment");
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let status = |headers: &str| ed.post(headers, ping).unwrap().lines().next().unwrap_or_default().to_string();
    assert_eq!(status(""), "HTTP/1.1 401 Unauthorized");
    assert_eq!(status("Authorization: Bearer e2e-from-the-env\r\n"), "HTTP/1.1 401 Unauthorized");
    assert_eq!(status("Authorization: Bearer e2e-from-the-flag\r\n"), "HTTP/1.1 200 OK");
    assert_eq!(status("Authorization: Bearer e2e-from-the-flag\r\nOrigin: http://localhost:8000\r\n"), "HTTP/1.1 403 Forbidden");

    open_preferences(&ed);
    ed.screenshot("window", "preferences");
}

#[test]
#[ignore]
fn the_mcp_http_server_needs_no_token_by_default() {
    let ed = Editor::launch("mcp_no_token");
    assert_eq!(ed.token, None);
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let status = |headers: &str| ed.post(headers, ping).unwrap().lines().next().unwrap_or_default().to_string();
    assert_eq!(status(""), "HTTP/1.1 200 OK");
    assert_eq!(status("Authorization: Bearer anything\r\n"), "HTTP/1.1 200 OK");
    assert_eq!(status("Origin: http://localhost:8000\r\n"), "HTTP/1.1 403 Forbidden");
}

/// A TRENCHDOT_MCP_TOKEN that no header can carry starts no server, and stderr says why.
#[test]
#[ignore]
fn an_unusable_token_starts_no_server_and_says_why() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let port = free_port();
    let child = editor_command(port).env("TRENCHDOT_MCP_TOKEN", "two words").stderr(Stdio::piped()).spawn().expect("launch editor");
    let mut ed = Editor { child, port, token: None, name: "mcp_bad_token" };
    let stderr = ed.child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let line = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("stderr says why no server runs");
        if line.contains("TRENCHDOT_MCP_TOKEN must be printable ASCII") {
            break;
        }
    }

    assert!(TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(250)).is_err(), "no server listens");
    assert!(ed.child.try_wait().unwrap().is_none(), "the editor still runs");
}

/// Ticking Require a token in Preferences locks out the next request without it, unticking lets it in again, all
/// while the server keeps running.
#[test]
#[ignore]
fn the_token_checkbox_takes_effect_at_once() {
    let mut ed = Editor::launch("mcp_token_checkbox");
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let status = |ed: &Editor, headers: &str| ed.post(headers, ping).unwrap().lines().next().unwrap_or_default().to_string();
    open_preferences(&ed);
    ed.input("window", json!([mcp_pref(&ed, "require_token")]));
    assert_eq!(status(&ed, ""), "HTTP/1.1 401 Unauthorized");
    let token = std::fs::read_to_string(std::env::temp_dir().join("trenchdot-default-prefs.mcp-token")).expect("the token is kept");
    let token = token.trim().to_string();
    assert_eq!(status(&ed, &format!("Authorization: Bearer {token}\r\n")), "HTTP/1.1 200 OK");
    ed.token = Some(token);
    ed.screenshot("window", "token_required");

    ed.input("window", json!([mcp_pref(&ed, "require_token")]));
    assert_eq!(status(&ed, ""), "HTTP/1.1 200 OK");
}

/// The Theme row in Preferences opens a list whose entries switch the interface, System included, and the closed row
/// names the theme System resolves to.
#[test]
#[ignore]
fn preferences_switch_the_theme() {
    let ed = Editor::launch("prefs_theme");
    open_preferences(&ed);
    let click = |name: &str| {
        let rect = ed.state()["ui"]["mcp_prefs"][name].clone();
        let r: Vec<f64> = rect.as_array().unwrap_or_else(|| panic!("no {name} in Preferences: {rect}")).iter().map(|v| v.as_f64().unwrap()).collect();
        ed.input("window", json!([{ "type": "click", "x": (r[0] + r[2]) / 2.0, "y": (r[1] + r[3]) / 2.0 }]));
        std::thread::sleep(Duration::from_millis(300));
    };
    let ui_state = |key: &str| ed.state()["ui"][key].as_str().unwrap_or_default().to_string();
    assert!(ed.state()["ui"]["mcp_prefs"]["theme_light"].is_null(), "the list starts closed");
    assert_eq!((ui_state("theme"), ui_state("theme_active")), ("dark".into(), "dark".into()));

    click("theme");
    click("theme_light");
    assert_eq!((ui_state("theme"), ui_state("theme_active")), ("light".into(), "light".into()));
    assert!(ed.state()["ui"]["mcp_prefs"]["theme_light"].is_null(), "picking an entry closes the list");

    click("theme");
    click("theme_system");
    assert_eq!(ui_state("theme"), "system");
    let active = ui_state("theme_active");
    assert!(["dark", "light"].contains(&active.as_str()), "{active}");

    click("theme");
    click("theme_dark");
    assert_eq!((ui_state("theme"), ui_state("theme_active")), ("dark".into(), "dark".into()));
}

/// The palette lists a command per language. Running one redraws Preferences in that language while MCP keeps reading
/// English, and "english" in the palette brings the English window back exactly.
#[test]
#[ignore]
fn the_palette_switches_the_language_and_english_comes_back() {
    let ed = Editor::launch("palette_language");
    open_preferences(&ed);
    let run = |query: &str| {
        ed.input(
            "window",
            json!([{ "type": "move", "x": 10, "y": 10 }, { "type": "key", "key": "F1" }, { "type": "text", "text": query }, { "type": "key", "key": "Enter" }]),
        );
        std::thread::sleep(Duration::from_millis(500));
    };
    // The Preferences window keeps the width a longer language gave it, so only its left part is compared.
    let body = |img: &image::RgbaImage| image::imageops::crop_imm(img, 0, 56, 590, 680).to_image();
    let window = |label: &str| body(&shot(&ed, json!({ "target": "window" }), label));
    let english = window("english");

    run("language: deutsch");
    assert_eq!(ed.state()["status"], "Language: Deutsch");
    assert_eq!(window_titles(&ed).iter().filter(|t| *t == "Preferences").count(), 1, "{:?}", window_titles(&ed));
    let german = window("german");
    assert!(image_difference(&english, &german) > 0.01, "Preferences did not change");

    run("language: 日本語");
    assert_eq!(ed.state()["status"], "Language: 日本語");
    assert!(image_difference(&german, &window("japanese")) > 0.01);

    run("language: english");
    assert_eq!(ed.state()["status"], "Language: English");
    assert_eq!(image_difference(&english, &window("english_again")), 0.0);
}

/// set_editor language redraws the editor at once while MCP keeps reading English: window titles, tool names and the
/// failure status. A bad language changes nothing, not even the other settings of the same call.
#[test]
#[ignore]
fn language_switches_without_a_restart() {
    let ed = Editor::launch("language_switch");
    let language = || ed.state()["ui"]["language"].as_str().unwrap().to_string();
    // The Preferences window keeps the width a longer language gave it, so only its left part is compared.
    let body = |img: &image::RgbaImage| image::imageops::crop_imm(img, 0, 56, 590, 680).to_image();
    let window = |label: &str| body(&shot(&ed, json!({ "target": "window" }), label));
    assert_eq!(language(), "en");
    open_preferences(&ed);
    let english = window("english");

    ed.call("set_editor", json!({ "language": "de" }));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(language(), "de");
    let german = window("german");
    assert!(image_difference(&english, &german) > 0.01, "Preferences did not change");
    assert!(window_titles(&ed).contains(&"Preferences".to_string()), "{:?}", window_titles(&ed));
    let tools = ed.state()["ui"]["tool_buttons"].clone();
    assert!(tools.as_array().unwrap().iter().any(|b| b["tool"] == "Clip"), "{tools}");
    let failure = ed.call_err("run_action", json!({ "action": "csg_subtract" }));
    assert!(failure.contains("Select the box to cut out"), "{failure}");
    assert!(ed.state()["status"].as_str().unwrap().starts_with("Select the box to cut out"));

    for bad in [json!("xx"), json!(3), json!(["de"])] {
        let e = ed.call_err("set_editor", json!({ "language": bad, "ui_scale": 1.5, "grid": 8 }));
        assert!(e.contains("unknown language") && e.contains("zh-Hans"), "{e}");
        let ui = &ed.state()["ui"];
        assert_eq!((ui["language"].as_str(), ui["scale"].as_f64()), (Some("de"), Some(1.0)), "{ui}");
    }

    let set = ed.call("set_editor", json!({ "language": "JA" }));
    assert!(set.is_object(), "{set}");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(language(), "ja");
    assert!(image_difference(&german, &window("japanese")) > 0.01);

    ed.input(
        "window",
        json!([{ "type": "move", "x": 10, "y": 10 }, { "type": "key", "key": "F1" }, { "type": "text", "text": "language: english" }, { "type": "key", "key": "Enter" }]),
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(language(), "en");
    assert_eq!(image_difference(&english, &window("english_again")), 0.0);
}

/// The panels name the bundled prefabs in the editor's language, while the prefab_library tool keeps listing,
/// copying and pasting them by their English names.
#[test]
#[ignore]
fn prefab_library_names_stay_english_in_german() {
    let ed = Editor::launch("prefab_names_german");
    let names = |reply: &Value| -> Vec<String> { reply["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap().to_string()).collect() };
    let english = names(&ed.call("prefab_library", json!({ "op": "list" })));
    assert!(english.contains(&"Doorways/Window".to_string()), "{english:?}");

    ed.call("set_editor", json!({ "language": "de" }));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(names(&ed.call("prefab_library", json!({ "op": "list" }))), english);
    assert_eq!(names(&ed.call("prefab_library", json!({ "op": "list", "query": "window" }))), ["Doorways/Window"]);
    assert!(
        ed.call("prefab_library", json!({ "op": "list", "query": "fenster" }))["entries"].as_array().unwrap().is_empty(),
        "the search is on the English names"
    );
    assert_eq!(ed.call("prefab_library", json!({ "op": "copy", "name": "doorways/window" }))["name"], "Doorways/Window");
    assert!(ed.call_err("prefab_library", json!({ "op": "paste", "name": "Türöffnungen/Fenster" })).contains("no prefab"));
    assert_eq!(ed.call("prefab_library", json!({ "op": "paste", "name": "Doorways/Window" }))["ok"], true);
    assert!(ed.state()["status"].as_str().unwrap().starts_with("Pasted Window"), "{}", ed.state()["status"]);
}

/// The tool hint at the corner of a view is painted text, so it wraps to the view and follows the language while
/// the tool, its status for MCP and the history labels stay English.
#[test]
#[ignore]
fn viewport_hints_follow_the_language_and_mcp_stays_english() {
    let ed = Editor::launch("hint_language");
    let id = ed.box_brush([37.0, -21.0, 90.0], [133.0, 43.0, 154.0]);
    ed.call("select", json!({ "ids": [id] }));
    ed.call("run_action", json!({ "action": "set_tool", "args": { "tool": "scale" } }));
    let view = |label: &str| shot(&ed, json!({ "target": "window" }), label);
    let english = view("english");
    let mut seen = vec![english.clone()];
    for (code, label) in [("de", "german"), ("ru", "russian"), ("ja", "japanese")] {
        ed.call("set_editor", json!({ "language": code }));
        std::thread::sleep(Duration::from_millis(500));
        let now = view(label);
        for before in &seen {
            assert!(image_difference(before, &now) > 0.01, "{label} looks like another language");
        }

        seen.push(now);
        assert_eq!(ed.state()["editor"]["tool"].as_str().map(str::to_lowercase).as_deref(), Some("scale"));
    }

    ed.call("set_editor", json!({ "language": "en" }));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(image_difference(&english, &view("english_again")), 0.0);
}

/// validate_map words every issue in English while the editor is German, the Issues panel is what translates them.
#[test]
#[ignore]
fn validate_map_stays_english_in_german() {
    let ed = Editor::launch("validate_german");
    let first = ed.box_brush([3.0, 5.0, 9.0], [70.0, 40.0, 33.0]);
    let twin = ed.box_brush([3.0, 5.0, 9.0], [70.0, 40.0, 33.0]);
    ed.call("create_entity", json!({ "classname": "light", "origin": [8, 96, 96], "properties": { "target": "nobody" } }));
    let messages = || -> Vec<(String, String)> {
        let issues = ed.call("validate_map", json!({}));
        let list = issues["issues"].as_array().unwrap();
        assert!(list.iter().all(|i| i.get("template").is_none() && i.get("args").is_none()), "{issues}");
        let mut found: Vec<(String, String)> = list.iter().map(|i| (i["code"].as_str().unwrap().into(), i["message"].as_str().unwrap().into())).collect();
        found.sort();
        found
    };
    let english = messages();
    assert!(english.contains(&("duplicate_brush".into(), format!("Brush occupies the same space as #{first}"))), "{english:?}");
    assert!(english.contains(&("missing_target".into(), "target 'nobody' does not exist".into())), "{english:?}");
    assert_ne!(first, twin);

    ed.call("set_editor", json!({ "language": "de" }));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(messages(), english);
}

/// The palette finds a command by its German name and by its English one, and the window it opens keeps its English
/// title for MCP.
#[test]
#[ignore]
fn the_palette_matches_german_and_english_names_and_windows_keep_english_ids() {
    let ed = Editor::launch("palette_german");
    ed.call("set_editor", json!({ "language": "de" }));
    std::thread::sleep(Duration::from_millis(300));
    let run = |query: &str| {
        ed.input(
            "window",
            json!([{ "type": "move", "x": 10, "y": 10 }, { "type": "key", "key": "F1" }, { "type": "text", "text": query }, { "type": "key", "key": "Enter" }]),
        );
        std::thread::sleep(Duration::from_millis(400));
    };
    assert!(window_titles(&ed).is_empty());
    run("brush: formgenerator");
    assert_eq!(window_titles(&ed), ["Shape Generator"], "the German name opens the generator");

    run("help: keyboard shortcuts");
    let titles = window_titles(&ed);
    assert!(titles.contains(&"Keyboard Shortcuts".to_string()) && titles.contains(&"Shape Generator".to_string()), "{titles:?}");
    assert!(titles.iter().all(|t| t.is_ascii()), "MCP reads English titles: {titles:?}");

    run("qqqq");
    assert_eq!(window_titles(&ed).len(), 2, "a query nothing matches runs nothing");
}

/// The palette and the dialogs it opens work with the light colors, and the palette frame is light too.
#[test]
#[ignore]
fn command_palette_and_dialogs_open_in_light() {
    let ed = Editor::launch("light_palette");
    ed.call("set_editor", json!({ "theme": "light" }));
    std::thread::sleep(Duration::from_millis(300));
    ed.input("window", json!([{ "type": "move", "x": 900, "y": 946 }, { "type": "key", "key": "F1" }, { "type": "text", "text": "shape gen" }]));
    std::thread::sleep(Duration::from_millis(300));
    let (w, h, _) = ed.screenshot("window", "light_palette");
    let shot = image::open(artifacts().join(format!("{}_light_palette.png", ed.name))).unwrap().to_rgba8();
    // The palette sits top center and its text field is the lightest fill of the theme.
    let px = shot.get_pixel(w / 2, (h as f64 * 0.1) as u32);
    assert!(px.0[..3].iter().all(|c| *c > 200), "palette fill in light: {px:?}");

    ed.input("window", json!([{ "type": "key", "key": "Enter" }]));
    std::thread::sleep(Duration::from_millis(300));
    assert!(window_titles(&ed).contains(&"Shape Generator".to_string()), "{:?}", window_titles(&ed));
    assert_eq!(ed.state()["ui"]["theme_active"], "light");
}

/// Gameplay: Logic Graph opens the graph, brings it forward when run again, and get_state reports its canvas.
#[test]
#[ignore]
fn logic_graph_command_opens_the_graph() {
    let ed = Editor::launch("logic_command");
    assert!(ed.state()["ui"]["logic_canvas"].is_null(), "the graph is closed at first");
    let run = || {
        ed.input("window", json!([{ "type": "move", "x": 10, "y": 10 }, { "type": "key", "key": "F1" }, { "type": "text", "text": "logic graph" }, { "type": "key", "key": "Enter" }]));
        std::thread::sleep(Duration::from_millis(500));
        let state = ed.state();
        let canvas = state["ui"]["logic_canvas"].as_array().unwrap_or_else(|| panic!("no Logic graph canvas: {}", state["ui"])).clone();
        canvas.iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>()
    };
    let first = run();
    assert!(first[2] - first[0] > 100.0 && first[3] - first[1] > 100.0, "{first:?}");
    assert_eq!(run(), first, "running it again keeps the graph open");
}

/// The pixels of a window shot inside a rectangle given in points, counted by color.
fn pixel_counts(shot: &image::RgbaImage, rect: &[f64], ppp: f64) -> std::collections::HashMap<[u8; 3], usize> {
    let [x0, y0, x1, y1] = [rect[0], rect[1], rect[2], rect[3]].map(|v| (v * ppp).round() as u32);
    let mut counts = std::collections::HashMap::new();
    for y in y0..y1.min(shot.height()) {
        for x in x0..x1.min(shot.width()) {
            let p = shot.get_pixel(x, y).0;
            *counts.entry([p[0], p[1], p[2]]).or_default() += 1;
        }
    }

    counts
}

/// The Logic graph follows the theme: its canvas is the lightest fill in light and the darkest in dark, with node
/// bodies and grid lines in their own grays.
#[test]
#[ignore]
fn logic_graph_canvas_follows_the_theme() {
    let ed = Editor::launch("logic_theme");
    ed.call("create_entity", json!({ "classname": "light", "origin": [-100, 90, 70], "properties": { "targetname": "hall_lamp" } }));
    ed.call(
        "create_entity",
        json!({
        "classname": "logic_flipflop", "origin": [-40, 40, 100], "properties": { "targetname": "hall_switch" },
        "outputs": [{ "output": "on_a", "target": "hall_lamp", "input": "turn_on" }, { "output": "on_b", "target": "hall_lamp", "input": "turn_off" }] }),
    );
    assert!(ed.state()["ui"]["logic_canvas"].is_null(), "the graph is closed at first");
    ed.input("window", json!([{ "type": "move", "x": 10, "y": 10 }, { "type": "key", "key": "F1" }, { "type": "text", "text": "logic graph" }, { "type": "key", "key": "Enter" }]));
    std::thread::sleep(Duration::from_millis(500));

    let canvas_colors = |label: &str| {
        let window = shot(&ed, json!({ "target": "window" }), label);
        let state = ed.state();
        let ppp = state["ui"]["pixels_per_point"].as_f64().unwrap();
        let rect: Vec<f64> = state["ui"]["logic_canvas"]
            .as_array()
            .unwrap_or_else(|| panic!("no Logic graph canvas: {}", state["ui"]))
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let counts = pixel_counts(&window, &rect, ppp);
        let total: usize = counts.values().sum();
        (counts, total)
    };
    let rgb = |c: egui::Color32| [c.r(), c.g(), c.b()];
    let (palette, other) = (td_editor::theme::LIGHT, td_editor::theme::DARK);
    for (theme, p, q) in [("dark", other, palette), ("light", palette, other)] {
        ed.call("set_editor", json!({ "theme": theme }));
        theme_active(&ed, theme);
        std::thread::sleep(Duration::from_millis(300));
        let (counts, total) = canvas_colors(&format!("logic_{theme}"));
        let count = |c: egui::Color32| counts.get(&rgb(c)).copied().unwrap_or(0);
        assert!(count(p.gray_0) * 2 > total, "{theme}: the canvas is gray_0, {} of {total} pixels", count(p.gray_0));
        assert!(count(p.gray_1) > 0, "{theme}: grid lines are gray_1");
        assert!(count(p.gray_2) > 1000, "{theme}: two node bodies are gray_2, {} pixels", count(p.gray_2));
        assert_eq!(count(q.gray_0), 0, "{theme}: nothing is drawn in the other theme's canvas color");
    }
}

fn open_preferences(ed: &Editor) {
    ed.input("window", json!([{ "type": "key", "key": "F1" }, { "type": "text", "text": "preferences" }, { "type": "key", "key": "Enter" }]));
    assert!(window_titles(ed).contains(&"Preferences".to_string()), "{:?}", window_titles(ed));
    std::thread::sleep(Duration::from_millis(300));
}

/// A click on the middle of an MCP setting in Preferences, which has to be open. The settings sit under the
/// collapsed Advanced header, which this opens first.
fn mcp_pref(ed: &Editor, name: &str) -> Value {
    if ed.state()["ui"]["mcp_prefs"][name].is_null() {
        let header = ed.state()["ui"]["mcp_prefs"]["advanced"].clone();
        let r: Vec<f64> =
            header.as_array().unwrap_or_else(|| panic!("no Advanced header in Preferences: {header}")).iter().map(|v| v.as_f64().unwrap()).collect();
        ed.input("window", json!([{ "type": "click", "x": r[0] + 10.0, "y": (r[1] + r[3]) / 2.0 }]));
        std::thread::sleep(Duration::from_millis(300));
    }

    let rect = ed.state()["ui"]["mcp_prefs"][name].clone();
    let r: Vec<f64> = rect.as_array().unwrap_or_else(|| panic!("no {name} in Preferences: {rect}")).iter().map(|v| v.as_f64().unwrap()).collect();
    json!({ "type": "click", "x": (r[0] + r[2]) / 2.0, "y": (r[1] + r[3]) / 2.0 })
}

fn answers(port: u16) -> bool {
    TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(250)).is_ok()
}

/// Waits a few seconds for the MCP HTTP port to close.
fn wait_until_closed(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while answers(port) {
        assert!(Instant::now() < deadline, "the MCP port stays open");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A map saved under the old name, as JSON text or in the binary container, opens as modified under its own path, says
/// it was updated, and saves back in place in the current format with its .bak next to it.
#[test]
#[ignore]
fn a_map_of_the_old_format_opens_updated_and_saves_in_place() {
    use td_doc::{Entity, Map, NodeKind, format, legacy};

    let mut map = Map::new();
    let layer = map.default_layer();
    let mut door = Entity::new("func_door");
    door.properties.insert("targetname".into(), "door1".into());
    map.insert(layer, NodeKind::Entity(door));
    let mut value: Value = serde_json::from_str(&format::to_string(&map)).unwrap();
    value["format"] = legacy::MAP_FORMAT.into();
    let old = legacy::MAP_EXTS[1];
    let dir = artifacts().join("legacy_maps");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let text = dir.join(format!("text_room.{old}"));
    let binary = dir.join(format!("binary_room.{old}"));
    std::fs::write(&text, value.to_string()).unwrap();
    std::fs::write(&binary, td_doc::binary::encode(value, &[]).unwrap()).unwrap();
    assert!(td_doc::binary::is_binary(&std::fs::read(&binary).unwrap()));

    let ed = Editor::launch("legacy_map");
    for (path, name) in [(&text, format!("text_room.{old}")), (&binary, format!("binary_room.{old}"))] {
        let opened = ed.call("map_file", json!({ "op": "open", "path": path }));
        assert_eq!(opened["problems"], json!([]), "{opened}");
        let state = ed.state();
        assert_eq!(state["map"]["modified"], true, "{state}");
        assert_eq!(state["map"]["entities"], 1);
        let status = state["status"].as_str().unwrap();
        assert_eq!(status, format!("Updated {name} from the {} format, save to keep it", legacy::NAME));
        assert!(state["map"]["path"].as_str().unwrap().ends_with(&name));

        let saved = ed.call("map_file", json!({ "op": "save" }));
        assert!(saved["path"].as_str().unwrap().ends_with(&name), "{saved}");
        assert_eq!(ed.state()["map"]["modified"], false);
        let bytes = std::fs::read(path).unwrap();
        assert!(td_doc::binary::is_binary(&bytes), "saved in the binary container");
        let (value, _) = format::file_json_value(&bytes).unwrap();
        assert_eq!(value["format"], format::FORMAT_NAME);
        assert!(path.with_extension(format!("{old}.bak")).is_file(), "the old file is kept as .bak");
        assert!(!path.with_extension("tdm").exists());
        ed.call("map_file", json!({ "op": "open", "path": path }));
        assert_eq!(ed.state()["map"]["modified"], false, "the saved map needs no update");
    }
}

/// With TRENCHDOT_DISABLE_MCP set, --mcp-http opens no port and --mcp answers initialize with an error, while the
/// editor itself runs on and says why on stderr.
#[test]
#[ignore]
fn mcp_turned_off_by_the_variable_serves_nothing() {
    mcp_off_by_variable("TRENCHDOT_DISABLE_MCP", "mcp_off_env");
}

/// The variable's old name, from before TrenchDot was renamed, is still a kill switch.
#[test]
#[ignore]
fn mcp_turned_off_by_the_old_variable_serves_nothing() {
    mcp_off_by_variable(&format!("{}DISABLE_MCP", td_editor::legacy::ENV_PREFIX), "mcp_off_old_env");
}

fn mcp_off_by_variable(variable: &str, name: &'static str) {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let port = free_port();
    let child = editor_command(port)
        .arg("--mcp")
        .env(variable, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch editor");
    let mut ed = Editor { child, port, token: None, name };
    let mut stdin = ed.child.stdin.take().unwrap();
    let stdout = ed.child.stdout.take().unwrap();
    let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "e2e", "version": "1" } } });
    writeln!(stdin, "{init}").unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx.recv_timeout(Duration::from_secs(60)).expect("an answer on stdout");
    let answer: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
    assert_eq!(answer["id"], 1);
    assert!(answer["error"]["message"].as_str().unwrap_or_default().contains("turned off in TrenchDot by TRENCHDOT_DISABLE_MCP"), "{answer}");

    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        assert!(!answers(port), "the MCP HTTP port answered");
        std::thread::sleep(Duration::from_millis(200));
    }

    assert!(ed.child.try_wait().unwrap().is_none(), "the editor still runs");
    let mut stderr = ed.child.stderr.take().unwrap();
    let _ = ed.child.kill();
    let _ = ed.child.wait();
    let mut said = String::new();
    let _ = stderr.read_to_string(&mut said);
    assert!(said.contains("MCP is turned off by TRENCHDOT_DISABLE_MCP"), "{said}");
}

/// set_editor mcp false answers, then the port closes and the connections still open are dropped.
#[test]
#[ignore]
fn mcp_can_be_turned_off_over_mcp() {
    let mut ed = Editor::launch("mcp_off_call");
    let mut idle = TcpStream::connect(("127.0.0.1", ed.port)).unwrap();
    idle.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    assert_eq!(ed.call("set_editor", json!({ "mcp": false }))["mcp"], false);
    // Until the port closes, nothing but that last answer gets through.
    for (method, params) in
        [("tools/call", json!({ "name": "get_state", "arguments": {} })), ("initialize", json!({})), ("tools/list", json!({})), ("ping", json!({}))]
    {
        if let Some(r) = ed.try_rpc(method, params) {
            assert!(r["result"].is_null() && r["error"]["message"].as_str().unwrap_or_default().contains("turned off in TrenchDot"), "{method}: {r}");
        }
    }

    wait_until_closed(ed.port);
    let mut buf = [0u8; 1];
    assert!(matches!(idle.read(&mut buf), Ok(0) | Err(_)), "the idle connection is dropped");
    assert!(ed.child.try_wait().unwrap().is_none(), "the editor still runs");
}

/// A call that waits for Godot gets the error as soon as MCP goes off, instead of whenever Godot answers.
#[test]
#[ignore]
fn turning_mcp_off_answers_calls_that_wait_for_godot() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let dir = artifacts().join("mcp_off_godot");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("maps")).unwrap();
    std::fs::write(
        dir.join("project.godot"),
        "config_version=5
",
    )
    .unwrap();
    let map = dir.join("maps/yard.tdm");
    let (godot, asked) = fake_godot(&dir, &[&map], json!({}), false);
    let port = free_port();
    let child = editor_command(port).args(["--mcp", "--project", dir.to_str().unwrap()]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut ed = Editor { child, port, token: None, name: "mcp_off_godot" };
    let deadline = Instant::now() + Duration::from_secs(60);
    while ed.try_rpc("ping", json!({})).is_none() {
        assert!(Instant::now() < deadline, "editor did not start");
        std::thread::sleep(Duration::from_millis(250));
    }

    ed.call("set_editor", json!({ "live_link_port": godot }));
    ed.call("create_brush", json!({ "min": [0, 0, 0], "max": [256, 16, 256] }));
    ed.call("map_file", json!({ "op": "save", "path": map }));
    let mut stdin = ed.child.stdin.take().unwrap();
    let stdout = ed.child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let call = json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "screenshot", "arguments": { "source": "godot" } } });
    writeln!(stdin, "{call}").unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while asked.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the capture never reached Godot");
        std::thread::sleep(Duration::from_millis(100));
    }

    assert!(rx.recv_timeout(Duration::from_millis(500)).is_err(), "Godot has not answered");
    ed.call("set_editor", json!({ "mcp": false }));
    let line = rx.recv_timeout(Duration::from_secs(10)).expect("the waiting call is answered");
    let answer: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
    assert_eq!((answer["id"].as_u64(), answer["result"]["isError"].as_bool()), (Some(7), Some(true)), "{answer}");
    assert!(answer["result"]["content"][0]["text"].as_str().unwrap_or_default().contains("turned off in TrenchDot"), "{answer}");
}

/// Unticking MCP in Preferences closes the port in the same frame, so not even the input call that clicked gets an answer.
#[test]
#[ignore]
fn mcp_can_be_turned_off_in_preferences() {
    let mut ed = Editor::launch("mcp_off_prefs");
    open_preferences(&ed);
    let click = mcp_pref(&ed, "mcp");
    assert_eq!(ed.try_rpc("tools/call", json!({ "name": "simulate_input", "arguments": { "target": "window", "events": [click] } })), None);
    wait_until_closed(ed.port);
    assert!(ed.child.try_wait().unwrap().is_none(), "the editor still runs");
}

#[test]
#[ignore]
fn tool_defaults_paths_and_aliases_agree() {
    let dir = artifacts().join("tool_defaults");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("maps")).unwrap();
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(dir.join("project.godot"), "config_version=5\n").unwrap();
    add_pack_template(&dir);
    let ed = Editor::launch_with("tool_defaults", &["--project", dir.to_str().unwrap()]);
    let bounds = |v: &Value| -> Vec<f64> { ["min", "max"].iter().flat_map(|k| v[*k].as_array().unwrap().iter().map(|x| x.as_f64().unwrap())).collect() };

    // Rotate defaults to 90 degrees about Y and pivots on the exact center unless snapping is on.
    ed.call("set_editor", json!({ "grid": 16, "snap": false }));
    let brush = ed.box_brush([0.0, 0.0, 0.0], [10.0, 8.0, 30.0]);
    ed.call("select", json!({ "ids": [brush] }));
    let turned = ed.call("transform", json!({ "rotate": {} }));
    assert!(approx(&bounds(&turned["bounds"]), &[-10.0, 0.0, 10.0, 20.0, 8.0, 20.0]), "{turned}");
    ed.call("run_action", json!({ "action": "undo" }));
    ed.call("select", json!({ "ids": [brush] }));
    let snapped = ed.call("transform", json!({ "rotate": { "axis": "y", "degrees": 90 }, "snap": true }));
    assert!(approx(&bounds(&snapped["bounds"]), &[-16.0, 0.0, 6.0, 14.0, 8.0, 16.0]), "{snapped}");
    assert!(ed.call_err("transform", json!({ "flip": "x", "uv_lock": "on" })).contains("uv_lock"));
    assert!(ed.call_err("run_action", json!({ "action": "rotate", "args": { "axis": "w" } })).contains("axis"));
    assert!(ed.call_err("run_action", json!({ "action": "flip", "args": { "axis": 2 } })).contains("axis"));
    let before_nudge = (ed.state()["undo"].clone(), ed.selection_bounds());
    for bad in [json!({ "action": "nudge", "offset": [16, 0, 0] }), json!({ "action": "nudge", "args": { "offset": [16, 0] } })] {
        assert!(ed.call_err("run_action", bad.clone()).contains("nudge needs args.offset"), "{bad}");
    }

    assert_eq!((ed.state()["undo"].clone(), ed.selection_bounds()), before_nudge, "a refused nudge moves nothing");
    let undo_steps = ed.state()["undo"].clone();
    assert_eq!(ed.call("run_action", json!({ "action": "undo", "args": { "steps": 0 } }))["steps"], 0);
    assert_eq!(ed.state()["undo"], undo_steps, "zero steps undoes nothing");
    assert!(ed.call_err("run_action", json!({ "action": "toggle_cordon" })).contains("No cordon"));

    let platform = ed.call("gameplay", json!({ "op": "make_platform", "ids": [brush], "mode": "ping_pong" }));
    assert_eq!(platform["properties"]["mode"], "1");
    let layer = ed.state()["editor"]["current_layer"].clone();
    let moved = ed.call("hierarchy", json!({ "op": "reparent", "ids": [platform["id"]], "parent": layer }));
    assert_eq!((moved["moved"].clone(), moved["ids"].clone()), (json!(1), json!([platform["id"]])));

    // A terrain's origin is its corner, center lays it out around a point.
    let t = ed.call("create_terrain", json!({ "center": [1000, 0, 0], "resolution": 9, "cell_size": 64 }));
    let tb = bounds(&t["bounds"]);
    assert!(approx(&[tb[0], tb[2], tb[3], tb[5]], &[744.0, -256.0, 1256.0, 256.0]), "{t}");
    assert!(ed.call_err("create_terrain", json!({ "center": [0, 0, 0], "origin": [0, 0, 0] })).contains("not both"));
    assert!(ed.call_err("terrain_edit", json!({ "id": t["id"], "op": "clear_layer" })).contains("layer"));
    assert!(ed.call_err("terrain_edit", json!({ "id": t["id"], "op": "sculpt", "center": [1000, 0], "mode": "dig" })).contains("erase_alpha"));

    // res:// and relative paths start at the project for every file argument.
    let saved = ed.call("map_file", json!({ "op": "save", "path": "maps/defaults.tdm" }));
    assert!(dir.join("maps/defaults.tdm").is_file(), "{saved}");
    let exported = ed.call("map_file", json!({ "op": "export_map", "path": "res://maps/defaults.map" }));
    assert!(dir.join("maps/defaults.map").is_file());
    assert_eq!(exported["exported"], exported["path"]);
    assert!(exported["map_path"].as_str().unwrap().ends_with("maps/defaults.tdm"), "{exported}");
    assert_eq!(ed.call("changes_since", json!({ "file": "res://maps/defaults.tdm" }))["since"], "the map file res://maps/defaults.tdm");
    assert!(ed.call_err("changes_since", json!({ "file": "maps/defaults.tdm", "undo_steps": 1 })).contains("not both"));

    // Saved selections follow brushes a later step replaced.
    let script = json!({ "format": "trenchdot-mcp-script", "steps": [
        { "tool": "create_brush", "args": { "min": [0, 0, 400], "max": [128, 64, 416] }, "save": "wall" },
        { "tool": "select", "args": { "ids": "$wall.ids" }, "save": "sel" },
        { "tool": "create_brush", "args": { "min": [48, 0, 390], "max": [80, 48, 426] } },
        { "tool": "run_action", "args": { "action": "csg_subtract" } },
        { "tool": "select", "args": { "ids": "$sel.selected" }, "save": "pieces" }
    ] });
    std::fs::write(dir.join("scripts/cut.json"), script.to_string()).unwrap();
    let ran = ed.call("run_script", json!({ "path": "res://scripts/cut.json" }));
    assert_eq!(ran["errors"], json!([]), "{ran}");
    assert!(ran["vars"]["pieces"]["selected"].as_array().unwrap().len() >= 2, "{ran}");

    let r = ed.try_rpc("tools/call", json!({ "name": "screenshot", "arguments": { "target": "3d", "width": 320 } })).unwrap();
    let png = base64::engine::general_purpose::STANDARD.decode(r["result"]["content"][0]["data"].as_str().expect("image")).unwrap();
    let img = image::load_from_memory(&png).unwrap();
    assert_eq!(img.width(), 320, "width alone sizes the capture");
    assert!(img.height() >= 16 && img.height() < 640, "{}", img.height());
}

/// A 2D view in window pixels with its top left corner, shrunk so the dock card edge stays out of it.
fn view_pixels(ed: &Editor, window: &image::RgbaImage, view: &str) -> (image::RgbaImage, (u32, u32)) {
    let state = ed.state();
    let ppp = state["ui"]["pixels_per_point"].as_f64().unwrap();
    let r: Vec<f64> = state["cameras"].as_array().unwrap().iter().find(|c| c["view"] == view).unwrap()["rect"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap() * ppp)
        .collect();
    let (x, y) = (r[0] as u32 + 4, r[1] as u32 + 4);
    (image::imageops::crop_imm(window, x, y, r[2] as u32 - 8, r[3] as u32 - 8).to_image(), (x, y))
}

/// The linear alpha blending of the line shader on the CPU, `top` drawn `times` over `under`.
fn over(top: [f32; 4], under: egui::Color32, times: u32) -> [u8; 3] {
    let mut below = egui::Rgba::from(under);
    for _ in 0..times {
        let mix = |t: f32, u: f32| t * top[3] + u * (1.0 - top[3]);
        below = egui::Rgba::from_rgb(mix(top[0], below.r()), mix(top[1], below.g()), mix(top[2], below.b()));
    }

    let c = egui::Color32::from(below);
    [c.r(), c.g(), c.b()]
}

#[test]
#[ignore]
fn two_d_views_turn_to_paper() {
    let ed = Editor::launch("paper_2d");
    ed.box_brush([-111.5, 0.0, 24.0], [80.0, 96.0, 152.0]);
    ed.call("run_action", json!({ "action": "select_none" }));
    ed.call("set_camera", json!({ "view": "top", "center": [16, 0, 88], "zoom": 1.0 }));
    let args = json!({ "target": "top", "width": 320, "height": 200 });
    let dark_top = shot(&ed, args.clone(), "dark_top");
    let dark_window = shot(&ed, json!({ "target": "window" }), "dark_window");
    let (dark_crop, _) = view_pixels(&ed, &dark_window, "top");
    let (dark, light) = (td_editor::theme::DARK, td_editor::theme::LIGHT);
    let rgb = |c: egui::Color32| [c.r(), c.g(), c.b()];
    assert_eq!(dark_top.get_pixel(2, 2).0[..3], rgb(dark.view_2d), "Dark keeps the dark background");

    ed.call("set_editor", json!({ "theme": "light" }));
    theme_active(&ed, "light");
    let top = shot(&ed, args.clone(), "light_top");
    let paper = rgb(light.view_2d);
    for (x, y) in [(1, 1), (318, 1), (1, 198), (318, 198)] {
        assert_eq!(top.get_pixel(x, y).0[..3], paper, "paper at {x},{y}");
    }

    // The brush edge at x -111.5 lands on the middle of pixel 32, 160 + (-111.5 - 16), with zoom 1.
    let edge = td_editor::scene::EDGE_COLOR_2D;
    let c = td_render::ink::flip_linear(edge, light.flip_pivot().unwrap());
    // The 2D outline sits in both the wire and the overlay line list, so it blends in twice.
    let twin = over([c[0], c[1], c[2], td_render::ink::paper_alpha(edge[3])], light.view_2d, 2);
    let unflipped = over(edge, light.view_2d, 2);
    let at = top.get_pixel(32, 100).0;
    assert!((0..3).all(|i| at[i].abs_diff(twin[i]) <= 6), "edge pixel {at:?}, the CPU twin says {twin:?}");
    assert!(at[0].abs_diff(unflipped[0]) > 60, "the edge is not the unflipped {unflipped:?}");
    assert_eq!(top.get_pixel(31, 100).0[..3], paper, "the line is one pixel wide");

    // The view name at the top left is painted by egui, bright in Dark and flipped dark in Light.
    let light_window = shot(&ed, json!({ "target": "window" }), "light_window");
    let (light_crop, _) = view_pixels(&ed, &light_window, "top");
    let ppp = ed.state()["ui"]["pixels_per_point"].as_f64().unwrap();
    let (w, h) = ((100.0 * ppp) as u32, (18.0 * ppp) as u32);
    let mut label = 0;
    for y in 0..h {
        for x in 0..w {
            let (d, l) = (dark_crop.get_pixel(x, y).0, light_crop.get_pixel(x, y).0);
            if d[..3].iter().max().unwrap() > &205 {
                label += 1;
                assert!(l[..3].iter().max().unwrap() < &120, "label pixel {x},{y} is {d:?} in Dark and {l:?} in Light");
            }
        }
    }

    assert!(label >= 8, "only {label} bright label pixels in the Dark crop");

    ed.call("set_editor", json!({ "theme": "dark" }));
    theme_active(&ed, "dark");
    assert_eq!(image_difference(&dark_top, &shot(&ed, args, "dark_top_again")), 0.0);
    let (again, _) = view_pixels(&ed, &shot(&ed, json!({ "target": "window" }), "dark_window_again"), "top");
    assert_eq!(image_difference(&dark_crop, &again), 0.0, "Dark draws the Top view as before");
}

#[test]
#[ignore]
fn align_and_distribute_over_mcp() {
    let ed = Editor::launch("align_distribute");
    let a = ed.box_brush([0.0, 0.0, 0.0], [32.0, 32.0, 32.0]);
    let b = ed.box_brush([100.0, 10.0, 0.0], [116.0, 26.0, 16.0]);
    let c = ed.box_brush([300.0, 40.0, 0.0], [364.0, 104.0, 64.0]);
    let bounds_of = |id: u64| {
        ed.call("select", json!({ "ids": [id] }));
        ed.selection_bounds()
    };
    ed.call("select", json!({ "ids": [a, b, c] }));

    assert!(ed.call_err("run_action", json!({ "action": "align", "args": { "to": "min" } })).contains("axis"));
    assert!(ed.call_err("run_action", json!({ "action": "align", "args": { "axis": "y", "to": "top" } })).contains("min, center or max"));
    assert!(ed.call_err("run_action", json!({ "action": "distribute", "args": { "axis": "w" } })).contains("axis"));

    ed.call("run_action", json!({ "action": "align", "args": { "axis": "y", "to": "min" } }));
    for id in [a, b, c] {
        assert_eq!(bounds_of(id).0[1], 0.0, "every bottom is on the lowest one");
    }

    assert!(approx(&bounds_of(c).1, &[364.0, 64.0, 64.0]), "sizes are kept");

    ed.call("select", json!({ "ids": [a, b, c] }));
    assert!(ed.call_err("run_action", json!({ "action": "align", "args": { "axis": "y", "to": "min" } })).contains("already line up"));
    ed.call("run_action", json!({ "action": "align", "args": { "axis": "z", "to": "center" } }));
    for (id, size) in [(a, 32.0), (b, 16.0), (c, 64.0)] {
        let (min, max) = bounds_of(id);
        assert!(((min[2] + max[2]) / 2.0 - 32.0).abs() < 1e-6 && (max[2] - min[2] - size).abs() < 1e-6);
    }

    ed.call("select", json!({ "ids": [a, b] }));
    assert!(ed.call_err("run_action", json!({ "action": "distribute", "args": { "axis": "x" } })).contains("at least three"));

    ed.call("select", json!({ "ids": [a, b, c] }));
    ed.call("run_action", json!({ "action": "distribute", "args": { "axis": "x" } }));
    assert_eq!(bounds_of(a).0[0], 0.0);
    assert_eq!(bounds_of(c).1[0], 364.0);
    let (b_min, b_max) = bounds_of(b);
    assert!((b_min[0] - 32.0 - (300.0 - b_max[0])).abs() < 1e-6, "equal gaps, b is at {b_min:?}");

    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(bounds_of(b).0[0], 100.0, "one undo step brings the distribute back");
}

fn vertices_of(ed: &Editor, id: u64) -> Vec<[i64; 3]> {
    let node = ed.call("get_node", json!({ "id": id }));
    let mut out: Vec<[i64; 3]> =
        node["vertices"].as_array().unwrap().iter().map(|v| [0, 1, 2].map(|i| (v[i].as_f64().unwrap() * 1000.0).round() as i64)).collect();
    out.sort();
    out
}

fn mirrored_x(vertices: &[[i64; 3]], plane: f64) -> Vec<[i64; 3]> {
    let mut out: Vec<[i64; 3]> = vertices.iter().map(|v| [(2.0 * plane * 1000.0) as i64 - v[0], v[1], v[2]]).collect();
    out.sort();
    out
}

#[test]
#[ignore]
fn mirror_linked_copies_follow_edits_mirrored() {
    let ed = Editor::launch("mirror_linked");
    let source = ed.box_brush([8.0, 2.0, -4.0], [40.0, 34.0, 20.0]);
    ed.call("select", json!({ "ids": [source] }));
    let slant = |vertices: Value, offset: Value| {
        ed.call("run_action", json!({ "action": "move_vertices", "args": { "vertices": vertices, "offset": offset } }));
    };
    slant(json!([[40, 34, -4], [40, 34, 20]]), json!([0, -16, 0]));
    slant(json!([[8, 34, 20], [40, 18, 20]]), json!([0, 4, 0]));
    let flat = vertices_of(&ed, source);
    assert!(flat.contains(&[8000, 38000, 20000]) && flat.contains(&[40000, 22000, 20000]), "the top is slanted both ways {flat:?}");

    assert!(ed.state()["mirror_planes"].as_array().unwrap().is_empty(), "nothing is mirrored yet");
    let undo_before = ed.state()["undo"].as_array().unwrap().len();
    let reply = ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "x", "plane": "max" } }));
    assert_eq!(reply["plane"], json!({ "axis": "x", "at": 40.0 }), "{reply}");
    let copies = reply["copies"].as_array().unwrap().clone();
    assert_eq!(copies.len(), 1, "{reply}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before + 1, "wrapping and mirroring are one undo step");
    assert_eq!(ed.state()["undo"][0], "MCP: Mirror Linked");

    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0], min[1], max[1], min[2], max[2]], &[40.0, 72.0, 2.0, 38.0, -4.0, 20.0]), "the copy sits across the plane {min:?} {max:?}");
    let planes = ed.state()["mirror_planes"].clone();
    assert_eq!(planes.as_array().unwrap().len(), 1, "{planes}");
    assert_eq!((planes[0]["axis"].as_str(), planes[0]["at"].as_f64()), (Some("x"), Some(40.0)), "{planes}");
    assert_eq!(planes[0]["group"], copies[0], "{planes}");
    assert_eq!(planes[0]["gap"], json!(0.0), "the halves meet {planes}");
    ed.call("run_action", json!({ "action": "set_mirror_gap", "args": { "gap": 16 } }));
    let planes = ed.state()["mirror_planes"].clone();
    assert_eq!(planes[0]["gap"], json!(16.0), "{planes}");
    assert_eq!((planes[0]["axis"].as_str(), planes[0]["at"].as_f64()), (Some("x"), Some(40.0)), "the plane stays {planes}");
    assert_eq!(planes[0]["group"], copies[0], "{planes}");
    assert!(planes[0]["partner"].is_u64() && planes[0]["partner"] != planes[0]["group"], "{planes}");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.state()["mirror_planes"][0]["gap"], json!(0.0), "undo closes the gap");

    let other_brush = |ed: &Editor| {
        let brushes = ed.call("list_nodes", json!({ "type": "brush" }));
        let ids: Vec<u64> = brushes["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_u64().unwrap()).collect();
        assert_eq!(ids.len(), 2, "{brushes}");
        *ids.iter().find(|i| **i != source).unwrap()
    };
    let copy = other_brush(&ed);
    assert_eq!(vertices_of(&ed, copy), mirrored_x(&flat, 40.0), "the copy is the reflection of the slanted source");

    ed.call("select", json!({ "ids": [source] }));
    slant(json!([[8, 34, -4], [8, 38, 20]]), json!([0, -6, 0]));
    let edited = vertices_of(&ed, source);
    assert_ne!(edited, flat);
    let copy = other_brush(&ed);
    assert_eq!(vertices_of(&ed, copy), mirrored_x(&edited, 40.0), "a vertex edit in one half shows mirrored in the other");
    assert_eq!(ed.call("validate_map", json!({}))["count"], 0, "the mirrored half has outward faces");

    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(vertices_of(&ed, source), flat);
    let copy = other_brush(&ed);
    assert_eq!(vertices_of(&ed, copy), mirrored_x(&flat, 40.0), "one undo step takes both halves back");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(ed.brushes(), 1, "the next undo removes the mirror");

    ed.call("select", json!({ "ids": [source] }));
    let typed = ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "z", "at": 100.5 } }));
    assert_eq!(typed["plane"], json!({ "axis": "z", "at": 100.5 }), "at alone means a custom plane {typed}");
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[2], max[2]], &[181.0, 205.0]), "{min:?} {max:?}");
    assert_eq!(ed.state()["editor"]["mirror_plane"], json!({ "at": "max", "custom": 0.0 }), "a call leaves the Inspector choice alone");
    ed.call("run_action", json!({ "action": "undo" }));

    ed.call("select", json!({ "ids": [source] }));
    let world = ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "y", "plane": "world" } }));
    assert_eq!(world["plane"], json!({ "axis": "y", "at": 0.0 }), "{world}");
    assert_eq!(world["gap"], json!(4.0), "the slanted source reaches y 2, so the halves start 4 apart {world}");
    ed.call("run_action", json!({ "action": "undo" }));

    ed.call("select", json!({ "ids": [source] }));
    let wide = ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "x", "plane": "max", "gap": 16 } }));
    assert_eq!(wide["plane"], json!({ "axis": "x", "at": 48.0 }), "half the gap moves the plane {wide}");
    assert_eq!(wide["gap"], json!(16.0), "{wide}");
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0]], &[56.0, 88.0]), "the copy sits 16 further out {min:?} {max:?}");
    assert_eq!(vertices_of(&ed, source), flat, "the original stays");
    ed.call("run_action", json!({ "action": "undo" }));

    ed.call("select", json!({ "ids": [source] }));
    let overlap = ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "x", "plane": "max", "gap": -500 } }));
    assert_eq!(overlap["gap"], json!(-32.0), "a gap below minus the width stops there {overlap}");
    let at = overlap["plane"]["at"].as_f64().unwrap();
    assert_eq!(at, 24.0, "the plane stops a hair past the middle, which a reply rounds away {overlap}");
    let thousandths = |v: f64| (v * 1000.0).round() / 1000.0 == v;
    let (min, max) = ed.selection_bounds();
    assert!(min.iter().chain(&max).all(|v| thousandths(*v)), "bounds drop the slack {min:?} {max:?}");
    let state = ed.state();
    for key in ["at", "gap"] {
        assert!(state["mirror_planes"][0][key].as_f64().is_some_and(thousandths), "{key}: {}", state["mirror_planes"]);
    }

    ed.call("run_action", json!({ "action": "set_mirror_gap", "args": { "gap": 0.3 } }));
    assert_eq!(ed.state()["mirror_planes"][0]["gap"], json!(0.3), "a fractional gap reads back without drift");
    ed.call("run_action", json!({ "action": "undo" }));
    ed.call("run_action", json!({ "action": "undo" }));

    let before = ed.state()["undo"].as_array().unwrap().len();
    let refuse = |args: Value, words: &str| {
        let text = ed.call_err("run_action", json!({ "action": "mirror_linked", "args": args }));
        assert!(text.contains(words), "{args}: {text}");
    };
    refuse(json!({ "axis": "w" }), "mirror_linked axis must be x, y or z");
    refuse(json!({ "axis": 1 }), "mirror_linked axis must be x, y or z");
    refuse(json!({ "plane": "left" }), "mirror_linked plane must be center, max, min, world or custom");
    refuse(json!({ "plane": 3 }), "mirror_linked plane must be center, max, min, world or custom");
    refuse(json!({ "at": "far" }), "mirror_linked at must be a number");
    refuse(json!({ "gap": "far" }), "mirror_linked gap must be a number");
    refuse(json!({ "axis": "x", "gap": 1e300 }), "mirror_linked gap must be at most 65536 units");
    ed.call("run_action", json!({ "action": "select_none" }));
    refuse(json!({}), "select objects or groups to mirror first");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), before, "a refused call leaves no undo step");
}

#[test]
#[ignore]
fn set_mirror_gap_moves_both_halves() {
    let ed = Editor::launch("mirror_gap");
    let source = ed.box_brush([8.0, 2.0, -4.0], [40.0, 34.0, 20.0]);
    ed.call("select", json!({ "ids": [source] }));
    ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "x", "plane": "max" } }));
    let undo_before = ed.state()["undo"].as_array().unwrap().len();

    let reply = ed.call("run_action", json!({ "action": "set_mirror_gap", "args": { "gap": 24 } }));
    assert_eq!(reply["plane"], json!({ "axis": "x", "at": 40.0 }), "the plane stays {reply}");
    assert_eq!(reply["gap"], json!(24.0), "{reply}");
    let (min, max) = ed.selection_bounds();
    assert!(approx(&[min[0], max[0]], &[52.0, 84.0]), "the copy moved half of the change {min:?} {max:?}");
    let xs: Vec<i64> = vertices_of(&ed, source).iter().map(|v| v[0]).collect();
    assert_eq!((xs.iter().min(), xs.iter().max()), (Some(&-4000), Some(&28000)), "the original moved the other half {xs:?}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before + 1);
    assert_eq!(ed.state()["undo"][0], "MCP: Set Mirror Gap");

    let on_axis = ed.call("run_action", json!({ "action": "set_mirror_gap", "args": { "gap": -500, "axis": "x" } }));
    assert_eq!(on_axis["gap"], json!(-32.0), "a gap below minus the width stops there {on_axis}");
    assert_eq!(on_axis["plane"], json!({ "axis": "x", "at": 40.0 }), "{on_axis}");
    let (min, max) = ed.selection_bounds();
    assert_eq!([min[0], max[0]], [24.0, 56.0], "the copy covers the original and the slack is not shown");
    let restored = ed.call("run_action", json!({ "action": "set_mirror_gap", "args": { "gap": 0 } }));
    assert_eq!(restored["gap"], json!(0.0), "{restored}");
    let (min, max) = ed.selection_bounds();
    assert_eq!([min[0], max[0]], [40.0, 72.0], "gap 0 puts the copy back on its side");
    let xs: Vec<i64> = vertices_of(&ed, source).iter().map(|v| v[0]).collect();
    assert_eq!((xs.iter().min(), xs.iter().max()), (Some(&8000), Some(&40000)), "the original comes back on its own side {xs:?}");
    ed.call("run_action", json!({ "action": "undo", "args": { "steps": 3 } }));
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before, "each call is one undo step");
    assert_eq!(ed.state()["mirror_planes"][0]["at"], json!(40.0));

    let before = ed.state()["undo"].as_array().unwrap().len();
    let refuse = |args: Value, words: &str| {
        let text = ed.call_err("run_action", json!({ "action": "set_mirror_gap", "args": args }));
        assert!(text.contains(words), "{args}: {text}");
    };
    refuse(json!({}), "set_mirror_gap needs args.gap");
    refuse(json!({ "gap": "far" }), "set_mirror_gap needs args.gap");
    refuse(json!({ "gap": 4, "axis": "w" }), "set_mirror_gap axis must be x, y or z");
    refuse(json!({ "gap": 4, "axis": "z" }), "no mirror plane on z");
    refuse(json!({ "gap": 1e300 }), "set_mirror_gap gap must be at most 65536 units");
    ed.call("run_action", json!({ "action": "nudge", "args": { "offset": [0, 0, 48] } }));
    refuse(json!({ "gap": 20 }), "the selected half was slid along its mirror plane, run mirror_again first");
    ed.call("run_action", json!({ "action": "undo" }));
    ed.call("select", json!({ "ids": [source] }));
    ed.call("run_action", json!({ "action": "select_none" }));
    refuse(json!({ "gap": 4 }), "select one half of a mirrored pair first");
    let plain = ed.box_brush([200.0, 0.0, 0.0], [216.0, 16.0, 16.0]);
    ed.call("select", json!({ "ids": [plain] }));
    refuse(json!({ "gap": 4 }), "select one half of a mirrored pair first");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), before + 1, "a refused call leaves no undo step, only the box brush");

    let script = json!([
        { "tool": "select", "args": { "ids": [source] } },
        { "tool": "run_action", "args": { "action": "set_mirror_gap", "args": { "gap": 8 } } }
    ]);
    let ran = ed.call("run_script", json!({ "steps": script }));
    assert_eq!(ran["steps"], 2, "run_script accepts set_mirror_gap {ran}");
    assert_eq!(ed.state()["mirror_planes"][0]["at"], json!(40.0));
}

#[test]
#[ignore]
fn mirror_again_puts_a_slid_half_back() {
    let ed = Editor::launch("mirror_again");
    let source = ed.box_brush([8.0, 2.0, -4.0], [40.0, 34.0, 20.0]);
    ed.call("select", json!({ "ids": [source] }));
    ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "x", "plane": "max" } }));
    let copy = ed.state()["selection"]["nodes"].clone();
    let text = ed.call_err("run_action", json!({ "action": "mirror_again" }));
    assert!(text.contains("nothing selected was slid along its mirror plane"), "{text}");
    assert_eq!(ed.state()["mirror_slides"], json!([]), "a pair on its plane has no slide");
    let pair = ed.state()["mirror_planes"][0].clone();

    ed.call("run_action", json!({ "action": "nudge", "args": { "offset": [0, 0, 16] } }));
    assert_eq!(ed.state()["mirror_planes"], json!([]), "a slid half has no plane");
    let slid = json!([{ "axis": "x", "at": 40.0, "group": pair["group"], "partner": pair["partner"], "slide": [0.0, 0.0, -16.0], "distance": 16.0 }]);
    assert_eq!(ed.state()["mirror_slides"], slid);
    let text = ed.call_err("run_action", json!({ "action": "set_mirror_gap", "args": { "gap": 8 } }));
    assert!(text.contains("slid along its mirror plane, run mirror_again first"), "{text}");
    let undo_before = ed.state()["undo"].as_array().unwrap().len();
    let reply = ed.call("run_action", json!({ "action": "mirror_again" }));
    assert_eq!((reply["moved"].clone(), reply["distance"].clone()), (json!(1), json!(16.0)), "{reply}");
    assert_eq!(reply["plane"], json!({ "axis": "x", "at": 40.0 }), "{reply}");
    assert_eq!(ed.state()["selection"]["nodes"], copy, "the selected half stays selected");
    let zs: Vec<i64> = vertices_of(&ed, source).iter().map(|v| v[2]).collect();
    assert_eq!((zs.iter().min(), zs.iter().max()), (Some(&12000), Some(&36000)), "the original followed the slid copy {zs:?}");
    assert_eq!(ed.state()["mirror_planes"][0]["at"], json!(40.0));
    assert_eq!(ed.state()["mirror_slides"], json!([]), "mirrored again, nothing is slid");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before + 1);
    assert_eq!(ed.state()["undo"][0], "MCP: Mirror Again");

    let text = ed.call_err("run_action", json!({ "action": "mirror_again" }));
    assert!(text.contains("nothing selected was slid along its mirror plane"), "{text}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before + 1, "a refused call leaves no undo step");

    ed.call("run_action", json!({ "action": "undo" }));
    let script = json!([{ "tool": "run_action", "args": { "action": "mirror_again" } }]);
    let ran = ed.call("run_script", json!({ "steps": script }));
    assert_eq!(ran["steps"], 1, "run_script accepts mirror_again {ran}");
    assert_eq!(ed.state()["mirror_planes"][0]["at"], json!(40.0));
}

#[test]
#[ignore]
fn mirror_gap_and_again_refuse_a_locked_copy_and_move_a_hidden_one() {
    let ed = Editor::launch("mirror_locked");
    let source = ed.box_brush([8.0, 2.0, -4.0], [40.0, 34.0, 20.0]);
    ed.call("select", json!({ "ids": [source] }));
    ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "x", "plane": "max" } }));
    let copy = ed.state()["selection"]["nodes"][0].clone();
    // A hidden or locked copy cannot be selected, so its bounds come from the node list.
    let x_of_copy = |ed: &Editor| {
        let groups = ed.call("list_nodes", json!({ "type": "group" }));
        groups["nodes"].as_array().unwrap().iter().find(|n| n["id"] == copy).unwrap()["bounds"]["min"][0].as_f64().unwrap()
    };
    let start = x_of_copy(&ed);

    ed.call("run_action", json!({ "action": "hide_selected" }));
    ed.call("select", json!({ "ids": [source] }));
    ed.call("run_action", json!({ "action": "set_mirror_gap", "args": { "gap": 10 } }));
    assert_eq!(x_of_copy(&ed) - start, 5.0, "a hidden copy moved with its half");

    ed.call("run_action", json!({ "action": "unhide_all" }));
    ed.call("select", json!({ "ids": [copy] }));
    ed.call("run_action", json!({ "action": "lock_selected" }));
    ed.call("select", json!({ "ids": [source] }));
    let undo = ed.state()["undo"].as_array().unwrap().len();
    let text = ed.call_err("run_action", json!({ "action": "set_mirror_gap", "args": { "gap": 30 } }));
    assert!(text.contains("is locked") && text.contains("unlock it"), "{text}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo, "a refused call leaves no undo step");
    assert_eq!(x_of_copy(&ed) - start, 5.0, "nothing moved");
}

#[test]
#[ignore]
fn edits_across_two_linked_copies_are_refused() {
    let ed = Editor::launch("linked_conflict");
    let source = ed.box_brush([8.0, 2.0, -4.0], [40.0, 34.0, 20.0]);
    ed.call("select", json!({ "ids": [source] }));
    let copy = ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "x", "plane": "max" } }))["copies"][0].as_u64().unwrap();
    let brushes = ed.call("list_nodes", json!({ "type": "brush" }));
    let parent_of = |id: u64| brushes["nodes"].as_array().unwrap().iter().find(|n| n["id"] == id).map(|n| n["parent"].as_u64().unwrap());
    let other = brushes["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_u64().unwrap()).find(|i| *i != source).unwrap();
    let half = parent_of(source).unwrap();
    assert_eq!(parent_of(other), Some(copy), "{brushes}");

    ed.call("select", json!({ "ids": [source, other] }));
    let undo = ed.state()["undo"].clone();
    let shapes = (vertices_of(&ed, source), vertices_of(&ed, other));
    let refused = |text: String, how: &str| {
        assert!(text.contains("Linked copies would end up different, so nothing changed"), "{how}: {text}");
        assert_eq!(ed.state()["undo"], undo, "{how}: no undo step");
        assert_eq!((vertices_of(&ed, source), vertices_of(&ed, other)), shapes, "{how}: nothing moved");
    };
    refused(ed.call_err("run_action", json!({ "action": "nudge", "args": { "offset": [16, 0, -16] } })), "nudge");
    let edges = json!([[8, 34, -4], [8, 34, 20], [72, 34, -4], [72, 34, 20]]);
    refused(ed.call_err("run_action", json!({ "action": "move_vertices", "args": { "vertices": edges, "offset": [-4, 0, 0] } })), "move_vertices");
    let keys = json!([{ "type": "move", "world": [24, 0, 8] }, { "type": "key", "key": "ArrowRight" }]);
    refused(ed.call_err("simulate_input", json!({ "target": "top", "events": keys })), "simulate_input");

    let nudge = json!({ "tool": "run_action", "args": { "action": "nudge", "args": { "offset": [16, 0, -16] } } });
    let stopped: Value =
        serde_json::from_str(&ed.call_err("run_script", json!({ "steps": [nudge, { "tool": "select", "args": { "ids": [half] } }] }))).unwrap();
    assert_eq!((stopped["ran"].as_u64(), stopped["errors"].as_array().unwrap().len()), (Some(1), 1), "{stopped}");
    assert_eq!(stopped["errors"][0]["step"], 0);
    refused(stopped["errors"][0]["error"].as_str().unwrap().to_string(), "run_script step");
    let wholes = json!({ "tool": "select", "args": { "ids": [half, copy] } });
    let went_on = ed.call("run_script", json!({ "continue_on_error": true, "steps": [nudge, wholes, nudge] }));
    assert_eq!((went_on["ran"].as_u64(), went_on["errors"].as_array().unwrap().len()), (Some(3), 1), "{went_on}");
    assert!(went_on["errors"][0]["error"].as_str().unwrap().contains("Linked copies would end up different"), "{went_on}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo.as_array().unwrap().len() + 1, "the steps that applied are one undo step");
    ed.call("run_action", json!({ "action": "undo" }));
    ed.call("select", json!({ "ids": [source, other] }));
    assert_eq!(ed.state()["undo"], undo);

    ed.call("select", json!({ "ids": [half, copy] }));
    ed.call("run_action", json!({ "action": "nudge", "args": { "offset": [16, 0, -16] } }));
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo.as_array().unwrap().len() + 1, "both halves moved as wholes agree");
    assert_eq!(ed.state()["undo"][0], "MCP: Move");
}

#[test]
#[ignore]
fn separate_linked_groups_frees_both_halves() {
    let ed = Editor::launch("separate_linked");
    let source = ed.box_brush([8.0, 2.0, -4.0], [40.0, 34.0, 20.0]);
    ed.call("select", json!({ "ids": [source] }));
    let copy = ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "x", "plane": "max" } }))["copies"][0].as_u64().unwrap();
    let listed = |ed: &Editor, id: u64| ed.call("list_nodes", json!({}))["nodes"].as_array().unwrap().iter().find(|n| n["id"] == id).unwrap().clone();
    let half = listed(&ed, source)["parent"].as_u64().unwrap();
    let names = |ed: &Editor| [half, copy].map(|g| listed(ed, g)["name"].as_str().unwrap().to_string());
    assert!(names(&ed).iter().all(|n| n.ends_with("(linked)")), "{:?}", names(&ed));
    let steps = ed.state()["undo"].as_array().unwrap().len();

    for (action, selected) in [("separate_linked_groups", half), ("unlink_groups", copy)] {
        ed.call("select", json!({ "ids": [selected] }));
        let reply = ed.call("run_action", json!({ "action": action }));
        assert_eq!(reply["status"], "Made 2 linked groups plain", "{action}");
        assert_eq!(names(&ed), ["Mirrored", "Mirrored"], "{action}: the partner left alone is freed too");
        assert_eq!(ed.state()["undo"].as_array().unwrap().len(), steps + 1, "{action}");
        assert_eq!(ed.state()["undo"][0], "MCP: Separate Linked Groups", "{action}");
        ed.call("select", json!({ "ids": [half, copy] }));
        assert!(ed.state()["mirror_planes"].as_array().unwrap().is_empty(), "{action}: no mirror is left");

        ed.call("run_action", json!({ "action": "undo" }));
        assert!(names(&ed).iter().all(|n| n.ends_with("(linked)")), "{action}: one undo links both again {:?}", names(&ed));
    }

    let loose = ed.box_brush([200.0, -8.0, 12.0], [232.0, 24.0, 28.0]);
    let steps = ed.state()["undo"].as_array().unwrap().len();
    ed.call("select", json!({ "ids": [loose] }));
    let err = ed.call_err("run_action", json!({ "action": "separate_linked_groups" }));
    assert!(err.contains("Select a linked group first"), "{err}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), steps, "a refused call is no undo step");

    ed.call("select", json!({ "ids": [source] }));
    let err = ed.call_err("run_action", json!({ "action": "separate_linked_groups" }));
    assert!(err.contains("Select a linked group first"), "a brush inside a half frees nothing: {err}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), steps, "and is no undo step");
}

#[test]
#[ignore]
fn select_linked_selects_every_copy() {
    let ed = Editor::launch("select_linked");
    let source = ed.box_brush([-72.0, 6.0, 30.0], [-40.0, 50.0, 46.0]);
    let loose = ed.box_brush([200.0, -8.0, 12.0], [232.0, 24.0, 28.0]);
    ed.call("select", json!({ "ids": [source] }));
    ed.call("run_action", json!({ "action": "duplicate_linked" }));
    let selected = |ed: &Editor| {
        let mut ids: Vec<u64> = ed.state()["selection"]["nodes"].as_array().unwrap().iter().map(|n| n.as_u64().unwrap()).collect();
        ids.sort();
        ids
    };
    let copy = selected(&ed)[0];
    let steps = ed.state()["undo"].as_array().unwrap().len();

    for start in [source, copy] {
        ed.call("select", json!({ "ids": [start] }));
        let reply = ed.call("run_action", json!({ "action": "select_linked" }));
        let groups = selected(&ed);
        assert_eq!(groups.len(), 2, "{start}: {reply}");
        assert_eq!(reply["status"], "Selected 2 linked copies", "{start}");
        assert!(groups.contains(&copy) && !groups.contains(&loose), "{start}: {groups:?}");
        assert_eq!(ed.state()["undo"].as_array().unwrap().len(), steps, "{start}: a selection is no undo step");
    }

    ed.call("select", json!({ "ids": [loose] }));
    let err = ed.call_err("run_action", json!({ "action": "select_linked" }));
    assert!(err.contains("Select a linked group first"), "{err}");
    assert_eq!(selected(&ed), [loose], "a failed call keeps the selection");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), steps);

    ed.call("select", json!({ "ids": [copy] }));
    ed.call("run_action", json!({ "action": "hide_selected" }));
    ed.call("select", json!({ "ids": [source] }));
    let reply = ed.call("run_action", json!({ "action": "select_linked" }));
    assert_eq!(reply["status"], "Selected 1 linked copy", "{reply}");
    assert!(!selected(&ed).contains(&copy), "a hidden copy stays out");
}

#[test]
#[ignore]
fn apply_mirror_joins_brushes_and_welds_meshes() {
    let ed = Editor::launch("apply_mirror");
    let brush = ed.box_brush([8.0, 2.0, -4.0], [40.0, 34.0, 20.0]);
    let mesh = ed.call("create_mesh", json!({ "shape": "cuboid", "min": [8, 2, 100], "max": [40, 34, 124] }))["id"].as_u64().unwrap();
    ed.call("select", json!({ "ids": [brush, mesh] }));

    let apply = |args: Value| ed.call_err("run_action", json!({ "action": "apply_mirror", "args": args }));
    assert!(apply(json!({})).contains("Select one half of a mirrored pair"), "nothing is mirrored yet");

    let mirrored = ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "x", "plane": "max" } }));
    assert_eq!(mirrored["plane"], json!({ "axis": "x", "at": 40.0 }), "{mirrored}");
    assert_eq!(ed.brushes(), 2);
    let undo_before = ed.state()["undo"].as_array().unwrap().len();

    for (distance, words) in [(json!(-0.01), "0 to 16"), (json!(16.01), "0 to 16"), (json!("far"), "must be a number"), (json!([1]), "must be a number")] {
        let text = apply(json!({ "distance": distance }));
        assert!(text.contains("apply_mirror distance") && text.contains(words), "{distance}: {text}");
    }

    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before, "a refused call leaves no undo step");
    assert_eq!(ed.brushes(), 2);

    // The plane sits 1.5 units past each half, so they are 3 apart and a distance of 2.9 misses and 3 meets.
    ed.call("run_action", json!({ "action": "undo" }));
    ed.call("select", json!({ "ids": [brush, mesh] }));
    ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "x", "at": 41.5 } }));
    assert!(apply(json!({ "distance": 2.9 })).contains("do not meet"));
    assert_eq!(ed.brushes(), 2);
    let near = ed.call("run_action", json!({ "action": "apply_mirror", "args": { "distance": 3 } }));
    assert_eq!(near["joined"], 0, "boxes with a gap stay apart {near}");
    assert_eq!(ed.brushes(), 2);
    ed.call("run_action", json!({ "action": "undo" }));
    ed.call("run_action", json!({ "action": "undo" }));

    ed.call("select", json!({ "ids": [brush, mesh] }));
    ed.call("run_action", json!({ "action": "mirror_linked", "args": { "axis": "x", "plane": "max" } }));
    let undo_before = ed.state()["undo"].as_array().unwrap().len();
    let reply = ed.call("run_action", json!({ "action": "apply_mirror", "args": { "distance": 16 } }));
    assert_eq!((reply["joined"].as_u64(), reply["welded"].as_u64()), (Some(1), Some(1)), "{reply}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before + 1, "one undo step");
    assert_eq!(ed.state()["undo"][0], "MCP: Apply Mirror");
    let groups = reply["groups"].as_array().unwrap().clone();
    assert_eq!(groups.len(), 1, "{reply}");
    assert_eq!(reply["selection"], json!(groups), "the merged group is selected");
    assert_eq!(reply["replaced"].as_object().unwrap().len(), 3, "the copy group, brush and mesh were replaced {reply}");
    assert!(reply["replaced"].as_object().unwrap().values().all(|v| v.as_array().unwrap().len() == 1), "{reply}");

    assert_eq!(ed.brushes(), 1, "the half box and its mirror joined into one brush");
    assert_eq!(ed.state()["map"]["meshes"], 1);
    let ids = |kind: &str| -> Vec<u64> {
        ed.call("list_nodes", json!({ "type": kind }))["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_u64().unwrap()).collect()
    };
    let joined = ids("brush")[0];
    assert_eq!(vertices_of(&ed, joined).len(), 8, "one box, not two");
    let (min, max) = {
        let v = vertices_of(&ed, joined);
        (
            [v.iter().map(|p| p[0]).min().unwrap(), v.iter().map(|p| p[2]).min().unwrap()],
            [v.iter().map(|p| p[0]).max().unwrap(), v.iter().map(|p| p[2]).max().unwrap()],
        )
    };
    assert_eq!((min, max), ([8000, -4000], [72000, 20000]));

    let welded = ids("mesh")[0];
    let node = ed.call("get_node", json!({ "id": welded }));
    assert_eq!(node["faces"].as_array().unwrap().len(), 10, "the two cap faces on the plane are gone");
    assert_eq!(vertices_of(&ed, welded).len(), 12, "the seam vertices are shared");
    let check = ed.call("mesh_edit", json!({ "id": welded, "op": "weld", "distance": 0.001 }));
    assert_eq!((check["closed"].as_bool(), check["merged"].as_u64()), (Some(true), Some(0)), "one closed mesh {check}");

    let group = ed.call("get_node", json!({ "id": groups[0] }));
    assert!(group["link_id"].is_null(), "the merged group is no longer linked {group}");
    assert!(ed.state()["mirror_planes"].as_array().unwrap().is_empty());
    assert_eq!(ed.call("validate_map", json!({}))["count"], 0);
}

#[test]
#[ignore]
fn vertex_symmetry_moves_the_mirrored_corner() {
    let ed = Editor::launch("vertex_symmetry");
    let a = ed.box_brush([10.0, 5.0, -30.0], [90.0, 45.0, 20.0]);
    ed.call("select", json!({ "ids": [a] }));
    let before = vertices_of(&ed, a);
    assert_eq!(ed.state()["editor"]["edit_symmetry"]["axis"], Value::Null, "off to start with");

    ed.call("set_editor", json!({ "edit_symmetry": "x" }));
    let set = ed.state()["editor"]["edit_symmetry"].clone();
    assert_eq!(set, json!({ "axis": "x", "plane": "center", "at": 0.0 }), "{set}");

    let undo_before = ed.state()["undo"].as_array().unwrap().len();
    let reply = ed.call("run_action", json!({ "action": "move_vertices", "args": { "vertices": [[10, 45, 20]], "offset": [3, -6, 2] } }));
    assert_eq!(reply["mirrored"], 1, "{reply}");
    let moved = vertices_of(&ed, a);
    for want in [[13000, 39000, 22000], [87000, 39000, 22000]] {
        assert!(moved.contains(&want), "no corner at {want:?} in {moved:?}");
    }

    assert!(!moved.contains(&[10000, 45000, 20000]) && !moved.contains(&[90000, 45000, 20000]), "both old corners are gone {moved:?}");
    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo_before + 1, "the pair is one undo step");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(vertices_of(&ed, a), before);

    // Front and back corners overlap in the front view, and either one has a counterpart across x 50.
    ed.call("run_action", json!({ "action": "set_tool", "args": { "tool": "vertex" } }));
    ed.call("set_camera", json!({ "view": "front", "center": [50, 25, 0], "zoom": 2.0 }));
    ed.input("front", json!([{ "type": "drag", "world": [90, 45, 20], "to_world": [122, 45, 20] }]));
    let status = ed.state()["status"].as_str().unwrap().to_string();
    assert!(status.contains("Moved 1 mirrored vertex too"), "a drag says its counterpart moved: {status}");
    let dragged = vertices_of(&ed, a);
    let at = |x: i64| [20000, -30000].iter().any(|z| dragged.contains(&[x, 45000, *z]));
    assert!(at(122000) && at(-22000), "the corner and its counterpart moved apart {dragged:?}");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(vertices_of(&ed, a), before);
    ed.call("set_editor", json!({ "tool": "select" }));

    ed.call("set_editor", json!({ "edit_symmetry": "off" }));
    let alone = ed.call("run_action", json!({ "action": "move_vertices", "args": { "vertices": [[10, 45, 20]], "offset": [3, -6, 2] } }));
    assert_eq!(alone["mirrored"], 0, "{alone}");
    let one = vertices_of(&ed, a);
    assert!(one.contains(&[13000, 39000, 22000]) && one.contains(&[90000, 45000, 20000]), "only the picked corner moved {one:?}");
    ed.call("run_action", json!({ "action": "undo" }));

    let refused = ed.call_err("set_editor", json!({ "edit_symmetry": { "axis": "x", "plane": "max" } }));
    assert!(refused.contains("edit_symmetry plane must be center, world or custom"), "{refused}");
    assert_eq!(ed.state()["editor"]["edit_symmetry"]["axis"], Value::Null, "a refused value changes nothing");

    // The second box is thin in z, so the mirrored pull dents it and the first box must stay put as well.
    let c = ed.box_brush([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]);
    let b = ed.box_brush([80.0, 0.0, 0.0], [144.0, 64.0, 16.0]);
    ed.call("select", json!({ "ids": [c, b] }));
    let (c_before, b_before) = (vertices_of(&ed, c), vertices_of(&ed, b));
    let pull = json!({ "action": "move_vertices", "args": { "vertices": [[64, 64, 0]], "offset": [-24, -24, 10] } });
    ed.call("set_editor", json!({ "edit_symmetry": "off" }));
    assert_eq!(ed.call("run_action", pull.clone())["mirrored"], 0, "alone the first box takes the pull");
    ed.call("run_action", json!({ "action": "undo" }));

    ed.call("set_editor", json!({ "edit_symmetry": "x" }));
    let why = ed.call_err("run_action", pull);
    assert!(why.contains("concave"), "{why}");
    assert_eq!((vertices_of(&ed, c), vertices_of(&ed, b)), (c_before, b_before), "a refused symmetric move changes no brush");
}

/// A mesh box off the origin whose back top edge was lowered, so no two corners overlap in the front view.
fn slanted_mesh(ed: &Editor) -> u64 {
    let id = ed.call("create_mesh", json!({ "shape": "cuboid", "min": [10, 5, -20], "max": [65, 40, 30] }))["id"].as_u64().unwrap();
    let back_top: Vec<usize> = mesh_vertices(ed, id).iter().enumerate().filter(|(_, v)| v[1] == 40.0 && v[2] == -20.0).map(|(i, _)| i).collect();
    ed.call("mesh_edit", json!({ "id": id, "op": "translate", "verts": back_top, "offset": [0, -20, 0] }));
    id
}

fn mesh_vertices(ed: &Editor, id: u64) -> Vec<[f64; 3]> {
    let node = ed.call("get_node", json!({ "id": id }));
    node["vertices"].as_array().unwrap().iter().map(|v| [0, 1, 2].map(|i| v[i].as_f64().unwrap())).collect()
}

fn vertex_near(vertices: &[[f64; 3]], p: [f64; 3]) -> [f64; 3] {
    *vertices.iter().find(|v| (0..3).all(|i| (v[i] - p[i]).abs() < 0.01)).unwrap_or_else(|| panic!("no vertex near {p:?} in {vertices:?}"))
}

#[test]
#[ignore]
fn mesh_symmetry_drag_moves_the_mirrored_vertex() {
    let ed = Editor::launch("mesh_symmetry_drag");
    let id = slanted_mesh(&ed);
    let before = mesh_vertices(&ed, id);
    ed.call("select", json!({ "ids": [id] }));
    ed.call("set_editor", json!({ "tool": "mesh", "mesh_component": "vertex", "edit_symmetry": "x" }));
    ed.call("set_camera", json!({ "view": "front", "center": [37, 22, 0], "zoom": 2.0 }));
    ed.input("front", json!([{ "type": "click", "world": [65, 40, 30] }]));
    let selected = ed.state()["editor"]["mesh_selection"][0]["verts"].clone();
    assert_eq!(selected.as_array().unwrap().len(), 1, "one corner picked {selected}");

    let undo = ed.state()["undo"].as_array().unwrap().len();
    ed.input("front", json!([{ "type": "drag", "world": [65, 40, 30], "to_world": [97, 40, 30] }]));
    let after = mesh_vertices(&ed, id);
    let moved = vertex_near(&after, [65.0 + 32.0, 40.0, 30.0]);
    let twin = vertex_near(&after, [10.0 - 32.0, 40.0, 30.0]);
    assert_eq!((moved[1], moved[2], twin[1], twin[2]), (40.0, 30.0, 40.0, 30.0));
    let unchanged = before.iter().filter(|v| (v[0], v[1], v[2]) != (65.0, 40.0, 30.0) && (v[0], v[1], v[2]) != (10.0, 40.0, 30.0));
    for v in unchanged {
        vertex_near(&after, *v);
    }

    assert_eq!(ed.state()["undo"].as_array().unwrap().len(), undo + 1, "the drag and its mirror are one undo step");
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(mesh_vertices(&ed, id), before);

    ed.call("set_editor", json!({ "edit_symmetry": "off" }));
    ed.input("front", json!([{ "type": "drag", "world": [65, 40, 30], "to_world": [97, 40, 30] }]));
    let alone = mesh_vertices(&ed, id);
    vertex_near(&alone, [97.0, 40.0, 30.0]);
    vertex_near(&alone, [10.0, 40.0, 30.0]);
}

#[test]
#[ignore]
fn mesh_edit_translate_follows_the_symmetry_when_asked() {
    let ed = Editor::launch("mesh_translate_symmetry");
    let id = slanted_mesh(&ed);
    let before = mesh_vertices(&ed, id);
    let corner = before.iter().position(|v| *v == [65.0, 40.0, 30.0]).unwrap();
    let args = |symmetry: Value| json!({ "id": id, "op": "translate", "verts": [corner], "offset": [3, 2, -1], "symmetry": symmetry });

    let err = ed.call_err("mesh_edit", args(json!(true)));
    assert!(err.contains("edit_symmetry"), "symmetry needs the setting on: {err}");
    assert_eq!(mesh_vertices(&ed, id), before, "a refused translate changes nothing");

    ed.call("set_editor", json!({ "edit_symmetry": "x" }));
    for shape in [json!("x"), json!({ "axis": "x" }), json!(1), json!("true"), json!([true])] {
        let err = ed.call_err("mesh_edit", args(shape.clone()));
        assert!(err.contains("true or false"), "symmetry {shape} is neither a boolean nor null: {err}");
        assert_eq!(mesh_vertices(&ed, id), before, "a refused translate changes nothing");
    }

    let plain = ed.call("mesh_edit", json!({ "id": id, "op": "translate", "verts": [corner], "offset": [3, 2, -1] }));
    assert!(plain["mirrored"].is_null(), "without symmetry the setting is ignored {plain}");
    vertex_near(&mesh_vertices(&ed, id), [10.0, 40.0, 30.0]);
    ed.call("run_action", json!({ "action": "undo" }));

    let reply = ed.call("mesh_edit", args(json!(true)));
    assert_eq!(reply["mirrored"], 1, "{reply}");
    let after = mesh_vertices(&ed, id);
    vertex_near(&after, [68.0, 42.0, 29.0]);
    vertex_near(&after, [7.0, 42.0, 29.0]);
    ed.call("run_action", json!({ "action": "undo" }));
    assert_eq!(mesh_vertices(&ed, id), before, "one undo step takes both back");

    let all: Vec<usize> = (0..before.len()).collect();
    let whole = ed.call("mesh_edit", json!({ "id": id, "op": "translate", "verts": all, "offset": [3, 2, -1], "symmetry": true }));
    assert_eq!(whole["mirrored"], 0, "a selected counterpart moves plainly {whole}");
    vertex_near(&mesh_vertices(&ed, id), [13.0, 42.0, 29.0]);
}

// Where the axis menu sits in the Vertex tool's bar, from its left edge, and where its rows lie below the bar's middle.
const AXIS_MENU_X: f64 = 150.0;
const PLANE_MENU_X: f64 = 380.0;
const MENU_FIRST_ROW: f64 = 31.0;
const MENU_ROW: f64 = 29.0;

#[test]
#[ignore]
fn the_vertex_bar_picks_the_symmetry_axis_and_plane() {
    let ed = Editor::launch("vertex_symmetry_bar");
    let a = ed.box_brush([10.0, 5.0, -30.0], [90.0, 45.0, 20.0]);
    ed.call("select", json!({ "ids": [a] }));
    ed.call("run_action", json!({ "action": "set_tool", "args": { "tool": "vertex" } }));
    ed.input("window", json!([{ "type": "move", "x": 10, "y": 10 }]));
    let bar: Vec<f64> = ed.state()["ui"]["tool_options"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let y = (bar[1] + bar[3]) / 2.0;
    let x = bar[0] + AXIS_MENU_X;
    let axis = |ed: &Editor| ed.state()["editor"]["edit_symmetry"].clone();
    assert_eq!(axis(&ed)["axis"], Value::Null);

    ed.input("window", json!([{ "type": "click", "x": x, "y": y }]));
    ed.input("window", json!([{ "type": "move", "x": x, "y": y + 2.0 }]));
    ed.input("window", json!([{ "type": "move", "x": x, "y": y + 3.0 }]));
    ed.screenshot("window", "menu_open");
    ed.input("window", json!([{ "type": "click", "x": x, "y": y + MENU_FIRST_ROW + MENU_ROW * 2.0 }]));
    assert_eq!(axis(&ed)["axis"], "y", "the second row of the axis menu after off and X is Y: {}", axis(&ed));
    ed.screenshot("window", "plane_label");

    let pick = |x: f64, row: f64| {
        ed.input("window", json!([{ "type": "click", "x": x, "y": y }]));
        ed.input("window", json!([{ "type": "move", "x": x, "y": y + 2.0 }]));
        ed.input("window", json!([{ "type": "move", "x": x, "y": y + 3.0 }]));
        ed.input("window", json!([{ "type": "click", "x": x, "y": y + MENU_FIRST_ROW + MENU_ROW * row }]));
    };
    pick(bar[0] + PLANE_MENU_X, 2.0);
    let custom = axis(&ed);
    assert_eq!(
        (&custom["axis"], &custom["plane"], &custom["at"]),
        (&json!("y"), &json!("custom"), &json!(25.0)),
        "custom starts at the middle of the brush: {custom}"
    );
    pick(bar[0] + PLANE_MENU_X, 1.0);
    assert_eq!(axis(&ed)["plane"], "world");
    ed.screenshot("window", "plane_world");
}

/// The tool row lists the brush tools first, then a wider gap, then the terrain and paint tools.
#[test]
#[ignore]
fn the_tool_row_puts_brush_tools_before_terrain_and_paint_tools() {
    let ed = Editor::launch("tool_row_families");
    let state = ed.state();
    let buttons: Vec<(String, String, Vec<f64>)> = state["ui"]["tool_buttons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["tool"].as_str().unwrap().to_string(),
                b["family"].as_str().unwrap().to_string(),
                b["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect(),
            )
        })
        .collect();
    assert_eq!(buttons.len(), 17, "{buttons:?}");

    let terrain: Vec<&str> = buttons.iter().filter(|b| b.1 == "terrain").map(|b| b.0.as_str()).collect();
    assert_eq!(terrain, ["Sculpt", "Blend", "Paint", "Scatter"]);
    assert!(buttons.iter().filter(|b| b.1 == "brush").all(|b| !terrain.contains(&b.0.as_str())));
    assert_eq!(buttons.iter().filter(|b| b.1 == "brush").count(), 13);
    assert!(buttons.iter().all(|b| b.1 == "brush" || b.1 == "terrain"), "{buttons:?}");

    let last_brush = buttons.iter().filter(|b| b.1 == "brush").map(|b| b.2[2]).fold(f64::MIN, f64::max);
    let first_terrain = buttons.iter().filter(|b| b.1 == "terrain").map(|b| b.2[0]).fold(f64::MAX, f64::min);
    assert!(buttons.iter().filter(|b| b.1 == "brush").all(|b| b.2[2] <= first_terrain), "a brush tool sits right of a terrain tool: {buttons:?}");
    let separator = first_terrain - last_brush;

    let y_center = |b: &(String, String, Vec<f64>)| (b.2[1] + b.2[3]) / 2.0;
    assert!(buttons.iter().all(|b| (y_center(b) - y_center(&buttons[0])).abs() < 1.0), "the tool row wrapped: {buttons:?}");

    let mut inside = 0.0_f64;
    for pair in buttons.windows(2) {
        if pair[0].1 == pair[1].1 {
            inside = inside.max(pair[1].2[0] - pair[0].2[2]);
        }
    }

    assert!(inside > 0.0 && separator > inside, "separator {separator} is not wider than the widest gap inside a family {inside}");
    assert_eq!(buttons.first().unwrap().0, "Select");
    assert_eq!(buttons.iter().position(|b| b.0 == "Texture").map(|i| i < 12), Some(true), "Texture sits with the brush tools");
}

/// A fresh layout opens only the panels the block out, texture, entities and play loop needs.
#[test]
#[ignore]
fn a_fresh_start_opens_only_the_loop_panels() {
    let ed = Editor::launch("fresh_panels");
    let state = ed.state();
    let mut panels: Vec<(String, bool)> =
        state["ui"]["panels"].as_array().unwrap().iter().map(|p| (p["panel"].as_str().unwrap().to_string(), p["shown"].as_bool().unwrap())).collect();
    panels.sort();
    let expected = [("Entities", false), ("Inspector", true), ("Issues", false), ("Materials", true), ("Outliner", true)].map(|(n, s)| (n.to_string(), s));
    assert_eq!(panels, expected);
}

/// Entering the Texture tool brings the Inspector tab to the front, whatever tool or panel was in front before.
#[test]
#[ignore]
fn the_texture_tool_brings_the_inspector_to_the_front() {
    let ed = Editor::launch("texture_inspector");
    let shown = |name: &str| ed.state()["ui"]["panels"].as_array().unwrap().iter().any(|p| p["panel"] == name && p["shown"] == true);
    ed.box_brush([8.0, -16.0, 0.0], [72.0, 48.0, 40.0]);
    ed.call("set_editor", json!({ "tool": "scatter" }));
    ed.call("get_state", json!({}));
    assert!(shown("Scatter") && !shown("Inspector"), "the Scatter tool puts its panel in front of the Inspector");
    ed.call("set_editor", json!({ "tool": "texture" }));
    ed.call("get_state", json!({}));
    assert!(shown("Inspector") && !shown("Scatter"));
}

fn rect_of(value: &Value) -> Vec<f64> {
    value.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()
}

fn center_of(value: &Value) -> (f64, f64) {
    let r = rect_of(value);
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

/// The first start asks for a keymap preset on the empty map card, a click on a row applies it.
#[test]
#[ignore]
fn the_first_run_card_applies_a_keymap_preset() {
    let ed = Editor::launch("first_run_pick");
    let state = ed.state();
    let card = &state["ui"]["first_run"];
    let presets: Vec<(&str, bool)> =
        card["presets"].as_array().unwrap().iter().map(|p| (p["preset"].as_str().unwrap(), p["selected"].as_bool().unwrap())).collect();
    assert_eq!(presets, [("trenchbroom", true), ("hammer", false), ("blender", false)]);
    assert_eq!(state["editor"]["keymap"], "trenchbroom");
    // The camera's rect is x, y, width, height, the card's is min and max corners.
    let view = rect_of(&state["cameras"].as_array().unwrap().iter().find(|c| c["view"] == "3d").unwrap()["rect"]);
    let at = rect_of(&card["rect"]);
    assert!(at[0] >= view[0] && at[1] >= view[1] && at[2] <= view[0] + view[2] && at[3] <= view[1] + view[3], "the card {at:?} lies in the 3D view {view:?}");

    let hammer = card["presets"].as_array().unwrap().iter().find(|p| p["preset"] == "hammer").unwrap()["rect"].clone();
    let (x, y) = center_of(&hammer);
    ed.input("window", json!([{ "type": "click", "x": x, "y": y }]));
    let state = ed.state();
    assert_eq!(state["editor"]["keymap"], "hammer");
    assert_eq!(state["status"], "Keymap preset set to Hammer");
    assert!(state["ui"]["first_run"].is_null(), "the card goes once a preset is picked");
    assert_eq!(state["map"]["brushes"], 0);
}

/// Skip ends the question and leaves the keymap as it was.
#[test]
#[ignore]
fn the_first_run_card_skips_without_changing_the_keymap() {
    let ed = Editor::launch("first_run_skip");
    let state = ed.state();
    assert!(state["ui"]["first_run"].is_object());
    let before = state["status"].clone();
    let (x, y) = center_of(&state["ui"]["first_run"]["skip"]);
    ed.input("window", json!([{ "type": "click", "x": x, "y": y }]));
    let state = ed.state();
    assert_eq!(state["editor"]["keymap"], "trenchbroom");
    assert_eq!(state["status"], before, "skip says nothing");
    assert!(state["ui"]["first_run"].is_null());
}

/// Every file under `dir` as (path below it, bytes).
fn file_tree(dir: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn walk(base: &std::path::Path, at: &std::path::Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(at).into_iter().flatten().flatten() {
            if entry.path().is_dir() {
                walk(base, &entry.path(), out);
            } else {
                out.insert(entry.path().strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read(entry.path()).unwrap());
            }
        }
    }

    let mut out = Default::default();
    walk(dir, dir, &mut out);
    out
}

/// A project that has the addon's core definitions and the Gameplay pack template, as the addon leaves it before the pack
/// is installed.
fn override_project(name: &str) -> PathBuf {
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    let dir = linked_project(name, json!([]));
    add_pack_template(&dir);
    let addon = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../godot/addons/func_godot");
    copy(&addon.join("fgd"), &dir.join("addons/func_godot/fgd"));
    copy(&addon.join("src/trenchdot/entities"), &dir.join("addons/func_godot/src/trenchdot/entities"));
    dir
}

const OVERRIDE_SETTINGS: [(&str, &str); 2] = [("func_godot", "default_map_settings"), ("trenchdot", "game_config")];

/// Override in project through MCP writes the copy and its script, builds the project FGD, map settings and game
/// config, points project.godot at them while Godot is closed, never touches the addon, and refuses a repeat.
#[test]
#[ignore]
fn override_entity_wires_a_project_that_godot_has_closed() {
    let dir = override_project("override_closed");
    let ed = Editor::launch_with("override_closed", &["--project", dir.to_str().unwrap()]);
    ed.call("project_content", json!({ "install": "gameplay" }));
    let addons = file_tree(&dir.join("addons"));

    let door = ed.call("override_entity", json!({ "classname": "func_door" }));
    assert_eq!(door["definition"], "res://entities/func_door.tres");
    assert_eq!(door["script"], "res://entities/func_door.gd");
    assert_eq!(door["fgd"], "res://entities/project_fgd.tres");
    assert_eq!(door["godot"], "not open");
    let wired = json!({ "func_godot/default_map_settings": "res://entities/project_map_settings.tres", "trenchdot/game_config": "res://entities/project_game_config.tres" });
    assert_eq!(door["project_settings"], wired);
    let written =
        ["func_door.tres", "func_door.gd", "project_fgd.tres", "project_map_settings.tres", "project_game_config.tres"].map(|f| format!("res://entities/{f}"));
    assert_eq!(door["written"], json!(written), "{door}");

    let read = |rel: &str| std::fs::read_to_string(dir.join(rel)).unwrap();
    let definition = read("entities/func_door.tres");
    assert!(definition.contains("classname = \"func_door\"") && definition.contains("path=\"res://entities/func_door.gd\""), "{definition}");
    let script = read("entities/func_door.gd");
    assert!(script.contains("extends \"res://trenchdot/entities/scripts/td_door.gd\""), "{script}");
    let fgd = read("entities/project_fgd.tres");
    assert!(fgd.contains("trenchdot_default_fgd.tres") && fgd.contains("func_door.tres"), "{fgd}");
    let godot = read("project.godot");
    for (section, key) in OVERRIDE_SETTINGS {
        let value = wired[format!("{section}/{key}")].as_str().unwrap();
        assert_eq!(td_editor::godot_settings::get(&godot, section, key).as_deref(), Some(value), "{godot}");
    }

    let player = ed.call("override_entity", json!({ "first_person_player": true }));
    assert_eq!(player["classname"], "info_player_start");
    assert_eq!(player["written"], json!(["res://entities/info_player_start.tres", "res://entities/info_player_start.gd", "res://entities/project_fgd.tres"]));
    assert_eq!(player["project_settings"], json!({}), "the settings are set once");
    assert_eq!(read("entities/info_player_start.gd"), td_editor::entity_override::PLAYER_SCRIPT);
    assert!(read("entities/info_player_start.tres").contains("node_class = \"CharacterBody3D\""));
    let fgd = read("entities/project_fgd.tres");
    assert!(fgd.contains("func_door.tres") && fgd.contains("info_player_start.tres"), "{fgd}");
    assert_eq!(read("project.godot"), godot);
    assert_eq!(file_tree(&dir.join("addons")), addons, "nothing under addons changes");

    let again = ed.call_err("override_entity", json!({ "classname": "func_door" }));
    assert!(again.contains("func_door is overridden already. Change it in res://entities/func_door.tres and res://entities/func_door.gd"), "{again}");
    assert_eq!(read("entities/project_fgd.tres"), fgd);
    assert!(ed.call_err("override_entity", json!({ "first_person_player": true, "classname": "light" })).contains("info_player_start only"));
    assert!(ed.call_err("override_entity", json!({})).contains("classname required"));
    assert!(ed.call_err("override_entity", json!({ "classname": "my_unknown" })).contains("no definition to copy"));

    let scripts = dir.join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    let script = json!({ "steps": [{ "tool": "override_entity", "args": { "classname": "light" } }] });
    std::fs::write(scripts.join("override.json"), script.to_string()).unwrap();
    let summary = ed.call("run_script", json!({ "path": scripts.join("override.json"), "continue_on_error": true }));
    assert_eq!(summary["errors"].as_array().map(Vec::len), Some(1), "{summary}");
    assert!(!dir.join("entities/light.tres").exists(), "a script cannot override");
}

/// With Godot holding the project open the editor sends the project settings first and leaves project.godot to it.
#[test]
#[ignore]
fn override_entity_has_an_open_godot_take_the_settings() {
    let dir = override_project("override_open");
    let (godot, sent) = fake_godot_editor_with(&dir, &[], json!({ "project_settings": {}, "scan": {} }));
    let ed = Editor::launch_with("override_open", &["--project", dir.to_str().unwrap()]);
    ed.call("set_editor", json!({ "live_link_port": godot }));
    wait_state(&ed, "the live link", |s| s["godot"]["project_open"] == true);
    let before = std::fs::read_to_string(dir.join("project.godot")).unwrap();

    let player = ed.call("override_entity", json!({ "first_person_player": true }));
    assert_eq!(player["godot"], "told");
    let events: Vec<String> = sent.lock().unwrap().iter().map(|m| m["event"].as_str().unwrap().to_string()).collect();
    assert_eq!(events, ["project_settings", "scan"], "settings first, then the files are scanned");
    let settings = &sent_events(&sent, "project_settings")[0]["settings"];
    assert_eq!(
        settings,
        &json!({ "func_godot/default_map_settings": "res://entities/project_map_settings.tres", "trenchdot/game_config": "res://entities/project_game_config.tres" })
    );
    assert_eq!(std::fs::read_to_string(dir.join("project.godot")).unwrap(), before, "Godot writes project.godot itself");
    assert!(dir.join("entities/info_player_start.gd").is_file() && dir.join("entities/project_game_config.tres").is_file());

    // What Godot does after it took the settings.
    let saved = OVERRIDE_SETTINGS
        .iter()
        .zip(settings.as_object().unwrap().values())
        .fold(before, |text, ((section, key), value)| td_editor::godot_settings::with_setting(&text, section, key, value.as_str().unwrap()));
    std::fs::write(dir.join("project.godot"), saved).unwrap();
    ed.call("override_entity", json!({ "classname": "light" }));
    assert_eq!(sent_events(&sent, "project_settings").len(), 1, "nothing more to set");
    assert_eq!(sent_events(&sent, "scan").len(), 2);
}

/// An addon from before the project_settings event cannot take the settings, so nothing is written.
#[test]
#[ignore]
fn override_entity_asks_to_close_a_godot_with_an_old_addon() {
    let dir = override_project("override_old_addon");
    let (godot, sent) = fake_godot_editor_with(&dir, &[], json!({}));
    let ed = Editor::launch_with("override_old_addon", &["--project", dir.to_str().unwrap()]);
    ed.call("set_editor", json!({ "live_link_port": godot }));
    wait_state(&ed, "the live link", |s| s["godot"]["project_open"] == true);
    let before = file_tree(&dir);

    let e = ed.call_err("override_entity", json!({ "classname": "light" }));
    assert_eq!(e, "Close Godot first. It has this project open and its addon cannot take new project settings.");
    assert_eq!(file_tree(&dir), before);
    assert_eq!(sent_events(&sent, "project_settings").len(), 1);
    assert!(sent_events(&sent, "scan").is_empty());
}
