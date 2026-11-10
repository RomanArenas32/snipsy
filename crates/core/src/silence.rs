//! Cortar silencios: quitar las pausas de un video hablado (reels, entrevistas).
//!
//! A partir del nivel del audio (dB por cuadro) se buscan los tramos sin voz y se
//! eliminan del timeline cerrando los huecos. Las pistas «continuas» (música, logo)
//! no se cortan: solo se acortan al final para acompañar al video.

use std::collections::HashSet;

use crate::ducking::{voice_regions, DuckSettings};
use crate::{ClipId, Frame, Timeline, TrackKind, TrackRef};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SilenceSettings {
    /// Por debajo de este nivel (dBFS) se considera silencio.
    pub threshold_db: f32,
    /// Solo se cortan pausas de al menos estos segundos.
    pub min_silence: f64,
    /// Segundos de aire que se dejan antes y después de cada frase.
    pub padding: f64,
}

impl Default for SilenceSettings {
    fn default() -> Self {
        Self { threshold_db: -40.0, min_silence: 0.6, padding: 0.15 }
    }
}

/// Cuadros del timeline con algún clip en las pistas de audio analizadas (activas y
/// distintas de `exclude`). Fuera de ellas no hay «silencio» que cortar: puede ser
/// un plano sin sonido a propósito.
pub fn audio_coverage(tl: &Timeline, exclude: Option<TrackRef>) -> Vec<bool> {
    let mut covered = vec![false; tl.duration().max(0) as usize];
    for (i, track) in tl.audio_tracks.iter().enumerate() {
        if track.muted || exclude == Some(TrackRef::audio(i)) {
            continue;
        }
        for c in &track.clips {
            let (a, b) = (c.start.max(0) as usize, (c.end().max(0) as usize).min(covered.len()));
            covered[a.min(b)..b].iter_mut().for_each(|f| *f = true);
        }
    }
    covered
}

/// Tramos `[inicio, fin)` a cortar. `levels_db` y `covered` tienen un valor por cuadro.
/// Si no hay voz en ninguna parte no se corta nada (sería borrar todo el video).
pub fn silent_ranges(levels_db: &[f32], covered: &[bool], fps: f64, s: &SilenceSettings) -> Vec<(Frame, Frame)> {
    let len = levels_db.len().min(covered.len()) as Frame;
    // Las pausas más cortas que `min_silence` quedan dentro de una misma frase.
    let duck = DuckSettings { threshold_db: s.threshold_db, hold: s.min_silence, ..DuckSettings::default() };
    let voice = voice_regions(&levels_db[..len as usize], fps, &duck);
    if voice.is_empty() {
        return Vec::new();
    }
    let pad = (s.padding * fps).round() as Frame;
    let mut keep: Vec<(Frame, Frame)> = Vec::new();
    for (a, b) in voice {
        let (a, b) = ((a - pad).max(0), (b + pad).min(len));
        match keep.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => keep.push((a, b)),
        }
    }
    // Huecos entre lo que se conserva (incluye el principio y el final).
    let mut gaps = Vec::new();
    let mut cursor = 0;
    for (a, b) in keep {
        if a > cursor {
            gaps.push((cursor, a));
        }
        cursor = b;
    }
    if cursor < len {
        gaps.push((cursor, len));
    }
    // Solo donde hay audio, y sin cortes diminutos.
    let min = ((s.min_silence - 2.0 * s.padding).max(0.1) * fps).round().max(1.0) as Frame;
    let mut out = Vec::new();
    for (a, b) in gaps {
        let mut run = None;
        for f in a..=b {
            let on = f < b && covered[f as usize];
            match (on, run) {
                (true, None) => run = Some(f),
                (false, Some(r)) => {
                    if f - r >= min {
                        out.push((r, f));
                    }
                    run = None;
                }
                _ => {}
            }
        }
    }
    out
}

/// Elimina los tramos `ranges` (ordenados, sin solaparse) de todas las pistas excepto
/// `keep`, cerrando los huecos. Las pistas de `keep` se acortan al final según lo
/// quitado. Devuelve los cuadros eliminados.
pub fn cut_ranges(tl: &mut Timeline, ranges: &[(Frame, Frame)], keep: &[TrackRef]) -> Frame {
    let refs: Vec<TrackRef> = (0..tl.video_tracks.len())
        .map(TrackRef::video)
        .chain((0..tl.audio_tracks.len()).map(TrackRef::audio))
        .collect();
    let cut: Vec<TrackRef> = refs.iter().copied().filter(|r| !keep.contains(r)).collect();
    let end_of = |tl: &Timeline, rs: &[TrackRef]| rs.iter().filter_map(|r| tl.track(*r).clips.last().map(|c| c.end())).max().unwrap_or(0);
    let old_end = end_of(tl, &cut);
    let keep_ends: Vec<Frame> = keep.iter().map(|r| end_of(tl, &[*r])).collect();

    // Cortar en cada borde (conservando los vínculos video/audio de cada pedazo).
    for at in ranges.iter().flat_map(|(a, b)| [*a, *b]) {
        let ids: Vec<ClipId> =
            cut.iter().flat_map(|r| tl.track(*r).clips.iter()).filter(|c| c.start < at && at < c.end()).map(|c| c.id).collect();
        if !ids.is_empty() {
            tl.split(at, Some(&ids));
        }
    }
    // Quitar lo que cae dentro y correr lo que viene después (de atrás hacia adelante).
    let inside = |c: &crate::Clip| ranges.iter().any(|(a, b)| c.start >= *a && c.end() <= *b);
    for r in &cut {
        tl.track_mut(*r).clips.retain(|c| !inside(c));
    }
    let mut removed = 0;
    for (a, b) in ranges.iter().rev() {
        for r in &cut {
            for c in tl.track_mut(*r).clips.iter_mut().filter(|c| c.start >= *b) {
                c.start -= b - a;
            }
        }
        removed += b - a;
    }

    // Pistas continuas: terminan donde termina ahora el video (más lo que sobresalían).
    let new_end = end_of(tl, &cut);
    for (r, old_track_end) in keep.iter().zip(keep_ends) {
        let limit = new_end + (old_track_end - old_end).max(0);
        let clips = &mut tl.track_mut(*r).clips;
        clips.retain(|c| c.start < limit);
        if let Some(c) = clips.last_mut() {
            if c.end() > limit {
                c.len = limit - c.start;
                c.fade_out = c.fade_out.min(c.len);
            }
        }
    }
    removed
}

