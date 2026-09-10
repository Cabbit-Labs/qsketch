//! Brush Settings panel: Photoshop-style section list on the left, the
//! selected section's controls on the right, and a live stroke preview below.

use egui::{Color32, Sense, Ui, Vec2};
use qsketch_core::{AngleControl, BrushSettings, Rgba8, TextureMode, TipImage};

use crate::brush_library::{BrushLibrary, LibraryKind};
use crate::state::AppState;
use crate::tools::ToolKind;
use crate::ui::icons;
use crate::ui::widgets::{icon_button, percent_slider};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    TipShape,
    ShapeDynamics,
    Scattering,
    Texture,
    Transfer,
    ColorDynamics,
    Noise,
}

impl Page {
    const ALL: [Page; 7] = [
        Page::TipShape,
        Page::ShapeDynamics,
        Page::Scattering,
        Page::Texture,
        Page::Transfer,
        Page::ColorDynamics,
        Page::Noise,
    ];

    fn label(self) -> &'static str {
        match self {
            Page::TipShape => "Brush Tip Shape",
            Page::ShapeDynamics => "Shape Dynamics",
            Page::Scattering => "Scattering",
            Page::Texture => "Texture",
            Page::Transfer => "Transfer",
            Page::ColorDynamics => "Color Dynamics",
            Page::Noise => "Noise",
        }
    }

    /// The enable flag for toggleable sections.
    fn flag(self, b: &mut BrushSettings) -> Option<&mut bool> {
        match self {
            Page::TipShape => None,
            Page::ShapeDynamics => Some(&mut b.shape_dynamics),
            Page::Scattering => Some(&mut b.scattering),
            Page::Texture => Some(&mut b.texture_enabled),
            Page::Transfer => Some(&mut b.transfer),
            Page::ColorDynamics => Some(&mut b.color_dynamics),
            Page::Noise => Some(&mut b.noise),
        }
    }
}

/// Side effects requested by the UI, run after the borrow of the brush ends.
enum Deferred {
    ImportFile(LibraryKind),
    FromSelection(LibraryKind),
    Remove(LibraryKind, String),
    OpenFolder(LibraryKind),
    SavePreset,
    ResetToPreset,
}

