use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use eframe::egui::{self, vec2, Color32, RichText};
use wav2midi::analysis::{Analysis, AnalysisSettings, Mode, PolyWindow, TrackSettings};
use wav2midi::audio::AudioData;
use wav2midi::midi::MidiOptions;
use wav2midi::notes::{note_name, Note};

use crate::gm;
use crate::piano_roll::{self, format_time, Overview, RollView};
use crate::player::Player;

const ACCENT: Color32 = Color32::from_rgb(98, 110, 245);
const OK_COL: Color32 = Color32::from_rgb(110, 210, 140);
const ERR_COL: Color32 = Color32::from_rgb(250, 110, 110);
const AUDIO_EXTS: [&str; 8] = ["wav", "mp3", "flac", "ogg", "m4a", "aac", "aif", "aiff"];

struct Loaded {
    path: PathBuf,
    audio: Arc<AudioData>,
    overview: Overview,
}

enum JobMsg {
    Loaded(Result<Loaded, String>),
    Analyzed {
        settings: AnalysisSettings,
        result: Result<Analysis, String>,
        secs: f64,
    },
}

struct Job {
    label: String,
    rx: mpsc::Receiver<JobMsg>,
    progress: Arc<AtomicU32>,
    started: Instant,
}

#[derive(Clone, Copy, PartialEq)]
enum StatusKind {
    Info,
    Ok,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
enum PlaySource {
    Original,
    Midi,
    Both,
}

pub struct App {
    loaded: Option<Loaded>,
    analysis: Option<(AnalysisSettings, Analysis)>,
    notes: Vec<Note>,
    notes_version: u64,
    settings: AnalysisSettings,
    track: TrackSettings,
    /// Son nota takibinin yapıldığı (ayarlar, nota aralığı)
    tracked_with: Option<(TrackSettings, u8, u8)>,
    midi: MidiOptions,
    use_fixed_velocity: bool,
    fixed_velocity: u8,
    auto_analyze: bool,
    job: Option<Job>,
    status: (StatusKind, String),
    view: RollView,
    player: Player,
    playing: Option<PlaySource>,
    last_source: PlaySource,
    synth_cache: Option<(u64, Arc<Vec<f32>>)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        setup_style(&cc.egui_ctx);
        let mut app = Self {
            loaded: None,
            analysis: None,
            notes: Vec::new(),
            notes_version: 0,
            settings: AnalysisSettings::default(),
            track: TrackSettings::default(),
            tracked_with: None,
            midi: MidiOptions::default(),
            use_fixed_velocity: false,
            fixed_velocity: 100,
            auto_analyze: true,
            job: None,
            status: (
                StatusKind::Info,
                "Bir WAV veya MP3 dosyası açın ya da pencereye sürükleyip bırakın.".into(),
            ),
            view: RollView::default(),
            player: Player::default(),
            playing: None,
            last_source: PlaySource::Midi,
            synth_cache: None,
        };
        if let Some(p) = initial {
            app.open_file(p, &cc.egui_ctx);
        }
        app
    }

    // ------------------------------------------------------------------
    // İşler (arka plan iş parçacıkları)
    // ------------------------------------------------------------------

    fn open_file(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.stop_playback();
        let (tx, rx) = mpsc::channel();
        let progress = Arc::new(AtomicU32::new(0));
        let p2 = progress.clone();
        let ctx2 = ctx.clone();
        let name = file_name(&path);
        std::thread::spawn(move || {
            let res = wav2midi::load_audio(&path, &|f| p2.store(f.to_bits(), Ordering::Relaxed))
                .map(|audio| Loaded {
                    overview: Overview::new(&audio.samples, audio.sample_rate),
                    audio: Arc::new(audio),
                    path,
                })
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(JobMsg::Loaded(res));
            ctx2.request_repaint();
        });
        self.job = Some(Job {
            label: format!("Yükleniyor: {name}"),
            rx,
            progress,
            started: Instant::now(),
        });
    }

