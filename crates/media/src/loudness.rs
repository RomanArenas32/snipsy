//! Nivel del audio del timeline cuadro a cuadro (para detectar voz y hacer ducking).

use std::sync::Arc;

use snipsy_core::{Timeline, TrackKind, TrackRef};

use crate::{segments, Mixer};

/// Frecuencia de análisis: de sobra para medir nivel.
const RATE: u32 = 8_000;

/// Como [`levels_without`], con la pista a excluir opcional.
pub fn levels(tl: &Timeline, exclude: Option<TrackRef>) -> Vec<f32> {
    levels_without(tl, exclude.unwrap_or(TrackRef::audio(usize::MAX)))
}

/// Nivel RMS (dBFS) de cada cuadro del timeline, mezclando todas las pistas de audio
/// activas excepto `exclude` (la pista de música). Incluye volúmenes, fundidos y velocidad.
pub fn levels_without(tl: &Timeline, exclude: TrackRef) -> Vec<f32> {
    let mut tl = tl.clone();
    if exclude.kind == TrackKind::Audio && exclude.index < tl.audio_tracks.len() {
        tl.audio_tracks[exclude.index].muted = true;
    }
    let frames = tl.duration().max(0) as usize;
    let (_, audio) = segments::build(&tl);
    let mut mixer = Mixer::new(RATE, 1);
    mixer.set_timeline(Arc::new(audio));
    let mut out = Vec::with_capacity(frames);
    let mut pos = 0i64;
    let mut buf = Vec::new();
    for n in 0..frames {
        let end = (((n + 1) as f64 / tl.fps) * RATE as f64).round() as i64;
        buf.clear();
        buf.resize((end - pos).max(1) as usize, 0.0);
        mixer.render(pos, &mut buf);
        pos = end;
        let rms = (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt();
        out.push(20.0 * rms.max(1e-6).log10());
    }
    out
}
