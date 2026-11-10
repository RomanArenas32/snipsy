//! Panel «Biblioteca»: música, efectos, videos, imágenes, ediciones y exportados,
//! disponibles en todos los proyectos.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Child;

use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::{Color32, RichText, Ui};
use snipsy_core::library::{Category, Library, LibraryItem, NewItem};
use snipsy_core::Timeline;
use snipsy_media::MediaInfo;

/// Lo que se arrastra desde la biblioteca al timeline.
#[derive(Clone, Debug)]
pub struct LibraryDrop(pub PathBuf);

/// Pedidos a la app.
pub enum LibraryAction {
    /// Poner el archivo en el cabezal (en la pista que corresponda a su categoría).
    AddAtPlayhead(LibraryItem),
    OpenProject(PathBuf),
    /// Abrir una edición como proyecto nuevo, sin tocar el original.
    UseAsTemplate(PathBuf),
    SaveProject(String),
}

const THUMB_W: u32 = 96;
const THUMB_H: u32 = 54;

/// Miniaturas generadas en un hilo aparte.
struct Thumbs {
    tx: Sender<PathBuf>,
    rx: Receiver<(PathBuf, Option<egui::ColorImage>)>,
    pending: HashSet<PathBuf>,
    done: HashMap<PathBuf, Option<egui::TextureHandle>>,
}