    fn start_analysis(&mut self, ctx: &egui::Context) {
        let Some(l) = &self.loaded else { return };
        let audio = l.audio.clone();
        let settings = self.settings.clone();
        let (tx, rx) = mpsc::channel();
        let progress = Arc::new(AtomicU32::new(0));
        let p2 = progress.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let t = Instant::now();
            let result = wav2midi::analyze(&audio, &settings, &|f| {
                p2.store(f.to_bits(), Ordering::Relaxed)
            })
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(JobMsg::Analyzed {
                settings,
                result,
                secs: t.elapsed().as_secs_f64(),
            });
            ctx2.request_repaint();
        });
        let mode = match self.settings.mode {
            Mode::Ai => "yapay zekâ",
            Mode::Mono => "monofonik",
            Mode::Poly => "polifonik",
        };
        self.job = Some(Job {
            label: format!("Analiz ediliyor ({mode})"),
            rx,
            progress,
            started: Instant::now(),
        });
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let msg = match job.rx.try_recv() {
            Ok(m) => m,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.job = None;
                self.set_status(StatusKind::Error, "İşlem beklenmedik şekilde sonlandı.");
                return;
            }
        };
        self.job = None;
        match msg {
            JobMsg::Loaded(Ok(l)) => {
                let info = format!(
                    "Yüklendi: {} — {}, {} Hz",
                    file_name(&l.path),
                    format_time(l.audio.duration_secs(), true),
                    l.audio.sample_rate
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "WAV2MIDI — {}",
                    file_name(&l.path)
                )));
                self.loaded = Some(l);
                self.analysis = None;
                self.notes.clear();
                self.notes_version += 1;
                self.tracked_with = None;
                self.view = RollView::default();
                self.set_status(StatusKind::Ok, info);
                if self.auto_analyze {
                    self.start_analysis(ctx);
                }
            }
            JobMsg::Loaded(Err(e)) => self.set_status(StatusKind::Error, e),
            JobMsg::Analyzed {
                result: Err(e), ..
            } => self.set_status(StatusKind::Error, format!("Analiz başarısız: {e}")),
            JobMsg::Analyzed {
                settings,
                result: Ok(result),
                secs,
            } => {
                self.analysis = Some((settings, result));
                self.tracked_with = None;
                self.retrack();
                self.view.fit_pending = true;
                self.set_status(
                    StatusKind::Ok,
                    format!("Analiz tamamlandı: {} nota bulundu ({secs:.1} sn).", self.notes.len()),
                );
            }
        }
    }

    /// Nota takibi ayarları değiştiyse notaları yeniden hesapla (anlık).
    fn retrack(&mut self) {
        let Some((_, analysis)) = &self.analysis else { return };
        let key = (self.track.clone(), self.settings.min_note, self.settings.max_note);
        if self.tracked_with.as_ref() == Some(&key) {
            return;
        }
        self.notes = analysis.track_in_range(&self.track, key.1, key.2);
        self.notes_version += 1;
        self.tracked_with = Some(key);
    }

    fn set_status(&mut self, kind: StatusKind, text: impl Into<String>) {
        self.status = (kind, text.into());
    }

    // ------------------------------------------------------------------
    // Dosya işlemleri
    // ------------------------------------------------------------------

    fn pick_file(&mut self, ctx: &egui::Context) {
        let mut dlg = rfd::FileDialog::new()
            .set_title("Ses dosyası seçin")
            .add_filter("Ses dosyaları", &AUDIO_EXTS)
            .add_filter("Tüm dosyalar", &["*"]);
        if let Some(dir) = self.loaded.as_ref().and_then(|l| l.path.parent()) {
            dlg = dlg.set_directory(dir);
        }
        if let Some(p) = dlg.pick_file() {
            self.open_file(p, ctx);
        }
    }

    fn midi_options(&self, name: &str) -> MidiOptions {
        MidiOptions {
            fixed_velocity: self.use_fixed_velocity.then_some(self.fixed_velocity),
            track_name: name.to_string(),
            ..self.midi.clone()
        }
    }

    fn save_midi(&mut self) {
        let Some(l) = &self.loaded else { return };
        if self.notes.is_empty() {
            self.set_status(StatusKind::Error, "Kaydedilecek nota yok. Önce dönüştürün.");
            return;
        }
        let stem = l
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "cikti".into());
        let mut dlg = rfd::FileDialog::new()
            .set_title("MIDI dosyasını kaydet")
            .add_filter("MIDI dosyası", &["mid", "midi"])
            .set_file_name(format!("{stem}.mid"));
        if let Some(dir) = l.path.parent() {
            dlg = dlg.set_directory(dir);
        }
        let Some(mut out) = dlg.save_file() else { return };
        if out.extension().is_none() {
            out.set_extension("mid");
        }
        let opts = self.midi_options(&stem);
        match wav2midi::write_midi(&out, &self.notes, &opts) {
            Ok(()) => self.set_status(
                StatusKind::Ok,
                format!("Kaydedildi: {} ({} nota)", out.display(), self.notes.len()),
            ),
            Err(e) => self.set_status(StatusKind::Error, format!("Kaydedilemedi: {e}")),
        }
    }

    // ------------------------------------------------------------------
    // Çalma
    // ------------------------------------------------------------------

    fn synth_buffer(&mut self, sr: u32, duration: f64) -> Arc<Vec<f32>> {
        if let Some((v, buf)) = &self.synth_cache {
            if *v == self.notes_version {
                return buf.clone();
            }
        }
        let buf = Arc::new(wav2midi::synth::render_notes(&self.notes, sr, duration));
        self.synth_cache = Some((self.notes_version, buf.clone()));
        buf
    }

    fn play(&mut self, src: PlaySource) {
        let Some(l) = &self.loaded else { return };
        let sr = l.audio.sample_rate;
        let dur = l.audio.duration_secs();
        let audio = l.audio.clone();
        let mut from = self.view.cursor;
        if from >= dur - 0.05 {
            from = 0.0;
            self.view.cursor = 0.0;
        }
        let res = match src {
            PlaySource::Original => self.player.play(&audio.samples, sr, from),
            PlaySource::Midi => {
                let s = self.synth_buffer(sr, dur);
                self.player.play(&s, sr, from)
            }
            PlaySource::Both => {
                let s = self.synth_buffer(sr, dur);
                let len = audio.samples.len().max(s.len());
                let mixed: Vec<f32> = (0..len)
                    .map(|i| {
                        0.6 * audio.samples.get(i).copied().unwrap_or(0.0)
                            + 0.8 * s.get(i).copied().unwrap_or(0.0)
                    })
                    .collect();
                self.player.play(&mixed, sr, from)
            }
        };
        match res {
            Ok(()) => {
                self.playing = Some(src);
                self.last_source = src;
            }
            Err(e) => self.set_status(StatusKind::Error, e),
        }
    }

    fn stop_playback(&mut self) {
        self.player.stop();
        self.playing = None;
    }

    // ------------------------------------------------------------------
    // Arayüz
    // ------------------------------------------------------------------

    fn handle_input(&mut self, ctx: &egui::Context) {
        // Sürükle-bırak
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if let Some(p) = dropped.into_iter().next() {
            self.open_file(p, ctx);
        }

        if ctx.wants_keyboard_input() {
            return;
        }
        let (space, open, save, enter) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if space && self.loaded.is_some() {
            if self.playing.is_some() {
                self.stop_playback();
            } else {
                let src = if self.notes.is_empty() { PlaySource::Original } else { self.last_source };
                self.play(src);
            }
        }
        if open {
            self.pick_file(ctx);
        }
        if save {
            self.save_midi();
        }
        if enter && self.loaded.is_some() && self.job.is_none() {
            self.start_analysis(ctx);
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("🎼 WAV2MIDI").size(20.0).strong().color(Color32::WHITE));
                ui.label(RichText::new("ses dosyalarından MIDI üretir").weak());
                ui.add_space(16.0);
                if ui
                    .add(egui::Button::new(RichText::new("📂  Dosya Aç").size(15.0)).min_size(vec2(0.0, 30.0)))
                    .on_hover_text("WAV, MP3, FLAC, OGG, M4A (Ctrl+O)")
                    .clicked()
                {
                    self.pick_file(ctx);
                }
                let can_save = !self.notes.is_empty();
                if ui
                    .add_enabled(
                        can_save,
                        egui::Button::new(RichText::new("💾  MIDI Kaydet").size(15.0)).min_size(vec2(0.0, 30.0)),
                    )
                    .on_hover_text("Ctrl+S")
                    .clicked()
                {
                    self.save_midi();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("Boşluk: çal/durdur · Ctrl+tekerlek: yakınlaştır").weak().small());
                });
            });
            ui.add_space(6.0);
        });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                if let Some(job) = &self.job {
                    ui.spinner();
                    ui.label(&job.label);
                    let p = f32::from_bits(job.progress.load(Ordering::Relaxed)).clamp(0.0, 1.0);
                    ui.add(egui::ProgressBar::new(p).desired_width(260.0).show_percentage());
                    ui.label(RichText::new(format!("{:.1} sn", job.started.elapsed().as_secs_f32())).weak());
                } else {
                    let (kind, text) = &self.status;
                    let col = match kind {
                        StatusKind::Info => ui.visuals().text_color(),
                        StatusKind::Ok => OK_COL,
                        StatusKind::Error => ERR_COL,
                    };
                    let icon = match kind {
                        StatusKind::Info => "ℹ",
                        StatusKind::Ok => "✔",
                        StatusKind::Error => "⚠",
                    };
                    ui.label(RichText::new(format!("{icon}  {text}")).color(col));
                }
            });
            ui.add_space(3.0);
        });
    }

    fn side_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("settings")
            .resizable(false)
            .exact_width(330.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(8.0);
                    self.analysis_section(ui, ctx);
                    ui.add_space(10.0);
                    ui.separator();
                    self.tracking_section(ui);
                    ui.add_space(10.0);
                    ui.separator();
                    self.midi_section(ui);
                    ui.add_space(10.0);
                    ui.separator();
                    tips(ui);
                    ui.add_space(8.0);
                });
            });
    }

    fn analysis_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        section_title(ui, "1", "Analiz");
        ui.radio_value(&mut self.settings.mode, Mode::Ai, RichText::new("Yapay zekâ — her tür kayıt (önerilen)").strong())
            .on_hover_text("Spotify basic-pitch sinir ağı. Çok sesli, enstrümandan bağımsız; şarkılarda en iyi sonuç.");
        ui.radio_value(&mut self.settings.mode, Mode::Mono, "Klasik monofonik — vokal, solo melodi");
        ui.radio_value(&mut self.settings.mode, Mode::Poly, "Klasik polifonik — temiz piyano/gitar");
        ui.add_space(4.0);

        egui::Grid::new("analysis_grid")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("En düşük nota");
                note_drag(ui, &mut self.settings.min_note, 21, self.settings.max_note.saturating_sub(1));
                ui.end_row();
                ui.label("En yüksek nota");
                note_drag(ui, &mut self.settings.max_note, self.settings.min_note + 1, 108);
                ui.end_row();

                if self.settings.mode == Mode::Poly {
                    ui.label("Pencere");
                    egui::ComboBox::from_id_salt("window")
                        .selected_text(self.settings.window.label())
                        .width(190.0)
                        .show_ui(ui, |ui| {
                            for w in PolyWindow::ALL {
                                ui.selectable_value(&mut self.settings.window, w, w.label());
                            }
                        });
                    ui.end_row();
                    ui.label("Maks. polifoni");
                    ui.add(egui::Slider::new(&mut self.settings.max_polyphony, 1..=10).suffix(" ses"))
                        .on_hover_text("Aynı anda en fazla kaç nota algılanabilir");
                    ui.end_row();
                }
            });

        ui.add_space(8.0);
        let stale = self
            .analysis
            .as_ref()
            .is_some_and(|(s, _)| self.settings.requires_reanalysis(s));
        let label = if self.analysis.is_none() {
            "🎵  Dönüştür"
        } else if stale {
            "🔄  Ayarlar değişti — yeniden analiz et"
        } else {
            "🔄  Yeniden analiz et"
        };
        let enabled = self.loaded.is_some() && self.job.is_none();
        let fill = if stale || self.analysis.is_none() { ACCENT } else { ui.visuals().widgets.inactive.bg_fill };
        let btn = egui::Button::new(RichText::new(label).size(15.0).strong().color(Color32::WHITE))
            .fill(fill)
            .min_size(vec2(ui.available_width(), 36.0));
        if ui.add_enabled(enabled, btn).on_hover_text("Enter").clicked() {
            self.start_analysis(ctx);
        }
        ui.checkbox(&mut self.auto_analyze, "Dosya açılınca otomatik dönüştür");
    }

    fn tracking_section(&mut self, ui: &mut egui::Ui) {
        section_title(ui, "2", "Nota algılama");
        ui.label(RichText::new("Bu ayarlar sonucu anında günceller.").weak().small());
        ui.add_space(2.0);
        egui::Grid::new("track_grid")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("Hassasiyet");
                ui.add(
                    egui::Slider::new(&mut self.track.sensitivity, 0.0..=1.0)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                        .custom_parser(|s| s.trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0)),
                )
                .on_hover_text("Yüksek: daha fazla ve daha sessiz nota. Düşük: yalnızca belirgin notalar.");
                ui.end_row();
                ui.label("Min. nota süresi");
                ui.add(egui::Slider::new(&mut self.track.min_note_ms, 10.0..=500.0).suffix(" ms").integer());
                ui.end_row();
                if self.settings.mode != Mode::Ai {
                    ui.label("Boşluk birleştir");
                    ui.add(egui::Slider::new(&mut self.track.merge_gap_ms, 0.0..=250.0).suffix(" ms").integer())
                        .on_hover_text("Aynı perdedeki bu süreden kısa kesintiler tek nota sayılır");
                    ui.end_row();
                }
                ui.label("Eşzamanlı nota");
                let mut limit = if self.track.max_simultaneous == 0 { 13 } else { self.track.max_simultaneous };
                if ui
                    .add(
                        egui::Slider::new(&mut limit, 1..=13)
                            .custom_formatter(|v, _| if v >= 13.0 { "Sınırsız".into() } else { format!("en fazla {v:.0}") }),
                    )
                    .on_hover_text("Sadeleştirme: aynı anda çalan nota sayısını sınırlar, en zayıf notaları atar. Şarkılarda 3–4 deneyin.")
                    .changed()
                {
                    self.track.max_simultaneous = if limit >= 13 { 0 } else { limit };
                }
                ui.end_row();
            });
        ui.checkbox(&mut self.track.split_onsets, "Tekrar çalınan notaları ayır (atak algılama)");
        if ui.small_button("Varsayılanlara dön").clicked() {
            self.track = TrackSettings::default();
        }
    }

    fn midi_section(&mut self, ui: &mut egui::Ui) {
        section_title(ui, "3", "MIDI çıktısı");
        egui::Grid::new("midi_grid")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("Tempo");
                ui.add(egui::DragValue::new(&mut self.midi.bpm).range(30.0..=300.0).speed(0.5).suffix(" BPM"));
                ui.end_row();

                ui.label("Kuantalama");
                let qlabel = |q: u32| if q == 0 { "Yok".to_string() } else { format!("1/{q} nota") };
                egui::ComboBox::from_id_salt("quant")
                    .selected_text(qlabel(self.midi.quantize))
                    .show_ui(ui, |ui| {
                        for q in [0u32, 4, 8, 16, 32] {
                            ui.selectable_value(&mut self.midi.quantize, q, qlabel(q));
                        }
                    });
                ui.end_row();

                ui.label("Enstrüman");
                egui::ComboBox::from_id_salt("program")
                    .selected_text(format!(
                        "{} · {}",
                        self.midi.program + 1,
                        gm::INSTRUMENTS[self.midi.program as usize]
                    ))
                    .width(190.0)
                    .height(420.0)
                    .show_ui(ui, |ui| {
                        for (g, group) in gm::GROUPS.iter().enumerate() {
                            ui.label(RichText::new(*group).strong().color(ACCENT));
                            for i in 0..8u8 {
                                let p = g as u8 * 8 + i;
                                ui.selectable_value(
                                    &mut self.midi.program,
                                    p,
                                    format!("{:>3} · {}", p + 1, gm::INSTRUMENTS[p as usize]),
                                );
                            }
                        }
                    });
                ui.end_row();

                ui.checkbox(&mut self.use_fixed_velocity, "Sabit velocity");
                ui.add_enabled(
                    self.use_fixed_velocity,
                    egui::Slider::new(&mut self.fixed_velocity, 1..=127),
                );
                ui.end_row();
            });
        ui.add_space(8.0);
        let btn = egui::Button::new(RichText::new("💾  MIDI dosyasını kaydet").size(15.0).strong().color(Color32::WHITE))
            .fill(ACCENT)
            .min_size(vec2(ui.available_width(), 36.0));
        if ui.add_enabled(!self.notes.is_empty(), btn).clicked() {
            self.save_midi();
        }
    }

    fn central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(l) = &self.loaded else {
                self.empty_state(ui, ctx);
                return;
            };

            // Dosya bilgisi
            let dur = l.audio.duration_secs();
            let title = file_name(&l.path);
            let info = format!(
                "{} · {} Hz · {} kanal · {}",
                format_time(dur, true),
                l.audio.sample_rate,
                l.audio.channels,
                l.audio.codec
            );
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(16.0).strong());
                ui.label(RichText::new(info).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.notes.is_empty() {
                        let lo = self.notes.iter().map(|n| n.pitch).min().unwrap();
                        let hi = self.notes.iter().map(|n| n.pitch).max().unwrap();
                        ui.label(
                            RichText::new(format!(
                                "{} nota · {} – {}",
                                self.notes.len(),
                                note_name(lo),
                                note_name(hi)
                            ))
                            .color(OK_COL),
                        );
                    }
                });
            });
            ui.add_space(4.0);

            // Çalma araç çubuğu
            let has_notes = !self.notes.is_empty();
            let mut action: Option<Option<PlaySource>> = None;
            ui.horizontal(|ui| {
                let play_btn = |ui: &mut egui::Ui, text: &str, active: bool, enabled: bool| {
                    let mut b = egui::Button::new(RichText::new(text).size(14.0)).min_size(vec2(0.0, 28.0));
                    if active {
                        b = b.fill(ACCENT);
                    }
                    ui.add_enabled(enabled, b)
                };
                if play_btn(ui, "▶ Orijinal", self.playing == Some(PlaySource::Original), true).clicked() {
                    action = Some(Some(PlaySource::Original));
                }
                if play_btn(ui, "▶ MIDI", self.playing == Some(PlaySource::Midi), has_notes)
                    .on_hover_text("Algılanan notaları basit bir sentezleyiciyle çalar")
                    .clicked()
                {
                    action = Some(Some(PlaySource::Midi));
                }
                if play_btn(ui, "▶ Birlikte", self.playing == Some(PlaySource::Both), has_notes)
                    .on_hover_text("Orijinal ses ile MIDI'yi üst üste çalar")
                    .clicked()
                {
                    action = Some(Some(PlaySource::Both));
                }
                if play_btn(ui, "⏹ Durdur", false, self.playing.is_some()).clicked() {
                    action = Some(None);
                }
                let pos = self.player.position().unwrap_or(self.view.cursor);
                ui.label(RichText::new(format!("{} / {}", format_time(pos, true), format_time(dur, true))).monospace());

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⛶ Sığdır").on_hover_text("Tüm dosyayı göster").clicked() {
                        self.view.fit_pending = true;
                    }
                    ui.add(egui::Slider::new(&mut self.view.row_h, 5.0..=26.0).show_value(false))
                        .on_hover_text("Satır yüksekliği");
                    ui.label("↕");
                    let mut px = self.view.px_per_sec;
                    if ui
                        .add(egui::Slider::new(&mut px, 5.0..=1500.0).logarithmic(true).show_value(false))
                        .on_hover_text("Yatay yakınlaştırma (Ctrl + fare tekerleği)")
                        .changed()
                    {
                        let visible = (ui.available_width().max(300.0) / self.view.px_per_sec) as f64;
                        let center = self.view.t0 + visible / 2.0;
                        self.view.px_per_sec = px;
                        let nv = (ui.available_width().max(300.0) / px) as f64;
                        self.view.t0 = (center - nv / 2.0).max(0.0);
                    }
                    ui.label("↔");
                });
            });
            ui.add_space(4.0);

            let playhead = self.player.position();
            let resp = piano_roll::piano_roll(ui, &mut self.view, &self.notes, &l.overview, dur, playhead);

            match action {
                Some(Some(src)) => self.play(src),
                Some(None) => self.stop_playback(),
                None => {}
            }
            if resp.seek.is_some() {
                if let Some(src) = self.playing {
                    self.play(src);
                }
            }
        });
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let rect = ui.available_rect_before_wrap().shrink(24.0);
        let painter = ui.painter();
        painter.rect_stroke(
            rect,
            16.0,
            egui::Stroke::new(2.0_f32, Color32::from_rgb(70, 76, 96)),
            egui::StrokeKind::Inside,
        );
        let busy = self.job.is_some();
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((rect.height() / 2.0 - 110.0).max(10.0));
                ui.label(RichText::new("🎵").size(56.0));
                ui.add_space(6.0);
                ui.label(RichText::new("Ses dosyanızı buraya sürükleyip bırakın").size(22.0).strong());
                ui.label(RichText::new("WAV · MP3 · FLAC · OGG · M4A").weak());
                ui.add_space(14.0);
                let b = egui::Button::new(RichText::new("📂  Dosya Seç").size(16.0).strong().color(Color32::WHITE))
                    .fill(ACCENT)
                    .min_size(vec2(180.0, 40.0));
                if ui.add_enabled(!busy, b).clicked() {
                    self.pick_file(ctx);
                }
            });
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_job(ctx);
        self.retrack();
        self.handle_input(ctx);

        if self.playing.is_some() && !self.player.is_playing() {
            self.playing = None;
        }
        if self.playing.is_some() {
            ctx.request_repaint();
        } else if self.job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        self.top_bar(ctx);
        self.status_bar(ctx);
        self.side_panel(ctx);
        self.central(ctx);

        // Sürüklenirken kaplama
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            let r = ctx.screen_rect();
            painter.rect_filled(r, 0.0, Color32::from_black_alpha(190));
            painter.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                "Dosyayı bırakın…",
                egui::FontId::proportional(30.0),
                Color32::WHITE,
            );
        }
    }
}

