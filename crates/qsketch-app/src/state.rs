//! Application state shared by every panel, tool and dialog.

use std::collections::HashMap;
use std::path::PathBuf;

use egui::Pos2;
use qsketch_core::{BrushSettings, ClipImage, Document, Pt, Rgba8};

use crate::actions::{Action, Keymap};
use crate::canvas::view::CanvasView;
use crate::dialogs::Dialogs;
use crate::settings::Settings;
use crate::tools::{ToolKind, ToolOptions, ToolSession};
use crate::ui::toasts::Toasts;

pub type DocId = u64;

/// How many recently used colors the Color panel keeps.
pub const COLOR_HISTORY_LEN: usize = 20;

pub struct DocEntry {
    pub id: DocId,
    pub doc: Document,
    pub view: CanvasView,
    /// Cached marching-ants outline keyed by the selection's allocation address.
    pub sel_outline: Option<(usize, Vec<[Pt; 2]>)>,
    /// Bumped whenever the composite changes, so thumbnails can refresh lazily.
    pub generation: u64,
    /// Whether the GPU texture must receive every tile this frame.
    pub needs_full_upload: bool,
}

impl DocEntry {
    pub fn new(id: DocId, doc: Document) -> Self {
        Self { id, doc, view: CanvasView::default(), sel_outline: None, generation: 1, needs_full_upload: true }
    }
}

/// Latest stylus information gathered from input events.
#[derive(Clone, Copy, Debug, Default)]
pub struct PenState {
    /// Pressure reported by the last pen event (None = mouse).
    pub pressure: Option<f32>,
    pub in_contact: bool,
    pub eraser: bool,
    pub tilt: [f32; 2],
    /// A tablet backend is delivering pen poses this frame.
    pub tablet_active: bool,
    /// A barrel (side) button on the stylus is currently held. The windowing
    /// layer turns every pen contact into a primary press regardless of the
    /// barrel state, so the canvas uses this to recover a right-click.
    pub barrel_held: bool,
    /// The pen tip is currently down and its press was delivered to the
    /// canvas as this button (after barrel remapping), so the release
    /// can be matched to it.
    pub tip_button: Option<egui::PointerButton>,
}

/// Why a temporary tool override is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TempReason {
    Space,
    /// `Q` is held: quick-rotate the view.
    QuickRotate,
    /// A color-pick chord's modifiers are held (see `MouseSettings`).
    Pick,
    Ctrl,
    EraserTip,
    /// Middle-button drag pans (see `MouseSettings::middle_drag_pans`).
    Middle,
}

/// The quick brush settings popup opened by right-clicking the canvas.
#[derive(Clone, Copy, Debug)]
pub struct BrushPopup {
    pub pos: Pos2,
    pub tool: ToolKind,
    /// Screen rect from the last frame, so canvas input inside it is ignored.
    pub rect: egui::Rect,
    pub just_opened: bool,
}

pub struct AppState {
    pub docs: Vec<DocEntry>,
    pub active_doc: Option<DocId>,
    pub next_doc_id: DocId,

    pub tool: ToolKind,
    pub temp_tool: Option<(ToolKind, TempReason)>,
    pub session: Option<ToolSession>,
    pub session_doc: Option<DocId>,
    pub tool_opts: ToolOptions,
    /// End point of the last stroke for Shift+click straight lines.
    pub last_stroke_end: Option<(DocId, Pt)>,
    /// Pasted pixels awaiting placement (move/scale) before commit.
    pub floating: Option<crate::tools::floating::FloatingPaste>,
    /// Text being typed/placed by the Text tool.
    pub text_edit: Option<crate::tools::text::TextEdit>,
    /// Fonts for the Text tool (loaded lazily).
    pub fonts: crate::fonts::FontLibrary,
    pub brush_popup: Option<BrushPopup>,

    pub fg: Rgba8,
    pub bg: Rgba8,
    pub swatches: Vec<Rgba8>,
    /// Most recently painted-with colors, newest first (Color panel).
    pub color_history: Vec<Rgba8>,
    /// Mirror / radial painting.
    pub symmetry: crate::tools::symmetry::Symmetry,
    /// The next canvas press sets the symmetry center.
    pub symmetry_pick_center: bool,
    pub brush: BrushSettings,
    pub pencil: BrushSettings,
    pub eraser: BrushSettings,
    pub presets: Vec<BrushSettings>,

