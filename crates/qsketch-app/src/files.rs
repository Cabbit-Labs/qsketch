//! Open / save / export flows with native file dialogs.

use std::path::{Path, PathBuf};

use qsketch_core::io;
use qsketch_core::Document;

use crate::dialogs::{AfterClose, SaveConfirm};
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

fn image_filter_exts() -> Vec<&'static str> {
    io::IMAGE_EXTENSIONS.to_vec()
}

pub fn open_dialog(state: &mut AppState) {
    let mut all: Vec<&str> = io::DOCUMENT_EXTENSIONS.to_vec();
    all.extend(image_filter_exts());
    let dlg = rfd::FileDialog::new()
        .set_title("Open")
        .add_filter("All supported", &all)
        .add_filter("qsketch document", &[io::NATIVE_EXTENSION])
        .add_filter("Aseprite", io::ase::EXTENSIONS)
        .add_filter("Photoshop", &[io::psd::EXTENSION])
        .add_filter("Images", &image_filter_exts());
    let dlg = match state.settings.general.recent_files.first().and_then(|p| p.parent()) {
        Some(dir) if dir.exists() => dlg.set_directory(dir),
        _ => dlg,
    };
    if let Some(paths) = dlg.pick_files() {
        for p in paths {
            open_path(state, &p);
        }
    }
}

pub fn open_path(state: &mut AppState, path: &Path) -> Option<DocId> {
    // Already open? Focus it.
    if let Some(d) = state.docs.iter().find(|d| d.doc.path.as_deref() == Some(path)) {
        let id = d.id;
        state.active_doc = Some(id);
        return Some(id);
    }
    match io::open_with_warnings(path) {
        Ok((doc_state, warnings)) => {
            let title = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
            let doc = Document::from_state(doc_state, title, Some(path.to_path_buf()), "Open");
            let id = state.add_document(doc);
            state.settings.push_recent(path.to_path_buf());
            for w in warnings {
                state.toasts.push(Level::Info, w);
            }
            Some(id)
        }
        Err(e) => {
            state.toasts.push(Level::Error, format!("Couldn't open {}: {e:#}", path.display()));
            None
        }
    }
}

/// Save to the document's own path, or prompt if it has none / can't be
/// written. Returns true once the file is on disk; false when cancelled, or
/// when a format warning is up and the save continues from that dialog.
pub fn save(state: &mut AppState, doc_id: DocId) -> bool {
    save_then(state, doc_id, None)
}

/// `save`, with something to do after the write completes even when it goes
/// through the format-warning dialog (closing the document, quitting).
pub fn save_then(state: &mut AppState, doc_id: DocId, then: Option<AfterClose>) -> bool {
    let path = state.doc(doc_id).and_then(|d| d.doc.path.clone());
    match path {
        Some(p) if io::can_save(&p) => request_write(state, doc_id, &p, then),
        _ => save_as_then(state, doc_id, then),
    }
}

pub fn save_as(state: &mut AppState, doc_id: DocId) -> bool {
    save_as_then(state, doc_id, None)
}

pub fn save_as_then(state: &mut AppState, doc_id: DocId, then: Option<AfterClose>) -> bool {
    let Some(entry) = state.doc(doc_id) else { return false };
    let suggested = entry
        .doc
        .path
        .as_ref()
        .map(|p| if io::can_save(p) { p.clone() } else { p.with_extension(io::NATIVE_EXTENSION) })
        .unwrap_or_else(|| {
            PathBuf::from(format!("{}.{}", entry.doc.title.trim_end_matches('*'), io::NATIVE_EXTENSION))
        });
    let mut dlg = rfd::FileDialog::new()
        .set_title("Save As")
        .add_filter("qsketch document", &[io::NATIVE_EXTENSION])
        .add_filter("Aseprite (layers)", io::ase::EXTENSIONS)
        .add_filter("Photoshop (layers)", &[io::psd::EXTENSION])
        .add_filter("PNG (flattened)", &["png"])
        .add_filter("JPEG (flattened, no transparency)", &["jpg", "jpeg"])
        .add_filter("WebP (flattened)", &["webp"])
        .add_filter("BMP (flattened, no transparency)", &["bmp"])
        .add_filter("TGA (flattened)", &["tga"])
        .add_filter("TIFF (flattened)", &["tif", "tiff"]);
    if let Some(name) = suggested.file_name() {
        dlg = dlg.set_file_name(name.to_string_lossy());
    }
    if let Some(dir) = suggested.parent().filter(|d| d.exists() && !d.as_os_str().is_empty()) {
        dlg = dlg.set_directory(dir);
    }
    let Some(mut path) = dlg.save_file() else { return false };
    if path.extension().is_none() || !io::can_save(&path) {
        // Unknown or missing extension: keep what was typed and make it a
        // native document rather than guessing a format.
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        path.set_file_name(format!("{}.{}", name.trim_end_matches('.'), io::NATIVE_EXTENSION));
    }
    request_write(state, doc_id, &path, then)
}

