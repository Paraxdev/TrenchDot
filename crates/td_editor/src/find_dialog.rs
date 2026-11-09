//! Find and Replace: entities by class, key and value, and one material swapped for another over the map or the selection.

use egui::RichText;
use td_core::NodeId;
use td_doc::Selection;
use td_doc::find::{Changed, EntityQuery, MaterialScope, count_material, find_entities, materials_in_use, replace_material};

use crate::i18n::{en, msg_n, tr, tr_n, trf};
use crate::panels::DndPayload;
use crate::state::EditorState;

/// What the hit list and the material counts were computed from, so they are only redone when one of them changes.
type HitsKey = (EntityQuery, u64);
type CountKey = (String, bool, Selection, u64);

#[derive(Default)]
pub struct FindDialog {
    pub open: bool,
    query: EntityQuery,
    from: String,
    to: String,
    in_selection: bool,
    hits: Option<(HitsKey, Vec<NodeId>)>,
    count: Option<(CountKey, Changed)>,
    in_use: Option<(u64, Vec<(String, usize)>)>,
}

/// Selects the entities and frames them in the views. Returns how many were selected. An empty list keeps the selection.
pub fn select_entities(state: &mut EditorState, ids: &[NodeId]) -> usize {
    if ids.is_empty() {
        return 0;
    }

    state.doc.select(|_, s| {
        s.clear();
        s.nodes.extend(ids.iter().copied());
    });
    let n = state.doc.selection.nodes.len();
    if n > 0 {
        let bounds = crate::commands::selection_bounds(state);
        if !bounds.is_empty() {
            state.focus_request = Some(bounds);
        }
    }

    n
}

/// Replaces `from` with `to` in the whole map or the selection as one undo step.
pub fn replace(state: &mut EditorState, from: &str, to: &str, in_selection: bool) -> Result<Changed, String> {
    if from.is_empty() || to.is_empty() {
        return Err("both materials need a name".into());
    }

    if from == to {
        return Err("the two materials are the same".into());
    }

    let selection = in_selection.then(|| state.doc.selection.clone());
    if selection.as_ref().is_some_and(Selection::is_empty) {
        return Err("nothing is selected, select brushes or faces, or replace in the whole map".into());
    }

    let scope = || selection.as_ref().map_or(MaterialScope::Map, MaterialScope::Selection);
    if count_material(&state.doc.map, from, scope()).total() == 0 {
        return Ok(Changed::default());
    }

    Ok(state.doc.edit(en!("Replace Material"), |m, _| replace_material(m, from, to, scope())))
}

impl FindDialog {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        if !self.open {
            return;
        }

        let revision = state.doc.revision;
        let key = (self.query.clone(), revision);
        if self.hits.as_ref().is_none_or(|(k, _)| *k != key) {
            self.hits = Some((key, find_entities(&state.doc.map, &self.query)));
        }

        let selection = if self.in_selection { state.doc.selection.clone() } else { Selection::default() };
        let count_key = (self.from.clone(), self.in_selection, selection, revision);
        if self.count.as_ref().is_none_or(|(k, _)| *k != count_key) {
            let scope = if self.in_selection { MaterialScope::Selection(&count_key.2) } else { MaterialScope::Map };
            let counted = if self.from.is_empty() { Changed::default() } else { count_material(&state.doc.map, &self.from, scope) };
            self.count = Some((count_key, counted));
        }

        if self.in_use.as_ref().is_none_or(|(r, _)| *r != revision) {
            self.in_use = Some((revision, materials_in_use(&state.doc.map).into_iter().collect()));
        }

