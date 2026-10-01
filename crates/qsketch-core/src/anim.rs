//! Animation: frames, cels, tags, playback order and onion skins.
//!
//! A document has a list of [`Frame`]s (durations) and a current frame.
//! Every pixel layer keeps one [`Cel`] per frame; a cel either owns an image
//! or links to the cel of another frame (the owner), so several frames can
//! show, and edit, the same picture.
//!
//! `Layer::raster` stays the picture of the *current* frame: every tool,
//! filter and compositor keeps working on it unchanged. The current frame's
//! owner cel is brought up to date from `raster` at every commit and before
//! a frame change ([`DocState::sync_cels`]); changing frames then swaps the
//! new frame's cel into `raster` ([`DocState::load_cels`]). Both are cheap:
//! a raster is a vector of shared tile pointers.
//!
//! Group, adjustment and tilemap layers are static: the same in every
//! frame, with no cels.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::color::Rgba8;
use crate::document::DocState;
use crate::layer::Layer;
use crate::raster::Raster;

/// One frame of the animation: how long it is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub duration_ms: u32,
}

impl Default for Frame {
    fn default() -> Self {
        Self { duration_ms: DEFAULT_DURATION_MS }
    }
}

pub const DEFAULT_DURATION_MS: u32 = 100;
pub const MIN_DURATION_MS: u32 = 1;
pub const MAX_DURATION_MS: u32 = 65_535;

/// How a tag's frames play back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TagDirection {
    #[default]
    Forward,
    Reverse,
    PingPong,
    PingPongReverse,
}

impl TagDirection {
    pub const ALL: [TagDirection; 4] =
        [TagDirection::Forward, TagDirection::Reverse, TagDirection::PingPong, TagDirection::PingPongReverse];

    pub fn label(self) -> &'static str {
        match self {
            TagDirection::Forward => "Forward",
            TagDirection::Reverse => "Reverse",
            TagDirection::PingPong => "Ping-pong",
            TagDirection::PingPongReverse => "Ping-pong reverse",
        }
    }

    /// Aseprite's direction byte.
    pub fn to_ase(self) -> u8 {
        match self {
            TagDirection::Forward => 0,
            TagDirection::Reverse => 1,
            TagDirection::PingPong => 2,
            TagDirection::PingPongReverse => 3,
        }
    }

    pub fn from_ase(v: u8) -> Self {
        match v {
            1 => TagDirection::Reverse,
            2 => TagDirection::PingPong,
            3 => TagDirection::PingPongReverse,
            _ => TagDirection::Forward,
        }
    }

    /// Name used in Aseprite's sprite-sheet JSON.
    pub fn json_name(self) -> &'static str {
        match self {
            TagDirection::Forward => "forward",
            TagDirection::Reverse => "reverse",
            TagDirection::PingPong => "pingpong",
            TagDirection::PingPongReverse => "pingpong_reverse",
        }
    }
}

/// A named run of frames (an animation such as "walk" or "idle").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    /// First frame, inclusive.
    pub from: usize,
    /// Last frame, inclusive.
    pub to: usize,
    #[serde(default)]
    pub direction: TagDirection,
    /// How many times the tag plays before playback moves on; 0 = forever.
    #[serde(default)]
    pub repeat: u16,
    pub color: Rgba8,
}

/// Colors handed to new tags in turn.
pub const TAG_COLORS: [Rgba8; 8] = [
    Rgba8::new(255, 99, 71, 255),
    Rgba8::new(255, 171, 64, 255),
    Rgba8::new(240, 200, 50, 255),
    Rgba8::new(120, 200, 80, 255),
    Rgba8::new(64, 190, 200, 255),
    Rgba8::new(90, 140, 255, 255),
    Rgba8::new(170, 110, 255, 255),
    Rgba8::new(255, 110, 190, 255),
];

impl Tag {
    pub fn new(name: impl Into<String>, from: usize, to: usize, color: Rgba8) -> Self {
        let (from, to) = if from <= to { (from, to) } else { (to, from) };
        Self { name: name.into(), from, to, direction: TagDirection::Forward, repeat: 0, color }
    }

    pub fn contains(&self, frame: usize) -> bool {
        (self.from..=self.to).contains(&frame)
    }

    pub fn len(&self) -> usize {
        self.to - self.from + 1
    }

    pub fn is_empty(&self) -> bool {
        self.to < self.from
    }
}

/// A layer's picture in one frame: its own image, or a link to the cel of
/// another frame (the owner) whose image it shares.
#[derive(Clone)]
pub struct Cel {
    pub image: Raster,
    pub link: Option<usize>,
    /// Extra opacity of this cel (an owner's; links share it), 0..=1.
    pub opacity: f32,
    /// Aseprite's per-cel draw order nudge, kept for round trips.
    pub z_index: i16,
}

