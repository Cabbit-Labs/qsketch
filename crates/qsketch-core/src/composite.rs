//! Layer stack compositor. Produces premultiplied RGBA8 tiles ready for GPU
//! upload, recomputing only dirty tiles, in parallel.

use rayon::prelude::*;

use crate::blend::{composite_pixel, BlendMode};
use crate::document::DocState;
use crate::geom::IRect;
use crate::raster::{tile_rect, Raster, Tile, TILE, TILE_BYTES, TILE_PX};

/// A bitset over tile coordinates.
#[derive(Clone, Debug)]
pub struct TileSet {
    tiles_x: u32,
    tiles_y: u32,
    bits: Vec<u64>,
}

impl TileSet {
    pub fn new(tiles_x: u32, tiles_y: u32) -> Self {
        let n = (tiles_x * tiles_y) as usize;
        Self { tiles_x, tiles_y, bits: vec![0; n.div_ceil(64)] }
    }

    pub fn for_size(width: u32, height: u32) -> Self {
        Self::new(width.div_ceil(TILE as u32), height.div_ceil(TILE as u32))
    }

    pub fn tiles_x(&self) -> u32 {
        self.tiles_x
    }
    pub fn tiles_y(&self) -> u32 {
        self.tiles_y
    }

    #[inline]
    pub fn insert(&mut self, tx: u32, ty: u32) {
        if tx < self.tiles_x && ty < self.tiles_y {
            let i = (ty * self.tiles_x + tx) as usize;
            self.bits[i / 64] |= 1 << (i % 64);
        }
    }

    #[inline]
    pub fn insert_index(&mut self, i: usize) {
        if i < (self.tiles_x * self.tiles_y) as usize {
            self.bits[i / 64] |= 1 << (i % 64);
        }
    }

    #[inline]
    pub fn contains_index(&self, i: usize) -> bool {
        i < (self.tiles_x * self.tiles_y) as usize && self.bits[i / 64] & (1 << (i % 64)) != 0
    }

    /// Mark every tile overlapping `rect` (pixel coordinates).
    pub fn insert_rect(&mut self, rect: IRect) {
        let full = IRect::new(0, 0, (self.tiles_x as usize * TILE) as i32, (self.tiles_y as usize * TILE) as i32);
        let r = rect.intersect(&full);
        if r.is_empty() {
            return;
        }
        let tx0 = r.x as u32 / TILE as u32;
        let ty0 = r.y as u32 / TILE as u32;
        let tx1 = (r.right() - 1) as u32 / TILE as u32;
        let ty1 = (r.bottom() - 1) as u32 / TILE as u32;
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                self.insert(tx, ty);
            }
        }
    }

    pub fn insert_all(&mut self) {
        let n = (self.tiles_x * self.tiles_y) as usize;
        for (i, w) in self.bits.iter_mut().enumerate() {
            let remaining = n.saturating_sub(i * 64);
            *w = if remaining >= 64 { u64::MAX } else { (1u64 << remaining) - 1 };
        }
    }

    pub fn clear(&mut self) {
        self.bits.fill(0);
    }

    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&w| w == 0)
    }

    pub fn len(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn union_with(&mut self, other: &TileSet) {
        for (a, b) in self.bits.iter_mut().zip(&other.bits) {
            *a |= *b;
        }
    }

    /// Iterate set tile coordinates `(tx, ty)`.
    pub fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        let n = (self.tiles_x * self.tiles_y) as usize;
        (0..n).filter(move |&i| self.contains_index(i)).map(move |i| (i as u32 % self.tiles_x, i as u32 / self.tiles_x))
    }

    /// Bounding pixel rect of all set tiles.
    pub fn bounding_rect(&self) -> IRect {
        let mut acc = IRect::EMPTY;
        for (tx, ty) in self.iter() {
            acc = acc.union(&tile_rect(tx, ty));
        }
        acc
    }
}

/// Composited document as premultiplied RGBA8 tiles (dense).
pub struct Composite {
    width: u32,
    height: u32,
    tiles_x: u32,
    tiles_y: u32,
    tiles: Vec<Box<Tile>>,
}

