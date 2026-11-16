// clipmap::wgsl_consts() puts MESH_SIZE, MAP_SIDE, LOC_MIN, NO_REGION, COARSE_ONLY and the control word shifts in
// front of this file. Positions in cells are Terrain3D vertex grid coordinates, x along world x and y along world z.

struct Texture {
    // x: uv scale, y: detiling rotation, z: detiling shift
    uv: vec4<f32>,
    // rgb: albedo color
    color: vec4<f32>,
    // rgb: linear mean of the albedo, drawn when textures are off
    mean: vec4<f32>,
};

struct Clipmap {
    // x: region size in cells, y: cells between coarse texels, zw: region map location of the coarse atlas's first texel
    grid: vec4<i32>,
    // x: map units per cell, y: map units per meter, z: ground level in meters, w: region blend
    world: vec4<f32>,
    // x: background (0 none, 1 flat, 2 noise), y: 1 when selected, z: textures (0 none listed, 1 mean colors,
    // 2 sampled), w: 1 with the auto shader on
    look: vec4<f32>,
    // rgb: wireframe line color
    wire: vec4<f32>,
    // x: auto slope, y: auto height reduction, z: auto base texture, w: auto overlay texture
    auto_shader: vec4<f32>,
    // Terrain3D's world noise. x: scale, y: height in meters, z: lod distance in meters, w: most octaves
    noise: vec4<f32>,
    // xyz: Terrain3D's world noise offset, w: fewest octaves
    noise_offset: vec4<f32>,
    // Fine layer per region map location, COARSE_ONLY or NO_REGION, four to an element.
    regions: array<vec4<i32>, 256>,
    textures: array<Texture, 32>,
};

@group(1) @binding(0) var fine_heights: texture_2d_array<f32>;
@group(1) @binding(1) var fine_control: texture_2d_array<u32>;
@group(1) @binding(2) var fine_color: texture_2d_array<f32>;
@group(1) @binding(3) var coarse_heights: texture_2d<f32>;
@group(1) @binding(4) var coarse_control: texture_2d<u32>;
@group(1) @binding(5) var<uniform> clip: Clipmap;
@group(1) @binding(6) var albedo: texture_2d_array<f32>;
@group(1) @binding(7) var albedo_sampler: sampler;

const TAU: f32 = 6.283185307179586;
const PLAIN_GROUND: vec3<f32> = vec3<f32>(0.62, 0.6, 0.55);

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

// Terrain3D's world noise from its backgrounds.glsl, the twins are in clipmap::noise.
fn hashv2(v: vec2<f32>) -> f32 {
    return fract(1e4 * sin(fma(17.0, v.x, v.y * 0.1)) * (0.1 + abs(sin(fma(v.y, 13.0, v.x)))));
}

fn noise2d(x: vec2<f32>) -> vec3<f32> {
    let f = fract(x);
    let f2 = f * f;
    let s = f - 1.0;
    let u = f2 * f * fma(vec2<f32>(6.0), f2, fma(vec2<f32>(-15.0), f, vec2<f32>(10.0)));
    let du = 30.0 * f2 * s * s;
    let p = floor(x);
    let a = hashv2(p);
    let b = hashv2(p + vec2<f32>(1.0, 0.0));
    let c = hashv2(p + vec2<f32>(0.0, 1.0));
    let d = hashv2(p + vec2<f32>(1.0, 1.0));
    let k1 = b - a;
    let k2 = c - a;
    let k3 = d - (b + k2);
    return vec3<f32>(fma(k2, u.y, fma(u.x, fma(k3, u.y, k1), a)), du * fma(vec2<f32>(k3), u.yx, vec2<f32>(k1, k2)));
}

fn world_noise(start: vec2<f32>, octaves: i32) -> f32 {
    var p = start;
    var a = 0.0;
    var b = 1.0;
    var d = vec2<f32>(0.0);
    for (var i = 0; i < octaves; i++) {
        let n = noise2d(p);
        d += n.yz;
        a += b * n.x / (1.0 + dot(d, d));
        b *= 0.5;
        p = mat2x2<f32>(vec2<f32>(0.8, -0.6), vec2<f32>(0.6, 0.8)) * p * 2.0;
    }
    return a;
}

