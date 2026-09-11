//! Text tool: click to anchor a text box, type in the floating editor, and the
//! canvas shows the rasterized result live (rendered into the working layer,
//! like a floating paste). Enter commits one "Text" history step, Esc discards,
//! dragging the preview moves the anchor. Same shape as Leyline's sketchpad
//! text tool, with fonts, styles and alignment on top.

use ab_glyph::FontArc;
use egui::{Color32, Pos2, Stroke};
use qsketch_core::ops::{drop_floating, Floating};
use qsketch_core::text::{render, TextStyle};
use qsketch_core::{IRect, Pt, Raster, Rgba8};

use super::CanvasEvent;
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

/// An in-progress text placement.
pub struct TextEdit {
    pub doc: DocId,
    pub layer: usize,
    /// Top-left of the first line (alignment pivots on its x), document pixels.
    pub anchor: Pt,
    pub text: String,
    /// The layer before the preview was drawn.
    base: Raster,
    last_dirty: IRect,
    /// Placement of the last rendered preview, if any glyphs were drawn.
    pub bounds: Option<IRect>,
    /// What the current preview was rendered from; re-render when it changes.
    rendered: Option<RenderKey>,
    /// The floating editor should grab keyboard focus this frame.
    pub focus: bool,
    pub drag: Option<(Pt, Pt)>,
}

#[derive(Clone, PartialEq)]
struct RenderKey {
    text: String,
    style: TextStyle,
    family: String,
    bold: bool,
    italic: bool,
    color: Rgba8,
    anchor: (i32, i32),
}

impl TextEdit {
    pub fn anchor_i(&self) -> (i32, i32) {
        (self.anchor.x.round() as i32, self.anchor.y.round() as i32)
    }

    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// The style actually rendered: the options' style, with faux bold/italic
/// filled in when the family has no real face for the requested style.
pub fn effective_style(state: &AppState) -> TextStyle {
    let o = &state.tool_opts;
    let mut s = o.text.clone();
    s.clamp();
    if o.text_bold && !state.fonts.has_style(&o.text_family, true, o.text_italic) {
        s.faux_bold = (s.size * 0.045).clamp(1.0, 20.0);
    }
    if o.text_italic && !state.fonts.has_style(&o.text_family, o.text_bold, true) {
        s.faux_italic = 0.22;
    }
    s
}

fn current_font(state: &mut AppState) -> Option<FontArc> {
    let (fam, b, i) = (state.tool_opts.text_family.clone(), state.tool_opts.text_bold, state.tool_opts.text_italic);
    state.fonts.font(&fam, b, i)
}

/// Start editing at `anchor` on the active layer. Any previous edit is committed first.
pub fn begin(state: &mut AppState, doc_id: DocId, anchor: Pt) -> bool {
    commit(state);
    state.cancel_session();
    super::floating::commit(state);
    state.fonts.ensure_loaded();
    if !state.fonts.has_family(&state.tool_opts.text_family) {
        state.tool_opts.text_family = crate::fonts::DEFAULT_FAMILY.to_string();
    }
    let Some(entry) = state.doc_mut(doc_id) else { return false };
    let s = entry.doc.state();
    let li = s.active;
    if !s.layer_editable(li) {
        state.toasts.push(Level::Info, "The active layer is locked or hidden.");
        return false;
    }
    let base = s.layers[li].raster.clone();
    state.text_edit = Some(TextEdit {
        doc: doc_id,
        layer: li,
        anchor: Pt::new(anchor.x.round(), anchor.y.round()),
        text: String::new(),
        base,
        last_dirty: IRect::EMPTY,
        bounds: None,
        rendered: None,
        focus: true,
        drag: None,
    });
    true
}

/// Re-render the preview into the working layer if anything changed. Call once per frame.
pub fn refresh(state: &mut AppState) {
    let Some(te) = state.text_edit.as_ref() else { return };
    let key = RenderKey {
        text: te.text.clone(),
        style: effective_style(state),
        family: state.tool_opts.text_family.clone(),
        bold: state.tool_opts.text_bold,
        italic: state.tool_opts.text_italic,
        color: state.fg,
        anchor: te.anchor_i(),
    };
    if te.rendered.as_ref() == Some(&key) {
        return;
    }
    let font = current_font(state);
    let Some(mut te) = state.text_edit.take() else { return };
    let image = font.and_then(|f| render(&f, &key.text, &key.style, key.color));
    if let Some(entry) = state.doc_mut(te.doc) {
        let (ax, ay) = key.anchor;
        let s = entry.doc.state_mut();
        let mut dirty = te.last_dirty;
        match image {
            Some(img) => {
                let r = img.rect_at(ax, ay);
                let f = Floating { raster: img.raster, origin: (r.x, r.y), mask: None };
                if te.layer < s.layers.len() {
                    s.layers[te.layer].raster = drop_floating(&te.base, &f, 0, 0);
                }
                dirty = dirty.union(&r);
                te.bounds = Some(r);
                te.last_dirty = r;
            }
            None => {
                if te.layer < s.layers.len() {
                    s.layers[te.layer].raster = te.base.clone();
                }
                te.bounds = None;
                te.last_dirty = IRect::EMPTY;
            }
        }
        let doc_rect = entry.doc.state().rect();
        let dirty = dirty.intersect(&doc_rect);
        if !dirty.is_empty() {
            entry.doc.mark_dirty_rect(dirty);
        }
    }
    te.rendered = Some(key);
    state.text_edit = Some(te);
}

/// Commit the text as a history step. Blank text is discarded. Returns true if an edit was open.
pub fn commit(state: &mut AppState) -> bool {
    if state.text_edit.is_none() {
        return false;
    }
    refresh(state);
    let Some(te) = state.text_edit.take() else { return false };
    if let Some(entry) = state.doc_mut(te.doc) {
        if te.bounds.is_some() && !te.is_blank() {
            entry.doc.commit("Text");
        } else {
            entry.doc.revert_working();
        }
        entry.sel_outline = None;
    }
    true
}

/// Discard the edit, restoring the layer.
pub fn cancel(state: &mut AppState) -> bool {
    let Some(te) = state.text_edit.take() else { return false };
    if let Some(entry) = state.doc_mut(te.doc) {
        entry.doc.revert_working();
        entry.sel_outline = None;
    }
    true
}

/// Pointer handling for the Text tool.
pub fn handle(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) => {
            if inp.button != egui::PointerButton::Primary {
                return;
            }
            let inside = state.text_edit.as_ref().is_some_and(|te| {
                te.doc == doc_id
                    && te
                        .bounds
                        .is_some_and(|b| b.expand(4).contains(inp.doc.x.floor() as i32, inp.doc.y.floor() as i32))
            });
            if inside {
                if let Some(te) = state.text_edit.as_mut() {
                    te.drag = Some((inp.doc, te.anchor));
                    te.focus = true;
                }
            } else {
                begin(state, doc_id, inp.doc);
            }
        }
        CanvasEvent::Drag(inp) => {
            if let Some(te) = state.text_edit.as_mut() {
                if let Some((start, anchor0)) = te.drag {
                    let mut dx = inp.doc.x - start.x;
                    let mut dy = inp.doc.y - start.y;
                    if inp.mods.shift {
                        if dx.abs() > dy.abs() {
                            dy = 0.0;
                        } else {
                            dx = 0.0;
                        }
                    }
                    te.anchor = Pt::new((anchor0.x + dx).round(), (anchor0.y + dy).round());
                }
            }
        }
        CanvasEvent::Release(_) => {
            if let Some(te) = state.text_edit.as_mut() {
                te.drag = None;
            }
        }
        CanvasEvent::DoubleClick(_) => {
            commit(state);
        }
        _ => {}
    }
}

