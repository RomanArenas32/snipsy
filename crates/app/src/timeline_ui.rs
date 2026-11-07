//! Widget de la línea de tiempo: regla, pistas, clips e interacción.

use std::collections::HashSet;

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind,
    Ui,
};
use snipsy_core::timecode::format_timecode;
use snipsy_core::{AssetId, Clip, ClipId, Edge, Frame, History, Timeline, TrackKind, TrackRef};

const HEADER_W: f32 = 84.0;
const RULER_H: f32 = 26.0;
const VIDEO_H: f32 = 54.0;
const AUDIO_H: f32 = 44.0;
const DIVIDER_H: f32 = 6.0;
const EDGE_PX: f32 = 6.0;
const SNAP_PX: f32 = 8.0;

const BG: Color32 = Color32::from_gray(22);
const ROW_A: Color32 = Color32::from_gray(30);
const ROW_B: Color32 = Color32::from_gray(34);
const HEADER_BG: Color32 = Color32::from_gray(40);
const VIDEO_CLIP: Color32 = Color32::from_rgb(52, 92, 150);
const AUDIO_CLIP: Color32 = Color32::from_rgb(48, 120, 82);
const TEXT_CLIP: Color32 = Color32::from_rgb(130, 72, 160);
const PLAYHEAD: Color32 = Color32::from_rgb(235, 64, 52);
const SNAP_LINE: Color32 = Color32::from_rgb(250, 200, 40);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Blade,
}

#[derive(Default)]
pub struct TimelineOutput {
    pub seek: Option<Frame>,
    pub changed: bool,
}

enum Drag {
    Scrub,
    Move {
        anchor_track: TrackRef,
        ids: Vec<ClipId>,
        grab_frame: f64,
        delta: Frame,
        track_delta: i64,
    },
    Trim {
        clip: ClipId,
        edge: Edge,
        before: Box<Timeline>,
    },
}

#[derive(Clone, Copy)]
struct Row {
    track: TrackRef,
    y0: f32,
    y1: f32,
}

enum Hit {
    Ruler,
    Header(TrackRef, bool),
    Clip { id: ClipId, track: TrackRef, edge: Option<Edge> },
    Empty,
}

pub struct TimelineView {
    pub px_per_frame: f64,
    /// Cuadro en el borde izquierdo.
    pub scroll: f64,
    vscroll: f32,
    pub tool: Tool,
    pub snapping: bool,
    pub selection: HashSet<ClipId>,
    drag: Option<Drag>,
    snap_at: Option<Frame>,
    visible_frames: f64,
}

impl Default for TimelineView {
    fn default() -> Self {
        Self {
            px_per_frame: 4.0,
            scroll: 0.0,
            vscroll: 0.0,
            tool: Tool::Select,
            snapping: true,
            selection: HashSet::new(),
            drag: None,
            snap_at: None,
            visible_frames: 300.0,
        }
    }
}

impl TimelineView {
    pub fn zoom(&mut self, factor: f64, anchor_frame: Option<f64>) {
        let anchor = anchor_frame.unwrap_or(self.scroll + self.visible_frames / 2.0);
        let offset_px = (anchor - self.scroll) * self.px_per_frame;
        self.px_per_frame = (self.px_per_frame * factor).clamp(0.02, 60.0);
        self.scroll = anchor - offset_px / self.px_per_frame;
        self.clamp_scroll();
    }

    pub fn zoom_to_fit(&mut self, tl: &Timeline) {
        let frames = (tl.duration() as f64).max(tl.fps * 10.0);
        let width = self.visible_frames * self.px_per_frame;
        self.px_per_frame = ((width - 40.0) / frames).clamp(0.02, 60.0);
        self.scroll = 0.0;
    }

    fn clamp_scroll(&mut self) {
        let margin = 12.0 / self.px_per_frame;
        self.scroll = self.scroll.max(-margin);
    }

    /// Mantiene visible el cabezal durante la reproducción.
    pub fn follow(&mut self, playhead: Frame) {
        let p = playhead as f64;
        if p < self.scroll || p > self.scroll + self.visible_frames * 0.95 {
            self.scroll = p - self.visible_frames * 0.1;
            self.clamp_scroll();
        }
    }

