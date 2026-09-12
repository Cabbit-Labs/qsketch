//! Per-document viewport: zoom, pan, rotation and the document↔screen mapping.

use egui::{Pos2, Rect, Vec2};
use qsketch_core::Pt;

/// Photoshop-like zoom stops.
pub const ZOOM_STEPS: &[f32] = &[
    0.02, 0.03, 0.05, 0.0667, 0.0833, 0.125, 0.1667, 0.25, 0.3333, 0.5, 0.6667, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0,
    12.0, 16.0, 24.0, 32.0, 48.0, 64.0,
];
pub const MIN_ZOOM: f32 = 0.01;
pub const MAX_ZOOM: f32 = 128.0;

#[derive(Clone, Debug)]
pub struct CanvasView {
    /// Screen points per document pixel.
    pub zoom: f32,
    /// Document point shown at the viewport center.
    pub center: Pt,
    /// Radians, clockwise on screen.
    pub rotation: f32,
    pub flip_h: bool,
    /// Screen rect of the canvas widget (points).
    pub viewport: Rect,
    pub initialized: bool,
    /// Leftover pan speed after a hand drag ends (screen points per second);
    /// ticked down each frame by `tick_inertia`.
    pub pan_velocity: Vec2,
}

impl Default for CanvasView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            center: Pt::new(0.0, 0.0),
            rotation: 0.0,
            flip_h: false,
            viewport: Rect::ZERO,
            initialized: false,
            pan_velocity: Vec2::ZERO,
        }
    }
}

impl CanvasView {
    pub fn doc_to_screen(&self, p: Pt) -> Pos2 {
        let mut d = Vec2::new((p.x - self.center.x) * self.zoom, (p.y - self.center.y) * self.zoom);
        if self.flip_h {
            d.x = -d.x;
        }
        let (s, c) = self.rotation.sin_cos();
        let r = Vec2::new(c * d.x - s * d.y, s * d.x + c * d.y);
        self.viewport.center() + r
    }

    pub fn screen_to_doc(&self, s: Pos2) -> Pt {
        let d = s - self.viewport.center();
        let (sn, c) = (-self.rotation).sin_cos();
        let mut r = Vec2::new(c * d.x - sn * d.y, sn * d.x + c * d.y);
        if self.flip_h {
            r.x = -r.x;
        }
        Pt::new(self.center.x + r.x / self.zoom, self.center.y + r.y / self.zoom)
    }

    /// Convert a screen-space delta to a document-space delta.
    pub fn screen_delta_to_doc(&self, d: Vec2) -> Pt {
        let (sn, c) = (-self.rotation).sin_cos();
        let mut r = Vec2::new(c * d.x - sn * d.y, sn * d.x + c * d.y);
        if self.flip_h {
            r.x = -r.x;
        }
        Pt::new(r.x / self.zoom, r.y / self.zoom)
    }

    /// Fit the whole document in the viewport with a small margin.
    pub fn fit(&mut self, doc_w: u32, doc_h: u32) {
        let vp = self.viewport;
        if vp.width() <= 1.0 || vp.height() <= 1.0 {
            return;
        }
        self.rotation = 0.0;
        self.flip_h = false;
        let margin = 24.0;
        let zx = (vp.width() - margin * 2.0) / doc_w as f32;
        let zy = (vp.height() - margin * 2.0) / doc_h as f32;
        self.zoom = zx.min(zy).clamp(MIN_ZOOM, MAX_ZOOM);
        self.center = Pt::new(doc_w as f32 / 2.0, doc_h as f32 / 2.0);
        self.initialized = true;
    }

    /// Set zoom keeping the document point under `anchor` (screen) fixed.
    pub fn set_zoom(&mut self, new_zoom: f32, anchor: Option<Pos2>) {
        let new_zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let anchor = anchor.unwrap_or(self.viewport.center());
        let a = self.screen_to_doc(anchor);
        let k = self.zoom / new_zoom;
        self.center = Pt::new(a.x - (a.x - self.center.x) * k, a.y - (a.y - self.center.y) * k);
        self.zoom = new_zoom;
    }

    pub fn zoom_in(&mut self, anchor: Option<Pos2>) {
        let next = ZOOM_STEPS.iter().copied().find(|&z| z > self.zoom * 1.001).unwrap_or(MAX_ZOOM);
        self.set_zoom(next, anchor);
    }

