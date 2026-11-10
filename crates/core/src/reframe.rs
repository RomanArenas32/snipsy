//! Reencuadre automático: adapta un timeline a otro formato (16:9 → 9:16, 1:1…).
//!
//! - Los videos que llenaban la pantalla la siguen llenando (recortando lo que sobra),
//!   con el mismo zoom relativo.
//! - Textos, logos y demás elementos conservan su tamaño respecto al lado corto de
//!   la pantalla y su distancia al borde más cercano (un logo en una esquina sigue en
//!   esa esquina). Lo que estaba cerca del centro mantiene su posición proporcional.

use crate::compose::{contain, cover_scale};
use crate::effects::Curve;
use crate::{Clip, Timeline};

/// Copia de `tl` adaptada a un lienzo de `width × height`. Con `follow_faces`, los
/// videos que quedan recortados siguen al rostro principal (ver [`crate::follow`]).
pub fn reframe(tl: &Timeline, width: u32, height: u32, follow_faces: bool) -> Timeline {
    let mut out = tl.clone();
    out.width = width;
    out.height = height;
    if (tl.width, tl.height) == (width, height) {
        return out;
    }
    let old = (tl.width as f32, tl.height as f32);
    let new = (width as f32, height as f32);
    let k = new.0.min(new.1) / old.0.min(old.1);
    let sources: Vec<_> = tl.assets.iter().map(|a| (a.id, (a.width as f32, a.height as f32))).collect();
    for track in &mut out.video_tracks {
        for clip in &mut track.clips {
            let src = sources.iter().find(|(id, _)| *id == clip.asset).map(|(_, s)| *s);
            let cropped = reframe_clip(clip, src, old, new, k);
            if follow_faces && cropped {
                clip.follow_face = true;
            }
        }
    }
    out
}

/// Posición en un eje: cerca del centro, proporcional; si no, misma distancia (escalada) al borde.
fn map_pos(v: f32, half_old: f32, half_new: f32, k: f32) -> f32 {
    if v.abs() <= half_old / 3.0 {
        v * half_new / half_old
    } else {
        v.signum() * (half_new - (half_old - v.abs()) * k)
    }
}

fn map_curve(curve: &mut Curve, f: impl Fn(f32) -> f32) {
    for key in &mut curve.keys {
        key.value = f(key.value);
    }
}

