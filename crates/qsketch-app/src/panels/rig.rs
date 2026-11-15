//! The Rig panel: everything the Bone tool needs besides the canvas itself,
//! in a bottom panel like the Timeline. Pick or create an animation, play and
//! scrub it on a key track, set the easing of new keys, and handle the limb
//! (IK), the active layer's mesh and weights, and what the layer follows.

use egui::{Align2, Color32, FontId, Pos2, Sense, Stroke, Ui};
use qsketch_core::rig::{BoneId, Curve};
use qsketch_core::IkConstraint;

use crate::dialogs::rig::Easing;
use crate::state::AppState;
use crate::ui::icons;

/// Height of the key track (ruler plus three rows).
const TRACK_H: f32 = 104.0;

/// What a row of the key track holds.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Row {
    /// The selected bone's keys.
    #[default]
    Bone,
    /// The selected bone's IK constraint (mix and bend).
    Ik,
    /// The active layer's opacity (show, hide, fade).
    Alpha,
}

const ROWS: [Row; 3] = [Row::Bone, Row::Ik, Row::Alpha];

/// One row of the track: its label (`None` leaves the row empty) and keys.
struct TrackRow<'a> {
    label: Option<String>,
    keys: &'a [u32],
    color: Option<Color32>,
}

/// What the key track was used for this frame.
#[derive(Default)]
struct TrackOut {
    /// The playhead was scrubbed (or a key was clicked) to this time.
    scrub: Option<u32>,
    /// A key was right-clicked: its row and time.
    delete_key: Option<(Row, u32)>,
    /// A key was dragged to a new time: its row, from and to.
    move_key: Option<(Row, u32, u32)>,
}

/// A key being dragged along the track: its row and the time it started at.
#[derive(Clone, Copy, Default)]
struct KeyDrag {
    row: Row,
    from: u32,
}

/// A pleasant ruler step (ms) so ticks sit at least ~70 px apart.
fn ruler_step(duration_ms: u32, width: f32) -> u32 {
    let px_per_ms = width / duration_ms.max(1) as f32;
    [10u32, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 5000, 10_000, 30_000]
        .into_iter()
        .find(|&s| s as f32 * px_per_ms >= 70.0)
        .unwrap_or(60_000)
}

fn diamond(painter: &egui::Painter, c: Pos2, r: f32, fill: Color32, stroke: Stroke) {
    let pts = vec![Pos2::new(c.x, c.y - r), Pos2::new(c.x + r, c.y), Pos2::new(c.x, c.y + r), Pos2::new(c.x - r, c.y)];
    painter.add(egui::Shape::convex_polygon(pts, fill, stroke));
}

