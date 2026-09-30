//! Slices: named rectangles of the canvas, Aseprite-style, for UI parts and
//! sprite sub-images. A slice can carry a 9-slice center (the stretchable
//! middle of a panel or button) and a pivot point. Slices are part of the
//! document state (undoable), saved in `.qsk` and in Aseprite files, and
//! File ▸ Export Slices writes each one as a PNG plus a JSON description.

use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::color::Rgba8;
use crate::document::DocState;
use crate::geom::IRect;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slice {
    pub name: String,
    /// Canvas pixels.
    pub rect: IRect,
    /// 9-slice center, relative to `rect`'s top-left corner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<IRect>,
    /// Pivot point, relative to `rect`'s top-left corner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot: Option<(i32, i32)>,
    /// Outline color on the canvas.
    #[serde(default = "default_color")]
    pub color: Rgba8,
}

pub fn default_color() -> Rgba8 {
    Rgba8::new(0, 120, 255, 255)
}

impl Slice {
    pub fn new(name: impl Into<String>, rect: IRect) -> Self {
        Self { name: name.into(), rect, center: None, pivot: None, color: default_color() }
    }

    /// Keep the 9-slice center and pivot inside the slice after it changed size.
    pub fn clamp_parts(&mut self) {
        let (w, h) = (self.rect.w.max(1), self.rect.h.max(1));
        if let Some(c) = &mut self.center {
            let inner = IRect::new(0, 0, w, h).intersect(c);
            if inner.is_empty() {
                self.center = None;
            } else {
                *c = inner;
            }
        }
        if let Some((px, py)) = &mut self.pivot {
            *px = (*px).clamp(0, w);
            *py = (*py).clamp(0, h);
        }
    }
}

/// `base`, or `base 2`, `base 3`… so it differs from every slice's name.
pub fn unique_name(slices: &[Slice], base: &str) -> String {
    if !slices.iter().any(|s| s.name == base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base} {n}")).find(|n| !slices.iter().any(|s| &s.name == n)).unwrap_or_default()
}

/// Move every slice by `(dx, dy)` and drop the ones that leave a `w × h`
/// canvas entirely (canvas crops and resizes that shift the origin).
pub fn offset_all(doc: &mut DocState, dx: i32, dy: i32) {
    let canvas = IRect::new(0, 0, doc.width as i32, doc.height as i32);
    doc.slices.retain_mut(|s| {
        s.rect.x += dx;
        s.rect.y += dy;
        let clipped = s.rect.intersect(&canvas);
        if clipped.is_empty() {
            return false;
        }
        // Keep the parts where they were on the canvas.
        let (cx, cy) = (clipped.x - s.rect.x, clipped.y - s.rect.y);
        if let Some(c) = &mut s.center {
            c.x -= cx;
            c.y -= cy;
        }
        if let Some((px, py)) = &mut s.pivot {
            *px -= cx;
            *py -= cy;
        }
        s.rect = clipped;
        s.clamp_parts();
        true
    });
}

/// Scale every slice by `(sx, sy)` (Image Size).
pub fn scale_all(doc: &mut DocState, sx: f32, sy: f32) {
    let r = |v: i32, k: f32| (v as f32 * k).round() as i32;
    for s in &mut doc.slices {
        s.rect = IRect::new(r(s.rect.x, sx), r(s.rect.y, sy), r(s.rect.w, sx).max(1), r(s.rect.h, sy).max(1));
        if let Some(c) = &mut s.center {
            *c = IRect::new(r(c.x, sx), r(c.y, sy), r(c.w, sx).max(1), r(c.h, sy).max(1));
        }
        if let Some((px, py)) = &mut s.pivot {
            *px = r(*px, sx);
            *py = r(*py, sy);
        }
        s.clamp_parts();
    }
}

