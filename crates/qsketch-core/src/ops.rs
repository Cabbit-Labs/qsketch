//! Whole-document and layer operations: canvas/image resize, flips, rotation,
//! crop, fills, gradients, floating (move) pixels and color adjustments.
//! Every function mutates a `DocState` and returns the dirty pixel rect where
//! that is meaningful (empty rect = nothing changed; callers mark everything
//! dirty for structural changes).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::blend::src_over;
use crate::color::{Hsv, Rgba8};
use crate::composite::Composite;
use crate::document::DocState;
use crate::geom::{IRect, Pt};
use crate::mask::Mask;
use crate::raster::{Raster, ResizeFilter};

/// Anchor for canvas resize (Photoshop's 3×3 grid).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    pub const ALL: [Anchor; 9] = [
        Anchor::TopLeft,
        Anchor::Top,
        Anchor::TopRight,
        Anchor::Left,
        Anchor::Center,
        Anchor::Right,
        Anchor::BottomLeft,
        Anchor::Bottom,
        Anchor::BottomRight,
    ];

    /// Offset of the old content inside the new canvas.
    pub fn offset(self, old_w: u32, old_h: u32, new_w: u32, new_h: u32) -> (i32, i32) {
        let dw = new_w as i32 - old_w as i32;
        let dh = new_h as i32 - old_h as i32;
        let x = match self {
            Anchor::TopLeft | Anchor::Left | Anchor::BottomLeft => 0,
            Anchor::Top | Anchor::Center | Anchor::Bottom => dw / 2,
            _ => dw,
        };
        let y = match self {
            Anchor::TopLeft | Anchor::Top | Anchor::TopRight => 0,
            Anchor::Left | Anchor::Center | Anchor::Right => dh / 2,
            _ => dh,
        };
        (x, y)
    }
}

pub fn resize_canvas(doc: &mut DocState, new_w: u32, new_h: u32, anchor: Anchor) {
    let new_w = new_w.max(1);
    let new_h = new_h.max(1);
    let (ox, oy) = anchor.offset(doc.width, doc.height, new_w, new_h);
    for l in &mut doc.layers {
        l.raster = l.raster.with_canvas_size(new_w, new_h, ox, oy);
    }
    doc.selection = doc.selection.as_ref().map(|m| Arc::new(m.with_canvas_size(new_w, new_h, ox, oy)));
    doc.width = new_w;
    doc.height = new_h;
}

pub fn resize_image(doc: &mut DocState, new_w: u32, new_h: u32, filter: ResizeFilter) {
    let new_w = new_w.max(1);
    let new_h = new_h.max(1);
    for l in &mut doc.layers {
        l.raster = l.raster.resized(new_w, new_h, filter);
    }
    doc.selection = None;
    doc.width = new_w;
    doc.height = new_h;
}

pub fn crop(doc: &mut DocState, rect: IRect) {
    let r = rect.intersect(&doc.rect());
    if r.is_empty() {
        return;
    }
    for l in &mut doc.layers {
        l.raster = l.raster.crop(r);
    }
    doc.selection = None;
    doc.width = r.w as u32;
    doc.height = r.h as u32;
}

pub fn flip_horizontal(doc: &mut DocState) {
    for l in &mut doc.layers {
        l.raster = l.raster.flipped_h();
    }
    doc.selection = None;
}

pub fn flip_vertical(doc: &mut DocState) {
    for l in &mut doc.layers {
        l.raster = l.raster.flipped_v();
    }
    doc.selection = None;
}

/// Rotate the whole canvas by `times` × 90° clockwise.
pub fn rotate_canvas(doc: &mut DocState, times: u32) {
    for l in &mut doc.layers {
        l.raster = l.raster.rotated(times);
    }
    if times % 2 == 1 {
        std::mem::swap(&mut doc.width, &mut doc.height);
    }
    doc.selection = None;
}

/// Flip / rotate a single layer in place (content only).
pub fn flip_layer_horizontal(doc: &mut DocState, idx: usize) {
    if let Some(l) = doc.layers.get_mut(idx) {
        l.raster = l.raster.flipped_h();
    }
}
pub fn flip_layer_vertical(doc: &mut DocState, idx: usize) {
    if let Some(l) = doc.layers.get_mut(idx) {
        l.raster = l.raster.flipped_v();
    }
}

