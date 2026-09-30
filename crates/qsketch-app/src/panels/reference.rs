//! Reference panel: a picture kept beside the canvas to draw from. Open a
//! file, pan and zoom it, flip it, view it in grayscale to judge values, and
//! pick colors straight out of it (click sets the foreground color, Alt+click
//! the background). The last picture is remembered between sessions.

use std::path::{Path, PathBuf};

use egui::{Color32, ColorImage, Sense, TextureHandle, TextureOptions, Ui, Vec2};
use qsketch_core::Rgba8;

use crate::state::AppState;
use crate::ui::icons;
use crate::ui::widgets::{icon_button, icon_toggle, small_button, wheel_notch, Swatch};

/// Longest side the picture is kept at; bigger files are shrunk on load (GPU
/// textures have limits, and a reference needs no more).
const MAX_SIDE: u32 = 4096;

#[derive(Default)]
pub struct Reference {
    pub path: Option<PathBuf>,
    /// RGBA8 pixels, `w × h`.
    pixels: Option<(u32, u32, Vec<u8>)>,
    texture: Option<TextureHandle>,
    gray_texture: Option<TextureHandle>,
    /// Screen pixels per image pixel; `None` fits the panel.
    pub zoom: Option<f32>,
    /// Image point shown at the centre of the panel.
    pub center: Vec2,
    pub flip_h: bool,
    pub gray: bool,
    pub hover_color: Option<Rgba8>,
    error: Option<String>,
    /// The remembered path from the settings has been tried once.
    restored: bool,
}

impl Reference {
    pub fn is_empty(&self) -> bool {
        self.pixels.is_none()
    }

