//! Pixelate filters: mosaic, crystallize, fragment, color halftone and
//! pointillize.

use rayon::prelude::*;

use super::{add4, color, premul, rand01, scale4, unpremul, Img, Src};
use crate::color::Rgba8;

/// Average of the source pixels in a square around `(x, y)` (radius `r`).
fn box_avg(src: &Src, x: i32, y: i32, r: i32) -> [f32; 4] {
    let mut acc = [0.0f32; 4];
    for dy in -r..=r {
        for dx in -r..=r {
            acc = add4(acc, src.at(x + dx, y + dy));
        }
    }
    scale4(acc, 1.0 / ((2 * r + 1) * (2 * r + 1)) as f32)
}

/// Square cells filled with their average color.
pub fn mosaic(src: &Src, cell: u32) -> Img {
    let c = cell.max(1) as usize;
    if c <= 1 {
        return src.copy();
    }
    let cells_x = src.w.div_ceil(c);
    let cells_y = src.h.div_ceil(c);
    let avgs: Vec<[f32; 4]> = (0..cells_y)
        .into_par_iter()
        .flat_map_iter(|cy| {
            (0..cells_x).map(move |cx| {
                let x0 = cx * c;
                let y0 = cy * c;
                let x1 = (x0 + c).min(src.w);
                let y1 = (y0 + c).min(src.h);
                let mut acc = [0.0f32; 4];
                for y in y0..y1 {
                    for x in x0..x1 {
                        acc = add4(acc, src.at(x as i32, y as i32));
                    }
                }
                scale4(acc, 1.0 / ((x1 - x0) * (y1 - y0)).max(1) as f32)
            })
        })
        .collect();
    src.map(|x, y| avgs[(y as usize / c) * cells_x + x as usize / c])
}

/// Jittered-grid Voronoi seed nearest to `(x, y)`: returns the seed position
/// and the squared distance to it.
fn nearest_seed(x: i32, y: i32, cell: i32, seed: u32) -> ((i32, i32), f32) {
    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
    let ci = x.div_euclid(cell);
    let cj = y.div_euclid(cell);
    let mut best = ((0, 0), f32::MAX);
    for j in cj - 1..=cj + 1 {
        for i in ci - 1..=ci + 1 {
            let sx = i as f32 * cell as f32 + rand01(i, j, seed) * cell as f32;
            let sy = j as f32 * cell as f32 + rand01(i, j, seed ^ 0x5bd1_e995) * cell as f32;
            let d = (sx - fx) * (sx - fx) + (sy - fy) * (sy - fy);
            if d < best.1 {
                best = ((sx as i32, sy as i32), d);
            }
        }
    }
    best
}

/// Voronoi cells (jittered grid) filled with the color around their seed.
pub fn crystallize(src: &Src, cell: u32, seed: u32) -> Img {
    let cell = cell.max(2) as i32;
    src.map(|x, y| {
        let ((sx, sy), _) = nearest_seed(x, y, cell, seed);
        box_avg(src, sx, sy, 1)
    })
}

/// Four copies offset by 4 px in each direction, averaged.
pub fn fragment(src: &Src) -> Img {
    src.map(|x, y| {
        let acc = add4(add4(src.at(x - 4, y), src.at(x + 4, y)), add4(src.at(x, y - 4), src.at(x, y + 4)));
        scale4(acc, 0.25)
    })
}

/// Per-channel halftone screens at Photoshop's default angles (108°, 162°,
/// 90°); `radius` is the maximum dot radius in pixels.
pub fn color_halftone(src: &Src, radius: f32) -> Img {
    let cell = (radius * 2.0).max(2.0);
    let angles = [108.0f32, 162.0, 90.0];
    let trig: Vec<(f32, f32)> = angles.iter().map(|a| a.to_radians().sin_cos()).collect();
    let (cx, cy) = src.center();
    src.map(|x, y| {
        let p = src.at(x, y);
        if p[3] <= 0.0 {
            return p;
        }
        let a = p[3];
        let (fx, fy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let mut out = [1.0f32, 1.0, 1.0, a];
        for (ch, &(s, c)) in trig.iter().enumerate() {
            // Rotate into screen space, find the cell center, rotate back.
            let u = fx * c + fy * s;
            let v = -fx * s + fy * c;
            let cu = ((u / cell).floor() + 0.5) * cell;
            let cv = ((v / cell).floor() + 0.5) * cell;
            let sx = cu * c - cv * s + cx;
            let sy = cu * s + cv * c + cy;
            let mut ink = 0.0;
            let r = (cell * 0.25).max(0.5);
            for (ox, oy) in [(0.0, 0.0), (r, 0.0), (-r, 0.0), (0.0, r), (0.0, -r)] {
                let q = unpremul(src.sample(sx + ox, sy + oy));
                ink += 1.0 - q[ch];
            }
            ink /= 5.0;
            let dot_r = cell * 0.6 * ink.sqrt();
            let d = ((u - cu) * (u - cu) + (v - cv) * (v - cv)).sqrt();
            let cov = (dot_r - d + 0.5).clamp(0.0, 1.0);
            out[ch] = 1.0 - cov;
        }
        premul(out)
    })
}

/// Random dots of the local color on a background color.
pub fn pointillize(src: &Src, cell: u32, seed: u32, background: Rgba8) -> Img {
    let cell = cell.max(3) as i32;
    let bg = color(background);
    src.map(|x, y| {
        let ((sx, sy), d2) = nearest_seed(x, y, cell, seed);
        let jitter = 0.55 + 0.45 * rand01(sx, sy, seed ^ 0x27d4_eb2f);
        let dot_r = cell as f32 * 0.5 * jitter;
        let d = d2.sqrt();
        let cov = (dot_r - d + 0.5).clamp(0.0, 1.0);
        if cov <= 0.0 {
            return bg;
        }
        let dot = box_avg(src, sx, sy, 1);
        // Dot composited over the background.
        let under = scale4(bg, 1.0 - cov);
        add4(scale4(dot, cov), under)
    })
}
