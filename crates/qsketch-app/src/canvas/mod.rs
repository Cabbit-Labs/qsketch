//! The document canvas widget: input routing to tools, GPU rendering of the
//! composite, and screen-space overlays (selection ants, brush cursor).

pub mod render;
pub mod view;

use egui::{Color32, Pos2, Sense, Stroke, Ui};
use qsketch_core::{Pt, TILE, TILE_BYTES};

use crate::settings::{BrushCursor, WheelBehavior};
use crate::state::{AppState, BrushPopup, DocId, TempReason};
use crate::tools::{self, CanvasEvent, CanvasInput, ToolKind};
use render::{CanvasCallback, TileUpload, Uniforms};

pub fn show(ui: &mut Ui, state: &mut AppState, doc_id: DocId) {
    let avail = ui.available_size_before_wrap();
    let (rect, response) = ui.allocate_exact_size(avail, Sense::click_and_drag());
    if rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();

    // --- view setup -------------------------------------------------------
    {
        let Some(entry) = state.doc_mut(doc_id) else { return };
        entry.view.viewport = rect;
        if !entry.view.initialized {
            let (w, h) = (entry.doc.width(), entry.doc.height());
            entry.view.fit(w, h);
        }
    }

    let hovered = response.hovered();
    let capturing = state.session.is_some() && state.session_doc == Some(doc_id);
    let mods = ui.input(|i| i.modifiers);
    let events = ui.input(|i| i.raw.events.clone());
    let mouse_pressure = state.settings.tablet.mouse_pressure;

    if response.clicked() || response.drag_started() || ui.input(|i| i.pointer.any_pressed()) && hovered {
        // Painting takes keyboard focus away from text fields so single-key shortcuts work.
        ui.memory_mut(|m| m.stop_text_input());
        state.active_doc = Some(doc_id);
    }

    // --- wheel / pinch ----------------------------------------------------
    if hovered && !capturing {
        let wheel_mode = state.settings.canvas.wheel;
        let invert = state.settings.canvas.invert_wheel_zoom;
        let zoom_to_cursor = state.settings.canvas.zoom_to_cursor;
        let pointer = ui.input(|i| i.pointer.hover_pos());
        if let Some(entry) = state.doc_mut(doc_id) {
            for ev in &events {
                match ev {
                    egui::Event::MouseWheel { unit, delta, modifiers, .. } => {
                        let (dx, dy) = match unit {
                            egui::MouseWheelUnit::Point => (delta.x, delta.y),
                            egui::MouseWheelUnit::Line => (delta.x * 40.0, delta.y * 40.0),
                            egui::MouseWheelUnit::Page => (delta.x * 400.0, delta.y * 400.0),
                        };
                        let want_zoom = match wheel_mode {
                            WheelBehavior::Zoom => !modifiers.command && !modifiers.shift,
                            WheelBehavior::Scroll => modifiers.command || modifiers.alt,
                        };
                        if want_zoom {
                            let mut steps = dy / 40.0;
                            if invert {
                                steps = -steps;
                            }
                            let factor = 1.2f32.powf(steps);
                            let anchor = if zoom_to_cursor { pointer } else { None };
                            entry.view.zoom_by(factor, anchor);
                        } else if modifiers.shift {
                            entry.view.pan_by_screen(egui::vec2(dy + dx, 0.0));
                        } else {
                            entry.view.pan_by_screen(egui::vec2(dx, dy));
                        }
                    }
                    egui::Event::Zoom(f) => {
                        entry.view.zoom_by(*f, pointer);
                    }
                    egui::Event::Rotate(r) => {
                        entry.view.rotate_by(*r, pointer);
                    }
                    _ => {}
                }
            }
            let (w, h) = (entry.doc.width(), entry.doc.height());
            entry.view.clamp_to_document(w, h);
        }
    }

    // --- pointer events -> tool events ------------------------------------
    let tool = state.effective_tool();
    let mut last_pos: Option<Pos2> = None;
    let tablet_samples = std::mem::take(&mut state.tablet_samples);
    let use_tablet = state.pen.tablet_active && !tablet_samples.is_empty();
    let make_input = |state: &AppState, pos: Pos2, button: egui::PointerButton, mods: egui::Modifiers| -> CanvasInput {
        let view = &state.doc(doc_id).unwrap().view;
        let raw = state.pen.pressure.unwrap_or(mouse_pressure);
        CanvasInput { doc: view.screen_to_doc(pos), screen: pos, pressure: state.curve_pressure(raw), mods, button }
    };

    let mouse = state.settings.mouse.clone();
    let popup_rect = state.brush_popup.map(|p| p.rect);

    for ev in &events {
        match ev {
            egui::Event::Touch { phase, pos, force, .. } => {
                use egui::TouchPhase::*;
                if !state.pen.tablet_active {
                    match phase {
                        Start | Move => {
                            state.pen.pressure = *force;
                            state.pen.in_contact = force.is_some();
                        }
                        End | Cancel => {
                            state.pen.pressure = None;
                            state.pen.in_contact = false;
                        }
                    }
                }
                last_pos = Some(*pos);
            }
            egui::Event::PointerButton { pos, button: raw_button, pressed, modifiers } => {
                last_pos = Some(*pos);
                // The windowing layer reports every pen-tip contact as a primary press
                // even when a barrel button mapped to right-click is held. Recover the
                // intended button from the tablet backend and keep the release matched.
                let mut mapped = *raw_button;
                if *raw_button == egui::PointerButton::Primary {
                    if *pressed {
                        if state.pen.barrel_held || crate::win_pointer::barrel_held() {
                            mapped = egui::PointerButton::Secondary;
                        }
                        state.pen.tip_button = Some(mapped);
                    } else if let Some(b) = state.pen.tip_button.take() {
                        mapped = b;
                    }
                }
                let button = &mapped;
                let capturing_now = state.session.is_some() && state.session_doc == Some(doc_id);
                if *pressed {
                    // Clicks on the quick brush popup belong to it, not the canvas.
                    if popup_rect.is_some_and(|r| r.contains(*pos)) {
                        continue;
                    }
                    if state.brush_popup.is_some() {
                        state.brush_popup = None;
                    }
                    if hovered && rect.contains(*pos) && state.session.is_none() {
                        // A pick chord without modifiers (bare right-click, say)
                        // can't arm the temporary eyedropper, so pick here.
                        if state.temp_tool.is_none() && state.tool.uses_color() {
                            if let Some(t) = state.settings.mouse.pick_target(*modifiers, *button) {
                                let inp = make_input(state, *pos, *button, *modifiers);
                                tools::fill::pick_once(state, doc_id, inp, t);
                                continue;
                            }
                        }
                        match *button {
                            // Middle-drag: temporary Hand with any tool.
                            egui::PointerButton::Middle if mouse.middle_drag_pans => {
                                state.temp_tool = Some((ToolKind::Hand, TempReason::Middle));
                                let inp = make_input(state, *pos, *button, *modifiers);
                                tools::handle(state, doc_id, CanvasEvent::Press(inp));
                            }
                            // Right-click: quick brush settings (Alt+right-click keeps picking colors).
                            egui::PointerButton::Secondary
                                if mouse.right_click_brush_popup
                                    && !modifiers.alt
                                    && state.tool.uses_brush()
                                    && state.temp_tool.is_none() =>
                            {
                                state.brush_popup = Some(BrushPopup {
                                    pos: *pos,
                                    tool: state.tool,
                                    rect: egui::Rect::NOTHING,
                                    just_opened: true,
                                });
                            }
                            _ => {
                                let inp = make_input(state, *pos, *button, *modifiers);
                                tools::handle(state, doc_id, CanvasEvent::Press(inp));
                            }
                        }
                    }
                } else if capturing_now {
                    let inp = make_input(state, *pos, *button, *modifiers);
                    tools::handle(state, doc_id, CanvasEvent::Release(inp));
                    if *button == egui::PointerButton::Middle
                        && matches!(state.temp_tool, Some((_, TempReason::Middle)))
                        && state.session.is_none()
                    {
                        state.temp_tool = None;
                    }
                } else if state.floating.is_some() || tools::text::dragging(state, doc_id) {
                    let inp = make_input(state, *pos, *button, *modifiers);
                    tools::handle(state, doc_id, CanvasEvent::Release(inp));
                }
            }
            egui::Event::PointerMoved(pos) => {
                last_pos = Some(*pos);
                let capturing_now = state.session.is_some() && state.session_doc == Some(doc_id);
                let float_drag = state.floating.as_ref().is_some_and(|f| f.doc == doc_id && f.drag.is_some());
                let text_drag = tools::text::dragging(state, doc_id);
                if capturing_now || float_drag || text_drag {
                    if !use_tablet {
                        let inp = make_input(state, *pos, egui::PointerButton::Primary, mods);
                        tools::handle(state, doc_id, CanvasEvent::Drag(inp));
                    }
                } else if hovered && rect.contains(*pos) {
                    let inp = make_input(state, *pos, egui::PointerButton::Primary, mods);
                    tools::handle(state, doc_id, CanvasEvent::Hover(inp));
                }
            }
            _ => {}
        }
    }
    if use_tablet && state.session.is_some() && state.session_doc == Some(doc_id) {
        for (pos, pressure) in &tablet_samples {
            state.pen.pressure = Some(*pressure);
            let inp = make_input(state, *pos, egui::PointerButton::Primary, mods);
            tools::handle(state, doc_id, CanvasEvent::Drag(inp));
        }
    }
    if response.double_clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let inp = make_input(state, pos, egui::PointerButton::Primary, mods);
            tools::handle(state, doc_id, CanvasEvent::DoubleClick(inp));
        }
    }
    // Safety net: if the pointer button is no longer down but a session is
    // still active (e.g. release happened outside the window), finish it.
    if state.session.is_some() && state.session_doc == Some(doc_id) {
        let any_down = ui.input(|i| i.pointer.any_down());
        if !any_down && !state.pen.tablet_active {
            if let Some(pos) = last_pos.or(ui.input(|i| i.pointer.latest_pos())) {
                let inp = make_input(state, pos, egui::PointerButton::Primary, mods);
                tools::handle(state, doc_id, CanvasEvent::Release(inp));
            }
        }
    }

    // A floating paste whose drag ended off-window.
    if state.floating.as_ref().is_some_and(|f| f.doc == doc_id && f.drag.is_some())
        && !ui.input(|i| i.pointer.any_down())
    {
        if let Some(pos) = last_pos.or(ui.input(|i| i.pointer.latest_pos())) {
            let inp = make_input(state, pos, egui::PointerButton::Primary, mods);
            tools::handle(state, doc_id, CanvasEvent::Release(inp));
        }
    }
    tools::floating::refresh(state);
    if tools::text::dragging(state, doc_id) && !ui.input(|i| i.pointer.any_down()) {
        tools::text::handle(
            state,
            doc_id,
            CanvasEvent::Release(make_input(state, rect.min, egui::PointerButton::Primary, mods)),
        );
    }
    tools::text::refresh(state);

    let capturing = state.session.is_some() && state.session_doc == Some(doc_id);
    if capturing || state.floating.as_ref().is_some_and(|f| f.drag.is_some()) || tools::text::dragging(state, doc_id) {
        ctx.request_repaint();
    }

    // Hover info for the Info panel + cursor.
    let hover_pos = ui.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p) && hovered);
    if hovered {
        state.hover_screen_pos = hover_pos;
        state.hover_doc_pos = hover_pos.map(|p| state.doc(doc_id).unwrap().view.screen_to_doc(p));
        state.hover_color = state
            .hover_doc_pos
            .and_then(|p| tools::sample_color_public(state, doc_id, p.x.floor() as i32, p.y.floor() as i32, true));
        let cursor = if state.temp_tool.is_some_and(|(t, _)| t == ToolKind::Hand) && capturing {
            egui::CursorIcon::Grabbing
        } else if let Some(c) = tools::floating::cursor(state, doc_id, hover_pos)
            .filter(|_| !matches!(tool, ToolKind::Hand | ToolKind::Zoom | ToolKind::RotateView))
        {
            c
        } else if tool == ToolKind::Text
            && state.text_edit.as_ref().is_some_and(|te| {
                te.doc == doc_id
                    && (te.drag.is_some()
                        || hover_pos.is_some_and(|p| {
                            let d = state.doc(doc_id).unwrap().view.screen_to_doc(p);
                            te.bounds.is_some_and(|b| b.expand(4).contains(d.x.floor() as i32, d.y.floor() as i32))
                        }))
            })
        {
            egui::CursorIcon::Move
        } else if tool == ToolKind::RotateView && hover_pos.is_some() {
            // Drawn as a glyph at the pointer (see `draw_rotate_cursor`).
            egui::CursorIcon::None
        } else {
            tool.cursor()
        };
        ui.output_mut(|o| o.cursor_icon = cursor);
    }

    // --- composite + GPU upload -------------------------------------------
    let settings = &state.settings.canvas;
    let checker_a = settings.checker_a;
    let checker_b = settings.checker_b;
    let checker_size = settings.checker_size;
    let theme_bg = state.settings.ui.palette().bg;
    let outside = settings.outside_color.unwrap_or([theme_bg.r(), theme_bg.g(), theme_bg.b()]);
    let grid_on = settings.show_pixel_grid;
    let grid_min = settings.pixel_grid_min_zoom;
    let smooth_out = settings.smooth_zoom_out;
    let render_state = state.render_state.clone();
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let dirty = entry.doc.update_composite();
    if !dirty.is_empty() {
        entry.generation += 1;
    }
    let comp = &entry.doc.composite;
    let mut tiles = Vec::new();
    let full = entry.needs_full_upload
        || render::needs_full_upload(render_state.as_ref(), doc_id, entry.doc.width(), entry.doc.height());
    if full {
        for ty in 0..comp.tiles_y() {
            for tx in 0..comp.tiles_x() {
                tiles.push(TileUpload { tx, ty, data: comp.tile(tx, ty).px.to_vec() });
            }
        }
        entry.needs_full_upload = false;
    } else {
        for (tx, ty) in dirty.iter() {
            if tx < comp.tiles_x() && ty < comp.tiles_y() {
                tiles.push(TileUpload { tx, ty, data: comp.tile(tx, ty).px.to_vec() });
            }
        }
    }
    debug_assert!(tiles.iter().all(|t| t.data.len() == TILE_BYTES));
    let view = &entry.view;
    let (dw, dh) = (entry.doc.width(), entry.doc.height());
    let c = |v: [u8; 3]| [v[0] as f32 / 255.0, v[1] as f32 / 255.0, v[2] as f32 / 255.0, 1.0];
    let uniforms = Uniforms {
        viewport_min: [rect.min.x, rect.min.y],
        viewport_size: [rect.width(), rect.height()],
        doc_size: [dw as f32, dh as f32],
        tex_size: [(comp.tiles_x() as usize * TILE) as f32, (comp.tiles_y() as usize * TILE) as f32],
        center: [view.center.x, view.center.y],
        zoom: view.zoom,
        rotation: view.rotation,
        flip: if view.flip_h { 1.0 } else { 0.0 },
        checker_size: checker_size * ppp,
        grid: if grid_on && view.zoom >= grid_min { 1.0 } else { 0.0 },
        ppp,
        checker_a: c(checker_a),
        checker_b: c(checker_b),
        outside: c(outside),
    };
    let callback =
        CanvasCallback { doc_id, width: dw, height: dh, tiles, uniforms, linear: view.zoom < 1.0 && smooth_out, full };
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, callback));

    // --- overlays ---------------------------------------------------------
    let painter = ui.painter().with_clip_rect(rect);
    // Document border
    let border_pts = [
        view.doc_to_screen(Pt::new(0.0, 0.0)),
        view.doc_to_screen(Pt::new(dw as f32, 0.0)),
        view.doc_to_screen(Pt::new(dw as f32, dh as f32)),
        view.doc_to_screen(Pt::new(0.0, dh as f32)),
    ];
    painter.add(egui::Shape::closed_line(border_pts.to_vec(), Stroke::new(1.0, Color32::from_black_alpha(160))));

    draw_selection(&painter, entry, &ctx);
    tools::draw_overlay(state, doc_id, &painter);
    if state.brush_popup.is_none() {
        draw_brush_cursor(&painter, state, doc_id, hover_pos, tool);
        if tool == ToolKind::RotateView {
            draw_rotate_cursor(&painter, hover_pos);
        }
    }
    brush_popup(ui, state);
    tools::text::editor_ui(ui, state, doc_id);
}

