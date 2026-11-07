use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender, TryRecvError};
use snipsy_audio::{AudioChunk, AudioQueue};

use crate::AudioReader;

/// Un clip de audio tal como lo ve el mezclador (tiempos en segundos).
#[derive(Clone, Debug)]
pub struct AudioSegment {
    pub clip: u64,
    pub path: Arc<PathBuf>,
    pub start: f64,
    pub end: f64,
    pub source_in: f64,
    /// Velocidad de reproducción (cuántos segundos de la fuente por segundo).
    pub speed: f64,
    /// Ganancia fija (se usa si no hay `envelope`).
    pub gain: f32,
    /// Ganancia variable en el tiempo (volumen animado, fundidos, fundidos cruzados).
    pub envelope: Option<Envelope>,
}

/// Ganancia muestreada una vez por cuadro e interpolada linealmente entre cuadros.
#[derive(Clone, Debug)]
pub struct Envelope {
    /// Instante (s) del primer valor.
    pub start: f64,
    pub fps: f64,
    pub gains: Arc<Vec<f32>>,
}

impl Envelope {
    pub fn at(&self, t: f64) -> f32 {
        let g = &self.gains;
        if g.is_empty() {
            return 1.0;
        }
        let x = ((t - self.start) * self.fps).max(0.0);
        let i = x.floor() as usize;
        if i + 1 >= g.len() {
            return g[g.len() - 1];
        }
        let f = (x - i as f64) as f32;
        g[i] + (g[i + 1] - g[i]) * f
    }
}

#[derive(Clone, Debug, Default)]
pub struct AudioTimeline {
    pub duration: f64,
    pub segments: Vec<AudioSegment>,
}

enum Command {
    Update { serial: u64, time: f64, timeline: Option<Arc<AudioTimeline>> },
}

/// Muestras (por canal) por bloque de mezcla.
const BLOCK: usize = 1024;
/// Cuánto audio mezclar por adelantado.
const MAX_BUFFERED_SECS: f64 = 0.5;
/// Con cuánta anticipación preparar el siguiente clip.
const PREROLL_SECS: f64 = 1.0;

/// Mezclador: suma todas las pistas de audio del timeline y llena la [`AudioQueue`].
pub struct AudioEngine {
    cmd_tx: Option<Sender<Command>>,
    handle: Option<JoinHandle<()>>,
}

impl AudioEngine {
    pub fn new(queue: Arc<AudioQueue>) -> Self {
        crate::init();
        let (cmd_tx, cmd_rx) = unbounded();
        let handle = std::thread::Builder::new()
            .name("snipsy-mixer".into())
            .spawn(move || run(&queue, cmd_rx))
            .expect("no se pudo crear el hilo de audio");
        Self { cmd_tx: Some(cmd_tx), handle: Some(handle) }
    }

    /// Reemplaza el timeline (si se da) y se posiciona en `time`.
    /// Quien llama debe hacer `queue.reset(serial, time)` antes.
    pub fn update(&self, serial: u64, time: f64, timeline: Option<Arc<AudioTimeline>>) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Command::Update { serial, time, timeline });
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.cmd_tx.take();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Mezclador síncrono: suma todas las pistas del timeline en bloques de muestras.
/// Lo usan la reproducción (en un hilo) y la exportación.
pub struct Mixer {
    rate: u32,
    channels: u16,
    timeline: Arc<AudioTimeline>,
    readers: HashMap<u64, AudioReader>,
    failed: HashSet<u64>,
}

impl Mixer {
    pub fn new(rate: u32, channels: u16) -> Self {
        Self { rate, channels, timeline: Arc::new(AudioTimeline::default()), readers: HashMap::new(), failed: HashSet::new() }
    }

    pub fn set_timeline(&mut self, timeline: Arc<AudioTimeline>) {
        let alive: HashSet<u64> = timeline.segments.iter().map(|s| s.clip).collect();
        self.readers.retain(|k, _| alive.contains(k));
        self.timeline = timeline;
    }

    pub fn duration(&self) -> f64 {
        self.timeline.duration
    }

