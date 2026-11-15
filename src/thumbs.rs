//! Miniatury klatek do osi czasu, liczone w tle (dekodowanie po kolei, tryb szkicu).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, ColorImage};

use crate::media::{MediaInfo, VideoReader};

pub const HEIGHT: usize = 72;

#[derive(Default)]
pub struct ThumbStore {
    /// Odstęp miniatur w czasie źródła (s).
    pub step: f64,
    /// Szerokość / wysokość kadru.
    pub aspect: f32,
    pub images: Vec<ColorImage>,
    pub done: bool,
}

pub type SharedThumbs = Arc<Mutex<ThumbStore>>;

/// Uruchom liczenie miniatur; `alive` = false przerywa wątek (np. po otwarciu innego pliku).
pub fn start(info: &MediaInfo, ctx: egui::Context, alive: Arc<AtomicBool>) -> SharedThumbs {
    let step = (info.duration / 240.0).max(0.5);
    let aspect = info.width.max(1) as f32 / info.height.max(1) as f32;
    let store = Arc::new(Mutex::new(ThumbStore { step, aspect, images: Vec::new(), done: false }));
    if !info.has_video {
        return store;
    }
    let (st, info) = (store.clone(), info.clone());
    std::thread::spawn(move || {
        let Ok(mut r) = VideoReader::open(&info.path) else { return };
        r.set_draft(true);
        let mut last = None;
        let n = (info.duration / step).ceil() as usize;
        let tw = ((HEIGHT as f32 * aspect).round() as usize).max(8);
        for k in 0..n {
            if !alive.load(Ordering::Relaxed) {
                return;
            }
            let t = k as f64 * step;
            let Ok(Some(f)) = r.frame_at(t, &mut last, info.fps) else { break };
            let mut px = Vec::with_capacity(tw * HEIGHT);
            let cw = f.cw();
            for y in 0..HEIGHT {
                let sy = y * f.h / HEIGHT;
                for x in 0..tw {
                    let sx = x * f.w / tw;
                    let yy = (f.y[sy * f.w + sx] as f32 - 16.0) * 1.164383;
                    let u = f.u[(sy / 2) * cw + sx / 2] as f32 - 128.0;
                    let v = f.v[(sy / 2) * cw + sx / 2] as f32 - 128.0;
                    let c = |x: f32| x.round().clamp(0.0, 255.0) as u8;
                    px.push(egui::Color32::from_rgb(c(yy + 1.792741 * v), c(yy - 0.213249 * u - 0.532909 * v), c(yy + 2.112402 * u)));
                }
            }
            if let Ok(mut s) = st.lock() {
                s.images.push(ColorImage::new([tw, HEIGHT], px));
            }
            if k % 8 == 0 {
                ctx.request_repaint();
            }
        }
        if let Ok(mut s) = st.lock() {
            s.done = true;
        }
        ctx.request_repaint();
    });
    store
}
