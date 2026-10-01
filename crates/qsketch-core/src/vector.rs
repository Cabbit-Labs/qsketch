//! Vector shape layers for pixel art: a polygon of pixel-grid points that
//! is rasterized without anti-aliasing (an even-odd fill through pixel
//! centers plus a pixel-stepped outline), so the shape stays editable
//! while the layer shows crisp pixels. One shape per layer.

use serde::{Deserialize, Serialize};

use crate::color::Rgba8;
use crate::document::DocState;
use crate::geom::IRect;
use crate::raster::Raster;

/// The editable description of a shape layer's picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShapePath {
    /// Vertices as pixel coordinates (the pixel the point sits on), in order.
    pub points: Vec<[i32; 2]>,
    /// Join the last point back to the first (and fill the inside).
    pub closed: bool,
    pub fill: Option<Rgba8>,
    pub stroke: Option<Rgba8>,
    /// Outline thickness in pixels (0 = no outline even with a color).
    pub stroke_width: u32,
}

impl Default for ShapePath {
    fn default() -> Self {
        Self { points: Vec::new(), closed: true, fill: Some(Rgba8::BLACK), stroke: None, stroke_width: 1 }
    }
}

impl ShapePath {
    pub fn new(fill: Option<Rgba8>, stroke: Option<Rgba8>) -> Self {
        Self { fill, stroke, ..Default::default() }
    }

