//! Acciones con nombre: lo que se puede pedir a Snipsy por chat (atajos o IA).
//!
//! - [`Command`]: ediciones puras del timeline, que se aplican aquí mismo.
//! - [`Action`]: un comando o una función de la app que necesita medios o ventanas
//!   (cortar silencios, subtítulos, marca, exportar…); esas las ejecuta la app.
//!
//! Todo es serializable en JSON para que más adelante la IA las pida como herramientas.

use serde::{Deserialize, Serialize};

use crate::{silence, ClipId, Frame, TextStyle, Timeline, TrackKind};

/// A qué clips se aplica una edición.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// Los clips seleccionados en el timeline.
    Selected,
    /// Todos los clips de video (y su audio).
    All,
    /// El primer / último clip de video del proyecto (con su audio).
    First,
    Last,
    /// Un clip concreto.
    Clip(u64),
}

/// Cambios de color relativos (se suman a los actuales).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorChange {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub black_and_white: bool,
    pub reset: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// Cortar en un segundo (o en el cabezal) los clips bajo ese punto.
    Split { at: Option<f64> },
    /// Quitar un tramo de tiempo de todo el video y cerrar el hueco (la música y el logo siguen).
    CutRange { from: f64, to: f64 },
    /// Borrar clips (y su audio vinculado).
    Delete { target: Target, ripple: bool },
    Speed { target: Target, speed: f64 },
    /// Volumen: `relative` suma dB al actual; si no, lo fija.
    Volume { target: Target, db: f32, relative: bool },
    Fade { target: Target, fade_in: Option<f64>, fade_out: Option<f64> },
    /// Título en el segundo indicado (o en el cabezal).
    AddText { text: String, at: Option<f64>, secs: f64 },
    Color { target: Target, change: ColorChange },
    /// Cambiar el formato del proyecto reencuadrando el contenido.
    Format { width: u32, height: u32, follow_faces: bool },
    FollowFace { target: Target, on: bool },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Edit(Command),
    CutSilences,
    Subtitles,
    PlaceLogo,
    /// Logo + colores y tipografía de la marca activa en los textos.
    ApplyBrand,
    Ducking,
    /// Abrir la exportación con estos formatos elegidos (vacío = el actual). Siempre confirma el usuario.
    Export { formats: Vec<(u32, u32)> },
    Undo,
    Redo,
    Play,
    Pause,
    Help,
}

/// Estado de la interfaz que necesitan los comandos.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub selection: Vec<ClipId>,
    pub playhead: Frame,
}

/// Resultado de un comando: mensaje para el usuario y clips a seleccionar.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub message: String,
    pub select: Vec<ClipId>,
}

impl Outcome {
    fn msg(message: impl Into<String>) -> Self {
        Self { message: message.into(), select: Vec::new() }
    }
}

fn secs_label(s: f64) -> String {
    let m = (s / 60.0).floor();
    format!("{}:{:04.1}", m as i64, s - m * 60.0)
}

/// Clips afectados (con sus vinculados). `video_only`: solo clips de video que no son texto.
fn resolve(tl: &Timeline, target: &Target, ctx: &Context, video_only: bool) -> Result<Vec<ClipId>, String> {
    let ids: Vec<ClipId> = match target {
        Target::Selected => {
            if ctx.selection.is_empty() {
                return Err("Primero selecciona un clip en el timeline (o di «a todos»).".into());
            }
            tl.with_links(ctx.selection.iter().copied())
        }
        Target::All => tl
            .clips()
            .filter(|(r, c)| r.kind == TrackKind::Video && !c.is_text() && tl.track(*r).name != crate::brand::LOGO_TRACK)
            .map(|(_, c)| c.id)
            .collect::<Vec<_>>(),
        Target::First | Target::Last => {
            let videos = tl
                .clips()
                .filter(|(r, c)| r.kind == TrackKind::Video && !c.is_text() && tl.track(*r).name != crate::brand::LOGO_TRACK);
            let pick = if matches!(target, Target::First) {
                videos.min_by_key(|(_, c)| c.start)
            } else {
                videos.max_by_key(|(_, c)| c.end())
            };
            pick.map(|(_, c)| tl.with_links([c.id])).unwrap_or_default()
        }
        Target::Clip(id) => {
            if tl.find(ClipId(*id)).is_none() {
                return Err(format!("No existe el clip {id}."));
            }
            vec![ClipId(*id)]
        }
    };
    let ids = if matches!(target, Target::All) { tl.with_links(ids) } else { ids };
    let ids: Vec<ClipId> = ids
        .into_iter()
        .filter(|id| {
            let Some((r, c)) = tl.find(*id) else { return false };
            !video_only || (r.kind == TrackKind::Video && !c.is_text() && !tl.is_unbounded(c))
        })
        .collect();
    if ids.is_empty() {
        return Err(if video_only { "No hay clips de video a los que aplicarlo." } else { "No hay clips a los que aplicarlo." }.into());
    }
    Ok(ids)
}

