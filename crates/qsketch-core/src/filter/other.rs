//! "Other" filters: high pass, maximum / minimum, offset.

use super::{blur, clamp_premul, Img, OffsetEdge, Src};

/// Source minus its Gaussian blur, around mid-gray.
pub fn high_pass(src: &Src, radius: f32) -> Img {
    let blurred = blur::gaussian(src, radius);
    src.map(|x, y| {
        let c = src.at(x, y);
        if c[3] <= 0.0 {
            return c;
        }
        let b = blurred.get(x, y);
        let mut out = c;
        for ch in 0..3 {
            out[ch] = 0.5 * c[3] + (c[ch] - b[ch]);
        }
        clamp_premul(out)
    })
}

/// Separable square dilation (`max`) or erosion (`!max`) of every channel,
/// alpha included, so shapes grow or shrink.
pub fn max_min(src: &Src, radius: u32, max: bool) -> Img {
    let r = radius as i32;
    if r == 0 {
        return src.copy();
    }
    let pick = |a: [f32; 4], b: [f32; 4]| -> [f32; 4] {
        if max {
            [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2]), a[3].max(b[3])]
        } else {
            [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2]), a[3].min(b[3])]
        }
    };
    let img = src.img;
    let h = Img::from_fn(img.w, img.h, |x, y| {
        let mut acc = img.get(x, y);
        for d in 1..=r {
            acc = pick(acc, pick(img.get(x - d, y), img.get(x + d, y)));
        }
        acc
    });
    let v = Img::from_fn(img.w, img.h, |x, y| {
        let mut acc = h.get(x, y);
        for d in 1..=r {
            acc = pick(acc, pick(h.get(x, y - d), h.get(x, y + d)));
        }
        acc
    });
    src.crop(&v)
}

/// Shift the content by `(dx, dy)`; the uncovered area wraps, goes
/// transparent or repeats the edge pixels.
pub fn offset(src: &Src, dx: i32, dy: i32, edge: OffsetEdge) -> Img {
    src.map(|x, y| {
        let (sx, sy) = (x - dx, y - dy);
        match edge {
            OffsetEdge::Wrap => src.at_wrap(sx, sy),
            OffsetEdge::Transparent => src.at_or_clear(sx, sy),
            OffsetEdge::Repeat => src.at(sx.clamp(0, src.w as i32 - 1), sy.clamp(0, src.h as i32 - 1)),
        }
    })
}
