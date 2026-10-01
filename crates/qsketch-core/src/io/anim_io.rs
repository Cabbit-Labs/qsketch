//! Animation import and export: animated GIF and APNG (both ways), numbered
//! PNG frames, sprite sheets with Aseprite-style JSON, and video through
//! `ffmpeg` when it is installed.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{anyhow, bail, Context};
use image::AnimationDecoder;

use crate::anim::{Frame, NewFrame};
use crate::color::Rgba8;
use crate::document::DocState;
use crate::raster::Raster;

/// Rendered frames ready to be written: straight RGBA, all the same size.
pub struct Frames {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<RenderedFrame>,
}

pub struct RenderedFrame {
    /// Index in the document.
    pub index: usize,
    pub rgba: Vec<u8>,
    pub duration_ms: u32,
}

/// Render the given frames of `doc` at `scale`× (nearest neighbor), with
/// the document's pixel aspect ratio applied. `done` counts frames.
pub fn render(doc: &DocState, which: &[usize], scale: u32, done: &AtomicUsize) -> anyhow::Result<Frames> {
    let scale = scale.clamp(1, 16);
    let [ax, ay] = if doc.pixel_aspect.contains(&0) { [1, 1] } else { doc.pixel_aspect };
    let (sx, sy) = (scale * ax as u32, scale * ay as u32);
    let (w, h) = (doc.width * sx, doc.height * sy);
    if w == 0 || h == 0 || w > 16_384 || h > 16_384 {
        bail!("frames would be {w}×{h} pixels; at most 16384 on a side");
    }
    let mut frames = Vec::with_capacity(which.len());
    for (k, &f) in which.iter().enumerate() {
        let f = f.min(doc.frame_count() - 1);
        let flat = crate::anim::render_frame(doc, f);
        let rgba = if sx == 1 && sy == 1 {
            flat.to_rgba()
        } else {
            flat.resized(w, h, crate::raster::ResizeFilter::Nearest).to_rgba()
        };
        frames.push(RenderedFrame { index: f, rgba, duration_ms: doc.frame_duration(f) });
        done.store(k + 1, Ordering::Relaxed);
    }
    Ok(Frames { width: w, height: h, frames })
}

/// Every frame of the document, or those of a tag.
pub fn frame_list(doc: &DocState, tag: Option<usize>) -> Vec<usize> {
    match tag.and_then(|t| doc.tags.get(t)) {
        Some(t) => (t.from..=t.to.min(doc.frame_count() - 1)).collect(),
        None => (0..doc.frame_count()).collect(),
    }
}

// ---------------------------------------------------------------------------
// GIF

