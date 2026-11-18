//! What a click in the Select tool does to the selection, decided in one place.

use crate::i18n::{en, tr, trf};
use crate::state::SelectMode;

/// What lies under the cursor. A face is a brush or mesh face, every other object counts as an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickHit {
    None,
    Object,
    Face,
}

/// The change a click makes to the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickRule {
    Clear,
    Keep,
    ReplaceObject,
    ToggleObject,
    ReplaceFace,
    ToggleFace,
    /// Blender Face mode on something that has no faces: nothing changes.
    FaceModeMiss,
}

/// The TrenchBroom rules, which the Hammer preset shares. Ctrl (Cmd on a Mac) toggles instead of replacing, Shift on a
/// face works on the face alone, and Shift on anything without faces behaves like a plain click. The Blender preset
/// follows its mode instead: Shift and Ctrl both toggle, objects in Object mode and faces in Face mode.
pub fn click_rule(preset: &str, mode: SelectMode, command: bool, shift: bool, hit: ClickHit) -> ClickRule {
    if preset == "blender" {
        return blender_rule(mode, command || shift, hit);
    }

    match hit {
        ClickHit::None if command => ClickRule::Keep,
        ClickHit::None => ClickRule::Clear,
        ClickHit::Face if shift => {
            if command {
                ClickRule::ToggleFace
            } else {
                ClickRule::ReplaceFace
            }
        }
        ClickHit::Object | ClickHit::Face => {
            if command {
                ClickRule::ToggleObject
            } else {
                ClickRule::ReplaceObject
            }
        }
    }
}

/// The gesture that makes `rule` in `preset`, in the current language, for hints and the Mouse tab. Ctrl reads Cmd on a
/// Mac. The Blender preset names the mode a gesture belongs to, the others have no modes.
pub fn chord_label(preset: &str, rule: ClickRule) -> String {
    let command = if cfg!(target_os = "macos") { "Cmd+" } else { "Ctrl+" };
    let click = |mods: &str| if mods.is_empty() { tr("Click").to_string() } else { trf!("{mods}click", mods = mods) };
    let in_mode = |click: String, mode: &str| trf!("{click} ({mode})", click = click, mode = tr(mode));
    if preset == "blender" {
        return match rule {
            ClickRule::Clear => trf!("{click} on empty space", click = click("")),
            ClickRule::Keep => trf!("{click} on empty space", click = click("Shift+")),
            ClickRule::ReplaceObject => in_mode(click(""), en!("Object mode")),
            ClickRule::ToggleObject => in_mode(click("Shift+"), en!("Object mode")),
            ClickRule::ReplaceFace => in_mode(click(""), en!("Face mode")),
            ClickRule::ToggleFace => in_mode(click("Shift+"), en!("Face mode")),
            ClickRule::FaceModeMiss => trf!("{click} on something without faces ({mode})", click = click(""), mode = tr("Face mode")),
        };
    }

    match rule {
        ClickRule::Clear => trf!("{click} on empty space", click = click("")),
        ClickRule::Keep => trf!("{click} on empty space", click = click(command)),
        ClickRule::ReplaceObject => click(""),
        ClickRule::ToggleObject => click(command),
        ClickRule::ReplaceFace => click("Shift+"),
        ClickRule::ToggleFace => click(&format!("{command}Shift+")),
        ClickRule::FaceModeMiss => trf!("{click} on something without faces", click = click("")),
    }
}

fn blender_rule(mode: SelectMode, toggle: bool, hit: ClickHit) -> ClickRule {
    match (hit, mode) {
        (ClickHit::None, _) if toggle => ClickRule::Keep,
        (ClickHit::None, _) => ClickRule::Clear,
        (ClickHit::Object, SelectMode::Face) => ClickRule::FaceModeMiss,
        (ClickHit::Face, SelectMode::Face) if toggle => ClickRule::ToggleFace,
        (ClickHit::Face, SelectMode::Face) => ClickRule::ReplaceFace,
        _ if toggle => ClickRule::ToggleObject,
        _ => ClickRule::ReplaceObject,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Click,
    RightClick,
    Drag,
    Wheel,
}

/// A mouse button with the modifiers held. `command` is Ctrl, or Cmd on a Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub button: Button,
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Chord {
    pub fn new(button: Button, m: egui::Modifiers) -> Self {
        Self { button, command: m.command, shift: m.shift, alt: m.alt }
    }
}

/// What a mouse gesture on a face does in the Texture tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureGesture {
    Select,
    Toggle,
    Pick,
    Apply,
    ApplyReset,
    PasteAlignment,
    Wrap,
    Slide,
    Scale,
    Rotate,
}

