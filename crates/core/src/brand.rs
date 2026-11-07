//! Kits de marca: logo, colores y tipografía de la empresa, guardados una vez y
//! aplicados con un clic para que todos los videos salgan con la misma imagen.
//!
//! Cada kit vive en su carpeta (`~/Library/Application Support/Snipsy/brands/<id>/`)
//! con su `kit.json` y una copia del logo y de la fuente, para que no se pierdan
//! si se mueven los archivos originales.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::compose::{self, TextStyle, Transform};
use crate::effects::Ease;
use crate::{AssetId, ClipId, Frame, Timeline, TrackKind};

/// Nombre de la pista donde va el logo.
pub const LOGO_TRACK: &str = "Logo";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub const ALL: [Corner; 4] = [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight];

    pub fn label(self) -> &'static str {
        match self {
            Corner::TopLeft => "Arriba a la izquierda",
            Corner::TopRight => "Arriba a la derecha",
            Corner::BottomLeft => "Abajo a la izquierda",
            Corner::BottomRight => "Abajo a la derecha",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrandKit {
    /// Nombre de la carpeta del kit.
    pub id: String,
    pub name: String,
    pub logo: Option<PathBuf>,
    pub logo_corner: Corner,
    /// Ancho del logo, como fracción del lado corto del video.
    pub logo_size: f32,
    pub logo_opacity: f32,
    /// Distancia al borde, como fracción del lado corto del video.
    pub margin: f32,
    /// Familia tipográfica (instalada en el sistema o la de `font_file`).
    pub font: Option<String>,
    pub font_file: Option<PathBuf>,
    /// Color principal (fondos de rótulos).
    pub primary: [u8; 4],
    /// Color secundario (contornos, texto sobre el color de acento).
    pub secondary: [u8; 4],
    /// Color de acento (palabra resaltada, llamada a la acción).
    pub accent: [u8; 4],
    /// Color del texto.
    pub text: [u8; 4],
}

impl Default for BrandKit {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "Mi marca".into(),
            logo: None,
            logo_corner: Corner::TopRight,
            logo_size: 0.2,
            logo_opacity: 1.0,
            margin: 0.05,
            font: None,
            font_file: None,
            primary: [0, 102, 255, 255],
            secondary: [20, 20, 20, 255],
            accent: [255, 214, 10, 255],
            text: [255, 255, 255, 255],
        }
    }
}

/// Textos prediseñados con los colores y la fuente de la marca.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextPreset {
    /// Título grande en el centro.
    Title,
    /// Rótulo con nombre y cargo sobre una franja del color principal.
    LowerThird,
    /// Llamada a la acción sobre el color de acento, con rebote.
    CallToAction,
}

impl TextPreset {
    pub const ALL: [TextPreset; 3] = [TextPreset::Title, TextPreset::LowerThird, TextPreset::CallToAction];

    pub fn label(self) -> &'static str {
        match self {
            TextPreset::Title => "Título",
            TextPreset::LowerThird => "Rótulo (nombre y cargo)",
            TextPreset::CallToAction => "Llamada a la acción",
        }
    }

    /// Duración por defecto en segundos.
    pub fn secs(self) -> f64 {
        match self {
            TextPreset::Title => 3.0,
            TextPreset::LowerThird => 4.0,
            TextPreset::CallToAction => 3.0,
        }
    }
}

fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            c if c.is_ascii_alphanumeric() => c,
            _ => '-',
        })
        .collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "marca".into() } else { s }
}

impl BrandKit {
    /// `~/Library/Application Support/Snipsy/brands`
    pub fn library_dir() -> PathBuf {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        home.join("Library/Application Support/Snipsy/brands")
    }

    /// Kit nuevo con una carpeta propia (que no choca con las existentes) dentro de `library`.
    pub fn new_in(library: &Path, name: &str) -> Self {
        let base = slug(name);
        let mut id = base.clone();
        let mut n = 2;
        while library.join(&id).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        Self { id, name: name.to_owned(), ..Default::default() }
    }

    pub fn dir(&self, library: &Path) -> PathBuf {
        library.join(&self.id)
    }

    pub fn save(&self, library: &Path) -> std::io::Result<()> {
        let dir = self.dir(library);
        std::fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(dir.join("kit.json"), json)
    }

