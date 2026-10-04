//! Image tracing: turn pixels into filled vector shapes (Bézier outlines),
//! the "posterize, then trace" route to edges that stay smooth at any size.
//!
//! Tracing runs visioncortex's color clustering and curve fitting, following
//! the pipeline of its VTracer front end (MIT OR Apache-2.0,
//! <https://github.com/visioncortex/vtracer>). The result is a [`VectorArt`]:
//! shapes in paint order, each a color and closed outlines of lines and
//! cubic curves. It is drawn anti-aliased with tiny-skia, through any point
//! mapping (so a smart object's box, corners or warp lattice bend the
//! outlines, not pixels), and written out as SVG.

use serde::{Deserialize, Serialize};
use visioncortex::color_clusters::{KeyingAction, Runner, RunnerConfig, HIERARCHICAL_MAX};
use visioncortex::{BinaryImage, Color, ColorImage, CompoundPath, CompoundPathElement, PathSimplifyMode};

use crate::color::Rgba8;
use crate::geom::{IRect, Pt};
use crate::raster::Raster;

/// One step of an outline.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Seg {
    Line(Pt),
    /// Control points, then the end point.
    Cubic(Pt, Pt, Pt),
}

/// A closed outline: a start point and the steps around back to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Outline {
    pub start: Pt,
    pub segs: Vec<Seg>,
}

/// A filled region: one color, outlines filled with the nonzero rule
/// (holes run the other way round).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub color: Rgba8,
    pub outlines: Vec<Outline>,
}

/// Traced artwork in its own coordinate space (`0..width`, `0..height`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VectorArt {
    pub width: u32,
    pub height: u32,
    /// Bottom to top.
    pub shapes: Vec<Shape>,
}

/// Which colors become shapes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TraceColors {
    /// Every color region, stacked.
    #[default]
    Color,
    /// Dark pixels only, as black shapes (`threshold` on luminance).
    BlackWhite,
}

/// How outlines follow the pixel edges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TraceCurves {
    /// Smooth Bézier curves with corners kept where the edge turns sharply.
    #[default]
    Smooth,
    /// Straight segments.
    Polygon,
    /// Exactly along the pixel edges (crisp pixel-art blocks).
    Pixel,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TraceSettings {
    pub colors: TraceColors,
    pub curves: TraceCurves,
    /// Color precision, 1..=8 bits per channel (higher keeps more shades).
    pub color_precision: i32,
    /// How different stacked color layers must be (higher merges more).
    pub gradient_step: i32,
    /// Regions smaller than this many pixels across are dropped.
    pub speckle: u32,
    /// Corners sharper than this many degrees stay corners.
    pub corner_threshold: i32,
    /// Shorter segments are merged (more = smoother, fewer points).
    pub segment_length: f32,
    /// Black & white: luminance below which a pixel counts as ink, 0..=255.
    pub threshold: u8,
}

impl Default for TraceSettings {
    fn default() -> Self {
        Self {
            colors: TraceColors::Color,
            curves: TraceCurves::Smooth,
            color_precision: 6,
            gradient_step: 16,
            speckle: 4,
            corner_threshold: 60,
            segment_length: 4.0,
            threshold: 128,
        }
    }
}

/// Pixels with less coverage than this count as empty when tracing, so a
/// soft anti-aliased rim doesn't become a halo of its own.
const ALPHA_CUTOFF: u8 = 128;

