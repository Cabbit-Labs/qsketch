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
    Palette,
    Navigator,
    Brushes,
    BrushSettings,
    Info,
    Reference,
}

impl PanelKind {
    #[allow(dead_code)]
    pub const PANELS: [PanelKind; 11] = [
        PanelKind::Tools,
        PanelKind::Layers,
        PanelKind::History,
        PanelKind::Color,
        PanelKind::Swatches,
        PanelKind::Palette,
        PanelKind::Navigator,
        PanelKind::Brushes,
        PanelKind::BrushSettings,
        PanelKind::Info,
        PanelKind::Reference,
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
            PanelKind::Palette => "Palette",
            PanelKind::Navigator => "Navigator",
            PanelKind::Brushes => "Brushes",
            PanelKind::BrushSettings => "Brush Settings",
            PanelKind::Info => "Info",
            PanelKind::Reference => "Reference",
        }
    }

    /// Color-coding family of the panel (`None` for documents / Home).
    pub fn section(&self) -> Option<Section> {
        match self {
            PanelKind::Home | PanelKind::Document(_) => None,
            PanelKind::Tools => Some(Section::Tools),
            PanelKind::Layers | PanelKind::History => Some(Section::Layers),
            PanelKind::Color | PanelKind::Swatches | PanelKind::Palette => Some(Section::Color),
            PanelKind::Navigator | PanelKind::Info | PanelKind::Reference => Some(Section::View),
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
            PanelKind::Palette => icons::GRID_FOUR,
            PanelKind::Navigator => icons::COMPASS,
            PanelKind::Brushes => icons::PAINT_BRUSH_HOUSEHOLD,
            PanelKind::BrushSettings => icons::SLIDERS_HORIZONTAL,
            PanelKind::Info => icons::INFO,
            PanelKind::Reference => icons::IMAGES,
        }
    }
}

pub struct Workspace {
    pub dock: DockState<PanelKind>,
}

/// How [`Workspace::arrange_documents`] lays the documents out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrange {
    Tabs,
    SideBySide,
    Stacked,
    Grid,
}

impl Workspace {
    pub fn default_layout() -> DockState<PanelKind> {
        let mut dock = DockState::new(vec![PanelKind::Home]);
        let root = NodeIndex::root();
        let tree = dock.main_surface_mut();
        // Tools strip on the left.
        let [center, _tools] = tree.split_left(root, 0.05, vec![PanelKind::Tools]);
        // Right column.
        let [_center, right_top] =
            tree.split_right(center, 0.78, vec![PanelKind::Color, PanelKind::Swatches, PanelKind::Palette]);
        let [_right_top, right_mid] =
            tree.split_below(right_top, 0.32, vec![PanelKind::Navigator, PanelKind::Brushes, PanelKind::Info]);
        let [_right_mid, _right_bottom] = tree.split_below(right_mid, 0.4, vec![PanelKind::Layers, PanelKind::History]);
        dock
    }

    pub fn new() -> Self {
        Self { dock: Self::default_layout() }
    }

    pub fn from_json(json: &str) -> Option<Self> {
        // A leaf that was never shown (created and closed within one session)
        // carries a non-finite rect, which serde_json wrote as `null`; the
        // rects are recomputed on the first frame, so any number will do.
        let json = json.replace(":null", ":0.0");
        let mut dock: DockState<PanelKind> = serde_json::from_str(&json).ok()?;
        // Documents never survive a restart.
        strip_documents(&mut dock);
        // A layout saved by an older version can be structurally broken
        // (egui_dock's `retain_tabs` rebalanced in hash order and could
        // orphan a subtree); such a tree panics when split, so start over.
        if !dock.iter_surfaces().all(|s| s.node_tree().is_none_or(tree_is_consistent)) {
            log::warn!("saved workspace layout is inconsistent; using the default layout");
            return None;
        }
        if !dock.iter_all_tabs().any(|(_, t)| *t == PanelKind::Home) {
            dock.push_to_first_leaf(PanelKind::Home);
        }
        Some(Self { dock })
    }

