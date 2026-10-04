//! Free-transform geometry: projective quads and warp meshes, plus the
//! inverse-mapped resampler that renders a raster (or selection mask) through
//! them. Used by the app's Free Transform (Ctrl+T) and floating paste.

use rayon::prelude::*;

use crate::raster::ResizeFilter;
use crate::{IRect, Mask, Pt, Raster};

/// Four corners in the order top-left, top-right, bottom-right, bottom-left.
pub type Quad = [Pt; 4];

/// Corners (tl, tr, br, bl) of a `size` box centred on `center`, turned
/// `angle` radians clockwise on screen.
pub fn free_quad(center: Pt, size: (f32, f32), angle: f32) -> Quad {
    let (hw, hh) = (size.0 / 2.0, size.1 / 2.0);
    let (s, c) = angle.sin_cos();
    let rot = |x: f32, y: f32| Pt::new(center.x + x * c - y * s, center.y + x * s + y * c);
    [rot(-hw, -hh), rot(hw, -hh), rot(hw, hh), rot(-hw, hh)]
}

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
        ResizeFilter::Nearest | ResizeFilter::RotSprite => {
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

/// Catmull-Rom between `p1` and `p2` at `t`, with `p0`/`p3` as the outer
/// neighbors (extrapolated past the lattice edge).
#[inline]
fn catmull_rom(p0: Pt, p1: Pt, p2: Pt, p3: Pt, t: f32) -> Pt {
    let t2 = t * t;
    let t3 = t2 * t;
    let f = |a: f32, b: f32, c: f32, d: f32| {
        0.5 * (2.0 * b + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3)
    };
    Pt::new(f(p0.x, p1.x, p2.x, p3.x), f(p0.y, p1.y, p2.y, p3.y))
}

impl Mesh {
    /// A lattice point, extrapolated linearly one step past the edge so the
    /// outer spans of the spline stay straight on an unbent lattice.
    fn point_clamped(&self, i: i64, j: i64) -> Pt {
        let (ci, cj) = (self.cols as i64, self.rows as i64);
        let axis = |k: i64, n: i64| -> (i64, i64, f32) {
            // (index, neighbor, weight): value = a + (a - b) * w
            if k < 0 {
                (0, 1, 1.0)
            } else if k > n {
                (n, n - 1, 1.0)
            } else {
                (k, k, 0.0)
            }
        };
        let (ia, ib, wi) = axis(i, ci);
        let (ja, jb, wj) = axis(j, cj);
        let a = self.point(ia as u32, ja as u32);
        if wi == 0.0 && wj == 0.0 {
            return a;
        }
        let bi = self.point(ib as u32, ja as u32);
        let bj = self.point(ia as u32, jb as u32);
        let bij = self.point(ib as u32, jb as u32);
        // Corner phantom: extrapolate along both axes.
        let dx = (a.x - bi.x) * wi + (a.x - bj.x) * wj + (a.x - bi.x - bj.x + bij.x) * wi * wj;
        let dy = (a.y - bi.y) * wi + (a.y - bj.y) * wj + (a.y - bi.y - bj.y + bij.y) * wi * wj;
        Pt::new(a.x + dx, a.y + dy)
    }

    /// The smooth surface through the control points at lattice coordinates
    /// `(u, v)` (`0..=cols`, `0..=rows`): a bicubic Catmull-Rom spline, so a
    /// bend at one point eases into its neighbors instead of kinking at the
    /// cell borders the way independent per-cell maps do. On an unbent
    /// lattice it is the plain linear map.
    pub fn surface(&self, u: f32, v: f32) -> Pt {
        let u = u.clamp(0.0, self.cols as f32);
        let v = v.clamp(0.0, self.rows as f32);
        let i = (u.floor() as i64).min(self.cols as i64 - 1);
        let j = (v.floor() as i64).min(self.rows as i64 - 1);
        let (tu, tv) = (u - i as f32, v - j as f32);
        let mut rows = [Pt::new(0.0, 0.0); 4];
        for (k, row) in rows.iter_mut().enumerate() {
            let jj = j - 1 + k as i64;
            *row = catmull_rom(
                self.point_clamped(i - 1, jj),
                self.point_clamped(i, jj),
                self.point_clamped(i + 1, jj),
                self.point_clamped(i + 2, jj),
                tu,
            );
        }
        catmull_rom(rows[0], rows[1], rows[2], rows[3], tv)
    }

    /// Whether any control point has left the straight lattice between the
    /// corners, so the smooth surface differs from the projective cells.
    fn is_bent(&self) -> bool {
        if self.cols <= 1 && self.rows <= 1 {
            return false;
        }
        let Some(h) = Homography::unit_to_quad(self.corners()) else { return true };
        for j in 0..=self.rows {
            for i in 0..=self.cols {
                let p = h.apply(Pt::new(i as f32 / self.cols as f32, j as f32 / self.rows as f32));
                let q = self.point(i, j);
                if (p.x - q.x).abs() > 1e-3 || (p.y - q.y).abs() > 1e-3 {
                    return true;
                }
            }
        }
        false
    }
}

/// One lattice cell: the projective guess for where an output point came
/// from, and the pixels it can reach.
struct Cell {
    inv: Homography,
    bbox: IRect,
    i: u32,
    j: u32,
}

/// Maps output points back to normalized source coordinates (`0..=1`²).
struct Mapper<'a> {
    mesh: &'a Mesh,
    cells: Vec<Cell>,
    /// Use the smooth spline surface (Newton-refined from the cell guess)
    /// rather than each cell's own projective map.
    smooth: bool,
}

