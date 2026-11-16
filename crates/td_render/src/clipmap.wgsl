// clipmap::wgsl_consts() puts MESH_SIZE, MAP_SIDE, LOC_MIN, NO_REGION and COARSE_ONLY in front of this file.
// Positions in cells are Terrain3D vertex grid coordinates, x along world x and y along world z.

struct Clipmap {
    // x: region size in cells, y: cells between coarse texels, zw: region map location of the coarse atlas's first texel
    grid: vec4<i32>,
    // x: map units per cell, y: map units per meter, z: ground level in meters, w: region blend
    world: vec4<f32>,
    // x: background (0 none, 1 flat, 2 noise), y: 1 when selected
    look: vec4<f32>,
    // rgb: wireframe line color
    wire: vec4<f32>,
    // Fine layer per region map location, COARSE_ONLY or NO_REGION, four to an element.
    regions: array<vec4<i32>, 256>,
};

@group(1) @binding(0) var fine_heights: texture_2d_array<f32>;
@group(1) @binding(1) var fine_control: texture_2d_array<u32>;
@group(1) @binding(2) var fine_color: texture_2d_array<f32>;
@group(1) @binding(3) var coarse_heights: texture_2d<f32>;
@group(1) @binding(4) var coarse_control: texture_2d<u32>;
@group(1) @binding(5) var<uniform> clip: Clipmap;

struct VIn {
    @location(0) local: vec2<u32>,
    @location(1) origin: vec2<i32>,
    @location(2) level: u32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

fn floor_div(a: vec2<i32>, b: i32) -> vec2<i32> {
    let q = a / b;
    return q - vec2<i32>(a % b < vec2<i32>(0));
}

fn slot_at(loc: vec2<i32>) -> i32 {
    let p = loc - LOC_MIN;
    if (any(p < vec2<i32>(0)) || any(p >= vec2<i32>(MAP_SIDE))) {
        return NO_REGION;
    }
    let i = p.y * MAP_SIDE + p.x;
    return clip.regions[i / 4][i % 4];
}

fn loc_of(cell: vec2<i32>) -> vec2<i32> {
    return floor_div(cell, clip.grid.x);
}

fn present(cell: vec2<i32>) -> bool {
    return slot_at(loc_of(cell)) != NO_REGION;
}

fn coarse_texel(cell: vec2<i32>) -> f32 {
    return textureLoad(coarse_heights, (cell - clip.grid.zw * clip.grid.x) / clip.grid.y, 0).r;
}

// A vertex of the coarse grid inside the region `lo` to `hi`. A vertex past the edge of the regions repeats the
// region's own edge, so the last coarse cell does not slope toward nothing.
fn coarse_vertex(cell: vec2<i32>, lo: vec2<i32>, hi: vec2<i32>) -> f32 {
    let loc = loc_of(cell);
    let slot = slot_at(loc);
    if (slot >= 0) {
        return textureLoad(fine_heights, cell - loc * clip.grid.x, slot, 0).r;
    }
    if (slot == COARSE_ONLY) {
        return coarse_texel(cell);
    }
    return coarse_texel(clamp(cell, lo, hi));
}

fn coarse_height(cell: vec2<i32>, loc: vec2<i32>) -> f32 {
    let s = clip.grid.y;
    let base = floor_div(cell, s) * s;
    let f = vec2<f32>(cell - base) / f32(s);
    let lo = loc * clip.grid.x;
    let hi = lo + vec2<i32>(clip.grid.x - s);
    let h00 = coarse_vertex(base, lo, hi);
    let h10 = coarse_vertex(base + vec2<i32>(s, 0), lo, hi);
    let h01 = coarse_vertex(base + vec2<i32>(0, s), lo, hi);
    let h11 = coarse_vertex(base + vec2<i32>(s, s), lo, hi);
    return mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y);
}

// Region data in meters at a vertex, 0 where no region holds it, like Terrain3D reading a missing layer.
fn data_height(cell: vec2<i32>) -> f32 {
    let loc = loc_of(cell);
    let slot = slot_at(loc);
    if (slot >= 0) {
        return textureLoad(fine_heights, cell - loc * clip.grid.x, slot, 0).r;
    }
    if (slot == COARSE_ONLY) {
        return coarse_height(cell, loc);
    }
    return 0.0;
}

// The height a vertex is drawn at before the background blend. With no background a vertex just past the regions
// repeats the edge it borders, the fragment shader then drops the cells that touch it, as picking misses them.
fn raw_height(cell: vec2<i32>) -> f32 {
    if (clip.look.x > 0.5 || present(cell)) {
        return data_height(cell);
    }
    for (var k = 0; k < 3; k++) {
        let back = cell - vec2<i32>(i32(k != 1), i32(k != 0));
        if (present(back)) {
            return data_height(back);
        }
    }
    return 0.0;
}

fn edge_weight(f: f32, factor: f32) -> f32 {
    let g = clamp(f, 1e-8, 1.0 - 1e-8);
    return 1.0 / (1.0 + exp(min(factor * log((1.0 - g) / g), 80.0)));
}

fn present_at(uv: vec2<f32>) -> f32 {
    return f32(slot_at(vec2<i32>(floor(uv))) != NO_REGION);
}

