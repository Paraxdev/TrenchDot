// clipmap::wgsl_consts() puts MESH_SIZE, MAP_SIDE, LOC_MIN, NO_REGION, COARSE_ONLY and the control word shifts in
// front of this file, then ring.wgsl. Positions in cells are Terrain3D vertex grid coordinates, x along world x and y
// along world z.

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
    // x: 1 when the background draws, y: 1 with water over the ground, z: the water's opacity, w: the paint overlay
    // (0 none, 1 navigation, 2 wetness, 3 autoshader)
    toggles: vec4<f32>,
    // rgb: linear water color, w: map unit height of the water surface
    water: vec4<f32>,
    // x: Terrain3D's blend sharpness
    material: vec4<f32>,
    // Terrain3D's dual scaling. x: the texture, -1 when off, y: far uv scale over the near one, z and w: meters from
    // the eye where the far scale fades in and where it shows alone
    dual_scale: vec4<f32>,
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
@group(1) @binding(8) var coarse_color: texture_2d<f32>;

const TAU: f32 = 6.283185307179586;
const PLAIN_GROUND: vec3<f32> = vec3<f32>(0.62, 0.6, 0.55);
const NAVIGATION_WASH: vec3<f32> = vec3<f32>(0.5, 0.2, 0.5);
const WET_WASH: vec3<f32> = vec3<f32>(0.4, 0.6, 1.0);
const DRY_WASH: vec3<f32> = vec3<f32>(0.6, 0.4, 0.0);
const AUTOSHADER_WASH: vec3<f32> = vec3<f32>(0.36, 0.2, 0.09);

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    // How far the noise background falls one cell along x and z, in meters.
    @location(1) noise_fall: vec2<f32>,
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
    if (clip.look.x < 1.5 || clip.toggles.x < 0.5) {
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

fn noise_fall(cell: vec2<f32>, octaves: i32) -> vec2<f32> {
    let h = noise_height(cell, octaves);
    return h - vec2<f32>(noise_height(cell + vec2<f32>(1.0, 0.0), octaves), noise_height(cell + vec2<f32>(0.0, 1.0), octaves));
}

fn surface(cell: vec2<f32>) -> f32 {
    return blended(cell, raw_height(vec2<i32>(cell)));
}

// Terrain3D's fragment normal: each corner of the pixel's cell takes the slope toward the next vertex along x and z at
// full resolution whatever the lod. Where a pixel spans 16 cells or more, or no region holds a corner, the first corner
// alone gives it, otherwise the four mix bilinearly without normalizing, which is the normal its auto shader reads.
// `ddx` and `ddy` are the screen derivatives of the position in meters.
fn pixel_normal(world: vec3<f32>, fall: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> vec3<f32> {
    let p = world.xz / clip.world.x;
    let c = floor(p);
    let f = p - c;
    let s = clip.world.x / clip.world.y;
    let h00 = surface(c);
    let h10 = surface(c + vec2<f32>(1.0, 0.0));
    let h01 = surface(c + vec2<f32>(0.0, 1.0));
    let n00 = normalize(vec3<f32>(h00 - h10 + fall.x, s, h00 - h01 + fall.y));
    let k = vec2<i32>(c);
    let held = present(k) || present(k + vec2<i32>(1, 0)) || present(k + vec2<i32>(0, 1)) || present(k + vec2<i32>(1, 1));
    if (!held || max(length(ddx), length(ddy)) >= 16.0 * s) {
        return n00;
    }
    let h11 = surface(c + vec2<f32>(1.0, 1.0));
    let h20 = surface(c + vec2<f32>(2.0, 0.0));
    let h21 = surface(c + vec2<f32>(2.0, 1.0));
    let h02 = surface(c + vec2<f32>(0.0, 2.0));
    let h12 = surface(c + vec2<f32>(1.0, 2.0));
    let n10 = normalize(vec3<f32>(h10 - h20 + fall.x, s, h10 - h11 + fall.y));
    let n01 = normalize(vec3<f32>(h01 - h11 + fall.x, s, h01 - h02 + fall.y));
    let n11 = normalize(vec3<f32>(h11 - h21 + fall.x, s, h11 - h12 + fall.y));
    return mix(mix(n00, n10, f.x), mix(n01, n11, f.x), f.y);
}

struct RingVertex {
    world: vec3<f32>,
    morph: RingMorph,
    octaves: i32,
};

fn ring_vertex(v: VIn) -> RingVertex {
    let m = ring_morph(v, cam.eye.xz / clip.world.x);
    let pos = mix(vec2<f32>(m.cell), vec2<f32>(m.end), m.lerp);
    let octaves = noise_octaves(m.cell);
    var noise = noise_height(vec2<f32>(m.cell), octaves);
    if (m.lerp > 0.0) {
        noise = mix(noise, noise_height(vec2<f32>(m.end), octaves), m.lerp);
    }
    let h = blended(pos, mix(raw_height(m.cell), raw_height(m.end), m.lerp)) + noise;
    return RingVertex(vec3<f32>(pos.x * clip.world.x, h * clip.world.y, pos.y * clip.world.x), m, octaves);
}

@vertex
fn vs_main(v: VIn) -> VOut {
    let r = ring_vertex(v);
    let m = r.morph;
    var fall = noise_fall(vec2<f32>(m.cell), r.octaves);
    if (m.lerp > 0.0) {
        fall = mix(fall, noise_fall(vec2<f32>(m.end), r.octaves), m.lerp);
    }

    var o: VOut;
    o.world = r.world;
    o.clip = cam.view_proj * vec4<f32>(r.world, 1.0);
    o.noise_fall = fall;
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

// With no background drawn, whether the level 0 triangle under a pixel has a corner no region holds, so this drops exactly
// the triangles picking misses.
fn outside(world: vec3<f32>) -> bool {
    if (clip.look.x > 0.5 && clip.toggles.x > 0.5) {
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

// Terrain3D's color map at a vertex, roughness in alpha, and its blank color where no region holds it.
fn color_texel(cell: vec2<i32>) -> vec4<f32> {
    let loc = loc_of(cell);
    let slot = slot_at(loc);
    if (slot >= 0) {
        return textureLoad(fine_color, cell - loc * clip.grid.x, slot, 0);
    }
    if (slot == COARSE_ONLY) {
        return textureLoad(coarse_color, (cell - clip.grid.zw * clip.grid.x) / clip.grid.y, 0);
    }
    return vec4<f32>(1.0, 1.0, 1.0, 0.5);
}

fn color_at(cell: vec2<i32>) -> vec3<f32> {
    return color_texel(cell).rgb;
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

// Where Terrain3D samples texture `id` at `world` meters and uv scale `s`, in a cell whose corner vertex at `corner`
// meters holds `control`. The twin of look::texture_uv.
fn texture_uv(id: u32, world: vec2<f32>, corner: vec2<f32>, control: u32, s: f32) -> vec2<f32> {
    let scale = control_scale(control);
    let angle = f32((control >> ANGLE_SHIFT) & 15u) * (-TAU / 16.0);
    let turn = vec2<f32>(cos(angle), sin(angle));
    let uv = rotate(world * scale, turn);
    let pos = rotate(corner * scale, turn);
    let t = clip.textures[id].uv;
    let center = floor(pos * s + 0.5);
    let tile = (random(center) * 2.0 - 1.0) * TAU;
    let spin = tile * t.y;
    return rotate(uv * s - center, vec2<f32>(cos(spin), sin(spin))) + center + tile * t.z - 0.5;
}

fn sample_layer(id: u32, control: u32, world: vec2<f32>, corner: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>, s: f32) -> vec4<f32> {
    let k = control_scale(control) * s;
    return textureSampleGrad(albedo, albedo_sampler, texture_uv(id, world, corner, control, s), id, ddx * k, ddy * k);
}

// How far texture `id` has faded to its dual scale sample `dist` meters from the eye, 0 for any other texture or with
// dual scaling off. Terrain3D's smoothstep, made a step where the two distances meet.
fn far_share(id: u32, dist: f32) -> f32 {
    let d = clip.dual_scale;
    if (f32(id) != d.x) {
        return 0.0;
    }
    if (d.w <= d.z) {
        return step(d.z, dist);
    }
    return smoothstep(d.z, d.w, dist);
}

// Texture `id` at a pixel with its height in alpha, `ddx` and `ddy` the pixel's footprint in meters, `far` its share
// of the dual scale sample.
fn layer_color(id: u32, control: u32, world: vec2<f32>, corner: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>, far: f32) -> vec4<f32> {
    let t = clip.textures[id];
    if (clip.look.z < 1.5 || id >= textureNumLayers(albedo)) {
        return vec4<f32>(t.mean.rgb * t.color.rgb, t.mean.a);
    }
    var s = vec4<f32>(0.0);
    if (far < 1.0) {
        s = sample_layer(id, control, world, corner, ddx, ddy, t.uv.x);
    }
    if (far > 0.0) {
        s = mix(s, sample_layer(id, control, world, corner, ddx, ddy, t.uv.x * clip.dual_scale.y), far);
    }
    return vec4<f32>(s.rgb * t.color.rgb, s.a);
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

fn control_blend(control: u32) -> f32 {
    return f32((control >> BLEND_SHIFT) & 255u) / 255.0;
}

// How much of the pixel's cell asks for texture `id`, its corners weighed bilinearly.
fn texture_share(id: u32, controls: array<u32, 4>, weights: array<f32, 4>) -> f32 {
    var share = 0.0;
    for (var i = 0u; i < 4u; i++) {
        let blend = control_blend(controls[i]);
        share += weights[i] * (select(0.0, 1.0 - blend, ((controls[i] >> BASE_SHIFT) & 31u) == id) + select(0.0, blend, ((controls[i] >> OVERLAY_SHIFT) & 31u) == id));
    }
    return share;
}

// Terrain3D's accumulate_material: a texture at a corner of bilinear weight w weighs (w + share + height)^sharpness * w,
// so where textures meet the taller one shows through. The overlay's height is scaled by the ground's slope, which
// Terrain3D takes from the base texture's normal map.
fn height_weight(w: f32, share: f32, height: f32) -> f32 {
    return exp2((60.0 * clip.material.x + 4.0) * log2(w + share + height)) * w;
}

// The corners of the pixel's cell that ground_albedo blends, with their bilinear weights.
struct Corners {
    cells: array<vec2<i32>, 4>,
    weights: vec4<f32>,
};

fn corners_of(world: vec3<f32>) -> Corners {
    let stride = control_step(world.xz / clip.world.x);
    let q = world.xz / (clip.world.x * f32(stride));
    let c = vec2<i32>(floor(q)) * stride;
    let f = q - floor(q);
    var k: Corners;
    for (var i = 0u; i < 4u; i++) {
        let o = vec2<i32>(i32(i & 1u), i32(i >> 1u));
        k.cells[i] = c + o * stride;
        k.weights[i] = mix(1.0 - f.x, f.x, f32(o.x)) * mix(1.0 - f.y, f.y, f32(o.y));
    }
    return k;
}

// The color map blended over the pixel's cell, roughness in alpha.
fn color_tint(world: vec3<f32>) -> vec4<f32> {
    let k = corners_of(world);
    var tint = vec4<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        if (k.weights[i] > 0.0) {
            tint += k.weights[i] * color_texel(k.cells[i]);
        }
    }
    return tint;
}

// How much of the pixel's cell has `bit` set in its control words.
fn control_share(world: vec3<f32>, bit: u32) -> f32 {
    let k = corners_of(world);
    var share = 0.0;
    for (var i = 0u; i < 4u; i++) {
        share += k.weights[i] * f32((control_at(k.cells[i]) >> bit) & 1u);
    }
    return share;
}

// The wash the Sculpt tool's Terrain3D channels show while they paint. Wetness is Terrain3D's roughness modifier, alpha
// below 0.5 is wetter.
fn paint_overlay(base: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let mode = u32(clip.toggles.w + 0.5);
    if (mode == 1u) {
        return mix(base, NAVIGATION_WASH, 0.5 * control_share(world, NAV_BIT));
    }
    if (mode == 2u) {
        let wet = color_tint(world).a - 0.5;
        return mix(base, select(DRY_WASH, WET_WASH, wet < 0.0), abs(wet));
    }
    if (mode == 3u) {
        return mix(base, AUTOSHADER_WASH, 0.5 * control_share(world, AUTO_BIT));
    }
    return base;
}

// The ground's color before light: the textures at the corners of the pixel's cell, height blended and tinted by the
// bilinearly blended color map.
fn ground_albedo(world: vec3<f32>, n: vec3<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> vec3<f32> {
    if (clip.look.z < 0.5) {
        return PLAIN_GROUND * color_tint(world).rgb;
    }
    let stride = control_step(world.xz / clip.world.x);
    let q = world.xz / (clip.world.x * f32(stride));
    let c = vec2<i32>(floor(q)) * stride;
    let f = q - floor(q);
    let meters = world.xz / clip.world.y;
    let cell_meters = clip.world.x / clip.world.y;
    let dist = distance(world, cam.eye.xyz) / clip.world.y;
    var controls: array<u32, 4>;
    var weights: array<f32, 4>;
    var tint = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        let o = vec2<i32>(i32(i & 1u), i32(i >> 1u));
        weights[i] = mix(1.0 - f.x, f.x, f32(o.x)) * mix(1.0 - f.y, f.y, f32(o.y));
        let corner = c + o * stride;
        controls[i] = painted_control(corner, n, world.y / clip.world.y);
        if (weights[i] > 0.0) {
            tint += weights[i] * color_at(corner);
        }
    }
    var rgb = vec3<f32>(0.0);
    var total = 0.0;
    for (var i = 0u; i < 4u; i++) {
        let w = weights[i];
        if (w <= 0.0) {
            continue;
        }
        let control = controls[i];
        let corner = vec2<f32>(c + vec2<i32>(i32(i & 1u), i32(i >> 1u)) * stride) * cell_meters;
        let base = (control >> BASE_SHIFT) & 31u;
        let over = (control >> OVERLAY_SHIFT) & 31u;
        let blend = control_blend(control);
        var up = 1.0;
        if (blend < 1.0) {
            let far = far_share(base, dist);
            let a = layer_color(base, control, meters, corner, ddx, ddy, far);
            let k = height_weight(w, texture_share(base, controls, weights), a.a);
            rgb += k * a.rgb;
            total += k;
            up = n.y;
        }
        if (blend > 0.0 && over != base) {
            let far = far_share(over, dist);
            let b = layer_color(over, control, meters, corner, ddx, ddy, far);
            let k = height_weight(w, texture_share(over, controls, weights), b.a * mix(clamp(up, 0.0, 1.0), 1.0, far));
            rgb += k * b.rgb;
            total += k;
        }
    }
    return rgb / max(total, 1e-8) * tint;
}

// A selected node fills the whole view, so the red wash stays light and the region boxes carry the outline.
fn selection_tint(base: vec3<f32>, selected: f32) -> vec3<f32> {
    return mix(base, base * vec3<f32>(1.0, 0.62, 0.58), selected * 0.35);
}

@fragment
fn fs_main(f: VOut) -> @location(0) vec4<f32> {
    let meters = f.world.xz / clip.world.y;
    let ddx = dpdx(meters);
    let ddy = dpdy(meters);
    if (dropped(f.world)) {
        discard;
    }
    let w = pixel_normal(f.world, f.noise_fall, ddx, ddy);
    let n = normalize(w);
    let flat_mode = step(0.5, cam.params.z) * step(cam.params.z, 1.5);
    var base = vec3<f32>(0.8);
    if (flat_mode < 0.5) {
        base = ground_albedo(f.world, w, ddx, ddy);
    }
    base = paint_overlay(base, f.world);
    base = selection_tint(base, clip.look.y);
    var rgb: vec3<f32>;
    if (is_lit()) {
        rgb = tonemap(base * light_at(f.world, n));
    } else {
        rgb = base * relief(n);
    }
    if (clip.toggles.y > 0.5 && f.world.y < clip.water.w) {
        // The water plane blends over this pixel by `a`, so the ground takes on the rest of Terrain3D's depth fade.
        let a = water_alpha(pixel_ray(f.clip), clip.toggles.z);
        let cover = water_depth_cover((clip.water.w - f.world.y) / clip.world.y);
        let under = clamp((cover - a) / max(1.0 - a, 1e-4), 0.0, 1.0);
        rgb = mix(rgb, shade(clip.water.rgb, f.world, vec3<f32>(0.0, 1.0, 0.0)), under);
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