impl Thumbs {
    fn new(ctx: egui::Context) -> Self {
        let (tx, jobs) = unbounded::<PathBuf>();
        let (out, rx) = unbounded();
        std::thread::Builder::new()
            .name("snipsy-thumbs".into())
            .spawn(move || {
                for path in jobs {
                    let img = thumbnail(&path);
                    if out.send((path, img)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("no se pudo crear el hilo de miniaturas");
        Self { tx, rx, pending: HashSet::new(), done: HashMap::new() }
    }

    fn get(&mut self, ctx: &egui::Context, path: &Path) -> Option<&egui::TextureHandle> {
        while let Ok((p, img)) = self.rx.try_recv() {
            self.pending.remove(&p);
            let tex = img.map(|i| ctx.load_texture(format!("thumb:{}", p.display()), i, egui::TextureOptions::LINEAR));
            self.done.insert(p, tex);
        }
        if !self.done.contains_key(path) && self.pending.insert(path.to_owned()) {
            let _ = self.tx.send(path.to_owned());
        }
        self.done.get(path).and_then(|t| t.as_ref())
    }
}

/// Miniatura de un video (cuadro del primer segundo) o de una imagen.
fn thumbnail(path: &Path) -> Option<egui::ColorImage> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    let (w, h, rgba) = if ["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()) {
        let img = snipsy_render::load_image(path).ok()?;
        (img.width, img.height, img.rgba)
    } else {
        let info = MediaInfo::probe(path).ok().filter(|i| i.has_video)?;
        let mut r = snipsy_media::VideoReader::open(path, (THUMB_W * 2, THUMB_H * 2)).ok()?;
        let img = r.frame_at((info.duration / 3.0).min(1.0)).ok()??;
        (img.width, img.height, img.rgba)
    };
    // Reducir (vecino más cercano) para no guardar fotos enteras en la GPU.
    let k = (THUMB_W as f32 / w as f32).min(THUMB_H as f32 / h as f32).min(1.0);
    let (tw, th) = (((w as f32 * k) as usize).max(1), ((h as f32 * k) as usize).max(1));
    let mut px = Vec::with_capacity(tw * th * 4);
    for y in 0..th {
        for x in 0..tw {
            let i = ((y * h / th) * w + x * w / tw) * 4;
            px.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    Some(egui::ColorImage::from_rgba_unmultiplied([tw, th], &px))
}

pub struct LibraryUi {
    lib: Library,
    query: String,
    category: Option<Category>,
    favorites: bool,
    thumbs: Thumbs,
    /// Escucha previa con `afplay` (elemento, proceso).
    playing: Option<(u64, Child)>,
    editing: Option<(u64, String, String)>,
    save_name: String,
    pub message: Option<String>,
}

impl LibraryUi {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            lib: Library::open(Library::default_root()),
            query: String::new(),
            category: None,
            favorites: false,
            thumbs: Thumbs::new(ctx.clone()),
            playing: None,
            editing: None,
            save_name: String::new(),
            message: None,
        }
    }

    fn save(&mut self) {
        if let Err(e) = self.lib.save() {
            self.message = Some(format!("No se pudo guardar la biblioteca: {e}"));
        }
    }

    /// Registra un video exportado.
    pub fn add_export(&mut self, path: &Path) {
        let info = MediaInfo::probe(path).ok();
        self.lib.add_reference(
            path,
            Category::Export,
            NewItem {
                name: String::new(),
                duration: info.as_ref().map(|i| i.duration).unwrap_or(0.0),
                width: info.as_ref().map(|i| i.width).unwrap_or(0),
                height: info.as_ref().map(|i| i.height).unwrap_or(0),
            },
        );
        self.save();
    }

    pub fn save_project(&mut self, name: &str, tl: &Timeline) {
        let msg = match self.lib.add_project(name, tl) {
            Ok(_) => format!("📁 «{name}» guardada en la biblioteca (Mis ediciones)."),
            Err(e) => format!("No se pudo guardar la edición: {e}"),
        };
        self.save();
        self.message = Some(msg);
    }

    /// Copia archivos (o el contenido de carpetas) a la biblioteca.
    fn add_files(&mut self, paths: Vec<PathBuf>) {
        let mut files = Vec::new();
        for p in paths {
            if p.is_dir() {
                collect_files(&p, &mut files, 2);
            } else {
                files.push(p);
            }
        }
        let (mut added, mut skipped) = (0, 0);
        for f in files {
            let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let (category, info) = if ["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()) {
                match snipsy_render::load_image(&f) {
                    Ok(img) => (Category::Image, NewItem { width: img.width as u32, height: img.height as u32, ..Default::default() }),
                    Err(_) => {
                        skipped += 1;
                        continue;
                    }
                }
            } else if ext == "snipsy" {
                let tl = snipsy_core::project::load(&f).ok();
                let info = tl.map(|t| NewItem { duration: t.frame_to_secs(t.duration()), width: t.width, height: t.height, ..Default::default() });
                (Category::Project, info.unwrap_or_default())
            } else {
                match MediaInfo::probe(&f) {
                    Ok(i) if i.has_video || i.has_audio => {
                        let Some(c) = Category::guess(&f, i.has_video, i.duration) else {
                            skipped += 1;
                            continue;
                        };
                        (c, NewItem { duration: i.duration, width: i.width, height: i.height, ..Default::default() })
                    }
                    _ => {
                        skipped += 1;
                        continue;
                    }
                }
            };
            match self.lib.add_copy(&f, category, info) {
                Ok(_) => added += 1,
                Err(e) => {
                    self.message = Some(format!("No se pudo copiar {}: {e}", f.display()));
                    skipped += 1;
                }
            }
        }
        self.save();
        if self.message.is_none() || added > 0 {
            self.message = Some(match skipped {
                0 => format!("📚 {added} archivo(s) agregado(s) a la biblioteca."),
                s => format!("📚 {added} agregado(s); {s} ignorado(s) (no son medios compatibles)."),
            });
        }
    }

    fn stop_preview(&mut self) {
        if let Some((_, mut child)) = self.playing.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn toggle_preview(&mut self, item: &LibraryItem) {
        let same = self.playing.as_ref().is_some_and(|(id, _)| *id == item.id);
        self.stop_preview();
        if !same {
            match std::process::Command::new("afplay").arg(&item.path).spawn() {
                Ok(child) => self.playing = Some((item.id, child)),
                Err(e) => self.message = Some(format!("No se pudo reproducir: {e}")),
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, tl: &Timeline) -> Option<LibraryAction> {
        // ¿Terminó la escucha previa?
        if let Some((_, child)) = self.playing.as_mut() {
            if child.try_wait().ok().flatten().is_some() {
                self.playing = None;
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        let mut action = None;

        ui.horizontal(|ui| {
            if ui.button("➕ Agregar…").on_hover_text("Copia archivos a tu biblioteca (música, efectos, videos, imágenes)").clicked() {
                if let Some(files) = rfd::FileDialog::new()
                    .add_filter("Medios", &["mp3", "wav", "m4a", "aac", "aiff", "flac", "mp4", "mov", "m4v", "png", "jpg", "jpeg", "webp", "snipsy"])
                    .pick_files()
                {
                    self.add_files(files);
                }
            }
            if ui.button("📂 Carpeta…").on_hover_text("Agrega todo el contenido de una carpeta").clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.add_files(vec![dir]);
                }
            }
        });
        ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("🔍 Buscar por nombre o etiqueta").desired_width(f32::INFINITY));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            if ui.selectable_label(self.category.is_none() && !self.favorites, "Todo").clicked() {
                self.category = None;
                self.favorites = false;
            }
            if ui.selectable_label(self.favorites, "★").on_hover_text("Favoritos").clicked() {
                self.favorites = !self.favorites;
            }
            for c in Category::ALL {
                let n = self.lib.count(c);
                let r = ui.selectable_label(self.category == Some(c), format!("{} {n}", c.icon())).on_hover_text(c.label());
                if r.clicked() {
                    self.category = if self.category == Some(c) { None } else { Some(c) };
                }
            }
        });

        if self.category == Some(Category::Project) {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.save_name).hint_text("Nombre de la edición").desired_width(140.0));
                let ok = !self.save_name.trim().is_empty() && tl.duration() > 0;
                if ui.add_enabled(ok, egui::Button::new("💾 Guardar actual")).on_hover_text("Guarda una copia del proyecto actual en Mis ediciones").clicked() {
                    action = Some(LibraryAction::SaveProject(self.save_name.trim().to_owned()));
                    self.save_name.clear();
                }
            });
        }
        ui.separator();

