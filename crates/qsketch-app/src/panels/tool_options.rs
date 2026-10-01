//! The context-sensitive tool options bar under the menu.

use egui::Ui;
use qsketch_core::mask::SelectionOp;
use qsketch_core::ops::GradientKind;

use crate::state::AppState;
use crate::tools::ToolKind;
use crate::ui::icons;
use crate::ui::theme::{Palette, Section};
use crate::ui::widgets::{chip, icon, icon_button, param, param_pct};

/// Fixed height of the options bar (points), so it never resizes with its contents.
pub const BAR_HEIGHT: f32 = 26.0;

/// A color-coded group of controls belonging to `section`.
fn group<R>(ui: &mut Ui, p: &Palette, section: Section, contents: impl FnOnce(&mut Ui) -> R) -> R {
    chip(ui, p.section(section), p.section_tint(section), contents)
}

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let tool = state.tool;
    // The bar can outgrow a narrow window; let it scroll sideways (wheel or
    // drag) instead of clipping the trailing controls.
    egui::ScrollArea::horizontal()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| bar(ui, state, tool));
}

fn bar(ui: &mut Ui, state: &mut AppState, tool: ToolKind) {
    let theme = state.settings.ui.palette();
    ui.horizontal(|ui| {
        ui.set_min_height(BAR_HEIGHT - 4.0);
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.add_space(2.0);
        if state.floating.is_some() {
            floating_options(ui, state);
            return;
        }
        // Tool identity chip (blue): icon + name, and the tool's own knobs.
        group(ui, &theme, Section::Tools, |ui| {
            ui.label(icon(tool.icon(), 15.0));
            ui.label(egui::RichText::new(tool.label()).strong());
        });
        match tool {
            ToolKind::Brush
            | ToolKind::Pencil
            | ToolKind::Eraser
            | ToolKind::Line
            | ToolKind::Rect
            | ToolKind::Ellipse
            | ToolKind::Smudge => brush_options(ui, state, tool),
            ToolKind::Clone => {
                brush_options(ui, state, tool);
                group(ui, &theme, Section::Tools, |ui| {
                    let o = &mut state.tool_opts;
                    ui.checkbox(&mut o.clone_aligned, "Aligned")
                        .on_hover_text("Keep the same offset from source to brush across strokes");
                    ui.checkbox(&mut o.clone_sample_merged, "Sample all layers")
                        .on_hover_text("Copy from the whole picture, not just the active layer");
                    let text = match o.clone_source {
                        Some((_, p)) => format!("Source {}, {}", p.x.round() as i32, p.y.round() as i32),
                        None => "Alt+click to set the source".to_string(),
                    };
                    ui.label(egui::RichText::new(text).weak().small());
                });
            }
            ToolKind::SelectBrush => {
                group(ui, &theme, Section::Tools, |ui| {
                    let erase = state.tool_opts.select_brush_erase;
                    if icon_button(ui, icons::SELECTION_PLUS, "Select: strokes add to the selection", 22.0, !erase)
                        .clicked()
                    {
                        state.tool_opts.select_brush_erase = false;
                    }
                    if icon_button(
                        ui,
                        icons::SELECTION_SLASH,
                        "Deselect: strokes take away from the selection (or hold Alt)",
                        22.0,
                        erase,
                    )
                    .clicked()
                    {
                        state.tool_opts.select_brush_erase = true;
                    }
                });
                brush_options(ui, state, tool);
            }
            ToolKind::Contour => {
                group(ui, &theme, Section::Tools, |ui| {
                    ui.checkbox(&mut state.tool_opts.contour_outline, "Outline with brush")
                        .on_hover_text("Stroke the closed edge with the current brush after filling");
                });
                symmetry_group(ui, state);
                hint(ui, "Drag a freehand shape; it closes and fills with the foreground color.");
            }
            ToolKind::Text => text_options(ui, state),
            ToolKind::RectSelect | ToolKind::EllipseSelect | ToolKind::Lasso | ToolKind::PolyLasso => {
                group(ui, &theme, Section::Tools, |ui| selection_ops(ui, state));
                group(ui, &theme, Section::Tools, |ui| {
                    param(ui, "Feather", &mut state.tool_opts.selection_feather, 0.0..=250.0, " px", false, 1)
                        .on_hover_text("Soften the edge of new selections by this many pixels (0 = hard edge)");
                });
            }
            ToolKind::MagicWand => {
                group(ui, &theme, Section::Tools, |ui| selection_ops(ui, state));
                group(ui, &theme, Section::Tools, |ui| {
                    let mut tol = state.tool_opts.wand_tolerance as f32;
                    if param(ui, "Tolerance", &mut tol, 0.0..=255.0, "", false, 0).changed() {
                        state.tool_opts.wand_tolerance = tol as u8;
                    }
                    ui.checkbox(&mut state.tool_opts.wand_contiguous, "Contiguous");
                    ui.checkbox(&mut state.tool_opts.wand_sample_merged, "All layers")
                        .on_hover_text("Sample all layers");
                });
            }
            ToolKind::Fill => {
                group(ui, &theme, Section::Tools, |ui| {
                    let mut tol = state.tool_opts.fill_tolerance as f32;
                    if param(ui, "Tolerance", &mut tol, 0.0..=255.0, "", false, 0).changed() {
                        state.tool_opts.fill_tolerance = tol as u8;
                    }
                    ui.checkbox(&mut state.tool_opts.fill_contiguous, "Contiguous");
                    ui.checkbox(&mut state.tool_opts.fill_sample_merged, "All layers")
                        .on_hover_text("Sample all layers");
                });
                hint(ui, "Alt+click fills with the background color");
            }
            ToolKind::Gradient => {
                group(ui, &theme, Section::Tools, |ui| {
                    ui.selectable_value(&mut state.tool_opts.gradient_kind, GradientKind::Linear, "Linear");
                    ui.selectable_value(&mut state.tool_opts.gradient_kind, GradientKind::Radial, "Radial");
                });
                hint(ui, "Drag to draw a foreground → background gradient. Shift snaps the angle.");
            }
            ToolKind::Shape => {
                let doc_id = state.active_doc;
                let shape = state.active().and_then(|d| {
                    let l = d.doc.state().active_layer();
                    l.props.shape.clone().filter(|_| l.is_shape())
                });
                match (doc_id, shape) {
                    (Some(doc_id), Some(sh)) => {
                        group(ui, &theme, Section::Tools, |ui| {
                            let mut fill = sh.fill.is_some();
                            if ui.checkbox(&mut fill, "Fill").changed() {
                                let fg = state.fg;
                                crate::tools::vector::edit_shape(state, doc_id, "Shape Fill", |s| {
                                    s.fill = fill.then_some(fg);
                                });
                            }
                            if let Some(c) = sh.fill {
                                let mut c32 = egui::Color32::from_rgb(c.r, c.g, c.b);
                                if egui::color_picker::color_edit_button_srgba(ui, &mut c32, egui::color_picker::Alpha::Opaque).changed() {
                                    crate::tools::vector::edit_shape(state, doc_id, "Shape Fill", |s| {
                                        s.fill = Some(qsketch_core::Rgba8::new(c32.r(), c32.g(), c32.b(), 255));
                                    });
                                }
                            }
                            let mut stroke = sh.stroke.is_some();
                            if ui.checkbox(&mut stroke, "Outline").changed() {
                                let bg = state.bg;
                                crate::tools::vector::edit_shape(state, doc_id, "Shape Outline", |s| {
                                    s.stroke = stroke.then_some(bg);
                                });
                            }
                            if let Some(c) = sh.stroke {
                                let mut c32 = egui::Color32::from_rgb(c.r, c.g, c.b);
                                if egui::color_picker::color_edit_button_srgba(ui, &mut c32, egui::color_picker::Alpha::Opaque).changed() {
                                    crate::tools::vector::edit_shape(state, doc_id, "Shape Outline", |s| {
                                        s.stroke = Some(qsketch_core::Rgba8::new(c32.r(), c32.g(), c32.b(), 255));
                                    });
                                }
                                let mut w = sh.stroke_width.max(1);
                                let r = ui.add(egui::DragValue::new(&mut w).range(1..=64).suffix(" px"));
                                if r.changed() && w != sh.stroke_width {
                                    crate::tools::vector::edit_shape(state, doc_id, "Outline Width", |s| s.stroke_width = w);
                                }
                            }
                            let mut closed = sh.closed;
                            if ui.checkbox(&mut closed, "Closed").on_hover_text("Join the last point back to the first (and fill)").changed() {
                                crate::tools::vector::edit_shape(state, doc_id, "Shape Closed", |s| s.closed = closed);
                            }
                            ui.label(egui::RichText::new(format!("{} points", sh.points.len())).weak());
                            if ui.button("Rasterize").on_hover_text("Turn the shape into plain pixels (Layer › Rasterize Shape Layer)").clicked() {
                                state.pending.push(crate::actions::Action::RasterizeShape);
                            }
                        });
                        hint(ui, "Click to add a point (on an edge: splits it; Shift: after the selected point). Drag a point to move it, Alt+click or Delete removes it.");
                    }
                    _ => {
                        group(ui, &theme, Section::Tools, |ui| {
                            let o = &mut state.tool_opts;
                            ui.checkbox(&mut o.shape_fill, "Fill").on_hover_text("Fill with the foreground color");
                            ui.checkbox(&mut o.shape_stroke, "Outline").on_hover_text("Outline in the background color");
                            if o.shape_stroke {
                                ui.add(egui::DragValue::new(&mut o.shape_stroke_width).range(1..=64).suffix(" px"));
                            }
                            ui.checkbox(&mut o.shape_closed, "Closed");
                        });
                        hint(ui, "Click the canvas to start a new shape layer; keep clicking to add points. Shapes stay editable and draw crisp pixels.");
                    }
                }
            }
            ToolKind::Tile => {
                group(ui, &theme, Section::Tools, |ui| {
                    ui.label(format!("Tile {}", state.tool_opts.tile_index));
                    let o = &mut state.tool_opts;
                    crate::ui::widgets::icon_toggle(ui, icons::FLIP_HORIZONTAL, icons::FLIP_HORIZONTAL, &mut o.tile_flip_h, "Stamp flipped horizontally", 22.0);
                    crate::ui::widgets::icon_toggle(ui, icons::FLIP_VERTICAL, icons::FLIP_VERTICAL, &mut o.tile_flip_v, "Stamp flipped vertically", 22.0);
                    if ui.button("Tileset…").on_hover_text("Pick tiles in the Tileset panel").clicked() {
                        state.show_panel_requests.push(crate::workspace::PanelKind::Tileset);
                    }
                });
                hint(ui, "Click or drag to stamp the tile into a tilemap layer. Alt or right-click clears cells; Ctrl+click picks a tile.");
            }
            ToolKind::Slice => {
                let doc_id = state.active_doc;
                let sel = state.active().and_then(|d| d.slice_sel.and_then(|i| d.doc.state().slices.get(i).cloned().map(|s| (i, s))));
                match (doc_id, sel) {
                    (Some(doc_id), Some((i, s))) => {
                        group(ui, &theme, Section::Tools, |ui| {
                            ui.label(format!("{}  {} × {} at ({}, {})", s.name, s.rect.w, s.rect.h, s.rect.x, s.rect.y));
                            if ui.button("Properties…").on_hover_text("Name, bounds, 9-slice center, pivot, color").clicked() {
                                crate::dialogs::open_slice_props(state, doc_id, i);
                            }
                            if ui.button(format!("{} Delete", icons::TRASH)).on_hover_text("Delete").clicked() {
                                crate::tools::slice::delete_selected(state);
                            }
                        });
                    }
                    _ => hint(ui, "Drag to add a slice. Drag a slice to move it, its corners to resize; double-click for its properties."),
                }
            }
            ToolKind::Eyedropper => {
                group(ui, &theme, Section::Tools, |ui| {
                    let o = &mut state.tool_opts;
                    let mut scope = if o.eyedropper_sample_merged { 2 } else if o.eyedropper_sample_below { 1 } else { 0 };
                    ui.label("Sample");
                    egui::ComboBox::from_id_salt("eyedropper_scope")
                        .selected_text(["Current layer", "Current & below", "All layers"][scope])
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut scope, 0, "Current layer");
                            ui.selectable_value(&mut scope, 1, "Current & below")
                                .on_hover_text("The layers up to the active one: picks the gray under index-painting adjustments");
                            ui.selectable_value(&mut scope, 2, "All layers");
                        });
                    o.eyedropper_sample_merged = scope == 2;
                    o.eyedropper_sample_below = scope == 1;
                });
                hint(ui, "Alt+click / right-click sets the background color");
            }
            ToolKind::Crop => {
                if let Some(r) = state.tool_opts.crop_rect {
                    group(ui, &theme, Section::Tools, |ui| {
                        ui.label(format!("{} × {} px at ({}, {})", r.w, r.h, r.x, r.y));
                        if ui.button(format!("{} Apply", icons::CHECK)).on_hover_text("Enter").clicked() {
                            if let Some(id) = state.active_doc {
                                crate::tools::commit_crop(state, id);
                            }
                        }
                        if ui.button(format!("{} Cancel", icons::X)).on_hover_text("Esc").clicked() {
                            state.tool_opts.crop_rect = None;
                        }
                    });
                } else {
                    hint(ui, "Drag to define the crop area, then press Enter.");
                }
            }
            ToolKind::Move => hint(ui, "Drag to move the selection or layer. Shift constrains, arrow keys nudge."),
            ToolKind::Zoom => {
                group(ui, &theme, Section::View, |ui| {
                    ui.checkbox(&mut state.tool_opts.zoom_scrub, "Scrubby zoom");
                    if let Some(entry) = state.active_mut() {
                        if ui.button("Fit").clicked() {
                            let (w, h) = (entry.doc.width(), entry.doc.height());
                            entry.view.fit(w, h);
                        }
                        if ui.button("100%").clicked() {
                            entry.view.set_zoom(1.0, None);
                        }
                        ui.label(format!("{:.0}%", entry.view.zoom * 100.0));
                    }
                });
            }
            ToolKind::Hand => {
                hint(ui, "Drag to pan. Hold Space with any tool to pan temporarily. Double-click fits the view.")
            }
            ToolKind::RotateView => {
                group(ui, &theme, Section::View, |ui| {
                    if let Some(entry) = state.active_mut() {
                        let mut deg = entry.view.rotation_degrees();
                        if ui.add(egui::DragValue::new(&mut deg).prefix("Angle ").suffix("°").speed(0.5)).changed() {
                            entry.view.set_rotation(deg.to_radians());
                        }
                        if ui.button("Reset").clicked() {
                            entry.view.reset_rotation();
                        }
                        let mut flip = entry.view.flip_h;
                        if ui.checkbox(&mut flip, "Flip view").changed() {
                            entry.view.flip_h = flip;
                        }
                    }
                });
            }
        }
    });
}