/// The brush a tool edits here: the tool's own brush, or the Brush tool's when
/// a non-painting tool is active.
fn edited_tool(state: &AppState) -> ToolKind {
    let t = state.tool;
    if t.uses_brush() {
        t
    } else {
        ToolKind::Brush
    }
}

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let tool = edited_tool(state);
    let mut deferred: Vec<Deferred> = Vec::new();
    {
        let AppState { brush, pencil, eraser, library, brush_page, fg, bg, presets, .. } = state;
        let b: &mut BrushSettings = match tool {
            ToolKind::Pencil => pencil,
            ToolKind::Eraser => eraser,
            _ => brush,
        };
        let (fg, bg) = (*fg, *bg);
        let is_preset = presets.iter().any(|p| p.name == b.name);

        // Header
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{} {}", crate::ui::widgets::icon(tool.icon(), 14.0).text(), b.name))
                    .strong(),
            );
            ui.label(egui::RichText::new(format!("· editing the {} tool", tool.label())).weak().small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, icons::PLUS, "Save as a new preset", 22.0, false).clicked() {
                    deferred.push(Deferred::SavePreset);
                }
                if is_preset
                    && icon_button(ui, icons::ARROW_COUNTER_CLOCKWISE, "Reset to the saved preset", 22.0, false)
                        .clicked()
                {
                    deferred.push(Deferred::ResetToPreset);
                }
            });
        });
        ui.separator();

        let preview_h = 96.0;
        let full = ui.available_rect_before_wrap();
        let wide = full.width() >= 440.0;
        let body = egui::Rect::from_min_max(
            full.min,
            egui::pos2(full.max.x, (full.max.y - preview_h - 10.0).max(full.min.y + 120.0)),
        );
        if wide {
            let left = egui::Rect::from_min_size(body.min, egui::vec2(168.0, body.height()));
            let right = egui::Rect::from_min_max(egui::pos2(left.max.x + 6.0, body.min.y), body.max);
            let mut lui =
                ui.new_child(egui::UiBuilder::new().max_rect(left).layout(egui::Layout::top_down(egui::Align::Min)));
            egui::Frame::group(lui.style()).inner_margin(4.0).show(&mut lui, |ui| {
                ui.set_width(left.width() - 10.0);
                ui.set_min_height(left.height() - 10.0);
                section_list(ui, b, brush_page);
            });
            let mut rui =
                ui.new_child(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::top_down(egui::Align::Min)));
            egui::ScrollArea::vertical().id_salt("brush_settings_page").auto_shrink([false, false]).show(
                &mut rui,
                |ui| {
                    ui.spacing_mut().slider_width = (ui.available_width() - 140.0).clamp(80.0, 260.0);
                    page_ui(ui, b, library, tool, *brush_page, &mut deferred);
                },
            );
        } else {
            // Narrow (docked in a side column): section picker on top, page below.
            let mut cui =
                ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
            let ui = &mut cui;
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("brush_settings_section")
                    .selected_text(brush_page.label())
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for p in Page::ALL {
                            ui.selectable_value(brush_page, p, p.label());
                        }
                    });
                if let Some(flag) = brush_page.flag(b) {
                    ui.checkbox(flag, "On");
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut b.antialias, "Anti-aliasing");
                ui.label("Smoothing");
                ui.spacing_mut().slider_width = 50.0;
                let mut pct = (b.smoothing * 100.0).round();
                if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%").fixed_decimals(0)).changed() {
                    b.smoothing = pct / 100.0;
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt("brush_settings_page_narrow").auto_shrink([false, false]).show(
                ui,
                |ui| {
                    ui.spacing_mut().slider_width = (ui.available_width() - 100.0).clamp(60.0, 260.0);
                    page_ui(ui, b, library, tool, *brush_page, &mut deferred);
                },
            );
        }
        ui.advance_cursor_after_rect(body);
        b.clamp();

        // Stroke preview
        ui.separator();
        let w = full.width().clamp(120.0, 1400.0);
        let size = [w.round() as usize, preview_h as usize];
        let key = format!("brush-settings-{}", tool.label());
        let tex = library.stroke_preview(ui.ctx(), &key, b, fg, bg, size);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, preview_h), Sense::hover());
        let checker = fg.luma() > 0.6;
        if checker {
            crate::ui::widgets::checkerboard(ui.painter(), rect, 8.0);
        } else {
            ui.painter().rect_filled(rect, 3, ui.visuals().extreme_bg_color);
        }
        ui.painter().image(
            tex.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        ui.painter().rect_stroke(rect, 3, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    }
    run_deferred(ui, state, tool, deferred);
}

fn page_ui(
    ui: &mut Ui,
    b: &mut BrushSettings,
    library: &mut BrushLibrary,
    tool: ToolKind,
    page: Page,
    deferred: &mut Vec<Deferred>,
) {
    match page {
        Page::TipShape => tip_page(ui, b, library, tool, deferred),
        Page::ShapeDynamics => shape_dynamics_page(ui, b),
        Page::Scattering => scattering_page(ui, b),
        Page::Texture => texture_page(ui, b, library, deferred),
        Page::Transfer => transfer_page(ui, b),
        Page::ColorDynamics => color_dynamics_page(ui, b),
        Page::Noise => noise_page(ui, b),
    }
}

