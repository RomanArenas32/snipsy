//! Traduce el timeline del proyecto a lo que necesitan los motores de video y audio.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use snipsy_core::effects::db_to_gain;
use snipsy_core::{Prop, Timeline, TrackKind};

use crate::{AudioSegment, AudioTimeline, Envelope, VideoSegment, VideoTimeline};

/// Segmentos para exportar (siempre los archivos originales).
pub fn build(tl: &Timeline) -> (VideoTimeline, AudioTimeline) {
    build_with_proxies(tl, &HashMap::new())
}

/// Segmentos para la vista previa: el video de los medios con proxy (id → archivo)
/// se lee del proxy; el audio, siempre del original.
pub fn build_with_proxies(tl: &Timeline, proxies: &HashMap<u64, PathBuf>) -> (VideoTimeline, AudioTimeline) {
    let mut paths: HashMap<u64, Arc<PathBuf>> = HashMap::new();
    let mut video_paths: HashMap<u64, Arc<PathBuf>> = HashMap::new();
    for a in &tl.assets {
        let original = Arc::new(a.path.clone());
        let video = proxies.get(&a.id.0).map(|p| Arc::new(p.clone())).unwrap_or_else(|| original.clone());
        paths.insert(a.id.0, original);
        video_paths.insert(a.id.0, video);
    }
    let fps = tl.fps;
    let mut video = VideoTimeline { fps, duration: tl.duration(), segments: Vec::new() };
    for s in tl.active_spans(TrackKind::Video) {
        let c = s.clip;
        // Textos e imágenes fijas los dibuja la escena; no hay nada que decodificar.
        if tl.is_unbounded(c) {
            continue;
        }
        let Some(path) = video_paths.get(&c.asset.0) else { continue };
        video.segments.push(VideoSegment {
            clip: c.id.0,
            path: path.clone(),
            layer: s.track.index,
            start: s.start,
            end: s.end,
            source_in: s.source_in as f64 / fps,
            speed: c.speed,
        });
    }
    let mut audio = AudioTimeline { duration: tl.frame_to_secs(tl.duration()), segments: Vec::new() };
    for s in tl.active_spans(TrackKind::Audio) {
        let c = s.clip;
        let Some(path) = paths.get(&c.asset.0) else { continue };
        let constant = c.fade_in == 0 && c.fade_out == 0 && s.trans_in == 0 && s.trans_out == 0 && !c.anim.volume.is_animated();
        let envelope = (!constant).then(|| {
            let gains = (s.start..=s.end)
                .map(|f| {
                    let t = f as f64;
                    let fade = c.fade_at(t.clamp(c.start as f64, c.end() as f64), false);
                    db_to_gain(c.prop_at(Prop::Volume, f)) * fade * s.trans_in_at(t) * s.trans_out_at(t)
                })
                .collect();
            Envelope { start: s.start as f64 / fps, fps, gains: Arc::new(gains) }
        });
        audio.segments.push(AudioSegment {
            clip: c.id.0,
            path: path.clone(),
            start: s.start as f64 / fps,
            end: s.end as f64 / fps,
            source_in: s.source_in as f64 / fps,
            speed: c.speed,
            gain: db_to_gain(c.volume_db),
            envelope,
        });
    }
    (video, audio)
}
