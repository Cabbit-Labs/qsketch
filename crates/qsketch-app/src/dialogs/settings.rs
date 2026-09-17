//! Preferences window: general, interface, canvas, tablet and a full
//! keyboard-shortcut editor with key capture and conflict detection.

use egui::{Context, RichText, Ui};

use crate::actions::{Action, Category, Shortcut};
use crate::settings::{
    BrushCursor, ChromeTexture, CustomPalette, IconSet, MouseChord, NewDocBackground, Settings, Theme, WheelBehavior,
};
use crate::state::AppState;
use crate::tools::ToolKind;
use crate::ui::iconset;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Interface,
    Icons,
    Canvas,
    Mouse,
    Tablet,
    Shortcuts,
}

pub struct SettingsDialog {
    pub page: Page,
    pub filter: String,
    /// Action whose shortcut is being captured, and which slot (None = add).
    pub capturing: Option<(Action, Option<usize>)>,
    /// What the last assignment took away from another action, so the page
    /// can say so: (shortcut, actions that lost it).
    pub conflict: Option<(Shortcut, Vec<Action>)>,
}

impl SettingsDialog {
    pub fn new(page: Page) -> Self {
        Self { page, filter: String::new(), capturing: None, conflict: None }
    }
}

pub fn show(ctx: &Context, state: &mut AppState) {
    if state.dialogs.settings.is_none() {
        return;
    }
    let mut open = true;
    let mut dlg = state.dialogs.settings.take().unwrap();
    egui::Window::new("Preferences")
        .id(egui::Id::new("preferences_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([780.0, 560.0])
        .default_pos(ctx.content_rect().center() - egui::vec2(390.0, 280.0))
        .show(ctx, |ui| {
            ui.set_min_size(egui::vec2(740.0, 500.0));
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(130.0);
                    for (p, label) in [
                        (Page::General, "General"),
                        (Page::Interface, "Interface"),
                        (Page::Icons, "Icons"),
                        (Page::Canvas, "Canvas"),
                        (Page::Mouse, "Mouse"),
                        (Page::Tablet, "Tablet & Pen"),
                        (Page::Shortcuts, "Keyboard Shortcuts"),
                    ] {
                        if ui.selectable_label(dlg.page == p, label).clicked() {
                            dlg.page = p;
                        }
                    }
                    ui.add_space(12.0);
                    if crate::ui::widgets::small_button(ui, "Reset all to defaults").clicked() {
                        let keep_recent = state.settings.general.recent_files.clone();
                        state.settings = Settings::default();
                        state.settings.general.recent_files = keep_recent;
                        state.keymap.reset_all();
                    }
                    if let Some(p) = Settings::path() {
                        ui.add_space(8.0);
                        ui.label(RichText::new("Stored at").weak().small());
                        ui.label(RichText::new(p.display().to_string()).weak().small())
                            .on_hover_text(p.display().to_string());
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_min_height(480.0);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match dlg.page {
                        Page::General => general(ui, state),
                        Page::Interface => interface(ui, state),
                        Page::Icons => icons_page(ui, state),
                        Page::Canvas => canvas(ui, state),
                        Page::Mouse => mouse(ui, state),
                        Page::Tablet => tablet(ui, state),
                        Page::Shortcuts => shortcuts(ui, ctx, state, &mut dlg),
                    });
                });
            });
        });
    if open {
        state.dialogs.settings = Some(dlg);
    } else {
        state.settings.sanitize();
    }
}

