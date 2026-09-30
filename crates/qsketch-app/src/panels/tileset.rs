//! Tileset panel: the tiles of the active tilemap layer's tileset. Click a
//! tile to stamp it with the Tile tool; the flip toggles stamp it mirrored.

use egui::{Color32, Sense, Ui};
use qsketch_core::tilemap;

use crate::actions::Action;
use crate::state::AppState;
use crate::ui::icons;
use crate::ui::widgets::icon_toggle;

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let ctx = ui.ctx().clone();
    let Some(doc_id) = state.active_doc else {
        ui.centered_and_justified(|ui| ui.label(egui::RichText::new("No document").weak()));
        return;
    };
    let info = state.doc(doc_id).and_then(|e| {
        let s = e.doc.state();
        s.active_layer().props.tilemap.as_ref().map(|tm| (tm.tileset, e.generation))
    });
    let Some((ts_index, generation)) = info else {
        ui.add_space(4.0);
        ui.label(egui::RichText::new("The active layer isn't a tilemap layer.").weak());
        ui.add_space(6.0);
        if ui.button("New Tilemap Layer").on_hover_text("Uses the grid size as the tile size (View › Grid)").clicked()
        {
            state.pending.push(Action::NewTilemapLayer);
        }
        if ui.button("Convert Active Layer to Tilemap").clicked() {
            state.pending.push(Action::ConvertToTilemap);
        }
        return;
    };
    let (tiles, tw, th, name, usage) = {
        let e = state.doc(doc_id).unwrap();
        let s = e.doc.state();
        let ts = &s.tilesets[ts_index];
        (ts.tiles.clone(), ts.tile_w, ts.tile_h, ts.name.clone(), tilemap::usage(s, ts_index))
    };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{name} · {tw}×{th} · {} tiles", tiles.len().saturating_sub(1))).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            icon_toggle(
                ui,
                icons::FLIP_VERTICAL,
                icons::FLIP_VERTICAL,
                &mut state.tool_opts.tile_flip_v,
                "Stamp flipped vertically",
                20.0,
            );
            icon_toggle(
                ui,
                icons::FLIP_HORIZONTAL,
                icons::FLIP_HORIZONTAL,
                &mut state.tool_opts.tile_flip_h,
                "Stamp flipped horizontally",
                20.0,
            );
        });
    });
    ui.add_space(4.0);
    let cell = 40.0;
    let accent = ui.visuals().selection.stroke.color;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
            for (i, t) in tiles.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(cell, cell), Sense::click());
                let key = (doc_id | (1u64 << 40) | ((ts_index as u64) << 32), i as u64);
                let tex = state
                    .thumbs
                    .get(&ctx, key, generation, [36, 36], |m| crate::panels::thumbs::raster_thumb(t, m, true));
                let size = tex.size_vec2();
                let k = ((cell - 4.0) / size.x).min((cell - 4.0) / size.y);
                let draw = egui::Rect::from_center_size(rect.center(), size * k);
                if i == 0 {
                    // The empty tile: stamping it clears cells.
                    ui.painter().rect_stroke(
                        draw,
                        2.0,
                        egui::Stroke::new(1.0, Color32::from_gray(110)),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().line_segment(
                        [draw.left_bottom(), draw.right_top()],
                        egui::Stroke::new(1.0, Color32::from_gray(110)),
                    );
                } else {
                    ui.painter().image(
                        tex.id(),
                        draw,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                let selected = state.tool_opts.tile_index as usize == i;
                let stroke = if selected {
                    egui::Stroke::new(2.0, accent)
                } else if resp.hovered() {
                    egui::Stroke::new(1.0, ui.visuals().widgets.hovered.fg_stroke.color)
                } else {
                    egui::Stroke::new(1.0, Color32::from_black_alpha(90))
                };
                ui.painter().rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
                let tip = if i == 0 {
                    "Empty tile (stamping it clears cells)".to_string()
                } else {
                    format!("Tile {i} · used {}×", usage.get(i).copied().unwrap_or(0))
                };
                if resp.on_hover_text(tip).clicked() {
                    state.tool_opts.tile_index = i as u32;
                    state.set_tool(crate::tools::ToolKind::Tile);
                }
            }
        });
    });
}
