//! Floating paste: pasted pixels hover over the active layer and can be moved
//! and scaled with handles before being committed (Enter / double-click) or
//! discarded (Esc). While floating, the working state already contains the
//! preview so the regular composite/render path shows it; committing just
//! records a history step.

use egui::{Color32, Pos2, Rect, Stroke, Vec2};
use qsketch_core::ops::{drop_floating, Floating, Orient};
use qsketch_core::raster::ResizeFilter;
use qsketch_core::{IRect, Pt, Raster};

use super::CanvasEvent;
use crate::state::{AppState, DocId};

/// Handle positions around the floating rect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// Unit position of the handle on the rect (0 = min edge, 1 = max edge).
    fn uv(self) -> (f32, f32) {
        match self {
            Handle::TopLeft => (0.0, 0.0),
            Handle::Top => (0.5, 0.0),
            Handle::TopRight => (1.0, 0.0),
            Handle::Right => (1.0, 0.5),
            Handle::BottomRight => (1.0, 1.0),
            Handle::Bottom => (0.5, 1.0),
            Handle::BottomLeft => (0.0, 1.0),
            Handle::Left => (0.0, 0.5),
        }
    }

    fn is_corner(self) -> bool {
        matches!(self, Handle::TopLeft | Handle::TopRight | Handle::BottomRight | Handle::BottomLeft)
    }

    fn moves_x(self) -> bool {
        !matches!(self, Handle::Top | Handle::Bottom)
    }

    fn moves_y(self) -> bool {
        !matches!(self, Handle::Left | Handle::Right)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum FloatDrag {
    Move { start: Pt, start_rect: [f32; 4] },
    Scale { handle: Handle, start: Pt, start_rect: [f32; 4] },
}

pub struct FloatingPaste {
    pub doc: DocId,
    pub layer: usize,
    /// The pasted pixels at their original size.
    pub source: Raster,
    /// The layer raster before the paste (the preview is composited onto it).
    pub base: Raster,
    /// Current placement in document space (x, y, w, h), fractional while dragging.
    pub rect: [f32; 4],
    pub drag: Option<FloatDrag>,
    pub last_dirty: IRect,
    pub keep_aspect: bool,
    pub filter: ResizeFilter,
    /// The preview must be re-rendered into the working layer.
    dirty: bool,
}

impl FloatingPaste {
    pub fn new(doc: DocId, layer: usize, source: Raster, base: Raster, place: IRect) -> Self {
        Self {
            doc,
            layer,
            source,
            base,
            rect: [place.x as f32, place.y as f32, place.w.max(1) as f32, place.h.max(1) as f32],
            drag: None,
            last_dirty: IRect::EMPTY,
            keep_aspect: true,
            filter: ResizeFilter::Bilinear,
            dirty: true,
        }
    }

    /// Integer placement (rounded).
    pub fn irect(&self) -> IRect {
        let [x, y, w, h] = self.rect;
        IRect::new(x.round() as i32, y.round() as i32, (w.round() as i32).max(1), (h.round() as i32).max(1))
    }

    pub fn is_scaled(&self) -> bool {
        let r = self.irect();
        r.w as u32 != self.source.width() || r.h as u32 != self.source.height()
    }

    pub fn set_rect(&mut self, r: IRect) {
        self.rect = [r.x as f32, r.y as f32, r.w.max(1) as f32, r.h.max(1) as f32];
        self.dirty = true;
    }

    pub fn nudge(&mut self, dx: i32, dy: i32) {
        self.rect[0] += dx as f32;
        self.rect[1] += dy as f32;
        self.dirty = true;
    }

    pub fn reset_size(&mut self) {
        let c = self.irect().center();
        let (w, h) = (self.source.width() as f32, self.source.height() as f32);
        self.rect = [(c.x - w / 2.0).round(), (c.y - h / 2.0).round(), w, h];
        self.dirty = true;
    }

    /// Rotate / flip the floating pixels about their center.
    pub fn orient(&mut self, o: Orient) {
        let c = self.irect().center();
        let r = self.irect();
        self.source = o.apply_raster(&self.source);
        let (w, h) = match o {
            Orient::Rotate(t) if t % 2 == 1 => (r.h as f32, r.w as f32),
            _ => (r.w as f32, r.h as f32),
        };
        self.rect = [(c.x - w / 2.0).round(), (c.y - h / 2.0).round(), w, h];
        self.dirty = true;
    }

    /// Screen-space handle rects for hit-testing and drawing.
    pub fn handle_rects(&self, view: &crate::canvas::view::CanvasView) -> [(Handle, Rect); 8] {
        let [x, y, w, h] = self.rect;
        let mut out = [(Handle::TopLeft, Rect::NOTHING); 8];
        for (i, hd) in Handle::ALL.iter().enumerate() {
            let (u, v) = hd.uv();
            let p = view.doc_to_screen(Pt::new(x + w * u, y + h * v));
            out[i] = (*hd, Rect::from_center_size(p, Vec2::splat(12.0)));
        }
        out
    }

    fn handle_at(&self, view: &crate::canvas::view::CanvasView, pos: Pos2) -> Option<Handle> {
        self.handle_rects(view).iter().find(|(_, r)| r.contains(pos)).map(|(h, _)| *h)
    }

    /// Render the preview into the working layer if anything changed.
    fn render_into(&mut self, entry: &mut crate::state::DocEntry, filter: ResizeFilter) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let r = self.irect();
        let scaled = if r.w as u32 == self.source.width() && r.h as u32 == self.source.height() {
            self.source.clone()
        } else {
            self.source.resized(r.w as u32, r.h as u32, filter)
        };
        let f = Floating { raster: scaled, origin: (r.x, r.y), mask: None };
        let s = entry.doc.state_mut();
        if self.layer < s.layers.len() {
            s.layers[self.layer].raster = drop_floating(&self.base, &f, 0, 0);
        }
        let doc_rect = entry.doc.state().rect();
        let dirty = r.union(&self.last_dirty).intersect(&doc_rect);
        entry.doc.mark_dirty_rect(dirty);
        self.last_dirty = r;
    }
}

