//! Copy / cut / paste between documents and the OS clipboard.

use std::borrow::Cow;

use qsketch_core::{ClipImage, IRect, Layer, Pt};

use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

/// Hand the image to the OS clipboard; false when that failed (typically
/// another application holding the clipboard for a moment).
fn to_os(clip: &ClipImage) -> bool {
    match arboard::Clipboard::new() {
        Ok(mut cb) => {
            let img = arboard::ImageData {
                width: clip.width as usize,
                height: clip.height as usize,
                bytes: Cow::Borrowed(&clip.rgba),
            };
            match cb.set_image(img) {
                Ok(()) => true,
                Err(e) => {
                    log::warn!("clipboard set_image: {e}");
                    false
                }
            }
        }
        Err(e) => {
            log::warn!("clipboard unavailable: {e}");
            false
        }
    }
}

fn from_os() -> Option<ClipImage> {
    let mut cb = match arboard::Clipboard::new() {
        Ok(cb) => cb,
        Err(e) => {
            log::warn!("clipboard unavailable: {e}");
            return None;
        }
    };
    let img = match cb.get_image() {
        Ok(img) => img,
        Err(e) => {
            log::info!("clipboard has no image: {e}");
            return None;
        }
    };
    if img.width == 0 || img.height == 0 {
        return None;
    }
    log::info!("clipboard image {}×{}", img.width, img.height);
    Some(ClipImage::from_rgba(img.width as u32, img.height as u32, img.bytes.into_owned()))
}

pub fn copy(state: &mut AppState, doc_id: DocId, merged: bool) -> bool {
    let Some(entry) = state.doc(doc_id) else { return false };
    let s = entry.doc.state();
    let clip = if merged {
        ClipImage::from_composite(&entry.doc.composite, s.selection_mask())
    } else {
        ClipImage::from_layer(&s.active_layer().raster, s.selection_mask())
    };
    match clip {
        Some(c) => {
            state.clipboard_in_os = to_os(&c);
            state.clipboard = Some(c);
            true
        }
        None => {
            state.toasts.push(Level::Info, "Nothing to copy.");
            false
        }
    }
}

pub fn cut(state: &mut AppState, doc_id: DocId) {
    if !copy(state, doc_id, false) {
        return;
    }
    state.settle();
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let li = entry.doc.state().active;
    if !entry.doc.state().layer_editable(li) {
        state.toasts.push(Level::Info, "The active layer is locked or hidden.");
        return;
    }
    let dirty = qsketch_core::ops::clear(entry.doc.state_mut(), li);
    entry.doc.mark_dirty_rect(dirty);
    entry.doc.commit("Cut");
}

/// Paste onto the active layer as a floating image that can be moved and
/// scaled before it is committed. `in_place` keeps the original position when
/// the clip came from qsketch; otherwise the image is centered in the view
/// (and shrunk to fit if it is larger than the document).
pub fn paste(state: &mut AppState, doc_id: DocId, in_place: bool) {
    // The internal copy carries the origin (and exact alpha); the OS
    // clipboard only wins when it holds something newer, i.e. a different
    // picture that was put there after our copy reached it. A picture that
    // merely lost its alpha on the way through the OS is still ours.
    let os = from_os();
    let clip = match (os, &state.clipboard) {
        (Some(o), Some(i)) if !state.clipboard_in_os || i.same_picture(&o) => {
            log::info!("paste: internal clip ({}×{} at {:?})", i.width, i.height, i.origin);
            i.clone()
        }
        (Some(o), _) => {
            log::info!("paste: OS clip ({}×{}), no origin", o.width, o.height);
            o
        }
        (None, Some(i)) => i.clone(),
        (None, None) => {
            state.toasts.push(Level::Info, "Clipboard has no image.");
            return;
        }
    };
    if in_place && clip.origin == (0, 0) && state.clipboard.as_ref().is_none_or(|i| !i.same_picture(&clip)) {
        state.toasts.push(
            Level::Info,
            "This image did not come from qsketch, so it has no place to go back to; pasting at the top-left.",
        );
    }
    let Some(entry) = state.doc(doc_id) else { return };
    let (dw, dh) = (entry.doc.width(), entry.doc.height());
    let (mut w, mut h) = (clip.width, clip.height);
    let (x, y) = if in_place {
        clip.origin
    } else {
        if w > dw || h > dh {
            let k = (dw as f32 / w as f32).min(dh as f32 / h as f32);
            w = ((w as f32 * k).round() as u32).max(1);
            h = ((h as f32 * k).round() as u32).max(1);
        }
        // Center in the visible viewport, snapped to pixels.
        let c: Pt = entry.view.screen_to_doc(entry.view.viewport.center());
        let cx = c.x.clamp(0.0, dw as f32);
        let cy = c.y.clamp(0.0, dh as f32);
        ((cx - w as f32 / 2.0).round() as i32, (cy - h as f32 / 2.0).round() as i32)
    };
    if state.settings.general.paste_new_layer {
        // Finish any pending paste first, then add the target layer; it becomes
        // part of the "Paste" history step (Esc reverts it along with the pixels).
        crate::tools::floating::commit(state);
        state.cancel_session();
        if let Some(entry) = state.doc_mut(doc_id) {
            let s = entry.doc.state_mut();
            let name = s.unique_layer_name("Pasted Layer");
            s.add_layer(name, None);
            let r = entry.doc.state().rect();
            entry.doc.mark_dirty_rect(r);
        }
    }
    if crate::tools::floating::begin(state, doc_id, clip.to_raster(), x, y, w, h) {
        state.toasts.push(Level::Info, "Drag to move, drag handles to scale. Enter applies, Esc cancels.");
    }
}

