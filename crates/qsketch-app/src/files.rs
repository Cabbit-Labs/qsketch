//! Open / save / export flows with native file dialogs.

use std::path::{Path, PathBuf};

use qsketch_core::io;
use qsketch_core::Document;

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
    match io::open(path) {
        Ok(doc_state) => {
            let title = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
            let doc = Document::from_state(doc_state, title, Some(path.to_path_buf()), "Open");
            let id = state.add_document(doc);
            state.settings.push_recent(path.to_path_buf());
            Some(id)
        }
        Err(e) => {
            state.toasts.push(Level::Error, format!("Couldn't open {}: {e:#}", path.display()));
            None
        }
    }
}

/// Save to the document's own path, or prompt if it has none / isn't native.
pub fn save(state: &mut AppState, doc_id: DocId) -> bool {
    let path = state.doc(doc_id).and_then(|d| d.doc.path.clone());
    match path {
        Some(p) if io::is_document(&p) => write_native(state, doc_id, &p),
        _ => save_as(state, doc_id),
    }
}

pub fn save_as(state: &mut AppState, doc_id: DocId) -> bool {
    let Some(entry) = state.doc(doc_id) else { return false };
    let suggested = entry
        .doc
        .path
        .as_ref()
        .map(|p| if io::is_document(p) { p.clone() } else { p.with_extension(io::NATIVE_EXTENSION) })
        .unwrap_or_else(|| {
            PathBuf::from(format!("{}.{}", entry.doc.title.trim_end_matches('*'), io::NATIVE_EXTENSION))
        });
    let mut dlg = rfd::FileDialog::new()
        .set_title("Save As")
        .add_filter("qsketch document", &[io::NATIVE_EXTENSION])
        .add_filter("Photoshop (layers, no selection)", &[io::psd::EXTENSION]);
    if let Some(name) = suggested.file_name() {
        dlg = dlg.set_file_name(name.to_string_lossy());
    }
    if let Some(dir) = suggested.parent().filter(|d| d.exists() && !d.as_os_str().is_empty()) {
        dlg = dlg.set_directory(dir);
    }
    let Some(mut path) = dlg.save_file() else { return false };
    if path.extension().is_none() {
        path.set_extension(io::NATIVE_EXTENSION);
    }
    write_native(state, doc_id, &path)
}

fn write_native(state: &mut AppState, doc_id: DocId, path: &Path) -> bool {
    let Some(entry) = state.doc_mut(doc_id) else { return false };
    match io::save_document(path, entry.doc.state()) {
        Ok(()) => {
            entry.doc.path = Some(path.to_path_buf());
            entry.doc.title = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            entry.doc.mark_saved();
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
