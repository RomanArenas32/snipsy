//! Modelo de la línea de tiempo.
//!
//! Todas las posiciones se expresan en **cuadros del proyecto** (`Frame`), no en
//! segundos, para evitar errores de redondeo: un corte siempre cae en un cuadro.
//! `source_in` también está en cuadros del proyecto (segundos = cuadros / fps).
//!
//! Las ediciones siguen la semántica de "sobrescritura" de los NLE: lo que se
//! coloca encima de otro clip lo recorta o lo divide.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::compose::{TextStyle, Transform};
use crate::effects::{Animation, ColorGrade};

pub type Frame = i64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AssetId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClipId(pub u64);

/// Archivo importado al proyecto (media pool).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    pub path: PathBuf,
    pub name: String,
    /// Duración en segundos.
    pub duration: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub has_video: bool,
    pub has_audio: bool,
    /// Imagen fija (PNG, JPG…): se puede alargar sin límite en el timeline.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub still: bool,
    /// Trayectoria del rostro principal (se calcula al activar «Seguir rostro»).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces: Option<crate::follow::FaceTrack>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrackRef {
    pub kind: TrackKind,
    pub index: usize,
}

impl TrackRef {
    pub fn video(index: usize) -> Self {
        Self { kind: TrackKind::Video, index }
    }
    pub fn audio(index: usize) -> Self {
        Self { kind: TrackKind::Audio, index }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub asset: AssetId,
    /// Posición en el timeline.
    pub start: Frame,
    pub len: Frame,
    /// Punto de entrada dentro del archivo fuente.
    pub source_in: Frame,
    /// Clips con el mismo `link` (p. ej. video + su audio) se editan juntos.
    pub link: Option<u64>,
    /// Posición, escala, rotación y opacidad (solo pistas de video).
    #[serde(default)]
    pub transform: Transform,
    /// Si es un clip de texto (no usa archivo: `asset` es `AssetId(0)`).
    #[serde(default)]
    pub text: Option<TextStyle>,
    /// Keyframes (sobrescriben los valores fijos de `transform` / `volume_db`).
    #[serde(default, skip_serializing_if = "Animation::is_empty")]
    pub anim: Animation,
    /// Fundido de entrada / salida, en cuadros (video: opacidad, audio: volumen).
    #[serde(default)]
    pub fade_in: Frame,
    #[serde(default)]
    pub fade_out: Frame,
    /// Disolvencia (video) o fundido cruzado (audio) con el clip anterior de la pista, en cuadros.
    #[serde(default)]
    pub transition_in: Frame,
    /// Volumen en dB (solo audio).
    #[serde(default)]
    pub volume_db: f32,
    /// Corrección de color (solo video).
    #[serde(default)]
    pub color: ColorGrade,
    /// Velocidad de reproducción (1 = normal, 2 = el doble de rápido, 0.5 = cámara lenta).
    /// `len` está en cuadros del timeline; el clip consume `len × speed` cuadros de la fuente.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub speed: f64,
    /// Desplazar el recorte para seguir el rostro principal (videos más grandes que el lienzo).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub follow_face: bool,
}

fn one() -> f64 {
    1.0
}

fn is_one(v: &f64) -> bool {
    *v == 1.0
}

/// Velocidades permitidas.
pub const SPEED_RANGE: std::ops::RangeInclusive<f64> = 0.25..=4.0;

impl Clip {
    pub fn new(id: ClipId, asset: AssetId, start: Frame, len: Frame) -> Self {
        Self {
            id,
            asset,
            start,
            len,
            source_in: 0,
            link: None,
            transform: Transform::default(),
            text: None,
            anim: Animation::default(),
            fade_in: 0,
            fade_out: 0,
            transition_in: 0,
            volume_db: 0.0,
            color: ColorGrade::default(),
            speed: 1.0,
            follow_face: false,
        }
    }

    /// Cuadros de la fuente que corresponden a `d` cuadros del timeline.
    pub fn src_offset(&self, d: Frame) -> Frame {
        (d as f64 * self.speed).round() as Frame
    }

