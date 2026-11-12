//! Rulers along the canvas edges and the guides pulled off them.
//!
//! Guides are axis-aligned lines in document pixels stored on the
//! `Document`: they travel with the `.qsk` and sit outside undo, like
//! Photoshop's. A drag that starts on a ruler creates one; the Move tool (or
//! any tool with Ctrl held) drags an existing one; dropping a guide back on
//! a ruler, or anywhere off the canvas, removes it. Geometric tools snap to
//! guides through [`snap_point`] (see `tools::handle`).

use egui::{Align2, Color32, CursorIcon, FontId, Modifiers, Pos2, Rect, Stroke, Ui, Vec2};
use qsketch_core::{Guide, GuideDiag, Pt};

use super::view::CanvasView;
use crate::state::{AppState, DocId, GuideDrag};
use crate::tools::ToolKind;

/// Width of the ruler bands, in points.
pub const RULER: f32 = 18.0;
/// How close (screen px) the pointer must be to grab a guide.
const GRAB_PX: f32 = 5.0;
const GUIDE_COLOR: Color32 = Color32::from_rgb(0, 190, 255);

/// Carve the ruler bands off the allocated area: the canvas proper, and
/// `(top band, left band)` when rulers are shown.
pub fn layout(full: Rect, rulers: bool) -> (Rect, Option<(Rect, Rect)>) {
    if !rulers || full.width() < RULER * 3.0 || full.height() < RULER * 3.0 {
        return (full, None);
    }
    let canvas = Rect::from_min_max(full.min + Vec2::splat(RULER), full.max);
    let top = Rect::from_min_max(egui::pos2(canvas.left(), full.top()), egui::pos2(full.right(), canvas.top()));
    let left = Rect::from_min_max(egui::pos2(full.left(), canvas.top()), egui::pos2(canvas.left(), full.bottom()));
    (canvas, Some((top, left)))
}

/// Two screen points on the guide's line.
fn guide_points(view: &CanvasView, g: &Guide) -> (Pos2, Pos2) {
    let (p, (dx, dy)) = g.line();
    (view.doc_to_screen(p), view.doc_to_screen(Pt::new(p.x + dx * 1000.0, p.y + dy * 1000.0)))
}

/// The ruler corner square (where diagonal guides are pulled from).
fn corner_rect(top: Rect, left: Rect) -> Rect {
    Rect::from_min_max(egui::pos2(left.left(), top.top()), egui::pos2(top.left(), left.top()))
}

/// Point on the guide's line nearest `p` (document space).
fn project(g: &Guide, p: Pt) -> Pt {
    let (o, (dx, dy)) = g.line();
    let t = (p.x - o.x) * dx + (p.y - o.y) * dy;
    Pt::new(o.x + dx * t, o.y + dy * t)
}

/// Turn an angle (degrees) and a point into a guide: an axis guide when the
/// angle is (within a hair of) horizontal or vertical, a diagonal otherwise.
fn guide_from(angle: f32, at: Pt) -> (bool, f32, Option<GuideDiag>) {
    let a = angle.rem_euclid(180.0);
    if a < 0.05 || a > 179.95 {
        (false, at.y.round(), None)
    } else if (a - 90.0).abs() < 0.05 {
        (true, at.x.round(), None)
    } else {
        (false, 0.0, Some(GuideDiag { x: at.x.round(), y: at.y.round(), angle: a }))
    }
}

/// Perpendicular screen distance from `p` to the guide, through the view so
/// it holds under rotation and flips.
fn guide_distance(view: &CanvasView, g: &Guide, p: Pos2) -> f32 {
    let (a, b) = guide_points(view, g);
    let d = b - a;
    let n = egui::vec2(-d.y, d.x) / d.length().max(1e-3);
    (p - a).dot(n).abs()
}

