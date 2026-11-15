//! The Rig dialog: build a bone skeleton, attach layers to bones, key the
//! bones over time with a live preview, and bake the result to frames.
//!
//! The dialog edits a working copy of the document's rig (kept inside a
//! snapshot of the document, which also feeds the preview); OK writes it
//! back as one undo step, "Bake to New Document" turns the selected
//! animation into an ordinary frame animation in a new tab.

use egui::{Context, Ui};
use qsketch_core::raster::ResizeFilter;
use qsketch_core::rig::{self, BoneId, Curve, KeyValue};
use qsketch_core::{DocState, Document};

use super::modal;
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;
use crate::workspace::PanelKind;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Easing {
    #[default]
    Linear,
    Stepped,
    Ease,
    EaseIn,
    EaseOut,
}

impl Easing {
    pub const ALL: [Easing; 5] = [Easing::Linear, Easing::Stepped, Easing::Ease, Easing::EaseIn, Easing::EaseOut];

    pub fn label(self) -> &'static str {
        match self {
            Easing::Linear => "Linear",
            Easing::Stepped => "Stepped",
            Easing::Ease => "Ease in-out",
            Easing::EaseIn => "Ease in",
            Easing::EaseOut => "Ease out",
        }
    }

    pub fn curve(self) -> Curve {
        match self {
            Easing::Linear => Curve::Linear,
            Easing::Stepped => Curve::Stepped,
            Easing::Ease => Curve::Bezier([0.42, 0.0, 0.58, 1.0]),
            Easing::EaseIn => Curve::Bezier([0.42, 0.0, 1.0, 1.0]),
            Easing::EaseOut => Curve::Bezier([0.0, 0.0, 0.58, 1.0]),
        }
    }
}

pub struct RigDialog {
    pub doc: DocId,
    /// The document as it was when the dialog opened; its `rig` is the
    /// working copy being edited.
    pub base: DocState,
    pub title: String,
    pub sel_bone: Option<BoneId>,
    pub anim: Option<usize>,
    pub time_ms: u32,
    pub fps: u32,
    pub nearest: bool,
    pub easing: Easing,
    /// The selected bone's offsets at `time_ms`, as edited (see `refresh`).
    pub tr: [f32; 2],
    pub rot: f32,
    pub sc: [f32; 2],
    /// What `tr`/`rot`/`sc` were last read for: bone, animation, time.
    synced: Option<(Option<BoneId>, Option<usize>, u32)>,
    preview: Option<egui::TextureHandle>,
    stale: bool,
}

pub fn open(state: &mut AppState, doc: DocId) {
    let Some(e) = state.doc(doc) else { return };
    let base = e.doc.state().clone();
    let anim = if base.rig.animations.is_empty() { None } else { Some(0) };
    let sel_bone = base.rig.bones.first().map(|b| b.id);
    state.dialogs.rig = Some(RigDialog {
        doc,
        title: e.doc.title.clone(),
        base,
        sel_bone,
        anim,
        time_ms: 0,
        fps: 12,
        nearest: false,
        easing: Easing::Linear,
        tr: [0.0; 2],
        rot: 0.0,
        sc: [1.0; 2],
        synced: None,
        preview: None,
        stale: true,
    });
}

impl RigDialog {
    fn filter(&self) -> ResizeFilter {
        if self.nearest {
            ResizeFilter::Nearest
        } else {
            ResizeFilter::Bilinear
        }
    }

    /// Re-read the editable offsets when the bone, animation or time moved.
    fn refresh(&mut self) {
        let key = (self.sel_bone, self.anim, self.time_ms);
        if self.synced == Some(key) {
            return;
        }
        self.synced = Some(key);
        self.stale = true;
        (self.tr, self.rot, self.sc) = match (self.sel_bone, self.anim) {
            (Some(b), Some(a)) => self.base.rig.offsets(a, b, self.time_ms),
            _ => ([0.0; 2], 0.0, [1.0; 2]),
        };
    }

