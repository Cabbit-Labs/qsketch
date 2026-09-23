//! The Filter dialog: parameter controls with a live preview on the canvas,
//! plus the immediate-apply path used by parameterless filters and
//! Filter ▸ Last Filter.
//!
//! Previewing works on the document's working state: every parameter change
//! reverts the working state to the last committed snapshot (a cheap COW
//! clone) and re-runs the filter. OK commits that state; Cancel reverts.

use std::time::Instant;

use egui::{Context, Key, RichText, Ui};
use qsketch_core::filter::{self, DitherPattern, Filter, Levels, LevelsChannel, OffsetEdge, RadialMode};
use qsketch_core::{LayerId, Rgba8};

use crate::actions::Action;
use crate::state::{AppState, DocEntry, DocId};
use crate::ui::toasts::Level;

pub struct FilterDialog {
    pub doc: DocId,
    /// Every layer the filter applies to (the selected layers, groups
    /// expanded to their members). By id: inserting or deleting a layer
    /// while the dialog is open would shift indices onto other layers.
    pub layers: Vec<LayerId>,
    pub filter: Filter,
    /// The parameters currently rendered into the working state as a preview.
    pub applied: Option<Filter>,
    pub preview: bool,
    /// How long the last preview took; cheap filters re-render while a slider
    /// is still being dragged, expensive ones wait for the release.
    pub last_ms: f32,
    /// History entry the preview was rendered on top of. If something else
    /// commits underneath, the preview is part of that commit now and must
    /// not be re-applied on top of itself.
    pub base: u64,
    /// Luma / R / G / B histograms of the target pixels (Levels only),
    /// taken from the committed state when the dialog opened.
    pub hist: Option<Box<[[u32; 256]; 4]>>,
}

/// Default parameters for a Filter-menu action. Color parameters start from
/// the current foreground / background colors.
pub fn filter_for_action(state: &AppState, a: Action) -> Option<Filter> {
    default_filter(a, state.fg, state.bg)
}

/// `filter_for_action` with the colors passed in.
pub fn default_filter(a: Action, fg: Rgba8, bg: Rgba8) -> Option<Filter> {
    Some(match a {
        Action::HueSaturation => Filter::HueSaturation { hue: 0.0, saturation: 0.0, lightness: 0.0, colorize: false },
        Action::BrightnessContrast => Filter::BrightnessContrast { brightness: 0.0, contrast: 0.0 },
        Action::Levels => Filter::Levels(Levels::default()),
        Action::FilterGaussianBlur => Filter::GaussianBlur { radius: 5.0 },
        Action::FilterBoxBlur => Filter::BoxBlur { radius: 3 },
        Action::FilterMotionBlur => Filter::MotionBlur { angle: 0.0, distance: 20.0 },
        Action::FilterRadialBlur => Filter::RadialBlur { amount: 20.0, mode: RadialMode::Spin },
        Action::FilterRipple => Filter::Ripple { amplitude: 6.0, wavelength: 24.0 },
        Action::FilterWave => Filter::Wave { wavelength: 60.0, amplitude: 10.0, angle: 0.0 },
        Action::FilterTwirl => Filter::Twirl { angle: 90.0 },
        Action::FilterSpherize => Filter::Spherize { amount: 50.0 },
        Action::FilterZigZag => Filter::ZigZag { amount: 10.0, ridges: 5 },
        Action::FilterPolarCoordinates => Filter::PolarCoordinates { to_polar: true },
        Action::FilterAddNoise => Filter::AddNoise { amount: 0.15, monochrome: false, gaussian: true, seed: 1 },
        Action::FilterMedian => Filter::Median { radius: 2 },
        // Threshold is how different a pixel must be before it is replaced,
        // so a high default makes the filter look like it does nothing.
        Action::FilterDustAndScratches => Filter::DustAndScratches { radius: 2, threshold: 8 },
        Action::FilterMosaic => Filter::Mosaic { cell: 8 },
        Action::FilterCrystallize => Filter::Crystallize { cell: 16, seed: 1 },
        Action::FilterFragment => Filter::Fragment,
        Action::FilterColorHalftone => Filter::ColorHalftone { radius: 6.0 },
        Action::FilterPointillize => Filter::Pointillize { cell: 12, seed: 1, background: bg },
        Action::FilterClouds => Filter::Clouds { scale: 128.0, seed: 1, color_a: fg, color_b: bg },
        Action::FilterDifferenceClouds => Filter::DifferenceClouds { scale: 128.0, seed: 1, color_a: fg, color_b: bg },
        Action::FilterSharpen => Filter::Sharpen,
        Action::FilterSharpenMore => Filter::SharpenMore,
        Action::FilterUnsharpMask => Filter::UnsharpMask { amount: 1.0, radius: 2.0, threshold: 0 },
        Action::FilterFindEdges => Filter::FindEdges,
        Action::FilterEmboss => Filter::Emboss { angle: 135.0, height: 3.0, amount: 1.0 },
        Action::FilterSolarize => Filter::Solarize,
        Action::FilterDiffuse => Filter::Diffuse { distance: 3, seed: 1 },
        Action::FilterOilPaint => Filter::OilPaint { radius: 4, levels: 20 },
        Action::FilterWind => Filter::Wind { strength: 12, from_left: true, seed: 1 },
        Action::FilterHighPass => Filter::HighPass { radius: 10.0 },
        Action::FilterMaximum => Filter::Maximum { radius: 2 },
        Action::FilterMinimum => Filter::Minimum { radius: 2 },
        Action::FilterOffset => Filter::Offset { dx: 0, dy: 0, edge: OffsetEdge::Wrap },
        Action::FilterChromaticAberration => Filter::ChromaticAberration { amount: 4.0, radial: true, angle: 0.0 },
        Action::FilterDither => Filter::Dither { levels: 4, pattern: DitherPattern::Bayer4 },
        Action::FilterPixelSort => Filter::PixelSort { threshold: 0.5, vertical: false, reverse: false },
        Action::FilterScanlines => Filter::Scanlines { spacing: 4, darkness: 0.5, rgb_mask: false },
        Action::FilterVignette => Filter::Vignette { amount: 0.7, softness: 0.6, color: Rgba8::BLACK },
        Action::FilterGlow => Filter::Glow { radius: 12.0, intensity: 1.0, threshold: 0.6 },
        Action::FilterKaleidoscope => Filter::Kaleidoscope { segments: 6, angle: 0.0 },
        Action::FilterOutline => Filter::Outline { width: 4, color: fg, inside: false },
        Action::FilterGlitch => Filter::Glitch { amount: 24.0, seed: 1 },
        Action::FilterPencilSketch => Filter::PencilSketch { radius: 6.0, strength: 1.0 },
        _ => return None,
    })
}

