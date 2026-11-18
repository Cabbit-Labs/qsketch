//! Model panel: a 3D model wearing a document as its live texture, so you can see what you
//! are painting. The design, including what comes next, is in `docs/Q3D.md`.

use crate::native_dialog::Ask as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use egui::{epaint::Vertex as ScreenVertex, Color32, RichText, Sense, Ui};
use qsketch_3d::camera::{model_matrix, mul, pick_triangle, up_in_mesh};
use qsketch_3d::hidden::{find_hidden, without_triangles};
use qsketch_3d::mesh::{embedded_texture, palette_regions, texture_look, EmbeddedImage, TextureLook};
use qsketch_3d::unwrap::{rearrange, section_coverage, section_map, Report, Texture, NO_SECTION};
use qsketch_3d::{find_sections, Mesh, Orbit, Sections};
use qsketch_core::palette::{Palette, PaletteRegion};
use qsketch_core::Rgba8;

use qsketch_core::{Mask, Pt};

use crate::canvas::render::needs_full_upload;
use crate::state::{AppState, DocId};
use crate::ui::widgets::small_button;
use qsketch_3d_view::ModelCallback;

const SPIN_RADIANS_PER_SECOND: f32 = 0.6;
const DRAG_RADIANS_PER_POINT: f32 = 0.01;
/// A selected section extends this far past its edges, so paint reaches the rim without leaving a seam.
const SELECTION_PADDING: u32 = 3;
/// About three rows of section buttons.
const SECTION_LIST_HEIGHT: f32 = 66.0;
/// The section map is made no larger than this on its longer side.
const MAX_MAP_SIDE: u32 = 2048;
const CANVAS_HIGHLIGHT: Color32 = Color32::from_rgba_premultiplied(0, 90, 115, 120);
/// Laid over texels that no shown part of the model uses, with Dim Unused Texels.
const CANVAS_UNUSED: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 175);

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
    map: Option<SectionMap>,
    /// A model being read on another thread, and its path.
    loading: Option<(PathBuf, Receiver<anyhow::Result<Loaded>>)>,
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
    /// How the model's material asks for its texture to be drawn (crisp texels, see-through cut out).
    look: TextureLook,
    /// Which of the model's named parts are shown, one entry per part.
    shown: Vec<bool>,
    /// The triangles of hidden parts, or `None` when every part is shown.
    hidden: Option<Arc<Vec<bool>>>,
    /// Bumped whenever `hidden` changes, so the GPU copy and the section map follow.
    hidden_generation: u64,
    /// Darken the texels on the canvas that no shown part uses.
    dim_unused: bool,
    /// The texture's palette regions from the model file, and the image size they were written for.
    palettes: Option<([u32; 2], Arc<Vec<PaletteRegion>>)>,
    /// Paint freely instead of snapping to the texture's palettes.
    ignore_palettes: bool,
    /// The document the palette regions are set on, so they can be taken off it again.
    regions_doc: Option<DocId>,
    /// The palette region the pointer was last over on that document's canvas, whose colors the panel offers.
    color_region: Option<usize>,
}

/// The model file's palette regions as the document's kind.
fn read_palettes(glb: &[u8]) -> Option<([u32; 2], Arc<Vec<PaletteRegion>>)> {
    let (size, regions) = palette_regions(glb)?;
    let regions = regions
        .into_iter()
        .map(|r| PaletteRegion {
            x: r.rect[0],
            y: r.rect[1],
            width: r.rect[2],
            height: r.rect[3],
            palette: Palette::new(
                r.name.unwrap_or_default(),
                r.colors.iter().map(|c| Rgba8::new(c[0], c[1], c[2], c[3])).collect(),
            ),
        })
        .collect();
    Some((size, Arc::new(regions)))
}

/// A finished Rearrange Texture run, whose files are on disk.
struct Finished {
    model: PathBuf,
    texture: PathBuf,
    sections: Sections,
    report: Report,
    removed: usize,
}