impl Cel {
    pub fn empty(width: u32, height: u32) -> Self {
        Self { image: Raster::new(width, height), link: None, opacity: 1.0, z_index: 0 }
    }

    pub fn own(image: Raster) -> Self {
        Self { image, link: None, opacity: 1.0, z_index: 0 }
    }

    pub fn linked(to: usize, width: u32, height: u32) -> Self {
        Self { image: Raster::new(width, height), link: Some(to), opacity: 1.0, z_index: 0 }
    }

    pub fn is_linked(&self) -> bool {
        self.link.is_some()
    }
}

/// How a new frame starts out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewFrame {
    /// Every layer empty (continuous layers link to the frame before).
    Empty,
    /// A copy of frame `src` (continuous layers link to it).
    Duplicate(usize),
}

impl Layer {
    /// Whether this layer has one picture per frame. Groups, adjustment and
    /// tilemap layers are the same in every frame.
    pub fn animated(&self) -> bool {
        self.props.kind == crate::layer::LayerKind::Raster
    }

    /// The frame whose cel holds the picture shown in `frame`.
    pub fn cel_owner(&self, frame: usize) -> usize {
        match self.cels.get(frame) {
            Some(c) => c.link.unwrap_or(frame),
            None => frame,
        }
    }

    /// The cel opacity in effect in `frame` (1 for static layers).
    pub fn cel_opacity(&self, frame: usize) -> f32 {
        if !self.animated() {
            return 1.0;
        }
        self.cels.get(self.cel_owner(frame)).map_or(1.0, |c| c.opacity.clamp(0.0, 1.0))
    }

    /// The owner cel shown in `frame`, mutable (for its opacity / z-index).
    pub fn cel_mut(&mut self, frame: usize) -> Option<&mut Cel> {
        let o = self.cel_owner(frame);
        self.cels.get_mut(o)
    }

    /// The picture this layer shows in `frame`, given the document's
    /// current frame (`raster` is always the current frame's picture).
    /// `None` is an empty cel.
    pub fn cel_image(&self, frame: usize, current: usize) -> Option<&Raster> {
        if !self.animated() {
            return Some(&self.raster);
        }
        if self.cel_owner(frame) == self.cel_owner(current) {
            return Some(&self.raster);
        }
        self.cels.get(self.cel_owner(frame)).map(|c| &c.image)
    }

    /// Give every frame its own cel: the current frame's is `raster`, the
    /// others are empty. A no-op when the cels already match `frames`.
    fn materialize_cels(&mut self, frames: usize, current: usize, width: u32, height: u32) {
        if !self.animated() {
            self.cels.clear();
            return;
        }
        if self.cels.len() == frames {
            return;
        }
        if self.cels.is_empty() || self.cels.len() < frames {
            // New frames at the end hold nothing. (A straggler from a layer
            // created before the frames were added has only its raster.)
            let had = self.cels.len();
            self.cels.resize_with(frames, || Cel::empty(width, height));
            if had == 0 && current < frames {
                self.cels[current] = Cel::own(self.raster.clone());
            }
        } else {
            self.cels.truncate(frames);
            for c in &mut self.cels {
                if c.link.is_some_and(|l| l >= frames) {
                    c.link = None;
                }
            }
        }
    }
}

impl DocState {
    pub fn frame_count(&self) -> usize {
        self.frames.len().max(1)
    }

    /// More than one frame.
    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    pub fn frame_duration(&self, frame: usize) -> u32 {
        self.frames.get(frame).map_or(DEFAULT_DURATION_MS, |f| f.duration_ms.max(MIN_DURATION_MS))
    }

    /// Length of the whole animation.
    pub fn total_duration_ms(&self) -> u64 {
        (0..self.frame_count()).map(|f| self.frame_duration(f) as u64).sum()
    }

    /// The picture layer `li` shows in `frame`; `None` is an empty cel.
    pub fn cel_image(&self, li: usize, frame: usize) -> Option<&Raster> {
        self.layers.get(li).and_then(|l| l.cel_image(frame, self.frame))
    }

    /// Whether layer `li` has any pixels in `frame`.
    pub fn cel_has_pixels(&self, li: usize, frame: usize) -> bool {
        self.cel_image(li, frame).is_some_and(|r| !r.is_empty())
    }

    /// Write every layer's `raster` into its current-frame cel. Called at
    /// each commit and before a frame change; cheap (shared tiles).
    pub fn sync_cels(&mut self) {
        if self.frames.is_empty() {
            self.frames.push(Frame::default());
        }
        let n = self.frames.len();
        self.frame = self.frame.min(n - 1);
        let (w, h, f) = (self.width, self.height, self.frame);
        for l in &mut self.layers {
            l.materialize_cels(n, f, w, h);
            if !l.animated() {
                continue;
            }
            let owner = l.cel_owner(f);
            l.cels[owner].image = l.raster.clone();
        }
    }