fn general(ui: &mut Ui, state: &mut AppState) {
    let g = &mut state.settings.general;
    ui.heading("General");
    ui.add_space(6.0);
    egui::Grid::new("gen_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Undo history limit");
        if ui.add(egui::Slider::new(&mut g.undo_limit, 5..=1000).logarithmic(true)).changed() {
            for d in &mut state.docs {
                d.doc.history.set_limit(g.undo_limit);
            }
        }
        ui.end_row();
        ui.label("Confirm before closing unsaved work");
        ui.checkbox(&mut g.confirm_close, "");
        ui.end_row();
        ui.label("Recent files to remember");
        ui.add(egui::Slider::new(&mut g.max_recent, 1..=30));
        ui.end_row();
        ui.label("Default new document");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut g.new_doc_width).range(1..=16384).suffix(" px"));
            ui.label("×");
            ui.add(egui::DragValue::new(&mut g.new_doc_height).range(1..=16384).suffix(" px"));
        });
        ui.end_row();
        ui.label("Default background");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut g.new_doc_background, NewDocBackground::White, "White");
            ui.selectable_value(&mut g.new_doc_background, NewDocBackground::Transparent, "Transparent");
            ui.selectable_value(&mut g.new_doc_background, NewDocBackground::BackgroundColor, "Background color");
        });
        ui.end_row();
        ui.label("Reopen last files on start");
        ui.checkbox(&mut g.open_last_files_on_start, "");
        ui.end_row();
        ui.label("Paste images onto");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut g.paste_new_layer, true, "A new layer above");
            ui.selectable_value(&mut g.paste_new_layer, false, "The current layer");
        })
        .response
        .on_hover_text("Where Ctrl+V puts pasted pixels. Either way they float for placement until Enter.");
        ui.end_row();
        ui.label("Delete also deselects");
        ui.checkbox(&mut g.deselect_after_delete, "")
            .on_hover_text("Delete clears the selected pixels and drops the selection. Off: the selection stays.");
        ui.end_row();
        ui.label("Autosave for crash recovery");
        ui.horizontal(|ui| {
            ui.checkbox(&mut g.autosave, "");
            ui.add_enabled_ui(g.autosave, |ui| {
                ui.label("every");
                let mut secs = g.autosave_interval_secs;
                if ui.add(egui::DragValue::new(&mut secs).range(15..=3600).suffix(" s").speed(5)).changed() {
                    g.autosave_interval_secs = secs;
                }
            });
        })
        .response
        .on_hover_text(
            "Unsaved documents are snapshotted to the settings folder and offered for recovery on the next start.",
        );
        ui.end_row();
        ui.label("Keep previous versions of saved files");
        ui.horizontal(|ui| {
            ui.checkbox(&mut g.backups, "");
            ui.add_enabled_ui(g.backups, |ui| {
                ui.label("keep");
                ui.add(egui::DragValue::new(&mut g.backup_versions).range(1..=20));
                ui.label("versions");
            });
        })
        .response
        .on_hover_text(
            "Saving over a file first copies the old one into the settings folder. File \u{25b8} Restore Previous Version opens one as a new document.",
        );
        ui.end_row();
    });
    ui.add_space(14.0);
    ui.heading("Updates");
    ui.add_space(6.0);
    let u = &mut state.settings.update;
    egui::Grid::new("update_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Check for updates on start");
        ui.horizontal(|ui| {
            ui.checkbox(&mut u.check_on_start, "");
            ui.label("every");
            ui.add(egui::DragValue::new(&mut u.check_interval_hours).range(1..=720).suffix(" h"));
        });
        ui.end_row();
        ui.label("Update manifest URL");
        ui.add(
            egui::TextEdit::singleline(&mut u.manifest_url)
                .desired_width(320.0)
                .hint_text(crate::update::EXAMPLE_MANIFEST_URL),
        )
        .on_hover_text(
            "Where qsketch looks for new releases. Leave empty to disable update checks; ask whoever distributes your build for their URL.",
        );
        ui.end_row();
        ui.label("Skipped version");
        ui.horizontal(|ui| {
            if u.skipped_version.is_empty() {
                ui.label(RichText::new("none").weak());
            } else {
                ui.label(&u.skipped_version);
                if crate::ui::widgets::small_button(ui, "Clear").clicked() {
                    u.skipped_version.clear();
                }
            }
        });
        ui.end_row();
    });
    let url = state.settings.update.effective_manifest_url();
    if ui.add_enabled(!state.updater.busy() && url.is_some(), egui::Button::new("Check now")).clicked() {
        if let Some(url) = url.clone() {
            state.updater.check(url, true);
        }
    }
    let note = if url.is_some() {
        "Updates are downloaded from the manifest above and their signature is verified before installing."
    } else {
        "Update checks are off until a manifest URL is entered above."
    };
    ui.label(RichText::new(format!("Current version {}. {note}", crate::update::CURRENT_VERSION)).weak().small());
}

fn interface(ui: &mut Ui, state: &mut AppState) {
    let u = &mut state.settings.ui;
    ui.heading("Interface");
    ui.add_space(6.0);
    ui.label("Theme");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for t in Theme::ALL {
            theme_card(ui, &mut u.theme, t, &u.custom_palette);
        }
    });
    if u.theme == Theme::Custom {
        ui.add_space(6.0);
        custom_palette_editor(ui, &mut u.custom_palette);
    }
    ui.add_space(8.0);
    texture_editor(ui, &mut u.texture);
    ui.add_space(8.0);
    egui::Grid::new("ui_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("UI scale");
        ui.add(egui::Slider::new(&mut u.scale, 0.75..=2.0).step_by(0.05).fixed_decimals(2));
        ui.end_row();
        ui.label("Snap floating panels to window edges");
        ui.checkbox(&mut u.snap_windows, "");
        ui.end_row();
        ui.label("Snap distance");
        ui.add(egui::Slider::new(&mut u.snap_distance, 2.0..=40.0).suffix(" px"));
        ui.end_row();
        ui.label("Show tooltips");
        ui.checkbox(&mut u.show_tooltips, "");
        ui.end_row();
        ui.label("Use system window frame");
        ui.checkbox(&mut u.native_frame, "").on_hover_text(
            "Show the OS title bar above the menus instead of qsketch's compact strip. Applies immediately.",
        );
        ui.end_row();
        ui.label("Minimal tool options bar");
        ui.checkbox(&mut u.compact_tool_options, "")
            .on_hover_text("Hide the \"More\" menu and secondary text controls");
        ui.end_row();
        ui.label("Tools strip");
        ui.horizontal(|ui| {
            ui.checkbox(&mut u.tools_locked, "Locked")
                .on_hover_text("Also toggled by the lock icon on the Tools tab. Unlock to drag tools around.");
            if ui.add_enabled(!u.tool_order.is_empty(), egui::Button::new("Reset order")).clicked() {
                u.tool_order.clear();
            }
        });
        ui.end_row();
    });
    ui.add_space(8.0);
    ui.label(RichText::new("Panels can be dragged by their tabs to dock anywhere or float. Use Window › Reset Workspace to restore the default layout.").weak());
}

