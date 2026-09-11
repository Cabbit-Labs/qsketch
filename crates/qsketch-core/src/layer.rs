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
