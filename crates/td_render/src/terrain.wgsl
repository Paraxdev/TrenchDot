struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // x: 1 when selected
    @location(2) uv: vec2<f32>,
    // blend weights of the four layers
    @location(3) color: vec4<f32>,
    @location(4) uv2: vec2<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) flags: vec2<f32>,
    @location(3) weights: vec4<f32>,
    @location(4) uv2: vec2<f32>,
};

@vertex
fn vs_main(v: VIn) -> VOut {
    var o: VOut;
    o.clip = cam.view_proj * vec4<f32>(v.pos, 1.0);
    o.world = v.pos;
    o.normal = v.normal;
    o.flags = v.uv;
    o.weights = v.color;
    o.uv2 = v.uv2;
    return o;
}

@fragment
fn fs_main(f: VOut) -> @location(0) vec4<f32> {
    return terrain_shade(f.world, f.normal, f.weights, f.flags.x, f.uv2);
}
