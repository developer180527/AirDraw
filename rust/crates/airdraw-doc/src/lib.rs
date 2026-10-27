//! Document model: layers, strokes (raw input + brush snapshot), undo/redo and `.airdraw`
//! files. See docs/ARCHITECTURE.md §3c.

mod document;
mod file;
pub mod recents;

pub use document::{BrushRef, Change, DEFAULT_GUIDE_OPACITY, Document, Layer, LayerId, MAX_GUIDE_OPACITY, Stroke, StrokeId};
pub use recents::{RecentEntry, Recents};
pub use file::{DocError, EXTENSION, FORMAT_VERSION, MAX_THUMBNAIL_BYTES, read_thumbnail};
