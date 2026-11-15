//! Eksport MP4 w czystym Rust: wycięte pauzy, wypalone animowane napisy,
//! H.264 (craft-video) + AAC (craft-audio) w kontenerze MP4 (craft-media).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow, bail};
use craft_audio::aac::{Encoder as AacEncoder, EncoderConfig as AacConfigEnc};
use craft_media::isobmff::{AacConfig, AvcConfig, Brand, CodecConfig, FourCc, Mp4Writer, SampleEntry, TrackConfig, WriteSample, WriterOptions};
use craft_video::h264::{Encoder as H264Encoder, EncoderConfig, PacketFormat, Preset, RateControl, YuvFrame};

use crate::media::{Audio, Frame, MediaInfo, VideoReader};
use crate::project::{Project, Timeline, page_at};
use crate::render::{CaptionRenderer, composite_yuv};

pub struct ExportOptions {
    pub burn_captions: bool,
    pub cut_pauses: bool,
    /// Jakość H.264 (CRF: mniej = lepiej, 18–28).
    pub crf: f32,
}

/// Ułamek klatek na sekundę dla enkodera.
pub fn fps_ratio(fps: f64) -> (u32, u32) {
    for (f, n, d) in [(23.976, 24000, 1001), (29.97, 30000, 1001), (59.94, 60000, 1001)] {
        if (fps - f).abs() < 0.01 {
            return (n, d);
        }
    }
    ((fps.round() as u32).max(1), 1)
}

/// Dźwięk wyniku: fragmenty źródła sklejone z krótkim przenikaniem (bez trzasków).
pub fn cut_audio(audio: &Audio, tl: &Timeline) -> Audio {
    let rate = audio.rate as f64;
    let fade = (audio.rate as usize / 250).max(1); // 4 ms
    let channels = audio
        .channels
        .iter()
        .map(|ch| {
            let mut out: Vec<f32> = Vec::with_capacity((tl.total * rate) as usize + 16);
            for s in &tl.segs {
                let a = ((s.src0 * rate) as usize).min(ch.len());
                let b = ((s.src1 * rate) as usize).min(ch.len());
                let part = &ch[a..b];
                let n = part.len();
                for (i, v) in part.iter().enumerate() {
                    let g = (i.min(n - 1 - i) as f32 / fade as f32).min(1.0);
                    out.push(v * g);
                }
            }
            out
        })
        .collect();
    Audio { rate: audio.rate, channels }
}

