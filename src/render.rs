//! Renderer animowanych napisów (CPU, czysty Rust): czcionka przez ab_glyph,
//! obrys z transformaty odległości, cień, podświetlenie aktywnego słowa.
//! Ten sam kod rysuje podgląd i eksport, więc wynik wygląda identycznie.

use std::path::{Path, PathBuf};

use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};
use anyhow::{Result, anyhow};

use crate::project::{Anim, Page, Project};

pub const SYSTEM_FONTS: &[(&str, &str)] = &[
    ("Arial Black", "/System/Library/Fonts/Supplemental/Arial Black.ttf"),
    ("Arial Bold", "/System/Library/Fonts/Supplemental/Arial Bold.ttf"),
    ("Impact", "/System/Library/Fonts/Supplemental/Impact.ttf"),
    ("Arial Rounded", "/System/Library/Fonts/Supplemental/Arial Rounded Bold.ttf"),
    ("Verdana Bold", "/System/Library/Fonts/Supplemental/Verdana Bold.ttf"),
    ("Trebuchet Bold", "/System/Library/Fonts/Supplemental/Trebuchet MS Bold.ttf"),
];

/// Wycinek RGBA (premultiplied, f32 0..1) położony w kadrze w (x, y).
pub struct Overlay {
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 4]>,
}

pub struct CaptionRenderer {
    font: FontArc,
    pub path: PathBuf,
}

fn default_font_path() -> PathBuf {
    SYSTEM_FONTS.iter().map(|(_, p)| PathBuf::from(p)).find(|p| p.exists()).unwrap_or_else(|| PathBuf::from(SYSTEM_FONTS[0].1))
}

impl CaptionRenderer {
    pub fn new(font: Option<&Path>) -> Result<Self> {
        let path = font.map(PathBuf::from).unwrap_or_else(default_font_path);
        let bytes = std::fs::read(&path).map_err(|e| anyhow!("czcionka {}: {e}", path.display()))?;
        let font = FontArc::try_from_vec(bytes).map_err(|e| anyhow!("czcionka {}: {e}", path.display()))?;
        Ok(CaptionRenderer { font, path })
    }

    fn text_width(&self, text: &str, px: f32) -> f32 {
        let sf = self.font.as_scaled(PxScale::from(px));
        let mut w = 0.0;
        let mut prev = None;
        for c in text.chars() {
            let id = sf.glyph_id(c);
            if let Some(p) = prev {
                w += sf.kern(p, id);
            }
            w += sf.h_advance(id);
            prev = Some(id);
        }
        w
    }

