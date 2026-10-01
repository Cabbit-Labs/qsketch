//! Shape tool: pixel-art vector shapes. On a shape layer, click to add a
//! point (after the selected one, or on an edge to split it), drag a point
//! to move it, Alt+click a point to remove it; Delete removes the selected
//! point. On any other layer the first click starts a new shape layer. The
//! layer's pixels are redrawn from the points at every change, with no
//! anti-aliasing, so the shape stays editable and crisp.

use egui::{Color32, Stroke};
use qsketch_core::vector::{self, ShapePath};
use qsketch_core::{IRect, LayerKind, Pt};

use super::{CanvasEvent, ToolSession};
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

/// Screen distance (points) within which a point or edge is grabbed.
const HANDLE: f32 = 7.0;

fn pixel(p: Pt) -> [i32; 2] {
    [p.x.floor() as i32, p.y.floor() as i32]
}

/// Redraw the shape layer `li` and mark what changed (old and new bounds).
fn redraw(state: &mut AppState, doc_id: DocId, li: usize, before: Option<IRect>) {
    let Some(e) = state.doc_mut(doc_id) else { return };
    let s = e.doc.state_mut();
    let after = s.layers.get(li).and_then(|l| l.props.shape.as_ref()).and_then(|sh| sh.bounds());
    vector::rerender(s, li);
    let rect = match (before, after) {
        (Some(a), Some(b)) => a.union(&b),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => return,
    };
    e.doc.mark_dirty_rect(rect.expand(1));
}

fn shape_bounds(state: &AppState, doc_id: DocId, li: usize) -> Option<IRect> {
    state.doc(doc_id)?.doc.state().layers.get(li)?.props.shape.as_ref()?.bounds()
}

/// A new shape layer above the active one, from the tool's options.
fn new_shape_layer(state: &mut AppState, doc_id: DocId) -> Option<usize> {
    let fill = state.tool_opts.shape_fill.then_some(state.fg);
    let stroke = state.tool_opts.shape_stroke.then_some(state.bg);
    let width = state.tool_opts.shape_stroke_width.max(1);
    let closed = state.tool_opts.shape_closed;
    let e = state.doc_mut(doc_id)?;
    let s = e.doc.state_mut();
    let name = s.unique_layer_name("Shape");
    let id = s.add_layer(name, None);
    let li = s.index_of(id)?;
    let l = &mut s.layers[li];
    l.props.kind = LayerKind::Shape;
    let mut shape = ShapePath::new(fill, stroke);
    shape.stroke_width = width;
    shape.closed = closed;
    l.props.shape = Some(shape);
    e.selected.clear();
    e.shape_sel = None;
    Some(li)
}

pub fn handle(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            if state.session.is_some() {
                return;
            }
            let Some(e) = state.doc(doc_id) else { return };
            let zoom = e.view.zoom.max(0.01);
            let s = e.doc.state();
            let mut li = s.active;
            let tol = HANDLE / zoom;
            let is_shape = s.layers[li].is_shape();
            if is_shape && !s.layer_editable(li) && !s.layers[li].props.visible {
                state.toasts.push(Level::Info, "The active layer is hidden.");
                return;
            }
            if is_shape && s.layers[li].props.locked {
                state.toasts.push(Level::Info, "The active layer is locked.");
                return;
            }
            let p = pixel(inp.doc);
            if !is_shape {
                let Some(new_li) = new_shape_layer(state, doc_id) else { return };
                li = new_li;
            }
            let before = shape_bounds(state, doc_id, li);
            let sel = state.doc(doc_id).and_then(|e| e.shape_sel);
            let Some(e) = state.doc_mut(doc_id) else { return };
            let s = e.doc.state_mut();
            let Some(shape) = s.layers[li].props.shape.as_mut() else { return };
            let hit = shape.point_near(inp.doc.x, inp.doc.y, tol);
            let point = match hit {
                Some(i) if inp.mods.alt => {
                    shape.points.remove(i);
                    e.shape_sel = (!shape.points.is_empty()).then_some(i.min(shape.points.len() - 1));
                    redraw(state, doc_id, li, before);
                    if let Some(e) = state.doc_mut(doc_id) {
                        e.doc.commit("Remove Point");
                    }
                    return;
                }
                Some(i) => i,
                None => {
                    let at = match shape.edge_near(inp.doc.x, inp.doc.y, tol) {
                        Some(edge) if !inp.mods.shift => edge + 1,
                        _ => sel.map_or(shape.points.len(), |s| (s + 1).min(shape.points.len())),
                    };
                    shape.points.insert(at, p);
                    at
                }
            };
            let orig = shape.points[point];
            e.shape_sel = Some(point);
            redraw(state, doc_id, li, before);
            state.session = Some(ToolSession::ShapeDrag { point, orig, grab: p });
            state.session_doc = Some(doc_id);
        }
        CanvasEvent::Drag(inp) => {
            let Some(ToolSession::ShapeDrag { point, orig, grab }) = state.session else { return };
            let Some(e) = state.doc(doc_id) else { return };
            let s = e.doc.state();
            let li = s.active;
            let Some(shape) = s.layers[li].props.shape.as_ref() else { return };
            let Some(&cur) = shape.points.get(point) else { return };
            // The point keeps its offset from the pixel it was grabbed on (a
            // fresh point was grabbed on itself, so it follows the pointer).
            let now = pixel(inp.doc);
            let target = [orig[0] + now[0] - grab[0], orig[1] + now[1] - grab[1]];
            if target == cur {
                return;
            }
            let before = shape.bounds();
            if let Some(e) = state.doc_mut(doc_id) {
                let s = e.doc.state_mut();
                if let Some(sh) = s.layers[li].props.shape.as_mut() {
                    if let Some(pt) = sh.points.get_mut(point) {
                        *pt = target;
                    }
                }
            }
            redraw(state, doc_id, li, before);
        }
        CanvasEvent::Release(_) => {
            let Some(ToolSession::ShapeDrag { .. }) = state.session.take() else { return };
            state.session_doc = None;
            if let Some(e) = state.doc_mut(doc_id) {
                let s = e.doc.state();
                let li = s.active;
                let changed = s.layers.get(li).map(|l| &l.props.shape)
                    != e.doc.history.current().layers.get(li).map(|l| &l.props.shape)
                    || s.layers.len() != e.doc.history.current().layers.len();
                if changed {
                    e.doc.commit("Edit Shape");
                }
            }
        }
        _ => {}
    }
}

