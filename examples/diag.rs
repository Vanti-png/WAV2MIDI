//! Teşhis aracı: cargo run --release --example diag -- <dosya> [başlangıç_sn]
use wav2midi::{analyze, load_audio, note_name, AnalysisSettings, Mode, Note, TrackSettings};

fn stats(label: &str, notes: &[Note], dur: f64, from: f64) {
    let mut d: Vec<f64> = notes.iter().map(|n| n.duration()).collect();
    d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |q: f64| d.get(((d.len().max(1) - 1) as f64 * q) as usize).copied().unwrap_or(0.0);
    let total: f64 = d.iter().sum();
    let short = d.iter().filter(|&&x| x < 0.1).count();
    println!(
        "[{label}] {} nota, {:.1} nota/sn, süre p10/p50/p90 = {:.0}/{:.0}/{:.0} ms, <100ms: {}, ort. polifoni {:.2}",
        notes.len(),
        notes.len() as f64 / dur,
        pct(0.1) * 1000.0,
        pct(0.5) * 1000.0,
        pct(0.9) * 1000.0,
        short,
        total / dur
    );
    let win: Vec<String> = notes
        .iter()
        .filter(|n| n.start >= from && n.start < from + 4.0)
        .map(|n| format!("{}@{:.2}({:.0})", note_name(n.pitch), n.start, n.duration() * 1000.0))
        .collect();
    println!("  {from:.0}-{:.0} sn: {}", from + 4.0, win.join(" "));
}

fn main() {
    let path = std::env::args().nth(1).expect("dosya");
    let from: f64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let t = std::time::Instant::now();
    let audio = load_audio(path.as_ref(), &|_| {}).unwrap();
    let dur = audio.duration_secs();
    println!("{} sn, {} Hz, {} kanal, çözme {:.2} sn", dur, audio.sample_rate, audio.channels, t.elapsed().as_secs_f64());
    for mode in [Mode::Ai, Mode::Poly, Mode::Mono] {
        let s = AnalysisSettings { mode, ..Default::default() };
        let t = std::time::Instant::now();
        let a = analyze(&audio, &s, &|_| {}).unwrap();
        let el = t.elapsed().as_secs_f64();
        let notes = a.track(&TrackSettings::default());
        println!("analiz {:?}: {:.2} sn", mode, el);
        stats(&format!("{mode:?}"), &notes, dur, from);
    }
}
