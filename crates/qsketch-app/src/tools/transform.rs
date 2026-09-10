//! Move tool (drag layer content / selected pixels) and the Crop tool.

use qsketch_core::ops::{self, drop_floating};
use qsketch_core::{IRect, Pt};

use super::{rect_from_drag, CanvasEvent, ToolSession};
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

pub fn handle_move(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            if state.session.is_some() {
                return;
            }
            let Some(entry) = state.doc_mut(doc_id) else { return };
            let s = entry.doc.state_mut();
            let li = s.active;
            if !s.layers[li].editable() {
                state.toasts.push(Level::Info, "The active layer is locked or hidden.");
                return;
            }
            let sel_before = s.selection.clone();
            let Some(floating) = ops::lift(s, li) else { return };
            let base = s.layers[li].raster.clone();
            let fr = IRect::new(
                floating.origin.0,
                floating.origin.1,
                floating.raster.width() as i32,
                floating.raster.height() as i32,
            );
            state.session = Some(ToolSession::Moving {
                base,
                floating,
                start: inp.doc,
                cur: inp.doc,
                layer: li,
                sel_before,
                last_rect: fr,
            });
            state.session_doc = Some(doc_id);
        }
        CanvasEvent::Drag(inp) => {
            let Some(ToolSession::Moving { base, floating, start, cur, layer, last_rect, .. }) = &mut state.session
            else {
                return;
            };
            let mut d = Pt::new(inp.doc.x - start.x, inp.doc.y - start.y);
            if inp.mods.shift {
                if d.x.abs() > d.y.abs() {
                    d.y = 0.0;
                } else {
                    d.x = 0.0;
                }
            }
            let (dx, dy) = (d.x.round() as i32, d.y.round() as i32);
            let prev = Pt::new(cur.x - start.x, cur.y - start.y);
            if prev.x.round() as i32 == dx && prev.y.round() as i32 == dy {
                return;
            }
            *cur = Pt::new(start.x + dx as f32, start.y + dy as f32);
            let Some(entry) = state.docs.iter_mut().find(|d| d.id == doc_id) else { return };
            let new_raster = drop_floating(base, floating, dx, dy);
            let nr = IRect::new(
                floating.origin.0 + dx,
                floating.origin.1 + dy,
                floating.raster.width() as i32,
                floating.raster.height() as i32,
            );
            let s = entry.doc.state_mut();
            s.layers[*layer].raster = new_raster;
            s.selection = floating.mask.as_ref().map(|m| {
                std::sync::Arc::new(m.with_canvas_size(
                    s.width,
                    s.height,
                    floating.origin.0 + dx,
                    floating.origin.1 + dy,
                ))
            });
            entry.doc.mark_dirty_rect(last_rect.union(&nr));
            entry.sel_outline = None;
            *last_rect = nr;
        }
        CanvasEvent::Release(_) => {
            let Some(ToolSession::Moving { start, cur, sel_before, .. }) = state.session.take() else { return };
            state.session_doc = None;
            let Some(entry) = state.docs.iter_mut().find(|d| d.id == doc_id) else { return };
            let moved = (cur.x - start.x).round() as i32 != 0 || (cur.y - start.y).round() as i32 != 0;
            if moved {
                entry.doc.commit("Move");
            } else {
                entry.doc.state_mut().selection = sel_before;
                entry.doc.revert_working();
            }
            entry.sel_outline = None;
        }
        _ => {}
    }
}

/// Nudge the layer / selection by whole pixels (arrow keys).
pub fn nudge(state: &mut AppState, doc_id: DocId, dx: i32, dy: i32) {
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let s = entry.doc.state_mut();
    let li = s.active;
    if !s.layers[li].editable() {
        return;
    }
    let Some(floating) = ops::lift(s, li) else { return };
    let base = s.layers[li].raster.clone();
    s.layers[li].raster = drop_floating(&base, &floating, dx, dy);
    if let Some(m) = &floating.mask {
        s.selection = Some(std::sync::Arc::new(m.with_canvas_size(
            s.width,
            s.height,
            floating.origin.0 + dx,
            floating.origin.1 + dy,
        )));
    }
    let r = IRect::new(
        floating.origin.0,
        floating.origin.1,
        floating.raster.width() as i32,
        floating.raster.height() as i32,
    );
    entry.doc.mark_dirty_rect(r.union(&r.translate(dx, dy)));
    entry.doc.commit("Nudge");
    entry.sel_outline = None;
}

pub fn handle_crop(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            if state.session.is_none() {
                state.session = Some(ToolSession::CropDrag { start: inp.doc, cur: inp.doc });
                state.session_doc = Some(doc_id);
            }
        }
        CanvasEvent::Drag(inp) => {
            if let Some(ToolSession::CropDrag { cur, .. }) = &mut state.session {
                *cur = inp.doc;
            }
        }
        CanvasEvent::Release(inp) => {
            let Some(ToolSession::CropDrag { start, .. }) = state.session.take() else { return };
            state.session_doc = None;
            let Some(entry) = state.doc(doc_id) else { return };
            let full = entry.doc.state().rect();
            let r = rect_from_drag(start, inp.doc, inp.mods.shift).intersect(&full);
            if r.w >= 1 && r.h >= 1 && start.dist(inp.doc) > 1.0 {
                state.tool_opts.crop_rect = Some(r);
            }
        }
        CanvasEvent::DoubleClick(_) => commit_crop(state, doc_id),
        _ => {}
    }
}

/// Apply the pending crop rect (Enter / double-click).
pub fn commit_crop(state: &mut AppState, doc_id: DocId) {
    let Some(r) = state.tool_opts.crop_rect.take() else { return };
    let Some(entry) = state.doc_mut(doc_id) else { return };
    ops::crop(entry.doc.state_mut(), r);
    entry.doc.resized();
    entry.doc.commit("Crop");
    entry.sel_outline = None;
    entry.needs_full_upload = true;
    entry.view.fit(r.w as u32, r.h as u32);
}
