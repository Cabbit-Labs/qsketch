//! Animation dialogs: frame, tag and cel properties, sprite-sheet import and
//! the Export Animation dialog.

use std::sync::Arc;

use egui::{Context, Ui};
use qsketch_core::anim::{TagDirection, MAX_DURATION_MS, MIN_DURATION_MS, TAG_COLORS};
use qsketch_core::io::anim_io::{self, SheetLayout, SheetOptions};
use qsketch_core::{Raster, Rgba8, Tag};

use super::modal;
use crate::anim::{self, ExportFormat, ExportRange, ExportSpec};
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;
use crate::ui::widgets::rgba_to_color32;

pub struct FramePropsDialog {
    pub doc: DocId,
    pub frames: Vec<usize>,
    pub duration_ms: u32,
}

pub struct TagPropsDialog {
    pub doc: DocId,
    /// `None` while the tag is new (not in the document yet).
    pub index: Option<usize>,
    pub tag: Tag,
}

pub struct CelPropsDialog {
    pub doc: DocId,
    pub layer: qsketch_core::LayerId,
    pub frame: usize,
    pub opacity: f32,
    pub z_index: i16,
}

pub struct ImportSheetDialog {
    pub name: String,
    pub sheet: Arc<Raster>,
    pub cell_w: u32,
    pub cell_h: u32,
    /// 0 = every cell.
    pub count: usize,
    pub duration_ms: u32,
}

pub struct ExportAnimDialog {
    pub doc: DocId,
    pub format: ExportFormat,
    pub scale: u32,
    pub range: ExportRange,
    pub repeat: bool,
    pub sheet: SheetOptions,
    pub video_bg: [u8; 3],
    pub ffmpeg: bool,
}

/// What Export Animation remembers between uses (in the settings).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ExportPrefs {
    pub format: ExportFormat,
    pub scale: u32,
    pub repeat: bool,
    pub sheet: SheetOptions,
    pub video_bg: [u8; 3],
}

impl Default for ExportPrefs {
    fn default() -> Self {
        Self { format: ExportFormat::Gif, scale: 1, repeat: true, sheet: SheetOptions::default(), video_bg: [0, 0, 0] }
    }
}

pub fn open_export(state: &mut AppState, doc: DocId) {
    let p = state.settings.anim.export.clone();
    state.dialogs.export_anim = Some(ExportAnimDialog {
        doc,
        format: p.format,
        scale: p.scale.clamp(1, 16),
        range: ExportRange::All,
        repeat: p.repeat,
        sheet: p.sheet,
        video_bg: p.video_bg,
        ffmpeg: anim_io::ffmpeg_path().is_some(),
    });
}

pub fn show(ctx: &Context, state: &mut AppState) {
    show_frame_props(ctx, state);
    show_tag_props(ctx, state);
    show_cel_props(ctx, state);
    show_import_sheet(ctx, state);
    show_export(ctx, state);
}

fn ok_cancel(ui: &mut Ui, ok_label: &str) -> (bool, bool) {
    let (mut ok, mut cancel) = (false, false);
    ui.add_space(10.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.button(ok_label).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            ok = true;
        }
        if ui.button("Cancel").clicked() {
            cancel = true;
        }
    });
    (ok, cancel)
}

fn fps_text(ms: u32) -> String {
    let fps = 1000.0 / ms.max(1) as f32;
    if (fps - fps.round()).abs() < 0.05 {
        format!("{} fps", fps.round() as u32)
    } else {
        format!("{fps:.1} fps")
    }
}

// --- frame properties ------------------------------------------------------------

fn show_frame_props(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.frame_props.as_mut() else { return };
    let n = d.frames.len();
    let title = if n > 1 { format!("Frame Properties ({n} frames)") } else { "Frame Properties".to_string() };
    let ((ok, cancel), closed) = modal(ctx, "frame_props", &title, 300.0, |ui| {
        egui::Grid::new("frame_props_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Duration");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut d.duration_ms).range(MIN_DURATION_MS..=MAX_DURATION_MS).suffix(" ms"));
                ui.label(egui::RichText::new(fps_text(d.duration_ms)).weak());
            });
            ui.end_row();
            ui.label("Presets");
            ui.horizontal(|ui| {
                for (label, ms) in [("12 fps", 83), ("24 fps", 42), ("30 fps", 33), ("60 fps", 17)] {
                    if ui.small_button(label).clicked() {
                        d.duration_ms = ms;
                    }
                }
            });
            ui.end_row();
        });
        ok_cancel(ui, "OK")
    });
    if ok {
        let d = state.dialogs.frame_props.take().unwrap();
        anim::set_duration(state, d.doc, &d.frames, d.duration_ms);
    } else if cancel || closed {
        state.dialogs.frame_props = None;
    }
}

