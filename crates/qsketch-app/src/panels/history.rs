//! History panel: click any state to jump to it.

use egui::{Color32, Sense, Ui};

use crate::state::AppState;
use crate::ui::icons;

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    // Thumbnail of the snapshot (the document as opened / last saved), cached
    // until the snapshot itself changes.
    let snap_tex = {
        let AppState { docs, thumbs, active_doc, .. } = &mut *state;
        active_doc.and_then(|id| docs.iter().find(|d| d.id == id)).map(|d| {
            let (_, st) = d.doc.snapshot();
            thumbs.get(&ui.ctx().clone(), (d.id, u64::MAX - 1), d.doc.snapshot_rev(), [48, 36], |m| {
                let flat = qsketch_core::composite::flatten(st);
                crate::panels::thumbs::raster_thumb(&flat, m, true)
            })
        })
    };
    let Some(entry) = state.active_mut() else {
        ui.centered_and_justified(|ui| ui.label(egui::RichText::new("No document").weak()));
        return;
    };
    let cursor = entry.doc.history.cursor();
    let len = entry.doc.history.len();
    let mut jump: Option<usize> = None;
    let mut revert = false;
    let row_h = 22.0;
    let last_id = ui.id().with("last_cursor");
    let last = ui.data(|d| d.get_temp::<usize>(last_id));
    let cursor_changed = last != Some(cursor);
    ui.data_mut(|d| d.insert_temp(last_id, cursor));
    // Leave room for the footer so the list never pushes it out of the panel.
    let footer_h = 26.0;
    let list_h = (ui.available_height() - footer_h).max(row_h);
    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(list_h).show(ui, |ui| {
        // The snapshot row: the document as opened or last saved, always
        // reachable even after the oldest states have been trimmed.
        {
            let (label, _) = entry.doc.snapshot();
            let snap_h = 42.0;
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), snap_h), Sense::click());
            let fill = if resp.hovered() {
                crate::ui::chrome::row_fill(ui.visuals().widgets.hovered.bg_fill)
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 3, fill);
            let color = ui.visuals().text_color();
            let mut x = 8.0;
            if let Some(tex) = &snap_tex {
                let thumb_rect = egui::Rect::from_min_size(rect.min + egui::vec2(x, 3.0), egui::vec2(48.0, 36.0));
                let size = tex.size_vec2();
                let scale = (thumb_rect.width() / size.x).min(thumb_rect.height() / size.y);
                let draw = egui::Rect::from_center_size(thumb_rect.center(), size * scale);
                ui.painter().image(
                    tex.id(),
                    draw,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                ui.painter().rect_stroke(
                    draw,
                    0,
                    egui::Stroke::new(1.0, Color32::from_black_alpha(120)),
                    egui::StrokeKind::Outside,
                );
                x += 56.0;
            }
            ui.painter().text(
                rect.left_center() + egui::vec2(x, -7.0),
                egui::Align2::LEFT_CENTER,
                &entry.doc.title,
                egui::FontId::proportional(13.0),
                color,
            );
            let dim = crate::ui::theme::dim_text(ui.visuals());
            ui.painter().text(
                rect.left_center() + egui::vec2(x, 8.0),
                egui::Align2::LEFT_CENTER,
                icons::CAMERA,
                egui::FontId::new(11.0, crate::ui::iconset::family()),
                dim,
            );
            ui.painter().text(
                rect.left_center() + egui::vec2(x + 16.0, 8.0),
                egui::Align2::LEFT_CENTER,
                label.to_lowercase(),
                egui::FontId::proportional(11.0),
                dim,
            );
            if resp.on_hover_text("Revert to the document as it was opened or last saved (undoable)").clicked() {
                revert = true;
            }
            ui.separator();
        }
        for (i, e) in entry.doc.history.entries().iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), Sense::click());
            let is_cur = i == cursor;
            let future = i > cursor;
            let fill = if is_cur {
                crate::ui::chrome::row_fill(ui.visuals().selection.bg_fill)
            } else if resp.hovered() {
                crate::ui::chrome::row_fill(ui.visuals().widgets.hovered.bg_fill)
            } else if i % 2 == 0 {
                crate::ui::chrome::zebra(ui)
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 3, fill);
            let color = if future { crate::ui::theme::dim_text(ui.visuals()) } else { ui.visuals().text_color() };
            let glyph = if i == 0 { icons::FILE } else { icons::PAINT_BRUSH };
            ui.painter().text(
                rect.left_center() + egui::vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                glyph,
                egui::FontId::new(13.0, crate::ui::iconset::family()),
                color,
            );
            ui.painter().text(
                rect.left_center() + egui::vec2(28.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &e.label,
                egui::FontId::proportional(13.0),
                color,
            );
            if resp.clicked() {
                jump = Some(i);
            }
            if is_cur && cursor_changed {
                resp.scroll_to_me(None);
            }
        }
    });
    if jump.is_some() || revert {
        // Like Undo: a jump abandons a paste or text still being placed
        // rather than committing it as a step that the jump then leaves.
        state.cancel_session();
        crate::tools::floating::cancel(state);
        crate::tools::text::cancel(state);
        if let Some(entry) = state.active_mut() {
            match jump {
                Some(i) => {
                    entry.doc.jump_to(i);
                }
                None => {
                    entry.doc.revert_to_snapshot();
                    entry.needs_full_upload = true;
                }
            }
            entry.sel_outline = None;
        }
    }
    ui.separator();
    if let Some(entry) = state.active_mut() {
        ui.horizontal(|ui| {
            let limit = entry.doc.history.limit();
            let text = if len >= limit { format!("{len} states (oldest dropped)") } else { format!("{len} states") };
            ui.label(egui::RichText::new(text).weak()).on_hover_text(format!(
                "Undo keeps the last {limit} states (Preferences ▸ General); the snapshot row above is always available"
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::ui::widgets::small_button(ui, "Clear")
                    .on_hover_text("Discard undo history (frees memory)")
                    .clicked()
                {
                    entry.doc.history.clear_to_current();
                }
            });
        });
    }
}
