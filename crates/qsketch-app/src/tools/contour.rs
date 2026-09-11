//! Contour tool (Aseprite's filled freehand shape, as in Leyline's sketchpad):
//! trace a path, and on release it closes and fills with the foreground color
//! using the non-zero rule, so loops that cross themselves still fill.
//! Optionally the edge is stroked with the current brush. Fewer than three
//! points is a single brush dab.

use std::sync::Arc;

use egui::{Color32, Pos2, Stroke};
use qsketch_core::{Mask, Pt, StrokeSample};

use super::{CanvasEvent, ToolKind, ToolSession};
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

pub fn handle(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            if state.session.is_none() {
                state.session = Some(ToolSession::Contour { pts: vec![inp.doc] });
                state.session_doc = Some(doc_id);
            }
        }
        CanvasEvent::Drag(inp) => {
            if let Some(ToolSession::Contour { pts }) = &mut state.session {
                if pts.last().is_none_or(|p| p.dist(inp.doc) >= 0.5) {
                    pts.push(inp.doc);
                }
            }
        }
        CanvasEvent::Release(inp) => {
            let Some(ToolSession::Contour { mut pts }) = state.session.take() else { return };
            state.session_doc = None;
            if pts.last().is_none_or(|p| p.dist(inp.doc) >= 0.5) {
                pts.push(inp.doc);
            }
            commit(state, doc_id, pts);
        }
        _ => {}
    }
}

fn commit(state: &mut AppState, doc_id: DocId, pts: Vec<Pt>) {
    let outline = state.tool_opts.contour_outline;
    let fg = state.fg;
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let (w, h) = (entry.doc.width(), entry.doc.height());
    let s = entry.doc.state_mut();
    let li = s.active;
    if !s.layer_editable(li) {
        state.toasts.push(Level::Info, "The active layer is locked or hidden.");
        return;
    }
    let shape = if pts.len() >= 3 { Mask::from_polygon_rule(w, h, &pts, true) } else { Mask::new(w, h) };
    if !shape.is_empty() {
        let mask = match &s.selection {
            Some(sel) => shape.intersect(sel),
            None => shape,
        };
        let saved = s.selection.take();
        s.selection = Some(Arc::new(mask));
        let dirty = qsketch_core::ops::fill(s, li, fg);
        s.selection = saved;
        entry.doc.mark_dirty_rect(dirty);
        if !outline {
            entry.doc.commit("Contour");
            return;
        }
    }
    // Edge (or the dab for a click): a brush stroke along the closed path.
    let Some((engine, layer, extra)) = super::paint::begin_engine_with(state, doc_id, ToolKind::Brush) else {
        if let Some(entry) = state.doc_mut(doc_id) {
            entry.doc.revert_working();
        }
        return;
    };
    state.session = Some(ToolSession::Stroke { engine, layer, extra });
    state.session_doc = Some(doc_id);
    if pts.len() >= 3 {
        let mut path = pts.clone();
        path.push(pts[0]);
        for seg in path.windows(2) {
            super::paint::feed_line(state, doc_id, seg[0], seg[1], 1.0);
        }
    } else {
        let p = pts[0];
        super::paint::feed(state, doc_id, StrokeSample { pos: p, pressure: 1.0 });
    }
    super::paint::finish_with(state, doc_id, "Contour");
}

/// Live preview: the traced path plus a dashed closing edge.
pub fn draw_overlay(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    if state.session_doc != Some(doc_id) {
        return;
    }
    let Some(ToolSession::Contour { pts }) = &state.session else { return };
    let Some(entry) = state.doc(doc_id) else { return };
    let view = &entry.view;
    let sp: Vec<Pos2> = pts.iter().map(|p| view.doc_to_screen(*p)).collect();
    if sp.len() < 2 {
        return;
    }
    let accent = Color32::from_rgb(23, 227, 180);
    painter.add(egui::Shape::line(sp.clone(), Stroke::new(3.0, Color32::from_black_alpha(120))));
    painter.add(egui::Shape::line(sp.clone(), Stroke::new(1.0, accent)));
    if sp.len() >= 3 {
        let (a, b) = (sp[sp.len() - 1], sp[0]);
        painter.add(egui::Shape::dashed_line(&[a, b], Stroke::new(1.0, accent), 5.0, 4.0));
    }
}
