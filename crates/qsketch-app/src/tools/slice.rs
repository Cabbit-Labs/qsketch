//! Slice tool: drag on empty canvas to add a slice, drag a slice to move
//! it, drag one of the selected slice's corners to resize it, double-click
//! a slice for its properties. Delete removes the selected slice.

use egui::{Color32, Stroke};
use qsketch_core::slice::{unique_name, Slice};
use qsketch_core::{IRect, Pt};

use super::{CanvasEvent, ToolSession};
use crate::state::{AppState, DocId};

/// What a slice drag is doing.
#[derive(Clone, Copy, Debug)]
pub enum SliceDrag {
    Create {
        start: Pt,
    },
    Move {
        index: usize,
        grab: Pt,
        orig: IRect,
    },
    /// `corner`: 0 top-left, 1 top-right, 2 bottom-right, 3 bottom-left.
    Resize {
        index: usize,
        corner: u8,
        orig: IRect,
    },
}

/// Screen distance (points) within which a corner handle is grabbed.
const HANDLE: f32 = 7.0;

fn corners(r: IRect) -> [Pt; 4] {
    let (x0, y0, x1, y1) = (r.x as f32, r.y as f32, r.right() as f32, r.bottom() as f32);
    [Pt::new(x0, y0), Pt::new(x1, y0), Pt::new(x1, y1), Pt::new(x0, y1)]
}

/// The slice under a document point, topmost (last drawn) first.
pub fn slice_at(slices: &[Slice], p: Pt) -> Option<usize> {
    slices
        .iter()
        .enumerate()
        .rev()
        .find(|(_, s)| {
            let r = s.rect;
            p.x >= r.x as f32 && p.y >= r.y as f32 && p.x < r.right() as f32 && p.y < r.bottom() as f32
        })
        .map(|(i, _)| i)
}

pub fn handle(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            if state.session.is_some() {
                return;
            }
            let Some(entry) = state.doc_mut(doc_id) else { return };
            let view = entry.view.clone();
            let slices = entry.doc.state().slices.clone();
            let p = inp.doc;
            // A corner of the selected slice resizes it.
            let corner = entry.slice_sel.and_then(|i| slices.get(i).map(|s| (i, s.rect))).and_then(|(i, r)| {
                corners(r)
                    .iter()
                    .position(|c| (view.doc_to_screen(*c) - inp.screen).length() <= HANDLE)
                    .map(|k| (i, k, r))
            });
            let drag = if let Some((index, k, orig)) = corner {
                SliceDrag::Resize { index, corner: k as u8, orig }
            } else if let Some(index) = slice_at(&slices, p) {
                entry.slice_sel = Some(index);
                SliceDrag::Move { index, grab: p, orig: slices[index].rect }
            } else {
                entry.slice_sel = None;
                SliceDrag::Create { start: p }
            };
            state.session = Some(ToolSession::Slice { drag, cur: p });
            state.session_doc = Some(doc_id);
        }
        CanvasEvent::Drag(inp) => {
            let Some(ToolSession::Slice { drag, cur }) = &mut state.session else { return };
            *cur = inp.doc;
            let (drag, p) = (*drag, inp.doc);
            let Some(entry) = state.doc_mut(doc_id) else { return };
            let s = entry.doc.state_mut();
            match drag {
                SliceDrag::Move { index, grab, orig } => {
                    if let Some(sl) = s.slices.get_mut(index) {
                        let (dx, dy) = ((p.x - grab.x).round() as i32, (p.y - grab.y).round() as i32);
                        sl.rect = IRect::new(orig.x + dx, orig.y + dy, orig.w, orig.h);
                    }
                }
                SliceDrag::Resize { index, corner, orig } => {
                    if let Some(sl) = s.slices.get_mut(index) {
                        sl.rect = resized(orig, corner, p);
                        sl.clamp_parts();
                    }
                }
                SliceDrag::Create { .. } => {}
            }
        }
        CanvasEvent::Release(inp) => {
            let Some(ToolSession::Slice { drag, .. }) = state.session.take() else { return };
            state.session_doc = None;
            let Some(entry) = state.doc_mut(doc_id) else { return };
            match drag {
                SliceDrag::Create { start } => {
                    let r = super::rect_from_drag(start, inp.doc, inp.mods.shift);
                    if r.w >= 1 && r.h >= 1 && start.dist(inp.doc) > 1.0 {
                        let s = entry.doc.state_mut();
                        let name = unique_name(&s.slices, "Slice");
                        s.slices.push(Slice::new(name, r));
                        entry.slice_sel = Some(s.slices.len() - 1);
                        entry.doc.commit("New Slice");
                    }
                }
                SliceDrag::Move { index, orig, .. } | SliceDrag::Resize { index, orig, .. } => {
                    let now = entry.doc.state().slices.get(index).map(|s| s.rect);
                    if now.is_some_and(|r| r != orig) {
                        let label = if matches!(drag, SliceDrag::Move { .. }) { "Move Slice" } else { "Resize Slice" };
                        entry.doc.commit(label);
                    }
                }
            }
        }
        CanvasEvent::DoubleClick(inp) => {
            let hit = state.doc(doc_id).and_then(|e| slice_at(&e.doc.state().slices, inp.doc));
            if let Some(i) = hit {
                if let Some(e) = state.doc_mut(doc_id) {
                    e.slice_sel = Some(i);
                }
                crate::dialogs::open_slice_props(state, doc_id, i);
            }
        }
        _ => {}
    }
}