    fn duration(&self) -> u32 {
        self.anim.and_then(|a| self.base.rig.animations.get(a)).map_or(1000, |a| a.duration_ms)
    }

    fn draw_preview(&mut self, ctx: &Context) {
        if !self.stale && self.preview.is_some() {
            return;
        }
        self.stale = false;
        let r = rig::render_pose(&self.base, self.anim, self.time_ms, self.filter());
        let img = egui::ColorImage::from_rgba_unmultiplied([r.width() as usize, r.height() as usize], &r.to_rgba());
        let opts = if self.nearest { egui::TextureOptions::NEAREST } else { egui::TextureOptions::LINEAR };
        self.preview = Some(ctx.load_texture("rig_preview", img, opts));
    }
}

pub fn show(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.rig.as_mut() else { return };
    d.refresh();
    d.draw_preview(ctx);
    let mut bake = false;
    let mut export = false;
    let (ok_cancel, closed) = modal(ctx, "rig", "Rig Animation", 820.0, |ui| {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(300.0);
                bones_section(ui, d);
                ui.add_space(8.0);
                layers_section(ui, d);
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.set_width(480.0);
                preview_section(ui, d);
                ui.add_space(6.0);
                animation_section(ui, d);
                ui.add_space(6.0);
                keys_section(ui, d);
                ui.add_space(6.0);
                bake = bake_section(ui, d);
                ui.horizontal(|ui| {
                    export = ui
                        .button("Export for Games…")
                        .on_hover_text("rig.json plus a PNG per layer, for a game runtime")
                        .clicked();
                });
            });
        });
        ok_cancel(ui)
    });
    let (ok, cancel) = ok_cancel;
    if export {
        let d = state.dialogs.rig.take().unwrap();
        apply_rig(state, d.doc, &d.base);
        crate::files::export_rig(state, d.doc);
    } else if bake {
        let d = state.dialogs.rig.take().unwrap();
        apply_rig(state, d.doc, &d.base);
        bake_to_new_doc(state, &d);
    } else if ok {
        let d = state.dialogs.rig.take().unwrap();
        apply_rig(state, d.doc, &d.base);
    } else if cancel || closed {
        state.dialogs.rig = None;
    }
}

fn ok_cancel(ui: &mut Ui) -> (bool, bool) {
    let (mut ok, mut cancel) = (false, false);
    ui.add_space(10.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.button("OK").clicked() {
            ok = true;
        }
        if ui.button("Cancel").clicked() {
            cancel = true;
        }
    });
    (ok, cancel)
}

// --- bones ---------------------------------------------------------------------------