/// Quick brush settings popup at the right-click position.
fn brush_popup(ui: &mut Ui, state: &mut AppState) {
    let Some(mut popup) = state.brush_popup else { return };
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.brush_popup = None;
        return;
    }
    let screen = ctx.content_rect();
    let size = egui::vec2(260.0, 0.0);
    let mut pos = popup.pos + egui::vec2(8.0, 8.0);
    pos.x = pos.x.min(screen.right() - size.x - 8.0).max(screen.left());
    let resp = egui::Area::new(egui::Id::new("quick_brush_popup")).order(egui::Order::Foreground).fixed_pos(pos).show(
        &ctx,
        |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(size.x);
                ui.spacing_mut().slider_width = 150.0;
                let tool = popup.tool;
                ui.horizontal(|ui| {
                    ui.label(crate::ui::widgets::icon(tool.icon(), 15.0));
                    ui.label(egui::RichText::new(tool.label()).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(crate::ui::icons::X).clicked() {
                            state.brush_popup = None;
                        }
                    });
                });
                ui.separator();
                let Some(b) = state.brush_for_tool_mut(tool) else { return };
                egui::Grid::new("quick_brush_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label("Size");
                    ui.add(
                        egui::Slider::new(&mut b.size, 1.0..=500.0)
                            .logarithmic(true)
                            .suffix(" px")
                            .fixed_decimals(0)
                            .clamping(egui::SliderClamping::Always),
                    );
                    ui.end_row();
                    if tool != ToolKind::Pencil {
                        ui.label("Hardness");
                        pct(ui, &mut b.hardness);
                        ui.end_row();
                    }
                    ui.label("Opacity");
                    pct(ui, &mut b.opacity);
                    ui.end_row();
                    if tool != ToolKind::Pencil {
                        ui.label("Flow");
                        pct(ui, &mut b.flow);
                        ui.end_row();
                        ui.label("Smoothing");
                        pct(ui, &mut b.smoothing);
                        ui.end_row();
                    }
                });
                b.clamp();
                if !state.presets.is_empty() && tool == ToolKind::Brush {
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        let names: Vec<String> = state.presets.iter().map(|p| p.name.clone()).collect();
                        for (i, name) in names.iter().enumerate() {
                            if ui.small_button(name).clicked() {
                                state.brush = state.presets[i].clone();
                            }
                        }
                    });
                }
            });
        },
    );
    popup.rect = resp.response.rect.expand(4.0);
    popup.just_opened = false;
    // Keep the slot unless something inside closed it.
    if state.brush_popup.is_some() {
        state.brush_popup = Some(popup);
    }
}

