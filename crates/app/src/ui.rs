//! Minimal floating toolbar (egui). Kept small and at the top edge so it never covers much.

use crate::projects::{self, ProjectInfo, Projects};
use canvas_core::stroke::Brush;
use std::collections::HashMap;
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pen,
    Eraser,
}

pub struct ToolState {
    pub tool: Tool,
    pub brush: Brush,
    pub color: Color32,
    /// Width of the current brush in logical points (screen size at drawing time).
    pub width: f32,
    /// Remembered width of every other brush.
    widths: HashMap<Brush, f32>,
    /// Eraser diameter in logical points.
    pub eraser: f32,
    pub palette: Vec<Color32>,
}

impl ToolState {
    /// Switches brush (and to drawing), keeping a separate width per brush.
    pub fn set_brush(&mut self, b: Brush) {
        self.tool = Tool::Pen;
        if b == self.brush {
            return;
        }
        self.widths.insert(self.brush, self.width);
        self.brush = b;
        self.width = self.widths.get(&b).copied().unwrap_or(b.default_width());
    }
}

/// Short brush label for the toolbar (the menu shows full names).
fn short_name(b: Brush) -> &'static str {
    match b {
        Brush::Pen => "Pióro",
        Brush::Fountain => "Wieczne",
        Brush::Dynamic => "Dynamiczne",
        Brush::Fixed => "Stałe",
        Brush::SoftPencil => "Ołówek M",
        Brush::HardPencil => "Ołówek T",
        Brush::Marker => "Marker",
        Brush::Dotted => "Kropki",
        Brush::Fill => "Wypełnij",
    }
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            tool: Tool::Pen,
            brush: Brush::Pen,
            widths: HashMap::new(),
            color: Color32::from_rgb(0x1d, 0x1d, 0x1f),
            width: 3.0,
            eraser: 24.0,
            palette: vec![
                Color32::from_rgb(0x1d, 0x1d, 0x1f),
                Color32::from_rgb(0x8e, 0x8e, 0x93),
                Color32::from_rgb(0xff, 0xff, 0xff),
                Color32::from_rgb(0xe5, 0x39, 0x35),
                Color32::from_rgb(0xfb, 0x8c, 0x00),
                Color32::from_rgb(0xfd, 0xd8, 0x35),
                Color32::from_rgb(0x43, 0xa0, 0x47),
                Color32::from_rgb(0x1e, 0x88, 0xe5),
                Color32::from_rgb(0x8e, 0x24, 0xaa),
            ],
        }
    }
}

pub struct Info {
    pub can_undo: bool,
    pub can_redo: bool,
    pub log_zoom: f64,
    /// View rotation in radians.
    pub rotation: f64,
    pub strokes: usize,
}

#[derive(Default, Debug)]
pub struct Actions {
    pub undo: bool,
    pub redo: bool,
    pub home: bool,
    pub menu: bool,
    /// Rotate the view back to upright.
    pub upright: bool,
    /// Open the photos window.
    pub photos: bool,
}

/// "×10^31", "250%", "×1/1000" style zoom text.
pub fn zoom_text(log_zoom: f64) -> String {
    let log10 = log_zoom * std::f64::consts::LOG10_2;
    if log10.abs() < 2.0 {
        format!("{:.0}%", 100.0 * log_zoom.exp2())
    } else if log10 > 0.0 {
        format!("×10^{:.0}", log10)
    } else {
        format!("×10^-{:.0}", -log10)
    }
}

fn swatch(ui: &mut egui::Ui, color: Color32, selected: bool) -> egui::Response {
    let size = ui.spacing().interact_size.y;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), size * 0.38, color);
    painter.circle_stroke(rect.center(), size * 0.38, Stroke::new(1.0, Color32::from_gray(120)));
    if selected {
        painter.circle_stroke(rect.center(), size * 0.48, Stroke::new(2.0, Color32::from_rgb(0x1e, 0x88, 0xe5)));
    }
    resp
}

/// Draws the toolbar; returns actions and the screen rects (in points) the UI covers.
/// Brush button (opens the list of brushes) and the eraser.
fn tool_buttons(ui: &mut egui::Ui, s: &mut ToolState) {
    let button = egui::Button::new(short_name(s.brush)).selected(s.tool == Tool::Pen);
    let r = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        ui.set_min_width(170.0);
        for b in Brush::ALL {
            if ui.add(egui::Button::new(b.name()).selected(s.tool == Tool::Pen && s.brush == b)).clicked() {
                s.set_brush(b);
                ui.close();
            }
        }
    });
    r.0.on_hover_text("Pędzel");
    ui.selectable_value(&mut s.tool, Tool::Eraser, "Gumka");
}

