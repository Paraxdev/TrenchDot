struct ShapeIn {
    @builtin(vertex_index) index: u32,
    @location(0) min: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) max: vec3<f32>,
    @location(3) arrow: vec4<f32>,
};

fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

fn box_corner(lo: vec3<f32>, hi: vec3<f32>, bits: u32) -> vec3<f32> {
    return select(lo, hi, vec3<bool>((bits & 1u) != 0u, (bits & 2u) != 0u, (bits & 4u) != 0u));
}

@vertex
fn vs_main(v: ShapeIn) -> VOut {
    let seg = v.index / 6u;
    let corner = v.index % 6u;
    var color = vec4<f32>(srgb_decode(v.color.rgb), v.color.a);
    let center = (v.min + v.max) * 0.5;
    var a: vec3<f32>;
    var b: vec3<f32>;
    if (v.arrow.w < -0.5) {
        if (seg > 1u) {
            return hidden();
        }
        if (seg == 0u) {
            a = vec3<f32>(v.min.x, center.y, center.z);
            b = vec3<f32>(v.max.x, center.y, center.z);
        } else {
            a = vec3<f32>(center.x, center.y, v.min.z);
            b = vec3<f32>(center.x, center.y, v.max.z);
        }
    } else if (seg < 12u) {
        // Corner bits are x, y, z. Each edge joins two corners that differ in one bit.
        var edges = array<vec2<u32>, 12>(
            vec2<u32>(0u, 1u), vec2<u32>(2u, 3u), vec2<u32>(4u, 5u), vec2<u32>(6u, 7u),
            vec2<u32>(0u, 2u), vec2<u32>(1u, 3u), vec2<u32>(4u, 6u), vec2<u32>(5u, 7u),
            vec2<u32>(0u, 4u), vec2<u32>(1u, 5u), vec2<u32>(2u, 6u), vec2<u32>(3u, 7u),
        );
        let e = edges[seg];
        a = box_corner(v.min, v.max, e.x);
        b = box_corner(v.min, v.max, e.y);
    } else {
        if (v.arrow.w < 0.5) {
            return hidden();
        }
        let size = v.max - v.min;
        a = center;
        b = center + normalize(v.arrow.xyz) * max(size.x, max(size.y, size.z)) * 0.75;
        color.a = color.a * 0.9;
    }
    return segment(a, b, color, color, corner);
}
