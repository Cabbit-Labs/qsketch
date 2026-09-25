//! Modal dialogs: new document, canvas/image size, adjustments, filters,
//! layer properties, unsaved-changes confirmation, about, and preferences.

pub mod filter;
pub mod liquify;
pub mod settings;
pub mod share;

use egui::{Context, RichText, Ui};
use qsketch_core::ops::{self, Anchor};
use qsketch_core::raster::ResizeFilter;
use qsketch_core::{Document, Rgba8};

use crate::settings::NewDocBackground;
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorTarget {
    Foreground,
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterClose {
    Nothing,
    Quit,
}

pub struct NewDocDialog {
    pub width: u32,
    pub height: u32,
    pub background: NewDocBackground,
    pub name: String,
    /// Dimensions were taken from an image on the clipboard.
    pub from_clipboard: bool,
}

pub struct CanvasSizeDialog {
    pub doc: DocId,
    pub width: u32,
    pub height: u32,
    pub anchor: Anchor,
}

pub struct ModifyDialog {
    pub doc: DocId,
    pub kind: crate::tools::ModifyKind,
    pub amount: f32,
}

pub struct ImageSizeDialog {
    pub doc: DocId,
    pub width: u32,
    pub height: u32,
    pub orig_w: u32,
    pub orig_h: u32,
    pub keep_aspect: bool,
    pub filter: ResizeFilter,
}

pub struct LayerPropsDialog {
    pub doc: DocId,
    /// By id: the dialog is modeless enough that the layer list can change
    /// under it, and an index would then name a different layer.
    pub layer: qsketch_core::layer::LayerId,
    pub name: String,
}

pub struct CloseConfirm {
    pub doc: DocId,
    pub then: AfterClose,
}

/// "This format can't hold everything": shown before a Save / Save As to a
/// flat image, PSD or Aseprite file that would drop something.
pub struct SaveConfirm {
    pub doc: DocId,
    pub path: std::path::PathBuf,
    pub warnings: Vec<String>,
    /// Continue with a close / quit once the file is written.
    pub then: Option<AfterClose>,
}

#[derive(Default)]
pub struct Dialogs {
    pub color_target: Option<ColorTarget>,
    pub new_doc: Option<NewDocDialog>,
    pub canvas_size: Option<CanvasSizeDialog>,
    pub modify: Option<ModifyDialog>,
    pub image_size: Option<ImageSizeDialog>,
    pub filter: Option<filter::FilterDialog>,
    pub layer_props: Option<LayerPropsDialog>,
    pub close_confirm: Option<CloseConfirm>,
    pub save_confirm: Option<SaveConfirm>,
    pub settings: Option<settings::SettingsDialog>,
    pub share: Option<share::ShareDialog>,
    pub liquify: Option<liquify::LiquifyDialog>,
    pub about: bool,
    /// Autosave snapshots found at startup, offered for recovery.
    pub recover: Option<Vec<crate::autosave::Recoverable>>,
}

impl Dialogs {
    pub fn any_open(&self) -> bool {
        self.new_doc.is_some()
            || self.canvas_size.is_some()
            || self.modify.is_some()
            || self.image_size.is_some()
            || self.filter.is_some()
            || self.layer_props.is_some()
            || self.close_confirm.is_some()
            || self.save_confirm.is_some()
            || self.settings.is_some()
            || self.share.is_some()
            || self.about
            || self.recover.is_some()
    }
}

/// A bulleted line whose wrapped text hangs under itself, not under the dot.
fn bullet(ui: &mut Ui, text: &str) {
    ui.horizontal_top(|ui| {
        ui.add_space(2.0);
        ui.label(RichText::new("•").weak());
        ui.add_space(4.0);
        let w = ui.available_width();
        ui.allocate_ui_with_layout(egui::vec2(w, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.label(text);
        });
    });
}

pub(super) fn modal<R>(ctx: &Context, id: &str, title: &str, width: f32, add: impl FnOnce(&mut Ui) -> R) -> (R, bool) {
    let mut closed = false;
    let resp = egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        ui.set_width(width);
        ui.heading(title);
        ui.add_space(6.0);

        add(ui)
    });
    if resp.should_close() {
        closed = true;
    }
    (resp.inner, closed)
}

