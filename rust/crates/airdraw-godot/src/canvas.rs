use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use airdraw_brush::{Brush, BrushLibrary, BrushSource, InputPoint, MaterialDesc, MeshBuffers, Shading, StageRegistry};
use airdraw_doc::{BrushRef, Change, Document, Stroke, StrokeId, read_thumbnail};
use airdraw_export::{ExporterRegistry, FolderSink, SceneOptions, build_scene};
use airdraw_guide::Guide;
use airdraw_look::LookLibrary;
use airdraw_pick::{Camera, PickIndex};
use godot::classes::base_material_3d::{CullMode, Feature, ShadingMode};
use godot::classes::mesh::PrimitiveType;
use godot::classes::{ArrayMesh, Camera3D, INode3D, Material, MeshInstance3D, Node3D, Shader, ShaderMaterial, StandardMaterial3D};
use godot::prelude::*;

use crate::export_bridge::{def_to_dict, dict_to_def, dict_to_params, slot_from_name, spec_to_dict};
use crate::live::{LiveStroke, UpdateTiming, mesh_arrays};

/// Render layers of stroke nodes: the default layer plus layer 20, which only strokes use,
/// so offscreen cameras (thumbnails) can render strokes without grids or guides.
const STROKE_LAYERS: u32 = 1 | (1 << 19);

/// The accent outline of selected strokes (`UiTheme.ACCENT`).
const SELECTION_COLOR: Color = Color::from_rgb(0.33, 0.52, 1.0);
/// Drawn over a selected stroke's own surface: lightens it, whatever its colour.
const SELECTION_TINT_SHADER: &str = "shader_type spatial;
render_mode unshaded, cull_disabled, depth_draw_never, shadows_disabled;
void fragment() {
\tALBEDO = vec3(1.0);
\tALPHA = 0.55;
}
";
/// Partial-erase pieces shorter than this (metres) are dropped.
const MIN_PIECE: f32 = 0.001;
/// Ids of live partial-erase pieces in the pick index: far above any document stroke id.
const PIECE_IDS: u64 = 1 << 62;

/// An eraser drag in progress (§3i).
enum Erase {
    /// Strokes touched so far, hidden until the drag ends.
    Whole(Vec<StrokeId>),
    Partial(Partial),
}

/// A partial erase: the document strokes it has cut (hidden) and their live pieces.
#[derive(Default)]
struct Partial {
    /// Each cut stroke with its current pieces in order along it (none: all erased).
    originals: Vec<(StrokeId, Vec<StrokeId>)>,
    pieces: HashMap<StrokeId, Piece>,
    next: u64,
}

