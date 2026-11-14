//! Resizing a terrain in the map, with the scatter painted on it kept on the ground.

use td_core::{Aabb, DVec3, NodeId};
use td_geom::Terrain;
use td_geom::terrain_resize::{EdgeFill, Sides, resized_cells};

use crate::map::Map;
use crate::scatter::{Scatter, ScatterInstance};

/// A resize as the designer sets it up: sides first, in the current cells, then the cell size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainResize {
    pub sides: Sides,
    pub cell_size: Option<f64>,
    pub fill: EdgeFill,
    /// Removes scatter on this terrain that sits over ground the resize removes.
    pub drop_scatter: bool,
}

impl Default for TerrainResize {
    fn default() -> Self {
        Self { sides: Sides::default(), cell_size: None, fill: EdgeFill::Edge, drop_scatter: true }
    }
}

impl TerrainResize {
    pub fn is_noop(&self, terrain: &Terrain) -> bool {
        self.sides.is_zero() && self.cell_size.is_none_or(|c| c == terrain.cell_size)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResizePlan {
    pub resolution: [u32; 2],
    pub cells: [u32; 2],
    pub cell_size: f64,
    pub origin: DVec3,
    /// The new footprint. Its height range is the old one, widened to 0 when Zero fill grows a side.
    pub bounds: Aabb,
    pub scatter_removed: usize,
    pub detail_lost: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResizeReport {
    /// The plan, with the bounds of the terrain as built.
    pub plan: ResizePlan,
    pub scatter_moved: usize,
}

/// What a resize of terrain `id` would do, without building it. Refuses what [`apply`] refuses.
pub fn plan(map: &Map, id: NodeId, resize: &TerrainResize) -> Result<ResizePlan, String> {
    plan_over(map, id, resize, height_range(editable_terrain(map, id)?))
}

/// The lowest and highest ground of `t`. Finding it reads every height, so a caller that plans every frame keeps it.
pub fn height_range(t: &Terrain) -> [f64; 2] {
    let b = t.bounds();
    [b.min.y, b.max.y]
}

/// [`plan`] with the terrain's [`height_range`] already known.
pub fn plan_over(map: &Map, id: NodeId, resize: &TerrainResize, heights: [f64; 2]) -> Result<ResizePlan, String> {
    let old = editable_terrain(map, id)?;
    let after_sides = resized_cells(old.cells(), resize.sides)?;
    let origin = old.origin - DVec3::new(resize.sides.west as f64 * old.cell_size, 0.0, resize.sides.north as f64 * old.cell_size);
    let (cells, cell_size) = match resize.cell_size {
        Some(c) => (cell_shell(after_sides, old.cell_size).rescaled_cells(c)?, c),
        None => (after_sides, old.cell_size),
    };

    let grows = [resize.sides.north, resize.sides.south, resize.sides.west, resize.sides.east].iter().any(|s| *s > 0);
    let [mut lo, mut hi] = heights;
    if resize.fill == EdgeFill::Zero && grows {
        lo = lo.min(old.origin.y);
        hi = hi.max(old.origin.y);
    }

    let size = DVec3::new(cells[0] as f64 * cell_size, 0.0, cells[1] as f64 * cell_size);
    let bounds = Aabb::new(DVec3::new(origin.x, lo, origin.z), DVec3::new(origin.x + size.x, hi, origin.z + size.z));
    let (before, after) = (Footprint::of(old), Footprint::new(origin, cells, cell_size));
    let removed = |set: &Scatter, i: &ScatterInstance| before.contains(i.position) && !after.contains(i.position) && on_terrain(set, i, id, old);
    let scatter_removed =
        if resize.drop_scatter { editable_sets(map).map(|(_, set)| set.instances.iter().filter(|i| removed(set, i)).count()).sum() } else { 0 };
    Ok(ResizePlan {
        resolution: [cells[0] + 1, cells[1] + 1],
        cells,
        cell_size,
        origin,
        bounds,
        scatter_removed,
        detail_lost: resize.cell_size.is_some_and(|c| old.loses_detail(c)),
    })
}

/// Resizes terrain `id` and its scatter. Instances on this terrain over removed ground are removed unless
/// `drop_scatter` is off, the others on it follow the ground. Hidden or locked scatter sets are left as they are.
/// Leaves the map untouched on an error.
pub fn apply(map: &mut Map, id: NodeId, resize: &TerrainResize) -> Result<ResizeReport, String> {
    let mut plan = plan(map, id, resize)?;
    let old = map.terrain(id).cloned().ok_or("not a terrain")?;
    if resize.is_noop(&old) {
        return Ok(ResizeReport { plan, scatter_moved: 0 });
    }

    let mut new = old.resized(resize.sides, resize.fill)?;
    if let Some(c) = resize.cell_size {
        new = new.rescaled(c)?;
    }

    plan.bounds = new.bounds();
    let (footprint, old_footprint) = (Footprint::of(&new), Footprint::of(&old));
    let mut moved = 0;
    let sets: Vec<NodeId> = editable_sets(map).map(|(s, _)| s).collect();
    for set_id in sets {
        let set = map.scatter(set_id).expect("listed above");
        let mut instances = Vec::with_capacity(set.instances.len());
        let mut changed = false;
        for inst in &set.instances {
            let p = inst.position;
            if !old_footprint.contains(p) || !on_terrain(set, inst, id, &old) {
                instances.push(*inst);
                continue;
            }

            if !footprint.contains(p) {
                if resize.drop_scatter {
                    changed = true;
                } else {
                    instances.push(*inst);
                }

                continue;
            }

            let dy = match (ground(&old, p.x, p.z), ground(&new, p.x, p.z)) {
                (Some(a), Some(b)) => b - a,
                _ => 0.0,
            };

            // Below this the change is float noise from the moved origin, not a different ground.
            if dy.abs() > 1e-9 {
                instances.push(ScatterInstance { position: p + DVec3::Y * dy, ..*inst });
                moved += 1;
                changed = true;
            } else {
                instances.push(*inst);
            }
        }

        if changed {
            map.scatter_mut(set_id).expect("listed above").instances = instances;
        }
    }

    *map.terrain_mut(id).expect("checked by plan") = new;
    Ok(ResizeReport { plan, scatter_moved: moved })
}

fn editable_terrain(map: &Map, id: NodeId) -> Result<&Terrain, String> {
    let terrain = map.terrain(id).ok_or_else(|| format!("node {} is not a terrain", id.0))?;
    if !map.is_editable(id) {
        return Err(format!("terrain {} is hidden or locked", id.0));
    }

    terrain.check_data()?;
    Ok(terrain)
}

fn editable_sets(map: &Map) -> impl Iterator<Item = (NodeId, &Scatter)> {
    map.scatters().filter(|(s, _)| map.is_editable(*s))
}

/// A terrain with only its grid, enough for [`Terrain::rescaled_cells`].
fn cell_shell(cells: [u32; 2], cell_size: f64) -> Terrain {
    Terrain { resolution: [cells[0] + 1, cells[1] + 1], heights: Vec::new(), ..Terrain::new(DVec3::ZERO, [2, 2], cell_size, "") }
}

struct Footprint {
    min: [f64; 2],
    max: [f64; 2],
}

impl Footprint {
    fn new(origin: DVec3, cells: [u32; 2], cell_size: f64) -> Self {
        Self { min: [origin.x, origin.z], max: [origin.x + cells[0] as f64 * cell_size, origin.z + cells[1] as f64 * cell_size] }
    }

    fn of(t: &Terrain) -> Self {
        Self::new(t.origin, t.cells(), t.cell_size)
    }

    fn contains(&self, p: DVec3) -> bool {
        (self.min[0]..=self.max[0]).contains(&p.x) && (self.min[1]..=self.max[1]).contains(&p.z)
    }
}

/// The set targets this terrain or paints everywhere, and the instance's base lies within a meter (or a cell) of the
/// ground. A set can target a roof over the terrain as well, so a target alone does not put an instance on the ground.
fn on_terrain(set: &Scatter, inst: &ScatterInstance, id: NodeId, old: &Terrain) -> bool {
    let sink = set.items.get(inst.item as usize).map_or(0.0, |i| i.sink);
    let base = inst.position.y + sink * inst.scale;
    (set.targets.is_empty() || set.targets.contains(&id))
        && old.height_at(inst.position.x, inst.position.z).is_some_and(|g| (base - g).abs() <= old.cell_size.max(32.0))
}

/// The ground height at x and z, none outside or over a hole.
fn ground(t: &Terrain, x: f64, z: f64) -> Option<f64> {
    let h = t.height_at(x, z)?;
    let [cw, cd] = t.cells();
    let ci = (((x - t.origin.x) / t.cell_size).floor().max(0.0) as u32).min(cw - 1);
    let cj = (((z - t.origin.z) / t.cell_size).floor().max(0.0) as u32).min(cd - 1);
    (!t.is_hole(ci, cj)).then_some(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::NodeKind;
    use crate::ops::{self, EditOptions};
    use crate::scatter::{ScatterItem, ScatterKind};
    use crate::{Document, format};
    use td_geom::Brush;

    const SINK: f64 = 2.0;
    const SCALE: f64 = 1.5;

    struct Scene {
        terrain: NodeId,
        brush: NodeId,
        targeted: NodeId,
        free: NodeId,
        brush_only: NodeId,
    }

    /// 6 x 5 cells off the origin, a non planar slope so a diagonal flip moves the ground, a hole at cell (4, 3).
    fn terrain() -> Terrain {
        let mut t = Terrain::new(DVec3::new(-96.5, 12.0, 40.25), [7, 6], 32.0, "grass");
        for j in 0..6 {
            for i in 0..7 {
                let k = t.index(i, j);
                t.heights[k] = (7 + 5 * i + 3 * j + (i * j % 3) * 4) as f32;
            }
        }

        t.splat = (0..42).flat_map(|k| if k % 5 == 0 { [128, 127, 0, 0] } else { [255, 0, 0, 0] }).collect();
        t.holes = vec![0; 30];
        t.holes[3 * 6 + 4] = 1;
        t
    }

    fn inst(position: DVec3) -> ScatterInstance {
        ScatterInstance { item: 0, position, angles: DVec3::new(0.0, 33.0, 0.0), scale: SCALE }
    }

    /// An instance seated the way painting seats it: its base on the ground, pushed in by its sink.
    fn seated(t: &Terrain, x: f64, z: f64, sink: f64) -> ScatterInstance {
        inst(DVec3::new(x, t.height_at(x, z).unwrap() - sink * SCALE, z))
    }

    fn build(m: &mut Map) -> Scene {
        let layer = m.default_layer();
        let t = terrain();
        let terrain = m.insert(layer, NodeKind::Terrain(t.clone()));
        let brush =
            m.insert(layer, NodeKind::Brush(Brush::from_aabb(&Aabb::new(DVec3::new(-90.0, 300.0, 140.0), DVec3::new(-60.0, 316.0, 160.0)), "a").unwrap()));

        let mut targeted = Scatter::new("targeted", ScatterKind::Props, vec![ScatterItem::new("res://rock.glb")]);
        targeted.targets = vec![terrain];
        targeted.instances = vec![seated(&t, -80.0, 100.0, 0.0), seated(&t, 10.0, 120.0, 0.0), inst(DVec3::new(-150.0, 30.0, 100.0))];

        let mut item = ScatterItem::new("res://tree.glb");
        item.sink = SINK;
        let mut free = Scatter::new("free", ScatterKind::Props, vec![item]);
        free.instances =
            vec![seated(&t, -80.3, 60.0, SINK), inst(DVec3::new(-75.0, 316.0, 150.0)), seated(&t, 20.7, 77.9, SINK), seated(&t, 45.0, 150.0, SINK)];

        let mut brush_only = Scatter::new("brush only", ScatterKind::Props, vec![ScatterItem::new("res://bush.glb")]);
        brush_only.targets = vec![brush];
        brush_only.instances = vec![seated(&t, -85.0, 70.0, 0.0)];

        let targeted = m.insert(layer, NodeKind::Scatter(targeted));
        let free = m.insert(layer, NodeKind::Scatter(free));
        let brush_only = m.insert(layer, NodeKind::Scatter(brush_only));
        Scene { terrain, brush, targeted, free, brush_only }
    }

    fn scene() -> (Map, Scene) {
        let mut m = Map::new();
        let s = build(&mut m);
        (m, s)
    }

    fn sides(north: i32, south: i32, west: i32, east: i32) -> TerrainResize {
        TerrainResize { sides: Sides { north, south, west, east }, ..Default::default() }
    }

    fn instances(m: &Map, set: NodeId) -> Vec<ScatterInstance> {
        m.scatter(set).unwrap().instances.clone()
    }

    fn lift(i: ScatterInstance, old: &Terrain, new: &Terrain) -> ScatterInstance {
        let p = i.position;
        ScatterInstance { position: p + DVec3::Y * (new.height_at(p.x, p.z).unwrap() - old.height_at(p.x, p.z).unwrap()), ..i }
    }

    #[test]
    fn an_odd_west_crop_removes_scatter_over_cut_ground_and_seats_the_rest() {
        let (mut m, s) = scene();
        let old = m.terrain(s.terrain).unwrap().clone();
        let (a, f, b) = (instances(&m, s.targeted), instances(&m, s.free), instances(&m, s.brush_only));
        let r = sides(0, 0, -1, 0);
        let planned = plan(&m, s.terrain, &r).unwrap();
        let report = apply(&mut m, s.terrain, &r).unwrap();
        let new = m.terrain(s.terrain).unwrap().clone();

        assert_eq!(new, old.resized(r.sides, EdgeFill::Edge).unwrap());
        assert_eq!(
            planned,
            ResizePlan {
                resolution: [6, 6],
                cells: [5, 5],
                cell_size: 32.0,
                origin: DVec3::new(-64.5, 12.0, 40.25),
                bounds: Aabb::new(DVec3::new(-64.5, old.bounds().min.y, 40.25), DVec3::new(95.5, old.bounds().max.y, 200.25)),
                scatter_removed: 2,
                detail_lost: false,
            }
        );
        assert_eq!(report.plan, ResizePlan { bounds: new.bounds(), ..planned });

        // The flipped diagonal moves the ground under both kept instances, which follow it by exactly that much.
        let (a2, f3) = (lift(a[1], &old, &new), lift(f[2], &old, &new));
        assert!((a2.position.y - a[1].position.y).abs() > 0.1 && (f3.position.y - f[2].position.y).abs() > 0.1);
        assert!((f3.position.y + SINK * SCALE - new.height_at(20.7, 77.9).unwrap()).abs() < 1e-9);
        assert_eq!(report.scatter_moved, 2);
        assert_eq!(instances(&m, s.targeted), vec![a2, a[2]]);
        // The roof instance is over cut ground but not on the terrain, the one over the hole keeps its height.
        assert_eq!(instances(&m, s.free), vec![f[1], f3, f[3]]);
        assert_eq!(instances(&m, s.brush_only), b);
    }

    #[test]
    fn keeping_scatter_leaves_instances_over_cut_ground() {
        let (mut m, s) = scene();
        let old = m.terrain(s.terrain).unwrap().clone();
        let (a, f) = (instances(&m, s.targeted), instances(&m, s.free));
        let r = TerrainResize { drop_scatter: false, ..sides(0, 0, -1, 0) };
        assert_eq!(plan(&m, s.terrain, &r).unwrap().scatter_removed, 0);
        let report = apply(&mut m, s.terrain, &r).unwrap();
        let new = m.terrain(s.terrain).unwrap().clone();
        assert_eq!(report.scatter_moved, 2);
        assert_eq!(instances(&m, s.targeted), vec![a[0], lift(a[1], &old, &new), a[2]]);
        assert_eq!(instances(&m, s.free), vec![f[0], f[1], lift(f[2], &old, &new), f[3]]);
    }

    #[test]
    fn an_even_north_crop_moves_no_instance() {
        let (mut m, s) = scene();
        let (a, f, b) = (instances(&m, s.targeted), instances(&m, s.free), instances(&m, s.brush_only));
        let r = sides(-2, 0, 0, 0);
        assert_eq!(plan(&m, s.terrain, &r).unwrap().scatter_removed, 3);
        let report = apply(&mut m, s.terrain, &r).unwrap();
        assert_eq!((report.scatter_moved, report.plan.scatter_removed), (0, 3));
        assert_eq!(instances(&m, s.targeted), vec![a[1], a[2]]);
        assert_eq!(instances(&m, s.free), vec![f[1], f[3]]);
        assert_eq!(instances(&m, s.brush_only), b);
    }

    #[test]
    fn a_set_targeting_the_terrain_and_a_roof_leaves_the_roof_alone() {
        for cell_size in [None, Some(20.0)] {
            let (mut m, s) = scene();
            let old = m.terrain(s.terrain).unwrap().clone();
            let mut both = Scatter::new("both", ScatterKind::Props, vec![ScatterItem::new("res://crate.glb")]);
            both.targets = vec![s.brush, s.terrain];
            let roof = inst(DVec3::new(-75.0, 316.0, 150.0));
            both.instances = vec![roof, seated(&old, -80.0, 100.0, 0.0), seated(&old, 10.0, 120.0, 0.0)];
            let both = m.insert(m.default_layer(), NodeKind::Scatter(both));

            let r = TerrainResize { cell_size, ..sides(0, 0, -1, 0) };
            assert_eq!(plan(&m, s.terrain, &r).unwrap().scatter_removed, 3, "{cell_size:?}");
            apply(&mut m, s.terrain, &r).unwrap();
            let new = m.terrain(s.terrain).unwrap().clone();
            let kept = lift(seated(&old, 10.0, 120.0, 0.0), &old, &new);
            assert_eq!(instances(&m, both), vec![roof, kept], "{cell_size:?}");
        }
    }

    #[test]
    fn hidden_or_locked_scatter_sets_are_left_as_they_are() {
        for hide in [false, true] {
            let (mut m, s) = scene();
            let old = m.terrain(s.terrain).unwrap().clone();
            let (a, f) = (instances(&m, s.targeted), instances(&m, s.free));
            let node = m.get_mut(s.free).unwrap();
            if hide {
                node.hidden = true;
            } else {
                node.locked = true;
            }

            let r = sides(0, 0, -1, 0);
            assert_eq!(plan(&m, s.terrain, &r).unwrap().scatter_removed, 1, "hide {hide}");
            let report = apply(&mut m, s.terrain, &r).unwrap();
            let new = m.terrain(s.terrain).unwrap().clone();
            assert_eq!(report.scatter_moved, 1, "hide {hide}");
            assert_eq!(instances(&m, s.targeted), vec![lift(a[1], &old, &new), a[2]], "hide {hide}");
            assert_eq!(instances(&m, s.free), f, "hide {hide}");
        }
    }

    #[test]
    fn a_plan_over_a_known_height_range_matches_the_plan() {
        let (m, s) = scene();
        let t = m.terrain(s.terrain).unwrap();
        let heights = height_range(t);
        assert_eq!(heights, [t.bounds().min.y, t.bounds().max.y]);
        for r in
            [sides(2, -1, 0, 3), TerrainResize { fill: EdgeFill::Zero, ..sides(0, 1, 0, 0) }, TerrainResize { cell_size: Some(48.0), ..Default::default() }]
        {
            assert_eq!(plan_over(&m, s.terrain, &r, heights), plan(&m, s.terrain, &r));
        }

        let wide = plan_over(&m, s.terrain, &sides(0, 0, 0, 1), [-500.0, 900.0]).unwrap();
        assert_eq!([wide.bounds.min.y, wide.bounds.max.y], [-500.0, 900.0]);
        assert!(plan_over(&m, s.brush, &sides(0, 0, 0, 1), heights).unwrap_err().contains("is not a terrain"));
    }

    #[test]
    fn a_cell_size_change_keeps_the_origin_and_seats_scatter_on_the_resampled_ground() {
        let (mut m, s) = scene();
        let old = m.terrain(s.terrain).unwrap().clone();
        let (a, f, b) = (instances(&m, s.targeted), instances(&m, s.free), instances(&m, s.brush_only));
        let r = TerrainResize { cell_size: Some(24.0), ..Default::default() };
        let planned = plan(&m, s.terrain, &r).unwrap();
        assert_eq!((planned.cells, planned.resolution, planned.cell_size, planned.origin), ([8, 7], [9, 8], 24.0, old.origin));
        assert_eq!((planned.scatter_removed, planned.detail_lost), (0, false));
        assert_eq!((planned.bounds.min.z, planned.bounds.max.z), (40.25, 208.25));
        let report = apply(&mut m, s.terrain, &r).unwrap();
        let new = m.terrain(s.terrain).unwrap().clone();
        assert_eq!(new, old.rescaled(24.0).unwrap());

        let expect_a = vec![lift(a[0], &old, &new), lift(a[1], &old, &new), a[2]];
        let expect_f = vec![lift(f[0], &old, &new), f[1], lift(f[2], &old, &new), f[3]];
        let moved = expect_a.iter().zip(&a).chain(expect_f.iter().zip(&f)).filter(|(x, y)| x != y).count();
        assert_eq!(moved, 4);
        assert_eq!(report.scatter_moved, moved);
        assert_eq!(instances(&m, s.targeted), expect_a);
        assert_eq!(instances(&m, s.free), expect_f);
        assert_eq!(instances(&m, s.brush_only), b);
        assert!(plan(&m, s.terrain, &TerrainResize { cell_size: Some(24.5), ..Default::default() }).unwrap().detail_lost);
    }

    #[test]
    fn zero_fill_widens_the_planned_height_range_to_the_origin() {
        let (mut m, s) = scene();
        let old = m.terrain(s.terrain).unwrap().clone();
        assert!(old.bounds().min.y > 12.0);
        let r = TerrainResize { fill: EdgeFill::Zero, ..sides(0, 1, 0, 0) };
        let planned = plan(&m, s.terrain, &r).unwrap();
        assert_eq!((planned.bounds.min.y, planned.bounds.max.y), (12.0, old.bounds().max.y));
        assert_eq!(plan(&m, s.terrain, &TerrainResize { fill: EdgeFill::Zero, ..sides(0, -1, 0, 0) }).unwrap().bounds.min.y, old.bounds().min.y);
        let report = apply(&mut m, s.terrain, &r).unwrap();
        assert_eq!(report.plan.bounds, m.terrain(s.terrain).unwrap().bounds());
    }

    #[test]
    fn a_noop_changes_nothing() {
        let (mut m, s) = scene();
        let before = m.nodes.clone();
        let t = m.terrain(s.terrain).unwrap().clone();
        for r in [TerrainResize::default(), TerrainResize { cell_size: Some(32.0), ..Default::default() }] {
            assert!(r.is_noop(&t));
            let report = apply(&mut m, s.terrain, &r).unwrap();
            assert_eq!((report.scatter_moved, report.plan.scatter_removed, report.plan.cells), (0, 0, [6, 5]));
            assert_eq!(m.nodes, before);
        }

        assert!(!sides(0, 0, 0, 1).is_noop(&t));
        assert!(!TerrainResize { cell_size: Some(31.0), ..Default::default() }.is_noop(&t));
    }

    #[test]
    fn refusals_leave_the_map_untouched() {
        let (mut m, s) = scene();
        let cases: [(NodeId, TerrainResize, &str); 5] = [
            (NodeId(9999), sides(0, 0, 1, 0), "is not a terrain"),
            (s.brush, sides(0, 0, 1, 0), "is not a terrain"),
            (s.terrain, sides(0, 0, -3, -3), "along x"),
            (s.terrain, TerrainResize { cell_size: Some(0.0), ..Default::default() }, "cell size"),
            (s.terrain, TerrainResize { cell_size: Some(0.05), ..Default::default() }, "at most 2048"),
        ];
        let before = m.nodes.clone();
        for (id, r, error) in cases {
            assert!(plan(&m, id, &r).unwrap_err().contains(error), "{error}");
            assert!(apply(&mut m, id, &r).unwrap_err().contains(error), "{error}");
            assert_eq!(m.nodes, before);
        }

        for hide in [false, true] {
            let (mut m, s) = scene();
            let node = m.get_mut(s.terrain).unwrap();
            if hide {
                node.hidden = true;
            } else {
                node.locked = true;
            }

            let before = m.nodes.clone();
            assert!(apply(&mut m, s.terrain, &sides(0, 0, 1, 0)).unwrap_err().contains("hidden or locked"));
            assert_eq!(m.nodes, before);
        }
    }

    #[test]
    fn one_undo_step_and_an_exact_redo_through_try_edit() {
        let mut doc = Document::new();
        let s = doc.edit("setup", |m, _| build(m));
        let before = doc.map.nodes.clone();
        let r = TerrainResize { cell_size: Some(20.0), ..sides(2, -1, -1, 3) };
        doc.try_edit("Resize Terrain", |m, _| apply(m, s.terrain, &r)).unwrap();
        let after = doc.map.nodes.clone();
        assert_ne!(after, before);
        assert_eq!(doc.history.undo_labels().collect::<Vec<_>>(), ["Resize Terrain", "setup"]);

        doc.undo();
        assert_eq!(doc.map.nodes, before);
        let refused = doc.try_edit("Resize Terrain", |m, _| apply(m, s.terrain, &sides(0, 0, -7, 0)));
        assert!(refused.unwrap_err().contains("along x"));
        assert_eq!(doc.map.nodes, before);
        assert_eq!(doc.history.undo_labels().collect::<Vec<_>>(), ["setup"]);
        assert_eq!(doc.history.redo_labels().collect::<Vec<_>>(), ["Resize Terrain"]);

        doc.redo();
        assert_eq!(doc.map.nodes, after);
        assert_eq!(doc.map.terrain(s.terrain).unwrap().resolution, [14, 11]);
    }

    #[test]
    fn a_linked_copy_takes_the_new_size() {
        let mut doc = Document::new();
        let layer = doc.map.default_layer();
        let terrain = doc.edit("add", |m, s| {
            let id = m.insert(layer, NodeKind::Terrain(terrain()));
            s.select_node(id);
            id
        });
        doc.edit("group", |m, s| ops::group_selection(m, s, "hill", layer)).unwrap();
        let offset = DVec3::new(1000.5, 0.0, -40.0);
        let copy = doc.edit("link", |m, s| ops::duplicate_linked(m, s, offset, EditOptions::default()))[0];
        doc.try_edit("Resize Terrain", |m, _| apply(m, terrain, &sides(1, 0, -2, 3))).unwrap();
        // Syncing rebuilds the copy's children, so look its terrain up afterwards.
        let copy_terrain = doc.map.descendants(copy).into_iter().find(|id| doc.map.terrain(*id).is_some()).unwrap();
        let (a, b) = (doc.map.terrain(terrain).unwrap(), doc.map.terrain(copy_terrain).unwrap());
        assert_eq!(a.resolution, [8, 7]);
        assert_eq!(b, &a.translated(offset));
    }

    #[test]
    fn a_resized_terrain_round_trips_in_both_containers_and_a_second_file_keeps_its_own() {
        let (mut m, s) = scene();
        apply(&mut m, s.terrain, &TerrainResize { fill: EdgeFill::Zero, ..sides(2, 0, -1, 1) }).unwrap();
        let resized = m.terrain(s.terrain).unwrap().clone();
        assert!(!resized.splat.is_empty() && !resized.holes.is_empty());

        let node = format::node_to_json(&m, s.terrain).unwrap();
        let mut keys: Vec<&str> = node.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["cell_size", "chunk_cells", "heights", "holes", "id", "layers", "origin", "resolution", "splat", "type"]);

        let mut other = Map::new();
        let layer = other.default_layer();
        let other_terrain = Terrain::new(DVec3::new(500.0, -3.0, -700.0), [4, 9], 16.0, "sand");
        let other_id = other.insert(layer, NodeKind::Terrain(other_terrain.clone()));
        let other_bytes = format::to_bytes(&other).unwrap();

        let from_json = format::from_str(&format::to_string(&m)).unwrap();
        let from_binary = format::from_bytes(&format::to_bytes(&m).unwrap()).unwrap().map;
        for loaded in [from_json, from_binary] {
            assert_eq!(loaded.terrain(s.terrain), Some(&resized));
            for set in [s.targeted, s.free, s.brush_only] {
                let (got, want) = (instances(&loaded, set), instances(&m, set));
                assert_eq!(got.len(), want.len());
                assert!(got.iter().zip(&want).all(|(g, w)| g.position.distance(w.position) < 0.01));
            }

            let second = format::from_bytes(&other_bytes).unwrap().map;
            assert_eq!(second.terrain(other_id), Some(&other_terrain));
            assert_eq!(second.terrains().count(), 1);
            assert_eq!(second.scatters().count(), 0);
            assert_eq!(second.nodes.len(), other.nodes.len());
        }
    }
}