/// Clickable preview card for a theme: a miniature of the chrome with the
/// section colors, the name underneath.
fn theme_card(ui: &mut Ui, current: &mut Theme, t: Theme, custom: &CustomPalette) {
    use crate::ui::theme::Palette;
    let p = if t == Theme::Custom { custom.to_palette() } else { Palette::for_theme(t) };
    let size = egui::vec2(84.0, 64.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let selected = *current == t;
    let painter = ui.painter();
    let preview = egui::Rect::from_min_size(rect.min, egui::vec2(size.x, 46.0));
    painter.rect_filled(preview, 4, p.bg);
    // Top bar.
    painter.rect_filled(
        egui::Rect::from_min_size(preview.min, egui::vec2(size.x, 7.0)),
        egui::CornerRadius { nw: 4, ne: 4, ..Default::default() },
        p.panel,
    );
    // Left tools strip + right panel column.
    painter.rect_filled(
        egui::Rect::from_min_size(preview.min + egui::vec2(0.0, 7.0), egui::vec2(12.0, 39.0)),
        0,
        p.panel,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(preview.min + egui::vec2(size.x - 26.0, 7.0), egui::vec2(26.0, 39.0)),
        0,
        p.panel,
    );
    // Neutral rows in the side column, one accent row.
    for i in 0..3 {
        let y = preview.top() + 10.0 + i as f32 * 12.0;
        let c = if i == 0 { p.accent } else { p.text_dim };
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(preview.right() - 24.0, y), egui::vec2(22.0, 2.0)),
            1,
            c,
        );
    }
    painter.rect_filled(
        egui::Rect::from_min_size(preview.min + egui::vec2(2.0, 10.0), egui::vec2(8.0, 2.0)),
        1,
        p.accent,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(preview.min + egui::vec2(2.0, 16.0), egui::vec2(8.0, 2.0)),
        1,
        p.text_dim,
    );
    // Canvas + accent dot.
    painter.rect_filled(
        egui::Rect::from_min_size(preview.min + egui::vec2(16.0, 12.0), egui::vec2(38.0, 30.0)),
        2,
        if p.dark { egui::Color32::from_gray(235) } else { egui::Color32::WHITE },
    );
    painter.circle_filled(preview.min + egui::vec2(size.x - 13.0, 3.5), 2.2, p.accent);
    let stroke = if selected {
        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color)
    } else if resp.hovered() {
        egui::Stroke::new(1.0, ui.visuals().widgets.hovered.fg_stroke.color)
    } else {
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
    };
    painter.rect_stroke(preview, 4, stroke, egui::StrokeKind::Outside);
    let text = RichText::new(t.label()).small();
    let text = if selected { text.strong() } else { text };
    painter.text(
        egui::pos2(rect.center().x, preview.bottom() + 9.0),
        egui::Align2::CENTER_CENTER,
        t.label(),
        egui::FontId::proportional(11.5),
        if selected { ui.visuals().strong_text_color() } else { ui.visuals().text_color() },
    );
    let _ = text;
    if resp.on_hover_text(t.description()).clicked() {
        *current = t;
    }
}

/// Panel texture: style, strength, sheen, and the custom image path.
fn texture_editor(ui: &mut Ui, t: &mut crate::settings::TextureSettings) {
    ui.label("Panel texture");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for k in ChromeTexture::ALL {
            ui.selectable_value(&mut t.kind, k, k.label());
        }
    });
    ui.add_enabled_ui(t.kind != ChromeTexture::None, |ui| {
        egui::Grid::new("texture_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Strength");
            ui.add(egui::Slider::new(&mut t.strength, 0.0..=0.5).step_by(0.01).fixed_decimals(2));
            ui.end_row();
            ui.label("Scale");
            ui.add(egui::Slider::new(&mut t.scale, 0.25..=4.0).logarithmic(true).fixed_decimals(2).suffix("×"))
                .on_hover_text("Size multiplier for the tile");
            ui.end_row();
            ui.label("Smooth filtering");
            ui.checkbox(&mut t.smooth, "")
                .on_hover_text("Bilinear filtering when the tile is scaled. Off = crisp, aliased pixels.");
            ui.end_row();
            ui.label("Top sheen");
            ui.checkbox(&mut t.sheen, "").on_hover_text("Faint highlight along the top edge of each panel");
            ui.end_row();
            if t.kind == ChromeTexture::Custom {
                ui.label("Image");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut t.custom_path)
                            .desired_width(260.0)
                            .hint_text("Path to a seamless tile (PNG/JPG)"),
                    );
                    if ui.button("Browse…").clicked() {
                        let dlg = rfd::FileDialog::new()
                            .set_title("Choose Panel Texture")
                            .add_filter("Images", &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff"]);
                        if let Some(p) = dlg.pick_file() {
                            t.custom_path = p.display().to_string();
                        }
                    }
                });
                ui.end_row();
                if !t.custom_path.is_empty() && !std::path::Path::new(&t.custom_path).is_file() {
                    ui.label("");
                    ui.colored_label(ui.visuals().warn_fg_color, "File not found; no texture is drawn.");
                    ui.end_row();
                }
            }
        });
    });
    ui.label(
        RichText::new("The tile is converted to neutral light/dark grain, so it follows any theme.").weak().small(),
    );
}

