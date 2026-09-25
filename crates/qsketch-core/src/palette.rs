//! Color palettes: a named, ordered list of colors with nearest-color lookup,
//! ramps, median-cut quantization, a few classic presets and the common
//! interchange formats (`.gpl`, `.hex`, `.pal`, `.act`, `.aco`, palette
//! images and Aseprite files).

use std::path::Path;

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};

use crate::color::Rgba8;

/// A document's palette. Empty = "no palette" (free RGBA editing).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Palette {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub colors: Vec<Rgba8>,
}

/// Hard cap: every format here tops out at 256, and the app's shading and
/// indexed workflows assume a `u8` index fits.
pub const MAX_COLORS: usize = 256;

impl Palette {
    pub fn new(name: impl Into<String>, colors: Vec<Rgba8>) -> Self {
        let mut colors = colors;
        colors.truncate(MAX_COLORS);
        Self { name: name.into(), colors }
    }

    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    pub fn len(&self) -> usize {
        self.colors.len()
    }

    /// Exact match (RGB, ignoring alpha unless both are fully transparent).
    pub fn index_of(&self, c: Rgba8) -> Option<usize> {
        self.colors.iter().position(|p| same_color(*p, c))
    }

    /// The perceptually closest slot to `c`. Transparent input maps to a
    /// transparent slot when there is one, otherwise to nothing.
    pub fn nearest(&self, c: Rgba8) -> Option<usize> {
        if self.colors.is_empty() {
            return None;
        }
        if c.a == 0 {
            return self.colors.iter().position(|p| p.a == 0);
        }
        let lab = oklab(c);
        let mut best = None;
        let mut best_d = f32::MAX;
        for (i, p) in self.colors.iter().enumerate() {
            if p.a == 0 {
                continue;
            }
            let d = lab_dist(lab, oklab(*p));
            if d < best_d {
                best_d = d;
                best = Some(i);
            }
        }
        best
    }

    /// Snap a color to the palette, keeping its alpha. Colors are returned
    /// unchanged when the palette is empty.
    pub fn snap(&self, c: Rgba8) -> Rgba8 {
        match self.nearest(c) {
            Some(i) if c.a > 0 => self.colors[i].with_alpha(c.a),
            _ => c,
        }
    }

    /// Add a color unless an identical one is already in the palette.
    /// Returns the slot either way, or `None` when the palette is full.
    pub fn add_unique(&mut self, c: Rgba8) -> Option<usize> {
        if let Some(i) = self.index_of(c) {
            return Some(i);
        }
        if self.colors.len() >= MAX_COLORS {
            return None;
        }
        self.colors.push(c);
        Some(self.colors.len() - 1)
    }

    /// Sort by hue, then lightness (grays first).
    pub fn sort_by_hue(&mut self) {
        self.colors.sort_by(|a, b| {
            let (ha, hb) = (a.to_hsv(), b.to_hsv());
            let ka = (ha.s > 0.08) as u8;
            let kb = (hb.s > 0.08) as u8;
            ka.cmp(&kb)
                .then(ha.h.partial_cmp(&hb.h).unwrap_or(std::cmp::Ordering::Equal))
                .then(a.luma().partial_cmp(&b.luma()).unwrap_or(std::cmp::Ordering::Equal))
        });
    }

