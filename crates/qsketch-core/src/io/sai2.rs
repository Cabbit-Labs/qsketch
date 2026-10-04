//! PaintTool SAI 2 (`.sai2`), read-only.
//!
//! A `.sai2` file is a 64-byte header (`SAI-CANVAS-TYPE0`, canvas size,
//! chunk count), a table of 16-byte chunk entries (four-letter type, object
//! id, absolute offset) and the chunks themselves: `layr` layer records,
//! `lpix` layer pixels, `thum` a JPEG-like preview, `intg` the full-size
//! merged image, and so on (layout after the unofficial specification at
//! <https://github.com/photopea/SAI2-specification>).
//!
//! Layers open with their pixels, names, opacity, blend mode and
//! visibility, in SAI 2's stacking order. The layer pixel format
//! (`lpix`: 32-pixel strips of 32×32 blocks, see [`decode_layer`]) was
//! worked out for qsketch against real files and their merged images; the
//! bit coder it shares with the merged image (`dpcm`) is a port of
//! libsai's (<https://github.com/Wunkolo/libsai>, MIT License, Copyright
//! (c) 2017-2023 Wunkolo; see `THIRD_PARTY_NOTICES.md`). A file whose
//! layers can't be read opens as its merged image instead.

use std::path::Path;

use anyhow::{anyhow, bail, Context};

use crate::blend::BlendMode;
use crate::document::DocState;
use crate::layer::Layer;
use crate::raster::Raster;

pub const EXTENSION: &str = "sai2";

const MAGIC: &[u8; 16] = b"SAI-CANVAS-TYPE0";
const HEADER_LEN: usize = 64;
const ENTRY_LEN: usize = 16;
/// Edge of a `dpcm` tile.
const TILE: usize = 256;
/// Larger canvases than this are refused rather than allocated.
const MAX_SIDE: u32 = 30_000;

pub fn is_sai2(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

struct Header {
    width: u32,
    height: u32,
    /// Bits 0-2 set: the canvas has an opaque paper color (the merged
    /// image is stored without alpha).
    background_flags: u8,
}

struct Entry {
    tag: [u8; 4],
    id: u32,
    offset: usize,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn parse(data: &[u8]) -> anyhow::Result<(Header, Vec<Entry>)> {
    if data.len() < HEADER_LEN || &data[..16] != MAGIC {
        bail!("not a PaintTool SAI 2 file");
    }
    let header = Header {
        background_flags: data[17],
        width: u32_at(data, 20).unwrap_or(0),
        height: u32_at(data, 24).unwrap_or(0),
    };
    if header.width == 0 || header.height == 0 || header.width > MAX_SIDE || header.height > MAX_SIDE {
        bail!("unsupported canvas size {}x{}", header.width, header.height);
    }
    let count = u32_at(data, 32).unwrap_or(0) as usize;
    let mut entries = Vec::with_capacity(count.min(4096));
    for i in 0..count {
        let at = HEADER_LEN + i * ENTRY_LEN;
        let (Some(tag), Some(id), Some(offset)) = (data.get(at..at + 4), u32_at(data, at + 4), u64_at(data, at + 8))
        else {
            bail!("the chunk table is cut short");
        };
        let offset = usize::try_from(offset).ok().filter(|&o| o <= data.len()).context("chunk outside the file")?;
        entries.push(Entry { tag: tag.try_into().unwrap(), id, offset });
    }
    Ok((header, entries))
}

/// The bytes of `e`: up to the next chunk in the file, or its end.
fn chunk<'a>(data: &'a [u8], entries: &[Entry], e: &Entry) -> &'a [u8] {
    let end = entries.iter().map(|o| o.offset).filter(|&o| o > e.offset).min().unwrap_or(data.len());
    &data[e.offset..end]
}

/// One `layr` record: the layer's kind, where its pixels sit (in 32-pixel
/// blocks), blend mode, opacity, flags and name.
#[derive(Debug, Clone)]
struct LayerRec {
    id: u32,
    kind: [u8; 4],
    /// Left, top, width and height of the pixel area, in 32-pixel blocks.
    blocks: (i32, i32, i32, i32),
    blend: [u8; 4],
    opacity: u32,
    flags: u32,
    name: String,
}

