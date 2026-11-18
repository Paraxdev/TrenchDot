//! What a click in the Select tool does to the selection, decided in one place.

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
}

/// The TrenchBroom rules. Ctrl (Cmd on a Mac) toggles instead of replacing, Shift on a face works on the face alone, and
/// Shift on anything without faces behaves like a plain click.
pub fn click_rule(command: bool, shift: bool, hit: ClickHit) -> ClickRule {
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

#[cfg(test)]
mod tests {
    use super::ClickRule::*;
    use super::*;

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
        for (command, shift, hit, rule) in cases {
            assert_eq!(click_rule(command, shift, hit), rule, "command {command} shift {shift} {hit:?}");
        }
    }
}
