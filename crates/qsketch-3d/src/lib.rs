//! Headless 3D support for qsketch's Model panel. No GUI or GPU code lives here.
//! The design is in `docs/Q3D.md`.

pub mod anim;
pub mod camera;
pub mod hidden;
pub mod mesh;
pub mod sections;
pub mod unwrap;

pub use camera::{Mat4, Orbit};
pub use mesh::{shown_by_name, Mesh, Part, Piece, Vertex};
pub use sections::{find_sections, Section, Sections};