/// Trace `src` (straight RGBA, any size) into shapes.
pub fn trace(src: &Raster, s: &TraceSettings) -> VectorArt {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let mut art = VectorArt { width: src.width(), height: src.height(), shapes: Vec::new() };
    if w == 0 || h == 0 {
        return art;
    }
    let mode = match s.curves {
        TraceCurves::Smooth => PathSimplifyMode::Spline,
        TraceCurves::Polygon => PathSimplifyMode::Polygon,
        TraceCurves::Pixel => PathSimplifyMode::None,
    };
    let corner = (s.corner_threshold.clamp(0, 180) as f64).to_radians();
    let length = s.segment_length.clamp(3.5, 10.0) as f64;
    let splice = 45f64.to_radians();
    let speckle = (s.speckle.max(1) * s.speckle.max(1)) as usize;
    let rgba = src.to_rgba();
    match s.colors {
        TraceColors::BlackWhite => {
            let mut img = BinaryImage::new_w_h(w, h);
            for (i, p) in rgba.chunks_exact(4).enumerate() {
                let lum = (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000;
                img.set_pixel(i % w, i / w, p[3] >= ALPHA_CUTOFF && lum < s.threshold as u32);
            }
            let clusters = img.to_clusters(false);
            for i in 0..clusters.len() {
                let c = clusters.get_cluster(i);
                if c.size() >= speckle {
                    let path = c.to_compound_path(mode, corner, length, 10, splice);
                    push_shape(&mut art, &path, Rgba8::BLACK);
                }
            }
        }
        TraceColors::Color => {
            // Empty pixels get a color nothing else uses and are keyed out.
            let key = unused_color(&rgba);
            let mut pixels = rgba;
            let mut keyed = false;
            for p in pixels.chunks_exact_mut(4) {
                if p[3] < ALPHA_CUTOFF {
                    p.copy_from_slice(&[key.r, key.g, key.b, 255]);
                    keyed = true;
                } else {
                    p[3] = 255;
                }
            }
            let img = ColorImage { pixels, width: w, height: h };
            let runner = Runner::new(
                RunnerConfig {
                    diagonal: s.gradient_step == 0,
                    hierarchical: HIERARCHICAL_MAX,
                    batch_size: 25600,
                    good_min_area: speckle,
                    good_max_area: w * h,
                    is_same_color_a: 8 - s.color_precision.clamp(1, 8),
                    is_same_color_b: 1,
                    deepen_diff: s.gradient_step.clamp(0, 255),
                    hollow_neighbours: 1,
                    key_color: if keyed { key } else { Color::default() },
                    keying_action: KeyingAction::Discard,
                },
                img,
            );
            let clusters = runner.run();
            let view = clusters.view();
            for &ci in view.clusters_output.iter().rev() {
                let c = view.get_cluster(ci);
                let path = c.to_compound_path(&view, false, mode, corner, length, 10, splice);
                let col = c.residue_color();
                push_shape(&mut art, &path, Rgba8::new(col.r, col.g, col.b, 255));
            }
        }
    }
    art
}

/// A color that appears nowhere in `rgba` (to key out empty pixels).
fn unused_color(rgba: &[u8]) -> Color {
    let mut seen = std::collections::HashSet::new();
    for p in rgba.chunks_exact(4) {
        seen.insert((p[0], p[1], p[2]));
    }
    let candidates = [(255u8, 0u8, 255u8), (0, 255, 0), (0, 255, 255), (255, 255, 0), (1, 2, 3)];
    let (r, g, b) = candidates
        .into_iter()
        .chain((0..=255u8).flat_map(|r| (0..=255u8).map(move |g| (r, g, 7))))
        .find(|c| !seen.contains(c))
        .unwrap_or((255, 0, 255));
    Color::new(r, g, b)
}

fn push_shape(art: &mut VectorArt, path: &CompoundPath, color: Rgba8) {
    let mut outlines = Vec::new();
    for el in &path.paths {
        let o = match el {
            CompoundPathElement::PathI32(p) => polygon(p.path.iter().map(|q| Pt::new(q.x as f32, q.y as f32))),
            CompoundPathElement::PathF64(p) => polygon(p.path.iter().map(|q| Pt::new(q.x as f32, q.y as f32))),
            CompoundPathElement::Spline(sp) => {
                let pts: Vec<Pt> = sp.points.iter().map(|q| Pt::new(q.x as f32, q.y as f32)).collect();
                if pts.len() < 4 {
                    None
                } else {
                    let segs = pts[1..].chunks_exact(3).map(|c| Seg::Cubic(c[0], c[1], c[2])).collect::<Vec<_>>();
                    Some(Outline { start: pts[0], segs })
                }
            }
        };
        if let Some(o) = o.filter(|o| !o.segs.is_empty()) {
            outlines.push(o);
        }
    }
    if !outlines.is_empty() {
        art.shapes.push(Shape { color, outlines });
    }
}

fn polygon(mut pts: impl Iterator<Item = Pt>) -> Option<Outline> {
    let start = pts.next()?;
    let segs: Vec<Seg> = pts.map(Seg::Line).collect();
    Some(Outline { start, segs })
}

impl VectorArt {
    /// Number of points (for a size readout).
    pub fn point_count(&self) -> usize {
        self.shapes
            .iter()
            .flat_map(|s| &s.outlines)
            .map(|o| 1 + o.segs.iter().map(|s| if matches!(s, Seg::Cubic(..)) { 3 } else { 1 }).sum::<usize>())
            .sum()
    }

    /// Draw the art into `out` (a canvas-sized raster), every point sent
    /// through `map` first (from art space to canvas space). `scale` is
    /// roughly how much `map` enlarges, so curves are split finely enough.
    pub fn render_into(&self, out: &mut Raster, map: &dyn Fn(Pt) -> Pt, scale: f32) {
        let (w, h) = (out.width(), out.height());
        let Some(mut pm) = tiny_skia::Pixmap::new(w.max(1), h.max(1)) else { return };
        for shape in &self.shapes {
            let mut pb = tiny_skia::PathBuilder::new();
            for o in &shape.outlines {
                let s = map(o.start);
                pb.move_to(s.x, s.y);
                let mut cur = o.start;
                for seg in &o.segs {
                    match *seg {
                        Seg::Line(p) => {
                            let q = map(p);
                            pb.line_to(q.x, q.y);
                            cur = p;
                        }
                        Seg::Cubic(c1, c2, p) => {
                            // Split in art space, then map each point: right
                            // for any mapping, projective or warped.
                            let len = cur.dist(c1) + c1.dist(c2) + c2.dist(p);
                            let n = ((len * scale.max(0.05) / 2.0).ceil() as usize).clamp(2, 128);
                            for k in 1..=n {
                                let t = k as f32 / n as f32;
                                let q = map(cubic_at(cur, c1, c2, p, t));
                                pb.line_to(q.x, q.y);
                            }
                            cur = p;
                        }
                    }
                }
                pb.close();
            }
            let Some(path) = pb.finish() else { continue };
            let mut paint = tiny_skia::Paint::default();
            paint.set_color_rgba8(shape.color.r, shape.color.g, shape.color.b, shape.color.a);
            paint.anti_alias = true;
            pm.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
        }
        // Premultiplied pixmap → straight raster, over what `out` holds.
        let data = pm.data();
        let mut rgba = out.to_rgba();
        for (d, s) in rgba.chunks_exact_mut(4).zip(data.chunks_exact(4)) {
            let a = s[3] as u32;
            if a == 0 {
                continue;
            }
            let un = |c: u8| ((c as u32 * 255 + a / 2) / a).min(255) as u8;
            let src = [un(s[0]), un(s[1]), un(s[2]), s[3]];
            if d[3] == 0 || a == 255 {
                d.copy_from_slice(&src);
            } else {
                let o = crate::blend::src_over(
                    Rgba8::from_array(*<&[u8; 4]>::try_from(&*d).unwrap()).to_f32(),
                    Rgba8::from_array(src).to_f32(),
                );
                d.copy_from_slice(&Rgba8::from_f32(o).to_array());
            }
        }
        *out = Raster::from_rgba(w, h, &rgba);
    }

    /// The art drawn at its own size (1 art unit = 1 pixel).
    pub fn render(&self) -> Raster {
        let mut r = Raster::new(self.width.max(1), self.height.max(1));
        self.render_into(&mut r, &|p| p, 1.0);
        r
    }

    /// SVG paths for the art sent through `map` (`affine` = the same map as
    /// an SVG `matrix(a b c d e f)`, which keeps the curves exact; without
    /// it the curves are written as fine polylines through `map`).
    pub fn svg_paths(&self, map: &dyn Fn(Pt) -> Pt, affine: Option<[f32; 6]>, scale: f32) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let f = |v: f32| format!("{:.2}", v).trim_end_matches('0').trim_end_matches('.').to_string();
        for shape in &self.shapes {
            let mut d = String::new();
            for o in &shape.outlines {
                let (s, mut cur) = (if affine.is_some() { o.start } else { map(o.start) }, o.start);
                let _ = write!(d, "M{} {}", f(s.x), f(s.y));
                for seg in &o.segs {
                    match (*seg, affine.is_some()) {
                        (Seg::Line(p), true) => {
                            let _ = write!(d, "L{} {}", f(p.x), f(p.y));
                        }
                        (Seg::Cubic(a, b, p), true) => {
                            let _ = write!(d, "C{} {} {} {} {} {}", f(a.x), f(a.y), f(b.x), f(b.y), f(p.x), f(p.y));
                        }
                        (Seg::Line(p), false) => {
                            let q = map(p);
                            let _ = write!(d, "L{} {}", f(q.x), f(q.y));
                        }
                        (Seg::Cubic(a, b, p), false) => {
                            let len = cur.dist(a) + a.dist(b) + b.dist(p);
                            let n = ((len * scale.max(0.05) / 2.0).ceil() as usize).clamp(2, 128);
                            for k in 1..=n {
                                let q = map(cubic_at(cur, a, b, p, k as f32 / n as f32));
                                let _ = write!(d, "L{} {}", f(q.x), f(q.y));
                            }
                        }
                    }
                    cur = match *seg {
                        Seg::Line(p) | Seg::Cubic(_, _, p) => p,
                    };
                }
                d.push('Z');
            }
            let c = shape.color;
            let opacity =
                if c.a < 255 { format!(" fill-opacity=\"{:.3}\"", c.a as f32 / 255.0) } else { String::new() };
            let transform = affine
                .map(|m| {
                    format!(
                        " transform=\"matrix({} {} {} {} {} {})\"",
                        f(m[0]),
                        f(m[1]),
                        f(m[2]),
                        f(m[3]),
                        f(m[4]),
                        f(m[5])
                    )
                })
                .unwrap_or_default();
            let _ =
                writeln!(out, "  <path d=\"{d}\" fill=\"#{:02x}{:02x}{:02x}\"{opacity}{transform}/>", c.r, c.g, c.b);
        }
        out
    }
}