/// Primary + secondary color pickers for the Custom theme, plus "start from" presets.
fn custom_palette_editor(ui: &mut Ui, c: &mut CustomPalette) {
    use crate::ui::theme::Palette;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Start from").weak());
        for (label, p) in [
            ("Ink", Palette::ink()),
            ("Graphite", Palette::graphite()),
            ("Light", Palette::light()),
            ("Sepia", Palette::sepia()),
        ] {
            if crate::ui::widgets::small_button(ui, label).clicked() {
                *c = CustomPalette::from_palette(&p);
            }
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        for (name, tip, rgb) in [
            ("Primary", "Panels and chrome; backgrounds, widgets, borders and text are shaded from it", &mut c.primary),
            ("Secondary", "Accent: selection, links, the active tool", &mut c.secondary),
        ] {
            let mut col = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
            if egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::Opaque).changed() {
                *rgb = [col.r(), col.g(), col.b()];
            }
            ui.label(name).on_hover_text(tip);
            ui.add_space(12.0);
        }
    });
    ui.label(
        RichText::new(format!(
            "Changes apply live. The chrome is shaded {} from the primary color.",
            if c.is_dark() { "as a dark theme" } else { "as a light theme" }
        ))
        .weak()
        .small(),
    );
}

/// Icon set cards and the per-tool override editor.
fn icons_page(ui: &mut Ui, state: &mut AppState) {
    let u = &mut state.settings.ui;
    ui.heading("Icons");
    ui.add_space(6.0);
    ui.label("Icon set");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for set in IconSet::ALL {
            icon_set_card(ui, &mut u.icon_set, set);
        }
    });
    ui.add_space(10.0);
    egui::CollapsingHeader::new("Advanced: override individual tool icons").id_salt("icon_overrides").show(ui, |ui| {
        ui.label(RichText::new("Pick any Phosphor glyph for a tool. Overrides apply on top of the chosen set.").weak());
        ui.add_space(4.0);
        let filter_id = ui.id().with("glyph_filter");
        egui::Grid::new("icon_overrides_grid").num_columns(4).spacing([10.0, 4.0]).striped(true).show(ui, |ui| {
            for t in ToolKind::ALL {
                let key = iconset::tool_key(t);
                let current = t.icon();
                ui.label(crate::ui::widgets::icon(current, 18.0));
                ui.label(t.label());
                let name = iconset::name_of(current).unwrap_or("?");
                let overridden = u.icon_overrides.contains_key(&key);
                let btn_text = if overridden { format!("{name} (custom)") } else { name.to_string() };
                let popup_id = ui.id().with(("glyph_popup", t));
                let r = ui.button(btn_text);
                if r.clicked() {
                    egui::Popup::toggle_id(ui.ctx(), popup_id);
                }
                let chosen = glyph_picker_popup(ui, popup_id, &r, filter_id);
                if let Some(g) = chosen {
                    u.icon_overrides.insert(key.clone(), g.to_string());
                }
                if ui.add_enabled(overridden, egui::Button::new("Reset").small()).clicked() {
                    u.icon_overrides.remove(&key);
                }
                ui.end_row();
            }
        });
        if !u.icon_overrides.is_empty() && crate::ui::widgets::small_button(ui, "Reset all overrides").clicked() {
            u.icon_overrides.clear();
        }
    });
}

/// Searchable grid of every Phosphor glyph anchored under `anchor`. Returns
/// the chosen glyph name.
fn glyph_picker_popup(ui: &mut Ui, id: egui::Id, anchor: &egui::Response, filter_id: egui::Id) -> Option<&'static str> {
    let mut chosen = None;
    egui::Popup::from_response(anchor).id(id).open_memory(None).show(|ui| {
        ui.set_min_width(340.0);
        let mut filter: String = ui.data_mut(|d| d.get_temp(filter_id).unwrap_or_default());
        let r = ui.add(egui::TextEdit::singleline(&mut filter).hint_text("Search glyphs…").desired_width(320.0));
        if r.changed() {
            ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
        }
        let needle = filter.to_uppercase().replace(' ', "_");
        ui.set_max_height(280.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
                for (name, glyph) in crate::ui::icons::ICONS.iter() {
                    if !needle.is_empty() && !name.contains(needle.as_str()) {
                        continue;
                    }
                    if crate::ui::widgets::icon_button(ui, glyph, name, 26.0, false).clicked() {
                        chosen = Some(*name);
                    }
                }
            });
        });
    });
    if chosen.is_some() {
        egui::Popup::close_id(ui.ctx(), id);
    }
    chosen
}

/// Preview card for an icon set: a row of representative tool glyphs.
fn icon_set_card(ui: &mut Ui, current: &mut IconSet, set: IconSet) {
    use crate::ui::theme::{ICON_FONT, ICON_FONT_FILL};
    let size = egui::vec2(150.0, 58.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let selected = *current == set;
    let painter = ui.painter();
    let preview = egui::Rect::from_min_size(rect.min, egui::vec2(size.x, 40.0));
    painter.rect_filled(preview, 4, ui.visuals().extreme_bg_color);
    let family = egui::FontFamily::Name(if set.filled() { ICON_FONT_FILL } else { ICON_FONT }.into());
    let sample = [ToolKind::Move, ToolKind::Brush, ToolKind::Pencil, ToolKind::Fill, ToolKind::Text, ToolKind::Zoom];
    let step = preview.width() / sample.len() as f32;
    for (i, t) in sample.iter().enumerate() {
        painter.text(
            egui::pos2(preview.left() + step * (i as f32 + 0.5), preview.center().y),
            egui::Align2::CENTER_CENTER,
            iconset::set_glyph(set, *t),
            egui::FontId::new(18.0, family.clone()),
            ui.visuals().text_color(),
        );
    }
    let stroke = if selected {
        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color)
    } else if resp.hovered() {
        egui::Stroke::new(1.0, ui.visuals().widgets.hovered.fg_stroke.color)
    } else {
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
    };
    painter.rect_stroke(preview, 4, stroke, egui::StrokeKind::Outside);
    painter.text(
        egui::pos2(rect.center().x, preview.bottom() + 9.0),
        egui::Align2::CENTER_CENTER,
        set.label(),
        egui::FontId::proportional(11.5),
        if selected { ui.visuals().strong_text_color() } else { ui.visuals().text_color() },
    );
    if resp.on_hover_text(set.description()).clicked() {
        *current = set;
    }
}

