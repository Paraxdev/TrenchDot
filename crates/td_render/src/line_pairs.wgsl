struct VIn {
    @builtin(vertex_index) corner: u32,
    @location(0) a: vec3<f32>,
    @location(1) color_a: vec4<f32>,
    @location(2) b: vec3<f32>,
    @location(3) color_b: vec4<f32>,
};

@vertex
fn vs_main(v: VIn) -> VOut {
    return segment(v.a, v.b, v.color_a, v.color_b, v.corner);
}