/// Open the dialog for a Filter-menu action (or apply it right away when it
/// has no parameters). Remembered parameters win over the defaults, except
/// that the cloud and pointillize colors always follow the current
/// foreground / background, as in Photoshop.
pub fn open(state: &mut AppState, action: Action) {
    let Some(mut f) = filter_for_action(state, action) else { return };
    if let Some(m) = state.filter_memory.get(f.id()) {
        f = m.clone();
    }
    match &mut f {
        Filter::Clouds { color_a, color_b, .. } | Filter::DifferenceClouds { color_a, color_b, .. } => {
            *color_a = state.fg;
            *color_b = state.bg;
        }
        Filter::Pointillize { background, .. } => *background = state.bg,
        _ => {}
    }
    open_with(state, f);
}

/// Open the dialog pre-filled with `f` (applies immediately when `f` has no
/// parameters).
pub fn open_with(state: &mut AppState, f: Filter) {
    state.settle();
    let Some(target) = target_layer(state) else { return };
    if !f.has_params() {
        apply_now(state, f);
        return;
    }
    if let Some(prev) = state.dialogs.filter.take() {
        revert(state, &prev);
    }
    let hist = match &f {
        Filter::Levels(_) => state.doc(target.0).map(|e| histogram(e, &target.1)),
        _ => None,
    };
    let base = state.doc(target.0).map(|e| e.doc.history.current_id()).unwrap_or(0);
    state.dialogs.filter = Some(FilterDialog {
        doc: target.0,
        layers: target.1,
        filter: f,
        applied: None,
        preview: true,
        last_ms: 0.0,
        base,
        hist,
    });
}

/// Resolve target layer ids to current indices, dropping any that are gone.
fn indices(e: &DocEntry, ids: &[LayerId]) -> Vec<usize> {
    let s = e.doc.state();
    ids.iter().filter_map(|id| s.index_of(*id)).collect()
}

