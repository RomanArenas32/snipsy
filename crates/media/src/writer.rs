//! Escritura de MP4: video H.264/HEVC (por hardware si es posible) + audio AAC.

use std::path::Path;

use anyhow::{anyhow, Context as _, Result};

use crate::ffmpeg::{
    self, codec, color, encoder, format::sample, format::Pixel, software::scaling, ChannelLayout, Dictionary, Rational,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
}

#[derive(Clone, Debug)]
pub struct WriterSettings {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub codec: VideoCodec,
    /// Bits por segundo del video.
    pub bitrate: u64,
    /// Intentar primero el codificador por hardware (VideoToolbox en macOS).
    pub hardware: bool,
    /// Segundos entre keyframes (menos = saltar más rápido, archivos más grandes).
    pub keyframe_secs: f64,
    /// (frecuencia, canales) del audio; `None` para un archivo sin audio.
    pub audio: Option<(u32, u16)>,
    pub audio_bitrate: u64,
}

pub struct Mp4Writer {
    octx: ffmpeg::format::context::Output,
    video: encoder::Video,
    video_index: usize,
    video_tb: Rational,
    scaler: scaling::Context,
    rgba: ffmpeg::frame::Video,
    yuv: ffmpeg::frame::Video,
    frames: i64,
    audio: Option<AudioOut>,
    encoder_name: String,
}

struct AudioOut {
    encoder: encoder::Audio,
    index: usize,
    tb: Rational,
    channels: usize,
    frame_size: usize,
    layout: ChannelLayout,
    rate: u32,
    /// Muestras intercaladas pendientes de completar un cuadro de audio.
    pending: Vec<f32>,
    pts: i64,
}

/// Convierte fps decimales a fracción (29.97 → 30000/1001).
pub fn fps_rational(fps: f64) -> Rational {
    let fps = if fps.is_finite() && fps > 0.0 { fps } else { 30.0 };
    for base in [24.0, 30.0, 60.0, 120.0] {
        let ntsc = base * 1000.0 / 1001.0;
        if (fps - ntsc).abs() < 0.005 {
            return Rational::new((base * 1000.0) as i32, 1001);
        }
    }
    if (fps - fps.round()).abs() < 0.001 {
        Rational::new(fps.round() as i32, 1)
    } else {
        Rational::new((fps * 1000.0).round() as i32, 1000)
    }
}

impl Mp4Writer {
    pub fn create(path: &Path, s: &WriterSettings) -> Result<Self> {
        crate::init();
        anyhow::ensure!(s.width % 2 == 0 && s.height % 2 == 0, "el tamaño debe ser par ({}x{})", s.width, s.height);
        let mut octx = ffmpeg::format::output(path).with_context(|| format!("no se pudo crear {}", path.display()))?;
        let global_header = octx.format().flags().contains(ffmpeg::format::Flags::GLOBAL_HEADER);
        let rate = fps_rational(s.fps);

        // --- video: probar hardware y luego software
        let names: &[&str] = match (s.codec, s.hardware) {
            (VideoCodec::H264, true) => &["h264_videotoolbox", "libx264"],
            (VideoCodec::H264, false) => &["libx264", "h264_videotoolbox"],
            (VideoCodec::Hevc, true) => &["hevc_videotoolbox", "libx265"],
            (VideoCodec::Hevc, false) => &["libx265", "hevc_videotoolbox"],
        };
        let mut opened = None;
        let mut last_err = None;
        for name in names {
            let Some(codec) = encoder::find_by_name(name) else { continue };
            match open_video(codec, s, rate, global_header) {
                Ok(enc) => {
                    opened = Some((codec, enc, name.to_string()));
                    break;
                }
                Err(e) => {
                    log::warn!("no se pudo abrir {name}: {e}");
                    last_err = Some(e);
                }
            }
        }
        let (vcodec, video, encoder_name) = opened.ok_or_else(|| {
            anyhow!("no hay codificador disponible: {}", last_err.map(|e| e.to_string()).unwrap_or_default())
        })?;

        let video_index;
        let video_tb;
        {
            let mut ost = octx.add_stream(vcodec)?;
            ost.set_parameters(&video);
            ost.set_time_base(rate.invert());
            if s.codec == VideoCodec::Hevc {
                // 'hvc1' para que el HEVC se reproduzca en QuickTime/iPhone.
                unsafe {
                    (*(*ost.as_mut_ptr()).codecpar).codec_tag = u32::from_le_bytes(*b"hvc1");
                }
            }
            video_index = ost.index();
            video_tb = rate.invert();
        }

        // --- audio
        let audio = match s.audio {
            Some((arate, channels)) => Some(open_audio(&mut octx, arate, channels, s.audio_bitrate, global_header)?),
            None => None,
        };

        let mut opts = Dictionary::new();
        opts.set("movflags", "+faststart");
        octx.write_header_with(opts).context("no se pudo escribir la cabecera del MP4")?;

        // RGBA (rango completo) → YUV 4:2:0 BT.709 (rango limitado).
        let mut scaler = scaling::Context::get(
            Pixel::RGBA,
            s.width,
            s.height,
            Pixel::YUV420P,
            s.width,
            s.height,
            scaling::Flags::BICUBIC | scaling::Flags::ACCURATE_RND,
        )?;
        crate::set_colorspace(&mut scaler, true, true, true, false);

        Ok(Self {
            octx,
            video,
            video_index,
            video_tb,
            scaler,
            rgba: ffmpeg::frame::Video::new(Pixel::RGBA, s.width, s.height),
            yuv: ffmpeg::frame::Video::new(Pixel::YUV420P, s.width, s.height),
            frames: 0,
            audio,
            encoder_name,
        })
    }

