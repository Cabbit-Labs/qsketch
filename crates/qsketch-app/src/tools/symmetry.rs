//! Symmetry painting: every brush sample is mirrored across the horizontal
//! and/or vertical axis and/or repeated radially around a center point, so a
//! single stroke paints all copies at once.

use qsketch_core::Pt;
use serde::{Deserialize, Serialize};

use super::CanvasInput;
use crate::state::{AppState, DocId};

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
    /// Rotation of the whole axis set around the center, radians (0 = the
    /// mirror lines are the canvas axes). Dragging a guide rotates it.
    pub angle: f32,
    /// Guides stay put: presses on them paint instead of grabbing them.
    pub locked: bool,
}

impl Default for Symmetry {
    fn default() -> Self {
        Self {
            horizontal: false,
            vertical: false,
            radial: 0,
            center: None,
            show_guides: true,
            angle: 0.0,
            locked: false,
        }
    }
}

/// Which guide a pointer is over, for dragging it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GuideHit {
    /// The center handle: dragging moves the whole axis set.
    Center,
    /// A line, by its index in [`Symmetry::guide_directions`]. Dragging a
    /// mirror line slides it across the canvas (the center follows); a
    /// radial line, or any line with Alt, rotates the set.
    Line { index: usize, radial: bool },
}

/// One mirrored copy of the input: reflect about the center, then rotate.
#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub center: Pt,
    pub mirror_x: bool,
    pub mirror_y: bool,
    /// Rotation in radians around `center`.
    pub angle: f32,
    /// Rotation of the mirror axes themselves ([`Symmetry::angle`]).
    pub base: f32,
}

