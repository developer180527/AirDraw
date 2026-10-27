//! Binary FBX 7.4 writer (no Autodesk SDK). Layers become Null models, strokes become Mesh
//! models with geometry (per-polygon-vertex normals) and Phong materials. Units are meters
//! (UnitScaleFactor 100, since FBX counts in centimeters). Curves aren't written in v1.
//!
//! The file id, creation time and footer bytes are the fixed values Blender's exporter
//! uses, which readers that validate the footer accept.

use airdraw_brush::{ParamSpec, Params};

use crate::exporter::{ExportError, ExportReport, ExportSink, Exporter, ExporterCaps, ExporterInfo};
use crate::scene::{ExportScene, UpAxis, srgb_to_linear};

pub struct Fbx;

const VERSION: u32 = 7400;
const FILE_ID: [u8; 16] = [0x28, 0xb3, 0x2a, 0xeb, 0xb6, 0x24, 0xcc, 0xc2, 0xbf, 0xc8, 0xb0, 0x2a, 0xa9, 0x2b, 0xfc, 0xf1];
const CREATION_TIME: &str = "1970-01-01 10:00:00:000";
const FOOT_ID: [u8; 16] = [0xfa, 0xbc, 0xab, 0x09, 0xd0, 0xc8, 0xd4, 0x66, 0xb1, 0x76, 0xfb, 0x83, 0x1c, 0xf7, 0x26, 0x7e];
const FOOT_MAGIC: [u8; 16] = [0xf8, 0x5a, 0x8c, 0x6a, 0xde, 0xf5, 0xd9, 0x7e, 0xec, 0xe9, 0x0c, 0xe3, 0x75, 0x8f, 0x29, 0x0b];
/// A node record with all fields zero ends a list of child nodes (FBX < 7.5).
const NULL_RECORD: [u8; 13] = [0; 13];

impl Exporter for Fbx {
    fn info(&self) -> ExporterInfo {
        ExporterInfo {
            id: "com.airdraw.fbx".into(),
            name: "FBX (binary 7.4)".into(),
            extension: "fbx".into(),
            caps: ExporterCaps { meshes: true, materials: true, hierarchy: true, ..Default::default() },
        }
    }

    fn options(&self) -> Vec<ParamSpec> {
        Vec::new()
    }

    fn export(&self, scene: &ExportScene, _: &Params, out: &mut dyn ExportSink) -> Result<ExportReport, ExportError> {
        let name = format!("{}.fbx", out.stem());
        out.write(&name, &write_fbx(scene))?;
        Ok(ExportReport { files: vec![name], warnings: Vec::new() })
    }
}

// ---- Generic binary node encoding ------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Prop {
    Bool(bool),
    I32(i32),
    I64(i64),
    F64(f64),
    Str(Vec<u8>),
    Raw(Vec<u8>),
    ArrF64(Vec<f64>),
    ArrI32(Vec<i32>),
}

