//! Aseprite `.ase` / `.aseprite` files.
//!
//! The parser keeps the whole sprite — every frame, its cels, tags, tilesets
//! and user data — in [`AseSprite`]; [`AseSprite::to_doc`] turns it into a
//! qsketch document with all of its frames, linked cels, tags, tilemaps and
//! slices. [`encode`] writes the same back, so a sprite survives a round
//! trip through qsketch.
//!
//! Format reference: <https://github.com/aseprite/aseprite/blob/main/docs/ase-file-specs.md>

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{bail, Context};

use crate::anim::{Cel, Frame, Tag, TagDirection};
use crate::blend::BlendMode;
use crate::color::Rgba8;
use crate::document::DocState;
use crate::geom::IRect;
use crate::layer::{Layer, LayerId, LayerKind, LayerProps};
use crate::raster::Raster;
use crate::tilemap::{Tilemap, Tileset, FLIP_H, FLIP_V};

pub const EXTENSIONS: &[&str] = &["ase", "aseprite"];

pub fn is_ase(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

const FILE_MAGIC: u16 = 0xA5E0;
const FRAME_MAGIC: u16 = 0xF1FA;

const CHUNK_OLD_PALETTE: u16 = 0x0004;
const CHUNK_OLD_PALETTE_6BIT: u16 = 0x0011;
const CHUNK_LAYER: u16 = 0x2004;
const CHUNK_CEL: u16 = 0x2005;
const CHUNK_COLOR_PROFILE: u16 = 0x2007;
const CHUNK_TAGS: u16 = 0x2018;
const CHUNK_PALETTE: u16 = 0x2019;
const CHUNK_USER_DATA: u16 = 0x2020;
const CHUNK_SLICE: u16 = 0x2022;
const CHUNK_TILESET: u16 = 0x2023;

// Layer flags.
const LAYER_VISIBLE: u16 = 1;
const LAYER_EDITABLE: u16 = 2;
const LAYER_BACKGROUND: u16 = 8;
const LAYER_PREFER_LINKED: u16 = 16;
const LAYER_COLLAPSED: u16 = 32;

const LAYER_KIND_IMAGE: u16 = 0;
const LAYER_KIND_GROUP: u16 = 1;
const LAYER_KIND_TILEMAP: u16 = 2;

// Tilemap cel bit masks (Aseprite's defaults; the file states them too).
const TILE_ID_MASK: u32 = 0x1fff_ffff;
const TILE_FLIP_X: u32 = 0x8000_0000;
const TILE_FLIP_Y: u32 = 0x4000_0000;
const TILE_FLIP_D: u32 = 0x2000_0000;

// ---------------------------------------------------------------------------
// Parsed sprite

#[derive(Clone, Debug)]
pub struct AseSprite {
    pub width: u32,
    pub height: u32,
    /// Bits per pixel: 32 RGBA, 16 grayscale+alpha, 8 indexed.
    pub depth: u16,
    pub transparent_index: u8,
    pub palette: Vec<Rgba8>,
    /// Whether the file had a palette chunk at all.
    pub has_palette: bool,
    /// In file order (bottom to top, a group before its children).
    pub layers: Vec<AseLayer>,
    pub frames: Vec<AseFrame>,
    pub tags: Vec<AseTag>,
    pub tilesets: Vec<AseTileset>,
    pixel_aspect: [u8; 2],
    slices: Vec<crate::slice::Slice>,
}

/// Aseprite's "user data": a color and a note that layers, cels, tags and
/// slices can carry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserData {
    pub text: String,
    pub color: Option<[u8; 4]>,
}

#[derive(Clone, Debug)]
pub struct AseLayer {
    pub name: String,
    pub flags: u16,
    pub kind: u16,
    pub child_level: u16,
    pub blend: u16,
    pub opacity: u8,
    /// Tilemap layers: the id of their tileset (see `AseTileset::id`).
    pub tileset: Option<u32>,
    pub user: UserData,
}

impl AseLayer {
    pub fn visible(&self) -> bool {
        self.flags & LAYER_VISIBLE != 0
    }
    pub fn is_group(&self) -> bool {
        self.kind == LAYER_KIND_GROUP
    }
}

#[derive(Clone, Debug)]
pub struct AseFrame {
    pub duration_ms: u16,
    pub cels: Vec<AseCel>,
}

#[derive(Clone, Debug)]
pub struct AseCel {
    /// Index into `AseSprite::layers`.
    pub layer: usize,
    pub x: i32,
    pub y: i32,
    pub opacity: u8,
    pub z_index: i16,
    pub image: CelImage,
    pub user: UserData,
}

#[derive(Clone, Debug)]
pub enum CelImage {
    /// Raw pixels in the sprite's depth, `w * h * bytes_per_pixel`.
    Pixels {
        w: u32,
        h: u32,
        data: Vec<u8>,
    },
    /// Same image as this layer's cel in another frame.
    Linked(usize),
    /// Tile ids (with flip bits) of a tilemap cel, `w * h` of them.
    Tiles {
        w: u32,
        h: u32,
        cells: Vec<u32>,
        flip_x: u32,
        flip_y: u32,
        flip_d: u32,
        id_mask: u32,
    },
    Unsupported,
}

#[derive(Clone, Debug)]
pub struct AseTag {
    pub name: String,
    pub from: u16,
    pub to: u16,
    /// 0 forward, 1 reverse, 2 ping-pong, 3 ping-pong reverse.
    pub direction: u8,
    pub repeat: u16,
    pub color: [u8; 3],
    pub user: UserData,
}

