//! Rust mirror of `include/airdraw_export.h`. Layout is checked against the header by the
//! `layout_matches_header` test, which compiles C code that reports sizes and offsets.

use std::ffi::{c_char, c_void};

pub const ABI_VERSION: u32 = 1;
pub const ENTRY_SYMBOL: &[u8] = b"airdraw_exporter_entry\0";

pub const UP_Y: u32 = 0;
pub const UP_Z: u32 = 1;

pub const CAP_MESHES: u32 = 1 << 0;
pub const CAP_CURVES: u32 = 1 << 1;
pub const CAP_MATERIALS: u32 = 1 << 2;
pub const CAP_HIERARCHY: u32 = 1 << 3;
pub const CAP_MULTI_FILE: u32 = 1 << 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdVec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[repr(C)]
pub struct AdSceneMeta {
    pub name: *const c_char,
    pub generator: *const c_char,
    pub up_axis: u32,
}

#[repr(C)]
pub struct AdExportNode {
    pub name: *const c_char,
    pub parent: i32,
    pub transform: [f32; 16],
    pub mesh: i32,
    pub curve: i32,
    pub layer: *const c_char,
}

#[repr(C)]
pub struct AdExportMesh {
    pub name: *const c_char,
    pub positions: *const AdVec3,
    pub normals: *const AdVec3,
    pub vertex_count: u32,
    pub indices: *const u32,
    pub index_count: u32,
    pub material: u32,
}

#[repr(C)]
pub struct AdExportMaterial {
    pub name: *const c_char,
    pub base_color: [f32; 4],
    pub roughness: f32,
    pub metallic: f32,
    pub emission: f32,
    pub double_sided: u8,
    pub unlit: u8,
}

#[repr(C)]
pub struct AdExportCurve {
    pub points: *const AdVec3,
    pub widths: *const f32,
    pub point_count: u32,
    pub material: u32,
}

#[repr(C)]
pub struct AdExportScene {
    pub meta: AdSceneMeta,
    pub nodes: *const AdExportNode,
    pub node_count: u32,
    pub meshes: *const AdExportMesh,
    pub mesh_count: u32,
    pub materials: *const AdExportMaterial,
    pub material_count: u32,
    pub curves: *const AdExportCurve,
    pub curve_count: u32,
}

pub type AdWriteFn = unsafe extern "C" fn(ctx: *mut c_void, file_name: *const c_char, data: *const u8, len: usize) -> i32;
pub type AdReceiveFn = unsafe extern "C" fn(user: *mut c_void, file_name: *const c_char, data: *const u8, len: usize);
pub type AdExportAsFn = unsafe extern "C" fn(ctx: *mut c_void, exporter_id: *const c_char, receive: AdReceiveFn, user: *mut c_void) -> i32;
pub type AdWarnFn = unsafe extern "C" fn(ctx: *mut c_void, message: *const c_char);

#[repr(C)]
pub struct AdSink {
    pub ctx: *mut c_void,
    pub stem: *const c_char,
    pub write: AdWriteFn,
}

#[repr(C)]
pub struct AdHost {
    pub ctx: *mut c_void,
    pub export_as: AdExportAsFn,
    pub warn: AdWarnFn,
}

pub type AdExportSceneFn = unsafe extern "C" fn(
    scene: *const AdExportScene,
    options_json: *const c_char,
    sink: *const AdSink,
    host: *const AdHost,
    error: *mut c_char,
    error_len: usize,
) -> i32;

#[repr(C)]
pub struct AdExporterPlugin {
    pub abi_version: u32,
    pub id: *const c_char,
    pub name: *const c_char,
    pub extension: *const c_char,
    pub caps: u32,
    pub options_json: *const c_char,
    pub export_scene: Option<AdExportSceneFn>,
    pub destroy: Option<unsafe extern "C" fn()>,
}

pub type AdExporterEntryFn = unsafe extern "C" fn(host_abi_version: u32) -> *const AdExporterPlugin;

// Positions/normals are passed zero-copy from glam::Vec3 arrays.
const _: () = assert!(size_of::<glam::Vec3>() == size_of::<AdVec3>() && align_of::<glam::Vec3>() == align_of::<AdVec3>());
