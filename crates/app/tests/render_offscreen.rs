//! Renders into an offscreen texture and checks pixels. Skipped when no GPU is available.

use infinite_canvas::app::CanvasApp;
use infinite_canvas::gestures::{MouseButton, Phase, PointerEvent, PointerKind};
use infinite_canvas::render::{Renderer, Target};

const W: u32 = 768; // 384 * 4 bytes is a multiple of 256 (copy alignment)
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
    app.tools.eraser = 24.0;
    draw_line(&mut app, 100.0, 200.0, 200.5); // touches nothing
    assert_eq!(app.doc.len(), 1);

    // A tap cuts a gap; both ends stay.
    mouse(&mut app, Phase::Down, [200.0, 150.0], 2.0);
    mouse(&mut app, Phase::Up, [200.0, 150.0], 2.1);
    assert_eq!(app.doc.len(), 2);
    let px = gpu.frame(&mut app);
    assert!(!dark(&px, 200, 150));
    // Flat cut ends just outside the eraser circle: radius 12 + half the brush radius (2.5).
    assert!(!dark(&px, 212, 150) && !dark(&px, 188, 150));
    assert!(dark(&px, 217, 150) && dark(&px, 183, 150));
    assert!(dark(&px, 225, 150) && dark(&px, 175, 150) && dark(&px, 120, 150) && dark(&px, 280, 150));

    // A vertical swipe across the right piece: one gesture, one undo step.
    mouse(&mut app, Phase::Down, [260.0, 100.0], 3.0);
    for i in 1..=20 {
        mouse(&mut app, Phase::Move, [260.0, 100.0 + i as f64 * 5.0], 3.0 + i as f64 / 120.0);
    }
    mouse(&mut app, Phase::Up, [260.0, 200.0], 3.5);
    assert_eq!(app.doc.len(), 3);
    let px = gpu.frame(&mut app);
    assert!(!dark(&px, 260, 150) && dark(&px, 290, 150) && dark(&px, 230, 150));

    app.undo();
    assert_eq!(app.doc.len(), 2);
    assert!(dark(&gpu.frame(&mut app), 260, 150));
    app.undo();
    assert_eq!(app.doc.len(), 1);
    assert!(dark(&gpu.frame(&mut app), 200, 150));
    app.redo();
    app.redo();
    assert_eq!(app.doc.len(), 3);
}

/// `cargo test --release -p infinite_canvas --test render_offscreen -- --ignored --nocapture`
#[test]
#[ignore]
fn perf_10k_strokes() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    app.tools.width = 2.0;
    // 10 000 wavy strokes, all on screen at once (worst case), 20 points each.
    let mut t = 0.0;
    for i in 0..10_000u32 {
        let (x0, y0) = ((i % 100) as f64 * 3.8, 40.0 + (i / 100) as f64 * 2.6);
        mouse(&mut app, Phase::Down, [x0, y0], t);
        for k in 1..20 {
            t += 1.0 / 240.0;
            let x = x0 + k as f64 * 0.6;
            let y = y0 + (k as f64 * 0.8).sin() * 3.0;
            mouse(&mut app, Phase::Move, [x, y], t);
        }
        mouse(&mut app, Phase::Up, [x0 + 12.0, y0], t);
        t += 0.1;
    }
    assert_eq!(app.doc.len(), 10_000);
    // Warm the mesh cache.
    for _ in 0..200 {
        gpu.frame(&mut app);
        if !app.animating() {
            break;
        }
    }
    let n = 30;
    let start = std::time::Instant::now();
    for i in 0..n {
        app.pan([if i % 2 == 0 { 0.3 } else { -0.3 }, 0.0]);
        gpu.frame(&mut app);
    }
    let per_frame = start.elapsed() / (n * 3); // gpu.frame renders 3 frames
    eprintln!("10k visible strokes: {per_frame:?} per frame (incl. GPU sync + readback)");
    // Zoomed in on one area: most strokes culled.
    app.wheel_zoom([190.0, 150.0], 4.0);
    gpu.frame(&mut app);
    let start = std::time::Instant::now();
    for i in 0..n {
        app.pan([if i % 2 == 0 { 0.3 } else { -0.3 }, 0.0]);
        gpu.frame(&mut app);
    }
    eprintln!("zoomed 16x: {:?} per frame", start.elapsed() / (n * 3));
}

#[test]
fn leaving_a_project_writes_its_preview() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("icnv-thumb-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = CanvasApp::with_projects(&dir);
    app.resize([W, H]);
    app.tools.width = 30.0;
    app.gallery_action(infinite_canvas::ui::GalleryAction::New);
    // Thick horizontal line across the middle of the screen.
    draw_line(&mut app, H as f64 / 2.0, 100.0, W as f64 - 100.0);
    assert!(app.back());
    gpu.frame(&mut app);
    assert_eq!(app.screen, infinite_canvas::app::Screen::Gallery);
    let projects = infinite_canvas::projects::Projects::new(&dir).unwrap();
    let (w, h, px) = projects.load_thumbnail("Rysunek 1").expect("preview written");
    let at = |x: u32, y: u32| px[((y * w + x) * 4) as usize];
    assert!(at(w / 2, h / 2) < 100, "line visible in the middle");
    assert!(at(w / 2, h / 2 + 40) > 200, "background elsewhere");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rotated_view_renders_on_gpu_and_cpu_paths() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    // Horizontal line through the screen centre (384, 150), 10 px thick.
    draw_line(&mut app, 150.0, 300.0, 468.0);
    let px = gpu.frame(&mut app);
    assert!(dark(&px, 350, 150) && !dark(&px, 384, 110));
    // Quarter turn around the centre: the line becomes vertical (GPU instancing path).
    app.wheel_rotate([384.0, 150.0], std::f64::consts::FRAC_PI_2);
    let px = gpu.frame(&mut app);
    assert!(dark(&px, 384, 110) && dark(&px, 384, 190), "vertical after rotation");
    assert!(!dark(&px, 340, 150) && !dark(&px, 430, 150));
    // Magnify 2^8 around the line's right edge (x = 389): strokes this magnified take the
    // exact CPU path, which must apply the same rotation. The edge stays put.
    app.wheel_zoom([389.0, 150.0], 8.0);
    let px = gpu.frame(&mut app);
    assert!(dark(&px, 384, 150) && dark(&px, 386, 40), "inside the line");
    assert!(!dark(&px, 393, 150) && !dark(&px, 393, 260), "outside the edge");
    // Straighten again.
    app.reset_rotation();
    app.wheel_zoom([389.0, 150.0], -8.0);
    let px = gpu.frame(&mut app);
    assert!(dark(&px, 350, 150) && !dark(&px, 384, 110));
}
