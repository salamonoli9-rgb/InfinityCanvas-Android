//! Minimal floating toolbar (egui). Kept small and at the top edge so it never covers much.

use crate::projects::{self, ProjectInfo, Projects};
use crate::shapes::Shape;
use canvas_core::stroke::Brush;
use std::collections::HashMap;
use crate::i18n::{t, tf};
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
    /// The full menu (under ☰) is open.
    pub bar_open: bool,
    /// Fold the menu away when a stroke starts.
    pub bar_autohide: bool,
    /// What the quick squares next to ☰ do (More → Quick buttons).
    pub quick: [Quick; QUICK_SLOTS],
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
        Brush::Pen => t("Pióro"),
        Brush::Fountain => t("Wieczne"),
        Brush::Dynamic => t("Dynamiczne"),
        Brush::Fixed => t("Stałe"),
        Brush::SoftPencil => t("Ołówek M"),
        Brush::HardPencil => t("Ołówek T"),
        Brush::Marker => t("Marker"),
        Brush::Dotted => t("Kropki"),
        Brush::Fill => t("Wypełnij"),
        Brush::Ballpoint => t("Długopis"),
        Brush::Brush => t("Pędzel"),
        Brush::Highlighter => t("Zakreślacz"),
        Brush::Crayon => t("Kredka"),
        Brush::Charcoal => t("Węgiel"),
        Brush::Airbrush => t("Aerograf"),
        Brush::Watercolor => t("Akwarela"),
        Brush::Spray => t("Spray"),
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
            bar_open: false,
            bar_autohide: true,
            quick: Quick::DEFAULT,
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
    /// Screen edges covered by system bars / cutouts on the left and right (points).
    pub safe_side: [f32; 2],
    /// A stroke is being drawn right now (the toolbar can slide out of the way).
    pub drawing: bool,
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
    /// Open the settings (language, phone / tablet).
    pub settings: bool,
    /// Open the window choosing the quick squares.
    pub quick_settings: bool,
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
    let label = s.shape.map_or(short_name(s.brush), |sh| t(sh.name()));
    let button = egui::Button::new(label).selected(s.tool == Tool::Pen);
    let r = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        ui.set_min_width(250.0);
        let max_h = (ui.ctx().content_rect().height() * 0.62).max(240.0);
        egui::ScrollArea::vertical().scroll_source(egui::scroll_area::ScrollSource::ALL).max_height(max_h).show(ui, |ui| {
            for b in Brush::ALL {
                let selected = s.tool == Tool::Pen && s.brush == b;
                let button = match s.previews.get(&b) {
                    // The sample of how the brush draws, then its name.
                    Some(tex) => egui::Button::image_and_text(egui::Image::new((tex.id(), egui::vec2(96.0, 24.0))), t(b.name())),
                    None => egui::Button::new(t(b.name())),
                };
                if ui.add(button.selected(selected)).clicked() {
                    s.set_brush(b);
                    ui.close();
                }
            }
        });
        ui.separator();
        ui.label(egui::RichText::new(t("Kształt (aktualnym pędzlem)")).small().weak());
        ui.horizontal_wrapped(|ui| {
            if ui.add(egui::Button::new(t("Odręcznie")).selected(s.shape.is_none())).clicked() {
                s.shape = None;
                s.tool = Tool::Pen;
                ui.close();
            }
            for sh in Shape::ALL {
                if ui.add(egui::Button::new(t(sh.name())).selected(s.shape == Some(sh))).clicked() {
                    s.shape = Some(sh);
                    s.tool = Tool::Pen;
                    ui.close();
                }
            }
        });
    });
    r.0.on_hover_text(t("Pędzel"));
    ui.selectable_value(&mut s.tool, Tool::Eraser, t("Gumka"));
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
        Tool::Pen | Tool::Hand | Tool::Select => (&mut s.width, 0.5..=64.0, 1, t("grubość")),
        Tool::Text => (&mut s.text_size, 8.0..=240.0, 0, t("rozmiar tekstu")),
        Tool::Measure => (&mut s.width, 0.5..=64.0, 1, t("grubość")),
        Tool::Nudge => (&mut s.nudge, 8.0..=200.0, 0, t("zasięg")),
        Tool::Eraser => (&mut s.eraser, 4.0..=160.0, 0, t("rozmiar")),
    };
    let slider = egui::Slider::new(value, range).logarithmic(true).max_decimals(decimals);
    ui.add(if label { slider.text(text) } else { slider }).on_hover_text(text);
}

fn history_buttons(ui: &mut egui::Ui, info: &Info, a: &mut Actions) {
    a.undo |= ui.add_enabled(info.can_undo, egui::Button::new(t("Cofnij"))).clicked();
    a.redo |= ui.add_enabled(info.can_redo, egui::Button::new(t("Ponów"))).clicked();
}

fn hand_button(ui: &mut egui::Ui, s: &mut ToolState) {
    ui.selectable_value(&mut s.tool, Tool::Select, t("Zaznacz")).on_hover_text(t("Obrysuj kreski palcem, potem przeciągnij je albo zmień rozmiar uchwytem w rogu"));
    ui.selectable_value(&mut s.tool, Tool::Text, t("Tekst")).on_hover_text(t("Stuknij w płótno, żeby wpisać tekst (wektorowy)"));
    ui.selectable_value(&mut s.tool, Tool::Nudge, t("Wygnij")).on_hover_text(t("Przeciągnij po kreskach, żeby je wygiąć (zasięg – suwak)"));
    ui.selectable_value(&mut s.tool, Tool::Measure, t("Miarka")).on_hover_text(t("Przeciągnij, żeby zmierzyć odległość i kąt"));
    ui.selectable_value(&mut s.tool, Tool::Hand, t("Przesuń"))
        .on_hover_text(t("Jeden palec przesuwa płótno, nic nie rysuje. Dwa stuknięcia i przeciągnięcie w górę/dół – zoom."));
}

