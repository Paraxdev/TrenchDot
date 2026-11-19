use td_core::{DVec3, NodeId, Plane};

use crate::i18n::en;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolKind {
    /// Selection, moving, brush drawing, face dragging and entity gizmo handles (TrenchBroom default tool).
    Select,
    Clip,
    /// Draws an outline of corners and extrudes it into a brush, or several for a concave one.
    Polygon,
    Vertex,
    Rotate,
    Scale,
    /// Slides one side of the selection's bounds sideways, turning the box into a parallelepiped.
    Shear,
    /// Displacement and terrain sculpting.
    Sculpt,
    /// Material blending on terrains, displacements and faces with a blend material.
    Blend,
    /// Vertex color painting on brush and mesh faces.
    Paint,
    /// Blender style editing of mesh vertices, edges and faces.
    Mesh,
    /// Radial painting and erasing of trees, rocks and foliage into scatter layers.
    Scatter,
    /// Draws gameplay volumes: triggers, spawn areas, hurt and teleport zones.
    Volume,
    /// Places chains of path_corner entities.
    Path,
    /// Measures distances between two points.
    Measure,
    /// Hammer style texture application: apply, wrap, pick and slide textures on faces.
    Texture,
}

impl ToolKind {
    pub fn label(&self) -> &'static str {
        match self {
            ToolKind::Select => en!("Select"),
            ToolKind::Clip => en!("Clip"),
            ToolKind::Polygon => en!("Polygon"),
            ToolKind::Vertex => en!("Vertex"),
            ToolKind::Rotate => en!("Rotate"),
            ToolKind::Scale => en!("Scale"),
            ToolKind::Shear => en!("Shear"),
            ToolKind::Sculpt => en!("Sculpt"),
            ToolKind::Blend => en!("Blend"),
            ToolKind::Paint => en!("Paint"),
            ToolKind::Mesh => en!("Mesh"),
            ToolKind::Scatter => en!("Scatter"),
            ToolKind::Volume => en!("Volume"),
            ToolKind::Path => en!("Path"),
            ToolKind::Measure => en!("Measure"),
            ToolKind::Texture => en!("Texture"),
        }
    }

    pub fn all() -> [ToolKind; 16] {
        [
            ToolKind::Select,
            ToolKind::Polygon,
            ToolKind::Clip,
            ToolKind::Vertex,
            ToolKind::Rotate,
            ToolKind::Scale,
            ToolKind::Shear,
            ToolKind::Mesh,
            ToolKind::Sculpt,
            ToolKind::Blend,
            ToolKind::Paint,
            ToolKind::Scatter,
            ToolKind::Volume,
            ToolKind::Path,
            ToolKind::Measure,
            ToolKind::Texture,
        ]
    }

    pub fn family(self) -> ToolFamily {
        if TERRAIN_TOOLS.iter().any(|g| g.contains(&self)) { ToolFamily::Terrain } else { ToolFamily::Brush }
    }

    /// Accepts labels case insensitively, and "sprinkle" as the old name of the scatter tool.
    pub fn from_name(name: &str) -> Option<ToolKind> {
        if name.eq_ignore_ascii_case("sprinkle") {
            return Some(ToolKind::Scatter);
        }

        ToolKind::all().into_iter().find(|t| t.label().eq_ignore_ascii_case(name))
    }
}

/// The two blocks of the tool row and the Tools menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolFamily {
    Brush,
    Terrain,
}

impl ToolFamily {
    /// The name `get_state` reports.
    pub fn name(self) -> &'static str {
        match self {
            ToolFamily::Brush => "brush",
            ToolFamily::Terrain => "terrain",
        }
    }
}

/// Brush editing, then surfaces, meshes and the gameplay helpers. Sub groups are split by a small gap in the toolbar and a separator in the Tools menu.
pub const BRUSH_TOOLS: &[&[ToolKind]] = &[
    &[ToolKind::Select, ToolKind::Polygon, ToolKind::Clip, ToolKind::Vertex, ToolKind::Rotate, ToolKind::Scale, ToolKind::Shear],
    &[ToolKind::Texture],
    &[ToolKind::Mesh],
    &[ToolKind::Volume, ToolKind::Path, ToolKind::Measure],
];

