//! Minimal floating toolbar (egui). Kept small and at the top edge so it never covers much.

use crate::projects::{self, ProjectInfo, Projects};
use crate::shapes::Shape;
use canvas_core::stroke::Brush;
use std::collections::HashMap;
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pen,
    Eraser,
    /// One finger moves the view (nothing is drawn): for showing a drawing around.
    Hand,
    /// Lasso selection: select strokes, then move / scale / delete / duplicate / recolour them.
    Select,
    /// Tap to place vector text.
    Text,
    /// Drag to measure a distance and an angle.
    Measure,
    /// Drag over strokes to bend them.
    Nudge,
}

pub struct ToolState {
    pub tool: Tool,
    pub brush: Brush,
    /// Draw a straight line / rectangle / ellipse instead of freehand.
    pub shape: Option<Shape>,
    pub color: Color32,
    /// Width of the current brush in logical points (screen size at drawing time).
    pub width: f32,
    /// Remembered width of every other brush.
    widths: HashMap<Brush, f32>,
    /// Text height in logical points.
    pub text_size: f32,
    /// Show the canvas grid (shape guides snap to it).
    pub show_grid: bool,
    /// Radius of the nudge tool, logical points.
    pub nudge: f32,
    /// Sample of how each brush draws (shown in the brush menu), and the colour they were made in.
    pub previews: HashMap<Brush, egui::TextureHandle>,
    pub previews_color: Option<Color32>,
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
pub fn short_name(b: Brush) -> &'static str {
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
        Brush::Ballpoint => "Długopis",
        Brush::Brush => "Pędzel",
        Brush::Highlighter => "Zakreślacz",
        Brush::Crayon => "Kredka",
        Brush::Charcoal => "Węgiel",
        Brush::Airbrush => "Aerograf",
        Brush::Watercolor => "Akwarela",
        Brush::Spray => "Spray",
    }
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            tool: Tool::Pen,
            brush: Brush::Pen,
            shape: None,
            widths: HashMap::new(),
            previews: HashMap::new(),
            previews_color: None,
            text_size: 32.0,
            show_grid: false,
            nudge: 40.0,
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
    /// Export the view as a picture.
    pub export: bool,
    /// Open the bookmarks window.
    pub bookmarks: bool,
    /// Open the layers window.
    pub layers: bool,
    /// Start / stop recording a video of the canvas.
    pub record: bool,
    /// The grid was switched on or off.
    pub grid_changed: bool,
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
    let label = s.shape.map_or(short_name(s.brush), Shape::name);
    let button = egui::Button::new(label).selected(s.tool == Tool::Pen);
    let r = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        ui.set_min_width(250.0);
        let max_h = (ui.ctx().content_rect().height() * 0.62).max(240.0);
        egui::ScrollArea::vertical().scroll_source(egui::scroll_area::ScrollSource::ALL).max_height(max_h).show(ui, |ui| {
            for b in Brush::ALL {
                let selected = s.tool == Tool::Pen && s.brush == b;
                let button = match s.previews.get(&b) {
                    // The sample of how the brush draws, then its name.
                    Some(tex) => egui::Button::image_and_text(egui::Image::new((tex.id(), egui::vec2(96.0, 24.0))), b.name()),
                    None => egui::Button::new(b.name()),
                };
                if ui.add(button.selected(selected)).clicked() {
                    s.set_brush(b);
                    ui.close();
                }
            }
        });
        ui.separator();
        ui.label(egui::RichText::new("Kształt (aktualnym pędzlem)").small().weak());
        ui.horizontal_wrapped(|ui| {
            if ui.add(egui::Button::new("Odręcznie").selected(s.shape.is_none())).clicked() {
                s.shape = None;
                s.tool = Tool::Pen;
                ui.close();
            }
            for sh in Shape::ALL {
                if ui.add(egui::Button::new(sh.name()).selected(s.shape == Some(sh))).clicked() {
                    s.shape = Some(sh);
                    s.tool = Tool::Pen;
                    ui.close();
                }
            }
        });
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
        Tool::Pen | Tool::Hand | Tool::Select => (&mut s.width, 0.5..=64.0, 1, "grubość"),
        Tool::Text => (&mut s.text_size, 8.0..=240.0, 0, "rozmiar tekstu"),
        Tool::Measure => (&mut s.width, 0.5..=64.0, 1, "grubość"),
        Tool::Nudge => (&mut s.nudge, 8.0..=200.0, 0, "zasięg"),
        Tool::Eraser => (&mut s.eraser, 4.0..=160.0, 0, "rozmiar"),
    };
    let slider = egui::Slider::new(value, range).logarithmic(true).max_decimals(decimals);
    ui.add(if label { slider.text(text) } else { slider }).on_hover_text(text);
}