    /// Mezcla en `buf` (intercalado, `buf.len() / channels` muestras) desde la muestra `pos`.
    pub fn render(&mut self, pos: i64, buf: &mut [f32]) {
        let rate = self.rate as f64;
        let ch = self.channels as usize;
        buf.fill(0.0);
        let block_end = pos + (buf.len() / ch) as i64;
        let timeline = self.timeline.clone();
        for seg in &timeline.segments {
            let s0 = ((seg.start * rate).round() as i64).max(pos);
            let s1 = ((seg.end * rate).round() as i64).min(block_end);
            if s0 >= s1 {
                continue;
            }
            let Some(reader) = self.reader_for(seg) else { continue };
            let src = seg.source_in + (s0 as f64 / rate - seg.start) * seg.speed;
            let out = &mut buf[(s0 - pos) as usize * ch..(s1 - pos) as usize * ch];
            let t0 = s0 as f64 / rate;
            let result = match &seg.envelope {
                Some(env) => reader.mix_into_with(src, out, |i| env.at(t0 + i as f64 / rate)),
                None => reader.mix_into(src, out, seg.gain),
            };
            if let Err(e) = result {
                log::warn!("error de audio en {}: {e}", seg.path.display());
            }
        }

        // Preparar clips que empiezan pronto; cerrar los que ya pasaron.
        let t = block_end as f64 / rate;
        for seg in &timeline.segments {
            if seg.start > t && seg.start <= t + PREROLL_SECS && !self.readers.contains_key(&seg.clip) {
                let n = BLOCK * ch;
                if let Some(r) = self.reader_for(seg) {
                    let _ = r.prepare(seg.source_in, n);
                }
            }
        }
        self.readers.retain(|clip, _| {
            timeline.segments.iter().any(|s| s.clip == *clip && s.end > t - 0.5 && s.start <= t + PREROLL_SECS + 0.5)
        });
    }

    fn reader_for(&mut self, seg: &AudioSegment) -> Option<&mut AudioReader> {
        if self.failed.contains(&seg.clip) {
            return None;
        }
        // Si cambió la velocidad del clip, el lector hay que rehacerlo.
        if self.readers.get(&seg.clip).is_some_and(|r| r.speed() != seg.speed) {
            self.readers.remove(&seg.clip);
        }
        if !self.readers.contains_key(&seg.clip) {
            let opened = AudioReader::open(&seg.path, self.rate, self.channels).and_then(|mut r| {
                if seg.speed != 1.0 {
                    r.set_speed(seg.speed)?;
                }
                Ok(r)
            });
            match opened {
                Ok(r) => {
                    self.readers.insert(seg.clip, r);
                }
                Err(e) => {
                    log::warn!("no se pudo abrir audio {}: {e}", seg.path.display());
                    self.failed.insert(seg.clip);
                    return None;
                }
            }
        }
        self.readers.get_mut(&seg.clip)
    }
}

fn run(queue: &AudioQueue, cmd_rx: Receiver<Command>) {
    let rate = queue.sample_rate as f64;
    let ch = queue.channels as usize;
    let mut serial = 0u64;
    let mut pos: i64 = 0; // en muestras por canal
    let mut mixer = Mixer::new(queue.sample_rate, queue.channels);
    let mut ended = true;

    loop {
        let cmd = if ended {
            match cmd_rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            }
        } else if queue.buffered_secs() > MAX_BUFFERED_SECS {
            match cmd_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match cmd_rx.try_recv() {
                Ok(c) => Some(c),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        };
        if let Some(mut cmd) = cmd {
            while let Ok(newer) = cmd_rx.try_recv() {
                cmd = newer;
            }
            let Command::Update { serial: s, time, timeline: t } = cmd;
            serial = s;
            pos = (time.max(0.0) * rate).round() as i64;
            if let Some(t) = t {
                mixer.set_timeline(t);
            }
            ended = false;
            continue;
        }

        let t0 = pos as f64 / rate;
        if t0 >= mixer.duration() {
            ended = true;
            continue;
        }
        let mut buf = vec![0.0f32; BLOCK * ch];
        mixer.render(pos, &mut buf);
        queue.push(serial, AudioChunk { pts: t0, samples: buf });
        pos += BLOCK as i64;
    }
}