/// A model read from disk, with its sections: the saved ones if it has a sidecar, else found afresh.
struct Loaded {
    path: PathBuf,
    mesh: Mesh,
    embedded: Option<EmbeddedImage>,
    look: TextureLook,
    sections: Option<Sections>,
    turns: u8,
    /// The sections came from the sidecar.
    edited: bool,
    /// Which named parts were shown when the sidecar was written, one entry per part.
    shown: Option<Vec<bool>>,
    palettes: Option<([u32; 2], Arc<Vec<PaletteRegion>>)>,
}

/// Reads a model and its sidecar. Without a sidecar the sections are found here when `find` is set.
fn load(path: &Path, find: bool) -> anyhow::Result<Loaded> {
    let bytes = std::fs::read(path)?;
    let mesh = Mesh::from_glb(&bytes)?;
    let text = std::fs::read_to_string(sidecar(path)).ok();
    let saved = text.as_deref().and_then(|t| Sections::from_json(t, mesh.triangle_count()));
    // The parts on show, kept beside the sections, one entry per part and by name. When the model's
    // parts have changed (a new export with more variants), the choices that still name a part come back.
    let shown = text.as_deref().filter(|_| !mesh.parts.is_empty()).and_then(|t| {
        let root: serde_json::Value = serde_json::from_str(t).ok()?;
        let by_index: Option<Vec<bool>> =
            root.get("shown").and_then(|v| v.as_array()).and_then(|a| a.iter().map(|v| v.as_bool()).collect());
        if let Some(shown) = by_index.as_ref().filter(|s| saved.is_some() && s.len() == mesh.parts.len()) {
            return Some(shown.clone());
        }
        let by_name: serde_json::Map<String, serde_json::Value> = match root
            .get("shown_parts")
            .and_then(|v| v.as_object())
        {
            Some(map) => map.clone(),
            // Written before `shown_parts`: a model's first sections are its parts, in order and named
            // after them, so a section name that is still a part's name carries that part's choice.
            None => {
                let names = root.get("names")?.as_array()?;
                let shown = by_index?;
                (names.len() == shown.len()).then(|| {
                    names.iter().zip(shown).filter_map(|(n, on)| Some((n.as_str()?.to_string(), on.into()))).collect()
                })?
            }
        };
        Some(qsketch_3d::shown_by_name(&mesh.parts, |name| by_name.get(name).and_then(|v| v.as_bool())))
    });
    let (edited, turns) = (saved.is_some(), saved.as_ref().map_or(0, |(_, turns)| *turns));
    let sections = match saved {
        Some((sections, _)) => Some(sections),
        None if find => Some(initial_sections(&mesh, 0)),
        None => None,
    };
    let (embedded, look) = (embedded_texture(&bytes), texture_look(&bytes));
    let palettes = read_palettes(&bytes);
    Ok(Loaded { path: path.to_path_buf(), embedded, look, mesh, sections, turns, edited, shown, palettes })
}

/// The selected section's highlight for a canvas: one image the size of the texture. Cheap to take each frame.
pub struct CanvasOverlay {
    image: egui::TextureHandle,
}

impl CanvasOverlay {
    /// Lays the highlight over a document `width` by `height` pixels, following the view's zoom and rotation.
    pub fn paint(&self, painter: &egui::Painter, view: &crate::canvas::view::CanvasView, width: f32, height: f32) {
        let mut shape = egui::Mesh::with_texture(self.image.id());
        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            let pos = view.doc_to_screen(Pt::new(u * width, v * height));
            shape.vertices.push(ScreenVertex { pos, uv: egui::pos2(u, v), color: Color32::WHITE });
        }
        shape.indices.extend([0, 1, 2, 0, 2, 3]);
        painter.add(shape);
    }
}