    /// Wyrenderuj planszę `page` w chwili `t` (oś wyniku) dla kadru `fw`×`fh`.
    pub fn render(&self, proj: &Project, page: &Page, t: f64, fw: usize, fh: usize) -> Option<Overlay> {
        let st = &proj.style;
        // Rozmiar względem krótszego boku: tak samo czytelny w pionie i w poziomie.
        let base = (st.size * fw.min(fh) as f32).max(6.0);
        let space = self.text_width(" ", base) * 1.05;
        let line_h = base * 1.25;
        let max_w = fw as f32 * 0.86;

        // Stały podział na linie (z szerokości bez animacji, żeby linie nie skakały).
        let widths: Vec<f32> = page.words.iter().map(|w| self.text_width(&w.text, base)).collect();
        let mut lines: Vec<Vec<usize>> = vec![Vec::new()];
        let mut lw = 0.0;
        for (i, &w) in widths.iter().enumerate() {
            let empty = lines.last().is_none_or(|l| l.is_empty());
            let add = if empty { w } else { space + w };
            if lw + add > max_w && !empty {
                lines.push(vec![i]);
                lw = w;
            } else {
                lines.last_mut()?.push(i);
                lw += add;
            }
        }
        let block_h = line_h * lines.len() as f32;
        let cy = st.pos_y * fh as f32;
        let top = (cy - block_h / 2.0).clamp(0.0, (fh as f32 - block_h).max(0.0));

        // Animacja wejścia/wyjścia planszy.
        let since = (t - page.start) as f32;
        let until = (page.end - t) as f32;
        let enter = (since / 0.14).clamp(0.0, 1.0);
        let page_alpha = enter.min((until / 0.08).clamp(0.0, 1.0));
        let page_scale = if st.anim == Anim::Bounce { 1.0 } else { 0.82 + 0.18 * ease_out_back(enter) };
        if page_alpha <= 0.0 {
            return None;
        }
        let text_c = rgba_f(st.text_color);

        // Stan animacji każdego słowa.
        struct WordAnim {
            visible: bool,
            scale: f32,
            alpha: f32,
            dy: f32,
            fill: [f32; 4],
            hl: [f32; 4],
            karaoke: Option<f32>,
            boxed: bool,
        }
        let anims: Vec<WordAnim> = page
            .words
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let next_start = page.words.get(i + 1).map_or(page.end, |n| n.start);
                let active = t >= w.start && t < next_start;
                let past = t >= next_start;
                let hl = rgba_f(proj.speaker_color(w.speaker));
                let k = ((t - w.start) as f32).max(0.0);
                let mut a = WordAnim { visible: true, scale: 1.0, alpha: page_alpha, dy: 0.0, fill: text_c, hl, karaoke: None, boxed: false };
                match st.anim {
                    Anim::Pop => {
                        if active {
                            a.scale = 1.0 + 0.22 * pop_curve(k / 0.2);
                            a.fill = hl;
                        }
                    }
                    Anim::Color => {
                        if active {
                            a.fill = hl;
                        }
                    }
                    Anim::Box => {
                        if active {
                            a.boxed = true;
                            a.scale = 1.0 + 0.08 * pop_curve(k / 0.15);
                        }
                    }
                    Anim::Karaoke => {
                        if past {
                            a.fill = hl;
                        } else if active {
                            a.karaoke = Some((k / (w.end - w.start).max(0.05) as f32).clamp(0.0, 1.0));
                        }
                    }
                    Anim::Bounce => {
                        if t < w.start {
                            a.visible = false;
                        }
                        let e = (k / 0.28).clamp(0.0, 1.0);
                        a.alpha *= (k / 0.08).clamp(0.0, 1.0);
                        a.dy = -base * 0.35 * (1.0 - ease_out_bounce(e));
                        a.scale = 0.7 + 0.3 * ease_out_back(e);
                        if active {
                            a.fill = hl;
                        }
                    }
                }
                a
            })
            .collect();

        // Układ z bieżącymi szerokościami: powiększone słowo rozsuwa sąsiadów.
        let ascent = self.font.as_scaled(PxScale::from(base)).ascent();
        let mut place: Vec<(f32, f32)> = vec![(0.0, 0.0); page.words.len()];
        let mut maxw: f32 = 0.0;
        let bcx = fw as f32 / 2.0;
        let bcy = top + block_h / 2.0;
        for (li, line) in lines.iter().enumerate() {
            let cur_w = |i: usize| widths[i] * anims[i].scale.max(1.0);
            let total: f32 = line.iter().map(|&i| cur_w(i)).sum::<f32>() + space * line.len().saturating_sub(1) as f32;
            maxw = maxw.max(total * page_scale);
            let mut x = bcx - total / 2.0;
            let baseline = top + li as f32 * line_h + (line_h - base) / 2.0 + ascent * 0.92;
            for &i in line {
                let cx = x + cur_w(i) / 2.0;
                place[i] = (bcx + (cx - bcx) * page_scale, bcy + (baseline - bcy) * page_scale);
                x += cur_w(i) + space;
            }
        }

        // Płótno z zapasem na powiększenie, obrys i cień.
        let margin = base * 0.9;
        let cx0 = ((fw as f32 - maxw) / 2.0 - margin).floor().max(0.0) as i32;
        let cx1 = ((fw as f32 + maxw) / 2.0 + margin).ceil().min(fw as f32) as i32;
        let cy0 = (top - margin).floor().max(0.0) as i32;
        let cy1 = (top + block_h + margin).ceil().min(fh as f32) as i32;
        if cx1 <= cx0 || cy1 <= cy0 {
            return None;
        }
        let mut ov = Overlay { x: cx0, y: cy0, w: (cx1 - cx0) as usize, h: (cy1 - cy0) as usize, px: vec![[0.0; 4]; ((cx1 - cx0) * (cy1 - cy0)) as usize] };

        // Najpierw zwykłe słowa, na końcu aktywne (powiększone) — żeby było na wierzchu.
        let mut order: Vec<usize> = (0..page.words.len()).collect();
        order.sort_by(|&a, &b| anims[a].scale.total_cmp(&anims[b].scale));
        for i in order {
            let a = &anims[i];
            if !a.visible {
                continue;
            }
            let (wx, wy) = place[i];
            self.draw_word(&mut ov, &page.words[i].text, base * a.scale * page_scale, wx, wy + a.dy, a.fill, a.hl, a.karaoke, a.boxed.then_some(a.hl), a.alpha, st, base);
        }
        Some(ov)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_word(&self, ov: &mut Overlay, text: &str, px: f32, cx: f32, baseline: f32, fill: [f32; 4], hl: [f32; 4], karaoke: Option<f32>, boxc: Option<[f32; 4]>, alpha: f32, st: &crate::project::Style, base: f32) {
        let sf = self.font.as_scaled(PxScale::from(px));
        let width = self.text_width(text, px);
        let r = (st.outline * px).max(0.0);
        let pad = (r + px * 0.12).ceil() as i32 + 2;
        let asc = sf.ascent();
        let desc = sf.descent();
        let x0 = (cx - width / 2.0).floor() as i32 - pad;
        let y0 = (baseline - asc).floor() as i32 - pad;
        let mw = (width.ceil() as i32 + 2 * pad).max(1) as usize;
        let mh = ((asc - desc).ceil() as i32 + 2 * pad).max(1) as usize;
        let mut cov = vec![0f32; mw * mh];
        let ox = cx - width / 2.0 - x0 as f32;
        let oy = baseline - y0 as f32;
        let mut x = ox;
        let mut prev = None;
        for c in text.chars() {
            let id = sf.glyph_id(c);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(PxScale::from(px), point(x, oy));
            if let Some(og) = self.font.outline_glyph(g) {
                let b = og.px_bounds();
                og.draw(|gx, gy, v| {
                    let xx = b.min.x as i32 + gx as i32;
                    let yy = b.min.y as i32 + gy as i32;
                    if xx >= 0 && yy >= 0 && (xx as usize) < mw && (yy as usize) < mh {
                        let i = yy as usize * mw + xx as usize;
                        cov[i] = (cov[i] + v).min(1.0);
                    }
                });
            }
            x += sf.h_advance(id);
            prev = Some(id);
        }

        // Ramka pod aktywnym słowem.
        if let Some(bc) = boxc {
            let bx0 = (cx - width / 2.0 - base * 0.16) - ov.x as f32;
            let bx1 = (cx + width / 2.0 + base * 0.16) - ov.x as f32;
            let by0 = (baseline - asc * 0.86 - base * 0.1) - ov.y as f32;
            let by1 = (baseline - desc * 0.4 + base * 0.12) - ov.y as f32;
            fill_round_rect(ov, bx0, by0, bx1, by1, base * 0.22, bc, alpha);
        }

        let outline = if r > 0.5 { Some(outline_mask(&cov, mw, mh, r)) } else { None };
        let outc = rgba_f(st.outline_color);
        let sh_off = ((r * 0.35 + px * 0.04).round() as i32).max(1);
        let karaoke_x = karaoke.map(|f| ox + width * f);
        for my in 0..mh {
            for mx in 0..mw {
                let i = my * mw + mx;
                let (gx, gy) = (x0 + mx as i32 - ov.x, y0 + my as i32 - ov.y);
                if st.shadow {
                    let sx = mx as i32 - sh_off;
                    let sy = my as i32 - sh_off;
                    if sx >= 0 && sy >= 0 && (sx as usize) < mw && (sy as usize) < mh {
                        let si = sy as usize * mw + sx as usize;
                        let a = outline.as_ref().map_or(cov[si], |o| o[si]) * 0.55 * alpha;
                        if a > 0.0 {
                            blend(ov, gx, gy, [0.0, 0.0, 0.0, 1.0], a);
                        }
                    }
                }
                if let Some(o) = &outline {
                    if boxc.is_none() && o[i] > 0.0 {
                        blend(ov, gx, gy, outc, o[i] * alpha * outc[3]);
                    }
                }
                if cov[i] > 0.0 {
                    let c = match karaoke_x {
                        Some(kx) if (mx as f32) < kx => hl,
                        _ => fill,
                    };
                    blend(ov, gx, gy, c, cov[i] * alpha * c[3]);
                }
            }
        }
    }
}