/// Fill the selection (or whole layer) with a color. Respects selection coverage.
pub fn fill(doc: &mut DocState, layer_idx: usize, color: Rgba8) -> IRect {
    let sel = doc.selection.clone();
    let Some(layer) = doc.layers.get_mut(layer_idx) else { return IRect::EMPTY };
    let alpha_lock = layer.props.alpha_locked;
    let rect = match &sel {
        Some(m) => m.bounds(),
        None => layer.raster.rect(),
    };
    if rect.is_empty() {
        return IRect::EMPTY;
    }
    if sel.is_none() && !alpha_lock {
        layer.raster.fill_rect(rect, color);
        return rect;
    }
    let cf = color.to_f32();
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let cov = sel.as_ref().map_or(1.0, |m| m.coverage(x, y));
            if cov <= 0.0 {
                continue;
            }
            let o = layer.raster.get_pixel(x, y);
            if alpha_lock {
                if o.a == 0 {
                    continue;
                }
                let of = o.to_f32();
                let k = cov * cf[3];
                let n = [of[0] + (cf[0] - of[0]) * k, of[1] + (cf[1] - of[1]) * k, of[2] + (cf[2] - of[2]) * k, of[3]];
                layer.raster.set_pixel(x, y, Rgba8::from_f32(n));
            } else if cov >= 1.0 {
                layer.raster.set_pixel(x, y, color);
            } else {
                let n = src_over(o.to_f32(), [cf[0], cf[1], cf[2], cf[3] * cov]);
                layer.raster.set_pixel(x, y, Rgba8::from_f32(n));
            }
        }
    }
    rect
}

/// Clear the selection (or whole layer) to transparent.
pub fn clear(doc: &mut DocState, layer_idx: usize) -> IRect {
    let sel = doc.selection.clone();
    let Some(layer) = doc.layers.get_mut(layer_idx) else { return IRect::EMPTY };
    match sel {
        None => {
            let r = layer.raster.rect();
            layer.raster.clear();
            r
        }
        Some(m) => {
            let rect = m.bounds();
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    let cov = m.coverage(x, y);
                    if cov <= 0.0 {
                        continue;
                    }
                    let o = layer.raster.get_pixel(x, y);
                    if o.a == 0 {
                        continue;
                    }
                    let a = (o.a as f32 * (1.0 - cov)).round() as u8;
                    layer.raster.set_pixel(x, y, o.with_alpha(a));
                }
            }
            layer.raster.prune_empty_tiles();
            rect
        }
    }
}

/// Paint-bucket fill from a seed point. `merged` samples the composite instead
/// of the layer when given. Respects the selection.
pub fn bucket_fill(
    doc: &mut DocState,
    layer_idx: usize,
    seed: (i32, i32),
    color: Rgba8,
    tolerance: u8,
    contiguous: bool,
    merged: Option<&Composite>,
) -> IRect {
    let (w, h) = (doc.width, doc.height);
    let region = match merged {
        Some(c) => {
            let sample = |x: i32, y: i32| {
                let p = c.get_premul(x, y);
                // compare premultiplied values; good enough for tolerance matching
                Rgba8::from_array(p)
            };
            Mask::from_flood(w, h, seed, tolerance, contiguous, &sample)
        }
        None => {
            let Some(layer) = doc.layers.get(layer_idx) else { return IRect::EMPTY };
            let r = &layer.raster;
            let sample = |x: i32, y: i32| r.get_pixel(x, y);
            Mask::from_flood(w, h, seed, tolerance, contiguous, &sample)
        }
    };
    if region.is_empty() {
        return IRect::EMPTY;
    }
    let region = match &doc.selection {
        Some(sel) => region.intersect(sel),
        None => region,
    };
    let saved = doc.selection.take();
    doc.selection = Some(Arc::new(region));
    let r = fill(doc, layer_idx, color);
    doc.selection = saved;
    r
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GradientKind {
    #[default]
    Linear,
    Radial,
}

/// Fill the selection (or whole layer) with a two-color gradient.
pub fn gradient_fill(
    doc: &mut DocState,
    layer_idx: usize,
    from: Pt,
    to: Pt,
    start: Rgba8,
    end: Rgba8,
    kind: GradientKind,
) -> IRect {
    let sel = doc.selection.clone();
    let Some(layer) = doc.layers.get_mut(layer_idx) else { return IRect::EMPTY };
    let rect = match &sel {
        Some(m) => m.bounds(),
        None => layer.raster.rect(),
    };
    if rect.is_empty() {
        return IRect::EMPTY;
    }
    let alpha_lock = layer.props.alpha_locked;
    let sf = start.to_f32();
    let ef = end.to_f32();
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let len2 = (dx * dx + dy * dy).max(1e-6);
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let cov = sel.as_ref().map_or(1.0, |m| m.coverage(x, y));
            if cov <= 0.0 {
                continue;
            }
            let px = x as f32 + 0.5 - from.x;
            let py = y as f32 + 0.5 - from.y;
            let t = match kind {
                GradientKind::Linear => (px * dx + py * dy) / len2,
                GradientKind::Radial => ((px * px + py * py) / len2).sqrt(),
            }
            .clamp(0.0, 1.0);
            let c = [
                sf[0] + (ef[0] - sf[0]) * t,
                sf[1] + (ef[1] - sf[1]) * t,
                sf[2] + (ef[2] - sf[2]) * t,
                (sf[3] + (ef[3] - sf[3]) * t) * cov,
            ];
            let o = layer.raster.get_pixel(x, y);
            let of = o.to_f32();
            let n = if alpha_lock {
                if o.a == 0 {
                    continue;
                }
                [of[0] + (c[0] - of[0]) * c[3], of[1] + (c[1] - of[1]) * c[3], of[2] + (c[2] - of[2]) * c[3], of[3]]
            } else {
                src_over(of, c)
            };
            layer.raster.set_pixel(x, y, Rgba8::from_f32(n));
        }
    }
    rect
}