pub const TERRAIN_TOOLS: &[&[ToolKind]] = &[&[ToolKind::Sculpt, ToolKind::Blend, ToolKind::Paint, ToolKind::Scatter]];

/// The families in toolbar order, brush work first.
pub const FAMILIES: [&[&[ToolKind]]; 2] = [BRUSH_TOOLS, TERRAIN_TOOLS];

/// An in-progress mouse drag in a viewport.
#[derive(Clone, Debug)]
pub enum Drag {
    Move {
        start: DVec3,
        plane: Plane,
        axis: Option<DVec3>,
        duplicate: bool,
        /// Shift held during the drag locks it to the main axis, which the preset may not want.
        lock: bool,
    },
    CreateBrush {
        start: DVec3,
        normal: DVec3,
        anchor: f64,
    },
    CreateBrush2d {
        start: DVec3,
        /// The selection's extent along the view axis when the brush started, for draw_depth_from_selection.
        depth: Option<(f64, f64)>,
    },
    FaceResize {
        faces: Vec<(NodeId, usize)>,
        origin: DVec3,
        normal: DVec3,
        /// The last distance every face could move, where the drag stays while the pointer asks for a collapse.
        last_ok: f64,
    },
    /// Dragging the side of terrain `id` facing `normal` in a 2D view, which adds or removes whole cells there.
    TerrainEdge {
        id: NodeId,
        origin: DVec3,
        normal: DVec3,
    },
    Look,
    Pan,
    Orbit {
        pivot: DVec3,
    },
    /// Dragging an entity gizmo handle.
    Gizmo {
        handle: crate::gizmos::GizmoHandle,
        start: DVec3,
    },
    /// Dragging a move, rotate or scale handle of the transform gizmo.
    Transform(crate::transform_gizmo::GizmoDrag),
    /// Drawing the Shape Generator's box from `start`, or moving its side facing `side`. Holds no transaction, the box
    /// is not part of the map.
    ShapeBounds {
        start: DVec3,
        side: Option<DVec3>,
    },
    /// Tool specific drags handled by the tool itself.
    Tool,
}

/// Closest point parameter on the line `origin + dir * s` to the ray.
pub fn line_ray_param(origin: DVec3, dir: DVec3, ray: &td_core::Ray) -> Option<f64> {
    let w0 = origin - ray.origin;
    let b = dir.dot(ray.dir);
    let d = dir.dot(w0);
    let e = ray.dir.dot(w0);
    let denom = dir.dot(dir) * ray.dir.dot(ray.dir) - b * b;
    if denom.abs() < 1e-6 {
        return None;
    }

    Some((b * e - ray.dir.dot(ray.dir) * d) / denom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_list_every_tool_once() {
        let listed: Vec<ToolKind> = FAMILIES.iter().flat_map(|f| f.iter()).flat_map(|g| g.iter().copied()).collect();
        assert_eq!(listed.len(), ToolKind::all().len());
        assert!(ToolKind::all().iter().all(|t| listed.iter().filter(|l| *l == t).count() == 1));
    }

    #[test]
    fn brush_tools_come_first_and_terrain_tools_are_their_own_family() {
        assert_eq!(FAMILIES[0], BRUSH_TOOLS);
        assert_eq!(FAMILIES[1], TERRAIN_TOOLS);
        assert_eq!(BRUSH_TOOLS[0][0], ToolKind::Select);
        assert!(BRUSH_TOOLS.iter().any(|g| g.contains(&ToolKind::Texture)));
        let terrain: Vec<ToolKind> = TERRAIN_TOOLS.iter().flat_map(|g| g.iter().copied()).collect();
        assert_eq!(terrain, [ToolKind::Sculpt, ToolKind::Blend, ToolKind::Paint, ToolKind::Scatter]);
        for t in ToolKind::all() {
            let want = if terrain.contains(&t) { ToolFamily::Terrain } else { ToolFamily::Brush };
            assert_eq!(t.family(), want, "{}", t.label());
        }
    }
}
