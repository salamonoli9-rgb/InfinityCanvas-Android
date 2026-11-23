//! Platform-independent application state: document, camera, tools, input → actions.

use crate::gestures::{Gesture, Gestures, PointerEvent, PointerKind};
use crate::render::{FrameInput, Renderer, Target};
use crate::ui::{self, Tool, ToolState};
use canvas_core::camera::Camera;
use canvas_core::document::Document;
use canvas_core::history::{Action, History};
use canvas_core::io;
use canvas_core::smoothing::PointerSmoother;
use canvas_core::stroke::{Stroke, StrokePoint};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const BACKGROUND: [f32; 4] = [0.98, 0.976, 0.965, 1.0];
const AUTOSAVE_DELAY: Duration = Duration::from_millis(1500);
/// Pan inertia decay rate (1/s) and zoom inertia decay rate.
const PAN_DECAY: f64 = 4.0;
const ZOOM_DECAY: f64 = 6.0;
/// Minimum distance between stored samples, in local units (≈ px at drawing time).
const MIN_SAMPLE_DIST: f32 = 0.4;

struct ActiveStroke {
    stroke: Stroke,
    smoother: PointerSmoother,
}

struct Erasing {
    removed: Vec<Stroke>,
    last: [f64; 2],
}

#[derive(Default)]
struct Inertia {
    velocity: [f64; 2],
    zoom_velocity: f64,
    center: [f64; 2],
}

impl Inertia {
    fn active(&self) -> bool {
        self.velocity[0].hypot(self.velocity[1]) > 5.0 || self.zoom_velocity.abs() > 0.02
    }
}

pub struct CanvasApp {
    pub doc: Document,
    pub camera: Camera,
    history: History,
    pub tools: ToolState,
    gestures: Gestures,
    active: Option<ActiveStroke>,
    erasing: Option<Erasing>,
    inertia: Inertia,
    viewport: [f64; 2],
    /// Physical pixels per logical point.
    pub pixels_per_point: f32,
    pub egui_ctx: egui::Context,
    ui_rects: Vec<egui::Rect>,
    ui_pointers: HashSet<u64>,
    save_path: Option<PathBuf>,
    dirty: bool,
    last_change: Instant,
    last_frame: Instant,
    /// Whether another frame should be drawn soon (animation, pending meshes, input).
    pub wants_redraw: bool,
    /// Height (points) covered by the status bar / notch at the top.
    pub safe_top: f32,
    stats: FrameStats,
}

/// Frame-time statistics, logged every two seconds while frames are being drawn.
#[derive(Default)]
struct FrameStats {
    start: Option<Instant>,
    frames: u32,
    sum: f64,
    max: f64,
    visible: usize,
    phases: [f64; 5],
    triangles: usize,
}

impl FrameStats {
    fn record(&mut self, cpu: f64, visible: usize, query: f64, t: crate::render::Timings) {
        self.phases[0] += query;
        self.phases[1] += t.acquire;
        self.phases[2] += t.build;
        self.phases[3] += t.submit;
        self.phases[4] += t.present;
        self.triangles = self.triangles.max(t.triangles);
        let start = *self.start.get_or_insert_with(Instant::now);
        self.frames += 1;
        self.sum += cpu;
        self.max = self.max.max(cpu);
        self.visible = self.visible.max(visible);
        let span = start.elapsed().as_secs_f64();
        if span >= 2.0 {
            log::info!(
                "frames: {:.0}/s, cpu avg {:.1} ms, max {:.1} ms, visible strokes ≤ {}",
                self.frames as f64 / span,
                self.sum / self.frames as f64 * 1e3,
                self.max * 1e3,
                self.visible
            );
            let n = self.frames as f64 / 1e3;
            let p = self.phases.map(|x| x / n);
            log::info!(
                "  avg ms: query {:.2}, acquire {:.2}, build {:.2}, submit {:.2}, present {:.2}; triangles ≤ {}",
                p[0], p[1], p[2], p[3], p[4], self.triangles
            );
            *self = Self::default();
        }
    }
}