    /// Load an image file; a failure is shown in the panel.
    pub fn open(&mut self, path: &Path) {
        match qsketch_core::io::image_io::import(path) {
            Ok(raster) => {
                let (w, h, rgba) = shrink(raster.width(), raster.height(), raster.to_rgba());
                self.center = Vec2::new(w as f32 / 2.0, h as f32 / 2.0);
                self.pixels = Some((w, h, rgba));
                self.path = Some(path.to_path_buf());
                self.texture = None;
                self.gray_texture = None;
                self.zoom = None;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    pub fn clear(&mut self) {
        *self = Self { restored: true, ..Self::default() };
    }

    fn texture(&mut self, ctx: &egui::Context) -> Option<TextureHandle> {
        let (w, h, rgba) = self.pixels.as_ref()?;
        let size = [*w as usize, *h as usize];
        if self.gray {
            if self.gray_texture.is_none() {
                let gray: Vec<u8> = rgba
                    .chunks_exact(4)
                    .flat_map(|p| {
                        let l = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32).round() as u8;
                        [l, l, l, p[3]]
                    })
                    .collect();
                let img = ColorImage::from_rgba_unmultiplied(size, &gray);
                self.gray_texture = Some(ctx.load_texture("reference_gray", img, TextureOptions::LINEAR));
            }
            self.gray_texture.clone()
        } else {
            if self.texture.is_none() {
                let img = ColorImage::from_rgba_unmultiplied(size, rgba);
                self.texture = Some(ctx.load_texture("reference", img, TextureOptions::LINEAR));
            }
            self.texture.clone()
        }
    }

    fn pixel(&self, x: i64, y: i64) -> Option<Rgba8> {
        let (w, h, px) = self.pixels.as_ref()?;
        if x < 0 || y < 0 || x >= *w as i64 || y >= *h as i64 {
            return None;
        }
        let i = (y as usize * *w as usize + x as usize) * 4;
        Some(Rgba8::new(px[i], px[i + 1], px[i + 2], px[i + 3]))
    }
}

/// Box-average shrink by an integer factor so the longest side is at most
/// `MAX_SIDE`.
fn shrink(w: u32, h: u32, rgba: Vec<u8>) -> (u32, u32, Vec<u8>) {
    let k = w.max(h).div_ceil(MAX_SIDE);
    if k <= 1 || w == 0 || h == 0 {
        return (w, h, rgba);
    }
    let (tw, th) = (w.div_ceil(k), h.div_ceil(k));
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for ty in 0..th {
        for tx in 0..tw {
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for y in ty * k..((ty + 1) * k).min(h) {
                for x in tx * k..((tx + 1) * k).min(w) {
                    let i = ((y * w + x) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += rgba[i + c] as u32;
                    }
                    n += 1;
                }
            }
            let o = ((ty * tw + tx) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    (tw, th, out)
}

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let ctx = ui.ctx().clone();
    let AppState { reference: r, settings, fg, bg, toasts, .. } = state;
    // Reopen the picture from the last session the first time the panel shows.
    if !r.restored {
        r.restored = true;
        if let Some(p) = settings.reference_image.clone().filter(|p| p.exists()) {
            r.open(&p);
        }
    }

    ui.horizontal(|ui| {
        if icon_button(ui, icons::FOLDER_OPEN, "Open a reference image…", 22.0, false).clicked() {
            let mut dlg = rfd::FileDialog::new()
                .set_title("Open Reference Image")
                .add_filter("Images", qsketch_core::io::IMAGE_EXTENSIONS);
            if let Some(dir) = r.path.as_ref().and_then(|p| p.parent()).filter(|d| d.exists()) {
                dlg = dlg.set_directory(dir);
            }
            if let Some(path) = dlg.pick_file() {
                r.open(&path);
                match &r.error {
                    Some(e) => toasts.push(crate::ui::toasts::Level::Error, format!("Couldn't open reference: {e}")),
                    None => settings.reference_image = r.path.clone(),
                }
            }
        }
        if !r.is_empty() {
            if icon_button(ui, icons::ARROWS_OUT_SIMPLE, "Fit the picture in the panel", 22.0, r.zoom.is_none())
                .clicked()
            {
                r.zoom = None;
            }
            if small_button(ui, "100%").on_hover_text("One picture pixel per screen pixel").clicked() {
                r.zoom = Some(1.0);
            }
            icon_toggle(ui, icons::FLIP_HORIZONTAL, icons::FLIP_HORIZONTAL, &mut r.flip_h, "Flip horizontally", 22.0);
            icon_toggle(ui, icons::CIRCLE_HALF, icons::CIRCLE_HALF, &mut r.gray, "Grayscale, to judge values", 22.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, icons::X, "Close the reference", 22.0, false).clicked() {
                    r.clear();
                    settings.reference_image = None;
                }
            });
        }
    });

    if r.is_empty() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "Open a picture to keep beside the canvas. Drag to pan, wheel to zoom, click to pick its color (Alt+click for the background color).",
            )
            .weak(),
        );
        if let Some(e) = &r.error {
            ui.label(egui::RichText::new(e).color(ui.visuals().error_fg_color).small());
        }
        return;
    }
    let Some(tex) = r.texture(&ctx) else { return };
    let (w, h) = r.pixels.as_ref().map(|(w, h, _)| (*w as f32, *h as f32)).unwrap_or((1.0, 1.0));

    // --- the picture -----------------------------------------------------
    let avail = ui.available_size();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(avail.x, (avail.y - 22.0).max(40.0)), Sense::click_and_drag());
    let fit = (rect.width() / w).min(rect.height() / h).max(1e-3);
    let mut zoom = r.zoom.unwrap_or(fit);
    // Image ↔ screen, mirrored when flipped.
    let to_screen = |img: Vec2, center: Vec2, zoom: f32, flip: bool| -> egui::Pos2 {
        let dx = if flip { center.x - img.x } else { img.x - center.x };
        rect.center() + egui::vec2(dx * zoom, (img.y - center.y) * zoom)
    };
    let to_image = |p: egui::Pos2, center: Vec2, zoom: f32, flip: bool| -> Vec2 {
        let d = (p - rect.center()) / zoom;
        Vec2::new(if flip { center.x - d.x } else { center.x + d.x }, center.y + d.y)
    };
    // Wheel zooms about the pointer.
    if resp.hovered() {
        if let (Some(n), Some(p)) = (wheel_notch(ui), resp.hover_pos()) {
            let before = to_image(p, r.center, zoom, r.flip_h);
            zoom = (zoom * 1.2f32.powf(n)).clamp(fit * 0.25, 32.0);
            let d = (p - rect.center()) / zoom;
            r.center = Vec2::new(if r.flip_h { before.x + d.x } else { before.x - d.x }, before.y - d.y);
            r.zoom = Some(zoom);
        }
    }
    if resp.dragged() {
        let d = resp.drag_delta() / zoom;
        r.center -= Vec2::new(if r.flip_h { -d.x } else { d.x }, d.y);
    }
    r.center = Vec2::new(r.center.x.clamp(0.0, w), r.center.y.clamp(0.0, h));

    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let a = to_screen(Vec2::ZERO, r.center, zoom, r.flip_h);
    let b = to_screen(Vec2::new(w, h), r.center, zoom, r.flip_h);
    let img_rect = egui::Rect::from_two_pos(a, b);
    let uv = if r.flip_h {
        egui::Rect::from_min_max(egui::pos2(1.0, 0.0), egui::pos2(0.0, 1.0))
    } else {
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
    };
    painter.image(tex.id(), img_rect, uv, Color32::WHITE);
    painter.rect_stroke(
        img_rect,
        0.0,
        egui::Stroke::new(1.0, Color32::from_black_alpha(120)),
        egui::StrokeKind::Outside,
    );

    // --- color under the pointer / picking --------------------------------
    let hover = resp.hover_pos().map(|p| to_image(p, r.center, zoom, r.flip_h));
    r.hover_color = hover.and_then(|v| r.pixel(v.x.floor() as i64, v.y.floor() as i64));
    if resp.clicked() {
        if let Some(c) = r.hover_color {
            if ui.input(|i| i.modifiers.alt) {
                *bg = c;
            } else {
                *fg = c;
            }
        }
    }
    if resp.hovered() {
        ui.output_mut(|o| {
            o.cursor_icon = if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Crosshair }
        });
    }
    ui.horizontal(|ui| {
        match r.hover_color {
            Some(c) => {
                ui.add(Swatch { color: c, size: egui::vec2(16.0, 14.0), selected: false });
                ui.label(egui::RichText::new(format!("{} {} {}  {}", c.r, c.g, c.b, c.to_hex())).small().monospace());
            }
            None => {
                ui.label(egui::RichText::new(format!("{} × {}  ·  {:.0}%", w, h, zoom * 100.0)).weak().small());
            }
        }
        if let Some(name) = r.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(name).weak().small());
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::shrink;

    #[test]
    fn shrink_keeps_small_pictures_and_averages_big_ones() {
        let (w, h, px) = shrink(10, 10, vec![9u8; 400]);
        assert_eq!((w, h, px.len()), (10, 10, 400));
        let big = vec![200u8; (9000 * 2 * 4) as usize];
        let (w, h, px) = shrink(9000, 2, big);
        assert_eq!((w, h), (3000, 1));
        assert!(px.iter().all(|&v| v == 200));
    }
}
