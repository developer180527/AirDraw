//! The stroke being drawn (docs/ARCHITECTURE.md §3e). Its brush streams: geometry that will
//! never change is uploaded once, in sealed chunks; only the open chunk and the provisional
//! tail near the pen are re-uploaded per update, so cost doesn't grow with stroke length.

use std::ops::Range;
use std::time::{Duration, Instant};

use airdraw_brush::{Brush, BrushStream, InputPoint, MeshBuffers};
use airdraw_doc::BrushRef;
use godot::classes::mesh::{ArrayType, PrimitiveType};
use godot::classes::{ArrayMesh, Material, MeshInstance3D};
use godot::prelude::*;

/// Committed geometry is sealed once the open chunk spans this many vertices.
const CHUNK_VERTICES: usize = 4096;
/// Godot's limit on surfaces per mesh (`RS::MAX_MESH_SURFACES`).
const MAX_SURFACES: i32 = 256;

/// The finished stroke is the stroke node with one surface per sealed chunk plus one for the
/// rest, so pen-up uploads at most one chunk. Short strokes (one chunk) end as one surface.
pub(crate) struct LiveStroke {
    pub brush_ref: BrushRef,
    /// Raw input so far; this is what the document stores.
    pub points: Vec<InputPoint>,
    stream: BrushStream,
    /// The stroke's node; its mesh holds the sealed chunks.
    pub instance: Gd<MeshInstance3D>,
    sealed_mesh: Gd<ArrayMesh>,
    /// Committed index entries already in sealed chunks.
    sealed_indices: usize,
    /// Child of `instance` showing the open chunk and the provisional tail, re-uploaded per update.
    live_node: Gd<MeshInstance3D>,
    live_mesh: Gd<ArrayMesh>,
}

/// Cost of one live-stroke update, split by phase (design 3e).
#[derive(Default)]
pub(crate) struct UpdateTiming {
    pub points: usize,
    /// Vertices of the whole stroke as drawn (committed + provisional).
    pub vertices: usize,
    /// Vertices sent to Godot in this update.
    pub uploaded: usize,
    /// Streaming the new points and building the provisional tail.
    pub build: Duration,
    /// `MeshBuffers` → Godot packed arrays.
    pub convert: Duration,
    /// `add_surface_from_arrays` (GPU buffer creation) and node updates.
    pub upload: Duration,
}

impl LiveStroke {
    pub fn new(brush: &Brush, brush_ref: BrushRef, first: InputPoint, material: &Gd<Material>, layers: u32) -> Self {
        let node = |mesh: &Gd<ArrayMesh>| {
            let mut n = MeshInstance3D::new_alloc();
            n.set_layer_mask(layers);
            n.set_material_override(material);
            n.set_mesh(mesh);
            n
        };
        let (sealed_mesh, live_mesh) = (ArrayMesh::new_gd(), ArrayMesh::new_gd());
        let mut instance = node(&sealed_mesh);
        let live_node = node(&live_mesh);
        instance.add_child(&live_node);
        let mut stream = brush.stream();
        stream.push(&[first]);
        Self { brush_ref, points: vec![first], stream, instance, sealed_mesh, sealed_indices: 0, live_node, live_mesh }
    }

    /// Adds real points, replaces the predicted tail, and updates what's on screen.
    pub fn update(&mut self, points: &[InputPoint], predicted: &[InputPoint]) -> UpdateTiming {
        let start = Instant::now();
        self.points.extend_from_slice(points);
        self.stream.push(points);
        let preview = self.stream.preview(predicted);
        let mut timing = UpdateTiming {
            points: self.points.len(),
            vertices: self.stream.committed_mesh().positions.len() + preview.positions.len(),
            build: start.elapsed(),
            ..Default::default()
        };
        self.seal_if_full(&mut timing);
        self.show_live(&preview, &mut timing);
        timing
    }

