//! Yapay zekâ modu: Spotify **basic-pitch** (ICASSP 2022) sinir ağı modeli.
//!
//! Model (`assets/basic_pitch/nmp.onnx`, Apache-2.0) exe'ye gömülüdür ve saf Rust
//! ONNX çalıştırıcısı [tract](https://github.com/sonos/tract) ile çalıştırılır.
//! Pencereleme, kare-zaman hizalaması ve nota oluşturma, basic-pitch'in Python
//! uygulamasıyla (`inference.py`, `note_creation.py`) aynı mantığı izler.

use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use rayon::prelude::*;
use tract_onnx::prelude::*;

use super::util::ProgressCounter;
use super::TrackSettings;
use crate::notes::Note;
use crate::resample::resample;

const MODEL: &[u8] = include_bytes!("../../assets/basic_pitch/nmp.onnx");

pub const MODEL_SAMPLE_RATE: u32 = 22050;
const FFT_HOP: usize = 256;
const AUDIO_N_SAMPLES: usize = 22050 * 2 - FFT_HOP; // 43844
const ANNOT_N_FRAMES: usize = 172; // 2 sn pencere başına kare
const ANNOTATIONS_FPS: usize = 22050 / FFT_HOP; // 86 (tamsayı bölme, orijinaldeki gibi)
const N_OVERLAP_FRAMES: usize = 30;
const OVERLAP_LEN: usize = N_OVERLAP_FRAMES * FFT_HOP; // 7680
const HOP_SIZE: usize = AUDIO_N_SAMPLES - OVERLAP_LEN; // 36164
pub const N_PITCHES: usize = 88;
const MIDI_OFFSET: usize = 21;
const ENERGY_TOL: usize = 11;

type Plan = std::sync::Arc<TypedRunnableModel>;

fn plan() -> Result<&'static Plan> {
    static PLAN: OnceLock<std::result::Result<Plan, String>> = OnceLock::new();
    PLAN.get_or_init(|| {
        tract_onnx::onnx()
            .model_for_read(&mut &MODEL[..])
            .and_then(|m| m.with_input_fact(0, f32::fact([1, AUDIO_N_SAMPLES, 1]).into()))
            .and_then(|m| m.into_optimized())
            .and_then(|m| m.into_runnable())
            .map_err(|e| format!("{e:?}"))
    })
    .as_ref()
    .map_err(|e| anyhow!("Yapay zekâ modeli yüklenemedi: {e}"))
}

/// Çıkış sırası: modelin çıkış düğümü adlarından "note" ve "onset" indisleri.
fn output_indices() -> Result<(usize, usize)> {
    static IDX: OnceLock<std::result::Result<(usize, usize), String>> = OnceLock::new();
    IDX.get_or_init(|| {
        let model = tract_onnx::onnx()
            .model_for_read(&mut &MODEL[..])
            .map_err(|e| format!("{e:?}"))?;
        let outlets = model.output_outlets().map_err(|e| format!("{e:?}"))?;
        let find = |suffix: &str| {
            outlets
                .iter()
                .position(|o| model.node(o.node).name.ends_with(suffix))
                .ok_or_else(|| format!("çıkış bulunamadı: {suffix}"))
        };
        // basic-pitch: StatefulPartitionedCall:1 = note, :2 = onset, :0 = contour
        Ok((find("StatefulPartitionedCall:1")?, find("StatefulPartitionedCall:2")?))
    })
    .clone()
    .map_err(|e| anyhow!("Model çıkışları okunamadı: {e}"))
}

/// Modelin kare kare çıktısı (her kare 88 tuş, A0..C8).
pub struct AiAnalysis {
    pub n_frames: usize,
    /// n_frames × 88, nota etkinliği (0..1)
    pub note: Vec<f32>,
    /// n_frames × 88, nota başlangıcı olasılığı (0..1)
    pub onset: Vec<f32>,
    /// Her karenin zamanı (sn)
    pub times: Vec<f64>,
    pub min_note: u8,
    pub max_note: u8,
}

