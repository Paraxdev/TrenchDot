//! The message at the start of the status bar and the bell before it. The bell swings once when a command fails and
//! keeps a dot until the user looks at the status bar, so a failure that a later message replaced is not missed.
//! Clicking it lists recent messages.

use egui::{Color32, Rect, RichText, Sense, Stroke, Ui, Vec2};

use crate::i18n::{en, tr};
use crate::state::{EditorState, STATUS_RING_SECS};
use crate::theme::{Palette, blend};

const RECENT: &str = en!("Recent messages");
const NO_MESSAGES: &str = en!("No messages yet");
/// A message is drawn bright on a faint pill for this long, then dimmed.
const FRESH_SECS: f32 = 6.0;
/// The pill is its tint at this alpha, plus [`RING_ALPHA`] while the bell rings for the message.
const PILL_ALPHA: f32 = 0.07;
const RING_ALPHA: f32 = 0.18;

/// Draws the bell and the status message, and marks a failure seen once the user looks at either.
pub fn show(ui: &mut Ui, state: &mut EditorState) {
    let (looked, clicked) = bell(ui, state);
    let looked = message(ui, state) || looked;
    if looked || clicked {
        state.status_seen(clicked);
    }
}

/// Draws the bell and its list. Returns whether the user looked at it, and whether they clicked it.
fn bell(ui: &mut Ui, state: &EditorState) -> (bool, bool) {
    let c = crate::theme::colors();
    let alert = state.status_alert_age();
    let ringing = alert.filter(|t| *t < STATUS_RING_SECS);
    let size = crate::icons::SMALL + 2.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size + 6.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr(RECENT)));
    let response = response.on_hover_text(tr(RECENT));
    let warning = c.readable(c.warning(), c.chrome);
    let tint = if alert.is_some() {
        warning
    } else if response.hovered() {
        c.fg
    } else {
        c.fg.gamma_multiply(0.6)
    };

    let icon = if ringing.is_some() { crate::icons::BELL_RING } else { crate::icons::BELL };
    let angle = ringing.map_or(0.0, swing);
    icon.image(size).tint(tint).rotate(angle, Vec2::new(0.5, 0.1)).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(size)));
    if let Some(t) = ringing {
        let p = t / STATUS_RING_SECS;
        ui.painter().circle_stroke(rect.center(), size * (0.5 + 0.6 * p), Stroke::new(1.5, warning.gamma_multiply(1.0 - p)));
        ui.ctx().request_repaint();
    }

    if alert.is_some() {
        ui.painter().circle_filled(rect.right_top() + Vec2::new(-4.0, 4.0), 3.0, warning);
    }

    let open = egui::Popup::menu(&response).width(360.0).show(|ui| {
        if state.status_log().is_empty() {
            ui.weak(tr(NO_MESSAGES));
        }

        let failed = c.readable_at(c.warning(), ui.visuals().window_fill, 4.5);
        for entry in state.status_log().iter().rev() {
            let color = if entry.failed { failed } else { c.fg };
            ui.add(egui::Label::new(RichText::new(entry.text()).color(color)).wrap());
        }
    });
    (response.hovered() || open.is_some(), response.clicked())
}

/// Draws the status message, truncated to the width left. Returns whether the user looked at it.
fn message(ui: &mut Ui, state: &EditorState) -> bool {
    let c = crate::theme::colors();
    let age = state.status_age();
    let fresh = age < FRESH_SECS;
    let (color, tint) = message_colors(c, state.status_failed(), fresh);
    let pill = ui.painter().add(egui::Shape::Noop);
    let text = state.status_shown();
    let label = ui.add(egui::Label::new(RichText::new(text).color(color)).truncate());
    if fresh && !text.is_empty() {
        let ring = state.status_alert_age().filter(|t| state.status_rang() && *t < STATUS_RING_SECS).map_or(0.0, |t| 1.0 - t / STATUS_RING_SECS);
        let fill = tint.gamma_multiply(PILL_ALPHA + RING_ALPHA * ring);
        ui.painter().set(pill, egui::epaint::RectShape::filled(label.rect.expand2(Vec2::new(6.0, 2.0)), 4.0, fill));
        ui.ctx().request_repaint_after_secs(FRESH_SECS - age);
    }

    label.hovered()
}

