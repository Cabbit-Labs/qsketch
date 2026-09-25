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

#[derive(Clone, Debug)]
pub struct Field {
    w: u32,
    h: u32,
    /// Source offset per output pixel, row-major.
    d: Vec<[f32; 2]>,
}

impl Field {
    pub fn new(w: u32, h: u32) -> Self {
        Self { w, h, d: vec![[0.0; 2]; (w * h) as usize] }
    }

    pub fn is_identity(&self) -> bool {
        self.d.iter().all(|v| v[0] == 0.0 && v[1] == 0.0)
    }

    pub fn reset(&mut self) {
        self.d.iter_mut().for_each(|v| *v = [0.0; 2]);
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
}
