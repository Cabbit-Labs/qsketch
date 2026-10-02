//! The eframe application: menus, keyboard dispatch, action execution and
//! document lifecycle around the dockable workspace.

use std::time::{Duration, Instant};

use egui::{Context, Key, RichText, Ui};
use qsketch_core::ops;
use qsketch_core::Document;
use qsketch_core::Filter;

use crate::actions::{Action, Category, Trigger};
use crate::canvas::render::CanvasRenderer;
use crate::dialogs::{self, AfterClose, CloseConfirm};
use crate::settings::{NewDocBackground, Settings};
use crate::state::{AppState, DocId, TempReason};
use crate::tools::ModifyKind;
use crate::tools::ToolKind;
use crate::ui::menus;
use crate::ui::toasts::Level;
use crate::ui::{icons, theme};
use crate::workspace::{PanelKind, Workspace};

pub struct QSketchApp {
    state: AppState,
    /// Single-instance listener; other launches hand their files here.
    instance: crate::single_instance::Primary,
    workspace: Workspace,
    tablet: Option<crate::tablet::Tablet>,
    wintab: Option<crate::wintab::WinTab>,
    applied_theme: (crate::settings::UiSettings, f32),
    /// Decorations state last sent to the OS (see `UiSettings::native_frame`).
    applied_native_frame: bool,
    /// App icon for the custom title strip.
    app_icon: egui::TextureHandle,
    /// Startup animation while it runs.
    splash: Option<crate::ui::splash::Splash>,
    cloak: Option<crate::startup_cloak::StartupCloak>,
    trace: crate::startup_trace::StartupTrace,
    /// Frames painted so far (saturating).
    frames: u32,
    last_settings_save: Instant,
    /// Last plain `Q` press, for the double-tap view reset.
    last_q_press: Option<Instant>,
    /// Attempts made to get a fullscreen window to actually cover the
    /// monitor, and when the last one was sent.
    fullscreen_tries: u8,
    fullscreen_fix_at: Option<Instant>,
    last_title: String,
    /// A text field had focus during the previous frame (shortcuts are suspended).
    text_editing: bool,
    /// A clipboard chord already fired via `Event::Copy/Cut/Paste` this press,
    /// so the key-release fallback must not fire it again.
    clipboard_chord_fired: bool,
    /// Keys whose press event reached us (a release without one means egui-winit
    /// swallowed the press as a clipboard chord).
    keys_seen_pressed: std::collections::HashSet<Key>,
    /// Shift was down at some point during the current Ctrl hold. A paste
    /// chord that only shows up on the V *release* (image clipboard: no
    /// `Event::Paste`) must read Shift from the hold, not from the release
    /// event, where Shift is often already up.
    shift_during_ctrl: bool,
    auto_update_checked: bool,
}

/// Keep painting for a moment after asking the window manager to resize the
/// window (fullscreen, maximize). The new size arrives asynchronously and a
/// window that is otherwise idle would keep showing the frame it drew at the
/// old size, leaving the newly exposed strip unpainted.
fn resize_settled(ctx: &Context) {
    ctx.request_repaint();
    for ms in [16, 50, 120, 250, 500] {
        ctx.request_repaint_after(std::time::Duration::from_millis(ms));
    }
}

