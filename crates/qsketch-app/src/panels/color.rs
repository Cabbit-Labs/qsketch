//! Color panel: Photoshop-style saturation/value square with a hue strip,
//! RGB/HSV sliders and hex entry, editing the foreground or background color.

use egui::{Color32, Mesh, Pos2, Rect, Sense, Ui, Vec2};
use qsketch_core::{Hsv, Rgba8};

use crate::dialogs::ColorTarget;
use crate::state::AppState;
use crate::ui::widgets::{rgba_to_color32, Swatch};

#[derive(Clone, Copy)]
struct HsvMemory {
    hsv: Hsv,
    rgb: Rgba8,
}

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let target = state.dialogs.color_target.unwrap_or(ColorTarget::Foreground);
    ui.horizontal(|ui| {
        let fg_sel = target == ColorTarget::Foreground;
        if ui
            .add(Swatch { color: state.fg, size: Vec2::new(28.0, 22.0), selected: fg_sel })
            .on_hover_text("Foreground")
            .clicked()
        {
            state.dialogs.color_target = Some(ColorTarget::Foreground);
        }
        if ui
            .add(Swatch { color: state.bg, size: Vec2::new(28.0, 22.0), selected: !fg_sel })
            .on_hover_text("Background")
            .clicked()
        {
            state.dialogs.color_target = Some(ColorTarget::Background);
        }
        ui.label(egui::RichText::new(if fg_sel { "Foreground" } else { "Background" }).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button(crate::ui::icons::ARROWS_LEFT_RIGHT).on_hover_text("Swap (X)").clicked() {
                std::mem::swap(&mut state.fg, &mut state.bg);
            }
        });
    });

    let target = state.dialogs.color_target.unwrap_or(ColorTarget::Foreground);
    let current = match target {
        ColorTarget::Foreground => state.fg,
        ColorTarget::Background => state.bg,
    };
    // Keep HSV in memory so hue survives when saturation/value hit 0.
    let mem_id = ui.id().with(("hsv_mem", target as u8));
    let mut mem =
        ui.data(|d| d.get_temp::<HsvMemory>(mem_id)).unwrap_or(HsvMemory { hsv: current.to_hsv(), rgb: current });
    if mem.rgb != current {
        mem.hsv = current.to_hsv();
        mem.rgb = current;
    }
    let mut hsv = mem.hsv;
    let alpha = current.a;
    let mut changed = false;

    // --- SV square + hue strip -------------------------------------------
    // Size the picker to the panel: as wide as fits next to the hue strip,
    // but never so tall that the sliders and recent colors fall off the
    // bottom. Below 80 px it stops shrinking and the panel scrolls.
    let strip_w = 18.0;
    let gap = ui.spacing().item_spacing.x;
    let avail_w = ui.available_width();
    // Everything below the picker (sliders, hex, recent colors) is measured
    // each frame and remembered, so the picker takes exactly the height
    // that is left and the panel never needs a scroll bar when it fits.
    let rest_id = ui.id().with("rest_h");
    let rest_h = ui.data(|d| d.get_temp::<f32>(rest_id)).unwrap_or(180.0);
    let avail_h = ui.available_height() - rest_h - ui.spacing().item_spacing.y;
    // The picker fills the width; its height follows the width but yields
    // to a short panel (a wide, short picker beats a tiny square).
    let sv_w = (avail_w - strip_w - gap).clamp(60.0, 400.0);
    let sv_h = sv_w.min(avail_h).clamp(80.0, 400.0);
    let picker_bottom = ui.cursor().top() + sv_h;
    ui.horizontal(|ui| {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(sv_w, sv_h), Sense::click_and_drag());
        paint_sv_square(ui, rect, hsv.h);
        if resp.dragged() || resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                hsv.s = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                hsv.v = 1.0 - ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
                changed = true;
            }
        }
        let marker = Pos2::new(rect.left() + hsv.s * rect.width(), rect.top() + (1.0 - hsv.v) * rect.height());
        ui.painter().circle_stroke(marker, 5.0, egui::Stroke::new(2.0, Color32::BLACK));
        ui.painter().circle_stroke(marker, 5.0, egui::Stroke::new(1.0, Color32::WHITE));

        let (hrect, hresp) = ui.allocate_exact_size(Vec2::new(strip_w, sv_h), Sense::click_and_drag());
        paint_hue_strip(ui, hrect);
        if hresp.dragged() || hresp.clicked() {
            if let Some(p) = hresp.interact_pointer_pos() {
                hsv.h = ((p.y - hrect.top()) / hrect.height()).clamp(0.0, 0.9999) * 360.0;
                changed = true;
            }
        }
        let y = hrect.top() + hsv.h / 360.0 * hrect.height();
        ui.painter().hline(hrect.x_range(), y, egui::Stroke::new(3.0, Color32::BLACK));
        ui.painter().hline(hrect.x_range(), y, egui::Stroke::new(1.0, Color32::WHITE));
    });

    // --- sliders ---------------------------------------------------------
    let mut rgb = current;
    let mut hex = ui.data(|d| d.get_temp::<String>(ui.id().with("hex"))).unwrap_or_else(|| current.to_hex());
    let hex_focused = ui.memory(|m| m.has_focus(ui.id().with("hex_edit")));
    if !hex_focused {
        hex = current.to_hex();
    }
    // Sliders stretch to the panel width (label + slider + value box).
    let slider_w = (ui.available_width() - 8.0 - 16.0 - 58.0).max(40.0);
    ui.spacing_mut().slider_width = slider_w;
    egui::Grid::new("color_sliders").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        let mut h = hsv.h;
        let mut s = hsv.s * 100.0;
        let mut v = hsv.v * 100.0;
        ui.label("H");
        if ui.add(egui::Slider::new(&mut h, 0.0..=360.0).suffix("°").fixed_decimals(0)).changed() {
            hsv.h = h;
            changed = true;
        }
        ui.end_row();
        ui.label("S");
        if ui.add(egui::Slider::new(&mut s, 0.0..=100.0).suffix("%").fixed_decimals(0)).changed() {
            hsv.s = s / 100.0;
            changed = true;
        }
        ui.end_row();
        ui.label("V");
        if ui.add(egui::Slider::new(&mut v, 0.0..=100.0).suffix("%").fixed_decimals(0)).changed() {
            hsv.v = v / 100.0;
            changed = true;
        }
        ui.end_row();
        let mut rgb_changed = false;
        for (label, ch) in [("R", &mut rgb.r), ("G", &mut rgb.g), ("B", &mut rgb.b)] {
            ui.label(label);
            if ui.add(egui::Slider::new(ch, 0..=255)).changed() {
                rgb_changed = true;
            }
            ui.end_row();
        }
        if rgb_changed {
            hsv = rgb.to_hsv();
            changed = true;
        }
        ui.label("Hex");
        let te = ui
            .add(egui::TextEdit::singleline(&mut hex).id(ui.id().with("hex_edit")).desired_width(slider_w.min(110.0)));
        if te.changed() {
            if let Some(c) = Rgba8::from_hex(&hex) {
                hsv = c.to_hsv();
                changed = true;
            }
        }
        ui.end_row();
    });
    ui.data_mut(|d| d.insert_temp(ui.id().with("hex"), hex));

    if changed {
        let new = hsv.to_rgba8(alpha);
        mem = HsvMemory { hsv, rgb: new };
        match target {
            ColorTarget::Foreground => state.fg = new,
            ColorTarget::Background => state.bg = new,
        }
    }
    ui.data_mut(|d| d.insert_temp(mem_id, mem));

    // --- recently used colors ------------------------------------------------
    if !state.color_history.is_empty() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Recent").weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button(crate::ui::icons::X).on_hover_text("Clear recent colors").clicked() {
                    state.color_history.clear();
                }
            });
        });
        let history = state.color_history.clone();
        let mut pick: Option<Rgba8> = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(2.0, 2.0);
            for c in history {
                let r = ui
                    .add(Swatch { color: c, size: Vec2::new(16.0, 16.0), selected: c == current })
                    .on_hover_text(c.to_hex());
                if r.clicked() {
                    pick = Some(c);
                }
                if r.secondary_clicked() {
                    state.bg = c;
                }
            }
        });
        if let Some(c) = pick {
            match target {
                ColorTarget::Foreground => state.fg = c,
                ColorTarget::Background => state.bg = c,
            }
        }
    }
    let rest = (ui.cursor().top() - picker_bottom).max(0.0);
    if (rest - rest_h).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(rest_id, rest));
        ui.ctx().request_repaint();
    }
}