/// The document coordinate a guide of this orientation takes at screen `p`:
/// whole pixels, or grid lines while snap to grid is on.
fn guide_pos_at(state: &AppState, view: &CanvasView, vertical: bool, p: Pos2) -> f32 {
    let d = view.screen_to_doc(p);
    let v = if vertical { d.x } else { d.y };
    let c = &state.settings.canvas;
    if c.snap_to_grid {
        let g = c.grid_size.max(1) as f32;
        (v / g).round() * g
    } else {
        v.round()
    }
}

fn drag_cursor(vertical: bool, diag: bool) -> CursorIcon {
    if diag {
        CursorIcon::Move
    } else if vertical {
        CursorIcon::ResizeHorizontal
    } else {
        CursorIcon::ResizeVertical
    }
}

/// Guides through the middle of the canvas, both ways (skips ones already there).
pub fn add_center_guides(state: &mut AppState, doc_id: DocId) {
    let Some(e) = state.doc_mut(doc_id) else { return };
    let (w, h) = (e.doc.state().width as f32, e.doc.state().height as f32);
    for g in [Guide::axis(true, (w / 2.0).round()), Guide::axis(false, (h / 2.0).round())] {
        if !e.doc.guides.contains(&g) {
            e.doc.guides.push(g);
        }
    }
}

