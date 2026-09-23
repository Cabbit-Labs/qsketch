//! Small reusable widgets.

use egui::{Color32, Response, RichText, Sense, Ui, Vec2, Widget};
use qsketch_core::Rgba8;

pub fn rgba_to_color32(c: Rgba8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

#[allow(dead_code)]
pub fn color32_to_rgba(c: Color32) -> Rgba8 {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Rgba8::new(r, g, b, a)
}

/// An icon glyph as rich text in the icon font.
pub fn icon(glyph: &str, size: f32) -> RichText {
    RichText::new(glyph).family(super::iconset::family()).size(size)
}

/// A compact button that keeps its size on hover. egui's `Button::small`
/// drops the vertical padding but still adds the hovered stroke width to the
/// frame, so with an unstroked idle state (this theme) the button grows two
/// pixels on hover and shoves everything below it. Small text in a normal
/// frame stays put.
pub fn small_button(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.add(egui::Button::new(RichText::new(text.into()).small()))
}

/// Square icon button with tooltip; `selected` renders it in the accent state.
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str, size: f32, selected: bool) -> Response {
    let btn = egui::Button::new(icon(glyph, size * 0.62))
        .min_size(Vec2::splat(size))
        .corner_radius(4)
        .selected(selected)
        // Flat at rest, but light up on hover so it reads as clickable.
        .frame(true)
        .frame_when_inactive(selected);
    let r = ui.add(btn);
    if tooltip.is_empty() {
        r
    } else {
        r.on_hover_text(tooltip)
    }
}

/// Small icon toggle (e.g. eye / lock) drawn without a frame.
pub fn icon_toggle(
    ui: &mut Ui,
    glyph_on: &str,
    glyph_off: &str,
    value: &mut bool,
    tooltip: &str,
    size: f32,
) -> Response {
    let glyph = if *value { glyph_on } else { glyph_off };
    let text = icon(glyph, size * 0.7).color(if *value {
        ui.visuals().text_color()
    } else {
        crate::ui::theme::dim_text(ui.visuals())
    });
    let r = ui.add(egui::Button::new(text).frame(true).frame_when_inactive(false).min_size(Vec2::splat(size)));
    if r.clicked() {
        *value = !*value;
    }
    if tooltip.is_empty() {
        r
    } else {
        r.on_hover_text(tooltip)
    }
}

/// A labelled slider that also accepts typed values. `suffix` e.g. "%" or "px".
#[allow(dead_code)]
pub fn labeled_slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
    log: bool,
) -> Response {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut s =
            egui::Slider::new(value, range).suffix(suffix).logarithmic(log).clamping(egui::SliderClamping::Always);
        if suffix == "%" {
            s = s.fixed_decimals(0);
        }
        ui.add(s)
    })
    .inner
}

/// Percentage slider over a `0..=1` float, shown as 0–100%.
pub fn percent_slider(ui: &mut Ui, label: &str, value: &mut f32) -> bool {
    percent_slider_tip(ui, label, value, "")
}

/// `percent_slider` with a tooltip on the label and the slider.
pub fn percent_slider_tip(ui: &mut Ui, label: &str, value: &mut f32, tip: &str) -> bool {
    let mut pct = (*value * 100.0).round();
    let r = ui.horizontal(|ui| {
        let l = ui.label(label);
        let s = ui.add(
            egui::Slider::new(&mut pct, 0.0..=100.0)
                .suffix("%")
                .fixed_decimals(0)
                .clamping(egui::SliderClamping::Always),
        );
        if !tip.is_empty() {
            l.on_hover_text(tip);
            return s.on_hover_text(tip);
        }
        s
    });
    let mut changed = r.inner.changed();
    // The wheel nudges the value while the pointer is over the slider (1 % a
    // notch, 10 % with Shift), and the enclosing scroll area does not see it.
    if r.inner.hovered() {
        if let Some(notch) = wheel_notch(ui) {
            let step = if ui.input(|i| i.modifiers.shift) { 10.0 } else { 1.0 };
            pct = (pct + notch * step).clamp(0.0, 100.0);
            changed = true;
        }
    }
    if changed {
        *value = (pct / 100.0).clamp(0.0, 1.0);
        true
    } else {
        false
    }
}

/// Take this frame's wheel movement as a signed notch count (+ = wheel up),
/// consuming it so nothing behind the widget scrolls. `None` when the wheel
/// did not move.
pub fn wheel_notch(ui: &Ui) -> Option<f32> {
    let dy = ui.input_mut(|i| {
        let d = i.smooth_scroll_delta.y;
        if d != 0.0 {
            i.smooth_scroll_delta = egui::Vec2::ZERO;
        }
        d
    });
    (dy != 0.0).then(|| dy.signum())
}

/// A color swatch rectangle with checkerboard behind transparent colors.
pub struct Swatch {
    pub color: Rgba8,
    pub size: Vec2,
    pub selected: bool,
}

