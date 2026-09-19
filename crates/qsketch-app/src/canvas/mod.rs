//! The document canvas widget: input routing to tools, GPU rendering of the
//! composite, and screen-space overlays (selection ants, brush cursor).

pub mod flash;
pub mod render;
pub mod view;

use egui::{Color32, Pos2, Sense, Stroke, Ui};
use qsketch_core::{Pt, TILE, TILE_BYTES};

use crate::settings::{BrushCursor, WheelBehavior};
use crate::state::{AppState, BrushPopup, DocId, TempReason};
use crate::tools::{self, CanvasEvent, CanvasInput, ToolKind, ToolSession};
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
            // A pen-tip press with a barrel button held already arrives as a
            // secondary press: `QSketchApp::raw_input_hook` remaps it before egui
            // sees the frame's input.
            egui::Event::PointerButton { pos, button, pressed, modifiers } => {
                last_pos = Some(*pos);
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
                                tools::fill::begin_pick(state, doc_id, inp, t);
                                continue;
                            }
                        }
                        // Quick move: the chord drags the layer / selection as
                        // the Move tool whatever tool is active (a chord
                        // without modifiers arms the Move tool only now).
                        if mouse.is_quick_move(*modifiers, *button)
                            && !tools::floating::selection_hit(state, doc_id, *pos)
                            && !matches!(
                                state.tool,
                                ToolKind::Move | ToolKind::Hand | ToolKind::Zoom | ToolKind::RotateView
                            )
                            && state.floating.is_none()
                        {
                            if state.temp_tool.is_none() {
                                state.temp_tool = Some((ToolKind::Move, TempReason::QuickMove));
                            }
                            if matches!(state.temp_tool, Some((ToolKind::Move, _))) {
                                let inp = make_input(state, *pos, egui::PointerButton::Primary, *modifiers);
                                tools::handle(state, doc_id, CanvasEvent::Press(inp));
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
                    // Mouse motion drives a stroke only when no tablet sample
                    // did this frame, and never while a pen in proximity has
                    // lifted: Windows keeps sending mouse moves for the pen
                    // after the tip is up, and with the pen's pressure gone
                    // they would be painted at the mouse pressure (a full-size
                    // dot at the end of a tapered stroke). Only a paint stroke
                    // is gated: quick rotate and the other hover-driven
                    // gestures are meant to work with the pen up.
                    let pen_lifted = state.pen.tablet_active
                        && state.pen.pressure.is_none()
                        && matches!(state.session, Some(ToolSession::Stroke { .. }));
                    if !use_tablet && !pen_lifted {
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
    // Tablet-sourced motion drives whatever is capturing the pointer: a tool
    // session, a floating paste / Free Transform drag, or a text-box drag.
    // (WinTab drivers deliver packets for the mouse too, so this path is the
    // only one that moves a transform handle while a tablet is in proximity.)
    // Modifiers are read live, not only on motion: Shift / Alt pressed or
    // released while the pointer is held still must still change what a
    // selection or shape drag does, whether they were down at the press or
    // let go and pressed again halfway through.
    if state.session_doc == Some(doc_id) {
        match &mut state.session {
            Some(ToolSession::DragRect { mods: m, .. })
            | Some(ToolSession::Lasso { mods: m, .. })
            | Some(ToolSession::PolyLasso { mods: m, .. })
            | Some(ToolSession::Shape { mods: m, .. }) => *m = mods,
            _ => {}
        }
    }
    let tablet_capturing = (state.session.is_some() && state.session_doc == Some(doc_id))
        || state.floating.as_ref().is_some_and(|f| f.doc == doc_id && f.drag.is_some())
        || tools::text::dragging(state, doc_id);
    if use_tablet && tablet_capturing {
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
    let quick_rotating = matches!(state.temp_tool, Some((_, TempReason::QuickRotate)))
        && matches!(state.session, Some(ToolSession::RotateDrag { .. }));
    if state.session.is_some() && state.session_doc == Some(doc_id) && !quick_rotating {
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

    // Pan inertia: glide on after a hand-tool flick until it decays or a press.
    if !capturing {
        let dt = ctx.input(|i| i.stable_dt);
        if let Some(entry) = state.doc_mut(doc_id) {
            let (w, h) = (entry.doc.width(), entry.doc.height());
            if entry.view.tick_inertia(dt, w, h) {
                ctx.request_repaint();
            }
        }
    }

    // Hover info for the Info panel + cursor.
    let hover_pos = ui.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p) && hovered);
    // Pointer sits where a drag would rotate the transform / selection box.
    let rotate_band = hovered
        && state.brush_popup.is_none()
        && state.pick_preview.is_none()
        && !matches!(tool, ToolKind::Hand | ToolKind::Zoom | ToolKind::RotateView)
        && !(state.temp_tool.is_some_and(|(t, _)| t == ToolKind::Hand) && capturing)
        && hover_pos.is_some_and(|p| tools::floating::in_rotation_band(state, doc_id, p));
    if hovered {
        state.hover_screen_pos = hover_pos;
        state.hover_doc_pos = hover_pos.map(|p| state.doc(doc_id).unwrap().view.screen_to_doc(p));
        state.hover_color = state
            .hover_doc_pos
            .and_then(|p| tools::sample_color_public(state, doc_id, p.x.floor() as i32, p.y.floor() as i32, true));
        let cursor = if state.pick_preview.is_some() {
            // The loupe draws its own precision crosshair; a system cursor on
            // top of it would read as two pointers.
            egui::CursorIcon::None
        } else if state.temp_tool.is_some_and(|(t, _)| t == ToolKind::Hand) && capturing {
            egui::CursorIcon::Grabbing
        } else if rotate_band {
            // Drawn as a corner-rotate glyph at the pointer (see
            // `draw_rotate_cursor`): a drag here rotates, not deselects.
            egui::CursorIcon::None
        } else if let Some(c) = tools::floating::cursor(state, doc_id, hover_pos)
            .filter(|_| !matches!(tool, ToolKind::Hand | ToolKind::Zoom | ToolKind::RotateView))
        {
            c
        } else if let Some(c) = tools::floating::selection_cursor(state, doc_id, hover_pos)
            .filter(|_| tool == ToolKind::Move || tool.is_selection())
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
    let tile_grid = settings.show_grid.then_some(settings.grid_size.max(1));
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
    // Tile grid (View ▸ Grid): a line every N document pixels, drawn through
    // the view so it follows rotation; skipped once the cells would be too
    // small to read as cells.
    if let Some(n) = tile_grid {
        if n as f32 * view.zoom >= 4.0 {
            let stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 170, 255, 120));
            let (w, h) = (dw as f32, dh as f32);
            let mut x = n;
            while x < dw {
                let xf = x as f32;
                painter
                    .line_segment([view.doc_to_screen(Pt::new(xf, 0.0)), view.doc_to_screen(Pt::new(xf, h))], stroke);
                x += n;
            }
            let mut y = n;
            while y < dh {
                let yf = y as f32;
                painter
                    .line_segment([view.doc_to_screen(Pt::new(0.0, yf)), view.doc_to_screen(Pt::new(w, yf))], stroke);
                y += n;
            }
        }
    }

    draw_selection(&painter, entry, &ctx);
    flash::update(state, doc_id, &ctx, &painter);
    tools::draw_overlay(state, doc_id, &painter);
    {
        // The quick brush popup takes the pointer off the canvas, which would
        // normally hide the cursor. Keep drawing it, tracking the pointer
        // wherever it is, so size / hardness / roundness edits preview live.
        let (cursor_pos, cursor_tool) = if state.pick_preview.is_some() {
            // The loupe stands in for the cursor while picking.
            (None, tool)
        } else {
            match state.brush_popup {
                Some(p) => (ui.input(|i| i.pointer.latest_pos()).or(Some(p.pos)), p.tool),
                None => (hover_pos, tool),
            }
        };
        // With the popup open the cursor must stay visible over it, so it goes
        // on a layer above the popup rather than the canvas layer.
        let cursor_painter = match state.brush_popup {
            Some(_) => ctx
                .layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new(("brush_preview", doc_id))))
                .with_clip_rect(rect),
            None => painter.clone(),
        };
        draw_brush_cursor(&cursor_painter, state, doc_id, cursor_pos, cursor_tool);
        if state.brush_popup.is_none() && mods.shift {
            draw_shift_line_preview(&painter, state, doc_id, hover_pos, tool);
        }
        if tool == ToolKind::RotateView && state.brush_popup.is_none() {
            draw_rotate_cursor(&painter, hover_pos, crate::ui::icons::ARROWS_CLOCKWISE, 22.0);
        } else if rotate_band {
            draw_rotate_cursor(&painter, hover_pos, crate::ui::icons::ARROW_ARC_RIGHT, 20.0);
        }
    }
    brush_popup(ui, state);
    tools::text::editor_ui(ui, state, doc_id);
    tools::floating::panel_ui(ui, state, doc_id);
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
                        if crate::ui::widgets::small_button(ui, crate::ui::icons::X).clicked() {
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
                            if crate::ui::widgets::small_button(ui, name).clicked() {
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
/// A glyph standing in for the pointer: the view-rotate arrows for the
/// Rotate View tool, a curved corner arrow inside a transform's rotation band.
fn draw_rotate_cursor(painter: &egui::Painter, hover: Option<Pos2>, glyph: &str, size: f32) {
    let Some(pos) = hover else { return };
    let font = egui::FontId::new(size, crate::ui::iconset::family());
    for d in [egui::vec2(1.0, 1.0), egui::vec2(-1.0, 1.0), egui::vec2(1.0, -1.0), egui::vec2(-1.0, -1.0)] {
        painter.text(pos + d, egui::Align2::CENTER_CENTER, glyph, font.clone(), Color32::from_black_alpha(160));
    }
    painter.text(pos, egui::Align2::CENTER_CENTER, glyph, font, Color32::WHITE);
}

/// Widest pencil / eraser that still previews pixel by pixel. Past this the
/// outline is indistinguishable from the round cursor and walking the dab
/// every frame stops being free.
const PIXEL_PREVIEW_MAX_SIZE: f32 = 256.0;

/// Outline the exact pixels a press would mark, along the pixel grid. The
/// pencil and its eraser work pixel by pixel, so the cursor shows the pixels
/// themselves rather than a circle that only approximates them.
fn draw_pixel_preview(
    painter: &egui::Painter,
    view: &view::CanvasView,
    spans: &qsketch_core::shape::Spans,
    fill: Option<Color32>,
) -> bool {
    let segs = qsketch_core::shape::spans_outline(spans);
    if segs.is_empty() {
        return false;
    }
    // Paint the pixels in the color they are about to get, so the change is
    // visible before it is made. One quad per row; the view may be rotated.
    if let Some(color) = fill {
        for (i, (a, b)) in spans.rows.iter().enumerate() {
            if b <= a {
                continue;
            }
            let y = (spans.y0 + i as i32) as f32;
            let (l, r) = (*a as f32, *b as f32);
            let quad = [Pt::new(l, y), Pt::new(r, y), Pt::new(r, y + 1.0), Pt::new(l, y + 1.0)];
            let pts: Vec<Pos2> = quad.iter().map(|p| view.doc_to_screen(*p)).collect();
            painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
    }
    for (width, color) in [(2.0, Color32::from_black_alpha(140)), (1.0, Color32::from_white_alpha(220))] {
        for seg in &segs {
            painter.line_segment([view.doc_to_screen(seg[0]), view.doc_to_screen(seg[1])], Stroke::new(width, color));
        }
    }
    true
}

/// Aseprite-style Shift preview: with Shift held, a stroke tool shows the
/// straight line a click would draw from the end of the previous stroke to the
/// pointer, so it can be judged before it is committed. The pencil (and its
/// eraser) previews the exact pixels; a soft or anti-aliased tip gets a hairline
/// along its path.
fn draw_shift_line_preview(
    painter: &egui::Painter,
    state: &AppState,
    doc_id: DocId,
    hover: Option<Pos2>,
    tool: ToolKind,
) {
    let Some(pos) = hover else { return };
    if !matches!(tool, ToolKind::Brush | ToolKind::Pencil | ToolKind::Eraser) || state.session.is_some() {
        return;
    }
    let Some((d, from)) = state.last_stroke_end else { return };
    if d != doc_id {
        return;
    }
    let Some(brush) = state.brush_for_tool(tool) else { return };
    let Some(entry) = state.doc(doc_id) else { return };
    let view = &entry.view;
    let to = view.screen_to_doc(pos);
    if brush.size <= PIXEL_PREVIEW_MAX_SIZE {
        if let Some(spans) = brush.line_spans(from, to, 1.0) {
            let fill = (tool == ToolKind::Pencil).then(|| {
                let c = crate::ui::widgets::rgba_to_color32(state.fg);
                c.gamma_multiply((brush.opacity * brush.flow).clamp(0.0, 1.0))
            });
            if draw_pixel_preview(painter, view, &spans, fill) {
                return;
            }
        }
    }
    let (a, b) = (view.doc_to_screen(from), pos);
    painter.line_segment([a, b], Stroke::new(3.0, Color32::from_black_alpha(120)));
    painter.line_segment([a, b], Stroke::new(1.0, Color32::from_white_alpha(230)));
    let r = (brush.size / 2.0 * view.zoom).max(1.5);
    painter.circle_stroke(a, r, Stroke::new(1.0, Color32::from_white_alpha(160)));
}

fn draw_brush_cursor(painter: &egui::Painter, state: &AppState, doc_id: DocId, hover: Option<Pos2>, tool: ToolKind) {
    let Some(pos) = hover else { return };
    if !tool.uses_brush() {
        return;
    }
    let Some(brush) = state.brush_for_tool(tool) else { return };
    let Some(entry) = state.doc(doc_id) else { return };
    let mode = state.settings.canvas.brush_cursor;
    if mode == BrushCursor::Hidden {
        return;
    }
    let r = brush.size / 2.0 * entry.view.zoom;
    let mut outline = matches!(mode, BrushCursor::Outline | BrushCursor::Both);
    let mut cross = matches!(mode, BrushCursor::Crosshair | BrushCursor::Both) || r < 3.0;
    // A pencil press lands on whole pixels, so show those instead of a ring
    // that runs between them. The eraser does the same: it is a pencil that
    // takes ink away, and it has to line up with the one that put it there.
    if outline && matches!(tool, ToolKind::Pencil | ToolKind::Eraser) && brush.size <= PIXEL_PREVIEW_MAX_SIZE {
        let at = entry.view.screen_to_doc(pos);
        if let Some(spans) = brush.dab_spans(at, 1.0) {
            // The pencil shows its ink; the eraser has nothing to show but
            // the hole, so it keeps to the outline.
            let fill = (tool == ToolKind::Pencil).then(|| {
                let c = crate::ui::widgets::rgba_to_color32(state.fg);
                let a = (brush.opacity * brush.flow).clamp(0.0, 1.0);
                c.gamma_multiply(a)
            });
            if draw_pixel_preview(painter, &entry.view, &spans, fill) {
                outline = false;
                // The outlined pixels are the cursor; a crosshair over a
                // one-pixel box would only hide it.
                cross = matches!(mode, BrushCursor::Crosshair | BrushCursor::Both);
                // The footprint says which pixels are reached, not how hard:
                // a soft tip keeps its hardness ring.
                if brush.is_round() && brush.hardness < 0.999 && r >= 3.0 {
                    let inner = r * brush.hardness.max(0.05);
                    painter.circle_stroke(pos, inner, Stroke::new(1.0, Color32::from_white_alpha(90)));
                }
            }
        }
    }
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
