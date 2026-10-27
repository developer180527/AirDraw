//! iOS / iPadOS input backend. Observes UIKit touches with a passive gesture recognizer and
//! converts them to the unified stylus model: the real tool type (Pencil vs finger), every
//! coalesced sample between frames, the OS's predicted samples, pressure, altitude, azimuth
//! and roll. Pencil hover, squeeze and double-tap come from UIKit's hover recognizer and
//! `UIPencilInteraction`.
//!
//! Godot keeps receiving the same touches (the recognizer never claims them), so Godot's GUI
//! still works; the app must stop converting Godot touch events itself while this is active.

use airdraw_platform::QueueInput;

#[cfg(target_os = "ios")]
mod files;
#[cfg(target_os = "ios")]
mod uikit;

/// Result of the system document picker.
#[derive(Clone, Debug, PartialEq)]
pub enum FileEvent {
    /// `bookmark` (hex) restores access later; present when the OS provides one.
    Picked { path: String, bookmark: Option<String> },
    Cancelled,
    /// A file handed to [`export_file`] was saved; `name` is the name it got there.
    Exported { name: String },
    ExportCancelled,
}

/// Shows the system "Open" picker for files of `type_identifier`. Returns false where there
/// is no native picker (the app then uses Godot's dialog) or it couldn't be shown.
pub fn pick_file(type_identifier: &str) -> bool {
    #[cfg(target_os = "ios")]
    {
        files::pick(type_identifier)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = type_identifier;
        false
    }
}

/// Shows the system "Save to Files" picker for a copy of the file at `path`. Returns false
/// where there is none (desktop: the app uses a native save dialog) or it couldn't be shown.
/// The result arrives as `FileEvent::Exported` or `ExportCancelled`; then `path` can go.
pub fn export_file(path: &str) -> bool {
    #[cfg(target_os = "ios")]
    {
        files::export(path)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = path;
        false
    }
}

/// Picker results since the last call.
pub fn poll_file_events(out: &mut Vec<FileEvent>) {
    #[cfg(target_os = "ios")]
    files::drain(out);
    #[cfg(not(target_os = "ios"))]
    let _ = out;
}

/// Restores access to a previously picked file. Returns its current path.
pub fn open_bookmark(hex: &str) -> Option<String> {
    #[cfg(target_os = "ios")]
    {
        files::open_bookmark(hex)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = hex;
        None
    }
}

/// Starts receiving documents opened from other apps (and the one the app was launched
/// with); they arrive as `FileEvent::Picked` through `poll_file_events`. iOS only.
pub fn listen_for_opened_files() {
    #[cfg(target_os = "ios")]
    files::listen_for_opened_files();
}

/// The folder users see as the app's storage in the Files app (iOS Documents). `None`
/// elsewhere: desktop apps choose a folder in the user's Documents.
pub fn documents_dir() -> Option<String> {
    #[cfg(target_os = "ios")]
    {
        files::documents_dir()
    }
    #[cfg(not(target_os = "ios"))]
    {
        None
    }
}

/// Ends access to a file opened through the picker or a bookmark.
pub fn release_file(path: &str) {
    #[cfg(target_os = "ios")]
    files::release(path);
    #[cfg(not(target_os = "ios"))]
    let _ = path;
}

/// What an iPad with Apple Pencil reports.
pub const IPAD_PENCIL_CAPS: airdraw_input::DeviceCaps = airdraw_input::DeviceCaps {
    pressure: true,
    tilt: true,
    azimuth: true,
    roll: true,
    hover: true,
    predicted: true,
    coalesced: true,
    report_rate_hz: 240,
};

/// Tells the backend Godot's viewport size in pixels. Touch positions are mapped from UIKit
/// points by viewport ÷ view bounds, so they match Godot's coordinates whatever its render
/// scale is. Call whenever the viewport size may have changed (cheap).
pub fn set_viewport_size(width: f32, height: f32) {
    #[cfg(target_os = "ios")]
    uikit::set_viewport_size(width, height);
    #[cfg(not(target_os = "ios"))]
    let _ = (width, height);
}

/// Installs the UIKit observers on the app's root view. Returns the input source they feed,
/// or `None` when not on iOS, not on the main thread, or no window exists yet.
pub fn attach() -> Option<QueueInput> {
    #[cfg(target_os = "ios")]
    {
        uikit::attach()
    }
    #[cfg(not(target_os = "ios"))]
    {
        None
    }
}
