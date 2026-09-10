//! Experimental effects: chromatic aberration, ordered dither, pixel sort,
//! scanlines, vignette, glow, kaleidoscope, outline, glitch, pencil sketch.

use std::f32::consts::TAU;

use rayon::prelude::*;

use super::{add4, clamp_premul, luma, premul, rand01, scale4, smoothstep, unpremul, DitherPattern, Img, Src};
use crate::color::Rgba8;
use crate::tip::Rng;

/// Shift the red and blue channels apart; radially from the center or along
/// a fixed angle.
pub fn chromatic_aberration(src: &Src, amount: f32, radial: bool, angle: f32) -> Img {
    if amount.abs() < 1e-3 {
        return src.copy();
    }
    let (cx, cy) = src.center();
    let (s, c) = angle.to_radians().sin_cos();
    let norm = (cx * cx + cy * cy).sqrt().max(1.0);
    src.map(|x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let (dx, dy) = if radial {
            let (vx, vy) = (fx - cx, fy - cy);
            let d = (vx * vx + vy * vy).sqrt();
            if d < 1e-3 {
                (0.0, 0.0)
            } else {
                let k = amount * (d / norm) / d;
                (vx * k, vy * k)
            }
        } else {
            (c * amount, -s * amount)
        };
        let r = src.sample(fx + dx, fy + dy);
        let g = src.at(x, y);
        let b = src.sample(fx - dx, fy - dy);
        clamp_premul([r[0], g[1], b[2], r[3].max(g[3]).max(b[3])])
    })
}

const BAYER2: [[u8; 2]; 2] = [[0, 2], [3, 1]];
const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
const BAYER8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// Posterize to `levels` per channel with an ordered (or noise) dither.
pub fn dither(src: &Src, levels: u32, pattern: DitherPattern) -> Img {
    let l = (levels.clamp(2, 256) - 1) as f32;
    src.map(|x, y| {
        let t = match pattern {
            DitherPattern::None => 0.5,
            DitherPattern::Bayer2 => (BAYER2[(y & 1) as usize][(x & 1) as usize] as f32 + 0.5) / 4.0,
            DitherPattern::Bayer4 => (BAYER4[(y & 3) as usize][(x & 3) as usize] as f32 + 0.5) / 16.0,
            DitherPattern::Bayer8 => (BAYER8[(y & 7) as usize][(x & 7) as usize] as f32 + 0.5) / 64.0,
            DitherPattern::Noise => rand01(x, y, 0x1234),
        };
        let p = src.at(x, y);
        if p[3] <= 0.0 {
            return p;
        }
        let c = unpremul(p);
        let q = |v: f32| ((v * l + t).floor() / l).clamp(0.0, 1.0);
        premul([q(c[0]), q(c[1]), q(c[2]), c[3]])
    })
}

/// Sort runs of pixels brighter than `threshold` by luma, per row (or
/// column). The glitch-art classic.
pub fn pixel_sort(src: &Src, threshold: f32, vertical: bool, reverse: bool) -> Img {
    let sort_line = |line: &mut Vec<[f32; 4]>| {
        let bright = |p: &[f32; 4]| luma(unpremul(*p)) >= threshold && p[3] > 0.0;
        let mut i = 0;
        while i < line.len() {
            if !bright(&line[i]) {
                i += 1;
                continue;
            }
            let start = i;
            while i < line.len() && bright(&line[i]) {
                i += 1;
            }
            let span = &mut line[start..i];
            span.sort_by(|a, b| luma(unpremul(*a)).total_cmp(&luma(unpremul(*b))));
            if reverse {
                span.reverse();
            }
        }
    };
    if vertical {
        let cols: Vec<Vec<[f32; 4]>> = (0..src.w as i32)
            .into_par_iter()
            .map(|x| {
                let mut col: Vec<[f32; 4]> = (0..src.h as i32).map(|y| src.at(x, y)).collect();
                sort_line(&mut col);
                col
            })
            .collect();
        src.map(|x, y| cols[x as usize][y as usize])
    } else {
        let rows: Vec<Vec<[f32; 4]>> = (0..src.h as i32)
            .into_par_iter()
            .map(|y| {
                let mut row: Vec<[f32; 4]> = (0..src.w as i32).map(|x| src.at(x, y)).collect();
                sort_line(&mut row);
                row
            })
            .collect();
        src.map(|x, y| rows[y as usize][x as usize])
    }
}

