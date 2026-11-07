//! Composición: transformaciones de capas, estilos de texto, formatos y geometría.
//!
//! Las coordenadas están en **píxeles del proyecto**, con origen en la esquina
//! superior izquierda del lienzo y el eje Y hacia abajo. La posición (`x`, `y`)
//! de una capa es el desplazamiento de su centro respecto al centro del lienzo.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transform {
    pub x: f32,
    pub y: f32,
    /// 1.0 = el video se ajusta dentro del lienzo (o el texto a su tamaño natural).
    pub scale: f32,
    /// Grados, sentido horario.
    pub rotation: f32,
    /// 0.0 – 1.0
    pub opacity: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, scale: 1.0, rotation: 0.0, opacity: 1.0 }
    }
}

/// Estilo de un clip de texto. Colores en RGBA (sRGB, alfa no premultiplicado).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    pub text: String,
    /// Tamaño de la fuente en píxeles del proyecto.
    pub size: f32,
    pub bold: bool,
    /// Familia tipográfica (`None` = la sans serif del sistema).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Archivo de la fuente (.ttf / .otf) cuando no está instalada en el sistema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_file: Option<std::path::PathBuf>,
    pub color: [u8; 4],
    pub outline: Option<[u8; 4]>,
    pub background: Option<[u8; 4]>,
    /// Animación palabra a palabra (subtítulos de reels).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<WordAnimation>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WordMode {
    /// Todas las palabras visibles; la que se está diciendo cambia de color.
    #[default]
    Highlight,
    /// Las palabras aparecen a medida que se dicen (la actual, resaltada).
    Reveal,
}

impl WordMode {
    pub const ALL: [WordMode; 2] = [WordMode::Highlight, WordMode::Reveal];

    pub fn label(self) -> &'static str {
        match self {
            WordMode::Highlight => "Resaltar la palabra que se dice",
            WordMode::Reveal => "Aparecer palabra a palabra",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WordAnimation {
    pub mode: WordMode,
    /// Color de la palabra actual.
    pub color: [u8; 4],
    /// Inicio de cada palabra del texto, en cuadros de la fuente del clip (como los keyframes).
    /// Si no coincide con el número de palabras (p. ej. tras editar el texto), se reparten parejas.
    pub starts: Vec<i64>,
}

impl Default for WordAnimation {
    fn default() -> Self {
        Self { mode: WordMode::Highlight, color: [255, 214, 10, 255], starts: Vec::new() }
    }
}

/// Estado de cada palabra en un instante.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WordState {
    Normal,
    Active,
    Hidden,
}

impl WordAnimation {
    /// Índice de la palabra que se está diciendo en el cuadro de fuente `at`, para
    /// un clip que ocupa `[source_in, source_in + len)` y tiene `count` palabras.
    pub fn active(&self, at: i64, source_in: i64, len: i64, count: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }
        let start = |i: usize| -> i64 {
            if self.starts.len() == count {
                self.starts[i]
            } else {
                source_in + len * i as i64 / count as i64
            }
        };
        (0..count).rev().find(|i| start(*i) <= at)
    }

    pub fn states(&self, active: Option<usize>, count: usize) -> Vec<WordState> {
        (0..count)
            .map(|i| match active {
                Some(a) if i == a => WordState::Active,
                Some(a) if i > a && self.mode == WordMode::Reveal => WordState::Hidden,
                None if self.mode == WordMode::Reveal => WordState::Hidden,
                _ => WordState::Normal,
            })
            .collect()
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            text: "Tu texto aquí".into(),
            size: 96.0,
            bold: true,
            font: None,
            font_file: None,
            color: [255, 255, 255, 255],
            outline: Some([0, 0, 0, 255]),
            background: None,
            words: None,
        }
    }
}

/// Formatos de proyecto habituales.
pub const FORMATS: &[(&str, u32, u32)] = &[
    ("16:9 Horizontal · YouTube", 1920, 1080),
    ("9:16 Vertical · Reels / TikTok / Shorts", 1080, 1920),
    ("1:1 Cuadrado", 1080, 1080),
    ("4:5 Feed de Instagram", 1080, 1350),
    ("16:9 4K", 3840, 2160),
];

