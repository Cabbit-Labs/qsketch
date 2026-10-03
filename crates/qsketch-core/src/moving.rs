//! Whole-layer moves: the Move tool (and its Ctrl+drag quick-move chord)
//! shifting any set of layers and groups together. Every pixel comes along,
//! including what crosses the canvas edge, which is kept outside it (see
//! [`crate::raster`]) so dragging it back in brings it back.

use std::sync::Arc;

use crate::document::DocState;
use crate::geom::IRect;
use crate::layer::{LayerId, LayerKind};
use crate::mask::Mask;
use crate::raster::Raster;

/// The layers a move of `ids` drags along: each id stands for itself and,
/// for a group, every layer inside it. Hidden layers move too; layers that
/// are locked (or inside a locked group) and tilemaps, which are pinned to
/// their cell grid, stay put.
#[derive(Debug, Default, PartialEq)]
pub struct Targets {
    /// Layer indices, bottom to top.
    pub layers: Vec<usize>,
    pub locked: usize,
    pub tilemaps: usize,
}

pub fn targets(doc: &DocState, ids: &[LayerId]) -> Targets {
    let mut t = Targets::default();
    let mut seen = Vec::new();
    for &id in ids {
        let Some(idx) = doc.index_of(id) else { continue };
        for i in doc.block(idx) {
            if seen.contains(&i) {
                continue;
            }
            seen.push(i);
            let l = &doc.layers[i];
            // Groups and adjustment layers own no pixels; their masks move.
            if !(l.owns_pixels() || l.mask.is_some()) {
                continue;
            }
            let locked =
                l.props.locked || doc.ancestors(i).iter().any(|g| doc.layer_by_id(*g).is_some_and(|g| g.props.locked));
            if locked {
                t.locked += 1;
            } else if l.props.kind == LayerKind::Tilemap {
                t.tilemaps += 1;
            } else {
                t.layers.push(i);
            }
        }
    }
    t.layers.sort_unstable();
    t
}

/// One layer as it was when the move started.
#[derive(Clone)]
pub struct AtRest {
    pub idx: usize,
    raster: Raster,
    mask: Option<Arc<Mask>>,
    shape: Option<Vec<[i32; 2]>>,
    text_anchor: Option<(i32, i32)>,
    /// Where it has pixels or mask coverage, on and off the canvas.
    extent: IRect,
}

/// Snapshot `layers` before moving them.
pub fn begin(doc: &DocState, layers: &[usize]) -> Vec<AtRest> {
    layers
        .iter()
        .filter_map(|&idx| {
            let l = doc.layers.get(idx)?;
            let mut extent = l.raster.full_bounds().unwrap_or(IRect::EMPTY);
            if let Some(m) = &l.mask {
                extent = extent.union(&m.extent());
            }
            // Layer effects (shadows, glows), its own or an enclosing
            // group's, repaint around the content.
            let reach = doc
                .ancestors(idx)
                .iter()
                .filter_map(|g| doc.layer_by_id(*g))
                .map(|g| g.props.style.reach())
                .fold(l.props.style.reach(), i32::max);
            if reach > 0 && !extent.is_empty() {
                extent = extent.expand(reach);
            }
            Some(AtRest {
                idx,
                raster: l.raster.clone(),
                mask: l.mask.clone(),
                shape: l.props.shape.as_ref().map(|s| s.points.clone()),
                text_anchor: l.props.text.as_ref().map(|t| t.anchor),
                extent,
            })
        })
        .collect()
}

/// The canvas area the snapshots cover when offset by `(dx, dy)`.
pub fn area(rest: &[AtRest], dx: i32, dy: i32, canvas: IRect) -> IRect {
    rest.iter().fold(IRect::EMPTY, |a, r| a.union(&r.extent.translate(dx, dy).intersect(&canvas)))
}