// --- tag properties ----------------------------------------------------------------

fn show_tag_props(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.tag_props.as_mut() else { return };
    let nframes = state.docs.iter().find(|e| e.id == d.doc).map_or(1, |e| e.doc.state().frame_count());
    let is_new = d.index.is_none();
    let mut delete = false;
    let ((ok, cancel), closed) =
        modal(ctx, "tag_props", if is_new { "New Tag" } else { "Tag Properties" }, 340.0, |ui| {
            let t = &mut d.tag;
            egui::Grid::new("tag_props_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Name");
                let r = ui.text_edit_singleline(&mut t.name);
                if is_new {
                    r.request_focus();
                }
                ui.end_row();
                ui.label("Frames");
                ui.horizontal(|ui| {
                    let (mut from, mut to) = (t.from + 1, t.to + 1);
                    ui.add(egui::DragValue::new(&mut from).range(1..=nframes).prefix("from "));
                    ui.add(egui::DragValue::new(&mut to).range(1..=nframes).prefix("to "));
                    t.from = from - 1;
                    t.to = to - 1;
                    if t.from > t.to {
                        std::mem::swap(&mut t.from, &mut t.to);
                    }
                });
                ui.end_row();
                ui.label("Direction");
                egui::ComboBox::from_id_salt("tag_dir").selected_text(t.direction.label()).show_ui(ui, |ui| {
                    for dir in TagDirection::ALL {
                        ui.selectable_value(&mut t.direction, dir, dir.label());
                    }
                });
                ui.end_row();
                ui.label("Repeat");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut t.repeat).range(0..=u16::MAX));
                    ui.label(egui::RichText::new(if t.repeat == 0 { "times (0 = forever)" } else { "times" }).weak());
                });
                ui.end_row();
                ui.label("Color");
                ui.horizontal(|ui| {
                    for c in TAG_COLORS {
                        let (rect, r) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
                        ui.painter().rect_filled(rect, 3.0, rgba_to_color32(c));
                        if t.color == c {
                            ui.painter().rect_stroke(
                                rect,
                                3.0,
                                egui::Stroke::new(2.0, ui.visuals().text_color()),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if r.clicked() {
                            t.color = c;
                        }
                    }
                    let mut c32 = egui::Color32::from_rgb(t.color.r, t.color.g, t.color.b);
                    if egui::color_picker::color_edit_button_srgba(ui, &mut c32, egui::color_picker::Alpha::Opaque)
                        .changed()
                    {
                        t.color = Rgba8::new(c32.r(), c32.g(), c32.b(), 255);
                    }
                });
                ui.end_row();
            });
            ui.add_space(10.0);
            let (mut ok, mut cancel) = (false, false);
            ui.horizontal(|ui| {
                if !is_new && ui.button("Delete Tag").clicked() {
                    delete = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        ok = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
            (ok, cancel)
        });
    if ok || delete {
        let d = state.dialogs.tag_props.take().unwrap();
        if delete {
            anim::delete_tag(state, d.doc, d.index);
            return;
        }
        let mut tag = d.tag;
        if tag.name.trim().is_empty() {
            tag.name = "Tag".into();
        }
        state.settle();
        anim::stop(state);
        if let Some(e) = state.doc_mut(d.doc) {
            let s = e.doc.state_mut();
            let n = s.frame_count();
            tag.from = tag.from.min(n - 1);
            tag.to = tag.to.min(n - 1);
            match d.index {
                Some(i) if i < s.tags.len() => {
                    if s.tags[i] != tag {
                        s.tags[i] = tag;
                        e.doc.commit("Tag Properties");
                    }
                }
                _ => {
                    s.tags.push(tag);
                    e.doc.commit("New Tag");
                }
            }
        }
    } else if cancel || closed {
        state.dialogs.tag_props = None;
    }
}

// --- cel properties ----------------------------------------------------------------

