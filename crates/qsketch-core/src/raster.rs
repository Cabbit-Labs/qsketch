//! Tiled, copy-on-write RGBA8 raster.
//!
//! A [`Raster`] is a fixed-size grid of 64×64 tiles. Empty (fully transparent)
//! tiles are simply `None`, so sparse layers cost almost nothing. Tiles live
//! behind `Arc`, so cloning a raster is O(tiles) pointer copies and mutation
//! goes through `Arc::make_mut` (copy-on-write). This is what makes document
//! snapshots for undo essentially free.

use std::sync::Arc;

use crate::color::Rgba8;
use crate::geom::IRect;

/// Tile edge length in pixels.
pub const TILE: usize = 64;
/// Pixels per tile.
pub const TILE_PX: usize = TILE * TILE;
/// Bytes per RGBA8 tile.
pub const TILE_BYTES: usize = TILE_PX * 4;

/// One 64×64 block of straight-alpha RGBA8 pixels, row-major.
#[derive(Clone)]
pub struct Tile {
    pub px: [u8; TILE_BYTES],
}

impl Tile {
    pub fn zeroed() -> Arc<Tile> {
        // Allocate directly on the heap: a zeroed 16 KiB block.
        // SAFETY: `Tile` is a plain byte array; all-zero is a valid value.
        let layout = std::alloc::Layout::new::<Tile>();
        unsafe {
            let p = std::alloc::alloc_zeroed(layout) as *mut Tile;
            if p.is_null() {
                std::alloc::handle_alloc_error(layout);
            }
            Arc::from(Box::from_raw(p))
        }
    }

    pub fn filled(c: Rgba8) -> Arc<Tile> {
        let mut t = Tile::zeroed();
        let m = Arc::make_mut(&mut t);
        let v = c.to_array();
        for p in m.px.chunks_exact_mut(4) {
            p.copy_from_slice(&v);
        }
        t
    }

    #[inline]
    pub fn get(&self, lx: usize, ly: usize) -> Rgba8 {
        let i = (ly * TILE + lx) * 4;
        Rgba8::new(self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3])
    }

    #[inline]
    pub fn set(&mut self, lx: usize, ly: usize, c: Rgba8) {
        let i = (ly * TILE + lx) * 4;
        self.px[i..i + 4].copy_from_slice(&c.to_array());
    }

    /// True if every pixel is fully transparent.
    pub fn is_empty(&self) -> bool {
        self.px.chunks_exact(4).all(|p| p[3] == 0)
    }
}

/// Pixel rect covered by tile `(tx, ty)`.
#[inline]
pub fn tile_rect(tx: u32, ty: u32) -> IRect {
    IRect::new((tx as usize * TILE) as i32, (ty as usize * TILE) as i32, TILE as i32, TILE as i32)
}

#[derive(Clone)]
pub struct Raster {
    width: u32,
    height: u32,
    tiles_x: u32,
    tiles_y: u32,
    tiles: Vec<Option<Arc<Tile>>>,
}

impl Raster {
    pub fn new(width: u32, height: u32) -> Self {
        let tiles_x = width.div_ceil(TILE as u32);
        let tiles_y = height.div_ceil(TILE as u32);
        Self { width, height, tiles_x, tiles_y, tiles: vec![None; (tiles_x * tiles_y) as usize] }
    }

    pub fn new_filled(width: u32, height: u32, c: Rgba8) -> Self {
        let mut r = Self::new(width, height);
        if c.a != 0 {
            let t = Tile::filled(c);
            for slot in &mut r.tiles {
                *slot = Some(t.clone());
            }
        }
        r
    }