fn colors(ui: &mut egui::Ui, s: &mut ToolState) {
    let palette = s.palette.clone();
    for c in palette {
        if swatch(ui, c, s.color == c && s.tool == Tool::Pen).clicked() {
            s.color = c;
            s.tool = Tool::Pen;
        }
    }
    if egui::color_picker::color_edit_button_srgba(ui, &mut s.color, egui::color_picker::Alpha::Opaque).changed() {
        s.tool = Tool::Pen;
    }
}

fn size_slider(ui: &mut egui::Ui, s: &mut ToolState, label: bool) {
    let (value, range, decimals, text) = match s.tool {
        Tool::Pen => (&mut s.width, 0.5..=64.0, 1, "grubość"),
        Tool::Eraser => (&mut s.eraser, 4.0..=160.0, 0, "rozmiar"),
    };
    let slider = egui::Slider::new(value, range).logarithmic(true).max_decimals(decimals);
    ui.add(if label { slider.text(text) } else { slider }).on_hover_text(text);
}

fn history_buttons(ui: &mut egui::Ui, info: &Info, a: &mut Actions) {
    a.undo |= ui.add_enabled(info.can_undo, egui::Button::new("Cofnij")).clicked();
    a.redo |= ui.add_enabled(info.can_redo, egui::Button::new("Ponów")).clicked();
}

fn home_button(ui: &mut egui::Ui, a: &mut Actions) {
    a.home |= ui.button("Początek").on_hover_text("Wróć do początku (Home)").clicked();
}

fn photos_button(ui: &mut egui::Ui, a: &mut Actions) {
    a.photos |= ui.button("Zdjęcia").on_hover_text("Zdjęcie do odrysowania").clicked();
}

fn menu_button(ui: &mut egui::Ui, a: &mut Actions) {
    a.menu |= ui.button("Projekty").on_hover_text("Wróć do listy projektów").clicked();
}

/// Shown only while the view is rotated; a tap straightens it.
fn rotation_button(ui: &mut egui::Ui, info: &Info, a: &mut Actions) {
    let deg = info.rotation.to_degrees().round();
    if deg != 0.0 || info.rotation != 0.0 {
        a.upright |= ui.button(format!("{deg:.0}°")).on_hover_text("Obrót płótna – stuknij, aby wyprostować").clicked();
    }
}

fn zoom_label(ui: &mut egui::Ui, info: &Info) {
    ui.label(egui::RichText::new(zoom_text(info.log_zoom)).monospace())
        .on_hover_text(format!("log2 zoomu: {:.2}\nkresek: {}", info.log_zoom, info.strokes));
}

pub fn toolbar(ctx: &egui::Context, s: &mut ToolState, info: &Info, safe_top: f32) -> (Actions, Vec<Rect>) {
    let mut a = Actions::default();
    let mut rects = Vec::new();

    // Keyboard shortcuts (desktop).
    ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z)
            || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
        {
            a.redo = true;
        }
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z) {
            a.undo = true;
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::Home) {
            a.home = true;
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::E) {
            s.tool = Tool::Eraser;
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::P) {
            s.tool = Tool::Pen;
        }
    });

    // Explicit rows instead of `horizontal_wrapped`: egui's sliders are nested horizontal
    // groups that never wrap, which pushed the toolbar off narrow phone screens.
    let screen_w = ctx.content_rect().width();
    let frame = egui::Frame::popup(&ctx.global_style()).corner_radius(CornerRadius::same(12));
    let inner_w = (screen_w - 16.0 - frame.total_margin().sum().x).max(150.0);
    let compact = inner_w < 760.0;
    let anchor = if compact { Align2::LEFT_TOP } else { Align2::CENTER_TOP };
    let offset = if compact { [8.0, 8.0 + safe_top] } else { [0.0, 8.0 + safe_top] };
    let resp = egui::Area::new(egui::Id::unique("toolbar")).anchor(anchor, offset).show(ctx, |ui| {
        frame.show(ui, |ui| {
            ui.set_max_width(inner_w);
            if compact {
                ui.horizontal(|ui| {
                    menu_button(ui, &mut a);
                    tool_buttons(ui, s);
                    history_buttons(ui, info, &mut a);
                });
                ui.horizontal_wrapped(|ui| {
                    colors(ui, s);
                    photos_button(ui, &mut a);
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().slider_width = ui.spacing().slider_width.min(90.0);
                    size_slider(ui, s, false);
                    ui.separator();
                    home_button(ui, &mut a);
                    rotation_button(ui, info, &mut a);
                    zoom_label(ui, info);
                });
            } else {
                ui.horizontal(|ui| {
                    menu_button(ui, &mut a);
                    ui.separator();
                    tool_buttons(ui, s);
                    ui.separator();
                    colors(ui, s);
                    ui.separator();
                    size_slider(ui, s, true);
                    ui.separator();
                    history_buttons(ui, info, &mut a);
                    home_button(ui, &mut a);
                    photos_button(ui, &mut a);
                    ui.separator();
                    rotation_button(ui, info, &mut a);
                    zoom_label(ui, info);
                });
            }
        });
    });
    rects.push(resp.response.rect);
    // Popups (colour picker) also block the canvas.
    if egui::Popup::is_any_open(ctx) || ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input() {
        rects.push(Rect::EVERYTHING);
    }
    (a, rects)
}