fn count(n: usize) -> String {
    if n == 1 { "1 clip".into() } else { format!("{n} clips") }
}

impl Command {
    /// Aplica el comando. Si falla, el timeline queda como estaba.
    pub fn apply(&self, tl: &mut Timeline, ctx: &Context) -> Result<Outcome, String> {
        let mut work = tl.clone();
        let out = self.apply_inner(&mut work, ctx)?;
        *tl = work;
        Ok(out)
    }

    fn apply_inner(&self, tl: &mut Timeline, ctx: &Context) -> Result<Outcome, String> {
        let duration = tl.frame_to_secs(tl.duration());
        if tl.duration() == 0 && !matches!(self, Command::AddText { .. } | Command::Format { .. }) {
            return Err("El proyecto está vacío: importa un video primero.".into());
        }
        match self {
            Command::Split { at } => {
                let frame = at.map(|s| tl.secs_to_frame(s)).unwrap_or(ctx.playhead);
                let ids = (!ctx.selection.is_empty()).then_some(ctx.selection.as_slice());
                if tl.split(frame, ids) {
                    Ok(Outcome::msg(format!("Cortado en {}.", secs_label(tl.frame_to_secs(frame)))))
                } else {
                    Err(format!("No hay ningún clip que cortar en {}.", secs_label(tl.frame_to_secs(frame))))
                }
            }
            Command::CutRange { from, to } => {
                let (from, to) = (from.max(0.0), to.min(duration));
                if to <= from {
                    return Err(format!("El tramo {} – {} está fuera del video ({}).", secs_label(from), secs_label(to), secs_label(duration)));
                }
                let range = (tl.secs_to_frame(from), tl.secs_to_frame(to));
                let keep = silence::continuous_tracks(tl, silence::guess_music_track(tl));
                let removed = silence::cut_ranges(tl, &[range], &keep);
                Ok(Outcome::msg(format!("Quitado de {} a {} ({:.1} s).", secs_label(from), secs_label(to), tl.frame_to_secs(removed))))
            }
            Command::Delete { target, ripple } => {
                let ids = resolve(tl, target, ctx, false)?;
                tl.delete(&ids, *ripple);
                Ok(Outcome::msg(format!("Borrado(s) {}.", count(ids.len()))))
            }
            Command::Speed { target, speed } => {
                if !speed.is_finite() || *speed <= 0.0 {
                    return Err("Velocidad inválida.".into());
                }
                let ids = resolve(tl, target, ctx, false)?;
                let speed = speed.clamp(*crate::timeline::SPEED_RANGE.start(), *crate::timeline::SPEED_RANGE.end());
                // Un clip por grupo vinculado basta (set_speed cambia los vinculados).
                let mut done = std::collections::HashSet::new();
                for id in &ids {
                    if done.insert(*id) {
                        done.extend(tl.with_links([*id]));
                        tl.set_speed(*id, speed);
                    }
                }
                Ok(Outcome::msg(format!("Velocidad {speed}× en {}.", count(ids.len()))))
            }
            Command::Volume { target, db, relative } => {
                let ids = resolve(tl, target, ctx, false)?;
                let mut n = 0;
                for id in ids {
                    if tl.find(id).is_some_and(|(r, _)| r.kind == TrackKind::Audio) {
                        let c = tl.clip_mut(id).unwrap();
                        c.anim.volume.keys.clear();
                        c.volume_db = if *relative { c.volume_db + db } else { *db }.clamp(-60.0, 12.0);
                        n += 1;
                    }
                }
                if n == 0 {
                    return Err("Esos clips no tienen audio.".into());
                }
                Ok(Outcome::msg(if *relative {
                    format!("Volumen {db:+.0} dB en {}.", count(n))
                } else {
                    format!("Volumen a {db:.0} dB en {}.", count(n))
                }))
            }
            Command::Fade { target, fade_in, fade_out } => {
                let ids = resolve(tl, target, ctx, false)?;
                let fps = tl.fps;
                for id in &ids {
                    let c = tl.clip_mut(*id).unwrap();
                    if let Some(s) = fade_in {
                        c.fade_in = ((s * fps).round() as Frame).clamp(0, c.len);
                    }
                    if let Some(s) = fade_out {
                        c.fade_out = ((s * fps).round() as Frame).clamp(0, c.len);
                    }
                }
                Ok(Outcome::msg(format!("Fundido aplicado a {}.", count(ids.len()))))
            }
            Command::AddText { text, at, secs } => {
                let text = text.trim();
                if text.is_empty() {
                    return Err("¿Qué texto pongo? Escríbelo entre comillas, por ejemplo: título \"OFERTA 2x1\".".into());
                }
                let start = at.map(|s| tl.secs_to_frame(s)).unwrap_or(ctx.playhead).max(0);
                let len = ((secs.max(0.5) * tl.fps).round() as Frame).max(1);
                // Una pista de video libre arriba (sin pisar el logo ni otros textos).
                let free = |t: &crate::Track| t.name != crate::brand::LOGO_TRACK && !t.clips.iter().any(|c| c.start < start + len && c.end() > start);
                let track = match tl.video_tracks.iter().enumerate().skip(1).rev().find(|(_, t)| free(t)) {
                    Some((i, _)) => i,
                    None => tl.add_track(TrackKind::Video),
                };
                let id = tl.insert_text(start, track, len, TextStyle { text: text.to_owned(), ..TextStyle::default() });
                Ok(Outcome { message: format!("Texto «{text}» en {}.", secs_label(tl.frame_to_secs(start))), select: vec![id] })
            }
            Command::Color { target, change } => {
                let ids = resolve(tl, target, ctx, true)?;
                for id in &ids {
                    let g = &mut tl.clip_mut(*id).unwrap().color;
                    if change.reset {
                        let lut = (g.lut.take(), g.lut_mix);
                        *g = Default::default();
                        (g.lut, g.lut_mix) = lut;
                    }
                    g.exposure = (g.exposure + change.exposure).clamp(-3.0, 3.0);
                    g.contrast = (g.contrast + change.contrast).clamp(0.0, 2.0);
                    g.saturation = if change.black_and_white { 0.0 } else { (g.saturation + change.saturation).clamp(0.0, 2.0) };
                    g.temperature = (g.temperature + change.temperature).clamp(-1.0, 1.0);
                }
                Ok(Outcome::msg(format!("Color ajustado en {}.", count(ids.len()))))
            }
            Command::Format { width, height, follow_faces } => {
                let (w, h) = ((*width).clamp(16, 8192) & !1, (*height).clamp(16, 8192) & !1);
                if (w, h) == (tl.width, tl.height) {
                    return Ok(Outcome::msg(format!("El proyecto ya está en {w}×{h}.")));
                }
                *tl = crate::reframe::reframe(tl, w, h, *follow_faces);
                Ok(Outcome::msg(format!("Formato {w}×{h}: contenido reencuadrado.")))
            }
            Command::FollowFace { target, on } => {
                let ids = resolve(tl, target, ctx, true)?;
                for id in &ids {
                    tl.clip_mut(*id).unwrap().follow_face = *on;
                }
                Ok(Outcome::msg(format!("Seguir rostro {} en {}.", if *on { "activado" } else { "desactivado" }, count(ids.len()))))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Asset, AssetId};

    fn project() -> (Timeline, Vec<ClipId>) {
        let mut tl = Timeline::new();
        let a = tl.add_asset(Asset {
            id: AssetId(0),
            path: "/m/a.mp4".into(),
            name: "a".into(),
            duration: 10.0,
            fps: 30.0,
            width: 1920,
            height: 1080,
            has_video: true,
            has_audio: true,
            still: false,
            faces: None,
        });
        let ids = tl.append_asset(a);
        (tl, ids)
    }

    #[test]
    fn edits_and_failures_leave_timeline_consistent() {
        let (mut tl, ids) = project();
        let ctx = Context::default();
        // Sin selección: error y nada cambia.
        let before = tl.clone();
        assert!(Command::Speed { target: Target::Selected, speed: 2.0 }.apply(&mut tl, &ctx).is_err());
        assert_eq!(tl.duration(), before.duration());

        Command::CutRange { from: 2.0, to: 3.0 }.apply(&mut tl, &ctx).unwrap();
        assert_eq!(tl.duration(), 270);
        Command::Speed { target: Target::All, speed: 2.0 }.apply(&mut tl, &ctx).unwrap();
        assert_eq!(tl.duration(), 135);
        assert!(tl.clips().all(|(_, c)| c.speed == 2.0));

        let sel = Context { selection: vec![ids[1]], playhead: 0 };
        Command::Volume { target: Target::Selected, db: -6.0, relative: true }.apply(&mut tl, &sel).unwrap();
        assert!(tl.audio_tracks[0].clips.iter().any(|c| c.volume_db == -6.0));

        Command::Color { target: Target::All, change: ColorChange { black_and_white: true, ..Default::default() } }
            .apply(&mut tl, &ctx)
            .unwrap();
        assert!(tl.video_tracks[0].clips.iter().all(|c| c.color.saturation == 0.0));

        let out = Command::AddText { text: "OFERTA".into(), at: Some(1.0), secs: 2.0 }.apply(&mut tl, &ctx).unwrap();
        let (r, c) = tl.find(out.select[0]).unwrap();
        assert!(r.index >= 1 && c.start == 30 && c.len == 60);

        Command::Format { width: 1080, height: 1920, follow_faces: false }.apply(&mut tl, &ctx).unwrap();
        assert_eq!((tl.width, tl.height), (1080, 1920));
        assert!(Command::CutRange { from: 50.0, to: 60.0 }.apply(&mut tl, &ctx).is_err());
    }

    #[test]
    fn actions_roundtrip_as_json() {
        let a = Action::Edit(Command::CutRange { from: 1.0, to: 2.5 });
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), a);
        let e = Action::Export { formats: vec![(1080, 1920)] };
        assert_eq!(serde_json::from_str::<Action>(&serde_json::to_string(&e).unwrap()).unwrap(), e);
    }
}
