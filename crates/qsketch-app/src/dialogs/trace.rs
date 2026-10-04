//! Image › Trace to Vector: turn the active layer's pixels into smooth vector
//! shapes (posterize first for clean, flat regions). Traced on a background
//! thread and previewed on the canvas; OK makes a vector smart object, which
//! Ctrl+T and Image Size redraw crisp at any size, and which File › Export
//! as SVG writes out as a real vector file.

use std::sync::{mpsc, Arc};

use egui::{Context, RichText, Ui};
use qsketch_core::smart::{SmartObject, SmartSource};
use qsketch_core::trace::{self, TraceColors, TraceCurves, TraceSettings, VectorArt};
use qsketch_core::{IRect, Layer, LayerId, Pt, Raster};

use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

/// What OK does with the traced shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceOutput {
    /// The layer becomes the vector art.
    Replace,
    /// A new vector layer above; the original stays as it was.
    NewLayer,
}

pub struct TraceDialog {
    pub doc: DocId,
    pub layer: LayerId,
    /// The pixels being traced (the layer through its mask), and where on
    /// the canvas they came from.
    source: Arc<Raster>,
    bounds: IRect,
    pub settings: TraceSettings,
    pub output: TraceOutput,
    /// The latest finished trace: the settings it used, the art, and how
    /// long it took (ms).
    art: Option<(TraceSettings, Arc<VectorArt>, f32)>,
    /// A trace running on a background thread.
    pending: Option<(TraceSettings, mpsc::Receiver<(VectorArt, f32)>)>,
    /// Settings of the trace the canvas currently shows.
    previewed: Option<TraceSettings>,
}

pub fn open(state: &mut AppState) {
    state.settle();
    let Some(e) = state.active() else { return };
    let s = e.doc.state();
    let l = s.active_layer();
    if !l.owns_pixels() {
        state.toasts.push(Level::Info, "Pick a layer with pixels to trace.");
        return;
    }
    let pixels = l.masked_raster();
    let Some(bounds) = pixels.full_bounds() else {
        state.toasts.push(Level::Info, "The active layer is empty: nothing to trace.");
        return;
    };
    state.dialogs.trace = Some(TraceDialog {
        doc: e.id,
        layer: l.props.id,
        source: Arc::new(pixels.crop(bounds)),
        bounds,
        settings: state.settings.trace.clone(),
        output: TraceOutput::Replace,
        art: None,
        pending: None,
        previewed: None,
    });
}

/// Collect a finished trace, start one for changed settings, and draw the
/// latest result onto the canvas preview.
fn pump(ctx: &Context, state: &mut AppState) {
    let Some(d) = state.dialogs.trace.as_mut() else { return };
    if let Some((s, rx)) = &d.pending {
        match rx.try_recv() {
            Ok((art, ms)) => {
                d.art = Some((s.clone(), Arc::new(art), ms));
                d.pending = None;
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(40)),
            Err(mpsc::TryRecvError::Disconnected) => d.pending = None,
        }
    }
    let current = d.art.as_ref().is_some_and(|a| a.0 == d.settings);
    if d.pending.is_none() && !current {
        let (tx, rx) = mpsc::channel();
        let (src, s) = (d.source.clone(), d.settings.clone());
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let art = trace::trace(&src, &s);
            let _ = tx.send((art, t.elapsed().as_secs_f32() * 1000.0));
        });
        d.pending = Some((d.settings.clone(), rx));
        ctx.request_repaint();
    }
    let Some((s, art, _)) = d.art.as_ref() else { return };
    if d.previewed.as_ref() == Some(s) {
        return;
    }
    d.previewed = Some(s.clone());
    let (doc, layer, b, art) = (d.doc, d.layer, d.bounds, art.clone());
    let Some(e) = state.doc_mut(doc) else { return };
    e.doc.revert_working();
    let st = e.doc.state_mut();
    let Some(li) = st.index_of(layer) else { return };
    let mut out = Raster::new(st.width, st.height);
    let (bx, by) = (b.x as f32, b.y as f32);
    art.render_into(&mut out, &|p: Pt| Pt::new(p.x + bx, p.y + by), 1.0);
    st.layers[li].raster = out;
    e.doc.mark_all_dirty();
}

/// Put the layer back as it was.
fn cancel(state: &mut AppState) {
    let Some(d) = state.dialogs.trace.take() else { return };
    state.settings.trace = d.settings;
    if let Some(e) = state.doc_mut(d.doc) {
        e.doc.revert_working();
        e.doc.mark_all_dirty();
    }
}

