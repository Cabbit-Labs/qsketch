//! Timeline panel (Aseprite-style): playback controls, a tag row, a frame
//! header, and one row per layer with a cel marker per frame. Click a cel
//! to go there; drag the header to scrub (and select a run of frames);
//! drag a selected run to move it; right-click for everything else.

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use qsketch_core::LayerId;

use crate::actions::Action;
use crate::anim::{self, TimelineDrag, TimelineHit};
use crate::state::{AppState, DocId};
use crate::ui::icons;
use crate::ui::widgets::{icon_button, icon_toggle, rgba_to_color32};

const ROW_H: f32 = 20.0;
const HEADER_H: f32 = 18.0;
const TAG_LANE_H: f32 = 15.0;
const LEFT_W: f32 = 176.0;
const ICON_FAMILY: fn() -> egui::FontFamily = crate::ui::iconset::family;

/// Work queued by the grid's input handling, run once the painter and the
/// document borrows are gone.
type Deferred = Vec<Box<dyn FnOnce(&mut AppState)>>;

/// What a row shows about its layer.
struct LayerInfo {
    name: String,
    visible: bool,
    locked: bool,
    is_group: bool,
    animated: bool,
    continuous: bool,
    color: Option<[u8; 4]>,
    editable: bool,
}

/// One visible layer row (top to bottom).
struct Row {
    li: usize,
    id: LayerId,
    depth: usize,
}

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let Some(doc_id) = state.active_doc else {
        ui.centered_and_justified(|ui| ui.label(egui::RichText::new("No document").weak()));
        return;
    };
    toolbar(ui, state, doc_id);
    ui.add_space(2.0);
    grid(ui, state, doc_id);
}

fn fps_text(ms: u32) -> String {
    let fps = 1000.0 / ms.max(1) as f32;
    if (fps - fps.round()).abs() < 0.05 {
        format!("{} fps", fps.round() as u32)
    } else {
        format!("{fps:.1} fps")
    }
}

fn toolbar(ui: &mut Ui, state: &mut AppState, doc_id: DocId) {
    let playing = anim::is_playing(state, doc_id);
    let (frame, nframes, cur_ms) = state
        .doc(doc_id)
        .map(|e| {
            let s = e.doc.state();
            (s.frame, s.frame_count(), s.frame_duration(s.frame))
        })
        .unwrap_or((0, 1, 100));
    let sel = anim::selected_frames(state, doc_id);
    let size = 22.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if icon_button(ui, icons::SKIP_BACK, "First frame (Home)", size, false).clicked() {
            anim::first_frame(state, doc_id);
        }
        if icon_button(ui, icons::CARET_LEFT, "Previous frame (,)", size, false).clicked() {
            anim::step(state, doc_id, -1);
        }
        let glyph = if playing { icons::PAUSE } else { icons::PLAY };
        if icon_button(ui, glyph, "Play / Stop (Enter)", size, playing).clicked() {
            anim::toggle_play(state, doc_id);
        }
        if icon_button(ui, icons::CARET_RIGHT, "Next frame (.)", size, false).clicked() {
            anim::step(state, doc_id, 1);
        }
        if icon_button(ui, icons::SKIP_FORWARD, "Last frame (End)", size, false).clicked() {
            anim::last_frame(state, doc_id);
        }
        ui.add_space(6.0);
        icon_toggle(
            ui,
            icons::REPEAT,
            icons::REPEAT,
            &mut state.settings.anim.loop_tag,
            "Loop the tag under the current frame (playback, stepping and onion skins stay inside it)",
            size,
        );
        icon_toggle(
            ui,
            icons::GHOST,
            icons::GHOST,
            &mut state.settings.anim.onion.enabled,
            "Onion skin (F3): the frames before and after, tinted",
            size,
        );
        let gear = icon_button(ui, icons::GEAR_SIX, "Onion skin settings", size, false);
        egui::Popup::from_toggle_button_response(&gear).show(|ui| {
            ui.set_min_width(260.0);
            onion_settings(ui, &mut state.settings.anim.onion);
        });
        ui.add_space(8.0);
        ui.label(egui::RichText::new(format!("{} / {nframes}", frame + 1)).monospace());
        ui.add_space(6.0);
        // Duration of the selected frames; applied when the drag / edit ends.
        let id = ui.id().with("duration_edit");
        let mut ms: u32 = ui.data(|m| m.get_temp(id)).unwrap_or(cur_ms);
        let r = ui.add(egui::DragValue::new(&mut ms).range(1..=65_535).suffix(" ms")).on_hover_text(if sel.len() > 1 {
            format!("Duration of the {} selected frames", sel.len())
        } else {
            "Frame duration".into()
        });
        if r.dragged() || r.has_focus() {
            ui.data_mut(|m| m.insert_temp(id, ms));
        } else {
            let pending: Option<u32> = ui.data(|m| m.get_temp(id));
            if let Some(v) = pending {
                ui.data_mut(|m| m.remove::<u32>(id));
                if v != cur_ms {
                    anim::set_duration(state, doc_id, &sel, v);
                }
            }
        }
        ui.label(egui::RichText::new(fps_text(ms)).weak().small());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let more = icon_button(ui, icons::DOTS_THREE, "More", size, false);
            egui::Popup::menu(&more).show(|ui| animation_menu(ui, state, doc_id));
            if icon_button(ui, icons::TRASH, "Delete the selected frames (Alt+C)", size, false).clicked() {
                anim::delete_frames(state, doc_id);
            }
            if icon_button(ui, icons::TAG, "New tag over the selected frames", size, false).clicked() {
                anim::new_tag(state, doc_id);
            }
            if icon_button(ui, icons::FILE_PLUS, "New empty frame (Alt+B)", size, false).clicked() {
                anim::new_frame(state, doc_id, true);
            }
            if icon_button(ui, icons::PLUS, "New frame: a copy of this one (Alt+N)", size, false).clicked() {
                anim::new_frame(state, doc_id, false);
            }
        });
    });
}