fn rgba_f(c: [u8; 4]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

#[inline]
fn blend(ov: &mut Overlay, x: i32, y: i32, c: [f32; 4], a: f32) {
    if x < 0 || y < 0 || x as usize >= ov.w || y as usize >= ov.h {
        return;
    }
    let a = a.clamp(0.0, 1.0);
    let p = &mut ov.px[y as usize * ov.w + x as usize];
    for k in 0..3 {
        p[k] = p[k] * (1.0 - a) + c[k] * a;
    }
    p[3] = p[3] * (1.0 - a) + a;
}

fn fill_round_rect(ov: &mut Overlay, x0: f32, y0: f32, x1: f32, y1: f32, rad: f32, c: [f32; 4], alpha: f32) {
    let rad = rad.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    for y in (y0.floor() as i32)..=(y1.ceil() as i32) {
        for x in (x0.floor() as i32)..=(x1.ceil() as i32) {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Odległość od zaokrąglonego prostokąta (SDF).
            let dx = (x0 + rad - px).max(px - (x1 - rad)).max(0.0);
            let dy = (y0 + rad - py).max(py - (y1 - rad)).max(0.0);
            let d = (dx * dx + dy * dy).sqrt() - rad;
            let a = (0.5 - d).clamp(0.0, 1.0);
            if a > 0.0 {
                blend(ov, x, y, c, a * alpha * c[3]);
            }
        }
    }
}

/// Obrys: transformata odległości (chamfer 3-4) od pikseli litery.
fn outline_mask(cov: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    const INF: f32 = 1e9;
    let mut d: Vec<f32> = cov.iter().map(|&c| if c >= 0.5 { 0.0 } else if c > 0.0 { 0.5 - c } else { INF }).collect();
    let (a, b) = (1.0f32, std::f32::consts::SQRT_2);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let mut v = d[i];
            if x > 0 {
                v = v.min(d[i - 1] + a);
            }
            if y > 0 {
                v = v.min(d[i - w] + a);
                if x > 0 {
                    v = v.min(d[i - w - 1] + b);
                }
                if x + 1 < w {
                    v = v.min(d[i - w + 1] + b);
                }
            }
            d[i] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            let mut v = d[i];
            if x + 1 < w {
                v = v.min(d[i + 1] + a);
            }
            if y + 1 < h {
                v = v.min(d[i + w] + a);
                if x + 1 < w {
                    v = v.min(d[i + w + 1] + b);
                }
                if x > 0 {
                    v = v.min(d[i + w - 1] + b);
                }
            }
            d[i] = v;
        }
    }
    d.iter().zip(cov).map(|(&dist, &c)| (r + 0.5 - dist).clamp(0.0, 1.0).max(c)).collect()
}

