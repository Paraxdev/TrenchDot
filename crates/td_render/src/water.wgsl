struct Water {
    // Linear color, alpha scales the opacity
    color: vec4<f32>,
    // x: map unit height of the surface
    plane: vec4<f32>,
};

@group(1) @binding(0) var<uniform> water: Water;

// Half width in map units of the quad around the camera, past the farthest a clipmap reaches.
const WATER_REACH: f32 = 4194304.0;

struct WaterOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> WaterOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corners[i] * WATER_REACH;
    var o: WaterOut;
    o.clip = cam.view_proj * vec4<f32>(cam.eye.x + c.x, water.plane.x, cam.eye.z + c.y, 1.0);
    return o;
}

// The pixel's ray meets the plane again here rather than interpolating a world position, which loses precision across
// a quad this wide.
@fragment
fn fs_main(f: WaterOut) -> @location(0) vec4<f32> {
    let ndc = vec2<f32>(f.clip.x / cam.viewport.x * 2.0 - 1.0, 1.0 - f.clip.y / cam.viewport.y * 2.0);
    let near = cam.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let start = near.xyz / near.w;
    let dir = pixel_ray(f.clip);
    let dy = select(dir.y, -1e-6, abs(dir.y) < 1e-6);
    let world = start + dir * clamp((water.plane.x - start.y) / dy, 0.0, WATER_REACH * 2.0);
    let rgb = apply_fog(shade(water.color.rgb, world, vec3<f32>(0.0, 1.0, 0.0)), world);
    return vec4<f32>(rgb, water_alpha(dir, water.color.a));
}