/// Luma / R / G / B histograms over the target layers, within the
/// selection's bounds, ignoring fully transparent pixels.
fn histogram(e: &DocEntry, layers: &[LayerId]) -> Box<[[u32; 256]; 4]> {
    let s = e.doc.state();
    let mut h = Box::new([[0u32; 256]; 4]);
    let full = qsketch_core::IRect::new(0, 0, s.width as i32, s.height as i32);
    let rect = s.selection_mask().map(|m| m.bounds().intersect(&full)).unwrap_or(full);
    for li in indices(e, layers) {
        let Some(l) = s.layers.get(li) else { continue };
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                let p = l.raster.get_pixel(x, y);
                if p.a == 0 {
                    continue;
                }
                let luma = (0.299 * p.r as f32 + 0.587 * p.g as f32 + 0.114 * p.b as f32 + 0.5) as usize;
                h[0][luma.min(255)] += 1;
                h[1][p.r as usize] += 1;
                h[2][p.g as usize] += 1;
                h[3][p.b as usize] += 1;
            }
        }
    }
    h
}

/// Apply `f` to the selected layers and commit it as one history step.
pub fn apply_now(state: &mut AppState, f: Filter) {
    state.cancel_session();
    crate::tools::floating::commit(state);
    let Some((id, layers)) = target_layer(state) else { return };
    if let Some(e) = state.doc_mut(id) {
        let r = run(e, &layers, &f);
        if !r.is_empty() {
            e.doc.commit(f.name());
        }
    }
    remember(state, f);
}

/// Filter ▸ Last Filter: re-apply the previous filter with the same settings.
pub fn repeat_last(state: &mut AppState) {
    match state.last_filter.clone() {
        Some(f) => apply_now(state, f),
        None => state.toasts.push(Level::Info, "No filter has been applied yet."),
    }
}

/// Filter ▸ Last Filter Settings…: reopen the previous filter's dialog.
pub fn reopen_last(state: &mut AppState) {
    match state.last_filter.clone() {
        Some(f) => open_with(state, f),
        None => state.toasts.push(Level::Info, "No filter has been applied yet."),
    }
}

/// The active document and the editable raster layers among the selected
/// ones (groups expanded), if there are any.
fn target_layer(state: &mut AppState) -> Option<(DocId, Vec<LayerId>)> {
    let e = state.active()?;
    let (id, layers) = (e.id, e.target_layer_ids());
    if layers.is_empty() {
        let msg = if e.selected_ids().len() > 1 || e.doc.state().active_layer().is_group() {
            "None of the selected layers can be edited (locked, hidden or empty groups)."
        } else {
            "The active layer is locked or hidden."
        };
        state.toasts.push(Level::Info, msg);
        return None;
    }
    Some((id, layers))
}

fn remember(state: &mut AppState, f: Filter) {
    state.filter_memory.insert(f.id(), f.clone());
    state.last_filter = Some(f);
}

/// Run a filter on every target layer of the working state and mark the
/// result dirty. Returns the union of the dirty rects.
fn run(e: &mut DocEntry, layers: &[LayerId], f: &Filter) -> qsketch_core::IRect {
    let mut acc = qsketch_core::IRect::EMPTY;
    for li in indices(e, layers) {
        let r = filter::apply_filter(e.doc.state_mut(), li, f);
        e.doc.mark_dirty_rect(r);
        acc = acc.union(&r);
    }
    acc
}

/// Throw away a rendered preview.
fn revert(state: &mut AppState, d: &FilterDialog) {
    if d.applied.is_none() {
        return;
    }
    if let Some(e) = state.doc_mut(d.doc) {
        e.doc.revert_working();
    }
}

