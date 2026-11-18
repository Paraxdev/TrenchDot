//! docs/editor/shortcuts.md against the keymap: every chord the page lists is bound in the preset it names, every chord a
//! preset takes over is on the page, and the mouse rows name the gestures the Select tool really reads.

use std::collections::BTreeSet;
use td_editor::commands::{Action, parse_shortcut, preset_bindings, shortcut_to_text};
use td_editor::mouse_map::{Bound, ClickRule, chord_label, mouse_rows};

struct Table {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn page() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/editor/shortcuts.md")).expect("docs/editor/shortcuts.md")
}

fn tables() -> Vec<Table> {
    let text = page();
    let mut tables: Vec<Table> = Vec::new();
    let mut open = false;
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            open = false;
            continue;
        }

        let cells: Vec<String> = line.trim_matches('|').split('|').map(|c| c.trim().replace('`', "")).collect();
        if cells.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ' ')) {
            continue;
        }

        if open {
            tables.last_mut().unwrap().rows.push(cells);
        } else {
            tables.push(Table { header: cells, rows: Vec::new() });
            open = true;
        }
    }

    tables
}

fn table(column: &str) -> Table {
    tables().into_iter().find(|t| t.header.iter().any(|h| h == column)).unwrap_or_else(|| panic!("no table with a {column} column"))
}

/// Keys the page lists that the keymap does not hold: nudges and the clipboard, which the window handles as events.
const UNBOUND: [&str; 6] = ["Arrows", "Page Up", "Page Down", "Ctrl+X", "Ctrl+C", "Ctrl+V"];

/// Bound, but explained in the text above the tables because it steps through several things.
const IN_PROSE: [&str; 1] = ["Escape"];

/// The chords in one key cell as normalized text, "Ctrl+1 to 9" expanded. A cell that names a click has none.
fn chords(cell: &str) -> Vec<String> {
    if cell.to_ascii_lowercase().contains("click") {
        return Vec::new();
    }

    let mut out = Vec::new();
    for part in cell.replace(" and ", ", ").replace(", or ", ", ").replace(" or ", ", ").split(", ") {
        let part = part.trim();
        if UNBOUND.contains(&part) {
            continue;
        }

        if let Some((from, to)) = part.split_once(" to ") {
            let (prefix, first) = from.split_at(from.len() - 1);
            let (first, last): (u8, u8) = (first.parse().unwrap(), to.parse().unwrap());
            for digit in first..=last {
                out.push(chord_text(&format!("{prefix}{digit}")));
            }

            continue;
        }

        out.push(chord_text(part));
    }

    out
}

fn chord_text(text: &str) -> String {
    shortcut_to_text(&parse_shortcut(text).unwrap_or_else(|| panic!("the page lists {text:?}, which is not a chord")))
}

fn bound(preset: &str) -> BTreeSet<String> {
    preset_bindings(preset).iter().map(|(s, _)| shortcut_to_text(s)).collect()
}

fn listed(tables: &[Table]) -> BTreeSet<String> {
    tables.iter().flat_map(|t| t.rows.iter()).flat_map(|r| chords(&r[0])).collect()
}

fn base_tables() -> Vec<Table> {
    tables().into_iter().filter(|t| t.header[0] == "Key" && !t.header[1].ends_with("preset")).collect()
}

#[test]
fn every_chord_in_the_main_tables_is_bound_in_the_trenchbroom_preset() {
    let bound = bound("trenchbroom");
    let missing: Vec<String> = listed(&base_tables()).into_iter().filter(|c| !bound.contains(c)).collect();
    assert!(missing.is_empty(), "listed but not bound: {missing:?}");
}

#[test]
fn every_trenchbroom_chord_is_on_the_page() {
    let listed = listed(&base_tables());
    let missing: Vec<String> = bound("trenchbroom").into_iter().filter(|c| !listed.contains(c) && !IN_PROSE.contains(&c.as_str())).collect();
    assert!(missing.is_empty(), "bound but not on the page: {missing:?}");
}

