//! Marquee, lasso and magic wand selection tools.

use qsketch_core::mask::SelectionOp;
use qsketch_core::{Mask, Pt, Rgba8};

use super::{apply_selection, op_from_mods, rect_from_drag, CanvasEvent, ToolKind, ToolSession};
use crate::state::{AppState, DocId};

pub fn handle_marquee(state: &mut AppState, doc_id: DocId, tool: ToolKind, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            if state.session.is_none() {
                state.session = Some(ToolSession::DragRect { start: inp.doc, cur: inp.doc, mods: inp.mods });
                state.session_doc = Some(doc_id);
            }
        }
        CanvasEvent::Drag(inp) => {
            if let Some(ToolSession::DragRect { cur, mods, .. }) = &mut state.session {
                *cur = inp.doc;
                *mods = inp.mods;
            }
        }
        CanvasEvent::Release(inp) => {
            let Some(ToolSession::DragRect { start, .. }) = state.session.take() else { return };
            state.session_doc = None;
            // The modifiers held at release decide the combine mode.
            let mods = inp.mods;
            let cur = inp.doc;
            let Some(entry) = state.doc(doc_id) else { return };
            let (w, h) = (entry.doc.width(), entry.doc.height());
            let has_sel = entry.doc.state().selection.is_some();
            let op =
                if has_sel { op_from_mods(state.tool_opts.selection_op, mods) } else { state.tool_opts.selection_op };
            // Shift constrains to a square unless it means "add to selection".
            let square = mods.shift && (!has_sel || mods.alt);
            let r = rect_from_drag(start, cur, square);
            // A click (no drag) deselects, like Photoshop.
            if start.dist(cur) < 0.5 {
                if op == SelectionOp::Replace {
                    let entry = state.doc_mut(doc_id).unwrap();
                    if entry.doc.state().selection.is_some() {
                        entry.doc.state_mut().selection = None;
                        entry.doc.commit("Deselect");
                        entry.sel_outline = None;
                    }
                }
                return;
            }
            let mask = match tool {
                ToolKind::EllipseSelect => Mask::from_ellipse(w, h, r),
                _ => Mask::from_rect(w, h, r),
            };
            let mask = feather_new(state, mask);
            apply_selection(state, doc_id, mask, op, "Select");
        }
        _ => {}
    }
}

pub fn handle_lasso(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            if state.session.is_none() {
                state.session = Some(ToolSession::Lasso { pts: vec![inp.doc], mods: inp.mods });
                state.session_doc = Some(doc_id);
            }
        }
        CanvasEvent::Drag(inp) => {
            if let Some(ToolSession::Lasso { pts, mods }) = &mut state.session {
                if pts.last().is_none_or(|l| l.dist(inp.doc) >= 0.75) {
                    pts.push(inp.doc);
                }
                *mods = inp.mods;
            }
        }
        CanvasEvent::Release(inp) => {
            let Some(ToolSession::Lasso { pts, .. }) = state.session.take() else { return };
            state.session_doc = None;
            let op = op_from_mods(state.tool_opts.selection_op, inp.mods);
            let Some(entry) = state.doc(doc_id) else { return };
            let (w, h) = (entry.doc.width(), entry.doc.height());
            if pts.len() < 3 {
                if op == SelectionOp::Replace {
                    let entry = state.doc_mut(doc_id).unwrap();
                    if entry.doc.state().selection.is_some() {
                        entry.doc.state_mut().selection = None;
                        entry.doc.commit("Deselect");
                        entry.sel_outline = None;
                    }
                }
                return;
            }
            let mask = Mask::from_polygon(w, h, &pts);
            let mask = feather_new(state, mask);
            apply_selection(state, doc_id, mask, op, "Lasso");
        }
        _ => {}
    }
}

/// Screen-pixel radius around the first vertex where a click closes the polygon.
pub const POLY_CLOSE_PX: f32 = 7.0;

/// Polygonal lasso: each click places a vertex, the edge to the pointer
/// follows it. Click the first vertex, double-click, or press Enter to close;
/// Backspace removes the last vertex, Escape cancels. The modifiers held when
/// it closes decide add / subtract / intersect, as with the other selection
/// tools.
pub fn handle_poly_lasso(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            let zoom = state.doc(doc_id).map(|d| d.view.zoom).unwrap_or(1.0).max(0.01);
            let mut close = false;
            match &mut state.session {
                None => {
                    state.session = Some(ToolSession::PolyLasso { pts: vec![inp.doc], cur: inp.doc, mods: inp.mods });
                    state.session_doc = Some(doc_id);
                }
                Some(ToolSession::PolyLasso { pts, cur, mods }) => {
                    *cur = inp.doc;
                    *mods = inp.mods;
                    // Ctrl+click closes the loop from wherever the pointer is
                    // (the band already snaps to the first vertex while Ctrl
                    // is held), as does clicking near the first vertex.
                    if pts.len() >= 3 && (inp.mods.command || pts[0].dist(inp.doc) <= POLY_CLOSE_PX / zoom) {
                        close = true;
                    } else if pts.last().is_none_or(|l| l.dist(inp.doc) > 1.0 / zoom) {
                        pts.push(inp.doc);
                    }
                }
                _ => {}
            }
            if close {
                close_poly_lasso(state, doc_id, inp.mods);
            }
        }
        CanvasEvent::Drag(inp) | CanvasEvent::Hover(inp) => {
            if let Some(ToolSession::PolyLasso { pts, cur, mods }) = &mut state.session {
                *cur = if inp.mods.command && pts.len() >= 3 { pts[0] } else { inp.doc };
                *mods = inp.mods;
            }
        }
        CanvasEvent::DoubleClick(inp) => {
            if matches!(state.session, Some(ToolSession::PolyLasso { .. })) {
                close_poly_lasso(state, doc_id, inp.mods);
            }
        }
        _ => {}
    }
}

