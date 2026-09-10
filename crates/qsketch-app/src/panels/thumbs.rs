//! Small cached thumbnails (egui textures) for layers and the navigator.

use std::collections::HashMap;

use egui::{ColorImage, TextureHandle, TextureOptions};
use qsketch_core::{Composite, Raster};

#[derive(Default)]
pub struct ThumbCache {
    entries: HashMap<(u64, u64), (u64, TextureHandle)>,
}

impl ThumbCache {
    /// Get or rebuild a thumbnail for `key` at `generation`.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        key: (u64, u64),
        generation: u64,
        max: [usize; 2],
        build: impl FnOnce([usize; 2]) -> ColorImage,
    ) -> TextureHandle {
        if let Some((g, h)) = self.entries.get(&key) {
            if *g == generation {
                return h.clone();
            }
        }
        let img = build(max);
        let handle = ctx.load_texture(format!("thumb-{}-{}", key.0, key.1), img, TextureOptions::LINEAR);
        self.entries.insert(key, (generation, handle.clone()));
        handle
    }

    pub fn retain(&mut self, keep: impl Fn(&(u64, u64)) -> bool) {
        self.entries.retain(|k, _| keep(k));
    }
}

/// Thumbnail size that fits `max` while keeping the aspect of `w×h`.
pub fn fit_size(w: u32, h: u32, max: [usize; 2]) -> [usize; 2] {
    let sx = max[0] as f32 / w.max(1) as f32;
    let sy = max[1] as f32 / h.max(1) as f32;
    let s = sx.min(sy).min(1.0);
    [((w as f32 * s).round() as usize).max(1), ((h as f32 * s).round() as usize).max(1)]
}

/// Downsample a straight-alpha raster (box filter) with a checkerboard behind.
pub fn raster_thumb(raster: &Raster, max: [usize; 2], checker: bool) -> ColorImage {
    let (w, h) = (raster.width(), raster.height());
    let size = fit_size(w, h, max);
    let mut px = Vec::with_capacity(size[0] * size[1]);
    let bx = (w as f32 / size[0] as f32).max(1.0);
    let by = (h as f32 / size[1] as f32).max(1.0);
    let samples = 2usize;
    for y in 0..size[1] {
        for x in 0..size[0] {
            let mut acc = [0f32; 4];
            for sy in 0..samples {
                for sx in 0..samples {
                    let px_x = ((x as f32 + (sx as f32 + 0.5) / samples as f32) * bx) as i32;
                    let px_y = ((y as f32 + (sy as f32 + 0.5) / samples as f32) * by) as i32;
                    let c = raster.get_pixel(px_x.min(w as i32 - 1), px_y.min(h as i32 - 1)).to_f32();
                    acc[0] += c[0] * c[3];
                    acc[1] += c[1] * c[3];
                    acc[2] += c[2] * c[3];
                    acc[3] += c[3];
                }
            }
            let n = (samples * samples) as f32;
            let a = acc[3] / n;
            let (mut r, mut g, mut b) = (acc[0] / n, acc[1] / n, acc[2] / n);
            if checker {
                let cell = ((x / 4) + (y / 4)) % 2 == 0;
                let bg = if cell { 0.75 } else { 1.0 };
                r += bg * (1.0 - a);
                g += bg * (1.0 - a);
                b += bg * (1.0 - a);
                px.push(egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8));
            } else {
                px.push(egui::Color32::from_rgba_premultiplied(
                    (r * 255.0) as u8,
                    (g * 255.0) as u8,
                    (b * 255.0) as u8,
                    (a * 255.0) as u8,
                ));
            }
        }
    }
    ColorImage::new(size, px)
}

/// Downsample the premultiplied composite with a checkerboard behind.
pub fn composite_thumb(comp: &Composite, max: [usize; 2]) -> ColorImage {
    let (w, h) = (comp.width(), comp.height());
    let size = fit_size(w, h, max);
    let mut px = Vec::with_capacity(size[0] * size[1]);
    let bx = (w as f32 / size[0] as f32).max(1.0);
    let by = (h as f32 / size[1] as f32).max(1.0);
    let samples = 2usize;
    for y in 0..size[1] {
        for x in 0..size[0] {
            let mut acc = [0f32; 4];
            for sy in 0..samples {
                for sx in 0..samples {
                    let px_x = ((x as f32 + (sx as f32 + 0.5) / samples as f32) * bx) as i32;
                    let px_y = ((y as f32 + (sy as f32 + 0.5) / samples as f32) * by) as i32;
                    let c = comp.get_premul(px_x.min(w as i32 - 1), px_y.min(h as i32 - 1));
                    for i in 0..4 {
                        acc[i] += c[i] as f32 / 255.0;
                    }
                }
            }
            let n = (samples * samples) as f32;
            let a = acc[3] / n;
            let cell = ((x / 4) + (y / 4)) % 2 == 0;
            let bg = if cell { 0.75 } else { 1.0 };
            let r = acc[0] / n + bg * (1.0 - a);
            let g = acc[1] / n + bg * (1.0 - a);
            let b = acc[2] / n + bg * (1.0 - a);
            px.push(egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8));
        }
    }
    ColorImage::new(size, px)
}
