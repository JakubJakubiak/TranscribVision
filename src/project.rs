//! Model projektu: słowa z czasami, mówcy, styl napisów, wycinanie pauz,
//! mapowanie czasu źródło ↔ wynik i podział słów na plansze napisów.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    /// Czas w pliku źródłowym (sekundy).
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub speaker: usize,
    /// Słowo wycięte ręcznie (lub jako „yyy”).
    #[serde(default)]
    pub cut: bool,
    #[serde(default)]
    pub prob: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Speaker {
    pub name: String,
    /// Kolor podświetlenia aktywnego słowa (RGBA).
    pub color: [u8; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Anim {
    /// Aktywne słowo wypełnia się kolorem od lewej do prawej.
    Karaoke,
    /// Aktywne słowo „wyskakuje” (powiększenie) w kolorze mówcy.
    Pop,
    /// Kolorowa ramka pod aktywnym słowem.
    Box,
    /// Słowa pojawiają się dopiero, gdy są wypowiadane, z odbiciem.
    Bounce,
    /// Bez animacji: tylko zmiana koloru.
    Color,
}

impl Anim {
    pub const ALL: [Anim; 5] = [Anim::Pop, Anim::Karaoke, Anim::Box, Anim::Bounce, Anim::Color];
    pub fn label(self) -> &'static str {
        match self {
            Anim::Karaoke => "Karaoke (wypełnianie)",
            Anim::Pop => "Pop (powiększenie)",
            Anim::Box => "Ramka pod słowem",
            Anim::Bounce => "Słowo po słowie",
            Anim::Color => "Tylko kolor",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Style {
    /// Własny plik czcionki (.ttf/.otf); None = systemowa pogrubiona.
    pub font: Option<PathBuf>,
    /// Wysokość liter jako ułamek krótszego boku kadru.
    pub size: f32,
    pub text_color: [u8; 4],
    pub outline_color: [u8; 4],
    /// Grubość obrysu jako ułamek rozmiaru czcionki.
    pub outline: f32,
    pub shadow: bool,
    pub anim: Anim,
    /// Środek bloku napisów (0 = góra, 1 = dół).
    pub pos_y: f32,
    pub uppercase: bool,
    pub max_words: usize,
    pub max_chars: usize,
    /// Kolor aktywnego słowa bierze się od mówcy (inaczej zawsze `highlight`).
    pub color_by_speaker: bool,
    pub highlight: [u8; 4],
}

impl Default for Style {
    fn default() -> Self {
        Style {
            font: None,
            size: 0.09,
            text_color: [255, 255, 255, 255],
            outline_color: [0, 0, 0, 255],
            outline: 0.12,
            shadow: true,
            anim: Anim::Pop,
            pos_y: 0.70,
            uppercase: true,
            max_words: 3,
            max_chars: 18,
            color_by_speaker: true,
            highlight: [255, 214, 0, 255],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CutSettings {
    /// Wycinaj pauzy dłuższe niż `min_pause` sekund.
    pub enabled: bool,
    pub min_pause: f64,
    /// Ile ciszy zostawić po obu stronach mowy.
    pub padding: f64,
}

impl Default for CutSettings {
    fn default() -> Self {
        CutSettings { enabled: true, min_pause: 0.45, padding: 0.10 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub video: PathBuf,
    pub language: String,
    pub translate: bool,
    pub words: Vec<Word>,
    pub speakers: Vec<Speaker>,
    pub style: Style,
    pub cuts: CutSettings,
}

pub const SPEAKER_COLORS: [[u8; 4]; 6] = [
    [255, 214, 0, 255],   // żółty
    [0, 220, 255, 255],   // cyjan
    [255, 80, 160, 255],  // róż
    [120, 255, 90, 255],  // zieleń
    [255, 140, 40, 255],  // pomarańcz
    [170, 120, 255, 255], // fiolet
];

pub fn default_speakers(n: usize) -> Vec<Speaker> {
    (0..n.max(1)).map(|i| Speaker { name: format!("Mówca {}", i + 1), color: SPEAKER_COLORS[i % SPEAKER_COLORS.len()] }).collect()
}

impl Project {
    pub fn new(video: PathBuf) -> Self {
        Project { video, language: "pl".into(), translate: false, words: Vec::new(), speakers: default_speakers(1), style: Style::default(), cuts: CutSettings::default() }
    }

    /// Plik projektu obok wideo: `film.mp4` → `film.napisy.json`.
    pub fn sidecar(video: &Path) -> PathBuf {
        video.with_extension("napisy.json")
    }

    pub fn save(&self) -> Result<PathBuf> {
        let p = Self::sidecar(&self.video);
        std::fs::write(&p, serde_json::to_vec_pretty(self)?)?;
        Ok(p)
    }

    pub fn load(video: &Path) -> Option<Project> {
        let data = std::fs::read(Self::sidecar(video)).ok()?;
        let mut p: Project = serde_json::from_slice(&data).ok()?;
        p.video = video.into();
        Some(p)
    }

    pub fn speaker_color(&self, s: usize) -> [u8; 4] {
        if self.style.color_by_speaker {
            self.speakers.get(s).map_or(self.style.highlight, |sp| sp.color)
        } else {
            self.style.highlight
        }
    }

    /// Fragmenty źródła, które zostają w wyniku.
    pub fn timeline(&self, duration: f64) -> Timeline {
        let live: Vec<&Word> = self.words.iter().filter(|w| !w.cut).collect();
        let mut keep: Vec<(f64, f64)> = Vec::new();
        if self.cuts.enabled && !live.is_empty() {
            let pad = self.cuts.padding.max(0.0);
            let gap = self.cuts.min_pause.max(0.05);
            let mut cur = (live[0].start, live[0].end);
            for w in &live[1..] {
                if w.start - cur.1 <= gap {
                    cur.1 = cur.1.max(w.end);
                } else {
                    keep.push(cur);
                    cur = (w.start, w.end);
                }
            }
            keep.push(cur);
            for k in &mut keep {
                k.0 = (k.0 - pad).max(0.0);
                k.1 = (k.1 + pad).min(duration);
            }
        } else {
            keep.push((0.0, duration));
            // Bez wycinania pauz: usuń tylko ręcznie wycięte słowa.
            for w in self.words.iter().filter(|w| w.cut) {
                keep = subtract(&keep, w.start, w.end);
            }
        }
        // Scal nachodzące się i odrzuć okruchy.
        keep.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for k in keep {
            if k.1 - k.0 < 0.04 {
                continue;
            }
            match merged.last_mut() {
                Some(m) if k.0 <= m.1 + 0.02 => m.1 = m.1.max(k.1),
                _ => merged.push(k),
            }
        }
        if merged.is_empty() {
            merged.push((0.0, duration));
        }
        let mut segs = Vec::new();
        let mut out = 0.0;
        for (a, b) in merged {
            segs.push(Seg { src0: a, src1: b, out0: out });
            out += b - a;
        }
        Timeline { segs, total: out }
    }

    /// Podział słów na plansze napisów (czasy w osi wyniku).
    pub fn pages(&self, tl: &Timeline) -> Vec<Page> {
        let st = &self.style;
        let mut pages: Vec<Page> = Vec::new();
        let mut cur: Option<Page> = None;
        let mut prev: Option<&Word> = None;
        for (i, w) in self.words.iter().enumerate() {
            if w.cut {
                continue;
            }
            let (Some(s), Some(e)) = (tl.src_to_out(w.start + 0.001), tl.src_to_out((w.end - 0.001).max(w.start + 0.001))) else {
                continue;
            };
            let text = if st.uppercase { w.text.to_uppercase() } else { w.text.clone() };
            let pw = PageWord { index: i, text, start: s, end: e.max(s + 0.05), speaker: w.speaker };
            let split = match (&cur, prev) {
                (Some(p), Some(pr)) => {
                    let chars: usize = p.words.iter().map(|x| x.text.chars().count() + 1).sum::<usize>() + pw.text.chars().count();
                    p.words.len() >= st.max_words.max(1)
                        || chars > st.max_chars.max(4)
                        || pr.text.ends_with(['.', '?', '!', '…', ','])
                        || w.start - pr.end > 0.6
                        || pr.speaker != w.speaker
                }
                _ => true,
            };
            if split {
                if let Some(p) = cur.take() {
                    pages.push(p);
                }
                cur = Some(Page { start: pw.start, end: pw.end, speaker: pw.speaker, words: Vec::new() });
            }
            let p = cur.as_mut().expect("page");
            p.end = pw.end;
            p.words.push(pw);
            prev = Some(w);
        }
        if let Some(p) = cur {
            pages.push(p);
        }
        // Trzymaj planszę do następnej, jeśli przerwa jest krótka.
        for i in 0..pages.len() {
            let next = pages.get(i + 1).map(|p| p.start);
            let p = &mut pages[i];
            p.end = match next {
                Some(n) if n - p.end < 0.5 => n,
                Some(n) => (p.end + 0.25).min(n),
                None => p.end + 0.3,
            };
        }
        pages
    }
}

fn subtract(keep: &[(f64, f64)], a: f64, b: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(s, e) in keep {
        if b <= s || a >= e {
            out.push((s, e));
            continue;
        }
        if a > s {
            out.push((s, a));
        }
        if b < e {
            out.push((b, e));
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
pub struct Seg {
    pub src0: f64,
    pub src1: f64,
    pub out0: f64,
}

/// Lista zachowanych fragmentów źródła i ich położenie w wyniku.
#[derive(Clone, Debug)]
pub struct Timeline {
    pub segs: Vec<Seg>,
    pub total: f64,
}

impl Timeline {
    pub fn identity(duration: f64) -> Self {
        Timeline { segs: vec![Seg { src0: 0.0, src1: duration, out0: 0.0 }], total: duration }
    }

    pub fn src_to_out(&self, t: f64) -> Option<f64> {
        self.segs.iter().find(|s| t >= s.src0 && t < s.src1).map(|s| s.out0 + (t - s.src0))
    }

    /// Najbliższy czas wyniku dla dowolnego czasu źródła (wycięte → początek następnego fragmentu).
    pub fn src_to_out_nearest(&self, t: f64) -> f64 {
        for s in &self.segs {
            if t < s.src0 {
                return s.out0;
            }
            if t < s.src1 {
                return s.out0 + (t - s.src0);
            }
        }
        self.total
    }

    pub fn out_to_src(&self, t: f64) -> f64 {
        let idx = self.segs.partition_point(|s| s.out0 <= t).saturating_sub(1);
        match self.segs.get(idx) {
            Some(s) => (s.src0 + (t - s.out0)).min(s.src1),
            None => t,
        }
    }

    pub fn removed(&self, duration: f64) -> f64 {
        duration - self.total
    }
}

#[derive(Clone, Debug)]
pub struct PageWord {
    pub index: usize,
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub speaker: usize,
}

#[derive(Clone, Debug)]
pub struct Page {
    pub start: f64,
    pub end: f64,
    pub speaker: usize,
    pub words: Vec<PageWord>,
}

pub fn page_at(pages: &[Page], t: f64) -> Option<&Page> {
    let i = pages.partition_point(|p| p.start <= t).checked_sub(1)?;
    pages.get(i).filter(|p| t < p.end)
}

fn srt_time(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

fn hex(c: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// SRT: `word_level` = osobny wpis dla każdego słowa z kolorowym podświetleniem (jak w starej wersji).
pub fn to_srt(proj: &Project, pages: &[Page], word_level: bool) -> String {
    let mut out = String::new();
    let mut n = 1;
    for p in pages {
        if !word_level {
            let text: Vec<&str> = p.words.iter().map(|w| w.text.as_str()).collect();
            out += &format!("{n}\n{} --> {}\n{}\n\n", srt_time(p.start), srt_time(p.end), text.join(" "));
            n += 1;
            continue;
        }
        for (i, w) in p.words.iter().enumerate() {
            let end = p.words.get(i + 1).map_or(p.end, |nw| nw.start);
            let text: Vec<String> = p
                .words
                .iter()
                .enumerate()
                .map(|(j, x)| if j == i { format!("<font color=\"{}\">{}</font>", hex(proj.speaker_color(x.speaker)), x.text) } else { x.text.clone() })
                .collect();
            let start = if i == 0 { p.start } else { w.start };
            out += &format!("{n}\n{} --> {}\n{}\n\n", srt_time(start), srt_time(end), text.join(" "));
            n += 1;
        }
    }
    out
}