// Octaves of the noise at a vertex, fewer with each lod distance between it and the eye.
fn noise_octaves(cell: vec2<i32>) -> i32 {
    let dist = length(vec2<f32>(cell) * clip.world.x - cam.eye.xz) / clip.world.y;
    let steps = select(1e30, floor(dist / clip.noise.z), clip.noise.z > 0.0);
    return i32(min(max(clip.noise.w - steps, clip.noise_offset.w), clip.noise.w));
}

// Terrain3D's `get_noise_region_blend`, `uv` in regions.
fn noise_weight(uv: vec2<f32>) -> f32 {
    let g = uv - 0.5011;
    let a = present_at(g + vec2<f32>(0.0, 1.0));
    let b = present_at(g + vec2<f32>(1.0, 1.0));
    let c = present_at(g + vec2<f32>(1.0, 0.0));
    let d = present_at(g);
    let w = smoothstep(vec2<f32>(0.0), vec2<f32>(1.0), fract(g));
    return 1.0 - mix(mix(d, c, w.x), mix(a, b, w.x), w.y) * 2.0;
}

// The meters a Noise background raises the ground at `pos` in cells, nothing near the regions.
fn noise_height(pos: vec2<f32>, octaves: i32) -> f32 {
    if (clip.look.x < 1.5) {
        return 0.0;
    }
    let size = f32(clip.grid.x);
    let uv = pos / size;
    let weight = noise_weight(uv);
    if (weight <= 0.5) {
        return 0.0;
    }
    let p = (uv + clip.noise_offset.xz * 1024.0 / size) * clip.noise.x * size / 1024.0 * 0.1;
    let h = world_noise(p, octaves) * clip.noise.y * 10.0 + clip.noise_offset.y * 100.0;
    return smoothstep(0.5, 1.0, weight) * h;
}

fn ground(cell: vec2<i32>, octaves: i32) -> f32 {
    return blended(vec2<f32>(cell), raw_height(cell)) + noise_height(vec2<f32>(cell), octaves);
}

// Normal from the heights `step` cells around a vertex, in map units.
fn ground_normal(cell: vec2<i32>, step: i32, octaves: i32) -> vec3<f32> {
    let dx = ground(cell + vec2<i32>(step, 0), octaves) - ground(cell - vec2<i32>(step, 0), octaves);
    let dz = ground(cell + vec2<i32>(0, step), octaves) - ground(cell - vec2<i32>(0, step), octaves);
    return normalize(vec3<f32>(-dx * clip.world.y, 2.0 * f32(step) * clip.world.x, -dz * clip.world.y));
}

struct RingVertex {
    world: vec3<f32>,
    cell: vec2<i32>,
    end: vec2<i32>,
    scale: i32,
    lerp: f32,
    octaves: i32,
};

// The twin of layout::Morph: odd vertices slide onto the next level's grid near the outer edge of their ring.
fn ring_vertex(v: VIn) -> RingVertex {
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
    let octaves = noise_octaves(cell);
    var noise = noise_height(vec2<f32>(cell), octaves);
    if (lerp > 0.0) {
        noise = mix(noise, noise_height(vec2<f32>(end), octaves), lerp);
    }
    let h = blended(pos, mix(raw_height(cell), raw_height(end), lerp)) + noise;
    return RingVertex(vec3<f32>(pos.x * clip.world.x, h * clip.world.y, pos.y * clip.world.x), cell, end, scale, lerp, octaves);
}

@vertex
fn vs_main(v: VIn) -> VOut {
    let r = ring_vertex(v);
    var n = ground_normal(r.cell, r.scale, r.octaves);
    if (r.lerp > 0.0) {
        n = normalize(mix(n, ground_normal(r.end, r.scale * 2, r.octaves), r.lerp));
    }

    var o: VOut;
    o.world = r.world;
    o.clip = cam.view_proj * vec4<f32>(r.world, 1.0);
    o.normal = n;
    return o;
}

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

// In the shadow pass `cam` holds the sun's view projection and the eye the ring follows.
@vertex
fn vs_shadow(v: VIn) -> ShadowOut {
    let world = ring_vertex(v).world;
    return ShadowOut(cam.view_proj * vec4<f32>(world, 1.0), world);
}

