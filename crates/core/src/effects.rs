//! Efectos por clip: animación por keyframes, fundidos, transiciones, volumen y color.
//!
//! Los keyframes se guardan en **cuadros de la fuente** (`source_in + (cuadro - start)`),
//! de modo que quedan pegados al contenido: cortar o recortar un clip no los mueve.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::compose::Transform;
use crate::timeline::{Clip, Frame, Timeline, TrackKind, TrackRef};

/// Curva de interpolación hacia el siguiente keyframe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ease {
    Linear,
    /// Acelera y frena (ease in-out).
    #[default]
    Smooth,
    /// Mantiene el valor hasta el siguiente keyframe.
    Hold,
}

impl Ease {
    pub const ALL: [Ease; 3] = [Ease::Smooth, Ease::Linear, Ease::Hold];

    pub fn label(self) -> &'static str {
        match self {
            Ease::Linear => "Lineal",
            Ease::Smooth => "Suave",
            Ease::Hold => "Mantener",
        }
    }

    fn apply(self, t: f32) -> f32 {
        match self {
            Ease::Linear => t,
            Ease::Smooth => t * t * (3.0 - 2.0 * t),
            Ease::Hold => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Cuadro de la fuente.
    pub at: Frame,
    pub value: f32,
    #[serde(default)]
    pub ease: Ease,
}

/// Keyframes de una propiedad, ordenados por `at`. Vacía = la propiedad no está animada.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub keys: Vec<Keyframe>,
}

impl Curve {
    pub fn is_animated(&self) -> bool {
        !self.keys.is_empty()
    }

    pub fn eval(&self, at: Frame) -> Option<f32> {
        let keys = &self.keys;
        let first = keys.first()?;
        if at <= first.at {
            return Some(first.value);
        }
        let i = keys.partition_point(|k| k.at <= at);
        let a = keys[i - 1];
        let Some(b) = keys.get(i) else { return Some(a.value) };
        let t = (at - a.at) as f32 / (b.at - a.at) as f32;
        Some(a.value + (b.value - a.value) * a.ease.apply(t))
    }

    pub fn key_at(&self, at: Frame) -> Option<&Keyframe> {
        self.keys.iter().find(|k| k.at == at)
    }

    /// Crea o actualiza el keyframe en `at`.
    pub fn set(&mut self, at: Frame, value: f32) {
        match self.keys.iter_mut().find(|k| k.at == at) {
            Some(k) => k.value = value,
            None => {
                let pos = self.keys.partition_point(|k| k.at < at);
                // El nuevo keyframe hereda la curva del anterior.
                let ease = pos.checked_sub(1).map(|p| self.keys[p].ease).unwrap_or_default();
                self.keys.insert(pos, Keyframe { at, value, ease });
            }
        }
    }

    pub fn remove(&mut self, at: Frame) -> bool {
        let n = self.keys.len();
        self.keys.retain(|k| k.at != at);
        self.keys.len() != n
    }
}

/// Propiedades animables de un clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Prop {
    X,
    Y,
    Scale,
    Rotation,
    Opacity,
    /// Volumen (dB), solo audio.
    Volume,
}

impl Prop {
    pub const TRANSFORM: [Prop; 5] = [Prop::X, Prop::Y, Prop::Scale, Prop::Rotation, Prop::Opacity];
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Animation {
    pub x: Curve,
    pub y: Curve,
    pub scale: Curve,
    pub rotation: Curve,
    pub opacity: Curve,
    pub volume: Curve,
}

impl Animation {
    pub fn curve(&self, p: Prop) -> &Curve {
        match p {
            Prop::X => &self.x,
            Prop::Y => &self.y,
            Prop::Scale => &self.scale,
            Prop::Rotation => &self.rotation,
            Prop::Opacity => &self.opacity,
            Prop::Volume => &self.volume,
        }
    }

    pub fn curve_mut(&mut self, p: Prop) -> &mut Curve {
        match p {
            Prop::X => &mut self.x,
            Prop::Y => &mut self.y,
            Prop::Scale => &mut self.scale,
            Prop::Rotation => &mut self.rotation,
            Prop::Opacity => &mut self.opacity,
            Prop::Volume => &mut self.volume,
        }
    }

    pub fn is_empty(&self) -> bool {
        [Prop::X, Prop::Y, Prop::Scale, Prop::Rotation, Prop::Opacity, Prop::Volume]
            .iter()
            .all(|p| !self.curve(*p).is_animated())
    }