/// Drop the last placed vertex (Backspace); the polygon is cancelled when
/// none remain.
pub fn poly_lasso_undo_point(state: &mut AppState) {
    if let Some(ToolSession::PolyLasso { pts, .. }) = &mut state.session {
        pts.pop();
        if pts.is_empty() {
            state.cancel_session();
        }
    }
}

/// Close the polygon (Enter / click on the first vertex / double-click) and
/// apply it to the selection. Fewer than three vertices just cancels.
pub fn close_poly_lasso(state: &mut AppState, doc_id: DocId, mods: egui::Modifiers) {
    let Some(ToolSession::PolyLasso { pts, .. }) = state.session.take() else { return };
    state.session_doc = None;
    if pts.len() < 3 {
        return;
    }
    let op = op_from_mods(state.tool_opts.selection_op, mods);
    let Some(entry) = state.doc(doc_id) else { return };
    let (w, h) = (entry.doc.width(), entry.doc.height());
    let mask = feather_new(state, Mask::from_polygon(w, h, &pts));
    apply_selection(state, doc_id, mask, op, "Polygonal Lasso");
}

pub fn handle_wand(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    let CanvasEvent::Press(inp) = ev else { return };
    if inp.button != egui::PointerButton::Primary {
        return;
    }
    let op = op_from_mods(state.tool_opts.selection_op, inp.mods);
    let tol = state.tool_opts.wand_tolerance;
    let contiguous = state.tool_opts.wand_contiguous;
    let merged = state.tool_opts.wand_sample_merged;
    let Some(entry) = state.doc(doc_id) else { return };
    let (w, h) = (entry.doc.width(), entry.doc.height());
    let seed = (inp.doc.x.floor() as i32, inp.doc.y.floor() as i32);
    let mask = if merged {
        let comp = &entry.doc.composite;
        let sample = |x: i32, y: i32| Rgba8::from_array(comp.get_premul(x, y));
        Mask::from_flood(w, h, seed, tol, contiguous, &sample)
    } else {
        let raster = &entry.doc.state().active_layer().raster;
        let sample = |x: i32, y: i32| raster.get_pixel(x, y);
        Mask::from_flood(w, h, seed, tol, contiguous, &sample)
    };
    if mask.is_empty() {
        return;
    }
    apply_selection(state, doc_id, mask, op, "Magic Wand");
}

/// The options-bar Feather radius, applied to a freshly drawn selection.
fn feather_new(state: &AppState, mask: Mask) -> Mask {
    let r = state.tool_opts.selection_feather;
    if r > 0.0 {
        mask.feathered(r)
    } else {
        mask
    }
}

/// Select ▸ Feather…: soften the current selection's edge in place.
pub fn feather_selection(state: &mut AppState, doc_id: DocId, radius: f32) {
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let Some(sel) = entry.doc.state().selection.clone() else { return };
    let m = sel.feathered(radius);
    entry.doc.state_mut().selection = if m.is_empty() { None } else { Some(std::sync::Arc::new(m)) };
    entry.doc.commit("Feather");
    entry.sel_outline = None;
}

/// Select all / deselect / invert helpers used by actions.
pub fn select_all(state: &mut AppState, doc_id: DocId) {
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let (w, h) = (entry.doc.width(), entry.doc.height());
    entry.doc.state_mut().selection = Some(std::sync::Arc::new(Mask::full(w, h)));
    entry.doc.commit("Select All");
    entry.sel_outline = None;
}

pub fn deselect(state: &mut AppState, doc_id: DocId) {
    let Some(entry) = state.doc_mut(doc_id) else { return };
    if entry.doc.state().selection.is_some() {
        entry.doc.state_mut().selection = None;
        entry.doc.commit("Deselect");
        entry.sel_outline = None;
    }
}

pub fn invert(state: &mut AppState, doc_id: DocId) {
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let (w, h) = (entry.doc.width(), entry.doc.height());
    let s = entry.doc.state_mut();
    let inv = match &s.selection {
        Some(m) => m.invert(),
        None => Mask::full(w, h),
    };
    s.selection = if inv.is_empty() { None } else { Some(std::sync::Arc::new(inv)) };
    entry.doc.commit("Inverse");
    entry.sel_outline = None;
}

pub fn select_layer_content(state: &mut AppState, doc_id: DocId) {
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let (w, h) = (entry.doc.width(), entry.doc.height());
    let s = entry.doc.state_mut();
    let raster = &s.active_layer().raster;
    let mut m = Mask::new(w, h);
    if let Some(b) = raster.bounds() {
        for y in b.y..b.bottom() {
            for x in b.x..b.right() {
                m.set(x, y, raster.get_pixel(x, y).a);
            }
        }
        m.recompute_bounds();
    }
    s.selection = if m.is_empty() { None } else { Some(std::sync::Arc::new(m)) };
    entry.doc.commit("Select Layer Content");
    entry.sel_outline = None;
}

#[allow(dead_code)]
pub fn point_in_doc(p: Pt, w: u32, h: u32) -> bool {
    p.x >= 0.0 && p.y >= 0.0 && p.x < w as f32 && p.y < h as f32
}