fn home_button(ui: &mut egui::Ui, a: &mut Actions) {
    a.home |= ui.button(t("Początek")).on_hover_text(t("Wróć do początku (Home)")).clicked();
}

/// "More": photos, bookmarks, export.
fn photos_button(ui: &mut egui::Ui, a: &mut Actions, s: &mut ToolState) {
    egui::containers::menu::MenuButton::from_button(egui::Button::new(t("Więcej"))).ui(ui, |ui| {
        ui.set_min_width(190.0);
        if ui.checkbox(&mut s.show_grid, t("Siatka (kształty się przyciągają)")).changed() {
            a.grid_changed = true;
        }
        ui.checkbox(&mut s.bar_autohide, t("Chowaj menu podczas rysowania"));
        if ui.button(t("Zdjęcia…")).clicked() {
            a.photos = true;
            ui.close();
        }
        if ui.button(t("Warstwy…")).clicked() {
            a.layers = true;
            ui.close();
        }
        if ui.button(t("Zakładki…")).clicked() {
            a.bookmarks = true;
            ui.close();
        }
        if ui.button(t("Eksport do PNG")).clicked() {
            a.export = true;
            ui.close();
        }
        if ui.button(t("Nagrywaj wideo (MP4)")).on_hover_text(t("Nagrywa płótno – rysowanie, przesuwanie, zoom – bez paska narzędzi")).clicked() {
            a.record = true;
            ui.close();
        }
        if ui.button(t("Szybkie przyciski…")).clicked() {
            a.quick_settings = true;
            ui.close();
        }
        if ui.button(t("Ustawienia…")).clicked() {
            a.settings = true;
            ui.close();
        }
    });
}

fn menu_button(ui: &mut egui::Ui, a: &mut Actions) {
    a.menu |= ui.button(t("Projekty")).on_hover_text(t("Wróć do listy projektów")).clicked();
}

/// Shown only while the view is rotated; a tap straightens it.
fn rotation_button(ui: &mut egui::Ui, info: &Info, a: &mut Actions) {
    let deg = info.rotation.to_degrees().round();
    if deg != 0.0 || info.rotation != 0.0 {
        a.upright |= ui.button(format!("{deg:.0}°")).on_hover_text(t("Obrót płótna – stuknij, aby wyprostować")).clicked();
    }
}

fn zoom_label(ui: &mut egui::Ui, info: &Info) {
    ui.label(egui::RichText::new(zoom_text(info.log_zoom)).monospace())
        .on_hover_text(tf("log2 zoomu: {}\nkresek: {}", &[&format!("{:.2}", info.log_zoom), &info.strokes]));
}

/// What a quick square next to the ☰ menu does (chosen in More → Quick buttons).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quick {
    None,
    Pen,
    Eraser,
    Hand,
    Select,
    Text,
    Measure,
    Nudge,
    Undo,
    Redo,
    Home,
    Grid,
    Color,
    Photos,
    Layers,
}

/// How many quick squares there are (empty ones are not shown).
pub const QUICK_SLOTS: usize = 5;

impl Quick {
    pub const ALL: [Quick; 15] = [
        Quick::None, Quick::Pen, Quick::Eraser, Quick::Hand, Quick::Select, Quick::Text, Quick::Measure, Quick::Nudge,
        Quick::Undo, Quick::Redo, Quick::Home, Quick::Grid, Quick::Color, Quick::Photos, Quick::Layers,
    ];
    pub const DEFAULT: [Quick; QUICK_SLOTS] = [Quick::Pen, Quick::Eraser, Quick::Hand, Quick::Undo, Quick::Redo];

    pub fn name(self) -> &'static str {
        match self {
            Quick::None => t("Puste"),
            Quick::Pen => t("Rysuj"),
            Quick::Eraser => t("Gumka"),
            Quick::Hand => t("Przesuń"),
            Quick::Select => t("Zaznacz"),
            Quick::Text => t("Tekst"),
            Quick::Measure => t("Miarka"),
            Quick::Nudge => t("Wygnij"),
            Quick::Undo => t("Cofnij"),
            Quick::Redo => t("Ponów"),
            Quick::Home => t("Początek"),
            Quick::Grid => t("Siatka"),
            Quick::Color => t("Kolor"),
            Quick::Photos => t("Zdjęcia"),
            Quick::Layers => t("Warstwy"),
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Quick::None => "none",
            Quick::Pen => "pen",
            Quick::Eraser => "eraser",
            Quick::Hand => "hand",
            Quick::Select => "select",
            Quick::Text => "text",
            Quick::Measure => "measure",
            Quick::Nudge => "nudge",
            Quick::Undo => "undo",
            Quick::Redo => "redo",
            Quick::Home => "home",
            Quick::Grid => "grid",
            Quick::Color => "color",
            Quick::Photos => "photos",
            Quick::Layers => "layers",
        }
    }

    pub fn from_code(c: &str) -> Quick {
        Quick::ALL.into_iter().find(|q| q.code() == c.trim()).unwrap_or(Quick::None)
    }

    /// "pen,eraser,…" (saved next to the settings).
    pub fn save_list(list: &[Quick; QUICK_SLOTS]) -> String {
        list.iter().map(|q| q.code()).collect::<Vec<_>>().join(",")
    }

    pub fn load_list(text: &str) -> [Quick; QUICK_SLOTS] {
        let mut out = [Quick::None; QUICK_SLOTS];
        for (i, c) in text.trim().split(',').take(QUICK_SLOTS).enumerate() {
            out[i] = Quick::from_code(c);
        }
        out
    }
}

