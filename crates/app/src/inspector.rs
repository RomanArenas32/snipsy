//! Inspector: propiedades del clip seleccionado o, sin selección, del proyecto.

use std::collections::HashSet;

use egui::{Color32, RichText, Ui};
use snipsy_core::compose::{self, FORMATS};
use snipsy_core::{Animation, Clip, ClipId, ColorGrade, Ease, Frame, History, Prop, Timeline, TrackKind, TrackRef, Transform};

const KEY_ON: Color32 = Color32::from_rgb(255, 200, 60);

/// Registra un punto de deshacer una sola vez por interacción.
struct Undo<'a> {
    before: Option<Timeline>,
    history: &'a mut History,
    changed: bool,
}

impl Undo<'_> {
    fn checkpoint(&mut self) {
        if let Some(before) = self.before.take() {
            self.history.checkpoint(&before);
        }
    }

    fn track(&mut self, r: &egui::Response) {
        let starts = r.drag_started() || r.gained_focus() || (r.changed() && !r.dragged() && !r.has_focus());
        if starts {
            self.checkpoint();
        }
        if r.changed() {
            self.changed = true;
        }
    }

    fn clicked(&mut self, r: &egui::Response) -> bool {
        if r.clicked() {
            self.checkpoint();
            self.changed = true;
            return true;
        }
        false
    }
}

#[derive(Default)]
pub struct InspectorOutput {
    /// Cambió algo del proyecto.
    pub changed: bool,
    /// El cambio afecta a lo que hay que decodificar o mezclar (transiciones, audio):
    /// hay que reconstruir los segmentos del reproductor.
    pub rebuild: bool,
    pub seek: Option<Frame>,
    /// Mensaje para la barra de estado.
    pub message: Option<String>,
}

pub fn show(
    ui: &mut Ui,
    tl: &mut Timeline,
    history: &mut History,
    selection: &HashSet<ClipId>,
    playhead: Frame,
    faces: &crate::faces_ui::Faces,
    follow_faces: bool,
) -> InspectorOutput {
    let mut undo = Undo { before: Some(tl.clone()), history, changed: false };
    let mut out = InspectorOutput::default();
    ui.add_space(4.0);

    // Preferir el clip de video/texto de la selección; su audio vinculado se muestra debajo.
    let mut ids: Vec<ClipId> = selection.iter().copied().collect();
    ids.sort();
    let target = ids
        .iter()
        .find(|id| tl.find(**id).is_some_and(|(r, _)| r.kind == TrackKind::Video))
        .or(ids.first())
        .copied();

    egui::ScrollArea::vertical().show(ui, |ui| match target {
        Some(id) => clip_ui(ui, tl, id, playhead, faces, &mut undo, &mut out),
        None => project_ui(ui, tl, follow_faces, &mut undo, &mut out),
    });
    out.changed = undo.changed;
    out
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong());
    ui.separator();
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(Color32::GRAY));
}