fn show_cel_props(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.cel_props.as_mut() else { return };
    let ((ok, cancel), closed) = modal(ctx, "cel_props", "Cel Properties", 300.0, |ui| {
        egui::Grid::new("cel_props_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Opacity");
            let mut pct = d.opacity * 100.0;
            if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%")).changed() {
                d.opacity = pct / 100.0;
            }
            ui.end_row();
            ui.label("Z-index");
            ui.add(egui::DragValue::new(&mut d.z_index))
                .on_hover_text("Aseprite's per-cel draw order nudge; kept for Aseprite, not drawn here");
            ui.end_row();
        });
        ui.label(egui::RichText::new("Linked cels share these settings.").weak().small());
        ok_cancel(ui, "OK")
    });
    if ok {
        let d = state.dialogs.cel_props.take().unwrap();
        state.settle();
        anim::stop(state);
        if let Some(e) = state.doc_mut(d.doc) {
            let s = e.doc.state_mut();
            s.sync_cels();
            let changed = s.index_of(d.layer).and_then(|li| s.layers[li].cel_mut(d.frame)).is_some_and(|c| {
                let ch = (c.opacity - d.opacity).abs() > 1e-4 || c.z_index != d.z_index;
                c.opacity = d.opacity.clamp(0.0, 1.0);
                c.z_index = d.z_index;
                ch
            });
            if changed {
                e.doc.mark_all_dirty();
                e.doc.commit("Cel Properties");
            }
        }
    } else if cancel || closed {
        state.dialogs.cel_props = None;
    }
}

// --- import sprite sheet -----------------------------------------------------------------

fn show_import_sheet(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.import_sheet.as_mut() else { return };
    let (sw, sh) = (d.sheet.width(), d.sheet.height());
    let ((ok, cancel), closed) = modal(ctx, "import_sheet", "Import Sprite Sheet", 420.0, |ui| {
        ui.label(egui::RichText::new(format!("{} · {sw}×{sh} px", d.name)).weak());
        ui.add_space(4.0);
        egui::Grid::new("import_sheet_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Frame size");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut d.cell_w).range(1..=sw).prefix("w "));
                ui.add(egui::DragValue::new(&mut d.cell_h).range(1..=sh).prefix("h "));
                for (label, w, h) in [("8", 8, 8), ("16", 16, 16), ("32", 32, 32), ("64", 64, 64)] {
                    if w <= sw && h <= sh && ui.small_button(label).clicked() {
                        d.cell_w = w;
                        d.cell_h = h;
                    }
                }
            });
            ui.end_row();
            ui.label("Frames");
            let (cols, rows) = (sw / d.cell_w.max(1), sh / d.cell_h.max(1));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut d.count).range(0..=(cols * rows) as usize));
                ui.label(
                    egui::RichText::new(format!("(0 = all; {cols} × {rows} = {} cells, left to right)", cols * rows))
                        .weak(),
                );
            });
            ui.end_row();
            ui.label("Duration");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut d.duration_ms).range(MIN_DURATION_MS..=MAX_DURATION_MS).suffix(" ms"));
                ui.label(egui::RichText::new(fps_text(d.duration_ms)).weak());
            });
            ui.end_row();
        });
        // A preview with the cut lines.
        let avail = 380.0_f32;
        let scale = (avail / sw as f32).min(160.0 / sh as f32).min(8.0);
        let size = egui::vec2(sw as f32 * scale, sh as f32 * scale);
        let tex_id = ui.id().with("sheet_tex");
        let tex: egui::TextureHandle = match ui.data(|m| m.get_temp::<egui::TextureHandle>(tex_id)) {
            Some(t) => t,
            None => {
                let img = egui::ColorImage::from_rgba_unmultiplied([sw as usize, sh as usize], &d.sheet.to_rgba());
                let t = ui.ctx().load_texture("import_sheet", img, egui::TextureOptions::NEAREST);
                ui.data_mut(|m| m.insert_temp(tex_id, t.clone()));
                t
            }
        };
        ui.add_space(6.0);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        crate::ui::widgets::checkerboard(ui.painter(), rect, 8.0);
        ui.painter().image(
            tex.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 170, 0, 200));
        let (cw, ch) = (d.cell_w.max(1) as f32 * scale, d.cell_h.max(1) as f32 * scale);
        let mut x = rect.left();
        while x <= rect.right() + 0.5 {
            ui.painter().line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], stroke);
            x += cw;
        }
        let mut y = rect.top();
        while y <= rect.bottom() + 0.5 {
            ui.painter().line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], stroke);
            y += ch;
        }
        ok_cancel(ui, "Import")
    });
    if ok {
        let d = state.dialogs.import_sheet.take().unwrap();
        let doc = anim_io::from_sheet(&d.name, &d.sheet, d.cell_w, d.cell_h, d.count, d.duration_ms);
        let n = doc.frame_count();
        let title = state.untitled_title();
        let document = qsketch_core::Document::from_state(doc, title, None, "Import Sprite Sheet");
        state.add_document(document);
        state.show_panel_requests.push(crate::workspace::PanelKind::Timeline);
        state.toasts.push(Level::Success, format!("Imported {n} frames from the sheet."));
    } else if cancel || closed {
        state.dialogs.import_sheet = None;
    }
}

