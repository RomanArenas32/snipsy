//! Selector de tipografía: fuentes instaladas en el Mac o un archivo .ttf / .otf.

use std::path::PathBuf;

use egui::{RichText, Ui};

/// Dibuja el selector. Devuelve `true` si cambió la fuente.
pub fn font_picker(ui: &mut Ui, salt: &str, font: &mut Option<String>, font_file: &mut Option<PathBuf>) -> bool {
    let mut changed = false;
    let label = match (font.as_deref(), font_file.is_some()) {
        (Some(f), true) => format!("{f} (archivo)"),
        (Some(f), false) => f.to_owned(),
        (None, _) => "Sistema (por defecto)".to_owned(),
    };
    ui.horizontal(|ui| {
        let filter_id = ui.make_persistent_id((salt, "font_filter"));
        egui::ComboBox::from_id_salt((salt, "font")).selected_text(RichText::new(label)).width(170.0).height(320.0).show_ui(ui, |ui| {
            let mut filter: String = ui.data_mut(|d| d.get_temp(filter_id)).unwrap_or_default();
            ui.add(egui::TextEdit::singleline(&mut filter).hint_text("Buscar…").desired_width(160.0));
            if ui.selectable_label(font.is_none(), "Sistema (por defecto)").clicked() {
                *font = None;
                *font_file = None;
                changed = true;
            }
            let needle = filter.to_lowercase();
            for family in snipsy_render::font_families().iter().filter(|f| f.to_lowercase().contains(&needle)) {
                let selected = font.as_deref() == Some(family.as_str()) && font_file.is_none();
                if ui.selectable_label(selected, RichText::new(family)).clicked() {
                    *font = Some(family.clone());
                    *font_file = None;
                    changed = true;
                }
            }
            ui.data_mut(|d| d.insert_temp(filter_id, filter));
        });
        if ui.small_button("Archivo…").on_hover_text("Usar una fuente .ttf / .otf que no está instalada").clicked() {
            if let Some(path) = rfd::FileDialog::new().add_filter("Fuente", &["ttf", "otf", "ttc"]).pick_file() {
                match snipsy_render::font_file_family(&path) {
                    Some(family) => {
                        *font = Some(family);
                        *font_file = Some(path);
                        changed = true;
                    }
                    None => {
                        rfd::MessageDialog::new()
                            .set_title("Fuente no válida")
                            .set_description(format!("No se pudo leer {} como fuente.", path.display()))
                            .show();
                    }
                }
            }
        }
    });
    changed
}
