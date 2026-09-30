//! Tilemap layers: a grid of cells that each show one tile of a shared
//! tileset, Aseprite-style. Painting a cell edits its tile, and every other
//! cell that uses the same tile (in any tilemap layer on that tileset)
//! follows.
//!
//! The layer's raster stays the source of truth for display, compositing
//! and painting: brushes paint it as usual, and on each commit
//! [`sync`] reads the edited cells back into their tiles and re-renders the
//! other instances. The cells themselves live in the layer's props (so they
//! are undoable and saved with the layer), the tiles in
//! `DocState::tilesets`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::document::DocState;
use crate::geom::IRect;
use crate::raster::{Raster, TILE};

/// Cell value bits: the tile index is the rest.
pub const FLIP_H: u32 = crate::ops::TILE_FLIP_H;
pub const FLIP_V: u32 = crate::ops::TILE_FLIP_V;
pub const INDEX: u32 = !(FLIP_H | FLIP_V);

/// A set of equally sized tiles. Tile 0 is always the empty tile.
#[derive(Clone)]
pub struct Tileset {
    pub name: String,
    pub tile_w: u32,
    pub tile_h: u32,
    pub tiles: Vec<Raster>,
}

impl Tileset {
    pub fn new(name: impl Into<String>, tile_w: u32, tile_h: u32) -> Self {
        let (tw, th) = (tile_w.max(1), tile_h.max(1));
        Self { name: name.into(), tile_w: tw, tile_h: th, tiles: vec![Raster::new(tw, th)] }
    }

    /// Index of a tile with exactly these pixels, adding it when new.
    pub fn intern(&mut self, px: Raster) -> u32 {
        let bytes = px.to_rgba();
        if let Some(i) = self.tiles.iter().position(|t| t.to_rgba() == bytes) {
            return i as u32;
        }
        self.tiles.push(px);
        (self.tiles.len() - 1) as u32
    }
}

/// The cells of a tilemap layer (in its props).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tilemap {
    /// Index into `DocState::tilesets`.
    pub tileset: usize,
    pub cols: u32,
    pub rows: u32,
    /// Row-major cell values: tile index plus `FLIP_H` / `FLIP_V`.
    pub cells: Vec<u32>,
}

impl Tilemap {
    pub fn cell(&self, cx: u32, cy: u32) -> u32 {
        self.cells.get((cy * self.cols + cx) as usize).copied().unwrap_or(0)
    }
}

fn flipped(r: &Raster, v: u32) -> Raster {
    let mut r = r.clone();
    if v & FLIP_H != 0 {
        r = r.flipped_h();
    }
    if v & FLIP_V != 0 {
        r = r.flipped_v();
    }
    r
}

/// The cell's rect on the canvas.
pub fn cell_rect(ts: &Tileset, cx: u32, cy: u32) -> IRect {
    IRect::new((cx * ts.tile_w) as i32, (cy * ts.tile_h) as i32, ts.tile_w as i32, ts.tile_h as i32)
}

/// The pixels of one cell (transparent beyond the canvas).
fn cell_pixels(raster: &Raster, ts: &Tileset, cx: u32, cy: u32) -> Raster {
    let r = cell_rect(ts, cx, cy);
    let mut out = Raster::new(ts.tile_w, ts.tile_h);
    for y in 0..r.h {
        for x in 0..r.w {
            let p = raster.get_pixel(r.x + x, r.y + y);
            if p.a > 0 {
                out.set_pixel(x, y, p);
            }
        }
    }
    out
}

/// Draw cell value `v` of `ts` into `raster` at `(cx, cy)`, replacing
/// what was there. Returns the canvas rect written.
pub fn render_cell(raster: &mut Raster, ts: &Tileset, cx: u32, cy: u32, v: u32) -> IRect {
    let r = cell_rect(ts, cx, cy);
    let tile = ts.tiles.get((v & INDEX) as usize).map(|t| flipped(t, v));
    for y in 0..r.h {
        for x in 0..r.w {
            let (px, py) = (r.x + x, r.y + y);
            if px >= raster.width() as i32 || py >= raster.height() as i32 {
                continue;
            }
            let c = tile.as_ref().map(|t| t.get_pixel(x, y)).unwrap_or(crate::color::Rgba8::TRANSPARENT);
            if c.a == 0 && raster.get_pixel(px, py).a == 0 {
                continue;
            }
            raster.set_pixel(px, py, c);
        }
    }
    r
}