    /// Ends the stroke: the open chunk and the end of the stroke become the node's last
    /// surface. `None` if the stroke has no geometry (the node is freed).
    pub fn finish(mut self) -> Option<(Gd<MeshInstance3D>, BrushRef, Vec<InputPoint>, UpdateTiming)> {
        let start = Instant::now();
        let mesh = self.stream.into_mesh(&[]);
        let mut timing = UpdateTiming { points: self.points.len(), vertices: mesh.positions.len(), build: start.elapsed(), ..Default::default() };
        self.live_node.queue_free();
        if mesh.is_empty() {
            self.instance.queue_free();
            return None;
        }
        let rest = &mesh.indices[self.sealed_indices..];
        if !rest.is_empty() {
            if self.sealed_mesh.get_surface_count() >= MAX_SURFACES {
                merge_sealed(&mut self.sealed_mesh, &mesh, self.sealed_indices, &mut timing);
            }
            add_surface(&mut self.sealed_mesh, &mesh, vertex_range(rest), rest, &mut timing);
        }
        Some((self.instance, self.brush_ref, self.points, timing))
    }

    pub fn cancel(mut self) {
        // The live node is a child of the instance and goes with it.
        self.instance.queue_free();
    }

    /// Moves the open committed triangles into a sealed chunk once they span a full chunk.
    fn seal_if_full(&mut self, timing: &mut UpdateTiming) {
        let committed = self.stream.committed_mesh();
        let open = &committed.indices[self.sealed_indices..];
        let Some(range) = (!open.is_empty()).then(|| vertex_range(open)) else { return };
        if committed.positions.len() - range.start < CHUNK_VERTICES {
            return;
        }
        // Keep one surface free for the end of the stroke.
        if self.sealed_mesh.get_surface_count() >= MAX_SURFACES - 1 {
            merge_sealed(&mut self.sealed_mesh, committed, self.sealed_indices, timing);
        }
        add_surface(&mut self.sealed_mesh, committed, range, open, timing);
        self.sealed_indices = committed.indices.len();
    }

    /// Re-uploads the open chunk plus the provisional tail into the live mesh.
    fn show_live(&mut self, preview: &MeshBuffers, timing: &mut UpdateTiming) {
        let committed = self.stream.committed_mesh();
        let len = committed.positions.len();
        let open = &committed.indices[self.sealed_indices..];
        // The tail's indices can point at committed vertices (e.g. the last tube ring).
        let lo = open.iter().chain(preview.indices.iter().filter(|&&i| (i as usize) < len)).min().map_or(len, |&i| i as usize);
        let converted = Instant::now();
        let arrays = surface_arrays(committed, lo..len, Some(preview), &[open, &preview.indices]);
        timing.convert += converted.elapsed();
        let uploaded = Instant::now();
        self.live_mesh.clear_surfaces();
        if !open.is_empty() || !preview.indices.is_empty() {
            self.live_mesh.add_surface_from_arrays(PrimitiveType::TRIANGLES, &arrays);
        }
        timing.upload += uploaded.elapsed();
        timing.uploaded += len - lo + preview.positions.len();
    }
}

/// Adds triangles `tris` of `m` (using its vertices in `range`) to `mesh` as a new surface.
fn add_surface(mesh: &mut Gd<ArrayMesh>, m: &MeshBuffers, range: Range<usize>, tris: &[i32], timing: &mut UpdateTiming) {
    let converted = Instant::now();
    let arrays = surface_arrays(m, range.clone(), None, &[tris]);
    timing.convert += converted.elapsed();
    let uploaded = Instant::now();
    mesh.add_surface_from_arrays(PrimitiveType::TRIANGLES, &arrays);
    timing.upload += uploaded.elapsed();
    timing.uploaded += range.len();
}

/// Rare fallback at the surface limit (~1M vertices): re-uploads the first `sealed` index
/// entries of `m`, i.e. all sealed chunks, as one surface.
fn merge_sealed(mesh: &mut Gd<ArrayMesh>, m: &MeshBuffers, sealed: usize, timing: &mut UpdateTiming) {
    let tris = &m.indices[..sealed];
    mesh.clear_surfaces();
    add_surface(mesh, m, vertex_range(tris), tris, timing);
}