/// Point at `t` on a cubic Bézier.
fn cubic_at(p0: Pt, p1: Pt, p2: Pt, p3: Pt, t: f32) -> Pt {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Pt::new(a * p0.x + b * p1.x + c * p2.x + d * p3.x, a * p0.y + b * p1.y + c * p2.y + d * p3.y)
}

/// A whole SVG document `width`×`height` around `body` (from
/// [`VectorArt::svg_paths`]).
pub fn svg_document(width: u32, height: u32, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- Generator: qsketch -->\n<svg version=\"1.1\" xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">\n{body}</svg>\n"
    )
}

/// The bounding rect the art covers once sent through `map`.
pub fn mapped_bounds(art: &VectorArt, map: &dyn Fn(Pt) -> Pt) -> IRect {
    let (w, h) = (art.width as f32, art.height as f32);
    let pts = [Pt::new(0.0, 0.0), Pt::new(w, 0.0), Pt::new(w, h), Pt::new(0.0, h)].map(map);
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pts {
        (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
    }
    IRect::from_f32_bounds(x0, y0, x1, y1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pixelated circle-ish blob on a transparent background.
    fn blob() -> Raster {
        let mut r = Raster::new(40, 40);
        for y in 0..40 {
            for x in 0..40 {
                let (dx, dy) = (x as f32 - 19.5, y as f32 - 19.5);
                if dx * dx + dy * dy < 14.0 * 14.0 {
                    r.set_pixel(x, y, Rgba8::new(200, 30, 60, 255));
                }
            }
        }
        r
    }

    #[test]
    fn traces_a_blob_into_one_smooth_red_shape() {
        let art = trace(&blob(), &TraceSettings::default());
        assert_eq!(art.shapes.len(), 1, "transparent background keyed out");
        assert_eq!(art.shapes[0].color, Rgba8::new(200, 30, 60, 255));
        assert!(art.shapes[0].outlines[0].segs.iter().any(|s| matches!(s, Seg::Cubic(..))));
        // Drawn back at its own size it covers the blob.
        let r = art.render();
        assert_eq!(r.get_pixel(20, 20), Rgba8::new(200, 30, 60, 255));
        assert_eq!(r.get_pixel(1, 1).a, 0);
    }

    #[test]
    fn scales_up_with_smooth_edges() {
        let art = trace(&blob(), &TraceSettings::default());
        let mut big = Raster::new(320, 320);
        art.render_into(&mut big, &|p| Pt::new(p.x * 8.0, p.y * 8.0), 8.0);
        // The rim is anti-aliased (partial alpha somewhere), not 8×8 blocks.
        let partial = (0..320).any(|x| {
            let a = big.get_pixel(x, 160).a;
            a > 0 && a < 255
        });
        assert!(partial);
        assert_eq!(big.get_pixel(160, 160).a, 255);
        let svg = svg_document(320, 320, &art.svg_paths(&|p| p, Some([8.0, 0.0, 0.0, 8.0, 0.0, 0.0]), 8.0));
        assert!(svg.contains("<path d=\"M") && svg.contains(" C") || svg.contains("C"));
        assert!(svg.contains("fill=\"#c81e3c\""));
    }

    #[test]
    fn black_and_white_keeps_only_ink() {
        let mut r = blob();
        r.fill_rect(IRect::new(0, 0, 6, 6), Rgba8::new(250, 250, 250, 255));
        let s = TraceSettings { colors: TraceColors::BlackWhite, threshold: 200, ..Default::default() };
        let art = trace(&r, &s);
        assert!(art.shapes.iter().all(|s| s.color == Rgba8::BLACK));
        assert!(!art.shapes.is_empty());
    }
}