impl From<&str> for Prop {
    fn from(s: &str) -> Self {
        Prop::Str(s.as_bytes().to_vec())
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub name: &'static str,
    pub props: Vec<Prop>,
    pub children: Vec<Node>,
}

fn n(name: &'static str) -> Node {
    Node { name, props: Vec::new(), children: Vec::new() }
}

impl Node {
    fn p(mut self, p: impl Into<Prop>) -> Self {
        self.props.push(p.into());
        self
    }
    fn c(mut self, child: Node) -> Self {
        self.children.push(child);
        self
    }
    /// `name: value` leaf, the common FBX pattern.
    fn leaf(name: &'static str, p: impl Into<Prop>) -> Self {
        n(name).p(p)
    }
}

impl From<i32> for Prop {
    fn from(v: i32) -> Self {
        Prop::I32(v)
    }
}
impl From<i64> for Prop {
    fn from(v: i64) -> Self {
        Prop::I64(v)
    }
}
impl From<f64> for Prop {
    fn from(v: f64) -> Self {
        Prop::F64(v)
    }
}
impl From<bool> for Prop {
    fn from(v: bool) -> Self {
        Prop::Bool(v)
    }
}

fn write_prop(o: &mut Vec<u8>, p: &Prop) {
    fn array<T: Copy>(o: &mut Vec<u8>, code: u8, v: &[T], bytes: impl Fn(T) -> Vec<u8>, size: usize) {
        o.push(code);
        o.extend_from_slice(&(v.len() as u32).to_le_bytes());
        o.extend_from_slice(&0u32.to_le_bytes()); // uncompressed
        o.extend_from_slice(&((v.len() * size) as u32).to_le_bytes());
        for &x in v {
            o.extend_from_slice(&bytes(x));
        }
    }
    match p {
        Prop::Bool(b) => o.extend_from_slice(&[b'C', *b as u8]),
        Prop::I32(v) => {
            o.push(b'I');
            o.extend_from_slice(&v.to_le_bytes());
        }
        Prop::I64(v) => {
            o.push(b'L');
            o.extend_from_slice(&v.to_le_bytes());
        }
        Prop::F64(v) => {
            o.push(b'D');
            o.extend_from_slice(&v.to_le_bytes());
        }
        Prop::Str(s) | Prop::Raw(s) => {
            o.push(if matches!(p, Prop::Str(_)) { b'S' } else { b'R' });
            o.extend_from_slice(&(s.len() as u32).to_le_bytes());
            o.extend_from_slice(s);
        }
        Prop::ArrF64(v) => array(o, b'd', v, |x: f64| x.to_le_bytes().to_vec(), 8),
        Prop::ArrI32(v) => array(o, b'i', v, |x: i32| x.to_le_bytes().to_vec(), 4),
    }
}

/// Node record (7.4: 32-bit offsets). `EndOffset` is absolute, so `o` must be the whole file.
fn write_node(o: &mut Vec<u8>, node: &Node) {
    let start = o.len();
    o.extend_from_slice(&[0; 12]); // end offset, property count, property bytes
    o.push(node.name.len() as u8);
    o.extend_from_slice(node.name.as_bytes());
    let props_start = o.len();
    for p in &node.props {
        write_prop(o, p);
    }
    let props_len = (o.len() - props_start) as u32;
    for child in &node.children {
        write_node(o, child);
    }
    // Same rule as Blender's writer: terminate children, and mark property-less nodes.
    if !node.children.is_empty() || node.props.is_empty() {
        o.extend_from_slice(&NULL_RECORD);
    }
    let end = o.len() as u32;
    o[start..start + 4].copy_from_slice(&end.to_le_bytes());
    o[start + 4..start + 8].copy_from_slice(&(node.props.len() as u32).to_le_bytes());
    o[start + 8..start + 12].copy_from_slice(&props_len.to_le_bytes());
}

pub fn encode(nodes: &[Node]) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(b"Kaydara FBX Binary  \0\x1a\0");
    o.extend_from_slice(&VERSION.to_le_bytes());
    for node in nodes {
        write_node(&mut o, node);
    }
    o.extend_from_slice(&NULL_RECORD);
    o.extend_from_slice(&FOOT_ID);
    o.extend_from_slice(&[0; 4]);
    let pad = match o.len() % 16 {
        0 => 16,
        r => 16 - r,
    };
    o.resize(o.len() + pad, 0);
    o.extend_from_slice(&VERSION.to_le_bytes());
    o.resize(o.len() + 120, 0);
    o.extend_from_slice(&FOOT_MAGIC);
    o
}

// ---- Scene → FBX document ---------------------------------------------------------------

/// `P: "name", "type", "subtype", "flags", values...` inside Properties70.
fn prop70(name: &str, ty: &str, sub: &str, flags: &str, values: Vec<Prop>) -> Node {
    let mut p = n("P").p(name).p(ty).p(sub).p(flags);
    p.props.extend(values);
    p
}

fn int70(name: &str, v: i32) -> Node {
    prop70(name, "int", "Integer", "", vec![v.into()])
}

fn double70(name: &str, v: f64) -> Node {
    prop70(name, "double", "Number", "", vec![v.into()])
}

/// Binary FBX object names are "Name\x00\x01Class".
fn object_name(name: &str, class: &str) -> Prop {
    let mut s = name.replace('\0', "").into_bytes();
    s.extend_from_slice(b"\x00\x01");
    s.extend_from_slice(class.as_bytes());
    Prop::Str(s)
}

