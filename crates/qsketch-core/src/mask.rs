//! Selection masks: one coverage byte per document pixel, plus boolean ops,
//! shape/flood constructors and marching-ants outline extraction.

use crate::color::Rgba8;
use crate::geom::{IRect, Pt};

#[derive(Clone)]
pub struct Mask {
    width: u32,
    height: u32,
    data: Vec<u8>,
    /// Bounding box of non-zero coverage (empty rect if none).
    bounds: IRect,
}

/// How a new selection combines with the existing one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum SelectionOp {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}

impl Mask {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, data: vec![0; (width * height) as usize], bounds: IRect::EMPTY }
    }

    pub fn full(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![255; (width * height) as usize],
            bounds: IRect::new(0, 0, width as i32, height as i32),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn rect(&self) -> IRect {
        IRect::new(0, 0, self.width as i32, self.height as i32)
    }
    pub fn bounds(&self) -> IRect {
        self.bounds
    }
    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// True if every pixel is fully selected.
    pub fn is_full(&self) -> bool {
        self.data.iter().all(|&v| v == 255)
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            0
        } else {
            self.data[(y as u32 * self.width + x as u32) as usize]
        }
    }

    #[inline]
    pub fn coverage(&self, x: i32, y: i32) -> f32 {
        self.get(x, y) as f32 / 255.0
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, v: u8) {
        if x >= 0 && y >= 0 && x < self.width as i32 && y < self.height as i32 {
            self.data[(y as u32 * self.width + x as u32) as usize] = v;
        }
    }

    pub fn recompute_bounds(&mut self) {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for y in 0..self.height as i32 {
            let row = &self.data[(y as u32 * self.width) as usize..][..self.width as usize];
            if let Some(first) = row.iter().position(|&v| v != 0) {
                let last = row.iter().rposition(|&v| v != 0).unwrap();
                x0 = x0.min(first as i32);
                x1 = x1.max(last as i32);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
        self.bounds = if x0 <= x1 { IRect::from_corners(x0, y0, x1, y1) } else { IRect::EMPTY };
    }

    pub fn from_rect(width: u32, height: u32, rect: IRect) -> Self {
        let mut m = Self::new(width, height);
        let r = rect.intersect(&m.rect());
        for y in r.y..r.bottom() {
            let row = (y as u32 * width) as usize;
            m.data[row + r.x as usize..row + r.right() as usize].fill(255);
        }
        m.bounds = r;
        m
    }

    /// Anti-aliased ellipse inscribed in `rect`.
    pub fn from_ellipse(width: u32, height: u32, rect: IRect) -> Self {
        let mut m = Self::new(width, height);
        if rect.is_empty() {
            return m;
        }
        let cx = rect.x as f32 + rect.w as f32 / 2.0;
        let cy = rect.y as f32 + rect.h as f32 / 2.0;
        let rx = rect.w as f32 / 2.0;
        let ry = rect.h as f32 / 2.0;
        let r = rect.intersect(&m.rect());
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                // 4x4 supersample for smooth edges.
                let mut inside = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                        let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                        let dx = (px - cx) / rx;
                        let dy = (py - cy) / ry;
                        if dx * dx + dy * dy <= 1.0 {
                            inside += 1;
                        }
                    }
                }
                if inside > 0 {
                    m.set(x, y, (inside * 255 / 16) as u8);
                }
            }
        }
        m.recompute_bounds();
        m
    }

    /// Polygon (lasso) fill using the even-odd scanline rule, anti-aliased
    /// vertically with 4 sub-scanlines.
    pub fn from_polygon(width: u32, height: u32, pts: &[Pt]) -> Self {
        Self::from_polygon_rule(width, height, pts, false)
    }

    /// Polygon fill; `nonzero` selects the non-zero winding rule (a freehand
    /// contour that crosses itself still fills its loops), otherwise even-odd.
    pub fn from_polygon_rule(width: u32, height: u32, pts: &[Pt], nonzero: bool) -> Self {
        let mut m = Self::new(width, height);
        if pts.len() < 3 {
            return m;
        }
        let min_y = pts.iter().map(|p| p.y).fold(f32::MAX, f32::min).floor().max(0.0) as i32;
        let max_y = pts.iter().map(|p| p.y).fold(f32::MIN, f32::max).ceil().min(height as f32) as i32;
        let n = pts.len();
        // Crossings as (x, winding direction).
        let mut xs: Vec<(f32, i32)> = Vec::new();
        let mut spans: Vec<(f32, f32)> = Vec::new();
        let mut acc = vec![0f32; width as usize];
        const SUB: usize = 4;
        for y in min_y..max_y {
            acc.fill(0.0);
            for s in 0..SUB {
                let sy = y as f32 + (s as f32 + 0.5) / SUB as f32;
                xs.clear();
                for i in 0..n {
                    let a = pts[i];
                    let b = pts[(i + 1) % n];
                    if (a.y <= sy && b.y > sy) || (b.y <= sy && a.y > sy) {
                        let t = (sy - a.y) / (b.y - a.y);
                        xs.push((a.x + (b.x - a.x) * t, if b.y > a.y { 1 } else { -1 }));
                    }
                }
                xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                spans.clear();
                if nonzero {
                    let mut wind = 0;
                    for w in xs.windows(2) {
                        wind += w[0].1;
                        if wind != 0 {
                            spans.push((w[0].0, w[1].0));
                        }
                    }
                } else {
                    for pair in xs.chunks_exact(2) {
                        spans.push((pair[0].0, pair[1].0));
                    }
                }
                for &(sx0, sx1) in &spans {
                    let x0 = sx0.max(0.0);
                    let x1 = sx1.min(width as f32);
                    if x1 <= x0 {
                        continue;
                    }
                    // Horizontal coverage with fractional ends.
                    let ix0 = x0.floor() as usize;
                    let ix1 = (x1.ceil() as usize).min(width as usize);
                    for (x, a) in acc.iter_mut().enumerate().take(ix1).skip(ix0) {
                        let l = x0.max(x as f32);
                        let r = x1.min(x as f32 + 1.0);
                        let cov = (r - l).max(0.0);
                        *a += cov * 255.0 / SUB as f32;
                    }
                }
            }
            let row = (y as u32 * width) as usize;
            for (x, a) in acc.iter().enumerate() {
                if *a > 0.0 {
                    m.data[row + x] = (*a + 0.5).min(255.0) as u8;
                }
            }
        }
        m.recompute_bounds();
        m
    }

    /// Flood/magic-wand region from a seed pixel. `sample` returns the color at
    /// any coordinate (transparent outside). `tolerance` is 0..=255 max channel
    /// difference. Non-contiguous selects every matching pixel.
    pub fn from_flood(
        width: u32,
        height: u32,
        seed: (i32, i32),
        tolerance: u8,
        contiguous: bool,
        sample: &dyn Fn(i32, i32) -> Rgba8,
    ) -> Self {
        let mut m = Self::new(width, height);
        let (sx, sy) = seed;
        if sx < 0 || sy < 0 || sx >= width as i32 || sy >= height as i32 {
            return m;
        }
        let target = sample(sx, sy);
        let matches = |x: i32, y: i32| sample(x, y).max_channel_diff(target) <= tolerance;
        if !contiguous {
            for y in 0..height as i32 {
                for x in 0..width as i32 {
                    if matches(x, y) {
                        m.set(x, y, 255);
                    }
                }
            }
            m.recompute_bounds();
            return m;
        }
        // Scanline flood fill.
        let w = width as i32;
        let h = height as i32;
        let mut stack = vec![(sx, sy)];
        while let Some((x, y)) = stack.pop() {
            if m.get(x, y) != 0 || !matches(x, y) {
                continue;
            }
            let mut x0 = x;
            while x0 > 0 && m.get(x0 - 1, y) == 0 && matches(x0 - 1, y) {
                x0 -= 1;
            }
            let mut x1 = x;
            while x1 + 1 < w && m.get(x1 + 1, y) == 0 && matches(x1 + 1, y) {
                x1 += 1;
            }
            for xx in x0..=x1 {
                m.set(xx, y, 255);
            }
            for ny in [y - 1, y + 1] {
                if ny < 0 || ny >= h {
                    continue;
                }
                let mut xx = x0;
                while xx <= x1 {
                    if m.get(xx, ny) == 0 && matches(xx, ny) {
                        stack.push((xx, ny));
                        while xx <= x1 && m.get(xx, ny) == 0 && matches(xx, ny) {
                            xx += 1;
                        }
                    } else {
                        xx += 1;
                    }
                }
            }
        }
        m.recompute_bounds();
        m
    }

    pub fn invert(&self) -> Self {
        let mut m = self.clone();
        for v in &mut m.data {
            *v = 255 - *v;
        }
        m.recompute_bounds();
        m
    }

    pub fn union(&self, o: &Mask) -> Self {
        let mut m = self.clone();
        for (a, &b) in m.data.iter_mut().zip(&o.data) {
            *a = (*a).max(b);
        }
        m.bounds = self.bounds.union(&o.bounds);
        m
    }

    pub fn subtract(&self, o: &Mask) -> Self {
        let mut m = self.clone();
        for (a, &b) in m.data.iter_mut().zip(&o.data) {
            *a = a.saturating_sub(b);
        }
        m.recompute_bounds();
        m
    }

    pub fn intersect(&self, o: &Mask) -> Self {
        let mut m = self.clone();
        for (a, &b) in m.data.iter_mut().zip(&o.data) {
            *a = (*a).min(b);
        }
        m.recompute_bounds();
        m
    }

    /// Combine `new` into `self` according to `op`.
    pub fn combine(&self, new: &Mask, op: SelectionOp) -> Self {
        match op {
            SelectionOp::Replace => new.clone(),
            SelectionOp::Add => self.union(new),
            SelectionOp::Subtract => self.subtract(new),
            SelectionOp::Intersect => self.intersect(new),
        }
    }

    pub fn translated(&self, dx: i32, dy: i32) -> Self {
        let mut m = Self::new(self.width, self.height);
        let b = self.bounds;
        for y in b.y..b.bottom() {
            for x in b.x..b.right() {
                let v = self.get(x, y);
                if v != 0 {
                    m.set(x + dx, y + dy, v);
                }
            }
        }
        m.recompute_bounds();
        m
    }

    /// Mirror the whole mask left-right.
    pub fn flipped_h(&self) -> Self {
        let mut m = Self::new(self.width, self.height);
        let w = self.width as i32;
        for y in 0..self.height as i32 {
            for x in 0..w {
                m.set(w - 1 - x, y, self.get(x, y));
            }
        }
        m.recompute_bounds();
        m
    }

    /// Mirror the whole mask top-bottom.
    pub fn flipped_v(&self) -> Self {
        let mut m = Self::new(self.width, self.height);
        let h = self.height as i32;
        for y in 0..h {
            for x in 0..self.width as i32 {
                m.set(x, h - 1 - y, self.get(x, y));
            }
        }
        m.recompute_bounds();
        m
    }

    /// Rotate by `times` × 90° clockwise (the canvas swaps dimensions for odd `times`).
    pub fn rotated(&self, times: u32) -> Self {
        let times = times % 4;
        if times == 0 {
            return self.clone();
        }
        let (w, h) = (self.width as i32, self.height as i32);
        let (nw, nh) = if times % 2 == 1 { (h, w) } else { (w, h) };
        let mut m = Self::new(nw as u32, nh as u32);
        for y in 0..h {
            for x in 0..w {
                let v = self.get(x, y);
                if v == 0 {
                    continue;
                }
                let (nx, ny) = match times {
                    1 => (h - 1 - y, x),
                    2 => (w - 1 - x, h - 1 - y),
                    _ => (y, w - 1 - x),
                };
                m.set(nx, ny, v);
            }
        }
        m.recompute_bounds();
        m
    }

    /// Resize the mask canvas, keeping content at offset.
    pub fn with_canvas_size(&self, new_w: u32, new_h: u32, ox: i32, oy: i32) -> Self {
        let mut m = Self::new(new_w, new_h);
        let b = self.bounds;
        for y in b.y..b.bottom() {
            for x in b.x..b.right() {
                let v = self.get(x, y);
                if v != 0 {
                    m.set(x + ox, y + oy, v);
                }
            }
        }
        m.recompute_bounds();
        m
    }

    /// Outline segments (pixel-edge coordinates) between selected (>127) and
    /// unselected pixels, for drawing marching ants. Each segment is
    /// `[start, end]` in document pixel space.
    pub fn outline_segments(&self) -> Vec<[Pt; 2]> {
        let mut segs = Vec::new();
        let b = self.bounds.expand(1).intersect(&self.rect().expand(1));
        let sel = |x: i32, y: i32| self.get(x, y) > 127;
        for y in b.y..b.bottom() {
            for x in b.x..b.right() {
                let s = sel(x, y);
                // Top edge
                if s != sel(x, y - 1) {
                    segs.push([Pt::new(x as f32, y as f32), Pt::new(x as f32 + 1.0, y as f32)]);
                }
                // Left edge
                if s != sel(x - 1, y) {
                    segs.push([Pt::new(x as f32, y as f32), Pt::new(x as f32, y as f32 + 1.0)]);
                }
            }
        }
        segs
    }

    /// Raw coverage bytes (0 = unselected, 255 = fully selected), for saving.
    pub fn to_gray(&self) -> &[u8] {
        &self.data
    }

    pub fn from_gray(width: u32, height: u32, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), (width * height) as usize);
        let mut m = Self { width, height, data, bounds: IRect::EMPTY };
        m.recompute_bounds();
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_ops() {
        let a = Mask::from_rect(10, 10, IRect::new(0, 0, 5, 5));
        let b = Mask::from_rect(10, 10, IRect::new(3, 3, 5, 5));
        assert_eq!(a.union(&b).bounds(), IRect::new(0, 0, 8, 8));
        assert_eq!(a.intersect(&b).bounds(), IRect::new(3, 3, 2, 2));
        assert_eq!(a.subtract(&b).get(4, 4), 0);
        assert_eq!(a.subtract(&b).get(0, 0), 255);
        assert_eq!(a.invert().get(0, 0), 0);
        assert!(!a.is_empty());
        assert!(Mask::new(4, 4).is_empty());
        assert!(Mask::full(4, 4).is_full());
    }

    #[test]
    fn polygon_and_ellipse() {
        let tri = Mask::from_polygon(20, 20, &[Pt::new(2.0, 2.0), Pt::new(18.0, 2.0), Pt::new(2.0, 18.0)]);
        assert_eq!(tri.get(3, 3), 255);
        assert_eq!(tri.get(17, 17), 0);
        let e = Mask::from_ellipse(20, 20, IRect::new(0, 0, 20, 20));
        assert_eq!(e.get(10, 10), 255);
        assert_eq!(e.get(0, 0), 0);
    }

    #[test]
    fn polygon_winding_rules() {
        // A pentagram: even-odd leaves the center empty, non-zero fills it.
        let star: Vec<Pt> = (0..5)
            .map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + i as f32 * 4.0 * std::f32::consts::PI / 5.0;
                Pt::new(20.0 + 18.0 * a.cos(), 20.0 + 18.0 * a.sin())
            })
            .collect();
        let eo = Mask::from_polygon(40, 40, &star);
        let nz = Mask::from_polygon_rule(40, 40, &star, true);
        assert_eq!(eo.get(20, 20), 0);
        assert_eq!(nz.get(20, 20), 255);
        // The points of the star fill under both rules.
        assert!(eo.get(20, 4) > 0 && nz.get(20, 4) > 0);
    }

    #[test]
    fn flood() {
        let colors = |x: i32, y: i32| if x < 5 && y < 5 { Rgba8::WHITE } else { Rgba8::BLACK };
        let m = Mask::from_flood(10, 10, (1, 1), 0, true, &colors);
        assert_eq!(m.bounds(), IRect::new(0, 0, 5, 5));
        let m = Mask::from_flood(10, 10, (7, 7), 0, true, &colors);
        assert_eq!(m.get(9, 0), 255);
        assert_eq!(m.get(0, 0), 0);
    }

    #[test]
    fn outline() {
        let a = Mask::from_rect(10, 10, IRect::new(2, 2, 3, 3));
        // 3x3 square: 12 unit edges
        assert_eq!(a.outline_segments().len(), 12);
    }
}
