//! Every interface language must fit a narrow phone (360 pt) and, with the bigger tablet
//! style, a tablet in portrait. (Own test binary: the language is a process-wide setting.)

use canvas_core::stroke::Brush;
use egui::{Pos2, Rect, vec2};
use infinite_canvas::i18n::{self, Lang};
use infinite_canvas::settings::{Device, Settings};
use infinite_canvas::ui::{self, Info, ToolState};

fn styled(k: f32) -> egui::Context {
    // Same as the app's Android touch style, scaled.
    let ctx = egui::Context::default();
    ctx.all_styles_mut(|st| {
        let d = egui::Style::default();
        st.text_styles = d.text_styles.iter().map(|(n, f)| (n.clone(), egui::FontId::new(f.size * k, f.family.clone()))).collect();
        st.spacing.interact_size = vec2(40.0, 34.0) * k;
        st.spacing.item_spacing = vec2(8.0, 6.0) * k;
        st.spacing.button_padding = vec2(8.0, 6.0) * k;
        st.spacing.slider_width = 120.0 * k;
    });
    ctx
}

fn run(ctx: &egui::Context, w: f32, mut f: impl FnMut(&egui::Context)) {
    for _ in 0..4 {
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w, 900.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| f(ui.ctx()));
        out.textures_delta.clear();
    }
}

#[test]
fn every_language_fits() {
    let info = Info { can_undo: true, can_redo: true, log_zoom: -33.4, rotation: -2.5, strokes: 2, safe_side: [0.0, 0.0], drawing: false };
    for (device, w) in [(Device::Phone, 360.0), (Device::Tablet, 600.0)] {
        for lang in Lang::ALL {
            i18n::set(lang);
            let ctx = styled(device.ui_scale());
            for brush in Brush::ALL {
                let mut s = ToolState::default();
                s.set_brush(brush);
                s.bar_open = true;               // the open menu (under ☰) and the row of squares
                let mut rects = Vec::new();
                run(&ctx, w, |c| rects = ui::toolbar(c, &mut s, &info, 24.0).1);
                for rect in rects.iter().filter(|r| **r != Rect::EVERYTHING) {
                    assert!(rect.min.x >= 0.0 && rect.max.x <= w, "{device:?} {lang:?} {brush:?}: {rects:?}");
                }
                assert!(rects.len() >= 2, "{device:?} {lang:?}: menu not shown {rects:?}");
            }
            let mut rect = Rect::NOTHING;
            run(&ctx, w, |c| rect = ui::selection_bar(c, 12).1);
            assert!(rect.min.x >= 0.0 && rect.max.x <= w, "selection bar {device:?} {lang:?}: {rect:?}");
            run(&ctx, w, |c| rect = ui::recording_badge(c, 75.0, 24.0).1);
            assert!(rect.min.x >= 0.0 && rect.max.x <= w, "recording {device:?} {lang:?}: {rect:?}");
            // The welcome window, with the texts of the language picked in it.
            let mut draft = Settings { lang, device };
            run(&ctx, w, |c| {
                ui::settings_window(c, &mut draft, true);
            });
            let used = ctx.globally_used_rect();
            assert!(used.max.x <= w + 0.5 && used.max.y <= 900.0, "welcome {device:?} {lang:?}: {used:?}");
        }
    }
    i18n::set(Lang::Pl);
}

#[test]
fn texts_change_with_the_language() {
    // (`t_in` doesn't touch the process-wide language.)
    assert_eq!(i18n::t_in(Lang::De, "Gumka"), "Radierer");
    assert_eq!(i18n::t_in(Lang::Ru, "Cofnij"), "Отменить");
    for b in Brush::ALL {
        for lang in Lang::ALL {
            if lang != Lang::Pl && b.name() != "Marker" && b.name() != "Spray" && b.name() != "Aerograf" {
                assert_ne!(i18n::t_in(lang, b.name()), b.name(), "{lang:?} {b:?}: no translation");
            }
        }
    }
}
