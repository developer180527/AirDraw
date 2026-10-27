use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use airdraw_brush::{Brush, BrushLibrary, BrushSource, InputPoint, MaterialDesc, MeshBuffers, Shading, StageRegistry};
use airdraw_doc::{BrushRef, Change, Document, Stroke, StrokeId, read_thumbnail};
use airdraw_export::{ExporterRegistry, FolderSink, SceneOptions, build_scene};
use godot::classes::base_material_3d::{CullMode, Feature, ShadingMode};
use godot::classes::mesh::PrimitiveType;
use godot::classes::{ArrayMesh, INode3D, Material, MeshInstance3D, Node3D, StandardMaterial3D};
use godot::prelude::*;

use crate::export_bridge::{def_to_dict, dict_to_def, dict_to_params, slot_from_name, spec_to_dict};
use crate::live::{LiveStroke, UpdateTiming, mesh_arrays};

/// Render layers of stroke nodes: the default layer plus layer 20, which only strokes use,
/// so offscreen cameras (thumbnails) can render strokes without grids or guides.
const STROKE_LAYERS: u32 = 1 | (1 << 19);

/// Shows a [`Document`] as mesh nodes and edits it. Input code feeds world-space points;
/// finished strokes go into the document, and undo/redo/load update only the nodes that
/// changed. Also owns the brush library.
#[derive(GodotClass)]
#[class(base = Node3D)]
pub struct SketchCanvas {
    base: Base<Node3D>,
    library: BrushLibrary,
    brush: Arc<Brush>,
    /// Where user brushes are loaded from and saved to (set by `load_brush_dir`).
    user_brush_dir: Option<std::path::PathBuf>,
    exporters: ExporterRegistry,
    doc: Document,
    /// Compiled brushes and Godot materials per document brush snapshot.
    compiled: HashMap<BrushRef, Arc<Brush>>,
    materials: HashMap<BrushRef, Gd<Material>>,
    nodes: HashMap<StrokeId, Gd<MeshInstance3D>>,
    current: Option<LiveStroke>,
    /// Per-update live-stroke costs, recorded only while benchmarking.
    timings: Option<Vec<UpdateTiming>>,
}

#[godot_api]
impl INode3D for SketchCanvas {
    fn init(base: Base<Node3D>) -> Self {
        let library = BrushLibrary::new(StageRegistry::with_builtins());
        let brush = library.iter().next().expect("built-in brushes").clone();
        let mut exporters = ExporterRegistry::with_builtins();
        for entry in crate::static_plugins::STATIC_PLUGINS {
            match unsafe { airdraw_ffi::PluginExporter::from_entry(*entry) } {
                Ok(p) => exporters.register(p),
                Err(e) => godot_warn!("Built-in exporter plugin rejected: {e}"),
            }
        }
        Self {
            base,
            library,
            brush,
            user_brush_dir: None,
            exporters,
            doc: Document::new(),
            compiled: HashMap::new(),
            materials: HashMap::new(),
            nodes: HashMap::new(),
            current: None,
            timings: None,
        }
    }

    fn exit_tree(&mut self) {
        self.cancel_stroke();
    }
}

#[godot_api]
impl SketchCanvas {
    #[signal]
    fn strokes_changed(count: i64);
    #[signal]
    fn brush_changed(id: GString);
    /// Brushes were added or replaced (e.g. user brushes loaded).
    #[signal]
    fn brushes_changed();

    // ---- Strokes -------------------------------------------------------------------------

    #[func]
    pub(crate) fn begin_stroke(&mut self, pos: Vector3, normal: Vector3, pressure: f32, time: f64) {
        self.cancel_stroke();
        let brush = self.brush.clone();
        let brush_ref = self.doc.intern_brush(&brush.def);
        self.compiled.entry(brush_ref).or_insert_with(|| brush.clone());
        let material = self.material_for(brush_ref);
        let first = input_point(pos, normal, pressure, time);
        let live = LiveStroke::new(&brush, brush_ref, first, &material, STROKE_LAYERS);
        self.base_mut().add_child(&live.instance);
        self.current = Some(live);
    }

