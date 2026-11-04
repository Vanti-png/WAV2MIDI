use std::sync::atomic::{AtomicUsize, Ordering};

/// Paralel döngülerde ilerleme bildirmek için yardımcı.
pub struct ProgressCounter<'a> {
    done: AtomicUsize,
    total: usize,
    every: usize,
    scale: f32,
    offset: f32,
    cb: &'a (dyn Fn(f32) + Sync),
}

impl<'a> ProgressCounter<'a> {
    /// İlerleme `offset .. offset+scale` aralığına eşlenir.
    pub fn new(total: usize, offset: f32, scale: f32, cb: &'a (dyn Fn(f32) + Sync)) -> Self {
        Self {
            done: AtomicUsize::new(0),
            total: total.max(1),
            every: (total / 200).max(1),
            scale,
            offset,
            cb,
        }
    }

    pub fn tick(&self) {
        let d = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        if d % self.every == 0 || d == self.total {
            (self.cb)(self.offset + self.scale * d as f32 / self.total as f32);
        }
    }
}

pub fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = std::f32::consts::PI * 2.0 * i as f32 / n as f32;
            0.5 - 0.5 * x.cos()
        })
        .collect()
}

/// En yakın 2'nin kuvveti (log ölçeğinde yuvarlama).
pub fn nearest_pow2(x: f64) -> usize {
    let e = x.max(2.0).log2().round() as u32;
    1usize << e
}

pub fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Değerlerin `q` (0..1) yüzdeliği (NaN olmayanlar üzerinden).
pub fn percentile(values: &[f32], q: f32) -> f32 {
    let mut v: Vec<f32> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((v.len() - 1) as f32 * q.clamp(0.0, 1.0)).round() as usize;
    v[idx]
}

/// Desibele çevirip en yüksek kareye göre normalize eder (en gürültülü kare = 0 dB).
pub fn to_relative_db(rms: &[f32]) -> Vec<f32> {
    let max = rms.iter().copied().fold(0.0f32, f32::max).max(1e-12);
    rms.iter()
        .map(|&r| 20.0 * ((r.max(1e-12)) / max).log10())
        .map(|d| d.max(-120.0))
        .collect()
}

/// `values` dizisinde, `±radius` içinde yerel maksimum olan ve `thr` üzerindeki
/// indisleri döndürür (atak tepe noktaları).
pub fn peak_pick(values: &[f32], thr: f32, radius: usize) -> Vec<usize> {
    let n = values.len();
    let mut peaks = Vec::new();
    let mut last: Option<usize> = None;
    for t in 0..n {
        let v = values[t];
        if v < thr {
            continue;
        }
        let lo = t.saturating_sub(radius);
        let hi = (t + radius + 1).min(n);
        let is_max = (lo..hi).all(|i| values[i] <= v || i == t)
            && (lo..t).all(|i| values[i] < v);
        if is_max {
            if let Some(l) = last {
                if t - l <= radius {
                    continue;
                }
            }
            peaks.push(t);
            last = Some(t);
        }
    }
    peaks
}