/// Paste as a smart object: a new layer holding the clipboard picture at its
/// full size, placed (scaled to fit the canvas if it is bigger) in a
/// transform box. However small it is shown, the original stays whole.
pub fn paste_smart(state: &mut AppState, doc_id: DocId) {
    // Same choice as a plain paste: ours unless the OS holds something newer.
    let clip = match (from_os(), &state.clipboard) {
        (Some(o), Some(i)) if !state.clipboard_in_os || i.same_picture(&o) => i.clone(),
        (Some(o), _) => o,
        (None, Some(i)) => i.clone(),
        (None, None) => {
            state.toasts.push(Level::Info, "Clipboard has no image.");
            return;
        }
    };
    crate::tools::floating::commit(state);
    state.cancel_session();
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let (dw, dh) = (entry.doc.width(), entry.doc.height());
    let (mut w, mut h) = (clip.width, clip.height);
    if w > dw || h > dh {
        let k = (dw as f32 / w as f32).min(dh as f32 / h as f32);
        w = ((w as f32 * k).round() as u32).max(1);
        h = ((h as f32 * k).round() as u32).max(1);
    }
    let c: Pt = entry.view.screen_to_doc(entry.view.viewport.center());
    let (cx, cy) = (c.x.clamp(0.0, dw as f32), c.y.clamp(0.0, dh as f32));
    let (x, y) = ((cx - w as f32 / 2.0).round() as i32, (cy - h as f32 / 2.0).round() as i32);
    let s = entry.doc.state_mut();
    let name = s.unique_layer_name("Smart Object");
    s.add_layer(name, None);
    let li = s.active;
    if crate::tools::floating::begin_smart_paste(
        state,
        doc_id,
        li,
        clip.to_raster(),
        IRect::new(x, y, w as i32, h as i32),
    ) {
        state.toasts.push(Level::Info, "Place the smart object, then Enter. Ctrl+T reopens it any time, losslessly.");
    }
}

/// Paste as a brand new layer (no floating placement).
#[allow(dead_code)]
pub fn paste_as_layer(state: &mut AppState, doc_id: DocId, clip: &ClipImage, x: i32, y: i32) {
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let (dw, dh) = (entry.doc.width(), entry.doc.height());
    let raster = clip.to_layer_raster(dw, dh, x, y);
    let s = entry.doc.state_mut();
    let id = s.new_id();
    let name = s.unique_layer_name("Layer");
    let layer = Layer::new(id, name, dw, dh).with_raster(raster);
    let at = s.active + 1;
    s.insert_layer(layer, at);
    s.selection = None;
    entry.doc.mark_all_dirty();
    entry.doc.commit("Paste");
    entry.sel_outline = None;
}

/// Paste into a brand new document sized to the clipboard image.
/// Size of the image on the OS clipboard, if there is one.
pub fn os_image_size() -> Option<(u32, u32)> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let img = cb.get_image().ok()?;
    (img.width > 0 && img.height > 0).then_some((img.width as u32, img.height as u32))
}

pub fn paste_as_new_document(state: &mut AppState) {
    let clip = match from_os().or_else(|| state.clipboard.clone()) {
        Some(c) => c,
        None => {
            state.toasts.push(Level::Info, "Clipboard has no image.");
            return;
        }
    };
    let title = state.untitled_title();
    let doc_state = qsketch_core::DocState::from_raster("Layer 1", clip.to_raster());
    let doc = qsketch_core::Document::from_state(doc_state, title, None, "Paste");
    state.add_document(doc);
}
