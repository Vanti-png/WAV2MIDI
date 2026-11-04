//! Polifonik (çok sesli) perde tahmini.
//!
//! Klapuri (2006) "Multiple fundamental frequency estimation by summing harmonic
//! amplitudes" yaklaşımının sadeleştirilmiş hâli:
//! 1. Hann pencereli STFT (2x sıfır dolgulu)
//! 2. Spektral beyazlatma (ERB benzeri bantlarda sıkıştırma, ν = 0.33)
//! 3. Her aday perde için ağırlıklı harmonik genlik toplamı (salience)
//! 4. Yinelemeli tahmin + "spektral düzgünlük" ile harmonik çıkarma
//!
//! Nota takibi aşamasında zayıf adaylar, karedeki en güçlü adaya göre göreli eşikle
//! ve tüm dosyaya göre global eşikle elenir.

use rayon::prelude::*;
use rustfft::{num_complex::Complex, FftPlanner};

use super::util::{hann, nearest_pow2, peak_pick, percentile, to_relative_db, ProgressCounter};
use super::TrackSettings;
use crate::notes::{midi_to_hz, Note};

pub struct PolyAnalysis {
    pub hop_secs: f64,
    /// Analiz penceresinin kare (hop) cinsinden uzunluğu
    pub window_frames: usize,
    /// Her kare için tespit sırasıyla (perde, salience) listesi
    pub frames: Vec<Vec<(u8, f32)>>,
    /// Global referans salience (≈ en güçlü değerler)
    pub ref_salience: f32,
    pub rms_db: Vec<f32>,
    /// Normalize atak (onset) gücü; ~1 = güçlü atak
    pub onset: Vec<f32>,
    pub min_note: u8,
    pub max_note: u8,
}

struct Harmonic {
    lo: usize,
    hi: usize,
    weight: f32,
}

struct Candidate {
    pitch: u8,
    harmonics: Vec<Harmonic>,
}

struct Whitening {
    /// Her bant için (bin, üçgen ağırlık)
    bands: Vec<Vec<(usize, f32)>>,
    /// Her bin için (bant j, bant j+1, kesir) – γ ara değerlemesi
    interp: Vec<(usize, usize, f32)>,
}

struct Scratch {
    buf: Vec<Complex<f32>>,
    fft: Vec<Complex<f32>>,
    mag: Vec<f32>,
    res: Vec<f32>,
    gamma: Vec<f32>,
    amps: Vec<f32>,
    peaks: Vec<usize>,
    used: Vec<bool>,
}

const TOLERANCE_CENTS: f32 = 40.0;
const MAX_HARMONICS: usize = 20;
const NU: f32 = 0.33;
/// 2.–8. harmoniklerin temel frekansa uzaklığı (yarım ton)
const HARMONIC_INTERVALS: [u8; 7] = [12, 19, 24, 28, 31, 34, 36];

