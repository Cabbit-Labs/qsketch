//! Liquify: a per-pixel displacement field the user pushes around with brush
//! tools, applied to a raster by inverse mapping (each output pixel samples
//! the source at `p + d(p)`), so no pixel is ever "lost" mid-edit and the
//! field can be smoothed back toward zero to reconstruct.

use rayon::prelude::*;

use crate::geom::{IRect, Pt};
use crate::mask::Mask;
use crate::raster::Raster;

/// Smooth 1 → 0 brush falloff over the radius.
#[inline]
fn falloff(dist: f32, radius: f32) -> f32 {
    if dist >= radius {
        return 0.0;
    }
    let t = 1.0 - dist / radius;
    t * t * (3.0 - 2.0 * t)
}

/// Side of the squares the stroke history copies the field in.
const TILE: i32 = 64;
/// Most memory the undoable strokes may hold; the oldest go first.
const HISTORY_BYTES: usize = 192 << 20;

/// What one stroke changed: the field values under every tile the stroke
/// touched (before it, or after it once undone), so undo and redo are the same
/// swap.
#[derive(Clone, Debug, Default)]
struct Patch {
    tiles: Vec<(i32, i32, Vec<[f32; 2]>)>,
}

impl Patch {
    fn bytes(&self) -> usize {
        self.tiles.iter().map(|t| t.2.len() * std::mem::size_of::<[f32; 2]>()).sum()
    }
}

#[derive(Clone, Debug)]
pub struct Field {
    w: u32,
    h: u32,
    /// Source offset per output pixel, row-major.
    d: Vec<[f32; 2]>,
    /// The stroke being drawn (between `begin_stroke` and `end_stroke`).
    open: Option<Patch>,
    undo: Vec<Patch>,
    redo: Vec<Patch>,
}

impl Field {
    pub fn new(w: u32, h: u32) -> Self {
        Self { w, h, d: vec![[0.0; 2]; (w * h) as usize], open: None, undo: Vec::new(), redo: Vec::new() }
    }

    pub fn is_identity(&self) -> bool {
        self.d.iter().all(|v| v[0] == 0.0 && v[1] == 0.0)
    }

    /// Back to the untouched picture. Forgets the stroke history too.
    pub fn reset(&mut self) {
        self.d.iter_mut().for_each(|v| *v = [0.0; 2]);
        self.open = None;
        self.undo.clear();
        self.redo.clear();
    }

    /// Start recording a stroke: edits until `end_stroke` undo as one step.
    /// Edits made outside a stroke are not undoable.
    pub fn begin_stroke(&mut self) {
        self.end_stroke();
        self.open = Some(Patch::default());
    }