/// Make the traced art a vector smart object (replacing the layer or on a
/// new one above) as one history step.
fn apply(state: &mut AppState) {
    let Some(d) = state.dialogs.trace.take() else { return };
    state.settings.trace = d.settings.clone();
    let Some((_, art, _)) = d.art else { return };
    let shapes = art.shapes.len();
    let Some(e) = state.doc_mut(d.doc) else { return };
    e.doc.revert_working();
    let st = e.doc.state_mut();
    let Some(mut li) = st.index_of(d.layer) else { return };
    let placement = SmartObject::identity(d.bounds);
    match d.output {
        TraceOutput::Replace => {
            // The mask was traced through; it's part of the shapes now.
            st.layers[li].mask = None;
            st.layers[li].props.tilemap = None;
        }
        TraceOutput::NewLayer => {
            let id = st.new_id();
            let name = format!("{} (traced)", st.layers[li].props.name);
            let mut l = Layer::new(id, name, st.width, st.height);
            l.props.parent = st.layers[li].props.parent;
            st.insert_layer(l, li + 1);
            li += 1;
            st.active = li;
        }
    }
    qsketch_core::smart::install(st, li, SmartSource::Vector((*art).clone()), placement);
    e.doc.mark_all_dirty();
    e.doc.commit("Trace to Vector");
    state.toasts.push(
        Level::Info,
        format!(
            "Traced into {shapes} shape{}. It's a vector smart object: Ctrl+T or Image Size redraw it crisp at any size, File › Export as SVG saves it.",
            if shapes == 1 { "" } else { "s" }
        ),
    );
}

/// Write the art (placed where it was traced) to an SVG file.
fn export_svg(state: &mut AppState) {
    let Some(d) = state.dialogs.trace.as_ref() else { return };
    let Some((_, art, _)) = d.art.as_ref() else { return };
    let Some(e) = state.doc(d.doc) else { return };
    let (w, h) = (e.doc.width(), e.doc.height());
    let b = d.bounds;
    let body = art.svg_paths(&|p| p, Some([1.0, 0.0, 0.0, 1.0, b.x as f32, b.y as f32]), 1.0);
    let name =
        format!("{}.svg", e.doc.title.trim_end_matches('*').rsplit_once('.').map_or(e.doc.title.as_str(), |(n, _)| n));
    let Some(path) = rfd::FileDialog::new().add_filter("SVG", &["svg"]).set_file_name(name).save_file() else {
        return;
    };
    match std::fs::write(&path, trace::svg_document(w, h, &body)) {
        Ok(()) => state.status_msg = Some((format!("Exported {}", path.display()), std::time::Instant::now())),
        Err(e) => state.toasts.push(Level::Error, format!("Couldn't write the SVG: {e}")),
    }
}

/// Starting points for common pictures.
fn presets(ui: &mut Ui, s: &mut TraceSettings) {
    ui.horizontal_wrapped(|ui| {
        let mut preset = |ui: &mut Ui, label: &str, tip: &str, v: TraceSettings| {
            if ui.small_button(label).on_hover_text(tip).clicked() {
                *s = v;
            }
        };
        let d = TraceSettings::default();
        preset(ui, "Logo", "Flat colors, smooth curves, sharp corners kept.", d.clone());
        preset(
            ui,
            "Black & white",
            "Dark ink only, as black shapes (line art, stamps, signatures).",
            TraceSettings { colors: TraceColors::BlackWhite, ..d.clone() },
        );
        preset(
            ui,
            "Pixel art",
            "Follows the pixel edges exactly: crisp blocks at any size.",
            TraceSettings { curves: TraceCurves::Pixel, speckle: 1, gradient_step: 0, color_precision: 8, ..d.clone() },
        );
        preset(
            ui,
            "Detailed",
            "More colors and smaller details kept (illustrations, photos).",
            TraceSettings { color_precision: 8, gradient_step: 8, speckle: 2, corner_threshold: 90, ..d },
        );
    });
}

