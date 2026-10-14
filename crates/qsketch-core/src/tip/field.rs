//! Tileable scalar fields: the building blocks of the procedural grain textures.
//!
//! Everything here wraps at the tile edge (lattice indices, cell distances,
//! blurs and brush stamps all use modular arithmetic), so whatever is built
//! from these repeats with no seam and no edge blending.

use std::f32::consts::TAU;

use super::Rng;

/// A square `n × n` grid of `f32`, row-major, that wraps at its edges.
#[derive(Clone, Debug)]
pub struct Field {
    pub n: usize,
    pub v: Vec<f32>,
}

#[inline]
pub fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 0..1 hash of two integers and a seed (stable, no state).
pub fn hash01(a: u32, b: u32, seed: u64) -> f32 {
    let mut h = (a as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (b as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ seed;
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

impl Field {
    pub fn new(n: usize, fill: f32) -> Self {
        Self { n, v: vec![fill; n * n] }
    }

    /// Build from a function of the texel `(x, y)`.
    pub fn from_fn(n: usize, mut f: impl FnMut(usize, usize) -> f32) -> Self {
        let mut v = Vec::with_capacity(n * n);
        for y in 0..n {
            for x in 0..n {
                v.push(f(x, y));
            }
        }
        Self { n, v }
    }

    #[inline]
    pub fn at(&self, x: i64, y: i64) -> f32 {
        let n = self.n as i64;
        self.v[(y.rem_euclid(n) * n + x.rem_euclid(n)) as usize]
    }

    /// Bilinear sample at texel coordinates (texel centers on whole numbers + 0.5).
    pub fn bilinear(&self, x: f32, y: f32) -> f32 {
        let (x, y) = (x - 0.5, y - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (xi, yi) = (x0 as i64, y0 as i64);
        let top = self.at(xi, yi) + (self.at(xi + 1, yi) - self.at(xi, yi)) * fx;
        let bot = self.at(xi, yi + 1) + (self.at(xi + 1, yi + 1) - self.at(xi, yi + 1)) * fx;
        top + (bot - top) * fy
    }

    pub fn map(mut self, f: impl Fn(f32) -> f32) -> Self {
        self.v.iter_mut().for_each(|x| *x = f(*x));
        self
    }

    pub fn combine(mut self, o: &Field, f: impl Fn(f32, f32) -> f32) -> Self {
        assert_eq!(self.n, o.n);
        self.v.iter_mut().zip(&o.v).for_each(|(a, b)| *a = f(*a, *b));
        self
    }

    /// `self + o * k`.
    pub fn add(self, o: &Field, k: f32) -> Self {
        self.combine(o, |a, b| a + b * k)
    }

    /// Linear blend: `self` at `t = 0`, `o` at `t = 1`.
    pub fn mix(self, o: &Field, t: f32) -> Self {
        self.combine(o, |a, b| a + (b - a) * t)
    }

    pub fn range(&self) -> (f32, f32) {
        self.v.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)))
    }

    /// Stretch so the smallest value is 0 and the largest 1.
    pub fn normalize(self) -> Self {
        let (lo, hi) = self.range();
        let k = 1.0 / (hi - lo).max(1e-9);
        self.map(|x| (x - lo) * k)
    }

    /// Zero mean, unit variance: lets differently built noises be added with
    /// weights that mean what they say.
    pub fn standardize(self) -> Self {
        let n = self.v.len() as f32;
        let mean = self.v.iter().sum::<f32>() / n;
        let var = self.v.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
        let k = 1.0 / var.sqrt().max(1e-9);
        self.map(|x| (x - mean) * k)
    }

    /// Histogram equalization: remap so values spread evenly over 0..1 (a
    /// `mix` of 1), or only partway (`mix` between the stretched original and
    /// the flat histogram). The remap is a smooth curve through the field's
    /// own cumulative distribution, so it cannot band.
    pub fn equalize(self, mix: f32) -> Self {
        const BINS: usize = 2048;
        let (lo, hi) = self.range();
        let span = (hi - lo).max(1e-9);
        let mut hist = [0u32; BINS];
        for &x in &self.v {
            hist[(((x - lo) / span) * (BINS as f32 - 1.0)) as usize] += 1;
        }
        // cdf[i] = share of values below bin i; interpolate inside a bin.
        let mut cdf = [0.0f32; BINS + 1];
        let total = self.v.len() as f32;
        let mut run = 0u32;
        for i in 0..BINS {
            cdf[i] = run as f32 / total;
            run += hist[i];
        }
        cdf[BINS] = 1.0;
        self.map(|x| {
            let t = ((x - lo) / span) * (BINS as f32 - 1.0);
            let i = t as usize;
            let eq = cdf[i] + (cdf[i + 1] - cdf[i]) * (t - i as f32);
            let plain = (x - lo) / span;
            plain + (eq - plain) * mix
        })
    }

    /// Gaussian blur with wrap-around.
    pub fn blur(&self, sigma: f32) -> Field {
        if sigma < 0.05 {
            return self.clone();
        }
        let n = self.n;
        let r = (sigma * 3.0).ceil() as usize;
        let mut k: Vec<f32> = (0..=2 * r)
            .map(|i| {
                let d = i as f32 - r as f32;
                (-d * d / (2.0 * sigma * sigma)).exp()
            })
            .collect();
        let sum: f32 = k.iter().sum();
        k.iter_mut().for_each(|w| *w /= sum);
        let pass = |src: &[f32]| -> Vec<f32> {
            // Blur along rows; the caller transposes between passes.
            let mut out = vec![0.0; n * n];
            let mut padded = vec![0.0; n + 2 * r];
            for y in 0..n {
                let row = &src[y * n..(y + 1) * n];
                for (i, p) in padded.iter_mut().enumerate() {
                    *p = row[(i + n * 2 - r) % n];
                }
                for x in 0..n {
                    out[y * n + x] = padded[x..x + 2 * r + 1].iter().zip(&k).map(|(a, b)| a * b).sum();
                }
            }
            out
        };
        let transpose = |src: &[f32]| -> Vec<f32> {
            let mut out = vec![0.0; n * n];
            for y in 0..n {
                for x in 0..n {
                    out[x * n + y] = src[y * n + x];
                }
            }
            out
        };
        let a = pass(&self.v);
        let b = pass(&transpose(&a));
        Field { n, v: transpose(&b) }
    }

    /// Resample through a displacement: each texel reads the field at
    /// `(x, y) + ((dx, dy) - 0.5) * 2 * amount` texels (`dx`, `dy` in 0..1).
    pub fn warped(&self, dx: &Field, dy: &Field, amount: f32) -> Field {
        let n = self.n;
        Field::from_fn(n, |x, y| {
            let i = y * n + x;
            self.bilinear(
                x as f32 + 0.5 + (dx.v[i] - 0.5) * 2.0 * amount,
                y as f32 + 0.5 + (dy.v[i] - 0.5) * 2.0 * amount,
            )
        })
    }

    /// Slope shading of the field as a height map, lit from `(lx, ly)` (a unit
    /// vector on the canvas plane): bumps catch light on one side and shadow
    /// the other. `strength` is the slope that counts as fully lit/shadowed
    /// per unit of height; the result is centered on 0.5.
    pub fn relief(&self, lx: f32, ly: f32, strength: f32) -> Field {
        Field::from_fn(self.n, |x, y| {
            let (x, y) = (x as i64, y as i64);
            let gx = (self.at(x + 1, y) - self.at(x - 1, y)) * 0.5;
            let gy = (self.at(x, y + 1) - self.at(x, y - 1)) * 0.5;
            0.5 - (gx * lx + gy * ly) * strength
        })
    }

    /// Visit the wrapped texels of a box, calling `f(px, py)` with the texel
    /// center in unwrapped coordinates; the result is folded into the texel
    /// with `blend(old, new)`. The workhorse for stamping dots and strokes.
    pub fn stamp(
        &mut self,
        (x0, y0): (f32, f32),
        (x1, y1): (f32, f32),
        mut f: impl FnMut(f32, f32) -> f32,
        blend: impl Fn(f32, f32) -> f32,
    ) {
        let n = self.n as i64;
        for y in y0.floor() as i64..=y1.ceil() as i64 {
            for x in x0.floor() as i64..=x1.ceil() as i64 {
                let new = f(x as f32 + 0.5, y as f32 + 0.5);
                if new != 0.0 {
                    let i = (y.rem_euclid(n) * n + x.rem_euclid(n)) as usize;
                    self.v[i] = blend(self.v[i], new);
                }
            }
        }
    }

    /// A soft-edged disc (1 px antialiased rim) of `value` peak.
    pub fn disc(&mut self, cx: f32, cy: f32, r: f32, value: f32, blend: impl Fn(f32, f32) -> f32) {
        self.stamp(
            (cx - r - 1.0, cy - r - 1.0),
            (cx + r + 1.0, cy + r + 1.0),
            |px, py| {
                let d = ((px - cx) * (px - cx) + (py - cy) * (py - cy)).sqrt();
                (r - d + 0.5).clamp(0.0, 1.0) * value
            },
            blend,
        );
    }

    /// A round-capped line segment `width` px wide with an antialiased rim.
    pub fn segment(
        &mut self,
        (ax, ay): (f32, f32),
        (bx, by): (f32, f32),
        width: f32,
        value: f32,
        blend: impl Fn(f32, f32) -> f32,
    ) {
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        let pad = width * 0.5 + 1.0;
        self.stamp(
            (ax.min(bx) - pad, ay.min(by) - pad),
            (ax.max(bx) + pad, ay.max(by) + pad),
            |px, py| {
                let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
                let d = ((px - ax - dx * t).powi(2) + (py - ay - dy * t).powi(2)).sqrt();
                (width * 0.5 - d + 0.5).clamp(0.0, 1.0) * value
            },
            blend,
        );
    }
}

/// Uniform white noise in 0..1.
pub fn white(n: usize, seed: u64) -> Field {
    let mut r = Rng::new(seed);
    Field::from_fn(n, |_, _| r.unit())
}

/// Random gradient lattice, `cx × cy` cells per tile, wrapping.
struct Lattice {
    cx: usize,
    cy: usize,
    g: Vec<[f32; 2]>,
}

impl Lattice {
    fn new(cx: usize, cy: usize, rng: &mut Rng) -> Self {
        let g = (0..cx * cy)
            .map(|_| {
                let a = rng.unit() * TAU;
                [a.cos(), a.sin()]
            })
            .collect();
        Self { cx, cy, g }
    }

    /// Add `amp ×` this lattice's noise to `out`, sampled at an `(ox, oy)`
    /// offset (in cells) so octaves do not all line up at the tile origin.
    fn accumulate(&self, out: &mut [f32], n: usize, amp: f32, ox: f32, oy: f32) {
        let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
        // Per-column cell indices and fades, reused for every row.
        let cols: Vec<(usize, usize, f32, f32)> = (0..n)
            .map(|x| {
                let t = (x as f32 + 0.5) / n as f32 * self.cx as f32 + ox;
                let i = t.floor();
                let f = t - i;
                let i0 = (i as i64).rem_euclid(self.cx as i64) as usize;
                (i0, (i0 + 1) % self.cx, f, fade(f))
            })
            .collect();
        for y in 0..n {
            let t = (y as f32 + 0.5) / n as f32 * self.cy as f32 + oy;
            let j = t.floor();
            let fy = t - j;
            let sy = fade(fy);
            let j0 = (j as i64).rem_euclid(self.cy as i64) as usize;
            let j1 = (j0 + 1) % self.cy;
            let (r0, r1) = (j0 * self.cx, j1 * self.cx);
            for (x, &(i0, i1, fx, sx)) in cols.iter().enumerate() {
                let g00 = self.g[r0 + i0];
                let g10 = self.g[r0 + i1];
                let g01 = self.g[r1 + i0];
                let g11 = self.g[r1 + i1];
                let d00 = g00[0] * fx + g00[1] * fy;
                let d10 = g10[0] * (fx - 1.0) + g10[1] * fy;
                let d01 = g01[0] * fx + g01[1] * (fy - 1.0);
                let d11 = g11[0] * (fx - 1.0) + g11[1] * (fy - 1.0);
                let top = d00 + (d10 - d00) * sx;
                let bot = d01 + (d11 - d01) * sx;
                out[y * n + x] += amp * (top + (bot - top) * sy);
            }
        }
    }
}

/// Fractal gradient noise with `cells` lattice cells per tile on the first
/// octave, each next octave twice as fine and `gain` times as strong.
/// Standardized (zero mean, unit variance).
pub fn fbm(n: usize, cells: usize, octaves: u32, gain: f32, seed: u64) -> Field {
    fbm_xy(n, cells, cells, octaves, gain, seed)
}

/// `fbm` with different feature sizes along x and y: `cx` cells across and
/// `cy` down. Few cells along an axis stretch the features along it.
pub fn fbm_xy(n: usize, cx: usize, cy: usize, octaves: u32, gain: f32, seed: u64) -> Field {
    let mut rng = Rng::new(seed);
    let mut out = vec![0.0; n * n];
    let mut amp = 1.0;
    for o in 0..octaves {
        // Each octave is a little under twice as fine as the last. Exact
        // doubling would line the lattices up and print a faint grid.
        let k = LACUNARITY.powi(o as i32);
        let (px, py) = (((cx as f32 * k).round() as usize).max(1), ((cy as f32 * k).round() as usize).max(1));
        // Finer than 4 texels per cell, gradient noise shows its lattice.
        if o > 0 && px.max(py) > n / 4 {
            break;
        }
        let lat = Lattice::new(px, py, &mut rng);
        let (ox, oy) = (rng.unit() * px as f32, rng.unit() * py as f32);
        lat.accumulate(&mut out, n, amp, ox, oy);
        amp *= gain;
    }
    Field { n, v: out }.standardize()
}

/// How much finer each noise octave is than the one before.
const LACUNARITY: f32 = 1.93;

/// Wrapped Worley (cellular) distances: for every texel the distance to the
/// nearest and second-nearest feature point, in cell widths, plus the cell
/// that owns each.
pub struct Cells {
    pub f1: Field,
    pub f2: Field,
    pub c1: Vec<u32>,
    pub c2: Vec<u32>,
}

/// Scatter one point per cell of a `count × count` grid, `jitter` (0..1) of
/// the way from the cell center to anywhere in the cell.
pub fn cells(n: usize, count: usize, jitter: f32, seed: u64) -> Cells {
    let mut rng = Rng::new(seed);
    let pts: Vec<(f32, f32)> =
        (0..count * count).map(|_| (0.5 + (rng.unit() - 0.5) * jitter, 0.5 + (rng.unit() - 0.5) * jitter)).collect();
    let mut f1 = Field::new(n, 0.0);
    let mut f2 = Field::new(n, 0.0);
    let mut c1 = vec![0u32; n * n];
    let mut c2 = vec![0u32; n * n];
    let scale = count as f32 / n as f32;
    for y in 0..n {
        let v = (y as f32 + 0.5) * scale;
        let cj = v.floor() as i64;
        for x in 0..n {
            let u = (x as f32 + 0.5) * scale;
            let ci = u.floor() as i64;
            let (mut d1, mut d2) = (f32::MAX, f32::MAX);
            let (mut i1, mut i2) = (0u32, 0u32);
            for dj in -1..=1 {
                for di in -1..=1 {
                    let (gi, gj) = (ci + di, cj + dj);
                    let id = (gj.rem_euclid(count as i64) * count as i64 + gi.rem_euclid(count as i64)) as usize;
                    let p = pts[id];
                    let (ex, ey) = (gi as f32 + p.0 - u, gj as f32 + p.1 - v);
                    let d = ex * ex + ey * ey;
                    if d < d1 {
                        (d2, i2) = (d1, i1);
                        (d1, i1) = (d, id as u32);
                    } else if d < d2 {
                        (d2, i2) = (d, id as u32);
                    }
                }
            }
            let i = y * n + x;
            f1.v[i] = d1.sqrt();
            f2.v[i] = d2.sqrt();
            c1[i] = i1;
            c2[i] = i2;
        }
    }
    Cells { f1, f2, c1, c2 }
}
