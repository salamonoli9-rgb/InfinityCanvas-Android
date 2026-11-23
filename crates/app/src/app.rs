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
        self.decode_images();
        let images = self.image_draws(&cam, vp);
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
                images: &images,
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
        if self.image_edit.is_some() {
            self.finish_image_edit();
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
        // Magenta background: anything visibly different is ink (white ink included).
        let bg = [1.0, 0.0, 1.0, 1.0];
        let px = renderer.render_rgba(
            device,
            queue,
            format,
            [w, h],
            FrameInput {
                visible: &visible,
                doc: &self.doc,
                active: None,
                images: &[],
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
                    // A tap with the fill brush = bucket fill of the enclosed area there.
                    if s.brush == Brush::Fill && !s.is_eraser() {
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
        let mut stroke = Stroke::new(id, level, anchor, color, (width_px / frame.scale) as f32);
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
        });
        self.extend_stroke(pos, pressure, time, kind);
    }

    fn extend_stroke(&mut self, pos: [f64; 2], pressure: f32, time: f64, kind: PointerKind) {
        let Some(a) = self.active.as_mut() else { return };
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
        if let Some(pos) = self.pending_bucket.take() {
            self.bucket_fill(target, renderer, pos);
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
        let mut finish_edit = false;
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
            if let Some((msg, t)) = &self.toast
                && t.elapsed() < Duration::from_millis(3000)
            {
                ui::toast(ui.ctx(), msg);
                toast_visible = true;
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
        if actions.photos {
            self.photo_panel.open = !self.photo_panel.open;
            self.photo_panel.picking = false;
        }
        if finish_edit {
            self.finish_image_edit();
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
        self.decode_images();
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
        let images = if self.screen == Screen::Canvas { self.image_draws(&self.camera, self.viewport) } else { Vec::new() };
        let prims = ctx.tessellate(output.shapes, output.pixels_per_point);
        renderer.render_target(
            target,
            FrameInput {
                visible: &visible,
                doc: &self.doc,
                active,
                images: &images,
                log_zoom: self.camera.log_zoom,
                rotation: self.camera.rot(),
                background: BACKGROUND,
                egui: Some((&prims, output.pixels_per_point)),
                textures: output.textures_delta,
            },
        );
        self.stats.record(now.elapsed().as_secs_f64(), visible.len(), query_time, renderer.timings);
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
