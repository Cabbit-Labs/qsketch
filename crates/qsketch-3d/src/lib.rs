//! Headless 3D support for qsketch's Model panel. No GUI or GPU code lives here.
//! The design is in `docs/Q3D.md`.

pub mod camera;
pub mod mesh;
pub mod sections;
pub mod unwrap;

pub use camera::{Mat4, Orbit};
pub use mesh::{Mesh, Vertex};
pub use sections::{find_sections, Section, Sections};