// The corners of the triangle under `f` in cell `c`, which splits along the alternating diagonal like
// layout::MeshKind::indices.
fn triangle(c: vec2<i32>, f: vec2<f32>) -> array<vec2<i32>, 3> {
    if (((c.x + c.y) & 1) == 0) {
        return array<vec2<i32>, 3>(vec2<i32>(0, 0), vec2<i32>(1, 1), select(vec2<i32>(1, 0), vec2<i32>(0, 1), f.y >= f.x));
    }
    return array<vec2<i32>, 3>(vec2<i32>(0, 1), vec2<i32>(1, 0), select(vec2<i32>(1, 1), vec2<i32>(0, 0), f.x + f.y < 1.0));
}

// With no background, whether the level 0 triangle under a pixel has a corner no region holds, so this drops exactly
// the triangles picking misses.
fn outside(world: vec3<f32>) -> bool {
    if (clip.look.x > 0.5) {
        return false;
    }
    let p = world.xz / clip.world.x;
    let c = vec2<i32>(floor(p));
    let t = triangle(c, p - floor(p));
    return !(present(c + t[0]) && present(c + t[1]) && present(c + t[2]));
}

// The control word at a vertex. Where no region holds it, a lone auto bit: Terrain3D's auto shader paints there, and
// otherwise it decodes like the empty word Terrain3D reads.
fn control_at(cell: vec2<i32>) -> u32 {
    let loc = loc_of(cell);
    let slot = slot_at(loc);
    if (slot >= 0) {
        return textureLoad(fine_control, cell - loc * clip.grid.x, slot, 0).r;
    }
    if (slot == COARSE_ONLY) {
        return textureLoad(coarse_control, (cell - clip.grid.zw * clip.grid.x) / clip.grid.y, 0).r;
    }
    return 1u << AUTO_BIT;
}

// Terrain3D's color map at a vertex, white where only the coarse atlas or nothing holds it.
fn color_at(cell: vec2<i32>) -> vec3<f32> {
    let loc = loc_of(cell);
    let slot = slot_at(loc);
    if (slot >= 0) {
        return textureLoad(fine_color, cell - loc * clip.grid.x, slot, 0).rgb;
    }
    return vec3<f32>(1.0);
}

// Cells between the vertices whose control words a pixel at `p` cells reads: the coarse texels in a region only the
// atlas holds.
fn control_step(p: vec2<f32>) -> i32 {
    return select(1, clip.grid.y, slot_at(loc_of(vec2<i32>(floor(p)))) == COARSE_ONLY);
}

fn is_hole(cell: vec2<i32>) -> bool {
    return ((control_at(cell) >> HOLE_BIT) & 1u) == 1u;
}

// Terrain3D drops every triangle with a hole at a corner. Here that holds per pixel, at every distance.
fn in_hole(world: vec3<f32>) -> bool {
    let stride = control_step(world.xz / clip.world.x);
    let q = world.xz / (clip.world.x * f32(stride));
    let c = vec2<i32>(floor(q));
    let t = triangle(c, q - floor(q));
    return is_hole((c + t[0]) * stride) || is_hole((c + t[1]) * stride) || is_hole((c + t[2]) * stride);
}

fn dropped(world: vec3<f32>) -> bool {
    return outside(world) || in_hole(world);
}

fn rotate(v: vec2<f32>, cs: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(cs.x * v.x + cs.y * v.y, cs.x * v.y - cs.y * v.x);
}