fn parse_layr(b: &[u8]) -> Option<LayerRec> {
    let i32_at = |at: usize| u32_at(b, at).map(|v| v as i32);
    let mut rec = LayerRec {
        id: u32_at(b, 4)?,
        kind: b.get(16..20)?.try_into().ok()?,
        blocks: (i32_at(28)?, i32_at(32)?, i32_at(36)?, i32_at(40)?),
        blend: b.get(44..48)?.try_into().ok()?,
        opacity: u32_at(b, 48)?,
        flags: u32_at(b, 52)?,
        name: String::new(),
    };
    // Then (key, length, value) parameters up to a zero key.
    let mut at = 56;
    while let (Some(key), Some(len)) = (b.get(at..at + 4), u32_at(b, at + 4)) {
        if key == [0; 4] {
            break;
        }
        let len = len as usize;
        let Some(value) = b.get(at + 8..at + 8 + len) else { break };
        if key == b"name" {
            // UTF-16LE with a leading character count.
            let n = u16_at(value, 0).unwrap_or(0) as usize;
            let units: Vec<u16> = (0..n).filter_map(|i| u16_at(value, 2 + i * 2)).collect();
            rec.name = String::from_utf16_lossy(&units).trim_end_matches('\0').to_string();
        }
        at += 8 + len;
    }
    Some(rec)
}

/// Full-strength value of a layer pixel channel (layer pixels keep 16
/// bits; 0x4000 is fully opaque / full intensity).
const ONE: i32 = 0x4000;
/// `layr` flag bit: the layer is shown.
const VISIBLE: u32 = 0x1_0000;
/// Edge of a layer pixel block.
const BLOCK: usize = 32;

/// Decode a layer's `lpix` pixels onto a `width`×`height` raster. The
/// layer's area is `blocks` (in 32-pixel units, possibly reaching past the
/// canvas: that part is kept outside it). The chunk is `dpcm`, one byte
/// length per 32-pixel strip, then the strips. A strip is a run of records,
/// each a 16-bit tag (low byte 0xff; bits 8-11 the block's canvas column
/// mod 16, as a check; bits 12-15 the kind) and a 16-bit value: kind 0
/// skips value+1 empty blocks, kind 0xa is a block of `value` bytes, kind
/// 5 is a block of one color (four 16-bit values instead of a length),
/// kind 0xf ends the strip. A block holds 32×32 pixels as four planes (B, G, R,
/// A) of [`unpack_rle16`] values, each the difference from the clamped
/// gradient prediction left + up − up-left, premultiplied, 0x4000 = 1.
fn decode_layer(blob: &[u8], blocks: (i32, i32, i32, i32), width: u32, height: u32) -> anyhow::Result<Raster> {
    let mut out = Raster::new(width, height);
    if blob.len() <= 4 {
        return Ok(out);
    }
    if blob.get(..4) != Some(b"dpcm") {
        bail!("unknown layer encoding");
    }
    let (left, top, bw, bh) = blocks;
    if bw <= 0 || bh <= 0 {
        return Ok(out);
    }
    let mut at = 4 + bh as usize * 4;
    let mut deltas = vec![0i16; BLOCK * BLOCK * 4];
    let mut px = vec![[0i32; 4]; BLOCK * BLOCK];
    let mut row = [0u8; BLOCK * 4];
    for sy in 0..bh as usize {
        let len = u32_at(blob, 4 + sy * 4).context("strip table cut short")? as usize;
        let strip = blob.get(at..at + len).context("strip data cut short")?;
        at += len;
        let mut p = 0;
        let mut x = 0usize;
        while p + 2 <= strip.len() {
            let tag = u16_at(strip, p).unwrap_or(0);
            p += 2;
            // The check nibble is the block's column on the canvas, mod 16.
            if tag & 0xff != 0xff || (tag >> 8) as i32 & 15 != (left + x as i32) & 15 {
                bail!("strip {sy}: unexpected record {tag:#06x}");
            }
            match tag >> 12 {
                0x0 => {
                    x += u16_at(strip, p).context("record cut short")? as usize + 1;
                    p += 2;
                }
                0xa => {
                    let n = u16_at(strip, p).context("record cut short")? as usize;
                    p += 2;
                    let data = strip.get(p..p + n).context("block cut short")?;
                    p += n;
                    unpack_rle16(data, &mut deltas, BLOCK * BLOCK, 4)
                        .with_context(|| format!("strip {sy} block {x}"))?;
                    predict_block(&deltas, &mut px);
                    let (x0, y0) = ((left + x as i32) * BLOCK as i32, (top + sy as i32) * BLOCK as i32);
                    for r in 0..BLOCK {
                        for c in 0..BLOCK {
                            row[c * 4..c * 4 + 4].copy_from_slice(&to_rgba8(px[r * BLOCK + c]));
                        }
                        out.write_row_any(x0, y0 + r as i32, &row);
                    }
                    x += 1;
                }
                0x5 => {
                    // One color over the whole block.
                    let v = |i: usize| u16_at(strip, p + i * 2).map(|v| v as i32).context("record cut short");
                    let px = [v(0)?, v(1)?, v(2)?, v(3)?];
                    p += 8;
                    let px = to_rgba8(px);
                    for chunk in row.chunks_exact_mut(4) {
                        chunk.copy_from_slice(&px);
                    }
                    let (x0, y0) = ((left + x as i32) * BLOCK as i32, (top + sy as i32) * BLOCK as i32);
                    for r in 0..BLOCK {
                        out.write_row_any(x0, y0 + r as i32, &row);
                    }
                    x += 1;
                }
                0xf => break,
                k => bail!("strip {sy}: unknown record kind {k:#x}"),
            }
        }
    }
    out.prune_empty_tiles();
    Ok(out)
}

