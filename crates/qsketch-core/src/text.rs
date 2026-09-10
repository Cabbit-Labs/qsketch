//! Text rasterization for the Text tool: lays out UTF-8 text with a TrueType /
//! OpenType font and renders it into a straight-alpha [`Raster`] the caller
//! drops onto a layer. Pure and headless; font discovery lives in the app.

use ab_glyph::{Font, FontArc, Glyph, PxScale, ScaleFont};
use serde::{Deserialize, Serialize};

use crate::{IRect, Raster, Rgba8};

/// Horizontal alignment of the lines relative to the anchor point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// How the text is set. Every field has a sensible default so a saved
/// options struct from an older version still loads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    /// Font size in document pixels (em height).
    pub size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// Extra advance between glyphs in pixels.
    pub letter_spacing: f32,
    pub align: TextAlign,
    /// Anti-aliased coverage; off gives hard 1-bit edges for pixel art.
    pub antialias: bool,
    /// Synthetic emboldening in pixels (0 = off), for families without a bold face.
    pub faux_bold: f32,
    /// Synthetic slant (0 = upright, ~0.2 = typical italic), for families without an italic face.
    pub faux_italic: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            size: 48.0,
            line_height: 1.2,
            letter_spacing: 0.0,
            align: TextAlign::Left,
            antialias: true,
            faux_bold: 0.0,
            faux_italic: 0.0,
        }
    }
}

impl TextStyle {
    pub fn clamp(&mut self) {
        self.size = self.size.clamp(1.0, 2000.0);
        self.line_height = self.line_height.clamp(0.5, 4.0);
        self.letter_spacing = self.letter_spacing.clamp(-50.0, 200.0);
        self.faux_bold = self.faux_bold.clamp(0.0, 20.0);
        self.faux_italic = self.faux_italic.clamp(-1.0, 1.0);
    }
}

/// Rendered text: the pixels plus where their top-left sits relative to the
/// anchor point the text was laid out at.
#[derive(Clone)]
pub struct TextImage {
    pub raster: Raster,
    /// Offset of `raster`'s origin from the anchor, in document pixels.
    pub offset: (i32, i32),
}

impl TextImage {
    /// Placement rect for an anchor at `(ax, ay)`.
    pub fn rect_at(&self, ax: i32, ay: i32) -> IRect {
        IRect::new(ax + self.offset.0, ay + self.offset.1, self.raster.width() as i32, self.raster.height() as i32)
    }
}

struct Positioned {
    glyph: Glyph,
}

/// Lay out `text` (lines split on `\n`) so that the anchor is the top of the
/// first line; x alignment is relative to the anchor per [`TextAlign`].
fn layout(font: &FontArc, text: &str, style: &TextStyle) -> Vec<Positioned> {
    let scale = PxScale::from(style.size.max(1.0));
    let sf = font.as_scaled(scale);
    let ascent = sf.ascent();
    let line_h = style.size * style.line_height;
    let mut out = Vec::new();
    for (li, line) in text.split('\n').enumerate() {
        let y = li as f32 * line_h + ascent;
        // Measure the line first for alignment.
        let mut width = 0.0f32;
        let mut prev: Option<ab_glyph::GlyphId> = None;
        let mut ids = Vec::with_capacity(line.chars().count());
        for ch in line.chars() {
            let id = sf.glyph_id(ch);
            if let Some(p) = prev {
                width += sf.kern(p, id);
            }
            width += sf.h_advance(id) + style.letter_spacing;
            ids.push(id);
            prev = Some(id);
        }
        if !ids.is_empty() {
            width -= style.letter_spacing;
        }
        let mut x = match style.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => -width / 2.0,
            TextAlign::Right => -width,
        };
        prev = None;
        for id in ids {
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            out.push(Positioned { glyph: id.with_scale_and_position(scale, ab_glyph::point(x, y)) });
            x += sf.h_advance(id) + style.letter_spacing;
            prev = Some(id);
        }
    }
    out
}

/// Bounding box (relative to the anchor) the rendered text would occupy,
/// or None for text with no visible glyphs.
pub fn measure(font: &FontArc, text: &str, style: &TextStyle) -> Option<IRect> {
    let glyphs = layout(font, text, style);
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for g in &glyphs {
        let Some(og) = font.outline_glyph(g.glyph.clone()) else { continue };
        let b = og.px_bounds();
        let slant = style.faux_italic.abs() * style.size;
        min_x = min_x.min(b.min.x - slant);
        min_y = min_y.min(b.min.y);
        max_x = max_x.max(b.max.x + style.faux_bold + slant);
        max_y = max_y.max(b.max.y + style.faux_bold);
    }
    if min_x > max_x {
        return None;
    }
    let x0 = min_x.floor() as i32;
    let y0 = min_y.floor() as i32;
    let x1 = max_x.ceil() as i32;
    let y1 = max_y.ceil() as i32;
    Some(IRect::new(x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)))
}