fn bones_section(ui: &mut Ui, d: &mut RigDialog) {
    ui.strong("Bones");
    ui.label(egui::RichText::new("Setup pose: how the art is drawn.").weak().small());
    let rows: Vec<(BoneId, String, usize)> = {
        let rig = &d.base.rig;
        rig.bones
            .iter()
            .map(|b| {
                let mut depth = 0;
                let mut p = b.parent;
                while let Some(pid) = p {
                    depth += 1;
                    p = rig.bone(pid).and_then(|pb| pb.parent);
                }
                (b.id, b.name.clone(), depth)
            })
            .collect()
    };
    egui::ScrollArea::vertical().id_salt("rig_bones").max_height(130.0).show(ui, |ui| {
        if rows.is_empty() {
            ui.label(egui::RichText::new("No bones yet. Add one below.").weak());
        }
        for (id, name, depth) in &rows {
            ui.horizontal(|ui| {
                ui.add_space(*depth as f32 * 14.0);
                if ui.selectable_label(d.sel_bone == Some(*id), name).clicked() {
                    d.sel_bone = Some(*id);
                }
            });
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Add Bone").clicked() {
            let (w, h) = (d.base.width as f32, d.base.height as f32);
            let n = d.base.rig.bones.len() + 1;
            let parent = d.sel_bone.filter(|&p| d.base.rig.bone(p).is_some());
            let (x, y, len) = match parent.and_then(|p| d.base.rig.bone(p)) {
                Some(p) => (p.length, 0.0, (p.length * 0.8).max(8.0)),
                None => (w / 2.0, h / 2.0, (h / 6.0).max(8.0)),
            };
            let id = d.base.rig.add_bone(format!("bone {n}"), parent, x, y, len);
            d.sel_bone = Some(id);
            d.stale = true;
        }
        if ui.add_enabled(d.sel_bone.is_some(), egui::Button::new("Delete")).clicked() {
            if let Some(b) = d.sel_bone.take() {
                d.base.rig.remove_bone(b);
                d.sel_bone = d.base.rig.bones.first().map(|b| b.id);
                d.stale = true;
            }
        }
    });
    let Some(id) = d.sel_bone else { return };
    let mut changed = false;
    if let Some(b) = d.base.rig.bone_mut(id) {
        egui::Grid::new("rig_bone_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut b.name);
            ui.end_row();
            ui.label("Position");
            ui.horizontal(|ui| {
                changed |= ui.add(egui::DragValue::new(&mut b.x).speed(0.5).prefix("x ")).changed();
                changed |= ui.add(egui::DragValue::new(&mut b.y).speed(0.5).prefix("y ")).changed();
            });
            ui.end_row();
            ui.label("Rotation");
            changed |= ui.add(egui::DragValue::new(&mut b.rotation).speed(0.5).suffix("°")).changed();
            ui.end_row();
            ui.label("Scale");
            ui.horizontal(|ui| {
                changed |= ui.add(egui::DragValue::new(&mut b.scale[0]).speed(0.01).prefix("x ")).changed();
                changed |= ui.add(egui::DragValue::new(&mut b.scale[1]).speed(0.01).prefix("y ")).changed();
            });
            ui.end_row();
            ui.label("Length");
            changed |= ui.add(egui::DragValue::new(&mut b.length).speed(0.5).range(0.0..=10_000.0)).changed();
            ui.end_row();
        });
    }
    if changed {
        d.stale = true;
        d.synced = None;
    }
}

// --- layers --------------------------------------------------------------------------

fn layers_section(ui: &mut Ui, d: &mut RigDialog) {
    ui.strong("Attach layers");
    ui.label(egui::RichText::new("A layer follows the bone it is attached to.").weak().small());
    let layers: Vec<(u64, String)> =
        d.base.layers.iter().rev().filter(|l| l.owns_pixels()).map(|l| (l.id(), l.name().to_string())).collect();
    let bones: Vec<(BoneId, String)> = d.base.rig.bones.iter().map(|b| (b.id, b.name.clone())).collect();
    egui::ScrollArea::vertical().id_salt("rig_layers").max_height(140.0).show(ui, |ui| {
        egui::Grid::new("rig_layer_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            for (lid, name) in &layers {
                ui.label(name);
                let cur = d.base.rig.attached_bone(*lid);
                let text =
                    cur.and_then(|b| bones.iter().find(|(id, _)| *id == b)).map_or("(none)", |(_, n)| n.as_str());
                egui::ComboBox::from_id_salt(("rig_attach", *lid)).selected_text(text).width(130.0).show_ui(ui, |ui| {
                    if ui.selectable_label(cur.is_none(), "(none)").clicked() {
                        d.base.rig.detach(*lid);
                        d.stale = true;
                    }
                    for (bid, bname) in &bones {
                        if ui.selectable_label(cur == Some(*bid), bname).clicked() {
                            d.base.rig.attach(*lid, *bid);
                            d.stale = true;
                        }
                    }
                });
                ui.end_row();
            }
        });
    });
}

// --- preview -------------------------------------------------------------------------

