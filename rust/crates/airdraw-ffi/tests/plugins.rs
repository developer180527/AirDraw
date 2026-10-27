//! Plugin ABI tests: header/Rust layout agreement, a Rust-implemented plugin through the C
//! ABI, and the real C++ sample plugin compiled and loaded as a shared library.

use std::ffi::{CStr, c_char};
use std::path::{Path, PathBuf};
use std::process::Command;

use airdraw_brush::{ParamValue, Params};
use airdraw_export::{
    ExportCurve, ExportMaterial, ExportMesh, ExportNode, ExportScene, ExporterRegistry, MemorySink, UpAxis,
    scene::SceneMeta,
};
use airdraw_ffi::abi::*;
use airdraw_ffi::{HEADER_DIR, PluginExporter};
use glam::{Mat4, Vec3};

fn scene() -> ExportScene {
    let tri = |x: f32| ExportMesh {
        name: format!("stroke_{x}"),
        positions: vec![Vec3::new(x, 0.0, 0.0), Vec3::new(x + 1.0, 0.0, 0.0), Vec3::new(x, 1.0, 0.0)],
        normals: vec![Vec3::Z; 3],
        indices: vec![0, 1, 2],
        material: 0,
    };
    let node = |name: &str, parent: Option<usize>, mesh: Option<usize>, curve: Option<usize>| ExportNode {
        name: name.into(),
        parent,
        transform: Mat4::IDENTITY,
        mesh,
        curve,
        layer: Some("Layer 1".into()),
    };
    ExportScene {
        meta: SceneMeta { name: "test".into(), up: UpAxis::Y, generator: "AirDraw test".into() },
        nodes: vec![node("Layer 1", None, None, None), node("a", Some(0), Some(0), None), node("b", Some(0), Some(1), None), node("c", Some(0), None, Some(0))],
        meshes: vec![tri(0.0), tri(2.0)],
        materials: vec![ExportMaterial {
            name: "Ink_0".into(),
            base_color: [0.1, 0.2, 0.3, 1.0],
            roughness: 0.6,
            metallic: 0.0,
            emission: 0.0,
            double_sided: true,
            unlit: false,
        }],
        curves: vec![ExportCurve { points: vec![Vec3::ZERO, Vec3::X], widths: vec![0.01, 0.02], material: 0 }],
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("airdraw-ffi-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn compiler(name: &str) -> Option<&str> {
    Command::new(name).arg("--version").output().ok().filter(|o| o.status.success()).map(|_| name)
}

// ---- Layout ------------------------------------------------------------------------------

#[test]
fn layout_matches_header() {
    let Some(cc) = compiler("cc") else {
        eprintln!("skipping: no C compiler");
        return;
    };
    let dir = temp_dir("layout");
    let src = dir.join("layout.c");
    let checks: &[(&str, &[&str])] = &[
        ("AdVec3", &["x", "y", "z"]),
        ("AdSceneMeta", &["name", "generator", "up_axis"]),
        ("AdExportNode", &["name", "parent", "transform", "mesh", "curve", "layer"]),
        ("AdExportMesh", &["name", "positions", "normals", "vertex_count", "indices", "index_count", "material"]),
        ("AdExportMaterial", &["name", "base_color", "roughness", "metallic", "emission", "double_sided", "unlit"]),
        ("AdExportCurve", &["points", "widths", "point_count", "material"]),
        ("AdExportScene", &["meta", "nodes", "node_count", "meshes", "mesh_count", "materials", "material_count", "curves", "curve_count"]),
        ("AdSink", &["ctx", "stem", "write"]),
        ("AdHost", &["ctx", "export_as", "warn"]),
        ("AdExporterPlugin", &["abi_version", "id", "name", "extension", "caps", "options_json", "export_scene", "destroy"]),
    ];
    let mut c = String::from("#include <stdio.h>\n#include <stddef.h>\n#include \"airdraw_export.h\"\nint main(void) {\n");
    for (ty, fields) in checks {
        c += &format!("printf(\"{ty} %zu\\n\", sizeof({ty}));\n");
        for f in *fields {
            c += &format!("printf(\"{ty}.{f} %zu\\n\", offsetof({ty}, {f}));\n");
        }
    }
    c += "return 0; }\n";
    std::fs::write(&src, c).unwrap();
    let exe = dir.join("layout");
    let st = Command::new(cc).args(["-std=c11", "-I", HEADER_DIR, "-o"]).arg(&exe).arg(&src).status().unwrap();
    assert!(st.success(), "header must compile as C11");
    let out = String::from_utf8(Command::new(&exe).output().unwrap().stdout).unwrap();
    let c_layout: std::collections::HashMap<&str, usize> =
        out.lines().map(|l| l.split_once(' ').map(|(k, v)| (k, v.parse().unwrap())).unwrap()).collect();

    macro_rules! check {
        ($ty:ident: $($f:ident),*) => {
            assert_eq!(c_layout[stringify!($ty)], size_of::<$ty>(), "size of {}", stringify!($ty));
            $(assert_eq!(c_layout[concat!(stringify!($ty), ".", stringify!($f))], std::mem::offset_of!($ty, $f),
                "offset of {}.{}", stringify!($ty), stringify!($f));)*
        };
    }
    check!(AdVec3: x, y, z);
    check!(AdSceneMeta: name, generator, up_axis);
    check!(AdExportNode: name, parent, transform, mesh, curve, layer);
    check!(AdExportMesh: name, positions, normals, vertex_count, indices, index_count, material);
    check!(AdExportMaterial: name, base_color, roughness, metallic, emission, double_sided, unlit);
    check!(AdExportCurve: points, widths, point_count, material);
    check!(AdExportScene: meta, nodes, node_count, meshes, mesh_count, materials, material_count, curves, curve_count);
    check!(AdSink: ctx, stem, write);
    check!(AdHost: ctx, export_as, warn);
    check!(AdExporterPlugin: abi_version, id, name, extension, caps, options_json, export_scene, destroy);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- A plugin written in Rust against the C ABI -------------------------------------------

unsafe fn s(p: *const c_char) -> String {
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

unsafe extern "C" fn collect(user: *mut std::ffi::c_void, name: *const c_char, data: *const u8, len: usize) {
    let out = unsafe { &mut *(user as *mut Vec<(String, Vec<u8>)>) };
    out.push((unsafe { s(name) }, unsafe { std::slice::from_raw_parts(data, len) }.to_vec()));
}

/// Writes a text summary of what it received; behavior switches on the "mode" option.
unsafe extern "C" fn summary_export(
    scene: *const AdExportScene,
    options: *const c_char,
    sink: *const AdSink,
    host: *const AdHost,
    error: *mut c_char,
    error_len: usize,
) -> i32 {
    let (scene, sink, host) = unsafe { (&*scene, &*sink, &*host) };
    let options = unsafe { s(options) };
    if options.contains("\"mode\":\"fail\"") {
        let msg = b"cooker exploded\0";
        unsafe { std::ptr::copy_nonoverlapping(msg.as_ptr().cast(), error, msg.len().min(error_len)) };
        return 1;
    }
    if options.contains("\"mode\":\"escape\"") {
        let name = c"../evil.txt";
        return unsafe { (sink.write)(sink.ctx, name.as_ptr(), b"x".as_ptr(), 1) };
    }
    let meshes = unsafe { std::slice::from_raw_parts(scene.meshes, scene.mesh_count as usize) };
    let verts: u32 = meshes.iter().map(|m| m.vertex_count).sum();
    let first = unsafe { *meshes[0].positions.add(1) };
    let mat = unsafe { &*scene.materials };
    let text = format!(
        "nodes={} meshes={} verts={} curves={} v1=({},{},{}) material={} up={} options={options}",
        scene.node_count,
        scene.mesh_count,
        verts,
        scene.curve_count,
        first.x,
        first.y,
        first.z,
        unsafe { s(mat.name) },
        scene.meta.up_axis
    );
    let name = std::ffi::CString::new(format!("{}.txt", unsafe { s(sink.stem) })).unwrap();
    if unsafe { (sink.write)(sink.ctx, name.as_ptr(), text.as_ptr(), text.len()) } != 0 {
        return 1;
    }
    let mut glb: Vec<(String, Vec<u8>)> = Vec::new();
    if unsafe { (host.export_as)(host.ctx, c"com.airdraw.glb".as_ptr(), collect, (&mut glb as *mut Vec<_>).cast()) } != 0 {
        return 1;
    }
    let name = std::ffi::CString::new(format!("from_host_{}", glb[0].0)).unwrap();
    unsafe { (sink.write)(sink.ctx, name.as_ptr(), glb[0].1.as_ptr(), glb[0].1.len()) };
    unsafe { (host.warn)(host.ctx, c"curves ignored".as_ptr()) };
    0
}

struct SyncPlugin(AdExporterPlugin);
unsafe impl Sync for SyncPlugin {}

static SUMMARY: SyncPlugin = SyncPlugin(AdExporterPlugin {
    abi_version: ABI_VERSION,
    id: c"test.summary".as_ptr(),
    name: c"Summary".as_ptr(),
    extension: c"txt".as_ptr(),
    caps: CAP_MESHES,
    options_json: c"[{\"name\":\"mode\",\"kind\":\"string\",\"default\":\"ok\"},{\"name\":\"lod\",\"kind\":\"int\",\"default\":2,\"min\":0,\"max\":4}]".as_ptr(),
    export_scene: Some(summary_export),
    destroy: None,
});

unsafe extern "C" fn summary_entry(version: u32) -> *const AdExporterPlugin {
    if version == ABI_VERSION { &SUMMARY.0 } else { std::ptr::null() }
}

unsafe extern "C" fn refusing_entry(_version: u32) -> *const AdExporterPlugin {
    std::ptr::null()
}

fn registry_with_summary() -> ExporterRegistry {
    let mut reg = ExporterRegistry::with_builtins();
    reg.register(unsafe { PluginExporter::from_entry(summary_entry) }.unwrap());
    reg
}

#[test]
fn rust_plugin_receives_scene_options_and_host_services() {
    let reg = registry_with_summary();
    let mut sink = MemorySink::new("out");
    let opts: Params = [("lod".to_string(), ParamValue::Float(3.0))].into();
    let report = reg.export("test.summary", &scene(), &opts, &mut sink).unwrap();
    let text = String::from_utf8(sink.files["out.txt"].clone()).unwrap();
    assert!(text.starts_with("nodes=4 meshes=2 verts=6 curves=1 v1=(1,0,0) material=Ink_0 up=0"), "{text}");
    // Options arrive validated, defaulted, and coerced to the declared kind.
    assert!(text.contains("\"lod\":3") && text.contains("\"mode\":\"ok\""), "{text}");
    assert_eq!(&sink.files["from_host_out.glb"][..4], b"glTF");
    assert_eq!(report.files, ["out.txt", "from_host_out.glb"]);
    assert!(report.warnings.contains(&"curves ignored".to_string()));
    assert!(report.warnings.iter().any(|w| w.contains("can't store curves")), "host adds its own caps warning");
}

#[test]
fn plugin_errors_are_reported_and_writes_are_confined() {
    let reg = registry_with_summary();
    let fail: Params = [("mode".to_string(), ParamValue::Str("fail".into()))].into();
    let e = reg.export("test.summary", &scene(), &fail, &mut MemorySink::new("x")).unwrap_err();
    assert_eq!(e.to_string(), "export failed: Summary: cooker exploded");
    let escape: Params = [("mode".to_string(), ParamValue::Str("escape".into()))].into();
    let mut sink = MemorySink::new("x");
    let e = reg.export("test.summary", &scene(), &escape, &mut sink).unwrap_err();
    assert!(e.to_string().contains("invalid file name"), "{e}");
    assert!(sink.files.is_empty());
}

#[test]
fn incompatible_plugin_is_rejected() {
    let e = unsafe { PluginExporter::from_entry(refusing_entry) }.err().unwrap();
    assert!(e.contains("ABI v1"), "{e}");
}

// ---- The C++ sample plugin, built and loaded for real -------------------------------------

fn build_sample(dir: &Path) -> Option<PathBuf> {
    let cxx = compiler("c++")?;
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../plugins/sample_engine_exporter/sample_engine_exporter.cpp");
    let ext = if cfg!(target_os = "macos") { "dylib" } else if cfg!(windows) { "dll" } else { "so" };
    let lib = dir.join(format!("libsample_engine_exporter.{ext}"));
    let st = Command::new(cxx)
        .args(["-std=c++17", "-shared", "-fPIC", "-fvisibility=hidden", "-Wall", "-Werror", "-I", HEADER_DIR, "-o"])
        .arg(&lib)
        .arg(src)
        .status()
        .ok()?;
    assert!(st.success(), "sample plugin must compile cleanly");
    Some(lib)
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[test]
fn cpp_sample_plugin_loads_and_exports() {
    let dir = temp_dir("cpp");
    let Some(lib) = build_sample(&dir) else {
        eprintln!("skipping: no C++ compiler");
        return;
    };
    let loaded = unsafe { airdraw_ffi::load_dir(&dir) };
    assert_eq!(loaded.len(), 1);
    let (path, plugin) = loaded.into_iter().next().unwrap();
    assert_eq!(path, lib);
    let plugin = plugin.unwrap();
    let mut reg = ExporterRegistry::with_builtins();
    reg.register(plugin);
    let info = reg.get("com.example.sample_engine").unwrap().info();
    assert_eq!((info.name.as_str(), info.extension.as_str()), ("Sample Engine Mesh", "sem"));
    let options: Vec<_> = reg.get("com.example.sample_engine").unwrap().options().iter().map(|o| o.name).collect();
    assert_eq!(options, ["engine_units_per_meter", "normals", "index16", "also_glb"]);

    let opts: Params = [
        ("engine_units_per_meter".to_string(), ParamValue::Float(100.0)),
        ("also_glb".to_string(), ParamValue::Bool(true)),
    ]
    .into();
    let mut sink = MemorySink::new("level1");
    let report = reg.export("com.example.sample_engine", &scene(), &opts, &mut sink).unwrap();
    assert_eq!(report.files, ["level1.sem", "level1_source.glb"]);
    assert_eq!(report.warnings, ["Sample Engine Mesh can't store curves; they were skipped"]);
    assert_eq!(&sink.files["level1_source.glb"][..4], b"glTF");

    // Parse the .sem back.
    let b = &sink.files["level1.sem"];
    let u32_at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let f32_at = |i: usize| f32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    assert_eq!(&b[..4], b"SEM1");
    assert_eq!(u32_at(4), 1);
    assert_eq!(u32_at(8), 1, "one material");
    let name_len = u16::from_le_bytes([b[12], b[13]]) as usize;
    assert_eq!(&b[14..14 + name_len], b"Ink_0");
    let mut p = 14 + name_len;
    assert!((f32_at(p) - 0.1).abs() < 1e-6);
    p += 16 + 12 + 1; // rgba, roughness/metallic/emission, flags
    assert_eq!(u32_at(p), 2, "two meshes");
    p += 4;
    let mesh_name_len = u16::from_le_bytes([b[p], b[p + 1]]) as usize;
    p += 2 + mesh_name_len;
    assert_eq!((u32_at(p), u32_at(p + 4), u32_at(p + 8)), (0, 3, 3), "material, vertices, indices");
    assert_eq!(b[p + 12], 0b11, "normals + 16-bit indices");
    p += 13;
    assert_eq!(f32_at(p + 12), 100.0, "second vertex x scaled to engine units");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[test]
fn non_plugin_library_is_rejected() {
    let Some(cc) = compiler("cc") else { return };
    let dir = temp_dir("notplugin");
    let src = dir.join("x.c");
    std::fs::write(&src, "int something(void) { return 1; }\n").unwrap();
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let lib = dir.join(format!("libx.{ext}"));
    assert!(Command::new(cc).args(["-shared", "-fPIC", "-o"]).arg(&lib).arg(&src).status().unwrap().success());
    let e = unsafe { PluginExporter::load(&lib) }.err().unwrap();
    assert!(e.contains("not an AirDraw exporter plugin"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}