    /// Finish the stroke started by `begin_stroke`. A stroke that changed
    /// nothing leaves no step behind; a new one drops the redo steps.
    pub fn end_stroke(&mut self) {
        let Some(patch) = self.open.take() else { return };
        if patch.tiles.is_empty() {
            return;
        }
        self.undo.push(patch);
        self.redo.clear();
        let mut total: usize = self.undo.iter().map(Patch::bytes).sum();
        while total > HISTORY_BYTES && self.undo.len() > 1 {
            total -= self.undo.remove(0).bytes();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Take back the last stroke. Returns the area that changed.
    pub fn undo(&mut self) -> Option<IRect> {
        self.end_stroke();
        let patch = self.undo.pop()?;
        let (rect, redo) = self.swap_patch(patch);
        self.redo.push(redo);
        Some(rect)
    }

    /// Put back the stroke `undo` took. Returns the area that changed.
    pub fn redo(&mut self) -> Option<IRect> {
        self.end_stroke();
        let patch = self.redo.pop()?;
        let (rect, undo) = self.swap_patch(patch);
        self.undo.push(undo);
        Some(rect)
    }

    /// The part of tile (`tx`, `ty`) that lies on the field.
    fn tile_rect(&self, tx: i32, ty: i32) -> IRect {
        IRect::new(tx * TILE, ty * TILE, TILE, TILE).intersect(&IRect::new(0, 0, self.w as i32, self.h as i32))
    }

    fn swap_patch(&mut self, mut patch: Patch) -> (IRect, Patch) {
        let mut changed = IRect::EMPTY;
        for (tx, ty, saved) in &mut patch.tiles {
            let r = self.tile_rect(*tx, *ty);
            let row = r.w as usize;
            for (k, y) in (r.y..r.bottom()).enumerate() {
                let at = (y as u32 * self.w + r.x as u32) as usize;
                self.d[at..at + row].swap_with_slice(&mut saved[k * row..(k + 1) * row]);
            }
            changed = changed.union(&r);
        }
        (changed, patch)
    }

    /// Before `rect` is edited, keep what the open stroke has not yet kept of
    /// the tiles it falls on.
    fn save_tiles(&mut self, rect: IRect) {
        if rect.is_empty() {
            return;
        }
        let Some(mut patch) = self.open.take() else { return };
        for ty in rect.y.div_euclid(TILE)..=(rect.bottom() - 1).div_euclid(TILE) {
            for tx in rect.x.div_euclid(TILE)..=(rect.right() - 1).div_euclid(TILE) {
                if patch.tiles.iter().any(|t| t.0 == tx && t.1 == ty) {
                    continue;
                }
                let r = self.tile_rect(tx, ty);
                let mut saved = Vec::with_capacity((r.w * r.h) as usize);
                for y in r.y..r.bottom() {
                    let at = (y as u32 * self.w + r.x as u32) as usize;
                    saved.extend_from_slice(&self.d[at..at + r.w as usize]);
                }
                patch.tiles.push((tx, ty, saved));
            }
        }
        self.open = Some(patch);
    }

    #[inline]
    fn at(&self, x: i32, y: i32) -> [f32; 2] {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return [0.0; 2];
        }
        self.d[(y as u32 * self.w + x as u32) as usize]
    }

    /// The pixels a brush of `radius` at `center` touches.
    pub fn brush_rect(&self, center: Pt, radius: f32) -> IRect {
        IRect::from_f32_bounds(
            center.x - radius - 1.0,
            center.y - radius - 1.0,
            center.x + radius + 1.0,
            center.y + radius + 1.0,
        )
        .intersect(&IRect::new(0, 0, self.w as i32, self.h as i32))
    }

    /// Visit every pixel under the brush with its falloff (× selection
    /// coverage) and current offset.
    fn edit(
        &mut self,
        center: Pt,
        radius: f32,
        sel: Option<&Mask>,
        mut f: impl FnMut(Pt, f32, &mut [f32; 2]),
    ) -> IRect {
        let rect = self.brush_rect(center, radius);
        self.save_tiles(rect);
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                let p = Pt::new(x as f32 + 0.5, y as f32 + 0.5);
                let mut w = falloff(p.dist(center), radius);
                if let Some(m) = sel {
                    w *= m.coverage(x, y);
                }
                if w <= 0.0 {
                    continue;
                }
                let i = (y as u32 * self.w + x as u32) as usize;
                f(p, w, &mut self.d[i]);
            }
        }
        rect
    }

    /// Forward warp: drag pixels along with the pointer by `delta`.
    pub fn push(&mut self, center: Pt, delta: Pt, radius: f32, strength: f32, sel: Option<&Mask>) -> IRect {
        let (dx, dy) = (delta.x * strength, delta.y * strength);
        self.edit(center, radius, sel, |_, w, d| {
            d[0] -= dx * w;
            d[1] -= dy * w;
        })
    }

    /// Rotate the picture around `center` by `angle` radians at the middle,
    /// fading to nothing at the edge.
    pub fn twirl(&mut self, center: Pt, radius: f32, angle: f32, sel: Option<&Mask>) -> IRect {
        self.edit(center, radius, sel, |p, w, d| {
            let (rx, ry) = (p.x + d[0] - center.x, p.y + d[1] - center.y);
            let a = angle * w;
            let (s, c) = a.sin_cos();
            let (nx, ny) = (rx * c - ry * s, rx * s + ry * c);
            d[0] += nx - rx;
            d[1] += ny - ry;
        })
    }

    /// Pull pixels toward the center (`amount` > 0) or push them out (< 0).
    pub fn pinch(&mut self, center: Pt, radius: f32, amount: f32, sel: Option<&Mask>) -> IRect {
        self.edit(center, radius, sel, |p, w, d| {
            let (rx, ry) = (p.x - center.x, p.y - center.y);
            let k = amount * w;
            d[0] += rx * k;
            d[1] += ry * k;
        })
    }

    /// Ease the field back toward the original picture.
    pub fn reconstruct(&mut self, center: Pt, radius: f32, amount: f32, sel: Option<&Mask>) -> IRect {
        self.edit(center, radius, sel, |_, w, d| {
            let k = (1.0 - amount * w).max(0.0);
            d[0] *= k;
            d[1] *= k;
        })
    }

    /// Largest displacement magnitude inside `rect` (how far outside it the
    /// output can have changed after an edit).
    pub fn max_offset(&self, rect: IRect) -> f32 {
        let mut m = 0.0f32;
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                let d = self.at(x, y);
                m = m.max(d[0].abs().max(d[1].abs()));
            }
        }
        m
    }

    /// Render `rect` of the warped picture into `dst` (the rest of `dst` is
    /// left alone). Bilinear on premultiplied color; sampling off the canvas
    /// gives transparency.
    pub fn apply_rect(&self, src: &Raster, dst: &mut Raster, rect: IRect) {
        let rect = rect.intersect(&src.rect());
        if rect.is_empty() {
            return;
        }
        let (w, h) = (src.width() as i32, src.height() as i32);
        let sample = |x: f32, y: f32| -> [f32; 4] {
            let fx = x - 0.5;
            let fy = y - 0.5;
            let x0 = fx.floor() as i32;
            let y0 = fy.floor() as i32;
            let tx = fx - x0 as f32;
            let ty = fy - y0 as f32;
            let px = |xx: i32, yy: i32| -> [f32; 4] {
                if xx < 0 || yy < 0 || xx >= w || yy >= h {
                    return [0.0; 4];
                }
                let c = src.get_pixel(xx, yy);
                let a = c.a as f32 / 255.0;
                [c.r as f32 * a, c.g as f32 * a, c.b as f32 * a, a]
            };
            let (a, b, c, d) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
            let mut o = [0.0; 4];
            for k in 0..4 {
                o[k] = (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty;
            }
            o
        };
        let rows: Vec<Vec<crate::color::Rgba8>> = (rect.y..rect.bottom())
            .into_par_iter()
            .map(|y| {
                (rect.x..rect.right())
                    .map(|x| {
                        let d = self.at(x, y);
                        let o = sample(x as f32 + 0.5 + d[0], y as f32 + 0.5 + d[1]);
                        let a = o[3];
                        if a <= 1.0 / 510.0 {
                            return crate::color::Rgba8::TRANSPARENT;
                        }
                        let un = |v: f32| (v / a + 0.5).clamp(0.0, 255.0) as u8;
                        crate::color::Rgba8::new(
                            un(o[0]),
                            un(o[1]),
                            un(o[2]),
                            (a * 255.0 + 0.5).clamp(0.0, 255.0) as u8,
                        )
                    })
                    .collect()
            })
            .collect();
        for (row, y) in rows.iter().zip(rect.y..) {
            for (px, x) in row.iter().zip(rect.x..) {
                dst.set_pixel(x, y, *px);
            }
        }
    }

    /// The whole warped picture.
    pub fn apply(&self, src: &Raster) -> Raster {
        let mut out = Raster::new(src.width(), src.height());
        self.apply_rect(src, &mut out, src.rect());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    /// An identity field copies the picture; a push moves a dot along with the
    /// drag; reconstruct brings it back.
    #[test]
    fn push_moves_and_reconstruct_restores() {
        let mut src = Raster::new(64, 64);
        src.set_pixel(30, 30, Rgba8::new(255, 0, 0, 255));
        let mut f = Field::new(64, 64);
        assert!(f.is_identity());
        assert_eq!(f.apply(&src).get_pixel(30, 30), Rgba8::new(255, 0, 0, 255));
        // Drag from (30,30) to (36,30) with full strength.
        let mut pos = Pt::new(30.5, 30.5);
        for _ in 0..6 {
            let next = Pt::new(pos.x + 1.0, pos.y);
            f.push(next, Pt::new(1.0, 0.0), 12.0, 1.0, None);
            pos = next;
        }
        let out = f.apply(&src);
        assert_eq!(out.get_pixel(30, 30).a, 0, "the dot left its place");
        let moved: Vec<i32> = (31..40).filter(|&x| out.get_pixel(x, 30).r > 100).collect();
        assert!(!moved.is_empty(), "the dot moved right");
        let before = f.max_offset(IRect::new(0, 0, 64, 64));
        for _ in 0..40 {
            f.reconstruct(Pt::new(33.0, 30.5), 40.0, 0.5, None);
        }
        let after = f.max_offset(IRect::new(0, 0, 64, 64));
        assert!(after < before * 0.05, "reconstruct eases back to identity: {before} -> {after}");
    }

    fn whole(f: &Field) -> Vec<[f32; 2]> {
        f.d.clone()
    }

    /// Undo takes back one stroke at a time, redo puts it back, and a stroke
    /// that spans several tiles (and runs off the edge) restores exactly.
    #[test]
    fn undo_and_redo_step_one_stroke_at_a_time() {
        let mut f = Field::new(150, 90);
        let blank = whole(&f);
        f.begin_stroke();
        for k in 0..20 {
            f.push(Pt::new(10.0 + k as f32 * 7.0, 40.0), Pt::new(3.0, 1.0), 25.0, 1.0, None);
        }
        f.end_stroke();
        let one = whole(&f);
        assert_ne!(one, blank);
        f.begin_stroke();
        f.pinch(Pt::new(140.0, 85.0), 30.0, 0.5, None);
        f.end_stroke();
        let two = whole(&f);
        assert_ne!(two, one);

        assert!(f.can_undo() && !f.can_redo());
        let area = f.undo().expect("second stroke undone");
        assert_eq!(whole(&f), one, "only the last stroke went");
        assert!(area.right() <= 150 && area.bottom() <= 90, "clipped to the field");
        assert!(f.can_redo());
        f.undo().expect("first stroke undone");
        assert_eq!(whole(&f), blank);
        assert!(f.is_identity() && !f.can_undo());
        assert!(f.undo().is_none(), "nothing left");
        f.redo().expect("first stroke back");
        assert_eq!(whole(&f), one);
        f.redo().expect("second stroke back");
        assert_eq!(whole(&f), two);
        assert!(f.redo().is_none());

        // A new stroke after an undo drops what was undone.
        f.undo();
        f.begin_stroke();
        f.twirl(Pt::new(30.0, 30.0), 20.0, 0.3, None);
        f.end_stroke();
        assert!(!f.can_redo());
    }

    #[test]
    fn edits_outside_a_stroke_and_empty_strokes_leave_no_history() {
        let mut f = Field::new(64, 64);
        f.push(Pt::new(30.0, 30.0), Pt::new(2.0, 0.0), 10.0, 1.0, None);
        assert!(!f.can_undo());
        f.begin_stroke();
        f.end_stroke();
        assert!(!f.can_undo());
        f.begin_stroke();
        f.push(Pt::new(30.0, 30.0), Pt::new(2.0, 0.0), 10.0, 1.0, None);
        f.end_stroke();
        f.reset();
        assert!(!f.can_undo() && !f.can_redo() && f.is_identity());
    }
}