    /// Todos los instantes con keyframe (sin repetir, ordenados).
    pub fn key_times(&self) -> Vec<Frame> {
        let mut v: Vec<Frame> = [Prop::X, Prop::Y, Prop::Scale, Prop::Rotation, Prop::Opacity, Prop::Volume]
            .iter()
            .flat_map(|p| self.curve(*p).keys.iter().map(|k| k.at))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// Corrección de color de un clip de video. Los valores neutros no cambian la imagen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorGrade {
    /// Pasos de exposición (−3 … +3).
    pub exposure: f32,
    /// 1.0 = neutro.
    pub contrast: f32,
    /// 1.0 = neutro, 0.0 = blanco y negro.
    pub saturation: f32,
    /// −1 (frío) … +1 (cálido).
    pub temperature: f32,
    /// −1 (verde) … +1 (magenta).
    pub tint: f32,
    /// LUT 3D en formato `.cube`.
    pub lut: Option<PathBuf>,
    /// Intensidad de la LUT (0 – 1).
    pub lut_mix: f32,
}

impl Default for ColorGrade {
    fn default() -> Self {
        Self { exposure: 0.0, contrast: 1.0, saturation: 1.0, temperature: 0.0, tint: 0.0, lut: None, lut_mix: 1.0 }
    }
}

impl ColorGrade {
    pub fn is_neutral(&self) -> bool {
        let d = Self::default();
        self.exposure == d.exposure
            && self.contrast == d.contrast
            && self.saturation == d.saturation
            && self.temperature == d.temperature
            && self.tint == d.tint
            && (self.lut.is_none() || self.lut_mix <= 0.0)
    }
}

pub fn db_to_gain(db: f32) -> f32 {
    if db <= -60.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Rampa 0→1 a lo largo de `len` cuadros que empiezan en `from` (sin rampa: siempre 1).
fn ramp(frame: f64, from: f64, len: f64) -> f32 {
    if len <= 0.0 {
        1.0
    } else {
        ((frame - from) / len).clamp(0.0, 1.0) as f32
    }
}

/// Rampa 1→0 a lo largo de `len` cuadros que terminan en `to` (sin rampa: siempre 1).
fn ramp_down(frame: f64, to: f64, len: f64) -> f32 {
    if len <= 0.0 {
        1.0
    } else {
        1.0 - ramp(frame, to - len, len)
    }
}

impl Clip {
    /// Cuadro de la fuente que corresponde a un cuadro del timeline.
    pub fn source_frame(&self, frame: Frame) -> Frame {
        self.source_in + self.src_offset(frame - self.start)
    }

    /// Cuadro del timeline que corresponde a un cuadro de la fuente.
    pub fn timeline_frame(&self, source: Frame) -> Frame {
        self.start + ((source - self.source_in) as f64 / self.speed).round() as Frame
    }

    /// Valor de una propiedad en un cuadro del timeline (con keyframes, si los hay).
    pub fn prop_at(&self, p: Prop, frame: Frame) -> f32 {
        if let Some(v) = self.anim.curve(p).eval(self.source_frame(frame)) {
            return v;
        }
        let t = &self.transform;
        match p {
            Prop::X => t.x,
            Prop::Y => t.y,
            Prop::Scale => t.scale,
            Prop::Rotation => t.rotation,
            Prop::Opacity => t.opacity,
            Prop::Volume => self.volume_db,
        }
    }

    /// Cambia una propiedad en un cuadro: si está animada crea/actualiza el keyframe,
    /// si no, cambia el valor fijo.
    pub fn set_prop(&mut self, p: Prop, frame: Frame, value: f32) {
        let at = self.source_frame(frame);
        let curve = self.anim.curve_mut(p);
        if curve.is_animated() {
            curve.set(at, value);
            return;
        }
        let t = &mut self.transform;
        match p {
            Prop::X => t.x = value,
            Prop::Y => t.y = value,
            Prop::Scale => t.scale = value,
            Prop::Rotation => t.rotation = value,
            Prop::Opacity => t.opacity = value,
            Prop::Volume => self.volume_db = value,
        }
    }

    /// Transformación evaluada en un cuadro del timeline (sin fundidos).
    pub fn transform_at(&self, frame: Frame) -> Transform {
        Transform {
            x: self.prop_at(Prop::X, frame),
            y: self.prop_at(Prop::Y, frame),
            scale: self.prop_at(Prop::Scale, frame).max(0.0),
            rotation: self.prop_at(Prop::Rotation, frame),
            opacity: self.prop_at(Prop::Opacity, frame).clamp(0.0, 1.0),
        }
    }

    /// Multiplicador de los fundidos de entrada/salida en el instante `t` (en cuadros, fraccionario).
    /// En audio `t` es continuo; en video se pasa el número de cuadro, de modo que
    /// el primer cuadro de un fundido de entrada y el último de uno de salida son negros.
    pub fn fade_at(&self, t: f64, video: bool) -> f32 {
        let fin = ramp(t, self.start as f64, self.fade_in as f64);
        let t_out = if video { t + 1.0 } else { t };
        let fout = ramp_down(t_out, self.end() as f64, self.fade_out as f64);
        fin * fout
    }

    /// Mayor escala que alcanza el clip (para decidir a qué resolución decodificar).
    pub fn max_scale(&self) -> f32 {
        let keys = self.anim.scale.keys.iter().map(|k| k.value);
        keys.fold(self.transform.scale, f32::max)
    }
}

/// Un clip tal como se reproduce: incluye la extensión de las transiciones.
#[derive(Clone, Copy, Debug)]
pub struct Span<'a> {
    pub track: TrackRef,
    pub clip: &'a Clip,
    /// Rango del timeline en que el clip suena o se ve (puede exceder el clip por las transiciones).
    pub start: Frame,
    pub end: Frame,
    /// Cuadro de la fuente que corresponde a `start`.
    pub source_in: Frame,
    /// Duración de la transición con el clip anterior (empieza en `start`).
    pub trans_in: Frame,
    /// Duración de la transición con el siguiente (termina en `end`).
    pub trans_out: Frame,
}

impl Span<'_> {
    pub fn contains(&self, frame: Frame) -> bool {
        self.start <= frame && frame < self.end
    }

