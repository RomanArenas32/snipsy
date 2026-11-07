//! Visor: compone las capas del cuadro actual en la GPU y permite mover y
//! escalar la capa seleccionada directamente sobre la imagen.

use std::collections::{HashMap, HashSet};

use egui::{pos2, vec2, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind};
use snipsy_core::compose::{self, Transform};
use snipsy_core::{ClipId, Frame, History, Prop, Timeline};
use snipsy_media::VideoFrame;
use snipsy_render::{wgpu, Compositor, RgbaImage, Scene};

/// Tamaño máximo del lienzo de vista previa.
const PREVIEW_MAX: f32 = 1920.0;
const HANDLE_PX: f32 = 10.0;
const SNAP_PX: f32 = 8.0;
const SELECTION: Color32 = Color32::from_rgb(255, 170, 40);

struct Visible {
    clip: ClipId,
    base: (f32, f32),
    transform: Transform,
}

enum Drag {
    Move { clip: ClipId, start: (f32, f32), origin: Pos2 },
    Scale { clip: ClipId, start_scale: f32, start_dist: f32, center: Pos2 },
}

pub struct Viewer {
    render_state: Option<egui_wgpu::RenderState>,
    compositor: Option<Compositor>,
    texture: Option<egui::TextureId>,
    scene: Scene,
    /// Cuadro que se está mostrando y las imágenes decodificadas de sus clips.
    frame: Frame,
    images: HashMap<u64, RgbaImage>,
    drag: Option<Drag>,
    guides: (bool, bool),
}