fn project_ui(ui: &mut Ui, tl: &mut Timeline, follow_faces: bool, undo: &mut Undo, out: &mut InspectorOutput) {
    ui.label(RichText::new("Proyecto").heading());
    section(ui, "Formato");
    let current = FORMATS.iter().find(|(_, w, h)| *w == tl.width && *h == tl.height).map(|f| f.0);
    let label = current.map(str::to_owned).unwrap_or_else(|| format!("Personalizado {}x{}", tl.width, tl.height));
    egui::ComboBox::from_id_salt("format").selected_text(label).width(ui.available_width()).show_ui(ui, |ui| {
        for (name, w, h) in FORMATS {
            let r = ui.selectable_label(current == Some(*name), *name);
            if undo.clicked(&r) {
                // Reencuadrar el contenido para el formato nuevo (⌘Z vuelve atrás).
                *tl = snipsy_core::reframe::reframe(tl, *w, *h, follow_faces);
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("Tamaño");
        let r = ui.add(egui::DragValue::new(&mut tl.width).range(16..=8192).suffix(" px"));
        undo.track(&r);
        ui.label("×");
        let r = ui.add(egui::DragValue::new(&mut tl.height).range(16..=8192).suffix(" px"));
        undo.track(&r);
    });
    // Los encoders de video requieren dimensiones pares.
    tl.width = tl.width.max(16) & !1;
    tl.height = tl.height.max(16) & !1;
    ui.label(RichText::new(format!("{:.3} fps", tl.fps)).color(Color32::GRAY));

    silence_ui(ui, tl, undo, out);

    section(ui, "Consejo");
    hint(
        ui,
        "Selecciona un clip para cambiar su posición, escala, color, fundidos o volumen. \
         También puedes arrastrarlo en el visor. Con el rombo ◆ de cada propiedad creas keyframes para animarla.",
    );
}

fn clip_ui(ui: &mut Ui, tl: &mut Timeline, id: ClipId, playhead: Frame, faces: &crate::faces_ui::Faces, undo: &mut Undo, out: &mut InspectorOutput) {
    let Some((track, clip)) = tl.find(id) else { return };
    let source = tl.asset(clip.asset).map(|a| (a.width as f32, a.height as f32, a.name.clone()));
    let title = match (&clip.text, &source) {
        (Some(_), _) => "Texto".to_owned(),
        (None, Some((_, _, name))) => name.clone(),
        _ => "Clip".to_owned(),
    };
    ui.label(RichText::new(title).heading());
    let unbounded = tl.is_unbounded(clip);
    let speed = clip.speed;

    if !unbounded {
        speed_ui(ui, tl, id, speed, undo, out);
    }

    if track.kind == TrackKind::Video {
        video_ui(ui, tl, id, track, playhead, faces, undo, out);
    }

    // Audio: el propio clip, o el audio vinculado al video seleccionado.
    let audio = if track.kind == TrackKind::Audio {
        Some(id)
    } else {
        tl.with_links([id]).into_iter().find(|l| tl.find(*l).is_some_and(|(r, _)| r.kind == TrackKind::Audio))
    };
    if let Some(aid) = audio {
        audio_ui(ui, tl, aid, track.kind == TrackKind::Audio, playhead, undo, out);
    }
}

fn speed_ui(ui: &mut Ui, tl: &mut Timeline, id: ClipId, speed: f64, undo: &mut Undo, out: &mut InspectorOutput) {
    section(ui, "Velocidad");
    let mut new = None;
    ui.horizontal_wrapped(|ui| {
        for (label, v) in [("0.25×", 0.25), ("0.5×", 0.5), ("1×", 1.0), ("1.5×", 1.5), ("2×", 2.0), ("3×", 3.0)] {
            let r = ui.selectable_label((speed - v).abs() < 1e-6, label);
            if r.clicked() {
                new = Some(v);
            }
        }
    });
    ui.horizontal(|ui| {
        let mut pct = speed * 100.0;
        let r = ui.add(egui::DragValue::new(&mut pct).range(25.0..=400.0).speed(1.0).suffix(" %"));
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            new = Some(pct / 100.0);
        }
        hint(ui, "El clip se alarga o acorta; los siguientes se desplazan. El audio conserva el tono.");
    });
    if let Some(v) = new {
        if (v - speed).abs() > 1e-6 {
            undo.checkpoint();
            if tl.set_speed(id, v) {
                undo.changed = true;
                out.rebuild = true;
            }
        }
    }
}

// ---------------------------------------------------------------- video

#[allow(clippy::too_many_arguments)]
fn video_ui(
    ui: &mut Ui,
    tl: &mut Timeline,
    id: ClipId,
    track: TrackRef,
    playhead: Frame,
    faces: &crate::faces_ui::Faces,
    undo: &mut Undo,
    out: &mut InspectorOutput,
) {
    let project = (tl.width as f32, tl.height as f32);
    let fps = tl.fps;
    let source = tl.asset(tl.find(id).unwrap().1.asset).map(|a| (a.width as f32, a.height as f32));
    let index = tl.track(track).clips.iter().position(|c| c.id == id).unwrap_or(0);
    let has_prev = index > 0 && tl.track(track).clips[index - 1].end() == tl.track(track).clips[index].start;
    let effective = tl.transition_split(track, index).map(|(a, b)| a + b);
    let links = tl.with_links([id]);
    let is_subtitle = tl.track(track).name == snipsy_subtitles::TRACK_NAME;
    let mut apply_to_all = None;
    // Seguimiento de rostro: solo videos (no textos ni imágenes fijas).
    let face_asset = tl
        .asset(tl.find(id).unwrap().1.asset)
        .filter(|a| a.has_video && !a.still)
        .map(|a| (a.path.clone(), a.faces.as_ref().map(|f| f.path.is_empty())));

    let Some(clip) = tl.clip_mut(id) else { return };
    if let Some(style) = clip.text.as_mut() {
        section(ui, "Texto");
        let r = ui.add(egui::TextEdit::multiline(&mut style.text).desired_rows(3).desired_width(f32::INFINITY));
        undo.track(&r);
        ui.horizontal(|ui| {
            ui.label("Tamaño");
            let r = ui.add(egui::DragValue::new(&mut style.size).range(4.0..=1000.0).speed(1.0).suffix(" px"));
            undo.track(&r);
            let r = ui.checkbox(&mut style.bold, "Negrita");
            undo.track(&r);
        });
        ui.horizontal(|ui| {
            ui.label("Fuente");
            // (El punto de deshacer guarda el timeline de antes de este cuadro.)
            if crate::fonts_ui::font_picker(ui, "inspector", &mut style.font, &mut style.font_file) {
                undo.checkpoint();
                undo.changed = true;
            }
        });
        color_row(ui, "Color", &mut style.color, undo);
        optional_color_row(ui, "Contorno", &mut style.outline, [0, 0, 0, 255], undo);
        optional_color_row(ui, "Fondo", &mut style.background, [0, 0, 0, 160], undo);

        section(ui, "Animación de palabras");
        let mut mode = style.words.as_ref().map(|w| w.mode);
        let before = mode;
        ui.radio_value(&mut mode, None, "Ninguna");
        for m in snipsy_core::WordMode::ALL {
            ui.radio_value(&mut mode, Some(m), m.label());
        }
        if mode != before {
            undo.checkpoint();
            undo.changed = true;
            style.words = mode.map(|m| snipsy_core::WordAnimation { mode: m, ..style.words.take().unwrap_or_default() });
        }
        if let Some(w) = style.words.as_mut() {
            color_row(ui, "Color de la palabra actual", &mut w.color, undo);
            if w.starts.len() != style.text.split_whitespace().count() {
                hint(ui, "Sin tiempos por palabra: se reparten parejas a lo largo del clip.");
            }
        }
        if is_subtitle {
            ui.add_space(4.0);
            let r = ui.button("Aplicar estilo a todos los subtítulos").on_hover_text("Copia tamaño, colores, posición y escala a toda la pista «Subtítulos»");
            if undo.clicked(&r) {
                apply_to_all = Some((style.clone(), clip.transform));
            }
        }
    }

    // --- transformación con keyframes
    section(ui, "Transformación");
    let inside = clip.contains(playhead);
    egui::Grid::new("transform").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
        for p in Prop::TRANSFORM {
            ui.label(prop_label(p));
            let mut v = clip.prop_at(p, playhead);
            let r = match p {
                Prop::X | Prop::Y => ui.add(egui::DragValue::new(&mut v).speed(1.0).suffix(" px")),
                Prop::Scale => percent_slider(ui, &mut v, 1.0..=500.0, false),
                Prop::Rotation => ui.add(egui::Slider::new(&mut v, -180.0..=180.0).suffix("°")),
                _ => percent_slider(ui, &mut v, 0.0..=100.0, true),
            };
            if r.changed() {
                clip.set_prop(p, playhead, if p == Prop::Scale { v.max(0.01) } else { v });
            }
            undo.track(&r);
            key_button(ui, clip, p, playhead, inside, undo);
            ui.end_row();
        }
    });
    keyframe_nav(ui, clip, playhead, out);

    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        let opacity = clip.transform.opacity;
        if clip.text.is_none() {
            let r = ui.button("Ajustar").on_hover_text("El video completo dentro del lienzo");
            if undo.clicked(&r) {
                clear_geometry(&mut clip.anim);
                clip.transform = Transform { opacity, ..Default::default() };
            }
            let r = ui.button("Rellenar").on_hover_text("Cubre todo el lienzo recortando los bordes (ideal para pasar de 16:9 a 9:16)");
            if undo.clicked(&r) {
                let src = source.unwrap_or(project);
                clear_geometry(&mut clip.anim);
                clip.transform = Transform { scale: compose::cover_scale(src, project), opacity, ..Default::default() };
            }
        }
        let r = ui.button("Centrar");
        if undo.clicked(&r) {
            clip.set_prop(Prop::X, playhead, 0.0);
            clip.set_prop(Prop::Y, playhead, 0.0);
        }
        let r = ui.button("Restablecer");
        if undo.clicked(&r) {
            clear_geometry(&mut clip.anim);
            clip.anim.opacity.keys.clear();
            clip.transform = Transform::default();
        }
    });

    if let Some((path, analyzed)) = &face_asset {
        let r = ui
            .checkbox(&mut clip.follow_face, "🎯 Seguir rostro")
            .on_hover_text("Mueve el recorte para mantener a la persona en cuadro (ideal al pasar un video horizontal a vertical)");
        if undo.clicked(&r) {}
        if clip.follow_face {
            let spare = source.map(|src| {
                let base = compose::contain(src, project);
                let s = clip.prop_at(Prop::Scale, playhead);
                base.0 * s > project.0 + 1.0 || base.1 * s > project.1 + 1.0
            });
            if let Some(f) = faces.progress(path) {
                ui.add(egui::ProgressBar::new(f).text("Buscando rostros…").desired_width(200.0));
            } else if let Some(e) = faces.failed(path) {
                hint(ui, &format!("No se pudo analizar el video: {e}"));
            } else if *analyzed == Some(true) {
                hint(ui, "No se encontraron rostros en este video: se mantiene la posición.");
            } else if spare == Some(false) {
                hint(ui, "Solo tiene efecto si el video está recortado: usa «Rellenar» o un formato distinto al del video.");
            } else {
                hint(ui, "La posición la decide el seguimiento; desactívalo para moverlo a mano.");
            }
        }
    }

    // --- animaciones rápidas
    section(ui, "Animación rápida");
    ui.horizontal_wrapped(|ui| {
        let r = ui.button("🔍 Zoom lento").on_hover_text("Acerca la imagen suavemente durante todo el clip (efecto Ken Burns)");
        if undo.clicked(&r) {
            preset_slow_zoom(clip);
        }
        let r = ui.button("⬆ Entrar desde abajo").on_hover_text("Sube desde fuera del cuadro durante el primer medio segundo");
        if undo.clicked(&r) {
            preset_slide_in(clip, project.1, fps);
        }
        let r = ui.button("💥 Pop").on_hover_text("Aparece creciendo con un pequeño rebote");
        if undo.clicked(&r) {
            preset_pop(clip, fps);
        }
        if !clip.anim.is_empty() {
            let r = ui.button("Quitar animación");
            if undo.clicked(&r) {
                remove_animation(clip, playhead);
            }
        }
    });

    // --- fundidos y transición
    section(ui, "Fundidos y transición");
    egui::Grid::new("fades").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Fundido de entrada");
        let r = seconds(ui, &mut clip.fade_in, clip.len, fps);
        undo.track(&r);
        ui.end_row();
        ui.label("Fundido de salida");
        let r = seconds(ui, &mut clip.fade_out, clip.len, fps);
        undo.track(&r);
        ui.end_row();
        ui.label("Disolvencia");
        ui.add_enabled_ui(has_prev, |ui| {
            let mut t = clip.transition_in;
            let r = seconds(ui, &mut t, clip.len, fps)
                .on_hover_text("Fundido cruzado con el clip anterior (deben estar pegados en la misma pista)");
            if r.changed() {
                out.rebuild = true;
            }
            undo.track(&r);
            clip.transition_in = t;
        });
        ui.end_row();
    });
    if !has_prev {
        hint(ui, "La disolvencia se aplica entre este clip y el anterior cuando están pegados en la misma pista.");
    } else if let (true, Some(eff)) = (clip.transition_in > 0, effective) {
        if eff < clip.transition_in {
            hint(ui, &format!("Se acortó a {:.2} s: no hay más material antes o después del corte.", eff as f64 / fps));
        }
    } else if clip.transition_in > 0 {
        hint(ui, "No hay material sobrante en los clips para hacer la disolvencia (recorta sus bordes).");
    }
    let transition = clip.transition_in;
    let is_text = clip.text.is_some();

    // --- color
    if !is_text {
        color_ui(ui, &mut clip.color, undo);
    }

    if let Some((style, transform)) = apply_to_all {
        for c in tl.track_mut(track).clips.iter_mut() {
            if let Some(s) = c.text.as_mut() {
                // Cada subtítulo conserva su texto y los tiempos de sus palabras.
                let starts = s.words.take().map(|w| w.starts).unwrap_or_default();
                let words = style.words.clone().map(|w| snipsy_core::WordAnimation { starts, ..w });
                *s = snipsy_core::TextStyle { text: std::mem::take(&mut s.text), words, ..style.clone() };
                c.transform = transform;
            }
        }
    }

    // La disolvencia del video también funde el audio vinculado.
    for l in links {
        if l != id {
            if let Some(c) = tl.clip_mut(l) {
                if c.transition_in != transition {
                    c.transition_in = transition;
                    out.rebuild = true;
                }
            }
        }
    }
}

