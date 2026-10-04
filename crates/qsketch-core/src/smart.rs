//! Smart objects: a layer that keeps its original pixels and draws them
//! through an editable placement (a box that moves, scales and turns; four
//! free corners; or a warp lattice). Every edit of the placement redraws
//! the layer from the originals, so scaling down and back up, or rotating
//! again and again, never wears the pixels down.
//!
//! The originals live in `Layer::smart` (shared, so copies of the layer
//! are cheap); the placement in `LayerProps::smart`; the layer's `raster`
//! is the drawn result, which is what the compositor and every format that
//! can't hold a smart object see.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::document::DocState;
use crate::geom::{IRect, Pt};
use crate::layer::LayerKind;
use crate::ops::{drop_floating, Floating};
use crate::raster::{Raster, ResizeFilter};
use crate::warp::{self, Mesh, Quad};

/// How the placement is described.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmartMode {
    /// A rectangle: `center`, `size`, `angle`.
    #[default]
    Box,
    /// Four free corners: `quad` (projective).
    Deform,
    /// A control lattice: `mesh`.
    Warp,
}

/// Where and how a smart object's original pixels are drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartObject {
    pub mode: SmartMode,
    pub center: Pt,
    pub size: (f32, f32),
    /// Radians, clockwise on screen.
    pub angle: f32,
    /// Corners (tl, tr, br, bl) for `Deform`.
    pub quad: Quad,
    /// Lattice for `Warp`: `mesh_rows + 1` rows of `mesh_cols + 1` points.
    pub mesh_cols: u32,
    pub mesh_rows: u32,
    pub mesh: Vec<Pt>,
    /// How the pixels are resampled when drawn.
    pub filter: ResizeFilter,
}

impl Default for SmartObject {
    fn default() -> Self {
        Self::identity(IRect::new(0, 0, 1, 1))
    }
}

impl SmartObject {
    /// The originals drawn unchanged, covering `rect`.
    pub fn identity(rect: IRect) -> Self {
        let (w, h) = (rect.w.max(1) as f32, rect.h.max(1) as f32);
        let center = Pt::new(rect.x as f32 + w / 2.0, rect.y as f32 + h / 2.0);
        Self {
            mode: SmartMode::Box,
            center,
            size: (w, h),
            angle: 0.0,
            quad: warp::free_quad(center, (w, h), 0.0),
            mesh_cols: 0,
            mesh_rows: 0,
            mesh: Vec::new(),
            filter: ResizeFilter::Bilinear,
        }
    }

    /// The outer corners (tl, tr, br, bl).
    pub fn corners(&self) -> Quad {
        match self.mode {
            SmartMode::Box => warp::free_quad(self.center, self.size, self.angle),
            SmartMode::Deform => self.quad,
            SmartMode::Warp => self.warp_mesh().map(|m| m.corners()).unwrap_or(self.quad),
        }
    }

    fn warp_mesh(&self) -> Option<Mesh> {
        let n = ((self.mesh_cols + 1) * (self.mesh_rows + 1)) as usize;
        (self.mesh_cols > 0 && self.mesh_rows > 0 && self.mesh.len() == n).then(|| Mesh {
            cols: self.mesh_cols,
            rows: self.mesh_rows,
            pts: self.mesh.clone(),
        })
    }

    /// The mesh the pixels are drawn through.
    pub fn render_mesh(&self) -> Mesh {
        match (self.mode, self.warp_mesh()) {
            (SmartMode::Warp, Some(m)) => m,
            _ => Mesh::from_quad(self.corners(), 1, 1),
        }
    }

