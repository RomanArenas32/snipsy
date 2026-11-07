use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cosmic_text::{fontdb, Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};

/// Parámetros de un texto a rasterizar (tamaño ya en píxeles de salida).
#[derive(Clone, Debug, PartialEq)]
pub struct TextParams<'a> {
    pub text: &'a str,
    pub size_px: f32,
    pub bold: bool,
    /// Familia tipográfica (`None` = sans serif del sistema).
    pub font: Option<&'a str>,
    /// Archivo de fuente a cargar si no está instalada.
    pub font_file: Option<&'a Path>,
    pub color: [u8; 4],
    pub outline: Option<[u8; 4]>,
    pub background: Option<[u8; 4]>,
    /// Color de cada palabra (separadas por espacios), en orden. Alfa 0 = oculta
    /// (sigue ocupando su lugar, para que el texto no se mueva al aparecer).
    pub word_colors: Option<&'a [[u8; 4]]>,
}

/// Divide el texto en tramos alternando espacios y palabras; devuelve (tramo, índice de palabra).
fn word_spans(text: &str) -> Vec<(&str, Option<usize>)> {
    let mut out = Vec::new();
    let mut word = 0;
    let mut start = 0;
    let mut in_space: Option<bool> = None;
    for (i, ch) in text.char_indices() {
        let space = ch.is_whitespace();
        if in_space.is_some_and(|s| s != space) {
            let piece = &text[start..i];
            out.push((piece, (!in_space.unwrap()).then_some(word)));
            if !in_space.unwrap() {
                word += 1;
            }
            start = i;
        }
        in_space = Some(space);
    }
    if start < text.len() {
        out.push((&text[start..], (!in_space.unwrap_or(true)).then_some(word)));
    }
    out
}

/// Texto rasterizado: RGBA8 con alfa no premultiplicado.
#[derive(Clone)]
pub struct TextRaster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Rasteriza texto en CPU con las fuentes del sistema (centrado, multilínea).
pub struct TextRenderer {
    fonts: FontSystem,
    swash: SwashCache,
    /// Archivos de fuente ya cargados → su familia.
    files: HashMap<PathBuf, Option<String>>,
}

/// Familias tipográficas instaladas en el sistema, ordenadas (se calculan una vez).
pub fn font_families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let mut v: Vec<String> = db
            .faces()
            .filter_map(|f| f.families.first().map(|(name, _)| name.clone()))
            .filter(|n| !n.starts_with('.'))
            .collect();
        v.sort_by_key(|n| n.to_lowercase());
        v.dedup();
        v
    })
}