    /// Nombre del codificador de video usado (p. ej. `h264_videotoolbox`).
    pub fn encoder_name(&self) -> &str {
        &self.encoder_name
    }

    /// Agrega el siguiente cuadro (RGBA compacto, del tamaño configurado).
    pub fn write_video(&mut self, rgba: &[u8]) -> Result<()> {
        let (w, h) = (self.rgba.width() as usize, self.rgba.height() as usize);
        anyhow::ensure!(rgba.len() >= w * h * 4, "cuadro de tamaño incorrecto");
        let stride = self.rgba.stride(0);
        let data = self.rgba.data_mut(0);
        for y in 0..h {
            data[y * stride..y * stride + w * 4].copy_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
        }
        self.scaler.run(&self.rgba, &mut self.yuv)?;
        self.yuv.set_pts(Some(self.frames));
        self.frames += 1;
        self.video.send_frame(&self.yuv)?;
        self.drain_video()
    }

    /// Agrega audio intercalado (cualquier cantidad de muestras).
    pub fn write_audio(&mut self, samples: &[f32]) -> Result<()> {
        let Some(a) = self.audio.as_mut() else { return Ok(()) };
        a.pending.extend_from_slice(samples);
        let block = a.frame_size * a.channels;
        while a.pending.len() >= block {
            let chunk: Vec<f32> = a.pending.drain(..block).collect();
            send_audio(a, &chunk)?;
            drain_audio(a, &mut self.octx)?;
        }
        Ok(())
    }

    /// Vacía los codificadores y cierra el archivo.
    pub fn finish(mut self) -> Result<()> {
        if let Some(a) = self.audio.as_mut() {
            if !a.pending.is_empty() {
                // Completar el último cuadro con silencio.
                let block = a.frame_size * a.channels;
                let mut chunk = std::mem::take(&mut a.pending);
                chunk.resize(block, 0.0);
                send_audio(a, &chunk)?;
            }
            a.encoder.send_eof()?;
            drain_audio(a, &mut self.octx)?;
        }
        self.video.send_eof()?;
        self.drain_video()?;
        self.octx.write_trailer()?;
        Ok(())
    }

    fn drain_video(&mut self) -> Result<()> {
        let mut packet = ffmpeg::Packet::empty();
        while self.video.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.video_index);
            // Cada paquete dura un cuadro (si no, el último no cuenta y los fps promedio salen mal).
            if packet.duration() <= 0 {
                packet.set_duration(1);
            }
            let ost_tb = self.octx.stream(self.video_index).unwrap().time_base();
            packet.rescale_ts(self.video_tb, ost_tb);
            packet.write_interleaved(&mut self.octx)?;
        }
        Ok(())
    }
}