    /// Pixel bounds of the points, with the outline's reach.
    pub fn bounds(&self) -> Option<IRect> {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for p in &self.points {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        if x0 > x1 {
            return None;
        }
        let r = if self.stroke.is_some() { self.stroke_width as i32 / 2 + 1 } else { 0 };
        Some(IRect::new(x0 - r, y0 - r, x1 - x0 + 1 + 2 * r, y1 - y0 + 1 + 2 * r))
    }

    /// Index of the point within `dist` pixels of `(x, y)`, nearest first.
    pub fn point_near(&self, x: f32, y: f32, dist: f32) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, p) in self.points.iter().enumerate() {
            let (dx, dy) = (p[0] as f32 + 0.5 - x, p[1] as f32 + 0.5 - y);
            let d = (dx * dx + dy * dy).sqrt();
            if d <= dist && best.is_none_or(|(_, b)| d < b) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Index of the edge (from point `i` to `i + 1`) within `dist` pixels of
    /// `(x, y)`, for inserting a point on it.
    pub fn edge_near(&self, x: f32, y: f32, dist: f32) -> Option<usize> {
        let n = self.points.len();
        if n < 2 {
            return None;
        }
        let edges = if self.closed { n } else { n - 1 };
        let mut best: Option<(usize, f32)> = None;
        for i in 0..edges {
            let a = self.points[i];
            let b = self.points[(i + 1) % n];
            let (ax, ay) = (a[0] as f32 + 0.5, a[1] as f32 + 0.5);
            let (bx, by) = (b[0] as f32 + 0.5, b[1] as f32 + 0.5);
            let (vx, vy) = (bx - ax, by - ay);
            let len2 = vx * vx + vy * vy;
            let t = if len2 <= 0.0 { 0.0 } else { (((x - ax) * vx + (y - ay) * vy) / len2).clamp(0.0, 1.0) };
            let (px, py) = (ax + vx * t, ay + vy * t);
            let d = ((x - px).powi(2) + (y - py).powi(2)).sqrt();
            if d <= dist && best.is_none_or(|(_, b)| d < b) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Draw the shape into a blank canvas-sized raster: the fill first (even-odd
    /// through pixel centers), then the outline.
    pub fn render(&self, width: u32, height: u32) -> Raster {
        let mut out = Raster::new(width, height);
        self.render_into(&mut out);
        out
    }

    pub fn render_into(&self, out: &mut Raster) {
        let n = self.points.len();
        let (w, h) = (out.width() as i32, out.height() as i32);
        if let (Some(fill), true) = (self.fill, self.closed && n >= 3) {
            if fill.a > 0 {
                let Some(b) = self.bounds() else { return };
                let y0 = b.y.max(0);
                let y1 = (b.bottom()).min(h);
                let mut xs: Vec<f32> = Vec::new();
                for y in y0..y1 {
                    let yc = y as f32 + 0.5;
                    xs.clear();
                    for i in 0..n {
                        let a = self.points[i];
                        let bb = self.points[(i + 1) % n];
                        let (ax, ay) = (a[0] as f32 + 0.5, a[1] as f32 + 0.5);
                        let (bx, by) = (bb[0] as f32 + 0.5, bb[1] as f32 + 0.5);
                        if (ay <= yc) != (by <= yc) {
                            xs.push(ax + (yc - ay) * (bx - ax) / (by - ay));
                        }
                    }
                    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    for pair in xs.chunks_exact(2) {
                        let xa = (pair[0] - 0.5).ceil() as i32;
                        let xb = (pair[1] - 0.5).floor() as i32;
                        let (xa, xb) = (xa.max(0), xb.min(w - 1));
                        if xa <= xb {
                            out.fill_rect(IRect::new(xa, y, xb - xa + 1, 1), fill);
                        }
                    }
                }
                // The boundary pixels belong to the shape too: trace the
                // edges in the fill color, so a square from (2,2) to (5,5)
                // covers 4×4 pixels, not the 3×3 strictly inside.
                for i in 0..n {
                    let a = self.points[i];
                    let bb = self.points[(i + 1) % n];
                    bresenham(a[0], a[1], bb[0], bb[1], |x, y| {
                        if x >= 0 && y >= 0 && x < w && y < h {
                            out.set_pixel(x, y, fill);
                        }
                    });
                }
            }
        }
        if let Some(stroke) = self.stroke {
            if stroke.a > 0 && self.stroke_width > 0 && n >= 1 {
                let edges = if self.closed && n >= 2 { n } else { n.saturating_sub(1) };
                let sw = self.stroke_width as i32;
                let mut plot = |x: i32, y: i32| {
                    if sw <= 1 {
                        if x >= 0 && y >= 0 && x < w && y < h {
                            out.set_pixel(x, y, stroke);
                        }
                    } else {
                        let o = (sw - 1) / 2;
                        out.fill_rect(IRect::new(x - o, y - o, sw, sw).intersect(&IRect::new(0, 0, w, h)), stroke);
                    }
                };
                if edges == 0 {
                    plot(self.points[0][0], self.points[0][1]);
                }
                for i in 0..edges {
                    let a = self.points[i];
                    let b = self.points[(i + 1) % n];
                    bresenham(a[0], a[1], b[0], b[1], &mut plot);
                }
            }
        }
    }
}

/// Step every pixel of the line from `(x0, y0)` to `(x1, y1)`, inclusive.
pub fn bresenham(x0: i32, y0: i32, x1: i32, y1: i32, mut plot: impl FnMut(i32, i32)) {
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut x, mut y, mut err) = (x0, y0, dx + dy);
    loop {
        plot(x, y);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Redraw a shape layer's pixels from its path (no-op for other layers).
pub fn rerender(doc: &mut DocState, li: usize) {
    let (w, h) = (doc.width, doc.height);
    if let Some(l) = doc.layers.get_mut(li) {
        if let Some(shape) = &l.props.shape {
            l.raster = shape.render(w, h);
        }
    }
}

/// Turn a shape layer into a plain pixel layer, keeping its pixels.
pub fn rasterize(doc: &mut DocState, li: usize) -> bool {
    match doc.layers.get_mut(li) {
        Some(l) if l.props.shape.is_some() => {
            l.props.shape = None;
            l.props.kind = crate::layer::LayerKind::Raster;
            true
        }
        _ => false,
    }
}

/// Turn every shape layer into pixels (before a whole-canvas change the
/// points could not follow).
pub fn detach_all(doc: &mut DocState) {
    for li in 0..doc.layers.len() {
        rasterize(doc, li);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_a_triangle_and_outlines_it_on_the_pixel_grid() {
        let red = Rgba8::new(255, 0, 0, 255);
        let blue = Rgba8::new(0, 0, 255, 255);
        let mut s = ShapePath::new(Some(red), Some(blue));
        s.points = vec![[1, 1], [8, 1], [1, 8]];
        let r = s.render(10, 10);
        assert_eq!(r.get_pixel(1, 1), blue, "corner on the outline");
        assert_eq!(r.get_pixel(4, 1), blue, "top edge");
        assert_eq!(r.get_pixel(2, 2), red, "inside");
        assert_eq!(r.get_pixel(7, 7).a, 0, "outside");
        assert_eq!(r.get_pixel(0, 0).a, 0);
        // No fill when open.
        s.closed = false;
        let r = s.render(10, 10);
        assert_eq!(r.get_pixel(2, 2).a, 0);
        assert_eq!(r.get_pixel(4, 1), blue);
        assert_eq!(r.get_pixel(1, 4).a, 0, "no closing edge");
        // Hit tests.
        assert_eq!(s.point_near(8.4, 1.6, 1.0), Some(1));
        assert_eq!(s.point_near(5.0, 5.0, 1.0), None);
        assert_eq!(s.edge_near(4.5, 1.6, 1.0), Some(0));
    }

    #[test]
    fn square_fill_covers_exactly_its_pixels() {
        let c = Rgba8::new(0, 255, 0, 255);
        let mut s = ShapePath::new(Some(c), None);
        s.points = vec![[2, 2], [5, 2], [5, 5], [2, 5]];
        let r = s.render(8, 8);
        let count = (0..8).flat_map(|y| (0..8).map(move |x| (x, y))).filter(|&(x, y)| r.get_pixel(x, y).a > 0).count();
        // The pixels on and inside the edges: 4×4.
        assert_eq!(count, 16);
        assert_eq!(r.get_pixel(2, 2), c, "a vertex pixel belongs to the shape");
        assert_eq!(r.get_pixel(5, 5), c);
        assert_eq!(r.get_pixel(3, 3), c);
        assert_eq!(r.get_pixel(6, 3).a, 0);
    }
}
