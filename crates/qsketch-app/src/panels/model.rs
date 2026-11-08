//! Model panel: a 3D model wearing a document as its live texture, so you can see what you
//! are painting. The design, including what comes next, is in `docs/Q3D.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{epaint::Vertex as ScreenVertex, Color32, RichText, Sense, Ui};
use qsketch_3d::camera::{model_matrix, mul, pick_triangle, up_in_mesh};
use qsketch_3d::{find_sections, Mesh, Orbit, Sections};
use qsketch_core::Pt;

use super::model_render::ModelCallback;
use crate::canvas::render::needs_full_upload;
use crate::state::{AppState, DocId};
use crate::ui::widgets::small_button;

const SPIN_RADIANS_PER_SECOND: f32 = 0.6;
const DRAG_RADIANS_PER_POINT: f32 = 0.01;
/// Sections are found automatically up to this size; larger meshes pause the app noticeably, so they wait for the button.
const AUTO_SECTION_TRIANGLES: usize = 400_000;
const CANVAS_HIGHLIGHT: Color32 = Color32::from_rgba_premultiplied(0, 90, 115, 120);

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
    sections: Option<Arc<Sections>>,
    /// Bumped whenever `sections` changes, so the GPU copy is replaced.
    sections_generation: u64,
    selected: Option<u32>,
    tint_all: bool,
    /// Also highlight the selected section on the canvas.
    show_on_canvas: bool,
}

/// What a canvas needs to highlight the selected section. Cheap to take each frame.
pub struct CanvasOverlay {
    mesh: Arc<Mesh>,
    sections: Arc<Sections>,
    selected: u32,
}

impl CanvasOverlay {
    /// Fills the selected section's triangles at their UV positions on the canvas.
    pub fn paint(&self, painter: &egui::Painter, view: &crate::canvas::view::CanvasView, width: f32, height: f32) {
        let mut shape = egui::Mesh::default();
        for (t, tri) in self.mesh.indices.chunks_exact(3).enumerate() {
            if self.sections.of_triangle[t] != self.selected {
                continue;
            }
            let first = shape.vertices.len() as u32;
            for &i in tri {
                let uv = self.mesh.vertices[i as usize].uv;
                let pos = view.doc_to_screen(Pt::new(uv[0] * width, uv[1] * height));
                shape.vertices.push(ScreenVertex { pos, uv: egui::epaint::WHITE_UV, color: CANVAS_HIGHLIGHT });
            }
            shape.indices.extend([first, first + 1, first + 2]);
        }
        painter.add(shape);
    }
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
                self.sections = None;
                self.selected = None;
                self.show_on_canvas = true;
                if self.mesh.as_ref().is_some_and(|m| m.triangle_count() <= AUTO_SECTION_TRIANGLES) {
                    self.find_sections();
                }
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    fn find_sections(&mut self) {
        let Some(mesh) = &self.mesh else { return };
        self.sections = Some(Arc::new(find_sections(mesh, up_in_mesh(self.turns))));
        self.sections_generation += 1;
        self.selected = None;
    }

    /// The highlight for a canvas, if it shows the document this model is wearing.
    pub fn canvas_overlay(&self, doc: DocId, active: Option<DocId>) -> Option<CanvasOverlay> {
        if !self.show_on_canvas || self.texture.or(active) != Some(doc) {
            return None;
        }
        Some(CanvasOverlay { mesh: self.mesh.clone()?, sections: self.sections.clone()?, selected: self.selected? })
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

    section_row(ui, state);
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
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

    let aspect = rect.width() / rect.height();
    let matrix = model_matrix(model.center, model.radius, model.turns);
    if response.clicked() {
        if let (Some(p), Some(sections)) = (response.interact_pointer_pos(), &model.sections) {
            let ndc = [(p.x - rect.left()) / rect.width() * 2.0 - 1.0, 1.0 - (p.y - rect.top()) / rect.height() * 2.0];
            let hit = pick_triangle(&mesh, &mul(&model.orbit.view_proj(aspect), &matrix), ndc);
            let picked = hit.map(|t| sections.of_triangle[t]);
            model.selected = if picked == model.selected { None } else { picked };
        }
    }

    let doc = model.texture.or(state.active_doc);
    let doc_size = doc.and_then(|id| state.doc(id)).map_or((1, 1), |d| (d.doc.width(), d.doc.height()));
    let ppp = ui.ctx().pixels_per_point();
    let size = ((rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32);
    let model = &state.model;
    let callback = ModelCallback {
        mesh,
        generation: model.generation,
        sections: model.sections.clone(),
        sections_generation: model.sections_generation,
        selected: model.selected.map_or(-1, |s| s as i32),
        tint_all: model.tint_all,
        view_proj: model.orbit.view_proj(aspect),
        model: matrix,
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

/// One button per section; clicking it selects the section, and clicking again clears it.
fn section_row(ui: &mut Ui, state: &mut AppState) {
    let model = &mut state.model;
    let Some(sections) = model.sections.clone() else {
        if small_button(ui, "Find Sections").on_hover_text("Large models take a moment").clicked() {
            model.find_sections();
        }
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut model.tint_all, "Tint All");
        ui.checkbox(&mut model.show_on_canvas, "Show on Canvas");
    });
    ui.horizontal_wrapped(|ui| {
        for (i, section) in sections.list.iter().enumerate() {
            let i = i as u32;
            if ui.selectable_label(model.selected == Some(i), &section.name).clicked() {
                model.selected = if model.selected == Some(i) { None } else { Some(i) };
            }
        }
    });
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
            // Which side is "top" depends on which way is up.
            if state.model.sections.is_some() {
                state.model.find_sections();
            }
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
