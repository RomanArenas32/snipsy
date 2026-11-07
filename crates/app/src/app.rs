//! Interfaz principal: media pool, visor, transporte y línea de tiempo.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, Key, Modifiers, RichText};
use snipsy_audio::AudioOutput;
use snipsy_core::compose::FORMATS;
use snipsy_core::timecode::format_timecode;
use snipsy_core::{project, Asset, AssetId, ClipId, Frame, History, TextStyle, Timeline};
use snipsy_export::{ExportEvent, ExportJob, ExportSettings, Quality, VideoCodec};
use snipsy_media::MediaInfo;

use crate::brand_ui::{BrandAction, BrandUi};
use crate::inspector;
use crate::player::Player;
use crate::proxies_ui::Proxies;
use crate::subtitles_ui::{SubtitleResult, SubtitlesUi};
use crate::timeline_ui::{TimelineView, Tool};
use crate::viewer::Viewer;

const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "webm", "avi", "mxf", "ts", "mts", "mp3", "wav", "m4a", "aac", "png", "jpg", "jpeg", "webp",
];
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp"];
/// Duración inicial de una imagen en el timeline (se puede alargar sin límite).
const STILL_SECS: f64 = 5.0;

fn is_image(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|e| IMAGE_EXTS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// Opciones elegidas en el diálogo de exportación.
struct ExportDialog {
    codec: VideoCodec,
    quality: Quality,
    hardware: bool,
}

impl Default for ExportDialog {
    fn default() -> Self {
        Self { codec: VideoCodec::H264, quality: Quality::High, hardware: true }
    }
}

struct RunningExport {
    job: ExportJob,
    path: PathBuf,
    progress: (Frame, Frame),
    eta: f64,
    encoder: String,
}

pub struct SnipsyApp {
    timeline: Timeline,
    history: History,
    view: TimelineView,
    player: Player,
    project_path: Option<PathBuf>,
    dirty: bool,
    title: String,
    viewer: Viewer,
    status: String,
    export_dialog: Option<ExportDialog>,
    export: Option<RunningExport>,
    last_export: Option<PathBuf>,
    subtitles: SubtitlesUi,
    brand: BrandUi,
    proxies: Proxies,
}

impl SnipsyApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Vec<PathBuf>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (audio, status) = match AudioOutput::new() {
            Ok(a) => (Some(Arc::new(a)), "Importa videos para empezar (o arrástralos a la ventana).".to_owned()),
            Err(e) => (None, format!("Sin audio: {e}")),
        };
        let repaint_ctx = cc.egui_ctx.clone();
        let player = Player::new(audio, Arc::new(move || repaint_ctx.request_repaint()));
        let proxy_ctx = cc.egui_ctx.clone();
        let proxies = Proxies::new(Arc::new(move || proxy_ctx.request_repaint()));
        let mut app = Self {
            timeline: Timeline::new(),
            history: History::default(),
            view: TimelineView::default(),
            player,
            project_path: None,
            dirty: false,
            title: String::new(),
            viewer: Viewer::new(cc),
            status,
            export_dialog: None,
            export: None,
            last_export: None,
            subtitles: SubtitlesUi::default(),
            brand: BrandUi::new(),
            proxies,
        };
        if !app.viewer.has_gpu() {
            app.status = "Aviso: no hay GPU (wgpu); el visor no podrá componer.".into();
        }
        app.refresh_player();

        let (projects, media): (Vec<_>, Vec<_>) =
            initial.into_iter().partition(|p| p.extension().is_some_and(|e| e == "snipsy"));
        if let Some(p) = projects.into_iter().next() {
            app.load_project(p);
        } else if !media.is_empty() {
            app.import(media);
        }
        app
    }

    /// El timeline cambió: avisar al reproductor y marcar el proyecto como modificado.
    fn commit(&mut self) {
        self.refresh_player();
        self.dirty = true;
    }

    /// Reconstruye el reproductor (con los proxies listos, si están activados).
    fn refresh_player(&mut self) {
        self.proxies.scan(&self.timeline);
        let proxies = self.proxies.active(&self.timeline);
        self.player.set_timeline(&self.timeline, &proxies);
    }

    fn playhead(&self) -> Frame {
        self.timeline.secs_to_frame(self.player.position())
    }

    fn seek_frame(&mut self, frame: Frame) {
        self.player.seek(self.timeline.frame_to_secs(frame.max(0)));
    }

    // ------------------------------------------------------------ archivos

    fn import(&mut self, paths: Vec<PathBuf>) {
        let was_empty = self.timeline.duration() == 0;
        let mut added: Vec<AssetId> = Vec::new();
        let mut errors = Vec::new();
        for path in paths {
            if is_image(&path) {
                match self.add_image(path) {
                    Ok(id) => added.push(id),
                    Err(e) => errors.push(e),
                }
                continue;
            }
            match MediaInfo::probe(&path) {
                Ok(info) if info.has_video || info.has_audio => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let id = self.timeline.add_asset(Asset {
                        id: AssetId(0),
                        path,
                        name,
                        duration: info.duration,
                        fps: info.fps,
                        width: info.width,
                        height: info.height,
                        has_video: info.has_video,
                        has_audio: info.has_audio,
                        still: false,
                    });
                    added.push(id);
                }
                Ok(_) => errors.push(format!("{}: sin video ni audio", path.display())),
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        if added.is_empty() && errors.is_empty() {
            return;
        }
        // Con el timeline vacío, poner lo importado en orden para empezar rápido.
        if was_empty && !added.is_empty() {
            self.history.checkpoint(&self.timeline);
            for id in &added {
                self.timeline.append_asset(*id);
            }
            self.view.zoom_to_fit(&self.timeline);
        }
        self.commit();
        self.status = if errors.is_empty() {
            format!(
                "{} archivo(s) importado(s) · proyecto {}x{} @ {:.2} fps",
                added.len(),
                self.timeline.width,
                self.timeline.height,
                self.timeline.fps
            )
        } else {
            format!("Errores al importar: {}", errors.join(" · "))
        };
    }

    /// Agrega una imagen fija (logo, foto) al proyecto.
    fn add_image(&mut self, path: PathBuf) -> Result<AssetId, String> {
        let img = snipsy_render::load_image(&path)?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(self.timeline.add_asset(Asset {
            id: AssetId(0),
            path,
            name,
            duration: STILL_SECS,
            fps: self.timeline.fps,
            width: img.width as u32,
            height: img.height as u32,
            has_video: true,
            has_audio: false,
            still: true,
        }))
    }

    fn brand_action(&mut self, action: BrandAction) {
        match action {
            BrandAction::PlaceLogo(kit) => {
                let Some(path) = kit.logo.clone() else { return };
                let before = self.timeline.clone();
                let placed = self.add_image(path).and_then(|asset| kit.place_logo(&mut self.timeline, asset).ok_or_else(|| "no se pudo colocar".to_owned()));
                match placed {
                    Ok(id) => {
                        self.history.checkpoint(&before);
                        self.view.selection = [id].into_iter().collect();
                        self.commit();
                        self.status = "✅ Logo colocado en la pista «Logo». Si alargas el video, pulsa «Poner logo» otra vez.".into();
                    }
                    Err(e) => {
                        self.timeline = before;
                        self.status = format!("No se pudo poner el logo: {e}");
                    }
                }
            }
            BrandAction::RestyleAll(kit) => {
                let before = self.timeline.clone();
                let n = kit.restyle_all(&mut self.timeline);
                if n > 0 {
                    self.history.checkpoint(&before);
                    self.commit();
                }
                self.status = format!("Marca «{}» aplicada a {n} texto(s).", kit.name);
            }
            BrandAction::Insert(kit, preset) => {
                let before = self.timeline.clone();
                let at = self.playhead();
                let id = kit.insert_preset(preset, &mut self.timeline, at);
                self.history.checkpoint(&before);
                self.view.selection = [id].into_iter().collect();
                self.commit();
                self.status = format!("«{}» insertado: edita el texto en el inspector.", preset.label());
            }
        }
    }

    fn pick_import(&mut self) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter("Medios", VIDEO_EXTS)
            .add_filter("Todos", &["*"])
            .pick_files()
        {
            self.import(paths);
        }
    }

    fn confirm_discard(&self) -> bool {
        !self.dirty
            || rfd::MessageDialog::new()
                .set_title("Cambios sin guardar")
                .set_description("El proyecto tiene cambios sin guardar. ¿Descartarlos?")
                .set_buttons(rfd::MessageButtons::YesNo)
                .show()
                == rfd::MessageDialogResult::Yes
    }

    fn new_project(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        self.timeline = Timeline::new();
        self.history.clear();
        self.view = TimelineView::default();
        self.project_path = None;
        self.player.seek(0.0);
        self.commit();
        self.dirty = false;
        self.status = "Proyecto nuevo.".into();
    }

    fn open_project_dialog(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new().add_filter("Proyecto Snipsy", &["snipsy"]).pick_file() {
            self.load_project(path);
        }
    }

    fn load_project(&mut self, path: PathBuf) {
        match project::load(&path) {
            Ok(tl) => {
                let missing: Vec<String> =
                    tl.assets.iter().filter(|a| !a.path.exists()).map(|a| a.name.clone()).collect();
                self.timeline = tl;
                self.history.clear();
                self.view = TimelineView::default();
                self.view.zoom_to_fit(&self.timeline);
                self.player.pause();
                self.player.seek(0.0);
                self.commit();
                self.dirty = false;
                self.status = if missing.is_empty() {
                    format!("Proyecto abierto: {}", path.display())
                } else {
                    format!("Faltan archivos: {}", missing.join(", "))
                };
                self.project_path = Some(path);
            }
            Err(e) => self.status = format!("No se pudo abrir el proyecto: {e}"),
        }
    }

    fn save_project(&mut self, save_as: bool) {
        let path = match (&self.project_path, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let Some(mut p) = rfd::FileDialog::new()
                    .add_filter("Proyecto Snipsy", &["snipsy"])
                    .set_file_name("Proyecto.snipsy")
                    .save_file()
                else {
                    return;
                };
                if p.extension().is_none() {
                    p.set_extension("snipsy");
                }
                p
            }
        };
        match project::save(&path, &self.timeline) {
            Ok(()) => {
                self.status = format!("Guardado: {}", path.display());
                self.project_path = Some(path);
                self.dirty = false;
            }
            Err(e) => self.status = format!("No se pudo guardar: {e}"),
        }
    }

    // ------------------------------------------------------------ ediciones

    fn undo(&mut self) {
        if self.history.undo(&mut self.timeline) {
            self.commit();
        }
    }

    fn redo(&mut self) {
        if self.history.redo(&mut self.timeline) {
            self.commit();
        }
    }

    fn split_at_playhead(&mut self) {
        let at = self.playhead();
        let sel: Vec<ClipId> = self.view.selection.iter().copied().collect();
        let before = self.timeline.clone();
        let ids = (!sel.is_empty()).then_some(sel.as_slice());
        if self.timeline.split(at, ids) {
            self.history.checkpoint(&before);
            self.commit();
        }
    }

    fn delete_selection(&mut self, ripple: bool) {
        let sel: Vec<ClipId> = self.view.selection.iter().copied().collect();
        if sel.is_empty() {
            return;
        }
        let before = self.timeline.clone();
        if self.timeline.delete(&sel, ripple) {
            self.history.checkpoint(&before);
            self.view.selection.clear();
            self.commit();
        }
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect()
        });
        if !dropped.is_empty() {
            if let Some(proj) = dropped.iter().find(|p| p.extension().is_some_and(|e| e == "snipsy")) {
                if self.confirm_discard() {
                    self.load_project(proj.clone());
                }
            } else {
                self.import(dropped);
            }
        }

        // Atajos con Cmd (primero las variantes con Shift: Cmd+Z también coincide con Cmd+Shift+Z).
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let consume = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if consume(cmd_shift, Key::Z) {
            self.redo();
        } else if consume(Modifiers::COMMAND, Key::Z) {
            self.undo();
        }
        if consume(cmd_shift, Key::S) {
            self.save_project(true);
        } else if consume(Modifiers::COMMAND, Key::S) {
            self.save_project(false);
        }
        if consume(Modifiers::COMMAND, Key::O) {
            self.open_project_dialog();
        }
        if consume(Modifiers::COMMAND, Key::I) {
            self.pick_import();
        }
        if consume(Modifiers::COMMAND, Key::E) {
            self.open_export_dialog();
        }
        if consume(Modifiers::COMMAND, Key::B) {
            self.split_at_playhead();
        }

        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k) && !i.modifiers.command);
        let shift = ctx.input(|i| i.modifiers.shift);
        // Espacio y flechas se consumen para que no activen ni muevan el foco de los botones.
        if consume(Modifiers::NONE, Key::Space) || pressed(Key::K) {
            self.player.toggle();
        }
        if consume(Modifiers::NONE, Key::ArrowRight) {
            self.player.step_forward();
        }
        if consume(Modifiers::NONE, Key::ArrowLeft) {
            self.player.step_backward();
        }
        if pressed(Key::Home) {
            self.player.seek(0.0);
        }
        if pressed(Key::End) {
            self.player.seek(self.player.duration());
        }
        if pressed(Key::A) {
            self.view.tool = Tool::Select;
        }
        if pressed(Key::B) {
            self.view.tool = Tool::Blade;
        }
        if pressed(Key::T) {
            self.add_text();
        }
        if pressed(Key::N) {
            self.view.snapping = !self.view.snapping;
        }
        if pressed(Key::Z) && shift {
            self.view.zoom_to_fit(&self.timeline);
        }
        if pressed(Key::Equals) || pressed(Key::Plus) {
            self.view.zoom(1.5, Some(self.playhead() as f64));
        }
        if pressed(Key::Minus) {
            self.view.zoom(1.0 / 1.5, Some(self.playhead() as f64));
        }
        if pressed(Key::Delete) || pressed(Key::Backspace) {
            self.delete_selection(shift);
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let name = self
            .project_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Sin título".into());
        let title = format!("{}{} — Snipsy", name, if self.dirty { " •" } else { "" });
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

impl eframe::App for SnipsyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_input(&ctx);

        if let Some(frame) = self.player.tick() {
            self.viewer.set_frame(frame);
        }
        let (message, ready) = self.proxies.poll(&self.timeline);
        if let Some(m) = message {
            self.status = m;
        }
        if ready && self.proxies.enabled {
            self.refresh_player();
        }
        if let Some(e) = self.player.error.take() {
            self.status = format!("Error: {e}");
        }
        if self.player.is_playing() {
            self.view.follow(self.playhead());
            ctx.request_repaint();
        }
        self.update_title(&ctx);

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(RichText::new("Snipsy").strong());
                ui.separator();
                if ui.button("📄 Nuevo").clicked() {
                    self.new_project();
                }
                if ui.button("📂 Abrir").on_hover_text("⌘O").clicked() {
                    self.open_project_dialog();
                }
                if ui.button("💾 Guardar").on_hover_text("⌘S").clicked() {
                    self.save_project(false);
                }
                ui.separator();
                if ui.button("📥 Importar medios…").on_hover_text("⌘I").clicked() {
                    self.pick_import();
                }
                if ui.button("🅣 Texto").on_hover_text("Agregar un título en el cabezal (T)").clicked() {
                    self.add_text();
                }
                let can_subs = !self.subtitles.is_busy() && self.timeline.duration() > 0;
                if ui
                    .add_enabled(can_subs, egui::Button::new("💬 Subtítulos"))
                    .on_hover_text("Transcribir la voz y crear subtítulos automáticamente")
                    .clicked()
                {
                    self.player.pause();
                    self.subtitles.open();
                }
                if ui.button("🎨 Marca").on_hover_text("Logo, colores y tipografía de la empresa").clicked() {
                    self.brand.open();
                }
                ui.separator();
                self.format_combo(ui);
                let r = ui.checkbox(&mut self.proxies.enabled, "⚡ Proxies").on_hover_text(
                    "Vista previa con copias livianas (720p) de los videos 4K. Se crean solas al importar; al exportar siempre se usa el original.",
                );
                if r.changed() {
                    self.refresh_player();
                }
                ui.separator();
                let can_export = self.export.is_none() && self.timeline.duration() > 0;
                if ui.add_enabled(can_export, egui::Button::new("📤 Exportar")).on_hover_text("⌘E").clicked() {
                    self.open_export_dialog();
                }
                ui.separator();
                if ui.add_enabled(self.history.can_undo(), egui::Button::new("↶")).on_hover_text("Deshacer ⌘Z").clicked() {
                    self.undo();
                }
                if ui.add_enabled(self.history.can_redo(), egui::Button::new("↷")).on_hover_text("Rehacer ⇧⌘Z").clicked() {
                    self.redo();
                }
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.status).small().color(Color32::GRAY));
                if let Some(path) = self.last_export.clone() {
                    if ui.small_button("Mostrar en Finder").clicked() {
                        let _ = std::process::Command::new("open").arg("-R").arg(&path).spawn();
                    }
                    if ui.small_button("Reproducir").clicked() {
                        let _ = std::process::Command::new("open").arg(&path).spawn();
                    }
                }
            });
        });
        self.export_windows(&ctx);
        if let Some(action) = self.brand.show(&ctx) {
            self.player.pause();
            self.brand_action(action);
        }
        match self.subtitles.show(&ctx, &self.timeline) {
            Some(SubtitleResult::Apply(captions, look)) => {
                if captions.is_empty() {
                    self.status = "No se reconoció voz en el audio del timeline.".into();
                } else {
                    self.history.checkpoint(&self.timeline);
                    let n = snipsy_subtitles::apply(&mut self.timeline, &captions, &look);
                    if let (true, Some(kit)) = (self.brand.use_for_subtitles, self.brand.active()) {
                        if let Some(i) = snipsy_subtitles::subtitle_track(&self.timeline) {
                            for clip in &mut self.timeline.video_tracks[i].clips {
                                if let Some(style) = &mut clip.text {
                                    kit.restyle(style);
                                }
                            }
                        }
                    }
                    self.view.selection.clear();
                    self.commit();
                    self.status = format!("✅ {n} subtítulos creados. Edita cualquiera en el inspector; «Aplicar estilo a todos» copia su formato.");
                }
            }
            Some(SubtitleResult::Message(m)) => self.status = m,
            None => {}
        }

        egui::Panel::bottom("timeline").resizable(true).default_size(300.0).min_size(160.0).show(ui, |ui| {
            let playhead = self.playhead();
            let out = self.view.show(ui, &mut self.timeline, &mut self.history, playhead);
            if out.changed {
                self.commit();
            }
            if let Some(f) = out.seek {
                self.seek_frame(f);
            }
        });

        egui::Panel::left("media_pool").resizable(true).default_size(240.0).show(ui, |ui| {
            self.media_pool(ui);
        });

        egui::Panel::right("inspector").resizable(true).default_size(290.0).show(ui, |ui| {
            let playhead = self.playhead();
            let out = inspector::show(ui, &mut self.timeline, &mut self.history, &self.view.selection, playhead);
            if out.rebuild {
                self.commit();
            } else if out.changed {
                self.dirty = true;
            }
            if let Some(f) = out.seek {
                self.seek_frame(f);
            }
        });

        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(Color32::from_gray(12))).show(ui, |ui| {
            egui::Panel::bottom("transport").frame(egui::Frame::side_top_panel(ui.style())).show(ui, |ui| {
                ui.add_space(4.0);
                self.transport(ui);
                ui.add_space(4.0);
            });
            // Cambiar transformaciones no requiere decodificar de nuevo: solo se recompone.
            let playhead = self.playhead();
            if self.viewer.show(ui, &mut self.timeline, &mut self.history, &mut self.view.selection, playhead) {
                self.dirty = true;
            }
        });
    }
}