fn prop_label(p: Prop) -> &'static str {
    match p {
        Prop::X => "Posición X",
        Prop::Y => "Posición Y",
        Prop::Scale => "Escala",
        Prop::Rotation => "Rotación",
        Prop::Opacity => "Opacidad",
        Prop::Volume => "Volumen",
    }
}

fn percent_slider(ui: &mut Ui, v: &mut f32, range: std::ops::RangeInclusive<f32>, clamp: bool) -> egui::Response {
    let mut pct = *v * 100.0;
    let clamping = if clamp { egui::SliderClamping::Always } else { egui::SliderClamping::Never };
    let r = ui.add(egui::Slider::new(&mut pct, range).suffix(" %").clamping(clamping));
    if r.changed() {
        *v = pct / 100.0;
    }
    r
}

/// Duración en segundos, guardada en cuadros.
fn seconds(ui: &mut Ui, frames: &mut Frame, max: Frame, fps: f64) -> egui::Response {
    let mut s = *frames as f64 / fps;
    let r = ui.add(egui::DragValue::new(&mut s).range(0.0..=max as f64 / fps).speed(0.02).max_decimals(2).suffix(" s"));
    if r.changed() {
        *frames = ((s * fps).round() as Frame).clamp(0, max);
    }
    r
}

/// Rombo de keyframe: ◆ hay keyframe aquí (clic = quitar), ◇ añadir.
fn key_button(ui: &mut Ui, clip: &mut Clip, p: Prop, playhead: Frame, inside: bool, undo: &mut Undo) {
    let at = clip.source_frame(playhead);
    let curve = clip.anim.curve(p);
    let here = curve.key_at(at).copied();
    let (glyph, color) = match (here.is_some(), curve.is_animated()) {
        (true, _) => ("◆", KEY_ON),
        (false, true) => ("◇", KEY_ON),
        (false, false) => ("◇", Color32::GRAY),
    };
    let hover = if !inside {
        "Mueve el cursor de reproducción sobre el clip para crear keyframes"
    } else if here.is_some() {
        "Quitar el keyframe (clic derecho: tipo de curva)"
    } else {
        "Crear un keyframe con el valor actual"
    };
    let r = ui.add_enabled(inside, egui::Button::new(RichText::new(glyph).color(color)).frame(false)).on_hover_text(hover);
    if undo.clicked(&r) {
        let value = clip.prop_at(p, playhead);
        let curve = clip.anim.curve_mut(p);
        if here.is_some() {
            curve.remove(at);
            // Sin keyframes, la propiedad conserva el valor que tenía en este cuadro.
            if !curve.is_animated() {
                clip.set_prop(p, playhead, value);
            }
        } else {
            curve.set(at, value);
        }
    }
    if here.is_some() {
        r.context_menu(|ui| {
            for ease in Ease::ALL {
                let selected = here.is_some_and(|k| k.ease == ease);
                let r = ui.selectable_label(selected, ease.label());
                if undo.clicked(&r) {
                    if let Some(k) = clip.anim.curve_mut(p).keys.iter_mut().find(|k| k.at == at) {
                        k.ease = ease;
                    }
                    ui.close();
                }
            }
        });
    }
}