fn history_buttons(ui: &mut egui::Ui, info: &Info, a: &mut Actions) {
    a.undo |= ui.add_enabled(info.can_undo, egui::Button::new("Cofnij")).clicked();
    a.redo |= ui.add_enabled(info.can_redo, egui::Button::new("Ponów")).clicked();
}

fn hand_button(ui: &mut egui::Ui, s: &mut ToolState) {
    ui.selectable_value(&mut s.tool, Tool::Select, "Zaznacz").on_hover_text("Obrysuj kreski palcem, potem przeciągnij je albo zmień rozmiar uchwytem w rogu");
    ui.selectable_value(&mut s.tool, Tool::Text, "Tekst").on_hover_text("Stuknij w płótno, żeby wpisać tekst (wektorowy)");
    ui.selectable_value(&mut s.tool, Tool::Nudge, "Wygnij").on_hover_text("Przeciągnij po kreskach, żeby je wygiąć (zasięg – suwak)");
    ui.selectable_value(&mut s.tool, Tool::Measure, "Miarka").on_hover_text("Przeciągnij, żeby zmierzyć odległość i kąt");
    ui.selectable_value(&mut s.tool, Tool::Hand, "Przesuń")
        .on_hover_text("Jeden palec przesuwa płótno, nic nie rysuje. Dwa stuknięcia i przeciągnięcie w górę/dół – zoom.");
}

fn home_button(ui: &mut egui::Ui, a: &mut Actions) {
    a.home |= ui.button("Początek").on_hover_text("Wróć do początku (Home)").clicked();
}

/// "More": photos, bookmarks, export.
fn photos_button(ui: &mut egui::Ui, a: &mut Actions, s: &mut ToolState) {
    egui::containers::menu::MenuButton::from_button(egui::Button::new("Więcej")).ui(ui, |ui| {
        ui.set_min_width(190.0);
        if ui.checkbox(&mut s.show_grid, "Siatka (kształty się przyciągają)").changed() {
            a.grid_changed = true;
        }
        if ui.button("Zdjęcia…").clicked() {
            a.photos = true;
            ui.close();
        }
        if ui.button("Warstwy…").clicked() {
            a.layers = true;
            ui.close();
        }
        if ui.button("Zakładki…").clicked() {
            a.bookmarks = true;
            ui.close();
        }
        if ui.button("Eksport do PNG").clicked() {
            a.export = true;
            ui.close();
        }
        if ui.button("Nagrywaj wideo (MP4)").on_hover_text("Nagrywa płótno – rysowanie, przesuwanie, zoom – bez paska narzędzi").clicked() {
            a.record = true;
            ui.close();
        }
    });
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
                    hand_button(ui, s);
                    photos_button(ui, &mut a, s);
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
                    hand_button(ui, s);
                    ui.separator();
                    colors(ui, s);
                    ui.separator();
                    size_slider(ui, s, true);
                    ui.separator();
                    history_buttons(ui, info, &mut a);
                    home_button(ui, &mut a);
                    photos_button(ui, &mut a, s);
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
pub struct PhotoInfo<'a> {
    pub id: u64,
    pub opacity: f32,
    pub visible: bool,
    /// Decoded pixels, for the little preview next to the name.
    pub rgba: Option<&'a image::RgbaImage>,
}

/// A small texture of `img` (longest side `side` px), cached under `key`.
fn mini_texture(ctx: &egui::Context, cache: &mut HashMap<String, Option<egui::TextureHandle>>, key: String, img: &image::RgbaImage, side: u32) -> egui::TextureHandle {
    cache
        .entry(key.clone())
        .or_insert(None)
        .get_or_insert_with(|| {
            let (w, h) = img.dimensions();
            let k = (side as f32 / w.max(h).max(1) as f32).min(1.0);
            let small = image::imageops::thumbnail(img, ((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1));
            let ci = egui::ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw());
            ctx.load_texture(key, ci, egui::TextureOptions::LINEAR)
        })
        .clone()
}

