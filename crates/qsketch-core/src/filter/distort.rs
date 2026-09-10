//! Geometric distortions: every kernel maps an output pixel back to a source
//! position and resamples bilinearly.

use std::f32::consts::{PI, TAU};

use super::{Img, Src};

/// Sinusoidal displacement on both axes.
pub fn ripple(src: &Src, amplitude: f32, wavelength: f32) -> Img {
    let k = TAU / wavelength.max(1.0);
    src.map(|x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let dx = amplitude * (fy * k).sin();
        let dy = amplitude * (fx * k).sin();
        src.sample(fx + dx, fy + dy)
    })
}

/// A single sine wave travelling along `angle`, displacing across it.
pub fn wave(src: &Src, wavelength: f32, amplitude: f32, angle: f32) -> Img {
    let k = TAU / wavelength.max(1.0);
    let (s, c) = angle.to_radians().sin_cos();
    src.map(|x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let u = fx * c + fy * s;
        let d = amplitude * (u * k).sin();
        src.sample(fx - s * d, fy + c * d)
    })
}

/// Normalized coordinates inside the ellipse inscribed in the rect.
#[inline]
fn unit(src: &Src, x: i32, y: i32) -> (f32, f32, f32, f32, f32, f32) {
    let (cx, cy) = src.center();
    let rx = cx.max(0.5);
    let ry = cy.max(0.5);
    let nx = (x as f32 + 0.5 - cx) / rx;
    let ny = (y as f32 + 0.5 - cy) / ry;
    (cx, cy, rx, ry, nx, ny)
}

/// Rotate pixels around the center, strongest in the middle.
pub fn twirl(src: &Src, angle: f32) -> Img {
    let a = angle.to_radians();
    src.map(|x, y| {
        let (cx, cy, rx, ry, nx, ny) = unit(src, x, y);
        let d = (nx * nx + ny * ny).sqrt();
        if d >= 1.0 {
            return src.at(x, y);
        }
        let t = a * (1.0 - d) * (1.0 - d);
        let (s, c) = t.sin_cos();
        let sx = nx * c - ny * s;
        let sy = nx * s + ny * c;
        src.sample(cx + sx * rx, cy + sy * ry)
    })
}

/// Bulge (`amount` > 0) or pinch (`amount` < 0) the inscribed ellipse; ±100.
pub fn spherize(src: &Src, amount: f32) -> Img {
    let k = (amount / 100.0).clamp(-1.0, 1.0);
    if k.abs() < 1e-3 {
        return src.copy();
    }
    let e = if k >= 0.0 { 1.0 + 2.0 * k } else { 1.0 / (1.0 + 2.0 * k.abs()) };
    src.map(|x, y| {
        let (cx, cy, rx, ry, nx, ny) = unit(src, x, y);
        let d = (nx * nx + ny * ny).sqrt();
        if d >= 1.0 || d <= 1e-6 {
            return src.at(x, y);
        }
        let f = d.powf(e) / d;
        src.sample(cx + nx * f * rx, cy + ny * f * ry)
    })
}

/// Concentric ripples ("pond") fading toward the edge of the inscribed ellipse.
pub fn zigzag(src: &Src, amount: f32, ridges: u32) -> Img {
    let ridges = ridges.max(1) as f32;
    src.map(|x, y| {
        let (_, _, _, _, nx, ny) = unit(src, x, y);
        let d = (nx * nx + ny * ny).sqrt();
        if d >= 1.0 || d <= 1e-6 {
            return src.at(x, y);
        }
        let disp = amount * (d * ridges * TAU).sin() * (1.0 - d);
        let (ux, uy) = (nx / d, ny / d);
        src.sample(x as f32 + 0.5 + ux * disp, y as f32 + 0.5 + uy * disp)
    })
}

/// Rectangular ↔ polar mapping. Rect→polar wraps the image around the center
/// (top row becomes the center); polar→rect is the inverse.
pub fn polar(src: &Src, to_polar: bool) -> Img {
    let (cx, cy) = src.center();
    let radius = (cx * cx + cy * cy).sqrt().max(1.0);
    let (w, h) = (src.w as f32, src.h as f32);
    if to_polar {
        src.map(|x, y| {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let r = (dx * dx + dy * dy).sqrt();
            let theta = (dy.atan2(dx) + PI * 0.5).rem_euclid(TAU);
            src.sample_or_clear(theta / TAU * w, r / radius * h)
        })
    } else {
        src.map(|x, y| {
            let theta = (x as f32 + 0.5) / w * TAU - PI * 0.5;
            let r = (y as f32 + 0.5) / h * radius;
            src.sample_or_clear(cx + r * theta.cos(), cy + r * theta.sin())
        })
    }
}