    /// Cuadros del timeline (hacia abajo) que caben en `src` cuadros de la fuente.
    pub fn tl_offset(&self, src: Frame) -> Frame {
        (src as f64 / self.speed + 1e-6).floor() as Frame
    }

    /// Último cuadro de la fuente (exclusivo) que usa el clip.
    pub fn source_end(&self) -> Frame {
        self.source_in + self.src_offset(self.len)
    }

    /// Divide el clip en `at` (que debe estar dentro). Devuelve la parte derecha
    /// con el id dado; los fundidos y la transición quedan en el borde que corresponde.
    fn split_off(&mut self, at: Frame, id: ClipId, link: Option<u64>) -> Clip {
        let right = Clip {
            id,
            start: at,
            len: self.end() - at,
            source_in: self.source_in + self.src_offset(at - self.start),
            link,
            fade_in: 0,
            transition_in: 0,
            ..self.clone()
        };
        self.len = at - self.start;
        self.fade_out = 0;
        right
    }

    pub fn end(&self) -> Frame {
        self.start + self.len
    }
    pub fn contains(&self, frame: Frame) -> bool {
        self.start <= frame && frame < self.end()
    }
    pub fn is_text(&self) -> bool {
        self.text.is_some()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    /// Video: oculto. Audio: silenciado.
    pub muted: bool,
    /// Siempre ordenados por `start` y sin solaparse.
    pub clips: Vec<Clip>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Timeline {
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub assets: Vec<Asset>,
    pub video_tracks: Vec<Track>,
    pub audio_tracks: Vec<Track>,
    next_id: u64,
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Timeline {
    pub fn new() -> Self {
        let mut t = Self {
            fps: 30.0,
            width: 1920,
            height: 1080,
            assets: Vec::new(),
            video_tracks: Vec::new(),
            audio_tracks: Vec::new(),
            next_id: 1,
        };
        t.add_track(TrackKind::Video);
        t.add_track(TrackKind::Audio);
        t
    }

    fn next(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // ---------------------------------------------------------------- pistas

    pub fn tracks(&self, kind: TrackKind) -> &Vec<Track> {
        match kind {
            TrackKind::Video => &self.video_tracks,
            TrackKind::Audio => &self.audio_tracks,
        }
    }

    pub fn tracks_mut(&mut self, kind: TrackKind) -> &mut Vec<Track> {
        match kind {
            TrackKind::Video => &mut self.video_tracks,
            TrackKind::Audio => &mut self.audio_tracks,
        }
    }

    pub fn track(&self, r: TrackRef) -> &Track {
        &self.tracks(r.kind)[r.index]
    }

    pub fn track_mut(&mut self, r: TrackRef) -> &mut Track {
        &mut self.tracks_mut(r.kind)[r.index]
    }

    pub fn add_track(&mut self, kind: TrackKind) -> usize {
        let tracks = self.tracks_mut(kind);
        let prefix = if kind == TrackKind::Video { "V" } else { "A" };
        tracks.push(Track { name: format!("{prefix}{}", tracks.len() + 1), ..Default::default() });
        tracks.len() - 1
    }

    fn ensure_track(&mut self, kind: TrackKind, index: usize) {
        while self.tracks(kind).len() <= index {
            self.add_track(kind);
        }
    }

    // ---------------------------------------------------------------- medios

    pub fn asset(&self, id: AssetId) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// Agrega un archivo al proyecto (si ya existe, devuelve el existente).
    /// El primer video importado define la resolución y fps del proyecto.
    pub fn add_asset(&mut self, mut asset: Asset) -> AssetId {
        if let Some(existing) = self.assets.iter().find(|a| a.path == asset.path) {
            return existing.id;
        }
        if self.assets.is_empty() && self.clips().next().is_none() && asset.has_video && !asset.still {
            if asset.fps > 0.0 {
                self.fps = asset.fps;
            }
            if asset.width > 0 && asset.height > 0 {
                self.width = asset.width;
                self.height = asset.height;
            }
        }
        asset.id = AssetId(self.next());
        let id = asset.id;
        self.assets.push(asset);
        id
    }

    /// ¿El clip se puede alargar sin límite? (textos e imágenes fijas no tienen fin de archivo).
    pub fn is_unbounded(&self, c: &Clip) -> bool {
        c.is_text() || self.asset(c.asset).is_some_and(|a| a.still)
    }

    /// Duración del archivo en cuadros del proyecto.
    pub fn asset_frames(&self, id: AssetId) -> Frame {
        self.asset(id)
            .map(|a| ((a.duration * self.fps) + 1e-6).floor() as Frame)
            .unwrap_or(0)
            .max(1)
    }

    pub fn frame_to_secs(&self, frame: Frame) -> f64 {
        frame as f64 / self.fps
    }

    pub fn secs_to_frame(&self, secs: f64) -> Frame {
        (secs * self.fps + 1e-6).floor() as Frame
    }

    // ---------------------------------------------------------------- consultas

    /// Fin del último clip.
    pub fn duration(&self) -> Frame {
        self.clips().map(|(_, c)| c.end()).max().unwrap_or(0)
    }

    pub fn clips(&self) -> impl Iterator<Item = (TrackRef, &Clip)> {
        let video = self.video_tracks.iter().enumerate().flat_map(|(i, t)| {
            t.clips.iter().map(move |c| (TrackRef::video(i), c))
        });
        let audio = self.audio_tracks.iter().enumerate().flat_map(|(i, t)| {
            t.clips.iter().map(move |c| (TrackRef::audio(i), c))
        });
        video.chain(audio)
    }

    pub fn find(&self, id: ClipId) -> Option<(TrackRef, &Clip)> {
        self.clips().find(|(_, c)| c.id == id)
    }

    fn find_index(&self, id: ClipId) -> Option<(TrackRef, usize)> {
        for kind in [TrackKind::Video, TrackKind::Audio] {
            for (ti, t) in self.tracks(kind).iter().enumerate() {
                if let Some(ci) = t.clips.iter().position(|c| c.id == id) {
                    return Some((TrackRef { kind, index: ti }, ci));
                }
            }
        }
        None
    }

    /// Expande una selección con los clips vinculados.
    pub fn with_links(&self, ids: impl IntoIterator<Item = ClipId>) -> Vec<ClipId> {
        let ids: HashSet<ClipId> = ids.into_iter().collect();
        let links: HashSet<u64> = self
            .clips()
            .filter(|(_, c)| ids.contains(&c.id))
            .filter_map(|(_, c)| c.link)
            .collect();
        let mut out: Vec<ClipId> = self
            .clips()
            .filter(|(_, c)| ids.contains(&c.id) || c.link.is_some_and(|l| links.contains(&l)))
            .map(|(_, c)| c.id)
            .collect();
        out.sort();
        out
    }

    /// Bordes de clips (y el inicio) para el imán.
    pub fn snap_points(&self, exclude: &HashSet<ClipId>) -> Vec<Frame> {
        let mut pts = vec![0];
        for (_, c) in self.clips() {
            if !exclude.contains(&c.id) {
                pts.push(c.start);
                pts.push(c.end());
            }
        }
        pts.sort_unstable();
        pts.dedup();
        pts
    }

    // ---------------------------------------------------------------- ediciones

    /// Coloca un archivo en el timeline (sobrescribiendo). Video y audio quedan vinculados.
    pub fn insert_asset(&mut self, asset: AssetId, at: Frame, video_track: usize, audio_track: usize) -> Vec<ClipId> {
        let Some(a) = self.asset(asset).cloned() else { return Vec::new() };
        let len = self.asset_frames(asset);
        let at = at.max(0);
        let link = (a.has_video && a.has_audio).then(|| self.next());
        let mut ids = Vec::new();
        let mut place = |tl: &mut Self, r: TrackRef| {
            tl.ensure_track(r.kind, r.index);
            tl.clear_range(r, at, at + len);
            let id = ClipId(tl.next());
            tl.place(r, Clip { link, ..Clip::new(id, asset, at, len) });
            ids.push(id);
        };
        if a.has_video {
            place(self, TrackRef::video(video_track));
        }
        if a.has_audio {
            place(self, TrackRef::audio(audio_track));
        }
        ids
    }

    /// Coloca un clip de texto en una pista de video (sobrescribiendo).
    pub fn insert_text(&mut self, at: Frame, video_track: usize, len: Frame, style: TextStyle) -> ClipId {
        let r = TrackRef::video(video_track);
        let at = at.max(0);
        let len = len.max(1);
        self.ensure_track(TrackKind::Video, video_track);
        self.clear_range(r, at, at + len);
        let id = ClipId(self.next());
        self.place(r, Clip { text: Some(style), ..Clip::new(id, AssetId(0), at, len) });
        id
    }

    pub fn clip_mut(&mut self, id: ClipId) -> Option<&mut Clip> {
        let (r, ci) = self.find_index(id)?;
        Some(&mut self.track_mut(r).clips[ci])
    }

    /// Agrega un archivo al final de V1/A1.
    pub fn append_asset(&mut self, asset: AssetId) -> Vec<ClipId> {
        let end = |t: Option<&Track>| t.and_then(|t| t.clips.last()).map(|c| c.end()).unwrap_or(0);
        let at = end(self.video_tracks.first()).max(end(self.audio_tracks.first()));
        self.insert_asset(asset, at, 0, 0)
    }

    /// Libera el rango `[start, end)` de una pista recortando, dividiendo o borrando clips.
    pub fn clear_range(&mut self, r: TrackRef, start: Frame, end: Frame) {
        if start >= end {
            return;
        }
        let old = std::mem::take(&mut self.track_mut(r).clips);
        let mut out = Vec::with_capacity(old.len() + 1);
        for mut c in old {
            if c.end() <= start || c.start >= end {
                out.push(c);
            } else if c.start >= start && c.end() <= end {
                // Cubierto por completo: se elimina.
            } else if c.start < start && c.end() > end {
                // El rango cae dentro del clip: se divide en dos.
                let id = ClipId(self.next());
                let right = c.split_off(end, id, None);
                c.split_off(start, ClipId(0), None);
                out.push(c);
                out.push(right);
            } else if c.start < start {
                c.split_off(start, ClipId(0), None);
                out.push(c);
            } else {
                let right = c.split_off(end, c.id, c.link);
                out.push(right);
            }
        }
        self.track_mut(r).clips = out;
    }

    fn place(&mut self, r: TrackRef, clip: Clip) {
        let clips = &mut self.track_mut(r).clips;
        let pos = clips.partition_point(|c| c.start < clip.start);
        clips.insert(pos, clip);
    }

    /// Mueve clips (y sus vinculados) `delta` cuadros y cambia de pista.
    /// `track_delta` = (pistas de video, pistas de audio); solo afecta a los clips de cada tipo.
    pub fn move_clips(&mut self, ids: &[ClipId], delta: Frame, track_delta: (i64, i64)) -> bool {
        let ids = self.with_links(ids.iter().copied());
        if ids.is_empty() {
            return false;
        }
        let moving: Vec<(TrackRef, Clip)> = ids
            .iter()
            .filter_map(|id| self.find(*id).map(|(r, c)| (r, c.clone())))
            .collect();

        // Limitar para no salir antes del cuadro 0 ni fuera de las pistas existentes.
        let min_start = moving.iter().map(|(_, c)| c.start).min().unwrap_or(0);
        let delta = delta.max(-min_start);
        let clamp_track = |kind: TrackKind, d: i64| -> i64 {
            let idx: Vec<i64> = moving.iter().filter(|(r, _)| r.kind == kind).map(|(r, _)| r.index as i64).collect();
            let (Some(lo), Some(hi)) = (idx.iter().min(), idx.iter().max()) else { return 0 };
            let len = self.tracks(kind).len() as i64;
            d.clamp(-lo, len - 1 - hi)
        };
        let dv = clamp_track(TrackKind::Video, track_delta.0);
        let da = clamp_track(TrackKind::Audio, track_delta.1);
        if delta == 0 && dv == 0 && da == 0 {
            return false;
        }

        for (r, c) in &moving {
            self.track_mut(*r).clips.retain(|x| x.id != c.id);
        }
        for (r, mut c) in moving {
            let d = if r.kind == TrackKind::Video { dv } else { da };
            let dest = TrackRef { kind: r.kind, index: (r.index as i64 + d) as usize };
            c.start += delta;
            self.clear_range(dest, c.start, c.end());
            self.place(dest, c);
        }
        true
    }

    /// Recorta un borde de un clip (y de sus vinculados) hacia `target`.
    /// Respeta los límites del archivo fuente y no invade clips vecinos.
    pub fn trim(&mut self, id: ClipId, edge: Edge, target: Frame) -> bool {
        let Some((_, primary)) = self.find(id) else { return false };
        let desired = match edge {
            Edge::Start => target - primary.start,
            Edge::End => target - primary.end(),
        };
        let group = self.with_links([id]);
        let (mut lo, mut hi) = (Frame::MIN, Frame::MAX);
        for gid in &group {
            let (r, ci) = self.find_index(*gid).unwrap();
            let clips = &self.track(r).clips;
            let c = &clips[ci];
            match edge {
                Edge::Start => {
                    let prev_end = if ci > 0 { clips[ci - 1].end() } else { 0 };
                    let source_limit = if self.is_unbounded(c) { Frame::MIN / 4 } else { -c.tl_offset(c.source_in) };
                    lo = lo.max(source_limit).max(prev_end - c.start);
                    hi = hi.min(c.len - 1);
                }
                Edge::End => {
                    let next_start = clips.get(ci + 1).map(|n| n.start).unwrap_or(Frame::MAX);
                    // El texto no tiene archivo fuente: se puede alargar sin límite.
                    let available =
                        if self.is_unbounded(c) { Frame::MAX / 4 } else { c.tl_offset(self.asset_frames(c.asset) - c.source_end()) };
                    hi = hi.min(available).min(next_start.saturating_sub(c.end()));
                    lo = lo.max(1 - c.len);
                }
            }
        }
        if lo > hi {
            return false;
        }
        let d = desired.clamp(lo, hi);
        if d == 0 {
            return false;
        }
        for gid in group {
            let (r, ci) = self.find_index(gid).unwrap();
            let c = &mut self.track_mut(r).clips[ci];
            match edge {
                Edge::Start => {
                    c.start += d;
                    c.source_in += c.src_offset(d);
                    c.len -= d;
                }
                Edge::End => c.len += d,
            }
        }
        true
    }

    /// Corta en `at`. Si `ids` es `None`, corta todos los clips bajo ese cuadro.
    pub fn split(&mut self, at: Frame, ids: Option<&[ClipId]>) -> bool {
        let filter: Option<HashSet<ClipId>> = ids.map(|ids| self.with_links(ids.iter().copied()).into_iter().collect());
        let targets: Vec<ClipId> = self
            .clips()
            .filter(|(_, c)| c.start < at && at < c.end())
            .filter(|(_, c)| filter.as_ref().is_none_or(|f| f.contains(&c.id)))
            .map(|(_, c)| c.id)
            .collect();
        let mut new_links: HashMap<u64, u64> = HashMap::new();
        for id in &targets {
            let (r, ci) = self.find_index(*id).unwrap();
            let link = self.track(r).clips[ci].link.map(|l| *new_links.entry(l).or_insert_with(|| {
                let id = self.next_id;
                self.next_id += 1;
                id
            }));
            let id = ClipId(self.next());
            let clips = &mut self.track_mut(r).clips;
            let right = clips[ci].split_off(at, id, link);
            clips.insert(ci + 1, right);
        }
        !targets.is_empty()
    }

    /// Cambia la velocidad de un clip (y sus vinculados). El clip se alarga o se
    /// acorta conservando el contenido, y los clips siguientes de sus pistas se desplazan.
    pub fn set_speed(&mut self, id: ClipId, speed: f64) -> bool {
        let speed = speed.clamp(*SPEED_RANGE.start(), *SPEED_RANGE.end());
        let group = self.with_links([id]);
        let mut changed = false;
        for gid in group {
            let Some((r, ci)) = self.find_index(gid) else { continue };
            let c = self.track(r).clips[ci].clone();
            if self.is_unbounded(&c) || c.speed == speed {
                continue;
            }
            let source_len = c.src_offset(c.len);
            let mut len = ((source_len as f64 / speed).round() as Frame).max(1);
            let limit = ((self.asset_frames(c.asset) - c.source_in) as f64 / speed + 1e-6).floor() as Frame;
            len = len.min(limit.max(1));
            let delta = len - c.len;
            let old_end = c.end();
            let clips = &mut self.track_mut(r).clips;
            for other in clips.iter_mut().filter(|o| o.start >= old_end) {
                other.start += delta;
            }
            let c = &mut clips[ci];
            c.speed = speed;
            c.len = len;
            c.fade_in = c.fade_in.min(len);
            c.fade_out = c.fade_out.min(len);
            changed = true;
        }
        changed
    }

    /// Borra clips (y sus vinculados). Con `ripple`, cierra el hueco en esas pistas.
    pub fn delete(&mut self, ids: &[ClipId], ripple: bool) -> bool {
        let ids: HashSet<ClipId> = self.with_links(ids.iter().copied()).into_iter().collect();
        if ids.is_empty() {
            return false;
        }
        for kind in [TrackKind::Video, TrackKind::Audio] {
            for track in self.tracks_mut(kind) {
                let mut removed: Vec<Clip> = track.clips.iter().filter(|c| ids.contains(&c.id)).cloned().collect();
                track.clips.retain(|c| !ids.contains(&c.id));
                if ripple {
                    removed.sort_by_key(|c| std::cmp::Reverse(c.start));
                    for r in removed {
                        for c in track.clips.iter_mut().filter(|c| c.start >= r.start) {
                            c.start -= r.len;
                        }
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(tl: &mut Timeline, name: &str, secs: f64, audio: bool) -> AssetId {
        tl.add_asset(Asset {
            id: AssetId(0),
            path: PathBuf::from(format!("/media/{name}.mp4")),
            name: name.into(),
            duration: secs,
            fps: 30.0,
            width: 1920,
            height: 1080,
            has_video: true,
            has_audio: audio,
            still: false,
            faces: None,
        })
    }

    fn spans(tl: &Timeline, r: TrackRef) -> Vec<(Frame, Frame, Frame)> {
        tl.track(r).clips.iter().map(|c| (c.start, c.len, c.source_in)).collect()
    }

    #[test]
    fn append_links_video_and_audio() {
        let mut tl = Timeline::new();
        let a = asset(&mut tl, "a", 2.0, true);
        let ids = tl.append_asset(a);
        assert_eq!(ids.len(), 2);
        tl.append_asset(a);
        assert_eq!(spans(&tl, TrackRef::video(0)), vec![(0, 60, 0), (60, 60, 0)]);
        assert_eq!(spans(&tl, TrackRef::audio(0)), vec![(0, 60, 0), (60, 60, 0)]);
        assert_eq!(tl.with_links([ids[0]]), ids);
        assert_eq!(tl.duration(), 120);
    }

    #[test]
    fn overwrite_splits_underlying_clip() {
        let mut tl = Timeline::new();
        let a = asset(&mut tl, "a", 10.0, false);
        let b = asset(&mut tl, "b", 1.0, false);
        tl.append_asset(a);
        tl.insert_asset(b, 100, 0, 0);
        assert_eq!(spans(&tl, TrackRef::video(0)), vec![(0, 100, 0), (100, 30, 0), (130, 170, 130)]);
    }

    #[test]
    fn trim_respects_source_neighbors_and_links() {
        let mut tl = Timeline::new();
        let a = asset(&mut tl, "a", 2.0, true);
        let first = tl.append_asset(a)[0];
        let second = tl.append_asset(a)[0];
        // No se puede alargar más allá del archivo fuente (ni sobre el vecino).
        assert!(!tl.trim(first, Edge::End, 90));
        // Recortar el final 20 cuadros, también en el audio vinculado.
        assert!(tl.trim(first, Edge::End, 40));
        assert_eq!(spans(&tl, TrackRef::audio(0))[0], (0, 40, 0));
        // El inicio del segundo puede retroceder solo hasta el vecino y la fuente (in = 0).
        assert!(!tl.trim(second, Edge::Start, 10));
        assert!(tl.trim(second, Edge::Start, 75));
        assert_eq!(spans(&tl, TrackRef::video(0))[1], (75, 45, 15));
        assert!(tl.trim(second, Edge::Start, 0));
        assert_eq!(spans(&tl, TrackRef::video(0))[1], (60, 60, 0));
    }

    #[test]
    fn split_keeps_links_paired() {
        let mut tl = Timeline::new();
        let a = asset(&mut tl, "a", 2.0, true);
        tl.append_asset(a);
        assert!(tl.split(20, None));
        assert_eq!(spans(&tl, TrackRef::video(0)), vec![(0, 20, 0), (20, 40, 20)]);
        let right_video = tl.video_tracks[0].clips[1].id;
        let right_audio = tl.audio_tracks[0].clips[1].id;
        let left_video = tl.video_tracks[0].clips[0].id;
        assert_eq!(tl.with_links([right_video]), vec![right_video.min(right_audio), right_video.max(right_audio)]);
        assert!(!tl.with_links([left_video]).contains(&right_audio));
    }

    #[test]
    fn ripple_delete_closes_gap() {
        let mut tl = Timeline::new();
        let a = asset(&mut tl, "a", 1.0, true);
        let first = tl.append_asset(a)[0];
        tl.append_asset(a);
        tl.append_asset(a);
        assert!(tl.delete(&[first], true));
        assert_eq!(spans(&tl, TrackRef::video(0)), vec![(0, 30, 0), (30, 30, 0)]);
        assert_eq!(spans(&tl, TrackRef::audio(0)), vec![(0, 30, 0), (30, 30, 0)]);
    }

    #[test]
    fn move_clamps_and_overwrites() {
        let mut tl = Timeline::new();
        let a = asset(&mut tl, "a", 1.0, false);
        let first = tl.append_asset(a)[0];
        let second = tl.append_asset(a)[0];
        // No puede ir antes del cuadro 0.
        assert!(!tl.move_clips(&[first], -10, (0, 0)));
        // Mover el segundo 10 cuadros a la izquierda recorta el final del primero.
        assert!(tl.move_clips(&[second], -10, (0, 0)));
        assert_eq!(spans(&tl, TrackRef::video(0)), vec![(0, 20, 0), (20, 30, 0)]);
        // Cambiar de pista: solo si existe.
        assert!(!tl.move_clips(&[second], 0, (1, 0)));
        tl.add_track(TrackKind::Video);
        assert!(tl.move_clips(&[second], 0, (1, 0)));
        assert_eq!(spans(&tl, TrackRef::video(1)), vec![(20, 30, 0)]);
    }

    #[test]
    fn text_clips_extend_freely_and_split_keeps_style() {
        let mut tl = Timeline::new();
        let style = TextStyle { text: "Hola".into(), ..Default::default() };
        let id = tl.insert_text(30, 1, 60, style.clone());
        assert_eq!(tl.video_tracks.len(), 2);
        assert!(tl.trim(id, Edge::End, 300));
        assert!(tl.trim(id, Edge::Start, 0));
        assert_eq!(spans(&tl, TrackRef::video(1)), vec![(0, 300, -30)]);
        tl.clip_mut(id).unwrap().transform.scale = 2.0;
        assert!(tl.split(100, None));
        let right = &tl.video_tracks[1].clips[1];
        assert_eq!(right.text.as_ref().unwrap().text, "Hola");
        assert_eq!(right.transform.scale, 2.0);
    }

    #[test]
    fn undo_redo_and_project_roundtrip() {
        let mut tl = Timeline::new();
        let a = asset(&mut tl, "a", 1.0, true);
        let mut history = crate::History::default();
        history.checkpoint(&tl);
        tl.append_asset(a);
        assert_eq!(tl.duration(), 30);
        assert!(history.undo(&mut tl));
        assert_eq!(tl.duration(), 0);
        assert!(history.redo(&mut tl));
        assert_eq!(tl.duration(), 30);

        let path = std::env::temp_dir().join("snipsy_roundtrip.snipsy");
        crate::project::save(&path, &tl).unwrap();
        let loaded = crate::project::load(&path).unwrap();
        assert_eq!(loaded.video_tracks[0].clips, tl.video_tracks[0].clips);
        assert_eq!(loaded.assets.len(), 1);
    }
}