impl CanvasApp {
    pub fn new(save_path: Option<PathBuf>) -> Self {
        let (doc, camera) = match save_path.as_deref().map(io::load_file) {
            Some(Ok((d, c))) => {
                log::info!("loaded {} strokes", d.len());
                (d, c)
            }
            Some(Err(io::LoadError::Io(e))) if e.kind() == std::io::ErrorKind::NotFound => (Document::new(), Camera::default()),
            Some(Err(e)) => {
                log::error!("could not load the drawing: {e}");
                // Keep the broken file around instead of overwriting it.
                if let Some(p) = &save_path {
                    let _ = std::fs::rename(p, p.with_extension("broken"));
                }
                (Document::new(), Camera::default())
            }
            None => (Document::new(), Camera::default()),
        };
        Self {
            doc,
            camera,
            history: History::default(),
            tools: ToolState::default(),
            gestures: Gestures::default(),
            active: None,
            erasing: None,
            inertia: Inertia::default(),
            viewport: [1.0, 1.0],
            pixels_per_point: 1.0,
            egui_ctx: egui::Context::default(),
            ui_rects: Vec::new(),
            ui_pointers: HashSet::new(),
            save_path,
            dirty: false,
            last_change: Instant::now(),
            last_frame: Instant::now(),
            wants_redraw: true,
            safe_top: 0.0,
            stats: FrameStats::default(),
        }
        .with_touch_style()
    }

    /// Starts a fresh egui context. Needed when the GPU renderer is recreated (Android
    /// surface loss): the new renderer has no textures, and a fresh context re-uploads them.
    pub fn reset_ui(&mut self) {
        self.egui_ctx = egui::Context::default();
        self.apply_style();
    }

    fn with_touch_style(self) -> Self {
        self.apply_style();
        self
    }

    fn apply_style(&self) {
        self.egui_ctx.set_theme(egui::Theme::Light);
        if cfg!(target_os = "android") {
            self.egui_ctx.all_styles_mut(|st| {
                st.spacing.interact_size = egui::vec2(40.0, 34.0);
                st.spacing.item_spacing = egui::vec2(8.0, 6.0);
                st.spacing.button_padding = egui::vec2(8.0, 6.0);
                st.spacing.slider_width = 120.0;
            });
        }
    }

    pub fn resize(&mut self, size: [u32; 2]) {
        self.viewport = [size[0].max(1) as f64, size[1].max(1) as f64];
        self.wants_redraw = true;
    }

    fn over_ui(&self, pos: [f64; 2]) -> bool {
        let p = egui::pos2(pos[0] as f32 / self.pixels_per_point, pos[1] as f32 / self.pixels_per_point);
        self.ui_rects.iter().any(|r| r.contains(p))
    }

    /// Feeds a pointer event to the canvas. UI hit-testing happens here: a pointer that went
    /// down over the toolbar belongs to the UI until it is released.
    pub fn pointer(&mut self, e: PointerEvent) {
        use crate::gestures::Phase;
        self.wants_redraw = true;
        if e.phase == Phase::Down && self.over_ui(e.pos) {
            self.ui_pointers.insert(e.id);
            return;
        }
        if self.ui_pointers.contains(&e.id) {
            if matches!(e.phase, Phase::Up | Phase::Cancel) {
                self.ui_pointers.remove(&e.id);
            }
            return;
        }
        let mut out = Vec::new();
        self.gestures.handle(&e, &mut out);
        for g in out {
            self.apply(g, e.kind);
        }
    }

    /// Mouse wheel / touchpad zoom by `dlog` binary orders of magnitude around `pos`.
    pub fn wheel_zoom(&mut self, pos: [f64; 2], dlog: f64) {
        self.inertia = Inertia::default();
        self.camera.zoom_at(pos, self.viewport, dlog);
        self.wants_redraw = true;
    }

    pub fn pan(&mut self, delta: [f64; 2]) {
        self.camera.pan_px(delta);
        self.wants_redraw = true;
    }

    fn apply(&mut self, g: Gesture, kind: PointerKind) {
        match g {
            Gesture::StrokeBegin { pos, pressure, time, erase } => {
                if erase || self.tools.tool == Tool::Eraser {
                    self.erasing = Some(Erasing { removed: Vec::new(), last: pos });
                    self.erase_at(pos);
                } else {
                    self.begin_stroke(pos, pressure, time, kind);
                }
            }
            Gesture::StrokeMove { pos, pressure, time } => {
                if self.erasing.is_some() {
                    self.erase_along(pos);
                } else {
                    self.extend_stroke(pos, pressure, time, kind);
                }
            }
            Gesture::StrokeEnd => {
                if let Some(er) = self.erasing.take() {
                    if !er.removed.is_empty() {
                        self.history.record(Action::Remove(er.removed));
                        self.changed();
                    }
                } else if let Some(a) = self.active.take() {
                    let mut s = a.stroke;
                    s.refresh_cache();
                    self.history.apply(&mut self.doc, Action::Add(vec![s]));
                    self.changed();
                }
            }
            Gesture::StrokeCancel => {
                self.active = None;
                if let Some(er) = self.erasing.take() {
                    for s in er.removed {
                        self.doc.insert(s);
                    }
                }
            }
            Gesture::Pan { delta } => self.camera.pan_px(delta),
            Gesture::Zoom { center, dlog } => self.camera.zoom_at(center, self.viewport, dlog),
            Gesture::Fling { velocity, zoom_velocity, center } => {
                self.inertia = Inertia { velocity, zoom_velocity: zoom_velocity.clamp(-20.0, 20.0), center };
            }
            Gesture::Stop => self.inertia = Inertia::default(),
        }
    }