/// Cut `raster` into `tile_w × tile_h` cells and build a tileset of its
/// distinct tiles (tile 0 empty) and the cells that reproduce it.
pub fn from_raster(raster: &Raster, name: &str, tile_w: u32, tile_h: u32) -> (Tileset, Tilemap) {
    let mut ts = Tileset::new(name, tile_w, tile_h);
    let cols = raster.width().div_ceil(ts.tile_w);
    let rows = raster.height().div_ceil(ts.tile_h);
    let mut index: HashMap<Vec<u8>, u32> = HashMap::new();
    index.insert(ts.tiles[0].to_rgba(), 0);
    let mut cells = Vec::with_capacity((cols * rows) as usize);
    for cy in 0..rows {
        for cx in 0..cols {
            let px = cell_pixels(raster, &ts, cx, cy);
            let key = px.to_rgba();
            let id = match index.get(&key) {
                Some(&i) => i,
                None => {
                    ts.tiles.push(px);
                    let i = (ts.tiles.len() - 1) as u32;
                    index.insert(key, i);
                    i
                }
            };
            cells.push(id);
        }
    }
    (ts, Tilemap { tileset: 0, cols, rows, cells })
}

/// Read the cells painted since `before` back into their tiles and redraw
/// every other cell that shows a changed tile. A painted empty cell gets a
/// tile of its own (or an identical existing one); a cell painted empty
/// becomes empty. Returns the canvas rects redrawn outside the painted cells.
pub fn sync(doc: &mut DocState, before: &DocState) -> Vec<IRect> {
    let mut redrawn = Vec::new();
    for li in 0..doc.layers.len() {
        let Some(mut tm) = doc.layers[li].props.tilemap.clone() else { continue };
        let id = doc.layers[li].props.id;
        let Some(prev) = before.layers.iter().find(|l| l.props.id == id) else { continue };
        let (raster, prev_raster) = (&doc.layers[li].raster, &prev.raster);
        if raster.width() != prev_raster.width() || raster.height() != prev_raster.height() {
            continue;
        }
        let Some(ts) = doc.tilesets.get(tm.tileset) else { continue };
        let (tw, th) = (ts.tile_w as i32, ts.tile_h as i32);
        // Cells under the raster tiles that changed.
        let tiles_x = raster.tiles_x();
        let mut cells: Vec<(u32, u32)> = Vec::new();
        let mut seen = vec![false; (tm.cols * tm.rows) as usize];
        for i in 0..raster.tile_count() {
            if raster.tile_ptr_eq(prev_raster, i) {
                continue;
            }
            let (tx, ty) = (i as u32 % tiles_x, i as u32 / tiles_x);
            let r = IRect::new((tx as usize * TILE) as i32, (ty as usize * TILE) as i32, TILE as i32, TILE as i32);
            for cy in (r.y / th)..=((r.bottom() - 1) / th) {
                for cx in (r.x / tw)..=((r.right() - 1) / tw) {
                    let (cx, cy) = (cx as u32, cy as u32);
                    if cx < tm.cols && cy < tm.rows && !std::mem::replace(&mut seen[(cy * tm.cols + cx) as usize], true)
                    {
                        cells.push((cx, cy));
                    }
                }
            }
        }
        if cells.is_empty() {
            continue;
        }
        let mut changed_tiles: Vec<u32> = Vec::new();
        {
            let ts = &mut doc.tilesets[tm.tileset];
            let raster = &doc.layers[li].raster;
            for &(cx, cy) in &cells {
                let k = (cy * tm.cols + cx) as usize;
                let v = tm.cells[k];
                let px = cell_pixels(raster, ts, cx, cy);
                // Only cells whose own pixels changed were edited; the rest
                // merely share a 64 px raster tile with one that was.
                if px.to_rgba() == cell_pixels(prev_raster, ts, cx, cy).to_rgba() {
                    continue;
                }
                if px.is_empty() {
                    tm.cells[k] = 0;
                    continue;
                }
                // Back into tile orientation.
                let upright = flipped(&px, v);
                let idx = v & INDEX;
                if idx == 0 || idx as usize >= ts.tiles.len() {
                    tm.cells[k] = ts.intern(upright);
                } else if ts.tiles[idx as usize].to_rgba() != upright.to_rgba() {
                    ts.tiles[idx as usize] = upright;
                    if !changed_tiles.contains(&idx) {
                        changed_tiles.push(idx);
                    }
                }
            }
        }
        let tileset = tm.tileset;
        doc.layers[li].props.tilemap = Some(tm);
        if changed_tiles.is_empty() {
            continue;
        }
        // Every instance of a changed tile, in every layer on this tileset.
        let ts = doc.tilesets[tileset].clone();
        for layer in &mut doc.layers {
            let Some(tm) = layer.props.tilemap.as_ref().filter(|t| t.tileset == tileset) else { continue };
            let tm = tm.clone();
            for cy in 0..tm.rows {
                for cx in 0..tm.cols {
                    let v = tm.cell(cx, cy);
                    if changed_tiles.contains(&(v & INDEX)) {
                        redrawn.push(render_cell(&mut layer.raster, &ts, cx, cy, v));
                    }
                }
            }
        }
    }
    redrawn
}

