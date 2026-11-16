//! Model panel: a 3D model wearing a document as its live texture, so you can see what you
//! are painting. The design, including what comes next, is in `docs/Q3D.md`.

use crate::native_dialog::Ask as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use egui::{epaint::Vertex as ScreenVertex, Color32, RichText, Sense, Ui};
use qsketch_3d::camera::{model_matrix, mul, pick_triangle, up_in_mesh};
use qsketch_3d::hidden::{find_hidden, without_triangles};
use qsketch_3d::mesh::{embedded_texture, EmbeddedImage};
use qsketch_3d::unwrap::{rearrange, section_coverage, Report, Texture};
use qsketch_3d::{find_sections, Mesh, Orbit, Sections};
use qsketch_core::{Mask, Pt};

use super::model_render::ModelCallback;
use crate::canvas::render::needs_full_upload;
use crate::state::{AppState, DocId};
use crate::ui::widgets::small_button;

const SPIN_RADIANS_PER_SECOND: f32 = 0.6;
const DRAG_RADIANS_PER_POINT: f32 = 0.01;
/// Sections are found automatically up to this size; larger meshes pause the app noticeably, so they wait for the button.
const AUTO_SECTION_TRIANGLES: usize = 400_000;
/// A selected section extends this far past its edges, so paint reaches the rim without leaving a seam.
const SELECTION_PADDING: u32 = 3;
/// About three rows of section buttons.
const SECTION_LIST_HEIGHT: f32 = 66.0;
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
    job: Option<Receiver<Result<Finished, String>>>,
    /// Rearrange normally drops surfaces nothing can see; this keeps them.
    keep_hidden: bool,
    /// The sections were renamed, merged or read from the sidecar, so they are kept rather than found again.
    edited: bool,
    /// The sidecar is behind the sections and is written once typing stops.
    unsaved: bool,
    /// The section being renamed in its menu, the text typed so far, and the frame the menu was last drawn.
    renaming: Option<(u32, String, u64)>,
    /// The color texture stored inside the model file, until it has been extracted.
    embedded: Option<EmbeddedImage>,
}

/// A finished Rearrange Texture run, whose files are on disk.
struct Finished {
    model: PathBuf,
    texture: PathBuf,
    sections: Sections,
    report: Report,
    removed: usize,
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
        let loaded = std::fs::read(path)
            .map_err(anyhow::Error::from)
            .and_then(|b| Ok((Mesh::from_glb(&b)?, embedded_texture(&b))));
        match loaded {
            Ok((mesh, embedded)) => {
                self.embedded = embedded;
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
                (self.edited, self.unsaved, self.renaming) = (false, false, None);
                let triangles = self.mesh.as_ref().map_or(0, |m| m.triangle_count());
                let saved =
                    std::fs::read_to_string(sidecar(path)).ok().and_then(|t| Sections::from_json(&t, triangles));
                if let Some((sections, turns)) = saved {
                    self.sections = Some(Arc::new(sections));
                    self.sections_generation += 1;
                    self.turns = turns;
                    self.edited = true;
                } else if triangles <= AUTO_SECTION_TRIANGLES {
                    self.find_sections();
                }
            }
            Err(e) => {
                let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
                self.error = Some(format!("Couldn't open {name}: {e:#}"));
            }
        }
    }

    fn find_sections(&mut self) {
        let Some(mesh) = &self.mesh else { return };
        self.sections = Some(Arc::new(find_sections(mesh, up_in_mesh(self.turns), None)));
        self.sections_generation += 1;
        self.selected = None;
        self.edited = false;
    }

    /// Changes the sections and marks them to be saved beside the model.
    fn edit_sections(&mut self, edit: impl FnOnce(&mut Sections)) {
        let Some(sections) = &mut self.sections else { return };
        edit(Arc::make_mut(sections));
        self.sections_generation += 1;
        (self.edited, self.unsaved) = (true, true);
    }

    /// Writes the sections and the model's turns to the sidecar file.
    fn save_sections(&mut self) {
        self.unsaved = false;
        let (Some(sections), Some(path)) = (&self.sections, &self.path) else { return };
        let file = sidecar(path);
        if let Err(e) = std::fs::write(&file, sections.to_json(self.turns)) {
            self.error = Some(format!("Couldn't save {}: {e}", file.display()));
        }
    }

    /// Throws away renames and merges and finds the sections afresh.
    fn discard_section_edits(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(sidecar(path));
        }
        self.unsaved = false;
        self.find_sections();
    }

    /// The highlight for a canvas, if it shows the document this model is wearing.
    pub fn canvas_overlay(&self, doc: DocId, active: Option<DocId>) -> Option<CanvasOverlay> {
        if !self.show_on_canvas || self.texture.or(active) != Some(doc) {
            return None;
        }
        Some(CanvasOverlay { mesh: self.mesh.clone()?, sections: self.sections.clone()?, selected: self.selected? })
    }
}

