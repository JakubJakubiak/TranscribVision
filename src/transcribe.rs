//! Rozpoznawanie mowy: whisper.cpp przez whisper-rs (GPU Metal na Macu),
//! słowa z czasami, potem dociągnięcie granic słów do rzeczywistego dźwięku.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use whisper_rs::{DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::project::Word;

pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub mb: u32,
}

/// Modele ggml z huggingface.co/ggerganov/whisper.cpp.
pub const MODELS: &[ModelInfo] = &[
    ModelInfo { id: "large-v3-turbo-q5_0", label: "large-v3-turbo q5 (polecany, dobry polski)", mb: 547 },
    ModelInfo { id: "large-v3-turbo", label: "large-v3-turbo (pełny)", mb: 1620 },
    ModelInfo { id: "medium", label: "medium", mb: 1530 },
    ModelInfo { id: "small", label: "small", mb: 488 },
    ModelInfo { id: "base", label: "base (szybki, słabszy)", mb: 148 },
    ModelInfo { id: "tiny", label: "tiny (najszybszy)", mb: 78 },
];

pub const LANGUAGES: &[(&str, &str)] = &[
    ("auto", "Wykryj automatycznie"),
    ("pl", "Polski"),
    ("en", "Angielski"),
    ("de", "Niemiecki"),
    ("uk", "Ukraiński"),
    ("es", "Hiszpański"),
    ("fr", "Francuski"),
    ("it", "Włoski"),
    ("ru", "Rosyjski"),
    ("ja", "Japoński"),
];

pub fn models_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("TranscribVision").join("models")
}

pub fn model_path(id: &str) -> PathBuf {
    models_dir().join(format!("ggml-{id}.bin"))
}

pub fn model_installed(id: &str) -> bool {
    model_path(id).metadata().map(|m| m.len() > 1_000_000).unwrap_or(false)
}

/// Pobierz model (z postępem 0..1). Plik pojawia się dopiero po pełnym pobraniu.
pub fn download_model(id: &str, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<PathBuf> {
    use std::io::{Read, Write};
    let url = format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{id}.bin");
    std::fs::create_dir_all(models_dir())?;
    let dest = model_path(id);
    let part = dest.with_extension("part");
    let resp = ureq::get(&url).call().map_err(|e| anyhow!("pobieranie {url}: {e}"))?;
    let total: u64 = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = resp.into_body();
    let mut reader = body.as_reader();
    let mut file = std::fs::File::create(&part)?;
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&part);
            bail!("przerwano");
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if total > 0 {
            progress(done as f32 / total as f32);
        }
    }
    file.flush()?;
    std::fs::rename(&part, &dest)?;
    Ok(dest)
}

pub struct Options {
    pub language: String,
    pub translate: bool,
    pub beam: bool,
    /// Podpowiedź dla Whispera: nazwy własne i trudne słowa (initial prompt).
    pub prompt: String,
}

/// Transkrypcja mono 16 kHz na GPU; gdy Metal zawiedzie (np. przy uśpionym GPU), powtarza na CPU.
pub fn transcribe(audio: &[f32], model: &Path, opts: &Options, progress: Arc<dyn Fn(f32) + Send + Sync>, cancel: Arc<AtomicBool>) -> Result<Vec<Word>> {
    let gpu = std::env::var_os("TV_CPU").is_none();
    match transcribe_on(audio, model, opts, progress.clone(), cancel.clone(), gpu) {
        Err(e) if gpu && !cancel.load(Ordering::Relaxed) => {
            eprintln!("GPU zawiodło ({e}), ponawiam na CPU");
            transcribe_on(audio, model, opts, progress, cancel, false)
        }
        r => r,
    }
}

