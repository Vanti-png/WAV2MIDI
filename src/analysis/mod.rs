//! Perde analizi ve nota takibi.
//!
//! Analiz iki aşamalıdır:
//! 1. **Ağır aşama** ([`analyze`]): sesin kare kare analizi. Sonuç önbelleğe alınır.
//! 2. **Hafif aşama** ([`Analysis::track`]): kare sonuçlarından nota çıkarımı.
//!    Hassasiyet, minimum süre gibi ayarlar yalnızca bu aşamayı etkiler, bu yüzden
//!    arayüzde kaydırıcılar anlık olarak sonucu günceller.
//!
//! Üç mod vardır:
//! * [`Mode::Ai`]   – Spotify basic-pitch sinir ağı (önerilen, her tür kayıt)
//! * [`Mode::Poly`] – klasik sinyal işleme, çok sesli (harmonik toplama)
//! * [`Mode::Mono`] – klasik sinyal işleme, tek sesli (YIN)

pub mod ai;
pub mod mono;
pub mod poly;
mod util;

use anyhow::Result;

use crate::audio::AudioData;
use crate::notes::Note;

pub use ai::AiAnalysis;
pub use mono::MonoAnalysis;
pub use poly::PolyAnalysis;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Yapay zekâ (basic-pitch): çok sesli, enstrümandan bağımsız
    Ai,
    /// Tek ses: vokal, flüt, keman, ıslık, solo melodi (YIN)
    Mono,
    /// Çok sesli: piyano, gitar, akorlar (harmonik toplama)
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
            mode: Mode::Ai,
            min_note: 36, // C2
            max_note: 96, // C7
            window: PolyWindow::Balanced,
            max_polyphony: 6,
        }
    }
}

impl AnalysisSettings {
    fn range(&self) -> (u8, u8) {
        if self.min_note <= self.max_note {
            (self.min_note, self.max_note)
        } else {
            (self.max_note, self.min_note)
        }
    }

    /// `analyzed` ayarlarıyla yapılmış bir analizi bu ayarlara göre kullanmak için
    /// yeniden analiz gerekir mi? (Yapay zekâ modunda nota aralığı anlık uygulanır.)
    pub fn requires_reanalysis(&self, analyzed: &AnalysisSettings) -> bool {
        if self.mode != analyzed.mode {
            return true;
        }
        match self.mode {
            Mode::Ai => false,
            Mode::Mono => self.range() != analyzed.range(),
            Mode::Poly => {
                self.range() != analyzed.range()
                    || self.window != analyzed.window
                    || self.max_polyphony != analyzed.max_polyphony
            }
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
    /// Aynı perdedeki bu süreden kısa boşluklar birleştirilir (klasik modlar)
    pub merge_gap_ms: f32,
    /// Aynı perdenin tekrar çalınışlarını atak (onset) algılamayla ayır
    pub split_onsets: bool,
    /// Aynı anda en fazla bu kadar nota tutulur (0 = sınırsız); en zayıflar atılır
    pub max_simultaneous: usize,
}

impl Default for TrackSettings {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            min_note_ms: 120.0,
            merge_gap_ms: 40.0,
            split_onsets: true,
            max_simultaneous: 0,
        }
    }
}

pub enum Analysis {
    Ai(AiAnalysis),
    Mono(MonoAnalysis),
    Poly(PolyAnalysis),
}

/// Ağır analiz aşaması. `progress` 0..1 arası ilerleme bildirir.
pub fn analyze(
    audio: &AudioData,
    settings: &AnalysisSettings,
    progress: &(dyn Fn(f32) + Sync),
) -> Result<Analysis> {
    let (lo, hi) = settings.range();
    Ok(match settings.mode {
        Mode::Ai => Analysis::Ai(ai::analyze_ai(&audio.samples, audio.sample_rate, lo, hi, progress)?),
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
    })
}

impl Analysis {
    /// Analizdeki nota aralığıyla nota listesini üretir.
    pub fn track(&self, t: &TrackSettings) -> Vec<Note> {
        let (lo, hi) = match self {
            Analysis::Ai(a) => (a.min_note, a.max_note),
            Analysis::Mono(a) => (a.min_note, a.max_note),
            Analysis::Poly(a) => (a.min_note, a.max_note),
        };
        self.track_in_range(t, lo, hi)
    }

    /// Kare sonuçlarından nota listesini üretir (başlangıç zamanına göre sıralı).
    pub fn track_in_range(&self, t: &TrackSettings, min_note: u8, max_note: u8) -> Vec<Note> {
        let (lo, hi) = (min_note.min(max_note), max_note.max(min_note));
        let mut notes = match self {
            Analysis::Ai(a) => ai::track_ai(a, t, lo, hi),
            Analysis::Mono(a) => mono::track_mono(a, t),
            Analysis::Poly(a) => poly::track_poly(a, t),
        };
        notes.retain(|n| n.pitch >= lo && n.pitch <= hi);
        notes.sort_by(|a, b| {
            a.start
                .partial_cmp(&b.start)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.pitch.cmp(&b.pitch))
        });
        if t.max_simultaneous > 0 {
            notes = limit_simultaneous(notes, t.max_simultaneous);
        }
        notes
    }
}

/// Aynı anda en fazla `max` nota çalacak şekilde en zayıf notaları atar.
/// Güçlü (yüksek velocity) ve uzun notalar önceliklidir.
pub fn limit_simultaneous(notes: Vec<Note>, max: usize) -> Vec<Note> {
    let mut order: Vec<usize> = (0..notes.len()).collect();
    let score = |n: &Note| n.velocity as f64 * n.duration().max(0.01).sqrt();
    order.sort_by(|&a, &b| score(&notes[b]).partial_cmp(&score(&notes[a])).unwrap_or(std::cmp::Ordering::Equal));

    // Kabul edilenleri başlangıca göre sıralı tut; çakışma sayımı için tara
    let mut accepted: Vec<Note> = Vec::new();
    for i in order {
        let n = notes[i];
        // Bu notanın aralığında aynı anda çalan kabul edilmiş notaların en yüksek sayısı
        let mut edges: Vec<(f64, i32)> = accepted
            .iter()
            .filter(|m| m.start < n.end && m.end > n.start)
            .flat_map(|m| [(m.start.max(n.start), 1), (m.end.min(n.end), -1)])
            .collect();
        if edges.len() / 2 >= max {
            edges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
            let mut cur = 0;
            let mut peak = 0;
            for (_, d) in edges {
                cur += d;
                peak = peak.max(cur);
            }
            if peak as usize >= max {
                continue;
            }
        }
        let pos = accepted.partition_point(|m| m.start <= n.start);
        accepted.insert(pos, n);
    }
    accepted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(p: u8, s: f64, e: f64, v: u8) -> Note {
        Note { pitch: p, start: s, end: e, velocity: v }
    }

    #[test]
    fn limit_keeps_strongest() {
        let notes = vec![
            note(60, 0.0, 1.0, 100),
            note(64, 0.0, 1.0, 90),
            note(67, 0.0, 1.0, 30),
            note(72, 1.0, 2.0, 20), // çakışmıyor, kalmalı
        ];
        let out = limit_simultaneous(notes, 2);
        let pitches: Vec<u8> = out.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![60, 64, 72]);
    }
}