/// Delete the selected point of the active shape layer (Delete key).
pub fn delete_selected(state: &mut AppState) -> bool {
    let Some(doc_id) = state.active_doc else { return false };
    let Some(e) = state.doc(doc_id) else { return false };
    let Some(point) = e.shape_sel else { return false };
    let s = e.doc.state();
    let li = s.active;
    if !s.layers[li].is_shape() {
        return false;
    }
    let before = shape_bounds(state, doc_id, li);
    let Some(e) = state.doc_mut(doc_id) else { return false };
    let s = e.doc.state_mut();
    let Some(shape) = s.layers[li].props.shape.as_mut() else { return false };
    if point >= shape.points.len() {
        return false;
    }
    shape.points.remove(point);
    e.shape_sel = (!shape.points.is_empty()).then_some(point.min(shape.points.len() - 1));
    redraw(state, doc_id, li, before);
    if let Some(e) = state.doc_mut(doc_id) {
        e.doc.commit("Remove Point");
    }
    true
}

/// Change the active shape layer's options (fill, outline, closed) and
/// redraw it as one undo step.
pub fn edit_shape(state: &mut AppState, doc_id: DocId, label: &str, f: impl FnOnce(&mut ShapePath)) {
    let Some(e) = state.doc(doc_id) else { return };
    let li = e.doc.state().active;
    if !e.doc.state().layers[li].is_shape() {
        return;
    }
    let before = shape_bounds(state, doc_id, li);
    if let Some(e) = state.doc_mut(doc_id) {
        if let Some(sh) = e.doc.state_mut().layers[li].props.shape.as_mut() {
            f(sh);
        }
    }
    redraw(state, doc_id, li, before);
    if let Some(e) = state.doc_mut(doc_id) {
        e.doc.commit(label);
    }
}

/// Points and edges of the active shape layer while the Shape tool is up.
pub fn draw_overlay(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    if state.effective_tool() != super::ToolKind::Shape {
        return;
    }
    let Some(entry) = state.doc(doc_id) else { return };
    let s = entry.doc.state();
    let l = s.active_layer();
    let Some(shape) = l.props.shape.as_ref().filter(|_| l.is_shape()) else { return };
    let view = &entry.view;
    let to_screen = |p: [i32; 2]| view.doc_to_screen(Pt::new(p[0] as f32 + 0.5, p[1] as f32 + 0.5));
    let n = shape.points.len();
    let edge = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 170, 0, 200));
    let edges = if shape.closed && n >= 2 { n } else { n.saturating_sub(1) };
    for i in 0..edges {
        painter.line_segment([to_screen(shape.points[i]), to_screen(shape.points[(i + 1) % n])], edge);
    }
    for (i, p) in shape.points.iter().enumerate() {
        let c = to_screen(*p);
        let selected = entry.shape_sel == Some(i);
        let r = egui::Rect::from_center_size(c, egui::vec2(7.0, 7.0));
        painter.rect_filled(r, 1.0, if selected { Color32::from_rgb(255, 170, 0) } else { Color32::WHITE });
        painter.rect_stroke(r, 1.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
    }
}
