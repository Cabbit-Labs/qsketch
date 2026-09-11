//! Image filters (the Filter menu): blur, distort, noise, pixelate, render,
//! sharpen, stylize, "other" and a set of experimental effects.
//!
//! Every filter is a pure function from a source image to an output image
//! covering the target rect; [`apply_filter`] handles reading the layer, the
//! selection mask, alpha lock and writing the result back. Kernels work on
//! **premultiplied** f32 RGBA so transparent pixels never bleed black into
//! blurs, and they run row-parallel on rayon.

pub mod adjust;
pub mod blur;
pub mod distort;
pub mod fx;
pub mod noise;
pub mod other;
pub mod pixelate;
pub mod render;
pub mod sharpen;
pub mod stylize;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::color::Rgba8;
use crate::document::DocState;
use crate::geom::IRect;
use crate::layer::Layer;
use crate::mask::Mask;
use crate::raster::{tile_rect, Raster, TILE};

// ---------------------------------------------------------------------------
// Working buffers

/// A premultiplied-alpha f32 RGBA image (`0..=1`), row-major.
#[derive(Clone, Debug)]
pub struct Img {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 4]>,
}

impl Img {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, px: vec![[0.0; 4]; w * h] }
    }

    /// Build an image by evaluating `f` for every pixel, rows in parallel.
    pub fn from_fn(w: usize, h: usize, f: impl Fn(i32, i32) -> [f32; 4] + Sync) -> Self {
        let mut px = vec![[0.0f32; 4]; w * h];
        if w > 0 {
            px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, p) in row.iter_mut().enumerate() {
                    *p = f(x as i32, y as i32);
                }
            });
        }
        Self { w, h, px }
    }

    /// Pixel at `(x, y)`, clamped to the edge.
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> [f32; 4] {
        if self.w == 0 || self.h == 0 {
            return [0.0; 4];
        }
        let x = x.clamp(0, self.w as i32 - 1) as usize;
        let y = y.clamp(0, self.h as i32 - 1) as usize;
        self.px[y * self.w + x]
    }

    /// Pixel at `(x, y)`, transparent outside the image.
    #[inline]
    pub fn get_or_clear(&self, x: i32, y: i32) -> [f32; 4] {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            [0.0; 4]
        } else {
            self.px[y as usize * self.w + x as usize]
        }
    }

    /// Bilinear sample; pixel centers sit at `n + 0.5`. Edge-clamped.
    pub fn sample(&self, fx: f32, fy: f32) -> [f32; 4] {
        self.sample_with(fx, fy, |x, y| self.get(x, y))
    }

    /// Bilinear sample fading to transparent outside the image.
    pub fn sample_or_clear(&self, fx: f32, fy: f32) -> [f32; 4] {
        self.sample_with(fx, fy, |x, y| self.get_or_clear(x, y))
    }

    #[inline]
    fn sample_with(&self, fx: f32, fy: f32, get: impl Fn(i32, i32) -> [f32; 4]) -> [f32; 4] {
        let fx = fx - 0.5;
        let fy = fy - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;
        let (x0, y0) = (x0 as i32, y0 as i32);
        let p00 = get(x0, y0);
        let p10 = get(x0 + 1, y0);
        let p01 = get(x0, y0 + 1);
        let p11 = get(x0 + 1, y0 + 1);
        let mut out = [0.0; 4];
        for c in 0..4 {
            let top = p00[c] + (p10[c] - p00[c]) * tx;
            let bot = p01[c] + (p11[c] - p01[c]) * tx;
            out[c] = top + (bot - top) * ty;
        }
        out
    }
}

/// The source a kernel reads from: the target rect plus a margin of context
/// pixels around it (clipped to the layer). Output coordinates are relative to
/// the target rect; `(0, 0)` is its top-left corner.
#[derive(Clone, Copy)]
pub struct Src<'a> {
    pub img: &'a Img,
    /// Offset of the output rect inside `img`.
    pub ox: i32,
    pub oy: i32,
    /// Output size.
    pub w: usize,
    pub h: usize,
}

impl<'a> Src<'a> {
    /// Source pixel, clamped to the available context.
    #[inline]
    pub fn at(&self, x: i32, y: i32) -> [f32; 4] {
        self.img.get(x + self.ox, y + self.oy)
    }

