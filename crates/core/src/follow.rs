//! Seguimiento de rostros al reencuadrar: el recorte de un video que no cabe en el
//! formato (p. ej. 16:9 en 9:16) se desplaza para mantener a la persona en cuadro.

use serde::{Deserialize, Serialize};

use crate::compose::contain;

/// Trayectoria (ya suavizada) del rostro principal de un video.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FaceTrack {
    /// Segundos entre muestras.
    pub interval: f64,
    /// Centro del encuadre por muestra, normalizado (0–1, origen arriba a la izquierda).
    pub path: Vec<[f32; 2]>,
}

impl FaceTrack {
    /// Centro del encuadre en el segundo `secs` del archivo (interpolado).
    pub fn at(&self, secs: f64) -> Option<[f32; 2]> {
        let last = self.path.len().checked_sub(1)?;
        let pos = (secs / self.interval.max(1e-6)).clamp(0.0, last as f64);
        let i = pos.floor() as usize;
        let j = (i + 1).min(last);
        let f = (pos - i as f64) as f32;
        let (a, b) = (self.path[i], self.path[j]);
        Some([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f])
    }
}

/// Altura (desde arriba, fracción del cuadro) donde se coloca el rostro: un poco por
/// encima del centro, como se encuadra a una persona.
const EYE_LINE: f32 = 0.42;

/// Desplazamiento (x, y) de un video de `src` px en `canvas` con escala `scale` para
/// centrar `face`. En un eje sin sobrante (el video no se recorta) se deja `current`.
pub fn follow_offset(src: (f32, f32), canvas: (f32, f32), scale: f32, face: [f32; 2], current: (f32, f32)) -> (f32, f32) {
    let base = contain(src, canvas);
    let visual = (base.0 * scale, base.1 * scale);
    let axis = |size: f32, canvas: f32, f: f32, target: f32, cur: f32| {
        let spare = (size - canvas) / 2.0;
        if spare < 0.5 {
            return cur;
        }
        // Posición del rostro respecto al centro del video → mover el video al revés.
        let want = (target - 0.5) * canvas - (f - 0.5) * size;
        want.clamp(-spare, spare)
    };
    (axis(visual.0, canvas.0, face[0], 0.5, current.0), axis(visual.1, canvas.1, face[1], EYE_LINE, current.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_track() {
        let t = FaceTrack { interval: 0.5, path: vec![[0.2, 0.5], [0.4, 0.5], [0.4, 0.3]] };
        assert_eq!(t.at(0.25), Some([0.3, 0.5]));
        assert_eq!(t.at(-1.0), Some([0.2, 0.5]));
        assert_eq!(t.at(99.0), Some([0.4, 0.3]));
        assert_eq!(FaceTrack { interval: 0.5, path: vec![] }.at(1.0), None);
    }

    #[test]
    fn keeps_face_in_vertical_crop() {
        // 16:9 llenando un 9:16 de 1080×1920: el video mide 3413×1920, sobran 1166 px por lado.
        let src = (1920.0, 1080.0);
        let canvas = (1080.0, 1920.0);
        let cover = crate::compose::cover_scale(src, canvas);
        // Persona a la izquierda (25 %): el video se corre a la derecha para centrarla.
        let (x, y) = follow_offset(src, canvas, cover, [0.25, 0.4], (0.0, 0.0));
        let visual_w = 1080.0 * cover; // contain = 1080 de ancho, × cover
        let face_x = 540.0 + x + (0.25 - 0.5) * visual_w;
        assert!((face_x - 540.0).abs() < 1.0, "rostro centrado: {face_x}");
        assert_eq!(y, 0.0, "en vertical no sobra alto: no se toca");
        // En el borde no se sale del video (sin franjas negras).
        let (x, _) = follow_offset(src, canvas, cover, [0.0, 0.4], (0.0, 0.0));
        assert!((x - (visual_w - 1080.0) / 2.0).abs() < 1.0);
        // Si el video no se recorta, se respeta la posición del usuario.
        assert_eq!(follow_offset(src, (1920.0, 1080.0), 1.0, [0.1, 0.1], (12.0, -5.0)), (12.0, -5.0));
    }
}