/// Pixels lifted off a layer for moving (the Move tool / floating selection).
#[derive(Clone)]
pub struct Floating {
    /// Cropped to the content bounds.
    pub raster: Raster,
    /// Where the raster's origin sat in the layer when lifted.
    pub origin: (i32, i32),
    /// The selection that was lifted, if any (already cropped to bounds).
    pub mask: Option<Mask>,
}

/// Lift the selected pixels (or the whole layer content) off a layer, leaving
/// transparency behind. Returns `None` if there is nothing to move.
pub fn lift(doc: &mut DocState, layer_idx: usize) -> Option<Floating> {
    let sel = doc.selection.clone();
    let layer = doc.layers.get_mut(layer_idx)?;
    match sel {
        None => {
            let b = layer.raster.bounds()?;
            let raster = layer.raster.crop(b);
            layer.raster.fill_rect(b, Rgba8::TRANSPARENT);
            Some(Floating { raster, origin: (b.x, b.y), mask: None })
        }
        Some(m) => {
            let b = m.bounds();
            if b.is_empty() {
                return None;
            }
            let mut raster = Raster::new(b.w as u32, b.h as u32);
            for y in b.y..b.bottom() {
                for x in b.x..b.right() {
                    let cov = m.coverage(x, y);
                    if cov <= 0.0 {
                        continue;
                    }
                    let o = layer.raster.get_pixel(x, y);
                    if o.a == 0 {
                        continue;
                    }
                    let lifted = o.with_alpha((o.a as f32 * cov).round() as u8);
                    raster.set_pixel(x - b.x, y - b.y, lifted);
                    layer.raster.set_pixel(x, y, o.with_alpha((o.a as f32 * (1.0 - cov)).round() as u8));
                }
            }
            layer.raster.prune_empty_tiles();
            let mut mask = Mask::new(b.w as u32, b.h as u32);
            for y in 0..b.h {
                for x in 0..b.w {
                    mask.set(x, y, m.get(x + b.x, y + b.y));
                }
            }
            mask.recompute_bounds();
            Some(Floating { raster, origin: (b.x, b.y), mask: Some(mask) })
        }
    }
}

/// Composite floating pixels back onto `base` at the floating origin + offset,
/// producing the layer raster. `base` is the layer as left by `lift`.
pub fn drop_floating(base: &Raster, floating: &Floating, dx: i32, dy: i32) -> Raster {
    let mut out = base.clone();
    let ox = floating.origin.0 + dx;
    let oy = floating.origin.1 + dy;
    let dst =
        IRect::new(ox, oy, floating.raster.width() as i32, floating.raster.height() as i32).intersect(&out.rect());
    for y in dst.y..dst.bottom() {
        for x in dst.x..dst.right() {
            let s = floating.raster.get_pixel(x - ox, y - oy);
            if s.a == 0 {
                continue;
            }
            if s.a == 255 {
                out.set_pixel(x, y, s);
            } else {
                let o = out.get_pixel(x, y);
                out.set_pixel(x, y, Rgba8::from_f32(src_over(o.to_f32(), s.to_f32())));
            }
        }
    }
    out
}

/// A 90°-step orientation change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orient {
    /// `times` × 90° clockwise.
    Rotate(u32),
    FlipH,
    FlipV,
}

impl Orient {
    pub fn apply_raster(self, r: &Raster) -> Raster {
        match self {
            Orient::Rotate(t) => r.rotated(t),
            Orient::FlipH => r.flipped_h(),
            Orient::FlipV => r.flipped_v(),
        }
    }

