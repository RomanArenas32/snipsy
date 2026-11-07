use std::path::Path;

use anyhow::{Context as _, Result};

use crate::ffmpeg::{self, media};

/// Información básica de un archivo de medios.
#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub duration: f64,
    pub has_video: bool,
    pub has_audio: bool,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Rotación aplicada al mostrar (ya incluida en `width`/`height`).
    pub rotation: u32,
    pub video_codec: String,
    pub audio_codec: String,
}

impl MediaInfo {
    pub fn probe(path: &Path) -> Result<Self> {
        crate::init();
        let ictx = ffmpeg::format::input(path)
            .with_context(|| format!("no se pudo abrir {}", path.display()))?;

        let mut info = MediaInfo {
            duration: (ictx.duration().max(0) as f64) / ffmpeg::ffi::AV_TIME_BASE as f64,
            has_video: false,
            has_audio: false,
            width: 0,
            height: 0,
            fps: 30.0,
            rotation: 0,
            video_codec: String::new(),
            audio_codec: String::new(),
        };

        if let Some(stream) = ictx.streams().best(media::Type::Video) {
            let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
            info.video_codec = ctx.id().name().to_string();
            let dec = ctx.decoder().video()?;
            info.has_video = true;
            info.rotation = crate::stream_rotation(&stream);
            (info.width, info.height) = if info.rotation % 180 == 90 {
                (dec.height(), dec.width())
            } else {
                (dec.width(), dec.height())
            };
            let rate = stream.avg_frame_rate();
            let fps = if rate.denominator() != 0 { f64::from(rate) } else { 0.0 };
            if fps.is_finite() && fps > 0.0 {
                info.fps = fps;
            }
            if info.duration <= 0.0 && stream.duration() > 0 {
                info.duration = stream.duration() as f64 * f64::from(stream.time_base());
            }
        }
        if let Some(stream) = ictx.streams().best(media::Type::Audio) {
            let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
            info.audio_codec = ctx.id().name().to_string();
            info.has_audio = true;
        }
        Ok(info)
    }
}
