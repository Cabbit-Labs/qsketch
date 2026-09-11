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
            | ToolKind::Ellipse => brush_options(ui, state, tool),
            ToolKind::Contour => {
                group(ui, &theme, Section::Tools, |ui| {
                    ui.checkbox(&mut state.tool_opts.contour_outline, "Outline with brush")
                        .on_hover_text("Stroke the closed edge with the current brush after filling");
                });
                symmetry_group(ui, state);
                hint(ui, "Drag a freehand shape; it closes and fills with the foreground color.");
            }
            ToolKind::Text => text_options(ui, state),
            ToolKind::RectSelect | ToolKind::EllipseSelect | ToolKind::Lasso => {
                group(ui, &theme, Section::Tools, |ui| selection_ops(ui, state));
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
            ToolKind::Eyedropper => {
                group(ui, &theme, Section::Tools, |ui| {
                    ui.checkbox(&mut state.tool_opts.eyedropper_sample_merged, "All layers")
                        .on_hover_text("Sample all layers");
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
                    ui.label("W");
                    let rw = ui.add(egui::DragValue::new(&mut w).speed(0.5).suffix("%").range(1.0..=10000.0));
                    ui.label("H");
                    let rh = ui.add(egui::DragValue::new(&mut h).speed(0.5).suffix("%").range(1.0..=10000.0));
                    if rw.changed() || rh.changed() {
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
                        .on_hover_text("Corner handles keep the aspect ratio (Shift toggles)");
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
            if filter != f.filter {
                f.filter = filter;
                f.reset_filter();
            }
            if !f.is_identity()
                && ui.small_button("Reset").on_hover_text("Back to the original size and angle").clicked()
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
    if matches!(tool, ToolKind::Rect | ToolKind::Ellipse) {
        group(ui, &theme, Section::Tools, |ui| {
            ui.checkbox(&mut state.tool_opts.shape_filled, "Filled");
        });
        // A filled shape is a plain fill of the foreground color; the brush
        // only matters for the outlined variant.
        if state.tool_opts.shape_filled {
            return;
        }
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
    let tip_label = if b.is_round() { "Round".to_string() } else { b.tip.clone() };
    chip(ui, col, tint, |ui| {
        if ui
            .add(egui::Button::new(format!("{} {}", icons::SLIDERS_HORIZONTAL, tip_label)).frame_when_inactive(false))
            .on_hover_text("Open Brush Settings (F9): tip shape, dynamics, scattering, texture, color")
            .clicked()
        {
            open_settings = true;
        }
    });
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
            if tool == ToolKind::Pencil {
                let r = icon_button(ui, icons::WAVE_SINE, "Anti-aliasing", 22.0, b.antialias);
                if r.clicked() {
                    b.antialias = !b.antialias;
                }
            }
            if tool.is_paint() {
                stabilizer_options(ui, b);
            }
        }
        ui.add_space(2.0);
        symmetry_options(ui, state);
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
                if tool != ToolKind::Pencil {
                    ui.checkbox(&mut b.antialias, "Anti-aliasing");
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
            && ui.small_button("Center").on_hover_text("Reset the center to the canvas middle").clicked()
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
