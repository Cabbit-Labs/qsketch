//! Aseprite `.ase` / `.aseprite` files.
//!
//! The parser keeps the whole sprite — every frame, its cels, tags and
//! durations — in [`AseSprite`], so the upcoming animation work can consume
//! it as-is. `load` currently builds a document from the first frame.
//!
//! Format reference: <https://github.com/aseprite/aseprite/blob/main/docs/ase-file-specs.md>

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{bail, Context};

use crate::blend::BlendMode;
use crate::color::Rgba8;
use crate::document::DocState;
use crate::geom::IRect;
use crate::layer::{Layer, LayerId, LayerKind, LayerProps};
use crate::raster::Raster;

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

// Layer flags.
const LAYER_VISIBLE: u16 = 1;
const LAYER_EDITABLE: u16 = 2;
const LAYER_BACKGROUND: u16 = 8;
const LAYER_COLLAPSED: u16 = 32;

const LAYER_KIND_IMAGE: u16 = 0;
const LAYER_KIND_GROUP: u16 = 1;
const LAYER_KIND_TILEMAP: u16 = 2;

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
    /// In file order (bottom to top, a group before its children).
    pub layers: Vec<AseLayer>,
    pub frames: Vec<AseFrame>,
    pub tags: Vec<AseTag>,
}

#[derive(Clone, Debug)]
pub struct AseLayer {
    pub name: String,
    pub flags: u16,
    pub kind: u16,
    pub child_level: u16,
    pub blend: u16,
    pub opacity: u8,
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
    pub image: CelImage,
}

#[derive(Clone, Debug)]
pub enum CelImage {
    /// Raw pixels in the sprite's depth, `w * h * bytes_per_pixel`.
    Pixels { w: u32, h: u32, data: Vec<u8> },
    /// Same image as this layer's cel in another frame.
    Linked(usize),
    /// Tilemap cels are not supported.
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
    c.u8()?; // pixel width
    c.u8()?; // pixel height
    c.take(2 + 2 + 2 + 2)?; // grid x, y, w, h
    c.take(84)?;
    if !matches!(depth, 8 | 16 | 32) {
        bail!("unsupported color depth {depth}");
    }
    if width == 0 || height == 0 || width > 65_535 || height > 65_535 {
        bail!("implausible sprite size {width}x{height}");
    }
    let layer_opacity_valid = flags & 1 != 0;

