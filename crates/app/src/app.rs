//! Platform-independent application state: document, camera, tools, input → actions.

use crate::gestures::{Gesture, Gestures, PointerEvent, PointerKind};
use crate::render::{ActiveDraw, FrameInput, Renderer, Target};
use crate::ui::{self, Tool, ToolState};
use canvas_core::camera::Camera;
use canvas_core::document::Document;
use canvas_core::history::{Action, History};
use canvas_core::io;
use canvas_core::mesh::{Mesh, lod_for, tessellate_part};
use canvas_core::smoothing::PointerSmoother;
use canvas_core::stroke::{Stroke, StrokePoint};
use std::collections::HashSet;
use crate::projects::Projects;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const BACKGROUND: [f32; 4] = [0.98, 0.976, 0.965, 1.0];
const AUTOSAVE_DELAY: Duration = Duration::from_millis(1500);
/// Pan inertia decay rate (1/s) and zoom inertia decay rate.
const PAN_DECAY: f64 = 4.0;
const ZOOM_DECAY: f64 = 6.0;
/// Minimum distance between stored samples, in local units (≈ px at drawing time).
const MIN_SAMPLE_DIST: f32 = 0.4;
/// Rotation closer to upright than this (radians, ~6°) snaps to 0 when the fingers lift.
const SNAP_ANGLE: f64 = 0.1;
/// Preview width in px (multiple of 64 for the GPU readback).
const THUMB_W: u32 = 256;
/// Minimum time between automatic preview refreshes while drawing.
const THUMB_INTERVAL: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Gallery,
    Canvas,
}

struct ActiveStroke {
    stroke: Stroke,
    smoother: PointerSmoother,
    /// Tessellated parts of the stroke so far (see `FROZEN_PART`).
    frozen: Vec<Mesh>,
    /// Points before this index are covered by `frozen`.
    frozen_upto: usize,
}

/// While drawing, every this many points are tessellated once and kept, so each frame only
/// re-tessellates the short tail instead of the whole (possibly very long) stroke.
const FROZEN_PART: usize = 48;

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
    /// Screen position of the eraser while erasing (for the cursor circle).
    eraser_cursor: Option<[f64; 2]>,
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
    projects: Option<Projects>,
    /// Name of the open project (empty in single-file mode).
    pub project: String,
    gallery: ui::GalleryState,
    /// Gallery (start screen) or an open project.
    pub screen: Screen,
    /// "Projects" was pressed: leave the canvas on the next frame (needs the renderer).
    want_gallery: bool,
    /// When the last preview was rendered (previews are refreshed now and then while drawing).
    last_thumb: Instant,
    thumb_due: bool,
    save_thread: Option<std::thread::JoinHandle<()>>,
    /// Set when a different document was loaded: cached meshes are keyed by stroke id.
    clear_render_cache: bool,
}

/// Frame-time statistics, logged every two seconds while frames are being drawn.
#[derive(Default)]
struct FrameStats {
    start: Option<Instant>,
    frames: u32,
    sum: f64,
    max: f64,
    visible: usize,
    phases: [f64; 6],
    triangles: usize,
}