/// Write now, or first put up the format-warning dialog when the format
/// can't hold everything and the user hasn't already accepted that for
/// this path.
fn request_write(state: &mut AppState, doc_id: DocId, path: &Path, then: Option<AfterClose>) -> bool {
    let Some(entry) = state.doc(doc_id) else { return false };
    let warnings = io::compat_warnings(path, entry.doc.state());
    if warnings.is_empty() || entry.format_ack.as_deref() == Some(path) {
        return write_document(state, doc_id, path);
    }
    state.dialogs.save_confirm = Some(SaveConfirm { doc: doc_id, path: path.to_path_buf(), warnings, then });
    false
}

/// Write the document to `path` in the format its extension names.
pub fn write_document(state: &mut AppState, doc_id: DocId, path: &Path) -> bool {
    if state.settings.general.backups {
        crate::backups::take(path, state.settings.general.backup_versions as usize);
    }
    let Some(entry) = state.doc_mut(doc_id) else { return false };
    match io::save_any(path, entry.doc.state()) {
        Ok(()) => {
            entry.doc.path = Some(path.to_path_buf());
            entry.doc.title = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            entry.doc.mark_saved();
            entry.format_ack = Some(path.to_path_buf());
            state.autosave.forget(doc_id);
            state.settings.push_recent(path.to_path_buf());
            state.toasts.push(Level::Success, format!("Saved {}", path.display()));
            true
        }
        Err(e) => {
            state.toasts.push(Level::Error, format!("Save failed: {e:#}"));
            false
        }
    }
}

pub fn export(state: &mut AppState, doc_id: DocId) -> bool {
    let Some(entry) = state.doc(doc_id) else { return false };
    let base = entry.doc.title.trim_end_matches('*').to_string();
    let stem = Path::new(&base).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(base);
    let mut dlg = rfd::FileDialog::new()
        .set_title("Export Image")
        .add_filter("PNG", &["png"])
        .add_filter("JPEG", &["jpg", "jpeg"])
        .add_filter("WebP", &["webp"])
        .add_filter("BMP", &["bmp"])
        .add_filter("TGA", &["tga"])
        .add_filter("TIFF", &["tif", "tiff"])
        .set_file_name(format!("{stem}.png"));
    if let Some(dir) = entry.doc.path.as_ref().and_then(|p| p.parent()).filter(|d| d.exists()) {
        dlg = dlg.set_directory(dir);
    }
    let Some(mut path) = dlg.save_file() else { return false };
    if path.extension().is_none() {
        path.set_extension("png");
    }
    match io::image_io::export(&path, entry.doc.state()) {
        Ok(()) => {
            state.toasts.push(Level::Success, format!("Exported {}", path.display()));
            true
        }
        Err(e) => {
            state.toasts.push(Level::Error, format!("Export failed: {e:#}"));
            false
        }
    }
}