/// Dim helper text trailing the chips.
fn hint(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).weak().small());
}

/// Symmetry chip (brush section) shared by brush-driven tools.
fn symmetry_group(ui: &mut Ui, state: &mut AppState) {
    let theme = state.settings.ui.palette();
    group(ui, &theme, Section::Brush, |ui| symmetry_options(ui, state));
}

/// Options while pixels are floating (paste / Free Transform): mode, numbers,
/// aspect lock, filter, apply/cancel.
/// Widest the W / H fields go; the box itself is capped in `FloatingPaste`.
const MAX_SCALE_PCT: f32 = 100_000.0;

fn floating_options(ui: &mut Ui, state: &mut AppState) {
    use crate::tools::floating::Mode;
    let theme = state.settings.ui.palette();
    let is_paste = state.floating.as_ref().is_some_and(|f| f.is_paste());
    group(ui, &theme, Section::Tools, |ui| {
        ui.label(icon(if is_paste { icons::SELECTION } else { icons::ARROWS_OUT_CARDINAL }, 15.0));
        ui.label(egui::RichText::new(if is_paste { "Paste" } else { "Free Transform" }).strong());
    });
    let mut apply = false;
    let mut cancel = false;
    let p = state.settings.ui.palette();
    if let Some(f) = state.floating.as_mut() {
        chip(ui, p.section(Section::Tools), p.section_tint(Section::Tools), |ui| {
            let mut mode = f.mode;
            egui::ComboBox::from_id_salt("float_mode").selected_text(mode.label()).width(80.0).show_ui(ui, |ui| {
                for m in Mode::ALL {
                    ui.selectable_value(&mut mode, m, m.label());
                }
            });
            if mode != f.mode {
                f.set_mode(mode);
            }
            match f.mode {
                Mode::Freeform | Mode::Resize | Mode::Rotate => {
                    let c = f.center;
                    let (mut cx, mut cy) = (c.x, c.y);
                    ui.label("X");
                    let rx = ui.add(egui::DragValue::new(&mut cx).speed(1.0).fixed_decimals(0));
                    ui.label("Y");
                    let ry = ui.add(egui::DragValue::new(&mut cy).speed(1.0).fixed_decimals(0));
                    if rx.changed() || ry.changed() {
                        f.set_center(qsketch_core::Pt::new(cx, cy));
                    }
                    let (pw, ph) = f.scale_pct();
                    let (mut w, mut h) = (pw, ph);
                    // `clamp_existing_to_range` must stay off: a box scaled
                    // past the range by its handles would otherwise be written
                    // back clamped every frame, fighting the drag (the shape
                    // jittered and crept). The range still caps typing and
                    // dragging the field itself.
                    fn pct(v: &mut f32) -> egui::DragValue<'_> {
                        egui::DragValue::new(v)
                            .speed(0.5)
                            .suffix("%")
                            .range(0.1..=MAX_SCALE_PCT)
                            .clamp_existing_to_range(false)
                    }
                    ui.label("W");
                    let rw = ui.add(pct(&mut w));
                    ui.label("H");
                    let rh = ui.add(pct(&mut h));
                    // A handle drag owns the box while it lasts.
                    if (rw.changed() || rh.changed()) && f.drag.is_none() {
                        if f.keep_aspect {
                            if rw.changed() {
                                h = w;
                            } else {
                                w = h;
                            }
                        }
                        let (sw, sh) = (f.source.width() as f32, f.source.height() as f32);
                        f.set_size(sw * w / 100.0, sh * h / 100.0);
                    }
                    let mut deg = f.angle.to_degrees();
                    ui.label(icon(icons::ARROWS_CLOCKWISE, 13.0)).on_hover_text("Rotation");
                    if ui.add(egui::DragValue::new(&mut deg).speed(0.5).suffix("°").fixed_decimals(1)).changed() {
                        f.set_angle(deg);
                    }
                    ui.checkbox(&mut f.keep_aspect, "Keep aspect")
                        .on_hover_text("Corner handles keep the aspect ratio (Shift toggles this while dragging)");
                }
                Mode::Deform => {
                    ui.label(egui::RichText::new("Drag corners or edges").weak());
                }
                Mode::Warp => {
                    ui.label("Grid");
                    let mut div = f.warp_div;
                    for d in [2u32, 3, 4, 6] {
                        ui.selectable_value(&mut div, d, format!("{d}×{d}"));
                    }
                    if div != f.warp_div {
                        f.set_warp_div(div);
                    }
                }
            }
            let mut filter = f.filter;
            ui.selectable_value(&mut filter, qsketch_core::raster::ResizeFilter::Bilinear, "Smooth");
            ui.selectable_value(&mut filter, qsketch_core::raster::ResizeFilter::Nearest, "Pixel");
            ui.selectable_value(&mut filter, qsketch_core::raster::ResizeFilter::RotSprite, "RotSprite")
                .on_hover_text("Pixel-art rotation: smooths staircase edges and keeps the exact colors (no blending)");
            if filter != f.filter {
                f.filter = filter;
                f.reset_filter();
                state.settings.canvas.transform_filter = filter;
            }
            if !f.is_identity()
                && crate::ui::widgets::small_button(ui, "Reset")
                    .on_hover_text("Back to the original size and angle")
                    .clicked()
            {
                f.reset();
            }
            apply = ui.button(format!("{} Apply", icons::CHECK)).on_hover_text("Enter").clicked();
            cancel = ui.button(format!("{} Cancel", icons::X)).on_hover_text("Esc").clicked();
        });
    }
    if apply {
        crate::tools::floating::commit(state);
    } else if cancel {
        crate::tools::floating::cancel(state);
    }
}

