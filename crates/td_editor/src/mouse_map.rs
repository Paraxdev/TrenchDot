//! What a click in the Select tool does to the selection, decided in one place.

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
}