#[test]
fn each_preset_table_lists_what_the_preset_changes() {
    let base = preset_bindings("trenchbroom");
    for (preset, header) in [("hammer", "Hammer preset"), ("blender", "Blender preset")] {
        let listed = listed(&[table(header)]);
        let bound = bound(preset);
        let stray: Vec<&String> = listed.iter().filter(|c| !bound.contains(*c)).collect();
        assert!(stray.is_empty(), "{preset} lists chords it does not bind: {stray:?}");

        let changed: BTreeSet<String> =
            preset_bindings(preset).iter().filter(|(s, a)| !base.iter().any(|(bs, ba)| bs == s && ba == a)).map(|(s, _)| shortcut_to_text(s)).collect();
        let missing: Vec<&String> = changed.iter().filter(|c| !listed.contains(*c)).collect();
        assert!(missing.is_empty(), "{preset} changes chords the page does not list: {missing:?}");
    }
}

#[test]
fn the_selection_chords_are_where_trenchbroom_has_them() {
    let at = |chord: &str, preset: &str| preset_bindings(preset).into_iter().find(|(s, _)| shortcut_to_text(s) == chord).map(|(_, a)| a);
    assert_eq!(at("Ctrl+Shift+A", "trenchbroom"), Some(Action::SelectNone));
    assert_eq!(at("Ctrl+E", "trenchbroom"), Some(Action::SelectInside));
    assert_eq!(at("Ctrl+Shift+A", "hammer"), Some(Action::SelectBrushFaces));
    assert_eq!(at("Ctrl+Shift+T", "trenchbroom"), None);
}

fn mouse_cells(rows: &[Vec<String>]) -> Vec<String> {
    rows.iter().flat_map(|r| r[0].replace(", or ", ", ").replace(" or ", ", ").split(", ").map(|c| c.trim().to_ascii_lowercase()).collect::<Vec<_>>()).collect()
}

#[test]
fn the_mouse_table_names_every_click_the_select_tool_reads() {
    let cells = mouse_cells(&table("Input").rows);
    for rule in [ClickRule::Clear, ClickRule::Keep, ClickRule::ReplaceObject, ClickRule::ToggleObject, ClickRule::ReplaceFace, ClickRule::ToggleFace] {
        let label = chord_label("trenchbroom", rule).to_ascii_lowercase().replace("cmd+", "ctrl+");
        assert!(cells.contains(&label), "the mouse table has no {label:?} row");
    }
}

#[test]
fn the_mouse_table_names_every_double_click_of_the_views() {
    let cells = mouse_cells(&table("Input").rows);
    let rows: Vec<_> = mouse_rows("trenchbroom").into_iter().filter(|r| matches!(r.bound, Some(Bound::View(..)))).collect();
    assert_eq!(rows.len(), 3, "open a group, select an entity, close a group");
    for row in rows {
        let label = row.gesture.to_ascii_lowercase();
        assert!(cells.contains(&label), "the mouse table has no {label:?} row");
    }
}

#[test]
fn the_blender_mouse_table_names_every_click_of_its_modes() {
    let cells = mouse_cells(&table("Blender input").rows);
    for rule in [
        ClickRule::Clear,
        ClickRule::Keep,
        ClickRule::ReplaceObject,
        ClickRule::ToggleObject,
        ClickRule::ReplaceFace,
        ClickRule::ToggleFace,
        ClickRule::FaceModeMiss,
    ] {
        let label = chord_label("blender", rule).replace("Cmd+", "Ctrl+");
        for cell in mouse_cells(&[vec![label.clone()]]) {
            assert!(cells.contains(&cell), "the Blender mouse table has no {label:?} row");
        }
    }
}

#[test]
fn the_page_no_longer_has_the_uv_tab_click() {
    let text = page();
    assert!(!text.contains("UV Editor](uv-editor.md) is the visible tab"));
}

#[test]
fn the_page_names_the_preset_row_as_the_window_labels_it() {
    let text = page();
    assert!(!text.contains("Keys like"), "the Keyboard Shortcuts window labels the row Preset");
    assert!(text.contains("**Preset** row"));
}