    /// Put the current frame's cel of every layer into its `raster`.
    pub fn load_cels(&mut self) {
        let n = self.frame_count();
        let (w, h, f) = (self.width, self.height, self.frame.min(n - 1));
        for l in &mut self.layers {
            l.materialize_cels(n, f, w, h);
            if !l.animated() {
                continue;
            }
            let owner = l.cel_owner(f);
            l.raster = l.cels[owner].image.clone();
        }
    }

    /// Make `frame` the current one (clamped). Returns whether it changed.
    pub fn set_frame(&mut self, frame: usize) -> bool {
        let frame = frame.min(self.frame_count() - 1);
        if frame == self.frame && self.layers.iter().all(|l| !l.animated() || l.cels.len() == self.frames.len()) {
            return false;
        }
        self.sync_cels();
        self.frame = frame;
        self.load_cels();
        true
    }

    /// Insert a frame at `at` (clamped to the end) and make it current.
    pub fn insert_frame(&mut self, at: usize, how: NewFrame) -> usize {
        self.sync_cels();
        let at = at.min(self.frames.len());
        let src = match how {
            NewFrame::Duplicate(s) => Some(s.min(self.frames.len() - 1)),
            NewFrame::Empty => None,
        };
        let duration = src.map_or(self.frame_duration(self.frame), |s| self.frame_duration(s));
        self.frames.insert(at, Frame { duration_ms: duration });
        let (w, h) = (self.width, self.height);
        for l in &mut self.layers {
            if !l.animated() {
                continue;
            }
            // Links to frames at or after `at` move along with them.
            for c in &mut l.cels {
                if let Some(t) = c.link.as_mut() {
                    if *t >= at {
                        *t += 1;
                    }
                }
            }
            let cel = match (src, l.props.continuous) {
                (Some(s), true) => {
                    let s = if s >= at { s + 1 } else { s };
                    Cel::linked(l.cel_owner(s), w, h)
                }
                (Some(s), false) => {
                    let s = if s >= at { s + 1 } else { s };
                    let src = &l.cels[l.cel_owner(s)];
                    Cel { image: src.image.clone(), link: None, opacity: src.opacity, z_index: src.z_index }
                }
                (None, true) if at > 0 => Cel::linked(l.cel_owner(at - 1), w, h),
                _ => Cel::empty(w, h),
            };
            l.cels.insert(at, cel);
        }
        for t in &mut self.tags {
            if at <= t.from {
                t.from += 1;
                t.to += 1;
            } else if at <= t.to {
                t.to += 1;
            }
        }
        self.frame = at;
        self.load_cels();
        at
    }

    /// Remove the given frames (the document always keeps one). Returns how
    /// many were removed.
    pub fn remove_frames(&mut self, frames: &[usize]) -> usize {
        self.sync_cels();
        let n = self.frames.len();
        let mut gone = vec![false; n];
        for &f in frames {
            if f < n {
                gone[f] = true;
            }
        }
        if gone.iter().all(|g| *g) {
            // Keep the first one.
            gone[0] = false;
        }
        let removed = gone.iter().filter(|g| **g).count();
        if removed == 0 {
            return 0;
        }
        // New index of each surviving frame.
        let mut new_index = vec![usize::MAX; n];
        let mut k = 0;
        for f in 0..n {
            if !gone[f] {
                new_index[f] = k;
                k += 1;
            }
        }
        for l in &mut self.layers {
            if !l.animated() || l.cels.len() != n {
                continue;
            }
            // A removed owner hands its image to its first surviving link.
            for owner in 0..n {
                if !gone[owner] || l.cels[owner].link.is_some() {
                    continue;
                }
                let heirs: Vec<usize> = (0..n).filter(|&f| !gone[f] && l.cels[f].link == Some(owner)).collect();
                if let Some(&first) = heirs.first() {
                    l.cels[first] = Cel::own(l.cels[owner].image.clone());
                    for &h in &heirs[1..] {
                        l.cels[h].link = Some(first);
                    }
                }
            }
            let old = std::mem::take(&mut l.cels);
            l.cels = old
                .into_iter()
                .enumerate()
                .filter(|(f, _)| !gone[*f])
                .map(|(_, mut c)| {
                    if let Some(t) = c.link {
                        c.link = (new_index[t] != usize::MAX).then_some(new_index[t]);
                    }
                    c
                })
                .collect();
        }
        self.frames = self.frames.iter().enumerate().filter(|(f, _)| !gone[*f]).map(|(_, fr)| *fr).collect();
        self.tags.retain_mut(|t| {
            let keep: Vec<usize> = (t.from..=t.to).filter(|&f| f < n && !gone[f]).collect();
            match (keep.first(), keep.last()) {
                (Some(&a), Some(&b)) => {
                    t.from = new_index[a];
                    t.to = new_index[b];
                    true
                }
                _ => false,
            }
        });
        // Land on the frame that took the removed one's place.
        let mut f = self.frame;
        while f < n && gone[f] {
            f += 1;
        }
        self.frame = if f < n { new_index[f] } else { self.frames.len() - 1 };
        self.load_cels();
        removed
    }