/// Pointer handling for rulers and guides. Returns whether a guide drag owns
/// the pointer this frame (tools must not see the events) and the cursor to
/// show while the pointer is over a grabbable guide.
#[allow(clippy::too_many_arguments)]
pub fn handle(
    ui: &Ui,
    state: &mut AppState,
    doc_id: DocId,
    canvas: Rect,
    bands: Option<(Rect, Rect)>,
    events: &[egui::Event],
    mods: Modifiers,
    tool: ToolKind,
) -> (bool, Option<CursorIcon>) {
    let pointer = ui.input(|i| i.pointer.latest_pos());
    let any_down = ui.input(|i| i.pointer.primary_down());
    let locked = state.settings.canvas.lock_guides;

    // A drag in flight: follow the pointer, finish on release.
    if let Some(mut drag) = state.guide_drag.filter(|d| d.doc == doc_id) {
        let Some(entry) = state.doc(doc_id) else {
            state.guide_drag = None;
            return (false, None);
        };
        if let Some(p) = pointer {
            let d = entry.view.screen_to_doc(p);
            if mods.shift {
                // Shift: turn the guide around the point it was grabbed at
                // (snapped to the fine angle step; Shift+Alt for any angle).
                let cur = drag.guide();
                let pivot = *drag.pivot.get_or_insert_with(|| project(&cur, d));
                let (vx, vy) = (d.x - pivot.x, d.y - pivot.y);
                let mut angle = cur.angle();
                if (vx * vx + vy * vy).sqrt() * entry.view.zoom > 6.0 {
                    angle = vy.atan2(vx).to_degrees();
                    if !mods.alt {
                        let step = state.settings.canvas.fine_angle_step.clamp(1.0, 90.0);
                        angle = (angle / step).round() * step;
                    }
                }
                (drag.vertical, drag.pos, drag.diag) = guide_from(angle, pivot);
                if drag.diag.is_none() {
                    drag.pos = if drag.vertical { pivot.x.round() } else { pivot.y.round() };
                }
            } else if let Some(dg) = drag.diag {
                drag.pivot = None;
                drag.diag = Some(GuideDiag { x: d.x.round(), y: d.y.round(), ..dg });
            } else {
                drag.pivot = None;
                drag.pos = guide_pos_at(state, &entry.view, drag.vertical, p);
            }
        }
        let on_canvas = pointer.is_some_and(|p| canvas.contains(p));
        if any_down {
            if let Some(g) = drag.index.and_then(|i| state.doc_mut(doc_id).and_then(|e| e.doc.guides.get_mut(i))) {
                *g = drag.guide();
            }
            state.guide_drag = Some(drag);
        } else {
            if let Some(entry) = state.doc_mut(doc_id) {
                match drag.index {
                    // An existing guide dropped off the canvas is removed.
                    Some(i) if !on_canvas => {
                        if i < entry.doc.guides.len() {
                            entry.doc.guides.remove(i);
                        }
                    }
                    Some(i) => {
                        if let Some(g) = entry.doc.guides.get_mut(i) {
                            *g = drag.guide();
                        }
                    }
                    None if on_canvas => entry.doc.guides.push(drag.guide()),
                    None => {}
                }
            }
            state.guide_drag = None;
        }
        ui.ctx().request_repaint();
        return (true, Some(drag_cursor(drag.vertical, drag.diag.is_some())));
    }

    // Hover and presses.
    let (started, cursor) = {
        let Some(entry) = state.doc(doc_id) else { return (false, None) };
        let grab_ok =
            !locked && state.settings.canvas.show_guides && (tool == ToolKind::Move || mods.ctrl || mods.command);
        let hit = |p: Pos2| -> Option<usize> {
            if !grab_ok || !canvas.contains(p) {
                return None;
            }
            entry
                .doc
                .guides
                .iter()
                .enumerate()
                .map(|(i, g)| (i, guide_distance(&entry.view, g, p)))
                .filter(|(_, d)| *d <= GRAB_PX)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };
        let cursor = pointer.and_then(hit).map(|i| {
            let g = &entry.doc.guides[i];
            drag_cursor(g.vertical, g.diag.is_some())
        });
        let started = events.iter().find_map(|ev| {
            let egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } = ev else {
                return None;
            };
            let pos = *pos;
            if !locked {
                if let Some((top, left)) = bands {
                    // The corner square pulls out a diagonal (45°) guide;
                    // Shift-drag turns any guide.
                    if corner_rect(top, left).contains(pos) {
                        // Double-click the corner: a guide through the middle each way.
                        if ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary)) {
                            return Some(GuideDrag {
                                doc: doc_id,
                                vertical: true,
                                index: Some(usize::MAX),
                                pos: 0.0,
                                diag: None,
                                pivot: None,
                            });
                        }
                        let d = entry.view.screen_to_doc(pos);
                        let diag = Some(GuideDiag { x: d.x.round(), y: d.y.round(), angle: 45.0 });
                        return Some(GuideDrag {
                            doc: doc_id,
                            vertical: false,
                            index: None,
                            pos: 0.0,
                            diag,
                            pivot: None,
                        });
                    }
                    if top.contains(pos) {
                        let p = guide_pos_at(state, &entry.view, false, pos);
                        return Some(GuideDrag {
                            doc: doc_id,
                            vertical: false,
                            index: None,
                            pos: p,
                            diag: None,
                            pivot: None,
                        });
                    }
                    if left.contains(pos) {
                        let p = guide_pos_at(state, &entry.view, true, pos);
                        return Some(GuideDrag {
                            doc: doc_id,
                            vertical: true,
                            index: None,
                            pos: p,
                            diag: None,
                            pivot: None,
                        });
                    }
                }
            }
            hit(pos).map(|i| {
                let g = entry.doc.guides[i];
                GuideDrag { doc: doc_id, vertical: g.vertical, index: Some(i), pos: g.pos, diag: g.diag, pivot: None }
            })
        });
        (started, cursor)
    };
    if let Some(d) = started {
        if d.index == Some(usize::MAX) {
            // the corner double-click
            state.guide_drag = None;
            add_center_guides(state, doc_id);
            return (true, None);
        }
        state.guide_drag = Some(d);
        state.active_doc = Some(doc_id);
        state.focus_doc_request = Some(doc_id);
        return (true, Some(drag_cursor(d.vertical, d.diag.is_some())));
    }
    // Over the corner square: hint that it pulls a guide.
    let cursor = cursor.or_else(|| {
        let (top, left) = bands?;
        (!locked && pointer.is_some_and(|p| corner_rect(top, left).contains(p))).then_some(CursorIcon::Crosshair)
    });
    (false, cursor)
}

