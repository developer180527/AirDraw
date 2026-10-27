//! glTF 2.0, as a single binary `.glb` or `.gltf` + `.bin`. Meters, Y up (glTF's fixed
//! convention; the Z-up scene option is ignored with a warning). Curves become LINE_STRIP
//! primitives.

use airdraw_brush::{ParamSpec, Params, params::ParamsExt};
use glam::Vec3;
use serde_json::{Map, Value, json};

use crate::exporter::{ExportError, ExportReport, ExportSink, Exporter, ExporterCaps, ExporterInfo};
use crate::scene::{ExportScene, UpAxis, srgb_to_linear};

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;
const MODE_LINE_STRIP: u32 = 3;
const MODE_TRIANGLES: u32 = 4;

pub struct Glb;
pub struct Gltf;

fn caps() -> ExporterCaps {
    ExporterCaps { meshes: true, curves: true, materials: true, hierarchy: true, ..Default::default() }
}

fn options() -> Vec<ParamSpec> {
    vec![ParamSpec::bool("normals", true, "Include vertex normals (smooth shading)")]
}

impl Exporter for Glb {
    fn info(&self) -> ExporterInfo {
        ExporterInfo { id: "com.airdraw.glb".into(), name: "glTF Binary (GLB)".into(), extension: "glb".into(), caps: caps() }
    }
    fn options(&self) -> Vec<ParamSpec> {
        options()
    }
    fn export(&self, scene: &ExportScene, opts: &Params, out: &mut dyn ExportSink) -> Result<ExportReport, ExportError> {
        let (json, bin) = build(scene, opts, None);
        let name = format!("{}.glb", out.stem());
        out.write(&name, &glb(&json, &bin))?;
        Ok(report(scene, vec![name]))
    }
}

impl Exporter for Gltf {
    fn info(&self) -> ExporterInfo {
        ExporterInfo {
            id: "com.airdraw.gltf".into(),
            name: "glTF (separate .bin)".into(),
            extension: "gltf".into(),
            caps: ExporterCaps { multi_file: true, ..caps() },
        }
    }
    fn options(&self) -> Vec<ParamSpec> {
        options()
    }
    fn export(&self, scene: &ExportScene, opts: &Params, out: &mut dyn ExportSink) -> Result<ExportReport, ExportError> {
        let stem = out.stem().to_string();
        let bin_name = format!("{stem}.bin");
        let (json, bin) = build(scene, opts, Some(&bin_name));
        let gltf_name = format!("{stem}.gltf");
        out.write(&gltf_name, serde_json::to_string_pretty(&json).unwrap().as_bytes())?;
        let mut files = vec![gltf_name];
        if !bin.is_empty() {
            out.write(&bin_name, &bin)?;
            files.push(bin_name);
        }
        Ok(report(scene, files))
    }
}

fn report(scene: &ExportScene, files: Vec<String>) -> ExportReport {
    let mut warnings = Vec::new();
    if scene.meta.up == UpAxis::Z {
        warnings.push("glTF is always Y-up; importers convert it, so the Z-up option has no effect".into());
    }
    ExportReport { files, warnings }
}

/// Accumulates binary data and the views/accessors describing it.
struct Buffers {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Buffers {
    fn view(&mut self, bytes: &[u8], target: u32) -> usize {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        self.views.push(json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len(), "target": target}));
        self.bin.extend_from_slice(bytes);
        self.views.len() - 1
    }

    fn vec3s(&mut self, data: &[Vec3], with_bounds: bool) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|v| [v.x, v.y, v.z]).flat_map(f32::to_le_bytes).collect();
        let view = self.view(&bytes, ARRAY_BUFFER);
        let mut acc = json!({"bufferView": view, "componentType": FLOAT, "count": data.len(), "type": "VEC3"});
        if with_bounds {
            // Required for POSITION accessors.
            let min = data.iter().copied().fold(Vec3::splat(f32::MAX), Vec3::min);
            let max = data.iter().copied().fold(Vec3::splat(f32::MIN), Vec3::max);
            acc["min"] = json!([min.x, min.y, min.z]);
            acc["max"] = json!([max.x, max.y, max.z]);
        }
        self.accessors.push(acc);
        self.accessors.len() - 1
    }

    /// A float vertex attribute with `components` per vertex.
    fn floats(&mut self, data: &[f32], components: usize, kind: &str) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, ARRAY_BUFFER);
        self.accessors.push(json!({"bufferView": view, "componentType": FLOAT, "count": data.len() / components, "type": kind}));
        self.accessors.len() - 1
    }

    fn indices(&mut self, data: &[u32]) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|i| i.to_le_bytes()).collect();
        let view = self.view(&bytes, ELEMENT_ARRAY_BUFFER);
        self.accessors.push(json!({"bufferView": view, "componentType": UNSIGNED_INT, "count": data.len(), "type": "SCALAR"}));
        self.accessors.len() - 1
    }
}

