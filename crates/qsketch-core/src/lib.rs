//! qsketch-core: the headless heart of qsketch.
//!
//! Everything here is GUI-agnostic and fully unit-testable: the tiled raster
//! store, blend modes and compositor, selection masks, the brush/stroke engine,
//! the persistent-snapshot history, document operations, clipboard data and the
//! `.qsk` / image file formats.
//!
//! Design notes
//! ------------
//! * Layers are stored as 64×64 RGBA8 **straight-alpha** tiles behind `Arc`s.
//!   Mutating a tile clones it on write, so a full document snapshot is a cheap
//!   shallow clone. Undo history is simply a list of snapshots; only the tiles a
//!   stroke actually touched are duplicated in memory.
//! * The compositor produces **premultiplied** RGBA8 tiles, which the GPU draws
//!   over a checkerboard with a single blend.
//! * Pixel math inside the brush engine and compositor is done in `f32` on the
//!   affected pixels only.

pub mod blend;
pub mod brush;
pub mod clipboard;
pub mod color;
pub mod composite;
pub mod document;
pub mod filter;
pub mod geom;
pub mod group;
pub mod history;
pub mod io;
pub mod layer;
pub mod mask;
pub mod ops;
pub mod raster;
pub mod text;
pub mod tip;
pub mod warp;

pub use blend::BlendMode;
pub use brush::{AngleControl, BrushSettings, PaintMode, StabilizerMode, StrokeEngine, StrokeSample, TextureMode};
pub use clipboard::ClipImage;
pub use color::{Hsv, Rgba8};
pub use composite::{Composite, TileSet};
pub use document::{DocState, Document};
pub use filter::Filter;
pub use geom::{IRect, Pt};
pub use history::{History, HistoryEntry};
pub use layer::{Layer, LayerId, LayerKind};
pub use mask::Mask;
pub use raster::{Raster, Tile, TILE, TILE_BYTES, TILE_PX};
pub use text::{TextAlign, TextImage, TextStyle};
pub use tip::TipImage;

/// Crate version, surfaced in the About dialog and file manifests.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