/// Picture shown in a square: drawn with lines, so it needs no icon font.
#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Menu,
    Close,
    Q(Quick),
}

/// Draws `icon` inside `r` (a square) with colour `c`; `fill` is the colour of the button behind it.
fn draw_icon(p: &egui::Painter, r: Rect, icon: Icon, c: Color32, fill: Color32, current: Color32) {
    let w = r.width();
    let at = |x: f32, y: f32| egui::pos2(r.left() + x * w, r.top() + y * r.height());
    let st = Stroke::new((w * 0.085).max(1.6), c);
    let thin = Stroke::new((w * 0.06).max(1.2), c);
    let line = |pts: &[(f32, f32)], s: Stroke| p.add(egui::Shape::line(pts.iter().map(|&(x, y)| at(x, y)).collect(), s));
    // rotated shape around the centre, coordinates relative to the centre (−0.5…0.5)
    let rot = |pts: &[(f32, f32)], deg: f32| -> Vec<egui::Pos2> {
        let (sn, cs) = deg.to_radians().sin_cos();
        pts.iter().map(|&(x, y)| at(0.5 + x * cs - y * sn, 0.5 + x * sn + y * cs)).collect()
    };
    match icon {
        Icon::Menu => {
            for y in [0.22, 0.5, 0.78] {
                line(&[(0.12, y), (0.88, y)], st);
            }
        }
        Icon::Close => {
            line(&[(0.2, 0.2), (0.8, 0.8)], st);
            line(&[(0.8, 0.2), (0.2, 0.8)], st);
        }
        Icon::Q(q) => match q {
            Quick::None => {}
            Quick::Pen => {
                let body = rot(&[(-0.26, -0.11), (0.3, -0.11), (0.3, 0.11), (-0.26, 0.11)], 135.0);
                p.add(egui::Shape::convex_polygon(body, fill, thin));
                let tip = rot(&[(-0.26, -0.11), (-0.44, 0.0), (-0.26, 0.11)], 135.0);
                p.add(egui::Shape::convex_polygon(tip, c, thin));
                let cap = rot(&[(0.3, -0.11), (0.42, -0.11), (0.42, 0.11), (0.3, 0.11)], 135.0);
                p.add(egui::Shape::convex_polygon(cap, c, thin));
            }
            Quick::Eraser => {
                let body = rot(&[(-0.38, -0.17), (0.38, -0.17), (0.38, 0.17), (-0.38, 0.17)], -40.0);
                p.add(egui::Shape::convex_polygon(body, fill, thin));
                let half = rot(&[(-0.38, -0.17), (-0.02, -0.17), (-0.02, 0.17), (-0.38, 0.17)], -40.0);
                p.add(egui::Shape::convex_polygon(half, c, thin));
                line(&[(0.18, 0.9), (0.85, 0.9)], thin);
            }
            Quick::Hand => {
                // finger: palm, raised index finger, thumb, and two little swipe marks
                let palm = Rect::from_min_max(at(0.27, 0.46), at(0.79, 0.94));
                p.rect(palm, CornerRadius::same((w * 0.14) as u8), fill, thin, egui::StrokeKind::Middle);
                let thumb = rot(&[(-0.36, 0.06), (-0.2, -0.04), (-0.13, 0.06), (-0.27, 0.17)], 0.0);
                p.add(egui::Shape::convex_polygon(thumb, fill, thin));
                let finger = Rect::from_min_max(at(0.4, 0.06), at(0.58, 0.62));
                p.rect(finger, CornerRadius::same((w * 0.09) as u8), fill, thin, egui::StrokeKind::Middle);
                line(&[(0.62, 0.58), (0.62, 0.66)], thin);
                line(&[(0.7, 0.6), (0.7, 0.68)], thin);
                line(&[(0.22, 0.14), (0.3, 0.2)], thin);
                line(&[(0.76, 0.14), (0.68, 0.2)], thin);
            }
            Quick::Select => {
                let n = 16;
                for i in 0..n {
                    if i % 2 == 0 {
                        let a0 = i as f32 / n as f32 * std::f32::consts::TAU;
                        let a1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
                        line(&[(0.5 + 0.36 * a0.cos(), 0.44 + 0.3 * a0.sin()), (0.5 + 0.36 * a1.cos(), 0.44 + 0.3 * a1.sin())], thin);
                    }
                }
                line(&[(0.36, 0.7), (0.3, 0.86), (0.4, 0.92)], thin);
            }
            Quick::Text => {
                line(&[(0.18, 0.16), (0.82, 0.16)], st);
                line(&[(0.5, 0.16), (0.5, 0.86)], st);
                line(&[(0.38, 0.86), (0.62, 0.86)], thin);
            }
            Quick::Measure => {
                let body = rot(&[(-0.46, -0.15), (0.46, -0.15), (0.46, 0.15), (-0.46, 0.15)], -35.0);
                p.add(egui::Shape::convex_polygon(body, fill, thin));
                for (i, x) in [-0.3f32, -0.15, 0.0, 0.15, 0.3].iter().enumerate() {
                    let len = if i % 2 == 0 { 0.13 } else { 0.07 };
                    let t = rot(&[(*x, -0.15), (*x, -0.15 + len)], -35.0);
                    p.add(egui::Shape::line(t, thin));
                }
            }
            Quick::Nudge => {
                let pts: Vec<(f32, f32)> = (0..=20).map(|i| {
                    let x = i as f32 / 20.0;
                    (0.1 + 0.8 * x, 0.55 - 0.2 * (x * std::f32::consts::PI).sin())
                }).collect();
                line(&pts, st);
                p.circle_stroke(at(0.5, 0.35), w * 0.16, thin);
            }
            Quick::Undo | Quick::Redo => {
                let m = if q == Quick::Undo { 1.0 } else { -1.0 };
                let fx = |x: f32| if m > 0.0 { x } else { 1.0 - x };
                let arc: Vec<(f32, f32)> = (0..=14).map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 14.0);
                    (fx(0.55 + 0.3 * a.cos()), 0.62 + 0.3 * a.sin())
                }).collect();
                line(&arc, st);
                line(&[(fx(0.25), 0.62), (fx(0.25), 0.82)], st);
                line(&[(fx(0.25), 0.82), (fx(0.08), 0.62)], st);
                let _ = line(&[(fx(0.25), 0.82), (fx(0.42), 0.62)], st);
                line(&[(fx(0.85), 0.62), (fx(0.85), 0.78)], st);
            }
            Quick::Home => {
                line(&[(0.1, 0.48), (0.5, 0.12), (0.9, 0.48)], st);
                line(&[(0.22, 0.38), (0.22, 0.88), (0.78, 0.88), (0.78, 0.38)], st);
                line(&[(0.42, 0.88), (0.42, 0.62), (0.58, 0.62), (0.58, 0.88)], thin);
            }
            Quick::Grid => {
                p.rect_stroke(Rect::from_min_max(at(0.12, 0.12), at(0.88, 0.88)), CornerRadius::same(2), thin, egui::StrokeKind::Middle);
                for v in [0.37, 0.63] {
                    line(&[(v, 0.12), (v, 0.88)], thin);
                    line(&[(0.12, v), (0.88, v)], thin);
                }
            }
            Quick::Color => {
                p.circle_filled(at(0.5, 0.5), w * 0.38, current);
                p.circle_stroke(at(0.5, 0.5), w * 0.38, Stroke::new(1.5, Color32::from_gray(120)));
            }
            Quick::Photos => {
                p.rect_stroke(Rect::from_min_max(at(0.08, 0.16), at(0.92, 0.84)), CornerRadius::same(3), thin, egui::StrokeKind::Middle);
                line(&[(0.14, 0.78), (0.4, 0.46), (0.58, 0.66), (0.7, 0.54), (0.86, 0.78)], thin);
                p.circle_filled(at(0.68, 0.34), w * 0.07, c);
            }
            Quick::Layers => {
                for dy in [0.14f32, -0.1] {
                    let d = [(0.5, 0.34 + dy), (0.9, 0.52 + dy), (0.5, 0.7 + dy), (0.1, 0.52 + dy)];
                    let pts: Vec<egui::Pos2> = d.iter().map(|&(x, y)| at(x, y)).collect();
                    p.add(egui::Shape::convex_polygon(pts, fill, thin));
                }
            }
        },
    }
}

