//! Monofonik (tek sesli) perde takibi: YIN algoritması (de Cheveigné & Kawahara, 2002).
//! Fark fonksiyonu FFT tabanlı otokorelasyonla hızlı hesaplanır, kareler paralel işlenir.

use rayon::prelude::*;
use rustfft::{num_complex::Complex, FftPlanner};

use super::util::{median, peak_pick, to_relative_db, ProgressCounter};
use super::TrackSettings;
use crate::notes::{hz_to_midi, midi_to_hz, Note};

/// Kare kare YIN sonuçları.
pub struct MonoAnalysis {
    pub hop_secs: f64,
    /// Kesirli MIDI perdesi; perde yoksa NaN
    pub pitch: Vec<f32>,
    /// 0..1 periyodiklik güveni (1 - CMND)
    pub confidence: Vec<f32>,
    /// En gürültülü kareye göre dB (0 = en gürültülü)
    pub rms_db: Vec<f32>,
    pub min_note: u8,
    pub max_note: u8,
}

struct Scratch {
    a: Vec<Complex<f32>>,
    b: Vec<Complex<f32>>,
    fft: Vec<Complex<f32>>,
    prefix: Vec<f64>,
    cmnd: Vec<f32>,
}

const YIN_THRESHOLD: f32 = 0.15;

pub fn analyze_mono(
    samples: &[f32],
    sr: u32,
    min_note: u8,
    max_note: u8,
    progress: &(dyn Fn(f32) + Sync),
) -> MonoAnalysis {
    let srf = sr as f32;
    let fmin = midi_to_hz(min_note as f32 - 0.5);
    let fmax = midi_to_hz(max_note as f32 + 0.5);
    let tau_min = ((srf / fmax).floor() as usize).max(2);
    let tau_max = ((srf / fmin).ceil() as usize).max(tau_min + 2);
    // Pencere: en uzun periyodu kapsayan 2'nin kuvveti
    let w = (tau_max + 2).next_power_of_two().max(256);
    let fft_len = 4 * w;
    let hop = ((srf * 0.005).round() as usize).max(1); // 5 ms

    // Kare t, orijinal sinyalde t*hop merkezli olacak şekilde dolgu
    let pad = w / 2;
    let mut x = vec![0.0f32; pad];
    x.extend_from_slice(samples);
    x.extend(std::iter::repeat(0.0).take(2 * w + hop));

    let n_frames = samples.len() / hop + 1;

    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(fft_len);
    let inv = planner.plan_fft_inverse(fft_len);
    let scratch_len = fwd
        .get_inplace_scratch_len()
        .max(inv.get_inplace_scratch_len());

    let counter = ProgressCounter::new(n_frames, 0.0, 1.0, progress);

    let results: Vec<(f32, f32, f32)> = (0..n_frames)
        .into_par_iter()
        .map_init(
            || Scratch {
                a: vec![Complex::default(); fft_len],
                b: vec![Complex::default(); fft_len],
                fft: vec![Complex::default(); scratch_len],
                prefix: vec![0.0; 2 * w + 1],
                cmnd: vec![0.0; tau_max + 1],
            },
            |s, t| {
                let start = t * hop;
                let frame = &x[start..start + 2 * w];
                let r = yin_frame(frame, w, tau_min, tau_max, srf, s, &*fwd, &*inv);
                counter.tick();
                r
            },
        )
        .collect();

    let mut pitch = Vec::with_capacity(n_frames);
    let mut confidence = Vec::with_capacity(n_frames);
    let mut rms = Vec::with_capacity(n_frames);
    for (f0, c, r) in results {
        pitch.push(if f0.is_finite() && f0 > 0.0 {
            hz_to_midi(f0)
        } else {
            f32::NAN
        });
        confidence.push(c);
        rms.push(r);
    }

    MonoAnalysis {
        hop_secs: hop as f64 / sr as f64,
        pitch,
        confidence,
        rms_db: to_relative_db(&rms),
        min_note,
        max_note,
    }
}