/// Whether the pointer is being dragged to move the text.
pub fn dragging(state: &AppState, doc_id: DocId) -> bool {
    state.text_edit.as_ref().is_some_and(|te| te.doc == doc_id && te.drag.is_some())
}

/// Dashed box around the preview and an anchor marker.
pub fn draw_overlay(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    let Some(te) = state.text_edit.as_ref() else { return };
    if te.doc != doc_id {
        return;
    }
    let Some(entry) = state.doc(doc_id) else { return };
    let view = &entry.view;
    let accent = Color32::from_rgb(23, 227, 180);
    let a = view.doc_to_screen(te.anchor);
    // Anchor: a small I-beam-ish cross.
    painter.line_segment(
        [a + egui::vec2(-6.0, 0.0), a + egui::vec2(6.0, 0.0)],
        Stroke::new(3.0, Color32::from_black_alpha(120)),
    );
    painter.line_segment(
        [a + egui::vec2(0.0, -6.0), a + egui::vec2(0.0, 6.0)],
        Stroke::new(3.0, Color32::from_black_alpha(120)),
    );
    painter.line_segment([a + egui::vec2(-6.0, 0.0), a + egui::vec2(6.0, 0.0)], Stroke::new(1.0, accent));
    painter.line_segment([a + egui::vec2(0.0, -6.0), a + egui::vec2(0.0, 6.0)], Stroke::new(1.0, accent));
    if let Some(r) = te.bounds {
        let pts = [
            view.doc_to_screen(Pt::new(r.x as f32, r.y as f32)),
            view.doc_to_screen(Pt::new(r.right() as f32, r.y as f32)),
            view.doc_to_screen(Pt::new(r.right() as f32, r.bottom() as f32)),
            view.doc_to_screen(Pt::new(r.x as f32, r.bottom() as f32)),
        ];
        painter.add(egui::Shape::closed_line(pts.to_vec(), Stroke::new(3.0, Color32::from_black_alpha(120))));
        painter.add(egui::Shape::dashed_line(
            &[pts[0], pts[1], pts[2], pts[3], pts[0]],
            Stroke::new(1.0, accent),
            6.0,
            4.0,
        ));
    }
}

