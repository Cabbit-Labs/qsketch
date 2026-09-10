//! Dockable workspace built on egui_dock: document tabs in the center, tool
//! and property panels around them, all draggable/floatable and persisted.

use egui::{Ui, WidgetText};
use egui_dock::widgets::tab_viewer::OnCloseResponse;
use egui_dock::{DockArea, DockState, NodeIndex, NodePath, SurfaceIndex, TabPath, TabViewer};
use serde::{Deserialize, Serialize};

use crate::panels;
use crate::state::{AppState, DocId};
use crate::ui::icons;
use crate::ui::theme::Section;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PanelKind {
    Home,
    Document(DocId),
    Tools,
    Layers,
    History,
    Color,
    Swatches,
    Navigator,
    Brushes,
    BrushSettings,
    Info,
}

impl PanelKind {
    #[allow(dead_code)]
    pub const PANELS: [PanelKind; 9] = [
        PanelKind::Tools,
        PanelKind::Layers,
        PanelKind::History,
        PanelKind::Color,
        PanelKind::Swatches,
        PanelKind::Navigator,
        PanelKind::Brushes,
        PanelKind::BrushSettings,
        PanelKind::Info,
    ];

    pub fn static_title(&self) -> &'static str {
        match self {
            PanelKind::Home => "Home",
            PanelKind::Document(_) => "Document",
            PanelKind::Tools => "Tools",
            PanelKind::Layers => "Layers",
            PanelKind::History => "History",
            PanelKind::Color => "Color",
            PanelKind::Swatches => "Swatches",
            PanelKind::Navigator => "Navigator",
            PanelKind::Brushes => "Brushes",
            PanelKind::BrushSettings => "Brush Settings",
            PanelKind::Info => "Info",
        }
    }

    /// Color-coding family of the panel (`None` for documents / Home).
    pub fn section(&self) -> Option<Section> {
        match self {
            PanelKind::Home | PanelKind::Document(_) => None,
            PanelKind::Tools => Some(Section::Tools),
            PanelKind::Layers | PanelKind::History => Some(Section::Layers),
            PanelKind::Color | PanelKind::Swatches => Some(Section::Color),
            PanelKind::Navigator | PanelKind::Info => Some(Section::View),
            PanelKind::Brushes | PanelKind::BrushSettings => Some(Section::Brush),
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            PanelKind::Home => icons::HOUSE,
            PanelKind::Document(_) => icons::IMAGE,
            PanelKind::Tools => icons::WRENCH,
            PanelKind::Layers => icons::STACK,
            PanelKind::History => icons::CLOCK_COUNTER_CLOCKWISE,
            PanelKind::Color => icons::PALETTE,
            PanelKind::Swatches => icons::SWATCHES,
            PanelKind::Navigator => icons::COMPASS,
            PanelKind::Brushes => icons::PAINT_BRUSH_HOUSEHOLD,
            PanelKind::BrushSettings => icons::SLIDERS_HORIZONTAL,
            PanelKind::Info => icons::INFO,
        }
    }
}

pub struct Workspace {
    pub dock: DockState<PanelKind>,
}

impl Workspace {
    pub fn default_layout() -> DockState<PanelKind> {
        let mut dock = DockState::new(vec![PanelKind::Home]);
        let root = NodeIndex::root();
        let tree = dock.main_surface_mut();
        // Tools strip on the left.
        let [center, _tools] = tree.split_left(root, 0.05, vec![PanelKind::Tools]);
        // Right column.
        let [_center, right_top] = tree.split_right(center, 0.78, vec![PanelKind::Color, PanelKind::Swatches]);
        let [_right_top, right_mid] =
            tree.split_below(right_top, 0.32, vec![PanelKind::Navigator, PanelKind::Brushes, PanelKind::Info]);
        let [_right_mid, _right_bottom] = tree.split_below(right_mid, 0.4, vec![PanelKind::Layers, PanelKind::History]);
        dock
    }

    pub fn new() -> Self {
        Self { dock: Self::default_layout() }
    }