/// Pistas que no conviene cortar: el logo de marca y la música indicada.
pub fn continuous_tracks(tl: &Timeline, music: Option<usize>) -> Vec<TrackRef> {
    let mut out: Vec<TrackRef> = tl
        .video_tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.name == crate::brand::LOGO_TRACK)
        .map(|(i, _)| TrackRef::video(i))
        .collect();
    out.extend(music.filter(|m| *m < tl.audio_tracks.len()).map(TrackRef::audio));
    out
}

/// Pista de música más probable: una pista de audio (que no sea la primera) con
/// ducking aplicado o con «música» en el nombre.
pub fn guess_music_track(tl: &Timeline) -> Option<usize> {
    let named = |n: &str| {
        let n = n.to_lowercase();
        n.contains("música") || n.contains("musica") || n.contains("music")
    };
    tl.audio_tracks
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, t)| named(&t.name) || t.clips.iter().any(|c| !c.anim.volume.keys.is_empty()))
        .map(|(i, _)| i)
}

/// Los clips de video que quedaron sin su audio vinculado (o al revés) no deberían
/// existir tras cortar: útil para pruebas.
#[doc(hidden)]
pub fn orphan_links(tl: &Timeline) -> usize {
    let mut seen: std::collections::HashMap<u64, HashSet<TrackKind>> = Default::default();
    for (r, c) in tl.clips() {
        if let Some(l) = c.link {
            seen.entry(l).or_default().insert(r.kind);
        }
    }
    seen.values().filter(|k| k.len() < 2).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Asset, AssetId};

    fn asset(tl: &mut Timeline, name: &str, secs: f64, video: bool) -> AssetId {
        tl.add_asset(Asset {
            id: AssetId(0),
            path: format!("/m/{name}.mp4").into(),
            name: name.into(),
            duration: secs,
            fps: 30.0,
            width: 1920,
            height: 1080,
            has_video: video,
            has_audio: true,
            still: false,
            faces: None,
        })
    }

    #[test]
    fn finds_pauses_with_padding_only_where_there_is_audio() {
        let s = SilenceSettings::default(); // pad 0,15 s = 5 cuadros; mínimo 0,6 s
        let mut levels = vec![-80.0f32; 300];
        // Silencio inicial, voz 30–90, pausa corta (9 cuadros), voz 99–150, pausa larga, voz 210–240.
        for i in (30..90).chain(99..150).chain(210..240) {
            levels[i] = -20.0;
        }
        let mut covered = vec![true; 300];
        assert_eq!(silent_ranges(&levels, &covered, 30.0, &s), vec![(0, 25), (155, 205), (245, 300)]);
        // Sin clips de audio al final: ese tramo no se toca.
        covered[260..].iter_mut().for_each(|c| *c = false);
        assert_eq!(silent_ranges(&levels, &covered, 30.0, &s), vec![(0, 25), (155, 205), (245, 260)]);
        // Sin voz: nada.
        assert!(silent_ranges(&[-80.0; 100], &[true; 100], 30.0, &s).is_empty());
    }

    #[test]
    fn cuts_ripple_keeping_links_music_and_logo() {
        let mut tl = Timeline::new();
        let a = asset(&mut tl, "charla", 10.0, true);
        let m = asset(&mut tl, "musica", 30.0, false);
        tl.append_asset(a); // V1 + A1, 0–300
        let music = tl.add_track(TrackKind::Audio);
        tl.audio_tracks[music].name = "Música".into();
        tl.insert_asset(m, 0, 0, music);
        tl.audio_tracks[music].clips[0].len = 300;
        assert_eq!(tl.video_tracks[0].clips.len(), 1);

        assert_eq!(guess_music_track(&tl), Some(music));
        let keep = continuous_tracks(&tl, guess_music_track(&tl));
        let removed = cut_ranges(&mut tl, &[(0, 25), (155, 205), (245, 300)], &keep);
        assert_eq!(removed, 25 + 50 + 55);

        let spans = |r: TrackRef| tl.track(r).clips.iter().map(|c| (c.start, c.len, c.source_in)).collect::<Vec<_>>();
        let expected = vec![(0, 130, 25), (130, 40, 205)];
        assert_eq!(spans(TrackRef::video(0)), expected);
        assert_eq!(spans(TrackRef::audio(0)), expected);
        assert_eq!(orphan_links(&tl), 0);
        // Cada pedazo de video sigue vinculado a su pedazo de audio.
        let v = &tl.video_tracks[0].clips;
        let au = &tl.audio_tracks[0].clips;
        assert!(v.iter().zip(au).all(|(v, a)| v.link.is_some() && v.link == a.link));
        assert_ne!(v[0].link, v[1].link);
        // La música sigue entera desde el principio y termina con el video.
        assert_eq!(spans(TrackRef::audio(music)), vec![(0, 170, 0)]);
        assert_eq!(tl.duration(), 170);
    }
}