impl Composite {
    pub fn new(width: u32, height: u32) -> Self {
        let tiles_x = width.div_ceil(TILE as u32);
        let tiles_y = height.div_ceil(TILE as u32);
        let n = (tiles_x * tiles_y) as usize;
        let mut tiles = Vec::with_capacity(n);
        for _ in 0..n {
            tiles.push(zeroed_box());
        }
        Self { width, height, tiles_x, tiles_y, tiles }
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

    #[inline]
    pub fn tile(&self, tx: u32, ty: u32) -> &Tile {
        &self.tiles[(ty * self.tiles_x + tx) as usize]
    }

    /// Premultiplied pixel.
    pub fn get_premul(&self, x: i32, y: i32) -> [u8; 4] {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return [0; 4];
        }
        let t = self.tile(x as u32 / TILE as u32, y as u32 / TILE as u32);
        let i = ((y as usize % TILE) * TILE + (x as usize % TILE)) * 4;
        [t.px[i], t.px[i + 1], t.px[i + 2], t.px[i + 3]]
    }

    /// Recomposite the tiles in `dirty` from `doc`. Parallel over tiles.
    pub fn update(&mut self, doc: &DocState, dirty: &TileSet) {
        debug_assert_eq!(doc.width, self.width);
        debug_assert_eq!(doc.height, self.height);
        let tiles_x = self.tiles_x;
        self.tiles.par_iter_mut().enumerate().filter(|(i, _)| dirty.contains_index(*i)).for_each(|(i, out)| {
            let tx = i as u32 % tiles_x;
            let ty = i as u32 / tiles_x;
            let mut buf = [[0f32; 4]; TILE_PX];
            composite_tile_straight(doc, tx, ty, &mut buf);
            for (p, o) in buf.iter().zip(out.px.chunks_exact_mut(4)) {
                let a = p[3].clamp(0.0, 1.0);
                o[0] = (p[0].clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
                o[1] = (p[1].clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
                o[2] = (p[2].clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
                o[3] = (a * 255.0 + 0.5) as u8;
            }
        });
    }

    /// Straight-alpha RGBA8 image of the whole composite (for export/thumbnails).
    pub fn to_rgba_straight(&self) -> Vec<u8> {
        let mut out = vec![0u8; (self.width * self.height * 4) as usize];
        let stride = self.width as usize * 4;
        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                let t = self.tile(tx, ty);
                let x0 = tx as usize * TILE;
                let y0 = ty as usize * TILE;
                let w = TILE.min(self.width as usize - x0);
                let h = TILE.min(self.height as usize - y0);
                for ly in 0..h {
                    let dst = &mut out[(y0 + ly) * stride + x0 * 4..][..w * 4];
                    let src = &t.px[ly * TILE * 4..][..w * 4];
                    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                        let a = s[3] as u32;
                        if a == 0 {
                            d.copy_from_slice(&[0, 0, 0, 0]);
                        } else if a == 255 {
                            d.copy_from_slice(s);
                        } else {
                            d[0] = ((s[0] as u32 * 255 + a / 2) / a).min(255) as u8;
                            d[1] = ((s[1] as u32 * 255 + a / 2) / a).min(255) as u8;
                            d[2] = ((s[2] as u32 * 255 + a / 2) / a).min(255) as u8;
                            d[3] = s[3];
                        }
                    }
                }
            }
        }
        out
    }

    /// Premultiplied RGBA8 image of the whole composite.
    pub fn to_rgba_premul(&self) -> Vec<u8> {
        let mut out = vec![0u8; (self.width * self.height * 4) as usize];
        let stride = self.width as usize * 4;
        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                let t = self.tile(tx, ty);
                let x0 = tx as usize * TILE;
                let y0 = ty as usize * TILE;
                let w = TILE.min(self.width as usize - x0);
                let h = TILE.min(self.height as usize - y0);
                for ly in 0..h {
                    out[(y0 + ly) * stride + x0 * 4..][..w * 4].copy_from_slice(&t.px[ly * TILE * 4..][..w * 4]);
                }
            }
        }
        out
    }
}

fn zeroed_box() -> Box<Tile> {
    let layout = std::alloc::Layout::new::<Tile>();
    // SAFETY: Tile is a plain byte array, zero is valid.
    unsafe {
        let p = std::alloc::alloc_zeroed(layout) as *mut Tile;
        if p.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Box::from_raw(p)
    }
}

/// Composite one tile of `doc` into `out` as straight-alpha f32 RGBA.
pub fn composite_tile_straight(doc: &DocState, tx: u32, ty: u32, out: &mut [[f32; 4]; TILE_PX]) {
    for p in out.iter_mut() {
        *p = [0.0; 4];
    }
    composite_range(doc, tx, ty, 0..doc.layers.len(), None, out);
}