impl SnipsyApp {
    // ------------------------------------------------------------ exportación

    fn open_export_dialog(&mut self) {
        if self.export.is_none() && self.timeline.duration() > 0 {
            self.export_dialog.get_or_insert_with(ExportDialog::default);
        }
    }

    fn start_export(&mut self) {
        let Some(opts) = self.export_dialog.take() else { return };
        let name = snipsy_export::suggested_name(self.project_path.as_deref());
        let mut dialog = rfd::FileDialog::new().add_filter("Video MP4", &["mp4"]).set_file_name(name);
        if let Some(dir) = self.project_path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut path) = dialog.save_file() else {
            self.export_dialog = Some(opts);
            return;
        };
        if path.extension().is_none() {
            path.set_extension("mp4");
        }
        self.player.pause();
        let settings = ExportSettings { path: path.clone(), codec: opts.codec, quality: opts.quality, hardware: opts.hardware };
        let total = self.timeline.duration();
        self.export = Some(RunningExport {
            job: ExportJob::start(self.timeline.clone(), settings),
            path,
            progress: (0, total),
            eta: f64::NAN,
            encoder: String::new(),
        });
        self.last_export = None;
    }

    fn poll_export(&mut self) {
        let Some(run) = self.export.as_mut() else { return };
        let mut finished = None;
        while let Ok(ev) = run.job.events().try_recv() {
            match ev {
                ExportEvent::Started { encoder } => run.encoder = encoder,
                ExportEvent::Progress { frame, total, eta_secs } => {
                    run.progress = (frame, total);
                    run.eta = eta_secs;
                }
                ExportEvent::Done { path, secs } => {
                    finished = Some(Ok((path, secs)));
                }
                ExportEvent::Cancelled => finished = Some(Err("Exportación cancelada.".to_owned())),
                ExportEvent::Error(e) => finished = Some(Err(format!("Error al exportar: {e}"))),
            }
        }
        match finished {
            Some(Ok((path, secs))) => {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                self.status = format!("✅ Exportado en {secs:.1} s ({}): {}", human_size(size), path.display());
                self.last_export = Some(path);
                self.export = None;
            }
            Some(Err(msg)) => {
                self.status = msg;
                self.export = None;
            }
            None => {}
        }
    }

    fn export_windows(&mut self, ctx: &egui::Context) {
        self.poll_export();

        if let Some(run) = &self.export {
            let (done, total) = run.progress;
            let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
            let mut cancel = false;
            egui::Window::new("Exportando…").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(RichText::new(run.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).strong());
                ui.add(egui::ProgressBar::new(frac).show_percentage());
                let eta = if run.eta.is_finite() { format!(" · quedan ~{:.0} s", run.eta.ceil()) } else { String::new() };
                ui.label(RichText::new(format!("Cuadro {done} de {total}{eta}")).small());
                if !run.encoder.is_empty() {
                    ui.label(RichText::new(format!("Codificador: {}", run.encoder)).small().color(Color32::GRAY));
                }
                ui.add_space(4.0);
                if ui.button("Cancelar").clicked() {
                    cancel = true;
                }
            });
            if cancel {
                run.job.cancel();
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }

        let Some(opts) = self.export_dialog.as_mut() else { return };
        let tl = &self.timeline;
        let mut open = true;
        let mut go = false;
        egui::Window::new("Exportar video").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.set_min_width(380.0);
            let secs = tl.frame_to_secs(tl.duration());
            ui.label(format!(
                "{}×{} · {:.2} fps · {}",
                tl.width,
                tl.height,
                tl.fps,
                format_timecode(secs, tl.fps)
            ));
            if tl.width > tl.height {
                ui.label(RichText::new("Formato horizontal. Para Reels/TikTok cambia el formato a 9:16.").small().color(Color32::from_rgb(230, 180, 80)));
            }
            ui.add_space(6.0);
            egui::Grid::new("export_opts").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Códec");
                ui.vertical(|ui| {
                    ui.radio_value(&mut opts.codec, VideoCodec::H264, "H.264 · máxima compatibilidad (redes sociales)");
                    ui.radio_value(&mut opts.codec, VideoCodec::Hevc, "HEVC (H.265) · archivos más pequeños");
                });
                ui.end_row();
                ui.label("Calidad");
                ui.horizontal(|ui| {
                    for q in Quality::ALL {
                        ui.radio_value(&mut opts.quality, q, q.label());
                    }
                });
                ui.end_row();
                ui.label("Hardware");
                ui.checkbox(&mut opts.hardware, "Usar el codificador del Mac (más rápido)");
                ui.end_row();
            });
            ui.add_space(6.0);
            let mbps = snipsy_export::video_bitrate(tl, opts.codec, opts.quality) as f64 / 1e6;
            let size = snipsy_export::estimated_size(tl, opts.codec, opts.quality);
            ui.label(RichText::new(format!("≈ {mbps:.1} Mbps · tamaño estimado {}", human_size(size))).small().color(Color32::GRAY));
            ui.add_space(8.0);
            if ui.button(RichText::new("Exportar…").strong()).clicked() {
                go = true;
            }
        });
        if !open {
            self.export_dialog = None;
        } else if go {
            self.start_export();
        }
    }

    /// Agrega un clip de texto de 3 s en el cabezal, en una pista por encima del video.
    fn add_text(&mut self) {
        let at = self.playhead();
        let len = (self.timeline.fps * 3.0).round() as Frame;
        let before = self.timeline.clone();
        let top = self.timeline.video_tracks.len() - 1;
        let free = !self.timeline.video_tracks[top].clips.iter().any(|c| c.start < at + len && c.end() > at);
        let track = if top > 0 && free { top } else { self.timeline.add_track(snipsy_core::TrackKind::Video) };
        let id = self.timeline.insert_text(at, track, len, TextStyle::default());
        self.history.checkpoint(&before);
        self.view.selection = [id].into_iter().collect();
        self.commit();
    }

    fn format_combo(&mut self, ui: &mut egui::Ui) {
        let (w, h) = (self.timeline.width, self.timeline.height);
        let current = FORMATS.iter().find(|f| f.1 == w && f.2 == h);
        let label = current.map(|f| f.0.split(' ').next().unwrap_or(f.0).to_owned()).unwrap_or_else(|| format!("{w}x{h}"));
        egui::ComboBox::from_id_salt("toolbar_format").selected_text(format!("Formato {label}")).show_ui(ui, |ui| {
            for (name, fw, fh) in FORMATS {
                if ui.selectable_label(w == *fw && h == *fh, *name).clicked() && (w, h) != (*fw, *fh) {
                    self.history.checkpoint(&self.timeline);
                    self.timeline.width = *fw;
                    self.timeline.height = *fh;
                    self.dirty = true;
                }
            }
        });
    }

    fn media_pool(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.label(RichText::new("Media pool").strong());
        ui.label(RichText::new("Arrastra al timeline o pulsa ➕").small().color(Color32::GRAY));
        ui.separator();
        let mut append: Option<AssetId> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for asset in &self.timeline.assets {
                ui.horizontal(|ui| {
                    if ui.small_button("➕").on_hover_text("Agregar al final").clicked() {
                        append = Some(asset.id);
                    }
                    ui.dnd_drag_source(ui.scope_id().with(("asset", asset.id.0)), asset.id, |ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&asset.name).color(Color32::from_gray(230)));
                            let kind = match (asset.has_video, asset.has_audio) {
                                _ if asset.still => "🖼",
                                (true, true) => "🎞 🔊",
                                (true, false) => "🎞",
                                _ => "🔊",
                            };
                            let res = if asset.has_video { format!("{}x{} · ", asset.width, asset.height) } else { String::new() };
                            self.proxies.asset_badge(ui, asset);
                            ui.label(
                                RichText::new(if asset.still {
                                    format!("{kind}  {}x{} · imagen", asset.width, asset.height)
                                } else {
                                    format!("{kind}  {res}{}", format_timecode(asset.duration, asset.fps))
                                })
                                    .small()
                                    .color(Color32::GRAY),
                            );
                        });
                    });
                });
                ui.add_space(4.0);
            }
        });
        if let Some(id) = append {
            self.history.checkpoint(&self.timeline);
            let ids = self.timeline.append_asset(id);
            self.view.selection = ids.into_iter().collect();
            self.commit();
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        let fps = self.player.fps();
        ui.horizontal(|ui| {
            if ui.button("⏮").on_hover_text("Inicio (Home)").clicked() {
                self.player.seek(0.0);
            }
            if ui.button("◀|").on_hover_text("Cuadro anterior (←)").clicked() {
                self.player.step_backward();
            }
            let play_label = if self.player.is_playing() { "⏸" } else { "▶" };
            if ui.button(play_label).on_hover_text("Reproducir/Pausa (Espacio)").clicked() {
                self.player.toggle();
            }
            if ui.button("|▶").on_hover_text("Cuadro siguiente (→)").clicked() {
                self.player.step_forward();
            }
            if ui.button("⏭").on_hover_text("Final (End)").clicked() {
                self.player.seek(self.player.duration());
            }
            ui.add_space(8.0);
            ui.label(RichText::new(format_timecode(self.player.position(), fps)).monospace().size(16.0));
            ui.label(
                RichText::new(format!("/ {}", format_timecode(self.player.duration(), fps)))
                    .monospace()
                    .color(Color32::GRAY),
            );
        });
    }
}

fn human_size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1024.0 {
        format!("{:.2} GB", mb / 1024.0)
    } else {
        format!("{mb:.1} MB")
    }
}
