//! Proxies: copias livianas (720p, keyframes frecuentes) de los videos pesados
//! (4K, 1440p) para que la edición vaya fluida. La exportación usa siempre el original.
//!
//! Se guardan en `~/Library/Application Support/Snipsy/proxies`, con un nombre que
//! depende de la ruta, el tamaño y la fecha del original: si el archivo cambia, el
//! proxy viejo deja de usarse.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{Context as _, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};

use crate::{Mp4Writer, Notify, VideoCodec, VideoReader, WriterSettings};

/// Lado corto de los proxies.
pub const PROXY_SHORT_SIDE: u32 = 720;

/// ¿Conviene un proxy para un video de este tamaño? (más que Full HD)
pub fn needs_proxy(width: u32, height: u32) -> bool {
    width.min(height) > 1080
}

/// Tamaño del proxy: lado corto de 720 px, misma proporción, dimensiones pares.
pub fn proxy_size(width: u32, height: u32) -> (u32, u32) {
    let short = width.min(height).max(1);
    let k = (PROXY_SHORT_SIDE as f64 / short as f64).min(1.0);
    let even = |v: u32| ((v as f64 * k / 2.0).round() as u32).max(1) * 2;
    (even(width), even(height))
}

pub fn proxies_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Library/Application Support/Snipsy/proxies")
}