/// Composite the direct children of `parent` found in `range` onto `out`
/// (bottom to top). A group child is rendered into its own buffer and then
/// blended with the group's opacity and blend mode, so members composite
/// against each other first, as in Photoshop's "Normal" group mode.
pub fn composite_range(
    doc: &DocState,
    tx: u32,
    ty: u32,
    range: std::ops::Range<usize>,
    parent: Option<crate::layer::LayerId>,
    out: &mut [[f32; 4]; TILE_PX],
) {
    // Index of the most recent non-clipped visible layer (clipping base).
    let mut base: Option<usize> = None;
    for li in range {
        let layer = &doc.layers[li];
        if layer.props.parent != parent {
            continue; // inside a nested group; drawn when its entry comes up
        }
        if !layer.props.visible {
            continue;
        }
        if !layer.props.clipped {
            base = Some(li);
        }
        let opacity = layer.props.opacity;
        if opacity <= 0.0 {
            continue;
        }
        let mode = layer.props.blend;
        let clip_tile = if layer.props.clipped {
            match base {
                Some(bi) if bi != li && !doc.layers[bi].is_group() => match doc.layers[bi].raster.tile(tx, ty) {
                    Some(t) => Some(t),
                    None => continue, // base is transparent here: nothing shows
                },
                _ => None,
            }
        } else {
            None
        };
        if layer.is_group() && mode == BlendMode::PassThrough {
            // Members blend straight onto what is below the group; a partial
            // opacity fades the members' effect back toward the backdrop.
            if opacity >= 1.0 {
                composite_range(doc, tx, ty, doc.members(li), Some(layer.props.id), out);
            } else {
                let mut buf = *out;
                composite_range(doc, tx, ty, doc.members(li), Some(layer.props.id), &mut buf);
                for (o, b) in out.iter_mut().zip(buf.iter()) {
                    for c in 0..4 {
                        o[c] += (b[c] - o[c]) * opacity;
                    }
                }
            }
            continue;
        }
        if layer.is_group() {
            let mut buf = [[0f32; 4]; TILE_PX];
            composite_range(doc, tx, ty, doc.members(li), Some(layer.props.id), &mut buf);
            for (i, (o, src)) in out.iter_mut().zip(buf.iter()).enumerate() {
                if src[3] <= 0.0 {
                    continue;
                }
                let mut src = *src;
                if let Some(ct) = clip_tile {
                    src[3] *= ct.px[i * 4 + 3] as f32 / 255.0;
                }
                *o = composite_pixel(mode, *o, src, opacity);
            }
            continue;
        }
        let Some(tile) = layer.raster.tile(tx, ty) else {
            continue;
        };
        let fast_normal = mode == BlendMode::Normal && opacity >= 1.0 && clip_tile.is_none();
        for (i, o) in out.iter_mut().enumerate() {
            let s = &tile.px[i * 4..i * 4 + 4];
            let sa = s[3];
            if sa == 0 {
                continue;
            }
            if fast_normal && sa == 255 {
                *o = [s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0, 1.0];
                continue;
            }
            let mut src = [s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0, sa as f32 / 255.0];
            if let Some(ct) = clip_tile {
                src[3] *= ct.px[i * 4 + 3] as f32 / 255.0;
            }
            *o = composite_pixel(mode, *o, src, opacity);
        }
    }
}

/// Flatten all visible layers into a single straight-alpha raster.
pub fn flatten(doc: &DocState) -> Raster {
    flatten_range(doc, 0..doc.layers.len(), None)
}

