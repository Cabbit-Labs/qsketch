//! Photoshop `.psd` open/save, written from Adobe's public file-format
//! specification. Scope: 8/16-bit RGB or grayscale, raw or RLE (PackBits)
//! channel data, layer name/visibility/opacity/blend mode/clipping. Layer
//! groups are flattened into the plain layer list on load; masks, adjustment
//! layers, text and vector data are ignored (their raster contents, if any,
//! are kept).

use std::path::Path;

use anyhow::{bail, Context};

use crate::blend::BlendMode;
use crate::composite::flatten;
use crate::document::DocState;
use crate::geom::IRect;
use crate::layer::{Layer, LayerProps};
use crate::raster::Raster;

pub const EXTENSION: &str = "psd";

pub fn is_psd(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

// ---------------------------------------------------------------------------
// Blend mode keys

const BLEND_KEYS: &[(BlendMode, &[u8; 4])] = &[
    (BlendMode::Normal, b"norm"),
    (BlendMode::Darken, b"dark"),
    (BlendMode::Multiply, b"mul "),
    (BlendMode::ColorBurn, b"idiv"),
    (BlendMode::LinearBurn, b"lbrn"),
    (BlendMode::Lighten, b"lite"),
    (BlendMode::Screen, b"scrn"),
    (BlendMode::ColorDodge, b"div "),
    (BlendMode::LinearDodge, b"lddg"),
    (BlendMode::Overlay, b"over"),
    (BlendMode::SoftLight, b"sLit"),
    (BlendMode::HardLight, b"hLit"),
    (BlendMode::Difference, b"diff"),
    (BlendMode::Exclusion, b"smud"),
    (BlendMode::Subtract, b"fsub"),
    (BlendMode::Divide, b"fdiv"),
    (BlendMode::Hue, b"hue "),
    (BlendMode::Saturation, b"sat "),
    (BlendMode::Color, b"colr"),
    (BlendMode::Luminosity, b"lum "),
];

fn blend_from_key(key: &[u8]) -> BlendMode {
    BLEND_KEYS.iter().find(|(_, k)| &k[..] == key).map(|(m, _)| *m).unwrap_or(BlendMode::Normal)
}

fn blend_key(mode: BlendMode) -> &'static [u8; 4] {
    BLEND_KEYS.iter().find(|(m, _)| *m == mode).map(|(_, k)| *k).unwrap_or(b"norm")
}