    pub fn apply_mask(self, m: &Mask) -> Mask {
        match self {
            Orient::Rotate(t) => m.rotated(t),
            Orient::FlipH => m.flipped_h(),
            Orient::FlipV => m.flipped_v(),
        }
    }
}

/// Rotate / flip the selected pixels of a layer in place, keeping the block
/// centered on its previous center. The selection follows the pixels.
/// Returns the dirty rect (empty when there was nothing selected).
pub fn orient_selection(doc: &mut DocState, layer_idx: usize, o: Orient) -> IRect {
    if doc.selection.is_none() {
        return IRect::EMPTY;
    }
    let Some(f) = lift(doc, layer_idx) else { return IRect::EMPTY };
    let old = IRect::new(f.origin.0, f.origin.1, f.raster.width() as i32, f.raster.height() as i32);
    let raster = o.apply_raster(&f.raster);
    let mask = f.mask.as_ref().map(|m| o.apply_mask(m));
    // Keep the center fixed (rounding toward the top-left on odd differences).
    let ox = f.origin.0 + (old.w - raster.width() as i32) / 2;
    let oy = f.origin.1 + (old.h - raster.height() as i32) / 2;
    let new = IRect::new(ox, oy, raster.width() as i32, raster.height() as i32);
    let placed = Floating { raster, origin: (ox, oy), mask: None };
    let (w, h) = (doc.width, doc.height);
    let layer = &mut doc.layers[layer_idx];
    layer.raster = drop_floating(&layer.raster, &placed, 0, 0);
    doc.selection = mask.map(|m| Arc::new(m.with_canvas_size(w, h, ox, oy)));
    old.union(&new).intersect(&doc.rect())
}

/// Translate the selection mask along with moved pixels.
pub fn translated_selection(doc: &DocState, dx: i32, dy: i32) -> Option<Arc<Mask>> {
    doc.selection.as_ref().map(|m| Arc::new(m.translated(dx, dy)))
}

/// Apply a per-pixel color function to a layer within the selection.
pub fn adjust<F: Fn(Rgba8) -> Rgba8>(doc: &mut DocState, layer_idx: usize, f: F) -> IRect {
    let sel = doc.selection.clone();
    let Some(layer) = doc.layers.get_mut(layer_idx) else { return IRect::EMPTY };
    let rect = match &sel {
        Some(m) => m.bounds(),
        None => layer.raster.bounds().unwrap_or(IRect::EMPTY),
    };
    if rect.is_empty() {
        return IRect::EMPTY;
    }
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let o = layer.raster.get_pixel(x, y);
            if o.a == 0 {
                continue;
            }
            let cov = sel.as_ref().map_or(1.0, |m| m.coverage(x, y));
            if cov <= 0.0 {
                continue;
            }
            let n = f(o);
            let out = if cov >= 1.0 {
                n
            } else {
                let (a, b) = (o.to_f32(), n.to_f32());
                Rgba8::from_f32([
                    a[0] + (b[0] - a[0]) * cov,
                    a[1] + (b[1] - a[1]) * cov,
                    a[2] + (b[2] - a[2]) * cov,
                    a[3] + (b[3] - a[3]) * cov,
                ])
            };
            layer.raster.set_pixel(x, y, out);
        }
    }
    rect
}

pub fn invert_colors(doc: &mut DocState, layer_idx: usize) -> IRect {
    adjust(doc, layer_idx, |c| Rgba8::new(255 - c.r, 255 - c.g, 255 - c.b, c.a))
}

pub fn desaturate(doc: &mut DocState, layer_idx: usize) -> IRect {
    adjust(doc, layer_idx, |c| {
        let l = (c.luma() * 255.0).round() as u8;
        Rgba8::new(l, l, l, c.a)
    })
}

/// `brightness` and `contrast` in -1..=1.
pub fn brightness_contrast(doc: &mut DocState, layer_idx: usize, brightness: f32, contrast: f32) -> IRect {
    let k = (1.0 + contrast).max(0.0);
    adjust(doc, layer_idx, |c| {
        let f = c.to_f32();
        let m = |v: f32| ((v - 0.5) * k + 0.5 + brightness).clamp(0.0, 1.0);
        Rgba8::from_f32([m(f[0]), m(f[1]), m(f[2]), f[3]])
    })
}