fn brush_options(ui: &mut Ui, state: &mut AppState, tool: ToolKind) {
    let theme = state.settings.ui.palette();
    // The shape tools paint hard pixels in the foreground color, so they have
    // no brush options at all.
    if matches!(tool, ToolKind::Rect | ToolKind::Ellipse) {
        group(ui, &theme, Section::Tools, |ui| {
            ui.checkbox(&mut state.tool_opts.shape_filled, "Filled");
            if !state.tool_opts.shape_filled {
                let mut r = ui.add(
                    egui::DragValue::new(&mut state.tool_opts.shape_thickness)
                        .range(1..=64)
                        .speed(0.2)
                        .prefix("Thickness "),
                );
                let mut t = state.tool_opts.shape_thickness as f32;
                crate::ui::widgets::wheel_adjust(ui, &mut r, &mut t, 1.0, 1.0..=64.0);
                state.tool_opts.shape_thickness = t as u32;
                r.on_hover_text("Outline width in pixels");
            }
        });
        return;
    }
    let compact = state.settings.ui.compact_tool_options;
    let p = state.settings.ui.palette();
    let (col, tint) = (p.section(Section::Brush), p.section_tint(Section::Brush));
    // The selected tool's brush, even while a temporary tool (Space/Alt) is active.
    if state.brush_for_tool_mut(tool).is_none() {
        return;
    }
    let mut open_settings = false;
    // Tip shape: part of "what am I painting with", so it sits right after the
    // tool name rather than at the far end of the bar.
    let b = state.brush_for_tool_mut(tool).expect("checked above");
    // The preset's name ("Hard Round", "Pixel", …); the tip shape alone
    // ("Round") read like a tool that does not exist.
    let tip_shape = if b.is_round() { "Round".to_string() } else { b.tip.clone() };
    let label = if b.name.trim().is_empty() { tip_shape.clone() } else { b.name.clone() };
    let current_name = b.name.clone();
    let mut pick: Option<usize> = None;
    chip(ui, col, tint, |ui| {
        // The sliders icon opens the full Brush Settings; the name opens a
        // quick list of presets.
        if icon_button(
            ui,
            icons::SLIDERS_HORIZONTAL,
            "Brush Settings (F9): tip shape, dynamics, scattering, texture, color",
            20.0,
            false,
        )
        .clicked()
        {
            open_settings = true;
        }
        // A framed button both at rest and on hover, with no stroke: a frame
        // that only appears on hover grows the button by its stroke width and
        // makes the bar jitter.
        let btn = egui::Button::new(&label).stroke(egui::Stroke::NONE);
        let resp = ui.add(btn).on_hover_text(format!("Brush preset · tip: {tip_shape}\nClick to pick another preset"));
        let popup_id = ui.id().with("preset_quick_pick");
        egui::Popup::from_toggle_button_response(&resp)
            .id(popup_id)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| {
                ui.set_min_width(180.0);
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for (i, preset) in state.presets.iter().enumerate() {
                        let selected = preset.name == current_name;
                        if ui.selectable_label(selected, &preset.name).clicked() {
                            pick = Some(i);
                        }
                    }
                });
            });
    });
    if let Some(i) = pick {
        let preset = state.presets[i].clone();
        if let Some(b) = state.brush_for_tool_mut(tool) {
            *b = preset;
        }
    }
    // Primary knobs.
    chip(ui, col, tint, |ui| {
        let b = state.brush_for_tool_mut(tool).expect("checked above");
        param(ui, "Size", &mut b.size, 1.0..=500.0, " px", true, 0);
        if tool != ToolKind::Pencil && b.is_round() {
            param_pct(ui, "Hardness", &mut b.hardness);
        }
        param_pct(ui, "Opacity", &mut b.opacity);
        if tool != ToolKind::Pencil {
            param_pct(ui, "Flow", &mut b.flow);
        }
        b.clamp();
    });
    // Everything else in one chip: pressure toggles, stabilizer, symmetry,
    // and the rarely-touched extras folded into a "More" menu.
    chip(ui, col, tint, |ui| {
        {
            let b = state.brush_for_tool_mut(tool).expect("checked above");
            let r = icon_button(ui, icons::ARROWS_IN_LINE_VERTICAL, "Pressure controls size", 22.0, b.pressure_size);
            if r.clicked() {
                b.pressure_size = !b.pressure_size;
            }
            let r = icon_button(ui, icons::DROP_HALF, "Pressure controls opacity/flow", 22.0, b.pressure_opacity);
            if r.clicked() {
                b.pressure_opacity = !b.pressure_opacity;
            }
            // The pencil's pixel switch, and the eraser's too: with it off the
            // eraser takes pixels away the way the pencil puts them down.
            if matches!(tool, ToolKind::Pencil | ToolKind::Eraser) {
                let r = icon_button(ui, icons::WAVE_SINE, "Anti-aliasing", 22.0, b.antialias);
                if r.clicked() {
                    b.antialias = !b.antialias;
                }
                if !b.antialias {
                    let r = icon_button(
                        ui,
                        icons::STAIRS,
                        "Pixel-perfect: drop the doubled corner pixels of hand-drawn lines",
                        22.0,
                        b.pixel_perfect,
                    );
                    if r.clicked() {
                        b.pixel_perfect = !b.pixel_perfect;
                    }
                }
            }
            if matches!(tool, ToolKind::Brush | ToolKind::Pencil | ToolKind::Eraser) {
                dither_combo(ui, b);
            }
            if tool.is_paint() || tool.is_selection_brush() {
                stabilizer_options(ui, b);
            }
        }
        ui.add_space(2.0);
        symmetry_options(ui, state);
        // Shading ink: needs a document palette; the selected run in the
        // Palette panel is the ramp.
        if matches!(tool, ToolKind::Brush | ToolKind::Pencil)
            && state.active().is_some_and(|d| !d.doc.state().palette.is_empty())
        {
            ui.add_space(2.0);
            let o = &mut state.tool_opts;
            let r = icon_button(
                ui,
                icons::STACK,
                "Shading: pixels under the brush step along the selected palette ramp instead of taking the foreground color",
                22.0,
                o.shading,
            );
            if r.clicked() {
                o.shading = !o.shading;
            }
            if o.shading {
                let glyph = if o.shading_reverse { icons::ARROW_LEFT } else { icons::ARROW_RIGHT };
                let tip = if o.shading_reverse { "Step back along the ramp" } else { "Step forward along the ramp" };
                if icon_button(ui, glyph, tip, 22.0, false).clicked() {
                    o.shading_reverse = !o.shading_reverse;
                }
            }
        }
        if !compact {
            let b = state.brush_for_tool_mut(tool).expect("checked above");
            ui.add_space(2.0);
            ui.menu_button(format!("{} More", icons::DOTS_THREE_OUTLINE), |ui| {
                ui.set_min_width(220.0);
                ui.spacing_mut().slider_width = 120.0;
                crate::ui::widgets::percent_slider(ui, "Smoothing", &mut b.smoothing);
                let mut sp = b.spacing * 100.0;
                ui.horizontal(|ui| {
                    ui.label("Spacing");
                    if ui.add(egui::Slider::new(&mut sp, 1.0..=200.0).suffix("%").fixed_decimals(0)).changed() {
                        b.spacing = sp / 100.0;
                    }
                });
                if !matches!(tool, ToolKind::Pencil | ToolKind::Eraser) {
                    ui.checkbox(&mut b.antialias, "Anti-aliasing");
                    if !b.antialias {
                        ui.checkbox(&mut b.pixel_perfect, "Pixel-perfect")
                            .on_hover_text("Drop the doubled corner pixels of hand-drawn lines (round tips)");
                    }
                }
                if tool.is_paint() && b.stabilizer == qsketch_core::StabilizerMode::Rope {
                    ui.checkbox(&mut b.stabilizer_catch_up, "Stabilizer catches up on release")
                        .on_hover_text("Finish the stroke at the pointer when released");
                }
            })
            .response
            .on_hover_text("Smoothing, spacing, anti-aliasing");
            b.clamp();
        }
    });
    if open_settings {
        state.show_panel_requests.push(crate::workspace::PanelKind::BrushSettings);
    }
}