/// Draws a texture fitted into a box of `side`×`side` points.
fn mini_image(ui: &mut egui::Ui, tex: &egui::TextureHandle, side: f32) {
    let [w, h] = tex.size().map(|v| v as f32);
    let k = side / w.max(h);
    ui.add(egui::Image::new((tex.id(), egui::vec2(w * k, h * k))).corner_radius(CornerRadius::same(4)));
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
    /// Coloured illustration (patches) rather than outlines only.
    pub colored: bool,
    /// Outlines on top of the colours.
    pub outlines: bool,
}

impl PhotoPanel {
    /// Forgets the preview of the photo being chosen (a new one will be shown next time).
    pub fn forget_pending_preview(&mut self) {
        self.thumbs.remove("pending-preview");
    }
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
            keep_photo: false,
            colored: true,
            outlines: true,
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
    pending_preview: Option<&image::RgbaImage>,
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
                ui.horizontal(|ui| {
                    if let Some(img) = pending_preview {
                        let tex = mini_texture(ui.ctx(), &mut st.thumbs, "pending-preview".into(), img, 160);
                        mini_image(ui, &tex, 96.0);
                    }
                    ui.label("Jak dodać to zdjęcie?");
                });
                ui.add_space(4.0);
                if ui.button("Do odrysowania – zdjęcie pod rysunkiem").clicked() {
                    out.push(PhotoAction::UseAsReference);
                }
                ui.separator();
                ui.label("Zamień na rysunek (wektorowy, można go potem edytować):");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut st.colored, true, "Kolorowy");
                    ui.selectable_value(&mut st.colored, false, "Same kontury");
                });
                ui.add(egui::Slider::new(&mut st.detail, 0.0..=1.0).show_value(false).text("szczegóły"));
                if st.colored {
                    ui.checkbox(&mut st.outlines, "kontury na wierzchu (aktualnym pędzlem i kolorem)");
                } else {
                    ui.label(egui::RichText::new("Kontury aktualnym pędzlem, kolorem i grubością.").small().weak());
                }
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
                    if let Some(img) = p.rgba {
                        let tex = mini_texture(ui.ctx(), &mut st.thumbs, format!("photo:{}", p.id), img, 128);
                        mini_image(ui, &tex, 56.0);
                    }
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
            egui::ScrollArea::vertical().scroll_source(egui::scroll_area::ScrollSource::ALL).max_height(max_h.max(200.0)).show(ui, |ui| {
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

#[derive(Default)]
pub struct BookmarkPanel {
    pub open: bool,
    /// Bookmark being renamed, with the text field contents.
    pub renaming: Option<(usize, String)>,
}

#[derive(Debug, PartialEq)]
pub enum BookmarkAction {
    Add,
    /// Video of a flight through all bookmarks, in order.
    ExportVideo,
    Go(usize),
    Rename(usize, String),
    Delete(usize),
}

/// The bookmarks window: saved views of this drawing; tapping one flies there.
pub fn bookmarks_window(ctx: &egui::Context, st: &mut BookmarkPanel, names: &[String], safe_top: f32) -> Vec<BookmarkAction> {
    let mut out = Vec::new();
    if !st.open {
        return out;
    }
    let screen = ctx.content_rect();
    let width = (screen.width() - 16.0).min(440.0);
    let max_h = (screen.height() - safe_top - 140.0).max(200.0);
    let mut open = true;
    egui::Window::new("Zakładki")
        .id(egui::Id::unique("bookmarks"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, [0.0, 8.0 + safe_top])
        .fixed_size([width, 0.0])
        .show(ctx, |ui| {
            ui.set_width(width - 16.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Dodaj zakładkę (obecny widok)").clicked() {
                    out.push(BookmarkAction::Add);
                }
                if ui
                    .add_enabled(names.len() >= 2, egui::Button::new("Wideo MP4"))
                    .on_hover_text("Film z przelotem przez zakładki po kolei")
                    .on_disabled_hover_text("Potrzebne są co najmniej 2 zakładki")
                    .clicked()
                {
                    out.push(BookmarkAction::ExportVideo);
                }
            });
            if names.is_empty() {
                ui.label(egui::RichText::new("Zakładka zapamiętuje miejsce, przybliżenie i obrót. Stuknij ją, żeby tam przelecieć.").weak());
            }
            egui::ScrollArea::vertical().scroll_source(egui::scroll_area::ScrollSource::ALL).max_height(max_h).show(ui, |ui| {
                for (i, name) in names.iter().enumerate() {
                    ui.separator();
                    if let Some((j, text)) = st.renaming.as_mut()
                        && *j == i
                    {
                        let mut done = false;
                        ui.horizontal(|ui| {
                            let r = ui.add(egui::TextEdit::singleline(text).desired_width(ui.available_width() - 70.0));
                            done = ui.button("OK").clicked() || (r.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter)));
                        });
                        if done {
                            out.push(BookmarkAction::Rename(i, text.clone()));
                            st.renaming = None;
                        }
                        continue;
                    }
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(egui::RichText::new(name).strong()).frame(false)).clicked() {
                            out.push(BookmarkAction::Go(i));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Usuń").clicked() {
                                out.push(BookmarkAction::Delete(i));
                            }
                            if ui.button("Nazwa").clicked() {
                                st.renaming = Some((i, name.clone()));
                            }
                            if ui.button("Leć").clicked() {
                                out.push(BookmarkAction::Go(i));
                            }
                        });
                    });
                }
            });
        });
    if !open {
        st.open = false;
        st.renaming = None;
    }
    out
}

