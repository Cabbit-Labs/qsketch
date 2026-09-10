//! Sharpen filters: Laplacian sharpen and unsharp mask. Color math is done
//! on premultiplied RGB with the alpha kept, so transparent neighbors don't
//! produce dark halos.

use super::{blur, clamp_premul, luma, unpremul, Img, Src};

/// `out = c + amount * (c - mean(4-neighbors))`.
pub fn sharpen(src: &Src, amount: f32) -> Img {
    src.map(|x, y| {
        let c = src.at(x, y);
        if c[3] <= 0.0 {
            return c;
        }
        let n = [src.at(x - 1, y), src.at(x + 1, y), src.at(x, y - 1), src.at(x, y + 1)];
        let mut out = c;
        for ch in 0..3 {
            let mean = (n[0][ch] + n[1][ch] + n[2][ch] + n[3][ch]) * 0.25;
            out[ch] = c[ch] + amount * (c[ch] - mean);
        }
        clamp_premul(out)
    })
}

/// Classic unsharp mask: `amount` (0..=5), Gaussian `radius`, luma
/// `threshold` (0..=255) below which pixels are left alone.
pub fn unsharp(src: &Src, amount: f32, radius: f32, threshold: u32) -> Img {
    let blurred = blur::gaussian(src, radius);
    let thr = threshold as f32 / 255.0;
    src.map(|x, y| {
        let c = src.at(x, y);
        if c[3] <= 0.0 {
            return c;
        }
        let b = blurred.get(x, y);
        if thr > 0.0 && (luma(unpremul(c)) - luma(unpremul(b))).abs() < thr {
            return c;
        }
        let mut out = c;
        for ch in 0..3 {
            out[ch] = c[ch] + amount * (c[ch] - b[ch]);
        }
        clamp_premul(out)
    })
}
