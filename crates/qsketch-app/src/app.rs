//! The eframe application: menus, keyboard dispatch, action execution and
//! document lifecycle around the dockable workspace.

use std::time::{Duration, Instant};

use egui::{Context, Key, RichText, Ui};
use qsketch_core::ops;
use qsketch_core::Document;

use crate::actions::{Action, Category};
use crate::canvas::render::CanvasRenderer;
use crate::dialogs::{self, AdjustKind, AfterClose, CloseConfirm};
use crate::settings::{NewDocBackground, Settings};
use crate::state::{AppState, DocId, TempReason};
use crate::tools::ToolKind;
use crate::ui::toasts::Level;
use crate::ui::{icons, theme};
use crate::workspace::{PanelKind, Workspace};

pub struct QSketchApp {
    state: AppState,
    /// Single-instance listener; other launches hand their files here.
    instance: crate::single_instance::Primary,
    workspace: Workspace,
    tablet: Option<crate::tablet::Tablet>,
    applied_theme: (crate::settings::UiSettings, f32),
    /// Decorations state last sent to the OS (see `UiSettings::native_frame`).
    applied_native_frame: bool,
    /// App icon for the custom title strip.
    app_icon: egui::TextureHandle,
    last_settings_save: Instant,
    last_title: String,
    /// A text field had focus during the previous frame (shortcuts are suspended).
    text_editing: bool,
    /// A clipboard chord already fired via `Event::Copy/Cut/Paste` this press,
    /// so the key-release fallback must not fire it again.
    clipboard_chord_fired: bool,
    /// Keys whose press event reached us (a release without one means egui-winit
    /// swallowed the press as a clipboard chord).
    keys_seen_pressed: std::collections::HashSet<Key>,
    auto_update_checked: bool,
}