pub fn analyze_poly(
    samples: &[f32],
    sr: u32,
    min_note: u8,
    max_note: u8,
    window_secs: f64,
    max_poly: usize,
    progress: &(dyn Fn(f32) + Sync),
) -> PolyAnalysis {
    let srf = sr as f64;
    let n = nearest_pow2(srf * window_secs).max(512);
    let k_len = 2 * n;
    let nbins = k_len / 2 + 1;
    let bin_hz = sr as f32 / k_len as f32;
    let hop = ((srf * 0.01).round() as usize).max(1); // 10 ms
    let nyq = sr as f32 / 2.0;
    let f_lim = (nyq * 0.92).min(8000.0);

    let pad = n / 2;
    let mut x = vec![0.0f32; pad];
    x.extend_from_slice(samples);
    x.extend(std::iter::repeat(0.0).take(n + hop));
    let n_frames = samples.len() / hop + 1;

    let win = hann(n);
    let candidates = build_candidates(min_note, max_note, bin_hz, nbins, f_lim);
    // Salience için gereken en yüksek bin
    let kmax = candidates
        .iter()
        .flat_map(|c| c.harmonics.iter().map(|h| h.hi))
        .max()
        .unwrap_or(1)
        .min(nbins - 1)
        + 4;
    let kmax = kmax.min(nbins);
    let white = build_whitening(kmax, bin_hz, f_lim * 1.1, nyq);

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(k_len);
    let scratch_len = fft.get_inplace_scratch_len();
    let counter = ProgressCounter::new(n_frames, 0.0, 0.9, progress);

    let frames: Vec<Vec<(u8, f32)>> = (0..n_frames)
        .into_par_iter()
        .map_init(
            || Scratch {
                buf: vec![Complex::default(); k_len],
                fft: vec![Complex::default(); scratch_len],
                mag: vec![0.0; kmax],
                res: vec![0.0; kmax],
                gamma: vec![0.0; white.bands.len()],
                amps: Vec::with_capacity(MAX_HARMONICS),
                peaks: Vec::with_capacity(MAX_HARMONICS),
                used: vec![false; candidates.len()],
            },
            |s, t| {
                let start = t * hop;
                let frame = &x[start..start + n];
                let r = analyze_frame(frame, &win, &*fft, &white, &candidates, max_poly, s);
                counter.tick();
                r
            },
        )
        .collect();

    let (rms_db, onset) = energy_and_onsets(samples, sr, hop, n, n_frames);
    progress(1.0);

    let firsts: Vec<f32> = frames
        .iter()
        .filter_map(|f| f.first().map(|d| d.1))
        .collect();
    let ref_salience = percentile(&firsts, 0.995).max(1e-9);

    PolyAnalysis {
        hop_secs: hop as f64 / srf,
        window_frames: (n / hop).max(1),
        frames,
        ref_salience,
        rms_db,
        onset,
        min_note,
        max_note,
    }
}

fn build_candidates(min_note: u8, max_note: u8, bin_hz: f32, nbins: usize, f_lim: f32) -> Vec<Candidate> {
    let tol = 2f32.powf(TOLERANCE_CENTS / 1200.0);
    (min_note..=max_note)
        .map(|p| {
            let f0 = midi_to_hz(p as f32);
            let mut harmonics = Vec::new();
            for m in 1..=MAX_HARMONICS {
                let f = f0 * m as f32;
                if f > f_lim {
                    break;
                }
                let cb = f / bin_hz;
                let center = cb.round() as isize;
                let lo = ((cb / tol).floor() as isize).min(center - 1).max(1) as usize;
                let hi = ((cb * tol).ceil() as isize).max(center + 1) as usize;
                let hi = hi.min(nbins - 1);
                if lo > hi {
                    continue;
                }
                let weight = (f0 + 20.0) / (m as f32 * f0 + 320.0);
                harmonics.push(Harmonic { lo, hi, weight });
            }
            Candidate { pitch: p, harmonics }
        })
        .filter(|c| !c.harmonics.is_empty())
        .collect()
}

fn build_whitening(kmax: usize, bin_hz: f32, f_hi: f32, nyq: f32) -> Whitening {
    let mut cs = Vec::new();
    let mut b = 0;
    loop {
        let c = 229.0 * (10f32.powf((b as f32 + 1.0) / 21.4) - 1.0);
        if c > f_hi || c >= nyq {
            break;
        }
        cs.push(c);
        b += 1;
    }
    if cs.len() < 2 {
        cs = vec![100.0, 1000.0];
    }
    let nb = cs.len();
    let mut bands = vec![Vec::new(); nb];
    for (b, band) in bands.iter_mut().enumerate() {
        let lower = if b == 0 { 0.0 } else { cs[b - 1] };
        let upper = if b + 1 < nb { cs[b + 1] } else { nyq.min(cs[b] * 1.5) };
        let c = cs[b];
        for k in 0..kmax {
            let f = k as f32 * bin_hz;
            let w = if f > lower && f <= c {
                (f - lower) / (c - lower)
            } else if f > c && f < upper {
                (upper - f) / (upper - c)
            } else {
                0.0
            };
            if w > 0.0 {
                band.push((k, w));
            }
        }
    }
    let interp = (0..kmax)
        .map(|k| {
            let f = k as f32 * bin_hz;
            if f <= cs[0] {
                (0, 0, 0.0)
            } else if f >= cs[nb - 1] {
                (nb - 1, nb - 1, 0.0)
            } else {
                let j = cs.iter().rposition(|&c| c <= f).unwrap_or(0).min(nb - 2);
                (j, j + 1, (f - cs[j]) / (cs[j + 1] - cs[j]))
            }
        })
        .collect();
    Whitening { bands, interp }
}