/// Saltar al keyframe anterior/siguiente del clip.
fn keyframe_nav(ui: &mut Ui, clip: &Clip, playhead: Frame, out: &mut InspectorOutput) {
    let times: Vec<Frame> = clip
        .anim
        .key_times()
        .into_iter()
        .map(|at| clip.timeline_frame(at))
        .filter(|f| clip.contains(*f))
        .collect();
    if times.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        let prev = times.iter().rev().find(|f| **f < playhead).copied();
        let next = times.iter().find(|f| **f > playhead).copied();
        if ui.add_enabled(prev.is_some(), egui::Button::new("◀ Keyframe")).clicked() {
            out.seek = prev;
        }
        if ui.add_enabled(next.is_some(), egui::Button::new("Keyframe ▶")).clicked() {
            out.seek = next;
        }
        hint(ui, &format!("{} keyframes", times.len()));
    });
}

fn clear_geometry(anim: &mut Animation) {
    for p in [Prop::X, Prop::Y, Prop::Scale, Prop::Rotation] {
        anim.curve_mut(p).keys.clear();
    }
}

/// Quita todos los keyframes conservando los valores del cuadro actual.
fn remove_animation(clip: &mut Clip, playhead: Frame) {
    let frame = playhead.clamp(clip.start, clip.end() - 1);
    let t = clip.transform_at(frame);
    let volume = clip.prop_at(Prop::Volume, frame);
    clip.anim = Animation::default();
    clip.transform = t;
    clip.volume_db = volume;
}