/// Undo the gradient prediction over one 32×32 block (four channels).
fn predict_block(deltas: &[i16], px: &mut [[i32; 4]]) {
    for r in 0..BLOCK {
        for c in 0..BLOCK {
            let i = r * BLOCK + c;
            for ch in 0..4 {
                let l = if c > 0 { px[i - 1][ch] } else { 0 };
                let u = if r > 0 { px[i - BLOCK][ch] } else { 0 };
                let ul = if r > 0 && c > 0 { px[i - BLOCK - 1][ch] } else { 0 };
                let pred = (l + u - ul).clamp(0, ONE);
                px[i][ch] = (pred + deltas[i * 4 + ch] as i32) & 0xffff;
            }
        }
    }
}

/// A premultiplied 16-bit BGRA layer pixel as straight RGBA8.
fn to_rgba8([b, g, r, a]: [i32; 4]) -> [u8; 4] {
    let a8 = to8(a);
    if a8 == 0 {
        return [0; 4];
    }
    let a = a.clamp(1, ONE) as i64;
    let un = |v: i32| ((v.clamp(0, a as i32) as i64 * 255 + a / 2) / a) as u8;
    [un(r), un(g), un(b), a8]
}

/// 16-bit layer channel (0x4000 = full) to 8 bits.
fn to8(v: i32) -> u8 {
    ((v.clamp(0, ONE) * 255 + ONE / 2) / ONE) as u8
}

fn blend_mode(tag: &[u8; 4]) -> Option<BlendMode> {
    Some(match tag {
        b"norm" => BlendMode::Normal,
        b"mul " | b"mult" | b"mul\0" => BlendMode::Multiply,
        b"scrn" => BlendMode::Screen,
        b"over" => BlendMode::Overlay,
        b"add " | b"add\0" | b"lddg" => BlendMode::LinearDodge,
        b"ddge" => BlendMode::ColorDodge,
        b"burn" => BlendMode::ColorBurn,
        b"lbrn" => BlendMode::LinearBurn,
        b"hard" | b"hrdl" => BlendMode::HardLight,
        b"soft" | b"sftl" => BlendMode::SoftLight,
        b"dark" => BlendMode::Darken,
        b"lite" | b"lght" => BlendMode::Lighten,
        b"diff" => BlendMode::Difference,
        b"excl" => BlendMode::Exclusion,
        b"sub " | b"sub\0" => BlendMode::Subtract,
        b"div " | b"div\0" => BlendMode::Divide,
        b"hue " | b"hue\0" => BlendMode::Hue,
        b"satu" | b"sat " => BlendMode::Saturation,
        b"colr" | b"colo" => BlendMode::Color,
        b"lumi" => BlendMode::Luminosity,
        _ => return None,
    })
}

