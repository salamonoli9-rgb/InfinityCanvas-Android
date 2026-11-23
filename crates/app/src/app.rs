//! Platform-independent application state: document, camera, tools, input → actions.

use crate::gestures::{Gesture, Gestures, PointerEvent, PointerKind};
use crate::render::{ActiveDraw, FrameInput, ImageDraw, Renderer, Target};
use crate::ui::{self, Tool, ToolState};
use canvas_core::camera::Camera;
use canvas_core::document::Document;
use canvas_core::history::{Action, History};
use canvas_core::io;
use canvas_core::mesh::{Mesh, lod_for, tessellate_part};
use canvas_core::smoothing::PointerSmoother;
use canvas_core::stroke::{Brush, Stroke, StrokePoint};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::photos::{PhotoBackend, PhotoLibrary};
use crate::projects::Projects;
use canvas_core::image::{ImageId, RefImage};
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
    /// Smoothed pointer speed (points per second), for the dynamic pen.
    speed: f64,
    last_sample: Option<([f64; 2], f64)>,
    /// Shape guide being dragged, with its start point (screen px).
    shape: Option<(crate::shapes::Shape, [f64; 2])>,
    /// Raw pointer position / time and smoothed velocity (screen px/s), for drawing the tip a
    /// little ahead of the input (hides display and smoothing latency).
    raw: Option<([f64; 2], f64)>,
    raw_vel: [f64; 2],
}

/// How far ahead (s) the tip of the stroke being drawn is extrapolated on screen.
const PREDICT_AHEAD: f64 = 0.014;
/// …but never more than this (px per logical point of screen density).
const PREDICT_MAX: f64 = 24.0;

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
    /// Decoded reference photos (from the compressed data in the document).
    decoded: HashMap<ImageId, Arc<image::RgbaImage>>,
    /// Photo being moved / scaled with the fingers instead of drawing.
    pub image_edit: Option<ImageEdit>,
    photo_panel: ui::PhotoPanel,
    photo_lib: Option<PhotoLibrary>,
    /// Bytes of a photo waiting for "trace over it" / "turn into a drawing".
    pending_photo: Option<Vec<u8>>,
    pending_preview: Option<image::RgbaImage>,
    /// Screen point of a bucket fill to do on the next frame (it needs the renderer).
    pending_bucket: Option<[f64; 2]>,
    /// Export the view as a picture on the next frame.
    pending_export: bool,
    bookmark_panel: ui::BookmarkPanel,
    layer_panel: ui::LayerPanel,
    /// Id of the layer new strokes go to.
    pub active_layer: u32,
    /// Strokes added since the last frame (rendered without delay).
    urgent: Vec<canvas_core::stroke::StrokeId>,
    video_factory: Option<crate::video::VideoFactory>,
    /// Video export in progress.
    video: Option<VideoExport>,
    /// Screen recording in progress, and a finished one being written.
    recording: Option<Recording>,
    recording_saving: Option<crate::video::VideoJob>,
    text_panel: ui::TextPanel,
    /// Selected strokes (Select tool) and what the finger is doing with them.
    selection: Vec<canvas_core::stroke::StrokeId>,
    sel_drag: Option<SelDrag>,
    /// Bumped on every change of what is drawn (strokes, photos, layers).
    content_rev: u64,
    /// What the renderer's scene cache holds.
    cache: Option<CacheInfo>,
    /// Camera of the previous frame and when it last moved.
    last_cam: Option<Camera>,
    last_move: Instant,
    /// Camera flight to a bookmark in progress, with its start time.
    flight: Option<(canvas_core::flight::Flight, Instant)>,
    /// Short message at the bottom of the screen, with the time it appeared.
    toast: Option<(String, Instant)>,
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

enum SelDrag {
    Lasso(Vec<[f64; 2]>),
    Move { start: [f64; 2], cur: [f64; 2] },
    Scale { center: [f64; 2], start: [f64; 2], cur: [f64; 2] },
}

impl SelDrag {
    /// The move / scale in progress, if any.
    fn xform(&self) -> Option<crate::selection::Xform> {
        use crate::selection::Xform;
        match *self {
            SelDrag::Lasso(_) => None,
            SelDrag::Move { start, cur } => Some(Xform::Move([cur[0] - start[0], cur[1] - start[1]])),
            SelDrag::Scale { center, start, cur } => {
                let d0 = (start[0] - center[0]).hypot(start[1] - center[1]).max(1.0);
                let d1 = (cur[0] - center[0]).hypot(cur[1] - center[1]);
                Some(Xform::Scale { center, k: (d1 / d0).max(0.02) })
            }
        }
    }
}

/// Size of the selection's scale handle (logical points).
const SELECTION_HANDLE: f64 = 28.0;

/// Recording the canvas: frames at a fixed rate, the last one repeated while nothing changes.
struct Recording {
    job: crate::video::VideoJob,
    start: Instant,
    /// Frames written so far.
    written: u64,
    size: [u32; 2],
    /// Video px per screen px.
    k: f64,
    last: Vec<u8>,
}

struct VideoExport {
    cams: Vec<Camera>,
    next: usize,
    size: [u32; 2],
    job: crate::video::VideoJob,
    finishing: bool,
}

/// The scene cache: a full render (with a margin around the screen) at `cam`.
struct CacheInfo {
    cam: Camera,
    /// Viewport of the cache render (screen plus margins), px.
    vp: [f64; 2],
    rev: u64,
    /// All strokes had their meshes (nothing was still being tessellated).
    complete: bool,
    made: Instant,
}

/// Extra area rendered around the screen for the scene cache (fraction of each side).
const CACHE_MARGIN: f64 = 0.15;
/// The view counts as moving for this long after the camera last changed.
const MOVE_SETTLE: Duration = Duration::from_millis(150);
/// While moving, the cache is re-rendered at most this often (and only when needed).
const CACHE_REFRESH: Duration = Duration::from_millis(250);

/// Size of a brush sample in the brush menu, px.
pub const SAMPLE_SIZE: [u32; 2] = [192, 48];

/// RGBA pixels of a sample stroke of `brush`: a wave with pressure swelling in the middle
/// (a blob for the fill brush), drawn by the real renderer.
pub fn brush_sample(target: Target, renderer: &mut Renderer, brush: Brush, color: [u8; 4]) -> Vec<u8> {
    let [w, h] = SAMPLE_SIZE;
    let (device, queue, format) = target.device_queue_format();
    let vp = [w as f64, h as f64];
    let cam = Camera::default();
    let mut doc = Document::new();
    // Local units = screen px here (level 0, zoom 1); the origin is the screen centre.
    let width = (brush.default_width() * 1.1).clamp(1.5, 18.0);
    // Ids far from real strokes', one per brush: the renderer caches meshes by stroke id.
    let id = u64::MAX - 64 - Brush::ALL.iter().position(|b| *b == brush).unwrap_or(0) as u64;
    let mut s = Stroke::new(id, 0, [num_bigint::BigInt::from(0), num_bigint::BigInt::from(0)], color, width);
    s.brush = brush;
    let n = 64;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let (x, y) = if brush == Brush::Fill {
            let a = t * std::f32::consts::TAU;
            (60.0 * a.cos(), 15.0 * a.sin())
        } else {
            (-82.0 + 164.0 * t, 9.0 * (t * std::f32::consts::TAU * 1.2).sin())
        };
        let pressure = 0.15 + 0.85 * (t * std::f32::consts::PI).sin();
        s.push(StrokePoint { x, y, pressure });
    }
    s.refresh_cache();
    doc.insert(s);
    let visible = doc.query(&cam, vp, [0.0, 0.0, vp[0], vp[1]]);
    renderer.render_rgba(
        device,
        queue,
        format,
        [w, h],
        FrameInput {
            visible: &visible,
            doc: &doc,
            active: None,
            scene: None,
            images: &[],
            grid: &[],
            layer_opacity: &[1.0],
            urgent: &[],
            log_zoom: 0.0,
            rotation: [1.0, 0.0],
            background: BACKGROUND,
            egui: None,
            textures: Default::default(),
        },
    )
}

/// How to turn a photo into a drawing.
#[derive(Clone, Copy, Debug)]
pub struct DrawingOptions {
    /// 0..1
    pub detail: f32,
    /// Colour patches as filled shapes (an illustration) instead of outlines only.
    pub colored: bool,
    /// Outlines on top of the colours (always on for the outline-only style).
    pub outlines: bool,
    /// Keep the photo faintly underneath.
    pub keep_photo: bool,
}

