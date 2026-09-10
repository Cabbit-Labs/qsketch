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
            let Some(ToolSession::DragRect { start, mods, .. }) = state.session.take() else { return };
            state.session_doc = None;
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
        CanvasEvent::Release(_) => {
            let Some(ToolSession::Lasso { pts, mods }) = state.session.take() else { return };
            state.session_doc = None;
            let op = op_from_mods(state.tool_opts.selection_op, mods);
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
            apply_selection(state, doc_id, mask, op, "Lasso");
        }
        _ => {}
    }
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