    pub fn from_json(json: &str) -> Option<Self> {
        let mut dock: DockState<PanelKind> = serde_json::from_str(json).ok()?;
        // Documents never survive a restart.
        dock.retain_tabs(|t| !matches!(t, PanelKind::Document(_)));
        if !dock.iter_all_tabs().any(|(_, t)| *t == PanelKind::Home) {
            dock.push_to_first_leaf(PanelKind::Home);
        }
        Some(Self { dock })
    }

    pub fn to_json(&self) -> Option<String> {
        let mut dock = self.dock.clone();
        dock.retain_tabs(|t| !matches!(t, PanelKind::Document(_)));
        serde_json::to_string(&dock).ok()
    }

    pub fn reset(&mut self, docs: &[DocId]) {
        self.dock = Self::default_layout();
        for &d in docs {
            self.add_document(d);
        }
    }

    fn find(&self, kind: &PanelKind) -> Option<TabPath> {
        self.dock.find_tab(kind)
    }

    /// Open a document tab next to the other documents (or the Home tab).
    pub fn add_document(&mut self, id: DocId) {
        let kind = PanelKind::Document(id);
        if let Some(path) = self.find(&kind) {
            let _ = self.dock.set_active_tab(path);
            return;
        }
        // Find the leaf holding another document or Home.
        let anchor = self
            .dock
            .iter_all_tabs()
            .find(|(_, t)| matches!(t, PanelKind::Document(_)))
            .or_else(|| self.dock.iter_all_tabs().find(|(_, t)| **t == PanelKind::Home))
            .map(|(p, _)| NodePath { surface: p.surface, node: p.node });
        match anchor {
            Some(np) => {
                self.dock.set_focused_node_and_surface(np);
                self.dock.push_to_focused_leaf(kind);
            }
            None => self.dock.push_to_first_leaf(kind),
        }
    }

    pub fn remove_document(&mut self, id: DocId) {
        if let Some(path) = self.find(&PanelKind::Document(id)) {
            self.dock.remove_tab(path);
        }
    }

    pub fn focus_document(&mut self, id: DocId) {
        if let Some(path) = self.find(&PanelKind::Document(id)) {
            let _ = self.dock.set_active_tab(path);
        }
    }

    /// Show a panel, re-adding it next to a sensible neighbour if it was closed.
    pub fn show_panel(&mut self, kind: PanelKind) {
        if let Some(path) = self.find(&kind) {
            let _ = self.dock.set_active_tab(path);
            return;
        }
        let neighbour = match kind {
            PanelKind::Layers => PanelKind::History,
            PanelKind::History => PanelKind::Layers,
            PanelKind::Color => PanelKind::Swatches,
            PanelKind::Swatches => PanelKind::Color,
            PanelKind::Navigator | PanelKind::Brushes | PanelKind::Info => PanelKind::Layers,
            PanelKind::BrushSettings => PanelKind::Brushes,
            _ => PanelKind::Home,
        };
        // Brush Settings is a large editor: open it floating rather than squeezing it into a column.
        if kind != PanelKind::Tools && kind != PanelKind::BrushSettings {
            let found = self
                .dock
                .iter_all_tabs()
                .find(|(_, t)| **t == neighbour)
                .map(|(p, _)| NodePath { surface: p.surface, node: p.node });
            if let Some(np) = found {
                self.dock.set_focused_node_and_surface(np);
                self.dock.push_to_focused_leaf(kind);
                return;
            }
        }
        // Fall back to a floating window.
        let size = if kind == PanelKind::BrushSettings { egui::vec2(640.0, 560.0) } else { egui::vec2(260.0, 360.0) };
        let rect = egui::Rect::from_min_size(egui::pos2(120.0, 120.0), size);
        let surface = self.dock.add_window(vec![kind]);
        if let Some(ws) = self.dock.get_window_state_mut(surface) {
            ws.set_position(rect.min);
            ws.set_size(rect.size());
        }
    }

    pub fn is_panel_open(&self, kind: &PanelKind) -> bool {
        self.find(kind).is_some()
    }