    /// Reorder the frames: `order[i]` is the old index of the frame that
    /// becomes frame `i`. Links follow their owners; tags keep their
    /// positions.
    pub fn reorder_frames(&mut self, order: &[usize]) {
        let n = self.frames.len();
        if order.len() != n {
            return;
        }
        let mut seen = vec![false; n];
        for &o in order {
            if o >= n || seen[o] {
                return;
            }
            seen[o] = true;
        }
        self.sync_cels();
        let mut new_index = vec![0usize; n];
        for (new, &old) in order.iter().enumerate() {
            new_index[old] = new;
        }
        self.frames = order.iter().map(|&o| self.frames[o]).collect();
        for l in &mut self.layers {
            if !l.animated() || l.cels.len() != n {
                continue;
            }
            let old = std::mem::take(&mut l.cels);
            let mut cells: Vec<Option<Cel>> = old.into_iter().map(Some).collect();
            l.cels = order
                .iter()
                .map(|&o| {
                    let mut c = cells[o].take().expect("each old index once");
                    if let Some(t) = c.link.as_mut() {
                        *t = new_index[*t];
                    }
                    c
                })
                .collect();
        }
        self.frame = new_index[self.frame.min(n - 1)];
        self.load_cels();
    }

    /// Move the frames in `which` so they start at `to` (an index in the
    /// list without them), keeping their order.
    pub fn move_frames(&mut self, which: &[usize], to: usize) {
        let n = self.frames.len();
        let mut moving: Vec<usize> = which.iter().copied().filter(|&f| f < n).collect();
        moving.sort_unstable();
        moving.dedup();
        if moving.is_empty() {
            return;
        }
        let rest: Vec<usize> = (0..n).filter(|f| !moving.contains(f)).collect();
        let to = to.min(rest.len());
        let mut order = Vec::with_capacity(n);
        order.extend_from_slice(&rest[..to]);
        order.extend_from_slice(&moving);
        order.extend_from_slice(&rest[to..]);
        self.reorder_frames(&order);
    }

    /// Reverse the order of the frames `from..=to`.
    pub fn reverse_frames(&mut self, from: usize, to: usize) {
        let n = self.frames.len();
        let (from, to) = (from.min(to).min(n - 1), from.max(to).min(n - 1));
        if from >= to {
            return;
        }
        let mut order: Vec<usize> = (0..n).collect();
        order[from..=to].reverse();
        self.reorder_frames(&order);
    }

    pub fn set_frame_duration(&mut self, frame: usize, ms: u32) {
        if let Some(f) = self.frames.get_mut(frame) {
            f.duration_ms = ms.clamp(MIN_DURATION_MS, MAX_DURATION_MS);
        }
    }

    /// Link the cels of layer `li` in `frames` to the first of them (they
    /// all show, and edit, that picture).
    pub fn link_cels(&mut self, li: usize, frames: &[usize]) {
        self.sync_cels();
        let n = self.frames.len();
        let mut fs: Vec<usize> = frames.iter().copied().filter(|&f| f < n).collect();
        fs.sort_unstable();
        fs.dedup();
        let Some(&first) = fs.first() else { return };
        let (w, h) = (self.width, self.height);
        let Some(l) = self.layers.get_mut(li) else { return };
        if !l.animated() || l.cels.len() != n {
            return;
        }
        let owner = l.cel_owner(first);
        for &f in &fs[1..] {
            if l.cel_owner(f) == owner {
                continue;
            }
            // Anything linked to this cel follows it to the new owner.
            if l.cels[f].link.is_none() {
                for g in 0..n {
                    if l.cels[g].link == Some(f) {
                        l.cels[g].link = Some(owner);
                    }
                }
            }
            l.cels[f] = Cel::linked(owner, w, h);
        }
        self.load_cels();
    }

    /// Give the cel of layer `li` in `frame` its own copy of the picture.
    pub fn unlink_cel(&mut self, li: usize, frame: usize) {
        self.sync_cels();
        let n = self.frames.len();
        let Some(l) = self.layers.get_mut(li) else { return };
        if !l.animated() || l.cels.len() != n || frame >= n {
            return;
        }
        let Some(owner) = l.cels[frame].link else {
            // The owner itself: hand the image to the first link, if any, so
            // this frame can go its own way.
            let links: Vec<usize> = (0..n).filter(|&g| l.cels[g].link == Some(frame)).collect();
            if let Some(&first) = links.first() {
                let img = l.cels[frame].image.clone();
                l.cels[first] = Cel::own(img.clone());
                for &g in &links[1..] {
                    l.cels[g].link = Some(first);
                }
                l.cels[frame] = Cel::own(img);
            }
            self.load_cels();
            return;
        };
        let img = l.cels[owner].image.clone();
        l.cels[frame] = Cel::own(img);
        self.load_cels();
    }

