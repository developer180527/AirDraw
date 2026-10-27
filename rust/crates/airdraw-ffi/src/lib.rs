//! Exporter plugin ABI (see `include/airdraw_export.h`) and the host-side adapter that turns
//! a C/C++ plugin into a regular [`airdraw_export::Exporter`]. See docs/ARCHITECTURE.md §4e.

pub mod abi;
mod plugin;

pub use plugin::PluginExporter;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub use plugin::load_dir;

/// Path of the C header plugins compile against.
pub const HEADER_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/include");