fn onion_settings(ui: &mut Ui, o: &mut qsketch_core::OnionSettings) {
    ui.label(egui::RichText::new("Onion skin").strong());
    egui::Grid::new("onion_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("Frames before");
        ui.add(egui::DragValue::new(&mut o.prev).range(0..=10));
        ui.end_row();
        ui.label("Frames after");
        ui.add(egui::DragValue::new(&mut o.next).range(0..=10));
        ui.end_row();
        ui.label("Opacity");
        let mut pct = o.opacity * 100.0;
        if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%")).changed() {
            o.opacity = pct / 100.0;
        }
        ui.end_row();
        ui.label("Fade per step");
        let mut pct = o.falloff * 100.0;
        if ui.add(egui::Slider::new(&mut pct, 10.0..=100.0).suffix("%")).changed() {
            o.falloff = pct / 100.0;
        }
        ui.end_row();
        ui.label("Tint");
        ui.horizontal(|ui| {
            ui.checkbox(&mut o.tint, "");
            let mut a = Color32::from_rgb(o.tint_prev[0], o.tint_prev[1], o.tint_prev[2]);
            if egui::color_picker::color_edit_button_srgba(ui, &mut a, egui::color_picker::Alpha::Opaque).changed() {
                o.tint_prev = [a.r(), a.g(), a.b()];
            }
            ui.label(egui::RichText::new("before").weak());
            let mut b = Color32::from_rgb(o.tint_next[0], o.tint_next[1], o.tint_next[2]);
            if egui::color_picker::color_edit_button_srgba(ui, &mut b, egui::color_picker::Alpha::Opaque).changed() {
                o.tint_next = [b.r(), b.g(), b.b()];
            }
            ui.label(egui::RichText::new("after").weak());
        });
        ui.end_row();
        ui.label("Tint strength");
        let mut pct = o.tint_amount * 100.0;
        if ui.add_enabled(o.tint, egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%")).changed() {
            o.tint_amount = pct / 100.0;
        }
        ui.end_row();
    });
    ui.checkbox(&mut o.behind, "Behind the artwork");
    ui.checkbox(&mut o.loop_tag, "Stay inside the current tag");
    ui.checkbox(&mut o.active_layer_only, "Active layer only");
}

/// The Animation menu, also reachable from the panel's ⋯ button.
pub fn animation_menu(ui: &mut Ui, state: &mut AppState, doc_id: DocId) {
    let (continuous, in_tag, pixel) = state
        .doc(doc_id)
        .map(|e| {
            let s = e.doc.state();
            (s.active_layer().props.continuous, s.tag_at(s.frame).is_some(), s.active_layer().animated())
        })
        .unwrap_or((false, false, false));
    let item = menu_item;
    item(ui, state, Action::NewFrame, true);
    item(ui, state, Action::NewEmptyFrame, true);
    item(ui, state, Action::DuplicateFrames, true);
    item(ui, state, Action::DeleteFrames, true);
    item(ui, state, Action::FrameProperties, true);
    item(ui, state, Action::ReverseFrames, true);
    ui.separator();
    item(ui, state, Action::PlayAnimation, true);
    item(ui, state, Action::FirstFrame, true);
    item(ui, state, Action::PrevFrame, true);
    item(ui, state, Action::NextFrame, true);
    item(ui, state, Action::LastFrame, true);
    ui.separator();
    item(ui, state, Action::NewTag, true);
    item(ui, state, Action::TagProperties, in_tag);
    item(ui, state, Action::DeleteTag, in_tag);
    ui.separator();
    item(ui, state, Action::ClearCel, pixel);
    item(ui, state, Action::LinkCels, pixel);
    item(ui, state, Action::UnlinkCel, pixel);
    item(ui, state, Action::CopyCel, pixel);
    item(ui, state, Action::PasteCel, pixel);
    item(ui, state, Action::CelProperties, pixel);
    let btn = egui::Button::new(format!(
        "{} {}",
        if continuous { icons::CHECK } else { " " },
        Action::ToggleContinuous.label()
    ))
    .shortcut_text(state.keymap.primary_text(Action::ToggleContinuous));
    if ui
        .add_enabled(pixel, btn)
        .on_hover_text("New frames on this layer keep showing the previous frame's picture (linked) until unlinked")
        .clicked()
    {
        state.pending.push(Action::ToggleContinuous);
        ui.close();
    }
    ui.separator();
    item(ui, state, Action::ToggleOnionSkin, true);
    ui.separator();
    item(ui, state, Action::ImportFrames, true);
    item(ui, state, Action::ImportSpriteSheet, true);
    item(ui, state, Action::ExportAnimation, true);
}

