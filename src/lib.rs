//! WAV2MIDI çekirdek kütüphanesi.
//!
//! * [`audio`]     – WAV / MP3 / FLAC / OGG / M4A çözme (symphonia)
//! * [`analysis`]  – perde (pitch) analizi ve nota çıkarımı
//! * [`midi`]      – Standart MIDI Dosyası (SMF) yazıcı
//! * [`synth`]     – notaları dinlemek için basit önizleme sentezleyici

pub mod analysis;
pub mod audio;
pub mod midi;
pub mod notes;
pub mod resample;
pub mod synth;

pub use analysis::{analyze, Analysis, AnalysisSettings, Mode, PolyWindow, TrackSettings};
pub use audio::{load_audio, AudioData};
pub use midi::{notes_to_midi, write_midi, MidiOptions};
pub use notes::{note_name, Note};
