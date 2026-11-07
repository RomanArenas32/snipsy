//! Cambio de velocidad del audio conservando el tono (filtro `atempo` de FFmpeg, WSOLA).

use std::collections::VecDeque;

use anyhow::{anyhow, Result};

use crate::ffmpeg::{self, filter, format::sample, ChannelLayout};

pub struct Tempo {
    graph: filter::Graph,
    rate: u32,
    channels: usize,
    layout: ChannelLayout,
    pts: i64,
}

/// `atempo` admite 0.5 – 100 por instancia: para valores menores se encadenan.
fn chain(speed: f64) -> String {
    let mut s = speed;
    let mut parts = Vec::new();
    while s < 0.5 {
        parts.push("atempo=0.5".to_owned());
        s /= 0.5;
    }
    parts.push(format!("atempo={s:.6}"));
    parts.join(",")
}

impl Tempo {
    /// Audio f32 intercalado a `rate` Hz con `channels` canales.
    pub fn new(speed: f64, rate: u32, channels: usize) -> Result<Self> {
        let layout = ChannelLayout::default(channels as i32);
        let name = match channels {
            1 => "mono".to_owned(),
            2 => "stereo".to_owned(),
            n => format!("{n}c"),
        };
        let mut graph = filter::Graph::new();
        let abuffer = filter::find("abuffer").ok_or_else(|| anyhow!("falta el filtro abuffer"))?;
        let sink = filter::find("abuffersink").ok_or_else(|| anyhow!("falta el filtro abuffersink"))?;
        graph.add(&abuffer, "in", &format!("time_base=1/{rate}:sample_rate={rate}:sample_fmt=flt:channel_layout={name}"))?;
        let mut out = graph.add(&sink, "out", "")?;
        out.set_sample_format(sample::Sample::F32(sample::Type::Packed));
        graph.output("in", 0)?.input("out", 0)?.parse(&chain(speed))?;
        graph.validate()?;
        Ok(Self { graph, rate, channels, layout, pts: 0 })
    }

    /// Entrega muestras intercaladas al filtro.
    pub fn push(&mut self, samples: &[f32]) -> Result<()> {
        let n = samples.len() / self.channels;
        if n == 0 {
            return Ok(());
        }
        let mut frame = ffmpeg::frame::Audio::new(sample::Sample::F32(sample::Type::Packed), n, self.layout.clone());
        frame.set_rate(self.rate);
        frame.set_pts(Some(self.pts));
        self.pts += n as i64;
        let data = frame.data_mut(0);
        for (chunk, s) in data.chunks_exact_mut(4).zip(&samples[..n * self.channels]) {
            chunk.copy_from_slice(&s.to_ne_bytes());
        }
        self.graph.get("in").ok_or_else(|| anyhow!("filtro sin entrada"))?.source().add(&frame)?;
        Ok(())
    }

    /// Fin del audio: vacía lo que el filtro tenga retenido.
    pub fn flush(&mut self) -> Result<()> {
        self.graph.get("in").ok_or_else(|| anyhow!("filtro sin entrada"))?.source().flush()?;
        Ok(())
    }

    /// Mueve al final de `out` todas las muestras ya procesadas.
    pub fn pull(&mut self, out: &mut VecDeque<f32>) {
        let Some(mut sink) = self.graph.get("out") else { return };
        let mut frame = ffmpeg::frame::Audio::empty();
        while sink.sink().frame(&mut frame).is_ok() {
            let n = frame.samples() * self.channels;
            let bytes = &frame.data(0)[..n * 4];
            out.extend(bytes.chunks_exact(4).map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]])));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretches_duration_and_keeps_pitch() {
        crate::init();
        let rate = 48_000;
        // 1 s de 440 Hz en estéreo.
        let input: Vec<f32> = (0..rate)
            .flat_map(|i| {
                let v = (i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 0.5;
                [v, v]
            })
            .collect();
        for speed in [2.0, 0.5, 0.25] {
            let mut t = Tempo::new(speed, rate, 2).unwrap();
            let mut out = VecDeque::new();
            for chunk in input.chunks(2048) {
                t.push(chunk).unwrap();
                t.pull(&mut out);
            }
            t.flush().unwrap();
            t.pull(&mut out);
            let secs = out.len() as f64 / 2.0 / rate as f64;
            assert!((secs * speed - 1.0).abs() < 0.03, "{speed}x: {secs} s");
            // El tono se conserva: contar cruces por cero en el canal izquierdo (≈ 2 × 440 por segundo).
            let left: Vec<f32> = out.iter().step_by(2).copied().collect();
            let mid = &left[left.len() / 4..left.len() * 3 / 4];
            let crossings = mid.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
            let hz = crossings as f64 / 2.0 / (mid.len() as f64 / rate as f64);
            assert!((hz - 440.0).abs() < 15.0, "{speed}x: {hz} Hz");
        }
    }
}