impl FrameStats {
    fn record(&mut self, cpu: f64, visible: usize, query: f64, t: crate::render::Timings) {
        self.phases[0] += query;
        self.phases[1] += t.acquire;
        self.phases[2] += t.build;
        self.phases[3] += t.submit;
        self.phases[4] += t.present;
        self.phases[5] += t.active;
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
                "  avg ms: query {:.2}, acquire {:.2}, build {:.2} (active stroke {:.2}), submit {:.2}, present {:.2}; triangles ≤ {}",
                p[0], p[1], p[2], p[5], p[3], p[4], self.triangles
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
            eraser_cursor: None,
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
            projects: None,
            project: String::new(),
            gallery: ui::GalleryState::default(),
            screen: Screen::Canvas,
            want_gallery: false,
            last_thumb: Instant::now(),
            thumb_due: false,
            save_thread: None,
            clear_render_cache: false,
        }
        .with_touch_style()
    }

    /// Opens the projects folder in `data_dir` and starts on the projects gallery.
    pub fn with_projects(data_dir: &Path) -> Self {
        let projects = match Projects::new(data_dir) {
            Ok(p) => p,
            Err(e) => {
                log::error!("cannot open the projects folder: {e}");
                return Self::new(Some(data_dir.join("canvas.icnv")));
            }
        };
        let mut app = Self::new(None);
        app.projects = Some(projects);
        app.screen = Screen::Gallery;
        app.refresh_gallery();
        app
    }

    /// Replaces the open document with the file at `path` (empty if it doesn't exist).
    fn load_document(&mut self, path: Option<PathBuf>) {
        self.save_now();
        let (doc, camera) = match path.as_deref().map(io::load_file) {
            None => (Document::new(), Camera::default()),
            Some(Ok(dc)) => dc,
            Some(Err(io::LoadError::Io(e))) if e.kind() == std::io::ErrorKind::NotFound => (Document::new(), Camera::default()),
            Some(Err(e)) => {
                let p = path.as_ref().expect("some");
                log::error!("could not load {}: {e}", p.display());
                let _ = std::fs::rename(p, p.with_extension("broken"));
                self.gallery.message = Some(format!("Plik był uszkodzony: {e}"));
                (Document::new(), Camera::default())
            }
        };
        self.doc = doc;
        self.camera = camera;
        self.history = History::default();
        self.active = None;
        self.eraser_cursor = None;
        self.inertia = Inertia::default();
        self.save_path = path;
        self.dirty = false;
        self.clear_render_cache = true;
        self.wants_redraw = true;
    }

    /// Opens a project from the gallery.
    fn open_project(&mut self, name: String) {
        let Some(projects) = self.projects.clone() else { return };
        self.load_document(Some(projects.path_for(&name)));
        projects.set_last(&name);
        log::info!("project {name:?}: {} strokes", self.doc.len());
        self.project = name;
        self.screen = Screen::Canvas;
        self.last_thumb = Instant::now();
    }

    /// Saves the open project, renders its preview and shows the gallery.
    fn leave_to_gallery(&mut self, target: Target, renderer: &mut Renderer) {
        self.render_thumbnail(target, renderer, false);
        self.load_document(None);
        self.gallery.thumbs.remove(&self.project);
        self.project.clear();
        self.screen = Screen::Gallery;
        self.gallery.message = None;
        self.refresh_gallery();
    }

    /// Renders what the camera sees into the project's preview PNG.
    fn render_thumbnail(&mut self, target: Target, renderer: &mut Renderer, background: bool) {
        let Some(projects) = self.projects.clone() else { return };
        if self.project.is_empty() {
            return;
        }
        self.last_thumb = Instant::now();
        let (w, h) = (THUMB_W, (THUMB_W as f32 * ui::THUMB_ASPECT) as u32);
        let mut cam = self.camera.clone();
        // Same horizontal extent as the screen, centred on the same point.
        cam.log_zoom += (w as f64 / self.viewport[0]).log2();
        let vp = [w as f64, h as f64];
        let visible = self.doc.query(&cam, vp, [0.0, 0.0, vp[0], vp[1]]);
        let (device, queue, format) = target.device_queue_format();
        let px = renderer.render_rgba(
            device,
            queue,
            format,
            [w, h],
            FrameInput {
                visible: &visible,
                doc: &self.doc,
                active: None,
                log_zoom: cam.log_zoom,
                rotation: cam.rot(),
                background: BACKGROUND,
                egui: None,
                textures: Default::default(),
            },
        );
        if px.len() != (w * h * 4) as usize {
            return;
        }
        let name = self.project.clone();
        let save = move || {
            if let Err(e) = projects.save_thumbnail(&name, w, h, &px) {
                log::warn!("preview of {name:?} not saved: {e}");
            }
        };
        if background {
            std::thread::spawn(save);
        } else {
            save();
        }
    }

    fn refresh_gallery(&mut self) {
        if let Some(p) = &self.projects {
            self.gallery.list = p.list();
        }
    }

    pub fn gallery_action(&mut self, a: ui::GalleryAction) {
        use ui::GalleryAction::*;
        let Some(projects) = self.projects.clone() else { return };
        match a {
            New => {
                let name = projects.new_name();
                if let Err(e) = io::save_file(&projects.path_for(&name), &Document::new(), &Camera::default()) {
                    self.gallery.message = Some(format!("Nie udało się utworzyć kartki: {e}"));
                } else {
                    self.open_project(name);
                }
            }
            Open(name) => self.open_project(name),
            Rename { from, to } => match projects.rename(&from, &to) {
                Ok(name) => {
                    self.gallery.thumbs.remove(&from);
                    self.gallery.options = None;
                    self.gallery.message = Some(format!("Zmieniono nazwę na „{name}”"));
                }
                Err(e) => self.gallery.message = Some(e),
            },
            Duplicate(name) => {
                self.gallery.options = None;
                self.gallery.message = Some(match projects.duplicate(&name) {
                    Ok(copy) => format!("Utworzono kopię „{copy}”"),
                    Err(e) => format!("Nie udało się skopiować: {e}"),
                });
            }
            Delete(name) => {
                self.gallery.options = None;
                self.gallery.thumbs.remove(&name);
                self.gallery.message = Some(match projects.delete(&name) {
                    Ok(()) => format!("Usunięto „{name}”"),
                    Err(e) => format!("Nie udało się usunąć: {e}"),
                });
            }
        }
        self.refresh_gallery();
    }

    /// System "back": from a project to the gallery, or closes a dialog.
    /// Returns whether it was handled (otherwise the app may close).
    pub fn back(&mut self) -> bool {
        self.wants_redraw = true;
        match self.screen {
            Screen::Canvas if self.projects.is_some() => {
                self.want_gallery = true;
                true
            }
            Screen::Gallery if self.gallery.options.is_some() => {
                self.gallery.options = None;
                true
            }
            _ => false,
        }
    }

    fn wait_for_save(&mut self) {
        if let Some(t) = self.save_thread.take() {
            let _ = t.join();
        }
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

    /// Rotates the view by `angle` radians around `pos` (desktop: Alt + wheel).
    pub fn wheel_rotate(&mut self, pos: [f64; 2], angle: f64) {
        self.inertia = Inertia::default();
        self.camera.rotate_at(pos, self.viewport, angle);
        if self.camera.rotation.abs() < 1e-9 {
            self.camera.rotation = 0.0;
        }
        self.changed();
    }

    /// Back to upright, rotating around the screen centre.
    pub fn reset_rotation(&mut self) {
        let c = [self.viewport[0] * 0.5, self.viewport[1] * 0.5];
        let r = self.camera.rotation;
        self.camera.rotate_at(c, self.viewport, -r);
        self.camera.rotation = 0.0;
        self.changed();
    }

    pub fn pan(&mut self, delta: [f64; 2]) {
        self.camera.pan_px(delta);
        self.wants_redraw = true;
    }

    fn apply(&mut self, g: Gesture, kind: PointerKind) {
        match g {
            Gesture::StrokeBegin { pos, pressure, time, erase } => {
                let eraser = erase || self.tools.tool == Tool::Eraser;
                self.eraser_cursor = eraser.then_some(pos);
                self.begin_stroke(pos, pressure, time, kind, eraser);
            }
            Gesture::StrokeMove { pos, pressure, time } => {
                if self.eraser_cursor.is_some() {
                    self.eraser_cursor = Some(pos);
                }
                self.extend_stroke(pos, pressure, time, kind);
            }
            Gesture::StrokeEnd => {
                self.eraser_cursor = None;
                if let Some(a) = self.active.take() {
                    let mut s = a.stroke;
                    s.refresh_cache();
                    self.history.apply(&mut self.doc, Action::Add(vec![s]));
                    self.changed();
                }
            }
            Gesture::StrokeCancel => {
                self.active = None;
                self.eraser_cursor = None;
            }
            Gesture::Pan { delta } => self.camera.pan_px(delta),
            Gesture::Zoom { center, dlog } => self.camera.zoom_at(center, self.viewport, dlog),
            Gesture::Rotate { center, angle } => self.camera.rotate_at(center, self.viewport, angle),
            Gesture::PinchEnd { center } => {
                // Snap to upright when released close to it.
                if self.camera.rotation != 0.0 && self.camera.rotation.abs() < SNAP_ANGLE {
                    let r = self.camera.rotation;
                    self.camera.rotate_at(center, self.viewport, -r);
                    self.camera.rotation = 0.0;
                }
                self.changed();
            }
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

    /// Starts an ink stroke, or an eraser stroke (it clears earlier ink under its path).
    fn begin_stroke(&mut self, pos: [f64; 2], pressure: f32, time: f64, kind: PointerKind, eraser: bool) {
        let level = self.camera.level();
        let w = self.camera.screen_to_world(pos, self.viewport).mul_pow2(level);
        let anchor = [w.x.floor_at(0), w.y.floor_at(0)];
        let frame = self.camera.level_frame(level, self.viewport);
        let c = self.tools.color;
        let (color, width_pt) =
            if eraser { (Stroke::ERASER_COLOR, self.tools.eraser) } else { ([c.r(), c.g(), c.b(), c.a()], self.tools.width) };
        let width_px = width_pt as f64 * self.pixels_per_point as f64;
        let id = self.doc.alloc_id();
        let stroke = Stroke::new(id, level, anchor, color, (width_px / frame.scale) as f32);
        self.active = Some(ActiveStroke { stroke, smoother: PointerSmoother::default(), frozen: Vec::new(), frozen_upto: 0 });
        self.extend_stroke(pos, pressure, time, kind);
    }

    fn extend_stroke(&mut self, pos: [f64; 2], pressure: f32, time: f64, kind: PointerKind) {
        let Some(a) = self.active.as_mut() else { return };
        // The eraser always has its full size.
        let pressure = if a.stroke.is_eraser() { 1.0 } else { Self::pressure(kind, pressure) };
        let (x, y, p) = a.smoother.push(pos[0], pos[1], pressure as f64, time);
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
        let n = a.stroke.points.len();
        if n - a.frozen_upto > FROZEN_PART {
            // Parts share their boundary point; round caps make the joint seamless.
            let end = a.frozen_upto + FROZEN_PART;
            let lod = lod_for(a.stroke.level, self.camera.log_zoom).min(2);
            a.frozen.push(tessellate_part(&a.stroke, a.frozen_upto..end + 1, lod));
            a.frozen_upto = end;
        }
    }

    pub fn undo(&mut self) {
        if self.active.is_none() && self.history.undo(&mut self.doc) {
            self.changed();
        }
    }

    pub fn redo(&mut self) {
        if self.active.is_none() && self.history.redo(&mut self.doc) {
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
        self.wait_for_save();
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
            self.thumb_due = true;
            self.wait_for_save();
            self.save_thread = Some(std::thread::spawn(move || {
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
            }));
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

        if std::mem::take(&mut self.want_gallery) && self.screen == Screen::Canvas {
            self.leave_to_gallery(target, renderer);
        }

        // UI.
        let info = ui::Info {
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
            log_zoom: self.camera.log_zoom,
            rotation: self.camera.rotation,
            strokes: self.doc.len(),
        };
        let mut actions = ui::Actions::default();
        let mut gallery_actions = Vec::new();
        let mut rects = Vec::new();
        let ctx = self.egui_ctx.clone();
        let output = ctx.run_ui(raw_input, |ui| {
            if self.screen == Screen::Gallery {
                if let Some(projects) = &self.projects {
                    let bg = egui::Color32::from_rgb(
                        (BACKGROUND[0] * 255.0) as u8,
                        (BACKGROUND[1] * 255.0) as u8,
                        (BACKGROUND[2] * 255.0) as u8,
                    );
                    gallery_actions = ui::gallery(ui, &mut self.gallery, projects, self.safe_top, bg);
                }
                // The canvas takes no input on the gallery screen.
                rects.push(egui::Rect::EVERYTHING);
                return;
            }
            let (a, r) = ui::toolbar(ui.ctx(), &mut self.tools, &info, self.safe_top);
            actions = a;
            rects = r;
            if let Some(pos) = self.eraser_cursor {
                let ppp = self.pixels_per_point;
                let c = egui::pos2(pos[0] as f32 / ppp, pos[1] as f32 / ppp);
                let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("eraser")));
                let r = self.tools.eraser * 0.5;
                p.circle_stroke(c, r, egui::Stroke::new(1.5, egui::Color32::from_gray(255)));
                p.circle_stroke(c, r, egui::Stroke::new(0.8, egui::Color32::from_gray(60)));
            }
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
        if actions.upright {
            self.reset_rotation();
        }
        if actions.menu && self.projects.is_some() {
            self.want_gallery = true;
            self.wants_redraw = true;
        }
        for a in gallery_actions {
            self.gallery_action(a);
        }
        if std::mem::take(&mut self.clear_render_cache) {
            renderer.clear_cache();
        }
        self.autosave();
        if self.thumb_due && self.screen == Screen::Canvas && self.last_thumb.elapsed() >= THUMB_INTERVAL {
            self.thumb_due = false;
            self.render_thumbnail(target, renderer, true);
        }

        // Scene.
        let full = [0.0, 0.0, self.viewport[0], self.viewport[1]];
        let tq = Instant::now();
        let visible = self.doc.query(&self.camera, self.viewport, full);
        let query_time = tq.elapsed().as_secs_f64();
        let active = self.active.as_ref().map(|a| {
            let f = self.camera.level_frame(a.stroke.level, self.viewport);
            ActiveDraw {
                stroke: &a.stroke,
                offset: f.anchor_to_screen(&a.stroke.anchor, a.stroke.cache.anchor_fast),
                scale: f.scale,
                frozen: &a.frozen,
                tail_from: a.frozen_upto,
            }
        });
        let prims = ctx.tessellate(output.shapes, output.pixels_per_point);
        renderer.render_target(
            target,
            FrameInput {
                visible: &visible,
                doc: &self.doc,
                active,
                log_zoom: self.camera.log_zoom,
                rotation: self.camera.rot(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gestures::{MouseButton, Phase};
    use crate::ui::GalleryAction;

    fn draw(app: &mut CanvasApp, y: f64) {
        let ev = |phase, x: f64, t: f64| PointerEvent {
            id: 0,
            kind: PointerKind::Mouse,
            phase,
            pos: [x, y],
            pressure: 1.0,
            time: t,
            button: MouseButton::Primary,
        };
        app.pointer(ev(Phase::Down, 100.0, 0.0));
        for i in 1..20 {
            app.pointer(ev(Phase::Move, 100.0 + i as f64 * 5.0, i as f64 / 100.0));
        }
        app.pointer(ev(Phase::Up, 200.0, 0.3));
    }

    #[test]
    fn project_workflow() {
        let dir = std::env::temp_dir().join(format!("icnv-app-projects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = CanvasApp::with_projects(&dir);
        app.resize([800, 600]);
        // Starts on the gallery with nothing open.
        assert_eq!(app.screen, Screen::Gallery);
        assert!(app.gallery.list.is_empty());

        app.gallery_action(GalleryAction::New);
        assert_eq!((app.screen, app.project.as_str()), (Screen::Canvas, "Rysunek 1"));
        draw(&mut app, 300.0);
        assert_eq!(app.doc.len(), 1);
        // Leaving without a renderer (the preview needs the GPU; covered by the offscreen test).
        app.load_document(None);
        app.project.clear();
        app.screen = Screen::Gallery;
        app.refresh_gallery();

        app.gallery_action(GalleryAction::New);
        assert_eq!(app.project, "Rysunek 2");
        assert!(app.doc.is_empty() && !app.history.can_undo());
        draw(&mut app, 100.0);
        draw(&mut app, 200.0);
        app.load_document(None);
        app.screen = Screen::Gallery;
        app.refresh_gallery();
        assert_eq!(app.gallery.list.len(), 2);

        app.gallery_action(GalleryAction::Rename { from: "Rysunek 1".into(), to: "Szkice".into() });
        app.gallery_action(GalleryAction::Duplicate("Szkice".into()));
        let names: Vec<String> = app.gallery.list.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(names.contains(&"Szkice (kopia)".to_string()));
        app.gallery_action(GalleryAction::Delete("Szkice".into()));
        assert_eq!(app.gallery.list.len(), 2);

        app.gallery_action(GalleryAction::Open("Szkice (kopia)".into()));
        assert_eq!(app.doc.len(), 1);
        app.gallery_action(GalleryAction::Open("Rysunek 2".into()));
        assert_eq!(app.doc.len(), 2);
        assert!(app.back() && app.want_gallery);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