    pub fn show(&mut self, ui: &mut Ui, tl: &mut Timeline, history: &mut History, playhead: Frame) -> TimelineOutput {
        let mut out = TimelineOutput::default();
        self.selection.retain(|id| tl.find(*id).is_some());

        self.toolbar(ui, tl, history, &mut out);
        ui.add_space(2.0);

        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let content = Rect::from_min_max(pos2(rect.left() + HEADER_W, rect.top()), rect.max);
        self.visible_frames = (content.width() as f64 / self.px_per_frame).max(1.0);

        let rows = self.layout_rows(tl, rect);
        let total_h: f32 = rows.last().map(|r| r.y1 + self.vscroll - rect.top()).unwrap_or(0.0);
        let max_v = (total_h - rect.height()).max(0.0);

        // --- zoom y desplazamiento
        if response.hovered() {
            let (zoom, scroll, hover) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta(), i.pointer.hover_pos()));
            if zoom != 1.0 {
                let anchor = hover.map(|p| self.x_to_frame(content, p.x));
                self.zoom(zoom as f64, anchor);
            } else {
                self.scroll -= scroll.x as f64 / self.px_per_frame;
                if max_v > 0.0 {
                    self.vscroll = (self.vscroll - scroll.y).clamp(0.0, max_v);
                } else {
                    self.scroll -= scroll.y as f64 / self.px_per_frame;
                }
                self.clamp_scroll();
            }
        }
        self.vscroll = self.vscroll.min(max_v);
        let rows = self.layout_rows(tl, rect);

        // --- interacción
        self.interact(ui, &response, tl, history, &rows, rect, content, playhead, &mut out);

        // --- soltar un archivo desde el media pool
        let hover_asset = response.dnd_hover_payload::<AssetId>();
        let drop_target = ui.input(|i| i.pointer.hover_pos()).and_then(|p| {
            let row = rows.iter().find(|r| p.y >= r.y0 && p.y < r.y1).copied();
            Some((self.snap(tl, self.x_to_frame(content, p.x).round() as Frame, &HashSet::new(), playhead), row))
        });
        if let Some(asset) = response.dnd_release_payload::<AssetId>() {
            if let Some((frame, row)) = drop_target {
                let (vt, at) = match row.map(|r| r.track) {
                    Some(TrackRef { kind: TrackKind::Video, index }) => (index, index.min(tl.audio_tracks.len().saturating_sub(1))),
                    Some(TrackRef { kind: TrackKind::Audio, index }) => (0, index),
                    None => (0, 0),
                };
                history.checkpoint(tl);
                let ids = tl.insert_asset(*asset, frame.max(0), vt, at);
                self.selection = ids.into_iter().collect();
                out.changed = true;
            }
        }

        // --- dibujo
        painter.rect_filled(rect, 0.0, BG);
        self.draw_rows(&painter, tl, &rows, rect, content);
        self.draw_clips(&painter, tl, &rows, content);
        if let (Some(asset), Some((frame, Some(row)))) = (hover_asset, drop_target) {
            let len = tl.asset_frames(*asset);
            let r = Rect::from_min_max(
                pos2(self.frame_to_x(content, frame as f64), row.y0 + 2.0),
                pos2(self.frame_to_x(content, (frame + len) as f64), row.y1 - 2.0),
            );
            painter.rect_stroke(r, 3.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Inside);
        }
        self.draw_ruler(&painter, tl, rect, content);
        self.draw_headers(&painter, tl, &rows, rect);

        let px = self.frame_to_x(content, playhead as f64);
        if px >= content.left() {
            painter.line_segment([pos2(px, rect.top()), pos2(px, rect.bottom())], Stroke::new(1.5, PLAYHEAD));
            let top = rect.top() + RULER_H;
            painter.add(egui::Shape::convex_polygon(
                vec![pos2(px - 6.0, top - 12.0), pos2(px + 6.0, top - 12.0), pos2(px, top - 4.0)],
                PLAYHEAD,
                Stroke::NONE,
            ));
        }
        if let Some(f) = self.snap_at {
            let x = self.frame_to_x(content, f as f64);
            painter.line_segment([pos2(x, rect.top() + RULER_H), pos2(x, rect.bottom())], Stroke::new(1.0, SNAP_LINE));
        }
        out
    }

    fn toolbar(&mut self, ui: &mut Ui, tl: &mut Timeline, history: &mut History, out: &mut TimelineOutput) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tool, Tool::Select, "⬉ Selección").on_hover_text("A");
            ui.selectable_value(&mut self.tool, Tool::Blade, "✂ Cuchilla").on_hover_text("B");
            ui.separator();
            ui.toggle_value(&mut self.snapping, "🧲 Imán").on_hover_text("N");
            ui.separator();
            if ui.button("➖").on_hover_text("Alejar (-)").clicked() {
                self.zoom(1.0 / 1.5, None);
            }
            if ui.button("➕").on_hover_text("Acercar (=)").clicked() {
                self.zoom(1.5, None);
            }
            if ui.button("Ajustar").on_hover_text("Ver todo (Shift+Z)").clicked() {
                self.zoom_to_fit(tl);
            }
            ui.separator();
            if ui.button("+ Pista video").clicked() {
                history.checkpoint(tl);
                tl.add_track(TrackKind::Video);
                out.changed = true;
            }
            if ui.button("+ Pista audio").clicked() {
                history.checkpoint(tl);
                tl.add_track(TrackKind::Audio);
                out.changed = true;
            }
        });
    }

    // ------------------------------------------------------------ geometría

    fn frame_to_x(&self, content: Rect, frame: f64) -> f32 {
        content.left() + ((frame - self.scroll) * self.px_per_frame) as f32
    }

    fn x_to_frame(&self, content: Rect, x: f32) -> f64 {
        self.scroll + (x - content.left()) as f64 / self.px_per_frame
    }

    fn layout_rows(&self, tl: &Timeline, rect: Rect) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut y = rect.top() + RULER_H - self.vscroll;
        for i in (0..tl.video_tracks.len()).rev() {
            rows.push(Row { track: TrackRef::video(i), y0: y, y1: y + VIDEO_H });
            y += VIDEO_H;
        }
        y += DIVIDER_H;
        for i in 0..tl.audio_tracks.len() {
            rows.push(Row { track: TrackRef::audio(i), y0: y, y1: y + AUDIO_H });
            y += AUDIO_H;
        }
        rows
    }

    fn clip_rect(&self, content: Rect, row: &Row, start: Frame, end: Frame) -> Rect {
        Rect::from_min_max(
            pos2(self.frame_to_x(content, start as f64), row.y0 + 2.0),
            pos2(self.frame_to_x(content, end as f64), row.y1 - 2.0),
        )
    }

    fn hit(&self, tl: &Timeline, rows: &[Row], rect: Rect, content: Rect, p: Pos2) -> Hit {
        if p.y < rect.top() + RULER_H {
            return Hit::Ruler;
        }
        let Some(row) = rows.iter().find(|r| p.y >= r.y0 && p.y < r.y1) else { return Hit::Empty };
        if p.x < content.left() {
            let icon = mute_rect(rect, row);
            return Hit::Header(row.track, icon.contains(p));
        }
        for c in &tl.track(row.track).clips {
            let r = self.clip_rect(content, row, c.start, c.end());
            if r.x_range().contains(p.x) {
                let edge_px = EDGE_PX.min(r.width() / 3.0);
                let edge = if p.x - r.left() <= edge_px {
                    Some(Edge::Start)
                } else if r.right() - p.x <= edge_px {
                    Some(Edge::End)
                } else {
                    None
                };
                return Hit::Clip { id: c.id, track: row.track, edge };
            }
        }
        Hit::Empty
    }

    /// Ajusta `frame` al punto de imán más cercano (bordes de clips y cabezal).
    fn snap(&mut self, tl: &Timeline, frame: Frame, exclude: &HashSet<ClipId>, playhead: Frame) -> Frame {
        self.snap_offset(tl, &[frame], exclude, playhead).map(|d| frame + d).unwrap_or(frame)
    }

    /// Menor desplazamiento que lleva alguno de `candidates` a un punto de imán.
    fn snap_offset(&mut self, tl: &Timeline, candidates: &[Frame], exclude: &HashSet<ClipId>, playhead: Frame) -> Option<Frame> {
        self.snap_at = None;
        if !self.snapping {
            return None;
        }
        let threshold = (SNAP_PX as f64 / self.px_per_frame).ceil() as Frame;
        let mut points = tl.snap_points(exclude);
        points.push(playhead);
        let mut best: Option<(Frame, Frame)> = None;
        for &c in candidates {
            for &p in &points {
                let d = p - c;
                if d.abs() <= threshold && best.is_none_or(|(bd, _)| d.abs() < bd.abs()) {
                    best = Some((d, p));
                }
            }
        }
        best.map(|(d, p)| {
            self.snap_at = Some(p);
            d
        })
    }

    // ------------------------------------------------------------ interacción

    #[allow(clippy::too_many_arguments)]
    fn interact(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        tl: &mut Timeline,
        history: &mut History,
        rows: &[Row],
        rect: Rect,
        content: Rect,
        playhead: Frame,
        out: &mut TimelineOutput,
    ) {
        let (hover, press, shift) = ui.input(|i| (i.pointer.hover_pos(), i.pointer.press_origin(), i.modifiers.shift));

        // Cursor según lo que hay debajo.
        if self.drag.is_none() {
            if let Some(p) = hover.filter(|p| content.contains(*p)) {
                let icon = match self.hit(tl, rows, rect, content, p) {
                    Hit::Clip { .. } if self.tool == Tool::Blade => CursorIcon::Crosshair,
                    Hit::Clip { edge: Some(_), .. } => CursorIcon::ResizeHorizontal,
                    Hit::Clip { .. } => CursorIcon::Grab,
                    _ => CursorIcon::Default,
                };
                ui.ctx().set_cursor_icon(icon);
            }
        }

        // Inicio de arrastre: se decide qué se arrastra según dónde se presionó.
        if response.drag_started() {
            if let Some(p) = press {
                self.drag = match self.hit(tl, rows, rect, content, p) {
                    Hit::Ruler => Some(Drag::Scrub),
                    Hit::Clip { id, edge: Some(edge), .. } if self.tool == Tool::Select => {
                        Some(Drag::Trim { clip: id, edge, before: Box::new(tl.clone()) })
                    }
                    Hit::Clip { id, track, .. } if self.tool == Tool::Select => {
                        if !self.selection.contains(&id) {
                            if !shift {
                                self.selection.clear();
                            }
                            self.selection.insert(id);
                        }
                        let ids = tl.with_links(self.selection.iter().copied());
                        Some(Drag::Move {
                            anchor_track: track,
                            ids,
                            grab_frame: self.x_to_frame(content, p.x),
                            delta: 0,
                            track_delta: 0,
                        })
                    }
                    _ => None,
                };
            }
        }

        // Durante el arrastre.
        if let (Some(p), true) = (response.interact_pointer_pos(), response.dragged()) {
            let pointer_frame = self.x_to_frame(content, p.x);
            let pointer_row = rows.iter().find(|r| p.y >= r.y0 && p.y < r.y1).map(|r| r.track);
            match &self.drag {
                Some(Drag::Scrub) => {
                    out.seek = Some(pointer_frame.round().max(0.0) as Frame);
                }
                Some(Drag::Move { anchor_track, ids, grab_frame, .. }) => {
                    let anchor_track = *anchor_track;
                    let raw = (pointer_frame - *grab_frame).round() as Frame;
                    let exclude: HashSet<ClipId> = ids.iter().copied().collect();
                    let edges: Vec<Frame> = ids
                        .iter()
                        .filter_map(|id| tl.find(*id))
                        .flat_map(|(_, c)| [c.start + raw, c.end() + raw])
                        .collect();
                    let snapped = self.snap_offset(tl, &edges, &exclude, playhead).map(|d| raw + d).unwrap_or(raw);
                    if let Some(Drag::Move { delta, track_delta, .. }) = &mut self.drag {
                        *delta = snapped;
                        if let Some(r) = pointer_row.filter(|r| r.kind == anchor_track.kind) {
                            *track_delta = r.index as i64 - anchor_track.index as i64;
                        }
                    }
                    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                }
                Some(Drag::Trim { clip, edge, before }) => {
                    let (clip, edge) = (*clip, *edge);
                    let before = before.clone();
                    let exclude: HashSet<ClipId> = before.with_links([clip]).into_iter().collect();
                    let target = self.snap(&before, pointer_frame.round() as Frame, &exclude, playhead);
                    *tl = (*before).clone();
                    tl.trim(clip, edge, target);
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                }
                None => {}
            }
        }

        // Fin del arrastre: aplicar la edición.
        if response.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Move { ids, delta, track_delta, anchor_track, .. }) => {
                    let td = match anchor_track.kind {
                        TrackKind::Video => (track_delta, 0),
                        TrackKind::Audio => (0, track_delta),
                    };
                    if delta != 0 || track_delta != 0 {
                        let before = tl.clone();
                        if tl.move_clips(&ids, delta, td) {
                            history.checkpoint(&before);
                            out.changed = true;
                        }
                    }
                }
                Some(Drag::Trim { clip, before, .. }) => {
                    let now = tl.find(clip).map(|(_, c)| (c.start, c.len));
                    let was = before.find(clip).map(|(_, c)| (c.start, c.len));
                    if now != was {
                        history.checkpoint(&before);
                        out.changed = true;
                    }
                }
                _ => {}
            }
            self.snap_at = None;
        }

        // Clic simple.
        if response.clicked() {
            if let Some(p) = response.interact_pointer_pos() {
                match self.hit(tl, rows, rect, content, p) {
                    Hit::Ruler => out.seek = Some(self.x_to_frame(content, p.x).round().max(0.0) as Frame),
                    Hit::Header(track, true) => {
                        history.checkpoint(tl);
                        let t = tl.track_mut(track);
                        t.muted = !t.muted;
                        out.changed = true;
                    }
                    Hit::Clip { id, .. } if self.tool == Tool::Blade => {
                        let raw = self.x_to_frame(content, p.x).round() as Frame;
                        let at = self.snap(tl, raw, &HashSet::new(), playhead);
                        self.snap_at = None;
                        let before = tl.clone();
                        if tl.split(at, Some(&[id])) {
                            history.checkpoint(&before);
                            out.changed = true;
                        }
                    }
                    Hit::Clip { id, .. } => {
                        if shift {
                            if !self.selection.remove(&id) {
                                self.selection.insert(id);
                            }
                        } else {
                            self.selection = [id].into_iter().collect();
                        }
                    }
                    Hit::Empty => self.selection.clear(),
                    Hit::Header(..) => {}
                }
            }
        }
    }

    // ------------------------------------------------------------ dibujo

    fn draw_rows(&self, painter: &Painter, tl: &Timeline, rows: &[Row], rect: Rect, content: Rect) {
        for (i, row) in rows.iter().enumerate() {
            let r = Rect::from_min_max(pos2(content.left(), row.y0), pos2(rect.right(), row.y1));
            painter.rect_filled(r, 0.0, if i % 2 == 0 { ROW_A } else { ROW_B });
        }
        // Fin del timeline.
        let end = self.frame_to_x(content, tl.duration() as f64);
        if end > content.left() && end < rect.right() {
            painter.line_segment(
                [pos2(end, rect.top() + RULER_H), pos2(end, rect.bottom())],
                Stroke::new(1.0, Color32::from_gray(70)),
            );
        }
    }

    fn draw_clips(&self, painter: &Painter, tl: &Timeline, rows: &[Row], content: Rect) {
        let selected: HashSet<ClipId> = tl.with_links(self.selection.iter().copied()).into_iter().collect();
        let (moving, delta, track_delta, anchor_kind) = match &self.drag {
            Some(Drag::Move { ids, delta, track_delta, anchor_track, .. }) => {
                (ids.iter().copied().collect::<HashSet<_>>(), *delta, *track_delta, Some(anchor_track.kind))
            }
            _ => (HashSet::new(), 0, 0, None),
        };
        let clip_painter = painter.with_clip_rect(content.intersect(painter.clip_rect()));

        for row in rows {
            let track = tl.track(row.track);
            for (ci, c) in track.clips.iter().enumerate() {
                let r = self.clip_rect(content, row, c.start, c.end());
                if r.right() < content.left() || r.left() > content.right() {
                    continue;
                }
                let base = if c.is_text() {
                    TEXT_CLIP
                } else if row.track.kind == TrackKind::Video {
                    VIDEO_CLIP
                } else {
                    AUDIO_CLIP
                };
                let is_sel = selected.contains(&c.id);
                let mut fill = if is_sel { base.gamma_multiply(1.35) } else { base };
                if track.muted {
                    fill = fill.gamma_multiply(0.4);
                }
                if moving.contains(&c.id) {
                    fill = fill.gamma_multiply(0.45);
                }
                clip_painter.rect_filled(r, CornerRadius::same(3), fill);
                let stroke = if is_sel { Stroke::new(1.5, Color32::WHITE) } else { Stroke::new(1.0, Color32::from_black_alpha(120)) };
                clip_painter.rect_stroke(r, CornerRadius::same(3), stroke, StrokeKind::Inside);
                self.draw_effects(&clip_painter, tl, row, ci, c, r, content);
                if r.width() > 24.0 {
                    let name = match &c.text {
                        Some(t) => t.text.lines().next().unwrap_or(""),
                        None => tl.asset(c.asset).map(|a| a.name.as_str()).unwrap_or("?"),
                    };
                    let text_painter = clip_painter.with_clip_rect(r.shrink(2.0).intersect(clip_painter.clip_rect()));
                    text_painter.text(
                        pos2(r.left().max(content.left()) + 6.0, r.top() + 4.0),
                        Align2::LEFT_TOP,
                        name,
                        FontId::proportional(12.0),
                        Color32::from_gray(235),
                    );
                }
            }
        }

        // Fantasmas de lo que se está moviendo.
        if let Some(kind) = anchor_kind {
            for id in &moving {
                let Some((track, c)) = tl.find(*id) else { continue };
                let index = if track.kind == kind { track.index as i64 + track_delta } else { track.index as i64 };
                if index < 0 || index as usize >= tl.tracks(track.kind).len() {
                    continue;
                }
                let dest = TrackRef { kind: track.kind, index: index as usize };
                let Some(row) = rows.iter().find(|r| r.track == dest) else { continue };
                let r = self.clip_rect(content, row, c.start + delta, c.end() + delta);
                clip_painter.rect_filled(r, CornerRadius::same(3), Color32::from_white_alpha(40));
                clip_painter.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.5, Color32::WHITE), StrokeKind::Inside);
            }
        }
    }

    /// Fundidos (triángulos), transición con el clip anterior y keyframes (rombos).
    #[allow(clippy::too_many_arguments)]
    fn draw_effects(&self, painter: &Painter, tl: &Timeline, row: &Row, ci: usize, c: &Clip, r: Rect, content: Rect) {
        let shade = Color32::from_black_alpha(110);
        let edge = Stroke::new(1.0, Color32::from_white_alpha(170));
        if c.fade_in > 0 {
            let x = self.frame_to_x(content, (c.start + c.fade_in.min(c.len)) as f64);
            painter.add(egui::Shape::convex_polygon(vec![r.left_top(), pos2(x, r.top()), r.left_bottom()], shade, Stroke::NONE));
            painter.line_segment([r.left_bottom(), pos2(x, r.top())], edge);
        }
        if c.fade_out > 0 {
            let x = self.frame_to_x(content, (c.end() - c.fade_out.min(c.len)) as f64);
            painter.add(egui::Shape::convex_polygon(vec![pos2(x, r.top()), r.right_top(), r.right_bottom()], shade, Stroke::NONE));
            painter.line_segment([pos2(x, r.top()), r.right_bottom()], edge);
        }
        if let Some((before, after)) = tl.transition_split(row.track, ci) {
            let t = Rect::from_min_max(
                pos2(self.frame_to_x(content, (c.start - before) as f64), r.top() + 2.0),
                pos2(self.frame_to_x(content, (c.start + after) as f64), r.bottom() - 2.0),
            );
            painter.rect_filled(t, 2.0, Color32::from_rgba_unmultiplied(255, 255, 255, 50));
            painter.rect_stroke(t, 2.0, Stroke::new(1.0, Color32::from_white_alpha(200)), StrokeKind::Inside);
            painter.line_segment([t.left_top(), t.right_bottom()], edge);
            painter.line_segment([t.left_bottom(), t.right_top()], edge);
        }
        if c.speed != 1.0 && r.width() > 40.0 {
            let label = format!("{}×", (c.speed * 100.0).round() / 100.0);
            painter.text(pos2(r.right() - 5.0, r.top() + 4.0), Align2::RIGHT_TOP, label, FontId::proportional(11.0), Color32::from_rgb(255, 220, 120));
        }
        let y = r.bottom() - 6.0;
        for at in c.anim.key_times() {
            let f = c.timeline_frame(at);
            if f < c.start || f >= c.end() {
                continue;
            }
            let x = self.frame_to_x(content, f as f64 + 0.5);
            let d = 4.0;
            painter.add(egui::Shape::convex_polygon(
                vec![pos2(x, y - d), pos2(x + d, y), pos2(x, y + d), pos2(x - d, y)],
                Color32::from_rgb(255, 200, 60),
                Stroke::new(1.0, Color32::from_black_alpha(160)),
            ));
        }
    }

    fn draw_ruler(&self, painter: &Painter, tl: &Timeline, rect: Rect, content: Rect) {
        let ruler = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + RULER_H));
        painter.rect_filled(ruler, 0.0, HEADER_BG);
        let fps = tl.fps.max(1.0);
        let nominal = fps.round().max(1.0);
        let steps_frames = [1.0, 2.0, 5.0, 10.0];
        let steps_secs = [1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0];
        let major = steps_frames
            .iter()
            .copied()
            .chain(steps_secs.iter().map(|s| s * nominal))
            .find(|s| s * self.px_per_frame >= 90.0)
            .unwrap_or(3600.0 * nominal);
        let minor = if major >= 5.0 { major / 5.0 } else { 1.0 };
        let first = (self.scroll / minor).floor() * minor;
        let last = self.scroll + self.visible_frames;
        let mut f = first.max(0.0);
        let text_painter = painter.with_clip_rect(Rect::from_min_max(pos2(content.left(), rect.top()), ruler.max));
        while f <= last {
            let x = self.frame_to_x(content, f);
            let is_major = (f / major).fract().abs() < 1e-6;
            let h = if is_major { 10.0 } else { 4.0 };
            text_painter.line_segment(
                [pos2(x, ruler.bottom() - h), pos2(x, ruler.bottom())],
                Stroke::new(1.0, Color32::from_gray(if is_major { 150 } else { 90 })),
            );
            if is_major {
                text_painter.text(
                    pos2(x + 3.0, ruler.top() + 2.0),
                    Align2::LEFT_TOP,
                    format_timecode(f / fps, fps),
                    FontId::monospace(10.0),
                    Color32::from_gray(170),
                );
            }
            f += minor;
        }
    }

    fn draw_headers(&self, painter: &Painter, tl: &Timeline, rows: &[Row], rect: Rect) {
        let header = Rect::from_min_max(pos2(rect.left(), rect.top() + RULER_H), pos2(rect.left() + HEADER_W, rect.bottom()));
        painter.rect_filled(header, 0.0, HEADER_BG);
        let hp = painter.with_clip_rect(header);
        for row in rows {
            let track = tl.track(row.track);
            hp.line_segment(
                [pos2(rect.left(), row.y1), pos2(rect.left() + HEADER_W, row.y1)],
                Stroke::new(1.0, Color32::from_gray(30)),
            );
            hp.text(
                pos2(rect.left() + 10.0, (row.y0 + row.y1) / 2.0),
                Align2::LEFT_CENTER,
                &track.name,
                FontId::proportional(14.0),
                Color32::from_gray(220),
            );
            let icon = mute_rect(rect, row);
            let (label, active) = match row.track.kind {
                TrackKind::Video => (if track.muted { "◌" } else { "👁" }, !track.muted),
                TrackKind::Audio => (if track.muted { "🔇" } else { "🔊" }, !track.muted),
            };
            hp.rect_filled(icon, 3.0, if active { Color32::from_gray(60) } else { Color32::from_rgb(120, 50, 50) });
            hp.text(icon.center(), Align2::CENTER_CENTER, label, FontId::proportional(12.0), Color32::WHITE);
        }
        painter.line_segment(
            [pos2(rect.left() + HEADER_W, rect.top()), pos2(rect.left() + HEADER_W, rect.bottom())],
            Stroke::new(1.0, Color32::from_gray(15)),
        );
    }
}

/// Botón de ocultar/silenciar en la cabecera de la pista.
fn mute_rect(rect: Rect, row: &Row) -> Rect {
    Rect::from_center_size(pos2(rect.left() + HEADER_W - 20.0, (row.y0 + row.y1) / 2.0), vec2(24.0, 20.0))
}