impl<'a> Mapper<'a> {
    fn new(mesh: &'a Mesh, out_rect: IRect) -> Self {
        let smooth = mesh.is_bent();
        let mut cells = Vec::new();
        for j in 0..mesh.rows {
            for i in 0..mesh.cols {
                let q = mesh.cell(i, j);
                let Some(h) = Homography::unit_to_quad(q) else { continue };
                let Some(inv) = h.inverse() else { continue };
                let xs = q.iter().map(|p| p.x);
                let ys = q.iter().map(|p| p.y);
                let (x0, x1) = (xs.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max));
                let (y0, y1) = (ys.clone().fold(f32::MAX, f32::min), ys.fold(f32::MIN, f32::max));
                // The spline can bulge past the control hull; give the
                // cell room to catch those pixels.
                let slack = if smooth { 2 + ((x1 - x0).max(y1 - y0) * 0.3) as i32 } else { 1 };
                let bbox = IRect::from_f32_bounds(x0, y0, x1, y1).expand(slack).intersect(&out_rect);
                if bbox.is_empty() {
                    continue;
                }
                cells.push(Cell { inv, bbox, i, j });
            }
        }
        Self { mesh, cells, smooth }
    }

    /// Lattice coordinates `(u, v)` whose surface point is `p`, refined by
    /// Newton's method from `guess`.
    fn invert_smooth(&self, p: Pt, guess: Pt) -> Option<Pt> {
        let m = self.mesh;
        let (cols, rows) = (m.cols as f32, m.rows as f32);
        let mut uv = guess;
        const H: f32 = 1e-3;
        for _ in 0..12 {
            let s = m.surface(uv.x, uv.y);
            let (rx, ry) = (s.x - p.x, s.y - p.y);
            if rx.abs() < 2e-3 && ry.abs() < 2e-3 {
                let eps = 1e-3;
                return (uv.x >= -eps && uv.x <= cols + eps && uv.y >= -eps && uv.y <= rows + eps).then_some(uv);
            }
            let su = m.surface(uv.x + H, uv.y);
            let sv = m.surface(uv.x, uv.y + H);
            let (a, b) = ((su.x - s.x) / H, (sv.x - s.x) / H);
            let (c, d) = ((su.y - s.y) / H, (sv.y - s.y) / H);
            let det = a * d - b * c;
            if det.abs() < 1e-9 {
                return None;
            }
            let du = (d * rx - b * ry) / det;
            let dv = (-c * rx + a * ry) / det;
            // Stay near the lattice so a wild step can't wander off.
            uv.x = (uv.x - du).clamp(-1.0, cols + 1.0);
            uv.y = (uv.y - dv).clamp(-1.0, rows + 1.0);
        }
        None
    }

    /// Normalized source coordinates for output point `p`, or None outside
    /// the warped picture. `(x, y)` is the pixel `p` lies in.
    fn source(&self, p: Pt, x: i32, y: i32) -> Option<(f32, f32)> {
        let (cols, rows) = (self.mesh.cols as f32, self.mesh.rows as f32);
        for c in &self.cells {
            if !c.bbox.contains(x, y) {
                continue;
            }
            let uv = c.inv.apply(p);
            if self.smooth {
                let guess = Pt::new(c.i as f32 + uv.x.clamp(-0.5, 1.5), c.j as f32 + uv.y.clamp(-0.5, 1.5));
                if let Some(g) = self.invert_smooth(p, guess) {
                    return Some(((g.x / cols).clamp(0.0, 1.0), (g.y / rows).clamp(0.0, 1.0)));
                }
            } else if (0.0..=1.0).contains(&uv.x) && (0.0..=1.0).contains(&uv.y) {
                return Some(((c.i as f32 + uv.x) / cols, (c.j as f32 + uv.y) / rows));
            }
        }
        None
    }
}

