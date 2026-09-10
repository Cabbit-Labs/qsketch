//! Blur filters: Gaussian, box, motion and radial (spin / zoom).

use super::{add4, scale4, Img, RadialMode, Src};

/// Kernel half-width plus one for a Gaussian of the given radius.
pub fn gaussian_margin(radius: f32) -> i32 {
    (sigma_for(radius) * 3.0).ceil() as i32 + 1
}

fn sigma_for(radius: f32) -> f32 {
    (radius * 0.5).max(0.05)
}

/// Normalized 1-D Gaussian kernel with taps out to 3σ.
pub fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil().max(1.0) as i32;
    let mut k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let s: f32 = k.iter().sum();
    for v in &mut k {
        *v /= s;
    }
    k
}

pub fn box_kernel(radius: i32) -> Vec<f32> {
    let n = (2 * radius.max(0) + 1) as usize;
    vec![1.0 / n as f32; n]
}

/// Horizontal 1-D convolution, edge-clamped, same size as the input.
pub fn convolve_h(img: &Img, k: &[f32]) -> Img {
    let r = (k.len() / 2) as i32;
    let w = img.w as i32;
    Img::from_fn(img.w, img.h, |x, y| {
        let row = &img.px[y as usize * img.w..][..img.w];
        let mut acc = [0.0f32; 4];
        for (i, wt) in k.iter().enumerate() {
            let sx = (x + i as i32 - r).clamp(0, w - 1) as usize;
            let p = row[sx];
            acc[0] += p[0] * wt;
            acc[1] += p[1] * wt;
            acc[2] += p[2] * wt;
            acc[3] += p[3] * wt;
        }
        acc
    })
}

/// Vertical 1-D convolution, edge-clamped, same size as the input.
pub fn convolve_v(img: &Img, k: &[f32]) -> Img {
    let r = (k.len() / 2) as i32;
    let h = img.h as i32;
    Img::from_fn(img.w, img.h, |x, y| {
        let mut acc = [0.0f32; 4];
        for (i, wt) in k.iter().enumerate() {
            let sy = (y + i as i32 - r).clamp(0, h - 1) as usize;
            let p = img.px[sy * img.w + x as usize];
            acc[0] += p[0] * wt;
            acc[1] += p[1] * wt;
            acc[2] += p[2] * wt;
            acc[3] += p[3] * wt;
        }
        acc
    })
}

/// Separable Gaussian blur of a whole image (same size).
pub fn gaussian_full(img: &Img, radius: f32) -> Img {
    if radius <= 0.05 {
        return img.clone();
    }
    let k = gaussian_kernel(sigma_for(radius));
    convolve_v(&convolve_h(img, &k), &k)
}

pub fn gaussian(src: &Src, radius: f32) -> Img {
    src.crop(&gaussian_full(src.img, radius))
}

pub fn box_blur(src: &Src, radius: u32) -> Img {
    if radius == 0 {
        return src.copy();
    }
    let k = box_kernel(radius as i32);
    src.crop(&convolve_v(&convolve_h(src.img, &k), &k))
}

/// Average along a line of `distance` pixels at `angle` degrees.
pub fn motion(src: &Src, angle: f32, distance: f32) -> Img {
    let n = (distance.ceil() as usize).clamp(1, 256);
    if n <= 1 {
        return src.copy();
    }
    let (s, c) = angle.to_radians().sin_cos();
    let (dx, dy) = (c, -s);
    src.map(|x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let mut acc = [0.0f32; 4];
        for i in 0..n {
            let t = ((i as f32 + 0.5) / n as f32 - 0.5) * distance;
            acc = add4(acc, src.sample(fx + dx * t, fy + dy * t));
        }
        scale4(acc, 1.0 / n as f32)
    })
}

/// Spin (rotate around the center) or zoom (scale toward the center) blur.
/// `amount` is 0..=100.
pub fn radial(src: &Src, amount: f32, mode: RadialMode) -> Img {
    let amount = amount.clamp(0.0, 100.0);
    if amount <= 0.0 {
        return src.copy();
    }
    let (cx, cy) = src.center();
    let n = (amount as usize).clamp(8, 64);
    src.map(|x, y| {
        let px = x as f32 + 0.5 - cx;
        let py = y as f32 + 0.5 - cy;
        let mut acc = [0.0f32; 4];
        for i in 0..n {
            let t = i as f32 / (n - 1) as f32; // 0..1
            let (sx, sy) = match mode {
                RadialMode::Spin => {
                    let a = (t - 0.5) * (amount * 0.6).to_radians();
                    let (s, c) = a.sin_cos();
                    (px * c - py * s, px * s + py * c)
                }
                RadialMode::Zoom => {
                    let k = 1.0 - t * (amount / 100.0) * 0.5;
                    (px * k, py * k)
                }
            };
            acc = add4(acc, src.sample(cx + sx, cy + sy));
        }
        scale4(acc, 1.0 / n as f32)
    })
}
