mod app;
mod export;
mod media;
mod player;
mod preview;
mod project;
mod render;
mod speakers;
mod transcribe;
mod thumbs;
mod timeline;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("cli") {
        return cli(&args[1..]);
    }
    app::run(args.first().map(PathBuf::from))
}

/// Tryb bez okna:
/// `transcribvision cli <wideo> [--model ID|ŚCIEŻKA] [--lang pl] [--translate] [--prompt "nazwy"] [--speakers N]
///  [--words plik.json] [--out wynik.mp4] [--no-cut] [--anim pop|karaoke|box|bounce|color] [--frame SEK plik.ppm]`
fn cli(args: &[String]) -> Result<()> {
    let video = PathBuf::from(args.first().context("podaj plik wideo")?);
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let has = |k: &str| args.iter().any(|a| a == k);
    let info = media::probe(&video)?;
    eprintln!("{}x{} {:.2} kl/s, {:.2} s", info.width, info.height, info.fps, info.duration);
    let audio = media::load_audio(&video).ok();
    let mut proj = project::Project::load(&video).unwrap_or_else(|| project::Project::new(video.clone()));
    if let Some(l) = get("--lang") {
        proj.language = l;
    }
    proj.translate = has("--translate");
    if let Some(p) = get("--prompt") {
        proj.prompt = p;
    }
    if let Some(a) = get("--anim") {
        proj.style.anim = match a.as_str() {
            "karaoke" => project::Anim::Karaoke,
            "box" => project::Anim::Box,
            "bounce" => project::Anim::Bounce,
            "color" => project::Anim::Color,
            _ => project::Anim::Pop,
        };
    }
    if let Some(wf) = get("--words") {
        proj.words = serde_json::from_slice(&std::fs::read(wf)?)?;
    }
    if let Some(m) = get("--model") {
        let path = if m.ends_with(".bin") { PathBuf::from(&m) } else { transcribe::model_path(&m) };
        if !path.exists() {
            bail!("brak modelu {}", path.display());
        }
        let a16 = audio.as_ref().context("brak audio")?.to_mono_16k();
        let t0 = std::time::Instant::now();
        let opts = transcribe::Options { language: proj.language.clone(), translate: proj.translate, beam: false, prompt: proj.prompt.clone() };
        let prog: Arc<dyn Fn(f32) + Send + Sync> = Arc::new(|p| eprint!("\rTranskrypcja {:3.0}%", p * 100.0));
        proj.words = transcribe::transcribe(&a16, &path, &opts, prog, Arc::new(AtomicBool::new(false)))?;
        eprintln!("\n{} słów w {:.1} s", proj.words.len(), t0.elapsed().as_secs_f64());
        let n: usize = get("--speakers").and_then(|s| s.parse().ok()).unwrap_or(1);
        speakers::diarize(&mut proj.words, &a16, n);
        proj.speakers = project::default_speakers(n);
    }
    let tl = proj.timeline(info.duration);
    let pages = proj.pages(&tl);
    eprintln!("Wycięto {:.2} s pauz ({} fragmentów), {} plansz napisów", tl.removed(info.duration), tl.segs.len(), pages.len());
    for w in &proj.words {
        eprintln!("  {:6.2}-{:6.2} [{}] {}", w.start, w.end, w.speaker, w.text);
    }
    std::fs::write(video.with_extension("srt"), project::to_srt(&proj, &pages, false))?;
    std::fs::write(video.with_extension("kolor.srt"), project::to_srt(&proj, &pages, true))?;

    if let Some(i) = args.iter().position(|a| a == "--frame") {
        let t: f64 = args.get(i + 1).context("czas")?.parse()?;
        let out = args.get(i + 2).context("plik")?;
        let mut r = media::VideoReader::open(&video)?;
        let mut last = None;
        let mut f = r.frame_at(tl.out_to_src(t), &mut last, info.fps)?.context("brak klatki")?;
        let rend = render::CaptionRenderer::new(proj.style.font.as_deref())?;
        if let Some(pg) = project::page_at(&pages, t) {
            if let Some(ov) = rend.render(&proj, pg, t, f.w, f.h) {
                render::composite_yuv(&mut f, &ov);
            }
        }
        preview::write_ppm(&f, std::path::Path::new(out))?;
        eprintln!("Zapisano klatkę {out}");
    }
    if let Some(out) = get("--out") {
        let t0 = std::time::Instant::now();
        let opts = export::ExportOptions { burn_captions: true, cut_pauses: !has("--no-cut"), crf: 21.0 };
        export::export_mp4(&proj, &info, audio.as_ref(), std::path::Path::new(&out), &opts, &|p, s| eprint!("\r{s} {:3.0}%   ", p * 100.0), &AtomicBool::new(false))?;
        eprintln!("\nZapisano {out} w {:.1} s", t0.elapsed().as_secs_f64());
    }
    proj.save()?;
    Ok(())
}