    /// Keep the Tools strip a fixed pixel width instead of a fraction of the
    /// window, so it doesn't balloon on wide displays.
    fn pin_tools_width(&mut self) {
        const TOOLS_WIDTH: f32 = 86.0;
        let Some(path) = self.find(&PanelKind::Tools) else { return };
        if path.surface != SurfaceIndex::main() {
            return;
        }
        let tree = self.dock.main_surface_mut();
        let node = path.node;
        let Some(parent) = node.parent() else { return };
        let Some(egui_dock::Node::Horizontal(split)) = tree.iter_mut().nth(parent.0) else { return };
        let width = split.rect.width();
        if width <= TOOLS_WIDTH * 2.0 {
            return;
        }
        let want = if node.is_left() { TOOLS_WIDTH / width } else { 1.0 - TOOLS_WIDTH / width };
        if (split.fraction - want).abs() > 0.0005 {
            split.fraction = want;
        }
    }

    pub fn show(&mut self, ui: &mut Ui, state: &mut AppState) {
        self.pin_tools_width();
        let style = crate::ui::theme::dock_style(ui.ctx(), &state.settings.ui.palette());
        let mut viewer = Viewer { state };
        DockArea::new(&mut self.dock)
            .id(egui::Id::new("qsketch_dock"))
            .style(style)
            .show_add_buttons(false)
            .show_close_buttons(true)
            .show_leaf_close_all_buttons(false)
            .show_leaf_collapse_buttons(false)
            .draggable_tabs(true)
            .show_tab_name_on_hover(false)
            .show_inside(ui, &mut viewer);
        if let Some((_, PanelKind::Document(id))) = self.dock.find_active_focused() {
            let id = *id;
            if state.doc(id).is_some() {
                state.active_doc = Some(id);
            }
        }
        self.snap_windows(ui.ctx(), state);
    }

    /// Snap floating panel windows to the screen edges after they are dropped.
    fn snap_windows(&mut self, ctx: &egui::Context, state: &AppState) {
        if !state.settings.ui.snap_windows {
            return;
        }
        let screen = ctx.content_rect();
        let d = state.settings.ui.snap_distance;
        let count = self.dock.surfaces_count();
        for i in 0..count {
            let surface = SurfaceIndex(i);
            if surface == SurfaceIndex::main() {
                continue;
            }
            let Some(ws) = self.dock.get_window_state_mut(surface) else { continue };
            if ws.dragged() {
                continue;
            }
            let r = ws.rect();
            let mut pos = r.min;
            let mut snapped = false;
            if (r.min.x - screen.min.x).abs() < d {
                pos.x = screen.min.x;
                snapped = true;
            } else if (r.max.x - screen.max.x).abs() < d {
                pos.x = screen.max.x - r.width();
                snapped = true;
            }
            if (r.min.y - screen.min.y).abs() < d {
                pos.y = screen.min.y;
                snapped = true;
            } else if (r.max.y - screen.max.y).abs() < d {
                pos.y = screen.max.y - r.height();
                snapped = true;
            }
            if snapped && (pos - r.min).length() > 0.01 {
                ws.set_position(pos);
            }
        }
    }
}

struct Viewer<'a> {
    state: &'a mut AppState,
}

