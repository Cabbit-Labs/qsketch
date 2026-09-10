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
        Self { width, height, layers: vec![layer], active: 0, selection: None, next_layer_id: 2 }
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

    /// Insert a new empty layer above `above` (default: above the active layer)
    /// and make it active. Returns its id.
    pub fn add_layer(&mut self, name: impl Into<String>, above: Option<usize>) -> LayerId {
        let id = self.new_id();
        let layer = Layer::new(id, name, self.width, self.height);
        let at = above.unwrap_or(self.active).min(self.layers.len().saturating_sub(1)) + 1;
        self.insert_layer(layer, at);
        id
    }

    pub fn insert_layer(&mut self, layer: Layer, at: usize) {
        let at = at.min(self.layers.len());
        self.layers.insert(at, layer);
        self.active = at;
    }

    /// Remove a layer; refuses to remove the last one.
    pub fn remove_layer(&mut self, idx: usize) -> Option<Layer> {
        if self.layers.len() <= 1 || idx >= self.layers.len() {
            return None;
        }
        let l = self.layers.remove(idx);
        self.active = self.active.min(self.layers.len() - 1);
        if idx < self.active {
            self.active -= 1;
        }
        Some(l)
    }

    pub fn duplicate_layer(&mut self, idx: usize) -> Option<LayerId> {
        let src = self.layers.get(idx)?.clone();
        let id = self.new_id();
        let mut dup = src;
        dup.props.id = id;
        dup.props.name = format!("{} copy", dup.props.name);
        // A copy of the bottom (Background-style) layer is a regular layer:
        // it must not inherit the implicit alpha lock, or erasing on it would
        // paint the background color instead of revealing the layer below.
        if idx == 0 {
            dup.props.alpha_locked = false;
        }
        self.insert_layer(dup, idx + 1);
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

    /// Merge layer `idx` onto the one below it (respecting blend mode/opacity).
    pub fn merge_down(&mut self, idx: usize) -> bool {
        if idx == 0 || idx >= self.layers.len() {
            return false;
        }
        let top = self.layers[idx].clone();
        let below = &mut self.layers[idx - 1];
        if top.props.visible {
            merge_raster(&mut below.raster, &top.raster, top.props.blend, top.props.opacity);
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

    /// Merge all visible layers into the active one, leaving hidden layers alone.
    pub fn merge_visible(&mut self) {
        let mut base = Raster::new(self.width, self.height);
        for l in self.layers.iter().filter(|l| l.props.visible) {
            merge_raster(&mut base, &l.raster, l.props.blend, l.props.opacity);
        }
        let hidden: Vec<Layer> = self.layers.iter().filter(|l| !l.props.visible).cloned().collect();
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
    saved_at: Option<usize>,
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

    /// Push the working state onto the history as a new undo step.
    pub fn commit(&mut self, label: impl Into<String>) {
        self.history.push(label, self.working.clone());
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
        self.working = state;
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
        self.saved_at != Some(self.history.cursor())
    }

    pub fn mark_saved(&mut self) {
        self.saved_at = Some(self.history.cursor());
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
}