/// Hue shift in degrees, saturation and value scale factors (1.0 = unchanged).
pub fn hue_saturation(doc: &mut DocState, layer_idx: usize, hue_shift: f32, sat_scale: f32, val_scale: f32) -> IRect {
    adjust(doc, layer_idx, |c| {
        let h = c.to_hsv();
        Hsv::new(h.h + hue_shift, h.s * sat_scale, h.v * val_scale).to_rgba8(c.a)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_ops() {
        let mut d = DocState::new(10, 10, Some(Rgba8::WHITE));
        resize_canvas(&mut d, 20, 20, Anchor::Center);
        assert_eq!((d.width, d.height), (20, 20));
        assert_eq!(d.layers[0].raster.get_pixel(5, 5), Rgba8::WHITE);
        assert_eq!(d.layers[0].raster.get_pixel(0, 0), Rgba8::TRANSPARENT);
        crop(&mut d, IRect::new(5, 5, 10, 10));
        assert_eq!((d.width, d.height), (10, 10));
        assert_eq!(d.layers[0].raster.get_pixel(0, 0), Rgba8::WHITE);
        rotate_canvas(&mut d, 1);
        resize_image(&mut d, 5, 5, ResizeFilter::Bilinear);
        assert_eq!(d.layers[0].raster.get_pixel(2, 2), Rgba8::WHITE);
    }

    #[test]
    fn fills() {
        let mut d = DocState::new(10, 10, None);
        d.selection = Some(Arc::new(Mask::from_rect(10, 10, IRect::new(0, 0, 5, 5))));
        let r = fill(&mut d, 0, Rgba8::BLACK);
        assert_eq!(r, IRect::new(0, 0, 5, 5));
        assert_eq!(d.layers[0].raster.get_pixel(4, 4), Rgba8::BLACK);
        assert_eq!(d.layers[0].raster.get_pixel(5, 5), Rgba8::TRANSPARENT);
        d.selection = None;
        bucket_fill(&mut d, 0, (7, 7), Rgba8::WHITE, 0, true, None);
        assert_eq!(d.layers[0].raster.get_pixel(9, 9), Rgba8::WHITE);
        assert_eq!(d.layers[0].raster.get_pixel(0, 0), Rgba8::BLACK);
        clear(&mut d, 0);
        assert!(d.layers[0].raster.is_empty());
        gradient_fill(
            &mut d,
            0,
            Pt::new(0.0, 0.0),
            Pt::new(10.0, 0.0),
            Rgba8::BLACK,
            Rgba8::WHITE,
            GradientKind::Linear,
        );
        assert!(d.layers[0].raster.get_pixel(9, 0).r > d.layers[0].raster.get_pixel(0, 0).r);
    }

    #[test]
    fn lift_and_drop() {
        let mut d = DocState::new(10, 10, None);
        d.layers[0].raster.fill_rect(IRect::new(0, 0, 3, 3), Rgba8::WHITE);
        let f = lift(&mut d, 0).unwrap();
        assert_eq!(f.origin, (0, 0));
        assert!(d.layers[0].raster.is_empty());
        let moved = drop_floating(&d.layers[0].raster, &f, 4, 4);
        assert_eq!(moved.get_pixel(4, 4), Rgba8::WHITE);
        assert_eq!(moved.get_pixel(0, 0), Rgba8::TRANSPARENT);
    }

    #[test]
    fn orient_selected_block() {
        // A 4×2 white block at (2,2); rotating 90° gives a 2×4 block on the same center.
        let mut d = DocState::new(10, 10, None);
        d.layers[0].raster.fill_rect(IRect::new(2, 2, 4, 2), Rgba8::WHITE);
        d.layers[0].raster.set_pixel(2, 2, Rgba8::BLACK);
        d.selection = Some(Arc::new(Mask::from_rect(10, 10, IRect::new(2, 2, 4, 2))));
        let dirty = orient_selection(&mut d, 0, Orient::Rotate(1));
        assert!(!dirty.is_empty());
        let sel = d.selection.clone().unwrap();
        assert_eq!(sel.bounds(), IRect::new(3, 1, 2, 4));
        // Top-left pixel of the source ends up top-right after a CW turn.
        assert_eq!(d.layers[0].raster.get_pixel(4, 1), Rgba8::BLACK);
        assert_eq!(d.layers[0].raster.get_pixel(2, 2), Rgba8::TRANSPARENT);
        // Flip back and forth is identity on the bounds.
        orient_selection(&mut d, 0, Orient::FlipH);
        assert_eq!(d.selection.clone().unwrap().bounds(), IRect::new(3, 1, 2, 4));
        assert_eq!(d.layers[0].raster.get_pixel(3, 1), Rgba8::BLACK);
    }
}