#[inline]
fn salience(c: &Candidate, r: &[f32]) -> f32 {
    let mut s = 0.0;
    for h in &c.harmonics {
        let mut m = 0.0f32;
        for &v in &r[h.lo..=h.hi] {
            m = m.max(v);
        }
        s += h.weight * m;
    }
    s
}

fn analyze_frame(
    frame: &[f32],
    win: &[f32],
    fft: &dyn rustfft::Fft<f32>,
    white: &Whitening,
    candidates: &[Candidate],
    max_poly: usize,
    s: &mut Scratch,
) -> Vec<(u8, f32)> {
    let n = win.len();
    let mut energy = 0.0f32;
    for (i, c) in s.buf.iter_mut().enumerate() {
        let v = if i < n { frame[i] * win[i] } else { 0.0 };
        energy += v * v;
        *c = Complex::new(v, 0.0);
    }
    if energy < 1e-10 {
        return Vec::new();
    }
    fft.process_with_scratch(&mut s.buf, &mut s.fft);
    let kmax = s.mag.len();
    for k in 0..kmax {
        s.mag[k] = s.buf[k].norm();
    }

    // Spektral beyazlatma
    let klen = s.buf.len() as f32;
    for (b, band) in white.bands.iter().enumerate() {
        let mut acc = 0.0f32;
        for &(k, w) in band {
            acc += w * s.mag[k] * s.mag[k];
        }
        let sigma = (acc / klen).sqrt() + 1e-9;
        s.gamma[b] = sigma.powf(NU - 1.0);
    }
    for k in 0..kmax {
        let (j0, j1, fr) = white.interp[k];
        let g = s.gamma[j0] * (1.0 - fr) + s.gamma[j1] * fr;
        s.res[k] = s.mag[k] * g;
    }

    // Yinelemeli tahmin ve çıkarma
    s.used.iter_mut().for_each(|u| *u = false);
    let mut detected = Vec::with_capacity(max_poly);
    const LOBE: usize = 3;
    for _ in 0..max_poly {
        let mut best = None;
        let mut best_s = 0.0f32;
        for (ci, c) in candidates.iter().enumerate() {
            if s.used[ci] {
                continue;
            }
            let v = salience(c, &s.res);
            if v > best_s {
                best_s = v;
                best = Some(ci);
            }
        }
        let Some(ci) = best else { break };
        if best_s <= 1e-9 {
            break;
        }
        s.used[ci] = true;
        let c = &candidates[ci];
        detected.push((c.pitch, best_s));

        s.amps.clear();
        s.peaks.clear();
        for h in &c.harmonics {
            let mut kb = h.lo;
            let mut a = 0.0f32;
            for k in h.lo..=h.hi {
                if s.res[k] > a {
                    a = s.res[k];
                    kb = k;
                }
            }
            s.amps.push(a);
            s.peaks.push(kb);
        }
        let m = s.amps.len();
        for i in 0..m {
            let a = s.amps[i];
            if a <= 1e-12 {
                continue;
            }
            // Spektral düzgünlük: bu notanın bu harmoniğe katkısını komşu harmoniklerden
            // tahmin et. Başka bir notayla paylaşılan (fazla güçlü) harmonikler yalnızca
            // kısmen çıkarılır; böylece oktav/kuint üstündeki notalar kaybolmaz.
            let env = if i > 0 && i + 1 < m {
                let (x, y, z) = (s.amps[i - 1], a, s.amps[i + 1]);
                x.max(y.min(z)).min(y.max(z)) // üçün medyanı
            } else {
                let lo = i.saturating_sub(1);
                let hi = (i + 2).min(m);
                s.amps[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
            };
            let remove = a.min(env);
            let factor = (1.0 - remove / a).clamp(0.0, 1.0);
            let k0 = s.peaks[i].saturating_sub(LOBE);
            let k1 = (s.peaks[i] + LOBE).min(kmax - 1);
            for k in k0..=k1 {
                s.res[k] *= factor;
            }
        }
    }
    detected
}

/// Kare RMS'i (uzun pencere) ve spektral akı tabanlı atak gücü (kısa pencere).
fn energy_and_onsets(
    samples: &[f32],
    sr: u32,
    hop: usize,
    long_n: usize,
    n_frames: usize,
) -> (Vec<f32>, Vec<f32>) {
    let len = samples.len();
    let mut prefix = vec![0.0f64; len + 1];
    for i in 0..len {
        prefix[i + 1] = prefix[i] + (samples[i] as f64) * (samples[i] as f64);
    }
    let half = long_n / 2;
    let rms: Vec<f32> = (0..n_frames)
        .map(|t| {
            let c = t * hop;
            let a = c.saturating_sub(half).min(len);
            let b = (c + half).min(len);
            if b > a {
                ((prefix[b] - prefix[a]) / (b - a) as f64).sqrt() as f32
            } else {
                0.0
            }
        })
        .collect();

    // Spektral akı
    let ns = nearest_pow2(sr as f64 * 0.023).max(256);
    let win = hann(ns);
    let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let g = 1.0 / peak;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(ns);
    let mut scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
    let mut buf = vec![Complex::default(); ns];
    let kmax = ((11000.0 / (sr as f32 / ns as f32)) as usize).min(ns / 2);
    let mut prev = vec![0.0f32; kmax];
    let mut cur = vec![0.0f32; kmax];
    let mut flux = vec![0.0f32; n_frames];
    for (t, fl) in flux.iter_mut().enumerate() {
        let c = (t * hop) as isize;
        for (i, b) in buf.iter_mut().enumerate() {
            let idx = c - (ns / 2) as isize + i as isize;
            let v = if idx >= 0 && (idx as usize) < len {
                samples[idx as usize] * g * win[i]
            } else {
                0.0
            };
            *b = Complex::new(v, 0.0);
        }
        fft.process_with_scratch(&mut buf, &mut scratch);
        let mut f = 0.0;
        for k in 1..kmax {
            cur[k] = (1.0 + 100.0 * buf[k].norm()).ln();
            if t > 0 {
                f += (cur[k] - prev[k]).max(0.0);
            }
        }
        *fl = f;
        std::mem::swap(&mut prev, &mut cur);
    }

    // Yerel ortalamayı çıkar ve normalize et
    let r = 10usize;
    let mut onset = vec![0.0f32; n_frames];
    let mut fprefix = vec![0.0f64; n_frames + 1];
    for i in 0..n_frames {
        fprefix[i + 1] = fprefix[i] + flux[i] as f64;
    }
    for t in 0..n_frames {
        let a = t.saturating_sub(r);
        let b = (t + r + 1).min(n_frames);
        let mean = ((fprefix[b] - fprefix[a]) / (b - a) as f64) as f32;
        onset[t] = (flux[t] - mean).max(0.0);
    }
    let norm = percentile(&onset, 0.99).max(1e-6);
    for o in onset.iter_mut() {
        *o /= norm;
    }

    (to_relative_db(&rms), onset)
}

// ---------------------------------------------------------------------------
// Nota takibi
// ---------------------------------------------------------------------------

pub fn track_poly(a: &PolyAnalysis, p: &TrackSettings) -> Vec<Note> {
    let n = a.frames.len();
    if n == 0 {
        return Vec::new();
    }
    let sens = p.sensitivity.clamp(0.0, 1.0);
    let hop = a.hop_secs;
    let frames = |ms: f32| ((ms as f64 / 1000.0) / hop).round() as usize;

    // Kare içindeki ilk (en güçlü) adaya göre göreli eşik. Yinelemeli çıkarma nedeniyle
    // salience değerleri azalan sıradadır, bu yüzden eşiğin altına düşünce durulur.
    let rel = 0.45 - 0.3 * sens;
    let thr = a.ref_salience * (0.32 - 0.29 * sens);
    let gate_db = -20.0 - 40.0 * sens;
    let onset_thr = 0.4 - 0.3 * sens;

    let lo = a.min_note as usize;
    let hi = a.max_note as usize;
    let rows = hi - lo + 1;
    let mut act = vec![vec![0.0f32; n]; rows];

    for (t, dets) in a.frames.iter().enumerate() {
        if a.rms_db[t] < gate_db {
            continue;
        }
        let first = dets.first().map_or(0.0, |d| d.1);
        let mut accepted: Vec<u8> = Vec::with_capacity(dets.len());
        for &(pitch, sal) in dets.iter() {
            if sal < thr || sal < rel * first {
                break;
            }
            // Daha önce kabul edilmiş alçak bir notanın harmoniğine denk gelen aday
            // ("harmonik gölge") için daha güçlü kanıt iste.
            let shadow = accepted
                .iter()
                .any(|&q| pitch > q && HARMONIC_INTERVALS.contains(&(pitch - q)));
            if shadow && sal < (rel + 0.15) * first {
                continue;
            }
            accepted.push(pitch);
            let pi = pitch as usize;
            if pi >= lo && pi <= hi {
                act[pi - lo][t] = sal;
            }
        }
    }

    let onset_peaks = peak_pick(&a.onset, onset_thr, 3);
    let gap_frames = frames(p.merge_gap_ms);
    let min_frames = frames(p.min_note_ms).max(1);
    let wf = a.window_frames;
    let k_rise = (wf / 2).max(3);

    let mut notes = Vec::new();
    for (ri, row) in act.iter().enumerate() {
        let pitch = (lo + ri) as u8;
        // Boşluk birleştirmeli bölütler
        let mut segs: Vec<(usize, usize)> = Vec::new();
        let mut t = 0;
        while t < n {
            if row[t] <= 0.0 {
                t += 1;
                continue;
            }
            let start = t;
            let mut end = t;
            let mut gap = 0;
            t += 1;
            while t < n {
                if row[t] > 0.0 {
                    end = t;
                    gap = 0;
                } else {
                    gap += 1;
                    if gap > gap_frames {
                        break;
                    }
                }
                t += 1;
            }
            segs.push((start, end));
        }

        // Ataklarda böl (aynı notanın tekrar çalınması)
        let mut split: Vec<(usize, usize)> = Vec::new();
        for (s, e) in segs {
            let mut cs = s;
            if p.split_onsets {
                for &o in onset_peaks.iter().filter(|&&o| o > s && o < e) {
                    if o - cs < min_frames || e - o < min_frames {
                        continue;
                    }
                    let before = row[o.saturating_sub(k_rise)..o]
                        .iter()
                        .copied()
                        .fold(f32::MAX, f32::min);
                    let after = row[o..(o + k_rise).min(n)]
                        .iter()
                        .copied()
                        .fold(0.0f32, f32::max);
                    if after > 1.5 * before + 1e-9 {
                        split.push((cs, o - 1));
                        cs = o;
                    }
                }
            }
            split.push((cs, e));
        }

        for (s, e) in split {
            let mut start = s;
            // Uzun pencere notayı erken yakalar: başlangıcı yakındaki atağa hizala
            let lo_o = s.saturating_sub(wf / 4);
            let hi_o = s + wf / 2;
            if let Some(&o) = onset_peaks
                .iter()
                .filter(|&&o| o >= lo_o && o <= hi_o && o + 1 < e)
                .max_by(|&&x, &&y| a.onset[x].partial_cmp(&a.onset[y]).unwrap())
            {
                start = o;
            }
            let end_excl = (e + 1).saturating_sub(wf / 4).max(start + 1);
            if end_excl - start < min_frames {
                continue;
            }
            let smax = row[s..=e].iter().copied().fold(0.0f32, f32::max);
            let r = (smax / a.ref_salience).clamp(0.0, 1.0);
            let velocity = (40.0 + 87.0 * r.sqrt()).clamp(1.0, 127.0) as u8;
            notes.push(Note {
                pitch,
                start: start as f64 * hop,
                end: end_excl as f64 * hop,
                velocity,
            });
        }
    }
    notes
}

