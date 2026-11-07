//! Ventana «Marca»: kits con logo, colores y tipografía de la empresa.

use std::path::{Path, PathBuf};

use egui::{Color32, RichText, Ui};
use snipsy_core::{BrandKit, Corner, TextPreset};

/// Lo que el usuario pidió aplicar al proyecto.
pub enum BrandAction {
    PlaceLogo(BrandKit),
    RestyleAll(BrandKit),
    Insert(BrandKit, TextPreset),
}

pub struct BrandUi {
    library: PathBuf,
    kits: Vec<BrandKit>,
    current: usize,
    open: bool,
    /// Hay cambios sin escribir a disco (se guardan al soltar el ratón).
    pending_save: bool,
    /// Aplicar la marca a los subtítulos automáticos.
    pub use_for_subtitles: bool,
    preview: Option<(PathBuf, egui::TextureHandle)>,
    error: Option<String>,
}

impl BrandUi {
    pub fn new() -> Self {
        let library = BrandKit::library_dir();
        let kits = BrandKit::load_all(&library);
        let active = std::fs::read_to_string(library.join("activa.txt")).unwrap_or_default();
        let current = kits.iter().position(|k| k.id == active.trim()).unwrap_or(0);
        Self { library, kits, current, open: false, pending_save: false, use_for_subtitles: true, preview: None, error: None }
    }

    pub fn open(&mut self) {
        self.open = true;
    }

    /// Kit activo (si hay alguno).
    pub fn active(&self) -> Option<&BrandKit> {
        self.kits.get(self.current)
    }

    fn select(&mut self, i: usize) {
        self.current = i;
        if let Some(k) = self.kits.get(i) {
            let _ = std::fs::create_dir_all(&self.library);
            let _ = std::fs::write(self.library.join("activa.txt"), &k.id);
        }
    }

    fn add_kit(&mut self) {
        let kit = BrandKit::new_in(&self.library, &format!("Marca {}", self.kits.len() + 1));
        if let Err(e) = kit.save(&self.library) {
            self.error = Some(format!("No se pudo crear la marca: {e}"));
            return;
        }
        self.kits.push(kit);
        self.select(self.kits.len() - 1);
    }

