//! Sculpt and blend symmetry: the axis planes a dab is mirrored across and where they sit. Also the plane a mirrored
//! linked copy or a symmetric vertex edit uses.

use serde::{Deserialize, Serialize};
use td_core::{Aabb, AxisPlane, DVec3, Mirror, NodeId};
use td_geom::Terrain;

use crate::map::Map;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymmetryAxes {
    #[default]
    Off,
    X,
    Z,
    Xz,
}

impl SymmetryAxes {
    pub fn x(self) -> bool {
        matches!(self, SymmetryAxes::X | SymmetryAxes::Xz)
    }

    pub fn z(self) -> bool {
        matches!(self, SymmetryAxes::Z | SymmetryAxes::Xz)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymmetryCenter {
    /// Each terrain's middle, and the middle of the displacements and blend faces one dab edits.
    #[default]
    Terrain,
    World,
    /// At [`Symmetry::x`] and [`Symmetry::z`].
    Custom,
}

/// Which planes sculpt and blend dabs are mirrored across. Session state, never saved in a map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Symmetry {
    pub axes: SymmetryAxes,
    pub center: SymmetryCenter,
    /// World x of the X plane with a custom center.
    pub x: f64,
    /// World z of the Z plane with a custom center.
    pub z: f64,
}

impl Symmetry {
    pub const OFF: Symmetry = Symmetry { axes: SymmetryAxes::Off, center: SymmetryCenter::Terrain, x: 0.0, z: 0.0 };

    pub fn is_off(&self) -> bool {
        self.axes == SymmetryAxes::Off
    }

    pub fn for_terrain(&self, t: &Terrain) -> Mirror {
        let size = t.size();
        self.planes(t.origin + DVec3::new(size.x * 0.5, 0.0, size.y * 0.5))
    }

    /// Planes for displacements and blend faces edited together. A terrain center is the middle of their combined
    /// footprint, and there is none without faces.
    pub fn for_faces(&self, map: &Map, faces: &[(NodeId, usize)]) -> Mirror {
        if self.is_off() || self.center != SymmetryCenter::Terrain {
            return self.planes(DVec3::ZERO);
        }

        self.for_bounds(&faces_bounds(map, faces))
    }

    /// [`Symmetry::for_faces`] for faces whose [`faces_bounds`] are already known.
    pub fn for_bounds(&self, bounds: &Aabb) -> Mirror {
        if self.center == SymmetryCenter::Terrain && bounds.is_empty() { Mirror::NONE } else { self.planes(bounds.center()) }
    }

    fn planes(&self, middle: DVec3) -> Mirror {
        let at = match self.center {
            SymmetryCenter::Terrain => middle,
            SymmetryCenter::World => DVec3::ZERO,
            SymmetryCenter::Custom => DVec3::new(self.x, 0.0, self.z),
        };
        Mirror { x: self.axes.x().then_some(at.x), z: self.axes.z().then_some(at.z) }
    }
}

/// How far a vertex may sit from the mirror image of another and still count as its counterpart, and how close to a
/// mirror plane a seam vertex is welded onto it, in map units.
pub const MIRROR_TOLERANCE: f64 = 0.5;

/// Where along its axis a mirror plane sits, mostly relative to the bounds of what is mirrored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaneAt {
    Center,
    /// So a mirrored copy lands next to the original and the halves meet.
    #[default]
    Max,
    Min,
    World,
    /// At [`PlaneChoice::custom`].
    Custom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlaneChoice {
    pub at: PlaneAt,
    /// Coordinate along the axis of a custom plane.
    pub custom: f64,
}

impl PlaneChoice {
    /// The plane on `axis` for something within `bounds`, none when the choice depends on bounds that are empty.
    pub fn resolve(&self, axis: usize, bounds: &Aabb) -> Option<AxisPlane> {
        let at = match self.at {
            PlaneAt::World => 0.0,
            PlaneAt::Custom => self.custom,
            _ if bounds.is_empty() => return None,
            PlaneAt::Center => bounds.center()[axis],
            PlaneAt::Max => bounds.max[axis],
            PlaneAt::Min => bounds.min[axis],
        };
        Some(AxisPlane { axis, at })
    }
}

/// Which plane the Vertex and Mesh tools mirror vertex edits across. Session state, never saved in a map.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditSymmetry {
    /// 0, 1 or 2 for X, Y or Z, none while symmetry is off.
    pub axis: Option<usize>,
    pub plane: PlaneChoice,
}