/// Export the flattened image scaled up by an integer factor with
/// nearest-neighbor sampling, so pixel art stays crisp.
pub fn export_scaled(state: &mut AppState, doc_id: DocId, k: u32) -> bool {
    let Some(entry) = state.doc(doc_id) else { return false };
    let base = entry.doc.title.trim_end_matches('*').to_string();
    let stem = Path::new(&base).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(base);
    let (w, h) = (entry.doc.width(), entry.doc.height());
    if w.saturating_mul(k) > 16_384 || h.saturating_mul(k) > 16_384 {
        state.toasts.push(Level::Error, format!("{k}× would exceed 16384 px on a side."));
        return false;
    }
    let mut dlg = rfd::FileDialog::new()
        .set_title(format!("Export at {k}×"))
        .add_filter("PNG", &["png"])
        .add_filter("WebP", &["webp"])
        .add_filter("BMP", &["bmp"])
        .set_file_name(format!("{stem}@{k}x.png"));
    if let Some(dir) = entry.doc.path.as_ref().and_then(|p| p.parent()).filter(|d| d.exists()) {
        dlg = dlg.set_directory(dir);
    }
    let Some(mut path) = dlg.save_file() else { return false };
    if path.extension().is_none() {
        path.set_extension("png");
    }
    let flat = qsketch_core::composite::flatten(entry.doc.state());
    let big = flat.resized(w * k, h * k, qsketch_core::raster::ResizeFilter::Nearest);
    match io::image_io::export_raster(&path, &big) {
        Ok(()) => {
            state.toasts.push(Level::Success, format!("Exported {} at {k}×", path.display()));
            true
        }
        Err(e) => {
            state.toasts.push(Level::Error, format!("Export failed: {e:#}"));
            false
        }
    }
}

/// Export a deduplicated tileset cut on the tile grid (View ▸ Grid size):
/// `<name>.png` sheet, `<name>.csv` map (one row per grid row, ids with
/// Tiled-style flip bits) and `<name>.json` with the geometry.
pub fn export_tileset(state: &mut AppState, doc_id: DocId) -> bool {
    let Some(entry) = state.doc(doc_id) else { return false };
    let base = entry.doc.title.trim_end_matches('*').to_string();
    let stem = Path::new(&base).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(base);
    let g = state.settings.canvas.grid_size.max(1);
    let mut dlg = rfd::FileDialog::new()
        .set_title(format!("Export Tileset ({g}×{g} tiles from the grid size)"))
        .add_filter("PNG", &["png"])
        .set_file_name(format!("{stem}-tiles.png"));
    if let Some(dir) = entry.doc.path.as_ref().and_then(|p| p.parent()).filter(|d| d.exists()) {
        dlg = dlg.set_directory(dir);
    }
    let Some(mut path) = dlg.save_file() else { return false };
    path.set_extension("png");
    let flat = qsketch_core::composite::flatten(entry.doc.state());
    let ts = qsketch_core::ops::build_tileset(&flat, g, g, true);
    let csv_path = path.with_extension("csv");
    let json_path = path.with_extension("json");
    let csv: String = ts
        .map
        .chunks(ts.map_w.max(1) as usize)
        .map(|row| row.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","))
        .collect::<Vec<_>>()
        .join("\n");
    let json = serde_json::json!({
        "tilewidth": g,
        "tileheight": g,
        "columns": ts.columns,
        "tilecount": ts.tile_count,
        "image": path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        "map": { "width": ts.map_w, "height": ts.map_h, "data": ts.map },
        "flipflags": { "h": qsketch_core::ops::TILE_FLIP_H, "v": qsketch_core::ops::TILE_FLIP_V }
    });
    let result = io::image_io::export_raster(&path, &ts.sheet)
        .and_then(|_| std::fs::write(&csv_path, csv).map_err(anyhow::Error::from))
        .and_then(|_| std::fs::write(&json_path, serde_json::to_string_pretty(&json)?).map_err(anyhow::Error::from));
    match result {
        Ok(()) => {
            state.toasts.push(
                Level::Success,
                format!("Exported {} tiles to {} (+ .csv / .json map)", ts.tile_count, path.display()),
            );
            true
        }
        Err(e) => {
            state.toasts.push(Level::Error, format!("Tileset export failed: {e:#}"));
            false
        }
    }
}

/// Close without any confirmation.
pub fn force_close(state: &mut AppState, doc_id: DocId) {
    if let Some(rs) = &state.render_state {
        if let Some(r) = rs.renderer.write().callback_resources.get_mut::<crate::canvas::render::CanvasRenderer>() {
            r.forget(doc_id);
        }
    }
    state.thumbs.retain(|k| k.0 != doc_id);
    state.autosave.forget(doc_id);
    state.remove_document(doc_id);
    state.close_doc_requests.retain(|d| *d != doc_id);
}