/// The key track: a ruler with the playhead and three rows of keys (the
/// selected bone, its IK, the active layer's opacity). Click or drag to
/// scrub (keys attract the playhead unless Shift is held); drag a key to
/// retime it; right-click a key to delete it.
fn key_track(ui: &mut Ui, duration_ms: u32, time_ms: u32, rows: &[TrackRow; 3]) -> TrackOut {
    let mut out = TrackOut::default();
    let width = ui.available_width().max(240.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, TRACK_H), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals().clone();
    let (accent, text) = (visuals.selection.bg_fill, visuals.text_color());
    let weak = visuals.weak_text_color();
    painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );

    let pad = 14.0;
    let (x0, x1) = (rect.left() + pad, rect.right() - pad);
    let dur = duration_ms.max(1) as f32;
    let to_x = |t: u32| x0 + (x1 - x0) * t as f32 / dur;
    let to_t = |x: f32| (((x - x0) / (x1 - x0)).clamp(0.0, 1.0) * dur).round() as u32;

    // Ruler.
    let step = ruler_step(duration_ms, x1 - x0);
    let font = FontId::proportional(10.0);
    let mut t = 0u32;
    while t <= duration_ms {
        let x = to_x(t);
        painter.line_segment([Pos2::new(x, rect.top() + 4.0), Pos2::new(x, rect.top() + 12.0)], Stroke::new(1.0, weak));
        let label = if t % 1000 == 0 { format!("{}s", t / 1000) } else { format!("{t}") };
        painter.text(Pos2::new(x + 3.0, rect.top() + 3.0), Align2::LEFT_TOP, label, font.clone(), weak);
        t += step;
    }
    // Row lines at 36, 60 and 84 px; a press belongs to the nearest row below
    // the one above's midpoint.
    let row_y = |i: usize| rect.top() + 36.0 + 24.0 * i as f32;
    let row_at = |y: f32| {
        let i = (((y - (rect.top() + 24.0)) / 24.0).floor().max(0.0) as usize).min(2);
        ROWS[i]
    };
    let row_index = |row: Row| ROWS.iter().position(|r| *r == row).unwrap_or(0);
    for (i, row) in rows.iter().enumerate() {
        let y = row_y(i);
        painter.line_segment([Pos2::new(x0, y), Pos2::new(x1, y)], Stroke::new(1.0, weak.gamma_multiply(0.35)));
        if let Some(l) = &row.label {
            painter.text(Pos2::new(x0 + 8.0, y - 16.0), Align2::LEFT_TOP, l, font.clone(), weak);
        }
    }

    // Keys.
    let r = 5.5;
    for (i, row) in rows.iter().enumerate() {
        let base = row.color.unwrap_or(text);
        for &k in row.keys {
            let fill = if k == time_ms { accent } else { base };
            diamond(&painter, Pos2::new(to_x(k), row_y(i)), r, fill, Stroke::new(1.0, Color32::BLACK));
        }
    }

    // Playhead.
    let px = to_x(time_ms.min(duration_ms));
    painter
        .line_segment([Pos2::new(px, rect.top() + 2.0), Pos2::new(px, rect.bottom() - 2.0)], Stroke::new(2.0, accent));
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(px - 5.0, rect.top() + 1.0),
            Pos2::new(px + 5.0, rect.top() + 1.0),
            Pos2::new(px, rect.top() + 9.0),
        ],
        accent,
        Stroke::NONE,
    ));

    // Interaction.
    let near = |row: Row, x: f32| rows[row_index(row)].keys.iter().copied().find(|&k| (to_x(k) - x).abs() <= 7.0);
    let near_any = |x: f32| ROWS.iter().find_map(|&row| near(row, x));
    let drag_id = ui.id().with("key_drag");
    if resp.drag_started() {
        if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
            let row = row_at(p.y);
            if let Some(from) = near(row, p.x) {
                ui.data_mut(|d| d.insert_temp(drag_id, KeyDrag { row, from }));
            }
        }
    }
    let key_drag: Option<KeyDrag> = ui.data(|d| d.get_temp(drag_id));
    if let Some(kd) = key_drag {
        // Dragging a key retimes it (in 5 ms steps); the playhead stays put.
        if let Some(p) = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.latest_pos())) {
            let to = (to_t(p.x) + 2) / 5 * 5;
            let y = row_y(row_index(kd.row));
            let c = Color32::from_rgb(255, 200, 40);
            diamond(&painter, Pos2::new(to_x(to), y), r, c.gamma_multiply(0.85), Stroke::new(1.5, Color32::WHITE));
            painter.text(Pos2::new(to_x(to) + 9.0, y - 5.0), Align2::LEFT_TOP, format!("{to} ms"), font.clone(), text);
            if resp.drag_stopped() && to != kd.from {
                out.move_key = Some((kd.row, kd.from, to));
            }
        }
        if resp.drag_stopped() {
            ui.data_mut(|d| d.remove_temp::<KeyDrag>(drag_id));
        }
    } else if let Some(p) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            let snap = !ui.input(|i| i.modifiers.shift);
            // Whole 5 ms steps (like retiming), unless Shift is held.
            let mut t = if snap { ((to_t(p.x) + 2) / 5 * 5).min(duration_ms) } else { to_t(p.x) };
            if snap {
                if let Some(k) = near_any(p.x) {
                    t = k;
                }
            }
            out.scrub = Some(t);
        }
    }
    if resp.secondary_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let row = row_at(p.y);
            out.delete_key = near(row, p.x).map(|k| (row, k));
        }
    }
    resp.on_hover_text(
        "Click or drag to scrub (Shift: no snapping to keys). Drag a key to retime it; right-click a key to delete it. Rows: the selected bone, its IK, and the active layer's opacity.",
    );
    out
}

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let Some(doc_id) = state.active_doc else {
        ui.label("Open a document to rig it.");
        return;
    };
    let Some(e) = state.doc(doc_id) else { return };
    let mut rui = e.rig_ui.clone();
    let rig = &e.doc.state().rig;
    let cur = rui.anim_in(rig);
    rui.anim = cur;
    let names: Vec<String> = rig.animations.iter().map(|a| a.name.clone()).collect();
    let original = cur.map(|a| rig.animations[a].clone());
    let mut anim_edit = original.clone();
    let layer = e.doc.state().active_layer();
    let (layer_id, layer_name) = (layer.id(), layer.name().to_string());
    let layer_bounds = layer.raster.full_bounds();
    let mesh_dims = rig.mesh_of(layer_id).map(|m| (m.cols, m.rows));
    let layer_bone = rig.attached_bone(layer_id);
    let attached = layer_bone.and_then(|b| rig.bone(b)).map(|b| b.name.clone());
    let sel_name = rui.sel.and_then(|b| rig.bone(b)).map(|b| b.name.clone());
    let bone_keys: Vec<u32> = rui.sel.zip(cur).map(|(b, a)| rig.key_times(a, b)).unwrap_or_default();
    let key_here = bone_keys.contains(&rui.time_ms);
    let alpha_keys: Vec<u32> = cur.map(|a| rig.alpha_key_times(a, layer_id)).unwrap_or_default();
    let rest_alpha_now = rig.rest_alpha_of(layer_id);
    let alpha_now = rig.alpha_at(cur.map(|a| (a, rui.time_ms)), layer_id);
    if !ui.memory(|m| m.focused().is_some()) {
        rui.alpha_key = alpha_now;
    }
    let ik_info: Option<IkConstraint> =
        rui.sel.and_then(|b| rig.ik_of_bone(b).or_else(|| rig.ik.iter().find(|c| c.target == b)).cloned());
    let can_add_ik = ik_info.is_none() && rui.sel.and_then(|b| rig.bone(b)).is_some_and(|b| b.parent.is_some());
    let mut ik_edit = ik_info.clone();
    let ik_keys: Vec<u32> = ik_info
        .as_ref()
        .zip(cur)
        .and_then(|(c, a)| rig.animations[a].ik.iter().find(|t| t.constraint == c.id))
        .map(|t| t.keys.iter().map(|k| k.time_ms).collect())
        .unwrap_or_default();

    let (mut new_anim, mut del_anim, mut attach, mut detach, mut del_keys, mut open_dialog) =
        (false, false, false, false, false, false);
    let (mut add_ik, mut del_ik, mut key_ik) = (false, false, false);
    let (mut make_mesh, mut auto_near, mut auto_all, mut del_mesh) = (false, false, false, false);
    let mut export_rig = false;
    let mut close_loop = false;
    let mut key_all = false;
    let mut alpha_key: Option<f32> = None;
    let mut rest_alpha: Option<f32> = None;
    let mut track = TrackOut::default();

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 5.0);
        // --- animation -------------------------------------------------------
        ui.horizontal_wrapped(|ui| {
            ui.strong("Animation");
            let text = cur.and_then(|a| names.get(a).cloned()).unwrap_or_else(|| "Setup pose".into());
            egui::ComboBox::from_id_salt("rig_panel_anim").selected_text(text).width(130.0).show_ui(ui, |ui| {
                if ui.selectable_label(cur.is_none(), "Setup pose").clicked() {
                    rui.anim = None;
                    rui.playing = false;
                }
                for (i, n) in names.iter().enumerate() {
                    if ui.selectable_label(cur == Some(i), n).clicked() {
                        rui.anim = Some(i);
                        rui.time_ms = 0;
                    }
                }
            });
            new_anim = ui.button("New").on_hover_text("New animation").clicked();
            if let Some(a) = anim_edit.as_mut() {
                ui.add(egui::TextEdit::singleline(&mut a.name).desired_width(110.0));
                ui.add(egui::DragValue::new(&mut a.duration_ms).range(1..=600_000).suffix(" ms"))
                    .on_hover_text("Length of the animation");
                ui.checkbox(&mut a.looped, "Loop");
                if a.looped
                    && ui
                        .button("Close loop")
                        .on_hover_text("Copy each timeline's first key to the end so the last pose runs into the first")
                        .clicked()
                {
                    close_loop = true;
                }
                ui.checkbox(&mut rui.nearest, "Pixel art").on_hover_text("Preview without smoothing");
                del_anim = ui.button(format!("{} Delete", icons::TRASH)).on_hover_text("Delete this animation").clicked();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Bake & more…").on_hover_text("The Rig dialog: bake to frames, edit bone numbers").clicked() {
                    open_dialog = true;
                }
                export_rig = ui
                    .button("Export for games…")
                    .on_hover_text("rig.json plus a PNG per layer, for a game runtime")
                    .clicked();
            });
        });

        // --- transport and keys ------------------------------------------------
        if let Some(a) = &anim_edit {
            ui.horizontal_wrapped(|ui| {
                let glyph = if rui.playing { icons::PAUSE } else { icons::PLAY };
                if ui.button(glyph).on_hover_text(if rui.playing { "Pause" } else { "Play the animation" }).clicked() {
                    rui.playing = !rui.playing;
                    if rui.playing && !a.looped && rui.time_ms >= a.duration_ms {
                        rui.time_ms = 0;
                    }
                }
                rui.time_ms = rui.time_ms.min(a.duration_ms);
                ui.label(format!("{:.2} / {:.2} s", rui.time_ms as f32 / 1000.0, a.duration_ms as f32 / 1000.0));
                ui.separator();
                ui.label("Easing for new keys");
                egui::ComboBox::from_id_salt("rig_panel_easing").selected_text(rui.easing.label()).width(100.0).show_ui(
                    ui,
                    |ui| {
                        for e in Easing::ALL {
                            ui.selectable_value(&mut rui.easing, e, e.label());
                        }
                    },
                );
                ui.separator();
                if ui
                    .button("Key all")
                    .on_hover_text("Key every animated bone at the playhead where it is now, so it holds its pose from here")
                    .clicked()
                {
                    key_all = true;
                }
                if ui
                    .add_enabled(key_here, egui::Button::new("Delete keys here"))
                    .on_hover_text("Delete the selected bone's keys at the playhead")
                    .clicked()
                {
                    del_keys = true;
                }
            });
            let rows = [
                TrackRow {
                    label: Some(sel_name.clone().unwrap_or_else(|| "select a bone to see its keys".to_string())),
                    keys: &bone_keys,
                    color: None,
                },
                TrackRow {
                    label: ik_info.as_ref().map(|c| format!("IK {}", c.name)),
                    keys: &ik_keys,
                    color: Some(Color32::from_rgb(110, 220, 110)),
                },
                TrackRow {
                    label: Some(format!("{layer_name} opacity")),
                    keys: &alpha_keys,
                    color: Some(Color32::from_rgb(255, 160, 80)),
                },
            ];
            track = key_track(ui, a.duration_ms, rui.time_ms, &rows);
        } else {
            ui.label(
                egui::RichText::new(
                    "Setup pose: bones you draw or drag edit the skeleton itself. Pick or create an animation to pose and key bones over time.",
                )
                .weak(),
            );
        }
        ui.separator();

        // --- limb, mesh, layer: one wrapped row of compact groups ----------------
        ui.horizontal_wrapped(|ui| {
            ui.strong("Limb");
            match ik_edit.as_mut() {
                Some(c) => {
                    ui.label(format!("{} → {}", c.name, target_name(rig, c.target)));
                    ui.add(egui::DragValue::new(&mut c.mix).speed(0.01).range(0.0..=1.0).prefix("mix "))
                        .on_hover_text("How much IK replaces the keyed rotations");
                    if ui.button("Flip bend").on_hover_text("Fold the joint the other way").clicked() {
                        c.bend = if c.bend < 0 { 1 } else { -1 };
                    }
                    if cur.is_some() && ui.button("Key").on_hover_text("Key the mix and bend at the playhead").clicked() {
                        key_ik = true;
                    }
                    if ui.button(format!("{} Remove", icons::TRASH)).on_hover_text("Remove this IK").clicked() {
                        del_ik = true;
                    }
                }
                None if can_add_ik => {
                    add_ik = ui
                        .button("Add IK")
                        .on_hover_text("The selected bone and its parent become a limb reaching for a new target bone")
                        .clicked();
                }
                None => {
                    ui.label(egui::RichText::new("select a bone that has a parent").weak());
                }
            }

            ui.separator();
            ui.strong("Mesh");
            match mesh_dims {
                Some((c, r)) => {
                    ui.label(format!("{c}×{r}"));
                    ui.toggle_value(&mut rui.weights, "Paint weights")
                        .on_hover_text("Drag on the art to add the selected bone's weight; hold Ctrl to remove it");
                    if rui.weights {
                        ui.add(egui::DragValue::new(&mut rui.brush).range(2.0..=200.0).prefix("size ").suffix(" px"));
                        ui.add(egui::DragValue::new(&mut rui.strength).speed(0.01).range(0.01..=1.0).prefix("strength "));
                    }
                    auto_near = ui
                        .button("Auto weights")
                        .on_hover_text("Weight each vertex to this layer's bone, its parent and its children")
                        .clicked();
                    auto_all = ui.button("All bones").on_hover_text("Auto weights from every bone").clicked();
                    if ui.button(format!("{} Remove", icons::TRASH)).on_hover_text("Remove this layer's mesh").clicked() {
                        del_mesh = true;
                    }
                }
                None => {
                    ui.add(egui::DragValue::new(&mut rui.cell).range(4..=64).suffix(" px")).on_hover_text("Mesh cell size");
                    make_mesh = ui
                        .add_enabled(layer_bounds.is_some(), egui::Button::new("Make mesh"))
                        .on_hover_text("Lay a grid over the active layer so it can bend; it starts rigid")
                        .clicked();
                }
            }

            ui.separator();
            ui.strong("Layer");
            // Buttons first and the names after, so the buttons stay put when
            // the active layer changes.
            if rui.sel.is_some() && rui.sel == layer_bone {
                detach = ui.button("Detach").on_hover_text("Stop the active layer following its bone").clicked();
            } else if ui
                .add_enabled(sel_name.is_some(), egui::Button::new("Attach"))
                .on_hover_text(match &sel_name {
                    Some(n) => format!("Make the active layer follow “{n}”"),
                    None => "Select a bone on the canvas first".to_string(),
                })
                .clicked()
            {
                attach = true;
            }
            ui.label(match &attached {
                Some(b) => format!("{layer_name} → {b}"),
                None => format!("{layer_name}: not attached"),
            });

            ui.separator();
            ui.strong("Visibility");
            let mut visible_at_rest = rest_alpha_now >= 0.5;
            if ui
                .checkbox(&mut visible_at_rest, "at rest")
                .on_hover_text("Whether the active layer shows when an animation doesn't key it (and in the setup pose)")
                .changed()
            {
                rest_alpha = Some(if visible_at_rest { 1.0 } else { 0.0 });
            }
            if cur.is_some() {
                if ui.button("Show").on_hover_text("Key the layer fully shown at the playhead").clicked() {
                    alpha_key = Some(1.0);
                }
                if ui.button("Hide").on_hover_text("Key the layer hidden at the playhead").clicked() {
                    alpha_key = Some(0.0);
                }
                ui.add(egui::DragValue::new(&mut rui.alpha_key).speed(0.01).range(0.0..=1.0).prefix("opacity "))
                    .on_hover_text("Opacity of the active layer at the playhead right now");
                if ui.button("Key").on_hover_text("Key this opacity at the playhead").clicked() {
                    alpha_key = Some(rui.alpha_key);
                }
            }
        });
    });

    // --- write back --------------------------------------------------------------
    let Some(e) = state.doc_mut(doc_id) else { return };
    e.rig_ui.anim = rui.anim;
    e.rig_ui.easing = rui.easing;
    e.rig_ui.nearest = rui.nearest;
    e.rig_ui.weights = rui.weights;
    e.rig_ui.brush = rui.brush;
    e.rig_ui.strength = rui.strength;
    e.rig_ui.cell = rui.cell;
    e.rig_ui.playing = rui.playing;
    e.rig_ui.alpha_key = rui.alpha_key;
    e.rig_ui.time_ms = rui.time_ms;
    if let Some(t) = track.scrub {
        e.rig_ui.time_ms = t;
        e.rig_ui.playing = false;
    }
    if anim_edit != original {
        if let (Some(a), Some(edit)) = (cur, anim_edit) {
            e.doc.state_mut().rig.animations[a] = edit;
            e.rig_ui.edited = true;
        }
    }
    if ik_edit != ik_info {
        if let Some(c) = &ik_edit {
            if let Some(slot) = e.doc.state_mut().rig.ik.iter_mut().find(|x| x.id == c.id) {
                slot.mix = c.mix;
                slot.bend = c.bend;
                e.rig_ui.edited = true;
            }
        }
    }
    let (sel, time) = (rui.sel, e.rig_ui.time_ms);
    if new_anim {
        let rig = &mut e.doc.state_mut().rig;
        let n = rig.animations.len() + 1;
        e.rig_ui.anim = Some(rig.add_animation(format!("anim {n}"), 1000));
        e.rig_ui.time_ms = 0;
        e.doc.commit("New Rig Animation");
    } else if del_anim {
        if let Some(a) = cur {
            e.doc.state_mut().rig.animations.remove(a);
            e.rig_ui.anim = None;
            e.rig_ui.time_ms = 0;
            e.rig_ui.playing = false;
            e.doc.commit("Delete Rig Animation");
        }
    } else if key_all {
        if let Some(a) = cur {
            if e.doc.state_mut().rig.key_all(a, time) > 0 {
                e.doc.commit("Key All");
            }
        }
    } else if close_loop {
        if let Some(a) = cur {
            if e.doc.state_mut().rig.close_loop(a) > 0 {
                e.doc.commit("Close Loop");
            }
        }
    } else if del_keys {
        if let (Some(b), Some(a)) = (sel, cur) {
            e.doc.state_mut().rig.remove_keys_at(a, b, time);
            e.doc.commit("Delete Keys");
        }
    } else if let Some((row, from, to)) = track.move_key {
        let moved = match (row, sel, &ik_info, cur) {
            (Row::Bone, Some(b), _, Some(a)) => {
                e.doc.state_mut().rig.move_keys(a, b, from, to);
                true
            }
            (Row::Ik, _, Some(c), Some(a)) => {
                e.doc.state_mut().rig.move_ik_keys(a, c.id, from, to);
                true
            }
            (Row::Alpha, _, _, Some(a)) => {
                e.doc.state_mut().rig.move_alpha_keys(a, layer_id, from, to);
                true
            }
            _ => false,
        };
        if moved {
            e.rig_ui.time_ms = to;
            e.doc.commit("Move Keys");
        }
    } else if let Some((row, t)) = track.delete_key {
        let done = match (row, sel, &ik_info, cur) {
            (Row::Bone, Some(b), _, Some(a)) => {
                e.doc.state_mut().rig.remove_keys_at(a, b, t);
                true
            }
            (Row::Ik, _, Some(c), Some(a)) => {
                e.doc.state_mut().rig.remove_ik_keys_at(a, c.id, t);
                true
            }
            (Row::Alpha, _, _, Some(a)) => {
                e.doc.state_mut().rig.remove_alpha_keys_at(a, layer_id, t);
                true
            }
            _ => false,
        };
        if done {
            e.doc.commit("Delete Keys");
        }
    } else if let Some(alpha) = alpha_key {
        if let Some(a) = cur {
            let curve = e.rig_ui.easing.curve();
            e.doc.state_mut().rig.set_alpha_key(a, layer_id, time, alpha, curve);
            e.doc.commit(if alpha < 0.5 { "Hide Layer Key" } else { "Show Layer Key" });
        }
    } else if let Some(rest) = rest_alpha {
        e.doc.state_mut().rig.set_rest_alpha(layer_id, rest);
        e.doc.commit(if rest < 0.5 { "Hide Layer at Rest" } else { "Show Layer at Rest" });
    } else if make_mesh {
        if let Some(b) = layer_bounds {
            // Starts rigid (all weight on the layer's bone), looking just like
            // before; Auto weights or the brush blend it from there.
            if e.doc.state_mut().rig.add_mesh(layer_id, b, rui.cell) {
                e.doc.commit("Make Mesh");
            }
        }
    } else if auto_near || auto_all {
        e.doc.state_mut().rig.auto_weights(layer_id, auto_near);
        e.doc.commit("Auto Weights");
    } else if del_mesh {
        e.doc.state_mut().rig.remove_mesh(layer_id);
        e.rig_ui.weights = false;
        e.doc.commit("Remove Mesh");
    } else if add_ik {
        if let Some((_, target)) = sel.and_then(|b| e.doc.state_mut().rig.add_ik(b)) {
            e.rig_ui.sel = Some(target);
            e.doc.commit("Add IK");
        }
    } else if del_ik {
        if let Some(c) = &ik_info {
            e.doc.state_mut().rig.remove_ik(c.id);
            e.doc.commit("Remove IK");
        }
    } else if key_ik {
        if let (Some(c), Some(a)) = (&ik_edit, cur) {
            let curve: Curve = e.rig_ui.easing.curve();
            let v = qsketch_core::IkValue { mix: c.mix, bend: c.bend };
            e.doc.state_mut().rig.set_ik_key(a, c.id, time, v, curve);
            e.doc.commit("Key IK");
        }
    } else if attach {
        if let Some(b) = sel {
            e.doc.state_mut().rig.attach(layer_id, b);
            e.doc.commit("Attach Layer to Bone");
        }
    } else if detach {
        e.doc.state_mut().rig.detach(layer_id);
        e.doc.commit("Detach Layer from Bone");
    } else if e.rig_ui.edited && !ui.input(|i| i.pointer.any_down()) && ui.memory(|m| m.focused().is_none()) {
        e.rig_ui.edited = false;
        e.doc.commit("Edit Rig Animation");
    }
    if open_dialog {
        crate::dialogs::rig::open(state, doc_id);
    }
    if export_rig {
        crate::files::export_rig(state, doc_id);
    }
}

fn target_name(rig: &qsketch_core::Rig, id: BoneId) -> String {
    rig.bone(id).map_or_else(|| "?".into(), |b| b.name.clone())
}
