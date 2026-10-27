/*
 * AirDraw exporter plugin ABI.
 *
 * A plugin is a shared library (.dylib / .so / .dll) exporting one C function,
 * `airdraw_exporter_entry`, that returns a static AdExporterPlugin. On iOS, where apps can't
 * load downloaded code, the same plugin source is linked statically and registered by the
 * host build instead.
 *
 * Rules:
 *  - Plain C types only. No C++ types, exceptions or allocators cross this boundary:
 *    catch everything inside export_scene and report failures through its return value.
 *  - All scene data is read-only and valid only during export_scene. Copy what you keep.
 *  - Strings are UTF-8, NUL-terminated. Coordinates are meters, right-handed; the up axis is
 *    in AdSceneMeta (geometry is already converted and scaled).
 *  - Write output only through AdSink, never to paths of your own.
 *  - The ABI is versioned. The host passes its version to the entry point; return NULL if you
 *    can't support it. Fields are only ever appended within a major version.
 */
#ifndef AIRDRAW_EXPORT_H
#define AIRDRAW_EXPORT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define AIRDRAW_EXPORT_ABI_VERSION 1u
#define AIRDRAW_EXPORTER_ENTRY "airdraw_exporter_entry"

#if defined(_WIN32)
#define AIRDRAW_PLUGIN_EXPORT __declspec(dllexport)
#else
#define AIRDRAW_PLUGIN_EXPORT __attribute__((visibility("default")))
#endif

/* ---- Scene (read-only views) ---------------------------------------------------------- */

typedef struct AdVec3 {
    float x, y, z;
} AdVec3;

enum { AD_UP_Y = 0, AD_UP_Z = 1 };

typedef struct AdSceneMeta {
    const char *name;
    const char *generator; /* e.g. "AirDraw 0.1.0" */
    uint32_t up_axis;      /* AD_UP_Y or AD_UP_Z */
} AdSceneMeta;

typedef struct AdExportNode {
    const char *name;
    int32_t parent;       /* index into nodes, -1 = root */
    float transform[16];  /* column-major local transform (identity today: geometry is baked) */
    int32_t mesh;         /* index into meshes, -1 = none */
    int32_t curve;        /* index into curves, -1 = none */
    const char *layer;    /* may be NULL */
} AdExportNode;

typedef struct AdExportMesh {
    const char *name;
    const AdVec3 *positions;
    const AdVec3 *normals; /* vertex_count entries, or NULL */
    uint32_t vertex_count;
    const uint32_t *indices; /* triangle list */
    uint32_t index_count;
    uint32_t material;       /* index into materials */
} AdExportMesh;

typedef struct AdExportMaterial {
    const char *name;
    float base_color[4]; /* sRGB RGBA */
    float roughness;
    float metallic;
    float emission;      /* 0 = none; emits base_color */
    uint8_t double_sided;
    uint8_t unlit;
} AdExportMaterial;

/* A stroke's processed centerline, present when the user enabled curve export. */
typedef struct AdExportCurve {
    const AdVec3 *points;
    const float *widths; /* point_count entries, meters */
    uint32_t point_count;
    uint32_t material;
} AdExportCurve;

typedef struct AdExportScene {
    AdSceneMeta meta;
    const AdExportNode *nodes;
    uint32_t node_count;
    const AdExportMesh *meshes;
    uint32_t mesh_count;
    const AdExportMaterial *materials;
    uint32_t material_count;
    const AdExportCurve *curves;
    uint32_t curve_count;
} AdExportScene;

/* ---- Host services ------------------------------------------------------------------- */

/* Output target. `write` returns 0 on success. file_name is a bare name, e.g. "sketch.bin". */
typedef struct AdSink {
    void *ctx;
    const char *stem; /* base name chosen by the user, without extension */
    int32_t (*write)(void *ctx, const char *file_name, const uint8_t *data, size_t len);
} AdSink;

/* Receives one file produced by AdHost.export_as; data is valid only during the call. */
typedef void (*AdReceiveFn)(void *user, const char *file_name, const uint8_t *data, size_t len);

typedef struct AdHost {
    void *ctx;
    /* Exports the same scene with another registered exporter (e.g. "com.airdraw.glb") into
     * memory, calling `receive` per file. Lets a plugin feed an existing importer (FBX/GLB/OBJ)
     * instead of reading the scene directly. Returns 0 on success. */
    int32_t (*export_as)(void *ctx, const char *exporter_id, AdReceiveFn receive, void *user);
    /* Adds a warning shown to the user after the export. */
    void (*warn)(void *ctx, const char *message);
} AdHost;

/* ---- Plugin description -------------------------------------------------------------- */

enum {
    AD_CAP_MESHES = 1u << 0,
    AD_CAP_CURVES = 1u << 1,
    AD_CAP_MATERIALS = 1u << 2,
    AD_CAP_HIERARCHY = 1u << 3,
    AD_CAP_MULTI_FILE = 1u << 4,
};

typedef struct AdExporterPlugin {
    uint32_t abi_version; /* AIRDRAW_EXPORT_ABI_VERSION */
    const char *id;       /* stable reverse-DNS id, e.g. "com.mystudio.engine" */
    const char *name;     /* shown in the format picker */
    const char *extension;/* main file extension, without the dot */
    uint32_t caps;        /* AD_CAP_* */
    /* JSON array describing options; the host generates the export dialog from it:
     * [{"name":"lod","kind":"int","default":2,"min":0,"max":4,"doc":"..."}, ...]
     * kind: "bool" | "int" | "float" | "string" | "color" ("#RRGGBB"). May be NULL. */
    const char *options_json;
    /* options_json: JSON object with every option, validated and defaulted by the host.
     * Return 0 on success; otherwise write a message into `error` (at most error_len bytes,
     * NUL-terminated) and return non-zero. */
    int32_t (*export_scene)(const AdExportScene *scene, const char *options_json, const AdSink *sink,
                            const AdHost *host, char *error, size_t error_len);
    /* Called before the library is unloaded. May be NULL. */
    void (*destroy)(void);
} AdExporterPlugin;

typedef const AdExporterPlugin *(*AdExporterEntryFn)(uint32_t host_abi_version);

#ifdef __cplusplus
}
#endif

#endif /* AIRDRAW_EXPORT_H */
