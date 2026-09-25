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
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerKind {
    #[default]
    Raster,
    Group,
}

#[derive(Clone)]
pub struct Layer {
    pub props: LayerProps,
    pub raster: Raster,
    /// Photoshop-style layer mask: 8-bit coverage the size of the canvas,
    /// 255 = shown, 0 = hidden. `None` = no mask. Shared like tiles so an
    /// undo snapshot costs a pointer.
    pub mask: Option<Arc<Mask>>,
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
            },
            raster: Raster::new(width, height),
            mask: None,
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

    /// Can the user paint on this layer right now? Groups own no pixels, so
    /// they are never editable; ancestors' visibility is checked by
    /// [`crate::DocState::layer_editable`].
    pub fn editable(&self) -> bool {
        self.props.visible && !self.props.locked && !self.is_group()
    }
}