pub fn show(ctx: &Context, state: &mut AppState) {
    if state.dialogs.trace.is_none() {
        return;
    }
    pump(ctx, state);
    let palette = state.settings.ui.palette();
    let screen = ctx.content_rect();
    let (mut ok, mut cancel_it, mut svg, mut open) = (false, false, false, true);
    let d = state.dialogs.trace.as_mut().unwrap();
    // A floating window, not a modal: the canvas shows the trace live and
    // stays usable for zooming in on the edges.
    egui::Window::new("Trace to Vector")
        .id(egui::Id::new("trace_dialog"))
        .open(&mut open)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .auto_sized()
        .default_pos(egui::pos2(screen.right() - 372.0, screen.top() + 80.0))
        .show(ctx, |ui| {
            ui.set_width(320.0);
            if crate::ui::chrome::window_header(ui, &palette, "Trace to Vector") {
                cancel_it = true;
            }
            ui.label(
                RichText::new("Turns the layer into smooth vector shapes. Posterize first for clean, flat colors.")
                    .weak()
                    .small(),
            );
            ui.add_space(4.0);
            presets(ui, &mut d.settings);
            ui.add_space(6.0);
            let s = &mut d.settings;
            egui::Grid::new("trace_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Colors");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut s.colors, TraceColors::Color, "Color");
                    ui.selectable_value(&mut s.colors, TraceColors::BlackWhite, "Black & white");
                });
                ui.end_row();
                if s.colors == TraceColors::Color {
                    ui.label("Color detail").on_hover_text("Bits per channel kept: lower merges similar shades.");
                    ui.add(egui::Slider::new(&mut s.color_precision, 1..=8));
                    ui.end_row();
                    ui.label("Merge layers").on_hover_text(
                        "How different stacked color regions must be to stay apart: higher gives fewer, bigger shapes.",
                    );
                    ui.add(egui::Slider::new(&mut s.gradient_step, 0..=128));
                    ui.end_row();
                } else {
                    ui.label("Threshold").on_hover_text("Pixels darker than this become ink.");
                    ui.add(egui::Slider::new(&mut s.threshold, 1..=254));
                    ui.end_row();
                }
                ui.label("Curves");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut s.curves, TraceCurves::Smooth, "Smooth")
                        .on_hover_text("Bézier curves; sharp corners stay sharp.");
                    ui.selectable_value(&mut s.curves, TraceCurves::Polygon, "Polygon")
                        .on_hover_text("Straight segments.");
                    ui.selectable_value(&mut s.curves, TraceCurves::Pixel, "Pixel")
                        .on_hover_text("Exactly along the pixel edges.");
                });
                ui.end_row();
                if s.curves == TraceCurves::Smooth {
                    ui.label("Corners")
                        .on_hover_text("Turns sharper than this angle stay corners; higher rounds more.");
                    ui.add(egui::Slider::new(&mut s.corner_threshold, 0..=180).suffix("°"));
                    ui.end_row();
                }
                if s.curves != TraceCurves::Pixel {
                    ui.label("Smoothness").on_hover_text("Longer segments: smoother outlines with fewer points.");
                    ui.add(egui::Slider::new(&mut s.segment_length, 3.5..=10.0).fixed_decimals(1));
                    ui.end_row();
                }
                ui.label("Ignore specks").on_hover_text("Regions smaller than this across are dropped.");
                ui.add(egui::Slider::new(&mut s.speckle, 0..=32).suffix(" px"));
                ui.end_row();
                ui.label("Result");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut d.output, TraceOutput::Replace, "Replace layer");
                    ui.selectable_value(&mut d.output, TraceOutput::NewLayer, "New layer above");
                });
                ui.end_row();
            });
            ui.add_space(6.0);
            let busy = d.pending.is_some();
            ui.horizontal(|ui| {
                if busy {
                    ui.spinner();
                    ui.label(RichText::new("Tracing…").weak().small());
                } else if let Some((_, art, ms)) = &d.art {
                    ui.label(
                        RichText::new(format!(
                            "{} shapes · {} points · {:.0} ms",
                            art.shapes.len(),
                            art.point_count(),
                            ms
                        ))
                        .weak()
                        .small(),
                    );
                }
            });
            ui.add_space(8.0);
            let ready = !busy && d.art.as_ref().is_some_and(|a| a.0 == d.settings);
            ui.horizontal(|ui| {
                if ui.add_enabled(ready, egui::Button::new("Export SVG…")).clicked() {
                    svg = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(ready, egui::Button::new("OK")).clicked()
                        || (ready && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        ok = true;
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel_it = true;
                    }
                });
            });
        });
    if svg {
        export_svg(state);
    }
    if ok {
        apply(state);
    } else if cancel_it || !open {
        cancel(state);
    }
}