fn canvas(ui: &mut Ui, state: &mut AppState) {
    let c = &mut state.settings.canvas;
    ui.heading("Canvas");
    ui.add_space(6.0);
    egui::Grid::new("canvas_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Mouse wheel");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut c.wheel, WheelBehavior::Zoom, "Zooms (Shift/Ctrl pan)");
            ui.selectable_value(&mut c.wheel, WheelBehavior::Scroll, "Scrolls (Ctrl/Alt zoom)");
        });
        ui.end_row();
        ui.label("Invert wheel zoom direction");
        ui.checkbox(&mut c.invert_wheel_zoom, "");
        ui.end_row();
        ui.label("Zoom toward cursor");
        ui.checkbox(&mut c.zoom_to_cursor, "");
        ui.end_row();
        ui.label("Smooth (bilinear) filtering when zoomed out");
        ui.checkbox(&mut c.smooth_zoom_out, "");
        ui.end_row();
        ui.label("Quick rotate (hold Q) follows the pointer");
        ui.checkbox(&mut c.quick_rotate_follow_pointer, "").on_hover_text(
            "Rotate the view by moving the mouse while Q is held, no click needed. Off: drag to rotate.",
        );
        ui.end_row();
        ui.label("Double-tap Q resets the rotation");
        ui.checkbox(&mut c.quick_rotate_double_tap_reset, "");
        ui.end_row();
        ui.label("Flash selected layer");
        ui.checkbox(&mut c.flash_selected_layer, "")
            .on_hover_text("When the active layer changes, its pixels light up on the canvas for a moment.");
        ui.end_row();
        ui.label("Pan inertia");
        ui.checkbox(&mut c.pan_inertia, "").on_hover_text(
            "A fast hand-tool (Space) drag keeps gliding briefly after release instead of stopping dead.",
        );
        ui.end_row();
        ui.label("Brush cursor");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut c.brush_cursor, BrushCursor::Outline, "Outline");
            ui.selectable_value(&mut c.brush_cursor, BrushCursor::Crosshair, "Crosshair");
            ui.selectable_value(&mut c.brush_cursor, BrushCursor::Both, "Both");
            ui.selectable_value(&mut c.brush_cursor, BrushCursor::Hidden, "Hidden");
        });
        ui.end_row();
        ui.label("Color picker loupe");
        ui.horizontal(|ui| {
            ui.checkbox(&mut c.pick_loupe, "Show");
            ui.add_enabled_ui(c.pick_loupe, |ui| {
                ui.add(egui::DragValue::new(&mut c.pick_loupe_size).range(40.0..=260.0).speed(1.0).suffix(" pt"))
                    .on_hover_text("Diameter of the magnifier");
                ui.add(egui::DragValue::new(&mut c.pick_loupe_pixels).range(3..=41).speed(0.2).suffix(" px"))
                    .on_hover_text("How many canvas pixels it spans");
            });
        })
        .response
        .on_hover_text("A zoomed circle of pixels while a color is being picked, so single pixels can be picked without zooming in.");
        ui.end_row();
        ui.label("Pixel grid");
        ui.horizontal(|ui| {
            ui.checkbox(&mut c.show_pixel_grid, "Show");
            ui.label("from zoom");
            ui.add(egui::DragValue::new(&mut c.pixel_grid_min_zoom).range(2.0..=64.0).suffix("×"));
        });
        ui.end_row();
        ui.label("Transparency checkerboard");
        ui.horizontal(|ui| {
            let mut a = egui::Color32::from_rgb(c.checker_a[0], c.checker_a[1], c.checker_a[2]);
            if egui::color_picker::color_edit_button_srgba(ui, &mut a, egui::color_picker::Alpha::Opaque).changed() {
                c.checker_a = [a.r(), a.g(), a.b()];
            }
            let mut b = egui::Color32::from_rgb(c.checker_b[0], c.checker_b[1], c.checker_b[2]);
            if egui::color_picker::color_edit_button_srgba(ui, &mut b, egui::color_picker::Alpha::Opaque).changed() {
                c.checker_b = [b.r(), b.g(), b.b()];
            }
            ui.add(egui::Slider::new(&mut c.checker_size, 2.0..=64.0).suffix(" px").text("size"));
        });
        ui.end_row();
        ui.label("Area outside the canvas");
        ui.horizontal(|ui| {
            let mut custom = c.outside_color.is_some();
            if ui.checkbox(&mut custom, "Custom color").changed() {
                c.outside_color = if custom { Some([38, 38, 38]) } else { None };
            }
            if let Some(col) = c.outside_color.as_mut() {
                let mut o = egui::Color32::from_rgb(col[0], col[1], col[2]);
                if egui::color_picker::color_edit_button_srgba(ui, &mut o, egui::color_picker::Alpha::Opaque).changed()
                {
                    *col = [o.r(), o.g(), o.b()];
                }
            } else {
                ui.label(egui::RichText::new("follows the theme").weak());
            }
        });
        ui.end_row();
    });
}