struct Piece {
    original: StrokeId,
    /// Same layer and brush as the original; a temporary id.
    stroke: Stroke,
    node: Gd<MeshInstance3D>,
}

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
    /// How strokes are shaded (§3g); the document says which one.
    looks: LookLibrary,
    /// Compiled Look shaders by (shader name, double-sided).
    shaders: HashMap<(String, bool), Gd<Shader>>,
    /// The current Look's outline pass by double-sided (ribbons are outlined differently).
    outlines: HashMap<bool, Gd<Material>>,
    /// Sub-pixel triangles drawn once per material variant to compile shaders ahead of the
    /// first stroke, with the frames they still stay.
    warm_up: Vec<(Gd<MeshInstance3D>, u32)>,
    nodes: HashMap<StrokeId, Gd<MeshInstance3D>>,
    current: Option<LiveStroke>,
    /// Strokes the eraser and selection can pick (§3i), kept in step with the document.
    pick: PickIndex,
    /// Widest half width per brush snapshot, for picking.
    half_widths: HashMap<BrushRef, f32>,
    /// The eraser drag in progress, if any.
    erasing: Option<Erase>,
    /// Selected strokes, in the order they were picked. Not part of the document (§3i).
    selection: Vec<StrokeId>,
    /// The accent outline drawn over selected strokes, by double-sided.
    select_overlays: HashMap<bool, Gd<Material>>,
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
                Err(e) => {
                    godot_warn!("Built-in exporter plugin rejected: {e}");
                }
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
            looks: LookLibrary::with_builtins(),
            shaders: HashMap::new(),
            outlines: HashMap::new(),
            warm_up: Vec::new(),
            nodes: HashMap::new(),
            current: None,
            pick: PickIndex::new(),
            half_widths: HashMap::new(),
            erasing: None,
            selection: Vec::new(),
            select_overlays: HashMap::new(),
            timings: None,
        }
    }

    fn exit_tree(&mut self) {
        self.cancel_stroke();
    }

    /// Frees warm-up triangles once their shaders have been drawn.
    fn process(&mut self, _delta: f64) {
        if self.warm_up.is_empty() {
            return;
        }
        for (_, frames) in &mut self.warm_up {
            *frames = frames.saturating_sub(1);
        }
        for (mut node, _) in self.warm_up.extract_if(.., |(_, f)| *f == 0) {
            node.queue_free();
        }
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
    /// The drawn 3D guide was set, removed, or changed by undo/redo or loading.
    #[signal]
    fn guide_changed();
    /// The Look changed (picked, or a sketch was loaded): apply `look_environment()`.
    #[signal]
    fn look_changed();
    /// The selection changed; `count` strokes are selected.
    #[signal]
    fn selection_changed(count: i64);

    // ---- Looks ---------------------------------------------------------------------------

    /// Available Looks in menu order: `[{id, name}]`, plus the sketch's own if it isn't built in.
    #[func]
    fn looks(&self) -> Array<VarDictionary> {
        // A sketch's Look from another device (not in this library) is listed last, so the
        // menu still shows what the sketch uses.
        let current = self.doc.look().filter(|l| self.looks.get(&l.id).is_none());
        self.looks
            .iter()
            .chain(current)
            .map(|l| {
                let mut d = VarDictionary::new();
                d.set("id", l.id.as_str());
                d.set("name", l.name.as_str());
                d
            })
            .collect()
    }

    /// The sketch's Look id (the default for sketches without one).
    #[func]
    fn look(&self) -> GString {
        GString::from(self.current_look().id.as_str())
    }

    /// The sketch's permanent id, for remembering per-device state outside the file.
    #[func]
    fn content_id(&self) -> GString {
        GString::from(self.doc.content_id())
    }

    /// Shades every stroke with Look `id` and saves it with the sketch. False if unknown.
    #[func]
    fn set_look(&mut self, id: GString) -> bool {
        let Some(look) = self.looks.get(&id.to_string()).cloned() else { return false };
        self.doc.set_look(Some(look));
        self.refresh_materials();
        self.signals().look_changed().emit();
        true
    }

    /// Compiles the current Look's shaders now instead of on the first stroke that needs them
    /// (on iPad a first draw can stall ~0.5–0.8 s): draws one sub-pixel triangle per material
    /// variant (lit and unlit brushes, one- and two-sided, with their outline passes) in front
    /// of `camera`, for a few frames. Call when a Look is applied.
    #[func]
    fn warm_up_look(&mut self, camera: Transform3D) {
        for (node, _) in self.warm_up.drain(..) {
            let mut node = node;
            node.queue_free();
        }
        let tri = warm_up_mesh();
        for (unshaded, double_sided) in [(false, false), (false, true), (true, false), (true, true)] {
            let desc = MaterialDesc {
                shading: if unshaded { Shading::Unshaded } else { Shading::Lit },
                color: [1.0; 4],
                roughness: 0.5,
                metallic: 0.0,
                emission: 0.0,
                double_sided,
            };
            let material = self.build_material(&desc);
            let mut node = MeshInstance3D::new_alloc();
            node.set_mesh(&tri);
            node.set_material_override(&material);
            node.set_layer_mask(STROKE_LAYERS);
            self.base_mut().add_child(&node);
            // Half a metre ahead: past any near plane (a clipped triangle isn't drawn, so its
            // shader wouldn't compile), and still sub-pixel.
            node.set_global_transform(camera * Transform3D::new(Basis::IDENTITY, Vector3::new(0.0, 0.0, -0.5)));
            self.warm_up.push((node, 3));
        }
    }

    /// The Look's lighting: `{background, ambient: Color, ambient_energy, sun_energy: float,
    /// shadows, ssao: bool}`.
    #[func]
    fn look_environment(&self) -> VarDictionary {
        let e = &self.current_look().environment;
        let color = |hex: &str| {
            let [r, g, b] = airdraw_look::parse_color(hex).unwrap_or([1.0; 3]);
            Color::from_rgb(r, g, b)
        };
        let mut d = VarDictionary::new();
        d.set("background", color(&e.background));
        d.set("ambient", color(&e.ambient));
        d.set("ambient_energy", e.ambient_energy);
        d.set("sun_energy", e.sun_energy);
        d.set("shadows", e.shadows);
        d.set("ssao", e.ssao);
        d
    }

    // ---- 3D guide ------------------------------------------------------------------------

    /// True while a drawn guide is active (otherwise strokes use the plane presets).
    #[func]
    fn has_drawn_guide(&self) -> bool {
        self.doc.guide().is_some()
    }

    /// Removes the drawn guide (undoable); strokes go back to the plane presets.
    #[func]
    fn remove_guide(&mut self) {
        let changes = self.doc.set_guide(None);
        self.apply(changes);
    }

    /// The drawn guide's display mesh (UVs in world units for the grid shader), or null.
    #[func]
    fn guide_mesh(&self) -> Option<Gd<ArrayMesh>> {
        let Some(Guide::Sweep(sweep)) = self.doc.guide().map(|g| &**g) else { return None };
        Some(crate::live::guide_mesh(&sweep.mesh(8)))
    }

    /// The drawn guide's start edge (two points), or empty.
    #[func]
    fn guide_start_line(&self) -> PackedVector3Array {
        match self.doc.guide().map(|g| &**g) {
            Some(Guide::Sweep(sweep)) => sweep.start_line().iter().map(|v| Vector3::new(v.x, v.y, v.z)).collect(),
            _ => PackedVector3Array::new(),
        }
    }

    /// Makes `guide` the active guide (undoable).
    pub(crate) fn set_drawn_guide(&mut self, guide: Guide) {
        let changes = self.doc.set_guide(Some(guide));
        self.apply(changes);
    }

    pub(crate) fn drawn_guide(&self) -> Option<Arc<Guide>> {
        self.doc.guide().cloned()
    }

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
        let ms = |f: fn(&UpdateTiming) -> Duration| -> PackedFloat64Array { timings.iter().map(|t| f(t).as_secs_f64() * 1000.0).collect() };
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

    // ---- Erasing (§3i) -----------------------------------------------------------------------

    /// Starts an eraser drag. Whole: strokes it touches hide at once and are removed together
    /// by [`Self::end_erase`]. Partial: strokes are cut where it passes, shown as live pieces,
    /// and replaced by their pieces at the end. Either way, one undo step.
    pub(crate) fn begin_erase(&mut self, partial: bool) {
        self.cancel_stroke();
        self.cancel_erase();
        let skipped = self.doc.layers().iter().filter(|l| !l.visible || l.locked).map(|l| l.id);
        self.pick.set_skipped_layers(skipped);
        self.erasing = Some(if partial { Erase::Partial(Partial::default()) } else { Erase::Whole(Vec::new()) });
    }

    /// Erases along `path` (screen pixels) with an eraser of `radius` pixels: the front strokes
    /// under it. Returns how many strokes this drag has touched so far.
    pub(crate) fn erase_along(&mut self, cam: &Camera, path: &[glam::Vec2], radius: f32) -> usize {
        let moves: Vec<(glam::Vec2, glam::Vec2)> = match path {
            [p] => vec![(*p, *p)],
            _ => path.windows(2).map(|w| (w[0], w[1])).collect(),
        };
        for (a, b) in moves {
            let hits = self.pick.under_capsule(cam, a, b, radius, true);
            match &mut self.erasing {
                None => return 0,
                Some(Erase::Whole(touched)) => {
                    for id in hits {
                        // Hidden strokes no longer hide what's behind them.
                        self.pick.remove(id);
                        if let Some(node) = self.nodes.get_mut(&id) {
                            node.set_visible(false);
                        }
                        touched.push(id);
                    }
                }
                Some(Erase::Partial(_)) => {
                    for id in hits {
                        self.cut_under(cam, id, (a, b), radius);
                    }
                }
            }
        }
        match &self.erasing {
            Some(Erase::Whole(touched)) => touched.len(),
            Some(Erase::Partial(p)) => p.originals.len(),
            None => 0,
        }
    }

    /// Partial erase: cuts stroke or piece `id` along one eraser move. A document stroke is
    /// hidden and becomes pieces the first time; pieces are cut again on later moves.
    fn cut_under(&mut self, cam: &Camera, id: StrokeId, along: (glam::Vec2, glam::Vec2), radius: f32) {
        let Some(Erase::Partial(p)) = &self.erasing else { return };
        let target = match p.pieces.get(&id) {
            Some(piece) => Some((piece.stroke.clone(), Some(id))),
            None => self.doc.stroke(id).cloned().map(|s| (s, None)),
        };
        let Some((stroke, piece)) = target else { return };
        let half_width = self.half_width_for(stroke.brush);
        let Some(kept) = airdraw_pick::cut(cam, &stroke.points, half_width, &[along], radius, MIN_PIECE) else { return };
        let original = match piece {
            Some(_) => self.erasing_partial().pieces[&id].original,
            None => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.set_visible(false);
                }
                self.erasing_partial().originals.push((id, vec![id]));
                id
            }
        };
        self.pick.remove(id);
        if let Some(old) = self.erasing_partial().pieces.remove(&id) {
            let mut node = old.node;
            node.queue_free();
        }
        // The new pieces, in order, take the cut one's place.
        let brush = self.brush_for(stroke.brush);
        let material = self.material_for(stroke.brush);
        let mut ids = Vec::with_capacity(kept.len());
        for points in kept {
            let p = self.erasing_partial();
            p.next += 1;
            let piece_id = StrokeId(PIECE_IDS + p.next);
            let piece = Stroke { id: piece_id, points: points.into(), ..stroke.clone() };
            let mut node = MeshInstance3D::new_alloc();
            node.set_layer_mask(STROKE_LAYERS);
            node.set_material_override(&material);
            set_mesh(&mut node, &brush.build(&piece.points));
            self.base_mut().add_child(&node);
            self.pick.insert(&piece, half_width);
            self.erasing_partial().pieces.insert(piece_id, Piece { original, stroke: piece, node });
            ids.push(piece_id);
        }
        let p = self.erasing_partial();
        let order = &mut p.originals.iter_mut().find(|(o, _)| *o == original).expect("registered above").1;
        let at = order.iter().position(|&x| x == id).expect("cut piece is listed");
        order.splice(at..=at, ids);
    }

    fn erasing_partial(&mut self) -> &mut Partial {
        match &mut self.erasing {
            Some(Erase::Partial(p)) => p,
            _ => unreachable!("only called during a partial erase"),
        }
    }

    /// Commits the drag as one undo step. Returns how many strokes it changed.
    pub(crate) fn end_erase(&mut self) -> usize {
        match self.erasing.take() {
            None => 0,
            Some(Erase::Whole(ids)) => {
                let changes = self.doc.remove_strokes(&ids);
                self.apply(changes);
                ids.len()
            }
            Some(Erase::Partial(mut p)) => {
                let replacements: Vec<(StrokeId, Vec<Vec<InputPoint>>)> = p
                    .originals
                    .iter()
                    .map(|(orig, order)| (*orig, order.iter().map(|id| p.pieces[id].stroke.points.to_vec()).collect()))
                    .collect();
                for (id, piece) in p.pieces.drain() {
                    self.pick.remove(id);
                    let mut node = piece.node;
                    node.queue_free();
                }
                let (changes, _) = self.doc.replace_strokes(replacements);
                self.apply(changes);
                p.originals.len()
            }
        }
    }

    /// Shows what the drag touched again, as if it never happened.
    pub(crate) fn cancel_erase(&mut self) {
        let originals: Vec<StrokeId> = match self.erasing.take() {
            None => return,
            Some(Erase::Whole(ids)) => ids,
            Some(Erase::Partial(mut p)) => {
                for (id, piece) in p.pieces.drain() {
                    self.pick.remove(id);
                    let mut node = piece.node;
                    node.queue_free();
                }
                p.originals.into_iter().map(|(id, _)| id).collect()
            }
        };
        for id in originals {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.set_visible(true);
            }
            self.index_stroke(id);
        }
    }

    /// Erase timings on this device (§3i, `BENCH=erase`): picking on synthetic 100k- and
    /// 1M-point sketches in front of `camera`, then partial and whole erases on a 5,000-point
    /// stroke in this canvas (live pieces rebuilt, as when erasing). Replaces the document.
    /// Returns the report, one line per measure.
    #[func]
    fn erase_benchmark(&mut self, camera: Gd<Camera3D>) -> GString {
        use std::time::Instant;
        let Some(cam) = pick_camera(&camera) else { return "no camera".into() };
        let to_world = camera.get_global_transform();
        let world = |p: glam::Vec3| {
            let v = to_world * Vector3::new(p.x, p.y, p.z);
            glam::Vec3::new(v.x, v.y, v.z)
        };
        let size = camera.get_viewport().map_or(Vector2::new(1000.0, 1000.0), |v| v.get_visible_rect().size);
        let (w, h) = (size.x, size.y);
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
        let stats = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
            format!("mean {mean:.3} ms, p95 {:.3} ms, max {:.3} ms", v[v.len() * 95 / 100], v[v.len() - 1])
        };
        let mut out = Vec::new();
        let mut seed = 7u64;
        let mut rnd = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
        };
        // Picking: wavy strokes of 1,000 points filling the view 3–5 m away; swipes across it.
        let radius = 30.0;
        for strokes in [100usize, 1000] {
            let mut index = PickIndex::new();
            for id in 0..strokes {
                let start = glam::Vec3::new(rnd() * 1.6, rnd() * 1.1, -4.0 + rnd());
                let dir = glam::Vec3::new(rnd(), rnd(), rnd() * 0.3).normalize() * 0.0006;
                let points: Vec<InputPoint> = (0..1000)
                    .map(|k| {
                        let local = start + dir * k as f32 + glam::Vec3::Y * (k as f32 * 0.02).sin() * 0.02;
                        InputPoint { pos: world(local), normal: glam::Vec3::Z, pressure: 0.5, tilt: None, t: k as f64 }
                    })
                    .collect();
                let stroke = Stroke { id: StrokeId(id as u64), layer: airdraw_doc::LayerId(1), brush: BrushRef(0), points: points.into() };
                index.insert(&stroke, 0.006);
            }
            let mut times = Vec::new();
            let mut hits = 0;
            for k in 0..200 {
                let x = w * (0.1 + 0.8 * k as f32 / 200.0);
                let (a, b) = (glam::Vec2::new(x, h * 0.5), glam::Vec2::new(x + w * 0.004, h * 0.5 + 3.0));
                let t = Instant::now();
                hits += index.under_capsule(&cam, a, b, radius, true).len();
                times.push(ms(t));
            }
            let label = if strokes == 1000 { "1M".to_string() } else { format!("{}k", strokes) };
            out.push(format!("pick {label} points, eraser move: {} ({hits} hits)", stats(times)));
            let t = Instant::now();
            let lasso = [
                glam::Vec2::new(w * 0.3, h * 0.3),
                glam::Vec2::new(w * 0.7, h * 0.3),
                glam::Vec2::new(w * 0.7, h * 0.7),
                glam::Vec2::new(w * 0.3, h * 0.7),
            ];
            let n = index.in_lasso(&cam, &lasso).len();
            let lasso_ms = ms(t);
            let t = Instant::now();
            let tap = index.nearest(&cam, glam::Vec2::new(w * 0.5, h * 0.5), 12.0);
            out.push(format!("pick {label} points: lasso {lasso_ms:.2} ms ({n}), tap {:.3} ms ({})", ms(t), tap.is_some()));
        }
        // Erasing a real 5,000-point Ink stroke across the whole view, 4 m away.
        self.new_document();
        let brush = self.library.get("com.airdraw.ink").unwrap_or_else(|| self.brush.clone());
        let r = self.doc.intern_brush(&brush.def);
        self.compiled.insert(r, brush);
        let long: Vec<InputPoint> = (0..5000)
            .map(|k| {
                let f = k as f32 / 4999.0;
                let local = glam::Vec3::new(-6.0 + 12.0 * f, (f * 40.0).sin() * 0.05, -4.0);
                InputPoint { pos: world(local), normal: glam::Vec3::Z, pressure: 0.6, tilt: None, t: f as f64 * 4.0 }
            })
            .collect();
        let at_x = |k: usize| w * (0.15 + 0.7 * k as f32 / 20.0);
        for partial in [true, false] {
            let (_, changes) = self.doc.add_stroke(r, long.clone());
            self.apply(changes);
            self.begin_erase(partial);
            let mut times = Vec::new();
            for k in 0..20 {
                let path = [glam::Vec2::new(at_x(k), h * 0.3), glam::Vec2::new(at_x(k) + 2.0, h * 0.7)];
                let t = Instant::now();
                self.erase_along(&cam, &path, 20.0);
                times.push(ms(t));
            }
            let t = Instant::now();
            self.end_erase();
            let end_ms = ms(t);
            let mode = if partial { "partial" } else { "whole" };
            out.push(format!(
                "{mode} erase of a 5k-point stroke, per move: {}; lift {end_ms:.2} ms; strokes after {}",
                stats(times),
                self.doc.strokes().len()
            ));
            self.clear();
        }
        self.new_document();
        out.join("\n").as_str().into()
    }

    // ---- Selection (§3i) ---------------------------------------------------------------------

    /// Selects the frontmost stroke within `tolerance` pixels of `pos`. `mode`: 0 replaces the
    /// selection (a miss clears it), 1 adds, 2 removes. Returns whether a stroke was there.
    #[func]
    pub(crate) fn select_at(&mut self, camera: Gd<Camera3D>, pos: Vector2, tolerance: f32, mode: i64) -> bool {
        let Some(cam) = pick_camera(&camera) else { return false };
        self.skip_hidden_layers();
        let hit = self.pick.nearest(&cam, glam::Vec2::new(pos.x, pos.y), tolerance);
        self.select(hit.into_iter().collect(), mode);
        hit.is_some()
    }

    /// Selects strokes with at least half their points inside the lasso (screen pixels), at any
    /// depth. `mode` as for [`Self::select_at`]. Returns how many the lasso caught.
    #[func]
    pub(crate) fn select_lasso(&mut self, camera: Gd<Camera3D>, polygon: PackedVector2Array, mode: i64) -> i64 {
        let Some(cam) = pick_camera(&camera) else { return 0 };
        self.skip_hidden_layers();
        let poly: Vec<glam::Vec2> = polygon.as_slice().iter().map(|p| glam::Vec2::new(p.x, p.y)).collect();
        let ids = self.pick.in_lasso(&cam, &poly);
        let n = ids.len() as i64;
        self.select(ids, mode);
        n
    }

    /// Whether the frontmost stroke within `tolerance` pixels of `pos` is selected.
    #[func]
    fn selected_at(&mut self, camera: Gd<Camera3D>, pos: Vector2, tolerance: f32) -> bool {
        let Some(cam) = pick_camera(&camera) else { return false };
        self.skip_hidden_layers();
        self.pick.nearest(&cam, glam::Vec2::new(pos.x, pos.y), tolerance).is_some_and(|id| self.selection.contains(&id))
    }

    #[func]
    fn select_all(&mut self) {
        let hidden: Vec<_> = self.doc.layers().iter().filter(|l| !l.visible || l.locked).map(|l| l.id).collect();
        let ids = self.doc.strokes().iter().filter(|s| !hidden.contains(&s.layer)).map(|s| s.id).collect();
        self.set_selection(ids);
    }

    #[func]
    fn deselect(&mut self) {
        self.set_selection(Vec::new());
    }

    #[func]
    fn selection_count(&self) -> i64 {
        self.selection.len() as i64
    }

    /// Removes the selected strokes (one undo step).
    #[func]
    fn delete_selected(&mut self) -> i64 {
        let ids = std::mem::take(&mut self.selection);
        let changes = self.doc.remove_strokes(&ids);
        self.apply(changes);
        self.signals().selection_changed().emit(0);
        ids.len() as i64
    }

    /// Copies the selected strokes, a little to the camera's right, and selects the copies
    /// (one undo step).
    #[func]
    fn duplicate_selected(&mut self, camera: Gd<Camera3D>) -> i64 {
        if self.selection.is_empty() {
            return 0;
        }
        let t = camera.get_global_transform();
        let (lo, hi) = self
            .selection
            .iter()
            .filter_map(|id| self.doc.stroke(*id))
            .flat_map(|s| s.points.iter())
            .fold((glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(p.pos), hi.max(p.pos)));
        let eye = glam::Vec3::new(t.origin.x, t.origin.y, t.origin.z);
        let right = t.basis.col_a();
        let offset = glam::Vec3::new(right.x, right.y, right.z) * eye.distance((lo + hi) * 0.5) * 0.05;
        let (changes, copies) = self.doc.duplicate_strokes(&self.selection, offset);
        self.apply(changes);
        let n = copies.len() as i64;
        self.set_selection(copies);
        n
    }

    /// Re-brushes the selected strokes with the current brush (one undo step).
    #[func]
    fn rebrush_selected(&mut self) -> bool {
        let brush = self.brush.clone();
        let r = self.doc.intern_brush(&brush.def);
        self.compiled.entry(r).or_insert(brush);
        let changes = self.doc.set_brush(&self.selection.clone(), r);
        let changed = !changes.is_empty();
        self.apply(changes);
        changed
    }

    /// The middle of the selected strokes' bounds (the origin when nothing is selected).
    #[func]
    fn selection_center(&self) -> Vector3 {
        let (lo, hi) = self
            .selection
            .iter()
            .filter_map(|id| self.doc.stroke(*id))
            .flat_map(|s| s.points.iter())
            .fold((glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(p.pos), hi.max(p.pos)));
        if lo.x > hi.x {
            return Vector3::ZERO;
        }
        let c = (lo + hi) * 0.5;
        Vector3::new(c.x, c.y, c.z)
    }

    /// Shows the selection moved by `xf` while the gizmo is dragged: only the nodes move, nothing
    /// is rebuilt or recorded. Identity puts them back.
    #[func]
    fn preview_selection_transform(&mut self, xf: Transform3D) {
        for id in &self.selection {
            if let Some(node) = self.nodes.get_mut(id) {
                node.set_transform(xf);
            }
        }
    }

    /// Bakes `xf` into the selected strokes' points (one undo step) and keeps them selected.
    /// Returns false if `xf` doesn't move anything.
    #[func]
    fn commit_selection_transform(&mut self, xf: Transform3D) -> bool {
        self.preview_selection_transform(Transform3D::IDENTITY);
        let a = |v: Vector3| glam::Vec3A::new(v.x, v.y, v.z);
        let affine = glam::Affine3A::from_cols(a(xf.basis.col_a()), a(xf.basis.col_b()), a(xf.basis.col_c()), a(xf.origin));
        if self.selection.is_empty() || affine.abs_diff_eq(glam::Affine3A::IDENTITY, 1e-6) {
            return false;
        }
        let ids = std::mem::take(&mut self.selection);
        let (changes, moved) = self.doc.transform_strokes(&ids, affine);
        self.apply(changes);
        self.set_selection(moved);
        true
    }

    fn skip_hidden_layers(&mut self) {
        let skipped = self.doc.layers().iter().filter(|l| !l.visible || l.locked).map(|l| l.id);
        self.pick.set_skipped_layers(skipped);
    }

    fn select(&mut self, ids: Vec<StrokeId>, mode: i64) {
        let mut next = if mode == 0 { Vec::new() } else { self.selection.clone() };
        for id in ids {
            match mode {
                2 => next.retain(|s| *s != id),
                _ if !next.contains(&id) => next.push(id),
                _ => {}
            }
        }
        self.set_selection(next);
    }

    fn set_selection(&mut self, ids: Vec<StrokeId>) {
        if ids == self.selection {
            return;
        }
        for id in std::mem::take(&mut self.selection) {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.set_material_overlay(Option::<&Gd<Material>>::None);
            }
        }
        self.selection = ids;
        for id in self.selection.clone() {
            self.highlight(id);
        }
        let n = self.selection.len() as i64;
        self.signals().selection_changed().emit(n);
    }

    /// Draws the accent outline over a selected stroke.
    fn highlight(&mut self, id: StrokeId) {
        let Some(brush) = self.doc.stroke(id).map(|s| s.brush) else { return };
        let double_sided = self.brush_for(brush).material().double_sided;
        let overlay = match self.select_overlays.get(&double_sided) {
            Some(m) => m.clone(),
            None => {
                // A pale tint over the stroke (so it shows on strokes the accent's colour),
                // then the accent outline.
                let mut outline = ShaderMaterial::new_gd();
                outline.set_shader(&self.shader(airdraw_look::OUTLINE_SHADER, double_sided));
                outline.set_shader_parameter("outline_color", &SELECTION_COLOR.to_variant());
                outline.set_shader_parameter("outline_width", &3.0f32.to_variant());
                outline.set_shader_parameter("flat_ribbon", &double_sided.to_variant());
                let mut tint_shader = Shader::new_gd();
                tint_shader.set_code(SELECTION_TINT_SHADER);
                let mut tint = ShaderMaterial::new_gd();
                tint.set_shader(&tint_shader);
                tint.set_next_pass(&outline);
                let m: Gd<Material> = tint.upcast();
                self.select_overlays.insert(double_sided, m.clone());
                m
            }
        };
        if let Some(node) = self.nodes.get_mut(&id) {
            node.set_material_overlay(&overlay);
        }
    }

    #[func]
    fn is_erasing(&self) -> bool {
        self.erasing.is_some()
    }

    /// Strokes the pick index holds (tests).
    #[func]
    fn pickable_count(&self) -> i64 {
        self.pick.len() as i64
    }

    #[func]
    pub(crate) fn undo(&mut self) {
        self.cancel_erase();
        let changes = self.doc.undo();
        self.apply(changes);
    }

    #[func]
    pub(crate) fn redo(&mut self) {
        self.cancel_erase();
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
        self.cancel_erase();
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
        let compiled =
            dict_to_def(&def, self.library.registry()).and_then(|d| Brush::compile(d, self.library.registry()).map_err(|e| e.to_string()));
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
        let result =
            dict_to_def(&def, self.library.registry()).and_then(|d| self.library.save(d, &dir).map(|_| ()).map_err(|e| e.to_string()));
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
                let mut files: Vec<_> =
                    std::fs::read_dir(dir.to_string()).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
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

    /// Writes a copy of the sketch to `path` without undo history (deleted strokes are gone
    /// from it) and with its own content id, for sharing. The open sketch, its file and its
    /// history are unchanged.
    /// Returns an empty string on success, else the error message.
    #[func]
    fn save_copy_without_history(&self, path: GString) -> GString {
        // A copy is a sketch of its own: a new content id, so it doesn't share per-device
        // state (camera, tool) with this one.
        let opts = airdraw_doc::SaveOptions { history: false, new_content_id: true, ..Default::default() };
        match self.doc.save_with(Path::new(&path.to_string()), opts) {
            Ok(()) => GString::new(),
            Err(e) => GString::from(&e.to_string()),
        }
    }

    /// File format size/speed table (docs/ARCHITECTURE.md §3h) for the sketches at `paths`,
    /// plus a sketch of at least `points` points tiled from them. For
    /// `tests/file_format_bench.tscn`; unreadable files are listed and skipped.
    #[func]
    fn file_format_benchmark(&self, paths: PackedStringArray, points: i64) -> GString {
        let mut samples = Vec::new();
        let mut skipped = String::new();
        for path in paths.as_slice() {
            let path = path.to_string();
            match Document::load(Path::new(&path)) {
                Ok(d) => samples.push((Path::new(&path).file_name().unwrap_or_default().to_string_lossy().into_owned(), d)),
                Err(e) => skipped.push_str(&format!("skipped {path}: {e}\n")),
            }
        }
        GString::from(&(airdraw_doc::bench::file_format(&samples, points.max(0) as usize) + &skipped))
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
                let files: PackedStringArray = report.files.iter().map(|f| GString::from(dir.join(f).to_string_lossy().as_ref())).collect();
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
                Change::Added(id) => self.index_stroke(id),
                Change::Updated(id) => {
                    self.index_stroke(id);
                    if self.selection.contains(&id) {
                        self.highlight(id);
                    }
                }
                Change::Removed(id) => {
                    self.pick.remove(id);
                    if let Some(i) = self.selection.iter().position(|s| *s == id) {
                        self.selection.remove(i);
                        let n = self.selection.len() as i64;
                        self.signals().selection_changed().emit(n);
                    }
                }
                Change::Guide => {}
                Change::Reset => {
                    if let Some(Erase::Partial(p)) = self.erasing.take() {
                        for (_, piece) in p.pieces {
                            let mut node = piece.node;
                            node.queue_free();
                        }
                    }
                    self.pick.clear();
                    if !self.selection.is_empty() {
                        self.selection.clear();
                        self.signals().selection_changed().emit(0);
                    }
                }
            }
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
                Change::Guide => self.signals().guide_changed().emit(),
                Change::Reset => {
                    self.signals().guide_changed().emit();
                    self.signals().look_changed().emit();
                    for (_, mut node) in self.nodes.drain() {
                        node.queue_free();
                    }
                    // Snapshot indices belong to the old document; its Look may differ.
                    self.compiled.clear();
                    self.materials.clear();
                    self.outlines.clear();
                    self.half_widths.clear();
                    for stroke in self.doc.strokes().to_vec() {
                        let node = self.make_node(&stroke);
                        self.nodes.insert(stroke.id, node);
                        self.index_stroke(stroke.id);
                    }
                }
            }
        }
        let count = self.doc.strokes().len() as i64;
        self.signals().strokes_changed().emit(count);
    }

    /// Adds (or refreshes) a document stroke in the pick index.
    fn index_stroke(&mut self, id: StrokeId) {
        let Some(stroke) = self.doc.stroke(id).cloned() else { return };
        let half_width = self.half_width_for(stroke.brush);
        self.pick.insert(&stroke, half_width);
    }

    /// A brush snapshot's widest half width: its width on a slow, full-pressure test stroke
    /// (pressure and speed only ever thin a line, tapers only shorten it).
    fn half_width_for(&mut self, r: BrushRef) -> f32 {
        if let Some(&w) = self.half_widths.get(&r) {
            return w;
        }
        let brush = self.brush_for(r);
        let probe: Vec<InputPoint> = (0..40)
            .map(|i| InputPoint {
                pos: glam::Vec3::X * (i as f32 * 0.01),
                normal: glam::Vec3::Z,
                pressure: 1.0,
                tilt: None,
                t: i as f64 * 0.05,
            })
            .collect();
        let widest = brush.centerline(&probe).iter().map(|p| p.width).fold(0.0f32, f32::max);
        let w = (widest * 0.5).max(0.001);
        self.half_widths.insert(r, w);
        w
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

    /// The sketch's own copy of its Look (it may not be in this device's library), or the
    /// default Look.
    fn current_look(&self) -> airdraw_look::LookDef {
        self.doc.look().cloned().unwrap_or_else(|| self.looks.get_or_default(None).clone())
    }

    /// The material for a brush snapshot under the current Look: the Look's shader (its unlit
    /// variant for unlit brushes; back faces drawn for double-sided ones) with the brush's
    /// colour, roughness, metallic and emission plus the Look's own parameters as uniforms.
    fn material_for(&mut self, r: BrushRef) -> Gd<Material> {
        if let Some(m) = self.materials.get(&r) {
            return m.clone();
        }
        let d = self.brush_for(r).material().clone();
        let m = self.build_material(&d);
        self.materials.insert(r, m.clone());
        m
    }

    fn build_material(&mut self, d: &MaterialDesc) -> Gd<Material> {
        let look = self.current_look();
        let key = (look.shader_for(d.shading == Shading::Unshaded).to_string(), d.double_sided);
        let shader = self.shader(&key.0, key.1);
        let mut m = ShaderMaterial::new_gd();
        m.set_shader(&shader);
        let [cr, cg, cb, ca] = d.color;
        m.set_shader_parameter("albedo", &Color::from_rgba(cr, cg, cb, ca).to_variant());
        m.set_shader_parameter("roughness", &d.roughness.to_variant());
        m.set_shader_parameter("metallic", &d.metallic.to_variant());
        m.set_shader_parameter("emission_energy", &d.emission.to_variant());
        for (name, value) in &look.params {
            m.set_shader_parameter(name.as_str(), &value.to_variant());
        }
        if let Some(outline) = &look.outline {
            m.set_next_pass(&self.outline_material(outline, d.double_sided));
        }
        m.upcast()
    }

    fn shader(&mut self, name: &str, double_sided: bool) -> Gd<Shader> {
        self.shaders
            .entry((name.to_string(), double_sided))
            .or_insert_with(|| {
                let mut s = Shader::new_gd();
                s.set_code(&airdraw_look::shader_code(name, double_sided).expect("looks are validated"));
                s
            })
            .clone()
    }

    /// The outline pass for the current Look: an inverted hull for tubes and dots, sideways
    /// expansion for flat (double-sided) ribbons.
    fn outline_material(&mut self, outline: &airdraw_look::Outline, double_sided: bool) -> Gd<Material> {
        if let Some(m) = self.outlines.get(&double_sided) {
            return m.clone();
        }
        let mut m = ShaderMaterial::new_gd();
        m.set_shader(&self.shader(airdraw_look::OUTLINE_SHADER, double_sided));
        let [r, g, b] = airdraw_look::parse_color(&outline.color).unwrap_or([0.0; 3]);
        m.set_shader_parameter("outline_color", &Color::from_rgb(r, g, b).to_variant());
        m.set_shader_parameter("outline_width", &outline.width.to_variant());
        m.set_shader_parameter("flat_ribbon", &double_sided.to_variant());
        let m: Gd<Material> = m.upcast();
        self.outlines.insert(double_sided, m.clone());
        m
    }

    /// Re-applies materials to every stroke (after the Look changed).
    fn refresh_materials(&mut self) {
        self.materials.clear();
        self.outlines.clear();
        let nodes: Vec<(StrokeId, Gd<MeshInstance3D>)> = self.nodes.iter().map(|(k, v)| (*k, v.clone())).collect();
        for (id, mut node) in nodes {
            if let Some(brush) = self.doc.stroke(id).map(|s| s.brush) {
                node.set_material_override(&self.material_for(brush));
            }
        }
        if let Some(r) = self.current.as_ref().map(|s| s.brush_ref) {
            let m = self.material_for(r);
            if let Some(s) = &mut self.current {
                s.set_material(&m);
            }
        }
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
    InputPoint { pos: glam::Vec3::new(pos.x, pos.y, pos.z), normal: glam::Vec3::new(normal.x, normal.y, normal.z), pressure, tilt: None, t }
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

/// The camera as picking sees it: world → camera, projection, viewport size in pixels.
pub(crate) fn pick_camera(cam: &Gd<Camera3D>) -> Option<Camera> {
    let size = cam.get_viewport()?.get_visible_rect().size;
    let t = cam.get_global_transform().affine_inverse();
    let a = |v: Vector3| glam::Vec3A::new(v.x, v.y, v.z);
    let view = glam::Affine3A::from_cols(a(t.basis.col_a()), a(t.basis.col_b()), a(t.basis.col_c()), a(t.origin));
    let p = cam.get_camera_projection();
    let c = |v: Vector4| glam::Vec4::new(v.x, v.y, v.z, v.w);
    let proj = glam::Mat4::from_cols(c(p.cols[0]), c(p.cols[1]), c(p.cols[2]), c(p.cols[3]));
    Some(Camera::new(view, proj, glam::Vec2::new(size.x, size.y)))
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

/// A triangle far below a pixel in size, with every attribute strokes have (positions,
/// normals, tangents, UVs, indices): shader pipelines depend on the vertex layout.
fn warm_up_mesh() -> Gd<ArrayMesh> {
    let mut m = MeshBuffers::default();
    let n = glam::Vec3::Z;
    let t = glam::Vec4::new(1.0, 0.0, 0.0, 1.0);
    for p in [glam::Vec3::ZERO, glam::Vec3::new(1e-6, 0.0, 0.0), glam::Vec3::new(0.0, 1e-6, 0.0)] {
        m.push(p, n, glam::Vec2::ZERO, t);
    }
    m.indices.extend_from_slice(&[0, 1, 2]);
    let mut mesh = ArrayMesh::new_gd();
    mesh.add_surface_from_arrays(PrimitiveType::TRIANGLES, &mesh_arrays(&m));
    mesh
}
