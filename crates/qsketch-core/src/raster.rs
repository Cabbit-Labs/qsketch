//! Tiled, copy-on-write RGBA8 raster.
//!
//! A [`Raster`] is a fixed-size grid of 64×64 tiles. Empty (fully transparent)
//! tiles are simply `None`, so sparse layers cost almost nothing. Tiles live
//! behind `Arc`, so cloning a raster is O(tiles) pointer copies and mutation
//! goes through `Arc::make_mut` (copy-on-write). This is what makes document
//! snapshots for undo essentially free.
//!
//! Pixels can also live *outside* the canvas: a layer moved partly off the
//! edge keeps what went over in `outside` tiles (signed tile coordinates on
//! the same 64 px lattice) instead of losing it. Painting and compositing
//! only ever see the canvas grid; moves, canvas resizes and flips carry the
//! outside pixels along (see the `*_any` / `*_keep` methods).

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

/// Off-canvas tiles keyed by signed tile coordinates. They only ever hold
/// pixels that lie outside the canvas rect.
type Outside = std::collections::BTreeMap<(i32, i32), Arc<Tile>>;

#[derive(Clone)]
pub struct Raster {
    width: u32,
    height: u32,
    tiles_x: u32,
    tiles_y: u32,
    tiles: Vec<Option<Arc<Tile>>>,
    outside: Option<Arc<Outside>>,
}