impl Widget for Swatch {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, resp) = ui.allocate_exact_size(self.size, Sense::click());
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            if self.color.a < 255 {
                checkerboard(p, rect, 4.0);
            }
            p.rect_filled(rect, 2, rgba_to_color32(self.color));
            let stroke = if self.selected {
                egui::Stroke::new(2.0, ui.visuals().selection.stroke.color)
            } else if resp.hovered() {
                egui::Stroke::new(1.0, ui.visuals().widgets.hovered.fg_stroke.color)
            } else {
                egui::Stroke::new(1.0, Color32::from_black_alpha(120))
            };
            p.rect_stroke(rect, 2, stroke, egui::StrokeKind::Inside);
        }
        resp
    }
}

pub fn checkerboard(p: &egui::Painter, rect: egui::Rect, cell: f32) {
    p.rect_filled(rect, 0, Color32::from_gray(200));
    let mut y = rect.top();
    let mut row = 0;
    while y < rect.bottom() {
        let mut x = rect.left() + if row % 2 == 0 { 0.0 } else { cell };
        while x < rect.right() {
            let r = egui::Rect::from_min_size(egui::pos2(x, y), Vec2::splat(cell)).intersect(rect);
            p.rect_filled(r, 0, Color32::from_gray(255));
            x += cell * 2.0;
        }
        y += cell;
        row += 1;
    }
}

/// Nudge `value` by `step` per wheel notch while `r` is hovered, staying in
/// `range`. Marks the response changed so callers see it like a drag.
pub fn wheel_adjust(ui: &Ui, r: &mut Response, value: &mut f32, step: f32, range: std::ops::RangeInclusive<f32>) {
    if !r.hovered() {
        return;
    }
    // Whole notches from the raw wheel events, which are then taken so a
    // scroll area around the widget does not scroll as well.
    let mut notches = 0.0;
    ui.input_mut(|i| {
        i.events.retain(|e| match e {
            egui::Event::MouseWheel { delta, .. } if delta.y != 0.0 => {
                notches += delta.y.signum();
                false
            }
            _ => true,
        });
    });
    if notches == 0.0 {
        return;
    }
    let next = (*value + notches * step).clamp(*range.start(), *range.end());
    if next != *value {
        *value = next;
        r.mark_changed();
    }
}

/// Compact numeric control for toolbars: `Label 80 px`, draggable and
/// click-to-type, with a thin fill bar underneath showing the position in
/// `range`. Replaces a full slider at about a third of the width.
pub fn param(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
    log: bool,
    decimals: usize,
) -> Response {
    let (lo, hi) = (*range.start(), *range.end());
    let speed = if log { (*value * 0.02).max(0.05) } else { (hi - lo) / 200.0 };
    let mut r = ui.add(
        egui::DragValue::new(value)
            .range(range.clone())
            .speed(speed as f64)
            .prefix(format!("{label} "))
            .suffix(suffix)
            .fixed_decimals(decimals),
    );
    // The wheel steps it too: one unit for whole numbers, a hundredth of the
    // range otherwise, and a tenth of the value on logarithmic ranges.
    let unit = 10f32.powi(-(decimals as i32));
    let step = if log { (*value * 0.1).max(unit) } else { ((hi - lo) / 100.0).max(unit) };
    let step = (step / unit).round().max(1.0) * unit;
    wheel_adjust(ui, &mut r, value, step, range);
    if r.changed() {
        *value = (*value / unit).round() * unit;
    }
    // Position bar along the bottom edge of the widget.
    let t = if log {
        ((value.max(lo.max(1e-3)) / lo.max(1e-3)).ln() / (hi / lo.max(1e-3)).ln()).clamp(0.0, 1.0)
    } else {
        ((*value - lo) / (hi - lo)).clamp(0.0, 1.0)
    };
    let rect = r.rect;
    let bar = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 2.0, rect.bottom() - 2.0),
        rect.right_bottom() - Vec2::new(2.0, 0.0),
    );
    let p = ui.painter();
    p.rect_filled(bar, 1, ui.visuals().widgets.noninteractive.bg_stroke.color);
    let fill = egui::Rect::from_min_max(bar.min, egui::pos2(bar.left() + bar.width() * t, bar.bottom()));
    // Faint: this is a position hint, not a highlight.
    p.rect_filled(fill, 1, ui.visuals().selection.stroke.color.gamma_multiply(0.35));
    r
}

/// Percent variant of [`param`] over a `0..=1` float.
pub fn param_pct(ui: &mut Ui, label: &str, value: &mut f32) -> bool {
    let mut pct = (*value * 100.0).round();
    let r = param(ui, label, &mut pct, 0.0..=100.0, "%", false, 0);
    if r.changed() {
        *value = (pct / 100.0).clamp(0.0, 1.0);
        true
    } else {
        false
    }
}