fn transcribe_on(audio: &[f32], model: &Path, opts: &Options, progress: Arc<dyn Fn(f32) + Send + Sync>, cancel: Arc<AtomicBool>, gpu: bool) -> Result<Vec<Word>> {
    if std::env::var_os("TV_DEBUG").is_none() {
        whisper_rs::install_logging_hooks();
    }
    let mut cp = WhisperContextParameters::default();
    cp.use_gpu(gpu);
    // DTW: dokładne czasy słów z wyrównania uwagi (dobrane do modelu po nazwie pliku).
    if let Some(preset) = dtw_preset(model) {
        cp.dtw_parameters(DtwParameters { mode: DtwMode::ModelPreset { model_preset: preset }, ..Default::default() });
    }
    let ctx = WhisperContext::new_with_params(model.to_str().context("ścieżka modelu")?, cp).map_err(|e| anyhow!("wczytanie modelu: {e:?}"))?;
    let mut state = ctx.create_state().map_err(|e| anyhow!("{e:?}"))?;

    let strategy = if opts.beam { SamplingStrategy::BeamSearch { beam_size: 5, patience: -1.0 } } else { SamplingStrategy::Greedy { best_of: 1 } };
    let mut p = FullParams::new(strategy);
    let lang = if opts.language == "auto" { None } else { Some(opts.language.as_str()) };
    p.set_language(lang);
    p.set_translate(opts.translate);
    if !opts.prompt.trim().is_empty() {
        p.set_initial_prompt(opts.prompt.trim());
    }
    p.set_token_timestamps(true);
    p.set_split_on_word(true);
    p.set_max_len(1);
    p.set_print_progress(false);
    p.set_print_realtime(false);
    p.set_print_special(false);
    p.set_print_timestamps(false);
    p.set_suppress_blank(true);
    p.set_n_threads(std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4).min(8));
    let pr = progress.clone();
    p.set_progress_callback_safe(move |pct: i32| pr(pct as f32 / 100.0));
    // Własne przerywanie: `set_abort_callback_safe` z whisper-rs 0.16 ma błąd (trampolina czyta
    // `Box<Box<dyn FnMut>>` jako `F` i zwraca losową wartość, przez co koder „przerywa się” sam).
    unsafe extern "C" fn abort_cb(user_data: *mut std::ffi::c_void) -> bool {
        // SAFETY: wskaźnik na AtomicBool z Arc `cancel`, który żyje dłużej niż wywołanie `full`.
        unsafe { (*(user_data as *const AtomicBool)).load(Ordering::Relaxed) }
    }
    // SAFETY: `cancel` jest trzymany do końca funkcji, a `abort_cb` tylko odczytuje AtomicBool.
    unsafe {
        p.set_abort_callback(Some(abort_cb));
        p.set_abort_callback_user_data(Arc::as_ptr(&cancel) as *mut std::ffi::c_void);
    }

    state.full(p, audio).map_err(|e| if cancel.load(Ordering::Relaxed) { anyhow!("przerwano") } else { anyhow!("whisper: {e:?}") })?;

    let eot = ctx.token_eot();
    let mut words: Vec<Word> = Vec::new();
    for seg in state.as_iter() {
        let raw = seg.to_str_lossy().map(|s| s.into_owned()).unwrap_or_default();
        if raw.trim().is_empty() || raw.contains("[_") {
            continue;
        }
        let mut t0 = seg.start_timestamp() as f64 / 100.0;
        let t1 = seg.end_timestamp() as f64 / 100.0;
        // Początek słowa z DTW (pierwszy token tekstu), jeśli dostępny.
        let dtw = (0..seg.n_tokens()).filter_map(|i| seg.get_token(i)).map(|t| t.token_data()).find(|d| d.id < eot && d.t_dtw >= 0).map(|d| d.t_dtw as f64 / 100.0);
        if let Some(d) = dtw {
            t0 = d;
        }
        let text = raw.trim().to_string();
        let joins = !raw.starts_with(' ') || text.chars().all(|c| c.is_ascii_punctuation() || "…„”«»".contains(c));
        match words.last_mut() {
            Some(prev) if joins => {
                prev.text.push_str(&text);
                prev.end = prev.end.max(t1);
            }
            _ => words.push(Word { text, start: t0, end: t1.max(t0 + 0.02), speaker: 0, cut: false, prob: 1.0 - seg.no_speech_probability() }),
        }
    }
    words.retain(|w| !w.text.trim().is_empty());
    if dtw_preset(model).is_some() {
        // DTW daje początki słów; koniec = początek następnego (ciszę przytnie refine_bounds).
        for i in 0..words.len() {
            let next = words.get(i + 1).map(|n| n.start);
            let w = &mut words[i];
            w.end = match next {
                Some(n) if n > w.start + 0.02 => n,
                _ => w.end.max(w.start + 0.25),
            };
        }
    }
    refine_bounds(&mut words, audio, 16_000);
    progress(1.0);
    Ok(words)
}