    /// Source pixel, transparent outside the output rect.
    #[inline]
    pub fn at_or_clear(&self, x: i32, y: i32) -> [f32; 4] {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            [0.0; 4]
        } else {
            self.at(x, y)
        }
    }

    /// Source pixel, wrapping around the output rect.
    #[inline]
    pub fn at_wrap(&self, x: i32, y: i32) -> [f32; 4] {
        self.at(x.rem_euclid(self.w.max(1) as i32), y.rem_euclid(self.h.max(1) as i32))
    }

    /// Bilinear sample (edge-clamped).
    #[inline]
    pub fn sample(&self, fx: f32, fy: f32) -> [f32; 4] {
        self.img.sample(fx + self.ox as f32, fy + self.oy as f32)
    }

    /// Bilinear sample fading to transparent outside the output rect.
    #[inline]
    pub fn sample_or_clear(&self, fx: f32, fy: f32) -> [f32; 4] {
        if fx < 0.0 || fy < 0.0 || fx > self.w as f32 || fy > self.h as f32 {
            [0.0; 4]
        } else {
            self.sample(fx, fy)
        }
    }

    /// Output-sized image from a per-pixel function.
    pub fn map(&self, f: impl Fn(i32, i32) -> [f32; 4] + Sync) -> Img {
        Img::from_fn(self.w, self.h, f)
    }

    /// The unmodified output region.
    pub fn copy(&self) -> Img {
        self.map(|x, y| self.at(x, y))
    }

    /// Crop an image the size of `self.img` down to the output rect.
    pub fn crop(&self, full: &Img) -> Img {
        Img::from_fn(self.w, self.h, |x, y| full.get(x + self.ox, y + self.oy))
    }

    /// Same geometry over a different full-size image.
    pub fn with_img<'b>(&self, img: &'b Img) -> Src<'b> {
        Src { img, ox: self.ox, oy: self.oy, w: self.w, h: self.h }
    }

    /// Center of the output rect in output coordinates.
    pub fn center(&self) -> (f32, f32) {
        (self.w as f32 * 0.5, self.h as f32 * 0.5)
    }
}

// ---------------------------------------------------------------------------
// Pixel helpers shared by the kernels

#[inline]
pub fn premul(c: [f32; 4]) -> [f32; 4] {
    let a = c[3].clamp(0.0, 1.0);
    [c[0].clamp(0.0, 1.0) * a, c[1].clamp(0.0, 1.0) * a, c[2].clamp(0.0, 1.0) * a, a]
}

#[inline]
pub fn unpremul(p: [f32; 4]) -> [f32; 4] {
    if p[3] <= 1e-6 {
        [0.0, 0.0, 0.0, 0.0]
    } else {
        [(p[0] / p[3]).clamp(0.0, 1.0), (p[1] / p[3]).clamp(0.0, 1.0), (p[2] / p[3]).clamp(0.0, 1.0), p[3].min(1.0)]
    }
}

