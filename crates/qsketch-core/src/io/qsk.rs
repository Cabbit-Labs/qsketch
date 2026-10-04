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

use crate::document::{DocMeta, DocState, DocStats, Guide, LoadedMeta};
use crate::geom::IRect;
use crate::layer::{Layer, LayerProps};
use crate::mask::Mask;
use crate::palette::Palette;
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
    #[serde(default)]
    palette: Option<Palette>,
    #[serde(default)]
    palette_lock: bool,
    /// Lifetime statistics (0.47+): creation time, work time, edit and save
    /// counts. Optional; older files simply have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stats: Option<DocStats>,
    /// Ruler guides (0.49+). Optional.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    guides: Vec<Guide>,
    /// Tilesets of tilemap layers (0.57+): one PNG strip each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tilesets: Vec<TilesetInfo>,
    /// Slices (0.56+).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    slices: Vec<crate::slice::Slice>,
    /// Pixel aspect ratio (0.54+), width : height; absent = square.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pixel_aspect: Option<[u8; 2]>,
    /// Timelapse (0.51+): frames live in `timelapse/NNNNNN.png`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timelapse: Option<TimelapseInfo>,
    /// Animation (0.59+): frame durations, the frame being shown and tags.
    /// Absent = a single frame; a layer's `file` is then its only picture.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    frames: Vec<crate::anim::Frame>,
    #[serde(default)]
    frame: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<crate::anim::Tag>,
}

/// One cel of a layer in a `.qsk` with several frames.
#[derive(Serialize, Deserialize)]
struct CelEntry {
    /// `layers/NNN/fFFFF.png`; absent for an empty or linked cel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    /// Frame whose cel this one shares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    link: Option<usize>,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    opacity: f32,
    #[serde(default, skip_serializing_if = "is_zero_i16")]
    z_index: i16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outside: Option<OutsideEntry>,
}

/// Pixels a layer keeps outside the canvas (0.62+): a PNG of their bounding
/// box, with its top-left at `(x, y)` in canvas coordinates. Older versions
/// ignore it and open the on-canvas part.
#[derive(Serialize, Deserialize)]
struct OutsideEntry {
    file: String,
    x: i32,
    y: i32,
}

fn one() -> f32 {
    1.0
}
fn is_one(v: &f32) -> bool {
    (*v - 1.0).abs() < 1e-6
}
fn is_zero_i16(v: &i16) -> bool {
    *v == 0
}

#[derive(Serialize, Deserialize)]
struct TilesetInfo {
    name: String,
    tile_w: u32,
    tile_h: u32,
    count: u32,
    /// PNG with the tiles side by side, tile 0 (empty) first.
    file: String,
}

#[derive(Serialize, Deserialize)]
struct TimelapseInfo {
    frames: usize,
    recording: bool,
    every: u32,
}

fn timelapse_entry(i: usize) -> String {
    format!("timelapse/{i:06}.png")
}

#[derive(Serialize, Deserialize)]
struct LayerEntry {
    #[serde(flatten)]
    props: LayerProps,
    file: String,
    /// Grayscale PNG of the layer mask, when the layer has one.
    #[serde(default)]
    mask: Option<String>,
    /// Animation: one entry per frame (0.59+). `file` above is frame 0's
    /// picture, so older versions still open the first frame.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cels: Vec<CelEntry>,
    /// Frame 0's off-canvas pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outside: Option<OutsideEntry>,
    /// The mask's off-canvas coverage (grayscale PNG).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mask_outside: Option<OutsideEntry>,
    /// Smart objects (0.65+): the original pixels, at their own size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    smart_source: Option<String>,
}

/// Store `r`'s off-canvas pixels as `name`, if it has any.
fn write_outside<W: Write + std::io::Seek>(
    zip: &mut ZipWriter<W>,
    opts: SimpleFileOptions,
    name: String,
    r: &Raster,
) -> anyhow::Result<Option<OutsideEntry>> {
    let Some((rect, img)) = r.outside_image() else { return Ok(None) };
    zip.start_file(&name, opts)?;
    zip.write_all(&super::image_io::encode_png(img.width(), img.height(), &img.to_rgba())?)?;
    Ok(Some(OutsideEntry { file: name, x: rect.x, y: rect.y }))
}

