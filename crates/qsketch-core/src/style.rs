//! Layer styles: Photoshop-style non-destructive effects on a layer (drop
//! shadow, outer glow, stroke, color overlay, inner glow).
//!
//! A style lives in the layer's props (so it is undoable and saved in the
//! `.qsk`) and never touches the layer's pixels. The compositor draws a
//! *styled* copy of the layer instead: the effects rendered around and over
//! the content, then blended with the layer's own opacity and blend mode.
//! Because effects reach beyond the pixels that changed (a shadow falls
//! several tiles away from the stroke that casts it), styled copies are kept
//! in a [`StyleCache`] and re-rendered only around the tiles that changed.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::color::Rgba8;
use crate::composite::TileSet;
use crate::document::DocState;
use crate::geom::IRect;
use crate::layer::{Layer, LayerId};
use crate::mask::Mask;
use crate::raster::{Raster, TILE};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokePosition {
    #[default]
    Outside,
    Inside,
    Center,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DropShadow {
    pub enabled: bool,
    pub color: Rgba8,
    pub opacity: f32,
    /// Light direction in degrees (Photoshop's: 120 = light from the upper left).
    pub angle: f32,
    /// Offset in pixels.
    pub distance: f32,
    /// 0..=1: how much the edge is hardened before blurring.
    pub spread: f32,
    /// Blur size in pixels.
    pub size: f32,
}

impl Default for DropShadow {
    fn default() -> Self {
        Self {
            enabled: false,
            color: Rgba8::new(0, 0, 0, 255),
            opacity: 0.75,
            angle: 120.0,
            distance: 8.0,
            spread: 0.0,
            size: 10.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Glow {
    pub enabled: bool,
    pub color: Rgba8,
    pub opacity: f32,
    pub spread: f32,
    pub size: f32,
}

impl Default for Glow {
    fn default() -> Self {
        Self { enabled: false, color: Rgba8::new(255, 255, 190, 255), opacity: 0.75, spread: 0.0, size: 12.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StrokeFx {
    pub enabled: bool,
    pub color: Rgba8,
    pub opacity: f32,
    pub size: f32,
    pub position: StrokePosition,
}

impl Default for StrokeFx {
    fn default() -> Self {
        Self {
            enabled: false,
            color: Rgba8::new(0, 0, 0, 255),
            opacity: 1.0,
            size: 3.0,
            position: StrokePosition::Outside,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorOverlay {
    pub enabled: bool,
    pub color: Rgba8,
    pub opacity: f32,
}

impl Default for ColorOverlay {
    fn default() -> Self {
        Self { enabled: false, color: Rgba8::new(255, 60, 60, 255), opacity: 1.0 }
    }
}

/// The effects of one layer. Each has its own on/off switch so turning one
/// off keeps its settings while the layer is open.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerStyle {
    pub drop_shadow: DropShadow,
    pub outer_glow: Glow,
    pub stroke: StrokeFx,
    pub color_overlay: ColorOverlay,
    pub inner_glow: Glow,
}

impl LayerStyle {
    /// No effect is on (the layer draws as plain pixels).
    pub fn is_off(&self) -> bool {
        !(self.drop_shadow.enabled
            || self.outer_glow.enabled
            || self.stroke.enabled
            || self.color_overlay.enabled
            || self.inner_glow.enabled)
    }

    /// Names of the effects that are on, for the Layers panel.
    pub fn summary(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.drop_shadow.enabled {
            v.push("Drop Shadow");
        }
        if self.outer_glow.enabled {
            v.push("Outer Glow");
        }
        if self.stroke.enabled {
            v.push("Stroke");
        }
        if self.color_overlay.enabled {
            v.push("Color Overlay");
        }
        if self.inner_glow.enabled {
            v.push("Inner Glow");
        }
        v
    }

    fn box_radius(size: f32) -> usize {
        (size / 3.0).round().max(0.0) as usize
    }

    fn shadow_offset(&self) -> (i32, i32) {
        let a = self.drop_shadow.angle.to_radians();
        let d = self.drop_shadow.distance;
        // Light comes from `angle`, so the shadow falls the other way.
        ((-a.cos() * d).round() as i32, (a.sin() * d).round() as i32)
    }

    /// How far (pixels) any effect can reach from the content that casts it.
    pub fn reach(&self) -> i32 {
        let mut r = 0i32;
        let blur = |size: f32| 3 * Self::box_radius(size) as i32 + 1;
        if self.drop_shadow.enabled {
            let (dx, dy) = self.shadow_offset();
            r = r.max(dx.abs().max(dy.abs()) + blur(self.drop_shadow.size));
        }
        if self.outer_glow.enabled {
            r = r.max(blur(self.outer_glow.size));
        }
        if self.stroke.enabled {
            r = r.max(self.stroke.size.ceil() as i32 + 1);
        }
        if self.inner_glow.enabled {
            r = r.max(self.inner_glow.size.ceil() as i32 + 1);
        }
        r + 1
    }
}

// ---------------------------------------------------------------------------
// Rendering

/// Squared Euclidean distance transform along one line (Felzenszwalb &
/// Huttenlocher): `f` holds 0 at feature pixels and a large value elsewhere.
fn edt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32);
            if s <= z[k] && k > 0 {
                k -= 1;
                continue;
            }
            if s <= z[k] {
                // k == 0: replace the first parabola.
                v[0] = q;
                z[0] = f32::NEG_INFINITY;
                z[1] = f32::INFINITY;
                break;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f32::INFINITY;
            break;
        }
    }
    k = 0;
    for (q, dq) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        let dx = q as f32 - p as f32;
        *dq = dx * dx + f[p];
    }
}

/// Distance (pixels) from each pixel to the nearest pixel where `feature` is
/// true, over a `w × h` grid. Pixels with no feature in the grid get a large
/// value.
fn distance_to(feature: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e12;
    let mut grid: Vec<f32> = feature.iter().map(|&b| if b { 0.0 } else { INF }).collect();
    let n = w.max(h);
    let (mut f, mut d) = (vec![0f32; n], vec![0f32; n]);
    let (mut v, mut z) = (vec![0usize; n], vec![0f32; n + 1]);
    for x in 0..w {
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        edt_1d(&f[..h], &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            grid[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&grid[y * w..y * w + w]);
        edt_1d(&f[..w], &mut d[..w], &mut v, &mut z);
        grid[y * w..y * w + w].copy_from_slice(&d[..w]);
    }
    grid.iter_mut().for_each(|g| *g = g.sqrt());
    grid
}

/// Three separable box-blur passes of radius `r` (about a Gaussian of the
/// same extent); values beyond the grid count as 0.
fn blur(buf: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    let norm = 1.0 / (2 * r + 1) as f32;
    let mut tmp = vec![0f32; w.max(h)];
    for _ in 0..3 {
        for y in 0..h {
            let row = &mut buf[y * w..y * w + w];
            let mut acc: f32 = row[..r.min(w)].iter().sum();
            for x in 0..w {
                if x + r < w {
                    acc += row[x + r];
                }
                tmp[x] = acc * norm;
                if x >= r {
                    acc -= row[x - r];
                }
            }
            row.copy_from_slice(&tmp[..w]);
        }
        for x in 0..w {
            let mut acc: f32 = (0..r.min(h)).map(|y| buf[y * w + x]).sum();
            for y in 0..h {
                if y + r < h {
                    acc += buf[(y + r) * w + x];
                }
                tmp[y] = acc * norm;
                if y >= r {
                    acc -= buf[(y - r) * w + x];
                }
            }
            for y in 0..h {
                buf[y * w + x] = tmp[y];
            }
        }
    }
}

#[inline]
fn over(dst: [f32; 4], rgb: [f32; 3], a: f32) -> [f32; 4] {
    if a <= 0.0 {
        return dst;
    }
    let a_o = a + dst[3] * (1.0 - a);
    if a_o <= 0.0 {
        return [0.0; 4];
    }
    let k = dst[3] * (1.0 - a);
    [(rgb[0] * a + dst[0] * k) / a_o, (rgb[1] * a + dst[1] * k) / a_o, (rgb[2] * a + dst[2] * k) / a_o, a_o]
}

fn rgb(c: Rgba8) -> [f32; 3] {
    [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0]
}

/// Render `src` (with its mask applied) and `style` into `dst` over `out`
/// (canvas pixels), reading source pixels up to `style.reach()` beyond it.
pub fn render_region(src: &Raster, mask: Option<&Mask>, style: &LayerStyle, out: IRect, dst: &mut Raster) {
    let (cw, ch) = (src.width() as i32, src.height() as i32);
    let canvas = IRect::new(0, 0, cw, ch);
    let out = out.intersect(&canvas);
    if out.is_empty() {
        return;
    }
    let reach = style.reach();
    let win = out.expand(reach).intersect(&canvas);
    let (ww, wh) = (win.w as usize, win.h as usize);
    let idx = |x: i32, y: i32| (y - win.y) as usize * ww + (x - win.x) as usize;
    // Content alpha over the window (mask applied).
    let mut alpha = vec![0f32; ww * wh];
    for y in win.y..win.bottom() {
        for x in win.x..win.right() {
            let a = src.get_pixel(x, y).a as f32 / 255.0;
            let m = mask.map_or(1.0, |m| m.get(x, y) as f32 / 255.0);
            alpha[idx(x, y)] = a * m;
        }
    }
    let spread = |a: f32, s: f32| (a / (1.0 - s.clamp(0.0, 0.99))).min(1.0);

    let shadow = style.drop_shadow.enabled.then(|| {
        let s = &style.drop_shadow;
        let (dx, dy) = style.shadow_offset();
        let mut buf = vec![0f32; ww * wh];
        for y in 0..wh as i32 {
            for x in 0..ww as i32 {
                let (sx, sy) = (x - dx, y - dy);
                if sx >= 0 && sy >= 0 && sx < ww as i32 && sy < wh as i32 {
                    buf[y as usize * ww + x as usize] = spread(alpha[sy as usize * ww + sx as usize], s.spread);
                }
            }
        }
        blur(&mut buf, ww, wh, LayerStyle::box_radius(s.size));
        buf
    });
    let glow = style.outer_glow.enabled.then(|| {
        let g = &style.outer_glow;
        let mut buf: Vec<f32> = alpha.iter().map(|&a| spread(a, g.spread)).collect();
        blur(&mut buf, ww, wh, LayerStyle::box_radius(g.size));
        buf
    });
    let need_out = style.stroke.enabled && style.stroke.position != StrokePosition::Inside;
    let need_in =
        (style.stroke.enabled && style.stroke.position != StrokePosition::Outside) || style.inner_glow.enabled;
    let d_out = need_out.then(|| distance_to(&alpha.iter().map(|&a| a >= 0.5).collect::<Vec<_>>(), ww, wh));
    let d_in = need_in.then(|| distance_to(&alpha.iter().map(|&a| a < 0.5).collect::<Vec<_>>(), ww, wh));
    // Pixels beyond the canvas count as transparent for the inner distance.
    let d_in = d_in.map(|mut d| {
        for y in 0..wh {
            for x in 0..ww {
                let (gx, gy) = (x as i32 + win.x, y as i32 + win.y);
                let edge = (gx + 1).min(cw - gx).min(gy + 1).min(ch - gy) as f32;
                let i = y * ww + x;
                d[i] = d[i].min(edge);
            }
        }
        d
    });

    let st = &style.stroke;
    let (stroke_out_w, stroke_in_w) = match st.position {
        StrokePosition::Outside => (st.size, 0.0),
        StrokePosition::Inside => (0.0, st.size),
        StrokePosition::Center => (st.size * 0.5, st.size * 0.5),
    };

    // Compose tile by tile into the destination.
    let (tx0, ty0) = (out.x / TILE as i32, out.y / TILE as i32);
    let (tx1, ty1) = ((out.right() - 1) / TILE as i32, (out.bottom() - 1) / TILE as i32);
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let trect = IRect::new(tx * TILE as i32, ty * TILE as i32, TILE as i32, TILE as i32).intersect(&out);
            let mut any = false;
            let mut pixels: Vec<(usize, usize, Rgba8)> = Vec::with_capacity((trect.w * trect.h) as usize);
            for y in trect.y..trect.bottom() {
                for x in trect.x..trect.right() {
                    let i = idx(x, y);
                    let a = alpha[i];
                    let mut px = [0f32; 4];
                    if let Some(s) = &shadow {
                        px = over(px, rgb(style.drop_shadow.color), s[i] * style.drop_shadow.opacity);
                    }
                    if let Some(g) = &glow {
                        px = over(px, rgb(style.outer_glow.color), g[i] * style.outer_glow.opacity);
                    }
                    if let Some(d) = &d_out {
                        let cov = (stroke_out_w - d[i] + 0.5).clamp(0.0, 1.0);
                        px = over(px, rgb(st.color), cov * st.opacity);
                    }
                    if a > 0.0 {
                        let c = src.get_pixel(x, y);
                        let mut col = rgb(c);
                        if style.color_overlay.enabled {
                            let o = rgb(style.color_overlay.color);
                            let k = style.color_overlay.opacity.clamp(0.0, 1.0);
                            col = [
                                col[0] + (o[0] - col[0]) * k,
                                col[1] + (o[1] - col[1]) * k,
                                col[2] + (o[2] - col[2]) * k,
                            ];
                        }
                        if style.inner_glow.enabled {
                            if let Some(d) = &d_in {
                                let g = &style.inner_glow;
                                let t = (1.0 - (d[i] - 0.5).max(0.0) / g.size.max(1.0)).clamp(0.0, 1.0);
                                let k = t * t * g.opacity;
                                let o = rgb(g.color);
                                col = [
                                    col[0] + (o[0] - col[0]) * k,
                                    col[1] + (o[1] - col[1]) * k,
                                    col[2] + (o[2] - col[2]) * k,
                                ];
                            }
                        }
                        px = over(px, col, a);
                    }
                    if stroke_in_w > 0.0 {
                        if let Some(d) = &d_in {
                            let cov = (stroke_in_w - d[i] + 0.5).clamp(0.0, 1.0) * a;
                            px = over(px, rgb(st.color), cov * st.opacity);
                        }
                    }
                    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    let c = Rgba8::new(q(px[0]), q(px[1]), q(px[2]), q(px[3]));
                    any |= c.a > 0;
                    pixels.push(((x - tx * TILE as i32) as usize, (y - ty * TILE as i32) as usize, c));
                }
            }
            if !any && dst.tile(tx as u32, ty as u32).is_none() {
                continue;
            }
            let tile = dst.tile_mut(tx as u32, ty as u32);
            for (lx, ly, c) in pixels {
                tile.set(lx, ly, c);
            }
        }
    }
}

/// The whole layer, styled.
pub fn render(layer: &Layer) -> Raster {
    let (w, h) = (layer.raster.width(), layer.raster.height());
    let mut dst = Raster::new(w, h);
    render_region(
        &layer.raster,
        layer.active_mask(),
        &layer.props.style,
        IRect::new(0, 0, w as i32, h as i32),
        &mut dst,
    );
    dst
}

// ---------------------------------------------------------------------------
// Cache

struct Entry {
    style: LayerStyle,
    /// The layer's pixels as of the last render (tiles shared, so a change
    /// shows up as a different tile allocation).
    source: Raster,
    mask: Option<(Arc<Mask>, bool)>,
    out: Raster,
}

/// Styled copies of the styled layers of one document, kept up to date
/// incrementally (see [`StyleCache::update`]).
#[derive(Default)]
pub struct StyleCache {
    entries: HashMap<LayerId, Entry>,
}

impl StyleCache {
    /// Styled pixels to composite in place of the layer's own, if it has a style.
    pub fn get(&self, id: LayerId) -> Option<&Raster> {
        self.entries.get(&id).map(|e| &e.out)
    }

    /// Styled copies of every styled, visible layer, rendered from scratch
    /// (for a one-off flatten).
    pub fn build(doc: &DocState) -> Self {
        let mut c = Self::default();
        let mut dirty = TileSet::for_size(doc.width, doc.height);
        c.update(doc, &mut dirty);
        c
    }

    /// Bring the cache up to date with `doc`, adding to `dirty` every tile
    /// whose styled look changed (effects spread beyond the tiles whose
    /// pixels did).
    pub fn update(&mut self, doc: &DocState, dirty: &mut TileSet) {
        let (w, h) = (doc.width, doc.height);
        let mut seen: Vec<LayerId> = Vec::new();
        for layer in &doc.layers {
            let style = &layer.props.style;
            if !layer.owns_pixels() || !layer.props.visible || style.is_off() {
                continue;
            }
            let id = layer.props.id;
            seen.push(id);
            let mask = layer.mask.as_ref().map(|m| (m.clone(), layer.props.mask_enabled));
            let same_mask = |e: &Entry| match (&e.mask, &mask) {
                (None, None) => true,
                (Some((a, ea)), Some((b, eb))) => Arc::ptr_eq(a, b) && ea == eb,
                _ => false,
            };
            let full = match self.entries.get(&id) {
                None => true,
                Some(e) => e.style != *style || e.source.width() != w || e.source.height() != h || !same_mask(e),
            };
            if full {
                let out = render(layer);
                self.entries.insert(id, Entry { style: style.clone(), source: layer.raster.clone(), mask, out });
                dirty.insert_all();
                continue;
            }
            let e = self.entries.get_mut(&id).expect("checked above");
            let tiles_x = layer.raster.tiles_x();
            let changed: Vec<usize> =
                (0..layer.raster.tile_count()).filter(|&i| !layer.raster.tile_ptr_eq(&e.source, i)).collect();
            if changed.is_empty() {
                continue;
            }
            // Output tiles: every tile within `reach` of a changed one.
            let reach_tiles = (style.reach() as u32).div_ceil(TILE as u32) as i64;
            let tiles_y = layer.raster.tile_count() as u32 / tiles_x.max(1);
            let mut out_tiles = TileSet::for_size(w, h);
            for &i in &changed {
                let (cx, cy) = ((i as u32 % tiles_x) as i64, (i as u32 / tiles_x) as i64);
                for ty in (cy - reach_tiles).max(0)..=(cy + reach_tiles).min(tiles_y as i64 - 1) {
                    for tx in (cx - reach_tiles).max(0)..=(cx + reach_tiles).min(tiles_x as i64 - 1) {
                        out_tiles.insert(tx as u32, ty as u32);
                    }
                }
            }
            if out_tiles.len() * 2 > layer.raster.tile_count() {
                e.out = render(layer);
                dirty.insert_all();
            } else {
                for (tx, ty) in out_tiles.iter() {
                    let r =
                        IRect::new((tx as usize * TILE) as i32, (ty as usize * TILE) as i32, TILE as i32, TILE as i32);
                    render_region(&layer.raster, layer.active_mask(), style, r, &mut e.out);
                }
                dirty.union_with(&out_tiles);
            }
            e.source = layer.raster.clone();
        }
        let before = self.entries.len();
        self.entries.retain(|id, _| seen.contains(id));
        if self.entries.len() != before {
            dirty.insert_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_layer() -> Layer {
        let mut l = Layer::new(1, "sq", 128, 128);
        for y in 40..80 {
            for x in 40..80 {
                l.raster.set_pixel(x, y, Rgba8::new(0, 0, 255, 255));
            }
        }
        l
    }

    #[test]
    fn distance_transform_matches_manhattan_free_cases() {
        let w = 9;
        let mut f = vec![false; w * w];
        f[4 * w + 4] = true;
        let d = distance_to(&f, w, w);
        assert_eq!(d[4 * w + 4], 0.0);
        assert!((d[4 * w + 7] - 3.0).abs() < 1e-4);
        assert!((d[0] - (32f32).sqrt()).abs() < 1e-3);
    }

    #[test]
    fn outside_stroke_rings_the_shape() {
        let mut l = square_layer();
        l.props.style.stroke =
            StrokeFx { enabled: true, size: 3.0, color: Rgba8::new(255, 0, 0, 255), ..Default::default() };
        let out = render(&l);
        // Inside stays blue, just outside turns red, far outside is empty.
        assert_eq!(out.get_pixel(60, 60), Rgba8::new(0, 0, 255, 255));
        let ring = out.get_pixel(38, 60);
        assert_eq!((ring.r, ring.a), (255, 255));
        assert_eq!(out.get_pixel(30, 60).a, 0);
    }

    #[test]
    fn drop_shadow_falls_away_from_the_light() {
        let mut l = square_layer();
        l.props.style.drop_shadow =
            DropShadow { enabled: true, angle: 90.0, distance: 10.0, size: 0.0, ..Default::default() };
        let out = render(&l);
        // Light from above (90°): the shadow shows below the square.
        assert!(out.get_pixel(60, 85).a > 100);
        assert_eq!(out.get_pixel(60, 35).a, 0);
    }

    #[test]
    fn incremental_update_matches_a_full_render() {
        let mut doc = DocState::new(256, 256, None);
        doc.layers[0].props.style.outer_glow = Glow { enabled: true, size: 12.0, ..Default::default() };
        doc.layers[0].raster.set_pixel(100, 100, Rgba8::new(255, 255, 255, 255));
        let mut cache = StyleCache::default();
        let mut dirty = TileSet::for_size(256, 256);
        cache.update(&doc, &mut dirty);
        // Paint far away: only that area is re-rendered, and it matches.
        doc.layers[0].raster.set_pixel(200, 30, Rgba8::new(255, 255, 255, 255));
        let mut dirty = TileSet::for_size(256, 256);
        cache.update(&doc, &mut dirty);
        assert!(!dirty.is_empty() && dirty.len() < 16);
        let fresh = render(&doc.layers[0]);
        let id = doc.layers[0].props.id;
        for (x, y) in [(200, 30), (205, 35), (100, 100), (150, 150)] {
            assert_eq!(cache.get(id).unwrap().get_pixel(x, y), fresh.get_pixel(x, y), "at {x},{y}");
        }
    }
}