    #[func]
    fn add_point(&mut self, pos: Vector3, normal: Vector3, pressure: f32, time: f64) {
        self.extend_stroke(&[input_point(pos, normal, pressure, time)], &[]);
    }

    /// Appends points and replaces the predicted tail, updating the mesh once.
    pub(crate) fn extend_stroke(&mut self, points: &[InputPoint], predicted: &[InputPoint]) {
        if let Some(s) = &mut self.current {
            let t = s.update(points, predicted);
            if let Some(timings) = &mut self.timings {
                timings.push(t);
            }
        }
    }

    /// Appends several points with one rebuild, as a frame of coalesced pen samples does.
    #[func]
    fn add_points(&mut self, positions: PackedVector3Array, normal: Vector3, pressures: PackedFloat32Array, times: PackedFloat64Array) {
        let points: Vec<InputPoint> = (0..positions.len())
            .map(|i| input_point(positions[i], normal, pressures.get(i).unwrap_or(1.0), times.get(i).unwrap_or(0.0)))
            .collect();
        self.extend_stroke(&points, &[]);
    }

    /// Starts (or stops) recording the cost of every live-stroke update; clears earlier records.
    #[func]
    fn set_record_live_timings(&mut self, on: bool) {
        self.timings = on.then(Vec::new);
    }

    /// Recorded updates as columns: `points`, `vertices`, `uploaded_vertices` (packed ints) and `build_ms`,
    /// `convert_ms`, `upload_ms` (packed floats). Clears the records.
    #[func]
    fn take_live_timings(&mut self) -> VarDictionary {
        let timings = self.timings.as_mut().map(std::mem::take).unwrap_or_default();
        let ms = |f: fn(&UpdateTiming) -> Duration| -> PackedFloat64Array {
            timings.iter().map(|t| f(t).as_secs_f64() * 1000.0).collect()
        };
        let mut d = VarDictionary::new();
        d.set("points", &timings.iter().map(|t| t.points as i64).collect::<PackedInt64Array>());
        d.set("vertices", &timings.iter().map(|t| t.vertices as i64).collect::<PackedInt64Array>());
        d.set("uploaded_vertices", &timings.iter().map(|t| t.uploaded as i64).collect::<PackedInt64Array>());
        d.set("build_ms", &ms(|t| t.build));
        d.set("convert_ms", &ms(|t| t.convert));
        d.set("upload_ms", &ms(|t| t.upload));
        d
    }

    #[func]
    pub(crate) fn end_stroke(&mut self) {
        let Some(s) = self.current.take() else { return };
        let Some((instance, brush_ref, points, t)) = s.finish() else { return };
        if let Some(timings) = &mut self.timings {
            timings.push(t);
        }
        let (id, changes) = self.doc.add_stroke(brush_ref, points);
        // The live node already shows this stroke; adopt it instead of rebuilding.
        self.nodes.insert(id, instance);
        self.apply(changes);
    }

    #[func]
    pub(crate) fn cancel_stroke(&mut self) {
        if let Some(s) = self.current.take() {
            s.cancel();
        }
    }

    #[func]
    fn is_drawing(&self) -> bool {
        self.current.is_some()
    }

    #[func]
    pub(crate) fn undo(&mut self) {
        let changes = self.doc.undo();
        self.apply(changes);
    }

    #[func]
    pub(crate) fn redo(&mut self) {
        let changes = self.doc.redo();
        self.apply(changes);
    }

    #[func]
    fn can_undo(&self) -> bool {
        self.doc.can_undo()
    }

    #[func]
    fn can_redo(&self) -> bool {
        self.doc.can_redo()
    }

    /// Removes all strokes (undoable).
    #[func]
    fn clear(&mut self) {
        self.cancel_stroke();
        let changes = self.doc.clear();
        self.apply(changes);
    }