/// Snap a document point to the nearest guide on each axis within `tol_px`
/// screen pixels (`zoom` converts that to document pixels).
pub fn snap_point(guides: &[Guide], zoom: f32, p: Pt, tol_px: f32) -> Pt {
    let tol = tol_px / zoom.max(1e-6);
    let mut out = p;
    let (mut best_x, mut best_y) = (tol, tol);
    let mut snapped_x = None;
    let mut snapped_y = None;
    for g in guides.iter().filter(|g| g.diag.is_none()) {
        if g.vertical {
            let d = (g.pos - p.x).abs();
            if d <= best_x {
                best_x = d;
                out.x = g.pos;
                snapped_x = Some(g.pos);
            }
        } else {
            let d = (g.pos - p.y).abs();
            if d <= best_y {
                best_y = d;
                out.y = g.pos;
                snapped_y = Some(g.pos);
            }
        }
    }
    // Diagonal guides: the nearest one within reach pulls the point onto it,
    // keeping an axis snap by landing on the crossing.
    let mut best = tol;
    let mut diag_out = None;
    for g in guides.iter().filter(|g| g.diag.is_some()) {
        let (o, (dx, dy)) = g.line();
        let dist = ((p.x - o.x) * -dy + (p.y - o.y) * dx).abs();
        if dist > best {
            continue;
        }
        let q = match (snapped_x, snapped_y) {
            (Some(x), _) if dx.abs() > 0.05 => Pt::new(x, o.y + dy * (x - o.x) / dx),
            (_, Some(y)) if dy.abs() > 0.05 => Pt::new(o.x + dx * (y - o.y) / dy, y),
            _ => project(g, p),
        };
        if (q.x - p.x).hypot(q.y - p.y) <= tol * 1.5 {
            best = dist;
            diag_out = Some(q);
        }
    }
    diag_out.unwrap_or(out)
}

/// Major and minor tick spacing (document pixels) for a zoom: majors every
/// 1, 2 or 5 × 10ⁿ px so labels stay at least ~56 pt apart, minors as fine
/// as keeps them ~5 pt apart.
fn tick_steps(zoom: f32) -> (f32, f32) {
    let mut major = 1.0f32;
    'outer: for k in 0..9 {
        for m in [1.0f32, 2.0, 5.0] {
            let s = m * 10f32.powi(k);
            major = s;
            if s * zoom >= 56.0 {
                break 'outer;
            }
        }
    }
    let minor = [10.0f32, 5.0, 2.0].into_iter().map(|n| major / n).find(|m| m * zoom >= 5.0).unwrap_or(major);
    (major, minor)
}