/// `orig` with `corner` dragged to `p` (whole pixels, never inverted).
fn resized(orig: IRect, corner: u8, p: Pt) -> IRect {
    let (px, py) = (p.x.round() as i32, p.y.round() as i32);
    let (mut x0, mut y0, mut x1, mut y1) = (orig.x, orig.y, orig.right(), orig.bottom());
    match corner {
        0 => {
            x0 = px.min(x1 - 1);
            y0 = py.min(y1 - 1);
        }
        1 => {
            x1 = px.max(x0 + 1);
            y0 = py.min(y1 - 1);
        }
        2 => {
            x1 = px.max(x0 + 1);
            y1 = py.max(y0 + 1);
        }
        _ => {
            x0 = px.min(x1 - 1);
            y1 = py.max(y0 + 1);
        }
    }
    IRect::from_min_max(x0, y0, x1, y1)
}

/// Delete the selected slice; true when one was removed.
pub fn delete_selected(state: &mut AppState) -> bool {
    let Some(entry) = state.active_mut() else { return false };
    let Some(i) = entry.slice_sel.take() else { return false };
    let s = entry.doc.state_mut();
    if i >= s.slices.len() {
        return false;
    }
    s.slices.remove(i);
    entry.doc.commit("Delete Slice");
    true
}