/// Where the floating editor box goes: under the preview (or the anchor), clamped to the viewport.
fn editor_pos(state: &AppState, doc_id: DocId) -> Option<Pos2> {
    let te = state.text_edit.as_ref()?;
    let view = &state.doc(doc_id)?.view;
    let below = match te.bounds {
        Some(r) => {
            // Lowest screen-space corner of the (possibly rotated) box.
            let corners = [
                view.doc_to_screen(Pt::new(r.x as f32, r.y as f32)),
                view.doc_to_screen(Pt::new(r.right() as f32, r.y as f32)),
                view.doc_to_screen(Pt::new(r.right() as f32, r.bottom() as f32)),
                view.doc_to_screen(Pt::new(r.x as f32, r.bottom() as f32)),
            ];
            let x = corners.iter().map(|p| p.x).fold(f32::MAX, f32::min);
            let y = corners.iter().map(|p| p.y).fold(f32::MIN, f32::max);
            Pos2::new(x, y + 10.0)
        }
        None => view.doc_to_screen(te.anchor) + egui::vec2(0.0, 12.0),
    };
    let vp = view.viewport;
    let size = egui::vec2(300.0, 80.0);
    let mut p = below;
    p.x = p.x.min(vp.right() - size.x - 4.0).max(vp.left() + 4.0);
    if p.y + size.y > vp.bottom() - 4.0 {
        // No room below: put it above the anchor instead.
        let top = te.bounds.map_or(view.doc_to_screen(te.anchor).y, |r| {
            [
                view.doc_to_screen(Pt::new(r.x as f32, r.y as f32)).y,
                view.doc_to_screen(Pt::new(r.right() as f32, r.y as f32)).y,
                view.doc_to_screen(Pt::new(r.right() as f32, r.bottom() as f32)).y,
                view.doc_to_screen(Pt::new(r.x as f32, r.bottom() as f32)).y,
            ]
            .into_iter()
            .fold(f32::MAX, f32::min)
        });
        p.y = (top - size.y - 10.0).max(vp.top() + 4.0);
    }
    Some(p)
}

/// The floating editor: a small text box pinned under the preview. Enter
/// commits, Shift+Enter breaks a line, Esc cancels.
pub fn editor_ui(ui: &mut egui::Ui, state: &mut AppState, doc_id: DocId) {
    if state.text_edit.as_ref().is_none_or(|te| te.doc != doc_id) {
        return;
    }
    let ctx = ui.ctx().clone();
    let Some(pos) = editor_pos(state, doc_id) else { return };
    // Keys first, so the TextEdit never sees the Enter we want.
    // `consume_key(NONE, ..)` matches Shift+Enter too, so filter the raw events:
    // a plain Enter commits, a Shift+Enter is left for the TextEdit to break the line.
    let enter = ctx.input_mut(|i| {
        let mut hit = false;
        i.events.retain(|e| match e {
            egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if !modifiers.shift => {
                hit = true;
                false
            }
            _ => true,
        });
        hit
    });
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if escape {
        cancel(state);
        return;
    }
    if enter {
        commit(state);
        return;
    }
    let mut apply = false;
    let mut discard = false;
    egui::Area::new(egui::Id::new(("text_editor", doc_id))).order(egui::Order::Foreground).fixed_pos(pos).show(
        &ctx,
        |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(300.0);
                let Some(te) = state.text_edit.as_mut() else { return };
                let edit = egui::TextEdit::multiline(&mut te.text)
                    .desired_width(f32::INFINITY)
                    .desired_rows(1)
                    .hint_text("Type text…")
                    .font(egui::TextStyle::Body)
                    .id(egui::Id::new(("text_editor_field", doc_id)));
                let r = ui.add(edit);
                if te.focus {
                    r.request_focus();
                    te.focus = false;
                }
                ui.horizontal(|ui| {
                    apply =
                        ui.small_button(format!("{} Apply", crate::ui::icons::CHECK)).on_hover_text("Enter").clicked();
                    discard = ui.small_button(format!("{} Cancel", crate::ui::icons::X)).on_hover_text("Esc").clicked();
                    ui.label(egui::RichText::new("Shift+Enter: new line · drag the text to move it").weak().small());
                });
            });
        },
    );
    if apply {
        commit(state);
    } else if discard {
        cancel(state);
    }
}