impl QSketchApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        files: Vec<std::path::PathBuf>,
        instance: crate::single_instance::Primary,
    ) -> Self {
        let mut settings = Settings::load();
        settings.sanitize();
        theme::install_fonts(&cc.egui_ctx, settings.ui.icon_set.filled());
        crate::ui::iconset::configure(settings.ui.icon_set, &settings.ui.icon_overrides);
        theme::apply(&cc.egui_ctx, &settings.ui.palette(), settings.ui.scale);
        let applied_theme = (settings.ui.clone(), settings.ui.scale);
        let applied_native_frame = settings.ui.native_frame;
        let app_icon = {
            let img = image::load_from_memory(include_bytes!("../../../assets/icon/icon-256.png"))
                .map(|i| i.to_rgba8())
                .unwrap_or_else(|_| image::RgbaImage::new(1, 1));
            let (w, h) = img.dimensions();
            let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
            cc.egui_ctx.load_texture("app_icon", ci, egui::TextureOptions::LINEAR)
        };

        let mut state = AppState::new(settings);
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            let renderer = CanvasRenderer::new(&rs.device, rs.target_format);
            rs.renderer.write().callback_resources.insert(renderer);
            state.render_state = Some(rs.clone());
        } else {
            log::error!("wgpu render state missing; canvas rendering disabled");
        }

        let workspace = state.settings.layout.as_deref().and_then(Workspace::from_json).unwrap_or_else(Workspace::new);
        let tablet = if state.settings.tablet.use_octotablet { crate::tablet::Tablet::new(cc) } else { None };

        let mut app = Self {
            state,
            instance,
            workspace,
            tablet,
            applied_theme,
            applied_native_frame,
            app_icon,
            last_settings_save: Instant::now(),
            last_title: String::new(),
            text_editing: false,
            clipboard_chord_fired: false,
            keys_seen_pressed: Default::default(),
            auto_update_checked: false,
        };
        if files.is_empty() {
            let g = &app.state.settings.general;
            let bg = match g.new_doc_background {
                NewDocBackground::White => Some(qsketch_core::Rgba8::WHITE),
                NewDocBackground::Transparent => None,
                NewDocBackground::BackgroundColor => Some(app.state.bg),
            };
            let title = app.state.untitled_title();
            let id = app.state.add_document(Document::new(g.new_doc_width, g.new_doc_height, bg, title));
            app.workspace.add_document(id);
        } else {
            app.state.open_file_requests = files;
        }
        let leftovers = crate::autosave::scan();
        if !leftovers.is_empty() {
            app.state.dialogs.recover = Some(leftovers);
        }
        app
    }

    // ---------------------------------------------------------------------
    // Frame

    fn process_requests(&mut self, ctx: &Context) {
        // Files forwarded by a second launch (file manager "Open with"): open
        // them as tabs and bring this window to the front.
        let mut forwarded = false;
        while let Ok(paths) = self.instance.rx.try_recv() {
            self.state.open_file_requests.extend(paths);
            forwarded = true;
        }
        if forwarded {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        // File opens (CLI, drag & drop, recent list).
        let reqs = std::mem::take(&mut self.state.open_file_requests);
        for p in reqs {
            if let Some(id) = crate::files::open_path(&mut self.state, &p) {
                self.workspace.add_document(id);
            }
        }
        // Make sure each document has a tab, and drop tabs for closed docs.
        let ids: Vec<DocId> = self.state.docs.iter().map(|d| d.id).collect();
        for &id in &ids {
            if !self.workspace.is_panel_open(&PanelKind::Document(id)) {
                self.workspace.add_document(id);
            }
        }
        let stale: Vec<DocId> = self
            .workspace
            .dock
            .iter_all_tabs()
            .filter_map(|(_, t)| if let PanelKind::Document(id) = t { Some(*id) } else { None })
            .filter(|id| !ids.contains(id))
            .collect();
        for id in stale {
            self.workspace.remove_document(id);
        }
        // Panels requested by menus / actions.
        for p in std::mem::take(&mut self.state.show_panel_requests) {
            self.workspace.show_panel(p);
        }
        if self.state.layout_reset_requested {
            self.state.layout_reset_requested = false;
            self.workspace.reset(&ids);
        }
        // Close requests (with confirmation).
        if let Some(id) = self.state.close_doc_requests.first().copied() {
            if self.state.dialogs.close_confirm.is_none() {
                self.request_close(id, AfterClose::Nothing);
            }
        }
        // Quit flow: close documents one at a time, confirming as needed.
        if self.state.quit_requested && self.state.dialogs.close_confirm.is_none() {
            match self
                .state
                .docs
                .iter()
                .find(|d| d.doc.is_modified() && self.state.settings.general.confirm_close)
                .map(|d| d.id)
            {
                Some(id) => self.request_close(id, AfterClose::Quit),
                None => {
                    self.persist();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        // Window title
        let title = match self.state.active() {
            Some(d) => format!("{} — qsketch", d.doc.display_title()),
            None => "qsketch".to_string(),
        };
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    fn request_close(&mut self, id: DocId, then: AfterClose) {
        let Some(d) = self.state.doc(id) else {
            self.state.close_doc_requests.retain(|x| *x != id);
            return;
        };
        if d.doc.is_modified() && self.state.settings.general.confirm_close {
            self.state.dialogs.close_confirm = Some(CloseConfirm { doc: id, then });
        } else {
            crate::files::force_close(&mut self.state, id);
            if then == AfterClose::Quit {
                self.state.quit_requested = true;
            }
        }
    }

    /// Start the background update check once per launch when it is due.
    fn auto_update_check(&mut self) {
        if self.auto_update_checked {
            return;
        }
        self.auto_update_checked = true;
        let u = &mut self.state.settings.update;
        if !u.check_on_start || crate::update::platform_key() == "unsupported" {
            return;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if now.saturating_sub(u.last_check) < u.check_interval_hours as u64 * 3600 {
            return;
        }
        let Some(url) = u.effective_manifest_url() else { return };
        u.last_check = now;
        self.state.updater.check(url, false);
    }

    fn persist(&mut self) {
        self.state.sync_settings();
        self.state.settings.layout = self.workspace.to_json();
        if let Err(e) = self.state.settings.save() {
            log::warn!("saving settings: {e}");
        }
        self.last_settings_save = Instant::now();
    }

    fn handle_keyboard(&mut self, ctx: &Context) {
        let wants_text = self.text_editing;
        let dialog_open = self.state.dialogs.any_open();

        // Temporary tools while a key is held.
        let space = ctx.input(|i| i.key_down(Key::Space));
        let alt = ctx.input(|i| i.modifiers.alt);
        let ctrl = ctx.input(|i| i.modifiers.command);
        let in_stroke = self.state.session.is_some();
        let middle_down = ctx.input(|i| i.pointer.middle_down());
        match self.state.temp_tool {
            Some((_, TempReason::Space)) if !space => self.state.temp_tool = None,
            Some((_, TempReason::Alt)) if !alt => self.state.temp_tool = None,
            Some((_, TempReason::Ctrl)) if !ctrl => self.state.temp_tool = None,
            Some((_, TempReason::Middle)) if !middle_down && !in_stroke => self.state.temp_tool = None,
            _ => {}
        }
        let m = &self.state.settings.mouse;
        let alt_picks = m.alt_click_picks_foreground || m.alt_right_click_picks_background;
        if !wants_text && !dialog_open && !in_stroke && self.state.temp_tool.is_none() {
            if space {
                self.state.temp_tool = Some((ToolKind::Hand, TempReason::Space));
            } else if alt && !ctrl && alt_picks && self.state.tool.uses_brush() {
                self.state.temp_tool = Some((ToolKind::Eyedropper, TempReason::Alt));
            } else if ctrl
                && !alt
                && self.state.tool.is_paint()
                && !ctx.input(|i| i.keys_down.iter().any(|k| !matches!(k, Key::Space)))
            {
                self.state.temp_tool = Some((ToolKind::Move, TempReason::Ctrl));
            }
        }

        let events = ctx.input(|i| i.events.clone());
        if wants_text || dialog_open {
            return;
        }
        for ev in events {
            // egui-winit turns Ctrl+C / Ctrl+X / Ctrl+V presses into these events
            // instead of key presses (and emits nothing for an image-only
            // clipboard), so they never reach the keymap.
            match &ev {
                egui::Event::Copy => {
                    self.clipboard_chord_fired = true;
                    self.perform(Action::Copy, ctx);
                    continue;
                }
                egui::Event::Cut => {
                    self.clipboard_chord_fired = true;
                    self.perform(Action::Cut, ctx);
                    continue;
                }
                egui::Event::Paste(_) => {
                    self.clipboard_chord_fired = true;
                    let shift = ctx.input(|i| i.modifiers.shift);
                    self.perform(if shift { Action::PasteInPlace } else { Action::Paste }, ctx);
                    continue;
                }
                egui::Event::Key { key, pressed: false, modifiers, .. } => {
                    // Fallback for a swallowed press: a C/X/V release whose press we
                    // never saw was a Ctrl chord. Fire it now (unless the press already
                    // produced a Copy/Cut/Paste event), whatever order the keys came up in.
                    let seen = self.keys_seen_pressed.remove(key);
                    if !seen && matches!(key, Key::C | Key::X | Key::V) {
                        let fired = std::mem::take(&mut self.clipboard_chord_fired);
                        if !fired {
                            let mods = egui::Modifiers { command: true, ctrl: true, ..*modifiers };
                            if let Some(action) = self.state.keymap.lookup(*key, mods) {
                                if matches!(
                                    action,
                                    Action::Copy
                                        | Action::CopyMerged
                                        | Action::Cut
                                        | Action::Paste
                                        | Action::PasteInPlace
                                ) {
                                    self.perform(action, ctx);
                                }
                            }
                        }
                    }
                    continue;
                }
                egui::Event::Key { key, pressed: true, .. } => {
                    self.keys_seen_pressed.insert(*key);
                }
                _ => {}
            }
            let egui::Event::Key { key, pressed: true, repeat, modifiers, .. } = ev else { continue };
            // Floating paste keys.
            if let Some(id) = self.state.active_doc {
                if self.state.floating.as_ref().is_some_and(|f| f.doc == id) {
                    match key {
                        Key::Enter => {
                            crate::tools::floating::commit(&mut self.state);
                            continue;
                        }
                        Key::Escape => {
                            crate::tools::floating::cancel(&mut self.state);
                            continue;
                        }
                        Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown if !modifiers.command => {
                            let k = if modifiers.shift { 10 } else { 1 };
                            let (dx, dy) = match key {
                                Key::ArrowLeft => (-k, 0),
                                Key::ArrowRight => (k, 0),
                                Key::ArrowUp => (0, -k),
                                _ => (0, k),
                            };
                            if let Some(f) = self.state.floating.as_mut() {
                                f.nudge(dx, dy);
                            }
                            continue;
                        }
                        _ => {}
                    }
                }
            }
            // Tool-specific keys first.
            if let Some(id) = self.state.active_doc {
                if self.state.tool == ToolKind::Crop && self.state.tool_opts.crop_rect.is_some() {
                    if key == Key::Enter {
                        crate::tools::commit_crop(&mut self.state, id);
                        continue;
                    }
                    if key == Key::Escape {
                        self.state.tool_opts.crop_rect = None;
                        continue;
                    }
                }
                if key == Key::Escape && self.state.session.is_some() {
                    self.state.cancel_session();
                    continue;
                }
                let nudge = match key {
                    Key::ArrowLeft => Some((-1, 0)),
                    Key::ArrowRight => Some((1, 0)),
                    Key::ArrowUp => Some((0, -1)),
                    Key::ArrowDown => Some((0, 1)),
                    _ => None,
                };
                if let Some((dx, dy)) = nudge {
                    if self.state.tool == ToolKind::Move && !modifiers.command {
                        let k = if modifiers.shift { 10 } else { 1 };
                        crate::tools::nudge(&mut self.state, id, dx * k, dy * k);
                        continue;
                    }
                }
            }
            if let Some(action) = self.state.keymap.lookup(key, modifiers) {
                if repeat && !action.repeatable() {
                    continue;
                }
                self.perform(action, ctx);
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &Context) {
        let dropped: Vec<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            self.state.open_file_requests.extend(dropped);
        }
    }

    // ---------------------------------------------------------------------
    // Menus

    fn menu_item(&mut self, ui: &mut Ui, action: Action, enabled: bool) {
        let text = self.state.keymap.primary_text(action);
        let btn = egui::Button::new(action.label()).shortcut_text(text);
        if ui.add_enabled(enabled, btn).clicked() {
            self.state.pending.push(action);
            ui.close();
        }
    }

    /// The all-in-one top strip used instead of the OS title bar: app icon,
    /// menus, window title, update indicator and minimize/maximize/close.
    /// The empty parts of the strip drag the window; double-click maximizes.
    fn title_strip(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let strip = ui.available_rect_before_wrap();
        let strip = egui::Rect::from_min_size(strip.min, egui::vec2(strip.width(), 24.0));
        // Register the drag surface first so widgets drawn on top win clicks.
        let drag = ui.interact(strip, ui.id().with("title_drag"), egui::Sense::click_and_drag());
        if drag.double_clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        } else if drag.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        let p = self.state.settings.ui.palette();
        let title = self.last_title.clone();
        let update_ready =
            matches!(self.state.updater.status, crate::update::Status::Available | crate::update::Status::Ready(_));
        let mut menus_end = strip.left();
        let mut cluster_start = strip.right();
        ui.horizontal(|ui| {
            ui.set_min_height(24.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.add(egui::Image::new(&self.app_icon).fit_to_exact_size(egui::vec2(16.0, 16.0)));
            ui.add_space(4.0);
            // `MenuBar` claims the full width, so the last title button reports
            // where the menus really end (see `top_menu`).
            ui.data_mut(|d| d.insert_temp(menu_bar_right_id(), strip.left()));
            self.menu_bar(ui);
            menus_end = ui.data(|d| d.get_temp(menu_bar_right_id()).unwrap_or(strip.left()));
            // Right-aligned cluster: window buttons, then the update pill.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let btn = |ui: &mut Ui, glyph: &str, tip: &str, danger: bool| -> bool {
                    let text = crate::ui::widgets::icon(glyph, 12.0);
                    let r = ui.add(egui::Button::new(text).frame(false).min_size(egui::vec2(34.0, 24.0)));
                    if r.hovered() {
                        let fill = if danger { p.danger } else { p.widget_hover };
                        ui.painter().rect_filled(r.rect, 0, fill);
                        ui.painter().text(
                            r.rect.center(),
                            egui::Align2::CENTER_CENTER,
                            glyph,
                            egui::FontId::new(12.0, crate::ui::iconset::family()),
                            if danger { egui::Color32::WHITE } else { p.text },
                        );
                    }
                    r.on_hover_text(tip).clicked()
                };
                if btn(ui, icons::X, "Close", true) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                let (glyph, tip) = if maximized { (icons::CORNERS_IN, "Restore") } else { (icons::SQUARE, "Maximize") };
                if btn(ui, glyph, tip, false) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
                if btn(ui, icons::MINUS, "Minimize", false) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                ui.add_space(8.0);
                if update_ready {
                    let label = RichText::new(format!("{} Update", icons::ARROW_CIRCLE_DOWN))
                        .color(egui::Color32::WHITE)
                        .small();
                    let r = ui
                        .add(egui::Button::new(label).fill(p.accent).corner_radius(4).min_size(egui::vec2(0.0, 18.0)));
                    if r.on_hover_text("A new version is ready to install").clicked() {
                        self.state.updater.show_dialog = true;
                    }
                    ui.add_space(8.0);
                }
                cluster_start = ui.min_rect().left();
            });
        });
        // Window title, centered in the strip when it fits between the menus
        // and the button cluster, otherwise squeezed into whatever is free.
        let free = egui::Rect::from_min_max(
            egui::pos2(menus_end + 16.0, strip.top()),
            egui::pos2(cluster_start - 16.0, strip.bottom()),
        );
        if free.width() > 40.0 {
            let font = egui::FontId::proportional(11.5);
            let galley = ui.painter().layout(title, font, p.text_dim, free.width());
            let w = galley.size().x;
            let centered = strip.center().x - w / 2.0;
            let x = centered.clamp(free.left(), (free.right() - w).max(free.left()));
            let pos = egui::pos2(x, strip.center().y - galley.size().y / 2.0);
            ui.painter().with_clip_rect(free).galley(pos, galley, p.text_dim);
        }
    }

    /// Invisible resize border for the undecorated window: hovering within a
    /// few points of an edge shows the resize cursor, pressing starts an OS
    /// resize. Disabled while maximized.
    fn edge_resize(&mut self, ctx: &Context) {
        use egui::viewport::ResizeDirection as D;
        if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else { return };
        let r = ctx.content_rect();
        const B: f32 = 5.0;
        let (l, rt, t, b) = (pos.x < r.left() + B, pos.x > r.right() - B, pos.y < r.top() + B, pos.y > r.bottom() - B);
        let dir = match (l, rt, t, b) {
            (true, _, true, _) => D::NorthWest,
            (_, true, true, _) => D::NorthEast,
            (true, _, _, true) => D::SouthWest,
            (_, true, _, true) => D::SouthEast,
            (true, _, _, _) => D::West,
            (_, true, _, _) => D::East,
            (_, _, true, _) => D::North,
            (_, _, _, true) => D::South,
            _ => return,
        };
        let cursor = match dir {
            D::North | D::South => egui::CursorIcon::ResizeVertical,
            D::East | D::West => egui::CursorIcon::ResizeHorizontal,
            D::NorthWest | D::SouthEast => egui::CursorIcon::ResizeNwSe,
            D::NorthEast | D::SouthWest => egui::CursorIcon::ResizeNeSw,
        };
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
        }
    }

    fn menu_bar(&mut self, ui: &mut Ui) {
        let has_doc = self.state.active_doc.is_some();
        let (can_undo, can_redo, has_sel, layers, active, sel_or_content) = match self.state.active() {
            Some(d) => (
                d.doc.can_undo(),
                d.doc.can_redo(),
                d.doc.state().selection.is_some(),
                d.doc.state().layers.len(),
                d.doc.state().active,
                true,
            ),
            None => (false, false, false, 0, 0, false),
        };
        // Menu buttons span the whole 24 px strip so their own hover
        // highlight (and click) reaches the top edge of the window.
        ui.spacing_mut().interact_size.y = 24.0;
        egui::MenuBar::new().ui(ui, |ui| {
            top_menu(ui, "File", |ui| {
                self.menu_item(ui, Action::NewDocument, true);
                self.menu_item(ui, Action::OpenDocument, true);
                ui.menu_button("Open Recent", |ui| {
                    let recent = self.state.settings.general.recent_files.clone();
                    if recent.is_empty() {
                        ui.add_enabled(false, egui::Button::new("(empty)"));
                    }
                    for p in recent {
                        let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                        if ui.button(name).on_hover_text(p.display().to_string()).clicked() {
                            self.state.open_file_requests.push(p);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Clear list").clicked() {
                        self.state.settings.general.recent_files.clear();
                        ui.close();
                    }
                });
                ui.separator();
                self.menu_item(ui, Action::Save, has_doc);
                self.menu_item(ui, Action::SaveAs, has_doc);
                self.menu_item(ui, Action::ExportImage, has_doc);
                ui.separator();
                self.menu_item(ui, Action::CloseDocument, has_doc);
                self.menu_item(ui, Action::Quit, true);
            });
            top_menu(ui, "Edit", |ui| {
                self.menu_item(ui, Action::Undo, can_undo);
                self.menu_item(ui, Action::Redo, can_redo);
                ui.separator();
                self.menu_item(ui, Action::Cut, has_doc);
                self.menu_item(ui, Action::Copy, has_doc);
                self.menu_item(ui, Action::CopyMerged, has_doc);
                self.menu_item(ui, Action::Paste, has_doc);
                self.menu_item(ui, Action::PasteInPlace, has_doc);
                if ui.button("Paste as New Document").clicked() {
                    crate::clipboard::paste_as_new_document(&mut self.state);
                    ui.close();
                }
                ui.separator();
                self.menu_item(ui, Action::Clear, has_doc);
                self.menu_item(ui, Action::FillForeground, has_doc);
                self.menu_item(ui, Action::FillBackground, has_doc);
                ui.separator();
                self.menu_item(ui, Action::Preferences, true);
            });
            top_menu(ui, "Image", |ui| {
                self.menu_item(ui, Action::ImageSize, has_doc);
                self.menu_item(ui, Action::CanvasSize, has_doc);
                self.menu_item(ui, Action::CropToSelection, has_sel);
                ui.separator();
                ui.menu_button("Rotate / Flip", |ui| {
                    self.menu_item(ui, Action::Rotate90CW, has_doc);
                    self.menu_item(ui, Action::Rotate90CCW, has_doc);
                    self.menu_item(ui, Action::Rotate180, has_doc);
                    ui.separator();
                    self.menu_item(ui, Action::FlipHorizontal, has_doc);
                    self.menu_item(ui, Action::FlipVertical, has_doc);
                });
                ui.separator();
                ui.menu_button("Adjustments", |ui| {
                    self.menu_item(ui, Action::BrightnessContrast, has_doc);
                    self.menu_item(ui, Action::HueSaturation, has_doc);
                    ui.separator();
                    self.menu_item(ui, Action::Desaturate, has_doc);
                    self.menu_item(ui, Action::InvertColors, has_doc);
                });
            });
            top_menu(ui, "Layer", |ui| {
                self.menu_item(ui, Action::NewLayer, has_doc);
                self.menu_item(ui, Action::DuplicateLayer, has_doc);
                self.menu_item(ui, Action::DeleteLayer, layers > 1);
                self.menu_item(ui, Action::LayerProperties, has_doc);
                ui.separator();
                self.menu_item(ui, Action::MergeDown, active > 0);
                self.menu_item(ui, Action::MergeVisible, layers > 1);
                self.menu_item(ui, Action::Flatten, layers > 1);
                ui.separator();
                ui.menu_button("Arrange", |ui| {
                    self.menu_item(ui, Action::LayerToTop, has_doc);
                    self.menu_item(ui, Action::LayerUp, has_doc);
                    self.menu_item(ui, Action::LayerDown, has_doc);
                    self.menu_item(ui, Action::LayerToBottom, has_doc);
                });
                self.menu_item(ui, Action::SelectLayerAbove, has_doc);
                self.menu_item(ui, Action::SelectLayerBelow, has_doc);
                ui.separator();
                self.menu_item(ui, Action::ToggleLayerVisibility, has_doc);
                self.menu_item(ui, Action::ToggleAlphaLock, has_doc);
                self.menu_item(ui, Action::ToggleLayerLock, has_doc);
                ui.separator();
                self.menu_item(ui, Action::FlipLayerHorizontal, has_doc);
                self.menu_item(ui, Action::FlipLayerVertical, has_doc);
                self.menu_item(ui, Action::ClearLayer, has_doc);
            });
            top_menu(ui, "Select", |ui| {
                self.menu_item(ui, Action::SelectAll, has_doc);
                self.menu_item(ui, Action::Deselect, has_sel);
                self.menu_item(ui, Action::InvertSelection, has_doc);
                self.menu_item(ui, Action::SelectLayerContent, sel_or_content);
            });
            top_menu(ui, "View", |ui| {
                self.menu_item(ui, Action::ZoomIn, has_doc);
                self.menu_item(ui, Action::ZoomOut, has_doc);
                self.menu_item(ui, Action::ZoomFit, has_doc);
                self.menu_item(ui, Action::Zoom100, has_doc);
                self.menu_item(ui, Action::Zoom200, has_doc);
                ui.separator();
                self.menu_item(ui, Action::RotateViewLeft, has_doc);
                self.menu_item(ui, Action::RotateViewRight, has_doc);
                self.menu_item(ui, Action::FlipViewHorizontal, has_doc);
                self.menu_item(ui, Action::ResetView, has_doc);
                ui.separator();
                let grid = self.state.settings.canvas.show_pixel_grid;
                let btn = egui::Button::new(format!("{} Pixel Grid", if grid { icons::CHECK } else { " " }))
                    .shortcut_text(self.state.keymap.primary_text(Action::TogglePixelGrid));
                if ui.add(btn).clicked() {
                    self.state.pending.push(Action::TogglePixelGrid);
                    ui.close();
                }
                ui.separator();
                ui.menu_button("Symmetry", |ui| {
                    let sym = self.state.symmetry;
                    for (a, on) in [
                        (Action::ToggleSymmetryHorizontal, sym.horizontal),
                        (Action::ToggleSymmetryVertical, sym.vertical),
                    ] {
                        let btn = egui::Button::new(format!("{} {}", if on { icons::CHECK } else { " " }, a.label()))
                            .shortcut_text(self.state.keymap.primary_text(a));
                        if ui.add(btn).clicked() {
                            self.state.pending.push(a);
                            ui.close();
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.label("Radial copies");
                        let mut n = self.state.symmetry.radial.max(1);
                        if ui.add(egui::DragValue::new(&mut n).range(1..=64)).changed() {
                            self.state.symmetry.radial = if n <= 1 { 0 } else { n };
                        }
                    });
                    ui.separator();
                    self.menu_item(ui, Action::SymmetrySetCenter, has_doc);
                    self.menu_item(ui, Action::SymmetryResetCenter, true);
                    let g = self.state.symmetry.show_guides;
                    if ui
                        .add(egui::Button::new(format!("{} Show Guides", if g { icons::CHECK } else { " " })))
                        .clicked()
                    {
                        self.state.symmetry.show_guides = !g;
                        ui.close();
                    }
                    ui.separator();
                    self.menu_item(ui, Action::SymmetryOff, sym.active());
                });
                self.menu_item(ui, Action::TogglePanels, true);
                self.menu_item(ui, Action::ToggleFullscreen, true);
            });
            top_menu(ui, "Filter", |ui| self.filter_menu(ui, has_doc));
            top_menu(ui, "Window", |ui| {
                for (a, k) in [
                    (Action::ShowTools, PanelKind::Tools),
                    (Action::ShowLayers, PanelKind::Layers),
                    (Action::ShowHistory, PanelKind::History),
                    (Action::ShowColor, PanelKind::Color),
                    (Action::ShowSwatches, PanelKind::Swatches),
                    (Action::ShowNavigator, PanelKind::Navigator),
                    (Action::ShowBrushes, PanelKind::Brushes),
                    (Action::ShowBrushSettings, PanelKind::BrushSettings),
                    (Action::ShowInfo, PanelKind::Info),
                ] {
                    let open = self.workspace.is_panel_open(&k);
                    let btn = egui::Button::new(format!("{} {}", if open { icons::CHECK } else { " " }, a.label()))
                        .shortcut_text(self.state.keymap.primary_text(a));
                    if ui.add(btn).clicked() {
                        self.state.pending.push(a);
                        ui.close();
                    }
                }
                ui.separator();
                self.menu_item(ui, Action::ResetLayout, true);
                ui.separator();
                let docs: Vec<(DocId, String)> =
                    self.state.docs.iter().map(|d| (d.id, d.doc.display_title())).collect();
                for (id, title) in docs {
                    let active = self.state.active_doc == Some(id);
                    if ui
                        .add(egui::Button::new(format!("{} {}", if active { icons::CHECK } else { " " }, title)))
                        .clicked()
                    {
                        self.state.active_doc = Some(id);
                        self.workspace.focus_document(id);
                        ui.close();
                    }
                }
            });
            top_menu(ui, "Help", |ui| {
                self.menu_item(ui, Action::KeyboardShortcuts, true);
                if ui.button("Open Settings Folder").clicked() {
                    if let Some(dir) = Settings::config_dir() {
                        let _ = std::fs::create_dir_all(&dir);
                        open_in_file_manager(&dir);
                    }
                    ui.close();
                }
                ui.separator();
                self.menu_item(ui, Action::CheckForUpdates, !self.state.updater.busy());
                self.menu_item(ui, Action::About, true);
            });
        });
    }

    /// The Filter menu: last filter, Oil Paint, then Photoshop's submenu
    /// groups plus an Experimental group of our own.
    fn filter_menu(&mut self, ui: &mut Ui, has_doc: bool) {
        let last = self.state.last_filter.as_ref().map(|f| f.name());
        let label = last.map_or_else(|| "Last Filter".to_string(), |n| n.to_string());
        let btn = egui::Button::new(label).shortcut_text(self.state.keymap.primary_text(Action::LastFilter));
        if ui.add_enabled(has_doc && last.is_some(), btn).clicked() {
            self.state.pending.push(Action::LastFilter);
            ui.close();
        }
        self.menu_item(ui, Action::LastFilterDialog, has_doc && last.is_some());
        ui.separator();
        self.menu_item(ui, Action::FilterOilPaint, has_doc);
        ui.separator();
        let groups: [(&str, &[Action]); 8] = [
            (
                "Blur",
                &[
                    Action::FilterGaussianBlur,
                    Action::FilterBoxBlur,
                    Action::FilterMotionBlur,
                    Action::FilterRadialBlur,
                ],
            ),
            (
                "Distort",
                &[
                    Action::FilterRipple,
                    Action::FilterWave,
                    Action::FilterTwirl,
                    Action::FilterSpherize,
                    Action::FilterZigZag,
                    Action::FilterPolarCoordinates,
                ],
            ),
            ("Noise", &[Action::FilterAddNoise, Action::FilterMedian, Action::FilterDustAndScratches]),
            (
                "Pixelate",
                &[
                    Action::FilterMosaic,
                    Action::FilterCrystallize,
                    Action::FilterFragment,
                    Action::FilterColorHalftone,
                    Action::FilterPointillize,
                ],
            ),
            ("Render", &[Action::FilterClouds, Action::FilterDifferenceClouds]),
            ("Sharpen", &[Action::FilterSharpen, Action::FilterSharpenMore, Action::FilterUnsharpMask]),
            (
                "Stylize",
                &[
                    Action::FilterFindEdges,
                    Action::FilterEmboss,
                    Action::FilterSolarize,
                    Action::FilterDiffuse,
                    Action::FilterWind,
                ],
            ),
            ("Other", &[Action::FilterHighPass, Action::FilterMaximum, Action::FilterMinimum, Action::FilterOffset]),
        ];
        for (name, actions) in groups {
            ui.menu_button(name, |ui| {
                for &a in actions {
                    self.menu_item(ui, a, has_doc);
                }
            });
        }
        ui.separator();
        ui.menu_button("Experimental", |ui| {
            for a in [
                Action::FilterOutline,
                Action::FilterGlow,
                Action::FilterVignette,
                Action::FilterPencilSketch,
                Action::FilterDither,
                Action::FilterKaleidoscope,
                Action::FilterChromaticAberration,
                Action::FilterScanlines,
                Action::FilterPixelSort,
                Action::FilterGlitch,
            ] {
                self.menu_item(ui, a, has_doc);
            }
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if let Some(d) = self.state.active() {
                // View group (zoom, size, cursor) then document group (layer, selection),
                // each tagged with its section color.
                ui.label(RichText::new(icons::COMPASS).weak().small());
                ui.label(RichText::new(format!("{:.0}%", d.view.zoom * 100.0)).monospace().small());
                ui.label(RichText::new(format!("{} × {}", d.doc.width(), d.doc.height())).weak().small());
                if let Some(pos) = self.state.hover_doc_pos {
                    ui.label(
                        RichText::new(format!("{}, {}", pos.x.floor() as i32, pos.y.floor() as i32))
                            .monospace()
                            .weak()
                            .small(),
                    );
                }
                ui.label(RichText::new(icons::STACK).weak().small());
                ui.label(RichText::new(d.doc.state().active_layer().props.name.clone()).weak().small());
                if let Some(m) = d.doc.state().selection_mask() {
                    let b = m.bounds();
                    ui.label(RichText::new(format!("sel {}×{}", b.w, b.h)).weak().small());
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let tool = self.state.effective_tool();
                let hint = match tool {
                    ToolKind::Brush | ToolKind::Pencil | ToolKind::Eraser => {
                        "Shift+click: straight line · [ ]: size · Alt+click: pick · right-click: brush · middle-drag: pan"
                    }
                    ToolKind::RectSelect | ToolKind::EllipseSelect | ToolKind::Lasso => {
                        "Shift: add · Alt: subtract · click: deselect"
                    }
                    _ if self.state.floating.is_some() => {
                        "drag: move · handles: scale (Shift toggles aspect) · Enter: apply · Esc: cancel"
                    }
                    ToolKind::Move => "Shift: constrain · arrows: nudge",
                    ToolKind::Zoom => "click: zoom in · Alt+click: zoom out · drag: scrub",
                    ToolKind::Crop => "Enter: apply · Esc: cancel",
                    _ => "",
                };
                ui.label(RichText::new(hint).weak().small());
                if self.state.pen.pressure.is_some() {
                    ui.label(
                        RichText::new(format!("{} {:.0}%", icons::PEN, self.state.pen.pressure.unwrap_or(0.0) * 100.0))
                            .weak()
                            .small(),
                    );
                }
            });
        });
    }

    // ---------------------------------------------------------------------
    // Actions

    fn perform(&mut self, action: Action, ctx: &Context) {
        if let Some(tool) = action.tool() {
            self.state.set_tool(tool);
            return;
        }
        let active = self.state.active_doc;
        match action {
            Action::NewDocument => dialogs::open_new_doc(&mut self.state),
            Action::OpenDocument => crate::files::open_dialog(&mut self.state),
            Action::Save => {
                if let Some(id) = active {
                    crate::files::save(&mut self.state, id);
                }
            }
            Action::SaveAs => {
                if let Some(id) = active {
                    crate::files::save_as(&mut self.state, id);
                }
            }
            Action::ExportImage => {
                if let Some(id) = active {
                    crate::files::export(&mut self.state, id);
                }
            }
            Action::CloseDocument => {
                if let Some(id) = active {
                    self.state.close_doc_requests.push(id);
                }
            }
            Action::Quit => self.state.quit_requested = true,
            Action::Undo => {
                self.state.cancel_session();
                // Undo while placing a paste (or typing text) discards it.
                if crate::tools::floating::cancel(&mut self.state) || crate::tools::text::cancel(&mut self.state) {
                    return;
                }
                if let Some(d) = self.state.active_mut() {
                    if d.doc.undo() {
                        d.sel_outline = None;
                    }
                }
            }
            Action::Redo => {
                self.state.cancel_session();
                crate::tools::floating::commit(&mut self.state);
                crate::tools::text::commit(&mut self.state);
                if let Some(d) = self.state.active_mut() {
                    if d.doc.redo() {
                        d.sel_outline = None;
                    }
                }
            }
            Action::Cut => {
                if let Some(id) = active {
                    crate::clipboard::cut(&mut self.state, id);
                }
            }
            Action::Copy => {
                if let Some(id) = active {
                    crate::clipboard::copy(&mut self.state, id, false);
                }
            }
            Action::CopyMerged => {
                if let Some(id) = active {
                    crate::clipboard::copy(&mut self.state, id, true);
                }
            }
            Action::Paste => {
                if let Some(id) = active {
                    crate::clipboard::paste(&mut self.state, id, false);
                }
            }
            Action::PasteInPlace => {
                if let Some(id) = active {
                    crate::clipboard::paste(&mut self.state, id, true);
                }
            }
            Action::Clear => self.edit_layer("Clear", ops::clear),
            Action::ClearLayer => {
                if let Some(d) = self.state.active_mut() {
                    let li = d.doc.state().active;
                    if d.doc.state().layers[li].editable() {
                        let saved = d.doc.state_mut().selection.take();
                        let r = ops::clear(d.doc.state_mut(), li);
                        d.doc.state_mut().selection = saved;
                        d.doc.mark_dirty_rect(r);
                        d.doc.commit("Clear Layer");
                    }
                }
            }
            Action::FillForeground => {
                let c = self.state.fg;
                self.edit_layer("Fill", move |s, li| ops::fill(s, li, c));
            }
            Action::FillBackground => {
                let c = self.state.bg;
                self.edit_layer("Fill", move |s, li| ops::fill(s, li, c));
            }
            Action::Preferences => {
                self.state.dialogs.settings =
                    Some(dialogs::settings::SettingsDialog::new(dialogs::settings::Page::General));
            }
            Action::KeyboardShortcuts => {
                self.state.dialogs.settings =
                    Some(dialogs::settings::SettingsDialog::new(dialogs::settings::Page::Shortcuts));
            }
            Action::ImageSize => dialogs::open_image_size(&mut self.state),
            Action::CanvasSize => dialogs::open_canvas_size(&mut self.state),
            Action::CropToSelection => {
                if let Some(d) = self.state.active_mut() {
                    if let Some(b) = d.doc.state().selection_mask().map(|m| m.bounds()) {
                        if !b.is_empty() {
                            ops::crop(d.doc.state_mut(), b);
                            d.doc.resized();
                            d.doc.commit("Crop");
                            d.needs_full_upload = true;
                            d.sel_outline = None;
                            d.view.fit(b.w as u32, b.h as u32);
                        }
                    }
                }
            }
            Action::FlipHorizontal => self.orient("Flip Horizontal", ops::Orient::FlipH),
            Action::FlipVertical => self.orient("Flip Vertical", ops::Orient::FlipV),
            Action::Rotate90CW => self.orient("Rotate 90° CW", ops::Orient::Rotate(1)),
            Action::Rotate90CCW => self.orient("Rotate 90° CCW", ops::Orient::Rotate(3)),
            Action::Rotate180 => self.orient("Rotate 180°", ops::Orient::Rotate(2)),
            Action::InvertColors => self.edit_layer("Invert", ops::invert_colors),
            Action::Desaturate => self.edit_layer("Desaturate", ops::desaturate),
            Action::BrightnessContrast => dialogs::open_adjust(&mut self.state, AdjustKind::BrightnessContrast),
            Action::HueSaturation => dialogs::open_adjust(&mut self.state, AdjustKind::HueSaturation),
            Action::LastFilter => dialogs::filter::repeat_last(&mut self.state),
            Action::LastFilterDialog => dialogs::filter::reopen_last(&mut self.state),
            a if a.category() == Category::Filter => dialogs::filter::open(&mut self.state, a),
            Action::NewLayer => {
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let name = s.unique_layer_name("Layer");
                    s.add_layer(name, None);
                    d.doc.commit("New Layer");
                }
            }
            Action::DuplicateLayer => {
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let a = s.active;
                    s.duplicate_layer(a);
                    d.doc.mark_all_dirty();
                    d.doc.commit("Duplicate Layer");
                }
            }
            Action::DeleteLayer => {
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let a = s.active;
                    if s.remove_layer(a).is_some() {
                        d.doc.mark_all_dirty();
                        d.doc.commit("Delete Layer");
                    } else {
                        self.state.toasts.push(Level::Info, "A document needs at least one layer.");
                    }
                }
            }
            Action::MergeDown => {
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let a = s.active;
                    if s.merge_down(a) {
                        d.doc.mark_all_dirty();
                        d.doc.commit("Merge Down");
                    }
                }
            }
            Action::MergeVisible => self.edit_doc("Merge Visible", |s| s.merge_visible(), false),
            Action::Flatten => self.edit_doc("Flatten Image", |s| s.flatten(), false),
            Action::LayerUp | Action::LayerDown | Action::LayerToTop | Action::LayerToBottom => {
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let a = s.active;
                    let n = s.layers.len();
                    let to = match action {
                        Action::LayerUp => (a + 1).min(n - 1),
                        Action::LayerDown => a.saturating_sub(1),
                        Action::LayerToTop => n - 1,
                        _ => 0,
                    };
                    if to != a {
                        s.move_layer(a, to);
                        d.doc.mark_all_dirty();
                        d.doc.commit("Reorder Layers");
                    }
                }
            }
            Action::SelectLayerAbove | Action::SelectLayerBelow => {
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let n = s.layers.len();
                    s.active = if action == Action::SelectLayerAbove {
                        (s.active + 1).min(n - 1)
                    } else {
                        s.active.saturating_sub(1)
                    };
                }
            }
            Action::ToggleLayerVisibility => self.toggle_prop("Toggle Visibility", |p| p.visible = !p.visible),
            Action::ToggleAlphaLock => {
                self.toggle_prop("Lock Transparent Pixels", |p| p.alpha_locked = !p.alpha_locked)
            }
            Action::ToggleLayerLock => self.toggle_prop("Lock Layer", |p| p.locked = !p.locked),
            Action::LayerProperties => dialogs::open_layer_props(&mut self.state),
            Action::FlipLayerHorizontal => {
                if let Some(d) = self.state.active_mut() {
                    let li = d.doc.state().active;
                    ops::flip_layer_horizontal(d.doc.state_mut(), li);
                    d.doc.mark_all_dirty();
                    d.doc.commit("Flip Layer Horizontal");
                }
            }
            Action::FlipLayerVertical => {
                if let Some(d) = self.state.active_mut() {
                    let li = d.doc.state().active;
                    ops::flip_layer_vertical(d.doc.state_mut(), li);
                    d.doc.mark_all_dirty();
                    d.doc.commit("Flip Layer Vertical");
                }
            }
            Action::SelectAll => {
                if let Some(id) = active {
                    crate::tools::select_all(&mut self.state, id);
                }
            }
            Action::Deselect => {
                if let Some(id) = active {
                    crate::tools::deselect(&mut self.state, id);
                }
            }
            Action::InvertSelection => {
                if let Some(id) = active {
                    crate::tools::invert_selection(&mut self.state, id);
                }
            }
            Action::SelectLayerContent => {
                if let Some(id) = active {
                    crate::tools::select_layer_content(&mut self.state, id);
                }
            }
            Action::ZoomIn => self.with_view(|v, _, _| v.zoom_in(None)),
            Action::ZoomOut => self.with_view(|v, _, _| v.zoom_out(None)),
            Action::ZoomFit => self.with_view(|v, w, h| v.fit(w, h)),
            Action::Zoom100 => self.with_view(|v, _, _| v.set_zoom(1.0, None)),
            Action::Zoom200 => self.with_view(|v, _, _| v.set_zoom(2.0, None)),
            Action::RotateViewLeft => self.with_view(|v, _, _| v.rotate_by(-15f32.to_radians(), None)),
            Action::RotateViewRight => self.with_view(|v, _, _| v.rotate_by(15f32.to_radians(), None)),
            Action::FlipViewHorizontal => self.with_view(|v, _, _| v.flip_h = !v.flip_h),
            Action::ResetView => {
                if self.state.session.is_some() {
                    self.state.cancel_session();
                } else if self.state.tool_opts.crop_rect.is_some() {
                    self.state.tool_opts.crop_rect = None;
                } else {
                    self.with_view(|v, _, _| {
                        v.reset_rotation();
                        v.flip_h = false;
                    });
                }
            }
            Action::TogglePixelGrid => {
                self.state.settings.canvas.show_pixel_grid = !self.state.settings.canvas.show_pixel_grid
            }
            Action::ToggleFullscreen => {
                self.state.fullscreen = !self.state.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.state.fullscreen));
            }
            Action::TogglePanels => self.state.panels_hidden = !self.state.panels_hidden,
            Action::ToggleSymmetryHorizontal => self.state.symmetry.horizontal = !self.state.symmetry.horizontal,
            Action::ToggleSymmetryVertical => self.state.symmetry.vertical = !self.state.symmetry.vertical,
            Action::SymmetryOff => {
                self.state.symmetry.clear();
                self.state.symmetry_pick_center = false;
            }
            Action::SymmetrySetCenter => {
                self.state.symmetry_pick_center = true;
                self.state.toasts.push(Level::Info, "Click the canvas to place the symmetry center.");
            }
            Action::SymmetryResetCenter => self.state.symmetry.center = None,
            Action::BrushSizeUp | Action::BrushSizeDown => {
                if let Some(b) = self.state.current_brush_mut() {
                    let step = if b.size < 10.0 {
                        1.0
                    } else if b.size < 50.0 {
                        2.0
                    } else if b.size < 200.0 {
                        10.0
                    } else {
                        25.0
                    };
                    b.size = if action == Action::BrushSizeUp { b.size + step } else { b.size - step };
                    b.clamp();
                }
            }
            Action::BrushHardnessUp | Action::BrushHardnessDown => {
                if let Some(b) = self.state.current_brush_mut() {
                    b.hardness += if action == Action::BrushHardnessUp { 0.25 } else { -0.25 };
                    b.clamp();
                }
            }
            Action::SwapColors => std::mem::swap(&mut self.state.fg, &mut self.state.bg),
            Action::DefaultColors => {
                self.state.fg = qsketch_core::Rgba8::BLACK;
                self.state.bg = qsketch_core::Rgba8::WHITE;
            }
            Action::ShowTools => self.state.show_panel_requests.push(PanelKind::Tools),
            Action::ShowLayers => self.state.show_panel_requests.push(PanelKind::Layers),
            Action::ShowHistory => self.state.show_panel_requests.push(PanelKind::History),
            Action::ShowColor => self.state.show_panel_requests.push(PanelKind::Color),
            Action::ShowSwatches => self.state.show_panel_requests.push(PanelKind::Swatches),
            Action::ShowNavigator => self.state.show_panel_requests.push(PanelKind::Navigator),
            Action::ShowBrushes => self.state.show_panel_requests.push(PanelKind::Brushes),
            Action::ShowBrushSettings => self.state.show_panel_requests.push(PanelKind::BrushSettings),
            Action::ShowInfo => self.state.show_panel_requests.push(PanelKind::Info),
            Action::ResetLayout => self.state.layout_reset_requested = true,
            Action::About => self.state.dialogs.about = true,
            Action::CheckForUpdates => match self.state.settings.update.effective_manifest_url() {
                Some(url) => self.state.updater.check(url, true),
                // No URL configured: take the user to the field that needs filling in.
                None => {
                    self.state.dialogs.settings =
                        Some(dialogs::settings::SettingsDialog::new(dialogs::settings::Page::General));
                }
            },
            _ => {}
        }
    }

    /// Rotate / flip the floating paste, the selected pixels, or the whole canvas,
    /// whichever is the current target.
    fn orient(&mut self, label: &str, o: ops::Orient) {
        if let Some(f) = self.state.floating.as_mut() {
            f.orient(o);
            return;
        }
        let has_sel = self.state.active().is_some_and(|d| d.doc.state().selection.is_some());
        if has_sel {
            let label = format!("{label} Selection");
            self.edit_layer(&label, move |s, li| ops::orient_selection(s, li, o));
            if let Some(d) = self.state.active_mut() {
                d.sel_outline = None;
            }
            return;
        }
        let label = format!("{label} Canvas");
        match o {
            ops::Orient::FlipH => self.edit_doc(&label, ops::flip_horizontal, false),
            ops::Orient::FlipV => self.edit_doc(&label, ops::flip_vertical, false),
            ops::Orient::Rotate(t) => self.edit_doc(&label, move |s| ops::rotate_canvas(s, t), t % 2 == 1),
        }
    }

    /// Apply an operation to the active layer (respecting the selection) and commit.
    fn edit_layer(&mut self, label: &str, f: impl FnOnce(&mut qsketch_core::DocState, usize) -> qsketch_core::IRect) {
        self.state.cancel_session();
        crate::tools::floating::commit(&mut self.state);
        crate::tools::text::commit(&mut self.state);
        let Some(d) = self.state.active_mut() else { return };
        let li = d.doc.state().active;
        if !d.doc.state().layers[li].editable() {
            self.state.toasts.push(Level::Info, "The active layer is locked or hidden.");
            return;
        }
        let r = f(d.doc.state_mut(), li);
        if r.is_empty() {
            return;
        }
        d.doc.mark_dirty_rect(r);
        d.doc.commit(label);
    }

    /// Apply a whole-document operation and commit.
    fn edit_doc(&mut self, label: &str, f: impl FnOnce(&mut qsketch_core::DocState), resizes: bool) {
        self.state.cancel_session();
        crate::tools::floating::commit(&mut self.state);
        crate::tools::text::commit(&mut self.state);
        let Some(d) = self.state.active_mut() else { return };
        f(d.doc.state_mut());
        if resizes {
            d.doc.resized();
            d.needs_full_upload = true;
            let (w, h) = (d.doc.width(), d.doc.height());
            d.view.fit(w, h);
        } else {
            d.doc.mark_all_dirty();
        }
        d.sel_outline = None;
        d.doc.commit(label);
    }

    fn toggle_prop(&mut self, label: &str, f: impl FnOnce(&mut qsketch_core::layer::LayerProps)) {
        let Some(d) = self.state.active_mut() else { return };
        let li = d.doc.state().active;
        f(&mut d.doc.state_mut().layers[li].props);
        d.doc.mark_all_dirty();
        d.doc.commit(label);
    }

    fn with_view(&mut self, f: impl FnOnce(&mut crate::canvas::view::CanvasView, u32, u32)) {
        if let Some(d) = self.state.active_mut() {
            let (w, h) = (d.doc.width(), d.doc.height());
            f(&mut d.view, w, h);
        }
    }
}