/// FNV-1a: estable entre versiones (el hash de la biblioteca estándar no lo es).
fn fnv(bytes: &[u8], mut h: u64) -> u64 {
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Dónde va (o iría) el proxy de `src` dentro de `dir`. `None` si el original no existe.
pub fn proxy_path_in(dir: &Path, src: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(src).ok()?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let mut h = fnv(src.as_os_str().as_encoded_bytes(), 0xcbf29ce484222325);
    h = fnv(&meta.len().to_le_bytes(), h);
    h = fnv(&mtime.to_le_bytes(), h);
    let stem: String = src
        .file_stem()
        .map(|s| s.to_string_lossy().chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').take(40).collect())
        .unwrap_or_default();
    Some(dir.join(format!("{stem}-{h:016x}.mp4")))
}

/// Proxy ya creado de `src`, si existe.
pub fn existing_proxy_in(dir: &Path, src: &Path) -> Option<PathBuf> {
    proxy_path_in(dir, src).filter(|p| p.exists())
}

/// Crea el proxy de `src` en `dest` (se escribe en un archivo temporal y se renombra
/// al terminar, así nunca se usa uno a medias). `progress` recibe 0–1.
pub fn make_proxy(
    src: &Path,
    dest: &Path,
    size: (u32, u32),
    fps: f64,
    duration: f64,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("no se pudo crear {}", dir.display()))?;
    }
    let part = dest.with_extension("part.mp4");
    let fps = if fps.is_finite() && fps > 0.0 { fps } else { 30.0 };
    let total = ((duration * fps).floor() as i64).max(1);
    let result = (|| -> Result<()> {
        let mut reader = VideoReader::open(src, size)?;
        reader.set_high_quality(true);
        let mut writer = Mp4Writer::create(
            &part,
            &WriterSettings {
                width: size.0,
                height: size.1,
                fps,
                codec: VideoCodec::H264,
                bitrate: (size.0 as u64 * size.1 as u64 * 6).max(1_000_000),
                hardware: true,
                // Keyframes frecuentes: saltar en el timeline es casi instantáneo.
                keyframe_secs: 0.5,
                audio: None,
                audio_bitrate: 0,
            },
        )?;
        let mut black = None;
        for n in 0..total {
            if cancel.load(Ordering::Relaxed) {
                anyhow::bail!("cancelado");
            }
            let img = reader.frame_at(n as f64 / fps)?;
            match img {
                // Por si el lector devuelve otro tamaño (redondeos), se ajusta copiando.
                Some(img) if (img.width as u32, img.height as u32) == size => writer.write_video(&img.rgba)?,
                Some(img) => writer.write_video(&fit(&img, size))?,
                None => writer.write_video(black.get_or_insert_with(|| vec![0u8; (size.0 * size.1 * 4) as usize]))?,
            }
            if n % 10 == 0 {
                progress(n as f32 / total as f32);
            }
        }
        writer.finish()?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            std::fs::rename(&part, dest)?;
            progress(1.0);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// Copia `img` en un lienzo de `size` (vecino más cercano; solo para diferencias de un píxel).
fn fit(img: &crate::Image, size: (u32, u32)) -> Vec<u8> {
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let sy = (y * img.height / h).min(img.height - 1);
        for x in 0..w {
            let sx = (x * img.width / w).min(img.width - 1);
            let s = (sy * img.width + sx) * 4;
            out[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&img.rgba[s..s + 4]);
        }
    }
    out
}

/// Pedido de proxy para un archivo.
#[derive(Clone, Debug)]
pub struct ProxyRequest {
    /// Identificador del medio en el proyecto (se devuelve en los eventos).
    pub asset: u64,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
}

#[derive(Debug)]
pub enum ProxyEvent {
    Progress { asset: u64, fraction: f32 },
    Done { asset: u64, proxy: PathBuf, secs: f64 },
    Failed { asset: u64, error: String },
    Cancelled { asset: u64 },
}

/// Cola de proxies: los crea de a uno en un hilo de fondo.
pub struct ProxyQueue {
    tx: Option<Sender<ProxyRequest>>,
    events: Receiver<ProxyEvent>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ProxyQueue {
    pub fn new(dir: PathBuf, notify: Notify) -> Self {
        crate::init();
        let (tx, rx) = unbounded::<ProxyRequest>();
        let (ev_tx, events) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let handle = std::thread::Builder::new()
            .name("snipsy-proxies".into())
            .spawn(move || {
                for req in rx {
                    flag.store(false, Ordering::Relaxed);
                    let asset = req.asset;
                    let Some(dest) = proxy_path_in(&dir, &req.path) else {
                        let _ = ev_tx.send(ProxyEvent::Failed { asset, error: "no se encuentra el archivo".into() });
                        notify();
                        continue;
                    };
                    if dest.exists() {
                        let _ = ev_tx.send(ProxyEvent::Done { asset, proxy: dest, secs: 0.0 });
                        notify();
                        continue;
                    }
                    let started = std::time::Instant::now();
                    let size = proxy_size(req.width, req.height);
                    let ptx = ev_tx.clone();
                    let pnotify = notify.clone();
                    let res = make_proxy(&req.path, &dest, size, req.fps, req.duration, &flag, |fraction| {
                        let _ = ptx.send(ProxyEvent::Progress { asset, fraction });
                        pnotify();
                    });
                    let ev = match res {
                        Ok(()) => {
                            let secs = started.elapsed().as_secs_f64();
                            log::info!("proxy de {} creado en {secs:.1} s", req.path.display());
                            ProxyEvent::Done { asset, proxy: dest, secs }
                        }
                        Err(_) if flag.load(Ordering::Relaxed) => ProxyEvent::Cancelled { asset },
                        Err(e) => ProxyEvent::Failed { asset, error: format!("{e:#}") },
                    };
                    let _ = ev_tx.send(ev);
                    notify();
                }
            })
            .expect("no se pudo crear el hilo de proxies");
        Self { tx: Some(tx), events, cancel, handle: Some(handle) }
    }

    pub fn request(&self, req: ProxyRequest) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(req);
        }
    }

    /// Cancela el proxy en curso (los pendientes siguen en la cola).
    pub fn cancel_current(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn events(&self) -> &Receiver<ProxyEvent> {
        &self.events
    }
}

impl Drop for ProxyQueue {
    fn drop(&mut self) {
        self.tx.take();
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_names() {
        assert!(needs_proxy(3840, 2160) && needs_proxy(2160, 3840) && needs_proxy(2560, 1440));
        assert!(!needs_proxy(1920, 1080) && !needs_proxy(1080, 1920));
        assert_eq!(proxy_size(3840, 2160), (1280, 720));
        assert_eq!(proxy_size(2160, 3840), (720, 1280));
        assert_eq!(proxy_size(4096, 2160), (1366, 720));
        assert_eq!(proxy_size(640, 360), (640, 360), "nunca se agranda");

        let dir = std::env::temp_dir().join(format!("snipsy_proxy_names_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("Mi video 4K.mov");
        std::fs::write(&src, b"a").unwrap();
        let p1 = proxy_path_in(&dir, &src).unwrap();
        assert!(p1.file_name().unwrap().to_string_lossy().starts_with("Mivideo4K-"));
        assert_eq!(proxy_path_in(&dir, &src), Some(p1.clone()), "estable");
        std::fs::write(&src, b"otro contenido").unwrap();
        assert_ne!(proxy_path_in(&dir, &src), Some(p1), "si el archivo cambia, el proxy también");
        assert!(proxy_path_in(&dir, &dir.join("no-existe.mov")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
