//! Layer ▸ Layer Style…: turn layer effects on and tune them with a live
//! preview. The dialog edits the layer's style in the working state (the
//! compositor redraws it at once); OK commits one undo step, Cancel puts the
//! old style back.

use egui::{Color32, Context, RichText, Ui};
use qsketch_core::layer::LayerId;
use qsketch_core::style::{LayerStyle, StrokePosition};
use qsketch_core::Rgba8;

use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    DropShadow,
    OuterGlow,
    Stroke,
    ColorOverlay,
    InnerGlow,
}

impl Page {
    const ALL: [Page; 5] = [Page::DropShadow, Page::OuterGlow, Page::Stroke, Page::ColorOverlay, Page::InnerGlow];

    fn label(self) -> &'static str {
        match self {
            Page::DropShadow => "Drop Shadow",
            Page::OuterGlow => "Outer Glow",
            Page::Stroke => "Stroke",
            Page::ColorOverlay => "Color Overlay",
            Page::InnerGlow => "Inner Glow",
        }
    }

    fn enabled(self, s: &mut LayerStyle) -> &mut bool {
        match self {
            Page::DropShadow => &mut s.drop_shadow.enabled,
            Page::OuterGlow => &mut s.outer_glow.enabled,
            Page::Stroke => &mut s.stroke.enabled,
            Page::ColorOverlay => &mut s.color_overlay.enabled,
            Page::InnerGlow => &mut s.inner_glow.enabled,
        }
    }
}

pub struct LayerStyleDialog {
    pub doc: DocId,
    pub layer: LayerId,
    /// The style when the dialog opened, restored by Cancel.
    pub original: LayerStyle,
    pub style: LayerStyle,
    pub page: Page,
}

/// Open the dialog on the active layer.
pub fn open(state: &mut AppState) {
    state.settle();
    let Some(e) = state.active() else { return };
    let l = e.doc.state().active_layer();
    if !l.owns_pixels() {
        state.toasts.push(Level::Info, "Layer styles apply to pixel layers, not groups or adjustment layers.");
        return;
    }
    let style = l.props.style.clone();
    let page = Page::ALL.into_iter().find(|p| *p.enabled(&mut style.clone())).unwrap_or(Page::DropShadow);
    state.dialogs.layer_style =
        Some(LayerStyleDialog { doc: e.id, layer: l.props.id, original: style.clone(), style, page });
}

/// Layer › Outline: the layer style dialog on its Stroke page with the
/// outline switched on in the foreground color, the non-destructive
/// counterpart of Aseprite's Outline.
pub fn open_outline(state: &mut AppState) {
    let fg = state.fg;
    open(state);
    if let Some(d) = state.dialogs.layer_style.as_mut() {
        if !d.style.stroke.enabled {
            d.style.stroke.enabled = true;
            d.style.stroke.color = fg;
            d.style.stroke.size = 1.0;
            d.style.stroke.position = qsketch_core::style::StrokePosition::Outside;
        }
        d.page = Page::Stroke;
        let (doc, layer, style) = (d.doc, d.layer, d.style.clone());
        preview(state, doc, layer, &style);
    }
}

/// Set a layer's style in the working state (no undo step) and redraw.
fn preview(state: &mut AppState, doc: DocId, layer: LayerId, style: &LayerStyle) -> bool {
    let Some(entry) = state.doc_mut(doc) else { return false };
    let s = entry.doc.state_mut();
    let Some(i) = s.index_of(layer) else { return false };
    if s.layers[i].props.style != *style {
        s.layers[i].props.style = style.clone();
        entry.doc.mark_all_dirty();
    }
    true
}

