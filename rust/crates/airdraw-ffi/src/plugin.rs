//! Wraps a C ABI exporter plugin as a regular [`Exporter`].

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use std::path::{Path, PathBuf};
use std::sync::Arc;

use airdraw_brush::params::Hint;
use airdraw_brush::{ParamSpec, ParamValue, Params};
use airdraw_export::{
    ExportError, ExportReport, ExportScene, ExportSink, Exporter, ExporterCaps, ExporterInfo, ExporterRegistry, MemorySink,
    UpAxis,
};
use serde_json::{Map, Value};

use crate::abi::*;

/// A loaded plugin. Keeps its library loaded for as long as the exporter exists.
pub struct PluginExporter {
    plugin: *const AdExporterPlugin,
    info: ExporterInfo,
    options: Vec<ParamSpec>,
    /// Built-in exporters for `AdHost.export_as`.
    builtins: ExporterRegistry,
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    _library: Option<Arc<libloading::Library>>,
}

// The plugin table is immutable static data; the host calls export_scene from one thread at
// a time (the ABI documents no re-entrancy guarantees beyond that).
unsafe impl Send for PluginExporter {}
unsafe impl Sync for PluginExporter {}

impl PluginExporter {
    /// From an entry point that is linked into the app (static plugins, used on iOS).
    ///
    /// # Safety
    /// `entry` must follow the ABI in `airdraw_export.h`.
    pub unsafe fn from_entry(entry: AdExporterEntryFn) -> Result<Self, String> {
        unsafe { Self::new(entry, None) }
    }

    /// Loads a plugin library and resolves `airdraw_exporter_entry`.
    ///
    /// # Safety
    /// Loading a library runs its initializers: only load plugins the user chose to install.
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    pub unsafe fn load(path: &Path) -> Result<Self, String> {
        let lib = unsafe { libloading::Library::new(path) }.map_err(|e| e.to_string())?;
        let entry: AdExporterEntryFn = *unsafe { lib.get::<AdExporterEntryFn>(ENTRY_SYMBOL) }
            .map_err(|_| "not an AirDraw exporter plugin (no airdraw_exporter_entry)".to_string())?;
        unsafe { Self::new(entry, Some(Arc::new(lib))) }
    }

    #[allow(unused_variables)]
    unsafe fn new(entry: AdExporterEntryFn, library: Option<Arc<SharedLibrary>>) -> Result<Self, String> {
        let plugin = unsafe { entry(ABI_VERSION) };
        if plugin.is_null() {
            return Err(format!("plugin doesn't support AirDraw exporter ABI v{ABI_VERSION}"));
        }
        let p = unsafe { &*plugin };
        if p.abi_version != ABI_VERSION {
            return Err(format!("plugin uses ABI v{}, this app supports v{ABI_VERSION}", p.abi_version));
        }
        if p.export_scene.is_none() {
            return Err("plugin has no export_scene function".into());
        }
        let text = |s: *const c_char, what: &str| unsafe { cstr(s) }.ok_or_else(|| format!("plugin {what} is missing"));
        let info = ExporterInfo {
            id: text(p.id, "id")?,
            name: text(p.name, "name")?,
            extension: text(p.extension, "extension")?,
            caps: ExporterCaps {
                meshes: p.caps & CAP_MESHES != 0,
                curves: p.caps & CAP_CURVES != 0,
                materials: p.caps & CAP_MATERIALS != 0,
                hierarchy: p.caps & CAP_HIERARCHY != 0,
                multi_file: p.caps & CAP_MULTI_FILE != 0,
                requires_curves: false,
            },
        };
        let options = match unsafe { cstr(p.options_json) } {
            Some(json) => parse_options(&json).map_err(|e| format!("options_json: {e}"))?,
            None => Vec::new(),
        };
        Ok(Self {
            plugin,
            info,
            options,
            builtins: ExporterRegistry::with_builtins(),
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            _library: library,
        })
    }
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
type SharedLibrary = libloading::Library;
#[cfg(any(target_os = "ios", target_os = "android"))]
type SharedLibrary = ();

impl Drop for PluginExporter {
    fn drop(&mut self) {
        if let Some(destroy) = unsafe { (*self.plugin).destroy } {
            unsafe { destroy() };
        }
    }
}

/// Loads every plugin library (`.dylib`, `.so`, `.dll`) in `dir`. A missing folder yields
/// nothing; each file reports its own success or error.
///
/// # Safety
/// See [`PluginExporter::load`].
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub unsafe fn load_dir(dir: &Path) -> Vec<(PathBuf, Result<PluginExporter, String>)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("dylib" | "so" | "dll")))
        .collect();
    paths.sort();
    paths.into_iter().map(|p| (p.clone(), unsafe { PluginExporter::load(&p) })).collect()
}

impl Exporter for PluginExporter {
    fn info(&self) -> ExporterInfo {
        self.info.clone()
    }