/// Darken every other band of rows; optionally tint columns in an RGB triad
/// like a CRT shadow mask.
pub fn scanlines(src: &Src, spacing: u32, darkness: f32, rgb_mask: bool) -> Img {
    let spacing = spacing.max(2) as i32;
    let thick = (spacing / 2).max(1);
    let darkness = darkness.clamp(0.0, 1.0);
    src.map(|x, y| {
        let p = src.at(x, y);
        if p[3] <= 0.0 {
            return p;
        }
        let line = y.rem_euclid(spacing) >= spacing - thick;
        let f = if line { 1.0 - darkness } else { 1.0 };
        let mut m = [f, f, f];
        if rgb_mask {
            let k = x.rem_euclid(3) as usize;
            let dim = 1.0 - 0.45 * darkness;
            for (i, v) in m.iter_mut().enumerate() {
                if i != k {
                    *v *= dim;
                }
            }
        }
        [p[0] * m[0], p[1] * m[1], p[2] * m[2], p[3]]
    })
}

/// Fade the edges toward a color. `amount` and `softness` in 0..=1.
pub fn vignette(src: &Src, amount: f32, softness: f32, color: Rgba8) -> Img {
    let (cx, cy) = src.center();
    let col = color.to_f32();
    let inner = (1.0 - softness.clamp(0.0, 1.0)).max(0.0);
    src.map(|x, y| {
        let p = src.at(x, y);
        if p[3] <= 0.0 {
            return p;
        }
        let dx = (x as f32 + 0.5 - cx) / cx.max(0.5);
        let dy = (y as f32 + 0.5 - cy) / cy.max(0.5);
        let d = (dx * dx + dy * dy).sqrt() / std::f32::consts::SQRT_2;
        let t = smoothstep(inner, 1.0, d) * amount.clamp(0.0, 1.0);
        let c = unpremul(p);
        premul([c[0] + (col[0] - c[0]) * t, c[1] + (col[1] - c[1]) * t, c[2] + (col[2] - c[2]) * t, c[3]])
    })
}

/// Bloom: blur the pixels brighter than `threshold` and add them back.
pub fn glow(src: &Src, radius: f32, intensity: f32, threshold: f32) -> Img {
    let thr = threshold.clamp(0.0, 0.999);
    let img = src.img;
    let bright = Img::from_fn(img.w, img.h, |x, y| {
        let p = img.get(x, y);
        if p[3] <= 0.0 {
            return p;
        }
        let k = ((luma(unpremul(p)) - thr) / (1.0 - thr)).clamp(0.0, 1.0);
        scale4(p, k * k)
    });
    let blurred = super::blur::gaussian_full(&bright, radius);
    let halo = src.crop(&blurred);
    src.map(|x, y| clamp_premul(add4(src.at(x, y), scale4(halo.get(x, y), intensity))))
}

/// Mirror the image into `segments` wedges around the center.
pub fn kaleidoscope(src: &Src, segments: u32, angle: f32) -> Img {
    let n = segments.max(2) as f32;
    let seg = TAU / n;
    let a0 = angle.to_radians();
    let (cx, cy) = src.center();
    src.map(|x, y| {
        let dx = x as f32 + 0.5 - cx;
        let dy = y as f32 + 0.5 - cy;
        let r = (dx * dx + dy * dy).sqrt();
        let mut t = (dy.atan2(dx) - a0).rem_euclid(seg);
        if t > seg * 0.5 {
            t = seg - t;
        }
        t += a0;
        src.sample(cx + r * t.cos(), cy + r * t.sin())
    })
}

/// Chamfer (3-4) distance from every pixel to the nearest pixel whose alpha
/// satisfies `inside`. Sequential two-pass over the whole image.
fn distance_field(img: &Img, inside: impl Fn(f32) -> bool) -> Vec<f32> {
    let (w, h) = (img.w, img.h);
    let mut d = vec![f32::MAX; w * h];
    for (i, p) in img.px.iter().enumerate() {
        if inside(p[3]) {
            d[i] = 0.0;
        }
    }
    const D1: f32 = 1.0;
    const D2: f32 = std::f32::consts::SQRT_2;
    let at = |d: &Vec<f32>, x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            f32::MAX
        } else {
            d[y as usize * w + x as usize]
        }
    };
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let i = y as usize * w + x as usize;
            if d[i] == 0.0 {
                continue;
            }
            let m = (at(&d, x - 1, y) + D1)
                .min(at(&d, x, y - 1) + D1)
                .min(at(&d, x - 1, y - 1) + D2)
                .min(at(&d, x + 1, y - 1) + D2);
            d[i] = d[i].min(m);
        }
    }
    for y in (0..h as i32).rev() {
        for x in (0..w as i32).rev() {
            let i = y as usize * w + x as usize;
            if d[i] == 0.0 {
                continue;
            }
            let m = (at(&d, x + 1, y) + D1)
                .min(at(&d, x, y + 1) + D1)
                .min(at(&d, x + 1, y + 1) + D2)
                .min(at(&d, x - 1, y + 1) + D2);
            d[i] = d[i].min(m);
        }
    }
    d
}

