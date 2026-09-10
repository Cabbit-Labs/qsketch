//! Import of third-party brush-tip files: GIMP `.gbr` / `.gih` and Photoshop
//! `.abr` (sampled brushes only). Written from the public format layouts;
//! every file yields one or more alpha masks (`1.0` = paint).

/// One brush tip extracted from a file.
#[derive(Debug, Clone)]
pub struct ImportedTip {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Row-major, `0..=1`, `1.0` = fully painted.
    pub alpha: Vec<f32>,
    /// Dab spacing as a fraction of the tip size, when the file records one.
    pub spacing: Option<f32>,
}

/// Extensions handled by [`import`].
pub const BRUSH_EXTENSIONS: &[&str] = &["gbr", "gih", "abr"];

pub fn is_brush_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| BRUSH_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Parse a brush file by extension.
pub fn import(path: &std::path::Path) -> anyhow::Result<Vec<ImportedTip>> {
    let bytes = std::fs::read(path)?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Brush".into());
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let tips = match ext.as_str() {
        "gbr" => vec![parse_gbr(&bytes, &stem)?.0],
        "gih" => parse_gih(&bytes, &stem)?,
        "abr" => parse_abr(&bytes, &stem)?,
        _ => anyhow::bail!("unsupported brush format .{ext}"),
    };
    if tips.is_empty() {
        anyhow::bail!("no sampled brushes found in file");
    }
    Ok(tips)
}

// ---------------------------------------------------------------------------
// Byte cursor

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }
    fn left(&self) -> usize {
        self.b.len().saturating_sub(self.p)
    }
    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        if self.left() < n {
            anyhow::bail!("unexpected end of file");
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn skip(&mut self, n: usize) -> anyhow::Result<()> {
        self.take(n).map(|_| ())
    }
    fn u8(&mut self) -> anyhow::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> anyhow::Result<u16> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> anyhow::Result<u32> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
}

