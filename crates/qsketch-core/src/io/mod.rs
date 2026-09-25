//! File formats: the native `.qsk` container and flat image import/export.

pub mod ase;
pub mod brush_formats;
pub mod image_io;
pub mod psd;
pub mod qsk;

use std::path::Path;

/// Extensions we can open as a flat image.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "tga", "webp", "gif", "tif", "tiff"];
/// Extensions we can export a flattened image to.
pub const EXPORT_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "tga", "webp", "tif", "tiff"];
pub const NATIVE_EXTENSION: &str = "qsk";

/// Layered formats we can open and save: native `.qsk`, Aseprite
/// `.ase`/`.aseprite` and Photoshop `.psd`.
pub const DOCUMENT_EXTENSIONS: &[&str] = &[NATIVE_EXTENSION, "ase", "aseprite", psd::EXTENSION];

/// Save a layered document in the format implied by the extension.
pub fn save_document(path: &Path, doc: &crate::document::DocState) -> anyhow::Result<()> {
    if psd::is_psd(path) {
        psd::save(path, doc)
    } else if ase::is_ase(path) {
        ase::save(path, doc)
    } else {
        qsk::save(path, doc)
    }
}

/// Save in whatever format the extension names: a layered document format,
/// or a flat image (the document is flattened into the file).
pub fn save_any(path: &Path, doc: &crate::document::DocState) -> anyhow::Result<()> {
    if is_document(path) {
        save_document(path, doc)
    } else if is_export_image(path) {
        image_io::export(path, doc)
    } else {
        anyhow::bail!("unknown file format for {}", path.display())
    }
}

/// Whether `path` names a format `save_any` can write.
pub fn can_save(path: &Path) -> bool {
    is_document(path) || is_export_image(path)
}

pub fn is_document(path: &Path) -> bool {
    is_native(path) || psd::is_psd(path) || ase::is_ase(path)
}

/// A flat image format we can write (`EXPORT_EXTENSIONS`).
pub fn is_export_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXPORT_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Human-readable format name for a path's extension.
pub fn format_name(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "qsk" => "qsketch document".into(),
        "ase" | "aseprite" => "Aseprite".into(),
        "psd" => "Photoshop".into(),
        "png" => "PNG".into(),
        "jpg" | "jpeg" => "JPEG".into(),
        "webp" => "WebP".into(),
        "bmp" => "BMP".into(),
        "tga" => "TGA".into(),
        "tif" | "tiff" => "TIFF".into(),
        "gif" => "GIF".into(),
        other => other.to_ascii_uppercase(),
    }
}

/// What saving `doc` to `path` would lose, one sentence each; empty when the
/// format keeps everything (the native `.qsk`).
pub fn compat_warnings(path: &Path, doc: &crate::document::DocState) -> Vec<String> {
    let mut w = Vec::new();
    if is_native(path) {
        return w;
    }
    let name = format_name(path);
    if is_export_image(path) {
        let layered = doc.layers.len() > 1 || doc.layers.iter().any(|l| l.is_group());
        if layered {
            w.push(format!(
                "{name} can't store layers: the file will hold the flattened image. The layers stay in the open document."
            ));
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if matches!(ext.as_str(), "jpg" | "jpeg" | "bmp") {
            w.push(format!("{name} has no transparency: transparent areas are filled with white."));
        }
        if doc.selection.is_some() {
            w.push("The selection isn't stored.".into());
        }
        return w;
    }
    if ase::is_ase(path) && doc.layers.iter().any(|l| l.mask.is_some()) {
        w.push("Aseprite has no layer masks: each mask is applied to its layer's pixels in the file.".into());
    }
    if psd::is_psd(path) {
        if doc.selection.is_some() {
            w.push("The selection isn't stored in Photoshop files.".into());
        }
        if doc.layers.iter().any(|l| l.is_group()) {
            w.push(
                "Layer groups are written as their member layers; folder opacity and blend modes aren't kept.".into(),
            );
        }
        return w;
    }
    if ase::is_ase(path) {
        return ase::compat_warnings(doc);
    }
    w
}

pub fn is_native(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(NATIVE_EXTENSION))
}

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Open any supported file as a document state.
pub fn open(path: &Path) -> anyhow::Result<crate::document::DocState> {
    Ok(open_with_warnings(path)?.0)
}

/// Open any supported file, with notes about what the import could not
/// carry over (dropped animation frames, unsupported layer kinds).
pub fn open_with_warnings(path: &Path) -> anyhow::Result<(crate::document::DocState, Vec<String>)> {
    if is_native(path) {
        Ok((qsk::load(path)?, Vec::new()))
    } else if psd::is_psd(path) {
        Ok((psd::load(path)?, Vec::new()))
    } else if ase::is_ase(path) {
        ase::load_with_warnings(path)
    } else {
        let raster = image_io::import(path)?;
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Background").to_string();
        Ok((crate::document::DocState::from_raster(name, raster), Vec::new()))
    }
}