fn preview_section(ui: &mut Ui, d: &mut RigDialog) {
    let Some(tex) = d.preview.clone() else { return };
    let (iw, ih) = (d.base.width as f32, d.base.height as f32);
    let scale = (480.0 / iw).min(230.0 / ih).min(8.0);
    let size = egui::vec2(iw * scale, ih * scale);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(480.0, size.y.max(40.0)), egui::Sense::hover());
    let img = egui::Rect::from_center_size(rect.center(), size);
    crate::ui::widgets::checkerboard(ui.painter(), img, 8.0);
    ui.painter().image(
        tex.id(),
        img,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    // The bones, over the picture, in the current pose.
    let pose = d.base.rig.pose(d.anim.map(|a| (a, d.time_ms)));
    let to_screen = |p: qsketch_core::Pt| egui::pos2(img.min.x + p.x * scale, img.min.y + p.y * scale);
    for (i, b) in d.base.rig.bones.iter().enumerate() {
        let Some(xf) = pose.get(i) else { continue };
        let (a, z) = (xf.apply(qsketch_core::Pt::new(0.0, 0.0)), xf.apply(qsketch_core::Pt::new(b.length, 0.0)));
        let sel = d.sel_bone == Some(b.id);
        let col = if sel { egui::Color32::from_rgb(255, 200, 40) } else { egui::Color32::from_rgb(80, 200, 255) };
        ui.painter().line_segment([to_screen(a), to_screen(z)], egui::Stroke::new(if sel { 3.0 } else { 2.0 }, col));
        ui.painter().circle_filled(to_screen(a), 4.0, col);
    }
}

// --- animation -----------------------------------------------------------------------

fn animation_section(ui: &mut Ui, d: &mut RigDialog) {
    ui.strong("Animation");
    ui.horizontal(|ui| {
        let names: Vec<String> = d.base.rig.animations.iter().map(|a| a.name.clone()).collect();
        let cur = d.anim.and_then(|i| names.get(i).cloned()).unwrap_or_else(|| "(setup pose)".into());
        egui::ComboBox::from_id_salt("rig_anim").selected_text(cur).width(150.0).show_ui(ui, |ui| {
            if ui.selectable_label(d.anim.is_none(), "(setup pose)").clicked() {
                d.anim = None;
            }
            for (i, n) in names.iter().enumerate() {
                if ui.selectable_label(d.anim == Some(i), n).clicked() {
                    d.anim = Some(i);
                    d.time_ms = d.time_ms.min(d.base.rig.animations[i].duration_ms);
                }
            }
        });
        if ui.button("New").clicked() {
            let n = d.base.rig.animations.len() + 1;
            d.anim = Some(d.base.rig.add_animation(format!("anim {n}"), 1000));
            d.time_ms = 0;
        }
        if ui.add_enabled(d.anim.is_some(), egui::Button::new("Delete")).clicked() {
            if let Some(a) = d.anim.take() {
                d.base.rig.animations.remove(a);
                d.anim = (!d.base.rig.animations.is_empty()).then_some(0);
                d.time_ms = 0;
            }
        }
    });
    if let Some(a) = d.anim.and_then(|i| d.base.rig.animations.get_mut(i)) {
        let mut dur_changed = false;
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut a.name);
            dur_changed = ui.add(egui::DragValue::new(&mut a.duration_ms).range(1..=600_000).suffix(" ms")).changed();
            ui.checkbox(&mut a.looped, "Loop");
        });
        if dur_changed {
            d.time_ms = d.time_ms.min(a.duration_ms);
        }
    }
    let dur = d.duration();
    ui.horizontal(|ui| {
        ui.label("Time");
        ui.spacing_mut().slider_width = 330.0;
        ui.add_enabled(d.anim.is_some(), egui::Slider::new(&mut d.time_ms, 0..=dur).suffix(" ms"));
    });
}

// --- keys ----------------------------------------------------------------------------

