//! Escena: qué capas se ven en un cuadro del timeline y dónde.
//!
//! La usan el visor (vista previa) y la exportación, para que ambos produzcan
//! exactamente la misma imagen.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use snipsy_core::compose::{self, Transform};
use snipsy_core::{ClipId, ColorGrade, Frame, TextStyle, Timeline, WordState};

use crate::{Grade, Layer, LayerSource, Lut, LutSource, TextParams, TextRaster, TextRenderer};

/// Imagen decodificada de un clip (RGBA compacto).
#[derive(Clone)]
pub struct RgbaImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Arc<Vec<u8>>,
}

#[derive(Clone, PartialEq)]
struct TextKey {
    style: TextStyle,
    /// Color de cada palabra (animación palabra a palabra).
    word_colors: Option<Vec<[u8; 4]>>,
    /// Tamaño de rasterizado en px de salida × 4 (cuantizado).
    px: i32,
}

struct TextEntry {
    key: TextKey,
    raster: Arc<TextRaster>,
    version: u64,
}

enum Source {
    Video(RgbaImage),
    Text(Arc<TextRaster>, u64),
}

/// Una capa visible, de abajo hacia arriba.
pub struct SceneLayer {
    pub clip: ClipId,
    /// Tamaño base en píxeles del proyecto (antes de `transform.scale`).
    pub base: (f32, f32),
    pub transform: Transform,
    source: Source,
    grade: Grade,
    lut: Option<(u64, Arc<Lut>)>,
}

struct LutEntry {
    key: u64,
    lut: Option<Arc<Lut>>,
}

#[derive(Default)]
pub struct Scene {
    text: Option<TextRenderer>,
    cache: HashMap<ClipId, TextEntry>,
    version: u64,
    luts: HashMap<PathBuf, LutEntry>,
    /// Imágenes fijas (logos, fotos) ya decodificadas.
    stills: HashMap<PathBuf, Option<RgbaImage>>,
}

/// Decodifica una imagen (PNG, JPG, WebP) a RGBA.
pub fn load_image(path: &std::path::Path) -> Result<RgbaImage, String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?.into_rgba8();
    let (w, h) = img.dimensions();
    Ok(RgbaImage { width: w as usize, height: h as usize, rgba: Arc::new(img.into_raw()) })
}

