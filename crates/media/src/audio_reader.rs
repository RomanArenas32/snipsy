use std::collections::VecDeque;
use std::path::Path;

use anyhow::{anyhow, Result};

use crate::ffmpeg::{self, format::sample, media, software::resampling, ChannelLayout};

/// Si el instante pedido difiere más que esto de lo que hay en el buffer, se hace seek.
const DRIFT: f64 = 0.02;

/// Lector síncrono de audio, ya convertido a f32 intercalado a la tasa de salida.
pub struct AudioReader {
    ictx: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Audio,
    index: usize,
    time_base: f64,
    start: f64,
    rate: u32,
    channels: usize,
    layout: ChannelLayout,
    resampler: Option<(resampling::Context, (sample::Sample, u64, i32, u32))>,
    packet: ffmpeg::Packet,
    frame: ffmpeg::frame::Audio,
    fifo: VecDeque<f32>,
    /// Instante de la primera muestra de `fifo`.
    fifo_pts: f64,
    /// Tras un seek, descartar lo anterior a este instante.
    discard_until: Option<f64>,
    eof: bool,
    started: bool,
    /// Velocidad (≠ 1: el audio pasa por `atempo`, que conserva el tono).
    speed: f64,
    tempo: Option<crate::Tempo>,
}

impl AudioReader {
    pub fn open(path: &Path, rate: u32, channels: u16) -> Result<Self> {
        crate::init();
        let ictx = ffmpeg::format::input(path)?;
        let start = crate::container_start(&ictx);
        let stream = ictx
            .streams()
            .best(media::Type::Audio)
            .ok_or_else(|| anyhow!("{} no tiene audio", path.display()))?;
        let index = stream.index();
        let time_base = f64::from(stream.time_base());
        let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?
            .decoder()
            .audio()?;
        Ok(Self {
            ictx,
            decoder,
            index,
            time_base,
            start,
            rate,
            channels: channels as usize,
            layout: ChannelLayout::default(channels as i32),
            resampler: None,
            packet: ffmpeg::Packet::empty(),
            frame: ffmpeg::frame::Audio::empty(),
            fifo: VecDeque::new(),
            fifo_pts: 0.0,
            discard_until: None,
            eof: false,
            started: false,
            speed: 1.0,
            tempo: None,
        })
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    /// Reproduce el archivo a otra velocidad conservando el tono. Los instantes
    /// que reciben `mix_into` / `prepare` siguen siendo del archivo fuente.
    pub fn set_speed(&mut self, speed: f64) -> Result<()> {
        self.speed = speed;
        self.tempo = None;
        self.started = false;
        self.reset_tempo()
    }

    fn reset_tempo(&mut self) -> Result<()> {
        if (self.speed - 1.0).abs() > 1e-6 {
            self.tempo = Some(crate::Tempo::new(self.speed, self.rate, self.channels)?);
        }
        Ok(())
    }

    /// Agrega muestras (ya alineadas con la fuente) al buffer de salida.
    fn emit(&mut self, samples: &[f32]) -> Result<()> {
        match &mut self.tempo {
            Some(t) => {
                t.push(samples)?;
                t.pull(&mut self.fifo);
            }
            None => self.fifo.extend(samples),
        }
        Ok(())
    }

    /// Mezcla (suma) en `out` el audio a partir del instante `t`, con ganancia `gain`.
    pub fn mix_into(&mut self, t: f64, out: &mut [f32], gain: f32) -> Result<()> {
        self.mix_into_with(t, out, |_| gain)
    }

    /// Como [`mix_into`](Self::mix_into), con una ganancia por muestra:
    /// `gain(i)` recibe el índice de la muestra (por canal) dentro de `out`.
    pub fn mix_into_with(&mut self, t: f64, out: &mut [f32], gain: impl Fn(usize) -> f32) -> Result<()> {
        self.prepare(t, out.len())?;
        let n = out.len().min(self.fifo.len());
        let ch = self.channels;
        for (i, (o, s)) in out[..n].iter_mut().zip(self.fifo.drain(..n)).enumerate() {
            *o += s * gain(i / ch);
        }
        self.fifo_pts = t + (out.len() / self.channels) as f64 / self.rate as f64 * self.speed;
        Ok(())
    }

    /// Posiciona el lector en `t` y decodifica al menos `samples` muestras por adelantado.
    pub fn prepare(&mut self, t: f64, samples: usize) -> Result<()> {
        if !self.started && t < DRIFT {
            // Recién abierto y al inicio: no hace falta seek, solo descartar el preroll del códec.
            self.fifo_pts = t;
            self.discard_until = Some(t);
        } else if !self.started || (t - self.fifo_pts).abs() > DRIFT {
            crate::seek_input(&mut self.ictx, self.start, t);
            self.decoder.flush();
            self.fifo.clear();
            if self.tempo.is_some() {
                self.reset_tempo()?;
            }
            self.fifo_pts = t;
            self.discard_until = Some(t);
            self.eof = false;
        }
        self.started = true;
        while self.fifo.len() < samples && !self.eof {
            self.decode_more()?;
        }
        Ok(())
    }

    fn decode_more(&mut self) -> Result<()> {
        match self.packet.read(&mut self.ictx) {
            Ok(()) => {
                if self.packet.stream() != self.index {
                    return Ok(());
                }
                if let Err(e) = self.decoder.send_packet(&self.packet) {
                    log::debug!("paquete de audio inválido: {e}");
                }
            }
            Err(_) => {
                let _ = self.decoder.send_eof();
                self.eof = true;
            }
        }
        while self.decoder.receive_frame(&mut self.frame).is_ok() {
            self.push_frame()?;
        }
        if self.eof {
            if let Some(t) = &mut self.tempo {
                let _ = t.flush();
                t.pull(&mut self.fifo);
            }
        }
        Ok(())
    }

    fn push_frame(&mut self) -> Result<()> {
        let frame = &mut self.frame;
        // Algunos códecs no declaran el layout: asumir el estándar para N canales.
        let mut layout = frame.channel_layout();
        if layout.bits() == 0 {
            layout = ChannelLayout::default(frame.channels() as i32);
            frame.set_channel_layout(layout.clone());
        }
        let key = (frame.format(), layout.bits(), layout.channels(), frame.rate());
        if self.resampler.as_ref().is_none_or(|(_, k)| *k != key) {
            let ctx = resampling::Context::get(
                frame.format(),
                layout,
                frame.rate(),
                sample::Sample::F32(sample::Type::Packed),
                self.layout.clone(),
                self.rate,
            )?;
            self.resampler = Some((ctx, key));
        }
        let pts = frame.timestamp().or(frame.pts()).map(|p| p as f64 * self.time_base - self.start);

        let mut out = ffmpeg::frame::Audio::empty();
        if let Err(e) = self.resampler.as_mut().unwrap().0.run(frame, &mut out) {
            log::debug!("error de resampling: {e}");
            return Ok(());
        }
        let n = out.samples() * self.channels;
        let bytes = &out.data(0)[..n * 4];
        let samples: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]])).collect();

        if let Some(target) = self.discard_until {
            let pts = pts.unwrap_or(target);
            let end = pts + out.samples() as f64 / self.rate as f64;
            if end <= target {
                return Ok(());
            }
            let skip = if pts < target { ((target - pts) * self.rate as f64) as usize * self.channels } else { 0 };
            // Si el primer bloque empieza después del objetivo, rellenar con silencio.
            if pts > target {
                let pad = ((pts - target) * self.rate as f64) as usize * self.channels;
                self.emit(&vec![0.0; pad])?;
            }
            self.emit(&samples[skip.min(samples.len())..])?;
            self.discard_until = None;
        } else {
            self.emit(&samples)?;
        }
        Ok(())
    }
}
