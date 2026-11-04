//! Uçtan uca testler: sentetik ses -> WAV -> analiz -> nota -> MIDI.

use std::path::PathBuf;

use wav2midi::{
    analyze, load_audio, note_name, notes_to_midi, AnalysisSettings, MidiOptions, Mode, Note,
    TrackSettings,
};

fn hz(m: u8) -> f32 {
    440.0 * 2f32.powf((m as f32 - 69.0) / 12.0)
}

/// Harmonik açısından zengin, sönümlü bir ton ekler.
fn add_tone(buf: &mut [f32], sr: u32, pitch: u8, start: f64, dur: f64, amp: f32, harmonics: usize, decay: f32) {
    let s0 = (start * sr as f64) as usize;
    let len = (dur * sr as f64) as usize;
    let f0 = hz(pitch);
    for i in 0..len {
        let idx = s0 + i;
        if idx >= buf.len() {
            break;
        }
        let t = i as f32 / sr as f32;
        let att = (t / 0.01).min(1.0);
        let rel = ((dur as f32 - t) / 0.02).clamp(0.0, 1.0);
        let env = att * rel * (-decay * t).exp();
        let mut v = 0.0;
        for h in 1..=harmonics {
            let f = f0 * h as f32;
            if f > (sr as f32 / 2.0).min(10000.0) {
                break;
            }
            v += (2.0 * std::f32::consts::PI * f * t).sin() / (h as f32).powf(1.3);
        }
        buf[idx] += amp * env * v;
    }
}

