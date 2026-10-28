//! USD: `.usda` text, and `.usdz` (the .usda in an aligned, uncompressed zip) for AR Quick
//! Look on iPhone/iPad. Meshes use UsdPreviewSurface materials; curves become linear
//! BasisCurves with per-point widths.

use std::collections::HashSet;
use std::fmt::Write as _;

use airdraw_brush::{ParamSpec, Params};
use glam::Vec3;

use super::zip::ZipWriter;
use crate::exporter::{ExportError, ExportReport, ExportSink, Exporter, ExporterCaps, ExporterInfo};
use crate::scene::{ExportScene, UpAxis, srgb_to_linear};

pub struct Usda;
pub struct Usdz;

fn caps() -> ExporterCaps {
    ExporterCaps { meshes: true, curves: true, materials: true, hierarchy: true, ..Default::default() }
}

impl Exporter for Usda {
    fn info(&self) -> ExporterInfo {
        ExporterInfo { id: "com.airdraw.usda".into(), name: "USD (text .usda)".into(), extension: "usda".into(), caps: caps() }
    }
    fn options(&self) -> Vec<ParamSpec> {
        Vec::new()
    }
    fn export(&self, scene: &ExportScene, _: &Params, out: &mut dyn ExportSink) -> Result<ExportReport, ExportError> {
        let name = format!("{}.usda", out.stem());
        out.write(&name, usda(scene).as_bytes())?;
        Ok(ExportReport { files: vec![name], warnings: Vec::new() })
    }
}

impl Exporter for Usdz {
    fn info(&self) -> ExporterInfo {
        ExporterInfo { id: "com.airdraw.usdz".into(), name: "USDZ (AR Quick Look)".into(), extension: "usdz".into(), caps: caps() }
    }
    fn options(&self) -> Vec<ParamSpec> {
        Vec::new()
    }
    fn export(&self, scene: &ExportScene, _: &Params, out: &mut dyn ExportSink) -> Result<ExportReport, ExportError> {
        let mut zip = ZipWriter::new();
        // The first file in a USDZ is its root layer.
        zip.add("scene.usda", usda(scene).as_bytes());
        let name = format!("{}.usdz", out.stem());
        out.write(&name, &zip.finish())?;
        Ok(ExportReport { files: vec![name], warnings: Vec::new() })
    }
}

/// Unique, valid USD prim names ([A-Za-z_][A-Za-z0-9_]*).
#[derive(Default)]
struct Names(HashSet<String>);

impl Names {
    fn make(&mut self, raw: &str) -> String {
        let mut base: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        if base.is_empty() || base.starts_with(|c: char| c.is_ascii_digit()) {
            base.insert(0, '_');
        }
        let mut name = base.clone();
        let mut n = 1;
        while !self.0.insert(name.clone()) {
            name = format!("{base}_{n}");
            n += 1;
        }
        name
    }
}

fn vec3s(v: &[Vec3]) -> String {
    let mut s = String::with_capacity(v.len() * 24);
    s.push('[');
    for (i, p) in v.iter().enumerate() {
        let _ = write!(s, "{}({}, {}, {})", if i > 0 { ", " } else { "" }, p.x, p.y, p.z);
    }
    s.push(']');
    s
}

fn list<T: std::fmt::Display>(v: impl IntoIterator<Item = T>) -> String {
    let items: Vec<String> = v.into_iter().map(|x| x.to_string()).collect();
    format!("[{}]", items.join(", "))
}