impl Viewer {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let render_state = cc.wgpu_render_state.clone();
        let compositor = render_state.as_ref().map(|rs| Compositor::new(&rs.device, &rs.queue));
        Self {
            render_state,
            compositor,
            texture: None,
            scene: Scene::new(),
            frame: 0,
            images: HashMap::new(),
            drag: None,
            guides: (false, false),
        }
    }

    pub fn has_gpu(&self) -> bool {
        self.compositor.is_some()
    }

    pub fn set_frame(&mut self, frame: VideoFrame) {
        self.frame = frame.frame;
        self.images = frame
            .layers
            .into_iter()
            .map(|(id, img)| (id, RgbaImage { width: img.width, height: img.height, rgba: img.rgba }))
            .collect();
    }

    /// Lienzo de vista previa (px) y factor respecto al proyecto.
    fn canvas_size(tl: &Timeline) -> ((u32, u32), f32) {
        let (pw, ph) = (tl.width.max(1) as f32, tl.height.max(1) as f32);
        let k = (PREVIEW_MAX / pw).min(PREVIEW_MAX / ph).min(1.0);
        (((pw * k).round() as u32, (ph * k).round() as u32), k)
    }

    /// Dibuja el visor. Devuelve `true` si se modificó alguna transformación.
    /// Si la propiedad tiene keyframes, arrastrar crea o ajusta el keyframe en `playhead`.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tl: &mut Timeline,
        history: &mut History,
        selection: &mut HashSet<ClipId>,
        playhead: Frame,
    ) -> bool {
        let rect = ui.available_rect_before_wrap();
        let (pw, ph) = (tl.width.max(1) as f32, tl.height.max(1) as f32);
        let fit = (rect.width() / pw).min(rect.height() / ph) * 0.96;
        let screen = Rect::from_center_size(rect.center(), vec2(pw, ph) * fit);
        let painter = ui.painter_at(rect);

        if self.compositor.is_none() {
            painter.rect_filled(screen, 0.0, Color32::BLACK);
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "GPU no disponible", egui::FontId::proportional(18.0), Color32::GRAY);
            return false;
        }

        // --- componer en la GPU
        let (canvas, k) = Self::canvas_size(tl);
        let scene_layers = self.scene.layers(tl, self.frame, &self.images, k);
        let gpu_layers = Scene::gpu_layers(&scene_layers, tl, k);
        let comp = self.compositor.as_mut().unwrap();
        let recreated = comp.render(canvas, &gpu_layers);
        drop(gpu_layers);
        let layers: Vec<Visible> =
            scene_layers.iter().map(|l| Visible { clip: l.clip, base: l.base, transform: l.transform }).collect();
        if let (Some(rs), Some(view)) = (&self.render_state, comp.canvas_view()) {
            let mut renderer = rs.renderer.write();
            match self.texture {
                Some(id) if recreated => {
                    renderer.update_egui_texture_from_wgpu_texture(&rs.device, view, wgpu::FilterMode::Linear, id)
                }
                Some(_) => {}
                None => self.texture = Some(renderer.register_native_texture(&rs.device, view, wgpu::FilterMode::Linear)),
            }
        }
        if let Some(id) = self.texture {
            painter.image(id, screen, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        painter.rect_stroke(screen, 0.0, Stroke::new(1.0, Color32::from_gray(50)), StrokeKind::Outside);

        if tl.duration() == 0 {
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                "Importa o arrastra videos aquí",
                egui::FontId::proportional(20.0),
                Color32::from_gray(90),
            );
        }

        // --- interacción
        let to_screen = |p: [f32; 2]| pos2(screen.left() + p[0] * fit, screen.top() + p[1] * fit);
        let to_project = |p: Pos2| ((p.x - screen.left()) / fit, (p.y - screen.top()) / fit);
        let pick = |p: Pos2| {
            layers
                .iter()
                .rev()
                .find(|v| compose::contains(&v.transform, v.base, (pw, ph), to_project(p)))
                .map(|v| v.clip)
        };
        let selected = layers.iter().rev().find(|v| selection.contains(&v.clip));
        let handles: Vec<Pos2> = selected
            .map(|v| compose::corners(&v.transform, v.base, (pw, ph)).iter().map(|c| to_screen(*c)).collect())
            .unwrap_or_default();

        let response = ui.interact(rect, ui.scope_id().with("viewer"), Sense::click_and_drag());
        let mut changed = false;

        if self.drag.is_none() {
            if let Some(p) = response.hover_pos() {
                if handles.iter().any(|h| h.distance(p) <= HANDLE_PX) {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe);
                } else if selected.is_some_and(|v| compose::contains(&v.transform, v.base, (pw, ph), to_project(p))) {
                    ui.ctx().set_cursor_icon(CursorIcon::Move);
                }
            }
        }

        if response.drag_started() {
            if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                let on_handle = handles.iter().any(|h| h.distance(p) <= HANDLE_PX);
                let target = if on_handle { selected.map(|v| v.clip) } else { pick(p) };
                if let Some(clip) = target {
                    let v = layers.iter().find(|v| v.clip == clip).unwrap();
                    history.checkpoint(tl);
                    if !selection.contains(&clip) {
                        selection.clear();
                        selection.insert(clip);
                    }
                    let center = to_screen([pw / 2.0 + v.transform.x, ph / 2.0 + v.transform.y]);
                    self.drag = Some(if on_handle {
                        Drag::Scale { clip, start_scale: v.transform.scale, start_dist: center.distance(p).max(1.0), center }
                    } else {
                        Drag::Move { clip, start: (v.transform.x, v.transform.y), origin: p }
                    });
                }
            }
        }

        if let (true, Some(p)) = (response.dragged(), response.interact_pointer_pos()) {
            self.guides = (false, false);
            match &self.drag {
                Some(Drag::Move { clip, start, origin }) => {
                    let mut x = start.0 + (p.x - origin.x) / fit;
                    let mut y = start.1 + (p.y - origin.y) / fit;
                    // Imán al centro del lienzo.
                    if x.abs() * fit < SNAP_PX {
                        x = 0.0;
                        self.guides.0 = true;
                    }
                    if y.abs() * fit < SNAP_PX {
                        y = 0.0;
                        self.guides.1 = true;
                    }
                    if let Some(c) = tl.clip_mut(*clip) {
                        c.set_prop(Prop::X, playhead, x.round());
                        c.set_prop(Prop::Y, playhead, y.round());
                        changed = true;
                    }
                }
                Some(Drag::Scale { clip, start_scale, start_dist, center }) => {
                    let scale = (start_scale * center.distance(p) / start_dist).clamp(0.01, 20.0);
                    if let Some(c) = tl.clip_mut(*clip) {
                        c.set_prop(Prop::Scale, playhead, (scale * 1000.0).round() / 1000.0);
                        changed = true;
                    }
                }
                None => {}
            }
        }
        if response.drag_stopped() {
            self.drag = None;
            self.guides = (false, false);
        }
        if response.clicked() {
            if let Some(p) = response.interact_pointer_pos() {
                selection.clear();
                if let Some(clip) = pick(p) {
                    selection.insert(clip);
                }
            }
        }

        // --- contorno de la selección y guías
        if handles.len() == 4 {
            let mut pts = handles.clone();
            pts.push(handles[0]);
            painter.add(egui::Shape::line(pts, Stroke::new(1.5, SELECTION)));
            for h in &handles {
                painter.rect_filled(Rect::from_center_size(*h, vec2(8.0, 8.0)), 1.0, SELECTION);
            }
        }
        let guide = Stroke::new(1.0, Color32::from_rgb(80, 200, 255));
        if self.guides.0 {
            painter.line_segment([pos2(screen.center().x, screen.top()), pos2(screen.center().x, screen.bottom())], guide);
        }
        if self.guides.1 {
            painter.line_segment([pos2(screen.left(), screen.center().y), pos2(screen.right(), screen.center().y)], guide);
        }
        changed
    }
}