/// Show every open dialog. Called once per frame after the main UI.
pub fn show_all(ctx: &Context, state: &mut AppState) {
    show_new_doc(ctx, state);
    show_canvas_size(ctx, state);
    show_modify(ctx, state);
    show_image_size(ctx, state);
    filter::show(ctx, state);
    show_layer_props(ctx, state);
    show_close_confirm(ctx, state);
    show_save_confirm(ctx, state);
    settings::show(ctx, state);
    share::show(ctx, state);
    liquify::show(ctx, state);
    show_about(ctx, state);
    show_update(ctx, state);
    show_recover(ctx, state);
}

/// Offer autosave snapshots left by a crashed or killed session.
fn show_recover(ctx: &Context, state: &mut AppState) {
    let Some(items) = state.dialogs.recover.clone() else { return };
    let mut recover_all = false;
    let mut discard_all = false;
    let mut single: Option<(usize, bool)> = None;
    let (_, closed) = modal(ctx, "recover", "Recover Unsaved Work", 460.0, |ui| {
        ui.label(format!(
            "qsketch didn't close normally last time. {} autosaved {} found:",
            items.len(),
            if items.len() == 1 { "document was" } else { "documents were" }
        ));
        ui.add_space(8.0);
        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
            for (i, r) in items.iter().enumerate() {
                ui.horizontal(|ui| {
                    let when = if r.meta.written > 0 {
                        let secs = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0)
                            .saturating_sub(r.meta.written);
                        if secs < 60 {
                            "just now".to_string()
                        } else if secs < 3600 {
                            format!("{} min ago", secs / 60)
                        } else if secs < 86400 {
                            format!("{} h ago", secs / 3600)
                        } else {
                            format!("{} d ago", secs / 86400)
                        }
                    } else {
                        String::new()
                    };
                    ui.label(RichText::new(&r.meta.title).strong());
                    if let Some(p) = &r.meta.path {
                        ui.label(RichText::new(p.display().to_string()).weak());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::ui::widgets::small_button(ui, "Discard").clicked() {
                            single = Some((i, false));
                        }
                        if crate::ui::widgets::small_button(ui, "Recover").clicked() {
                            single = Some((i, true));
                        }
                        ui.label(RichText::new(when).weak());
                    });
                });
            }
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("Discard All").clicked() {
                discard_all = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Recover All").strong())).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    recover_all = true;
                }
            });
        });
    });
    if closed && !recover_all && !discard_all && single.is_none() {
        // Esc / click-outside keeps the snapshots for next time.
        state.dialogs.recover = None;
        return;
    }
    let mut remaining = items;
    if recover_all || discard_all {
        for r in &remaining {
            if recover_all {
                crate::autosave::recover(state, r);
            } else {
                crate::autosave::discard(r);
            }
        }
        remaining.clear();
    } else if let Some((i, yes)) = single {
        let r = remaining.remove(i);
        if yes {
            crate::autosave::recover(state, &r);
        } else {
            crate::autosave::discard(&r);
        }
    }
    state.dialogs.recover = if remaining.is_empty() { None } else { Some(remaining) };
}

