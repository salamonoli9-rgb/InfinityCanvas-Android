//! Desktop launcher (Linux etc.): winit window, mouse acts as the pen.
//!
//! LMB draws, RMB / MMB drag pans, wheel zooms around the cursor (Ctrl+wheel = fine zoom),
//! touchpad two-finger scroll pans, pinch zooms.

use crate::app::CanvasApp;
use crate::gestures::{MouseButton, Phase, PointerEvent, PointerKind};
use crate::render::{Gpu, Renderer};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

struct Running {
    window: Arc<Window>,
    gpu: Gpu,
    renderer: Renderer,
    egui: egui_winit::State,
}

struct Desktop {
    app: CanvasApp,
    instance: wgpu::Instance,
    running: Option<Running>,
    start: Instant,
    cursor: [f64; 2],
    buttons: Vec<MouseButton>,
    ctrl: bool,
    alt: bool,
}

fn data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| ".".into());
    base.join("infinite-canvas")
}

impl Desktop {
    fn time(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn mouse(&mut self, phase: Phase, button: MouseButton) {
        let e = PointerEvent {
            id: 0,
            kind: PointerKind::Mouse,
            phase,
            pos: self.cursor,
            pressure: 1.0,
            time: self.time(),
            button,
        };
        self.app.pointer(e);
    }

    fn handle(&mut self, el: &ActiveEventLoop, r: &mut Running, event: WindowEvent) {
        let resp = r.egui.on_window_event(&r.window, &event);
        if resp.repaint {
            r.window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => {
                self.app.save_now();
                el.exit();
            }
            WindowEvent::Resized(s) => {
                r.gpu.resize([s.width, s.height]);
                self.app.resize([s.width, s.height]);
                r.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.app.pixels_per_point = scale_factor as f32;
            }
            WindowEvent::ModifiersChanged(m) => {
                self.ctrl = m.state().control_key();
                self.alt = m.state().alt_key();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = [position.x, position.y];
                if let Some(&b) = self.buttons.first() {
                    self.mouse(Phase::Move, b);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    winit::event::MouseButton::Left => MouseButton::Primary,
                    winit::event::MouseButton::Right => MouseButton::Secondary,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    _ => return,
                };
                match state {
                    ElementState::Pressed => {
                        // Only the first held button drives a gesture.
                        if self.buttons.is_empty() {
                            self.mouse(Phase::Down, b);
                        }
                        self.buttons.push(b);
                    }
                    ElementState::Released => {
                        if self.buttons.first() == Some(&b) {
                            self.mouse(Phase::Up, b);
                        }
                        self.buttons.retain(|&x| x != b);
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if resp.consumed {
                    return;
                }
                match delta {
                    // Alt + wheel rotates (7.5° per notch).
                    MouseScrollDelta::LineDelta(_, y) if self.alt => {
                        self.app.wheel_rotate(self.cursor, y as f64 * 7.5f64.to_radians());
                    }
                    MouseScrollDelta::LineDelta(_, y) => {
                        let step = if self.ctrl { 0.05 } else { 0.25 };
                        self.app.wheel_zoom(self.cursor, y as f64 * step);
                    }
                    // Touchpads: scroll pans, Ctrl+scroll zooms.
                    MouseScrollDelta::PixelDelta(p) => {
                        if self.ctrl {
                            self.app.wheel_zoom(self.cursor, p.y / 200.0);
                        } else {
                            self.app.pan([p.x, p.y]);
                        }
                    }
                }
            }
            WindowEvent::DroppedFile(path) => match std::fs::read(&path) {
                Ok(b) => self.app.offer_photo(b),
                Err(e) => log::warn!("cannot read {}: {e}", path.display()),
            },
            WindowEvent::PinchGesture { delta, .. } => self.app.wheel_zoom(self.cursor, (1.0 + delta).log2()),
            WindowEvent::RotationGesture { delta, .. } => self.app.wheel_rotate(self.cursor, -(delta as f64).to_radians()),
            WindowEvent::Touch(t) => {
                let phase = match t.phase {
                    TouchPhase::Started => Phase::Down,
                    TouchPhase::Moved => Phase::Move,
                    TouchPhase::Ended => Phase::Up,
                    TouchPhase::Cancelled => Phase::Cancel,
                };
                let e = PointerEvent {
                    id: t.id + 1,
                    kind: PointerKind::Touch,
                    phase,
                    pos: [t.location.x, t.location.y],
                    pressure: 1.0,
                    time: self.time(),
                    button: MouseButton::Primary,
                };
                self.app.pointer(e);
            }
            WindowEvent::RedrawRequested => {
                let raw = r.egui.take_egui_input(&r.window);
                let out = self.app.frame(crate::render::Target::Surface(&r.gpu), &mut r.renderer, raw);
                r.egui.handle_platform_output(&r.window, out);
            }
            _ => {}
        }
    }
}

impl ApplicationHandler for Desktop {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("InfinityCanvas")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        let window = Arc::new(el.create_window(attrs).expect("cannot create window"));
        let size = window.inner_size();
        let surface = self.instance.create_surface(window.clone()).expect("cannot create surface");
        let gpu = pollster::block_on(Gpu::new(&self.instance, surface, [size.width, size.height]));
        let renderer = Renderer::new(&gpu.device, gpu.config.format);
        let egui = egui_winit::State::new(
            self.app.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        self.app.pixels_per_point = window.scale_factor() as f32;
        self.app.resize([size.width, size.height]);
        window.request_redraw();
        self.running = Some(Running { window, gpu, renderer, egui });
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(mut r) = self.running.take() else { return };
        self.handle(el, &mut r, event);
        if self.app.animating() {
            r.window.request_redraw();
        }
        self.running = Some(r);
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some(r) = &self.running {
            if self.app.animating() {
                r.window.request_redraw();
                el.set_control_flow(ControlFlow::Wait);
            } else if let Some(d) = self.app.next_wakeup() {
                el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + d));
                if d.is_zero() {
                    r.window.request_redraw();
                }
            } else {
                el.set_control_flow(ControlFlow::Wait);
            }
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        self.app.save_now();
    }
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"))
        .init();
    // A file argument opens that one drawing; otherwise the projects folder is used.
    let app = match std::env::args().nth(1).map(std::path::PathBuf::from) {
        Some(path) => {
            log::info!("drawing file: {}", path.display());
            CanvasApp::new(Some(path))
        }
        None => {
            let dir = data_dir();
            log::info!("data folder: {}", dir.display());
            let mut app = CanvasApp::with_projects(&dir);
            let locale = std::env::var("LC_ALL").or_else(|_| std::env::var("LANG")).unwrap_or_default();
            app.load_settings(&dir, crate::i18n::Lang::from_code(&locale));
            app
        }
    };
    let mut app = app;
    app.set_photo_backend(Box::new(crate::photos::FolderPhotos::default_dirs()));
    app.set_video_factory(crate::video::ffmpeg_factory());
    let event_loop = EventLoop::new().expect("cannot create event loop");
    let mut d = Desktop {
        app,
        instance: wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
            event_loop.owned_display_handle(),
        ))),
        running: None,
        start: Instant::now(),
        cursor: [0.0; 2],
        buttons: Vec::new(),
        ctrl: false,
        alt: false,
    };
    event_loop.run_app(&mut d).expect("event loop failed");
}
