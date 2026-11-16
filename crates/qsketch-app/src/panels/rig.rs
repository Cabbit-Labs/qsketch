//! The Rig panel: everything the Bone tool needs besides the canvas itself,
//! in a bottom panel like the Timeline. Pick or create an animation, play and
//! scrub it on a key track, set the easing of new keys, and handle the limb
//! (IK), the active layer's mesh and weights, and what the layer follows.

use egui::{Align2, Color32, FontId, Pos2, Sense, Stroke, Ui};
use qsketch_core::rig::{BoneId, Curve, KeyValue};
use qsketch_core::{IkConstraint, LayerId};

use super::rig_graph;
use crate::dialogs::rig::Easing;
use crate::state::AppState;
use crate::ui::icons;

/// Height of one row of the dopesheet, and the width of its name column.
const ROW_H: f32 = 20.0;
const GUTTER: f32 = 136.0;

/// What a row of the dopesheet holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    /// A bone's keys.
    Bone(BoneId),
    /// An IK constraint's keys (mix and bend).
    Ik(u32),
    /// A layer's opacity keys (show, hide, fade).
    Alpha(LayerId),
}

/// One row of the dopesheet.
struct TrackRow {
    row: Row,
    label: String,
    keys: Vec<u32>,
    color: Option<Color32>,
    /// The selected bone, its IK, or the active layer.
    selected: bool,
}

/// What the dopesheet was used for this frame.
#[derive(Default)]
struct TrackOut {
    /// The playhead was scrubbed (or a key was clicked) to this time.
    scrub: Option<u32>,
    /// A key was right-clicked: its row and time.
    delete_key: Option<(Row, u32)>,
    /// A key was dragged to a new time: its row, from and to.
    move_key: Option<(Row, u32, u32)>,
    /// A row was clicked: select what it shows.
    select: Option<Row>,
}

/// A key being dragged along the track: its row and the time it started at.
#[derive(Clone, Copy)]
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

