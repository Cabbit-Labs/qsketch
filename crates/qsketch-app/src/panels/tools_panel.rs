//! The vertical toolbar.

use egui::Ui;
use qsketch_core::Rgba8;

use crate::actions::Action;
use crate::state::AppState;
use crate::tools::ToolKind;
use crate::ui::widgets::{checkerboard, rgba_to_color32};

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let width = ui.available_width();
    let columns = if width >= 64.0 { 2 } else { 1 };
    let btn = 28.0;
    let mut last_group = None;
    ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
    let current = state.tool;
    let mut clicked: Option<ToolKind> = None;
    // (dragged, index in `order` to insert at after removal)
    let mut dropped: Option<(ToolKind, usize)> = None;
    let palette = state.settings.ui.palette();
    let locked = state.settings.ui.tools_locked;
    let custom = !state.settings.ui.tool_order.is_empty();
    let order = tool_order(&state.settings.ui.tool_order);
    // Group dividers only make sense in the default order; a custom (or
    // being-edited) layout is a plain uniform grid so tools can sit anywhere.
    let dividers = locked && !custom;
    let panel_rect = ui.max_rect();

    // Drag-to-scroll would steal the tool drags while unlocked.
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .scroll_source(egui::scroll_area::ScrollSource {
            drag: if locked { egui::scroll_area::DragScroll::OnTouch } else { egui::scroll_area::DragScroll::Never },
            ..Default::default()
        })
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(2.0);
                let mut row: Vec<ToolKind> = Vec::new();
                let dropped = &mut dropped;
                let mut flush = |ui: &mut Ui, row: &mut Vec<ToolKind>, clicked: &mut Option<ToolKind>| {
                    if row.is_empty() {
                        return;
                    }
                    ui.horizontal(|ui| {
                        // `horizontal` spans the full width, so center the row by hand.
                        let n = row.len() as f32;
                        let content = n * btn + (n - 1.0) * ui.spacing().item_spacing.x;
                        ui.add_space(((ui.available_width() - content) / 2.0).max(0.0));
                        for &t in row.iter() {
                            let key = state.keymap.primary_text(Action::for_tool(t));
                            let tip =
                                if key.is_empty() { t.label().to_string() } else { format!("{} ({})", t.label(), key) };
                            if locked {
                                let r = crate::ui::widgets::icon_button(ui, t.icon(), &tip, btn, t == current);
                                if r.clicked() {
                                    *clicked = Some(t);
                                }
                            } else {
                                // Unlocked: every button is a drag source and a drop target.
                                let id = ui.id().with(("tool_dnd", t));
                                let inner = ui.dnd_drag_source(id, t, |ui| {
                                    crate::ui::widgets::icon_button(ui, t.icon(), &tip, btn, t == current)
                                });
                                if inner.inner.clicked() {
                                    *clicked = Some(t);
                                }
                                let target = inner.response;
                                // Left half = before this tool, right half = after it.
                                let after =
                                    ui.input(|i| i.pointer.latest_pos()).is_some_and(|p| p.x > target.rect.center().x);
                                if let Some(src) = target.dnd_hover_payload::<ToolKind>() {
                                    if *src != t {
                                        let r = target.rect;
                                        let x = if after { r.right() + 1.0 } else { r.left() - 1.0 };
                                        ui.painter().vline(x, r.y_range(), egui::Stroke::new(2.0, palette.accent));
                                    }
                                }
                                if let Some(src) = target.dnd_release_payload::<ToolKind>() {
                                    if *src != t {
                                        let idx = order.iter().position(|&x| x == t).unwrap_or(0);
                                        *dropped = Some((*src, if after { idx + 1 } else { idx }));
                                    }
                                }
                            }
                        }
                    });
                    row.clear();
                };
                for &t in order.iter() {
                    if dividers && last_group.is_some_and(|g| g != t.group()) {
                        flush(ui, &mut row, &mut clicked);
                        ui.add_space(2.0);
                        let (r, _) =
                            ui.allocate_exact_size(egui::vec2(ui.available_width(), 2.0), egui::Sense::hover());
                        let w = (columns as f32 * btn + (columns as f32 - 1.0) * 2.0).min(r.width());
                        let line = egui::Rect::from_center_size(r.center(), egui::vec2(w, 2.0));
                        ui.painter().rect_filled(line, 1, palette.border);
                        ui.add_space(2.0);
                    }
                    last_group = Some(t.group());
                    row.push(t);
                    if row.len() == columns {
                        flush(ui, &mut row, &mut clicked);
                    }
                }
                flush(ui, &mut row, &mut clicked);
                ui.add_space(4.0);
                let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 2.0), egui::Sense::hover());
                let w = (columns as f32 * btn + (columns as f32 - 1.0) * 2.0).min(r.width());
                let line = egui::Rect::from_center_size(r.center(), egui::vec2(w, 2.0));
                ui.painter().rect_filled(line, 1, palette.border);
                ui.add_space(3.0);
                color_pair(ui, state);
            });
        });

    if let Some(t) = clicked {
        state.set_tool(t);
    }
    // Released anywhere else in the strip: move to the end.
    if dropped.is_none() && !locked {
        let catch = ui.interact(panel_rect, ui.id().with("tool_dnd_catch"), egui::Sense::hover());
        if let Some(src) = catch.dnd_release_payload::<ToolKind>() {
            dropped = Some((*src, order.len()));
        }
    }
    if let Some((src, at)) = dropped {
        let mut v = order;
        let from = v.iter().position(|&x| x == src).unwrap_or(0);
        v.remove(from);
        let at = if at > from { at - 1 } else { at }.min(v.len());
        v.insert(at, src);
        state.settings.ui.tool_order = v;
    }
}

