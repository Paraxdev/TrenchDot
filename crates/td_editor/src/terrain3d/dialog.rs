//! The Link Terrain3D dialog.

use egui::Ui;

use super::link::{self, NewFolder};
use crate::i18n::{en, msg, tr, trf};
use crate::state::EditorState;
use crate::{theme, widgets};

const RES: &str = "res://";

#[derive(Default)]
pub struct LinkDialog {
    pub open: bool,
    folders: Vec<String>,
    /// The picked folder, None for a new one.
    choice: Option<String>,
    new: NewFolder,
}

impl LinkDialog {
    pub fn open(&mut self, state: &EditorState) {
        self.folders = state.game.project_root.as_deref().map(link::data_folders).unwrap_or_default();
        self.choice = self.folders.first().cloned();
        self.open = true;
    }

    pub fn show(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        let mut open = self.open;
        widgets::window(en!("Link Terrain3D")).open(&mut open).resizable(false).show(ctx, |ui| {
            let Some(root) = state.game.project_root.clone() else {
                ui.label(tr("Open a Godot project first, Terrain3D data lives in one"));
                return;
            };

            self.fields(ui);
            if !root.join("addons").join("terrain_3d").is_dir() {
                ui.label(tr("Terrain3D is not installed in this project, the link works and edits wait until it is"));
            }

            let problem = self.choice.is_none().then(|| self.new.problem()).flatten();
            if let Some(problem) = problem {
                ui.colored_label(theme::colors().warning(), tr(problem.english()));
            }

            if ui.add_enabled(problem.is_none(), egui::Button::new(tr("Link"))).clicked() {
                let result = match &self.choice {
                    Some(folder) => state.link_terrain3d(folder, None),
                    None => state.link_terrain3d(&self.new.data_directory(), Some(&self.new)),
                };
                match result {
                    Ok(_) => self.open = false,
                    Err(problem) => state.fail_status(msg!("Terrain3D was not linked: {problem}", problem)),
                }
            }
        });
        self.open &= open;
    }

    fn fields(&mut self, ui: &mut Ui) {
        egui::Grid::new("terrain3d_link").num_columns(2).show(ui, |ui| {
            ui.label(tr("Data Folder"));
            let shown = self.choice.clone().unwrap_or_else(|| tr("New Folder").into());
            egui::ComboBox::from_id_salt("terrain3d_folder").selected_text(shown).show_ui(ui, |ui| {
                for folder in &self.folders {
                    ui.selectable_value(&mut self.choice, Some(folder.clone()), folder);
                }

                ui.selectable_value(&mut self.choice, None, tr("New Folder"));
            });
            ui.end_row();
            if self.choice.is_some() {
                return;
            }

            ui.label(tr("Folder Name"));
            ui.horizontal(|ui| {
                ui.label(RES);
                ui.add(egui::TextEdit::singleline(&mut self.new.name).desired_width(140.0));
            });
            ui.end_row();
            ui.label(tr("Region Size"));
            egui::ComboBox::from_id_salt("terrain3d_region_size").selected_text(self.new.region_size.to_string()).show_ui(ui, |ui| {
                for size in td_geom::terrain3d::REGION_SIZES {
                    ui.selectable_value(&mut self.new.region_size, size, size.to_string());
                }
            });
            ui.end_row();
            ui.label(tr("Vertex Spacing"));
            ui.add(
                egui::DragValue::new(&mut self.new.vertex_spacing)
                    .range(td_geom::terrain3d::MIN_VERTEX_SPACING..=td_geom::terrain3d::MAX_VERTEX_SPACING)
                    .speed(0.05)
                    .suffix(" m"),
            );
            ui.end_row();
            ui.label(tr("Regions"));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.new.columns).range(1..=32));
                ui.label("x");
                ui.add(egui::DragValue::new(&mut self.new.rows).range(1..=32));
            });
            ui.end_row();
            ui.label("");
            let side = |n: u32| (n as f64 * self.new.region_size as f64 * self.new.vertex_spacing).round();
            ui.label(trf!("{w} x {h} meters", w = side(self.new.columns), h = side(self.new.rows)));
            ui.end_row();
        });
    }
}
