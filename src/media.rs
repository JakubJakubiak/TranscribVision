//! Wczytywanie wideo i audio w czystym Rust: kontener MP4/MOV (craft-media),
//! dekodery H.264 / HEVC (craft-video) i AAC (craft-audio).

use std::collections::VecDeque;
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use craft_media::isobmff::{self, CodecConfig, Mp4File, TrackKind};
use craft_video::h264::decode::Decoder as H264Decoder;
use craft_video::hevc::decode::{Decoder as HevcDecoder, Plane};

/// Klatka 8-bit YUV 4:2:0 (płaszczyzny bez paddingu).
#[derive(Clone)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    /// Czas prezentacji w sekundach.
    pub t: f64,
}

impl Frame {
    pub fn cw(&self) -> usize {
        self.w.div_ceil(2)
    }
    pub fn ch(&self) -> usize {
        self.h.div_ceil(2)
    }

    /// Obrót o `quarter` ćwierćobrotów zgodnie z ruchem wskazówek zegara.
    pub fn rotated(self, quarter: u8) -> Frame {
        if quarter % 4 == 0 {
            return self;
        }
        let (w, h, cw, ch) = (self.w, self.h, self.cw(), self.ch());
        let y = rotate_plane(&self.y, w, h, quarter);
        let u = rotate_plane(&self.u, cw, ch, quarter);
        let v = rotate_plane(&self.v, cw, ch, quarter);
        let (nw, nh) = if quarter % 2 == 1 { (h, w) } else { (w, h) };
        Frame { w: nw, h: nh, y, u, v, t: self.t }
    }
}

fn rotate_plane(src: &[u8], w: usize, h: usize, quarter: u8) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    match quarter % 4 {
        1 => {
            // 90° w prawo: nowa szerokość = h
            for y in 0..h {
                for x in 0..w {
                    out[x * h + (h - 1 - y)] = src[y * w + x];
                }
            }
        }
        2 => {
            for (i, p) in src.iter().enumerate() {
                out[w * h - 1 - i] = *p;
            }
        }
        3 => {
            for y in 0..h {
                for x in 0..w {
                    out[(w - 1 - x) * h + y] = src[y * w + x];
                }
            }
        }
        _ => out.copy_from_slice(src),
    }
    out
}

#[derive(Clone, Debug)]
pub struct MediaInfo {
    pub path: PathBuf,
    /// Wymiary po obrocie (tak jak widz je ogląda).
    pub width: usize,
    pub height: usize,
    pub fps: f64,
    pub duration: f64,
    pub rotation: u8,
    pub has_video: bool,
}

/// Dźwięk w pamięci: planarnie, f32.
#[derive(Clone)]
pub struct Audio {
    pub rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl Audio {
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, |c| c.len())
    }
    pub fn duration(&self) -> f64 {
        self.len() as f64 / self.rate as f64
    }

    /// Mono 16 kHz dla Whispera (filtr dolnoprzepustowy przez uśrednianie + interpolacja liniowa).
    pub fn to_mono_16k(&self) -> Vec<f32> {
        let n = self.len();
        if n == 0 {
            return Vec::new();
        }
        let chans = self.channels.len().max(1) as f32;
        let mono: Vec<f32> = (0..n).map(|i| self.channels.iter().map(|c| c[i]).sum::<f32>() / chans).collect();
        resample(&mono, self.rate, 16_000)
    }
}

/// Prosty resampler: przy zmniejszaniu częstotliwości najpierw uśrednia okno (antyaliasing).
pub fn resample(src: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || src.is_empty() {
        return src.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = ((src.len() as f64) / ratio).floor() as usize;
    let half = if ratio > 1.0 { (ratio / 2.0).ceil() as isize } else { 0 };
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            if half > 0 {
                let c = pos as isize;
                let (a, b) = ((c - half).max(0) as usize, ((c + half) as usize).min(src.len() - 1));
                let s: f32 = src[a..=b].iter().sum();
                s / (b - a + 1) as f32
            } else {
                let i0 = pos.floor() as usize;
                let i1 = (i0 + 1).min(src.len() - 1);
                let f = (pos - i0 as f64) as f32;
                src[i0] * (1.0 - f) + src[i1] * f
            }
        })
        .collect()
}