    fn options(&self) -> Vec<ParamSpec> {
        self.options.clone()
    }

    fn export(&self, scene: &ExportScene, opts: &Params, out: &mut dyn ExportSink) -> Result<ExportReport, ExportError> {
        let view = SceneView::new(scene);
        let options = CString::new(options_json(opts)).expect("JSON has no NUL bytes");
        let stem = c_string(out.stem());

        let mut sink_ctx = SinkCtx { out, files: Vec::new(), error: None };
        let sink = AdSink { ctx: &mut sink_ctx as *mut SinkCtx as *mut c_void, stem: stem.as_ptr(), write: sink_write };
        let host_ctx = HostCtx { scene, builtins: &self.builtins, stem: stem.to_string_lossy().into_owned(), warnings: RefCell::default() };
        let host = AdHost { ctx: &host_ctx as *const HostCtx as *mut c_void, export_as: host_export_as, warn: host_warn };

        let mut error = vec![0 as c_char; 1024];
        let export_scene = unsafe { (*self.plugin).export_scene }.expect("checked at load");
        let status = unsafe { export_scene(&view.scene, options.as_ptr(), &sink, &host, error.as_mut_ptr(), error.len() - 1) };

        if let Some(e) = sink_ctx.error {
            return Err(e);
        }
        if status != 0 {
            let msg = unsafe { cstr(error.as_ptr()) }.filter(|m| !m.is_empty()).unwrap_or_else(|| format!("error code {status}"));
            return Err(ExportError::Failed(format!("{}: {msg}", self.info.name)));
        }
        Ok(ExportReport { files: sink_ctx.files, warnings: host_ctx.warnings.into_inner() })
    }
}

/// C views over an `ExportScene`, plus the strings they point to.
struct SceneView {
    scene: AdExportScene,
    _nodes: Vec<AdExportNode>,
    _meshes: Vec<AdExportMesh>,
    _materials: Vec<AdExportMaterial>,
    _curves: Vec<AdExportCurve>,
    _strings: Vec<CString>,
}

impl SceneView {
    fn new(s: &ExportScene) -> Self {
        let mut strings = Vec::new();
        // CString data lives on the heap, so its pointer stays valid when the Vec grows.
        let mut intern = |text: &str| {
            let c = c_string(text);
            let ptr = c.as_ptr();
            strings.push(c);
            ptr
        };
        let idx = |i: Option<usize>| i.map_or(-1, |i| i as i32);
        let nodes: Vec<AdExportNode> = s
            .nodes
            .iter()
            .map(|n| AdExportNode {
                name: intern(&n.name),
                parent: idx(n.parent),
                transform: n.transform.to_cols_array(),
                mesh: idx(n.mesh),
                curve: idx(n.curve),
                layer: n.layer.as_deref().map_or(std::ptr::null(), &mut intern),
            })
            .collect();
        let meshes: Vec<AdExportMesh> = s
            .meshes
            .iter()
            .map(|m| AdExportMesh {
                name: intern(&m.name),
                positions: m.positions.as_ptr().cast(),
                normals: if m.normals.len() == m.positions.len() { m.normals.as_ptr().cast() } else { std::ptr::null() },
                vertex_count: m.positions.len() as u32,
                indices: m.indices.as_ptr(),
                index_count: m.indices.len() as u32,
                material: m.material as u32,
            })
            .collect();
        let materials: Vec<AdExportMaterial> = s
            .materials
            .iter()
            .map(|m| AdExportMaterial {
                name: intern(&m.name),
                base_color: m.base_color,
                roughness: m.roughness,
                metallic: m.metallic,
                emission: m.emission,
                double_sided: m.double_sided as u8,
                unlit: m.unlit as u8,
            })
            .collect();
        let curves: Vec<AdExportCurve> = s
            .curves
            .iter()
            .map(|c| AdExportCurve {
                points: c.points.as_ptr().cast(),
                widths: c.widths.as_ptr(),
                point_count: c.points.len() as u32,
                material: c.material as u32,
            })
            .collect();
        let meta = AdSceneMeta {
            name: intern(&s.meta.name),
            generator: intern(&s.meta.generator),
            up_axis: if s.meta.up == UpAxis::Z { UP_Z } else { UP_Y },
        };
        let scene = AdExportScene {
            meta,
            nodes: nodes.as_ptr(),
            node_count: nodes.len() as u32,
            meshes: meshes.as_ptr(),
            mesh_count: meshes.len() as u32,
            materials: materials.as_ptr(),
            material_count: materials.len() as u32,
            curves: curves.as_ptr(),
            curve_count: curves.len() as u32,
        };
        Self { scene, _nodes: nodes, _meshes: meshes, _materials: materials, _curves: curves, _strings: strings }
    }
}

