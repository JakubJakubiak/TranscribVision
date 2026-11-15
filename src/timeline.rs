//! Oś czasu w stylu CapCut: klipy z miniaturami i falą dźwięku, przeciąganie (magnetyczne,
//! bez dziur), przycinanie krawędzi z przyciąganiem do słów i głowicy, cięcie, duplikowanie,
//! usuwanie, cofanie, zoom kółkiem / gestem.

use std::sync::atomic::Ordering;

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::app::{App, push_undo};
use crate::project::Clip;
use crate::ui::{self, ACCENT, ACCENT2, BORDER, CARD, CARD_HI, MUTED, TEXT};

const PAD: f32 = 14.0;
const RULER_H: f32 = 22.0;
const VIDEO_H: f32 = 76.0;
const CAPTION_H: f32 = 22.0;
const EDGE: f32 = 9.0;

#[derive(Clone, Copy)]
enum Drag {
    /// Przeciąganie klipu: indeks, przesunięcie chwytu od lewej krawędzi (px).
    Move { clip: usize, grab: f32 },
    TrimL { clip: usize, orig: Clip, press_x: f32 },
    TrimR { clip: usize, orig: Clip, press_x: f32 },
    Scrub,
}

pub struct TimelineView {
    /// Piksele na sekundę (None = dopasuj do szerokości).
    pps: Option<f32>,
    /// Początek widoku (s).
    scroll: f64,
    pub selected: Option<usize>,
    drag: Option<Drag>,
    pub snap: bool,
    thumb_tex: Vec<egui::TextureHandle>,
    last_fit: f32,
}

impl Default for TimelineView {
    fn default() -> Self {
        TimelineView { pps: None, scroll: 0.0, selected: None, drag: None, snap: true, thumb_tex: Vec::new(), last_fit: 10.0 }
    }
}

impl App {
    /// Skróty montażu (gdy nic nie przyjmuje tekstu).
    pub(crate) fn timeline_keys(&mut self, ctx: &egui::Context) {
        use egui::Key;
        use egui::Modifiers as M;
        // consume_key sprawdza modyfikatory zapisane w samym zdarzeniu (pewniejsze niż stan klawiszy).
        // Kolejność ma znaczenie: dopasowanie „logiczne” ignoruje Shift, więc warianty z Shiftem najpierw.
        let (split, del, dup, redo, undo, sleft, sright, left, right) = ctx.input_mut(|i| {
            let redo = i.consume_key(M::COMMAND | M::SHIFT, Key::Z);
            let undo = i.consume_key(M::COMMAND, Key::Z);
            let dup = i.consume_key(M::COMMAND, Key::D);
            let sleft = i.consume_key(M::SHIFT, Key::ArrowLeft);
            let sright = i.consume_key(M::SHIFT, Key::ArrowRight);
            (
                i.consume_key(M::NONE, Key::S),
                i.consume_key(M::NONE, Key::Delete) || i.consume_key(M::NONE, Key::Backspace),
                dup,
                redo,
                undo,
                sleft,
                sright,
                i.consume_key(M::NONE, Key::ArrowLeft),
                i.consume_key(M::NONE, Key::ArrowRight),
            )
        });
        let shift = sleft || sright;
        let (left, right) = (left || sleft, right || sright);
        if split {
            self.edit_split();
        }
        if del {
            self.edit_delete();
        }
        if dup {
            self.edit_duplicate();
        }
        if undo {
            self.edit_undo();
        }
        if redo {
            self.edit_redo();
        }
        if left || right {
            if let Some(l) = self.loaded.as_ref() {
                let step = if shift { 1.0 } else { 1.0 / l.info.fps.max(1.0) };
                let sh = &l.player.shared;
                let p = sh.pos() + if right { step } else { -step };
                sh.set_pos(p.clamp(0.0, self.tl.total));
            }
        }
    }

    fn edit_split(&mut self) {
        let Some(l) = self.loaded.as_mut() else { return };
        let before = l.project.clone();
        let t = l.player.shared.pos();
        if let Some(i) = l.project.split_at(l.info.duration, &self.tl, t) {
            push_undo(&mut self.undo, &mut self.redo, before);
            self.tlv.selected = Some(i + 1);
            self.dirty = true;
        }
    }