// ---------------------------------------------------------------------------
// Reader

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        if self.b.len().saturating_sub(self.p) < n {
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
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn i16(&mut self) -> anyhow::Result<i16> {
        Ok(self.u16()? as i16)
    }
    fn u32(&mut self) -> anyhow::Result<u32> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> anyhow::Result<i32> {
        Ok(self.u32()? as i32)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Gray,
    Rgb,
}

struct Header {
    mode: Mode,
    depth: u16,
    channels: u16,
    width: u32,
    height: u32,
}

struct ChannelInfo {
    id: i16,
    len: usize,
}

struct LayerRecord {
    rect: IRect,
    channels: Vec<ChannelInfo>,
    blend: BlendMode,
    opacity: u8,
    clipped: bool,
    visible: bool,
    name: String,
    /// Section divider (group open/close) — carries no pixels of its own.
    is_section: bool,
}

pub fn load(path: &Path) -> anyhow::Result<DocState> {
    let bytes = std::fs::read(path).with_context(|| format!("opening {}", path.display()))?;
    let mut c = Cur { b: &bytes, p: 0 };
    if c.take(4)? != b"8BPS" {
        bail!("not a Photoshop file");
    }
    let version = c.u16()?;
    if version != 1 {
        bail!("PSB (large document) files are not supported");
    }
    c.take(6)?;
    let channels = c.u16()?;
    let height = c.u32()?;
    let width = c.u32()?;
    let depth = c.u16()?;
    let mode = match c.u16()? {
        1 => Mode::Gray,
        3 => Mode::Rgb,
        0 => bail!("bitmap-mode PSDs are not supported"),
        2 => bail!("indexed-color PSDs are not supported; convert to RGB first"),
        4 => bail!("CMYK PSDs are not supported; convert to RGB first"),
        7 => bail!("multichannel PSDs are not supported"),
        8 => bail!("duotone PSDs are not supported; convert to RGB first"),
        9 => bail!("Lab PSDs are not supported; convert to RGB first"),
        m => bail!("unknown PSD color mode {m}"),
    };
    if !matches!(depth, 8 | 16) {
        bail!("{depth}-bit PSDs are not supported (only 8 and 16)");
    }
    if width == 0 || height == 0 || width > 30_000 || height > 30_000 {
        bail!("implausible image size {width}x{height}");
    }
    let hdr = Header { mode, depth, channels, width, height };

    // Color mode data, image resources.
    let n = c.u32()? as usize;
    c.take(n)?;
    let n = c.u32()? as usize;
    c.take(n)?;

    // Layer and mask information.
    let lm_len = c.u32()? as usize;
    let lm_end = c.p + lm_len;
    let mut layers: Vec<Layer> = Vec::new();
    let mut next_id = 1u64;
    if lm_len > 0 {
        let li_len = c.u32()? as usize;
        if li_len > 0 {
            let li_end = c.p + li_len;
            let count = c.i16()?.unsigned_abs() as usize;
            let mut records = Vec::with_capacity(count);
            for _ in 0..count {
                records.push(read_layer_record(&mut c)?);
            }
            for rec in &records {
                let raster = read_layer_pixels(&mut c, &hdr, rec)?;
                if rec.is_section {
                    continue;
                }
                let props = LayerProps {
                    id: next_id,
                    name: rec.name.clone(),
                    visible: rec.visible,
                    locked: false,
                    alpha_locked: false,
                    opacity: rec.opacity as f32 / 255.0,
                    blend: rec.blend,
                    clipped: rec.clipped,
                };
                next_id += 1;
                layers.push(Layer { props, raster });
            }
            c.p = li_end;
        }
    }
    c.p = lm_end;

    if layers.is_empty() {
        // No layers: the merged image is all there is.
        let raster = read_merged(&mut c, &hdr)?;
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Background").to_string();
        return Ok(DocState::from_raster(name, raster));
    }
    let active = layers.len() - 1;
    Ok(DocState { width, height, layers, active, selection: None, next_layer_id: next_id })
}

fn read_layer_record(c: &mut Cur) -> anyhow::Result<LayerRecord> {
    let top = c.i32()?;
    let left = c.i32()?;
    let bottom = c.i32()?;
    let right = c.i32()?;
    let rect = IRect::from_min_max(left, top, right, bottom);
    let nch = c.u16()? as usize;
    let mut channels = Vec::with_capacity(nch);
    for _ in 0..nch {
        let id = c.i16()?;
        let len = c.u32()? as usize;
        channels.push(ChannelInfo { id, len });
    }
    if c.take(4)? != b"8BIM" {
        bail!("bad layer record signature");
    }
    let blend = blend_from_key(c.take(4)?);
    let opacity = c.u8()?;
    let clipped = c.u8()? != 0;
    let flags = c.u8()?;
    c.u8()?; // filler
    let visible = flags & 0x02 == 0;
    let extra_len = c.u32()? as usize;
    let extra_end = c.p + extra_len;
    // Layer mask data, blending ranges.
    let n = c.u32()? as usize;
    c.take(n)?;
    let n = c.u32()? as usize;
    c.take(n)?;
    // Pascal name padded to a multiple of 4.
    let nlen = c.u8()? as usize;
    let mut name = String::from_utf8_lossy(c.take(nlen)?).into_owned();
    let pad = (4 - (nlen + 1) % 4) % 4;
    c.take(pad)?;
    // Additional layer information.
    let mut is_section = false;
    while c.p + 12 <= extra_end {
        let sig = c.take(4)?;
        if sig != b"8BIM" && sig != b"8B64" {
            break;
        }
        let key = c.take(4)?.to_vec();
        let len = c.u32()? as usize;
        let start = c.p;
        let end = (start + len).min(extra_end);
        match &key[..] {
            b"luni" => {
                if let Ok(n) = c.u32() {
                    let n = (n as usize).min((end - c.p) / 2);
                    let raw = c.take(n * 2)?;
                    let units: Vec<u16> = raw.chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]])).collect();
                    let s = String::from_utf16_lossy(&units);
                    let s = s.trim_end_matches('\0');
                    if !s.is_empty() {
                        name = s.to_string();
                    }
                }
            }
            b"lsct" => {
                let kind = c.u32().unwrap_or(0);
                is_section = matches!(kind, 1..=3);
            }
            _ => {}
        }
        // Lengths are padded to even bytes.
        c.p = start + len + (len & 1);
        if c.p > extra_end {
            break;
        }
    }
    c.p = extra_end;
    Ok(LayerRecord { rect, channels, blend, opacity, clipped, visible, name, is_section })
}