    /// Empty the cel of layer `li` in `frame` (unlinking it first).
    pub fn clear_cel(&mut self, li: usize, frame: usize) {
        self.sync_cels();
        let n = self.frames.len();
        let (w, h) = (self.width, self.height);
        let Some(l) = self.layers.get_mut(li) else { return };
        if !l.animated() || frame >= n {
            return;
        }
        if l.cels.len() == n {
            if l.cels[frame].link.is_some() {
                l.cels[frame] = Cel::empty(w, h);
            } else {
                // Links to this cel keep the old picture among themselves.
                let links: Vec<usize> = (0..n).filter(|&g| l.cels[g].link == Some(frame)).collect();
                if let Some(&first) = links.first() {
                    l.cels[first] = Cel::own(l.cels[frame].image.clone());
                    for &g in &links[1..] {
                        l.cels[g].link = Some(first);
                    }
                }
                l.cels[frame] = Cel::empty(w, h);
            }
        }
        self.load_cels();
    }

    /// Replace the picture of layer `li` in `frame` (unlinking it).
    pub fn set_cel_image(&mut self, li: usize, frame: usize, image: Raster) {
        self.sync_cels();
        let n = self.frames.len();
        let Some(l) = self.layers.get_mut(li) else { return };
        if !l.animated() || frame >= n {
            return;
        }
        if l.cels.len() == n {
            if l.cels[frame].link.is_none() {
                let links: Vec<usize> = (0..n).filter(|&g| l.cels[g].link == Some(frame)).collect();
                if let Some(&first) = links.first() {
                    l.cels[first] = Cel::own(l.cels[frame].image.clone());
                    for &g in &links[1..] {
                        l.cels[g].link = Some(first);
                    }
                }
            }
            l.cels[frame] = Cel::own(image);
        }
        self.load_cels();
    }

    /// Frames of layer `li` that share the cel shown in `frame`.
    pub fn linked_frames(&self, li: usize, frame: usize) -> Vec<usize> {
        let Some(l) = self.layers.get(li) else { return Vec::new() };
        let n = self.frame_count();
        if !l.animated() || l.cels.len() != n {
            return vec![frame];
        }
        let owner = l.cel_owner(frame);
        (0..n).filter(|&f| l.cel_owner(f) == owner).collect()
    }

    // --- tags --------------------------------------------------------------

    /// The first tag containing `frame`, if any.
    pub fn tag_at(&self, frame: usize) -> Option<usize> {
        self.tags.iter().position(|t| t.contains(frame))
    }

    /// Add a tag over `from..=to` with the next color in turn; returns its index.
    pub fn add_tag(&mut self, name: impl Into<String>, from: usize, to: usize) -> usize {
        let n = self.frame_count();
        let color = TAG_COLORS[self.tags.len() % TAG_COLORS.len()];
        let tag = Tag::new(name, from.min(n - 1), to.min(n - 1), color);
        self.tags.push(tag);
        self.tags.len() - 1
    }

    /// A tag name not in use yet (`Tag 1`, `Tag 2`…).
    pub fn unique_tag_name(&self) -> String {
        let mut k = self.tags.len() + 1;
        loop {
            let name = format!("Tag {k}");
            if !self.tags.iter().any(|t| t.name == name) {
                return name;
            }
            k += 1;
        }
    }

    /// Clamp every tag and the current frame to the frame list (after an
    /// import or a hand-edited file).
    pub fn repair_animation(&mut self) {
        if self.frames.is_empty() {
            self.frames.push(Frame::default());
        }
        let n = self.frames.len();
        self.frame = self.frame.min(n - 1);
        self.tags.retain_mut(|t| {
            t.from = t.from.min(n - 1);
            t.to = t.to.min(n - 1);
            if t.from > t.to {
                std::mem::swap(&mut t.from, &mut t.to);
            }
            true
        });
        let (w, h, f) = (self.width, self.height, self.frame);
        for l in &mut self.layers {
            if l.animated() && l.cels.len() != n && !l.cels.is_empty() {
                l.materialize_cels(n, f, w, h);
            }
            for c in &mut l.cels {
                if c.link.is_some_and(|t| t >= n) {
                    c.link = None;
                }
            }
            // A link to a link: point at the root.
            for i in 0..l.cels.len() {
                let mut t = l.cels[i].link;
                let mut hops = 0;
                while let Some(x) = t {
                    match l.cels[x].link {
                        Some(y) if y != x && hops < n => {
                            t = Some(y);
                            hops += 1;
                        }
                        _ => break,
                    }
                }
                if t != l.cels[i].link {
                    l.cels[i].link = t.filter(|&x| x != i);
                }
            }
        }
    }

