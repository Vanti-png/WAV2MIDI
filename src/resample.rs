//! Pencereli sinc (Blackman) yeniden örnekleyici. Çekirdek önceden tabloya alınır,
//! çıktı örnekleri rayon ile paralel hesaplanır.

use rayon::prelude::*;

const TAPS_PER_SIDE: f64 = 16.0;
const TABLE_RES: usize = 512;

pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let ratio = to as f64 / from as f64;
    // Alçak geçiren kesim (giriş Nyquist'ine göre); aşağı örneklemede kenar yumuşatma
    let cutoff = ratio.min(1.0) * 0.96;
    let half = (TAPS_PER_SIDE / cutoff).ceil(); // giriş örneği cinsinden yarı genişlik
    let table_len = (half as usize) * TABLE_RES + 2;
    let table: Vec<f64> = (0..table_len)
        .map(|i| {
            let d = i as f64 / TABLE_RES as f64;
            if d >= half {
                return 0.0;
            }
            let sinc = if d == 0.0 {
                1.0
            } else {
                let a = std::f64::consts::PI * cutoff * d;
                a.sin() / a
            };
            let u = d / half; // 0..1
            let w = 0.42 + 0.5 * (std::f64::consts::PI * u).cos() + 0.08 * (2.0 * std::f64::consts::PI * u).cos();
            cutoff * sinc * w
        })
        .collect();

    let n_out = (x.len() as f64 * ratio).floor() as usize;
    let hw = half as isize;
    let len = x.len() as isize;
    let mut out = vec![0.0f32; n_out];
    out.par_chunks_mut(8192).enumerate().for_each(|(ci, chunk)| {
        for (j, o) in chunk.iter_mut().enumerate() {
            let i = ci * 8192 + j;
            let pos = i as f64 / ratio;
            let c = pos.floor() as isize;
            let mut acc = 0.0f64;
            for k in (c - hw + 1)..=(c + hw) {
                if k < 0 || k >= len {
                    continue;
                }
                let d = (pos - k as f64).abs() * TABLE_RES as f64;
                let di = d as usize;
                if di + 1 >= table_len {
                    continue;
                }
                let fr = d - di as f64;
                let w = table[di] * (1.0 - fr) + table[di + 1] * fr;
                acc += x[k as usize] as f64 * w;
            }
            *o = acc as f32;
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_survives_downsampling() {
        let (from, to) = (44100u32, 22050u32);
        let f = 1000.0f64;
        let x: Vec<f32> = (0..44100)
            .map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / from as f64).sin() as f32)
            .collect();
        let y = resample(&x, from, to);
        assert_eq!(y.len(), 22050);
        // Kenarlardan uzakta genlik ve faz korunmalı
        let mut err = 0.0f64;
        for (i, v) in y.iter().enumerate().skip(200).take(20000) {
            let e = (2.0 * std::f64::consts::PI * f * i as f64 / to as f64).sin();
            err = err.max((*v as f64 - e).abs());
        }
        assert!(err < 0.01, "en büyük hata {err}");
    }

    #[test]
    fn aliasing_is_suppressed() {
        // 15 kHz, 22.05 kHz'e indirildiğinde (Nyquist 11 kHz) neredeyse yok olmalı
        let x: Vec<f32> = (0..44100)
            .map(|i| (2.0 * std::f64::consts::PI * 15000.0 * i as f64 / 44100.0).sin() as f32)
            .collect();
        let y = resample(&x, 44100, 22050);
        let rms = (y[500..21000].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / 20500.0).sqrt();
        assert!(rms < 0.01, "rms {rms}");
    }
}