/// Decode one channel of `w*h` samples at the cursor, given its byte length.
fn read_channel(c: &mut Cur, w: usize, h: usize, depth: u16, len: usize) -> anyhow::Result<Vec<u8>> {
    let end = c.p + len;
    if len < 2 {
        c.p = end;
        return Ok(vec![0; w * h]);
    }
    let compression = c.u16()?;
    let bps = (depth / 8) as usize;
    let out = decode_planes(c, w, h, 1, bps, compression, end)?;
    c.p = end;
    Ok(out)
}

/// Decode `planes` consecutive image planes (used for the merged image, where
/// all channels share one compression header and one row-length table).
fn decode_planes(
    c: &mut Cur,
    w: usize,
    h: usize,
    planes: usize,
    bps: usize,
    compression: u16,
    end: usize,
) -> anyhow::Result<Vec<u8>> {
    let row_bytes = w * bps;
    let mut out = Vec::with_capacity(w * h * planes);
    match compression {
        0 => {
            let raw = c.take(row_bytes * h * planes)?;
            push_samples(&mut out, raw, bps);
        }
        1 => {
            let rows = h * planes;
            let mut lens = Vec::with_capacity(rows);
            for _ in 0..rows {
                lens.push(c.u16()? as usize);
            }
            let mut row = Vec::with_capacity(row_bytes);
            for len in lens {
                if c.p + len > end {
                    bail!("RLE row overruns channel data");
                }
                let src = c.take(len)?;
                row.clear();
                unpack_bits(src, &mut row, row_bytes);
                row.resize(row_bytes, 0);
                push_samples(&mut out, &row, bps);
            }
        }
        2 | 3 => bail!("ZIP-compressed PSD channels are not supported"),
        k => bail!("unknown PSD compression {k}"),
    }
    Ok(out)
}

fn push_samples(out: &mut Vec<u8>, raw: &[u8], bps: usize) {
    if bps == 1 {
        out.extend_from_slice(raw);
    } else {
        out.extend(raw.chunks_exact(2).map(|p| p[0]));
    }
}

fn unpack_bits(src: &[u8], dst: &mut Vec<u8>, cap: usize) {
    let mut i = 0;
    while i < src.len() && dst.len() < cap {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let cnt = n as usize + 1;
            let avail = src.len().saturating_sub(i).min(cnt).min(cap - dst.len());
            dst.extend_from_slice(&src[i..i + avail]);
            i += cnt;
        } else if n != -128 {
            let cnt = (1 - n as i32) as usize;
            if i < src.len() {
                let v = src[i];
                i += 1;
                dst.extend(std::iter::repeat_n(v, cnt.min(cap - dst.len())));
            }
        }
    }
}

fn read_layer_pixels(c: &mut Cur, hdr: &Header, rec: &LayerRecord) -> anyhow::Result<Raster> {
    let (w, h) = (rec.rect.w.max(0) as usize, rec.rect.h.max(0) as usize);
    let n = w * h;
    let mut planes: [Option<Vec<u8>>; 4] = [None, None, None, None];
    for ch in &rec.channels {
        let data = read_channel(c, w, h, hdr.depth, ch.len)?;
        let slot = match (hdr.mode, ch.id) {
            (_, -1) => Some(3),
            (Mode::Rgb, 0..=2) => Some(ch.id as usize),
            (Mode::Gray, 0) => Some(0),
            _ => None, // user mask (-2/-3) or extra channels
        };
        if let Some(s) = slot {
            if data.len() == n {
                planes[s] = Some(data);
            }
        }
    }
    if n == 0 {
        return Ok(Raster::new(hdr.width, hdr.height));
    }
    let mut rgba = vec![0u8; n * 4];
    let gray = hdr.mode == Mode::Gray;
    for i in 0..n {
        let r = planes[0].as_ref().map_or(0, |p| p[i]);
        let (g, b) = if gray {
            (r, r)
        } else {
            (planes[1].as_ref().map_or(0, |p| p[i]), planes[2].as_ref().map_or(0, |p| p[i]))
        };
        let a = planes[3].as_ref().map_or(255, |p| p[i]);
        rgba[i * 4..i * 4 + 4].copy_from_slice(&[r, g, b, a]);
    }
    let sub = Raster::from_rgba(w as u32, h as u32, &rgba);
    // Place at the layer's offset on the document canvas (rect may exceed it).
    let mut out = Raster::new(hdr.width, hdr.height);
    blit(&mut out, &sub, rec.rect.x, rec.rect.y);
    Ok(out)
}

