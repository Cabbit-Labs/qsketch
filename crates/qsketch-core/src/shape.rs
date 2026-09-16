//! Hard-edged (pixel-perfect) rectangles and ellipses.
//!
//! The shape tools paint discrete pixels rather than brush dabs, so both the
//! committed mask and the on-canvas preview come from the same row spans: what
//! the preview outlines is exactly what gets painted.

use crate::{IRect, Mask, Pt};

/// The horizontal run `[x0, x1)` covered on each row of a shape, top row
/// first. An empty run means the row is not covered at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spans {
    /// Document y of the first row.
    pub y0: i32,
    pub rows: Vec<(i32, i32)>,
}

impl Spans {
    pub fn is_empty(&self) -> bool {
        self.rows.iter().all(|(a, b)| b <= a)
    }

    fn row(&self, y: i32) -> (i32, i32) {
        let i = y - self.y0;
        if i < 0 || i as usize >= self.rows.len() {
            return (0, 0);
        }
        self.rows[i as usize]
    }

    pub fn bounds(&self) -> IRect {
        let mut r = IRect::EMPTY;
        for (i, (a, b)) in self.rows.iter().enumerate() {
            if b > a {
                let row = IRect::new(*a, self.y0 + i as i32, b - a, 1);
                r = if r.is_empty() { row } else { r.union(&row) };
            }
        }
        r
    }
}

/// Every pixel of `rect`.
pub fn rect_spans(rect: IRect) -> Spans {
    if rect.is_empty() {
        return Spans::default();
    }
    Spans { y0: rect.y, rows: vec![(rect.x, rect.right()); rect.h as usize] }
}

/// The pixels of the ellipse inscribed in `rect`, with no anti-aliasing: a
/// pixel is in when its center is inside the ellipse. Symmetric in both axes.
pub fn ellipse_spans(rect: IRect) -> Spans {
    if rect.is_empty() {
        return Spans::default();
    }
    let (w, h) = (rect.w as f32, rect.h as f32);
    let (rx, ry) = (w / 2.0, h / 2.0);
    let mut rows = Vec::with_capacity(rect.h as usize);
    for j in 0..rect.h {
        // Distance of this row's center from the ellipse center, normalized.
        let dy = (j as f32 + 0.5 - ry) / ry;
        let t = 1.0 - dy * dy;
        if t <= 0.0 {
            rows.push((0, 0));
            continue;
        }
        // Half-width of the run, in pixel centers.
        let half = rx * t.sqrt();
        // Centers lie at i + 0.5 - rx; the run is the integers with |c| < half.
        let mut a = (rx - half - 0.5).ceil() as i32;
        let mut b = rect.w - a;
        // A row inside the ellipse is never empty: keep at least one pixel.
        if b <= a {
            a = (rect.w - 1) / 2;
            b = a + 1 + (rect.w % 2 == 0) as i32;
        }
        rows.push((rect.x + a.max(0), rect.x + b.min(rect.w)));
    }
    Spans { y0: rect.y, rows }
}

/// Shrink a shape's bounding rect by `thick` pixels on every side; the result
/// is the hole of an outlined shape. Empty when the shape is too small to have
/// one, i.e. the outline is solid.
fn inner_rect(rect: IRect, thick: i32) -> IRect {
    let (w, h) = (rect.w - 2 * thick, rect.h - 2 * thick);
    if w <= 0 || h <= 0 {
        return IRect::EMPTY;
    }
    IRect::new(rect.x + thick, rect.y + thick, w, h)
}

/// Spans of a shape: `(outer, hole)`. The painted pixels are the outer rows
/// minus the hole rows; a filled shape has no hole.
pub fn shape_spans(rect: IRect, ellipse: bool, filled: bool, thick: i32) -> (Spans, Spans) {
    let outer = if ellipse { ellipse_spans(rect) } else { rect_spans(rect) };
    if filled {
        return (outer, Spans::default());
    }
    let ir = inner_rect(rect, thick.max(1));
    let hole = if ir.is_empty() {
        Spans::default()
    } else if ellipse {
        ellipse_spans(ir)
    } else {
        rect_spans(ir)
    };
    (outer, hole)
}

