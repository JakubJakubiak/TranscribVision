//! Motyw i własne kontrolki: ciemny, nowoczesny wygląd (karty, gradienty, przełączniki).

use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2, epaint};

pub const BG: Color32 = Color32::from_rgb(10, 10, 15);
pub const PANEL: Color32 = Color32::from_rgb(16, 16, 23);
pub const CARD: Color32 = Color32::from_rgb(22, 22, 31);
pub const CARD_HI: Color32 = Color32::from_rgb(31, 31, 43);
pub const BORDER: Color32 = Color32::from_rgb(40, 40, 54);
pub const TEXT: Color32 = Color32::from_rgb(236, 236, 242);
pub const MUTED: Color32 = Color32::from_rgb(138, 138, 156);
pub const ACCENT: Color32 = Color32::from_rgb(139, 92, 246);
pub const ACCENT2: Color32 = Color32::from_rgb(236, 72, 153);
pub const DANGER: Color32 = Color32::from_rgb(239, 68, 68);
pub const GOOD: Color32 = Color32::from_rgb(52, 211, 153);

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}

pub fn rgb(c: [u8; 4]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Czcionki (San Francisco z systemu + zapasowa z symbolami) i styl.
pub fn install(ctx: &egui::Context) -> TextureHandle {
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |name: &str, path: &str, fam: egui::FontFamily, first: bool| {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
            let list = fonts.families.entry(fam).or_default();
            if first {
                list.insert(0, name.into());
            } else {
                list.push(name.into());
            }
        }
    };
    add("sf", "/System/Library/Fonts/SFNS.ttf", egui::FontFamily::Proportional, true);
    add("sfmono", "/System/Library/Fonts/SFNSMono.ttf", egui::FontFamily::Monospace, true);
    add("unicode", "/System/Library/Fonts/Supplemental/Arial Unicode.ttf", egui::FontFamily::Proportional, false);
    add("unicode_m", "/System/Library/Fonts/Supplemental/Arial Unicode.ttf", egui::FontFamily::Monospace, false);
    ctx.set_fonts(fonts);

    ctx.set_visuals(egui::Visuals::dark());
    ctx.global_style_mut(|s| {
        use egui::{FontFamily::*, FontId, TextStyle};
        s.text_styles = [
            (TextStyle::Small, FontId::new(11.0, Proportional)),
            (TextStyle::Body, FontId::new(13.5, Proportional)),
            (TextStyle::Button, FontId::new(13.5, Proportional)),
            (TextStyle::Heading, FontId::new(19.0, Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, Monospace)),
        ]
        .into();
        let v = &mut s.visuals;
        v.panel_fill = PANEL;
        v.window_fill = CARD;
        v.extreme_bg_color = Color32::from_rgb(12, 12, 18);
        v.faint_bg_color = CARD;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = ACCENT.gamma_multiply(0.55);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.window_corner_radius = CornerRadius::same(14);
        v.menu_corner_radius = CornerRadius::same(12);
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_shadow = epaint::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(140) };
        v.popup_shadow = v.window_shadow;
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        for (w, fill, stroke) in [
            (&mut v.widgets.noninteractive, CARD, BORDER),
            (&mut v.widgets.inactive, CARD_HI, Color32::TRANSPARENT),
            (&mut v.widgets.hovered, Color32::from_rgb(42, 42, 58), Color32::from_rgb(70, 70, 92)),
            (&mut v.widgets.active, Color32::from_rgb(52, 46, 80), ACCENT),
            (&mut v.widgets.open, CARD_HI, BORDER),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.bg_stroke = Stroke::new(1.0, stroke);
            w.corner_radius = CornerRadius::same(8);
            w.fg_stroke.color = TEXT;
            w.expansion = 0.0;
        }
        v.widgets.noninteractive.fg_stroke.color = MUTED;
        v.widgets.inactive.fg_stroke.color = TEXT;
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.interact_size.y = 28.0;
        s.spacing.slider_width = 150.0;
        s.spacing.slider_rail_height = 4.0;
        s.spacing.combo_width = 160.0;
        s.spacing.scroll.floating = true;
    });

    // Tekstura gradientu (fiolet → róż) do przycisków i pasków.
    let w = 256;
    let px: Vec<Color32> = (0..w).map(|i| lerp(ACCENT, ACCENT2, i as f32 / (w - 1) as f32)).collect();
    let img = egui::ColorImage::new([w, 1], px);
    ctx.load_texture("gradient", img, egui::TextureOptions::LINEAR)
}

/// Pole z gradientem i zaokrągleniem.
pub fn gradient_rect(ui: &Ui, rect: Rect, radius: f32, grad: &TextureHandle, alpha: f32) {
    let shape = epaint::RectShape::filled(rect, CornerRadius::from(radius), Color32::WHITE.gamma_multiply(alpha)).with_texture(grad.id(), Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)));
    ui.painter().add(shape);
}

/// Karta sekcji.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(14)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Nagłówek karty: numer kroku w kółku z gradientem, tytuł i podpis.
pub fn card_header(ui: &mut Ui, grad: &TextureHandle, step: &str, title: &str, sub: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
        gradient_rect(ui, r, 13.0, grad, 1.0);
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, step, egui::FontId::proportional(13.0), Color32::WHITE);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new(title).size(15.5).strong().color(TEXT));
            ui.label(RichText::new(sub).size(11.5).color(MUTED));
        });
    });
    ui.add_space(6.0);
}