/// Stabilizer mode + strength (rope / moving average) for freehand strokes.
fn stabilizer_options(ui: &mut Ui, b: &mut qsketch_core::BrushSettings) {
    use qsketch_core::StabilizerMode;
    let label = match b.stabilizer {
        StabilizerMode::Off => "Stabilizer",
        StabilizerMode::Rope => "Rope",
        StabilizerMode::Average => "Average",
    };
    egui::ComboBox::from_id_salt("stabilizer_mode").selected_text(label).width(78.0).show_ui(ui, |ui| {
        ui.selectable_value(&mut b.stabilizer, StabilizerMode::Off, "Off");
        ui.selectable_value(&mut b.stabilizer, StabilizerMode::Rope, "Rope")
            .on_hover_text("Lazy brush: the stroke follows the pointer on a rope, ignoring small jitter");
        ui.selectable_value(&mut b.stabilizer, StabilizerMode::Average, "Average")
            .on_hover_text("Moving average of recent samples");
    });
    if b.stabilizer != StabilizerMode::Off {
        param_pct(ui, "Strength", &mut b.stabilizer_strength);
    }
}

/// Mirror / radial painting toggles shared by every brush-driven tool.
fn symmetry_options(ui: &mut Ui, state: &mut AppState) {
    let sym = &mut state.symmetry;
    let r = icon_button(ui, icons::FLIP_HORIZONTAL, "Mirror horizontally (Shift+H)", 22.0, sym.horizontal);
    if r.clicked() {
        sym.horizontal = !sym.horizontal;
    }
    let r = icon_button(ui, icons::FLIP_VERTICAL, "Mirror vertically (Shift+V)", 22.0, sym.vertical);
    if r.clicked() {
        sym.vertical = !sym.vertical;
    }
    let r = icon_button(ui, icons::FLOWER, "Radial symmetry", 22.0, sym.radial >= 2);
    if r.clicked() {
        sym.radial = if sym.radial >= 2 { 0 } else { 6 };
    }
    if sym.radial >= 2 {
        let mut n = sym.radial;
        if ui.add(egui::DragValue::new(&mut n).range(2..=64).suffix("×")).on_hover_text("Radial copies").changed() {
            sym.radial = n.max(2);
        }
    }
    if sym.active() {
        let r = icon_button(
            ui,
            icons::CROSSHAIR,
            "Click the canvas to set the symmetry center",
            22.0,
            state.symmetry_pick_center,
        );
        if r.clicked() {
            state.symmetry_pick_center = !state.symmetry_pick_center;
        }
        if state.symmetry.center.is_some()
            && crate::ui::widgets::small_button(ui, "Center")
                .on_hover_text("Reset the center to the canvas middle")
                .clicked()
        {
            state.symmetry.center = None;
        }
    }
}

