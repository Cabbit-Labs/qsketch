//! Document state (layer stack + selection) and the `Document` wrapper that
//! ties a working state, its history, the composite cache and file identity
//! together.

use std::path::PathBuf;
use std::sync::Arc;

use crate::blend::composite_pixel;
use crate::color::Rgba8;
use crate::composite::{Composite, TileSet};
use crate::geom::IRect;
use crate::history::History;
use crate::layer::{Layer, LayerId};
use crate::mask::Mask;
use crate::palette::Palette;
use crate::raster::Raster;

/// An immutable-by-convention snapshot of everything undoable.
#[derive(Clone)]
pub struct DocState {
    pub width: u32,
    pub height: u32,
    /// Bottom to top.
    pub layers: Vec<Layer>,
    /// Index into `layers`.
    pub active: usize,
    pub selection: Option<Arc<Mask>>,
    pub next_layer_id: LayerId,
    /// The document's color palette (empty = none). Undoable like everything
    /// else here so a palette edit can be taken back.
    pub palette: Palette,
    /// Indexed-color workflow: every edit is snapped to `palette` on commit
    /// and the color panel only hands out palette colors.
    pub palette_lock: bool,
}

impl DocState {
    /// New document with one layer: filled `Background` when `bg` is given,
    /// otherwise a transparent `Layer 1`.
    pub fn new(width: u32, height: u32, bg: Option<Rgba8>) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let layer = match bg {
            Some(c) if c.a > 0 => {
                // Like Photoshop's Background: opaque, and erasing paints the background color.
                let mut l =
                    Layer::new(1, "Background", width, height).with_raster(Raster::new_filled(width, height, c));
                l.props.alpha_locked = true;
                l
            }
            _ => Layer::new(1, "Layer 1", width, height),
        };
        Self {
            width,
            height,
            layers: vec![layer],
            active: 0,
            selection: None,
            next_layer_id: 2,
            palette: Palette::default(),
            palette_lock: false,
        }
    }

    pub fn from_raster(name: impl Into<String>, raster: Raster) -> Self {
        let (w, h) = (raster.width(), raster.height());
        Self {
            width: w,
            height: h,
            layers: vec![Layer::new(1, name, w, h).with_raster(raster)],
            active: 0,
            selection: None,
            next_layer_id: 2,
            palette: Palette::default(),
            palette_lock: false,
        }
    }

    pub fn rect(&self) -> IRect {
        IRect::new(0, 0, self.width as i32, self.height as i32)
    }

    pub fn new_id(&mut self) -> LayerId {
        let id = self.next_layer_id;
        self.next_layer_id += 1;
        id
    }

    pub fn active_layer(&self) -> &Layer {
        &self.layers[self.active.min(self.layers.len() - 1)]
    }

    pub fn active_layer_mut(&mut self) -> &mut Layer {
        let i = self.active.min(self.layers.len() - 1);
        &mut self.layers[i]
    }

    pub fn index_of(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.props.id == id)
    }

    pub fn layer_by_id(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.props.id == id)
    }

    pub fn selection_mask(&self) -> Option<&Mask> {
        self.selection.as_deref()
    }

    /// `Layer N` with the first unused N.
    pub fn unique_layer_name(&self, base: &str) -> String {
        let mut n = self.layers.len() + 1;
        loop {
            let name = format!("{base} {n}");
            if !self.layers.iter().any(|l| l.props.name == name) {
                return name;
            }
            n += 1;
        }
    }

    /// Insert a new empty layer above `above` (default: above the active
    /// layer) and make it active. A layer inside a group stays in that
    /// group; when `above` is a group the new layer goes inside it, on top.
    /// Returns its id.
    pub fn add_layer(&mut self, name: impl Into<String>, above: Option<usize>) -> LayerId {
        let id = self.new_id();
        let mut layer = Layer::new(id, name, self.width, self.height);
        let anchor = above.unwrap_or(self.active).min(self.layers.len().saturating_sub(1));
        let at = match self.layers.get(anchor) {
            Some(l) if l.is_group() => {
                layer.props.parent = Some(l.props.id);
                anchor
            }
            Some(l) => {
                layer.props.parent = l.props.parent;
                anchor + 1
            }
            None => 0,
        };
        self.insert_layer(layer, at);
        id
    }

    pub fn insert_layer(&mut self, layer: Layer, at: usize) {
        let at = at.min(self.layers.len());
        self.layers.insert(at, layer);
        self.active = at;
    }

    /// Remove a layer (a group together with its members); refuses to empty
    /// the document. Returns the removed entry.
    pub fn remove_layer(&mut self, idx: usize) -> Option<Layer> {
        if idx >= self.layers.len() {
            return None;
        }
        let block = self.block(idx);
        if block.len() >= self.layers.len() {
            return None;
        }
        let removed: Vec<Layer> = self.layers.drain(block.clone()).collect();
        if self.active >= block.end {
            self.active -= block.len();
        } else if self.active >= block.start {
            self.active = block.start;
        }
        self.active = self.active.min(self.layers.len() - 1);
        removed.into_iter().last()
    }

    /// Duplicate a layer (a group with everything inside it) right above the
    /// original and make the copy active.
    pub fn duplicate_layer(&mut self, idx: usize) -> Option<LayerId> {
        self.layers.get(idx)?;
        let block = self.block(idx);
        let mut dup = self.clone_block(block.clone());
        let entry = dup.last_mut()?;
        entry.props.name = format!("{} copy", entry.props.name);
        // A copy of the bottom (Background-style) layer is a regular layer:
        // it must not inherit the implicit alpha lock, or erasing on it would
        // paint the background color instead of revealing the layer below.
        if idx == 0 {
            entry.props.alpha_locked = false;
        }
        let id = entry.props.id;
        let n = dup.len();
        for (k, l) in dup.into_iter().enumerate() {
            self.layers.insert(block.end + k, l);
        }
        self.active = block.end + n - 1;
        Some(id)
    }

    pub fn move_layer(&mut self, from: usize, to: usize) {
        if from >= self.layers.len() || to >= self.layers.len() || from == to {
            return;
        }
        let l = self.layers.remove(from);
        self.layers.insert(to, l);
        if self.active == from {
            self.active = to;
        } else if from < self.active && to >= self.active {
            self.active -= 1;
        } else if from > self.active && to <= self.active {
            self.active += 1;
        }
    }

    /// Merge layer `idx` onto the sibling below it (respecting blend
    /// mode/opacity). A group merges into one raster layer; merging onto a
    /// group rasterizes that group first.
    pub fn merge_down(&mut self, idx: usize) -> bool {
        if idx >= self.layers.len() {
            return false;
        }
        if self.layers[idx].is_group() {
            return self.rasterize_group(idx);
        }
        let Some(below) = self.sibling_below(idx) else { return false };
        let idx = if self.layers[below].is_group() {
            let shrink = self.block(below).len() - 1;
            self.rasterize_group(below);
            idx - shrink
        } else {
            idx
        };
        let top = self.layers[idx].clone();
        let below = &mut self.layers[idx - 1];
        if top.props.visible {
            // Merging bakes both masks in: the result has none.
            below.apply_mask();
            merge_raster(&mut below.raster, &top.masked_raster(), top.props.blend, top.props.opacity);
        }
        self.layers.remove(idx);
        self.active = idx - 1;
        true
    }

    /// Flatten all visible layers into one `Background` layer (hidden layers are dropped).
    pub fn flatten(&mut self) {
        let flat = crate::composite::flatten(self);
        let id = self.new_id();
        self.layers = vec![Layer::new(id, "Background", self.width, self.height).with_raster(flat)];
        self.active = 0;
    }

    /// Merge all visible layers into the active one, leaving hidden layers
    /// (and hidden groups, intact) alone.
    pub fn merge_visible(&mut self) {
        let base = crate::composite::flatten(self);
        // Keep every hidden block; hidden layers found inside visible groups
        // move to the top level since their group goes away.
        let mut hidden: Vec<Layer> = Vec::new();
        let mut i = 0;
        while i < self.layers.len() {
            let l = &self.layers[i];
            if !l.props.visible {
                let block = self.block(i);
                let mut chunk: Vec<Layer> = self.layers[block.clone()].to_vec();
                if let Some(e) = chunk.last_mut() {
                    e.props.parent = None;
                }
                hidden.extend(chunk);
                i = block.end;
            } else {
                i += 1;
            }
        }
        let id = self.new_id();
        let mut merged = Layer::new(id, "Merged", self.width, self.height).with_raster(base);
        merged.props.name = self.active_layer().props.name.clone();
        self.layers = hidden;
        self.layers.push(merged);
        self.active = self.layers.len() - 1;
    }
}