    /// Whether the two draw the pixels the same way (fields the mode doesn't
    /// use, such as a box's idle warp lattice, don't count).
    pub fn same_as(&self, o: &SmartObject) -> bool {
        let close = |a: Pt, b: Pt| (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3;
        if self.mode != o.mode || self.filter != o.filter {
            return false;
        }
        match self.mode {
            SmartMode::Box => {
                close(self.center, o.center)
                    && (self.size.0 - o.size.0).abs() < 1e-3
                    && (self.size.1 - o.size.1).abs() < 1e-3
                    && (self.angle - o.angle).abs() < 1e-5
            }
            SmartMode::Deform => self.quad.iter().zip(&o.quad).all(|(a, b)| close(*a, *b)),
            SmartMode::Warp => {
                (self.mesh_cols, self.mesh_rows) == (o.mesh_cols, o.mesh_rows)
                    && self.mesh.len() == o.mesh.len()
                    && self.mesh.iter().zip(&o.mesh).all(|(a, b)| close(*a, *b))
            }
        }
    }

    /// Move the placement by `(dx, dy)`.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.center = Pt::new(self.center.x + dx, self.center.y + dy);
        for p in self.quad.iter_mut().chain(self.mesh.iter_mut()) {
            *p = Pt::new(p.x + dx, p.y + dy);
        }
    }

    /// Send every point of the placement through `f` (a canvas move,
    /// resize, flip or turn). `scale` says `f` is a move plus that scale;
    /// `None` (a flip or turn) or a stretch of a turned box makes a box
    /// four free corners.
    pub fn map(&mut self, f: impl Fn(Pt) -> Pt, scale: Option<(f32, f32)>) {
        match (self.mode, scale) {
            // A move or even scale keeps a turned box a box.
            (SmartMode::Box, Some((sx, sy))) if self.angle == 0.0 || (sx - sy).abs() < 1e-6 => {
                self.center = f(self.center);
                self.size = (self.size.0 * sx, self.size.1 * sy);
                self.quad = warp::free_quad(self.center, self.size, 0.0);
            }
            (SmartMode::Box, _) => {
                self.quad = self.corners().map(&f);
                self.mode = SmartMode::Deform;
            }
            _ => {
                self.quad = self.quad.map(&f);
                for p in &mut self.mesh {
                    *p = f(*p);
                }
                self.center = f(self.center);
            }
        }
    }

    /// Draw `source` through the placement onto a `width`×`height` raster,
    /// keeping whatever lands past the canvas edge outside it.
    pub fn render(&self, source: &Raster, width: u32, height: u32) -> Raster {
        let mesh = self.render_mesh();
        let canvas = IRect::new(0, 0, width as i32, height as i32);
        // Everything the placement covers, within reason.
        let clip = mesh.bounds().expand(2).intersect(&canvas.expand(MAX_OVERHANG)).union(&canvas);
        let out = Raster::new(width, height);
        let (img, r) = warp::warp_raster(source, &mesh, self.filter, clip);
        if r.is_empty() {
            return out;
        }
        drop_floating(&out, &Floating { raster: img, origin: (r.x, r.y), mask: None }, 0, 0)
    }
}

/// How far past the canvas a smart object's drawn pixels are kept (the
/// originals are always kept whole).
const MAX_OVERHANG: i32 = 16_384;

/// Redraw smart object `li` from its originals and placement.
pub fn rerender(doc: &mut DocState, li: usize) {
    let (w, h) = (doc.width, doc.height);
    if let Some(l) = doc.layers.get_mut(li) {
        if let (Some(p), Some(src)) = (&l.props.smart, &l.smart) {
            l.raster = p.render(src, w, h);
        }
    }
}

/// Turn layer `li` into a smart object holding its current pixels (on and
/// off the canvas). Shape and text layers keep their look but lose their
/// editable points / text. False when the layer has no pixels or can't be
/// converted.
pub fn convert(doc: &mut DocState, li: usize) -> bool {
    let Some(l) = doc.layers.get_mut(li) else { return false };
    if !matches!(l.props.kind, LayerKind::Raster | LayerKind::Shape | LayerKind::Text) {
        return false;
    }
    let Some(b) = l.raster.full_bounds() else { return false };
    l.smart = Some(Arc::new(l.raster.crop(b)));
    l.props.smart = Some(SmartObject::identity(b));
    l.props.kind = LayerKind::Smart;
    l.props.shape = None;
    l.props.text = None;
    l.cels.clear();
    true
}

/// A new smart object from `source`, placed by `placement`, drawn into
/// layer `li` (which becomes the smart object).
pub fn install(doc: &mut DocState, li: usize, source: Raster, placement: SmartObject) {
    if let Some(l) = doc.layers.get_mut(li) {
        l.props.kind = LayerKind::Smart;
        l.props.smart = Some(placement);
        l.props.shape = None;
        l.props.text = None;
        l.smart = Some(Arc::new(source));
        l.cels.clear();
    }
    rerender(doc, li);
}

