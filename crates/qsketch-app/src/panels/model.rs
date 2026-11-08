//! Model panel: a 3D model wearing a document as its live texture, so you can see what you
//! are painting. The design, including what comes next, is in `docs/Q3D.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{RichText, Sense, Ui};
use qsketch_3d::camera::model_matrix;
use qsketch_3d::{Mesh, Orbit};

use super::model_render::ModelCallback;
use crate::canvas::render::needs_full_upload;
use crate::state::{AppState, DocId};
use crate::ui::widgets::small_button;

const SPIN_RADIANS_PER_SECOND: f32 = 0.6;
const DRAG_RADIANS_PER_POINT: f32 = 0.01;

#[derive(Default)]
pub struct Model {
    mesh: Option<Arc<Mesh>>,
    path: Option<PathBuf>,
    /// Bumped per load so the GPU copy of the mesh is replaced.
    generation: u64,
    center: [f32; 3],
    radius: f32,
    orbit: Orbit,
    spin: bool,
    /// Quarter turns about X, for models that arrive with the wrong axis up.
    turns: u8,
    /// `None` follows the active document.
    texture: Option<DocId>,
    error: Option<String>,
}

impl Model {
    /// Load a `.glb`; a failure is shown in the panel.
    pub fn open(&mut self, path: &Path) {
        let loaded = std::fs::read(path).map_err(anyhow::Error::from).and_then(|b| Mesh::from_glb(&b));
        match loaded {
            Ok(mesh) => {
                (self.center, self.radius) = mesh.bounds();
                self.mesh = Some(Arc::new(mesh));
                self.path = Some(path.to_path_buf());
                self.generation += 1;
                self.orbit = Orbit::default();
                self.turns = 0;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }
}

fn choose_file(state: &mut AppState) {
    if let Some(path) =
        rfd::FileDialog::new().set_title("Open 3D Model").add_filter("glTF binary", &["glb"]).pick_file()
    {
        state.model.open(&path);
    }
}

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    toolbar(ui, state);
    if let Some(error) = &state.model.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    let Some(mesh) = state.model.mesh.clone() else {
        ui.centered_and_justified(|ui| ui.label(RichText::new("Open a .glb model to see it here").weak()));
        return;
    };

    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::drag());
    if rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }
    let model = &mut state.model;
    if response.dragged() {
        let delta = response.drag_delta();
        model.orbit.rotate(-delta.x * DRAG_RADIANS_PER_POINT, delta.y * DRAG_RADIANS_PER_POINT);
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            model.orbit.zoom_by((-scroll * 0.002).exp());
        }
    }
    if model.spin && !response.dragged() {
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        model.orbit.rotate(-dt * SPIN_RADIANS_PER_SECOND, 0.0);
        ui.ctx().request_repaint();
    }

    let doc = model.texture.or(state.active_doc);
    let doc_size = doc.and_then(|id| state.doc(id)).map_or((1, 1), |d| (d.doc.width(), d.doc.height()));
    let ppp = ui.ctx().pixels_per_point();
    let size = ((rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32);
    let model = &state.model;
    let callback = ModelCallback {
        mesh,
        generation: model.generation,
        view_proj: model.orbit.view_proj(rect.width() / rect.height()),
        model: model_matrix(model.center, model.radius, model.turns),
        doc,
        size,
        doc_size,
    };
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, callback));

    // Only a visible canvas sends its pixels to the GPU, and the model reads those.
    if doc.is_some_and(|id| needs_full_upload(state.render_state.as_ref(), id, doc_size.0, doc_size.1)) {
        ui.painter().text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Show this document's canvas to see it on the model",
            egui::FontId::proportional(16.0),
            ui.visuals().warn_fg_color,
        );
    }
}

fn toolbar(ui: &mut Ui, state: &mut AppState) {
    ui.horizontal_wrapped(|ui| {
        if small_button(ui, "Open…").clicked() {
            choose_file(state);
        }
        if state.model.mesh.is_none() {
            return;
        }
        if small_button(ui, "Rotate 90°").on_hover_text("Turn the model if the wrong axis is up").clicked() {
            state.model.turns = (state.model.turns + 1) % 4;
        }
        if small_button(ui, "Reset View").clicked() {
            state.model.orbit = Orbit::default();
        }
        ui.checkbox(&mut state.model.spin, "Spin");
    });
    if state.model.mesh.is_none() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label("Texture");
        let chosen = state.model.texture.filter(|id| state.doc(*id).is_some());
        let name =
            chosen.and_then(|id| state.doc(id)).map_or_else(|| "Active document".to_string(), |d| d.doc.title.clone());
        let docs: Vec<(DocId, String)> = state.docs.iter().map(|d| (d.id, d.doc.title.clone())).collect();
        egui::ComboBox::from_id_salt("model_texture").selected_text(name).show_ui(ui, |ui| {
            ui.selectable_value(&mut state.model.texture, None, "Active document");
            for (id, title) in docs {
                ui.selectable_value(&mut state.model.texture, Some(id), title);
            }
        });

        if let Some(mesh) = &state.model.mesh {
            ui.label(RichText::new(format!("{} triangles", mesh.triangle_count())).weak());
        }
    });
}
