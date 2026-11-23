//! Android launcher (NativeActivity via android-activity).
//!
//! winit 0.30 hides the pointer tool type and the batched historical samples on Android, so
//! input is read straight from `MotionEvent`: stylus vs finger (palm rejection), pressure and
//! all intermediate samples of fast pen movements.

use crate::app::CanvasApp;
use crate::gestures::{MouseButton, Phase, PointerEvent, PointerKind};
use crate::render::{Gpu, Renderer, Target};
use android_activity::input::{InputEvent, MotionAction, MotionEvent, ToolType};
use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent};
use raw_window_handle::{AndroidDisplayHandle, HasWindowHandle, RawDisplayHandle};
use std::time::{Duration, Instant};

struct Graphics {
    gpu: Gpu,
    renderer: Renderer,
    // Keeps the native window alive as long as the surface.
    _window: ndk::native_window::NativeWindow,
}

struct State {
    canvas: CanvasApp,
    instance: wgpu::Instance,
    gfx: Option<Graphics>,
    egui_events: Vec<egui::Event>,
    start: Instant,
    size: [u32; 2],
    /// Pointer currently forwarded to egui (first finger / pen).
    egui_pointer: Option<u64>,
}

fn kind(t: ToolType) -> PointerKind {
    match t {
        ToolType::Stylus => PointerKind::Pen,
        ToolType::Eraser => PointerKind::PenEraser,
        ToolType::Mouse => PointerKind::Mouse,
        _ => PointerKind::Touch,
    }
}