/// Where a model's sections are kept: `name.sections.json` beside it. The model file is never changed.
fn sidecar(model: &Path) -> PathBuf {
    model.with_extension("sections.json")
}

fn choose_file(state: &mut AppState) {
    if let Some(path) = rfd::FileDialog::new().set_title("Open 3D Model").add_filter("glTF binary", &["glb"]).ask_file()
    {
        state.model.open(&path);
    }
}

/// The first `name_suffix` base beside the model for which none of `extensions` exists yet.
fn free_base(model: &Path, suffix: &str, extensions: &[&str]) -> PathBuf {
    let stem = model.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "model".into());
    let directory = model.parent().unwrap_or(Path::new("."));
    (1..)
        .map(|n| directory.join(if n == 1 { format!("{stem}_{suffix}") } else { format!("{stem}_{suffix}{n}") }))
        .find(|base| extensions.iter().all(|e| !base.with_extension(e).exists()))
        .expect("an unused file name")
}

/// Saves the texture stored inside the model as an image beside it, and opens that to paint on.
fn extract_texture(state: &mut AppState) {
    let (Some(image), Some(path)) = (state.model.embedded.clone(), state.model.path.clone()) else { return };
    // An earlier extraction that is still untouched is opened again, not copied.
    let first = path.with_file_name(format!(
        "{}_texture.{}",
        path.file_stem().unwrap_or_default().to_string_lossy(),
        image.extension
    ));
    let already = std::fs::read(&first).is_ok_and(|bytes| bytes == image.bytes);
    let file =
        if already { first } else { free_base(&path, "texture", &[image.extension]).with_extension(image.extension) };
    if !already {
        if let Err(e) = std::fs::write(&file, &image.bytes) {
            state.model.error = Some(format!("Couldn't save {}: {e}", file.display()));
            return;
        }
    }
    if let Some(doc) = crate::files::open_path(state, &file) {
        state.model.texture = Some(doc);
        state.model.embedded = None;
        if !already {
            let note = format!("Saved the model's texture as {}", file.display());
            state.toasts.push(crate::ui::toasts::Level::Info, note);
        }
    }
}

/// Starts laying the model's texture out again per section, on a background thread.
fn start_rearrange(state: &mut AppState) {
    let model = &state.model;
    let (Some(mesh), Some(path)) = (model.mesh.clone(), model.path.clone()) else { return };
    let Some(entry) = model.texture.or(state.active_doc).and_then(|id| state.doc(id)) else { return };
    let raster = qsketch_core::composite::flatten(entry.doc.state());
    let (width, height, rgba) = (raster.width(), raster.height(), raster.to_rgba());
    let up = up_in_mesh(model.turns);
    let keep_hidden = model.keep_hidden;
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let run = || -> anyhow::Result<Finished> {
            let source = Texture { width, height, rgba: &rgba };
            let hidden = if keep_hidden { vec![false; mesh.triangle_count()] } else { find_hidden(&mesh) };
            let removed = hidden.iter().filter(|h| **h).count();
            let mesh = without_triangles(&mesh, &hidden);
            let sections = find_sections(&mesh, up, Some(&source));
            let size = width.max(height).clamp(2048, 4096);
            let result = rearrange(&mesh, &sections, &source, size);
            let mut png = Vec::new();
            image::write_buffer_with_format(
                &mut std::io::Cursor::new(&mut png),
                &result.rgba,
                size,
                size,
                image::ColorType::Rgba8,
                image::ImageFormat::Png,
            )?;
            let base = free_base(&path, "rearranged", &["glb", "png"]);
            let (model, texture) = (base.with_extension("glb"), base.with_extension("png"));
            std::fs::write(&texture, &png)?;
            std::fs::write(&model, result.mesh.to_glb(&png))?;
            Ok(Finished { model, texture, sections, report: result.report, removed })
        };
        let _ = send.send(run().map_err(|e| format!("{e:#}")));
    });
    state.model.job = Some(receive);
}

