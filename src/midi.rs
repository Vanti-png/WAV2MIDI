//! Standart MIDI Dosyası (SMF, format 1) yazıcı.

use std::path::Path;

use crate::notes::Note;

pub const PPQ: u16 = 480;

#[derive(Clone, Debug, PartialEq)]
pub struct MidiOptions {
    pub bpm: f64,
    /// General MIDI program numarası (0 = Akustik Kuyruklu Piyano)
    pub program: u8,
    /// 0 = kuantalama yok, aksi hâlde nota değeri (4 = dörtlük, 8 = sekizlik, 16 = onaltılık...)
    pub quantize: u32,
    /// `Some(v)` ise tüm notalar bu velocity ile yazılır
    pub fixed_velocity: Option<u8>,
    /// 0..=15
    pub channel: u8,
    pub track_name: String,
}

impl Default for MidiOptions {
    fn default() -> Self {
        Self {
            bpm: 120.0,
            program: 0,
            quantize: 0,
            fixed_velocity: None,
            channel: 0,
            track_name: "WAV2MIDI".into(),
        }
    }
}

fn push_vlq(out: &mut Vec<u8>, mut v: u32) {
    let mut bytes = [0u8; 5];
    let mut i = 4;
    bytes[i] = (v & 0x7F) as u8;
    v >>= 7;
    while v > 0 {
        i -= 1;
        bytes[i] = ((v & 0x7F) as u8) | 0x80;
        v >>= 7;
    }
    out.extend_from_slice(&bytes[i..]);
}

fn push_meta_text(out: &mut Vec<u8>, kind: u8, text: &str) {
    let bytes = text.as_bytes();
    out.push(0x00);
    out.push(0xFF);
    out.push(kind);
    push_vlq(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 8);
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
    out
}

/// Notalardan SMF baytlarını üretir.
pub fn notes_to_midi(notes: &[Note], opts: &MidiOptions) -> Vec<u8> {
    let bpm = opts.bpm.clamp(10.0, 1000.0);
    let ch = opts.channel.min(15);
    let ticks_per_sec = bpm / 60.0 * PPQ as f64;
    let grid: u64 = if opts.quantize > 0 {
        ((PPQ as u64 * 4) / opts.quantize as u64).max(1)
    } else {
        1
    };
    let q = |secs: f64| -> u64 {
        let t = (secs.max(0.0) * ticks_per_sec).round() as u64;
        if grid > 1 {
            (t + grid / 2) / grid * grid
        } else {
            t
        }
    };

    // (başlangıç, bitiş, perde, velocity)
    let mut items: Vec<(u64, u64, u8, u8)> = notes
        .iter()
        .filter(|n| n.pitch < 128 && n.end > n.start)
        .map(|n| {
            let s = q(n.start);
            let mut e = q(n.end);
            if e <= s {
                e = s + grid.max(1);
            }
            let v = opts.fixed_velocity.unwrap_or(n.velocity).clamp(1, 127);
            (s, e, n.pitch, v)
        })
        .collect();
    items.sort_by_key(|i| (i.0, i.2));

    // Aynı perdedeki çakışmaları çöz (önceki notayı kısalt)
    let mut last: [Option<usize>; 128] = [None; 128];
    for i in 0..items.len() {
        let p = items[i].2 as usize;
        if let Some(j) = last[p] {
            if items[j].1 > items[i].0 {
                items[j].1 = items[i].0;
            }
        }
        last[p] = Some(i);
    }
    items.retain(|i| i.1 > i.0);

    // Olaylar: (tick, sıra [0 = note-off önce], bayt)
    let mut events: Vec<(u64, u8, [u8; 3])> = Vec::with_capacity(items.len() * 2);
    for &(s, e, p, v) in &items {
        events.push((s, 1, [0x90 | ch, p, v]));
        events.push((e, 0, [0x80 | ch, p, 0]));
    }
    events.sort_by_key(|e| (e.0, e.1, e.2[1]));

    // --- İz 0: tempo / ölçü
    let mut t0 = Vec::new();
    push_meta_text(&mut t0, 0x03, &opts.track_name);
    let uspq = (60_000_000.0 / bpm).round() as u32;
    t0.extend_from_slice(&[0x00, 0xFF, 0x51, 0x03]);
    t0.extend_from_slice(&uspq.to_be_bytes()[1..]);
    t0.extend_from_slice(&[0x00, 0xFF, 0x58, 0x04, 0x04, 0x02, 0x18, 0x08]);
    t0.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);

    // --- İz 1: notalar
    let mut t1 = Vec::new();
    push_meta_text(&mut t1, 0x03, "Notalar");
    t1.extend_from_slice(&[0x00, 0xC0 | ch, opts.program.min(127)]);
    let mut prev = 0u64;
    for (tick, _, bytes) in &events {
        let delta = (tick - prev).min(0x0FFF_FFFF) as u32;
        push_vlq(&mut t1, delta);
        t1.extend_from_slice(bytes);
        prev = *tick;
    }
    t1.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);

    let mut header = Vec::new();
    header.extend_from_slice(&1u16.to_be_bytes()); // format 1
    header.extend_from_slice(&2u16.to_be_bytes()); // iz sayısı
    header.extend_from_slice(&PPQ.to_be_bytes());

    let mut out = chunk(b"MThd", &header);
    out.extend(chunk(b"MTrk", &t0));
    out.extend(chunk(b"MTrk", &t1));
    out
}

pub fn write_midi(path: &Path, notes: &[Note], opts: &MidiOptions) -> std::io::Result<()> {
    std::fs::write(path, notes_to_midi(notes, opts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vlq() {
        let cases: [(u32, &[u8]); 6] = [
            (0, &[0x00]),
            (0x40, &[0x40]),
            (0x7F, &[0x7F]),
            (0x80, &[0x81, 0x00]),
            (0x2000, &[0xC0, 0x00]),
            (0x0FFF_FFFF, &[0xFF, 0xFF, 0xFF, 0x7F]),
        ];
        for (v, expected) in cases {
            let mut out = Vec::new();
            push_vlq(&mut out, v);
            assert_eq!(out, expected, "değer {v:#x}");
        }
    }
}