/// Tek kare YIN. Dönen değer: (f0 Hz veya NaN, güven, RMS)
#[allow(clippy::too_many_arguments)]
fn yin_frame(
    x: &[f32],
    w: usize,
    tau_min: usize,
    tau_max: usize,
    sr: f32,
    s: &mut Scratch,
    fwd: &dyn rustfft::Fft<f32>,
    inv: &dyn rustfft::Fft<f32>,
) -> (f32, f32, f32) {
    let a = &x[..w];
    let e1: f64 = a.iter().map(|&v| (v as f64) * (v as f64)).sum();
    let rms = (e1 / w as f64).sqrt() as f32;
    if e1 < 1e-9 {
        return (f32::NAN, 0.0, rms);
    }

    // Çapraz korelasyon c(τ) = Σ a[j]·x[j+τ]  (FFT ile)
    for (i, c) in s.a.iter_mut().enumerate() {
        *c = Complex::new(if i < w { a[i] } else { 0.0 }, 0.0);
    }
    for (i, c) in s.b.iter_mut().enumerate() {
        *c = Complex::new(if i < 2 * w { x[i] } else { 0.0 }, 0.0);
    }
    fwd.process_with_scratch(&mut s.a, &mut s.fft);
    fwd.process_with_scratch(&mut s.b, &mut s.fft);
    for (ai, bi) in s.a.iter_mut().zip(s.b.iter()) {
        *ai = ai.conj() * bi;
    }
    inv.process_with_scratch(&mut s.a, &mut s.fft);
    let norm = 1.0 / s.a.len() as f64;

    // Kayan enerji için önek toplamı
    s.prefix[0] = 0.0;
    for i in 0..2 * w {
        s.prefix[i + 1] = s.prefix[i] + (x[i] as f64) * (x[i] as f64);
    }

    // Kümülatif ortalama normalize fark fonksiyonu (CMND)
    s.cmnd[0] = 1.0;
    let mut running = 0.0f64;
    for tau in 1..=tau_max {
        let e2 = s.prefix[tau + w] - s.prefix[tau];
        let c = s.a[tau].re as f64 * norm;
        let d = (e1 + e2 - 2.0 * c).max(0.0);
        running += d;
        s.cmnd[tau] = if running > 1e-12 {
            (d * tau as f64 / running) as f32
        } else {
            1.0
        };
    }

    // Mutlak eşik altındaki ilk yerel minimum
    let mut chosen = None;
    let mut tau = tau_min;
    while tau <= tau_max {
        if s.cmnd[tau] < YIN_THRESHOLD {
            while tau < tau_max && s.cmnd[tau + 1] < s.cmnd[tau] {
                tau += 1;
            }
            chosen = Some(tau);
            break;
        }
        tau += 1;
    }
    let t = chosen.unwrap_or_else(|| {
        let mut best = tau_min;
        for tau in tau_min..=tau_max {
            if s.cmnd[tau] < s.cmnd[best] {
                best = tau;
            }
        }
        best
    });

    // Parabolik ara değerleme
    let refined = if t > 1 && t < tau_max {
        let (y0, y1, y2) = (s.cmnd[t - 1], s.cmnd[t], s.cmnd[t + 1]);
        let denom = y0 - 2.0 * y1 + y2;
        if denom.abs() > 1e-9 {
            let delta = (0.5 * (y0 - y2) / denom).clamp(-1.0, 1.0);
            t as f32 + delta
        } else {
            t as f32
        }
    } else {
        t as f32
    };

    let conf = (1.0 - s.cmnd[t]).clamp(0.0, 1.0);
    (sr / refined, conf, rms)
}

// ---------------------------------------------------------------------------
// Nota takibi
// ---------------------------------------------------------------------------

struct Current {
    start: usize,
    last: usize,
    values: Vec<f32>,
    sum: f32,
    peak_db: f32,
    /// Ortalamadan sapan (olası yeni nota) kareler
    dev: Vec<(usize, f32, f32)>,
}

impl Current {
    fn new(t: usize, m: f32, db: f32) -> Self {
        Self {
            start: t,
            last: t,
            values: vec![m],
            sum: m,
            peak_db: db,
            dev: Vec::new(),
        }
    }
    fn mean(&self) -> f32 {
        self.sum / self.values.len() as f32
    }
    fn push(&mut self, t: usize, m: f32, db: f32) {
        self.values.push(m);
        self.sum += m;
        self.last = t;
        self.peak_db = self.peak_db.max(db);
    }
}

