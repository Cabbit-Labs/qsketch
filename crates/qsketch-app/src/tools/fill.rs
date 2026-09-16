//! Eyedropper, paint bucket and gradient tools.

use qsketch_core::ops;
use qsketch_core::Rgba8;

use super::{CanvasEvent, ToolSession};
use crate::settings::PickTarget;
use crate::state::{AppState, DocId};
use crate::tools::CanvasInput;
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

/// Start a color pick: the loupe opens under the pointer and the color
/// follows it until the button is released. Used by the Eyedropper tool and by
/// a pick chord (right-click by default) with any color-using tool.
pub fn begin_pick(state: &mut AppState, doc_id: DocId, inp: CanvasInput, target: PickTarget) {
    state.session = Some(ToolSession::Picking);
    state.session_doc = Some(doc_id);
    state.pick_preview = Some(crate::state::PickPreview {
        doc: doc_id,
        screen: inp.screen,
        doc_pos: inp.doc,
        target,
        color: match target {
            PickTarget::Foreground => state.fg,
            PickTarget::Background => state.bg,
        },
    });
    sample_into(state, doc_id, inp);
}

/// Take the color under the pointer, if there is one, into the pick target.
fn sample_into(state: &mut AppState, doc_id: DocId, inp: CanvasInput) {
    let Some(pick) = state.pick_preview else { return };
    if let Some(p) = state.pick_preview.as_mut() {
        p.screen = inp.screen;
        p.doc_pos = inp.doc;
    }
    let merged = state.tool_opts.eyedropper_sample_merged;
    let Some(mut c) = sample_color(state, doc_id, inp.doc.x.floor() as i32, inp.doc.y.floor() as i32, merged) else {
        return;
    };
    // Photoshop picks opaque colors; keep RGB, force full alpha unless fully
    // transparent, where there is nothing to pick.
    if c.a == 0 {
        return;
    }
    c.a = 255;
    match pick.target {
        PickTarget::Foreground => state.fg = c,
        PickTarget::Background => state.bg = c,
    }
    if let Some(p) = state.pick_preview.as_mut() {
        p.color = c;
    }
}

/// Drag / release of a pick in progress. Returns true when it consumed the
/// event, which it does for as long as the loupe is up.
pub fn handle_pick(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) -> bool {
    if state.pick_preview.is_none_or(|p| p.doc != doc_id) {
        return false;
    }
    match ev {
        CanvasEvent::Drag(inp) | CanvasEvent::Hover(inp) => sample_into(state, doc_id, inp),
        CanvasEvent::Release(inp) => {
            sample_into(state, doc_id, inp);
            end_pick(state);
        }
        CanvasEvent::Cancel => end_pick(state),
        _ => {}
    }
    true
}

pub fn end_pick(state: &mut AppState) {
    state.pick_preview = None;
    if matches!(state.session, Some(ToolSession::Picking)) {
        state.session = None;
        state.session_doc = None;
    }
}