    /// Bounding box of all strokes (empty at the origin when there are none).
    #[func]
    fn strokes_aabb(&self) -> Aabb {
        let mut boxes = self.nodes.values().filter(|n| n.get_mesh().is_some()).map(|n| n.get_global_transform() * n.get_aabb());
        let first = boxes.next().unwrap_or_default();
        boxes.fold(first, |acc, b| acc.merge(b))
    }

    /// Sets the PNG preview stored with the next save (empty clears it). Returns an error
    /// message, or an empty string.
    #[func]
    fn set_thumbnail(&mut self, png: PackedByteArray) -> GString {
        let png = (!png.is_empty()).then(|| png.to_vec());
        match self.doc.set_thumbnail(png) {
            Ok(()) => GString::new(),
            Err(e) => e.to_string().as_str().into(),
        }
    }

    /// The preview embedded in a sketch file, read without loading it (empty if none or
    /// unreadable). For Recents.
    #[func]
    fn thumbnail_of(&self, path: GString) -> PackedByteArray {
        match read_thumbnail(Path::new(&path.to_string())) {
            Ok(Some(png)) => PackedByteArray::from(png.as_slice()),
            _ => PackedByteArray::new(),
        }
    }

    #[func]
    fn stroke_count(&self) -> i64 {
        self.doc.strokes().len() as i64
    }

    /// Re-brushes the most recent stroke with the current brush, keeping its raw input (undoable).
    #[func]
    fn rebrush_last(&mut self) -> bool {
        let Some(id) = self.doc.strokes().last().map(|s| s.id) else { return false };
        let brush = self.brush.clone();
        let r = self.doc.intern_brush(&brush.def);
        self.compiled.entry(r).or_insert(brush);
        let changes = self.doc.set_brush(&[id], r);
        let changed = !changes.is_empty();
        self.apply(changes);
        changed
    }

    // ---- Brush editing --------------------------------------------------------------------

    /// Registered stage types per slot: `{filters: [...], dynamics, geometry, material}`.
    #[func]
    fn brush_stage_names(&self) -> VarDictionary {
        let names = self.library.registry().names();
        let arr = |v: Vec<String>| v.iter().map(|s| GString::from(s.as_str())).collect::<PackedStringArray>();
        let mut d = VarDictionary::new();
        d.set("filters", &arr(names.filters));
        d.set("dynamics", &arr(names.dynamics));
        d.set("geometry", &arr(names.geometry));
        d.set("material", &arr(names.materials));
        d
    }

    /// Parameter schema of one stage (`slot`: filters, dynamics, geometry or material).
    #[func]
    fn brush_stage_schema(&self, slot: GString, kind: GString) -> Array<VarDictionary> {
        slot_from_name(&slot.to_string())
            .and_then(|s| self.library.registry().schema(s, &kind.to_string()))
            .unwrap_or_default()
            .iter()
            .map(|s| spec_to_dict(s, "stage"))
            .collect()
    }

    /// Editable definition of a brush (see `export_bridge::def_to_dict`), empty if unknown.
    #[func]
    fn brush_definition(&self, id: GString) -> VarDictionary {
        self.library.get(&id.to_string()).map(|b| def_to_dict(&b.def, self.library.registry())).unwrap_or_default()
    }

    /// True if the brush ships with the app (it can be overridden but not deleted).
    #[func]
    fn brush_is_builtin(&self, id: GString) -> bool {
        BrushLibrary::is_builtin_id(&id.to_string())
    }

    /// True if a user file defines this brush (a custom brush or an override of a built-in).
    #[func]
    fn brush_is_user(&self, id: GString) -> bool {
        matches!(self.library.source(&id.to_string()), Some(BrushSource::User(_)))
    }

