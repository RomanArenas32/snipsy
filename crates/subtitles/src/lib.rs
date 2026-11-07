//! Subtítulos automáticos: transcribe el audio del timeline con Whisper y crea
//! clips de texto sincronizados en una pista «Subtítulos».

mod captions;
pub mod models;
mod transcribe;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{bail, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use snipsy_core::{Ease, Frame, TextStyle, Timeline, TrackKind, WordAnimation, WordMode};
use snipsy_media::{segments, Mixer};

pub use captions::{group, to_srt, Caption, CaptionStyle, Word};
pub use models::Model;
pub use transcribe::{transcribe, SAMPLE_RATE};

/// Nombre de la pista donde se colocan los subtítulos.
pub const TRACK_NAME: &str = "Subtítulos";

/// Idiomas ofrecidos (código ISO, nombre). `None` = detectar.
pub const LANGUAGES: &[(Option<&str>, &str)] = &[
    (Some("es"), "Español"),
    (Some("en"), "Inglés"),
    (Some("pt"), "Portugués"),
    (Some("fr"), "Francés"),
    (Some("it"), "Italiano"),
    (Some("de"), "Alemán"),
    (None, "Detectar automáticamente"),
];

#[derive(Clone, Debug)]
pub struct SubtitleSettings {
    pub model: Model,
    pub language: Option<String>,
    pub style: CaptionStyle,
}

#[derive(Debug)]
pub enum SubtitleEvent {
    Downloading { done: u64, total: Option<u64> },
    PreparingAudio,
    Transcribing { percent: i32 },
    Done(Vec<Caption>),
    Cancelled,
    Error(String),
}

/// Trabajo en segundo plano: descarga del modelo, mezcla del audio y transcripción.
pub struct SubtitleJob {
    events: Receiver<SubtitleEvent>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SubtitleJob {
    pub fn start(tl: &Timeline, settings: SubtitleSettings) -> Self {
        let tl = tl.clone();
        let (tx, events) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let handle = std::thread::Builder::new()
            .name("snipsy-subtitles".into())
            .spawn(move || {
                let ev = match run(&tl, &settings, &tx, &flag) {
                    Ok(c) => SubtitleEvent::Done(c),
                    Err(_) if flag.load(Ordering::Relaxed) => SubtitleEvent::Cancelled,
                    Err(e) => SubtitleEvent::Error(format!("{e:#}")),
                };
                let _ = tx.send(ev);
            })
            .expect("no se pudo crear el hilo de subtítulos");
        Self { events, cancel, handle: Some(handle) }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn events(&self) -> &Receiver<SubtitleEvent> {
        &self.events
    }
}

impl Drop for SubtitleJob {
    fn drop(&mut self) {
        self.cancel();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Todo el audio del timeline (pistas no silenciadas), mono a 16 kHz.
pub fn timeline_audio(tl: &Timeline) -> Vec<f32> {
    let (_, audio) = segments::build(tl);
    let duration = audio.duration;
    let mut mixer = Mixer::new(SAMPLE_RATE, 1);
    mixer.set_timeline(Arc::new(audio));
    let total = (duration * SAMPLE_RATE as f64).ceil() as i64;
    let mut out = vec![0.0f32; total.max(0) as usize];
    for (i, chunk) in out.chunks_mut(SAMPLE_RATE as usize).enumerate() {
        mixer.render(i as i64 * SAMPLE_RATE as i64, chunk);
    }
    out
}

pub fn run(tl: &Timeline, s: &SubtitleSettings, tx: &Sender<SubtitleEvent>, cancel: &Arc<AtomicBool>) -> Result<Vec<Caption>> {
    if !tl.audio_tracks.iter().any(|t| !t.muted && !t.clips.is_empty()) {
        bail!("el timeline no tiene audio para transcribir");
    }
    let model = models::ensure(s.model, cancel, |done, total| {
        let _ = tx.send(SubtitleEvent::Downloading { done, total });
    })?;
    let _ = tx.send(SubtitleEvent::PreparingAudio);
    let audio = timeline_audio(tl);
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelado");
    }
    let _ = tx.send(SubtitleEvent::Transcribing { percent: 0 });
    let ptx = tx.clone();
    let words = transcribe(&model, s.model, &audio, s.language.as_deref(), cancel.clone(), move |p| {
        let _ = ptx.send(SubtitleEvent::Transcribing { percent: p });
    })?;
    log::info!("{} palabras reconocidas", words.len());
    Ok(group(&words, s.style))
}

/// Estilo de texto de los subtítulos según el formato del proyecto.
pub fn text_style(tl: &Timeline, style: CaptionStyle) -> (TextStyle, f32) {
    let short = tl.width.min(tl.height) as f32;
    match style {
        CaptionStyle::Reel => {
            let size = (short * 0.085).round();
            (TextStyle { text: String::new(), size, bold: true, color: [255, 255, 255, 255], outline: Some([0, 0, 0, 255]), background: None, words: None, ..Default::default() }, tl.height as f32 * 0.16)
        }
        CaptionStyle::Classic => {
            let size = (short * 0.05).round();
            let y = tl.height as f32 / 2.0 - size * 2.2;
            (TextStyle { text: String::new(), size, bold: true, color: [255, 255, 255, 255], outline: None, background: Some([0, 0, 0, 170]), words: None, ..Default::default() }, y)
        }
    }
}

/// Índice de la pista de subtítulos, si existe.
pub fn subtitle_track(tl: &Timeline) -> Option<usize> {
    tl.video_tracks.iter().position(|t| t.name == TRACK_NAME)
}

/// Aspecto de los subtítulos generados.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptionLook {
    pub style: CaptionStyle,
    /// Animación palabra a palabra y color de la palabra actual.
    pub words: Option<WordMode>,
    pub highlight: [u8; 4],
    /// Cada subtítulo entra con un pequeño rebote.
    pub bounce: bool,
}

impl Default for CaptionLook {
    fn default() -> Self {
        Self { style: CaptionStyle::Reel, words: Some(WordMode::Highlight), highlight: [255, 214, 10, 255], bounce: true }
    }
}

/// Coloca los subtítulos como clips de texto en la pista «Subtítulos» (que se
/// crea arriba de todo si no existe; si existe, se reemplaza su contenido).
pub fn apply(tl: &mut Timeline, captions: &[Caption], look: &CaptionLook) -> usize {
    let style = look.style;
    let index = match subtitle_track(tl) {
        Some(i) => {
            tl.video_tracks[i].clips.clear();
            i
        }
        None => {
            let i = tl.add_track(TrackKind::Video);
            tl.video_tracks[i].name = TRACK_NAME.into();
            i
        }
    };
    let (base, y) = text_style(tl, style);
    let mut n = 0;
    for c in captions {
        let start: Frame = (c.start * tl.fps).round() as Frame;
        let end: Frame = ((c.end * tl.fps).round() as Frame).max(start + 1);
        let len = end - start;
        let words = look.words.map(|mode| WordAnimation {
            mode,
            color: look.highlight,
            // El clip empieza en el cuadro 0 de su «fuente»: los inicios son relativos al clip.
            starts: c.words.iter().map(|w| ((w * tl.fps).round() as Frame - start).clamp(0, len - 1)).collect(),
        });
        let fps = tl.fps;
        let id = tl.insert_text(start, index, len, TextStyle { text: c.text.clone(), words, ..base.clone() });
        if let Some(clip) = tl.clip_mut(id) {
            clip.transform.y = y;
            if look.bounce && len >= 4 {
                let f = |secs: f64| ((secs * fps).round() as Frame).clamp(1, len - 1);
                let (a, b) = (f(0.08), f(0.16).max(f(0.08) + 1).min(len - 1));
                let k = clip.source_in;
                clip.anim.scale.set(k, 0.8);
                clip.anim.scale.set(k + a, 1.06);
                if b > a {
                    clip.anim.scale.set(k + b, 1.0);
                }
                clip.anim.scale.keys.iter_mut().for_each(|key| key.ease = Ease::Smooth);
            }
        }
        n += 1;
    }
    n
}

/// Subtítulos actuales (los textos de la pista «Subtítulos»), para exportar a SRT.
pub fn captions_from_timeline(tl: &Timeline) -> Vec<Caption> {
    let Some(i) = subtitle_track(tl) else { return Vec::new() };
    tl.video_tracks[i]
        .clips
        .iter()
        .filter_map(|c| {
            let text = c.text.as_ref()?.text.clone();
            Some(Caption { start: tl.frame_to_secs(c.start), end: tl.frame_to_secs(c.end()), text, words: Vec::new() })
        })
        .collect()
}
