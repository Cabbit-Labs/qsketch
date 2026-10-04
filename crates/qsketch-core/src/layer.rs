//! A raster layer with its Photoshop-style properties.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::blend::BlendMode;
use crate::mask::Mask;
use crate::raster::Raster;

pub type LayerId = u64;

/// Serializable layer properties (everything except pixels).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerProps {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    /// Fully locked: no painting, moving or editing.
    pub locked: bool,
    /// Transparency locked: painting only affects already-opaque pixels.
    pub alpha_locked: bool,
    /// 0..=1
    pub opacity: f32,
    pub blend: BlendMode,
    /// Clip this layer to the alpha of the layer below (clipping mask).
    #[serde(default)]
    pub clipped: bool,
    /// Raster layer or a group folder (a group owns no pixels of its own).
    #[serde(default)]
    pub kind: LayerKind,
    /// The group this layer sits in, if any. A group's members are the
    /// contiguous run of layers immediately below its entry in the stack.
    #[serde(default)]
    pub parent: Option<LayerId>,
    /// Groups only: whether the Layers panel shows the members.
    #[serde(default = "default_true")]
    pub expanded: bool,
    /// Whether the layer mask (if any) is in effect; off = "disable mask",
    /// which keeps it around without hiding anything.
    #[serde(default = "default_true")]
    pub mask_enabled: bool,
    /// Layer effects (drop shadow, stroke, glows, color overlay); drawn by
    /// the compositor, never baked into the pixels.
    #[serde(default, skip_serializing_if = "crate::style::LayerStyle::is_off")]
    pub style: crate::style::LayerStyle,
    /// Adjustment layers only: the color adjustment they apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjustment: Option<crate::filter::Filter>,
    /// Tilemap layers only: which tile each cell shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilemap: Option<crate::tilemap::Tilemap>,
    /// Shape layers only: the polygon the pixels are drawn from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<crate::vector::ShapePath>,
    /// Text layers only: the editable text the pixels are drawn from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<crate::text::TextLayer>,
    /// Smart objects only: how the original pixels are placed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart: Option<crate::smart::SmartObject>,
    /// Animation: a new frame's cel on this layer links to the frame
    /// before instead of starting empty (or as a copy), so the picture
    /// carries on until it is deliberately unlinked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub continuous: bool,
    /// A label color for the layer row (Aseprite's layer user data).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 4]>,
    /// Free-form notes (Aseprite's layer user data text).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerKind {
    #[default]
    Raster,
    Group,
    /// Owns no pixels: applies `LayerProps::adjustment` to everything below
    /// it (within its group), through its mask and opacity.
    Adjustment,
    /// Pixels made of tiles from a shared tileset (`LayerProps::tilemap`);
    /// painting a cell edits its tile everywhere it is used.
    Tilemap,
    /// Pixels drawn from an editable vector shape (`LayerProps::shape`):
    /// the Shape tool moves its points and the layer is redrawn, crisp.
    Shape,
    /// Pixels drawn from editable text (`LayerProps::text`): the Text tool
    /// reopens it for editing and the layer is redrawn.
    Text,
    /// A smart object: original pixels (`Layer::smart`) drawn through an
    /// editable placement (`LayerProps::smart`), so scaling, rotating and
    /// warping never wear the pixels down.
    Smart,
}

#[derive(Clone)]
pub struct Layer {
    pub props: LayerProps,
    pub raster: Raster,
    /// Photoshop-style layer mask: 8-bit coverage the size of the canvas,
    /// 255 = shown, 0 = hidden. `None` = no mask. Shared like tiles so an
    /// undo snapshot costs a pointer.
    pub mask: Option<Arc<Mask>>,
    /// Animation: one picture per frame (see [`crate::anim`]). `raster` is
    /// the current frame's; this list is brought up to date from it at every
    /// commit. Empty for static layers and for layers of a one-frame
    /// document.
    pub cels: Vec<crate::anim::Cel>,
    /// Smart objects only: the original pixels the layer is drawn from
    /// through `LayerProps::smart` (see [`crate::smart`]). Shared, so copies
    /// of the layer cost a pointer.
    pub smart: Option<Arc<crate::smart::SmartSource>>,
}

