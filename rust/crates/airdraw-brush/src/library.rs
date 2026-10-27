//! The set of available brushes: built-ins embedded in the binary, plus user brushes loaded
//! from (and saved to) a folder. A user brush with a built-in's id replaces it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::brush::{Brush, BrushDef, BrushError};
use crate::registry::StageRegistry;

pub const BRUSH_EXTENSION: &str = "brush.toml";

/// Embedded so they work on every platform without file access.
const BUILTIN: &[(&str, &str)] = &[
    ("ink", include_str!("../brushes/ink.brush.toml")),
    ("marker", include_str!("../brushes/marker.brush.toml")),
    ("ribbon", include_str!("../brushes/ribbon.brush.toml")),
    ("dots", include_str!("../brushes/dots.brush.toml")),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrushSource {
    Builtin,
    User(PathBuf),
}

pub struct BrushLibrary {
    registry: StageRegistry,
    brushes: Vec<(Arc<Brush>, BrushSource)>,
}

impl BrushLibrary {
    /// Library holding the built-in brushes. Panics if a built-in is invalid (caught by tests).
    pub fn new(registry: StageRegistry) -> Self {
        let mut lib = Self { registry, brushes: Vec::new() };
        for (name, text) in BUILTIN {
            let brush = BrushDef::from_toml(text)
                .and_then(|d| Brush::compile(d, &lib.registry))
                .unwrap_or_else(|e| panic!("built-in brush '{name}' is invalid: {e}"));
            lib.insert(brush, BrushSource::Builtin);
        }
        lib
    }

    pub fn is_builtin_id(id: &str) -> bool {
        BUILTIN.iter().any(|(_, text)| BrushDef::from_toml(text).is_ok_and(|d| d.id == id))
    }

    pub fn registry(&self) -> &StageRegistry {
        &self.registry
    }

    pub fn get(&self, id: &str) -> Option<Arc<Brush>> {
        self.brushes.iter().find(|(b, _)| b.id() == id).map(|(b, _)| b.clone())
    }

    pub fn source(&self, id: &str) -> Option<&BrushSource> {
        self.brushes.iter().find(|(b, _)| b.id() == id).map(|(_, s)| s)
    }

    /// Brushes in display order (built-ins first, then user brushes as loaded).
    pub fn iter(&self) -> impl Iterator<Item = &Arc<Brush>> {
        self.brushes.iter().map(|(b, _)| b)
    }

    /// Loads every `*.brush.toml` in `dir`. Invalid files are skipped and reported, so one
    /// broken brush never hides the others. A missing folder is not an error.
    pub fn load_dir(&mut self, dir: &Path) -> Vec<(PathBuf, BrushError)> {
        let mut errors = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else { return errors };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.to_string_lossy().ends_with(&format!(".{BRUSH_EXTENSION}")))
            .collect();
        paths.sort();
        for path in paths {
            match self.load_file(&path) {
                Ok(brush) => self.insert(brush, BrushSource::User(path)),
                Err(e) => errors.push((path, e)),
            }
        }
        errors
    }

    fn load_file(&self, path: &Path) -> Result<Brush, BrushError> {
        let text = std::fs::read_to_string(path).map_err(|e| BrushError::Io(e.to_string()))?;
        Brush::compile(BrushDef::from_toml(&text)?, &self.registry)
    }

    /// Validates `def`, writes it to `dir` and makes it available. This is the path the brush
    /// editor uses to save custom brushes.
    pub fn save(&mut self, def: BrushDef, dir: &Path) -> Result<PathBuf, BrushError> {
        let brush = Brush::compile(def, &self.registry)?;
        let text = brush.def.to_toml()?;
        std::fs::create_dir_all(dir).map_err(|e| BrushError::Io(e.to_string()))?;
        let path = dir.join(format!("{}.{BRUSH_EXTENSION}", file_stem(brush.id())));
        std::fs::write(&path, text).map_err(|e| BrushError::Io(e.to_string()))?;
        self.insert(brush, BrushSource::User(path.clone()));
        Ok(path)
    }

    /// Deletes a user brush and its file. If it replaced a built-in, the built-in returns.
    pub fn delete(&mut self, id: &str) -> Result<(), BrushError> {
        let idx = self.brushes.iter().position(|(b, _)| b.id() == id).ok_or_else(|| BrushError::Io(format!("no brush '{id}'")))?;
        let BrushSource::User(path) = self.brushes[idx].1.clone() else {
            return Err(BrushError::NotAllowed("built-in brushes can't be deleted".into()));
        };
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(BrushError::Io(format!("{}: {e}", path.display()))),
        }
        match builtin(id, &self.registry) {
            Some(b) => self.brushes[idx] = (Arc::new(b), BrushSource::Builtin),
            None => {
                self.brushes.remove(idx);
            }
        }
        Ok(())
    }

    /// Validates a brush file and installs a copy in `dir`. Returns the brush id.
    pub fn import_file(&mut self, path: &Path, dir: &Path) -> Result<String, BrushError> {
        let text = std::fs::read_to_string(path).map_err(|e| BrushError::Io(format!("{}: {e}", path.display())))?;
        let def = BrushDef::from_toml(&text)?;
        let id = def.id.clone();
        self.save(def, dir)?;
        Ok(id)
    }

    /// Writes a brush as a shareable `.brush.toml` file.
    pub fn export_file(&self, id: &str, path: &Path) -> Result<(), BrushError> {
        let brush = self.get(id).ok_or_else(|| BrushError::Io(format!("no brush '{id}'")))?;
        std::fs::write(path, brush.def.to_toml()?).map_err(|e| BrushError::Io(format!("{}: {e}", path.display())))
    }

    fn insert(&mut self, brush: Brush, source: BrushSource) {
        let brush = Arc::new(brush);
        match self.brushes.iter_mut().find(|(b, _)| b.id() == brush.id()) {
            Some(slot) => *slot = (brush, source),
            None => self.brushes.push((brush, source)),
        }
    }
}