/// A square button with a drawn icon.
fn square(ui: &mut egui::Ui, side: f32, icon: Icon, selected: bool, enabled: bool, current: Color32) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(side), sense);
    let v = ui.style().interact_selectable(&resp, selected);
    let fill = if selected { ui.visuals().selection.bg_fill } else { v.weak_bg_fill };
    ui.painter().rect(rect, CornerRadius::same(9), fill, v.bg_stroke, egui::StrokeKind::Inside);
    let mut c = if selected { ui.visuals().selection.stroke.color } else { v.fg_stroke.color };
    if !enabled {
        c = c.gamma_multiply(0.35);
    }
    draw_icon(ui.painter(), rect.shrink(side * 0.22), icon, c, fill, current);
    resp
}

fn quick_button(ui: &mut egui::Ui, side: f32, q: Quick, s: &mut ToolState, info: &Info, a: &mut Actions) {
    let tool = |q: Quick| match q {
        Quick::Pen => Some(Tool::Pen),
        Quick::Eraser => Some(Tool::Eraser),
        Quick::Hand => Some(Tool::Hand),
        Quick::Select => Some(Tool::Select),
        Quick::Text => Some(Tool::Text),
        Quick::Measure => Some(Tool::Measure),
        Quick::Nudge => Some(Tool::Nudge),
        _ => None,
    };
    let selected = tool(q).is_some_and(|t| s.tool == t) || (q == Quick::Grid && s.show_grid);
    let enabled = match q {
        Quick::Undo => info.can_undo,
        Quick::Redo => info.can_redo,
        _ => true,
    };
    let resp = square(ui, side, Icon::Q(q), selected, enabled, s.color).on_hover_text(q.name());
    if q == Quick::Color {
        egui::Popup::menu(&resp).show(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width(side * 5.5);
                colors(ui, s);
            });
        });
        return;
    }
    if !resp.clicked() {
        return;
    }
    if let Some(t) = tool(q) {
        s.tool = t;
    }
    match q {
        Quick::Undo => a.undo = true,
        Quick::Redo => a.redo = true,
        Quick::Home => a.home = true,
        Quick::Grid => {
            s.show_grid = !s.show_grid;
            a.grid_changed = true;
        }
        Quick::Photos => a.photos = true,
        Quick::Layers => a.layers = true,
        _ => {}
    }
}