pub struct LayerInfo {
    pub id: u32,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub active: bool,
}

#[derive(Default)]
pub struct LayerPanel {
    pub open: bool,
    pub renaming: Option<(u32, String)>,
    /// Layer waiting for a delete confirmation.
    pub confirm_delete: Option<u32>,
}

#[derive(Debug, PartialEq)]
pub enum LayerAction {
    Add,
    Select(u32),
    ToggleVisible(u32),
    Opacity(u32, f32),
    Up(u32),
    Down(u32),
    Rename(u32, String),
    Delete(u32),
}

/// The layers window, top layer first. Tapping a layer's name makes it the one you draw on.
pub fn layers_window(ctx: &egui::Context, st: &mut LayerPanel, layers: &[LayerInfo], safe_top: f32) -> Vec<LayerAction> {
    let mut out = Vec::new();
    if !st.open {
        return out;
    }
    let screen = ctx.content_rect();
    let width = (screen.width() - 16.0).min(460.0);
    let max_h = (screen.height() - safe_top - 140.0).max(200.0);
    let mut open = true;
    egui::Window::new("Warstwy")
        .id(egui::Id::unique("layers"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, [0.0, 8.0 + safe_top])
        .fixed_size([width, 0.0])
        .show(ctx, |ui| {
            ui.set_width(width - 16.0);
            if ui.button("Dodaj warstwę").clicked() {
                out.push(LayerAction::Add);
            }
            ui.label(egui::RichText::new("Rysujesz na zaznaczonej warstwie. Gumka ściera tylko na niej.").small().weak());
            egui::ScrollArea::vertical().scroll_source(egui::scroll_area::ScrollSource::ALL).max_height(max_h).show(ui, |ui| {
                let n = layers.len();
                for (pos, l) in layers.iter().enumerate().rev() {
                    ui.separator();
                    if let Some((id, text)) = st.renaming.as_mut()
                        && *id == l.id
                    {
                        let mut done = false;
                        ui.horizontal(|ui| {
                            let r = ui.add(egui::TextEdit::singleline(text).desired_width(ui.available_width() - 70.0));
                            done = ui.button("OK").clicked() || (r.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter)));
                        });
                        if done {
                            out.push(LayerAction::Rename(l.id, text.clone()));
                            st.renaming = None;
                        }
                        continue;
                    }
                    ui.horizontal(|ui| {
                        let name = egui::RichText::new(&l.name);
                        let name = if l.active { name.strong() } else { name };
                        if ui.add(egui::Button::new(name).selected(l.active)).clicked() {
                            out.push(LayerAction::Select(l.id));
                        }
                        if !l.visible {
                            ui.label(egui::RichText::new("ukryta").small().weak());
                        }
                        let mut o = l.opacity;
                        if ui.add(egui::Slider::new(&mut o, 0.0..=1.0).show_value(false)).on_hover_text("krycie").changed() {
                            out.push(LayerAction::Opacity(l.id, o));
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(if l.visible { "Ukryj" } else { "Pokaż" }).clicked() {
                            out.push(LayerAction::ToggleVisible(l.id));
                        }
                        if ui.add_enabled(pos + 1 < n, egui::Button::new("W górę")).clicked() {
                            out.push(LayerAction::Up(l.id));
                        }
                        if ui.add_enabled(pos > 0, egui::Button::new("W dół")).clicked() {
                            out.push(LayerAction::Down(l.id));
                        }
                        if ui.button("Nazwa").clicked() {
                            st.renaming = Some((l.id, l.name.clone()));
                        }
                        if st.confirm_delete == Some(l.id) {
                            if ui.button(egui::RichText::new("Usuń z kreskami").color(Color32::from_rgb(0xc6, 0x28, 0x28))).clicked() {
                                out.push(LayerAction::Delete(l.id));
                                st.confirm_delete = None;
                            }
                            if ui.button("Nie").clicked() {
                                st.confirm_delete = None;
                            }
                        } else if ui.add_enabled(n > 1, egui::Button::new("Usuń")).clicked() {
                            st.confirm_delete = Some(l.id);
                        }
                    });
                }
            });
        });
    if !open {
        st.open = false;
        st.renaming = None;
        st.confirm_delete = None;
    }
    out
}

