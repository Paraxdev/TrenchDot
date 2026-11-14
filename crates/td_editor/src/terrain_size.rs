//! The Size block of the terrain Inspector. Everything typed there is a draft until Apply, which makes one undo step.

use egui::{RichText, Ui};
use td_core::{Aabb, NodeId};
use td_doc::terrain_resize::{ResizePlan, TerrainResize, apply, plan_over};
use td_geom::Terrain;
use td_geom::heightfield::{MAX_CELL_SIZE, MIN_CELL_SIZE};
use td_geom::terrain_resize::{EdgeFill, MAX_CELLS, Sides, resized_cells};
use td_render::LineVertex;

use crate::i18n::{en, msg, tr, tr_n, trf};
use crate::state::EditorState;
use crate::{theme, widgets};

/// Far past any limit, so a side field can hold a refused value and say why instead of clamping it away.
const SIDE_LIMIT: i64 = MAX_CELLS as i64 * 4;

/// A size change typed for terrain `id` and not applied yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainResizeDraft {
    pub id: NodeId,
    pub resize: TerrainResize,
}

/// Splits a change of `diff` cells between the two opposite sides of an axis, the odd cell goes to the far one.
pub fn split_difference(diff: i32) -> (i32, i32) {
    let near = diff / 2;
    (near, diff - near)
}

/// The cells along axis `axis` (0 is x, 1 is z) once the sides are applied, before any cell size change.
pub fn total_cells(cells: [u32; 2], sides: Sides, axis: usize) -> i64 {
    let (near, far) = if axis == 0 { (sides.west, sides.east) } else { (sides.north, sides.south) };
    cells[axis] as i64 + near as i64 + far as i64
}

/// Sets the cells along `axis` to `total` by growing or cropping both of its sides, replacing what was typed there.
pub fn set_total(sides: &mut Sides, cells: [u32; 2], axis: usize, total: i64) {
    let (near, far) = split_difference((total - cells[axis] as i64).clamp(-SIDE_LIMIT, SIDE_LIMIT) as i32);
    if axis == 0 {
        (sides.west, sides.east) = (near, far);
    } else {
        (sides.north, sides.south) = (near, far);
    }
}

fn grows(sides: Sides) -> bool {
    [sides.north, sides.south, sides.west, sides.east].iter().any(|s| *s > 0)
}

/// Why a draft is refused, in the editor's language.
fn refusal(terrain: &Terrain, resize: &TerrainResize) -> String {
    let totals = [total_cells(terrain.cells(), resize.sides, 0), total_cells(terrain.cells(), resize.sides, 1)];
    if totals.iter().any(|t| *t < 1) {
        tr("A terrain needs at least 1 cell along each axis").to_string()
    } else if totals.iter().any(|t| *t > MAX_CELLS as i64) || resized_cells(terrain.cells(), resize.sides).is_ok() {
        trf!("A terrain has at most {max} cells along each axis", max = MAX_CELLS)
    } else {
        tr("The change has to keep at least 1 cell of the terrain along each axis").to_string()
    }
}

/// The lowest and highest ground of terrain `id`, read again only once the map changes. Every view asks for it every
/// frame while a draft is open, and reading it walks every height.
pub fn height_range(state: &EditorState, id: NodeId, terrain: &Terrain) -> [f64; 2] {
    match state.terrain_heights.get() {
        Some((at, revision, heights)) if at == id && revision == state.doc.revision => heights,
        _ => {
            let heights = td_doc::terrain_resize::height_range(terrain);
            state.terrain_heights.set(Some((id, state.doc.revision, heights)));
            heights
        }
    }
}

fn plan(state: &EditorState, id: NodeId, terrain: &Terrain, resize: &TerrainResize) -> Result<ResizePlan, String> {
    plan_over(&state.doc.map, id, resize, height_range(state, id, terrain))
}

/// Where the terrain's edges land if the pending draft were applied, none while it changes nothing, is refused or
/// belongs to something other than the one selected node.
pub fn footprint(state: &EditorState) -> Option<Aabb> {
    let draft = state.terrain_resize?;
    let terrain = state.doc.map.terrain(draft.id)?;
    if draft.resize.is_noop(terrain) || !state.doc.selection.nodes.iter().eq([&draft.id]) {
        return None;
    }

    // Scatter is left out of the plan, counting it every frame in every view is wasted work for a box.
    plan(state, draft.id, terrain, &TerrainResize { drop_scatter: false, ..draft.resize }).ok().map(|p| p.bounds)
}

pub fn lines(state: &EditorState, out: &mut Vec<LineVertex>) {
    if let Some(b) = footprint(state) {
        crate::shape_bounds::box_lines(b, out);
    }
}

fn fill_label(fill: EdgeFill) -> &'static str {
    match fill {
        EdgeFill::Edge => en!("Edge height"),
        EdgeFill::Zero => en!("Zero"),
    }
}