pub fn analyze_ai(
    samples: &[f32],
    sr: u32,
    min_note: u8,
    max_note: u8,
    progress: &(dyn Fn(f32) + Sync),
) -> Result<AiAnalysis> {
    progress(0.0);
    let plan = plan()?;
    let (note_i, onset_i) = output_indices()?;
    let audio = resample(samples, sr, MODEL_SAMPLE_RATE);
    progress(0.08);

    let original_len = audio.len();
    let mut padded = vec![0.0f32; OVERLAP_LEN / 2];
    padded.extend_from_slice(&audio);
    drop(audio);

    let starts: Vec<usize> = (0..padded.len()).step_by(HOP_SIZE).collect();
    let counter = ProgressCounter::new(starts.len(), 0.08, 0.92, progress);
    let n_olap = N_OVERLAP_FRAMES / 2;
    let keep = ANNOT_N_FRAMES - 2 * n_olap;

    let windows: Vec<Result<(Vec<f32>, Vec<f32>)>> = starts
        .par_iter()
        .map(|&s| {
            let mut w = vec![0.0f32; AUDIO_N_SAMPLES];
            let end = (s + AUDIO_N_SAMPLES).min(padded.len());
            w[..end - s].copy_from_slice(&padded[s..end]);
            let input: Tensor = tract_ndarray::Array3::from_shape_vec((1, AUDIO_N_SAMPLES, 1), w)
                .map_err(|e| anyhow!("{e}"))?
                .into();
            let out = plan
                .run(tvec!(input.into()))
                .map_err(|e| anyhow!("Model çalıştırılamadı: {e:?}"))?;
            let take = |idx: usize| -> Result<Vec<f32>> {
                let v = out[idx].to_plain_array_view::<f32>().map_err(|e| anyhow!("{e:?}"))?;
                let shape = v.shape().to_vec();
                if shape.len() != 3 || shape[1] != ANNOT_N_FRAMES || shape[2] != N_PITCHES {
                    return Err(anyhow!("Beklenmeyen model çıktısı: {shape:?}"));
                }
                let flat: Vec<f32> = v.iter().copied().collect();
                Ok(flat[n_olap * N_PITCHES..(n_olap + keep) * N_PITCHES].to_vec())
            };
            let r = (take(note_i)?, take(onset_i)?);
            counter.tick();
            Ok(r)
        })
        .collect();

    let n_frames = ((original_len as f64) * (ANNOTATIONS_FPS as f64 / MODEL_SAMPLE_RATE as f64)).floor() as usize;
    let mut note = Vec::with_capacity(starts.len() * keep * N_PITCHES);
    let mut onset = Vec::with_capacity(note.capacity());
    for w in windows {
        let (n, o) = w?;
        note.extend_from_slice(&n);
        onset.extend_from_slice(&o);
    }
    let n_frames = n_frames.min(note.len() / N_PITCHES);
    note.truncate(n_frames * N_PITCHES);
    onset.truncate(n_frames * N_PITCHES);

    // basic-pitch'in kare -> zaman eşlemesi (pencere örtüşmesi düzeltmesiyle)
    let hop_secs = FFT_HOP as f64 / MODEL_SAMPLE_RATE as f64;
    let window_offset = hop_secs * (ANNOT_N_FRAMES as f64 - AUDIO_N_SAMPLES as f64 / FFT_HOP as f64) + 0.0018;
    let times = (0..n_frames)
        .map(|t| t as f64 * hop_secs - window_offset * (t / ANNOT_N_FRAMES) as f64)
        .collect();

    progress(1.0);
    Ok(AiAnalysis {
        n_frames,
        note,
        onset,
        times,
        min_note,
        max_note,
    })
}

// ---------------------------------------------------------------------------
// Nota oluşturma (basic-pitch `output_to_notes_polyphonic`)
// ---------------------------------------------------------------------------

