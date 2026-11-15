//! Bone tool: build a skeleton and pose it right on the canvas.
//!
//! * No animation selected (Setup): drag on empty canvas to add a bone (a
//!   child of the selected bone; hold Alt for a new root). Drag a bone's tip
//!   to turn and resize it, its head or body to move it.
//! * An animation selected (Animate): the same drags write keys at the
//!   current time (the tip rotates, the head or body moves), and the canvas
//!   shows the posed layers live.
//!
//! The skeleton, its attachments and animations live in the document
//! (`qsketch_core::rig`); this file is only the pointer handling, the
//! overlay and the live preview.

use egui::{Color32, Stroke};
use qsketch_core::raster::ResizeFilter;
use qsketch_core::rig::{self, BoneDrag, BoneId, Curve};
use qsketch_core::{Pt, Rig};

use super::{CanvasEvent, ToolSession};
use crate::dialogs::rig::Easing;
use crate::state::{AppState, DocEntry, DocId};

/// Screen distance (points) within which a handle is grabbed.
const HANDLE: f32 = 9.0;
/// Screen distance within which a bone's body is grabbed.
const BODY: f32 = 6.0;
/// Shortest bone (screen points) a drag creates.
const MIN_NEW: f32 = 8.0;

/// Per-document state of the Bone tool.
#[derive(Clone)]
pub struct RigUi {
    pub sel: Option<BoneId>,
    /// Index into the rig's animations; `None` edits the setup pose.
    pub anim: Option<usize>,
    pub time_ms: u32,
    pub easing: Easing,
    /// Preview with nearest-neighbor sampling (pixel art).
    pub nearest: bool,
    /// An animation property was edited in the tool options and waits for
    /// its undo step (committed when the field is let go).
    pub edited: bool,
    /// What the live preview was last drawn for.
    pose_key: Option<(u64, Rig, usize, u32, bool)>,
}

impl Default for RigUi {
    fn default() -> Self {
        Self {
            sel: None,
            anim: None,
            time_ms: 0,
            easing: Easing::Linear,
            nearest: false,
            edited: false,
            pose_key: None,
        }
    }
}

