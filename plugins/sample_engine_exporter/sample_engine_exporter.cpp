// Sample AirDraw exporter plugin: writes "Sample Engine Mesh" (.sem), a compact binary of the
// kind a game engine's asset cooker produces. It shows both ways to integrate a cooker:
//
//  1. Direct: read AdExportScene (meshes, materials, nodes) and write the engine format.
//  2. Via an existing importer: ask the host for GLB bytes with host->export_as() and hand
//     them to the cooker's GLB/FBX/OBJ importer (here: saved next to the output, `also_glb`).
//
// .sem layout (little-endian):
//   "SEM1" u32 version=1
//   u32 material_count, per material: str name, f32 rgba[4], f32 roughness, f32 metallic,
//                                     f32 emission, u8 flags (1 = double-sided, 2 = unlit)
//   u32 mesh_count, per mesh: str name, u32 material, u32 vertex_count, u32 index_count,
//                             u8 flags (1 = normals, 2 = 16-bit indices),
//                             positions f32x3 * n, [normals f32x3 * n], indices u16|u32
//   str = u16 byte length + UTF-8 bytes
//
// Build with CMakeLists.txt next to this file, then copy the library into AirDraw's plugins
// folder. Exceptions never leave export_scene: the ABI is plain C.

#include "airdraw_export.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

const char *const kOptions = R"json([
  {"name": "engine_units_per_meter", "kind": "float", "default": 1.0, "min": 0.001, "max": 1000.0,
   "doc": "Position scale for the engine (100 = centimeters)"},
  {"name": "normals", "kind": "bool", "default": true, "doc": "Store vertex normals"},
  {"name": "index16", "kind": "bool", "default": true,
   "doc": "16-bit indices for meshes under 65536 vertices (smaller files)"},
  {"name": "also_glb", "kind": "bool", "default": false,
   "doc": "Also save the GLB AirDraw generates, as a cooker with its own importer would use"}
])json";

class Writer {
public:
    std::vector<uint8_t> bytes;

    void raw(const void *p, size_t n) {
        auto b = static_cast<const uint8_t *>(p);
        bytes.insert(bytes.end(), b, b + n);
    }
    void u8(uint8_t v) { raw(&v, 1); }
    void u16(uint16_t v) { raw(&v, 2); }
    void u32(uint32_t v) { raw(&v, 4); }
    void f32(float v) { raw(&v, 4); }
    void str(const char *s) {
        size_t n = s ? std::strlen(s) : 0;
        if (n > 0xFFFF) throw std::runtime_error("name too long");
        u16(static_cast<uint16_t>(n));
        raw(s, n);
    }
};

// Minimal lookup in the host's flat options object ({"key":value,...}). Real plugins
// should use a JSON library; this keeps the sample dependency-free.
bool find_value(const char *json, const char *key, std::string &out) {
    std::string pattern = std::string("\"") + key + "\":";
    const char *p = std::strstr(json, pattern.c_str());
    if (!p) return false;
    p += pattern.size();
    while (*p == ' ') ++p;
    const char *end = p;
    while (*end && *end != ',' && *end != '}') ++end;
    out.assign(p, end);
    return true;
}

double opt_number(const char *json, const char *key, double fallback) {
    std::string v;
    return find_value(json, key, v) ? std::strtod(v.c_str(), nullptr) : fallback;
}

bool opt_bool(const char *json, const char *key, bool fallback) {
    std::string v;
    return find_value(json, key, v) ? v == "true" : fallback;
}

void write_file(const AdSink *sink, const std::string &name, const std::vector<uint8_t> &data) {
    if (sink->write(sink->ctx, name.c_str(), data.data(), data.size()) != 0)
        throw std::runtime_error("could not write " + name);
}

void receive_glb(void *user, const char *file_name, const uint8_t *data, size_t len) {
    if (std::strstr(file_name, ".glb")) static_cast<std::vector<uint8_t> *>(user)->assign(data, data + len);
}

void export_sem(const AdExportScene *scene, const char *options, const AdSink *sink, const AdHost *host) {
    const float scale = static_cast<float>(opt_number(options, "engine_units_per_meter", 1.0));
    const bool with_normals = opt_bool(options, "normals", true);
    const bool allow16 = opt_bool(options, "index16", true);

    Writer w;
    w.raw("SEM1", 4);
    w.u32(1);

    w.u32(scene->material_count);
    for (uint32_t i = 0; i < scene->material_count; ++i) {
        const AdExportMaterial &m = scene->materials[i];
        w.str(m.name);
        for (float c : m.base_color) w.f32(c);
        w.f32(m.roughness);
        w.f32(m.metallic);
        w.f32(m.emission);
        w.u8(static_cast<uint8_t>((m.double_sided ? 1 : 0) | (m.unlit ? 2 : 0)));
    }

    w.u32(scene->mesh_count);
    for (uint32_t i = 0; i < scene->mesh_count; ++i) {
        const AdExportMesh &m = scene->meshes[i];
        const bool normals = with_normals && m.normals != nullptr;
        const bool use16 = allow16 && m.vertex_count < 65536;
        if (allow16 && !use16) {
            // Something only the plugin knows; format-capability warnings (e.g. curves) come
            // from the host automatically.
            std::string msg = std::string(m.name) + " has over 65535 vertices; stored 32-bit indices";
            host->warn(host->ctx, msg.c_str());
        }
        w.str(m.name);
        w.u32(m.material);
        w.u32(m.vertex_count);
        w.u32(m.index_count);
        w.u8(static_cast<uint8_t>((normals ? 1 : 0) | (use16 ? 2 : 0)));
        for (uint32_t v = 0; v < m.vertex_count; ++v) {
            w.f32(m.positions[v].x * scale);
            w.f32(m.positions[v].y * scale);
            w.f32(m.positions[v].z * scale);
        }
        if (normals) w.raw(m.normals, sizeof(AdVec3) * m.vertex_count);
        for (uint32_t k = 0; k < m.index_count; ++k) {
            if (use16) w.u16(static_cast<uint16_t>(m.indices[k]));
            else w.u32(m.indices[k]);
        }
    }
    write_file(sink, std::string(sink->stem) + ".sem", w.bytes);

    if (opt_bool(options, "also_glb", false)) {
        std::vector<uint8_t> glb;
        if (host->export_as(host->ctx, "com.airdraw.glb", receive_glb, &glb) != 0 || glb.empty())
            throw std::runtime_error("host could not produce GLB");
        write_file(sink, std::string(sink->stem) + "_source.glb", glb);
    }
}

int32_t export_scene(const AdExportScene *scene, const char *options, const AdSink *sink, const AdHost *host,
                     char *error, size_t error_len) {
    try {
        export_sem(scene, options ? options : "{}", sink, host);
        return 0;
    } catch (const std::exception &e) {
        std::snprintf(error, error_len, "%s", e.what());
    } catch (...) {
        std::snprintf(error, error_len, "unknown error");
    }
    return 1;
}

const AdExporterPlugin kPlugin = {
    AIRDRAW_EXPORT_ABI_VERSION,
    "com.example.sample_engine",
    "Sample Engine Mesh",
    "sem",
    AD_CAP_MESHES | AD_CAP_MATERIALS | AD_CAP_MULTI_FILE,
    kOptions,
    export_scene,
    nullptr,
};

} // namespace

extern "C" AIRDRAW_PLUGIN_EXPORT const AdExporterPlugin *airdraw_exporter_entry(uint32_t host_abi_version) {
    return host_abi_version == AIRDRAW_EXPORT_ABI_VERSION ? &kPlugin : nullptr;
}