fn blit(dst: &mut Raster, src: &Raster, ox: i32, oy: i32) {
    let (dw, dh) = (dst.width() as i32, dst.height() as i32);
    for y in 0..src.height() as i32 {
        let dy = y + oy;
        if dy < 0 || dy >= dh {
            continue;
        }
        for x in 0..src.width() as i32 {
            let dx = x + ox;
            if dx < 0 || dx >= dw {
                continue;
            }
            let px = src.get_pixel(x, y);
            if px.a != 0 {
                dst.set_pixel(dx, dy, px);
            }
        }
    }
}

fn read_merged(c: &mut Cur, hdr: &Header) -> anyhow::Result<Raster> {
    let (w, h) = (hdr.width as usize, hdr.height as usize);
    let compression = c.u16()?;
    let planes = hdr.channels as usize;
    let data = decode_planes(c, w, h, planes, (hdr.depth / 8) as usize, compression, c.b.len())?;
    let n = w * h;
    let plane = |i: usize| -> Option<&[u8]> { data.get(i * n..(i + 1) * n) };
    let mut rgba = vec![0u8; n * 4];
    let (color_planes, gray) = match hdr.mode {
        Mode::Gray => (1, true),
        Mode::Rgb => (3, false),
    };
    let alpha = plane(color_planes);
    for i in 0..n {
        let r = plane(0).map_or(0, |p| p[i]);
        let (g, b) = if gray { (r, r) } else { (plane(1).map_or(0, |p| p[i]), plane(2).map_or(0, |p| p[i])) };
        let a = alpha.map_or(255, |p| p[i]);
        rgba[i * 4..i * 4 + 4].copy_from_slice(&[r, g, b, a]);
    }
    Ok(Raster::from_rgba(hdr.width, hdr.height, &rgba))
}

// ---------------------------------------------------------------------------
// Writer

struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i16(&mut self, v: i16) {
        self.u16(v as u16);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    /// Write a u32 length placeholder; returns its position for `patch_len`.
    fn len_slot(&mut self) -> usize {
        let p = self.0.len();
        self.u32(0);
        p
    }
    fn patch_len(&mut self, slot: usize) {
        let len = (self.0.len() - slot - 4) as u32;
        self.0[slot..slot + 4].copy_from_slice(&len.to_be_bytes());
    }
}

/// PackBits-encode one row.
fn pack_bits(row: &[u8], out: &mut Vec<u8>) {
    let n = row.len();
    let mut i = 0;
    while i < n {
        let mut run = 1;
        while i + run < n && run < 128 && row[i + run] == row[i] {
            run += 1;
        }
        if run >= 2 {
            out.push((1i32 - run as i32) as i8 as u8);
            out.push(row[i]);
            i += run;
            continue;
        }
        let start = i;
        i += 1;
        while i < n && i - start < 128 && !(i + 1 < n && row[i + 1] == row[i]) {
            i += 1;
        }
        out.push((i - start - 1) as u8);
        out.extend_from_slice(&row[start..i]);
    }
}

/// RLE-encode `planes` of `w*h` 8-bit samples: the row-length table followed
/// by the packed rows, as PSD expects.
fn encode_rle(planes: &[&[u8]], w: usize, h: usize) -> Vec<u8> {
    let mut lens: Vec<u16> = Vec::with_capacity(planes.len() * h);
    let mut body = Vec::new();
    let mut row_buf = Vec::with_capacity(w * 2);
    for plane in planes {
        for y in 0..h {
            row_buf.clear();
            pack_bits(&plane[y * w..(y + 1) * w], &mut row_buf);
            lens.push(row_buf.len() as u16);
            body.extend_from_slice(&row_buf);
        }
    }
    let mut out = Vec::with_capacity(lens.len() * 2 + body.len());
    for l in lens {
        out.extend_from_slice(&l.to_be_bytes());
    }
    out.extend_from_slice(&body);
    out
}

/// Split a cropped RGBA buffer into R, G, B, A planes.
fn split_planes(rgba: &[u8]) -> [Vec<u8>; 4] {
    let n = rgba.len() / 4;
    let mut p = [Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n)];
    for px in rgba.chunks_exact(4) {
        for k in 0..4 {
            p[k].push(px[k]);
        }
    }
    p
}