// ----------------------------------------------------------------------
// Yardımcılar
// ----------------------------------------------------------------------

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.display().to_string())
}

fn section_title(ui: &mut egui::Ui, num: &str, title: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(" {num} "))
                .strong()
                .color(Color32::WHITE)
                .background_color(ACCENT),
        );
        ui.label(RichText::new(title).size(16.0).strong());
    });
    ui.add_space(4.0);
}

fn note_drag(ui: &mut egui::Ui, value: &mut u8, lo: u8, hi: u8) {
    let mut v = *value as i32;
    ui.add(
        egui::DragValue::new(&mut v)
            .range(lo as i32..=hi.max(lo) as i32)
            .speed(0.2)
            .custom_formatter(|n, _| format!("{} ({})", note_name(n as u8), n as i32))
            .custom_parser(|s| parse_note(s).map(|n| n as f64)),
    );
    *value = v.clamp(0, 127) as u8;
}

/// "C4", "f#3", "60" gibi girdileri MIDI numarasına çevirir.
fn parse_note(s: &str) -> Option<u8> {
    let s = s.trim();
    if let Ok(n) = s.split_whitespace().next()?.parse::<i32>() {
        return (0..=127).contains(&n).then_some(n as u8);
    }
    let s = s.split_whitespace().next()?.to_uppercase();
    let mut chars = s.chars();
    let base = match chars.next()? {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = chars.collect();
    let (acc, oct) = if let Some(r) = rest.strip_prefix('#') {
        (1, r)
    } else if let Some(r) = rest.strip_prefix('B') {
        (-1, r)
    } else {
        (0, rest.as_str())
    };
    let oct: i32 = oct.parse().ok()?;
    let n = (oct + 1) * 12 + base + acc;
    (0..=127).contains(&n).then_some(n as u8)
}

fn tips(ui: &mut egui::Ui) {
    ui.label(RichText::new("💡 İpuçları").strong());
    for t in [
        "Şarkılarda Yapay zekâ modunu kullanın; sonuç kalabalıksa Eşzamanlı nota sınırını 2–4'e indirin.",
        "Distorsiyonlu gitar akorları (power chord) doğal olarak 2–3 nota olarak çıkar.",
        "Gereksiz kısa notalar çoksa hassasiyeti düşürün veya min. süreyi artırın.",
        "Rulo üzerinde tıklayarak çalma konumunu seçin, sürükleyerek gezinin.",
    ] {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("• {t}")).weak().small());
        });
    }
}

fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(26, 28, 35);
    v.window_fill = Color32::from_rgb(30, 32, 40);
    v.extreme_bg_color = Color32::from_rgb(17, 18, 23);
    v.faint_bg_color = Color32::from_rgb(32, 34, 42);
    v.selection.bg_fill = ACCENT;
    v.hyperlink_color = ACCENT;
    v.widgets.inactive.corner_radius = 6.0.into();
    v.widgets.hovered.corner_radius = 6.0.into();
    v.widgets.active.corner_radius = 6.0.into();
    v.widgets.noninteractive.corner_radius = 6.0.into();
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.spacing.item_spacing = vec2(8.0, 7.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
        s.spacing.slider_width = 130.0;
        s.spacing.combo_width = 190.0;
        use egui::{FontId, TextStyle};
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.5));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.5));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
        s.text_styles.insert(TextStyle::Heading, FontId::proportional(20.0));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(13.5));
    });
}

/// Uygulama simgesi (64x64, programatik).
pub fn make_icon() -> egui::IconData {
    let s = 64usize;
    let mut rgba = vec![0u8; s * s * 4];
    let bars = [(10.0, 30.0, 16.0), (22.0, 46.0, 29.0), (34.0, 54.0, 42.0)];
    for y in 0..s {
        for x in 0..s {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let r = 13.0;
            let cx = fx.clamp(r, s as f32 - r);
            let cy = fy.clamp(r, s as f32 - r);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let alpha = (r + 0.5 - d).clamp(0.0, 1.0);
            let t = (fx + fy) / (2.0 * s as f32);
            let mut c = [
                98.0 + (236.0 - 98.0) * t,
                110.0 + (72.0 - 110.0) * t,
                245.0 + (153.0 - 245.0) * t,
            ];
            for (x0, x1, yc) in bars {
                if fx >= x0 && fx <= x1 && (fy - yc).abs() <= 3.5 {
                    c = [255.0, 255.0, 255.0];
                }
            }
            let i = (y * s + x) * 4;
            rgba[i] = c[0] as u8;
            rgba[i + 1] = c[1] as u8;
            rgba[i + 2] = c[2] as u8;
            rgba[i + 3] = (alpha * 255.0) as u8;
        }
    }
    egui::IconData {
        rgba,
        width: s as u32,
        height: s as u32,
    }
}