pub fn usda(scene: &ExportScene) -> String {
    let mut s = String::new();
    let up = if scene.meta.up == UpAxis::Z { "Z" } else { "Y" };
    let _ = writeln!(
        s,
        "#usda 1.0\n(\n    defaultPrim = \"Root\"\n    metersPerUnit = 1\n    upAxis = \"{up}\"\n    doc = \"{}\"\n)\n",
        scene.meta.generator
    );
    let _ = writeln!(s, "def Xform \"Root\"\n{{");

    // Materials.
    let mut names = Names::default();
    let mat_names: Vec<String> = scene.materials.iter().map(|m| names.make(&m.name)).collect();
    let _ = writeln!(s, "    def Scope \"Materials\"\n    {{");
    for (m, name) in scene.materials.iter().zip(&mat_names) {
        let [r, g, b, a] = m.base_color;
        let lin = [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)];
        // No unlit shader in UsdPreviewSurface: emit the color instead of reflecting light.
        let (diffuse, emissive) = if m.unlit { ([0.0; 3], lin) } else { (lin, lin.map(|c| c * m.emission)) };
        let _ = writeln!(s, "        def Material \"{name}\"\n        {{");
        let _ = writeln!(s, "            token outputs:surface.connect = </Root/Materials/{name}/Surface.outputs:surface>");
        let _ = writeln!(s, "            def Shader \"Surface\"\n            {{");
        let _ = writeln!(s, "                uniform token info:id = \"UsdPreviewSurface\"");
        let _ = writeln!(s, "                color3f inputs:diffuseColor = ({}, {}, {})", diffuse[0], diffuse[1], diffuse[2]);
        let _ = writeln!(s, "                color3f inputs:emissiveColor = ({}, {}, {})", emissive[0], emissive[1], emissive[2]);
        let _ = writeln!(s, "                float inputs:roughness = {}", m.roughness);
        let _ = writeln!(s, "                float inputs:metallic = {}", m.metallic);
        let _ = writeln!(s, "                float inputs:opacity = {a}");
        let _ = writeln!(s, "                token outputs:surface\n            }}\n        }}");
    }
    let _ = writeln!(s, "    }}");

    // Node tree.
    fn node(s: &mut String, scene: &ExportScene, i: usize, depth: usize, names: &mut Names, mats: &[String]) {
        let pad = "    ".repeat(depth);
        let n = &scene.nodes[i];
        let name = names.make(&n.name);
        if let Some(mi) = n.mesh {
            let m = &scene.meshes[mi];
            let mat = &scene.materials[m.material];
            let (min, max) = m.positions.iter().fold((Vec3::MAX, Vec3::MIN), |(a, b), p| (a.min(*p), b.max(*p)));
            let _ = writeln!(s, "{pad}def Mesh \"{name}\" (\n{pad}    prepend apiSchemas = [\"MaterialBindingAPI\"]\n{pad})\n{pad}{{");
            let _ = writeln!(s, "{pad}    uniform bool doubleSided = {}", mat.double_sided as u8);
            let _ = writeln!(s, "{pad}    float3[] extent = {}", vec3s(&[min, max]));
            let _ = writeln!(s, "{pad}    int[] faceVertexCounts = {}", list(std::iter::repeat_n(3, m.indices.len() / 3)));
            let _ = writeln!(s, "{pad}    int[] faceVertexIndices = {}", list(&m.indices));
            let _ = writeln!(s, "{pad}    point3f[] points = {}", vec3s(&m.positions));
            if m.normals.len() == m.positions.len() {
                let _ = writeln!(
                    s,
                    "{pad}    normal3f[] normals = {} (\n{pad}        interpolation = \"vertex\"\n{pad}    )",
                    vec3s(&m.normals)
                );
            }
            if m.uvs.len() == m.positions.len() {
                let st: Vec<String> = m.uvs.iter().map(|u| format!("({}, {})", u.x, u.y)).collect();
                let _ = writeln!(
                    s,
                    "{pad}    texCoord2f[] primvars:st = [{}] (\n{pad}        interpolation = \"vertex\"\n{pad}    )",
                    st.join(", ")
                );
            }
            let _ = writeln!(s, "{pad}    uniform token subdivisionScheme = \"none\"");
            let _ = writeln!(s, "{pad}    rel material:binding = </Root/Materials/{}>", mats[m.material]);
        } else if let Some(ci) = n.curve {
            let c = &scene.curves[ci];
            let [r, g, b, _] = scene.materials[c.material].base_color.map(srgb_to_linear);
            let _ = writeln!(s, "{pad}def BasisCurves \"{name}\"\n{pad}{{");
            let _ = writeln!(s, "{pad}    uniform token type = \"linear\"");
            let _ = writeln!(s, "{pad}    int[] curveVertexCounts = [{}]", c.points.len());
            let _ = writeln!(s, "{pad}    point3f[] points = {}", vec3s(&c.points));
            let _ = writeln!(s, "{pad}    float[] widths = {} (\n{pad}        interpolation = \"vertex\"\n{pad}    )", list(&c.widths));
            let _ = writeln!(s, "{pad}    color3f[] primvars:displayColor = [({r}, {g}, {b})]");
        } else {
            let _ = writeln!(s, "{pad}def Xform \"{name}\"\n{pad}{{");
        }
        for child in scene.children(i) {
            node(s, scene, child, depth + 1, names, mats);
        }
        let _ = writeln!(s, "{pad}}}");
    }
    for root in scene.roots() {
        node(&mut s, scene, root, 1, &mut names, &mat_names);
    }
    let _ = writeln!(s, "}}");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exporter::{ExporterRegistry, MemorySink};
    use crate::scene::SceneOptions;
    use crate::scene::tests::{build, sample_doc};

    fn scene(curves: bool) -> ExportScene {
        let (doc, lib) = sample_doc();
        build(&doc, &lib, &SceneOptions { include_curves: curves, ..Default::default() })
    }

    #[test]
    fn usda_structure() {
        let text = usda(&scene(true));
        assert!(text.starts_with("#usda 1.0"));
        assert_eq!(text.matches("def Mesh ").count(), 3);
        assert_eq!(text.matches("def BasisCurves ").count(), 3);
        assert_eq!(text.matches("def Material ").count(), 2);
        assert_eq!(text.matches('{').count(), text.matches('}').count(), "balanced braces");
        // Every binding points at a defined material.
        for line in text.lines().filter(|l| l.contains("material:binding")) {
            let target = line.split("</Root/Materials/").nth(1).unwrap().trim_end_matches('>');
            assert!(text.contains(&format!("def Material \"{target}\"")), "{target}");
        }
    }

    #[test]
    fn prim_names_are_valid_and_unique() {
        let mut n = Names::default();
        assert_eq!(n.make("Layer 1"), "Layer_1");
        assert_eq!(n.make("Layer 1"), "Layer_1_1");
        assert_eq!(n.make("3d"), "_3d");
        assert_eq!(n.make(""), "_");
    }

    #[test]
    fn usdz_is_an_aligned_zip_with_the_usda_first() {
        let mut sink = MemorySink::new("s");
        ExporterRegistry::with_builtins().export("com.airdraw.usdz", &scene(false), &Params::new(), &mut sink).unwrap();
        let z = &sink.files["s.usdz"];
        assert_eq!(&z[..4], &[0x50, 0x4b, 3, 4]);
        let name_len = u16::from_le_bytes([z[26], z[27]]) as usize;
        assert_eq!(&z[30..30 + name_len], b"scene.usda");
        let extra = u16::from_le_bytes([z[28], z[29]]) as usize;
        let data = 30 + name_len + extra;
        assert_eq!(data % 64, 0);
        assert!(z[data..].starts_with(b"#usda 1.0"));
    }
}
