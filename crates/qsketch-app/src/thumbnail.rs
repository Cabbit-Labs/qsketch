//! `qsketch --thumbnail <in.qsk> <out.png> <size>`: write the preview stored
//! in a `.qsk`, scaled to fit `size`, as a PNG. This is the freedesktop
//! thumbnailer entry point (`share/thumbnailers/qsketch.thumbnailer`) that
//! Linux file managers run to draw a document's icon; on Windows the
//! `qsketch-thumb` shell extension does the same job inside Explorer.

use std::ffi::OsString;
use std::path::Path;

use anyhow::Context;

pub fn run(args: &[OsString]) -> anyhow::Result<()> {
    let [input, output, size] = args else {
        anyhow::bail!("usage: qsketch --thumbnail <in.qsk> <out.png> <size>");
    };
    let size: u32 = size.to_string_lossy().trim().parse().context("size must be a number of pixels")?;
    let size = size.max(1);
    let raster = qsketch_core::io::qsk::read_preview(Path::new(input))?;
    let (w, h) = (raster.width(), raster.height());
    let img = image::RgbaImage::from_raw(w, h, raster.to_rgba()).context("preview has an unexpected size")?;
    let longest = w.max(h);
    let img = if longest > size {
        let s = size as f32 / longest as f32;
        let tw = ((w as f32 * s).round() as u32).max(1);
        let th = ((h as f32 * s).round() as u32).max(1);
        image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let file = std::fs::File::create(output).with_context(|| format!("creating {}", Path::new(output).display()))?;
    let mut out = std::io::BufWriter::new(file);
    img.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(())
}