impl RigUi {
    /// The selected animation, if it still exists.
    pub fn anim_in(&self, rig: &Rig) -> Option<usize> {
        self.anim.filter(|&a| a < rig.animations.len())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum BoneGesture {
    /// Dragging out a new bone from `head` under `parent`.
    Create { parent: Option<BoneId>, head: Pt },
    /// Dragging a handle of an existing bone.
    Handle { bone: BoneId, drag: BoneDrag },
    /// Pressed on nothing in Animate mode: just deselects.
    Idle,
}

/// What a press landed on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Tip(BoneId),
    Head(BoneId),
    Body(BoneId),
}

fn seg_dist(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_sq();
    if l2 < 1e-6 {
        return (p - a).length();
    }
    let t = (((p - a).x * ab.x + (p - a).y * ab.y) / l2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Bone head/tip positions on screen for the pose being shown.
fn screen_bones(entry: &DocEntry) -> Vec<(BoneId, egui::Pos2, egui::Pos2)> {
    let rig = &entry.doc.state().rig;
    let pose = rig.pose(entry.rig_ui.anim_in(rig).map(|a| (a, entry.rig_ui.time_ms)));
    rig.bones
        .iter()
        .filter_map(|b| {
            let (h, t) = (rig.head(b.id, &pose)?, rig.tip(b.id, &pose)?);
            Some((b.id, entry.view.doc_to_screen(h), entry.view.doc_to_screen(t)))
        })
        .collect()
}

fn hit_test(entry: &DocEntry, at: egui::Pos2) -> Option<Hit> {
    let bones = screen_bones(entry);
    let near = |p: egui::Pos2| (p - at).length() <= HANDLE;
    let sel = entry.rig_ui.sel;
    if let Some((id, h, t)) = bones.iter().find(|(id, ..)| Some(*id) == sel) {
        if near(*t) {
            return Some(Hit::Tip(*id));
        }
        if near(*h) {
            return Some(Hit::Head(*id));
        }
    }
    // Heads before tips: a child's head sits on its parent's tip.
    if let Some((id, ..)) = bones.iter().rev().find(|(_, h, _)| near(*h)) {
        return Some(Hit::Head(*id));
    }
    if let Some((id, ..)) = bones.iter().rev().find(|(_, _, t)| near(*t)) {
        return Some(Hit::Tip(*id));
    }
    bones.iter().rev().find(|(_, h, t)| seg_dist(at, *h, *t) <= BODY).map(|(id, ..)| Hit::Body(*id))
}

pub fn handle(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) {
    match ev {
        CanvasEvent::Press(inp) if inp.button == egui::PointerButton::Primary => {
            if state.session.is_some() {
                return;
            }
            let Some(entry) = state.doc_mut(doc_id) else { return };
            let animating = entry.rig_ui.anim_in(&entry.doc.state().rig).is_some();
            let gesture = match hit_test(entry, inp.screen) {
                Some(hit) => {
                    let (bone, drag) = match hit {
                        Hit::Tip(b) => (b, if animating { BoneDrag::Aim } else { BoneDrag::AimStretch }),
                        Hit::Head(b) | Hit::Body(b) => (b, BoneDrag::Move),
                    };
                    entry.rig_ui.sel = Some(bone);
                    BoneGesture::Handle { bone, drag }
                }
                None if animating => {
                    entry.rig_ui.sel = None;
                    BoneGesture::Idle
                }
                None => {
                    let parent = if inp.mods.alt { None } else { entry.rig_ui.sel };
                    BoneGesture::Create { parent, head: inp.doc }
                }
            };
            state.session = Some(ToolSession::Bone { gesture, cur: inp.doc, moved: false });
            state.session_doc = Some(doc_id);
        }
        CanvasEvent::Drag(inp) => {
            let Some(ToolSession::Bone { gesture, cur, moved }) = &mut state.session else { return };
            *cur = inp.doc;
            *moved = true;
            let gesture = *gesture;
            if let BoneGesture::Handle { bone, drag } = gesture {
                let Some(entry) = state.doc_mut(doc_id) else { return };
                let (anim, t, curve) =
                    (entry.rig_ui.anim_in(&entry.doc.state().rig), entry.rig_ui.time_ms, entry.rig_ui.easing.curve());
                entry.doc.state_mut().rig.drag_bone(anim, t, bone, drag, inp.doc, curve);
            }
        }
        CanvasEvent::Release(inp) => {
            let Some(ToolSession::Bone { gesture, moved, .. }) = state.session.take() else { return };
            state.session_doc = None;
            let Some(entry) = state.doc_mut(doc_id) else { return };
            match gesture {
                BoneGesture::Create { parent, head } => {
                    let far = entry.view.doc_to_screen(head).distance(inp.screen) >= MIN_NEW;
                    if far {
                        let rig = &mut entry.doc.state_mut().rig;
                        let pose = rig.pose(None);
                        let parent = parent.filter(|&p| rig.bone(p).is_some());
                        let parent_world = parent.and_then(|p| rig.bone_index(p)).and_then(|i| pose.get(i).copied());
                        let local = parent_world.and_then(|w| w.inverse()).map_or(head, |inv| inv.apply(head));
                        let name = unique_bone_name(rig);
                        let id = rig.add_bone(name, parent, local.x, local.y, 1.0);
                        rig.drag_bone(None, 0, id, BoneDrag::AimStretch, inp.doc, Curve::Linear);
                        entry.rig_ui.sel = Some(id);
                        entry.doc.commit("Add Bone");
                    }
                }
                BoneGesture::Handle { drag, .. } => {
                    if moved {
                        let animating = entry.rig_ui.anim_in(&entry.doc.state().rig).is_some();
                        let label = match (animating, drag) {
                            (true, BoneDrag::Move) => "Key Bone Position",
                            (true, _) => "Key Bone Rotation",
                            (false, BoneDrag::Move) => "Move Bone",
                            (false, _) => "Turn Bone",
                        };
                        entry.doc.commit(label);
                    }
                }
                BoneGesture::Idle => {}
            }
        }
        _ => {}
    }
}

fn unique_bone_name(rig: &Rig) -> String {
    (1..)
        .map(|n| format!("bone {n}"))
        .find(|name| rig.bones.iter().all(|b| &b.name != name))
        .unwrap_or_else(|| "bone".into())
}

/// Delete the selected bone (and its children); true when one was removed.
pub fn delete_selected(state: &mut AppState) -> bool {
    let Some(entry) = state.active_mut() else { return false };
    let Some(id) = entry.rig_ui.sel.take() else { return false };
    if entry.doc.state().rig.bone(id).is_none() {
        return false;
    }
    entry.doc.state_mut().rig.remove_bone(id);
    entry.doc.commit("Delete Bone");
    true
}

/// Keep the canvas showing the pose being edited: while the Bone tool has
/// an animation selected the layers are drawn posed; otherwise as they are.
pub fn sync_preview(entry: &mut DocEntry, tool_active: bool) {
    let anim = tool_active.then(|| entry.rig_ui.anim_in(&entry.doc.state().rig)).flatten();
    let Some(a) = anim else {
        entry.rig_ui.pose_key = None;
        entry.doc.set_pose_preview(None);
        return;
    };
    let key =
        (entry.doc.history.current_id(), entry.doc.state().rig.clone(), a, entry.rig_ui.time_ms, entry.rig_ui.nearest);
    if entry.rig_ui.pose_key.as_ref() == Some(&key) && entry.doc.has_pose_preview() {
        return;
    }
    let filter = if key.4 { ResizeFilter::Nearest } else { ResizeFilter::Bilinear };
    let posed = rig::posed_copy(entry.doc.state(), Some(a), key.3, filter);
    entry.doc.set_pose_preview(Some(posed));
    entry.rig_ui.pose_key = Some(key);
}

/// Bones: a tapered shape from head to tip, joints, and the name of the
/// selected one. Drawn while the Bone tool is active.
pub fn draw_overlay(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    if state.effective_tool() != super::ToolKind::Bone {
        return;
    }
    let Some(entry) = state.doc(doc_id) else { return };
    let rig = &entry.doc.state().rig;
    let shadow = Stroke::new(3.0, Color32::from_black_alpha(120));
    let bones = screen_bones(entry);
    for (id, h, t) in &bones {
        let sel = entry.rig_ui.sel == Some(*id);
        let col = if sel { Color32::from_rgb(255, 200, 40) } else { Color32::from_rgb(90, 200, 255) };
        let axis = *t - *h;
        let len = axis.length();
        let dir = if len > 0.01 { axis / len } else { egui::vec2(1.0, 0.0) };
        let side = egui::vec2(-dir.y, dir.x) * (len * 0.12).clamp(2.5, 7.0);
        let waist = *h + axis * 0.2;
        let shape = vec![*h, waist + side, *t, waist - side];
        painter.add(egui::Shape::closed_line(shape.clone(), shadow));
        painter.add(egui::Shape::convex_polygon(shape, col.gamma_multiply(0.35), Stroke::new(1.5, col)));
        painter.circle_filled(*h, 4.0, col);
        painter.circle_stroke(*h, 4.0, Stroke::new(1.0, Color32::BLACK));
        if sel {
            painter.circle_stroke(*t, 5.0, Stroke::new(2.0, col));
            if let Some(b) = rig.bone(*id) {
                let g = painter.layout_no_wrap(b.name.clone(), egui::FontId::proportional(11.0), Color32::WHITE);
                let at = *h + egui::vec2(8.0, -16.0);
                painter.rect_filled(
                    egui::Rect::from_min_size(at, g.size() + egui::vec2(6.0, 2.0)),
                    2.0,
                    Color32::from_black_alpha(170),
                );
                painter.galley(at + egui::vec2(3.0, 1.0), g, Color32::WHITE);
            }
        }
    }
    // The bone being dragged out.
    if state.session_doc == Some(doc_id) {
        if let Some(ToolSession::Bone { gesture: BoneGesture::Create { head, .. }, cur, .. }) = &state.session {
            let (a, b) = (entry.view.doc_to_screen(*head), entry.view.doc_to_screen(*cur));
            painter.line_segment([a, b], shadow);
            painter.line_segment([a, b], Stroke::new(2.0, Color32::from_rgb(255, 200, 40)));
            painter.circle_filled(a, 4.0, Color32::from_rgb(255, 200, 40));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seg_dist_clamps_to_the_ends() {
        let (a, b) = (egui::pos2(0.0, 0.0), egui::pos2(10.0, 0.0));
        assert!((seg_dist(egui::pos2(5.0, 3.0), a, b) - 3.0).abs() < 1e-4);
        assert!((seg_dist(egui::pos2(-4.0, 3.0), a, b) - 5.0).abs() < 1e-4);
        assert!((seg_dist(egui::pos2(1.0, 1.0), a, a) - 2f32.sqrt()).abs() < 1e-4);
    }

    #[test]
    fn bone_names_do_not_repeat() {
        let mut rig = Rig::default();
        let a = unique_bone_name(&rig);
        rig.add_bone(a.clone(), None, 0.0, 0.0, 1.0);
        assert_ne!(unique_bone_name(&rig), a);
    }
}