/// Text tool: family, size, style, alignment, spacing, anti-aliasing.
fn text_options(ui: &mut Ui, state: &mut AppState) {
    use qsketch_core::TextAlign;
    state.fonts.ensure_loaded();
    let compact = state.settings.ui.compact_tool_options;
    let editing = state.text_edit.is_some();
    let p = state.settings.ui.palette();
    let (col, tint) = (p.section(Section::Tools), p.section_tint(Section::Tools));
    // Family picker with a filter box: system font lists run to hundreds.
    let current = state.tool_opts.text_family.clone();
    let filter_id = ui.id().with("font_filter");
    chip(ui, col, tint, |ui| {
        egui::ComboBox::from_id_salt("text_family").selected_text(&current).width(150.0).show_ui(ui, |ui| {
            let mut filter: String = ui.data_mut(|d| d.get_temp(filter_id).unwrap_or_default());
            let r = ui.add(egui::TextEdit::singleline(&mut filter).hint_text("Filter…").desired_width(150.0));
            if r.changed() {
                ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
            }
            let needle = filter.to_lowercase();
            ui.set_max_height(320.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                let names: Vec<String> = state
                    .fonts
                    .families()
                    .iter()
                    .filter(|n| needle.is_empty() || n.to_lowercase().contains(&needle))
                    .cloned()
                    .collect();
                for name in names {
                    if ui.selectable_label(name == current, &name).clicked() {
                        state.tool_opts.text_family = name;
                    }
                }
            });
        });
        let dir = crate::fonts::FontLibrary::dir().map(|d| d.display().to_string()).unwrap_or_default();
        if icon_button(
            ui,
            icons::ARROWS_CLOCKWISE,
            &format!("Rescan fonts (drop .ttf/.otf files into {dir})"),
            22.0,
            false,
        )
        .clicked()
        {
            state.fonts.reload();
        }
        let t = &mut state.tool_opts.text;
        ui.add(
            egui::DragValue::new(&mut t.size)
                .range(1.0..=2000.0)
                .speed(1.0)
                .prefix("Size ")
                .suffix(" px")
                .fixed_decimals(0),
        );
    });
    chip(ui, col, tint, |ui| {
        let fam = state.tool_opts.text_family.clone();
        let real_bold = state.fonts.has_style(&fam, true, false);
        let real_italic = state.fonts.has_style(&fam, false, true);
        let tip_b = if real_bold { "Bold" } else { "Bold (synthesized: this family has no bold face)" };
        let tip_i = if real_italic { "Italic" } else { "Italic (synthesized: this family has no italic face)" };
        if icon_button(ui, icons::TEXT_B, tip_b, 22.0, state.tool_opts.text_bold).clicked() {
            state.tool_opts.text_bold = !state.tool_opts.text_bold;
        }
        if icon_button(ui, icons::TEXT_ITALIC, tip_i, 22.0, state.tool_opts.text_italic).clicked() {
            state.tool_opts.text_italic = !state.tool_opts.text_italic;
        }
        let t = &mut state.tool_opts.text;
        for (al, glyph, tip) in [
            (TextAlign::Left, icons::TEXT_ALIGN_LEFT, "Align left"),
            (TextAlign::Center, icons::TEXT_ALIGN_CENTER, "Align center"),
            (TextAlign::Right, icons::TEXT_ALIGN_RIGHT, "Align right"),
        ] {
            if icon_button(ui, glyph, tip, 22.0, t.align == al).clicked() {
                t.align = al;
            }
        }
        if icon_button(ui, icons::WAVE_SINE, "Anti-aliasing (off for crisp pixel text)", 22.0, t.antialias).clicked() {
            t.antialias = !t.antialias;
        }
        if !compact {
            ui.add(
                egui::DragValue::new(&mut t.line_height).prefix("Line ").range(0.5..=4.0).speed(0.01).fixed_decimals(2),
            )
            .on_hover_text("Line height");
            ui.add(
                egui::DragValue::new(&mut t.letter_spacing)
                    .prefix("Track ")
                    .range(-50.0..=200.0)
                    .speed(0.1)
                    .suffix(" px")
                    .fixed_decimals(1),
            )
            .on_hover_text("Letter spacing");
        }
        t.clamp();
    });
    if editing {
        chip(ui, col, tint, |ui| {
            if ui.button(format!("{} Apply", icons::CHECK)).on_hover_text("Enter").clicked() {
                crate::tools::text::commit(state);
            }
            if ui.button(format!("{} Cancel", icons::X)).on_hover_text("Esc").clicked() {
                crate::tools::text::cancel(state);
            }
        });
    } else {
        hint(ui, "Click the canvas to place text. Drag the preview to move it.");
    }
}