/// Moving a reference photo: gestures transform it; each gesture is one undo step.
pub struct ImageEdit {
    pub id: ImageId,
    /// The photo before the current gesture.
    before: Option<RefImage>,
    last: Option<[f64; 2]>,
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
            decoded: HashMap::new(),
            image_edit: None,
            photo_panel: ui::PhotoPanel::default(),
            photo_lib: None,
            pending_photo: None,
            pending_preview: None,
            pending_bucket: None,
            pending_export: false,
            bookmark_panel: ui::BookmarkPanel::default(),
            layer_panel: ui::LayerPanel::default(),
            active_layer: 0,
            urgent: Vec::new(),
            content_rev: 0,
            text_panel: ui::TextPanel::default(),
            selection: Vec::new(),
            sel_drag: None,
            video_factory: None,
            video: None,
            recording: None,
            recording_saving: None,
            cache: None,
            last_cam: None,
            last_move: Instant::now(),
            flight: None,
            toast: None,
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
        self.decoded.clear();
        self.image_edit = None;
        self.flight = None;
        self.cache = None;
        self.content_rev += 1;
        self.active_layer = self.doc.layers.last().map_or(0, |l| l.id);
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
        if self.recording.is_some() {
            self.toggle_recording();
        }
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
        self.decode_images();
        let images = self.image_draws(&cam, vp);
        let layer_opacity: Vec<f32> = self.doc.layers.iter().map(|l| l.opacity).collect();
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
                scene: None,
                images: &images,
                grid: &[],
                layer_opacity: &layer_opacity,
                urgent: &[],
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
        if self.video.take().is_some() {
            // Dropping the job closes the encoder without finishing the file properly.
            self.toast = Some(("Eksport wideo anulowany".into(), Instant::now()));
            return true;
        }
        if self.image_edit.is_some() {
            self.finish_image_edit();
            return true;
        }
        if self.text_panel.open {
            self.text_panel.open = false;
            return true;
        }
        if self.bookmark_panel.open || self.layer_panel.open {
            self.bookmark_panel.open = false;
            self.layer_panel.open = false;
            return true;
        }
        if self.photo_panel.open {
            if self.photo_panel.picking {
                self.photo_panel.picking = false;
            } else {
                self.photo_panel.open = false;
            }
            return true;
        }
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
        // Textures belonged to the old context.
        self.tools.previews.clear();
        self.tools.previews_color = None;
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
        self.gestures.pan_only = self.tools.tool == Tool::Hand && self.image_edit.is_none();
        self.gestures.handle(&e, &mut out);
        for g in out {
            self.apply(g, e.kind);
        }
    }

    /// Where photos can be picked from (set by the platform launcher).
    pub fn set_photo_backend(&mut self, backend: Box<dyn PhotoBackend>) {
        self.photo_lib = Some(PhotoLibrary::new(backend));
    }

    /// Adds a photo (file bytes) centred on the screen and starts moving it.
    pub fn add_photo(&mut self, bytes: &[u8]) -> Result<ImageId, String> {
        let p = crate::photos::prepare(bytes).ok_or("Nie udało się odczytać zdjęcia")?;
        let pixels = [p.rgba.width(), p.rgba.height()];
        let img = RefImage::place(self.doc.alloc_id(), &self.camera, self.viewport, pixels, Arc::new(p.encoded), 0.8);
        let id = img.id;
        self.decoded.insert(id, Arc::new(p.rgba));
        self.history.apply(&mut self.doc, Action::Image { before: None, after: Some(Box::new(img)) });
        self.image_edit = Some(ImageEdit { id, before: None, last: None });
        self.changed();
        Ok(id)
    }

    /// A photo was picked or dropped: ask how to add it.
    pub fn offer_photo(&mut self, bytes: Vec<u8>) {
        self.pending_preview = crate::photos::thumbnail(&bytes, 256);
        self.photo_panel.forget_pending_preview();
        self.pending_photo = Some(bytes);
        self.photo_panel.open = true;
        self.photo_panel.picking = false;
        self.photo_panel.choosing = true;
        self.photo_panel.message = None;
        self.wants_redraw = true;
    }

    /// Turns a photo into strokes, centred on the screen like a placed photo:
    /// colour patches as filled shapes (if `opts.colored`) and/or outlines drawn with the
    /// current brush, colour and width. One undo step (plus one for the photo, if kept).
    pub fn photo_to_drawing(&mut self, bytes: &[u8], opts: DrawingOptions) -> Result<usize, String> {
        let img = crate::photos::decode(bytes).ok_or("Nie udało się odczytać zdjęcia")?;
        let detail = opts.detail.clamp(0.0, 1.0);
        let vp = self.viewport;
        let level = self.camera.level();
        let c = self.camera.screen_to_world([vp[0] * 0.5, vp[1] * 0.5], vp).mul_pow2(level);
        let anchor = [c.x.floor_at(0), c.y.floor_at(0)];
        let frame = self.camera.level_frame(level, vp);
        // Placement of an image of w×h px: fitted into 80% of the screen, centred, upright.
        // Returns (screen px per image px, top-left on screen).
        let place = |w: usize, h: usize| {
            let k = (vp[0] * 0.8 / w as f64).min(vp[1] * 0.8 / h as f64);
            (k, [vp[0] * 0.5 - w as f64 * k * 0.5, vp[1] * 0.5 - h as f64 * k * 0.5])
        };
        let to_local = |k: f64, origin: [f64; 2], p: [f32; 2]| {
            let l = frame.screen_to_local([origin[0] + p[0] as f64 * k, origin[1] + p[1] as f64 * k], &anchor, None);
            StrokePoint { x: l[0] as f32, y: l[1] as f32, pressure: 1.0 }
        };
        let mut strokes = Vec::new();

        if opts.colored {
            let side = (160.0 + 640.0 * detail) as u32;
            let rgb = img.thumbnail(side, side).to_rgb8();
            let (w, h) = (rgb.width() as usize, rgb.height() as usize);
            let patches = canvas_core::posterize::posterize(rgb.as_raw(), w, h, canvas_core::posterize::PosterizeOptions { detail });
            let (k, origin) = place(w, h);
            // The fill's own outline (≈1.2 source px wide) closes hairline gaps between patches.
            let width = (1.2 * k / frame.scale) as f32;
            for p in &patches {
                let mut s = Stroke::new(self.doc.alloc_id(), level, anchor.clone(), [p.color[0], p.color[1], p.color[2], 255], width);
                s.layer = self.active_layer;
                s.brush = Brush::Fill;
                s.points = p.polygon.iter().map(|q| to_local(k, origin, *q)).collect();
                s.refresh_cache();
                strokes.push(s);
            }
        }

        if opts.outlines || !opts.colored {
            let side = (500.0 + 1100.0 * detail) as u32;
            let gray = img.thumbnail(side, side).to_luma8();
            let (w, h) = (gray.width() as usize, gray.height() as usize);
            // Over colours, only the main outlines: less detail than a pure line drawing.
            let line_detail = if opts.colored { detail * 0.6 } else { detail };
            let lines = canvas_core::trace::trace(gray.as_raw(), w, h, canvas_core::trace::TraceOptions { detail: line_detail });
            let (k, origin) = place(w, h);
            let brush = match self.tools.brush {
                Brush::Fill | Brush::Dotted => Brush::Pen,
                b => b,
            };
            let col = self.tools.color;
            let width = (self.tools.width as f64 * self.pixels_per_point as f64 / frame.scale) as f32;
            for line in &lines {
                let mut s = Stroke::new(self.doc.alloc_id(), level, anchor.clone(), [col.r(), col.g(), col.b(), col.a()], width);
                s.layer = self.active_layer;
                s.brush = brush;
                s.points = line.iter().map(|q| to_local(k, origin, *q)).collect();
                s.refresh_cache();
                strokes.push(s);
            }
        }
        if strokes.is_empty() {
            return Err("Nic nie wyszło – spróbuj więcej szczegółów".into());
        }
        if opts.keep_photo {
            let id = self.add_photo(bytes)?;
            if let Some(img) = self.doc.image_mut(id) {
                img.opacity = 0.25;
            }
            self.image_edit = None;
        }
        let n = strokes.len();
        self.history.apply(&mut self.doc, Action::Add(strokes));
        self.changed();
        Ok(n)
    }

    fn bookmark_action(&mut self, a: ui::BookmarkAction) {
        use canvas_core::flight::{Bookmark, Flight};
        match a {
            ui::BookmarkAction::Add => {
                let n = self.doc.bookmarks.len() + 1;
                self.doc.bookmarks.push(Bookmark { name: format!("Zakładka {n}"), camera: self.camera.clone() });
                self.toast = Some((format!("Dodano „Zakładka {n}”"), Instant::now()));
            }
            ui::BookmarkAction::ExportVideo => {
                self.start_video();
                return;
            }
            ui::BookmarkAction::Go(i) => {
                if let Some(b) = self.doc.bookmarks.get(i) {
                    self.inertia = Inertia::default();
                    self.flight = Some((Flight::new(&self.camera, &b.camera, self.viewport), Instant::now()));
                    self.bookmark_panel.open = false;
                }
            }
            ui::BookmarkAction::Rename(i, name) => {
                let name = name.trim();
                if let Some(b) = self.doc.bookmarks.get_mut(i)
                    && !name.is_empty()
                {
                    b.name = name.chars().take(60).collect();
                }
            }
            ui::BookmarkAction::Delete(i) => {
                if i < self.doc.bookmarks.len() {
                    self.doc.bookmarks.remove(i);
                }
            }
        }
        self.changed();
    }

    pub fn layer_action(&mut self, a: ui::LayerAction) {
        use ui::LayerAction::*;
        match a {
            Add => {
                let n = self.doc.layers.len() + 1;
                self.active_layer = self.doc.add_layer(format!("Warstwa {n}"));
            }
            Select(id) => self.active_layer = id,
            ToggleVisible(id) => {
                if let Some(l) = self.doc.layers.iter_mut().find(|l| l.id == id) {
                    l.visible = !l.visible;
                }
            }
            Opacity(id, o) => {
                if let Some(l) = self.doc.layers.iter_mut().find(|l| l.id == id) {
                    l.opacity = o.clamp(0.0, 1.0);
                }
            }
            Up(id) | Down(id) => {
                let i = self.doc.layer_pos(id);
                let j = if matches!(a, Up(_)) { i + 1 } else { i.wrapping_sub(1) };
                if j < self.doc.layers.len() {
                    self.doc.layers.swap(i, j);
                }
            }
            Rename(id, name) => {
                let name = name.trim();
                if let Some(l) = self.doc.layers.iter_mut().find(|l| l.id == id)
                    && !name.is_empty()
                {
                    l.name = name.chars().take(40).collect();
                }
            }
            Delete(id) => {
                if self.doc.layers.len() <= 1 {
                    self.toast = Some(("Musi zostać co najmniej jedna warstwa".into(), Instant::now()));
                    return;
                }
                // Its strokes go through the undo history; the layer itself is just removed.
                let strokes: Vec<Stroke> = self.doc.strokes().filter(|s| s.layer == id).cloned().collect();
                if !strokes.is_empty() {
                    self.history.apply(&mut self.doc, Action::Remove(strokes));
                }
                self.doc.layers.retain(|l| l.id != id);
                if self.active_layer == id {
                    self.active_layer = self.doc.layers.last().map_or(0, |l| l.id);
                }
            }
        }
        self.changed();
    }

    /// One-finger gestures of the Select tool: lasso, or move / scale the selection.
    fn apply_select(&mut self, g: &Gesture) -> bool {
        use crate::selection::{Grab, bbox, grab, lasso};
        let ppp = self.pixels_per_point as f64;
        match *g {
            Gesture::StrokeBegin { pos, .. } => {
                let b = bbox(&self.doc, &self.camera, self.viewport, &self.selection, None);
                self.sel_drag = Some(match b.and_then(|b| grab(b, pos, SELECTION_HANDLE * ppp)) {
                    Some(Grab::Move) => SelDrag::Move { start: pos, cur: pos },
                    Some(Grab::Scale { center }) => SelDrag::Scale { center, start: pos, cur: pos },
                    None => {
                        self.selection.clear();
                        SelDrag::Lasso(vec![pos])
                    }
                });
            }
            Gesture::StrokeMove { pos, .. } => match self.sel_drag.as_mut() {
                Some(SelDrag::Lasso(pts)) => {
                    if pts.last().is_none_or(|l| (l[0] - pos[0]).hypot(l[1] - pos[1]) > 2.0) {
                        pts.push(pos);
                    }
                }
                Some(SelDrag::Move { cur, .. } | SelDrag::Scale { cur, .. }) => *cur = pos,
                None => {}
            },
            Gesture::StrokeEnd => match self.sel_drag.take() {
                Some(SelDrag::Lasso(pts)) => {
                    self.selection = lasso(&self.doc, &self.camera, self.viewport, &pts);
                    if self.selection.is_empty() && pts.len() > 3 {
                        self.toast = Some(("Nic nie zaznaczono – obrysuj kreski".into(), Instant::now()));
                    }
                }
                Some(d) => {
                    if let Some(x) = d.xform()
                        && x != crate::selection::Xform::Move([0.0, 0.0])
                    {
                        self.transform_selection(x, None);
                    }
                }
                None => {}
            },
            Gesture::StrokeCancel => self.sel_drag = None,
            _ => return false,
        }
        self.wants_redraw = true;
        true
    }

    /// Replaces the selected strokes by transformed / recoloured copies (one undo step).
    fn transform_selection(&mut self, x: crate::selection::Xform, color: Option<[u8; 4]>) {
        let mut removed = Vec::new();
        let mut added = Vec::new();
        for id in std::mem::take(&mut self.selection) {
            let Some(s) = self.doc.get(id).cloned() else { continue };
            let new_id = self.doc.alloc_id();
            let mut t = crate::selection::transformed(&s, &self.camera, self.viewport, x, new_id);
            if let Some(c) = color {
                t.color = c;
            }
            removed.push(s);
            added.push(t);
        }
        self.selection = added.iter().map(|s| s.id).collect();
        self.urgent.extend(self.selection.iter().copied());
        if !added.is_empty() {
            self.history.apply(&mut self.doc, Action::Replace { removed, added });
            self.changed();
        }
    }

    fn selection_action(&mut self, a: ui::SelectionAction) {
        match a {
            ui::SelectionAction::Delete => {
                let strokes: Vec<Stroke> = self.selection.iter().filter_map(|id| self.doc.get(*id).cloned()).collect();
                self.selection.clear();
                if !strokes.is_empty() {
                    self.history.apply(&mut self.doc, Action::Remove(strokes));
                    self.changed();
                }
            }
            ui::SelectionAction::Duplicate => {
                let d = 24.0 * self.pixels_per_point as f64;
                let mut copies = Vec::new();
                for id in self.selection.clone() {
                    let Some(s) = self.doc.get(id).cloned() else { continue };
                    let new_id = self.doc.alloc_id();
                    copies.push(crate::selection::transformed(&s, &self.camera, self.viewport, crate::selection::Xform::Move([d, d]), new_id));
                }
                self.selection = copies.iter().map(|s| s.id).collect();
                self.urgent.extend(self.selection.iter().copied());
                if !copies.is_empty() {
                    self.history.apply(&mut self.doc, Action::Add(copies));
                    self.changed();
                }
            }
            ui::SelectionAction::Recolor => {
                let c = self.tools.color;
                self.transform_selection(crate::selection::Xform::Move([0.0, 0.0]), Some([c.r(), c.g(), c.b(), c.a()]));
            }
            ui::SelectionAction::Done => self.selection.clear(),
        }
        self.wants_redraw = true;
    }

    /// Runs a selection action (as from the selection bar).
    pub fn select_action(&mut self, a: ui::SelectionAction) {
        self.selection_action(a);
    }

    /// Selected stroke ids (Select tool).
    pub fn selection(&self) -> &[canvas_core::stroke::StrokeId] {
        &self.selection
    }

    /// The grid setting changed (it is part of the cached scene).
    pub fn toggle_grid_changed(&mut self) {
        self.content_rev += 1;
        self.wants_redraw = true;
    }

    /// Places `text` with its top-left corner at screen point `pos`, `size` px tall, as one
    /// filled vector shape (letters with their holes) in the current colour.
    pub fn add_text(&mut self, text: &str, pos: [f64; 2], size: f64) -> bool {
        let loops = crate::text::outline(text, size, pos);
        if loops.is_empty() {
            return false;
        }
        let level = self.camera.level();
        let w = self.camera.screen_to_world(pos, self.viewport).mul_pow2(level);
        let anchor = [w.x.floor_at(0), w.y.floor_at(0)];
        let frame = self.camera.level_frame(level, self.viewport);
        let c = self.tools.color;
        // A thin outline of the letters' own colour keeps their edges crisp at any zoom.
        let mut s = Stroke::new(self.doc.alloc_id(), level, anchor.clone(), [c.r(), c.g(), c.b(), c.a()], (size * 0.004 / frame.scale) as f32);
        s.brush = Brush::Fill;
        s.layer = self.active_layer;
        for (i, l) in loops.iter().enumerate() {
            if i > 0 {
                let last = *s.points.last().expect("previous loop");
                s.points.push(StrokePoint { pressure: Stroke::SUBPATH_BREAK, ..last });
            }
            for p in l {
                let q = frame.screen_to_local(*p, &anchor, None);
                s.points.push(StrokePoint { x: q[0] as f32, y: q[1] as f32, pressure: 1.0 });
            }
        }
        s.refresh_cache();
        self.urgent.push(s.id);
        self.history.apply(&mut self.doc, Action::Add(vec![s]));
        self.changed();
        true
    }

    /// How videos are encoded on this platform (set by the launcher).
    pub fn set_video_factory(&mut self, f: crate::video::VideoFactory) {
        self.video_factory = Some(f);
    }

    /// Starts exporting a video of a flight through the bookmarks.
    pub fn start_video(&mut self) {
        let Some(factory) = self.video_factory.clone() else {
            self.toast = Some(("Eksport wideo nie jest dostępny na tym urządzeniu".into(), Instant::now()));
            return;
        };
        if self.doc.bookmarks.len() < 2 {
            self.toast = Some(("Dodaj co najmniej 2 zakładki".into(), Instant::now()));
            return;
        }
        let size = crate::video::size_for(self.viewport);
        let cams = crate::video::timeline(&self.doc.bookmarks, self.viewport, size);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let base = if self.project.is_empty() { "rysunek".to_string() } else { self.project.clone() };
        let job = crate::video::VideoJob::start(factory, format!("{base}-{stamp}"), size);
        self.video = Some(VideoExport { cams, next: 0, size, job, finishing: false });
        self.bookmark_panel.open = false;
        self.wants_redraw = true;
    }

    /// Starts or stops recording the canvas to a video.
    pub fn toggle_recording(&mut self) {
        if let Some(mut rec) = self.recording.take() {
            // Fill up to now with the last frame, then close the file.
            let due = (rec.start.elapsed().as_secs_f64() * crate::video::FPS as f64) as u64 + 1;
            let last = std::mem::take(&mut rec.last);
            if !last.is_empty() {
                for _ in rec.written..due.min(rec.written + 600) {
                    rec.job.push(last.clone());
                }
            }
            rec.job.finish();
            self.recording_saving = Some(rec.job);
            self.toast = Some(("Zapisywanie nagrania…".into(), Instant::now()));
            return;
        }
        let Some(factory) = self.video_factory.clone() else {
            self.toast = Some(("Nagrywanie nie jest dostępne na tym urządzeniu".into(), Instant::now()));
            return;
        };
        let size = crate::video::size_for(self.viewport);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let base = if self.project.is_empty() { "rysunek".to_string() } else { self.project.clone() };
        let job = crate::video::VideoJob::start(factory, format!("{base}-nagranie-{stamp}"), size);
        self.recording = Some(Recording { job, start: Instant::now(), written: 0, size, k: size[0] as f64 / self.viewport[0], last: Vec::new() });
        self.wants_redraw = true;
    }

    /// Whether the canvas is being recorded.
    pub fn recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Whether a video is being exported.
    pub fn exporting_video(&self) -> bool {
        self.video.is_some()
    }

    /// Renders the next video frames (a few per screen frame) and checks for the result.
    fn step_video(&mut self, target: Target, renderer: &mut Renderer) {
        let Some(mut v) = self.video.take() else { return };
        if let Some(r) = v.job.poll() {
            self.toast = Some((
                match r {
                    Ok(place) => format!("Zapisano wideo: {place}"),
                    Err(e) => format!("Wideo nie powstało: {e}"),
                },
                Instant::now(),
            ));
            return;
        }
        if !v.finishing {
            let (device, queue, format) = target.device_queue_format();
            let start = Instant::now();
            let layer_opacity: Vec<f32> = self.doc.layers.iter().map(|l| l.opacity).collect();
            self.decode_images();
            while v.next < v.cams.len() && start.elapsed() < Duration::from_millis(40) {
                let cam = &v.cams[v.next];
                let vvp = [v.size[0] as f64, v.size[1] as f64];
                let visible = self.doc.query(cam, vvp, [0.0, 0.0, vvp[0], vvp[1]]);
                let images = self.image_draws(cam, vvp);
                let px = renderer.render_rgba(
                    device,
                    queue,
                    format,
                    v.size,
                    FrameInput {
                        visible: &visible,
                        doc: &self.doc,
                        active: None,
                        scene: None,
                        images: &images,
                        grid: &[],
                        layer_opacity: &layer_opacity,
                        urgent: &[],
                        log_zoom: cam.log_zoom,
                        rotation: cam.rot(),
                        background: BACKGROUND,
                        egui: None,
                        textures: Default::default(),
                    },
                );
                if !v.job.push(px) {
                    v.finishing = true;
                    v.job.finish();
                    break;
                }
                v.next += 1;
            }
            if v.next >= v.cams.len() {
                v.finishing = true;
                v.job.finish();
            }
        }
        let pct = (100 * v.next / v.cams.len().max(1)).min(100);
        let msg = if v.finishing { "Zapisywanie wideo…".to_string() } else { format!("Eksport wideo: {pct}% (Wstecz – anuluj)") };
        self.toast = Some((msg, Instant::now()));
        self.video = Some(v);
        self.wants_redraw = true;
    }

    /// Flies to bookmark `i` (smooth zoom out → travel → zoom in).
    pub fn fly_to_bookmark(&mut self, i: usize) {
        self.bookmark_action(ui::BookmarkAction::Go(i));
    }

    /// Saves the current view as a bookmark.
    pub fn add_bookmark(&mut self) {
        self.bookmark_action(ui::BookmarkAction::Add);
    }

    /// Renders a small sample of every brush (for the brush menu) in the current colour.
    fn make_brush_previews(&mut self, target: Target, renderer: &mut Renderer) {
        let c = self.tools.color;
        self.tools.previews.clear();
        for brush in Brush::ALL {
            let px = brush_sample(target, renderer, brush, [c.r(), c.g(), c.b(), 255]);
            if px.len() == (SAMPLE_SIZE[0] * SAMPLE_SIZE[1] * 4) as usize {
                let img = egui::ColorImage::from_rgba_unmultiplied([SAMPLE_SIZE[0] as usize, SAMPLE_SIZE[1] as usize], &px);
                let tex = self.egui_ctx.load_texture(format!("brush:{brush:?}"), img, egui::TextureOptions::LINEAR);
                self.tools.previews.insert(brush, tex);
            }
        }
        self.tools.previews_color = Some(self.tools.color);
    }

    /// Exports the view as a picture (on the next frame).
    pub fn export(&mut self) {
        self.pending_export = true;
        self.wants_redraw = true;
    }

    /// Renders the current view at twice the screen resolution and hands it to the photo
    /// library to be encoded and saved (phone gallery / Pictures folder).
    fn export_png(&mut self, target: Target, renderer: &mut Renderer) {
        if self.photo_lib.is_none() {
            self.toast = Some(("Eksport nie jest dostępny na tym urządzeniu".into(), Instant::now()));
            return;
        }
        let vp = self.viewport;
        let (device, queue, format) = target.device_queue_format();
        let max = device.limits().max_texture_dimension_2d.min(8192) as f64;
        let k = (2.0f64).min(max / vp[0]).min(max / vp[1]);
        let w = (((vp[0] * k) / 64.0).floor() as u32 * 64).max(64);
        let k = w as f64 / vp[0];
        let h = ((vp[1] * k).round() as u32).max(1);
        let mut cam = self.camera.clone();
        cam.log_zoom += k.log2();
        let evp = [w as f64, h as f64];
        let visible = self.doc.query(&cam, evp, [0.0, 0.0, evp[0], evp[1]]);
        self.decode_images();
        let images = self.image_draws(&cam, evp);
        let layer_opacity: Vec<f32> = self.doc.layers.iter().map(|l| l.opacity).collect();
        let px = renderer.render_rgba(
            device,
            queue,
            format,
            [w, h],
            FrameInput {
                visible: &visible,
                doc: &self.doc,
                active: None,
                scene: None,
                images: &images,
                grid: &[],
                layer_opacity: &layer_opacity,
                urgent: &[],
                log_zoom: cam.log_zoom,
                rotation: cam.rot(),
                background: BACKGROUND,
                egui: None,
                textures: Default::default(),
            },
        );
        let Some(img) = image::RgbaImage::from_raw(w, h, px) else {
            self.toast = Some(("Nie udało się wyrenderować obrazu".into(), Instant::now()));
            return;
        };
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let base = if self.project.is_empty() { "rysunek".to_string() } else { self.project.clone() };
        if let Some(lib) = self.photo_lib.as_mut() {
            lib.save_png(format!("{base}-{stamp}"), img);
        }
        self.toast = Some(("Zapisywanie obrazu…".into(), Instant::now()));
    }

    /// Bucket fill: renders the current view offscreen, floods the enclosed area around the
    /// screen point `pos` and adds it as a vector fill (with holes around islands of ink).
    fn bucket_fill(&mut self, target: Target, renderer: &mut Renderer, pos: [f64; 2]) {
        let vp = self.viewport;
        // Mask at (slightly more than) screen resolution; width a multiple of 64 for readback.
        let w = ((vp[0] / 64.0).ceil() as u32 * 64).max(64);
        let k = w as f64 / vp[0];
        let h = ((vp[1] * k).round() as u32).max(1);
        let mut cam = self.camera.clone();
        cam.log_zoom += k.log2();
        let mvp = [w as f64, h as f64];
        let visible = self.doc.query(&cam, mvp, [0.0, 0.0, mvp[0], mvp[1]]);
        let (device, queue, format) = target.device_queue_format();
        // Magenta background: anything visibly different is ink (white ink included). Layers at
        // full opacity: faint layers still bound the fill.
        let bg = [1.0, 0.0, 1.0, 1.0];
        let layer_opacity: Vec<f32> = Vec::new();
        let px = renderer.render_rgba(
            device,
            queue,
            format,
            [w, h],
            FrameInput {
                visible: &visible,
                doc: &self.doc,
                active: None,
                scene: None,
                images: &[],
                grid: &[],
                layer_opacity: &layer_opacity,
                urgent: &[],
                log_zoom: cam.log_zoom,
                rotation: cam.rot(),
                background: bg,
                egui: None,
                textures: Default::default(),
            },
        );
        if px.len() != (w * h * 4) as usize {
            return;
        }
        let ink: Vec<bool> = px
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| (255 - p[0] as i32).abs() + p[1] as i32 + (255 - p[2] as i32).abs() > 90)
            .collect();
        let seed = ((pos[0] * k) as usize, (pos[1] * k) as usize);
        let Some(loops) = canvas_core::posterize::fill_region(&ink, w as usize, h as usize, seed) else {
            self.toast = Some(("Tu nie ma zamkniętego obszaru – obrysuj go albo przeciągnij, by narysować kształt".into(), Instant::now()));
            return;
        };
        let level = self.camera.level();
        let c = self.camera.screen_to_world(pos, vp).mul_pow2(level);
        let anchor = [c.x.floor_at(0), c.y.floor_at(0)];
        let frame = self.camera.level_frame(level, vp);
        let col = self.tools.color;
        // The fill's thin edge (1 mask px) overlaps the surrounding ink slightly: no gaps.
        let mut s = Stroke::new(self.doc.alloc_id(), level, anchor.clone(), [col.r(), col.g(), col.b(), col.a()], (1.0 / k / frame.scale) as f32);
        s.layer = self.active_layer;
        s.brush = Brush::Fill;
        for (i, l) in loops.iter().enumerate() {
            if i > 0 {
                let last = *s.points.last().expect("previous loop");
                s.points.push(StrokePoint { pressure: Stroke::SUBPATH_BREAK, ..last });
            }
            for p in l {
                let q = frame.screen_to_local([p[0] as f64 / k, p[1] as f64 / k], &anchor, None);
                s.points.push(StrokePoint { x: q[0] as f32, y: q[1] as f32, pressure: 1.0 });
            }
        }
        s.refresh_cache();
        self.urgent.push(s.id);
        self.history.apply(&mut self.doc, Action::Add(vec![s]));
        self.changed();
    }

    /// Decodes photos that aren't decoded yet (after loading a project, undo of a delete…).
    fn decode_images(&mut self) {
        let missing: Vec<(ImageId, Arc<Vec<u8>>)> =
            self.doc.images().filter(|i| !self.decoded.contains_key(&i.id)).map(|i| (i.id, i.data.clone())).collect();
        for (id, data) in missing {
            let rgba = crate::photos::decode(&data).map(|d| d.to_rgba8()).unwrap_or_else(|| {
                log::warn!("photo {id} could not be decoded");
                image::RgbaImage::from_pixel(1, 1, image::Rgba([200, 200, 200, 255]))
            });
            self.decoded.insert(id, Arc::new(rgba));
        }
    }

    fn image_draws(&self, cam: &Camera, vp: [f64; 2]) -> Vec<ImageDraw<'_>> {
        self.doc
            .images()
            .filter(|i| i.visible)
            .filter_map(|i| {
                Some(ImageDraw { id: i.id, corners: i.screen_corners(cam, vp), opacity: i.opacity, rgba: self.decoded.get(&i.id)? })
            })
            .collect()
    }

    fn begin_image_change(&mut self) {
        if let Some(e) = self.image_edit.as_mut()
            && e.before.is_none()
        {
            e.before = self.doc.image(e.id).cloned();
        }
    }

    fn commit_image_change(&mut self) {
        let Some(e) = self.image_edit.as_mut() else { return };
        e.last = None;
        let Some(before) = e.before.take() else { return };
        let after = self.doc.image(e.id).cloned();
        if after.as_ref() != Some(&before) {
            self.history.record(Action::Image { before: Some(Box::new(before)), after: after.map(Box::new) });
            self.changed();
        }
    }

    /// Applies a transform to the photo being edited.
    fn edit_image(&mut self, f: impl FnOnce(&mut RefImage, &Camera, [f64; 2])) {
        self.begin_image_change();
        let Some(id) = self.image_edit.as_ref().map(|e| e.id) else { return };
        let (cam, vp) = (self.camera.clone(), self.viewport);
        if let Some(img) = self.doc.image_mut(id) {
            f(img, &cam, vp);
        } else {
            self.image_edit = None;
        }
        self.content_rev += 1;
        self.wants_redraw = true;
    }

    /// Gestures while a photo is being edited move / scale / rotate it instead of the view.
    fn apply_to_image(&mut self, g: Gesture) {
        match g {
            Gesture::StrokeBegin { pos, .. } => {
                self.begin_image_change();
                if let Some(e) = self.image_edit.as_mut() {
                    e.last = Some(pos);
                }
            }
            Gesture::StrokeMove { pos, .. } => {
                let Some(last) = self.image_edit.as_ref().and_then(|e| e.last) else { return };
                if let Some(e) = self.image_edit.as_mut() {
                    e.last = Some(pos);
                }
                self.edit_image(|img, cam, vp| img.pan(cam, vp, [pos[0] - last[0], pos[1] - last[1]]));
            }
            Gesture::StrokeEnd | Gesture::PinchEnd { .. } => self.commit_image_change(),
            Gesture::StrokeCancel => {
                if let Some(e) = self.image_edit.as_mut() {
                    e.last = None;
                }
            }
            Gesture::Pan { delta } => self.edit_image(|img, cam, vp| img.pan(cam, vp, delta)),
            Gesture::Zoom { center, dlog } => self.edit_image(|img, cam, vp| img.zoom_at(cam, vp, center, dlog)),
            Gesture::Rotate { center, angle } => self.edit_image(|img, cam, vp| img.rotate_at(cam, vp, center, angle)),
            Gesture::Fling { .. } | Gesture::Stop => {}
        }
    }

    fn finish_image_edit(&mut self) {
        self.commit_image_change();
        self.image_edit = None;
        self.wants_redraw = true;
    }

    fn photo_action(&mut self, a: ui::PhotoAction) {
        use ui::PhotoAction::*;
        match a {
            StartPicking => {
                self.photo_panel.picking = true;
                self.photo_panel.message = None;
                if let Some(lib) = self.photo_lib.as_mut() {
                    lib.refresh();
                }
            }
            RequestAccess => {
                if let Some(lib) = self.photo_lib.as_mut() {
                    lib.request_access();
                    lib.entries = None;
                }
            }
            Pick(key) => {
                if let Some(lib) = self.photo_lib.as_mut() {
                    lib.load(&key);
                }
            }
            Opacity(id, o) => {
                if let Some(img) = self.doc.image_mut(id) {
                    img.opacity = o;
                    self.changed();
                }
            }
            ToggleVisible(id) => {
                if let Some(img) = self.doc.image(id).cloned() {
                    let mut after = img.clone();
                    after.visible = !after.visible;
                    self.history.apply(&mut self.doc, Action::Image { before: Some(Box::new(img)), after: Some(Box::new(after)) });
                    self.changed();
                }
            }
            Delete(id) => {
                if let Some(img) = self.doc.image(id).cloned() {
                    self.history.apply(&mut self.doc, Action::Image { before: Some(Box::new(img)), after: None });
                    if self.image_edit.as_ref().is_some_and(|e| e.id == id) {
                        self.image_edit = None;
                    }
                    self.changed();
                }
            }
            Edit(id) => {
                self.image_edit = Some(ImageEdit { id, before: None, last: None });
                self.photo_panel.open = false;
            }
            UseAsReference => {
                if let Some(bytes) = self.pending_photo.take() {
                    self.photo_panel.choosing = false;
                    match self.add_photo(&bytes) {
                        Ok(_) => self.photo_panel.open = false,
                        Err(e) => self.photo_panel.message = Some(e),
                    }
                }
            }
            Convert => {
                if let Some(bytes) = self.pending_photo.take() {
                    self.photo_panel.choosing = false;
                    let pp = &self.photo_panel;
                    let opts = DrawingOptions { detail: pp.detail, colored: pp.colored, outlines: pp.outlines, keep_photo: pp.keep_photo };
                    match self.photo_to_drawing(&bytes, opts) {
                        Ok(n) => {
                            log::info!("photo turned into {n} strokes");
                            self.photo_panel.open = false;
                        }
                        Err(e) => {
                            self.photo_panel.message = Some(e);
                            self.pending_photo = Some(bytes);
                            self.photo_panel.choosing = true;
                        }
                    }
                }
            }
            CancelChoice => {
                self.pending_photo = None;
                self.photo_panel.choosing = false;
            }
        }
    }

    /// Mouse wheel / touchpad zoom by `dlog` binary orders of magnitude around `pos`.
    pub fn wheel_zoom(&mut self, pos: [f64; 2], dlog: f64) {
        if self.image_edit.is_some() {
            self.edit_image(|img, cam, vp| img.zoom_at(cam, vp, pos, dlog));
            self.commit_image_change();
            return;
        }
        self.inertia = Inertia::default();
        self.flight = None;
        self.camera.zoom_at(pos, self.viewport, dlog);
        self.wants_redraw = true;
    }

    /// Rotates the view by `angle` radians around `pos` (desktop: Alt + wheel).
    pub fn wheel_rotate(&mut self, pos: [f64; 2], angle: f64) {
        if self.image_edit.is_some() {
            self.edit_image(|img, cam, vp| img.rotate_at(cam, vp, pos, angle));
            self.commit_image_change();
            return;
        }
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
        if self.image_edit.is_some() {
            self.apply_to_image(g);
            return;
        }
        if self.tools.tool == Tool::Select && self.apply_select(&g) {
            return;
        }
        if self.tools.tool == Tool::Text {
            match g {
                Gesture::StrokeBegin { pos, .. } => {
                    self.text_panel.open = true;
                    self.text_panel.pos = pos;
                    self.wants_redraw = true;
                    return;
                }
                Gesture::StrokeMove { .. } | Gesture::StrokeEnd | Gesture::StrokeCancel => return,
                _ => {}
            }
        }
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
                    if a.shape.is_some() {
                        // Shapes dragged by less than a few pixels are dropped (accidental taps).
                        let f = self.camera.level_frame(s.level, self.viewport);
                        let bb = s.cache.bbox;
                        let r = s.width as f64;
                        if ((bb[2] - bb[0]) as f64 - r).max((bb[3] - bb[1]) as f64 - r) * f.scale < 4.0 * self.pixels_per_point as f64 {
                            return;
                        }
                        if s.brush == Brush::Fill && s.points.len() <= 2 {
                            s.brush = Brush::Pen; // a "filled line" is just a line
                        }
                    }
                    // A tap with the fill brush = bucket fill of the enclosed area there.
                    if s.brush == Brush::Fill && !s.is_eraser() && a.shape.is_none() {
                        let f = self.camera.level_frame(s.level, self.viewport);
                        let len: f32 = s.points.windows(2).map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt()).sum();
                        if (len as f64) * f.scale < 12.0 * self.pixels_per_point as f64 {
                            let p = s.points[0];
                            let o = f.anchor_to_screen(&s.anchor, s.cache.anchor_fast);
                            let v = f.local_vec_to_screen([p.x as f64, p.y as f64]);
                            self.pending_bucket = Some([o[0] + v[0], o[1] + v[1]]);
                            self.wants_redraw = true;
                            return;
                        }
                    }
                    s.refresh_cache();
                    self.urgent.push(s.id);
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
            Gesture::Stop => {
                self.inertia = Inertia::default();
                self.flight = None;
            }
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
        let mut stroke = Stroke::new(id, level, anchor, color, (width_px / frame.scale) as f32);
        stroke.layer = self.active_layer;
        if !eraser {
            stroke.brush = self.tools.brush;
        }
        self.active = Some(ActiveStroke {
            stroke,
            smoother: PointerSmoother::default(),
            frozen: Vec::new(),
            frozen_upto: 0,
            speed: 0.0,
            last_sample: None,
            shape: if eraser { None } else { self.tools.shape.map(|sh| (sh, pos)) },
            raw: None,
            raw_vel: [0.0; 2],
        });
        self.extend_stroke(pos, pressure, time, kind);
    }

    fn extend_stroke(&mut self, pos: [f64; 2], pressure: f32, time: f64, kind: PointerKind) {
        let Some(a) = self.active.as_mut() else { return };
        if let Some((shape, start)) = a.shape {
            // A shape is rebuilt from its start to the pointer on every move; with the grid on,
            // its corners sit on grid crossings.
            let (start, pos) = if self.tools.show_grid {
                let g = crate::grid::Grid::new(&self.camera, self.viewport, self.pixels_per_point as f64);
                (g.snap(start), g.snap(pos))
            } else {
                (start, pos)
            };
            let frame = self.camera.level_frame(a.stroke.level, self.viewport);
            a.stroke.points = crate::shapes::points(shape, start, pos)
                .into_iter()
                .map(|p| {
                    let l = frame.screen_to_local(p, &a.stroke.anchor, a.stroke.cache.anchor_fast);
                    StrokePoint { x: l[0] as f32, y: l[1] as f32, pressure: 1.0 }
                })
                .collect();
            a.stroke.refresh_cache();
            return;
        }
        if let Some((lp, lt)) = a.raw {
            let dt = time - lt;
            if dt > 1e-4 {
                let v = [(pos[0] - lp[0]) / dt, (pos[1] - lp[1]) / dt];
                let k = (dt / (dt + 0.03)).min(1.0);
                a.raw_vel = [a.raw_vel[0] + (v[0] - a.raw_vel[0]) * k, a.raw_vel[1] + (v[1] - a.raw_vel[1]) * k];
            }
        }
        a.raw = Some((pos, time));
        // The eraser always has its full size.
        let pressure = if a.stroke.is_eraser() { 1.0 } else { Self::pressure(kind, pressure) };
        let (x, y, mut p) = a.smoother.push(pos[0], pos[1], pressure as f64, time);
        if a.stroke.brush == Brush::Dynamic {
            // Fast = thin: the width factor is stored as the point's pressure.
            if let Some((lp, lt)) = a.last_sample {
                let dt = (time - lt).max(1e-3);
                let v = ((x - lp[0]).powi(2) + (y - lp[1]).powi(2)).sqrt() / dt / self.pixels_per_point as f64;
                a.speed += (v - a.speed) * (dt / (dt + 0.05));
            }
            a.last_sample = Some(([x, y], time));
            let thin = (a.speed / 1500.0).clamp(0.0, 1.0);
            p = (1.0 - 0.7 * thin) * canvas_core::stroke::pressure_factor(p as f32) as f64;
        }
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
        // (A fill is previewed whole: filling it in parts would look wrong.)
        if a.stroke.brush != Brush::Fill && n - a.frozen_upto > FROZEN_PART {
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
        self.content_rev += 1;
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
        self.wants_redraw || self.inertia.active() || self.flight.is_some()
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

        if let Some((f, start)) = &self.flight {
            let t = start.elapsed().as_secs_f64();
            self.camera = f.at(t);
            if f.finished(t) {
                self.flight = None;
                self.changed();
            }
            self.wants_redraw = true;
        }

        if self.screen == Screen::Canvas && self.tools.previews_color != Some(self.tools.color) {
            self.make_brush_previews(target, renderer);
        }

        if std::mem::take(&mut self.want_gallery) && self.screen == Screen::Canvas {
            self.leave_to_gallery(target, renderer);
        }
        if let Some(pos) = self.pending_bucket.take() {
            self.bucket_fill(target, renderer, pos);
        }
        self.step_video(target, renderer);
        if std::mem::take(&mut self.pending_export) {
            self.export_png(target, renderer);
        }
        if let Some(r) = self.photo_lib.as_mut().and_then(|l| l.saved.take()) {
            self.toast = Some((
                match r {
                    Ok(place) => format!("Zapisano: {place}"),
                    Err(e) => format!("Nie zapisano: {e}"),
                },
                Instant::now(),
            ));
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
        let mut photo_actions = Vec::new();
        let mut bookmark_actions = Vec::new();
        let mut layer_actions = Vec::new();
        let mut finish_edit = false;
        let mut sel_action = None;
        let mut stop_recording = false;
        let mut text_action = None;
        let mut toast_visible = false;
        let mut rects = Vec::new();
        let lib_busy = self.photo_lib.as_mut().is_some_and(|l| l.poll());
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
            let (a, mut r) = ui::toolbar(ui.ctx(), &mut self.tools, &info, self.safe_top);
            let photos: Vec<ui::PhotoInfo> = self
                .doc
                .images()
                .map(|i| ui::PhotoInfo { id: i.id, opacity: i.opacity, visible: i.visible, rgba: self.decoded.get(&i.id).map(|a| &**a) })
                .collect();
            photo_actions = ui::photos_window(
                ui.ctx(),
                &mut self.photo_panel,
                &photos,
                self.pending_preview.as_ref(),
                self.photo_lib.as_mut(),
                self.safe_top,
            );
            if self.photo_panel.open {
                r.push(egui::Rect::EVERYTHING);
            }
            let layers: Vec<ui::LayerInfo> = self
                .doc
                .layers
                .iter()
                .map(|l| ui::LayerInfo { id: l.id, name: l.name.clone(), visible: l.visible, opacity: l.opacity, active: l.id == self.active_layer })
                .collect();
            layer_actions = ui::layers_window(ui.ctx(), &mut self.layer_panel, &layers, self.safe_top);
            if self.layer_panel.open {
                r.push(egui::Rect::EVERYTHING);
            }
            let names: Vec<String> = self.doc.bookmarks.iter().map(|b| b.name.clone()).collect();
            bookmark_actions = ui::bookmarks_window(ui.ctx(), &mut self.bookmark_panel, &names, self.safe_top);
            if self.bookmark_panel.open {
                r.push(egui::Rect::EVERYTHING);
            }
            if let Some((msg, t)) = &self.toast
                && t.elapsed() < Duration::from_millis(3000)
            {
                ui::toast(ui.ctx(), msg);
                toast_visible = true;
            }
            if self.video.is_some() {
                r.push(egui::Rect::EVERYTHING);
            }
            text_action = ui::text_window(ui.ctx(), &mut self.text_panel, &mut self.tools.text_size, self.safe_top);
            if self.text_panel.open {
                r.push(egui::Rect::EVERYTHING);
            }
            if let Some(rec) = &self.recording {
                let (stop, rect) = ui::recording_badge(ui.ctx(), rec.start.elapsed().as_secs_f64(), self.safe_top);
                stop_recording = stop;
                r.push(rect);
            }
            if self.tools.tool != Tool::Select {
                self.selection.clear();
                self.sel_drag = None;
            }
            let ppp = self.pixels_per_point;
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("selection")));
            let blue = egui::Color32::from_rgb(30, 120, 230);
            if let Some(SelDrag::Lasso(pts)) = &self.sel_drag {
                let line: Vec<egui::Pos2> = pts.iter().map(|p| egui::pos2(p[0] as f32 / ppp, p[1] as f32 / ppp)).collect();
                painter.add(egui::Shape::line(line, egui::Stroke::new(1.5, blue)));
            }
            if !self.selection.is_empty() {
                let x = self.sel_drag.as_ref().and_then(|d| d.xform());
                if let Some(b) = crate::selection::bbox(&self.doc, &self.camera, self.viewport, &self.selection, x) {
                    let rect = egui::Rect::from_min_max(egui::pos2(b[0] as f32 / ppp, b[1] as f32 / ppp), egui::pos2(b[2] as f32 / ppp, b[3] as f32 / ppp));
                    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.5, blue), egui::StrokeKind::Outside);
                    painter.circle(rect.max, 9.0, egui::Color32::WHITE, egui::Stroke::new(2.0, blue));
                }
                let (act, bar) = ui::selection_bar(ui.ctx(), self.selection.len());
                r.push(bar);
                sel_action = act;
            }
            if self.image_edit.is_some() && ui::image_edit_bar(ui.ctx()).0 {
                finish_edit = true;
            }
            if ui.ctx().is_pointer_over_egui() {
                r.push(egui::Rect::EVERYTHING);
            }
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
        for a in photo_actions {
            self.photo_action(a);
        }
        for a in bookmark_actions {
            self.bookmark_action(a);
        }
        for a in layer_actions {
            self.layer_action(a);
        }
        if actions.layers {
            self.layer_panel.open = !self.layer_panel.open;
            self.bookmark_panel.open = false;
            self.photo_panel.open = false;
        }
        if actions.bookmarks {
            self.bookmark_panel.open = !self.bookmark_panel.open;
            self.photo_panel.open = false;
        }
        match text_action {
            Some(ui::TextAction::Add) => {
                let text = std::mem::take(&mut self.text_panel.text);
                let size = self.tools.text_size as f64 * self.pixels_per_point as f64;
                self.text_panel.open = false;
                if !self.add_text(&text, self.text_panel.pos, size) {
                    self.toast = Some(("Nie udało się dodać tekstu".into(), Instant::now()));
                }
            }
            Some(ui::TextAction::Cancel) => self.text_panel.open = false,
            None => {}
        }
        if actions.grid_changed {
            // The grid is part of the cached scene.
            self.content_rev += 1;
            self.wants_redraw = true;
        }
        if actions.record || stop_recording {
            self.toggle_recording();
        }
        if let Some(r) = self.recording_saving.as_ref().and_then(|j| j.poll()) {
            self.recording_saving = None;
            self.toast = Some((
                match r {
                    Ok(place) => format!("Zapisano nagranie: {place}"),
                    Err(e) => format!("Nagranie nie powstało: {e}"),
                },
                Instant::now(),
            ));
        }
        if actions.export {
            self.pending_export = true;
            self.wants_redraw = true;
        }
        if actions.photos {
            self.photo_panel.open = !self.photo_panel.open;
            self.photo_panel.picking = false;
        }
        if finish_edit {
            self.finish_image_edit();
        }
        if let Some(a) = sel_action {
            self.selection_action(a);
        }
        if let Some((_, bytes)) = self.photo_lib.as_mut().and_then(|l| l.loaded.take()) {
            match bytes {
                Some(b) => self.offer_photo(b),
                None => self.photo_panel.message = Some("Nie udało się wczytać zdjęcia".into()),
            }
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
        let urgent = std::mem::take(&mut self.urgent);
        self.decode_images();
        let layer_opacity: Vec<f32> = self.doc.layers.iter().map(|l| l.opacity).collect();
        let vp = self.viewport;
        let (device, queue, format) = target.device_queue_format();

        // Is the view moving (gesture, inertia, flight, wheel)?
        if self.last_cam.as_ref() != Some(&self.camera) {
            self.last_move = now;
            self.last_cam = Some(self.camera.clone());
        }
        let moving = now - self.last_move < MOVE_SETTLE || self.inertia.active() || self.flight.is_some();
        let eraser_active = self.active.as_ref().is_some_and(|a| a.stroke.is_eraser());
        let sel_xform = self.sel_drag.as_ref().and_then(|d| d.xform());
        let drawing_ink = self.active.is_some() && !eraser_active;
        let canvas = self.screen == Screen::Canvas;

        // Scene cache: re-render it unless it can stand in for this frame. How well it fits the
        // current view: 0 = fine, 1 = slightly off (refresh soon), 2 = far off (refresh now).
        let cache_misfit = |c: &CacheInfo, cam: &Camera| -> u8 {
            let ratio = (cam.log_zoom - c.cam.log_zoom).exp2();
            let covered = [[0.0, 0.0], [vp[0], 0.0], [vp[0], vp[1]], [0.0, vp[1]], [vp[0] * 0.5, vp[1] * 0.5]]
                .iter()
                .filter(|p| {
                    let q = c.cam.world_to_screen(&cam.screen_to_world(**p, vp), c.vp);
                    q[0] >= 0.0 && q[1] >= 0.0 && q[0] <= c.vp[0] && q[1] <= c.vp[1]
                })
                .count();
            if !(0.4..=2.5).contains(&ratio) || covered <= 2 {
                2
            } else if !(0.6..=1.6).contains(&ratio) || covered < 5 {
                1
            } else {
                0
            }
        };
        let rerender = match &self.cache {
            _ if !canvas => false,
            None => true,
            Some(c) if c.rev != self.content_rev || eraser_active || self.image_edit.is_some() || sel_xform.is_some() => true,
            Some(c) if moving => match cache_misfit(c, &self.camera) {
                0 => false,
                1 => c.made.elapsed() >= CACHE_REFRESH,
                _ => true,
            },
            Some(_) if drawing_ink => false,
            Some(c) => c.cam != self.camera || !c.complete,
        };
        let mut visible_count = 0;
        let mut query_time = 0.0;
        if rerender {
            let m = [(vp[0] * CACHE_MARGIN).round(), (vp[1] * CACHE_MARGIN).round()];
            let cvp = [vp[0] + 2.0 * m[0], vp[1] + 2.0 * m[1]];
            let tq = Instant::now();
            let mut visible = self.doc.query(&self.camera, cvp, [0.0, 0.0, cvp[0], cvp[1]]);
            query_time = tq.elapsed().as_secs_f64();
            // Selection being moved / scaled: preview through the render transform (the cache
            // has the margin m around the screen, so screen points shift by m).
            if let Some(x) = sel_xform {
                let x = match x {
                    crate::selection::Xform::Scale { center, k } => crate::selection::Xform::Scale { center: [center[0] + m[0], center[1] + m[1]], k },
                    mv => mv,
                };
                for v in visible.iter_mut().filter(|v| self.selection.contains(&v.id)) {
                    x.apply_visible(v);
                }
            }
            visible_count = visible.len();
            let images = self.image_draws(&self.camera, cvp);
            // The eraser is previewed inside the scene (it removes ink there).
            let active = self.active.as_ref().filter(|a| a.stroke.is_eraser()).map(|a| {
                let f = self.camera.level_frame(a.stroke.level, cvp);
                ActiveDraw {
                    stroke: &a.stroke,
                    layer: self.doc.layer_pos(a.stroke.layer) as u16,
                    offset: f.anchor_to_screen(&a.stroke.anchor, a.stroke.cache.anchor_fast),
                    scale: f.scale,
                        frozen: &a.frozen,
                    tail_from: a.frozen_upto,
                    predicted: None,
                }
            });
            let grid = if self.tools.show_grid { crate::grid::Grid::new(&self.camera, cvp, self.pixels_per_point as f64).lines(cvp) } else { Vec::new() };
            renderer.render_scene_cache(
                device,
                queue,
                format,
                [cvp[0] as u32, cvp[1] as u32],
                FrameInput {
                    visible: &visible,
                    doc: &self.doc,
                    active,
                    scene: None,
                    images: &images,
                    grid: &grid,
                    layer_opacity: &layer_opacity,
                    urgent: &urgent,
                    log_zoom: self.camera.log_zoom,
                    rotation: self.camera.rot(),
                    background: BACKGROUND,
                    egui: None,
                    textures: Default::default(),
                },
            );
            self.cache = Some(CacheInfo {
                cam: self.camera.clone(),
                vp: cvp,
                rev: self.content_rev,
                complete: !renderer.needs_another_frame,
                made: Instant::now(),
            });
        }
        let incomplete = self.cache.as_ref().is_some_and(|c| !c.complete);
        // Where the cached scene lands on screen now.
        let scene = if canvas {
            self.cache.as_ref().map(|c| {
                [[0.0, 0.0], [c.vp[0], 0.0], [c.vp[0], c.vp[1]], [0.0, c.vp[1]]]
                    .map(|p| self.camera.world_to_screen(&c.cam.screen_to_world(p, c.vp), vp))
            })
        } else {
            None
        };
        // The ink stroke being drawn goes on top of the cached scene, its tip drawn slightly
        // ahead of the input.
        let ppp = self.pixels_per_point as f64;
        let active = self.active.as_ref().filter(|a| !a.stroke.is_eraser()).map(|a| {
            let f = self.camera.level_frame(a.stroke.level, vp);
            let predicted = match (a.raw, a.stroke.points.last()) {
                (Some((pos, _)), Some(last)) if a.shape.is_none() && a.stroke.brush != Brush::Fill => {
                    let mut d = [a.raw_vel[0] * PREDICT_AHEAD, a.raw_vel[1] * PREDICT_AHEAD];
                    let len = d[0].hypot(d[1]);
                    let max = PREDICT_MAX * ppp;
                    if len > max {
                        d = [d[0] * max / len, d[1] * max / len];
                    }
                    let l = f.screen_to_local([pos[0] + d[0], pos[1] + d[1]], &a.stroke.anchor, a.stroke.cache.anchor_fast);
                    Some(StrokePoint { x: l[0] as f32, y: l[1] as f32, pressure: last.pressure })
                }
                _ => None,
            };
            ActiveDraw {
                stroke: &a.stroke,
                layer: self.doc.layer_pos(a.stroke.layer) as u16,
                offset: f.anchor_to_screen(&a.stroke.anchor, a.stroke.cache.anchor_fast),
                scale: f.scale,
                frozen: &a.frozen,
                tail_from: a.frozen_upto,
                predicted,
            }
        });
        let capture_active = active.clone();
        let prims = ctx.tessellate(output.shapes, output.pixels_per_point);
        renderer.render_target(
            target,
            FrameInput {
                visible: &[],
                doc: &self.doc,
                active,
                scene,
                images: &[],
                grid: &[],
                layer_opacity: &layer_opacity,
                urgent: &[],
                log_zoom: self.camera.log_zoom,
                rotation: self.camera.rot(),
                background: BACKGROUND,
                egui: Some((&prims, output.pixels_per_point)),
                textures: output.textures_delta,
            },
        );
        // Recording: the same picture (cached scene + stroke in progress) at video size, no UI.
        if let Some(rec) = self.recording.as_mut()
            && canvas
        {
            let due = (rec.start.elapsed().as_secs_f64() * crate::video::FPS as f64) as u64 + 1;
            if due > rec.written {
                let k = rec.k;
                let active = capture_active.map(|mut a| {
                    a.offset = [a.offset[0] * k, a.offset[1] * k];
                    a.scale *= k;
                    a.predicted = None;
                    a
                });
                let px = renderer.render_rgba(
                    device,
                    queue,
                    format,
                    rec.size,
                    FrameInput {
                        visible: &[],
                        doc: &self.doc,
                        active,
                        scene: scene.map(|c| c.map(|p| [p[0] * k, p[1] * k])),
                        images: &[],
                        grid: &[],
                        layer_opacity: &layer_opacity,
                        urgent: &[],
                        log_zoom: self.camera.log_zoom + k.log2(),
                        rotation: self.camera.rot(),
                        background: BACKGROUND,
                        egui: None,
                        textures: Default::default(),
                    },
                );
                // Frames missed while nothing was redrawn repeat the previous picture.
                if !rec.last.is_empty() {
                    for _ in rec.written..(due - 1).min(rec.written + 600) {
                        rec.job.push(rec.last.clone());
                    }
                }
                if !rec.job.push(px.clone()) {
                    self.toast = Some(("Nagrywanie przerwane: błąd kodera".into(), Instant::now()));
                }
                rec.last = px;
                rec.written = due;
            }
            // Keep the clock in the badge ticking.
            self.wants_redraw = true;
        }
        self.stats.record(now.elapsed().as_secs_f64(), visible_count, query_time, renderer.timings);
        if incomplete || (canvas && moving) {
            // Keep going: missing meshes are on their way / the sharp frame follows the gesture.
            self.wants_redraw = true;
        }
        let egui_repaint = output.viewport_output.values().any(|v| v.repaint_delay.is_zero());
        self.wants_redraw |= renderer.needs_another_frame || egui_repaint || self.inertia.active() || lib_busy || toast_visible;
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