/// Which section owns each pixel of the texture, kept until the model, its sections or the size change.
struct SectionMap {
    /// Model generation, sections generation, and the document's size.
    /// Model, sections and hidden-part generations, and the document's size.
    key: (u64, u64, u64, u32, u32),
    size: (u32, u32),
    owner: Vec<u32>,
    /// The overlay image, and the highlighted section and dimming it was made for.
    image: Option<((Option<u32>, bool), egui::TextureHandle)>,
}

impl Model {
    /// Starts loading a `.glb` on another thread; the panel shows it, or why it failed, when that finishes.
    pub fn open(&mut self, path: &Path) {
        let (send, receive) = mpsc::channel();
        let file = path.to_path_buf();
        std::thread::spawn(move || {
            let _ = send.send(load(&file, true));
        });
        self.loading = Some((path.to_path_buf(), receive));
        self.error = None;
    }

    /// Takes a finished load. Returns the model's path once, when it has just appeared.
    fn finish_loading(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        let (path, job) = self.loading.as_ref()?;
        let result = match job.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                return None;
            }
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow::anyhow!("the loading thread stopped unexpectedly")),
        };
        let path = path.clone();
        self.loading = None;
        match result {
            Ok(loaded) => {
                self.show(loaded);
                Some(path)
            }
            Err(e) => {
                let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
                self.error = Some(format!("Couldn't open {name}: {e:#}"));
                None
            }
        }
    }

    /// Replaces the model on show with a loaded one.
    fn show(&mut self, loaded: Loaded) {
        self.embedded = loaded.embedded;
        self.look = loaded.look;
        self.palettes = loaded.palettes;
        (self.center, self.radius) = loaded.mesh.bounds();
        self.mesh = Some(Arc::new(loaded.mesh));
        self.path = Some(loaded.path);
        self.generation += 1;
        self.orbit = Orbit::default();
        self.turns = loaded.turns;
        self.error = None;
        self.sections = loaded.sections.map(Arc::new);
        self.sections_generation += 1;
        self.selected = None;
        self.show_on_canvas = true;
        (self.edited, self.unsaved, self.renaming) = (loaded.edited, false, None);
        let defaults = self.mesh.as_ref().map_or_else(Vec::new, |m| m.parts.iter().map(|p| p.visible).collect());
        self.shown = loaded.shown.unwrap_or(defaults);
        self.update_hidden();
        // Frame what is on show, not every variant the file carries; switching parts later keeps the view still.
        if let (Some(mesh), Some(hidden)) = (&self.mesh, &self.hidden) {
            (self.center, self.radius) = mesh.bounds_without(hidden);
        }
    }

    /// Works out which triangles the hidden parts leave out, after `shown` changed.
    fn update_hidden(&mut self) {
        self.hidden = self.mesh.as_ref().and_then(|mesh| {
            let mut hidden = vec![false; mesh.triangle_count()];
            for (part, _) in mesh.parts.iter().zip(&self.shown).filter(|(_, shown)| !**shown) {
                for t in part.triangles.clone() {
                    if let Some(h) = hidden.get_mut(t as usize) {
                        *h = true;
                    }
                }
            }
            hidden.contains(&true).then(|| Arc::new(hidden))
        });
        self.hidden_generation += 1;
    }

    fn find_sections(&mut self) {
        let Some(mesh) = &self.mesh else { return };
        self.sections = Some(Arc::new(initial_sections(mesh, self.turns)));
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
        let mut text = sections.to_json(self.turns);
        // A model with named parts also keeps which of them are on show.
        if !self.shown.is_empty() {
            if let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&text) {
                root["shown"] = serde_json::json!(self.shown);
                if let Some(mesh) = &self.mesh {
                    let by_name: serde_json::Map<String, serde_json::Value> =
                        mesh.parts.iter().zip(&self.shown).map(|(p, &on)| (p.name.clone(), on.into())).collect();
                    root["shown_parts"] = serde_json::Value::Object(by_name);
                }
                text = root.to_string();
            }
        }
        if let Err(e) = std::fs::write(&file, text) {
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

    /// Whether `doc` is the document this model is wearing.
    fn wears(&self, doc: DocId, active: Option<DocId>) -> bool {
        self.mesh.is_some() && self.texture.or(active) == Some(doc)
    }

    /// The map of sections over a `width` by `height` document, made on first use.
    fn section_map(&mut self, width: u32, height: u32) -> Option<&mut SectionMap> {
        let (mesh, sections) = (self.mesh.as_ref()?, self.sections.as_ref()?);
        let key = (self.generation, self.sections_generation, self.hidden_generation, width, height);
        if self.map.as_ref().is_none_or(|m| m.key != key) {
            // A huge texture is mapped at reduced size: the highlight does not need every pixel.
            let scale = (MAX_MAP_SIDE as f32 / width.max(height).max(1) as f32).min(1.0);
            let size = (((width as f32 * scale) as u32).max(1), ((height as f32 * scale) as u32).max(1));
            // Hidden parts claim no texels, so the canvas shows and picks only what is on the model.
            let owner = match &self.hidden {
                Some(hidden) => {
                    let mut shown = (**sections).clone();
                    for (s, _) in shown.of_triangle.iter_mut().zip(hidden.iter()).filter(|(_, h)| **h) {
                        *s = NO_SECTION;
                    }
                    section_map(mesh, &shown, size.0, size.1)
                }
                None => section_map(mesh, sections, size.0, size.1),
            };
            self.map = Some(SectionMap { key, size, owner, image: None });
        }
        self.map.as_mut()
    }

    /// The highlight for a canvas, if it shows the document this model is wearing.
    pub fn canvas_overlay(
        &mut self,
        ctx: &egui::Context,
        doc: DocId,
        active: Option<DocId>,
        size: (u32, u32),
    ) -> Option<CanvasOverlay> {
        if !self.wears(doc, active) {
            return None;
        }
        let highlight = self.selected.filter(|_| self.show_on_canvas);
        let dim = self.dim_unused;
        if highlight.is_none() && !dim {
            return None;
        }
        let map = self.section_map(size.0, size.1)?;
        let made_for = (highlight, dim);
        if map.image.as_ref().is_none_or(|(made, _)| *made != made_for) {
            let pixels = map
                .owner
                .iter()
                .map(|&o| match o {
                    _ if Some(o) == highlight => CANVAS_HIGHLIGHT,
                    NO_SECTION if dim => CANVAS_UNUSED,
                    _ => Color32::TRANSPARENT,
                })
                .collect();
            let image = egui::ColorImage::new([map.size.0 as usize, map.size.1 as usize], pixels);
            map.image = Some((made_for, ctx.load_texture("model_section", image, egui::TextureOptions::NEAREST)));
        }
        map.image.as_ref().map(|(_, image)| CanvasOverlay { image: image.clone() })
    }

    /// Selects the section painted at `at`, in document pixels, and returns its name. `None` if this document
    /// is not the model's texture or no section covers that pixel.
    pub fn pick_on_canvas(&mut self, doc: DocId, active: Option<DocId>, at: Pt, size: (u32, u32)) -> Option<String> {
        if !self.wears(doc, active) || at.x < 0.0 || at.y < 0.0 || at.x >= size.0 as f32 || at.y >= size.1 as f32 {
            return None;
        }
        let map = self.section_map(size.0, size.1)?;
        let x = ((at.x / size.0 as f32 * map.size.0 as f32) as usize).min(map.size.0 as usize - 1);
        let y = ((at.y / size.1 as f32 * map.size.1 as f32) as usize).min(map.size.1 as usize - 1);
        let section = map.owner[y * map.size.0 as usize + x];
        if section == NO_SECTION {
            return None;
        }
        self.selected = Some(section);
        self.sections.as_ref().and_then(|s| s.list.get(section as usize)).map(|s| s.name.clone())
    }

    /// Whether a canvas click can pick a section in `doc`.
    pub fn picks_on_canvas(&self, doc: DocId, active: Option<DocId>) -> bool {
        self.sections.is_some() && self.wears(doc, active)
    }
}

