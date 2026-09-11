//! Brush, pencil, eraser and the shape tools (line/rectangle/ellipse), all of
//! which go through the core stroke engine.

use std::collections::VecDeque;

use qsketch_core::{BrushSettings, IRect, PaintMode, Pt, Rgba8, StabilizerMode, StrokeEngine, StrokeSample};

use super::symmetry::Transform;
use super::{rect_from_drag, CanvasEvent, CanvasInput, ToolKind, ToolSession};
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

/// Per-stroke state beyond the primary engine: symmetry copies and the
/// input stabilizer.
#[derive(Default)]
pub struct StrokeExtra {
    /// One engine per symmetry copy, painting the transformed sample.
    pub mirrors: Vec<(Transform, Box<StrokeEngine>)>,
    pub stabilizer: Option<Stabilizer>,
    /// Last raw pointer sample, for the rope catch-up on release.
    pub last_raw: Option<StrokeSample>,
}

/// Input stabilizer for freehand strokes (see `StabilizerMode`).
pub struct Stabilizer {
    mode: StabilizerMode,
    /// Rope length in document pixels.
    radius: f32,
    /// Averaging window in samples.
    window: usize,
    pos: Option<Pt>,
    recent: VecDeque<Pt>,
    catch_up: bool,
}

impl Stabilizer {
    /// `zoom` converts the strength (tuned in screen pixels) into document space.
    pub fn new(b: &BrushSettings, zoom: f32) -> Option<Self> {
        if b.stabilizer == StabilizerMode::Off || b.stabilizer_strength <= 0.0 {
            return None;
        }
        let t = b.stabilizer_strength;
        Some(Self {
            mode: b.stabilizer,
            radius: (4.0 + t * t * 120.0) / zoom.max(0.01),
            window: 2 + (t * 40.0) as usize,
            pos: None,
            recent: VecDeque::new(),
            catch_up: b.stabilizer_catch_up,
        })
    }

    /// Feed a raw sample; returns the stabilized position to paint at.
    pub fn push(&mut self, raw: Pt) -> Pt {
        match self.mode {
            StabilizerMode::Rope => {
                let out = match self.pos {
                    None => raw,
                    Some(p) => {
                        let (dx, dy) = (raw.x - p.x, raw.y - p.y);
                        let d = (dx * dx + dy * dy).sqrt();
                        if d <= self.radius {
                            p
                        } else {
                            let k = (d - self.radius) / d;
                            Pt::new(p.x + dx * k, p.y + dy * k)
                        }
                    }
                };
                self.pos = Some(out);
                out
            }
            StabilizerMode::Average => {
                self.recent.push_back(raw);
                while self.recent.len() > self.window {
                    self.recent.pop_front();
                }
                let n = self.recent.len() as f32;
                let (sx, sy) = self.recent.iter().fold((0.0, 0.0), |(x, y), p| (x + p.x, y + p.y));
                let out = Pt::new(sx / n, sy / n);
                self.pos = Some(out);
                out
            }
            StabilizerMode::Off => raw,
        }
    }

    /// Where the painted stroke currently ends.
    pub fn current(&self) -> Option<Pt> {
        self.pos
    }

    /// Whether the stroke should be completed to the pointer on release.
    pub fn catches_up(&self) -> bool {
        self.catch_up || self.mode == StabilizerMode::Average
    }
}

fn brush_for(state: &AppState, tool: ToolKind) -> (BrushSettings, PaintMode, Rgba8) {
    match tool {
        ToolKind::Pencil => (state.pencil.clone(), PaintMode::Paint, state.fg),
        ToolKind::Eraser => (state.eraser.clone(), PaintMode::Erase, state.bg),
        _ => (state.brush.clone(), PaintMode::Paint, state.fg),
    }
}