/// The text color of the message and the tint of the pill behind a fresh one.
fn message_colors(c: &Palette, failed: bool, fresh: bool) -> (Color32, Color32) {
    match (failed, fresh) {
        // A failure stays at full strength, the user may come looking for it later. It is fitted against the pill at
        // its strongest, which darkens the light theme's chrome.
        (true, _) => (c.readable_at(c.warning(), blend(c.chrome, c.warning(), PILL_ALPHA + RING_ALPHA), 4.5), c.warning()),
        (false, true) => (c.fg, c.fg),
        (false, false) => (c.fg.gamma_multiply_u8(150), c.fg),
    }
}

/// The bell's tilt `t` seconds into a ring, a few swings that die down.
fn swing(t: f32) -> f32 {
    let fade = 1.0 - t / STATUS_RING_SECS;
    0.45 * fade * fade * (t * std::f32::consts::TAU * 2.5).sin()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Prefs;
    use crate::theme::contrast_ratio;
    use egui_kittest::kittest::Queryable;

    fn status_bar(state: EditorState) -> egui_kittest::Harness<'static, EditorState> {
        egui_kittest::Harness::builder()
            .with_size(egui::vec2(600.0, 400.0))
            .build_ui_state(|ui, state: &mut EditorState| drop(ui.horizontal(|ui| show(ui, state))), state)
    }

    #[test]
    fn a_click_on_the_bell_lists_the_messages_newest_first_and_clears_its_dot_at_once() {
        let mut state = EditorState::new(Prefs::default());
        state.set_status("Grid 32");
        state.fail_status("Nothing to paste");
        state.set_status("Grid 64");
        let mut bar = status_bar(state);
        assert!(bar.query_by_label("Nothing to paste").is_none());
        bar.get_by_label(RECENT).click();
        bar.step();
        assert!(bar.state().status_alert_age().is_none(), "the bell is still ringing, only the click clears it");
        let top = |label| bar.get_by_label(label).rect().min.y;
        assert!(top("Nothing to paste") < top("Grid 32"));
    }

    #[test]
    fn an_open_list_counts_as_a_look() {
        let mut state = EditorState::new(Prefs::default());
        state.set_status("Grid 32");
        let mut bar = status_bar(state);
        let away = egui::pos2(590.0, 390.0);
        bar.hover_at(away);
        bar.state_mut().fail_status("Nothing to paste");
        bar.state_mut().rewind_rings(std::time::Duration::from_secs(2));
        bar.step();
        assert!(bar.state().status_alert_age().is_some(), "nobody looked");
        bar.get_by_label(RECENT).click();
        bar.step();
        bar.hover_at(away);
        bar.state_mut().fail_status("Nothing to cut");
        bar.state_mut().rewind_rings(std::time::Duration::from_secs(2));
        bar.step();
        assert_eq!(bar.query_all_by_label("Nothing to cut").count(), 2, "on the status line and in the list");
        assert!(bar.state().status_alert_age().is_none());
    }

    #[test]
    fn the_bell_comes_to_rest_after_its_ring() {
        assert_eq!(swing(0.0), 0.0);
        assert!(swing(0.1).abs() > 0.2);
        assert!(swing(STATUS_RING_SECS).abs() < 1e-6);
    }

    #[test]
    fn a_fresh_message_keeps_4_5_to_1_on_its_pill_in_both_themes() {
        for c in [&crate::theme::DARK, &crate::theme::LIGHT] {
            for failed in [true, false] {
                let (text, tint) = message_colors(c, failed, true);
                for ring in [0.0, 0.5, 1.0] {
                    let ratio = contrast_ratio(text, blend(c.chrome, tint, PILL_ALPHA + RING_ALPHA * ring));
                    assert!(ratio >= 4.5, "{ratio:.2}:1 with failed {failed}, ring {ring}, dark {}", c.dark);
                }
            }
        }
    }
}
