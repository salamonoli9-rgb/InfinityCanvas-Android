//! Minimal floating toolbar (egui). Kept small and at the top edge so it never covers much.

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pen,
    Eraser,
}

pub struct ToolState {
    pub tool: Tool,
    pub color: Color32,
    /// Pen width in logical points (screen size at drawing time).
    pub width: f32,
    /// Eraser radius in logical points.
    pub eraser: f32,
    pub palette: Vec<Color32>,
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            tool: Tool::Pen,
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
    pub strokes: usize,
}

#[derive(Default, Debug)]
pub struct Actions {
    pub undo: bool,
    pub redo: bool,
    pub home: bool,
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
fn tool_buttons(ui: &mut egui::Ui, s: &mut ToolState) {
    ui.selectable_value(&mut s.tool, Tool::Pen, "Pióro");
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

fn size_slider(ui: &mut egui::Ui, s: &mut ToolState) {
    match s.tool {
        Tool::Pen => {
            ui.add(egui::Slider::new(&mut s.width, 0.5..=64.0).logarithmic(true).max_decimals(1).text("grubość"));
        }
        Tool::Eraser => {
            ui.add(egui::Slider::new(&mut s.eraser, 2.0..=80.0).logarithmic(true).max_decimals(0).text("rozmiar"));
        }
    }
}

fn history_buttons(ui: &mut egui::Ui, info: &Info, a: &mut Actions) {
    a.undo |= ui.add_enabled(info.can_undo, egui::Button::new("Cofnij")).clicked();
    a.redo |= ui.add_enabled(info.can_redo, egui::Button::new("Ponów")).clicked();
    a.home |= ui.button("Początek").on_hover_text("Wróć do początku (Home)").clicked();
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
                    tool_buttons(ui, s);
                    ui.separator();
                    history_buttons(ui, info, &mut a);
                });
                ui.horizontal_wrapped(|ui| colors(ui, s));
                ui.horizontal(|ui| {
                    size_slider(ui, s);
                    ui.separator();
                    zoom_label(ui, info);
                });
            } else {
                ui.horizontal(|ui| {
                    tool_buttons(ui, s);
                    ui.separator();
                    colors(ui, s);
                    ui.separator();
                    size_slider(ui, s);
                    ui.separator();
                    history_buttons(ui, info, &mut a);
                    ui.separator();
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
        let mut s = ToolState::default();
        let info = Info { can_undo: true, can_redo: false, log_zoom: 3.4, strokes: 2 };
        let mut rect = Rect::NOTHING;
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
        assert!(rect.min.x >= 0.0 && rect.max.x <= 360.0, "{rect:?}");
        assert!(rect.min.y >= 24.0, "{rect:?}");
    }
}