/// Re-render the floating preview into the working state (call once per frame
/// or after any change). Uses nearest-neighbor while a scale drag is active.
pub fn refresh(state: &mut AppState) {
    let Some(mut fp) = state.floating.take() else { return };
    let filter = match fp.drag {
        Some(FloatDrag::Scale { .. }) => ResizeFilter::Nearest,
        _ => fp.filter,
    };
    if let Some(entry) = state.doc_mut(fp.doc) {
        fp.render_into(entry, filter);
    }
    state.floating = Some(fp);
}

/// Begin floating `source` over the active layer of `doc_id` at `(x, y)` with
/// the given display size. Any existing floating paste is committed first.
pub fn begin(state: &mut AppState, doc_id: DocId, source: Raster, x: i32, y: i32, w: u32, h: u32) -> bool {
    commit(state);
    state.cancel_session();
    let Some(entry) = state.doc_mut(doc_id) else { return false };
    let s = entry.doc.state_mut();
    let li = s.active;
    if !s.layers[li].editable() {
        state.toasts.push(crate::ui::toasts::Level::Info, "The active layer is locked or hidden.");
        return false;
    }
    let base = s.layers[li].raster.clone();
    s.selection = None;
    entry.sel_outline = None;
    let place = IRect::new(x, y, w as i32, h as i32);
    state.floating = Some(FloatingPaste::new(doc_id, li, source, base, place));
    refresh(state);
    true
}

/// Commit the floating paste as a history step. Returns true if there was one.
pub fn commit(state: &mut AppState) -> bool {
    let Some(mut fp) = state.floating.take() else { return false };
    fp.drag = None;
    fp.dirty = true;
    if let Some(entry) = state.doc_mut(fp.doc) {
        fp.render_into(entry, fp.filter);
        entry.doc.commit("Paste");
        entry.sel_outline = None;
    }
    true
}

/// Discard the floating paste, restoring the layer.
pub fn cancel(state: &mut AppState) -> bool {
    let Some(fp) = state.floating.take() else { return false };
    if let Some(entry) = state.doc_mut(fp.doc) {
        entry.doc.revert_working();
        entry.sel_outline = None;
    }
    true
}