impl eframe::App for QSketchApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Theme / icon changes from preferences.
        let ui_now = &self.state.settings.ui;
        if *ui_now != self.applied_theme.0 {
            let was = &self.applied_theme.0;
            if ui_now.icon_set.filled() != was.icon_set.filled() {
                theme::install_fonts(&ctx, ui_now.icon_set.filled());
            }
            if ui_now.icon_set != was.icon_set || ui_now.icon_overrides != was.icon_overrides {
                crate::ui::iconset::configure(ui_now.icon_set, &ui_now.icon_overrides);
            }
            if ui_now.theme != was.theme || ui_now.custom_palette != was.custom_palette || ui_now.scale != was.scale {
                theme::apply(&ctx, &ui_now.palette(), ui_now.scale);
            }
            self.applied_theme = (ui_now.clone(), ui_now.scale);
        }
        if !self.state.settings.ui.show_tooltips {
            ctx.global_style_mut(|s| s.interaction.tooltip_delay = 1e9);
        }

        // OS close button.
        if ctx.input(|i| i.viewport().close_requested()) {
            let unsaved =
                self.state.docs.iter().any(|d| d.doc.is_modified()) && self.state.settings.general.confirm_close;
            if unsaved && !self.state.quit_requested {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.state.quit_requested = true;
            } else {
                self.persist();
            }
        }

        if let Some(t) = self.tablet.as_mut() {
            t.pump(&mut self.state);
        }
        self.handle_dropped_files(&ctx);
        self.handle_keyboard(&ctx);
        self.state.autosave.tick(&self.state.docs, &self.state.settings.general, self.state.session.is_some(), &ctx);
        if self.state.updater.ctx.is_none() {
            self.state.updater.ctx = Some(ctx.clone());
        }
        self.auto_update_check();
        if self.state.updater.poll() {
            ctx.request_repaint();
            // An automatic check stays quiet about a version the user skipped.
            let skipped = &self.state.settings.update.skipped_version;
            if !self.state.updater.manual && self.state.updater.info.as_ref().is_some_and(|i| &i.version == skipped) {
                self.state.updater.show_dialog = false;
            }
        }
        self.process_requests(&ctx);

        // --- chrome -----------------------------------------------------------
        let hidden = self.state.panels_hidden;
        let p = self.state.settings.ui.palette();
        let native_frame = self.state.settings.ui.native_frame;
        if native_frame != self.applied_native_frame {
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(native_frame));
            self.applied_native_frame = native_frame;
        }
        let tex = self.state.settings.ui.texture.clone();
        let texture = |ui: &Ui| {
            if tex.enabled() {
                crate::ui::chrome::paint_ui(ui, &p, &tex);
            }
        };
        // No top margin: the strip must start on the very first row of the
        // window so the menus and window buttons reach the screen edge.
        let bar_frame =
            egui::Frame::new().fill(p.panel_alt).inner_margin(egui::Margin { left: 4, right: 4, top: 0, bottom: 1 });
        egui::Panel::top("menu_bar").frame(bar_frame).show(ui, |ui| {
            texture(ui);
            if native_frame {
                self.menu_bar(ui);
            } else {
                self.title_strip(ui);
            }
        });
        if !native_frame {
            self.edge_resize(&ctx);
        }
        if !hidden {
            let opt_frame = egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(4, 0));
            egui::Panel::top("tool_options").resizable(false).frame(opt_frame).show(ui, |ui| {
                texture(ui);
                // Fixed height so the bar never jumps when its contents change
                // (e.g. a temporary tool while Space is held).
                ui.set_min_height(crate::panels::tool_options::BAR_HEIGHT);
                ui.set_max_height(crate::panels::tool_options::BAR_HEIGHT);
                ui.add_space(1.0);
                crate::panels::tool_options::ui(ui, &mut self.state);
            });
            let status_frame = egui::Frame::new().fill(p.panel_alt).inner_margin(egui::Margin::symmetric(6, 1));
            egui::Panel::bottom("status_bar").frame(status_frame).show(ui, |ui| {
                texture(ui);
                self.status_bar(ui);
            });
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.bg)).show(ui, |ui| {
            if hidden {
                if let Some(id) = self.state.active_doc {
                    crate::canvas::show(ui, &mut self.state, id);
                } else {
                    ui.centered_and_justified(|ui| ui.label("No document open. Press Tab to show panels."));
                }
            } else {
                self.workspace.show(ui, &mut self.state);
            }
        });

        dialogs::show_all(&ctx, &mut self.state);
        self.state.toasts.show(&ctx);

        // Deferred actions from panels/menus.
        let pending = std::mem::take(&mut self.state.pending);
        for a in pending {
            self.perform(a, &ctx);
        }
        // Actions may have created documents that still need tabs.
        let ids: Vec<DocId> = self.state.docs.iter().map(|d| d.id).collect();
        for id in ids {
            if !self.workspace.is_panel_open(&PanelKind::Document(id)) {
                self.workspace.add_document(id);
                ctx.request_repaint();
            }
        }

        if self.last_settings_save.elapsed() > Duration::from_secs(30) {
            self.persist();
        }
        // TextEdit publishes an IME rect every frame it has focus; use that to
        // know when single-key shortcuts must stay out of the way.
        self.text_editing = ctx.output(|o| o.ime.is_some());
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.persist();
    }

    fn on_exit(&mut self) {
        self.persist();
    }

    fn auto_save_interval(&self) -> Duration {
        Duration::from_secs(60)
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let p = self.state.settings.ui.palette();
        let [r, g, b, _] = p.bg.to_normalized_gamma_f32();
        [r, g, b, 1.0]
    }
}

