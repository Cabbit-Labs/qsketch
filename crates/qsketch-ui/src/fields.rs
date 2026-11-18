//! Inputs that keep their size. A field, a dropdown, a slider's value, a list row or a label is as wide as the layout gives it, never
//! as wide as its text, and changes size only when the user resizes what holds it. Text that does not fit scrolls while it is typed and
//! ends in an ellipsis when shown (its whole text on hover is the caller's). Each input is laid out in a rectangle reserved before it
//! is drawn and clipped to it, so a long name or a longer number cannot push its neighbors along or widen its panel.
//!
//! egui sizes a dropdown to its chosen text, a number to its digits and a field to what it holds unless told otherwise: a toolbar's
//! "Size 8 px" became "Size 120 px" and moved everything after it. A number with a range is given the width its widest value prints
//! at ([`range_width`]), so it never needs cutting; one without a range gets a width chosen by its caller.

use egui::{
    Align, ComboBox, DragValue, InnerResponse, Layout, Response, Sense, Slider, TextEdit, TextStyle, Ui, UiBuilder,
    Vec2, WidgetText,
};

/// A field's height: the look's interact height.
pub fn height(ui: &Ui) -> f32 {
    ui.spacing().interact_size.y
}

/// How wide `text` is in `style`, in points.
pub fn text_width(ui: &Ui, text: &str, style: TextStyle) -> f32 {
    let font = style.resolve(ui.style());
    let color = ui.visuals().text_color();
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font, color)).size().x
}

/// The width a number field needs to show each of `samples` whole (its padding included).
pub fn width_for(ui: &Ui, samples: &[String]) -> f32 {
    let widest = samples.iter().map(|s| text_width(ui, s, TextStyle::Button)).fold(0.0, f32::max);
    (widest + ui.spacing().button_padding.x * 2.0 + 2.0).ceil().max(ui.spacing().interact_size.x)
}

/// The width a number field over `lo..=hi` needs, shown with `decimals` places between `prefix` and `suffix`: the widest of its ends
/// (and of a negative zero's sign room, when the range crosses zero).
pub fn range_width(ui: &Ui, lo: f64, hi: f64, decimals: usize, prefix: &str, suffix: &str) -> f32 {
    let show = |v: f64| format!("{prefix}{v:.decimals$}{suffix}");
    width_for(ui, &[show(lo), show(hi)])
}

/// Runs `add` in a rectangle `w` points wide reserved in `ui`'s layout and clipped to it: whatever `add` draws, the row around it
/// stays where it is.
pub fn fixed<R>(ui: &mut Ui, w: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let w = w.max(4.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, height(ui)), Sense::hover());
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    add(&mut child)
}

/// A number field `w` points wide.
pub fn number(ui: &mut Ui, w: f32, value: DragValue<'_>) -> Response {
    fixed(ui, w, |ui| {
        let h = height(ui);
        ui.add_sized([w, h], value)
    })
}

/// A slider whose track is the look's slider width and whose value box is `value_w` points wide (see [`range_width`]; 0 for a slider
/// that shows no value).
pub fn slider(ui: &mut Ui, value_w: f32, slider: Slider<'_>) -> Response {
    let gap = ui.spacing().item_spacing.x;
    let w = ui.spacing().slider_width + if value_w > 0.0 { gap + value_w } else { 0.0 };
    fixed(ui, w, |ui| ui.add(slider))
}

/// A one-line text field `w` points wide; a longer text scrolls inside it.
pub fn text(ui: &mut Ui, w: f32, value: &mut dyn egui::TextBuffer) -> Response {
    text_edit(ui, w, TextEdit::singleline(value))
}

/// A one-line `TextEdit` (with its hint or font set by the caller) `w` points wide; a longer text scrolls inside it.
pub fn text_edit(ui: &mut Ui, w: f32, edit: TextEdit<'_>) -> Response {
    fixed(ui, w, |ui| {
        let pad = ui.spacing().button_padding.x * 2.0;
        let h = height(ui);
        ui.add_sized([w, h], edit.desired_width(w - pad).clip_text(true))
    })
}

/// A dropdown `w` points wide, its chosen text cut short with an ellipsis when it does not fit, and the rows of its list cut too.
pub fn combo<R>(
    ui: &mut Ui,
    id: impl egui::AsIdSalt,
    w: f32,
    chosen: impl Into<WidgetText>,
    menu: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<Option<R>> {
    fixed(ui, w, |ui| {
        ComboBox::from_id_salt(id).width(w).truncate().selected_text(chosen).show_ui(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            ui.set_max_width(w.max(160.0));
            menu(ui)
        })
    })
}

/// A row of a list to pick from, `w` points wide, its text at the left and cut short with an ellipsis; framed while picked.
pub fn pick(ui: &mut Ui, w: f32, on: bool, text: impl Into<WidgetText>) -> Response {
    fixed(ui, w, |ui| {
        let h = height(ui);
        // a spacer that grows after the text: sized to the row, a button centers its text
        ui.add_sized([w, h], egui::Button::selectable(on, text).frame_when_inactive(on).truncate().right_text(""))
    })
}

/// A label `w` points wide, cut short with an ellipsis.
pub fn label(ui: &mut Ui, w: f32, text: impl Into<WidgetText>) -> Response {
    fixed(ui, w, |ui| ui.add(egui::Label::new(text).truncate()))
}

/// `text` laid out to fit `w` points in `font` (an ellipsis at the end when cut), for text a painter draws.
pub fn elide(ui: &Ui, text: &str, font: egui::FontId, color: egui::Color32, w: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(w.max(1.0));
    ui.fonts_mut(|f| f.layout_job(job))
}

/// A number field as wide as `sample` prints (its widest value, "-88888.8 px" say), so it never needs cutting.
pub fn number_as(ui: &mut Ui, sample: &str, value: DragValue<'_>) -> Response {
    let w = width_for(ui, &[sample.to_string()]);
    number(ui, w, value)
}

/// A slider whose value box is as wide as `sample` prints (no box for an empty sample), with room after it for its `text` label.
pub fn slider_as(ui: &mut Ui, sample: &str, text: &str, value: Slider<'_>) -> Response {
    let gap = ui.spacing().item_spacing.x;
    let mut w = if sample.is_empty() { 0.0 } else { width_for(ui, &[sample.to_string()]) };
    if !text.is_empty() {
        w += gap + text_width(ui, text, TextStyle::Body).ceil() + 1.0;
    }
    slider(ui, w, value)
}

/// A one-line text field the look's text-field width wide.
pub fn text_default(ui: &mut Ui, value: &mut dyn egui::TextBuffer) -> Response {
    let w = ui.spacing().text_edit_width;
    text(ui, w, value)
}

/// A dropdown the look's dropdown width wide.
pub fn combo_default<R>(
    ui: &mut Ui,
    id: impl egui::AsIdSalt,
    chosen: impl Into<WidgetText>,
    menu: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<Option<R>> {
    let w = ui.spacing().combo_width;
    combo(ui, id, w, chosen, menu)
}
