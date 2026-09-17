//! Layers panel: blend/opacity controls, a drag-reorderable tree of layers
//! and groups with visibility/lock toggles, thumbnails, inline rename,
//! multi-selection (Ctrl / Shift click) and a footer toolbar.

use egui::{Color32, Sense, Ui};
use qsketch_core::layer::LayerId;
use qsketch_core::BlendMode;

use crate::actions::Action;
use crate::state::{AppState, DocId};
use crate::ui::icons;
use crate::ui::widgets::{icon_button, icon_toggle, percent_slider};

#[derive(Clone, Copy, PartialEq, Eq)]
struct DragLayer(usize);

const ICON_FAMILY: fn() -> egui::FontFamily = crate::ui::iconset::family;
const INDENT: f32 = 14.0;

/// How a row click changes the selection.
enum Click {
    Only(usize),
    Toggle(usize),
    Range(usize),
    /// Make the row active but keep the selection as it is.
    Activate(usize),
}

/// An inline layer-name edit in flight.
#[derive(Clone)]
struct Rename {
    /// Layer ids are per-document, so the document has to be part of the key.
    doc: DocId,
    /// The layer whose name is being edited.
    layer: LayerId,
    text: String,
    /// The layer that was active when the edit started. The edit ends as soon
    /// as the active layer moves off it.
    anchor: LayerId,
}

