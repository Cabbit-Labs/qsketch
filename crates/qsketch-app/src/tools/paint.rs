//! Brush, pencil, eraser and the shape tools (line/rectangle/ellipse), all of
//! which go through the core stroke engine.

use std::collections::VecDeque;
use std::sync::Arc;

use qsketch_core::shape::Spans;
use qsketch_core::{
    BrushSettings, IRect, Layer, Mask, PaintMode, Pt, Raster, Rgba8, StabilizerMode, StrokeEngine, StrokeSample,
};

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
    pub target: StrokeTarget,
}

/// Where a stroke's dabs land.
#[derive(Default)]
pub enum StrokeTarget {
    /// The active layer's raster (`ToolSession::Stroke::layer`).
    #[default]
    Layer,
    /// The Selection Brush: dabs go into a scratch raster whose alpha is
    /// merged live into the selection on top of `base` (the selection before
    /// the stroke), adding by default or taking away when `erase` is set.
    Selection { scratch: Raster, base: Option<Arc<Mask>>, erase: bool },
    /// Painting the active layer's mask: dabs go into a scratch raster whose
    /// alpha blends `value` (the paint color's gray) into the mask over
    /// `base` (the mask before the stroke).
    Mask { scratch: Raster, base: Arc<Mask>, value: u8 },
}

/// Photoshop paints a mask with the color's gray: white reveals, black hides.
fn gray_of(c: Rgba8) -> u8 {
    (0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32 + 0.5) as u8
}

/// Blend the scratch raster's alpha inside `dirty` into the layer's mask.
fn merge_mask_stroke(
    doc_state: &mut qsketch_core::DocState,
    li: usize,
    scratch: &Raster,
    base: &Mask,
    value: u8,
    dirty: IRect,
) {
    let Some(m) = doc_state.layers.get_mut(li).and_then(|l| l.mask.as_mut()) else { return };
    let m = Arc::make_mut(m);
    let r = dirty.intersect(&m.rect());
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let a = scratch.get_pixel(x, y).a as i32;
            if a == 0 {
                continue;
            }
            let b = base.get(x, y) as i32;
            m.set(x, y, (b + (value as i32 - b) * a / 255) as u8);
        }
    }
    m.expand_bounds(r);
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
        ToolKind::SelectBrush => (state.select_brush.clone(), PaintMode::Paint, Rgba8::WHITE),
        ToolKind::Smudge => (state.smudge.clone(), PaintMode::Smudge, state.fg),
        ToolKind::Clone => (state.clone.clone(), PaintMode::Clone, state.fg),
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
    let (settings, mut mode, mut color) = brush_for(state, tool);
    // Shading ink: the selected palette run (or the whole palette) is the
    // ramp; each pixel under the brush steps along it.
    let shade_ramp: Option<Vec<Rgba8>> = if state.tool_opts.shading
        && matches!(tool, ToolKind::Brush | ToolKind::Pencil)
        && state.doc(doc_id).is_some_and(|d| !d.editing_mask())
    {
        state.doc(doc_id).and_then(|d| {
            let pal = &d.doc.state().palette;
            if pal.is_empty() {
                return None;
            }
            let (a, b) = match d.palette_sel {
                Some((a, b)) if a != b => (a.min(b), a.max(b).min(pal.len() - 1)),
                _ => (0, pal.len() - 1),
            };
            Some(pal.colors[a..=b].to_vec())
        })
    } else {
        None
    };
    if state.tool_opts.shading && matches!(tool, ToolKind::Brush | ToolKind::Pencil) && shade_ramp.is_none() {
        state.toasts.push(Level::Info, "Shading needs a palette: add colors in the Palette panel.");
        return None;
    }
    if shade_ramp.is_some() {
        mode = PaintMode::Shade;
    }
    let shade_dir = if state.tool_opts.shading_reverse { -1 } else { 1 };
    let (tip, texture) = state.library.resolve(&settings);
    let bg = if mode == PaintMode::Erase { state.fg } else { state.bg };
    let to_selection = tool.is_selection_brush();
    if mode == PaintMode::Paint && !to_selection {
        state.note_color_used(color);
    }
    // Into the mask: the engine deposits coverage, the merge decides the gray.
    let to_mask = !to_selection && state.doc(doc_id).is_some_and(|d| d.editing_mask());
    let mask_value = gray_of(color);
    if to_mask {
        mode = PaintMode::Paint;
        color = Rgba8::WHITE;
    }
    let symmetry = state.symmetry;
    let zoom = state.doc(doc_id).map(|d| d.view.zoom).unwrap_or(1.0);
    let erase_sel = state.tool_opts.select_brush_erase;
    let clone_offset = state.tool_opts.clone_offset;
    let clone_merged = state.tool_opts.clone_sample_merged;
    if mode == PaintMode::Clone && clone_offset.is_none() {
        state.toasts.push(Level::Info, "Alt+click to set the clone source first.");
        return None;
    }
    let entry = state.doc_mut(doc_id)?;
    let (w, h) = (entry.doc.width(), entry.doc.height());
    let s = entry.doc.state();
    let li = s.active;
    // The selection brush paints into a blank scratch layer, unclipped by
    // the selection it is editing and regardless of layer locks.
    let scratch_layer = to_selection.then(|| Layer::new(u64::MAX, "selection", w, h));
    let layer = match &scratch_layer {
        Some(l) => l,
        None => &s.layers[li],
    };
    // Hidden or locked, including by an enclosing group: nothing to paint on.
    if !to_selection && !s.layer_editable(li) {
        let why = if !s.effectively_visible(li) { "hidden" } else { "locked" };
        state.toasts.push(Level::Info, format!("The active layer is {why}."));
        return None;
    }
    let erase_alpha_locked = mode == PaintMode::Erase && layer.props.alpha_locked;
    // The clone stamp needs a source: the layer (or everything) as it is now,
    // read at the offset the press established.
    let clone_src: Option<(Arc<Raster>, i32, i32)> = if let (PaintMode::Clone, Some((dx, dy))) = (mode, clone_offset) {
        let src = if clone_merged {
            let comp = &entry.doc.composite;
            Raster::from_rgba(comp.width(), comp.height(), &comp.to_rgba_straight())
        } else {
            layer.raster.clone()
        };
        Some((Arc::new(src), dx, dy))
    } else {
        None
    };
    let clip = if to_selection { None } else { s.selection.clone() };
    let target = if to_selection {
        StrokeTarget::Selection { scratch: Raster::new(w, h), base: s.selection.clone(), erase: erase_sel }
    } else if to_mask {
        StrokeTarget::Mask { scratch: Raster::new(w, h), base: s.layers[li].mask.clone().unwrap(), value: mask_value }
    } else {
        StrokeTarget::Layer
    };
    let make = |seed: u64| {
        let e = StrokeEngine::new(settings.clone(), mode, color, layer, clip.clone())
            .with_tip(tip.clone())
            .with_texture(texture.clone())
            .with_background(bg)
            .with_seed(seed)
            .with_zoom(zoom);
        let e = match &clone_src {
            Some((src, dx, dy)) => e.with_clone_source(src.clone(), *dx, *dy),
            None => e,
        };
        match &shade_ramp {
            Some(ramp) => e.with_shading(ramp.clone(), shade_dir),
            None => e,
        }
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
    Some((Box::new(engine), li, StrokeExtra { mirrors, stabilizer: None, last_raw: None, target }))
}

