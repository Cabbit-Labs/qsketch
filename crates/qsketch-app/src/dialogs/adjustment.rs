//! Adjustment layer settings: edit the color adjustment an adjustment layer
//! applies, with a live preview. OK commits one undo step; Cancel restores
//! the settings the dialog opened with.

use egui::{Context, RichText};
use qsketch_core::layer::LayerId;
use qsketch_core::Filter;

use crate::state::{AppState, DocId};

pub struct AdjustmentDialog {
    pub doc: DocId,
    pub layer: LayerId,
    pub original: Filter,
    pub filter: Filter,
    /// Histogram of what lies below the layer, for Levels and Curves.
    pub hist: Option<Box<[[u32; 256]; 4]>>,
}

/// Open the settings of the active layer, if it is an adjustment layer.
pub fn open(state: &mut AppState) {
    state.settle();
    let Some(e) = state.active() else { return };
    let s = e.doc.state();
    let l = s.active_layer();
    let Some(filter) = l.props.adjustment.clone().filter(|_| l.is_adjustment()) else {
        state.toasts.push(crate::ui::toasts::Level::Info, "The active layer isn't an adjustment layer.");
        return;
    };
    let hist = matches!(filter, Filter::Levels(_) | Filter::Curves(_)).then(|| {
        let below = qsketch_core::composite::flatten_range(s, 0..s.active, None);
        super::filter::raster_histogram(&below)
    });
    state.dialogs.adjustment =
        Some(AdjustmentDialog { doc: e.id, layer: l.props.id, original: filter.clone(), filter, hist });
}

/// Put `f` on the layer in the working state (no undo step) and redraw.
fn preview(state: &mut AppState, doc: DocId, layer: LayerId, f: &Filter) -> bool {
    let Some(entry) = state.doc_mut(doc) else { return false };
    let s = entry.doc.state_mut();
    let Some(i) = s.index_of(layer) else { return false };
    if !s.layers[i].is_adjustment() {
        return false;
    }
    if s.layers[i].props.adjustment.as_ref() != Some(f) {
        s.layers[i].props.adjustment = Some(f.clone());
        entry.doc.mark_all_dirty();
    }
    true
}

pub fn show(ctx: &Context, state: &mut AppState) {
    let Some(mut d) = state.dialogs.adjustment.take() else { return };
    let palette = state.settings.ui.palette();
    let (mut ok, mut cancel, mut open) = (false, false, true);
    let screen = ctx.content_rect();
    let title = format!("{} (Adjustment Layer)", d.filter.name());
    egui::Window::new(&title)
        .id(egui::Id::new("adjustment_dialog"))
        .open(&mut open)
        .title_bar(false)
        .collapsible(false)
        .auto_sized()
        .default_pos(egui::pos2(screen.right() - 372.0, screen.top() + 80.0))
        .show(ctx, |ui| {
            ui.set_width(320.0);
            if crate::ui::chrome::window_header(ui, &palette, &title) {
                cancel = true;
            }
            ui.label(RichText::new(d.filter.describe()).weak().small());
            ui.add_space(6.0);
            super::filter::adjustment_ui(ui, &mut d.filter, d.hist.as_deref());
            ui.add_space(8.0);
            ui.label(
                RichText::new("Changes everything below this layer; paint its mask to limit where.").weak().small(),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("OK").clicked() {
                        ok = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
        ok = true;
    }
    if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }
    if cancel {
        preview(state, d.doc, d.layer, &d.original);
        return;
    }
    if !preview(state, d.doc, d.layer, &d.filter) {
        return;
    }
    if ok {
        if d.filter != d.original {
            if let Some(entry) = state.doc_mut(d.doc) {
                entry.doc.commit("Adjustment Layer Settings");
            }
        }
        return;
    }
    state.dialogs.adjustment = Some(d);
}
