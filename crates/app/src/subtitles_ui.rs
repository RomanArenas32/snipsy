//! Diálogo y progreso de los subtítulos automáticos.

use egui::{Color32, RichText};
use snipsy_core::Timeline;
use snipsy_core::WordMode;
use snipsy_subtitles::{Caption, CaptionLook, CaptionStyle, Model, SubtitleEvent, SubtitleJob, SubtitleSettings, LANGUAGES};

#[derive(Clone)]
struct Options {
    model: Model,
    language: usize,
    look: CaptionLook,
}

impl Default for Options {
    fn default() -> Self {
        Self { model: Model::Small, language: 0, look: CaptionLook::default() }
    }
}

struct Running {
    job: SubtitleJob,
    stage: String,
    fraction: Option<f32>,
    look: CaptionLook,
}

pub enum SubtitleResult {
    Apply(Vec<Caption>, CaptionLook),
    Message(String),
}

#[derive(Default)]
pub struct SubtitlesUi {
    options: Options,
    dialog_open: bool,
    running: Option<Running>,
}

impl SubtitlesUi {
    pub fn open(&mut self) {
        if self.running.is_none() {
            self.dialog_open = true;
        }
    }

    /// Empieza a transcribir con las últimas opciones elegidas (sin abrir el diálogo).
    pub fn start_now(&mut self, tl: &Timeline) -> bool {
        if self.running.is_some() {
            return false;
        }
        self.start(tl);
        true
    }

    pub fn is_busy(&self) -> bool {
        self.running.is_some()
    }

    fn start(&mut self, tl: &Timeline) {
        let o = &self.options;
        let settings = SubtitleSettings {
            model: o.model,
            language: LANGUAGES[o.language].0.map(str::to_owned),
            style: o.look.style,
        };
        self.running = Some(Running {
            job: SubtitleJob::start(tl, settings),
            stage: "Preparando…".into(),
            fraction: None,
            look: o.look.clone(),
        });
        self.dialog_open = false;
    }

    /// Dibuja el diálogo o el progreso. Devuelve el resultado cuando termina.
    pub fn show(&mut self, ctx: &egui::Context, tl: &Timeline) -> Option<SubtitleResult> {
        if let Some(run) = self.running.as_mut() {
            let mut result = None;
            while let Ok(ev) = run.job.events().try_recv() {
                match ev {
                    SubtitleEvent::Downloading { done, total } => {
                        let mb = done as f64 / 1e6;
                        run.stage = match total {
                            Some(t) => format!("Descargando el modelo (solo la primera vez)… {mb:.0} de {:.0} MB", t as f64 / 1e6),
                            None => format!("Descargando el modelo (solo la primera vez)… {mb:.0} MB"),
                        };
                        run.fraction = total.map(|t| done as f32 / t.max(1) as f32);
                    }
                    SubtitleEvent::PreparingAudio => {
                        run.stage = "Preparando el audio del timeline…".into();
                        run.fraction = None;
                    }
                    SubtitleEvent::Transcribing { percent } => {
                        run.stage = "Reconociendo la voz…".into();
                        run.fraction = Some(percent as f32 / 100.0);
                    }
                    SubtitleEvent::Done(captions) => result = Some(SubtitleResult::Apply(captions, run.look.clone())),
                    SubtitleEvent::Cancelled => result = Some(SubtitleResult::Message("Subtítulos cancelados.".into())),
                    SubtitleEvent::Error(e) => result = Some(SubtitleResult::Message(format!("Error en los subtítulos: {e}"))),
                }
            }
            if result.is_some() {
                self.running = None;
                return result;
            }
            let mut cancel = false;
            egui::Window::new("Subtítulos automáticos").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.set_min_width(380.0);
                ui.label(&run.stage);
                match run.fraction {
                    Some(f) => ui.add(egui::ProgressBar::new(f).show_percentage()),
                    None => ui.add(egui::ProgressBar::new(0.0).animate(true)),
                };
                ui.add_space(4.0);
                if ui.button("Cancelar").clicked() {
                    cancel = true;
                }
            });
            if cancel {
                run.job.cancel();
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return None;
        }

        if !self.dialog_open {
            return None;
        }
        let mut open = true;
        let mut go = false;
        let mut result = None;
        let o = &mut self.options;
        egui::Window::new("Subtítulos automáticos").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.set_min_width(400.0);
            ui.label(RichText::new("Se transcribe el audio del timeline (en tu Mac, sin enviar nada a internet) y se crean textos sincronizados en la pista «Subtítulos».").small());
            ui.add_space(6.0);
            egui::Grid::new("subs_opts").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Idioma");
                egui::ComboBox::from_id_salt("subs_lang").selected_text(LANGUAGES[o.language].1).show_ui(ui, |ui| {
                    for (i, (_, name)) in LANGUAGES.iter().enumerate() {
                        ui.selectable_value(&mut o.language, i, *name);
                    }
                });
                ui.end_row();
                ui.label("Estilo");
                ui.vertical(|ui| {
                    for s in [CaptionStyle::Reel, CaptionStyle::Classic] {
                        ui.radio_value(&mut o.look.style, s, s.label());
                    }
                });
                ui.end_row();
                ui.label("Animación");
                ui.vertical(|ui| {
                    ui.radio_value(&mut o.look.words, None, "Sin animación de palabras");
                    for m in WordMode::ALL {
                        ui.radio_value(&mut o.look.words, Some(m), m.label());
                    }
                    if o.look.words.is_some() {
                        ui.horizontal(|ui| {
                            ui.label("Color de la palabra actual");
                            let c = &mut o.look.highlight;
                            let mut c32 = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
                            if egui::color_picker::color_edit_button_srgba(ui, &mut c32, egui::color_picker::Alpha::Opaque).changed() {
                                *c = c32.to_srgba_unmultiplied();
                            }
                        });
                    }
                    ui.checkbox(&mut o.look.bounce, "Entrada con rebote");
                });
                ui.end_row();
                ui.label("Modelo");
                ui.vertical(|ui| {
                    for m in Model::ALL {
                        let mark = if m.is_downloaded() { " ✓" } else { "" };
                        ui.radio_value(&mut o.model, m, format!("{}{mark}", m.label()));
                    }
                });
                ui.end_row();
            });
            if !o.model.is_downloaded() {
                ui.label(RichText::new("El modelo se descarga una sola vez y queda guardado en el Mac.").small().color(Color32::from_rgb(230, 180, 80)));
            }
            if snipsy_subtitles::subtitle_track(tl).is_some() {
                ui.label(RichText::new("Ya hay subtítulos: se reemplazarán (puedes deshacer con ⌘Z).").small().color(Color32::GRAY));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Generar").strong()).clicked() {
                    go = true;
                }
                let captions = snipsy_subtitles::captions_from_timeline(tl);
                if ui.add_enabled(!captions.is_empty(), egui::Button::new("Guardar .srt…")).on_hover_text("Archivo de subtítulos para YouTube, Facebook, etc.").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Subtítulos SRT", &["srt"]).set_file_name("subtitulos.srt").save_file() {
                        result = Some(match std::fs::write(&path, snipsy_subtitles::to_srt(&captions)) {
                            Ok(()) => SubtitleResult::Message(format!("✅ {} subtítulos guardados en {}", captions.len(), path.display())),
                            Err(e) => SubtitleResult::Message(format!("No se pudo guardar: {e}")),
                        });
                    }
                }
            });
        });
        if !open {
            self.dialog_open = false;
        } else if go {
            self.start(tl);
        }
        result
    }
}