/// More → Quick buttons: what each square next to the ☰ menu does. Returns (close, changed).
pub fn quick_window(ctx: &egui::Context, s: &mut ToolState, safe_top: f32) -> (bool, bool) {
    let mut close = false;
    let mut changed = false;
    egui::Window::new(t("Szybkie przyciski"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, [0.0, 70.0 + safe_top])
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(t("Wybierz, co pokazuje każdy kwadracik obok menu.")).small().weak());
            ui.add_space(4.0);
            let side = ui.spacing().interact_size.y;
            for i in 0..QUICK_SLOTS {
                ui.horizontal(|ui| {
                    square(ui, side, Icon::Q(s.quick[i]), false, true, s.color);
                    let before = s.quick[i];
                    egui::ComboBox::from_id_salt(format!("quick-slot-{i}"))
                        .selected_text(tf("Przycisk {}", &[&(i + 1)]) + ": " + s.quick[i].name())
                        .width(ui.available_width().min(240.0))
                        .show_ui(ui, |ui| {
                            for q in Quick::ALL {
                                ui.selectable_value(&mut s.quick[i], q, q.name());
                            }
                        });
                    changed |= s.quick[i] != before;
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(t("Gotowe")).clicked() {
                    close = true;
                }
                if ui.button(t("Domyślne")).clicked() {
                    s.quick = Quick::DEFAULT;
                    changed = true;
                }
            });
        });
    (close, changed)
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

    // The menu folds away by itself when a stroke starts (More → can be switched off).
    if info.drawing && s.bar_autohide {
        s.bar_open = false;
    }

    let [left, right] = info.safe_side;
    let screen_w = ctx.content_rect().width() - left - right;
    let frame = egui::Frame::popup(&ctx.global_style()).corner_radius(CornerRadius::same(12));
    let inner_w = (screen_w - 16.0 - frame.total_margin().sum().x).max(150.0);
    let compact = inner_w < 760.0;

    // Top row, always visible: ☰ menu + the quick squares.
    let top = egui::Area::new(egui::Id::unique("toolbar-top")).anchor(Align2::LEFT_TOP, [8.0 + left, 8.0 + safe_top]).show(ctx, |ui| {
        frame.inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let side = (ui.spacing().interact_size.y * 1.15).round();
                let icon = if s.bar_open { Icon::Close } else { Icon::Menu };
                let tip = if s.bar_open { t("Schowaj pasek narzędzi") } else { t("Pokaż pasek narzędzi") };
                if square(ui, side, icon, s.bar_open, true, s.color).on_hover_text(tip).clicked() {
                    s.bar_open = !s.bar_open;
                }
                let quick = s.quick;
                for q in quick {
                    if q != Quick::None {
                        quick_button(ui, side, q, s, info, &mut a);
                    }
                }
            });
        });
    });
    rects.push(top.response.rect);

    // The full menu slides down under the top row.
    let shown = ctx.animate_bool_with_time(egui::Id::unique("toolbar-shown"), s.bar_open, 0.16);
    if shown > 0.001 {
        let y = top.response.rect.bottom() + 6.0 - 24.0 * (1.0 - shown);
        let resp = egui::Area::new(egui::Id::unique("toolbar")).anchor(Align2::LEFT_TOP, [8.0 + left, y]).show(ctx, |ui| {
            ui.multiply_opacity(shown);
            frame.show(ui, |ui| {
                ui.set_max_width(inner_w);
                // Rows wrap when a language's words are long (German, Russian…).
                if compact {
                    ui.horizontal_wrapped(|ui| {
                        menu_button(ui, &mut a);
                        tool_buttons(ui, s);
                        history_buttons(ui, info, &mut a);
                    });
                    ui.horizontal_wrapped(|ui| {
                        colors(ui, s);
                        hand_button(ui, s);
                        photos_button(ui, &mut a, s);
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().slider_width = ui.spacing().slider_width.min(90.0);
                        size_slider(ui, s, false);
                        ui.separator();
                        home_button(ui, &mut a);
                        rotation_button(ui, info, &mut a);
                        zoom_label(ui, info);
                    });
                } else {
                    ui.horizontal_wrapped(|ui| {
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
    }
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
        t("Dodaj zdjęcie")
    } else if st.picking {
        t("Wybierz zdjęcie")
    } else {
        t("Zdjęcia")
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
                ui.label(egui::RichText::new(crate::i18n::t_dyn(m)).italics());
            }
            if st.choosing {
                ui.horizontal(|ui| {
                    if let Some(img) = pending_preview {
                        let tex = mini_texture(ui.ctx(), &mut st.thumbs, "pending-preview".into(), img, 160);
                        mini_image(ui, &tex, 96.0);
                    }
                    ui.label(t("Jak dodać to zdjęcie?"));
                });
                ui.add_space(4.0);
                if ui.button(t("Do odrysowania – zdjęcie pod rysunkiem")).clicked() {
                    out.push(PhotoAction::UseAsReference);
                }
                ui.separator();
                ui.label(t("Zamień na rysunek (wektorowy, można go potem edytować):"));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut st.colored, true, t("Kolorowy"));
                    ui.selectable_value(&mut st.colored, false, t("Same kontury"));
                });
                ui.add(egui::Slider::new(&mut st.detail, 0.0..=1.0).show_value(false).text(t("szczegóły")));
                if st.colored {
                    ui.checkbox(&mut st.outlines, t("kontury na wierzchu (aktualnym pędzlem i kolorem)"));
                } else {
                    ui.label(egui::RichText::new(t("Kontury aktualnym pędzlem, kolorem i grubością.")).small().weak());
                }
                ui.checkbox(&mut st.keep_photo, t("zostaw blade zdjęcie pod spodem"));
                ui.horizontal(|ui| {
                    if ui.button(t("Zamień na rysunek")).clicked() {
                        out.push(PhotoAction::Convert);
                    }
                    if ui.button(t("Anuluj")).clicked() {
                        out.push(PhotoAction::CancelChoice);
                    }
                });
                return;
            }
            if st.picking {
                picker(ui, st, lib, max_h - 120.0, &mut out);
                return;
            }
            if ui.button(t("Dodaj zdjęcie")).clicked() {
                out.push(PhotoAction::StartPicking);
            }
            if photos.is_empty() {
                ui.label(egui::RichText::new(t("Dodane zdjęcie leży pod rysunkiem – możesz je odrysować.")).weak());
            }
            for (i, p) in photos.iter().enumerate() {
                ui.separator();
                ui.horizontal(|ui| {
                    if let Some(img) = p.rgba {
                        let tex = mini_texture(ui.ctx(), &mut st.thumbs, format!("photo:{}", p.id), img, 128);
                        mini_image(ui, &tex, 56.0);
                    }
                    ui.label(tf("Zdjęcie {}", &[&(i + 1)]));
                    let mut o = p.opacity;
                    if ui.add(egui::Slider::new(&mut o, 0.05..=1.0).show_value(false).text(t("krycie"))).changed() {
                        out.push(PhotoAction::Opacity(p.id, o));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button(t("Przesuń / skaluj")).clicked() {
                        out.push(PhotoAction::Edit(p.id));
                    }
                    if ui.button(if p.visible { t("Ukryj") } else { t("Pokaż") }).clicked() {
                        out.push(PhotoAction::ToggleVisible(p.id));
                    }
                    if ui.button(t("Usuń")).clicked() {
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
    if ui.button(t("Wróć")).clicked() {
        st.picking = false;
        return;
    }
    let Some(lib) = lib else {
        ui.label(t("Wybieranie zdjęć nie jest dostępne na tym urządzeniu."));
        return;
    };
    if lib.busy {
        ui.label(t("Wczytywanie zdjęcia…"));
    }
    match &lib.entries {
        None => {
            ui.label(t("Szukam zdjęć…"));
        }
        Some(Err(e)) => {
            ui.label(crate::i18n::t_dyn(e));
            ui.horizontal(|ui| {
                if ui.button(t("Zezwól na dostęp do zdjęć")).clicked() {
                    out.push(PhotoAction::RequestAccess);
                }
                if ui.button(t("Odśwież")).clicked() {
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
    egui::Window::new(t("Zakładki"))
        .id(egui::Id::unique("bookmarks"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, [0.0, 8.0 + safe_top])
        .fixed_size([width, 0.0])
        .show(ctx, |ui| {
            ui.set_width(width - 16.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(t("Dodaj zakładkę (obecny widok)")).clicked() {
                    out.push(BookmarkAction::Add);
                }
                if ui
                    .add_enabled(names.len() >= 2, egui::Button::new(t("Wideo MP4")))
                    .on_hover_text(t("Film z przelotem przez zakładki po kolei"))
                    .on_disabled_hover_text(t("Potrzebne są co najmniej 2 zakładki"))
                    .clicked()
                {
                    out.push(BookmarkAction::ExportVideo);
                }
            });
            if names.is_empty() {
                ui.label(egui::RichText::new(t("Zakładka zapamiętuje miejsce, przybliżenie i obrót. Stuknij ją, żeby tam przelecieć.")).weak());
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
                            done = ui.button(t("OK")).clicked() || (r.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter)));
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
                            if ui.button(t("Usuń")).clicked() {
                                out.push(BookmarkAction::Delete(i));
                            }
                            if ui.button(t("Nazwa")).clicked() {
                                st.renaming = Some((i, name.clone()));
                            }
                            if ui.button(t("Leć")).clicked() {
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
    egui::Window::new(t("Warstwy"))
        .id(egui::Id::unique("layers"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, [0.0, 8.0 + safe_top])
        .fixed_size([width, 0.0])
        .show(ctx, |ui| {
            ui.set_width(width - 16.0);
            if ui.button(t("Dodaj warstwę")).clicked() {
                out.push(LayerAction::Add);
            }
            ui.label(egui::RichText::new(t("Rysujesz na zaznaczonej warstwie. Gumka ściera tylko na niej.")).small().weak());
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
                            done = ui.button(t("OK")).clicked() || (r.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter)));
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
                            ui.label(egui::RichText::new(t("ukryta")).small().weak());
                        }
                        let mut o = l.opacity;
                        if ui.add(egui::Slider::new(&mut o, 0.0..=1.0).show_value(false)).on_hover_text(t("krycie")).changed() {
                            out.push(LayerAction::Opacity(l.id, o));
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(if l.visible { t("Ukryj") } else { t("Pokaż") }).clicked() {
                            out.push(LayerAction::ToggleVisible(l.id));
                        }
                        if ui.add_enabled(pos + 1 < n, egui::Button::new(t("W górę"))).clicked() {
                            out.push(LayerAction::Up(l.id));
                        }
                        if ui.add_enabled(pos > 0, egui::Button::new(t("W dół"))).clicked() {
                            out.push(LayerAction::Down(l.id));
                        }
                        if ui.button(t("Nazwa")).clicked() {
                            st.renaming = Some((l.id, l.name.clone()));
                        }
                        if st.confirm_delete == Some(l.id) {
                            if ui.button(egui::RichText::new(t("Usuń z kreskami")).color(Color32::from_rgb(0xc6, 0x28, 0x28))).clicked() {
                                out.push(LayerAction::Delete(l.id));
                                st.confirm_delete = None;
                            }
                            if ui.button(t("Nie")).clicked() {
                                st.confirm_delete = None;
                            }
                        } else if ui.add_enabled(n > 1, egui::Button::new(t("Usuń"))).clicked() {
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
                ui.label(tf("Zaznaczone: {}", &[&count]));
                if ui.button(t("Usuń")).clicked() {
                    out = Some(SelectionAction::Delete);
                }
                if ui.button(t("Duplikuj")).clicked() {
                    out = Some(SelectionAction::Duplicate);
                }
                if ui.button(t("Kolor")).on_hover_text(t("Nadaj zaznaczonym kreskom aktualny kolor")).clicked() {
                    out = Some(SelectionAction::Recolor);
                }
                if ui.button(t("Gotowe")).clicked() {
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
    egui::Window::new(t("Tekst"))
        .id(egui::Id::unique("text"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, [0.0, 8.0 + safe_top])
        .fixed_size([width, 0.0])
        .show(ctx, |ui| {
            ui.set_width(width - 16.0);
            let r = ui.add(egui::TextEdit::multiline(&mut st.text).desired_rows(3).desired_width(f32::INFINITY).hint_text(t("Wpisz tekst…")));
            if !r.has_focus() && st.text.is_empty() {
                r.request_focus();
            }
            ui.add(egui::Slider::new(size, 8.0..=240.0).logarithmic(true).max_decimals(0).text(t("rozmiar")));
            ui.horizontal(|ui| {
                if ui.add_enabled(!st.text.trim().is_empty(), egui::Button::new(t("Dodaj"))).clicked() {
                    out = Some(TextAction::Add);
                }
                if ui.button(t("Anuluj")).clicked() {
                    out = Some(TextAction::Cancel);
                }
            });
            ui.label(egui::RichText::new(t("Tekst jest wektorowy: ostry przy każdym zoomie, w aktualnym kolorze.")).small().weak());
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
        let text = egui::RichText::new(tf("● Nagrywanie {} – zakończ", &[&format!("{}:{:02}", s / 60, s % 60)])).color(Color32::WHITE).strong();
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
                    ui.label(t("Przesuń zdjęcie palcem, dwoma palcami skaluj i obracaj"));
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
    Settings,
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
                ui.label(egui::RichText::new(t("Projekty")).size(28.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(t("Ustawienia…")).clicked() {
                        out.push(GalleryAction::Settings);
                    }
                });
            });
            if let Some(m) = &st.message {
                ui.label(egui::RichText::new(crate::i18n::t_dyn(m)).italics().weak());
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
                                        painter.text(rect.center() + egui::vec2(0.0, 30.0), Align2::CENTER_CENTER, t("Nowa kartka"), egui::FontId::proportional(16.0), Color32::from_gray(70));
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
                                        if ui.add_enabled(!dialog_open, egui::Button::new("•••")).on_hover_text(t("Opcje")).clicked() {
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
        egui::Window::new(t("Opcje projektu"))
            .id(egui::Id::unique("project-options"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, [0.0, safe_top + 80.0])
            .fixed_size([width, 0.0])
            .show(&ctx, |ui| {
                ui.set_width(width - 16.0);
                ui.label(egui::RichText::new(t("Nazwa")).small().weak());
                let r = ui.add(egui::TextEdit::singleline(&mut st.name_edit).desired_width(f32::INFINITY));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let changed = !st.name_edit.trim().is_empty() && st.name_edit.trim() != name;
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(changed, egui::Button::new(t("Zmień nazwę"))).clicked() || (enter && changed) {
                        out.push(GalleryAction::Rename { from: name.clone(), to: st.name_edit.clone() });
                    }
                    if ui.button(t("Duplikuj")).clicked() {
                        out.push(GalleryAction::Duplicate(name.clone()));
                    }
                    if st.confirm_delete {
                        if ui.button(egui::RichText::new(t("Usuń na zawsze")).color(Color32::from_rgb(0xc6, 0x28, 0x28))).clicked() {
                            out.push(GalleryAction::Delete(name.clone()));
                        }
                        if ui.button(t("Nie usuwaj")).clicked() {
                            st.confirm_delete = false;
                        }
                    } else if ui.button(t("Usuń")).clicked() {
                        st.confirm_delete = true;
                    }
                });
                if let Some(m) = &st.message {
                    ui.label(egui::RichText::new(crate::i18n::t_dyn(m)).italics());
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
        let info = Info { can_undo: true, can_redo: false, log_zoom: -33.4, rotation: -2.5, strokes: 2, safe_side: [0.0, 0.0], drawing: false };
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

    /// The menu opens from ☰ (a real tap, like on Android), folds away when drawing starts,
    /// and the quick squares sit next to ☰.
    #[test]
    fn menu_opens_from_the_hamburger_and_hides_while_drawing() {
        let ctx = egui::Context::default();
        let mut s = ToolState::default();
        let mut info = Info { can_undo: true, can_redo: false, log_zoom: 0.0, rotation: 0.0, strokes: 0, safe_side: [0.0, 0.0], drawing: false };
        let mut time = 0.0;
        let mut frame = |s: &mut ToolState, info: &Info, events: Vec<egui::Event>| {
            time += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 800.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut res = (Actions::default(), Vec::new());
            let mut out = ctx.run_ui(raw, |ui| res = toolbar(ui.ctx(), s, info, 24.0));
            out.textures_delta.clear();
            res
        };
        let tap = |p: egui::Pos2| {
            vec![
                vec![egui::Event::PointerMoved(p), egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }],
                vec![egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }, egui::Event::PointerGone],
            ]
        };
        let mut r = (Actions::default(), Vec::new());
        for _ in 0..6 { r = frame(&mut s, &info, vec![]); }
        assert!(!s.bar_open, "the menu starts folded");
        assert_eq!(r.1.len(), 1, "only the top row: {:?}", r.1);
        let top = r.1[0];
        assert!(top.min.y >= 24.0 && top.width() <= 360.0 && top.height() < 70.0, "{top:?}");
        // ☰ is the first square
        let menu = top.min + egui::vec2(24.0, top.height() * 0.5);
        for ev in tap(menu) { frame(&mut s, &info, ev); }
        assert!(s.bar_open, "tap on ☰ at {menu:?} in {top:?}");
        for _ in 0..8 { r = frame(&mut s, &info, vec![]); }
        assert_eq!(r.1.len(), 2, "{:?}", r.1);
        assert!(r.1[1].min.y > top.max.y && r.1[1].height() > 60.0, "menu under the row: {:?}", r.1);
        // the quick squares: the 4th one (undo) undoes
        let side = (ctx.global_style().spacing.interact_size.y * 1.15).round();
        let x0 = top.min.x + (top.width() - (side + 6.0) * 6.0 + 6.0) * 0.5;   // squares are centred in the frame
        let undo = egui::pos2(x0 + (side + 6.0) * 4.0 + side * 0.5, top.center().y);
        let mut undone = false;
        for ev in tap(undo) { undone |= frame(&mut s, &info, ev).0.undo; }
        assert!(undone, "undo square at {undo:?}");
        // eraser square (2nd quick = 3rd square) picks the eraser
        let eraser = egui::pos2(x0 + (side + 6.0) * 2.0 + side * 0.5, top.center().y);
        for ev in tap(eraser) { frame(&mut s, &info, ev); }
        assert_eq!(s.tool, Tool::Eraser);
        // drawing folds the menu
        info.drawing = true;
        for _ in 0..8 { r = frame(&mut s, &info, vec![]); }
        assert!(!s.bar_open && r.1.len() == 1, "{:?}", r.1);
        // quick list survives saving
        let saved = Quick::save_list(&s.quick);
        assert_eq!(Quick::load_list(&saved), s.quick);
        assert_eq!(Quick::load_list("hand,nonsense"), [Quick::Hand, Quick::None, Quick::None, Quick::None, Quick::None]);
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

/// What the settings window asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    Save,
    Cancel,
}

/// Language and phone / tablet. On the first start (`first_run`) it greets the user and
/// can't be cancelled. The language list switches the window's own texts at once.
pub fn settings_window(ctx: &egui::Context, draft: &mut crate::settings::Settings, first_run: bool) -> Option<SettingsAction> {
    use crate::i18n::{Lang, t_in};
    use crate::settings::Device;
    let lang = draft.lang;
    let tr = |pl: &'static str| t_in(lang, pl);
    let mut out = None;
    let max_w = (ctx.content_rect().width() - 64.0).clamp(200.0, 460.0);
    egui::Modal::new(egui::Id::unique("settings")).show(ctx, |ui| {
            ui.heading(if first_run { tr("Witaj!") } else { tr("Ustawienia") });
            ui.add_space(4.0);
            ui.set_width(max_w);
            if first_run {
                ui.label(tr("Wybierz język i urządzenie. Możesz to zmienić później w Więcej → Ustawienia."));
                ui.add_space(6.0);
            }
            ui.label(egui::RichText::new(tr("Język")).strong());
            egui::Grid::new("langs").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                for (i, l) in Lang::ALL.into_iter().enumerate() {
                    let b = egui::Button::new(l.native_name()).selected(draft.lang == l).min_size(egui::vec2(max_w * 0.5 - 8.0, 0.0));
                    if ui.add(b).clicked() {
                        draft.lang = l;
                    }
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
            ui.add_space(10.0);
            ui.label(egui::RichText::new(tr("Urządzenie")).strong());
            for (d, name, about) in [
                (Device::Phone, "Telefon", "Telefon: narzędzia w kilku rzędach, wygodne na wąskim ekranie."),
                (Device::Tablet, "Tablet", "Tablet: większe przyciski i tekst, narzędzia w jednym rzędzie na szerokim ekranie."),
            ] {
                let b = egui::Button::new(egui::RichText::new(tr(name)).strong()).selected(draft.device == d).min_size(egui::vec2(max_w, 0.0));
                if ui.add(b).clicked() {
                    draft.device = d;
                }
                ui.label(egui::RichText::new(tr(about)).small().weak());
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(egui::RichText::new(if first_run { tr("Zaczynamy") } else { tr("Gotowe") }).strong()).clicked() {
                    out = Some(SettingsAction::Save);
                }
                if !first_run && ui.button(tr("Anuluj")).clicked() {
                    out = Some(SettingsAction::Cancel);
                }
            });
        });
    out
}
