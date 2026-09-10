//! History panel: click any state to jump to it.

use egui::{Color32, Sense, Ui};

use crate::state::AppState;
use crate::ui::icons;

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let Some(entry) = state.active_mut() else {
        ui.centered_and_justified(|ui| ui.label(egui::RichText::new("No document").weak()));
        return;
    };
    let cursor = entry.doc.history.cursor();
    let len = entry.doc.history.len();
    let mut jump: Option<usize> = None;
    let row_h = 22.0;
    let last_id = ui.id().with("last_cursor");
    let last = ui.data(|d| d.get_temp::<usize>(last_id));
    let cursor_changed = last != Some(cursor);
    ui.data_mut(|d| d.insert_temp(last_id, cursor));
    // Leave room for the footer so the list never pushes it out of the panel.
    let footer_h = 26.0;
    let list_h = (ui.available_height() - footer_h).max(row_h);
    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(list_h).show(ui, |ui| {
        for (i, e) in entry.doc.history.entries().iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), Sense::click());
            let is_cur = i == cursor;
            let future = i > cursor;
            let fill = if is_cur {
                ui.visuals().selection.bg_fill
            } else if resp.hovered() {
                ui.visuals().widgets.hovered.bg_fill
            } else if i % 2 == 0 {
                ui.visuals().faint_bg_color
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 3, fill);
            let color =
                if future { ui.visuals().weak_text_color().gamma_multiply(0.6) } else { ui.visuals().text_color() };
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
    if let Some(i) = jump {
        state.cancel_session();
        if let Some(entry) = state.active_mut() {
            entry.doc.jump_to(i);
            entry.sel_outline = None;
        }
    }
    ui.separator();
    if let Some(entry) = state.active_mut() {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{} states", len)).weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Clear").on_hover_text("Discard undo history (frees memory)").clicked() {
                    entry.doc.history.clear_to_current();
                }
            });
        });
    }
}