/// Composite `src` onto `dst` in place using `mode`/`opacity` (straight alpha).
pub fn merge_raster(dst: &mut Raster, src: &Raster, mode: crate::blend::BlendMode, opacity: f32) {
    for ty in 0..src.tiles_y() {
        for tx in 0..src.tiles_x() {
            let Some(st) = src.tile(tx, ty) else { continue };
            let dt = dst.tile_mut(tx, ty);
            for i in 0..crate::raster::TILE_PX {
                let s = &st.px[i * 4..i * 4 + 4];
                if s[3] == 0 {
                    continue;
                }
                let d = &mut dt.px[i * 4..i * 4 + 4];
                let back = [d[0] as f32 / 255.0, d[1] as f32 / 255.0, d[2] as f32 / 255.0, d[3] as f32 / 255.0];
                let srcf = [s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0, s[3] as f32 / 255.0];
                let o = composite_pixel(mode, back, srcf, opacity);
                d.copy_from_slice(&Rgba8::from_f32(o).to_array());
            }
        }
    }
}

/// Which tiles differ between two states (by tile identity), for minimal
/// recompositing after undo/redo. Any structural change dirties everything.
pub fn dirty_between(a: &DocState, b: &DocState) -> TileSet {
    let mut set = TileSet::for_size(b.width, b.height);
    if a.width != b.width || a.height != b.height || a.layers.len() != b.layers.len() {
        set.insert_all();
        return set;
    }
    for (la, lb) in a.layers.iter().zip(&b.layers) {
        if la.props != lb.props {
            set.insert_all();
            return set;
        }
        let same_mask = match (&la.mask, &lb.mask) {
            (None, None) => true,
            (Some(x), Some(y)) => Arc::ptr_eq(x, y),
            _ => false,
        };
        if !same_mask {
            set.insert_all();
            return set;
        }
        for idx in 0..lb.raster.tile_count() {
            if !la.raster.tile_ptr_eq(&lb.raster, idx) {
                set.insert_index(idx);
            }
        }
    }
    set
}