/// Turn a smart object into a plain pixel layer, keeping its pixels.
pub fn rasterize(doc: &mut DocState, li: usize) -> bool {
    match doc.layers.get_mut(li) {
        Some(l) if l.props.kind == LayerKind::Smart => {
            l.props.kind = LayerKind::Raster;
            l.props.smart = None;
            l.smart = None;
            true
        }
        _ => false,
    }
}

/// Send every smart object's placement through `f` (see
/// [`SmartObject::map`]) and redraw it: canvas flips, turns and resizes
/// keep smart objects smart, drawn fresh from their originals.
pub fn map_all(doc: &mut DocState, f: impl Fn(Pt) -> Pt, scale: Option<(f32, f32)>) {
    for li in 0..doc.layers.len() {
        if let Some(p) = doc.layers[li].props.smart.as_mut() {
            p.map(&f, scale);
            rerender(doc, li);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    fn doc_with_square() -> DocState {
        let mut d = DocState::new(64, 64, None);
        d.layers[0].raster.fill_rect(IRect::new(10, 10, 20, 20), Rgba8::new(200, 40, 40, 255));
        d.layers[0].raster.set_pixel(10, 10, Rgba8::new(0, 0, 255, 255));
        d
    }

    #[test]
    fn convert_keeps_the_look_and_rasterize_keeps_the_pixels() {
        let mut d = doc_with_square();
        let before = d.layers[0].raster.to_rgba();
        assert!(convert(&mut d, 0));
        assert!(d.layers[0].is_smart());
        assert!(!d.layers[0].editable(), "paint needs rasterizing first");
        rerender(&mut d, 0);
        assert_eq!(d.layers[0].raster.to_rgba(), before, "identity placement redraws the same pixels");
        assert!(rasterize(&mut d, 0));
        assert_eq!(d.layers[0].props.kind, LayerKind::Raster);
        assert_eq!(d.layers[0].raster.to_rgba(), before);
    }

    #[test]
    fn shrinking_then_growing_back_loses_nothing() {
        let mut d = doc_with_square();
        let before = d.layers[0].raster.to_rgba();
        convert(&mut d, 0);
        let p = d.layers[0].props.smart.as_mut().unwrap();
        p.size = (2.0, 2.0);
        rerender(&mut d, 0);
        let p = d.layers[0].props.smart.as_mut().unwrap();
        p.size = (20.0, 20.0);
        rerender(&mut d, 0);
        assert_eq!(d.layers[0].raster.to_rgba(), before, "drawn fresh from the originals");
    }

    #[test]
    fn canvas_flips_and_resizes_keep_it_smart() {
        let mut d = doc_with_square();
        convert(&mut d, 0);
        crate::ops::flip_horizontal(&mut d);
        assert!(d.layers[0].is_smart());
        // The blue corner pixel went from (10,10) to (53,10).
        assert_eq!(d.layers[0].raster.get_pixel(53, 10), Rgba8::new(0, 0, 255, 255));
        crate::ops::resize_canvas_at(&mut d, 80, 80, 8, 4);
        assert!(d.layers[0].is_smart());
        assert_eq!(d.layers[0].raster.get_pixel(61, 14), Rgba8::new(0, 0, 255, 255));
        crate::ops::resize_image(&mut d, 160, 160, ResizeFilter::Nearest);
        assert!(d.layers[0].is_smart());
        let p = d.layers[0].props.smart.as_ref().unwrap();
        // Doubled: the square is now 40 px across.
        let q = p.corners();
        assert!(((q[1].x - q[0].x).abs() - 40.0).abs() < 0.01, "{q:?}");
    }

    #[test]
    fn moves_carry_the_placement() {
        let mut d = doc_with_square();
        convert(&mut d, 0);
        let rest = crate::moving::begin(&d, &[0]);
        crate::moving::apply(&mut d, &rest, 5, -3);
        rerender(&mut d, 0);
        assert_eq!(d.layers[0].raster.get_pixel(15, 7), Rgba8::new(0, 0, 255, 255));
    }
}