impl State {
    fn create_graphics(&mut self, app: &AndroidApp) {
        let Some(window) = app.native_window() else { return };
        let size = [window.width() as u32, window.height() as u32];
        let raw_window = window.window_handle().expect("window handle").as_raw();
        // SAFETY: the window is kept alive in `Graphics` for as long as the surface exists.
        let surface = unsafe {
            self.instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(RawDisplayHandle::Android(AndroidDisplayHandle::new())),
                raw_window_handle: raw_window,
            })
        }
        .expect("cannot create surface");
        let gpu = pollster::block_on(Gpu::new(&self.instance, surface, size));
        let renderer = Renderer::new(&gpu.device, gpu.config.format);
        self.size = size;
        self.canvas.resize(size);
        self.canvas.reset_ui();
        self.egui_pointer = None;
        self.gfx = Some(Graphics { gpu, renderer, _window: window });
    }

    fn resize(&mut self, app: &AndroidApp) {
        let Some(window) = app.native_window() else { return };
        let size = [window.width() as u32, window.height() as u32];
        if size != self.size {
            self.size = size;
            if let Some(g) = self.gfx.as_mut() {
                g.gpu.resize(size);
            }
            self.canvas.resize(size);
        }
        let r = app.content_rect();
        log::debug!("window {size:?}, content rect {:?}", (r.left, r.top, r.right, r.bottom));
        self.canvas.safe_top = r.top.max(0) as f32 / self.canvas.pixels_per_point;
    }

    fn time(&self, ns: i64) -> f64 {
        // MotionEvent times are in the uptime clock (ns); only differences matter.
        ns as f64 * 1e-9
    }

    fn egui_pos(&self, p: [f64; 2]) -> egui::Pos2 {
        let ppp = self.canvas.pixels_per_point;
        egui::pos2(p[0] as f32 / ppp, p[1] as f32 / ppp)
    }

    fn motion(&mut self, ev: &MotionEvent) {
        let action = ev.action();
        let time = self.time(ev.event_time());
        let emit = |s: &mut State, idx: usize, phase: Phase, pos: Option<([f64; 2], f32, f64)>| {
            let p = ev.pointer_at_index(idx);
            let id = p.pointer_id() as u64 + 1;
            let (pos, pressure, t) = pos.unwrap_or(([p.x() as f64, p.y() as f64], p.pressure(), time));
            let k = kind(p.tool_type());
            s.canvas.pointer(PointerEvent { id, kind: k, phase, pos, pressure, time: t, button: MouseButton::Primary });
            // egui sees the first pointer as a mouse.
            let ep = s.egui_pos(pos);
            match phase {
                Phase::Down if s.egui_pointer.is_none() => {
                    s.egui_pointer = Some(id);
                    s.egui_events.push(egui::Event::PointerMoved(ep));
                    s.egui_events.push(egui::Event::PointerButton {
                        pos: ep,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    });
                }
                Phase::Move if s.egui_pointer == Some(id) => s.egui_events.push(egui::Event::PointerMoved(ep)),
                Phase::Up | Phase::Cancel if s.egui_pointer == Some(id) => {
                    s.egui_pointer = None;
                    s.egui_events.push(egui::Event::PointerButton {
                        pos: ep,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    });
                    s.egui_events.push(egui::Event::PointerGone);
                }
                _ => {}
            }
        };
        match action {
            MotionAction::Down | MotionAction::PointerDown => emit(self, ev.pointer_index(), Phase::Down, None),
            MotionAction::Up | MotionAction::PointerUp => emit(self, ev.pointer_index(), Phase::Up, None),
            MotionAction::Cancel => {
                for i in 0..ev.pointer_count() {
                    emit(self, i, Phase::Cancel, None);
                }
            }
            MotionAction::Move => {
                for i in 0..ev.pointer_count() {
                    // Batched samples first (they are older), then the current one.
                    let hist: Vec<([f64; 2], f32, f64)> = ev
                        .pointer_at_index(i)
                        .history()
                        .map(|h| ([h.x() as f64, h.y() as f64], h.pressure(), self.time(h.event_time())))
                        .collect();
                    for h in hist {
                        emit(self, i, Phase::Move, Some(h));
                    }
                    emit(self, i, Phase::Move, None);
                }
            }
            _ => {}
        }
    }

    fn frame(&mut self) {
        let Some(g) = self.gfx.as_mut() else { return };
        let ppp = self.canvas.pixels_per_point;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(self.size[0] as f32 / ppp, self.size[1] as f32 / ppp),
            )),
            time: Some(self.start.elapsed().as_secs_f64()),
            max_texture_side: Some(g.gpu.device.limits().max_texture_dimension_2d as usize),
            events: std::mem::take(&mut self.egui_events),
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
        let _ = self.canvas.frame(Target::Surface(&g.gpu), &mut g.renderer, raw);
    }
}

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("infinite_canvas"),
    );
    std::panic::set_hook(Box::new(|info| log::error!("PANIC: {info}")));
    let path = app.internal_data_path().map(|p| p.join("canvas.icnv"));
    log::info!("drawing file: {path:?}");
    let density = app.config().density().unwrap_or(160) as f32 / 160.0;
    let mut canvas = CanvasApp::new(path);
    canvas.pixels_per_point = density.max(1.0);
    let mut s = State {
        canvas,
        instance: wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env()),
        gfx: None,
        egui_events: Vec::new(),
        start: Instant::now(),
        size: [1, 1],
        egui_pointer: None,
    };
    let mut quit = false;
    while !quit {
        let timeout = if s.gfx.is_some() && s.canvas.animating() {
            Some(Duration::ZERO)
        } else {
            s.canvas.next_wakeup().or(Some(Duration::from_secs(1)))
        };
        app.poll_events(timeout, |event| {
            if let PollEvent::Main(e) = event {
                match e {
                    MainEvent::InitWindow { .. } => {
                        s.create_graphics(&app);
                        s.resize(&app);
                        s.canvas.wants_redraw = true;
                    }
                    MainEvent::TerminateWindow { .. } => {
                        s.canvas.save_now();
                        s.gfx = None;
                    }
                    MainEvent::WindowResized { .. }
                    | MainEvent::ContentRectChanged { .. }
                    | MainEvent::ConfigChanged { .. }
                    | MainEvent::InsetsChanged { .. }
                    | MainEvent::RedrawNeeded { .. } => {
                        s.resize(&app);
                        s.canvas.wants_redraw = true;
                    }
                    MainEvent::Pause | MainEvent::SaveState { .. } | MainEvent::Stop => s.canvas.save_now(),
                    MainEvent::Destroy => quit = true,
                    _ => {}
                }
            }
        });
        if let Ok(mut iter) = app.input_events_iter() {
            loop {
                let more = iter.next(|event| match event {
                    InputEvent::MotionEvent(m) => {
                        s.motion(m);
                        InputStatus::Handled
                    }
                    _ => InputStatus::Unhandled,
                });
                if !more {
                    break;
                }
            }
        }
        if s.gfx.is_some() && (s.canvas.animating() || !s.egui_events.is_empty()) {
            s.frame();
        } else if s.gfx.is_some() && s.canvas.next_wakeup() == Some(Duration::ZERO) {
            // Lets the app run its autosave even when idle.
            s.frame();
        }
    }
    s.canvas.save_now();
}