impl Default for EditSymmetry {
    fn default() -> Self {
        Self::OFF
    }
}

impl EditSymmetry {
    pub const OFF: EditSymmetry = EditSymmetry { axis: None, plane: PlaneChoice { at: PlaneAt::Center, custom: 0.0 } };

    pub fn plane(&self, bounds: &Aabb) -> Option<AxisPlane> {
        self.plane.resolve(self.axis.filter(|a| *a < 3)?, bounds)
    }
}

/// The box around the corners of `faces`, empty without any.
pub fn faces_bounds(map: &Map, faces: &[(NodeId, usize)]) -> Aabb {
    Aabb::from_points(faces.iter().flat_map(|(id, face)| face_points(map, *id, *face)))
}

fn face_points(map: &Map, id: NodeId, face: usize) -> Vec<DVec3> {
    match map.brush(id) {
        Some(b) if face < b.faces.len() => b.face_points(face),
        Some(_) => Vec::new(),
        None => map.mesh(id).map(|m| m.face_points(face)).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use td_geom::{Brush, Mesh};

    use super::*;
    use crate::NodeKind;

    fn symmetry(axes: SymmetryAxes, center: SymmetryCenter) -> Symmetry {
        Symmetry { axes, center, x: 300.25, z: -77.5 }
    }

    #[test]
    fn terrain_planes_follow_the_axes_and_center() {
        // 17 by 9 cells of 16 units from an off origin corner: the middle is half of 272 by 144 past it.
        let t = Terrain::new(DVec3::new(-96.5, 12.0, 40.25), [18, 10], 16.0, "m");
        let (mx, mz) = (-96.5 + 136.0, 40.25 + 72.0);
        let cases = [
            (SymmetryAxes::Off, SymmetryCenter::Terrain, Mirror::NONE),
            (SymmetryAxes::Off, SymmetryCenter::Custom, Mirror::NONE),
            (SymmetryAxes::X, SymmetryCenter::Terrain, Mirror { x: Some(mx), z: None }),
            (SymmetryAxes::Z, SymmetryCenter::Terrain, Mirror { x: None, z: Some(mz) }),
            (SymmetryAxes::Xz, SymmetryCenter::Terrain, Mirror { x: Some(mx), z: Some(mz) }),
            (SymmetryAxes::Xz, SymmetryCenter::World, Mirror { x: Some(0.0), z: Some(0.0) }),
            (SymmetryAxes::X, SymmetryCenter::Custom, Mirror { x: Some(300.25), z: None }),
            (SymmetryAxes::Xz, SymmetryCenter::Custom, Mirror { x: Some(300.25), z: Some(-77.5) }),
        ];
        for (axes, center, want) in cases {
            assert_eq!(symmetry(axes, center).for_terrain(&t), want, "{axes:?} {center:?}");
        }

        assert!(Symmetry::OFF.is_off() && Symmetry::default() == Symmetry::OFF);
        assert!(!symmetry(SymmetryAxes::Z, SymmetryCenter::World).is_off());
    }

    #[test]
    fn face_planes_sit_at_the_middle_of_every_face_together() {
        let mut m = Map::new();
        let l = m.default_layer();
        let small = Brush::from_aabb(&Aabb::new(DVec3::new(-200.5, -16.0, 30.0), DVec3::new(-136.5, 4.0, 94.0)), "m").unwrap();
        let wide = Brush::from_aabb(&Aabb::new(DVec3::new(-136.5, -16.0, 30.0), DVec3::new(55.5, 9.0, 158.0)), "m").unwrap();
        let mesh = Mesh::from_brush(&Brush::from_aabb(&Aabb::new(DVec3::new(500.0, 0.0, -900.0), DVec3::new(564.0, 8.0, -836.0)), "m").unwrap());
        let top = |b: &Brush| b.faces.iter().position(|f| f.plane.normal.y > 0.5).unwrap();
        let (small_top, wide_top, mesh_faces) = (top(&small), top(&wide), mesh.faces.len());
        let (a, b, c) = (m.insert(l, NodeKind::Brush(small)), m.insert(l, NodeKind::Brush(wide)), m.insert(l, NodeKind::Mesh(mesh)));
        let xz = symmetry(SymmetryAxes::Xz, SymmetryCenter::Terrain);

        let both = [(a, small_top), (b, wide_top)];
        assert_eq!(xz.for_faces(&m, &both), Mirror { x: Some((-200.5 + 55.5) / 2.0), z: Some((30.0 + 158.0) / 2.0) });
        assert_eq!(xz.for_faces(&m, &both[..1]), Mirror { x: Some(-168.5), z: Some(62.0) });
        assert_eq!(symmetry(SymmetryAxes::Z, SymmetryCenter::Terrain).for_faces(&m, &both), Mirror { x: None, z: Some(94.0) });

        let mesh_and_stale = [(c, 0), (c, mesh_faces), (a, 99)];
        let mesh_mid = Aabb::from_points(m.mesh(c).unwrap().face_points(0)).center();
        assert_eq!(xz.for_faces(&m, &mesh_and_stale), Mirror { x: Some(mesh_mid.x), z: Some(mesh_mid.z) });

        assert_eq!(xz.for_faces(&m, &[]), Mirror::NONE);
        assert_eq!(xz.for_faces(&m, &[(a, 99), (c, mesh_faces)]), Mirror::NONE);
        assert_eq!(symmetry(SymmetryAxes::Off, SymmetryCenter::Terrain).for_faces(&m, &both), Mirror::NONE);
        assert_eq!(symmetry(SymmetryAxes::Xz, SymmetryCenter::World).for_faces(&m, &[]), Mirror { x: Some(0.0), z: Some(0.0) });
        assert_eq!(symmetry(SymmetryAxes::X, SymmetryCenter::Custom).for_faces(&m, &both), Mirror { x: Some(300.25), z: None });

        let sets: [&[(NodeId, usize)]; 4] = [&both, &both[..1], &mesh_and_stale, &[]];
        for s in [
            xz,
            symmetry(SymmetryAxes::Off, SymmetryCenter::Terrain),
            symmetry(SymmetryAxes::Z, SymmetryCenter::World),
            symmetry(SymmetryAxes::X, SymmetryCenter::Custom),
        ] {
            for faces in sets {
                assert_eq!(s.for_bounds(&faces_bounds(&m, faces)), s.for_faces(&m, faces), "{s:?} {faces:?}");
            }
        }
    }

    #[test]
    fn serde_uses_lowercase_names_and_refuses_unknown_fields() {
        let cases = [
            (SymmetryAxes::Off, SymmetryCenter::Terrain, json!({"axes": "off", "center": "terrain", "x": 300.25, "z": -77.5})),
            (SymmetryAxes::X, SymmetryCenter::World, json!({"axes": "x", "center": "world", "x": 300.25, "z": -77.5})),
            (SymmetryAxes::Z, SymmetryCenter::Custom, json!({"axes": "z", "center": "custom", "x": 300.25, "z": -77.5})),
            (SymmetryAxes::Xz, SymmetryCenter::Custom, json!({"axes": "xz", "center": "custom", "x": 300.25, "z": -77.5})),
        ];
        for (axes, center, value) in cases {
            let s = symmetry(axes, center);
            assert_eq!(serde_json::to_value(s).unwrap(), value);
            assert_eq!(serde_json::from_value::<Symmetry>(value).unwrap(), s);
        }

        let partial: Symmetry = serde_json::from_value(json!({"axes": "xz"})).unwrap();
        assert_eq!(partial, Symmetry { axes: SymmetryAxes::Xz, ..Symmetry::OFF });
        assert_eq!(serde_json::from_value::<Symmetry>(json!({})).unwrap(), Symmetry::OFF);
        assert!(serde_json::from_value::<Symmetry>(json!({"axes": "x", "y": 4.0})).is_err());
        assert!(serde_json::from_value::<Symmetry>(json!({"axes": "diagonal"})).is_err());
        assert!(serde_json::from_value::<Symmetry>(json!({"center": "brush"})).is_err());
        assert!(serde_json::from_value::<Symmetry>(json!("x")).is_err());
    }

    #[test]
    fn plane_choices_resolve_against_the_bounds() {
        let b = Aabb::new(DVec3::new(8.0, -30.0, 4.0), DVec3::new(40.0, 14.0, 21.0));
        let choice = |at| PlaneChoice { at, custom: -77.25 };
        let resolved = |at, bounds: &Aabb| [0, 1, 2].map(|axis| choice(at).resolve(axis, bounds).map(|p| (p.axis, p.at)));
        assert_eq!(resolved(PlaneAt::Center, &b), [Some((0, 24.0)), Some((1, -8.0)), Some((2, 12.5))]);
        assert_eq!(resolved(PlaneAt::Max, &b), [Some((0, 40.0)), Some((1, 14.0)), Some((2, 21.0))]);
        assert_eq!(resolved(PlaneAt::Min, &b), [Some((0, 8.0)), Some((1, -30.0)), Some((2, 4.0))]);
        assert_eq!(resolved(PlaneAt::World, &b), [Some((0, 0.0)), Some((1, 0.0)), Some((2, 0.0))]);
        assert_eq!(resolved(PlaneAt::Custom, &b), [Some((0, -77.25)), Some((1, -77.25)), Some((2, -77.25))]);

        for at in [PlaneAt::Center, PlaneAt::Max, PlaneAt::Min] {
            assert_eq!(resolved(at, &Aabb::EMPTY), [None; 3], "{at:?}");
        }

        assert_eq!(resolved(PlaneAt::World, &Aabb::EMPTY), [Some((0, 0.0)), Some((1, 0.0)), Some((2, 0.0))]);
        assert_eq!(resolved(PlaneAt::Custom, &Aabb::EMPTY), [Some((0, -77.25)), Some((1, -77.25)), Some((2, -77.25))]);
        assert_eq!(PlaneChoice::default(), PlaneChoice { at: PlaneAt::Max, custom: 0.0 });
    }

    #[test]
    fn edit_symmetry_resolves_only_while_on() {
        let b = Aabb::new(DVec3::new(8.0, -30.0, 4.0), DVec3::new(40.0, 14.0, 21.0));
        assert_eq!(EditSymmetry::default(), EditSymmetry::OFF);
        assert_eq!(EditSymmetry::OFF.plane(&b), None);
        assert_eq!(EditSymmetry { axis: Some(1), ..EditSymmetry::OFF }.plane(&b), Some(AxisPlane { axis: 1, at: -8.0 }));
        let custom = PlaneChoice { at: PlaneAt::Custom, custom: 96.5 };
        assert_eq!(EditSymmetry { axis: Some(2), plane: custom }.plane(&Aabb::EMPTY), Some(AxisPlane { axis: 2, at: 96.5 }));
        assert_eq!(EditSymmetry { axis: Some(0), ..EditSymmetry::OFF }.plane(&Aabb::EMPTY), None);
        assert_eq!(EditSymmetry { axis: Some(3), plane: custom }.plane(&b), None, "no fourth axis");
    }

    #[test]
    fn plane_settings_serde_uses_lowercase_names_and_refuses_unknown_fields() {
        let names = [(PlaneAt::Center, "center"), (PlaneAt::Max, "max"), (PlaneAt::Min, "min"), (PlaneAt::World, "world"), (PlaneAt::Custom, "custom")];
        for (at, name) in names {
            let choice = PlaneChoice { at, custom: -40.5 };
            let value = json!({"at": name, "custom": -40.5});
            assert_eq!(serde_json::to_value(choice).unwrap(), value);
            assert_eq!(serde_json::from_value::<PlaneChoice>(value).unwrap(), choice);
            let on = EditSymmetry { axis: Some(2), plane: choice };
            let value = json!({"axis": 2, "plane": {"at": name, "custom": -40.5}});
            assert_eq!(serde_json::to_value(on).unwrap(), value);
            assert_eq!(serde_json::from_value::<EditSymmetry>(value).unwrap(), on);
        }

        assert_eq!(serde_json::to_value(EditSymmetry::OFF).unwrap(), json!({"axis": null, "plane": {"at": "center", "custom": 0.0}}));
        assert_eq!(serde_json::from_value::<EditSymmetry>(json!({})).unwrap(), EditSymmetry::OFF);
        assert_eq!(serde_json::from_value::<EditSymmetry>(json!({"axis": 0})).unwrap(), EditSymmetry { axis: Some(0), ..EditSymmetry::OFF });
        assert_eq!(serde_json::from_value::<PlaneChoice>(json!({"custom": 3.0})).unwrap(), PlaneChoice { at: PlaneAt::Max, custom: 3.0 });
        assert!(serde_json::from_value::<PlaneChoice>(json!({"at": "max", "offset": 3.0})).is_err());
        assert!(serde_json::from_value::<PlaneChoice>(json!({"at": "middle"})).is_err());
        assert!(serde_json::from_value::<EditSymmetry>(json!({"axis": 0, "center": "world"})).is_err());
        assert!(serde_json::from_value::<EditSymmetry>(json!({"axis": "x"})).is_err());
    }
}