fn write_wav(name: &str, samples: &[f32], sr: u32, channels: u16) -> PathBuf {
    let path = std::env::temp_dir().join(format!("wav2midi_test_{name}_{}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels,
        sample_rate: sr,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for &s in samples {
        for _ in 0..channels {
            w.write_sample((s.clamp(-1.0, 1.0) * 32000.0) as i16).unwrap();
        }
    }
    w.finalize().unwrap();
    path
}

fn describe(notes: &[Note]) -> String {
    notes
        .iter()
        .map(|n| format!("{}[{:.2}-{:.2} v{}]", note_name(n.pitch), n.start, n.end, n.velocity))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn mono_melody_is_transcribed() {
    let sr = 44100;
    let melody = [60u8, 62, 64, 65, 67, 69, 71, 72];
    let (dur, gap) = (0.30, 0.05);
    let total = melody.len() as f64 * (dur + gap) + 0.3;
    let mut buf = vec![0.0f32; (total * sr as f64) as usize];
    for (i, &p) in melody.iter().enumerate() {
        add_tone(&mut buf, sr, p, 0.1 + i as f64 * (dur + gap), dur, 0.3, 6, 1.0);
    }
    let path = write_wav("mono", &buf, sr, 1);
    let audio = load_audio(&path, &|_| {}).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(audio.sample_rate, sr);

    let settings = AnalysisSettings {
        mode: Mode::Mono,
        ..Default::default()
    };
    let analysis = analyze(&audio, &settings, &|_| {});
    let notes = analysis.track(&TrackSettings::default());
    println!("mono: {}", describe(&notes));

    let pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
    assert_eq!(pitches, melody, "notalar: {}", describe(&notes));
    for (i, n) in notes.iter().enumerate() {
        let expected = 0.1 + i as f64 * (dur + gap);
        assert!((n.start - expected).abs() < 0.04, "nota {i} başlangıcı {} beklenen {expected}", n.start);
        assert!((n.duration() - dur).abs() < 0.08, "nota {i} süresi {}", n.duration());
    }
}

#[test]
fn poly_chords_are_transcribed() {
    let sr = 44100;
    let chords: [&[u8]; 4] = [&[60, 64, 67], &[65, 69, 72], &[55, 62, 67, 71], &[48, 60, 64]];
    let (dur, gap) = (0.9, 0.1);
    let total = chords.len() as f64 * (dur + gap) + 0.3;
    let mut buf = vec![0.0f32; (total * sr as f64) as usize];
    for (i, chord) in chords.iter().enumerate() {
        for &p in chord.iter() {
            add_tone(&mut buf, sr, p, 0.1 + i as f64 * (dur + gap), dur, 0.15, 25, 1.5);
        }
    }
    let path = write_wav("poly", &buf, sr, 1);
    let audio = load_audio(&path, &|_| {}).unwrap();
    let _ = std::fs::remove_file(&path);

    let settings = AnalysisSettings::default();
    assert_eq!(settings.mode, Mode::Poly);
    let analysis = analyze(&audio, &settings, &|_| {});
    let notes = analysis.track(&TrackSettings::default());
    println!("poly: {}", describe(&notes));
    let expected_total: usize = chords.iter().map(|c| c.len()).sum();
    assert_eq!(notes.len(), expected_total, "fazladan/eksik nota: {}", describe(&notes));

    for (i, chord) in chords.iter().enumerate() {
        let start = 0.1 + i as f64 * (dur + gap);
        let mid = start + dur / 2.0;
        let mut found: Vec<u8> = notes
            .iter()
            .filter(|n| n.start <= mid && n.end >= mid)
            .map(|n| n.pitch)
            .collect();
        found.sort();
        found.dedup();
        let mut expected = chord.to_vec();
        expected.sort();
        assert_eq!(found, expected, "akor {i}: {}", describe(&notes));
        // Akor notalarının başlangıcı atağa yakın olmalı
        for n in notes.iter().filter(|n| n.start <= mid && n.end >= mid) {
            assert!((n.start - start).abs() < 0.06, "akor {i} nota {} başlangıç {}", n.pitch, n.start);
        }
    }
}

#[test]
fn stereo_48k_wav_is_mixed_to_mono() {
    let sr = 48000;
    let mut buf = vec![0.0f32; sr as usize];
    add_tone(&mut buf, sr, 69, 0.0, 1.0, 0.5, 1, 0.0);
    let path = write_wav("stereo", &buf, sr, 2);
    let audio = load_audio(&path, &|_| {}).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(audio.sample_rate, sr);
    assert_eq!(audio.channels, 2);
    assert_eq!(audio.samples.len(), sr as usize);
    assert!((audio.duration_secs() - 1.0).abs() < 1e-6);
}

#[test]
fn midi_file_is_valid() {
    let notes = vec![
        Note { pitch: 60, start: 0.0, end: 0.5, velocity: 100 },
        Note { pitch: 64, start: 0.5, end: 1.0, velocity: 90 },
        // Aynı perde çakışması: ilk nota kısaltılmalı
        Note { pitch: 64, start: 0.75, end: 1.25, velocity: 80 },
        Note { pitch: 67, start: 1.0, end: 2.0, velocity: 70 },
    ];
    let opts = MidiOptions {
        bpm: 120.0,
        program: 24,
        ..Default::default()
    };
    let bytes = notes_to_midi(&notes, &opts);
    let smf = midly::Smf::parse(&bytes).expect("geçerli MIDI");
    assert_eq!(smf.header.format, midly::Format::Parallel);
    assert_eq!(smf.header.timing, midly::Timing::Metrical(480.into()));
    assert_eq!(smf.tracks.len(), 2);

    let has_tempo = smf.tracks[0].iter().any(|e| {
        matches!(e.kind, midly::TrackEventKind::Meta(midly::MetaMessage::Tempo(t)) if t.as_int() == 500_000)
    });
    assert!(has_tempo);

    let mut tick = 0u32;
    let mut ons = Vec::new();
    let mut offs = Vec::new();
    let mut program = None;
    for e in &smf.tracks[1] {
        tick += e.delta.as_int();
        if let midly::TrackEventKind::Midi { message, .. } = e.kind {
            match message {
                midly::MidiMessage::NoteOn { key, vel } if vel > 0 => ons.push((tick, key.as_int())),
                midly::MidiMessage::NoteOff { key, .. } => offs.push((tick, key.as_int())),
                midly::MidiMessage::ProgramChange { program: p } => program = Some(p.as_int()),
                _ => {}
            }
        }
    }
    assert_eq!(program, Some(24));
    // 120 BPM, 480 PPQ -> 960 tick/sn
    assert_eq!(ons, vec![(0, 60), (480, 64), (720, 64), (960, 67)]);
    assert_eq!(offs.len(), 4);
    assert!(offs.contains(&(720, 64)), "çakışan nota kısaltılmalı: {offs:?}");
    assert!(offs.contains(&(1920, 67)));
}

#[test]
fn quantization_snaps_to_grid() {
    let notes = vec![Note { pitch: 60, start: 0.13, end: 0.37, velocity: 100 }];
    let opts = MidiOptions { bpm: 120.0, quantize: 16, ..Default::default() };
    let smf_bytes = notes_to_midi(&notes, &opts);
    let smf = midly::Smf::parse(&smf_bytes).unwrap();
    let mut tick = 0u32;
    let mut times = Vec::new();
    for e in &smf.tracks[1] {
        tick += e.delta.as_int();
        if let midly::TrackEventKind::Midi { .. } = e.kind {
            times.push(tick);
        }
    }
    // program change @0, note on/off onaltılık (120 tick) ızgarada
    assert!(times[1..].iter().all(|t| t % 120 == 0), "{times:?}");
}