pub fn show(ctx: &Context, state: &mut AppState) {
    let Some(mut d) = state.dialogs.layer_style.take() else { return };
    let palette = state.settings.ui.palette();
    let (mut ok, mut cancel, mut open) = (false, false, true);
    let screen = ctx.content_rect();
    egui::Window::new("Layer Style")
        .id(egui::Id::new("layer_style_dialog"))
        .open(&mut open)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .default_pos(egui::pos2(screen.right() - 560.0, screen.top() + 80.0))
        .show(ctx, |ui| {
            ui.set_width(500.0);
            if crate::ui::chrome::window_header(ui, &palette, "Layer Style") {
                cancel = true;
            }
            ui.horizontal_top(|ui| {
                // The divider between the lists takes all the height it is
                // offered; keep the row to its content so OK / Cancel stay put.
                ui.set_max_height(230.0);
                ui.vertical(|ui| {
                    ui.set_width(150.0);
                    for p in Page::ALL {
                        ui.horizontal(|ui| {
                            let on = p.enabled(&mut d.style);
                            if ui.checkbox(on, "").changed() {
                                d.page = p;
                            }
                            if ui.selectable_label(d.page == p, p.label()).clicked() {
                                d.page = p;
                            }
                        });
                    }
                    ui.add_space(8.0);
                    if crate::ui::widgets::small_button(ui, "Turn all off").clicked() {
                        for p in Page::ALL {
                            *p.enabled(&mut d.style) = false;
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_min_height(220.0);
                    ui.label(RichText::new(d.page.label()).strong());
                    ui.add_space(4.0);
                    page_ui(ui, &mut d);
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Effects follow the layer's shape and never change its pixels.").weak().small());
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
    if !preview(state, d.doc, d.layer, &d.style) {
        return; // the layer is gone (deleted, undone): drop the dialog
    }
    if ok {
        if d.style != d.original {
            if let Some(entry) = state.doc_mut(d.doc) {
                entry.doc.commit(if d.style.is_off() { "Clear Layer Style" } else { "Layer Style" });
            }
        }
        return;
    }
    state.dialogs.layer_style = Some(d);
}

fn color_row(ui: &mut Ui, label: &str, c: &mut Rgba8) {
    ui.label(label);
    let mut c32 = Color32::from_rgb(c.r, c.g, c.b);
    if egui::color_picker::color_edit_button_srgba(ui, &mut c32, egui::color_picker::Alpha::Opaque).changed() {
        *c = Rgba8::new(c32.r(), c32.g(), c32.b(), 255);
    }
    ui.end_row();
}

fn pct_row(ui: &mut Ui, label: &str, v: &mut f32) {
    ui.label(label);
    let mut p = (*v * 100.0).round();
    if ui.add(egui::Slider::new(&mut p, 0.0..=100.0).suffix("%").fixed_decimals(0)).changed() {
        *v = p / 100.0;
    }
    ui.end_row();
}

fn px_row(ui: &mut Ui, label: &str, v: &mut f32, max: f32) {
    ui.label(label);
    ui.add(egui::Slider::new(v, 0.0..=max).suffix(" px").fixed_decimals(0));
    ui.end_row();
}

fn page_ui(ui: &mut Ui, d: &mut LayerStyleDialog) {
    let s = &mut d.style;
    // Editing a page turns its effect on, as in Photoshop.
    let before = s.clone();
    egui::Grid::new("layer_style_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| match d.page {
        Page::DropShadow => {
            let e = &mut s.drop_shadow;
            color_row(ui, "Color", &mut e.color);
            pct_row(ui, "Opacity", &mut e.opacity);
            ui.label("Angle");
            ui.add(egui::DragValue::new(&mut e.angle).suffix("°").speed(1.0).range(-180.0..=360.0));
            ui.end_row();
            px_row(ui, "Distance", &mut e.distance, 100.0);
            pct_row(ui, "Spread", &mut e.spread);
            px_row(ui, "Size", &mut e.size, 100.0);
        }
        Page::OuterGlow => {
            let e = &mut s.outer_glow;
            color_row(ui, "Color", &mut e.color);
            pct_row(ui, "Opacity", &mut e.opacity);
            pct_row(ui, "Spread", &mut e.spread);
            px_row(ui, "Size", &mut e.size, 100.0);
        }
        Page::Stroke => {
            let e = &mut s.stroke;
            color_row(ui, "Color", &mut e.color);
            pct_row(ui, "Opacity", &mut e.opacity);
            px_row(ui, "Size", &mut e.size, 50.0);
            ui.label("Position");
            egui::ComboBox::from_id_salt("stroke_pos")
                .selected_text(match e.position {
                    StrokePosition::Outside => "Outside",
                    StrokePosition::Inside => "Inside",
                    StrokePosition::Center => "Center",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut e.position, StrokePosition::Outside, "Outside");
                    ui.selectable_value(&mut e.position, StrokePosition::Inside, "Inside");
                    ui.selectable_value(&mut e.position, StrokePosition::Center, "Center");
                });
            ui.end_row();
        }
        Page::ColorOverlay => {
            let e = &mut s.color_overlay;
            color_row(ui, "Color", &mut e.color);
            pct_row(ui, "Opacity", &mut e.opacity);
        }
        Page::InnerGlow => {
            let e = &mut s.inner_glow;
            color_row(ui, "Color", &mut e.color);
            pct_row(ui, "Opacity", &mut e.opacity);
            px_row(ui, "Size", &mut e.size, 100.0);
        }
    });
    if *s != before {
        *d.page.enabled(s) = true;
    }
}