    /// Opacidad o ganancia por la transición de entrada en el instante `t` (cuadros).
    /// Se mide al centro del cuadro, para que ningún cuadro de la transición sea 0 ni 1 exactos.
    pub fn trans_in_at(&self, t: f64) -> f32 {
        ramp(t + 0.5, self.start as f64, self.trans_in as f64)
    }

    /// Multiplicador de la transición de salida (solo se usa en audio: en video
    /// el clip saliente queda debajo, opaco, mientras el entrante aparece encima).
    pub fn trans_out_at(&self, t: f64) -> f32 {
        ramp_down(t + 0.5, self.end as f64, self.trans_out as f64)
    }
}

impl Timeline {
    /// Cuadros de fuente disponibles después del final del clip.
    fn tail_handle(&self, c: &Clip) -> Frame {
        if self.is_unbounded(c) {
            Frame::MAX / 4
        } else {
            c.tl_offset(self.asset_frames(c.asset) - c.source_end()).max(0)
        }
    }

    /// Cuadros de fuente disponibles antes del inicio del clip.
    fn head_handle(&self, c: &Clip) -> Frame {
        if self.is_unbounded(c) {
            Frame::MAX / 4
        } else {
            c.tl_offset(c.source_in).max(0)
        }
    }

    /// Duración real (cuadros antes y después del corte) de la transición de entrada
    /// del clip `i` de una pista. Se limita al material sobrante de ambos clips.
    pub fn transition_split(&self, r: TrackRef, i: usize) -> Option<(Frame, Frame)> {
        let clips = &self.track(r).clips;
        let b = clips.get(i)?;
        if b.transition_in <= 0 || i == 0 {
            return None;
        }
        let a = &clips[i - 1];
        if a.end() != b.start {
            return None;
        }
        let d = b.transition_in.min(a.len).min(b.len);
        let (head, tail) = (self.head_handle(b), self.tail_handle(a));
        let mut before = (d / 2).min(head);
        let after = (d - before).min(tail);
        before = (d - after).min(head);
        (before + after > 0).then_some((before, after))
    }

    /// Clips de una pista con sus rangos de reproducción (incluidas las transiciones).
    pub fn spans(&self, r: TrackRef) -> Vec<Span<'_>> {
        let clips = &self.track(r).clips;
        let mut out: Vec<Span> = clips
            .iter()
            .map(|c| Span { track: r, clip: c, start: c.start, end: c.end(), source_in: c.source_in, trans_in: 0, trans_out: 0 })
            .collect();
        for i in 1..clips.len() {
            if let Some((before, after)) = self.transition_split(r, i) {
                let d = before + after;
                let b = &mut out[i];
                b.start -= before;
                b.source_in -= b.clip.src_offset(before);
                b.trans_in = d;
                let a = &mut out[i - 1];
                a.end += after;
                a.trans_out = d;
            }
        }
        out
    }