/// Put saved off-canvas pixels back on `r` (a missing or bad entry is skipped).
fn read_outside(zip: &mut ZipArchive<BufReader<File>>, e: &OutsideEntry, r: &mut Raster) {
    let mut bytes = Vec::new();
    let Ok(mut f) = zip.by_name(&e.file) else { return };
    if f.read_to_end(&mut bytes).is_err() {
        return;
    }
    drop(f);
    if let Ok(img) = super::image_io::decode_bytes(&bytes) {
        r.restore_outside(e.x, e.y, &img);
    }
}

fn gray_png(w: u32, h: u32, gray: &[u8]) -> anyhow::Result<Vec<u8>> {
    let img = image::GrayImage::from_raw(w, h, gray.to_vec()).ok_or_else(|| anyhow!("bad mask size"))?;
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}

fn read_gray(zip: &mut ZipArchive<BufReader<File>>, name: &str, w: u32, h: u32) -> Option<Mask> {
    let mut bytes = Vec::new();
    zip.by_name(name).ok()?.read_to_end(&mut bytes).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_luma8();
    (img.dimensions() == (w, h)).then(|| Mask::from_gray(w, h, img.into_raw()))
}

pub fn save(path: &Path, doc: &DocState) -> anyhow::Result<()> {
    save_with_meta(path, doc, DocMeta::default())
}