/// The Texture tool's gestures. Shift only refines a step, except on a right click where it pastes the alignment.
pub fn texture_gesture(chord: Chord) -> Option<TextureGesture> {
    let Chord { command, shift, alt, .. } = chord;
    Some(match chord.button {
        Button::Click if alt => TextureGesture::Pick,
        Button::Click if command => TextureGesture::Toggle,
        Button::Click => TextureGesture::Select,
        Button::RightClick if alt => TextureGesture::Wrap,
        Button::RightClick if shift => TextureGesture::PasteAlignment,
        Button::RightClick if command => TextureGesture::ApplyReset,
        Button::RightClick => TextureGesture::Apply,
        Button::Drag if alt => return None,
        Button::Drag => TextureGesture::Slide,
        Button::Wheel if command => TextureGesture::Scale,
        Button::Wheel if alt => TextureGesture::Rotate,
        Button::Wheel => return None,
    })
}

/// What a click on an Outliner row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowClick {
    Replace,
    Toggle,
    Range,
    AddRange,
}

pub fn row_click(command: bool, shift: bool) -> RowClick {
    match (command, shift) {
        (false, false) => RowClick::Replace,
        (true, false) => RowClick::Toggle,
        (false, true) => RowClick::Range,
        (true, true) => RowClick::AddRange,
    }
}

/// The handler decision a fixed Mouse tab row stands for, checked against [`texture_gesture`] and [`row_click`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    Texture(Chord, TextureGesture),
    Outliner(Chord, RowClick),
}

/// One line of the Mouse tab in Keyboard Shortcuts. `context` and `what` are English keys, shown through `tr`.
#[derive(Clone, Debug, PartialEq)]
pub struct MouseRow {
    pub context: &'static str,
    pub gesture: String,
    pub what: &'static str,
    /// The click rule the row stands for.
    pub rule: Option<ClickRule>,
    /// The Texture tool or Outliner decision the row stands for. The view drags have neither, the shortcut docs test
    /// checks those against the manual instead.
    pub bound: Option<Bound>,
}

const VIEWS: &str = en!("3D and 2D views");
const TEXTURE_TOOL: &str = en!("Texture Tool");
const OUTLINER: &str = en!("Outliner");