fn preset_slow_zoom(clip: &mut Clip) {
    let s = clip.prop_at(Prop::Scale, clip.start);
    let curve = &mut clip.anim.scale;
    curve.keys.clear();
    curve.set(clip.source_in, s);
    curve.set(clip.source_in + clip.len - 1, s * 1.15);
    curve.keys[0].ease = Ease::Linear;
}

fn preset_slide_in(clip: &mut Clip, project_h: f32, fps: f64) {
    let y = clip.prop_at(Prop::Y, clip.start + clip.len - 1);
    let d = ((fps * 0.5).round() as Frame).clamp(1, (clip.len - 1).max(1));
    let curve = &mut clip.anim.y;
    curve.keys.clear();
    curve.set(clip.source_in, y + project_h);
    curve.set(clip.source_in + d, y);
}

fn preset_pop(clip: &mut Clip, fps: f64) {
    let s = clip.prop_at(Prop::Scale, clip.start + clip.len - 1);
    let f = |secs: f64| ((fps * secs).round() as Frame).min((clip.len - 1).max(0));
    let (a, b) = (f(0.2), f(0.33));
    let at = clip.source_in;
    let curve = &mut clip.anim.scale;
    curve.keys.clear();
    curve.set(at, 0.01);
    if a > 0 && b > a {
        curve.set(at + a, s * 1.12);
        curve.set(at + b, s);
    } else {
        curve.set(at + b.max(1), s);
    }
}