/// Save with what the document keeps beside its layers: statistics, ruler
/// guides and the timelapse (see [`DocMeta`]).
pub fn save_with_meta(path: &Path, doc: &DocState, meta: DocMeta<'_>) -> anyhow::Result<()> {
    // Cels up to date with the current frame's picture (shared tiles: cheap).
    let mut synced = doc.clone();
    synced.sync_cels();
    let doc = &synced;
    let nframes = doc.frame_count();
    let tmp = path.with_extension("qsk.tmp");
    {
        let file = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        let mut zip = ZipWriter::new(BufWriter::new(file));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        let mut entries = Vec::with_capacity(doc.layers.len());
        for (i, layer) in doc.layers.iter().enumerate() {
            let file = format!("layers/{i:03}.png");
            let first = doc.cel_image(i, 0).cloned().unwrap_or_else(|| Raster::new(doc.width, doc.height));
            let png = super::image_io::encode_png(doc.width, doc.height, &first.to_rgba())?;
            // PNG is already compressed; store it.
            zip.start_file(&file, stored)?;
            zip.write_all(&png)?;
            // Other frames' cels: one PNG per owned picture.
            let mut cels = Vec::new();
            if nframes > 1 && layer.animated() && layer.cels.len() == nframes {
                for (f, c) in layer.cels.iter().enumerate() {
                    let mut entry =
                        CelEntry { file: None, link: c.link, opacity: c.opacity, z_index: c.z_index, outside: None };
                    if c.link.is_none() && f > 0 && (!c.image.is_empty() || c.image.has_outside()) {
                        let name = format!("layers/{i:03}/f{f:04}.png");
                        zip.start_file(&name, stored)?;
                        zip.write_all(&super::image_io::encode_png(doc.width, doc.height, &c.image.to_rgba())?)?;
                        entry.file = Some(name);
                        entry.outside =
                            write_outside(&mut zip, stored, format!("layers/{i:03}/f{f:04}.outside.png"), &c.image)?;
                    } else if c.link.is_none() && f == 0 {
                        entry.file = Some(file.clone());
                    }
                    cels.push(entry);
                }
            }
            let outside = write_outside(&mut zip, stored, format!("layers/{i:03}.outside.png"), &first)?;
            let mut mask_outside = None;
            let mask = match &layer.mask {
                Some(m) => {
                    let name = format!("layers/{i:03}.mask.png");
                    zip.start_file(&name, stored)?;
                    zip.write_all(&gray_png(doc.width, doc.height, m.to_gray())?)?;
                    if let Some((r, gray)) = m.outside_gray() {
                        let name = format!("layers/{i:03}.mask.outside.png");
                        zip.start_file(&name, stored)?;
                        zip.write_all(&gray_png(r.w as u32, r.h as u32, gray)?)?;
                        mask_outside = Some(OutsideEntry { file: name, x: r.x, y: r.y });
                    }
                    Some(name)
                }
                None => None,
            };
            let smart = match &layer.smart {
                Some(src) if layer.props.smart.is_some() => {
                    let name = format!("layers/{i:03}.smart.png");
                    zip.start_file(&name, stored)?;
                    zip.write_all(&super::image_io::encode_png(src.width(), src.height(), &src.to_rgba())?)?;
                    Some(name)
                }
                _ => None,
            };
            entries.push(LayerEntry {
                props: layer.props.clone(),
                file,
                mask,
                cels,
                outside,
                mask_outside,
                smart_source: smart,
            });
        }
        let selection = match &doc.selection {
            Some(m) if !m.is_empty() => {
                let name = "selection.png".to_string();
                zip.start_file(&name, stored)?;
                zip.write_all(&gray_png(doc.width, doc.height, m.to_gray())?)?;
                Some(name)
            }
            _ => None,
        };
        // Tilesets: the tiles of each side by side in one PNG.
        let mut tileset_infos = Vec::with_capacity(doc.tilesets.len());
        for (i, ts) in doc.tilesets.iter().enumerate() {
            let n = ts.tiles.len().max(1) as u32;
            let mut strip = Raster::new(ts.tile_w * n, ts.tile_h);
            for (k, t) in ts.tiles.iter().enumerate() {
                strip.blit(t, (k as u32 * ts.tile_w) as i32, 0, false);
            }
            let file = format!("tilesets/{i:03}.png");
            zip.start_file(&file, stored)?;
            zip.write_all(&super::image_io::encode_png(strip.width(), strip.height(), &strip.to_rgba())?)?;
            tileset_infos.push(TilesetInfo {
                name: ts.name.clone(),
                tile_w: ts.tile_w,
                tile_h: ts.tile_h,
                count: n,
                file,
            });
        }
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
            palette: (!doc.palette.is_empty()).then(|| doc.palette.clone()),
            palette_lock: doc.palette_lock,
            pixel_aspect: (doc.pixel_aspect != [1, 1]).then_some(doc.pixel_aspect),
            slices: doc.slices.clone(),
            tilesets: tileset_infos,
            stats: meta.stats.cloned(),
            guides: meta.guides.to_vec(),
            timelapse: meta.timelapse.filter(|t| t.recording || !t.frames.is_empty()).map(|t| TimelapseInfo {
                frames: t.frames.len(),
                recording: t.recording,
                every: t.every,
            }),
            frames: if nframes > 1 { doc.frames.clone() } else { Vec::new() },
            frame: doc.frame,
            tags: doc.tags.clone(),
        };
        if let Some(t) = meta.timelapse {
            for (i, frame) in t.frames.iter().enumerate() {
                zip.start_file(timelapse_entry(i), stored)?;
                zip.write_all(frame)?;
            }
        }
        zip.start_file("manifest.json", deflated)?;
        zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
        // Flattened preview for thumbnails / quick look (the first frame).
        let flat = crate::anim::render_frame(doc, 0);
        let preview = super::image_io::encode_png(doc.width, doc.height, &flat.to_rgba())?;
        zip.start_file("preview.png", stored)?;
        zip.write_all(&preview)?;
        zip.finish()?.flush()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

pub fn load(path: &Path) -> anyhow::Result<DocState> {
    Ok(load_with_meta(path)?.0)
}

/// Load a document with what the file stores beside its layers (defaults
/// for anything it predates).
pub fn load_with_meta(path: &Path) -> anyhow::Result<(DocState, LoadedMeta)> {
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
    let nframes = manifest.frames.len().max(1);
    let mut layers = Vec::with_capacity(manifest.layers.len());
    let read_raster = |zip: &mut ZipArchive<BufReader<File>>, name: &str| -> anyhow::Result<Raster> {
        let mut bytes = Vec::new();
        zip.by_name(name).with_context(|| format!("missing {name}"))?.read_to_end(&mut bytes)?;
        let raster = super::image_io::decode_bytes(&bytes)?;
        Ok(if raster.width() != w || raster.height() != h { raster.with_canvas_size(w, h, 0, 0) } else { raster })
    };
    for entry in manifest.layers {
        let mut raster = read_raster(&mut zip, &entry.file)?;
        if let Some(o) = &entry.outside {
            read_outside(&mut zip, o, &mut raster);
        }
        let mut mask = entry.mask.as_deref().and_then(|name| read_gray(&mut zip, name, w, h));
        if let (Some(m), Some(o)) = (mask.as_mut(), &entry.mask_outside) {
            let mut bytes = Vec::new();
            if let Ok(mut f) = zip.by_name(&o.file) {
                if f.read_to_end(&mut bytes).is_ok() {
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        let img = img.to_luma8();
                        let r = IRect::new(o.x, o.y, img.width() as i32, img.height() as i32);
                        m.restore_outside(r, img.into_raw());
                    }
                }
            }
        }
        let mask = mask.map(Arc::new);
        let mut cels = Vec::new();
        if nframes > 1 && entry.props.kind == crate::layer::LayerKind::Raster {
            for f in 0..nframes {
                let c = match entry.cels.get(f) {
                    Some(c) if c.link.is_some() => crate::anim::Cel::linked(c.link.unwrap_or(0), w, h),
                    Some(c) => {
                        let image = match (f, &c.file) {
                            (0, _) => raster.clone(),
                            (_, Some(name)) => {
                                let mut r = read_raster(&mut zip, name).unwrap_or_else(|_| Raster::new(w, h));
                                if let Some(o) = &c.outside {
                                    read_outside(&mut zip, o, &mut r);
                                }
                                r
                            }
                            (_, None) => Raster::new(w, h),
                        };
                        crate::anim::Cel { image, link: None, opacity: c.opacity, z_index: c.z_index }
                    }
                    // A layer saved without cels (never animated): its one
                    // picture in frame 0, nothing elsewhere.
                    None if f == 0 => crate::anim::Cel::own(raster.clone()),
                    None => crate::anim::Cel::empty(w, h),
                };
                cels.push(c);
            }
        }
        // A smart object whose originals are missing falls back to pixels.
        let smart = entry.smart_source.as_deref().and_then(|name| {
            let mut bytes = Vec::new();
            zip.by_name(name).ok()?.read_to_end(&mut bytes).ok()?;
            super::image_io::decode_bytes(&bytes).ok().map(Arc::new)
        });
        let mut props = entry.props;
        if props.kind == crate::layer::LayerKind::Smart && (smart.is_none() || props.smart.is_none()) {
            props.kind = crate::layer::LayerKind::Raster;
            props.smart = None;
        }
        let smart = smart.filter(|_| props.kind == crate::layer::LayerKind::Smart);
        layers.push(Layer { props, raster, mask, cels, smart });
    }
    if layers.is_empty() {
        layers.push(Layer::new(1, "Background", w, h));
    }
    let selection = manifest.selection.as_deref().and_then(|name| read_gray(&mut zip, name, w, h)).map(Arc::new);
    let next_layer_id = manifest.next_layer_id.max(layers.iter().map(|l| l.props.id).max().unwrap_or(0) + 1);
    let mut tilesets = Vec::with_capacity(manifest.tilesets.len());
    for info in &manifest.tilesets {
        let mut ts = crate::tilemap::Tileset::new(info.name.clone(), info.tile_w, info.tile_h);
        ts.tiles.clear();
        let mut bytes = Vec::new();
        if let Ok(mut f) = zip.by_name(&info.file) {
            f.read_to_end(&mut bytes)?;
        }
        let strip = super::image_io::decode_bytes(&bytes).unwrap_or_else(|_| Raster::new(1, 1));
        for k in 0..info.count.max(1) {
            ts.tiles.push(strip.crop(crate::geom::IRect::new(
                (k * info.tile_w) as i32,
                0,
                info.tile_w as i32,
                info.tile_h as i32,
            )));
        }
        tilesets.push(ts);
    }
    // A tilemap pointing at a missing tileset is plain pixels.
    for l in &mut layers {
        if l.props.tilemap.as_ref().is_some_and(|t| t.tileset >= tilesets.len()) {
            l.props.tilemap = None;
            l.props.kind = crate::layer::LayerKind::Raster;
        }
    }
    let mut doc = DocState {
        width: w,
        height: h,
        active: manifest.active.min(layers.len() - 1),
        layers,
        selection,
        next_layer_id,
        palette: manifest.palette.unwrap_or_default(),
        palette_lock: manifest.palette_lock,
        pixel_aspect: manifest.pixel_aspect.unwrap_or([1, 1]),
        slices: manifest.slices,
        tilesets,
        frames: if manifest.frames.is_empty() { vec![crate::anim::Frame::default()] } else { manifest.frames },
        frame: manifest.frame,
        tags: manifest.tags,
    };
    doc.repair_groups();
    doc.repair_animation();
    doc.load_cels();
    let mut timelapse = crate::timelapse::Timelapse::default();
    if let Some(info) = manifest.timelapse {
        timelapse.recording = info.recording;
        timelapse.every = info.every.max(1);
        for i in 0..info.frames {
            let mut bytes = Vec::new();
            if let Ok(mut f) = zip.by_name(&timelapse_entry(i)) {
                if f.read_to_end(&mut bytes).is_ok() {
                    timelapse.frames.push(bytes.into());
                }
            }
        }
    }
    let meta = LoadedMeta { stats: manifest.stats.unwrap_or_default(), guides: manifest.guides, timelapse };
    Ok((doc, meta))
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

    #[test]
    fn tilemap_round_trips() {
        let mut doc = DocState::new(16, 8, None);
        for y in 0..4 {
            for x in 0..4 {
                doc.layers[0].raster.set_pixel(x, y, crate::color::Rgba8::new(255, 0, 0, 255));
                doc.layers[0].raster.set_pixel(x + 8, y + 4, crate::color::Rgba8::new(255, 0, 0, 255));
            }
        }
        let (ts, tm) = crate::tilemap::from_raster(&doc.layers[0].raster, "t", 4, 4);
        doc.tilesets = vec![ts];
        doc.layers[0].props.kind = crate::layer::LayerKind::Tilemap;
        doc.layers[0].props.tilemap = Some(tm.clone());
        let dir = std::env::temp_dir().join(format!("qsk-tilemap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.qsk");
        save(&path, &doc).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(back.tilesets.len(), 1);
        assert_eq!(back.tilesets[0].tiles.len(), 2);
        assert_eq!(back.tilesets[0].tiles[1].get_pixel(2, 2), crate::color::Rgba8::new(255, 0, 0, 255));
        assert_eq!(back.layers[0].props.tilemap, Some(tm));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn animation_round_trips() {
        use crate::anim::NewFrame;
        let red = Rgba8::new(255, 0, 0, 255);
        let mut doc = DocState::new(8, 8, None);
        doc.layers[0].raster.set_pixel(0, 0, red);
        doc.insert_frame(1, NewFrame::Duplicate(0));
        doc.link_cels(0, &[0, 1]);
        doc.insert_frame(2, NewFrame::Empty);
        doc.layers[0].raster.set_pixel(2, 2, red);
        doc.layers[0].cels[2].opacity = 0.25;
        doc.frames[2].duration_ms = 40;
        doc.add_tag("t", 1, 2);
        doc.set_frame(1);
        let dir = std::env::temp_dir().join(format!("qsk-anim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.qsk");
        save(&path, &doc).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(back.frame_count(), 3);
        assert_eq!(back.frame, 1);
        assert_eq!(back.frames[2].duration_ms, 40);
        assert_eq!(back.tags[0].name, "t");
        assert_eq!(back.layers[0].cels[1].link, Some(0));
        assert_eq!(back.layers[0].raster.get_pixel(0, 0), red, "frame 1 shows frame 0's picture");
        assert_eq!(back.cel_image(0, 2).unwrap().get_pixel(2, 2), red);
        assert_eq!(back.cel_image(0, 2).unwrap().get_pixel(0, 0).a, 0);
        assert!((back.layers[0].cels[2].opacity - 0.25).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pixel_aspect_round_trips() {
        let mut doc = DocState::new(8, 8, None);
        doc.pixel_aspect = [2, 1];
        let dir = std::env::temp_dir().join(format!("qsk-aspect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.qsk");
        save(&path, &doc).unwrap();
        assert_eq!(load(&path).unwrap().pixel_aspect, [2, 1]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn layer_mask_round_trips() {
        let mut doc = DocState::new(8, 8, None);
        let mut m = Mask::full(8, 8);
        m.set(2, 3, 9);
        doc.layers[0].mask = Some(Arc::new(m));
        doc.layers[0].props.mask_enabled = false;
        let dir = std::env::temp_dir().join(format!("qsk-mask-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.qsk");
        save(&path, &doc).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(back.layers[0].mask.as_ref().unwrap().get(2, 3), 9);
        assert!(!back.layers[0].props.mask_enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }
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
        // Groups round-trip with their membership.
        let g = doc.group_layers(&[id]).unwrap();
        save(&p, &doc).unwrap();
        let back = load(&p).unwrap();
        let gi = back.index_of(g).unwrap();
        assert!(back.layers[gi].is_group());
        assert_eq!(back.members(gi), 1..2);
        assert_eq!(back.layers[1].props.parent, Some(g));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn offcanvas_pixels_and_mask_survive_a_save() {
        use crate::color::Rgba8;
        let mut doc = DocState::new(8, 8, None);
        doc.layers[0].raster.set_pixel(1, 1, Rgba8::WHITE);
        let mut m = Mask::full(8, 8);
        m.set(0, 0, 77);
        doc.layers[0].mask = Some(Arc::new(m));
        // Push it half off the top-left corner.
        let rest = crate::moving::begin(&doc, &[0]);
        crate::moving::apply(&mut doc, &rest, -3, -2);
        assert!(doc.layers[0].raster.has_outside());
        let dir = std::env::temp_dir().join(format!("qsk-outside-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("o.qsk");
        save(&path, &doc).unwrap();
        let mut back = load(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back.layers[0].raster.get_pixel_any(-2, -1), Rgba8::WHITE);
        assert_eq!(back.layers[0].mask.as_ref().unwrap().get_any(-3, -2), 77);
        let rest = crate::moving::begin(&back, &[0]);
        crate::moving::apply(&mut back, &rest, 3, 2);
        assert_eq!(back.layers[0].raster.get_pixel(1, 1), Rgba8::WHITE);
        assert_eq!(back.layers[0].mask.as_ref().unwrap().get(0, 0), 77);
    }

    #[test]
    fn smart_objects_survive_a_save() {
        use crate::color::Rgba8;
        let mut doc = DocState::new(16, 16, None);
        doc.layers[0].raster.fill_rect(crate::geom::IRect::new(2, 2, 6, 4), Rgba8::new(9, 8, 7, 255));
        assert!(crate::smart::convert(&mut doc, 0));
        doc.layers[0].props.smart.as_mut().unwrap().size = (3.0, 2.0);
        crate::smart::rerender(&mut doc, 0);
        let dir = std::env::temp_dir().join(format!("qsk-smart-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.qsk");
        save(&path, &doc).unwrap();
        let mut back = load(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(back.layers[0].is_smart());
        assert_eq!(back.layers[0].props.smart, doc.layers[0].props.smart);
        // Grown back to full size from the saved originals: nothing lost.
        back.layers[0].props.smart.as_mut().unwrap().size = (6.0, 4.0);
        crate::smart::rerender(&mut back, 0);
        assert_eq!(back.layers[0].raster.get_pixel(2, 2), Rgba8::new(9, 8, 7, 255));
        assert_eq!(back.layers[0].raster.get_pixel(7, 5), Rgba8::new(9, 8, 7, 255));
    }
}