fn section_list(ui: &mut Ui, b: &mut BrushSettings, page: &mut Page) {
    for p in Page::ALL {
        let selected = *page == p;
        let row_h = 24.0;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), Sense::click());
        let fill = if selected {
            ui.visuals().selection.bg_fill
        } else if resp.hovered() {
            ui.visuals().widgets.hovered.bg_fill
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 3, fill);
        let mut x = rect.left() + 6.0;
        if let Some(flag) = p.flag(b) {
            let box_rect = egui::Rect::from_center_size(egui::pos2(x + 7.0, rect.center().y), Vec2::splat(14.0));
            let box_resp = ui.interact(box_rect.expand(3.0), resp.id.with(("flag", p.label())), Sense::click());
            if box_resp.clicked() {
                *flag = !*flag;
            }
            let v = ui.visuals();
            ui.painter().rect_filled(box_rect, 2, v.widgets.inactive.bg_fill);
            ui.painter().rect_stroke(box_rect, 2, v.widgets.inactive.bg_stroke, egui::StrokeKind::Inside);
            if *flag {
                ui.painter().text(
                    box_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    icons::CHECK,
                    egui::FontId::new(11.0, crate::ui::iconset::family()),
                    v.text_color(),
                );
            }
            x += 22.0;
        } else {
            x += 0.0;
        }
        let color = if p.flag(b).is_some_and(|f| !*f) && !selected {
            ui.visuals().weak_text_color()
        } else {
            ui.visuals().text_color()
        };
        ui.painter().text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            p.label(),
            egui::FontId::proportional(13.0),
            color,
        );
        if resp.clicked() {
            *page = p;
            // Clicking a disabled section's name turns it on, like Photoshop.
            if let Some(flag) = p.flag(b) {
                if !*flag {
                    *flag = true;
                }
            }
        }
        if p == Page::TipShape {
            ui.add_space(2.0);
            ui.separator();
            ui.add_space(2.0);
        }
    }
    ui.add_space(6.0);
    ui.separator();
    ui.checkbox(&mut b.antialias, "Anti-aliasing");
    ui.horizontal(|ui| {
        ui.label("Smoothing");
        let mut pct = (b.smoothing * 100.0).round();
        ui.spacing_mut().slider_width = 60.0;
        if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%").fixed_decimals(0).show_value(false)).changed() {
            b.smoothing = pct / 100.0;
        }
        ui.label(egui::RichText::new(format!("{pct:.0}%")).weak().small());
    });
}

fn control_combo(ui: &mut Ui, id: &str, on: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.label("Control:");
        egui::ComboBox::from_id_salt(id).selected_text(if *on { "Pen Pressure" } else { "Off" }).show_ui(ui, |ui| {
            changed |= ui.selectable_value(on, false, "Off").changed();
            changed |= ui.selectable_value(on, true, "Pen Pressure").changed();
        });
    });
    changed
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(egui::RichText::new(text).strong());
}