/// How far a control point may sit from its undistorted position and still
/// count as "not moved" for the straight-copy fast path below. Well under the
/// rounding a pixel could ever show.
const TRANSLATE_EPS: f32 = 1e-3;

/// The whole-pixel offset `mesh` translates the source by, when that is all it
/// does: every control point still on its undistorted lattice and the top-left
/// corner on integer coordinates.
fn integer_translation(mesh: &Mesh, sw: u32, sh: u32) -> Option<(i32, i32)> {
    let (x, y) = (mesh.point(0, 0).x, mesh.point(0, 0).y);
    if (x - x.round()).abs() > TRANSLATE_EPS || (y - y.round()).abs() > TRANSLATE_EPS {
        return None;
    }
    let (sw, sh) = (sw as f32, sh as f32);
    for j in 0..=mesh.rows {
        for i in 0..=mesh.cols {
            let p = mesh.point(i, j);
            let wx = x + sw * i as f32 / mesh.cols as f32;
            let wy = y + sh * j as f32 / mesh.rows as f32;
            if (p.x - wx).abs() > TRANSLATE_EPS || (p.y - wy).abs() > TRANSLATE_EPS {
                return None;
            }
        }
    }
    Some((x.round() as i32, y.round() as i32))
}

/// Copy `src` to whole-pixel offset `(tx, ty)`, cropped to `clip`.
fn translate_rgba(src: &[u8], sw: u32, sh: u32, tx: i32, ty: i32, clip: IRect) -> (Vec<u8>, IRect) {
    let out_rect = IRect::new(tx, ty, sw as i32, sh as i32).intersect(&clip);
    if out_rect.is_empty() {
        return (Vec::new(), IRect::EMPTY);
    }
    let (ow, oh) = (out_rect.w as usize, out_rect.h as usize);
    let sw_us = sw as usize;
    let (skip_x, skip_y) = ((out_rect.x - tx) as usize, (out_rect.y - ty) as usize);
    let mut out = vec![0u8; ow * oh * 4];
    for (row, line) in out.chunks_exact_mut(ow * 4).enumerate() {
        let start = ((skip_y + row) * sw_us + skip_x) * 4;
        line.copy_from_slice(&src[start..start + ow * 4]);
    }
    (out, out_rect)
}

/// 4×4 subsample offsets for edge coverage.
const SUB16: [(f32, f32); 16] = [
    (0.125, 0.125),
    (0.375, 0.125),
    (0.625, 0.125),
    (0.875, 0.125),
    (0.125, 0.375),
    (0.375, 0.375),
    (0.625, 0.375),
    (0.875, 0.375),
    (0.125, 0.625),
    (0.375, 0.625),
    (0.625, 0.625),
    (0.875, 0.625),
    (0.125, 0.875),
    (0.375, 0.875),
    (0.625, 0.875),
    (0.875, 0.875),
];