/// Start a stroke engine on the active layer (plus one per symmetry copy);
/// returns None (with a toast) if the layer can't be painted on.
pub(super) fn begin_engine_with(
    state: &mut AppState,
    doc_id: DocId,
    tool: ToolKind,
) -> Option<(Box<StrokeEngine>, usize, StrokeExtra)> {
    let (settings, mode, color) = brush_for(state, tool);
    let (tip, texture) = state.library.resolve(&settings);
    let bg = if mode == PaintMode::Erase { state.fg } else { state.bg };
    if mode == PaintMode::Paint {
        state.note_color_used(color);
    }
    let symmetry = state.symmetry;
    let entry = state.doc_mut(doc_id)?;
    let (w, h) = (entry.doc.width(), entry.doc.height());
    let s = entry.doc.state();
    let li = s.active;
    let layer = &s.layers[li];
    if !layer.props.visible {
        state.toasts.push(Level::Info, "The active layer is hidden.");
        return None;
    }
    if layer.props.locked {
        state.toasts.push(Level::Info, "The active layer is locked.");
        return None;
    }
    let erase_alpha_locked = mode == PaintMode::Erase && layer.props.alpha_locked;
    let make = |seed: u64| {
        StrokeEngine::new(settings.clone(), mode, color, layer, s.selection.clone())
            .with_tip(tip.clone())
            .with_texture(texture.clone())
            .with_background(bg)
            .with_seed(seed)
    };
    let base_seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15);
    let mirrors = symmetry
        .transforms(w, h)
        .into_iter()
        .enumerate()
        .map(|(i, t)| (t, Box::new(make(base_seed.wrapping_add(i as u64 + 1)))))
        .collect();
    let engine = make(base_seed);
    if erase_alpha_locked {
        state.toasts.push(Level::Info, "Transparency is locked on this layer: the eraser paints the background color.");
    }
    Some((Box::new(engine), li, StrokeExtra { mirrors, stabilizer: None, last_raw: None }))
}

fn stroke_label(tool: ToolKind) -> &'static str {
    match tool {
        ToolKind::Pencil => "Pencil",
        ToolKind::Eraser => "Eraser",
        ToolKind::Line => "Line",
        ToolKind::Rect => "Rectangle",
        ToolKind::Ellipse => "Ellipse",
        _ => "Brush",
    }
}

pub(super) fn feed(state: &mut AppState, doc_id: DocId, sample: StrokeSample) {
    let Some(ToolSession::Stroke { engine, layer, extra }) = &mut state.session else { return };
    let Some(entry) = state.docs.iter_mut().find(|d| d.id == doc_id) else { return };
    let li = *layer;
    let raster = &mut entry.doc.state_mut().layers[li].raster;
    let mut dirty = engine.extend(raster, sample);
    for (t, m) in extra.mirrors.iter_mut() {
        let d = m.extend(raster, StrokeSample { pos: t.apply(sample.pos), pressure: sample.pressure });
        dirty = if dirty.is_empty() {
            d
        } else if d.is_empty() {
            dirty
        } else {
            dirty.union(&d)
        };
    }
    if !dirty.is_empty() {
        entry.doc.mark_dirty_rect(dirty);
    }
}

/// Feed a raw pointer sample through the session's stabilizer (if any).
fn feed_raw(state: &mut AppState, doc_id: DocId, sample: StrokeSample) {
    let Some(ToolSession::Stroke { extra, .. }) = &mut state.session else { return };
    extra.last_raw = Some(sample);
    let pos = match extra.stabilizer.as_mut() {
        Some(st) => st.push(sample.pos),
        None => sample.pos,
    };
    feed(state, doc_id, StrokeSample { pos, pressure: sample.pressure });
}

fn finish(state: &mut AppState, doc_id: DocId, tool: ToolKind) {
    finish_with(state, doc_id, stroke_label(tool));
}

