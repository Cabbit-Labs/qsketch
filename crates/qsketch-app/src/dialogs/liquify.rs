//! Filter ▸ Liquify: push, twirl, pinch and bloat the picture with a brush,
//! previewed live in the working state (like the other filter dialogs), and
//! committed as one "Liquify" step on OK.

use egui::{Context, RichText};
use qsketch_core::filter::liquify::Field;
use qsketch_core::{IRect, LayerId, Pt, Raster};

use crate::state::{AppState, DocId};
use crate::tools::{CanvasEvent, CanvasInput};
use crate::ui::toasts::Level;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum LiquifyTool {
    #[default]
    Push,
    TwirlCw,
    TwirlCcw,
    Pinch,
    Bloat,
    Reconstruct,
}

impl LiquifyTool {
    pub const ALL: [LiquifyTool; 6] = [
        LiquifyTool::Push,
        LiquifyTool::TwirlCw,
        LiquifyTool::TwirlCcw,
        LiquifyTool::Pinch,
        LiquifyTool::Bloat,
        LiquifyTool::Reconstruct,
    ];
    pub fn label(self) -> &'static str {
        match self {
            LiquifyTool::Push => "Push",
            LiquifyTool::TwirlCw => "Twirl ↻",
            LiquifyTool::TwirlCcw => "Twirl ↺",
            LiquifyTool::Pinch => "Pinch",
            LiquifyTool::Bloat => "Bloat",
            LiquifyTool::Reconstruct => "Reconstruct",
        }
    }
    pub fn describe(self) -> &'static str {
        match self {
            LiquifyTool::Push => "Drag to push pixels along with the pointer",
            LiquifyTool::TwirlCw => "Hold or drag to rotate clockwise",
            LiquifyTool::TwirlCcw => "Hold or drag to rotate counter-clockwise",
            LiquifyTool::Pinch => "Hold or drag to pull pixels toward the center",
            LiquifyTool::Bloat => "Hold or drag to push pixels away from the center",
            LiquifyTool::Reconstruct => "Paint to bring the original back",
        }
    }
}

pub struct LiquifyDialog {
    pub doc: DocId,
    /// Target layers with their untouched pixels.
    bases: Vec<(LayerId, Raster)>,
    field: Field,
    pub tool: LiquifyTool,
    pub size: f32,
    pub strength: f32,
    /// History entry the preview sits on; a commit underneath ends the session.
    base: u64,
    last: Option<Pt>,
    /// The field differs from the picture on screen.
    stale: Option<IRect>,
}

pub fn open(state: &mut AppState) {
    state.settle();
    if state.dialogs.liquify.is_some() {
        return;
    }
    let Some(entry) = state.active() else { return };
    let ids = entry.target_layer_ids();
    let s = entry.doc.state();
    let bases: Vec<(LayerId, Raster)> = ids
        .iter()
        .filter_map(|id| s.layer_by_id(*id).filter(|l| !l.is_group()).map(|l| (*id, l.raster.clone())))
        .collect();
    if bases.is_empty() {
        state.toasts.push(Level::Info, "Liquify needs a raster layer.");
        return;
    }
    let size = state.tool_opts.liquify_size;
    let strength = state.tool_opts.liquify_strength;
    state.dialogs.liquify = Some(LiquifyDialog {
        doc: entry.id,
        bases,
        field: Field::new(s.width, s.height),
        tool: state.tool_opts.liquify_tool,
        size,
        strength,
        base: entry.doc.history.current_id(),
        last: None,
        stale: None,
    });
}

/// Canvas input while the dialog is open: brush edits to the field.
pub fn handle(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    let Some(d) = state.dialogs.liquify.as_mut() else { return };
    if d.doc != doc_id {
        return;
    }
    let sel = state.docs.iter().find(|e| e.id == doc_id).and_then(|e| e.doc.state().selection.clone());
    let radius = d.size / 2.0;
    let apply = |d: &mut LiquifyDialog, inp: &CanvasInput, delta: Pt| {
        let s = d.strength * inp.pressure.max(0.2);
        let m = sel.as_deref();
        let rect = match d.tool {
            LiquifyTool::Push => d.field.push(inp.doc, delta, radius, s, m),
            LiquifyTool::TwirlCw => d.field.twirl(inp.doc, radius, 0.06 * s, m),
            LiquifyTool::TwirlCcw => d.field.twirl(inp.doc, radius, -0.06 * s, m),
            LiquifyTool::Pinch => d.field.pinch(inp.doc, radius, 0.04 * s, m),
            LiquifyTool::Bloat => d.field.pinch(inp.doc, radius, -0.04 * s, m),
            LiquifyTool::Reconstruct => d.field.reconstruct(inp.doc, radius, 0.25 * s, m),
        };
        d.stale = Some(match d.stale {
            Some(r) => r.union(&rect),
            None => rect,
        });
    };
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            d.last = Some(inp.doc);
            if d.tool != LiquifyTool::Push {
                apply(d, &inp, Pt::new(0.0, 0.0));
            }
            // Capture the pointer so the drag keeps coming to us.
            if state.session.is_none() {
                state.session = Some(crate::tools::ToolSession::Liquify);
                state.session_doc = Some(doc_id);
            }
        }
        CanvasEvent::Drag(inp) => {
            let Some(last) = d.last else { return };
            let delta = Pt::new(inp.doc.x - last.x, inp.doc.y - last.y);
            d.last = Some(inp.doc);
            if d.tool == LiquifyTool::Push {
                if delta.x == 0.0 && delta.y == 0.0 {
                    return;
                }
                // Walk the drag in brush-sized steps so a fast move still warps
                // the whole path.
                let dist = delta.x.hypot(delta.y);
                let steps = (dist / (radius * 0.5).max(1.0)).ceil().max(1.0) as usize;
                for k in 1..=steps {
                    let t = k as f32 / steps as f32;
                    let p = Pt::new(last.x + delta.x * t, last.y + delta.y * t);
                    let step = Pt::new(delta.x / steps as f32, delta.y / steps as f32);
                    apply(d, &CanvasInput { doc: p, ..inp }, step);
                }
            } else {
                apply(d, &inp, delta);
            }
        }
        CanvasEvent::Release(_) | CanvasEvent::Cancel => {
            d.last = None;
            if matches!(state.session, Some(crate::tools::ToolSession::Liquify)) {
                state.session = None;
                state.session_doc = None;
            }
        }
        _ => {}
    }
    render(state, doc_id);
}