/// A framed group in a toolbar: a quiet tinted pill with a faint section-colored
/// left edge. The edge is deliberately subtle so several chips in a row read as
/// one bar rather than a run of colored dividers.
pub fn chip<R>(ui: &mut Ui, color: Color32, tint: Color32, contents: impl FnOnce(&mut Ui) -> R) -> R {
    let frame = egui::Frame::new().fill(tint).corner_radius(4).inner_margin(egui::Margin {
        left: 6,
        right: 5,
        top: 1,
        bottom: 1,
    });
    let r = frame.show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.horizontal(|ui| contents(ui)).inner
    });
    let rect = r.response.rect;
    let edge = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 3.0),
        egui::pos2(rect.left() + 1.5, rect.bottom() - 3.0),
    );
    ui.painter().rect_filled(edge, 1, color.gamma_multiply(0.35));
    r.inner
}

/// Section header inside panels.
#[allow(dead_code)]
pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).small().color(crate::ui::theme::dim_text(ui.visuals())).strong());
    ui.separator();
}

/// Read-only key-cap style label for shortcuts.
pub fn keycap(ui: &mut Ui, text: &str) -> Response {
    let text = if text.is_empty() { "—" } else { text };
    ui.add(
        egui::Label::new(RichText::new(text).monospace().small().color(crate::ui::theme::dim_text(ui.visuals())))
            .sense(Sense::hover()),
    )
}

/// Width of the always-visible scroll bar drawn by [`scroll_left_bar`].
pub const LEFT_BAR_W: f32 = 14.0;

/// A vertical scroll area with a solid, always-visible scroll bar on its
/// **left** edge instead of egui's auto-hiding one on the right. The bar is
/// wide enough to grab with a pen; a drag on the handle scrolls, a tap on the
/// track jumps there. egui only ever draws its bars on the right, so the
/// built-in bar is hidden and this one is painted from the area's state.
pub fn scroll_left_bar<R>(
    ui: &mut Ui,
    id_salt: &str,
    max_height: f32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    let outer = ui.available_rect_before_wrap();
    let bar_rect = egui::Rect::from_min_size(outer.min, egui::vec2(LEFT_BAR_W, max_height.min(outer.height())));
    let list_rect = egui::Rect::from_min_max(egui::pos2(outer.min.x + LEFT_BAR_W + 2.0, outer.min.y), outer.max);

    let out = ui
        .scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(max_height)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .id_salt(id_salt)
                .show(ui, add_contents)
        })
        .inner;

    // The bar spans exactly the height the list ended up with.
    let track = egui::Rect::from_min_size(bar_rect.min, egui::vec2(LEFT_BAR_W, out.inner_rect.height()));
    let view_h = out.inner_rect.height();
    let content_h = out.content_size.y.max(view_h);
    let max_off = (content_h - view_h).max(0.0);
    let mut offset = out.state.offset.y.clamp(0.0, max_off);

    let handle_h = (track.height() * view_h / content_h).clamp(24.0_f32.min(track.height()), track.height());
    let travel = (track.height() - handle_h).max(0.0);
    let handle_top = |off: f32| track.top() + if max_off > 0.0 { off / max_off * travel } else { 0.0 };
    let handle =
        egui::Rect::from_min_size(egui::pos2(track.left(), handle_top(offset)), egui::vec2(LEFT_BAR_W, handle_h));

    let resp = ui.interact(track, ui.id().with("left_scroll_bar"), Sense::click_and_drag());
    if max_off > 0.0 && travel > 0.0 {
        if resp.drag_started() {
            // Grab the handle where it is; a press on the bare track pulls the
            // handle under the pen first so the drag then continues from there.
            if let Some(p) = resp.interact_pointer_pos() {
                if !handle.contains(p) {
                    offset = ((p.y - track.top() - handle_h * 0.5) / travel * max_off).clamp(0.0, max_off);
                }
            }
        }
        if resp.dragged() {
            offset = (offset + resp.drag_delta().y / travel * max_off).clamp(0.0, max_off);
        } else if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                offset = ((p.y - track.top() - handle_h * 0.5) / travel * max_off).clamp(0.0, max_off);
            }
        }
    }
    if offset != out.state.offset.y {
        let mut st = out.state;
        st.offset.y = offset;
        st.store(ui.ctx(), out.id);
        ui.ctx().request_repaint();
    }

    // Paint: a solid track and a handle that follows the widget visuals, so
    // it reads as one control in every theme.
    let v = ui.visuals();
    let p = ui.painter();
    p.rect_filled(track, 3.0, v.extreme_bg_color);
    let handle =
        egui::Rect::from_min_size(egui::pos2(track.left(), handle_top(offset)), egui::vec2(LEFT_BAR_W, handle_h));
    let w = if resp.dragged() {
        &v.widgets.active
    } else if resp.hovered() {
        &v.widgets.hovered
    } else {
        &v.widgets.inactive
    };
    let handle_fill =
        if max_off > 0.0 { w.fg_stroke.color.gamma_multiply(0.55) } else { w.bg_fill.gamma_multiply(0.6) };
    p.rect_filled(handle.shrink2(egui::vec2(3.0, 2.0)), 3.0, handle_fill);
    // Keep the bar inside the parent's layout so nothing overlaps it.
    ui.allocate_rect(track, Sense::hover());
    out
}