/// End the stroke session, committing `label` if anything was painted.
pub(super) fn finish_with(state: &mut AppState, doc_id: DocId, label: &str) {
    // Rope stabilizer: complete the stroke to the pointer.
    if let Some(ToolSession::Stroke { extra, .. }) = &state.session {
        if let (Some(st), Some(raw)) = (&extra.stabilizer, extra.last_raw) {
            if st.catches_up() {
                if let Some(from) = st.current() {
                    if from.dist(raw.pos) > 0.5 {
                        feed_line(state, doc_id, from, raw.pos, raw.pressure);
                    }
                }
            }
        }
    }
    let Some(ToolSession::Stroke { mut engine, layer, mut extra }) = state.session.take() else { return };
    state.session_doc = None;
    let Some(entry) = state.docs.iter_mut().find(|d| d.id == doc_id) else { return };
    let raster = &mut entry.doc.state_mut().layers[layer].raster;
    let mut dirty = engine.finish(raster);
    for (_, m) in extra.mirrors.iter_mut() {
        let d = m.finish(raster);
        dirty = if dirty.is_empty() {
            d
        } else if d.is_empty() {
            dirty
        } else {
            dirty.union(&d)
        };
    }
    if !dirty.is_empty() {
        entry.doc.mark_dirty_rect(dirty);
    }
    if engine.dab_count() > 0 {
        entry.doc.commit(label);
    } else {
        entry.doc.revert_working();
    }
}

pub fn handle_stroke(state: &mut AppState, doc_id: DocId, tool: ToolKind, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            if state.session.is_some() {
                return;
            }
            // Shift+click: straight line from the end of the previous stroke.
            let shift_line = if inp.mods.shift {
                match state.last_stroke_end {
                    Some((d, p)) if d == doc_id => Some(p),
                    _ => None,
                }
            } else {
                None
            };
            let Some((engine, layer, mut extra)) = begin_engine_with(state, doc_id, tool) else { return };
            if shift_line.is_none() {
                let zoom = state.doc(doc_id).map(|d| d.view.zoom).unwrap_or(1.0);
                extra.stabilizer = Stabilizer::new(engine.settings(), zoom);
            }
            state.session = Some(ToolSession::Stroke { engine, layer, extra });
            state.session_doc = Some(doc_id);
            if let Some(from) = shift_line {
                feed_line(state, doc_id, from, inp.doc, inp.pressure);
                state.last_stroke_end = Some((doc_id, inp.doc));
                finish(state, doc_id, tool);
                return;
            }
            feed_raw(state, doc_id, StrokeSample { pos: inp.doc, pressure: inp.pressure });
            state.last_stroke_end = Some((doc_id, inp.doc));
        }
        CanvasEvent::Drag(inp) => {
            if matches!(state.session, Some(ToolSession::Stroke { .. })) {
                feed_raw(state, doc_id, StrokeSample { pos: inp.doc, pressure: inp.pressure });
                state.last_stroke_end = Some((doc_id, inp.doc));
            }
        }
        CanvasEvent::Release(_) => {
            if matches!(state.session, Some(ToolSession::Stroke { .. })) {
                finish(state, doc_id, tool);
            }
        }
        _ => {}
    }
}

/// Feed evenly spaced samples along a straight segment.
pub(super) fn feed_line(state: &mut AppState, doc_id: DocId, a: Pt, b: Pt, pressure: f32) {
    let len = a.dist(b);
    let steps = (len / 0.5).ceil().max(1.0) as usize;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        feed(state, doc_id, StrokeSample { pos: a.lerp(b, t), pressure });
    }
}

pub fn constrain_line(start: Pt, cur: Pt, snap: bool) -> Pt {
    if !snap {
        return cur;
    }
    let dx = cur.x - start.x;
    let dy = cur.y - start.y;
    let ang = dy.atan2(dx);
    let step = std::f32::consts::FRAC_PI_4;
    let snapped = (ang / step).round() * step;
    let len = (dx * dx + dy * dy).sqrt();
    Pt::new(start.x + len * snapped.cos(), start.y + len * snapped.sin())
}

