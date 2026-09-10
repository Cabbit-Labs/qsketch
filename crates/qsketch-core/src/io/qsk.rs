//! The native `.qsk` format: a ZIP container holding `manifest.json`, one PNG
//! per layer and an optional selection mask. Plain PNGs keep the format
//! inspectable and recoverable with ordinary tools.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::document::DocState;
use crate::layer::{Layer, LayerProps};
use crate::mask::Mask;
use crate::raster::Raster;

pub const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    app_version: String,
    width: u32,
    height: u32,
    active: usize,
    next_layer_id: u64,
    layers: Vec<LayerEntry>,
    #[serde(default)]
    selection: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct LayerEntry {
    #[serde(flatten)]
    props: LayerProps,
    file: String,
}

pub fn save(path: &Path, doc: &DocState) -> anyhow::Result<()> {
    let tmp = path.with_extension("qsk.tmp");
    {
        let file = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        let mut zip = ZipWriter::new(BufWriter::new(file));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        let mut entries = Vec::with_capacity(doc.layers.len());
        for (i, layer) in doc.layers.iter().enumerate() {
            let file = format!("layers/{i:03}.png");
            let png = super::image_io::encode_png(doc.width, doc.height, &layer.raster.to_rgba())?;
            // PNG is already compressed; store it.
            zip.start_file(&file, stored)?;
            zip.write_all(&png)?;
            entries.push(LayerEntry { props: layer.props.clone(), file });
        }
        let selection = match &doc.selection {
            Some(m) if !m.is_empty() => {
                let name = "selection.png".to_string();
                let gray = m.to_gray();
                let img = image::GrayImage::from_raw(doc.width, doc.height, gray.to_vec())
                    .ok_or_else(|| anyhow!("bad mask size"))?;
                let mut buf = std::io::Cursor::new(Vec::new());
                img.write_to(&mut buf, image::ImageFormat::Png)?;
                zip.start_file(&name, stored)?;
                zip.write_all(&buf.into_inner())?;
                Some(name)
            }
            _ => None,
        };
        let manifest = Manifest {
            format: "qsketch".into(),
            version: FORMAT_VERSION,
            app_version: crate::VERSION.into(),
            width: doc.width,
            height: doc.height,
            active: doc.active,
            next_layer_id: doc.next_layer_id,
            layers: entries,
            selection,
        };
        zip.start_file("manifest.json", deflated)?;
        zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
        // Flattened preview for thumbnails / quick look.
        let flat = crate::composite::flatten(doc);
        let preview = super::image_io::encode_png(doc.width, doc.height, &flat.to_rgba())?;
        zip.start_file("preview.png", stored)?;
        zip.write_all(&preview)?;
        zip.finish()?.flush()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

pub fn load(path: &Path) -> anyhow::Result<DocState> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut zip = ZipArchive::new(BufReader::new(file))?;
    let manifest: Manifest = {
        let mut f = zip.by_name("manifest.json").context("missing manifest.json")?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        serde_json::from_str(&s).context("parsing manifest.json")?
    };
    if manifest.format != "qsketch" {
        return Err(anyhow!("not a qsketch file"));
    }
    if manifest.version > FORMAT_VERSION {
        return Err(anyhow!("file was saved by a newer qsketch (format v{})", manifest.version));
    }
    let (w, h) = (manifest.width, manifest.height);
    let mut layers = Vec::with_capacity(manifest.layers.len());
    for entry in manifest.layers {
        let mut bytes = Vec::new();
        zip.by_name(&entry.file).with_context(|| format!("missing {}", entry.file))?.read_to_end(&mut bytes)?;
        let raster = super::image_io::decode_bytes(&bytes)?;
        let raster =
            if raster.width() != w || raster.height() != h { raster.with_canvas_size(w, h, 0, 0) } else { raster };
        layers.push(Layer { props: entry.props, raster });
    }
    if layers.is_empty() {
        layers.push(Layer::new(1, "Background", w, h));
    }
    let selection = match manifest.selection {
        Some(name) => {
            let mut bytes = Vec::new();
            zip.by_name(&name)?.read_to_end(&mut bytes)?;
            let img = image::load_from_memory(&bytes)?.to_luma8();
            if img.dimensions() == (w, h) {
                Some(Arc::new(Mask::from_gray(w, h, img.into_raw())))
            } else {
                None
            }
        }
        None => None,
    };
    let next_layer_id = manifest.next_layer_id.max(layers.iter().map(|l| l.props.id).max().unwrap_or(0) + 1);
    Ok(DocState {
        width: w,
        height: h,
        active: manifest.active.min(layers.len() - 1),
        layers,
        selection,
        next_layer_id,
    })
}

/// Read just the preview PNG bytes of a `.qsk` (for recent-file thumbnails).
pub fn read_preview(path: &Path) -> anyhow::Result<Raster> {
    let file = File::open(path)?;
    let mut zip = ZipArchive::new(BufReader::new(file))?;
    let mut bytes = Vec::new();
    zip.by_name("preview.png")?.read_to_end(&mut bytes)?;
    super::image_io::decode_bytes(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;
    use crate::geom::IRect;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("qsketch-qsk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.qsk");
        let mut doc = DocState::new(70, 40, Some(Rgba8::WHITE));
        let id = doc.add_layer("Ink", None);
        let i = doc.index_of(id).unwrap();
        doc.layers[i].raster.set_pixel(3, 4, Rgba8::new(1, 2, 3, 200));
        doc.layers[i].props.opacity = 0.5;
        doc.layers[i].props.blend = crate::BlendMode::Multiply;
        doc.selection = Some(Arc::new(Mask::from_rect(70, 40, IRect::new(1, 1, 5, 5))));
        save(&p, &doc).unwrap();
        let back = load(&p).unwrap();
        assert_eq!((back.width, back.height), (70, 40));
        assert_eq!(back.layers.len(), 2);
        assert_eq!(back.layers[1].props.name, "Ink");
        assert_eq!(back.layers[1].props.blend, crate::BlendMode::Multiply);
        assert_eq!(back.layers[1].raster.get_pixel(3, 4), Rgba8::new(1, 2, 3, 200));
        assert_eq!(back.layers[0].raster.get_pixel(0, 0), Rgba8::WHITE);
        assert_eq!(back.selection.unwrap().bounds(), IRect::new(1, 1, 5, 5));
        assert_eq!(back.active, 1);
        assert!(read_preview(&p).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }
}
