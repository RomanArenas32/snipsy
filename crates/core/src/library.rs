//! Biblioteca personal: música, efectos de sonido, videos, imágenes, proyectos
//! («mis ediciones») y videos exportados, disponible en todos los proyectos.
//!
//! Vive en `~/Library/Application Support/Snipsy/library/` con un índice
//! `biblioteca.json`. Los archivos agregados se copian a la carpeta de su categoría
//! (para que no se pierdan si se mueve el original); los exportados solo se referencian.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    Music,
    Sfx,
    Video,
    Image,
    Project,
    Export,
}

impl Category {
    pub const ALL: [Category; 6] = [Category::Music, Category::Sfx, Category::Video, Category::Image, Category::Project, Category::Export];

    pub fn label(self) -> &'static str {
        match self {
            Category::Music => "Música",
            Category::Sfx => "Efectos de sonido",
            Category::Video => "Videos",
            Category::Image => "Imágenes",
            Category::Project => "Mis ediciones",
            Category::Export => "Exportados",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Category::Music => "🎵",
            Category::Sfx => "🔊",
            Category::Video => "🎞",
            Category::Image => "🖼",
            Category::Project => "📁",
            Category::Export => "📤",
        }
    }

    fn folder(self) -> &'static str {
        match self {
            Category::Music => "musica",
            Category::Sfx => "efectos",
            Category::Video => "videos",
            Category::Image => "imagenes",
            Category::Project => "proyectos",
            Category::Export => "exportados",
        }
    }

    /// Categoría probable de un archivo por su extensión y duración (audio corto = efecto).
    pub fn guess(path: &Path, has_video: bool, duration: f64) -> Option<Category> {
        let ext = path.extension()?.to_string_lossy().to_lowercase();
        match ext.as_str() {
            "snipsy" => Some(Category::Project),
            "png" | "jpg" | "jpeg" | "webp" => Some(Category::Image),
            "mp3" | "wav" | "m4a" | "aac" | "aif" | "aiff" | "flac" | "ogg" | "caf" => {
                Some(if duration > 0.0 && duration < 15.0 { Category::Sfx } else { Category::Music })
            }
            _ if has_video => Some(Category::Video),
            _ if duration > 0.0 => Some(if duration < 15.0 { Category::Sfx } else { Category::Music }),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryItem {
    pub id: u64,
    pub category: Category,
    pub name: String,
    pub path: PathBuf,
    /// Etiquetas libres («alegre», «corporativo», «whoosh»…).
    pub tags: Vec<String>,
    pub favorite: bool,
    /// Segundos desde 1970.
    pub added: u64,
    /// Segundos (0 si no aplica).
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    /// El archivo es una copia dentro de la biblioteca (se borra al quitarlo).
    pub owned: bool,
}

impl Default for LibraryItem {
    fn default() -> Self {
        Self {
            id: 0,
            category: Category::Music,
            name: String::new(),
            path: PathBuf::new(),
            tags: Vec::new(),
            favorite: false,
            added: 0,
            duration: 0.0,
            width: 0,
            height: 0,
            owned: false,
        }
    }
}

impl LibraryItem {
    /// ¿Coincide con la búsqueda? (nombre o etiquetas, sin distinguir mayúsculas ni tildes)
    pub fn matches(&self, query: &str) -> bool {
        let q = fold(query);
        q.split_whitespace().all(|w| fold(&self.name).contains(w) || self.tags.iter().any(|t| fold(t).contains(w)))
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }
}

/// Minúsculas y sin tildes, para buscar «musica» y encontrar «Música».
fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            c => c,
        })
        .collect()
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    next_id: u64,
    items: Vec<LibraryItem>,
}

pub struct Library {
    root: PathBuf,
    index: Index,
}

/// Datos de un archivo a agregar (los obtiene la app al analizarlo).
#[derive(Clone, Debug, Default)]
pub struct NewItem {
    pub name: String,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
}

impl Library {
    /// `~/Library/Application Support/Snipsy/library`
    pub fn default_root() -> PathBuf {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        home.join("Library/Application Support/Snipsy/library")
    }