/// One reference photo as listed in the photos window.
pub struct PhotoInfo {
    pub id: u64,
    pub opacity: f32,
    pub visible: bool,
}

pub struct PhotoPanel {
    pub open: bool,
    /// Showing the grid of photos to pick from.
    pub picking: bool,
    thumbs: HashMap<String, Option<egui::TextureHandle>>,
    pub message: Option<String>,
    /// A photo was loaded and the user chooses how to add it.
    pub choosing: bool,
    /// Detail of the photo → drawing conversion, 0..1.
    pub detail: f32,
    /// Keep the photo faintly under the converted drawing.
    pub keep_photo: bool,
}

impl Default for PhotoPanel {
    fn default() -> Self {
        Self {
            open: false,
            picking: false,
            thumbs: HashMap::new(),
            message: None,
            choosing: false,
            detail: 0.5,
            keep_photo: true,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum PhotoAction {
    StartPicking,
    RequestAccess,
    Pick(String),
    Opacity(u64, f32),
    ToggleVisible(u64),
    Delete(u64),
    Edit(u64),
    /// Add the loaded photo to trace over.
    UseAsReference,
    /// Turn the loaded photo into strokes.
    Convert,
    CancelChoice,
}

/// The photos window: photos in this drawing (opacity, move, hide, delete) and the picker.
pub fn photos_window(
    ctx: &egui::Context,
    st: &mut PhotoPanel,
    photos: &[PhotoInfo],
    lib: Option<&mut crate::photos::PhotoLibrary>,
    safe_top: f32,
) -> Vec<PhotoAction> {
    let mut out = Vec::new();
    if !st.open {
        return out;
    }
    let screen = ctx.content_rect();
    let width = (screen.width() - 16.0).min(520.0);
    let max_h = (screen.height() - safe_top - 24.0).max(240.0);
    let mut open = true;
    let title = if st.choosing {
        "Dodaj zdjęcie"
    } else if st.picking {
        "Wybierz zdjęcie"
    } else {
        "Zdjęcia"
    };
    egui::Window::new(title)
        .id(egui::Id::unique("photos"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, [0.0, 8.0 + safe_top])
        .fixed_size([width, 0.0])
        .show(ctx, |ui| {
            ui.set_width(width - 16.0);
            if let Some(m) = &st.message {
                ui.label(egui::RichText::new(m).italics());
            }
            if st.choosing {
                ui.label("Jak dodać to zdjęcie?");
                ui.add_space(4.0);
                if ui.button("Do odrysowania – zdjęcie pod rysunkiem").clicked() {
                    out.push(PhotoAction::UseAsReference);
                }
                ui.separator();
                ui.label("Zamień na rysunek – kontury zdjęcia jako kreski (aktualnym pędzlem i kolorem):");
                ui.add(egui::Slider::new(&mut st.detail, 0.0..=1.0).show_value(false).text("szczegóły"));
                ui.checkbox(&mut st.keep_photo, "zostaw blade zdjęcie pod spodem");
                ui.horizontal(|ui| {
                    if ui.button("Zamień na rysunek").clicked() {
                        out.push(PhotoAction::Convert);
                    }
                    if ui.button("Anuluj").clicked() {
                        out.push(PhotoAction::CancelChoice);
                    }
                });
                return;
            }
            if st.picking {
                picker(ui, st, lib, max_h - 120.0, &mut out);
                return;
            }
            if ui.button("Dodaj zdjęcie").clicked() {
                out.push(PhotoAction::StartPicking);
            }
            if photos.is_empty() {
                ui.label(egui::RichText::new("Dodane zdjęcie leży pod rysunkiem – możesz je odrysować.").weak());
            }
            for (i, p) in photos.iter().enumerate() {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(format!("Zdjęcie {}", i + 1));
                    let mut o = p.opacity;
                    if ui.add(egui::Slider::new(&mut o, 0.05..=1.0).show_value(false).text("krycie")).changed() {
                        out.push(PhotoAction::Opacity(p.id, o));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Przesuń / skaluj").clicked() {
                        out.push(PhotoAction::Edit(p.id));
                    }
                    if ui.button(if p.visible { "Ukryj" } else { "Pokaż" }).clicked() {
                        out.push(PhotoAction::ToggleVisible(p.id));
                    }
                    if ui.button("Usuń").clicked() {
                        out.push(PhotoAction::Delete(p.id));
                    }
                });
            }
        });
    if !open {
        st.open = false;
        st.picking = false;
        if st.choosing {
            out.push(PhotoAction::CancelChoice);
        }
    }
    out
}

fn picker(ui: &mut egui::Ui, st: &mut PhotoPanel, lib: Option<&mut crate::photos::PhotoLibrary>, max_h: f32, out: &mut Vec<PhotoAction>) {
    if ui.button("Wróć").clicked() {
        st.picking = false;
        return;
    }
    let Some(lib) = lib else {
        ui.label("Wybieranie zdjęć nie jest dostępne na tym urządzeniu.");
        return;
    };
    if lib.busy {
        ui.label("Wczytywanie zdjęcia…");
    }
    match &lib.entries {
        None => {
            ui.label("Szukam zdjęć…");
        }
        Some(Err(e)) => {
            ui.label(e.as_str());
            ui.horizontal(|ui| {
                if ui.button("Zezwól na dostęp do zdjęć").clicked() {
                    out.push(PhotoAction::RequestAccess);
                }
                if ui.button("Odśwież").clicked() {
                    lib.refresh();
                }
            });
        }
        Some(Ok(entries)) => {
            let entries = entries.clone();
            let avail = ui.available_width();
            let cols = ((avail + 6.0) / 106.0).floor().max(3.0) as usize;
            let side = (avail - 6.0 * (cols - 1) as f32) / cols as f32;
            egui::ScrollArea::vertical().max_height(max_h.max(200.0)).show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                for row in entries.chunks(cols) {
                    ui.horizontal(|ui| {
                        for e in row {
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), Sense::click());
                            if ui.is_rect_visible(rect) {
                                lib.request_thumb(&e.key);
                            }
                            if !st.thumbs.contains_key(&e.key)
                                && let Some(t) = lib.thumbs.get(&e.key)
                            {
                                let tex = t.as_ref().map(|img| {
                                    let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                                    ui.ctx().load_texture(format!("pick:{}", e.key), ci, egui::TextureOptions::LINEAR)
                                });
                                st.thumbs.insert(e.key.clone(), tex);
                            }
                            ui.painter().rect_filled(rect, CornerRadius::same(6), Color32::from_gray(225));
                            if let Some(Some(tex)) = st.thumbs.get(&e.key) {
                                // Centre-crop to a square.
                                let [w, h] = tex.size().map(|v| v as f32);
                                let k = w.min(h);
                                let uv = Rect::from_min_size(
                                    egui::pos2((w - k) / (2.0 * w), (h - k) / (2.0 * h)),
                                    egui::vec2(k / w, k / h),
                                );
                                egui::Image::new((tex.id(), rect.size())).uv(uv).corner_radius(CornerRadius::same(6)).paint_at(ui, rect);
                            }
                            if resp.clicked() && !lib.busy {
                                out.push(PhotoAction::Pick(e.key.clone()));
                            }
                        }
                    });
                }
            });
        }
    }
}