    /// Todos los kits guardados, ordenados por nombre (los que no se pueden leer se ignoran).
    pub fn load_all(library: &Path) -> Vec<BrandKit> {
        let Ok(entries) = std::fs::read_dir(library) else { return Vec::new() };
        let mut kits: Vec<BrandKit> = entries
            .flatten()
            .filter_map(|e| {
                let data = std::fs::read(e.path().join("kit.json")).ok()?;
                let mut kit: BrandKit = serde_json::from_slice(&data).ok()?;
                kit.id = e.file_name().to_string_lossy().into_owned();
                Some(kit)
            })
            .collect();
        kits.sort_by_key(|k| (k.name.to_lowercase(), k.id.clone()));
        kits
    }

    pub fn delete(&self, library: &Path) -> std::io::Result<()> {
        if self.id.is_empty() {
            return Ok(());
        }
        std::fs::remove_dir_all(self.dir(library))
    }

    /// Copia un archivo (logo, fuente) a la carpeta del kit y devuelve la copia.
    pub fn import_file(&self, library: &Path, src: &Path, stem: &str) -> std::io::Result<PathBuf> {
        let dir = self.dir(library);
        std::fs::create_dir_all(&dir)?;
        let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let dest = dir.join(if ext.is_empty() { stem.to_owned() } else { format!("{stem}.{ext}") });
        if src != dest {
            std::fs::copy(src, &dest)?;
        }
        Ok(dest)
    }

    fn short_side(tl: &Timeline) -> f32 {
        tl.width.min(tl.height) as f32
    }

    // ------------------------------------------------------------ textos

    /// Pone la fuente y los colores de la marca a un texto, conservando su contenido,
    /// tamaño y si tiene contorno o fondo.
    pub fn restyle(&self, s: &mut TextStyle) {
        if self.font.is_some() {
            s.font = self.font.clone();
            s.font_file = self.font_file.clone();
        }
        s.color = self.text;
        if let Some(bg) = &mut s.background {
            // Se conserva la transparencia del fondo (p. ej. subtítulos semitransparentes).
            *bg = [self.primary[0], self.primary[1], self.primary[2], bg[3]];
        }
        if let Some(w) = &mut s.words {
            w.color = self.accent;
        }
    }

    /// Aplica la marca a todos los textos del timeline. Devuelve cuántos cambió.
    pub fn restyle_all(&self, tl: &mut Timeline) -> usize {
        let mut n = 0;
        for track in &mut tl.video_tracks {
            for clip in &mut track.clips {
                if let Some(s) = &mut clip.text {
                    self.restyle(s);
                    n += 1;
                }
            }
        }
        n
    }

    /// Estilo y posición de un texto prediseñado para el formato del proyecto.
    pub fn preset(&self, preset: TextPreset, tl: &Timeline) -> (TextStyle, Transform) {
        let short = Self::short_side(tl);
        let h = tl.height as f32;
        let base = TextStyle {
            font: self.font.clone(),
            font_file: self.font_file.clone(),
            color: self.text,
            bold: true,
            words: None,
            ..Default::default()
        };
        match preset {
            TextPreset::Title => (
                TextStyle { text: "TU TÍTULO".into(), size: (short * 0.11).round(), outline: Some(self.secondary), background: None, ..base },
                Transform { y: -h * 0.05, ..Default::default() },
            ),
            TextPreset::LowerThird => (
                TextStyle {
                    text: "Nombre Apellido\nCargo".into(),
                    size: (short * 0.045).round(),
                    outline: None,
                    background: Some(self.primary),
                    ..base
                },
                Transform { y: h / 2.0 - short * 0.25, ..Default::default() },
            ),
            TextPreset::CallToAction => (
                TextStyle {
                    text: "¡Escríbenos hoy!".into(),
                    size: (short * 0.065).round(),
                    color: self.secondary,
                    outline: None,
                    background: Some(self.accent),
                    ..base
                },
                Transform { y: h * 0.25, ..Default::default() },
            ),
        }
    }

