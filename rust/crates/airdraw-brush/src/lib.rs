//! Brush engine. A brush is data: a `.brush.toml` file naming registered stages and their
//! parameters. Strokes run input points → filters → dynamics → geometry, plus a material.
//! See docs/ARCHITECTURE.md §3.

pub mod brush;
pub mod library;
pub mod params;
pub mod registry;
pub mod stage;
pub mod stages;

pub use brush::{Brush, BrushDef, BrushError, StageRef};
pub use library::{BrushLibrary, BrushSource};
pub use params::{ParamSpec, ParamValue, Params};
pub use registry::{Slot, StageRegistry};
pub use stage::{InputPoint, MaterialDesc, MeshBuffers, Shading, StrokePoint};
