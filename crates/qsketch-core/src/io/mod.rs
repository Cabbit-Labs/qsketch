//! File formats: the native `.qsk` container and flat image import/export.

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

/// Layered formats we can open and save: native `.qsk` and Photoshop `.psd`.
pub const DOCUMENT_EXTENSIONS: &[&str] = &[NATIVE_EXTENSION, psd::EXTENSION];

/// Save a layered document in the format implied by the extension.
pub fn save_document(path: &Path, doc: &crate::document::DocState) -> anyhow::Result<()> {
    if psd::is_psd(path) {
        psd::save(path, doc)
    } else {
        qsk::save(path, doc)
    }
}

pub fn is_document(path: &Path) -> bool {
    is_native(path) || psd::is_psd(path)
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
    if is_native(path) {
        qsk::load(path)
    } else if psd::is_psd(path) {
        psd::load(path)
    } else {
        let raster = image_io::import(path)?;
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Background").to_string();
        Ok(crate::document::DocState::from_raster(name, raster))
    }
}