/// Map every slice through a whole-canvas flip or quarter turn. `map` takes
/// a canvas rect (in the old canvas) to the new canvas; parts are remapped
/// through the same function in canvas space.
pub fn transform_all(doc: &mut DocState, map: impl Fn(IRect) -> IRect, map_pt: impl Fn(i32, i32) -> (i32, i32)) {
    for s in &mut doc.slices {
        let old = s.rect;
        let new = map(old);
        if let Some(c) = &mut s.center {
            let abs = IRect::new(old.x + c.x, old.y + c.y, c.w, c.h);
            let m = map(abs);
            *c = IRect::new(m.x - new.x, m.y - new.y, m.w, m.h);
        }
        if let Some((px, py)) = &mut s.pivot {
            let (ax, ay) = map_pt(old.x + *px, old.y + *py);
            *px = ax - new.x;
            *py = ay - new.y;
        }
        s.rect = new;
        s.clamp_parts();
    }
}

#[derive(Serialize)]
struct JsonSlice<'a> {
    name: &'a str,
    file: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    center: Option<[i32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pivot: Option<[i32; 2]>,
}

/// Write each slice of `doc` (flattened) as `<name>.png` into `dir`, plus
/// `slices.json` listing their rects, 9-slice centers and pivots. Returns how
/// many slices were written.
pub fn export(doc: &DocState, dir: &Path) -> anyhow::Result<usize> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let flat = crate::composite::flatten(doc);
    let mut used: Vec<String> = Vec::new();
    let mut entries = Vec::new();
    for s in &doc.slices {
        let mut stem: String =
            s.name.chars().map(|c| if c.is_alphanumeric() || "-_ ".contains(c) { c } else { '_' }).collect();
        if stem.trim().is_empty() {
            stem = "slice".into();
        }
        let mut file = format!("{stem}.png");
        let mut n = 2;
        while used.contains(&file) {
            file = format!("{stem} {n}.png");
            n += 1;
        }
        used.push(file.clone());
        let part = flat.crop(s.rect);
        crate::io::image_io::export_raster(&dir.join(&file), &part)?;
        entries.push(JsonSlice {
            name: &s.name,
            file,
            x: s.rect.x,
            y: s.rect.y,
            w: s.rect.w,
            h: s.rect.h,
            center: s.center.map(|c| [c.x, c.y, c.w, c.h]),
            pivot: s.pivot.map(|(x, y)| [x, y]),
        });
    }
    let json = serde_json::json!({ "width": doc.width, "height": doc.height, "slices": entries });
    std::fs::write(dir.join("slices.json"), serde_json::to_vec_pretty(&json)?)?;
    Ok(entries.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique() {
        let v = vec![Slice::new("Slice", IRect::new(0, 0, 1, 1)), Slice::new("Slice 2", IRect::new(0, 0, 1, 1))];
        assert_eq!(unique_name(&v, "Slice"), "Slice 3");
        assert_eq!(unique_name(&v, "Button"), "Button");
    }

    #[test]
    fn offset_clips_and_drops() {
        let mut doc = DocState::new(100, 100, None);
        let mut s = Slice::new("a", IRect::new(10, 10, 30, 30));
        s.center = Some(IRect::new(5, 5, 20, 20));
        doc.slices = vec![s, Slice::new("gone", IRect::new(0, 0, 5, 5))];
        offset_all(&mut doc, -20, -20);
        assert_eq!(doc.slices.len(), 1);
        assert_eq!(doc.slices[0].rect, IRect::new(0, 0, 20, 20));
        // The center kept its canvas position: it started at (15,15) → (-5,-5).
        assert_eq!(doc.slices[0].center, Some(IRect::new(0, 0, 15, 15)));
    }

    #[test]
    fn export_writes_pngs_and_json() {
        let mut doc = DocState::new(40, 20, Some(Rgba8::new(255, 0, 0, 255)));
        doc.slices = vec![Slice::new("left", IRect::new(0, 0, 20, 20)), Slice::new("left", IRect::new(20, 0, 20, 20))];
        let dir = std::env::temp_dir().join(format!("qsk-slices-{}", std::process::id()));
        let n = export(&doc, &dir).unwrap();
        assert_eq!(n, 2);
        assert!(dir.join("left.png").exists() && dir.join("left 2.png").exists());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("slices.json")).unwrap()).unwrap();
        assert_eq!(json["slices"][1]["x"], 20);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