fn random(xy: vec2<f32>) -> f32 {
    return fract(sin(dot(xy, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

fn control_scale(control: u32) -> f32 {
    return 0.9 - f32(((((control >> SCALE_SHIFT) & 7u) + 3u) % 8u) + 1u) * 0.1;
}

// Where Terrain3D samples texture `id` at `world` meters, in a cell whose corner vertex at `corner` meters holds
// `control`. The twin of look::texture_uv.
fn texture_uv(id: u32, world: vec2<f32>, corner: vec2<f32>, control: u32) -> vec2<f32> {
    let scale = control_scale(control);
    let angle = f32((control >> ANGLE_SHIFT) & 15u) * (-TAU / 16.0);
    let turn = vec2<f32>(cos(angle), sin(angle));
    let uv = rotate(world * scale, turn);
    let pos = rotate(corner * scale, turn);
    let t = clip.textures[id].uv;
    let center = floor(pos * t.x + 0.5);
    let tile = (random(center) * 2.0 - 1.0) * TAU;
    let spin = tile * t.y;
    return rotate(uv * t.x - center, vec2<f32>(cos(spin), sin(spin))) + center + tile * t.z - 0.5;
}

// Texture `id` at a pixel, `ddx` and `ddy` the pixel's footprint in meters.
fn layer_color(id: u32, control: u32, world: vec2<f32>, corner: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> vec3<f32> {
    let t = clip.textures[id];
    if (clip.look.z < 1.5 || id >= textureNumLayers(albedo)) {
        return t.mean.rgb * t.color.rgb;
    }
    let k = control_scale(control) * t.uv.x;
    let uv = texture_uv(id, world, corner, control);
    return textureSampleGrad(albedo, albedo_sampler, uv, id, ddx * k, ddy * k).rgb * t.color.rgb;
}

// Terrain3D's auto shader: where a vertex asks for it, or no region holds it, the slope and height of the ground mix
// two textures.
fn painted_control(cell: vec2<i32>, n: vec3<f32>, y: f32) -> u32 {
    let control = control_at(cell);
    if (clip.look.w < 0.5 || ((control >> AUTO_BIT) & 1u) == 0u) {
        return control;
    }
    let a = clip.auto_shader;
    let blend = clamp(a.x * 2.0 * (n.y - 1.0) + 1.0 - a.y * 0.01 * y, 0.0, 1.0);
    return ((u32(a.z) & 31u) << BASE_SHIFT) | ((u32(a.w) & 31u) << OVERLAY_SHIFT) | ((u32(blend * 255.0 + 0.5) & 255u) << BLEND_SHIFT);
}

fn corner_color(control: u32, world: vec2<f32>, corner: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> vec3<f32> {
    let base = (control >> BASE_SHIFT) & 31u;
    let over = (control >> OVERLAY_SHIFT) & 31u;
    let blend = select(f32((control >> BLEND_SHIFT) & 255u) / 255.0, 0.0, over == base);
    var rgb = vec3<f32>(0.0);
    if (blend < 1.0) {
        rgb += (1.0 - blend) * layer_color(base, control, world, corner, ddx, ddy);
    }
    if (blend > 0.0) {
        rgb += blend * layer_color(over, control, world, corner, ddx, ddy);
    }
    return rgb;
}

// The ground's color before light: the corners of the pixel's cell each mix their two textures, the four are blended
// bilinearly and tinted by the color map.
fn ground_albedo(world: vec3<f32>, n: vec3<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> vec3<f32> {
    if (clip.look.z < 0.5) {
        return PLAIN_GROUND;
    }
    let stride = control_step(world.xz / clip.world.x);
    let q = world.xz / (clip.world.x * f32(stride));
    let c = vec2<i32>(floor(q)) * stride;
    let f = q - floor(q);
    let meters = world.xz / clip.world.y;
    let cell_meters = clip.world.x / clip.world.y;
    var rgb = vec3<f32>(0.0);
    var tint = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        let o = vec2<i32>(i32(i & 1u), i32(i >> 1u));
        let w = mix(1.0 - f.x, f.x, f32(o.x)) * mix(1.0 - f.y, f.y, f32(o.y));
        let corner = c + o * stride;
        if (w > 0.0) {
            let control = painted_control(corner, n, world.y / clip.world.y);
            rgb += w * corner_color(control, meters, vec2<f32>(corner) * cell_meters, ddx, ddy);
            tint += w * color_at(corner);
        }
    }
    return rgb * tint;
}

@fragment
fn fs_main(f: VOut) -> @location(0) vec4<f32> {
    let meters = f.world.xz / clip.world.y;
    let ddx = dpdx(meters);
    let ddy = dpdy(meters);
    if (dropped(f.world)) {
        discard;
    }
    let n = normalize(f.normal);
    let flat_mode = step(0.5, cam.params.z) * step(cam.params.z, 1.5);
    var base = vec3<f32>(0.8);
    if (flat_mode < 0.5) {
        base = ground_albedo(f.world, n, ddx, ddy);
    }
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
    if (dropped(f.world)) {
        discard;
    }
    if (cam.viewport.w > 0.0) {
        return vec4<f32>(flip(clip.wire.rgb, cam.viewport.w), 1.0);
    }
    return vec4<f32>(clip.wire.rgb, 1.0);
}

@fragment
fn fs_shadow(f: ShadowOut) {
    if (dropped(f.world)) {
        discard;
    }
}