    pub clipboard: Option<ClipImage>,
    pub settings: Settings,
    pub keymap: Keymap,
    pub toasts: Toasts,
    pub dialogs: Dialogs,
    pub pen: PenState,

    /// Actions queued by UI code, executed by the app after the frame's UI.
    pub pending: Vec<Action>,
    pub panels_hidden: bool,
    pub fullscreen: bool,
    pub quit_requested: bool,
    pub layout_reset_requested: bool,
    pub show_panel_requests: Vec<crate::workspace::PanelKind>,
    pub close_doc_requests: Vec<DocId>,
    pub open_file_requests: Vec<PathBuf>,

    /// Pointer position over the active canvas (document coords), for the Info panel.
    pub hover_doc_pos: Option<Pt>,
    pub hover_screen_pos: Option<Pos2>,
    pub hover_color: Option<Rgba8>,

    /// Pen samples (screen pos, pressure) delivered by the tablet backend this frame.
    pub tablet_samples: Vec<(Pos2, f32)>,
    /// GPU render state, for releasing document textures.
    pub render_state: Option<egui_wgpu::RenderState>,
    pub thumbs: crate::panels::thumbs::ThumbCache,
    pub updater: crate::update::Updater,
    /// Crash-recovery snapshots of unsaved documents.
    pub autosave: crate::autosave::Autosave,
    /// Brush tips and grain textures (built-in + user PNGs).
    pub library: crate::brush_library::BrushLibrary,
    /// Section shown in the Brush Settings panel.
    pub brush_page: crate::panels::brush_settings::Page,

    /// The most recently applied filter (Filter ▸ Last Filter, Ctrl+F).
    pub last_filter: Option<qsketch_core::Filter>,
    /// Last-used parameters per filter kind, keyed by `Filter::id()`.
    pub filter_memory: HashMap<&'static str, qsketch_core::Filter>,
}

impl AppState {
    pub fn new(settings: Settings) -> Self {
        let keymap = Keymap::with_overrides(&settings.shortcuts);
        let paint = settings.paint.clone();
        Self {
            docs: Vec::new(),
            active_doc: None,
            next_doc_id: 1,
            tool: ToolKind::Brush,
            temp_tool: None,
            session: None,
            session_doc: None,
            tool_opts: ToolOptions::default(),
            last_stroke_end: None,
            floating: None,
            text_edit: None,
            fonts: Default::default(),
            brush_popup: None,
            fg: paint.foreground,
            bg: paint.background,
            swatches: paint.swatches,
            color_history: paint.recent_colors,
            symmetry: paint.symmetry,
            symmetry_pick_center: false,
            brush: paint.brush,
            pencil: paint.pencil,
            eraser: paint.eraser,
            presets: paint.presets,
            clipboard: None,
            settings,
            keymap,
            toasts: Toasts::default(),
            dialogs: Dialogs::default(),
            pen: PenState::default(),
            pending: Vec::new(),
            panels_hidden: false,
            fullscreen: false,
            quit_requested: false,
            layout_reset_requested: false,
            show_panel_requests: Vec::new(),
            close_doc_requests: Vec::new(),
            open_file_requests: Vec::new(),
            hover_doc_pos: None,
            hover_screen_pos: None,
            hover_color: None,
            tablet_samples: Vec::new(),
            render_state: None,
            thumbs: Default::default(),
            updater: Default::default(),
            autosave: Default::default(),
            library: crate::brush_library::BrushLibrary::load(),
            brush_page: Default::default(),
            last_filter: None,
            filter_memory: HashMap::new(),
        }
    }

    pub fn effective_tool(&self) -> ToolKind {
        self.temp_tool.map(|(t, _)| t).unwrap_or(self.tool)
    }

    /// The pointer is over the canvas, inside the active document's selection.
    /// Ctrl-dragging there moves the selected pixels with any tool.
    pub fn hovering_selection(&self) -> bool {
        let Some(p) = self.hover_doc_pos else { return false };
        let Some(d) = self.active() else { return false };
        d.doc.state().selection_mask().is_some_and(|m| m.get(p.x.floor() as i32, p.y.floor() as i32) > 0)
    }

    pub fn set_tool(&mut self, tool: ToolKind) {
        if self.tool != tool {
            self.cancel_session();
            // Switching tools finalizes a floating paste (Photoshop behavior) or text.
            crate::tools::floating::commit(self);
            crate::tools::text::commit(self);
            self.tool = tool;
            self.tool_opts.crop_rect = None;
            self.brush_popup = None;
        }
    }