impl Raster {
    pub fn new(width: u32, height: u32) -> Self {
        let tiles_x = width.div_ceil(TILE as u32);
        let tiles_y = height.div_ceil(TILE as u32);
        Self { width, height, tiles_x, tiles_y, tiles: vec![None; (tiles_x * tiles_y) as usize], outside: None }
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

    /// Run `f` on each listed tile, in parallel (allocating absent tiles and
    /// un-sharing shared ones first). The tiles are distinct, so the calls
    /// never touch the same memory.
    pub fn par_tiles_mut(&mut self, which: &[(u32, u32)], f: impl Fn(u32, u32, &mut Tile) + Sync + Send) {
        use rayon::prelude::*;
        let tx = self.tiles_x;
        let mut wanted = vec![false; self.tiles.len()];
        for &(x, y) in which {
            if let Some(w) = wanted.get_mut((y * tx + x) as usize) {
                *w = true;
            }
        }
        self.tiles.par_iter_mut().enumerate().filter(|(i, _)| wanted[*i]).for_each(|(i, slot)| {
            let t = Arc::make_mut(slot.get_or_insert_with(Tile::zeroed));
            f(i as u32 % tx, i as u32 / tx, t);
        });
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

    /// Empty the layer, including any pixels kept outside the canvas.
    pub fn clear(&mut self) {
        self.tiles.fill(None);
        self.outside = None;
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
        if let Some(o) = &mut self.outside {
            if o.values().any(|t| t.is_empty()) {
                Arc::make_mut(o).retain(|_, t| !t.is_empty());
            }
            if o.is_empty() {
                self.outside = None;
            }
        }
    }

    /// True if no tile holds any opaque pixel.
    pub fn is_empty(&self) -> bool {
        self.tiles.iter().all(|t| t.as_ref().is_none_or(|t| t.is_empty()))
    }

    /// Bounding box of non-transparent pixels on the canvas, if any.
    pub fn bounds(&self) -> Option<IRect> {
        let mut acc = IRect::EMPTY;
        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                let Some(t) = self.tile(tx, ty) else { continue };
                if let Some(r) = tile_content(t, tx as i32, ty as i32) {
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

    /// Copy of the pixels in `rect`, as a new raster of the rect's size
    /// positioned at its own origin. Off-canvas pixels count; places with no
    /// pixels are transparent.
    pub fn crop(&self, rect: IRect) -> Raster {
        let mut out = Raster::new(rect.w.max(0) as u32, rect.h.max(0) as u32);
        let src = if self.outside.is_some() { rect } else { rect.intersect(&self.rect()) };
        for y in src.y..src.bottom() {
            for x in src.x..src.right() {
                let c = self.get_pixel_any(x, y);
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
        self.for_each_outside(|x, y, c| out.set_pixel_any(w - 1 - x, y, c));
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
        self.for_each_outside(|x, y, c| out.set_pixel_any(x, h - 1 - y, c));
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
                let (nx, ny) = rotate_xy(x, y, w, h, times);
                out.set_pixel(nx, ny, c);
            }
        }
        self.for_each_outside(|x, y, c| {
            let (nx, ny) = rotate_xy(x, y, w, h, times);
            out.set_pixel_any(nx, ny, c);
        });
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
                    ResizeFilter::Nearest | ResizeFilter::RotSprite => {
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

    /// Change canvas size, placing the old content at `(ox, oy)` in the new
    /// raster. Nothing is lost: what falls off the new canvas (and what was
    /// already off the old one) is kept outside it.
    pub fn with_canvas_size(&self, new_w: u32, new_h: u32, ox: i32, oy: i32) -> Raster {
        let mut out = Raster::new(new_w, new_h);
        self.place_into(&mut out, ox, oy);
        out
    }

    /// The same content shifted by `(dx, dy)`, keeping whatever crosses the
    /// canvas edge outside it (and bringing outside pixels back in).
    pub fn shifted_keep(&self, dx: i32, dy: i32) -> Raster {
        let mut out = Raster::new(self.width, self.height);
        self.place_into(&mut out, dx, dy);
        out
    }

    /// Copy every pixel (on and off the canvas) into the empty raster `out`,
    /// offset by `(dx, dy)`, a row segment at a time.
    fn place_into(&self, out: &mut Raster, dx: i32, dy: i32) {
        let (w, h) = (self.width as i32, self.height as i32);
        let mut copy = |t: &Tile, tx: i32, ty: i32, on_canvas: bool| {
            let (x0, y0) = (tx * TILE as i32, ty * TILE as i32);
            // Grid tiles may carry stray pixels in their padding past the
            // canvas edge; only the canvas part of them counts.
            let (lx1, ly1) = if on_canvas {
                ((w - x0).clamp(0, TILE as i32) as usize, (h - y0).clamp(0, TILE as i32) as usize)
            } else {
                (TILE, TILE)
            };
            // Only the non-transparent runs: an outside tile can share its
            // key with an edge grid tile, and its empty cells there must not
            // overwrite the grid's pixels.
            for ly in 0..ly1 {
                let row = &t.px[ly * TILE * 4..][..lx1 * 4];
                let mut lx = 0;
                while lx < lx1 {
                    if row[lx * 4 + 3] == 0 {
                        lx += 1;
                        continue;
                    }
                    let start = lx;
                    while lx < lx1 && row[lx * 4 + 3] != 0 {
                        lx += 1;
                    }
                    out.put_row(x0 + start as i32 + dx, y0 + ly as i32 + dy, &row[start * 4..lx * 4]);
                }
            }
        };
        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                if let Some(t) = self.tile(tx, ty) {
                    copy(t, tx as i32, ty as i32, true);
                }
            }
        }
        if let Some(o) = &self.outside {
            for (&(tx, ty), t) in o.iter() {
                copy(t, tx, ty, false);
            }
        }
        out.prune_empty_tiles();
    }

    /// Write straight RGBA pixels starting at `(x, y)` along one row,
    /// splitting at tile and canvas edges: canvas pixels land in the grid,
    /// the rest outside. Transparent runs don't allocate tiles.
    fn put_row(&mut self, x: i32, y: i32, px: &[u8]) {
        let (w, h) = (self.width as i32, self.height as i32);
        let in_row = y >= 0 && y < h;
        let n = (px.len() / 4) as i32;
        let (ty, ly) = (y.div_euclid(TILE as i32), y.rem_euclid(TILE as i32) as usize);
        let mut i = 0;
        while i < n {
            let cx = x + i;
            let mut end = (cx.div_euclid(TILE as i32) + 1) * TILE as i32;
            if in_row {
                if cx < 0 {
                    end = end.min(0);
                } else if cx < w {
                    end = end.min(w);
                }
            }
            let seg = (end - cx).min(n - i);
            let bytes = &px[i as usize * 4..(i + seg) as usize * 4];
            let lx = cx.rem_euclid(TILE as i32) as usize;
            let tx = cx.div_euclid(TILE as i32);
            let clear = bytes.chunks_exact(4).all(|p| p[3] == 0);
            let tile: Option<&mut Tile> = if in_row && cx >= 0 && cx < w {
                if clear && self.tile(tx as u32, ty as u32).is_none() {
                    None
                } else {
                    Some(self.tile_mut(tx as u32, ty as u32))
                }
            } else {
                let o = Arc::make_mut(self.outside.get_or_insert_with(Default::default));
                if clear && !o.contains_key(&(tx, ty)) {
                    None
                } else {
                    Some(Arc::make_mut(o.entry((tx, ty)).or_insert_with(Tile::zeroed)))
                }
            };
            if let Some(t) = tile {
                t.px[(ly * TILE + lx) * 4..][..bytes.len()].copy_from_slice(bytes);
            }
            i += seg;
        }
        if self.outside.as_ref().is_some_and(|o| o.is_empty()) {
            self.outside = None;
        }
    }

    /// The pixel at `(x, y)`, on or off the canvas.
    pub fn get_pixel_any(&self, x: i32, y: i32) -> Rgba8 {
        if x >= 0 && y >= 0 && x < self.width as i32 && y < self.height as i32 {
            return self.get_pixel(x, y);
        }
        let Some(o) = &self.outside else { return Rgba8::TRANSPARENT };
        let k = (x.div_euclid(TILE as i32), y.div_euclid(TILE as i32));
        match o.get(&k) {
            Some(t) => t.get(x.rem_euclid(TILE as i32) as usize, y.rem_euclid(TILE as i32) as usize),
            None => Rgba8::TRANSPARENT,
        }
    }

    /// Set the pixel at `(x, y)`, keeping it outside the canvas if it lies
    /// there (plain `set_pixel` clips it away).
    pub fn set_pixel_any(&mut self, x: i32, y: i32, c: Rgba8) {
        self.put_row(x, y, &c.to_array());
    }

    /// Whether any pixels are kept outside the canvas.
    pub fn has_outside(&self) -> bool {
        self.outside.as_ref().is_some_and(|o| o.values().any(|t| !t.is_empty()))
    }

    /// Forget the pixels kept outside the canvas.
    pub fn clear_outside(&mut self) {
        self.outside = None;
    }

    /// Bounding box of the pixels kept outside the canvas, if any.
    pub fn outside_bounds(&self) -> Option<IRect> {
        let o = self.outside.as_ref()?;
        let r = o.iter().filter_map(|(&(tx, ty), t)| tile_content(t, tx, ty)).fold(IRect::EMPTY, |a, r| a.union(&r));
        (!r.is_empty()).then_some(r)
    }

    /// Bounding box of all non-transparent pixels, on and off the canvas.
    pub fn full_bounds(&self) -> Option<IRect> {
        match (self.bounds(), self.outside_bounds()) {
            (Some(a), Some(b)) => Some(a.union(&b)),
            (a, b) => a.or(b),
        }
    }

    /// Visit every non-transparent pixel kept outside the canvas.
    pub fn for_each_outside(&self, mut f: impl FnMut(i32, i32, Rgba8)) {
        let Some(o) = &self.outside else { return };
        for (&(tx, ty), t) in o.iter() {
            for ly in 0..TILE {
                for lx in 0..TILE {
                    let c = t.get(lx, ly);
                    if c.a != 0 {
                        f(tx * TILE as i32 + lx as i32, ty * TILE as i32 + ly as i32, c);
                    }
                }
            }
        }
    }

    /// The outside pixels as one picture covering their bounding box
    /// (canvas pixels inside that box left transparent), for saving.
    pub fn outside_image(&self) -> Option<(IRect, Raster)> {
        let r = self.outside_bounds()?;
        let mut img = Raster::new(r.w as u32, r.h as u32);
        self.for_each_outside(|x, y, c| img.set_pixel(x - r.x, y - r.y, c));
        Some((r, img))
    }

    /// Put back pixels saved by [`Raster::outside_image`] with its top-left
    /// at `(x, y)`. Only the ones that fall outside the canvas are kept.
    pub fn restore_outside(&mut self, x: i32, y: i32, img: &Raster) {
        let (w, h) = (self.width as i32, self.height as i32);
        for iy in 0..img.height() as i32 {
            for ix in 0..img.width() as i32 {
                let c = img.get_pixel(ix, iy);
                let (px, py) = (x + ix, y + iy);
                if c.a != 0 && !(px >= 0 && py >= 0 && px < w && py < h) {
                    self.set_pixel_any(px, py, c);
                }
            }
        }
    }
}

/// Where a 90°-step clockwise rotation (`times` of them) of a `w`×`h`
/// canvas sends `(x, y)`.
fn rotate_xy(x: i32, y: i32, w: i32, h: i32, times: u32) -> (i32, i32) {
    match times % 4 {
        0 => (x, y),
        1 => (h - 1 - y, x),
        2 => (w - 1 - x, h - 1 - y),
        _ => (y, w - 1 - x),
    }
}

/// Document-space bounding box of tile `(tx, ty)`'s non-transparent pixels.
fn tile_content(t: &Tile, tx: i32, ty: i32) -> Option<IRect> {
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
    let (ox, oy) = (tx * TILE as i32, ty * TILE as i32);
    (x0 <= x1).then(|| IRect::from_corners(ox + x0, oy + y0, ox + x1, oy + y1))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResizeFilter {
    Nearest,
    Bilinear,
    /// Pixel-art rotation (RotSprite): the source is enlarged 8× with
    /// Scale2x, which rounds diagonal staircases into clean slopes, then
    /// sampled nearest-neighbor with hard edges. For plain resizing it is
    /// the same as `Nearest`.
    RotSprite,
}

/// One Scale2x (EPX) pass: each pixel becomes a 2×2 block whose corners
/// take a neighbor's color where two neighbors agree along that corner,
/// which smooths diagonal edges without inventing new colors. `px` holds
/// straight RGBA packed as `u32`; edges repeat the border pixels.
pub fn scale2x(px: &[u32], w: usize, h: usize) -> Vec<u32> {
    let mut out = vec![0u32; w * h * 4];
    let at = |x: isize, y: isize| px[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
    let ow = w * 2;
    for y in 0..h as isize {
        for x in 0..w as isize {
            let e = at(x, y);
            let (b, d, f, hh) = (at(x, y - 1), at(x - 1, y), at(x + 1, y), at(x, y + 1));
            let (e0, e1, e2, e3) = if b != hh && d != f {
                (
                    if d == b { d } else { e },
                    if b == f { f } else { e },
                    if d == hh { d } else { e },
                    if hh == f { f } else { e },
                )
            } else {
                (e, e, e, e)
            };
            let (ox, oy) = (x as usize * 2, y as usize * 2);
            out[oy * ow + ox] = e0;
            out[oy * ow + ox + 1] = e1;
            out[(oy + 1) * ow + ox] = e2;
            out[(oy + 1) * ow + ox + 1] = e3;
        }
    }
    out
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

    #[test]
    fn shifted_keep_round_trips_off_the_canvas() {
        // new_filled paints the padding of the edge tiles too: it must not
        // leak in as content.
        let r = Raster::new_filled(100, 70, Rgba8::rgb(9, 8, 7));
        for &(dx, dy) in &[(30, -10), (-130, 5), (64, 64), (-1, 69)] {
            let moved = r.shifted_keep(dx, dy);
            assert!(moved.has_outside());
            assert_eq!(moved.get_pixel_any(dx, dy), Rgba8::rgb(9, 8, 7));
            assert_eq!(moved.get_pixel_any(dx + 100, dy), Rgba8::TRANSPARENT, "padding stays out");
            assert_eq!(moved.full_bounds(), Some(IRect::new(dx, dy, 100, 70)));
            let back = moved.shifted_keep(-dx, -dy);
            assert!(!back.has_outside());
            assert_eq!(back.to_rgba(), r.to_rgba());
        }
        // Flipping the canvas mirrors outside pixels too.
        let mut a = Raster::new(10, 10);
        a.set_pixel_any(-3, 2, Rgba8::WHITE);
        assert_eq!(a.get_pixel(0, 2), Rgba8::TRANSPARENT);
        assert_eq!(a.flipped_h().get_pixel_any(12, 2), Rgba8::WHITE);
        assert_eq!(a.with_canvas_size(13, 10, 3, 0).get_pixel(0, 2), Rgba8::WHITE);
    }
}