/// Update dialog: available / downloading / ready / failed / up to date.
fn show_update(ctx: &Context, state: &mut AppState) {
    if !state.updater.show_dialog {
        return;
    }
    use crate::update::Status;
    let status = state.updater.status.clone();
    let info = state.updater.info.clone();
    let mut action: Option<&str> = None;
    let title = match &status {
        Status::UpToDate => "qsketch is up to date",
        Status::Failed(_) => "Update problem",
        Status::Ready(_) => "Update ready to install",
        _ => "Update available",
    };
    let (_, closed) = modal(ctx, "update", title, 520.0, |ui| {
        match &status {
            Status::UpToDate => {
                ui.label(format!("You are running the latest version ({}).", crate::update::CURRENT_VERSION));
            }
            Status::Failed(e) => {
                ui.label(RichText::new(e).color(egui::Color32::from_rgb(235, 120, 100)));
                if info.is_some() {
                    ui.add_space(4.0);
                    ui.label(RichText::new("You can retry, or download the release manually later.").weak());
                }
            }
            _ => {
                if let Some(i) = &info {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("qsketch {}", i.version)).strong().size(16.0));
                        ui.label(
                            RichText::new(format!("· you have {}", crate::update::CURRENT_VERSION)).weak().size(13.0),
                        );
                    });
                    let blocks = crate::update::notes_blocks(&i.notes);
                    if !blocks.is_empty() || !i.history.is_empty() {
                        ui.add_space(10.0);
                        let title = if i.history.len() > 1 {
                            format!(
                                "WHAT'S NEW · {} releases since {}",
                                i.history.len(),
                                crate::update::CURRENT_VERSION
                            )
                        } else {
                            "WHAT'S NEW".to_string()
                        };
                        ui.label(RichText::new(title).weak().small().strong());
                        ui.add_space(4.0);
                        // Grows with the window, so a long changelog is
                        // readable instead of clipped after a few lines.
                        let max_h = (ctx.content_rect().height() * 0.45).clamp(140.0, 420.0);
                        egui::Frame::new()
                            .fill(ui.visuals().extreme_bg_color)
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .corner_radius(4)
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(
                                    ui,
                                    |ui| {
                                        if i.history.is_empty() {
                                            for (n, block) in blocks.iter().enumerate() {
                                                if n > 0 {
                                                    ui.add_space(6.0);
                                                }
                                                bullet(ui, block);
                                            }
                                            return;
                                        }
                                        // Several versions at once: one
                                        // section per release, newest first.
                                        for (k, rel) in i.history.iter().enumerate() {
                                            if k > 0 {
                                                ui.add_space(10.0);
                                            }
                                            ui.horizontal(|ui| {
                                                ui.label(RichText::new(&rel.version).strong());
                                                if !rel.date.is_empty() {
                                                    ui.label(RichText::new(&rel.date).weak().small());
                                                }
                                            });
                                            ui.add_space(2.0);
                                            for (n, block) in crate::update::notes_blocks(&rel.notes).iter().enumerate()
                                            {
                                                if n > 0 {
                                                    ui.add_space(6.0);
                                                }
                                                bullet(ui, block);
                                            }
                                        }
                                    },
                                );
                            });
                    }
                }
                match &status {
                    Status::Downloading { downloaded, total } => {
                        ui.add_space(6.0);
                        let frac = total.filter(|t| *t > 0).map(|t| *downloaded as f32 / t as f32);
                        let mb = |b: u64| b as f32 / 1_048_576.0;
                        let text = match total {
                            Some(t) => format!("{:.1} / {:.1} MB", mb(*downloaded), mb(*t)),
                            None => format!("{:.1} MB", mb(*downloaded)),
                        };
                        ui.add(egui::ProgressBar::new(frac.unwrap_or(0.0)).animate(frac.is_none()).text(text));
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    Status::Available => {
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(
                                "Update and Restart downloads the release, verifies its signature, then closes qsketch, installs and reopens it. Save your work first.",
                            )
                            .weak()
                            .small(),
                        );
                    }
                    Status::Ready(_) => {
                        ui.add_space(6.0);
                        ui.label("Downloaded and verified. Installing and restarting…");
                        if cfg!(target_os = "linux") {
                            ui.label(
                                RichText::new("The running executable is replaced in place; installs owned by root need to be updated by hand.")
                                    .weak()
                                    .small(),
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match &status {
            Status::Available => {
                if ui.button(RichText::new("Update and Restart").strong()).clicked() {
                    action = Some("download");
                }
                if ui.button("Later").clicked() {
                    action = Some("later");
                }
                if ui.button("Skip this version").clicked() {
                    action = Some("skip");
                }
            }
            Status::Downloading { .. } => {
                ui.add_enabled(false, egui::Button::new("Updating…"));
            }
            Status::Ready(_) => {
                if ui.button(RichText::new("Install and Restart").strong()).clicked() {
                    action = Some("install");
                }
                if ui.button("Later").clicked() {
                    action = Some("later");
                }
            }
            Status::Failed(_) if info.is_some() => {
                if ui.button("Retry").clicked() {
                    action = Some("download");
                }
                if ui.button("Close").clicked() {
                    action = Some("later");
                }
            }
            _ => {
                if ui.button("Close").clicked() {
                    action = Some("later");
                }
            }
        });
    });
    // One click: once the download verifies, install without asking again.
    if state.updater.auto_install {
        if let Status::Ready(p) = &status {
            state.updater.auto_install = false;
            if let Err(e) = install_and_reopen(state, p) {
                state.updater.status = Status::Failed(e);
            }
        }
    }
    match action {
        Some("download") => {
            state.updater.auto_install = true;
            state.updater.download();
        }
        Some("skip") => {
            if let Some(i) = &info {
                state.settings.update.skipped_version = i.version.clone();
            }
            state.updater.show_dialog = false;
        }
        Some("later") => {
            state.updater.auto_install = false;
            state.updater.show_dialog = false;
        }
        Some("install") => {
            if let Status::Ready(p) = &status {
                if let Err(e) = install_and_reopen(state, p) {
                    state.updater.status = Status::Failed(e);
                }
            }
        }
        _ => {}
    }
    if closed {
        state.updater.show_dialog = false;
    }
}

