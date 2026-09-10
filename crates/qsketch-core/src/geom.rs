//! Integer rectangles and float points used throughout the engine.

use serde::{Deserialize, Serialize};

/// A 2D point in document (pixel) space. Pixel `(x, y)` covers `[x, x+1) × [y, y+1)`
/// and its center is at `(x + 0.5, y + 0.5)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn dist(self, o: Pt) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
    pub fn lerp(self, o: Pt, t: f32) -> Pt {
        Pt::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }
}

impl From<(f32, f32)> for Pt {
    fn from((x, y): (f32, f32)) -> Self {
        Pt { x, y }
    }
}
impl From<[f32; 2]> for Pt {
    fn from([x, y]: [f32; 2]) -> Self {
        Pt { x, y }
    }
}

/// Axis-aligned integer rectangle. `w`/`h` may be zero (empty) but never negative
/// after normalization through the constructors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    pub const EMPTY: IRect = IRect { x: 0, y: 0, w: 0, h: 0 };

    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// From an inclusive min corner and an *exclusive* max corner.
    pub fn from_min_max(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x: x0, y: y0, w: (x1 - x0).max(0), h: (y1 - y0).max(0) }
    }

    /// Smallest integer rect containing the float bounding box.
    pub fn from_f32_bounds(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        let (x0, x1) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
        let (y0, y1) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        Self::from_min_max(x0.floor() as i32, y0.floor() as i32, x1.ceil() as i32, y1.ceil() as i32)
    }

    /// Rect spanned by two arbitrary corner points (inclusive of both pixels).
    pub fn from_corners(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        let (x0, x1) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
        let (y0, y1) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        Self::from_min_max(x0, y0, x1 + 1, y1 + 1)
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn area(&self) -> i64 {
        if self.is_empty() {
            0
        } else {
            self.w as i64 * self.h as i64
        }
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
    pub fn center(&self) -> Pt {
        Pt::new(self.x as f32 + self.w as f32 / 2.0, self.y as f32 + self.h as f32 / 2.0)
    }

    pub fn intersect(&self, o: &IRect) -> IRect {
        if self.is_empty() || o.is_empty() {
            return IRect::EMPTY;
        }
        let r = IRect::from_min_max(
            self.x.max(o.x),
            self.y.max(o.y),
            self.right().min(o.right()),
            self.bottom().min(o.bottom()),
        );
        if r.is_empty() {
            IRect::EMPTY
        } else {
            r
        }
    }

    pub fn union(&self, o: &IRect) -> IRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        IRect::from_min_max(
            self.x.min(o.x),
            self.y.min(o.y),
            self.right().max(o.right()),
            self.bottom().max(o.bottom()),
        )
    }

    pub fn translate(&self, dx: i32, dy: i32) -> IRect {
        IRect { x: self.x + dx, y: self.y + dy, ..*self }
    }

    pub fn expand(&self, by: i32) -> IRect {
        IRect::from_min_max(self.x - by, self.y - by, self.right() + by, self.bottom() + by)
    }

    /// Iterate all `(x, y)` in the rect, row-major.
    pub fn iter(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        (self.y..self.bottom()).flat_map(move |y| (self.x..self.right()).map(move |x| (x, y)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersect_union() {
        let a = IRect::new(0, 0, 10, 10);
        let b = IRect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(&b), IRect::new(5, 5, 5, 5));
        assert_eq!(a.union(&b), IRect::new(0, 0, 15, 15));
        assert!(a.intersect(&IRect::new(20, 20, 1, 1)).is_empty());
        assert_eq!(IRect::EMPTY.union(&b), b);
    }

    #[test]
    fn f32_bounds() {
        let r = IRect::from_f32_bounds(1.2, 3.7, -0.5, 2.0);
        assert_eq!(r, IRect::from_min_max(-1, 2, 2, 4));
    }
}
