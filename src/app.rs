//! Okno aplikacji (egui + wgpu), nowoczesny ciemny wygląd z kartami i gradientami.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, Vec2};

use crate::export::{ExportOptions, export_mp4};
use crate::media::{self, Audio, MediaInfo};
use crate::player::Player;
use crate::preview::{self, PreviewCallback, PreviewData, SharedPreview};
use crate::project::{self, Anim, Page, Project, Timeline, Word};
use crate::render::{CaptionRenderer, SYSTEM_FONTS};
use crate::ui::{self, ACCENT, ACCENT2, BG, BORDER, CARD, CARD_HI, DANGER, MUTED, PANEL, TEXT};
use crate::{speakers, transcribe};

const FILLERS: &[&str] = &["yyy", "yy", "eee", "ee", "eh", "ehm", "em", "emm", "mmm", "hmm", "hm", "mhm", "um", "uh", "uhm", "erm", "ah", "aaa"];
const VIDEO_EXT: &[&str] = &["mp4", "mov", "m4v", "m4a", "wav", "mp3"];

pub fn run(file: Option<PathBuf>) -> Result<()> {
    let opts = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1480.0, 920.0])
            .with_min_inner_size([1100.0, 680.0])
            .with_title("TranscribVision")
            // Przezroczysty pasek tytułu: treść sięga pod przyciski okna (macOS).
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false),
        ..Default::default()
    };
    eframe::run_native("TranscribVision", opts, Box::new(move |cc| Ok(Box::new(App::new(cc, file))))).map_err(|e| anyhow::anyhow!("{e}"))
}

enum JobMsg {
    Progress(f32, String),
    Words(Vec<Word>, usize),
    Downloaded(String),
    Exported(PathBuf),
    Failed(String),
}

struct Job {
    title: String,
    progress: f32,
    stage: String,
    cancel: Arc<AtomicBool>,
}

struct Loaded {
    info: MediaInfo,
    audio: Option<Arc<Audio>>,
    audio16: Arc<Vec<f32>>,
    /// Szczyt głośności w ramkach 10 ms (do rysowania fali).
    peaks: Vec<f32>,
    project: Project,
    player: Player,
}

pub struct App {
    grad: TextureHandle,
    preview: SharedPreview,
    loaded: Option<Loaded>,
    renderer: Option<CaptionRenderer>,
    renderer_font: Option<Option<PathBuf>>,
    tl: Timeline,
    pages: Vec<Page>,
    dirty: bool,
    overlay_key: Option<(usize, i64, u64)>,
    style_ver: u64,
    model: String,
    num_speakers: usize,
    beam: bool,
    preview_cuts: bool,
    speed: f64,
    burn: bool,
    job: Option<Job>,
    tx: Sender<JobMsg>,
    rx: Receiver<JobMsg>,
    status: String,
    status_error: bool,
    editing: Option<(usize, String)>,
    pending_open: Option<PathBuf>,
}

