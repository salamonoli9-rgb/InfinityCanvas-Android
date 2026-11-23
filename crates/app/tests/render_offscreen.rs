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
    /// Renders exactly one frame and returns RGBA pixels.
    fn frame_once(&mut self, app: &mut CanvasApp) -> Vec<u8> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W as f32, H as f32))),
            ..Default::default()
        };
        let target = Target::View { device: &self.device, queue: &self.queue, format: FORMAT, size: [W, H], view: &self.view };
        let _ = app.frame(target, &mut self.renderer, raw);
        self.read()
    }

    /// Renders a frame and returns RGBA pixels.
    fn frame(&mut self, app: &mut CanvasApp) -> Vec<u8> {
        // Render until the picture is complete: meshes may still be coming from the worker
        // threads, and the sharp frame follows once the view has settled.
        for i in 0..400 {
            if i >= 3 && !app.animating() {
                break;
            }
            if i >= 3 {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W as f32, H as f32))),
                ..Default::default()
            };
            let target = Target::View { device: &self.device, queue: &self.queue, format: FORMAT, size: [W, H], view: &self.view };
            let _ = app.frame(target, &mut self.renderer, raw);
        }
        self.read()
    }

    fn read(&mut self) -> Vec<u8> {
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
    draw_line(&mut app, 150.0, 100.0, 300.0); // 10 px thick, y 145..155
    app.tools.tool = infinite_canvas::ui::Tool::Eraser;
    app.tools.eraser = 24.0;

    // A tap erases a round hole of radius 12: exactly the eraser's shape, not a band.
    mouse(&mut app, Phase::Down, [200.0, 150.0], 2.0);
    mouse(&mut app, Phase::Up, [200.0, 150.0], 2.1);
    assert_eq!(app.doc.len(), 2, "the eraser is a stroke of its own");
    let px = gpu.frame(&mut app);
    assert!(!dark(&px, 200, 150) && !dark(&px, 208, 150) && !dark(&px, 192, 150));
    assert!(!dark(&px, 209, 146), "inside the circle near the line's edge");
    assert!(dark(&px, 214, 150) && dark(&px, 186, 150), "just outside the circle");
    // Round, not flat: at the line's edge the circle is narrower than at its middle.
    assert!(dark(&px, 212, 146) || dark(&px, 212, 154));
    assert!(dark(&px, 120, 150) && dark(&px, 280, 150));

    // A vertical swipe at x = 260 erases a band; ink drawn afterwards is not erased.
    mouse(&mut app, Phase::Down, [260.0, 100.0], 3.0);
    for i in 1..=20 {
        mouse(&mut app, Phase::Move, [260.0, 100.0 + i as f64 * 5.0], 3.0 + i as f64 / 120.0);
    }
    mouse(&mut app, Phase::Up, [260.0, 200.0], 3.5);
    let px = gpu.frame(&mut app);
    assert!(!dark(&px, 260, 150) && dark(&px, 290, 150) && dark(&px, 230, 150));
    app.tools.tool = infinite_canvas::ui::Tool::Pen;
    draw_line(&mut app, 120.0, 240.0, 280.0);
    let px = gpu.frame(&mut app);
    assert!(dark(&px, 260, 120), "later ink stays");
    assert_eq!(app.doc.len(), 4);

    app.undo();
    app.undo();
    assert!(dark(&gpu.frame(&mut app), 260, 150));
    app.undo();
    let px = gpu.frame(&mut app);
    assert!(dark(&px, 200, 150));
    assert_eq!(app.doc.len(), 1);
    app.redo();
    assert!(!dark(&gpu.frame(&mut app), 200, 150));
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
    let n = 20;
    // Full renders: the view settles between frames, so the whole scene is drawn.
    let mut full = std::time::Duration::ZERO;
    for i in 0..n {
        app.pan([if i % 2 == 0 { 0.3 } else { -0.3 }, 0.0]);
        std::thread::sleep(std::time::Duration::from_millis(170));
        let t = std::time::Instant::now();
        gpu.frame_once(&mut app);
        full += t.elapsed();
    }
    eprintln!("10k visible strokes, full render: {:?} per frame (incl. GPU sync + readback)", full / n);
    // During a gesture: the cached scene is moved around.
    let t = std::time::Instant::now();
    for i in 0..n {
        app.pan([if i % 2 == 0 { 3.0 } else { -2.0 }, 1.0]);
        gpu.frame_once(&mut app);
    }
    let moving = t.elapsed() / n;
    eprintln!("10k visible strokes, while panning: {moving:?} per frame");
    // Drawing over it: the cached scene plus the stroke in progress.
    std::thread::sleep(std::time::Duration::from_millis(170));
    gpu.frame_once(&mut app);
    mouse(&mut app, Phase::Down, [100.0, 280.0], 1000.0);
    let t = std::time::Instant::now();
    for i in 0..n {
        mouse(&mut app, Phase::Move, [100.0 + i as f64 * 20.0, 280.0], 1000.0 + i as f64 / 60.0);
        gpu.frame_once(&mut app);
    }
    let drawing = t.elapsed() / n;
    mouse(&mut app, Phase::Up, [500.0, 280.0], 1001.0);
    eprintln!("10k visible strokes, while drawing: {drawing:?} per frame");
    // (On a fast desktop GPU all three are dominated by the readback; the difference shows on
    // phones – see the frame stats in logcat.)
    let _ = (moving, drawing, full);
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
    assert!(dark(&px, 384, 150) && dark(&px, 386, 260), "inside the line (below the toolbar)");
    assert!(!dark(&px, 393, 150) && !dark(&px, 393, 260), "outside the edge");
    // Straighten again.
    app.reset_rotation();
    app.wheel_zoom([389.0, 150.0], -8.0);
    let px = gpu.frame(&mut app);
    assert!(dark(&px, 350, 150) && !dark(&px, 384, 110));
}