/// A hard mask of `outer` minus `hole`, clipped to the canvas.
pub fn mask_from_spans(width: u32, height: u32, outer: &Spans, hole: &Spans) -> Mask {
    let mut m = Mask::new(width, height);
    let (w, h) = (width as i32, height as i32);
    for (i, (a, b)) in outer.rows.iter().enumerate() {
        let y = outer.y0 + i as i32;
        if y < 0 || y >= h || b <= a {
            continue;
        }
        let (ha, hb) = hole.row(y);
        for x in (*a).max(0)..(*b).min(w) {
            if hb > ha && x >= ha && x < hb {
                continue;
            }
            m.set(x, y, 255);
        }
    }
    m.recompute_bounds();
    m
}

/// The pixel-boundary segments of a span set, for drawing a preview that lines
/// up with the pixels that will be painted.
pub fn spans_outline(s: &Spans) -> Vec<[Pt; 2]> {
    let mut out = Vec::new();
    for (i, (a, b)) in s.rows.iter().enumerate() {
        if b <= a {
            continue;
        }
        let y = s.y0 + i as i32;
        let (a, b) = (*a as f32, *b as f32);
        let (yt, yb) = (y as f32, y as f32 + 1.0);
        out.push([Pt::new(a, yt), Pt::new(a, yb)]);
        out.push([Pt::new(b, yt), Pt::new(b, yb)]);
        let prev = s.rows.get(i.wrapping_sub(1)).copied().unwrap_or((0, 0));
        let next = s.rows.get(i + 1).copied().unwrap_or((0, 0));
        for (x0, x1) in interval_diff((prev.0 as f32, prev.1 as f32), (a, b)) {
            out.push([Pt::new(x0, yt), Pt::new(x1, yt)]);
        }
        for (x0, x1) in interval_diff((next.0 as f32, next.1 as f32), (a, b)) {
            out.push([Pt::new(x0, yb), Pt::new(x1, yb)]);
        }
    }
    out
}

/// `span` minus `other`: the parts of this row not shared with the neighbour,
/// which is where a horizontal pixel edge shows.
fn interval_diff(other: (f32, f32), span: (f32, f32)) -> Vec<(f32, f32)> {
    let (oa, ob) = other;
    let (a, b) = span;
    if ob <= oa || ob <= a || oa >= b {
        return vec![(a, b)];
    }
    let mut out = Vec::new();
    if a < oa {
        out.push((a, oa));
    }
    if ob < b {
        out.push((ob, b));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipse_spans_are_symmetric_and_solid() {
        for (w, h) in [(1, 1), (2, 2), (5, 5), (8, 8), (9, 4), (4, 9), (31, 17)] {
            let s = ellipse_spans(IRect::new(0, 0, w, h));
            assert_eq!(s.rows.len(), h as usize);
            for j in 0..h {
                let (a, b) = s.rows[j as usize];
                let (a2, b2) = s.rows[(h - 1 - j) as usize];
                assert_eq!((a, b), (a2, b2), "row {j} of {w}x{h} is not mirrored vertically");
                assert!(b > a, "row {j} of {w}x{h} is empty");
                assert_eq!(a, w - b, "row {j} of {w}x{h} is not mirrored horizontally");
            }
        }
    }

    #[test]
    fn outline_leaves_a_hole_and_fills_when_too_small() {
        let (outer, hole) = shape_spans(IRect::new(0, 0, 9, 9), true, false, 1);
        let m = mask_from_spans(9, 9, &outer, &hole);
        assert_eq!(m.get(4, 0), 255, "top of the ring is painted");
        assert_eq!(m.get(4, 4), 0, "center is the hole");
        let (outer, hole) = shape_spans(IRect::new(0, 0, 3, 3), false, false, 4);
        assert!(hole.is_empty());
        let m = mask_from_spans(3, 3, &outer, &hole);
        assert_eq!(m.get(1, 1), 255, "a too-thick outline is solid");
    }

    #[test]
    fn preview_outline_traces_the_painted_pixels() {
        let s = rect_spans(IRect::new(2, 3, 4, 2));
        let segs = spans_outline(&s);
        // A 4x2 rect has 12 unit edges on its boundary.
        let total: f32 = segs.iter().map(|[a, b]| (b.x - a.x).abs() + (b.y - a.y).abs()).sum();
        assert_eq!(total, 12.0);
    }
}
