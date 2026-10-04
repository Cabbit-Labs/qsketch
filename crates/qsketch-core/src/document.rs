//! Document state (layer stack + selection) and the `Document` wrapper that
//! ties a working state, its history, the composite cache and file identity
//! together.

use std::path::PathBuf;
use std::sync::Arc;

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
    /// Pixel aspect ratio, width : height (1:1 square, 2:1 wide, 1:2 tall).
    /// Only changes how the canvas shows the pixels (and Export Scaled).
    pub pixel_aspect: [u8; 2],
    /// Named rectangles (UI parts, sprite sub-images) with optional 9-slice
    /// centers and pivots; see `crate::slice`.
    pub slices: Vec<crate::slice::Slice>,
    /// Tilesets of the tilemap layers (see `crate::tilemap`).
    pub tilesets: Vec<crate::tilemap::Tileset>,
    /// Animation frames (always at least one) and the one being shown; see
    /// `crate::anim`.
    pub frames: Vec<crate::anim::Frame>,
    pub frame: usize,
    /// Named frame ranges.
    pub tags: Vec<crate::anim::Tag>,
}

impl DocState {
    /// Horizontal stretch of a pixel relative to its height.
    pub fn aspect(&self) -> f32 {
        let [w, h] = self.pixel_aspect;
        if w == 0 || h == 0 {
            1.0
        } else {
            w as f32 / h as f32
        }
    }
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
            pixel_aspect: [1, 1],
            slices: Vec::new(),
            tilesets: Vec::new(),
            frames: vec![crate::anim::Frame::default()],
            frame: 0,
            tags: Vec::new(),
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
            pixel_aspect: [1, 1],
            slices: Vec::new(),
            tilesets: Vec::new(),
            frames: vec![crate::anim::Frame::default()],
            frame: 0,
            tags: Vec::new(),
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
        // Nothing to merge into: an adjustment layer owns no pixels.
        if self.layers[below].is_adjustment() {
            return false;
        }
        let idx = if self.layers[below].is_group() {
            let shrink = self.block(below).len() - 1;
            self.rasterize_group(below);
            idx - shrink
        } else {
            idx
        };
        let top = self.layers[idx].clone();
        if top.is_adjustment() {
            // Merging an adjustment layer bakes the adjustment into the
            // pixels below.
            let below = &mut self.layers[idx - 1];
            if top.props.visible {
                below.apply_mask();
                top.apply_adjustment_to(&mut below.raster);
            }
        } else if top.props.visible {
            self.merge_pair(idx - 1, idx);
        }
        self.layers.remove(idx);
        self.active = idx - 1;
        true
    }

    /// Bake layer `top` onto layer `below` so the result looks the way the
    /// two did: rendered by the compositor on their own, so clipping, masks,
    /// opacity, blend modes and effects all count, and off-canvas pixels are
    /// kept. A Normal layer below takes in its opacity and ends up Normal at
    /// 100%; another blend mode below is kept with its opacity, and the pair
    /// is merged under it as if it were Normal (the closest a single layer
    /// can get). The result is a plain pixel layer without mask or effects.
    fn merge_pair(&mut self, below: usize, top: usize) {
        let lo = &self.layers[below];
        let up = &self.layers[top];
        let keep_mode = !matches!(lo.props.blend, crate::blend::BlendMode::Normal);
        // The pair alone, as plain static layers (the current frame's
        // pictures): the top's frame opacity is baked, the bottom's stays on
        // its cels.
        let mut a = lo.clone();
        a.props.parent = None;
        a.props.visible = true;
        a.props.clipped = false;
        a.cels = Vec::new();
        if keep_mode {
            a.props.blend = crate::blend::BlendMode::Normal;
            a.props.opacity = 1.0;
        }
        let mut b = up.clone();
        b.props.parent = None;
        b.props.opacity *= up.cel_opacity(self.frame);
        b.cels = Vec::new();
        // Both clipped to the same base further down: they stay clipped as
        // one, so here the top just goes over the bottom.
        b.props.clipped = up.props.clipped && !lo.props.clipped;
        let mut pair = self.clone();
        pair.layers = vec![a, b];
        pair.active = 0;
        let raster = crate::composite::flatten_range_keep(&pair, 0..2, None);
        let l = &mut self.layers[below];
        l.raster = raster;
        l.mask = None;
        l.props.style = Default::default();
        if !keep_mode {
            l.props.opacity = 1.0;
        }
        // Drawn from data no more: it is pixels now (a tilemap keeps its
        // grid and takes the pixels into its cells at commit).
        if matches!(
            l.props.kind,
            crate::layer::LayerKind::Shape | crate::layer::LayerKind::Text | crate::layer::LayerKind::Smart
        ) {
            l.props.kind = crate::layer::LayerKind::Raster;
            l.props.shape = None;
            l.props.text = None;
            l.props.smart = None;
            l.smart = None;
        }
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
        // Unlike Flatten, the merged layer keeps what reaches off the canvas.
        let base = crate::composite::flatten_range_keep(self, 0..self.layers.len(), None);
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

/// Which tiles differ between two states (by tile identity), for minimal
/// recompositing after undo/redo. Any structural change dirties everything.
pub fn dirty_between(a: &DocState, b: &DocState) -> TileSet {
    let mut set = TileSet::for_size(b.width, b.height);
    if a.width != b.width || a.height != b.height || a.layers.len() != b.layers.len() || a.frame != b.frame {
        set.insert_all();
        return set;
    }
    for (la, lb) in a.layers.iter().zip(&b.layers) {
        if la.props != lb.props || la.cel_opacity(a.frame) != lb.cel_opacity(b.frame) {
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
/// Lightweight lifetime statistics carried inside a `.qsk` (not undoable,
/// not part of `DocState`): when the document was started, how long it has
/// been worked on, how many edits were committed and how many times it was
/// saved. The app advances `work_secs` while the document is active and the
/// user is interacting; idle time does not count.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DocStats {
    /// Unix seconds (UTC) when the document was created or first imported.
    pub created: Option<u64>,
    /// Active editing time, in seconds.
    pub work_secs: f64,
    /// Number of committed edits (history steps) over the document's life.
    pub edits: u64,
    /// Number of times the document was saved.
    pub saves: u32,
}

impl DocStats {
    /// Stats for a document that starts now.
    pub fn started_now() -> Self {
        Self { created: Some(unix_now()), ..Default::default() }
    }

    /// `work_secs` as `h:mm:ss`.
    pub fn work_time_text(&self) -> String {
        let t = self.work_secs.max(0.0).round() as u64;
        format!("{}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60)
    }

    /// `created` as a `YYYY-MM-DD HH:MM` UTC string.
    pub fn created_text(&self) -> Option<String> {
        self.created.map(|t| {
            let (y, m, d, hh, mm) = civil_from_unix(t);
            format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02} UTC")
        })
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Unix seconds → (year, month, day, hour, minute) in UTC.
pub fn civil_from_unix(secs: u64) -> (i64, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's days → civil algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, (rem / 3600) as u32, ((rem / 60) % 60) as u32)
}

/// A ruler guide: a vertical (`x = pos`) or horizontal (`y = pos`) line in
/// document pixels that geometric tools snap to. Not undoable; saved in
/// `.qsk`.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Guide {
    pub vertical: bool,
    pub pos: f32,
}

/// What a `.qsk` stores besides the layer stack, borrowed from a
/// [`Document`] for saving (see [`Document::meta`]). The default stores none
/// of it.
#[derive(Clone, Copy, Default)]
pub struct DocMeta<'a> {
    pub stats: Option<&'a DocStats>,
    pub guides: &'a [Guide],
    pub timelapse: Option<&'a crate::timelapse::Timelapse>,
}

/// The same, as read back from a file (see [`Document::apply_meta`]).
#[derive(Clone, Debug, Default)]
pub struct LoadedMeta {
    pub stats: DocStats,
    pub guides: Vec<Guide>,
    pub timelapse: crate::timelapse::Timelapse,
}

/// What a document looks like beyond its undoable content: the per-layer
/// visibility and expansion flags and the palette lock (see
/// [`Document::view_state`]).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ViewState {
    layers: Vec<(LayerId, bool, bool)>,
    palette_lock: bool,
}

impl ViewState {
    fn of(s: &DocState) -> Self {
        Self {
            layers: s.layers.iter().map(|l| (l.props.id, l.props.visible, l.props.expanded)).collect(),
            palette_lock: s.palette_lock,
        }
    }
}

pub struct Document {
    pub history: History,
    working: DocState,
    pub path: Option<PathBuf>,
    pub title: String,
    /// Lifetime statistics (see [`DocStats`]); saved in `.qsk`.
    pub stats: DocStats,
    /// Ruler guides (see [`Guide`]); saved in `.qsk`, outside undo.
    pub guides: Vec<Guide>,
    /// Timelapse recording and frames; saved in `.qsk`, outside undo.
    pub timelapse: crate::timelapse::Timelapse,
    /// `History` id of the state on disk; `None` when never saved.
    saved_at: Option<u64>,
    /// The view toggles (layer visibility, group expansion, palette lock)
    /// as they were saved. They are applied to every history state rather
    /// than recorded as undo steps, yet the file carries them, so a change
    /// counts as unsaved until the next save.
    saved_view: ViewState,
    /// Photoshop's snapshot row: the document as it was opened (or last
    /// saved), kept outside the undo limit so there is always a way back to
    /// it however long the history grows.
    snapshot: (String, DocState),
    /// Bumped whenever the snapshot changes, so a thumbnail of it can be cached.
    snapshot_rev: u64,
    pub composite: Composite,
    dirty: TileSet,
    /// Rendered onion-skin frames, keyed by frame (see `refresh_onion`).
    onion_cache: std::collections::HashMap<usize, OnionEntry>,
    /// Bumped by view-only changes (layer visibility) that the onion cache
    /// must notice without a history step.
    view_rev: u64,
}

struct OnionEntry {
    history_id: u64,
    view_rev: u64,
    key: Vec<bool>,
    image: Arc<Raster>,
}

impl Document {
    pub fn new(width: u32, height: u32, bg: Option<Rgba8>, title: impl Into<String>) -> Self {
        Self::from_state(DocState::new(width, height, bg), title, None, "New")
    }

    pub fn from_state(mut state: DocState, title: impl Into<String>, path: Option<PathBuf>, label: &str) -> Self {
        state.repair_animation();
        state.sync_cels();
        let (w, h) = (state.width, state.height);
        let mut dirty = TileSet::for_size(w, h);
        dirty.insert_all();
        let saved_view = ViewState::of(&state);
        let mut doc = Self {
            history: History::new(state.clone(), label),
            snapshot: (label.to_string(), state.clone()),
            snapshot_rev: 1,
            saved_view,
            working: state,
            path,
            title: title.into(),
            stats: DocStats::started_now(),
            guides: Vec::new(),
            timelapse: Default::default(),
            saved_at: Some(0),
            composite: Composite::new(w, h),
            dirty,
            onion_cache: Default::default(),
            view_rev: 0,
        };
        doc.update_composite();
        doc
    }

    // --- animation -------------------------------------------------------

    /// Show another frame (no undo step: like the active layer, the frame
    /// is where the user is, not an edit). Returns whether it changed.
    pub fn set_frame(&mut self, frame: usize) -> bool {
        if !self.working.set_frame(frame) {
            return false;
        }
        self.dirty.insert_all();
        true
    }

    pub fn frame(&self) -> usize {
        self.working.frame
    }

    /// Bring the compositor's onion skins in line with `settings` and the
    /// current frame, rendering frames whose picture changed. Call before
    /// `update_composite`. With `None` (or onion skin off) none are shown.
    pub fn refresh_onion(&mut self, settings: Option<&crate::anim::OnionSettings>) {
        let wanted: Vec<crate::anim::OnionImage> = match settings {
            Some(s) if s.enabled && self.working.is_animated() => {
                let hid = self.history.current_id();
                let mut out = Vec::new();
                for (f, k, before) in s.frames_around(&self.working, self.working.frame) {
                    let key = crate::anim::onion_key(&self.working, f, s.active_layer_only);
                    if !key.iter().any(|v| *v) {
                        continue;
                    }
                    let fresh = self
                        .onion_cache
                        .get(&f)
                        .is_some_and(|e| e.history_id == hid && e.view_rev == self.view_rev && e.key == key);
                    if !fresh {
                        let image = Arc::new(crate::anim::render_onion_frame(&self.working, f, s.active_layer_only));
                        self.onion_cache.insert(f, OnionEntry { history_id: hid, view_rev: self.view_rev, key, image });
                    }
                    let image = self.onion_cache[&f].image.clone();
                    let tint = if before { s.tint_prev } else { s.tint_next };
                    out.push(crate::anim::OnionImage {
                        frame: f,
                        image,
                        tint: [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0],
                        tint_amount: if s.tint { s.tint_amount.clamp(0.0, 1.0) } else { 0.0 },
                        opacity: s.opacity_at(k),
                        behind: s.behind,
                    });
                }
                // Farthest first, so nearer frames draw over them.
                out.reverse();
                out
            }
            _ => Vec::new(),
        };
        if wanted.is_empty() {
            self.onion_cache.clear();
        } else {
            let keep: Vec<usize> = wanted.iter().map(|o| o.frame).collect();
            self.onion_cache.retain(|f, _| keep.contains(f));
        }
        if self.composite.onion != wanted {
            self.composite.onion = wanted;
            self.dirty.insert_all();
        }
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
        self.view_rev += 1;
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
        // The current frame's cel takes the edit (shared tiles: cheap).
        self.working.sync_cels();
        // Painted tilemap cells update their tiles, and every other use of
        // those tiles follows.
        if self.working.layers.iter().any(|l| l.props.tilemap.is_some()) {
            let redrawn = crate::tilemap::sync(&mut self.working, self.history.current());
            for r in redrawn {
                self.mark_dirty_rect(r);
            }
        }
        self.history.push(label, self.working.clone());
        self.stats.edits += 1;
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
        // Likewise stay on the current frame while it exists; the restored
        // state carries every frame's picture, so this is only a swap.
        let keep_frame = self.working.frame;
        self.working = state;
        if let Some(i) = self.working.index_of(keep) {
            self.working.active = i;
        }
        let frame_moved = self.working.set_frame(keep_frame) || keep_frame != self.working.frame;
        if resized {
            self.resized();
        } else if frame_moved {
            self.dirty.insert_all();
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
        let mut dirty = std::mem::replace(&mut self.dirty, TileSet::for_size(self.working.width, self.working.height));
        // Layer effects reach beyond the pixels that changed: refresh the
        // styled copies first, which adds the tiles their effects touch.
        self.composite.styles.update(&self.working, &mut dirty);
        self.composite.update(&self.working, &dirty);
        dirty
    }

    pub fn has_pending_composite(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Unsaved changes: an undo step since the save, or a view toggle
    /// (visibility, group expansion, palette lock) that the file would
    /// record differently.
    pub fn is_modified(&self) -> bool {
        self.saved_at != Some(self.history.current_id()) || self.view_state() != self.saved_view
    }

    /// The view toggles as they stand now (see [`ViewState`]).
    pub fn view_state(&self) -> ViewState {
        ViewState::of(&self.working)
    }

    /// Everything besides the layers that a `.qsk` save should carry.
    pub fn meta(&self) -> DocMeta<'_> {
        DocMeta { stats: Some(&self.stats), guides: &self.guides, timelapse: Some(&self.timelapse) }
    }

    /// Take over what a file stored with the layers: its statistics (when it
    /// had any), guides and timelapse. Recording resumes from here without
    /// an immediate frame.
    pub fn apply_meta(&mut self, meta: LoadedMeta) {
        if meta.stats.created.is_some() {
            self.stats = meta.stats;
        }
        self.guides = meta.guides;
        self.timelapse = meta.timelapse;
        self.timelapse.last_edits = Some(self.stats.edits);
    }

    pub fn mark_saved(&mut self) {
        self.saved_at = Some(self.history.current_id());
        self.saved_view = self.view_state();
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

    /// Hiding a layer is not an undo step, but the file would open with the
    /// layer hidden, so the document counts as unsaved until it is saved
    /// (or the toggle is put back).
    #[test]
    fn view_toggles_mark_the_document_unsaved() {
        let mut d = Document::new(8, 8, None, "t");
        assert!(!d.is_modified());
        let id = d.state().layers[0].props.id;
        d.set_layer_visible(id, false);
        assert!(d.is_modified());
        assert_eq!(d.history.len(), 1, "no history step");
        d.set_layer_visible(id, true);
        assert!(!d.is_modified(), "back to how it was saved");
        d.set_palette_lock(true);
        assert!(d.is_modified());
        d.mark_saved();
        assert!(!d.is_modified());
        d.set_layer_visible(id, false);
        assert!(d.is_modified());
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

    /// Merge Down must look the same as before: flatten, merge, flatten again.
    fn assert_merge_is_wysiwyg(mut d: DocState, top: usize) {
        let before = crate::composite::flatten(&d);
        assert!(d.merge_down(top));
        let after = crate::composite::flatten(&d);
        for y in 0..d.height as i32 {
            for x in 0..d.width as i32 {
                let (a, b) = (before.get_pixel(x, y), after.get_pixel(x, y));
                let close = |p: u8, q: u8| (p as i32 - q as i32).abs() <= 2;
                assert!(
                    close(a.r, b.r) && close(a.g, b.g) && close(a.b, b.b) && close(a.a, b.a),
                    "({x},{y}): before {a:?}, after {b:?}"
                );
            }
        }
    }

    fn three_layers() -> DocState {
        // White backdrop, a half-transparent blob, and a layer above it.
        let mut d = DocState::new(8, 8, Some(Rgba8::WHITE));
        let mut lo = Layer::new(2, "lo", 8, 8);
        lo.raster.fill_rect(IRect::new(1, 1, 4, 4), Rgba8::new(0, 0, 255, 128));
        let mut up = Layer::new(3, "up", 8, 8);
        up.raster.fill_rect(IRect::new(0, 0, 8, 8), Rgba8::new(255, 0, 0, 255));
        d.layers.push(lo);
        d.layers.push(up);
        d.next_layer_id = 4;
        d
    }

    #[test]
    fn merging_a_clipped_layer_keeps_it_inside_its_base() {
        let mut d = three_layers();
        d.layers[2].props.clipped = true;
        let mut check = d.clone();
        assert_merge_is_wysiwyg(d.clone(), 2);
        assert!(check.merge_down(2));
        // Outside the blob the merged layer stays transparent, not red.
        assert_eq!(check.layers[1].raster.get_pixel(6, 6).a, 0);
        // Inside: red clipped to the half-transparent blob, over it.
        let px = check.layers[1].raster.get_pixel(2, 2);
        assert!(px.r > px.b && px.a > 128, "{px:?}");
    }

    #[test]
    fn merging_keeps_opacity_blend_modes_and_masks() {
        let mut d = three_layers();
        d.layers[1].props.opacity = 0.6;
        d.layers[2].props.opacity = 0.5;
        d.layers[2].props.blend = crate::blend::BlendMode::Multiply;
        d.layers[2].props.clipped = true;
        let mut m = Mask::full(8, 8);
        m.set(2, 2, 40);
        d.layers[2].mask = Some(Arc::new(m));
        assert_merge_is_wysiwyg(d, 2);
        // A Normal top over a translucent layer.
        let mut d = three_layers();
        d.layers[2].raster.clear();
        d.layers[2].raster.fill_rect(IRect::new(3, 3, 3, 3), Rgba8::new(0, 200, 0, 90));
        d.layers[1].props.opacity = 0.7;
        assert_merge_is_wysiwyg(d, 2);
    }

    #[test]
    fn merging_keeps_offcanvas_pixels() {
        let mut d = three_layers();
        d.layers[2].raster.clear();
        d.layers[2].raster.set_pixel(0, 0, Rgba8::new(0, 255, 0, 255));
        let rest = crate::moving::begin(&d, &[1, 2]);
        crate::moving::apply(&mut d, &rest, -3, -3);
        assert!(d.merge_down(2));
        let r = &d.layers[1].raster;
        assert_eq!(r.get_pixel_any(-3, -3), Rgba8::new(0, 255, 0, 255));
        assert_eq!(r.get_pixel_any(-2, -2).a, 128);
        assert_eq!(r.get_pixel(1, 1).a, 128);
    }
}