/// Thumbnail grid of tips or textures; returns a clicked name ("" = Round).
fn image_grid(
    ui: &mut Ui,
    library: &mut BrushLibrary,
    kind: LibraryKind,
    current: &str,
    include_round: bool,
    deferred: &mut Vec<Deferred>,
) -> Option<String> {
    let cell = 46.0;
    let mut picked = None;
    let names: Vec<(String, bool)> = library.list(kind).iter().map(|t| (t.name.clone(), t.builtin)).collect();
    let cols = ((ui.available_width() + 4.0) / (cell + 4.0)).floor().max(1.0) as usize;
    let mut items: Vec<Option<(String, bool)>> = Vec::new();
    if include_round {
        items.push(None);
    }
    items.extend(names.into_iter().map(Some));
    egui::Frame::new().fill(ui.visuals().extreme_bg_color).inner_margin(4.0).corner_radius(4).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::Grid::new(("tip_grid", kind.dir_name())).spacing([4.0, 4.0]).show(ui, |ui| {
            for (i, item) in items.iter().enumerate() {
                let (name, builtin) = match item {
                    None => (String::new(), true),
                    Some((n, b)) => (n.clone(), *b),
                };
                let selected = if name.is_empty() {
                    current.is_empty() || current == qsketch_core::brush::ROUND_TIP
                } else {
                    current == name
                };
                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                let v = ui.visuals();
                let fill = if selected {
                    v.selection.bg_fill
                } else if resp.hovered() {
                    v.widgets.hovered.bg_fill
                } else {
                    v.faint_bg_color
                };
                ui.painter().rect_filled(rect, 4, fill);
                if name.is_empty() {
                    ui.painter().circle_filled(rect.center(), cell * 0.32, v.text_color());
                } else if let Some(tex) = library.preview(ui.ctx(), kind, &name, 64) {
                    let inner = rect.shrink(3.0);
                    ui.painter().image(
                        tex.id(),
                        inner,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        v.text_color(),
                    );
                }
                if selected {
                    ui.painter().rect_stroke(
                        rect,
                        4,
                        egui::Stroke::new(1.5, v.selection.stroke.color),
                        egui::StrokeKind::Inside,
                    );
                }
                let label = if name.is_empty() { "Round (soft/hard by Hardness)".to_string() } else { name.clone() };
                let resp = resp.on_hover_text(label);
                if resp.clicked() {
                    picked = Some(name.clone());
                }
                if !builtin {
                    resp.context_menu(|ui| {
                        if ui.button(format!("{} Delete \"{}\"", icons::TRASH, name)).clicked() {
                            deferred.push(Deferred::Remove(kind, name.clone()));
                            ui.close();
                        }
                    });
                }
                if (i + 1) % cols == 0 {
                    ui.end_row();
                }
            }
        });
    });
    picked
}

fn library_buttons(ui: &mut Ui, kind: LibraryKind, deferred: &mut Vec<Deferred>) {
    let what = match kind {
        LibraryKind::Tip => "tip",
        LibraryKind::Texture => "texture",
    };
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(format!("{} Import…", icons::FOLDER_OPEN))
            .on_hover_text(format!("Load a PNG/JPEG as a {what}"))
            .clicked()
        {
            deferred.push(Deferred::ImportFile(kind));
        }
        if ui
            .button(format!("{} From selection", icons::SELECTION))
            .on_hover_text(format!(
                "Define a {what} from the selected pixels of the active document (whole canvas if nothing is selected)"
            ))
            .clicked()
        {
            deferred.push(Deferred::FromSelection(kind));
        }
        if ui
            .small_button("Open folder")
            .on_hover_text("Drop PNGs here; they load on the next start or Rescan")
            .clicked()
        {
            deferred.push(Deferred::OpenFolder(kind));
        }
    });
}

fn tip_page(
    ui: &mut Ui,
    b: &mut BrushSettings,
    library: &mut BrushLibrary,
    tool: ToolKind,
    deferred: &mut Vec<Deferred>,
) {
    if let Some(name) = image_grid(ui, library, LibraryKind::Tip, &b.tip, true, deferred) {
        b.tip = name;
    }
    library_buttons(ui, LibraryKind::Tip, deferred);
    ui.add_space(4.0);
    ui.separator();

    heading(ui, "Size");
    ui.add(
        egui::Slider::new(&mut b.size, 1.0..=2000.0)
            .logarithmic(true)
            .suffix(" px")
            .fixed_decimals(0)
            .clamping(egui::SliderClamping::Always),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.checkbox(&mut b.flip_x, "Flip X");
        ui.checkbox(&mut b.flip_y, "Flip Y");
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label("Angle:");
                ui.add(egui::DragValue::new(&mut b.angle).suffix("°").speed(1.0).range(-360.0..=720.0));
            });
            ui.horizontal(|ui| {
                ui.label("Roundness:");
                let mut pct = (b.roundness * 100.0).round();
                if ui.add(egui::DragValue::new(&mut pct).suffix("%").speed(1.0).range(1.0..=100.0)).changed() {
                    b.roundness = pct / 100.0;
                }
            });
        });
        ui.add_space(8.0);
        angle_widget(ui, b);
    });
    ui.add_space(4.0);
    if b.is_round() {
        heading(ui, "Hardness");
        percent_slider(ui, "", &mut b.hardness);
    } else if tool != ToolKind::Pencil {
        ui.label(egui::RichText::new("Hardness applies to the round tip only.").weak().small());
    }
    heading(ui, "Spacing");
    let mut sp = b.spacing * 100.0;
    if ui
        .add(
            egui::Slider::new(&mut sp, 1.0..=500.0)
                .logarithmic(true)
                .suffix("%")
                .fixed_decimals(0)
                .clamping(egui::SliderClamping::Always),
        )
        .changed()
    {
        b.spacing = sp / 100.0;
    }
}

