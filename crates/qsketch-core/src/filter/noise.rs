//! Noise filters: add noise, median (and its thresholded cousin,
//! Dust & Scratches).

use std::f32::consts::TAU;

use super::{luma, premul, rand01, unpremul, Img, Src};

/// Largest supported median radius (window of 17×17 = 289 samples).
pub const MAX_MEDIAN_RADIUS: u32 = 8;

/// Add uniform or Gaussian noise to the color of opaque pixels; `amount` 0..=1.
pub fn add_noise(src: &Src, amount: f32, monochrome: bool, gaussian: bool, seed: u32) -> Img {
    src.map(|x, y| {
        let p = src.at(x, y);
        if p[3] <= 0.0 {
            return p;
        }
        let c = unpremul(p);
        let noise = |ch: u32| -> f32 {
            let u1 = rand01(x, y, seed ^ ch.wrapping_mul(0x9e37_79b9));
            if gaussian {
                let u2 = rand01(x, y, seed ^ (ch + 7).wrapping_mul(0x85eb_ca6b));
                ((-2.0 * u1.max(1e-6).ln()).sqrt() * (TAU * u2).cos() * 0.35).clamp(-1.0, 1.0)
            } else {
                u1 * 2.0 - 1.0
            }
        };
        let (nr, ng, nb) = if monochrome {
            let v = noise(0);
            (v, v, v)
        } else {
            (noise(0), noise(1), noise(2))
        };
        premul([c[0] + nr * amount, c[1] + ng * amount, c[2] + nb * amount, c[3]])
    })
}

/// Per-channel median over a square window. With `threshold > 0` (0..=255)
/// a pixel is only replaced when its luma differs from the median by more
/// than the threshold (Dust & Scratches).
pub fn median(src: &Src, radius: u32, threshold: u32) -> Img {
    let r = radius.min(MAX_MEDIAN_RADIUS) as i32;
    if r == 0 {
        return src.copy();
    }
    let n = ((2 * r + 1) * (2 * r + 1)) as usize;
    let thr = threshold as f32 / 255.0;
    src.map(|x, y| {
        const CAP: usize = (2 * MAX_MEDIAN_RADIUS as usize + 1) * (2 * MAX_MEDIAN_RADIUS as usize + 1);
        let mut buf = [[0.0f32; 4]; CAP];
        let mut i = 0;
        for dy in -r..=r {
            for dx in -r..=r {
                buf[i] = src.at(x + dx, y + dy);
                i += 1;
            }
        }
        let mut med = [0.0f32; 4];
        let mut ch = [0.0f32; CAP];
        for c in 0..4 {
            for (k, p) in buf[..n].iter().enumerate() {
                ch[k] = p[c];
            }
            let (_, m, _) = ch[..n].select_nth_unstable_by(n / 2, |a, b| a.total_cmp(b));
            med[c] = *m;
        }
        if thr > 0.0 {
            let o = src.at(x, y);
            if (luma(unpremul(o)) - luma(unpremul(med))).abs() <= thr {
                return o;
            }
        }
        med
    })
}
