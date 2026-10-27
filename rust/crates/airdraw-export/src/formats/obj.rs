//! Wavefront OBJ + MTL. Universally readable; curves are written as polylines (`l`).

use std::fmt::Write as _;

use airdraw_brush::{ParamSpec, Params, params::ParamsExt};

use crate::exporter::{ExportError, ExportReport, ExportSink, Exporter, ExporterCaps, ExporterInfo};
use crate::scene::{ExportScene, sanitize};

pub struct Obj;

impl Exporter for Obj {
    fn info(&self) -> ExporterInfo {
        ExporterInfo {
            id: "com.airdraw.obj".into(),
            name: "OBJ".into(),
            extension: "obj".into(),
            caps: ExporterCaps { meshes: true, curves: true, materials: true, multi_file: true, ..Default::default() },
        }
    }

    fn options(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec::bool("materials", true, "Write a .mtl file with stroke colors"),
            ParamSpec::int("precision", 6, 2, 9, "Decimal places for coordinates"),
        ]
    }

    fn export(&self, scene: &ExportScene, opts: &Params, out: &mut dyn ExportSink) -> Result<ExportReport, ExportError> {
        let stem = out.stem().to_string();
        let with_mtl = opts.flag("materials");
        let prec = opts.int("precision") as usize;
        let mut obj = String::new();
        let _ = writeln!(obj, "# {}\n# Units: meters, {:?} up", scene.meta.generator, scene.meta.up);
        if with_mtl {
            let _ = writeln!(obj, "mtllib {stem}.mtl");
        }
        // OBJ indices are 1-based and global across the file.
        let (mut v_base, mut vn_base) = (1u32, 1u32);
        for node in &scene.nodes {
            if let Some(mi) = node.mesh {
                let m = &scene.meshes[mi];
                let _ = writeln!(obj, "o {}", sanitize(&node.name));
                if with_mtl {
                    let _ = writeln!(obj, "usemtl {}", scene.materials[m.material].name);
                }
                for p in &m.positions {
                    let _ = writeln!(obj, "v {:.prec$} {:.prec$} {:.prec$}", p.x, p.y, p.z);
                }
                for n in &m.normals {
                    let _ = writeln!(obj, "vn {:.4} {:.4} {:.4}", n.x, n.y, n.z);
                }
                for tri in m.indices.chunks_exact(3) {
                    let [a, b, c] = [tri[0] + v_base, tri[1] + v_base, tri[2] + v_base];
                    let [na, nb, nc] = [tri[0] + vn_base, tri[1] + vn_base, tri[2] + vn_base];
                    let _ = writeln!(obj, "f {a}//{na} {b}//{nb} {c}//{nc}");
                }
                v_base += m.positions.len() as u32;
                vn_base += m.normals.len() as u32;
            }
            if let Some(ci) = node.curve {
                let c = &scene.curves[ci];
                let _ = writeln!(obj, "o {}", sanitize(&node.name));
                for p in &c.points {
                    let _ = writeln!(obj, "v {:.prec$} {:.prec$} {:.prec$}", p.x, p.y, p.z);
                }
                let idx: Vec<String> = (0..c.points.len() as u32).map(|i| (i + v_base).to_string()).collect();
                let _ = writeln!(obj, "l {}", idx.join(" "));
                v_base += c.points.len() as u32;
            }
        }

        let mut files = vec![format!("{stem}.obj")];
        out.write(&files[0], obj.as_bytes())?;
        if with_mtl {
            let mut mtl = format!("# {}\n", scene.meta.generator);
            for m in &scene.materials {
                let [r, g, b, a] = m.base_color;
                let _ = writeln!(mtl, "\nnewmtl {}", m.name);
                let _ = writeln!(mtl, "Kd {r:.4} {g:.4} {b:.4}");
                let _ = writeln!(mtl, "d {a:.4}");
                // Phong exponent from roughness, plus the PBR extension most tools read.
                let _ = writeln!(mtl, "Ns {:.1}", (1.0 - m.roughness).powi(2) * 1000.0);
                let _ = writeln!(mtl, "Pr {:.4}\nPm {:.4}", m.roughness, m.metallic);
                if m.emission > 0.0 {
                    let e = m.emission.min(1.0);
                    let _ = writeln!(mtl, "Ke {:.4} {:.4} {:.4}", r * e, g * e, b * e);
                }
                let _ = writeln!(mtl, "illum {}", if m.unlit { 0 } else { 2 });
            }
            files.push(format!("{stem}.mtl"));
            out.write(&files[1], mtl.as_bytes())?;
        }
        Ok(ExportReport { files, warnings: Vec::new() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exporter::{ExporterRegistry, MemorySink};
    use crate::scene::SceneOptions;
    use crate::scene::tests::{build, sample_doc};

    fn export(opts: &SceneOptions, params: Params) -> MemorySink {
        let (doc, lib) = sample_doc();
        let scene = build(&doc, &lib, opts);
        let mut sink = MemorySink::new("sketch");
        ExporterRegistry::with_builtins().export("com.airdraw.obj", &scene, &params, &mut sink).unwrap();
        sink
    }

    #[test]
    fn writes_obj_and_mtl_with_valid_indices() {
        let sink = export(&SceneOptions::default(), Params::new());
        let obj = String::from_utf8(sink.files["sketch.obj"].clone()).unwrap();
        let mtl = String::from_utf8(sink.files["sketch.mtl"].clone()).unwrap();
        let verts = obj.lines().filter(|l| l.starts_with("v ")).count() as u32;
        let max_index = obj
            .lines()
            .filter(|l| l.starts_with("f "))
            .flat_map(|l| l[2..].split(' ').map(|c| c.split("//").next().unwrap().parse::<u32>().unwrap()))
            .max()
            .unwrap();
        assert_eq!(max_index, verts);
        assert_eq!(obj.lines().filter(|l| l.starts_with("o ")).count(), 3);
        assert!(obj.contains("mtllib sketch.mtl"));
        assert_eq!(mtl.matches("newmtl ").count(), 2);
        for name in obj.lines().filter_map(|l| l.strip_prefix("usemtl ")) {
            assert!(mtl.contains(&format!("newmtl {name}")));
        }
    }

    #[test]
    fn curves_become_polylines_and_materials_are_optional() {
        let sink = export(
            &SceneOptions { include_curves: true, ..Default::default() },
            [("materials".to_string(), airdraw_brush::ParamValue::Bool(false))].into(),
        );
        let obj = String::from_utf8(sink.files["sketch.obj"].clone()).unwrap();
        assert_eq!(obj.lines().filter(|l| l.starts_with("l ")).count(), 3);
        assert!(!sink.files.contains_key("sketch.mtl"));
        assert!(!obj.contains("usemtl"));
    }
}
