//! Text tool: click to start a text layer, type in the floating editor, and
//! the layer shows the rasterized result live. Enter keeps it as an editable
//! text layer (`LayerKind::Text`): clicking it again with the Text tool, or
//! Layer › Text › Edit Text, reopens it with its text, font and style. Esc
//! discards, dragging the preview moves the anchor, and Rasterize turns the
//! layer into plain pixels.

use ab_glyph::FontArc;
use egui::{Color32, Pos2, Stroke};
use qsketch_core::ops::{drop_floating, Floating};
use qsketch_core::text::{render, TextLayer, TextStyle};
use qsketch_core::{IRect, LayerKind, Pt, Raster, Rgba8};

use super::CanvasEvent;
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

/// An in-progress edit of a text layer. The layer is redrawn live from the
/// editor's text and the tool options; Enter keeps it (as an editable text
/// layer), Esc puts the layer back the way it was.
pub struct TextEdit {
    pub doc: DocId,
    /// Index of the text layer being edited.
    pub layer: usize,
    /// Top-left of the first line (alignment pivots on its x), document pixels.
    pub anchor: Pt,
    pub text: String,
    /// The text's color: the layer's own, following the foreground color
    /// whenever that is changed during the edit.
    pub color: Rgba8,
    fg0: Rgba8,
    /// The layer was created by this edit (named after its first line).
    is_new: bool,
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

/// A name for a new text layer: its first line, like Photoshop.
fn layer_name(text: &str) -> String {
    let line: String = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Text").chars().take(24).collect();
    line
}

/// The topmost visible text layer whose pixels sit under `p`, for a click
/// that should reopen existing text instead of starting new text.
fn text_layer_at(state: &AppState, doc_id: DocId, p: Pt) -> Option<usize> {
    let e = state.doc(doc_id)?;
    let s = e.doc.state();
    let (x, y) = (p.x.floor() as i32, p.y.floor() as i32);
    let hit = |li: usize| {
        let l = &s.layers[li];
        l.is_text() && s.effectively_visible(li) && l.raster.bounds().is_some_and(|b| b.expand(2).contains(x, y))
    };
    if hit(s.active) {
        return Some(s.active);
    }
    (0..s.layers.len()).rev().find(|&li| hit(li))
}

/// Start editing: reopen the text layer under `anchor` (or the active text
/// layer when `reopen_active`), else make a new text layer above the active
/// layer. Any previous edit is committed first.
pub fn begin(state: &mut AppState, doc_id: DocId, anchor: Pt, reopen_active: bool) -> bool {
    commit(state);
    state.cancel_session();
    super::floating::commit(state);
    state.fonts.ensure_loaded();
    let existing = if reopen_active {
        state
            .doc(doc_id)
            .map(|e| e.doc.state().active)
            .filter(|&li| state.doc(doc_id).is_some_and(|e| e.doc.state().layers.get(li).is_some_and(|l| l.is_text())))
    } else {
        text_layer_at(state, doc_id, anchor)
    };
    let fg = state.fg;
    if let Some(li) = existing {
        let Some(entry) = state.doc_mut(doc_id) else { return false };
        let s = entry.doc.state_mut();
        if s.layers[li].props.locked || !s.effectively_visible(li) {
            state.toasts.push(Level::Info, "That text layer is locked or hidden.");
            return false;
        }
        s.active = li;
        let Some(t) = s.layers[li].props.text.clone() else { return false };
        entry.selected.clear();
        state.tool_opts.text = t.style.clone();
        state.tool_opts.text.faux_bold = 0.0;
        state.tool_opts.text.faux_italic = 0.0;
        state.tool_opts.text_family = t.family.clone();
        state.tool_opts.text_bold = t.bold;
        state.tool_opts.text_italic = t.italic;
        if !state.fonts.has_family(&state.tool_opts.text_family) {
            state.tool_opts.text_family = crate::fonts::DEFAULT_FAMILY.to_string();
        }
        state.text_edit = Some(TextEdit {
            doc: doc_id,
            layer: li,
            anchor: Pt::new(t.anchor.0 as f32, t.anchor.1 as f32),
            text: t.text,
            color: t.color,
            fg0: fg,
            is_new: false,
            bounds: None,
            rendered: None,
            focus: true,
            drag: None,
        });
        return true;
    }
    if !state.fonts.has_family(&state.tool_opts.text_family) {
        state.tool_opts.text_family = crate::fonts::DEFAULT_FAMILY.to_string();
    }
    let Some(entry) = state.doc_mut(doc_id) else { return false };
    let s = entry.doc.state_mut();
    let name = s.unique_layer_name("Text");
    let id = s.add_layer(name, None);
    let Some(li) = s.index_of(id) else { return false };
    s.layers[li].props.kind = LayerKind::Text;
    s.layers[li].props.text = Some(TextLayer::default());
    entry.selected.clear();
    state.text_edit = Some(TextEdit {
        doc: doc_id,
        layer: li,
        anchor: Pt::new(anchor.x.round(), anchor.y.round()),
        text: String::new(),
        color: fg,
        fg0: fg,
        is_new: true,
        bounds: None,
        rendered: None,
        focus: true,
        drag: None,
    });
    true
}

/// Re-render the layer from the edit if anything changed. Call once per frame.
pub fn refresh(state: &mut AppState) {
    let fg = state.fg;
    let style = effective_style(state);
    let mut raw_style = state.tool_opts.text.clone();
    raw_style.clamp();
    let Some(te) = state.text_edit.as_mut() else { return };
    if te.fg0 != fg {
        te.fg0 = fg;
        te.color = fg;
    }
    let key = RenderKey {
        text: te.text.clone(),
        style,
        family: state.tool_opts.text_family.clone(),
        bold: state.tool_opts.text_bold,
        italic: state.tool_opts.text_italic,
        color: te.color,
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
        let (w, h) = (s.width, s.height);
        if te.layer < s.layers.len() {
            let l = &mut s.layers[te.layer];
            let mut dirty = l.raster.bounds().unwrap_or(IRect::EMPTY);
            l.props.text = Some(TextLayer {
                text: key.text.clone(),
                style: raw_style,
                family: key.family.clone(),
                bold: key.bold,
                italic: key.italic,
                color: key.color,
                anchor: key.anchor,
            });
            if te.is_new {
                l.props.name = layer_name(&key.text);
            }
            match image {
                Some(img) => {
                    let r = img.rect_at(ax, ay);
                    let f = Floating { raster: img.raster, origin: (r.x, r.y), mask: None };
                    l.raster = drop_floating(&Raster::new(w, h), &f, 0, 0);
                    dirty = dirty.union(&r);
                    te.bounds = Some(r);
                }
                None => {
                    l.raster = Raster::new(w, h);
                    te.bounds = None;
                }
            }
            let dirty = dirty.intersect(&IRect::new(0, 0, w as i32, h as i32));
            if !dirty.is_empty() {
                entry.doc.mark_dirty_rect(dirty);
            }
        }
    }
    te.rendered = Some(key);
    state.text_edit = Some(te);
}

/// Keep the edit as a history step. Blank text is discarded (a new layer
/// vanishes, existing text goes back). Returns true if an edit was open.
pub fn commit(state: &mut AppState) -> bool {
    if state.text_edit.is_none() {
        return false;
    }
    refresh(state);
    let Some(te) = state.text_edit.take() else { return false };
    if let Some(entry) = state.doc_mut(te.doc) {
        if te.bounds.is_some() && !te.is_blank() {
            entry.doc.commit(if te.is_new { "Text" } else { "Edit Text" });
        } else {
            entry.doc.revert_working();
        }
        entry.sel_outline = None;
    }
    true
}

/// Discard the edit, restoring the layer (or removing a new one).
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
                begin(state, doc_id, inp.doc, false);
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
                    apply = crate::ui::widgets::small_button(ui, format!("{} Apply", crate::ui::icons::CHECK))
                        .on_hover_text("Enter")
                        .clicked();
                    discard = crate::ui::widgets::small_button(ui, format!("{} Cancel", crate::ui::icons::X))
                        .on_hover_text("Esc")
                        .clicked();
                    ui.label(
                        egui::RichText::new("Shift+Enter: new line · drag the text to move it · stays editable")
                            .weak()
                            .small(),
                    );
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
