//! Ducking: bajar la música automáticamente mientras alguien habla.
//!
//! A partir del nivel (dB por cuadro) del resto del audio se detectan los tramos
//! con voz, y se escriben keyframes de volumen en los clips de la pista de música:
//! quedan a la vista en el timeline y se pueden retocar a mano.

use crate::effects::Ease;
use crate::{Frame, Timeline, TrackRef};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DuckSettings {
    /// Cuánto se baja la música (dB, positivo).
    pub amount_db: f32,
    /// Nivel a partir del cual se considera que hay voz (dBFS).
    pub threshold_db: f32,
    /// Segundos que tarda en bajar (empieza antes de la voz).
    pub attack: f64,
    /// Segundos que tarda en volver a subir después de la voz.
    pub release: f64,
    /// Silencios más cortos que esto no hacen subir la música entre frases.
    pub hold: f64,
}

impl Default for DuckSettings {
    fn default() -> Self {
        Self { amount_db: 12.0, threshold_db: -40.0, attack: 0.25, release: 0.6, hold: 0.5 }
    }
}

/// Voz más corta que esto se ignora (golpes, clics).
const MIN_VOICE_SECS: f64 = 0.15;

/// Tramos `[inicio, fin)` (cuadros del timeline) con voz, a partir del nivel de cada cuadro.
pub fn voice_regions(levels_db: &[f32], fps: f64, s: &DuckSettings) -> Vec<(Frame, Frame)> {
    let mut regions: Vec<(Frame, Frame)> = Vec::new();
    let mut start = None;
    for (i, l) in levels_db.iter().chain(std::iter::once(&f32::NEG_INFINITY)).enumerate() {
        let on = *l >= s.threshold_db;
        match (on, start) {
            (true, None) => start = Some(i as Frame),
            (false, Some(a)) => {
                regions.push((a, i as Frame));
                start = None;
            }
            _ => {}
        }
    }
    // Unir frases separadas por pausas cortas; descartar ruidos aislados.
    let hold = (s.hold * fps).round() as Frame;
    let mut merged: Vec<(Frame, Frame)> = Vec::new();
    for r in regions {
        match merged.last_mut() {
            Some(last) if r.0 - last.1 <= hold => last.1 = r.1,
            _ => merged.push(r),
        }
    }
    let min = (MIN_VOICE_SECS * fps).round() as Frame;
    merged.retain(|(a, b)| b - a >= min);
    merged
}

/// Ganancia (dB, relativa) en el cuadro `t`, según los tramos de voz.
fn duck_at(t: f64, regions: &[(Frame, Frame)], attack: f64, release: f64, amount: f32) -> f32 {
    let mut depth = 0.0f64;
    for (a, b) in regions {
        let (a, b) = (*a as f64, *b as f64);
        let d = if t >= a && t <= b {
            1.0
        } else if t < a && t > a - attack {
            1.0 - (a - t) / attack
        } else if t > b && t < b + release {
            1.0 - (t - b) / release
        } else {
            0.0
        };
        depth = depth.max(d);
    }
    -(amount as f64 * depth) as f32
}

/// Escribe el ducking en los clips de la pista `track` (reemplaza sus keyframes de
/// volumen). Devuelve cuántos clips quedaron con ducking.
pub fn apply_ducking(tl: &mut Timeline, track: TrackRef, regions: &[(Frame, Frame)], s: &DuckSettings) -> usize {
    let fps = tl.fps;
    // En cuadros enteros, para que las esquinas de la curva caigan justo en un keyframe.
    let attack = (s.attack * fps).round().max(1.0);
    let release = (s.release * fps).round().max(1.0);
    // Puntos donde cambia la pendiente de la curva.
    let mut corners: Vec<f64> = Vec::new();
    for (a, b) in regions {
        corners.extend([*a as f64 - attack, *a as f64, *b as f64, *b as f64 + release]);
    }
    let mut n = 0;
    for clip in &mut tl.track_mut(track).clips {
        clip.anim.volume.keys.clear();
        let (start, end) = (clip.start as f64, (clip.end() - 1) as f64);
        let touches = regions.iter().any(|(a, b)| (*a as f64 - attack) < end && (*b as f64 + release) > start);
        if !touches {
            continue;
        }
        let mut times: Vec<f64> = corners.iter().copied().filter(|t| *t > start && *t < end).collect();
        times.extend([start, end]);
        times.sort_by(f64::total_cmp);
        let base = clip.volume_db;
        for t in times {
            let frame = t.round() as Frame;
            let value = base + duck_at(frame as f64, regions, attack, release, s.amount_db);
            let at = clip.source_frame(frame);
            clip.anim.volume.set(at, value);
        }
        clip.anim.volume.keys.iter_mut().for_each(|k| k.ease = Ease::Linear);
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Asset, AssetId, Prop};

    #[test]
    fn detects_voice_and_bridges_short_pauses() {
        let s = DuckSettings::default();
        let mut levels = vec![-80.0f32; 300];
        // Voz de 30 a 90, pausa de 6 cuadros, voz de 96 a 150; un clic en 200; voz de 240 a 280.
        for i in (30..90).chain(96..150).chain(200..202).chain(240..280) {
            levels[i] = -20.0;
        }
        assert_eq!(voice_regions(&levels, 30.0, &s), vec![(30, 150), (240, 280)]);
    }

    #[test]
    fn writes_volume_keys_on_music() {
        let mut tl = Timeline::new();
        let music = tl.add_asset(Asset {
            id: AssetId(0),
            path: "/m/musica.mp3".into(),
            name: "musica".into(),
            duration: 20.0,
            fps: 30.0,
            width: 0,
            height: 0,
            has_video: false,
            has_audio: true,
            still: false,
            faces: None,
        });
        tl.insert_asset(music, 0, 0, 1);
        let track = TrackRef::audio(1);
        let id = tl.track(track).clips[0].id;
        tl.clip_mut(id).unwrap().volume_db = -6.0;
        let s = DuckSettings::default();
        assert_eq!(apply_ducking(&mut tl, track, &[(60, 150)], &s), 1);
        let c = &tl.track(track).clips[0];
        let v = |f| c.prop_at(Prop::Volume, f);
        assert!((v(0) + 6.0).abs() < 1e-4, "sin voz: volumen original");
        assert!((v(52) + 6.0).abs() < 1e-3, "antes del ataque");
        assert!((v(60) + 18.0).abs() < 1e-3, "con voz: −12 dB");
        assert!((v(120) + 18.0).abs() < 1e-3);
        assert!(v(57) < -7.0 && v(57) > -17.0, "bajando: {}", v(57));
        assert!((v(168) + 6.0).abs() < 1e-3, "recuperado tras la voz: {}", v(168));
        assert!((v(500) + 6.0).abs() < 1e-3);

        // Volver a aplicar sin voz deja la música sin keyframes.
        assert_eq!(apply_ducking(&mut tl, track, &[], &s), 0);
        assert!(tl.track(track).clips[0].anim.volume.keys.is_empty());
    }
}
