//! Layer flash: when the active layer changes, its pixels light up briefly
//! on the canvas so you can see which layer you just picked (like
//! Aseprite's experimental "flash layer" hint).

use std::time::Instant;

use egui::{Color32, ColorImage, Context, Mesh, Painter, Pos2, TextureHandle, TextureOptions};
use qsketch_core::{IRect, LayerId, Pt};

use crate::state::{AppState, DocId};

/// Total fade time.
const DURATION: f32 = 0.5;
/// Peak overlay opacity.
const PEAK: f32 = 0.6;
/// Longest side of the silhouette texture; bigger layers are subsampled.
const MAX_TEX: i32 = 1024;

pub struct LayerFlash {
    started: Instant,
    tex: TextureHandle,
    /// Document-space rect the texture covers.
    rect: IRect,
}

/// Start a flash when the active layer differs from the one last seen, then
/// draw and age the current flash. Call once per canvas frame.
pub fn update(state: &mut AppState, doc_id: DocId, ctx: &Context, painter: &Painter) {
    let enabled = state.settings.canvas.flash_selected_layer;
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let active = entry.doc.state().active_layer().props.id;
    match entry.flash_seen_active {
        // First frame for this document: remember, don't flash.
        None => entry.flash_seen_active = Some(active),
        Some(prev) if prev != active => {
            entry.flash_seen_active = Some(active);
            entry.layer_flash = if enabled { build(ctx, entry, active) } else { None };
        }
        _ => {}
    }

    let Some(flash) = &entry.layer_flash else { return };
    let t = flash.started.elapsed().as_secs_f32() / DURATION;
    if t >= 1.0 {
        entry.layer_flash = None;
        return;
    }
    // Ease-out fade.
    let alpha = PEAK * (1.0 - t) * (1.0 - t);
    let tint = Color32::from_white_alpha((alpha * 255.0).round() as u8);
    let view = &entry.view;
    let r = flash.rect;
    let corners = [
        (Pt::new(r.x as f32, r.y as f32), Pos2::new(0.0, 0.0)),
        (Pt::new(r.right() as f32, r.y as f32), Pos2::new(1.0, 0.0)),
        (Pt::new(r.right() as f32, r.bottom() as f32), Pos2::new(1.0, 1.0)),
        (Pt::new(r.x as f32, r.bottom() as f32), Pos2::new(0.0, 1.0)),
    ];
    let mut mesh = Mesh::with_texture(flash.tex.id());
    for (p, uv) in corners {
        mesh.colored_vertex(view.doc_to_screen(p), tint);
        let last = mesh.vertices.len() - 1;
        mesh.vertices[last].uv = uv;
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
    ctx.request_repaint();
}

/// Silhouette (white, layer alpha) of the layer, or of a group's members.
fn build(ctx: &Context, entry: &crate::state::DocEntry, id: LayerId) -> Option<LayerFlash> {
    let s = entry.doc.state();
    let idx = s.index_of(id)?;
    let members: Vec<usize> = if s.layers[idx].is_group() {
        s.members(idx).filter(|&i| !s.layers[i].is_group()).collect()
    } else {
        vec![idx]
    };
    let mut rect = IRect::EMPTY;
    for &i in &members {
        if let Some(b) = s.layers[i].raster.bounds() {
            rect = if rect.is_empty() { b } else { rect.union(&b) };
        }
    }
    if rect.is_empty() {
        return None;
    }
    let step = (rect.w.max(rect.h) + MAX_TEX - 1) / MAX_TEX;
    let step = step.max(1);
    let tw = ((rect.w + step - 1) / step) as usize;
    let th = ((rect.h + step - 1) / step) as usize;
    let mut px = vec![Color32::TRANSPARENT; tw * th];
    for (ty, row) in px.chunks_mut(tw).enumerate() {
        let y = rect.y + ty as i32 * step;
        for (tx, out) in row.iter_mut().enumerate() {
            let x = rect.x + tx as i32 * step;
            let mut a = 0u8;
            for &i in &members {
                a = a.max(s.layers[i].raster.get_pixel(x, y).a);
            }
            if a > 0 {
                *out = Color32::from_white_alpha(a);
            }
        }
    }
    let img = ColorImage::new([tw, th], px);
    let opts = if step == 1 { TextureOptions::NEAREST } else { TextureOptions::LINEAR };
    let tex = ctx.load_texture(format!("layer-flash-{}-{id}", entry.id), img, opts);
    Some(LayerFlash { started: Instant::now(), tex, rect })
}