fn grade_of(c: &ColorGrade) -> Grade {
    Grade {
        exposure: c.exposure,
        contrast: c.contrast,
        saturation: c.saturation,
        temperature: c.temperature,
        tint: c.tint,
        lut_mix: c.lut_mix,
    }
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    /// Capas visibles en `frame`. `images` tiene la imagen de cada clip de video
    /// (por id); `k` = píxeles de salida por píxel de proyecto (para el texto).
    pub fn layers(&mut self, tl: &Timeline, frame: Frame, images: &HashMap<u64, RgbaImage>, k: f32) -> Vec<SceneLayer> {
        let project = (tl.width as f32, tl.height as f32);
        let mut out = Vec::new();
        let mut alive = HashSet::new();
        for (span, t) in tl.video_layers_at(frame) {
            let clip = span.clip;
            if t.opacity <= 0.0 {
                continue;
            }
            let grade = grade_of(&clip.color);
            let lut = clip.color.lut.as_ref().filter(|_| clip.color.lut_mix > 0.0).and_then(|p| self.lut(p));
            if let Some(style) = &clip.text {
                alive.insert(clip.id);
                // Con escala animada, rasterizar a la mayor escala para no regenerar el texto en cada cuadro.
                let raster_scale = clip.max_scale().max(t.scale).max(0.01);
                // Rasterizar al tamaño final para que el texto se vea nítido a cualquier escala.
                let px = ((style.size * raster_scale * k * 4.0).round() as i32).max(4);
                let word_colors = style.words.as_ref().map(|w| {
                    let count = style.text.split_whitespace().count();
                    let active = w.active(clip.source_frame(frame), clip.source_in, clip.len, count);
                    w.states(active, count)
                        .into_iter()
                        .map(|s| match s {
                            WordState::Normal => style.color,
                            WordState::Active => w.color,
                            WordState::Hidden => [0, 0, 0, 0],
                        })
                        .collect()
                });
                let entry = self.text_entry(clip.id, TextKey { style: style.clone(), word_colors, px });
                let r = &entry.raster;
                let base = (r.width as f32 / (k * raster_scale), r.height as f32 / (k * raster_scale));
                let source = Source::Text(r.clone(), entry.version);
                out.push(SceneLayer { clip: clip.id, base, transform: t, source, grade, lut });
            } else if let Some(img) = images.get(&clip.id.0).cloned().or_else(|| self.still(tl, clip.asset)) {
                let base = compose::contain((img.width as f32, img.height as f32), project);
                out.push(SceneLayer { clip: clip.id, base, transform: t, source: Source::Video(img), grade, lut });
            }
        }
        self.cache.retain(|id, _| alive.contains(id));
        out
    }

    /// Imagen de un medio fijo (con caché). Si no se puede leer se avisa una vez.
    fn still(&mut self, tl: &Timeline, asset: snipsy_core::AssetId) -> Option<RgbaImage> {
        let a = tl.asset(asset).filter(|a| a.still)?;
        self.stills
            .entry(a.path.clone())
            .or_insert_with(|| match load_image(&a.path) {
                Ok(img) => Some(img),
                Err(e) => {
                    log::warn!("imagen ignorada: {e}");
                    None
                }
            })
            .clone()
    }

    /// LUT cargada (con caché). Si el archivo no es válido se avisa una sola vez y se ignora.
    fn lut(&mut self, path: &PathBuf) -> Option<(u64, Arc<Lut>)> {
        let next_key = self.luts.len() as u64 + 1;
        let entry = self.luts.entry(path.clone()).or_insert_with(|| {
            let lut = match Lut::load(path) {
                Ok(l) => Some(Arc::new(l)),
                Err(e) => {
                    log::warn!("LUT ignorada: {e}");
                    None
                }
            };
            LutEntry { key: next_key, lut }
        });
        entry.lut.clone().map(|l| (entry.key, l))
    }

    /// Convierte las capas en capas del compositor para un lienzo `k` veces el proyecto.
    pub fn gpu_layers<'a>(layers: &'a [SceneLayer], tl: &Timeline, k: f32) -> Vec<Layer<'a>> {
        let project = (tl.width as f32, tl.height as f32);
        layers
            .iter()
            .map(|v| {
                let corners = compose::corners(&v.transform, v.base, project).map(|p| [p[0] * k, p[1] * k]);
                let source = match &v.source {
                    Source::Video(img) => LayerSource {
                        key: v.clip.0,
                        version: Arc::as_ptr(&img.rgba) as u64,
                        width: img.width as u32,
                        height: img.height as u32,
                        rgba: &img.rgba,
                    },
                    Source::Text(r, version) => LayerSource {
                        key: v.clip.0 | 1 << 63,
                        version: *version,
                        width: r.width,
                        height: r.height,
                        rgba: &r.rgba,
                    },
                };
                let lut = v.lut.as_ref().map(|(key, l)| LutSource { key: *key, size: l.size, rgba: &l.rgba });
                Layer { source, corners, opacity: v.transform.opacity, grade: v.grade, lut }
            })
            .collect()
    }

    fn text_entry(&mut self, clip: ClipId, key: TextKey) -> &TextEntry {
        if self.cache.get(&clip).is_none_or(|e| e.key != key) {
            let renderer = self.text.get_or_insert_with(TextRenderer::new);
            let raster = renderer.render(&TextParams {
                text: &key.style.text,
                size_px: key.px as f32 / 4.0,
                bold: key.style.bold,
                font: key.style.font.as_deref(),
                font_file: key.style.font_file.as_deref(),
                color: key.style.color,
                outline: key.style.outline,
                background: key.style.background,
                word_colors: key.word_colors.as_deref(),
            });
            self.version += 1;
            self.cache.insert(clip, TextEntry { key, raster: Arc::new(raster), version: self.version });
        }
        &self.cache[&clip]
    }
}