/// A live document: the editable working state, its history, composite cache
/// and on-disk identity.
pub struct Document {
    pub history: History,
    working: DocState,
    pub path: Option<PathBuf>,
    pub title: String,
    /// `History` id of the state on disk; `None` when never saved.
    saved_at: Option<u64>,
    /// Photoshop's snapshot row: the document as it was opened (or last
    /// saved), kept outside the undo limit so there is always a way back to
    /// it however long the history grows.
    snapshot: (String, DocState),
    /// Bumped whenever the snapshot changes, so a thumbnail of it can be cached.
    snapshot_rev: u64,
    pub composite: Composite,
    dirty: TileSet,
}

impl Document {
    pub fn new(width: u32, height: u32, bg: Option<Rgba8>, title: impl Into<String>) -> Self {
        Self::from_state(DocState::new(width, height, bg), title, None, "New")
    }

    pub fn from_state(state: DocState, title: impl Into<String>, path: Option<PathBuf>, label: &str) -> Self {
        let (w, h) = (state.width, state.height);
        let mut dirty = TileSet::for_size(w, h);
        dirty.insert_all();
        let mut doc = Self {
            history: History::new(state.clone(), label),
            snapshot: (label.to_string(), state.clone()),
            snapshot_rev: 1,
            working: state,
            path,
            title: title.into(),
            saved_at: Some(0),
            composite: Composite::new(w, h),
            dirty,
        };
        doc.update_composite();
        doc
    }