#[test]
fn erased_edge_stays_sharp_when_magnified() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    draw_line(&mut app, 150.0, 100.0, 300.0);
    app.tools.tool = infinite_canvas::ui::Tool::Eraser;
    app.tools.eraser = 24.0;
    mouse(&mut app, Phase::Down, [200.0, 150.0], 2.0);
    mouse(&mut app, Phase::Up, [200.0, 150.0], 2.1);
    // 2^10 around the hole's right edge (x = 212): both the line and the eraser now take
    // the exact CPU path. The edge stays at x = 212 and stays sharp.
    app.wheel_zoom([212.0, 150.0], 10.0);
    let px = gpu.frame(&mut app);
    assert!(!dark(&px, 209, 150) && !dark(&px, 209, 60), "erased side");
    assert!(dark(&px, 215, 150) && dark(&px, 215, 240), "ink side");
}

#[test]
fn long_stroke_in_progress_is_continuous() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    // ~600 samples (many frozen parts), finger still down.
    mouse(&mut app, Phase::Down, [40.0, 150.0], 0.0);
    for i in 1..=600 {
        let x = 40.0 + i as f64 * 1.15;
        mouse(&mut app, Phase::Move, [x, 150.0 + (x * 0.05).sin() * 20.0], i as f64 / 240.0);
    }
    let px = gpu.frame(&mut app);
    // Every column along the stroke has ink near the curve: no gaps at part joints.
    for x in (50..700).step_by(3) {
        let y = 150.0 + (x as f64 * 0.05).sin() * 20.0;
        let hit = (-6..=6).any(|dy| dark(&px, x, (y as i32 + dy) as u32));
        assert!(hit, "gap at x = {x}");
    }
    mouse(&mut app, Phase::Up, [730.0, 150.0], 3.0);
    assert_eq!(app.doc.len(), 1);
}

fn gray(px: &[u8], x: u32, y: u32) -> u8 {
    px[((y * W + x) * 4) as usize]
}

/// Draws through the given points with the mouse (one stroke).
fn draw_path(app: &mut CanvasApp, pts: &[[f64; 2]], t0: f64) {
    mouse(app, Phase::Down, pts[0], t0);
    let mut t = t0;
    for w in pts.windows(2) {
        for k in 1..=40 {
            let f = k as f64 / 40.0;
            t += 1.0 / 240.0;
            mouse(app, Phase::Move, [w[0][0] + (w[1][0] - w[0][0]) * f, w[0][1] + (w[1][1] - w[0][1]) * f], t);
        }
    }
    for _ in 0..30 {
        t += 1.0 / 240.0;
        mouse(app, Phase::Move, *pts.last().unwrap(), t);
    }
    mouse(app, Phase::Up, *pts.last().unwrap(), t);
}