fn keys_section(ui: &mut Ui, d: &mut RigDialog) {
    let (Some(bone), Some(anim)) = (d.sel_bone, d.anim) else {
        ui.label(egui::RichText::new("Pick a bone and an animation to set keys.").weak());
        return;
    };
    let name = d.base.rig.bone(bone).map_or(String::new(), |b| b.name.clone());
    ui.strong(format!("Keys for “{name}” at {} ms", d.time_ms));
    let t = d.time_ms;
    let curve = d.easing.curve();
    let mut key: Option<KeyValue> = None;
    egui::Grid::new("rig_keys_grid").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("Move");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut d.tr[0]).speed(0.5).prefix("x "));
            ui.add(egui::DragValue::new(&mut d.tr[1]).speed(0.5).prefix("y "));
        });
        if ui.button("Key").clicked() {
            key = Some(KeyValue::Translate(d.tr));
        }
        ui.end_row();
        ui.label("Rotate");
        ui.add(egui::DragValue::new(&mut d.rot).speed(0.5).suffix("°"));
        if ui.button("Key").clicked() {
            key = Some(KeyValue::Rotate(d.rot));
        }
        ui.end_row();
        ui.label("Scale");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut d.sc[0]).speed(0.01).prefix("x "));
            ui.add(egui::DragValue::new(&mut d.sc[1]).speed(0.01).prefix("y "));
        });
        if ui.button("Key").clicked() {
            key = Some(KeyValue::Scale(d.sc));
        }
        ui.end_row();
    });
    if let Some(k) = key {
        d.base.rig.set_key(anim, bone, t, k, curve);
        d.stale = true;
    }
    ui.horizontal(|ui| {
        ui.label("Easing for new keys");
        egui::ComboBox::from_id_salt("rig_easing").selected_text(d.easing.label()).show_ui(ui, |ui| {
            for e in Easing::ALL {
                ui.selectable_value(&mut d.easing, e, e.label());
            }
        });
    });
    let times = d.base.rig.key_times(anim, bone);
    if !times.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label("Keys at");
            for k in times {
                if ui.selectable_label(k == d.time_ms, format!("{k}")).clicked() {
                    d.time_ms = k;
                }
            }
            if ui.small_button("Delete keys here").clicked() {
                d.base.rig.remove_keys_at(anim, bone, d.time_ms);
                d.synced = None;
            }
        });
    }
}

// --- bake ----------------------------------------------------------------------------

/// Returns whether Bake was clicked.
fn bake_section(ui: &mut Ui, d: &mut RigDialog) -> bool {
    let mut bake = false;
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Bake at");
        ui.add(egui::DragValue::new(&mut d.fps).range(1..=120).suffix(" fps"));
        if ui.checkbox(&mut d.nearest, "Pixel art (no smoothing)").changed() {
            d.stale = true;
        }
        let frames = d.anim.and_then(|a| d.base.rig.animations.get(a)).map(|a| rig::bake_frame_count(a, d.fps));
        if ui.add_enabled(frames.is_some(), egui::Button::new("Bake to New Document")).clicked() {
            bake = true;
        }
        if let Some(n) = frames {
            ui.label(egui::RichText::new(format!("{n} frames")).weak());
        }
    });
    bake
}

// --- applying ------------------------------------------------------------------------

fn apply_rig(state: &mut AppState, doc: DocId, base: &DocState) {
    let mut rig = base.rig.clone();
    let Some(e) = state.doc_mut(doc) else { return };
    // Attachments to layers deleted meanwhile don't survive.
    let ids: Vec<_> = e.doc.state().layers.iter().map(|l| l.id()).collect();
    rig.prune(&ids);
    if e.doc.state().rig == rig {
        return;
    }
    e.doc.state_mut().rig = rig;
    e.doc.commit("Edit Rig");
}

fn bake_to_new_doc(state: &mut AppState, d: &RigDialog) {
    let Some(anim) = d.anim else { return };
    match rig::bake(&d.base, anim, d.fps, d.filter()) {
        Ok(baked) => {
            let name = d.base.rig.animations.get(anim).map_or("", |a| a.name.as_str());
            let n = baked.frames.len();
            let doc = Document::from_state(baked, format!("{} – {name}", d.title), None, "Bake Rig Animation");
            state.add_document(doc);
            state.show_panel_requests.push(PanelKind::Timeline);
            state.toasts.push(Level::Info, format!("Baked {n} frames into a new document."));
        }
        Err(e) => state.toasts.push(Level::Error, format!("Bake failed: {e}")),
    }
}