/// Render straight-alpha RGBA `src` (`sw`×`sh`) through `mesh`. Output covers
/// the mesh bounds clipped to `clip`; returns the buffer and its rect.
///
/// Smooth (bilinear): the interior takes one filtered sample per pixel
/// center (no extra blur) and the outline is anti-aliased with 4×4
/// coverage. Pixel (nearest): one unfiltered sample per pixel, hard edges,
/// so pixel art stays made of whole pixels.
pub fn warp_rgba(src: &[u8], sw: u32, sh: u32, mesh: &Mesh, filter: ResizeFilter, clip: IRect) -> (Vec<u8>, IRect) {
    let out_rect = mesh.bounds().expand(2).intersect(&clip);
    if out_rect.is_empty() || sw == 0 || sh == 0 {
        return (Vec::new(), IRect::EMPTY);
    }
    if filter == ResizeFilter::RotSprite && integer_translation(mesh, sw, sh).is_none() {
        return rotsprite_rgba(src, sw, sh, mesh, clip);
    }
    // A mesh that only shifts the pixels by whole pixels is a copy. Running it
    // through the resampler instead would soften the pixels and feather the
    // edges, so a moved or duplicated selection would come back blurrier
    // every time.
    if let Some((tx, ty)) = integer_translation(mesh, sw, sh) {
        return translate_rgba(src, sw, sh, tx, ty, clip);
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
    let map = Mapper::new(mesh, out_rect);
    let (swf, shf) = (sw as f32, sh as f32);
    let hard = filter != ResizeFilter::Bilinear;

    let ow = out_rect.w as usize;
    let mut out = vec![0u8; ow * out_rect.h as usize * 4];
    out.par_chunks_mut(ow * 4).enumerate().for_each(|(row, line)| {
        let y = out_rect.y + row as i32;
        for (col, px) in line.chunks_exact_mut(4).enumerate() {
            let x = out_rect.x + col as i32;
            let center = map.source(Pt::new(x as f32 + 0.5, y as f32 + 0.5), x, y);
            if hard {
                let Some((u, v)) = center else { continue };
                let s = sample(&pre, sw_us, sh_us, u * swf, v * shf, filter);
                write_px(px, s, 1.0);
                continue;
            }
            // Coverage: a quick 2×2 probe, then the full 4×4 only on pixels
            // the outline passes through.
            let probe = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
            let hits2 = probe
                .iter()
                .filter(|(ox, oy)| map.source(Pt::new(x as f32 + ox, y as f32 + oy), x, y).is_some())
                .count();
            if hits2 == 4 {
                if let Some((u, v)) = center {
                    write_px(px, sample(&pre, sw_us, sh_us, u * swf, v * shf, filter), 1.0);
                    continue;
                }
            } else if hits2 == 0 && center.is_none() {
                continue;
            }
            let mut acc = [0f32; 4];
            let mut hits = 0u32;
            for (ox, oy) in SUB16 {
                let Some((u, v)) = map.source(Pt::new(x as f32 + ox, y as f32 + oy), x, y) else { continue };
                let s = sample(&pre, sw_us, sh_us, u * swf, v * shf, filter);
                for k in 0..4 {
                    acc[k] += s[k];
                }
                hits += 1;
            }
            if hits == 0 {
                continue;
            }
            let coverage = hits as f32 / SUB16.len() as f32;
            let mean = [acc[0] / hits as f32, acc[1] / hits as f32, acc[2] / hits as f32, acc[3] / hits as f32];
            write_px(px, mean, coverage);
        }
    });
    (out, out_rect)
}

/// Store a premultiplied sample at `coverage` as straight RGBA.
#[inline]
fn write_px(px: &mut [u8], s: [f32; 4], coverage: f32) {
    let a = s[3];
    if a <= 0.0 {
        return;
    }
    px[0] = (s[0] / a * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    px[1] = (s[1] / a * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    px[2] = (s[2] / a * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    px[3] = (a * coverage + 0.5).clamp(0.0, 255.0) as u8;
}

/// RotSprite: enlarge the source with Scale2x (up to three passes, 8×, as
/// far as a sane memory budget allows), then map every output pixel center
/// back into it and take that one pixel: no blending, no soft edges, and no
/// color that was not in the source.
fn rotsprite_rgba(src: &[u8], sw: u32, sh: u32, mesh: &Mesh, clip: IRect) -> (Vec<u8>, IRect) {
    const BUDGET: u64 = 16 * 1024 * 1024;
    let mut px: Vec<u32> = src.chunks_exact(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]])).collect();
    let (mut w, mut h) = (sw as usize, sh as usize);
    for _ in 0..3 {
        if (w as u64 * 2) * (h as u64 * 2) > BUDGET {
            break;
        }
        px = crate::raster::scale2x(&px, w, h);
        w *= 2;
        h *= 2;
    }
    let out_rect = mesh.bounds().expand(2).intersect(&clip);
    if out_rect.is_empty() {
        return (Vec::new(), IRect::EMPTY);
    }
    let map = Mapper::new(mesh, out_rect);
    let ow = out_rect.w as usize;
    let mut out = vec![0u8; ow * out_rect.h as usize * 4];
    out.par_chunks_mut(ow * 4).enumerate().for_each(|(row, line)| {
        let y = out_rect.y + row as i32;
        for (col, dst) in line.chunks_exact_mut(4).enumerate() {
            let x = out_rect.x + col as i32;
            let Some((u, v)) = map.source(Pt::new(x as f32 + 0.5, y as f32 + 0.5), x, y) else { continue };
            let sx = ((u * w as f32).floor() as i64).clamp(0, w as i64 - 1) as usize;
            let sy = ((v * h as f32).floor() as i64).clamp(0, h as i64 - 1) as usize;
            dst.copy_from_slice(&px[sy * w + sx].to_le_bytes());
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

    #[test]
    fn rotsprite_keeps_the_palette_and_hard_edges() {
        // A two-color 8×8 sprite rotated 30°: every output pixel is one of
        // the source colors, fully opaque or fully clear.
        let red = [200u8, 30, 30, 255];
        let blue = [20u8, 40, 220, 255];
        let mut src = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                src.extend_from_slice(if (x + y) % 3 == 0 { &red } else { &blue });
            }
        }
        let c = Pt::new(12.0, 12.0);
        let (s, co) = (30f32.to_radians().sin(), 30f32.to_radians().cos());
        let rot = |x: f32, y: f32| Pt::new(c.x + x * co - y * s, c.y + x * s + y * co);
        let quad = [rot(-4.0, -4.0), rot(4.0, -4.0), rot(4.0, 4.0), rot(-4.0, 4.0)];
        let (out, r) =
            warp_rgba(&src, 8, 8, &Mesh::from_quad(quad, 1, 1), ResizeFilter::RotSprite, IRect::new(0, 0, 32, 32));
        assert!(!r.is_empty());
        let mut opaque = 0;
        for p in out.chunks_exact(4) {
            match p[3] {
                0 => {}
                255 => {
                    opaque += 1;
                    assert!(p == red || p == blue, "new color {p:?}");
                }
                a => panic!("partial alpha {a}"),
            }
        }
        assert!(opaque > 40);
    }

    #[test]
    fn scale2x_rounds_a_diagonal() {
        // 2×2 checker of A/B: Scale2x fills the corners along the diagonal.
        let (a, b) = (1u32, 2u32);
        let out = crate::raster::scale2x(&[a, b, b, a], 2, 2);
        assert_eq!(out.len(), 16);
        assert!(out.iter().all(|&v| v == a || v == b));
    }
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

    /// A plain move / duplicate must copy the pixels verbatim: no softening
    /// from the resampler, no feathered edge.
    #[test]
    fn integer_translation_is_a_verbatim_copy() {
        let mut r = Raster::new(4, 4);
        r.fill_rect(IRect::new(0, 0, 4, 4), Rgba8::rgb(200, 10, 10));
        r.set_pixel(1, 1, Rgba8::rgb(0, 255, 0));
        r.set_pixel(0, 0, Rgba8::TRANSPARENT);
        let q = [Pt::new(7.0, 3.0), Pt::new(11.0, 3.0), Pt::new(11.0, 7.0), Pt::new(7.0, 7.0)];
        for div in [1, 3] {
            let mesh = Mesh::from_quad(q, div, div);
            let (out, rect) = warp_raster(&r, &mesh, ResizeFilter::Bilinear, IRect::new(0, 0, 100, 100));
            assert_eq!(rect, IRect::new(7, 3, 4, 4), "div {div}");
            for y in 0..4 {
                for x in 0..4 {
                    assert_eq!(out.get_pixel(x, y), r.get_pixel(x, y), "div {div} at {x},{y}");
                }
            }
            // The mask travels with it, still hard-edged.
            let m = warp_mask(&Mask::full(4, 4), &mesh, 100, 100);
            assert_eq!(m.bounds(), IRect::new(7, 3, 4, 4), "div {div}");
            assert_eq!(m.get(7, 3), 255, "div {div}");
            assert_eq!(m.get(6, 3), 0, "div {div}");
        }
    }

    /// A half-pixel offset is a real resample, not a copy.
    #[test]
    fn subpixel_translation_still_resamples() {
        let mesh =
            Mesh::from_quad([Pt::new(7.5, 3.0), Pt::new(11.5, 3.0), Pt::new(11.5, 7.0), Pt::new(7.5, 7.0)], 1, 1);
        assert_eq!(integer_translation(&mesh, 4, 4), None);
    }

    /// Bending one node of a 3×3 lattice gives a smooth surface: the
    /// control points are hit exactly, the unbent lattice is still linear,
    /// and a vertical source line comes out unbroken (every row covered).
    #[test]
    fn bent_mesh_is_smooth_and_unbroken() {
        let q = [Pt::new(10.0, 10.0), Pt::new(40.0, 10.0), Pt::new(40.0, 40.0), Pt::new(10.0, 40.0)];
        let mut mesh = Mesh::from_quad(q, 3, 3);
        assert!(!mesh.is_bent());
        let p = mesh.surface(1.5, 0.5);
        assert!((p.x - 25.0).abs() < 1e-3 && (p.y - 15.0).abs() < 1e-3, "{p:?}");
        let k = 5usize; // node (1, 1) of the 4-wide lattice
        mesh.pts[k].x += 6.0;
        mesh.pts[k].y += 4.0;
        assert!(mesh.is_bent());
        let c = mesh.surface(1.0, 1.0);
        assert!((c.x - mesh.pts[k].x).abs() < 1e-3 && (c.y - mesh.pts[k].y).abs() < 1e-3);
        // Source: 30×30 with a 2-px vertical black line at x = 10.
        let mut r = Raster::new(30, 30);
        r.fill_rect(IRect::new(10, 0, 2, 30), Rgba8::rgb(0, 0, 0));
        let (out, rect) = warp_raster(&r, &mesh, ResizeFilter::Bilinear, IRect::new(0, 0, 100, 100));
        for y in 12..38 {
            let row = y - rect.y;
            let dark = (0..rect.w).any(|x| {
                let p = out.get_pixel(x, row);
                p.a > 200 && p.r < 60
            });
            assert!(dark, "row {y} lost the line");
        }
        // Hard mode: whole pixels only.
        let (out, _) = warp_raster(&r, &mesh, ResizeFilter::Nearest, IRect::new(0, 0, 100, 100));
        for y in 0..out.height() as i32 {
            for x in 0..out.width() as i32 {
                let a = out.get_pixel(x, y).a;
                assert!(a == 0 || a == 255);
            }
        }
    }

    #[test]
    fn warp_translates_and_scales() {
        let mut r = Raster::new(4, 4);
        r.fill_rect(IRect::new(0, 0, 4, 4), Rgba8::rgb(200, 10, 10));
        let q = [Pt::new(10.0, 10.0), Pt::new(18.0, 10.0), Pt::new(18.0, 18.0), Pt::new(10.0, 18.0)];
        let mesh = Mesh::from_quad(q, 1, 1);
        let (out, rect) = warp_raster(&r, &mesh, ResizeFilter::Nearest, IRect::new(0, 0, 100, 100));
        assert_eq!(rect, IRect::new(8, 8, 12, 12));
        assert_eq!(out.get_pixel(6, 6), Rgba8::rgb(200, 10, 10));
        assert_eq!(out.get_pixel(0, 0).a, 0);
        // The mask follows.
        let m = warp_mask(&Mask::full(4, 4), &mesh, 100, 100);
        assert_eq!(m.get(14, 14), 255);
        assert_eq!(m.get(5, 5), 0);
    }
}