impl TabViewer for Viewer<'_> {
    type Tab = PanelKind;

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(("qsketch_tab", tab.clone()))
    }

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        match tab {
            // The Tools strip title carries a lock: click it to allow or
            // prevent rearranging tools by drag.
            PanelKind::Tools => {
                let p = self.state.settings.ui.palette();
                let locked = self.state.settings.ui.tools_locked;
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    if locked { icons::LOCK_SIMPLE } else { icons::LOCK_SIMPLE_OPEN },
                    0.0,
                    egui::TextFormat {
                        font_id: egui::FontId::new(13.0, crate::ui::iconset::family()),
                        color: if locked { p.text_dim } else { p.accent },
                        ..Default::default()
                    },
                );
                job.append(
                    "Tools",
                    4.0,
                    egui::TextFormat { font_id: egui::FontId::proportional(12.5), color: p.text, ..Default::default() },
                );
                job.into()
            }
            PanelKind::Document(id) => match self.state.doc(*id) {
                Some(d) => format!("{} {}", icons::IMAGE, d.doc.display_title()).into(),
                None => "(closed)".into(),
            },
            other => {
                let Some(_sec) = other.section() else {
                    return format!("{} {}", other.icon(), other.static_title()).into();
                };
                // Section-colored icon, plain title text.
                let p = self.state.settings.ui.palette();
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    other.icon(),
                    0.0,
                    egui::TextFormat {
                        font_id: egui::FontId::new(13.0, crate::ui::iconset::family()),
                        color: p.text_dim,
                        ..Default::default()
                    },
                );
                job.append(
                    other.static_title(),
                    4.0,
                    egui::TextFormat { font_id: egui::FontId::proportional(12.5), color: p.text, ..Default::default() },
                );
                job.into()
            }
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        if self.state.settings.ui.texture.enabled() && !matches!(tab, PanelKind::Document(_)) {
            let p = self.state.settings.ui.palette();
            crate::ui::chrome::paint_ui(ui, &p, &self.state.settings.ui.texture);
        }
        match tab {
            PanelKind::Home => panels::home::ui(ui, self.state),
            PanelKind::Document(id) => {
                if self.state.doc(*id).is_some() {
                    crate::canvas::show(ui, self.state, *id);
                } else {
                    ui.label("This document has been closed.");
                }
            }
            PanelKind::Tools => panels::tools_panel::ui(ui, self.state),
            PanelKind::Layers => panels::layers::ui(ui, self.state),
            PanelKind::History => panels::history::ui(ui, self.state),
            PanelKind::Color => panels::color::ui(ui, self.state),
            PanelKind::Swatches => panels::swatches::ui(ui, self.state),
            PanelKind::Navigator => panels::navigator::ui(ui, self.state),
            PanelKind::Brushes => panels::brushes::ui(ui, self.state),
            PanelKind::BrushSettings => panels::brush_settings::ui(ui, self.state),
            PanelKind::Info => panels::info::ui(ui, self.state),
        }
    }

    fn on_tab_button(&mut self, tab: &mut Self::Tab, response: &egui::Response) {
        if *tab == PanelKind::Tools {
            let locked = self.state.settings.ui.tools_locked;
            response.clone().on_hover_text(if locked {
                "Tools are locked in place. Click to unlock and drag tools to rearrange them."
            } else {
                "Tools can be dragged to rearrange. Click to lock them in place."
            });
            if response.clicked() {
                self.state.settings.ui.tools_locked = !locked;
            }
        }
        if let PanelKind::Document(id) = tab {
            if response.clicked() || response.middle_clicked() {
                self.state.active_doc = Some(*id);
            }
            if response.middle_clicked() {
                self.state.close_doc_requests.push(*id);
            }
        }
    }

    fn is_closeable(&self, tab: &Self::Tab) -> bool {
        // Tools shows only a lock glyph; a close button next to it would be
        // too easy to hit. Hide it via Window > Tools instead.
        !matches!(tab, PanelKind::Home | PanelKind::Tools)
    }

    fn on_close(&mut self, tab: &mut Self::Tab) -> OnCloseResponse {
        match tab {
            PanelKind::Document(id) => {
                // Route through the app so unsaved changes are confirmed.
                self.state.close_doc_requests.push(*id);
                OnCloseResponse::Ignore
            }
            PanelKind::Home => OnCloseResponse::Ignore,
            _ => OnCloseResponse::Close,
        }
    }

    fn allowed_in_windows(&self, _tab: &mut Self::Tab) -> bool {
        true
    }

    fn clear_background(&self, tab: &Self::Tab) -> bool {
        !matches!(tab, PanelKind::Document(_))
    }

    fn scroll_bars(&self, tab: &Self::Tab) -> [bool; 2] {
        match tab {
            // History runs its own scroll area; a second one from the dock
            // put the footer behind a second scroll bar.
            PanelKind::Document(_)
            | PanelKind::Home
            | PanelKind::Tools
            | PanelKind::BrushSettings
            | PanelKind::History => [false, false],
            _ => [false, true],
        }
    }

    fn context_menu(&mut self, ui: &mut Ui, tab: &mut Self::Tab, _path: egui_dock::NodePath) {
        if let PanelKind::Document(id) = tab {
            let id = *id;
            if ui.button("Close").clicked() {
                self.state.close_doc_requests.push(id);
                ui.close();
            }
            if ui.button("Close Others").clicked() {
                let others: Vec<DocId> = self.state.docs.iter().map(|d| d.id).filter(|d| *d != id).collect();
                self.state.close_doc_requests.extend(others);
                ui.close();
            }
        }
    }
}
