//! Render filters: Perlin-noise clouds and difference clouds.

use std::f32::consts::TAU;

use super::{hash, premul, unpremul, Img, Src};
use crate::color::Rgba8;

#[inline]
fn grad(ix: i32, iy: i32, seed: u32) -> (f32, f32) {
    let a = (hash(ix, iy, seed) >> 8) as f32 / (1u32 << 24) as f32 * TAU;
    (a.cos(), a.sin())
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// 2-D gradient noise in roughly `-1..=1`.
pub fn perlin(x: f32, y: f32, seed: u32) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let (ix, iy) = (x0 as i32, y0 as i32);
    let dot = |gx: i32, gy: i32, dx: f32, dy: f32| {
        let (a, b) = grad(gx, gy, seed);
        a * dx + b * dy
    };
    let n00 = dot(ix, iy, fx, fy);
    let n10 = dot(ix + 1, iy, fx - 1.0, fy);
    let n01 = dot(ix, iy + 1, fx, fy - 1.0);
    let n11 = dot(ix + 1, iy + 1, fx - 1.0, fy - 1.0);
    let u = fade(fx);
    let v = fade(fy);
    let a = n00 + (n10 - n00) * u;
    let b = n01 + (n11 - n01) * u;
    (a + (b - a) * v) * 1.414
}

/// Fractal Brownian motion of [`perlin`], normalized to `0..=1`.
pub fn fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut freq = 1.0;
    for o in 0..octaves {
        sum += perlin(x * freq, y * freq, seed.wrapping_add(o * 0x9e37)) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    (sum / norm * 0.5 + 0.5).clamp(0.0, 1.0)
}

/// Fill with clouds between two colors (`scale` = feature size in pixels).
/// With `difference` the clouds are difference-blended onto the source.
pub fn clouds(src: &Src, scale: f32, seed: u32, color_a: Rgba8, color_b: Rgba8, difference: bool) -> Img {
    let inv = 1.0 / scale.max(2.0);
    let a = color_a.to_f32();
    let b = color_b.to_f32();
    src.map(|x, y| {
        let v = fbm((x as f32 + 0.5) * inv, (y as f32 + 0.5) * inv, seed, 6);
        // Photoshop clouds are fairly contrasty.
        let v = ((v - 0.5) * 1.7 + 0.5).clamp(0.0, 1.0);
        let c =
            [a[0] + (b[0] - a[0]) * v, a[1] + (b[1] - a[1]) * v, a[2] + (b[2] - a[2]) * v, a[3] + (b[3] - a[3]) * v];
        if !difference {
            return premul(c);
        }
        let s = unpremul(src.at(x, y));
        premul([(s[0] - c[0]).abs(), (s[1] - c[1]).abs(), (s[2] - c[2]).abs(), s[3].max(c[3])])
    })
}
