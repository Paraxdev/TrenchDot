// The ring vertices every clipmap shader shares. MESH_SIZE goes in front of this file, see clipmap::wgsl_consts().
// Positions in cells are vertex grid coordinates, x along world x and y along world z.

struct VIn {
    @location(0) local: vec2<u32>,
    @location(1) origin: vec2<i32>,
    @location(2) level: u32,
};

struct RingMorph {
    cell: vec2<i32>,
    end: vec2<i32>,
    scale: i32,
    lerp: f32,
};

// The twin of layout::Morph: odd vertices slide onto the next level's grid near the outer edge of their ring.
fn ring_morph(v: VIn, eye_cells: vec2<f32>) -> RingMorph {
    let scale = 1 << v.level;
    let cell = v.origin + vec2<i32>(v.local) * scale;
    let away = max(abs(f32(cell.x) - eye_cells.x), abs(f32(cell.y) - eye_cells.y)) / f32(scale);
    let t = clamp((away - MESH_SIZE - 4.0) / (MESH_SIZE - 4.0), 0.0, 1.0);
    let r = (cell >> vec2<u32>(v.level)) & vec2<i32>(3);
    let shift = vec2<i32>(r == vec2<i32>(1)) - vec2<i32>(r == vec2<i32>(3));
    return RingMorph(cell, cell + shift * scale, scale, t * t * (3.0 - 2.0 * t));
}

// The corners of the triangle under `f` in cell `c`, which splits along the alternating diagonal like
// layout::MeshKind::indices.
fn triangle(c: vec2<i32>, f: vec2<f32>) -> array<vec2<i32>, 3> {
    if (((c.x + c.y) & 1) == 0) {
        return array<vec2<i32>, 3>(vec2<i32>(0, 0), vec2<i32>(1, 1), select(vec2<i32>(1, 0), vec2<i32>(0, 1), f.y >= f.x));
    }
    return array<vec2<i32>, 3>(vec2<i32>(0, 1), vec2<i32>(1, 0), select(vec2<i32>(1, 1), vec2<i32>(0, 0), f.x + f.y < 1.0));
}