fn color_ui(ui: &mut Ui, color: &mut ColorGrade, undo: &mut Undo) {
    section(ui, "Color");
    egui::Grid::new("color").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        let mut row = |ui: &mut Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>| {
            ui.label(label);
            let r = ui.add(egui::Slider::new(v, range).fixed_decimals(2));
            undo.track(&r);
            ui.end_row();
        };
        row(ui, "Exposición", &mut color.exposure, -3.0..=3.0);
        row(ui, "Contraste", &mut color.contrast, 0.0..=2.0);
        row(ui, "Saturación", &mut color.saturation, 0.0..=2.0);
        row(ui, "Temperatura", &mut color.temperature, -1.0..=1.0);
        row(ui, "Tinte", &mut color.tint, -1.0..=1.0);
    });

    ui.add_space(4.0);
    let error_id = ui.scope_id().with("lut_error");
    ui.horizontal(|ui| {
        ui.label("LUT");
        let name = color.lut.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        let r = ui.button(name.as_deref().unwrap_or("Cargar .cube…"));
        if r.clicked() {
            if let Some(path) = rfd::FileDialog::new().add_filter("LUT 3D", &["cube"]).pick_file() {
                match snipsy_render::Lut::load(&path) {
                    Ok(_) => {
                        undo.checkpoint();
                        undo.changed = true;
                        color.lut = Some(path);
                        ui.data_mut(|d| {
                            d.remove::<String>(error_id);
                        });
                    }
                    Err(e) => {
                        ui.data_mut(|d| d.insert_temp(error_id, e));
                    }
                }
            }
        }
        if color.lut.is_some() {
            let r = ui.small_button("✕").on_hover_text("Quitar la LUT");
            if undo.clicked(&r) {
                color.lut = None;
            }
        }
    });
    if let Some(e) = ui.data(|d| d.get_temp::<String>(error_id)) {
        ui.label(RichText::new(e).small().color(Color32::from_rgb(230, 110, 90)));
    }
    if color.lut.is_some() {
        ui.horizontal(|ui| {
            ui.label("Intensidad");
            let r = percent_slider(ui, &mut color.lut_mix, 0.0..=100.0, true);
            undo.track(&r);
        });
    }
    if !color.is_neutral() || color.lut.is_some() {
        let r = ui.button("Restablecer color");
        if undo.clicked(&r) {
            *color = ColorGrade::default();
        }
    }
}

// ---------------------------------------------------------------- audio

fn audio_ui(ui: &mut Ui, tl: &mut Timeline, id: ClipId, standalone: bool, playhead: Frame, undo: &mut Undo, out: &mut InspectorOutput) {
    let fps = tl.fps;
    let Some((track, _)) = tl.find(id) else { return };
    let index = tl.track(track).clips.iter().position(|c| c.id == id).unwrap_or(0);
    let has_prev = index > 0 && tl.track(track).clips[index - 1].end() == tl.track(track).clips[index].start;
    let Some(clip) = tl.clip_mut(id) else { return };
    let before = clip.clone();

    section(ui, if standalone { "Audio" } else { "Audio vinculado" });
    let inside = clip.contains(playhead);
    egui::Grid::new("audio").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Volumen");
        let mut db = clip.prop_at(Prop::Volume, playhead);
        let r = ui.add(egui::Slider::new(&mut db, -60.0..=12.0).suffix(" dB").fixed_decimals(1).custom_formatter(|v, _| {
            if v <= -60.0 { "−∞".to_owned() } else { format!("{v:.1}") }
        }));
        if r.changed() {
            clip.set_prop(Prop::Volume, playhead, db);
        }
        undo.track(&r);
        key_button(ui, clip, Prop::Volume, playhead, inside, undo);
        ui.end_row();

        ui.label("Fundido de entrada");
        let r = seconds(ui, &mut clip.fade_in, clip.len, fps);
        undo.track(&r);
        ui.label("");
        ui.end_row();
        ui.label("Fundido de salida");
        let r = seconds(ui, &mut clip.fade_out, clip.len, fps);
        undo.track(&r);
        ui.label("");
        ui.end_row();
        if standalone {
            ui.label("Fundido cruzado");
            ui.add_enabled_ui(has_prev, |ui| {
                let r = seconds(ui, &mut clip.transition_in, clip.len, fps).on_hover_text("Con el clip anterior, si están pegados en la misma pista");
                undo.track(&r);
            });
            ui.end_row();
        }
    });
    ui.horizontal(|ui| {
        let r = ui.small_button("Silenciar");
        if undo.clicked(&r) {
            clip.anim.volume.keys.clear();
            clip.volume_db = -60.0;
        }
        let r = ui.small_button("0 dB");
        if undo.clicked(&r) {
            clip.anim.volume.keys.clear();
            clip.volume_db = 0.0;
        }
    });
    // Todo cambio de audio requiere volver a preparar el mezclador.
    if *clip != before {
        out.rebuild = true;
    }
    if standalone {
        ducking_ui(ui, tl, track, undo, out);
    }
}