    /// The frames `from..=to` played by `direction`, as a sequence of frame
    /// indices for one pass (ping-pong goes there and back without
    /// repeating the ends).
    pub fn play_sequence(from: usize, to: usize, direction: TagDirection) -> Vec<usize> {
        let (from, to) = (from.min(to), from.max(to));
        let fwd: Vec<usize> = (from..=to).collect();
        match direction {
            TagDirection::Forward => fwd,
            TagDirection::Reverse => fwd.into_iter().rev().collect(),
            TagDirection::PingPong => {
                let mut v = fwd.clone();
                if fwd.len() > 2 {
                    v.extend(fwd[1..fwd.len() - 1].iter().rev());
                }
                v
            }
            TagDirection::PingPongReverse => {
                let mut v: Vec<usize> = fwd.iter().rev().copied().collect();
                if fwd.len() > 2 {
                    v.extend(fwd[1..fwd.len() - 1].iter());
                }
                v
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering other frames

/// A full composite of `frame`: every visible layer, as it would be saved.
pub fn render_frame(doc: &DocState, frame: usize) -> Raster {
    if frame == doc.frame || !doc.is_animated() {
        return crate::composite::flatten_par(doc);
    }
    let mut copy = doc.clone();
    copy.set_frame(frame);
    crate::composite::flatten_par(&copy)
}

/// Straight RGBA8 of `render_frame`.
pub fn render_frame_rgba(doc: &DocState, frame: usize) -> Vec<u8> {
    render_frame(doc, frame).to_rgba()
}

/// What an onion skin of `frame` shows: the pixel layers whose picture in
/// that frame differs from the current one (static layers and cels shared
/// with the current frame would only draw the same thing again), or with
/// `active_only` just the active layer. Groups keep their blending.
pub fn render_onion_frame(doc: &DocState, frame: usize, active_only: bool) -> Raster {
    let mut copy = doc.clone();
    copy.sync_cels();
    let cur = copy.frame;
    let active = copy.active;
    for (i, l) in copy.layers.iter_mut().enumerate() {
        if l.is_group() {
            continue;
        }
        let same = !l.animated() || l.cel_owner(frame) == l.cel_owner(cur);
        if same || (active_only && i != active) {
            l.props.visible = false;
        }
    }
    copy.set_frame(frame);
    crate::composite::flatten_par(&copy)
}

/// Which layers an onion skin of `frame` would show (see
/// [`render_onion_frame`]); the key its cache is checked against.
pub fn onion_key(doc: &DocState, frame: usize, active_only: bool) -> Vec<bool> {
    let cur = doc.frame;
    doc.layers
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if l.is_group() {
                return true;
            }
            let same = !l.animated() || l.cel_owner(frame) == l.cel_owner(cur);
            !(same || (active_only && i != doc.active))
        })
        .collect()
}

/// One onion-skin picture the compositor blends with the layers.
#[derive(Clone)]
pub struct OnionImage {
    pub frame: usize,
    pub image: Arc<Raster>,
    /// Tint color and how far toward it the picture is pulled (0 = none).
    pub tint: [f32; 3],
    pub tint_amount: f32,
    pub opacity: f32,
    /// Drawn under the layers (true) or over them.
    pub behind: bool,
}

impl PartialEq for OnionImage {
    fn eq(&self, o: &Self) -> bool {
        self.frame == o.frame
            && Arc::ptr_eq(&self.image, &o.image)
            && self.tint == o.tint
            && self.tint_amount == o.tint_amount
            && self.opacity == o.opacity
            && self.behind == o.behind
    }
}

/// Onion-skin appearance (kept in the app's settings).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OnionSettings {
    pub enabled: bool,
    /// Frames before and after the current one to show.
    pub prev: u8,
    pub next: u8,
    /// Opacity of the nearest frame, and how much each step further fades.
    pub opacity: f32,
    pub falloff: f32,
    pub tint: bool,
    pub tint_prev: [u8; 3],
    pub tint_next: [u8; 3],
    pub tint_amount: f32,
    /// Under the artwork rather than over it.
    pub behind: bool,
    /// Stay inside the current tag, wrapping around its ends.
    pub loop_tag: bool,
    /// Only the active layer.
    pub active_layer_only: bool,
}

impl Default for OnionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            prev: 1,
            next: 1,
            opacity: 0.5,
            falloff: 0.6,
            tint: true,
            tint_prev: [255, 60, 60],
            tint_next: [60, 120, 255],
            tint_amount: 0.6,
            behind: true,
            loop_tag: true,
            active_layer_only: false,
        }
    }
}