/// Write a looping animated GIF. Frames with at most 255 distinct colors
/// (plus transparency) are written with an exact palette, so pixel art
/// comes out untouched; richer frames are quantized per frame.
pub fn write_gif(frames: &Frames, path: &Path, repeat: bool, done: &AtomicUsize) -> anyhow::Result<()> {
    use gif::{DisposalMethod, Encoder, Repeat};
    if frames.frames.is_empty() {
        bail!("nothing to export");
    }
    // A global palette of every color in every frame, when it fits.
    let mut colors: Vec<[u8; 3]> = Vec::new();
    let mut lookup: std::collections::HashMap<[u8; 3], u8> = std::collections::HashMap::new();
    let mut has_alpha = false;
    let mut fits = true;
    'scan: for f in &frames.frames {
        for p in f.rgba.chunks_exact(4) {
            if p[3] < 128 {
                has_alpha = true;
                continue;
            }
            let key = [p[0], p[1], p[2]];
            if let std::collections::hash_map::Entry::Vacant(e) = lookup.entry(key) {
                if colors.len() >= 255 {
                    fits = false;
                    break 'scan;
                }
                e.insert(colors.len() as u8);
                colors.push(key);
            }
        }
    }
    let file = std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut out = BufWriter::new(file);
    let (w, h) = (frames.width as u16, frames.height as u16);
    if fits {
        // Index 0 is the transparent slot when needed; colors follow.
        let mut pal: Vec<u8> = Vec::with_capacity((colors.len() + 1) * 3);
        let transparent = if has_alpha || colors.is_empty() {
            pal.extend_from_slice(&[0, 0, 0]);
            Some(0u8)
        } else {
            None
        };
        let base = pal.len() as u8 / 3;
        for c in &colors {
            pal.extend_from_slice(c);
        }
        let mut enc = Encoder::new(&mut out, w, h, &pal)?;
        enc.set_repeat(if repeat { Repeat::Infinite } else { Repeat::Finite(0) })?;
        for (i, f) in frames.frames.iter().enumerate() {
            let buf: Vec<u8> = f
                .rgba
                .chunks_exact(4)
                .map(|p| if p[3] < 128 { 0 } else { base + lookup[&[p[0], p[1], p[2]]] })
                .collect();
            let frame = gif::Frame {
                delay: gif_delay(f.duration_ms),
                dispose: if has_alpha { DisposalMethod::Background } else { DisposalMethod::Keep },
                transparent,
                width: w,
                height: h,
                buffer: std::borrow::Cow::Owned(buf),
                ..Default::default()
            };
            enc.write_frame(&frame)?;
            done.store(i + 1, Ordering::Relaxed);
        }
    } else {
        let mut enc = Encoder::new(&mut out, w, h, &[])?;
        enc.set_repeat(if repeat { Repeat::Infinite } else { Repeat::Finite(0) })?;
        for (i, f) in frames.frames.iter().enumerate() {
            let mut rgba = f.rgba.clone();
            let mut frame = gif::Frame::from_rgba_speed(w, h, &mut rgba, 10);
            frame.delay = gif_delay(f.duration_ms);
            frame.dispose = DisposalMethod::Background;
            enc.write_frame(&frame)?;
            done.store(i + 1, Ordering::Relaxed);
        }
    }
    out.flush()?;
    Ok(())
}

/// GIF delays are in hundredths of a second; browsers treat 0 and 1 as
/// "fast", so 2 (20 ms) is the floor.
fn gif_delay(ms: u32) -> u16 {
    ((ms + 5) / 10).clamp(2, u16::MAX as u32) as u16
}

// ---------------------------------------------------------------------------
// APNG

