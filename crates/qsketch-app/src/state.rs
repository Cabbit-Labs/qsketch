//! Application state shared by every panel, tool and dialog.

use std::collections::HashMap;
use std::path::PathBuf;

use egui::Pos2;
use qsketch_core::{BrushSettings, ClipImage, Document, LayerId, Pt, Rgba8};

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
    /// Layers highlighted in the Layers panel besides the active one (ids,
    /// not undoable). Only meaningful while it still contains the active
    /// layer: changing the active layer elsewhere implicitly collapses the
    /// selection back to that layer alone.
    pub selected: Vec<LayerId>,
    /// Active layer id as of the last canvas frame; a change starts a flash.
    pub flash_seen_active: Option<LayerId>,
    pub layer_flash: Option<crate::canvas::flash::LayerFlash>,
    /// Path whose format warnings (flattening, dropped features) the user
    /// has accepted, so plain Save there doesn't ask again.
    pub format_ack: Option<std::path::PathBuf>,
    /// The selection that was last cleared, for Select ▸ Reselect.
    pub last_selection: Option<std::sync::Arc<qsketch_core::Mask>>,
    /// Selection tint texture keyed by the selection's allocation address.
    pub sel_tint: Option<(usize, egui::TextureHandle, qsketch_core::IRect)>,
    /// Live collaboration over Leyline, when this document is shared.
    pub share: Option<crate::share::ShareSession>,
    /// The layer whose *mask* is the paint target (its mask thumbnail is
    /// selected in the Layers panel). Only matters while that layer is active.
    pub mask_edit: Option<LayerId>,
    /// Selected run of palette slots in the Palette panel (anchor, end),
    /// which the shading ink uses as its ramp.
    pub palette_sel: Option<(usize, usize)>,
}

impl DocEntry {
    pub fn new(id: DocId, doc: Document) -> Self {
        Self {
            id,
            doc,
            view: CanvasView::default(),
            sel_outline: None,
            generation: 1,
            needs_full_upload: true,
            selected: Vec::new(),
            format_ack: None,
            last_selection: None,
            sel_tint: None,
            share: None,
            mask_edit: None,
            palette_sel: None,
            flash_seen_active: None,
            layer_flash: None,
        }
    }

    /// Ids of the selected layers (always including the active one).
    pub fn selected_ids(&self) -> Vec<LayerId> {
        let s = self.doc.state();
        let active = s.active_layer().props.id;
        if self.selected.contains(&active) {
            let mut ids: Vec<LayerId> = self.selected.iter().copied().filter(|id| s.index_of(*id).is_some()).collect();
            if !ids.contains(&active) {
                ids.push(active);
            }
            ids
        } else {
            vec![active]
        }
    }

    /// Indices of the selected layers, bottom to top.
    pub fn selected_indices(&self) -> Vec<usize> {
        let s = self.doc.state();
        let mut v: Vec<usize> = self.selected_ids().iter().filter_map(|id| s.index_of(*id)).collect();
        v.sort_unstable();
        v
    }

    /// The editable raster layers an edit should touch: every selected
    /// layer, with groups standing for their members.
    pub fn target_layers(&self) -> Vec<usize> {
        self.doc.state().raster_layers_in(&self.selected_ids())
    }

    /// Painting goes into the active layer's mask rather than its pixels.
    pub fn editing_mask(&self) -> bool {
        let s = self.doc.state();
        let l = s.active_layer();
        self.mask_edit == Some(l.props.id) && l.mask.is_some()
    }