#[test]
fn translucent_brushes_blend_once_per_stroke() {
    use canvas_core::stroke::Brush;
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    app.tools.set_brush(Brush::Marker);
    app.tools.width = 16.0;
    // A: one pass. B: there and back over itself. C: crosses A (a second stroke).
    // (Below the toolbar, which covers the top ~110 px.)
    draw_path(&mut app, &[[100.0, 160.0], [500.0, 160.0]], 0.0);
    draw_path(&mut app, &[[100.0, 240.0], [500.0, 240.0], [150.0, 240.0]], 5.0);
    draw_path(&mut app, &[[300.0, 125.0], [300.0, 200.0]], 10.0);
    let px = gpu.frame(&mut app);
    let (a, b, both) = (gray(&px, 400, 160), gray(&px, 400, 240), gray(&px, 300, 160));
    eprintln!("marker: single {a}, over itself {b}, two strokes {both}");
    assert!(a > 40 && a < 200, "translucent: {a}");
    assert!((a as i32 - b as i32).abs() <= 3, "a stroke over itself is not darker: {a} vs {b}");
    assert!(both + 25 < a, "two strokes stack: {both} vs {a}");
}

#[test]
fn pencil_dotted_and_fill() {
    use canvas_core::stroke::Brush;
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    // Pencil: lighter than ink, with grain (not uniform along the line).
    app.tools.set_brush(Brush::SoftPencil);
    app.tools.width = 8.0;
    draw_path(&mut app, &[[50.0, 150.0], [350.0, 150.0]], 0.0);
    // Dotted: dots with gaps.
    app.tools.set_brush(Brush::Dotted);
    app.tools.width = 6.0;
    draw_path(&mut app, &[[50.0, 220.0], [350.0, 220.0]], 5.0);
    // Fill: a closed loop is filled inside.
    app.tools.set_brush(Brush::Fill);
    draw_path(&mut app, &[[450.0, 130.0], [700.0, 130.0], [700.0, 280.0], [450.0, 280.0], [450.0, 130.0]], 10.0);
    let px = gpu.frame(&mut app);

    let pencil: Vec<u8> = (60..340).map(|x| gray(&px, x, 150)).collect();
    let (lo, hi) = (*pencil.iter().min().unwrap(), *pencil.iter().max().unwrap());
    eprintln!("pencil range {lo}..{hi}");
    assert!(hi > 60, "pencil is not solid black");
    assert!(hi - lo > 30, "pencil has grain");
    assert!(lo < 200, "pencil is visible");

    let dots: Vec<bool> = (60..340).map(|x| dark(&px, x, 220)).collect();
    let changes = dots.windows(2).filter(|w| w[0] != w[1]).count();
    assert!(changes >= 10, "dotted line has gaps ({changes} edges)");

    assert!(dark(&px, 575, 200) && dark(&px, 500, 260), "inside of the fill");
    assert!(!dark(&px, 730, 200), "outside of the fill");
}


#[test]
fn reference_photo_under_strokes() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    // A pure red 400×200 photo, placed centred (fits 80% of 768×300 → 480×240 at (144, 30)).
    let img = image::RgbaImage::from_pixel(400, 200, image::Rgba([255, 0, 0, 255]));
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    app.add_photo(&png).unwrap();
    assert!(app.image_edit.is_some(), "a new photo starts in move mode");
    // Moving it with the "finger" moves the photo, not the view.
    mouse(&mut app, Phase::Down, [400.0, 200.0], 0.0);
    mouse(&mut app, Phase::Move, [450.0, 200.0], 0.1);
    mouse(&mut app, Phase::Up, [450.0, 200.0], 0.2);
    assert!(app.back(), "back finishes moving");
    assert!(app.image_edit.is_none());
    let px = gpu.frame(&mut app);
    let at = |x: u32, y: u32| { let i = ((y * W + x) * 4) as usize; [px[i], px[i + 1], px[i + 2]] };
    // Opacity 0.5 over the light background: pinkish red. Right of the old right edge (624)
    // because it was moved 50 px; left of the new left edge (194) there is no photo.
    let c = at(650, 250);
    assert!(c[0] > 200 && c[1] > 90 && c[1] < 160, "half-transparent red: {c:?}");
    assert!(at(170, 250)[1] > 230, "background left of the moved photo");

    // Ink goes on top; the eraser removes ink but not the photo.
    draw_line(&mut app, 250.0, 300.0, 500.0);
    assert!(dark(&gpu.frame(&mut app), 400, 250));
    app.tools.tool = infinite_canvas::ui::Tool::Eraser;
    app.tools.eraser = 40.0;
    mouse(&mut app, Phase::Down, [400.0, 250.0], 5.0);
    mouse(&mut app, Phase::Up, [400.0, 250.0], 5.1);
    let px2 = gpu.frame(&mut app);
    let i = ((250 * W + 400) * 4) as usize;
    let c2 = [px2[i], px2[i + 1], px2[i + 2]];
    assert!(c2[0] > 200 && c2[1] > 90 && c2[1] < 160, "photo still under the erased spot: {c2:?}");

    // Undo removes the move, then the photo.
    app.undo(); // eraser
    app.undo(); // line
    app.undo(); // move
    app.undo(); // photo
    assert_eq!(app.doc.images().count(), 0);
}