// Terrain3D's region blend, td_geom's `edge_blended`: a height pulled toward the ground level near the edge of the
// regions. `pos` is in cells.
fn blended(pos: vec2<f32>, h: f32) -> f32 {
    if (clip.look.x < 0.5) {
        return h;
    }
    // The bias matches Terrain3D's shader.
    let uv = pos / f32(clip.grid.x) - 0.5011;
    let a = present_at(uv + vec2<f32>(0.0, 1.0));
    let b = present_at(uv + vec2<f32>(1.0, 1.0));
    let c = present_at(uv + vec2<f32>(1.0, 0.0));
    let d = present_at(uv);
    let factor = 2.0 + 126.0 * (1.0 - clip.world.w);
    let f = uv - floor(uv);
    let wx = edge_weight(f.x, factor);
    let wy = edge_weight(f.y, factor);
    let s = clamp((1.0 - mix(mix(d, c, wx), mix(a, b, wx), wy)) * 2.0, 0.0, 1.0);
    return h + s * s * (3.0 - 2.0 * s) * (clip.world.z - h);
}

fn ground(cell: vec2<i32>) -> f32 {
    return blended(vec2<f32>(cell), raw_height(cell));
}

// Normal from the heights `step` cells around a vertex, in map units.
fn ground_normal(cell: vec2<i32>, step: i32) -> vec3<f32> {
    let dx = ground(cell + vec2<i32>(step, 0)) - ground(cell - vec2<i32>(step, 0));
    let dz = ground(cell + vec2<i32>(0, step)) - ground(cell - vec2<i32>(0, step));
    return normalize(vec3<f32>(-dx * clip.world.y, 2.0 * f32(step) * clip.world.x, -dz * clip.world.y));
}

// The twin of layout::Morph: odd vertices slide onto the next level's grid near the outer edge of their ring.
@vertex
fn vs_main(v: VIn) -> VOut {
    let scale = 1 << v.level;
    let cell = v.origin + vec2<i32>(v.local) * scale;
    let target_cells = cam.eye.xz / clip.world.x;
    let away = max(abs(f32(cell.x) - target_cells.x), abs(f32(cell.y) - target_cells.y)) / f32(scale);
    let t = clamp((away - MESH_SIZE - 4.0) / (MESH_SIZE - 4.0), 0.0, 1.0);
    let lerp = t * t * (3.0 - 2.0 * t);
    let r = (cell >> vec2<u32>(v.level)) & vec2<i32>(3);
    let shift = vec2<i32>(r == vec2<i32>(1)) - vec2<i32>(r == vec2<i32>(3));
    let end = cell + shift * scale;
    let pos = mix(vec2<f32>(cell), vec2<f32>(end), lerp);
    let h = blended(pos, mix(raw_height(cell), raw_height(end), lerp));
    var n = ground_normal(cell, scale);
    if (lerp > 0.0) {
        n = normalize(mix(n, ground_normal(end, scale * 2), lerp));
    }

    var o: VOut;
    o.world = vec3<f32>(pos.x * clip.world.x, h * clip.world.y, pos.y * clip.world.x);
    o.clip = cam.view_proj * vec4<f32>(o.world, 1.0);
    o.normal = n;
    return o;
}

// With no background, whether the level 0 triangle under a pixel has a corner no region holds. Cells split along the
// alternating diagonal like layout::MeshKind::indices, so this drops exactly the triangles picking misses.
fn outside(world: vec3<f32>) -> bool {
    if (clip.look.x > 0.5) {
        return false;
    }
    let p = world.xz / clip.world.x;
    let c = vec2<i32>(floor(p));
    let f = p - floor(p);
    var a = vec2<i32>(0, 0);
    var b = vec2<i32>(1, 1);
    var k = vec2<i32>(1, 0);
    if (((c.x + c.y) & 1) == 0) {
        if (f.y >= f.x) {
            k = vec2<i32>(0, 1);
        }
    } else {
        a = vec2<i32>(0, 1);
        b = vec2<i32>(1, 0);
        if (f.x + f.y < 1.0) {
            k = vec2<i32>(0, 0);
        } else {
            k = vec2<i32>(1, 1);
        }
    }
    return !(present(c + a) && present(c + b) && present(c + k));
}

@fragment
fn fs_main(f: VOut) -> @location(0) vec4<f32> {
    if (outside(f.world)) {
        discard;
    }
    let n = normalize(f.normal);
    let flat_mode = step(0.5, cam.params.z) * step(cam.params.z, 1.5);
    var base = mix(vec3<f32>(0.62, 0.6, 0.55), vec3<f32>(0.8), flat_mode);
    base = mix(base, base * vec3<f32>(1.0, 0.62, 0.58), clip.look.y);
    var rgb: vec3<f32>;
    if (is_lit()) {
        rgb = tonemap(base * light_at(f.world, n));
    } else {
        rgb = base * relief(n);
    }
    rgb = apply_fog(rgb, f.world);
    let grid_strength = select(0.12, 0.03, is_lit());
    rgb = mix(rgb, vec3<f32>(1.0), grid_amount(f.world, n) * grid_strength);
    return vec4<f32>(rgb, 1.0);
}

@fragment
fn fs_wire(f: VOut) -> @location(0) vec4<f32> {
    if (outside(f.world)) {
        discard;
    }
    if (cam.viewport.w > 0.0) {
        return vec4<f32>(flip(clip.wire.rgb, cam.viewport.w), 1.0);
    }
    return vec4<f32>(clip.wire.rgb, 1.0);
}
