//! Tools: kinds, per-tool options, in-progress sessions and event dispatch.

mod contour;
mod fill;
pub mod floating;
mod paint;
mod select;
pub mod symmetry;
pub mod text;
mod transform;
mod view;

use std::sync::Arc;

use egui::{Modifiers, PointerButton, Pos2};
use qsketch_core::mask::SelectionOp;
use qsketch_core::ops::{Floating, GradientKind};
use qsketch_core::{IRect, Mask, Pt, Raster, StrokeEngine, TextStyle};
use serde::{Deserialize, Serialize};

use crate::state::{AppState, DocId};
use crate::ui::icons;

pub use fill::sample_color as sample_color_public;
pub use select::{deselect, invert as invert_selection, select_all, select_layer_content};
pub use transform::{commit_crop, nudge};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ToolKind {
    Move,
    RectSelect,
    EllipseSelect,
    Lasso,
    MagicWand,
    Crop,
    Eyedropper,
    Brush,
    Pencil,
    Eraser,
    Fill,
    Gradient,
    Line,
    Rect,
    Ellipse,
    Contour,
    Text,
    Zoom,
    Hand,
    RotateView,
}

impl ToolKind {
    pub const ALL: [ToolKind; 20] = [
        ToolKind::Move,
        ToolKind::RectSelect,
        ToolKind::EllipseSelect,
        ToolKind::Lasso,
        ToolKind::MagicWand,
        ToolKind::Crop,
        ToolKind::Eyedropper,
        ToolKind::Brush,
        ToolKind::Pencil,
        ToolKind::Eraser,
        ToolKind::Fill,
        ToolKind::Gradient,
        ToolKind::Line,
        ToolKind::Rect,
        ToolKind::Ellipse,
        ToolKind::Contour,
        ToolKind::Text,
        ToolKind::Zoom,
        ToolKind::Hand,
        ToolKind::RotateView,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ToolKind::Move => "Move",
            ToolKind::RectSelect => "Rectangular Marquee",
            ToolKind::EllipseSelect => "Elliptical Marquee",
            ToolKind::Lasso => "Lasso",
            ToolKind::MagicWand => "Magic Wand",
            ToolKind::Crop => "Crop",
            ToolKind::Eyedropper => "Eyedropper",
            ToolKind::Brush => "Brush",
            ToolKind::Pencil => "Pencil",
            ToolKind::Eraser => "Eraser",
            ToolKind::Fill => "Paint Bucket",
            ToolKind::Gradient => "Gradient",
            ToolKind::Line => "Line",
            ToolKind::Rect => "Rectangle",
            ToolKind::Ellipse => "Ellipse",
            ToolKind::Contour => "Contour",
            ToolKind::Text => "Text",
            ToolKind::Zoom => "Zoom",
            ToolKind::Hand => "Hand",
            ToolKind::RotateView => "Rotate View",
        }
    }

    /// Glyph under the active icon set and user overrides.
    pub fn icon(self) -> &'static str {
        crate::ui::iconset::tool_glyph(self)
    }

    /// Built-in glyph of the default (Outline) set.
    pub fn default_icon(self) -> &'static str {
        match self {
            ToolKind::Move => icons::ARROWS_OUT_CARDINAL,
            ToolKind::RectSelect => icons::SELECTION,
            ToolKind::EllipseSelect => icons::CIRCLE_DASHED,
            ToolKind::Lasso => icons::LASSO,
            ToolKind::MagicWand => icons::MAGIC_WAND,
            ToolKind::Crop => icons::CROP,
            ToolKind::Eyedropper => icons::EYEDROPPER,
            ToolKind::Brush => icons::PAINT_BRUSH,
            ToolKind::Pencil => icons::PENCIL_SIMPLE,
            ToolKind::Eraser => icons::ERASER,
            ToolKind::Fill => icons::PAINT_BUCKET,
            ToolKind::Gradient => icons::GRADIENT,
            ToolKind::Line => icons::LINE_SEGMENT,
            ToolKind::Rect => icons::RECTANGLE,
            ToolKind::Ellipse => icons::CIRCLE,
            ToolKind::Contour => icons::SCRIBBLE_LOOP,
            ToolKind::Text => icons::TEXT_T,
            ToolKind::Zoom => icons::MAGNIFYING_GLASS,
            ToolKind::Hand => icons::HAND,
            ToolKind::RotateView => icons::ARROWS_CLOCKWISE,
        }
    }

    /// Toolbar grouping (separators between groups), Photoshop order.
    pub fn group(self) -> u8 {
        match self {
            ToolKind::Move => 0,
            ToolKind::RectSelect | ToolKind::EllipseSelect | ToolKind::Lasso | ToolKind::MagicWand => 1,
            ToolKind::Crop | ToolKind::Eyedropper => 2,
            ToolKind::Brush | ToolKind::Pencil | ToolKind::Eraser => 3,
            ToolKind::Fill | ToolKind::Gradient => 4,
            ToolKind::Line | ToolKind::Rect | ToolKind::Ellipse | ToolKind::Contour => 5,
            ToolKind::Text => 6,
            ToolKind::Zoom | ToolKind::Hand | ToolKind::RotateView => 7,
        }
    }

    pub fn is_paint(self) -> bool {
        matches!(self, ToolKind::Brush | ToolKind::Pencil | ToolKind::Eraser)
    }

    pub fn uses_brush(self) -> bool {
        matches!(
            self,
            ToolKind::Brush | ToolKind::Pencil | ToolKind::Eraser | ToolKind::Line | ToolKind::Rect | ToolKind::Ellipse
        )
    }

    pub fn cursor(self) -> egui::CursorIcon {
        match self {
            ToolKind::Move => egui::CursorIcon::Move,
            ToolKind::Hand => egui::CursorIcon::Grab,
            ToolKind::Zoom => egui::CursorIcon::ZoomIn,
            ToolKind::RotateView => egui::CursorIcon::Alias,
            ToolKind::Brush | ToolKind::Pencil | ToolKind::Eraser => egui::CursorIcon::None,
            ToolKind::Text => egui::CursorIcon::Text,
            _ => egui::CursorIcon::Crosshair,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolOptions {
    pub selection_op: SelectionOp,
    pub wand_tolerance: u8,
    pub wand_contiguous: bool,
    pub wand_sample_merged: bool,
    pub fill_tolerance: u8,
    pub fill_contiguous: bool,
    pub fill_sample_merged: bool,
    pub gradient_kind: GradientKind,
    pub shape_filled: bool,
    pub eyedropper_sample_merged: bool,
    pub zoom_scrub: bool,
    /// Contour: stroke the edge with the current brush after filling.
    pub contour_outline: bool,
    /// Text tool style (size, alignment, spacing, anti-aliasing).
    pub text: TextStyle,
    pub text_family: String,
    pub text_bold: bool,
    pub text_italic: bool,
    #[serde(skip)]
    pub crop_rect: Option<IRect>,
}

impl Default for ToolOptions {
    fn default() -> Self {
        Self {
            selection_op: SelectionOp::Replace,
            wand_tolerance: 32,
            wand_contiguous: true,
            wand_sample_merged: false,
            fill_tolerance: 32,
            fill_contiguous: true,
            fill_sample_merged: false,
            gradient_kind: GradientKind::Linear,
            shape_filled: false,
            eyedropper_sample_merged: true,
            zoom_scrub: true,
            contour_outline: false,
            text: TextStyle::default(),
            text_family: crate::fonts::DEFAULT_FAMILY.to_string(),
            text_bold: false,
            text_italic: false,
            crop_rect: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CanvasInput {
    pub doc: Pt,
    pub screen: Pos2,
    /// Curved pressure 0..=1.
    pub pressure: f32,
    pub mods: Modifiers,
    pub button: PointerButton,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub enum CanvasEvent {
    Press(CanvasInput),
    Drag(CanvasInput),
    Release(CanvasInput),
    Hover(CanvasInput),
    DoubleClick(CanvasInput),
    Cancel,
}

pub enum ToolSession {
    Stroke {
        engine: Box<StrokeEngine>,
        layer: usize,
        extra: paint::StrokeExtra,
    },
    DragRect {
        start: Pt,
        cur: Pt,
        mods: Modifiers,
    },
    Lasso {
        pts: Vec<Pt>,
        mods: Modifiers,
    },
    Moving {
        base: Raster,
        floating: Floating,
        start: Pt,
        cur: Pt,
        layer: usize,
        sel_before: Option<Arc<Mask>>,
        last_rect: IRect,
    },
    Pan {
        last: Pos2,
    },
    ZoomDrag {
        start: Pos2,
        start_zoom: f32,
        moved: bool,
    },
    RotateDrag {
        start_angle: f32,
        start_rot: f32,
    },
    Shape {
        start: Pt,
        cur: Pt,
        mods: Modifiers,
    },
    GradientDrag {
        start: Pt,
        cur: Pt,
    },
    CropDrag {
        start: Pt,
        cur: Pt,
    },
    /// Freehand contour path in document space.
    Contour {
        pts: Vec<Pt>,
    },
    Picking,
}

/// Route a canvas event to the active tool.
pub fn handle(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    if let CanvasEvent::Cancel = ev {
        state.cancel_session();
        return;
    }
    // A session belongs to one document; ignore events from others.
    if let Some(sd) = state.session_doc {
        if sd != doc_id && state.session.is_some() {
            return;
        }
    }
    let tool = state.effective_tool();
    // Symmetry "set center": the next canvas press places the axes.
    if state.symmetry_pick_center {
        match ev {
            CanvasEvent::Press(inp) if inp.button == PointerButton::Primary => {
                state.symmetry.center = Some(inp.doc);
                state.symmetry_pick_center = false;
                return;
            }
            CanvasEvent::Press(_) => {
                state.symmetry_pick_center = false;
            }
            _ => return,
        }
    }
    // A floating paste captures the pointer unless a view tool is (temporarily) active.
    if state.floating.is_some()
        && !matches!(tool, ToolKind::Hand | ToolKind::Zoom | ToolKind::RotateView)
        && floating::handle(state, doc_id, ev)
    {
        return;
    }
    match tool {
        ToolKind::Brush | ToolKind::Pencil | ToolKind::Eraser => paint::handle_stroke(state, doc_id, tool, ev),
        ToolKind::Line | ToolKind::Rect | ToolKind::Ellipse => paint::handle_shape(state, doc_id, tool, ev),
        ToolKind::Contour => contour::handle(state, doc_id, ev),
        ToolKind::Text => text::handle(state, doc_id, ev),
        ToolKind::RectSelect | ToolKind::EllipseSelect => select::handle_marquee(state, doc_id, tool, ev),
        ToolKind::Lasso => select::handle_lasso(state, doc_id, ev),
        ToolKind::MagicWand => select::handle_wand(state, doc_id, ev),
        ToolKind::Move => transform::handle_move(state, doc_id, ev),
        ToolKind::Crop => transform::handle_crop(state, doc_id, ev),
        ToolKind::Eyedropper => fill::handle_eyedropper(state, doc_id, ev),
        ToolKind::Fill => fill::handle_bucket(state, doc_id, ev),
        ToolKind::Gradient => fill::handle_gradient(state, doc_id, ev),
        ToolKind::Zoom => view::handle_zoom(state, doc_id, ev),
        ToolKind::Hand => view::handle_hand(state, doc_id, ev),
        ToolKind::RotateView => view::handle_rotate(state, doc_id, ev),
    }
}

/// Draw tool-specific overlays (previews, crop shading) in screen space.
pub fn draw_overlay(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    let Some(entry) = state.doc(doc_id) else { return };
    let view = &entry.view;
    let accent = egui::Color32::from_rgb(23, 227, 180);
    let stroke = egui::Stroke::new(1.0, accent);
    let shadow = egui::Stroke::new(3.0, egui::Color32::from_black_alpha(120));
    let to_s = |p: Pt| view.doc_to_screen(p);

    if state.session_doc == Some(doc_id) {
        match &state.session {
            Some(ToolSession::DragRect { start, cur, .. }) | Some(ToolSession::CropDrag { start, cur }) => {
                let r = rect_from_drag(*start, *cur, false);
                draw_doc_rect(painter, view, r, shadow, stroke);
            }
            Some(ToolSession::Shape { start, cur, mods }) => {
                let tool = state.effective_tool();
                match tool {
                    ToolKind::Line => {
                        let end = paint::constrain_line(*start, *cur, mods.shift);
                        painter.line_segment([to_s(*start), to_s(end)], shadow);
                        painter.line_segment([to_s(*start), to_s(end)], stroke);
                    }
                    ToolKind::Rect => {
                        let r = rect_from_drag(*start, *cur, mods.shift);
                        draw_doc_rect(painter, view, r, shadow, stroke);
                    }
                    _ => {
                        let r = rect_from_drag(*start, *cur, mods.shift);
                        let pts: Vec<Pos2> = paint::ellipse_points(r, 64).into_iter().map(to_s).collect();
                        painter.add(egui::Shape::closed_line(pts.clone(), shadow));
                        painter.add(egui::Shape::closed_line(pts, stroke));
                    }
                }
            }
            Some(ToolSession::Lasso { pts, .. }) => {
                let sp: Vec<Pos2> = pts.iter().map(|p| to_s(*p)).collect();
                if sp.len() > 1 {
                    painter.add(egui::Shape::line(sp.clone(), shadow));
                    painter.add(egui::Shape::line(sp, stroke));
                }
            }
            Some(ToolSession::GradientDrag { start, cur }) => {
                painter.line_segment([to_s(*start), to_s(*cur)], shadow);
                painter.line_segment([to_s(*start), to_s(*cur)], stroke);
                painter.circle_filled(to_s(*start), 4.0, accent);
                painter.circle_filled(to_s(*cur), 4.0, accent);
            }
            _ => {}
        }
    }

    draw_symmetry_guides(state, doc_id, painter);
    contour::draw_overlay(state, doc_id, painter);
    floating::draw_overlay(state, doc_id, painter);
    text::draw_overlay(state, doc_id, painter);

    // Crop overlay: darken outside the pending crop rect.
    if state.tool == ToolKind::Crop {
        if let Some(r) = state.tool_opts.crop_rect {
            let vp = view.viewport;
            let inner = egui::Rect::from_points(&[
                to_s(Pt::new(r.x as f32, r.y as f32)),
                to_s(Pt::new(r.right() as f32, r.y as f32)),
                to_s(Pt::new(r.x as f32, r.bottom() as f32)),
                to_s(Pt::new(r.right() as f32, r.bottom() as f32)),
            ]);
            let dim = egui::Color32::from_black_alpha(140);
            // Four rects around `inner` within viewport.
            let top = egui::Rect::from_min_max(vp.min, egui::pos2(vp.max.x, inner.min.y.max(vp.min.y)));
            let bottom = egui::Rect::from_min_max(egui::pos2(vp.min.x, inner.max.y.min(vp.max.y)), vp.max);
            let left =
                egui::Rect::from_min_max(egui::pos2(vp.min.x, inner.min.y), egui::pos2(inner.min.x, inner.max.y));
            let right =
                egui::Rect::from_min_max(egui::pos2(inner.max.x, inner.min.y), egui::pos2(vp.max.x, inner.max.y));
            for rr in [top, bottom, left, right] {
                let rr = rr.intersect(vp);
                if rr.is_positive() {
                    painter.rect_filled(rr, 0, dim);
                }
            }
            draw_doc_rect(painter, view, r, shadow, egui::Stroke::new(1.0, egui::Color32::WHITE));
            // rule of thirds
            let thin = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(60));
            for i in 1..3 {
                let fx = r.x as f32 + r.w as f32 * i as f32 / 3.0;
                let fy = r.y as f32 + r.h as f32 * i as f32 / 3.0;
                painter.line_segment([to_s(Pt::new(fx, r.y as f32)), to_s(Pt::new(fx, r.bottom() as f32))], thin);
                painter.line_segment([to_s(Pt::new(r.x as f32, fy)), to_s(Pt::new(r.right() as f32, fy))], thin);
            }
        }
    }
}

/// Symmetry axes through the center while a painting tool is active.
fn draw_symmetry_guides(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    let sym = &state.symmetry;
    let picking = state.symmetry_pick_center;
    if !(picking || (sym.active() && sym.show_guides)) {
        return;
    }
    let tool = state.tool;
    if !(tool.uses_brush() || tool == ToolKind::Contour || picking) {
        return;
    }
    let Some(entry) = state.doc(doc_id) else { return };
    let view = &entry.view;
    let (w, h) = (entry.doc.width() as f32, entry.doc.height() as f32);
    let c = sym.center_for(entry.doc.width(), entry.doc.height());
    let reach = w + h;
    let col = egui::Color32::from_rgba_unmultiplied(120, 200, 255, 180);
    let stroke = egui::Stroke::new(1.0, col);
    let shadow = egui::Stroke::new(3.0, egui::Color32::from_black_alpha(90));
    for (dx, dy) in sym.guide_directions() {
        let a = view.doc_to_screen(Pt::new(c.x - dx * reach, c.y - dy * reach));
        let b = view.doc_to_screen(Pt::new(c.x + dx * reach, c.y + dy * reach));
        painter.line_segment([a, b], shadow);
        painter.line_segment([a, b], stroke);
    }
    let sc = view.doc_to_screen(c);
    painter.circle_stroke(sc, 5.0, egui::Stroke::new(2.0, egui::Color32::from_black_alpha(120)));
    painter.circle_stroke(sc, 5.0, stroke);
    if picking {
        painter.text(
            sc + egui::vec2(10.0, -10.0),
            egui::Align2::LEFT_BOTTOM,
            "Click to place the symmetry center",
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
    }
}

fn draw_doc_rect(
    painter: &egui::Painter,
    view: &crate::canvas::view::CanvasView,
    r: IRect,
    shadow: egui::Stroke,
    stroke: egui::Stroke,
) {
    let pts = vec![
        view.doc_to_screen(Pt::new(r.x as f32, r.y as f32)),
        view.doc_to_screen(Pt::new(r.right() as f32, r.y as f32)),
        view.doc_to_screen(Pt::new(r.right() as f32, r.bottom() as f32)),
        view.doc_to_screen(Pt::new(r.x as f32, r.bottom() as f32)),
    ];
    painter.add(egui::Shape::closed_line(pts.clone(), shadow));
    painter.add(egui::Shape::closed_line(pts, stroke));
}

/// Integer rect from a drag in document space (pixel-snapped). `square`
/// constrains to equal sides.
pub fn rect_from_drag(start: Pt, cur: Pt, square: bool) -> IRect {
    let mut dx = cur.x - start.x;
    let mut dy = cur.y - start.y;
    if square {
        let m = dx.abs().max(dy.abs());
        dx = m * dx.signum();
        dy = m * dy.signum();
    }
    let x0 = start.x.floor() as i32;
    let y0 = start.y.floor() as i32;
    let x1 = (start.x + dx).floor() as i32;
    let y1 = (start.y + dy).floor() as i32;
    IRect::from_corners(x0, y0, x1, y1)
}

/// Selection op from modifiers (Shift = add, Alt = subtract, both = intersect).
pub fn op_from_mods(base: SelectionOp, mods: Modifiers) -> SelectionOp {
    match (mods.shift, mods.alt) {
        (true, true) => SelectionOp::Intersect,
        (true, false) => SelectionOp::Add,
        (false, true) => SelectionOp::Subtract,
        _ => base,
    }
}

/// Apply a new selection mask to the document with the op, then commit.
pub fn apply_selection(state: &mut AppState, doc_id: DocId, new: Mask, op: SelectionOp, label: &str) {
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let s = entry.doc.state_mut();
    let combined = match (&s.selection, op) {
        (Some(old), op) if op != SelectionOp::Replace => old.combine(&new, op),
        _ => new,
    };
    s.selection = if combined.is_empty() { None } else { Some(Arc::new(combined)) };
    entry.doc.commit(label);
    entry.sel_outline = None;
}
