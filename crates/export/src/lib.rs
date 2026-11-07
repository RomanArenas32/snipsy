//! Exportación a MP4: recorre el timeline cuadro a cuadro a resolución completa,
//! compone en la GPU (sin ventana), mezcla el audio y codifica.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{anyhow, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use snipsy_core::compose;
use snipsy_core::{ClipId, Frame, Timeline};
use snipsy_media::{segments, Mixer, Mp4Writer, VideoReader, WriterSettings};
use snipsy_render::{Compositor, RgbaImage, Scene};

pub use snipsy_media::VideoCodec;

const AUDIO_RATE: u32 = 48_000;
const AUDIO_CHANNELS: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    High,
    Medium,
    Light,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::High, Quality::Medium, Quality::Light];

    pub fn label(self) -> &'static str {
        match self {
            Quality::High => "Alta",
            Quality::Medium => "Media",
            Quality::Light => "Liviana",
        }
    }

    /// Bits por píxel y cuadro para H.264 (HEVC necesita ~40% menos).
    fn bits_per_pixel(self) -> f64 {
        match self {
            Quality::High => 0.20,
            Quality::Medium => 0.12,
            Quality::Light => 0.07,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExportSettings {
    pub path: PathBuf,
    pub codec: VideoCodec,
    pub quality: Quality,
    pub hardware: bool,
}

/// Bitrate de video recomendado para un proyecto.
pub fn video_bitrate(tl: &Timeline, codec: VideoCodec, quality: Quality) -> u64 {
    let factor = if codec == VideoCodec::Hevc { 0.6 } else { 1.0 };
    let bps = tl.width as f64 * tl.height as f64 * tl.fps * quality.bits_per_pixel() * factor;
    (bps as u64).clamp(1_000_000, 200_000_000)
}

/// Tamaño aproximado del archivo en bytes.
pub fn estimated_size(tl: &Timeline, codec: VideoCodec, quality: Quality) -> u64 {
    let secs = tl.frame_to_secs(tl.duration());
    ((video_bitrate(tl, codec, quality) + 192_000) as f64 * secs / 8.0) as u64
}

pub enum ExportEvent {
    Started { encoder: String },
    Progress { frame: Frame, total: Frame, eta_secs: f64 },
    Done { path: PathBuf, secs: f64 },
    Cancelled,
    Error(String),
}

/// Exportación en curso (en su propio hilo).
pub struct ExportJob {
    events: Receiver<ExportEvent>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ExportJob {
    /// Empieza a exportar una copia del timeline (se puede seguir editando).
    pub fn start(tl: Timeline, settings: ExportSettings) -> Self {
        let (tx, events) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let handle = std::thread::Builder::new()
            .name("snipsy-export".into())
            .spawn(move || {
                let started = Instant::now();
                let ev = match run(&tl, &settings, &tx, &flag) {
                    Ok(()) => ExportEvent::Done { path: settings.path.clone(), secs: started.elapsed().as_secs_f64() },
                    Err(e) => {
                        let _ = std::fs::remove_file(&settings.path);
                        if flag.load(Ordering::Relaxed) {
                            ExportEvent::Cancelled
                        } else {
                            ExportEvent::Error(format!("{e:#}"))
                        }
                    }
                };
                let _ = tx.send(ev);
            })
            .expect("no se pudo crear el hilo de exportación");
        Self { events, cancel, handle: Some(handle) }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn events(&self) -> &Receiver<ExportEvent> {
        &self.events
    }
}

impl Drop for ExportJob {
    fn drop(&mut self) {
        self.cancel();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Exporta de forma síncrona (bloquea hasta terminar).
pub fn export_blocking(tl: &Timeline, settings: &ExportSettings) -> Result<()> {
    let (tx, _rx) = unbounded();
    run(tl, settings, &tx, &AtomicBool::new(false))
}

fn run(tl: &Timeline, s: &ExportSettings, tx: &Sender<ExportEvent>, cancel: &AtomicBool) -> Result<()> {
    let total = tl.duration();
    anyhow::ensure!(total > 0, "el timeline está vacío");
    let (w, h) = (tl.width & !1, tl.height & !1);
    let (device, queue) = snipsy_render::headless_device().ok_or_else(|| anyhow!("no hay GPU disponible"))?;
    let mut compositor = Compositor::new(&device, &queue);
    let mut scene = Scene::new();

    let (video_tl, audio_tl) = segments::build(tl);
    let has_audio = !audio_tl.segments.is_empty();
    let mut mixer = Mixer::new(AUDIO_RATE, AUDIO_CHANNELS);
    mixer.set_timeline(Arc::new(audio_tl));

    let mut writer = Mp4Writer::create(
        &s.path,
        &WriterSettings {
            width: w,
            height: h,
            fps: tl.fps,
            codec: s.codec,
            bitrate: video_bitrate(tl, s.codec, s.quality),
            hardware: s.hardware,
            // Un keyframe cada ~2 s: buen equilibrio entre tamaño y facilidad para saltar.
            keyframe_secs: 2.0,
            // Siempre con pista de audio (en silencio si no hay): las redes lo esperan.
            audio: Some((AUDIO_RATE, AUDIO_CHANNELS)),
            audio_bitrate: 192_000,
        },
    )?;
    let _ = tx.send(ExportEvent::Started { encoder: writer.encoder_name().to_owned() });
    log::info!("exportando {}x{} @ {:.3} con {} (audio: {has_audio})", w, h, tl.fps, writer.encoder_name());

    let mut readers: HashMap<u64, VideoReader> = HashMap::new();
    let mut audio_pos: i64 = 0;
    let mut audio_buf = Vec::new();
    let started = Instant::now();

    for n in 0..total {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("cancelado"));
        }

        // Decodificar cada clip visible al tamaño con el que aparece en pantalla.
        let mut images = HashMap::new();
        for seg in video_tl.layers_at(n) {
            let max = on_canvas_size(tl, ClipId(seg.clip));
            if !readers.contains_key(&seg.clip) {
                let mut r = VideoReader::open(&seg.path, max)?;
                r.set_high_quality(true);
                readers.insert(seg.clip, r);
            }
            let r = readers.get_mut(&seg.clip).unwrap();
            let t = seg.source_time(n, tl.fps);
            if let Some(img) = r.frame_at(t)? {
                images.insert(seg.clip, RgbaImage { width: img.width, height: img.height, rgba: img.rgba });
            }
        }
        readers.retain(|clip, _| video_tl.segments.iter().any(|s| s.clip == *clip && s.end > n));

        // Componer a resolución completa y leer el resultado.
        let layers = scene.layers(tl, n, &images, 1.0);
        let gpu = Scene::gpu_layers(&layers, tl, 1.0);
        compositor.render((w, h), &gpu);
        let rgba = compositor.read_canvas().ok_or_else(|| anyhow!("no se pudo leer el cuadro de la GPU"))?;
        writer.write_video(&rgba)?;

        // Audio hasta el final de este cuadro.
        let target = (((n + 1) as f64 / tl.fps) * AUDIO_RATE as f64).round() as i64;
        while audio_pos < target {
            let block = (target - audio_pos).min(1024) as usize;
            audio_buf.resize(block * AUDIO_CHANNELS as usize, 0.0);
            mixer.render(audio_pos, &mut audio_buf);
            writer.write_audio(&audio_buf)?;
            audio_pos += block as i64;
        }

        if n % 5 == 0 || n + 1 == total {
            let done = (n + 1) as f64;
            let elapsed = started.elapsed().as_secs_f64();
            let eta = elapsed / done * (total as f64 - done);
            let _ = tx.send(ExportEvent::Progress { frame: n + 1, total, eta_secs: eta });
        }
    }
    writer.finish()?;
    Ok(())
}

/// Tamaño (px) con que un clip de video aparece en el lienzo: no tiene sentido decodificar más.
fn on_canvas_size(tl: &Timeline, clip: ClipId) -> (u32, u32) {
    let project = (tl.width as f32, tl.height as f32);
    let Some((_, c)) = tl.find(clip) else { return (tl.width, tl.height) };
    let src = tl.asset(c.asset).map(|a| (a.width as f32, a.height as f32)).unwrap_or(project);
    let (bw, bh) = compose::contain(src, project);
    let s = c.max_scale().max(0.01);
    (((bw * s).ceil() as u32 + 2).max(16), ((bh * s).ceil() as u32 + 2).max(16))
}

/// Nombre de archivo sugerido a partir del proyecto.
pub fn suggested_name(project: Option<&Path>) -> String {
    let stem = project.and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Snipsy".into());
    format!("{stem}.mp4")
}