/// Crop `rect` out of a full-canvas RGBA buffer.
fn crop(rgba: &[u8], stride: usize, rect: IRect) -> Vec<u8> {
    let (w, h) = (rect.w as usize, rect.h as usize);
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = (rect.y as usize + y) * stride + rect.x as usize;
        out.extend_from_slice(&rgba[row * 4..(row + w) * 4]);
    }
    out
}

pub fn save(path: &Path, doc: &DocState) -> anyhow::Result<()> {
    if doc.width > 30_000 || doc.height > 30_000 {
        bail!("PSD is limited to 30000x30000 pixels");
    }
    let mut o = Out(Vec::new());
    // Header: RGB, 8-bit, 3 channels in the merged image.
    o.bytes(b"8BPS");
    o.u16(1);
    o.bytes(&[0; 6]);
    o.u16(3);
    o.u32(doc.height);
    o.u32(doc.width);
    o.u16(8);
    o.u16(3);
    o.u32(0); // color mode data
    o.u32(0); // image resources

    // Layer and mask information. Photoshop stores layers bottom-up, which is
    // also qsketch's order.
    let lm = o.len_slot();
    let li = o.len_slot();
    o.i16(doc.layers.len() as i16);
    let mut channel_blobs: Vec<Vec<Vec<u8>>> = Vec::with_capacity(doc.layers.len());
    for layer in &doc.layers {
        let rect = layer.raster.bounds().unwrap_or(IRect::EMPTY);
        let (w, h) = (rect.w as usize, rect.h as usize);
        let blobs: Vec<Vec<u8>> = if w == 0 || h == 0 {
            (0..4).map(|_| vec![0, 0]).collect()
        } else {
            let full = layer.raster.to_rgba();
            let planes = split_planes(&crop(&full, doc.width as usize, rect));
            // Channel order in the record: -1 (alpha), 0, 1, 2.
            [3usize, 0, 1, 2]
                .iter()
                .map(|&k| {
                    let mut b = vec![0, 1];
                    b.extend(encode_rle(&[&planes[k]], w, h));
                    b
                })
                .collect()
        };
        o.i32(rect.y);
        o.i32(rect.x);
        o.i32(rect.y + rect.h);
        o.i32(rect.x + rect.w);
        o.u16(4);
        for (id, blob) in [-1i16, 0, 1, 2].iter().zip(&blobs) {
            o.i16(*id);
            o.u32(blob.len() as u32);
        }
        o.bytes(b"8BIM");
        o.bytes(blend_key(layer.props.blend));
        o.u8((layer.props.opacity.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        o.u8(layer.props.clipped as u8);
        let mut flags = 0u8;
        if !layer.props.visible {
            flags |= 0x02;
        }
        o.u8(flags);
        o.u8(0);
        let extra = o.len_slot();
        o.u32(0); // no layer mask
        o.u32(0); // no blending ranges

        // Pascal name (ASCII-safe, truncated), padded to 4.
        let ascii: String = layer.props.name.chars().map(|ch| if ch.is_ascii() { ch } else { '?' }).take(255).collect();
        let nb = ascii.as_bytes();
        o.u8(nb.len() as u8);
        o.bytes(nb);
        let pad = (4 - (nb.len() + 1) % 4) % 4;
        o.bytes(&[0; 3][..pad]);
        // Unicode name.
        o.bytes(b"8BIM");
        o.bytes(b"luni");
        let ls = o.len_slot();
        let units: Vec<u16> = layer.props.name.encode_utf16().collect();
        o.u32(units.len() as u32);
        for u in units {
            o.u16(u);
        }
        if (o.0.len() - ls - 4) & 1 == 1 {
            o.u8(0);
        }
        o.patch_len(ls);
        o.patch_len(extra);
        channel_blobs.push(blobs);
    }
    for blobs in &channel_blobs {
        for b in blobs {
            o.bytes(b);
        }
    }
    o.patch_len(li);
    o.u32(0); // global layer mask info
    o.patch_len(lm);

    // Merged image: flattened over white, RGB, RLE.
    let flat = flatten(doc).to_rgba();
    let n = (doc.width * doc.height) as usize;
    let mut planes = [vec![0u8; n], vec![0u8; n], vec![0u8; n]];
    for (i, px) in flat.chunks_exact(4).enumerate() {
        let a = px[3] as u32;
        for k in 0..3 {
            planes[k][i] = ((px[k] as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        }
    }
    o.u16(1);
    let refs: Vec<&[u8]> = planes.iter().map(|p| p.as_slice()).collect();
    o.bytes(&encode_rle(&refs, doc.width as usize, doc.height as usize));

    std::fs::write(path, &o.0).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    #[test]
    fn packbits_roundtrip() {
        let rows: Vec<Vec<u8>> = vec![
            vec![],
            vec![7],
            vec![1, 2, 3, 4],
            vec![5; 300],
            vec![1, 1, 2, 2, 2, 3, 4, 4, 4, 4, 5],
            (0..=255).collect(),
            (0..1000).map(|i| (i * 7 % 13 == 0) as u8 * 200).collect(),
        ];
        for row in rows {
            let mut packed = Vec::new();
            pack_bits(&row, &mut packed);
            let mut back = Vec::new();
            unpack_bits(&packed, &mut back, row.len());
            assert_eq!(back, row);
        }
    }

    #[test]
    fn save_load_roundtrip() {
        let mut doc = DocState::new(70, 50, Some(Rgba8::new(255, 255, 255, 255)));
        let id = doc.add_layer("Ünïcode ✓", Some(0));
        let idx = doc.index_of(id).unwrap();
        let l = &mut doc.layers[idx];
        l.raster.fill_rect(IRect::new(10, 5, 40, 30), Rgba8::new(200, 30, 60, 128));
        l.props.blend = BlendMode::Multiply;
        l.props.opacity = 0.5;
        l.props.clipped = true;
        l.props.visible = false;
        let dir = std::env::temp_dir().join(format!("qsketch-psd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.psd");
        save(&p, &doc).unwrap();
        let back = load(&p).unwrap();
        assert_eq!((back.width, back.height), (70, 50));
        assert_eq!(back.layers.len(), 2);
        let l = &back.layers[1];
        assert_eq!(l.props.name, "Ünïcode ✓");
        assert_eq!(l.props.blend, BlendMode::Multiply);
        assert!((l.props.opacity - 0.5).abs() < 0.01);
        assert!(l.props.clipped);
        assert!(!l.props.visible);
        assert_eq!(l.raster.get_pixel(20, 10), Rgba8::new(200, 30, 60, 128));
        assert_eq!(l.raster.get_pixel(5, 5).a, 0);
        assert_eq!(back.layers[0].raster.get_pixel(0, 0), Rgba8::new(255, 255, 255, 255));
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod external_tests {
    /// `QSKETCH_PSD_OUT=/x.psd` writes a sample; `QSKETCH_PSD_IN=/y.psd` dumps a file.
    #[test]
    fn external() {
        use crate::color::Rgba8;
        use crate::geom::IRect;
        if let Ok(p) = std::env::var("QSKETCH_PSD_OUT") {
            let mut doc = crate::document::DocState::new(200, 120, Some(Rgba8::WHITE));
            let id = doc.add_layer("Red box", Some(0));
            let i = doc.index_of(id).unwrap();
            doc.layers[i].raster.fill_rect(IRect::new(20, 10, 100, 60), Rgba8::new(220, 40, 40, 255));
            let id = doc.add_layer("Half blue", Some(1));
            let i = doc.index_of(id).unwrap();
            doc.layers[i].raster.fill_rect(IRect::new(60, 40, 120, 70), Rgba8::new(40, 60, 220, 128));
            doc.layers[i].props.blend = crate::blend::BlendMode::Multiply;
            super::save(std::path::Path::new(&p), &doc).unwrap();
        }
        if let Ok(p) = std::env::var("QSKETCH_PSD_IN") {
            let doc = super::load(std::path::Path::new(&p)).unwrap();
            println!("{}x{} layers={}", doc.width, doc.height, doc.layers.len());
            for l in &doc.layers {
                let b = l.raster.bounds();
                println!(
                    "  {:?} vis={} op={:.2} blend={:?} clip={} bounds={:?} px@center={:?}",
                    l.props.name,
                    l.props.visible,
                    l.props.opacity,
                    l.props.blend,
                    l.props.clipped,
                    b,
                    b.map(|b| l.raster.get_pixel(b.x + b.w / 2, b.y + b.h / 2))
                );
            }
            let flat = crate::composite::flatten(&doc).to_rgba();
            let png = super::super::image_io::encode_png(doc.width, doc.height, &flat).unwrap();
            std::fs::write(format!("{p}.flat.png"), png).unwrap();
        }
    }
}
