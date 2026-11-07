//! Salida de audio (cpal) y cola de muestras con reloj.
//!
//! El reloj de audio es el reloj maestro de la reproducción: el video se
//! sincroniza a la posición de la última muestra enviada a la tarjeta de sonido.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context as _, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

/// Bloque de muestras intercaladas (interleaved) en f32, ya en el formato del dispositivo.
pub struct AudioChunk {
    /// Instante (segundos) de la primera muestra del bloque.
    pub pts: f64,
    pub samples: Vec<f32>,
}

struct QueueState {
    serial: u64,
    chunks: VecDeque<AudioChunk>,
    /// Muestras ya consumidas del primer bloque.
    offset: usize,
    buffered: usize,
    clock: f64,
}

/// Cola compartida entre el decodificador (productor) y el callback de audio (consumidor).
pub struct AudioQueue {
    state: Mutex<QueueState>,
    playing: AtomicBool,
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioQueue {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            state: Mutex::new(QueueState {
                serial: 0,
                chunks: VecDeque::new(),
                offset: 0,
                buffered: 0,
                clock: 0.0,
            }),
            playing: AtomicBool::new(false),
            sample_rate,
            channels,
        }
    }

    /// Descarta todo lo encolado y fija el reloj (tras un seek).
    pub fn reset(&self, serial: u64, pts: f64) {
        let mut s = self.state.lock().unwrap();
        s.serial = serial;
        s.chunks.clear();
        s.offset = 0;
        s.buffered = 0;
        s.clock = pts;
    }

    /// Encola un bloque. Se ignora si pertenece a un seek anterior.
    pub fn push(&self, serial: u64, chunk: AudioChunk) {
        let mut s = self.state.lock().unwrap();
        if s.serial != serial || chunk.samples.is_empty() {
            return;
        }
        s.buffered += chunk.samples.len();
        s.chunks.push_back(chunk);
    }

    /// Segundos de audio pendientes de reproducir.
    pub fn buffered_secs(&self) -> f64 {
        let s = self.state.lock().unwrap();
        s.buffered as f64 / (self.sample_rate as f64 * self.channels as f64)
    }

    /// Posición actual de reproducción en segundos.
    pub fn clock(&self) -> f64 {
        self.state.lock().unwrap().clock
    }

    pub fn set_playing(&self, playing: bool) {
        self.playing.store(playing, Ordering::Release);
    }

    fn fill(&self, out: &mut [f32]) {
        out.fill(0.0);
        if !self.playing.load(Ordering::Acquire) {
            return;
        }
        let mut s = self.state.lock().unwrap();
        let samples_per_sec = self.sample_rate as f64 * self.channels as f64;
        let mut written = 0;
        while written < out.len() {
            let Some(front) = s.chunks.front() else { break };
            let avail = &front.samples[s.offset..];
            let n = avail.len().min(out.len() - written);
            out[written..written + n].copy_from_slice(&avail[..n]);
            written += n;
            let front_pts = front.pts;
            let front_len = front.samples.len();
            s.offset += n;
            s.buffered -= n;
            s.clock = front_pts + s.offset as f64 / samples_per_sec;
            if s.offset >= front_len {
                s.chunks.pop_front();
                s.offset = 0;
            }
        }
    }
}

/// Stream de salida activo. Mientras exista, el audio suena.
pub struct AudioOutput {
    _stream: cpal::Stream,
    pub queue: Arc<AudioQueue>,
}

impl AudioOutput {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no hay dispositivo de salida de audio"))?;
        let supported = device
            .default_output_config()
            .context("no se pudo leer la configuración de audio")?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let queue = Arc::new(AudioQueue::new(config.sample_rate, config.channels));

        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, queue.clone())?,
            SampleFormat::I16 => build::<i16>(&device, config, queue.clone())?,
            SampleFormat::I32 => build::<i32>(&device, config, queue.clone())?,
            SampleFormat::U16 => build::<u16>(&device, config, queue.clone())?,
            other => return Err(anyhow!("formato de audio no soportado: {other:?}")),
        };
        stream.play().context("no se pudo iniciar el audio")?;
        Ok(Self { _stream: stream, queue })
    }
}

fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, queue: Arc<AudioQueue>) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let mut scratch: Vec<f32> = Vec::new();
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            if scratch.len() < data.len() {
                scratch.resize(data.len(), 0.0);
            }
            let buf = &mut scratch[..data.len()];
            queue.fill(buf);
            for (d, s) in data.iter_mut().zip(buf.iter()) {
                *d = T::from_sample(*s);
            }
        },
        |err| log::error!("error de audio: {err}"),
        None,
    )?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_advances_clock_and_drops_stale_serials() {
        let q = AudioQueue::new(10, 1);
        q.reset(1, 5.0);
        q.push(0, AudioChunk { pts: 0.0, samples: vec![9.0; 10] }); // serial viejo
        q.push(1, AudioChunk { pts: 5.0, samples: vec![1.0; 10] });
        q.set_playing(true);
        let mut out = vec![0.0; 4];
        q.fill(&mut out);
        assert_eq!(out, vec![1.0; 4]);
        assert!((q.clock() - 5.4).abs() < 1e-9);
        assert!((q.buffered_secs() - 0.6).abs() < 1e-9);
    }

    #[test]
    fn paused_queue_outputs_silence() {
        let q = AudioQueue::new(10, 1);
        q.push(0, AudioChunk { pts: 0.0, samples: vec![1.0; 10] });
        let mut out = vec![7.0; 4];
        q.fill(&mut out);
        assert_eq!(out, vec![0.0; 4]);
        assert_eq!(q.clock(), 0.0);
    }
}
