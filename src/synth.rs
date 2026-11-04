//! Notaları dinleyebilmek için çok basit bir toplamalı (additive) sentezleyici.

use crate::notes::{midi_to_hz, Note};

/// Notaları mono bir tampona çizer. Uzunluk en az `min_secs` saniye olur.
pub fn render_notes(notes: &[Note], sr: u32, min_secs: f64) -> Vec<f32> {
    let srf = sr as f64;
    let release = 0.12;
    let end_secs = notes
        .iter()
        .map(|n| n.end + release)
        .fold(min_secs, f64::max);
    let len = (end_secs * srf).ceil() as usize + 1;
    let mut out = vec![0.0f32; len];

    // (harmonik, genlik, ek sönüm hızı)
    const PARTIALS: [(f32, f32, f32); 4] = [(1.0, 1.0, 0.0), (2.0, 0.45, 1.5), (3.0, 0.22, 3.0), (4.0, 0.1, 4.5)];

    for n in notes {
        let f0 = midi_to_hz(n.pitch as f32);
        let amp = (n.velocity as f32 / 127.0).powf(1.5) * 0.18;
        let s0 = (n.start * srf) as usize;
        let dur = ((n.end - n.start).max(0.02) * srf) as usize;
        let rel = (release * srf) as usize;
        let attack = (0.004 * srf) as usize;
        let decay = 1.2 + f0 / 500.0; // tiz notalar daha hızlı söner
        for &(h, a, extra) in PARTIALS.iter() {
            let f = f0 * h;
            if f as f64 >= srf / 2.0 {
                continue;
            }
            // Döner fazör ile sinüs (her örnekte sin() çağırmadan)
            let w = 2.0 * std::f64::consts::PI * f as f64 / srf;
            let (sw, cw) = w.sin_cos();
            let (mut s, mut c) = (0.0f64, 1.0f64);
            let k = ((decay + extra) as f64 / srf) as f32;
            let mut env_decay = 1.0f32;
            let dmul = (-k).exp();
            let mut level_at_release = 0.0f32;
            for i in 0..dur + rel {
                let idx = s0 + i;
                if idx >= len {
                    break;
                }
                let env_a = if i < attack { i as f32 / attack as f32 } else { 1.0 };
                let env = if i < dur {
                    level_at_release = env_a * env_decay;
                    level_at_release
                } else {
                    let r = (i - dur) as f32 / rel as f32;
                    level_at_release * (1.0 - r) * (1.0 - r)
                };
                out[idx] += amp * a * env * s as f32;
                env_decay *= dmul;
                let ns = s * cw + c * sw;
                let nc = c * cw - s * sw;
                s = ns;
                c = nc;
            }
        }
    }

    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak > 0.95 {
        let g = 0.95 / peak;
        out.iter_mut().for_each(|v| *v *= g);
    }
    out
}