impl Layer {
    pub fn new(id: LayerId, name: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            props: LayerProps {
                id,
                name: name.into(),
                visible: true,
                locked: false,
                alpha_locked: false,
                opacity: 1.0,
                blend: BlendMode::Normal,
                clipped: false,
                kind: LayerKind::Raster,
                parent: None,
                expanded: true,
                mask_enabled: true,
                style: Default::default(),
                adjustment: None,
                tilemap: None,
                shape: None,
                text: None,
                smart: None,
                continuous: false,
                color: None,
                notes: String::new(),
            },
            raster: Raster::new(width, height),
            smart: None,
            mask: None,
            cels: Vec::new(),
        }
    }

    /// The mask that is currently in effect, if any.
    pub fn active_mask(&self) -> Option<&Mask> {
        if self.props.mask_enabled {
            self.mask.as_deref()
        } else {
            None
        }
    }

    /// The pixels as they show: the raster with the mask (when in effect)
    /// multiplied into its alpha. Cheap when there is no mask.
    pub fn masked_raster(&self) -> Raster {
        let Some(m) = self.active_mask() else { return self.raster.clone() };
        let mut out = self.raster.clone();
        let (w, h) = (out.width() as i32, out.height() as i32);
        for (tx, ty) in out.tiles_in_rect(out.rect()) {
            if out.tile(tx, ty).is_none() {
                continue;
            }
            let t = out.tile_mut(tx, ty);
            let (x0, y0) = (tx as i32 * crate::raster::TILE as i32, ty as i32 * crate::raster::TILE as i32);
            for ly in 0..crate::raster::TILE {
                for lx in 0..crate::raster::TILE {
                    let (x, y) = (x0 + lx as i32, y0 + ly as i32);
                    if x >= w || y >= h {
                        continue;
                    }
                    let i = (ly * crate::raster::TILE + lx) * 4 + 3;
                    let a = t.px[i];
                    if a != 0 {
                        t.px[i] = ((a as u32 * m.get(x, y) as u32 + 127) / 255) as u8;
                    }
                }
            }
        }
        // Pixels kept off the canvas go through the mask's off-canvas part.
        self.raster.for_each_outside(|x, y, c| {
            out.set_pixel_any(x, y, c.with_alpha(((c.a as u32 * m.get_any(x, y) as u32 + 127) / 255) as u8));
        });
        out.prune_empty_tiles();
        out
    }

    /// Bake the mask into the pixels and drop it.
    pub fn apply_mask(&mut self) {
        if self.mask.is_some() {
            if self.props.mask_enabled {
                self.raster = self.masked_raster();
            }
            self.mask = None;
            self.props.mask_enabled = true;
        }
    }

    pub fn with_raster(mut self, raster: Raster) -> Self {
        self.raster = raster;
        self
    }

    pub fn id(&self) -> LayerId {
        self.props.id
    }
    pub fn name(&self) -> &str {
        &self.props.name
    }
    pub fn visible(&self) -> bool {
        self.props.visible
    }
    pub fn locked(&self) -> bool {
        self.props.locked
    }
    pub fn opacity(&self) -> f32 {
        self.props.opacity
    }
    pub fn blend(&self) -> BlendMode {
        self.props.blend
    }

    /// A group folder rather than a raster layer.
    pub fn is_group(&self) -> bool {
        self.props.kind == LayerKind::Group
    }

    /// An adjustment layer (see [`LayerKind::Adjustment`]).
    pub fn is_adjustment(&self) -> bool {
        self.props.kind == LayerKind::Adjustment
    }

    /// A plain raster layer: the only kind whose pixels filters, transforms
    /// and the like work on.
    pub fn owns_pixels(&self) -> bool {
        matches!(
            self.props.kind,
            LayerKind::Raster | LayerKind::Tilemap | LayerKind::Shape | LayerKind::Text | LayerKind::Smart
        )
    }

    /// A vector shape layer (see [`LayerKind::Shape`]).
    pub fn is_shape(&self) -> bool {
        self.props.kind == LayerKind::Shape && self.props.shape.is_some()
    }

    /// A smart object (see [`LayerKind::Smart`]).
    pub fn is_smart(&self) -> bool {
        self.props.kind == LayerKind::Smart && self.props.smart.is_some() && self.smart.is_some()
    }

    /// An editable text layer (see [`LayerKind::Text`]).
    pub fn is_text(&self) -> bool {
        self.props.kind == LayerKind::Text && self.props.text.is_some()
    }

    /// Apply this adjustment layer to `raster` (straight RGBA) the way the
    /// compositor would draw it over those pixels: through the mask, at the
    /// layer's opacity and blend mode. Alpha is kept. For Merge Down.
    pub fn apply_adjustment_to(&self, raster: &mut Raster) {
        let Some(f) = self.props.adjustment.as_ref().and_then(crate::filter::adjust::pixel_fn) else { return };
        let mask = self.active_mask();
        let (opacity, mode) = (self.props.opacity, self.props.blend);
        let tile = crate::raster::TILE;
        for ty in 0..raster.tiles_y() {
            for tx in 0..raster.tiles_x() {
                if raster.tile(tx, ty).is_none() {
                    continue;
                }
                let t = raster.tile_mut(tx, ty);
                for ly in 0..tile {
                    for lx in 0..tile {
                        let p = t.get(lx, ly);
                        if p.a == 0 {
                            continue;
                        }
                        let (x, y) = ((tx as usize * tile + lx) as i32, (ty as usize * tile + ly) as i32);
                        let k = opacity * mask.map_or(1.0, |m| m.get(x, y) as f32 / 255.0);
                        if k <= 0.0 {
                            continue;
                        }
                        let c = [p.r as f32 / 255.0, p.g as f32 / 255.0, p.b as f32 / 255.0];
                        let adj = f(c);
                        let b = crate::composite::adjusted(mode, c, adj);
                        let q = |i: usize| ((c[i] + (b[i] - c[i]) * k).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                        t.set(lx, ly, crate::color::Rgba8::new(q(0), q(1), q(2), p.a));
                    }
                }
            }
        }
    }

    /// Can the user paint on this layer right now? Groups own no pixels, so
    /// they are never editable; ancestors' visibility is checked by
    /// [`crate::DocState::layer_editable`].
    pub fn editable(&self) -> bool {
        // An adjustment layer is painted through its mask only; a shape
        // layer is drawn from its points (rasterize it to paint on it).
        self.props.visible
            && !self.props.locked
            && !self.is_shape()
            && !self.is_text()
            && !self.is_smart()
            && (self.owns_pixels() || (self.is_adjustment() && self.mask.is_some()))
    }
}
