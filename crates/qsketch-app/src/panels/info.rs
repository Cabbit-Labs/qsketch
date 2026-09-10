//! Info panel: cursor position, color under cursor, document and view facts.

use egui::Ui;

use crate::state::AppState;
use crate::ui::widgets::{rgba_to_color32, Swatch};

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let Some(entry) = state.active() else {
        ui.centered_and_justified(|ui| ui.label(egui::RichText::new("No document").weak()));
        return;
    };
    egui::Grid::new("info_grid").num_columns(2).spacing([10.0, 4.0]).striped(true).show(ui, |ui| {
        ui.label(egui::RichText::new("Cursor").weak());
        match state.hover_doc_pos {
            Some(p) => ui.label(format!("{}, {}", p.x.floor() as i32, p.y.floor() as i32)),
            None => ui.label("—"),
        };
        ui.end_row();
        ui.label(egui::RichText::new("Color").weak());
        match state.hover_color {
            Some(c) => {
                ui.horizontal(|ui| {
                    ui.add(Swatch { color: c, size: egui::vec2(16.0, 14.0), selected: false });
                    ui.label(format!("{} {} {} / {}  {}", c.r, c.g, c.b, c.a, c.to_hex()));
                });
            }
            None => {
                ui.label("—");
            }
        }
        ui.end_row();
        ui.label(egui::RichText::new("Document").weak());
        ui.label(format!("{} × {} px", entry.doc.width(), entry.doc.height()));
        ui.end_row();
        ui.label(egui::RichText::new("Zoom").weak());
        ui.label(format!("{:.1}%  rot {:.0}°", entry.view.zoom * 100.0, entry.view.rotation_degrees()));
        ui.end_row();
        ui.label(egui::RichText::new("Layer").weak());
        let l = entry.doc.state().active_layer();
        ui.label(format!("{} ({}/{})", l.props.name, entry.doc.state().active + 1, entry.doc.state().layers.len()));
        ui.end_row();
        ui.label(egui::RichText::new("Selection").weak());
        match entry.doc.state().selection_mask() {
            Some(m) => {
                let b = m.bounds();
                ui.label(format!("{}×{} at {}, {}", b.w, b.h, b.x, b.y))
            }
            None => ui.label("none"),
        };
        ui.end_row();
        ui.label(egui::RichText::new("Pen").weak());
        match state.pen.pressure {
            Some(p) => {
                ui.label(format!("pressure {:.0}%{}", p * 100.0, if state.pen.eraser { " (eraser)" } else { "" }))
            }
            None => ui.label(if state.pen.tablet_active { "tablet idle" } else { "mouse" }),
        };
        ui.end_row();
        ui.label(egui::RichText::new("File").weak());
        match &entry.doc.path {
            Some(p) => ui.label(p.display().to_string()).on_hover_text(p.display().to_string()),
            None => ui.label("unsaved"),
        };
        ui.end_row();
        ui.label(egui::RichText::new("History").weak());
        ui.label(format!("{} / {}", entry.doc.history.cursor() + 1, entry.doc.history.len()));
        ui.end_row();
    });
    let _ = rgba_to_color32;
}
