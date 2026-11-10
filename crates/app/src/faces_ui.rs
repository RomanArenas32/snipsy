//! Análisis de rostros en segundo plano para «Seguir rostro».

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crossbeam_channel::{unbounded, Receiver, Sender};
use snipsy_core::follow::FaceTrack;
use snipsy_core::Timeline;
use snipsy_media::Notify;

#[derive(Clone, Debug)]
enum State {
    Working(f32),
    Done(FaceTrack),
    Failed(String),
}

struct Job {
    original: PathBuf,
    /// Archivo a leer (el proxy si existe: mismo contenido y tiempos, más rápido).
    read: PathBuf,
    duration: f64,
}

enum Event {
    Progress(PathBuf, f32),
    Done(PathBuf, FaceTrack),
    Failed(PathBuf, String),
}

pub struct Faces {
    tx: Sender<Job>,
    rx: Receiver<Event>,
    states: HashMap<PathBuf, State>,
    _cancel: Arc<AtomicBool>,
}

impl Faces {
    pub fn new(notify: Notify) -> Self {
        let (tx, jobs) = unbounded::<Job>();
        let (etx, rx) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::Builder::new()
            .name("snipsy-faces".into())
            .spawn(move || {
                for job in jobs {
                    let ptx = etx.clone();
                    let pnotify = notify.clone();
                    let original = job.original.clone();
                    let res = snipsy_faces::analyze(&job.read, job.duration, &flag, |f| {
                        let _ = ptx.send(Event::Progress(original.clone(), f));
                        pnotify();
                    });
                    let ev = match res {
                        // Sin rostros: trayectoria vacía (así no se vuelve a analizar).
                        Ok(track) => Event::Done(job.original, track.unwrap_or(FaceTrack { interval: 1.0, path: Vec::new() })),
                        Err(e) => Event::Failed(job.original, format!("{e:#}")),
                    };
                    if etx.send(ev).is_err() {
                        return;
                    }
                    notify();
                }
            })
            .expect("no se pudo crear el hilo de rostros");
        Self { tx, rx, states: HashMap::new(), _cancel: cancel }
    }

    /// Pide el análisis de los videos que lo necesitan (clips con «Seguir rostro» o la
    /// lista `extra`, sin trayectoria) y copia al timeline las trayectorias ya listas.
    /// Devuelve un mensaje para la barra de estado y si cambió el timeline.
    pub fn sync(&mut self, tl: &mut Timeline, extra: &[snipsy_core::AssetId], proxies: &HashMap<u64, PathBuf>) -> (Option<String>, bool) {
        let mut message = None;
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Event::Progress(p, f) => {
                    self.states.insert(p, State::Working(f));
                }
                Event::Done(p, track) => {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    message = Some(if track.path.is_empty() {
                        format!("No se encontraron rostros en {name}: se mantiene el recorte centrado.")
                    } else {
                        format!("🎯 Rostros analizados: {name}.")
                    });
                    self.states.insert(p, State::Done(track));
                }
                Event::Failed(p, e) => {
                    message = Some(format!("No se pudieron analizar los rostros: {e}"));
                    self.states.insert(p, State::Failed(e));
                }
            }
        }
        let wanted: Vec<snipsy_core::AssetId> = tl
            .video_tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.follow_face)
            .map(|c| c.asset)
            .chain(extra.iter().copied())
            .collect();
        let mut changed = false;
        for asset in tl.assets.iter_mut().filter(|a| a.has_video && !a.still && a.faces.is_none()) {
            match self.states.get(&asset.path) {
                Some(State::Done(track)) => {
                    asset.faces = Some(track.clone());
                    changed = true;
                }
                None if wanted.contains(&asset.id) => {
                    self.states.insert(asset.path.clone(), State::Working(0.0));
                    let read = proxies.get(&asset.id.0).cloned().unwrap_or_else(|| asset.path.clone());
                    let _ = self.tx.send(Job { original: asset.path.clone(), read, duration: asset.duration });
                }
                _ => {}
            }
        }
        (message, changed)
    }

    /// Progreso (0–1) del análisis de un archivo, si está en curso.
    pub fn progress(&self, path: &std::path::Path) -> Option<f32> {
        match self.states.get(path) {
            Some(State::Working(f)) => Some(*f),
            _ => None,
        }
    }

    /// Error del análisis de un archivo, si falló.
    pub fn failed(&self, path: &std::path::Path) -> Option<&str> {
        match self.states.get(path) {
            Some(State::Failed(e)) => Some(e),
            _ => None,
        }
    }
}