/// Draw the guides (clipped to the canvas) and the rulers.
pub fn draw(ui: &Ui, state: &AppState, doc_id: DocId, canvas: Rect, bands: Option<(Rect, Rect)>) {
    let Some(entry) = state.doc(doc_id) else { return };
    let view = &entry.view;
    let c = &state.settings.canvas;
    let drag = state.guide_drag.filter(|d| d.doc == doc_id);
    let v = ui.visuals();
    let accent = v.selection.stroke.color;

    // --- guides ------------------------------------------------------------
    if c.show_guides || drag.is_some() {
        let painter = ui.painter().with_clip_rect(canvas);
        let draw_line = |g: &Guide, color: Color32, width: f32| {
            let (a, b) = guide_points(view, g);
            let d = (b - a).normalized();
            let l = 100_000.0;
            painter.line_segment([a - d * l, a + d * l], Stroke::new(width, color));
        };
        if c.show_guides {
            for (i, g) in entry.doc.guides.iter().enumerate() {
                if drag.is_some_and(|d| d.index == Some(i)) {
                    continue;
                }
                draw_line(g, GUIDE_COLOR, 1.0);
            }
        }
        if let Some(d) = drag {
            draw_line(&d.guide(), accent, 1.5);
            if let Some(pv) = d.pivot {
                painter.circle_stroke(view.doc_to_screen(pv), 4.0, Stroke::new(1.5, accent));
            }
            if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
                let label = match d.diag {
                    Some(g) => format!("{:.0}°  (Shift: turn)", 180.0 - g.angle),
                    None => format!("{} {}", if d.vertical { "x" } else { "y" }, d.pos),
                };
                let galley = painter.layout_no_wrap(label, FontId::proportional(11.0), Color32::WHITE);
                let rect = Rect::from_min_size(p + egui::vec2(14.0, 14.0), galley.size() + egui::vec2(8.0, 4.0));
                painter.rect_filled(rect, 3.0, Color32::from_black_alpha(190));
                painter.galley(rect.min + egui::vec2(4.0, 2.0), galley, Color32::WHITE);
            }
        }
    }

    // --- rulers ------------------------------------------------------------
    let Some((top, left)) = bands else { return };
    let painter = ui.painter();
    let fill = v.extreme_bg_color;
    let edge = v.widgets.noninteractive.bg_stroke.color;
    let tick = crate::ui::theme::dim_text(v);
    let text = v.text_color();
    let corner = corner_rect(top, left);
    for r in [top, left, corner] {
        painter.rect_filled(r, 0.0, fill);
    }
    // A little diagonal in the corner square: drag it out for a diagonal guide.
    {
        let r = corner.shrink(4.0);
        painter.line_segment([r.left_bottom(), r.right_top()], Stroke::new(1.0, tick));
    }
    painter.line_segment(
        [egui::pos2(top.left(), top.bottom()), egui::pos2(top.right(), top.bottom())],
        Stroke::new(1.0, edge),
    );
    painter.line_segment(
        [egui::pos2(left.right(), left.top()), egui::pos2(left.right(), left.bottom())],
        Stroke::new(1.0, edge),
    );
    let font = FontId::proportional(9.5);
    // Ticks only make sense with the document axes along the rulers.
    if view.rotation.abs() < 1e-4 {
        let (major, minor) = tick_steps(view.zoom);
        let is_major = |x: f32| ((x / major).round() * major - x).abs() < minor * 0.01;
        // Top ruler: document x across.
        {
            let p = painter.with_clip_rect(top);
            let x_at = |sx: f32| view.screen_to_doc(egui::pos2(sx, canvas.top())).x;
            let (mut d0, mut d1) = (x_at(top.left()), x_at(top.right()));
            if d0 > d1 {
                std::mem::swap(&mut d0, &mut d1);
            }
            let mut x = (d0 / minor).floor() * minor;
            let mut n = 0;
            while x <= d1 && n < 4000 {
                let sx = view.doc_to_screen(Pt::new(x, 0.0)).x;
                let h = if is_major(x) { top.height() } else { top.height() * 0.3 };
                p.line_segment(
                    [egui::pos2(sx, top.bottom() - h), egui::pos2(sx, top.bottom())],
                    Stroke::new(1.0, tick),
                );
                if is_major(x) {
                    p.text(
                        egui::pos2(sx + 2.0, top.top() + 1.0),
                        Align2::LEFT_TOP,
                        format!("{}", x.round() as i64),
                        font.clone(),
                        text,
                    );
                }
                x += minor;
                n += 1;
            }
        }
        // Left ruler: document y down, labels turned to read along the band.
        {
            let p = painter.with_clip_rect(left);
            let y_at = |sy: f32| view.screen_to_doc(egui::pos2(canvas.left(), sy)).y;
            let (mut d0, mut d1) = (y_at(left.top()), y_at(left.bottom()));
            if d0 > d1 {
                std::mem::swap(&mut d0, &mut d1);
            }
            let mut y = (d0 / minor).floor() * minor;
            let mut n = 0;
            while y <= d1 && n < 4000 {
                let sy = view.doc_to_screen(Pt::new(0.0, y)).y;
                let w = if is_major(y) { left.width() } else { left.width() * 0.3 };
                p.line_segment(
                    [egui::pos2(left.right() - w, sy), egui::pos2(left.right(), sy)],
                    Stroke::new(1.0, tick),
                );
                if is_major(y) {
                    let galley = p.layout_no_wrap(format!("{}", y.round() as i64), font.clone(), text);
                    let shape = egui::epaint::TextShape::new(egui::pos2(left.left() + 1.0, sy - 2.0), galley, text)
                        .with_angle(-std::f32::consts::FRAC_PI_2);
                    p.add(shape);
                }
                y += minor;
                n += 1;
            }
        }
    }
    // Pointer markers.
    if let Some(h) = ui.input(|i| i.pointer.hover_pos()).filter(|h| canvas.contains(*h)) {
        painter
            .with_clip_rect(top)
            .line_segment([egui::pos2(h.x, top.top()), egui::pos2(h.x, top.bottom())], Stroke::new(1.0, accent));
        painter
            .with_clip_rect(left)
            .line_segment([egui::pos2(left.left(), h.y), egui::pos2(left.right(), h.y)], Stroke::new(1.0, accent));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_the_nearest_guide_per_axis() {
        let guides = [Guide::axis(true, 100.0), Guide::axis(false, 40.0)];
        let p = snap_point(&guides, 1.0, Pt::new(103.0, 37.0), 8.0);
        assert_eq!((p.x, p.y), (100.0, 40.0));
        // Out of tolerance: untouched.
        let p = snap_point(&guides, 1.0, Pt::new(120.0, 60.0), 8.0);
        assert_eq!((p.x, p.y), (120.0, 60.0));
        // Tolerance is in screen pixels: at 4× zoom, 3 doc px is 12 screen px.
        let p = snap_point(&guides, 4.0, Pt::new(103.0, 40.0), 8.0);
        assert_eq!(p.x, 103.0);
    }

    #[test]
    fn snaps_onto_diagonal_guides() {
        let diag = Guide { vertical: false, pos: 0.0, diag: Some(GuideDiag { x: 0.0, y: 0.0, angle: 45.0 }) };
        // Near the y = x line: projected onto it.
        let p = snap_point(&[diag], 1.0, Pt::new(52.0, 48.0), 8.0);
        assert!((p.x - 50.0).abs() < 1e-3 && (p.y - 50.0).abs() < 1e-3, "{p:?}");
        // With a vertical guide too: lands on the crossing.
        let p = snap_point(&[diag, Guide::axis(true, 60.0)], 1.0, Pt::new(58.0, 62.0), 8.0);
        assert!((p.x - 60.0).abs() < 1e-3 && (p.y - 60.0).abs() < 1e-3, "{p:?}");
        // Far away: untouched.
        let p = snap_point(&[diag], 1.0, Pt::new(80.0, 20.0), 8.0);
        assert_eq!((p.x, p.y), (80.0, 20.0));
    }

    #[test]
    fn near_axis_angles_become_axis_guides() {
        assert_eq!(guide_from(180.0, Pt::new(3.0, 7.0)), (false, 7.0, None));
        assert_eq!(guide_from(90.0, Pt::new(3.0, 7.0)), (true, 3.0, None));
        assert!(guide_from(30.0, Pt::new(3.0, 7.0)).2.is_some());
    }

    #[test]
    fn diagonal_guides_survive_json_and_old_files_still_load() {
        let g = Guide { vertical: false, pos: 0.0, diag: Some(GuideDiag { x: 1.0, y: 2.0, angle: 30.0 }) };
        let s = serde_json::to_string(&g).unwrap();
        assert_eq!(serde_json::from_str::<Guide>(&s).unwrap(), g);
        let old: Guide = serde_json::from_str(r#"{"vertical":true,"pos":5.0}"#).unwrap();
        assert_eq!(old, Guide::axis(true, 5.0));
    }

    #[test]
    fn tick_steps_keep_labels_apart() {
        for zoom in [0.05f32, 0.25, 0.5, 1.0, 2.0, 8.0, 32.0, 64.0] {
            let (major, minor) = tick_steps(zoom);
            assert!(major * zoom >= 56.0 || major >= 5e8, "zoom {zoom}: major {major}");
            assert!(minor <= major && minor * zoom >= 5.0 || minor == major, "zoom {zoom}: minor {minor}");
        }
    }
}
