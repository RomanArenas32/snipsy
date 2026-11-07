use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{bounded, select, unbounded, Receiver, Sender, TryRecvError};

use crate::{Image, Notify, VideoReader};

/// Un clip de video tal como lo ve el motor (tiempos en cuadros del proyecto).
#[derive(Clone, Debug)]
pub struct VideoSegment {
    pub clip: u64,
    pub path: Arc<PathBuf>,
    /// Índice de pista: el número mayor queda encima.
    pub layer: usize,
    pub start: i64,
    pub end: i64,
    /// Punto de entrada en el archivo, en segundos.
    pub source_in: f64,
    /// Velocidad de reproducción.
    pub speed: f64,
}

impl VideoSegment {
    /// Instante del archivo que se ve en el cuadro `frame` del timeline.
    pub fn source_time(&self, frame: i64, fps: f64) -> f64 {
        self.source_in + (frame - self.start) as f64 / fps * self.speed
    }
}

#[derive(Clone, Debug, Default)]
pub struct VideoTimeline {
    pub fps: f64,
    pub duration: i64,
    pub segments: Vec<VideoSegment>,
}

impl VideoTimeline {
    /// Clips visibles en un cuadro, de la pista más baja a la más alta.
    pub fn layers_at(&self, frame: i64) -> Vec<&VideoSegment> {
        let mut v: Vec<&VideoSegment> = self.segments.iter().filter(|s| s.start <= frame && frame < s.end).collect();
        v.sort_by_key(|s| s.layer);
        v
    }
}

/// Cuadro del timeline: la imagen decodificada de cada clip visible (de abajo
/// hacia arriba). La composición (posición, escala, opacidad) se hace después,
/// en la GPU, para que cambiar una transformación no requiera decodificar.
pub struct VideoFrame {
    pub serial: u64,
    pub frame: i64,
    /// Instante en segundos.
    pub pts: f64,
    /// (id del clip, imagen)
    pub layers: Vec<(u64, Image)>,
}

pub enum VideoEvent {
    Frame(VideoFrame),
    EndOfStream { serial: u64 },
    Error(String),
}

enum Command {
    Update { serial: u64, frame: i64, timeline: Option<Arc<VideoTimeline>> },
}

/// Cuántos cuadros por delante se prepara el siguiente clip.
const PREROLL_FRAMES: i64 = 12;
const MAX_READERS: usize = 12;

/// Motor de video: recorre el timeline cuadro a cuadro en un hilo propio.
pub struct VideoEngine {
    cmd_tx: Option<Sender<Command>>,
    events: Receiver<VideoEvent>,
    handle: Option<JoinHandle<()>>,
}

impl VideoEngine {
    pub fn new(max_size: (u32, u32), notify: Notify) -> Self {
        crate::init();
        let (cmd_tx, cmd_rx) = unbounded();
        let (ev_tx, events) = bounded(8);
        let handle = std::thread::Builder::new()
            .name("snipsy-video".into())
            .spawn(move || run(max_size, cmd_rx, ev_tx, notify))
            .expect("no se pudo crear el hilo de video");
        Self { cmd_tx: Some(cmd_tx), events, handle: Some(handle) }
    }

    /// Reemplaza el timeline (si se da) y se posiciona en `frame`.
    pub fn update(&self, serial: u64, frame: i64, timeline: Option<Arc<VideoTimeline>>) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Command::Update { serial, frame, timeline });
        }
    }

    pub fn events(&self) -> &Receiver<VideoEvent> {
        &self.events
    }
}

