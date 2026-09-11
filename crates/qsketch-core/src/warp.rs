//! Free-transform geometry: projective quads and warp meshes, plus the
//! inverse-mapped resampler that renders a raster (or selection mask) through
//! them. Used by the app's Free Transform (Ctrl+T) and floating paste.

use rayon::prelude::*;

use crate::raster::ResizeFilter;
use crate::{IRect, Mask, Pt, Raster};

/// Four corners in the order top-left, top-right, bottom-right, bottom-left.
pub type Quad = [Pt; 4];

/// A 3×3 projective transform, row-major.
#[derive(Clone, Copy, Debug)]
pub struct Homography(pub [f32; 9]);

impl Homography {
    /// Maps the unit square (0,0)-(1,1) onto `q` (Heckbert's formulation).
    pub fn unit_to_quad(q: Quad) -> Option<Self> {
        let [p0, p1, p2, p3] = q;
        let sx = p0.x - p1.x + p2.x - p3.x;
        let sy = p0.y - p1.y + p2.y - p3.y;
        let (a, b, c, d, e, f, g, h);
        if sx.abs() < 1e-6 && sy.abs() < 1e-6 {
            // Affine (parallelogram).
            a = p1.x - p0.x;
            b = p3.x - p0.x;
            c = p0.x;
            d = p1.y - p0.y;
            e = p3.y - p0.y;
            f = p0.y;
            g = 0.0;
            h = 0.0;
        } else {
            let dx1 = p1.x - p2.x;
            let dx2 = p3.x - p2.x;
            let dy1 = p1.y - p2.y;
            let dy2 = p3.y - p2.y;
            let den = dx1 * dy2 - dx2 * dy1;
            if den.abs() < 1e-9 {
                return None;
            }
            g = (sx * dy2 - dx2 * sy) / den;
            h = (dx1 * sy - sx * dy1) / den;
            a = p1.x - p0.x + g * p1.x;
            b = p3.x - p0.x + h * p3.x;
            c = p0.x;
            d = p1.y - p0.y + g * p1.y;
            e = p3.y - p0.y + h * p3.y;
            f = p0.y;
        }
        Some(Self([a, b, c, d, e, f, g, h, 1.0]))
    }

    pub fn inverse(&self) -> Option<Self> {
        let m = &self.0;
        let (a, b, c, d, e, f, g, h, i) = (m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8]);
        let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Self([
            (e * i - f * h) * inv,
            (c * h - b * i) * inv,
            (b * f - c * e) * inv,
            (f * g - d * i) * inv,
            (a * i - c * g) * inv,
            (c * d - a * f) * inv,
            (d * h - e * g) * inv,
            (b * g - a * h) * inv,
            (a * e - b * d) * inv,
        ]))
    }

    pub fn apply(&self, p: Pt) -> Pt {
        let m = &self.0;
        let w = m[6] * p.x + m[7] * p.y + m[8];
        let w = if w.abs() < 1e-12 { 1e-12 } else { w };
        Pt::new((m[0] * p.x + m[1] * p.y + m[2]) / w, (m[3] * p.x + m[4] * p.y + m[5]) / w)
    }
}

/// A lattice of `(cols+1) × (rows+1)` control points in document space; each
/// cell maps a uniform sub-rectangle of the source projectively.
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub cols: u32,
    pub rows: u32,
    /// Row-major, `rows+1` rows of `cols+1` points.
    pub pts: Vec<Pt>,
}

impl Mesh {
    /// Lattice sampled from the projective map of `quad`.
    pub fn from_quad(quad: Quad, cols: u32, rows: u32) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let h = Homography::unit_to_quad(quad).unwrap_or(Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]));
        let mut pts = Vec::with_capacity(((cols + 1) * (rows + 1)) as usize);
        for j in 0..=rows {
            for i in 0..=cols {
                pts.push(h.apply(Pt::new(i as f32 / cols as f32, j as f32 / rows as f32)));
            }
        }
        Self { cols, rows, pts }
    }

    pub fn point(&self, i: u32, j: u32) -> Pt {
        self.pts[(j * (self.cols + 1) + i) as usize]
    }

    /// The outer corners (tl, tr, br, bl).
    pub fn corners(&self) -> Quad {
        [self.point(0, 0), self.point(self.cols, 0), self.point(self.cols, self.rows), self.point(0, self.rows)]
    }

    pub fn cell(&self, i: u32, j: u32) -> Quad {
        [self.point(i, j), self.point(i + 1, j), self.point(i + 1, j + 1), self.point(i, j + 1)]
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        for p in &mut self.pts {
            p.x += dx;
            p.y += dy;
        }
    }

    /// Integer bounding box of all control points.
    pub fn bounds(&self) -> IRect {
        let mut x0 = f32::MAX;
        let mut y0 = f32::MAX;
        let mut x1 = f32::MIN;
        let mut y1 = f32::MIN;
        for p in &self.pts {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        if x0 > x1 {
            return IRect::EMPTY;
        }
        IRect::from_f32_bounds(x0, y0, x1, y1)
    }
}

