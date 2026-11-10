//! Del montón de rostros por cuadro a una trayectoria de cámara suave.

use crate::Face;

/// Rostro principal en una muestra: centro normalizado y tamaño (alto relativo).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceSample {
    pub x: f32,
    pub y: f32,
    pub size: f32,
}

/// Elige el rostro principal: el más grande, pero prefiriendo seguir al de la muestra
/// anterior (para no saltar entre dos personas a cada rato).
pub fn main_face(faces: &[Face], previous: Option<FaceSample>) -> Option<FaceSample> {
    faces
        .iter()
        .filter(|f| f.confidence >= 0.5 && f.w > 0.0 && f.h > 0.0)
        .map(|f| {
            let (x, y) = f.center();
            let mut score = f.w * f.h;
            if let Some(p) = previous {
                let d = ((x - p.x).powi(2) + (y - p.y).powi(2)).sqrt();
                // Cerca del anterior: bonificación equivalente a un rostro un 50 % más grande.
                if d < 0.15 {
                    score *= 1.5;
                }
            }
            (score, FaceSample { x, y, size: f.h })
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| s)
}

/// Trayectoria suave del encuadre (centro normalizado por muestra) a partir del rostro
/// principal de cada muestra, tomadas cada `interval` segundos.
///
/// - Los huecos (nadie en cuadro) mantienen la última posición.
/// - Promedio móvil centrado de ~1 s: sin temblores.
/// - Zona muerta: movimientos pequeños no mueven la cámara (como un operador de cámara).
///
/// `None` si no hay ningún rostro en todo el video.
pub fn smooth_path(samples: &[Option<FaceSample>], interval: f64) -> Option<Vec<[f32; 2]>> {
    let first = samples.iter().flatten().next()?;
    let mut last = [first.x, first.y];
    let filled: Vec<[f32; 2]> = samples
        .iter()
        .map(|s| {
            if let Some(s) = s {
                last = [s.x, s.y];
            }
            last
        })
        .collect();

    let half = ((0.5 / interval.max(1e-3)).round() as usize).max(1);
    let averaged: Vec<[f32; 2]> = (0..filled.len())
        .map(|i| {
            // Ventana simétrica (más corta en los bordes) para no atrasarse al principio ni al final.
            let h = half.min(i).min(filled.len() - 1 - i);
            let (a, b) = (i - h, i + h + 1);
            let n = (b - a) as f32;
            let sum = filled[a..b].iter().fold([0.0, 0.0], |s, p| [s[0] + p[0], s[1] + p[1]]);
            [sum[0] / n, sum[1] / n]
        })
        .collect();

    // Zona muerta: absorbe el temblor que queda tras el promedio. Pequeña, porque al
    // pasar a vertical el recorte amplía cualquier desfase (~3× en 16:9 → 9:16).
    const DEAD: f32 = 0.008;
    let mut cam = averaged[0];
    let mut out = Vec::with_capacity(averaged.len());
    for p in averaged {
        for k in 0..2 {
            let d = p[k] - cam[k];
            if d.abs() > DEAD {
                cam[k] += d - DEAD * d.signum();
            }
        }
        out.push(cam);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x: f32, y: f32, s: f32) -> Face {
        Face { x: x - s / 2.0, y: y - s / 2.0, w: s, h: s, confidence: 0.9 }
    }

    #[test]
    fn prefers_largest_but_sticks_to_previous() {
        let faces = [face(0.2, 0.5, 0.2), face(0.8, 0.5, 0.22)];
        assert!((main_face(&faces, None).unwrap().x - 0.8).abs() < 1e-5, "el más grande");
        let prev = FaceSample { x: 0.21, y: 0.5, size: 0.2 };
        assert!((main_face(&faces, Some(prev)).unwrap().x - 0.2).abs() < 1e-5, "sigue al mismo");
        assert!(main_face(&[Face { confidence: 0.1, ..face(0.5, 0.5, 0.3) }], None).is_none());
    }

    #[test]
    fn smooths_jitter_fills_gaps_and_follows_moves() {
        assert!(smooth_path(&[None, None], 0.2).is_none());
        let s = |x: f32| Some(FaceSample { x, y: 0.4, size: 0.2 });
        // Hueco al principio, temblor alrededor de 0,3 y luego la persona se mueve a 0,7.
        let mut samples = vec![None, None];
        for i in 0..20 {
            samples.push(s(0.3 + if i % 2 == 0 { 0.01 } else { -0.01 }));
        }
        samples.push(None);
        for _ in 0..20 {
            samples.push(s(0.7));
        }
        let path = smooth_path(&samples, 0.2).unwrap();
        assert_eq!(path.len(), samples.len());
        assert!((path[0][0] - 0.3).abs() < 0.02, "el hueco inicial toma el primer rostro");
        // Durante el temblor la cámara casi no se mueve.
        let still = &path[4..18];
        let (lo, hi) = still.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
        assert!(hi - lo < 0.01, "{still:?}");
        // Se desplaza de forma gradual (sin saltos grandes) y llega cerca de 0,7.
        assert!(path.windows(2).all(|w| (w[1][0] - w[0][0]).abs() < 0.12));
        assert!((path.last().unwrap()[0] - 0.7).abs() <= 0.009);
    }
}