fn mouse(ui: &mut Ui, state: &mut AppState) {
    let d = crate::settings::MouseSettings::default();
    let m = &mut state.settings.mouse;
    ui.heading("Mouse");
    ui.add_space(6.0);
    egui::Grid::new("mouse_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Middle-button drag pans the view");
        toggle_row(ui, &mut m.middle_drag_pans, d.middle_drag_pans, None);
        ui.end_row();
        ui.label("Right-click opens quick brush settings");
        toggle_row(ui, &mut m.right_click_brush_popup, d.right_click_brush_popup, None);
        ui.end_row();
        ui.label("Alt+click picks a color");
        toggle_row(
            ui,
            &mut m.alt_click_picks,
            d.alt_click_picks,
            Some(
                "Alt takes the foreground color, Alt+right-click the background, with any tool that paints a color. Independent of the chords below.",
            ),
        );
        ui.end_row();
        ui.label("Pick foreground color");
        chord_binder(ui, "pick_fg", &mut m.pick_foreground, d.pick_foreground);
        ui.end_row();
        ui.label("Pick background color");
        chord_binder(ui, "pick_bg", &mut m.pick_background, d.pick_background);
        ui.end_row();
        ui.label("Quick move (drag as the Move tool)");
        chord_binder(ui, "quick_move", &mut m.quick_move, d.quick_move);
        ui.end_row();
    });
    ui.add_space(8.0);
    ui.label(
        RichText::new(
            "Pick chords work with every tool that paints a color (brush, pencil, shapes, fill, gradient, text); holding a chord's modifiers shows the eyedropper, and holding the button shows a zoomed loupe. Alt picks a color whatever the chords say unless that is turned off above. Right-click picks the foreground color by default; binding a pick chord to a bare right-click takes that button away from the quick brush settings popup above, which is why the popup is off out of the box. Quick move drags the active layer, or the selected pixels when there is a selection, with any tool; holding its modifiers shows the Move cursor. Reset puts a setting back to what it ships as. Keyboard chords (marquee on G, Ctrl+R rotate, Shift+X flip, Ctrl+V paste…) are edited under Keyboard Shortcuts.",
        )
        .weak(),
    );
}

/// A checkbox with the Reset that every row on this page carries.
fn toggle_row(ui: &mut Ui, value: &mut bool, default: bool, hover: Option<&str>) {
    ui.horizontal(|ui| {
        let r = ui.checkbox(value, "");
        if let Some(h) = hover {
            r.on_hover_text(h);
        }
        if ui
            .add_enabled(
                *value != default,
                egui::Button::new(egui::RichText::new("Reset").small()).min_size(RESET_BUTTON),
            )
            .clicked()
        {
            *value = default;
        }
    });
}

/// A button showing a mouse chord. Right-click (or middle-click) it to bind
/// that button straight away; left-click arms it, and then the next mouse
/// press anywhere in the dialog is taken, with whatever modifiers are held.
/// Backspace/Delete clears, Escape cancels. A bare left click can't be bound:
/// it's how every tool draws, so it just disarms. Only one binder is ever
/// armed, and Reset puts the chord back to what it ships as.
fn chord_binder(ui: &mut Ui, id: &str, chord: &mut Option<MouseChord>, default: Option<MouseChord>) {
    // One shared slot, so arming a binder disarms whichever was armed before.
    let slot = egui::Id::new("chord_armed_binder");
    let armed_now: Option<String> = ui.ctx().data(|d| d.get_temp(slot)).unwrap_or_default();
    let mut armed = armed_now.as_deref() == Some(id);
    let label = if armed {
        "Press a chord…".to_string()
    } else {
        chord.map(|c| c.label()).unwrap_or_else(|| "—".to_string())
    };
    let set_armed = |ui: &Ui, on: bool| {
        let v: Option<String> = if on { Some(id.to_string()) } else { None };
        ui.ctx().data_mut(|d| d.insert_temp(slot, v));
    };
    let btn = ui.add_sized(egui::vec2(150.0, 20.0), egui::Button::new(label).selected(armed)).on_hover_text(
        "Right-click to bind right-click · left-click, then press the chord you want · Reset restores the default",
    );
    let mut captured = false;
    if armed {
        let events = ui.input(|i| i.events.clone());
        for ev in events {
            match ev {
                egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } => {
                    armed = false;
                    captured = true;
                }
                egui::Event::Key { key: egui::Key::Backspace | egui::Key::Delete, pressed: true, .. } => {
                    *chord = None;
                    armed = false;
                    captured = true;
                }
                // Any press ends the arming, wherever it lands: a chord that
                // has to be aimed at the button is a chord you can't bind if
                // the aiming is what went wrong.
                egui::Event::PointerButton { pressed: true, button, modifiers, .. } => {
                    if let Some(c) = MouseChord::from_egui(modifiers, button) {
                        if c.has_modifiers() || c.button != crate::settings::MouseButton::Left {
                            *chord = Some(c);
                        }
                    }
                    armed = false;
                    captured = true;
                }
                _ => {}
            }
        }
        if captured {
            set_armed(ui, false);
        }
    } else if btn.secondary_clicked() || btn.middle_clicked() {
        // Binding the button you just pressed is what that press means here.
        let button = if btn.secondary_clicked() { egui::PointerButton::Secondary } else { egui::PointerButton::Middle };
        let mods = ui.input(|i| i.modifiers);
        if let Some(c) = MouseChord::from_egui(mods, button) {
            *chord = Some(c);
        }
        captured = true;
    }
    if !captured && btn.clicked() {
        set_armed(ui, !armed);
    }
    if ui.add_sized(RESET_BUTTON, egui::Button::new(egui::RichText::new("Clear").small())).clicked() {
        *chord = None;
        set_armed(ui, false);
    }
    if ui
        .add_enabled(*chord != default, egui::Button::new(egui::RichText::new("Reset").small()).min_size(RESET_BUTTON))
        .clicked()
    {
        *chord = default;
        set_armed(ui, false);
    }
}

