//! Clipboard image payloads (internal clipboard; the app bridges to the OS).

use crate::color::Rgba8;
use crate::composite::Composite;
use crate::geom::IRect;
use crate::mask::Mask;
use crate::raster::Raster;

/// A straight-alpha RGBA8 image with the document position it was copied from.
#[derive(Clone, Debug)]
pub struct ClipImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub origin: (i32, i32),
}

impl ClipImage {
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        assert_eq!(rgba.len(), (width * height * 4) as usize);
        Self { width, height, rgba, origin: (0, 0) }
    }

    /// Whether `other` is this image as it comes back from the OS clipboard:
    /// same size, and the same color wherever this image is opaque. Alpha and
    /// the color under transparent pixels are ignored because some platforms
    /// flatten or premultiply on the way through the clipboard.
    pub fn same_picture(&self, other: &ClipImage) -> bool {
        if self.width != other.width || self.height != other.height {
            return false;
        }
        self.rgba.chunks_exact(4).zip(other.rgba.chunks_exact(4)).all(|(a, b)| {
            a[3] != 255 || (a[0].abs_diff(b[0]) <= 2 && a[1].abs_diff(b[1]) <= 2 && a[2].abs_diff(b[2]) <= 2)
        })
    }

    /// Copy the selected region (or the whole content bounds) of a layer.
    pub fn from_layer(raster: &Raster, selection: Option<&Mask>) -> Option<Self> {
        let rect = match selection {
            Some(m) => m.bounds(),
            None => raster.bounds()?,
        };
        if rect.is_empty() {
            return None;
        }
        let mut out = vec![0u8; (rect.w * rect.h * 4) as usize];
        let mut any = false;
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                let cov = selection.map_or(1.0, |m| m.coverage(x, y));
                if cov <= 0.0 {
                    continue;
                }
                let c = raster.get_pixel(x, y);
                if c.a == 0 {
                    continue;
                }
                any = true;
                let a = (c.a as f32 * cov).round() as u8;
                let i = (((y - rect.y) * rect.w + (x - rect.x)) * 4) as usize;
                out[i..i + 4].copy_from_slice(&c.with_alpha(a).to_array());
            }
        }
        any.then_some(Self { width: rect.w as u32, height: rect.h as u32, rgba: out, origin: (rect.x, rect.y) })
    }

    /// Copy merged (composite) pixels in the selection or the whole image.
    pub fn from_composite(comp: &Composite, selection: Option<&Mask>) -> Option<Self> {
        let full = IRect::new(0, 0, comp.width() as i32, comp.height() as i32);
        let rect = match selection {
            Some(m) => m.bounds(),
            None => full,
        };
        if rect.is_empty() {
            return None;
        }
        let mut out = vec![0u8; (rect.w * rect.h * 4) as usize];
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                let cov = selection.map_or(1.0, |m| m.coverage(x, y));
                if cov <= 0.0 {
                    continue;
                }
                let p = comp.get_premul(x, y);
                if p[3] == 0 {
                    continue;
                }
                let a = p[3] as u32;
                let un = |v: u8| ((v as u32 * 255 + a / 2) / a).min(255) as u8;
                let c = Rgba8::new(un(p[0]), un(p[1]), un(p[2]), (p[3] as f32 * cov).round() as u8);
                let i = (((y - rect.y) * rect.w + (x - rect.x)) * 4) as usize;
                out[i..i + 4].copy_from_slice(&c.to_array());
            }
        }
        Some(Self { width: rect.w as u32, height: rect.h as u32, rgba: out, origin: (rect.x, rect.y) })
    }

    pub fn to_raster(&self) -> Raster {
        Raster::from_rgba(self.width, self.height, &self.rgba)
    }

    /// Place the image on a new layer-sized raster at `(x, y)`.
    pub fn to_layer_raster(&self, doc_w: u32, doc_h: u32, x: i32, y: i32) -> Raster {
        let mut r = Raster::new(doc_w, doc_h);
        r.blit(&self.to_raster(), x, y, false);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_picture_tolerates_clipboard_alpha_loss() {
        let a = ClipImage::from_rgba(2, 1, vec![10, 20, 30, 255, 0, 0, 0, 0]);
        // Same opaque pixel, transparent pixel flattened to white.
        let b = ClipImage::from_rgba(2, 1, vec![11, 20, 29, 255, 255, 255, 255, 255]);
        assert!(a.same_picture(&b));
        let c = ClipImage::from_rgba(2, 1, vec![90, 20, 30, 255, 0, 0, 0, 0]);
        assert!(!a.same_picture(&c));
        let d = ClipImage::from_rgba(1, 2, vec![10, 20, 30, 255, 0, 0, 0, 0]);
        assert!(!a.same_picture(&d));
    }

    #[test]
    fn copy_selection() {
        let mut r = Raster::new(10, 10);
        r.fill_rect(IRect::new(2, 2, 4, 4), Rgba8::WHITE);
        let m = Mask::from_rect(10, 10, IRect::new(0, 0, 4, 4));
        let c = ClipImage::from_layer(&r, Some(&m)).unwrap();
        assert_eq!((c.width, c.height), (4, 4));
        assert_eq!(c.origin, (0, 0));
        let back = c.to_raster();
        assert_eq!(back.get_pixel(3, 3), Rgba8::WHITE);
        assert_eq!(back.get_pixel(0, 0), Rgba8::TRANSPARENT);
        let whole = ClipImage::from_layer(&r, None).unwrap();
        assert_eq!(whole.origin, (2, 2));
    }
}
