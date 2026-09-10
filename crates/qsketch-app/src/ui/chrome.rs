//! Subtle textures for the chrome (panel bodies, bars).
//!
//! A small tile is generated (or loaded from a user image) once, uploaded
//! with `Repeat` wrapping, and painted over a rect at low alpha. Tiles are
//! premultiplied light/dark speckle over a transparent base, so they
//! modulate whatever fill lies beneath and work on any theme. An optional
//! faint top sheen finishes the "techno-lite" look.

use egui::{Color32, ColorImage, Context, Painter, Rect, TextureHandle, TextureOptions, Ui};

use super::theme::Palette;
use crate::settings::{ChromeTexture, TextureSettings};

const TILE: usize = 128;

fn hash(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B) ^ seed.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65535.0
}

/// Smooth, tileable value noise at `cells` cells per tile.
fn value_noise(x: usize, y: usize, cells: usize, seed: u32) -> f32 {
    let cs = TILE as f32 / cells as f32;
    let fx = x as f32 / cs;
    let fy = y as f32 / cs;
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sy = ty * ty * (3.0 - 2.0 * ty);
    let c = |i: usize, j: usize| hash((i % cells) as u32, (j % cells) as u32, seed);
    let a = c(x0, y0) + (c(x0 + 1, y0) - c(x0, y0)) * sx;
    let b = c(x0, y0 + 1) + (c(x0 + 1, y0 + 1) - c(x0, y0 + 1)) * sx;
    (a + (b - a) * sy) * 2.0 - 1.0
}

/// Signed intensity in -1..=1 → premultiplied light/dark speckle.
fn speckle(n: f32) -> Color32 {
    let n = n.clamp(-1.0, 1.0);
    let a = (n.abs() * 90.0) as u8;
    if n >= 0.0 {
        Color32::from_white_alpha(a)
    } else {
        Color32::from_black_alpha(a)
    }
}

/// Horizontal brush streaks plus fine grain.
fn brushed(x: usize, y: usize) -> f32 {
    let mut streak = 0.0;
    for k in 0..3u32 {
        let r = hash(y as u32, k, 11) - 0.5;
        let r2 = hash(((y + 1) % TILE) as u32, k, 11) - 0.5;
        let r3 = hash(((y + TILE - 1) % TILE) as u32, k, 11) - 0.5;
        streak += (r * 2.0 + r2 + r3) / 4.0;
    }
    let along = hash((x / 8) as u32, y as u32, 7) - 0.5;
    let grain = hash(x as u32, y as u32, 3) - 0.5;
    streak * 1.4 + along * 0.35 + grain * 0.5
}

/// Twill weave: 8 px cells of alternating diagonal ridges.
fn carbon(x: usize, y: usize) -> f32 {
    let cell = 8;
    let (cx, cy) = (x / cell, y / cell);
    let (lx, ly) = ((x % cell) as f32 / cell as f32, (y % cell) as f32 / cell as f32);
    let d = if (cx + cy) % 2 == 0 { lx + ly } else { lx - ly + 1.0 };
    let ridge = ((d * std::f32::consts::PI * 2.0).sin()) * 0.5;
    let edge = if x % cell == 0 || y % cell == 0 { -0.35 } else { 0.0 };
    let grain = (hash(x as u32, y as u32, 5) - 0.5) * 0.3;
    ridge + edge + grain
}

/// Soft blotches at two scales plus a little fiber.
fn paper(x: usize, y: usize) -> f32 {
    let low = value_noise(x, y, 8, 21) * 0.6;
    let mid = value_noise(x, y, 32, 22) * 0.5;
    let fiber = (hash(x as u32, y as u32, 9) - 0.5) * 0.6;
    low + mid + fiber
}

/// Uniform fine grain.
fn grain(x: usize, y: usize) -> f32 {
    (hash(x as u32, y as u32, 13) - 0.5) * 1.6
}

/// Fine grid whose line opacity drifts across the tile (value noise), so
/// it reads as a faded blueprint rather than graph paper.
fn grid(x: usize, y: usize) -> f32 {
    let cell = 16;
    let on_line = x % cell == 0 || y % cell == 0;
    let major = x % (cell * 4) == 0 || y % (cell * 4) == 0;
    let fade = (value_noise(x, y, 4, 31) * 0.5 + 0.5).clamp(0.0, 1.0);
    if on_line {
        (0.35 + fade * 0.65) * if major { 1.0 } else { 0.6 }
    } else {
        (hash(x as u32, y as u32, 33) - 0.5) * 0.12
    }
}