    /// Inserta un texto prediseñado en `at`, en la pista de video libre más alta
    /// (o en una nueva), con su animación de entrada.
    pub fn insert_preset(&self, preset: TextPreset, tl: &mut Timeline, at: Frame) -> ClipId {
        let (style, transform) = self.preset(preset, tl);
        let fps = tl.fps;
        let len = ((preset.secs() * fps).round() as Frame).max(2);
        let frames = |secs: f64| ((secs * fps).round() as Frame).clamp(1, (len - 1).max(1));
        // Pista: la de más arriba si está libre en ese rango (y no es la de subtítulos ni del logo).
        let free = |t: &crate::Track| !t.clips.iter().any(|c| c.start < at + len && c.end() > at);
        let top = tl.video_tracks.len() - 1;
        let reserved = |t: &crate::Track| t.name == LOGO_TRACK || t.name == "Subtítulos";
        let track = if top > 0 && free(&tl.video_tracks[top]) && !reserved(&tl.video_tracks[top]) {
            top
        } else {
            tl.add_track(TrackKind::Video)
        };
        let id = tl.insert_text(at, track, len, style);
        let clip = tl.clip_mut(id).expect("recién insertado");
        clip.transform = transform;
        clip.fade_in = frames(0.2);
        clip.fade_out = frames(0.2);
        if preset == TextPreset::CallToAction {
            let k = clip.source_in;
            clip.anim.scale.set(k, 0.6);
            clip.anim.scale.set(k + frames(0.15), 1.08);
            clip.anim.scale.set(k + frames(0.3), 1.0);
            clip.anim.scale.keys.iter_mut().for_each(|key| key.ease = Ease::Smooth);
        }
        id
    }

    // ------------------------------------------------------------ logo

    /// Posición y escala del logo (imagen de `img` píxeles) en el lienzo del proyecto.
    pub fn logo_transform(&self, img: (u32, u32), tl: &Timeline) -> Transform {
        let canvas = (tl.width as f32, tl.height as f32);
        let short = Self::short_side(tl);
        let base = compose::contain((img.0.max(1) as f32, img.1.max(1) as f32), canvas);
        let w = (short * self.logo_size).max(1.0);
        let scale = w / base.0.max(1.0);
        let h = base.1 * scale;
        let m = short * self.margin;
        let x = canvas.0 / 2.0 - m - w / 2.0;
        let y = canvas.1 / 2.0 - m - h / 2.0;
        let (sx, sy) = match self.logo_corner {
            Corner::TopLeft => (-1.0, -1.0),
            Corner::TopRight => (1.0, -1.0),
            Corner::BottomLeft => (-1.0, 1.0),
            Corner::BottomRight => (1.0, 1.0),
        };
        Transform { x: x * sx, y: y * sy, scale, rotation: 0.0, opacity: self.logo_opacity }
    }

