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
    let text =
        icon(glyph, size * 0.7).color(if *value { ui.visuals().text_color() } else { ui.visuals().weak_text_color() });
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
    let mut pct = (*value * 100.0).round();
    let r = ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::Slider::new(&mut pct, 0.0..=100.0)
                .suffix("%")
                .fixed_decimals(0)
                .clamping(egui::SliderClamping::Always),
        )
    });
    if r.inner.changed() {
        *value = (pct / 100.0).clamp(0.0, 1.0);
        true
    } else {
        false
    }
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
    let r = ui.add(
        egui::DragValue::new(value)
            .range(range)
            .speed(speed as f64)
            .prefix(format!("{label} "))
            .suffix(suffix)
            .fixed_decimals(decimals),
    );
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
    ui.label(RichText::new(title).small().color(ui.visuals().weak_text_color()).strong());
    ui.separator();
}

/// Read-only key-cap style label for shortcuts.
pub fn keycap(ui: &mut Ui, text: &str) -> Response {
    let text = if text.is_empty() { "—" } else { text };
    ui.add(
        egui::Label::new(RichText::new(text).monospace().small().color(ui.visuals().weak_text_color()))
            .sense(Sense::hover()),
    )
}