/// Devuelve `true` si es un video que llena la pantalla y queda recortado en el formato nuevo.
fn reframe_clip(clip: &mut Clip, src: Option<(f32, f32)>, old: (f32, f32), new: (f32, f32), k: f32) -> bool {
    let (hx_old, hy_old, hx_new, hy_new) = (old.0 / 2.0, old.1 / 2.0, new.0 / 2.0, new.1 / 2.0);
    clip.transform.x = map_pos(clip.transform.x, hx_old, hx_new, k);
    clip.transform.y = map_pos(clip.transform.y, hy_old, hy_new, k);
    map_curve(&mut clip.anim.x, |v| map_pos(v, hx_old, hx_new, k));
    map_curve(&mut clip.anim.y, |v| map_pos(v, hy_old, hy_new, k));

    let mut cropped = false;
    let factor = if let Some(style) = clip.text.as_mut() {
        // El texto se rasteriza a `size`: basta con escalar la fuente.
        style.size = (style.size * k).max(1.0);
        1.0
    } else if let Some(src) = src.filter(|s| s.0 > 0.0 && s.1 > 0.0) {
        let min_scale = if clip.anim.scale.is_animated() {
            clip.anim.scale.keys.iter().map(|k| k.value).fold(f32::INFINITY, f32::min)
        } else {
            clip.transform.scale
        };
        let cover_old = cover_scale(src, old);
        if min_scale >= cover_old * 0.98 {
            // Llenaba la pantalla: que la siga llenando con el mismo zoom relativo.
            let f = cover_scale(src, new) / cover_old;
            let visual = contain(src, new);
            cropped = visual.0 * min_scale * f > new.0 + 1.0 || visual.1 * min_scale * f > new.1 + 1.0;
            f
        } else {
            // Elemento más chico (logo, video dentro de otro): mismo tamaño relativo al lado corto.
            k * contain(src, old).0 / contain(src, new).0.max(1e-3)
        }
    } else {
        1.0
    };
    clip.transform.scale *= factor;
    map_curve(&mut clip.anim.scale, |v| v * factor);
    cropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{self, TextStyle};
    use crate::{Asset, AssetId, BrandKit, Corner, Prop};

    fn asset(tl: &mut Timeline, name: &str, w: u32, h: u32, still: bool) -> AssetId {
        tl.add_asset(Asset {
            id: AssetId(0),
            path: format!("/m/{name}").into(),
            name: name.into(),
            duration: 10.0,
            fps: 30.0,
            width: w,
            height: h,
            has_video: true,
            has_audio: false,
            still,
            faces: None,
        })
    }

    #[test]
    fn horizontal_ad_to_vertical() {
        let mut tl = Timeline::new();
        let video = asset(&mut tl, "v.mp4", 1920, 1080, false);
        assert_eq!((tl.width, tl.height), (1920, 1080));
        tl.insert_asset(video, 0, 0, 0);
        // Ken Burns: zoom de 1,0 a 1,2.
        let vid = tl.video_tracks[0].clips[0].id;
        let c = tl.clip_mut(vid).unwrap();
        c.anim.scale.set(0, 1.0);
        c.anim.scale.set(200, 1.2);
        let title = tl.insert_text(0, 1, 90, TextStyle { size: 100.0, ..Default::default() });
        tl.clip_mut(title).unwrap().transform.y = -100.0;
        let logo = asset(&mut tl, "logo.png", 400, 200, true);
        let kit = BrandKit { logo_corner: Corner::TopRight, ..Default::default() };
        let logo_id = kit.place_logo(&mut tl, logo).unwrap();

        let v = reframe(&tl, 1080, 1920, false);
        assert_eq!((v.width, v.height), (1080, 1920));
        assert_eq!((tl.width, tl.height), (1920, 1080), "el original no cambia");

        // El video llena el vertical (cover de 16:9 en 9:16 = ×3,16) y conserva el zoom.
        let c = &v.video_tracks[0].clips[0];
        let cover = compose::cover_scale((1920.0, 1080.0), (1080.0, 1920.0));
        assert!((c.prop_at(Prop::Scale, 0) - cover).abs() < 1e-3);
        assert!((c.prop_at(Prop::Scale, 200) - cover * 1.2).abs() < 1e-3);

        // Título: mismo tamaño (lado corto igual) y cerca del centro: posición proporcional.
        let t = v.find(title).unwrap().1;
        assert_eq!(t.text.as_ref().unwrap().size, 100.0);
        assert!((t.transform.y - (-100.0 * 1920.0 / 1080.0)).abs() < 1e-3);

        // El logo queda igual que si se hubiera colocado directamente en el vertical.
        let placed = v.find(logo_id).unwrap().1.transform;
        let mut direct = Timeline::new();
        direct.width = 1080;
        direct.height = 1920;
        let expected = kit.logo_transform((400, 200), &direct);
        assert!((placed.x - expected.x).abs() < 0.01 && (placed.y - expected.y).abs() < 0.01, "{placed:?} vs {expected:?}");
        assert!((placed.scale - expected.scale).abs() < 1e-4, "{placed:?} vs {expected:?}");

        assert!(!c.follow_face, "sin seguimiento si no se pide");
        let followed = reframe(&tl, 1080, 1920, true);
        assert!(followed.video_tracks[0].clips[0].follow_face, "el video recortado sigue al rostro");
        assert!(!followed.find(logo_id).unwrap().1.follow_face, "el logo no");

        // Y de vuelta a 16:9 se recupera lo original.
        let back = reframe(&v, 1920, 1080, false);
        let c = &back.video_tracks[0].clips[0];
        assert!((c.prop_at(Prop::Scale, 200) - 1.2).abs() < 1e-3);
        assert!((back.find(title).unwrap().1.transform.y + 100.0).abs() < 1e-3);
        let l = back.find(logo_id).unwrap().1.transform;
        let orig = tl.find(logo_id).unwrap().1.transform;
        assert!((l.x - orig.x).abs() < 0.01 && (l.y - orig.y).abs() < 0.01 && (l.scale - orig.scale).abs() < 1e-4);
    }

    #[test]
    fn square_scales_text_and_keeps_bottom_captions_near_bottom() {
        let mut tl = Timeline::new();
        tl.width = 1080;
        tl.height = 1920;
        // Subtítulo clásico pegado abajo (a 100 px del borde).
        let id = tl.insert_text(0, 0, 30, TextStyle { size: 60.0, ..Default::default() });
        tl.clip_mut(id).unwrap().transform.y = 960.0 - 100.0;
        let sq = reframe(&tl, 1080, 1080, false);
        let c = sq.find(id).unwrap().1;
        assert_eq!(c.text.as_ref().unwrap().size, 60.0);
        assert!((c.transform.y - (540.0 - 100.0)).abs() < 1e-3, "{}", c.transform.y);
        // 4K: todo al doble.
        let big = reframe(&sq, 2160, 2160, false);
        assert_eq!(big.find(id).unwrap().1.text.as_ref().unwrap().size, 120.0);
    }
}
