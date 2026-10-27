//! `ExportScene`: the neutral, read-only snapshot every exporter receives. Built from a
//! document by re-running each stroke's brush on its raw input, then applying the common
//! scene options (scale, up axis, merging, curves) once so exporters don't have to.

use std::collections::HashMap;
use std::sync::Arc;

use airdraw_brush::{Brush, MaterialDesc, ParamSpec, Params, Shading, params::ParamsExt};
use airdraw_doc::{BrushRef, Document};
use glam::{Mat4, Vec2, Vec3, Vec4};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpAxis {
    Y,
    Z,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneMeta {
    pub name: String,
    /// Always meters after `scale` is applied (1 world unit × scale = 1 meter).
    pub up: UpAxis,
    /// Always right-handed.
    pub generator: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExportNode {
    pub name: String,
    pub parent: Option<usize>,
    /// Geometry is baked into world space, so this is identity today; kept for formats and
    /// plugins that expect a transform per node.
    pub transform: Mat4,
    pub mesh: Option<usize>,
    pub curve: Option<usize>,
    pub layer: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ExportMesh {
    pub name: String,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// Per vertex; empty if the source has none. Strokes: `u` = distance along the stroke in
    /// sketch units, `v` = 0..1 around the tube or across the ribbon.
    pub uvs: Vec<Vec2>,
    /// Per vertex, glTF convention (xyz along `u`, w = bitangent sign); empty if none.
    pub tangents: Vec<Vec4>,
    pub indices: Vec<u32>,
    pub material: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExportMaterial {
    pub name: String,
    /// sRGB RGBA. Formats that want linear values convert with [`srgb_to_linear`].
    pub base_color: [f32; 4],
    pub roughness: f32,
    pub metallic: f32,
    /// Emission strength; 0 = none. Emits `base_color`.
    pub emission: f32,
    pub double_sided: bool,
    pub unlit: bool,
}

/// A stroke's processed centerline, for formats that keep strokes as curves.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ExportCurve {
    pub points: Vec<Vec3>,
    pub widths: Vec<f32>,
    pub material: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExportScene {
    pub meta: SceneMeta,
    pub nodes: Vec<ExportNode>,
    pub meshes: Vec<ExportMesh>,
    pub materials: Vec<ExportMaterial>,
    pub curves: Vec<ExportCurve>,
}

impl ExportScene {
    /// Nodes without a parent.
    pub fn roots(&self) -> impl Iterator<Item = usize> + '_ {
        self.nodes.iter().enumerate().filter(|(_, n)| n.parent.is_none()).map(|(i, _)| i)
    }

    pub fn children(&self, parent: usize) -> impl Iterator<Item = usize> + '_ {
        self.nodes.iter().enumerate().filter(move |(_, n)| n.parent == Some(parent)).map(|(i, _)| i)
    }
}

/// Options every export shares, applied while building the scene.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneOptions {
    pub scale: f32,
    pub up: UpAxis,
    /// One mesh per layer and material instead of one per stroke.
    pub merge: bool,
    pub include_curves: bool,
    pub include_hidden: bool,
}

impl Default for SceneOptions {
    fn default() -> Self {
        Self { scale: 1.0, up: UpAxis::Y, merge: false, include_curves: false, include_hidden: false }
    }
}

impl SceneOptions {
    pub fn schema() -> Vec<ParamSpec> {
        vec![
            ParamSpec::float("scale", 1.0, 0.0001, 10000.0, "Meters per sketch unit"),
            ParamSpec::bool("z_up", false, "Z axis up (Blender, Unreal, 3ds Max) instead of Y up"),
            ParamSpec::bool("merge", false, "One mesh per layer and color instead of one per stroke"),
            ParamSpec::bool("curves", false, "Also export stroke centerlines as curves/lines"),
            ParamSpec::bool("hidden_layers", false, "Include hidden layers"),
        ]
    }

    pub fn from_params(p: &Params) -> Self {
        let d = Self::default();
        let get = |k: &str| p.contains_key(k);
        Self {
            scale: if get("scale") { p.float("scale") } else { d.scale },
            up: if p.flag("z_up") { UpAxis::Z } else { UpAxis::Y },
            merge: p.flag("merge"),
            include_curves: p.flag("curves"),
            include_hidden: p.flag("hidden_layers"),
        }
    }

    /// Sketch space (Y up, right-handed) → export space.
    fn point(&self, v: Vec3) -> Vec3 {
        self.dir(v) * self.scale
    }

    fn dir(&self, v: Vec3) -> Vec3 {
        match self.up {
            UpAxis::Y => v,
            // Rotate +90° about X: Y up becomes Z up, handedness preserved.
            UpAxis::Z => Vec3::new(v.x, -v.z, v.y),
        }
    }
}

/// Builds the export scene. `brush` resolves a document brush snapshot to a compiled brush
/// (the host owns compilation and caching).
pub fn build_scene(
    doc: &Document,
    name: &str,
    brush: &mut dyn FnMut(BrushRef) -> Arc<Brush>,
    opts: &SceneOptions,
) -> ExportScene {
    let mut scene = ExportScene {
        meta: SceneMeta { name: name.into(), up: opts.up, generator: format!("AirDraw {}", env!("CARGO_PKG_VERSION")) },
        nodes: Vec::new(),
        meshes: Vec::new(),
        materials: Vec::new(),
        curves: Vec::new(),
    };
    let mut materials: HashMap<BrushRef, usize> = HashMap::new();

    for layer in doc.layers() {
        if !layer.visible && !opts.include_hidden {
            continue;
        }
        let strokes: Vec<_> = doc.strokes().iter().filter(|s| s.layer == layer.id).collect();
        if strokes.is_empty() {
            continue;
        }
        let layer_node = scene.nodes.len();
        scene.nodes.push(node(&layer.name, None, Some(&layer.name)));
        // Merge target per material: mesh index.
        let mut merged: Vec<(usize, usize)> = Vec::new();

        for stroke in strokes {
            let b = brush(stroke.brush);
            let material = *materials.entry(stroke.brush).or_insert_with(|| {
                scene.materials.push(material(&b, scene.materials.len()));
                scene.materials.len() - 1
            });
            let built = b.build(&stroke.points);
            let stroke_name = format!("stroke_{}", stroke.id.0);

            if !built.is_empty() {
                let mesh = ExportMesh {
                    name: stroke_name.clone(),
                    positions: built.positions.iter().map(|&p| opts.point(p)).collect(),
                    normals: built.normals.iter().map(|&n| opts.dir(n)).collect(),
                    uvs: built.uvs.clone(),
                    // The axis change is a rotation, so the handedness sign carries over.
                    tangents: built.tangents.iter().map(|t| opts.dir(t.truncate()).extend(t.w)).collect(),
                    indices: built.indices.iter().map(|&i| i as u32).collect(),
                    material,
                };
                if opts.merge {
                    match merged.iter().find(|(m, _)| *m == material) {
                        Some(&(_, target)) => append(&mut scene.meshes[target], mesh),
                        None => {
                            let mat_name = scene.materials[material].name.clone();
                            let target = scene.meshes.len();
                            scene.meshes.push(ExportMesh { name: format!("{}_{}", layer.name, mat_name), ..mesh });
                            let mut n = node(&scene.meshes[target].name, Some(layer_node), Some(&layer.name));
                            n.mesh = Some(target);
                            scene.nodes.push(n);
                            merged.push((material, target));
                        }
                    }
                } else {
                    scene.meshes.push(mesh);
                    let mut n = node(&stroke_name, Some(layer_node), Some(&layer.name));
                    n.mesh = Some(scene.meshes.len() - 1);
                    scene.nodes.push(n);
                }
            }

            if opts.include_curves {
                let line = b.centerline(&stroke.points);
                if line.len() >= 2 {
                    scene.curves.push(ExportCurve {
                        points: line.iter().map(|p| opts.point(p.pos)).collect(),
                        widths: line.iter().map(|p| p.width * opts.scale).collect(),
                        material,
                    });
                    let mut n = node(&format!("{stroke_name}_curve"), Some(layer_node), Some(&layer.name));
                    n.curve = Some(scene.curves.len() - 1);
                    scene.nodes.push(n);
                }
            }
        }
    }
    scene
}

fn node(name: &str, parent: Option<usize>, layer: Option<&str>) -> ExportNode {
    ExportNode { name: name.into(), parent, transform: Mat4::IDENTITY, mesh: None, curve: None, layer: layer.map(Into::into) }
}

fn append(into: &mut ExportMesh, from: ExportMesh) {
    let offset = into.positions.len() as u32;
    into.positions.extend(from.positions);
    into.normals.extend(from.normals);
    into.uvs.extend(from.uvs);
    into.tangents.extend(from.tangents);
    into.indices.extend(from.indices.into_iter().map(|i| i + offset));
}

fn material(brush: &Brush, index: usize) -> ExportMaterial {
    let d: &MaterialDesc = brush.material();
    // Brush names can repeat across snapshots (edited brushes); keep material names unique.
    let name = format!("{}_{}", sanitize(&brush.def.name), index);
    ExportMaterial {
        name,
        base_color: d.color,
        roughness: d.roughness,
        metallic: d.metallic,
        emission: d.emission,
        double_sided: d.double_sided,
        unlit: d.shading == Shading::Unshaded,
    }
}

/// Identifier-safe name for formats with strict naming rules (OBJ/MTL, engines).
pub fn sanitize(s: &str) -> String {
    let out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    if out.is_empty() { "unnamed".into() } else { out }
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use airdraw_brush::{BrushLibrary, InputPoint, StageRegistry};

    pub fn sample_doc() -> (Document, BrushLibrary) {
        let lib = BrushLibrary::new(StageRegistry::with_builtins());
        let mut doc = Document::new();
        for (i, id) in ["com.airdraw.ink", "com.airdraw.ribbon", "com.airdraw.ink"].iter().enumerate() {
            let r = doc.intern_brush(&lib.get(id).unwrap().def);
            let pts = (0..30)
                .map(|k| InputPoint {
                    pos: Vec3::new(k as f32 * 0.01, i as f32 * 0.1, 0.0),
                    normal: Vec3::Z,
                    pressure: 0.8,
                    tilt: None,
                    t: k as f64 / 120.0,
                })
                .collect();
            doc.add_stroke(r, pts);
        }
        (doc, lib)
    }

    pub fn build(doc: &Document, lib: &BrushLibrary, opts: &SceneOptions) -> ExportScene {
        let reg = lib.registry().clone();
        let mut resolve = |r: BrushRef| Arc::new(Brush::compile(doc.brush_def(r).unwrap().clone(), &reg).unwrap());
        build_scene(doc, "test", &mut resolve, opts)
    }

    #[test]
    fn one_node_per_stroke_under_layer() {
        let (doc, lib) = sample_doc();
        let s = build(&doc, &lib, &SceneOptions::default());
        assert_eq!(s.roots().count(), 1);
        assert_eq!(s.children(0).count(), 3);
        assert_eq!(s.meshes.len(), 3);
        assert_eq!(s.materials.len(), 2, "two strokes share the ink snapshot");
        assert!(s.meshes.iter().all(|m| m.indices.iter().all(|&i| (i as usize) < m.positions.len())));
    }

    #[test]
    fn merge_groups_by_material() {
        let (doc, lib) = sample_doc();
        let s = build(&doc, &lib, &SceneOptions { merge: true, ..Default::default() });
        assert_eq!(s.meshes.len(), 2);
        let ink = s.meshes.iter().find(|m| m.material == 0).unwrap();
        let single = build(&doc, &lib, &SceneOptions::default());
        assert_eq!(ink.positions.len(), single.meshes[0].positions.len() * 2);
        assert!(ink.indices.iter().all(|&i| (i as usize) < ink.positions.len()));
    }

    #[test]
    fn uvs_and_tangents_follow_the_axis_change() {
        let (doc, lib) = sample_doc();
        for up in [UpAxis::Y, UpAxis::Z] {
            let s = build(&doc, &lib, &SceneOptions { up, ..Default::default() });
            for m in &s.meshes {
                assert_eq!(m.uvs.len(), m.positions.len());
                assert_eq!(m.tangents.len(), m.positions.len());
                for (t, n) in m.tangents.iter().zip(&m.normals) {
                    assert!(t.truncate().dot(*n).abs() < 1e-3, "tangent must stay perpendicular after {up:?}");
                }
            }
        }
    }

    #[test]
    fn z_up_and_scale() {
        let (doc, lib) = sample_doc();
        let y = build(&doc, &lib, &SceneOptions::default());
        let z = build(&doc, &lib, &SceneOptions { up: UpAxis::Z, scale: 2.0, ..Default::default() });
        let (a, b) = (y.meshes[1].positions[0], z.meshes[1].positions[0]);
        assert!((b - Vec3::new(a.x, -a.z, a.y) * 2.0).length() < 1e-5);
    }

    #[test]
    fn curves_are_optional() {
        let (doc, lib) = sample_doc();
        assert!(build(&doc, &lib, &SceneOptions::default()).curves.is_empty());
        let s = build(&doc, &lib, &SceneOptions { include_curves: true, ..Default::default() });
        assert_eq!(s.curves.len(), 3);
        assert_eq!(s.nodes.iter().filter(|n| n.curve.is_some()).count(), 3);
    }

    #[test]
    fn hidden_layers_skipped_unless_requested() {
        let (mut doc, lib) = sample_doc();
        let layer = doc.layers()[0].id;
        doc.set_layer_visible(layer, false);
        assert!(build(&doc, &lib, &SceneOptions::default()).meshes.is_empty());
        let s = build(&doc, &lib, &SceneOptions { include_hidden: true, ..Default::default() });
        assert_eq!(s.meshes.len(), 3);
    }

    #[test]
    fn empty_document_gives_empty_scene() {
        let lib = BrushLibrary::new(StageRegistry::with_builtins());
        let s = build(&Document::new(), &lib, &SceneOptions::default());
        assert!(s.nodes.is_empty() && s.meshes.is_empty());
    }
}