/// Interactive angle/roundness dial: drag to set the angle, scroll to change roundness.
fn angle_widget(ui: &mut Ui, b: &mut BrushSettings) {
    let size = 84.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let c = rect.center();
    let r = size * 0.42;
    let v = ui.visuals();
    ui.painter().rect_filled(rect, 4, v.extreme_bg_color);
    ui.painter().circle_stroke(c, r, egui::Stroke::new(1.0, v.weak_text_color()));
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let d = p - c;
            if d.length() > 2.0 {
                // Screen y points down; keep Photoshop's counter-clockwise-positive angle.
                b.angle = (-d.y).atan2(d.x).to_degrees().rem_euclid(360.0).round();
                if ui.input(|i| i.modifiers.shift) {
                    b.angle = (b.angle / 15.0).round() * 15.0;
                }
            }
        }
    }
    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
    if resp.hovered() && scroll != 0.0 {
        b.roundness = (b.roundness + scroll.signum() * 0.05).clamp(0.01, 1.0);
    }
    // The tip ellipse.
    let ang = -b.angle.to_radians();
    let (s, co) = ang.sin_cos();
    let n = 40;
    let pts: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            let (x, y) = (r * t.cos(), r * b.roundness * t.sin());
            egui::pos2(c.x + x * co - y * s, c.y + x * s + y * co)
        })
        .collect();
    ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.5, v.text_color())));
    // Axis + direction arrow.
    let ax = egui::vec2(co, s) * r;
    ui.painter().line_segment([c - ax, c + ax], egui::Stroke::new(1.0, v.weak_text_color()));
    let tip = c + ax;
    let side = egui::vec2(-s, co) * 4.0;
    let back = egui::vec2(co, s) * 7.0;
    ui.painter().add(egui::Shape::convex_polygon(
        vec![tip + back * 0.6, tip - back + side, tip - back - side],
        v.text_color(),
        egui::Stroke::NONE,
    ));
    let ay = egui::vec2(-s, co) * r * b.roundness;
    ui.painter().circle_filled(c + ay, 2.5, v.text_color());
    ui.painter().circle_filled(c - ay, 2.5, v.text_color());
    resp.on_hover_text("Drag to set the angle (Shift snaps to 15°). Scroll to change roundness.");
}

fn shape_dynamics_page(ui: &mut Ui, b: &mut BrushSettings) {
    if !b.shape_dynamics {
        ui.label(egui::RichText::new("Shape Dynamics is off. Tick the box to enable it.").weak());
        ui.add_space(4.0);
    }
    heading(ui, "Size Jitter");
    percent_slider(ui, "", &mut b.size_jitter);
    control_combo(ui, "size_ctrl", &mut b.pressure_size);
    ui.add_enabled_ui(b.pressure_size, |ui| {
        heading(ui, "Minimum Diameter");
        percent_slider(ui, "", &mut b.min_size);
    });
    ui.add_space(6.0);
    ui.separator();
    heading(ui, "Angle Jitter");
    percent_slider(ui, "", &mut b.angle_jitter);
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.label("Control:");
        egui::ComboBox::from_id_salt("angle_ctrl")
            .selected_text(match b.angle_control {
                AngleControl::Off => "Off",
                AngleControl::Direction => "Direction",
                AngleControl::Pressure => "Pen Pressure",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut b.angle_control, AngleControl::Off, "Off");
                ui.selectable_value(&mut b.angle_control, AngleControl::Direction, "Direction")
                    .on_hover_text("Rotate the tip to follow the stroke");
                ui.selectable_value(&mut b.angle_control, AngleControl::Pressure, "Pen Pressure");
            });
    });
    ui.add_space(6.0);
    ui.separator();
    heading(ui, "Roundness Jitter");
    percent_slider(ui, "", &mut b.roundness_jitter);
    ui.add_enabled_ui(b.roundness_jitter > 0.0, |ui| {
        heading(ui, "Minimum Roundness");
        percent_slider(ui, "", &mut b.min_roundness);
    });
}