pub fn ellipse_points(r: IRect, n: usize) -> Vec<Pt> {
    let cx = r.x as f32 + r.w as f32 / 2.0;
    let cy = r.y as f32 + r.h as f32 / 2.0;
    let rx = r.w as f32 / 2.0;
    let ry = r.h as f32 / 2.0;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            Pt::new(cx + rx * t.cos(), cy + ry * t.sin())
        })
        .collect()
}

pub fn handle_shape(state: &mut AppState, doc_id: DocId, tool: ToolKind, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            if state.session.is_none() {
                state.session = Some(ToolSession::Shape { start: inp.doc, cur: inp.doc, mods: inp.mods });
                state.session_doc = Some(doc_id);
            }
        }
        CanvasEvent::Drag(inp) => {
            if let Some(ToolSession::Shape { cur, mods, .. }) = &mut state.session {
                *cur = inp.doc;
                *mods = inp.mods;
            }
        }
        CanvasEvent::Release(inp) => {
            let Some(ToolSession::Shape { start, mods, .. }) = state.session.take() else { return };
            state.session_doc = None;
            let cur = inp.doc;
            commit_shape(state, doc_id, tool, start, cur, mods, inp);
        }
        _ => {}
    }
}

fn commit_shape(
    state: &mut AppState,
    doc_id: DocId,
    tool: ToolKind,
    start: Pt,
    cur: Pt,
    mods: egui::Modifiers,
    inp: CanvasInput,
) {
    let filled = state.tool_opts.shape_filled && tool != ToolKind::Line;
    if filled {
        let r = rect_from_drag(start, cur, mods.shift);
        if r.is_empty() {
            return;
        }
        let fg = state.fg;
        let Some(entry) = state.doc_mut(doc_id) else { return };
        let (w, h) = (entry.doc.width(), entry.doc.height());
        let s = entry.doc.state_mut();
        let li = s.active;
        if !s.layer_editable(li) {
            state.toasts.push(Level::Info, "The active layer is locked or hidden.");
            return;
        }
        let shape_mask = match tool {
            ToolKind::Rect => qsketch_core::Mask::from_rect(w, h, r),
            _ => qsketch_core::Mask::from_ellipse(w, h, r),
        };
        let mask = match &s.selection {
            Some(sel) => shape_mask.intersect(sel),
            None => shape_mask,
        };
        let saved = s.selection.take();
        s.selection = Some(std::sync::Arc::new(mask));
        let dirty = qsketch_core::ops::fill(s, li, fg);
        s.selection = saved;
        entry.doc.mark_dirty_rect(dirty);
        entry.doc.commit(stroke_label(tool));
        return;
    }
    let Some((engine, layer, extra)) = begin_engine_with(state, doc_id, ToolKind::Brush) else { return };
    state.session = Some(ToolSession::Stroke { engine, layer, extra });
    state.session_doc = Some(doc_id);
    let _ = inp;
    match tool {
        ToolKind::Line => {
            let end = constrain_line(start, cur, mods.shift);
            feed_line(state, doc_id, start, end, 1.0);
        }
        ToolKind::Rect => {
            let r = rect_from_drag(start, cur, mods.shift);
            let pts = [
                Pt::new(r.x as f32, r.y as f32),
                Pt::new(r.right() as f32, r.y as f32),
                Pt::new(r.right() as f32, r.bottom() as f32),
                Pt::new(r.x as f32, r.bottom() as f32),
                Pt::new(r.x as f32, r.y as f32),
            ];
            for w in pts.windows(2) {
                feed_line(state, doc_id, w[0], w[1], 1.0);
            }
        }
        _ => {
            let r = rect_from_drag(start, cur, mods.shift);
            let n = ((r.w.max(r.h) as f32 * 0.8).clamp(24.0, 512.0)) as usize;
            let mut pts = ellipse_points(r, n);
            pts.push(pts[0]);
            for w in pts.windows(2) {
                feed_line(state, doc_id, w[0], w[1], 1.0);
            }
        }
    }
    finish(state, doc_id, tool);
}