/// A model's first sections: its named parts when the file has them, else what the sectioner finds.
fn initial_sections(mesh: &Mesh, turns: u8) -> Sections {
    Sections::from_parts(mesh).unwrap_or_else(|| find_sections(mesh, up_in_mesh(turns), None))
}

pub fn is_model(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("glb"))
}

/// Where a model's sections are kept: `name.sections.json` beside it. The model file is never changed.
fn sidecar(model: &Path) -> PathBuf {
    model.with_extension("sections.json")
}

fn choose_file(state: &mut AppState) {
    if let Some(path) = rfd::FileDialog::new().set_title("Open 3D Model").add_filter("glTF binary", &["glb"]).ask_file()
    {
        crate::files::open_path(state, &path);
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

/// Puts the model's palette regions on the document it wears, so every stroke there snaps to the colors its
/// region can hold, and takes them off a document it no longer wears. Only an explicitly worn document of the size
/// the regions were written for takes them: an unrelated active document, or a resized one, would be snapped wrongly.
fn sync_palette_regions(state: &mut AppState) {
    let model = &state.model;
    let want = match (&model.palettes, model.texture, model.ignore_palettes) {
        (Some((size, regions)), Some(doc), false) => {
            state.doc(doc).filter(|d| [d.doc.width(), d.doc.height()] == *size).map(|_| (doc, regions.clone()))
        }
        _ => None,
    };
    if let Some(old) = state.model.regions_doc.filter(|old| want.as_ref().map(|w| w.0) != Some(*old)) {
        if let Some(entry) = state.doc_mut(old) {
            entry.doc.palette_regions = None;
        }
    }
    state.model.regions_doc = want.as_ref().map(|w| w.0);
    let Some((doc, regions)) = want else { return };
    let Some(entry) = state.doc_mut(doc) else { return };
    if entry.doc.palette_regions.as_ref().is_some_and(|r| Arc::ptr_eq(r, &regions)) {
        return;
    }
    // The regions' colors, row by row, become the document's palette when it has none, so the exact colors can be
    // picked; set on every history state, as the palette lock is, so it is not an undo step of its own.
    if entry.doc.state().palette.is_empty() {
        let colors = regions.iter().flat_map(|r| r.palette.colors.iter().copied()).collect();
        let palette = Palette::new("Texture palettes", colors);
        entry.doc.history.for_each_state_mut(|s| s.palette = palette.clone());
        entry.doc.state_mut().palette = palette;
    }
    entry.doc.palette_regions = Some(regions);
}

/// Where Extract Texture first saves a model's texture: `name_texture.ext` beside the model.
fn extracted_path(model: &Path, image: &EmbeddedImage) -> PathBuf {
    model.with_file_name(format!(
        "{}_texture.{}",
        model.file_stem().unwrap_or_default().to_string_lossy(),
        image.extension
    ))
}

/// A model that carries its own texture opens wearing it: the image an earlier Extract Texture saved beside it,
/// with whatever was painted on it since, or else the texture inside the file, extracted now. Without this the
/// model wears whichever unrelated document happens to be active.
fn wear_own_texture(state: &mut AppState) {
    let (Some(image), Some(path)) = (state.model.embedded.clone(), state.model.path.clone()) else { return };
    let saved = extracted_path(&path, &image);
    if saved.is_file() {
        if let Some(doc) = crate::files::open_path(state, &saved) {
            state.model.texture = Some(doc);
            state.model.embedded = None;
        }
    } else {
        extract_texture(state);
    }
}

/// Saves the texture stored inside the model as an image beside it, and opens that to paint on.
fn extract_texture(state: &mut AppState) {
    let (Some(image), Some(path)) = (state.model.embedded.clone(), state.model.path.clone()) else { return };
    // An earlier extraction that is still untouched is opened again, not copied.
    let first = extracted_path(&path, &image);
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
            match load(&done.model, false) {
                Ok(loaded) => state.model.show(loaded),
                Err(e) => {
                    state.model.error = Some(format!("Couldn't open {}: {e:#}", done.model.display()));
                    return;
                }
            }
            state.settings.push_recent(done.model.clone());
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
    if let Some(path) = state.model.finish_loading(ui.ctx()) {
        state.settings.push_recent(path);
        wear_own_texture(state);
    }
    sync_palette_regions(state);
    if state.model.unsaved && ui.memory(|m| m.focused().is_none()) {
        state.model.save_sections();
    }
    let Some(mesh) = state.model.mesh.clone() else {
        status_rows(ui, state);
        // The model worked on last, from the recent files, is one click away.
        let last = state.settings.general.recent_files.iter().find(|p| is_model(p) && p.is_file()).cloned();
        ui.add_space(ui.available_height() * 0.35);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Open a .glb model to see it here").weak());
            if let Some(last) = last {
                let name = last.file_name().unwrap_or_default().to_string_lossy().to_string();
                ui.add_space(6.0);
                if ui.button(format!("Reopen {name}")).on_hover_text(last.display().to_string()).clicked() {
                    crate::files::open_path(state, &last);
                }
            }
        });
        return;
    };

    // A wide, short panel (a window tiled to half a screen) puts the controls in a scrolling column beside the view;
    // stacked above it they would leave the view a strip.
    let (width, height) = (ui.available_width(), ui.available_height());
    if width > height * 1.4 && width > 400.0 {
        let column = (width * 0.5).clamp(240.0, 480.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(column, height), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                egui::ScrollArea::vertical().id_salt("model_controls").auto_shrink([false, false]).show(ui, |ui| {
                    controls(ui, state);
                });
            });
            view(ui, state, mesh);
        });
    } else {
        controls(ui, state);
        view(ui, state, mesh);
    }
}