/// Right edge of the last top-level menu button this frame (temp data).
fn menu_bar_right_id() -> egui::Id {
    egui::Id::new("qsketch_menu_bar_right")
}

/// A top-level menu-bar button that also switches menus on hover: with one
/// bar menu open, hovering another title opens that one instead (egui 0.36's
/// `MenuBar` only switches on click).
fn top_menu<R>(ui: &mut Ui, label: &str, contents: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
    let r = ui.menu_button(label, contents);
    let ctx = ui.ctx().clone();
    let right = r.response.rect.right();
    ctx.data_mut(|d| {
        let v: &mut f32 = d.get_temp_mut_or_default(menu_bar_right_id());
        *v = v.max(right);
    });
    let mine = egui::Popup::default_response_id(&r.response);
    // Fitts's law: when the bar sits at the top of the window, let the
    // menu accept clicks all the way up to the top edge (the button itself
    // is a few points shorter than the strip), so a maximized window opens
    // menus with the pointer slammed against the screen edge.
    let top = ctx.content_rect().top();
    let rect = r.response.rect;
    let mut hovered = r.response.hovered();
    if rect.top() > top && rect.top() - top < 12.0 {
        let ext = egui::Rect::from_min_max(egui::pos2(rect.left(), top), egui::pos2(rect.right(), rect.top()));
        let ext_resp = ui.interact(ext, r.response.id.with("top_ext"), egui::Sense::click());
        if ext_resp.clicked() {
            egui::Popup::toggle_id(&ctx, mine);
        }
        hovered |= ext_resp.hovered();
    }
    // Remember every bar menu's popup id so we only switch between *these*
    // popups, not e.g. an open combo box elsewhere.
    let key = egui::Id::new("qsketch_menu_bar_popups");
    let ids: Vec<egui::Id> = ctx.data_mut(|d| {
        let v: &mut Vec<egui::Id> = d.get_temp_mut_or_default(key);
        if !v.contains(&mine) {
            v.push(mine);
        }
        v.clone()
    });
    if hovered
        && !egui::Popup::is_id_open(&ctx, mine)
        && ids.iter().any(|id| *id != mine && egui::Popup::is_id_open(&ctx, *id))
    {
        egui::Popup::open_id(&ctx, mine);
        ctx.request_repaint();
    }
    r
}

fn open_in_file_manager(dir: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

#[allow(dead_code)]
fn category_label(c: Category) -> &'static str {
    c.label()
}