/// Bar shown while a photo is being moved: what the fingers do, and "done".
pub fn image_edit_bar(ctx: &egui::Context) -> (bool, Rect) {
    let mut done = false;
    let r = egui::Area::new(egui::Id::unique("image-edit"))
        .anchor(Align2::CENTER_BOTTOM, [0.0, -24.0])
        .show(ctx, |ui| {
            let max_w = (ui.ctx().content_rect().width() - 48.0).max(160.0);
            egui::Frame::popup(ui.style()).corner_radius(CornerRadius::same(12)).show(ui, |ui| {
                ui.set_max_width(max_w);
                ui.vertical_centered(|ui| {
                    ui.label("Przesuń zdjęcie palcem, dwoma palcami skaluj i obracaj");
                    done = ui.button("    Gotowe    ").clicked();
                });
            });
        });
    (done, r.response.rect)
}

/// State of the projects gallery (start screen).
#[derive(Default)]
pub struct GalleryState {
    pub list: Vec<ProjectInfo>,
    /// Loaded previews by project name (`None` = no preview file).
    pub thumbs: HashMap<String, Option<egui::TextureHandle>>,
    /// Project whose options dialog is open.
    pub options: Option<String>,
    pub name_edit: String,
    pub confirm_delete: bool,
    /// Feedback line (errors, "zmieniono nazwę").
    pub message: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum GalleryAction {
    New,
    Open(String),
    Rename { from: String, to: String },
    Duplicate(String),
    Delete(String),
}

const CARD_MIN_WIDTH: f32 = 150.0;
const CARD_GAP: f32 = 12.0;
/// Preview aspect (height / width), same as the rendered thumbnails.
pub const THUMB_ASPECT: f32 = 340.0 / 256.0;

/// Full-screen grid of projects with previews. Returns what the user picked.
pub fn gallery(root: &mut egui::Ui, st: &mut GalleryState, projects: &Projects, safe_top: f32, background: Color32) -> Vec<GalleryAction> {
    let ctx = root.ctx().clone();
    let mut out = Vec::new();
    // Load previews lazily (they are small PNGs).
    for p in &st.list {
        if !st.thumbs.contains_key(&p.name) {
            let tex = projects.load_thumbnail(&p.name).map(|(w, h, rgba)| {
                let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                ctx.load_texture(format!("thumb:{}", p.name), img, egui::TextureOptions::LINEAR)
            });
            st.thumbs.insert(p.name.clone(), tex);
        }
    }
    let dialog_open = st.options.is_some();
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(background).inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 0 }))
        .show(root, |ui| {
            ui.add_space(safe_top + 16.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Projekty").size(28.0).strong());
            });
            if let Some(m) = &st.message {
                ui.label(egui::RichText::new(m).italics().weak());
            }
            ui.add_space(8.0);
            let avail = ui.available_width();
            let cols = ((avail + CARD_GAP) / (CARD_MIN_WIDTH + CARD_GAP)).floor().max(2.0) as usize;
            let card_w = (avail - CARD_GAP * (cols - 1) as f32) / cols as f32;
            let thumb = egui::vec2(card_w, card_w * THUMB_ASPECT);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(CARD_GAP, CARD_GAP);
                let items: Vec<Option<&ProjectInfo>> = std::iter::once(None).chain(st.list.iter().map(Some)).collect();
                for row in items.chunks(cols) {
                    ui.horizontal_top(|ui| {
                        for item in row {
                            ui.vertical(|ui| {
                                ui.set_width(card_w);
                                ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
                                let (rect, resp) = ui.allocate_exact_size(thumb, if dialog_open { Sense::hover() } else { Sense::click() });
                                let painter = ui.painter();
                                let corner = CornerRadius::same(12);
                                painter.rect_filled(rect, corner, Color32::WHITE);
                                match item {
                                    None => {
                                        painter.text(rect.center() - egui::vec2(0.0, 14.0), Align2::CENTER_CENTER, "+", egui::FontId::proportional(56.0), Color32::from_gray(90));
                                        painter.text(rect.center() + egui::vec2(0.0, 30.0), Align2::CENTER_CENTER, "Nowa kartka", egui::FontId::proportional(16.0), Color32::from_gray(70));
                                        if resp.clicked() {
                                            out.push(GalleryAction::New);
                                        }
                                    }
                                    Some(p) => {
                                        if let Some(Some(tex)) = st.thumbs.get(&p.name) {
                                            egui::Image::new((tex.id(), thumb)).corner_radius(corner).paint_at(ui, rect);
                                        }
                                        if resp.clicked() {
                                            out.push(GalleryAction::Open(p.name.clone()));
                                        }
                                    }
                                }
                                let border = if resp.hovered() { Color32::from_gray(150) } else { Color32::from_gray(205) };
                                ui.painter().rect_stroke(rect, corner, Stroke::new(1.0, border), egui::StrokeKind::Inside);
                                if let Some(p) = item {
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.set_width(card_w - 52.0);
                                            ui.add(egui::Label::new(egui::RichText::new(&p.name).strong()).truncate());
                                            ui.label(egui::RichText::new(projects::ago(p.modified)).small().weak());
                                        });
                                        if ui.add_enabled(!dialog_open, egui::Button::new("•••")).on_hover_text("Opcje").clicked() {
                                            st.options = Some(p.name.clone());
                                            st.name_edit = p.name.clone();
                                            st.confirm_delete = false;
                                            st.message = None;
                                        }
                                    });
                                }
                            });
                        }
                    });
                }
                ui.add_space(24.0);
            });
        });

    // Options of one project.
    if let Some(name) = st.options.clone() {
        let mut open = true;
        let width = (ctx.content_rect().width() - 32.0).min(400.0);
        egui::Window::new("Opcje projektu")
            .id(egui::Id::unique("project-options"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, [0.0, safe_top + 80.0])
            .fixed_size([width, 0.0])
            .show(&ctx, |ui| {
                ui.set_width(width - 16.0);
                ui.label(egui::RichText::new("Nazwa").small().weak());
                let r = ui.add(egui::TextEdit::singleline(&mut st.name_edit).desired_width(f32::INFINITY));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let changed = !st.name_edit.trim().is_empty() && st.name_edit.trim() != name;
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(changed, egui::Button::new("Zmień nazwę")).clicked() || (enter && changed) {
                        out.push(GalleryAction::Rename { from: name.clone(), to: st.name_edit.clone() });
                    }
                    if ui.button("Duplikuj").clicked() {
                        out.push(GalleryAction::Duplicate(name.clone()));
                    }
                    if st.confirm_delete {
                        if ui.button(egui::RichText::new("Usuń na zawsze").color(Color32::from_rgb(0xc6, 0x28, 0x28))).clicked() {
                            out.push(GalleryAction::Delete(name.clone()));
                        }
                        if ui.button("Nie usuwaj").clicked() {
                            st.confirm_delete = false;
                        }
                    } else if ui.button("Usuń").clicked() {
                        st.confirm_delete = true;
                    }
                });
                if let Some(m) = &st.message {
                    ui.label(egui::RichText::new(m).italics());
                }
            });
        if !open {
            st.options = None;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn zoom_text() {
        assert_eq!(super::zoom_text(0.0), "100%");
        assert_eq!(super::zoom_text(100.0), "×10^30");
        assert_eq!(super::zoom_text(-100.0), "×10^-30");
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    /// The toolbar must fit inside a narrow phone screen (360 pt wide).
    #[test]
    fn toolbar_fits_phone_width() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|st| {
            st.spacing.interact_size = egui::vec2(40.0, 34.0);
            st.spacing.item_spacing = egui::vec2(8.0, 6.0);
            st.spacing.button_padding = egui::vec2(8.0, 6.0);
            st.spacing.slider_width = 120.0;
        });
        let info = Info { can_undo: true, can_redo: false, log_zoom: -33.4, rotation: -2.5, strokes: 2 };
        let mut rect = Rect::NOTHING;
        for brush in Brush::ALL {
            let mut s = ToolState::default();
            s.set_brush(brush);
            for _ in 0..4 {
                let raw = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 800.0))),
                    ..Default::default()
                };
                let mut out = ctx.run_ui(raw, |ui| {
                    let (_, r) = toolbar(ui.ctx(), &mut s, &info, 24.0);
                    rect = r[0];
                });
                out.textures_delta.clear();
            }
            assert!(rect.min.x >= 0.0 && rect.max.x <= 360.0, "{brush:?}: {rect:?}");
        }
        assert!(rect.min.x >= 0.0 && rect.max.x <= 360.0, "{rect:?}");
        assert!(rect.min.y >= 24.0, "{rect:?}");

        // The photo edit bar fits too.
        let mut bar = Rect::NOTHING;
        for _ in 0..4 {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 800.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ui| bar = image_edit_bar(ui.ctx()).1);
            out.textures_delta.clear();
        }
        assert!(bar.min.x >= 0.0 && bar.max.x <= 360.0 && bar.max.y <= 800.0, "{bar:?}");
    }
}