        let items: Vec<LibraryItem> = self.lib.search(&self.query, self.category, self.favorites).into_iter().cloned().collect();
        if items.is_empty() {
            ui.add_space(8.0);
            let hint = if self.lib.items().is_empty() {
                "Tu biblioteca está vacía. Agrega música, efectos, logos o videos que uses seguido: quedan disponibles en todos tus proyectos."
            } else {
                "Nada coincide con la búsqueda."
            };
            ui.label(RichText::new(hint).small().color(Color32::GRAY));
        }
        let mut changed = false;
        let mut remove = None;
        egui::ScrollArea::vertical().id_salt("library_scroll").show(ui, |ui| {
            for item in &items {
                if let Some(a) = self.item_row(ui, item, &mut changed, &mut remove) {
                    action = Some(a);
                }
                ui.add_space(2.0);
            }
        });
        if let Some(id) = remove {
            if self.playing.as_ref().is_some_and(|(p, _)| *p == id) {
                self.stop_preview();
            }
            if let Err(e) = self.lib.remove(id) {
                self.message = Some(format!("No se pudo borrar: {e}"));
            }
            changed = true;
        }
        if changed {
            self.save();
        }
        action
    }

    fn item_row(
        &mut self,
        ui: &mut Ui,
        item: &LibraryItem,
        changed: &mut bool,
        remove: &mut Option<u64>,
    ) -> Option<LibraryAction> {
        let mut out = None;
        let exists = item.exists();
        let frame = egui::Frame::new().fill(Color32::from_gray(28)).corner_radius(4.0).inner_margin(4.0);
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // Miniatura o icono.
                let thumb = matches!(item.category, Category::Video | Category::Image | Category::Export) && exists;
                let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 27.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.0, Color32::from_gray(45));
                match thumb.then(|| self.thumbs.get(ui.ctx(), &item.path)).flatten() {
                    Some(tex) => {
                        let s = tex.size_vec2();
                        let k = (rect.width() / s.x).min(rect.height() / s.y);
                        let r = egui::Rect::from_center_size(rect.center(), s * k);
                        ui.painter().image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, item.category.icon(), egui::FontId::proportional(16.0), Color32::GRAY);
                    }
                }
                ui.vertical(|ui| {
                    // Arrastrable al timeline (excepto ediciones).
                    let label = RichText::new(&item.name).color(if exists { Color32::from_gray(230) } else { Color32::from_rgb(230, 120, 90) });
                    if item.category != Category::Project && exists {
                        let id = ui.make_persistent_id(("lib_drag", item.id));
                        ui.dnd_drag_source(id, LibraryDrop(item.path.clone()), |ui| {
                            ui.label(label);
                        })
                        .response
                        .on_hover_text("Arrástralo al timeline");
                    } else {
                        ui.label(label);
                    }
                    let mut info = Vec::new();
                    if !exists {
                        info.push("⚠ falta el archivo".to_owned());
                    }
                    if item.duration > 0.0 {
                        let secs = item.duration.round() as u64;
                        info.push(format!("{}:{:02}", secs / 60, secs % 60));
                    }
                    if item.width > 0 {
                        info.push(format!("{}×{}", item.width, item.height));
                    }
                    if !item.tags.is_empty() {
                        info.push(item.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "));
                    }
                    ui.label(RichText::new(info.join(" · ")).small().color(Color32::GRAY));
                });
            });
            ui.horizontal(|ui| {
                let star = if item.favorite { RichText::new("★").color(Color32::from_rgb(255, 200, 60)) } else { RichText::new("☆") };
                if ui.small_button(star).on_hover_text("Favorito").clicked() {
                    if let Some(i) = self.lib.get_mut(item.id) {
                        i.favorite = !i.favorite;
                        *changed = true;
                    }
                }
                match item.category {
                    Category::Music | Category::Sfx => {
                        let playing = self.playing.as_ref().is_some_and(|(id, _)| *id == item.id);
                        if ui.add_enabled(exists, egui::Button::new(if playing { "■" } else { "▶" }).small()).on_hover_text("Escuchar").clicked() {
                            self.toggle_preview(item);
                        }
                    }
                    Category::Project => {}
                    _ => {
                        if ui.add_enabled(exists, egui::Button::new("▶").small()).on_hover_text("Ver").clicked() {
                            let _ = std::process::Command::new("open").arg(&item.path).spawn();
                        }
                    }
                }
                if item.category == Category::Project {
                    if ui.add_enabled(exists, egui::Button::new("Abrir").small()).clicked() {
                        out = Some(LibraryAction::OpenProject(item.path.clone()));
                    }
                    if ui.add_enabled(exists, egui::Button::new("Usar como plantilla").small()).on_hover_text("Abre una copia: el original no cambia").clicked() {
                        out = Some(LibraryAction::UseAsTemplate(item.path.clone()));
                    }
                } else if ui.add_enabled(exists, egui::Button::new("➕ Al cabezal").small()).clicked() {
                    out = Some(LibraryAction::AddAtPlayhead(item.clone()));
                }
                ui.menu_button("…", |ui| {
                    if ui.button("Renombrar y etiquetas…").clicked() {
                        self.editing = Some((item.id, item.name.clone(), item.tags.join(", ")));
                        ui.close();
                    }
                    if ui.button("Mostrar en Finder").clicked() {
                        let _ = std::process::Command::new("open").arg("-R").arg(&item.path).spawn();
                        ui.close();
                    }
                    let label = if item.owned { "Quitar y borrar la copia" } else { "Quitar de la biblioteca" };
                    if ui.button(label).clicked() {
                        *remove = Some(item.id);
                        ui.close();
                    }
                });
            });
            // Edición en línea de nombre y etiquetas.
            if let Some((id, name, tags)) = self.editing.as_mut().filter(|e| e.0 == item.id) {
                ui.horizontal(|ui| {
                    ui.label("Nombre");
                    ui.text_edit_singleline(name);
                });
                ui.horizontal(|ui| {
                    ui.label("Etiquetas");
                    ui.add(egui::TextEdit::singleline(tags).hint_text("alegre, corporativo…"));
                });
                let id = *id;
                if ui.small_button("Listo").clicked() {
                    let (name, tags) = (name.trim().to_owned(), tags.clone());
                    if let Some(i) = self.lib.get_mut(id) {
                        if !name.is_empty() {
                            i.name = name;
                        }
                        i.tags = tags.split(',').map(|t| t.trim().trim_start_matches('#').to_owned()).filter(|t| !t.is_empty()).collect();
                    }
                    self.editing = None;
                    *changed = true;
                }
            }
        });
        out
    }
}