/// The mouse gestures of `preset`, grouped by where they apply. The click rows come from [`chord_label`] and
/// [`click_rule`], so they cannot drift from what a click does.
pub fn mouse_rows(preset: &str) -> Vec<MouseRow> {
    let mut rules = vec![ClickRule::ReplaceObject, ClickRule::ToggleObject, ClickRule::ReplaceFace, ClickRule::ToggleFace];
    if preset == "blender" {
        rules.push(ClickRule::FaceModeMiss);
    }

    rules.extend([ClickRule::Clear, ClickRule::Keep]);
    let fixed = |context: &'static str, gesture: &'static str, what: &'static str| MouseRow {
        context,
        gesture: tr(gesture).to_string(),
        what,
        rule: None,
        bound: None,
    };
    let chord = |button, mods: &str| Chord { button, command: mods.contains('c'), shift: mods.contains('s'), alt: mods.contains('a') };
    let texture = |button, mods: &str, gesture: &'static str, what: &'static str, g| MouseRow {
        bound: Some(Bound::Texture(chord(button, mods), g)),
        ..fixed(TEXTURE_TOOL, gesture, what)
    };
    let outliner = |mods: &str, gesture: &'static str, what: &'static str, g| MouseRow {
        bound: Some(Bound::Outliner(chord(Button::Click, mods), g)),
        ..fixed(OUTLINER, gesture, what)
    };
    let mut rows: Vec<MouseRow> = rules
        .into_iter()
        .map(|rule| MouseRow { context: VIEWS, gesture: chord_label(preset, rule), what: rule_what(rule), rule: Some(rule), bound: None })
        .collect();
    rows.extend([
        fixed(VIEWS, en!("Double click"), en!("Select the whole group")),
        fixed(VIEWS, en!("Click again soon at the same spot, or Alt+click (2D)"), en!("Select the next object under the cursor")),
        fixed(VIEWS, en!("Drag off the selection"), en!("Draw a brush")),
        fixed(VIEWS, en!("Drag the selection"), en!("Move it. Ctrl copies, Shift locks the axis")),
        fixed(VIEWS, en!("Shift+drag a selected face (3D)"), en!("Push or pull the face")),
        fixed(VIEWS, en!("Ctrl+Shift+drag a selected face (3D)"), en!("Extrude a new brush")),
        fixed(VIEWS, en!("Right drag (3D)"), en!("Look around, WASD flies, Q and E go down and up")),
        fixed(VIEWS, en!("Middle drag"), en!("Pan")),
        fixed(VIEWS, en!("Alt+left drag (3D)"), en!("Orbit")),
        texture(Button::Click, "", en!("Click a face"), en!("Select the face"), TextureGesture::Select),
        texture(Button::Click, "c", en!("Ctrl+click a face"), en!("Add the face to the selection or remove it"), TextureGesture::Toggle),
        texture(Button::Click, "a", en!("Alt+click a face"), en!("Pick its material and alignment"), TextureGesture::Pick),
        texture(Button::RightClick, "", en!("Right click a face"), en!("Apply the current material"), TextureGesture::Apply),
        texture(Button::RightClick, "c", en!("Ctrl+right click a face"), en!("Apply the current material and reset the alignment"), TextureGesture::ApplyReset),
        texture(Button::RightClick, "s", en!("Shift+right click a face"), en!("Paste the picked alignment"), TextureGesture::PasteAlignment),
        texture(
            Button::RightClick,
            "a",
            en!("Alt+right click a face"),
            en!("Wrap the current material from the selected face across the edge"),
            TextureGesture::Wrap,
        ),
        texture(Button::Drag, "", en!("Drag a selected face"), en!("Slide the texture"), TextureGesture::Slide),
        texture(Button::Wheel, "c", en!("Ctrl+wheel"), en!("Scale the texture, Shift in finer steps"), TextureGesture::Scale),
        texture(Button::Wheel, "a", en!("Alt+wheel"), en!("Rotate the texture, Shift in 1 degree steps"), TextureGesture::Rotate),
        outliner("", en!("Click a row"), en!("Select it"), RowClick::Replace),
        outliner("c", en!("Ctrl+click a row"), en!("Add it to the selection or remove it"), RowClick::Toggle),
        outliner("s", en!("Shift+click a row"), en!("Select the rows from the last clicked one"), RowClick::Range),
        outliner("cs", en!("Ctrl+Shift+click a row"), en!("Add the rows from the last clicked one"), RowClick::AddRange),
    ]);
    rows
}

fn rule_what(rule: ClickRule) -> &'static str {
    match rule {
        ClickRule::Clear => en!("Clear the selection"),
        ClickRule::Keep => en!("Keep the selection"),
        ClickRule::ReplaceObject => en!("Select the object under the cursor"),
        ClickRule::ToggleObject => en!("Add the object to the selection or remove it"),
        ClickRule::ReplaceFace => en!("Select the face under the cursor, replacing the selection"),
        ClickRule::ToggleFace => en!("Add the face to the selection or remove it"),
        ClickRule::FaceModeMiss => en!("Nothing changes, it has no faces to pick"),
    }
}

#[cfg(test)]
mod tests {
    use super::ClickRule::*;
    use super::*;

    const HITS: [ClickHit; 3] = [ClickHit::None, ClickHit::Object, ClickHit::Face];

    #[test]
    fn every_modifier_and_hit_has_its_rule() {
        let cases = [
            // (command, shift, hit, rule)
            (false, false, ClickHit::None, Clear),
            (false, false, ClickHit::Object, ReplaceObject),
            (false, false, ClickHit::Face, ReplaceObject),
            (true, false, ClickHit::None, Keep),
            (true, false, ClickHit::Object, ToggleObject),
            (true, false, ClickHit::Face, ToggleObject),
            (false, true, ClickHit::None, Clear),
            (false, true, ClickHit::Object, ReplaceObject),
            (false, true, ClickHit::Face, ReplaceFace),
            (true, true, ClickHit::None, Keep),
            (true, true, ClickHit::Object, ToggleObject),
            (true, true, ClickHit::Face, ToggleFace),
        ];
        for preset in ["trenchbroom", "hammer"] {
            for mode in [SelectMode::Object, SelectMode::Face] {
                for (command, shift, hit, rule) in cases {
                    assert_eq!(click_rule(preset, mode, command, shift, hit), rule, "{preset} {mode:?} command {command} shift {shift} {hit:?}");
                }
            }
        }
    }