fn open_video(codec: ffmpeg::Codec, s: &WriterSettings, rate: Rational, global_header: bool) -> Result<encoder::Video> {
    let mut enc = codec::context::Context::new_with_codec(codec).encoder().video()?;
    enc.set_width(s.width);
    enc.set_height(s.height);
    enc.set_format(Pixel::YUV420P);
    enc.set_time_base(rate.invert());
    enc.set_frame_rate(Some(rate));
    enc.set_bit_rate(s.bitrate as usize);
    enc.set_gop((f64::from(rate) * s.keyframe_secs).round().max(1.0) as u32);
    enc.set_colorspace(color::Space::BT709);
    enc.set_color_range(color::Range::MPEG);
    unsafe {
        let ctx = enc.as_mut_ptr();
        (*ctx).color_primaries = ffmpeg::ffi::AVColorPrimaries::AVCOL_PRI_BT709;
        (*ctx).color_trc = ffmpeg::ffi::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
    }
    if global_header {
        enc.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let mut opts = Dictionary::new();
    match codec.name() {
        "libx264" | "libx265" => {
            opts.set("preset", "medium");
            // Con bitrate objetivo y margen para escenas complejas.
            opts.set("maxrate", &(s.bitrate * 3 / 2).to_string());
            opts.set("bufsize", &(s.bitrate * 2).to_string());
        }
        _ => {
            opts.set("allow_sw", "1");
        }
    }
    if codec.name() == "libx264" {
        opts.set("profile", "high");
    }
    Ok(enc.open_as_with(codec, opts)?)
}

fn open_audio(
    octx: &mut ffmpeg::format::context::Output,
    rate: u32,
    channels: u16,
    bitrate: u64,
    global_header: bool,
) -> Result<AudioOut> {
    let codec = encoder::find(codec::Id::AAC).ok_or_else(|| anyhow!("no hay codificador AAC"))?;
    let layout = ChannelLayout::default(channels as i32);
    let mut enc = codec::context::Context::new_with_codec(codec).encoder().audio()?;
    enc.set_rate(rate as i32);
    enc.set_channel_layout(layout.clone());
    enc.set_format(sample::Sample::F32(sample::Type::Planar));
    enc.set_bit_rate(bitrate as usize);
    enc.set_time_base(Rational::new(1, rate as i32));
    if global_header {
        enc.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let encoder = enc.open_as_with(codec, Dictionary::new())?;
    let frame_size = (encoder.frame_size() as usize).max(1);
    let index;
    {
        let mut ost = octx.add_stream(codec)?;
        ost.set_parameters(&encoder);
        ost.set_time_base(Rational::new(1, rate as i32));
        index = ost.index();
    }
    Ok(AudioOut {
        encoder,
        index,
        tb: Rational::new(1, rate as i32),
        channels: channels as usize,
        frame_size,
        layout,
        rate,
        pending: Vec::new(),
        pts: 0,
    })
}

fn send_audio(a: &mut AudioOut, interleaved: &[f32]) -> Result<()> {
    let n = interleaved.len() / a.channels;
    let mut frame = ffmpeg::frame::Audio::new(sample::Sample::F32(sample::Type::Planar), n, a.layout.clone());
    frame.set_rate(a.rate);
    for ch in 0..a.channels {
        let plane = frame.plane_mut::<f32>(ch);
        for (i, v) in plane.iter_mut().enumerate().take(n) {
            *v = interleaved[i * a.channels + ch];
        }
    }
    frame.set_pts(Some(a.pts));
    a.pts += n as i64;
    a.encoder.send_frame(&frame)?;
    Ok(())
}

fn drain_audio(a: &mut AudioOut, octx: &mut ffmpeg::format::context::Output) -> Result<()> {
    let mut packet = ffmpeg::Packet::empty();
    while a.encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(a.index);
        let ost_tb = octx.stream(a.index).unwrap().time_base();
        packet.rescale_ts(a.tb, ost_tb);
        packet.write_interleaved(octx)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::fps_rational;
    use crate::ffmpeg::Rational;

    #[test]
    fn fps_to_fraction() {
        assert_eq!(fps_rational(30.0), Rational::new(30, 1));
        assert_eq!(fps_rational(29.97), Rational::new(30000, 1001));
        assert_eq!(fps_rational(23.976), Rational::new(24000, 1001));
        assert_eq!(fps_rational(12.5), Rational::new(12500, 1000));
    }
}
