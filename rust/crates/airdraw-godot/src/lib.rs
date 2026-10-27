use godot::prelude::*;

mod app_files;
mod canvas;
mod export_bridge;
mod godot_input;
mod live;
mod session;
mod static_plugins;

struct AirDrawCore;

#[gdextension]
unsafe impl ExtensionLibrary for AirDrawCore {}