    /// Todos los spans de las pistas no silenciadas de un tipo.
    pub fn active_spans(&self, kind: TrackKind) -> Vec<Span<'_>> {
        (0..self.tracks(kind).len())
            .filter(|i| !self.tracks(kind)[*i].muted)
            .flat_map(|i| self.spans(TrackRef { kind, index: i }))
            .collect()
    }

    /// Seguimiento de rostro: reemplaza la posición por la que centra a la persona.
    fn apply_follow(&self, clip: &Clip, frame: Frame, tr: &mut Transform) {
        let Some(asset) = self.asset(clip.asset) else { return };
        let Some(face) = asset.faces.as_ref().and_then(|f| f.at(clip.source_frame(frame) as f64 / self.fps)) else { return };
        let src = (asset.width as f32, asset.height as f32);
        let canvas = (self.width as f32, self.height as f32);
        (tr.x, tr.y) = crate::follow::follow_offset(src, canvas, tr.scale, face, (tr.x, tr.y));
    }

    /// Capas de video visibles en `frame` (de abajo hacia arriba) con su
    /// transformación evaluada; la opacidad ya incluye fundidos y transiciones.
    pub fn video_layers_at(&self, frame: Frame) -> Vec<(Span<'_>, Transform)> {
        let t = frame as f64;
        self.active_spans(TrackKind::Video)
            .into_iter()
            .filter(|s| s.contains(frame))
            .map(|s| {
                let mut tr = s.clip.transform_at(frame);
                if s.clip.follow_face {
                    self.apply_follow(s.clip, frame, &mut tr);
                }
                let fade_t = t.clamp(s.clip.start as f64, (s.clip.end() - 1) as f64);
                tr.opacity *= s.clip.fade_at(fade_t, true) * s.trans_in_at(t);
                (s, tr)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{Asset, AssetId};

    fn tl_with(secs: f64, n: usize) -> Timeline {
        let mut tl = Timeline::new();
        let a = tl.add_asset(Asset {
            id: AssetId(0),
            path: "/m/a.mp4".into(),
            name: "a".into(),
            duration: secs,
            fps: 30.0,
            width: 1920,
            height: 1080,
            has_video: true,
            has_audio: true,
            still: false,
            faces: None,
        });
        for _ in 0..n {
            tl.append_asset(a);
        }
        tl
    }

    #[test]
    fn curve_interpolates_with_easing() {
        let mut c = Curve::default();
        assert_eq!(c.eval(5), None);
        c.set(10, 0.0);
        c.set(20, 100.0);
        c.keys[0].ease = Ease::Linear;
        assert_eq!(c.eval(0), Some(0.0));
        assert_eq!(c.eval(15), Some(50.0));
        assert_eq!(c.eval(30), Some(100.0));
        c.keys[0].ease = Ease::Smooth;
        assert!(c.eval(12).unwrap() < 20.0);
        assert_eq!(c.eval(15), Some(50.0));
        c.keys[0].ease = Ease::Hold;
        assert_eq!(c.eval(19), Some(0.0));
        assert!(c.remove(10));
        assert_eq!(c.eval(0), Some(100.0));
    }

    #[test]
    fn keyframes_stick_to_content_after_split() {
        let mut tl = tl_with(4.0, 1);
        let id = tl.video_tracks[0].clips[0].id;
        let c = tl.clip_mut(id).unwrap();
        c.anim.scale.set(c.source_frame(0), 1.0);
        c.anim.scale.set(c.source_frame(100), 2.0);
        c.anim.scale.keys[0].ease = Ease::Linear;
        let before = c.transform_at(75).scale;
        tl.split(50, None);
        let right = &tl.video_tracks[0].clips[1];
        assert_eq!(right.transform_at(75).scale, before);
        // set_prop sobre una propiedad animada crea un keyframe.
        let right = tl.video_tracks[0].clips[1].id;
        tl.clip_mut(right).unwrap().set_prop(Prop::Scale, 60, 3.0);
        assert_eq!(tl.clip_mut(right).unwrap().anim.scale.keys.len(), 3);
        assert_eq!(tl.find(right).unwrap().1.transform_at(60).scale, 3.0);
    }

    #[test]
    fn fades_ramp_opacity() {
        let mut tl = tl_with(1.0, 1);
        let id = tl.video_tracks[0].clips[0].id;
        let c = tl.clip_mut(id).unwrap();
        c.fade_in = 10;
        c.fade_out = 10;
        let layers = |tl: &Timeline, f| tl.video_layers_at(f)[0].1.opacity;
        assert_eq!(layers(&tl, 0), 0.0);
        assert_eq!(layers(&tl, 5), 0.5);
        assert_eq!(layers(&tl, 15), 1.0);
        assert!((layers(&tl, 24) - 0.5).abs() < 1e-6);
        assert_eq!(layers(&tl, 29), 0.0);
    }

    #[test]
    fn dissolve_uses_handles_on_both_sides() {
        // Un clip de 4 s cortado en dos: hay material de sobra a ambos lados del corte.
        let mut tl = tl_with(4.0, 1);
        tl.split(60, None);
        let b = tl.video_tracks[0].clips[1].id;
        tl.clip_mut(b).unwrap().transition_in = 20;
        let r = TrackRef::video(0);
        assert_eq!(tl.transition_split(r, 1), Some((10, 10)));
        let spans = tl.spans(r);
        assert_eq!((spans[0].start, spans[0].end), (0, 70));
        assert_eq!((spans[1].start, spans[1].end, spans[1].source_in), (50, 120, 50));
        // A mitad de la transición se ven los dos clips; el entrante a ~50 %.
        let layers = tl.video_layers_at(60);
        assert_eq!(layers.len(), 2);
        assert!((layers[1].1.opacity - 0.5).abs() < 0.03);
        assert_eq!(layers[0].1.opacity, 1.0);
        assert_eq!(tl.video_layers_at(49).len(), 1);
        assert_eq!(tl.video_layers_at(70).len(), 1);

        // Sin material antes del inicio (clip completo al principio del archivo):
        // la transición ocurre entera antes del corte, sobre el final del clip anterior.
        let mut tl = tl_with(2.0, 2);
        let b = tl.video_tracks[0].clips[1].id;
        tl.clip_mut(b).unwrap().transition_in = 20;
        assert_eq!(tl.transition_split(r, 1), None);
        let a = tl.video_tracks[0].clips[0].id;
        tl.trim(a, crate::Edge::End, 50);
        assert!(tl.trim(b, crate::Edge::Start, 65));
        assert!(tl.move_clips(&[b], -15, (0, 0)));
        // Ahora A tiene 10 cuadros sobrantes y B tiene 5 antes de su inicio.
        assert_eq!(tl.transition_split(r, 1), Some((5, 10)));
    }

    #[test]
    fn audio_crossfade_is_complementary() {
        let mut tl = tl_with(4.0, 1);
        tl.split(60, None);
        let b = tl.audio_tracks[0].clips[1].id;
        tl.clip_mut(b).unwrap().transition_in = 20;
        let spans = tl.spans(TrackRef::audio(0));
        for f in 50..70 {
            let t = f as f64;
            let sum = spans[0].trans_out_at(t) + spans[1].trans_in_at(t);
            assert!((sum - 1.0).abs() < 1e-5, "{f}: {sum}");
        }
    }

    #[test]
    fn speed_changes_length_and_ripples() {
        let mut tl = tl_with(2.0, 2);
        let first = tl.video_tracks[0].clips[0].id;
        assert!(tl.set_speed(first, 2.0));
        let spans: Vec<_> = tl.video_tracks[0].clips.iter().map(|c| (c.start, c.len)).collect();
        assert_eq!(spans, vec![(0, 30), (30, 60)]);
        assert_eq!(tl.audio_tracks[0].clips[0].len, 30);
        // Cortar a mitad conserva el contenido: el lado derecho empieza en el cuadro 30 de la fuente.
        tl.split(15, None);
        assert_eq!(tl.video_tracks[0].clips[1].source_in, 30);
        // No se puede alargar más allá del archivo: a 2x quedan 15 cuadros de timeline.
        let right = tl.video_tracks[0].clips[1].id;
        assert!(!tl.trim(right, crate::Edge::End, 40));
        // Cámara lenta: el clip dura el doble y empuja al siguiente.
        assert!(tl.set_speed(right, 0.5));
        let spans: Vec<_> = tl.video_tracks[0].clips.iter().map(|c| (c.start, c.len, c.source_in)).collect();
        assert_eq!(spans, vec![(0, 15, 0), (15, 60, 30), (75, 60, 0)]);
        let c = &tl.video_tracks[0].clips[1];
        assert_eq!(c.source_frame(15 + 58), 59);
    }

    #[test]
    fn volume_conversion() {
        assert_eq!(db_to_gain(0.0), 1.0);
        assert!((db_to_gain(-6.0) - 0.501).abs() < 0.01);
        assert_eq!(db_to_gain(-60.0), 0.0);
    }
}