pub fn load_audio(path: &Path) -> Result<Audio> {
    let bytes = std::fs::read(path).with_context(|| format!("nie można odczytać {}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    let (_info, buf) = craft_audio::decode(&bytes, ext.as_deref()).map_err(|e| anyhow!("dekodowanie audio: {e}"))?;
    if buf.channels.is_empty() {
        bail!("plik nie ma ścieżki audio");
    }
    Ok(Audio { rate: buf.sample_rate, channels: buf.channels })
}

enum Dec {
    H264(Box<H264Decoder>),
    Hevc(Box<HevcDecoder>),
}

/// Sekwencyjny czytnik klatek ścieżki wideo z obsługą przewijania.
pub struct VideoReader {
    file: File,
    mp4: Mp4File,
    track: usize,
    dec: Dec,
    config: Vec<u8>,
    is_hevc: bool,
    next_sample: usize,
    timescale: f64,
    edit_offset: i64,
    rotation: u8,
    ready: VecDeque<Frame>,
    flushed: bool,
    draft: bool,
}

pub fn probe(path: &Path) -> Result<MediaInfo> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(), "wav" | "mp3" | "flac" | "aiff" | "aif") {
        let a = load_audio(path)?;
        return Ok(MediaInfo { path: path.into(), width: 0, height: 0, fps: 30.0, duration: a.duration(), rotation: 0, has_video: false });
    }
    let file = File::open(path)?;
    let mp4 = isobmff::open(file).map_err(|e| anyhow!("MP4: {e}"))?;
    let Some(ti) = mp4.track_of_kind(TrackKind::Video) else {
        let a = load_audio(path)?;
        return Ok(MediaInfo { path: path.into(), width: 0, height: 0, fps: 30.0, duration: a.duration(), rotation: 0, has_video: false });
    };
    let t = &mp4.tracks[ti];
    let vp = t.video().ok_or_else(|| anyhow!("brak parametrów wideo"))?;
    let rotation = t.display_rotation().unwrap_or(0);
    let (mut w, mut h) = (vp.width as usize, vp.height as usize);
    if rotation % 2 == 1 {
        std::mem::swap(&mut w, &mut h);
    }
    let duration = t.duration as f64 / t.timescale as f64;
    let fps = if duration > 0.0 { t.samples.len() as f64 / duration } else { 30.0 };
    // Zaokrąglij do typowych wartości (29.97, 30, 25, 60…).
    let fps = [23.976, 24.0, 25.0, 29.97, 30.0, 50.0, 59.94, 60.0].into_iter().find(|c| (c - fps).abs() < 0.6).unwrap_or(fps);
    Ok(MediaInfo { path: path.into(), width: w, height: h, fps, duration, rotation, has_video: true })
}

impl VideoReader {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mp4 = isobmff::open(File::open(path)?).map_err(|e| anyhow!("MP4: {e}"))?;
        let track = mp4.track_of_kind(TrackKind::Video).ok_or_else(|| anyhow!("brak ścieżki wideo"))?;
        let t = &mp4.tracks[track];
        let (config, is_hevc) = match t.codec() {
            Some(CodecConfig::Avc(c)) => (c.to_bytes(), false),
            Some(CodecConfig::Hevc(c)) => (c.to_bytes(), true),
            other => bail!("nieobsługiwany kodek wideo: {:?} (obsługiwane: H.264, HEVC)", other.map(std::mem::discriminant)),
        };
        let timescale = t.timescale as f64;
        let edit_offset = t.edit_offset;
        let rotation = t.display_rotation().unwrap_or(0);
        let dec = make_decoder(&config, is_hevc, false)?;
        Ok(VideoReader { file, mp4, track, dec, config, is_hevc, next_sample: 0, timescale, edit_offset, rotation, ready: VecDeque::new(), flushed: false, draft: false })
    }

    /// Tryb szkicu HEVC (szybszy, lekko gorsza jakość) — do podglądu.
    pub fn set_draft(&mut self, on: bool) {
        self.draft = on;
        if let Dec::Hevc(d) = &mut self.dec {
            d.set_draft(on);
        }
    }

    fn sample_count(&self) -> usize {
        self.mp4.tracks[self.track].samples.len()
    }

    /// Ustaw dekoder na klatce kluczowej przed czasem `t` (sekundy).
    pub fn seek(&mut self, t: f64) -> Result<()> {
        let raw = (t.max(0.0) * self.timescale) as i64 - self.edit_offset;
        let idx = self.mp4.sample_at_pts(self.track, raw).unwrap_or(0);
        let key = self.mp4.sync_sample_before(self.track, idx).unwrap_or(0);
        self.dec = make_decoder(&self.config, self.is_hevc, self.draft)?;
        self.next_sample = key;
        self.ready.clear();
        self.flushed = false;
        Ok(())
    }

    /// Następna klatka w kolejności prezentacji (None na końcu pliku).
    pub fn next_frame(&mut self) -> Result<Option<Frame>> {
        loop {
            if let Some(f) = self.ready.pop_front() {
                return Ok(Some(f));
            }
            if self.flushed {
                return Ok(None);
            }
            if self.next_sample >= self.sample_count() {
                let pics = match &mut self.dec {
                    Dec::H264(d) => d.flush().into_iter().map(|p| h264_frame(&p)).collect::<Vec<_>>(),
                    Dec::Hevc(d) => d.flush().into_iter().map(|p| hevc_frame(&p)).collect(),
                };
                self.push(pics);
                self.flushed = true;
                continue;
            }
            let s = self.mp4.tracks[self.track].samples[self.next_sample];
            let data = self.mp4.read_sample(&self.file, self.track, self.next_sample).map_err(|e| anyhow!("odczyt próbki: {e}"))?;
            self.next_sample += 1;
            let pts = s.pts;
            let pics = match &mut self.dec {
                Dec::H264(d) => d.decode(&data, pts).map_err(|e| anyhow!("H.264: {e}"))?.into_iter().map(|p| h264_frame(&p)).collect(),
                Dec::Hevc(d) => d.decode(&data, pts).map_err(|e| anyhow!("HEVC: {e}"))?.into_iter().map(|p| hevc_frame(&p)).collect(),
            };
            self.push(pics);
        }
    }

    fn push(&mut self, pics: Vec<(i64, Frame)>) {
        for (pts, mut f) in pics {
            f.t = (pts + self.edit_offset) as f64 / self.timescale;
            let f = f.rotated(self.rotation);
            self.ready.push_back(f);
        }
    }

    /// Klatka widoczna w chwili `t`: dekoduje do przodu (bez przewijania, jeśli to możliwe).
    /// `last` to ostatnio zwrócona klatka (dla odczytów sekwencyjnych).
    pub fn frame_at(&mut self, t: f64, last: &mut Option<Frame>, fps: f64) -> Result<Option<Frame>> {
        let half = 0.5 / fps;
        let need_seek = match last {
            Some(l) => t + half < l.t || t - l.t > 2.0,
            None => true,
        };
        if need_seek {
            self.seek(t)?;
            *last = None;
        }
        loop {
            if let Some(l) = last.as_ref() {
                // Następna klatka już w buforze? Sprawdź, czy nie jest bliżej.
                if let Some(n) = self.ready.front() {
                    if n.t <= t + half {
                        *last = self.ready.pop_front();
                        continue;
                    }
                    if l.t <= t + half {
                        return Ok(last.clone());
                    }
                }
            }
            match self.next_frame()? {
                Some(f) => {
                    if f.t > t + half {
                        if last.is_none() {
                            *last = Some(f);
                            return Ok(last.clone());
                        }
                        self.ready.push_front(f);
                        return Ok(last.clone());
                    }
                    *last = Some(f);
                }
                None => return Ok(last.clone()),
            }
        }
    }
}

