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

    /// Grow the cached bounds to include `r` (for callers that `set` pixels
    /// inside a known rect and don't want a full `recompute_bounds` scan).
    pub fn expand_bounds(&mut self, r: IRect) {
        let r = r.intersect(&self.rect());
        if r.is_empty() {
            return;
        }
        self.bounds = if self.bounds.is_empty() { r } else { self.bounds.union(&r) };
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

    /// Signed distance from the selection edge over `region`, in pixels:
    /// negative inside, positive outside, zero on the boundary. Coverage of
    /// 128 or more counts as inside, so a feathered edge is treated as the
    /// shape it outlines.
    fn signed_distance(&self, region: IRect) -> Vec<f32> {
        let (w, h) = (region.w as usize, region.h as usize);
        let inside: Vec<bool> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| self.get(region.x + x as i32, region.y + y as i32) >= 128)
            .collect();
        let far = f32::MAX / 4.0;
        // Distance to the nearest selected pixel, and to the nearest
        // unselected one; their difference is the signed distance.
        let mut out: Vec<f32> = inside.iter().map(|&i| if i { 0.0 } else { far }).collect();
        let mut into: Vec<f32> = inside.iter().map(|&i| if i { far } else { 0.0 }).collect();
        edt(&mut out, w, h);
        edt(&mut into, w, h);
        out.iter().zip(&into).map(|(&a, &b)| a.sqrt() - b.sqrt()).collect()
    }

    /// The region a distance-based edit has to touch: the current bounds plus
    /// the radius, clipped to the mask.
    fn edit_region(&self, radius: f32) -> IRect {
        self.bounds.expand(radius.abs().ceil() as i32 + 2).intersect(&self.rect())
    }

    /// Grow the selection by `px` pixels in every direction (Photoshop's
    /// Select ▸ Modify ▸ Expand). A negative amount contracts it.
    pub fn expanded(&self, px: f32) -> Mask {
        if px == 0.0 || self.is_empty() {
            return self.clone();
        }
        let region = self.edit_region(px + 1.0);
        if region.is_empty() {
            return self.clone();
        }
        let dist = self.signed_distance(region);
        let mut m = Mask::new(self.width, self.height);
        for (i, d) in dist.iter().enumerate() {
            let (x, y) = (region.x + (i % region.w as usize) as i32, region.y + (i / region.w as usize) as i32);
            // Half a pixel of feathering across the new edge, as the shape
            // tools do, so an expanded circle stays a circle.
            let a = (0.5 + px - d).clamp(0.0, 1.0);
            if a > 0.0 {
                m.set(x, y, (a * 255.0 + 0.5) as u8);
            }
        }
        m.recompute_bounds();
        m
    }

    /// Shrink the selection by `px` pixels (Select ▸ Modify ▸ Contract).
    pub fn contracted(&self, px: f32) -> Mask {
        self.expanded(-px)
    }

    /// Keep only a band `px` wide centered on the selection's edge
    /// (Select ▸ Modify ▸ Border).
    pub fn bordered(&self, px: f32) -> Mask {
        if px <= 0.0 || self.is_empty() {
            return Mask::new(self.width, self.height);
        }
        let half = px / 2.0;
        let region = self.edit_region(half + 1.0);
        let dist = self.signed_distance(region);
        let mut m = Mask::new(self.width, self.height);
        for (i, d) in dist.iter().enumerate() {
            let (x, y) = (region.x + (i % region.w as usize) as i32, region.y + (i / region.w as usize) as i32);
            let a = (0.5 + half - d.abs()).clamp(0.0, 1.0);
            if a > 0.0 {
                m.set(x, y, (a * 255.0 + 0.5) as u8);
            }
        }
        m.recompute_bounds();
        m
    }

    /// Round off corners and drop specks: a pixel ends up selected when most
    /// of the pixels within `radius` are (Select ▸ Modify ▸ Smooth).
    pub fn smoothed(&self, radius: u32) -> Mask {
        if radius == 0 || self.is_empty() {
            return self.clone();
        }
        let r = radius as i32;
        let region = self.edit_region(r as f32);
        // Offsets inside the disc, so corners round rather than square off.
        let disc: Vec<(i32, i32)> = (-r..=r)
            .flat_map(|dy| (-r..=r).map(move |dx| (dx, dy)))
            .filter(|(dx, dy)| dx * dx + dy * dy <= r * r)
            .collect();
        let mut m = Mask::new(self.width, self.height);
        for y in region.y..region.bottom() {
            for x in region.x..region.right() {
                let hits = disc.iter().filter(|(dx, dy)| self.get(x + dx, y + dy) >= 128).count();
                if hits * 2 > disc.len() {
                    m.set(x, y, 255);
                }
            }
        }
        m.recompute_bounds();
        m
    }

    /// Make every pixel fully selected or not at all, dropping feathered and
    /// anti-aliased edges (GIMP's Select ▸ Sharpen).
    pub fn sharpened(&self) -> Mask {
        let data = self.data.iter().map(|&v| if v >= 128 { 255 } else { 0 }).collect();
        let mut m = Mask { width: self.width, height: self.height, data, bounds: IRect::EMPTY };
        m.recompute_bounds();
        m
    }

    /// Select the unselected pockets fully enclosed by the selection
    /// (GIMP's Select ▸ Remove Holes).
    pub fn without_holes(&self) -> Mask {
        if self.is_empty() {
            return self.clone();
        }
        let (w, h) = (self.width as usize, self.height as usize);
        // Flood the unselected area inward from the edges of the image; what
        // it never reaches is enclosed.
        let mut outside = vec![false; w * h];
        let mut stack: Vec<(i32, i32)> = Vec::new();
        let push = |x: i32, y: i32, outside: &mut Vec<bool>, stack: &mut Vec<(i32, i32)>| {
            if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
                return;
            }
            let i = y as usize * w + x as usize;
            if outside[i] || self.get(x, y) >= 128 {
                return;
            }
            outside[i] = true;
            stack.push((x, y));
        };
        for x in 0..self.width as i32 {
            push(x, 0, &mut outside, &mut stack);
            push(x, self.height as i32 - 1, &mut outside, &mut stack);
        }
        for y in 0..self.height as i32 {
            push(0, y, &mut outside, &mut stack);
            push(self.width as i32 - 1, y, &mut outside, &mut stack);
        }
        while let Some((x, y)) = stack.pop() {
            push(x + 1, y, &mut outside, &mut stack);
            push(x - 1, y, &mut outside, &mut stack);
            push(x, y + 1, &mut outside, &mut stack);
            push(x, y - 1, &mut outside, &mut stack);
        }
        let mut m = self.clone();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if !outside[i] && m.data[i] < 255 {
                    m.data[i] = 255;
                }
            }
        }
        m.recompute_bounds();
        m
    }

    /// Photoshop-style feather: a Gaussian blur of the coverage with
    /// `radius` px (sigma = radius / 2, so the edge fades over about
    /// ±radius). Only the neighbourhood of the current bounds is touched.
    pub fn feathered(&self, radius: f32) -> Mask {
        use rayon::prelude::*;
        if radius <= 0.0 || self.is_empty() {
            return self.clone();
        }
        let sigma = radius * 0.5;
        let k = (sigma * 3.0).ceil() as i32;
        let kernel: Vec<f32> = (-k..=k).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
        let norm: f32 = kernel.iter().sum();
        let kernel: Vec<f32> = kernel.into_iter().map(|v| v / norm).collect();
        let region = self.bounds.expand(k).intersect(&self.rect());
        let (rw, rh) = (region.w as usize, region.h as usize);
        let w = self.width as usize;
        // Horizontal pass into a float buffer covering the region.
        let mut tmp = vec![0.0f32; rw * rh];
        tmp.par_chunks_mut(rw).enumerate().for_each(|(ry, row)| {
            let y = region.y as usize + ry;
            let src = &self.data[y * w..(y + 1) * w];
            for (rx, out) in row.iter_mut().enumerate() {
                let x = region.x + rx as i32;
                let mut acc = 0.0;
                for (i, kv) in kernel.iter().enumerate() {
                    let sx = x + i as i32 - k;
                    if sx >= 0 && (sx as usize) < w {
                        acc += src[sx as usize] as f32 * kv;
                    }
                }
                *out = acc;
            }
        });
        // Vertical pass back into bytes.
        let mut m = self.clone();
        let mut out_rows: Vec<Vec<u8>> = (0..rh)
            .into_par_iter()
            .map(|ry| {
                let mut row = vec![0u8; rw];
                for (rx, o) in row.iter_mut().enumerate() {
                    let mut acc = 0.0;
                    for (i, kv) in kernel.iter().enumerate() {
                        let sy = ry as i32 + i as i32 - k;
                        if sy >= 0 && (sy as usize) < rh {
                            acc += tmp[sy as usize * rw + rx] * kv;
                        }
                    }
                    *o = (acc + 0.5).clamp(0.0, 255.0) as u8;
                }
                row
            })
            .collect();
        for (ry, row) in out_rows.drain(..).enumerate() {
            let y = region.y as usize + ry;
            let x0 = region.x as usize;
            m.data[y * w + x0..y * w + x0 + rw].copy_from_slice(&row);
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

/// Exact squared Euclidean distance transform, in place
/// (Felzenszwalb & Huttenlocher 2012). Seeds hold 0, everything else a large
/// value; afterwards each cell holds the squared distance to the nearest seed.
fn edt(f: &mut [f32], w: usize, h: usize) {
    if w == 0 || h == 0 {
        return;
    }
    let mut col = vec![0.0f32; h.max(w)];
    // Columns, then rows: the 1-D transform is separable.
    for x in 0..w {
        for y in 0..h {
            col[y] = f[y * w + x];
        }
        dt_1d(&mut col[..h]);
        for y in 0..h {
            f[y * w + x] = col[y];
        }
    }
    for y in 0..h {
        col[..w].copy_from_slice(&f[y * w..y * w + w]);
        dt_1d(&mut col[..w]);
        f[y * w..y * w + w].copy_from_slice(&col[..w]);
    }
}

/// One-dimensional squared distance transform of a sampled function.
fn dt_1d(f: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut d = vec![0.0f32; n];
    // v: parabola vertices, z: the boundaries between them.
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f32; n + 1];
    let mut k = 0usize;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32);
            if s <= z[k] {
                if k == 0 {
                    k = usize::MAX; // signals "replace the first parabola"
                    break;
                }
                k -= 1;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f32::INFINITY;
                break;
            }
        }
        if k == usize::MAX {
            k = 0;
            v[0] = q;
            z[0] = f32::NEG_INFINITY;
            z[1] = f32::INFINITY;
        }
    }
    let mut k = 0usize;
    for (q, out) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        let dx = q as f32 - p as f32;
        *out = dx * dx + f[p];
    }
    f.copy_from_slice(&d);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expanding a single pixel gives a disc of that radius, not a square:
    /// the distance field has to be Euclidean.
    #[test]
    fn expand_is_round_and_exact() {
        let mut m = Mask::new(41, 41);
        m.set(20, 20, 255);
        m.recompute_bounds();
        let e = m.expanded(8.0);
        // The edge lands on the radius, half covered, as an anti-aliased
        // circle would; a pixel inside it is solid and one past it is clear.
        assert_eq!(e.get(27, 20), 255, "7 px right is solid");
        assert_eq!(e.get(28, 20), 128, "the edge at 8 px is half covered");
        assert_eq!(e.get(30, 20), 0, "10 px right is outside");
        assert_eq!(e.get(20, 12), 128, "same distance upward, same coverage");
        // A square dilation would still be solid 8 px out diagonally.
        assert_eq!(e.get(27, 27), 0, "the corner of the bounding square is not selected");
        assert_eq!(e.get(25, 25), 255, "inside the diagonal radius is");
        assert_eq!(e.bounds().w, 17, "bounds grew by the radius on both sides");
    }

    #[test]
    fn contract_border_and_smooth() {
        let m = Mask::from_rect(64, 64, IRect::new(16, 16, 32, 32));
        let c = m.contracted(4.0);
        assert_eq!(c.get(32, 32), 255, "middle stays selected");
        assert_eq!(c.get(17, 32), 0, "the old edge is gone");
        assert_eq!(c.get(21, 32), 255, "4 px in is still selected");
        // Contracting past the shape empties the selection.
        assert!(m.contracted(40.0).is_empty());

        // Border keeps a band centered on the edge and drops the middle.
        let b = m.bordered(6.0);
        assert_eq!(b.get(32, 32), 0, "the interior is not in the border");
        assert_eq!(b.get(16, 32), 255, "the edge is");
        assert_eq!(b.get(14, 32), 255, "and so is just outside it");
        assert_eq!(b.get(10, 32), 0, "but not far outside");

        // Smooth removes a lone speck and fills a lone gap.
        let mut speckled = m.clone();
        speckled.set(2, 2, 255);
        speckled.set(32, 32, 0);
        speckled.recompute_bounds();
        let sm = speckled.smoothed(3);
        assert_eq!(sm.get(2, 2), 0, "speck removed");
        assert_eq!(sm.get(32, 32), 255, "pinhole filled");
        assert_eq!(sm.get(32, 20), 255, "the shape itself survives");
    }

    #[test]
    fn sharpen_and_remove_holes() {
        let mut m = Mask::new(32, 32);
        m.set(5, 5, 100);
        m.set(6, 5, 200);
        m.recompute_bounds();
        let s = m.sharpened();
        assert_eq!(s.get(5, 5), 0, "under half coverage drops out");
        assert_eq!(s.get(6, 5), 255, "over half becomes solid");

        // A ring: the hole in the middle is enclosed, the outside is not.
        let ring = Mask::from_rect(32, 32, IRect::new(8, 8, 16, 16)).subtract(&Mask::from_rect(
            32,
            32,
            IRect::new(12, 12, 8, 8),
        ));
        assert_eq!(ring.get(16, 16), 0);
        let filled = ring.without_holes();
        assert_eq!(filled.get(16, 16), 255, "hole filled");
        assert_eq!(filled.get(1, 1), 0, "outside untouched");
    }

    #[test]
    fn feather_softens_edges_symmetrically() {
        let m = Mask::from_rect(64, 64, IRect::new(16, 16, 32, 32));
        assert_eq!(m.feathered(0.0).data(), m.data());
        let f = m.feathered(6.0);
        assert_eq!(f.get(32, 32), 255, "deep inside stays fully selected");
        assert_eq!(f.get(2, 2), 0, "far outside stays unselected");
        let inside = f.get(20, 32);
        let edge_in = f.get(15, 32);
        let edge_out = f.get(16, 32);
        assert!(inside > edge_out && edge_out > 60 && edge_out < 200, "{inside} {edge_out}");
        // Symmetric falloff across the edge (pixel centers at 15.5 and 16.5).
        assert!((edge_in as i32 + edge_out as i32 - 255).abs() <= 2, "{edge_in} + {edge_out}");
        assert!(f.bounds().w > m.bounds().w, "bounds grow with the feather");
    }

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