/// Open a `.sai2` with its layers. When the layers can't be read, falls
/// back to the merged image, with a note saying so.
pub fn load_with_warnings(path: &Path) -> anyhow::Result<(DocState, Vec<String>)> {
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let (header, entries) = parse(&data)?;
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("SAI 2 image").to_string();
    let mut warnings = Vec::new();
    match load_layers(&data, &header, &entries, &mut warnings) {
        Ok(Some(doc)) => return Ok((doc, warnings)),
        Ok(None) => {}
        Err(e) => warnings.push(format!("qsketch couldn't read this file's SAI 2 layers ({e:#}).")),
    }
    let intg = entries
        .iter()
        .find(|e| &e.tag == b"intg")
        .ok_or_else(|| anyhow!("this SAI 2 file has neither layers qsketch can read nor a full-size image"))?;
    let raster = decode_dpcm(&header, chunk(&data, &entries, intg)).context("decoding the SAI 2 image")?;
    warnings.push("The file was opened as its merged image, in one layer.".into());
    Ok((DocState::from_raster(name, raster), warnings))
}

/// The file's merged image: the canvas as SAI 2 last showed it.
pub fn merged_image(path: &Path) -> anyhow::Result<Raster> {
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let (header, entries) = parse(&data)?;
    let intg = entries.iter().find(|e| &e.tag == b"intg").context("no merged image stored")?;
    decode_dpcm(&header, chunk(&data, &entries, intg))
}

/// The layers (the chunk table lists them top to bottom); `None` when the
/// file has none. A canvas with opaque paper gets a white `Paper` layer at
/// the bottom, so it looks the way it did in SAI 2.
fn load_layers(
    data: &[u8],
    header: &Header,
    entries: &[Entry],
    warnings: &mut Vec<String>,
) -> anyhow::Result<Option<DocState>> {
    use rayon::prelude::*;
    let recs: Vec<LayerRec> =
        entries.iter().filter(|e| &e.tag == b"layr").filter_map(|e| parse_layr(chunk(data, entries, e))).collect();
    if recs.is_empty() {
        return Ok(None);
    }
    let pixels = |id: u32| entries.iter().find(|e| &e.tag == b"lpix" && e.id == id).map(|e| chunk(data, entries, e));
    let (w, h) = (header.width, header.height);
    let decoded: Vec<anyhow::Result<Option<Layer>>> = recs
        .par_iter()
        .map(|rec| {
            if &rec.kind == b"fold" {
                return Ok(None);
            }
            let raster = match pixels(rec.id) {
                Some(blob) => {
                    decode_layer(blob, rec.blocks, w, h).with_context(|| format!("layer \"{}\"", rec.name))?
                }
                None => Raster::new(w, h),
            };
            let mut l = Layer::new(0, rec.name.clone(), w, h).with_raster(raster);
            if l.props.name.is_empty() {
                l.props.name = "Layer".into();
            }
            l.props.visible = rec.flags & VISIBLE != 0;
            l.props.opacity = rec.opacity.min(100) as f32 / 100.0;
            l.props.blend = blend_mode(&rec.blend).unwrap_or(BlendMode::Normal);
            Ok(Some(l))
        })
        .collect();
    let mut layers = Vec::with_capacity(decoded.len() + 1);
    if header.background_flags & 7 != 0 {
        let mut paper = Layer::new(0, "Paper", w, h).with_raster(Raster::new_filled(w, h, crate::color::Rgba8::WHITE));
        paper.props.locked = true;
        layers.push(paper);
    }
    let mut read = 0;
    for l in decoded.into_iter().rev() {
        if let Some(l) = l? {
            layers.push(l);
            read += 1;
        }
    }
    let folders = recs.iter().filter(|r| &r.kind == b"fold").count();
    if folders > 0 {
        warnings.push(format!("{folders} SAI 2 folder(s) were left out; their layers are listed on their own."));
    }
    let odd_kinds = recs.iter().filter(|r| !matches!(&r.kind, b"norm" | b"fold")).count();
    if odd_kinds > 0 {
        warnings.push(format!("{odd_kinds} text, shape or linework layer(s) were opened as plain pixels."));
    }
    let odd_blends = recs.iter().filter(|r| blend_mode(&r.blend).is_none()).count();
    if odd_blends > 0 {
        warnings
            .push(format!("{odd_blends} layer(s) use a SAI 2 blend mode qsketch doesn't have; they open as Normal."));
    }
    if read == 0 {
        return Ok(None);
    }
    let mut doc = DocState::from_raster("Layer", Raster::new(w, h));
    for (i, l) in layers.iter_mut().enumerate() {
        l.props.id = i as u64 + 1;
    }
    doc.next_layer_id = layers.len() as u64 + 1;
    doc.active = layers.len() - 1;
    doc.layers = layers;
    Ok(Some(doc))
}

