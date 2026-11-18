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