#[test]
fn photo_turned_into_a_drawing() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    app.tools.width = 3.0;
    // Dark disc (centre (200, 100), radius 80) on white, 400×200.
    let img = image::RgbaImage::from_fn(400, 200, |x, y| {
        let r = ((x as f32 - 200.0).powi(2) + (y as f32 - 100.0).powi(2)).sqrt();
        if r < 80.0 { image::Rgba([20, 20, 20, 255]) } else { image::Rgba([240, 240, 240, 255]) }
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let opts = infinite_canvas::app::DrawingOptions { detail: 0.5, colored: false, outlines: true, keep_photo: false };
    let n = app.photo_to_drawing(&png, opts).unwrap();
    assert!(n >= 1 && app.doc.images().count() == 0);
    let px = gpu.frame(&mut app);
    // Placed like a photo: ×1.2, centred at (384, 150) → the outline has radius 96 on screen.
    let near = |x: f64, y: f64| (-3..=3).any(|d| dark(&px, (x + d as f64) as u32, y as u32) || dark(&px, x as u32, (y + d as f64) as u32));
    for a in [0.0f64, 1.0, 2.0, 3.0, 4.0, 5.0] {
        let (x, y) = (384.0 + 96.0 * a.cos(), 150.0 + 96.0 * a.sin());
        if y > 120.0 {
            // (below the toolbar)
            assert!(near(x, y), "outline at angle {a}: ({x:.0}, {y:.0})");
        }
    }
    assert!(!dark(&px, 384, 150) && !dark(&px, 384, 200), "inside stays empty: it's a line drawing");
    // One undo removes the whole drawing.
    app.undo();
    assert_eq!(app.doc.len(), 0);
}

/// Visual check of photo → drawing on real pictures:
/// `PHOTOS=dir OUT=dir cargo test --release -p infinite_canvas --test render_offscreen preview_photo_drawings -- --ignored`
#[test]
#[ignore]
fn preview_photo_drawings() {
    let (Ok(dir), Ok(out)) = (std::env::var("PHOTOS"), std::env::var("OUT")) else { return };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let mut renderer = Renderer::new(&device, FORMAT);
    let (w, h) = (640u32, 1024u32);
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    files.sort();
    for path in files {
        let bytes = std::fs::read(&path).unwrap();
        for (tag, colored, outlines, detail) in [("kolor", true, true, 0.5), ("kolor-detal", true, false, 1.0), ("kontury", false, true, 0.5)] {
            let mut app = CanvasApp::new(None);
            app.resize([w, h]);
            app.tools.width = 1.5;
            let t = std::time::Instant::now();
            let n = app
                .photo_to_drawing(&bytes, infinite_canvas::app::DrawingOptions { detail, colored, outlines, keep_photo: false })
                .unwrap();
            eprintln!("{} {tag}: {n} strokes in {:?}", path.display(), t.elapsed());
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            // New meshes are tessellated within a per-frame time budget: render until done.
            let mut frames = 0;
            loop {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32))),
                    ..Default::default()
                };
                let _ = app.frame(Target::View { device: &device, queue: &queue, format: FORMAT, size: [w, h], view: &view }, &mut renderer, raw);
                frames += 1;
                if !renderer.needs_another_frame || frames > 500 {
                    break;
                }
            }
            eprintln!("  complete after {frames} frames");
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (w * h * 4) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut enc = device.create_command_encoder(&Default::default());
            enc.copy_texture_to_buffer(
                tex.as_image_copy(),
                wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) } },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            queue.submit([enc.finish()]);
            buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let data = buf.slice(..).get_mapped_range().unwrap().to_vec();
            let img = image::RgbaImage::from_raw(w, h, data).unwrap();
            let name = format!("{}-{tag}.png", path.file_stem().unwrap().to_string_lossy());
            img.save(std::path::Path::new(&out).join(name)).unwrap();
            renderer.clear_cache();
        }
    }
}