fn scattering_page(ui: &mut Ui, b: &mut BrushSettings) {
    if !b.scattering {
        ui.label(egui::RichText::new("Scattering is off. Tick the box to enable it.").weak());
        ui.add_space(4.0);
    }
    heading(ui, "Scatter");
    let mut pct = b.scatter * 100.0;
    if ui
        .add(
            egui::Slider::new(&mut pct, 0.0..=500.0)
                .suffix("%")
                .fixed_decimals(0)
                .clamping(egui::SliderClamping::Always),
        )
        .changed()
    {
        b.scatter = pct / 100.0;
    }
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.checkbox(&mut b.scatter_both_axes, "Both Axes")
            .on_hover_text("Also scatter along the stroke, not only across it");
    });
    control_combo(ui, "scatter_ctrl", &mut b.scatter_pressure);
    ui.add_space(6.0);
    ui.separator();
    heading(ui, "Count");
    ui.add(egui::Slider::new(&mut b.scatter_count, 1..=16).clamping(egui::SliderClamping::Always));
    ui.label(egui::RichText::new("Dabs placed at every spacing step.").weak().small());
}

fn texture_page(ui: &mut Ui, b: &mut BrushSettings, library: &mut BrushLibrary, deferred: &mut Vec<Deferred>) {
    if !b.texture_enabled {
        ui.label(egui::RichText::new("Texture is off. Tick the box to enable it.").weak());
        ui.add_space(4.0);
    }
    if let Some(name) = image_grid(ui, library, LibraryKind::Texture, &b.texture, false, deferred) {
        b.texture = name;
        b.texture_enabled = true;
    }
    if b.texture.is_empty() {
        ui.label(egui::RichText::new("Pick a grain texture above.").weak().small());
    }
    library_buttons(ui, LibraryKind::Texture, deferred);
    ui.add_space(4.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.checkbox(&mut b.texture_invert, "Invert");
        ui.checkbox(&mut b.texture_each_tip, "Texture Each Tip").on_hover_text(
            "Sample the grain per dab so it moves with the brush, instead of staying fixed to the canvas",
        );
    });
    heading(ui, "Scale");
    let mut pct = b.texture_scale * 100.0;
    if ui
        .add(
            egui::Slider::new(&mut pct, 10.0..=800.0)
                .logarithmic(true)
                .suffix("%")
                .fixed_decimals(0)
                .clamping(egui::SliderClamping::Always),
        )
        .changed()
    {
        b.texture_scale = pct / 100.0;
    }
    heading(ui, "Mode");
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("tex_mode")
            .selected_text(match b.texture_mode {
                TextureMode::Multiply => "Multiply",
                TextureMode::Subtract => "Subtract",
                TextureMode::Height => "Height",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut b.texture_mode, TextureMode::Multiply, "Multiply")
                    .on_hover_text("Grain lightens the dab smoothly");
                ui.selectable_value(&mut b.texture_mode, TextureMode::Subtract, "Subtract")
                    .on_hover_text("Grain carves hard holes");
                ui.selectable_value(&mut b.texture_mode, TextureMode::Height, "Height")
                    .on_hover_text("Only the raised paper catches paint; depth sets how much of the paper is raised");
            });
    });
    heading(ui, "Depth");
    percent_slider(ui, "", &mut b.texture_depth);
}