pub fn open_new_doc(state: &mut AppState) {
    let g = &state.settings.general;
    // Like Photoshop: an image on the clipboard sets the default size.
    let clip = crate::clipboard::os_image_size().filter(|&(w, h)| w <= 16384 && h <= 16384);
    let (width, height) = clip.unwrap_or((g.new_doc_width, g.new_doc_height));
    state.dialogs.new_doc = Some(NewDocDialog {
        width,
        height,
        background: g.new_doc_background,
        name: state.untitled_title(),
        from_clipboard: clip.is_some(),
    });
}

fn show_new_doc(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.new_doc.as_mut() else { return };
    let mut create = false;
    let (_, closed) = modal(ctx, "new_doc", "New Document", 340.0, |ui| {
        egui::Grid::new("new_doc_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut d.name);
            ui.end_row();
            ui.label("Width");
            ui.add(egui::DragValue::new(&mut d.width).range(1..=16384).suffix(" px"));
            ui.end_row();
            ui.label("Height");
            ui.add(egui::DragValue::new(&mut d.height).range(1..=16384).suffix(" px"));
            ui.end_row();
            if d.from_clipboard {
                ui.label("");
                ui.weak("Size taken from the clipboard image");
                ui.end_row();
            }
            ui.label("Background");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut d.background, NewDocBackground::White, "White");
                ui.selectable_value(&mut d.background, NewDocBackground::Transparent, "Transparent");
                ui.selectable_value(&mut d.background, NewDocBackground::BackgroundColor, "Background color");
            });
            ui.end_row();
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Presets").weak().small());
        ui.horizontal_wrapped(|ui| {
            for (label, w, h) in [
                ("1920×1080", 1920, 1080),
                ("1080×1920", 1080, 1920),
                ("2048×2048", 2048, 2048),
                ("A4 300dpi", 2480, 3508),
                ("4K", 3840, 2160),
                ("Pixel 64", 64, 64),
                ("Pixel 256", 256, 256),
            ] {
                if crate::ui::widgets::small_button(ui, label).clicked() {
                    d.width = w;
                    d.height = h;
                }
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Create").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    create = true;
                }
                if ui.button("Cancel").clicked() {
                    ui.close();
                }
            });
        });
    });
    if create {
        let d = state.dialogs.new_doc.take().unwrap();
        let bg = match d.background {
            NewDocBackground::White => Some(Rgba8::WHITE),
            NewDocBackground::Transparent => None,
            NewDocBackground::BackgroundColor => Some(state.bg),
        };
        let name = if d.name.trim().is_empty() { state.untitled_title() } else { d.name.trim().to_string() };
        state.settings.general.new_doc_width = d.width;
        state.settings.general.new_doc_height = d.height;
        state.settings.general.new_doc_background = d.background;
        let doc = Document::new(d.width.max(1), d.height.max(1), bg, name);
        state.add_document(doc);
    } else if closed {
        state.dialogs.new_doc = None;
    }
}

pub fn open_canvas_size(state: &mut AppState) {
    state.settle();
    if let Some(e) = state.active() {
        state.dialogs.canvas_size =
            Some(CanvasSizeDialog { doc: e.id, width: e.doc.width(), height: e.doc.height(), anchor: Anchor::Center });
    }
}