    /// Build from a straight-alpha RGBA8 buffer of `width*height*4` bytes.
    pub fn from_rgba(width: u32, height: u32, data: &[u8]) -> Self {
        assert_eq!(data.len(), (width * height * 4) as usize, "buffer size mismatch");
        let mut r = Self::new(width, height);
        let stride = width as usize * 4;
        for ty in 0..r.tiles_y {
            for tx in 0..r.tiles_x {
                let mut tile = Tile::zeroed();
                let t = Arc::make_mut(&mut tile);
                let x0 = tx as usize * TILE;
                let y0 = ty as usize * TILE;
                let w = TILE.min(width as usize - x0);
                let h = TILE.min(height as usize - y0);
                let mut any = false;
                for ly in 0..h {
                    let src = &data[(y0 + ly) * stride + x0 * 4..][..w * 4];
                    let dst = &mut t.px[ly * TILE * 4..][..w * 4];
                    dst.copy_from_slice(src);
                    if !any {
                        any = src.chunks_exact(4).any(|p| p[3] != 0);
                    }
                }
                if any {
                    r.tiles[(ty * r.tiles_x + tx) as usize] = Some(tile);
                }
            }
        }
        r
    }

    /// Straight-alpha RGBA8, `width*height*4` bytes.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = vec![0u8; (self.width * self.height * 4) as usize];
        let stride = self.width as usize * 4;
        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                let Some(t) = &self.tiles[(ty * self.tiles_x + tx) as usize] else { continue };
                let x0 = tx as usize * TILE;
                let y0 = ty as usize * TILE;
                let w = TILE.min(self.width as usize - x0);
                let h = TILE.min(self.height as usize - y0);
                for ly in 0..h {
                    let dst = &mut out[(y0 + ly) * stride + x0 * 4..][..w * 4];
                    dst.copy_from_slice(&t.px[ly * TILE * 4..][..w * 4]);
                }
            }
        }
        out
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn tiles_x(&self) -> u32 {
        self.tiles_x
    }
    pub fn tiles_y(&self) -> u32 {
        self.tiles_y
    }
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }
    pub fn rect(&self) -> IRect {
        IRect::new(0, 0, self.width as i32, self.height as i32)
    }

    #[inline]
    pub fn tile_index(&self, tx: u32, ty: u32) -> usize {
        (ty * self.tiles_x + tx) as usize
    }

    #[inline]
    pub fn tile(&self, tx: u32, ty: u32) -> Option<&Arc<Tile>> {
        self.tiles.get(self.tile_index(tx, ty)).and_then(|t| t.as_ref())
    }

    #[inline]
    pub fn tile_at_index(&self, idx: usize) -> Option<&Arc<Tile>> {
        self.tiles.get(idx).and_then(|t| t.as_ref())
    }

    /// Mutable access to a tile, allocating a transparent one if absent and
    /// copying it if shared (copy-on-write).
    #[inline]
    pub fn tile_mut(&mut self, tx: u32, ty: u32) -> &mut Tile {
        let i = self.tile_index(tx, ty);
        let slot = &mut self.tiles[i];
        if slot.is_none() {
            *slot = Some(Tile::zeroed());
        }
        Arc::make_mut(slot.as_mut().unwrap())
    }

    /// Replace a tile slot wholesale (`None` = transparent).
    pub fn set_tile(&mut self, tx: u32, ty: u32, tile: Option<Arc<Tile>>) {
        let i = self.tile_index(tx, ty);
        self.tiles[i] = tile;
    }

    /// True if the two rasters share the exact same tile allocation at `idx`.
    pub fn tile_ptr_eq(&self, other: &Raster, idx: usize) -> bool {
        match (&self.tiles[idx], &other.tiles[idx]) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    #[inline]
    pub fn get_pixel(&self, x: i32, y: i32) -> Rgba8 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return Rgba8::TRANSPARENT;
        }
        let (tx, ty) = (x as u32 / TILE as u32, y as u32 / TILE as u32);
        match self.tile(tx, ty) {
            Some(t) => t.get(x as usize % TILE, y as usize % TILE),
            None => Rgba8::TRANSPARENT,
        }
    }

    #[inline]
    pub fn set_pixel(&mut self, x: i32, y: i32, c: Rgba8) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let (tx, ty) = (x as u32 / TILE as u32, y as u32 / TILE as u32);
        self.tile_mut(tx, ty).set(x as usize % TILE, y as usize % TILE, c);
    }

    /// Tile coordinates intersecting `rect` (clamped to the raster).
    pub fn tiles_in_rect(&self, rect: IRect) -> Vec<(u32, u32)> {
        let r = rect.intersect(&self.rect());
        if r.is_empty() {
            return Vec::new();
        }
        let tx0 = r.x as u32 / TILE as u32;
        let ty0 = r.y as u32 / TILE as u32;
        let tx1 = (r.right() - 1) as u32 / TILE as u32;
        let ty1 = (r.bottom() - 1) as u32 / TILE as u32;
        let mut v = Vec::with_capacity(((tx1 - tx0 + 1) * (ty1 - ty0 + 1)) as usize);
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                v.push((tx, ty));
            }
        }
        v
    }

    pub fn fill_rect(&mut self, rect: IRect, c: Rgba8) {
        let r = rect.intersect(&self.rect());
        if r.is_empty() {
            return;
        }
        for (tx, ty) in self.tiles_in_rect(r) {
            let tr = tile_rect(tx, ty);
            let sub = r.intersect(&tr);
            if sub == tr && c.a == 0 {
                self.set_tile(tx, ty, None);
                continue;
            }
            if sub == tr {
                self.set_tile(tx, ty, Some(Tile::filled(c)));
                continue;
            }
            let t = self.tile_mut(tx, ty);
            for y in sub.y..sub.bottom() {
                for x in sub.x..sub.right() {
                    t.set((x - tr.x) as usize, (y - tr.y) as usize, c);
                }
            }
        }
    }

    pub fn clear(&mut self) {
        for t in &mut self.tiles {
            *t = None;
        }
    }

    /// Drop tiles that became fully transparent (e.g. after erasing).
    pub fn prune_empty_tiles(&mut self) {
        for slot in &mut self.tiles {
            if let Some(t) = slot {
                if t.is_empty() {
                    *slot = None;
                }
            }
        }
    }

    /// True if no tile holds any opaque pixel.
    pub fn is_empty(&self) -> bool {
        self.tiles.iter().all(|t| t.as_ref().is_none_or(|t| t.is_empty()))
    }

    /// Bounding box of non-transparent pixels, if any.
    pub fn bounds(&self) -> Option<IRect> {
        let mut acc = IRect::EMPTY;
        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                let Some(t) = self.tile(tx, ty) else { continue };
                let tr = tile_rect(tx, ty);
                let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
                for ly in 0..TILE {
                    for lx in 0..TILE {
                        if t.px[(ly * TILE + lx) * 4 + 3] != 0 {
                            x0 = x0.min(lx as i32);
                            x1 = x1.max(lx as i32);
                            y0 = y0.min(ly as i32);
                            y1 = y1.max(ly as i32);
                        }
                    }
                }
                if x0 <= x1 {
                    let r = IRect::from_corners(tr.x + x0, tr.y + y0, tr.x + x1, tr.y + y1);
                    acc = acc.union(&r);
                }
            }
        }
        let clipped = acc.intersect(&self.rect());
        if clipped.is_empty() {
            None
        } else {
            Some(clipped)
        }
    }

    /// Copy of the pixels in `rect` (clamped), as a new raster of the rect's size
    /// positioned at its own origin. Pixels outside the source are transparent.
    pub fn crop(&self, rect: IRect) -> Raster {
        let mut out = Raster::new(rect.w.max(0) as u32, rect.h.max(0) as u32);
        let src = rect.intersect(&self.rect());
        for y in src.y..src.bottom() {
            for x in src.x..src.right() {
                let c = self.get_pixel(x, y);
                if c.a != 0 {
                    out.set_pixel(x - rect.x, y - rect.y, c);
                }
            }
        }
        out
    }

    /// Blit `src` so that its origin lands at `(dx, dy)`, replacing pixels
    /// (no blending). Pixels with alpha 0 are skipped unless `replace_transparent`.
    pub fn blit(&mut self, src: &Raster, dx: i32, dy: i32, replace_transparent: bool) {
        let dst_rect = IRect::new(dx, dy, src.width as i32, src.height as i32).intersect(&self.rect());
        for y in dst_rect.y..dst_rect.bottom() {
            for x in dst_rect.x..dst_rect.right() {
                let c = src.get_pixel(x - dx, y - dy);
                if c.a != 0 || replace_transparent {
                    self.set_pixel(x, y, c);
                }
            }
        }
    }

    /// Same-size raster with content shifted by `(dx, dy)`.
    pub fn translated(&self, dx: i32, dy: i32) -> Raster {
        let mut out = Raster::new(self.width, self.height);
        // Fast path: whole-tile shifts.
        if dx % TILE as i32 == 0 && dy % TILE as i32 == 0 {
            let (sx, sy) = (dx / TILE as i32, dy / TILE as i32);
            for ty in 0..self.tiles_y as i32 {
                for tx in 0..self.tiles_x as i32 {
                    let (nx, ny) = (tx + sx, ty + sy);
                    if nx >= 0 && ny >= 0 && nx < self.tiles_x as i32 && ny < self.tiles_y as i32 {
                        out.tiles[(ny as u32 * self.tiles_x + nx as u32) as usize] =
                            self.tiles[(ty as u32 * self.tiles_x + tx as u32) as usize].clone();
                    }
                }
            }
            return out;
        }
        out.blit(self, dx, dy, false);
        out
    }

    pub fn flipped_h(&self) -> Raster {
        let mut out = Raster::new(self.width, self.height);
        let w = self.width as i32;
        for y in 0..self.height as i32 {
            for x in 0..w {
                let c = self.get_pixel(x, y);
                if c.a != 0 {
                    out.set_pixel(w - 1 - x, y, c);
                }
            }
        }
        out
    }

    pub fn flipped_v(&self) -> Raster {
        let mut out = Raster::new(self.width, self.height);
        let h = self.height as i32;
        for y in 0..h {
            for x in 0..self.width as i32 {
                let c = self.get_pixel(x, y);
                if c.a != 0 {
                    out.set_pixel(x, h - 1 - y, c);
                }
            }
        }
        out
    }

    /// Rotate 90° clockwise (`times` = 1), 180° (2) or 270° (3).
    pub fn rotated(&self, times: u32) -> Raster {
        let times = times % 4;
        if times == 0 {
            return self.clone();
        }
        let (w, h) = (self.width as i32, self.height as i32);
        let (nw, nh) = if times % 2 == 1 { (h, w) } else { (w, h) };
        let mut out = Raster::new(nw as u32, nh as u32);
        for y in 0..h {
            for x in 0..w {
                let c = self.get_pixel(x, y);
                if c.a == 0 {
                    continue;
                }
                let (nx, ny) = match times {
                    1 => (h - 1 - y, x),
                    2 => (w - 1 - x, h - 1 - y),
                    _ => (y, w - 1 - x),
                };
                out.set_pixel(nx, ny, c);
            }
        }
        out
    }

    /// Resample to a new size.
    pub fn resized(&self, new_w: u32, new_h: u32, filter: ResizeFilter) -> Raster {
        if new_w == 0 || new_h == 0 {
            return Raster::new(new_w.max(1), new_h.max(1));
        }
        let src = self.to_rgba();
        let (sw, sh) = (self.width as usize, self.height as usize);
        let mut dst = vec![0u8; (new_w * new_h * 4) as usize];
        let sx = sw as f32 / new_w as f32;
        let sy = sh as f32 / new_h as f32;
        for y in 0..new_h as usize {
            for x in 0..new_w as usize {
                let o = (y * new_w as usize + x) * 4;
                match filter {
                    ResizeFilter::Nearest => {
                        let fx = ((x as f32 + 0.5) * sx) as usize;
                        let fy = ((y as f32 + 0.5) * sy) as usize;
                        let i = (fy.min(sh - 1) * sw + fx.min(sw - 1)) * 4;
                        dst[o..o + 4].copy_from_slice(&src[i..i + 4]);
                    }
                    ResizeFilter::Bilinear => {
                        let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
                        let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
                        let x0 = (fx as usize).min(sw - 1);
                        let y0 = (fy as usize).min(sh - 1);
                        let x1 = (x0 + 1).min(sw - 1);
                        let y1 = (y0 + 1).min(sh - 1);
                        let tx = fx - x0 as f32;
                        let ty = fy - y0 as f32;
                        let px = |xx: usize, yy: usize| -> [f32; 4] {
                            let i = (yy * sw + xx) * 4;
                            let a = src[i + 3] as f32;
                            // premultiply for correct interpolation
                            [src[i] as f32 * a, src[i + 1] as f32 * a, src[i + 2] as f32 * a, a]
                        };
                        let (p00, p10, p01, p11) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
                        let mut r = [0f32; 4];
                        for c in 0..4 {
                            let top = p00[c] + (p10[c] - p00[c]) * tx;
                            let bot = p01[c] + (p11[c] - p01[c]) * tx;
                            r[c] = top + (bot - top) * ty;
                        }
                        if r[3] > 0.0 {
                            dst[o] = (r[0] / r[3] + 0.5) as u8;
                            dst[o + 1] = (r[1] / r[3] + 0.5) as u8;
                            dst[o + 2] = (r[2] / r[3] + 0.5) as u8;
                            dst[o + 3] = (r[3] + 0.5) as u8;
                        }
                    }
                }
            }
        }
        Raster::from_rgba(new_w, new_h, &dst)
    }

    /// Change canvas size, placing the old content at `(ox, oy)` in the new raster.
    pub fn with_canvas_size(&self, new_w: u32, new_h: u32, ox: i32, oy: i32) -> Raster {
        let mut out = Raster::new(new_w, new_h);
        out.blit(self, ox, oy, false);
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResizeFilter {
    Nearest,
    Bilinear,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_and_tiles() {
        let mut r = Raster::new(100, 70);
        assert_eq!(r.tiles_x(), 2);
        assert_eq!(r.tiles_y(), 2);
        assert!(r.is_empty());
        r.set_pixel(99, 69, Rgba8::rgb(1, 2, 3));
        assert_eq!(r.get_pixel(99, 69), Rgba8::rgb(1, 2, 3));
        assert_eq!(r.get_pixel(100, 69), Rgba8::TRANSPARENT);
        assert_eq!(r.bounds(), Some(IRect::new(99, 69, 1, 1)));
        assert_eq!(r.tiles_in_rect(IRect::new(60, 0, 10, 10)), vec![(0, 0), (1, 0)]);
    }

    #[test]
    fn cow_snapshot() {
        let mut a = Raster::new(64, 64);
        a.set_pixel(1, 1, Rgba8::WHITE);
        let snap = a.clone();
        assert!(a.tile_ptr_eq(&snap, 0));
        a.set_pixel(2, 2, Rgba8::BLACK);
        assert!(!a.tile_ptr_eq(&snap, 0));
        assert_eq!(snap.get_pixel(2, 2), Rgba8::TRANSPARENT);
        assert_eq!(a.get_pixel(2, 2), Rgba8::BLACK);
    }

    #[test]
    fn rgba_roundtrip() {
        let mut data = vec![0u8; 130 * 70 * 4];
        for (i, p) in data.chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[(i % 251) as u8, (i % 13) as u8, 7, ((i * 7) % 256) as u8]);
        }
        let r = Raster::from_rgba(130, 70, &data);
        let back = r.to_rgba();
        // Pixels with alpha 0 may be dropped entirely, everything else must survive.
        for (a, b) in data.chunks_exact(4).zip(back.chunks_exact(4)) {
            if a[3] != 0 {
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn transforms() {
        let mut r = Raster::new(4, 3);
        r.set_pixel(0, 0, Rgba8::WHITE);
        assert_eq!(r.flipped_h().get_pixel(3, 0), Rgba8::WHITE);
        assert_eq!(r.flipped_v().get_pixel(0, 2), Rgba8::WHITE);
        let rot = r.rotated(1);
        assert_eq!((rot.width(), rot.height()), (3, 4));
        assert_eq!(rot.get_pixel(2, 0), Rgba8::WHITE);
        assert_eq!(r.translated(1, 1).get_pixel(1, 1), Rgba8::WHITE);
        assert_eq!(r.translated(64, 0).get_pixel(0, 0), Rgba8::TRANSPARENT);
        let big = r.resized(8, 6, ResizeFilter::Nearest);
        assert_eq!(big.get_pixel(1, 1), Rgba8::WHITE);
        assert_eq!(big.get_pixel(2, 2), Rgba8::TRANSPARENT);
    }
}