impl Drop for LibraryUi {
    fn drop(&mut self) {
        self.stop_preview();
    }
}

/// Archivos dentro de una carpeta (hasta `depth` niveles), sin ocultos.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            continue;
        }
        if p.is_dir() {
            if depth > 0 {
                collect_files(&p, out, depth - 1);
            }
        } else {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_and_folder_scan() {
        let dir = std::env::temp_dir().join(format!("snipsy_libui_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub/deep/deeper")).unwrap();
        let run = |args: &[&str], out: &Path| {
            std::process::Command::new("ffmpeg").args(["-y", "-loglevel", "error"]).args(args).arg(out).status().is_ok_and(|s| s.success())
        };
        let png = dir.join("logo.png");
        let mp4 = dir.join("sub/toma.mp4");
        if !run(&["-f", "lavfi", "-i", "color=c=0x00FF00:size=400x200", "-frames:v", "1"], &png)
            || !run(&["-f", "lavfi", "-i", "color=c=0xFF0000:size=1920x1080:rate=30:duration=2", "-pix_fmt", "yuv420p"], &mp4)
        {
            eprintln!("sin ffmpeg; se omite");
            return;
        }
        std::fs::write(dir.join(".oculto.mp3"), b"x").unwrap();
        std::fs::write(dir.join("sub/deep/deeper/muy_hondo.mp3"), b"x").unwrap();

        let t = thumbnail(&png).unwrap();
        assert_eq!(t.size, [96, 48], "proporción conservada");
        assert!(t.pixels[0].g() > 200 && t.pixels[0].r() < 50);
        let t = thumbnail(&mp4).unwrap();
        assert_eq!(t.size, [96, 54]);
        assert!(t.pixels[100].r() > 200 && t.pixels[100].g() < 50);
        assert!(thumbnail(&dir.join("no.mp4")).is_none());

        let mut files = Vec::new();
        collect_files(&dir, &mut files, 2);
        let names: Vec<String> = files.iter().map(|f| f.strip_prefix(&dir).unwrap().display().to_string()).collect();
        assert_eq!(names, vec!["logo.png", "sub/toma.mp4"], "sin ocultos ni más de 2 niveles");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
