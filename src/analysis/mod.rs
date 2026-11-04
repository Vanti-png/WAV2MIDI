//! Perde analizi ve nota takibi.
//!
//! Analiz iki aşamalıdır:
//! 1. **Ağır aşama** ([`analyze`]): sesin kare kare perde analizi. Sonuç önbelleğe alınır.
//! 2. **Hafif aşama** ([`Analysis::track`]): kare sonuçlarından nota çıkarımı.
//!    Hassasiyet, minimum süre gibi ayarlar yalnızca bu aşamayı etkiler, bu yüzden
//!    arayüzde kaydırıcılar anlık olarak sonucu günceller.

pub mod mono;
pub mod poly;
mod util;

use crate::audio::AudioData;
use crate::notes::Note;

pub use mono::MonoAnalysis;
pub use poly::PolyAnalysis;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Tek ses: vokal, flüt, keman, ıslık, solo melodi
    Mono,
    /// Çok sesli: piyano, gitar, akorlar
    Poly,
}

/// Polifonik analizde kullanılan pencere uzunluğu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolyWindow {
    Fast,
    Balanced,
    Precise,
}

impl PolyWindow {
    pub const ALL: [PolyWindow; 3] = [PolyWindow::Fast, PolyWindow::Balanced, PolyWindow::Precise];

    pub fn secs(self) -> f64 {
        match self {
            PolyWindow::Fast => 0.046,
            PolyWindow::Balanced => 0.093,
            PolyWindow::Precise => 0.186,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PolyWindow::Fast => "Hızlı notalar (46 ms)",
            PolyWindow::Balanced => "Dengeli (93 ms)",
            PolyWindow::Precise => "Hassas perde / bas (186 ms)",
        }
    }
}

/// Ağır analiz aşamasını etkileyen ayarlar.
#[derive(Clone, Debug, PartialEq)]
pub struct AnalysisSettings {
    pub mode: Mode,
    pub min_note: u8,
    pub max_note: u8,
    pub window: PolyWindow,
    pub max_polyphony: usize,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            mode: Mode::Poly,
            min_note: 36, // C2
            max_note: 96, // C7
            window: PolyWindow::Balanced,
            max_polyphony: 6,
        }
    }
}

/// Nota takibi (hafif aşama) ayarları.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackSettings {
    /// 0..1 – yüksek değer daha fazla (ve daha sessiz) nota demektir
    pub sensitivity: f32,
    /// Bu süreden kısa notalar atılır
    pub min_note_ms: f32,
    /// Aynı perdedeki bu süreden kısa boşluklar birleştirilir
    pub merge_gap_ms: f32,
    /// Aynı perdenin tekrar çalınışlarını atak (onset) algılamayla ayır
    pub split_onsets: bool,
}

impl Default for TrackSettings {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            min_note_ms: 60.0,
            merge_gap_ms: 40.0,
            split_onsets: true,
        }
    }
}

pub enum Analysis {
    Mono(MonoAnalysis),
    Poly(PolyAnalysis),
}

/// Ağır analiz aşaması. `progress` 0..1 arası ilerleme bildirir.
pub fn analyze(
    audio: &AudioData,
    settings: &AnalysisSettings,
    progress: &(dyn Fn(f32) + Sync),
) -> Analysis {
    let (lo, hi) = if settings.min_note <= settings.max_note {
        (settings.min_note, settings.max_note)
    } else {
        (settings.max_note, settings.min_note)
    };
    match settings.mode {
        Mode::Mono => Analysis::Mono(mono::analyze_mono(
            &audio.samples,
            audio.sample_rate,
            lo,
            hi,
            progress,
        )),
        Mode::Poly => Analysis::Poly(poly::analyze_poly(
            &audio.samples,
            audio.sample_rate,
            lo,
            hi,
            settings.window.secs(),
            settings.max_polyphony.clamp(1, 12),
            progress,
        )),
    }
}

impl Analysis {
    /// Kare sonuçlarından nota listesini üretir (başlangıç zamanına göre sıralı).
    pub fn track(&self, t: &TrackSettings) -> Vec<Note> {
        let mut notes = match self {
            Analysis::Mono(a) => mono::track_mono(a, t),
            Analysis::Poly(a) => poly::track_poly(a, t),
        };
        notes.sort_by(|a, b| {
            a.start
                .partial_cmp(&b.start)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.pitch.cmp(&b.pitch))
        });
        notes
    }
}