pub fn show(ctx: &Context, state: &mut AppState) {
    // Take the dialog out of the state so the preview code can borrow the
    // document mutably; it goes back in unless it was closed.
    let Some(mut d) = state.dialogs.filter.take() else { return };
    if state.doc(d.doc).is_none() {
        return;
    }
    // Follow the Layers panel: picking a different layer while the dialog is
    // open re-targets it (and re-reads the histogram), after taking the
    // preview back off the layers it was rendered into.
    // Something else committed under the preview (a stroke, a layer change):
    // the preview is part of that commit now, so start again from it rather
    // than filtering an already-filtered image.
    let now_base = state.doc(d.doc).map(|e| e.doc.history.current_id()).unwrap_or(d.base);
    if now_base != d.base {
        d.base = now_base;
        d.applied = None;
        if matches!(d.filter, Filter::Levels(_)) {
            d.hist = state.doc(d.doc).map(|e| histogram(e, &d.layers));
        }
    }
    let targets: Vec<LayerId> = state.doc(d.doc).map(|e| e.target_layer_ids()).unwrap_or_default();
    if !targets.is_empty() && targets != d.layers {
        if d.applied.is_some() {
            if let Some(e) = state.doc_mut(d.doc) {
                e.doc.revert_working();
            }
            d.applied = None;
        }
        d.layers = targets;
        if matches!(d.filter, Filter::Levels(_)) {
            d.hist = state.doc(d.doc).map(|e| histogram(e, &d.layers));
        }
    }
    let (mut ok, mut cancel, mut open) = (false, false, true);
    let screen = ctx.content_rect();
    egui::Window::new(d.filter.name())
        .id(egui::Id::new("filter_dialog"))
        .open(&mut open)
        .collapsible(false)
        .auto_sized()
        .default_pos(egui::pos2(screen.right() - 372.0, screen.top() + 80.0))
        .show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(RichText::new(d.filter.describe()).weak().small());
            ui.add_space(6.0);
            if let Filter::Levels(l) = &mut d.filter {
                levels_ui(ui, l, d.hist.as_deref());
            } else {
                params_ui(ui, &mut d.filter);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.preview, "Preview");
                if d.last_ms > 0.0 {
                    ui.label(RichText::new(format!("{:.0} ms", d.last_ms)).weak().small());
                }
            });
            let scope = match d.layers.len() {
                1 => "Applies to the active layer (within the selection).".to_string(),
                n => format!("Applies to {n} selected layers (within the selection)."),
            };
            ui.label(RichText::new(scope).weak().small());
            ui.add_space(8.0);
            // Inside `horizontal` so the right-aligned row keeps a one-row
            // height; on its own it would stretch the auto-sized window.
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("OK").clicked() {
                        ok = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        });
    if ctx.input(|i| i.key_pressed(Key::Enter)) {
        ok = true;
    }
    if !open || ctx.input(|i| i.key_pressed(Key::Escape)) {
        cancel = true;
    }

    if !ok && !cancel {
        let dragging = ctx.input(|i| i.pointer.primary_down());
        let stale = d.preview && d.applied.as_ref() != Some(&d.filter);
        if stale && (!dragging || d.last_ms < 80.0) {
            if let Some(e) = state.doc_mut(d.doc) {
                if d.applied.is_some() {
                    e.doc.revert_working();
                }
                let t = Instant::now();
                run(e, &d.layers, &d.filter);
                d.last_ms = t.elapsed().as_secs_f32() * 1000.0;
                d.applied = Some(d.filter.clone());
            }
        }
        if !d.preview && d.applied.is_some() {
            revert(state, &d);
            d.applied = None;
        }
        state.dialogs.filter = Some(d);
        return;
    }

    if ok {
        if let Some(e) = state.doc_mut(d.doc) {
            if d.applied.as_ref() != Some(&d.filter) {
                if d.applied.is_some() {
                    e.doc.revert_working();
                }
                run(e, &d.layers, &d.filter);
            }
            e.doc.commit(d.filter.name());
        }
        remember(state, d.filter);
    } else {
        revert(state, &d);
    }
}

// ---------------------------------------------------------------------------
// Parameter controls

fn slider<T: egui::emath::Numeric>(
    ui: &mut Ui,
    v: &mut T,
    range: std::ops::RangeInclusive<T>,
    text: &str,
    suffix: &str,
) {
    ui.add(egui::Slider::new(v, range).text(text).suffix(suffix));
}

fn log_slider(ui: &mut Ui, v: &mut f32, range: std::ops::RangeInclusive<f32>, text: &str, suffix: &str) {
    ui.add(egui::Slider::new(v, range).text(text).suffix(suffix).logarithmic(true));
}

fn seed_ui(ui: &mut Ui, seed: &mut u32) {
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(seed).speed(1));
        ui.label("Seed");
        if crate::ui::widgets::small_button(ui, "Randomize").clicked() {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(1);
            *seed = nanos ^ seed.wrapping_mul(0x2c1b_3c6d).wrapping_add(0x9e37_79b9);
        }
    });
}

fn color_ui(ui: &mut Ui, label: &str, c: &mut Rgba8) {
    ui.horizontal(|ui| {
        let mut c32 = egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a);
        if egui::color_picker::color_edit_button_srgba(ui, &mut c32, egui::color_picker::Alpha::OnlyBlend).changed() {
            let [r, g, b, a] = c32.to_srgba_unmultiplied();
            *c = Rgba8::new(r, g, b, a);
        }
        ui.label(label);
    });
}

