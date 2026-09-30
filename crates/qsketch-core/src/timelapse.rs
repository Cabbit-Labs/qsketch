//! Timelapse recording: small snapshots of the composite taken as the
//! document is edited, kept as encoded PNGs and saved inside the `.qsk`,
//! then played back as an animated GIF or written out as numbered frames.

use std::io::{BufWriter, Cursor};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::Context;
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, ImageEncoder, RgbaImage};

use crate::composite::Composite;

/// Frames kept at most. Past this, every other frame is dropped and the
/// capture interval doubles, so a long painting still spans its whole
/// history in a bounded file.
pub const MAX_FRAMES: usize = 3000;

/// A document's timelapse: recording state and the frames captured so far.
#[derive(Clone, Debug)]
pub struct Timelapse {
    pub recording: bool,
    /// PNG-encoded frames, oldest first.
    pub frames: Vec<Arc<[u8]>>,
    /// Committed edits between two frames (grows when frames are thinned).
    pub every: u32,
    /// `DocStats::edits` at the last capture; `None` captures at the next
    /// opportunity (just after recording starts). Runtime only.
    pub last_edits: Option<u64>,
}

impl Default for Timelapse {
    fn default() -> Self {
        Self { recording: false, frames: Vec::new(), every: 1, last_edits: None }
    }
}

impl Timelapse {
    /// Whether a frame is due at `edits` committed edits.
    pub fn due(&self, edits: u64) -> bool {
        self.recording && self.last_edits.is_none_or(|last| edits >= last + self.every.max(1) as u64)
    }

    /// Add a frame, thinning the recording once it reaches [`MAX_FRAMES`].
    pub fn push(&mut self, png: Arc<[u8]>) {
        self.frames.push(png);
        if self.frames.len() > MAX_FRAMES {
            let last = self.frames.pop();
            let mut i = 0;
            self.frames.retain(|_| {
                i += 1;
                i % 2 == 1
            });
            self.frames.extend(last);
            self.every = self.every.saturating_mul(2).max(2);
        }
    }

    pub fn clear(&mut self) {
        self.frames.clear();
        self.every = 1;
        self.last_edits = None;
    }

    /// Total encoded size of the frames, in bytes.
    pub fn bytes(&self) -> usize {
        self.frames.iter().map(|f| f.len()).sum()
    }
}

/// The size a `w × h` picture takes when fitted inside `max_side`.
pub fn fit(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    let longest = w.max(h).max(1);
    if longest <= max_side {
        return (w.max(1), h.max(1));
    }
    let s = max_side as f64 / longest as f64;
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}

/// A straight-alpha RGBA snapshot of the composite, box-filtered down so the
/// longer side is at most `max_side` (at most 4 × 4 samples per pixel, so it
/// stays quick for large canvases).
pub fn snapshot(comp: &Composite, max_side: u32) -> (u32, u32, Vec<u8>) {
    let (w, h) = (comp.width(), comp.height());
    let (tw, th) = fit(w, h, max_side);
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for ty in 0..th {
        let y0 = (ty as u64 * h as u64 / th as u64) as u32;
        let y1 = (((ty + 1) as u64 * h as u64 / th as u64) as u32).clamp(y0 + 1, h);
        let ys = ((y1 - y0) / 4).max(1);
        for tx in 0..tw {
            let x0 = (tx as u64 * w as u64 / tw as u64) as u32;
            let x1 = (((tx + 1) as u64 * w as u64 / tw as u64) as u32).clamp(x0 + 1, w);
            let xs = ((x1 - x0) / 4).max(1);
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    let p = comp.get_premul(x as i32, y as i32);
                    for c in 0..4 {
                        acc[c] += p[c] as u32;
                    }
                    n += 1;
                    x += xs;
                }
                y += ys;
            }
            let o = ((ty * tw + tx) * 4) as usize;
            let a = acc[3] / n.max(1);
            if a > 0 {
                for c in 0..3 {
                    out[o + c] = ((acc[c] * 255 + acc[3] / 2) / acc[3].max(1)).min(255) as u8;
                }
            }
            out[o + 3] = a as u8;
        }
    }
    (tw, th, out)
}

/// PNG-encode a frame (fast compression: frames are many and small).
pub fn encode(w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        Cursor::new(&mut buf),
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(rgba, w, h, image::ExtendedColorType::Rgba8)?;
    Ok(buf)
}