struct SinkCtx<'a> {
    out: &'a mut dyn ExportSink,
    files: Vec<String>,
    error: Option<ExportError>,
}

struct HostCtx<'a> {
    scene: &'a ExportScene,
    builtins: &'a ExporterRegistry,
    stem: String,
    warnings: RefCell<Vec<String>>,
}

// Callbacks run inside plugin code: a Rust panic must never unwind into C/C++.

unsafe extern "C" fn sink_write(ctx: *mut c_void, name: *const c_char, data: *const u8, len: usize) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let ctx = unsafe { &mut *(ctx as *mut SinkCtx) };
        let Some(name) = (unsafe { cstr(name) }) else { return 1 };
        // Bare file names only: plugins can't write outside the export folder.
        if name.is_empty() || name.contains(['/', '\\']) || name == ".." {
            ctx.error = Some(ExportError::Failed(format!("plugin tried to write invalid file name '{name}'")));
            return 1;
        }
        let bytes = if len == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(data, len) } };
        match ctx.out.write(&name, bytes) {
            Ok(()) => {
                ctx.files.push(name);
                0
            }
            Err(e) => {
                ctx.error = Some(e);
                1
            }
        }
    }))
    .unwrap_or(1)
}

unsafe extern "C" fn host_export_as(ctx: *mut c_void, id: *const c_char, receive: AdReceiveFn, user: *mut c_void) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let ctx = unsafe { &*(ctx as *const HostCtx) };
        let Some(id) = (unsafe { cstr(id) }) else { return 1 };
        let mut mem = MemorySink::new(&ctx.stem);
        if ctx.builtins.export(&id, ctx.scene, &Params::new(), &mut mem).is_err() {
            return 1;
        }
        for (name, data) in &mem.files {
            let name = c_string(name);
            unsafe { receive(user, name.as_ptr(), data.as_ptr(), data.len()) };
        }
        0
    }))
    .unwrap_or(1)
}

unsafe extern "C" fn host_warn(ctx: *mut c_void, message: *const c_char) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let ctx = unsafe { &*(ctx as *const HostCtx) };
        if let Some(m) = unsafe { cstr(message) } {
            ctx.warnings.borrow_mut().push(m);
        }
    }));
}

unsafe fn cstr(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn c_string(s: &str) -> CString {
    CString::new(s.replace('\0', "")).expect("NULs removed")
}

/// Validated option values → the JSON object plugins receive.
fn options_json(opts: &Params) -> String {
    let obj: Map<String, Value> = opts
        .iter()
        .map(|(k, v)| {
            let v = match v {
                ParamValue::Bool(b) => Value::from(*b),
                ParamValue::Int(i) => Value::from(*i),
                ParamValue::Float(f) => Value::from(*f),
                ParamValue::Str(s) => Value::from(s.as_str()),
                ParamValue::Curve(pts) => Value::from(pts.iter().map(|p| vec![p[0], p[1]]).collect::<Vec<_>>()),
            };
            (k.clone(), v)
        })
        .collect();
    Value::Object(obj).to_string()
}

/// Plugin option descriptions → parameter schemas. Names and docs live for the rest of the
/// program (schemas use `&'static str`); plugins are loaded once, so this is bounded.
fn parse_options(json: &str) -> Result<Vec<ParamSpec>, String> {
    let leak = |s: &str| -> &'static str { Box::leak(s.to_string().into_boxed_str()) };
    let items: Vec<Value> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    items
        .iter()
        .map(|o| {
            let name = o["name"].as_str().ok_or("option without a name")?;
            let doc = leak(o["doc"].as_str().unwrap_or(""));
            let kind = o["kind"].as_str().ok_or_else(|| format!("option '{name}' has no kind"))?;
            let default = &o["default"];
            let bad = || format!("option '{name}': default doesn't match kind '{kind}'");
            let name = leak(name);
            Ok(match kind {
                "bool" => ParamSpec::bool(name, default.as_bool().ok_or_else(bad)?, doc),
                "int" => ParamSpec::int(
                    name,
                    default.as_i64().ok_or_else(bad)?,
                    o["min"].as_i64().unwrap_or(-1_000_000_000),
                    o["max"].as_i64().unwrap_or(1_000_000_000),
                    doc,
                ),
                "float" => ParamSpec::float(
                    name,
                    default.as_f64().ok_or_else(bad)?,
                    o["min"].as_f64().unwrap_or(-1e12),
                    o["max"].as_f64().unwrap_or(1e12),
                    doc,
                ),
                "color" => ParamSpec::color(name, default.as_str().ok_or_else(bad)?, doc),
                "string" => ParamSpec {
                    name,
                    default: ParamValue::Str(default.as_str().ok_or_else(bad)?.into()),
                    min: None,
                    max: None,
                    hint: Hint::None,
                    doc,
                },
                other => return Err(format!("option '{name}': unknown kind '{other}'")),
            })
        })
        .collect()
}