fn tablet(ui: &mut Ui, state: &mut AppState) {
    let t = &mut state.settings.tablet;
    ui.heading("Tablet & Pen");
    ui.add_space(6.0);
    egui::Grid::new("tablet_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Pressure curve");
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut t.pressure_gamma, 0.25..=4.0).logarithmic(true).fixed_decimals(2));
            ui.label(RichText::new(if t.pressure_gamma < 1.0 { "softer touch" } else if t.pressure_gamma > 1.0 { "firmer touch" } else { "linear" }).weak());
        });
        ui.end_row();
        ui.label("");
        pressure_curve_preview(ui, t.pressure_gamma, t.min_pressure);
        ui.end_row();
        ui.label("Minimum pressure (dead zone)");
        ui.add(egui::Slider::new(&mut t.min_pressure, 0.0..=0.5).fixed_decimals(2));
        ui.end_row();
        ui.label("Mouse pressure");
        ui.add(egui::Slider::new(&mut t.mouse_pressure, 0.1..=1.0).fixed_decimals(2));
        ui.end_row();
        ui.label("Tablet backend");
        ui.vertical(|ui| {
            ui.checkbox(&mut t.use_octotablet, "Use dedicated tablet API (Windows Ink RealTimeStylus / Wayland tablet)");
            ui.label(RichText::new("Off: pen pressure comes from the windowing system (Windows Ink pointer events). On: adds tilt and eraser-tip detection. Takes effect after restart.").weak().small());
            if cfg!(windows) {
                ui.checkbox(&mut t.use_wintab, "Use WinTab (Wacom driver API)");
                ui.label(RichText::new("For tablets with \"Use Windows Ink\" turned off in the driver: pressure, tilt, eraser and barrel buttons come straight from the driver. Ignored when the driver is not running. Takes effect after restart.").weak().small());
            }
        });
        ui.end_row();
        ui.label("Eraser tip selects the Eraser tool");
        ui.checkbox(&mut t.eraser_tip_switches_tool, "");
        ui.end_row();
    });
    ui.add_space(8.0);
    ui.label(
        RichText::new(format!(
            "Live: {}",
            match state.pen.pressure {
                Some(p) => format!("pen pressure {:.0}%", p * 100.0),
                None => "no pen contact".to_string(),
            }
        ))
        .weak(),
    );
    ui.label(
        RichText::new(
            "Wacom users: enable \"Use Windows Ink\" in the Wacom Tablet Properties for pressure to reach qsketch.",
        )
        .weak()
        .small(),
    );
}

fn pressure_curve_preview(ui: &mut Ui, gamma: f32, min: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(160.0, 90.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3, ui.visuals().extreme_bg_color);
    let n = 40;
    let pts: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let x = i as f32 / n as f32;
            let px = if x <= min { 0.0 } else { (x - min) / (1.0 - min) };
            let y = qsketch_core::brush::pressure_curve(px, gamma);
            egui::pos2(rect.left() + x * rect.width(), rect.bottom() - y * rect.height())
        })
        .collect();
    p.line_segment(
        [rect.left_bottom(), rect.right_top()],
        egui::Stroke::new(1.0, crate::ui::theme::dim_text(ui.visuals()).gamma_multiply(0.4)),
    );
    p.add(egui::Shape::line(pts, egui::Stroke::new(2.0, ui.visuals().selection.stroke.color)));
}

/// Fixed footprints for the shortcut editor, so nothing reflows while a
/// binding is being edited.
const SHORTCUT_BUTTON: egui::Vec2 = egui::vec2(96.0, 18.0);
const RESET_BUTTON: egui::Vec2 = egui::vec2(46.0, 18.0);
const STATUS_HEIGHT: f32 = 20.0;

/// A line of fixed height that holds a transient message.
fn status_line(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, STATUS_HEIGHT), egui::Sense::hover());
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    add(&mut child);
}

