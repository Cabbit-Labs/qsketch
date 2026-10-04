//! Move tool (drag layer content / selected pixels) and the Crop tool.

use std::sync::Arc;

use qsketch_core::layer::LayerKind;
use qsketch_core::moving;
use qsketch_core::ops::{self, drop_floating};
use qsketch_core::{IRect, Mask, Pt};

use super::{rect_from_drag, CanvasEvent, MoveWhat, ToolSession};
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

pub fn handle_move(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            if state.session.is_some() {
                return;
            }
            let Some((what, last_rect, sel_before)) = begin_move(state, doc_id) else { return };
            state.session = Some(ToolSession::Moving { what, start: inp.doc, cur: inp.doc, sel_before, last_rect });
            state.session_doc = Some(doc_id);
        }
        CanvasEvent::Drag(inp) => {
            let Some(ToolSession::Moving { what, start, cur, last_rect, .. }) = &mut state.session else {
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
            let nr = apply_move(entry.doc.state_mut(), what, dx, dy);
            entry.doc.mark_dirty_rect(last_rect.union(&nr));
            entry.sel_outline = None;
            *last_rect = nr;
        }
        CanvasEvent::Release(_) => {
            let Some(ToolSession::Moving { what, start, cur, sel_before, .. }) = state.session.take() else { return };
            state.session_doc = None;
            let Some(entry) = state.docs.iter_mut().find(|d| d.id == doc_id) else { return };
            let moved = (cur.x - start.x).round() as i32 != 0 || (cur.y - start.y).round() as i32 != 0;
            if moved {
                finish_move(entry, &what);
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

/// Whether a move would carry just the active layer (no group, no other
/// layers selected), which the selection transform box can hold.
pub fn moves_one_layer(state: &AppState, doc_id: DocId) -> bool {
    state.doc(doc_id).is_some_and(|e| {
        let s = e.doc.state();
        e.selected_ids().len() == 1 && !s.layers[s.active].is_group()
    })
}

/// Pick up what a move drags: with a selection, its pixels on every
/// selected layer; without one, the selected layers whole (a group stands
/// for everything inside it, hidden layers included). Returns the cargo,
/// the canvas area it covers, and the selection to restore if nothing
/// moves. Says why in a toast when there is nothing to move.
fn begin_move(state: &mut AppState, doc_id: DocId) -> Option<(MoveWhat, IRect, Option<Arc<Mask>>)> {
    let entry = state.doc_mut(doc_id)?;
    let ids = entry.selected_ids();
    let s = entry.doc.state_mut();
    let t = moving::targets(s, &ids);
    let sel_before = s.selection.clone();
    let canvas = s.rect();
    let (what, rect, skipped_kind) = if sel_before.is_some() {
        // Shapes and text are drawn from their data, so selected pixels
        // only lift off plain pixel layers.
        let (raster, other): (Vec<usize>, Vec<usize>) =
            t.layers.iter().partition(|&&i| s.layers[i].props.kind == LayerKind::Raster);
        let mut lifted = Vec::new();
        let mut rect = IRect::EMPTY;
        for i in raster {
            if let Some(f) = ops::lift(s, i) {
                rect =
                    rect.union(&IRect::new(f.origin.0, f.origin.1, f.raster.width() as i32, f.raster.height() as i32));
                lifted.push((i, s.layers[i].raster.clone(), f));
            }
        }
        (MoveWhat::Pixels(lifted), rect, other.len())
    } else {
        let rest = moving::begin(s, &t.layers);
        let rect = moving::area(&rest, 0, 0, canvas);
        (MoveWhat::Layers(rest), rect, 0)
    };
    let empty = match &what {
        MoveWhat::Pixels(v) => v.is_empty(),
        MoveWhat::Layers(v) => v.is_empty(),
    };
    let mut skipped = Vec::new();
    if t.locked > 0 {
        skipped.push(plural(t.locked, "locked layer"));
    }
    if t.tilemaps > 0 {
        skipped.push(plural(t.tilemaps, "tilemap layer"));
    }
    if skipped_kind > 0 {
        skipped.push(plural(skipped_kind, "shape or text layer"));
    }
    if empty {
        let msg = if skipped.is_empty() {
            "Nothing to move on the selected layers.".to_string()
        } else {
            format!("Nothing to move: {} can't move.", skipped.join(", "))
        };
        state.toasts.push(Level::Info, msg);
        return None;
    }
    if !skipped.is_empty() {
        state.toasts.push(Level::Info, format!("{} stayed put.", capitalize(&skipped.join(", "))));
    }
    Some((what, rect, sel_before))
}

/// Place the move's cargo offset by `(dx, dy)`; returns the canvas area it
/// now covers.
fn apply_move(s: &mut qsketch_core::DocState, what: &MoveWhat, dx: i32, dy: i32) -> IRect {
    match what {
        MoveWhat::Layers(rest) => {
            moving::apply(s, rest, dx, dy);
            moving::area(rest, dx, dy, s.rect())
        }
        MoveWhat::Pixels(lifted) => {
            let mut nr = IRect::EMPTY;
            for (i, base, f) in lifted {
                s.layers[*i].raster = drop_floating(base, f, dx, dy);
                nr = nr.union(&IRect::new(
                    f.origin.0 + dx,
                    f.origin.1 + dy,
                    f.raster.width() as i32,
                    f.raster.height() as i32,
                ));
            }
            // Every layer was lifted through the same selection.
            if let Some((_, _, f)) = lifted.first() {
                let (w, h) = (s.width, s.height);
                s.selection =
                    f.mask.as_ref().map(|m| Arc::new(m.with_canvas_size(w, h, f.origin.0 + dx, f.origin.1 + dy)));
            }
            nr
        }
    }
}

/// Vector smart objects only had their drawn pixels shifted during the
/// drag; draw them again so parts that came onto the canvas show.
fn finish_move(entry: &mut crate::state::DocEntry, what: &MoveWhat) {
    if let MoveWhat::Layers(rest) = what {
        let idxs: Vec<usize> = rest.iter().map(|r| r.idx).collect();
        qsketch_core::smart::rerender_vectors(entry.doc.state_mut(), &idxs);
        entry.doc.mark_all_dirty();
    }
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Nudge the selected layers / selected pixels by whole pixels (arrow keys).
pub fn nudge(state: &mut AppState, doc_id: DocId, dx: i32, dy: i32) {
    state.settle();
    let Some((what, last_rect, _)) = begin_move(state, doc_id) else { return };
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let nr = apply_move(entry.doc.state_mut(), &what, dx, dy);
    entry.doc.mark_dirty_rect(last_rect.union(&nr));
    finish_move(entry, &what);
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
    state.settle();
    let Some(entry) = state.doc_mut(doc_id) else { return };
    ops::crop(entry.doc.state_mut(), r);
    entry.doc.resized();
    entry.doc.commit("Crop");
    entry.sel_outline = None;
    entry.needs_full_upload = true;
    entry.view.fit(r.w as u32, r.h as u32);
}