        let hits: &[NodeId] = self.hits.as_ref().map_or(&[], |(_, h)| h);
        let counted = self.count.as_ref().map(|(_, c)| c.total()).unwrap_or_default();
        let in_use: &[(String, usize)] = self.in_use.as_ref().map_or(&[], |(_, m)| m);
        let mut open = self.open;
        let (mut select_all, mut pick, mut replace_now) = (false, None, false);
        crate::widgets::window(en!("Find and Replace")).open(&mut open).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new(tr("Find entities")).strong());
            egui::Grid::new("find_entity_fields").num_columns(2).show(ui, |ui| {
                for (label, text) in [(en!("Class"), &mut self.query.classname), (en!("Key"), &mut self.query.key), (en!("Value"), &mut self.query.value)] {
                    ui.label(tr(label));
                    let field = ui.add(egui::TextEdit::singleline(text).desired_width(240.0));
                    select_all |= field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                }
            });
            ui.checkbox(&mut self.query.exact, tr("Exact match")).on_hover_text(tr("Off finds part of a text, upper and lower case never matter"));
            if self.query.is_empty() {
                ui.label(RichText::new(tr("Type a class, key or value")).weak());
            } else {
                ui.horizontal(|ui| {
                    ui.label(tr_n!(hits.len(), "{n} entity", "{n} entities"));
                    let button = ui.add_enabled(!hits.is_empty(), egui::Button::new(tr("Select All")));
                    select_all |= button.on_hover_text(tr("Selects every entity found and frames them")).clicked();
                });
                let row_height = ui.text_style_height(&egui::TextStyle::Body);
                egui::ScrollArea::vertical().max_height(120.0).auto_shrink([false, true]).show_rows(ui, row_height, hits.len(), |ui, rows| {
                    for &id in &hits[rows] {
                        let Some(name) = state.doc.map.get(id).map(|n| n.name()) else { continue };
                        if ui.selectable_label(state.doc.selection.nodes.contains(&id), name).clicked() {
                            pick = Some(id);
                        }
                    }
                });
            }

            ui.separator();
            ui.label(RichText::new(tr("Replace material")).strong());
            egui::Grid::new("find_material_fields").num_columns(2).show(ui, |ui| {
                ui.label(tr("Replace"));
                ui.horizontal(|ui| {
                    material_field(ui, state, &mut self.from);
                    egui::ComboBox::from_id_salt("find_in_map").selected_text(tr("In map")).width(0.0).show_ui(ui, |ui| {
                        for (name, n) in in_use {
                            if ui.selectable_label(*name == self.from, format!("{name} ({n})")).clicked() {
                                self.from = name.clone();
                            }
                        }
                    });
                    if ui.small_button(tr("Use Current")).on_hover_text(trf!("Use {material}", material = state.current_material)).clicked() {
                        self.from = state.current_material.clone();
                    }
                });
                ui.end_row();
                ui.label(tr("With"));
                ui.horizontal(|ui| {
                    material_field(ui, state, &mut self.to);
                    if ui.small_button(tr("Use Current")).on_hover_text(trf!("Use {material}", material = state.current_material)).clicked() {
                        self.to = state.current_material.clone();
                    }
                });
                ui.end_row();
            });
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.in_selection, false, tr("Whole map"));
                ui.radio_value(&mut self.in_selection, true, tr("Selection"));
            });
            if !self.from.is_empty() {
                ui.label(RichText::new(tr_n!(counted, "{n} face uses it", "{n} faces use it")).weak());
            }

            let ready = counted > 0 && !self.to.is_empty() && self.to != self.from;
            replace_now = ui.add_enabled(ready, egui::Button::new(tr("Replace"))).on_hover_text(tr("Keeps each face's offset, scale and rotation")).clicked();
        });
        self.open = open;
        if let Some(id) = pick {
            select_entities(state, &[id]);
        } else if select_all {
            let n = select_entities(state, hits);
            state.set_status(msg_n!(n, "Selected {n} entity", "Selected {n} entities"));
        }

        if replace_now {
            let current = self.from.clone();
            if let Ok(changed) = replace(state, &self.from, &self.to, self.in_selection) {
                let n = changed.total();
                state.set_status(msg_n!(n, "Replaced {current} on {n} face", "Replaced {current} on {n} faces", current));
            }
        }
    }
}

/// A material name field that takes a material dragged from the Materials panel, with its thumbnail.
fn material_field(ui: &mut egui::Ui, state: &mut EditorState, name: &mut String) {
    let field = ui.add(egui::TextEdit::singleline(name).desired_width(170.0).hint_text(tr("Material")));
    if field.dnd_hover_payload::<DndPayload>().is_some_and(|p| matches!(*p, DndPayload::Material(_))) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
    }

    if let Some(p) = field.dnd_release_payload::<DndPayload>()
        && let DndPayload::Material(dropped) = &*p
    {
        *name = dropped.clone();
    }

    let size = egui::Vec2::splat(18.0);
    match state.materials.thumbnail(ui.ctx(), name, &mut 1) {
        Some(tex) => _ = ui.image((tex.id(), size)),
        None => _ = ui.allocate_space(size),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Prefs;

    #[test]
    fn selecting_no_hits_keeps_the_selection() {
        let mut state = EditorState::new(Prefs::default());
        let layer = state.doc.map.default_layer();
        let light = state.doc.edit("add", |m, s| {
            let id = m.insert(layer, td_doc::NodeKind::Entity(td_doc::Entity::new("light")));
            s.select_node(id);
            id
        });
        assert_eq!(select_entities(&mut state, &[]), 0);
        assert_eq!(state.doc.selection.nodes, [light].into_iter().collect());
    }
}