    /// Abre la biblioteca (vacía si todavía no existe o el índice no se puede leer).
    pub fn open(root: PathBuf) -> Self {
        let index = std::fs::read(root.join("biblioteca.json"))
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        Self { root, index }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let json = serde_json::to_string_pretty(&self.index).map_err(std::io::Error::other)?;
        // Escribir a un temporal y renombrar: un corte no deja el índice a medias.
        let tmp = self.root.join("biblioteca.json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, self.root.join("biblioteca.json"))
    }

    pub fn items(&self) -> &[LibraryItem] {
        &self.index.items
    }

    pub fn get(&self, id: u64) -> Option<&LibraryItem> {
        self.index.items.iter().find(|i| i.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut LibraryItem> {
        self.index.items.iter_mut().find(|i| i.id == id)
    }

    /// Elementos que coinciden con el filtro, los más nuevos primero.
    pub fn search(&self, query: &str, category: Option<Category>, favorites_only: bool) -> Vec<&LibraryItem> {
        let mut v: Vec<&LibraryItem> = self
            .index
            .items
            .iter()
            .filter(|i| category.is_none_or(|c| i.category == c))
            .filter(|i| !favorites_only || i.favorite)
            .filter(|i| i.matches(query))
            .collect();
        v.sort_by(|a, b| b.added.cmp(&a.added).then(b.id.cmp(&a.id)));
        v
    }

    pub fn count(&self, category: Category) -> usize {
        self.index.items.iter().filter(|i| i.category == category).count()
    }

    /// ¿Ya está este archivo? (como copia o como referencia)
    pub fn find_path(&self, path: &Path) -> Option<&LibraryItem> {
        self.index.items.iter().find(|i| i.path == path)
    }

    fn push(&mut self, mut item: LibraryItem) -> u64 {
        self.index.next_id += 1;
        item.id = self.index.next_id;
        if item.added == 0 {
            item.added = now();
        }
        self.index.items.push(item);
        self.index.next_id
    }

    /// Ruta libre dentro de la carpeta de `category` para un archivo llamado `file_name`.
    fn free_path(&self, category: Category, file_name: &str) -> PathBuf {
        let dir = self.root.join(category.folder());
        let p = Path::new(file_name);
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "archivo".into());
        let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let mut candidate = dir.join(format!("{stem}{ext}"));
        let mut n = 2;
        while candidate.exists() {
            candidate = dir.join(format!("{stem} ({n}){ext}"));
            n += 1;
        }
        candidate
    }

    /// Copia un archivo a la biblioteca. Si el mismo archivo ya se agregó, devuelve el existente.
    pub fn add_copy(&mut self, src: &Path, category: Category, info: NewItem) -> std::io::Result<u64> {
        if let Some(existing) = self.find_path(src) {
            return Ok(existing.id);
        }
        let file_name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "archivo".into());
        let dest = self.free_path(category, &file_name);
        std::fs::create_dir_all(dest.parent().unwrap())?;
        std::fs::copy(src, &dest)?;
        let name = if info.name.is_empty() { stem_of(src) } else { info.name };
        Ok(self.push(LibraryItem {
            category,
            name,
            path: dest,
            duration: info.duration,
            width: info.width,
            height: info.height,
            owned: true,
            ..Default::default()
        }))
    }

    /// Agrega una referencia a un archivo sin copiarlo (p. ej. un video exportado).
    pub fn add_reference(&mut self, path: &Path, category: Category, info: NewItem) -> u64 {
        if let Some(existing) = self.find_path(path) {
            return existing.id;
        }
        let name = if info.name.is_empty() { stem_of(path) } else { info.name };
        self.push(LibraryItem {
            category,
            name,
            path: path.to_owned(),
            duration: info.duration,
            width: info.width,
            height: info.height,
            owned: false,
            ..Default::default()
        })
    }

    /// Guarda una edición (proyecto) en la biblioteca con el nombre dado.
    pub fn add_project(&mut self, name: &str, tl: &crate::Timeline) -> std::io::Result<u64> {
        let dest = self.free_path(Category::Project, &format!("{}.snipsy", sanitize(name)));
        std::fs::create_dir_all(dest.parent().unwrap())?;
        crate::project::save(&dest, tl)?;
        Ok(self.push(LibraryItem {
            category: Category::Project,
            name: name.to_owned(),
            path: dest,
            duration: tl.frame_to_secs(tl.duration()),
            width: tl.width,
            height: tl.height,
            owned: true,
            ..Default::default()
        }))
    }

    /// Quita un elemento; si el archivo es una copia de la biblioteca, también lo borra.
    pub fn remove(&mut self, id: u64) -> std::io::Result<()> {
        let Some(pos) = self.index.items.iter().position(|i| i.id == id) else { return Ok(()) };
        let item = self.index.items.remove(pos);
        if item.owned && item.path.starts_with(&self.root) && item.path.exists() {
            std::fs::remove_file(&item.path)?;
        }
        Ok(())
    }
}

fn stem_of(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Nombre de archivo seguro (sin barras ni caracteres raros).
fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || " -_()".contains(c) { c } else { '_' }).collect();
    let s = s.trim().to_owned();
    if s.is_empty() { "Edición".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("snipsy_lib_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn guesses_categories() {
        assert_eq!(Category::guess(Path::new("a.mp3"), false, 120.0), Some(Category::Music));
        assert_eq!(Category::guess(Path::new("whoosh.WAV"), false, 1.2), Some(Category::Sfx));
        assert_eq!(Category::guess(Path::new("logo.png"), false, 0.0), Some(Category::Image));
        assert_eq!(Category::guess(Path::new("toma.mov"), true, 30.0), Some(Category::Video));
        assert_eq!(Category::guess(Path::new("x.snipsy"), false, 0.0), Some(Category::Project));
        assert_eq!(Category::guess(Path::new("notas.txt"), false, 0.0), None);
    }

    #[test]
    fn adds_searches_persists_and_removes() {
        let dir = temp("main");
        let root = dir.join("lib");
        let src = dir.join("Canción Alegre.mp3");
        std::fs::write(&src, b"mp3").unwrap();

        let mut lib = Library::open(root.clone());
        let a = lib.add_copy(&src, Category::Music, NewItem { duration: 95.0, ..Default::default() }).unwrap();
        let copy = lib.get(a).unwrap().path.clone();
        assert!(copy.starts_with(root.join("musica")) && copy.exists());
        assert_eq!(lib.get(a).unwrap().name, "Canción Alegre");
        // Mismo nombre de archivo desde otra carpeta: no se pisa.
        let other = dir.join("otra");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("Canción Alegre.mp3"), b"otra").unwrap();
        let b = lib.add_copy(&other.join("Canción Alegre.mp3"), Category::Music, NewItem::default()).unwrap();
        assert_ne!(lib.get(b).unwrap().path, copy);
        assert_eq!(std::fs::read(&copy).unwrap(), b"mp3");

        lib.get_mut(a).unwrap().tags = vec!["corporativo".into()];
        lib.get_mut(a).unwrap().favorite = true;
        let export = dir.join("Anuncio_9x16.mp4");
        std::fs::write(&export, b"mp4").unwrap();
        let e = lib.add_reference(&export, Category::Export, NewItem::default());
        assert_eq!(lib.add_reference(&export, Category::Export, NewItem::default()), e, "sin duplicados");

        assert_eq!(lib.search("cancion", None, false).len(), 2, "sin tildes");
        assert_eq!(lib.search("CORPORATIVO", None, false).iter().map(|i| i.id).collect::<Vec<_>>(), vec![a]);
        assert_eq!(lib.search("", None, true).len(), 1);
        assert_eq!(lib.search("", Some(Category::Export), false).len(), 1);
        assert_eq!(lib.search("", None, false)[0].id, e, "los más nuevos primero");

        let mut tl = crate::Timeline::new();
        tl.width = 1080;
        tl.height = 1920;
        let p = lib.add_project("Promo / Verano", &tl).unwrap();
        let item = lib.get(p).unwrap().clone();
        assert_eq!(item.path.file_name().unwrap(), "Promo _ Verano.snipsy");
        assert_eq!(crate::project::load(&item.path).unwrap().height, 1920);

        lib.save().unwrap();
        let mut again = Library::open(root.clone());
        assert_eq!(again.items().len(), 4);
        assert_eq!(again.get(a).unwrap().tags, vec!["corporativo".to_owned()]);

        // Quitar: borra las copias propias, nunca los archivos referenciados.
        again.remove(a).unwrap();
        again.remove(e).unwrap();
        assert!(!copy.exists());
        assert!(export.exists());
        assert_eq!(again.items().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
