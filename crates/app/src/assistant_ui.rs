//! Chat con Snipi: entiende órdenes en español y las ejecuta en el proyecto.

use egui::{Color32, RichText, Ui};

use crate::mascot::{self, Mood};

pub const NAME: &str = "Snipi";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Who {
    Me,
    Pet,
}

pub struct AssistantUi {
    messages: Vec<(Who, String)>,
    input: String,
    mood: Mood,
    /// Hasta cuándo dura el ánimo pasajero (feliz, confundido).
    mood_until: f64,
    /// Snipi está esperando que termine algo largo (para avisar al final).
    pub waiting_subtitles: bool,
    focus_input: bool,
}

const SUGGESTIONS: &[&str] = &["corta los silencios", "pásalo a vertical", "subtitula el video", "¿qué puedes hacer?"];

impl AssistantUi {
    pub fn new() -> Self {
        Self {
            messages: vec![(
                Who::Pet,
                format!(
                    "¡Hola! Soy {NAME} ✂️ Te ayudo a editar: escríbeme lo que quieres, por ejemplo «corta los silencios y pásalo a vertical»."
                ),
            )],
            input: String::new(),
            mood: Mood::Happy,
            mood_until: 2.5,
            waiting_subtitles: false,
            focus_input: false,
        }
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.messages.push((Who::Pet, text.into()));
    }

    /// Ánimo pasajero durante `secs` segundos.
    pub fn feel(&mut self, ctx: &egui::Context, mood: Mood, secs: f64) {
        self.mood = mood;
        self.mood_until = ctx.input(|i| i.time) + secs;
    }

    pub fn focus(&mut self) {
        self.focus_input = true;
    }

    /// Dibuja el panel. `busy`: hay un trabajo largo en curso. Devuelve lo que escribió el usuario.
    pub fn show(&mut self, ui: &mut Ui, busy: bool) -> Option<String> {
        let now = ui.input(|i| i.time);
        let mut sent = None;
        let typing = !self.input.trim().is_empty();
        let mood = if now < self.mood_until {
            self.mood
        } else if busy {
            Mood::Working
        } else if typing {
            Mood::Listening
        } else {
            Mood::Idle
        };

        ui.vertical_centered(|ui| {
            let r = mascot::draw(ui, 110.0, mood).on_hover_text(format!("¡Hola! Soy {NAME}."));
            if r.clicked() {
                self.feel(ui.ctx(), Mood::Happy, 1.5);
            }
            ui.label(RichText::new(NAME).heading().strong());
            let line = match mood {
                Mood::Working => "Trabajando…",
                Mood::Listening => "Te escucho…",
                Mood::Thinking => "Pensando…",
                Mood::Confused => "Mmm…",
                Mood::Happy => "¡Listo!",
                Mood::Idle => "¿Qué editamos?",
            };
            ui.label(RichText::new(line).small().color(Color32::GRAY));
        });
        ui.separator();

        // Entrada abajo; mensajes en el espacio que queda.
        egui::Panel::bottom("assistant_input").show_separator_line(false).show(ui, |ui| {
            ui.add_space(4.0);
            if self.messages.len() <= 1 {
                ui.horizontal_wrapped(|ui| {
                    for s in SUGGESTIONS {
                        if ui.small_button(*s).clicked() {
                            sent = Some((*s).to_owned());
                        }
                    }
                });
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                let edit = egui::TextEdit::singleline(&mut self.input)
                    .hint_text(format!("Pídele algo a {NAME}…"))
                    .desired_width(ui.available_width() - 34.0);
                let r = ui.add(edit);
                if std::mem::take(&mut self.focus_input) {
                    r.request_focus();
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let click = ui.add_enabled(typing, egui::Button::new("➤")).on_hover_text("Enviar (Enter)").clicked();
                if (enter || click) && typing {
                    sent = Some(std::mem::take(&mut self.input));
                    r.request_focus();
                }
            });
            ui.add_space(4.0);
        });

        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            let width = ui.available_width();
            for (who, text) in &self.messages {
                let (fill, align) = match who {
                    Who::Pet => (Color32::from_rgb(24, 64, 62), egui::Align::Min),
                    Who::Me => (Color32::from_rgb(52, 56, 70), egui::Align::Max),
                };
                ui.with_layout(egui::Layout::top_down(align), |ui| {
                    egui::Frame::new().fill(fill).corner_radius(8.0).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                        ui.set_max_width(width * 0.85);
                        ui.label(RichText::new(text).color(Color32::from_gray(235)));
                    });
                });
                ui.add_space(4.0);
            }
        });

        if let Some(text) = &sent {
            self.messages.push((Who::Me, text.trim().to_owned()));
        }
        sent
    }
}