fn paint_sv_square(ui: &Ui, rect: Rect, hue: f32) {
    let hue_rgb = Hsv::new(hue, 1.0, 1.0).to_rgba8(255);
    let hue_c = rgba_to_color32(hue_rgb);
    let mut mesh = Mesh::default();
    // Horizontal: white -> hue; vertical multiply by value (black overlay).
    let cols = [Color32::WHITE, hue_c, Color32::BLACK, Color32::BLACK];
    let pts = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
    for (p, c) in pts.iter().zip(cols) {
        mesh.colored_vertex(*p, c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    // Two-pass approximation: base gradient, then a subdivided vertical black ramp.
    let mut base = Mesh::default();
    base.colored_vertex(rect.left_top(), Color32::WHITE);
    base.colored_vertex(rect.right_top(), hue_c);
    base.colored_vertex(rect.right_bottom(), hue_c);
    base.colored_vertex(rect.left_bottom(), Color32::WHITE);
    base.add_triangle(0, 1, 2);
    base.add_triangle(0, 2, 3);
    ui.painter().add(base);
    let mut dark = Mesh::default();
    dark.colored_vertex(rect.left_top(), Color32::TRANSPARENT);
    dark.colored_vertex(rect.right_top(), Color32::TRANSPARENT);
    dark.colored_vertex(rect.right_bottom(), Color32::BLACK);
    dark.colored_vertex(rect.left_bottom(), Color32::BLACK);
    dark.add_triangle(0, 1, 2);
    dark.add_triangle(0, 2, 3);
    ui.painter().add(dark);
    ui.painter().rect_stroke(
        rect,
        0,
        egui::Stroke::new(1.0, Color32::from_black_alpha(160)),
        egui::StrokeKind::Outside,
    );
}

fn paint_hue_strip(ui: &Ui, rect: Rect) {
    let mut mesh = Mesh::default();
    let n = 12;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let c = rgba_to_color32(Hsv::new(t * 359.99, 1.0, 1.0).to_rgba8(255));
        let y = rect.top() + t * rect.height();
        mesh.colored_vertex(Pos2::new(rect.left(), y), c);
        mesh.colored_vertex(Pos2::new(rect.right(), y), c);
        if i > 0 {
            let b = (i * 2) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    ui.painter().add(mesh);
    ui.painter().rect_stroke(
        rect,
        0,
        egui::Stroke::new(1.0, Color32::from_black_alpha(160)),
        egui::StrokeKind::Outside,
    );
}