fn show_canvas_size(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.canvas_size.as_mut() else { return };
    let mut apply = false;
    let (_, closed) = modal(ctx, "canvas_size", "Canvas Size", 320.0, |ui| {
        egui::Grid::new("cs_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label("Width");
            ui.add(egui::DragValue::new(&mut d.width).range(1..=16384).suffix(" px"));
            ui.end_row();
            ui.label("Height");
            ui.add(egui::DragValue::new(&mut d.height).range(1..=16384).suffix(" px"));
            ui.end_row();
            ui.label("Anchor");
            ui.vertical(|ui| {
                for row in Anchor::ALL.chunks(3) {
                    ui.horizontal(|ui| {
                        for &a in row {
                            let glyph = match a {
                                Anchor::TopLeft => "↖",
                                Anchor::Top => "↑",
                                Anchor::TopRight => "↗",
                                Anchor::Left => "←",
                                Anchor::Center => "•",
                                Anchor::Right => "→",
                                Anchor::BottomLeft => "↙",
                                Anchor::Bottom => "↓",
                                Anchor::BottomRight => "↘",
                            };
                            if ui.add_sized([24.0, 22.0], egui::Button::new(glyph).selected(d.anchor == a)).clicked() {
                                d.anchor = a;
                            }
                        }
                    });
                }
            });
            ui.end_row();
        });
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                apply = true;
            }
            if ui.button("Cancel").clicked() {
                ui.close();
            }
        });
    });
    if apply {
        let d = state.dialogs.canvas_size.take().unwrap();
        if let Some(entry) = state.doc_mut(d.doc) {
            ops::resize_canvas(entry.doc.state_mut(), d.width, d.height, d.anchor);
            entry.doc.resized();
            entry.doc.commit("Canvas Size");
            entry.needs_full_upload = true;
            entry.sel_outline = None;
            entry.view.fit(d.width, d.height);
        }
    } else if closed {
        state.dialogs.canvas_size = None;
    }
}

pub fn open_modify(state: &mut AppState, kind: crate::tools::ModifyKind) {
    state.settle();
    let Some(e) = state.active() else { return };
    if e.doc.state().selection.is_none() {
        state.toasts.push(Level::Info, "Nothing is selected.");
        return;
    }
    let doc = e.id;
    if !kind.takes_amount() {
        crate::tools::modify_selection(state, doc, kind, 0.0);
        return;
    }
    let amount = state.tool_opts.modify_last(kind);
    state.dialogs.modify = Some(ModifyDialog { doc, kind, amount });
}

fn show_modify(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.modify.as_mut() else { return };
    let kind = d.kind;
    let (title, unit) = match kind {
        crate::tools::ModifyKind::Feather => ("Feather Selection", "Feather radius"),
        crate::tools::ModifyKind::Expand => ("Expand Selection", "Expand by"),
        crate::tools::ModifyKind::Contract => ("Contract Selection", "Contract by"),
        crate::tools::ModifyKind::Border => ("Border Selection", "Border width"),
        crate::tools::ModifyKind::Smooth => ("Smooth Selection", "Sample radius"),
        _ => ("Modify Selection", "Amount"),
    };
    let whole = kind == crate::tools::ModifyKind::Smooth;
    let mut apply = false;
    let (_, closed) = modal(ctx, "modify_selection", title, 320.0, |ui| {
        ui.horizontal(|ui| {
            ui.label(unit);
            let drag = if whole {
                egui::DragValue::new(&mut d.amount).range(1.0..=100.0).speed(0.2).suffix(" px").fixed_decimals(0)
            } else {
                egui::DragValue::new(&mut d.amount).range(0.1..=500.0).speed(0.2).suffix(" px").fixed_decimals(1)
            };
            ui.add(drag);
        });
        ui.add_space(4.0);
        ui.label(RichText::new(kind.describe()).weak());
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                apply = true;
            }
            if ui.button("Cancel").clicked() {
                ui.close();
            }
        });
    });
    if apply {
        let d = state.dialogs.modify.take().unwrap();
        state.tool_opts.set_modify_last(d.kind, d.amount);
        crate::tools::modify_selection(state, d.doc, d.kind, d.amount);
    } else if closed {
        state.dialogs.modify = None;
    }
}