impl Transform {
    pub fn apply(&self, p: Pt) -> Pt {
        let mut dx = p.x - self.center.x;
        let mut dy = p.y - self.center.y;
        // Into the axis frame: the mirrors are about lines rotated by `base`.
        if self.base != 0.0 {
            let (s, c) = (-self.base).sin_cos();
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
        let back = self.angle + self.base;
        if back != 0.0 {
            let (s, c) = back.sin_cos();
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
        let back = self.angle + self.base;
        if back != 0.0 {
            let (s, c) = (-back).sin_cos();
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
        if self.base != 0.0 {
            let (s, c) = self.base.sin_cos();
            let (rx, ry) = (dx * c - dy * s, dx * s + dy * c);
            dx = rx;
            dy = ry;
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
                out.push(Transform { center, mirror_x: mx, mirror_y: my, angle, base: self.angle });
            }
        }
        out
    }

    /// Guide lines through the center, as unit directions.
    pub fn guide_directions(&self) -> Vec<(f32, f32)> {
        self.guides().into_iter().map(|(a, _)| (a.cos(), a.sin())).collect()
    }

    /// Guide lines as (angle, is_radial), mirror lines first.
    pub fn guides(&self) -> Vec<(f32, bool)> {
        let mut out: Vec<(f32, bool)> = Vec::new();
        let mut push = |a: f32, radial: bool| {
            let a = (a + self.angle).rem_euclid(std::f32::consts::PI);
            if !out.iter().any(|(b, _)| (b - a).abs() < 1e-3) {
                out.push((a, radial));
            }
        };
        if self.horizontal {
            push(std::f32::consts::FRAC_PI_2, false);
        }
        if self.vertical {
            push(0.0, false);
        }
        if self.radial >= 2 {
            for k in 0..self.radial {
                push(k as f32 * std::f32::consts::PI / self.radial as f32, true);
            }
        }
        out
    }

    /// What guide sits within `tol` document pixels of `p` (`center_tol`
    /// for the center handle, which wins).
    pub fn hit(&self, p: Pt, width: u32, height: u32, tol: f32, center_tol: f32) -> Option<GuideHit> {
        let c = self.center_for(width, height);
        let (dx, dy) = (p.x - c.x, p.y - c.y);
        if (dx * dx + dy * dy).sqrt() <= center_tol {
            return Some(GuideHit::Center);
        }
        let mut best: Option<(GuideHit, f32)> = None;
        for (i, (a, radial)) in self.guides().into_iter().enumerate() {
            // Distance from the point to the infinite line through c.
            let d = (dx * a.sin() - dy * a.cos()).abs();
            if d <= tol && best.is_none_or(|(_, b)| d < b) {
                best = Some((GuideHit::Line { index: i, radial }, d));
            }
        }
        best.map(|(h, _)| h)
    }
}

/// Screen distance (points) within which a guide line is grabbed.
const LINE_GRAB: f32 = 6.0;
/// Screen distance within which the center handle is grabbed.
const CENTER_GRAB: f32 = 9.0;

/// Whether the guides are on screen, which is when they can be dragged.
pub fn guides_shown(state: &AppState) -> bool {
    let tool = state.effective_tool();
    state.symmetry.active()
        && state.symmetry.show_guides
        && (tool.uses_brush() || tool == super::ToolKind::Contour)
        && state.floating.is_none()
        && state.text_edit.is_none()
}

/// The guide under document point `p`, with the current center and angle.
/// None while the guides are locked, so the press paints.
pub fn hit_at(state: &AppState, doc_id: DocId, p: Pt) -> Option<(GuideHit, Pt, f32)> {
    if state.symmetry.locked {
        return None;
    }
    let e = state.doc(doc_id)?;
    let zoom = e.view.zoom.max(0.01);
    let (w, h) = (e.doc.width(), e.doc.height());
    let hit = state.symmetry.hit(p, w, h, LINE_GRAB / zoom, CENTER_GRAB / zoom)?;
    Some((hit, state.symmetry.center_for(w, h), state.symmetry.angle))
}

fn snap_half(v: f32) -> f32 {
    (v * 2.0).round() / 2.0
}

/// Move the dragged guide to follow the pointer at `inp`.
pub fn drag(state: &mut AppState, doc_id: DocId, hit: GuideHit, start: Pt, center0: Pt, angle0: f32, inp: CanvasInput) {
    let p = inp.doc;
    let rotate = match hit {
        GuideHit::Center => false,
        GuideHit::Line { radial, .. } => radial || inp.mods.alt,
    };
    match hit {
        GuideHit::Center => {
            let (mut dx, mut dy) = (p.x - start.x, p.y - start.y);
            if inp.mods.shift {
                if dx.abs() > dy.abs() {
                    dy = 0.0;
                } else {
                    dx = 0.0;
                }
            }
            state.symmetry.center = Some(Pt::new(snap_half(center0.x + dx), snap_half(center0.y + dy)));
        }
        GuideHit::Line { .. } if rotate => {
            let a0 = (start.y - center0.y).atan2(start.x - center0.x);
            let a1 = (p.y - center0.y).atan2(p.x - center0.x);
            let mut angle = angle0 + (a1 - a0);
            if inp.mods.shift {
                let step = 15f32.to_radians();
                angle = (angle / step).round() * step;
            }
            state.symmetry.angle = angle.rem_euclid(std::f32::consts::TAU);
            if state.symmetry.angle.abs() < 1e-4 || (std::f32::consts::TAU - state.symmetry.angle).abs() < 1e-4 {
                state.symmetry.angle = 0.0;
            }
        }
        GuideHit::Line { index, .. } => {
            // Slide the line along its normal; the center carries the rest.
            let Some(e) = state.doc(doc_id) else { return };
            let probe = Symmetry { center: Some(center0), angle: angle0, ..state.symmetry };
            let Some((a, _)) = probe.guides().get(index).copied() else { return };
            let (nx, ny) = (-a.sin(), a.cos());
            let d = (p.x - start.x) * nx + (p.y - start.y) * ny;
            let _ = e;
            state.symmetry.center = Some(Pt::new(snap_half(center0.x + nx * d), snap_half(center0.y + ny * d)));
        }
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
    fn rotated_axes_mirror_about_the_tilted_line() {
        let s = Symmetry { horizontal: true, angle: std::f32::consts::FRAC_PI_2, ..Default::default() };
        let t = s.transforms(100, 100)[0];
        // The mirror line is now horizontal: top ↔ bottom.
        let p = t.apply(Pt::new(10.0, 20.0));
        assert!((p.x - 10.0).abs() < 1e-3 && (p.y - 80.0).abs() < 1e-3, "{p:?}");
        let q = t.apply_inverse(p);
        assert!((q.x - 10.0).abs() < 1e-3 && (q.y - 20.0).abs() < 1e-3);
        assert_eq!(s.hit(Pt::new(50.0, 50.0), 100, 100, 1.0, 4.0), Some(GuideHit::Center));
        assert_eq!(s.hit(Pt::new(20.0, 50.5), 100, 100, 1.0, 4.0), Some(GuideHit::Line { index: 0, radial: false }));
        assert_eq!(s.hit(Pt::new(20.0, 60.0), 100, 100, 1.0, 4.0), None);
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
