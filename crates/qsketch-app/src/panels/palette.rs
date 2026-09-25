//! Palette panel: the document's color palette (Aseprite-style). Slots are
//! clicked for foreground / background, dragged to reorder, ramped between
//! two selected slots, sorted, loaded from and saved to the usual palette
//! files, and the whole thing can be locked so every edit snaps to it.

use egui::{Ui, Vec2};
use qsketch_core::palette::{self, Palette};
use qsketch_core::Rgba8;

use crate::state::AppState;
use crate::ui::icons;
use crate::ui::toasts::Level;
use crate::ui::widgets::{icon_button, Swatch};

/// Panel memory that is not worth undoing: the selected run of slots and a
/// drag in progress.
#[derive(Clone, Copy, Default)]
struct Mem {
    /// Anchor and end of the selected range (inclusive, either order);
    /// mirrored into `DocEntry::palette_sel` for the shading ink.
    sel: Option<(usize, usize)>,
    drag_from: Option<usize>,
}

fn range(m: &Mem) -> Option<(usize, usize)> {
    m.sel.map(|(a, b)| (a.min(b), a.max(b)))
}

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let Some(doc_id) = state.active().map(|e| e.id) else {
        ui.label(egui::RichText::new("Open a document to edit its palette.").weak());
        return;
    };
    let mem_id = ui.id().with(("palette_mem", doc_id));
    let mut mem: Mem = ui.data(|d| d.get_temp(mem_id)).unwrap_or_default();
    let (mut pal, locked) = {
        let e = state.doc(doc_id).unwrap();
        mem.sel = e.palette_sel;
        let s = e.doc.state();
        (s.palette.clone(), s.palette_lock)
    };
    let n = pal.len();
    if let Some((a, b)) = mem.sel {
        if a >= n || b >= n {
            mem.sel = None;
        }
    }
    // What this frame decided to do; applied at the end so the borrows stay
    // simple. `commit` = undoable palette edit, `lock` = toggle (view-like).
    let mut commit: Option<&'static str> = None;
    let mut lock: Option<bool> = None;
    let mut open_snap = false;

    // --- toolbar ------------------------------------------------------------
    ui.horizontal(|ui| {
        let glyph = if locked { icons::LOCK } else { icons::LOCK_OPEN };
        let tip = if locked {
            "Palette locked: every edit snaps to these colors (indexed-color mode). Click to unlock."
        } else {
            "Lock to palette: brush, fill and filter results snap to these colors."
        };
        if icon_button(ui, glyph, tip, 22.0, locked).clicked() {
            lock = Some(!locked);
        }
        if icon_button(ui, icons::PLUS, "Add the foreground color", 22.0, false).clicked() {
            match pal.add_unique(state.fg) {
                Some(i) => {
                    mem.sel = Some((i, i));
                    commit = Some("Add Palette Color");
                }
                None => state.toasts.push(Level::Info, "The palette is full (256 colors)."),
            }
        }
        let has_sel = range(&mem).is_some();
        if icon_button(ui, icons::MINUS, "Remove the selected colors", 22.0, false).clicked() && has_sel {
            let (a, b) = range(&mem).unwrap();
            pal.colors.drain(a..=b);
            mem.sel = None;
            commit = Some("Remove Palette Color");
        }
        // Edit the selected slot in place.
        if let Some((a, b)) = range(&mem) {
            if a == b {
                let c = pal.colors[a];
                let mut c32 = egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a);
                let resp = egui::color_picker::color_edit_button_srgba(ui, &mut c32, egui::color_picker::Alpha::Opaque);
                if resp.changed() {
                    let [r, g, b2, _] = c32.to_srgba_unmultiplied();
                    pal.colors[a] = Rgba8::rgb(r, g, b2);
                    // Live while dragging inside the picker; one undo step
                    // once the button is up.
                    commit = Some(if ui.input(|i| i.pointer.any_down()) { "" } else { "Edit Palette Color" });
                }
                resp.on_hover_text("Edit the selected color");
            } else if b - a >= 2
                && icon_button(ui, icons::GRADIENT, "Ramp between the two ends of the selection", 22.0, false).clicked()
            {
                pal.ramp_between(a, b);
                commit = Some("Palette Ramp");
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(icons::DOTS_THREE, |ui| {
                ui.set_min_width(180.0);
                ui.menu_button("Presets", |ui| {
                    for p in palette::presets() {
                        if ui.button(format!("{} ({})", p.name, p.len())).clicked() {
                            pal = p;
                            mem.sel = None;
                            commit = Some("Load Palette");
                            ui.close();
                        }
                    }
                });
                if ui.button("Load…").clicked() {
                    ui.close();
                    let dlg = rfd::FileDialog::new()
                        .add_filter("Palettes", palette::LOAD_EXTENSIONS)
                        .add_filter("All files", &["*"]);
                    if let Some(path) = dlg.pick_file() {
                        match palette::load(&path) {
                            Ok(p) => {
                                pal = p;
                                mem.sel = None;
                                commit = Some("Load Palette");
                            }
                            Err(e) => state.toasts.push(Level::Error, format!("Could not load palette: {e}")),
                        }
                    }
                }
                if ui.add_enabled(n > 0, egui::Button::new("Save As…")).clicked() {
                    ui.close();
                    let dlg = rfd::FileDialog::new()
                        .add_filter("GIMP palette (.gpl)", &["gpl"])
                        .add_filter("Hex list (.hex)", &["hex"])
                        .add_filter("JASC palette (.pal)", &["pal"])
                        .add_filter("Adobe Color Table (.act)", &["act"])
                        .add_filter("Adobe Swatch Exchange (.aco)", &["aco"])
                        .add_filter("PNG strip (.png)", &["png"])
                        .set_file_name(format!("{}.gpl", if pal.name.is_empty() { "palette" } else { &pal.name }));
                    if let Some(mut path) = dlg.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("gpl");
                        }
                        match palette::save(&path, &pal) {
                            Ok(()) => state.toasts.push(Level::Info, format!("Saved {}", path.display())),
                            Err(e) => state.toasts.push(Level::Error, format!("Could not save palette: {e}")),
                        }
                    }
                }
                ui.separator();
                ui.menu_button("From image", |ui| {
                    for count in [4usize, 8, 16, 32, 64, 128, 256] {
                        if ui.button(format!("{count} colors")).clicked() {
                            let e = state.doc(doc_id).unwrap();
                            let flat = qsketch_core::composite::flatten(e.doc.state()).to_rgba();
                            pal = Palette::from_rgba(e.doc.title.clone(), &flat, count);
                            pal.sort_by_luma();
                            mem.sel = None;
                            commit = Some("Palette from Image");
                            ui.close();
                        }
                    }
                });
                if ui.add_enabled(n > 0, egui::Button::new("Snap layer to palette…")).clicked() {
                    open_snap = true;
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(n > 1, egui::Button::new("Sort by hue")).clicked() {
                    pal.sort_by_hue();
                    mem.sel = None;
                    commit = Some("Sort Palette");
                    ui.close();
                }
                if ui.add_enabled(n > 1, egui::Button::new("Sort by lightness")).clicked() {
                    pal.sort_by_luma();
                    mem.sel = None;
                    commit = Some("Sort Palette");
                    ui.close();
                }
                if ui.add_enabled(n > 0, egui::Button::new("Replace selected with foreground")).clicked() {
                    if let Some((a, b)) = range(&mem) {
                        for c in &mut pal.colors[a..=b] {
                            *c = state.fg;
                        }
                        commit = Some("Edit Palette Color");
                    }
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(n > 0, egui::Button::new("Clear")).clicked() {
                    pal.colors.clear();
                    mem.sel = None;
                    commit = Some("Clear Palette");
                    ui.close();
                }
            })
            .response
            .on_hover_text("Presets, load, save, sort…");
        });
    });
    // Name + count line.
    ui.horizontal(|ui| {
        let mut name = pal.name.clone();
        let te = egui::TextEdit::singleline(&mut name).hint_text("Palette name").desired_width(120.0);
        if ui.add(te).changed() {
            pal.name = name;
            commit = Some("");
        }
        let idx = pal.index_of(state.fg).map(|i| format!(" · fg #{i}")).unwrap_or_default();
        ui.label(egui::RichText::new(format!("{n} colors{idx}")).weak().small());
    });
    ui.separator();

    // --- grid ---------------------------------------------------------------
    let size = 16.0;
    let spacing = 2.0;
    let cols = ((ui.available_width() + spacing) / (size + spacing)).floor().max(1.0) as usize;
    let mut hovered: Option<usize> = None;
    let mods = ui.input(|inp| inp.modifiers);
    let released = ui.input(|inp| inp.pointer.any_released());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(spacing);
        if n == 0 {
            ui.label(
                egui::RichText::new("No palette. Add the foreground color with +, pick a preset, load a file, or build one from the image.")
                    .weak()
                    .small(),
            );
        }
        let sel = range(&mem);
        let mut i = 0;
        while i < n {
            ui.horizontal(|ui| {
                for _ in 0..cols {
                    if i >= n {
                        break;
                    }
                    let c = pal.colors[i];
                    let in_sel = sel.is_some_and(|(a, b)| i >= a && i <= b);
                    let resp = ui.add(Swatch { color: c, size: Vec2::splat(size), selected: in_sel });
                    let resp = resp.on_hover_text(format!("#{i}  {}", c.to_hex()));
                    if resp.hovered() {
                        hovered = Some(i);
                    }
                    if resp.drag_started() {
                        mem.drag_from = Some(i);
                    }
                    if resp.clicked() {
                        if mods.shift {
                            mem.sel = Some((mem.sel.map(|(a, _)| a).unwrap_or(i), i));
                        } else {
                            mem.sel = Some((i, i));
                            state.fg = c;
                        }
                    }
                    if resp.secondary_clicked() {
                        state.bg = c;
                    }
                    if c == state.fg && !in_sel {
                        let r = resp.rect.shrink(1.0);
                        ui.painter().rect_stroke(
                            r,
                            0.0,
                            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(140)),
                            egui::StrokeKind::Inside,
                        );
                    }
                    i += 1;
                }
            });
        }
    });
    // Drop a dragged slot at the hovered position.
    if let Some(from) = mem.drag_from {
        if released {
            if let Some(to) = hovered.filter(|t| *t != from) {
                let c = pal.colors.remove(from);
                pal.colors.insert(to, c);
                mem.sel = Some((to, to));
                commit = Some("Reorder Palette");
            }
            mem.drag_from = None;
        } else if let Some(to) = hovered {
            // Insertion caret.
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            let _ = to;
        }
    }
    ui.data_mut(|d| d.insert_temp(mem_id, mem));
    if let Some(e) = state.doc_mut(doc_id) {
        e.palette_sel = mem.sel;
    }

    // --- apply --------------------------------------------------------------
    if let Some(on) = lock {
        if let Some(e) = state.doc_mut(doc_id) {
            e.doc.set_palette_lock(on);
        }
        if on {
            state.toasts.push(
                Level::Info,
                "Palette locked: new edits snap to the palette. Image ▸ Index Colors converts what is already there.",
            );
        }
    }
    if let Some(label) = commit {
        if let Some(e) = state.doc_mut(doc_id) {
            e.doc.state_mut().palette = pal;
            if !label.is_empty() {
                e.doc.commit(label);
            }
        }
    }
    if open_snap {
        crate::dialogs::filter::open_snap_to_palette(state, false);
    }
}
