//! Recently opened sketches: a history list for the home screen, not a view of any folder.
//! Stored as TOML in the app's data folder. Files live wherever the user keeps them; on iOS
//! an entry carries a security-scoped bookmark so files outside the app stay reachable.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentEntry {
    pub path: String,
    /// Display name (file name without extension at the time it was opened).
    pub name: String,
    /// Unix seconds.
    pub opened_at: u64,
    /// Platform bookmark (hex) that restores access to the file, where the OS needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookmark: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct RecentsFile {
    #[serde(default)]
    recent: Vec<RecentEntry>,
}

pub struct Recents {
    file: PathBuf,
    entries: Vec<RecentEntry>,
}

impl Recents {
    pub const MAX: usize = 50;

    /// Loads the list. A missing or unreadable file gives an empty list: losing history must
    /// never stop the app from starting.
    pub fn load(file: &Path) -> Self {
        let entries = std::fs::read_to_string(file)
            .ok()
            .and_then(|t| toml::from_str::<RecentsFile>(&t).ok())
            .map(|f| f.recent)
            .unwrap_or_default();
        Self { file: file.to_path_buf(), entries }
    }

    /// Writes atomically (temp file + rename).
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string(&RecentsFile { recent: self.entries.clone() }).map_err(std::io::Error::other)?;
        let tmp = self.file.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.file)
    }

    /// Most recent first.
    pub fn entries(&self) -> &[RecentEntry] {
        &self.entries
    }

    /// Records that `path` was opened now, moving it to the front. Keeps an existing
    /// bookmark when none is given.
    pub fn touch(&mut self, path: &str, bookmark: Option<String>, now: u64) {
        let old = self.entries.iter().position(|e| e.path == path).map(|i| self.entries.remove(i));
        let name = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.into());
        let bookmark = bookmark.or_else(|| old.and_then(|o| o.bookmark));
        self.entries.insert(0, RecentEntry { path: path.into(), name, opened_at: now, bookmark });
        self.entries.truncate(Self::MAX);
    }

    pub fn remove(&mut self, path: &str) {
        self.entries.retain(|e| e.path != path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("airdraw-recents-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("recents.toml")
    }

    #[test]
    fn touch_orders_dedupes_and_caps() {
        let mut r = Recents::load(&temp("order"));
        r.touch("/a/One.airdraw", None, 1);
        r.touch("/a/Two.airdraw", Some("beef".into()), 2);
        r.touch("/a/One.airdraw", None, 3);
        assert_eq!(r.entries().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["One", "Two"]);
        assert_eq!(r.entries()[0].opened_at, 3);
        r.touch("/a/Two.airdraw", None, 4);
        assert_eq!(r.entries()[0].bookmark.as_deref(), Some("beef"), "bookmark kept when re-opened without one");
        for i in 0..60 {
            r.touch(&format!("/b/{i}.airdraw"), None, 10 + i);
        }
        assert_eq!(r.entries().len(), Recents::MAX);
        assert_eq!(r.entries()[0].name, "59");
    }

    #[test]
    fn persists_and_survives_corruption() {
        let file = temp("persist");
        let mut r = Recents::load(&file);
        r.touch("/x/Sketch.airdraw", Some("00ff".into()), 7);
        r.touch("/x/Other.airdraw", None, 8);
        r.remove("/x/Other.airdraw");
        r.save().unwrap();
        assert_eq!(Recents::load(&file).entries(), r.entries());
        std::fs::write(&file, "not [valid toml").unwrap();
        assert!(Recents::load(&file).entries().is_empty());
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }
}