fn stroke_label(tool: ToolKind) -> &'static str {
    match tool {
        ToolKind::Pencil => "Pencil",
        ToolKind::Eraser => "Eraser",
        ToolKind::SelectBrush => "Selection Brush",
        ToolKind::Smudge => "Smudge",
        ToolKind::Clone => "Clone Stamp",
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
    let doc_state = entry.doc.state_mut();
    let raster = match &mut extra.target {
        StrokeTarget::Layer => &mut doc_state.layers[li].raster,
        StrokeTarget::Selection { scratch, .. } | StrokeTarget::Mask { scratch, .. } => scratch,
    };
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
    if dirty.is_empty() {
        return;
    }
    match &extra.target {
        StrokeTarget::Layer => entry.doc.mark_dirty_rect(dirty),
        StrokeTarget::Selection { scratch, base, erase } => {
            merge_selection_stroke(doc_state, scratch, base.as_deref(), *erase, dirty);
            entry.sel_outline = None;
        }
        StrokeTarget::Mask { scratch, base, value } => {
            merge_mask_stroke(doc_state, li, scratch, base, *value, dirty);
            entry.doc.mark_dirty_rect(dirty);
        }
    }
}

/// Copy the scratch raster's alpha inside `dirty` into the working
/// selection, on top of `base`: adding (max) or, with `erase`, cutting (min
/// with the inverse), so a stroke can never eat into what it did not cover.
fn merge_selection_stroke(
    doc_state: &mut qsketch_core::DocState,
    scratch: &Raster,
    base: Option<&Mask>,
    erase: bool,
    dirty: IRect,
) {
    let (w, h) = (scratch.width(), scratch.height());
    let sel = doc_state.selection.get_or_insert_with(|| Arc::new(Mask::new(w, h)));
    let m = Arc::make_mut(sel);
    let r = dirty.intersect(&m.rect());
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let v = scratch.get_pixel(x, y).a;
            let b = base.map(|b| b.get(x, y)).unwrap_or(0);
            let out = if erase { b.min(255 - v) } else { b.max(v) };
            m.set(x, y, out);
        }
    }
    if !erase {
        m.expand_bounds(r);
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
    let doc_state = entry.doc.state_mut();
    let raster = match &mut extra.target {
        StrokeTarget::Layer => &mut doc_state.layers[layer].raster,
        StrokeTarget::Selection { scratch, .. } | StrokeTarget::Mask { scratch, .. } => scratch,
    };
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
    match &extra.target {
        StrokeTarget::Layer => {
            if !dirty.is_empty() {
                entry.doc.mark_dirty_rect(dirty);
            }
        }
        StrokeTarget::Selection { scratch, base, erase } => {
            if !dirty.is_empty() {
                merge_selection_stroke(doc_state, scratch, base.as_deref(), *erase, dirty);
            }
            // Exact bounds (an erase stroke may have shrunk them), and an
            // all-clear selection is no selection.
            if let Some(sel) = &mut doc_state.selection {
                Arc::make_mut(sel).recompute_bounds();
                if sel.is_empty() {
                    doc_state.selection = None;
                }
            }
            entry.sel_outline = None;
        }
        StrokeTarget::Mask { scratch, base, value } => {
            if !dirty.is_empty() {
                merge_mask_stroke(doc_state, layer, scratch, base, *value, dirty);
                entry.doc.mark_dirty_rect(dirty);
            }
        }
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
            // Clone stamp: the offset from where the stroke starts to the
            // source point, kept between strokes when aligned.
            if tool == ToolKind::Clone {
                let o = &mut state.tool_opts;
                match o.clone_source {
                    Some((d, src)) if d == doc_id => {
                        if !o.clone_aligned || o.clone_offset.is_none() {
                            o.clone_offset =
                                Some(((src.x - inp.doc.x).round() as i32, (src.y - inp.doc.y).round() as i32));
                        }
                    }
                    _ => o.clone_offset = None,
                }
            }
            let Some((engine, layer, mut extra)) = begin_engine_with(state, doc_id, tool) else { return };
            // Alt flips the selection brush between select and deselect for
            // this one stroke, the way Alt means subtract for the marquees.
            if let StrokeTarget::Selection { erase, .. } = &mut extra.target {
                if inp.mods.alt {
                    *erase = !*erase;
                }
            }
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
    // A hard round tip walks the pixel grid itself (`StrokeEngine::extend`);
    // fed in half-pixel slices it would round each slice separately and turn
    // a diagonal back into a staircase of corners. Give it the endpoints.
    let hard = matches!(&state.session, Some(ToolSession::Stroke { engine, .. })
        if !engine.settings().antialias && engine.settings().is_round());
    if hard {
        feed(state, doc_id, StrokeSample { pos: a, pressure });
        feed(state, doc_id, StrokeSample { pos: b, pressure });
        return;
    }
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

pub fn handle_shape(state: &mut AppState, doc_id: DocId, tool: ToolKind, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            // Refuse at press, not release, so a hidden or locked layer never
            // shows a shape being dragged that could not land anywhere.
            if let Some(entry) = state.doc(doc_id) {
                let s = entry.doc.state();
                if !s.layer_editable(s.active) {
                    let why = if !s.effectively_visible(s.active) { "hidden" } else { "locked" };
                    state.toasts.push(Level::Info, format!("The active layer is {why}."));
                    return;
                }
            }
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
    let _ = inp;
    // Rectangles and ellipses are pixel art: they paint discrete pixels in the
    // foreground color, not brush dabs.
    if matches!(tool, ToolKind::Rect | ToolKind::Ellipse) {
        let r = rect_from_drag(start, cur, mods.shift);
        if r.is_empty() {
            return;
        }
        let fg = state.fg;
        let (outer, hole) = shape_spans_for(state, tool, r);
        let sym = state.symmetry;
        let Some(entry) = state.doc_mut(doc_id) else { return };
        let (w, h) = (entry.doc.width(), entry.doc.height());
        let s = entry.doc.state_mut();
        let li = s.active;
        if !s.layer_editable(li) {
            state.toasts.push(Level::Info, "The active layer is locked or hidden.");
            return;
        }
        let mut shape_mask = qsketch_core::shape::mask_from_spans(w, h, &outer, &hole);
        shape_mask = super::symmetry::mirror_mask(&sym, &shape_mask, w, h);
        let mask = match &s.selection {
            Some(sel) => shape_mask.intersect(sel),
            None => shape_mask,
        };
        if mask.is_empty() {
            return;
        }
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
    let end = constrain_line(start, cur, mods.shift);
    feed_line(state, doc_id, start, end, 1.0);
    finish(state, doc_id, tool);
}

/// Outer and hole rows of the shape the current options describe.
pub fn shape_spans_for(state: &AppState, tool: ToolKind, r: IRect) -> (Spans, Spans) {
    qsketch_core::shape::shape_spans(
        r,
        tool == ToolKind::Ellipse,
        state.tool_opts.shape_filled,
        state.tool_opts.shape_thickness as i32,
    )
}