    #[test]
    fn blender_object_mode_toggles_objects_with_shift_or_ctrl() {
        let cases = [
            // (command, shift, hit, rule)
            (false, false, ClickHit::None, Clear),
            (false, false, ClickHit::Object, ReplaceObject),
            (false, false, ClickHit::Face, ReplaceObject),
            (true, false, ClickHit::None, Keep),
            (true, false, ClickHit::Object, ToggleObject),
            (true, false, ClickHit::Face, ToggleObject),
            (false, true, ClickHit::None, Keep),
            (false, true, ClickHit::Object, ToggleObject),
            (false, true, ClickHit::Face, ToggleObject),
            (true, true, ClickHit::None, Keep),
            (true, true, ClickHit::Object, ToggleObject),
            (true, true, ClickHit::Face, ToggleObject),
        ];
        for (command, shift, hit, rule) in cases {
            assert_eq!(click_rule("blender", SelectMode::Object, command, shift, hit), rule, "command {command} shift {shift} {hit:?}");
        }
    }

    #[test]
    fn blender_face_mode_picks_faces_and_leaves_objects_without_faces_alone() {
        let cases = [
            // (command, shift, hit, rule)
            (false, false, ClickHit::None, Clear),
            (false, false, ClickHit::Object, FaceModeMiss),
            (false, false, ClickHit::Face, ReplaceFace),
            (true, false, ClickHit::None, Keep),
            (true, false, ClickHit::Object, FaceModeMiss),
            (true, false, ClickHit::Face, ToggleFace),
            (false, true, ClickHit::None, Keep),
            (false, true, ClickHit::Object, FaceModeMiss),
            (false, true, ClickHit::Face, ToggleFace),
            (true, true, ClickHit::None, Keep),
            (true, true, ClickHit::Object, FaceModeMiss),
            (true, true, ClickHit::Face, ToggleFace),
        ];
        for (command, shift, hit, rule) in cases {
            assert_eq!(click_rule("blender", SelectMode::Face, command, shift, hit), rule, "command {command} shift {shift} {hit:?}");
        }
    }

    #[test]
    fn only_the_blender_preset_reads_the_mode() {
        for preset in ["trenchbroom", "hammer", "unknown"] {
            for (command, shift, hit) in HITS.iter().flat_map(|h| [(false, false, *h), (true, false, *h), (false, true, *h), (true, true, *h)]) {
                assert_eq!(
                    click_rule(preset, SelectMode::Object, command, shift, hit),
                    click_rule(preset, SelectMode::Face, command, shift, hit),
                    "{preset} command {command} shift {shift} {hit:?}"
                );
            }
        }
    }

