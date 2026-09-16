//! Symmetry painting: every brush sample is mirrored across the horizontal
//! and/or vertical axis and/or repeated radially around a center point, so a
//! single stroke paints all copies at once.

use qsketch_core::Pt;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Symmetry {
    /// Mirror across the vertical axis through the center (left ↔ right).
    pub horizontal: bool,
    /// Mirror across the horizontal axis through the center (top ↔ bottom).
    pub vertical: bool,
    /// Number of radial copies (0 or 1 = off).
    pub radial: u32,
    /// Center in document coordinates; None = canvas center.
    pub center: Option<Pt>,
    /// Draw the axis guides on the canvas.
    pub show_guides: bool,
}

impl Default for Symmetry {
    fn default() -> Self {
        Self { horizontal: false, vertical: false, radial: 0, center: None, show_guides: true }
    }
}

/// One mirrored copy of the input: reflect about the center, then rotate.
#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub center: Pt,
    pub mirror_x: bool,
    pub mirror_y: bool,
    /// Rotation in radians around `center`.
    pub angle: f32,
}

impl Transform {
    pub fn apply(&self, p: Pt) -> Pt {
        let mut dx = p.x - self.center.x;
        let mut dy = p.y - self.center.y;
        if self.mirror_x {
            dx = -dx;
        }
        if self.mirror_y {
            dy = -dy;
        }
        if self.angle != 0.0 {
            let (s, c) = self.angle.sin_cos();
            let (rx, ry) = (dx * c - dy * s, dx * s + dy * c);
            dx = rx;
            dy = ry;
        }
        Pt::new(self.center.x + dx, self.center.y + dy)
    }

    /// Map a point back to the source: the copy is mirrored *then* rotated, so
    /// undoing it means unrotating first.
    pub fn apply_inverse(&self, p: Pt) -> Pt {
        let (mut dx, mut dy) = (p.x - self.center.x, p.y - self.center.y);
        if self.angle != 0.0 {
            let (s, c) = (-self.angle).sin_cos();
            let (rx, ry) = (dx * c - dy * s, dx * s + dy * c);
            dx = rx;
            dy = ry;
        }
        if self.mirror_x {
            dx = -dx;
        }
        if self.mirror_y {
            dy = -dy;
        }
        Pt::new(self.center.x + dx, self.center.y + dy)
    }
}

impl Symmetry {
    pub fn active(&self) -> bool {
        self.horizontal || self.vertical || self.radial >= 2
    }

    pub fn clear(&mut self) {
        self.horizontal = false;
        self.vertical = false;
        self.radial = 0;
    }

    pub fn center_for(&self, width: u32, height: u32) -> Pt {
        self.center.unwrap_or_else(|| Pt::new(width as f32 / 2.0, height as f32 / 2.0))
    }

    /// Every non-identity copy of the input for a canvas of the given size.
    pub fn transforms(&self, width: u32, height: u32) -> Vec<Transform> {
        if !self.active() {
            return Vec::new();
        }
        let center = self.center_for(width, height);
        let mut mirrors = vec![(false, false)];
        if self.horizontal {
            mirrors.push((true, false));
        }
        if self.vertical {
            mirrors.push((false, true));
        }
        if self.horizontal && self.vertical {
            mirrors.push((true, true));
        }
        let n = self.radial.max(1);
        let mut out = Vec::new();
        for k in 0..n {
            let angle = k as f32 * std::f32::consts::TAU / n as f32;
            for &(mx, my) in &mirrors {
                if k == 0 && !mx && !my {
                    continue;
                }
                out.push(Transform { center, mirror_x: mx, mirror_y: my, angle });
            }
        }
        out
    }

    /// Guide lines through the center, as unit directions.
    pub fn guide_directions(&self) -> Vec<(f32, f32)> {
        let mut angles: Vec<f32> = Vec::new();
        let mut push = |a: f32| {
            let a = a.rem_euclid(std::f32::consts::PI);
            if !angles.iter().any(|b| (b - a).abs() < 1e-3) {
                angles.push(a);
            }
        };
        if self.horizontal {
            push(std::f32::consts::FRAC_PI_2);
        }
        if self.vertical {
            push(0.0);
        }
        if self.radial >= 2 {
            for k in 0..self.radial {
                push(k as f32 * std::f32::consts::PI / self.radial as f32);
            }
        }
        angles.into_iter().map(|a| (a.cos(), a.sin())).collect()
    }
}

/// Union a mask with its mirrored / rotated copies, so a pixel-perfect shape
/// obeys symmetry the way a brush stroke does. Sampling is nearest-neighbor:
/// the copies stay hard-edged.
pub fn mirror_mask(sym: &Symmetry, mask: &qsketch_core::Mask, width: u32, height: u32) -> qsketch_core::Mask {
    let transforms = sym.transforms(width, height);
    if transforms.is_empty() || mask.is_empty() {
        return mask.clone();
    }
    let mut out = mask.clone();
    let (w, h) = (width as i32, height as i32);
    for t in transforms {
        // Map the source bounds through the transform to find where to write.
        let b = mask.bounds();
        let corners = [
            Pt::new(b.x as f32, b.y as f32),
            Pt::new(b.right() as f32, b.y as f32),
            Pt::new(b.right() as f32, b.bottom() as f32),
            Pt::new(b.x as f32, b.bottom() as f32),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for c in corners {
            let p = t.apply(c);
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        for y in (y0.floor() as i32).max(0)..(y1.ceil() as i32 + 1).min(h) {
            for x in (x0.floor() as i32).max(0)..(x1.ceil() as i32 + 1).min(w) {
                let src = t.apply_inverse(Pt::new(x as f32 + 0.5, y as f32 + 0.5));
                let v = mask.get(src.x.floor() as i32, src.y.floor() as i32);
                if v > 0 && out.get(x, y) < v {
                    out.set(x, y, v);
                }
            }
        }
    }
    out.recompute_bounds();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_count() {
        let mut s = Symmetry::default();
        assert!(s.transforms(100, 100).is_empty());
        s.horizontal = true;
        assert_eq!(s.transforms(100, 100).len(), 1);
        s.vertical = true;
        assert_eq!(s.transforms(100, 100).len(), 3);
        s.radial = 4;
        assert_eq!(s.transforms(100, 100).len(), 15);
    }

    #[test]
    fn mirror_and_rotate() {
        let s = Symmetry { horizontal: true, ..Default::default() };
        let t = s.transforms(100, 100)[0];
        let p = t.apply(Pt::new(10.0, 20.0));
        assert!((p.x - 90.0).abs() < 1e-4 && (p.y - 20.0).abs() < 1e-4);
        let s = Symmetry { radial: 4, ..Default::default() };
        let t = s.transforms(100, 100)[0];
        let p = t.apply(Pt::new(60.0, 50.0));
        assert!((p.x - 50.0).abs() < 1e-3 && (p.y - 60.0).abs() < 1e-3);
    }
}