/// Energia (RMS) w ramkach 10 ms.
pub fn rms_frames(audio: &[f32], rate: u32) -> Vec<f32> {
    let hop = (rate / 100) as usize;
    audio.chunks(hop).map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt()).collect()
}

/// Zwęża słowa do fragmentów z dźwiękiem: Whisper często „rozciąga” słowo na ciszę po nim,
/// przez co pauzy wyglądają na krótsze, niż są.
pub fn refine_bounds(words: &mut [Word], audio: &[f32], rate: u32) {
    let rms = rms_frames(audio, rate);
    if rms.is_empty() {
        return;
    }
    let mut sorted = rms.clone();
    sorted.sort_by(f32::total_cmp);
    let floor = sorted[sorted.len() / 10];
    let peak = sorted[sorted.len() * 98 / 100].max(1e-6);
    let thr = (floor * 3.0).max(peak * 0.04);
    let fr = |t: f64| ((t * 100.0) as usize).min(rms.len() - 1);
    for w in words.iter_mut() {
        let (mut a, mut b) = (fr(w.start), fr(w.end));
        if b <= a {
            continue;
        }
        // Słowo bez żadnego dźwięku zostaw w spokoju.
        if !rms[a..=b].iter().any(|&r| r > thr) {
            continue;
        }
        while a < b && rms[a] < thr {
            a += 1;
        }
        // Cisza ≥ 0.2 s w środku słowa: słowo kończy się przed nią (DTW daje tylko początki,
        // więc koniec słowa przed pauzą sięga do początku następnego).
        let mut run = 0;
        for f in a..=b {
            if rms[f] < thr {
                run += 1;
                if run >= 20 {
                    b = f - run;
                    break;
                }
            } else {
                run = 0;
            }
        }
        while b > a && rms[b] < thr {
            b -= 1;
        }
        // Margines 30 ms, żeby nie ucinać spółgłosek.
        let ns = (a as f64 / 100.0 - 0.03).max(w.start);
        let ne = ((b + 1) as f64 / 100.0 + 0.03).min(w.end);
        if ne - ns >= 0.06 {
            w.start = ns;
            w.end = ne;
        }
    }
    // Spóźniony początek (mowa trwa już przed nim): cofnij do początku dźwięku, najwyżej 0.3 s
    // i nie przed koniec poprzedniego słowa.
    for i in 0..words.len() {
        let prev_end = if i > 0 { words[i - 1].end } else { 0.0 };
        let from = fr(words[i].start + 0.03);
        let limit = from.saturating_sub(30);
        // Przeskakuj krótkie przerwy w słowie (zwarcia spółgłosek) do 80 ms.
        let (mut j, mut first, mut gap) = (from, from, 0);
        while j > limit && j > 0 && (j - 1) as f64 / 100.0 > prev_end {
            j -= 1;
            if rms[j] >= thr {
                first = j;
                gap = 0;
            } else {
                gap += 1;
                if gap > 8 {
                    break;
                }
            }
        }
        let ns = (first as f64 / 100.0 - 0.03).max(prev_end);
        if ns < words[i].start {
            words[i].start = ns;
        }
    }
}

fn dtw_preset(model: &Path) -> Option<DtwModelPreset> {
    let name = model.file_stem()?.to_str()?.to_ascii_lowercase();
    let name = name.trim_start_matches("ggml-");
    let base = name.split(['-', '_', '.']).collect::<Vec<_>>();
    Some(match () {
        _ if name.starts_with("large-v3-turbo") => DtwModelPreset::LargeV3Turbo,
        _ if name.starts_with("large-v3") => DtwModelPreset::LargeV3,
        _ if name.starts_with("large-v2") => DtwModelPreset::LargeV2,
        _ if name.starts_with("large-v1") || name == "large" => DtwModelPreset::LargeV1,
        _ if base.first() == Some(&"medium") => if name.contains(".en") { DtwModelPreset::MediumEn } else { DtwModelPreset::Medium },
        _ if base.first() == Some(&"small") => if name.contains(".en") { DtwModelPreset::SmallEn } else { DtwModelPreset::Small },
        _ if base.first() == Some(&"base") => if name.contains(".en") { DtwModelPreset::BaseEn } else { DtwModelPreset::Base },
        _ if base.first() == Some(&"tiny") => if name.contains(".en") { DtwModelPreset::TinyEn } else { DtwModelPreset::Tiny },
        _ => return None,
    })
}