/// Opens the results of a finished Rearrange Texture run.
fn finish_rearrange(ui: &Ui, state: &mut AppState) {
    let Some(job) = &state.model.job else { return };
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    let result = match job.try_recv() {
        Ok(result) => result,
        Err(mpsc::TryRecvError::Empty) => return,
        Err(mpsc::TryRecvError::Disconnected) => Err("the rearranging thread stopped unexpectedly".to_string()),
    };
    state.model.job = None;
    match result {
        Ok(done) => {
            let doc = crate::files::open_path(state, &done.texture);
            let turns = state.model.turns;
            state.model.open(&done.model);
            state.model.embedded = None; // it is the document just opened
            state.model.turns = turns;
            state.model.texture = doc;
            // The triangles are in the same order, so the sections found from the paint still apply.
            state.model.sections = Some(Arc::new(done.sections));
            state.model.sections_generation += 1;
            state.model.selected = None;
            state.model.edited = true;
            state.model.save_sections();
            let r = done.report;
            let note = format!(
                "Texture rearranged: {} islands, {} hidden triangles removed. {} triangles folded over, {} pixels overlap.",
                r.islands, done.removed, r.flipped_triangles, r.overlapping_pixels
            );
            state.toasts.push(crate::ui::toasts::Level::Info, note);
        }
        Err(e) => state.model.error = Some(e),
    }
}

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    finish_rearrange(ui, state);
    if state.model.unsaved && ui.memory(|m| m.focused().is_none()) {
        state.model.save_sections();
    }
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
    let delta = response.drag_delta();
    if response.dragged_by(egui::PointerButton::Primary) {
        model.orbit.rotate(-delta.x * DRAG_RADIANS_PER_POINT, delta.y * DRAG_RADIANS_PER_POINT);
    } else if response.dragged() {
        model.orbit.pan_by(delta.x, delta.y, rect.height(), rect.width() / rect.height());
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
    let note = if doc.is_none() {
        Some(if state.model.embedded.is_some() {
            "Extract Texture opens this model's own texture to paint on"
        } else {
            "Open or create an image to paint as this model's texture"
        })
    } else if doc.is_some_and(|id| needs_full_upload(state.render_state.as_ref(), id, doc_size.0, doc_size.1)) {
        Some("Show this document's canvas to see it on the model")
    } else {
        None
    };
    if let Some(note) = note {
        let galley = ui.painter().layout(
            note.to_string(),
            egui::FontId::proportional(16.0),
            ui.visuals().warn_fg_color,
            rect.width() - 24.0,
        );
        ui.painter().galley(rect.left_top() + egui::vec2(12.0, 12.0), galley, ui.visuals().warn_fg_color);
    }
}

/// Makes the selected section the texture document's selection, as one undoable step.
fn select_section_on_canvas(state: &mut AppState) {
    let model = &state.model;
    let (Some(mesh), Some(sections), Some(section)) = (model.mesh.clone(), model.sections.clone(), model.selected)
    else {
        return;
    };
    let Some(id) = model.texture.or(state.active_doc) else { return };
    let Some(entry) = state.doc_mut(id) else { return };
    let (width, height) = (entry.doc.width(), entry.doc.height());
    let covered = section_coverage(&mesh, &sections, section, width, height, SELECTION_PADDING);
    let mut mask = Mask::new(width, height);
    for (i, _) in covered.iter().enumerate().filter(|(_, c)| **c) {
        mask.set((i as u32 % width) as i32, (i as u32 / width) as i32, 255);
    }
    mask.recompute_bounds();
    entry.doc.state_mut().selection = if mask.is_empty() { None } else { Some(Arc::new(mask)) };
    entry.doc.commit("Select Section");
    entry.sel_outline = None;
    // The selection outline now marks the section, and the tint would hide the paint.
    state.model.selected = None;
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
    let mut select_on_canvas = false;
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut model.tint_all, "Tint All");
        ui.checkbox(&mut model.show_on_canvas, "Show on Canvas");
        if ui
            .add_enabled(model.selected.is_some(), egui::Button::new("Select on Canvas").small())
            .on_hover_text("Make this section the canvas selection, so brushes and fills stay inside it")
            .clicked()
        {
            select_on_canvas = true;
        }
    });
    if select_on_canvas {
        select_section_on_canvas(state);
    }
    let model = &mut state.model;
    // A model with many sections scrolls its list, so the view below keeps its room.
    let mut change = None;
    egui::ScrollArea::vertical().id_salt("model_sections").max_height(SECTION_LIST_HEIGHT).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (i, section) in sections.list.iter().enumerate() {
                let i = i as u32;
                let button = ui
                    .selectable_label(model.selected == Some(i), &section.name)
                    .on_hover_text(format!("{} triangles. Right-click to rename or merge.", section.triangles));
                if button.clicked() {
                    model.selected = if model.selected == Some(i) { None } else { Some(i) };
                }
                button.context_menu(|ui| section_menu(ui, model, &sections, i, &mut change));
            }
        });
    });
    match change {
        Some(Change::Rename(i, name)) => model.edit_sections(|s| s.rename(i, &name)),
        Some(Change::Merge(from, into)) => {
            let mut moved = None;
            model.edit_sections(|s| moved = Some(s.merge(from, into)));
            model.selected = moved.map(|moved| moved(from));
            model.renaming = None;
        }
        Some(Change::Discard) => model.discard_section_edits(),
        None => {}
    }
}

