use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use airdraw_brush::{ParamSpec, Params};

use crate::scene::ExportScene;

/// What a format can represent; the host warns when requested data would be dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExporterCaps {
    pub meshes: bool,
    pub curves: bool,
    pub materials: bool,
    pub hierarchy: bool,
    /// Writes companion files next to the main one (e.g. OBJ's .mtl).
    pub multi_file: bool,
    /// Works from stroke centerlines (e.g. SVG): the host includes curves in the scene.
    pub requires_curves: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExporterInfo {
    /// Stable id, e.g. `com.airdraw.glb`. Plugins use their own reverse-DNS ids.
    pub id: String,
    pub name: String,
    /// Main file extension without the dot.
    pub extension: String,
    pub caps: ExporterCaps,
}

/// Where exporters write. They never touch paths or platform file APIs directly, so the same
/// exporter can write to a folder, memory (share sheet), or a plugin-provided target.
pub trait ExportSink {
    /// Base name for the export without extension, e.g. `sketch`.
    fn stem(&self) -> &str;
    /// Writes one file; `name` is a bare file name like `sketch.mtl`.
    fn write(&mut self, name: &str, data: &[u8]) -> Result<(), ExportError>;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExportReport {
    pub files: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExportError {
    UnknownExporter(String),
    BadOption { name: String, reason: String },
    Io(String),
    Failed(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportError::UnknownExporter(id) => write!(f, "no exporter '{id}'"),
            ExportError::BadOption { name, reason } => write!(f, "option '{name}': {reason}"),
            ExportError::Io(e) => write!(f, "{e}"),
            ExportError::Failed(e) => write!(f, "export failed: {e}"),
        }
    }
}

impl std::error::Error for ExportError {}

pub trait Exporter: Send + Sync {
    fn info(&self) -> ExporterInfo;
    /// Format-specific options; drives the export dialog. Shares the brush parameter schema.
    fn options(&self) -> Vec<ParamSpec> {
        Vec::new()
    }
    /// `opts` is complete and validated against [`Exporter::options`].
    fn export(&self, scene: &ExportScene, opts: &Params, out: &mut dyn ExportSink) -> Result<ExportReport, ExportError>;
}

#[derive(Default, Clone)]
pub struct ExporterRegistry {
    exporters: BTreeMap<String, Arc<dyn Exporter>>,
    order: Vec<String>,
}

impl ExporterRegistry {
    pub fn with_builtins() -> Self {
        let mut r = Self::default();
        r.register(crate::formats::gltf::Glb);
        r.register(crate::formats::gltf::Gltf);
        r.register(crate::formats::fbx::Fbx);
        r.register(crate::formats::obj::Obj);
        r.register(crate::formats::usd::Usdz);
        r.register(crate::formats::usd::Usda);
        r.register(crate::formats::stl::Stl);
        r.register(crate::formats::ply::Ply);
        r.register(crate::formats::svg::Svg);
        r
    }

    /// Adds or replaces (same id) an exporter.
    pub fn register(&mut self, e: impl Exporter + 'static) {
        let id = e.info().id;
        if !self.exporters.contains_key(&id) {
            self.order.push(id.clone());
        }
        self.exporters.insert(id, Arc::new(e));
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn Exporter>> {
        self.exporters.get(id)
    }

    /// In registration order, for the format picker.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Exporter>> {
        self.order.iter().filter_map(|id| self.exporters.get(id))
    }

    /// Validates `opts` against the exporter's schema (filling defaults) and runs it.
    /// Adds a warning when the scene holds data the format can't store.
    pub fn export(&self, id: &str, scene: &ExportScene, opts: &Params, out: &mut dyn ExportSink) -> Result<ExportReport, ExportError> {
        let e = self.get(id).ok_or_else(|| ExportError::UnknownExporter(id.into()))?;
        let schema = e.options();
        let mut resolved: Params = schema.iter().map(|s| (s.name.to_string(), s.default.clone())).collect();
        for spec in &schema {
            if let Some(v) = opts.get(spec.name) {
                let v = spec.validate(v).map_err(|reason| ExportError::BadOption { name: spec.name.into(), reason })?;
                resolved.insert(spec.name.into(), v);
            }
        }
        let mut report = e.export(scene, &resolved, out)?;
        if !scene.curves.is_empty() && !e.info().caps.curves {
            report.warnings.push(format!("{} can't store curves; they were skipped", e.info().name));
        }
        Ok(report)
    }
}

/// Writes into a folder.
pub struct FolderSink {
    dir: PathBuf,
    stem: String,
}

impl FolderSink {
    /// For a main file path like `/x/sketch.glb`: folder `/x`, stem `sketch`.
    pub fn for_path(path: &Path) -> Self {
        Self {
            dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
            stem: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "export".into()),
        }
    }
}

impl ExportSink for FolderSink {
    fn stem(&self) -> &str {
        &self.stem
    }
    fn write(&mut self, name: &str, data: &[u8]) -> Result<(), ExportError> {
        std::fs::create_dir_all(&self.dir).map_err(|e| ExportError::Io(format!("{}: {e}", self.dir.display())))?;
        let path = self.dir.join(name);
        std::fs::write(&path, data).map_err(|e| ExportError::Io(format!("{}: {e}", path.display())))
    }
}

/// Collects files in memory (share sheets, tests).
#[derive(Default)]
pub struct MemorySink {
    pub stem: String,
    pub files: BTreeMap<String, Vec<u8>>,
}

impl MemorySink {
    pub fn new(stem: &str) -> Self {
        Self { stem: stem.into(), files: BTreeMap::new() }
    }
}

impl ExportSink for MemorySink {
    fn stem(&self) -> &str {
        &self.stem
    }
    fn write(&mut self, name: &str, data: &[u8]) -> Result<(), ExportError> {
        self.files.insert(name.into(), data.to_vec());
        Ok(())
    }
}