    fn pressure(kind: PointerKind, p: f32) -> f32 {
        match kind {
            // Mouse and fingers have no meaningful pressure: full width.
            PointerKind::Mouse | PointerKind::Touch => 1.0,
            _ => p.clamp(0.0, 1.0),
        }
    }

    fn begin_stroke(&mut self, pos: [f64; 2], pressure: f32, time: f64, kind: PointerKind) {
        let level = self.camera.level();
        let w = self.camera.screen_to_world(pos, self.viewport).mul_pow2(level);
        let anchor = [w.x.floor_at(0), w.y.floor_at(0)];
        let frame = self.camera.level_frame(level, self.viewport);
        let c = self.tools.color;
        let width_px = self.tools.width as f64 * self.pixels_per_point as f64;
        let id = self.doc.alloc_id();
        let stroke = Stroke::new(id, level, anchor, [c.r(), c.g(), c.b(), c.a()], (width_px / frame.scale) as f32);
        self.active = Some(ActiveStroke { stroke, smoother: PointerSmoother::default() });
        self.extend_stroke(pos, pressure, time, kind);
    }

    fn extend_stroke(&mut self, pos: [f64; 2], pressure: f32, time: f64, kind: PointerKind) {
        let Some(a) = self.active.as_mut() else { return };
        let (x, y, p) = a.smoother.push(pos[0], pos[1], Self::pressure(kind, pressure) as f64, time);
        // Recomputed every sample: the camera may move while drawing (wheel zoom on desktop).
        let frame = self.camera.level_frame(a.stroke.level, self.viewport);
        let l = frame.screen_to_local([x, y], &a.stroke.anchor, a.stroke.cache.anchor_fast);
        let pt = StrokePoint { x: l[0] as f32, y: l[1] as f32, pressure: p as f32 };
        if let Some(last) = a.stroke.points.last() {
            let d = ((last.x - pt.x).powi(2) + (last.y - pt.y).powi(2)).sqrt();
            if d < MIN_SAMPLE_DIST * (1.0 / frame.scale as f32).max(1.0) {
                return;
            }
        }
        a.stroke.push(pt);
    }

    fn erase_at(&mut self, pos: [f64; 2]) {
        let r = self.tools.eraser as f64 * self.pixels_per_point as f64;
        let hits = self.doc.hit_test(&self.camera, self.viewport, pos, r);
        if let Some(er) = self.erasing.as_mut() {
            for id in hits {
                if let Some(s) = self.doc.remove(id) {
                    er.removed.push(s);
                }
            }
        }
    }