impl OnionSettings {
    /// The frames to show around `frame`, with their distance (1 = adjacent)
    /// and whether they come before it, nearest first.
    pub fn frames_around(&self, doc: &DocState, frame: usize) -> Vec<(usize, u8, bool)> {
        let n = doc.frame_count();
        let range = if self.loop_tag { doc.tag_at(frame).map(|i| (doc.tags[i].from, doc.tags[i].to)) } else { None };
        let (lo, hi) = range.unwrap_or((0, n - 1));
        let len = hi - lo + 1;
        let mut out = Vec::new();
        let mut seen = vec![frame];
        for k in 1..=self.prev.max(self.next) {
            if k <= self.prev {
                let f = if range.is_some() {
                    lo + (frame - lo + len - (k as usize % len)) % len
                } else if frame >= k as usize {
                    frame - k as usize
                } else {
                    usize::MAX
                };
                if f != usize::MAX && !seen.contains(&f) {
                    seen.push(f);
                    out.push((f, k, true));
                }
            }
            if k <= self.next {
                let f = if range.is_some() {
                    lo + (frame - lo + k as usize) % len
                } else if frame + (k as usize) < n {
                    frame + k as usize
                } else {
                    usize::MAX
                };
                if f != usize::MAX && !seen.contains(&f) {
                    seen.push(f);
                    out.push((f, k, false));
                }
            }
        }
        out
    }