pub fn track_ai(a: &AiAnalysis, p: &TrackSettings, min_note: u8, max_note: u8) -> Vec<Note> {
    let n = a.n_frames;
    if n < 3 {
        return Vec::new();
    }
    let sens = p.sensitivity.clamp(0.0, 1.0);
    // Varsayılan hassasiyette (0.5) basic-pitch varsayılanları: 0.5 / 0.3
    let onset_thresh = (0.8 - 0.6 * sens).clamp(0.05, 0.95);
    let frame_thresh = (0.5 - 0.4 * sens).clamp(0.05, 0.95);
    let fps = MODEL_SAMPLE_RATE as f64 / FFT_HOP as f64;
    let min_len = ((p.min_note_ms as f64 / 1000.0) * fps).round() as usize;

    let lo = (min_note as usize).saturating_sub(MIDI_OFFSET).min(N_PITCHES - 1);
    let hi = (max_note as usize).saturating_sub(MIDI_OFFSET).min(N_PITCHES - 1);
    let in_range = |f: usize| f >= lo && f <= hi;
    let idx = |t: usize, f: usize| t * N_PITCHES + f;

    let frames = &a.note;
    let mut onsets: Vec<f32> = a.onset.clone();
    // Aralık dışını sıfırla
    for t in 0..n {
        for f in 0..N_PITCHES {
            if !in_range(f) {
                onsets[idx(t, f)] = 0.0;
            }
        }
    }

    // Nota etkinliğindeki ani artışlardan başlangıç çıkar (infer_onsets)
    if p.split_onsets {
        let mut diff = vec![0.0f32; n * N_PITCHES];
        let mut max_diff = 0.0f32;
        for t in 2..n {
            for f in 0..N_PITCHES {
                if !in_range(f) {
                    continue;
                }
                let d1 = frames[idx(t, f)] - frames[idx(t - 1, f)];
                let d2 = frames[idx(t, f)] - frames[idx(t - 2, f)];
                let d = d1.min(d2).max(0.0);
                diff[idx(t, f)] = d;
                max_diff = max_diff.max(d);
            }
        }
        let max_onset = onsets.iter().copied().fold(0.0f32, f32::max);
        if max_diff > 0.0 {
            let scale = max_onset / max_diff;
            for (o, d) in onsets.iter_mut().zip(diff.iter()) {
                *o = o.max(d * scale);
            }
        }
    }

    // Zaman ekseninde yerel tepe olan başlangıçlar
    let mut peaks: Vec<(usize, usize)> = Vec::new();
    for t in 1..n - 1 {
        for f in lo..=hi {
            let v = onsets[idx(t, f)];
            if v >= onset_thresh && v > onsets[idx(t - 1, f)] && v > onsets[idx(t + 1, f)] {
                peaks.push((t, f));
            }
        }
    }
    // Zamanda geriye doğru işle (orijinaldeki gibi)
    peaks.sort_by(|a, b| b.cmp(a));

    let mut remaining: Vec<f32> = frames.clone();
    for t in 0..n {
        for f in 0..N_PITCHES {
            if !in_range(f) {
                remaining[idx(t, f)] = 0.0;
            }
        }
    }

    let mut events: Vec<(usize, usize, usize, f32)> = Vec::new(); // (başlangıç, bitiş, perde idx, genlik)
    let mean_amp = |s: usize, e: usize, f: usize| -> f32 {
        let sum: f32 = (s..e).map(|t| frames[idx(t, f)]).sum();
        sum / (e - s).max(1) as f32
    };
    let clear = |rem: &mut Vec<f32>, t: usize, f: usize| {
        rem[idx(t, f)] = 0.0;
        if f + 1 < N_PITCHES {
            rem[idx(t, f + 1)] = 0.0;
        }
        if f > 0 {
            rem[idx(t, f - 1)] = 0.0;
        }
    };

    for &(start, f) in &peaks {
        if start >= n - 1 {
            continue;
        }
        let mut i = start + 1;
        let mut k = 0;
        while i < n - 1 && k < ENERGY_TOL {
            if remaining[idx(i, f)] < frame_thresh {
                k += 1;
            } else {
                k = 0;
            }
            i += 1;
        }
        i -= k;
        if i - start <= min_len {
            continue;
        }
        for t in start..i {
            clear(&mut remaining, t, f);
        }
        events.push((start, i, f, mean_amp(start, i, f)));
    }

    // "Melodia hilesi": başlangıcı algılanmamış ama güçlü etkinlikleri de nota yap.
    // Kalan enerji yalnızca sıfırlanabildiği için, eşik üstü hücreleri bir kez
    // sıralayıp sırayla işlemek orijinaldeki "her adımda argmax" ile aynıdır.
    let mut cands: Vec<(f32, usize, usize)> = Vec::new();
    for t in 0..n {
        for f in lo..=hi {
            let v = remaining[idx(t, f)];
            if v > frame_thresh {
                cands.push((v, t, f));
            }
        }
    }
    cands.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for &(v, mid, f) in &cands {
        if remaining[idx(mid, f)] != v {
            continue; // zaten başka bir notaya ait
        }
        remaining[idx(mid, f)] = 0.0;
        // ileri
        let mut i = mid + 1;
        let mut k = 0;
        while i < n - 1 && k < ENERGY_TOL {
            if remaining[idx(i, f)] < frame_thresh {
                k += 1;
            } else {
                k = 0;
            }
            clear(&mut remaining, i, f);
            i += 1;
        }
        let i_end = i - 1 - k;
        // geri
        let mut i = mid as isize - 1;
        let mut k = 0;
        while i > 0 && k < ENERGY_TOL {
            let iu = i as usize;
            if remaining[idx(iu, f)] < frame_thresh {
                k += 1;
            } else {
                k = 0;
            }
            clear(&mut remaining, iu, f);
            i -= 1;
        }
        let i_start = (i + 1 + k as isize).max(0) as usize;
        if i_end <= i_start || i_end - i_start <= min_len {
            continue;
        }
        events.push((i_start, i_end, f, mean_amp(i_start, i_end, f)));
    }

    events
        .into_iter()
        .filter(|e| e.1 < n)
        .map(|(s, e, f, amp)| Note {
            pitch: (f + MIDI_OFFSET) as u8,
            start: a.times[s].max(0.0),
            end: a.times[e].max(a.times[s] + 0.01),
            velocity: (127.0 * amp).round().clamp(1.0, 127.0) as u8,
        })
        .collect()
}
