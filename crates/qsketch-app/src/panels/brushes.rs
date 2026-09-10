//! Brush presets panel.

use egui::{Color32, Sense, Ui};
use qsketch_core::{BrushSettings, Rgba8};

use crate::state::AppState;
use crate::ui::icons;
use crate::ui::widgets::icon_button;

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let tool = state.effective_tool();
    let current_name = state.current_brush().map(|b| b.name.clone());
    ui.horizontal(|ui| {
        if icon_button(ui, icons::PLUS, "Save current brush as a preset", 22.0, false).clicked() {
            if let Some(b) = state.current_brush().cloned() {
                let mut b = b;
                let base = b.name.trim_end_matches(char::is_numeric).trim().to_string();
                let mut n = 2;
                let mut name = b.name.clone();
                while state.presets.iter().any(|p| p.name == name) {
                    name = format!("{base} {n}");
                    n += 1;
                }
                b.name = name;
                state.presets.push(b);
            }
        }
        if icon_button(ui, icons::ARROW_COUNTER_CLOCKWISE, "Restore default presets", 22.0, false).clicked() {
            state.presets = BrushSettings::presets();
        }
        if icon_button(ui, icons::SLIDERS_HORIZONTAL, "Brush Settings (F9)", 22.0, false).clicked() {
            state.show_panel_requests.push(crate::workspace::PanelKind::BrushSettings);
        }
        ui.label(
            egui::RichText::new(if tool.uses_brush() {
                "Click to apply · double-click to edit"
            } else {
                "Select a brush tool"
            })
            .weak()
            .small(),
        );
    });
    ui.separator();
    let mut apply: Option<usize> = None;
    let mut remove: Option<usize> = None;
    let mut edit = false;
    let text_color = ui.visuals().text_color();
    let preview_size = [72usize, 30usize];
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let AppState { presets, library, .. } = state;
        for (i, p) in presets.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 38.0), Sense::click());
            let selected = current_name.as_deref() == Some(p.name.as_str());
            let fill = if selected {
                ui.visuals().selection.bg_fill
            } else if resp.hovered() {
                ui.visuals().widgets.hovered.bg_fill
            } else if i % 2 == 0 {
                ui.visuals().faint_bg_color
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 3, fill);
            // Real stroke preview in the panel's text color.
            if ui.is_rect_visible(rect) {
                let tex = library.stroke_preview(
                    ui.ctx(),
                    &format!("preset-{i}"),
                    p,
                    Rgba8::BLACK,
                    Rgba8::new(128, 128, 128, 255),
                    preview_size,
                );
                let pr = egui::Rect::from_min_size(
                    rect.left_center() + egui::vec2(4.0, -(preview_size[1] as f32) / 2.0),
                    egui::vec2(preview_size[0] as f32, preview_size[1] as f32),
                );
                ui.painter().image(
                    tex.id(),
                    pr,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    text_color,
                );
            }
            let text_x = preview_size[0] as f32 + 12.0;
            ui.painter().text(
                rect.left_center() + egui::vec2(text_x, -7.0),
                egui::Align2::LEFT_CENTER,
                &p.name,
                egui::FontId::proportional(13.0),
                text_color,
            );
            let mut desc = format!("{:.0}px", p.size);
            if p.is_round() {
                desc.push_str(&format!(" · hard {:.0}%", p.hardness * 100.0));
            } else {
                desc.push_str(&format!(" · {}", p.tip));
            }
            desc.push_str(&format!(" · flow {:.0}%", p.flow * 100.0));
            if p.pressure_size {
                desc.push_str(" · ↕size");
            }
            if p.pressure_opacity {
                desc.push_str(" · ↕opacity");
            }
            if p.texture_enabled && !p.texture.is_empty() {
                desc.push_str(" · grain");
            }
            if p.scattering {
                desc.push_str(" · scatter");
            }
            if p.color_dynamics {
                desc.push_str(" · color");
            }
            ui.painter().text(
                rect.left_center() + egui::vec2(text_x, 8.0),
                egui::Align2::LEFT_CENTER,
                desc,
                egui::FontId::proportional(10.5),
                ui.visuals().weak_text_color(),
            );
            if resp.clicked() {
                apply = Some(i);
            }
            if resp.double_clicked() {
                apply = Some(i);
                edit = true;
            }
            resp.context_menu(|ui| {
                if ui.button("Apply").clicked() {
                    apply = Some(i);
                    ui.close();
                }
                if ui.button("Edit…").clicked() {
                    apply = Some(i);
                    edit = true;
                    ui.close();
                }
                if ui.button("Overwrite with current brush").clicked() {
                    apply = Some(usize::MAX - i);
                    ui.close();
                }
                if ui.button("Delete preset").clicked() {
                    remove = Some(i);
                    ui.close();
                }
            });
        }
    });
    if let Some(i) = apply {
        if i > usize::MAX / 2 {
            let idx = usize::MAX - i;
            if let Some(b) = state.current_brush().cloned() {
                let name = state.presets[idx].name.clone();
                state.presets[idx] = BrushSettings { name, ..b };
            }
        } else {
            let preset = state.presets[i].clone();
            if let Some(b) = state.current_brush_mut() {
                *b = preset;
            } else {
                state.set_tool(crate::tools::ToolKind::Brush);
                state.brush = preset;
            }
        }
    }
    if let Some(i) = remove {
        state.presets.remove(i);
        // Preview cache keys are positional; drop them so rows re-render.
        for k in i..=state.presets.len() {
            state.library.forget_stroke_preview(&format!("preset-{k}"));
        }
    }
    if edit {
        state.show_panel_requests.push(crate::workspace::PanelKind::BrushSettings);
    }
}