/// The toolbar, then any error and the loading spinner.
fn status_rows(ui: &mut Ui, state: &mut AppState) {
    toolbar(ui, state);
    if let Some(error) = &state.model.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if let Some((path, _)) = &state.model.loading {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!("Opening {name}…"));
        });
    }
}

/// Everything above (or beside) the 3D view.
fn controls(ui: &mut Ui, state: &mut AppState) {
    status_rows(ui, state);
    parts_row(ui, state);
    colors_here(ui, state);
    section_row(ui, state);
}

/// The colors of the texture region the pointer was last over on the canvas, as swatches that pick the brush
/// color, and a warning when the brush color is not one of them: a stroke there takes the nearest one it holds.
fn colors_here(ui: &mut Ui, state: &mut AppState) {
    let model = &mut state.model;
    let (Some((_, regions)), Some(doc)) = (&model.palettes, model.regions_doc) else { return };
    if state.active_doc == Some(doc) {
        if let Some(p) = state.hover_doc_pos.filter(|p| p.x >= 0.0 && p.y >= 0.0) {
            if let Some(i) = regions.iter().position(|r| r.contains(p.x as u32, p.y as u32)) {
                model.color_region = Some(i);
            }
        }
    }
    let Some(region) = model.color_region.and_then(|i| regions.get(i)) else {
        ui.label(RichText::new("Point at the texture to see the colors its region can hold").weak());
        return;
    };
    let name = if region.palette.name.is_empty() { "this region" } else { region.palette.name.as_str() };
    ui.label(format!("Colors in {name}"));
    let fg = state.fg;
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
        for &c in &region.palette.colors {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::click());
            let painter = ui.painter();
            let weak = ui.visuals().weak_text_color();
            if c.a == 0 {
                // The see-through entry: an empty square, crossed.
                painter.rect_stroke(rect.shrink(1.0), 2.0, (1.0, weak), egui::StrokeKind::Inside);
                painter.line_segment([rect.left_bottom(), rect.right_top()], (1.0, weak));
            } else {
                painter.rect_filled(rect, 2.0, Color32::from_rgb(c.r, c.g, c.b));
            }
            if c == fg {
                let strong = ui.visuals().strong_text_color();
                painter.rect_stroke(rect.expand(1.5), 3.0, (2.0, strong), egui::StrokeKind::Outside);
            }
            let tip = if c.a == 0 {
                "See-through".to_string()
            } else {
                format!("#{:02X}{:02X}{:02X}: click to paint with it", c.r, c.g, c.b)
            };
            if response.on_hover_text(tip).clicked() && c.a > 0 {
                picked = Some(c);
            }
        }
    });
    if let Some(c) = picked {
        state.fg = c;
    } else if fg.a > 0 && !region.palette.colors.contains(&fg.with_alpha(255)) {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "The brush color is not one of these: paint here takes the nearest one",
        );
    }
}