fn selection_ops(ui: &mut Ui, state: &mut AppState) {
    let ops = [
        (SelectionOp::Replace, icons::SELECTION, "New selection"),
        (SelectionOp::Add, icons::SELECTION_PLUS, "Add to selection (Shift)"),
        (SelectionOp::Subtract, icons::SELECTION_SLASH, "Subtract from selection (Alt)"),
        (SelectionOp::Intersect, icons::SELECTION_INVERSE, "Intersect with selection (Shift+Alt)"),
    ];
    for (op, glyph, tip) in ops {
        if icon_button(ui, glyph, tip, 22.0, state.tool_opts.selection_op == op).clicked() {
            state.tool_opts.selection_op = op;
        }
    }
}

/// Label for a dither pattern in the brush controls.
pub fn dither_label(d: qsketch_core::filter::DitherPattern) -> &'static str {
    use qsketch_core::filter::DitherPattern as D;
    match d {
        D::None => "Off",
        D::Bayer2 => "2×2",
        D::Bayer4 => "4×4",
        D::Bayer8 => "8×8",
        D::Noise => "Noise",
    }
}

/// Dither ink: the brush lays whole pixels in an ordered pattern instead of
/// blending, so softness and opacity turn into dither density.
pub fn dither_combo(ui: &mut egui::Ui, b: &mut qsketch_core::BrushSettings) {
    use qsketch_core::filter::DitherPattern as D;
    egui::ComboBox::from_id_salt("dither_ink")
        .selected_text(format!("Dither {}", dither_label(b.dither)))
        .width(92.0)
        .show_ui(ui, |ui| {
            for d in [D::None, D::Bayer2, D::Bayer4, D::Bayer8, D::Noise] {
                ui.selectable_value(&mut b.dither, d, dither_label(d));
            }
        })
        .response
        .on_hover_text(
            "Dither ink: whole pixels in an ordered pattern instead of blending. A soft edge or a lower opacity paints a sparser pattern (50% = checkerboard).",
        );
}