pub fn open_image_size(state: &mut AppState) {
    state.settle();
    if let Some(e) = state.active() {
        let (w, h) = (e.doc.width(), e.doc.height());
        state.dialogs.image_size = Some(ImageSizeDialog {
            doc: e.id,
            width: w,
            height: h,
            orig_w: w,
            orig_h: h,
            keep_aspect: true,
            filter: ResizeFilter::Bilinear,
        });
    }
}

fn show_image_size(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.image_size.as_mut() else { return };
    let mut apply = false;
    let (_, closed) = modal(ctx, "image_size", "Image Size", 320.0, |ui| {
        let aspect = d.orig_w as f32 / d.orig_h.max(1) as f32;
        egui::Grid::new("is_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label("Width");
            if ui.add(egui::DragValue::new(&mut d.width).range(1..=16384).suffix(" px")).changed() && d.keep_aspect {
                d.height = ((d.width as f32 / aspect).round() as u32).max(1);
            }
            ui.end_row();
            ui.label("Height");
            if ui.add(egui::DragValue::new(&mut d.height).range(1..=16384).suffix(" px")).changed() && d.keep_aspect {
                d.width = ((d.height as f32 * aspect).round() as u32).max(1);
            }
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut d.keep_aspect, "Constrain proportions");
            ui.end_row();
            ui.label("Scale");
            ui.horizontal(|ui| {
                for pct in [25u32, 50, 200, 400] {
                    if crate::ui::widgets::small_button(ui, format!("{pct}%")).clicked() {
                        d.width = (d.orig_w * pct / 100).max(1);
                        d.height = (d.orig_h * pct / 100).max(1);
                    }
                }
            });
            ui.end_row();
            ui.label("Resample");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut d.filter, ResizeFilter::Bilinear, "Bilinear");
                ui.selectable_value(&mut d.filter, ResizeFilter::Nearest, "Nearest (pixel art)");
            });
            ui.end_row();
        });
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                apply = true;
            }
            if ui.button("Cancel").clicked() {
                ui.close();
            }
        });
    });
    if apply {
        let d = state.dialogs.image_size.take().unwrap();
        if let Some(entry) = state.doc_mut(d.doc) {
            ops::resize_image(entry.doc.state_mut(), d.width, d.height, d.filter);
            entry.doc.resized();
            entry.doc.commit("Image Size");
            entry.needs_full_upload = true;
            entry.sel_outline = None;
            entry.view.fit(d.width, d.height);
        }
    } else if closed {
        state.dialogs.image_size = None;
    }
}

pub fn open_layer_props(state: &mut AppState) {
    state.settle();
    if let Some(e) = state.active() {
        let l = e.doc.state().active_layer();
        state.dialogs.layer_props = Some(LayerPropsDialog { doc: e.id, layer: l.props.id, name: l.props.name.clone() });
    }
}

fn show_layer_props(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.layer_props.as_mut() else { return };
    let mut apply = false;
    let (_, closed) = modal(ctx, "layer_props", "Layer Properties", 300.0, |ui| {
        ui.horizontal(|ui| {
            ui.label("Name");
            let r = ui.text_edit_singleline(&mut d.name);
            r.request_focus();
        });
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                apply = true;
            }
            if ui.button("Cancel").clicked() {
                ui.close();
            }
        });
    });
    if apply {
        let d = state.dialogs.layer_props.take().unwrap();
        if let Some(entry) = state.doc_mut(d.doc) {
            let s = entry.doc.state_mut();
            if let Some(l) = s.index_of(d.layer).map(|i| &mut s.layers[i]) {
                if !d.name.trim().is_empty() {
                    l.props.name = d.name.trim().to_string();
                }
            }
            entry.doc.commit("Layer Properties");
        }
    } else if closed {
        state.dialogs.layer_props = None;
    }
}

/// Outcome of the unsaved-changes dialog, handled by the app.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