/// Tamaño de `src` ajustado (sin recortar) dentro de `canvas`.
pub fn contain(src: (f32, f32), canvas: (f32, f32)) -> (f32, f32) {
    if src.0 <= 0.0 || src.1 <= 0.0 {
        return (0.0, 0.0);
    }
    let s = (canvas.0 / src.0).min(canvas.1 / src.1);
    (src.0 * s, src.1 * s)
}

/// Escala (relativa a `contain`) para que `src` cubra todo el lienzo, recortando.
pub fn cover_scale(src: (f32, f32), canvas: (f32, f32)) -> f32 {
    if src.0 <= 0.0 || src.1 <= 0.0 {
        return 1.0;
    }
    let contain = (canvas.0 / src.0).min(canvas.1 / src.1);
    let cover = (canvas.0 / src.0).max(canvas.1 / src.1);
    cover / contain
}

fn center(t: &Transform, canvas: (f32, f32)) -> (f32, f32) {
    (canvas.0 / 2.0 + t.x, canvas.1 / 2.0 + t.y)
}

/// Esquinas de una capa de tamaño base `base` (sup-izq, sup-der, inf-der, inf-izq).
pub fn corners(t: &Transform, base: (f32, f32), canvas: (f32, f32)) -> [[f32; 2]; 4] {
    let (cx, cy) = center(t, canvas);
    let (hw, hh) = (base.0 * t.scale / 2.0, base.1 * t.scale / 2.0);
    let (sin, cos) = t.rotation.to_radians().sin_cos();
    [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map(|(dx, dy)| [cx + dx * cos - dy * sin, cy + dx * sin + dy * cos])
}

/// ¿El punto `p` cae dentro de la capa?
pub fn contains(t: &Transform, base: (f32, f32), canvas: (f32, f32), p: (f32, f32)) -> bool {
    let (cx, cy) = center(t, canvas);
    let (dx, dy) = (p.0 - cx, p.1 - cy);
    let (sin, cos) = t.rotation.to_radians().sin_cos();
    let (lx, ly) = (dx * cos + dy * sin, -dx * sin + dy * cos);
    lx.abs() <= base.0 * t.scale / 2.0 && ly.abs() <= base.1 * t.scale / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_and_cover_for_vertical_canvas() {
        let canvas = (1080.0, 1920.0);
        assert_eq!(contain((1920.0, 1080.0), canvas), (1080.0, 607.5));
        let s = cover_scale((1920.0, 1080.0), canvas);
        assert!((607.5 * s - 1920.0).abs() < 0.01);
    }

    #[test]
    fn corners_follow_transform() {
        let t = Transform { x: 10.0, y: -20.0, scale: 2.0, ..Default::default() };
        let c = corners(&t, (100.0, 50.0), (1000.0, 1000.0));
        assert_eq!(c[0], [410.0, 430.0]);
        assert_eq!(c[2], [610.0, 530.0]);

        let r = Transform { rotation: 90.0, ..Default::default() };
        let c = corners(&r, (100.0, 50.0), (1000.0, 1000.0));
        assert!((c[0][0] - 525.0).abs() < 1e-3 && (c[0][1] - 450.0).abs() < 1e-3);
    }

    #[test]
    fn word_animation_states() {
        let w = WordAnimation { starts: vec![10, 20, 30], ..Default::default() };
        assert_eq!(w.active(5, 0, 40, 3), None);
        assert_eq!(w.active(25, 0, 40, 3), Some(1));
        assert_eq!(w.states(Some(1), 3), vec![WordState::Normal, WordState::Active, WordState::Normal]);
        let r = WordAnimation { mode: WordMode::Reveal, ..w.clone() };
        assert_eq!(r.states(Some(1), 3), vec![WordState::Normal, WordState::Active, WordState::Hidden]);
        // Texto editado (4 palabras): se reparten parejas en el clip.
        assert_eq!(w.active(25, 0, 40, 4), Some(2));
    }

    #[test]
    fn hit_test_respects_rotation() {
        let t = Transform { rotation: 90.0, ..Default::default() };
        // Tras rotar 90°, la capa de 200x20 queda vertical.
        assert!(contains(&t, (200.0, 20.0), (1000.0, 1000.0), (500.0, 590.0)));
        assert!(!contains(&t, (200.0, 20.0), (1000.0, 1000.0), (590.0, 500.0)));
    }
}