    pub fn opacity_at(&self, distance: u8) -> f32 {
        (self.opacity * self.falloff.clamp(0.0, 1.0).powi(distance.saturating_sub(1) as i32)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::IRect;

    fn red() -> Rgba8 {
        Rgba8::new(255, 0, 0, 255)
    }

    #[test]
    fn frames_keep_their_own_pictures() {
        let mut d = DocState::new(8, 8, None);
        d.layers[0].raster.set_pixel(0, 0, red());
        d.insert_frame(1, NewFrame::Empty);
        assert_eq!(d.frame, 1);
        assert_eq!(d.layers[0].raster.get_pixel(0, 0).a, 0, "new empty frame");
        d.layers[0].raster.set_pixel(1, 1, red());
        d.set_frame(0);
        assert_eq!(d.layers[0].raster.get_pixel(0, 0), red());
        assert_eq!(d.layers[0].raster.get_pixel(1, 1).a, 0);
        d.set_frame(1);
        assert_eq!(d.layers[0].raster.get_pixel(1, 1), red());
        assert!(d.cel_has_pixels(0, 0));
        assert_eq!(d.cel_image(0, 0).unwrap().get_pixel(0, 0), red());
    }

    #[test]
    fn duplicate_copies_and_continuous_links() {
        let mut d = DocState::new(8, 8, None);
        d.layers[0].raster.set_pixel(0, 0, red());
        d.add_layer("cont", None);
        d.layers[1].props.continuous = true;
        d.layers[1].raster.set_pixel(2, 2, red());
        d.insert_frame(1, NewFrame::Duplicate(0));
        assert!(!d.layers[0].cels[1].is_linked());
        assert!(d.layers[1].cels[1].is_linked());
        assert_eq!(d.layers[1].cel_owner(1), 0);
        // Editing a linked cel edits the owner too.
        d.layers[1].raster.set_pixel(3, 3, red());
        d.set_frame(0);
        assert_eq!(d.layers[1].raster.get_pixel(3, 3), red(), "linked");
        d.layers[0].raster.set_pixel(4, 4, red());
        d.set_frame(1);
        assert_eq!(d.layers[0].raster.get_pixel(4, 4).a, 0, "a copy is independent");
        assert_eq!(d.linked_frames(1, 0), vec![0, 1]);
        d.unlink_cel(1, 1);
        d.layers[1].raster.set_pixel(5, 5, red());
        d.set_frame(0);
        assert_eq!(d.layers[1].raster.get_pixel(5, 5).a, 0, "unlinked");
        d.link_cels(1, &[0, 1]);
        d.set_frame(1);
        assert_eq!(d.layers[1].raster.get_pixel(5, 5).a, 0, "linked back to frame 0's picture");
    }

    #[test]
    fn removing_an_owner_hands_the_picture_to_its_links() {
        let mut d = DocState::new(4, 4, None);
        d.layers[0].raster.set_pixel(0, 0, red());
        d.insert_frame(1, NewFrame::Duplicate(0));
        d.insert_frame(2, NewFrame::Duplicate(0));
        d.link_cels(0, &[0, 1, 2]);
        d.add_tag("all", 0, 2);
        assert_eq!(d.remove_frames(&[0]), 1);
        assert_eq!(d.frame_count(), 2);
        assert_eq!(d.tags[0].from, 0);
        assert_eq!(d.tags[0].to, 1);
        assert!(!d.layers[0].cels[0].is_linked());
        assert_eq!(d.layers[0].cels[1].link, Some(0));
        d.set_frame(1);
        assert_eq!(d.layers[0].raster.get_pixel(0, 0), red());
        // Never below one frame.
        assert_eq!(d.remove_frames(&[0, 1]), 1);
        assert_eq!(d.frame_count(), 1);
        assert!(d.tags.is_empty() || d.tags[0].len() == 1);
    }

    #[test]
    fn reorder_and_reverse_follow_links() {
        let mut d = DocState::new(4, 4, None);
        for i in 0..3 {
            d.layers[0].raster.set_pixel(i, 0, red());
            d.insert_frame(i as usize + 1, NewFrame::Empty);
        }
        // Frames 0..3 have 1,2,3,0 pixels in the top row.
        d.set_frame(0);
        d.reverse_frames(0, 3);
        d.set_frame(0);
        assert!(d.layers[0].raster.is_empty());
        d.set_frame(3);
        assert_eq!(d.layers[0].raster.get_pixel(0, 0), red());
        d.move_frames(&[3], 0);
        d.set_frame(0);
        assert_eq!(d.layers[0].raster.get_pixel(0, 0), red());
    }

    #[test]
    fn whole_canvas_ops_reach_every_frame() {
        let mut d = DocState::new(4, 4, None);
        d.layers[0].raster.set_pixel(0, 0, red());
        d.insert_frame(1, NewFrame::Empty);
        d.layers[0].raster.set_pixel(3, 3, red());
        crate::ops::flip_horizontal(&mut d);
        assert_eq!(d.layers[0].raster.get_pixel(0, 3), red());
        d.set_frame(0);
        assert_eq!(d.layers[0].raster.get_pixel(3, 0), red(), "the other frame flipped too");
        crate::ops::resize_canvas(&mut d, 8, 8, crate::ops::Anchor::TopLeft);
        d.set_frame(1);
        assert_eq!(d.layers[0].raster.width(), 8);
        assert_eq!(d.layers[0].raster.get_pixel(0, 3), red());
        crate::ops::crop(&mut d, IRect::new(0, 2, 4, 4));
        assert_eq!(d.layers[0].raster.get_pixel(0, 1), red());
        d.set_frame(0);
        assert_eq!(d.layers[0].raster.height(), 4);
    }

    #[test]
    fn play_sequences() {
        assert_eq!(DocState::play_sequence(1, 3, TagDirection::Forward), vec![1, 2, 3]);
        assert_eq!(DocState::play_sequence(1, 3, TagDirection::Reverse), vec![3, 2, 1]);
        assert_eq!(DocState::play_sequence(1, 4, TagDirection::PingPong), vec![1, 2, 3, 4, 3, 2]);
        assert_eq!(DocState::play_sequence(1, 4, TagDirection::PingPongReverse), vec![4, 3, 2, 1, 2, 3]);
        assert_eq!(DocState::play_sequence(2, 2, TagDirection::PingPong), vec![2]);
    }

    #[test]
    fn onion_frames_wrap_inside_a_tag() {
        let mut d = DocState::new(4, 4, None);
        for i in 1..6 {
            d.insert_frame(i, NewFrame::Empty);
        }
        d.add_tag("walk", 2, 4);
        let s = OnionSettings { prev: 1, next: 1, loop_tag: true, ..Default::default() };
        assert_eq!(s.frames_around(&d, 2), vec![(4, 1, true), (3, 1, false)]);
        let s = OnionSettings { prev: 2, next: 1, loop_tag: false, ..Default::default() };
        assert_eq!(s.frames_around(&d, 1), vec![(0, 1, true), (2, 1, false)]);
        assert_eq!(s.frames_around(&d, 5), vec![(4, 1, true), (3, 2, true)]);
    }

    #[test]
    fn onion_render_skips_shared_cels() {
        let mut d = DocState::new(4, 4, None);
        d.layers[0].raster.set_pixel(0, 0, red());
        d.insert_frame(1, NewFrame::Duplicate(0));
        d.link_cels(0, &[0, 1]);
        d.add_layer("own", None);
        d.layers[1].raster.set_pixel(1, 1, red());
        d.set_frame(0);
        let o = render_onion_frame(&d, 1, false);
        assert_eq!(o.get_pixel(0, 0).a, 0, "shared cel left out");
        assert_eq!(o.get_pixel(1, 1), red());
        assert_eq!(onion_key(&d, 1, false), vec![false, true]);
        let full = render_frame(&d, 1);
        assert_eq!(full.get_pixel(0, 0), red());
        assert_eq!(full.get_pixel(1, 1), red());
    }
}