/// The Size rows for terrain `id`, then the preview, the warnings and Apply and Reset while the draft changes something.
pub fn size_ui(ui: &mut Ui, state: &mut EditorState, id: NodeId, terrain: &Terrain) {
    if state.terrain_resize.is_some_and(|d| d.id != id) {
        state.terrain_resize = None;
    }

    let mut resize = state.terrain_resize.map_or_else(TerrainResize::default, |d| d.resize);
    let cells = terrain.cells();
    let label_w = widgets::label_width(ui);
    ui.add_space(4.0);
    ui.label(RichText::new(tr("Size")).strong());
    let field_w = |ui: &Ui| ui.available_width() - widgets::TRAILING;
    widgets::row(ui, 0, tr("Cells"), tr("The cells along X and Z after the change, typing a total grows or crops both opposite sides evenly"), label_w, |ui| {
        let half = (field_w(ui) - 6.0) / 2.0;
        for axis in 0..2 {
            let mut total = total_cells(cells, resize.sides, axis);
            if widgets::count_field(ui, &mut total, half, false) {
                set_total(&mut resize.sides, cells, axis, total.max(0));
            }
        }
    });
    let side_rows = [(en!("North (-Z)"), 0), (en!("South (+Z)"), 1), (en!("West (-X)"), 2), (en!("East (+X)"), 3)];
    for (row, (label, side)) in side_rows.into_iter().enumerate() {
        let slot = match side {
            0 => &mut resize.sides.north,
            1 => &mut resize.sides.south,
            2 => &mut resize.sides.west,
            _ => &mut resize.sides.east,
        };
        let mut value = *slot as i64;
        widgets::row(ui, row + 1, tr(label), tr("Cells to add on this side, a negative number removes them"), label_w, |ui| {
            if widgets::count_field(ui, &mut value, field_w(ui), true) {
                *slot = value.clamp(-SIDE_LIMIT, SIDE_LIMIT) as i32;
            }
        });
    }

    let mut cell_size = resize.cell_size.unwrap_or(terrain.cell_size);
    widgets::row(ui, 5, tr("Cell size"), tr("Changes the spacing and keeps the extent, heights and paint are resampled"), label_w, |ui| {
        let speed = widgets::drag_speed(&[cell_size]);
        let drag = egui::DragValue::new(&mut cell_size).range(MIN_CELL_SIZE..=MAX_CELL_SIZE).speed(speed).suffix(" u");
        if widgets::styled_drag(ui, drag, field_w(ui)) {
            resize.cell_size = (cell_size != terrain.cell_size).then_some(cell_size);
        }
    });
    if grows(resize.sides) {
        widgets::row(
            ui,
            6,
            tr("New ground"),
            tr("What the new cells get, the height and paint of the nearest edge or height 0 with the first layer"),
            label_w,
            |ui| {
                egui::ComboBox::from_id_salt("terrain_fill").selected_text(tr(fill_label(resize.fill))).show_ui(ui, |ui| {
                    for fill in [EdgeFill::Edge, EdgeFill::Zero] {
                        ui.selectable_value(&mut resize.fill, fill, tr(fill_label(fill)));
                    }
                });
            },
        );
    }

    if resize.is_noop(terrain) {
        state.terrain_resize = None;
        return;
    }

    // The count is asked with removal on, so the checkbox can still name it once it is switched off.
    let planned = plan(state, id, terrain, &TerrainResize { drop_scatter: true, ..resize });
    state.terrain_resize = Some(TerrainResizeDraft { id, resize });
    // Apply sits above the rows that come and go with the draft, so it stays put while sides are typed.
    buttons_ui(ui, state, id, resize, planned.is_ok());
    if state.terrain_resize.is_none() {
        return;
    }

    preview_ui(ui, state, id, terrain, planned, &mut resize);
    state.terrain_resize = Some(TerrainResizeDraft { id, resize });
}

/// The scatter checkbox, the size after the change and what it costs, or why the draft is refused.
fn preview_ui(ui: &mut Ui, state: &EditorState, id: NodeId, terrain: &Terrain, planned: Result<ResizePlan, String>, resize: &mut TerrainResize) {
    let Ok(plan) = planned else {
        let reason = if state.doc.map.is_editable(id) { refusal(terrain, resize) } else { tr("The terrain is hidden or locked").to_string() };
        ui.label(RichText::new(reason).color(theme::colors().error()));
        return;
    };

    if plan.scatter_removed > 0 {
        let text = tr_n!(plan.scatter_removed, "Remove {n} scatter instance on removed ground", "Remove {n} scatter instances on removed ground");
        ui.checkbox(&mut resize.drop_scatter, text).on_hover_text(tr("Off keeps them where they are, floating over nothing"));
    }

    ui.label(after_line(state, &plan));
    let sided = [total_cells(terrain.cells(), resize.sides, 0), total_cells(terrain.cells(), resize.sides, 1)];
    let grown = [0, 1].map(|k| plan.cells[k] as f64 * plan.cell_size - sided[k] as f64 * terrain.cell_size);
    if grown.iter().any(|g| g.abs() > 1e-6) {
        let note = trf!("Whole cells change the size by {w} x {d} units", w = format!("{:+}", grown[0]), d = format!("{:+}", grown[1]));
        ui.label(RichText::new(note).color(theme::colors().warning()));
    }

    if plan.detail_lost {
        let warning = trf!("Fine detail is lost, cells grow from {from} to {to} units", from = terrain.cell_size, to = plan.cell_size);
        ui.label(RichText::new(warning).color(theme::colors().warning()));
    }
}