/// The vertices a non-empty index list uses.
fn vertex_range(indices: &[i32]) -> Range<usize> {
    let (lo, hi) = indices.iter().fold((i32::MAX, i32::MIN), |(lo, hi), &i| (lo.min(i), hi.max(i)));
    lo as usize..hi as usize + 1
}

/// A guide's display mesh, with UVs for the grid shader. Guides are two-sided, so winding
/// doesn't matter.
pub(crate) fn guide_mesh(m: &airdraw_guide::GuideMesh) -> Gd<ArrayMesh> {
    let v3 = |v: &glam::Vec3| Vector3::new(v.x, v.y, v.z);
    let mut arrays = VarArray::new();
    arrays.resize(ArrayType::MAX.ord() as usize, &Variant::nil());
    arrays.set(ArrayType::VERTEX.ord() as usize, &m.positions.iter().map(v3).collect::<PackedVector3Array>().to_variant());
    arrays.set(ArrayType::NORMAL.ord() as usize, &m.normals.iter().map(v3).collect::<PackedVector3Array>().to_variant());
    arrays.set(ArrayType::TEX_UV.ord() as usize, &m.uvs.iter().map(|u| Vector2::new(u.x, u.y)).collect::<PackedVector2Array>().to_variant());
    arrays.set(ArrayType::INDEX.ord() as usize, &m.indices.iter().map(|&i| i as i32).collect::<PackedInt32Array>().to_variant());
    let mut mesh = ArrayMesh::new_gd();
    mesh.add_surface_from_arrays(PrimitiveType::TRIANGLES, &arrays);
    mesh
}

/// Surface arrays for a whole mesh.
pub(crate) fn mesh_arrays(m: &MeshBuffers) -> VarArray {
    surface_arrays(m, 0..m.positions.len(), None, &[&m.indices])
}

/// Surface arrays for triangles `tris`, whose indices address `a`'s vertices followed by all of
/// `b`'s, using only `a`'s vertices in `range` (indices are rebased to it).
fn surface_arrays(a: &MeshBuffers, range: Range<usize>, b: Option<&MeshBuffers>, tris: &[&[i32]]) -> VarArray {
    let v3 = |v: &glam::Vec3| Vector3::new(v.x, v.y, v.z);
    let extra = b.map_or(&[][..], |b| &b.positions[..]);
    let extra_normals = b.map_or(&[][..], |b| &b.normals[..]);
    let extra_uvs = b.map_or(&[][..], |b| &b.uvs[..]);
    let extra_tangents = b.map_or(&[][..], |b| &b.tangents[..]);
    let positions: PackedVector3Array = a.positions[range.clone()].iter().chain(extra).map(v3).collect();
    let normals: PackedVector3Array = a.normals[range.clone()].iter().chain(extra_normals).map(v3).collect();
    let uvs: PackedVector2Array = a.uvs[range.clone()].iter().chain(extra_uvs).map(|u| Vector2::new(u.x, u.y)).collect();
    // Godot packs a tangent as 4 floats: direction, then the bitangent sign (as glTF does).
    let tangents: PackedFloat32Array = a.tangents[range.clone()].iter().chain(extra_tangents).flat_map(|t| t.to_array()).collect();
    let base = range.start as i32;
    // Brush meshes are counter-clockwise (the convention of every export format); Godot
    // treats clockwise as front-facing, so flip each triangle for display.
    let indices: PackedInt32Array =
        tris.iter().flat_map(|t| t.as_chunks::<3>().0).flat_map(|t| [t[0] - base, t[2] - base, t[1] - base]).collect();

    let mut arrays = VarArray::new();
    arrays.resize(ArrayType::MAX.ord() as usize, &Variant::nil());
    arrays.set(ArrayType::VERTEX.ord() as usize, &positions.to_variant());
    arrays.set(ArrayType::NORMAL.ord() as usize, &normals.to_variant());
    arrays.set(ArrayType::TANGENT.ord() as usize, &tangents.to_variant());
    arrays.set(ArrayType::TEX_UV.ord() as usize, &uvs.to_variant());
    arrays.set(ArrayType::INDEX.ord() as usize, &indices.to_variant());
    arrays
}