/// The user's tool order, validated: unknown entries dropped, missing tools
/// appended in default order.
pub fn tool_order(saved: &[ToolKind]) -> Vec<ToolKind> {
    let mut v: Vec<ToolKind> = Vec::with_capacity(ToolKind::ALL.len());
    for &t in saved {
        if ToolKind::ALL.contains(&t) && !v.contains(&t) {
            v.push(t);
        }
    }
    for t in ToolKind::ALL {
        if !v.contains(&t) {
            v.push(t);
        }
    }
    v
}

/// Foreground/background swatch pair with swap and reset buttons.
pub fn color_pair(ui: &mut Ui, state: &mut AppState) {
    let size = 40.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size + 8.0, size + 8.0), egui::Sense::hover());
    let p = ui.painter();
    let bg_rect =
        egui::Rect::from_min_size(rect.min + egui::vec2(size * 0.35, size * 0.35), egui::Vec2::splat(size * 0.65));
    let fg_rect = egui::Rect::from_min_size(rect.min, egui::Vec2::splat(size * 0.65));
    let bg_resp = ui.interact(bg_rect, ui.id().with("bg"), egui::Sense::click());
    let fg_resp = ui.interact(fg_rect, ui.id().with("fg"), egui::Sense::click());
    if state.bg.a < 255 {
        checkerboard(p, bg_rect, 4.0);
    }
    p.rect_filled(bg_rect, 2, rgba_to_color32(state.bg));
    p.rect_stroke(bg_rect, 2, egui::Stroke::new(1.0, egui::Color32::from_gray(90)), egui::StrokeKind::Outside);
    if state.fg.a < 255 {
        checkerboard(p, fg_rect, 4.0);
    }
    p.rect_filled(fg_rect, 2, rgba_to_color32(state.fg));
    p.rect_stroke(fg_rect, 2, egui::Stroke::new(1.0, egui::Color32::from_gray(90)), egui::StrokeKind::Outside);
    // swap + reset mini buttons
    let swap_rect = egui::Rect::from_min_size(rect.min + egui::vec2(size * 0.7, 0.0), egui::Vec2::splat(14.0));
    let reset_rect = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, size * 0.72), egui::Vec2::splat(14.0));
    let swap = ui.interact(swap_rect, ui.id().with("swap"), egui::Sense::click());
    let reset = ui.interact(reset_rect, ui.id().with("reset"), egui::Sense::click());
    let p = ui.painter();
    p.text(
        swap_rect.center(),
        egui::Align2::CENTER_CENTER,
        crate::ui::icons::ARROWS_LEFT_RIGHT,
        egui::FontId::new(12.0, crate::ui::iconset::family()),
        ui.visuals().weak_text_color(),
    );
    let mini_fg = egui::Rect::from_min_size(reset_rect.min, egui::Vec2::splat(9.0));
    let mini_bg = egui::Rect::from_min_size(reset_rect.min + egui::vec2(4.0, 4.0), egui::Vec2::splat(9.0));
    p.rect_filled(mini_bg, 1, egui::Color32::WHITE);
    p.rect_stroke(mini_bg, 1, egui::Stroke::new(1.0, egui::Color32::from_gray(60)), egui::StrokeKind::Outside);
    p.rect_filled(mini_fg, 1, egui::Color32::BLACK);
    p.rect_stroke(mini_fg, 1, egui::Stroke::new(1.0, egui::Color32::from_gray(160)), egui::StrokeKind::Outside);

    if swap.on_hover_text(format!("Swap colors ({})", state.keymap.primary_text(Action::SwapColors))).clicked() {
        std::mem::swap(&mut state.fg, &mut state.bg);
    }
    if reset.on_hover_text(format!("Default colors ({})", state.keymap.primary_text(Action::DefaultColors))).clicked() {
        state.fg = Rgba8::BLACK;
        state.bg = Rgba8::WHITE;
    }
    if fg_resp.on_hover_text("Foreground color (click to edit)").clicked() {
        state.dialogs.color_target = Some(crate::dialogs::ColorTarget::Foreground);
        state.show_panel_requests.push(crate::workspace::PanelKind::Color);
    }
    if bg_resp.on_hover_text("Background color (click to edit)").clicked() {
        state.dialogs.color_target = Some(crate::dialogs::ColorTarget::Background);
        state.show_panel_requests.push(crate::workspace::PanelKind::Color);
    }
}