#[derive(Debug, PartialEq)]
pub enum SelectionAction {
    Delete,
    Duplicate,
    Recolor,
    Done,
}

/// Actions for the current selection, at the bottom of the screen.
pub fn selection_bar(ctx: &egui::Context, count: usize) -> (Option<SelectionAction>, Rect) {
    let mut out = None;
    let r = egui::Area::new(egui::Id::unique("selection-bar")).anchor(Align2::CENTER_BOTTOM, [0.0, -24.0]).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).corner_radius(CornerRadius::same(12)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Zaznaczone: {count}"));
                if ui.button("Usuń").clicked() {
                    out = Some(SelectionAction::Delete);
                }
                if ui.button("Duplikuj").clicked() {
                    out = Some(SelectionAction::Duplicate);
                }
                if ui.button("Kolor").on_hover_text("Nadaj zaznaczonym kreskom aktualny kolor").clicked() {
                    out = Some(SelectionAction::Recolor);
                }
                if ui.button("Gotowe").clicked() {
                    out = Some(SelectionAction::Done);
                }
            });
        });
    });
    (out, r.response.rect)
}

#[derive(Default)]
pub struct TextPanel {
    pub open: bool,
    /// Screen point where the text goes (its top-left corner).
    pub pos: [f64; 2],
    pub text: String,
}

#[derive(Debug, PartialEq)]
pub enum TextAction {
    Add,
    Cancel,
}