#[test]
fn photo_turned_into_a_coloured_drawing() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    // Red with a blue disc (centre (200, 100), radius 60), 400×200 → on screen ×1.2 around (384, 150).
    let img = image::RgbaImage::from_fn(400, 200, |x, y| {
        let r = ((x as f32 - 200.0).powi(2) + (y as f32 - 100.0).powi(2)).sqrt();
        if r < 60.0 { image::Rgba([30, 60, 220, 255]) } else { image::Rgba([220, 30, 40, 255]) }
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let opts = infinite_canvas::app::DrawingOptions { detail: 0.5, colored: true, outlines: false, keep_photo: false };
    app.photo_to_drawing(&png, opts).unwrap();
    let px = gpu.frame(&mut app);
    let at = |x: u32, y: u32| { let i = ((y * W + x) * 4) as usize; [px[i], px[i + 1], px[i + 2]] };
    let blue = at(384, 160);
    let red = at(200, 250);
    assert!(blue[2] > 180 && blue[0] < 80, "disc is blue: {blue:?}");
    assert!(red[0] > 180 && red[2] < 80, "background is red: {red:?}");
    // The corner of the picture is filled (no rounded-off corners): image spans x 144..624, y 30..270.
    let corner = at(620, 266);
    assert!(corner[0] > 180 && corner[2] < 80, "corner filled: {corner:?}");
    assert!(at(130, 250)[1] > 200, "outside the picture: background");
}

#[test]
fn bucket_fill_fills_the_enclosed_area() {
    use canvas_core::stroke::Brush;
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    app.tools.width = 4.0;
    // Closed square 400..700 × 130..290 and a small closed square (island) inside it.
    draw_path(&mut app, &[[400.0, 130.0], [700.0, 130.0], [700.0, 290.0], [400.0, 290.0], [400.0, 130.0]], 0.0);
    draw_path(&mut app, &[[530.0, 190.0], [570.0, 190.0], [570.0, 230.0], [530.0, 230.0], [530.0, 190.0]], 5.0);
    assert_eq!(app.doc.len(), 2);
    app.tools.set_brush(Brush::Fill);
    app.tools.color = egui::Color32::from_rgb(230, 40, 40);
    let tap = |app: &mut CanvasApp, p: [f64; 2], t: f64| {
        mouse(app, Phase::Down, p, t);
        mouse(app, Phase::Up, p, t + 0.05);
    };
    tap(&mut app, [450.0, 260.0], 10.0);
    let px = gpu.frame(&mut app);
    assert_eq!(app.doc.len(), 3, "one fill stroke");
    let at = |x: u32, y: u32| { let i = ((y * W + x) * 4) as usize; [px[i], px[i + 1], px[i + 2]] };
    let red = |c: [u8; 3]| c[0] > 180 && c[1] < 90;
    assert!(red(at(450, 260)) && red(at(650, 160)) && red(at(510, 210)), "inside is filled");
    assert!(!red(at(550, 210)), "the island stays empty");
    assert!(!red(at(380, 200)) && !red(at(720, 200)), "outside not filled");
    assert!(dark(&px, 400, 200) && dark(&px, 700, 220), "the outline stays on top / visible");
    // Outside the square nothing is enclosed: no stroke added.
    tap(&mut app, [200.0, 250.0], 12.0);
    gpu.frame(&mut app);
    assert_eq!(app.doc.len(), 3);
    // Undo removes the fill.
    app.undo();
    assert!(!red({ let px = gpu.frame(&mut app); let i = ((260 * W + 450) * 4) as usize; [px[i], px[i + 1], px[i + 2]] }));
}

#[test]
fn export_saves_a_double_resolution_png() {
    use infinite_canvas::photos::{PhotoBackend, PhotoEntry};
    use std::sync::{Arc, Mutex};
    type Saved = Arc<Mutex<Option<(String, Vec<u8>)>>>;
    struct Capture(Saved);
    impl PhotoBackend for Capture {
        fn list(&mut self) -> Result<Vec<PhotoEntry>, String> {
            Err("none".into())
        }
        fn read(&mut self, _: &str) -> Option<Vec<u8>> {
            None
        }
        fn save_png(&mut self, name: &str, png: &[u8]) -> Result<String, String> {
            *self.0.lock().unwrap() = Some((name.to_string(), png.to_vec()));
            Ok("test".into())
        }
    }
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let saved = Arc::new(Mutex::new(None));
    let mut app = new_app();
    app.set_photo_backend(Box::new(Capture(saved.clone())));
    draw_line(&mut app, 200.0, 100.0, 600.0);
    app.export();
    for _ in 0..200 {
        gpu.frame(&mut app);
        if saved.lock().unwrap().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let (name, png) = saved.lock().unwrap().clone().expect("exported");
    assert!(name.starts_with("rysunek-"));
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (2 * W, 2 * H), "twice the screen resolution");
    // The line (y = 200 on screen) is at y = 400 in the picture; no toolbar in the export.
    assert!(img.get_pixel(700, 400).0[0] < 100, "line drawn");
    assert!(img.get_pixel(100, 60).0[0] > 200, "no UI in the picture");
}

#[test]
fn bookmark_flight_returns_to_the_exact_view() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    draw_line(&mut app, 200.0, 300.0, 500.0);
    app.add_bookmark();
    let before = gpu.frame(&mut app);
    assert!(dark(&before, 400, 200));
    // Wander off: far away and 2^40 deeper, rotated.
    app.pan([5.0e5, -2.0e5]);
    app.wheel_zoom([100.0, 100.0], 40.0);
    app.wheel_rotate([300.0, 150.0], 1.0);
    assert!(!dark(&gpu.frame(&mut app), 400, 200));
    app.fly_to_bookmark(0);
    let start = std::time::Instant::now();
    while app.animating() && start.elapsed().as_secs_f64() < 6.0 {
        gpu.frame(&mut app);
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    let after = gpu.frame(&mut app);
    assert_eq!(app.camera.rotation, 0.0);
    assert!(dark(&after, 400, 200) && dark(&after, 310, 200) && !dark(&after, 400, 230), "back at the saved view");
}

#[test]
fn shape_guides() {
    use canvas_core::stroke::Brush;
    use infinite_canvas::shapes::Shape;
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    app.tools.width = 4.0;
    let drag = |app: &mut CanvasApp, a: [f64; 2], b: [f64; 2], t: f64| {
        mouse(app, Phase::Down, a, t);
        for k in 1..=10 {
            let f = k as f64 / 10.0;
            mouse(app, Phase::Move, [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f], t + k as f64 * 0.01);
        }
        mouse(app, Phase::Up, b, t + 0.2);
    };
    app.tools.shape = Some(Shape::Rectangle);
    drag(&mut app, [100.0, 130.0], [300.0, 280.0], 0.0);
    // An ellipse with the fill brush (near-square drag → circle of radius 70 around (520, 205)).
    app.tools.set_brush(Brush::Fill);
    app.tools.shape = Some(Shape::Ellipse);
    drag(&mut app, [450.0, 135.0], [590.0, 273.0], 1.0);
    // A tiny accidental drag adds nothing.
    drag(&mut app, [700.0, 200.0], [701.0, 200.0], 2.0);
    assert_eq!(app.doc.len(), 2);
    let px = gpu.frame(&mut app);
    assert!(dark(&px, 100, 130) && dark(&px, 300, 280) && dark(&px, 200, 130) && dark(&px, 300, 200), "rectangle edges and sharp corners");
    assert!(!dark(&px, 200, 200), "rectangle is not filled");
    assert!(dark(&px, 520, 205) && dark(&px, 520, 140) && dark(&px, 455, 205), "filled circle");
    assert!(!dark(&px, 460, 145), "circle, not a square");
}

#[test]
fn layers_erase_hide_opacity_and_order() {
    use infinite_canvas::ui::LayerAction;
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    let at = |px: &[u8], x: u32, y: u32| { let i = ((y * W + x) * 4) as usize; [px[i], px[i + 1], px[i + 2]] };
    // Bottom layer: a red horizontal line.
    app.tools.color = egui::Color32::from_rgb(230, 30, 30);
    draw_line(&mut app, 200.0, 100.0, 600.0);
    let bottom = app.active_layer;
    // Top layer: a blue vertical line crossing it, and an eraser over the crossing.
    app.layer_action(LayerAction::Add);
    let top = app.active_layer;
    assert_ne!(top, bottom);
    app.tools.color = egui::Color32::from_rgb(30, 30, 230);
    draw_path(&mut app, &[[350.0, 130.0], [350.0, 280.0]], 5.0);
    let px = gpu.frame(&mut app);
    assert!(at(&px, 350, 200)[2] > 180, "blue on top: {:?}", at(&px, 350, 200));
    app.tools.tool = infinite_canvas::ui::Tool::Eraser;
    app.tools.eraser = 30.0;
    mouse(&mut app, Phase::Down, [350.0, 200.0], 9.0);
    mouse(&mut app, Phase::Up, [350.0, 200.0], 9.1);
    let px = gpu.frame(&mut app);
    let c = at(&px, 350, 200);
    assert!(c[0] > 180 && c[2] < 90, "eraser on the top layer reveals the red line below: {c:?}");
    assert!(at(&px, 350, 260)[2] > 180, "blue line elsewhere intact");
    // Top layer at half opacity: the blue line becomes lighter.
    app.layer_action(LayerAction::Opacity(top, 0.5));
    let c = at(&gpu.frame(&mut app), 350, 260);
    assert!(c[0] > 100 && c[2] > 180, "half-transparent blue: {c:?}");
    // Hide the bottom layer: the red line disappears.
    app.layer_action(LayerAction::ToggleVisible(bottom));
    assert!(at(&gpu.frame(&mut app), 200, 200)[1] > 200);
    app.layer_action(LayerAction::ToggleVisible(bottom));
    // Move the blue layer below the red one: at a crossing point away from the eraser, red wins.
    app.layer_action(LayerAction::Opacity(top, 1.0));
    draw_path(&mut app, &[[500.0, 130.0], [500.0, 280.0]], 12.0); // tools: eraser still selected
    app.tools.tool = infinite_canvas::ui::Tool::Pen;
    draw_path(&mut app, &[[500.0, 130.0], [500.0, 280.0]], 15.0);
    assert!(at(&gpu.frame(&mut app), 500, 200)[2] > 180, "blue over red");
    app.layer_action(LayerAction::Down(top));
    let c = at(&gpu.frame(&mut app), 500, 200);
    assert!(c[0] > 180 && c[2] < 90, "after moving the layer down, red is on top: {c:?}");
    // The layers survive saving.
    let bytes = canvas_core::io::encode(&app.doc, &app.camera);
    let (doc, _) = canvas_core::io::decode(&bytes).unwrap();
    assert_eq!(doc.layers.len(), 2);
    assert_eq!(doc.layers[0].id, top);
}

/// A busy, colourful photo drawing: many large fills to tessellate.
fn busy_photo() -> Vec<u8> {
    let img = image::RgbaImage::from_fn(600, 300, |x, y| {
        let v = ((x as f32 * 0.05).sin() * (y as f32 * 0.07).cos() * 0.5 + 0.5) * 120.0 + 120.0;
        let w = (((x / 9 + y / 7) % 5) * 12) as f32;
        image::Rgba([(v + w) as u8, (240.0 - w * 2.0) as u8, (v * 0.8) as u8, 255])
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    png
}

#[test]
fn a_new_stroke_shows_immediately_even_when_busy() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    let opts = infinite_canvas::app::DrawingOptions { detail: 1.0, colored: true, outlines: false, keep_photo: false };
    let n = app.photo_to_drawing(&busy_photo(), opts).unwrap();
    assert!(n > 200, "{n}");
    gpu.frame_once(&mut app); // lots of tessellation still in flight
    app.tools.color = egui::Color32::BLACK;
    app.tools.width = 6.0;
    draw_line(&mut app, 230.0, 150.0, 650.0);
    let px = gpu.frame_once(&mut app);
    for x in [200, 400, 600] {
        assert!(dark(&px, x, 230), "new line visible in the very first frame at x = {x}");
    }
}

#[test]
fn zooming_never_shows_holes() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    app.tools.width = 6.0;
    // A grid of strokes covering the screen.
    for i in 0..30 {
        let y = 120.0 + i as f64 * 6.0;
        draw_line(&mut app, y, 20.0, 740.0);
    }
    for _ in 0..5 {
        gpu.frame(&mut app);
    }
    let covered = |px: &[u8]| (0..40).filter(|k| dark(px, 60 + k * 15, 200)).count();
    assert_eq!(covered(&gpu.frame(&mut app)), 40);
    // The first frame after a zoom must already show (nearly) everything the final,
    // fully tessellated frame shows: no white holes while new LODs are being made.
    let mut check = |app: &mut CanvasApp, what: &str| {
        let first = gpu.frame_once(app);
        let mut last = first.clone();
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            last = gpu.frame_once(app);
        }
        let ink = |px: &[u8]| -> Vec<bool> { (0..W * H).map(|i| (i / W) > 115 && dark(px, i % W, i / W)).collect() };
        let (a, b) = (ink(&first), ink(&last));
        let total = b.iter().filter(|v| **v).count();
        let missing = a.iter().zip(&b).filter(|(x, y)| !**x && **y).count();
        assert!(total > 1000 && missing * 100 <= total, "{what}: {missing} of {total} ink pixels missing in the first frame");
    };
    app.wheel_zoom([384.0, 200.0], -1.5);
    check(&mut app, "zoom out");
    app.wheel_zoom([384.0, 200.0], 4.0);
    check(&mut app, "zoom in");
}