    fn save(&mut self) {
        if let Some(k) = self.kits.get(self.current) {
            if let Err(e) = k.save(&self.library) {
                self.error = Some(format!("No se pudo guardar la marca: {e}"));
            }
        }
        self.pending_save = false;
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<BrandAction> {
        if !self.open {
            return None;
        }
        let mut open = true;
        let mut action = None;
        egui::Window::new("🎨 Marca").open(&mut open).resizable(false).default_width(420.0).show(ctx, |ui| {
            action = self.contents(ui);
        });
        self.open = open;
        if self.pending_save && !ctx.input(|i| i.pointer.any_down()) {
            self.save();
        }
        action
    }

    fn contents(&mut self, ui: &mut Ui) -> Option<BrandAction> {
        ui.label(
            RichText::new("Guarda el logo, los colores y la tipografía de la empresa y aplícalos con un clic a cualquier video.")
                .small(),
        );
        if let Some(e) = &self.error {
            ui.colored_label(Color32::from_rgb(230, 120, 90), e);
        }
        ui.add_space(4.0);

        // --- elegir kit
        let mut select = None;
        let mut delete = false;
        ui.horizontal(|ui| {
            let name = self.active().map(|k| k.name.clone()).unwrap_or_else(|| "(ninguna)".into());
            egui::ComboBox::from_id_salt("brand_kit").selected_text(name).width(200.0).show_ui(ui, |ui| {
                for (i, k) in self.kits.iter().enumerate() {
                    if ui.selectable_label(i == self.current, &k.name).clicked() {
                        select = Some(i);
                    }
                }
            });
            if ui.button("➕ Nueva").clicked() {
                self.add_kit();
            }
            if self.active().is_some() && ui.button("🗑").on_hover_text("Borrar esta marca").clicked() {
                delete = rfd::MessageDialog::new()
                    .set_title("Borrar marca")
                    .set_description(format!("¿Borrar la marca «{}»? Los proyectos que ya la usan no cambian.", self.kits[self.current].name))
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show()
                    == rfd::MessageDialogResult::Yes;
            }
        });
        if let Some(i) = select {
            self.select(i);
        }
        if delete {
            let kit = self.kits.remove(self.current);
            if let Err(e) = kit.delete(&self.library) {
                self.error = Some(format!("No se pudo borrar: {e}"));
            }
            self.select(self.current.min(self.kits.len().saturating_sub(1)));
        }

        let library = self.library.clone();
        let Some(kit) = self.kits.get_mut(self.current) else {
            ui.add_space(8.0);
            if ui.button(RichText::new("Crear mi primera marca").strong()).clicked() {
                self.add_kit();
            }
            return None;
        };
        let mut changed = false;
        let mut action = None;
        let mut error = None;

        egui::Grid::new("brand_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Nombre");
            changed |= ui.text_edit_singleline(&mut kit.name).changed();
            ui.end_row();

            // --- logo
            ui.label("Logo");
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    if ui.button(if kit.logo.is_some() { "Cambiar…" } else { "Elegir logo…" }).on_hover_text("PNG con fondo transparente, JPG o WebP").clicked() {
                        if let Some(src) = rfd::FileDialog::new().add_filter("Imagen", &["png", "jpg", "jpeg", "webp"]).pick_file() {
                            match import_logo(kit, &library, &src) {
                                Ok(()) => changed = true,
                                Err(e) => error = Some(e),
                            }
                        }
                    }
                    if kit.logo.is_some() && ui.small_button("Quitar").clicked() {
                        kit.logo = None;
                        changed = true;
                    }
                });
                if let Some(path) = &kit.logo {
                    if let Some(tex) = preview(&mut self.preview, ui.ctx(), path) {
                        let size = tex.size_vec2();
                        let s = (160.0 / size.x).min(60.0 / size.y);
                        // Fondo a cuadros para ver la transparencia.
                        let (rect, _) = ui.allocate_exact_size(size * s, egui::Sense::hover());
                        checkerboard(ui, rect);
                        ui.painter().image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    }
                }
            });
            ui.end_row();

            ui.label("Posición");
            egui::Grid::new("corners").spacing([4.0, 4.0]).show(ui, |ui| {
                for (i, c) in Corner::ALL.into_iter().enumerate() {
                    let arrow = ["↖", "↗", "↙", "↘"][i];
                    changed |= ui.selectable_value(&mut kit.logo_corner, c, arrow).on_hover_text(c.label()).changed();
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
            ui.end_row();
            ui.label("Tamaño");
            changed |= percent(ui, &mut kit.logo_size, 0.05..=0.6).changed();
            ui.end_row();
            ui.label("Margen");
            changed |= percent(ui, &mut kit.margin, 0.0..=0.2).changed();
            ui.end_row();
            ui.label("Opacidad");
            changed |= percent(ui, &mut kit.logo_opacity, 0.1..=1.0).changed();
            ui.end_row();

            // --- tipografía y colores
            ui.label("Tipografía");
            if crate::fonts_ui::font_picker(ui, "brand", &mut kit.font, &mut kit.font_file) {
                changed = true;
                // Copiar la fuente al kit para que no dependa del archivo original.
                if let Some(src) = kit.font_file.clone().filter(|f| !f.starts_with(kit.dir(&library))) {
                    match kit.import_file(&library, &src, "fuente") {
                        Ok(copy) => kit.font_file = Some(copy),
                        Err(e) => error = Some(format!("No se pudo copiar la fuente: {e}")),
                    }
                }
            }
            ui.end_row();
            for (label, hint, color) in [
                ("Principal", "Fondo de los rótulos", &mut kit.primary),
                ("Secundario", "Contornos y texto sobre el acento", &mut kit.secondary),
                ("Acento", "Palabra resaltada y llamadas a la acción", &mut kit.accent),
                ("Texto", "Color de los textos", &mut kit.text),
            ] {
                ui.label(label);
                ui.horizontal(|ui| {
                    let mut c = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
                    if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque).changed() {
                        *color = c.to_srgba_unmultiplied();
                        changed = true;
                    }
                    ui.label(RichText::new(hint).small().color(Color32::GRAY));
                });
                ui.end_row();
            }
        });

        ui.add_space(8.0);
        ui.label(RichText::new("Aplicar al proyecto").strong());
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(kit.logo.is_some(), egui::Button::new("🏷 Poner logo"))
                .on_hover_text("En la pista «Logo», arriba de todo y durante todo el video. Si ya hay uno, lo reemplaza.")
                .clicked()
            {
                action = Some(BrandAction::PlaceLogo(kit.clone()));
            }
            if ui.button("🔤 Aplicar a todos los textos").on_hover_text("Fuente y colores de la marca en todos los textos y subtítulos").clicked() {
                action = Some(BrandAction::RestyleAll(kit.clone()));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Insertar en el cabezal:");
            for p in TextPreset::ALL {
                if ui.button(p.label()).clicked() {
                    action = Some(BrandAction::Insert(kit.clone(), p));
                }
            }
        });
        ui.checkbox(&mut self.use_for_subtitles, "Usar esta marca en los subtítulos automáticos");

        if changed {
            self.pending_save = true;
        }
        if error.is_some() {
            self.error = error;
        } else if changed {
            self.error = None;
        }
        action
    }
}