fn check_dims(w: u32, h: u32) -> anyhow::Result<()> {
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        anyhow::bail!("implausible brush size {w}x{h}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GIMP .gbr

/// Parse one GBR brush at the start of `bytes`; returns the tip and bytes consumed.
fn parse_gbr(bytes: &[u8], fallback_name: &str) -> anyhow::Result<(ImportedTip, usize)> {
    let mut c = Cur::new(bytes);
    let header_size = c.u32()? as usize;
    let version = c.u32()?;
    let width = c.u32()?;
    let height = c.u32()?;
    let bytes_pp = c.u32()?;
    check_dims(width, height)?;
    let (name_off, spacing) = match version {
        1 => (20, None),
        2 | 3 => {
            let magic = c.take(4)?;
            if magic != b"GIMP" {
                anyhow::bail!("bad GBR magic");
            }
            let spacing = c.u32()?;
            (28, Some(spacing as f32 / 100.0))
        }
        v => anyhow::bail!("unsupported GBR version {v}"),
    };
    if header_size < name_off {
        anyhow::bail!("bad GBR header size");
    }
    let name_bytes = c.take(header_size - name_off)?;
    let name = cstr(name_bytes);
    let name = if name.is_empty() { fallback_name.to_string() } else { name };
    let n = (width * height) as usize;
    let alpha = match bytes_pp {
        1 => c.take(n)?.iter().map(|&v| v as f32 / 255.0).collect(),
        4 => c.take(n * 4)?.chunks_exact(4).map(|px| px[3] as f32 / 255.0).collect(),
        b => anyhow::bail!("unsupported GBR depth {b}"),
    };
    Ok((ImportedTip { name, width, height, alpha, spacing }, c.p))
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&x| x == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

// ---------------------------------------------------------------------------
// GIMP .gih (animated brush = text header + N concatenated GBR frames)

fn parse_gih(bytes: &[u8], fallback_name: &str) -> anyhow::Result<Vec<ImportedTip>> {
    let line = |from: usize| -> anyhow::Result<(String, usize)> {
        let rest = &bytes[from..];
        let end = rest.iter().position(|&x| x == b'\n').ok_or_else(|| anyhow::anyhow!("truncated GIH header"))?;
        Ok((String::from_utf8_lossy(&rest[..end]).trim().to_string(), from + end + 1))
    };
    let (name, p) = line(0)?;
    let (params, mut p) = line(p)?;
    let count: usize = params.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
    if count == 0 || count > 4096 {
        anyhow::bail!("bad GIH cell count");
    }
    let base = if name.is_empty() { fallback_name.to_string() } else { name };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let (mut tip, used) = parse_gbr(&bytes[p..], &base)?;
        p += used;
        if count > 1 {
            tip.name = format!("{base} {:02}", i + 1);
        } else {
            tip.name = base.clone();
        }
        out.push(tip);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Photoshop .abr

fn parse_abr(bytes: &[u8], stem: &str) -> anyhow::Result<Vec<ImportedTip>> {
    let mut c = Cur::new(bytes);
    let version = c.u16()?;
    match version {
        1 | 2 => parse_abr_v12(&mut c, version, stem),
        6 | 7 | 10 => {
            let sub = c.u16()?;
            parse_abr_v6(&mut c, sub, stem)
        }
        v => anyhow::bail!("unsupported ABR version {v}"),
    }
}

fn parse_abr_v12(c: &mut Cur, version: u16, stem: &str) -> anyhow::Result<Vec<ImportedTip>> {
    let count = c.u16()? as usize;
    let mut out = Vec::new();
    for i in 0..count {
        let kind = c.u16()?;
        let size = c.u32()? as usize;
        let start = c.p;
        if kind == 2 {
            // sampled brush
            c.skip(4)?; // misc
            let spacing = c.u16()?;
            let name = if version == 2 { read_ucs2(c)? } else { String::new() };
            c.skip(1)?; // antialiasing
            c.skip(8)?; // short bounds
            let top = c.u32()?;
            let left = c.u32()?;
            let bottom = c.u32()?;
            let right = c.u32()?;
            let depth = c.u16()?;
            let compression = c.u8()?;
            let (w, h) = (right.saturating_sub(left), bottom.saturating_sub(top));
            check_dims(w, h)?;
            let end = start + size;
            let data = read_mask(c, w, h, depth, compression, end)?;
            let name = if name.is_empty() { format!("{stem} {:02}", i + 1) } else { name };
            out.push(finish_tip(name, w, h, data, Some(spacing as f32 / 100.0)));
        }
        // computed brushes (kind 1) have no pixels; skip either way
        c.p = start + size;
        if c.p > c.b.len() {
            break;
        }
    }
    Ok(out)
}

fn parse_abr_v6(c: &mut Cur, sub: u16, stem: &str) -> anyhow::Result<Vec<ImportedTip>> {
    let mut out = Vec::new();
    // Sections: "8BIM" + tag + u32 length.
    while c.left() >= 12 {
        let sig = c.take(4)?;
        if sig != b"8BIM" {
            break;
        }
        let tag = c.take(4)?.to_vec();
        let len = c.u32()? as usize;
        let sec_end = (c.p + len).min(c.b.len());
        if &tag == b"samp" {
            while c.p + 4 <= sec_end {
                let bsize = c.u32()? as usize;
                let bstart = c.p;
                let bend = bstart + bsize;
                let padded = bend + ((4 - bsize % 4) % 4);
                let res = (|| -> anyhow::Result<Option<ImportedTip>> {
                    // 37-byte key (u8 length + 36 chars) then a subversion-dependent blob.
                    match sub {
                        1 => c.skip(47)?,
                        2 => c.skip(301)?,
                        s => anyhow::bail!("unsupported ABR 6.{s}"),
                    }
                    let top = c.u32()?;
                    let left = c.u32()?;
                    let bottom = c.u32()?;
                    let right = c.u32()?;
                    let depth = c.u16()?;
                    let compression = c.u8()?;
                    let (w, h) = (right.saturating_sub(left), bottom.saturating_sub(top));
                    check_dims(w, h)?;
                    let data = read_mask(c, w, h, depth, compression, bend)?;
                    Ok(Some(finish_tip(String::new(), w, h, data, None)))
                })();
                match res {
                    Ok(Some(mut t)) => {
                        t.name = format!("{stem} {:02}", out.len() + 1);
                        out.push(t);
                    }
                    Ok(None) => {}
                    Err(e) => log::warn!("abr: skipping brush: {e:#}"),
                }
                c.p = padded;
                if bsize == 0 {
                    break;
                }
            }
        }
        c.p = sec_end;
    }
    Ok(out)
}

fn read_ucs2(c: &mut Cur) -> anyhow::Result<String> {
    let n = c.u32()? as usize;
    let raw = c.take(n * 2)?;
    let units: Vec<u16> = raw.chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]])).collect();
    Ok(String::from_utf16_lossy(&units).trim_end_matches('\0').trim().to_string())
}