/// Pointer handling while a paste is floating. Returns true if the event was consumed.
pub fn handle(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) -> bool {
    let Some(fp) = state.floating.as_mut() else { return false };
    if fp.doc != doc_id {
        return false;
    }
    let Some(view) = state.docs.iter().find(|d| d.id == doc_id).map(|d| d.view.clone()) else { return false };
    match ev {
        CanvasEvent::Press(inp) => {
            if inp.button != egui::PointerButton::Primary {
                return false;
            }
            let start_rect = fp.rect;
            fp.drag = Some(match fp.handle_at(&view, inp.screen) {
                Some(handle) => FloatDrag::Scale { handle, start: inp.doc, start_rect },
                None => FloatDrag::Move { start: inp.doc, start_rect },
            });
            true
        }
        CanvasEvent::Drag(inp) => {
            let Some(drag) = fp.drag else { return true };
            match drag {
                FloatDrag::Move { start, start_rect } => {
                    let mut dx = inp.doc.x - start.x;
                    let mut dy = inp.doc.y - start.y;
                    if inp.mods.shift {
                        if dx.abs() > dy.abs() {
                            dy = 0.0;
                        } else {
                            dx = 0.0;
                        }
                    }
                    let nx = (start_rect[0] + dx).round();
                    let ny = (start_rect[1] + dy).round();
                    if nx != fp.rect[0] || ny != fp.rect[1] {
                        fp.rect[0] = nx;
                        fp.rect[1] = ny;
                        fp.dirty = true;
                    }
                }
                FloatDrag::Scale { handle, start, start_rect } => {
                    let [sx, sy, sw, sh] = start_rect;
                    let dx = if handle.moves_x() { inp.doc.x - start.x } else { 0.0 };
                    let dy = if handle.moves_y() { inp.doc.y - start.y } else { 0.0 };
                    let (u, v) = handle.uv();
                    // Anchor is the opposite edge/corner.
                    let (ax, ay) = (sx + sw * (1.0 - u), sy + sh * (1.0 - v));
                    let (mx, my) = (sx + sw * u + dx, sy + sh * v + dy);
                    let mut nw = if handle.moves_x() { (mx - ax).abs() } else { sw };
                    let mut nh = if handle.moves_y() { (my - ay).abs() } else { sh };
                    // Corners keep the aspect ratio unless Shift is held; edges never do.
                    let keep = fp.keep_aspect != inp.mods.shift;
                    if handle.is_corner() && keep && sw > 0.0 && sh > 0.0 {
                        let k = (nw / sw).max(nh / sh);
                        nw = sw * k;
                        nh = sh * k;
                    }
                    let nw = nw.max(1.0);
                    let nh = nh.max(1.0);
                    let nx = if !handle.moves_x() {
                        sx
                    } else if mx < ax {
                        ax - nw
                    } else {
                        ax
                    };
                    let ny = if !handle.moves_y() {
                        sy
                    } else if my < ay {
                        ay - nh
                    } else {
                        ay
                    };
                    let new = [nx.round(), ny.round(), nw.round(), nh.round()];
                    if new != fp.rect {
                        fp.rect = new;
                        fp.dirty = true;
                    }
                }
            }
            true
        }
        CanvasEvent::Release(_) => {
            if fp.drag.take().is_some() {
                // Switch back from the nearest-neighbor drag preview.
                fp.dirty = true;
            }
            true
        }
        CanvasEvent::DoubleClick(_) => {
            commit(state);
            true
        }
        CanvasEvent::Hover(inp) => {
            let _ = inp;
            true
        }
        CanvasEvent::Cancel => {
            cancel(state);
            true
        }
    }
}

/// Cursor for the pointer position while floating.
pub fn cursor(state: &AppState, doc_id: DocId, pos: Option<Pos2>) -> Option<egui::CursorIcon> {
    let fp = state.floating.as_ref()?;
    if fp.doc != doc_id {
        return None;
    }
    let view = &state.doc(doc_id)?.view;
    let handle = match fp.drag {
        Some(FloatDrag::Scale { handle, .. }) => Some(handle),
        Some(FloatDrag::Move { .. }) => return Some(egui::CursorIcon::Grabbing),
        None => pos.and_then(|p| fp.handle_at(view, p)),
    };
    Some(match handle {
        Some(Handle::TopLeft | Handle::BottomRight) => egui::CursorIcon::ResizeNwSe,
        Some(Handle::TopRight | Handle::BottomLeft) => egui::CursorIcon::ResizeNeSw,
        Some(Handle::Top | Handle::Bottom) => egui::CursorIcon::ResizeVertical,
        Some(Handle::Left | Handle::Right) => egui::CursorIcon::ResizeHorizontal,
        None => egui::CursorIcon::Move,
    })
}

/// Outline + handles overlay.
pub fn draw_overlay(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    let Some(fp) = state.floating.as_ref() else { return };
    if fp.doc != doc_id {
        return;
    }
    let Some(entry) = state.doc(doc_id) else { return };
    let view = &entry.view;
    let [x, y, w, h] = fp.rect;
    let pts = vec![
        view.doc_to_screen(Pt::new(x, y)),
        view.doc_to_screen(Pt::new(x + w, y)),
        view.doc_to_screen(Pt::new(x + w, y + h)),
        view.doc_to_screen(Pt::new(x, y + h)),
    ];
    let accent = Color32::from_rgb(23, 227, 180);
    painter.add(egui::Shape::closed_line(pts.clone(), Stroke::new(3.0, Color32::from_black_alpha(120))));
    painter.add(egui::Shape::dashed_line(
        &[pts[0], pts[1], pts[2], pts[3], pts[0]],
        Stroke::new(1.0, accent),
        6.0,
        4.0,
    ));
    for (_, r) in fp.handle_rects(view) {
        let r = r.shrink(2.0);
        painter.rect_filled(r, 1.0, Color32::from_black_alpha(160));
        painter.rect_stroke(r.shrink(1.0), 1.0, Stroke::new(1.5, accent), egui::StrokeKind::Inside);
    }
}