#[derive(Clone, Debug)]
pub struct AseTileset {
    pub id: u32,
    pub name: String,
    pub tile_w: u32,
    pub tile_h: u32,
    pub count: u32,
    pub base_index: i16,
    /// Raw pixels of the tiles stacked vertically, in the sprite's depth.
    pub pixels: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Reading

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        if self.p + n > self.b.len() {
            bail!("unexpected end of file");
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> anyhow::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> anyhow::Result<u16> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    fn i16(&mut self) -> anyhow::Result<i16> {
        Ok(self.u16()? as i16)
    }
    fn u32(&mut self) -> anyhow::Result<u32> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn string(&mut self) -> anyhow::Result<String> {
        let n = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

/// What the next user data chunk belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UserTarget {
    None,
    Layer(usize),
    Cel(usize),
    /// The tags of the last tags chunk, in turn: (next tag, how many are left).
    Tags(usize, usize),
    Slice(usize),
}

fn inflate(src: &[u8], want: usize) -> anyhow::Result<Vec<u8>> {
    let mut data = Vec::with_capacity(want);
    flate2::read::ZlibDecoder::new(src).take(want as u64).read_to_end(&mut data).context("decompressing")?;
    if data.len() != want {
        bail!("compressed data is {} bytes, expected {want}", data.len());
    }
    Ok(data)
}

/// Parse a whole sprite from its bytes.
pub fn parse(bytes: &[u8]) -> anyhow::Result<AseSprite> {
    let mut c = Cur { b: bytes, p: 0 };
    let _file_size = c.u32()?;
    if c.u16()? != FILE_MAGIC {
        bail!("not an Aseprite file");
    }
    let frame_count = c.u16()? as usize;
    let width = c.u16()? as u32;
    let height = c.u16()? as u32;
    let depth = c.u16()?;
    let flags = c.u32()?;
    c.u16()?; // deprecated speed
    c.u32()?;
    c.u32()?;
    let transparent_index = c.u8()?;
    c.take(3)?;
    let _num_colors = c.u16()?;
    let pixel_w = c.u8()?;
    let pixel_h = c.u8()?;
    c.take(2 + 2 + 2 + 2)?; // grid x, y, w, h
    c.take(84)?;
    if !matches!(depth, 8 | 16 | 32) {
        bail!("unsupported color depth {depth}");
    }
    if width == 0 || height == 0 || width > 65_535 || height > 65_535 {
        bail!("implausible sprite size {width}x{height}");
    }
    let layer_opacity_valid = flags & 1 != 0;
    let bpp = (depth / 8) as usize;

    let mut sprite = AseSprite {
        width,
        height,
        depth,
        transparent_index,
        palette: vec![Rgba8::new(0, 0, 0, 255); 256],
        has_palette: false,
        layers: Vec::new(),
        frames: Vec::with_capacity(frame_count),
        tags: Vec::new(),
        tilesets: Vec::new(),
        // 0 in either byte means "square" (older files).
        pixel_aspect: if pixel_w == 0 || pixel_h == 0 { [1, 1] } else { [pixel_w, pixel_h] },
        slices: Vec::new(),
    };

    for _ in 0..frame_count {
        let frame_start = c.p;
        let frame_bytes = c.u32()? as usize;
        if c.u16()? != FRAME_MAGIC {
            bail!("bad frame header");
        }
        let old_chunks = c.u16()? as usize;
        let duration_ms = c.u16()?;
        c.take(2)?;
        let new_chunks = c.u32()? as usize;
        let chunks = if new_chunks == 0 { old_chunks } else { new_chunks };
        let mut frame = AseFrame { duration_ms, cels: Vec::new() };
        let mut target = UserTarget::None;
        for _ in 0..chunks {
            let chunk_start = c.p;
            let size = c.u32()? as usize;
            let kind = c.u16()?;
            if size < 6 {
                bail!("bad chunk size");
            }
            let end = chunk_start + size;
            if end > bytes.len() {
                bail!("chunk runs past end of file");
            }
            let mut d = Cur { b: &bytes[..end], p: c.p };
            let mut next_target = UserTarget::None;
            match kind {
                CHUNK_LAYER => {
                    let flags = d.u16()?;
                    let kind = d.u16()?;
                    let child_level = d.u16()?;
                    d.u16()?; // default width
                    d.u16()?; // default height
                    let blend = d.u16()?;
                    let opacity = d.u8()?;
                    d.take(3)?;
                    let name = d.string()?;
                    let tileset = if kind == LAYER_KIND_TILEMAP { d.u32().ok() } else { None };
                    sprite.layers.push(AseLayer {
                        name,
                        flags,
                        kind,
                        child_level,
                        blend,
                        opacity: if layer_opacity_valid { opacity } else { 255 },
                        tileset,
                        user: UserData::default(),
                    });
                    next_target = UserTarget::Layer(sprite.layers.len() - 1);
                }
                CHUNK_CEL => {
                    let layer = d.u16()? as usize;
                    let x = d.i16()? as i32;
                    let y = d.i16()? as i32;
                    let opacity = d.u8()?;
                    let cel_type = d.u16()?;
                    let z_index = d.i16()?;
                    d.take(5)?;
                    let image = match cel_type {
                        0 => {
                            let w = d.u16()? as u32;
                            let h = d.u16()? as u32;
                            let data = d.take(w as usize * h as usize * bpp)?.to_vec();
                            CelImage::Pixels { w, h, data }
                        }
                        1 => CelImage::Linked(d.u16()? as usize),
                        2 => {
                            let w = d.u16()? as u32;
                            let h = d.u16()? as u32;
                            let data = inflate(&bytes[d.p..end], w as usize * h as usize * bpp).context("cel")?;
                            CelImage::Pixels { w, h, data }
                        }
                        3 => {
                            let w = d.u16()? as u32;
                            let h = d.u16()? as u32;
                            let bits = d.u16()?;
                            let id_mask = d.u32()?;
                            let flip_x = d.u32()?;
                            let flip_y = d.u32()?;
                            let flip_d = d.u32()?;
                            d.take(10)?;
                            let per = (bits / 8).max(1) as usize;
                            let raw =
                                inflate(&bytes[d.p..end], w as usize * h as usize * per).context("tilemap cel")?;
                            let cells = raw
                                .chunks_exact(per)
                                .map(|b| match per {
                                    1 => b[0] as u32,
                                    2 => u16::from_le_bytes([b[0], b[1]]) as u32,
                                    _ => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                                })
                                .collect();
                            CelImage::Tiles { w, h, cells, flip_x, flip_y, flip_d, id_mask }
                        }
                        _ => CelImage::Unsupported,
                    };
                    frame.cels.push(AseCel { layer, x, y, opacity, z_index, image, user: UserData::default() });
                    next_target = UserTarget::Cel(frame.cels.len() - 1);
                }
                CHUNK_PALETTE => {
                    sprite.has_palette = true;
                    let new_size = d.u32()? as usize;
                    let first = d.u32()? as usize;
                    let last = d.u32()? as usize;
                    d.take(8)?;
                    if new_size > sprite.palette.len() {
                        sprite.palette.resize(new_size.min(65_536), Rgba8::new(0, 0, 0, 255));
                    }
                    if new_size > 0 && new_size < sprite.palette.len() {
                        sprite.palette.truncate(new_size);
                    }
                    for i in first..=last {
                        let f = d.u16()?;
                        let rgba = d.take(4)?;
                        if f & 1 != 0 {
                            d.string()?;
                        }
                        if let Some(p) = sprite.palette.get_mut(i) {
                            *p = Rgba8::new(rgba[0], rgba[1], rgba[2], rgba[3]);
                        }
                    }
                }
                CHUNK_OLD_PALETTE | CHUNK_OLD_PALETTE_6BIT => {
                    // Only meaningful when no new-style palette follows; the
                    // new chunk overwrites these entries anyway.
                    sprite.has_palette = true;
                    let packets = d.u16()?;
                    let mut idx = 0usize;
                    for _ in 0..packets {
                        idx += d.u8()? as usize;
                        let n = match d.u8()? {
                            0 => 256,
                            n => n as usize,
                        };
                        for _ in 0..n {
                            let rgb = d.take(3)?;
                            let scale = |v: u8| if kind == CHUNK_OLD_PALETTE_6BIT { v.saturating_mul(4) } else { v };
                            if let Some(p) = sprite.palette.get_mut(idx) {
                                *p = Rgba8::new(scale(rgb[0]), scale(rgb[1]), scale(rgb[2]), 255);
                            }
                            idx += 1;
                        }
                    }
                }
                CHUNK_TAGS => {
                    let n = d.u16()? as usize;
                    d.take(8)?;
                    let first = sprite.tags.len();
                    for _ in 0..n {
                        let from = d.u16()?;
                        let to = d.u16()?;
                        let direction = d.u8()?;
                        let repeat = d.u16()?;
                        d.take(6)?;
                        let rgb = d.take(3)?;
                        let color = [rgb[0], rgb[1], rgb[2]];
                        d.u8()?; // extra byte
                        let name = d.string()?;
                        sprite.tags.push(AseTag {
                            name,
                            from,
                            to,
                            direction,
                            repeat,
                            color,
                            user: UserData::default(),
                        });
                    }
                    next_target = UserTarget::Tags(first, n);
                }
                CHUNK_TILESET => {
                    let id = d.u32()?;
                    let flags = d.u32()?;
                    let count = d.u32()?;
                    let tile_w = d.u16()? as u32;
                    let tile_h = d.u16()? as u32;
                    let base_index = d.i16()?;
                    d.take(14)?;
                    let name = d.string()?;
                    if flags & 1 != 0 {
                        d.u32()?; // external file id
                        d.u32()?; // tileset id there
                    }
                    let pixels = if flags & 2 != 0 {
                        let len = d.u32()? as usize;
                        let want = (count * tile_w * tile_h) as usize * bpp;
                        let src = &bytes[d.p..end.min(d.p + len)];
                        inflate(src, want).ok()
                    } else {
                        None
                    };
                    sprite.tilesets.push(AseTileset { id, name, tile_w, tile_h, count, base_index, pixels });
                }
                CHUNK_SLICE => {
                    let keys = d.u32()?;
                    let flags = d.u32()?;
                    d.u32()?;
                    let name = d.string()?;
                    // Only the first key (qsketch slices are the same in
                    // every frame); later keys are read past.
                    let mut slice: Option<crate::slice::Slice> = None;
                    for k in 0..keys {
                        d.u32()?; // frame
                        let x = d.u32()? as i32;
                        let y = d.u32()? as i32;
                        let w = d.u32()? as i32;
                        let h = d.u32()? as i32;
                        let center = if flags & 1 != 0 {
                            let (cx, cy, cw, ch) = (d.u32()? as i32, d.u32()? as i32, d.u32()? as i32, d.u32()? as i32);
                            Some(IRect::new(cx, cy, cw, ch))
                        } else {
                            None
                        };
                        let pivot = if flags & 2 != 0 { Some((d.u32()? as i32, d.u32()? as i32)) } else { None };
                        if k == 0 && w > 0 && h > 0 {
                            let mut s = crate::slice::Slice::new(name.clone(), IRect::new(x, y, w, h));
                            s.center = center;
                            s.pivot = pivot;
                            slice = Some(s);
                        }
                    }
                    if let Some(s) = slice {
                        sprite.slices.push(s);
                        next_target = UserTarget::Slice(sprite.slices.len() - 1);
                    }
                }
                CHUNK_USER_DATA => {
                    let flags = d.u32()?;
                    let mut user = UserData::default();
                    if flags & 1 != 0 {
                        user.text = d.string()?;
                    }
                    if flags & 2 != 0 {
                        let (r, g, b, a) = (d.u8()?, d.u8()?, d.u8()?, d.u8()?);
                        user.color = Some([r, g, b, a]);
                    }
                    // Property maps (flag 4) are skipped with the chunk.
                    match target {
                        UserTarget::Layer(i) => sprite.layers[i].user = user,
                        UserTarget::Cel(i) => frame.cels[i].user = user,
                        UserTarget::Slice(i) => {
                            if let Some([r, g, b, a]) = user.color {
                                sprite.slices[i].color = Rgba8::new(r, g, b, a.max(1));
                            }
                        }
                        UserTarget::Tags(next, left) if left > 0 => {
                            if let Some(t) = sprite.tags.get_mut(next) {
                                t.user = user;
                            }
                            next_target = UserTarget::Tags(next + 1, left - 1);
                        }
                        _ => {}
                    }
                }
                // Color profile, external files: skipped by size.
                _ => {}
            }
            target = next_target;
            c.p = end;
        }
        // Trust the frame size over the chunk walk.
        if frame_bytes >= 16 && frame_start + frame_bytes <= bytes.len() {
            c.p = frame_start + frame_bytes;
        }
        sprite.frames.push(frame);
    }
    Ok(sprite)
}

fn blend_from_ase(b: u16) -> Option<BlendMode> {
    Some(match b {
        0 => BlendMode::Normal,
        1 => BlendMode::Multiply,
        2 => BlendMode::Screen,
        3 => BlendMode::Overlay,
        4 => BlendMode::Darken,
        5 => BlendMode::Lighten,
        6 => BlendMode::ColorDodge,
        7 => BlendMode::ColorBurn,
        8 => BlendMode::HardLight,
        9 => BlendMode::SoftLight,
        10 => BlendMode::Difference,
        11 => BlendMode::Exclusion,
        12 => BlendMode::Hue,
        13 => BlendMode::Saturation,
        14 => BlendMode::Color,
        15 => BlendMode::Luminosity,
        16 => BlendMode::LinearDodge,
        17 => BlendMode::Subtract,
        18 => BlendMode::Divide,
        _ => return None,
    })
}

/// Aseprite blend id for a mode, or None when it has no equivalent.
fn blend_to_ase(m: BlendMode) -> Option<u16> {
    Some(match m {
        BlendMode::Normal => 0,
        BlendMode::Multiply => 1,
        BlendMode::Screen => 2,
        BlendMode::Overlay => 3,
        BlendMode::Darken => 4,
        BlendMode::Lighten => 5,
        BlendMode::ColorDodge => 6,
        BlendMode::ColorBurn => 7,
        BlendMode::HardLight => 8,
        BlendMode::SoftLight => 9,
        BlendMode::Difference => 10,
        BlendMode::Exclusion => 11,
        BlendMode::Hue => 12,
        BlendMode::Saturation => 13,
        BlendMode::Color => 14,
        BlendMode::Luminosity => 15,
        BlendMode::LinearDodge => 16,
        BlendMode::Subtract => 17,
        BlendMode::Divide => 18,
        BlendMode::LinearBurn | BlendMode::PassThrough => return None,
    })
}

impl AseSprite {
    /// Straight RGBA for `n` raw pixels in this sprite's depth. `background`
    /// keeps the transparent index opaque, as Aseprite does on a Background
    /// layer.
    fn rgba(&self, n: usize, data: &[u8], background: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(n * 4);
        match self.depth {
            32 => out.extend_from_slice(&data[..n * 4]),
            16 => {
                for p in data[..n * 2].chunks_exact(2) {
                    out.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
                }
            }
            _ => {
                for &i in &data[..n] {
                    if i == self.transparent_index && !background {
                        out.extend_from_slice(&[0, 0, 0, 0]);
                    } else {
                        let c = self.palette.get(i as usize).copied().unwrap_or(Rgba8::new(0, 0, 0, 255));
                        out.extend_from_slice(&[c.r, c.g, c.b, c.a]);
                    }
                }
            }
        }
        out
    }

    /// The frame whose cel holds layer `layer`'s picture in `frame`,
    /// following links; `None` when the frame has no cel there.
    fn cel_root(&self, frame: usize, layer: usize) -> Option<usize> {
        let mut fi = frame;
        for _ in 0..self.frames.len().max(1) {
            let cel = self.frames.get(fi)?.cels.iter().find(|c| c.layer == layer)?;
            match &cel.image {
                CelImage::Linked(f) if *f != fi => fi = *f,
                CelImage::Linked(_) => return None,
                _ => return Some(fi),
            }
        }
        None
    }

    fn cel(&self, frame: usize, layer: usize) -> Option<&AseCel> {
        self.frames.get(frame)?.cels.iter().find(|c| c.layer == layer)
    }

    /// A pixel cel as a canvas-sized raster.
    fn cel_raster(&self, cel: &AseCel, background: bool) -> Option<Raster> {
        let CelImage::Pixels { w, h, data } = &cel.image else { return None };
        let rgba = self.rgba((*w * *h) as usize, data, background);
        let img = Raster::from_rgba(*w, *h, &rgba);
        // A cel may reach past the sprite: that part is kept off the canvas.
        Some(img.with_canvas_size(self.width, self.height, cel.x, cel.y))
    }

    /// qsketch tilesets for the sprite's, in file order; `None` entries
    /// could not be read (external, or no pixels).
    fn tilesets(&self) -> Vec<Option<Tileset>> {
        self.tilesets
            .iter()
            .map(|t| {
                let px = t.pixels.as_ref()?;
                let count = t.count.max(1);
                let mut ts = Tileset::new(t.name.clone(), t.tile_w, t.tile_h);
                ts.tiles.clear();
                let strip = Raster::from_rgba(
                    t.tile_w,
                    t.tile_h * count,
                    &self.rgba((t.tile_w * t.tile_h * count) as usize, px, false),
                );
                for k in 0..count {
                    ts.tiles.push(strip.crop(IRect::new(0, (k * t.tile_h) as i32, t.tile_w as i32, t.tile_h as i32)));
                }
                Some(ts)
            })
            .collect()
    }

    /// A qsketch tilemap over the whole canvas from a tilemap cel.
    fn tilemap_from_cel(
        &self,
        cel: &AseCel,
        ts_index: usize,
        ts: &Tileset,
        dropped_diag: &mut bool,
    ) -> Option<Tilemap> {
        let CelImage::Tiles { w, h, cells, flip_x, flip_y, flip_d, id_mask } = &cel.image else { return None };
        let cols = self.width.div_ceil(ts.tile_w);
        let rows = self.height.div_ceil(ts.tile_h);
        let mut out = Tilemap { tileset: ts_index, cols, rows, cells: vec![0; (cols * rows) as usize] };
        let (ox, oy) = (cel.x.div_euclid(ts.tile_w as i32), cel.y.div_euclid(ts.tile_h as i32));
        for cy in 0..*h {
            for cx in 0..*w {
                let v = cells[(cy * w + cx) as usize];
                let (tx, ty) = (cx as i32 + ox, cy as i32 + oy);
                if tx < 0 || ty < 0 || tx >= cols as i32 || ty >= rows as i32 {
                    continue;
                }
                let mut q = v & id_mask;
                if q as usize >= ts.tiles.len() {
                    q = 0;
                }
                if v & flip_x != 0 {
                    q |= FLIP_H;
                }
                if v & flip_y != 0 {
                    q |= FLIP_V;
                }
                if v & flip_d != 0 {
                    *dropped_diag = true;
                }
                out.cells[(ty as u32 * cols + tx as u32) as usize] = q;
            }
        }
        Some(out)
    }

    fn render_tilemap(&self, tm: &Tilemap, ts: &Tileset) -> Raster {
        let mut r = Raster::new(self.width, self.height);
        for cy in 0..tm.rows {
            for cx in 0..tm.cols {
                crate::tilemap::render_cell(&mut r, ts, cx, cy, tm.cell(cx, cy));
            }
        }
        r
    }

    /// Build a document from the sprite: every frame, with linked cels,
    /// tags, tilemaps and slices. Returns the document and any warnings
    /// about what could not be represented.
    pub fn to_doc(&self) -> anyhow::Result<(DocState, Vec<String>)> {
        let mut warnings = Vec::new();
        if self.frames.is_empty() {
            bail!("the file has no frames");
        }
        let nframes = self.frames.len();
        // Layer tree from child levels: a layer at level N+1 belongs to the
        // most recent group at level N.
        struct Node {
            index: usize,
            children: Vec<Node>,
        }
        let mut roots: Vec<Node> = Vec::new();
        for (i, l) in self.layers.iter().enumerate() {
            let node = Node { index: i, children: Vec::new() };
            let mut level = l.child_level as usize;
            let mut list = &mut roots;
            while level > 0 {
                let descend = list.last().is_some_and(|p| self.layers[p.index].is_group());
                if !descend {
                    break;
                }
                list = &mut list.last_mut().expect("checked").children;
                level -= 1;
            }
            list.push(node);
        }
        // Emit in qsketch order: members (bottom to top) then the group entry.
        fn emit(node: Node, parent: Option<usize>, out: &mut Vec<(usize, Option<usize>)>) {
            for ch in node.children {
                emit(ch, Some(node.index), out);
            }
            out.push((node.index, parent));
        }
        let mut order: Vec<(usize, Option<usize>)> = Vec::new();
        for r in roots {
            emit(r, None, &mut order);
        }
        let mut id_of_file_index = vec![0 as LayerId; self.layers.len()];
        for (n, (fi, _)) in order.iter().enumerate() {
            id_of_file_index[*fi] = n as LayerId + 1;
        }
        let tilesets = self.tilesets();
        let mut used_tilesets: Vec<Tileset> = Vec::new();
        let mut tileset_map: Vec<Option<usize>> = vec![None; tilesets.len()];
        let mut dropped_diag = false;
        let mut layers: Vec<Layer> = Vec::with_capacity(order.len());
        for (fi, parent_fi) in &order {
            let al = &self.layers[*fi];
            let parent = parent_fi.map(|p| id_of_file_index[p]);
            let blend = match blend_from_ase(al.blend) {
                Some(b) => b,
                None => {
                    warnings.push(format!("Layer \"{}\": unknown blend mode {}, using Normal.", al.name, al.blend));
                    BlendMode::Normal
                }
            };
            let mut props = LayerProps {
                id: id_of_file_index[*fi],
                name: al.name.clone(),
                visible: al.visible(),
                locked: al.flags & LAYER_EDITABLE == 0,
                alpha_locked: false,
                opacity: al.opacity as f32 / 255.0,
                blend,
                clipped: false,
                kind: LayerKind::Raster,
                parent,
                expanded: al.flags & LAYER_COLLAPSED == 0,
                mask_enabled: true,
                style: Default::default(),
                adjustment: None,
                tilemap: None,
                shape: None,
                text: None,
                smart: None,
                continuous: al.flags & LAYER_PREFER_LINKED != 0,
                color: al.user.color.filter(|c| c[3] > 0),
                notes: al.user.text.clone(),
            };
            let mut raster = Raster::new(self.width, self.height);
            let mut cels: Vec<Cel> = Vec::new();
            match al.kind {
                LAYER_KIND_GROUP => props.kind = LayerKind::Group,
                LAYER_KIND_TILEMAP => {
                    // A tilemap layer that is the same in every frame stays a
                    // tilemap; one that animates becomes pixels per frame.
                    let ts_file = al.tileset.and_then(|id| self.tilesets.iter().position(|t| t.id == id));
                    let ts = ts_file.and_then(|k| tilesets[k].as_ref().map(|t| (k, t)));
                    let Some((k, ts)) = ts else {
                        warnings.push(format!(
                            "Layer \"{}\" uses a tileset that isn't in the file; it was left empty.",
                            al.name
                        ));
                        layers.push(Layer { props, raster, mask: None, cels, smart: None });
                        continue;
                    };
                    let roots: Vec<Option<usize>> = (0..nframes).map(|f| self.cel_root(f, *fi)).collect();
                    let distinct: std::collections::BTreeSet<Option<usize>> = roots.iter().copied().collect();
                    let qs_index = *tileset_map[k].get_or_insert_with(|| {
                        used_tilesets.push(ts.clone());
                        used_tilesets.len() - 1
                    });
                    let empty_map = || Tilemap {
                        tileset: qs_index,
                        cols: self.width.div_ceil(ts.tile_w),
                        rows: self.height.div_ceil(ts.tile_h),
                        cells: vec![0; (self.width.div_ceil(ts.tile_w) * self.height.div_ceil(ts.tile_h)) as usize],
                    };
                    if distinct.len() <= 1 {
                        let tm = roots[0]
                            .and_then(|r| self.cel(r, *fi))
                            .and_then(|cel| self.tilemap_from_cel(cel, qs_index, ts, &mut dropped_diag))
                            .unwrap_or_else(empty_map);
                        raster = self.render_tilemap(&tm, ts);
                        props.tilemap = Some(tm);
                        props.kind = LayerKind::Tilemap;
                    } else {
                        warnings.push(format!(
                            "Tilemap layer \"{}\" changes between frames; it was imported as a pixel layer.",
                            al.name
                        ));
                        for (f, root) in roots.iter().enumerate() {
                            match root {
                                Some(r) if *r != f => cels.push(Cel::linked(*r, self.width, self.height)),
                                Some(r) => {
                                    let cel = self.cel(*r, *fi).expect("root has a cel");
                                    let tm = self
                                        .tilemap_from_cel(cel, qs_index, ts, &mut dropped_diag)
                                        .unwrap_or_else(empty_map);
                                    let mut c = Cel::own(self.render_tilemap(&tm, ts));
                                    c.opacity = cel.opacity as f32 / 255.0;
                                    c.z_index = cel.z_index;
                                    cels.push(c);
                                }
                                None => cels.push(Cel::empty(self.width, self.height)),
                            }
                        }
                        raster = cels[cels[0].link.unwrap_or(0)].image.clone();
                    }
                }
                _ => {
                    let background = al.flags & LAYER_BACKGROUND != 0;
                    props.alpha_locked = background;
                    let mut unsupported = false;
                    for f in 0..nframes {
                        match self.cel_root(f, *fi) {
                            Some(r) if r != f => cels.push(Cel::linked(r, self.width, self.height)),
                            Some(r) => {
                                let cel = self.cel(r, *fi).expect("root has a cel");
                                match self.cel_raster(cel, background) {
                                    Some(img) => {
                                        let mut c = Cel::own(img);
                                        c.opacity = cel.opacity as f32 / 255.0;
                                        c.z_index = cel.z_index;
                                        cels.push(c);
                                    }
                                    None => {
                                        unsupported = true;
                                        cels.push(Cel::empty(self.width, self.height));
                                    }
                                }
                            }
                            None => cels.push(Cel::empty(self.width, self.height)),
                        }
                    }
                    if unsupported {
                        warnings.push(format!("Layer \"{}\" has an unsupported cel type; it was left empty.", al.name));
                    }
                    raster = cels[cels[0].link.unwrap_or(0)].image.clone();
                }
            }
            if nframes == 1 {
                // A single frame's cel opacity still counts: fold it into
                // the layer, since one-frame documents keep no cels.
                if let Some(c) = cels.first() {
                    props.opacity *= c.opacity;
                }
                cels.clear();
            }
            layers.push(Layer { props, raster, mask: None, cels, smart: None });
        }
        if dropped_diag {
            warnings.push("Diagonally flipped tiles aren't supported; they were placed unflipped.".into());
        }
        if layers.is_empty() {
            layers.push(Layer::new(1, "Layer 1", self.width, self.height));
        }
        let active = layers.iter().rposition(|l| !l.is_group()).unwrap_or(layers.len() - 1);
        let next_layer_id = layers.len() as LayerId + 1;
        let frames: Vec<Frame> =
            self.frames.iter().map(|f| Frame { duration_ms: (f.duration_ms as u32).max(1) }).collect();
        let tags: Vec<Tag> = self
            .tags
            .iter()
            .map(|t| {
                let color = match t.user.color {
                    Some([r, g, b, a]) if a > 0 => Rgba8::new(r, g, b, 255),
                    _ => Rgba8::new(t.color[0], t.color[1], t.color[2], 255),
                };
                let mut tag = Tag::new(t.name.clone(), t.from as usize, t.to as usize, color);
                tag.direction = TagDirection::from_ase(t.direction);
                tag.repeat = t.repeat;
                tag
            })
            .collect();
        let palette = if self.has_palette {
            let n = self.palette.iter().rposition(|c| c.a > 0).map_or(0, |i| i + 1);
            let colors: Vec<Rgba8> = self.palette.iter().copied().take(n.min(256)).collect();
            if colors.is_empty() {
                Default::default()
            } else {
                crate::palette::Palette::new("Aseprite", colors)
            }
        } else {
            Default::default()
        };
        let mut doc = DocState {
            width: self.width,
            height: self.height,
            layers,
            active,
            selection: None,
            next_layer_id,
            palette,
            palette_lock: self.depth == 8,
            pixel_aspect: self.pixel_aspect,
            slices: self.slices.clone(),
            tilesets: used_tilesets,
            frames,
            frame: 0,
            tags,
        };
        doc.repair_groups();
        doc.repair_animation();
        Ok((doc, warnings))
    }
}

/// Open an Aseprite file as a document (every frame), with warnings.
pub fn load_with_warnings(path: &Path) -> anyhow::Result<(DocState, Vec<String>)> {
    let bytes = std::fs::read(path).with_context(|| format!("opening {}", path.display()))?;
    let sprite = parse(&bytes).with_context(|| format!("reading {}", path.display()))?;
    sprite.to_doc()
}

pub fn load(path: &Path) -> anyhow::Result<DocState> {
    Ok(load_with_warnings(path)?.0)
}

// ---------------------------------------------------------------------------
// Writing

struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn zeros(&mut self, n: usize) {
        self.0.resize(self.0.len() + n, 0);
    }
    fn string(&mut self, s: &str) {
        let b = s.as_bytes();
        let b = &b[..b.len().min(u16::MAX as usize)];
        self.u16(b.len() as u16);
        self.bytes(b);
    }
    fn patch_u32(&mut self, at: usize, v: u32) {
        self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    /// Begin a chunk: writes a size placeholder + type, returns the offset.
    fn chunk(&mut self, kind: u16) -> usize {
        let at = self.0.len();
        self.u32(0);
        self.u16(kind);
        at
    }
    fn end_chunk(&mut self, at: usize) {
        let size = (self.0.len() - at) as u32;
        self.patch_u32(at, size);
    }
    /// A user data chunk (text and/or color); nothing when both are absent.
    fn user_data(&mut self, text: &str, color: Option<[u8; 4]>) -> bool {
        if text.is_empty() && color.is_none() {
            return false;
        }
        let at = self.chunk(CHUNK_USER_DATA);
        let flags = u32::from(!text.is_empty()) | (u32::from(color.is_some()) << 1);
        self.u32(flags);
        if !text.is_empty() {
            self.string(text);
        }
        if let Some(c) = color {
            self.bytes(&c);
        }
        self.end_chunk(at);
        true
    }
    /// A cel linked to `frame` on layer `layer`.
    fn linked_cel(&mut self, layer: u16, frame: u16) {
        let at = self.chunk(CHUNK_CEL);
        self.u16(layer);
        self.i16(0);
        self.i16(0);
        self.u8(255);
        self.u16(1);
        self.i16(0);
        self.zeros(5);
        self.u16(frame);
        self.end_chunk(at);
    }
}

fn deflate(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(data)?;
    Ok(z.finish()?)
}

/// What a save to `.ase` cannot carry, for warning the user beforehand.
pub fn compat_warnings(doc: &DocState) -> Vec<String> {
    let mut w = Vec::new();
    if doc.selection.is_some() {
        w.push("The selection isn't stored in Aseprite files.".into());
    }
    let clipped: Vec<&str> = doc.layers.iter().filter(|l| l.props.clipped).map(|l| l.props.name.as_str()).collect();
    if !clipped.is_empty() {
        w.push(format!("Clipping masks aren't supported by Aseprite; {} will be unclipped.", list(&clipped)));
    }
    let masked: Vec<&str> = doc.layers.iter().filter(|l| l.mask.is_some()).map(|l| l.props.name.as_str()).collect();
    if !masked.is_empty() {
        w.push(format!("Layer masks are baked into the pixels; {} will lose the editable mask.", list(&masked)));
    }
    let styled: Vec<&str> =
        doc.layers.iter().filter(|l| !l.props.style.is_off()).map(|l| l.props.name.as_str()).collect();
    if !styled.is_empty() {
        w.push(format!("Layer styles aren't stored; {} will be saved without effects.", list(&styled)));
    }
    let shapes: Vec<&str> = doc.layers.iter().filter(|l| l.is_shape()).map(|l| l.props.name.as_str()).collect();
    if !shapes.is_empty() {
        w.push(format!("Shape layers are saved as pixels; {} will lose the editable points.", list(&shapes)));
    }
    let smarts: Vec<&str> = doc.layers.iter().filter(|l| l.is_smart()).map(|l| l.props.name.as_str()).collect();
    if !smarts.is_empty() {
        w.push(format!(
            "Smart objects are saved as pixels; {} will lose their original pixels and editable transform.",
            list(&smarts)
        ));
    }
    let texts: Vec<&str> = doc.layers.iter().filter(|l| l.is_text()).map(|l| l.props.name.as_str()).collect();
    if !texts.is_empty() {
        w.push(format!("Text layers are saved as pixels; {} will lose the editable text.", list(&texts)));
    }
    let adjustment: Vec<&str> =
        doc.layers.iter().filter(|l| l.is_adjustment()).map(|l| l.props.name.as_str()).collect();
    if !adjustment.is_empty() {
        w.push(format!("Adjustment layers aren't supported by Aseprite; {} will be left out.", list(&adjustment)));
    }
    let alpha_locked: Vec<&str> = doc
        .layers
        .iter()
        .enumerate()
        .filter(|(i, l)| l.props.alpha_locked && !l.is_group() && !(*i == 0 && is_opaque(&l.raster)))
        .map(|(_, l)| l.props.name.as_str())
        .collect();
    if !alpha_locked.is_empty() {
        w.push(format!("Transparency lock isn't stored; {} will lose it.", list(&alpha_locked)));
    }
    let bad_blend: Vec<String> = doc
        .layers
        .iter()
        .filter(|l| blend_to_ase(l.props.blend).is_none())
        .map(|l| format!("\"{}\" ({})", l.props.name, l.props.blend.label()))
        .collect();
    if !bad_blend.is_empty() {
        let refs: Vec<&str> = bad_blend.iter().map(|s| s.as_str()).collect();
        w.push(format!(
            "Aseprite has no equivalent for the blend mode of {}; it will be saved as Normal.",
            refs.join(", ")
        ));
    }
    w
}

fn list(names: &[&str]) -> String {
    names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ")
}

/// Every pixel opaque (what Aseprite requires of a Background layer).
fn is_opaque(r: &Raster) -> bool {
    r.to_rgba().chunks_exact(4).all(|p| p[3] == 255)
}

/// Up to 256 of the document's most used opaque colors, so Aseprite opens
/// the file with a palette that matches the art.
fn palette_for(doc: &DocState) -> Vec<Rgba8> {
    use std::collections::HashMap;
    let flat = crate::composite::flatten(doc);
    let mut counts: HashMap<[u8; 4], u32> = HashMap::new();
    for p in flat.to_rgba().chunks_exact(4) {
        if p[3] > 0 {
            *counts.entry([p[0], p[1], p[2], 255]).or_default() += 1;
        }
    }
    let mut v: Vec<([u8; 4], u32)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(256);
    let mut pal: Vec<Rgba8> = v.into_iter().map(|(c, _)| Rgba8::new(c[0], c[1], c[2], c[3])).collect();
    if pal.is_empty() {
        pal.push(Rgba8::new(0, 0, 0, 255));
        pal.push(Rgba8::new(255, 255, 255, 255));
    }
    pal
}

/// Save a document as a 32-bit Aseprite sprite with all of its frames.
pub fn save(path: &Path, doc: &DocState) -> anyhow::Result<()> {
    if doc.width > 65_535 || doc.height > 65_535 {
        bail!("Aseprite files are limited to 65535x65535 pixels");
    }
    let bytes = encode(doc)?;
    let tmp = path.with_extension("ase.tmp");
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Encode the document as Aseprite bytes (every frame, RGBA).
pub fn encode(doc: &DocState) -> anyhow::Result<Vec<u8>> {
    // Work on a copy whose cels are up to date (shared tiles: cheap).
    let mut doc = doc.clone();
    doc.sync_cels();
    let doc = &doc;
    let nframes = doc.frame_count();
    // Aseprite order: a group chunk comes before its members, bottom to top.
    // qsketch keeps members immediately below the group entry, so rebuild
    // the tree from `parent` links first.
    struct Node {
        qi: usize,
        children: Vec<Node>,
    }
    fn collect(doc: &DocState, parent: Option<LayerId>) -> Vec<Node> {
        doc.layers
            .iter()
            .enumerate()
            // Aseprite has no adjustment layers: they are left out.
            .filter(|(_, l)| l.props.parent == parent && !l.is_adjustment())
            .map(|(qi, l)| Node {
                qi,
                children: if l.is_group() { collect(doc, Some(l.props.id)) } else { Vec::new() },
            })
            .collect()
    }
    fn walk(nodes: Vec<Node>, level: u16, out: &mut Vec<(usize, u16)>) {
        for n in nodes {
            out.push((n.qi, level));
            walk(n.children, level + 1, out);
        }
    }
    let mut order: Vec<(usize, u16)> = Vec::new();
    walk(collect(doc, None), 0, &mut order);
    // Anything orphaned by a bad parent link still gets written, at root.
    for qi in 0..doc.layers.len() {
        if !doc.layers[qi].is_adjustment() && !order.iter().any(|(q, _)| *q == qi) {
            order.push((qi, 0));
        }
    }
    let palette: Vec<Rgba8> =
        if doc.palette.is_empty() { palette_for(doc) } else { doc.palette.colors.iter().copied().take(256).collect() };
    // Tilesets referenced by a written tilemap layer, in order.
    let mut tileset_ids: Vec<usize> = Vec::new();
    for (qi, _) in &order {
        if let Some(tm) = &doc.layers[*qi].props.tilemap {
            if tm.tileset < doc.tilesets.len() && !tileset_ids.contains(&tm.tileset) {
                tileset_ids.push(tm.tileset);
            }
        }
    }

    let mut o = Out(Vec::new());
    // --- header ---
    o.u32(0); // file size, patched
    o.u16(FILE_MAGIC);
    o.u16(nframes as u16);
    o.u16(doc.width as u16);
    o.u16(doc.height as u16);
    o.u16(32);
    o.u32(1); // layer opacity is valid
    o.u16(doc.frame_duration(0).min(u16::MAX as u32) as u16); // deprecated speed
    o.u32(0);
    o.u32(0);
    o.u8(0); // transparent index
    o.zeros(3);
    o.u16(palette.len() as u16);
    let [pw, ph] = if doc.pixel_aspect.contains(&0) { [1, 1] } else { doc.pixel_aspect };
    o.u8(pw); // pixel width
    o.u8(ph); // pixel height
    o.i16(0); // grid x
    o.i16(0); // grid y
    let grid = doc.tilesets.first().map_or((16, 16), |t| (t.tile_w.min(65_535) as u16, t.tile_h.min(65_535) as u16));
    o.u16(grid.0); // grid w
    o.u16(grid.1); // grid h
    o.zeros(84);

    // Owner cel → the frame it was written in (later frames link to it).
    let mut written: Vec<std::collections::HashMap<usize, usize>> = vec![Default::default(); doc.layers.len()];
    for f in 0..nframes {
        let frame_at = o.0.len();
        o.u32(0); // frame bytes, patched
        o.u16(FRAME_MAGIC);
        let old_count_at = o.0.len();
        o.u16(0);
        o.u16(doc.frame_duration(f).min(u16::MAX as u32) as u16);
        o.zeros(2);
        let new_count_at = o.0.len();
        o.u32(0);
        let mut chunks = 0u32;

        if f == 0 {
            // Color profile: sRGB.
            let at = o.chunk(CHUNK_COLOR_PROFILE);
            o.u16(1);
            o.u16(0);
            o.u32(0);
            o.zeros(8);
            o.end_chunk(at);
            chunks += 1;

            // Old palette (for older readers) + new palette.
            let at = o.chunk(CHUNK_OLD_PALETTE);
            o.u16(1);
            o.u8(0);
            o.u8(if palette.len() >= 256 { 0 } else { palette.len() as u8 });
            for c in &palette {
                o.bytes(&[c.r, c.g, c.b]);
            }
            o.end_chunk(at);
            chunks += 1;
            let at = o.chunk(CHUNK_PALETTE);
            o.u32(palette.len() as u32);
            o.u32(0);
            o.u32(palette.len() as u32 - 1);
            o.zeros(8);
            for c in &palette {
                o.u16(0);
                o.bytes(&[c.r, c.g, c.b, c.a]);
            }
            o.end_chunk(at);
            chunks += 1;

            // Tilesets: tiles stacked vertically, compressed.
            for (k, &ti) in tileset_ids.iter().enumerate() {
                let ts = &doc.tilesets[ti];
                let n = ts.tiles.len().max(1) as u32;
                let mut strip = Raster::new(ts.tile_w, ts.tile_h * n);
                for (i, t) in ts.tiles.iter().enumerate() {
                    strip.blit(t, 0, (i as u32 * ts.tile_h) as i32, false);
                }
                let packed = deflate(&strip.to_rgba())?;
                let at = o.chunk(CHUNK_TILESET);
                o.u32(k as u32);
                o.u32(2 | 4); // tiles inside the file; tile 0 is the empty tile
                o.u32(n);
                o.u16(ts.tile_w as u16);
                o.u16(ts.tile_h as u16);
                o.i16(1); // base index
                o.zeros(14);
                o.string(&ts.name);
                o.u32(packed.len() as u32);
                o.bytes(&packed);
                o.end_chunk(at);
                chunks += 1;
            }

            // Layers.
            for (qi, level) in &order {
                let l = &doc.layers[*qi];
                let at = o.chunk(CHUNK_LAYER);
                let mut flags = 0u16;
                if l.props.visible {
                    flags |= LAYER_VISIBLE;
                }
                if !l.props.locked {
                    flags |= LAYER_EDITABLE;
                }
                if *qi == 0 && l.props.alpha_locked && l.owns_pixels() && is_opaque(&l.raster) {
                    flags |= LAYER_BACKGROUND;
                }
                if l.props.continuous {
                    flags |= LAYER_PREFER_LINKED;
                }
                if l.is_group() && !l.props.expanded {
                    flags |= LAYER_COLLAPSED;
                }
                let tilemap = l.props.tilemap.as_ref().and_then(|tm| tileset_ids.iter().position(|&t| t == tm.tileset));
                o.u16(flags);
                o.u16(if l.is_group() {
                    LAYER_KIND_GROUP
                } else if tilemap.is_some() {
                    LAYER_KIND_TILEMAP
                } else {
                    LAYER_KIND_IMAGE
                });
                o.u16(*level);
                o.u16(0);
                o.u16(0);
                o.u16(blend_to_ase(l.props.blend).unwrap_or(0));
                o.u8((l.props.opacity.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                o.zeros(3);
                o.string(&l.props.name);
                if let Some(k) = tilemap {
                    o.u32(k as u32);
                }
                o.end_chunk(at);
                chunks += 1;
                if o.user_data(&l.props.notes, l.props.color) {
                    chunks += 1;
                }
            }
        }

        // Cels: one per layer with any pixels in this frame; frames that
        // share a picture link to the frame that wrote it.
        for (ase_index, (qi, _)) in order.iter().enumerate() {
            let l = &doc.layers[*qi];
            if l.is_group() {
                continue;
            }
            let tilemap = l.props.tilemap.as_ref().filter(|tm| tileset_ids.contains(&tm.tileset));
            if let Some(tm) = tilemap {
                if f == 0 {
                    let mut raw = Vec::with_capacity(tm.cells.len() * 4);
                    for &v in &tm.cells {
                        let mut q = v & !(FLIP_H | FLIP_V) & TILE_ID_MASK;
                        if v & FLIP_H != 0 {
                            q |= TILE_FLIP_X;
                        }
                        if v & FLIP_V != 0 {
                            q |= TILE_FLIP_Y;
                        }
                        raw.extend_from_slice(&q.to_le_bytes());
                    }
                    let packed = deflate(&raw)?;
                    let at = o.chunk(CHUNK_CEL);
                    o.u16(ase_index as u16);
                    o.i16(0);
                    o.i16(0);
                    o.u8(255);
                    o.u16(3);
                    o.i16(0);
                    o.zeros(5);
                    o.u16(tm.cols as u16);
                    o.u16(tm.rows as u16);
                    o.u16(32);
                    o.u32(TILE_ID_MASK);
                    o.u32(TILE_FLIP_X);
                    o.u32(TILE_FLIP_Y);
                    o.u32(TILE_FLIP_D);
                    o.zeros(10);
                    o.bytes(&packed);
                    o.end_chunk(at);
                } else {
                    o.linked_cel(ase_index as u16, 0);
                }
                chunks += 1;
                continue;
            }
            let owner = if l.animated() && l.cels.len() == nframes { l.cel_owner(f) } else { 0 };
            if let Some(&first) = written[*qi].get(&owner) {
                o.linked_cel(ase_index as u16, first as u16);
                chunks += 1;
                continue;
            }
            let Some(picture) = doc.cel_image(*qi, f) else { continue };
            // The mask is baked in; the effects are not (see compat_warnings).
            let shown = match l.active_mask() {
                Some(_) => {
                    let mut tmp = l.clone();
                    tmp.raster = picture.clone();
                    tmp.masked_raster()
                }
                None => picture.clone(),
            };
            // The cel's own rect, off-canvas pixels included (Aseprite keeps
            // cels that reach past the sprite), within the format's i16 range.
            let in_range = |r: IRect| {
                r.x >= i16::MIN as i32
                    && r.y >= i16::MIN as i32
                    && r.right() <= i16::MAX as i32
                    && r.bottom() <= i16::MAX as i32
            };
            let Some(rect) = shown.full_bounds().filter(|r| in_range(*r)).or_else(|| shown.bounds()) else { continue };
            if rect.is_empty() {
                continue;
            }
            written[*qi].insert(owner, f);
            let crop = shown.crop(rect);
            let packed = deflate(&crop.to_rgba())?;
            let (opacity, z) = if l.cels.len() == nframes {
                l.cels.get(owner).map_or((1.0, 0), |c| (c.opacity, c.z_index))
            } else {
                (1.0, 0)
            };
            let at = o.chunk(CHUNK_CEL);
            o.u16(ase_index as u16);
            o.i16(rect.x as i16);
            o.i16(rect.y as i16);
            o.u8((opacity.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            o.u16(2);
            o.i16(z);
            o.zeros(5);
            o.u16(rect.w as u16);
            o.u16(rect.h as u16);
            o.bytes(&packed);
            o.end_chunk(at);
            chunks += 1;
        }

        if f == 0 {
            // Tags, each followed by a user data chunk with its color.
            if !doc.tags.is_empty() {
                let at = o.chunk(CHUNK_TAGS);
                o.u16(doc.tags.len() as u16);
                o.zeros(8);
                for t in &doc.tags {
                    o.u16(t.from.min(nframes - 1) as u16);
                    o.u16(t.to.min(nframes - 1) as u16);
                    o.u8(t.direction.to_ase());
                    o.u16(t.repeat);
                    o.zeros(6);
                    o.bytes(&[t.color.r, t.color.g, t.color.b]);
                    o.u8(0);
                    o.string(&t.name);
                }
                o.end_chunk(at);
                chunks += 1;
                for t in &doc.tags {
                    o.user_data("", Some([t.color.r, t.color.g, t.color.b, 255]));
                    chunks += 1;
                }
            }

            // Slices, each followed by a user data chunk with its color.
            for s in &doc.slices {
                let at = o.chunk(CHUNK_SLICE);
                let flags = u32::from(s.center.is_some()) | (u32::from(s.pivot.is_some()) << 1);
                o.u32(1);
                o.u32(flags);
                o.u32(0);
                o.string(&s.name);
                o.u32(0); // frame
                o.u32(s.rect.x as u32);
                o.u32(s.rect.y as u32);
                o.u32(s.rect.w.max(0) as u32);
                o.u32(s.rect.h.max(0) as u32);
                if let Some(c) = s.center {
                    o.u32(c.x as u32);
                    o.u32(c.y as u32);
                    o.u32(c.w.max(0) as u32);
                    o.u32(c.h.max(0) as u32);
                }
                if let Some((px, py)) = s.pivot {
                    o.u32(px as u32);
                    o.u32(py as u32);
                }
                o.end_chunk(at);
                o.user_data("", Some([s.color.r, s.color.g, s.color.b, s.color.a]));
                chunks += 2;
            }
        }

        let frame_bytes = (o.0.len() - frame_at) as u32;
        o.patch_u32(frame_at, frame_bytes);
        let old = chunks.min(0xFFFF) as u16;
        o.0[old_count_at..old_count_at + 2].copy_from_slice(&old.to_le_bytes());
        o.patch_u32(new_count_at, chunks);
    }
    let total = o.0.len() as u32;
    o.patch_u32(0, total);
    Ok(o.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::NewFrame;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ase-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{name}.aseprite"))
    }

    #[test]
    fn slices_round_trip() {
        let mut doc = DocState::new(32, 32, None);
        let mut sl = crate::slice::Slice::new("button", IRect::new(2, 3, 20, 10));
        sl.center = Some(IRect::new(4, 2, 12, 6));
        sl.pivot = Some((10, 9));
        sl.color = Rgba8::new(255, 0, 128, 255);
        doc.slices = vec![sl.clone(), crate::slice::Slice::new("plain", IRect::new(0, 0, 4, 4))];
        let path = temp("slices");
        save(&path, &doc).unwrap();
        let (back, _) = load_with_warnings(&path).unwrap();
        assert_eq!(back.slices.len(), 2);
        assert_eq!(back.slices[0], sl);
        assert_eq!(back.slices[1].rect, IRect::new(0, 0, 4, 4));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn offcanvas_cels_round_trip() {
        let mut doc = DocState::new(16, 12, None);
        doc.layers[0].raster.set_pixel(1, 1, Rgba8::new(10, 20, 30, 255));
        doc.layers[0].raster.set_pixel(2, 1, Rgba8::new(40, 50, 60, 255));
        // The mask (baked into the cel) hides one of the pixels, off-canvas too.
        let mut m = crate::mask::Mask::full(16, 12);
        m.set(2, 1, 0);
        doc.layers[0].mask = Some(std::sync::Arc::new(m));
        let rest = crate::moving::begin(&doc, &[0]);
        crate::moving::apply(&mut doc, &rest, -5, -4);
        let path = temp("outside");
        save(&path, &doc).unwrap();
        let (back, _) = load_with_warnings(&path).unwrap();
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let r = &back.layers[0].raster;
        assert_eq!(r.get_pixel_any(-4, -3), Rgba8::new(10, 20, 30, 255));
        assert_eq!(r.get_pixel_any(-3, -3), Rgba8::TRANSPARENT);
        assert_eq!(r.shifted_keep(5, 4).get_pixel(1, 1), Rgba8::new(10, 20, 30, 255));
    }

    #[test]
    fn pixel_aspect_round_trips() {
        let mut doc = DocState::new(8, 8, None);
        doc.pixel_aspect = [1, 2];
        let path = temp("aspect");
        save(&path, &doc).unwrap();
        let (back, _) = load_with_warnings(&path).unwrap();
        assert_eq!(back.pixel_aspect, [1, 2]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    fn doc_with_layers() -> DocState {
        let mut doc = DocState::new(16, 12, None);
        doc.layers[0].props.name = "base".into();
        doc.layers[0].raster.set_pixel(3, 4, Rgba8::new(10, 20, 30, 255));
        doc.layers[0].raster.set_pixel(15, 11, Rgba8::new(1, 2, 3, 128));
        let mut top = Layer::new(2, "top", 16, 12);
        top.props.opacity = 0.5;
        top.props.blend = BlendMode::Multiply;
        top.props.visible = false;
        top.props.parent = Some(3);
        top.props.continuous = true;
        top.props.color = Some([9, 8, 7, 255]);
        top.props.notes = "hello".into();
        top.raster.set_pixel(0, 0, Rgba8::new(200, 100, 50, 255));
        doc.layers.push(top);
        let mut g = Layer::new(3, "folder", 16, 12);
        g.props.kind = LayerKind::Group;
        g.props.expanded = false;
        doc.layers.push(g);
        doc.next_layer_id = 4;
        doc
    }

    #[test]
    fn rgba_roundtrip_with_group() {
        let doc = doc_with_layers();
        let bytes = encode(&doc).unwrap();
        let sprite = parse(&bytes).unwrap();
        assert_eq!(sprite.frames.len(), 1);
        // File order: base, folder, top (group before its member).
        let names: Vec<&str> = sprite.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["base", "folder", "top"]);
        assert_eq!(sprite.layers[2].child_level, 1);
        assert_eq!(sprite.layers[2].user.text, "hello");
        let (back, warnings) = sprite.to_doc().unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        // qsketch order: base, top, folder (members below the group entry).
        let names: Vec<&str> = back.layers.iter().map(|l| l.props.name.as_str()).collect();
        assert_eq!(names, ["base", "top", "folder"]);
        assert!(back.layers[2].is_group());
        assert!(!back.layers[2].props.expanded);
        assert_eq!(back.layers[1].props.parent, Some(back.layers[2].props.id));
        assert_eq!(back.layers[1].props.blend, BlendMode::Multiply);
        assert!(!back.layers[1].props.visible);
        assert!(back.layers[1].props.continuous);
        assert_eq!(back.layers[1].props.color, Some([9, 8, 7, 255]));
        assert_eq!(back.layers[1].props.notes, "hello");
        assert!((back.layers[1].props.opacity - 0.5).abs() < 0.01);
        assert_eq!(back.layers[0].raster.get_pixel(3, 4), Rgba8::new(10, 20, 30, 255));
        assert_eq!(back.layers[0].raster.get_pixel(15, 11), Rgba8::new(1, 2, 3, 128));
        assert_eq!(back.layers[1].raster.get_pixel(0, 0), Rgba8::new(200, 100, 50, 255));
        assert_eq!(back.layers[0].raster.get_pixel(0, 0), Rgba8::TRANSPARENT);
        assert!(back.frames.len() == 1 && back.layers[0].cels.is_empty());
    }

    #[test]
    fn animation_round_trips_with_links_tags_and_cel_opacity() {
        let red = Rgba8::new(255, 0, 0, 255);
        let mut doc = DocState::new(8, 8, None);
        doc.layers[0].raster.set_pixel(0, 0, red);
        doc.insert_frame(1, NewFrame::Duplicate(0));
        doc.link_cels(0, &[0, 1]);
        doc.insert_frame(2, NewFrame::Empty);
        doc.layers[0].raster.set_pixel(2, 2, red);
        doc.set_frame(0);
        doc.layers[0].cels[0].opacity = 0.5;
        doc.layers[0].cels[0].z_index = -2;
        doc.frames[1].duration_ms = 250;
        doc.add_tag("walk", 0, 1);
        doc.tags[0].direction = TagDirection::PingPong;
        doc.tags[0].repeat = 3;
        doc.tags[0].color = Rgba8::new(1, 2, 3, 255);
        let bytes = encode(&doc).unwrap();
        let sprite = parse(&bytes).unwrap();
        assert_eq!(sprite.frames.len(), 3);
        assert!(matches!(sprite.frames[1].cels[0].image, CelImage::Linked(0)));
        assert_eq!(sprite.frames[0].cels[0].opacity, 128);
        assert_eq!(sprite.tags[0].user.color, Some([1, 2, 3, 255]));
        let (back, warnings) = sprite.to_doc().unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.frame_count(), 3);
        assert_eq!(back.frames[1].duration_ms, 250);
        assert_eq!(back.layers[0].cels[1].link, Some(0));
        assert!((back.layers[0].cels[0].opacity - 0.5).abs() < 0.01);
        assert_eq!(back.layers[0].cels[0].z_index, -2);
        assert!(back.cel_has_pixels(0, 2));
        assert_eq!(back.cel_image(0, 2).unwrap().get_pixel(2, 2), red);
        assert_eq!(back.tags.len(), 1);
        assert_eq!(back.tags[0].name, "walk");
        assert_eq!(back.tags[0].direction, TagDirection::PingPong);
        assert_eq!(back.tags[0].repeat, 3);
        assert_eq!(back.tags[0].color, Rgba8::new(1, 2, 3, 255));
    }

    #[test]
    fn tilemap_round_trips() {
        let red = Rgba8::new(255, 0, 0, 255);
        let mut doc = DocState::new(8, 8, None);
        for y in 0..4 {
            for x in 0..4 {
                doc.layers[0].raster.set_pixel(x, y, red);
            }
        }
        let (ts, mut tm) = crate::tilemap::from_raster(&doc.layers[0].raster, "t", 4, 4);
        tm.cells[3] = tm.cells[0] | FLIP_H;
        doc.tilesets = vec![ts];
        doc.layers[0].props.kind = LayerKind::Tilemap;
        doc.layers[0].props.tilemap = Some(tm.clone());
        let bytes = encode(&doc).unwrap();
        let sprite = parse(&bytes).unwrap();
        assert_eq!(sprite.tilesets.len(), 1);
        assert_eq!(sprite.layers[0].kind, LAYER_KIND_TILEMAP);
        let (back, warnings) = sprite.to_doc().unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.tilesets.len(), 1);
        assert_eq!(back.tilesets[0].tiles.len(), 2);
        assert_eq!(back.layers[0].props.kind, LayerKind::Tilemap);
        assert_eq!(back.layers[0].props.tilemap, Some(tm));
        assert_eq!(back.layers[0].raster.get_pixel(5, 5), red, "rendered from the cells");
    }

    #[test]
    fn indexed_and_linked_cels() {
        // Hand-built 8-bit, 2-frame sprite: frame 0 has a raw cel, frame 1
        // links to it.
        let mut o = Out(Vec::new());
        o.u32(0);
        o.u16(FILE_MAGIC);
        o.u16(2);
        o.u16(2);
        o.u16(2);
        o.u16(8);
        o.u32(1);
        o.u16(100);
        o.u32(0);
        o.u32(0);
        o.u8(0);
        o.zeros(3);
        o.u16(2);
        o.u8(1);
        o.u8(1);
        o.zeros(8);
        o.zeros(84);
        // frame 0
        let f0 = o.0.len();
        o.u32(0);
        o.u16(FRAME_MAGIC);
        o.u16(3);
        o.u16(50);
        o.zeros(2);
        o.u32(3);
        let at = o.chunk(CHUNK_PALETTE);
        o.u32(2);
        o.u32(0);
        o.u32(1);
        o.zeros(8);
        o.u16(0);
        o.bytes(&[0, 0, 0, 255]);
        o.u16(0);
        o.bytes(&[255, 0, 0, 255]);
        o.end_chunk(at);
        let at = o.chunk(CHUNK_LAYER);
        o.u16(LAYER_VISIBLE | LAYER_EDITABLE);
        o.u16(0);
        o.u16(0);
        o.u16(0);
        o.u16(0);
        o.u16(0);
        o.u8(255);
        o.zeros(3);
        o.string("px");
        o.end_chunk(at);
        let at = o.chunk(CHUNK_CEL);
        o.u16(0);
        o.i16(1);
        o.i16(0);
        o.u8(255);
        o.u16(0);
        o.i16(0);
        o.zeros(5);
        o.u16(1);
        o.u16(2);
        o.bytes(&[1, 0]); // red, then transparent index
        o.end_chunk(at);
        let fb = (o.0.len() - f0) as u32;
        o.patch_u32(f0, fb);
        // frame 1
        let f1 = o.0.len();
        o.u32(0);
        o.u16(FRAME_MAGIC);
        o.u16(1);
        o.u16(50);
        o.zeros(2);
        o.u32(1);
        o.linked_cel(0, 0);
        let fb = (o.0.len() - f1) as u32;
        o.patch_u32(f1, fb);
        let total = o.0.len() as u32;
        o.patch_u32(0, total);

        let sprite = parse(&o.0).unwrap();
        assert_eq!(sprite.frames.len(), 2);
        assert_eq!(sprite.frames[0].duration_ms, 50);
        let (doc, warnings) = sprite.to_doc().unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(doc.frame_count(), 2);
        assert_eq!(doc.layers[0].cels[1].link, Some(0));
        assert_eq!(doc.layers[0].raster.get_pixel(1, 0), Rgba8::new(255, 0, 0, 255));
        assert_eq!(doc.layers[0].raster.get_pixel(1, 1), Rgba8::TRANSPARENT);
        assert_eq!(doc.layers[0].raster.get_pixel(0, 0), Rgba8::TRANSPARENT);
        assert!(doc.palette_lock);
        assert_eq!(doc.palette.colors.len(), 2);
    }
}