/// Write an animated PNG (APNG), full RGBA.
pub fn write_apng(frames: &Frames, path: &Path, repeat: bool, done: &AtomicUsize) -> anyhow::Result<()> {
    if frames.frames.is_empty() {
        bail!("nothing to export");
    }
    let file = std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut enc = png::Encoder::new(BufWriter::new(file), frames.width, frames.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_animated(frames.frames.len() as u32, if repeat { 0 } else { 1 })?;
    enc.set_dispose_op(png::DisposeOp::Background)?;
    enc.set_blend_op(png::BlendOp::Source)?;
    let mut writer = enc.write_header()?;
    for (i, f) in frames.frames.iter().enumerate() {
        writer.set_frame_delay(f.duration_ms.min(u16::MAX as u32) as u16, 1000)?;
        writer.write_image_data(&f.rgba)?;
        done.store(i + 1, Ordering::Relaxed);
    }
    writer.finish()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// PNG sequence

/// Write `stem_0001.png`… into `dir`; returns the paths written.
pub fn write_sequence(frames: &Frames, dir: &Path, stem: &str, done: &AtomicUsize) -> anyhow::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut paths = Vec::with_capacity(frames.frames.len());
    for (i, f) in frames.frames.iter().enumerate() {
        let path = dir.join(format!("{stem}_{:04}.png", f.index + 1));
        std::fs::write(&path, super::image_io::encode_png(frames.width, frames.height, &f.rgba)?)
            .with_context(|| format!("writing {}", path.display()))?;
        paths.push(path);
        done.store(i + 1, Ordering::Relaxed);
    }
    Ok(paths)
}

// ---------------------------------------------------------------------------
// Sprite sheet

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SheetLayout {
    Horizontal,
    Vertical,
    /// Rows of `columns` frames (0 = as square as possible).
    #[default]
    Grid,
}

impl SheetLayout {
    pub const ALL: [SheetLayout; 3] = [SheetLayout::Horizontal, SheetLayout::Vertical, SheetLayout::Grid];
    pub fn label(self) -> &'static str {
        match self {
            SheetLayout::Horizontal => "Horizontal strip",
            SheetLayout::Vertical => "Vertical strip",
            SheetLayout::Grid => "Grid",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SheetOptions {
    pub layout: SheetLayout,
    pub columns: u32,
    /// Pixels between frames, and around the whole sheet.
    pub padding: u32,
    pub border: u32,
    /// Also write `<name>.json` describing the frames (Aseprite's layout).
    pub json: bool,
}

impl Default for SheetOptions {
    fn default() -> Self {
        Self { layout: SheetLayout::Grid, columns: 0, padding: 0, border: 0, json: true }
    }
}

/// Columns × rows of a sheet for `n` frames.
pub fn sheet_grid(layout: SheetLayout, columns: u32, n: usize) -> (u32, u32) {
    let n = n.max(1) as u32;
    match layout {
        SheetLayout::Horizontal => (n, 1),
        SheetLayout::Vertical => (1, n),
        SheetLayout::Grid => {
            let cols = if columns == 0 { (n as f64).sqrt().ceil() as u32 } else { columns.min(n) }.max(1);
            (cols, n.div_ceil(cols))
        }
    }
}

/// Write the frames side by side into one PNG (and its JSON, when asked).
/// `name` is what the JSON calls the sprite; `extra` describes the
/// document's tags, layers and slices for the JSON's `meta`.
pub fn write_sheet(
    frames: &Frames,
    path: &Path,
    opts: &SheetOptions,
    doc: &DocState,
    name: &str,
    done: &AtomicUsize,
) -> anyhow::Result<()> {
    if frames.frames.is_empty() {
        bail!("nothing to export");
    }
    let (fw, fh) = (frames.width, frames.height);
    let (cols, rows) = sheet_grid(opts.layout, opts.columns, frames.frames.len());
    let (pad, border) = (opts.padding, opts.border);
    let sheet_w = border * 2 + cols * fw + (cols - 1) * pad;
    let sheet_h = border * 2 + rows * fh + (rows - 1) * pad;
    if sheet_w > 16_384 || sheet_h > 16_384 {
        bail!("the sheet would be {sheet_w}×{sheet_h} pixels; at most 16384 on a side");
    }
    let mut sheet = Raster::new(sheet_w, sheet_h);
    let mut rects = Vec::with_capacity(frames.frames.len());
    for (i, f) in frames.frames.iter().enumerate() {
        let (cx, cy) = (i as u32 % cols, i as u32 / cols);
        let x = (border + cx * (fw + pad)) as i32;
        let y = (border + cy * (fh + pad)) as i32;
        sheet.blit(&Raster::from_rgba(fw, fh, &f.rgba), x, y, false);
        rects.push((x, y));
        done.store(i + 1, Ordering::Relaxed);
    }
    super::image_io::export_raster(path, &sheet)?;
    if opts.json {
        let json = sheet_json(frames, &rects, doc, name, path, sheet_w, sheet_h);
        let jpath = path.with_extension("json");
        std::fs::write(&jpath, serde_json::to_string_pretty(&json)?)
            .with_context(|| format!("writing {}", jpath.display()))?;
    }
    Ok(())
}

/// Aseprite's sprite-sheet JSON ("hash" style): a `frames` map plus `meta`
/// with the tags, layers and slices, which game engines already read.
fn sheet_json(
    frames: &Frames,
    rects: &[(i32, i32)],
    doc: &DocState,
    name: &str,
    image: &Path,
    sheet_w: u32,
    sheet_h: u32,
) -> serde_json::Value {
    use serde_json::json;
    let (fw, fh) = (frames.width, frames.height);
    let mut map = serde_json::Map::new();
    for (f, &(x, y)) in frames.frames.iter().zip(rects) {
        map.insert(
            format!("{name} {}.ase", f.index),
            json!({
                "frame": { "x": x, "y": y, "w": fw, "h": fh },
                "rotated": false,
                "trimmed": false,
                "spriteSourceSize": { "x": 0, "y": 0, "w": fw, "h": fh },
                "sourceSize": { "w": fw, "h": fh },
                "duration": f.duration_ms,
            }),
        );
    }
    let hex = |c: Rgba8| format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a);
    let tags: Vec<_> = doc
        .tags
        .iter()
        .map(|t| {
            let mut v = json!({
                "name": t.name,
                "from": t.from,
                "to": t.to,
                "direction": t.direction.json_name(),
                "color": hex(t.color),
            });
            if t.repeat > 0 {
                v["repeat"] = json!(t.repeat.to_string());
            }
            v
        })
        .collect();
    let layers: Vec<_> = doc
        .layers
        .iter()
        .map(|l| {
            let mut v = json!({
                "name": l.props.name,
                "opacity": (l.props.opacity.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                "blendMode": l.props.blend.label().to_lowercase().replace(' ', "_"),
            });
            if let Some(p) = l.props.parent.and_then(|id| doc.layer_by_id(id)) {
                v["group"] = json!(p.props.name);
            }
            if let Some(c) = l.props.color {
                v["color"] = json!(format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]));
            }
            if !l.props.notes.is_empty() {
                v["data"] = json!(l.props.notes);
            }
            v
        })
        .collect();
    let slices: Vec<_> = doc
        .slices
        .iter()
        .map(|s| {
            let mut key = json!({
                "frame": 0,
                "bounds": { "x": s.rect.x, "y": s.rect.y, "w": s.rect.w, "h": s.rect.h },
            });
            if let Some(c) = s.center {
                key["center"] = json!({ "x": c.x, "y": c.y, "w": c.w, "h": c.h });
            }
            if let Some((px, py)) = s.pivot {
                key["pivot"] = json!({ "x": px, "y": py });
            }
            json!({ "name": s.name, "color": hex(s.color), "keys": [key] })
        })
        .collect();
    json!({
        "frames": map,
        "meta": {
            "app": "https://github.com/Cabbit-Labs/qsketch",
            "version": crate::VERSION,
            "image": image.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            "format": "RGBA8888",
            "size": { "w": sheet_w, "h": sheet_h },
            "scale": "1",
            "frameTags": tags,
            "layers": layers,
            "slices": slices,
        }
    })
}

// ---------------------------------------------------------------------------
// Video (ffmpeg)

/// Where `ffmpeg` is, if it can be found on the PATH.
pub fn ffmpeg_path() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(exe)).find(|p| p.is_file()))
}