fn pct(ui: &mut Ui, value: &mut f32) {
    let mut p = (*value * 100.0).round();
    if ui
        .add(
            egui::Slider::new(&mut p, 0.0..=100.0).suffix("%").fixed_decimals(0).clamping(egui::SliderClamping::Always),
        )
        .changed()
    {
        *value = (p / 100.0).clamp(0.0, 1.0);
    }
}

fn draw_selection(painter: &egui::Painter, entry: &mut crate::state::DocEntry, ctx: &egui::Context) {
    let Some(sel) = entry.doc.state().selection.clone() else {
        entry.sel_outline = None;
        return;
    };
    let key = std::sync::Arc::as_ptr(&sel) as usize;
    if entry.sel_outline.as_ref().is_none_or(|(k, _)| *k != key) {
        entry.sel_outline = Some((key, sel.outline_segments()));
    }
    let Some((_, segs)) = &entry.sel_outline else { return };
    if segs.is_empty() {
        return;
    }
    let view = &entry.view;
    let t = ctx.input(|i| i.time);
    let phase = ((t * 12.0) as i64) % 8;
    let animate = segs.len() < 400_000;
    if animate {
        ctx.request_repaint_after(std::time::Duration::from_millis(90));
    }
    let vp = view.viewport.expand(2.0);
    let black = Stroke::new(1.0, Color32::BLACK);
    let white = Stroke::new(1.0, Color32::WHITE);
    let long = view.zoom >= 6.0;
    for seg in segs {
        let a = view.doc_to_screen(seg[0]);
        let b = view.doc_to_screen(seg[1]);
        if !vp.contains(a) && !vp.contains(b) {
            continue;
        }
        if long {
            painter.line_segment([a, b], black);
            painter.add(egui::Shape::dashed_line_with_offset(&[a, b], white, &[4.0], &[4.0], phase as f32));
        } else {
            let idx = (seg[0].x + seg[0].y) as i64 + phase;
            let s = if (idx / 4) % 2 == 0 { black } else { white };
            painter.line_segment([a, b], s);
        }
    }
}