fn show_save_confirm(ctx: &Context, state: &mut AppState) {
    let Some(c) = state.dialogs.save_confirm.as_ref() else { return };
    let doc_id = c.doc;
    let path = c.path.clone();
    let then = c.then;
    let warnings = c.warnings.clone();
    let format = qsketch_core::io::format_name(&path);
    let flattens = qsketch_core::io::is_export_image(&path);
    let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut choice: Option<bool> = None;
    let (_, closed) = modal(ctx, "save_confirm", &format!("Save as {format}?"), 420.0, |ui| {
        ui.label(RichText::new(format!("\"{file}\" can't keep everything in this document:")));
        ui.add_space(6.0);
        for w in &warnings {
            ui.horizontal_wrapped(|ui| {
                ui.label("•");
                ui.label(w);
            });
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if flattens {
                ui.label(RichText::new("Tip: keep a .qsk copy to edit the layers later.").weak());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Save Anyway").strong())).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(false);
                }
            });
        });
    });
    if closed && choice.is_none() {
        choice = Some(false);
    }
    let Some(choice) = choice else { return };
    state.dialogs.save_confirm = None;
    if !choice {
        state.quit_requested = false;
        return;
    }
    if crate::files::write_document(state, doc_id, &path) {
        if let Some(then) = then {
            crate::files::force_close(state, doc_id);
            if then == AfterClose::Quit {
                state.quit_requested = true;
            }
        }
    } else {
        state.quit_requested = false;
    }
}

fn show_close_confirm(ctx: &Context, state: &mut AppState) {
    let Some(c) = state.dialogs.close_confirm.as_ref() else { return };
    let doc_id = c.doc;
    let then = c.then;
    let title = state.doc(doc_id).map(|d| d.doc.title.clone()).unwrap_or_default();
    let mut choice: Option<CloseChoice> = None;
    let (_, closed) = modal(ctx, "close_confirm", "Unsaved Changes", 360.0, |ui| {
        ui.label(format!("Save changes to \"{title}\" before closing?"));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("Don't Save").clicked() {
                choice = Some(CloseChoice::Discard);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Save").strong())).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    choice = Some(CloseChoice::Save);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(CloseChoice::Cancel);
                }
            });
        });
    });
    if closed && choice.is_none() {
        choice = Some(CloseChoice::Cancel);
    }
    let Some(choice) = choice else { return };
    state.dialogs.close_confirm = None;
    match choice {
        CloseChoice::Cancel => {
            state.quit_requested = false;
        }
        CloseChoice::Discard => {
            crate::files::force_close(state, doc_id);
            if then == AfterClose::Quit {
                state.quit_requested = true;
            }
        }
        CloseChoice::Save => {
            if crate::files::save_then(state, doc_id, Some(then)) {
                crate::files::force_close(state, doc_id);
                if then == AfterClose::Quit {
                    state.quit_requested = true;
                }
            } else if state.dialogs.save_confirm.is_some() {
                // The format-warning dialog carries the close forward.
            } else {
                state.quit_requested = false;
                state.toasts.push(Level::Info, "Close cancelled.");
            }
        }
    }
}

fn show_about(ctx: &Context, state: &mut AppState) {
    if !state.dialogs.about {
        return;
    }
    let (_, closed) = modal(ctx, "about", "About qsketch", 380.0, |ui| {
        ui.label(format!("qsketch {}", qsketch_core::VERSION));
        ui.label(RichText::new("A fast, professional sketching and raster painting app.").weak());
        ui.add_space(6.0);
        ui.label("Built with Rust, egui and wgpu. Icons: Phosphor (MIT).");
        ui.label("License: MIT OR Apache-2.0 · © 2026 Cabbit-Labs and contributors");
        ui.hyperlink_to("github.com/Cabbit-Labs/qsketch", "https://github.com/Cabbit-Labs/qsketch");
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Close").clicked() {
                ui.close();
            }
        });
    });
    if closed {
        state.dialogs.about = false;
    }
}

/// Snapshot unsaved work (recovered on next start) and relaunch the new build
/// with every file-backed document as an argument, so the update lands the
/// user back where they were.
fn install_and_reopen(state: &mut AppState, artifact: &std::path::Path) -> Result<(), String> {
    state.autosave.flush_all(&state.docs);
    let reopen: Vec<std::path::PathBuf> = state.docs.iter().filter_map(|d| d.doc.path.clone()).collect();
    crate::update::install_and_relaunch(artifact, &reopen)
}