/// Główny przycisk z gradientem.
pub fn primary_button(ui: &mut Ui, grad: &TextureHandle, text: &str, enabled: bool, size: Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let hover = ui.ctx().animate_bool_with_time(resp.id, enabled && resp.hovered(), 0.12);
    let rad = (size.y / 2.0).min(12.0);
    if enabled {
        // Delikatna poświata pod przyciskiem.
        let glow = epaint::Shadow { offset: [0, 6], blur: 18, spread: 0, color: ACCENT.gamma_multiply(0.35 + 0.25 * hover) };
        ui.painter().add(glow.as_shape(rect, CornerRadius::from(rad)));
        gradient_rect(ui, rect, rad, grad, 1.0);
        if hover > 0.0 {
            ui.painter().rect_filled(rect, rad, Color32::from_white_alpha((22.0 * hover) as u8));
        }
    } else {
        ui.painter().rect_filled(rect, rad, CARD_HI);
    }
    let col = if enabled { Color32::WHITE } else { MUTED };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(14.0), col);
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// Przycisk z obrysem (drugorzędny).
pub fn ghost_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let b = egui::Button::new(RichText::new(text).color(if enabled { TEXT } else { MUTED })).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, BORDER)).corner_radius(10).min_size(Vec2::new(0.0, 30.0));
    ui.add_enabled(enabled, b)
}

/// Okrągły przycisk z symbolem.
pub fn icon_button(ui: &mut Ui, sym: &str, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    ui.painter().circle_filled(rect.center(), size / 2.0, lerp(CARD_HI, Color32::from_rgb(52, 52, 70), h));
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, sym, egui::FontId::proportional(size * 0.5), TEXT);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Przełącznik w stylu iOS.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = Vec2::new(36.0, 20.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.15);
    ui.painter().rect_filled(rect, 10.0, lerp(Color32::from_rgb(52, 52, 68), ACCENT, t));
    let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
    ui.painter().circle_filled(Pos2::new(x, rect.center().y), 8.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Wiersz: opis po lewej, przełącznik po prawej.
pub fn toggle_row(ui: &mut Ui, label: &str, on: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = toggle(ui, on).changed();
        });
    });
    changed
}

/// Wiersz z suwakiem: opis i wartość nad suwakiem na całą szerokość.
pub fn slider_row<N: egui::emath::Numeric>(ui: &mut Ui, label: &str, value: &mut N, range: std::ops::RangeInclusive<N>, fmt: impl Fn(f64) -> String) -> bool {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(MUTED).size(12.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(fmt(value.to_f64())).color(TEXT).size(12.5).monospace());
        });
    });
    let w = ui.available_width();
    ui.spacing_mut().slider_width = w;
    ui.add(egui::Slider::new(value, range).show_value(false)).changed()
}

/// Kafelek wyboru (np. styl animacji).
pub fn tile(ui: &mut Ui, grad: &TextureHandle, title: &str, sub: &str, selected: bool, size: Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    if selected {
        gradient_rect(ui, rect, 10.0, grad, 1.0);
        ui.painter().rect_filled(rect.shrink(1.5), 9.0, Color32::from_rgb(34, 26, 52));
    } else {
        ui.painter().rect_filled(rect, 10.0, lerp(CARD_HI, Color32::from_rgb(40, 40, 56), h));
    }
    let tc = if selected { Color32::WHITE } else { TEXT };
    ui.painter().text(Pos2::new(rect.left() + 10.0, rect.top() + 9.0), egui::Align2::LEFT_TOP, title, egui::FontId::proportional(13.0), tc);
    ui.painter().text(Pos2::new(rect.left() + 10.0, rect.bottom() - 8.0), egui::Align2::LEFT_BOTTOM, sub, egui::FontId::proportional(10.5), MUTED);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Mała etykieta „pigułka” (stała wysokość niezależnie od układu).
pub fn pill(ui: &mut Ui, text: &str, fill: Color32, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(12.0), color);
    let size = galley.size() + Vec2::new(20.0, 8.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, size.y / 2.0, fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// Przerywana zaokrąglona ramka (strefa upuszczania).
pub fn dashed_rect(ui: &Ui, rect: Rect, radius: f32, color: Color32) {
    let mut pts = Vec::new();
    let r = radius;
    let steps = 10;
    let corners = [(rect.right_top() + Vec2::new(-r, r), -90.0f32), (rect.right_bottom() + Vec2::new(-r, -r), 0.0), (rect.left_bottom() + Vec2::new(r, -r), 90.0), (rect.left_top() + Vec2::new(r, r), 180.0)];
    for (c, start) in corners {
        for i in 0..=steps {
            let a = (start + 90.0 * i as f32 / steps as f32).to_radians();
            pts.push(c + Vec2::new(a.cos(), a.sin()) * r);
        }
    }
    pts.push(pts[0]);
    let shapes = egui::Shape::dashed_line(&pts, Stroke::new(1.5, color), 8.0, 6.0);
    ui.painter().extend(shapes);
}

pub fn stroke_rect(ui: &Ui, rect: Rect, radius: f32, stroke: Stroke) {
    ui.painter().rect_stroke(rect, radius, stroke, StrokeKind::Inside);
}