/// Rotate View has no system cursor; draw the rotate glyph at the pointer.
fn draw_rotate_cursor(painter: &egui::Painter, hover: Option<Pos2>) {
    let Some(pos) = hover else { return };
    let font = egui::FontId::new(22.0, crate::ui::iconset::family());
    let glyph = crate::ui::icons::ARROWS_CLOCKWISE;
    for d in [egui::vec2(1.0, 1.0), egui::vec2(-1.0, 1.0), egui::vec2(1.0, -1.0), egui::vec2(-1.0, -1.0)] {
        painter.text(pos + d, egui::Align2::CENTER_CENTER, glyph, font.clone(), Color32::from_black_alpha(160));
    }
    painter.text(pos, egui::Align2::CENTER_CENTER, glyph, font, Color32::WHITE);
}

fn draw_brush_cursor(painter: &egui::Painter, state: &AppState, doc_id: DocId, hover: Option<Pos2>, tool: ToolKind) {
    let Some(pos) = hover else { return };
    if !tool.uses_brush() {
        return;
    }
    let Some(brush) = state.current_brush() else { return };
    let Some(entry) = state.doc(doc_id) else { return };
    let mode = state.settings.canvas.brush_cursor;
    if mode == BrushCursor::Hidden {
        return;
    }
    let r = brush.size / 2.0 * entry.view.zoom;
    let outline = matches!(mode, BrushCursor::Outline | BrushCursor::Both);
    let cross = matches!(mode, BrushCursor::Crosshair | BrushCursor::Both) || r < 3.0;
    if outline && r >= 3.0 {
        let elliptical = brush.roundness < 0.999;
        if elliptical {
            // Rotated ellipse matching the tip (screen y is down, so negate the angle).
            let ang = -brush.angle.to_radians() + entry.view.rotation;
            let (s, c) = ang.sin_cos();
            let n = 48;
            let pts: Vec<Pos2> = (0..=n)
                .map(|i| {
                    let t = i as f32 / n as f32 * std::f32::consts::TAU;
                    let (x, y) = (r * t.cos(), r * brush.roundness * t.sin());
                    Pos2::new(pos.x + x * c - y * s, pos.y + x * s + y * c)
                })
                .collect();
            painter.add(egui::Shape::line(pts.clone(), Stroke::new(2.0, Color32::from_black_alpha(140))));
            painter.add(egui::Shape::line(pts, Stroke::new(1.0, Color32::from_white_alpha(220))));
        } else {
            painter.circle_stroke(pos, r, Stroke::new(2.0, Color32::from_black_alpha(140)));
            painter.circle_stroke(pos, r, Stroke::new(1.0, Color32::from_white_alpha(220)));
            if brush.is_round() && brush.hardness < 0.999 {
                let inner = r * brush.hardness.max(0.05);
                painter.circle_stroke(pos, inner, Stroke::new(1.0, Color32::from_white_alpha(90)));
            }
        }
    }
    if cross {
        let l = 6.0;
        for (a, b) in [
            (egui::vec2(-l, 0.0), egui::vec2(-2.0, 0.0)),
            (egui::vec2(2.0, 0.0), egui::vec2(l, 0.0)),
            (egui::vec2(0.0, -l), egui::vec2(0.0, -2.0)),
            (egui::vec2(0.0, 2.0), egui::vec2(0.0, l)),
        ] {
            painter.line_segment([pos + a, pos + b], Stroke::new(3.0, Color32::from_black_alpha(140)));
            painter.line_segment([pos + a, pos + b], Stroke::new(1.0, Color32::WHITE));
        }
    }
}
