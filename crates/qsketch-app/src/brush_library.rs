//! Brush tip and grain texture library: procedural built-ins plus user PNGs in
//! `<config>/brush_tips/` and `<config>/textures/`, and cached egui previews.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{ColorImage, TextureHandle, TextureOptions};
use qsketch_core::io::{brush_formats, image_io};
use qsketch_core::tip::{builtin_textures, builtin_tips};
use qsketch_core::{BrushSettings, Raster, Rgba8, TipImage};

use crate::settings::Settings;

/// Largest side a user tip/texture is stored at (bigger images are downsampled).
const MAX_USER_PX: u32 = 512;

pub const TIPS_DIR: &str = "brush_tips";
pub const TEXTURES_DIR: &str = "textures";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryKind {
    Tip,
    Texture,
}

impl LibraryKind {
    pub fn dir_name(self) -> &'static str {
        match self {
            LibraryKind::Tip => TIPS_DIR,
            LibraryKind::Texture => TEXTURES_DIR,
        }
    }
}

pub struct BrushLibrary {
    pub tips: Vec<Arc<TipImage>>,
    pub textures: Vec<Arc<TipImage>>,
    previews: HashMap<(LibraryKindKey, String), TextureHandle>,
    stroke_previews: HashMap<String, (u64, TextureHandle)>,
}

// `LibraryKind` isn't Hash by itself; keep a tiny key type for the cache map.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct LibraryKindKey(u8);

impl From<LibraryKind> for LibraryKindKey {
    fn from(k: LibraryKind) -> Self {
        LibraryKindKey(match k {
            LibraryKind::Tip => 0,
            LibraryKind::Texture => 1,
        })
    }
}

impl Default for BrushLibrary {
    fn default() -> Self {
        Self::load()
    }
}

impl BrushLibrary {
    /// Built-ins followed by whatever PNGs are in the user directories.
    pub fn load() -> Self {
        let mut lib = Self {
            tips: builtin_tips().into_iter().map(Arc::new).collect(),
            textures: builtin_textures().into_iter().map(Arc::new).collect(),
            previews: HashMap::new(),
            stroke_previews: HashMap::new(),
        };
        lib.reload_user();
        lib
    }

    /// Re-scan the user directories (keeps built-ins).
    pub fn reload_user(&mut self) {
        self.tips.retain(|t| t.builtin);
        self.textures.retain(|t| t.builtin);
        for (kind, list) in [(LibraryKind::Tip, &mut self.tips), (LibraryKind::Texture, &mut self.textures)] {
            let Some(dir) = Self::dir(kind) else { continue };
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            let mut paths: Vec<PathBuf> =
                rd.flatten().map(|e| e.path()).filter(|p| qsketch_core::io::is_image(p)).collect();
            paths.sort();
            for p in paths {
                match Self::load_file(kind, &p) {
                    Ok(t) => list.push(Arc::new(t)),
                    Err(e) => log::warn!("skipping {}: {e:#}", p.display()),
                }
            }
        }
        self.previews.clear();
    }

    pub fn dir(kind: LibraryKind) -> Option<PathBuf> {
        Settings::config_dir().map(|d| d.join(kind.dir_name()))
    }

    fn load_file(kind: LibraryKind, path: &Path) -> anyhow::Result<TipImage> {
        let raster = image_io::import(path)?;
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
        Ok(Self::from_raster(kind, name, &raster))
    }

