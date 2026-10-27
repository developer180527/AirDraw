//! PLY (Stanford): all meshes merged, with per-vertex normals and colors from the stroke
//! materials. Binary little-endian (default) or ASCII.

use std::fmt::Write as _;

use airdraw_brush::{ParamSpec, Params, params::ParamsExt};

use crate::exporter::{ExportError, ExportReport, ExportSink, Exporter, ExporterCaps, ExporterInfo};
use crate::scene::ExportScene;

pub struct Ply;

impl Exporter for Ply {
    fn info(&self) -> ExporterInfo {
        ExporterInfo {
            id: "com.airdraw.ply".into(),
            name: "PLY (colored mesh)".into(),
            extension: "ply".into(),
            caps: ExporterCaps { meshes: true, materials: true, ..Default::default() },
        }
    }

    fn options(&self) -> Vec<ParamSpec> {
        vec![ParamSpec::bool("ascii", false, "Text PLY instead of binary (larger files)")]
    }

    fn export(&self, scene: &ExportScene, opts: &Params, out: &mut dyn ExportSink) -> Result<ExportReport, ExportError> {
        let vertex_count: usize = scene.meshes.iter().map(|m| m.positions.len()).sum();
        let face_count: usize = scene.meshes.iter().map(|m| m.indices.len() / 3).sum();
        let ascii = opts.flag("ascii");
        // Texture coordinates only when every mesh has them (one vertex layout per file).
        let with_uv = scene.meshes.iter().all(|m| m.uvs.len() == m.positions.len());
        let mut header = String::from("ply\n");
        header += if ascii { "format ascii 1.0\n" } else { "format binary_little_endian 1.0\n" };
        let _ = writeln!(header, "comment {}", scene.meta.generator);
        let _ = writeln!(header, "element vertex {vertex_count}");
        header += "property float x\nproperty float y\nproperty float z\n";
        header += "property float nx\nproperty float ny\nproperty float nz\n";
        header += "property uchar red\nproperty uchar green\nproperty uchar blue\nproperty uchar alpha\n";
        if with_uv {
            header += "property float s\nproperty float t\n";
        }
        let _ = writeln!(header, "element face {face_count}");
        header += "property list uchar uint vertex_indices\nend_header\n";

        let mut b = header.into_bytes();
        let byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        for m in &scene.meshes {
            let rgba = scene.materials[m.material].base_color.map(byte);
            for (i, p) in m.positions.iter().enumerate() {
                let n = m.normals.get(i).copied().unwrap_or_default();
                let uv = m.uvs.get(i).copied().unwrap_or_default();
                if ascii {
                    let mut line = format!("{} {} {} {} {} {} {} {} {} {}", p.x, p.y, p.z, n.x, n.y, n.z, rgba[0], rgba[1], rgba[2], rgba[3]);
                    if with_uv {
                        line += &format!(" {} {}", uv.x, uv.y);
                    }
                    line.push('\n');
                    b.extend_from_slice(line.as_bytes());
                } else {
                    for c in [p.x, p.y, p.z, n.x, n.y, n.z] {
                        b.extend_from_slice(&c.to_le_bytes());
                    }
                    b.extend_from_slice(&rgba);
                    if with_uv {
                        b.extend_from_slice(&uv.x.to_le_bytes());
                        b.extend_from_slice(&uv.y.to_le_bytes());
                    }
                }
            }
        }
        let mut base = 0u32;
        for m in &scene.meshes {
            for t in m.indices.as_chunks::<3>().0 {
                let [x, y, z] = [t[0] + base, t[1] + base, t[2] + base];
                if ascii {
                    b.extend_from_slice(format!("3 {x} {y} {z}\n").as_bytes());
                } else {
                    b.push(3);
                    for i in [x, y, z] {
                        b.extend_from_slice(&i.to_le_bytes());
                    }
                }
            }
            base += m.positions.len() as u32;
        }
        let name = format!("{}.ply", out.stem());
        out.write(&name, &b)?;
        Ok(ExportReport { files: vec![name], warnings: Vec::new() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exporter::{ExporterRegistry, MemorySink};
    use crate::scene::SceneOptions;
    use crate::scene::tests::{build, sample_doc};
    use airdraw_brush::ParamValue;

    fn export(ascii: bool) -> (ExportScene, Vec<u8>) {
        let (doc, lib) = sample_doc();
        let scene = build(&doc, &lib, &SceneOptions::default());
        let mut sink = MemorySink::new("s");
        let opts: Params = [("ascii".to_string(), ParamValue::Bool(ascii))].into();
        ExporterRegistry::with_builtins().export("com.airdraw.ply", &scene, &opts, &mut sink).unwrap();
        (scene, sink.files.remove("s.ply").unwrap())
    }

    #[test]
    fn binary_sizes_match_header() {
        let (scene, b) = export(false);
        let end = b.windows(11).position(|w| w == b"end_header\n").unwrap() + 11;
        let verts: usize = scene.meshes.iter().map(|m| m.positions.len()).sum();
        let faces: usize = scene.meshes.iter().map(|m| m.indices.len() / 3).sum();
        let header = std::str::from_utf8(&b[..end]).unwrap();
        assert!(header.contains(&format!("element vertex {verts}\n")));
        assert!(header.contains(&format!("element face {faces}\n")));
        // Per vertex: position + normal (6 floats), RGBA, s + t (2 floats).
        assert_eq!(b.len() - end, verts * (24 + 4 + 8) + faces * 13);
    }

    #[test]
    fn ascii_has_colors_and_global_indices() {
        let (scene, b) = export(true);
        let text = String::from_utf8(b).unwrap();
        let body: Vec<&str> = text.split("end_header\n").nth(1).unwrap().lines().collect();
        let verts: usize = scene.meshes.iter().map(|m| m.positions.len()).sum();
        // Ink color #1E1E24 on the first vertex.
        assert!(body[0].contains(" 30 30 36 255 "), "{}", body[0]);
        assert_eq!(body[0].split(' ').count(), 12, "x y z nx ny nz r g b a s t");
        let max_index = body[verts..].iter().flat_map(|l| l.split(' ').skip(1).map(|v| v.parse::<usize>().unwrap())).max().unwrap();
        assert_eq!(max_index, verts - 1);
    }
}