/// Bilinear sample of a premultiplied f32 RGBA buffer at source pixel coords
/// (continuous space, pixel centers at +0.5). Clamps to the edge.
#[inline]
fn sample(src: &[[f32; 4]], sw: usize, sh: usize, x: f32, y: f32, filter: ResizeFilter) -> [f32; 4] {
    match filter {
        ResizeFilter::Nearest => {
            let xi = (x.floor() as i64).clamp(0, sw as i64 - 1) as usize;
            let yi = (y.floor() as i64).clamp(0, sh as i64 - 1) as usize;
            src[yi * sw + xi]
        }
        ResizeFilter::Bilinear => {
            let fx = (x - 0.5).clamp(0.0, (sw - 1) as f32);
            let fy = (y - 0.5).clamp(0.0, (sh - 1) as f32);
            let x0 = fx as usize;
            let y0 = fy as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let y1 = (y0 + 1).min(sh - 1);
            let tx = fx - x0 as f32;
            let ty = fy - y0 as f32;
            let (p00, p10, p01, p11) = (src[y0 * sw + x0], src[y0 * sw + x1], src[y1 * sw + x0], src[y1 * sw + x1]);
            let mut r = [0f32; 4];
            for c in 0..4 {
                let top = p00[c] + (p10[c] - p00[c]) * tx;
                let bot = p01[c] + (p11[c] - p01[c]) * tx;
                r[c] = top + (bot - top) * ty;
            }
            r
        }
    }
}