fn ease_out_back(x: f32) -> f32 {
    let (c1, c3) = (1.70158, 2.70158);
    1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
}

fn ease_out_bounce(x: f32) -> f32 {
    let (n1, d1) = (7.5625, 2.75);
    if x < 1.0 / d1 {
        n1 * x * x
    } else if x < 2.0 / d1 {
        let x = x - 1.5 / d1;
        n1 * x * x + 0.75
    } else if x < 2.5 / d1 {
        let x = x - 2.25 / d1;
        n1 * x * x + 0.9375
    } else {
        let x = x - 2.625 / d1;
        n1 * x * x + 0.984375
    }
}

/// Szybki skok do 1, potem osiada na ~0.55 (słowo zostaje lekko powiększone).
fn pop_curve(k: f32) -> f32 {
    let k = k.clamp(0.0, 3.0);
    if k < 0.5 {
        (k / 0.5).powf(0.7)
    } else {
        0.55 + 0.45 * (-(k - 0.5) * 3.0).exp()
    }
}

/// Nałóż wycinek na klatkę YUV 4:2:0 (BT.709, zakres ograniczony).
pub fn composite_yuv(frame: &mut crate::media::Frame, ov: &Overlay) {
    let (w, h, cw) = (frame.w as i32, frame.h as i32, frame.cw());
    for oy in 0..ov.h as i32 {
        let y = ov.y + oy;
        if y < 0 || y >= h {
            continue;
        }
        for ox in 0..ov.w as i32 {
            let x = ov.x + ox;
            if x < 0 || x >= w {
                continue;
            }
            let p = ov.px[(oy * ov.w as i32 + ox) as usize];
            if p[3] <= 0.0 {
                continue;
            }
            let yy = 16.0 + 219.0 * (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]);
            let i = (y * w + x) as usize;
            let old = frame.y[i] as f32;
            frame.y[i] = (old * (1.0 - p[3]) + yy).round().clamp(0.0, 255.0) as u8;
        }
    }
    // Chrominancja: średnia z bloku 2×2.
    let cy0 = (ov.y.max(0) / 2) as usize;
    let cy1 = (((ov.y + ov.h as i32).min(h) + 1) / 2) as usize;
    let cx0 = (ov.x.max(0) / 2) as usize;
    let cx1 = (((ov.x + ov.w as i32).min(w) + 1) / 2) as usize;
    for cy in cy0..cy1.min(frame.ch()) {
        for cx in cx0..cx1.min(cw) {
            let mut acc = [0f32; 4];
            let mut n = 0.0;
            for dy in 0..2 {
                for dx in 0..2 {
                    let (x, y) = ((cx * 2 + dx) as i32 - ov.x, (cy * 2 + dy) as i32 - ov.y);
                    if x >= 0 && y >= 0 && (x as usize) < ov.w && (y as usize) < ov.h {
                        let p = ov.px[y as usize * ov.w + x as usize];
                        for k in 0..4 {
                            acc[k] += p[k];
                        }
                    }
                    n += 1.0;
                }
            }
            let a = acc[3] / n;
            if a <= 0.0 {
                continue;
            }
            let (r, g, b) = (acc[0] / n, acc[1] / n, acc[2] / n);
            let u = 128.0 * a + 224.0 * (-0.1146 * r - 0.3854 * g + 0.5 * b);
            let v = 128.0 * a + 224.0 * (0.5 * r - 0.4542 * g - 0.0458 * b);
            let i = cy * cw + cx;
            frame.u[i] = (frame.u[i] as f32 * (1.0 - a) + u).round().clamp(0.0, 255.0) as u8;
            frame.v[i] = (frame.v[i] as f32 * (1.0 - a) + v).round().clamp(0.0, 255.0) as u8;
        }
    }
}
