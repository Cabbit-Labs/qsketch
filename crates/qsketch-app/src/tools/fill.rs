//! Eyedropper, paint bucket and gradient tools.

use qsketch_core::ops;
use qsketch_core::Rgba8;

use super::{CanvasEvent, ToolSession};
use crate::settings::PickTarget;
use crate::tools::CanvasInput;
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

/// Sample a color at a document pixel (merged or active layer).
pub fn sample_color(state: &AppState, doc_id: DocId, x: i32, y: i32, merged: bool) -> Option<Rgba8> {
    let entry = state.doc(doc_id)?;
    if x < 0 || y < 0 || x >= entry.doc.width() as i32 || y >= entry.doc.height() as i32 {
        return None;
    }
    if merged {
        let p = entry.doc.composite.get_premul(x, y);
        if p[3] == 0 {
            return Some(Rgba8::TRANSPARENT);
        }
        let a = p[3] as u32;
        let un = |v: u8| ((v as u32 * 255 + a / 2) / a).min(255) as u8;
        Some(Rgba8::new(un(p[0]), un(p[1]), un(p[2]), p[3]))
    } else {
        Some(entry.doc.state().active_layer().raster.get_pixel(x, y))
    }
}

/// One-shot pick for a modifier-less chord (e.g. a bare right-click bound to
/// pick), which can't put the temporary eyedropper up ahead of the press.
pub fn pick_once(state: &mut AppState, doc_id: DocId, inp: CanvasInput, target: PickTarget) {
    let merged = state.tool_opts.eyedropper_sample_merged;
    let Some(mut c) = sample_color(state, doc_id, inp.doc.x.floor() as i32, inp.doc.y.floor() as i32, merged) else {
        return;
    };
    if c.a == 0 {
        return;
    }
    c.a = 255;
    match target {
        PickTarget::Foreground => state.fg = c,
        PickTarget::Background => state.bg = c,
    }
}

pub fn handle_eyedropper(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    let inp = match ev {
        CanvasEvent::Press(i) => {
            state.session = Some(ToolSession::Picking);
            state.session_doc = Some(doc_id);
            i
        }
        CanvasEvent::Drag(i) if matches!(state.session, Some(ToolSession::Picking)) => i,
        CanvasEvent::Release(i) => {
            if matches!(state.session, Some(ToolSession::Picking)) {
                state.session = None;
                state.session_doc = None;
            }
            i
        }
        _ => return,
    };
    let merged = state.tool_opts.eyedropper_sample_merged;
    let Some(mut c) = sample_color(state, doc_id, inp.doc.x.floor() as i32, inp.doc.y.floor() as i32, merged) else {
        return;
    };
    // Photoshop picks opaque colors; keep RGB, force full alpha unless fully transparent.
    if c.a != 0 {
        c.a = 255;
    } else {
        return;
    }
    let secondary = inp.button == egui::PointerButton::Secondary;
    if matches!(state.temp_tool, Some((_, crate::state::TempReason::Pick))) {
        // Temporary eyedropper: the configured chord decides which color.
        match state.settings.mouse.pick_target(inp.mods, inp.button) {
            Some(PickTarget::Foreground) => state.fg = c,
            Some(PickTarget::Background) => state.bg = c,
            None => {}
        }
    } else if inp.mods.alt || secondary {
        state.bg = c;
    } else {
        state.fg = c;
    }
}

pub fn handle_bucket(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    let CanvasEvent::Press(inp) = ev else { return };
    if inp.button != egui::PointerButton::Primary {
        return;
    }
    let color = if inp.mods.alt { state.bg } else { state.fg };
    let tol = state.tool_opts.fill_tolerance;
    let contiguous = state.tool_opts.fill_contiguous;
    let merged = state.tool_opts.fill_sample_merged;
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let li = entry.doc.state().active;
    if !entry.doc.state().layers[li].editable() {
        state.toasts.push(Level::Info, "The active layer is locked or hidden.");
        return;
    }
    let seed = (inp.doc.x.floor() as i32, inp.doc.y.floor() as i32);
    let dirty = if merged {
        // Borrow juggling: composite is separate from the working state.
        let comp = std::mem::replace(&mut entry.doc.composite, qsketch_core::Composite::new(1, 1));
        let d = ops::bucket_fill(entry.doc.state_mut(), li, seed, color, tol, contiguous, Some(&comp));
        entry.doc.composite = comp;
        d
    } else {
        ops::bucket_fill(entry.doc.state_mut(), li, seed, color, tol, contiguous, None)
    };
    if dirty.is_empty() {
        return;
    }
    entry.doc.mark_dirty_rect(dirty);
    entry.doc.commit("Paint Bucket");
}

pub fn handle_gradient(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            if state.session.is_none() {
                state.session = Some(ToolSession::GradientDrag { start: inp.doc, cur: inp.doc });
                state.session_doc = Some(doc_id);
            }
        }
        CanvasEvent::Drag(inp) => {
            if let Some(ToolSession::GradientDrag { cur, start }) = &mut state.session {
                *cur = super::paint::constrain_line(*start, inp.doc, inp.mods.shift);
            }
        }
        CanvasEvent::Release(inp) => {
            let Some(ToolSession::GradientDrag { start, .. }) = state.session.take() else { return };
            state.session_doc = None;
            let end = super::paint::constrain_line(start, inp.doc, inp.mods.shift);
            if start.dist(end) < 1.0 {
                return;
            }
            let (fg, bg) = (state.fg, state.bg);
            let kind = state.tool_opts.gradient_kind;
            let Some(entry) = state.doc_mut(doc_id) else { return };
            let li = entry.doc.state().active;
            if !entry.doc.state().layers[li].editable() {
                state.toasts.push(Level::Info, "The active layer is locked or hidden.");
                return;
            }
            let dirty = ops::gradient_fill(entry.doc.state_mut(), li, start, end, fg, bg, kind);
            entry.doc.mark_dirty_rect(dirty);
            entry.doc.commit("Gradient");
        }
        _ => {}
    }
}