/// Decode the `dpcm` merged image: 256×256 tiles, each a run of rows that
/// are bit-packed deltas from the row above (see [`unpack_rle16`] and
/// [`delta_row`]), stored as BGR(A).
fn decode_dpcm(header: &Header, blob: &[u8]) -> anyhow::Result<Raster> {
    if blob.get(..4) != Some(b"dpcm") {
        bail!("unknown image encoding");
    }
    let (w, h) = (header.width as usize, header.height as usize);
    // An opaque paper stores 3 channels; a transparent canvas 4.
    let channels = if header.background_flags & 7 == 0 { 4 } else { 3 };
    let (tiles_x, tiles_y) = (w.div_ceil(TILE), h.div_ceil(TILE));
    let mut at = 4;
    let mut sizes = Vec::with_capacity(tiles_x * tiles_y);
    for _ in 0..tiles_x * tiles_y {
        sizes.push(u32_at(blob, at).context("tile table cut short")? as usize);
        at += 4;
    }
    let mut bgra = vec![0u8; w * h * 4];
    let mut deltas = vec![0i16; TILE * 4];
    let mut prev = vec![[0u8; 4]; TILE];
    let mut row = vec![[0u8; 4]; TILE];
    for ty in 0..tiles_y {
        let (y0, sy) = (ty * TILE, TILE.min(h - ty * TILE));
        for tx in 0..tiles_x {
            let size = sizes[ty * tiles_x + tx];
            let tile = blob.get(at..at + size).context("tile data cut short")?;
            at += size;
            let (x0, sx) = (tx * TILE, TILE.min(w - tx * TILE));
            // A 2-byte tag (high byte: the tile's column), then the rows.
            let mut pos = 2;
            prev[..sx].fill([0; 4]);
            for r in 0..sy {
                pos += unpack_rle16(&tile[pos.min(tile.len())..], &mut deltas[..sx * 4], sx, channels)
                    .with_context(|| format!("tile {tx},{ty} row {r}"))?;
                delta_row(&prev[..sx], &deltas[..sx * 4], &mut row[..sx]);
                let o = ((y0 + r) * w + x0) * 4;
                for (dst, px) in bgra[o..o + sx * 4].chunks_exact_mut(4).zip(&row[..sx]) {
                    dst.copy_from_slice(px);
                }
                prev[..sx].copy_from_slice(&row[..sx]);
            }
        }
        // Each row of tiles ends with another 2-byte tag.
        at += 2;
    }
    // BGRA → RGBA; without an alpha channel the canvas is opaque.
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
        if channels == 3 {
            px[3] = 255;
        }
    }
    Ok(Raster::from_rgba(header.width, header.height, &bgra))
}

