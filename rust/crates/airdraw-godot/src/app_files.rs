//! App-level file services for the home screen: the system "Open" picker (native on iOS;
//! the app falls back to Godot's dialog elsewhere), security-scoped bookmarks, and Recents.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use airdraw_doc::Recents;
use airdraw_ios::FileEvent;
use godot::classes::{INode, Node};
use godot::prelude::*;

/// Uniform type identifier of .airdraw files (declared in the app's Info.plist).
pub const SKETCH_TYPE: &str = "com.venugopal.airdraw.sketch";

#[derive(GodotClass)]
#[class(base = Node)]
pub struct AppFiles {
    base: Base<Node>,
    recents: Option<Recents>,
}

#[godot_api]
impl INode for AppFiles {
    fn init(base: Base<Node>) -> Self {
        Self { base, recents: None }
    }

    fn ready(&mut self) {
        airdraw_ios::listen_for_opened_files();
    }

    fn process(&mut self, _delta: f64) {
        let mut events = Vec::new();
        airdraw_ios::poll_file_events(&mut events);
        for e in events {
            match e {
                FileEvent::Picked { path, bookmark } => {
                    let (path, bookmark) = (GString::from(path.as_str()), GString::from(bookmark.unwrap_or_default().as_str()));
                    self.signals().file_picked().emit(&path, &bookmark);
                }
                FileEvent::Cancelled => self.signals().pick_cancelled().emit(),
                FileEvent::Exported { name } => self.signals().file_exported().emit(&GString::from(name.as_str())),
                FileEvent::ExportCancelled => self.signals().export_cancelled().emit(),
            }
        }
    }
}

#[godot_api]
impl AppFiles {
    /// A file was chosen in the native picker, or opened from another app (iOS). `bookmark` (hex, may be empty) restores
    /// access later via `open_bookmark`.
    #[signal]
    fn file_picked(path: GString, bookmark: GString);
    #[signal]
    fn pick_cancelled();
    /// The file given to `export_file` was saved where the user chose, as `name`.
    #[signal]
    fn file_exported(name: GString);
    #[signal]
    fn export_cancelled();

    /// Shows the native "Save to Files" picker for a copy of the file at `path` (iOS). False
    /// where there is none (desktop: use Godot's native save dialog). The result arrives
    /// through `file_exported` or `export_cancelled`; `path` can be deleted then.
    #[func]
    fn export_file(&self, path: GString) -> bool {
        airdraw_ios::export_file(&path.to_string())
    }

    /// Shows the native "Open" picker for sketches. False where there is none (desktop:
    /// use Godot's native file dialog instead).
    #[func]
    fn pick_sketch(&self) -> bool {
        airdraw_ios::pick_file(SKETCH_TYPE)
    }

    /// Folder for user-facing files (Sketches, Exports, Import): iOS Documents (shown in
    /// Files), or "" on desktop, where the app uses ~/Documents/AirDraw.
    #[func]
    fn documents_dir(&self) -> GString {
        airdraw_ios::documents_dir().map(|p| GString::from(p.as_str())).unwrap_or_default()
    }

    /// Restores access to a picked file; returns its path, or "" if it can't be found.
    #[func]
    fn open_bookmark(&self, bookmark: GString) -> GString {
        airdraw_ios::open_bookmark(&bookmark.to_string()).map(|p| GString::from(p.as_str())).unwrap_or_default()
    }

    /// Ends access to a file opened through the picker or a bookmark.
    #[func]
    fn release_file(&self, path: GString) {
        airdraw_ios::release_file(&path.to_string());
    }

    /// Loads the Recents list from `file` (absolute path).
    #[func]
    fn load_recents(&mut self, file: GString) {
        self.recents = Some(Recents::load(&PathBuf::from(file.to_string())));
    }

    /// `[{path, name, opened_at, bookmark}]`, most recent first. Paths are as stored
    /// (`user://` for files inside the app's own folder).
    #[func]
    fn recents(&self) -> Array<VarDictionary> {
        let Some(r) = &self.recents else { return Array::new() };
        r.entries()
            .iter()
            .map(|e| {
                let mut d = VarDictionary::new();
                d.set("path", e.path.as_str());
                d.set("name", e.name.as_str());
                d.set("opened_at", e.opened_at as i64);
                d.set("bookmark", e.bookmark.as_deref().unwrap_or(""));
                d
            })
            .collect()
    }

    /// Moves `path` to the front of Recents (adding it) and saves the list.
    #[func]
    fn touch_recent(&mut self, path: GString, bookmark: GString) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let bookmark = (!bookmark.is_empty()).then(|| bookmark.to_string());
        self.with_recents(|r| r.touch(&path.to_string(), bookmark, now));
    }

    #[func]
    fn remove_recent(&mut self, path: GString) {
        self.with_recents(|r| r.remove(&path.to_string()));
    }

    /// Renames a Recents entry to another form of the same file's path (see
    /// `Recents::retarget`) and saves the list.
    #[func]
    fn retarget_recent(&mut self, from: GString, to: GString) {
        self.with_recents(|r| r.retarget(&from.to_string(), &to.to_string()));
    }

    fn with_recents(&mut self, f: impl FnOnce(&mut Recents)) {
        if let Some(r) = &mut self.recents {
            f(r);
            if let Err(e) = r.save() {
                godot_warn!("Couldn't save recents: {e}");
            }
        }
    }
}