/// Copia el logo al kit con un nombre nuevo (así el proyecto y la caché ven el cambio)
/// y borra la copia anterior.
fn import_logo(kit: &mut BrandKit, library: &Path, src: &Path) -> Result<(), String> {
    snipsy_render::load_image(src)?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let copy = kit.import_file(library, src, &format!("logo-{stamp}")).map_err(|e| format!("No se pudo copiar el logo: {e}"))?;
    if let Some(old) = kit.logo.replace(copy.clone()) {
        if old != copy && old.starts_with(kit.dir(library)) {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(())
}

fn percent(ui: &mut Ui, v: &mut f32, range: std::ops::RangeInclusive<f32>) -> egui::Response {
    let mut pct = *v * 100.0;
    let r = ui.add(egui::Slider::new(&mut pct, range.start() * 100.0..=range.end() * 100.0).suffix(" %").fixed_decimals(0));
    *v = pct / 100.0;
    r
}

fn preview<'a>(cache: &'a mut Option<(PathBuf, egui::TextureHandle)>, ctx: &egui::Context, path: &Path) -> Option<&'a egui::TextureHandle> {
    if cache.as_ref().is_none_or(|(p, _)| p != path) {
        let img = snipsy_render::load_image(path).ok()?;
        let color = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.rgba);
        *cache = Some((path.to_owned(), ctx.load_texture("brand_logo", color, egui::TextureOptions::LINEAR)));
    }
    cache.as_ref().map(|(_, t)| t)
}

fn checkerboard(ui: &Ui, rect: egui::Rect) {
    let p = ui.painter().with_clip_rect(rect);
    let s = 8.0;
    let (cols, rows) = ((rect.width() / s).ceil() as i32, (rect.height() / s).ceil() as i32);
    for y in 0..rows {
        for x in 0..cols {
            let c = if (x + y) % 2 == 0 { Color32::from_gray(90) } else { Color32::from_gray(60) };
            let min = rect.min + egui::vec2(x as f32 * s, y as f32 * s);
            p.rect_filled(egui::Rect::from_min_size(min, egui::vec2(s, s)), 0.0, c);
        }
    }
}