impl App {
    fn new(cc: &eframe::CreationContext, file: Option<PathBuf>) -> Self {
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            preview::init(rs);
        }
        let grad = ui::install(&cc.egui_ctx);
        let (tx, rx) = channel();
        let model = transcribe::MODELS.iter().find(|m| transcribe::model_installed(m.id)).map_or(transcribe::MODELS[0].id, |m| m.id).to_string();
        App {
            grad,
            preview: Arc::new(Mutex::new(PreviewData::default())),
            loaded: None,
            renderer: None,
            renderer_font: None,
            tl: Timeline::identity(0.0),
            pages: Vec::new(),
            dirty: true,
            overlay_key: None,
            style_ver: 0,
            model,
            num_speakers: 1,
            beam: false,
            preview_cuts: true,
            speed: 1.0,
            burn: true,
            job: None,
            tx,
            rx,
            status: String::new(),
            status_error: false,
            editing: None,
            pending_open: file,
        }
    }

    fn set_status(&mut self, s: impl Into<String>, error: bool) {
        self.status = s.into();
        self.status_error = error;
    }

    fn open(&mut self, path: &Path, ctx: &egui::Context) {
        let res = (|| -> Result<Loaded> {
            let info = media::probe(path)?;
            let audio = media::load_audio(path).ok().map(Arc::new);
            let audio16 = Arc::new(audio.as_ref().map(|a| a.to_mono_16k()).unwrap_or_default());
            let peaks: Vec<f32> = audio16.chunks(160).map(|c| c.iter().fold(0f32, |m, s| m.max(s.abs()))).collect();
            if let Ok(mut d) = self.preview.lock() {
                *d = PreviewData::default();
            }
            let player = Player::new(&info, audio.clone(), self.preview.clone(), ctx.clone());
            let project = Project::load(path).unwrap_or_else(|| Project::new(path.into()));
            Ok(Loaded { info, audio, audio16, peaks, project, player })
        })();
        match res {
            Ok(l) => {
                self.num_speakers = l.project.speakers.len().max(1);
                let msg = if l.project.words.is_empty() { "Wideo wczytane. Wybierz język i kliknij „Transkrybuj”.".to_string() } else { format!("Wczytano zapisany projekt: {} słów.", l.project.words.len()) };
                self.set_status(msg, false);
                self.loaded = Some(l);
                self.dirty = true;
                self.overlay_key = None;
            }
            Err(e) => self.set_status(format!("Nie udało się otworzyć: {e}"), true),
        }
    }

    fn pick_and_open(&mut self, ctx: &egui::Context) {
        if let Some(f) = rfd::FileDialog::new().add_filter("Wideo / audio", VIDEO_EXT).pick_file() {
            self.open(&f, ctx);
        }
    }

    fn refresh(&mut self) {
        let Some(l) = self.loaded.as_mut() else { return };
        if self.dirty {
            let full = l.project.timeline(l.info.duration);
            let tl = if self.preview_cuts { full } else { Timeline::identity(l.info.duration) };
            self.pages = l.project.pages(&tl);
            l.player.set_timeline(tl.clone());
            self.tl = tl;
            self.dirty = false;
            self.style_ver += 1;
        }
        let font = l.project.style.font.clone();
        if self.renderer_font.as_ref() != Some(&font) {
            match CaptionRenderer::new(font.as_deref()) {
                Ok(r) => self.renderer = Some(r),
                Err(e) => {
                    self.status = format!("{e}");
                    self.status_error = true;
                    l.project.style.font = None;
                }
            }
            self.renderer_font = Some(font);
            self.style_ver += 1;
        }
    }

    fn update_overlay(&mut self) {
        let Some(l) = self.loaded.as_ref() else { return };
        let Some(r) = self.renderer.as_ref() else { return };
        let Some((w, h)) = self.preview.lock().ok().and_then(|d| d.frame.as_ref().map(|f| (f.w, f.h))) else { return };
        let t = l.player.shared.pos();
        let idx = self.pages.partition_point(|p| p.start <= t).checked_sub(1).filter(|&i| t < self.pages[i].end);
        let key = (idx.unwrap_or(usize::MAX), (t * 120.0) as i64, self.style_ver);
        if self.overlay_key == Some(key) {
            return;
        }
        if let Some(prev) = self.overlay_key {
            if prev.0 == key.0 && prev.2 == key.2 && idx.is_none() {
                return;
            }
        }
        self.overlay_key = Some(key);
        let ov = idx.and_then(|i| r.render(&l.project, &self.pages[i], t, w, h));
        if let Ok(mut d) = self.preview.lock() {
            d.set_overlay(w, h, ov.as_ref());
        }
    }

    fn start_transcription(&mut self) {
        let Some(l) = self.loaded.as_ref() else { return };
        if l.audio16.is_empty() {
            self.set_status("Plik nie ma dźwięku.", true);
            return;
        }
        let path = transcribe::model_path(&self.model);
        if !path.exists() {
            self.set_status("Najpierw pobierz model.", true);
            return;
        }
        let audio = l.audio16.clone();
        let opts = transcribe::Options { language: l.project.language.clone(), translate: l.project.translate, beam: self.beam };
        let n = self.num_speakers;
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(Job { title: "Transkrypcja".into(), progress: 0.0, stage: "Wczytywanie modelu".into(), cancel: cancel.clone() });
        std::thread::spawn(move || {
            let ptx = tx.clone();
            let prog: Arc<dyn Fn(f32) + Send + Sync> = Arc::new(move |p| {
                let _ = ptx.send(JobMsg::Progress(p * 0.95, "Rozpoznawanie mowy".into()));
            });
            match transcribe::transcribe(&audio, &path, &opts, prog, cancel) {
                Ok(mut words) => {
                    let _ = tx.send(JobMsg::Progress(0.97, "Rozpoznawanie mówców".into()));
                    speakers::diarize(&mut words, &audio, n);
                    let _ = tx.send(JobMsg::Words(words, n));
                }
                Err(e) => {
                    let _ = tx.send(JobMsg::Failed(format!("{e:#}")));
                }
            }
        });
    }

    fn start_download(&mut self) {
        let id = self.model.clone();
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(Job { title: format!("Pobieranie {id}"), progress: 0.0, stage: "huggingface.co".into(), cancel: cancel.clone() });
        std::thread::spawn(move || {
            let ptx = tx.clone();
            let r = transcribe::download_model(&id, &move |p| {
                let _ = ptx.send(JobMsg::Progress(p, "Pobieranie".into()));
            }, &cancel);
            let _ = tx.send(match r {
                Ok(_) => JobMsg::Downloaded(id),
                Err(e) => JobMsg::Failed(format!("{e:#}")),
            });
        });
    }

    fn start_export(&mut self) {
        let Some(l) = self.loaded.as_ref() else { return };
        let default = l.info.path.with_extension("").file_name().map(|n| format!("{}_napisy.mp4", n.to_string_lossy())).unwrap_or_else(|| "wynik.mp4".into());
        let Some(out) = rfd::FileDialog::new().set_file_name(&default).add_filter("MP4", &["mp4"]).save_file() else { return };
        let (proj, info, audio) = (l.project.clone(), l.info.clone(), l.audio.clone());
        let opts = ExportOptions { burn_captions: self.burn, cut_pauses: true, crf: 20.0 };
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(Job { title: "Eksport wideo".into(), progress: 0.0, stage: String::new(), cancel: cancel.clone() });
        std::thread::spawn(move || {
            let ptx = tx.clone();
            let r = export_mp4(&proj, &info, audio.as_deref(), &out, &opts, &move |p, s| {
                let _ = ptx.send(JobMsg::Progress(p, s.into()));
            }, &cancel);
            let _ = tx.send(match r {
                Ok(()) => JobMsg::Exported(out),
                Err(e) => JobMsg::Failed(format!("{e:#}")),
            });
        });
    }

    fn export_srt(&mut self) {
        let Some(l) = self.loaded.as_ref() else { return };
        let full = l.project.timeline(l.info.duration);
        let pages = l.project.pages(&full);
        let base = l.info.path.with_extension("");
        let a = base.with_extension("srt");
        let b = PathBuf::from(format!("{}_kolor.srt", base.display()));
        let r = std::fs::write(&a, project::to_srt(&l.project, &pages, false)).and_then(|_| std::fs::write(&b, project::to_srt(&l.project, &pages, true)));
        match r {
            Ok(()) => self.set_status(format!("Zapisano {} i {}", file_name(&a), file_name(&b)), false),
            Err(e) => self.set_status(format!("Błąd zapisu SRT: {e}"), true),
        }
    }

    fn save(&mut self) {
        let Some(l) = self.loaded.as_ref() else { return };
        match l.project.save() {
            Ok(p) => self.set_status(format!("Zapisano projekt {}", file_name(&p)), false),
            Err(e) => self.set_status(format!("Błąd zapisu: {e}"), true),
        }
    }

    fn poll_jobs(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                JobMsg::Progress(p, s) => {
                    if let Some(j) = self.job.as_mut() {
                        j.progress = p;
                        j.stage = s;
                    }
                }
                JobMsg::Words(words, n) => {
                    self.job = None;
                    if let Some(l) = self.loaded.as_mut() {
                        let nw = words.len();
                        l.project.words = words;
                        let old = std::mem::take(&mut l.project.speakers);
                        let mut sp = project::default_speakers(n);
                        for (i, s) in old.into_iter().enumerate().take(n) {
                            sp[i] = s;
                        }
                        l.project.speakers = sp;
                        let _ = l.project.save();
                        self.dirty = true;
                        self.set_status(format!("Gotowe: {nw} słów. Kliknij słowo, aby przejść; prawy przycisk — opcje."), false);
                    }
                }
                JobMsg::Downloaded(id) => {
                    self.job = None;
                    self.set_status(format!("Model {id} pobrany."), false);
                }
                JobMsg::Exported(p) => {
                    self.job = None;
                    self.set_status(format!("Zapisano {}", p.display()), false);
                }
                JobMsg::Failed(e) => {
                    self.job = None;
                    self.set_status(format!("Błąd: {e}"), true);
                }
            }
        }
    }

    fn seek_src(&self, src: f64) {
        if let Some(l) = self.loaded.as_ref() {
            l.player.shared.set_pos(self.tl.src_to_out_nearest(src));
        }
    }

    // ───────────── Pasek górny ─────────────

    fn top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let bar = ui.max_rect();
        // Przeciąganie okna za pusty obszar paska.
        let drag = ui.interact(bar, ui.scope_id().with("drag"), Sense::click_and_drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
        }
        ui.horizontal_centered(|ui| {
            ui.add_space(78.0); // miejsce na przyciski okna macOS
            let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
            ui::gradient_rect(ui, r, 7.0, &self.grad, 1.0);
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "T", egui::FontId::proportional(13.0), Color32::WHITE);
            ui.label(RichText::new("TranscribVision").size(15.0).strong());
            ui.add_space(10.0);
            if let Some(l) = self.loaded.as_ref() {
                let name = file_name(&l.info.path);
                let info = if l.info.has_video { format!("{name}  ·  {}×{}  ·  {}", l.info.width, l.info.height, fmt_time(l.info.duration)) } else { format!("{name}  ·  {}", fmt_time(l.info.duration)) };
                ui::pill(ui, &info, CARD_HI, MUTED);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(14.0);
                let has = self.loaded.as_ref().is_some_and(|l| !l.project.words.is_empty());
                let busy = self.job.is_some();
                if ui::primary_button(ui, &self.grad, "Eksportuj wideo", has && !busy, Vec2::new(150.0, 34.0)).clicked() {
                    self.start_export();
                }
                if ui::ghost_button(ui, "SRT", has).on_hover_text("Zapisz napisy SRT (zwykłe i z kolorowym słowem)").clicked() {
                    self.export_srt();
                }
                if ui::ghost_button(ui, "Zapisz", self.loaded.is_some()).on_hover_text("⌘S").clicked() {
                    self.save();
                }
                if ui::ghost_button(ui, "Otwórz…", true).on_hover_text("⌘O").clicked() {
                    self.pick_and_open(ctx);
                }
                ui.add_space(6.0);
                ui::toggle(ui, &mut self.burn).on_hover_text("Wypal napisy w obrazie przy eksporcie");
                ui.label(RichText::new("Wypal napisy").color(MUTED));
            });
        });
    }

    // ───────────── Lewy panel: kroki ─────────────

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(4.0);
            // ── 1. Transkrypcja
            let mut go = false;
            let mut download = false;
            ui::card(ui, |ui| {
                ui::card_header(ui, &self.grad, "1", "Transkrypcja", "Whisper na GPU, czasy każdego słowa");
                ui.label(RichText::new("Model").color(MUTED).size(12.5));
                let cur = transcribe::MODELS.iter().find(|m| m.id == self.model).map_or(self.model.clone(), |m| m.label.to_string());
                egui::ComboBox::from_id_salt("model").width(ui.available_width()).selected_text(cur).show_ui(ui, |ui| {
                    for m in transcribe::MODELS {
                        let ok = transcribe::model_installed(m.id);
                        let txt = RichText::new(format!("{}  {}  ·  {} MB", if ok { "✔" } else { "  " }, m.label, m.mb)).color(if ok { TEXT } else { MUTED });
                        ui.selectable_value(&mut self.model, m.id.to_string(), txt);
                    }
                });
                let installed = transcribe::model_installed(&self.model);
                if !installed {
                    let mb = transcribe::MODELS.iter().find(|m| m.id == self.model).map_or(0, |m| m.mb);
                    download = ui::ghost_button(ui, &format!("↓  Pobierz model  ·  {mb} MB"), !busy).clicked();
                }
                let Some(l) = self.loaded.as_mut() else {
                    ui.label(RichText::new("Najpierw otwórz wideo.").color(MUTED));
                    return;
                };
                ui.add_space(2.0);
                ui.label(RichText::new("Język").color(MUTED).size(12.5));
                let lang = transcribe::LANGUAGES.iter().find(|(c, _)| *c == l.project.language).map_or(l.project.language.clone(), |(_, n)| n.to_string());
                egui::ComboBox::from_id_salt("lang").width(ui.available_width()).selected_text(lang).show_ui(ui, |ui| {
                    for (c, n) in transcribe::LANGUAGES {
                        ui.selectable_value(&mut l.project.language, c.to_string(), *n);
                    }
                });
                ui.add_space(2.0);
                ui::toggle_row(ui, "Tłumacz na angielski", &mut l.project.translate);
                ui::toggle_row(ui, "Dokładniej (wolniej)", &mut self.beam);
                ui.horizontal(|ui| {
                    ui.label("Liczba mówców");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::icon_button(ui, "+", 26.0).clicked() {
                            self.num_speakers = (self.num_speakers + 1).min(6);
                        }
                        ui.label(RichText::new(self.num_speakers.to_string()).size(15.0).strong());
                        if ui::icon_button(ui, "−", 26.0).clicked() {
                            self.num_speakers = self.num_speakers.saturating_sub(1).max(1);
                        }
                    });
                });
                ui.add_space(6.0);
                let w = ui.available_width();
                go = ui::primary_button(ui, &self.grad, if l.project.words.is_empty() { "Transkrybuj" } else { "Transkrybuj ponownie" }, !busy && installed, Vec2::new(w, 40.0)).clicked();
                if self.num_speakers > 1 && !l.project.words.is_empty() && ui::ghost_button(ui, "Wykryj mówców ponownie", !busy).clicked() {
                    speakers::diarize(&mut l.project.words, &l.audio16, self.num_speakers);
                    while l.project.speakers.len() < self.num_speakers {
                        let i = l.project.speakers.len();
                        l.project.speakers.push(project::default_speakers(i + 1).pop().expect("mówca"));
                    }
                    self.dirty = true;
                }
            });
            if download {
                self.start_download();
            }
            if go {
                self.start_transcription();
            }
            let Some(l) = self.loaded.as_mut() else { return };

            ui.add_space(10.0);
            // ── 2. Pauzy
            ui::card(ui, |ui| {
                ui::card_header(ui, &self.grad, "2", "Pauzy", "Wycinanie ciszy i wtrąceń");
                let mut ch = ui::toggle_row(ui, "Wycinaj pauzy", &mut l.project.cuts.enabled);
                ch |= ui::slider_row(ui, "Dłuższe niż", &mut l.project.cuts.min_pause, 0.15..=2.0, |v| format!("{v:.2} s"));
                ch |= ui::slider_row(ui, "Zostaw ciszy", &mut l.project.cuts.padding, 0.0..=0.5, |v| format!("{v:.2} s"));
                if ui::ghost_button(ui, "Wytnij „yyy”, „eee”…", !l.project.words.is_empty()).clicked() {
                    let mut n = 0;
                    for w in &mut l.project.words {
                        let norm: String = w.text.to_lowercase().chars().filter(|c| c.is_alphabetic()).collect();
                        if FILLERS.contains(&norm.as_str()) && !w.cut {
                            w.cut = true;
                            n += 1;
                        }
                    }
                    self.status = format!("Wycięto {n} wtrąceń.");
                    self.status_error = false;
                    ch = true;
                }
                if ch {
                    self.dirty = true;
                }
                // Statystyka: przed → po.
                let full = l.project.timeline(l.info.duration);
                ui.add_space(4.0);
                egui::Frame::new().fill(CARD_HI).corner_radius(10).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        stat(ui, "Przed", &fmt_time(l.info.duration), TEXT);
                        ui.label(RichText::new("→").color(MUTED).size(16.0));
                        stat(ui, "Po", &fmt_time(full.total), Color32::WHITE);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui::pill(ui, &format!("−{:.1} s", full.removed(l.info.duration)), ACCENT2.gamma_multiply(0.25), Color32::from_rgb(255, 170, 210));
                        });
                    });
                });
            });

            ui.add_space(10.0);
            // ── 3. Napisy
            ui::card(ui, |ui| {
                ui::card_header(ui, &self.grad, "3", "Napisy", "Animacja, czcionka i kolory");
                let st = &mut l.project.style;
                let mut sc = false;
                ui.label(RichText::new("Animacja").color(MUTED).size(12.5));
                let tw = (ui.available_width() - 8.0) / 2.0;
                let subs = |a: Anim| match a {
                    Anim::Pop => "słowo wyskakuje",
                    Anim::Karaoke => "wypełnia się kolorem",
                    Anim::Box => "kolorowe tło słowa",
                    Anim::Bounce => "słowo po słowie",
                    Anim::Color => "tylko kolor",
                };
                let names = |a: Anim| match a {
                    Anim::Pop => "Pop",
                    Anim::Karaoke => "Karaoke",
                    Anim::Box => "Ramka",
                    Anim::Bounce => "Odbicie",
                    Anim::Color => "Kolor",
                };
                for row in Anim::ALL.chunks(2) {
                    ui.horizontal(|ui| {
                        for &a in row {
                            if ui::tile(ui, &self.grad, names(a), subs(a), st.anim == a, Vec2::new(tw, 46.0)).clicked() {
                                st.anim = a;
                                sc = true;
                            }
                        }
                    });
                }
                ui.add_space(4.0);
                ui.label(RichText::new("Czcionka").color(MUTED).size(12.5));
                let cur = match &st.font {
                    None => "Arial Black".to_string(),
                    Some(p) => SYSTEM_FONTS.iter().find(|(_, fp)| Path::new(fp) == p).map_or_else(|| file_name(p), |(n, _)| n.to_string()),
                };
                egui::ComboBox::from_id_salt("font").width(ui.available_width()).selected_text(cur).show_ui(ui, |ui| {
                    for (n, p) in SYSTEM_FONTS {
                        if Path::new(p).exists() && ui.selectable_label(st.font.as_deref() == Some(Path::new(p)), *n).clicked() {
                            st.font = Some(PathBuf::from(p));
                        }
                    }
                    if ui.button("Z pliku…").clicked() {
                        if let Some(f) = rfd::FileDialog::new().add_filter("Czcionka", &["ttf", "otf"]).pick_file() {
                            st.font = Some(f);
                        }
                    }
                });
                sc |= ui::slider_row(ui, "Rozmiar", &mut st.size, 0.03..=0.16, |v| format!("{:.0}%", v * 100.0));
                sc |= ui::slider_row(ui, "Pozycja", &mut st.pos_y, 0.08..=0.95, |v| format!("{:.0}%", v * 100.0));
                sc |= ui::slider_row(ui, "Obrys", &mut st.outline, 0.0..=0.3, |v| format!("{:.0}%", v * 100.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Słów na planszy").color(MUTED).size(12.5));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        sc |= ui.add(egui::DragValue::new(&mut st.max_words).range(1..=12)).changed();
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Znaków na planszy").color(MUTED).size(12.5));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        sc |= ui.add(egui::DragValue::new(&mut st.max_chars).range(4..=80)).changed();
                    });
                });
                sc |= ui::toggle_row(ui, "WIELKIE LITERY", &mut st.uppercase);
                sc |= ui::toggle_row(ui, "Cień", &mut st.shadow);
                sc |= ui::toggle_row(ui, "Kolor według mówcy", &mut st.color_by_speaker);
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    sc |= swatch(ui, "Tekst", &mut st.text_color);
                    sc |= swatch(ui, "Obrys", &mut st.outline_color);
                    if !st.color_by_speaker {
                        sc |= swatch(ui, "Podświetlenie", &mut st.highlight);
                    }
                });
                if st.color_by_speaker {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Mówcy").color(MUTED).size(12.5));
                    for sp in &mut l.project.speakers {
                        ui.horizontal(|ui| {
                            sc |= ui.color_edit_button_srgba_unmultiplied(&mut sp.color).changed();
                            sc |= ui.add(egui::TextEdit::singleline(&mut sp.name).desired_width(ui.available_width()).margin(egui::Margin::symmetric(8, 5))).changed();
                        });
                    }
                }
                if sc {
                    self.dirty = true;
                }
            });
            ui.add_space(12.0);
        });
    }

    // ───────────── Prawy panel: transkrypcja ─────────────

    fn transcript_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Transkrypcja").size(17.0).strong());
            if let Some(l) = self.loaded.as_ref() {
                if !l.project.words.is_empty() {
                    ui::pill(ui, &format!("{} słów", l.project.words.len()), CARD_HI, MUTED);
                }
            }
        });
        ui.label(RichText::new("Klik — przejdź  ·  2× klik — edytuj  ·  prawy — wytnij, mówca").size(11.5).color(MUTED));
        ui.add_space(6.0);
        let Some(l) = self.loaded.as_mut() else { return };
        if l.project.words.is_empty() {
            empty_hint(ui, "Tu pojawi się tekst", "Po transkrypcji każde słowo można kliknąć, wyciąć albo przypisać innemu mówcy.");
            return;
        }
        let t_src = self.tl.out_to_src(l.player.shared.pos());
        let mut seek: Option<f64> = None;
        let mut changed = false;
        let mut action: Option<(usize, WordAction)> = None;
        let nsp = l.project.speakers.len();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut i = 0;
            let words = &l.project.words;
            while i < words.len() {
                // Wypowiedź jednego mówcy jako osobna karta.
                let spk = words[i].speaker;
                let mut j = i + 1;
                while j < words.len() && (words[j].speaker == spk || nsp <= 1) && !(nsp <= 1 && words[j - 1].text.ends_with(['.', '?', '!']) && j - i > 25) {
                    j += 1;
                }
                let color = l.project.speakers.get(spk).map_or(ACCENT, |s| ui::rgb(s.color));
                egui::Frame::new().fill(CARD).corner_radius(12).inner_margin(egui::Margin::same(12)).stroke(Stroke::new(1.0, BORDER)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 5.0, color);
                        let name = l.project.speakers.get(spk).map_or("?", |s| s.name.as_str());
                        if nsp > 1 {
                            ui.label(RichText::new(name).strong().color(color));
                        }
                        ui.label(RichText::new(fmt_time(words[i].start)).size(11.5).color(MUTED).monospace());
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(1.0, 3.0);
                        ui.spacing_mut().button_padding = Vec2::new(5.0, 2.0);
                        for k in i..j {
                            let w = &words[k];
                            if let Some((ei, buf)) = self.editing.as_mut() {
                                if *ei == k {
                                    let r = ui.add(egui::TextEdit::singleline(buf).desired_width(110.0));
                                    r.request_focus();
                                    if r.lost_focus() {
                                        action = Some((k, WordAction::SetText(buf.trim().to_string())));
                                    }
                                    continue;
                                }
                            }
                            let active = t_src >= w.start && t_src < w.end;
                            let mut text = RichText::new(&w.text).size(14.5);
                            let mut fill = Color32::TRANSPARENT;
                            if w.cut {
                                text = text.strikethrough().color(DANGER.gamma_multiply(0.85));
                            } else if active {
                                text = text.color(Color32::BLACK);
                                fill = color;
                            } else if w.prob < 0.4 {
                                text = text.color(MUTED);
                            } else {
                                text = text.color(TEXT);
                            }
                            let b = egui::Button::new(text).fill(fill).stroke(Stroke::NONE).corner_radius(6).min_size(Vec2::new(0.0, 24.0));
                            let r = ui.add(b);
                            if r.clicked() {
                                seek = Some(w.start);
                            }
                            if r.double_clicked() {
                                action = Some((k, WordAction::Edit));
                            }
                            let r = r.on_hover_text(format!("{:.2}–{:.2} s", w.start, w.end));
                            r.context_menu(|ui| {
                                if ui.button(if w.cut { "Przywróć słowo" } else { "Wytnij słowo" }).clicked() {
                                    action = Some((k, WordAction::ToggleCut));
                                    ui.close();
                                }
                                if ui.button("Edytuj tekst").clicked() {
                                    action = Some((k, WordAction::Edit));
                                    ui.close();
                                }
                                if ui.button("Połącz z następnym").clicked() {
                                    action = Some((k, WordAction::Merge));
                                    ui.close();
                                }
                                if nsp > 1 {
                                    ui.separator();
                                    for s in 0..nsp {
                                        if ui.button(format!("● {} — od tego słowa", l.project.speakers[s].name)).clicked() {
                                            action = Some((k, WordAction::Speaker(s)));
                                            ui.close();
                                        }
                                    }
                                }
                            });
                        }
                    });
                });
                ui.add_space(6.0);
                i = j;
            }
        });
        if let Some((i, a)) = action {
            let words = &mut l.project.words;
            match a {
                WordAction::ToggleCut => words[i].cut = !words[i].cut,
                WordAction::Edit => self.editing = Some((i, words[i].text.clone())),
                WordAction::SetText(t) => {
                    if !t.is_empty() {
                        words[i].text = t;
                    }
                    self.editing = None;
                }
                WordAction::Merge => {
                    if i + 1 < words.len() {
                        let n = words.remove(i + 1);
                        words[i].text = format!("{} {}", words[i].text, n.text);
                        words[i].end = n.end;
                    }
                }
                WordAction::Speaker(s) => {
                    let old = words[i].speaker;
                    for w in words[i..].iter_mut() {
                        if w.speaker != old {
                            break;
                        }
                        w.speaker = s;
                    }
                }
            }
            changed = true;
        }
        if changed {
            self.dirty = true;
        }
        if let Some(s) = seek {
            self.seek_src(s);
        }
    }

    // ───────────── Dół: oś czasu ─────────────

    fn timeline(&mut self, ui: &mut egui::Ui) {
        let Some(l) = self.loaded.as_ref() else { return };
        let sh = l.player.shared.clone();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Oś czasu").strong());
            ui.label(RichText::new("czerwone = wycięte").size(11.5).color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui::toggle(ui, &mut self.preview_cuts).changed() {
                    self.dirty = true;
                }
                ui.label(RichText::new("Podgląd bez pauz").color(MUTED));
            });
        });
        let dur = l.info.duration.max(0.01);
        let avail = ui.available_size();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(avail.x, avail.y.max(70.0) - 6.0), Sense::click_and_drag());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 12.0, CARD);
        ui::stroke_rect(ui, rect, 12.0, Stroke::new(1.0, BORDER));
        let inner = rect.shrink2(Vec2::new(10.0, 8.0));
        let x_of = |t: f64| inner.left() + (t / dur) as f32 * inner.width();
        let full = l.project.timeline(l.info.duration);
        let kept = |t: f64| full.segs.iter().any(|s| t >= s.src0 && t < s.src1);
        // Wycięte fragmenty.
        let mut prev_end = 0.0;
        for s in full.segs.iter().map(|s| (s.src0, s.src1)).chain(std::iter::once((dur, dur))) {
            if s.0 > prev_end + 0.01 {
                let r = Rect::from_x_y_ranges(x_of(prev_end)..=x_of(s.0), inner.y_range());
                p.rect_filled(r, 6.0, DANGER.gamma_multiply(0.14));
            }
            prev_end = s.1;
        }
        // Fala dźwięku: słupki z gradientem.
        let mid = inner.top() + inner.height() * 0.42;
        let hh = inner.height() * 0.36;
        let n = l.peaks.len().max(1);
        let step = 3.0;
        let cols = (inner.width() / step) as usize;
        for c in 0..cols {
            let a = c * n / cols.max(1);
            let b = ((c + 1) * n / cols.max(1)).max(a + 1).min(n);
            let pk = l.peaks[a.min(n - 1)..b].iter().fold(0f32, |m, v| m.max(*v)).min(1.0).sqrt();
            let x = inner.left() + c as f32 * step + 1.0;
            let t = (c as f64 / cols as f64) * dur;
            let col = if kept(t) { ui::lerp(ACCENT, ACCENT2, c as f32 / cols as f32) } else { Color32::from_rgb(70, 60, 74) };
            let h = (pk * hh).max(1.0);
            p.rect_filled(Rect::from_center_size(Pos2::new(x, mid), Vec2::new(2.0, h * 2.0)), 1.0, col);
        }
        // Słowa jako „pigułki”.
        let wy = inner.bottom() - 10.0;
        for w in &l.project.words {
            let c = l.project.speakers.get(w.speaker).map_or(ACCENT, |s| ui::rgb(s.color));
            let col = if w.cut { DANGER } else { c };
            let r = Rect::from_x_y_ranges(x_of(w.start)..=(x_of(w.end) - 1.0).max(x_of(w.start) + 2.0), wy..=wy + 6.0);
            p.rect_filled(r, 3.0, col.gamma_multiply(0.9));
        }
        // Głowica.
        let px = x_of(self.tl.out_to_src(sh.pos()));
        p.line_segment([Pos2::new(px, rect.top() + 4.0), Pos2::new(px, rect.bottom() - 4.0)], Stroke::new(2.0, Color32::WHITE));
        p.circle_filled(Pos2::new(px, rect.top() + 6.0), 5.0, Color32::WHITE);
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - inner.left()) / inner.width()).clamp(0.0, 1.0) as f64 * dur;
            self.seek_src(t);
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    // ───────────── Środek: podgląd ─────────────

    fn center(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let full = ui.available_rect_before_wrap();
        let Some(l) = self.loaded.as_ref() else {
            // Ekran powitalny: strefa upuszczania.
            let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
            let zone = Rect::from_center_size(full.center(), Vec2::new(full.width().min(560.0), 340.0));
            ui.painter().rect_filled(zone, 22.0, if hovering { Color32::from_rgb(30, 24, 46) } else { CARD });
            ui::dashed_rect(ui, zone.shrink(1.0), 22.0, if hovering { ACCENT } else { BORDER });
            let c = zone.center();
            let icon = Rect::from_center_size(c - Vec2::new(0.0, 80.0), Vec2::splat(64.0));
            ui::gradient_rect(ui, icon, 20.0, &self.grad, 1.0);
            ui.painter().text(icon.center(), egui::Align2::CENTER_CENTER, "↑", egui::FontId::proportional(30.0), Color32::WHITE);
            ui.painter().text(c - Vec2::new(0.0, 14.0), egui::Align2::CENTER_CENTER, "Przeciągnij tutaj wideo", egui::FontId::proportional(22.0), TEXT);
            ui.painter().text(c + Vec2::new(0.0, 16.0), egui::Align2::CENTER_CENTER, "MP4 · MOV · M4A · WAV · MP3 — wszystko liczy się lokalnie", egui::FontId::proportional(13.0), MUTED);
            let btn = Rect::from_center_size(c + Vec2::new(0.0, 70.0), Vec2::new(190.0, 42.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(btn));
            if ui::primary_button(&mut child, &self.grad, "Wybierz plik", true, btn.size()).clicked() {
                self.pick_and_open(ctx);
            }
            return;
        };
        // Miejsce na pływający pasek odtwarzania.
        let area = Rect::from_min_max(full.min, Pos2::new(full.max.x, full.max.y - 64.0)).shrink(10.0);
        let (vw, vh) = (l.info.width.max(1) as f32, l.info.height.max(1) as f32);
        let scale = (area.width() / vw).min(area.height() / vh);
        let rect = Rect::from_center_size(area.center(), Vec2::new(vw * scale, vh * scale));
        // Cień pod kadrem.
        let shadow = egui::epaint::Shadow { offset: [0, 14], blur: 40, spread: 0, color: Color32::from_black_alpha(160) };
        ui.painter().add(shadow.as_shape(rect, CornerRadius::same(16)));
        let resp = ui.allocate_rect(rect, Sense::click());
        if l.info.has_video {
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, PreviewCallback { data: self.preview.clone() }));
            // Zaokrąglone rogi: maska w kolorze tła.
            ui.painter().rect_stroke(rect.expand(8.0), 24.0, Stroke::new(16.0, BG), egui::StrokeKind::Middle);
        } else {
            ui.painter().rect_filled(rect, 16.0, Color32::BLACK);
        }
        ui::stroke_rect(ui, rect, 16.0, Stroke::new(1.0, Color32::from_white_alpha(18)));
        if resp.clicked() {
            l.player.toggle();
        }

        // Pływający pasek odtwarzania.
        let sh = l.player.shared.clone();
        let playing = sh.playing.load(Ordering::Relaxed);
        let bar = Rect::from_center_size(Pos2::new(full.center().x, full.max.y - 34.0), Vec2::new(430.0f32.min(full.width() - 20.0), 48.0));
        let bshadow = egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(150) };
        ui.painter().add(bshadow.as_shape(bar, CornerRadius::same(24)));
        ui.painter().rect_filled(bar, 24.0, CARD_HI);
        ui::stroke_rect(ui, bar, 24.0, Stroke::new(1.0, BORDER));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(Vec2::new(8.0, 6.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let ui = &mut child;
        if ui::icon_button(ui, "|◀", 32.0).on_hover_text("Na początek").clicked() {
            sh.set_pos(0.0);
        }
        let (pr, presp) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
        ui::gradient_rect(ui, pr, 18.0, &self.grad, 1.0);
        ui.painter().text(pr.center() + Vec2::new(if playing { 0.0 } else { 1.5 }, 0.0), egui::Align2::CENTER_CENTER, if playing { "❚❚" } else { "▶" }, egui::FontId::proportional(15.0), Color32::WHITE);
        if presp.on_hover_text("Spacja").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            l.player.toggle();
        }
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{}", fmt_time(sh.pos()))).monospace().size(14.0).color(TEXT));
        ui.label(RichText::new(format!("/ {}", fmt_time(self.tl.total))).monospace().size(12.5).color(MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            for s in [2.0, 1.5, 1.0] {
                let sel = (self.speed - s).abs() < 1e-3;
                let b = egui::Button::new(RichText::new(format!("{s}×")).size(12.5).color(if sel { Color32::WHITE } else { MUTED })).fill(if sel { ACCENT.gamma_multiply(0.6) } else { Color32::TRANSPARENT }).stroke(Stroke::NONE).corner_radius(14).min_size(Vec2::new(38.0, 26.0));
                if ui.add(b).clicked() {
                    self.speed = s;
                    sh.set_speed(s);
                }
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.add_space(6.0);
            if let Some(j) = self.job.as_ref() {
                ui.label(RichText::new(&j.title).strong());
                // Pasek postępu z gradientem.
                let (r, _) = ui.allocate_exact_size(Vec2::new(240.0, 8.0), Sense::hover());
                ui.painter().rect_filled(r, 4.0, CARD_HI);
                let fill = Rect::from_min_size(r.min, Vec2::new(r.width() * j.progress.clamp(0.0, 1.0), r.height()));
                if fill.width() > 1.0 {
                    ui::gradient_rect(ui, fill, 4.0, &self.grad, 1.0);
                }
                ui.label(RichText::new(format!("{} {:.0}%", j.stage, j.progress * 100.0)).color(MUTED).size(12.5));
                if ui::ghost_button(ui, "Przerwij", true).clicked() {
                    j.cancel.store(true, Ordering::Relaxed);
                }
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            } else if !self.status.is_empty() {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, if self.status_error { DANGER } else { ui::GOOD });
                ui.label(RichText::new(&self.status).size(12.5).color(if self.status_error { Color32::from_rgb(255, 150, 150) } else { MUTED }));
            }
        });
    }
}