/// Photoshop-style Levels: channel picker, histogram, input black / gamma /
/// white, output black / white, Auto and Reset.
fn levels_ui(ui: &mut Ui, l: &mut Levels, hist: Option<&[[u32; 256]; 4]>) {
    ui.horizontal(|ui| {
        ui.label("Channel");
        for (c, name) in [
            (LevelsChannel::Rgb, "RGB"),
            (LevelsChannel::Red, "Red"),
            (LevelsChannel::Green, "Green"),
            (LevelsChannel::Blue, "Blue"),
        ] {
            ui.selectable_value(&mut l.channel, c, name);
        }
    });
    ui.add_space(4.0);
    // Histogram of the channel being edited.
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 96.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    if let Some(h) = hist {
        let bins = &h[match l.channel {
            LevelsChannel::Rgb => 0,
            LevelsChannel::Red => 1,
            LevelsChannel::Green => 2,
            LevelsChannel::Blue => 3,
        }];
        // Scale to the 99th-percentile bin so a few spikes don't flatten the rest.
        let mut sorted: Vec<u32> = bins.to_vec();
        sorted.sort_unstable();
        let top = sorted[253].max(1) as f32;
        let col = match l.channel {
            LevelsChannel::Rgb => ui.visuals().text_color(),
            LevelsChannel::Red => egui::Color32::from_rgb(230, 80, 80),
            LevelsChannel::Green => egui::Color32::from_rgb(80, 200, 90),
            LevelsChannel::Blue => egui::Color32::from_rgb(90, 130, 240),
        };
        let inner = rect.shrink(2.0);
        let bw = inner.width() / 256.0;
        for (i, &n) in bins.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let hgt = (n as f32 / top).min(1.0) * inner.height();
            let x0 = inner.left() + i as f32 * bw;
            p.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x0, inner.bottom() - hgt),
                    egui::pos2(x0 + bw.max(1.0), inner.bottom()),
                ),
                0.0,
                col,
            );
        }
        // Input black / white markers.
        let cur = l.curve(l.channel);
        for (v, c) in [(cur.in_black, egui::Color32::BLACK), (cur.in_white, egui::Color32::WHITE)] {
            let x = inner.left() + v / 255.0 * inner.width();
            p.line_segment([egui::pos2(x, inner.top()), egui::pos2(x, inner.bottom())], egui::Stroke::new(1.0, c));
        }
    } else {
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "No pixels to sample",
            egui::FontId::proportional(12.0),
            ui.visuals().weak_text_color(),
        );
    }
    ui.add_space(4.0);
    let cur = l.curve_mut(l.channel);
    ui.label(RichText::new("Input levels").weak().small());
    slider(ui, &mut cur.in_black, 0.0..=253.0, "Black", "");
    cur.in_black = cur.in_black.min(cur.in_white - 2.0).max(0.0);
    ui.add(egui::Slider::new(&mut cur.in_gamma, 0.1..=9.99).text("Gamma").logarithmic(true).fixed_decimals(2));
    slider(ui, &mut cur.in_white, 2.0..=255.0, "White", "");
    cur.in_white = cur.in_white.max(cur.in_black + 2.0).min(255.0);
    ui.label(RichText::new("Output levels").weak().small());
    slider(ui, &mut cur.out_black, 0.0..=255.0, "Black", "");
    slider(ui, &mut cur.out_white, 0.0..=255.0, "White", "");
    ui.horizontal(|ui| {
        if crate::ui::widgets::small_button(ui, "Auto")
            .on_hover_text("Stretch each channel so 0.1% of its pixels clip at each end")
            .clicked()
        {
            if let Some(h) = hist {
                *l = Levels::default();
                for (ci, curve) in [(1usize, &mut l.red), (2, &mut l.green), (3, &mut l.blue)] {
                    let bins = &h[ci];
                    let total: u64 = bins.iter().map(|&n| n as u64).sum();
                    if total == 0 {
                        continue;
                    }
                    let clip = (total as f64 * 0.001).max(1.0) as u64;
                    let mut acc = 0u64;
                    let lo = bins.iter().position(|&n| {
                        acc += n as u64;
                        acc > clip
                    });
                    acc = 0;
                    let hi = bins.iter().rposition(|&n| {
                        acc += n as u64;
                        acc > clip
                    });
                    if let (Some(lo), Some(hi)) = (lo, hi) {
                        if hi > lo + 1 {
                            curve.in_black = lo as f32;
                            curve.in_white = hi as f32;
                        }
                    }
                }
            }
        }
        if crate::ui::widgets::small_button(ui, "Reset").clicked() {
            let ch = l.channel;
            *l = Levels::default();
            l.channel = ch;
        }
    });
}