    /// Builds a sample stroke with an edited, unsaved definition.
    /// Returns `{error, mesh: ArrayMesh, material: Material}`.
    #[func]
    fn brush_preview(&self, def: VarDictionary) -> VarDictionary {
        let mut out = VarDictionary::new();
        let compiled = dict_to_def(&def, self.library.registry())
            .and_then(|d| Brush::compile(d, self.library.registry()).map_err(|e| e.to_string()));
        match compiled {
            Ok(brush) => {
                let mesh = brush.build(&preview_stroke());
                if mesh.is_empty() {
                    out.set("error", "this brush produces no geometry");
                } else {
                    out.set("error", "");
                    out.set("mesh", &array_mesh(&mesh));
                    out.set("material", &godot_material(brush.material()));
                }
            }
            Err(e) => out.set("error", e.as_str()),
        }
        out
    }

    /// Validates and saves a definition to the user brush folder, then selects it.
    /// Returns an empty string on success, else the error.
    #[func]
    fn save_brush(&mut self, def: VarDictionary) -> GString {
        let Some(dir) = self.user_brush_dir.clone() else { return "no user brush folder set".into() };
        let result = dict_to_def(&def, self.library.registry())
            .and_then(|d| self.library.save(d, &dir).map(|_| ()).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                let id = def.get("id").map(|v| v.to::<GString>()).unwrap_or_default();
                self.signals().brushes_changed().emit();
                self.set_brush(id);
                GString::new()
            }
            Err(e) => e.as_str().into(),
        }
    }

    /// Deletes a user brush (restoring the built-in it replaced, if any).
    #[func]
    fn delete_brush(&mut self, id: GString) -> GString {
        match self.library.delete(&id.to_string()) {
            Ok(()) => {
                // Keep a valid selection: the restored built-in, or the default brush.
                self.brush = self.library.get(self.brush.id()).unwrap_or_else(|| self.library.iter().next().unwrap().clone());
                self.signals().brushes_changed().emit();
                let current = GString::from(self.brush.id());
                self.signals().brush_changed().emit(&current);
                GString::new()
            }
            Err(e) => e.to_string().as_str().into(),
        }
    }

    /// Writes a brush to a shareable `.brush.toml` file (absolute path).
    #[func]
    fn export_brush(&self, id: GString, path: GString) -> GString {
        match self.library.export_file(&id.to_string(), Path::new(&path.to_string())) {
            Ok(()) => GString::new(),
            Err(e) => e.to_string().as_str().into(),
        }
    }

    /// Installs every `*.brush.toml` in a folder (absolute path) into the user brushes.
    /// Returns `{imported: PackedStringArray of ids, errors: PackedStringArray}`.
    #[func]
    fn import_brushes(&mut self, dir: GString) -> VarDictionary {
        let mut imported = PackedStringArray::new();
        let mut errors = PackedStringArray::new();
        match self.user_brush_dir.clone() {
            None => errors.push("no user brush folder set"),
            Some(target) => {
                let mut files: Vec<_> = std::fs::read_dir(dir.to_string())
                    .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
                    .unwrap_or_default();
                files.retain(|p| p.to_string_lossy().ends_with(".brush.toml"));
                files.sort();
                for file in files {
                    match self.library.import_file(&file, &target) {
                        Ok(id) => imported.push(id.as_str()),
                        Err(e) => errors.push(format!("{}: {e}", file.file_name().unwrap_or_default().to_string_lossy()).as_str()),
                    }
                }
            }
        }
        if !imported.is_empty() {
            self.signals().brushes_changed().emit();
        }
        let mut d = VarDictionary::new();
        d.set("imported", &imported);
        d.set("errors", &errors);
        d
    }

    // ---- Documents -----------------------------------------------------------------------

    #[func]
    fn new_document(&mut self) {
        self.cancel_stroke();
        self.doc = Document::new();
        self.apply(vec![Change::Reset]);
    }

    /// Saves to an absolute path. Returns an empty string on success, else the error message.
    #[func]
    fn save_document(&mut self, path: GString) -> GString {
        match self.doc.save(Path::new(&path.to_string())) {
            Ok(()) => {
                self.doc.mark_saved();
                GString::new()
            }
            Err(e) => GString::from(e.to_string().as_str()),
        }
    }

    /// Replaces the document with the file at an absolute path. Returns an empty string on
    /// success, else the error message (the current document is kept).
    #[func]
    fn load_document(&mut self, path: GString) -> GString {
        match Document::load(Path::new(&path.to_string())) {
            Ok(doc) => {
                self.cancel_stroke();
                self.doc = doc;
                self.apply(vec![Change::Reset]);
                GString::new()
            }
            Err(e) => GString::from(e.to_string().as_str()),
        }
    }

    /// True if there are changes since the last save or load.
    #[func]
    fn is_dirty(&self) -> bool {
        self.doc.is_dirty()
    }

    // ---- Export -------------------------------------------------------------------------

    /// Available formats: `[{id, name, extension}]` in display order.
    #[func]
    fn exporters(&self) -> Array<VarDictionary> {
        self.exporters
            .iter()
            .map(|e| {
                let info = e.info();
                let mut d = VarDictionary::new();
                d.set("id", info.id.as_str());
                d.set("name", info.name.as_str());
                d.set("extension", info.extension.as_str());
                d
            })
            .collect()
    }

    /// Loads exporter plugins (shared libraries) from a folder (absolute path) and adds them
    /// to the format list. Returns one message per file. Desktop only; mobile builds link
    /// plugins statically (see `static_plugins.rs`).
    #[func]
    fn load_exporter_plugins(&mut self, dir: GString) -> PackedStringArray {
        #[cfg_attr(any(target_os = "ios", target_os = "android"), allow(unused_mut))]
        let mut messages = PackedStringArray::new();
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        for (path, result) in unsafe { airdraw_ffi::load_dir(Path::new(&dir.to_string())) } {
            let file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            match result {
                Ok(plugin) => {
                    let name = airdraw_export::Exporter::info(&plugin).name;
                    messages.push(format!("Loaded exporter '{name}' ({file})").as_str());
                    self.exporters.register(plugin);
                }
                Err(e) => messages.push(format!("Skipped {file}: {e}").as_str()),
            }
        }
        #[cfg(any(target_os = "ios", target_os = "android"))]
        let _ = dir;
        messages
    }

    /// Options for the export dialog: common scene options (group "scene") followed by the
    /// format's own (group "format"). See `export_bridge::spec_to_dict` for the shape.
    #[func]
    fn export_options(&self, exporter_id: GString) -> Array<VarDictionary> {
        let mut out: Array<VarDictionary> = SceneOptions::schema().iter().map(|s| spec_to_dict(s, "scene")).collect();
        if let Some(e) = self.exporters.get(&exporter_id.to_string()) {
            for s in e.options() {
                out.push(&spec_to_dict(&s, "format"));
            }
        }
        out
    }

    /// Exports the document to `path` (absolute; companion files go next to it).
    /// Returns `{error: String, files: PackedStringArray, warnings: PackedStringArray}`.
    #[func]
    fn export_document(&mut self, exporter_id: GString, path: GString, options: VarDictionary) -> VarDictionary {
        let params = dict_to_params(&options);
        let path = std::path::PathBuf::from(path.to_string());
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        // Compile every snapshot up front: build_scene borrows the document.
        let refs: Vec<BrushRef> = self.doc.strokes().iter().map(|s| s.brush).collect();
        let brushes: HashMap<BrushRef, Arc<Brush>> = refs.into_iter().map(|r| (r, self.brush_for(r))).collect();
        let mut scene_opts = SceneOptions::from_params(&params);
        if self.exporters.get(&exporter_id.to_string()).is_some_and(|e| e.info().caps.requires_curves) {
            scene_opts.include_curves = true;
        }
        let scene = build_scene(&self.doc, &name, &mut |r| brushes[&r].clone(), &scene_opts);

        let mut result = VarDictionary::new();
        let mut sink = FolderSink::for_path(&path);
        match self.exporters.export(&exporter_id.to_string(), &scene, &params, &mut sink) {
            Ok(report) => {
                let dir = path.parent().unwrap_or(std::path::Path::new(""));
                let files: PackedStringArray =
                    report.files.iter().map(|f| GString::from(dir.join(f).to_string_lossy().as_ref())).collect();
                let warnings: PackedStringArray = report.warnings.iter().map(|w| GString::from(w.as_str())).collect();
                result.set("error", "");
                result.set("files", &files);
                result.set("warnings", &warnings);
            }
            Err(e) => {
                result.set("error", e.to_string().as_str());
                result.set("files", &PackedStringArray::new());
                result.set("warnings", &PackedStringArray::new());
            }
        }
        result
    }

    /// Brings the nodes in line with document changes.
    fn apply(&mut self, changes: Vec<Change>) {
        if changes.is_empty() {
            return;
        }
        for change in changes {
            match change {
                Change::Added(id) if !self.nodes.contains_key(&id) => {
                    if let Some(stroke) = self.doc.stroke(id).cloned() {
                        let node = self.make_node(&stroke);
                        self.nodes.insert(id, node);
                    }
                }
                Change::Added(_) => {}
                Change::Removed(id) => {
                    if let Some(mut node) = self.nodes.remove(&id) {
                        node.queue_free();
                    }
                }
                Change::Updated(id) => {
                    if let (Some(stroke), Some(mut node)) = (self.doc.stroke(id).cloned(), self.nodes.get(&id).cloned()) {
                        node.set_material_override(&self.material_for(stroke.brush));
                        set_mesh(&mut node, &self.brush_for(stroke.brush).build(&stroke.points));
                    }
                }
                Change::Reset => {
                    for (_, mut node) in self.nodes.drain() {
                        node.queue_free();
                    }
                    // Snapshot indices belong to the old document.
                    self.compiled.clear();
                    self.materials.clear();
                    for stroke in self.doc.strokes().to_vec() {
                        let node = self.make_node(&stroke);
                        self.nodes.insert(stroke.id, node);
                    }
                }
            }
        }
        let count = self.doc.strokes().len() as i64;
        self.signals().strokes_changed().emit(count);
    }

    fn make_node(&mut self, stroke: &Stroke) -> Gd<MeshInstance3D> {
        let mut node = MeshInstance3D::new_alloc();
        node.set_layer_mask(STROKE_LAYERS);
        node.set_material_override(&self.material_for(stroke.brush));
        set_mesh(&mut node, &self.brush_for(stroke.brush).build(&stroke.points));
        self.base_mut().add_child(&node);
        node
    }

    /// Compiled brush for a document snapshot. Snapshots the app can't compile (e.g. saved by
    /// a newer version with stages this one lacks) fall back to the default brush.
    fn brush_for(&mut self, r: BrushRef) -> Arc<Brush> {
        if let Some(b) = self.compiled.get(&r) {
            return b.clone();
        }
        let compiled = match self.doc.brush_def(r).cloned().map(|d| Brush::compile(d, self.library.registry())) {
            Some(Ok(b)) => Arc::new(b),
            Some(Err(e)) => {
                godot_warn!("Brush snapshot {} can't be used ({e}); drawing with the default brush", r.0);
                self.library.iter().next().unwrap().clone()
            }
            None => self.library.iter().next().unwrap().clone(),
        };
        self.compiled.insert(r, compiled.clone());
        compiled
    }

    fn material_for(&mut self, r: BrushRef) -> Gd<Material> {
        if let Some(m) = self.materials.get(&r) {
            return m.clone();
        }
        let m = godot_material(self.brush_for(r).material());
        self.materials.insert(r, m.clone());
        m
    }

    // ---- Brushes -------------------------------------------------------------------------

    /// Brush ids in display order.
    #[func]
    fn brush_ids(&self) -> PackedStringArray {
        self.library.iter().map(|b| GString::from(b.id())).collect()
    }

    #[func]
    fn brush_name(&self, id: GString) -> GString {
        self.library.get(&id.to_string()).map(|b| GString::from(b.def.name.as_str())).unwrap_or_default()
    }

    /// Display color of a brush (its material color), for swatches.
    #[func]
    fn brush_color(&self, id: GString) -> Color {
        let [r, g, b, a] = self.library.get(&id.to_string()).map_or([1.0; 4], |b| b.material().color);
        Color::from_rgba(r, g, b, a)
    }

    #[func]
    fn current_brush(&self) -> GString {
        GString::from(self.brush.id())
    }

    /// Selects a brush for the next stroke. Returns false if the id is unknown.
    #[func]
    fn set_brush(&mut self, id: GString) -> bool {
        let Some(brush) = self.library.get(&id.to_string()) else { return false };
        self.brush = brush;
        self.signals().brush_changed().emit(&id);
        true
    }

    /// Loads user brushes from a folder (absolute path). Returns how many loaded; invalid
    /// files are reported as warnings and skipped.
    #[func]
    fn load_brush_dir(&mut self, path: GString) -> i64 {
        let before = self.library.iter().count();
        self.user_brush_dir = Some(path.to_string().into());
        for (file, err) in self.library.load_dir(Path::new(&path.to_string())) {
            godot_warn!("Skipping brush {}: {err}", file.display());
        }
        // Pick up a reloaded version of the current brush. Existing strokes keep their snapshots.
        if let Some(b) = self.library.get(self.brush.id()) {
            self.brush = b;
        }
        self.signals().brushes_changed().emit();
        (self.library.iter().count() - before) as i64
    }

}