    pub fn state(&self) -> &DocState {
        &self.working
    }

    /// Mutable working state. Callers must mark the affected tiles dirty and
    /// `commit` when the edit is complete.
    pub fn state_mut(&mut self) -> &mut DocState {
        &mut self.working
    }

    pub fn width(&self) -> u32 {
        self.working.width
    }
    pub fn height(&self) -> u32 {
        self.working.height
    }

    pub fn mark_dirty_rect(&mut self, rect: IRect) {
        self.dirty.insert_rect(rect);
    }

    pub fn mark_dirty_tiles(&mut self, tiles: &TileSet) {
        self.dirty.union_with(tiles);
    }

    pub fn mark_all_dirty(&mut self) {
        self.dirty.insert_all();
    }

    /// Call after the working state's dimensions changed.
    pub fn resized(&mut self) {
        let (w, h) = (self.working.width, self.working.height);
        self.composite = Composite::new(w, h);
        self.dirty = TileSet::for_size(w, h);
        self.dirty.insert_all();
    }

    /// Show or hide a layer without creating an undo step. Visibility is a
    /// view toggle, not an edit: it is applied to every history state so
    /// undo/redo never flips it back.
    pub fn set_layer_visible(&mut self, layer_id: LayerId, visible: bool) {
        let set = |s: &mut DocState| {
            if let Some(l) = s.layers.iter_mut().find(|l| l.props.id == layer_id) {
                l.props.visible = visible;
            }
        };
        set(&mut self.working);
        self.history.for_each_state_mut(set);
        self.dirty.insert_all();
    }

    /// Lock or unlock the palette without an undo step: like visibility it
    /// is a mode, so it is applied to every history state.
    pub fn set_palette_lock(&mut self, on: bool) {
        self.working.palette_lock = on;
        self.history.for_each_state_mut(|s| s.palette_lock = on);
    }

    /// Push the working state onto the history as a new undo step. With the
    /// palette locked, pixels that changed since the last step are snapped
    /// to the palette first, so every edit lands on palette colors.
    pub fn commit(&mut self, label: impl Into<String>) {
        self.enforce_palette();
        self.history.push(label, self.working.clone());
    }

    /// Snap the tiles that differ from the current history state to the
    /// document palette (indexed-color workflow). Only rasters: masks and
    /// the selection are coverage, not color.
    pub fn enforce_palette(&mut self) {
        if !self.working.palette_lock || self.working.palette.is_empty() {
            return;
        }
        let prev = self.history.current();
        let pal = self.working.palette.clone();
        let mut cache: std::collections::HashMap<[u8; 3], [u8; 3]> = std::collections::HashMap::new();
        let mut touched = TileSet::for_size(self.working.width, self.working.height);
        for layer in &mut self.working.layers {
            let before = prev.layers.iter().find(|l| l.props.id == layer.props.id);
            let (tx, ty) = (layer.raster.tiles_x(), layer.raster.tiles_y());
            for idx in 0..layer.raster.tile_count() {
                let same = before.is_some_and(|b| {
                    b.raster.tiles_x() == tx && b.raster.tiles_y() == ty && b.raster.tile_ptr_eq(&layer.raster, idx)
                });
                if same || layer.raster.tile_at_index(idx).is_none() {
                    continue;
                }
                let (x, y) = (idx as u32 % tx, idx as u32 / tx);
                let mut changed = false;
                let tile = layer.raster.tile_mut(x, y);
                for px in tile.px.chunks_exact_mut(4) {
                    if px[3] == 0 {
                        continue;
                    }
                    let key = [px[0], px[1], px[2]];
                    let out = *cache.entry(key).or_insert_with(|| {
                        let c = pal.snap(Rgba8::new(key[0], key[1], key[2], 255));
                        [c.r, c.g, c.b]
                    });
                    if out != key {
                        px[..3].copy_from_slice(&out);
                        changed = true;
                    }
                }
                if changed {
                    touched.insert_index(idx);
                }
            }
        }
        self.dirty.union_with(&touched);
    }

