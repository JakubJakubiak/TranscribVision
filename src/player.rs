//! Odtwarzacz podglądu: dźwięk przez cpal (zegar odtwarzania), obraz dekodowany
//! w osobnym wątku. Odtwarza oś wyniku, czyli od razu pomija wycięte pauzy.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;

use crate::media::{Audio, MediaInfo, VideoReader};
use crate::preview::SharedPreview;
use crate::project::Timeline;

pub struct Shared {
    pub playing: AtomicBool,
    pos: AtomicU64,
    speed: AtomicU64,
    pub timeline: Mutex<Timeline>,
    pub alive: AtomicBool,
    has_audio_clock: AtomicBool,
}

impl Shared {
    pub fn pos(&self) -> f64 {
        f64::from_bits(self.pos.load(Ordering::Relaxed))
    }
    pub fn set_pos(&self, t: f64) {
        self.pos.store(t.max(0.0).to_bits(), Ordering::Relaxed);
    }
    pub fn speed(&self) -> f64 {
        f64::from_bits(self.speed.load(Ordering::Relaxed))
    }
    pub fn set_speed(&self, s: f64) {
        self.speed.store(s.to_bits(), Ordering::Relaxed);
    }
}

pub struct Player {
    pub shared: Arc<Shared>,
    _stream: Option<cpal::Stream>,
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.alive.store(false, Ordering::Relaxed);
    }
}

impl Player {
    pub fn new(info: &MediaInfo, audio: Option<Arc<Audio>>, preview: SharedPreview, ctx: egui::Context) -> Player {
        let shared = Arc::new(Shared {
            playing: AtomicBool::new(false),
            pos: AtomicU64::new(0f64.to_bits()),
            speed: AtomicU64::new(1f64.to_bits()),
            timeline: Mutex::new(Timeline::identity(info.duration)),
            alive: AtomicBool::new(true),
            has_audio_clock: AtomicBool::new(false),
        });
        let stream = audio.and_then(|a| start_audio(a, shared.clone()));
        shared.has_audio_clock.store(stream.is_some(), Ordering::Relaxed);
        if info.has_video {
            let (sh, info) = (shared.clone(), info.clone());
            std::thread::spawn(move || video_thread(info, sh, preview, ctx));
        } else {
            let sh = shared.clone();
            std::thread::spawn(move || clock_thread(sh, ctx));
        }
        Player { shared, _stream: stream }
    }

    pub fn toggle(&self) {
        let s = &self.shared;
        let total = s.timeline.lock().map(|t| t.total).unwrap_or(0.0);
        if !s.playing.load(Ordering::Relaxed) && s.pos() >= total - 0.05 {
            s.set_pos(0.0);
        }
        s.playing.fetch_xor(true, Ordering::Relaxed);
    }

    /// Zmień oś (np. po wycięciu pauz), zachowując miejsce w źródle.
    pub fn set_timeline(&self, tl: Timeline) {
        let s = &self.shared;
        if let Ok(mut cur) = s.timeline.lock() {
            let src = cur.out_to_src(s.pos());
            let new_pos = tl.src_to_out_nearest(src);
            *cur = tl;
            s.set_pos(new_pos);
        }
    }
}

fn start_audio(audio: Arc<Audio>, shared: Arc<Shared>) -> Option<cpal::Stream> {
    let host = cpal::default_host();
    let device = host.default_output_device()?;
    let sup = device.default_output_config().ok()?;
    if sup.sample_format() != cpal::SampleFormat::F32 {
        return None;
    }
    let config: cpal::StreamConfig = sup.config();
    let chans = config.channels as usize;
    let dev_rate = config.sample_rate as f64;
    let src_rate = audio.rate as f64;
    let sh = shared.clone();
    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let playing = sh.playing.load(Ordering::Relaxed);
                if !playing {
                    data.fill(0.0);
                    return;
                }
                let Ok(tl) = sh.timeline.lock() else {
                    data.fill(0.0);
                    return;
                };
                let speed = sh.speed();
                let mut pos = sh.pos();
                let step = speed / dev_rate;
                let n = audio.len();
                let mut ended = false;
                for frame in data.chunks_mut(chans) {
                    if pos >= tl.total {
                        ended = true;
                        frame.fill(0.0);
                        continue;
                    }
                    let src = tl.out_to_src(pos) * src_rate;
                    let i0 = src.floor() as usize;
                    let f = (src - i0 as f64) as f32;
                    for (c, out) in frame.iter_mut().enumerate() {
                        let ch = &audio.channels[c.min(audio.channels.len() - 1)];
                        *out = if i0 + 1 < n { ch[i0] * (1.0 - f) + ch[i0 + 1] * f } else { 0.0 };
                    }
                    pos += step;
                }
                sh.set_pos(pos.min(tl.total));
                if ended {
                    sh.playing.store(false, Ordering::Relaxed);
                }
            },
            |e| eprintln!("audio: {e}"),
            None,
        )
        .ok()?;
    stream.play().ok()?;
    Some(stream)
}

/// Zegar bez dźwięku (gdy nie ma wyjścia audio lub ścieżki dźwiękowej).
fn advance_clock(sh: &Shared, last: &mut Instant) {
    let now = Instant::now();
    let dt = now.duration_since(*last).as_secs_f64();
    *last = now;
    if sh.playing.load(Ordering::Relaxed) && !sh.has_audio_clock.load(Ordering::Relaxed) {
        let total = sh.timeline.lock().map(|t| t.total).unwrap_or(0.0);
        let p = sh.pos() + dt * sh.speed();
        if p >= total {
            sh.playing.store(false, Ordering::Relaxed);
        }
        sh.set_pos(p.min(total));
    }
}

fn clock_thread(sh: Arc<Shared>, ctx: egui::Context) {
    let mut last = Instant::now();
    while sh.alive.load(Ordering::Relaxed) {
        advance_clock(&sh, &mut last);
        if sh.playing.load(Ordering::Relaxed) {
            ctx.request_repaint();
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn video_thread(info: MediaInfo, sh: Arc<Shared>, preview: SharedPreview, ctx: egui::Context) {
    let Ok(mut reader) = VideoReader::open(&info.path) else { return };
    let mut last = None;
    let mut shown_t = f64::NAN;
    let mut clock = Instant::now();
    while sh.alive.load(Ordering::Relaxed) {
        advance_clock(&sh, &mut clock);
        let playing = sh.playing.load(Ordering::Relaxed);
        reader.set_draft(playing);
        let src = sh.timeline.lock().map(|t| t.out_to_src(sh.pos())).unwrap_or(0.0);
        let want_new = shown_t.is_nan() || (src - shown_t).abs() > 0.5 / info.fps;
        if want_new {
            if let Ok(Some(f)) = reader.frame_at(src, &mut last, info.fps) {
                if f.t != shown_t {
                    shown_t = f.t;
                    if let Ok(mut d) = preview.lock() {
                        d.frame = Some(Arc::new(f));
                        d.frame_ver += 1;
                    }
                    ctx.request_repaint();
                }
                // Na końcu pliku klatka się nie zmienia — nie dekoduj w kółko.
                if (src - shown_t).abs() > 0.5 / info.fps {
                    shown_t = src;
                }
            }
        } else if playing {
            ctx.request_repaint();
        }
        std::thread::sleep(Duration::from_millis(if playing { 3 } else { 12 }));
    }
}