pub fn write_fbx(scene: &ExportScene) -> Vec<u8> {
    let mut next_id = 1_000_000i64;
    let mut id = || {
        next_id += 1;
        next_id
    };
    let mut objects = n("Objects");
    let mut connections = n("Connections");
    let connect = |child: i64, parent: i64| n("C").p("OO").p(child).p(parent);

    let material_ids: Vec<i64> = scene.materials.iter().map(|_| id()).collect();
    for (m, &mid) in scene.materials.iter().zip(&material_ids) {
        let [r, g, b, a] = m.base_color;
        let lin = [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)].map(|c| c as f64);
        let color = |name: &str, c: [f64; 3]| prop70(name, "Color", "", "A", c.iter().map(|&v| Prop::F64(v)).collect());
        let emission = if m.unlit { 1.0 } else { m.emission as f64 };
        objects.children.push(
            n("Material")
                .p(mid)
                .p(object_name(&m.name, "Material"))
                .p("")
                .c(Node::leaf("Version", 102))
                .c(Node::leaf("ShadingModel", "phong"))
                .c(Node::leaf("MultiLayer", 0))
                .c(n("Properties70")
                    .c(color("DiffuseColor", lin))
                    .c(prop70("DiffuseFactor", "Number", "", "A", vec![1.0.into()]))
                    .c(color("EmissiveColor", lin))
                    .c(prop70("EmissiveFactor", "Number", "", "A", vec![emission.into()]))
                    .c(prop70("Opacity", "Number", "", "A", vec![(a as f64).into()]))
                    .c(prop70("SpecularFactor", "Number", "", "A", vec![(1.0 - m.roughness as f64).into()]))
                    .c(prop70("Shininess", "Number", "", "A", vec![((1.0 - m.roughness as f64).powi(2) * 100.0).into()]))),
        );
    }

    let mut model_ids = vec![0i64; scene.nodes.len()];
    let mut counts = (0, 0); // models, geometries
    for (i, node) in scene.nodes.iter().enumerate() {
        if node.curve.is_some() && node.mesh.is_none() {
            continue; // Curves aren't written in v1.
        }
        let model = id();
        model_ids[i] = model;
        counts.0 += 1;
        let kind = if node.mesh.is_some() { "Mesh" } else { "Null" };
        objects.children.push(
            n("Model")
                .p(model)
                .p(object_name(&node.name, "Model"))
                .p(kind)
                .c(Node::leaf("Version", 232))
                .c(n("Properties70").c(int70("DefaultAttributeIndex", 0)))
                .c(Node::leaf("Shading", true))
                .c(Node::leaf("Culling", "CullingOff")),
        );
        let parent = node.parent.map_or(0, |p| model_ids[p]);
        connections.children.push(connect(model, parent));

        if let Some(mi) = node.mesh {
            let m = &scene.meshes[mi];
            let geom = id();
            counts.1 += 1;
            let vertices: Vec<f64> = m.positions.iter().flat_map(|p| [p.x as f64, p.y as f64, p.z as f64]).collect();
            // The last index of each polygon is stored as -(index + 1).
            let polygons: Vec<i32> = m
                .indices
                .as_chunks::<3>().0.iter()
                .flat_map(|t| [t[0] as i32, t[1] as i32, -(t[2] as i32) - 1])
                .collect();
            let normals: Vec<f64> = m
                .indices
                .iter()
                .flat_map(|&i| {
                    let nrm = m.normals.get(i as usize).copied().unwrap_or_default();
                    [nrm.x as f64, nrm.y as f64, nrm.z as f64]
                })
                .collect();
            let with_uv = m.uvs.len() == m.positions.len();
            let mut geometry = n("Geometry")
                .p(geom)
                .p(object_name(&m.name, "Geometry"))
                .p("Mesh")
                .c(Node::leaf("Vertices", Prop::ArrF64(vertices)))
                .c(Node::leaf("PolygonVertexIndex", Prop::ArrI32(polygons)))
                .c(Node::leaf("GeometryVersion", 124))
                .c(n("LayerElementNormal")
                    .p(0)
                    .c(Node::leaf("Version", 101))
                    .c(Node::leaf("Name", ""))
                    .c(Node::leaf("MappingInformationType", "ByPolygonVertex"))
                    .c(Node::leaf("ReferenceInformationType", "Direct"))
                    .c(Node::leaf("Normals", Prop::ArrF64(normals))));
            if with_uv {
                // One UV per vertex, looked up per polygon corner.
                let uv: Vec<f64> = m.uvs.iter().flat_map(|u| [u.x as f64, u.y as f64]).collect();
                let uv_index: Vec<i32> = m.indices.iter().map(|&i| i as i32).collect();
                geometry = geometry.c(n("LayerElementUV")
                    .p(0)
                    .c(Node::leaf("Version", 101))
                    .c(Node::leaf("Name", "UVMap"))
                    .c(Node::leaf("MappingInformationType", "ByPolygonVertex"))
                    .c(Node::leaf("ReferenceInformationType", "IndexToDirect"))
                    .c(Node::leaf("UV", Prop::ArrF64(uv)))
                    .c(Node::leaf("UVIndex", Prop::ArrI32(uv_index))));
            }
            let mut layer = n("Layer")
                .p(0)
                .c(Node::leaf("Version", 100))
                .c(n("LayerElement").c(Node::leaf("Type", "LayerElementNormal")).c(Node::leaf("TypedIndex", 0)))
                .c(n("LayerElement").c(Node::leaf("Type", "LayerElementMaterial")).c(Node::leaf("TypedIndex", 0)));
            if with_uv {
                layer = layer.c(n("LayerElement").c(Node::leaf("Type", "LayerElementUV")).c(Node::leaf("TypedIndex", 0)));
            }
            geometry = geometry
                .c(n("LayerElementMaterial")
                    .p(0)
                    .c(Node::leaf("Version", 101))
                    .c(Node::leaf("Name", ""))
                    .c(Node::leaf("MappingInformationType", "AllSame"))
                    .c(Node::leaf("ReferenceInformationType", "IndexToDirect"))
                    .c(Node::leaf("Materials", Prop::ArrI32(vec![0]))))
                .c(layer);
            objects.children.push(geometry);
            connections.children.push(connect(geom, model));
            connections.children.push(connect(material_ids[m.material], model));
        }
    }

    let (up, front, front_sign) = if scene.meta.up == UpAxis::Z { (2, 1, -1) } else { (1, 2, 1) };
    let ts = n("CreationTimeStamp")
        .c(Node::leaf("Version", 1000))
        .c(Node::leaf("Year", 1970))
        .c(Node::leaf("Month", 1))
        .c(Node::leaf("Day", 1))
        .c(Node::leaf("Hour", 10))
        .c(Node::leaf("Minute", 0))
        .c(Node::leaf("Second", 0))
        .c(Node::leaf("Millisecond", 0));
    let definitions = n("Definitions")
        .c(Node::leaf("Version", 100))
        .c(Node::leaf("Count", 1 + counts.0 + counts.1 + scene.materials.len() as i32))
        .c(n("ObjectType").p("GlobalSettings").c(Node::leaf("Count", 1)))
        .c(n("ObjectType").p("Model").c(Node::leaf("Count", counts.0)))
        .c(n("ObjectType").p("Geometry").c(Node::leaf("Count", counts.1)))
        .c(n("ObjectType").p("Material").c(Node::leaf("Count", scene.materials.len() as i32)));

    let doc = [
        n("FBXHeaderExtension")
            .c(Node::leaf("FBXHeaderVersion", 1003))
            .c(Node::leaf("FBXVersion", VERSION as i32))
            .c(Node::leaf("EncryptionType", 0))
            .c(ts)
            .c(Node::leaf("Creator", scene.meta.generator.as_str())),
        Node::leaf("FileId", Prop::Raw(FILE_ID.to_vec())),
        Node::leaf("CreationTime", CREATION_TIME),
        Node::leaf("Creator", scene.meta.generator.as_str()),
        n("GlobalSettings").c(Node::leaf("Version", 1000)).c(n("Properties70")
            .c(int70("UpAxis", up))
            .c(int70("UpAxisSign", 1))
            .c(int70("FrontAxis", front))
            .c(int70("FrontAxisSign", front_sign))
            .c(int70("CoordAxis", 0))
            .c(int70("CoordAxisSign", 1))
            .c(int70("OriginalUpAxis", up))
            .c(int70("OriginalUpAxisSign", 1))
            .c(double70("UnitScaleFactor", 100.0))
            .c(double70("OriginalUnitScaleFactor", 100.0))),
        n("Documents").c(Node::leaf("Count", 1)).c(n("Document")
            .p(id())
            .p("Scene")
            .p("Scene")
            .c(n("Properties70"))
            .c(Node::leaf("RootNode", 0i64))),
        n("References"),
        definitions,
        objects,
        connections,
        n("Takes").c(Node::leaf("Current", "")),
    ];
    encode(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exporter::{ExporterRegistry, MemorySink};
    use crate::scene::SceneOptions;
    use crate::scene::tests::{build, sample_doc};

    /// Minimal reader: (name, props, children) per node, following end offsets.
    #[derive(Debug)]
    struct RNode {
        name: String,
        props: Vec<u8>, // property type codes
        children: Vec<RNode>,
    }

    fn read_nodes(b: &[u8], mut p: usize, end: usize) -> (Vec<RNode>, usize) {
        let mut out = Vec::new();
        while p + 13 <= end {
            let end_off = u32::from_le_bytes(b[p..p + 4].try_into().unwrap()) as usize;
            if end_off == 0 {
                return (out, p + 13);
            }
            let count = u32::from_le_bytes(b[p + 4..p + 8].try_into().unwrap()) as usize;
            let plen = u32::from_le_bytes(b[p + 8..p + 12].try_into().unwrap()) as usize;
            let nlen = b[p + 12] as usize;
            let name = String::from_utf8(b[p + 13..p + 13 + nlen].to_vec()).unwrap();
            let pstart = p + 13 + nlen;
            let mut q = pstart;
            let mut codes = Vec::new();
            for _ in 0..count {
                let code = b[q];
                codes.push(code);
                q += 1 + match code {
                    b'C' => 1,
                    b'I' | b'F' => 4,
                    b'D' | b'L' => 8,
                    b'S' | b'R' => 4 + u32::from_le_bytes(b[q + 1..q + 5].try_into().unwrap()) as usize,
                    b'd' | b'i' | b'f' | b'l' => 12 + u32::from_le_bytes(b[q + 9..q + 13].try_into().unwrap()) as usize,
                    c => panic!("unknown property type {}", c as char),
                };
            }
            assert_eq!(q - pstart, plen, "property bytes of {name}");
            let (children, _) = if q < end_off { read_nodes(b, q, end_off) } else { (Vec::new(), q) };
            out.push(RNode { name, props: codes, children });
            p = end_off;
        }
        (out, p)
    }

    fn export() -> (ExportScene, Vec<u8>) {
        let (doc, lib) = sample_doc();
        let scene = build(&doc, &lib, &SceneOptions::default());
        let mut sink = MemorySink::new("s");
        ExporterRegistry::with_builtins().export("com.airdraw.fbx", &scene, &Params::new(), &mut sink).unwrap();
        (scene, sink.files.remove("s.fbx").unwrap())
    }

    #[test]
    fn header_tree_and_footer() {
        let (scene, b) = export();
        assert!(b.starts_with(b"Kaydara FBX Binary  \0\x1a\0"));
        assert_eq!(u32::from_le_bytes(b[23..27].try_into().unwrap()), 7400);
        assert!(b.ends_with(&FOOT_MAGIC));
        let (top, after) = read_nodes(&b, 27, b.len());
        let names: Vec<&str> = top.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            ["FBXHeaderExtension", "FileId", "CreationTime", "Creator", "GlobalSettings", "Documents", "References", "Definitions", "Objects", "Connections", "Takes"]
        );
        assert_eq!(&b[after..after + 16], &FOOT_ID);
        let objects = top.iter().find(|n| n.name == "Objects").unwrap();
        let count = |k: &str| objects.children.iter().filter(|n| n.name == k).count();
        assert_eq!(count("Geometry"), scene.meshes.len());
        assert_eq!(count("Material"), scene.materials.len());
        assert_eq!(count("Model"), scene.meshes.len() + 1, "strokes + layer");
        let geom = objects.children.iter().find(|n| n.name == "Geometry").unwrap();
        assert_eq!(geom.children[0].name, "Vertices");
        assert_eq!(geom.children[0].props, b"d");
        // Model + geometry + material per stroke, plus the layer model.
        let conns = top.iter().find(|n| n.name == "Connections").unwrap();
        assert_eq!(conns.children.len(), scene.meshes.len() * 3 + 1);
    }

    #[test]
    fn polygon_indices_mark_polygon_ends() {
        let (doc, lib) = sample_doc();
        let scene = build(&doc, &lib, &SceneOptions::default());
        let b = write_fbx(&scene);
        let marker = b"PolygonVertexIndex";
        let p = b.windows(marker.len()).position(|w| w == marker).unwrap() + marker.len();
        assert_eq!(b[p], b'i');
        let count = u32::from_le_bytes(b[p + 1..p + 5].try_into().unwrap()) as usize;
        let vals: Vec<i32> = (0..count).map(|k| i32::from_le_bytes(b[p + 13 + k * 4..p + 17 + k * 4].try_into().unwrap())).collect();
        assert_eq!(count, scene.meshes[0].indices.len());
        assert!(vals.chunks(3).all(|t| t[0] >= 0 && t[1] >= 0 && t[2] < 0));
        assert_eq!(-vals[2] - 1, scene.meshes[0].indices[2] as i32);
    }
}
