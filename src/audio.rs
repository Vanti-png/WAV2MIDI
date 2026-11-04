use std::fs::File;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Çözülmüş ses: tek kanala (mono) indirilmiş örnekler.
#[derive(Clone, Debug)]
pub struct AudioData {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Orijinal dosyadaki kanal sayısı (bilgi amaçlı)
    pub channels: usize,
    pub codec: String,
}

impl AudioData {
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.samples.len() as f64 / self.sample_rate as f64
        }
    }
}

/// WAV, MP3, FLAC, OGG/Vorbis, M4A/AAC dosyalarını okur ve mono'ya indirger.
pub fn load_audio(path: &Path, progress: &dyn Fn(f32)) -> Result<AudioData> {
    let file = File::open(path).with_context(|| format!("Dosya açılamadı: {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|e| anyhow!("Desteklenmeyen veya bozuk ses dosyası: {e}"))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow!("Dosyada ses izi bulunamadı"))?;

    let track_id = track.id;
    let params = track.codec_params.clone();
    let total_frames = params.n_frames;
    let codec_name = symphonia::default::get_codecs()
        .get_codec(params.codec)
        .map(|d| d.short_name.to_uppercase())
        .unwrap_or_else(|| "?".into());

    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| anyhow!("Ses çözücü oluşturulamadı: {e}"))?;

    let mut sample_rate = params.sample_rate.unwrap_or(0);
    let mut channels = params.channels.map(|c| c.count()).unwrap_or(0);
    let mut mono: Vec<f32> = match total_frames {
        Some(n) => Vec::with_capacity(n as usize),
        None => Vec::new(),
    };

    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut buf_frames = 0usize;
    let mut packet_count = 0u64;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(anyhow!("Okuma hatası: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        packet_count += 1;
        let ts = packet.ts();

        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let frames = decoded.capacity();
                if sample_buf.is_none() || frames > buf_frames {
                    sample_buf = Some(SampleBuffer::<f32>::new(frames as u64, spec));
                    buf_frames = frames;
                }
                let buf = sample_buf.as_mut().unwrap();
                buf.copy_interleaved_ref(decoded);

                let ch = spec.channels.count().max(1);
                sample_rate = spec.rate;
                channels = ch;
                let inv = 1.0 / ch as f32;
                for frame in buf.samples().chunks_exact(ch) {
                    mono.push(frame.iter().sum::<f32>() * inv);
                }
            }
            // Bozuk paketleri atla
            Err(SymError::DecodeError(_)) => continue,
            Err(SymError::IoError(_)) => continue,
            Err(e) => return Err(anyhow!("Çözme hatası: {e}")),
        }

        if packet_count % 64 == 0 {
            if let Some(n) = total_frames {
                if n > 0 {
                    progress((ts as f64 / n as f64).clamp(0.0, 1.0) as f32);
                }
            }
        }
    }

    if sample_rate == 0 || mono.is_empty() {
        return Err(anyhow!("Dosyada çözülebilir ses verisi yok"));
    }
    progress(1.0);

    Ok(AudioData {
        samples: mono,
        sample_rate,
        channels,
        codec: codec_name,
    })
}