fn build(scene: &ExportScene, opts: &Params, bin_uri: Option<&str>) -> (Value, Vec<u8>) {
    let with_normals = opts.flag("normals");
    let mut buf = Buffers { bin: Vec::new(), views: Vec::new(), accessors: Vec::new() };
    let mut extensions: Vec<&str> = Vec::new();

    let materials: Vec<Value> = scene
        .materials
        .iter()
        .map(|m| {
            let [r, g, b, a] = m.base_color;
            let lin = [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a];
            let mut v = json!({
                "name": m.name,
                "pbrMetallicRoughness": {"baseColorFactor": lin, "metallicFactor": m.metallic, "roughnessFactor": m.roughness},
                "doubleSided": m.double_sided,
            });
            if a < 1.0 {
                v["alphaMode"] = json!("BLEND");
            }
            let mut ext = Map::new();
            if m.emission > 0.0 {
                v["emissiveFactor"] = json!([lin[0], lin[1], lin[2]]);
                if m.emission > 1.0 {
                    ext.insert("KHR_materials_emissive_strength".into(), json!({"emissiveStrength": m.emission}));
                    extensions.push("KHR_materials_emissive_strength");
                }
            }
            if m.unlit {
                ext.insert("KHR_materials_unlit".into(), json!({}));
                extensions.push("KHR_materials_unlit");
            }
            if !ext.is_empty() {
                v["extensions"] = Value::Object(ext);
            }
            v
        })
        .collect();

    let mut meshes: Vec<Value> = Vec::new();
    let mut mesh_of_node: Vec<Option<usize>> = vec![None; scene.nodes.len()];
    for (ni, node) in scene.nodes.iter().enumerate() {
        if let Some(mi) = node.mesh {
            let m = &scene.meshes[mi];
            let mut attrs = json!({"POSITION": buf.vec3s(&m.positions, true)});
            if with_normals && m.normals.len() == m.positions.len() {
                attrs["NORMAL"] = json!(buf.vec3s(&m.normals, false));
                // glTF allows TANGENT only together with NORMAL.
                if m.tangents.len() == m.positions.len() {
                    let flat: Vec<f32> = m.tangents.iter().flat_map(|t| t.to_array()).collect();
                    attrs["TANGENT"] = json!(buf.floats(&flat, 4, "VEC4"));
                }
            }
            if m.uvs.len() == m.positions.len() {
                let flat: Vec<f32> = m.uvs.iter().flat_map(|u| u.to_array()).collect();
                attrs["TEXCOORD_0"] = json!(buf.floats(&flat, 2, "VEC2"));
            }
            let prim = json!({"attributes": attrs, "indices": buf.indices(&m.indices), "material": m.material, "mode": MODE_TRIANGLES});
            meshes.push(json!({"name": m.name, "primitives": [prim]}));
            mesh_of_node[ni] = Some(meshes.len() - 1);
        } else if let Some(ci) = node.curve {
            let c = &scene.curves[ci];
            let prim =
                json!({"attributes": {"POSITION": buf.vec3s(&c.points, true)}, "material": c.material, "mode": MODE_LINE_STRIP});
            meshes.push(json!({"name": node.name, "primitives": [prim]}));
            mesh_of_node[ni] = Some(meshes.len() - 1);
        }
    }

    let nodes: Vec<Value> = scene
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut v = json!({"name": n.name});
            let children: Vec<usize> = scene.children(i).collect();
            if !children.is_empty() {
                v["children"] = json!(children);
            }
            if let Some(m) = mesh_of_node[i] {
                v["mesh"] = json!(m);
            }
            v
        })
        .collect();

    let mut root = json!({
        "asset": {"version": "2.0", "generator": scene.meta.generator},
        "scene": 0,
        "scenes": [{"name": scene.meta.name, "nodes": scene.roots().collect::<Vec<_>>()}],
    });
    let mut put = |k: &str, v: Vec<Value>| {
        if !v.is_empty() {
            root[k] = Value::Array(v);
        }
    };
    put("nodes", nodes);
    put("meshes", meshes);
    put("materials", materials);
    put("accessors", buf.accessors);
    put("bufferViews", buf.views);
    if !buf.bin.is_empty() {
        while buf.bin.len() % 4 != 0 {
            buf.bin.push(0);
        }
        let mut b = json!({"byteLength": buf.bin.len()});
        if let Some(uri) = bin_uri {
            b["uri"] = json!(uri);
        }
        root["buffers"] = json!([b]);
    }
    extensions.sort();
    extensions.dedup();
    if !extensions.is_empty() {
        root["extensionsUsed"] = json!(extensions);
    }
    (root, buf.bin)
}