/// Layer rows top to bottom, hiding members of collapsed groups.
fn rows(s: &qsketch_core::DocState) -> Vec<Row> {
    let mut out = Vec::with_capacity(s.layers.len());
    for li in (0..s.layers.len()).rev() {
        let mut depth = 0;
        let mut hidden = false;
        let mut p = s.layers[li].props.parent;
        let mut hops = 0;
        while let Some(pid) = p {
            let Some(pi) = s.index_of(pid) else { break };
            if !s.layers[pi].props.expanded {
                hidden = true;
            }
            depth += 1;
            p = s.layers[pi].props.parent;
            hops += 1;
            if hops > 64 {
                break;
            }
        }
        if !hidden {
            out.push(Row { li, id: s.layers[li].props.id, depth });
        }
    }
    out
}

/// Lane of each tag so overlapping tags stack instead of covering each other.
fn tag_lanes(tags: &[qsketch_core::Tag]) -> (Vec<usize>, usize) {
    let mut lanes: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut out = Vec::with_capacity(tags.len());
    for t in tags {
        let lane = lanes.iter().position(|l| l.iter().all(|&(a, b)| t.to < a || t.from > b)).unwrap_or_else(|| {
            lanes.push(Vec::new());
            lanes.len() - 1
        });
        lanes[lane].push((t.from, t.to));
        out.push(lane);
    }
    (out, lanes.len().max(1))
}

fn text_on(c: Color32) -> Color32 {
    let lum = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if lum > 150.0 {
        Color32::from_black_alpha(220)
    } else {
        Color32::from_white_alpha(230)
    }
}

