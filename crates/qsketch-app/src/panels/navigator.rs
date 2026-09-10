//! Navigator: document overview with the visible region, plus zoom controls.

use egui::{Color32, Sense, Ui};
use qsketch_core::Pt;

use crate::canvas::view::{MAX_ZOOM, MIN_ZOOM};
use crate::state::AppState;
use crate::ui::icons;
use crate::ui::widgets::icon_button;

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let Some(doc_id) = state.active_doc else {
        ui.centered_and_justified(|ui| ui.label(egui::RichText::new("No document").weak()));
        return;
    };
    let ctx = ui.ctx().clone();
    let AppState { docs, thumbs, .. } = state;
    let Some(entry) = docs.iter_mut().find(|d| d.id == doc_id) else { return };
    let (dw, dh) = (entry.doc.width(), entry.doc.height());

    let avail = ui.available_size();
    let max_w = avail.x.max(64.0) as usize;
    let max_h = (avail.y - 60.0).max(64.0) as usize;
    let tex = thumbs.get(&ctx, (doc_id, u64::MAX), entry.generation, [max_w.min(512), max_h.min(512)], |m| {
        crate::panels::thumbs::composite_thumb(&entry.doc.composite, m)
    });
    let size = tex.size_vec2();
    let scale = (avail.x / size.x).min((avail.y - 60.0).max(40.0) / size.y).min(1.0);
    let draw_size = size * scale;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(avail.x, draw_size.y), Sense::click_and_drag());
    let img_rect = egui::Rect::from_center_size(rect.center(), draw_size);
    ui.painter().image(
        tex.id(),
        img_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    ui.painter().rect_stroke(
        img_rect,
        0,
        egui::Stroke::new(1.0, Color32::from_black_alpha(160)),
        egui::StrokeKind::Outside,
    );

    // Visible region polygon (viewport corners mapped to doc space).
    let view = &entry.view;
    let to_thumb = |p: Pt| {
        egui::pos2(
            img_rect.left() + p.x / dw as f32 * img_rect.width(),
            img_rect.top() + p.y / dh as f32 * img_rect.height(),
        )
    };
    if view.initialized {
        let vp = view.viewport;
        let corners = [vp.left_top(), vp.right_top(), vp.right_bottom(), vp.left_bottom()];
        let pts: Vec<egui::Pos2> = corners.iter().map(|c| to_thumb(view.screen_to_doc(*c))).collect();
        ui.painter()
            .with_clip_rect(img_rect.expand(1.0))
            .add(egui::Shape::closed_line(pts, egui::Stroke::new(1.5, Color32::from_rgb(235, 70, 70))));
    }
    // Respond from the moment the button goes down (not only once egui detects
    // a drag or a completed click), as long as the press started on the preview.
    let pressed_on_preview = resp.is_pointer_button_down_on()
        && ui.input(|i| i.pointer.press_origin()).is_some_and(|o| img_rect.contains(o));
    if pressed_on_preview || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let dx = ((p.x - img_rect.left()) / img_rect.width()).clamp(0.0, 1.0) * dw as f32;
            let dy = ((p.y - img_rect.top()) / img_rect.height()).clamp(0.0, 1.0) * dh as f32;
            entry.view.center = Pt::new(dx, dy);
        }
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if icon_button(ui, icons::MAGNIFYING_GLASS_MINUS, "Zoom out", 22.0, false).clicked() {
            entry.view.zoom_out(None);
        }
        let mut z = entry.view.zoom;
        if ui.add(egui::Slider::new(&mut z, MIN_ZOOM..=MAX_ZOOM).logarithmic(true).show_value(false)).changed() {
            entry.view.set_zoom(z, None);
        }
        if icon_button(ui, icons::MAGNIFYING_GLASS_PLUS, "Zoom in", 22.0, false).clicked() {
            entry.view.zoom_in(None);
        }
        let mut pct = entry.view.zoom * 100.0;
        if ui
            .add(egui::DragValue::new(&mut pct).suffix("%").speed(1.0).range(MIN_ZOOM * 100.0..=MAX_ZOOM * 100.0))
            .changed()
        {
            entry.view.set_zoom(pct / 100.0, None);
        }
    });
    ui.horizontal(|ui| {
        if ui.small_button("Fit").clicked() {
            entry.view.fit(dw, dh);
        }
        if ui.small_button("100%").clicked() {
            entry.view.set_zoom(1.0, None);
        }
        if ui.small_button("200%").clicked() {
            entry.view.set_zoom(2.0, None);
        }
        if entry.view.rotation != 0.0
            && ui
                .small_button(format!("{} {:.0}°", icons::ARROW_COUNTER_CLOCKWISE, entry.view.rotation_degrees()))
                .on_hover_text("Reset rotation")
                .clicked()
        {
            entry.view.reset_rotation();
        }
        ui.label(egui::RichText::new(format!("{dw}×{dh}")).weak());
    });
}