    /// Replace the working state with the current history state (e.g. to
    /// abandon a half-finished edit).
    pub fn revert_working(&mut self) {
        let s = self.history.current().clone();
        self.restore(s);
    }

    fn restore(&mut self, state: DocState) {
        let resized = state.width != self.working.width || state.height != self.working.height;
        let d = dirty_between(&self.working, &state);
        // Undo/redo should not yank the user onto whichever layer was active
        // when the snapshot was taken: stay on the current layer as long as
        // it still exists in the restored state.
        let keep = self.working.active_layer().props.id;
        self.working = state;
        if let Some(i) = self.working.index_of(keep) {
            self.working.active = i;
        }
        if resized {
            self.resized();
        } else {
            self.dirty.union_with(&d);
        }
    }

    pub fn undo(&mut self) -> bool {
        match self.history.undo().cloned() {
            Some(s) => {
                self.restore(s);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.history.redo().cloned() {
            Some(s) => {
                self.restore(s);
                true
            }
            None => false,
        }
    }

    pub fn jump_to(&mut self, index: usize) -> bool {
        if index == self.history.cursor() {
            return false;
        }
        match self.history.jump_to(index).cloned() {
            Some(s) => {
                self.restore(s);
                true
            }
            None => false,
        }
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Recomposite dirty tiles; returns the set of tiles that changed so the
    /// renderer can upload just those.
    pub fn update_composite(&mut self) -> TileSet {
        if self.dirty.is_empty() {
            return TileSet::for_size(self.working.width, self.working.height);
        }
        let dirty = std::mem::replace(&mut self.dirty, TileSet::for_size(self.working.width, self.working.height));
        self.composite.update(&self.working, &dirty);
        dirty
    }

    pub fn has_pending_composite(&self) -> bool {
        !self.dirty.is_empty()
    }

    pub fn is_modified(&self) -> bool {
        self.saved_at != Some(self.history.current_id())
    }

    pub fn mark_saved(&mut self) {
        self.saved_at = Some(self.history.current_id());
        self.snapshot = ("Saved".to_string(), self.working.clone());
        self.snapshot_rev += 1;
    }

    pub fn snapshot_rev(&self) -> u64 {
        self.snapshot_rev
    }

    /// The snapshot row: what the document looked like when opened, or when
    /// last saved. `(label, state)`.
    pub fn snapshot(&self) -> (&str, &DocState) {
        (&self.snapshot.0, &self.snapshot.1)
    }

    /// Go back to the snapshot as a new, undoable history step.
    pub fn revert_to_snapshot(&mut self) {
        let s = self.snapshot.1.clone();
        self.restore(s);
        self.commit("Revert");
    }

    /// Treat the document as never saved (e.g. recovered from an autosave), so
    /// closing it asks and Save writes even without further edits.
    pub fn mark_unsaved(&mut self) {
        self.saved_at = None;
    }

    pub fn display_title(&self) -> String {
        if self.is_modified() {
            format!("{}*", self.title)
        } else {
            self.title.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mask hides pixels in the composite without touching them, disabling
    /// it shows them again, and merging down bakes it in.
    #[test]
    fn layer_mask_hides_composites_and_bakes_on_merge() {
        let mut s = DocState::new(4, 4, None);
        s.add_layer("Top", None);
        s.layers[1].raster.fill_rect(IRect::new(0, 0, 4, 4), Rgba8::new(255, 0, 0, 255));
        let mut m = Mask::full(4, 4);
        m.set(1, 1, 0);
        m.set(2, 2, 128);
        s.layers[1].mask = Some(Arc::new(m));
        let flat = crate::composite::flatten(&s);
        assert_eq!(flat.get_pixel(0, 0), Rgba8::new(255, 0, 0, 255));
        assert_eq!(flat.get_pixel(1, 1).a, 0, "masked out");
        assert!((flat.get_pixel(2, 2).a as i32 - 128).abs() <= 1, "half masked");
        assert_eq!(s.layers[1].raster.get_pixel(1, 1).a, 255, "pixels untouched");
        s.layers[1].props.mask_enabled = false;
        assert_eq!(crate::composite::flatten(&s).get_pixel(1, 1).a, 255, "disabled mask shows all");
        s.layers[1].props.mask_enabled = true;
        assert!(s.merge_down(1));
        assert_eq!(s.layers.len(), 1);
        assert!(s.layers[0].mask.is_none());
        assert_eq!(s.layers[0].raster.get_pixel(1, 1).a, 0, "baked in");
        assert_eq!(s.layers[0].raster.get_pixel(0, 0), Rgba8::new(255, 0, 0, 255));
    }

    /// Whole-canvas transforms carry the mask along.
    #[test]
    fn layer_mask_follows_canvas_transforms() {
        let mut s = DocState::new(4, 2, None);
        let mut m = Mask::new(4, 2);
        m.set(0, 0, 255);
        s.layers[0].mask = Some(Arc::new(m));
        crate::ops::flip_horizontal(&mut s);
        assert_eq!(s.layers[0].mask.as_ref().unwrap().get(3, 0), 255);
        crate::ops::rotate_canvas(&mut s, 1);
        assert_eq!((s.width, s.height), (2, 4));
        assert_eq!(s.layers[0].mask.as_ref().unwrap().width(), 2);
        crate::ops::resize_image(&mut s, 4, 8, crate::raster::ResizeFilter::Nearest);
        assert_eq!(s.layers[0].mask.as_ref().unwrap().width(), 4);
        crate::ops::resize_canvas(&mut s, 6, 8, crate::ops::Anchor::TopLeft);
        assert_eq!(s.layers[0].mask.as_ref().unwrap().width(), 6);
    }

    /// The snapshot outlives the undo limit: after more steps than the
    /// history keeps, the opened state is gone from the list but revert still
    /// reaches it, and as an undoable step.
    #[test]
    fn snapshot_survives_the_undo_limit() {
        let mut doc = Document::new(8, 8, None, "t");
        doc.history.set_limit(3);
        for i in 0..6u8 {
            doc.state_mut().layers[0].raster.set_pixel(0, 0, Rgba8::new(i, 0, 0, 255));
            doc.commit("paint");
        }
        assert_eq!(doc.history.len(), 3);
        assert_eq!(doc.state().layers[0].raster.get_pixel(0, 0).r, 5);
        doc.revert_to_snapshot();
        assert_eq!(doc.state().layers[0].raster.get_pixel(0, 0).a, 0, "back to the blank opened state");
        assert_eq!(doc.history.undo_label(), Some("Revert"));
        assert!(doc.undo());
        assert_eq!(doc.state().layers[0].raster.get_pixel(0, 0).r, 5, "revert is one undoable step");
        doc.mark_saved();
        assert_eq!(doc.snapshot().0, "Saved");
        assert_eq!(doc.snapshot().1.layers[0].raster.get_pixel(0, 0).r, 5);
    }

    #[test]
    fn layer_ops() {
        let mut s = DocState::new(10, 10, Some(Rgba8::WHITE));
        let id = s.add_layer("L2", None);
        assert_eq!(s.layers.len(), 2);
        assert_eq!(s.active, 1);
        assert_eq!(s.index_of(id), Some(1));
        s.duplicate_layer(1);
        assert_eq!(s.layers.len(), 3);
        assert_eq!(s.layers[2].props.name, "L2 copy");
        let mut b = DocState::new(4, 4, Some(Rgba8::WHITE));
        assert!(b.layers[0].props.alpha_locked);
        b.duplicate_layer(0);
        assert!(!b.layers[1].props.alpha_locked, "Background copy must be a regular layer");
        s.move_layer(2, 0);
        assert_eq!(s.layers[0].props.name, "L2 copy");
        assert_eq!(s.active, 0);
        assert!(s.remove_layer(0).is_some());
        assert!(s.merge_down(1));
        assert_eq!(s.layers.len(), 1);
        assert!(s.remove_layer(0).is_none());
    }

    #[test]
    fn document_undo_redo_dirty() {
        let mut d = Document::new(64, 64, None, "t");
        d.state_mut().layers[0].raster.set_pixel(1, 1, Rgba8::BLACK);
        d.mark_dirty_rect(IRect::new(1, 1, 1, 1));
        d.commit("Paint");
        assert!(d.is_modified());
        assert_eq!(d.update_composite().len(), 1);
        assert_eq!(d.composite.get_premul(1, 1), [0, 0, 0, 255]);
        assert!(d.undo());
        assert_eq!(d.update_composite().len(), 1);
        assert_eq!(d.composite.get_premul(1, 1), [0, 0, 0, 0]);
        assert!(d.redo());
        assert!(!d.redo());
        d.update_composite();
        assert_eq!(d.composite.get_premul(1, 1), [0, 0, 0, 255]);
    }

    #[test]
    fn merge_down_blend() {
        let mut s = DocState::new(4, 4, Some(Rgba8::WHITE));
        s.add_layer("top", None);
        s.layers[1].raster.set_pixel(0, 0, Rgba8::new(0, 0, 0, 255));
        s.layers[1].props.opacity = 0.5;
        s.merge_down(1);
        let p = s.layers[0].raster.get_pixel(0, 0);
        assert!((p.r as i32 - 128).abs() <= 1);
    }

    #[test]
    fn locked_palette_snaps_changed_pixels_on_commit() {
        let mut doc = Document::new(8, 8, Some(Rgba8::WHITE), "t");
        doc.state_mut().palette = Palette::new("bw", vec![Rgba8::BLACK, Rgba8::WHITE]);
        doc.set_palette_lock(true);
        doc.state_mut().layers[0].raster.set_pixel(1, 1, Rgba8::rgb(40, 40, 40));
        doc.state_mut().layers[0].raster.set_pixel(2, 2, Rgba8::rgb(220, 220, 220));
        doc.state_mut().layers[0].raster.set_pixel(3, 3, Rgba8::new(40, 40, 40, 100));
        doc.commit("paint");
        let r = &doc.state().layers[0].raster;
        assert_eq!(r.get_pixel(1, 1), Rgba8::BLACK);
        assert_eq!(r.get_pixel(2, 2), Rgba8::WHITE);
        assert_eq!(r.get_pixel(3, 3), Rgba8::new(0, 0, 0, 100), "alpha is kept");
        assert_eq!(doc.history.current().layers[0].raster.get_pixel(1, 1), Rgba8::BLACK);
        // Unlocked: nothing is touched.
        doc.set_palette_lock(false);
        doc.state_mut().layers[0].raster.set_pixel(4, 4, Rgba8::rgb(40, 40, 40));
        doc.commit("paint");
        assert_eq!(doc.state().layers[0].raster.get_pixel(4, 4), Rgba8::rgb(40, 40, 40));
    }
}
