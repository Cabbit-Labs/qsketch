//! Layers panel: blend/opacity controls, drag-reorderable list with
//! visibility/lock toggles, thumbnails, inline rename and a footer toolbar.

use egui::{Color32, Sense, Ui};
use qsketch_core::BlendMode;

use crate::actions::Action;
use crate::state::AppState;
use crate::ui::icons;
use crate::ui::widgets::{icon_button, icon_toggle, percent_slider};

#[derive(Clone, Copy, PartialEq, Eq)]
struct DragLayer(usize);

const ICON_FAMILY: fn() -> egui::FontFamily = crate::ui::iconset::family;

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let Some(doc_id) = state.active_doc else {
        ui.centered_and_justified(|ui| ui.label(egui::RichText::new("No document").weak()));
        return;
    };
    let ctx = ui.ctx().clone();
    let mut changed_props = false;
    let mut toggled_vis = false;
    let mut rename_target: Option<(usize, String)> = None;
    let mut activate: Option<usize> = None;
    let mut reorder: Option<(usize, usize)> = None;

    // Borrow disjoint fields of the state.
    let AppState { docs, thumbs, pending, keymap, .. } = state;
    let Some(entry) = docs.iter_mut().find(|d| d.id == doc_id) else { return };
    let generation = entry.generation;
    let s = entry.doc.state_mut();
    let active = s.active.min(s.layers.len() - 1);
    let n = s.layers.len();

    // --- header: blend mode + opacity -----------------------------------
    ui.horizontal(|ui| {
        let layer = &mut s.layers[active];
        let mut blend = layer.props.blend;
        egui::ComboBox::from_id_salt("blend_mode").selected_text(blend.label()).width(130.0).show_ui(ui, |ui| {
            for m in BlendMode::ALL {
                if m.starts_group() {
                    ui.separator();
                }
                ui.selectable_value(&mut blend, m, m.label());
            }
        });
        if blend != layer.props.blend {
            layer.props.blend = blend;
            changed_props = true;
        }
        let mut op = layer.props.opacity;
        if percent_slider(ui, "Opacity", &mut op) {
            layer.props.opacity = op;
            changed_props = true;
        }
    });
    ui.horizontal(|ui| {
        let layer = &mut s.layers[active];
        ui.label(egui::RichText::new("Lock:").weak());
        let mut al = layer.props.alpha_locked;
        if icon_toggle(ui, icons::CHECKERBOARD, icons::CHECKERBOARD, &mut al, "Lock transparent pixels", 20.0).changed()
        {
            layer.props.alpha_locked = al;
            changed_props = true;
        }
        let mut lk = layer.props.locked;
        if icon_toggle(ui, icons::LOCK_KEY, icons::LOCK_KEY_OPEN, &mut lk, "Lock layer", 20.0).changed() {
            layer.props.locked = lk;
            changed_props = true;
        }
        let mut cl = layer.props.clipped;
        if icon_toggle(
            ui,
            icons::ARROW_ELBOW_DOWN_RIGHT,
            icons::ARROW_ELBOW_DOWN_RIGHT,
            &mut cl,
            "Clip to layer below",
            20.0,
        )
        .changed()
        {
            layer.props.clipped = cl;
            changed_props = true;
        }
    });
    ui.separator();

    // --- list (top layer first) ------------------------------------------
    let row_h = 40.0;
    let footer_h = 34.0;
    let list_h = (ui.available_height() - footer_h).max(row_h);
    let rename_id = ui.id().with("rename");
    let mut renaming = ui.data(|d| d.get_temp::<(usize, String)>(rename_id));
    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(list_h).id_salt("layer_list").show(ui, |ui| {
        for i in (0..n).rev() {
            let layer_id = s.layers[i].props.id;
            let is_active = i == active;
            let vis = s.layers[i].props.visible;
            let (row_rect, row_resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), Sense::click());
            let fill = if is_active {
                ui.visuals().selection.bg_fill
            } else if row_resp.hovered() {
                ui.visuals().widgets.hovered.bg_fill
            } else if i % 2 == 0 {
                ui.visuals().faint_bg_color
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(row_rect, 3, fill);

            // Drag/click area (everything right of the eye toggle).
            let content = egui::Rect::from_min_size(
                row_rect.min + egui::vec2(28.0, 0.0),
                egui::vec2(row_rect.width() - 28.0, row_h),
            );
            let drag_resp = ui.interact(content, ui.id().with(("layer_row", layer_id)), Sense::click_and_drag());
            if drag_resp.drag_started() {
                egui::DragAndDrop::set_payload(&ctx, DragLayer(i));
            }
            let hover_y = ui.input(|inp| inp.pointer.hover_pos()).map(|p| p.y);
            if let Some(p) = drag_resp.dnd_hover_payload::<DragLayer>() {
                if p.0 != i {
                    let above = hover_y.is_some_and(|y| y < row_rect.center().y);
                    let y = if above { row_rect.top() } else { row_rect.bottom() };
                    ui.painter().hline(
                        row_rect.x_range(),
                        y,
                        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                    );
                }
            }
            if let Some(p) = drag_resp.dnd_release_payload::<DragLayer>() {
                let above = hover_y.is_some_and(|y| y < row_rect.center().y);
                // List is top-first: "above row i" means a higher index.
                let mut to = if above { i + 1 } else { i };
                if p.0 < to {
                    to -= 1;
                }
                if p.0 != to {
                    reorder = Some((p.0, to.min(n - 1)));
                }
            }

            // Visibility toggle
            let eye_rect =
                egui::Rect::from_min_size(row_rect.min + egui::vec2(4.0, (row_h - 20.0) / 2.0), egui::vec2(20.0, 20.0));
            let eye = ui.interact(eye_rect, ui.id().with(("eye", layer_id)), Sense::click());
            ui.painter().text(
                eye_rect.center(),
                egui::Align2::CENTER_CENTER,
                if vis { icons::EYE } else { icons::EYE_SLASH },
                egui::FontId::new(14.0, ICON_FAMILY()),
                if vis { ui.visuals().text_color() } else { ui.visuals().weak_text_color() },
            );
            if eye.on_hover_text("Toggle visibility").clicked() {
                s.layers[i].props.visible = !vis;
                changed_props = true;
                toggled_vis = true;
            }

            // Thumbnail
            let thumb_rect = egui::Rect::from_min_size(row_rect.min + egui::vec2(30.0, 2.0), egui::vec2(48.0, 36.0));
            let raster = &s.layers[i].raster;
            let tex = thumbs.get(&ctx, (doc_id, layer_id), generation, [48, 36], |m| {
                crate::panels::thumbs::raster_thumb(raster, m, true)
            });
            let size = tex.size_vec2();
            let scale = (thumb_rect.width() / size.x).min(thumb_rect.height() / size.y);
            let draw = egui::Rect::from_center_size(thumb_rect.center(), size * scale);
            ui.painter().image(
                tex.id(),
                draw,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            ui.painter().rect_stroke(
                draw,
                0,
                egui::Stroke::new(1.0, Color32::from_black_alpha(120)),
                egui::StrokeKind::Outside,
            );

            // Name / rename
            let name_rect = egui::Rect::from_min_size(
                row_rect.min + egui::vec2(84.0, 0.0),
                egui::vec2((row_rect.width() - 110.0).max(20.0), row_h),
            );
            if renaming.as_ref().is_some_and(|(ri, _)| *ri == i) {
                let (_, text) = renaming.as_mut().unwrap();
                let te = ui.put(name_rect.shrink2(egui::vec2(0.0, 9.0)), egui::TextEdit::singleline(text));
                te.request_focus();
                let done = te.lost_focus() || ui.input(|inp| inp.key_pressed(egui::Key::Enter));
                let cancel = ui.input(|inp| inp.key_pressed(egui::Key::Escape));
                if cancel {
                    renaming = None;
                } else if done {
                    rename_target = Some((i, text.clone()));
                    renaming = None;
                }
            } else {
                ui.painter().text(
                    name_rect.left_center(),
                    egui::Align2::LEFT_CENTER,
                    &s.layers[i].props.name,
                    egui::FontId::proportional(13.0),
                    if vis { ui.visuals().text_color() } else { ui.visuals().weak_text_color() },
                );
                let p = &s.layers[i].props;
                if p.locked || p.alpha_locked || p.clipped {
                    let mut glyphs = String::new();
                    if p.clipped {
                        glyphs.push_str(icons::ARROW_ELBOW_DOWN_RIGHT);
                    }
                    if p.alpha_locked {
                        glyphs.push_str(icons::CHECKERBOARD);
                    }
                    if p.locked {
                        glyphs.push_str(icons::LOCK_KEY);
                    }
                    ui.painter().text(
                        row_rect.right_center() - egui::vec2(6.0, 0.0),
                        egui::Align2::RIGHT_CENTER,
                        glyphs,
                        egui::FontId::new(12.0, ICON_FAMILY()),
                        ui.visuals().weak_text_color(),
                    );
                }
            }
            if drag_resp.clicked() || row_resp.clicked() {
                activate = Some(i);
            }
            if drag_resp.double_clicked() {
                renaming = Some((i, s.layers[i].props.name.clone()));
            }
            drag_resp.context_menu(|ui| {
                let mut queued: Option<Action> = None;
                if ui.button("Rename").clicked() {
                    renaming = Some((i, s.layers[i].props.name.clone()));
                    ui.close();
                }
                for (label, a) in [
                    ("Duplicate Layer", Action::DuplicateLayer),
                    ("Delete Layer", Action::DeleteLayer),
                    ("Merge Down", Action::MergeDown),
                    ("Merge Visible", Action::MergeVisible),
                    ("Flatten Image", Action::Flatten),
                ] {
                    if ui.button(label).clicked() {
                        queued = Some(a);
                    }
                }
                ui.separator();
                for (label, a) in [
                    ("Flip Horizontal", Action::FlipLayerHorizontal),
                    ("Flip Vertical", Action::FlipLayerVertical),
                    ("Clear Layer", Action::ClearLayer),
                ] {
                    if ui.button(label).clicked() {
                        queued = Some(a);
                    }
                }
                ui.separator();
                if ui.button("Layer Properties…").clicked() {
                    queued = Some(Action::LayerProperties);
                }
                if let Some(a) = queued {
                    activate = Some(i);
                    pending.push(a);
                    ui.close();
                }
            });
        }
    });
    ui.data_mut(|d| match &renaming {
        Some(r) => {
            d.insert_temp(rename_id, r.clone());
        }
        None => {
            d.remove::<(usize, String)>(rename_id);
        }
    });

    // --- apply deferred edits ----------------------------------------------
    if let Some(i) = activate {
        if s.active != i {
            s.active = i;
        }
    }
    if let Some((from, to)) = reorder {
        s.move_layer(from, to);
        entry.doc.mark_all_dirty();
        entry.doc.commit("Reorder Layers");
    } else if let Some((i, name)) = rename_target {
        let name = name.trim().to_string();
        if !name.is_empty() && s.layers[i].props.name != name {
            s.layers[i].props.name = name;
            entry.doc.commit("Rename Layer");
        }
    } else if changed_props {
        entry.doc.mark_all_dirty();
        // Coalesce slider drags: commit when the pointer is released.
        let dragging = ctx.input(|i| i.pointer.any_down());
        if !dragging || toggled_vis {
            entry.doc.commit("Layer Properties");
        }
    }

    // --- footer --------------------------------------------------------------
    ui.separator();
    ui.horizontal(|ui| {
        let tip = |a: Action| {
            let k = keymap.primary_text(a);
            if k.is_empty() {
                a.label().to_string()
            } else {
                format!("{} ({k})", a.label())
            }
        };
        if icon_button(ui, icons::PLUS, &tip(Action::NewLayer), 24.0, false).clicked() {
            pending.push(Action::NewLayer);
        }
        if icon_button(ui, icons::COPY, &tip(Action::DuplicateLayer), 24.0, false).clicked() {
            pending.push(Action::DuplicateLayer);
        }
        if icon_button(ui, icons::ARROW_LINE_DOWN, &tip(Action::MergeDown), 24.0, false).clicked() {
            pending.push(Action::MergeDown);
        }
        if icon_button(ui, icons::ARROW_UP, &tip(Action::LayerUp), 24.0, false).clicked() {
            pending.push(Action::LayerUp);
        }
        if icon_button(ui, icons::ARROW_DOWN, &tip(Action::LayerDown), 24.0, false).clicked() {
            pending.push(Action::LayerDown);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button(ui, icons::TRASH, &tip(Action::DeleteLayer), 24.0, false).clicked() {
                pending.push(Action::DeleteLayer);
            }
        });
    });
}