/// Signed distance-ish helpers for the sprite patterns. Each returns
/// >0 inside the shape (with a soft edge) and 0 outside.
fn soft(d: f32, edge: f32) -> f32 {
    (1.0 - d / edge).clamp(0.0, 1.0)
}

/// Five-point star centered at (cx, cy) with outer radius r.
fn star(px: f32, py: f32, cx: f32, cy: f32, r: f32) -> f32 {
    let (dx, dy) = (px - cx, py - cy);
    let dist = (dx * dx + dy * dy).sqrt();
    if dist > r {
        return 0.0;
    }
    let ang = dy.atan2(dx);
    let k = std::f32::consts::PI * 2.0 / 5.0;
    let a = (ang.rem_euclid(k) - k / 2.0).abs(); // 0 at point, k/2 between points
    let inner = r * 0.45;
    let edge_r = inner + (r - inner) * (1.0 - a / (k / 2.0));
    soft(dist - edge_r + 0.8, 1.2)
}

fn heart(px: f32, py: f32, cx: f32, cy: f32, r: f32) -> f32 {
    // Classic implicit heart: (x² + y² − 1)³ − x²y³ < 0, y up.
    let x = (px - cx) / r;
    let y = -(py - cy) / r + 0.15;
    let v = (x * x + y * y - 1.0).powi(3) - x * x * y * y * y;
    soft(v * 3.0 + 0.1, 0.5)
}

/// Scatter sprites on a jittered 32 px lattice, half of them offset a row.
fn sprites(x: usize, y: usize, f: fn(f32, f32, f32, f32, f32) -> f32, r: f32) -> f32 {
    let cell = 32usize;
    let mut best: f32 = 0.0;
    for j in -1i32..=1 {
        for i in -1i32..=1 {
            let cx = ((x / cell) as i32 + i).rem_euclid((TILE / cell) as i32) as u32;
            let cy = ((y / cell) as i32 + j).rem_euclid((TILE / cell) as i32) as u32;
            let jx = hash(cx, cy, 41) * 12.0 - 6.0;
            let jy = hash(cx, cy, 42) * 12.0 - 6.0;
            let rr = r * (0.7 + hash(cx, cy, 43) * 0.6);
            let bx = (((x / cell) as i32 + i) * cell as i32) as f32 + cell as f32 * 0.5 + jx;
            let by = (((y / cell) as i32 + j) * cell as i32) as f32 + cell as f32 * 0.5 + jy;
            let v = f(x as f32 + 0.5, y as f32 + 0.5, bx, by, rr) * (0.55 + hash(cx, cy, 44) * 0.45);
            best = best.max(v);
        }
    }
    best
}

fn stars(x: usize, y: usize) -> f32 {
    sprites(x, y, star, 6.0) * 0.9 + (hash(x as u32, y as u32, 45) - 0.5) * 0.1
}

fn hearts(x: usize, y: usize) -> f32 {
    sprites(x, y, heart, 6.0) * 0.8
}

/// Polka dots on a staggered lattice, alternating light and dark.
fn dots(x: usize, y: usize) -> f32 {
    let cell = 16.0;
    let row = (y as f32 / cell).floor();
    let ox = if row as i32 % 2 == 0 { 0.0 } else { cell / 2.0 };
    let fx = (x as f32 + ox).rem_euclid(cell) - cell / 2.0;
    let fy = (y as f32).rem_euclid(cell) - cell / 2.0;
    let d = (fx * fx + fy * fy).sqrt();
    let col = ((x as f32 + ox) / cell).floor() as i32;
    let sign = if (col + row as i32) % 2 == 0 { 1.0 } else { -0.7 };
    soft(d - 3.0, 1.5) * sign
}

/// Argyle: diamond lattice with a thin crossing stitch line.
fn argyle(x: usize, y: usize) -> f32 {
    let cell = 32.0;
    let u = (x as f32 / cell).rem_euclid(1.0) - 0.5;
    let v = (y as f32 / cell).rem_euclid(1.0) - 0.5;
    let m = u.abs() + v.abs(); // diamond distance
    let checker = ((x as f32 / cell).floor() as i32 + (y as f32 / cell).floor() as i32) % 2 == 0;
    let fill = if m < 0.5 {
        if checker {
            0.35
        } else {
            -0.35
        }
    } else {
        0.0
    };
    let stitch = if ((u + v).abs() - 0.5).abs() < 0.02 || ((u - v).abs() - 0.5).abs() < 0.02 { 0.6 } else { 0.0 };
    fill + stitch + (hash(x as u32, y as u32, 51) - 0.5) * 0.1
}