impl QSketchApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        files: Vec<std::path::PathBuf>,
        instance: crate::single_instance::Primary,
        trace: crate::startup_trace::StartupTrace,
    ) -> Self {
        let mut settings = Settings::load();
        settings.sanitize();
        theme::install_fonts(&cc.egui_ctx, settings.ui.icon_set.filled());
        crate::ui::iconset::configure(settings.ui.icon_set, &settings.ui.icon_overrides);
        theme::apply(&cc.egui_ctx, &settings.ui.palette(), settings.ui.scale, settings.ui.shape);
        // egui grows/shrinks the whole UI on Ctrl+= / Ctrl+- by default, on
        // top of the canvas zoom those keys mean here; the UI scale lives in
        // Preferences instead.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
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
        let cloak = crate::startup_cloak::StartupCloak::install(cc, state.settings.window_maximized);
        crate::win_pointer::install(cc);
        let wintab = if state.settings.tablet.use_wintab { crate::wintab::WinTab::new(cc) } else { None };
        let tablet = if wintab.is_none() && state.settings.tablet.use_octotablet {
            crate::tablet::Tablet::new(cc)
        } else {
            None
        };

        let mut app = Self {
            state,
            instance,
            workspace,
            tablet,
            wintab,
            applied_theme,
            applied_native_frame,
            fullscreen_tries: 0,
            fullscreen_fix_at: None,
            app_icon,
            splash: None,
            cloak,
            trace,
            frames: 0,
            last_settings_save: Instant::now(),
            last_q_press: None,
            last_title: String::new(),
            text_editing: false,
            clipboard_chord_fired: false,
            keys_seen_pressed: Default::default(),
            shift_during_ctrl: false,
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
        for (orig, b) in std::mem::take(&mut self.state.restore_requests) {
            if let Some(id) = crate::backups::restore(&mut self.state, &orig, &b) {
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
        for p in std::mem::take(&mut self.state.close_floating_requests) {
            self.workspace.close_if_floating(&p);
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
                    // Whatever the confirmation flow missed (confirm_close off,
                    // or a file on disk that no longer matches) gets a
                    // recovery snapshot before the process goes away.
                    self.state.autosave.flush_for_exit(&self.state.docs);
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

    /// Remember where the window is so the next launch can open there.
    fn record_window_geometry(&mut self, ctx: &Context) {
        let (outer, inner, maximized) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.maximized.unwrap_or(false))
        });
        // Only our own flag: the viewport's `fullscreen` reads true for a
        // plainly windowed window on some platforms (see ToggleFullscreen).
        if self.state.fullscreen {
            return;
        }
        self.state.settings.window_maximized = maximized;
        if maximized {
            return;
        }
        // X11 without a window manager reports no outer rect; the inner one
        // is then also the window's position.
        if let Some(inner) = inner {
            let origin = outer.map(|o| o.min).unwrap_or(inner.min);
            if inner.width() >= 100.0 && inner.height() >= 100.0 {
                self.state.settings.window_rect = Some([origin.x, origin.y, inner.width(), inner.height()]);
            }
        }
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
        // Q alone (no modifiers, so Ctrl+Q still quits) quick-rotates the view.
        let quick_rotate = ctx.input(|i| i.key_down(Key::Q) && i.modifiers.is_none());
        let quick_move_held = {
            let mods = ctx.input(|i| i.modifiers);
            self.state.settings.mouse.quick_move_modifiers_held(mods)
                && !matches!(self.state.tool, ToolKind::Move | ToolKind::Hand | ToolKind::Zoom | ToolKind::RotateView)
        };
        let in_stroke = self.state.session.is_some();
        let middle_down = ctx.input(|i| i.pointer.middle_down());
        // Eraser end of the stylus in range (tablet backend or the Windows
        // pointer flags): temporarily erase, like any pen-aware paint app.
        let eraser_end = self.state.settings.tablet.eraser_tip_switches_tool
            && (self.state.pen.eraser || crate::win_pointer::eraser())
            && self.state.tool != ToolKind::Eraser;
        let pick_held = {
            let mods = ctx.input(|i| i.modifiers);
            self.state.settings.mouse.pick_modifiers_held(mods) && self.state.tool.uses_color()
        };
        match self.state.temp_tool {
            Some((_, TempReason::Space)) if !space => self.state.temp_tool = None,
            Some((_, TempReason::QuickRotate)) if !quick_rotate => {
                self.state.temp_tool = None;
                // A pointer-following rotation has no button release to end it.
                if matches!(self.state.session, Some(crate::tools::ToolSession::RotateDrag { .. })) {
                    self.state.session = None;
                    self.state.session_doc = None;
                }
            }
            Some((_, TempReason::Pick)) if !pick_held => self.state.temp_tool = None,
            Some((_, TempReason::QuickMove)) if !quick_move_held && !in_stroke => self.state.temp_tool = None,
            Some((_, TempReason::Middle)) if !middle_down && !in_stroke => self.state.temp_tool = None,
            Some((_, TempReason::EraserTip)) if !eraser_end && !in_stroke => self.state.temp_tool = None,
            _ => {}
        }
        if !in_stroke && self.state.temp_tool.is_none() && eraser_end {
            self.state.temp_tool = Some((ToolKind::Eraser, TempReason::EraserTip));
        }
        if !wants_text && !dialog_open && !in_stroke && self.state.temp_tool.is_none() {
            if space {
                self.state.temp_tool = Some((ToolKind::Hand, TempReason::Space));
            } else if quick_rotate {
                self.state.temp_tool = Some((ToolKind::RotateView, TempReason::QuickRotate));
            } else if pick_held {
                self.state.temp_tool = Some((ToolKind::Eyedropper, TempReason::Pick));
            } else if quick_move_held && !ctx.input(|i| i.keys_down.iter().any(|k| !matches!(k, Key::Space))) {
                self.state.temp_tool = Some((ToolKind::Move, TempReason::QuickMove));
            }
        }

        let events = ctx.input(|i| i.events.clone());
        // Track Shift across the Ctrl hold (see `shift_during_ctrl`).
        let (ctrl_now, shift_now) = ctx.input(|i| (i.modifiers.command || i.modifiers.ctrl, i.modifiers.shift));
        if !ctrl_now {
            self.shift_during_ctrl = false;
        } else if shift_now {
            self.shift_during_ctrl = true;
        }
        let shift_held = self.shift_during_ctrl || shift_now;
        if wants_text || dialog_open {
            return;
        }
        let (mut consume_wheel, mut consume_extra) = (false, false);
        for ev in events {
            // Double-tap Q: reset the view rotation.
            if let egui::Event::Key { key: Key::Q, pressed: true, repeat: false, modifiers, .. } = &ev {
                if modifiers.is_none() && self.state.settings.canvas.quick_rotate_double_tap_reset {
                    let now = Instant::now();
                    let double = self.last_q_press.is_some_and(|t| now.duration_since(t) < Duration::from_millis(400));
                    self.last_q_press = if double { None } else { Some(now) };
                    if double {
                        self.state.session = None;
                        self.state.session_doc = None;
                        self.with_view(|v, _, _| v.reset_rotation());
                        continue;
                    }
                }
            }
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
                    self.perform(if shift_held { Action::PasteInPlace } else { Action::Paste }, ctx);
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
                            let mods = egui::Modifiers {
                                command: true,
                                ctrl: true,
                                shift: modifiers.shift || shift_held,
                                ..*modifiers
                            };
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
            // Wheel notches and the extra mouse buttons are bindable too.
            // A bound one is consumed so the canvas doesn't also zoom, pan or
            // start a tool press with it.
            match &ev {
                egui::Event::MouseWheel { delta, modifiers, .. } => {
                    if delta.y != 0.0 {
                        let t = if delta.y > 0.0 { Trigger::WheelUp } else { Trigger::WheelDown };
                        if let Some(action) = self.state.keymap.lookup_trigger(t, *modifiers) {
                            consume_wheel = true;
                            self.perform(action, ctx);
                        }
                    }
                    continue;
                }
                egui::Event::PointerButton { button, pressed: true, modifiers, .. } => {
                    if let Some(t) = Trigger::from_pointer_button(*button) {
                        consume_extra = true;
                        if let Some(action) = self.state.keymap.lookup_trigger(t, *modifiers) {
                            self.perform(action, ctx);
                        }
                    }
                    continue;
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
                if matches!(self.state.session, Some(crate::tools::ToolSession::PolyLasso { .. })) {
                    if key == Key::Enter {
                        crate::tools::select::close_poly_lasso(&mut self.state, id, modifiers);
                        continue;
                    }
                    if key == Key::Backspace {
                        crate::tools::select::poly_lasso_undo_point(&mut self.state);
                        continue;
                    }
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
            // Shift+B with a selection tool up opens Border Selection instead
            // of switching to the Brush: the marquee stays the active tool.
            if key == egui::Key::B
                && modifiers.shift
                && !modifiers.command
                && !modifiers.alt
                && self.state.effective_tool().is_selection()
                && !repeat
            {
                self.perform(Action::BorderSelection, ctx);
                continue;
            }
            if let Some(action) = self.state.keymap.lookup(key, modifiers) {
                if repeat && !action.repeatable() {
                    continue;
                }
                self.perform(action, ctx);
            }
        }
        if consume_wheel || consume_extra {
            // Both lists: `i.events` is what most widgets read, `i.raw.events`
            // is what the canvas reads. Leaving the raw copy in place is why a
            // wheel chord used to run its command and move the canvas too.
            let keep = |e: &egui::Event| match e {
                egui::Event::MouseWheel { .. } | egui::Event::Zoom(_) => !consume_wheel,
                egui::Event::PointerButton { button, .. } => {
                    !(consume_extra && Trigger::from_pointer_button(*button).is_some())
                }
                _ => true,
            };
            ctx.input_mut(|i| {
                i.events.retain(keep);
                i.raw.events.retain(keep);
                if consume_wheel {
                    i.smooth_scroll_delta = egui::Vec2::ZERO;
                }
            });
        }
    }

    /// Fullscreen has to cover the whole monitor. Some window managers only
    /// maximize an undecorated window when asked, which leaves the strip
    /// where the taskbar sits uncovered, so check the result and insist.
    fn enforce_fullscreen(&mut self, ctx: &Context) {
        if !self.state.fullscreen {
            self.fullscreen_tries = 0;
            self.fullscreen_fix_at = None;
            return;
        }
        let (inner, monitor) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().monitor_size));
        let (Some(inner), Some(monitor)) = (inner, monitor) else { return };
        // A few points of slack: window managers round sizes.
        let short = monitor.x - inner.width() > 4.0 || monitor.y - inner.height() > 4.0;
        if !short {
            self.fullscreen_tries = 0;
            return;
        }
        // Give the window manager a moment between attempts, and stop after
        // a few so we never fight it in a loop.
        if self.fullscreen_tries >= 3
            || self.fullscreen_fix_at.is_some_and(|t| t.elapsed() < Duration::from_millis(400))
        {
            return;
        }
        self.fullscreen_tries += 1;
        self.fullscreen_fix_at = Some(Instant::now());
        log::warn!(
            "fullscreen window is {:.0}x{:.0} on a {:.0}x{:.0} monitor (attempt {})",
            inner.width(),
            inner.height(),
            monitor.x,
            monitor.y,
            self.fullscreen_tries
        );
        if self.fullscreen_tries < 3 {
            // Maximized and fullscreen together confuse some window
            // managers; drop the first, then ask again.
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        } else if inner.min.x.abs() < 2.0 && inner.min.y.abs() < 2.0 {
            // Last resort, and only when the window sits at the origin (so a
            // second monitor can't be dragged into): place it over the whole
            // screen by hand.
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(0.0, 0.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(monitor));
        }
        resize_settled(ctx);
    }

    /// Advance the active document's work time while the user is
    /// interacting with qsketch (input within the last `IDLE_AFTER`); a
    /// pause, a minimized window or a stalled frame does not count.
    fn tick_work_time(&mut self, ctx: &Context) {
        const IDLE_AFTER: Duration = Duration::from_secs(30);
        let now = Instant::now();
        let busy = self.state.session.is_some()
            || self.state.floating.as_ref().is_some_and(|f| f.drag.is_some())
            || ctx.input(|i| !i.raw.events.is_empty() || i.pointer.any_down() || i.pointer.is_moving());
        if busy {
            self.state.last_activity = Some(now);
        }
        let prev = self.state.work_clock.replace(now);
        let (Some(prev), Some(last)) = (prev, self.state.last_activity) else { return };
        if now.duration_since(last) > IDLE_AFTER {
            return;
        }
        let dt = now.duration_since(prev).as_secs_f64().min(1.0);
        if let Some(d) = self.state.active_mut() {
            d.doc.stats.work_secs += dt;
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
        if menus::item(ui, false, action.label(), text, enabled).clicked() {
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
            resize_settled(&ctx);
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
                    resize_settled(&ctx);
                }
                if btn(ui, icons::MINUS, "Minimize", false) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                ui.add_space(8.0);
                if update_ready {
                    let label = RichText::new(format!("{} Update", icons::ARROW_CIRCLE_DOWN))
                        .color(egui::Color32::WHITE)
                        .small();
                    let r = ui.add(
                        egui::Button::new(label)
                            .fill(p.accent)
                            .corner_radius(crate::ui::theme::radius(4))
                            .min_size(egui::vec2(0.0, 18.0)),
                    );
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
        let (has_pal, pal_locked) = self
            .state
            .active()
            .map(|d| (!d.doc.state().palette.is_empty(), d.doc.state().palette_lock))
            .unwrap_or((false, false));
        let (can_undo, can_redo, has_sel, layers, can_merge_down, active_is_group, sel_or_content) =
            match self.state.active() {
                Some(d) => {
                    let st = d.doc.state();
                    (
                        d.doc.can_undo(),
                        d.doc.can_redo(),
                        st.selection.is_some(),
                        st.layers.len(),
                        st.active_layer().is_group() || st.sibling_below(st.active).is_some(),
                        st.active_layer().is_group(),
                        true,
                    )
                }
                None => (false, false, false, 0, false, false, false),
            };
        // Menu buttons span the whole 24 px strip so their own hover
        // highlight (and click) reaches the top edge of the window.
        ui.spacing_mut().interact_size.y = 24.0;
        egui::MenuBar::new().ui(ui, |ui| {
            // Titles butt against each other with their padding inside the
            // button, so there is no dead gap along the bar: the pointer is
            // always over one menu or the next.
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.spacing_mut().button_padding.x = 8.0;
            top_menu(ui, "File", |ui| {
                self.menu_item(ui, Action::NewDocument, true);
                self.menu_item(ui, Action::OpenDocument, true);
                menus::submenu(ui, false, "Open Recent", |ui| {
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
                let doc_path = self.state.active().and_then(|d| d.doc.path.clone());
                ui.add_enabled_ui(doc_path.is_some(), |ui| {
                    menus::submenu(ui, false, "Restore Previous Version", |ui| {
                        let Some(orig) = doc_path.clone() else { return };
                        let backups = crate::backups::list(&orig);
                        if backups.is_empty() {
                            ui.add_enabled(false, egui::Button::new("(no backups yet)"));
                        }
                        for b in backups {
                            let label = format!("{}  ·  {} KB", b.age(), b.bytes / 1024);
                            if ui.button(label).on_hover_text(b.file.display().to_string()).clicked() {
                                self.state.restore_requests.push((orig.clone(), b));
                                ui.close();
                            }
                        }
                    })
                    .0
                    .on_hover_text(
                        "Versions of this file that were overwritten by saving; opens one as a new document",
                    );
                });
                ui.separator();
                self.menu_item(ui, Action::Save, has_doc);
                self.menu_item(ui, Action::SaveAs, has_doc);
                self.menu_item(ui, Action::ExportImage, has_doc);
                self.menu_item(ui, Action::QuickExport, has_doc);
                menus::submenu(ui, false, "Export Scaled", |ui| {
                    ui.label(egui::RichText::new("Nearest-neighbor, for pixel art").weak().small());
                    for a in
                        [Action::ExportScaled2, Action::ExportScaled3, Action::ExportScaled4, Action::ExportScaled8]
                    {
                        self.menu_item(ui, a, has_doc);
                    }
                });
                self.menu_item(ui, Action::ExportTileset, has_doc);
                self.menu_item(ui, Action::ExportAnimation, has_doc);
                menus::submenu(ui, false, "Import", |ui| {
                    self.menu_item(ui, Action::ImportFrames, true);
                    self.menu_item(ui, Action::ImportSpriteSheet, true);
                });
                let has_slices = self.state.active().is_some_and(|d| !d.doc.state().slices.is_empty());
                self.menu_item(ui, Action::ExportSlices, has_slices);
                let (recording, frames, bytes) = self
                    .state
                    .active()
                    .map(|d| (d.doc.timelapse.recording, d.doc.timelapse.frames.len(), d.doc.timelapse.bytes()))
                    .unwrap_or((false, 0, 0));
                menus::submenu(ui, recording, "Timelapse", |ui| {
                    let btn = menus::button(recording, Action::ToggleTimelapse.label())
                    .shortcut_text(self.state.keymap.primary_text(Action::ToggleTimelapse));
                    if ui
                        .add_enabled(has_doc, btn)
                        .on_hover_text("Capture a small frame after every edit; the frames are saved in the .qsk")
                        .clicked()
                    {
                        self.state.pending.push(Action::ToggleTimelapse);
                        ui.close();
                    }
                    ui.separator();
                    self.menu_item(ui, Action::ExportTimelapseGif, frames > 0);
                    self.menu_item(ui, Action::ExportTimelapseFrames, frames > 0);
                    self.menu_item(ui, Action::ClearTimelapse, frames > 0);
                    if has_doc {
                        ui.label(
                            egui::RichText::new(format!(
                                "{frames} frames · {}",
                                crate::ui::widgets::fmt_bytes(bytes as u64)
                            ))
                            .weak()
                            .small(),
                        );
                    }
                });
                ui.separator();
                let shared = self.state.active().is_some_and(|d| d.share.is_some());
                self.menu_item(ui, Action::ShareCanvas, true);
                self.menu_item(ui, Action::StopSharing, shared);
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
                self.menu_item(ui, Action::FreeTransform, has_doc);
                ui.separator();
                self.menu_item(ui, Action::Preferences, true);
            });
            top_menu(ui, "Image", |ui| {
                self.menu_item(ui, Action::ImageSize, has_doc);
                self.menu_item(ui, Action::CanvasSize, has_doc);
                self.menu_item(ui, Action::CropToSelection, has_sel);
                let aspect = self.state.active().map(|d| d.doc.state().pixel_aspect).unwrap_or([1, 1]);
                menus::submenu(ui, false, "Pixel Aspect Ratio", |ui| {
                    ui.label(egui::RichText::new("How wide each pixel shows (for pixel art made for old screens)").weak().small());
                    for (a, v, label) in [
                        (Action::PixelAspectSquare, [1u8, 1u8], "Square (1:1)"),
                        (Action::PixelAspectWide, [2, 1], "Double-wide (2:1)"),
                        (Action::PixelAspectTall, [1, 2], "Double-tall (1:2)"),
                    ] {
                        let btn = menus::button(aspect == v, label)
                            .shortcut_text(self.state.keymap.primary_text(a));
                        if ui.add_enabled(has_doc, btn).clicked() {
                            self.state.pending.push(a);
                            ui.close();
                        }
                    }
                });
                ui.separator();
                menus::submenu(ui, false, "Rotate / Flip", |ui| {
                    self.menu_item(ui, Action::Rotate90CW, has_doc);
                    self.menu_item(ui, Action::Rotate90CCW, has_doc);
                    self.menu_item(ui, Action::Rotate180, has_doc);
                    ui.separator();
                    self.menu_item(ui, Action::FlipHorizontal, has_doc);
                    self.menu_item(ui, Action::FlipVertical, has_doc);
                });
                ui.separator();
                menus::submenu(ui, false, "Adjustments", |ui| {
                    self.menu_item(ui, Action::BrightnessContrast, has_doc);
                    self.menu_item(ui, Action::Levels, has_doc);
                    self.menu_item(ui, Action::Curves, has_doc);
                    self.menu_item(ui, Action::HueSaturation, has_doc);
                    self.menu_item(ui, Action::ColorBalance, has_doc);
                    ui.separator();
                    self.menu_item(ui, Action::Posterize, has_doc);
                    self.menu_item(ui, Action::GradientMap, has_doc);
                    self.menu_item(ui, Action::BlackWhite, has_doc);
                    ui.separator();
                    self.menu_item(ui, Action::Desaturate, has_doc);
                    self.menu_item(ui, Action::InvertColors, has_doc);
                    ui.separator();
                    self.menu_item(ui, Action::ReplaceColor, has_doc);
                });
                menus::submenu(ui, false, "Palette", |ui| {
                    self.menu_item(ui, Action::IndexColors, has_doc);
                    self.menu_item(ui, Action::SnapToPalette, has_pal);
                    ui.separator();
                    let text = self.state.keymap.primary_text(Action::TogglePaletteLock);
                    let btn = menus::button(pal_locked, Action::TogglePaletteLock.label()).shortcut_text(text);
                    if ui.add_enabled(has_doc, btn).clicked() {
                        self.state.pending.push(Action::TogglePaletteLock);
                        ui.close();
                    }
                    self.menu_item(ui, Action::ShowPalette, true);
                });
            });
            top_menu(ui, "Layer", |ui| {
                self.menu_item(ui, Action::NewLayer, has_doc);
                self.menu_item(ui, Action::DuplicateLayer, has_doc);
                self.menu_item(ui, Action::DeleteLayer, layers > 1);
                self.menu_item(ui, Action::LayerProperties, has_doc);
                let is_shape = self.state.active().is_some_and(|d| d.doc.state().active_layer().is_shape());
                let is_text = self.state.active().is_some_and(|d| d.doc.state().active_layer().is_text());
                menus::submenu(ui, false, "Text", |ui| {
                    self.menu_item(ui, Action::NewTextLayer, has_doc);
                    self.menu_item(ui, Action::EditText, is_text);
                    self.menu_item(ui, Action::RasterizeText, is_text);
                    ui.label(
                        egui::RichText::new("Text layers stay editable: click one with the Text tool (T) to change it")
                            .weak()
                            .small(),
                    );
                });
                menus::submenu(ui, false, "Shape", |ui| {
                    self.menu_item(ui, Action::NewShapeLayer, has_doc);
                    self.menu_item(ui, Action::RasterizeShape, is_shape);
                    ui.label(
                        egui::RichText::new("Pixel-art vector shapes: the Shape tool (P) adds and moves points")
                            .weak()
                            .small(),
                    );
                });
                menus::submenu(ui, false, "HD Index Painting", |ui| {
                    self.menu_item(ui, Action::IndexPaintingSetup, has_doc);
                    self.menu_item(ui, Action::NewDitherLayer, has_doc);
                    ui.label(
                        egui::RichText::new(
                            "Paint in grayscale with any brush; Black & White, Posterize and Gradient Map adjustment layers above turn it into indexed pixel art, a dither layer adds patterns.",
                        )
                        .weak()
                        .small(),
                    );
                });
                let (styled, pixel_layer) = self
                    .state
                    .active()
                    .map(|d| {
                        let l = d.doc.state().active_layer();
                        (!l.props.style.is_off(), l.owns_pixels())
                    })
                    .unwrap_or((false, false));
                let is_adjustment = self.state.active().is_some_and(|d| d.doc.state().active_layer().is_adjustment());
                let is_tilemap =
                    self.state.active().is_some_and(|d| d.doc.state().active_layer().props.tilemap.is_some());
                let is_pixels = self.state.active().is_some_and(|d| {
                    d.doc.state().active_layer().props.kind == qsketch_core::layer::LayerKind::Raster
                });
                menus::submenu(ui, false, "Tilemap", |ui| {
                    self.menu_item(ui, Action::NewTilemapLayer, has_doc);
                    self.menu_item(ui, Action::ConvertToTilemap, is_pixels);
                    self.menu_item(ui, Action::ConvertToPixels, is_tilemap);
                    ui.label(
                        egui::RichText::new(format!(
                            "Tile size = grid size ({0}×{0}, View › Grid)",
                            self.state.settings.canvas.grid_size
                        ))
                        .weak()
                        .small(),
                    );
                });
                menus::submenu(ui, false, "New Adjustment Layer", |ui| {
                    for (a, label) in [
                        (Action::NewAdjBrightnessContrast, "Brightness/Contrast…"),
                        (Action::NewAdjLevels, "Levels…"),
                        (Action::NewAdjCurves, "Curves…"),
                        (Action::NewAdjPosterize, "Posterize…"),
                        (Action::NewAdjGradientMap, "Gradient Map…"),
                        (Action::NewAdjBlackWhite, "Black & White"),
                        (Action::NewAdjHueSaturation, "Hue/Saturation…"),
                        (Action::NewAdjColorBalance, "Color Balance…"),
                    ] {
                        let btn = egui::Button::new(label).shortcut_text(self.state.keymap.primary_text(a));
                        if ui.add_enabled(has_doc, btn).clicked() {
                            self.state.pending.push(a);
                            ui.close();
                        }
                    }
                    ui.separator();
                    self.menu_item(ui, Action::AdjustmentSettings, is_adjustment);
                });
                self.menu_item(ui, Action::Outline, pixel_layer);
                menus::submenu(ui, false, "Layer Style", |ui| {
                    self.menu_item(ui, Action::LayerStyle, pixel_layer);
                    self.menu_item(ui, Action::CopyLayerStyle, styled);
                    let can_paste = pixel_layer && self.state.style_clipboard.is_some();
                    self.menu_item(ui, Action::PasteLayerStyle, can_paste);
                    self.menu_item(ui, Action::ClearLayerStyle, styled);
                });
                ui.separator();
                self.menu_item(ui, Action::GroupLayers, has_doc);
                self.menu_item(ui, Action::UngroupLayers, active_is_group);
                ui.separator();
                self.menu_item(ui, Action::MergeDown, can_merge_down);
                self.menu_item(ui, Action::MergeVisible, layers > 1);
                self.menu_item(ui, Action::Flatten, layers > 1);
                ui.separator();
                menus::submenu(ui, false, "Arrange", |ui| {
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
                ui.separator();
                let (has_mask, mask_on) = self
                    .state
                    .active()
                    .map(|d| {
                        let l = d.doc.state().active_layer();
                        (l.mask.is_some(), l.props.mask_enabled)
                    })
                    .unwrap_or((false, true));
                menus::submenu(ui, false, "Layer Mask", |ui| {
                    self.menu_item(ui, Action::MaskRevealAll, has_doc && !has_mask);
                    self.menu_item(ui, Action::MaskHideAll, has_doc && !has_mask);
                    self.menu_item(ui, Action::MaskFromSelection, has_doc && !has_mask && has_sel);
                    ui.separator();
                    let text = self.state.keymap.primary_text(Action::MaskToggle);
                    let label = if mask_on { "Disable Layer Mask" } else { "Enable Layer Mask" };
                    if ui.add_enabled(has_mask, egui::Button::new(label).shortcut_text(text)).clicked() {
                        self.state.pending.push(Action::MaskToggle);
                        ui.close();
                    }
                    self.menu_item(ui, Action::MaskApply, has_mask);
                    self.menu_item(ui, Action::MaskDelete, has_mask);
                });
            });
            top_menu(ui, "Animation", |ui| {
                if let Some(id) = self.state.active_doc {
                    crate::panels::timeline::animation_menu(ui, &mut self.state, id);
                } else {
                    ui.add_enabled(false, egui::Button::new("Open a document first"));
                    ui.separator();
                    self.menu_item(ui, Action::ImportFrames, true);
                    self.menu_item(ui, Action::ImportSpriteSheet, true);
                }
                ui.separator();
                let open = self.workspace.is_panel_open(&PanelKind::Timeline);
                let btn = menus::button(open, "Timeline")
                    .shortcut_text(self.state.keymap.primary_text(Action::ShowTimeline));
                if ui.add(btn).clicked() {
                    self.state.pending.push(Action::ShowTimeline);
                    ui.close();
                }
            });
            top_menu(ui, "Select", |ui| {
                self.menu_item(ui, Action::SelectAll, has_doc);
                self.menu_item(ui, Action::Deselect, has_sel);
                self.menu_item(ui, Action::Reselect, !has_sel);
                self.menu_item(ui, Action::InvertSelection, has_doc);
                self.menu_item(ui, Action::SelectLayerContent, sel_or_content);
                ui.separator();
                menus::submenu(ui, false, "Modify", |ui| {
                    self.menu_item(ui, Action::BorderSelection, has_sel);
                    self.menu_item(ui, Action::SmoothSelection, has_sel);
                    self.menu_item(ui, Action::ExpandSelection, has_sel);
                    self.menu_item(ui, Action::ContractSelection, has_sel);
                    self.menu_item(ui, Action::FeatherSelection, has_sel);
                    ui.separator();
                    self.menu_item(ui, Action::SharpenSelection, has_sel);
                    self.menu_item(ui, Action::RemoveSelectionHoles, has_sel);
                });
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
                let btn = menus::button(grid, "Pixel Grid")
                    .shortcut_text(self.state.keymap.primary_text(Action::TogglePixelGrid));
                if ui.add(btn).clicked() {
                    self.state.pending.push(Action::TogglePixelGrid);
                    ui.close();
                }
                // Tile grid: on/off plus its size, Aseprite style.
                let c = &mut self.state.settings.canvas;
                menus::submenu(ui, c.show_grid, "Grid", |ui| {
                    let btn = menus::button(c.show_grid, "Show Grid")
                        .shortcut_text(self.state.keymap.primary_text(Action::ToggleGrid));
                    if ui.add(btn).clicked() {
                        c.show_grid = !c.show_grid;
                        ui.close();
                    }
                    ui.separator();
                    for n in [4u32, 8, 16, 32, 64] {
                        if ui.radio(c.grid_size == n, format!("{n} × {n}")).clicked() {
                            c.grid_size = n;
                            c.show_grid = true;
                            ui.close();
                        }
                    }
                    ui.horizontal(|ui| {
                        menus::indent(ui);
                        let custom = ![4, 8, 16, 32, 64].contains(&c.grid_size);
                        let _ = ui.radio(custom, "Custom");
                        if ui.add(egui::DragValue::new(&mut c.grid_size).range(1..=4096).suffix(" px")).changed() {
                            c.show_grid = true;
                        }
                    });
                    ui.separator();
                    let btn = menus::button(c.snap_to_grid, Action::ToggleSnapToGrid.label())
                    .shortcut_text(self.state.keymap.primary_text(Action::ToggleSnapToGrid));
                    if ui
                        .add(btn)
                        .on_hover_text("Shapes, marquees, crop, move and gradient drags land on grid lines")
                        .clicked()
                    {
                        c.snap_to_grid = !c.snap_to_grid;
                        ui.close();
                    }
                });
                // Rulers and guides.
                let has_guides = self.state.active().is_some_and(|d| !d.doc.guides.is_empty());
                let c = &self.state.settings.canvas;
                let rg_on = c.show_rulers || (c.show_guides && has_guides);
                menus::submenu(ui, rg_on, "Rulers & Guides", |ui| {
                    ui.label(
                        egui::RichText::new(
                            "Drag from a ruler to add a guide; drag one back onto the ruler to remove it. The Move tool (or Ctrl with any tool) moves guides.",
                        )
                        .weak()
                        .small(),
                    );
                    let c = self.state.settings.canvas.clone();
                    for (a, on, enabled) in [
                        (Action::ToggleRulers, c.show_rulers, true),
                        (Action::ToggleGuides, c.show_guides, true),
                        (Action::ToggleSnapToGuides, c.snap_to_guides, true),
                        (Action::ToggleLockGuides, c.lock_guides, true),
                        (Action::ToggleShowSlices, c.show_slices, true),
                    ] {
                        let btn = menus::button(on, a.label())
                            .shortcut_text(self.state.keymap.primary_text(a));
                        if ui.add_enabled(enabled, btn).clicked() {
                            self.state.pending.push(a);
                            ui.close();
                        }
                    }
                    ui.separator();
                    self.menu_item(ui, Action::ClearGuides, has_guides);
                });
                let tiled = self.state.settings.canvas.tiled;
                menus::submenu(ui, tiled > 0, "Tiled Mode", |ui| {
                    ui.label(
                        egui::RichText::new("Repeat the document around itself to check seamless tiles").weak().small(),
                    );
                    for (a, v) in
                        [(Action::TiledOff, 0u8), (Action::TiledX, 1), (Action::TiledY, 2), (Action::TiledBoth, 3)]
                    {
                        let btn =
                            menus::button(tiled == v, a.label())
                                .shortcut_text(self.state.keymap.primary_text(a));
                        if ui.add(btn).clicked() {
                            self.state.pending.push(a);
                            ui.close();
                        }
                    }
                });
                ui.separator();
                menus::submenu(ui, false, "Symmetry", |ui| {
                    let sym = self.state.symmetry;
                    for (a, on) in [
                        (Action::ToggleSymmetryHorizontal, sym.horizontal),
                        (Action::ToggleSymmetryVertical, sym.vertical),
                    ] {
                        let btn = menus::button(on, a.label())
                            .shortcut_text(self.state.keymap.primary_text(a));
                        if ui.add(btn).clicked() {
                            self.state.pending.push(a);
                            ui.close();
                        }
                    }
                    ui.horizontal(|ui| {
                        menus::indent(ui);
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
                        .add(menus::button(g, "Show Guides"))
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
                    (Action::ShowPalette, PanelKind::Palette),
                    (Action::ShowNavigator, PanelKind::Navigator),
                    (Action::ShowBrushes, PanelKind::Brushes),
                    (Action::ShowBrushSettings, PanelKind::BrushSettings),
                    (Action::ShowInfo, PanelKind::Info),
                    (Action::ShowReference, PanelKind::Reference),
                    (Action::ShowTileset, PanelKind::Tileset),
                    (Action::ShowTimeline, PanelKind::Timeline),
                ] {
                    let open = self.workspace.is_panel_open(&k);
                    let btn = menus::button(open, a.label())
                        .shortcut_text(self.state.keymap.primary_text(a));
                    if ui.add(btn).clicked() {
                        self.state.pending.push(a);
                        ui.close();
                    }
                }
                ui.separator();
                self.menu_item(ui, Action::ResetLayout, true);
                ui.separator();
                let several = self.state.docs.len() > 1;
                menus::submenu(ui, false, "Arrange", |ui| {
                    self.menu_item(ui, Action::ArrangeSideBySide, several);
                    self.menu_item(ui, Action::ArrangeStacked, several);
                    self.menu_item(ui, Action::ArrangeGrid, several);
                    self.menu_item(ui, Action::ArrangeTabs, several);
                });
                self.menu_item(ui, Action::NextDocument, several);
                self.menu_item(ui, Action::PrevDocument, several);
                ui.separator();
                let docs: Vec<(DocId, String)> =
                    self.state.docs.iter().map(|d| (d.id, d.doc.display_title())).collect();
                for (id, title) in docs {
                    let active = self.state.active_doc == Some(id);
                    if ui
                        .add(menus::button(active, title))
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
        self.menu_item(ui, Action::Liquify, has_doc);
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
            menus::submenu(ui, false, name, |ui| {
                for &a in actions {
                    self.menu_item(ui, a, has_doc);
                }
            });
        }
        ui.separator();
        menus::submenu(ui, false, "Experimental", |ui| {
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
        let bar = ui.max_rect();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if let Some(d) = self.state.active() {
                // View group (zoom, size, cursor) then document group (layer, selection),
                // each tagged with its section color.
                ui.label(RichText::new(icons::COMPASS).weak().small());
                ui.label(RichText::new(format!("{:.0}%", d.view.zoom * 100.0)).monospace().small());
                ui.label(RichText::new(format!("{} × {}", d.doc.width(), d.doc.height())).weak().small());
                // File size as of the last save; "~" once the document has
                // changed since, as the next save will differ.
                if let Some(bytes) = d.file_size {
                    let approx = d.doc.is_modified();
                    let text = format!("{}{}", if approx { "~" } else { "" }, crate::ui::widgets::fmt_bytes(bytes));
                    ui.label(RichText::new(text).weak().small()).on_hover_text(if approx {
                        "File size at the last save (the document has unsaved changes)"
                    } else {
                        "File size on disk"
                    });
                }
                if d.doc.timelapse.recording {
                    let n = d.doc.timelapse.frames.len();
                    ui.label(
                        RichText::new(format!("{} REC {n}", icons::RECORD))
                            .small()
                            .color(egui::Color32::from_rgb(235, 70, 70)),
                    )
                    .on_hover_text(format!("Recording a timelapse ({n} frames). File › Timelapse to export or stop."));
                }
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
                // Shared canvas: the conversation's dot, and who else is drawing.
                if let Some(sh) = d.share.as_ref() {
                    if let Some(c) = crate::share::status_color(&self.state, sh) {
                        ui.label(crate::share::dot(c, 8.0));
                    }
                    let others = sh.present();
                    let who = match others.len() {
                        0 => format!("{} · only you", sh.conv.name),
                        _ => format!(
                            "{} · {}",
                            sh.conv.name,
                            others.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
                        ),
                    };
                    let connected = self.state.share.as_ref().is_some_and(|l| l.connected);
                    let text = if connected { who } else { format!("{} · Leyline not running", sh.conv.name) };
                    ui.label(RichText::new(text).weak().small());
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("v{}", crate::update::CURRENT_VERSION)).weak().small());
                for job in &self.state.jobs {
                    let done = job.done.load(std::sync::atomic::Ordering::Relaxed);
                    ui.add(
                        egui::ProgressBar::new(done as f32 / job.total.max(1) as f32)
                            .desired_width(90.0)
                            .desired_height(10.0),
                    );
                    ui.label(RichText::new(format!("{} {done}/{}", job.label, job.total)).weak().small());
                }
                let tool = self.state.effective_tool();
                let hint = match tool {
                    ToolKind::Brush | ToolKind::Pencil | ToolKind::Eraser => {
                        "Shift+click: straight line (Shift+Ctrl: snap angle) · [ ]: size · Alt or right-click: pick · middle-drag: pan"
                    }
                    ToolKind::RectSelect | ToolKind::EllipseSelect | ToolKind::Lasso => {
                        "Shift: add · Alt: subtract · click: deselect"
                    }
                    ToolKind::PolyLasso => {
                        "click: add point · first point / double-click / Enter: close · Backspace: undo point · Esc: cancel · Shift: add · Alt: subtract"
                    }
                    _ if self.state.floating.is_some() => {
                        match self.state.floating.as_ref().map(|f| f.mode) {
                            Some(crate::tools::floating::Mode::Deform) => {
                                "drag corners/edges: deform · inside: move · Enter: OK · Esc: cancel"
                            }
                            Some(crate::tools::floating::Mode::Warp) => {
                                "drag points: bend · inside: move · Enter: OK · Esc: cancel"
                            }
                            Some(crate::tools::floating::Mode::Rotate) => "drag: rotate (Shift: 15°) · Enter: OK · Esc: cancel",
                            Some(crate::tools::floating::Mode::Resize) => {
                                "handles: scale (Shift: keep aspect, Alt: from center) · inside: move · Enter: OK · Esc: cancel"
                            }
                            _ => "drag: move · outside: rotate (Shift: 15°) · handles: scale (Shift: keep aspect, Alt: from center, Ctrl: deform) · Enter: OK · Esc: cancel",
                        }
                    }
                    ToolKind::Move => "Shift: constrain · arrows: nudge",
                    ToolKind::Clone => "Alt+click: set source · [ ]: size · Shift+click: straight line (Shift+Ctrl: snap angle)",
                    ToolKind::Smudge => "Opacity = strength · [ ]: size",
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
        self.status_message(ui, bar);
    }

    /// A system message (an update check that failed, say) centered in the
    /// status bar for a few seconds, over whatever the bar shows.
    fn status_message(&mut self, ui: &mut Ui, bar: egui::Rect) {
        const SHOW_FOR: std::time::Duration = std::time::Duration::from_secs(8);
        let Some((msg, since)) = &self.state.status_msg else { return };
        let age = since.elapsed();
        if age >= SHOW_FOR {
            self.state.status_msg = None;
            return;
        }
        let painter = ui.painter();
        let font = egui::FontId::proportional(12.0);
        let galley = painter.layout_no_wrap(msg.clone(), font, ui.visuals().strong_text_color());
        let pad = egui::vec2(10.0, 3.0);
        let size = galley.size() + pad * 2.0;
        let rect = egui::Rect::from_center_size(bar.center(), size.min(bar.size()));
        crate::ui::chrome::fill_box(painter, rect, 4.0, crate::ui::chrome::row_fill(ui.visuals().selection.bg_fill));
        painter.galley(rect.min + pad, galley, ui.visuals().strong_text_color());
        ui.ctx().request_repaint_after(SHOW_FOR - age);
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
            Action::ToggleTimelapse => {
                if let Some(id) = active {
                    crate::timelapse::toggle(&mut self.state, id);
                }
            }
            Action::ExportTimelapseGif | Action::ExportTimelapseFrames => {
                if let Some(id) = active {
                    crate::timelapse::export(&mut self.state, id, action == Action::ExportTimelapseGif);
                }
            }
            Action::ClearTimelapse => {
                if let Some(d) = self.state.active_mut() {
                    d.doc.timelapse.clear();
                }
            }
            Action::QuickExport => {
                if let Some(id) = active {
                    crate::files::quick_export(&mut self.state, id);
                }
            }
            Action::NextDocument | Action::PrevDocument => {
                let step = if action == Action::NextDocument { 1 } else { -1 };
                if let Some(id) = self.state.document_from_active(step) {
                    self.state.active_doc = Some(id);
                    self.workspace.focus_document(id);
                }
            }
            Action::ArrangeSideBySide | Action::ArrangeStacked | Action::ArrangeGrid | Action::ArrangeTabs => {
                let mode = match action {
                    Action::ArrangeSideBySide => crate::workspace::Arrange::SideBySide,
                    Action::ArrangeStacked => crate::workspace::Arrange::Stacked,
                    Action::ArrangeGrid => crate::workspace::Arrange::Grid,
                    _ => crate::workspace::Arrange::Tabs,
                };
                let docs: Vec<DocId> = self.state.docs.iter().map(|d| d.id).collect();
                self.workspace.arrange_documents(&docs, active, mode);
            }
            Action::ExportScaled2 | Action::ExportScaled3 | Action::ExportScaled4 | Action::ExportScaled8 => {
                let k = match action {
                    Action::ExportScaled2 => 2,
                    Action::ExportScaled3 => 3,
                    Action::ExportScaled4 => 4,
                    _ => 8,
                };
                if let Some(id) = active {
                    crate::files::export_scaled(&mut self.state, id, k);
                }
            }
            Action::ExportTileset => {
                if let Some(id) = active {
                    crate::files::export_tileset(&mut self.state, id);
                }
            }
            Action::ExportSlices => {
                if let Some(id) = active {
                    crate::files::export_slices(&mut self.state, id);
                }
            }
            Action::ToggleShowSlices => {
                let c = &mut self.state.settings.canvas;
                c.show_slices = !c.show_slices;
            }
            Action::ToggleSnapToGrid => {
                self.state.settings.canvas.snap_to_grid = !self.state.settings.canvas.snap_to_grid
            }
            Action::TiledOff => self.state.settings.canvas.tiled = 0,
            Action::TiledX => self.state.settings.canvas.tiled = 1,
            Action::TiledY => self.state.settings.canvas.tiled = 2,
            Action::TiledBoth => self.state.settings.canvas.tiled = 3,
            Action::ShareCanvas => dialogs::share::open(&mut self.state, ctx),
            Action::StopSharing => {
                if let Some(id) = active {
                    crate::share::stop(&mut self.state, id);
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
                self.state.settle();
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
            Action::FreeTransform => {
                if let Some(id) = active {
                    crate::tools::floating::begin_transform(&mut self.state, id);
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
            // With the Slice tool, Delete removes the selected slice.
            Action::Clear
                if self.state.effective_tool() == ToolKind::Slice
                    && self.state.active().is_some_and(|d| d.slice_sel.is_some()) =>
            {
                crate::tools::slice::delete_selected(&mut self.state);
            }
            // With the Shape tool, Delete removes the selected point.
            Action::Clear
                if self.state.effective_tool() == ToolKind::Shape
                    && self.state.active().is_some_and(|d| d.shape_sel.is_some()) =>
            {
                crate::tools::vector::delete_selected(&mut self.state);
            }
            Action::Clear => self.clear_selected(),
            Action::ClearLayer => {
                if let Some(d) = self.state.active_mut() {
                    let targets = d.target_layers();
                    if !targets.is_empty() {
                        let saved = d.doc.state_mut().selection.take();
                        for li in targets {
                            let r = ops::clear(d.doc.state_mut(), li);
                            d.doc.mark_dirty_rect(r);
                        }
                        d.doc.state_mut().selection = saved;
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
            Action::BrightnessContrast
            | Action::Levels
            | Action::Curves
            | Action::ColorBalance
            | Action::Posterize
            | Action::GradientMap
            | Action::BlackWhite
            | Action::HueSaturation => dialogs::filter::open(&mut self.state, action),
            Action::Liquify => dialogs::liquify::open(&mut self.state),
            Action::ReplaceColor => dialogs::filter::open_replace_color(&mut self.state),
            Action::SnapToPalette => dialogs::filter::open_snap_to_palette(&mut self.state, false),
            Action::IndexColors => dialogs::filter::open_snap_to_palette(&mut self.state, true),
            Action::PixelAspectSquare | Action::PixelAspectWide | Action::PixelAspectTall => {
                let v = match action {
                    Action::PixelAspectWide => [2u8, 1u8],
                    Action::PixelAspectTall => [1, 2],
                    _ => [1, 1],
                };
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    if d.doc.state().pixel_aspect != v {
                        d.doc.state_mut().pixel_aspect = v;
                        d.doc.commit("Pixel Aspect Ratio");
                        let (w, h) = (d.doc.width(), d.doc.height());
                        d.view.aspect = d.doc.state().aspect();
                        d.view.fit(w, h);
                    }
                }
            }
            Action::TogglePaletteLock => {
                if let Some(e) = self.state.active_mut() {
                    let on = !e.doc.state().palette_lock;
                    e.doc.set_palette_lock(on);
                    if on && e.doc.state().palette.is_empty() {
                        self.state.show_panel_requests.push(PanelKind::Palette);
                        self.state.toasts.push(Level::Info, "The palette is empty: add colors in the Palette panel.");
                    }
                }
            }
            Action::LastFilter => dialogs::filter::repeat_last(&mut self.state),
            Action::LastFilterDialog => dialogs::filter::reopen_last(&mut self.state),
            a if a.category() == Category::Filter => dialogs::filter::open(&mut self.state, a),
            Action::NewLayer => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let name = s.unique_layer_name("Layer");
                    s.add_layer(name, None);
                    d.doc.commit("New Layer");
                }
            }
            Action::DuplicateLayer => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let a = s.active;
                    s.duplicate_layer(a);
                    d.doc.mark_all_dirty();
                    d.doc.commit("Duplicate Layer");
                }
            }
            Action::DeleteLayer => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    // Top to bottom so earlier removals don't shift later indices.
                    let mut targets = d.selected_indices();
                    targets.reverse();
                    let n = targets.len();
                    let s = d.doc.state_mut();
                    let mut removed = 0;
                    for i in targets {
                        if i < s.layers.len() && s.remove_layer(i).is_some() {
                            removed += 1;
                        }
                    }
                    if removed > 0 {
                        d.selected.clear();
                        d.doc.mark_all_dirty();
                        d.doc.commit(if n > 1 { "Delete Layers" } else { "Delete Layer" });
                    } else {
                        self.state.toasts.push(Level::Info, "A document needs at least one layer.");
                    }
                }
            }
            Action::GroupLayers => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let ids = d.selected_ids();
                    if d.doc.state_mut().group_layers(&ids).is_some() {
                        d.selected.clear();
                        d.doc.mark_all_dirty();
                        d.doc.commit("Group Layers");
                    }
                }
            }
            Action::UngroupLayers => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let a = d.doc.state().active;
                    if d.doc.state_mut().ungroup(a) {
                        d.selected.clear();
                        d.doc.mark_all_dirty();
                        d.doc.commit("Ungroup");
                    } else {
                        self.state.toasts.push(Level::Info, "The active layer is not a group.");
                    }
                }
            }
            Action::MergeDown => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let a = s.active;
                    let label = if s.layers[a].is_group() { "Merge Group" } else { "Merge Down" };
                    if s.merge_down(a) {
                        d.doc.mark_all_dirty();
                        d.doc.commit(label);
                    }
                }
            }
            Action::MergeVisible => self.edit_doc("Merge Visible", |s| s.merge_visible(), false),
            Action::Flatten => self.edit_doc("Flatten Image", |s| s.flatten(), false),
            Action::LayerUp | Action::LayerDown | Action::LayerToTop | Action::LayerToBottom => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let a = s.active;
                    let moved = match action {
                        Action::LayerUp => s.move_sibling(a, true),
                        Action::LayerDown => s.move_sibling(a, false),
                        Action::LayerToTop => s.move_to_end(a, true),
                        _ => s.move_to_end(a, false),
                    };
                    if moved {
                        d.doc.mark_all_dirty();
                        d.doc.commit("Reorder Layers");
                    }
                }
            }
            Action::SelectLayerAbove | Action::SelectLayerBelow => {
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let n = s.layers.len();
                    // Wraps: above the top layer is the bottom one and vice versa.
                    s.active =
                        if action == Action::SelectLayerAbove { (s.active + 1) % n } else { (s.active + n - 1) % n };
                }
            }
            Action::ToggleLayerVisibility => {
                if let Some(d) = self.state.active_mut() {
                    let l = &d.doc.state().layers[d.doc.state().active].props;
                    let (id, vis) = (l.id, l.visible);
                    d.doc.set_layer_visible(id, !vis);
                }
            }
            Action::ToggleAlphaLock => {
                self.toggle_prop("Lock Transparent Pixels", |p| p.alpha_locked = !p.alpha_locked)
            }
            Action::ToggleLayerLock => self.toggle_prop("Lock Layer", |p| p.locked = !p.locked),
            Action::MaskRevealAll | Action::MaskHideAll | Action::MaskFromSelection => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let (w, h) = (s.width, s.height);
                    let li = s.active;
                    if s.layers[li].is_group() {
                        self.state.toasts.push(Level::Info, "Groups can't have a mask yet.");
                    } else if s.layers[li].mask.is_none() {
                        let mask = match action {
                            Action::MaskHideAll => qsketch_core::Mask::new(w, h),
                            Action::MaskFromSelection => match &s.selection {
                                Some(sel) => (**sel).clone(),
                                None => qsketch_core::Mask::full(w, h),
                            },
                            _ => qsketch_core::Mask::full(w, h),
                        };
                        let id = s.layers[li].props.id;
                        s.layers[li].mask = Some(std::sync::Arc::new(mask));
                        s.layers[li].props.mask_enabled = true;
                        if action == Action::MaskFromSelection {
                            s.selection = None;
                            d.sel_outline = None;
                        }
                        d.doc.mark_all_dirty();
                        d.doc.commit("Add Layer Mask");
                        d.mask_edit = Some(id);
                    }
                }
            }
            Action::MaskDelete | Action::MaskApply => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let li = s.active;
                    if s.layers[li].mask.is_some() {
                        if action == Action::MaskApply {
                            s.layers[li].apply_mask();
                        } else {
                            s.layers[li].mask = None;
                            s.layers[li].props.mask_enabled = true;
                        }
                        d.mask_edit = None;
                        d.doc.mark_all_dirty();
                        d.doc.commit(if action == Action::MaskApply {
                            "Apply Layer Mask"
                        } else {
                            "Delete Layer Mask"
                        });
                    }
                }
            }
            Action::MaskToggle => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let li = s.active;
                    if s.layers[li].mask.is_some() {
                        s.layers[li].props.mask_enabled = !s.layers[li].props.mask_enabled;
                        let on = s.layers[li].props.mask_enabled;
                        d.doc.mark_all_dirty();
                        d.doc.commit(if on { "Enable Layer Mask" } else { "Disable Layer Mask" });
                    }
                }
            }
            Action::LayerProperties => dialogs::open_layer_props(&mut self.state),
            Action::LayerStyle => dialogs::layer_style::open(&mut self.state),
            Action::AdjustmentSettings => dialogs::adjustment::open(&mut self.state),
            Action::NewAdjBrightnessContrast
            | Action::NewAdjLevels
            | Action::NewAdjCurves
            | Action::NewAdjHueSaturation
            | Action::NewAdjPosterize
            | Action::NewAdjGradientMap
            | Action::NewAdjBlackWhite
            | Action::NewAdjColorBalance => {
                let base = match action {
                    Action::NewAdjBrightnessContrast => Action::BrightnessContrast,
                    Action::NewAdjLevels => Action::Levels,
                    Action::NewAdjCurves => Action::Curves,
                    Action::NewAdjHueSaturation => Action::HueSaturation,
                    Action::NewAdjPosterize => Action::Posterize,
                    Action::NewAdjGradientMap => Action::GradientMap,
                    Action::NewAdjBlackWhite => Action::BlackWhite,
                    _ => Action::ColorBalance,
                };
                let (fg, bg) = (self.state.fg, self.state.bg);
                let Some(mut filter) = dialogs::filter::default_filter(base, fg, bg) else { return };
                // A gradient map starts from the document's palette when it has one.
                if let Filter::GradientMap { colors } = &mut filter {
                    if let Some(pal) = self.state.active().map(|d| d.doc.state().palette.colors.clone()) {
                        if pal.len() >= 2 {
                            *colors = pal;
                        }
                    }
                }
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let (w, h) = (s.width, s.height);
                    let name = s.unique_layer_name(filter.name());
                    let id = s.add_layer(name, None);
                    if let Some(i) = s.index_of(id) {
                        let l = &mut s.layers[i];
                        l.props.kind = qsketch_core::layer::LayerKind::Adjustment;
                        l.props.adjustment = Some(filter);
                        // Reveal-all mask: paint black to keep the
                        // adjustment off an area.
                        l.mask = Some(std::sync::Arc::new(qsketch_core::Mask::full(w, h)));
                    }
                    d.mask_edit = Some(id);
                    d.doc.mark_all_dirty();
                    d.doc.commit("New Adjustment Layer");
                }
                dialogs::adjustment::open(&mut self.state);
            }
            Action::Outline => dialogs::layer_style::open_outline(&mut self.state),
            Action::NewShapeLayer => {
                self.state.settle();
                self.state.set_tool(ToolKind::Shape);
                let fill = self.state.tool_opts.shape_fill.then_some(self.state.fg);
                let stroke = self.state.tool_opts.shape_stroke.then_some(self.state.bg);
                let (width, closed) =
                    (self.state.tool_opts.shape_stroke_width.max(1), self.state.tool_opts.shape_closed);
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let name = s.unique_layer_name("Shape");
                    let id = s.add_layer(name, None);
                    if let Some(i) = s.index_of(id) {
                        let l = &mut s.layers[i];
                        l.props.kind = qsketch_core::layer::LayerKind::Shape;
                        let mut shape = qsketch_core::ShapePath::new(fill, stroke);
                        shape.stroke_width = width;
                        shape.closed = closed;
                        l.props.shape = Some(shape);
                    }
                    d.selected.clear();
                    d.shape_sel = None;
                    d.doc.commit("New Shape Layer");
                }
                self.state.toasts.push(Level::Info, "Click the canvas with the Shape tool to add points.");
            }
            Action::NewTextLayer => {
                self.state.settle();
                self.state.set_tool(ToolKind::Text);
                if let Some(id) = self.state.active_doc {
                    let (w, h) = self.state.doc(id).map(|d| (d.doc.width(), d.doc.height())).unwrap_or((0, 0));
                    let anchor = qsketch_core::Pt::new((w / 4) as f32, (h / 3) as f32);
                    crate::tools::text::begin(&mut self.state, id, anchor, false);
                }
            }
            Action::EditText => {
                self.state.settle();
                self.state.set_tool(ToolKind::Text);
                if let Some(id) = self.state.active_doc {
                    if !crate::tools::text::begin(&mut self.state, id, qsketch_core::Pt::new(0.0, 0.0), true) {
                        self.state.toasts.push(Level::Info, "The active layer isn't a text layer.");
                    }
                }
            }
            Action::RasterizeText => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let li = s.active;
                    if qsketch_core::text::rasterize(s, li) {
                        d.doc.commit("Rasterize Text");
                    } else {
                        self.state.toasts.push(Level::Info, "The active layer isn't a text layer.");
                    }
                }
            }
            Action::RasterizeShape => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let li = s.active;
                    if qsketch_core::vector::rasterize(s, li) {
                        d.shape_sel = None;
                        d.doc.commit("Rasterize Shape");
                    } else {
                        self.state.toasts.push(Level::Info, "The active layer isn't a shape layer.");
                    }
                }
            }
            Action::NewDitherLayer => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    dither_layer(&mut d.doc);
                    d.doc.commit("New Dither Pattern Layer");
                }
            }
            Action::IndexPaintingSetup => {
                self.state.settle();
                let (fg, bg) = (self.state.fg, self.state.bg);
                if let Some(d) = self.state.active_mut() {
                    index_painting_setup(&mut d.doc, fg, bg);
                    d.doc.commit("HD Index Painting Setup");
                }
                self.state.tool_opts.eyedropper_sample_merged = false;
                self.state.tool_opts.eyedropper_sample_below = true;
                self.state.toasts.push(
                    Level::Info,
                    "Paint in grayscale on the layer below the new ones. The eyedropper now samples the current layer and below.",
                );
            }
            Action::CopyLayerStyle => {
                if let Some(d) = self.state.active() {
                    let s = d.doc.state().active_layer().props.style.clone();
                    self.state.style_clipboard = (!s.is_off()).then_some(s);
                }
            }
            Action::PasteLayerStyle | Action::ClearLayerStyle => {
                let style = if action == Action::PasteLayerStyle {
                    self.state.style_clipboard.clone()
                } else {
                    Some(qsketch_core::LayerStyle::default())
                };
                if let (Some(style), Some(d)) = (style, self.state.active_mut()) {
                    // Every selected pixel layer takes it.
                    let ids = d.selected_ids();
                    let s = d.doc.state_mut();
                    let mut changed = false;
                    for id in ids {
                        if let Some(i) = s.index_of(id) {
                            if s.layers[i].owns_pixels() && s.layers[i].props.style != style {
                                s.layers[i].props.style = style.clone();
                                changed = true;
                            }
                        }
                    }
                    if changed {
                        d.doc.mark_all_dirty();
                        d.doc.commit(if action == Action::PasteLayerStyle {
                            "Paste Layer Style"
                        } else {
                            "Clear Layer Style"
                        });
                    }
                }
            }
            Action::FlipLayerHorizontal | Action::FlipLayerVertical => {
                if let Some(d) = self.state.active_mut() {
                    let targets = d.target_layers();
                    if targets.is_empty() {
                        self.state.toasts.push(Level::Info, "The active layer is locked or hidden.");
                    } else {
                        for li in targets {
                            if action == Action::FlipLayerHorizontal {
                                ops::flip_layer_horizontal(d.doc.state_mut(), li);
                            } else {
                                ops::flip_layer_vertical(d.doc.state_mut(), li);
                            }
                        }
                        d.doc.mark_all_dirty();
                        d.doc.commit(if action == Action::FlipLayerHorizontal {
                            "Flip Layer Horizontal"
                        } else {
                            "Flip Layer Vertical"
                        });
                    }
                }
            }
            Action::SelectAll => {
                self.state.settle();
                if let Some(id) = active {
                    crate::tools::select_all(&mut self.state, id);
                }
            }
            Action::Deselect => {
                self.state.settle();
                if let Some(id) = active {
                    crate::tools::deselect(&mut self.state, id);
                }
            }
            Action::InvertSelection => {
                self.state.settle();
                if let Some(id) = active {
                    crate::tools::invert_selection(&mut self.state, id);
                }
            }
            Action::FeatherSelection => dialogs::open_modify(&mut self.state, ModifyKind::Feather),
            Action::ExpandSelection => dialogs::open_modify(&mut self.state, ModifyKind::Expand),
            Action::ContractSelection => dialogs::open_modify(&mut self.state, ModifyKind::Contract),
            Action::BorderSelection => dialogs::open_modify(&mut self.state, ModifyKind::Border),
            Action::SmoothSelection => dialogs::open_modify(&mut self.state, ModifyKind::Smooth),
            Action::SharpenSelection => dialogs::open_modify(&mut self.state, ModifyKind::Sharpen),
            Action::RemoveSelectionHoles => dialogs::open_modify(&mut self.state, ModifyKind::RemoveHoles),
            Action::Reselect => {
                self.state.settle();
                if let Some(id) = active {
                    crate::tools::reselect(&mut self.state, id);
                }
            }
            Action::SelectLayerContent => {
                self.state.settle();
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
            Action::ToggleGrid => self.state.settings.canvas.show_grid = !self.state.settings.canvas.show_grid,
            Action::TogglePixelGrid => {
                self.state.settings.canvas.show_pixel_grid = !self.state.settings.canvas.show_pixel_grid
            }
            Action::ToggleFullscreen => {
                // The flag is the source of truth: the viewport's own
                // `fullscreen` is unreliable here (it reports true for a
                // plainly windowed window at startup), and trusting it made
                // the first F11 of a session a no-op.
                self.state.fullscreen = !self.state.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.state.fullscreen));
                resize_settled(ctx);
            }
            Action::TogglePanels => self.state.panels_hidden = !self.state.panels_hidden,
            Action::ToggleRulers => {
                let c = &mut self.state.settings.canvas;
                c.show_rulers = !c.show_rulers;
            }
            Action::ToggleGuides => {
                let c = &mut self.state.settings.canvas;
                c.show_guides = !c.show_guides;
            }
            Action::ToggleSnapToGuides => {
                let c = &mut self.state.settings.canvas;
                c.snap_to_guides = !c.snap_to_guides;
            }
            Action::ToggleLockGuides => {
                let c = &mut self.state.settings.canvas;
                c.lock_guides = !c.lock_guides;
            }
            Action::ClearGuides => {
                if let Some(d) = self.state.active_mut() {
                    d.doc.guides.clear();
                }
                self.state.guide_drag = None;
            }
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
            Action::SymmetryResetCenter => {
                self.state.symmetry.center = None;
                self.state.symmetry.angle = 0.0;
            }
            Action::BrushSizeUp | Action::BrushSizeDown => {
                // The tool the user picked, not a held-modifier stand-in: with
                // Alt down the eyedropper is in hand, but Alt+wheel means the
                // brush underneath.
                let tool =
                    if self.state.current_brush().is_some() { self.state.effective_tool() } else { self.state.tool };
                if let Some(b) = self.state.brush_for_tool_mut(tool) {
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
            Action::ShowPalette => self.state.show_panel_requests.push(PanelKind::Palette),
            Action::ShowNavigator => self.state.show_panel_requests.push(PanelKind::Navigator),
            Action::ShowBrushes => self.state.show_panel_requests.push(PanelKind::Brushes),
            Action::ShowBrushSettings => self.state.show_panel_requests.push(PanelKind::BrushSettings),
            Action::ShowInfo => self.state.show_panel_requests.push(PanelKind::Info),
            Action::ShowReference => self.state.show_panel_requests.push(PanelKind::Reference),
            Action::ShowTileset => self.state.show_panel_requests.push(PanelKind::Tileset),
            Action::ShowTimeline => {
                if self.workspace.is_panel_open(&PanelKind::Timeline) {
                    self.workspace.close_panel(&PanelKind::Timeline);
                } else {
                    self.state.show_panel_requests.push(PanelKind::Timeline);
                }
            }
            Action::ExportAnimation => {
                if let Some(id) = active {
                    dialogs::anim::open_export(&mut self.state, id);
                }
            }
            Action::ImportFrames => crate::anim::import_frames(&mut self.state),
            Action::ImportSpriteSheet => crate::anim::import_sheet(&mut self.state),
            Action::ToggleOnionSkin => {
                let o = &mut self.state.settings.anim.onion;
                o.enabled = !o.enabled;
            }
            Action::ToggleLoopTag => self.state.settings.anim.loop_tag = !self.state.settings.anim.loop_tag,
            Action::NewFrame
            | Action::NewEmptyFrame
            | Action::DuplicateFrames
            | Action::DeleteFrames
            | Action::FrameProperties
            | Action::ReverseFrames
            | Action::PlayAnimation
            | Action::FirstFrame
            | Action::PrevFrame
            | Action::NextFrame
            | Action::LastFrame
            | Action::NewTag
            | Action::TagProperties
            | Action::DeleteTag
            | Action::ClearCel
            | Action::LinkCels
            | Action::UnlinkCel
            | Action::CopyCel
            | Action::PasteCel
            | Action::CelProperties
            | Action::ToggleContinuous => {
                let Some(id) = active else { return };
                let st = &mut self.state;
                match action {
                    Action::NewFrame => crate::anim::new_frame(st, id, false),
                    Action::NewEmptyFrame => crate::anim::new_frame(st, id, true),
                    Action::DuplicateFrames => crate::anim::duplicate_frames(st, id),
                    Action::DeleteFrames => crate::anim::delete_frames(st, id),
                    Action::FrameProperties => crate::anim::frame_properties(st, id),
                    Action::ReverseFrames => crate::anim::reverse_frames(st, id),
                    Action::PlayAnimation => crate::anim::toggle_play(st, id),
                    Action::FirstFrame => crate::anim::first_frame(st, id),
                    Action::PrevFrame => crate::anim::step(st, id, -1),
                    Action::NextFrame => crate::anim::step(st, id, 1),
                    Action::LastFrame => crate::anim::last_frame(st, id),
                    Action::NewTag => crate::anim::new_tag(st, id),
                    Action::TagProperties => crate::anim::tag_properties(st, id, None),
                    Action::DeleteTag => crate::anim::delete_tag(st, id, None),
                    Action::ClearCel => crate::anim::clear_cel(st, id),
                    Action::LinkCels => crate::anim::link_cels(st, id),
                    Action::UnlinkCel => crate::anim::unlink_cel(st, id),
                    Action::CopyCel => crate::anim::copy_cel(st, id),
                    Action::PasteCel => crate::anim::paste_cel(st, id),
                    Action::CelProperties => crate::anim::cel_properties(st, id),
                    Action::ToggleContinuous => crate::anim::toggle_continuous(st, id),
                    _ => {}
                }
            }
            Action::NewTilemapLayer | Action::ConvertToTilemap => {
                let size = self.state.settings.canvas.grid_size.clamp(2, 256);
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let li = if action == Action::NewTilemapLayer {
                        let name = s.unique_layer_name("Tilemap");
                        let id = s.add_layer(name, None);
                        s.index_of(id)
                    } else {
                        Some(s.active).filter(|&i| s.layers[i].props.kind == qsketch_core::layer::LayerKind::Raster)
                    };
                    if let Some(li) = li {
                        let tname = format!("Tileset {}", s.tilesets.len() + 1);
                        let (mut ts, mut tm) =
                            qsketch_core::tilemap::from_raster(&s.layers[li].raster, &tname, size, size);
                        tm.tileset = s.tilesets.len();
                        ts.name = tname;
                        s.tilesets.push(ts);
                        let l = &mut s.layers[li];
                        l.props.kind = qsketch_core::layer::LayerKind::Tilemap;
                        l.props.tilemap = Some(tm);
                        d.doc.commit(if action == Action::NewTilemapLayer {
                            "New Tilemap Layer"
                        } else {
                            "Convert to Tilemap"
                        });
                        self.state.show_panel_requests.push(PanelKind::Tileset);
                    } else {
                        self.state.toasts.push(Level::Info, "Only a pixel layer can become a tilemap.");
                    }
                }
            }
            Action::ConvertToPixels => {
                self.state.settle();
                if let Some(d) = self.state.active_mut() {
                    let s = d.doc.state_mut();
                    let li = s.active;
                    if s.layers[li].props.tilemap.take().is_some() {
                        s.layers[li].props.kind = qsketch_core::layer::LayerKind::Raster;
                        d.doc.commit("Convert to Pixel Layer");
                    }
                }
            }
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

    /// Apply an operation to every selected layer (respecting the selection;
    /// groups stand for their members) and commit once.
    fn edit_layer(&mut self, label: &str, f: impl Fn(&mut qsketch_core::DocState, usize) -> qsketch_core::IRect) {
        self.state.settle();
        let Some(d) = self.state.active_mut() else { return };
        let targets = d.target_layers();
        if targets.is_empty() {
            self.state.toasts.push(Level::Info, "The active layer is locked or hidden.");
            return;
        }
        let mut any = false;
        for li in targets {
            let r = f(d.doc.state_mut(), li);
            if !r.is_empty() {
                d.doc.mark_dirty_rect(r);
                any = true;
            }
        }
        if any {
            d.doc.commit(label);
        }
    }

    /// Delete: clear the selected pixels, and (unless the setting is off) drop
    /// the selection in the same history step, so one undo puts both back.
    fn clear_selected(&mut self) {
        self.state.settle();
        let deselect = self.state.settings.general.deselect_after_delete;
        let Some(d) = self.state.active_mut() else { return };
        let targets = d.target_layers();
        if targets.is_empty() {
            self.state.toasts.push(Level::Info, "The active layer is locked or hidden.");
            return;
        }
        let mut any = false;
        for li in targets {
            let r = ops::clear(d.doc.state_mut(), li);
            if !r.is_empty() {
                d.doc.mark_dirty_rect(r);
                any = true;
            }
        }
        if deselect && d.doc.state().selection.is_some() {
            d.doc.state_mut().selection = None;
            d.sel_outline = None;
            any = true;
        }
        if any {
            d.doc.commit("Clear");
        }
    }

    /// Apply a whole-document operation and commit.
    fn edit_doc(&mut self, label: &str, f: impl FnOnce(&mut qsketch_core::DocState), resizes: bool) {
        self.state.settle();
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
    /// The windowing layer reports every pen-tip contact as a primary press
    /// even when a barrel button mapped to right-click is held. Recover the
    /// intended button from the tablet backend / Windows pointer flags before
    /// egui sees the events, so context menus and `secondary_clicked()`
    /// everywhere (Layers, Brushes, Swatches, ...) get it, not only the canvas.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.trace.frame(raw_input);
        let barrel = self.state.pen.barrel_held || crate::win_pointer::barrel_held();
        for ev in &mut raw_input.events {
            let egui::Event::PointerButton { button, pressed, .. } = ev else { continue };
            if *button != egui::PointerButton::Primary {
                continue;
            }
            if *pressed {
                if barrel {
                    *button = egui::PointerButton::Secondary;
                }
                self.state.pen.tip_button = Some(*button);
            } else if let Some(b) = self.state.pen.tip_button.take() {
                *button = b;
            }
        }
    }

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
            if ui_now.theme != was.theme
                || ui_now.custom_palette != was.custom_palette
                || ui_now.scale != was.scale
                || ui_now.shape != was.shape
            {
                theme::apply(&ctx, &ui_now.palette(), ui_now.scale, ui_now.shape);
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
                self.state.autosave.flush_for_exit(&self.state.docs);
            }
        }

        if let Some(w) = self.wintab.as_mut() {
            w.pump(&mut self.state, &ctx);
        } else if let Some(t) = self.tablet.as_mut() {
            t.pump(&mut self.state);
        }
        self.handle_dropped_files(&ctx);
        self.handle_keyboard(&ctx);
        crate::anim::tick(&mut self.state, &ctx);
        self.tick_work_time(&ctx);
        self.enforce_fullscreen(&ctx);
        self.state.autosave.tick(&self.state.docs, &self.state.settings.general, self.state.session.is_some(), &ctx);
        if self.state.updater.ctx.is_none() {
            self.state.updater.ctx = Some(ctx.clone());
        }
        self.auto_update_check();
        if let Some(wait) = self.state.updater.tick() {
            ctx.request_repaint_after(wait);
        }
        if self.state.updater.poll() {
            ctx.request_repaint();
            if let Some(n) = self.state.updater.notice.take() {
                self.state.status_msg = Some((n, std::time::Instant::now()));
            }
            // An automatic check stays quiet about a version the user skipped.
            let skipped = &self.state.settings.update.skipped_version;
            if !self.state.updater.manual && self.state.updater.info.as_ref().is_some_and(|i| &i.version == skipped) {
                self.state.updater.show_dialog = false;
            }
        }
        self.process_requests(&ctx);
        crate::share::tick(&mut self.state, &ctx);
        // Indexed-color workflow: the current colors always come from the
        // palette while it is locked, wherever they were picked.
        if let Some(s) = self.state.active().map(|e| e.doc.state()) {
            if s.palette_lock && !s.palette.is_empty() {
                let (fg, bg) = (s.palette.snap(self.state.fg), s.palette.snap(self.state.bg));
                self.state.fg = fg;
                self.state.bg = bg;
            }
        }

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
        // Timelapse frames for documents edited this frame, and finished
        // background jobs.
        crate::timelapse::tick(&mut self.state, &ctx);
        if crate::timelapse::poll_jobs(&mut self.state) {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        // Actions may have created documents that still need tabs.
        let ids: Vec<DocId> = self.state.docs.iter().map(|d| d.id).collect();
        for id in ids {
            if !self.workspace.is_panel_open(&PanelKind::Document(id)) {
                self.workspace.add_document(id);
                ctx.request_repaint();
            }
        }

        // While cloaked the window may not have its final geometry yet.
        if self.cloak.is_none() {
            self.record_window_geometry(&ctx);
        }
        if self.last_settings_save.elapsed() > Duration::from_secs(30) {
            self.persist();
        }
        // TextEdit publishes an IME rect every frame it has focus; use that to
        // know when single-key shortcuts must stay out of the way.
        self.text_editing = ctx.output(|o| o.ime.is_some());

        // Startup animation over everything, from the first frame.
        if self.frames == 0 && self.state.settings.ui.startup_animation {
            self.splash = Some(crate::ui::splash::Splash::new());
        }
        if let Some(s) = &mut self.splash {
            // Painted while cloaked too (it is what gets uncloaked), but the
            // clock only starts once the window can be seen.
            if self.cloak.is_some() {
                s.hold();
            }
            if !s.paint(&ctx, &self.state.settings.ui.palette(), &self.app_icon) {
                self.splash = None;
            }
        }
        if self.cloak.as_mut().is_some_and(|c| !c.tick(&ctx)) {
            self.cloak = None;
        }
        self.frames = self.frames.saturating_add(1);
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

/// A layer above the active one filled with a Bayer pattern at low
/// opacity: under Posterize it nudges values across the thresholds, so
/// soft strokes come out dithered (Dan Fessler's HD index painting).
fn dither_layer(doc: &mut qsketch_core::Document) {
    use qsketch_core::filter::fx::dither_threshold;
    use qsketch_core::filter::DitherPattern;
    let s = doc.state_mut();
    let (w, h) = (s.width, s.height);
    let name = s.unique_layer_name("Dither");
    let id = s.add_layer(name, None);
    let Some(i) = s.index_of(id) else { return };
    // Mid-gray plus or minus the pattern, blended with Overlay: a flat area
    // keeps its value on average (pure black and white are untouched) and
    // only values near a posterize threshold flip, pixel by pixel.
    let mut r = qsketch_core::Raster::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let t = dither_threshold(x, y, DitherPattern::Bayer4);
            let v = ((0.5 + 0.4 * (t - 0.5)) * 255.0).round() as u8;
            r.set_pixel(x, y, qsketch_core::Rgba8::new(v, v, v, 255));
        }
    }
    let l = &mut s.layers[i];
    l.raster = r;
    l.props.blend = qsketch_core::BlendMode::Overlay;
    l.props.opacity = 0.5;
    doc.mark_all_dirty();
}

