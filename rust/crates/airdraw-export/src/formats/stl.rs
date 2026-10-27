//! STL for 3D printing: all meshes as one solid, binary (default) or ASCII. STL has no
//! units, colors or materials; slicers assume millimeters, hence the `millimeters` option.

use std::fmt::Write as _;

use airdraw_brush::{ParamSpec, Params, params::ParamsExt};
use glam::Vec3;

use crate::exporter::{ExportError, ExportReport, ExportSink, Exporter, ExporterCaps, ExporterInfo};
use crate::scene::ExportScene;

pub struct Stl;

impl Exporter for Stl {
    fn info(&self) -> ExporterInfo {
        ExporterInfo {
            id: "com.airdraw.stl".into(),
            name: "STL (3D printing)".into(),
            extension: "stl".into(),
            caps: ExporterCaps { meshes: true, ..Default::default() },
        }
    }

    fn options(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec::bool("ascii", false, "Text STL instead of binary (larger files)"),
            ParamSpec::bool("millimeters", true, "Write coordinates in millimeters, as slicers expect"),
        ]
    }

    fn export(&self, scene: &ExportScene, opts: &Params, out: &mut dyn ExportSink) -> Result<ExportReport, ExportError> {
        let unit = if opts.flag("millimeters") { 1000.0 } else { 1.0 };
        let tris: Vec<[Vec3; 3]> = scene
            .meshes
            .iter()
            .flat_map(|m| m.indices.as_chunks::<3>().0.iter().map(move |t| [0, 1, 2].map(|k| m.positions[t[k] as usize] * unit)))
            .collect();
        let normal = |t: &[Vec3; 3]| (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
        let name = format!("{}.stl", out.stem());
        if opts.flag("ascii") {
            let solid = crate::scene::sanitize(&scene.meta.name);
            let mut s = format!("solid {solid}\n");
            for t in &tris {
                let n = normal(t);
                let _ = writeln!(s, "  facet normal {} {} {}\n    outer loop", n.x, n.y, n.z);
                for v in t {
                    let _ = writeln!(s, "      vertex {} {} {}", v.x, v.y, v.z);
                }
                s += "    endloop\n  endfacet\n";
            }
            let _ = writeln!(s, "endsolid {solid}");
            out.write(&name, s.as_bytes())?;
        } else {
            let mut b = Vec::with_capacity(84 + tris.len() * 50);
            // Binary headers must not start with "solid" or readers mistake them for ASCII.
            let mut header = [b' '; 80];
            let text = format!("Binary STL from {}", scene.meta.generator);
            header[..text.len().min(80)].copy_from_slice(&text.as_bytes()[..text.len().min(80)]);
            b.extend_from_slice(&header);
            b.extend_from_slice(&(tris.len() as u32).to_le_bytes());
            for t in &tris {
                for v in std::iter::once(normal(t)).chain(t.iter().copied()) {
                    for c in [v.x, v.y, v.z] {
                        b.extend_from_slice(&c.to_le_bytes());
                    }
                }
                b.extend_from_slice(&0u16.to_le_bytes());
            }
            out.write(&name, &b)?;
        }
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
        ExporterRegistry::with_builtins().export("com.airdraw.stl", &scene, &opts, &mut sink).unwrap();
        (scene, sink.files.remove("s.stl").unwrap())
    }

    #[test]
    fn binary_layout() {
        let (scene, b) = export(false);
        let tris: usize = scene.meshes.iter().map(|m| m.indices.len() / 3).sum();
        assert!(!b.starts_with(b"solid"));
        assert_eq!(u32::from_le_bytes(b[80..84].try_into().unwrap()) as usize, tris);
        assert_eq!(b.len(), 84 + tris * 50);
        // First vertex in millimeters.
        let x = f32::from_le_bytes(b[96..100].try_into().unwrap());
        let m = &scene.meshes[0];
        assert!((x - m.positions[m.indices[0] as usize].x * 1000.0).abs() < 1e-3);
    }

    #[test]
    fn ascii_layout() {
        let (scene, b) = export(true);
        let text = String::from_utf8(b).unwrap();
        let tris: usize = scene.meshes.iter().map(|m| m.indices.len() / 3).sum();
        assert!(text.starts_with("solid test"));
        assert_eq!(text.matches("facet normal").count(), tris);
        assert_eq!(text.matches("vertex ").count(), tris * 3);
        assert!(text.trim_end().ends_with("endsolid test"));
    }
}