    /// `target_layers` as layer ids, for callers that outlive a single frame
    /// (indices shift when layers are added, deleted or reordered).
    pub fn target_layer_ids(&self) -> Vec<qsketch_core::LayerId> {
        let s = self.doc.state();
        self.target_layers().into_iter().filter_map(|i| s.layers.get(i).map(|l| l.props.id)).collect()
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
    /// barrel state, so `QSketchApp::raw_input_hook` uses this to recover a
    /// right-click before egui processes the frame.
    pub barrel_held: bool,
    /// The pen tip is currently down and its press was delivered to egui as
    /// this button (after barrel remapping), so the release can be matched.
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
    /// The quick-move chord's modifiers are held, or its drag is under way
    /// (see `MouseSettings::quick_move`).
    QuickMove,
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

/// A color pick in flight (the Eyedropper tool, or a pick chord held down).
/// The loupe follows the pointer until the button is released.
#[derive(Clone, Copy, Debug)]
pub struct PickPreview {
    pub doc: DocId,
    pub screen: egui::Pos2,
    pub doc_pos: qsketch_core::Pt,
    pub target: crate::settings::PickTarget,
    /// The color under the pointer right now.
    pub color: Rgba8,
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
    /// Brush used by the Selection Brush tool (paints selection, not pixels).
    pub select_brush: BrushSettings,
    pub smudge: BrushSettings,
    pub clone: BrushSettings,
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
    /// Panels asking to be closed when they are on a floating window (Escape).
    pub close_floating_requests: Vec<crate::workspace::PanelKind>,
    pub close_doc_requests: Vec<DocId>,
    pub open_file_requests: Vec<PathBuf>,
    /// Backups picked from File ▸ Restore Previous Version: (original, backup).
    pub restore_requests: Vec<(PathBuf, crate::backups::Backup)>,

    /// Pointer position over the active canvas (document coords), for the Info panel.
    pub hover_doc_pos: Option<Pt>,
    /// A brief system message shown centered in the status bar.
    pub status_msg: Option<(String, std::time::Instant)>,
    pub hover_screen_pos: Option<Pos2>,
    pub hover_color: Option<Rgba8>,
    /// A color pick in progress: drives the zoomed loupe over the canvas.
    pub pick_preview: Option<PickPreview>,
    /// Seconds the pointer has been held in the auto-scroll band (velocity build-up).
    pub edge_scroll_hold: f32,
    /// Photoshop-style eye sweep: the visibility every eye the pointer
    /// crosses gets while the button pressed on an eye stays down.
    pub eye_drag: Option<bool>,

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
    /// The connection to a running Leyline, opened the first time sharing is used.
    pub share: Option<crate::share::link::Link>,
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
            select_brush: paint.select_brush,
            smudge: paint.smudge,
            clone: paint.clone,
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
            close_floating_requests: Vec::new(),
            status_msg: None,
            close_doc_requests: Vec::new(),
            open_file_requests: Vec::new(),
            restore_requests: Vec::new(),
            hover_doc_pos: None,
            hover_screen_pos: None,
            hover_color: None,
            pick_preview: None,
            edge_scroll_hold: 0.0,
            eye_drag: None,
            tablet_samples: Vec::new(),
            render_state: None,
            thumbs: Default::default(),
            updater: Default::default(),
            autosave: Default::default(),
            library: crate::brush_library::BrushLibrary::load(),
            brush_page: Default::default(),
            last_filter: None,
            filter_memory: HashMap::new(),
            share: None,
        }
    }

    pub fn effective_tool(&self) -> ToolKind {
        self.temp_tool.map(|(t, _)| t).unwrap_or(self.tool)
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

    /// Land every edit still in flight before the document is changed under
    /// it: the stroke in progress is dropped, floating pixels (a paste or a
    /// transform) and a text placement are committed. The three each hold a
    /// layer *index* and render their preview into the working state, so an
    /// action that edits the layer list or commits while they are up would
    /// point them at the wrong layer and snapshot the preview into history —
    /// where Esc could no longer take it back.
    pub fn settle(&mut self) {
        // A live filter preview lives in the working state: take it off
        // before anything else commits, or it is baked into that undo step
        // (and the next parameter change filters an already-filtered image).
        if let Some(d) = self.dialogs.filter.as_mut() {
            if d.applied.take().is_some() {
                let doc = d.doc;
                if let Some(e) = self.docs.iter_mut().find(|x| x.id == doc) {
                    e.doc.revert_working();
                }
            }
        }
        crate::dialogs::liquify::revert(self);
        self.cancel_session();
        crate::tools::floating::commit(self);
        crate::tools::text::commit(self);
    }

    pub fn cancel_session(&mut self) {
        // A Liquify drag only captures the pointer; its preview belongs to the
        // dialog and must not be reverted from under it.
        if matches!(self.session, Some(crate::tools::ToolSession::Liquify)) {
            self.session = None;
            self.session_doc = None;
            return;
        }
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
        crate::share::stop(self, id);
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
        self.brush_for_tool(self.effective_tool())
    }

    /// Brush settings for a specific tool, regardless of any temporary tool.
    pub fn brush_for_tool(&self, tool: ToolKind) -> Option<&BrushSettings> {
        match tool {
            ToolKind::Brush | ToolKind::Line => Some(&self.brush),
            ToolKind::Pencil => Some(&self.pencil),
            ToolKind::Eraser => Some(&self.eraser),
            ToolKind::SelectBrush => Some(&self.select_brush),
            ToolKind::Smudge => Some(&self.smudge),
            ToolKind::Clone => Some(&self.clone),
            _ => None,
        }
    }

    pub fn current_brush_mut(&mut self) -> Option<&mut BrushSettings> {
        self.brush_for_tool_mut(self.effective_tool())
    }

    /// Brush settings for a specific tool, regardless of any temporary tool.
    pub fn brush_for_tool_mut(&mut self, tool: ToolKind) -> Option<&mut BrushSettings> {
        match tool {
            ToolKind::Brush | ToolKind::Line => Some(&mut self.brush),
            ToolKind::Pencil => Some(&mut self.pencil),
            ToolKind::Eraser => Some(&mut self.eraser),
            ToolKind::SelectBrush => Some(&mut self.select_brush),
            ToolKind::Smudge => Some(&mut self.smudge),
            ToolKind::Clone => Some(&mut self.clone),
            _ => None,
        }
    }

    /// Drop the Brushes panel's stroke previews (keyed by row) so edited
    /// presets re-render.
    pub fn forget_preset_previews(&mut self) {
        for k in 0..=self.presets.len() {
            self.library.forget_stroke_preview(&format!("preset-{k}"));
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
        self.settings.paint.select_brush = self.select_brush.clone();
        self.settings.paint.smudge = self.smudge.clone();
        self.settings.paint.clone = self.clone.clone();
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