/// Read an 8/16-bit mask, raw or PackBits-compressed, bounded by `end`.
fn read_mask(c: &mut Cur, w: u32, h: u32, depth: u16, compression: u8, end: usize) -> anyhow::Result<Vec<u8>> {
    let bpp = match depth {
        8 => 1,
        16 => 2,
        d => anyhow::bail!("unsupported ABR depth {d}"),
    };
    let (w, h) = (w as usize, h as usize);
    let row_bytes = w * bpp;
    let mut out = Vec::with_capacity(w * h);
    match compression {
        0 => {
            let raw = c.take(row_bytes * h)?;
            push_pixels(&mut out, raw, bpp);
        }
        1 => {
            // PackBits: per-row compressed lengths, then the row streams.
            let mut lens = Vec::with_capacity(h);
            for _ in 0..h {
                lens.push(c.u16()? as usize);
            }
            let mut row = Vec::with_capacity(row_bytes);
            for len in lens {
                if c.p + len > end {
                    anyhow::bail!("RLE row overruns brush");
                }
                let src = c.take(len)?;
                row.clear();
                unpack_bits(src, &mut row, row_bytes);
                row.resize(row_bytes, 0);
                push_pixels(&mut out, &row, bpp);
            }
        }
        k => anyhow::bail!("unsupported ABR compression {k}"),
    }
    Ok(out)
}

fn push_pixels(out: &mut Vec<u8>, raw: &[u8], bpp: usize) {
    if bpp == 1 {
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
                let take = cnt.min(cap - dst.len());
                dst.extend(std::iter::repeat_n(v, take));
            }
        }
    }
}

/// Build a tip, auto-detecting polarity: brush masks are transparent at their
/// border, so a bright border means the data is stored inverted.
fn finish_tip(name: String, w: u32, h: u32, data: Vec<u8>, spacing: Option<f32>) -> ImportedTip {
    let (wu, hu) = (w as usize, h as usize);
    let mut sum = 0u64;
    let mut n = 0u64;
    for y in 0..hu {
        for x in 0..wu {
            if y == 0 || x == 0 || y + 1 == hu || x + 1 == wu {
                sum += data[y * wu + x] as u64;
                n += 1;
            }
        }
    }
    let invert = n > 0 && sum / n > 127;
    let alpha = data
        .iter()
        .map(|&v| {
            let v = if invert { 255 - v } else { v };
            v as f32 / 255.0
        })
        .collect();
    ImportedTip { name, width: w, height: h, alpha, spacing }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packbits_roundtrip() {
        // 3 literal bytes, then 4 repeats of 0xAA.
        let src = [2u8, 1, 2, 3, 0xFDu8, 0xAA];
        let mut out = Vec::new();
        unpack_bits(&src, &mut out, 7);
        assert_eq!(out, [1, 2, 3, 0xAA, 0xAA, 0xAA, 0xAA]);
    }

    #[test]
    fn gbr_v2_gray() {
        let mut b = Vec::new();
        b.extend_from_slice(&(28u32 + 4).to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(b"GIMP");
        b.extend_from_slice(&25u32.to_be_bytes());
        b.extend_from_slice(b"hi\0\0");
        b.extend_from_slice(&[0, 255]);
        let (t, used) = parse_gbr(&b, "x").unwrap();
        assert_eq!(used, b.len());
        assert_eq!(t.name, "hi");
        assert_eq!(t.alpha, vec![0.0, 1.0]);
        assert_eq!(t.spacing, Some(0.25));
    }
}

#[cfg(test)]
mod fixture_tests {
    /// `QSKETCH_BRUSH_FIXTURES=/dir cargo test -p qsketch-core fixtures -- --nocapture`
    /// parses every brush file in the directory and dumps PGMs next to it.
    #[test]
    fn fixtures() {
        let Ok(dir) = std::env::var("QSKETCH_BRUSH_FIXTURES") else { return };
        let out = std::path::Path::new(&dir).join("pgm");
        std::fs::create_dir_all(&out).unwrap();
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if !super::is_brush_file(&p) {
                continue;
            }
            match super::import(&p) {
                Ok(tips) => {
                    println!("{}: {} tips", p.display(), tips.len());
                    for t in tips.iter().take(12) {
                        println!("  {:?} {}x{} spacing={:?}", t.name, t.width, t.height, t.spacing);
                        let mut pgm = format!("P5\n{} {}\n255\n", t.width, t.height).into_bytes();
                        pgm.extend(t.alpha.iter().map(|a| (a * 255.0) as u8));
                        std::fs::write(out.join(format!("{}.pgm", t.name.replace('/', "_"))), pgm).unwrap();
                    }
                }
                Err(err) => println!("{}: ERROR {err:#}", p.display()),
            }
        }
    }
}