/// Typing the text to place.
pub fn text_window(ctx: &egui::Context, st: &mut TextPanel, size: &mut f32, safe_top: f32) -> Option<TextAction> {
    if !st.open {
        return None;
    }
    let mut out = None;
    let width = (ctx.content_rect().width() - 16.0).min(440.0);
    let mut open = true;
    egui::Window::new("Tekst")
        .id(egui::Id::unique("text"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, [0.0, 8.0 + safe_top])
        .fixed_size([width, 0.0])
        .show(ctx, |ui| {
            ui.set_width(width - 16.0);
            let r = ui.add(egui::TextEdit::multiline(&mut st.text).desired_rows(3).desired_width(f32::INFINITY).hint_text("Wpisz tekst…"));
            if !r.has_focus() && st.text.is_empty() {
                r.request_focus();
            }
            ui.add(egui::Slider::new(size, 8.0..=240.0).logarithmic(true).max_decimals(0).text("rozmiar"));
            ui.horizontal(|ui| {
                if ui.add_enabled(!st.text.trim().is_empty(), egui::Button::new("Dodaj")).clicked() {
                    out = Some(TextAction::Add);
                }
                if ui.button("Anuluj").clicked() {
                    out = Some(TextAction::Cancel);
                }
            });
            ui.label(egui::RichText::new("Tekst jest wektorowy: ostry przy każdym zoomie, w aktualnym kolorze.").small().weak());
        });
    if !open {
        out = Some(TextAction::Cancel);
    }
    out
}

/// While recording: a red button with the time; tapping it stops the recording.
pub fn recording_badge(ctx: &egui::Context, seconds: f64, safe_top: f32) -> (bool, Rect) {
    let mut stop = false;
    // Bottom-right, above everything: the toolbar must never cover it.
    let _ = safe_top;
    let r = egui::Area::new(egui::Id::unique("recording"))
        .order(egui::Order::Foreground)
        .anchor(Align2::RIGHT_BOTTOM, [-12.0, -84.0])
        .show(ctx, |ui| {
        let s = seconds as u64;
        let text = egui::RichText::new(format!("● Nagrywanie {}:{:02} – zakończ", s / 60, s % 60)).color(Color32::WHITE).strong();
        stop = ui
            .add(egui::Button::new(text.size(16.0)).fill(Color32::from_rgb(0xd3, 0x2f, 0x2f)).min_size(egui::vec2(0.0, 48.0)).corner_radius(CornerRadius::same(24)))
            .clicked();
    });
    (stop, r.response.rect)
}

/// A short message near the bottom of the screen.
pub fn toast(ctx: &egui::Context, msg: &str) {
    let max_w = (ctx.content_rect().width() - 48.0).max(160.0);
    egui::Area::new(egui::Id::unique("toast")).anchor(Align2::CENTER_BOTTOM, [0.0, -32.0]).interactable(false).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).corner_radius(CornerRadius::same(12)).show(ui, |ui| {
            ui.set_max_width(max_w);
            ui.label(msg);
        });
    });
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
            egui::ScrollArea::vertical().scroll_source(egui::scroll_area::ScrollSource::ALL).auto_shrink([false, false]).show(ui, |ui| {
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
        let shapes = [None, Some(Shape::Line), Some(Shape::Rectangle), Some(Shape::Ellipse)];
        for (brush, shape) in Brush::ALL.iter().flat_map(|b| shapes.iter().map(move |sh| (*b, *sh))) {
            let mut s = ToolState::default();
            s.set_brush(brush);
            s.shape = shape;
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

    /// Dragging over clickable cells scrolls the list (touch screens send mouse-like events)
    /// and does not count as a click.
    #[test]
    fn drag_scrolls_a_grid_of_clickable_cells() {
        let ctx = egui::Context::default();
        let mut first_cell_y = Vec::new();
        let mut clicked = false;
        let mut t = 0.0;
        let mut run = |events: Vec<egui::Event>, first_cell_y: &mut Vec<f32>, clicked: &mut bool| {
            t += 1.0 / 60.0;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 600.0))),
                time: Some(t),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |root| {
                egui::Window::new("t").fixed_pos([0.0, 0.0]).show(root.ctx(), |ui| {
                egui::ScrollArea::vertical().scroll_source(egui::scroll_area::ScrollSource::ALL).max_height(500.0).show(ui, |ui| {
                    for i in 0..40 {
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(100.0, 100.0), Sense::click());
                        if i == 0 {
                            first_cell_y.push(rect.min.y);
                        }
                        *clicked |= resp.clicked();
                    }
                });
                });
            });
            out.textures_delta.clear();
        };
        // egui hit-tests against the previous frame's widgets: let the list settle first.
        for _ in 0..3 {
            run(vec![], &mut first_cell_y, &mut clicked);
        }
        let p = |y: f32| egui::pos2(50.0, y);
        run(vec![egui::Event::PointerMoved(p(400.0)), egui::Event::PointerButton { pos: p(400.0), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], &mut first_cell_y, &mut clicked);
        for y in (150..400).rev().step_by(25) {
            run(vec![egui::Event::PointerMoved(p(y as f32))], &mut first_cell_y, &mut clicked);
        }
        run(vec![egui::Event::PointerButton { pos: p(150.0), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }, egui::Event::PointerGone], &mut first_cell_y, &mut clicked);
        run(vec![], &mut first_cell_y, &mut clicked);
        let moved = first_cell_y[0] - first_cell_y.last().unwrap();
        assert!(moved > 150.0, "scrolled by {moved}");
        assert!(!clicked, "a drag is not a click");
    }

}