fn params_ui(ui: &mut Ui, f: &mut Filter) {
    match f {
        Filter::BrightnessContrast { brightness, contrast } => {
            slider(ui, brightness, -100.0..=100.0, "Brightness", "");
            slider(ui, contrast, -100.0..=100.0, "Contrast", "");
        }
        Filter::Levels(_) => {}
        Filter::HueSaturation { hue, saturation, lightness, colorize } => {
            if *colorize {
                slider(ui, hue, 0.0..=360.0, "Hue", "°");
                slider(ui, saturation, 0.0..=100.0, "Saturation", "");
            } else {
                slider(ui, hue, -180.0..=180.0, "Hue", "°");
                slider(ui, saturation, -100.0..=100.0, "Saturation", "");
            }
            slider(ui, lightness, -100.0..=100.0, "Lightness", "");
            ui.horizontal(|ui| {
                if ui.checkbox(colorize, "Colorize").changed() {
                    // Switch between shift and absolute ranges sensibly.
                    if *colorize {
                        *hue = hue.rem_euclid(360.0);
                        *saturation = saturation.abs().clamp(25.0, 100.0);
                    } else {
                        *hue = 0.0;
                        *saturation = 0.0;
                    }
                }
                if crate::ui::widgets::small_button(ui, "Reset").clicked() {
                    *hue = 0.0;
                    *saturation = if *colorize { 25.0 } else { 0.0 };
                    *lightness = 0.0;
                }
            });
        }
        Filter::GaussianBlur { radius } => log_slider(ui, radius, 0.1..=250.0, "Radius", " px"),
        Filter::BoxBlur { radius } => slider(ui, radius, 1..=100, "Radius", " px"),
        Filter::MotionBlur { angle, distance } => {
            slider(ui, angle, -180.0..=180.0, "Angle", "°");
            log_slider(ui, distance, 1.0..=256.0, "Distance", " px");
        }
        Filter::RadialBlur { amount, mode } => {
            slider(ui, amount, 1.0..=100.0, "Amount", "");
            ui.horizontal(|ui| {
                ui.selectable_value(mode, RadialMode::Spin, "Spin");
                ui.selectable_value(mode, RadialMode::Zoom, "Zoom");
            });
        }
        Filter::Ripple { amplitude, wavelength } => {
            slider(ui, amplitude, -100.0..=100.0, "Amount", " px");
            log_slider(ui, wavelength, 2.0..=400.0, "Wavelength", " px");
        }
        Filter::Wave { wavelength, amplitude, angle } => {
            log_slider(ui, wavelength, 2.0..=600.0, "Wavelength", " px");
            slider(ui, amplitude, 0.0..=200.0, "Amplitude", " px");
            slider(ui, angle, -180.0..=180.0, "Angle", "°");
        }
        Filter::Twirl { angle } => slider(ui, angle, -999.0..=999.0, "Angle", "°"),
        Filter::Spherize { amount } => slider(ui, amount, -100.0..=100.0, "Amount", "%"),
        Filter::ZigZag { amount, ridges } => {
            slider(ui, amount, -100.0..=100.0, "Amount", " px");
            slider(ui, ridges, 1..=20, "Ridges", "");
        }
        Filter::PolarCoordinates { to_polar } => {
            ui.horizontal(|ui| {
                ui.selectable_value(to_polar, true, "Rectangular to Polar");
                ui.selectable_value(to_polar, false, "Polar to Rectangular");
            });
        }
        Filter::AddNoise { amount, monochrome, gaussian, seed } => {
            slider(ui, amount, 0.0..=1.0, "Amount", "");
            ui.horizontal(|ui| {
                ui.selectable_value(gaussian, false, "Uniform");
                ui.selectable_value(gaussian, true, "Gaussian");
            });
            ui.checkbox(monochrome, "Monochromatic");
            seed_ui(ui, seed);
        }
        Filter::Median { radius } => slider(ui, radius, 1..=filter::noise::MAX_MEDIAN_RADIUS, "Radius", " px"),
        Filter::DustAndScratches { radius, threshold } => {
            slider(ui, radius, 1..=filter::noise::MAX_MEDIAN_RADIUS, "Radius", " px");
            slider(ui, threshold, 0..=255, "Threshold", " levels");
        }
        Filter::Mosaic { cell } => slider(ui, cell, 2..=200, "Cell size", " px"),
        Filter::Crystallize { cell, seed } => {
            slider(ui, cell, 3..=300, "Cell size", " px");
            seed_ui(ui, seed);
        }
        Filter::ColorHalftone { radius } => slider(ui, radius, 2.0..=64.0, "Max radius", " px"),
        Filter::Pointillize { cell, seed, background } => {
            slider(ui, cell, 3..=300, "Cell size", " px");
            color_ui(ui, "Background", background);
            seed_ui(ui, seed);
        }
        Filter::Clouds { scale, seed, color_a, color_b }
        | Filter::DifferenceClouds { scale, seed, color_a, color_b } => {
            log_slider(ui, scale, 4.0..=2048.0, "Scale", " px");
            color_ui(ui, "Color A", color_a);
            color_ui(ui, "Color B", color_b);
            seed_ui(ui, seed);
        }
        Filter::UnsharpMask { amount, radius, threshold } => {
            slider(ui, amount, 0.0..=5.0, "Amount", "×");
            log_slider(ui, radius, 0.1..=250.0, "Radius", " px");
            slider(ui, threshold, 0..=255, "Threshold", " levels");
        }
        Filter::Emboss { angle, height, amount } => {
            slider(ui, angle, -180.0..=180.0, "Angle", "°");
            slider(ui, height, 1.0..=20.0, "Height", " px");
            slider(ui, amount, 0.1..=5.0, "Amount", "×");
        }
        Filter::Diffuse { distance, seed } => {
            slider(ui, distance, 1..=50, "Distance", " px");
            seed_ui(ui, seed);
        }
        Filter::OilPaint { radius, levels } => {
            slider(ui, radius, 1..=12, "Brush size", " px");
            slider(ui, levels, 2..=64, "Smoothness", " levels");
        }
        Filter::Wind { strength, from_left, seed } => {
            slider(ui, strength, 1..=100, "Strength", " px");
            ui.horizontal(|ui| {
                ui.selectable_value(from_left, true, "From the left");
                ui.selectable_value(from_left, false, "From the right");
            });
            seed_ui(ui, seed);
        }
        Filter::HighPass { radius } => log_slider(ui, radius, 0.1..=250.0, "Radius", " px"),
        Filter::Maximum { radius } | Filter::Minimum { radius } => slider(ui, radius, 1..=100, "Radius", " px"),
        Filter::Offset { dx, dy, edge } => {
            slider(ui, dx, -4096..=4096, "Horizontal", " px");
            slider(ui, dy, -4096..=4096, "Vertical", " px");
            ui.label(RichText::new("Undefined areas").weak().small());
            ui.horizontal(|ui| {
                ui.selectable_value(edge, OffsetEdge::Wrap, "Wrap around");
                ui.selectable_value(edge, OffsetEdge::Transparent, "Transparent");
                ui.selectable_value(edge, OffsetEdge::Repeat, "Repeat edge");
            });
        }
        Filter::ChromaticAberration { amount, radial, angle } => {
            slider(ui, amount, -50.0..=50.0, "Amount", " px");
            ui.horizontal(|ui| {
                ui.selectable_value(radial, true, "Radial");
                ui.selectable_value(radial, false, "Directional");
            });
            if !*radial {
                slider(ui, angle, -180.0..=180.0, "Angle", "°");
            }
        }
        Filter::Dither { levels, pattern } => {
            slider(ui, levels, 2..=32, "Levels per channel", "");
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(pattern, DitherPattern::None, "None");
                ui.selectable_value(pattern, DitherPattern::Bayer2, "Bayer 2×2");
                ui.selectable_value(pattern, DitherPattern::Bayer4, "Bayer 4×4");
                ui.selectable_value(pattern, DitherPattern::Bayer8, "Bayer 8×8");
                ui.selectable_value(pattern, DitherPattern::Noise, "Noise");
            });
        }
        Filter::PixelSort { threshold, vertical, reverse } => {
            slider(ui, threshold, 0.0..=1.0, "Brightness threshold", "");
            ui.horizontal(|ui| {
                ui.selectable_value(vertical, false, "Horizontal");
                ui.selectable_value(vertical, true, "Vertical");
            });
            ui.checkbox(reverse, "Brightest first");
        }
        Filter::Scanlines { spacing, darkness, rgb_mask } => {
            slider(ui, spacing, 2..=32, "Spacing", " px");
            slider(ui, darkness, 0.0..=1.0, "Darkness", "");
            ui.checkbox(rgb_mask, "RGB shadow mask");
        }
        Filter::Vignette { amount, softness, color } => {
            slider(ui, amount, 0.0..=1.0, "Amount", "");
            slider(ui, softness, 0.0..=1.0, "Softness", "");
            color_ui(ui, "Color", color);
        }
        Filter::Glow { radius, intensity, threshold } => {
            log_slider(ui, radius, 1.0..=250.0, "Radius", " px");
            slider(ui, intensity, 0.0..=4.0, "Intensity", "×");
            slider(ui, threshold, 0.0..=0.99, "Threshold", "");
        }
        Filter::Kaleidoscope { segments, angle } => {
            slider(ui, segments, 2..=32, "Segments", "");
            slider(ui, angle, -180.0..=180.0, "Angle", "°");
        }
        Filter::Outline { width, color, inside } => {
            slider(ui, width, 1..=64, "Width", " px");
            color_ui(ui, "Color", color);
            ui.horizontal(|ui| {
                ui.selectable_value(inside, false, "Outside");
                ui.selectable_value(inside, true, "Inside");
            });
        }
        Filter::Glitch { amount, seed } => {
            slider(ui, amount, 1.0..=200.0, "Amount", " px");
            seed_ui(ui, seed);
        }
        Filter::PencilSketch { radius, strength } => {
            log_slider(ui, radius, 0.5..=100.0, "Softness", " px");
            slider(ui, strength, 0.2..=4.0, "Darkness", "×");
        }
        Filter::Fragment | Filter::Sharpen | Filter::SharpenMore | Filter::FindEdges | Filter::Solarize => {
            ui.label(RichText::new("This filter has no settings.").weak());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qsketch_core::DocState;

    /// Every filter with default parameters must visibly change a test
    /// image (except the ones whose defaults are the identity on purpose),
    /// and no two filters may produce the same output.
    #[test]
    fn every_filter_changes_the_image_distinctly() {
        let (w, h) = (48u32, 48u32);
        let mut doc = DocState::new(w, h, None);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let checker = ((x / 6 + y / 6) % 2) as u8;
                let c = Rgba8::new(
                    (x * 5) as u8,
                    (y * 5) as u8,
                    if checker == 1 { 200 } else { 40 },
                    if x < 40 { 255 } else { 90 },
                );
                doc.layers[0].raster.set_pixel(x, y, c);
            }
        }
        // Speckles, so the noise-removal filters have something to remove.
        for k in 0..40i32 {
            let (x, y) = ((k * 7 + 3) % 47, (k * 13 + 5) % 47);
            doc.layers[0].raster.set_pixel(x, y, Rgba8::new(255, 255, 255, 255));
        }
        let base = doc.layers[0].raster.to_rgba();
        let fg = Rgba8::new(220, 30, 60, 255);
        let bg = Rgba8::new(20, 200, 120, 255);
        // Identity by design at their defaults: sliders start centered.
        let identity_ok = ["offset", "brightness_contrast", "levels", "hue_saturation"];
        let mut outputs: Vec<(&'static str, Vec<u8>)> = Vec::new();
        let mut inert = Vec::new();
        for &a in Action::ALL {
            let Some(f) = default_filter(a, fg, bg) else { continue };
            let mut d = doc.clone();
            let r = filter::apply_filter(&mut d, 0, &f);
            let out = d.layers[0].raster.to_rgba();
            let changed = out != base && !r.is_empty();
            if !changed && !identity_ok.contains(&f.id()) {
                inert.push(f.name());
            }
            outputs.push((f.name(), out));
        }
        assert!(inert.is_empty(), "filters that did nothing at their defaults: {inert:?}");
        let mut dupes = Vec::new();
        for i in 0..outputs.len() {
            for j in i + 1..outputs.len() {
                if outputs[i].1 == outputs[j].1 && outputs[i].1 != base {
                    dupes.push(format!("{} == {}", outputs[i].0, outputs[j].0));
                }
            }
        }
        assert!(dupes.is_empty(), "filters with identical output: {dupes:?}");
    }
}
