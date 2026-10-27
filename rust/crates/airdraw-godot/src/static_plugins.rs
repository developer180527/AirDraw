//! Exporter plugins compiled into the app. Platforms that can't load libraries at runtime
//! (iOS) get custom exporters this way; desktop builds can use them too.
//!
//! To add one: compile the plugin's C/C++ source into this library (e.g. a `build.rs` using
//! the `cc` crate, with `-DAIRDRAW_STATIC_PLUGIN` renaming its entry point to something
//! unique), declare that entry point below, and list it in `STATIC_PLUGINS`:
//!
//! ```ignore
//! unsafe extern "C" {
//!     fn my_engine_exporter_entry(host_abi_version: u32) -> *const AdExporterPlugin;
//! }
//! pub static STATIC_PLUGINS: &[AdExporterEntryFn] = &[my_engine_exporter_entry];
//! ```

use airdraw_ffi::abi::AdExporterEntryFn;

pub static STATIC_PLUGINS: &[AdExporterEntryFn] = &[];
