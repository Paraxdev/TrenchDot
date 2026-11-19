//! Native terrains drawn with the clipmap rings through `Renderer::render`. The noop backend stubs out the GPU but keeps
//! wgpu's checks of the pipeline layout, the bind groups and the instance ranges each draw reads.

use egui_wgpu::wgpu;
use glam::Vec3;
use glam::camera::rh::proj::directx::perspective_infinite_reverse;
use glam::camera::rh::view::look_to_mat4;
use td_render::clipmap::layout::MAX_INSTANCES;
use td_render::clipmap::native::{TerrainClipParams, TerrainClipmap};
use td_render::{Frame, FrameParams, Renderer, ShadeMode, ViewTarget};

fn gpu() -> (egui_wgpu::RenderState, Renderer) {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::NOOP;
    descriptor.backend_options.noop.enable = true;
    let instance = wgpu::Instance::new(descriptor);
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let config = egui_wgpu::WgpuConfiguration {
        wgpu_setup: egui_wgpu::WgpuSetup::Existing(egui_wgpu::WgpuSetupExisting { instance: instance.clone(), adapter, device, queue }),
        ..Default::default()
    };
    let state = block_on(egui_wgpu::RenderState::create(&config, &instance, None, egui_wgpu::RendererOptions::default())).unwrap();
    let renderer = Renderer::new(&state);
    (state, renderer)
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

const ORIGIN: Vec3 = Vec3::new(-1000.5, 37.0, 2210.0);
const CELL: f32 = 24.0;

fn view(shade: ShadeMode, eye: Vec3, dir: Vec3) -> FrameParams {
    FrameParams {
        view_proj: perspective_infinite_reverse(1.1, 16.0 / 9.0, 1.0) * look_to_mat4(eye, dir.normalize(), Vec3::Y),
        eye,
        grid_size: 0.0,
        grid_alpha: 0.0,
        shade,
        orthographic: false,
        clear: [0.0; 4],
        line_width: 1.0,
        selection_line_width: 2.0,
        line_flip: None,
    }
}

fn terrain(renderer: &Renderer, resolution: [u32; 2], material: &str) -> TerrainClipmap {
    let mut clip = renderer.new_terrain_clipmap(resolution).unwrap();
    clip.set_params(&renderer.queue, &TerrainClipParams { origin: ORIGIN, cell: CELL, ..Default::default() });
    clip.set_material(material);
    clip
}

fn draw(renderer: &Renderer, target: &ViewTarget, params: &FrameParams, clips: &[&TerrainClipmap]) {
    renderer.render(target, params, &Frame { terrain_clipmaps: clips.to_vec(), ..Default::default() });
}

#[test]
fn native_terrains_count_the_triangles_their_rings_draw() {
    let (_state, mut renderer) = gpu();
    let key = renderer.prepare_terrain_material(&[("grass".into(), 4.0, 0.0, 0.5)]);
    let mut target = None;
    renderer.ensure_target(&mut target, [320, 180]);
    let target = target.unwrap();
    let small = terrain(&renderer, [257, 257], &key);
    let big = terrain(&renderer, [2049, 2049], &key);
    let eye = ORIGIN + Vec3::new(100.5 * CELL, 150.0, 120.25 * CELL);
    let low = view(ShadeMode::Textured, eye, Vec3::new(1.0, -0.15, 0.8));

    draw(&renderer, &target, &low, &[&small]);
    let near = small.triangles();
    draw(&renderer, &target, &low, &[&big]);
    let far = big.triangles();
    assert!(near > 0 && far >= near && far < 4 * near, "{near} triangles for 257 vertices a side, {far} for 2049");
    assert!(far < 2 * 2048 * 2048 / 20, "{far} triangles is no saving over the chunk meshes");

    let above = view(ShadeMode::Lit, ORIGIN + Vec3::new(1024.0 * CELL, 60000.0, 1024.0 * CELL), Vec3::new(0.01, -1.0, 0.02));
    let many = [&big, &small, &big, &big];
    let parts: usize = many.iter().map(|clip| clip.instances(above.eye, above.view_proj).parts.len()).sum();
    assert!(parts > MAX_INSTANCES, "{parts} parts do not outgrow the first instance buffer");
    draw(&renderer, &target, &above, &many);
    assert!(small.triangles() > 0 && big.triangles() > small.triangles());
    draw(&renderer, &target, &low, &[&small, &big]);
    assert_eq!((small.triangles(), big.triangles()), (near, far), "each terrain keeps its own count in a shared frame");

    let behind = view(ShadeMode::Textured, ORIGIN - Vec3::new(500.0, -150.0, 500.0), Vec3::new(-1.0, -0.1, -1.0));
    assert!(small.instances(behind.eye, behind.view_proj).parts.is_empty());
    draw(&renderer, &target, &behind, &[&small]);
    assert_eq!(small.triangles(), 0);

    let unpainted = terrain(&renderer, [257, 257], "no such material");
    draw(&renderer, &target, &low, &[&unpainted, &small]);
    assert_eq!((unpainted.triangles(), small.triangles()), (0, near), "a terrain without its material draws nothing");

    let wire = terrain(&renderer, [257, 257], &key);
    draw(&renderer, &target, &view(ShadeMode::Wireframe, low.eye, Vec3::new(1.0, -0.15, 0.8)), &[&wire]);
    assert_eq!(wire.triangles(), 0, "wireframe draws the terrain's prebuilt lines instead");
    draw(&renderer, &target, &low, &[&wire]);
    assert_eq!(wire.triangles(), near);
    renderer.free_target(target);
}

#[test]
fn native_terrains_cast_into_the_shadow_map() {
    let (_state, renderer) = gpu();
    let small = terrain(&renderer, [257, 257], "no such material");
    let big = terrain(&renderer, [2049, 2049], "no such material");
    let eye = ORIGIN + Vec3::new(1024.5 * CELL, 150.0, 1000.25 * CELL);
    let far = ORIGIN + Vec3::new(2048.0 * CELL, 900.0, 2048.0 * CELL);
    let sun = Renderer::sun_view_proj(Vec3::new(0.3, -1.0, 0.45), ORIGIN - Vec3::new(100.0, 600.0, 100.0), far);
    let many = [&big, &small, &big, &big, &big];
    let parts: usize = many.iter().map(|clip| clip.instances(eye, sun).parts.len()).sum();
    assert!(parts > MAX_INSTANCES, "{parts} parts do not outgrow the first shadow instance buffer");

    renderer.render_shadow(&[], &[], &many, eye, sun);
    renderer.render_shadow(&[], &[], &[&small], eye, sun);
    renderer.render_shadow(&[], &[], &[], eye, sun);
    assert_eq!((small.triangles(), big.triangles()), (0, 0), "the shadow pass leaves the view's triangle counts alone");
}
