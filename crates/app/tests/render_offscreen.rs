//! Renders into an offscreen texture and checks pixels. Skipped when no GPU is available.

use infinite_canvas::app::CanvasApp;
use infinite_canvas::gestures::{MouseButton, Phase, PointerEvent, PointerKind};
use infinite_canvas::render::{Renderer, Target};

const W: u32 = 384; // 384 * 4 bytes is a multiple of 256 (copy alignment)
const H: u32 = 300;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct Headless {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    renderer: Renderer,
}

fn headless() -> Option<Headless> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (W * 4 * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let renderer = Renderer::new(&device, FORMAT);
    Some(Headless { device, queue, texture, view, readback, renderer })
}

impl Headless {
    /// Renders a frame and returns RGBA pixels.
    fn frame(&mut self, app: &mut CanvasApp) -> Vec<u8> {
        // Render twice: the first frame may postpone meshes because of the time budget.
        for _ in 0..3 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W as f32, H as f32))),
                ..Default::default()
            };
            let target = Target::View { device: &self.device, queue: &self.queue, format: FORMAT, size: [W, H], view: &self.view };
            let _ = app.frame(target, &mut self.renderer, raw);
        }
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 4), rows_per_image: Some(H) },
            },
            wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range().unwrap().to_vec();
        self.readback.unmap();
        if let Ok(path) = std::env::var("DUMP_FRAME") {
            std::fs::write(path, &data).unwrap();
        }
        data
    }
}

fn dark(px: &[u8], x: u32, y: u32) -> bool {
    let i = ((y * W + x) * 4) as usize;
    px[i] < 100 && px[i + 1] < 100 && px[i + 2] < 100
}

fn mouse(app: &mut CanvasApp, phase: Phase, pos: [f64; 2], t: f64) {
    app.pointer(PointerEvent {
        id: 0,
        kind: PointerKind::Mouse,
        phase,
        pos,
        pressure: 1.0,
        time: t,
        button: MouseButton::Primary,
    });
}

/// Horizontal line from x0 to x1 at height y.
fn draw_line(app: &mut CanvasApp, y: f64, x0: f64, x1: f64) {
    mouse(app, Phase::Down, [x0, y], 0.0);
    let n = 60;
    for i in 1..=n {
        let x = x0 + (x1 - x0) * i as f64 / n as f64;
        mouse(app, Phase::Move, [x, y], i as f64 / 120.0);
    }
    // Let the smoothing filter settle on the end point.
    for i in 0..30 {
        mouse(app, Phase::Move, [x1 + 0.001 * (i % 2) as f64, y], (n + i) as f64 / 120.0);
    }
    mouse(app, Phase::Up, [x1, y], 1.0);
}

fn new_app() -> CanvasApp {
    let mut app = CanvasApp::new(None);
    app.resize([W, H]);
    app.tools.width = 10.0;
    app
}

#[test]
fn deep_zoom_stroke_renders_in_place() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    // ~10^90 deep.
    app.wheel_zoom([200.0, 150.0], 300.0);
    draw_line(&mut app, 150.0, 100.0, 300.0);
    assert_eq!(app.doc.len(), 1);
    let px = gpu.frame(&mut app);
    // The toolbar sits at the top; the line is well below it.
    assert!(dark(&px, 200, 150), "line missing at its centre");
    assert!(dark(&px, 150, 152));
    assert!(!dark(&px, 200, 170), "line too thick / misplaced");
    assert!(!dark(&px, 50, 150), "line extends too far");

    // Sub-pixel pans move it by exactly that amount (no jitter).
    for _ in 0..40 {
        app.pan([0.0, 0.5]);
    }
    let px = gpu.frame(&mut app);
    assert!(dark(&px, 200, 170) && !dark(&px, 200, 150), "pan by 20 px failed");
    for _ in 0..40 {
        app.pan([0.0, -0.5]);
    }

    // Zoom far out: it becomes sub-pixel and disappears; zoom back in: it is back in place.
    app.wheel_zoom([200.0, 150.0], -100.0);
    let px = gpu.frame(&mut app);
    assert!(!dark(&px, 200, 150), "sub-pixel stroke should be culled");
    app.wheel_zoom([200.0, 150.0], 100.0);
    let px = gpu.frame(&mut app);
    assert!(dark(&px, 200, 150) && !dark(&px, 200, 170), "stroke moved after zooming out and back");
}

#[test]
fn magnified_stroke_edge_is_exact() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    draw_line(&mut app, 150.0, 100.0, 300.0);
    // Edge of a 10 px line at y = 145. Magnify 2^20 (CPU f64 path) around a point on the edge.
    app.wheel_zoom([200.0, 145.0], 20.0);
    let px = gpu.frame(&mut app);
    assert!(dark(&px, 200, 160), "inside of the line should be inked");
    assert!(!dark(&px, 200, 130), "outside of the line should be empty");
    // Between: the edge must be within a few pixels of y = 145 even at 2^20.
    let edge = (130..160).find(|&y| dark(&px, 200, y)).unwrap();
    assert!((edge as i32 - 145).abs() <= 3, "edge at {edge}");
}

#[test]
fn eraser_and_undo() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    draw_line(&mut app, 150.0, 100.0, 300.0);
    app.tools.tool = infinite_canvas::ui::Tool::Eraser;
    draw_line(&mut app, 100.0, 200.0, 200.5); // touches nothing
    assert_eq!(app.doc.len(), 1);
    mouse(&mut app, Phase::Down, [200.0, 150.0], 2.0);
    mouse(&mut app, Phase::Up, [200.0, 150.0], 2.1);
    assert_eq!(app.doc.len(), 0);
    assert!(!dark(&gpu.frame(&mut app), 200, 150));
    app.undo();
    assert_eq!(app.doc.len(), 1);
    assert!(dark(&gpu.frame(&mut app), 200, 150));
    app.redo();
    assert_eq!(app.doc.len(), 0);
}
