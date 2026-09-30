//! Tile tool: stamp the tile picked in the Tileset panel into the cells of a
//! tilemap layer (drag to stamp a run); Alt or right-click clears cells;
//! Ctrl+click picks the tile under the pointer.

use qsketch_core::tilemap::{self, FLIP_H, FLIP_V, INDEX};
use qsketch_core::Pt;

use super::{CanvasEvent, ToolSession};
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

/// The cell under a document point, if the active layer is a tilemap.
fn cell_at(state: &AppState, doc_id: DocId, p: Pt) -> Option<(u32, u32)> {
    let e = state.doc(doc_id)?;
    let s = e.doc.state();
    let tm = s.active_layer().props.tilemap.as_ref()?;
    let ts = s.tilesets.get(tm.tileset)?;
    if p.x < 0.0 || p.y < 0.0 {
        return None;
    }
    let (cx, cy) = ((p.x / ts.tile_w as f32) as u32, (p.y / ts.tile_h as f32) as u32);
    (cx < tm.cols && cy < tm.rows).then_some((cx, cy))
}

/// Put `v` into a cell of the active tilemap layer and redraw it.
fn stamp(state: &mut AppState, doc_id: DocId, cx: u32, cy: u32, v: u32) {
    let Some(e) = state.doc_mut(doc_id) else { return };
    let s = e.doc.state_mut();
    let li = s.active;
    let Some(mut tm) = s.layers[li].props.tilemap.clone() else { return };
    let k = (cy * tm.cols + cx) as usize;
    if tm.cells.get(k) == Some(&v) {
        return;
    }
    tm.cells[k] = v;
    let Some(ts) = s.tilesets.get(tm.tileset).cloned() else { return };
    let r = tilemap::render_cell(&mut s.layers[li].raster, &ts, cx, cy, v);
    s.layers[li].props.tilemap = Some(tm);
    e.doc.mark_dirty_rect(r);
}

fn value(state: &AppState) -> u32 {
    let o = &state.tool_opts;
    let mut v = o.tile_index & INDEX;
    if o.tile_flip_h {
        v |= FLIP_H;
    }
    if o.tile_flip_v {
        v |= FLIP_V;
    }
    v
}

pub fn handle(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) => {
            if state.session.is_some() {
                return;
            }
            let is_tilemap = state.doc(doc_id).is_some_and(|e| e.doc.state().active_layer().props.tilemap.is_some());
            if !is_tilemap {
                state.toasts.push(
                    Level::Info,
                    "The Tile tool stamps into a tilemap layer: Layer › New Tilemap Layer, or convert a layer.",
                );
                return;
            }
            if !state.doc(doc_id).is_some_and(|e| {
                let s = e.doc.state();
                s.layer_editable(s.active)
            }) {
                state.toasts.push(Level::Info, "The active layer is locked or hidden.");
                return;
            }
            let Some((cx, cy)) = cell_at(state, doc_id, inp.doc) else { return };
            // Ctrl+click: pick the tile under the pointer.
            if inp.mods.ctrl || inp.mods.command {
                if let Some(v) = state
                    .doc(doc_id)
                    .and_then(|e| e.doc.state().active_layer().props.tilemap.as_ref().map(|t| t.cell(cx, cy)))
                {
                    state.tool_opts.tile_index = v & INDEX;
                    state.tool_opts.tile_flip_h = v & FLIP_H != 0;
                    state.tool_opts.tile_flip_v = v & FLIP_V != 0;
                }
                return;
            }
            let erase = inp.mods.alt || inp.button == egui::PointerButton::Secondary;
            let v = if erase { 0 } else { value(state) };
            let tiles = state.doc(doc_id).and_then(|e| {
                let s = e.doc.state();
                s.active_layer().props.tilemap.as_ref().and_then(|t| s.tilesets.get(t.tileset)).map(|ts| ts.tiles.len())
            });
            if !erase && tiles.is_none_or(|n| (v & INDEX) as usize >= n) {
                state.toasts.push(Level::Info, "Pick a tile in the Tileset panel first (paint a cell to make one).");
                return;
            }
            stamp(state, doc_id, cx, cy, v);
            state.session = Some(ToolSession::TileStamp { erase });
            state.session_doc = Some(doc_id);
        }
        CanvasEvent::Drag(inp) => {
            let Some(ToolSession::TileStamp { erase }) = state.session else { return };
            if let Some((cx, cy)) = cell_at(state, doc_id, inp.doc) {
                let v = if erase { 0 } else { value(state) };
                stamp(state, doc_id, cx, cy, v);
            }
        }
        CanvasEvent::Release(_) => {
            let Some(ToolSession::TileStamp { .. }) = state.session.take() else { return };
            state.session_doc = None;
            if let Some(e) = state.doc_mut(doc_id) {
                if e.doc.state().active_layer().props.tilemap != e.doc.history.current().active_layer().props.tilemap {
                    e.doc.commit("Place Tiles");
                }
            }
        }
        _ => {}
    }
}

/// The active tilemap layer's cell grid, while it is the active layer.
pub fn draw_grid(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    let Some(entry) = state.doc(doc_id) else { return };
    let s = entry.doc.state();
    let Some(tm) = s.active_layer().props.tilemap.as_ref() else { return };
    let Some(ts) = s.tilesets.get(tm.tileset) else { return };
    let view = &entry.view;
    if (ts.tile_w.min(ts.tile_h) as f32) * view.zoom < 6.0 {
        return;
    }
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 170, 0, 90));
    let (w, h) = ((tm.cols * ts.tile_w) as f32, (tm.rows * ts.tile_h) as f32);
    for cx in 0..=tm.cols {
        let x = (cx * ts.tile_w) as f32;
        painter.line_segment([view.doc_to_screen(Pt::new(x, 0.0)), view.doc_to_screen(Pt::new(x, h))], stroke);
    }
    for cy in 0..=tm.rows {
        let y = (cy * ts.tile_h) as f32;
        painter.line_segment([view.doc_to_screen(Pt::new(0.0, y)), view.doc_to_screen(Pt::new(w, y))], stroke);
    }
    // The cell the Tile tool would stamp.
    if state.effective_tool() == super::ToolKind::Tile {
        if let Some(p) = state.hover_doc_pos {
            if let Some((cx, cy)) = cell_at(state, doc_id, p) {
                let r = tilemap::cell_rect(ts, cx, cy);
                let q = [
                    view.doc_to_screen(Pt::new(r.x as f32, r.y as f32)),
                    view.doc_to_screen(Pt::new(r.right() as f32, r.y as f32)),
                    view.doc_to_screen(Pt::new(r.right() as f32, r.bottom() as f32)),
                    view.doc_to_screen(Pt::new(r.x as f32, r.bottom() as f32)),
                ];
                painter.add(egui::Shape::closed_line(
                    q.to_vec(),
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 170, 0)),
                ));
            }
        }
    }
}