/// Put every snapshot back offset by `(dx, dy)` from where it started.
pub fn apply(doc: &mut DocState, rest: &[AtRest], dx: i32, dy: i32) {
    for r in rest {
        let Some(l) = doc.layers.get_mut(r.idx) else { continue };
        l.raster = if (dx, dy) == (0, 0) { r.raster.clone() } else { r.raster.shifted_keep(dx, dy) };
        l.mask = r.mask.as_ref().map(|m| if (dx, dy) == (0, 0) { m.clone() } else { Arc::new(m.shifted_keep(dx, dy)) });
        if let (Some(s), Some(pts)) = (l.props.shape.as_mut(), &r.shape) {
            s.points = pts.iter().map(|p| [p[0] + dx, p[1] + dy]).collect();
        }
        if let (Some(t), Some(a)) = (l.props.text.as_mut(), r.text_anchor) {
            t.anchor = (a.0 + dx, a.1 + dy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;
    use crate::layer::Layer;

    fn doc_with_group() -> DocState {
        // Bottom to top: a (in group), b (hidden, in group), group, c.
        let mut d = DocState::new(16, 16, None);
        d.layers.clear();
        let mut a = Layer::new(1, "a", 16, 16);
        a.raster.set_pixel(0, 0, Rgba8::WHITE);
        let mut b = Layer::new(2, "b", 16, 16);
        b.props.visible = false;
        b.raster.set_pixel(15, 15, Rgba8::BLACK);
        let mut g = Layer::new(3, "g", 16, 16);
        g.props.kind = LayerKind::Group;
        a.props.parent = Some(3);
        b.props.parent = Some(3);
        let c = Layer::new(4, "c", 16, 16);
        d.layers = vec![a, b, g, c];
        d.active = 2;
        d
    }

    #[test]
    fn group_moves_every_member_and_keeps_offcanvas_pixels() {
        let mut d = doc_with_group();
        let t = targets(&d, &[3]);
        assert_eq!(t.layers, vec![0, 1], "both members, hidden one included; the group itself owns no pixels");
        let rest = begin(&d, &t.layers);
        // Off the left/top edge and back: nothing is lost.
        apply(&mut d, &rest, -5, -5);
        assert_eq!(d.layers[0].raster.get_pixel(0, 0), Rgba8::TRANSPARENT);
        assert_eq!(d.layers[0].raster.get_pixel_any(-5, -5), Rgba8::WHITE);
        assert_eq!(d.layers[1].raster.get_pixel(10, 10), Rgba8::BLACK);
        let moved = begin(&d, &t.layers);
        apply(&mut d, &moved, 5, 5);
        assert_eq!(d.layers[0].raster.get_pixel(0, 0), Rgba8::WHITE);
        assert!(!d.layers[0].raster.has_outside());
        assert_eq!(d.layers[1].raster.get_pixel(15, 15), Rgba8::BLACK);
    }

    #[test]
    fn locked_layers_and_locked_groups_stay_put() {
        let mut d = doc_with_group();
        d.layers[2].props.locked = true;
        let t = targets(&d, &[3, 4]);
        assert_eq!(t.layers, vec![3]);
        assert_eq!(t.locked, 2);
    }

    #[test]
    fn masks_shapes_and_text_follow() {
        let mut d = doc_with_group();
        let mut m = Mask::new(16, 16);
        m.set(15, 0, 255);
        m.recompute_bounds();
        d.layers[0].mask = Some(Arc::new(m));
        d.layers[0].props.text = Some(crate::text::TextLayer { anchor: (2, 3), ..Default::default() });
        let rest = begin(&d, &[0]);
        apply(&mut d, &rest, 4, 0);
        let m = d.layers[0].mask.as_ref().unwrap();
        assert_eq!(m.get_any(19, 0), 255);
        assert_eq!(d.layers[0].props.text.as_ref().unwrap().anchor, (6, 3));
        let back = begin(&d, &[0]);
        apply(&mut d, &back, -4, 0);
        assert_eq!(d.layers[0].mask.as_ref().unwrap().get(15, 0), 255);
        assert!(!d.layers[0].mask.as_ref().unwrap().has_outside());
    }
}