/// The 3D view: orbiting, picking, and the drawing callback.
fn view(ui: &mut Ui, state: &mut AppState, mesh: Arc<Mesh>) {
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
            let hidden = model.hidden.as_deref().map(Vec::as_slice);
            let hit = pick_triangle(&mesh, &mul(&model.orbit.view_proj(aspect), &matrix), ndc, hidden);
            let picked = hit.map(|t| sections.of_triangle[t]);
            model.selected = if picked == model.selected { None } else { picked };
        }
    }

    let doc = model.texture.or(state.active_doc);
    let doc_size = doc.and_then(|id| state.doc(id)).map_or((1, 1), |d| (d.doc.width(), d.doc.height()));
    let ppp = ui.ctx().pixels_per_point();
    // A view laid out past the screen (the first frame of a newly docked panel can be: 4,648 by 11,690 seen)
    // renders no bigger than the screen, scaled down evenly; it is drawn stretched into its rect anyway.
    let screen = ui.ctx().content_rect().size() * ppp;
    let fit = (screen.x.max(screen.y) / (rect.width().max(rect.height()) * ppp)).min(1.0);
    let size =
        ((rect.width() * ppp * fit).round().max(1.0) as u32, (rect.height() * ppp * fit).round().max(1.0) as u32);
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
        texture: doc,
        size,
        image_size: doc_size,
        nearest: model.look.nearest,
        alpha_cutoff: model.look.alpha_cutoff,
        hidden: model.hidden.clone(),
        hidden_generation: model.hidden_generation,
        source: std::marker::PhantomData::<fn() -> crate::canvas::render::CanvasRenderer>,
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

