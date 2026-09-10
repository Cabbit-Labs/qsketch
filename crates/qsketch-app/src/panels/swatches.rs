//! Swatches panel: a grid of saved colors.

use egui::{Ui, Vec2};

use crate::state::AppState;
use crate::ui::icons;
use crate::ui::widgets::{icon_button, Swatch};

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        if icon_button(ui, icons::PLUS, "Add foreground color as swatch", 22.0, false).clicked()
            && !state.swatches.contains(&state.fg)
        {
            state.swatches.push(state.fg);
        }
        if icon_button(ui, icons::ARROW_COUNTER_CLOCKWISE, "Reset to default swatches", 22.0, false).clicked() {
            state.swatches = crate::settings::default_swatches();
        }
        ui.label(
            egui::RichText::new("Click: foreground · Right-click: background · Shift+click: remove").weak().small(),
        );
    });
    ui.separator();
    let size = 20.0;
    let spacing = 3.0;
    let cols = ((ui.available_width() + spacing) / (size + spacing)).floor().max(1.0) as usize;
    let mut remove: Option<usize> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(spacing);
        let n = state.swatches.len();
        let mut i = 0;
        while i < n {
            ui.horizontal(|ui| {
                for _ in 0..cols {
                    if i >= n {
                        break;
                    }
                    let c = state.swatches[i];
                    let resp = ui.add(Swatch { color: c, size: Vec2::splat(size), selected: c == state.fg });
                    let resp = resp.on_hover_text(c.to_hex());
                    let mods = ui.input(|inp| inp.modifiers);
                    if resp.clicked() {
                        if mods.shift {
                            remove = Some(i);
                        } else {
                            state.fg = c;
                        }
                    }
                    if resp.secondary_clicked() {
                        state.bg = c;
                    }
                    i += 1;
                }
            });
        }
    });
    if let Some(i) = remove {
        state.swatches.remove(i);
    }
}
