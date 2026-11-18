//! The keys the tools read themselves, listed in Keyboard Shortcuts. A drift test in `toolset.rs` presses every row
//! through `ToolSet::keys`, so a key added to a tool without a row, or a row without a key, fails there.

use egui::{Key, KeyboardShortcut, Modifiers};

use crate::commands;
use crate::i18n::{en, tr};
use crate::tools::ToolKind;

/// What has to be going on before the tool reads the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    Always,
    ClipPoint,
    PolygonPoint,
    VertexSelected,
    PathStarted,
    FacesInReach,
    MeshSelected,
    MeshGizmo,
    MeshTransform,
    MeshLoopCut,
}

impl When {
    pub fn label(self) -> Option<&'static str> {
        match self {
            When::Always => None,
            When::ClipPoint => Some(en!("once a point is placed")),
            When::PolygonPoint => Some(en!("once a corner is placed")),
            When::VertexSelected => Some(en!("with vertices selected")),
            When::PathStarted => Some(en!("while a path is being drawn")),
            When::FacesInReach => Some(en!("with faces or brushes selected")),
            When::MeshSelected => Some(en!("with vertices, edges or faces selected")),
            When::MeshGizmo => Some(en!("while the gizmo shows")),
            When::MeshTransform => Some(en!("while moving, rotating or scaling")),
            When::MeshLoopCut => Some(en!("while cutting a loop")),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ToolKeyRow {
    pub tool: ToolKind,
    pub key: Key,
    pub modifiers: Modifiers,
    pub what: &'static str,
    pub when: When,
}

impl ToolKeyRow {
    pub fn chord(&self) -> String {
        commands::shortcut_to_text(&KeyboardShortcut::new(self.modifiers, self.key))
    }

    pub fn what(&self) -> &'static str {
        tr(self.what)
    }
}

const NONE: Modifiers = Modifiers::NONE;
const SHIFT: Modifiers = Modifiers::SHIFT;
const CTRL: Modifiers = Modifiers::COMMAND;
const ALT: Modifiers = Modifiers::ALT;
const CTRL_SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::COMMAND };

const fn row(tool: ToolKind, key: Key, modifiers: Modifiers, what: &'static str, when: When) -> ToolKeyRow {
    ToolKeyRow { tool, key, modifiers, what, when }
}

