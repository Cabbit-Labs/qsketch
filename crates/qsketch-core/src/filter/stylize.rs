//! Stylize filters: find edges, emboss, solarize, diffuse, oil paint, wind.

use std::f32::consts::TAU;

use super::{add4, luma, map_rgb, premul, rand01, scale4, unpremul, Img, Src};

/// Sobel edge magnitude per channel, inverted (white background, dark edges).
pub fn find_edges(src: &Src) -> Img {
    src.map(|x, y| {
        let p = src.at(x, y);
        if p[3] <= 0.0 {
            return p;
        }
        let s = |dx: i32, dy: i32| unpremul(src.at(x + dx, y + dy));
        let (tl, t, tr) = (s(-1, -1), s(0, -1), s(1, -1));
        let (l, r) = (s(-1, 0), s(1, 0));
        let (bl, b, br) = (s(-1, 1), s(0, 1), s(1, 1));
        let mut out = [0.0f32; 4];
        for c in 0..3 {
            let gx = (tr[c] + 2.0 * r[c] + br[c]) - (tl[c] + 2.0 * l[c] + bl[c]);
            let gy = (bl[c] + 2.0 * b[c] + br[c]) - (tl[c] + 2.0 * t[c] + tr[c]);
            out[c] = 1.0 - (gx * gx + gy * gy).sqrt().min(1.0);
        }
        out[3] = p[3];
        premul(out)
    })
}

/// Gray relief lit from `angle` degrees; `height` in pixels, `amount` 0..=5.
pub fn emboss(src: &Src, angle: f32, height: f32, amount: f32) -> Img {
    let (s, c) = angle.to_radians().sin_cos();
    let (dx, dy) = (c * height, -s * height);
    src.map(|x, y| {
        let p = src.at(x, y);
        if p[3] <= 0.0 {
            return p;
        }
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let l1 = luma(unpremul(src.sample(fx + dx, fy + dy)));
        let l2 = luma(unpremul(src.sample(fx - dx, fy - dy)));
        let v = (0.5 + (l1 - l2) * amount).clamp(0.0, 1.0);
        premul([v, v, v, p[3]])
    })
}

/// Invert every channel above 50%.
pub fn solarize(src: &Src) -> Img {
    src.map(|x, y| map_rgb(src.at(x, y), |c| c.map(|v| if v > 0.5 { 1.0 - v } else { v })))
}

/// Every pixel takes the color of a random neighbor within `distance`.
pub fn diffuse(src: &Src, distance: u32, seed: u32) -> Img {
    let d = distance as f32;
    if distance == 0 {
        return src.copy();
    }
    src.map(|x, y| {
        let a = rand01(x, y, seed) * TAU;
        let r = rand01(x, y, seed ^ 0x68e3_1da4).sqrt() * d;
        src.sample(x as f32 + 0.5 + r * a.cos(), y as f32 + 0.5 + r * a.sin())
    })
}

/// Classic oil-painting effect: bucket the window's intensities into
/// `levels` bins and paint the average color of the most populated bin.
pub fn oil_paint(src: &Src, radius: u32, levels: u32) -> Img {
    const MAX_LEVELS: usize = 64;
    let r = radius.clamp(1, 12) as i32;
    let levels = (levels as usize).clamp(2, MAX_LEVELS);
    src.map(|x, y| {
        let mut counts = [0u32; MAX_LEVELS];
        let mut sums = [[0.0f32; 4]; MAX_LEVELS];
        for dy in -r..=r {
            for dx in -r..=r {
                let p = src.at(x + dx, y + dy);
                let l = luma(p); // premultiplied luma: transparent counts as dark
                let bin = ((l * (levels - 1) as f32).round() as usize).min(levels - 1);
                counts[bin] += 1;
                sums[bin] = add4(sums[bin], p);
            }
        }
        let mut best = 0;
        for i in 1..levels {
            if counts[i] > counts[best] {
                best = i;
            }
        }
        scale4(sums[best], 1.0 / counts[best].max(1) as f32)
    })
}

/// Horizontal streaks of random length up to `strength` pixels.
pub fn wind(src: &Src, strength: u32, from_left: bool, seed: u32) -> Img {
    if strength == 0 {
        return src.copy();
    }
    let dir = if from_left { -1 } else { 1 };
    src.map(|x, y| {
        // Streak length varies per 4-pixel run so streaks read as gusts.
        let len = 1 + (rand01(x >> 2, y, seed) * strength as f32) as i32;
        let mut acc = [0.0f32; 4];
        let mut wsum = 0.0;
        for i in 0..=len {
            let w = 1.0 - i as f32 / (len + 1) as f32;
            acc = add4(acc, scale4(src.at(x + dir * i, y), w));
            wsum += w;
        }
        scale4(acc, 1.0 / wsum)
    })
}
