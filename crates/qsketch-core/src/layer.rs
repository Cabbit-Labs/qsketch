//! A raster layer with its Photoshop-style properties.

use serde::{Deserialize, Serialize};

use crate::blend::BlendMode;
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
}

#[derive(Clone)]
pub struct Layer {
    pub props: LayerProps,
    pub raster: Raster,
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
            },
            raster: Raster::new(width, height),
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

    /// Can the user paint on this layer right now?
    pub fn editable(&self) -> bool {
        self.props.visible && !self.props.locked
    }
}