enum Change {
    Rename(u32, String),
    Merge(u32, u32),
    Discard,
}

/// A section's right-click menu: its name to edit, the sections it can merge into, and a way back.
fn section_menu(ui: &mut Ui, model: &mut Model, sections: &Sections, index: u32, change: &mut Option<Change>) {
    // A menu that was not on screen last frame has just opened: start from the current name, ready to type over.
    let frame = ui.ctx().cumulative_pass_nr();
    let opened = model.renaming.as_ref().is_none_or(|(i, _, last)| *i != index || last + 1 < frame);
    if opened {
        model.renaming = Some((index, sections.list[index as usize].name.clone(), frame));
    }
    if let Some((_, text, last)) = &mut model.renaming {
        *last = frame;
        ui.label("Name");
        let field = egui::TextEdit::singleline(text).desired_width(180.0).show(ui);
        if opened {
            field.response.request_focus();
            let all = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(text.chars().count()),
            );
            let mut edit = field.state;
            edit.cursor.set_char_range(Some(all));
            edit.store(ui.ctx(), field.response.id);
        }
        if field.response.changed() {
            *change = Some(Change::Rename(index, text.clone()));
        }
        if field.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            ui.close();
        }
    }
    ui.menu_button("Merge Into", |ui| {
        for (other, section) in sections.list.iter().enumerate().filter(|(other, _)| *other as u32 != index) {
            if ui.button(&section.name).clicked() {
                *change = Some(Change::Merge(index, other as u32));
                ui.close();
            }
        }
    });
    if model.edited {
        ui.separator();
        if ui
            .button("Discard Edits and Find Sections Again")
            .on_hover_text("Forgets every rename and merge for this model")
            .clicked()
        {
            *change = Some(Change::Discard);
            ui.close();
        }
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
            if state.model.edited {
                state.model.save_sections();
            } else if state.model.sections.is_some() {
                // Which side is "top" depends on which way is up.
                state.model.find_sections();
            }
        }
        if small_button(ui, "Reset View")
            .on_hover_text("Drag to turn the model, scroll to zoom, middle or right-drag to slide it")
            .clicked()
        {
            state.model.orbit = Orbit::default();
        }
        if state.model.embedded.is_some()
            && small_button(ui, "Extract Texture")
                .on_hover_text("Save the texture stored inside the model as an image beside it, and open it to paint on")
                .clicked()
        {
            extract_texture(state);
        }
        ui.checkbox(&mut state.model.spin, "Spin");
        if state.model.job.is_some() {
            ui.spinner();
            ui.label("Rearranging…");
        } else {
            let has_texture = state.model.texture.or(state.active_doc).is_some();
            if ui
                .add_enabled(has_texture, egui::Button::new("Rearrange Texture").small())
                .on_hover_text("Lay the texture out again with each part in one piece. Writes a new model and texture beside the original.")
                .on_disabled_hover_text("Open the model's texture first")
                .clicked()
            {
                start_rearrange(state);
            }
        }
        ui.checkbox(&mut state.model.keep_hidden, "Keep Hidden Surfaces")
            .on_hover_text("Rearrange normally removes surfaces that can never be seen, such as the inside of a hollow model");
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
