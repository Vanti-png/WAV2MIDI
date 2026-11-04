/// Algılanmış tek bir nota. Zamanlar saniye cinsindendir.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    /// MIDI nota numarası (60 = C4 / Do)
    pub pitch: u8,
    pub start: f64,
    pub end: f64,
    /// 1..=127
    pub velocity: u8,
}

impl Note {
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// 60 -> "C4"
pub fn note_name(pitch: u8) -> String {
    format!(
        "{}{}",
        NOTE_NAMES[(pitch % 12) as usize],
        pitch as i32 / 12 - 1
    )
}

/// Siyah tuş mu? (C#, D#, F#, G#, A#)
pub fn is_black_key(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

pub fn midi_to_hz(m: f32) -> f32 {
    440.0 * 2f32.powf((m - 69.0) / 12.0)
}

pub fn hz_to_midi(f: f32) -> f32 {
    69.0 + 12.0 * (f / 440.0).log2()
}