/// Unpack one row of `count` pixels: `channels` planes of bit-packed signed
/// values written interleaved into `out` (4 per pixel; missing channels are
/// 0). The stream is read as little-endian 32-bit words, least significant
/// bit first. Each value starts with an opcode: `z` zero bits, a one, and a
/// bit `b` give `op = 2z + b`; 0 is a zero, 1..=14 is an `op`-bit magnitude
/// plus a sign bit, 15 is a run of 8..=135 zeros (7-bit count). Returns the
/// bytes consumed, not counting whole bytes of bits left over for the next
/// row.
fn unpack_rle16(src: &[u8], out: &mut [i16], count: usize, channels: usize) -> anyhow::Result<usize> {
    out.fill(0);
    let mut p = 0usize;
    let mut bits: u64 = 0;
    let mut avail: u32 = 0;
    for ch in 0..channels {
        let mut n = 0usize;
        while n < count {
            while avail < 32 && p < src.len() {
                let (word, len) = match src.len() - p {
                    4.. => (u32::from_le_bytes(src[p..p + 4].try_into().unwrap()) as u64, 4),
                    2..=3 => (u16::from_le_bytes(src[p..p + 2].try_into().unwrap()) as u64, 2),
                    _ => (src[p] as u64, 1),
                };
                bits |= word << avail;
                avail += len as u32 * 8;
                p += len;
            }
            if bits == 0 {
                bail!("ran out of data");
            }
            let zeros = bits.trailing_zeros();
            let rest = bits >> (zeros + 1);
            let op = (2 * zeros) | (rest & 1) as u32;
            bits = rest >> 1;
            avail = avail.saturating_sub(2 + zeros);
            match op {
                0 => {
                    n += 1;
                }
                1..=14 => {
                    let negative = (bits >> op) & 1 == 1;
                    let magnitude = ((1u64 << op) | (bits & ((1 << op) - 1))) - 1;
                    let v = magnitude as i16;
                    bits >>= op + 1;
                    avail = avail.saturating_sub(op + 1);
                    out[n * 4 + ch] = if negative { v.wrapping_neg() } else { v };
                    n += 1;
                }
                15 => {
                    let run = (bits & 0x7f) as usize + 8;
                    bits >>= 7;
                    avail = avail.saturating_sub(7);
                    n += run; // `out` is already zero
                }
                _ => bail!("invalid opcode"),
            }
        }
    }
    Ok(p - (avail / 8) as usize)
}

