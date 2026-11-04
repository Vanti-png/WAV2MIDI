//! rodio tabanlı basit ses çalar (orijinal ses / sentezlenmiş MIDI önizlemesi).

use std::time::Instant;

pub struct Player {
    stream: Option<(rodio::OutputStream, rodio::OutputStreamHandle)>,
    sink: Option<rodio::Sink>,
    started: Option<Instant>,
    offset: f64,
    length: f64,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            stream: None,
            sink: None,
            started: None,
            offset: 0.0,
            length: 0.0,
        }
    }
}

impl Player {
    /// `samples` (mono) tamponunu `from_secs` konumundan çalmaya başlar.
    pub fn play(&mut self, samples: &[f32], sr: u32, from_secs: f64) -> Result<(), String> {
        self.stop();
        if self.stream.is_none() {
            let s = rodio::OutputStream::try_default()
                .map_err(|e| format!("Ses çıkışı açılamadı: {e}"))?;
            self.stream = Some(s);
        }
        let handle = &self.stream.as_ref().unwrap().1;
        let sink = rodio::Sink::try_new(handle).map_err(|e| format!("Ses çıkışı hatası: {e}"))?;
        let start = ((from_secs.max(0.0) * sr as f64) as usize).min(samples.len());
        let data = samples[start..].to_vec();
        if data.is_empty() {
            return Ok(());
        }
        self.length = samples.len() as f64 / sr as f64;
        sink.append(rodio::buffer::SamplesBuffer::new(1, sr, data));
        sink.play();
        self.sink = Some(sink);
        self.started = Some(Instant::now());
        self.offset = from_secs.max(0.0);
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(s) = self.sink.take() {
            s.stop();
        }
        self.started = None;
    }

    /// Çalıyorsa geçerli konum (saniye).
    pub fn position(&self) -> Option<f64> {
        let sink = self.sink.as_ref()?;
        if sink.empty() {
            return None;
        }
        let pos = self.offset + self.started?.elapsed().as_secs_f64();
        Some(pos.min(self.length))
    }

    pub fn is_playing(&self) -> bool {
        self.position().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ses aygıtını açar ve sessizlik çalar (duyulabilir ses üretmez).
    #[test]
    fn plays_silence_and_reports_position() {
        let mut p = Player::default();
        let sr = 44100;
        let silence = vec![0.0f32; sr as usize]; // 1 sn
        if let Err(e) = p.play(&silence, sr, 0.25) {
            eprintln!("ses aygıtı yok, test atlandı: {e}");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let pos = p.position().expect("çalıyor olmalı");
        assert!(pos >= 0.25 && pos < 1.0, "konum {pos}");
        p.stop();
        assert!(!p.is_playing());
    }
}