/// Render straight-alpha RGBA `src` (`sw`×`sh`) through `mesh`. Output covers
/// the mesh bounds clipped to `clip`; returns the buffer and its rect.
/// Edges are anti-aliased with a 2×2 supersample near cell borders.
pub fn warp_rgba(src: &[u8], sw: u32, sh: u32, mesh: &Mesh, filter: ResizeFilter, clip: IRect) -> (Vec<u8>, IRect) {
    let out_rect = mesh.bounds().expand(1).intersect(&clip);
    if out_rect.is_empty() || sw == 0 || sh == 0 {
        return (Vec::new(), IRect::EMPTY);
    }
    let (sw_us, sh_us) = (sw as usize, sh as usize);
    // Premultiply once.
    let pre: Vec<[f32; 4]> = src
        .chunks_exact(4)
        .map(|p| {
            let a = p[3] as f32;
            [p[0] as f32 * a / 255.0, p[1] as f32 * a / 255.0, p[2] as f32 * a / 255.0, a]
        })
        .collect();

    // Per cell: inverse homography + the source sub-rect it covers.
    struct Cell {
        inv: Homography,
        bbox: IRect,
        su: f32,
        sv: f32,
        cw: f32,
        ch: f32,
    }
    let mut cells = Vec::new();
    for j in 0..mesh.rows {
        for i in 0..mesh.cols {
            let q = mesh.cell(i, j);
            let Some(h) = Homography::unit_to_quad(q) else { continue };
            let Some(inv) = h.inverse() else { continue };
            let bbox = {
                let xs = q.iter().map(|p| p.x);
                let ys = q.iter().map(|p| p.y);
                IRect::from_f32_bounds(
                    xs.clone().fold(f32::MAX, f32::min),
                    ys.clone().fold(f32::MAX, f32::min),
                    xs.fold(f32::MIN, f32::max),
                    ys.fold(f32::MIN, f32::max),
                )
                .expand(1)
                .intersect(&out_rect)
            };
            if bbox.is_empty() {
                continue;
            }
            let cw = sw as f32 / mesh.cols as f32;
            let ch = sh as f32 / mesh.rows as f32;
            cells.push(Cell { inv, bbox, su: i as f32 * cw, sv: j as f32 * ch, cw, ch });
        }
    }

    let ow = out_rect.w as usize;
    let mut out = vec![0u8; ow * out_rect.h as usize * 4];
    // 2×2 supersample offsets for anti-aliased cell edges.
    const SUB: [(f32, f32); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
    out.par_chunks_mut(ow * 4).enumerate().for_each(|(row, line)| {
        let y = out_rect.y + row as i32;
        for (col, px) in line.chunks_exact_mut(4).enumerate() {
            let x = out_rect.x + col as i32;
            let mut acc = [0f32; 4];
            let mut hits = 0u32;
            for (ox, oy) in SUB {
                let p = Pt::new(x as f32 + ox, y as f32 + oy);
                for c in &cells {
                    if !c.bbox.contains(x, y) {
                        continue;
                    }
                    let uv = c.inv.apply(p);
                    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
                        continue;
                    }
                    let s = sample(&pre, sw_us, sh_us, c.su + uv.x * c.cw, c.sv + uv.y * c.ch, filter);
                    for k in 0..4 {
                        acc[k] += s[k];
                    }
                    hits += 1;
                    break;
                }
            }
            if hits == 0 {
                continue;
            }
            // Missed subsamples contribute transparency (edge coverage).
            let n = SUB.len() as f32;
            let a = acc[3] / n;
            if a <= 0.0 {
                continue;
            }
            px[0] = (acc[0] / n / a * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            px[1] = (acc[1] / n / a * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            px[2] = (acc[2] / n / a * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            px[3] = (a + 0.5).clamp(0.0, 255.0) as u8;
        }
    });
    (out, out_rect)
}

/// Render a raster through `mesh`; returns the result and its origin.
pub fn warp_raster(src: &Raster, mesh: &Mesh, filter: ResizeFilter, clip: IRect) -> (Raster, IRect) {
    let (buf, r) = warp_rgba(&src.to_rgba(), src.width(), src.height(), mesh, filter, clip);
    if r.is_empty() {
        return (Raster::new(1, 1), IRect::EMPTY);
    }
    (Raster::from_rgba(r.w as u32, r.h as u32, &buf), r)
}

/// Render a selection mask through `mesh` into a canvas-sized mask.
pub fn warp_mask(src: &Mask, mesh: &Mesh, canvas_w: u32, canvas_h: u32) -> Mask {
    let g = src.to_gray();
    let mut rgba = Vec::with_capacity(g.len() * 4);
    for &v in g {
        rgba.extend_from_slice(&[255, 255, 255, v]);
    }
    let clip = IRect::new(0, 0, canvas_w as i32, canvas_h as i32);
    let (buf, r) = warp_rgba(&rgba, src.width(), src.height(), mesh, ResizeFilter::Bilinear, clip);
    let mut out = Mask::new(canvas_w, canvas_h);
    if r.is_empty() {
        return out;
    }
    for y in 0..r.h {
        for x in 0..r.w {
            let a = buf[((y * r.w + x) * 4 + 3) as usize];
            if a > 0 {
                out.set(r.x + x, r.y + y, a);
            }
        }
    }
    out.recompute_bounds();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba8;

    #[test]
    fn identity_quad_roundtrips() {
        let q = [Pt::new(0.0, 0.0), Pt::new(10.0, 0.0), Pt::new(10.0, 5.0), Pt::new(0.0, 5.0)];
        let h = Homography::unit_to_quad(q).unwrap();
        let p = h.apply(Pt::new(0.5, 0.5));
        assert!((p.x - 5.0).abs() < 1e-4 && (p.y - 2.5).abs() < 1e-4);
        let inv = h.inverse().unwrap();
        let u = inv.apply(Pt::new(10.0, 5.0));
        assert!((u.x - 1.0).abs() < 1e-4 && (u.y - 1.0).abs() < 1e-4);
    }

    #[test]
    fn perspective_quad_maps_corners() {
        let q = [Pt::new(0.0, 0.0), Pt::new(8.0, 1.0), Pt::new(7.0, 6.0), Pt::new(1.0, 5.0)];
        let h = Homography::unit_to_quad(q).unwrap();
        for (i, uv) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].iter().enumerate() {
            let p = h.apply(Pt::new(uv.0, uv.1));
            assert!((p.x - q[i].x).abs() < 1e-3 && (p.y - q[i].y).abs() < 1e-3, "corner {i}");
        }
    }

    #[test]
    fn warp_translates_and_scales() {
        let mut r = Raster::new(4, 4);
        r.fill_rect(IRect::new(0, 0, 4, 4), Rgba8::rgb(200, 10, 10));
        let q = [Pt::new(10.0, 10.0), Pt::new(18.0, 10.0), Pt::new(18.0, 18.0), Pt::new(10.0, 18.0)];
        let mesh = Mesh::from_quad(q, 1, 1);
        let (out, rect) = warp_raster(&r, &mesh, ResizeFilter::Nearest, IRect::new(0, 0, 100, 100));
        assert_eq!(rect, IRect::new(9, 9, 10, 10));
        assert_eq!(out.get_pixel(5, 5), Rgba8::rgb(200, 10, 10));
        assert_eq!(out.get_pixel(0, 0).a, 0);
        // The mask follows.
        let m = warp_mask(&Mask::full(4, 4), &mesh, 100, 100);
        assert_eq!(m.get(14, 14), 255);
        assert_eq!(m.get(5, 5), 0);
    }
}