pub(crate) fn input_point(pos: Vector3, normal: Vector3, pressure: f32, t: f64) -> InputPoint {
    InputPoint {
        pos: glam::Vec3::new(pos.x, pos.y, pos.z),
        normal: glam::Vec3::new(normal.x, normal.y, normal.z),
        pressure,
        tilt: None,
        t,
    }
}

/// An S-curve with a pressure swell, at 120 Hz, for brush previews (fits a ~1 unit view).
fn preview_stroke() -> Vec<InputPoint> {
    const N: usize = 90;
    (0..N)
        .map(|i| {
            let t = i as f32 / (N - 1) as f32;
            InputPoint {
                pos: glam::Vec3::new(-0.4 + 0.8 * t, 0.12 * (t * std::f32::consts::TAU).sin(), 0.0),
                normal: glam::Vec3::Z,
                pressure: 0.15 + 0.85 * (t * std::f32::consts::PI).sin(),
                tilt: None,
                t: i as f64 / 120.0,
            }
        })
        .collect()
}

fn set_mesh(node: &mut Gd<MeshInstance3D>, mesh: &MeshBuffers) {
    if !mesh.is_empty() {
        node.set_mesh(&array_mesh(mesh));
    }
}

fn array_mesh(m: &MeshBuffers) -> Gd<ArrayMesh> {
    let mut mesh = ArrayMesh::new_gd();
    mesh.add_surface_from_arrays(PrimitiveType::TRIANGLES, &mesh_arrays(m));
    mesh
}

fn godot_material(d: &MaterialDesc) -> Gd<Material> {
    let mut m = StandardMaterial3D::new_gd();
    let [r, g, b, a] = d.color;
    m.set_albedo(Color::from_rgba(r, g, b, a));
    m.set_roughness(d.roughness);
    m.set_metallic(d.metallic);
    if d.shading == Shading::Unshaded {
        m.set_shading_mode(ShadingMode::UNSHADED);
    }
    if d.emission > 0.0 {
        m.set_feature(Feature::EMISSION, true);
        m.set_emission(Color::from_rgb(r, g, b));
        m.set_emission_energy_multiplier(d.emission);
    }
    if d.double_sided {
        m.set_cull_mode(CullMode::DISABLED);
    }
    m.upcast()
}