    let mut sprite = AseSprite {
        width,
        height,
        depth,
        transparent_index,
        palette: vec![Rgba8::new(0, 0, 0, 255); 256],
        layers: Vec::new(),
        frames: Vec::with_capacity(frame_count),
        tags: Vec::new(),
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
                    sprite.layers.push(AseLayer {
                        name,
                        flags,
                        kind,
                        child_level,
                        blend,
                        opacity: if layer_opacity_valid { opacity } else { 255 },
                    });
                }
                CHUNK_CEL => {
                    let layer = d.u16()? as usize;
                    let x = d.i16()? as i32;
                    let y = d.i16()? as i32;
                    let opacity = d.u8()?;
                    let cel_type = d.u16()?;
                    d.i16()?; // z-index
                    d.take(5)?;
                    let bpp = (depth / 8) as usize;
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
                            let want = w as usize * h as usize * bpp;
                            let mut data = Vec::with_capacity(want);
                            flate2::read::ZlibDecoder::new(&bytes[d.p..end])
                                .take(want as u64)
                                .read_to_end(&mut data)
                                .context("decompressing cel")?;
                            if data.len() != want {
                                bail!("cel image is {} bytes, expected {want}", data.len());
                            }
                            CelImage::Pixels { w, h, data }
                        }
                        _ => CelImage::Unsupported,
                    };
                    frame.cels.push(AseCel { layer, x, y, opacity, image });
                }
                CHUNK_PALETTE => {
                    let new_size = d.u32()? as usize;
                    let first = d.u32()? as usize;
                    let last = d.u32()? as usize;
                    d.take(8)?;
                    if new_size > sprite.palette.len() {
                        sprite.palette.resize(new_size.min(65_536), Rgba8::new(0, 0, 0, 255));
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
                    let n = d.u16()?;
                    d.take(8)?;
                    for _ in 0..n {
                        let from = d.u16()?;
                        let to = d.u16()?;
                        let direction = d.u8()?;
                        let repeat = d.u16()?;
                        d.take(6)?;
                        d.take(3)?; // deprecated color
                        d.u8()?; // extra byte
                        let name = d.string()?;
                        sprite.tags.push(AseTag { name, from, to, direction, repeat });
                    }
                }
                // Color profile, user data, slices, tilesets, external
                // files: skipped by size.
                _ => {}
            }
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
    /// Straight RGBA for one cel image in this sprite's depth. `background`
    /// keeps the transparent index opaque, as Aseprite does on a Background
    /// layer.
    fn cel_rgba(&self, w: u32, h: u32, data: &[u8], background: bool) -> Vec<u8> {
        let n = (w * h) as usize;
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

    /// The cel a layer shows in `frame`, following links.
    fn cel_for(&self, frame: usize, layer: usize) -> Option<(&AseCel, u32, u32, &[u8])> {
        let mut fi = frame;
        for _ in 0..self.frames.len().max(1) {
            let cel = self.frames.get(fi)?.cels.iter().find(|c| c.layer == layer)?;
            match &cel.image {
                CelImage::Pixels { w, h, data } => return Some((cel, *w, *h, data)),
                CelImage::Linked(f) if *f != fi => fi = *f,
                _ => return None,
            }
        }
        None
    }

    /// Build a document from one frame. Returns the document and any
    /// warnings about what could not be represented.
    pub fn to_doc(&self, frame: usize) -> anyhow::Result<(DocState, Vec<String>)> {
        let mut warnings = Vec::new();
        if self.frames.is_empty() {
            bail!("the file has no frames");
        }
        let frame = frame.min(self.frames.len() - 1);
        if self.frames.len() > 1 {
            warnings.push(format!(
                "This sprite has {} frames; only frame {} was loaded (animation support is coming).",
                self.frames.len(),
                frame + 1
            ));
        }
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
        // Parents are recorded as file indexes here and mapped to layer ids
        // once the output order (and so the ids) is known.
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
        // Layer ids: 1-based sequential in output order; parents are file
        // indexes until now, so map them.
        let mut id_of_file_index = vec![0 as LayerId; self.layers.len()];
        for (n, (fi, _)) in order.iter().enumerate() {
            id_of_file_index[*fi] = n as LayerId + 1;
        }
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
            };
            let raster = match al.kind {
                LAYER_KIND_GROUP => {
                    props.kind = LayerKind::Group;
                    Raster::new(self.width, self.height)
                }
                LAYER_KIND_TILEMAP => {
                    warnings.push(format!(
                        "Layer \"{}\" is a tilemap layer, which isn't supported; it was left empty.",
                        al.name
                    ));
                    Raster::new(self.width, self.height)
                }
                _ => {
                    let background = al.flags & LAYER_BACKGROUND != 0;
                    props.alpha_locked = background;
                    let mut r = Raster::new(self.width, self.height);
                    if let Some((cel, w, h, data)) = self.cel_for(frame, *fi) {
                        let mut rgba = self.cel_rgba(w, h, data, background);
                        if cel.opacity != 255 {
                            for p in rgba.chunks_exact_mut(4) {
                                p[3] = ((p[3] as u32 * cel.opacity as u32 + 127) / 255) as u8;
                            }
                        }
                        let img = Raster::from_rgba(w, h, &rgba);
                        r.blit(&img, cel.x, cel.y, false);
                    } else if self.frames[frame]
                        .cels
                        .iter()
                        .any(|c| c.layer == *fi && matches!(c.image, CelImage::Unsupported))
                    {
                        warnings.push(format!("Layer \"{}\" has an unsupported cel type; it was left empty.", al.name));
                    }
                    r
                }
            };
            layers.push(Layer { props, raster });
        }
        if layers.is_empty() {
            layers.push(Layer::new(1, "Layer 1", self.width, self.height));
        }
        let active = layers.iter().rposition(|l| !l.is_group()).unwrap_or(layers.len() - 1);
        let next_layer_id = layers.len() as LayerId + 1;
        let mut doc =
            DocState { width: self.width, height: self.height, layers, active, selection: None, next_layer_id };
        doc.repair_groups();
        Ok((doc, warnings))
    }
}