#[test]
fn move_tool_pans_with_one_finger() {
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let mut app = new_app();
    draw_line(&mut app, 200.0, 300.0, 500.0);
    assert_eq!(app.doc.len(), 1);
    app.tools.tool = infinite_canvas::ui::Tool::Hand;
    let touch = |app: &mut CanvasApp, phase, pos, t| {
        app.pointer(PointerEvent { id: 7, kind: PointerKind::Touch, phase, pos, pressure: 1.0, time: t, button: MouseButton::Primary })
    };
    touch(&mut app, Phase::Down, [400.0, 250.0], 10.0);
    for k in 1..=10 {
        touch(&mut app, Phase::Move, [400.0, 250.0 - k as f64 * 6.0], 10.0 + k as f64 * 0.02);
    }
    std::thread::sleep(std::time::Duration::from_millis(300)); // let the fingers "rest": no fling
    touch(&mut app, Phase::Up, [400.0, 190.0], 11.0);
    assert_eq!(app.doc.len(), 1, "nothing drawn");
    std::thread::sleep(std::time::Duration::from_millis(200));
    let px = gpu.frame(&mut app);
    assert!(dark(&px, 400, 140) && !dark(&px, 400, 200), "the drawing moved up by 60 px");
}

#[test]
fn video_of_a_flight_through_bookmarks() {
    if std::process::Command::new("ffprobe").arg("-version").output().is_err() {
        eprintln!("no ffmpeg: skipping");
        return;
    }
    let Some(mut gpu) = headless() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("icnv-video-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = new_app();
    app.set_video_factory(infinite_canvas::video::ffmpeg_factory_in(dir.clone()));
    draw_line(&mut app, 200.0, 300.0, 500.0);
    app.add_bookmark();
    app.pan([-3000.0, 0.0]);
    app.wheel_zoom([384.0, 150.0], 3.0);
    draw_line(&mut app, 200.0, 300.0, 500.0);
    app.add_bookmark();
    app.start_video();
    let start = std::time::Instant::now();
    while app.exporting_video() && start.elapsed().as_secs() < 120 {
        gpu.frame_once(&mut app);
    }
    assert!(!app.exporting_video(), "finished");
    let file = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "mp4")).expect("an mp4");
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-count_frames", "-show_entries", "stream=width,height,nb_read_frames,codec_name", "-of", "csv=p=0"])
        .arg(&file)
        .output()
        .unwrap();
    let info = String::from_utf8_lossy(&probe.stdout).trim().to_string();
    eprintln!("video: {info}");
    let parts: Vec<&str> = info.split(',').collect();
    assert_eq!(parts[0], "h264");
    assert_eq!((parts[1], parts[2]), ("768", "304"), "768 wide, screen aspect (768×300 → height rounded to 16)");
    let frames: usize = parts[3].parse().unwrap();
    assert!(frames > 2 * 24 + 30, "{frames} frames: holds + flight");
    std::fs::remove_dir_all(dir).unwrap();
}