/// Familia de un archivo de fuente (.ttf / .otf), o `None` si no es una fuente válida.
pub fn font_file_family(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let mut db = fontdb::Database::new();
    db.load_font_data(data);
    let family = db.faces().find_map(|f| f.families.first().map(|(name, _)| name.clone()));
    family
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRenderer {
    pub fn new() -> Self {
        Self { fonts: FontSystem::new(), swash: SwashCache::new(), files: HashMap::new() }
    }

    /// Familia a usar: carga el archivo de fuente la primera vez que se pide.
    fn family(&mut self, font: Option<&str>, file: Option<&Path>) -> Option<String> {
        let from_file = file.and_then(|f| {
            if !self.files.contains_key(f) {
                let family = std::fs::read(f).ok().and_then(|data| {
                    let ids = self.fonts.db_mut().load_font_source(fontdb::Source::Binary(std::sync::Arc::new(data)));
                    ids.first().and_then(|id| self.fonts.db().face(*id)).and_then(|f| f.families.first().map(|(n, _)| n.clone()))
                });
                if family.is_none() {
                    log::warn!("fuente ignorada: {}", f.display());
                }
                self.files.insert(f.to_owned(), family);
            }
            self.files[f].clone()
        });
        font.map(str::to_owned).or(from_file)
    }

    pub fn render(&mut self, p: &TextParams) -> TextRaster {
        let size = p.size_px.clamp(1.0, 2000.0);
        let family = self.family(p.font, p.font_file);
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(size, size * 1.2));
        let family = match &family {
            Some(name) => Family::Name(name),
            None => Family::SansSerif,
        };
        let attrs = Attrs::new().family(family).weight(if p.bold { Weight::BOLD } else { Weight::NORMAL });
        buffer.set_size(None, None);
        match p.word_colors {
            Some(colors) => {
                let spans = word_spans(p.text);
                let rich = spans.iter().map(|(piece, word)| {
                    let c = word.and_then(|w| colors.get(w)).copied().unwrap_or(p.color);
                    // metadata 1 = palabra oculta (ocupa su lugar pero no se dibuja).
                    let hidden = usize::from(c[3] == 0);
                    (*piece, attrs.clone().color(Color::rgba(c[0], c[1], c[2], 255)).metadata(hidden))
                });
                buffer.set_rich_text(rich, &attrs, Shaping::Advanced, Some(Align::Center));
            }
            None => buffer.set_text(p.text, &attrs, Shaping::Advanced, Some(Align::Center)),
        }
        buffer.shape_until_scroll(&mut self.fonts, false);

        // Medir y volver a maquetar con ese ancho para que el centrado sea correcto.
        let (mut text_w, mut text_h) = (0.0f32, 0.0f32);
        for run in buffer.layout_runs() {
            text_w = text_w.max(run.line_w);
            text_h = text_h.max(run.line_top + run.line_height);
        }
        if text_w <= 0.0 || text_h <= 0.0 {
            return TextRaster { width: 1, height: 1, rgba: vec![0; 4] };
        }
        buffer.set_size(Some(text_w.ceil() + 1.0), None);
        buffer.shape_until_scroll(&mut self.fonts, false);

        let outline_w = if p.outline.is_some() { (size * 0.06).max(1.0) } else { 0.0 };
        let pad = if p.background.is_some() { size * 0.3 } else { 0.0 };
        let margin = (pad + outline_w).ceil() as i32 + 2;
        let width = text_w.ceil() as i32 + 2 * margin;
        let height = text_h.ceil() as i32 + 2 * margin;
        let mut img = Canvas { w: width, h: height, px: vec![[0.0; 4]; (width * height) as usize] };

        if let Some(bg) = p.background {
            let c = to_f(bg);
            img.px.iter_mut().for_each(|px| *px = c);
        }

        // Se dibujan los glifos uno a uno para poder saltar las palabras ocultas.
        // `outline`: todo en ese color, con la cobertura de cada glifo.
        let base = Color::rgba(p.color[0], p.color[1], p.color[2], p.color[3]);
        let draw = |buffer: &mut Buffer, fonts: &mut FontSystem, swash: &mut SwashCache, img: &mut Canvas, outline: Option<[u8; 4]>, dx: i32, dy: i32| {
            buffer.shape_until_scroll(fonts, false);
            for run in buffer.layout_runs() {
                for glyph in run.glyphs.iter().filter(|g| g.metadata == 0) {
                    let physical = glyph.physical((0.0, run.line_y), 1.0);
                    let color = glyph.color_opt.unwrap_or(base);
                    swash.with_pixels(fonts, physical.cache_key, color, |x, y, c| {
                        let col = match outline {
                            Some(o) => [o[0], o[1], o[2], (c.a() as u32 * o[3] as u32 / 255) as u8],
                            None => [c.r(), c.g(), c.b(), c.a()],
                        };
                        img.blend(physical.x + x + margin + dx, physical.y + y + margin + dy, col);
                    });
                }
            }
        };

        if let Some(oc) = p.outline {
            // Contorno: el texto dibujado desplazado en círculo alrededor de su posición.
            let r = outline_w.round().max(1.0) as i32;
            let steps = 16;
            for i in 0..steps {
                let a = i as f32 / steps as f32 * std::f32::consts::TAU;
                let (dx, dy) = ((a.cos() * r as f32).round() as i32, (a.sin() * r as f32).round() as i32);
                draw(&mut buffer, &mut self.fonts, &mut self.swash, &mut img, Some(oc), dx, dy);
            }
        }
        draw(&mut buffer, &mut self.fonts, &mut self.swash, &mut img, None, 0, 0);

        let rgba = img.px.iter().flat_map(|c| c.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)).collect();
        TextRaster { width: width as u32, height: height as u32, rgba }
    }
}

struct Canvas {
    w: i32,
    h: i32,
    /// RGBA en 0–1, alfa no premultiplicado.
    px: Vec<[f32; 4]>,
}

impl Canvas {
    /// Composición "over" con alfa no premultiplicado.
    fn blend(&mut self, x: i32, y: i32, c: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h || c[3] == 0 {
            return;
        }
        let s = to_f(c);
        let d = &mut self.px[(y * self.w + x) as usize];
        let out_a = s[3] + d[3] * (1.0 - s[3]);
        if out_a <= 0.0 {
            return;
        }
        for i in 0..3 {
            d[i] = (s[i] * s[3] + d[i] * d[3] * (1.0 - s[3])) / out_a;
        }
        d[3] = out_a;
    }
}

fn to_f(c: [u8; 4]) -> [f32; 4] {
    c.map(|v| v as f32 / 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words_and_spaces() {
        let s = word_spans("HOLA a\nTODOS ");
        assert_eq!(s, vec![("HOLA", Some(0)), (" ", None), ("a", Some(1)), ("\n", None), ("TODOS", Some(2)), (" ", None)]);
    }
}