/// The HD index painting stack above the active layer: a dither pattern,
/// then Black & White, Posterize and Gradient Map adjustment layers.
fn index_painting_setup(doc: &mut qsketch_core::Document, fg: qsketch_core::Rgba8, bg: qsketch_core::Rgba8) {
    dither_layer(doc);
    let s = doc.state_mut();
    let (w, h) = (s.width, s.height);
    let palette = s.palette.colors.clone();
    let ramp = if palette.len() >= 2 {
        palette
    } else {
        // Darkest first, whichever of the two colors that is.
        let luma = |c: qsketch_core::Rgba8| 0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32;
        let (a, b) = if fg == bg {
            (qsketch_core::Rgba8::BLACK, qsketch_core::Rgba8::WHITE)
        } else if luma(fg) <= luma(bg) {
            (fg, bg)
        } else {
            (bg, fg)
        };
        (0..8)
            .map(|k| {
                let t = k as f32 / 7.0;
                let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
                qsketch_core::Rgba8::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), 255)
            })
            .collect()
    };
    let levels = ramp.len().clamp(2, 64) as u32;
    for (name, filter) in [
        ("Black & White", Filter::BlackWhite),
        ("Posterize", Filter::Posterize { levels }),
        ("Gradient Map", Filter::GradientMap { colors: ramp }),
    ] {
        let name = s.unique_layer_name(name);
        let id = s.add_layer(name, None);
        if let Some(i) = s.index_of(id) {
            let l = &mut s.layers[i];
            l.props.kind = qsketch_core::layer::LayerKind::Adjustment;
            l.props.adjustment = Some(filter);
            l.mask = Some(std::sync::Arc::new(qsketch_core::Mask::full(w, h)));
        }
    }
    // Back to the paint layer: the one the stack was built over.
    s.active = s.active.saturating_sub(4);
    doc.mark_all_dirty();
}

/// Right edge of the last top-level menu button this frame (temp data).
fn menu_bar_right_id() -> egui::Id {
    egui::Id::new("qsketch_menu_bar_right")
}

/// A top-level menu-bar button that also switches menus on hover: with one
/// bar menu open, hovering another title opens that one instead (egui 0.36's
/// `MenuBar` only switches on click).
fn top_menu<R>(ui: &mut Ui, label: &str, contents: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
    let ctx = ui.ctx().clone();
    // The title of the open menu reads as pressed, like a submenu row does
    // (egui's bar button only highlights under the pointer).
    let open_now = egui::Popup::is_id_open(&ctx, ui.next_auto_id().with("popup"));
    let inactive = ui.style().visuals.widgets.inactive;
    if open_now {
        ui.style_mut().visuals.widgets.inactive = ui.style().visuals.widgets.open;
    }
    let r = ui.menu_button(label, contents);
    ui.style_mut().visuals.widgets.inactive = inactive;
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