/// Open an Aseprite file as a document (first frame), with warnings.
pub fn load_with_warnings(path: &Path) -> anyhow::Result<(DocState, Vec<String>)> {
    let bytes = std::fs::read(path).with_context(|| format!("opening {}", path.display()))?;
    let sprite = parse(&bytes).with_context(|| format!("reading {}", path.display()))?;
    sprite.to_doc(0)
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
    let alpha_locked: Vec<&str> =
        doc.layers.iter().filter(|l| l.props.alpha_locked && !l.is_group()).map(|l| l.props.name.as_str()).collect();
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

/// Save a document as a single-frame 32-bit Aseprite sprite.
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

/// Encode the document as Aseprite bytes (one frame, RGBA).
pub fn encode(doc: &DocState) -> anyhow::Result<Vec<u8>> {
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
            .filter(|(_, l)| l.props.parent == parent)
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
        if !order.iter().any(|(q, _)| *q == qi) {
            order.push((qi, 0));
        }
    }
    let palette = palette_for(doc);

    let mut o = Out(Vec::new());
    // --- header ---
    o.u32(0); // file size, patched
    o.u16(FILE_MAGIC);
    o.u16(1); // frames
    o.u16(doc.width as u16);
    o.u16(doc.height as u16);
    o.u16(32);
    o.u32(1); // layer opacity is valid
    o.u16(100); // deprecated speed
    o.u32(0);
    o.u32(0);
    o.u8(0); // transparent index
    o.zeros(3);
    o.u16(palette.len() as u16);
    o.u8(1); // pixel width
    o.u8(1); // pixel height
    o.i16(0); // grid x
    o.i16(0); // grid y
    o.u16(16); // grid w
    o.u16(16); // grid h
    o.zeros(84);

    // --- frame ---
    let frame_at = o.0.len();
    o.u32(0); // frame bytes, patched
    o.u16(FRAME_MAGIC);
    let old_count_at = o.0.len();
    o.u16(0);
    o.u16(100); // duration ms
    o.zeros(2);
    let new_count_at = o.0.len();
    o.u32(0);
    let mut chunks = 0u32;

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
        if l.is_group() && !l.props.expanded {
            flags |= LAYER_COLLAPSED;
        }
        o.u16(flags);
        o.u16(if l.is_group() { LAYER_KIND_GROUP } else { LAYER_KIND_IMAGE });
        o.u16(*level);
        o.u16(0);
        o.u16(0);
        o.u16(blend_to_ase(l.props.blend).unwrap_or(0));
        o.u8((l.props.opacity.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        o.zeros(3);
        o.string(&l.props.name);
        o.end_chunk(at);
        chunks += 1;
    }

    // Cels: one compressed image per raster layer with any pixels.
    for (ase_index, (qi, _)) in order.iter().enumerate() {
        let l = &doc.layers[*qi];
        if l.is_group() {
            continue;
        }
        let Some(rect) = l.raster.bounds() else { continue };
        if rect.is_empty() {
            continue;
        }
        let rect = IRect::new(rect.x, rect.y, rect.w, rect.h);
        let crop = l.raster.crop(rect);
        let rgba = crop.to_rgba();
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&rgba)?;
        let packed = z.finish()?;
        let at = o.chunk(CHUNK_CEL);
        o.u16(ase_index as u16);
        o.i16(rect.x as i16);
        o.i16(rect.y as i16);
        o.u8(255);
        o.u16(2);
        o.i16(0);
        o.zeros(5);
        o.u16(rect.w as u16);
        o.u16(rect.h as u16);
        o.bytes(&packed);
        o.end_chunk(at);
        chunks += 1;
    }

    let frame_bytes = (o.0.len() - frame_at) as u32;
    o.patch_u32(frame_at, frame_bytes);
    let old = chunks.min(0xFFFF) as u16;
    o.0[old_count_at..old_count_at + 2].copy_from_slice(&old.to_le_bytes());
    o.patch_u32(new_count_at, chunks);
    let total = o.0.len() as u32;
    o.patch_u32(0, total);
    Ok(o.0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (back, warnings) = sprite.to_doc(0).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        // qsketch order: base, top, folder (members below the group entry).
        let names: Vec<&str> = back.layers.iter().map(|l| l.props.name.as_str()).collect();
        assert_eq!(names, ["base", "top", "folder"]);
        assert!(back.layers[2].is_group());
        assert!(!back.layers[2].props.expanded);
        assert_eq!(back.layers[1].props.parent, Some(back.layers[2].props.id));
        assert_eq!(back.layers[1].props.blend, BlendMode::Multiply);
        assert!(!back.layers[1].props.visible);
        assert!((back.layers[1].props.opacity - 0.5).abs() < 0.01);
        assert_eq!(back.layers[0].raster.get_pixel(3, 4), Rgba8::new(10, 20, 30, 255));
        assert_eq!(back.layers[0].raster.get_pixel(15, 11), Rgba8::new(1, 2, 3, 128));
        assert_eq!(back.layers[1].raster.get_pixel(0, 0), Rgba8::new(200, 100, 50, 255));
        assert_eq!(back.layers[0].raster.get_pixel(0, 0), Rgba8::TRANSPARENT);
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
        let at = o.chunk(CHUNK_CEL);
        o.u16(0);
        o.i16(1);
        o.i16(0);
        o.u8(255);
        o.u16(1);
        o.i16(0);
        o.zeros(5);
        o.u16(0);
        o.end_chunk(at);
        let fb = (o.0.len() - f1) as u32;
        o.patch_u32(f1, fb);
        let total = o.0.len() as u32;
        o.patch_u32(0, total);

        let sprite = parse(&o.0).unwrap();
        assert_eq!(sprite.frames.len(), 2);
        assert_eq!(sprite.frames[0].duration_ms, 50);
        let (doc, warnings) = sprite.to_doc(1).unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(doc.layers[0].raster.get_pixel(1, 0), Rgba8::new(255, 0, 0, 255));
        assert_eq!(doc.layers[0].raster.get_pixel(1, 1), Rgba8::TRANSPARENT);
        assert_eq!(doc.layers[0].raster.get_pixel(0, 0), Rgba8::TRANSPARENT);
    }
}