/// Rec. 601 luma of a straight-alpha color (matches [`Rgba8::luma`]).
#[inline]
pub fn luma(c: [f32; 4]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

/// Premultiplied f32 from an 8-bit color.
#[inline]
pub fn color(c: Rgba8) -> [f32; 4] {
    premul(c.to_f32())
}

#[inline]
pub fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

#[inline]
pub fn add4(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

#[inline]
pub fn scale4(a: [f32; 4], k: f32) -> [f32; 4] {
    [a[0] * k, a[1] * k, a[2] * k, a[3] * k]
}

/// Clamp a premultiplied pixel so no channel exceeds its alpha.
#[inline]
pub fn clamp_premul(p: [f32; 4]) -> [f32; 4] {
    let a = p[3].clamp(0.0, 1.0);
    [p[0].clamp(0.0, a), p[1].clamp(0.0, a), p[2].clamp(0.0, a), a]
}

/// Apply `f` to the straight RGB of a premultiplied pixel, keeping its alpha.
#[inline]
pub fn map_rgb(p: [f32; 4], f: impl FnOnce([f32; 3]) -> [f32; 3]) -> [f32; 4] {
    if p[3] <= 1e-6 {
        return [0.0; 4];
    }
    let c = unpremul(p);
    let r = f([c[0], c[1], c[2]]);
    premul([r[0], r[1], r[2], c[3]])
}

#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Stateless per-pixel hash (so kernels stay parallel and deterministic).
#[inline]
pub fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_add(0x9e37_79b9).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    h
}

/// Uniform `0..1` from a per-pixel hash.
#[inline]
pub fn rand01(x: i32, y: i32, seed: u32) -> f32 {
    (hash(x, y, seed) >> 8) as f32 / (1u32 << 24) as f32
}

// ---------------------------------------------------------------------------
// Layer I/O

/// Read `rect` (which must lie inside the raster) into a premultiplied image.
pub fn read_region(r: &Raster, rect: IRect) -> Img {
    let mut img = Img::new(rect.w.max(0) as usize, rect.h.max(0) as usize);
    if rect.is_empty() {
        return img;
    }
    let w = img.w;
    for (tx, ty) in r.tiles_in_rect(rect) {
        let Some(t) = r.tile(tx, ty) else { continue };
        let tr = tile_rect(tx, ty);
        let sub = rect.intersect(&tr);
        for y in sub.y..sub.bottom() {
            let ly = (y - tr.y) as usize;
            let row = &t.px[ly * TILE * 4..(ly + 1) * TILE * 4];
            let out = &mut img.px[(y - rect.y) as usize * w..][..w];
            for x in sub.x..sub.right() {
                let lx = (x - tr.x) as usize * 4;
                let a = row[lx + 3] as f32 / 255.0;
                out[(x - rect.x) as usize] =
                    [row[lx] as f32 / 255.0 * a, row[lx + 1] as f32 / 255.0 * a, row[lx + 2] as f32 / 255.0 * a, a];
            }
        }
    }
    img
}

/// Write `out` (sized like `rect`) back into the layer, blending by selection
/// coverage and honoring alpha lock.
pub fn write_region(layer: &mut Layer, rect: IRect, out: &Img, sel: Option<&Mask>) {
    debug_assert_eq!((out.w, out.h), (rect.w as usize, rect.h as usize));
    let alpha_lock = layer.props.alpha_locked;
    let mut made_transparent = false;
    for (tx, ty) in layer.raster.tiles_in_rect(rect) {
        let tr = tile_rect(tx, ty);
        let sub = rect.intersect(&tr);
        if layer.raster.tile(tx, ty).is_none() {
            if alpha_lock {
                continue;
            }
            // Don't allocate a tile that would stay fully transparent.
            let any = (sub.y..sub.bottom()).any(|y| {
                let row = &out.px[(y - rect.y) as usize * out.w..];
                (sub.x..sub.right()).any(|x| row[(x - rect.x) as usize][3] > 0.002)
            });
            if !any {
                continue;
            }
        }
        let t = layer.raster.tile_mut(tx, ty);
        for y in sub.y..sub.bottom() {
            let row = &out.px[(y - rect.y) as usize * out.w..];
            for x in sub.x..sub.right() {
                let cov = sel.map_or(1.0, |m| m.coverage(x, y));
                if cov <= 0.0 {
                    continue;
                }
                let i = ((y - tr.y) as usize * TILE + (x - tr.x) as usize) * 4;
                let o = [
                    t.px[i] as f32 / 255.0,
                    t.px[i + 1] as f32 / 255.0,
                    t.px[i + 2] as f32 / 255.0,
                    t.px[i + 3] as f32 / 255.0,
                ];
                let mut n = unpremul(row[(x - rect.x) as usize]);
                if alpha_lock {
                    if o[3] <= 0.0 {
                        continue;
                    }
                    n[3] = o[3];
                }
                let f = if cov >= 1.0 { n } else { lerp4(o, n, cov) };
                let c = Rgba8::from_f32(f);
                if c.a == 0 {
                    made_transparent = true;
                }
                t.px[i..i + 4].copy_from_slice(&c.to_array());
            }
        }
    }
    if made_transparent {
        layer.raster.prune_empty_tiles();
    }
}

// ---------------------------------------------------------------------------
// The filter catalogue

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RadialMode {
    Spin,
    Zoom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OffsetEdge {
    Wrap,
    Transparent,
    Repeat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DitherPattern {
    None,
    Bayer2,
    Bayer4,
    Bayer8,
    Noise,
}

/// A filter with its parameters. Colors are straight-alpha 8-bit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Filter {
    // Adjustments (Image ▸ Adjustments; see `adjust`)
    HueSaturation { hue: f32, saturation: f32, lightness: f32, colorize: bool },
    // Blur
    GaussianBlur { radius: f32 },
    BoxBlur { radius: u32 },
    MotionBlur { angle: f32, distance: f32 },
    RadialBlur { amount: f32, mode: RadialMode },
    // Distort
    Ripple { amplitude: f32, wavelength: f32 },
    Wave { wavelength: f32, amplitude: f32, angle: f32 },
    Twirl { angle: f32 },
    Spherize { amount: f32 },
    ZigZag { amount: f32, ridges: u32 },
    PolarCoordinates { to_polar: bool },
    // Noise
    AddNoise { amount: f32, monochrome: bool, gaussian: bool, seed: u32 },
    Median { radius: u32 },
    DustAndScratches { radius: u32, threshold: u32 },
    // Pixelate
    Mosaic { cell: u32 },
    Crystallize { cell: u32, seed: u32 },
    Fragment,
    ColorHalftone { radius: f32 },
    Pointillize { cell: u32, seed: u32, background: Rgba8 },
    // Render
    Clouds { scale: f32, seed: u32, color_a: Rgba8, color_b: Rgba8 },
    DifferenceClouds { scale: f32, seed: u32, color_a: Rgba8, color_b: Rgba8 },
    // Sharpen
    Sharpen,
    SharpenMore,
    UnsharpMask { amount: f32, radius: f32, threshold: u32 },
    // Stylize
    FindEdges,
    Emboss { angle: f32, height: f32, amount: f32 },
    Solarize,
    Diffuse { distance: u32, seed: u32 },
    OilPaint { radius: u32, levels: u32 },
    Wind { strength: u32, from_left: bool, seed: u32 },
    // Other
    HighPass { radius: f32 },
    Maximum { radius: u32 },
    Minimum { radius: u32 },
    Offset { dx: i32, dy: i32, edge: OffsetEdge },
    // Experimental
    ChromaticAberration { amount: f32, radial: bool, angle: f32 },
    Dither { levels: u32, pattern: DitherPattern },
    PixelSort { threshold: f32, vertical: bool, reverse: bool },
    Scanlines { spacing: u32, darkness: f32, rgb_mask: bool },
    Vignette { amount: f32, softness: f32, color: Rgba8 },
    Glow { radius: f32, intensity: f32, threshold: f32 },
    Kaleidoscope { segments: u32, angle: f32 },
    Outline { width: u32, color: Rgba8, inside: bool },
    Glitch { amount: f32, seed: u32 },
    PencilSketch { radius: f32, strength: f32 },
}

impl Filter {
    /// Stable identifier for one filter kind (used to remember last-used
    /// parameters per filter).
    pub fn id(&self) -> &'static str {
        match self {
            Filter::HueSaturation { .. } => "hue_saturation",
            Filter::GaussianBlur { .. } => "gaussian_blur",
            Filter::BoxBlur { .. } => "box_blur",
            Filter::MotionBlur { .. } => "motion_blur",
            Filter::RadialBlur { .. } => "radial_blur",
            Filter::Ripple { .. } => "ripple",
            Filter::Wave { .. } => "wave",
            Filter::Twirl { .. } => "twirl",
            Filter::Spherize { .. } => "spherize",
            Filter::ZigZag { .. } => "zigzag",
            Filter::PolarCoordinates { .. } => "polar_coordinates",
            Filter::AddNoise { .. } => "add_noise",
            Filter::Median { .. } => "median",
            Filter::DustAndScratches { .. } => "dust_and_scratches",
            Filter::Mosaic { .. } => "mosaic",
            Filter::Crystallize { .. } => "crystallize",
            Filter::Fragment => "fragment",
            Filter::ColorHalftone { .. } => "color_halftone",
            Filter::Pointillize { .. } => "pointillize",
            Filter::Clouds { .. } => "clouds",
            Filter::DifferenceClouds { .. } => "difference_clouds",
            Filter::Sharpen => "sharpen",
            Filter::SharpenMore => "sharpen_more",
            Filter::UnsharpMask { .. } => "unsharp_mask",
            Filter::FindEdges => "find_edges",
            Filter::Emboss { .. } => "emboss",
            Filter::Solarize => "solarize",
            Filter::Diffuse { .. } => "diffuse",
            Filter::OilPaint { .. } => "oil_paint",
            Filter::Wind { .. } => "wind",
            Filter::HighPass { .. } => "high_pass",
            Filter::Maximum { .. } => "maximum",
            Filter::Minimum { .. } => "minimum",
            Filter::Offset { .. } => "offset",
            Filter::ChromaticAberration { .. } => "chromatic_aberration",
            Filter::Dither { .. } => "dither",
            Filter::PixelSort { .. } => "pixel_sort",
            Filter::Scanlines { .. } => "scanlines",
            Filter::Vignette { .. } => "vignette",
            Filter::Glow { .. } => "glow",
            Filter::Kaleidoscope { .. } => "kaleidoscope",
            Filter::Outline { .. } => "outline",
            Filter::Glitch { .. } => "glitch",
            Filter::PencilSketch { .. } => "pencil_sketch",
        }
    }

    /// Human-readable name (history label, dialog title).
    pub fn name(&self) -> &'static str {
        match self {
            Filter::HueSaturation { .. } => "Hue/Saturation",
            Filter::GaussianBlur { .. } => "Gaussian Blur",
            Filter::BoxBlur { .. } => "Box Blur",
            Filter::MotionBlur { .. } => "Motion Blur",
            Filter::RadialBlur { .. } => "Radial Blur",
            Filter::Ripple { .. } => "Ripple",
            Filter::Wave { .. } => "Wave",
            Filter::Twirl { .. } => "Twirl",
            Filter::Spherize { .. } => "Spherize",
            Filter::ZigZag { .. } => "ZigZag",
            Filter::PolarCoordinates { .. } => "Polar Coordinates",
            Filter::AddNoise { .. } => "Add Noise",
            Filter::Median { .. } => "Median",
            Filter::DustAndScratches { .. } => "Dust & Scratches",
            Filter::Mosaic { .. } => "Mosaic",
            Filter::Crystallize { .. } => "Crystallize",
            Filter::Fragment => "Fragment",
            Filter::ColorHalftone { .. } => "Color Halftone",
            Filter::Pointillize { .. } => "Pointillize",
            Filter::Clouds { .. } => "Clouds",
            Filter::DifferenceClouds { .. } => "Difference Clouds",
            Filter::Sharpen => "Sharpen",
            Filter::SharpenMore => "Sharpen More",
            Filter::UnsharpMask { .. } => "Unsharp Mask",
            Filter::FindEdges => "Find Edges",
            Filter::Emboss { .. } => "Emboss",
            Filter::Solarize => "Solarize",
            Filter::Diffuse { .. } => "Diffuse",
            Filter::OilPaint { .. } => "Oil Paint",
            Filter::Wind { .. } => "Wind",
            Filter::HighPass { .. } => "High Pass",
            Filter::Maximum { .. } => "Maximum",
            Filter::Minimum { .. } => "Minimum",
            Filter::Offset { .. } => "Offset",
            Filter::ChromaticAberration { .. } => "Chromatic Aberration",
            Filter::Dither { .. } => "Dither",
            Filter::PixelSort { .. } => "Pixel Sort",
            Filter::Scanlines { .. } => "Scanlines",
            Filter::Vignette { .. } => "Vignette",
            Filter::Glow { .. } => "Glow",
            Filter::Kaleidoscope { .. } => "Kaleidoscope",
            Filter::Outline { .. } => "Outline",
            Filter::Glitch { .. } => "Glitch",
            Filter::PencilSketch { .. } => "Pencil Sketch",
        }
    }

    /// False for filters that apply immediately without a dialog.
    pub fn has_params(&self) -> bool {
        !matches!(self, Filter::Fragment | Filter::Sharpen | Filter::SharpenMore | Filter::FindEdges | Filter::Solarize)
    }

    /// Context pixels the kernel reads beyond the target rect.
    pub fn margin(&self) -> i32 {
        let g = blur::gaussian_margin;
        match *self {
            Filter::HueSaturation { .. } => 0,
            Filter::GaussianBlur { radius } => g(radius),
            Filter::BoxBlur { radius } => radius as i32,
            Filter::MotionBlur { distance, .. } => (distance * 0.5).ceil() as i32 + 1,
            Filter::RadialBlur { .. } => 0,
            Filter::Ripple { amplitude, .. } => amplitude.abs().ceil() as i32 + 1,
            Filter::Wave { amplitude, .. } => amplitude.abs().ceil() as i32 + 1,
            Filter::Twirl { .. } | Filter::Spherize { .. } | Filter::PolarCoordinates { .. } => 0,
            Filter::ZigZag { amount, .. } => amount.abs().ceil() as i32 + 1,
            Filter::AddNoise { .. } => 0,
            Filter::Median { radius } => radius as i32,
            Filter::DustAndScratches { radius, .. } => radius as i32,
            Filter::Mosaic { .. } => 0,
            Filter::Crystallize { cell, .. } => cell as i32 + 2,
            Filter::Fragment => 4,
            Filter::ColorHalftone { radius } => (radius * 2.0).ceil() as i32 + 2,
            Filter::Pointillize { cell, .. } => cell as i32 + 2,
            Filter::Clouds { .. } | Filter::DifferenceClouds { .. } => 0,
            Filter::Sharpen | Filter::SharpenMore => 1,
            Filter::UnsharpMask { radius, .. } => g(radius),
            Filter::FindEdges => 1,
            Filter::Emboss { height, .. } => height.abs().ceil() as i32 + 1,
            Filter::Solarize => 0,
            Filter::Diffuse { distance, .. } => distance as i32 + 1,
            Filter::OilPaint { radius, .. } => radius as i32,
            Filter::Wind { strength, .. } => strength as i32,
            Filter::HighPass { radius } => g(radius),
            Filter::Maximum { radius } | Filter::Minimum { radius } => radius as i32,
            Filter::Offset { .. } => 0,
            Filter::ChromaticAberration { amount, .. } => amount.abs().ceil() as i32 + 1,
            Filter::Dither { .. } | Filter::PixelSort { .. } | Filter::Scanlines { .. } | Filter::Vignette { .. } => 0,
            Filter::Glow { radius, .. } => g(radius),
            Filter::Kaleidoscope { .. } => 0,
            Filter::Outline { width, .. } => width as i32 + 2,
            Filter::Glitch { .. } => 0,
            Filter::PencilSketch { radius, .. } => g(radius),
        }
    }

    /// Run the kernel. The result is the size of the output rect.
    pub fn run(&self, src: &Src) -> Img {
        match self {
            Filter::HueSaturation { hue, saturation, lightness, colorize } => {
                adjust::hue_saturation(src, *hue, *saturation, *lightness, *colorize)
            }
            Filter::GaussianBlur { radius } => blur::gaussian(src, *radius),
            Filter::BoxBlur { radius } => blur::box_blur(src, *radius),
            Filter::MotionBlur { angle, distance } => blur::motion(src, *angle, *distance),
            Filter::RadialBlur { amount, mode } => blur::radial(src, *amount, *mode),
            Filter::Ripple { amplitude, wavelength } => distort::ripple(src, *amplitude, *wavelength),
            Filter::Wave { wavelength, amplitude, angle } => distort::wave(src, *wavelength, *amplitude, *angle),
            Filter::Twirl { angle } => distort::twirl(src, *angle),
            Filter::Spherize { amount } => distort::spherize(src, *amount),
            Filter::ZigZag { amount, ridges } => distort::zigzag(src, *amount, *ridges),
            Filter::PolarCoordinates { to_polar } => distort::polar(src, *to_polar),
            Filter::AddNoise { amount, monochrome, gaussian, seed } => {
                noise::add_noise(src, *amount, *monochrome, *gaussian, *seed)
            }
            Filter::Median { radius } => noise::median(src, *radius, 0),
            Filter::DustAndScratches { radius, threshold } => noise::median(src, *radius, *threshold),
            Filter::Mosaic { cell } => pixelate::mosaic(src, *cell),
            Filter::Crystallize { cell, seed } => pixelate::crystallize(src, *cell, *seed),
            Filter::Fragment => pixelate::fragment(src),
            Filter::ColorHalftone { radius } => pixelate::color_halftone(src, *radius),
            Filter::Pointillize { cell, seed, background } => pixelate::pointillize(src, *cell, *seed, *background),
            Filter::Clouds { scale, seed, color_a, color_b } => {
                render::clouds(src, *scale, *seed, *color_a, *color_b, false)
            }
            Filter::DifferenceClouds { scale, seed, color_a, color_b } => {
                render::clouds(src, *scale, *seed, *color_a, *color_b, true)
            }
            Filter::Sharpen => sharpen::sharpen(src, 0.5),
            Filter::SharpenMore => sharpen::sharpen(src, 1.25),
            Filter::UnsharpMask { amount, radius, threshold } => sharpen::unsharp(src, *amount, *radius, *threshold),
            Filter::FindEdges => stylize::find_edges(src),
            Filter::Emboss { angle, height, amount } => stylize::emboss(src, *angle, *height, *amount),
            Filter::Solarize => stylize::solarize(src),
            Filter::Diffuse { distance, seed } => stylize::diffuse(src, *distance, *seed),
            Filter::OilPaint { radius, levels } => stylize::oil_paint(src, *radius, *levels),
            Filter::Wind { strength, from_left, seed } => stylize::wind(src, *strength, *from_left, *seed),
            Filter::HighPass { radius } => other::high_pass(src, *radius),
            Filter::Maximum { radius } => other::max_min(src, *radius, true),
            Filter::Minimum { radius } => other::max_min(src, *radius, false),
            Filter::Offset { dx, dy, edge } => other::offset(src, *dx, *dy, *edge),
            Filter::ChromaticAberration { amount, radial, angle } => {
                fx::chromatic_aberration(src, *amount, *radial, *angle)
            }
            Filter::Dither { levels, pattern } => fx::dither(src, *levels, *pattern),
            Filter::PixelSort { threshold, vertical, reverse } => fx::pixel_sort(src, *threshold, *vertical, *reverse),
            Filter::Scanlines { spacing, darkness, rgb_mask } => fx::scanlines(src, *spacing, *darkness, *rgb_mask),
            Filter::Vignette { amount, softness, color } => fx::vignette(src, *amount, *softness, *color),
            Filter::Glow { radius, intensity, threshold } => fx::glow(src, *radius, *intensity, *threshold),
            Filter::Kaleidoscope { segments, angle } => fx::kaleidoscope(src, *segments, *angle),
            Filter::Outline { width, color, inside } => fx::outline(src, *width, *color, *inside),
            Filter::Glitch { amount, seed } => fx::glitch(src, *amount, *seed),
            Filter::PencilSketch { radius, strength } => fx::pencil_sketch(src, *radius, *strength),
        }
    }
}