fn grid(ui: &mut Ui, state: &mut AppState, doc_id: DocId) {
    let Some(entry) = state.doc(doc_id) else { return };
    let s = entry.doc.state();
    let n = s.frame_count();
    let cur = s.frame;
    let active = s.active;
    let rows = rows(s);
    let (lanes, nlanes) = tag_lanes(&s.tags);
    let tags: Vec<qsketch_core::Tag> = s.tags.clone();
    // Per row and frame: (owner frame, has pixels, static layer, group)
    struct CelInfo {
        owner: usize,
        filled: bool,
    }
    let cel_info: Vec<Vec<CelInfo>> = rows
        .iter()
        .map(|r| {
            let l = &s.layers[r.li];
            (0..n)
                .map(|f| CelInfo { owner: l.cel_owner(f), filled: l.animated() && s.cel_has_pixels(r.li, f) })
                .collect()
        })
        .collect();
    let layer_info: Vec<LayerInfo> = rows
        .iter()
        .map(|r| {
            let l = &s.layers[r.li];
            LayerInfo {
                name: l.props.name.clone(),
                visible: l.props.visible,
                locked: l.props.locked,
                is_group: l.is_group(),
                animated: l.animated(),
                continuous: l.props.continuous,
                color: l.props.color,
                editable: s.layer_editable(r.li) || l.is_group(),
            }
        })
        .collect();
    let sel = entry.frame_sel.map(|(a, b)| (a.min(b), a.max(b)));
    let playing = anim::is_playing(state, doc_id);
    let col_w = state.settings.anim.timeline_col_w.clamp(10.0, 64.0);
    let tags_h = nlanes as f32 * TAG_LANE_H;
    let top_h = tags_h + HEADER_H;
    let p = state.settings.ui.palette();
    let accent = p.accent;
    let visuals = ui.visuals().clone();
    let text = visuals.text_color();
    let dim = crate::ui::theme::dim_text(&visuals);
    let zebra = crate::ui::chrome::zebra(ui);
    let bg = visuals.panel_fill;

    let content = Vec2::new(LEFT_W + (n as f32 + 1.0) * col_w, top_h + rows.len() as f32 * ROW_H);
    let follow = state.timeline.followed_frame != Some((doc_id, cur));
    let mut new_col_w = None;
    let mut actions: Deferred = Vec::new();
    let sources = egui::scroll_area::ScrollSource { drag: egui::scroll_area::DragScroll::Never, ..Default::default() };
    egui::ScrollArea::both().id_salt("timeline_grid").scroll_source(sources).auto_shrink([false, false]).show(
        ui,
        |ui| {
            let avail = ui.available_size();
            let (rect, resp) = ui.allocate_exact_size(content.max(avail), Sense::click_and_drag());
            let vis = ui.clip_rect().intersect(rect.expand(1.0));
            let o = rect.min;
            let col_x = |f: usize| o.x + LEFT_W + f as f32 * col_w;
            let row_y = |r: usize| o.y + top_h + r as f32 * ROW_H;
            let frozen_top = vis.top();
            let frozen_left = vis.left();
            let painter = ui.painter().clone();
            let cells_clip = Rect::from_min_max(Pos2::new(frozen_left + LEFT_W, frozen_top + top_h), vis.max);
            let header_clip = Rect::from_min_max(
                Pos2::new(frozen_left + LEFT_W, frozen_top),
                Pos2::new(vis.right(), frozen_top + top_h),
            );
            let left_clip = Rect::from_min_max(
                Pos2::new(frozen_left, frozen_top + top_h),
                Pos2::new(frozen_left + LEFT_W, vis.bottom()),
            );

            // Keep the current frame in view when it moved.
            if follow {
                // Only sideways: the column spans the visible height, so
                // the vertical scroll position is left alone.
                let col = Rect::from_min_max(Pos2::new(col_x(cur), vis.top()), Pos2::new(col_x(cur + 1), vis.bottom()));
                if col.left() < vis.left() + LEFT_W || col.right() > vis.right() {
                    ui.scroll_to_rect(col.expand2(Vec2::new(LEFT_W + col_w, 0.0)), None);
                }
            }

            // --- hit testing ---------------------------------------------------
            let hit = |p: Pos2| -> TimelineHit {
                let in_left = p.x < frozen_left + LEFT_W;
                let in_top = p.y < frozen_top + top_h;
                let f = ((p.x - o.x - LEFT_W) / col_w).floor();
                let frame = (f >= 0.0 && (f as usize) < n).then_some(f as usize);
                let r = ((p.y - o.y - top_h) / ROW_H).floor();
                let row = (r >= 0.0 && (r as usize) < rows.len()).then_some(r as usize);
                match (in_left, in_top) {
                    (true, true) => TimelineHit::Nothing,
                    (false, true) => {
                        let Some(frame) = frame else { return TimelineHit::Nothing };
                        if p.y < frozen_top + tags_h {
                            let lane = ((p.y - frozen_top) / TAG_LANE_H).floor() as usize;
                            if let Some(i) =
                                tags.iter().enumerate().position(|(i, t)| lanes[i] == lane && t.contains(frame))
                            {
                                return TimelineHit::Tag(i);
                            }
                        }
                        TimelineHit::Frame(frame)
                    }
                    (true, false) => row.map_or(TimelineHit::Nothing, |r| TimelineHit::Layer(rows[r].li)),
                    (false, false) => match (row, frame) {
                        (Some(r), Some(frame)) => TimelineHit::Cel { layer: rows[r].li, frame },
                        _ => TimelineHit::Nothing,
                    },
                }
            };
            let eye_rect =
                |r: usize| Rect::from_min_size(Pos2::new(frozen_left + 8.0, row_y(r) + 4.0), Vec2::splat(12.0));
            let lock_rect =
                |r: usize| Rect::from_min_size(Pos2::new(frozen_left + 24.0, row_y(r) + 4.0), Vec2::splat(12.0));

            // --- input -----------------------------------------------------------
            let pointer = ui.input(|i| i.pointer.interact_pos());
            let mods = ui.input(|i| i.modifiers);
            let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
            let primary_down = ui.input(|i| i.pointer.primary_down());
            let hovered = resp.hovered() || resp.dragged();
            if hovered && mods.command {
                let z = ui.input(|i| i.zoom_delta());
                if (z - 1.0).abs() > 1e-3 {
                    new_col_w = Some((col_w * z).clamp(10.0, 64.0));
                }
            }
            if resp.secondary_clicked() {
                if let Some(p) = pointer {
                    let h = hit(p);
                    state.timeline.menu = Some(h);
                    match h {
                        TimelineHit::Cel { layer, frame } => {
                            let in_sel = sel.is_some_and(|(a, b)| (a..=b).contains(&frame));
                            actions.push(Box::new(move |st| {
                                if let Some(e) = st.doc_mut(doc_id) {
                                    e.doc.state_mut().active = layer;
                                    e.selected.clear();
                                }
                                if !in_sel {
                                    anim::set_frame(st, doc_id, frame, false);
                                }
                            }));
                        }
                        TimelineHit::Frame(frame) => {
                            let in_sel = sel.is_some_and(|(a, b)| (a..=b).contains(&frame));
                            if !in_sel {
                                actions.push(Box::new(move |st| anim::set_frame(st, doc_id, frame, false)));
                            }
                        }
                        TimelineHit::Layer(layer) => actions.push(Box::new(move |st| {
                            if let Some(e) = st.doc_mut(doc_id) {
                                e.doc.state_mut().active = layer;
                                e.selected.clear();
                            }
                        })),
                        _ => {}
                    }
                }
            }
            if primary_pressed && resp.contains_pointer() && state.timeline.drag.is_none() {
                if let Some(p) = pointer {
                    match hit(p) {
                        TimelineHit::Frame(f) => {
                            let in_multi = sel.is_some_and(|(a, b)| a != b && (a..=b).contains(&f));
                            if mods.alt || in_multi {
                                state.timeline.drag = Some(TimelineDrag::MoveFrames { to: usize::MAX });
                                if !in_multi {
                                    actions.push(Box::new(move |st| anim::set_frame(st, doc_id, f, false)));
                                }
                            } else {
                                let anchor = if mods.shift { sel.map_or(cur, |(a, _)| a) } else { f };
                                state.timeline.drag = Some(TimelineDrag::Scrub { anchor });
                                actions.push(Box::new(move |st| scrub_to(st, doc_id, anchor, f)));
                            }
                        }
                        TimelineHit::Tag(i) => actions.push(Box::new(move |st| anim::select_tag(st, doc_id, i))),
                        TimelineHit::Layer(li) => {
                            let r = rows.iter().position(|r| r.li == li).unwrap_or(0);
                            if eye_rect(r).contains(p) {
                                let id = rows[r].id;
                                let vis_now = layer_info[r].visible;
                                state.timeline.drag = Some(TimelineDrag::Eyes(!vis_now));
                                actions.push(Box::new(move |st| {
                                    if let Some(e) = st.doc_mut(doc_id) {
                                        e.doc.set_layer_visible(id, !vis_now);
                                    }
                                }));
                            } else if lock_rect(r).contains(p) {
                                actions.push(Box::new(move |st| {
                                    if let Some(e) = st.doc_mut(doc_id) {
                                        let s = e.doc.state_mut();
                                        s.layers[li].props.locked = !s.layers[li].props.locked;
                                        e.doc.commit("Lock Layer");
                                    }
                                }));
                            } else {
                                actions.push(Box::new(move |st| {
                                    if let Some(e) = st.doc_mut(doc_id) {
                                        e.doc.state_mut().active = li;
                                        e.selected.clear();
                                    }
                                }));
                            }
                        }
                        TimelineHit::Cel { layer, frame } => {
                            let anchor = if mods.shift { sel.map_or(cur, |(a, _)| a) } else { frame };
                            state.timeline.drag = Some(TimelineDrag::Scrub { anchor });
                            actions.push(Box::new(move |st| {
                                if let Some(e) = st.doc_mut(doc_id) {
                                    e.doc.state_mut().active = layer;
                                    e.selected.clear();
                                }
                                scrub_to(st, doc_id, anchor, frame);
                            }));
                        }
                        TimelineHit::Nothing => {
                            actions.push(Box::new(move |st| {
                                if let Some(e) = st.doc_mut(doc_id) {
                                    e.frame_sel = None;
                                }
                            }));
                        }
                    }
                }
            }
            if let (Some(drag), Some(p)) = (state.timeline.drag, pointer) {
                if primary_down {
                    let f = (((p.x - o.x - LEFT_W) / col_w).floor().max(0.0) as usize).min(n - 1);
                    match drag {
                        TimelineDrag::Scrub { anchor } => {
                            if !playing {
                                actions.push(Box::new(move |st| scrub_to(st, doc_id, anchor, f)));
                            }
                        }
                        TimelineDrag::MoveFrames { .. } => {
                            // Insertion point: before or after the frame under the pointer.
                            let half = p.x - col_x(f) > col_w / 2.0;
                            let to_abs = if half { f + 1 } else { f };
                            state.timeline.drag = Some(TimelineDrag::MoveFrames { to: to_abs });
                        }
                        TimelineDrag::Eyes(v) => {
                            if let TimelineHit::Layer(li) = hit(p) {
                                let r = rows.iter().position(|r| r.li == li);
                                if let Some(r) = r {
                                    if eye_rect(r).contains(p) && layer_info[r].visible != v {
                                        let id = rows[r].id;
                                        actions.push(Box::new(move |st| {
                                            if let Some(e) = st.doc_mut(doc_id) {
                                                e.doc.set_layer_visible(id, v);
                                            }
                                        }));
                                    }
                                }
                            }
                        }
                    }
                } else {
                    if let TimelineDrag::MoveFrames { to } = drag {
                        if to != usize::MAX {
                            if let Some((a, b)) = sel.or(Some((cur, cur))) {
                                let which: Vec<usize> = (a..=b).collect();
                                // `to` counts frames including the moving ones;
                                // convert to an index in the list without them.
                                let before = which.iter().filter(|&&f| f < to).count();
                                let to_rel = to - before;
                                if to_rel != a || which.len() == n {
                                    actions.push(Box::new(move |st| anim::move_frames(st, doc_id, &which, to_rel)));
                                }
                            }
                        }
                    }
                    state.timeline.drag = None;
                }
            }
            if resp.double_clicked() {
                if let Some(p) = pointer {
                    match hit(p) {
                        TimelineHit::Frame(_) => actions.push(Box::new(move |st| anim::frame_properties(st, doc_id))),
                        TimelineHit::Tag(i) => {
                            actions.push(Box::new(move |st| anim::tag_properties(st, doc_id, Some(i))))
                        }
                        TimelineHit::Cel { .. } => actions.push(Box::new(move |st| anim::cel_properties(st, doc_id))),
                        TimelineHit::Layer(_) => actions.push(Box::new(crate::dialogs::open_layer_props)),
                        TimelineHit::Nothing => {}
                    }
                }
            }

            // --- painting ----------------------------------------------------------
            // Cells.
            let cp = painter.with_clip_rect(cells_clip.intersect(vis));
            for (r, row) in rows.iter().enumerate() {
                let y = row_y(r);
                let row_rect = Rect::from_min_size(Pos2::new(col_x(0), y), Vec2::new(n as f32 * col_w, ROW_H));
                if r % 2 == 1 {
                    cp.rect_filled(row_rect, 0.0, zebra);
                }
                if row.li == active {
                    cp.rect_filled(row_rect, 0.0, accent.gamma_multiply(0.12));
                }
            }
            let rows_rect = Rect::from_min_max(Pos2::new(col_x(0), row_y(0)), Pos2::new(col_x(n), row_y(rows.len())));
            if let Some((a, b)) = sel {
                let r = Rect::from_min_max(
                    Pos2::new(col_x(a), rows_rect.top()),
                    Pos2::new(col_x(b + 1), rows_rect.bottom()),
                );
                cp.rect_filled(r, 0.0, accent.gamma_multiply(0.10));
            }
            let cur_rect = Rect::from_min_max(
                Pos2::new(col_x(cur), rows_rect.top()),
                Pos2::new(col_x(cur + 1), rows_rect.bottom()),
            );
            cp.rect_filled(cur_rect, 0.0, accent.gamma_multiply(0.22));
            for f in 0..=n {
                let x = col_x(f);
                cp.line_segment(
                    [Pos2::new(x, rows_rect.top()), Pos2::new(x, rows_rect.bottom())],
                    Stroke::new(1.0, dim.gamma_multiply(0.25)),
                );
            }
            for (r, row) in rows.iter().enumerate() {
                let y = row_y(r);
                let cy = y + ROW_H / 2.0;
                let info_l = &layer_info[r];
                if info_l.is_group {
                    continue;
                }
                let alpha = if info_l.visible { 1.0 } else { 0.45 };
                if !info_l.animated {
                    // Static layer: the same picture in every frame.
                    cp.line_segment(
                        [Pos2::new(col_x(0) + 4.0, cy), Pos2::new(col_x(n) - 4.0, cy)],
                        Stroke::new(1.0, dim.gamma_multiply(0.6 * alpha)),
                    );
                    continue;
                }
                let info = &cel_info[r];
                for f in 0..n {
                    let cx = col_x(f) + col_w / 2.0;
                    let owner = info[f].owner;
                    if f > 0 && info[f - 1].owner == owner {
                        cp.line_segment(
                            [Pos2::new(cx - col_w, cy), Pos2::new(cx, cy)],
                            Stroke::new(2.0, text.gamma_multiply(0.55 * alpha)),
                        );
                    }
                    let filled = info[owner.min(n - 1)].filled;
                    let color = text.gamma_multiply(alpha);
                    if owner != f {
                        cp.circle_filled(Pos2::new(cx, cy), 2.0, color.gamma_multiply(0.8));
                    } else if filled {
                        cp.circle_filled(Pos2::new(cx, cy), 3.5, color);
                    } else {
                        cp.circle_stroke(Pos2::new(cx, cy), 3.0, Stroke::new(1.0, dim.gamma_multiply(alpha)));
                    }
                }
                if row.li == active {
                    let cell = Rect::from_min_size(Pos2::new(col_x(cur), y), Vec2::new(col_w, ROW_H));
                    cp.rect_stroke(cell.shrink(1.0), 2.0, Stroke::new(1.5, accent), egui::StrokeKind::Inside);
                }
            }
            // Move-frames insertion marker.
            if let Some(TimelineDrag::MoveFrames { to }) = state.timeline.drag {
                if to != usize::MAX {
                    let x = col_x(to.min(n));
                    painter.with_clip_rect(vis).line_segment(
                        [Pos2::new(x, frozen_top + tags_h), Pos2::new(x, rows_rect.bottom().min(vis.bottom()))],
                        Stroke::new(3.0, accent),
                    );
                }
            }

            // Header (frozen at the top).
            let hp = painter.with_clip_rect(header_clip.intersect(vis));
            let header_rect =
                Rect::from_min_max(Pos2::new(col_x(0), frozen_top), Pos2::new(col_x(n) + col_w, frozen_top + top_h));
            hp.rect_filled(header_rect, 0.0, bg);
            let numbers =
                Rect::from_min_max(Pos2::new(col_x(0), frozen_top + tags_h), Pos2::new(col_x(n), frozen_top + top_h));
            if let Some((a, b)) = sel {
                let r =
                    Rect::from_min_max(Pos2::new(col_x(a), numbers.top()), Pos2::new(col_x(b + 1), numbers.bottom()));
                hp.rect_filled(r, 0.0, accent.gamma_multiply(0.35));
            }
            hp.rect_filled(
                Rect::from_min_max(Pos2::new(col_x(cur), numbers.top()), Pos2::new(col_x(cur + 1), numbers.bottom())),
                0.0,
                accent.gamma_multiply(0.6),
            );
            let every = if col_w >= 16.0 {
                1
            } else if col_w >= 12.0 {
                2
            } else {
                5
            };
            for f in 0..n {
                let x = col_x(f);
                hp.line_segment(
                    [Pos2::new(x, numbers.top()), Pos2::new(x, numbers.bottom())],
                    Stroke::new(1.0, dim.gamma_multiply(0.3)),
                );
                if f == cur || (f + 1) % every == 0 || f == 0 {
                    let c = if f == cur { text } else { dim };
                    hp.text(
                        Pos2::new(x + col_w / 2.0, numbers.center().y),
                        egui::Align2::CENTER_CENTER,
                        (f + 1).to_string(),
                        egui::FontId::proportional(if col_w < 16.0 { 9.0 } else { 11.0 }),
                        c,
                    );
                }
            }
            hp.line_segment(
                [Pos2::new(col_x(0), numbers.bottom()), Pos2::new(header_rect.right(), numbers.bottom())],
                Stroke::new(1.0, dim.gamma_multiply(0.5)),
            );
            // Tags.
            for (i, t) in tags.iter().enumerate() {
                let lane = lanes[i];
                let y0 = frozen_top + lane as f32 * TAG_LANE_H;
                let bar = Rect::from_min_max(
                    Pos2::new(col_x(t.from) + 1.0, y0 + 1.5),
                    Pos2::new(col_x(t.to + 1) - 1.0, y0 + TAG_LANE_H - 1.0),
                );
                let c = rgba_to_color32(t.color);
                let inside = t.contains(cur);
                hp.rect_filled(bar, 3.0, c.gamma_multiply(if inside { 1.0 } else { 0.7 }));
                let tp = hp.with_clip_rect(bar.intersect(vis));
                tp.text(
                    Pos2::new(bar.left() + 4.0, bar.center().y),
                    egui::Align2::LEFT_CENTER,
                    &t.name,
                    egui::FontId::proportional(10.0),
                    text_on(c),
                );
            }

            // Left column (frozen at the left).
            let lp = painter.with_clip_rect(left_clip.intersect(vis));
            let left_rect = Rect::from_min_max(
                Pos2::new(frozen_left, row_y(0)),
                Pos2::new(frozen_left + LEFT_W, row_y(rows.len())),
            );
            lp.rect_filled(left_rect, 0.0, bg);
            for (r, row) in rows.iter().enumerate() {
                let y = row_y(r);
                let row_rect = Rect::from_min_size(Pos2::new(frozen_left, y), Vec2::new(LEFT_W, ROW_H));
                if r % 2 == 1 {
                    lp.rect_filled(row_rect, 0.0, zebra);
                }
                if row.li == active {
                    lp.rect_filled(row_rect, 0.0, accent.gamma_multiply(0.18));
                }
                let LayerInfo { name, visible, locked, is_group, continuous, color, editable, .. } = &layer_info[r];
                if let Some(c) = color {
                    lp.rect_filled(
                        Rect::from_min_size(Pos2::new(frozen_left, y), Vec2::new(4.0, ROW_H)),
                        0.0,
                        Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]),
                    );
                }
                // Eye.
                let er = eye_rect(r);
                let frame = dim.gamma_multiply(0.7);
                crate::ui::chrome::stroke_box(&lp, er, 2.0, Stroke::new(1.0, frame), egui::StrokeKind::Inside);
                if *visible {
                    lp.text(
                        er.center(),
                        egui::Align2::CENTER_CENTER,
                        icons::EYE,
                        egui::FontId::new(9.0, ICON_FAMILY()),
                        text.gamma_multiply(0.8),
                    );
                }
                if *locked {
                    let lr = lock_rect(r);
                    lp.text(
                        lr.center(),
                        egui::Align2::CENTER_CENTER,
                        icons::LOCK_SIMPLE,
                        egui::FontId::new(10.0, ICON_FAMILY()),
                        text.gamma_multiply(0.8),
                    );
                }
                let x = frozen_left + 42.0 + row.depth as f32 * 12.0;
                let name_color = if *visible && *editable { text } else { dim };
                let label = if *is_group { format!("{} {}", icons::FOLDER_SIMPLE, name) } else { name.clone() };
                let np = lp.with_clip_rect(
                    Rect::from_min_max(Pos2::new(x, y), Pos2::new(frozen_left + LEFT_W - 18.0, y + ROW_H))
                        .intersect(vis),
                );
                np.text(
                    Pos2::new(x, y + ROW_H / 2.0),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::FontId::proportional(12.0),
                    name_color,
                );
                if *continuous {
                    lp.text(
                        Pos2::new(frozen_left + LEFT_W - 10.0, y + ROW_H / 2.0),
                        egui::Align2::CENTER_CENTER,
                        icons::LINK_SIMPLE,
                        egui::FontId::new(10.0, ICON_FAMILY()),
                        dim,
                    );
                }
            }
            lp.line_segment(
                [
                    Pos2::new(frozen_left + LEFT_W - 0.5, left_rect.top()),
                    Pos2::new(frozen_left + LEFT_W - 0.5, left_rect.bottom()),
                ],
                Stroke::new(1.0, dim.gamma_multiply(0.5)),
            );
            // Corner.
            let corner = Rect::from_min_size(Pos2::new(frozen_left, frozen_top), Vec2::new(LEFT_W, top_h));
            let cornp = painter.with_clip_rect(corner.intersect(vis));
            cornp.rect_filled(corner, 0.0, bg);
            cornp.text(
                Pos2::new(frozen_left + 8.0, frozen_top + top_h - HEADER_H / 2.0),
                egui::Align2::LEFT_CENTER,
                format!("{} layers · {} frames", rows.len(), n),
                egui::FontId::proportional(10.0),
                dim,
            );
            cornp.line_segment(
                [Pos2::new(corner.left(), corner.bottom() - 0.5), Pos2::new(corner.right(), corner.bottom() - 0.5)],
                Stroke::new(1.0, dim.gamma_multiply(0.5)),
            );

            // Context menu for whatever was right-clicked.
            let menu_hit = state.timeline.menu;
            resp.context_menu(|ui| context_menu(ui, state, doc_id, menu_hit));
        },
    );
    for a in actions {
        a(state);
    }
    if let Some(w) = new_col_w {
        state.settings.anim.timeline_col_w = w;
    }
    state.timeline.followed_frame = Some((doc_id, state.doc(doc_id).map_or(0, |e| e.doc.frame())));
}