pub fn export_mp4(proj: &Project, info: &MediaInfo, audio: Option<&Audio>, out: &Path, opts: &ExportOptions, progress: &dyn Fn(f32, &str), cancel: &AtomicBool) -> Result<()> {
    if !info.has_video {
        bail!("plik nie ma obrazu — wyeksportuj same napisy (SRT)");
    }
    let mut p = proj.clone();
    if !opts.cut_pauses {
        p.cuts.enabled = false;
        for w in &mut p.words {
            w.cut = false;
        }
    }
    let tl = p.timeline(info.duration);
    let pages = p.pages(&tl);
    let renderer = CaptionRenderer::new(p.style.font.as_deref())?;
    let (w, h) = (info.width & !1, info.height & !1);
    let (num, den) = fps_ratio(info.fps);
    let fps = num as f64 / den as f64;

    let mut cfg = EncoderConfig::new(w as u32, h as u32, num, den);
    cfg.format = PacketFormat::LengthPrefixed;
    cfg.preset = Preset::Balanced;
    cfg.rate = RateControl::Crf(opts.crf);
    cfg.bframes = 0;
    cfg.keyint = (fps * 4.0).round() as u32;
    let mut enc = H264Encoder::new(cfg).map_err(|e| anyhow!("enkoder H.264: {e:?}"))?;
    let avcc = AvcConfig::parse(&enc.avcc()).map_err(|e| anyhow!("avcC: {e}"))?;

    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(out)?;
    let mut mux = Mp4Writer::new(file, WriterOptions::new(Brand::Mp4)).map_err(|e| anyhow!("MP4: {e}"))?;
    let vtrack = mux.add_track(TrackConfig::new(SampleEntry::avc(avcc, w as u16, h as u16), num)).map_err(|e| anyhow!("MP4: {e}"))?;

    let mut reader = VideoReader::open(&info.path)?;
    let total = (tl.total * fps).round() as usize;
    let mut last: Option<Frame> = None;
    let write_packets = |mux: &mut Mp4Writer<std::fs::File>, pkts: Vec<craft_video::h264::Packet>| -> Result<()> {
        for pk in pkts {
            mux.write_sample(vtrack, WriteSample { data: &pk.data, duration: den, composition_offset: 0, is_sync: pk.keyframe }).map_err(|e| anyhow!("MP4: {e}"))?;
        }
        Ok(())
    };
    for n in 0..total {
        if cancel.load(Ordering::Relaxed) {
            bail!("przerwano");
        }
        let t_out = n as f64 / fps;
        let t_src = tl.out_to_src(t_out);
        let Some(src) = reader.frame_at(t_src, &mut last, fps)? else { break };
        let mut f = crop_even(src, w, h);
        if opts.burn_captions {
            if let Some(pg) = page_at(&pages, t_out) {
                if let Some(ov) = renderer.render(&p, pg, t_out, w, h) {
                    composite_yuv(&mut f, &ov);
                }
            }
        }
        let frame = YuvFrame { y: &f.y, u: &f.u, v: &f.v, y_stride: w, uv_stride: w / 2 };
        let pkts = enc.encode(&frame, n as i64).map_err(|e| anyhow!("H.264: {e:?}"))?;
        write_packets(&mut mux, pkts)?;
        if n % 5 == 0 {
            progress(n as f32 / total.max(1) as f32 * 0.95, "Kodowanie obrazu");
        }
    }
    let pkts = enc.flush();
    write_packets(&mut mux, pkts)?;

    if let Some(a) = audio {
        progress(0.96, "Kodowanie dźwięku");
        let cut = cut_audio(a, &tl);
        let chans = cut.channels.len().min(2);
        let bitrate = if chans == 1 { 128_000 } else { 192_000 };
        let mut aenc = AacEncoder::new(AacConfigEnc::cbr(cut.rate, chans, bitrate)).map_err(|e| anyhow!("AAC: {e:?}"))?;
        let asc = aenc.audio_specific_config();
        let entry = SampleEntry::audio(FourCc(*b"mp4a"), CodecConfig::Aac(AacConfig::from_asc(asc)), chans as u32, cut.rate as f64, 16);
        let mut tc = TrackConfig::new(entry, cut.rate);
        tc.media_start = Some(aenc.priming_samples() as i64);
        let atrack = mux.add_track(tc).map_err(|e| anyhow!("MP4: {e}"))?;
        let planar: Vec<&[f32]> = cut.channels.iter().take(chans).map(|c| c.as_slice()).collect();
        let mut packets = aenc.encode(&planar);
        packets.extend(aenc.flush());
        for pk in &packets {
            mux.write_sample(atrack, WriteSample { data: pk, duration: 1024, composition_offset: 0, is_sync: true }).map_err(|e| anyhow!("MP4: {e}"))?;
        }
    }
    progress(0.99, "Zapisywanie");
    mux.finish_faststart().map_err(|e| anyhow!("MP4: {e}"))?;
    progress(1.0, "Gotowe");
    Ok(())
}

fn crop_even(f: Frame, w: usize, h: usize) -> Frame {
    if f.w == w && f.h == h {
        return f;
    }
    let (cw, ch) = (w / 2, h / 2);
    let fcw = f.cw();
    let mut y = Vec::with_capacity(w * h);
    for r in 0..h.min(f.h) {
        y.extend_from_slice(&f.y[r * f.w..r * f.w + w.min(f.w)]);
    }
    y.resize(w * h, 16);
    let mut u = Vec::with_capacity(cw * ch);
    let mut v = Vec::with_capacity(cw * ch);
    for r in 0..ch.min(f.ch()) {
        u.extend_from_slice(&f.u[r * fcw..r * fcw + cw.min(fcw)]);
        v.extend_from_slice(&f.v[r * fcw..r * fcw + cw.min(fcw)]);
    }
    u.resize(cw * ch, 128);
    v.resize(cw * ch, 128);
    Frame { w, h, y, u, v, t: f.t }
}
