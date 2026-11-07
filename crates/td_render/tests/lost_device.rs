//! What the renderer does once the GPU device is gone, which a driver reset or a crash of the GPU process causes. The
//! noop backend stubs out the GPU but keeps wgpu's own device state, so `destroy` loses it the way a real loss does.

use egui_wgpu::wgpu;
use td_render::{LineVertex, MeshBatch, MeshInstance, MeshVertex, Renderer};

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

fn line(x: f32) -> LineVertex {
    LineVertex { pos: [x, -2.5, 7.25], color: [0.2, 0.4, 0.6, 1.0] }
}

fn triangle() -> MeshBatch {
    let v = |x: f32, y: f32| MeshVertex { pos: [x, y, 3.5], normal: [0.0, 0.0, 1.0], uv: [x, y], color: [1.0; 4], uv2: [0.0; 2] };
    let mut batch = MeshBatch::default();
    batch.add_polygon("stone", &[v(1.5, -2.0), v(4.0, 0.5), v(-3.0, 6.0)]);
    batch
}

fn instance(x: f32) -> MeshInstance {
    MeshInstance::new(glam::Mat4::from_translation(glam::Vec3::new(x, 12.5, -3.0)), [0.5, 0.25, 1.0, 1.0])
}

#[test]
fn uploads_work_while_the_device_is_healthy() {
    let (state, renderer) = gpu();
    let _ = state.device.poll(wgpu::PollType::Poll);
    assert_eq!(renderer.lost(), None);
    assert!(renderer.upload_lines(&[]).is_none());
    assert!(renderer.upload_lines(&[line(1.0)]).is_none());
    assert!(renderer.upload_lines(&[line(1.0), line(2.0)]).is_some());
    assert!(renderer.upload_lines(&[line(1.0), line(2.0), line(3.5), line(-4.0), line(9.0)]).is_some());
    assert!(renderer.upload_instances(&[]).is_none());
    assert!(renderer.upload_instances(&[instance(-7.5), instance(2.0), instance(31.0)]).is_some());
    assert!(renderer.upload_mesh(&triangle()).is_some());
}

#[test]
fn a_lost_device_does_not_panic_the_renderer() {
    let (state, mut renderer) = gpu();
    state.device.destroy();
    let _ = state.device.poll(wgpu::PollType::Poll);

    assert!(renderer.lost().is_some_and(|reason| reason.starts_with("Destroyed")));
    assert!(renderer.upload_lines(&[line(1.0), line(2.0)]).is_none());
    assert!(renderer.upload_lines(&[line(1.0), line(2.0), line(3.5), line(-4.0), line(9.0)]).is_none());
    assert!(renderer.upload_instances(&[instance(-7.5), instance(2.0), instance(31.0)]).is_none());
    assert!(renderer.upload_mesh(&triangle()).is_none());
    renderer.set_material("stone", &image::RgbaImage::from_pixel(4, 4, image::Rgba([90, 80, 70, 255])));
    renderer.prepare_terrain_material(&[("stone".to_string(), 128.0, 4.0, 0.25), ("grass".to_string(), 64.0, 0.0, 0.5)]);
}

#[test]
fn a_loss_not_reported_yet_still_does_not_panic() {
    let (state, renderer) = gpu();
    state.device.destroy();

    assert_eq!(renderer.lost(), None);
    let _ = renderer.upload_lines(&[line(1.0), line(2.0), line(3.5)]);
    let _ = renderer.upload_instances(&[instance(-7.5), instance(2.0)]);
    let _ = renderer.upload_mesh(&triangle());
}

#[test]
#[should_panic(expected = "wgpu error")]
fn other_gpu_errors_still_panic_on_a_healthy_device() {
    let (state, _renderer) = gpu();
    let _ = state.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("no usage"),
        size: 16,
        usage: wgpu::BufferUsages::empty(),
        mapped_at_creation: false,
    });
}