/// Apply a filter to a layer within the selection (or the whole layer when
/// nothing is selected). Returns the dirty rect.
pub fn apply_filter(doc: &mut DocState, layer_idx: usize, filter: &Filter) -> IRect {
    let sel = doc.selection.clone();
    let Some(layer) = doc.layers.get_mut(layer_idx) else { return IRect::EMPTY };
    let full = layer.raster.rect();
    let rect = match &sel {
        Some(m) => m.bounds().intersect(&full),
        None => full,
    };
    if rect.is_empty() {
        return IRect::EMPTY;
    }
    let src_rect = rect.expand(filter.margin().max(0)).intersect(&full);
    let img = read_region(&layer.raster, src_rect);
    let src =
        Src { img: &img, ox: rect.x - src_rect.x, oy: rect.y - src_rect.y, w: rect.w as usize, h: rect.h as usize };
    let out = filter.run(&src);
    write_region(layer, rect, &out, sel.as_deref());
    rect
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn checker(w: u32, h: u32) -> DocState {
        let mut d = DocState::new(w, h, None);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let c = if (x / 8 + y / 8) % 2 == 0 { Rgba8::WHITE } else { Rgba8::BLACK };
                d.layers[0].raster.set_pixel(x, y, c);
            }
        }
        d
    }

    #[test]
    fn region_roundtrip() {
        let d = checker(70, 50);
        let rect = IRect::new(3, 5, 60, 40);
        let img = read_region(&d.layers[0].raster, rect);
        assert_eq!((img.w, img.h), (60, 40));
        assert_eq!(img.get(0, 0), [1.0, 1.0, 1.0, 1.0]);
        // Writing the same image back changes nothing.
        let mut d2 = d.clone();
        write_region(&mut d2.layers[0], rect, &img, None);
        assert_eq!(d.layers[0].raster.to_rgba(), d2.layers[0].raster.to_rgba());
    }

    #[test]
    fn blur_respects_selection_and_alpha_lock() {
        let mut d = checker(64, 64);
        d.selection = Some(Arc::new(Mask::from_rect(64, 64, IRect::new(0, 0, 32, 64))));
        let r = apply_filter(&mut d, 0, &Filter::GaussianBlur { radius: 6.0 });
        assert_eq!(r, IRect::new(0, 0, 32, 64));
        let px = d.layers[0].raster.get_pixel(8, 8);
        assert!(px.r > 20 && px.r < 235, "blurred inside selection: {px:?}");
        assert_eq!(d.layers[0].raster.get_pixel(56, 8), Rgba8::WHITE);

        // Alpha lock keeps transparency: a blur can't grow the shape.
        let mut d = DocState::new(64, 64, None);
        d.layers[0].raster.fill_rect(IRect::new(20, 20, 10, 10), Rgba8::WHITE);
        d.layers[0].props.alpha_locked = true;
        apply_filter(&mut d, 0, &Filter::GaussianBlur { radius: 8.0 });
        assert_eq!(d.layers[0].raster.get_pixel(10, 10), Rgba8::TRANSPARENT);
        assert_eq!(d.layers[0].raster.get_pixel(25, 25).a, 255);
    }

    #[test]
    fn transparent_layers_stay_sparse() {
        let mut d = DocState::new(128, 128, None);
        d.layers[0].raster.set_pixel(96, 96, Rgba8::WHITE);
        apply_filter(&mut d, 0, &Filter::GaussianBlur { radius: 2.0 });
        assert!(d.layers[0].raster.tile(0, 0).is_none(), "untouched corner tile allocated");
        apply_filter(&mut d, 0, &Filter::Solarize);
    }

    #[test]
    fn every_filter_runs() {
        // Smoke-test the whole catalogue on a small document, with and without
        // a selection, and make sure the dirty rect is sane.
        let all = vec![
            Filter::HueSaturation { hue: 90.0, saturation: 20.0, lightness: -10.0, colorize: false },
            Filter::HueSaturation { hue: 200.0, saturation: 25.0, lightness: 0.0, colorize: true },
            Filter::GaussianBlur { radius: 3.0 },
            Filter::BoxBlur { radius: 2 },
            Filter::MotionBlur { angle: 30.0, distance: 10.0 },
            Filter::RadialBlur { amount: 20.0, mode: RadialMode::Spin },
            Filter::RadialBlur { amount: 20.0, mode: RadialMode::Zoom },
            Filter::Ripple { amplitude: 4.0, wavelength: 12.0 },
            Filter::Wave { wavelength: 20.0, amplitude: 5.0, angle: 30.0 },
            Filter::Twirl { angle: 90.0 },
            Filter::Spherize { amount: 60.0 },
            Filter::Spherize { amount: -60.0 },
            Filter::ZigZag { amount: 5.0, ridges: 4 },
            Filter::PolarCoordinates { to_polar: true },
            Filter::PolarCoordinates { to_polar: false },
            Filter::AddNoise { amount: 0.3, monochrome: false, gaussian: true, seed: 1 },
            Filter::Median { radius: 2 },
            Filter::DustAndScratches { radius: 2, threshold: 20 },
            Filter::Mosaic { cell: 6 },
            Filter::Crystallize { cell: 8, seed: 3 },
            Filter::Fragment,
            Filter::ColorHalftone { radius: 4.0 },
            Filter::Pointillize { cell: 7, seed: 2, background: Rgba8::WHITE },
            Filter::Clouds { scale: 32.0, seed: 1, color_a: Rgba8::BLACK, color_b: Rgba8::WHITE },
            Filter::DifferenceClouds { scale: 32.0, seed: 1, color_a: Rgba8::BLACK, color_b: Rgba8::WHITE },
            Filter::Sharpen,
            Filter::SharpenMore,
            Filter::UnsharpMask { amount: 1.0, radius: 2.0, threshold: 4 },
            Filter::FindEdges,
            Filter::Emboss { angle: 135.0, height: 2.0, amount: 1.0 },
            Filter::Solarize,
            Filter::Diffuse { distance: 3, seed: 0 },
            Filter::OilPaint { radius: 3, levels: 16 },
            Filter::Wind { strength: 8, from_left: true, seed: 0 },
            Filter::HighPass { radius: 5.0 },
            Filter::Maximum { radius: 2 },
            Filter::Minimum { radius: 2 },
            Filter::Offset { dx: 10, dy: -5, edge: OffsetEdge::Wrap },
            Filter::Offset { dx: 10, dy: -5, edge: OffsetEdge::Transparent },
            Filter::Offset { dx: 10, dy: -5, edge: OffsetEdge::Repeat },
            Filter::ChromaticAberration { amount: 3.0, radial: true, angle: 0.0 },
            Filter::Dither { levels: 2, pattern: DitherPattern::Bayer4 },
            Filter::PixelSort { threshold: 0.4, vertical: false, reverse: false },
            Filter::PixelSort { threshold: 0.4, vertical: true, reverse: true },
            Filter::Scanlines { spacing: 4, darkness: 0.5, rgb_mask: true },
            Filter::Vignette { amount: 0.8, softness: 0.6, color: Rgba8::BLACK },
            Filter::Glow { radius: 6.0, intensity: 1.0, threshold: 0.5 },
            Filter::Kaleidoscope { segments: 6, angle: 15.0 },
            Filter::Outline { width: 3, color: Rgba8::BLACK, inside: false },
            Filter::Outline { width: 3, color: Rgba8::BLACK, inside: true },
            Filter::Glitch { amount: 12.0, seed: 4 },
            Filter::PencilSketch { radius: 4.0, strength: 1.5 },
        ];
        for f in &all {
            let mut d = checker(70, 50);
            let r = apply_filter(&mut d, 0, f);
            assert_eq!(r, IRect::new(0, 0, 70, 50), "{}", f.name());
            for y in 0..50 {
                for x in 0..70 {
                    let _ = d.layers[0].raster.get_pixel(x, y);
                }
            }
            let mut d = checker(70, 50);
            d.selection = Some(Arc::new(Mask::from_ellipse(70, 50, IRect::new(10, 10, 40, 30))));
            let r = apply_filter(&mut d, 0, f);
            assert_eq!(r, IRect::new(10, 10, 40, 30), "{} with selection", f.name());
            // Pixels well outside the ellipse are untouched.
            let c = d.layers[0].raster.get_pixel(1, 1);
            assert!(c == Rgba8::WHITE, "{} leaked outside the selection: {c:?}", f.name());
            assert!(!f.id().is_empty() && !f.name().is_empty());
        }
    }

    #[test]
    fn outline_grows_shape_and_offset_wraps() {
        let mut d = DocState::new(64, 64, None);
        d.layers[0].raster.fill_rect(IRect::new(20, 20, 10, 10), Rgba8::WHITE);
        apply_filter(&mut d, 0, &Filter::Outline { width: 4, color: Rgba8::BLACK, inside: false });
        assert_eq!(d.layers[0].raster.get_pixel(25, 25), Rgba8::WHITE);
        let edge = d.layers[0].raster.get_pixel(17, 25);
        assert!(edge.a > 200 && edge.r < 30, "outline pixel {edge:?}");
        assert_eq!(d.layers[0].raster.get_pixel(5, 5), Rgba8::TRANSPARENT);

        let mut d = DocState::new(16, 16, Some(Rgba8::BLACK));
        d.layers[0].raster.set_pixel(0, 0, Rgba8::WHITE);
        apply_filter(&mut d, 0, &Filter::Offset { dx: 3, dy: 2, edge: OffsetEdge::Wrap });
        assert_eq!(d.layers[0].raster.get_pixel(3, 2), Rgba8::WHITE);
        apply_filter(&mut d, 0, &Filter::Offset { dx: -3, dy: -2, edge: OffsetEdge::Wrap });
        assert_eq!(d.layers[0].raster.get_pixel(0, 0), Rgba8::WHITE);
    }
}