/// The model's named parts: a choice for each slot (variants that replace one another, at most one shown)
/// and a checkbox for each part in no slot.
fn parts_row(ui: &mut Ui, state: &mut AppState) {
    let model = &mut state.model;
    let Some(mesh) = model.mesh.clone() else { return };
    let parts = &mesh.parts;
    if parts.is_empty() || model.shown.len() != parts.len() {
        return;
    }
    let mut slots: Vec<&str> = Vec::new();
    for slot in parts.iter().filter_map(|p| p.slot.as_deref()) {
        if !slots.contains(&slot) {
            slots.push(slot);
        }
    }
    let mut changed = false;
    // Collapsible, so the view gets the room back once the parts are chosen.
    egui::CollapsingHeader::new("Parts").id_salt("model_parts_header").default_open(true).show(ui, |ui| {
        // A slot a row: its name, then its choice, cut to the panel's width so a docked panel never grows past its edge.
        let combo_width = (ui.available_width() - 110.0).max(80.0);
        egui::Grid::new("model_parts").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            for slot in &slots {
                let members: Vec<usize> =
                    (0..parts.len()).filter(|&i| parts[i].slot.as_deref() == Some(*slot)).collect();
                let current = members.iter().copied().find(|&i| model.shown[i]);
                // A part named "Slot: name" reads as "name" under its slot.
                let label =
                    |i: usize| parts[i].name.strip_prefix(&format!("{slot}: ")).unwrap_or(&parts[i].name).to_string();
                let text = current.map_or_else(|| "None".to_string(), label);
                let mut choice = current;
                ui.label(*slot);
                crate::ui::fields::combo(ui, ("model_slot", *slot), combo_width, text, |ui| {
                    for &i in &members {
                        ui.selectable_value(&mut choice, Some(i), label(i))
                            .on_hover_text(format!("{} triangles", parts[i].triangles.len()));
                    }
                    ui.selectable_value(&mut choice, None, "None");
                });
                if choice != current {
                    for &i in &members {
                        model.shown[i] = Some(i) == choice;
                    }
                    changed = true;
                }
                ui.end_row();
            }
        });
        let loose: Vec<usize> = (0..parts.len()).filter(|&i| parts[i].slot.is_none()).collect();
        if !loose.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for i in loose {
                    changed |= ui.checkbox(&mut model.shown[i], &parts[i].name).changed();
                }
            });
        }
    });
    if changed {
        model.update_hidden();
        // Kept in the sidecar, so the model reopens as it was left.
        (model.edited, model.unsaved) = (true, true);
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
    let mut select_on_canvas = false;
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut model.tint_all, "Tint All");
        ui.checkbox(&mut model.show_on_canvas, "Show on Canvas").on_hover_text(
            "Tint the selected section on the texture. Ctrl+Alt+right-click the canvas to select the section painted there.",
        );
        ui.checkbox(&mut model.dim_unused, "Dim Unused Texels").on_hover_text(
            "Darken the texture where no shown part of the model reads it, so the canvas shows what painting there would change",
        );
        if model.palettes.is_some() {
            let mut keep = !model.ignore_palettes;
            if ui
                .checkbox(&mut keep, "Keep to Texture Palettes")
                .on_hover_text(
                    "Every stroke snaps to the colors its part of the texture can hold (a PlayStation texture page has 16), \
                     with soft edges made solid or see-through, so the texture stays one the console can draw. \
                     The colors are in the Palette panel.",
                )
                .changed()
            {
                model.ignore_palettes = !keep;
            }
        }
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
    // A section whose triangles are all in hidden parts is neither on the model nor on the canvas, so it is not listed.
    let on_show: Option<Vec<bool>> = model.hidden.as_ref().map(|hidden| {
        let mut any = vec![false; sections.list.len()];
        for (&s, _) in sections.of_triangle.iter().zip(hidden.iter()).filter(|(_, h)| !**h) {
            if let Some(a) = any.get_mut(s as usize) {
                *a = true;
            }
        }
        any
    });
    egui::ScrollArea::vertical().id_salt("model_sections").max_height(SECTION_LIST_HEIGHT).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            // A long name is cut at the panel's edge rather than widening the panel; the tooltip has it whole.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            for (i, section) in sections.list.iter().enumerate() {
                if on_show.as_ref().is_some_and(|shown| !shown[i]) {
                    continue;
                }
                let i = i as u32;
                let button = ui.selectable_label(model.selected == Some(i), &section.name).on_hover_text(format!(
                    "{}\n{} triangles. Right-click to rename or merge.",
                    section.name, section.triangles
                ));
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
        let field = crate::ui::fields::fixed(ui, 180.0, |ui| {
            egui::TextEdit::singleline(text).desired_width(172.0).clip_text(true).show(ui)
        });
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
    ui.horizontal_wrapped(|ui| {
        ui.label("Texture");
        let chosen = state.model.texture.filter(|id| state.doc(*id).is_some());
        let name =
            chosen.and_then(|id| state.doc(id)).map_or_else(|| "Active document".to_string(), |d| d.doc.title.clone());
        let docs: Vec<(DocId, String)> = state.docs.iter().map(|d| (d.id, d.doc.title.clone())).collect();
        crate::ui::fields::combo_default(ui, "model_texture", name, |ui| {
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