    const COMMAND: &str = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };

    #[test]
    fn mouse_rows_match_click_rule() {
        for preset in ["trenchbroom", "hammer", "blender"] {
            let rows = mouse_rows(preset);
            let mut reachable = Vec::new();
            for mode in [SelectMode::Object, SelectMode::Face] {
                for (command, shift, hit) in [false, true].into_iter().flat_map(|c| [false, true].into_iter().flat_map(move |s| HITS.map(|h| (c, s, h)))) {
                    let rule = click_rule(preset, mode, command, shift, hit);
                    if !reachable.contains(&rule) {
                        reachable.push(rule);
                    }
                }
            }

            let shown: Vec<ClickRule> = rows.iter().filter_map(|r| r.rule).collect();
            for rule in &reachable {
                assert_eq!(shown.iter().filter(|r| *r == rule).count(), 1, "{preset}: {rule:?} has one row");
            }

            for row in &rows {
                if let Some(rule) = row.rule {
                    assert!(reachable.contains(&rule), "{preset}: no click makes {rule:?}");
                    assert_eq!(row.gesture, chord_label(preset, rule), "{preset}: {rule:?}");
                }
            }

            for (i, row) in rows.iter().enumerate() {
                assert!(!rows[..i].iter().any(|r| r.context == row.context && r.gesture == row.gesture), "{preset}: {:?} twice", row.gesture);
            }
        }

        assert_eq!(mouse_rows("trenchbroom").iter().filter(|r| r.rule.is_some()).count(), 6);
        assert_eq!(mouse_rows("blender").iter().filter(|r| r.rule.is_some()).count(), 7, "Face mode adds the miss");
    }

    fn chords(button: Button) -> Vec<Chord> {
        (0..8).map(|b| Chord { button, command: b & 1 != 0, shift: b & 2 != 0, alt: b & 4 != 0 }).collect()
    }

    fn chord_prefix(chord: Chord) -> String {
        let mods = [(chord.command, "Ctrl+"), (chord.shift, "Shift+"), (chord.alt, "Alt+")];
        let mods: String = mods.iter().filter(|(on, _)| *on).map(|(_, m)| *m).collect();
        let button = match chord.button {
            Button::Click => "click",
            Button::RightClick => "right click",
            Button::Drag => "drag",
            Button::Wheel => "wheel",
        };
        let label = format!("{mods}{button}");
        if mods.is_empty() { label[..1].to_uppercase() + &label[1..] } else { label }
    }

    #[test]
    fn texture_and_outliner_rows_match_their_handlers() {
        let rows = mouse_rows("trenchbroom");
        let mut texture = Vec::new();
        let mut outliner = Vec::new();
        for row in &rows {
            match row.bound {
                Some(Bound::Texture(chord, g)) => {
                    assert_eq!(texture_gesture(chord), Some(g), "{}", row.gesture);
                    assert!(row.gesture.starts_with(&chord_prefix(chord)), "{} is not {:?}", row.gesture, chord);
                    texture.push(g);
                }
                Some(Bound::Outliner(chord, click)) => {
                    assert_eq!(row_click(chord.command, chord.shift), click, "{}", row.gesture);
                    assert!(row.gesture.starts_with(&chord_prefix(chord)), "{} is not {:?}", row.gesture, chord);
                    outliner.push(click);
                }
                None => assert!(row.context == VIEWS, "{} has no handler to check", row.gesture),
            }
        }

        for button in [Button::Click, Button::RightClick, Button::Drag, Button::Wheel] {
            for g in chords(button).into_iter().filter_map(texture_gesture) {
                assert!(texture.contains(&g), "no Texture tool row for {g:?}");
            }
        }

        for chord in chords(Button::Click) {
            assert!(outliner.contains(&row_click(chord.command, chord.shift)));
        }

        for preset in ["hammer", "blender"] {
            let other: Vec<_> = mouse_rows(preset).into_iter().filter(|r| r.bound.is_some()).collect();
            assert_eq!(other, rows.iter().filter(|r| r.bound.is_some()).cloned().collect::<Vec<_>>(), "{preset}");
        }
    }

    #[test]
    fn the_next_object_row_names_alt_click() {
        for preset in ["trenchbroom", "hammer", "blender"] {
            let rows = mouse_rows(preset);
            let row = rows.iter().find(|r| r.what == "Select the next object under the cursor").unwrap();
            assert!(row.gesture.contains("Alt+click"), "{preset}: {}", row.gesture);
        }
    }

    #[test]
    fn chord_labels_name_the_gesture_of_each_preset() {
        let tb = [
            (Clear, "Click on empty space".to_string()),
            (Keep, format!("{COMMAND}+click on empty space")),
            (ReplaceObject, "Click".to_string()),
            (ToggleObject, format!("{COMMAND}+click")),
            (ReplaceFace, "Shift+click".to_string()),
            (ToggleFace, format!("{COMMAND}+Shift+click")),
            (FaceModeMiss, "Click on something without faces".to_string()),
        ];
        for preset in ["trenchbroom", "hammer", "unknown"] {
            for (rule, label) in &tb {
                assert_eq!(&chord_label(preset, *rule), label, "{preset} {rule:?}");
            }
        }

        let blender = [
            (Clear, "Click on empty space"),
            (Keep, "Shift+click on empty space"),
            (ReplaceObject, "Click (Object mode)"),
            (ToggleObject, "Shift+click (Object mode)"),
            (ReplaceFace, "Click (Face mode)"),
            (ToggleFace, "Shift+click (Face mode)"),
            (FaceModeMiss, "Click on something without faces (Face mode)"),
        ];
        for (rule, label) in blender {
            assert_eq!(chord_label("blender", rule), label, "blender {rule:?}");
        }
    }

    #[test]
    fn chord_labels_of_one_preset_are_all_different() {
        for preset in ["trenchbroom", "blender"] {
            let labels: std::collections::BTreeSet<String> =
                [Clear, Keep, ReplaceObject, ToggleObject, ReplaceFace, ToggleFace, FaceModeMiss].iter().map(|r| chord_label(preset, *r)).collect();
            assert_eq!(labels.len(), 7, "{preset}");
        }
    }

    #[test]
    fn chord_labels_use_the_words_of_the_language() {
        let _de = crate::i18n::scope(crate::i18n::Lang::De);
        assert_eq!(chord_label("trenchbroom", ReplaceFace), "Shift+Klick");
        assert_eq!(chord_label("trenchbroom", ReplaceObject), "Klick");
        let face = chord_label("blender", ReplaceFace);
        assert!(face.starts_with("Klick (") && face.contains("Flächenmodus") && !face.contains("click"), "{face}");
    }
}