/// Undo the row filter: each 16-bit channel sum carries the previous
/// pixel's value plus the change from the row above (the PNG "Up"-like
/// filter SAI uses), saturated the way SAI's SIMD code does, plus the
/// stored delta. The result is clamped to 8 bits.
fn delta_row(prev: &[[u8; 4]], deltas: &[i16], out: &mut [[u8; 4]]) {
    let mut sum = [0u16; 4];
    let mut above_left = [0u16; 4];
    for (i, (above, dst)) in prev.iter().zip(out.iter_mut()).enumerate() {
        for c in 0..4 {
            let a = above[c] as u16;
            let v = sum[c].wrapping_add(a).saturating_sub(above_left[c]);
            let v = v.saturating_add(0xff00).saturating_sub(0xff00);
            sum[c] = v.wrapping_add(deltas[i * 4 + c] as u16);
            dst[c] = sum[c].min(255) as u8;
            above_left[c] = a;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inverse of [`unpack_rle16`] for one channel, for building test
    /// files: LSB-first bits, flushed as bytes.
    struct Bits {
        out: Vec<u8>,
        acc: u64,
        n: u32,
    }

    impl Bits {
        fn put(&mut self, v: u64, len: u32) {
            self.acc |= v << self.n;
            self.n += len;
            while self.n >= 8 {
                self.out.push(self.acc as u8);
                self.acc >>= 8;
                self.n -= 8;
            }
        }
        fn value(&mut self, v: i16) {
            if v == 0 {
                self.put(0b01, 2); // op 0: no zeros, the 1, then bit 0
                return;
            }
            let x = v.unsigned_abs() as u64 + 1;
            let op = 63 - x.leading_zeros();
            let (z, b) = (op / 2, op & 1);
            self.put(0, z);
            self.put(1, 1);
            self.put(b as u64, 1);
            self.put(x - (1 << op), op);
            self.put((v < 0) as u64, 1);
        }
        fn finish(mut self) -> Vec<u8> {
            if self.n > 0 {
                self.out.push(self.acc as u8);
            }
            self.out
        }
    }

    #[test]
    fn value_codes_round_trip() {
        let vals: Vec<i16> = vec![5, -3, 200, 255, -255, 1, 7, 1000];
        let mut b = Bits { out: Vec::new(), acc: 0, n: 0 };
        for &v in &vals {
            b.value(v);
        }
        let bytes = b.finish();
        let mut out = vec![0i16; vals.len() * 4];
        unpack_rle16(&bytes, &mut out, vals.len(), 1).unwrap();
        let got: Vec<i16> = out.chunks(4).map(|p| p[0]).collect();
        assert_eq!(got, vals);
    }

    #[test]
    fn opens_a_tiny_canvas() {
        // A 2×1 transparent canvas: pixels (B,G,R,A) = (10,20,30,255) and
        // (12,20,30,128), stored as deltas from the left neighbour.
        let mut b = Bits { out: Vec::new(), acc: 0, n: 0 };
        for (first, second) in [(10, 2), (20, 0), (30, 0), (255, -127)] {
            b.value(first);
            b.value(second);
        }
        let mut tile = vec![0u8, 0]; // tag: column 0
        tile.extend(b.finish());
        let mut blob = b"dpcm".to_vec();
        blob.extend((tile.len() as u32).to_le_bytes());
        blob.extend(&tile);
        blob.extend([0, 0]);
        let header = Header { width: 2, height: 1, background_flags: 0 };
        let r = decode_dpcm(&header, &blob).unwrap();
        assert_eq!(r.get_pixel(0, 0), crate::color::Rgba8::new(30, 20, 10, 255));
        assert_eq!(r.get_pixel(1, 0), crate::color::Rgba8::new(30, 20, 12, 128));
    }

    #[test]
    fn rejects_other_files() {
        assert!(parse(b"not a sai2 file at all, just some bytes that are long enough to pass..").is_err());
    }

    /// A strip record tag: kind, the block's canvas column (mod 16).
    fn tag(kind: u16, col: i32) -> [u8; 2] {
        ((kind << 12) | (((col & 15) as u16) << 8) | 0xff).to_le_bytes()
    }

    #[test]
    fn decodes_layer_blocks() {
        // A layer whose area starts one block in: skip a block, one solid
        // red block, one half-transparent blue block from deltas, end.
        let left = 1;
        let mut strip = Vec::new();
        strip.extend(tag(0x0, left));
        strip.extend(0u16.to_le_bytes()); // skip 1 block
        strip.extend(tag(0x5, left + 1));
        for v in [0u16, 0, 0x4000, 0x4000] {
            strip.extend(v.to_le_bytes()); // B, G, R, A (premultiplied)
        }
        // Planes B, G, R, A of 1024 deltas; only the first pixel has any,
        // and the gradient prediction carries it over the whole block.
        let mut b = Bits { out: Vec::new(), acc: 0, n: 0 };
        for first in [0x2000i16, 0, 0, 0x2000] {
            b.value(first);
            for _ in 1..1024 {
                b.value(0);
            }
        }
        let block = b.finish();
        strip.extend(tag(0xa, left + 2));
        strip.extend((block.len() as u16).to_le_bytes());
        strip.extend(&block);
        strip.extend(tag(0xf, left + 3));
        let mut blob = b"dpcm".to_vec();
        blob.extend((strip.len() as u32).to_le_bytes());
        blob.extend(&strip);
        let r = decode_layer(&blob, (left, 0, 4, 1), 200, 40).unwrap();
        use crate::color::Rgba8;
        assert_eq!(r.get_pixel(40, 0), Rgba8::TRANSPARENT, "skipped block");
        assert_eq!(r.get_pixel(64, 31), Rgba8::new(255, 0, 0, 255));
        assert_eq!(r.get_pixel(96 + 31, 0), Rgba8::new(0, 0, 255, 128));
        assert_eq!(r.get_pixel(96 + 17, 30), Rgba8::new(0, 0, 255, 128));
        assert_eq!(r.get_pixel(128, 0), Rgba8::TRANSPARENT);
        // One block row up, the strip lies above the canvas: kept off it.
        let r = decode_layer(&blob, (left, -1, 4, 1), 200, 40).unwrap();
        assert_eq!(r.get_pixel(64, 0), Rgba8::TRANSPARENT);
        assert_eq!(r.get_pixel_any(64, -5), Rgba8::new(255, 0, 0, 255));
        // A wrong column check is an error, not garbage.
        assert!(decode_layer(&blob, (left + 1, 0, 4, 1), 200, 40).is_err());
    }
}