    /// Convert pixels into a tip (dark = paint) or texture (light = paint),
    /// downsampling very large images.
    pub fn from_raster(kind: LibraryKind, name: String, raster: &Raster) -> TipImage {
        let (w, h) = (raster.width(), raster.height());
        let rgba = raster.to_rgba();
        let mut t = match kind {
            LibraryKind::Tip => TipImage::from_rgba_dark(name, w, h, &rgba),
            LibraryKind::Texture => TipImage::from_rgba_luma(name, w, h, &rgba),
        };
        if w.max(h) > MAX_USER_PX {
            let k = MAX_USER_PX as f32 / w.max(h) as f32;
            t = t.resized(((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1));
        }
        t
    }

    pub fn list(&self, kind: LibraryKind) -> &[Arc<TipImage>] {
        match kind {
            LibraryKind::Tip => &self.tips,
            LibraryKind::Texture => &self.textures,
        }
    }

    pub fn find(&self, kind: LibraryKind, name: &str) -> Option<Arc<TipImage>> {
        if name.is_empty() {
            return None;
        }
        self.list(kind).iter().find(|t| t.name == name).cloned()
    }

    /// Tip and texture referenced by `settings` (None for the round tip / no texture).
    pub fn resolve(&self, settings: &BrushSettings) -> (Option<Arc<TipImage>>, Option<Arc<TipImage>>) {
        let tip = if settings.is_round() { None } else { self.find(LibraryKind::Tip, &settings.tip) };
        let tex = if settings.texture_enabled { self.find(LibraryKind::Texture, &settings.texture) } else { None };
        (tip, tex)
    }

    /// Persist a new user tip/texture as a PNG in the library directory and add it.
    pub fn add(&mut self, kind: LibraryKind, mut image: TipImage) -> anyhow::Result<String> {
        let dir = Self::dir(kind).ok_or_else(|| anyhow::anyhow!("no config directory"))?;
        std::fs::create_dir_all(&dir)?;
        let base = sanitize(&image.name);
        let mut name = base.clone();
        let mut n = 2;
        while self.list(kind).iter().any(|t| t.name == name) {
            name = format!("{base} {n}");
            n += 1;
        }
        image.name = name.clone();
        image.builtin = false;
        // Tips are stored as "dark = paint" so the PNG round-trips through `from_rgba_dark`.
        let rgba: Vec<u8> = match kind {
            LibraryKind::Tip => image
                .alpha
                .iter()
                .flat_map(|a| {
                    let v = 255 - (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    [v, v, v, 255]
                })
                .collect(),
            LibraryKind::Texture => image
                .alpha
                .iter()
                .flat_map(|a| {
                    let v = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    [v, v, v, 255]
                })
                .collect(),
        };
        let png = image_io::encode_png(image.width, image.height, &rgba)?;
        std::fs::write(dir.join(format!("{name}.png")), png)?;
        match kind {
            LibraryKind::Tip => self.tips.push(Arc::new(image)),
            LibraryKind::Texture => self.textures.push(Arc::new(image)),
        }
        Ok(name)
    }

    /// Import an image, or a GIMP/Photoshop brush file (`.gbr`/`.gih`/`.abr`,
    /// tips only), into the library. Returns the name of the first item added.
    pub fn import(&mut self, kind: LibraryKind, path: &Path) -> anyhow::Result<String> {
        if brush_formats::is_brush_file(path) {
            let tips = brush_formats::import(path)?;
            let mut first = None;
            for t in tips {
                // Masks are "1 = paint", which is what both tips and textures use.
                let mut img = TipImage::from_alpha(t.name, t.width, t.height, t.alpha);
                if t.width.max(t.height) > MAX_USER_PX {
                    let k = MAX_USER_PX as f32 / t.width.max(t.height) as f32;
                    img = img.resized(((t.width as f32 * k) as u32).max(1), ((t.height as f32 * k) as u32).max(1));
                }
                let name = self.add(kind, img)?;
                first.get_or_insert(name);
            }
            return first.ok_or_else(|| anyhow::anyhow!("no brushes in file"));
        }
        let img = Self::load_file(kind, path)?;
        self.add(kind, img)
    }

    /// Remove a user item (built-ins can't be deleted) and its PNG.
    pub fn remove(&mut self, kind: LibraryKind, name: &str) -> anyhow::Result<()> {
        let list = match kind {
            LibraryKind::Tip => &mut self.tips,
            LibraryKind::Texture => &mut self.textures,
        };
        let Some(i) = list.iter().position(|t| t.name == name && !t.builtin) else {
            anyhow::bail!("built-in items can't be deleted");
        };
        list.remove(i);
        if let Some(dir) = Self::dir(kind) {
            let p = dir.join(format!("{name}.png"));
            if p.exists() {
                std::fs::remove_file(p)?;
            }
        }
        self.previews.remove(&(kind.into(), name.to_string()));
        Ok(())
    }

    /// Square thumbnail of a tip/texture (white on transparent), cached.
    pub fn preview(&mut self, ctx: &egui::Context, kind: LibraryKind, name: &str, px: usize) -> Option<TextureHandle> {
        let key = (kind.into(), name.to_string());
        if let Some(h) = self.previews.get(&key) {
            return Some(h.clone());
        }
        let t = self.find(kind, name)?;
        let img = tip_thumb(&t, px, kind == LibraryKind::Texture);
        let h = ctx.load_texture(format!("tip-{}-{name}", kind.dir_name()), img, TextureOptions::LINEAR);
        self.previews.insert(key, h.clone());
        Some(h)
    }

    /// A rendered sample stroke for `settings`, cached by content hash under `key`.
    pub fn stroke_preview(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        settings: &BrushSettings,
        fg: Rgba8,
        bg: Rgba8,
        size: [usize; 2],
    ) -> TextureHandle {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(settings).unwrap_or_default().hash(&mut hasher);
        (fg.r, fg.g, fg.b, fg.a, bg.r, bg.g, bg.b).hash(&mut hasher);
        size.hash(&mut hasher);
        let h = hasher.finish();
        if let Some((hh, tex)) = self.stroke_previews.get(key) {
            if *hh == h {
                return tex.clone();
            }
        }
        let (tip, tex) = self.resolve(settings);
        let px = qsketch_core::brush::render_preview(settings, tip, tex, fg, bg, size[0] as u32, size[1] as u32);
        let img = ColorImage::from_rgba_unmultiplied(size, &px);
        let handle = ctx.load_texture(format!("stroke-preview-{key}"), img, TextureOptions::LINEAR);
        self.stroke_previews.insert(key.to_string(), (h, handle.clone()));
        handle
    }

    pub fn forget_stroke_preview(&mut self, key: &str) {
        self.stroke_previews.remove(key);
    }
}

fn sanitize(name: &str) -> String {
    let s: String = name.chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_').collect();
    let s = s.trim();
    if s.is_empty() {
        "Custom".into()
    } else {
        s.to_string()
    }
}

/// Fit a tip into a `px × px` square with the mask as alpha (textures are shown
/// as their tile, tips are centered with a small margin).
fn tip_thumb(t: &TipImage, px: usize, tile: bool) -> ColorImage {
    let mut out = Vec::with_capacity(px * px);
    let (w, h) = (t.width as f32, t.height as f32);
    let margin = if tile { 0.0 } else { 0.06 };
    let scale = if tile { 1.0 } else { w.max(h) };
    for y in 0..px {
        for x in 0..px {
            let nx = (x as f32 + 0.5) / px as f32;
            let ny = (y as f32 + 0.5) / px as f32;
            let a = if tile {
                t.sample_wrap(nx * w, ny * h)
            } else {
                // Center the image inside the square, preserving aspect.
                let u = ((nx - 0.5) / (1.0 - margin * 2.0)) * scale + w / 2.0;
                let v = ((ny - 0.5) / (1.0 - margin * 2.0)) * scale + h / 2.0;
                if u < 0.0 || v < 0.0 || u >= w || v >= h {
                    0.0
                } else {
                    t.sample_wrap(u - 0.5, v - 0.5)
                }
            };
            let a8 = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            out.push(egui::Color32::from_rgba_premultiplied(a8, a8, a8, a8));
        }
    }
    ColorImage::new([px, px], out)
}