/// Slice outlines, names, 9-slice guides and pivots. Drawn while the Slice
/// tool is active or View ▸ Show Slices is on.
pub fn draw_overlay(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    let Some(entry) = state.doc(doc_id) else { return };
    let tool_on = state.effective_tool() == super::ToolKind::Slice;
    if !tool_on && !state.settings.canvas.show_slices {
        return;
    }
    let view = &entry.view;
    let to_s = |p: Pt| view.doc_to_screen(p);
    let shadow = Stroke::new(3.0, Color32::from_black_alpha(110));
    for (i, s) in entry.doc.state().slices.iter().enumerate() {
        let c = Color32::from_rgb(s.color.r, s.color.g, s.color.b);
        let selected = tool_on && entry.slice_sel == Some(i);
        let pts: Vec<egui::Pos2> = corners(s.rect).iter().map(|p| to_s(*p)).collect();
        painter.add(egui::Shape::closed_line(pts.clone(), shadow));
        painter.add(egui::Shape::closed_line(pts.clone(), Stroke::new(if selected { 2.0 } else { 1.5 }, c)));
        // 9-slice center: the four guide lines across the slice.
        if let Some(ct) = s.center {
            let (ox, oy) = (s.rect.x as f32, s.rect.y as f32);
            let (l, t, r, b) = (ox + ct.x as f32, oy + ct.y as f32, ox + ct.right() as f32, oy + ct.bottom() as f32);
            let (sx0, sy0, sx1, sy1) = (ox, oy, s.rect.right() as f32, s.rect.bottom() as f32);
            let dash = Stroke::new(1.0, c.gamma_multiply(0.8));
            for (a, bb) in [
                (Pt::new(l, sy0), Pt::new(l, sy1)),
                (Pt::new(r, sy0), Pt::new(r, sy1)),
                (Pt::new(sx0, t), Pt::new(sx1, t)),
                (Pt::new(sx0, b), Pt::new(sx1, b)),
            ] {
                painter.add(egui::Shape::dashed_line(&[to_s(a), to_s(bb)], dash, 4.0, 3.0));
            }
        }
        if let Some((px, py)) = s.pivot {
            let q = to_s(Pt::new(s.rect.x as f32 + px as f32, s.rect.y as f32 + py as f32));
            for (a, b) in [(egui::vec2(-5.0, 0.0), egui::vec2(5.0, 0.0)), (egui::vec2(0.0, -5.0), egui::vec2(0.0, 5.0))]
            {
                painter.line_segment([q + a, q + b], shadow);
                painter.line_segment([q + a, q + b], Stroke::new(1.5, c));
            }
        }
        // Name tag at the top-left corner.
        let font = egui::FontId::proportional(11.0);
        let galley = painter.layout_no_wrap(s.name.clone(), font, Color32::WHITE);
        let at = pts[0] + egui::vec2(2.0, 2.0);
        let tag = egui::Rect::from_min_size(at, galley.size() + egui::vec2(6.0, 2.0));
        painter.rect_filled(tag, 2.0, c.gamma_multiply(0.85));
        painter.galley(at + egui::vec2(3.0, 1.0), galley, Color32::WHITE);
        if selected {
            for p in &pts {
                let r = egui::Rect::from_center_size(*p, egui::vec2(7.0, 7.0));
                painter.rect_filled(r, 1.0, Color32::WHITE);
                painter.rect_stroke(r, 1.0, Stroke::new(1.0, c), egui::StrokeKind::Inside);
            }
        }
    }
    // The slice being drawn.
    if state.session_doc == Some(doc_id) {
        if let Some(ToolSession::Slice { drag: SliceDrag::Create { start }, cur }) = &state.session {
            let r = super::rect_from_drag(*start, *cur, false);
            let pts: Vec<egui::Pos2> = corners(r).iter().map(|p| to_s(*p)).collect();
            painter.add(egui::Shape::closed_line(pts.clone(), shadow));
            painter.add(egui::Shape::closed_line(pts, Stroke::new(1.0, Color32::from_rgb(0, 120, 255))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_never_inverts() {
        let r = IRect::new(10, 10, 20, 20);
        assert_eq!(resized(r, 2, Pt::new(40.0, 35.0)), IRect::new(10, 10, 30, 25));
        // Dragging the bottom-right corner past the top-left leaves one pixel.
        assert_eq!(resized(r, 2, Pt::new(0.0, 0.0)), IRect::new(10, 10, 1, 1));
        assert_eq!(resized(r, 0, Pt::new(5.0, 12.0)), IRect::new(5, 12, 25, 18));
    }

    #[test]
    fn topmost_slice_wins() {
        let v = vec![Slice::new("a", IRect::new(0, 0, 10, 10)), Slice::new("b", IRect::new(5, 5, 10, 10))];
        assert_eq!(slice_at(&v, Pt::new(7.0, 7.0)), Some(1));
        assert_eq!(slice_at(&v, Pt::new(1.0, 1.0)), Some(0));
        assert_eq!(slice_at(&v, Pt::new(30.0, 1.0)), None);
    }
}