    /// Pone el logo (ya importado como imagen) en la pista «Logo», arriba de todo
    /// y durante todo el video. Si ya había un logo, se reemplaza.
    pub fn place_logo(&self, tl: &mut Timeline, asset: AssetId) -> Option<ClipId> {
        let a = tl.asset(asset)?.clone();
        let index = match tl.video_tracks.iter().position(|t| t.name == LOGO_TRACK) {
            Some(i) => {
                tl.video_tracks[i].clips.clear();
                i
            }
            None => {
                let i = tl.add_track(TrackKind::Video);
                tl.video_tracks[i].name = LOGO_TRACK.into();
                i
            }
        };
        // Todo el video (o 5 s si todavía está vacío).
        let len = match tl.duration() {
            0 => (tl.fps * 5.0).round() as Frame,
            d => d,
        };
        let id = *tl.insert_asset(asset, 0, index, 0).first()?;
        let transform = self.logo_transform((a.width, a.height), tl);
        let clip = tl.clip_mut(id)?;
        clip.len = len;
        clip.transform = transform;
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Asset;

    fn timeline(w: u32, h: u32) -> Timeline {
        let mut tl = Timeline::new();
        tl.width = w;
        tl.height = h;
        tl
    }

    fn logo(tl: &mut Timeline) -> AssetId {
        tl.add_asset(Asset {
            id: AssetId(0),
            path: "/m/logo.png".into(),
            name: "logo.png".into(),
            duration: 5.0,
            fps: 30.0,
            width: 400,
            height: 200,
            has_video: true,
            has_audio: false,
            still: true,
        })
    }

    #[test]
    fn logo_sits_in_the_chosen_corner() {
        let mut tl = timeline(1080, 1920);
        let a = logo(&mut tl);
        assert_eq!((tl.width, tl.height), (1080, 1920), "un logo no cambia el formato del proyecto");
        let kit = BrandKit { logo_size: 0.2, margin: 0.05, ..Default::default() };
        let t = kit.logo_transform((400, 200), &tl);
        // Ancho 216 px (20 % de 1080), alto 108, a 54 px de los bordes superior y derecho.
        let c = compose::corners(&t, compose::contain((400.0, 200.0), (1080.0, 1920.0)), (1080.0, 1920.0));
        assert!((c[1][0] - (1080.0 - 54.0)).abs() < 0.01 && (c[1][1] - 54.0).abs() < 0.01, "{c:?}");
        assert!((c[0][0] - (1080.0 - 54.0 - 216.0)).abs() < 0.01);
        assert!((c[2][1] - (54.0 + 108.0)).abs() < 0.01);

        let bl = BrandKit { logo_corner: Corner::BottomLeft, ..kit.clone() };
        let c = compose::corners(&bl.logo_transform((400, 200), &tl), compose::contain((400.0, 200.0), (1080.0, 1920.0)), (1080.0, 1920.0));
        assert!((c[3][0] - 54.0).abs() < 0.01 && (c[3][1] - (1920.0 - 54.0)).abs() < 0.01, "{c:?}");

        // Colocar: pista «Logo» arriba, durante todo el video; volver a colocar lo reemplaza.
        tl.insert_text(0, 0, 300, TextStyle::default());
        let id = kit.place_logo(&mut tl, a).unwrap();
        let (r, clip) = tl.find(id).unwrap();
        assert_eq!(tl.track(r).name, LOGO_TRACK);
        assert_eq!(r.index, tl.video_tracks.len() - 1);
        assert_eq!((clip.start, clip.len), (0, 300));
        kit.place_logo(&mut tl, a).unwrap();
        assert_eq!(tl.video_tracks.iter().filter(|t| t.name == LOGO_TRACK).count(), 1);
        assert_eq!(tl.video_tracks[r.index].clips.len(), 1);
        // La imagen fija se puede alargar más allá de su «duración».
        let id = tl.video_tracks[r.index].clips[0].id;
        assert!(tl.trim(id, crate::Edge::End, 900));
        assert_eq!(tl.find(id).unwrap().1.len, 900);
    }

    #[test]
    fn restyle_and_presets_use_brand() {
        let mut tl = timeline(1920, 1080);
        let kit = BrandKit { font: Some("Futura".into()), primary: [10, 20, 30, 255], accent: [1, 2, 3, 255], ..Default::default() };
        let mut sub = TextStyle {
            background: Some([0, 0, 0, 170]),
            words: Some(crate::WordAnimation::default()),
            ..Default::default()
        };
        kit.restyle(&mut sub);
        assert_eq!(sub.font.as_deref(), Some("Futura"));
        assert_eq!(sub.background, Some([10, 20, 30, 170]));
        assert_eq!(sub.words.unwrap().color, [1, 2, 3, 255]);

        let id = kit.insert_preset(TextPreset::CallToAction, &mut tl, 30);
        let (r, c) = tl.find(id).unwrap();
        assert!(r.index > 0, "encima del video");
        assert_eq!((c.start, c.len), (30, 90));
        let s = c.text.as_ref().unwrap();
        assert_eq!(s.background, Some([1, 2, 3, 255]));
        assert_eq!(s.font.as_deref(), Some("Futura"));
        assert!(c.anim.scale.is_animated());
        assert!(c.prop_at(crate::Prop::Scale, 30) < 0.7);
        assert!((c.prop_at(crate::Prop::Scale, 60) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn saves_and_loads_kits() {
        let lib = std::env::temp_dir().join(format!("snipsy_brands_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&lib);
        let a = BrandKit::new_in(&lib, "Café Ñandú");
        assert_eq!(a.id, "cafe-nandu");
        a.save(&lib).unwrap();
        let b = BrandKit::new_in(&lib, "Café Ñandú");
        assert_eq!(b.id, "cafe-nandu-2");
        let src = lib.join("src.png");
        std::fs::write(&src, b"png").unwrap();
        let copy = b.import_file(&lib, &src, "logo").unwrap();
        assert_eq!(copy, lib.join("cafe-nandu-2/logo.png"));
        b.save(&lib).unwrap();
        let all = BrandKit::load_all(&lib);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], a);
        a.delete(&lib).unwrap();
        assert_eq!(BrandKit::load_all(&lib).len(), 1);
        std::fs::remove_dir_all(&lib).unwrap();
    }
}