pub fn handle_eyedropper(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    let CanvasEvent::Press(inp) = ev else {
        handle_pick(state, doc_id, ev);
        return;
    };
    // Which color this press picks: the chord's target for a temporary
    // eyedropper, otherwise Alt / right-click take the background.
    let target = if matches!(state.temp_tool, Some((_, crate::state::TempReason::Pick))) {
        match state.settings.mouse.pick_target(inp.mods, inp.button) {
            Some(t) => t,
            None => return,
        }
    } else if inp.mods.alt || inp.button == egui::PointerButton::Secondary {
        PickTarget::Background
    } else {
        PickTarget::Foreground
    };
    begin_pick(state, doc_id, inp, target);
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
    if !entry.doc.state().layer_editable(li) {
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
            if !entry.doc.state().layer_editable(li) {
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

/// The magnifier that follows a color pick: a circle of zoomed canvas pixels
/// with the sampled pixel boxed at its center, so a single pixel can be picked
/// without zooming the view.
pub fn draw_pick_loupe(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    let Some(pick) = state.pick_preview.filter(|p| p.doc == doc_id) else { return };
    let Some(entry) = state.doc(doc_id) else { return };
    let cfg = &state.settings.canvas;
    if !cfg.pick_loupe {
        return;
    }
    // Odd pixel count so there is a true center pixel.
    let cells = cfg.pick_loupe_pixels.clamp(3, 41) | 1;
    let radius = cfg.pick_loupe_size.clamp(40.0, 260.0) / 2.0;
    let cell = (radius * 2.0) / cells as f32;
    let half = cells as i32 / 2;
    let merged = state.tool_opts.eyedropper_sample_merged;
    let (px, py) = (pick.doc_pos.x.floor() as i32, pick.doc_pos.y.floor() as i32);

    // Sit above the pointer, flipping below when there is no room.
    let vp = entry.view.viewport;
    let gap = radius + 22.0;
    let mut c = egui::pos2(pick.screen.x, pick.screen.y - gap);
    if c.y - radius < vp.top() {
        c.y = pick.screen.y + gap;
    }
    c.x = c.x.clamp(vp.left() + radius + 2.0, (vp.right() - radius - 2.0).max(vp.left() + radius + 2.0));
    c.y = c.y.clamp(vp.top() + radius + 2.0, (vp.bottom() - radius - 2.0).max(vp.top() + radius + 2.0));

    let checker = [egui::Color32::from_gray(150), egui::Color32::from_gray(190)];
    for j in -half..=half {
        for i in -half..=half {
            // Round cell: skip anything whose center falls outside the circle.
            let p = egui::pos2(c.x + i as f32 * cell, c.y + j as f32 * cell);
            // Cells may overhang the rim; the ring drawn over them hides the
            // staircase, and culling further in would leave a ragged fringe.
            if p.distance(c) > radius {
                continue;
            }
            let col = match sample_color(state, doc_id, px + i, py + j, merged) {
                Some(s) if s.a == 255 => egui::Color32::from_rgb(s.r, s.g, s.b),
                Some(s) if s.a > 0 => {
                    let bg = checker[(((px + i) / 4 + (py + j) / 4) & 1) as usize];
                    let a = s.a as f32 / 255.0;
                    let mix = |f: u8, b: u8| (f as f32 * a + b as f32 * (1.0 - a)) as u8;
                    egui::Color32::from_rgb(mix(s.r, bg.r()), mix(s.g, bg.g()), mix(s.b, bg.b()))
                }
                // Outside the canvas, or fully transparent.
                Some(_) => checker[(((px + i) / 4 + (py + j) / 4) & 1) as usize],
                None => egui::Color32::from_gray(60),
            };
            painter.rect_filled(egui::Rect::from_center_size(p, egui::Vec2::splat(cell + 0.5)), 0.0, col);
        }
    }
    // The pixel being picked, and the rim.
    let center = egui::Rect::from_center_size(c, egui::Vec2::splat(cell));
    painter.rect_stroke(
        center.expand(1.0),
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(200)),
        egui::StrokeKind::Outside,
    );
    painter.rect_stroke(center, 0.0, egui::Stroke::new(1.0, egui::Color32::WHITE), egui::StrokeKind::Outside);
    let c8 = egui::Color32::from_rgb(pick.color.r, pick.color.g, pick.color.b);
    painter.circle_stroke(c, radius + cell * 0.6, egui::Stroke::new(cell * 1.4, c8));
    painter.circle_stroke(c, radius + cell * 1.3, egui::Stroke::new(1.5, egui::Color32::from_black_alpha(190)));
    painter.circle_stroke(c, radius, egui::Stroke::new(1.0, egui::Color32::from_black_alpha(90)));
    // Hex readout, and which color is being set.
    let label = format!(
        "{}  #{:02X}{:02X}{:02X}",
        if pick.target == crate::settings::PickTarget::Background { "BG" } else { "FG" },
        pick.color.r,
        pick.color.g,
        pick.color.b
    );
    let at = egui::pos2(c.x, c.y + radius + cell * 1.3 + 10.0);
    let galley = painter.layout_no_wrap(label, egui::FontId::proportional(12.0), egui::Color32::WHITE);
    let pill = egui::Rect::from_center_size(at, galley.size() + egui::vec2(10.0, 4.0));
    painter.rect_filled(pill, 4.0, egui::Color32::from_black_alpha(190));
    painter.galley(pill.center() - galley.size() / 2.0, galley, egui::Color32::WHITE);
}