    fn edit_delete(&mut self) {
        let Some(l) = self.loaded.as_mut() else { return };
        let Some(i) = self.tlv.selected else { return };
        if l.project.clips_or_full(l.info.duration).len() <= 1 {
            self.set_status_pub("Nie można usunąć ostatniego klipu.", true);
            return;
        }
        push_undo(&mut self.undo, &mut self.redo, l.project.clone());
        l.project.delete_clip(l.info.duration, i);
        let n = l.project.clips.len();
        self.tlv.selected = if n == 0 { None } else { Some(i.min(n - 1)) };
        self.dirty = true;
    }

    fn edit_duplicate(&mut self) {
        let Some(l) = self.loaded.as_mut() else { return };
        let Some(i) = self.tlv.selected else { return };
        push_undo(&mut self.undo, &mut self.redo, l.project.clone());
        self.tlv.selected = l.project.duplicate_clip(l.info.duration, i);
        self.dirty = true;
    }

    fn edit_undo(&mut self) {
        let Some(l) = self.loaded.as_mut() else { return };
        if let Some(p) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut l.project, p));
            self.dirty = true;
            self.tlv.selected = None;
        }
    }

    fn edit_redo(&mut self) {
        let Some(l) = self.loaded.as_mut() else { return };
        if let Some(p) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut l.project, p));
            self.dirty = true;
            self.tlv.selected = None;
        }
    }

    fn set_status_pub(&mut self, s: &str, err: bool) {
        self.status = s.into();
        self.status_error = err;
    }

    pub(crate) fn timeline_panel(&mut self, ui: &mut egui::Ui) {
        if self.loaded.is_none() {
            return;
        }
        // Nowe miniatury → tekstury.
        if let Some(l) = self.loaded.as_ref() {
            if let Ok(st) = l.thumbs.lock() {
                while self.tlv.thumb_tex.len() < st.images.len() {
                    let i = self.tlv.thumb_tex.len();
                    let tex = ui.ctx().load_texture(format!("thumb{i}"), st.images[i].clone(), egui::TextureOptions::LINEAR);
                    self.tlv.thumb_tex.push(tex);
                }
            }
        }
        ui.add_space(6.0);
        self.toolbar(ui);
        ui.add_space(4.0);
        self.tracks(ui);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let has_sel = self.tlv.selected.is_some();
        ui.horizontal(|ui| {
            if tool(ui, "↶", "Cofnij  ⌘Z", !self.undo.is_empty()).clicked() {
                self.edit_undo();
            }
            if tool(ui, "↷", "Ponów  ⇧⌘Z", !self.redo.is_empty()).clicked() {
                self.edit_redo();
            }
            ui.add_space(6.0);
            if tool_text(ui, "✂  Podziel", "Tnij w miejscu głowicy  S", true).clicked() {
                self.edit_split();
            }
            if tool_text(ui, "⧉  Duplikuj", "⌘D", has_sel).clicked() {
                self.edit_duplicate();
            }
            if tool_text(ui, "✕  Usuń", "Delete", has_sel).clicked() {
                self.edit_delete();
            }
            ui.add_space(6.0);
            ui.label(RichText::new("Przyciąganie").color(MUTED).size(12.5));
            ui::toggle(ui, &mut self.tlv.snap).on_hover_text("Krawędzie klipów przyklejają się do słów i głowicy");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui::toggle(ui, &mut self.preview_cuts).changed() {
                    self.dirty = true;
                }
                ui.label(RichText::new("Bez pauz").color(MUTED).size(12.5));
                ui.add_space(10.0);
                if tool_text(ui, "Dopasuj", "Cały film na szerokość", true).clicked() {
                    self.tlv.pps = None;
                    self.tlv.scroll = 0.0;
                }
                if tool(ui, "+", "Przybliż (⌘ + kółko / gest)", true).clicked() {
                    self.zoom_by(1.4, None);
                }
                if tool(ui, "−", "Oddal", true).clicked() {
                    self.zoom_by(1.0 / 1.4, None);
                }
            });
        });
    }

    fn fit_pps(&self, width: f32) -> f32 {
        ((width - 2.0 * PAD) / self.tl.total.max(0.1) as f32).max(1.0)
    }

    fn zoom_by(&mut self, f: f32, anchor: Option<(f32, f64)>) {
        let cur = self.tlv.pps.unwrap_or(self.tlv.last_fit);
        let new = (cur * f).clamp(self.tlv.last_fit.min(cur), 600.0);
        if let Some((ax, at)) = anchor {
            // Zachowaj czas pod kursorem.
            self.tlv.scroll = (at - (ax - PAD) as f64 / new as f64).max(0.0);
        }
        self.tlv.pps = Some(new);
    }

    fn tracks(&mut self, ui: &mut egui::Ui) {
        let Some((duration, shared, clips, words)) = self.loaded.as_ref().map(|l| (l.info.duration, l.player.shared.clone(), l.project.clips_or_full(l.info.duration), l.project.words.clone())) else { return };
        let size = Vec2::new(ui.available_width(), (RULER_H + VIDEO_H + CAPTION_H + 26.0).max(ui.available_height() - 4.0));
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let fit = self.fit_pps(rect.width());
        self.tlv.last_fit = fit;
        let pps = self.tlv.pps.unwrap_or(fit).max(fit.min(1.0));
        if self.tlv.pps.is_none() {
            self.tlv.scroll = 0.0;
        }
        let total = self.tl.total;
        let view_secs = ((rect.width() - 2.0 * PAD) / pps) as f64;
        self.tlv.scroll = self.tlv.scroll.clamp(0.0, (total - view_secs * 0.5).max(0.0));
        let scroll = self.tlv.scroll;
        let x_of = |t: f64| rect.left() + PAD + ((t - scroll) as f32) * pps;
        let t_of = |x: f32| scroll + ((x - rect.left() - PAD) / pps) as f64;

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 14.0, CARD);
        ui::stroke_rect(ui, rect, 14.0, Stroke::new(1.0, BORDER));

        let ruler = Rect::from_min_size(rect.min + Vec2::new(0.0, 4.0), Vec2::new(rect.width(), RULER_H));
        let vtrack = Rect::from_min_size(Pos2::new(rect.left(), ruler.bottom() + 6.0), Vec2::new(rect.width(), VIDEO_H));
        let ctrack = Rect::from_min_size(Pos2::new(rect.left(), vtrack.bottom() + 8.0), Vec2::new(rect.width(), CAPTION_H));

        // Linijka z podziałką.
        let tick = nice_step(70.0 / pps as f64);
        let mut t = (scroll / tick).floor() * tick;
        while x_of(t) < rect.right() {
            let x = x_of(t);
            if x >= rect.left() + PAD - 1.0 {
                painter.line_segment([Pos2::new(x, ruler.bottom() - 6.0), Pos2::new(x, ruler.bottom())], Stroke::new(1.0, MUTED.gamma_multiply(0.6)));
                painter.text(Pos2::new(x + 3.0, ruler.center().y - 2.0), egui::Align2::LEFT_CENTER, short_time(t), egui::FontId::monospace(10.5), MUTED);
                let sub = tick / 5.0;
                for k in 1..5 {
                    let xs = x_of(t + sub * k as f64);
                    painter.line_segment([Pos2::new(xs, ruler.bottom() - 3.0), Pos2::new(xs, ruler.bottom())], Stroke::new(1.0, MUTED.gamma_multiply(0.3)));
                }
            }
            t += tick;
        }

        // Klipy (w kolejności montażu) i ich zakresy w wyniku.
        let ranges: Vec<(f64, f64)> = (0..clips.len()).map(|i| self.tl.clip_range(i).unwrap_or((0.0, 0.0))).collect();
        let pointer = ui.ctx().pointer_hover_pos();
        let pressed_pos = ui.ctx().input(|i| i.pointer.press_origin());

        // Rozpoczęcie przeciągania.
        if resp.drag_started() {
            if let Some(p) = pressed_pos {
                self.tlv.drag = Some(if ruler.contains(p) || !vtrack.expand2(Vec2::new(0.0, 4.0)).contains(p) {
                    Drag::Scrub
                } else {
                    match hit_clip(&ranges, p.x, &x_of) {
                        Some((i, Hit::Left)) => Drag::TrimL { clip: i, orig: clips[i], press_x: p.x },
                        Some((i, Hit::Right)) => Drag::TrimR { clip: i, orig: clips[i], press_x: p.x },
                        Some((i, Hit::Body)) => {
                            self.tlv.selected = Some(i);
                            Drag::Move { clip: i, grab: p.x - x_of(ranges[i].0) }
                        }
                        None => Drag::Scrub,
                    }
                });
                if matches!(self.tlv.drag, Some(Drag::TrimL { .. } | Drag::TrimR { .. })) {
                    if let Some(lm) = self.loaded.as_ref() {
                        push_undo(&mut self.undo, &mut self.redo, lm.project.clone());
                    }
                }
            }
        }

        // Kursor nad krawędzią klipu.
        if let Some(p) = pointer {
            if vtrack.contains(p) && self.tlv.drag.is_none() {
                match hit_clip(&ranges, p.x, &x_of) {
                    Some((_, Hit::Left | Hit::Right)) => ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal),
                    Some((_, Hit::Body)) => ui.ctx().set_cursor_icon(egui::CursorIcon::Grab),
                    None => {}
                }
            }
        }

        // Kliknięcie: zaznacz klip / przewiń.
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if vtrack.contains(p) {
                    self.tlv.selected = hit_clip(&ranges, p.x, &x_of).map(|(i, _)| i);
                } else if !ruler.contains(p) {
                    self.tlv.selected = None;
                }
                shared.set_pos(t_of(p.x).clamp(0.0, total));
            }
        }

        // Przebieg przeciągania.
        let mut move_target: Option<(usize, usize, f32)> = None; // (from, to, x lewej krawędzi)
        let snap_px = 8.0;
        if let (Some(d), Some(p)) = (self.tlv.drag, resp.interact_pointer_pos()) {
            match d {
                Drag::Scrub => {
                    shared.set_pos(t_of(p.x).clamp(0.0, total));
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                Drag::Move { clip, grab } => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    let left = p.x - grab;
                    let w = ((ranges[clip].1 - ranges[clip].0) as f32 * pps).max(4.0);
                    let center_t = t_of(left + w / 2.0);
                    // Pozycja docelowa: ile pozostałych klipów ma środek przed środkiem przeciąganego.
                    let to = (0..ranges.len()).filter(|&i| i != clip).filter(|&i| (ranges[i].0 + ranges[i].1) / 2.0 < center_t).count();
                    move_target = Some((clip, to, left));
                }
                Drag::TrimL { clip, orig, press_x } => {
                    let mut src0 = orig.src0 + ((p.x - press_x) / pps) as f64;
                    if self.tlv.snap {
                        src0 = snap_src(&words, src0, snap_px as f64 / pps as f64);
                    }
                    let src0 = src0.clamp(0.0, orig.src1 - 0.1);
                    if let Some(lm) = self.loaded.as_mut() {
                        lm.project.trim_clip(lm.info.duration, clip, src0, orig.src1);
                    }
                    self.dirty = true;
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                Drag::TrimR { clip, orig, press_x } => {
                    let mut src1 = orig.src1 + ((p.x - press_x) / pps) as f64;
                    if self.tlv.snap {
                        src1 = snap_src(&words, src1, snap_px as f64 / pps as f64);
                    }
                    let dur = duration;
                    let src1 = src1.clamp(orig.src0 + 0.1, dur);
                    if let Some(lm) = self.loaded.as_mut() {
                        lm.project.trim_clip(lm.info.duration, clip, orig.src0, src1);
                    }
                    self.dirty = true;
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
            }
            // Przewijanie przy krawędzi podczas przeciągania.
            let edge = 30.0;
            if p.x > rect.right() - edge {
                self.tlv.scroll += (8.0 / pps) as f64;
                self.tlv.pps.get_or_insert(pps);
            } else if p.x < rect.left() + edge && self.tlv.scroll > 0.0 {
                self.tlv.scroll = (self.tlv.scroll - (8.0 / pps) as f64).max(0.0);
            }
        }
        if resp.drag_stopped() {
            if let Some((from, to, _)) = move_target {
                if from != to {
                    if let Some(lm) = self.loaded.as_mut() {
                        push_undo(&mut self.undo, &mut self.redo, lm.project.clone());
                        lm.project.move_clip(lm.info.duration, from, to);
                        self.tlv.selected = Some(to);
                        self.dirty = true;
                    }
                }
            }
            self.tlv.drag = None;
        }
        let Some(l) = self.loaded.as_ref() else { return };

        // Układ klipów (magnetyczny: przy przeciąganiu reszta się rozsuwa).
        let widths: Vec<f32> = ranges.iter().map(|r| ((r.1 - r.0) as f32 * pps).max(4.0)).collect();
        let mut target_x: Vec<f32> = ranges.iter().map(|r| x_of(r.0)).collect();
        if let Some((from, to, _)) = move_target {
            let mut x = x_of(0.0);
            let mut order: Vec<usize> = (0..ranges.len()).filter(|&i| i != from).collect();
            order.insert(to.min(order.len()), from);
            for i in order {
                target_x[i] = x;
                x += widths[i];
            }
        }
        let gap = 2.0;
        let thumbs = l.thumbs.lock().ok().map(|s| (s.step, s.aspect));
        for (i, c) in clips.iter().enumerate() {
            if move_target.is_some_and(|m| m.0 == i) {
                continue;
            }
            let x = ui.ctx().animate_value_with_time(egui::Id::unique(("clipx", c.id)), target_x[i], 0.12);
            let r = Rect::from_min_size(Pos2::new(x + gap / 2.0, vtrack.top()), Vec2::new((widths[i] - gap).max(2.0), VIDEO_H));
            self.draw_clip(ui, &painter, r, i, thumbs, pps, false);
        }
        // Przeciągany klip na wierzchu, z cieniem.
        if let Some((from, _, left)) = move_target {
            let r = Rect::from_min_size(Pos2::new(left, vtrack.top() - 4.0), Vec2::new(widths[from], VIDEO_H));
            let sh = egui::epaint::Shadow { offset: [0, 8], blur: 20, spread: 0, color: Color32::from_black_alpha(170) };
            painter.add(sh.as_shape(r, CornerRadius::same(8)));
            self.draw_clip(ui, &painter, r, from, thumbs, pps, true);
            // Znacznik miejsca wstawienia.
            let to = move_target.map_or(0, |m| m.1);
            let order: Vec<usize> = (0..ranges.len()).filter(|&i| i != from).collect();
            let ix = if to >= order.len() { x_of(self.tl.total) } else { target_x[order[to]] };
            painter.line_segment([Pos2::new(ix, vtrack.top() - 6.0), Pos2::new(ix, vtrack.bottom() + 6.0)], Stroke::new(2.0, ACCENT2));
        }

        // Ścieżka napisów.
        painter.text(Pos2::new(rect.left() + 4.0, ctrack.center().y), egui::Align2::LEFT_CENTER, "", egui::FontId::proportional(10.0), MUTED);
        for pg in &self.pages {
            let (x0, x1) = (x_of(pg.start), x_of(pg.end));
            if x1 < rect.left() || x0 > rect.right() {
                continue;
            }
            let r = Rect::from_x_y_ranges(x0..=(x1 - 1.0).max(x0 + 2.0), ctrack.y_range());
            let col = l.project.speakers.get(pg.speaker).map_or(ACCENT, |s| ui::rgb(s.color));
            painter.rect_filled(r, 5.0, col.gamma_multiply(0.22));
            painter.rect_filled(Rect::from_min_size(r.min, Vec2::new(3.0f32.min(r.width()), r.height())), 2.0, col);
            if r.width() > 24.0 {
                let text: String = pg.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
                let clip = painter.with_clip_rect(r.shrink2(Vec2::new(5.0, 0.0)));
                clip.text(Pos2::new(r.left() + 7.0, r.center().y), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(11.0), TEXT);
            }
        }

        // Głowica.
        let px = x_of(l.player.shared.pos());
        if px >= rect.left() && px <= rect.right() {
            painter.line_segment([Pos2::new(px, ruler.top() + 4.0), Pos2::new(px, rect.bottom() - 6.0)], Stroke::new(2.0, Color32::WHITE));
            let tri = vec![Pos2::new(px - 6.0, ruler.top() + 2.0), Pos2::new(px + 6.0, ruler.top() + 2.0), Pos2::new(px, ruler.top() + 10.0)];
            painter.add(egui::Shape::convex_polygon(tri, Color32::WHITE, Stroke::NONE));
        }
        // Podążanie za głowicą podczas odtwarzania.
        if l.player.shared.playing.load(Ordering::Relaxed) && self.tlv.pps.is_some() && (px > rect.right() - 40.0 || px < rect.left()) {
            self.tlv.scroll = l.player.shared.pos() - (40.0 / pps) as f64;
        }

        // Zoom (⌘ + kółko, gest) i przewijanie kółkiem.
        if resp.hovered() || ui.rect_contains_pointer(rect) {
            let (zoom, scroll_d) = ui.ctx().input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
            if (zoom - 1.0).abs() > 1e-3 {
                let ax = pointer.map_or(rect.center().x, |p| p.x);
                let at = t_of(ax);
                self.tlv.pps.get_or_insert(pps);
                self.zoom_by(zoom, Some((ax, at)));
            } else if self.tlv.pps.is_some() {
                let d = if scroll_d.x.abs() > scroll_d.y.abs() { scroll_d.x } else { scroll_d.y };
                if d.abs() > 0.0 {
                    self.tlv.scroll = (self.tlv.scroll - (d / pps) as f64).max(0.0);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_clip(&self, ui: &egui::Ui, painter: &egui::Painter, r: Rect, ci: usize, thumbs: Option<(f64, f32)>, pps: f32, lifted: bool) {
        let Some(l) = self.loaded.as_ref() else { return };
        let selected = self.tlv.selected == Some(ci);
        let p = painter.with_clip_rect(r.intersect(painter.clip_rect()));
        p.rect_filled(r, 8.0, CARD_HI);
        let range = self.tl.clip_range(ci).unwrap_or((0.0, 0.0));
        // Pozycja w osi wyniku dla x wewnątrz prostokąta (przeciągany klip może być przesunięty).
        let out_at = |x: f32| range.0 + ((x - r.left()) / pps) as f64;
        let wave_h = 18.0;
        let img_rect = Rect::from_min_max(r.min, Pos2::new(r.right(), r.bottom() - wave_h));
        // Miniatury.
        if let Some((step, aspect)) = thumbs {
            if !self.tlv.thumb_tex.is_empty() {
                let tw = (img_rect.height() * aspect).max(8.0);
                let mut x = r.left();
                while x < r.right() {
                    let src = self.tl.out_to_src(out_at(x + tw / 2.0).min(range.1));
                    let idx = ((src / step).round() as usize).min(self.tlv.thumb_tex.len() - 1);
                    let tr = Rect::from_min_size(Pos2::new(x, img_rect.top()), Vec2::new(tw, img_rect.height()));
                    p.image(self.tlv.thumb_tex[idx].id(), tr, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    x += tw;
                }
            }
        }
        // Fala dźwięku na dole klipu.
        let wr = Rect::from_min_max(Pos2::new(r.left(), r.bottom() - wave_h), r.max);
        p.rect_filled(wr, 0.0, Color32::from_rgb(28, 24, 44));
        let n = l.peaks.len();
        if n > 0 {
            let mut x = wr.left();
            while x < wr.right() {
                let src = self.tl.out_to_src(out_at(x));
                let k = ((src * 100.0) as usize).min(n - 1);
                let pk = l.peaks[k.saturating_sub(1)..(k + 2).min(n)].iter().fold(0f32, |m, v| m.max(*v)).min(1.0).sqrt();
                let h = (pk * (wave_h - 4.0)).max(1.0);
                let col = ui::lerp(ACCENT, ACCENT2, ((x - wr.left()) / wr.width().max(1.0)).clamp(0.0, 1.0));
                p.line_segment([Pos2::new(x, wr.center().y - h / 2.0), Pos2::new(x, wr.center().y + h / 2.0)], Stroke::new(1.5, col));
                x += 2.0;
            }
        }
        // Miejsca wyciętych pauz wewnątrz klipu.
        for s in self.tl.segs.iter().filter(|s| s.clip == ci).skip(1) {
            let x = r.left() + ((s.out0 - range.0) as f32) * pps;
            p.line_segment([Pos2::new(x, r.top() + 4.0), Pos2::new(x, r.bottom() - 4.0)], Stroke::new(1.0, Color32::from_white_alpha(70)));
        }
        // Czas trwania.
        let dur = range.1 - range.0;
        if r.width() > 44.0 {
            let label = format!("{dur:.1}s");
            let g = ui.painter().layout_no_wrap(label, egui::FontId::proportional(10.5), Color32::WHITE);
            let br = Rect::from_min_size(r.min + Vec2::new(5.0, 5.0), g.size() + Vec2::new(8.0, 2.0));
            p.rect_filled(br, 4.0, Color32::from_black_alpha(150));
            p.galley(br.min + Vec2::new(4.0, 1.0), g, Color32::WHITE);
        }
        // Obramowanie i uchwyty przycinania.
        if selected || lifted {
            ui::stroke_rect(ui, r, 8.0, Stroke::new(2.0, Color32::WHITE));
            for (x, dir) in [(r.left(), 1.0f32), (r.right(), -1.0)] {
                let hr = Rect::from_min_size(Pos2::new(if dir > 0.0 { x } else { x - EDGE }, r.top()), Vec2::new(EDGE, r.height()));
                painter.rect_filled(hr, CornerRadius::same(4), Color32::WHITE);
                let cx = hr.center().x;
                painter.line_segment([Pos2::new(cx, r.center().y - 8.0), Pos2::new(cx, r.center().y + 8.0)], Stroke::new(1.5, Color32::from_gray(90)));
            }
        } else {
            ui::stroke_rect(ui, r, 8.0, Stroke::new(1.0, Color32::from_white_alpha(25)));
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Left,
    Right,
    Body,
}

fn hit_clip(ranges: &[(f64, f64)], x: f32, x_of: &dyn Fn(f64) -> f32) -> Option<(usize, Hit)> {
    for (i, r) in ranges.iter().enumerate() {
        let (a, b) = (x_of(r.0), x_of(r.1));
        if x >= a - 2.0 && x <= b + 2.0 {
            let edge = EDGE.min((b - a) / 3.0);
            return Some((i, if x - a < edge { Hit::Left } else if b - x < edge { Hit::Right } else { Hit::Body }));
        }
    }
    None
}

/// Przyciąganie krawędzi klipu do granic słów (czas źródła).
fn snap_src(words: &[crate::project::Word], t: f64, tol: f64) -> f64 {
    let mut best = (tol, t);
    for w in words {
        for e in [w.start - 0.03, w.end + 0.05] {
            let d = (e - t).abs();
            if d < best.0 {
                best = (d, e);
            }
        }
    }
    best.1
}

fn nice_step(min: f64) -> f64 {
    for s in [0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0] {
        if s >= min {
            return s;
        }
    }
    1200.0
}

fn short_time(t: f64) -> String {
    let t = t.max(0.0);
    if t < 60.0 && t.fract() > 1e-6 { format!("{t:.1}s") } else { format!("{:02}:{:02}", (t / 60.0) as u32, (t % 60.0).round() as u32) }
}

fn tool(ui: &mut egui::Ui, sym: &str, tip: &str, enabled: bool) -> egui::Response {
    let b = egui::Button::new(RichText::new(sym).size(15.0)).fill(CARD_HI).stroke(Stroke::NONE).corner_radius(8).min_size(Vec2::new(30.0, 28.0));
    ui.add_enabled(enabled, b).on_hover_text(tip)
}

fn tool_text(ui: &mut egui::Ui, text: &str, tip: &str, enabled: bool) -> egui::Response {
    let b = egui::Button::new(RichText::new(text).size(12.5)).fill(CARD_HI).stroke(Stroke::NONE).corner_radius(8).min_size(Vec2::new(0.0, 28.0));
    ui.add_enabled(enabled, b).on_hover_text(tip)
}