/// Decode every frame and lay it on a canvas the size of the last one
/// (the document's final shape), fitted and centred over white, so frames
/// taken before a crop or resize still line up.
fn normalized_frames<'a>(
    frames: &'a [Arc<[u8]>],
    max_side: u32,
) -> anyhow::Result<(u32, u32, impl Iterator<Item = anyhow::Result<RgbaImage>> + 'a)> {
    let last = frames.last().context("the timelapse has no frames")?;
    let last = image::load_from_memory(last)?;
    let (cw, ch) = fit(last.width(), last.height(), max_side);
    let iter = frames.iter().map(move |f| {
        let img = image::load_from_memory(f)?.to_rgba8();
        // Fit inside the canvas keeping the frame's own aspect.
        let s = (cw as f64 / img.width().max(1) as f64).min(ch as f64 / img.height().max(1) as f64);
        let fw = ((img.width() as f64 * s).round() as u32).clamp(1, cw);
        let fh = ((img.height() as f64 * s).round() as u32).clamp(1, ch);
        let img = if (fw, fh) != img.dimensions() {
            image::imageops::resize(&img, fw, fh, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let mut canvas = RgbaImage::from_pixel(cw, ch, image::Rgba([255, 255, 255, 255]));
        let (ox, oy) = ((cw - fw.min(cw)) / 2, (ch - fh.min(ch)) / 2);
        for (x, y, p) in img.enumerate_pixels() {
            let (dx, dy) = (x + ox, y + oy);
            if dx >= cw || dy >= ch {
                continue;
            }
            let a = p[3] as u32;
            let d = canvas.get_pixel_mut(dx, dy);
            for c in 0..3 {
                d[c] = ((p[c] as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
            }
        }
        Ok(canvas)
    });
    Ok((cw, ch, iter))
}

/// Write the frames as a looping animated GIF at `fps`, holding the final
/// picture for `hold_ms`. `done` counts frames as they are encoded.
pub fn export_gif(
    frames: &[Arc<[u8]>],
    path: &Path,
    fps: u32,
    max_side: u32,
    hold_ms: u32,
    done: &AtomicUsize,
) -> anyhow::Result<()> {
    let (_, _, iter) = normalized_frames(frames, max_side)?;
    let file = std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut enc = GifEncoder::new_with_speed(BufWriter::new(file), 10);
    enc.set_repeat(Repeat::Infinite)?;
    let n = frames.len();
    let step = Delay::from_numer_denom_ms(1000, fps.clamp(1, 60));
    for (i, img) in iter.enumerate() {
        let delay = if i + 1 == n { Delay::from_numer_denom_ms(hold_ms.max(10), 1) } else { step };
        enc.encode_frame(Frame::from_parts(img?, 0, 0, delay))?;
        done.store(i + 1, Ordering::Relaxed);
    }
    Ok(())
}

/// Write the frames as `frame_000001.png`… into `dir` (for a video editor or
/// ffmpeg). Returns how many were written.
pub fn export_frames(frames: &[Arc<[u8]>], dir: &Path, max_side: u32, done: &AtomicUsize) -> anyhow::Result<usize> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let (_, _, iter) = normalized_frames(frames, max_side)?;
    let mut count = 0;
    for (i, img) in iter.enumerate() {
        let img = img?;
        let path = dir.join(format!("frame_{:06}.png", i + 1));
        img.save(&path).with_context(|| format!("writing {}", path.display()))?;
        count += 1;
        done.store(count, Ordering::Relaxed);
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(v: u8) -> Arc<[u8]> {
        encode(2, 2, &[v; 16]).unwrap().into()
    }

    #[test]
    fn due_respects_the_interval() {
        let mut t = Timelapse { recording: true, ..Default::default() };
        assert!(t.due(0));
        t.last_edits = Some(5);
        assert!(!t.due(5));
        assert!(t.due(6));
        t.every = 3;
        assert!(!t.due(7));
        assert!(t.due(8));
        t.recording = false;
        assert!(!t.due(100));
    }

    #[test]
    fn thinning_keeps_first_and_last_and_doubles_interval() {
        let mut t = Timelapse::default();
        for i in 0..=MAX_FRAMES {
            t.push(frame((i % 250) as u8));
        }
        assert!(t.frames.len() <= MAX_FRAMES / 2 + 2);
        assert_eq!(t.every, 2);
        assert_eq!(&*t.frames[0], &*frame(0));
        assert_eq!(&*t.frames[t.frames.len() - 1], &*frame((MAX_FRAMES % 250) as u8));
    }

    #[test]
    fn gif_export_writes_a_file() {
        let frames: Vec<Arc<[u8]>> = (0..3).map(|i| frame(80 * i as u8)).collect();
        let dir = std::env::temp_dir().join(format!("qsk-tl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.gif");
        let done = AtomicUsize::new(0);
        export_gif(&frames, &path, 12, 64, 1000, &done).unwrap();
        assert_eq!(done.load(Ordering::Relaxed), 3);
        let decoded = image::open(&path).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
        let n = export_frames(&frames, &dir.join("frames"), 64, &done).unwrap();
        assert_eq!(n, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
