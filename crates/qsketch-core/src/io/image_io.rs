//! Flat image import/export via the `image` crate.

use std::path::Path;

use anyhow::{anyhow, Context};
use image::{ImageBuffer, ImageFormat, Rgba};

use crate::composite::flatten;
use crate::document::DocState;
use crate::raster::Raster;

/// Load any supported image into a straight-alpha raster.
pub fn import(path: &Path) -> anyhow::Result<Raster> {
    let img = image::ImageReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("decoding {}", path.display()))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(Raster::from_rgba(w, h, rgba.as_raw()))
}

/// Decode an in-memory image (e.g. from the OS clipboard or drag-drop).
pub fn decode_bytes(bytes: &[u8]) -> anyhow::Result<Raster> {
    let img = image::load_from_memory(bytes)?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(Raster::from_rgba(w, h, rgba.as_raw()))
}

/// Encode a straight RGBA buffer as PNG bytes.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> anyhow::Result<Vec<u8>> {
    let img: ImageBuffer<Rgba<u8>, _> =
        ImageBuffer::from_raw(width, height, rgba.to_vec()).ok_or_else(|| anyhow!("bad buffer size"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
}

/// Export the flattened document. Formats without alpha are composited over white.
pub fn export(path: &Path, doc: &DocState) -> anyhow::Result<()> {
    let flat = flatten(doc);
    export_raster(path, &flat)
}

pub fn export_raster(path: &Path, raster: &Raster) -> anyhow::Result<()> {
    let format =
        ImageFormat::from_path(path).with_context(|| format!("unknown image format for {}", path.display()))?;
    let (w, h) = (raster.width(), raster.height());
    let rgba = raster.to_rgba();
    let has_alpha = matches!(
        format,
        ImageFormat::Png | ImageFormat::WebP | ImageFormat::Tiff | ImageFormat::Tga | ImageFormat::Gif
    );
    if has_alpha {
        let img: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(w, h, rgba).ok_or_else(|| anyhow!("bad buffer"))?;
        img.save_with_format(path, format)?;
    } else {
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for p in rgba.chunks_exact(4) {
            let a = p[3] as u32;
            for c in &p[..3] {
                rgb.push(((*c as u32 * a + 255 * (255 - a)) / 255) as u8);
            }
        }
        let img: ImageBuffer<image::Rgb<u8>, _> =
            ImageBuffer::from_raw(w, h, rgb).ok_or_else(|| anyhow!("bad buffer"))?;
        img.save_with_format(path, format)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    #[test]
    fn png_roundtrip() {
        let dir = std::env::temp_dir().join(format!("qsketch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.png");
        let mut doc = DocState::new(8, 8, None);
        doc.layers[0].raster.set_pixel(1, 1, Rgba8::new(10, 20, 30, 200));
        export(&p, &doc).unwrap();
        let r = import(&p).unwrap();
        assert_eq!(r.get_pixel(1, 1), Rgba8::new(10, 20, 30, 200));
        let j = dir.join("t.jpg");
        export(&j, &doc).unwrap();
        assert!(import(&j).unwrap().get_pixel(0, 0).a == 255);
        std::fs::remove_dir_all(&dir).ok();
    }
}
