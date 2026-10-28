//! Document model: layers, strokes (raw input + brush snapshot), undo/redo and `.airdraw`
//! files. See docs/ARCHITECTURE.md §3c.

#[doc(hidden)]
pub mod bench;
mod document;
mod file;
mod history;
pub mod recents;
mod strokes;

pub use document::{BrushRef, Change, Document, Layer, LayerId, Stroke, StrokeId};
pub use file::{DocError, EXTENSION, FORMAT_VERSION, Level, MAX_THUMBNAIL_BYTES, SaveOptions, read_thumbnail};
pub use recents::{RecentEntry, Recents};