impl Drop for VideoEngine {
    fn drop(&mut self) {
        self.cmd_tx.take();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

struct State {
    serial: u64,
    frame: i64,
    timeline: Arc<VideoTimeline>,
    ended: bool,
}

impl State {
    fn apply(&mut self, cmd: Command, cmd_rx: &Receiver<Command>) {
        let mut cmd = cmd;
        loop {
            let Command::Update { serial, frame, timeline } = cmd;
            self.serial = serial;
            self.frame = frame.max(0);
            if let Some(t) = timeline {
                self.timeline = t;
            }
            self.ended = false;
            match cmd_rx.try_recv() {
                Ok(newer) => cmd = newer,
                Err(_) => break,
            }
        }
    }
}

fn run(max_size: (u32, u32), cmd_rx: Receiver<Command>, ev_tx: Sender<VideoEvent>, notify: Notify) {
    let mut state = State { serial: 0, frame: 0, timeline: Arc::new(VideoTimeline::default()), ended: true };
    let mut readers: Readers = HashMap::new();
    let mut failed: HashMap<u64, Arc<PathBuf>> = HashMap::new();

    loop {
        if state.ended {
            match cmd_rx.recv() {
                Ok(cmd) => state.apply(cmd, &cmd_rx),
                Err(_) => return,
            }
            continue;
        }
        match cmd_rx.try_recv() {
            Ok(cmd) => {
                state.apply(cmd, &cmd_rx);
                continue;
            }
            Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {}
        }

        let tl = state.timeline.clone();
        if state.frame >= tl.duration {
            state.ended = true;
            if ev_tx.send(VideoEvent::EndOfStream { serial: state.serial }).is_err() {
                return;
            }
            notify();
            continue;
        }

        let fps = tl.fps.max(1.0);
        let n = state.frame;
        let mut layers = Vec::new();
        for seg in tl.layers_at(n) {
            let t = seg.source_time(n, fps);
            let Some(r) = reader_for(&mut readers, &mut failed, seg, max_size, &ev_tx) else { continue };
            match r.frame_at(t) {
                Ok(Some(img)) => layers.push((seg.clip, img)),
                Ok(None) => {}
                Err(e) => log::warn!("error decodificando {}: {e}", seg.path.display()),
            }
        }

        let frame = VideoFrame { serial: state.serial, frame: n, pts: n as f64 / fps, layers };
        select! {
            send(ev_tx, VideoEvent::Frame(frame)) -> res => {
                if res.is_err() { return; }
                notify();
                state.frame += 1;
            }
            recv(cmd_rx) -> cmd => match cmd {
                Ok(cmd) => { state.apply(cmd, &cmd_rx); continue; }
                Err(_) => return,
            },
        }

        // Preparar el clip que viene (abrir y decodificar hasta su entrada) para que el corte no se trabe.
        let upcoming = tl
            .segments
            .iter()
            .filter(|s| s.start > state.frame && s.start <= state.frame + PREROLL_FRAMES && readers.get(&s.clip).is_none_or(|(p, _)| *p != s.path))
            .cloned()
            .collect::<Vec<_>>();
        for seg in upcoming {
            if let Some(r) = reader_for(&mut readers, &mut failed, &seg, max_size, &ev_tx) {
                let _ = r.frame_at(seg.source_in);
            }
        }

        // Cerrar lectores que ya no se necesitan.
        if readers.len() > MAX_READERS {
            let now = state.frame;
            readers.retain(|clip, _| {
                tl.segments.iter().any(|s| s.clip == *clip && s.end + PREROLL_FRAMES >= now && s.start <= now + PREROLL_FRAMES)
            });
        }
    }
}

type Readers = HashMap<u64, (Arc<PathBuf>, VideoReader)>;

fn reader_for<'a>(
    readers: &'a mut Readers,
    failed: &mut HashMap<u64, Arc<PathBuf>>,
    seg: &VideoSegment,
    max_size: (u32, u32),
    ev_tx: &Sender<VideoEvent>,
) -> Option<&'a mut VideoReader> {
    if failed.get(&seg.clip).is_some_and(|p| *p == seg.path) {
        return None;
    }
    // Si el clip ahora se lee de otro archivo (p. ej. terminó su proxy), reabrir.
    if readers.get(&seg.clip).is_some_and(|(p, _)| *p != seg.path) {
        readers.remove(&seg.clip);
    }
    if !readers.contains_key(&seg.clip) {
        match VideoReader::open(&seg.path, max_size) {
            Ok(r) => {
                readers.insert(seg.clip, (seg.path.clone(), r));
            }
            Err(e) => {
                failed.insert(seg.clip, seg.path.clone());
                let _ = ev_tx.try_send(VideoEvent::Error(format!("{}: {e}", seg.path.display())));
                return None;
            }
        }
    }
    readers.get_mut(&seg.clip).map(|(_, r)| r)
}