fn after_line(state: &EditorState, plan: &ResizePlan) -> String {
    let upm = state.game.units_per_meter;
    let (w, d) = (plan.cells[0] as f64 * plan.cell_size, plan.cells[1] as f64 * plan.cell_size);
    trf!(
        "After: {cx} x {cz} cells, {w} x {d} units ({mw} x {md} m)",
        cx = plan.cells[0],
        cz = plan.cells[1],
        w = w,
        d = d,
        mw = format!("{:.0}", w / upm),
        md = format!("{:.0}", d / upm)
    )
}

fn buttons_ui(ui: &mut Ui, state: &mut EditorState, id: NodeId, resize: TerrainResize, planned: bool) {
    let editable = state.doc.map.is_editable(id);
    let valid = editable && planned;
    ui.horizontal_wrapped(|ui| {
        let mut apply_button = ui.add_enabled(valid, egui::Button::new(tr("Apply")));
        if !editable {
            apply_button = apply_button.on_disabled_hover_text(tr("The terrain is hidden or locked"));
        }

        if apply_button.clicked()
            && let Ok(report) = state.doc.try_edit("Resize Terrain", |m, _| apply(m, id, &resize))
        {
            state.terrain_resize = None;
            let [cx, cz] = report.plan.cells;
            state.set_status(msg!("Resized the terrain to {cx} x {cz} cells", cx, cz));
        }

        if ui.button(tr("Reset")).on_hover_text(tr("Puts the size back as it is")).clicked() {
            state.terrain_resize = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_odd_cell_goes_to_the_far_side() {
        let split: Vec<_> = [0, 1, -1, 2, 3, -3, -4, 5].into_iter().map(split_difference).collect();
        assert_eq!(split, [(0, 0), (0, 1), (0, -1), (1, 1), (1, 2), (-1, -2), (-2, -2), (2, 3)]);
    }

    #[test]
    fn a_total_replaces_both_sides_of_its_axis_only() {
        let cells = [16, 8];
        let mut sides = Sides { north: 5, south: 6, west: -3, east: 9 };
        set_total(&mut sides, cells, 0, 21);
        assert_eq!(sides, Sides { north: 5, south: 6, west: 2, east: 3 });
        set_total(&mut sides, cells, 1, 3);
        assert_eq!(sides, Sides { north: -2, south: -3, west: 2, east: 3 });
        assert_eq!([total_cells(cells, sides, 0), total_cells(cells, sides, 1)], [21, 3]);
        set_total(&mut sides, cells, 0, 16);
        assert_eq!((sides.west, sides.east), (0, 0));
    }

    #[test]
    fn the_height_range_is_read_again_only_for_another_terrain_or_revision() {
        use td_doc::map::NodeKind;
        let mut state = EditorState::new(Default::default());
        let layer = state.doc.map.default_layer();
        let mut low = Terrain::new(td_core::DVec3::new(-40.5, 7.0, 13.0), [3, 4], 16.0, "");
        low.heights = vec![-3.0, 1.0, 2.0, 0.0, 5.0, 4.0, 1.0, 1.0, 2.0, 9.5, 0.0, -1.0];
        let mut high = Terrain::new(td_core::DVec3::new(100.0, -20.0, 0.0), [2, 2], 32.0, "");
        high.heights = vec![40.0, 41.0, 42.0, 60.0];
        let a = state.doc.map.insert(layer, NodeKind::Terrain(low.clone()));
        let b = state.doc.map.insert(layer, NodeKind::Terrain(high.clone()));
        assert_eq!(state.terrain_heights.get(), None);

        assert_eq!(height_range(&state, a, &low), [4.0, 16.5]);
        assert_eq!(height_range(&state, b, &high), [20.0, 40.0]);
        assert_eq!(height_range(&state, a, &low), [4.0, 16.5], "a second terrain does not leak into the first");

        low.heights[9] = 30.0;
        assert_eq!(height_range(&state, a, &low), [4.0, 16.5], "the same revision keeps what it read");
        state.doc.revision += 1;
        assert_eq!(height_range(&state, a, &low), [4.0, 37.0]);
        assert_eq!(state.terrain_heights.get(), Some((a, state.doc.revision, [4.0, 37.0])));
    }

    #[test]
    fn a_huge_total_is_clamped_to_something_the_limit_check_refuses() {
        let mut sides = Sides::default();
        set_total(&mut sides, [16, 8], 0, i64::MAX);
        assert!(total_cells([16, 8], sides, 0) > MAX_CELLS as i64);
        set_total(&mut sides, [16, 8], 0, 0);
        assert_eq!(total_cells([16, 8], sides, 0), 0);
    }
}