fn make_tile(f: fn(usize, usize) -> f32) -> ColorImage {
    let mut px = vec![Color32::TRANSPARENT; TILE * TILE];
    for y in 0..TILE {
        for x in 0..TILE {
            px[y * TILE + x] = speckle(f(x, y));
        }
    }
    ColorImage::new([TILE, TILE], px)
}

/// Load a user image and turn it into neutral speckle around its mean luminance.
fn custom_tile(path: &str) -> Option<ColorImage> {
    if path.is_empty() {
        return None;
    }
    let img = image::open(path).ok()?.into_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let lum: Vec<f32> =
        img.pixels().map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0).collect();
    let mean = lum.iter().sum::<f32>() / lum.len() as f32;
    let px = lum.iter().map(|l| speckle((l - mean) * 2.5)).collect();
    Some(ColorImage::new([w, h], px))
}

#[derive(Clone)]
struct Cached {
    handle: Option<TextureHandle>,
    size: [usize; 2],
}

fn texture(ctx: &Context, t: &TextureSettings) -> Option<(TextureHandle, [usize; 2])> {
    let id = egui::Id::new(("qsketch_chrome_tex", t.kind, &t.custom_path, t.smooth));
    if let Some(c) = ctx.data(|d| d.get_temp::<Cached>(id)) {
        return c.handle.map(|h| (h, c.size));
    }
    let img = match t.kind {
        ChromeTexture::None => None,
        ChromeTexture::BrushedMetal => Some(make_tile(brushed)),
        ChromeTexture::Carbon => Some(make_tile(carbon)),
        ChromeTexture::Paper => Some(make_tile(paper)),
        ChromeTexture::Grain => Some(make_tile(grain)),
        ChromeTexture::Grid => Some(make_tile(grid)),
        ChromeTexture::Stars => Some(make_tile(stars)),
        ChromeTexture::Dots => Some(make_tile(dots)),
        ChromeTexture::Hearts => Some(make_tile(hearts)),
        ChromeTexture::Argyle => Some(make_tile(argyle)),
        ChromeTexture::Custom => custom_tile(&t.custom_path),
    };
    let base = if t.smooth { TextureOptions::LINEAR } else { TextureOptions::NEAREST };
    let opts = TextureOptions { wrap_mode: egui::TextureWrapMode::Repeat, ..base };
    let cached = match img {
        Some(img) => {
            let size = img.size;
            Cached { handle: Some(ctx.load_texture("chrome_texture", img, opts)), size }
        }
        None => Cached { handle: None, size: [1, 1] },
    };
    ctx.data_mut(|d| d.insert_temp(id, cached.clone()));
    cached.handle.map(|h| (h, cached.size))
}

/// Paint the texture over `rect` (screen coordinates). The tile is anchored
/// to the screen so scrolling content never drags the grain along with it.
pub fn paint(painter: &Painter, ctx: &Context, rect: Rect, p: &Palette, t: &TextureSettings) {
    if !t.enabled() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let Some((tex, size)) = texture(ctx, t) else { return };
    let scale = t.scale.clamp(0.25, 8.0);
    let (tw, th) = (size[0] as f32 * scale, size[1] as f32 * scale);
    let uv = Rect::from_min_max(
        egui::pos2(rect.left() / tw, rect.top() / th),
        egui::pos2(rect.right() / tw, rect.bottom() / th),
    );
    // Light themes read the grain louder; keep it fainter there.
    let k = if p.dark { t.strength } else { t.strength * 0.6 };
    painter.image(tex.id(), rect, uv, Color32::from_white_alpha((k * 255.0).round().clamp(0.0, 255.0) as u8));
    if !t.sheen {
        return;
    }
    // Top sheen: a short vertical fade from a faint highlight to nothing.
    let sheen_h = (rect.height() * 0.35).clamp(6.0, 48.0);
    let top = Rect::from_min_size(rect.min, egui::vec2(rect.width(), sheen_h));
    let hi = if p.dark { (k * 40.0) as u8 } else { (k * 70.0) as u8 };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(top.left_top(), Color32::from_white_alpha(hi));
    mesh.colored_vertex(top.right_top(), Color32::from_white_alpha(hi));
    mesh.colored_vertex(top.right_bottom(), Color32::TRANSPARENT);
    mesh.colored_vertex(top.left_bottom(), Color32::TRANSPARENT);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// Texture the visible area of `ui` (its clip rect), beneath whatever is
/// drawn afterwards. Call first thing inside a panel body or bar frame.
pub fn paint_ui(ui: &Ui, p: &Palette, t: &TextureSettings) {
    paint(ui.painter(), ui.ctx(), ui.clip_rect(), p, t);
}