    pub fn cancel_session(&mut self) {
        if self.session.take().is_some() {
            if let Some(id) = self.session_doc.take() {
                if let Some(d) = self.doc_mut(id) {
                    d.doc.revert_working();
                }
            }
        }
        self.session_doc = None;
    }

    pub fn doc(&self, id: DocId) -> Option<&DocEntry> {
        self.docs.iter().find(|d| d.id == id)
    }

    pub fn doc_mut(&mut self, id: DocId) -> Option<&mut DocEntry> {
        self.docs.iter_mut().find(|d| d.id == id)
    }

    pub fn active(&self) -> Option<&DocEntry> {
        self.active_doc.and_then(|id| self.doc(id))
    }

    pub fn active_mut(&mut self) -> Option<&mut DocEntry> {
        let id = self.active_doc?;
        self.doc_mut(id)
    }

    pub fn add_document(&mut self, mut doc: Document) -> DocId {
        let id = self.next_doc_id;
        self.next_doc_id += 1;
        doc.history.set_limit(self.settings.general.undo_limit);
        self.docs.push(DocEntry::new(id, doc));
        self.active_doc = Some(id);
        id
    }

    pub fn remove_document(&mut self, id: DocId) {
        if self.session_doc == Some(id) {
            self.session = None;
            self.session_doc = None;
        }
        if self.floating.as_ref().is_some_and(|f| f.doc == id) {
            self.floating = None;
        }
        if self.text_edit.as_ref().is_some_and(|t| t.doc == id) {
            self.text_edit = None;
        }
        self.docs.retain(|d| d.id != id);
        if self.active_doc == Some(id) {
            self.active_doc = self.docs.last().map(|d| d.id);
        }
    }

    /// Brush settings used by the current painting tool.
    pub fn current_brush(&self) -> Option<&BrushSettings> {
        match self.effective_tool() {
            ToolKind::Brush | ToolKind::Line | ToolKind::Rect | ToolKind::Ellipse => Some(&self.brush),
            ToolKind::Pencil => Some(&self.pencil),
            ToolKind::Eraser => Some(&self.eraser),
            _ => None,
        }
    }

    pub fn current_brush_mut(&mut self) -> Option<&mut BrushSettings> {
        self.brush_for_tool_mut(self.effective_tool())
    }

    /// Brush settings for a specific tool, regardless of any temporary tool.
    pub fn brush_for_tool_mut(&mut self, tool: ToolKind) -> Option<&mut BrushSettings> {
        match tool {
            ToolKind::Brush | ToolKind::Line | ToolKind::Rect | ToolKind::Ellipse => Some(&mut self.brush),
            ToolKind::Pencil => Some(&mut self.pencil),
            ToolKind::Eraser => Some(&mut self.eraser),
            _ => None,
        }
    }

    /// Unique untitled document title.
    pub fn untitled_title(&self) -> String {
        let mut n = 1;
        loop {
            let t = format!("Untitled-{n}");
            if !self.docs.iter().any(|d| d.doc.title == t) {
                return t;
            }
            n += 1;
        }
    }

    /// Copy live paint settings back into `settings` for persistence.
    pub fn sync_settings(&mut self) {
        self.settings.paint.brush = self.brush.clone();
        self.settings.paint.pencil = self.pencil.clone();
        self.settings.paint.eraser = self.eraser.clone();
        self.settings.paint.presets = self.presets.clone();
        self.settings.paint.foreground = self.fg;
        self.settings.paint.background = self.bg;
        self.settings.paint.swatches = self.swatches.clone();
        self.settings.paint.recent_colors = self.color_history.clone();
        self.settings.paint.symmetry = self.symmetry;
        self.settings.shortcuts = self.keymap.overrides();
    }

    /// Record a color the user painted with (front of the Color panel's history).
    pub fn note_color_used(&mut self, c: Rgba8) {
        if self.color_history.first() == Some(&c) {
            return;
        }
        self.color_history.retain(|x| *x != c);
        self.color_history.insert(0, c);
        self.color_history.truncate(COLOR_HISTORY_LEN);
    }

    /// Apply the pressure curve from settings to a raw pressure value.
    pub fn curve_pressure(&self, raw: f32) -> f32 {
        let t = &self.settings.tablet;
        let p = if raw <= t.min_pressure { 0.0 } else { (raw - t.min_pressure) / (1.0 - t.min_pressure) };
        qsketch_core::brush::pressure_curve(p, t.pressure_gamma)
    }
}