/// Corta los silencios del timeline (sin tocar la pista `music` ni el logo) y
/// devuelve un mensaje para el usuario.
pub fn cut_silences(tl: &mut Timeline, s: &snipsy_core::SilenceSettings, music: Option<usize>) -> String {
    use snipsy_core::silence;
    let music = music.filter(|m| *m < tl.audio_tracks.len());
    let exclude = music.map(TrackRef::audio);
    let levels = snipsy_media::loudness::levels(tl, exclude);
    let covered = silence::audio_coverage(tl, exclude);
    let ranges = silence::silent_ranges(&levels, &covered, tl.fps, s);
    let ducked = music.is_some_and(|m| tl.audio_tracks[m].clips.iter().any(|c| !c.anim.volume.keys.is_empty()));
    let keep = silence::continuous_tracks(tl, music);
    let removed = silence::cut_ranges(tl, &ranges, &keep);
    if ranges.is_empty() {
        return "No se encontraron pausas para cortar (prueba con menos sensibilidad o una pausa mínima más corta).".into();
    }
    let mut m = format!("✂ {} pausa(s) cortada(s): {:.1} s menos. ⌘Z para deshacer.", ranges.len(), tl.frame_to_secs(removed));
    if ducked {
        m.push_str(" Vuelve a aplicar el ducking de la música.");
    }
    m
}

fn silence_ui(ui: &mut Ui, tl: &mut Timeline, undo: &mut Undo, out: &mut InspectorOutput) {
    use snipsy_core::{silence, SilenceSettings};
    section(ui, "✂ Cortar silencios");
    let id = ui.make_persistent_id("silence_settings");
    let mut s: SilenceSettings = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    // `None` = adivinar la pista de música; `Some(None)` = ninguna.
    let music_id = ui.make_persistent_id("silence_music");
    let mut music: Option<Option<usize>> = ui.data_mut(|d| d.get_temp(music_id)).flatten();
    let current_music = music.unwrap_or_else(|| silence::guess_music_track(tl)).filter(|m| *m < tl.audio_tracks.len());
    egui::Grid::new("silence").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Sensibilidad");
        let mut sens = -s.threshold_db;
        ui.add(egui::Slider::new(&mut sens, 25.0..=60.0).show_value(false))
            .on_hover_text("Súbela si corta palabras dichas en voz baja; bájala si no corta pausas con ruido de fondo");
        s.threshold_db = -sens;
        ui.end_row();
        ui.label("Pausa mínima");
        ui.add(egui::Slider::new(&mut s.min_silence, 0.3..=2.0).suffix(" s").fixed_decimals(1))
            .on_hover_text("Solo se cortan pausas al menos así de largas");
        ui.end_row();
        ui.label("Aire");
        ui.add(egui::Slider::new(&mut s.padding, 0.0..=0.5).suffix(" s").fixed_decimals(2))
            .on_hover_text("Lo que se deja antes y después de cada frase para que no suene cortado");
        ui.end_row();
        ui.label("Música");
        let name = |i: usize| format!("A{} {}", i + 1, tl.audio_tracks[i].name);
        egui::ComboBox::from_id_salt("silence_music_combo")
            .selected_text(current_music.map(name).unwrap_or_else(|| "Ninguna".into()))
            .show_ui(ui, |ui| {
                if ui.selectable_label(current_music.is_none(), "Ninguna").clicked() {
                    music = Some(None);
                }
                for i in 0..tl.audio_tracks.len() {
                    if ui.selectable_label(current_music == Some(i), name(i)).clicked() {
                        music = Some(Some(i));
                    }
                }
            })
            .response
            .on_hover_text("Esta pista no se corta ni cuenta como voz: sigue sonando de corrido");
        ui.end_row();
    });
    ui.data_mut(|d| {
        d.insert_temp(id, s);
        d.insert_temp(music_id, music);
    });
    let r = ui.button("✂ Cortar silencios").on_hover_text(
        "Quita las pausas de todas las pistas y cierra los huecos (video, voz, textos y subtítulos). \
         La música y el logo no se cortan. ⌘Z para deshacer.",
    );
    if undo.clicked(&r) {
        out.message = Some(cut_silences(tl, &s, current_music));
        out.rebuild = true;
    }
    hint(ui, "Ideal para videos hablados a cámara. Hazlo antes de agregar subtítulos, música o ducking.");
}