/// Re-render the stale part of the preview into the working layers.
fn render(state: &mut AppState, doc_id: DocId) {
    let Some(d) = state.dialogs.liquify.as_mut() else { return };
    let Some(rect) = d.stale.take() else { return };
    // Output pixels sample up to the largest offset away, so widen the box.
    let reach = d.field.max_offset(rect).ceil() as i32 + 2;
    let rect = rect.expand(reach);
    let Some(entry) = state.docs.iter_mut().find(|e| e.id == doc_id) else { return };
    let s = entry.doc.state_mut();
    for (id, base) in &d.bases {
        if let Some(li) = s.index_of(*id) {
            d.field.apply_rect(base, &mut s.layers[li].raster, rect);
        }
    }
    entry.doc.mark_dirty_rect(rect);
}

fn render_all(state: &mut AppState) {
    if let Some(d) = state.dialogs.liquify.as_mut() {
        let (w, h) = d.bases.first().map(|(_, r)| (r.width(), r.height())).unwrap_or((0, 0));
        d.stale = Some(IRect::new(0, 0, w as i32, h as i32));
        let doc = d.doc;
        render(state, doc);
    }
}

/// Take the preview off the working state (used by `settle` and Cancel).
pub fn revert(state: &mut AppState) {
    if let Some(d) = state.dialogs.liquify.take() {
        if let Some(e) = state.docs.iter_mut().find(|x| x.id == d.doc) {
            e.doc.revert_working();
        }
    }
}

/// The brush circle at the pointer while the dialog is open.
pub fn draw_overlay(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    let Some(d) = state.dialogs.liquify.as_ref() else { return };
    if d.doc != doc_id {
        return;
    }
    let Some(entry) = state.doc(doc_id) else { return };
    let Some(pos) = state.hover_screen_pos else { return };
    let r = d.size / 2.0 * entry.view.zoom;
    painter.circle_stroke(pos, r, egui::Stroke::new(2.0, egui::Color32::from_black_alpha(140)));
    painter.circle_stroke(pos, r, egui::Stroke::new(1.0, egui::Color32::from_white_alpha(220)));
}

pub fn show(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.liquify.as_mut() else { return };
    // Something else committed underneath (a layer change, undo): the
    // preview's base is gone, so the session ends.
    let now_base = state.docs.iter().find(|e| e.id == d.doc).map(|e| e.doc.history.current_id());
    if now_base != Some(d.base) {
        state.dialogs.liquify = None;
        return;
    }
    let p = state.settings.ui.palette();
    let (mut ok, mut cancel, mut reset, mut open) = (false, false, false, true);
    let screen = ctx.content_rect();
    egui::Window::new("Liquify")
        .id(egui::Id::new("liquify_dialog"))
        .open(&mut open)
        .collapsible(false)
        .auto_sized()
        .default_pos(egui::pos2(screen.right() - 372.0, screen.top() + 80.0))
        .show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(
                RichText::new("Drag on the canvas to warp the picture. Nothing is written until OK.").weak().small(),
            );
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for t in LiquifyTool::ALL {
                    if ui.selectable_label(d.tool == t, t.label()).on_hover_text(t.describe()).clicked() {
                        d.tool = t;
                    }
                }
            });
            ui.add_space(4.0);
            ui.add(egui::Slider::new(&mut d.size, 5.0..=600.0).text("Brush size").suffix(" px").logarithmic(true));
            ui.add(egui::Slider::new(&mut d.strength, 0.05..=1.0).text("Strength").fixed_decimals(2));
            ui.label(RichText::new(d.tool.describe()).weak().small());
            ui.add_space(8.0);
            let scope = match d.bases.len() {
                1 => "Applies to the active layer (within the selection).".to_string(),
                n => format!("Applies to {n} selected layers (within the selection)."),
            };
            ui.label(RichText::new(scope).weak().small());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if crate::ui::widgets::small_button(ui, "Reset")
                    .on_hover_text("Undo every warp in this session")
                    .clicked()
                {
                    reset = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new("OK").fill(p.accent)).clicked() {
                        ok = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        });
    let (tool, size, strength) = (d.tool, d.size, d.strength);
    state.tool_opts.liquify_tool = tool;
    state.tool_opts.liquify_size = size;
    state.tool_opts.liquify_strength = strength;
    if reset {
        if let Some(d) = state.dialogs.liquify.as_mut() {
            d.field.reset();
        }
        render_all(state);
    }
    if ok {
        let Some(d) = state.dialogs.liquify.take() else { return };
        if let Some(e) = state.docs.iter_mut().find(|x| x.id == d.doc) {
            if d.field.is_identity() {
                e.doc.revert_working();
            } else {
                e.doc.commit("Liquify");
            }
        }
    } else if cancel || !open {
        revert(state);
    }
}