fn menu_item(ui: &mut Ui, state: &mut AppState, a: Action, enabled: bool) {
    let btn = egui::Button::new(a.label()).shortcut_text(state.keymap.primary_text(a));
    if ui.add_enabled(enabled, btn).clicked() {
        state.pending.push(a);
        ui.close();
    }
}

/// Scrub: show `f` and select `anchor..=f`.
fn scrub_to(state: &mut AppState, doc: DocId, anchor: usize, f: usize) {
    let same = state.doc(doc).is_some_and(|e| e.doc.frame() == f && e.frame_sel == Some((anchor, f)));
    if same {
        return;
    }
    anim::set_frame(state, doc, f, false);
    if let Some(e) = state.doc_mut(doc) {
        e.frame_sel = if anchor == f { None } else { Some((anchor, f)) };
    }
}

fn context_menu(ui: &mut Ui, state: &mut AppState, doc_id: DocId, hit: Option<TimelineHit>) {
    let item = menu_item;
    match hit {
        Some(TimelineHit::Tag(i)) => {
            if ui.button("Tag Properties…").clicked() {
                anim::tag_properties(state, doc_id, Some(i));
                ui.close();
            }
            if ui.button("Select Frames").clicked() {
                anim::select_tag(state, doc_id, i);
                ui.close();
            }
            if ui.button("Delete Tag").clicked() {
                anim::delete_tag(state, doc_id, Some(i));
                ui.close();
            }
        }
        Some(TimelineHit::Layer(_)) => {
            item(ui, state, Action::LayerProperties, true);
            item(ui, state, Action::ToggleContinuous, true);
            ui.separator();
            item(ui, state, Action::NewLayer, true);
            item(ui, state, Action::DuplicateLayer, true);
            item(ui, state, Action::DeleteLayer, true);
            item(ui, state, Action::ToggleLayerLock, true);
        }
        Some(TimelineHit::Cel { .. }) => {
            item(ui, state, Action::ClearCel, true);
            item(ui, state, Action::LinkCels, true);
            item(ui, state, Action::UnlinkCel, true);
            item(ui, state, Action::CopyCel, true);
            item(ui, state, Action::PasteCel, true);
            item(ui, state, Action::CelProperties, true);
            ui.separator();
            item(ui, state, Action::NewFrame, true);
            item(ui, state, Action::NewEmptyFrame, true);
            item(ui, state, Action::DeleteFrames, true);
            item(ui, state, Action::FrameProperties, true);
            ui.separator();
            item(ui, state, Action::NewTag, true);
        }
        _ => {
            item(ui, state, Action::NewFrame, true);
            item(ui, state, Action::NewEmptyFrame, true);
            item(ui, state, Action::DuplicateFrames, true);
            item(ui, state, Action::DeleteFrames, true);
            item(ui, state, Action::FrameProperties, true);
            item(ui, state, Action::ReverseFrames, true);
            ui.separator();
            item(ui, state, Action::NewTag, true);
            let in_tag = state.doc(doc_id).is_some_and(|e| {
                let s = e.doc.state();
                s.tag_at(s.frame).is_some()
            });
            item(ui, state, Action::TagProperties, in_tag);
            item(ui, state, Action::DeleteTag, in_tag);
        }
    }
}