pub fn track_mono(a: &MonoAnalysis, p: &TrackSettings) -> Vec<Note> {
    let n = a.pitch.len();
    let sens = p.sensitivity.clamp(0.0, 1.0);
    let hop = a.hop_secs;
    let frames = |ms: f32| ((ms as f64 / 1000.0) / hop).round() as usize;

    let conf_thr = 0.92 - 0.5 * sens; // 0.92 .. 0.42
    let gate_db = -15.0 - 45.0 * sens; // -15 .. -60 dB
    let lo = a.min_note as f32 - 0.5;
    let hi = a.max_note as f32 + 0.5;

    let voiced: Vec<Option<f32>> = (0..n)
        .map(|t| {
            let m = a.pitch[t];
            if m.is_finite()
                && a.confidence[t] >= conf_thr
                && a.rms_db[t] >= gate_db
                && m >= lo
                && m <= hi
            {
                Some(m)
            } else {
                None
            }
        })
        .collect();

    // 5 karelik medyan filtresi (tek karelik oktav sıçramalarını temizler)
    let smooth: Vec<Option<f32>> = (0..n)
        .map(|t| {
            voiced[t].map(|m| {
                let mut v: Vec<f32> = (t.saturating_sub(2)..(t + 3).min(n))
                    .filter_map(|i| voiced[i])
                    .collect();
                if v.len() >= 3 {
                    median(&mut v)
                } else {
                    m
                }
            })
        })
        .collect();

    // Atak noktaları: kısa sürede belirgin enerji artışı
    let onset_set: Vec<bool> = if p.split_onsets {
        let k = frames(40.0).max(1);
        let rise: Vec<f32> = (0..n)
            .map(|t| {
                let lo = t.saturating_sub(k);
                let min = a.rms_db[lo..=t].iter().copied().fold(f32::MAX, f32::min);
                a.rms_db[t] - min
            })
            .collect();
        let mut v = vec![false; n];
        for t in peak_pick(&rise, 7.0, k / 2 + 1) {
            v[t] = true;
        }
        v
    } else {
        vec![false; n]
    };

    let gap_frames = frames(p.merge_gap_ms).max(1);
    let change_frames = frames(30.0).max(2);
    let min_frames = frames(p.min_note_ms).max(1);

    let mut notes = Vec::new();
    let finish = |c: Current, notes: &mut Vec<Note>| {
        let len = c.last + 1 - c.start;
        if len < min_frames {
            return;
        }
        let mut vals = c.values;
        let pitch = median(&mut vals).round().clamp(0.0, 127.0) as u8;
        let velocity = (115.0 + 1.8 * c.peak_db).clamp(15.0, 127.0) as u8;
        notes.push(Note {
            pitch,
            start: c.start as f64 * hop,
            end: (c.last + 1) as f64 * hop,
            velocity,
        });
    };

    let mut cur: Option<Current> = None;
    for t in 0..n {
        let Some(m) = smooth[t] else { continue };
        let db = a.rms_db[t];

        let Some(c) = cur.as_mut() else {
            cur = Some(Current::new(t, m, db));
            continue;
        };

        // Uzun sessizlik: notayı kapat
        let last_seen = c.dev.last().map_or(c.last, |d| d.0.max(c.last));
        let gap = t - last_seen - 1;
        if gap > gap_frames {
            let done = cur.take().unwrap();
            finish(done, &mut notes);
            cur = Some(Current::new(t, m, db));
            continue;
        }

        if (m - c.mean()).abs() > 0.6 {
            // Olası perde değişimi – yeterince uzun ve tutarlıysa yeni nota başlat
            c.dev.push((t, m, db));
            if c.dev.len() >= change_frames {
                let mut dv: Vec<f32> = c.dev.iter().map(|d| d.1).collect();
                let dmin = dv.iter().copied().fold(f32::MAX, f32::min);
                let dmax = dv.iter().copied().fold(f32::MIN, f32::max);
                if dmax - dmin < 1.2 {
                    let med = median(&mut dv);
                    let devs = std::mem::take(&mut c.dev);
                    let done = cur.take().unwrap();
                    finish(done, &mut notes);
                    let (t0, _, db0) = devs[0];
                    let mut nc = Current::new(t0, med, db0);
                    nc.values.clear();
                    nc.sum = 0.0;
                    for (tt, mm, dd) in devs {
                        nc.push(tt, mm, dd);
                    }
                    cur = Some(nc);
                } else {
                    c.dev.remove(0);
                }
            }
        } else {
            // Notaya geri dönüldü: sapmalar vibratoydu, notaya kat
            for (tt, mm, dd) in std::mem::take(&mut c.dev) {
                c.push(tt, mm, dd);
            }
            if onset_set[t] && t - c.start >= min_frames {
                let done = cur.take().unwrap();
                finish(done, &mut notes);
                cur = Some(Current::new(t, m, db));
                continue;
            }
            c.push(t, m, db);
        }
    }
    if let Some(c) = cur {
        finish(c, &mut notes);
    }
    notes
}