fn transfer_page(ui: &mut Ui, b: &mut BrushSettings) {
    if !b.transfer {
        ui.label(egui::RichText::new("Transfer is off. Tick the box to enable it.").weak());
        ui.add_space(4.0);
    }
    heading(ui, "Flow Jitter");
    percent_slider(ui, "", &mut b.flow_jitter);
    control_combo(ui, "flow_ctrl", &mut b.pressure_opacity);
    ui.add_space(6.0);
    ui.separator();
    heading(ui, "Flow");
    percent_slider(ui, "", &mut b.flow);
    heading(ui, "Opacity");
    percent_slider(ui, "", &mut b.opacity);
    ui.label(
        egui::RichText::new("Flow is paint per dab; opacity caps how dark one stroke can build up.").weak().small(),
    );
}

fn color_dynamics_page(ui: &mut Ui, b: &mut BrushSettings) {
    if !b.color_dynamics {
        ui.label(egui::RichText::new("Color Dynamics is off. Tick the box to enable it.").weak());
        ui.add_space(4.0);
    }
    heading(ui, "Foreground/Background Jitter");
    percent_slider(ui, "", &mut b.fg_bg_jitter);
    control_combo(ui, "fgbg_ctrl", &mut b.fg_bg_pressure);
    ui.add_space(6.0);
    ui.separator();
    heading(ui, "Hue Jitter");
    percent_slider(ui, "", &mut b.hue_jitter);
    heading(ui, "Saturation Jitter");
    percent_slider(ui, "", &mut b.sat_jitter);
    heading(ui, "Brightness Jitter");
    percent_slider(ui, "", &mut b.bri_jitter);
    ui.label(egui::RichText::new("Each dab picks its own color. Not applied when erasing.").weak().small());
}

fn noise_page(ui: &mut Ui, b: &mut BrushSettings) {
    ui.checkbox(&mut b.noise, "Add noise to soft edges");
    ui.label(
        egui::RichText::new(
            "Randomizes the soft part of every dab so airbrush-like strokes look grainy instead of smooth.",
        )
        .weak()
        .small(),
    );
}