    fn erase_along(&mut self, pos: [f64; 2]) {
        let Some(last) = self.erasing.as_ref().map(|e| e.last) else { return };
        let r = (self.tools.eraser as f64 * self.pixels_per_point as f64).max(1.0);
        let d = ((pos[0] - last[0]).powi(2) + (pos[1] - last[1]).powi(2)).sqrt();
        let steps = (d / (r * 0.5)).ceil().clamp(1.0, 200.0) as usize;
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            self.erase_at([last[0] + (pos[0] - last[0]) * t, last[1] + (pos[1] - last[1]) * t]);
        }
        if let Some(e) = self.erasing.as_mut() {
            e.last = pos;
        }
    }

    pub fn undo(&mut self) {
        if self.active.is_none() && self.erasing.is_none() && self.history.undo(&mut self.doc) {
            self.changed();
        }
    }

    pub fn redo(&mut self) {
        if self.active.is_none() && self.erasing.is_none() && self.history.redo(&mut self.doc) {
            self.changed();
        }
    }

    pub fn go_home(&mut self) {
        self.inertia = Inertia::default();
        self.camera = Camera::default();
        self.dirty = true;
        self.last_change = Instant::now();
    }

    fn changed(&mut self) {
        self.dirty = true;
        self.last_change = Instant::now();
        self.wants_redraw = true;
    }

    /// Saves now if there are unsaved changes (call on pause / exit).
    pub fn save_now(&mut self) {
        let Some(path) = self.save_path.clone() else { return };
        if !self.dirty {
            return;
        }
        match io::save_file(&path, &self.doc, &self.camera) {
            Ok(()) => {
                self.dirty = false;
                log::info!("saved {} strokes to {}", self.doc.len(), path.display());
            }
            Err(e) => log::error!("autosave failed: {e}"),
        }
    }

    fn autosave(&mut self) {
        if self.dirty && self.last_change.elapsed() >= AUTOSAVE_DELAY && self.active.is_none() {
            let Some(path) = self.save_path.clone() else { return };
            // Encoding is fast; the disk write happens off the UI thread.
            let bytes = io::encode(&self.doc, &self.camera);
            self.dirty = false;
            std::thread::spawn(move || {
                let tmp = path.with_extension("tmp");
                let res = (|| -> std::io::Result<()> {
                    if let Some(d) = path.parent() {
                        std::fs::create_dir_all(d)?;
                    }
                    use std::io::Write;
                    let mut f = std::fs::File::create(&tmp)?;
                    f.write_all(&bytes)?;
                    f.sync_all()?;
                    std::fs::rename(&tmp, &path)
                })();
                if let Err(e) = res {
                    log::error!("autosave failed: {e}");
                }
            });
        }
    }

    /// Whether the app needs to keep animating without new input.
    pub fn animating(&self) -> bool {
        self.wants_redraw || self.inertia.active()
    }

    /// Time until the next autosave check is due, if any.
    pub fn next_wakeup(&self) -> Option<Duration> {
        self.dirty.then(|| AUTOSAVE_DELAY.saturating_sub(self.last_change.elapsed()))
    }

    /// Runs one frame: inertia, UI, autosave, rendering. Returns egui's platform output.
    pub fn frame(&mut self, target: Target, renderer: &mut Renderer, raw_input: egui::RawInput) -> egui::PlatformOutput {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f64().min(0.05);
        self.last_frame = now;
        self.wants_redraw = false;

        // Inertia.
        if self.inertia.active() && !self.gestures.is_drawing() {
            let v = self.inertia.velocity;
            self.camera.pan_px([v[0] * dt, v[1] * dt]);
            self.camera.zoom_at(self.inertia.center, self.viewport, self.inertia.zoom_velocity * dt);
            let kp = (-PAN_DECAY * dt).exp();
            let kz = (-ZOOM_DECAY * dt).exp();
            self.inertia.velocity = [v[0] * kp, v[1] * kp];
            self.inertia.zoom_velocity *= kz;
            if !self.inertia.active() {
                self.inertia = Inertia::default();
                self.dirty = true;
                self.last_change = Instant::now();
            }
        }

        // UI.
        let info = ui::Info {
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
            log_zoom: self.camera.log_zoom,
            strokes: self.doc.len(),
        };
        let mut actions = ui::Actions::default();
        let mut rects = Vec::new();
        let ctx = self.egui_ctx.clone();
        let output = ctx.run_ui(raw_input, |ui| {
            let (a, r) = ui::toolbar(ui.ctx(), &mut self.tools, &info, self.safe_top);
            actions = a;
            rects = r;
        });
        self.ui_rects = rects;
        if actions.undo {
            self.undo();
        }
        if actions.redo {
            self.redo();
        }
        if actions.home {
            self.go_home();
        }
        self.autosave();

        // Scene.
        let full = [0.0, 0.0, self.viewport[0], self.viewport[1]];
        let tq = Instant::now();
        let visible = self.doc.query(&self.camera, self.viewport, full);
        let query_time = tq.elapsed().as_secs_f64();
        let active = self.active.as_ref().map(|a| {
            let f = self.camera.level_frame(a.stroke.level, self.viewport);
            (&a.stroke, f.anchor_to_screen(&a.stroke.anchor, a.stroke.cache.anchor_fast), f.scale)
        });
        let prims = ctx.tessellate(output.shapes, output.pixels_per_point);
        renderer.render_target(
            target,
            FrameInput {
                visible: &visible,
                doc: &self.doc,
                active,
                log_zoom: self.camera.log_zoom,
                background: BACKGROUND,
                egui: Some((&prims, output.pixels_per_point)),
                textures: output.textures_delta,
            },
        );
        self.stats.record(now.elapsed().as_secs_f64(), visible.len(), query_time, renderer.timings);
        let egui_repaint = output.viewport_output.values().any(|v| v.repaint_delay.is_zero());
        self.wants_redraw |= renderer.needs_another_frame || egui_repaint || self.inertia.active();
        output.platform_output
    }
}