/// Render `text` in `color`. Returns None when nothing visible would be drawn.
pub fn render(font: &FontArc, text: &str, style: &TextStyle, color: Rgba8) -> Option<TextImage> {
    let bounds = measure(font, text, style)?;
    let glyphs = layout(font, text, style);
    let (w, h) = (bounds.w as usize, bounds.h as usize);
    // Coverage accumulator: overlapping glyphs (or faux bold passes) max, not add.
    let mut cov = vec![0f32; w * h];
    let bold_px = style.faux_bold.round().max(0.0) as i32;
    let bold_passes = bold_px + 1;
    let slant = style.faux_italic;
    // Slant pivots on the baseline of each line so a line's glyphs shear
    // consistently; ab_glyph's px_bounds gives glyph-local boxes so we shear
    // per row when writing coverage.
    for g in &glyphs {
        let Some(og) = font.outline_glyph(g.glyph.clone()) else { continue };
        let b = og.px_bounds();
        let baseline_y = g.glyph.position.y;
        og.draw(|gx, gy, c| {
            if c <= 0.0 {
                return;
            }
            let py = b.min.y + gy as f32;
            let row = py.floor() as i32 - bounds.y;
            if row < 0 || row >= h as i32 {
                return;
            }
            // Shear: rows above the baseline shift right (for positive slant).
            let dy = baseline_y - (py + 0.5);
            let shear = if slant != 0.0 { (dy * slant).round() as i32 } else { 0 };
            let base_col = b.min.x.floor() as i32 + gx as i32 - bounds.x + shear;
            for pass in 0..bold_passes {
                let col = base_col + pass;
                if col < 0 || col >= w as i32 {
                    continue;
                }
                let i = row as usize * w + col as usize;
                if c > cov[i] {
                    cov[i] = c;
                }
            }
        });
    }
    let mut raster = Raster::new(w as u32, h as u32);
    let a = color.a as f32 / 255.0;
    for y in 0..h {
        for x in 0..w {
            let c = cov[y * w + x];
            if c <= 0.0 {
                continue;
            }
            let c = if style.antialias {
                c.min(1.0)
            } else if c >= 0.5 {
                1.0
            } else {
                continue;
            };
            let alpha = (c * a * 255.0 + 0.5) as u8;
            if alpha == 0 {
                continue;
            }
            raster.set_pixel(x as i32, y as i32, Rgba8::new(color.r, color.g, color.b, alpha));
        }
    }
    Some(TextImage { raster, offset: (bounds.x, bounds.y) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> FontArc {
        // Any TTF works; the tests only need outlines. Use the vendored icon font
        // from the app crate if present, otherwise skip.
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/Phosphor.ttf");
        FontArc::try_from_vec(std::fs::read(p).expect("test font")).expect("parse")
    }

    #[test]
    fn empty_text_renders_nothing() {
        let f = font();
        assert!(render(&f, "", &TextStyle::default(), Rgba8::new(0, 0, 0, 255)).is_none());
        assert!(render(&f, "   \n ", &TextStyle::default(), Rgba8::new(0, 0, 0, 255)).is_none());
    }

    #[test]
    fn glyph_renders_with_color_and_alpha() {
        let f = font();
        let s = TextStyle { size: 32.0, ..Default::default() };
        let img = render(&f, "\u{E48A}", &s, Rgba8::new(200, 10, 20, 255)).expect("pixels");
        assert!(img.raster.width() > 4 && img.raster.height() > 4);
        let opaque = img.raster.bounds().expect("non-empty");
        assert!(!opaque.is_empty());
        let mut any = false;
        for y in 0..img.raster.height() as i32 {
            for x in 0..img.raster.width() as i32 {
                let p = img.raster.get_pixel(x, y);
                if p.a > 0 {
                    assert_eq!((p.r, p.g, p.b), (200, 10, 20));
                    any = true;
                }
            }
        }
        assert!(any);
        // Aliased output only has full or empty pixels.
        let s2 = TextStyle { antialias: false, ..s };
        let img2 = render(&f, "\u{E48A}", &s2, Rgba8::new(0, 0, 0, 255)).unwrap();
        for y in 0..img2.raster.height() as i32 {
            for x in 0..img2.raster.width() as i32 {
                let a = img2.raster.get_pixel(x, y).a;
                assert!(a == 0 || a == 255);
            }
        }
    }

    #[test]
    fn alignment_shifts_offset() {
        let f = font();
        let text = "\u{E48A}\u{E48A}\u{E48A}";
        let left = measure(&f, text, &TextStyle::default()).unwrap();
        let right = measure(&f, text, &TextStyle { align: TextAlign::Right, ..Default::default() }).unwrap();
        assert!(left.x >= -1);
        assert!(right.x < 0 && right.right() <= 1);
        assert_eq!(left.w, right.w);
    }

    #[test]
    fn second_line_is_lower() {
        let f = font();
        let one = measure(&f, "\u{E48A}", &TextStyle::default()).unwrap();
        let two = measure(&f, "\u{E48A}\n\u{E48A}", &TextStyle::default()).unwrap();
        assert!(two.h > one.h);
    }
}