fn builtin(id: &str, registry: &StageRegistry) -> Option<Brush> {
    BUILTIN
        .iter()
        .filter_map(|(_, text)| BrushDef::from_toml(text).ok())
        .find(|d| d.id == id)
        .and_then(|d| Brush::compile(d, registry).ok())
}

/// Brush ids become file names; keep them portable across file systems.
fn file_stem(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::StageRef;
    use crate::params::ParamValue;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("airdraw-brush-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn builtins_load() {
        let lib = BrushLibrary::new(StageRegistry::with_builtins());
        let ids: Vec<_> = lib.iter().map(|b| b.id().to_string()).collect();
        assert_eq!(ids, ["com.airdraw.ink", "com.airdraw.marker", "com.airdraw.ribbon", "com.airdraw.dots"]);
    }

    #[test]
    fn save_then_reload_custom_brush() {
        let dir = temp_dir("save");
        let mut lib = BrushLibrary::new(StageRegistry::with_builtins());
        let mut def = lib.get("com.airdraw.ink").unwrap().def.clone();
        def.id = "user.my ink/2".into();
        def.name = "My Ink".into();
        def.material = StageRef { kind: "solid".into(), params: [("color".into(), ParamValue::Str("#3366FF".into()))].into() };
        let path = lib.save(def, &dir).unwrap();
        assert!(path.ends_with("user.my_ink_2.brush.toml"));

        let mut fresh = BrushLibrary::new(StageRegistry::with_builtins());
        assert!(fresh.load_dir(&dir).is_empty());
        let loaded = fresh.get("user.my ink/2").unwrap();
        assert_eq!(loaded.def.name, "My Ink");
        assert_eq!(fresh.source("user.my ink/2"), Some(&BrushSource::User(path)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_save_is_rejected_and_bad_files_are_reported() {
        let dir = temp_dir("bad");
        let mut lib = BrushLibrary::new(StageRegistry::with_builtins());
        let mut def = lib.get("com.airdraw.ink").unwrap().def.clone();
        def.geometry = StageRef::new("nope");
        assert!(lib.save(def, &dir).is_err());

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.brush.toml"), "schema = 1\nid = 3").unwrap();
        let errors = lib.load_dir(&dir);
        assert_eq!(errors.len(), 1);
        assert_eq!(lib.iter().count(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_user_brush_and_restore_builtin() {
        let dir = temp_dir("delete");
        let mut lib = BrushLibrary::new(StageRegistry::with_builtins());
        let mut custom = lib.get("com.airdraw.ink").unwrap().def.clone();
        custom.id = "user.thin".into();
        let custom_path = lib.save(custom, &dir).unwrap();
        let mut over = lib.get("com.airdraw.marker").unwrap().def.clone();
        over.name = "Fat Marker".into();
        lib.save(over, &dir).unwrap();

        lib.delete("user.thin").unwrap();
        assert!(lib.get("user.thin").is_none() && !custom_path.exists());
        lib.delete("com.airdraw.marker").unwrap();
        assert_eq!(lib.get("com.airdraw.marker").unwrap().def.name, "Marker");
        assert_eq!(lib.source("com.airdraw.marker"), Some(&BrushSource::Builtin));
        assert!(matches!(lib.delete("com.airdraw.ink"), Err(BrushError::NotAllowed(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_then_import_roundtrip() {
        let dir = temp_dir("share");
        std::fs::create_dir_all(&dir).unwrap();
        let mut lib = BrushLibrary::new(StageRegistry::with_builtins());
        let mut def = lib.get("com.airdraw.dots").unwrap().def.clone();
        def.id = "friend.beads".into();
        lib.save(def, &dir.join("mine")).unwrap();
        let shared = dir.join("beads.brush.toml");
        lib.export_file("friend.beads", &shared).unwrap();

        let mut other = BrushLibrary::new(StageRegistry::with_builtins());
        assert_eq!(other.import_file(&shared, &dir.join("theirs")).unwrap(), "friend.beads");
        assert_eq!(other.get("friend.beads").unwrap().def, lib.get("friend.beads").unwrap().def);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_brush_overrides_builtin() {
        let dir = temp_dir("override");
        let mut lib = BrushLibrary::new(StageRegistry::with_builtins());
        let mut def = lib.get("com.airdraw.marker").unwrap().def.clone();
        def.name = "Fat Marker".into();
        lib.save(def, &dir).unwrap();
        assert_eq!(lib.get("com.airdraw.marker").unwrap().def.name, "Fat Marker");
        assert_eq!(lib.iter().count(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod perf {
    use super::*;
    use crate::InputPoint;
    use glam::Vec3;

    /// Not an assertion test: prints the full-rebuild cost per stroke length (run in release
    /// with --ignored --nocapture). Pencil reports 240 points/s: 2400 points ≈ 10 s of drawing.
    #[test]
    #[ignore]
    fn live_rebuild_cost() {
        let lib = BrushLibrary::new(StageRegistry::with_builtins());
        for id in ["com.airdraw.ink", "com.airdraw.ribbon"] {
            let brush = lib.get(id).unwrap();
            for n in [240, 1200, 2400, 4800] {
                let pts: Vec<InputPoint> = (0..n)
                    .map(|i| {
                        let t = i as f32 / 240.0;
                        let pos = Vec3::new(t.cos() * t * 0.1, t.sin() * t * 0.1, 0.0);
                        InputPoint { pos, normal: Vec3::Z, pressure: 0.7, tilt: None, t: t as f64 }
                    })
                    .collect();
                let runs = 20;
                let start = std::time::Instant::now();
                let mut verts = 0;
                for _ in 0..runs {
                    verts = brush.build(&pts).positions.len();
                }
                let ms = start.elapsed().as_secs_f64() * 1000.0 / runs as f64;
                println!("PERF {id} points={n} vertices={verts} build_ms={ms:.2}");
            }
        }
    }
}