    /// Sort dark to light.
    pub fn sort_by_luma(&mut self) {
        self.colors.sort_by(|a, b| a.luma().partial_cmp(&b.luma()).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// Replace the run `from..=to` with a ramp between its end colors,
    /// keeping the same number of slots.
    pub fn ramp_between(&mut self, from: usize, to: usize) {
        let (a, b) = (from.min(to), from.max(to));
        if b >= self.colors.len() || b - a < 2 {
            return;
        }
        let r = ramp(self.colors[a], self.colors[b], b - a + 1);
        self.colors[a..=b].copy_from_slice(&r);
    }

    /// The most-used colors of an image (exact when there are at most `max`
    /// distinct opaque colors, otherwise median-cut quantized).
    pub fn from_rgba(name: impl Into<String>, rgba: &[u8], max: usize) -> Self {
        let counts = count_colors(rgba);
        let max = max.clamp(1, MAX_COLORS);
        let colors = if counts.len() <= max {
            let mut v = counts;
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            v.into_iter().map(|(c, _)| Rgba8::from_array(c)).collect()
        } else {
            quantize(&counts, max)
        };
        Self::new(name, colors)
    }
}

fn same_color(a: Rgba8, b: Rgba8) -> bool {
    (a.a == 0 && b.a == 0) || (a.r == b.r && a.g == b.g && a.b == b.b && a.a > 0 && b.a > 0)
}

/// Distinct opaque colors of straight RGBA bytes with their pixel counts.
pub fn count_colors(rgba: &[u8]) -> Vec<([u8; 4], u32)> {
    use std::collections::HashMap;
    let mut counts: HashMap<[u8; 4], u32> = HashMap::new();
    for p in rgba.chunks_exact(4) {
        if p[3] > 0 {
            *counts.entry([p[0], p[1], p[2], 255]).or_default() += 1;
        }
    }
    counts.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Oklab, for ramps and nearest-color matching that follow perception.

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB → Oklab (L, a, b).
pub fn oklab(c: Rgba8) -> [f32; 3] {
    let r = srgb_to_linear(c.r as f32 / 255.0);
    let g = srgb_to_linear(c.g as f32 / 255.0);
    let b = srgb_to_linear(c.b as f32 / 255.0);
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Oklab → sRGB (opaque).
pub fn from_oklab(lab: [f32; 3]) -> Rgba8 {
    let l_ = lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2];
    let m_ = lab[0] - 0.105_561_346 * lab[1] - 0.063_854_17 * lab[2];
    let s_ = lab[0] - 0.089_484_18 * lab[1] - 1.291_485_5 * lab[2];
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let b = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
    let u = |v: f32| (linear_to_srgb(v) * 255.0 + 0.5) as u8;
    Rgba8::rgb(u(r), u(g), u(b))
}

fn lab_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// `steps` colors from `a` to `b` inclusive, interpolated in Oklab so the
/// middle never goes muddy. Alpha follows linearly.
pub fn ramp(a: Rgba8, b: Rgba8, steps: usize) -> Vec<Rgba8> {
    let steps = steps.max(2);
    let (la, lb) = (oklab(a), oklab(b));
    (0..steps)
        .map(|i| {
            let t = i as f32 / (steps - 1) as f32;
            let lab = [la[0] + (lb[0] - la[0]) * t, la[1] + (lb[1] - la[1]) * t, la[2] + (lb[2] - la[2]) * t];
            let alpha = a.a as f32 + (b.a as f32 - a.a as f32) * t;
            from_oklab(lab).with_alpha((alpha + 0.5) as u8)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Median-cut quantization.

/// Reduce weighted colors to at most `n` representative colors (median cut
/// on RGB, each box replaced by its weighted mean).
pub fn quantize(counts: &[([u8; 4], u32)], n: usize) -> Vec<Rgba8> {
    let n = n.clamp(1, MAX_COLORS);
    if counts.is_empty() {
        return Vec::new();
    }
    let mut boxes: Vec<Vec<([u8; 4], u32)>> = vec![counts.to_vec()];
    while boxes.len() < n {
        // Split the box with the largest channel spread (weighted by size).
        let mut pick = None;
        let mut pick_score = 0u64;
        for (i, b) in boxes.iter().enumerate() {
            if b.len() < 2 {
                continue;
            }
            let (ch, range) = widest_channel(b);
            let score = range as u64 * b.len() as u64;
            if score > pick_score {
                pick_score = score;
                pick = Some((i, ch));
            }
        }
        let Some((i, ch)) = pick else { break };
        let mut b = boxes.swap_remove(i);
        b.sort_by_key(|(c, _)| c[ch]);
        let total: u64 = b.iter().map(|(_, w)| *w as u64).sum();
        let mut acc = 0u64;
        let mut cut = 0;
        for (k, (_, w)) in b.iter().enumerate() {
            acc += *w as u64;
            if acc * 2 >= total {
                cut = k + 1;
                break;
            }
        }
        let cut = cut.clamp(1, b.len() - 1);
        let right = b.split_off(cut);
        boxes.push(b);
        boxes.push(right);
    }
    let mut out: Vec<Rgba8> = boxes
        .iter()
        .map(|b| {
            let w: u64 = b.iter().map(|(_, w)| *w as u64).sum::<u64>().max(1);
            let sum = |ch: usize| (b.iter().map(|(c, k)| c[ch] as u64 * *k as u64).sum::<u64>() + w / 2) / w;
            Rgba8::rgb(sum(0) as u8, sum(1) as u8, sum(2) as u8)
        })
        .collect();
    out.sort_by(|a, b| a.luma().partial_cmp(&b.luma()).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup();
    out
}

fn widest_channel(b: &[([u8; 4], u32)]) -> (usize, u8) {
    let mut lo = [255u8; 3];
    let mut hi = [0u8; 3];
    for (c, _) in b {
        for ch in 0..3 {
            lo[ch] = lo[ch].min(c[ch]);
            hi[ch] = hi[ch].max(c[ch]);
        }
    }
    let mut best = (0, 0u8);
    for ch in 0..3 {
        let r = hi[ch] - lo[ch];
        if r > best.1 {
            best = (ch, r);
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Presets.

/// Built-in palettes: (name, colors).
pub fn presets() -> Vec<Palette> {
    let hex = |name: &str, list: &[&str]| Palette::new(name, list.iter().filter_map(|s| Rgba8::from_hex(s)).collect());
    let grays = |name: &str, n: usize| {
        Palette::new(
            name,
            (0..n)
                .map(|i| Rgba8::rgb((i * 255 / (n - 1)) as u8, (i * 255 / (n - 1)) as u8, (i * 255 / (n - 1)) as u8))
                .collect(),
        )
    };
    vec![
        hex(
            "PICO-8",
            &[
                "000000", "1D2B53", "7E2553", "008751", "AB5236", "5F574F", "C2C3C7", "FFF1E8", "FF004D", "FFA300",
                "FFEC27", "00E436", "29ADFF", "83769C", "FF77A8", "FFCCAA",
            ],
        ),
        hex(
            "Sweetie 16",
            &[
                "1a1c2c", "5d275d", "b13e53", "ef7d57", "ffcd75", "a7f070", "38b764", "257179", "29366f", "3b5dc9",
                "41a6f6", "73eff7", "f4f4f4", "94b0c2", "566c86", "333c57",
            ],
        ),
        hex(
            "DawnBringer 16",
            &[
                "140c1c", "442434", "30346d", "4e4a4e", "854c30", "346524", "d04648", "757161", "597dce", "d27d2c",
                "8595a1", "6daa2c", "d2aa99", "6dc2ca", "dad45e", "deeed6",
            ],
        ),
        hex(
            "DawnBringer 32",
            &[
                "000000", "222034", "45283c", "663931", "8f563b", "df7126", "d9a066", "eec39a", "fbf236", "99e550",
                "6abe30", "37946e", "4b692f", "524b24", "323c39", "3f3f74", "306082", "5b6ee1", "639bff", "5fcde4",
                "cbdbfc", "ffffff", "9badb7", "847e87", "696a6a", "595652", "76428a", "ac3232", "d95763", "d77bba",
                "8f974a", "8a6f30",
            ],
        ),
        hex(
            "Endesga 32",
            &[
                "be4a2f", "d77643", "ead4aa", "e4a672", "b86f50", "733e39", "3e2731", "a22633", "e43b44", "f77622",
                "feae34", "fee761", "63c74d", "3e8948", "265c42", "193c3e", "124e89", "0099db", "2ce8f5", "ffffff",
                "c0cbdc", "8b9bb4", "5a6988", "3a4466", "262b44", "181425", "ff0044", "68386c", "b55088", "f6757a",
                "e8b796", "c28569",
            ],
        ),
        hex(
            "Commodore 64",
            &[
                "000000", "ffffff", "880000", "aaffee", "cc44cc", "00cc55", "0000aa", "eeee77", "dd8855", "664400",
                "ff7777", "333333", "777777", "aaff66", "0088ff", "bbbbbb",
            ],
        ),
        hex(
            "CGA",
            &[
                "000000", "0000aa", "00aa00", "00aaaa", "aa0000", "aa00aa", "aa5500", "aaaaaa", "555555", "5555ff",
                "55ff55", "55ffff", "ff5555", "ff55ff", "ffff55", "ffffff",
            ],
        ),
        hex("Game Boy", &["0f380f", "306230", "8bac0f", "9bbc0f"]),
        grays("Grayscale 4", 4),
        grays("Grayscale 16", 16),
    ]
}

// ---------------------------------------------------------------------------
// File formats.

/// Extensions `load` understands.
pub const LOAD_EXTENSIONS: &[&str] =
    &["gpl", "hex", "txt", "pal", "act", "aco", "ase", "aseprite", "png", "gif", "bmp"];
/// Extensions `save` understands.
pub const SAVE_EXTENSIONS: &[&str] = &["gpl", "hex", "pal", "act", "aco", "png"];

fn ext_of(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn stem_of(path: &Path) -> String {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("Palette").to_string()
}

/// Read a palette from any supported file. Images contribute their distinct
/// colors in reading order (quantized when there are more than 256).
pub fn load(path: &Path) -> anyhow::Result<Palette> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let name = stem_of(path);
    match ext_of(path).as_str() {
        "gpl" => parse_gpl(&bytes, &name),
        "hex" | "txt" => parse_hex(&bytes, &name),
        "pal" => parse_pal(&bytes, &name),
        "act" => parse_act(&bytes, &name),
        "aco" => parse_aco(&bytes, &name),
        "ase" | "aseprite" => {
            let sprite = crate::io::ase::parse(&bytes)?;
            let n = sprite.palette.len();
            Ok(Palette::new(name, sprite.palette.into_iter().take(n.min(MAX_COLORS)).collect()))
        }
        _ => {
            let raster = crate::io::image_io::decode_bytes(&bytes)?;
            let rgba = raster.to_rgba();
            // Reading order matters for a palette strip, so keep first-seen
            // order when the image has 256 colors or fewer.
            let mut seen = std::collections::HashSet::new();
            let mut colors = Vec::new();
            for p in rgba.chunks_exact(4) {
                if p[3] == 0 {
                    continue;
                }
                let c = [p[0], p[1], p[2], 255];
                if seen.insert(c) {
                    colors.push(Rgba8::from_array(c));
                    if colors.len() > MAX_COLORS {
                        return Ok(Palette::from_rgba(name, &rgba, MAX_COLORS));
                    }
                }
            }
            Ok(Palette::new(name, colors))
        }
    }
}

/// Write a palette in the format its extension names.
pub fn save(path: &Path, pal: &Palette) -> anyhow::Result<()> {
    let bytes = match ext_of(path).as_str() {
        "gpl" => write_gpl(pal),
        "hex" | "txt" => write_hex(pal),
        "pal" => write_pal(pal),
        "act" => write_act(pal),
        "aco" => write_aco(pal),
        "png" => write_png(pal)?,
        e => bail!("unsupported palette format \".{e}\""),
    };
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn parse_gpl(bytes: &[u8], fallback: &str) -> anyhow::Result<Palette> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines();
    if !lines.next().is_some_and(|l| l.trim_start().starts_with("GIMP Palette")) {
        bail!("not a GIMP palette");
    }
    let mut name = fallback.to_string();
    let mut colors = Vec::new();
    for line in lines {
        let l = line.trim();
        if let Some(n) = l.strip_prefix("Name:") {
            name = n.trim().to_string();
            continue;
        }
        if l.is_empty() || l.starts_with('#') || l.starts_with("Columns:") {
            continue;
        }
        let mut it = l.split_whitespace();
        let (Some(r), Some(g), Some(b)) = (it.next(), it.next(), it.next()) else { continue };
        if let (Ok(r), Ok(g), Ok(b)) = (r.parse::<u8>(), g.parse::<u8>(), b.parse::<u8>()) {
            colors.push(Rgba8::rgb(r, g, b));
        }
    }
    Ok(Palette::new(name, colors))
}

fn write_gpl(pal: &Palette) -> Vec<u8> {
    let mut s =
        format!("GIMP Palette\nName: {}\nColumns: 16\n#\n", if pal.name.is_empty() { "qsketch" } else { &pal.name });
    for (i, c) in pal.colors.iter().enumerate() {
        s.push_str(&format!("{:3} {:3} {:3}\tColor {}\n", c.r, c.g, c.b, i));
    }
    s.into_bytes()
}

fn parse_hex(bytes: &[u8], name: &str) -> anyhow::Result<Palette> {
    let text = String::from_utf8_lossy(bytes);
    let colors: Vec<Rgba8> = text
        .lines()
        .map(|l| l.split(|c: char| c == ';' || c == ',' || c.is_whitespace()).next().unwrap_or("").trim())
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .filter_map(Rgba8::from_hex)
        .collect();
    if colors.is_empty() {
        bail!("no colors found");
    }
    Ok(Palette::new(name, colors))
}

fn write_hex(pal: &Palette) -> Vec<u8> {
    pal.colors.iter().map(|c| format!("{:02x}{:02x}{:02x}\n", c.r, c.g, c.b)).collect::<String>().into_bytes()
}

fn parse_pal(bytes: &[u8], name: &str) -> anyhow::Result<Palette> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("JASC-PAL") {
        // Some `.pal` files are raw RIFF or plain hex lists; try hex.
        return parse_hex(bytes, name);
    }
    let _version = lines.next();
    let _count = lines.next();
    let mut colors = Vec::new();
    for l in lines {
        let mut it = l.split_whitespace();
        let (Some(r), Some(g), Some(b)) = (it.next(), it.next(), it.next()) else { continue };
        if let (Ok(r), Ok(g), Ok(b)) = (r.parse::<u8>(), g.parse::<u8>(), b.parse::<u8>()) {
            colors.push(Rgba8::rgb(r, g, b));
        }
    }
    Ok(Palette::new(name, colors))
}

fn write_pal(pal: &Palette) -> Vec<u8> {
    let mut s = format!("JASC-PAL\r\n0100\r\n{}\r\n", pal.colors.len());
    for c in &pal.colors {
        s.push_str(&format!("{} {} {}\r\n", c.r, c.g, c.b));
    }
    s.into_bytes()
}

fn parse_act(bytes: &[u8], name: &str) -> anyhow::Result<Palette> {
    if bytes.len() < 768 {
        bail!("Adobe Color Table is too short");
    }
    let mut count = 256usize;
    let mut transparent = None;
    if bytes.len() >= 772 {
        count = u16::from_be_bytes([bytes[768], bytes[769]]) as usize;
        let t = u16::from_be_bytes([bytes[770], bytes[771]]);
        if t != 0xFFFF {
            transparent = Some(t as usize);
        }
        if count == 0 || count > 256 {
            count = 256;
        }
    }
    let mut colors: Vec<Rgba8> =
        bytes[..768].chunks_exact(3).take(count).map(|c| Rgba8::rgb(c[0], c[1], c[2])).collect();
    if let Some(t) = transparent {
        if let Some(c) = colors.get_mut(t) {
            c.a = 0;
        }
    }
    Ok(Palette::new(name, colors))
}

fn write_act(pal: &Palette) -> Vec<u8> {
    let mut out = vec![0u8; 768];
    for (i, c) in pal.colors.iter().take(256).enumerate() {
        out[i * 3] = c.r;
        out[i * 3 + 1] = c.g;
        out[i * 3 + 2] = c.b;
    }
    let n = pal.colors.len().min(256) as u16;
    out.extend_from_slice(&n.to_be_bytes());
    let t = pal.colors.iter().position(|c| c.a == 0).map(|i| i as u16).unwrap_or(0xFFFF);
    out.extend_from_slice(&t.to_be_bytes());
    out
}

fn parse_aco(bytes: &[u8], name: &str) -> anyhow::Result<Palette> {
    let u16_at = |i: usize| -> anyhow::Result<u16> {
        bytes.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]])).ok_or_else(|| anyhow!("truncated .aco"))
    };
    let version = u16_at(0)?;
    if version != 1 && version != 2 {
        bail!("unsupported .aco version {version}");
    }
    let count = u16_at(2)? as usize;
    let mut colors = Vec::with_capacity(count);
    let mut pos = 4;
    for _ in 0..count {
        let space = u16_at(pos)?;
        let w = [u16_at(pos + 2)?, u16_at(pos + 4)?, u16_at(pos + 6)?, u16_at(pos + 8)?];
        pos += 10;
        if version == 2 {
            // v2 entries carry a name: u16 0, u32 length (in u16s incl. NUL), chars.
            let len = ((u16_at(pos + 2)? as usize) << 16) | u16_at(pos + 4)? as usize;
            pos += 6 + len * 2;
        }
        let c = match space {
            0 => Rgba8::rgb((w[0] >> 8) as u8, (w[1] >> 8) as u8, (w[2] >> 8) as u8),
            1 => {
                // HSB: hue 0..65535 → 0..360, saturation / brightness 0..65535.
                let hsv =
                    crate::color::Hsv::new(w[0] as f32 * 360.0 / 65535.0, w[1] as f32 / 65535.0, w[2] as f32 / 65535.0);
                hsv.to_rgba8(255)
            }
            2 => {
                // CMYK stored as 100% minus the ink amount.
                let k = w[3] as f32 / 65535.0;
                let f = |v: u16| ((v as f32 / 65535.0) * k * 255.0 + 0.5) as u8;
                Rgba8::rgb(f(w[0]), f(w[1]), f(w[2]))
            }
            8 => {
                let g = ((10000 - w[0].min(10000)) as u32 * 255 / 10000) as u8;
                Rgba8::rgb(g, g, g)
            }
            _ => continue,
        };
        colors.push(c);
    }
    Ok(Palette::new(name, colors))
}

fn write_aco(pal: &Palette) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(pal.colors.len() as u16).to_be_bytes());
    for c in &pal.colors {
        out.extend_from_slice(&0u16.to_be_bytes());
        for v in [c.r, c.g, c.b] {
            out.extend_from_slice(&(v as u16 * 257).to_be_bytes());
        }
        out.extend_from_slice(&0u16.to_be_bytes());
    }
    out
}

/// A one-pixel-per-color strip, 16 per row, so any image editor can read it.
fn write_png(pal: &Palette) -> anyhow::Result<Vec<u8>> {
    let n = pal.colors.len().max(1);
    let w = n.min(16) as u32;
    let h = n.div_ceil(16) as u32;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for (i, c) in pal.colors.iter().enumerate() {
        rgba[i * 4..i * 4 + 4].copy_from_slice(&c.to_array());
    }
    crate::io::image_io::encode_png(w, h, &rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_and_snap() {
        let p = Palette::new("t", vec![Rgba8::BLACK, Rgba8::WHITE, Rgba8::rgb(255, 0, 0)]);
        assert_eq!(p.nearest(Rgba8::rgb(250, 10, 10)), Some(2));
        assert_eq!(p.nearest(Rgba8::rgb(20, 20, 20)), Some(0));
        assert_eq!(p.snap(Rgba8::new(240, 240, 240, 128)), Rgba8::new(255, 255, 255, 128));
        assert_eq!(p.nearest(Rgba8::TRANSPARENT), None);
        assert_eq!(p.snap(Rgba8::TRANSPARENT), Rgba8::TRANSPARENT);
    }

    #[test]
    fn oklab_roundtrip() {
        for c in [Rgba8::rgb(12, 200, 90), Rgba8::rgb(255, 255, 255), Rgba8::rgb(0, 0, 0), Rgba8::rgb(128, 64, 200)] {
            let back = from_oklab(oklab(c));
            assert!(back.max_channel_diff(c) <= 1, "{c:?} -> {back:?}");
        }
    }

    #[test]
    fn ramp_ends_are_exact() {
        let r = ramp(Rgba8::rgb(10, 20, 30), Rgba8::rgb(200, 210, 220), 5);
        assert_eq!(r.len(), 5);
        assert!(r[0].max_channel_diff(Rgba8::rgb(10, 20, 30)) <= 1);
        assert!(r[4].max_channel_diff(Rgba8::rgb(200, 210, 220)) <= 1);
        let mut p = Palette::new("t", vec![Rgba8::BLACK, Rgba8::BLACK, Rgba8::BLACK, Rgba8::WHITE]);
        p.ramp_between(0, 3);
        assert!(p.colors[1].luma() < p.colors[2].luma());
    }

    #[test]
    fn quantize_reduces() {
        let mut rgba = Vec::new();
        for i in 0..256u32 {
            rgba.extend_from_slice(&[i as u8, (255 - i) as u8, 128, 255]);
        }
        let p = Palette::from_rgba("q", &rgba, 8);
        assert!(p.len() <= 8 && p.len() >= 4, "{}", p.len());
        let exact = Palette::from_rgba("e", &[0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255], 16);
        assert_eq!(exact.len(), 2);
    }

    #[test]
    fn formats_roundtrip() {
        let pal = Palette::new("Test", vec![Rgba8::rgb(1, 2, 3), Rgba8::rgb(200, 100, 50), Rgba8::WHITE]);
        assert_eq!(parse_gpl(&write_gpl(&pal), "x").unwrap().colors, pal.colors);
        assert_eq!(parse_gpl(&write_gpl(&pal), "x").unwrap().name, "Test");
        assert_eq!(parse_hex(&write_hex(&pal), "x").unwrap().colors, pal.colors);
        assert_eq!(parse_pal(&write_pal(&pal), "x").unwrap().colors, pal.colors);
        assert_eq!(parse_act(&write_act(&pal), "x").unwrap().colors, pal.colors);
        assert_eq!(parse_aco(&write_aco(&pal), "x").unwrap().colors, pal.colors);
        let png = write_png(&pal).unwrap();
        let r = crate::io::image_io::decode_bytes(&png).unwrap();
        assert_eq!(r.get_pixel(1, 0), Rgba8::rgb(200, 100, 50));
        assert!(presets().iter().all(|p| !p.is_empty()));
    }
}
