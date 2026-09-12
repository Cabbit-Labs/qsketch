//! Zoom, Hand and Rotate View tools.

use super::{CanvasEvent, ToolSession};
use crate::state::{AppState, DocId};

pub fn handle_zoom(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) => {
            if state.session.is_none() {
                let z = state.doc(doc_id).map(|d| d.view.zoom).unwrap_or(1.0);
                state.session = Some(ToolSession::ZoomDrag { start: inp.screen, start_zoom: z, moved: false });
                state.session_doc = Some(doc_id);
            }
        }
        CanvasEvent::Drag(inp) => {
            let scrub = state.tool_opts.zoom_scrub;
            let Some(ToolSession::ZoomDrag { start, start_zoom, moved }) = &mut state.session else { return };
            let dx = inp.screen.x - start.x;
            if dx.abs() > 3.0 {
                *moved = true;
            }
            if *moved && scrub {
                let factor = (dx / 120.0).exp2();
                let target = *start_zoom * factor;
                let anchor = *start;
                if let Some(entry) = state.doc_mut(doc_id) {
                    entry.view.set_zoom(target, Some(anchor));
                }
            }
        }
        CanvasEvent::Release(inp) => {
            let Some(ToolSession::ZoomDrag { moved, .. }) = state.session.take() else { return };
            state.session_doc = None;
            if !moved {
                if let Some(entry) = state.doc_mut(doc_id) {
                    if inp.mods.alt || inp.button == egui::PointerButton::Secondary {
                        entry.view.zoom_out(Some(inp.screen));
                    } else {
                        entry.view.zoom_in(Some(inp.screen));
                    }
                }
            }
        }
        _ => {}
    }
}

pub fn handle_hand(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) => {
            if state.session.is_none() {
                // Grabbing the canvas stops any glide from the previous drag.
                if let Some(entry) = state.doc_mut(doc_id) {
                    entry.view.pan_velocity = egui::Vec2::ZERO;
                }
                state.session = Some(ToolSession::Pan {
                    last: inp.screen,
                    last_at: std::time::Instant::now(),
                    velocity: egui::Vec2::ZERO,
                });
                state.session_doc = Some(doc_id);
            }
        }
        CanvasEvent::Drag(inp) => {
            let Some(ToolSession::Pan { last, last_at, velocity }) = &mut state.session else { return };
            let now = std::time::Instant::now();
            let dt = now.duration_since(*last_at).as_secs_f32();
            let delta = inp.screen - *last;
            *last = inp.screen;
            *last_at = now;
            if dt > 0.0005 {
                let sample = delta / dt;
                // Weight recent samples more; a long gap (pointer at rest) resets.
                let a = (dt / 0.05).clamp(0.3, 1.0);
                *velocity = *velocity * (1.0 - a) + sample * a;
            }
            if let Some(entry) = state.doc_mut(doc_id) {
                entry.view.pan_by_screen(delta);
                let (w, h) = (entry.doc.width(), entry.doc.height());
                entry.view.clamp_to_document(w, h);
            }
        }
        CanvasEvent::Release(_) => {
            if let Some(ToolSession::Pan { last_at, velocity, .. }) = state.session {
                state.session = None;
                state.session_doc = None;
                // Only glide when the pointer was still moving at release; a
                // pause before letting go means "stop here". Slow drags don't
                // glide, fast flicks do.
                let fresh = last_at.elapsed().as_secs_f32() < 0.08;
                if state.settings.canvas.pan_inertia && fresh && velocity.length() > 300.0 {
                    if let Some(entry) = state.doc_mut(doc_id) {
                        entry.view.pan_velocity = velocity;
                    }
                }
            }
        }
        CanvasEvent::DoubleClick(_) => {
            if let Some(entry) = state.doc_mut(doc_id) {
                let (w, h) = (entry.doc.width(), entry.doc.height());
                entry.view.fit(w, h);
            }
        }
        _ => {}
    }
}

pub fn handle_rotate(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        // Quick rotate (Q held): the view follows the pointer without a click.
        CanvasEvent::Hover(inp)
            if state.settings.canvas.quick_rotate_follow_pointer
                && matches!(state.temp_tool, Some((_, crate::state::TempReason::QuickRotate)))
                && state.session.is_none() =>
        {
            handle_rotate(state, doc_id, CanvasEvent::Press(inp));
        }
        CanvasEvent::Press(inp) => {
            if state.session.is_none() {
                let Some(entry) = state.doc(doc_id) else { return };
                let c = entry.view.viewport.center();
                let a = (inp.screen.y - c.y).atan2(inp.screen.x - c.x);
                state.session = Some(ToolSession::RotateDrag { start_angle: a, start_rot: entry.view.rotation });
                state.session_doc = Some(doc_id);
            }
        }
        CanvasEvent::Drag(inp) => {
            let Some(ToolSession::RotateDrag { start_angle, start_rot }) = state.session else { return };
            let Some(entry) = state.doc_mut(doc_id) else { return };
            let c = entry.view.viewport.center();
            let a = (inp.screen.y - c.y).atan2(inp.screen.x - c.x);
            let mut rot = start_rot + (a - start_angle);
            if inp.mods.shift {
                let step = 15f32.to_radians();
                rot = (rot / step).round() * step;
            }
            entry.view.set_rotation(rot);
        }
        CanvasEvent::Release(_) => {
            if matches!(state.session, Some(ToolSession::RotateDrag { .. })) {
                state.session = None;
                state.session_doc = None;
            }
        }
        CanvasEvent::DoubleClick(_) => {
            if let Some(entry) = state.doc_mut(doc_id) {
                entry.view.reset_rotation();
            }
        }
        _ => {}
    }
}