    pub fn to_json(&self) -> Option<String> {
        let mut dock = self.dock.clone();
        strip_documents(&mut dock);
        // See `from_json` for the `null`s.
        serde_json::to_string(&dock).ok().map(|j| j.replace(":null", ":0.0"))
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
            prune_empty_surfaces(&mut self.dock);
        }
    }

    pub fn focus_document(&mut self, id: DocId) {
        if let Some(path) = self.find(&PanelKind::Document(id)) {
            self.dock.set_focused_node_and_surface(path.node_path());
            let _ = self.dock.set_active_tab(path);
        }
    }

    /// Lay every open document out at once: all in one tab strip, side by
    /// side, stacked, or in a grid (rows of two). The documents are pulled
    /// out of wherever they sit (other leaves, floating windows) into the
    /// leaf that held the first document (or Home), which is then split
    /// evenly. `active` ends up as the focused document.
    pub fn arrange_documents(&mut self, docs: &[DocId], active: Option<DocId>, mode: Arrange) {
        if docs.is_empty() {
            return;
        }
        // Gather the tabs (they are all the same kind, so nothing is lost).
        for &d in docs {
            if let Some(path) = self.find(&PanelKind::Document(d)) {
                self.dock.remove_tab(path);
            }
        }
        prune_empty_surfaces(&mut self.dock);
        // The leaf that will hold the first document: Home's leaf on the
        // main surface (documents always open next to Home), or the first
        // leaf there is.
        let anchor = self
            .dock
            .main_surface()
            .iter()
            .enumerate()
            .find(|(_, n)| n.tabs().is_some_and(|t| t.contains(&PanelKind::Home)))
            .or_else(|| self.dock.main_surface().iter().enumerate().find(|(_, n)| n.is_leaf()))
            .map(|(i, _)| NodeIndex(i));
        let Some(anchor) = anchor else {
            for &d in docs {
                self.dock.push_to_first_leaf(PanelKind::Document(d));
            }
            return;
        };
        let tree = self.dock.main_surface_mut();
        tree[anchor].append_tab(PanelKind::Document(docs[0]));
        let rest = &docs[1..];
        match mode {
            Arrange::Tabs => {
                for &d in rest {
                    tree[anchor].append_tab(PanelKind::Document(d));
                }
            }
            Arrange::SideBySide | Arrange::Stacked => {
                // `fraction` is the share the old (left / top) leaf keeps; the
                // leaf being split spans `remaining + 1` shares, so keeping one
                // leaves the rest even.
                let mut last = anchor;
                for (i, &d) in rest.iter().enumerate() {
                    let remaining = (rest.len() - i) as f32;
                    let fraction = 1.0 / (remaining + 1.0);
                    let [_, new] = if mode == Arrange::SideBySide {
                        tree.split_right(last, fraction, vec![PanelKind::Document(d)])
                    } else {
                        tree.split_below(last, fraction, vec![PanelKind::Document(d)])
                    };
                    last = new;
                }
            }
            Arrange::Grid => {
                // Rows of two: split rows below (a split moves the old leaf
                // to a new index, which the return value reports), then each
                // row in half.
                let rows = docs.len().div_ceil(2);
                let mut row_leaves = Vec::with_capacity(rows);
                let mut cur = anchor;
                for r in 1..rows {
                    let remaining = (rows - r) as f32;
                    let [old, new] =
                        tree.split_below(cur, 1.0 / (remaining + 1.0), vec![PanelKind::Document(docs[r * 2])]);
                    row_leaves.push(old);
                    cur = new;
                }
                row_leaves.push(cur);
                for (r, &leaf) in row_leaves.iter().enumerate() {
                    if let Some(&d) = docs.get(r * 2 + 1) {
                        tree.split_right(leaf, 0.5, vec![PanelKind::Document(d)]);
                    }
                }
            }
        }
        // Make each document the active tab of its leaf, then focus `active`.
        for &d in docs {
            if let Some(path) = self.find(&PanelKind::Document(d)) {
                let _ = self.dock.set_active_tab(path);
            }
        }
        if let Some(id) = active.filter(|id| docs.contains(id)).or(docs.first().copied()) {
            self.focus_document(id);
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
            PanelKind::Palette => PanelKind::Swatches,
            PanelKind::Navigator | PanelKind::Brushes | PanelKind::Info | PanelKind::Reference => PanelKind::Layers,
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

    /// Close a panel only when it lives on a floating window; a docked tab is
    /// part of the layout and stays put.
    pub fn close_if_floating(&mut self, kind: &PanelKind) {
        if let Some(path) = self.find(kind) {
            if !path.surface.is_main() {
                self.dock.remove_tab(path);
            }
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
        // A canvas that was pressed or drawn on wins over the dock's own
        // notion of focus (which only follows clicks on tabs and plain
        // clicks in a leaf, never a drag): move the focus to that
        // document's leaf so the panels and the tab highlight agree.
        if let Some(id) = state.focus_doc_request.take() {
            if state.doc(id).is_some() {
                if let Some(path) = self.find(&PanelKind::Document(id)) {
                    self.dock.set_focused_node_and_surface(path.node_path());
                    let _ = self.dock.set_active_tab(path);
                }
                state.active_doc = Some(id);
            }
        } else if let Some((_, PanelKind::Document(id))) = self.dock.find_active_focused() {
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
                // A shared canvas wears a status dot: green while others are
                // active, grey when idle, none while Leyline is not connected.
                Some(d) if d.share.as_ref().is_some_and(|s| crate::share::status_color(self.state, s).is_some()) => {
                    let p = self.state.settings.ui.palette();
                    let s = d.share.as_ref().unwrap();
                    let color = crate::share::status_color(self.state, s).unwrap();
                    let mut job = egui::text::LayoutJob::default();
                    job.append(
                        icons::CIRCLE,
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::new(
                                8.0,
                                egui::FontFamily::Name(crate::ui::theme::ICON_FONT_FILL.into()),
                            ),
                            color,
                            ..Default::default()
                        },
                    );
                    job.append(
                        &format!("{} {}", icons::IMAGE, d.doc.display_title()),
                        4.0,
                        egui::TextFormat {
                            font_id: egui::FontId::proportional(12.5),
                            color: p.text,
                            ..Default::default()
                        },
                    );
                    job.into()
                }
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
            PanelKind::Palette => panels::palette::ui(ui, self.state),
            PanelKind::Navigator => panels::navigator::ui(ui, self.state),
            PanelKind::Brushes => panels::brushes::ui(ui, self.state),
            PanelKind::BrushSettings => panels::brush_settings::ui(ui, self.state),
            PanelKind::Info => panels::info::ui(ui, self.state),
            PanelKind::Reference => panels::reference::ui(ui, self.state),
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
                // Also move the dock's focus there: clicking the tab that is
                // already active in an unfocused leaf otherwise leaves the
                // focus (and the panels) on another document.
                self.state.focus_doc_request = Some(*id);
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
            | PanelKind::History
            // Layers runs its own list scroll area too; a horizontal dock
            // scroll let the header overflow and pushed the list's scroll
            // bar off-screen.
            | PanelKind::Layers => [false, false],
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

/// Drop floating windows that hold no tabs any more. egui_dock's
/// `retain_tabs` only removes a window surface whose tree has no nodes;
/// one whose single leaf lost its tabs (a document tab floated on its own,
/// which never survives a restart) keeps the empty node, and drawing that
/// window panics inside egui_dock at the next launch.
fn prune_empty_surfaces(dock: &mut DockState<PanelKind>) {
    let populated: std::collections::HashSet<SurfaceIndex> = dock.iter_all_tabs().map(|(p, _)| p.surface).collect();
    for i in (1..dock.surfaces_count()).rev() {
        let idx = SurfaceIndex(i);
        let live = dock.get_surface(idx).is_some_and(|s| !matches!(s, egui_dock::Surface::Empty));
        if live && !populated.contains(&idx) {
            let _ = dock.remove_surface(idx);
        }
    }
}

/// Take every document tab out of the layout (documents never survive a
/// restart) one tab at a time, through `remove_tab`, which moves subtrees
/// correctly when a leaf goes; `retain_tabs` rebalances emptied leaves in
/// hash order and can leave a parent with empty children and an orphaned
/// subtree behind. Floating windows left without tabs are dropped too.
fn strip_documents(dock: &mut DockState<PanelKind>) {
    loop {
        let next = dock.iter_all_tabs().find(|(_, t)| matches!(t, PanelKind::Document(_))).map(|(p, _)| p);
        match next {
            Some(path) => {
                dock.remove_tab(path);
            }
            None => break,
        }
    }
    prune_empty_surfaces(dock);
}

/// Every non-empty node other than the root has a parent that is a split,
/// and every split has two non-empty children.
fn tree_is_consistent(tree: &egui_dock::Tree<PanelKind>) -> bool {
    let nodes: Vec<&egui_dock::Node<PanelKind>> = tree.iter().collect();
    for (i, node) in nodes.iter().enumerate() {
        let idx = NodeIndex(i);
        if node.is_empty() {
            continue;
        }
        if let Some(parent) = idx.parent() {
            if !nodes.get(parent.0).is_some_and(|p| p.is_parent()) {
                return false;
            }
        }
        if node.is_parent() {
            let ok = |c: NodeIndex| nodes.get(c.0).is_some_and(|n| !n.is_empty());
            if !ok(idx.left()) || !ok(idx.right()) {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs(ws: &Workspace) -> Vec<DocId> {
        ws.dock
            .iter_all_tabs()
            .filter_map(|(_, t)| if let PanelKind::Document(d) = t { Some(*d) } else { None })
            .collect()
    }

    #[test]
    fn arrangements_keep_every_document_and_a_consistent_tree() {
        for mode in [Arrange::Tabs, Arrange::SideBySide, Arrange::Stacked, Arrange::Grid] {
            for n in 1..=5u64 {
                let mut ws = Workspace::new();
                let ids: Vec<DocId> = (1..=n).collect();
                for &d in &ids {
                    ws.add_document(d);
                }
                ws.arrange_documents(&ids, Some(n), mode);
                let mut got = docs(&ws);
                got.sort_unstable();
                assert_eq!(got, ids, "{mode:?} with {n} documents");
                assert!(tree_is_consistent(ws.dock.main_surface()), "{mode:?} with {n} documents");
                // Arranging again from that state must work too.
                ws.arrange_documents(&ids, Some(1), Arrange::Grid);
                assert!(tree_is_consistent(ws.dock.main_surface()), "re-arrange {mode:?} with {n}");
            }
        }
    }

    #[test]
    fn stripping_documents_leaves_a_layout_that_reloads() {
        let mut ws = Workspace::new();
        let ids: Vec<DocId> = (1..=4).collect();
        for &d in &ids {
            ws.add_document(d);
        }
        ws.arrange_documents(&ids, Some(1), Arrange::Grid);
        let json = ws.to_json().unwrap();
        let reloaded = Workspace::from_json(&json).expect("layout reloads");
        assert!(docs(&reloaded).is_empty());
        assert!(tree_is_consistent(reloaded.dock.main_surface()));
        assert!(reloaded.dock.iter_all_tabs().any(|(_, t)| *t == PanelKind::Home));
        // Every panel of the default layout is still there.
        for k in [PanelKind::Tools, PanelKind::Layers, PanelKind::Color, PanelKind::Navigator] {
            assert!(reloaded.dock.find_tab(&k).is_some(), "{k:?} survived");
        }
    }
}