enum WordAction {
    ToggleCut,
    Edit,
    SetText(String),
    Merge,
    Speaker(usize),
}

fn stat(ui: &mut egui::Ui, label: &str, value: &str, color: Color32) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(RichText::new(label).size(11.0).color(MUTED));
        ui.label(RichText::new(value).size(16.0).strong().color(color).monospace());
    });
}

fn swatch(ui: &mut egui::Ui, label: &str, c: &mut [u8; 4]) -> bool {
    let mut ch = false;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ch = ui.color_edit_button_srgba_unmultiplied(c).changed();
        ui.label(RichText::new(label).size(11.0).color(MUTED));
    });
    ch
}

fn empty_hint(ui: &mut egui::Ui, title: &str, text: &str) {
    egui::Frame::new().fill(CARD).corner_radius(12).inner_margin(egui::Margin::same(16)).stroke(Stroke::new(1.0, BORDER)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title).strong());
        ui.label(RichText::new(text).color(MUTED).size(12.5));
    });
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn fmt_time(t: f64) -> String {
    let t = t.max(0.0);
    format!("{:02}:{:05.2}", (t / 60.0) as u32, t % 60.0)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(p) = self.pending_open.take() {
            self.open(&p, &ctx);
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if let Some(p) = dropped.into_iter().next() {
            self.open(&p, &ctx);
        }
        self.poll_jobs();
        self.refresh();

        let (space, cmd_o, cmd_s) = ctx.input(|i| (i.key_pressed(egui::Key::Space), i.modifiers.command && i.key_pressed(egui::Key::O), i.modifiers.command && i.key_pressed(egui::Key::S)));
        if space && !ctx.egui_wants_keyboard_input() {
            if let Some(l) = self.loaded.as_ref() {
                l.player.toggle();
            }
        }
        if cmd_o {
            self.pick_and_open(&ctx);
        }
        if cmd_s {
            self.save();
        }

        let frame = |fill: Color32, m: egui::Margin| egui::Frame::new().fill(fill).inner_margin(m);
        egui::Panel::top("top").exact_size(52.0).frame(frame(BG, egui::Margin::symmetric(8, 0))).show(ui, |ui| self.top_bar(ui, &ctx));
        egui::Panel::bottom("status").exact_size(34.0).frame(frame(BG, egui::Margin::symmetric(10, 0))).show(ui, |ui| self.status_bar(ui));
        if self.loaded.is_some() {
            egui::Panel::bottom("timeline").resizable(true).default_size(150.0).min_size(110.0).frame(frame(BG, egui::Margin::symmetric(14, 4))).show(ui, |ui| self.timeline(ui));
        }
        egui::Panel::left("left").resizable(true).default_size(340.0).min_size(300.0).frame(frame(BG, egui::Margin { left: 14, right: 6, top: 4, bottom: 4 })).show(ui, |ui| self.left_panel(ui));
        egui::Panel::right("words").resizable(true).default_size(380.0).min_size(280.0).frame(frame(BG, egui::Margin { left: 6, right: 14, top: 4, bottom: 4 })).show(ui, |ui| self.transcript_panel(ui));
        egui::CentralPanel::default().frame(frame(BG, egui::Margin::same(6))).show(ui, |ui| self.center(ui, &ctx));

        self.update_overlay();
        if self.loaded.as_ref().is_some_and(|l| l.player.shared.playing.load(Ordering::Relaxed)) {
            ctx.request_repaint();
        }
        let _ = PANEL;
    }
}