fn shortcuts(ui: &mut Ui, ctx: &Context, state: &mut AppState, dlg: &mut SettingsDialog) {
    ui.heading("Keyboard Shortcuts");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Search");
        ui.add(egui::TextEdit::singleline(&mut dlg.filter).desired_width(200.0).hint_text("action or key…"));
        if ui.button("Reset all").clicked() {
            state.keymap.reset_all();
            dlg.capturing = None;
            dlg.conflict = None;
        }
        ui.label(RichText::new("Click a shortcut to change it; Esc cancels, Backspace clears.").weak().small());
    });
    ui.separator();
    // One fixed-height line for whatever the page has to say, so the list
    // below never shifts as messages come and go.
    status_line(ui, |ui| match (dlg.capturing, &dlg.conflict) {
        (Some((action, _)), _) => {
            ui.colored_label(
                ui.visuals().selection.stroke.color,
                format!("Press the new shortcut for \"{}\"…  (Esc cancels, Backspace clears)", action.label()),
            );
        }
        (None, Some((sc, losers))) => {
            let names: Vec<&str> = losers.iter().map(|a| a.label()).collect();
            ui.colored_label(
                egui::Color32::from_rgb(235, 170, 60),
                format!("{} was taken from \"{}\".", sc.display(), names.join("\", \"")),
            );
        }
        _ => {}
    });

    // Key capture.
    if let Some((action, slot)) = dlg.capturing {
        let mut captured: Option<Shortcut> = None;
        let mut cancel = false;
        let mut clear = false;
        ctx.input(|i| {
            for ev in &i.events {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = ev {
                    match key {
                        // Bare modifier presses (egui 0.36 emits them as physical keys):
                        // wait for the actual key of the chord.
                        k if crate::actions::is_modifier_key(*k) => {}
                        egui::Key::Escape => cancel = true,
                        egui::Key::Backspace if modifiers.is_none() => clear = true,
                        _ => captured = Some(Shortcut::new(*modifiers, *key)),
                    }
                }
            }
        });
        if cancel {
            dlg.capturing = None;
        } else if clear {
            let mut v = state.keymap.shortcuts(action).to_vec();
            if let Some(s) = slot {
                if s < v.len() {
                    v.remove(s);
                }
            }
            state.keymap.set(action, v);
            dlg.capturing = None;
        } else if let Some(sc) = captured {
            // The key goes where the user just put it: whoever held it loses
            // it, rather than the assignment being refused.
            let losers = state.keymap.unbind_elsewhere(sc, action);
            assign(state, action, slot, sc);
            dlg.conflict = (!losers.is_empty()).then_some((sc, losers));
            dlg.capturing = None;
        }
        ctx.request_repaint();
    }

    let filter = dlg.filter.to_lowercase();
    let mut to_capture: Option<(Action, Option<usize>)> = None;
    let mut to_reset: Option<Action> = None;
    for cat in Category::ALL {
        let actions: Vec<Action> = Action::ALL
            .iter()
            .copied()
            .filter(|a| a.category() == cat)
            .filter(|a| {
                filter.is_empty()
                    || a.label().to_lowercase().contains(&filter)
                    || state.keymap.shortcuts(*a).iter().any(|s| s.display().to_lowercase().contains(&filter))
            })
            .collect();
        if actions.is_empty() {
            continue;
        }
        ui.add_space(6.0);
        ui.label(RichText::new(cat.label().to_uppercase()).small().weak().strong());
        egui::Grid::new(("sc_grid", cat.label()))
            .num_columns(3)
            .spacing([12.0, 3.0])
            .striped(true)
            .min_col_width(60.0)
            .show(ui, |ui| {
                for a in actions {
                    ui.label(a.label());
                    ui.horizontal(|ui| {
                        let scs = state.keymap.shortcuts(a).to_vec();
                        // Fixed sizes: a shortcut's text changes as it is
                        // edited, and the row must not resize under the
                        // pointer when it does.
                        for (i, sc) in scs.iter().enumerate() {
                            let capturing_this = dlg.capturing == Some((a, Some(i)));
                            let text = if capturing_this { "…".to_string() } else { sc.display() };
                            if ui
                                .add_sized(SHORTCUT_BUTTON, egui::Button::new(RichText::new(text).monospace().small()))
                                .clicked()
                            {
                                to_capture = Some((a, Some(i)));
                            }
                        }
                        let capturing_new = dlg.capturing == Some((a, None));
                        let plus = if capturing_new { "…" } else { "+" };
                        if ui
                            .add_sized(
                                egui::vec2(20.0, SHORTCUT_BUTTON.y),
                                egui::Button::new(RichText::new(plus).small()),
                            )
                            .on_hover_text("Add a shortcut")
                            .clicked()
                        {
                            to_capture = Some((a, None));
                        }
                    });
                    let is_default = {
                        let d = crate::actions::Keymap::default();
                        d.shortcuts(a) == state.keymap.shortcuts(a)
                    };
                    // Always drawn, disabled when there is nothing to undo, so
                    // the column keeps its width.
                    if ui
                        .add_enabled(
                            !is_default,
                            egui::Button::new(RichText::new("reset").small()).min_size(RESET_BUTTON),
                        )
                        .clicked()
                    {
                        to_reset = Some(a);
                    }
                    ui.end_row();
                }
            });
    }
    if let Some(c) = to_capture {
        dlg.capturing = Some(c);
        dlg.conflict = None;
    }
    if let Some(a) = to_reset {
        state.keymap.reset(a);
    }
}

fn assign(state: &mut AppState, action: Action, slot: Option<usize>, sc: Shortcut) {
    let mut v = state.keymap.shortcuts(action).to_vec();
    match slot {
        Some(i) if i < v.len() => v[i] = sc,
        _ => {
            if !v.contains(&sc) {
                v.push(sc);
            }
        }
    }
    state.keymap.set(action, v);
}