fn run_deferred(ui: &mut Ui, state: &mut AppState, tool: ToolKind, deferred: Vec<Deferred>) {
    use crate::ui::toasts::Level;
    for d in deferred {
        match d {
            Deferred::SavePreset => {
                let Some(b) = state.brush_for_tool_mut(tool).cloned() else { continue };
                let mut b = b;
                let base = b.name.trim_end_matches(char::is_numeric).trim().to_string();
                let mut n = 2;
                let mut name = b.name.clone();
                while state.presets.iter().any(|p| p.name == name) {
                    name = format!("{base} {n}");
                    n += 1;
                }
                b.name = name.clone();
                state.presets.push(b);
                if let Some(cur) = state.brush_for_tool_mut(tool) {
                    cur.name = name.clone();
                }
                state.toasts.push(Level::Success, format!("Saved preset \"{name}\""));
            }
            Deferred::ResetToPreset => {
                let Some(name) = state.brush_for_tool_mut(tool).map(|b| b.name.clone()) else { continue };
                if let Some(p) = state.presets.iter().find(|p| p.name == name).cloned() {
                    if let Some(b) = state.brush_for_tool_mut(tool) {
                        *b = p;
                    }
                }
            }
            Deferred::ImportFile(kind) => {
                let dlg = rfd::FileDialog::new()
                    .set_title(match kind {
                        LibraryKind::Tip => "Import Brush Tip",
                        LibraryKind::Texture => "Import Texture",
                    })
                    .add_filter(
                        "Images and brushes",
                        &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff", "gbr", "gih", "abr"],
                    )
                    .add_filter("Brush files (GIMP, Photoshop)", &["gbr", "gih", "abr"])
                    .add_filter("Images", &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff"]);
                let Some(path) = dlg.pick_file() else { continue };
                match state.library.import(kind, &path) {
                    Ok(name) => {
                        apply_library_pick(state, tool, kind, &name);
                        state.toasts.push(Level::Success, format!("Imported \"{name}\""));
                    }
                    Err(e) => state.toasts.push(Level::Error, format!("Couldn't import: {e:#}")),
                }
            }
            Deferred::FromSelection(kind) => match define_from_selection(state, kind) {
                Ok(img) => match state.library.add(kind, img) {
                    Ok(name) => {
                        apply_library_pick(state, tool, kind, &name);
                        state.toasts.push(Level::Success, format!("Defined \"{name}\""));
                    }
                    Err(e) => state.toasts.push(Level::Error, format!("Couldn't save: {e:#}")),
                },
                Err(e) => state.toasts.push(Level::Info, e.to_string()),
            },
            Deferred::Remove(kind, name) => {
                if let Err(e) = state.library.remove(kind, &name) {
                    state.toasts.push(Level::Error, e.to_string());
                    continue;
                }
                // Anything referencing it falls back to round / no texture.
                let fix = |b: &mut BrushSettings| match kind {
                    LibraryKind::Tip if b.tip == name => b.tip.clear(),
                    LibraryKind::Texture if b.texture == name => b.texture.clear(),
                    _ => {}
                };
                fix(&mut state.brush);
                fix(&mut state.pencil);
                fix(&mut state.eraser);
                state.presets.iter_mut().for_each(fix);
            }
            Deferred::OpenFolder(kind) => {
                if let Some(dir) = BrushLibrary::dir(kind) {
                    let _ = std::fs::create_dir_all(&dir);
                    if let Err(e) = open_dir(&dir) {
                        state.toasts.push(Level::Info, format!("{}: {e}", dir.display()));
                    }
                    state.library.reload_user();
                }
            }
        }
    }
    let _ = ui;
}

fn apply_library_pick(state: &mut AppState, tool: ToolKind, kind: LibraryKind, name: &str) {
    if let Some(b) = state.brush_for_tool_mut(tool) {
        match kind {
            LibraryKind::Tip => b.tip = name.to_string(),
            LibraryKind::Texture => {
                b.texture = name.to_string();
                b.texture_enabled = true;
            }
        }
    }
}

/// Flatten the active document inside its selection (or whole canvas) into a tip/texture.
fn define_from_selection(state: &AppState, kind: LibraryKind) -> anyhow::Result<TipImage> {
    let entry = state.active().ok_or_else(|| anyhow::anyhow!("Open a document first"))?;
    let doc = entry.doc.state();
    let flat = qsketch_core::composite::flatten(doc);
    let (rect, mask) = match &doc.selection {
        Some(m) if !m.is_empty() => (m.bounds(), Some(m.clone())),
        _ => (flat.rect(), None),
    };
    if rect.w > 2048 || rect.h > 2048 {
        anyhow::bail!("Selection is too large (max 2048 px per side)");
    }
    let mut raster = flat.crop(rect);
    if let Some(m) = mask {
        for y in 0..rect.h {
            for x in 0..rect.w {
                let c = m.coverage(rect.x + x, rect.y + y);
                if c < 1.0 {
                    let p = raster.get_pixel(x, y);
                    raster.set_pixel(x, y, Rgba8 { a: (p.a as f32 * c) as u8, ..p });
                }
            }
        }
    }
    let name = match kind {
        LibraryKind::Tip => "Custom Tip",
        LibraryKind::Texture => "Custom Texture",
    };
    Ok(BrushLibrary::from_raster(kind, name.into(), &raster))
}

fn open_dir(dir: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    let cmd = ("explorer", vec![dir.as_os_str().to_owned()]);
    #[cfg(target_os = "macos")]
    let cmd = ("open", vec![dir.as_os_str().to_owned()]);
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let cmd = ("xdg-open", vec![dir.as_os_str().to_owned()]);
    std::process::Command::new(cmd.0).args(cmd.1).spawn().map(|_| ())
}