/// The dopesheet: a ruler with the playhead and one row of keys for every
/// bone, IK constraint and layer opacity the animation keys (plus the
/// selected bone and the active layer, keyed or not), names on the left.
/// Click or drag to scrub (keys attract the playhead unless Shift is held);
/// click a row to select its bone; drag a key to retime it; right-click a key
/// to delete it.
fn key_track(
    ui: &mut Ui,
    duration_ms: u32,
    time_ms: u32,
    rows: &[TrackRow],
    frames: &[u32],
    current_frame: usize,
) -> TrackOut {
    // With frames to show, a band under the ruler marks where each one plays.
    let band = if frames.is_empty() { 0.0 } else { 16.0 };
    let head = 20.0 + band;
    let mut out = TrackOut::default();
    let width = ui.available_width().max(GUTTER + 160.0);
    let height = head + ROW_H * rows.len().max(1) as f32 + 6.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), Sense::click_and_drag());
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

    let (x0, x1) = (rect.left() + GUTTER, rect.right() - 14.0);
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
    // The document's frames, cycling over the animation.
    let cycle: u32 = frames.iter().sum();
    if cycle > 0 {
        let (y0, y1) = (rect.top() + 16.0, rect.top() + 29.0);
        let (mut t, mut i) = (0u32, 0usize);
        while t < duration_ms {
            let end = (t + frames[i].max(1)).min(duration_ms);
            let r = egui::Rect::from_min_max(Pos2::new(to_x(t) + 0.5, y0), Pos2::new(to_x(end) - 0.5, y1));
            let fill = if i == current_frame {
                accent.gamma_multiply(0.55)
            } else if i % 2 == 0 {
                weak.gamma_multiply(0.22)
            } else {
                weak.gamma_multiply(0.10)
            };
            painter.rect_filled(r, 2.0, fill);
            if r.width() >= 14.0 {
                painter.text(r.center(), Align2::CENTER_CENTER, format!("{}", i + 1), font.clone(), text);
            }
            t = end;
            i = (i + 1) % frames.len();
        }
    }
    // Rows: a name on the left, a line with its keys on the right.
    let rows_top = rect.top() + head;
    let row_y = |i: usize| rows_top + ROW_H * (i as f32 + 0.5);
    let row_at = |y: f32| {
        let i = ((y - rows_top) / ROW_H).floor();
        (y >= rows_top && (i as usize) < rows.len()).then_some(i as usize)
    };
    let name_font = FontId::proportional(11.0);
    let names = painter.with_clip_rect(egui::Rect::from_min_max(rect.min, Pos2::new(x0 - 12.0, rect.bottom())));
    for (i, row) in rows.iter().enumerate() {
        let y = row_y(i);
        if row.selected {
            let r = egui::Rect::from_min_max(
                Pos2::new(rect.left() + 1.0, y - ROW_H / 2.0),
                Pos2::new(rect.right() - 1.0, y + ROW_H / 2.0),
            );
            painter.rect_filled(r, 0.0, accent.gamma_multiply(0.22));
        }
        painter.line_segment([Pos2::new(x0, y), Pos2::new(x1, y)], Stroke::new(1.0, weak.gamma_multiply(0.35)));
        let col = if row.selected { text } else { row.color.map_or(weak, |c| c.gamma_multiply(0.8)) };
        names.text(Pos2::new(rect.left() + 8.0, y), Align2::LEFT_CENTER, &row.label, name_font.clone(), col);
    }
    if rows.is_empty() {
        painter.text(
            Pos2::new(rect.left() + 8.0, row_y(0)),
            Align2::LEFT_CENTER,
            "Nothing is keyed yet: drag a bone on the canvas to key it here.",
            name_font.clone(),
            weak,
        );
    }

    // Keys.
    let r = 5.0;
    for (i, row) in rows.iter().enumerate() {
        let base = row.color.unwrap_or(text);
        for &k in &row.keys {
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
    let near = |i: usize, x: f32| rows[i].keys.iter().copied().find(|&k| (to_x(k) - x).abs() <= 7.0);
    let near_any = |x: f32| (0..rows.len()).find_map(|i| near(i, x));
    let in_names = |x: f32| x < x0 - 9.0;
    let drag_id = ui.id().with("key_drag");
    if resp.drag_started() {
        if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
            if let Some(i) = row_at(p.y).filter(|_| !in_names(p.x)) {
                if let Some(from) = near(i, p.x) {
                    ui.data_mut(|d| d.insert_temp(drag_id, KeyDrag { row: rows[i].row, from }));
                }
            }
        }
    }
    let key_drag: Option<KeyDrag> = ui.data(|d| d.get_temp(drag_id));
    if let Some(kd) = key_drag {
        // Dragging a key retimes it (in 5 ms steps); the playhead stays put.
        if let Some(p) = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.latest_pos())) {
            let to = (to_t(p.x) + 2) / 5 * 5;
            let y = rows.iter().position(|r| r.row == kd.row).map_or(p.y, row_y);
            let c = Color32::from_rgb(255, 200, 40);
            diamond(&painter, Pos2::new(to_x(to), y), r, c.gamma_multiply(0.85), Stroke::new(1.5, Color32::WHITE));
            painter.text(Pos2::new(to_x(to) + 9.0, y - 5.0), Align2::LEFT_TOP, format!("{to} ms"), font.clone(), text);
            if resp.drag_stopped() && to != kd.from {
                out.move_key = Some((kd.row, kd.from, to));
            }
        }
        if resp.drag_stopped() {
            ui.data_mut(|d| d.remove::<KeyDrag>(drag_id));
        }
    } else if let Some(p) = resp.interact_pointer_pos() {
        let row = row_at(p.y);
        if resp.clicked() {
            out.select = row.map(|i| rows[i].row);
        }
        let from_names = ui.input(|i| i.pointer.press_origin()).is_some_and(|o| in_names(o.x));
        if (resp.dragged() || resp.clicked()) && !from_names {
            let snap = !ui.input(|i| i.modifiers.shift);
            // Whole 5 ms steps (like retiming), unless Shift is held.
            let mut t = if snap { ((to_t(p.x) + 2) / 5 * 5).min(duration_ms) } else { to_t(p.x) };
            if snap {
                // The row under the pointer first, then any row.
                if let Some(k) = row.and_then(|i| near(i, p.x)).or_else(|| near_any(p.x)) {
                    t = k;
                }
            }
            out.scrub = Some(t);
        }
    }
    if resp.secondary_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some(i) = row_at(p.y) {
                out.delete_key = near(i, p.x).map(|k| (rows[i].row, k));
            }
        }
    }
    resp.on_hover_text(
        "Click or drag to scrub (Shift: no snapping to keys). Click a row to select its bone or layer. Drag a key to retime it; right-click a key to delete it.",
    );
    out
}

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let Some(doc_id) = state.active_doc else {
        ui.label("Open a document to rig it.");
        return;
    };
    let bone_tool = state.effective_tool() == crate::tools::ToolKind::Bone;
    let mut use_bone_tool = false;
    let Some(e) = state.doc(doc_id) else { return };
    let mut rui = e.rig_ui.clone();
    let rig = &e.doc.state().rig;
    let cur = rui.anim_in(rig);
    rui.anim = cur;
    let names: Vec<String> = rig.animations.iter().map(|a| a.name.clone()).collect();
    let mix_ms = rig.mix_ms();
    let mut mix_edit = mix_ms;
    let mixing = rui.mixing(rig).map(|(from, _, w)| (rig.animations[from].name.clone(), w));
    let was = (cur, rui.time_ms, rui.playing);
    let original = cur.map(|a| rig.animations[a].clone());
    let mut anim_edit = original.clone();
    let layer = e.doc.state().active_layer();
    let (layer_id, layer_name) = (layer.id(), layer.name().to_string());
    // Over every frame's picture, so a mesh covers the art in all of them.
    let layer_bounds = layer
        .cels
        .iter()
        .filter_map(|c| c.image.full_bounds())
        .chain(layer.raster.full_bounds())
        .reduce(|a, b| a.union(&b));
    let mesh_dims = rig.mesh_of(layer_id).map(|m| (m.cols, m.rows));
    let layer_bone = rig.attached_bone(layer_id);
    let attached = layer_bone.and_then(|b| rig.bone(b)).map(|b| b.name.clone());
    let sel_name = rui.sel.and_then(|b| rig.bone(b)).map(|b| b.name.clone());
    let bone_keys: Vec<u32> = rui.sel.zip(cur).map(|(b, a)| rig.key_times(a, b)).unwrap_or_default();
    let key_here = bone_keys.contains(&rui.time_ms);
    let st = e.doc.state();
    let (frame_count, current_frame) = (st.frame_count(), st.frame);
    let frame_durs: Vec<u32> = if frame_count > 1 && rui.sync_frames {
        (0..frame_count).map(|f| st.frame_duration(f)).collect()
    } else {
        Vec::new()
    };
    let rest_alpha_now = rig.rest_alpha_of(layer_id);
    let alpha_now = rig.alpha_at(cur.map(|a| (a, rui.time_ms)), layer_id);
    if !ui.memory(|m| m.focused().is_some()) {
        rui.alpha_key = alpha_now;
    }
    let ik_info: Option<IkConstraint> =
        rui.sel.and_then(|b| rig.ik_of_bone(b).or_else(|| rig.ik.iter().find(|c| c.target == b)).cloned());
    let can_add_ik = ik_info.is_none() && rui.sel.and_then(|b| rig.bone(b)).is_some_and(|b| b.parent.is_some());
    let mut ik_edit = ik_info.clone();
    // The dopesheet's rows: everything this animation keys, plus the selected
    // bone, its IK and the active layer's opacity so they can be found there.
    let mut rows: Vec<TrackRow> = Vec::new();
    if let Some(a) = cur {
        for b in &rig.bones {
            let keys = rig.key_times(a, b.id);
            let selected = rui.sel == Some(b.id);
            if !keys.is_empty() || selected {
                rows.push(TrackRow { row: Row::Bone(b.id), label: b.name.clone(), keys, color: None, selected });
            }
        }
        for c in &rig.ik {
            let keys: Vec<u32> = rig.animations[a]
                .ik
                .iter()
                .find(|t| t.constraint == c.id)
                .map(|t| t.keys.iter().map(|k| k.time_ms).collect())
                .unwrap_or_default();
            let selected = ik_info.as_ref().is_some_and(|i| i.id == c.id);
            if !keys.is_empty() || selected {
                let color = Some(Color32::from_rgb(110, 220, 110));
                rows.push(TrackRow { row: Row::Ik(c.id), label: format!("IK {}", c.name), keys, color, selected });
            }
        }
        for l in &st.layers {
            let keys = rig.alpha_key_times(a, l.id());
            let selected = l.id() == layer_id;
            if !keys.is_empty() || selected {
                let color = Some(Color32::from_rgb(255, 160, 80));
                let label = format!("{} opacity", l.name());
                rows.push(TrackRow { row: Row::Alpha(l.id()), label, keys, color, selected });
            }
        }
    }

    // The selected bone, as edited in the Bone row.
    let sel_bone = rui.sel.and_then(|b| rig.bone(b)).cloned();
    let mut bone_edit = sel_bone.clone();
    let offs = rui.sel.zip(cur).map(|(b, a)| rig.offsets(a, b, rui.time_ms));
    let mut offs_edit = offs;
    let mut new_parent: Option<Option<BoneId>> = None;
    let drive = rui.sel.and_then(|b| rig.drive_of(b)).copied();
    let mut drive_edit = drive;

    let (mut new_anim, mut del_anim, mut attach, mut detach, mut del_keys, mut open_dialog) =
        (false, false, false, false, false, false);
    let (mut add_ik, mut del_ik, mut key_ik) = (false, false, false);
    let (mut make_mesh, mut auto_near, mut auto_all, mut del_mesh) = (false, false, false, false);
    let mut export_rig = false;
    let mut close_loop = false;
    let mut key_all = false;
    let (mut copy_pose, mut paste_pose) = (false, false);
    let mut key_bone = false;
    let mut pick_bone: Option<BoneId> = None;
    let mut alpha_key: Option<f32> = None;
    let mut alpha_switch: Option<f32> = None;
    let mut rest_alpha: Option<f32> = None;
    let mut track = TrackOut::default();
    // Skins.
    let skin_names: Vec<String> = rig.skins.iter().map(|s| s.name.clone()).collect();
    let skin_cur = rig.active_skin();
    let in_skin = skin_cur.is_some_and(|i| rig.skins[i].layers.contains(&layer_id));
    let layer_skins: Vec<&str> =
        rig.skins.iter().filter(|s| s.layers.contains(&layer_id)).map(|s| s.name.as_str()).collect();
    let skin_note = match (layer_skins.is_empty(), skin_names.is_empty()) {
        (_, true) => String::new(),
        (true, false) => format!("{layer_name}: in no skin, always shown"),
        (false, false) => format!("{layer_name}: in {}", layer_skins.join(", ")),
    };
    let mut skin_pick: Option<Option<usize>> = None;
    let mut skin_name = skin_cur.map(|i| skin_names[i].clone());
    let (mut new_skin, mut dup_skin, mut del_skin) = (false, false, false);
    let mut skin_member: Option<bool> = None;

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 5.0);
        // Poses, animations and skins are only drawn with the Bone tool. The
        // notice goes at the end of a row that is there anyway, so it never
        // pushes the controls around.
        let mut tool_notice = |ui: &mut Ui| {
            if !bone_tool {
                use_bone_tool = ui.button("Use the Bone tool").clicked();
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "The canvas shows the layers as drawn: the pose, animation and skin show with the Bone tool.",
                );
            }
        };
        // --- animation -------------------------------------------------------
        ui.horizontal_wrapped(|ui| {
            row_label(ui, "Animation");
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
                if names.len() > 1 {
                    ui.add(egui::DragValue::new(&mut mix_edit).range(0..=5000).speed(5).prefix("mix ").suffix(" ms"))
                        .on_hover_text(
                            "Crossfade: pick another animation while this one plays and the pose blends into it over this long, the way a game switches animations (0 switches at once). Exported as the default mix.",
                        );
                }
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
                if frame_count > 1 {
                    ui.checkbox(&mut rui.sync_frames, "Sync frames").on_hover_text(
                        "The playhead also steps the document's frames (they repeat over the animation), and picking a frame in the Timeline moves the playhead there",
                    );
                    ui.separator();
                }
                ui.label("Easing for new keys");
                egui::ComboBox::from_id_salt("rig_panel_easing").selected_text(rui.easing.label()).width(100.0).show_ui(
                    ui,
                    |ui| {
                        for e in Easing::ALL {
                            ui.selectable_value(&mut rui.easing, e, e.label());
                        }
                    },
                );
                ui.toggle_value(&mut rui.graph, "Curves").on_hover_text(
                    "Show the curve graph: the selected bone's values over time, with handles to shape how it moves between keys",
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
                    .add_enabled(sel_name.is_some(), egui::Button::new("Copy pose"))
                    .on_hover_text("Copy the selected bone's pose at the playhead (what it keys: turn, move, scale)")
                    .clicked()
                {
                    copy_pose = true;
                }
                if ui
                    .add_enabled(sel_name.is_some() && rui.pose_clip.is_some(), egui::Button::new("Paste pose"))
                    .on_hover_text("Key the copied pose on the selected bone at the playhead: repeat a swing later, or give another bone the same move")
                    .clicked()
                {
                    paste_pose = true;
                }
                if ui
                    .add_enabled(key_here, egui::Button::new("Delete keys here"))
                    .on_hover_text("Delete the selected bone's keys at the playhead")
                    .clicked()
                {
                    del_keys = true;
                }
                // Last in the row, so nothing moves when it comes and goes.
                if let Some((from, w)) = &mixing {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("mixing from “{from}” {:.0}%", w * 100.0));
                }
                tool_notice(ui);
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut rui.adopt, "New bones take the active layer").on_hover_text(
                    "A bone you draw is attached to the active layer and named after it, when that layer follows no bone yet. Pick a layer, draw its bone, pick the next.",
                );
                if bone_tool {
                    ui.label(
                        egui::RichText::new(
                            "Setup pose: bones you draw or drag edit the skeleton itself. Pick or create an animation to pose and key bones over time.",
                        )
                        .weak(),
                    );
                }
                tool_notice(ui);
            });
        }
        ui.separator();

        // --- the selected bone: name, parent, numbers, what turns it ------------
        ui.horizontal_wrapped(|ui| {
            // Every bone by name, children indented under their parents: the
            // way to a bone that lies under another one on the canvas.
            ui.menu_button(egui::RichText::new("Bone ⏷").strong(), |ui| {
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    if rig.bones.is_empty() {
                        ui.label(egui::RichText::new("No bones yet: drag on the canvas to draw one.").weak());
                    }
                    // As a tree: each bone followed by its children.
                    let mut stack: Vec<(BoneId, usize)> =
                        rig.bones.iter().rev().filter(|o| o.parent.is_none()).map(|o| (o.id, 0)).collect();
                    while let Some((id, depth)) = stack.pop() {
                        let Some(o) = rig.bone(id) else { continue };
                        let label = format!("{}{}", "    ".repeat(depth), o.name);
                        if ui.selectable_label(rui.sel == Some(id), label).clicked() {
                            pick_bone = Some(id);
                            ui.close();
                        }
                        stack.extend(rig.bones.iter().rev().filter(|c| c.parent == Some(id)).map(|c| (c.id, depth + 1)));
                    }
                });
            })
            .response
            .on_hover_text("Pick any bone by name");
            let Some(b) = bone_edit.as_mut() else {
                ui.label(egui::RichText::new("click a bone on the canvas, or pick one here").weak());
                return;
            };
            ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(150.0)).on_hover_text("The bone's name");
            let parent_text = b.parent.map_or_else(|| "no parent".to_string(), |p| format!("in {}", target_name(rig, p)));
            egui::ComboBox::from_id_salt("rig_panel_parent")
                .selected_text(parent_text)
                .width(110.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(b.parent.is_none(), "no parent").clicked() && b.parent.is_some() {
                        new_parent = Some(None);
                    }
                    for o in rig.bones.iter().filter(|o| rig.can_parent(b.id, Some(o.id))) {
                        if ui.selectable_label(b.parent == Some(o.id), &o.name).clicked() && b.parent != Some(o.id) {
                            new_parent = Some(Some(o.id));
                        }
                    }
                })
                .response
                .on_hover_text("The bone this one hangs from and moves with. Changing it keeps the bone where it is.");
            match offs_edit.as_mut() {
                None => {
                    ui.add(egui::DragValue::new(&mut b.x).speed(0.5).max_decimals(1).prefix("x "));
                    ui.add(egui::DragValue::new(&mut b.y).speed(0.5).max_decimals(1).prefix("y "));
                    ui.add(egui::DragValue::new(&mut b.rotation).speed(0.5).max_decimals(1).prefix("angle ").suffix("°"));
                    ui.add(
                        egui::DragValue::new(&mut b.length).speed(0.5).range(1.0..=10_000.0).max_decimals(1).prefix("length "),
                    );
                }
                Some((tr, rot, sc)) => {
                    // Fixed widths, so the buttons after a number don't move
                    // when it gains a digit.
                    let num = |ui: &mut Ui, w: f32, d: egui::DragValue| ui.add_sized([w, ui.spacing().interact_size.y], d);
                    if ui
                        .button("Key")
                        .on_hover_text("Key this bone where it is now at the playhead, so it holds this pose until here")
                        .clicked()
                    {
                        key_bone = true;
                    }
                    num(ui, 84.0, egui::DragValue::new(rot).speed(1.0).max_decimals(1).prefix("turn ").suffix("°")).on_hover_text(
                        "Degrees turned from the setup pose at the playhead; changing it keys it. It isn't limited to a half turn: 360 is one full spin, 720 two, -360 one the other way.",
                    );
                    if ui.small_button("−360°").on_hover_text("Key one more full turn counter-clockwise here").clicked() {
                        *rot -= 360.0;
                    }
                    if ui.small_button("+360°").on_hover_text("Key one more full turn clockwise here").clicked() {
                        *rot += 360.0;
                    }
                    num(ui, 92.0, egui::DragValue::new(&mut tr[0]).speed(0.5).max_decimals(1).prefix("move x "))
                        .on_hover_text("Moved from the setup pose at the playhead; changing it keys it");
                    num(ui, 58.0, egui::DragValue::new(&mut tr[1]).speed(0.5).max_decimals(1).prefix("y "));
                    let mut k = sc[0];
                    if num(ui, 76.0, egui::DragValue::new(&mut k).speed(0.01).range(0.02..=8.0).max_decimals(2).prefix("scale "))
                        .on_hover_text("Scale at the playhead; changing it keys it")
                        .changed()
                    {
                        *sc = [k, k];
                    }
                }
            }
            ui.separator();
            ui.label("Turned by");
            let text = drive_edit.map_or_else(|| "nothing".to_string(), |d| target_name(rig, d.source));
            egui::ComboBox::from_id_salt("rig_panel_drive")
                .selected_text(text)
                .width(110.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(drive_edit.is_none(), "nothing").clicked() {
                        drive_edit = None;
                    }
                    for o in rig.bones.iter().filter(|o| rig.can_drive(b.id, o.id)) {
                        let on = drive_edit.is_some_and(|d| d.source == o.id);
                        if ui.selectable_label(on, &o.name).clicked() && !on {
                            // Meshed gears whose bones reach their rims: the
                            // smaller one turns faster, the other way.
                            let ratio = -(o.length / b.length.max(1.0));
                            // Hand-drawn bones are never exact: take the
                            // nearest half or third when it is close, so the
                            // gears still line up when the animation loops.
                            let snapped = [2.0f32, 3.0]
                                .into_iter()
                                .map(|d| (ratio * d).round() / d)
                                .find(|s| (s - ratio).abs() <= 0.04 * ratio.abs().max(0.25) && *s != 0.0);
                            let ratio = snapped.unwrap_or((ratio * 100.0).round() / 100.0);
                            drive_edit = Some(qsketch_core::Drive { bone: b.id, source: o.id, ratio });
                        }
                    }
                })
                .response
                .on_hover_text(
                    "Gears: this bone turns whenever the chosen bone does in an animation, by the ratio next to it. Key the one bone and everything meshed with it follows.",
                );
            if let Some(d) = drive_edit.as_mut() {
                ui.add(egui::DragValue::new(&mut d.ratio).speed(0.01).max_decimals(3).prefix("× ")).on_hover_text(
                    "Turns per turn of the driving bone. Negative turns the other way (meshed gears); -2 is a gear half the size. It starts as the ratio of the two bones' lengths.",
                );
            }
        });
        ui.separator();

        // --- layer, mesh, limb: a row each, so every control keeps its place
        // whatever is selected.
        ui.horizontal_wrapped(|ui| {
            row_label(ui, "Layer");
            // Buttons first and the names after, so the buttons stay put when
            // the active layer changes.
            if rui.sel.is_some() && rui.sel == layer_bone {
                detach = ui
                    .add_sized([64.0, 20.0], egui::Button::new("Detach"))
                    .on_hover_text("Stop the active layer following its bone")
                    .clicked();
            } else if ui
                .add_enabled_ui(sel_name.is_some(), |ui| ui.add_sized([64.0, 20.0], egui::Button::new("Attach")))
                .inner
                .on_hover_text(match (&sel_name, &attached) {
                    (Some(n), Some(old)) => format!("Move the active layer from “{old}” to “{n}”"),
                    (Some(n), None) => format!("Make the active layer follow “{n}”"),
                    (None, _) => "Select a bone on the canvas first".to_string(),
                })
                .clicked()
            {
                attach = true;
            }
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
                if ui
                    .button("Show")
                    .on_hover_text("Show the layer from the playhead on, at once (to fade instead, set an opacity and Key it)")
                    .clicked()
                {
                    alpha_switch = Some(1.0);
                }
                if ui
                    .button("Hide")
                    .on_hover_text("Hide the layer from the playhead on, at once (to fade instead, set an opacity and Key it)")
                    .clicked()
                {
                    alpha_switch = Some(0.0);
                }
                ui.add(egui::DragValue::new(&mut rui.alpha_key).speed(0.01).range(0.0..=1.0).prefix("opacity "))
                    .on_hover_text("Opacity of the active layer at the playhead right now");
                if ui.button("Key").on_hover_text("Key this opacity at the playhead").clicked() {
                    alpha_key = Some(rui.alpha_key);
                }
            }
            // The names last: they change with the active layer, and nothing
            // after them can be pushed around.
            ui.separator();
            ui.label(match &attached {
                Some(b) => format!("{layer_name} → {b}"),
                None => format!("{layer_name}: not attached"),
            });
        });
        // The mesh and the limb share a row; the mesh group keeps a fixed
        // width so the limb controls after it don't move with the layer.
        ui.horizontal_wrapped(|ui| {
            row_label(ui, "Mesh");
            let mesh_size = egui::vec2(470.0, 20.0);
            ui.allocate_ui_with_layout(mesh_size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_min_size(mesh_size);
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
                            .on_hover_text("Weight each vertex to this layer's bone, its parent and the chain of bones that hangs from it")
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
            });
            ui.separator();
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
        });

        // --- skins ---------------------------------------------------------------
        ui.horizontal_wrapped(|ui| {
            row_label(ui, "Skin").on_hover_text(
                "Skins swap outfits or whole characters on one skeleton. A layer you put in a skin shows only while that skin is worn; layers in no skin always show.",
            );
            let text = skin_cur.map_or_else(|| "All layers".to_string(), |i| skin_names[i].clone());
            egui::ComboBox::from_id_salt("rig_panel_skin").selected_text(text).width(130.0).show_ui(ui, |ui| {
                if ui
                    .selectable_label(skin_cur.is_none(), "All layers")
                    .on_hover_text("Wear no skin: every layer shows, to work on the art")
                    .clicked()
                {
                    skin_pick = Some(None);
                }
                for (i, n) in skin_names.iter().enumerate() {
                    if ui.selectable_label(skin_cur == Some(i), n).clicked() {
                        skin_pick = Some(Some(i));
                    }
                }
            });
            new_skin = ui.button("New").on_hover_text("New skin").clicked();
            if let Some(name) = skin_name.as_mut() {
                ui.add(egui::TextEdit::singleline(name).desired_width(110.0));
                let mut member = in_skin;
                if ui
                    .checkbox(&mut member, "Active layer in this skin")
                    .on_hover_text("The active layer shows only while a skin it is in is worn")
                    .changed()
                {
                    skin_member = Some(member);
                }
                dup_skin = ui
                    .button("Duplicate")
                    .on_hover_text("A new skin with the same layers, to swap some of them out")
                    .clicked();
                del_skin = ui
                    .button(format!("{} Delete", icons::TRASH))
                    .on_hover_text("Delete this skin (its layers stay, and show always again)")
                    .clicked();
            }
            if !skin_note.is_empty() {
                ui.separator();
                ui.label(&skin_note);
            }
        });

        // --- the curve graph, above the dopesheet so it stays put too.
        let mut graph_scrub = None;
        if let (true, Some(a)) = (rui.graph, anim_edit.as_mut()) {
            let subject = rig_graph::Subject { bone: rui.sel, ik: ik_info.as_ref().map(|c| c.id), layer: layer_id };
            let width = ui.available_width().max(GUTTER + 160.0) - GUTTER - 14.0;
            let step = ruler_step(a.duration_ms, width);
            ui.separator();
            if let Some(t) = rig_graph::ui(ui, a, &subject, rui.time_ms, GUTTER, step) {
                graph_scrub = Some(t);
            }
        }
        // --- the dopesheet, last: it grows with the animation, and the
        // controls above it stay where they are.
        if let Some(a) = &anim_edit {
            ui.separator();
            track = key_track(ui, a.duration_ms, rui.time_ms, &rows, &frame_durs, current_frame);
        }
        track.scrub = track.scrub.or(graph_scrub);
    });

    // --- write back --------------------------------------------------------------
    let Some(e) = state.doc_mut(doc_id) else { return };
    // Another animation picked while one plays: crossfade into it.
    if let ((Some(from), from_ms, true), Some(to)) = (was, rui.anim) {
        if from != to && rui.playing && mix_ms > 0 {
            e.rig_ui.start_mix(from, from_ms);
        }
    }
    if mix_edit != mix_ms {
        e.doc.state_mut().rig.set_mix_ms(mix_edit);
        e.rig_ui.edited = true;
    }
    e.rig_ui.anim = rui.anim;
    e.rig_ui.easing = rui.easing;
    e.rig_ui.graph = rui.graph;
    e.rig_ui.nearest = rui.nearest;
    e.rig_ui.weights = rui.weights;
    e.rig_ui.brush = rui.brush;
    e.rig_ui.strength = rui.strength;
    e.rig_ui.cell = rui.cell;
    e.rig_ui.playing = rui.playing;
    e.rig_ui.alpha_key = rui.alpha_key;
    e.rig_ui.sync_frames = rui.sync_frames;
    e.rig_ui.adopt = rui.adopt;
    e.rig_ui.time_ms = rui.time_ms;
    if let Some(t) = track.scrub {
        e.rig_ui.time_ms = t;
        e.rig_ui.playing = false;
    }
    match track.select {
        Some(Row::Bone(b)) => e.rig_ui.sel = Some(b),
        Some(Row::Ik(id)) => {
            if let Some(c) = e.doc.state().rig.ik.iter().find(|c| c.id == id) {
                e.rig_ui.sel = Some(c.target);
            }
        }
        Some(Row::Alpha(l)) => {
            if let Some(i) = e.doc.state().layers.iter().position(|x| x.id() == l) {
                e.doc.state_mut().active = i;
            }
        }
        None => {}
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
    // The Bone row. Its edits are committed like the animation fields: once
    // the field is let go.
    if let (Some(b), true) = (&bone_edit, bone_edit != sel_bone) {
        if let Some(slot) = e.doc.state_mut().rig.bone_mut(b.id) {
            *slot = b.clone();
            e.rig_ui.edited = true;
        }
    }
    if let (Some(b), Some(a), Some(new), Some(old)) = (sel, cur, offs_edit, offs) {
        let curve = e.rig_ui.easing.curve();
        let rig = &mut e.doc.state_mut().rig;
        if new.0 != old.0 {
            rig.key_from_rest(a, b, time, KeyValue::Translate(new.0), curve);
        }
        if new.1 != old.1 {
            rig.key_from_rest(a, b, time, KeyValue::Rotate(new.1), curve);
        }
        if new.2 != old.2 {
            rig.key_from_rest(a, b, time, KeyValue::Scale(new.2), curve);
        }
        if new != old {
            e.rig_ui.edited = true;
        }
    }
    if drive_edit != drive {
        if let Some(b) = sel {
            let rig = &mut e.doc.state_mut().rig;
            match drive_edit {
                Some(d) => {
                    rig.set_drive(b, d.source, d.ratio);
                }
                None => rig.remove_drive(b),
            }
            e.rig_ui.edited = true;
        }
    }
    // Skins.
    if let Some(pick) = skin_pick {
        if pick != skin_cur {
            e.doc.state_mut().rig.skin = pick;
            e.doc.commit("Change Skin");
        }
    }
    if let (Some(i), Some(name)) = (skin_cur, &skin_name) {
        let rig = &mut e.doc.state_mut().rig;
        if rig.skins.get(i).is_some_and(|s| &s.name != name) {
            rig.skins[i].name = name.clone();
            e.rig_ui.edited = true;
        }
    }
    if new_skin {
        let rig = &mut e.doc.state_mut().rig;
        let n = rig.skins.len() + 1;
        rig.add_skin(&format!("skin {n}"));
        e.doc.commit("New Skin");
    } else if dup_skin {
        if let Some(i) = skin_cur {
            e.doc.state_mut().rig.duplicate_skin(i);
            e.doc.commit("Duplicate Skin");
        }
    } else if del_skin {
        if let Some(i) = skin_cur {
            e.doc.state_mut().rig.remove_skin(i);
            e.doc.commit("Delete Skin");
        }
    } else if let (Some(i), Some(inside)) = (skin_cur, skin_member) {
        e.doc.state_mut().rig.set_in_skin(i, layer_id, inside);
        e.doc.commit(if inside { "Add Layer to Skin" } else { "Remove Layer from Skin" });
    }
    if let (Some(b), Some(p)) = (sel, new_parent) {
        if e.doc.state_mut().rig.set_parent(b, p) {
            e.doc.commit("Change Bone Parent");
        }
    }
    if let Some(b) = pick_bone {
        e.rig_ui.sel = Some(b);
    }
    if key_bone {
        if let (Some(b), Some(a)) = (sel, cur) {
            let curve = e.rig_ui.easing.curve();
            let rig = &mut e.doc.state_mut().rig;
            // The turn always; move and scale when the bone animates them.
            let (tr, rot, sc) = rig.offsets(a, b, time);
            let pose = rig.keyed_pose(a, b, time);
            rig.key_from_rest(a, b, time, KeyValue::Rotate(rot), curve);
            if pose[0].is_some() {
                rig.key_from_rest(a, b, time, KeyValue::Translate(tr), curve);
            }
            if pose[2].is_some() {
                rig.key_from_rest(a, b, time, KeyValue::Scale(sc), curve);
            }
            e.doc.commit("Key Bone");
        }
    }
    if copy_pose {
        if let (Some(b), Some(a)) = (sel, cur) {
            e.rig_ui.pose_clip = Some(e.doc.state().rig.keyed_pose(a, b, time));
        }
    }
    if paste_pose {
        if let (Some(b), Some(a), Some(clip)) = (sel, cur, e.rig_ui.pose_clip) {
            let curve = e.rig_ui.easing.curve();
            let rig = &mut e.doc.state_mut().rig;
            for v in clip.into_iter().flatten() {
                rig.key_from_rest(a, b, time, v, curve);
            }
            e.doc.commit("Paste Pose");
        }
    } else if new_anim {
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
    } else if let (Some((row, from, to)), Some(a)) = (track.move_key, cur) {
        let rig = &mut e.doc.state_mut().rig;
        match row {
            Row::Bone(b) => rig.move_keys(a, b, from, to),
            Row::Ik(id) => rig.move_ik_keys(a, id, from, to),
            Row::Alpha(l) => rig.move_alpha_keys(a, l, from, to),
        }
        e.rig_ui.time_ms = to;
        e.doc.commit("Move Keys");
    } else if let (Some((row, t)), Some(a)) = (track.delete_key, cur) {
        let rig = &mut e.doc.state_mut().rig;
        match row {
            Row::Bone(b) => rig.remove_keys_at(a, b, t),
            Row::Ik(id) => rig.remove_ik_keys_at(a, id, t),
            Row::Alpha(l) => rig.remove_alpha_keys_at(a, l, t),
        }
        e.doc.commit("Delete Keys");
    } else if let (Some(alpha), Some(a)) = (alpha_switch, cur) {
        e.doc.state_mut().rig.switch_alpha(a, layer_id, time, alpha);
        e.doc.commit(if alpha < 0.5 { "Hide Layer Key" } else { "Show Layer Key" });
    } else if let Some(alpha) = alpha_key {
        if let Some(a) = cur {
            let curve = e.rig_ui.easing.curve();
            e.doc.state_mut().rig.key_alpha_from_rest(a, layer_id, time, alpha, curve);
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
        e.doc.commit("Edit Rig");
    }
    if use_bone_tool {
        state.set_tool(crate::tools::ToolKind::Bone);
    }
    if open_dialog {
        crate::dialogs::rig::open(state, doc_id);
    }
    if export_rig {
        crate::files::export_rig(state, doc_id);
    }
}

/// The name that starts a row of the panel, in a column of its own so the
/// controls after it line up from row to row.
fn row_label(ui: &mut Ui, name: &str) -> egui::Response {
    let size = egui::vec2(72.0, 20.0);
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_size(size);
        ui.strong(name)
    })
    .inner
}

fn target_name(rig: &qsketch_core::Rig, id: BoneId) -> String {
    rig.bone(id).map_or_else(|| "?".into(), |b| b.name.clone())
}