/// Tools in the order of the toolbar, keys in the order the tool reads them. The Mesh tool shadows the same keys of
/// the Select menu (Ctrl+A, A, Alt+A, 1, 2, 3) while it is the active tool.
pub const TOOL_KEYS: &[ToolKeyRow] = &[
    row(ToolKind::Polygon, Key::Enter, NONE, en!("Close the outline and extrude it"), When::Always),
    row(ToolKind::Polygon, Key::Backspace, NONE, en!("Remove the last corner"), When::PolygonPoint),
    row(ToolKind::Polygon, Key::Escape, NONE, en!("Discard the outline"), When::PolygonPoint),
    row(ToolKind::Clip, Key::Tab, NONE, en!("Switch the kept side: front, back, both"), When::Always),
    row(ToolKind::Clip, Key::Enter, NONE, en!("Apply the clip"), When::Always),
    row(ToolKind::Clip, Key::Backspace, NONE, en!("Remove the last point"), When::ClipPoint),
    row(ToolKind::Vertex, Key::Delete, NONE, en!("Delete the selected vertices"), When::VertexSelected),
    row(ToolKind::Mesh, Key::Num1, NONE, en!("Vertex mode"), When::Always),
    row(ToolKind::Mesh, Key::Num2, NONE, en!("Edge mode"), When::Always),
    row(ToolKind::Mesh, Key::Num3, NONE, en!("Face mode"), When::Always),
    row(ToolKind::Mesh, Key::A, NONE, en!("Select all in the current mode"), When::Always),
    row(ToolKind::Mesh, Key::A, CTRL, en!("Select all in the current mode"), When::Always),
    row(ToolKind::Mesh, Key::A, ALT, en!("Select none"), When::Always),
    row(ToolKind::Mesh, Key::R, CTRL, en!("Cut a loop around the mesh"), When::Always),
    row(ToolKind::Mesh, Key::K, NONE, en!("Knife"), When::Always),
    row(ToolKind::Mesh, Key::G, NONE, en!("Grab"), When::MeshSelected),
    row(ToolKind::Mesh, Key::R, NONE, en!("Rotate"), When::MeshSelected),
    row(ToolKind::Mesh, Key::S, NONE, en!("Scale"), When::MeshSelected),
    row(ToolKind::Mesh, Key::E, NONE, en!("Extrude"), When::MeshSelected),
    row(ToolKind::Mesh, Key::I, NONE, en!("Inset faces"), When::MeshSelected),
    row(ToolKind::Mesh, Key::B, CTRL, en!("Bevel edges"), When::MeshSelected),
    row(ToolKind::Mesh, Key::B, CTRL_SHIFT, en!("Bevel vertices"), When::MeshSelected),
    row(ToolKind::Mesh, Key::M, NONE, en!("Merge at center"), When::MeshSelected),
    row(ToolKind::Mesh, Key::F, NONE, en!("Fill a face"), When::MeshSelected),
    row(ToolKind::Mesh, Key::X, NONE, en!("Delete the selection"), When::MeshSelected),
    row(ToolKind::Mesh, Key::Delete, NONE, en!("Delete the selection"), When::MeshSelected),
    row(ToolKind::Mesh, Key::D, SHIFT, en!("Duplicate and grab"), When::MeshSelected),
    row(ToolKind::Mesh, Key::P, NONE, en!("Separate into its own mesh"), When::MeshSelected),
    row(ToolKind::Mesh, Key::F, ALT, en!("Flip the faces"), When::MeshSelected),
    row(ToolKind::Mesh, Key::L, CTRL, en!("Select linked"), When::MeshSelected),
    row(ToolKind::Mesh, Key::Escape, NONE, en!("Hide the gizmo"), When::MeshGizmo),
    row(ToolKind::Mesh, Key::Enter, NONE, en!("Confirm"), When::MeshTransform),
    row(ToolKind::Mesh, Key::Escape, NONE, en!("Cancel"), When::MeshTransform),
    row(ToolKind::Mesh, Key::X, NONE, en!("Lock to the X axis, again to release"), When::MeshTransform),
    row(ToolKind::Mesh, Key::Y, NONE, en!("Lock to the Y axis, again to release"), When::MeshTransform),
    row(ToolKind::Mesh, Key::Z, NONE, en!("Lock to the Z axis, again to release"), When::MeshTransform),
    row(ToolKind::Mesh, Key::X, SHIFT, en!("Move in the plane without X"), When::MeshTransform),
    row(ToolKind::Mesh, Key::Y, SHIFT, en!("Move in the plane without Y"), When::MeshTransform),
    row(ToolKind::Mesh, Key::Z, SHIFT, en!("Move in the plane without Z"), When::MeshTransform),
    row(ToolKind::Mesh, Key::Backspace, NONE, en!("Erase the last typed digit"), When::MeshTransform),
    row(ToolKind::Mesh, Key::Plus, NONE, en!("One more cut"), When::MeshLoopCut),
    row(ToolKind::Mesh, Key::Minus, NONE, en!("One cut fewer"), When::MeshLoopCut),
    row(ToolKind::Texture, Key::ArrowLeft, NONE, en!("Slide the texture one grid step"), When::FacesInReach),
    row(ToolKind::Texture, Key::ArrowRight, NONE, en!("Slide the texture one grid step"), When::FacesInReach),
    row(ToolKind::Texture, Key::ArrowUp, NONE, en!("Slide the texture one grid step"), When::FacesInReach),
    row(ToolKind::Texture, Key::ArrowDown, NONE, en!("Slide the texture one grid step"), When::FacesInReach),
    row(ToolKind::Texture, Key::ArrowLeft, SHIFT, en!("Slide the texture by one unit"), When::FacesInReach),
    row(ToolKind::Texture, Key::ArrowRight, SHIFT, en!("Slide the texture by one unit"), When::FacesInReach),
    row(ToolKind::Texture, Key::ArrowUp, SHIFT, en!("Slide the texture by one unit"), When::FacesInReach),
    row(ToolKind::Texture, Key::ArrowDown, SHIFT, en!("Slide the texture by one unit"), When::FacesInReach),
    row(ToolKind::Path, Key::Enter, NONE, en!("Finish the path"), When::PathStarted),
    row(ToolKind::Path, Key::Escape, NONE, en!("Finish the path"), When::PathStarted),
];

pub fn tool_key_rows() -> &'static [ToolKeyRow] {
    TOOL_KEYS
}

/// The text a row shows beside its chord, with the condition when it has one.
pub fn describe(row: &ToolKeyRow) -> String {
    match row.when.label() {
        Some(when) => format!("{} ({})", row.what(), tr(when)),
        None => row.what().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_unique_per_tool_chord_and_condition() {
        for (i, a) in TOOL_KEYS.iter().enumerate() {
            for b in &TOOL_KEYS[i + 1..] {
                assert!(!(a.tool == b.tool && a.key == b.key && a.modifiers == b.modifiers && a.when == b.when), "{:?} {}", a.tool, a.chord());
            }
        }
    }

    #[test]
    fn a_chord_reads_the_way_the_keymap_writes_it() {
        let find = |tool, key, modifiers| TOOL_KEYS.iter().find(|r| r.tool == tool && r.key == key && r.modifiers == modifiers).unwrap().chord();
        assert_eq!(find(ToolKind::Mesh, Key::B, CTRL_SHIFT), "Ctrl+Shift+B");
        assert_eq!(find(ToolKind::Texture, Key::ArrowLeft, SHIFT), "Shift+Left");
        assert_eq!(find(ToolKind::Mesh, Key::Num1, NONE), "1");
    }
}