impl Rename {
    fn start(doc: DocId, layer: LayerId, name: &str, active: Option<LayerId>) -> Self {
        Self { doc, layer, text: name.to_string(), anchor: active.unwrap_or(layer) }
    }
}

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let Some(doc_id) = state.active_doc else {
        ui.centered_and_justified(|ui| ui.label(egui::RichText::new("No document").weak()));
        return;
    };
    let ctx = ui.ctx().clone();
    let mut changed_props = false;
    let mut toggle_vis: Option<(LayerId, bool)> = None;
    let mut toggle_expand: Option<usize> = None;
    let mut rename_target: Option<(LayerId, String)> = None;
    let mut click: Option<Click> = None;
    let mut reorder: Option<(usize, usize, Option<LayerId>)> = None;

    // Borrow disjoint fields of the state.
    let AppState { docs, thumbs, pending, keymap, .. } = state;
    let Some(entry) = docs.iter_mut().find(|d| d.id == doc_id) else { return };
    let generation = entry.generation;
    let selected_ids = entry.selected_ids();
    let s = entry.doc.state_mut();
    let active = s.active.min(s.layers.len() - 1);
    let n = s.layers.len();

    // --- header: blend mode + opacity -----------------------------------
    ui.horizontal(|ui| {
        let layer = &mut s.layers[active];
        let mut blend = layer.props.blend;
        // Fit the row to the panel: combo takes ~40%, the slider the rest.
        let total = ui.available_width();
        let combo_w = (total * 0.4).clamp(90.0, 130.0);
        let slider_w = (total - combo_w - 110.0).max(40.0);
        ui.spacing_mut().slider_width = slider_w;
        let is_group = layer.is_group();
        egui::ComboBox::from_id_salt("blend_mode").selected_text(blend.label()).width(combo_w).show_ui(ui, |ui| {
            if is_group {
                ui.selectable_value(&mut blend, BlendMode::PassThrough, BlendMode::PassThrough.label());
                ui.separator();
            }
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
        if !layer.is_group() {
            let mut al = layer.props.alpha_locked;
            if icon_toggle(ui, icons::CHECKERBOARD, icons::CHECKERBOARD, &mut al, "Lock transparent pixels", 20.0)
                .changed()
            {
                layer.props.alpha_locked = al;
                changed_props = true;
            }
        }
        let mut lk = layer.props.locked;
        let lock_tip = if layer.is_group() { "Lock group" } else { "Lock layer" };
        if icon_toggle(ui, icons::LOCK_KEY, icons::LOCK_KEY_OPEN, &mut lk, lock_tip, 20.0).changed() {
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
    // Rows inside a collapsed group are skipped.
    let rows: Vec<(usize, usize)> = (0..n)
        .rev()
        .filter(|&i| s.ancestors(i).iter().all(|g| s.layer_by_id(*g).is_some_and(|l| l.props.expanded)))
        .map(|i| (i, s.depth(i)))
        .collect();
    let row_h = 40.0;
    let footer_h = 34.0;
    let list_h = (ui.available_height() - footer_h).max(row_h);
    let rename_id = ui.id().with("rename");
    let mut renaming = ui.data(|d| d.get_temp::<Rename>(rename_id));
    let multi = selected_ids.len() > 1;
    // When the active layer changes from anywhere (keyboard, canvas, undo),
    // scroll the list so its row is in view. A click on a row is already
    // visible, so the scroll is a no-op there.
    let seen_id = ui.id().with("seen_active");
    let active_layer_id = s.layers.get(active).map(|l| l.props.id);
    let seen = ui.data(|d| d.get_temp::<Option<LayerId>>(seen_id)).flatten();
    let jump_to_active = seen != active_layer_id;
    ui.data_mut(|d| d.insert_temp(seen_id, active_layer_id));
    // Switching layers mid-rename (a row click, a shortcut, the canvas) means
    // the user is done with the name, not that they want the box to follow
    // them around: take the name and close the editor. The layer the rename
    // started on is the anchor, so a rename begun on a row that was not active
    // (right-click ▸ Rename) does not close itself on the next frame.
    if let Some(r) = &renaming {
        if r.doc != doc_id || !s.layers.iter().any(|l| l.props.id == r.layer) {
            // The layer (or the whole document) is gone; there is nothing left
            // to name.
            renaming = None;
        } else if active_layer_id != Some(r.anchor) {
            rename_target = Some((r.layer, r.text.clone()));
            renaming = None;
        }
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(list_h).id_salt("layer_list").show(ui, |ui| {
        for (row_no, &(i, depth)) in rows.iter().enumerate() {
            let layer_id = s.layers[i].props.id;
            let is_group = s.layers[i].is_group();
            let is_active = i == active;
            let is_selected = selected_ids.contains(&layer_id);
            let vis = s.layers[i].props.visible;
            let dim = !s.effectively_visible(i);
            let (row_rect, row_resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), Sense::click());
            if is_active && jump_to_active {
                ui.scroll_to_rect(row_rect, None);
            }
            let fill = if is_active {
                crate::ui::chrome::row_fill(ui.visuals().selection.bg_fill)
            } else if is_selected {
                ui.visuals().selection.bg_fill.gamma_multiply(0.45)
            } else if row_resp.hovered() {
                crate::ui::chrome::row_fill(ui.visuals().widgets.hovered.bg_fill)
            } else if row_no % 2 == 0 {
                // Translucent zebra striping: separates rows without hiding
                // the chrome texture or competing with the eye icons.
                crate::ui::chrome::zebra(ui)
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(row_rect, 3, fill);
            let indent = depth as f32 * INDENT;

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
            let above = hover_y.is_some_and(|y| y < row_rect.center().y);
            // Dropping on the lower half of an open group puts the layer
            // inside it, on top; otherwise the drop lands beside this row.
            let into_group = !above && is_group && s.layers[i].props.expanded;
            if let Some(p) = drag_resp.dnd_hover_payload::<DragLayer>() {
                if p.0 != i && !s.is_descendant_of(i, s.layers[p.0].props.id) {
                    let y = if above { row_rect.top() } else { row_rect.bottom() };
                    let x0 = row_rect.left() + 28.0 + indent + if into_group { INDENT } else { 0.0 };
                    ui.painter().hline(
                        x0..=row_rect.right(),
                        y,
                        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                    );
                }
            }
            if let Some(p) = drag_resp.dnd_release_payload::<DragLayer>() {
                if p.0 != i {
                    // List is top-first: "above row i" means a higher index.
                    let (to, parent) = if above {
                        (i + 1, s.layers[i].props.parent)
                    } else if into_group {
                        (i, Some(layer_id))
                    } else {
                        (s.block(i).start, s.layers[i].props.parent)
                    };
                    reorder = Some((p.0, to, parent));
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
                if vis && !dim { ui.visuals().text_color() } else { hidden_eye_color(ui) },
            );
            if eye.on_hover_text("Toggle visibility").clicked() {
                toggle_vis = Some((layer_id, !vis));
            }

            // Expand caret (groups) + thumbnail / folder glyph
            let mut x = 30.0 + indent;
            if is_group {
                let caret_rect = egui::Rect::from_min_size(
                    row_rect.min + egui::vec2(x - 4.0, (row_h - 16.0) / 2.0),
                    egui::vec2(14.0, 16.0),
                );
                let caret = ui.interact(caret_rect, ui.id().with(("caret", layer_id)), Sense::click());
                let expanded = s.layers[i].props.expanded;
                ui.painter().text(
                    caret_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    if expanded { icons::CARET_DOWN } else { icons::CARET_RIGHT },
                    egui::FontId::new(11.0, ICON_FAMILY()),
                    crate::ui::theme::dim_text(ui.visuals()),
                );
                if caret.on_hover_text(if expanded { "Collapse group" } else { "Expand group" }).clicked() {
                    toggle_expand = Some(i);
                }
                x += 12.0;
                let folder_rect = egui::Rect::from_min_size(row_rect.min + egui::vec2(x, 2.0), egui::vec2(36.0, 36.0));
                ui.painter().text(
                    folder_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    if expanded { icons::FOLDER_OPEN } else { icons::FOLDER },
                    egui::FontId::new(22.0, ICON_FAMILY()),
                    if dim { crate::ui::theme::dim_text(ui.visuals()) } else { ui.visuals().text_color() },
                );
                x += 40.0;
            } else {
                let thumb_rect = egui::Rect::from_min_size(row_rect.min + egui::vec2(x, 2.0), egui::vec2(48.0, 36.0));
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
                x += 54.0;
            }

            // Name / rename
            let name_rect = egui::Rect::from_min_size(
                row_rect.min + egui::vec2(x, 0.0),
                egui::vec2((row_rect.width() - x - 26.0).max(20.0), row_h),
            );
            if renaming.as_ref().is_some_and(|r| r.layer == layer_id) {
                let text = &mut renaming.as_mut().unwrap().text;
                let te = ui.put(name_rect.shrink2(egui::vec2(0.0, 9.0)), egui::TextEdit::singleline(text));
                te.request_focus();
                let done = te.lost_focus() || ui.input(|inp| inp.key_pressed(egui::Key::Enter));
                let cancel = ui.input(|inp| inp.key_pressed(egui::Key::Escape));
                if cancel {
                    renaming = None;
                } else if done {
                    rename_target = Some((layer_id, text.clone()));
                    renaming = None;
                }
            } else {
                ui.painter().text(
                    name_rect.left_center(),
                    egui::Align2::LEFT_CENTER,
                    &s.layers[i].props.name,
                    egui::FontId::proportional(13.0),
                    if dim { crate::ui::theme::dim_text(ui.visuals()) } else { ui.visuals().text_color() },
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
                        crate::ui::theme::dim_text(ui.visuals()),
                    );
                }
            }
            if drag_resp.clicked() || row_resp.clicked() {
                let mods = ui.input(|inp| inp.modifiers);
                click = Some(if mods.shift {
                    Click::Range(i)
                } else if mods.command {
                    Click::Toggle(i)
                } else {
                    Click::Only(i)
                });
            }
            if drag_resp.double_clicked() {
                renaming = Some(Rename::start(doc_id, layer_id, &s.layers[i].props.name, active_layer_id));
            }
            drag_resp.context_menu(|ui| {
                let mut queued: Option<Action> = None;
                if ui.button("Add Layer").clicked() {
                    queued = Some(Action::NewLayer);
                }
                if ui.button("Rename").clicked() {
                    renaming = Some(Rename::start(doc_id, layer_id, &s.layers[i].props.name, active_layer_id));
                    ui.close();
                }
                ui.separator();
                let group_label = if multi && is_selected { "Group Selected" } else { "Group Layer" };
                if ui.button(group_label).clicked() {
                    queued = Some(Action::GroupLayers);
                }
                if is_group && ui.button("Ungroup").clicked() {
                    queued = Some(Action::UngroupLayers);
                }
                ui.separator();
                let del_label = if multi && is_selected {
                    "Delete Layers"
                } else if is_group {
                    "Delete Group"
                } else {
                    "Delete Layer"
                };
                let dup_label = if is_group { "Duplicate Group" } else { "Duplicate Layer" };
                let merge_label = if is_group { "Merge Group" } else { "Merge Down" };
                for (label, a) in [
                    (dup_label, Action::DuplicateLayer),
                    (del_label, Action::DeleteLayer),
                    (merge_label, Action::MergeDown),
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
                    // Acting on a row outside the selection targets that row
                    // alone; inside it, the whole selection.
                    if !is_selected {
                        click = Some(Click::Only(i));
                    } else if !is_active {
                        click = Some(Click::Activate(i));
                    }
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
            d.remove::<Rename>(rename_id);
        }
    });

    // --- apply deferred edits ----------------------------------------------
    if let Some(c) = click {
        let ids = |s: &qsketch_core::DocState, r: std::ops::RangeInclusive<usize>| -> Vec<LayerId> {
            r.filter_map(|i| s.layers.get(i).map(|l| l.props.id)).collect()
        };
        match c {
            Click::Only(i) => {
                s.active = i;
                entry.selected = vec![s.layers[i].props.id];
            }
            Click::Activate(i) => {
                s.active = i;
                entry.selected = selected_ids.clone();
            }
            Click::Toggle(i) => {
                let id = s.layers[i].props.id;
                let mut sel = selected_ids.clone();
                if sel.contains(&id) {
                    if sel.len() > 1 {
                        sel.retain(|x| *x != id);
                        if i == s.active {
                            // Hand the active role to another selected layer.
                            if let Some(j) = sel.iter().find_map(|x| s.index_of(*x)) {
                                s.active = j;
                            }
                        }
                    }
                } else {
                    sel.push(id);
                    s.active = i;
                }
                entry.selected = sel;
            }
            Click::Range(i) => {
                let a = s.active;
                let (lo, hi) = if a <= i { (a, i) } else { (i, a) };
                let mut sel = selected_ids.clone();
                for id in ids(s, lo..=hi) {
                    if !sel.contains(&id) {
                        sel.push(id);
                    }
                }
                entry.selected = sel;
            }
        }
    }
    if let Some(i) = toggle_expand {
        // A view toggle like visibility: no undo step, applied to every state.
        let (id, expanded) = (s.layers[i].props.id, !s.layers[i].props.expanded);
        s.layers[i].props.expanded = expanded;
        entry.doc.history.for_each_state_mut(|st| {
            if let Some(l) = st.layers.iter_mut().find(|l| l.props.id == id) {
                l.props.expanded = expanded;
            }
        });
    }
    // Visibility is a view toggle, not an undoable edit.
    if let Some((id, visible)) = toggle_vis {
        entry.doc.set_layer_visible(id, visible);
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
        if icon_button(ui, icons::FOLDER_PLUS, &tip(Action::GroupLayers), 24.0, false).clicked() {
            pending.push(Action::GroupLayers);
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

    // --- undoable edits ------------------------------------------------------
    // These commit, so anything still floating over the document lands first
    // (see `AppState::settle`); that needs the panel's borrows to be over.
    if reorder.is_none() && rename_target.is_none() && !changed_props {
        return;
    }
    state.settle();
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let s = entry.doc.state_mut();
    if let Some((from, to, parent)) = reorder {
        let before: Vec<LayerId> = s.layers.iter().map(|l| l.props.id).collect();
        let parents: Vec<Option<LayerId>> = s.layers.iter().map(|l| l.props.parent).collect();
        s.move_block(from, to, parent);
        let after: Vec<LayerId> = s.layers.iter().map(|l| l.props.id).collect();
        let parents_after: Vec<Option<LayerId>> = s.layers.iter().map(|l| l.props.parent).collect();
        if before != after || parents != parents_after {
            entry.doc.mark_all_dirty();
            entry.doc.commit("Reorder Layers");
        }
    } else if let Some((id, name)) = rename_target {
        let name = name.trim().to_string();
        let i = s.layers.iter().position(|l| l.props.id == id);
        if let Some(i) = i.filter(|_| !name.is_empty()) {
            if s.layers[i].props.name != name {
                s.layers[i].props.name = name;
                let label = if s.layers[i].is_group() { "Rename Group" } else { "Rename Layer" };
                entry.doc.commit(label);
            }
        }
    } else if changed_props {
        entry.doc.mark_all_dirty();
        // Coalesce slider drags: commit when the pointer is released.
        let dragging = ctx.input(|i| i.pointer.any_down());
        if !dragging {
            entry.doc.commit("Layer Properties");
        }
    }
}

/// Eye-slash glyph for hidden layers: pushed well away from the visible
/// eye's text color (toward the row background) so the two states read at a
/// glance.
fn hidden_eye_color(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_gray(80)
    } else {
        Color32::from_gray(165)
    }
}