/// Encode the frames as an MP4 (H.264) or WebM (VP9) by extension, over
/// `background`, using `ffmpeg`. Frame durations are honored by picking a
/// frame rate that divides them (capped at 60) and repeating frames.
pub fn write_video(frames: &Frames, path: &Path, background: Rgba8, done: &AtomicUsize) -> anyhow::Result<()> {
    let ffmpeg = ffmpeg_path().ok_or_else(|| anyhow!("ffmpeg isn't installed (or isn't on the PATH)"))?;
    if frames.frames.is_empty() {
        bail!("nothing to export");
    }
    let mut step = 0u32;
    for f in &frames.frames {
        step = gcd(step, f.duration_ms.max(1));
    }
    let step = step.max(1);
    let fps = (1000.0 / step as f64).clamp(1.0, 60.0);
    let tick = 1000.0 / fps;
    let dir = tempfile::tempdir().context("creating a temporary folder")?;
    let (w, h) = (frames.width, frames.height);
    let mut n = 0usize;
    for (i, f) in frames.frames.iter().enumerate() {
        // Over the background color (video has no alpha).
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for p in f.rgba.chunks_exact(4) {
            let a = p[3] as u32;
            for c in 0..3 {
                let bg = [background.r, background.g, background.b][c] as u32;
                rgb.push(((p[c] as u32 * a + bg * (255 - a)) / 255) as u8);
            }
        }
        let img: image::ImageBuffer<image::Rgb<u8>, _> =
            image::ImageBuffer::from_raw(w, h, rgb).ok_or_else(|| anyhow!("bad buffer"))?;
        let repeats = ((f.duration_ms as f64 / tick).round() as usize).max(1);
        let first = dir.path().join(format!("f{n:06}.png"));
        img.save(&first)?;
        n += 1;
        for _ in 1..repeats {
            let p = dir.path().join(format!("f{n:06}.png"));
            std::fs::copy(&first, &p)?;
            n += 1;
        }
        done.store(i + 1, Ordering::Relaxed);
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_ascii_lowercase();
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.arg("-y")
        .arg("-loglevel")
        .arg("error")
        .arg("-framerate")
        .arg(format!("{fps}"))
        .arg("-i")
        .arg(dir.path().join("f%06d.png"))
        .arg("-vf")
        .arg("pad=ceil(iw/2)*2:ceil(ih/2)*2");
    if ext == "webm" {
        cmd.args(["-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "24", "-pix_fmt", "yuv420p"]);
    } else {
        cmd.args([
            "-c:v",
            "libx264",
            "-crf",
            "16",
            "-preset",
            "slow",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
        ]);
    }
    cmd.arg(path);
    let out = cmd.output().context("running ffmpeg")?;
    if !out.status.success() {
        bail!("ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

// ---------------------------------------------------------------------------
// Import

/// Whether `path` is a GIF or PNG (the two formats that can carry frames).
pub fn might_be_animated(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        e.eq_ignore_ascii_case("gif") || e.eq_ignore_ascii_case("png") || e.eq_ignore_ascii_case("apng")
    })
}

/// Open an animated GIF or APNG as a document with one frame per picture;
/// `None` when the file holds a single still image.
pub fn import_animated(path: &Path) -> anyhow::Result<Option<DocState>> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = std::io::BufReader::new(file);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let frames: Vec<image::Frame> = if ext == "gif" {
        let dec = image::codecs::gif::GifDecoder::new(reader)?;
        dec.into_frames().collect_frames()?
    } else {
        let dec = image::codecs::png::PngDecoder::new(reader)?;
        if !dec.is_apng()? {
            return Ok(None);
        }
        dec.apng()?.into_frames().collect_frames()?
    };
    if frames.len() < 2 {
        return Ok(None);
    }
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Frames").to_string();
    let pictures: Vec<(Raster, u32)> = frames
        .into_iter()
        .map(|f| {
            let (num, den) = f.delay().numer_denom_ms();
            let ms = num.checked_div(den).map_or(100, |v| v.max(1));
            let buf = f.into_buffer();
            let (w, h) = buf.dimensions();
            (Raster::from_rgba(w, h, buf.as_raw()), ms)
        })
        .collect();
    Ok(Some(from_pictures(&name, pictures)))
}

/// A document whose frames are `pictures` (each sized to the first).
pub fn from_pictures(name: &str, pictures: Vec<(Raster, u32)>) -> DocState {
    let Some((first, _)) = pictures.first() else { return DocState::new(1, 1, None) };
    let (w, h) = (first.width(), first.height());
    let mut doc = DocState::from_raster(name, first.clone());
    doc.frames[0].duration_ms = pictures[0].1.max(1);
    for (i, (pic, ms)) in pictures.into_iter().enumerate().skip(1) {
        doc.insert_frame(i, NewFrame::Empty);
        let pic = if pic.width() != w || pic.height() != h { pic.with_canvas_size(w, h, 0, 0) } else { pic };
        doc.layers[0].raster = pic;
        doc.frames[i] = Frame { duration_ms: ms.max(1) };
    }
    doc.set_frame(0);
    doc
}

/// Cut a sprite sheet into `cell_w × cell_h` frames, left to right then top
/// to bottom, keeping at most `count` of them (0 = all); the frames become
/// a one-layer document.
pub fn from_sheet(name: &str, sheet: &Raster, cell_w: u32, cell_h: u32, count: usize, duration_ms: u32) -> DocState {
    let (cw, ch) = (cell_w.max(1).min(sheet.width()), cell_h.max(1).min(sheet.height()));
    let cols = sheet.width() / cw;
    let rows = sheet.height() / ch;
    let mut pictures = Vec::new();
    'outer: for r in 0..rows {
        for c in 0..cols {
            if count > 0 && pictures.len() >= count {
                break 'outer;
            }
            let cell = sheet.crop(crate::geom::IRect::new((c * cw) as i32, (r * ch) as i32, cw as i32, ch as i32));
            pictures.push((cell, duration_ms));
        }
    }
    // Trailing empty cells are padding, not frames.
    while pictures.len() > 1 && pictures.last().is_some_and(|(p, _)| p.is_empty()) {
        pictures.pop();
    }
    from_pictures(name, pictures)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_frame_doc() -> DocState {
        let mut d = DocState::new(4, 3, None);
        d.layers[0].raster.set_pixel(0, 0, Rgba8::new(255, 0, 0, 255));
        d.insert_frame(1, NewFrame::Empty);
        d.layers[0].raster.set_pixel(3, 2, Rgba8::new(0, 0, 255, 255));
        d.frames[1].duration_ms = 250;
        d.set_frame(0);
        d
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("anim-io-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn gif_round_trips_exact_colors_and_durations() {
        let d = two_frame_doc();
        let done = AtomicUsize::new(0);
        let frames = render(&d, &frame_list(&d, None), 2, &done).unwrap();
        assert_eq!((frames.width, frames.height), (8, 6));
        let dir = temp("gif");
        let path = dir.join("a.gif");
        write_gif(&frames, &path, true, &done).unwrap();
        let back = import_animated(&path).unwrap().expect("animated");
        assert_eq!(back.frame_count(), 2);
        assert_eq!(back.frames[1].duration_ms, 250);
        assert_eq!(back.layers[0].raster.get_pixel(0, 0), Rgba8::new(255, 0, 0, 255));
        assert_eq!(back.layers[0].raster.get_pixel(7, 5).a, 0);
        assert_eq!(back.cel_image(0, 1).unwrap().get_pixel(7, 5), Rgba8::new(0, 0, 255, 255));
        assert_eq!(back.cel_image(0, 1).unwrap().get_pixel(0, 0).a, 0, "frame 2 is its own picture");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apng_round_trips() {
        let d = two_frame_doc();
        let done = AtomicUsize::new(0);
        let frames = render(&d, &[0, 1], 1, &done).unwrap();
        let dir = temp("apng");
        let path = dir.join("a.png");
        write_apng(&frames, &path, true, &done).unwrap();
        let back = import_animated(&path).unwrap().expect("animated");
        assert_eq!(back.frame_count(), 2);
        assert_eq!(back.frames[1].duration_ms, 250);
        assert_eq!(back.cel_image(0, 1).unwrap().get_pixel(3, 2), Rgba8::new(0, 0, 255, 255));
        // A still PNG is not an animation.
        let still = dir.join("s.png");
        super::super::image_io::export(&still, &DocState::new(2, 2, None)).unwrap();
        assert!(import_animated(&still).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sheet_and_sequence_and_back() {
        let mut d = two_frame_doc();
        d.add_tag("go", 0, 1);
        let done = AtomicUsize::new(0);
        let frames = render(&d, &[0, 1], 1, &done).unwrap();
        let dir = temp("sheet");
        let path = dir.join("s.png");
        let opts = SheetOptions { layout: SheetLayout::Horizontal, padding: 0, border: 0, json: true, columns: 0 };
        write_sheet(&frames, &path, &opts, &d, "s", &done).unwrap();
        let sheet = super::super::image_io::import(&path).unwrap();
        assert_eq!((sheet.width(), sheet.height()), (8, 3));
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path.with_extension("json")).unwrap()).unwrap();
        assert_eq!(json["frames"]["s 1.ase"]["frame"]["x"], 4);
        assert_eq!(json["meta"]["frameTags"][0]["name"], "go");
        let back = from_sheet("s", &sheet, 4, 3, 0, 80);
        assert_eq!(back.frame_count(), 2);
        assert_eq!(back.cel_image(0, 1).unwrap().get_pixel(3, 2), Rgba8::new(0, 0, 255, 255));
        let paths = write_sequence(&frames, &dir.join("seq"), "s", &done).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths[1].ends_with("s_0002.png"));
        assert_eq!(sheet_grid(SheetLayout::Grid, 0, 10), (4, 3));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