// --- export ------------------------------------------------------------------------------

fn show_export(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.export_anim.as_mut() else { return };
    let Some(entry) = state.docs.iter().find(|e| e.id == d.doc) else {
        state.dialogs.export_anim = None;
        return;
    };
    let s = entry.doc.state();
    let (w, h, n) = (s.width, s.height, s.frame_count());
    let [ax, ay] = if s.pixel_aspect.contains(&0) { [1, 1] } else { s.pixel_aspect };
    let tags: Vec<(usize, String, usize)> =
        s.tags.iter().enumerate().map(|(i, t)| (i, t.name.clone(), t.len())).collect();
    let sel = entry.frame_sel.map(|(a, b)| (a.min(b), a.max(b)));
    let ((ok, cancel), closed) = modal(ctx, "export_anim", "Export Animation", 400.0, |ui| {
        egui::Grid::new("export_anim_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Format");
            egui::ComboBox::from_id_salt("export_fmt").selected_text(d.format.label()).width(240.0).show_ui(ui, |ui| {
                for f in ExportFormat::ALL {
                    ui.selectable_value(&mut d.format, f, f.label());
                }
            });
            ui.end_row();
            ui.label("Scale");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut d.scale).range(1..=16).suffix("×"));
                let (ow, oh) = (w * d.scale * ax as u32, h * d.scale * ay as u32);
                ui.label(egui::RichText::new(format!("{ow}×{oh} px per frame, nearest neighbor")).weak());
            });
            ui.end_row();
            ui.label("Frames");
            ui.vertical(|ui| {
                ui.radio_value(&mut d.range, ExportRange::All, format!("All ({n})"));
                if let Some((a, b)) = sel {
                    ui.radio_value(&mut d.range, ExportRange::Selected, format!("Selected ({}–{})", a + 1, b + 1));
                }
                if !tags.is_empty() {
                    let tag_i = match d.range {
                        ExportRange::Tag(i) => Some(i),
                        _ => None,
                    };
                    ui.horizontal(|ui| {
                        let mut is_tag = tag_i.is_some();
                        if ui.radio(is_tag, "Tag").clicked() {
                            is_tag = true;
                            d.range = ExportRange::Tag(tag_i.unwrap_or(0));
                        }
                        let current = tag_i.and_then(|i| tags.get(i)).map(|t| t.1.clone()).unwrap_or_default();
                        ui.add_enabled_ui(is_tag, |ui| {
                            egui::ComboBox::from_id_salt("export_tag").selected_text(current).show_ui(ui, |ui| {
                                for (i, name, len) in &tags {
                                    if ui.selectable_label(tag_i == Some(*i), format!("{name} ({len})")).clicked() {
                                        d.range = ExportRange::Tag(*i);
                                    }
                                }
                            });
                        });
                    });
                }
            });
            ui.end_row();
            match d.format {
                ExportFormat::Gif | ExportFormat::Apng => {
                    ui.label("Loop");
                    ui.checkbox(&mut d.repeat, "Play forever");
                    ui.end_row();
                }
                ExportFormat::Sheet => {
                    ui.label("Layout");
                    egui::ComboBox::from_id_salt("sheet_layout").selected_text(d.sheet.layout.label()).show_ui(
                        ui,
                        |ui| {
                            for l in SheetLayout::ALL {
                                ui.selectable_value(&mut d.sheet.layout, l, l.label());
                            }
                        },
                    );
                    ui.end_row();
                    if d.sheet.layout == SheetLayout::Grid {
                        ui.label("Columns");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut d.sheet.columns).range(0..=4096));
                            ui.label(egui::RichText::new("(0 = as square as possible)").weak());
                        });
                        ui.end_row();
                    }
                    ui.label("Spacing");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut d.sheet.padding).range(0..=256).prefix("gap "));
                        ui.add(egui::DragValue::new(&mut d.sheet.border).range(0..=256).prefix("border "));
                    });
                    ui.end_row();
                    ui.label("Data");
                    ui.checkbox(&mut d.sheet.json, "Write a .json next to it (Aseprite layout: frames, tags, slices)");
                    ui.end_row();
                }
                ExportFormat::Mp4 | ExportFormat::WebM => {
                    ui.label("Background");
                    let mut c32 = egui::Color32::from_rgb(d.video_bg[0], d.video_bg[1], d.video_bg[2]);
                    if egui::color_picker::color_edit_button_srgba(ui, &mut c32, egui::color_picker::Alpha::Opaque)
                        .changed()
                    {
                        d.video_bg = [c32.r(), c32.g(), c32.b()];
                    }
                    ui.end_row();
                }
                ExportFormat::Sequence => {}
            }
        });
        if d.format.needs_ffmpeg() && !d.ffmpeg {
            ui.add_space(6.0);
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "ffmpeg wasn't found on the PATH. Install it (ffmpeg.org) to export video.",
            );
        }
        ok_cancel(ui, "Export…")
    });
    if ok {
        let d = state.dialogs.export_anim.take().unwrap();
        if d.format.needs_ffmpeg() && !d.ffmpeg {
            state.toasts.push(Level::Error, "ffmpeg is needed for video export.");
            return;
        }
        state.settings.anim.export = ExportPrefs {
            format: d.format,
            scale: d.scale,
            repeat: d.repeat,
            sheet: d.sheet.clone(),
            video_bg: d.video_bg,
        };
        let Some(entry) = state.doc(d.doc) else { return };
        let s = entry.doc.state();
        let frames = match d.range {
            ExportRange::All => anim_io::frame_list(s, None),
            ExportRange::Tag(i) => anim_io::frame_list(s, Some(i)),
            ExportRange::Selected => anim::selected_frames(state, d.doc),
        };
        let base = entry.doc.title.trim_end_matches('*').to_string();
        let stem = std::path::Path::new(&base).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(base);
        let dir = entry.doc.path.as_ref().and_then(|p| p.parent()).filter(|d| d.exists()).map(|d| d.to_path_buf());
        let suffix = match d.range {
            ExportRange::Tag(i) => s.tags.get(i).map(|t| format!("-{}", t.name)).unwrap_or_default(),
            _ => String::new(),
        };
        let target = if d.format == ExportFormat::Sequence {
            let mut dlg = rfd::FileDialog::new().set_title("Export PNG Sequence To Folder");
            if let Some(d) = &dir {
                dlg = dlg.set_directory(d);
            }
            dlg.pick_folder().map(|f| f.join(format!("{stem}{suffix}")))
        } else {
            let (name, exts): (&str, &[&str]) = match d.format {
                ExportFormat::Gif => ("Animated GIF", &["gif"]),
                ExportFormat::Apng => ("Animated PNG", &["png", "apng"]),
                ExportFormat::Sheet => ("PNG sprite sheet", &["png"]),
                ExportFormat::Mp4 => ("MP4 video", &["mp4"]),
                ExportFormat::WebM => ("WebM video", &["webm"]),
                ExportFormat::Sequence => ("PNG", &["png"]),
            };
            let sheet = if d.format == ExportFormat::Sheet { "-sheet" } else { "" };
            let mut dlg = rfd::FileDialog::new()
                .set_title("Export Animation")
                .add_filter(name, exts)
                .set_file_name(format!("{stem}{suffix}{sheet}.{}", d.format.extension()));
            if let Some(d) = &dir {
                dlg = dlg.set_directory(d);
            }
            dlg.save_file().map(|mut p| {
                if p.extension().is_none() {
                    p.set_extension(d.format.extension());
                }
                p
            })
        };
        let Some(target) = target else { return };
        let spec = ExportSpec {
            format: d.format,
            scale: d.scale,
            frames,
            repeat: d.repeat,
            sheet: d.sheet,
            video_bg: Rgba8::new(d.video_bg[0], d.video_bg[1], d.video_bg[2], 255),
            target,
            stem: format!("{stem}{suffix}"),
        };
        anim::run_export(state, d.doc, spec);
    } else if cancel || closed {
        state.dialogs.export_anim = None;
    }
}
