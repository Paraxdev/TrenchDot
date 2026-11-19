// A TrenchDot terrain drawn with the clipmap rings, see clipmap/native.rs. Groups 1 and 2 are the chunk path's
// material and light map, so both paths shade through terrain_shade. With 4 groups and 15 of 16 sampled textures in
// the fragment stage this layout sits at the default limits, a texture added to terrain_look.wgsl breaks it.

struct TerrainClip {
    // xyz: world position of the first vertex, w: map units per cell
    origin: vec4<f32>,
    // xy: cells along x and z, z: 1 when selected, w: 1 with a lightmap chart
    grid: vec4<u32>,
    chart: array<vec4<f32>, 2>,
};

@group(3) @binding(0) var clip_heights: texture_2d<f32>;
@group(3) @binding(1) var clip_paint: texture_2d<u32>;
@group(3) @binding(2) var<uniform> clip: TerrainClip;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

fn last_vertex() -> vec2<i32> {
    return vec2<i32>(clip.grid.xy);
}

fn height_at(cell: vec2<i32>) -> f32 {
    return textureLoad(clip_heights, clamp(cell, vec2<i32>(0), last_vertex()), 0).r;
}

// Central differences `step` cells around a vertex with the neighbors held inside the terrain, at level 0 exactly
// Terrain::normal.
fn ground_normal(cell: vec2<i32>, step: i32) -> vec3<f32> {
    let dx = height_at(cell + vec2<i32>(step, 0)) - height_at(cell - vec2<i32>(step, 0));
    let dz = height_at(cell + vec2<i32>(0, step)) - height_at(cell - vec2<i32>(0, step));
    return normalize(vec3<f32>(-dx, 2.0 * f32(step) * clip.origin.w, -dz));
}

struct ClipVertex {
    world: vec3<f32>,
    morph: RingMorph,
};

fn clip_vertex(v: VIn) -> ClipVertex {
    let m = ring_morph(v, (cam.eye.xz - clip.origin.xz) / clip.origin.w);
    let pos = mix(vec2<f32>(m.cell), vec2<f32>(m.end), m.lerp);
    let h = mix(height_at(m.cell), height_at(m.end), m.lerp);
    return ClipVertex(clip.origin.xyz + vec3<f32>(pos.x * clip.origin.w, h, pos.y * clip.origin.w), m);
}

@vertex
fn vs_main(v: VIn) -> VOut {
    let r = clip_vertex(v);
    let m = r.morph;
    var n = ground_normal(m.cell, m.scale);
    if (m.lerp > 0.0) {
        n = normalize(mix(n, ground_normal(m.end, m.scale * 2), m.lerp));
    }

    var o: VOut;
    o.clip = cam.view_proj * vec4<f32>(r.world, 1.0);
    o.world = r.world;
    o.normal = n;
    return o;
}

// Cells from the first vertex.
fn local_cells(world: vec3<f32>) -> vec2<f32> {
    return (world.xz - clip.origin.xz) / clip.origin.w;
}

fn cell_under(p: vec2<f32>) -> vec2<i32> {
    return clamp(vec2<i32>(floor(p)), vec2<i32>(0), last_vertex() - vec2<i32>(1));
}

// Past the terrain's edge or over a hole. Far rings cover more than the terrain, the rest is cut here. The edge keeps a
// thousandth of a cell, or f32 rounding leaves gaps along it where two terrains are tiled side by side.
fn dropped(p: vec2<f32>) -> bool {
    if (any(p < vec2<f32>(-1e-3)) || any(p > vec2<f32>(clip.grid.xy) + 1e-3)) {
        return true;
    }
    return (textureLoad(clip_paint, cell_under(p), 0).y & 1u) != 0u;
}

// Like Terrain::weights, splat bytes that sum to 0 are the first layer alone.
fn vertex_weights(cell: vec2<i32>) -> vec4<f32> {
    let bytes = textureLoad(clip_paint, cell, 0).x;
    let w = vec4<f32>((vec4<u32>(bytes) >> vec4<u32>(0u, 8u, 16u, 24u)) & vec4<u32>(255u));
    let sum = w.x + w.y + w.z + w.w;
    if (sum <= 0.0) {
        return vec4<f32>(1.0, 0.0, 0.0, 0.0);
    }
    return w / sum;
}

// The corner weights of the level 0 triangle under `p` mixed by where `p` sits in it, what the chunk meshes
// interpolate, so far rings keep the paint of the near ones.
fn paint_weights(p: vec2<f32>) -> vec4<f32> {
    let c = cell_under(p);
    let f = p - vec2<f32>(c);
    let t = triangle(c, f);
    let a = vec2<f32>(t[0]);
    let e0 = vec2<f32>(t[1]) - a;
    let e1 = vec2<f32>(t[2]) - a;
    let q = f - a;
    let det = e0.x * e1.y - e1.x * e0.y;
    let u = (q.x * e1.y - e1.x * q.y) / det;
    let v = (e0.x * q.y - q.x * e0.y) / det;
    return vertex_weights(c + t[0]) * (1.0 - u - v) + vertex_weights(c + t[1]) * u + vertex_weights(c + t[2]) * v;
}

fn lightmap_uv(world: vec3<f32>) -> vec2<f32> {
    if (clip.grid.w == 0u) {
        return vec2<f32>(-1.0);
    }
    let p = vec4<f32>(world, 1.0);
    return vec2<f32>(dot(clip.chart[0], p), dot(clip.chart[1], p));
}

@fragment
fn fs_main(f: VOut) -> @location(0) vec4<f32> {
    let p = local_cells(f.world);
    if (dropped(p)) {
        discard;
    }
    return terrain_shade(f.world, f.normal, paint_weights(p), f32(clip.grid.z), lightmap_uv(f.world));
}

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

// In the shadow pass `cam` holds the sun's view projection and the eye the rings follow.
@vertex
fn vs_shadow(v: VIn) -> ShadowOut {
    let world = clip_vertex(v).world;
    return ShadowOut(cam.view_proj * vec4<f32>(world, 1.0), world);
}

// Holes and the ground past the edge let the sun through, like they draw nothing.
@fragment
fn fs_shadow(f: ShadowOut) {
    if (dropped(local_cells(f.world))) {
        discard;
    }
}