fn ducking_ui(ui: &mut Ui, tl: &mut Timeline, track: TrackRef, undo: &mut Undo, out: &mut InspectorOutput) {
    use snipsy_core::{ducking, DuckSettings};
    section(ui, "Ducking: bajar esta pista cuando hablan");
    let id = ui.make_persistent_id("duck_settings");
    let mut s: DuckSettings = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    egui::Grid::new("duck").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Bajar");
        ui.add(egui::Slider::new(&mut s.amount_db, 3.0..=30.0).suffix(" dB").fixed_decimals(0));
        ui.end_row();
        ui.label("Sensibilidad");
        // Más sensibilidad = umbral más bajo (detecta voces más suaves).
        let mut sens = -s.threshold_db;
        ui.add(egui::Slider::new(&mut sens, 25.0..=60.0).show_value(false))
            .on_hover_text("Súbela si no detecta una voz suave; bájala si la música baja con ruidos de fondo");
        s.threshold_db = -sens;
        ui.end_row();
        ui.label("Volver a subir");
        ui.add(egui::Slider::new(&mut s.release, 0.2..=2.0).suffix(" s").fixed_decimals(1));
        ui.end_row();
    });
    ui.data_mut(|d| d.insert_temp(id, s));
    ui.horizontal(|ui| {
        let r = ui.button("🎙 Aplicar ducking").on_hover_text(
            "Analiza las otras pistas de audio (voz, entrevistas) y baja esta pista mientras se habla, con keyframes de volumen que puedes retocar",
        );
        if undo.clicked(&r) {
            let levels = snipsy_media::loudness::levels_without(tl, track);
            let regions = ducking::voice_regions(&levels, tl.fps, &s);
            let n = ducking::apply_ducking(tl, track, &regions, &s);
            out.rebuild = true;
            let secs: f64 = regions.iter().map(|(a, b)| tl.frame_to_secs(b - a)).sum();
            out.message = Some(if regions.is_empty() {
                "No se detectó voz en las otras pistas (prueba con más sensibilidad).".into()
            } else {
                format!("🎙 Ducking aplicado: {} tramo(s) con voz ({secs:.1} s) en {n} clip(s). Si cambias la voz, vuelve a aplicarlo.", regions.len())
            });
        }
        let has_keys = tl.track(track).clips.iter().any(|c| !c.anim.volume.keys.is_empty());
        if has_keys {
            let r = ui.button("Quitar");
            if undo.clicked(&r) {
                for c in &mut tl.track_mut(track).clips {
                    c.anim.volume.keys.clear();
                }
                out.rebuild = true;
            }
        }
    });
    hint(ui, "Pon la música en su propia pista de audio. Se aplica a todos los clips de esa pista.");
}

fn color_row(ui: &mut Ui, label: &str, color: &mut [u8; 4], undo: &mut Undo) {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut c = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
        let r = egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::OnlyBlend);
        if r.changed() {
            *color = c.to_srgba_unmultiplied();
        }
        undo.track(&r);
    });
}

fn optional_color_row(ui: &mut Ui, label: &str, value: &mut Option<[u8; 4]>, default: [u8; 4], undo: &mut Undo) {
    ui.horizontal(|ui| {
        let mut on = value.is_some();
        let r = ui.checkbox(&mut on, label);
        if r.changed() {
            *value = on.then_some(value.unwrap_or(default));
        }
        undo.track(&r);
        if let Some(color) = value.as_mut() {
            let mut c = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
            let r = egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::OnlyBlend);
            if r.changed() {
                *color = c.to_srgba_unmultiplied();
            }
            undo.track(&r);
        }
    });
}