fn make_decoder(config: &[u8], hevc: bool, draft: bool) -> Result<Dec> {
    Ok(if hevc {
        let mut d = HevcDecoder::from_hvcc(config).map_err(|e| anyhow!("hvcC: {e}"))?;
        d.set_draft(draft);
        Dec::Hevc(Box::new(d))
    } else {
        Dec::H264(Box::new(H264Decoder::from_avcc(config).map_err(|e| anyhow!("avcC: {e}"))?))
    })
}

fn copy_plane(src: &[u8], stride: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    for row in 0..h {
        out.extend_from_slice(&src[row * stride..row * stride + w]);
    }
    out
}

fn h264_frame(p: &craft_video::h264::decode::Picture) -> (i64, Frame) {
    let (w, h) = (p.width as usize, p.height as usize);
    let (cw, ch) = (p.chroma_width as usize, p.chroma_height as usize);
    let y = copy_plane(&p.y, p.y_stride, w, h);
    let u = fit_chroma(copy_plane(&p.u, p.uv_stride, cw, ch), cw, ch, w, h);
    let v = fit_chroma(copy_plane(&p.v, p.uv_stride, cw, ch), cw, ch, w, h);
    (p.pts, Frame { w, h, y, u, v, t: 0.0 })
}

fn plane8(p: &Plane, stride: usize, w: usize, h: usize, depth: u32) -> Vec<u8> {
    match p {
        Plane::U8(v) => copy_plane(v, stride, w, h),
        Plane::U16(v) => {
            let sh = depth.saturating_sub(8);
            let mut out = Vec::with_capacity(w * h);
            for row in 0..h {
                out.extend(v[row * stride..row * stride + w].iter().map(|s| (s >> sh).min(255) as u8));
            }
            out
        }
    }
}

fn hevc_frame(p: &craft_video::hevc::decode::Picture) -> (i64, Frame) {
    let (w, h) = (p.width as usize, p.height as usize);
    let (cw, ch) = (p.chroma_width as usize, p.chroma_height as usize);
    let y = plane8(&p.y, p.y_stride, w, h, p.bit_depth);
    let u = fit_chroma(plane8(&p.u, p.uv_stride, cw, ch, p.bit_depth), cw, ch, w, h);
    let v = fit_chroma(plane8(&p.v, p.uv_stride, cw, ch, p.bit_depth), cw, ch, w, h);
    (p.pts, Frame { w, h, y, u, v, t: 0.0 })
}

/// Sprowadź płaszczyznę chrominancji do 4:2:0 (np. z 4:2:2 lub 4:4:4).
fn fit_chroma(src: Vec<u8>, cw: usize, ch: usize, w: usize, h: usize) -> Vec<u8> {
    let (tw, th) = (w.div_ceil(2), h.div_ceil(2));
    if cw == tw && ch == th {
        return src;
    }
    let mut out = vec![128u8; tw * th];
    for y in 0..th {
        let sy = (y * ch / th).min(ch - 1);
        for x in 0..tw {
            let sx = (x * cw / tw).min(cw - 1);
            out[y * tw + x] = src[sy * cw + sx];
        }
    }
    out
}