fn glb(json: &Value, bin: &[u8]) -> Vec<u8> {
    let mut json_bytes = serde_json::to_vec(json).unwrap();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let total = 12 + 8 + json_bytes.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    if !bin.is_empty() {
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(bin);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exporter::{ExporterRegistry, MemorySink};
    use crate::scene::SceneOptions;
    use crate::scene::tests::{build as build_scene, sample_doc};

    fn u32_at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
    }

    /// Parses a GLB and checks the structural rules importers rely on.
    fn check_glb(bytes: &[u8]) -> Value {
        assert_eq!(&bytes[0..4], b"glTF");
        assert_eq!(u32_at(bytes, 4), 2);
        assert_eq!(u32_at(bytes, 8) as usize, bytes.len());
        let json_len = u32_at(bytes, 12) as usize;
        assert_eq!(&bytes[16..20], b"JSON");
        assert_eq!(json_len % 4, 0);
        let json: Value = serde_json::from_slice(&bytes[20..20 + json_len]).unwrap();
        let bin_start = 20 + json_len;
        let bin = if bin_start < bytes.len() {
            assert_eq!(&bytes[bin_start + 4..bin_start + 8], b"BIN\0");
            let len = u32_at(bytes, bin_start) as usize;
            assert_eq!(len % 4, 0);
            &bytes[bin_start + 8..bin_start + 8 + len]
        } else {
            &[][..]
        };
        if let Some(buffers) = json.get("buffers") {
            assert_eq!(buffers[0]["byteLength"].as_u64().unwrap() as usize, bin.len());
        }
        for acc in json["accessors"].as_array().into_iter().flatten() {
            let view = &json["bufferViews"][acc["bufferView"].as_u64().unwrap() as usize];
            let comps = match acc["type"].as_str().unwrap() {
                "VEC2" => 2,
                "VEC3" => 3,
                "VEC4" => 4,
                _ => 1,
            };
            let need = acc["count"].as_u64().unwrap() * comps * 4;
            assert_eq!(view["byteLength"].as_u64().unwrap(), need);
            assert_eq!(view["byteOffset"].as_u64().unwrap() % 4, 0);
            assert!(view["byteOffset"].as_u64().unwrap() + need <= bin.len() as u64);
        }
        json
    }

    fn export(id: &str, opts: &SceneOptions) -> MemorySink {
        let (doc, lib) = sample_doc();
        let scene = build_scene(&doc, &lib, opts);
        let mut sink = MemorySink::new("sketch");
        ExporterRegistry::with_builtins().export(id, &scene, &Params::new(), &mut sink).unwrap();
        sink
    }

    /// Stroke UVs (and tangents where the format has them) reach every mesh format.
    #[test]
    fn uvs_reach_every_format() {
        let sink = export("com.airdraw.glb", &SceneOptions::default());
        let json = check_glb(&sink.files["sketch.glb"]);
        for mesh in json["meshes"].as_array().unwrap() {
            let attrs = &mesh["primitives"][0]["attributes"];
            let count = |key: &str| json["accessors"][attrs[key].as_u64().unwrap() as usize]["count"].as_u64().unwrap();
            assert_eq!(count("TEXCOORD_0"), count("POSITION"));
            assert_eq!(count("TANGENT"), count("POSITION"));
        }
        let text = |id: &str, file: &str| String::from_utf8_lossy(&export(id, &SceneOptions::default()).files[file]).into_owned();
        let obj = text("com.airdraw.obj", "sketch.obj");
        assert!(obj.lines().any(|l| l.starts_with("vt ")));
        assert!(obj.lines().filter(|l| l.starts_with("f ")).all(|l| l.split_whitespace().skip(1).all(|c| c.split('/').count() == 3 && !c.contains("//"))));
        assert!(text("com.airdraw.ply", "sketch.ply").contains("property float s\nproperty float t\n"));
        assert!(text("com.airdraw.usda", "sketch.usda").contains("texCoord2f[] primvars:st"));
        assert!(text("com.airdraw.fbx", "sketch.fbx").contains("LayerElementUV"));
    }

    #[test]
    fn glb_is_structurally_valid() {
        let sink = export("com.airdraw.glb", &SceneOptions::default());
        let json = check_glb(&sink.files["sketch.glb"]);
        assert_eq!(json["meshes"].as_array().unwrap().len(), 3);
        assert_eq!(json["materials"].as_array().unwrap().len(), 2);
        assert_eq!(json["scenes"][0]["nodes"], json!([0]));
        assert_eq!(json["nodes"][0]["children"], json!([1, 2, 3]));
        // Ribbon material is double-sided; colors are linear.
        assert_eq!(json["materials"][1]["doubleSided"], json!(true));
        let r = json["materials"][1]["pbrMetallicRoughness"]["baseColorFactor"][0].as_f64().unwrap();
        assert!(r < 0.1, "sRGB 0x2F should be ~0.028 linear, got {r}");
    }

    #[test]
    fn curves_are_line_strips() {
        let sink = export("com.airdraw.glb", &SceneOptions { include_curves: true, ..Default::default() });
        let json = check_glb(&sink.files["sketch.glb"]);
        let lines =
            json["meshes"].as_array().unwrap().iter().filter(|m| m["primitives"][0]["mode"] == MODE_LINE_STRIP).count();
        assert_eq!(lines, 3);
    }

    #[test]
    fn gltf_writes_json_and_bin() {
        let sink = export("com.airdraw.gltf", &SceneOptions::default());
        let json: Value = serde_json::from_slice(&sink.files["sketch.gltf"]).unwrap();
        assert_eq!(json["buffers"][0]["uri"], "sketch.bin");
        assert_eq!(json["buffers"][0]["byteLength"].as_u64().unwrap() as usize, sink.files["sketch.bin"].len());
    }

    #[test]
    fn empty_scene_is_valid() {
        let (_, lib) = sample_doc();
        let scene = build_scene(&airdraw_doc::Document::new(), &lib, &SceneOptions::default());
        let mut sink = MemorySink::new("empty");
        ExporterRegistry::with_builtins().export("com.airdraw.glb", &scene, &Params::new(), &mut sink).unwrap();
        let json = check_glb(&sink.files["empty.glb"]);
        assert!(json.get("buffers").is_none());
    }

    #[test]
    fn z_up_warns_for_gltf() {
        let (doc, lib) = sample_doc();
        let scene = build_scene(&doc, &lib, &SceneOptions { up: UpAxis::Z, ..Default::default() });
        let mut sink = MemorySink::new("s");
        let r = ExporterRegistry::with_builtins().export("com.airdraw.glb", &scene, &Params::new(), &mut sink).unwrap();
        assert_eq!(r.warnings.len(), 1);
    }
}