    pub fn zoom_out(&mut self, anchor: Option<Pos2>) {
        let next = ZOOM_STEPS.iter().rev().copied().find(|&z| z < self.zoom * 0.999).unwrap_or(MIN_ZOOM);
        self.set_zoom(next, anchor);
    }

    /// Smooth multiplicative zoom (wheel / pinch).
    pub fn zoom_by(&mut self, factor: f32, anchor: Option<Pos2>) {
        self.set_zoom(self.zoom * factor, anchor);
    }

    pub fn pan_by_screen(&mut self, delta: Vec2) {
        let d = self.screen_delta_to_doc(delta);
        self.center = Pt::new(self.center.x - d.x, self.center.y - d.y);
    }

    /// Advance the post-drag glide by `dt` seconds. Returns true while still moving.
    pub fn tick_inertia(&mut self, dt: f32, doc_w: u32, doc_h: u32) -> bool {
        if self.pan_velocity == Vec2::ZERO {
            return false;
        }
        let dt = dt.clamp(0.0, 0.1);
        // Exponential decay: ~85% of the speed gone after 1/3 s.
        const DECAY_PER_SEC: f32 = 5.5;
        let keep = (-DECAY_PER_SEC * dt).exp();
        let before = self.center;
        self.pan_by_screen(self.pan_velocity * dt);
        self.clamp_to_document(doc_w, doc_h);
        self.pan_velocity *= keep;
        let moved = before.x != self.center.x || before.y != self.center.y;
        // Stop once slow, or when the clamp pins the view.
        if self.pan_velocity.length() < 20.0 || (!moved && dt > 0.0) {
            self.pan_velocity = Vec2::ZERO;
        }
        self.pan_velocity != Vec2::ZERO
    }

    pub fn rotate_by(&mut self, radians: f32, anchor: Option<Pos2>) {
        let anchor = anchor.unwrap_or(self.viewport.center());
        let a = self.screen_to_doc(anchor);
        self.rotation = (self.rotation + radians) % std::f32::consts::TAU;
        // keep the anchor fixed
        let after = self.doc_to_screen(a);
        self.pan_by_screen(anchor - after);
    }

    pub fn set_rotation(&mut self, radians: f32) {
        let a = self.screen_to_doc(self.viewport.center());
        self.rotation = radians % std::f32::consts::TAU;
        let after = self.doc_to_screen(a);
        self.pan_by_screen(self.viewport.center() - after);
    }

    pub fn reset_rotation(&mut self) {
        self.set_rotation(0.0);
    }

    pub fn rotation_degrees(&self) -> f32 {
        self.rotation.to_degrees()
    }

    /// Clamp the view so the document can't be scrolled entirely out of sight.
    pub fn clamp_to_document(&mut self, doc_w: u32, doc_h: u32) {
        let margin = 8.0 / self.zoom.max(0.001);
        self.center.x = self.center.x.clamp(-margin, doc_w as f32 + margin);
        self.center.y = self.center.y.clamp(-margin, doc_h as f32 + margin);
    }

    /// Screen-space rect of the document (axis aligned bounding box).
    #[allow(dead_code)]
    pub fn doc_bounds_on_screen(&self, doc_w: u32, doc_h: u32) -> Rect {
        let pts = [
            self.doc_to_screen(Pt::new(0.0, 0.0)),
            self.doc_to_screen(Pt::new(doc_w as f32, 0.0)),
            self.doc_to_screen(Pt::new(0.0, doc_h as f32)),
            self.doc_to_screen(Pt::new(doc_w as f32, doc_h as f32)),
        ];
        Rect::from_points(&pts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_anchor() {
        let mut v = CanvasView {
            viewport: Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(800.0, 600.0)),
            ..Default::default()
        };
        v.fit(400, 300);
        v.rotate_by(0.3, None);
        let p = Pt::new(12.0, 34.0);
        let s = v.doc_to_screen(p);
        let back = v.screen_to_doc(s);
        assert!((back.x - p.x).abs() < 1e-3 && (back.y - p.y).abs() < 1e-3);
        let anchor = Pos2::new(300.0, 200.0);
        let before = v.screen_to_doc(anchor);
        v.set_zoom(v.zoom * 2.0, Some(anchor));
        let after = v.screen_to_doc(anchor);
        assert!((before.x - after.x).abs() < 1e-3 && (before.y - after.y).abs() < 1e-3);
    }
}