/// Flatten the direct children of `parent` within `range` (a group's
/// members, say) into one straight-alpha raster.
pub fn flatten_range(doc: &DocState, range: std::ops::Range<usize>, parent: Option<crate::layer::LayerId>) -> Raster {
    let mut out = Raster::new(doc.width, doc.height);
    let mut buf = [[0f32; 4]; TILE_PX];
    for ty in 0..out.tiles_y() {
        for tx in 0..out.tiles_x() {
            for p in buf.iter_mut() {
                *p = [0.0; 4];
            }
            composite_range(doc, tx, ty, range.clone(), parent, &mut buf);
            if buf.iter().all(|p| p[3] <= 0.0) {
                continue;
            }
            let t = out.tile_mut(tx, ty);
            let mut bytes = [0u8; TILE_BYTES];
            for (p, o) in buf.iter().zip(bytes.chunks_exact_mut(4)) {
                o[0] = (p[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                o[1] = (p[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                o[2] = (p[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                o[3] = (p[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            t.px = bytes;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    #[test]
    fn tileset_basic() {
        let mut s = TileSet::for_size(200, 100);
        assert_eq!((s.tiles_x(), s.tiles_y()), (4, 2));
        s.insert_rect(IRect::new(60, 60, 10, 10));
        let v: Vec<_> = s.iter().collect();
        assert_eq!(v, vec![(0, 0), (1, 0), (0, 1), (1, 1)]);
        s.insert_all();
        assert_eq!(s.len(), 8);
        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    fn composite_two_layers() {
        let mut doc = DocState::new(64, 64, Some(Rgba8::WHITE));
        let id = doc.add_layer("top", None);
        let li = doc.index_of(id).unwrap();
        doc.layers[li].raster.set_pixel(1, 1, Rgba8::new(0, 0, 0, 128));
        doc.layers[li].props.opacity = 0.5;
        let mut c = Composite::new(64, 64);
        let mut d = TileSet::for_size(64, 64);
        d.insert_all();
        c.update(&doc, &d);
        let p = c.get_premul(1, 1);
        // white * (1 - 0.25) ≈ 191
        assert!((p[0] as i32 - 191).abs() <= 1);
        assert_eq!(p[3], 255);
        assert_eq!(c.get_premul(0, 0), [255, 255, 255, 255]);
        let flat = flatten(&doc);
        assert_eq!(flat.get_pixel(0, 0), Rgba8::WHITE);
    }

    #[test]
    fn composite_groups() {
        // White background; a group at 50% holding an opaque black layer:
        // the group's opacity applies to the members' composite.
        let mut doc = DocState::new(64, 64, Some(Rgba8::WHITE));
        let a = doc.add_layer("a", None);
        let b = doc.add_layer("b", None);
        let ai = doc.index_of(a).unwrap();
        doc.layers[ai].raster.set_pixel(1, 1, Rgba8::BLACK);
        doc.layers[ai].raster.set_pixel(2, 2, Rgba8::BLACK);
        let bi = doc.index_of(b).unwrap();
        doc.layers[bi].raster.set_pixel(2, 2, Rgba8::new(255, 0, 0, 255));
        let g = doc.group_layers(&[a, b]).unwrap();
        let gi = doc.index_of(g).unwrap();
        doc.layers[gi].props.opacity = 0.5;
        let flat = flatten(&doc);
        let p = flat.get_pixel(1, 1);
        assert!((p.r as i32 - 128).abs() <= 1, "{p:?}");
        // b covers a inside the group, then the whole thing is 50% over white.
        let p = flat.get_pixel(2, 2);
        assert!((p.r as i32 - 255).abs() <= 1 && (p.g as i32 - 128).abs() <= 1, "{p:?}");
        doc.layers[gi].props.visible = false;
        assert_eq!(flatten(&doc).get_pixel(1, 1), Rgba8::WHITE);
    }

    #[test]
    fn pass_through_group() {
        // Gray background, group holding a Multiply layer of 50% gray.
        let mut doc = DocState::new(64, 64, Some(Rgba8::new(128, 128, 128, 255)));
        let a = doc.add_layer("a", None);
        let ai = doc.index_of(a).unwrap();
        doc.layers[ai].raster.set_pixel(0, 0, Rgba8::new(128, 128, 128, 255));
        doc.layers[ai].props.blend = BlendMode::Multiply;
        let g = doc.group_layers(&[a]).unwrap();
        let gi = doc.index_of(g).unwrap();
        assert_eq!(doc.layers[gi].props.blend, BlendMode::PassThrough);
        // Pass through: multiply sees the background → 64.
        let p = flatten(&doc).get_pixel(0, 0);
        assert!((p.r as i32 - 64).abs() <= 1, "{p:?}");
        // Normal group: multiply against transparent = plain 128 over the bg.
        doc.layers[gi].props.blend = BlendMode::Normal;
        let p = flatten(&doc).get_pixel(0, 0);
        assert!((p.r as i32 - 128).abs() <= 1, "{p:?}");
        // Pass through at 50%: halfway between 128 and 64.
        doc.layers[gi].props.blend = BlendMode::PassThrough;
        doc.layers[gi].props.opacity = 0.5;
        let p = flatten(&doc).get_pixel(0, 0);
        assert!((p.r as i32 - 96).abs() <= 1, "{p:?}");
    }
}
