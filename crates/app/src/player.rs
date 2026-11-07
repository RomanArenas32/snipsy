//! Reproductor del timeline: coordina el motor de video, el mezclador de audio y el reloj.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use snipsy_audio::AudioOutput;
use snipsy_core::timecode::frame_duration;
use snipsy_core::Timeline;
use snipsy_media::{AudioEngine, AudioTimeline, Notify, VideoEngine, VideoEvent, VideoFrame, VideoTimeline};

/// Resolución máxima de la vista previa (los archivos 4K se escalan).
const PREVIEW_MAX: (u32, u32) = (1920, 1080);

pub struct Player {
    video: VideoEngine,
    audio: Option<(AudioEngine, Arc<AudioOutput>)>,
    fps: f64,
    duration: f64,
    serial: u64,
    playing: bool,
    /// Posición actual (segundos).
    position: f64,
    /// Reloj de sistema (cuando no hay salida de audio).
    clock_base: f64,
    clock_start: Option<Instant>,
    /// El audio no está alineado con `position` (tras un paso de cuadro).
    audio_dirty: bool,
    /// Cuadro recibido que aún no toca mostrar.
    pending: Option<VideoFrame>,
    /// Hay que mostrar el primer cuadro tras un seek aunque estemos en pausa.
    need_frame: bool,
    video_ended: bool,
    pub error: Option<String>,
}

impl Player {
    pub fn new(output: Option<Arc<AudioOutput>>, notify: Notify) -> Self {
        let audio = output.map(|out| (AudioEngine::new(out.queue.clone()), out));
        Self {
            video: VideoEngine::new(PREVIEW_MAX, notify),
            audio,
            fps: 30.0,
            duration: 0.0,
            serial: 0,
            playing: false,
            position: 0.0,
            clock_base: 0.0,
            clock_start: None,
            audio_dirty: false,
            pending: None,
            need_frame: true,
            video_ended: false,
            error: None,
        }
    }

    /// Carga una versión nueva del timeline manteniendo la posición. `proxies`: archivo
    /// liviano a usar para el video de cada medio (id → proxy).
    pub fn set_timeline(&mut self, tl: &Timeline, proxies: &HashMap<u64, PathBuf>) {
        self.fps = tl.fps;
        self.duration = tl.frame_to_secs(tl.duration());
        let (video, audio) = snipsy_media::segments::build_with_proxies(tl, proxies);
        let pos = self.position.min(self.duration);
        self.restart(pos, Some((Arc::new(video), Arc::new(audio))));
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn play(&mut self) {
        if self.playing || self.duration <= 0.0 {
            return;
        }
        if self.position >= self.duration - frame_duration(self.fps) * 0.5 {
            self.seek(0.0);
        }
        if self.audio_dirty {
            self.sync_audio(None);
        }
        self.playing = true;
        self.clock_base = self.position;
        self.clock_start = Some(Instant::now());
        if let Some((_, out)) = &self.audio {
            out.queue.set_playing(true);
        }
    }

    pub fn pause(&mut self) {
        if !self.playing {
            return;
        }
        self.update_clock();
        self.playing = false;
        self.clock_start = None;
        if let Some((_, out)) = &self.audio {
            out.queue.set_playing(false);
        }
    }

    pub fn toggle(&mut self) {
        if self.playing {
            self.pause()
        } else {
            self.play()
        }
    }

    pub fn seek(&mut self, target: f64) {
        self.restart(target, None);
    }

    fn restart(&mut self, target: f64, timeline: Option<(Arc<VideoTimeline>, Arc<AudioTimeline>)>) {
        // Alinear al cuadro para que el reloj y el video coincidan exactamente.
        let frame = (target.clamp(0.0, self.duration.max(0.0)) * self.fps + 1e-6).floor() as i64;
        let target = frame as f64 / self.fps;
        self.serial += 1;
        self.position = target;
        self.clock_base = target;
        if self.playing {
            self.clock_start = Some(Instant::now());
        }
        self.pending = None;
        self.need_frame = true;
        self.video_ended = false;
        let (video_tl, audio_tl) = match timeline {
            Some((v, a)) => (Some(v), Some(a)),
            None => (None, None),
        };
        self.video.update(self.serial, frame, video_tl);
        self.sync_audio(audio_tl);
    }

    fn sync_audio(&mut self, timeline: Option<Arc<AudioTimeline>>) {
        if let Some((engine, out)) = &self.audio {
            out.queue.reset(self.serial, self.position);
            engine.update(self.serial, self.position, timeline);
        }
        self.audio_dirty = false;
    }

    /// Avanza un cuadro (en pausa).
    pub fn step_forward(&mut self) {
        self.pause();
        let next = match self.pending.take() {
            Some(f) => Some(f),
            None => self.recv_frame(Duration::from_millis(250)),
        };
        if let Some(frame) = next {
            self.position = frame.pts;
            self.pending = Some(frame);
            self.need_frame = true;
            self.audio_dirty = true;
        }
    }

    /// Retrocede un cuadro (en pausa) mediante un seek preciso.
    pub fn step_backward(&mut self) {
        self.pause();
        let target = (self.position - frame_duration(self.fps)).max(0.0);
        self.seek(target);
    }

    fn recv_frame(&mut self, timeout: Duration) -> Option<VideoFrame> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.video.events().recv_timeout(remaining) {
                Ok(VideoEvent::Frame(f)) if f.serial == self.serial => return Some(f),
                Ok(VideoEvent::Frame(_)) => continue,
                Ok(VideoEvent::EndOfStream { serial }) if serial == self.serial => {
                    self.video_ended = true;
                    return None;
                }
                Ok(VideoEvent::EndOfStream { .. }) => continue,
                Ok(VideoEvent::Error(e)) => {
                    self.error = Some(e);
                    continue;
                }
                Err(_) => return None,
            }
        }
    }

    fn update_clock(&mut self) {
        if !self.playing {
            return;
        }
        self.position = match &self.audio {
            Some((_, out)) => out.queue.clock(),
            None => {
                let elapsed = self.clock_start.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
                self.clock_base + elapsed
            }
        };
    }

    /// Llamar en cada repintado. Devuelve el cuadro a mostrar si cambió.
    pub fn tick(&mut self) -> Option<VideoFrame> {
        self.update_clock();
        let mut shown: Option<VideoFrame> = None;
        let tolerance = frame_duration(self.fps) * 0.5;

        loop {
            if self.pending.is_none() {
                match self.video.events().try_recv() {
                    Ok(VideoEvent::Frame(f)) => {
                        if f.serial == self.serial {
                            self.pending = Some(f);
                        }
                        continue;
                    }
                    Ok(VideoEvent::EndOfStream { serial }) => {
                        if serial == self.serial {
                            self.video_ended = true;
                        }
                        continue;
                    }
                    Ok(VideoEvent::Error(e)) => {
                        self.error = Some(e);
                        continue;
                    }
                    Err(_) => break,
                }
            }
            let pts = self.pending.as_ref().unwrap().pts;
            if self.need_frame || (self.playing && pts <= self.position + tolerance) {
                self.need_frame = false;
                shown = self.pending.take();
                // En pausa basta con un cuadro; reproduciendo, se descartan los atrasados.
                if !self.playing {
                    break;
                }
            } else {
                break;
            }
        }

        // Fin del timeline: detener la reproducción.
        if self.playing && self.video_ended && self.pending.is_none() {
            self.pause();
            self.position = self.position.min(self.duration);
        }
        shown
    }
}