/// Turn every tilemap layer back into a plain pixel layer (after a canvas
/// crop, resize, rotation or flip, which the cell grid can't follow).
pub fn detach_all(doc: &mut DocState) {
    for l in &mut doc.layers {
        if l.props.tilemap.take().is_some() {
            l.props.kind = crate::layer::LayerKind::Raster;
        }
    }
}

/// How many cells use each tile of tileset `ts` across all layers.
pub fn usage(doc: &DocState, ts: usize) -> Vec<u32> {
    let n = doc.tilesets.get(ts).map_or(0, |t| t.tiles.len());
    let mut u = vec![0u32; n];
    for l in &doc.layers {
        if let Some(tm) = l.props.tilemap.as_ref().filter(|t| t.tileset == ts) {
            for &v in &tm.cells {
                if let Some(c) = u.get_mut((v & INDEX) as usize) {
                    *c += 1;
                }
            }
        }
    }
    u
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;
    use crate::layer::LayerKind;

    fn doc_with_tilemap() -> DocState {
        // Two identical 4×4 red squares in a 16×8 canvas of 4×4 cells.
        let mut doc = DocState::new(16, 8, None);
        for (ox, oy) in [(0, 0), (8, 4)] {
            for y in 0..4 {
                for x in 0..4 {
                    doc.layers[0].raster.set_pixel(ox + x, oy + y, Rgba8::new(255, 0, 0, 255));
                }
            }
        }
        let (ts, tm) = from_raster(&doc.layers[0].raster, "t", 4, 4);
        doc.tilesets = vec![ts];
        doc.layers[0].props.kind = LayerKind::Tilemap;
        doc.layers[0].props.tilemap = Some(tm);
        doc
    }

    #[test]
    fn identical_cells_share_a_tile() {
        let doc = doc_with_tilemap();
        assert_eq!(doc.tilesets[0].tiles.len(), 2, "empty + red");
        let tm = doc.layers[0].props.tilemap.as_ref().unwrap();
        assert_eq!(tm.cell(0, 0), 1);
        assert_eq!(tm.cell(2, 1), 1);
        assert_eq!(tm.cell(1, 0), 0);
    }

    #[test]
    fn painting_one_instance_updates_the_other() {
        let mut doc = doc_with_tilemap();
        let before = doc.clone();
        // Paint a blue pixel into the first red square.
        doc.layers[0].raster.set_pixel(1, 1, Rgba8::new(0, 0, 255, 255));
        let redrawn = sync(&mut doc, &before);
        assert!(!redrawn.is_empty());
        assert_eq!(doc.layers[0].raster.get_pixel(9, 5), Rgba8::new(0, 0, 255, 255), "the other instance follows");
        assert_eq!(doc.tilesets[0].tiles.len(), 2);
    }

    #[test]
    fn painting_an_empty_cell_makes_a_new_tile() {
        let mut doc = doc_with_tilemap();
        let before = doc.clone();
        doc.layers[0].raster.set_pixel(5, 1, Rgba8::new(0, 255, 0, 255));
        sync(&mut doc, &before);
        let tm = doc.layers[0].props.tilemap.as_ref().unwrap();
        assert_eq!(tm.cell(1, 0), 2);
        assert_eq!(doc.tilesets[0].tiles.len(), 3);
    }
}