/// Stroke the alpha edge of the content: outside (behind the pixels) or
/// inside (over them). Great for stickers and sprites.
pub fn outline(src: &Src, width: u32, color: Rgba8, inside: bool) -> Img {
    let width = width as f32;
    let col = color.to_f32();
    let img = src.img;
    if inside {
        let dist = distance_field(img, |a| a < 0.5);
        src.map(|x, y| {
            let p = src.at(x, y);
            if p[3] <= 0.0 {
                return p;
            }
            let d = dist[(y + src.oy) as usize * img.w + (x + src.ox) as usize];
            let cov = (width + 0.5 - d).clamp(0.0, 1.0) * col[3];
            if cov <= 0.0 {
                return p;
            }
            let c = unpremul(p);
            premul([c[0] + (col[0] - c[0]) * cov, c[1] + (col[1] - c[1]) * cov, c[2] + (col[2] - c[2]) * cov, c[3]])
        })
    } else {
        let dist = distance_field(img, |a| a >= 0.5);
        let stroke = super::color(color);
        src.map(|x, y| {
            let p = src.at(x, y);
            if p[3] >= 1.0 {
                return p;
            }
            let d = dist[(y + src.oy) as usize * img.w + (x + src.ox) as usize];
            let cov = (width + 0.5 - d).clamp(0.0, 1.0);
            if cov <= 0.0 {
                return p;
            }
            // Content over the stroke.
            add4(p, scale4(stroke, cov * (1.0 - p[3])))
        })
    }
}

/// Horizontal bands shifted sideways with the red/blue channels split;
/// deterministic per seed.
pub fn glitch(src: &Src, amount: f32, seed: u32) -> Img {
    let h = src.h;
    let mut rng = Rng::new(seed as u64 + 1);
    let mut shift = vec![0i32; h];
    let mut split = vec![0i32; h];
    let mut y = 0usize;
    while y < h {
        let band = 2 + (rng.unit() * (h as f32 / 6.0).max(2.0)) as usize;
        let (s, c) = if rng.unit() < 0.45 {
            ((rng.signed() * amount).round() as i32, (rng.unit() * amount * 0.5).round() as i32)
        } else {
            (0, 0)
        };
        for row in y..(y + band).min(h) {
            shift[row] = s;
            split[row] = c;
        }
        y += band;
    }
    src.map(|x, y| {
        let s = shift[y as usize];
        let c = split[y as usize];
        if s == 0 && c == 0 {
            return src.at(x, y);
        }
        let g = src.at_wrap(x - s, y);
        let r = src.at_wrap(x - s - c, y);
        let b = src.at_wrap(x - s + c, y);
        clamp_premul([r[0], g[1], b[2], r[3].max(g[3]).max(b[3])])
    })
}

/// Grayscale sketch: luma color-dodged by its blurred inverse.
pub fn pencil_sketch(src: &Src, radius: f32, strength: f32) -> Img {
    let img = src.img;
    let inv = Img::from_fn(img.w, img.h, |x, y| {
        let p = img.get(x, y);
        let l = 1.0 - luma(unpremul(p));
        [l * p[3], l * p[3], l * p[3], p[3]]
    });
    let blurred = src.crop(&super::blur::gaussian_full(&inv, radius));
    src.map(|x, y| {
        let p = src.at(x, y);
        if p[3] <= 0.0 {
            return p;
        }
        let l = luma(unpremul(p));
        let b = unpremul(blurred.get(x, y))[0];
        let dodge = (l / (1.0 - b).max(1e-3)).min(1.0);
        let v = dodge.powf(strength.max(0.05));
        [v * p[3], v * p[3], v * p[3], p[3]]
    })
}
