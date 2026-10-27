# AirDraw Architecture

Cross-platform 3D sketching app (Feather 3D-style) for iPad (first class), Android tablets,
and Windows/macOS/Linux (x64 + ARM64). Built on a source-built Godot 4.7.2-stable with the
core in Rust (godot-rust / GDExtension).

Two principles run through everything:

1. **Platforms collect data, the core makes decisions.** Native code (Swift, Kotlin, desktop)
   only reads hardware and normalizes it. No drawing logic lives in platform code.
2. **Brushes and exporters are pluggable.** Brushes are data files built from registered
   stages; exporters are registered implementations of one trait, including third-party
   C++ exporters loaded through a stable C ABI.

---

## 1. Layers

```
┌──────────────────────────────────────────────────────────────────┐
│ Godot app layer: UI, scenes, camera, brush editor, export dialog │
├──────────────────────────────────────────────────────────────────┤
│ airdraw-godot     Rust glue exposing the core to Godot           │
├─────────────┬─────────────┬─────────────┬────────────────────────┤
│airdraw-brush│ airdraw-doc │airdraw-input│ airdraw-export         │
│brush engine │ strokes,    │ stylus model│ export IR, exporter    │
│+ registry   │ undo, files │ routing     │ registry, built-ins    │
├─────────────┴─────────────┴─────────────┴────────────────────────┤
│ airdraw-platform  Rust traits = the platform contract            │
│ airdraw-ffi       C ABI (airdraw.h, airdraw_export.h)            │
├───────────┬───────────┬───────────┬──────────────────────────────┤
│ iOS       │ Android   │ Desktop   │ Fallback                     │
│ Rust+objc2│ Kotlin    │ Godot ev. │ Godot input events           │
│ UIKit     │MotionEvent│ / WinInk  │ (mouse, anything else)       │
└───────────┴───────────┴───────────┴──────────────────────────────┘
```

Each `airdraw-*` box is a crate in one Cargo workspace. Nothing above the platform contract
knows which OS it runs on. Native layers stay thin (~300 lines each).

```
rust/
  crates/
    airdraw-input/      PenSample, PenEvent, DeviceCaps, InputRouter
    airdraw-platform/   platform traits + null/queue (replay) impls
    airdraw-brush/      stage traits, registry, built-in stages, brush files + library
      brushes/          built-in *.brush.toml (embedded at compile time)
    airdraw-doc/        document model, strokes (raw input + brush snapshot), undo, .airdraw files
    airdraw-export/     ExportScene IR, Exporter trait, registry, built-in exporters
    airdraw-ffi/        exporter plugin ABI: include/airdraw_export.h + loader/adapter
    airdraw-ios/        UIKit input backend (Rust via objc2), compiled only for iOS
    airdraw-godot/      GDExtension cdylib (only crate that depends on `godot`);
                        also hosts the Godot-based fallback backends
platform/
  android/              Kotlin backend (planned)
plugins/
  sample_engine_exporter/  C++ exporter plugin example (CMake)
```

---

## 2. Platform abstraction

### 2a. Unified stylus model (`airdraw-input`)

```rust
struct PenSample {
    pos: Vec2,               // screen px
    t: f64,                  // seconds, monotonic, common clock
    pressure: Option<f32>,   // 0..1
    tilt: Option<f32>,       // altitude, radians
    azimuth: Option<f32>,
    roll: Option<f32>,       // barrel roll (Pencil Pro)
    kind: SampleKind,        // Real | Coalesced | Predicted
}

enum PointerTool { Pen, Eraser, Finger, Mouse }

enum PenEvent {
    Down { id, tool, sample },
    Move { id, samples: SmallVec<PenSample>, predicted: SmallVec<PenSample> },
    Up { id, sample },
    Cancel { id },
    Hover { sample },
    Action(PenAction),       // squeeze, double-tap, barrel button
}

struct DeviceCaps {
    pressure: bool, tilt: bool, azimuth: bool, roll: bool, hover: bool,
    predicted: bool, coalesced: bool, report_rate_hz: u32,
}
```

Missing capabilities are `None`, so brushes degrade gracefully (mouse = no pressure).
`DeviceCaps` lets the UI hide settings that do nothing on the current device.

### 2b. Platform contract (`airdraw-platform`)

| Trait         | Purpose                                | iOS               | Android     | Desktop        |
|---------------|----------------------------------------|-------------------|-------------|----------------|
| `InputSource` | Emit `PenEvent`s, report `DeviceCaps`  | UIKit touches     | MotionEvent | Godot / WinInk |
| `Haptics`     | Snap / squeeze feedback                | Pencil Pro, Taptic| Vibrator    | no-op          |
| `Storage`     | Documents dir, file picker             | Files app         | SAF         | native dialog  |
| `Share`       | Export / share sheet                   | UIActivityVC      | Intent      | save dialog    |
| `DisplayInfo` | Refresh rate, scale, device class      | ProMotion 120 Hz  | Display API | monitor        |

Backends register at startup through `Platform::builder()` (native ones via the C ABI).
Anything unimplemented falls back to a default (null, or Godot-based from `airdraw-godot`), so new platforms work immediately and get native backends incrementally.
`QueueInput` replays recorded strokes for tests and latency benchmarks.

### 2b-1. Godot fallback backend (implemented)

`airdraw-godot/src/godot_input.rs` converts Godot events into `PenEvent`s and pushes them into
the platform's `QueueInput`; `SketchSession` drains it immediately, routes, and drives the
canvas and camera. Godot touch events carry no tool type, so a lone touch is held until its
first drag and classified by tilt (stylus tilted, finger not); a second contact resolves all
waiting touches as fingers. Native backends replace this heuristic with the OS tool type.
Requires `input_devices/pointing/emulate_mouse_from_touch=false`.

### 2b-2. iOS backend (`airdraw-ios`, implemented)

**Decision (2026-10-26): Rust via `objc2` instead of Swift.** A Swift Godot iOS plugin is
linked into the app binary while the Rust core is a framework Godot loads later, so Swift
could only reach Rust through runtime `dlsym` lookups, plus a second language and build
pipeline. With `objc2-ui-kit` the backend lives inside `AirDrawCore.framework` and attaches
when the session starts. Same design: thin, normalizes only.

- `TouchObserver`: a `UIGestureRecognizer` subclass on the root view that never recognizes
  (`cancelsTouchesInView`/`delaysTouchesBegan`/`delaysTouchesEnded` = false; fails when all
  touches end), so Godot still gets every touch for its GUI. Emits `PenEvent`s with the real
  tool type (`UITouchType`), all coalesced samples, predicted samples, pressure
  (`force / maximumPossibleForce`), altitude, azimuth (converted to counter-clockwise) and
  roll (Pencil Pro, iPadOS 17.5+). Positions are points × `contentScaleFactor`, matching
  Godot's viewport pixels; times are `UITouch.timestamp`.
- `PencilHandler`: `UIHoverGestureRecognizer` (hover position/angles) and
  `UIPencilInteraction` (double-tap, squeeze). App mapping for now: double-tap = undo,
  squeeze = next brush.
- `SketchSession` merges native input with a Godot queue (`MergedInput`) for keyboard,
  mouse and trackpad, stops converting Godot touch events, and routes once per frame.
- **UI regions:** native backends see touches the GUI also handles, so the app reports the
  toolbar/dialog rectangles and the router ignores contacts that start inside them.
- **Prediction:** the live stroke is built from real points + the latest predicted points;
  the predicted tail is replaced on every update and dropped when the stroke ends.

### 2c. Threading

Native pen callbacks push into a queue the Rust core drains each frame. On iOS, UIKit and
Godot's loop share the main thread, so the current mutex-backed `QueueInput` is uncontended;
a lock-free SPSC ring buffer replaces it if a backend ever delivers from another thread.

### 2d. Input routing

`InputRouter` maps pointer tool → action using settings:

- Pen → active tool (brush / eraser / select)
- Finger → camera (orbit, pan, pinch zoom), 2-finger tap undo, 3-finger tap redo
- Mouse → button-mapped
- Pencil squeeze / double-tap → user-assigned action
- Palm rejection: finger touches ignored while a pen is down
- Phones without stylus: "finger draws" setting, no code change

---

## 3. Brush system (`airdraw-brush`)

### 3a. Brush = pipeline of registered stages

```
PenSamples → [Filters] → [Dynamics] → [Geometry] → [Material]
             smoothing    pressure→width   tube        solid
             stabilizer   tilt→flatness    ribbon      toon/ink
             pressure     speed→thinning   flat strip  glow
               curve      jitter           dots/dash   texture
             snapping     taper in/out     particles   gradient
```

```rust
trait Filter       { fn process(&mut self, s: &mut Vec<StrokePoint>, p: &Params); }
trait Dynamics     { fn apply(&self, s: &mut [StrokePoint], p: &Params); }
trait Geometry     { fn build(&self, s: &[StrokePoint], p: &Params, out: &mut MeshBuffers); }
trait MaterialSpec { fn describe(&self, p: &Params) -> MaterialDesc; }

registry.register_geometry("tube", Tube::default);
```

Each stage declares a parameter schema (name, type, range, default). The brush editor UI is
generated from these schemas.

### 3b. Brush files (built-ins and custom brushes share one format)

Filters and dynamics are ordered lists (`[[filters]]`, run top to bottom); every stage's
parameters are flat keys next to its `type`. Omitted parameters use the stage's defaults.

```toml
# crates/airdraw-brush/brushes/ink.brush.toml
schema = 1
id = "com.airdraw.ink"
name = "Ink"

[[filters]]
type = "resample"
spacing = 0.003

[[filters]]
type = "one_euro"
min_cutoff = 3.0
beta = 20.0

[[filters]]
type = "spline"
steps = 3

[[dynamics]]
type = "width"
base = 0.012
pressure_curve = [[0.0, 0.25], [1.0, 1.0]]
speed_thinning = 0.2

[[dynamics]]
type = "taper"
start = 0.03
end = 0.06

[geometry]
type = "tube"
sides = 8

[material]
type = "solid"
color = "#1E1E24"
roughness = 0.6
```

**Built-in stages (v1):**

| Slot     | Stages                                           |
|----------|--------------------------------------------------|
| filters  | `resample`, `exponential`, `one_euro`, `spline`  |
| dynamics | `width` (pressure curve, speed thinning), `taper`|
| geometry | `tube`, `ribbon` (flat on the drawing surface), `dots` |
| material | `solid` (PBR + emission), `flat` (unshaded)      |

Still to add for the v1 geometry set: `strip` (camera-facing flat stroke).

- **Built-in brushes** (Ink, Marker, Ribbon, Dots) are embedded in the binary from
  `crates/airdraw-brush/brushes/`, so they work without file access on every platform.
- **Validation:** `Brush::compile` checks stage names and every parameter against the stage
  schema (unknown names, wrong types, bad colors/curves are errors; numbers are clamped).
  Invalid user files are skipped with a warning; others still load.
- **Custom brushes:** `BrushLibrary::save` validates and writes `<id>.brush.toml` to the user
  brushes folder (`user://brushes` in the app); `load_dir` loads them at startup. A user
  brush with a built-in's id replaces it. The brush editor (step 7) sits on top of this.
- **Brush editor (implemented, `app/brush_editor.gd`):** live 3D preview (orthographic, a
  sample S-stroke with a pressure swell), name, ordered filter/dynamics lists (add, move,
  remove), geometry/material pickers, and per-parameter controls generated from stage
  schemas: checkbox, int spin box, float slider + spin box (log scale when min > 0), color
  picker, and `CurveEditor` (drag points, tap to add, double-tap to remove; mouse, touch and
  Pencil). Save (overrides a built-in when editing one; Reset restores it), Save as New,
  Delete, Share, Revert. Godot ↔ `BrushDef` conversion coerces values by schema.
- **Sharing:** Share writes `<id>.brush.toml` to `exports/`; Import installs every
  `*.brush.toml` in `import/` (both visible in the iPad Files app). Packs are plain TOML
  while v1 brushes have no textures; the zip `.brush` pack format comes with textured brushes.
- `schema` version: files newer than the app are rejected with an "update the app" message;
  migrations hook into `BrushDef::from_toml`.
- `Brush::build` is a pure function of the raw input points, which is what makes strokes
  re-editable (3c).

### 3c. Re-editable strokes and the document (`airdraw-doc`, implemented)

A stroke stores **raw input points + layer + a brush snapshot reference**, not the mesh.
Snapshots (full `BrushDef`s) live in a per-document table, deduplicated by content, so every
stroke rebuilds exactly as drawn even after the user edits or deletes the brush. Enables
re-brushing strokes, re-meshing for export/LOD, and deterministic replay.

- **Undo/redo** are reversible ops on document data (`Add`, `Remove`, `SetBrush`; 1000 deep).
  Each edit returns `Change`s (`Added`/`Removed`/`Updated`/`Reset`) and the Godot canvas
  updates only those nodes. Point data is shared (`Arc`) between the document and history.
- **`.airdraw` files:** `"ADRW"` + format version, TOML metadata (app version, layers,
  brush snapshots used by strokes), binary point arrays (shared normal and absent tilt are
  stored once/omitted), Adler-32 checksum. Exact round-trip. Clear errors for non-AirDraw,
  truncated, damaged and newer-version files; a failed open keeps the current document.
- **Safe saves:** write to `<file>.airdraw.tmp`, fsync, rename over the original.
- Unusable snapshots (e.g. from a newer app with extra stages) fall back to the default
  brush with a warning instead of failing the load.
- **Format 2 (current): embedded thumbnail.** Right after the 8-byte header:
  `u32 length (0 = none)` + PNG bytes, before the metadata. OS thumbnail extensions read only
  the first bytes of a file (check `ADRW`, version ≥ 2, u32 at offset 8, PNG). Format 1
  files still open (fixture `airdraw-doc/tests/fixtures/format1_smoke.airdraw`) and are
  upgraded on save. `Document::set_thumbnail` isn't an edit (no dirty flag, no undo);
  `read_thumbnail(path)` reads just the preview for Recents and thumbnailers.

### 3e. Incremental live strokes (proposed, awaiting sign-off)

**Problem.** Every input update rebuilds the stroke being drawn from scratch
(`LiveStroke::rebuild`, `airdraw-godot/src/canvas.rs`). All filters and dynamics re-run over
every point, the whole mesh is regenerated, converted to new `PackedArray`s, wrapped in a new
`ArrayMesh` and uploaded to the GPU. The cost of one update grows with stroke length, so the
cost of drawing a stroke grows with the square of its length.

Measured (2026-10-27, `live_rebuild_cost`, release build, Apple Silicon Mac):

| Brush  | Points (≈ s at 240 Hz) | Vertices | Rust `build` |
|--------|------------------------|----------|--------------|
| Ink    | 2400 (10 s)            | 31 930   | 0.23 ms      |
| Ink    | 4800 (20 s)            | 89 530   | 0.66 ms      |
| Ribbon | 4800 (20 s)            | 20 108   | 0.19 ms      |

The Rust side is cheap. The larger cost is the Godot side: at 90k vertices, each update
copies about 2 MB of positions and normals plus about 2 MB of indices into new arrays,
allocates a mesh and uploads it all. At 120 Hz that is close to 1 GB/s of copying and
uploading, just to append a few points.

**Step 0 results (2026-10-27).** These come from `app/tests/live_stroke_bench.tscn`: a 20 s
spiral fed at 240 Hz, 2 samples per update, through `SketchCanvas.add_points`, with per-phase
timing from `set_record_live_timings`. The iPad numbers come from `scripts/bench_ipad.sh`.
Times are medians per update over that second of stroke, in ms:

| Device | Brush | At | Vertices | build | convert | upload | **total** (max) |
|---|---|---|---|---|---|---|---|
| Mac M4 Pro | Ink | 1 s | 850 | 0.01 | 0.01 | 0.26 | **0.28** (0.50) |
| Mac M4 Pro | Ink | 10 s | 31 954 | 0.24 | 0.32 | 0.98 | **1.58** (3.99) |
| Mac M4 Pro | Ink | 20 s | 89 554 | 0.68 | 0.89 | 2.80 | **4.36** (5.08) |
| iPad Pro M5 | Ink | 1 s | 850 | 0.05 | 0.03 | 0.26 | **0.34** (0.50) |
| iPad Pro M5 | Ink | 10 s | 31 954 | 0.75 | 0.75 | 2.07 | **3.62** (4.14) |
| iPad Pro M5 | Ink | 20 s | 89 554 | 0.71 | 0.82 | 3.19 | **4.71** (5.16) |
| iPad Pro M5 | Ribbon | 20 s | 20 114 | 1.35 | 0.41 | 1.70 | **3.48** (4.13) |

- On the iPad at 20 s, one stroke update takes **57% of the 8.33 ms frame** (120 Hz), and
  the cost keeps growing with length. The app still held 120 Hz in the bench, but that is
  with an empty scene, nothing else on the main thread, and a fresh, cool device.
- About **85% of that cost is Godot-side** (convert + upload), so chunked upload is the
  bigger half of the fix. Making the brush pipeline incremental is still needed, because it
  is what produces a committed prefix that never has to be re-uploaded.
- `end_stroke` (final rebuild + adopting the node) took 4.3 ms on the Mac.
- Not measured: GPU time (Godot's Metal driver reports 0) and thermal throttling over a
  long session. Frame times are display-paced on both devices (vsync can't be turned off
  on Metal here), so they show dropped frames, not headroom.

**Goal.** Per-update cost bounded by *new points + a short tail*, independent of stroke
length, with the finished stroke **identical** to `Brush::build` on the stored raw points
(what load, re-brush and export produce). Otherwise a stroke would visibly change when the
file is reopened.

#### Idea: committed prefix + provisional tail

Every pipeline stage splits its output into:

- **committed**: output that no later input can change. It's appended once and never touched again.
- **provisional**: output that depends on the stroke ending *here*: the last few points, end
  tapers, end caps, snap-to-pen, the predicted Pencil points. It's recomputed on each update and
  thrown away.

A stage keeps a small state for its committed part (the last point, the filter value, the
current frame of the tube). To produce the provisional part it **clones that state**, runs
the tail through the clone and drops the clone. "Pretend the stroke ends now" and "the
stroke really ends" are the same operation, so the live preview at pen-up is exactly the
final stroke.

As implemented in `airdraw-brush/src/stream.rs`:

```rust
/// Streaming form of a filter or dynamics stage. Created per stroke by Filter/Dynamics::stream().
trait PointStream: Send {
    /// Consumes input that will never change; appends output that will never change.
    /// Must be split-invariant: commit(a); commit(b) ≡ commit(a ++ b).
    fn commit(&mut self, input: &[StrokePoint], out: &mut Vec<StrokePoint>);
    /// Output for "the stroke ends after `tail`", from a clone of the state. `&self`: no effect.
    fn finish(&self, tail: &[StrokePoint], out: &mut Vec<StrokePoint>);
}
/// Geometry::stream(): the same shape, appending to MeshBuffers. finish() writes into an empty
/// mesh whose indices count the committed vertices first, then its own.
trait MeshStream: Send { /* commit(&mut self, .., &mut MeshBuffers); finish(&self, .., &mut MeshBuffers) */ }

/// Chains the brush's stage streams (filters → distance pass → dynamics → geometry).
/// Created by Brush::stream(); owned by the live stroke.
struct BrushStream { /* stage streams, committed points, committed MeshBuffers */ }
impl BrushStream {
    fn push(&mut self, real: &[InputPoint]);                         // committed input
    fn committed_mesh(&self) -> &MeshBuffers;                        // append-only
    fn preview(&self, predicted: &[InputPoint]) -> MeshBuffers;      // provisional mesh
    fn mesh(&self, tail: &[InputPoint]) -> MeshBuffers;              // committed + preview
    fn centerline(&self, tail: &[InputPoint]) -> Vec<StrokePoint>;   // for exporters
}
```

The distance pass (`dist` along the stroke, between filters and dynamics) is an internal
stream that is causal from the start.

A stage's provisional output becomes the provisional input of the next stage, which runs it
through its own cloned state. **`Brush::build` itself is rewritten as
`push(all points)` + `finish`**. That leaves one code path, so live and final results match
by construction, provided each stage is split-invariant. Tests enforce that (see
acceptance).

**Fallback for any stage without a streaming version:** a default `stream()` that commits
nothing and runs the existing `apply` on everything in `finish`. Custom or future stages work
on day one with today's cost, and everything downstream of them becomes provisional. Only
the built-ins below need native streams.

#### What each built-in stage holds back

| Stage | Committed when | Held back (provisional) |
|---|---|---|
| `resample` | a point is kept (greedy, depends only on the previous kept point) | the newest kept point (the end rule may drop it) + the final raw point |
| `exponential`, `one_euro` | immediately (causal; state = previous raw/filtered values, speed, time) | only `snap_end` on the last point |
| `spline` | segment *i* once point *i+2* is committed | the last 2 segments and the final point (end clamping) |
| distance (`dist`) | immediately (running sum) | — |
| `width` | immediately (causal speed smoothing) | — |
| `taper` | a point once the stroke is ≥ `start + end` long **and** the point is more than `end` from the tip | points within `end` of the tip; the whole stroke while it's shorter than `start + end` (the `fit` factor still changes) |
| `tube` | ring *i* (+ quads to ring *i−1*) once point *i+1* is committed (tangent needs it); start cap with ring 0 | the last ring and the end cap |
| `ribbon` | strip pair *i* once point *i+1* is committed | the last pair |
| `dots` | dot *k* once a committed point lies at or beyond `k × spacing` | dots past the last committed point |

For Ink, the provisional tail is about `end` (0.06) / `spacing` (0.003) × `steps` (3) ≈ 60
points ≈ 500 vertices, whatever the stroke length. The predicted Pencil points are always
provisional; they never enter committed state.

One visible side effect: tube output moves the start-cap center vertex next to ring 0
instead of after the last ring. Vertex order in exports changes once. Files are unaffected,
since meshes are never stored.

#### Godot side (`LiveStroke` in `canvas.rs`)

Godot 4 can append surfaces to an `ArrayMesh` but can't grow or edit a surface's index
buffer in place. So committed geometry is uploaded in **sealed chunks**:

- The committed mesh (in Rust, append-only) is cut into chunks of about **4096 vertices**,
  as contiguous triangle ranges. A chunk copies the vertex range its triangles use, so a tube
  ring at the boundary is duplicated once, and its indices are rebased to zero. Triangles are
  flipped for Godot's winding while copying.
- **Sealed mesh:** a full chunk is appended once with `add_surface_from_arrays` and never
  uploaded again. At Godot's 256-surface limit (about 1M vertices) a new sealed node starts.
- **Live mesh:** one small `ArrayMesh`, rebuilt every update, holding the open (unfilled)
  chunk and the provisional tail. Its `PackedArray`s are reused, not reallocated.
- **Pen up:** the finished stroke becomes one single-surface mesh from the complete buffers
  (one upload), so documents still have one node and one surface per stroke. That keeps the
  later batching work (merging finished strokes per layer and material) independent of this
  change. Cancel drops everything.

Per update, the work is now bounded: Rust processes the new points plus the tail, and Godot
copies and uploads at most one chunk plus the tail (about 200 KB) instead of the whole stroke.

#### Rollout

| Step | Change | Behavior change |
|---|---|---|
| 0 ✅ | Measure today's cost per update on Mac and iPad (`live_stroke_bench`, results above) | none |
| 1 ✅ | `PointStream`/`MeshStream` traits, batch fallback for every stage, `BrushStream`; `Brush::build` and `centerline` = push + finish | none: build and centerline output bit-identical before/after (64 fingerprints over every stage), all tests and the smoke test pass. Build phase ~10% slower until step 2 (batch fallbacks copy held points) |
| 2 | Native streams for the built-in stages in the table, one at a time | tube vertex order only |
| 3 | `LiveStroke` uses `BrushStream` + sealed/live chunks; pen-up consolidation | none visible |
| 4 | Re-measure (step 0 again) on Mac and iPad | faster |

#### Acceptance

- **Split invariance (unit tests):** for every built-in brush and stage, random strokes
  pushed in random splits (including 1 point at a time) give a mesh **bit-identical** to one
  `build` call. With predicted points, `preview` equals `build(real ++ predicted)`.
- **Bounded work:** the `live_rebuild_cost` bench gains an incremental mode that feeds a
  20 s stroke at 240 Hz in 120 Hz frames. The worst per-frame time must not grow with
  length: the last second costs ≤ 1.5× the first.
- **Device:** on iPad, frame time during a 30 s stroke stays within the 120 Hz budget, with
  no upward trend (Instruments).
- The existing smoke test (draw, undo, re-brush, export) passes unchanged.

#### Decisions needed

1. Chunk size of 4096 vertices (tunable later).
2. Consolidate to one surface at pen-up (proposed), or keep the chunks as they are.
3. Batch fallback for non-streaming stages (proposed), or require every stage to stream.
4. Accept the one-time tube vertex-order change.

## 4. Export system (`airdraw-export`)

### 4a. Flow

```
Document ──▶ ExportScene (IR) ──▶ Exporter (built-in or plugin) ──▶ bytes / files
             re-meshed at export        │
             LOD, options applied       └─ options schema → export dialog UI
```

Exporters never touch the live document or Godot. They receive a neutral, read-only
**ExportScene** built from the document (strokes re-meshed from raw samples at the requested
quality).

### 4b. ExportScene (intermediate representation)

```rust
struct ExportScene {
    meta: SceneMeta,                // units (m), up axis (+Y), handedness, app version
    nodes: Vec<ExportNode>,         // hierarchy: layers/groups → strokes
    meshes: Vec<ExportMesh>,        // positions, normals, uvs, colors, indices, submeshes
    materials: Vec<ExportMaterial>, // PBR-ish: base color, roughness, metallic, emissive, textures
    curves: Vec<ExportCurve>,       // stroke centerlines (+ width/pressure per point)
    textures: Vec<ExportTexture>,
}
struct ExportNode { name, parent: Option<usize>, transform: Mat4, mesh: Option<usize>,
                    curve: Option<usize>, layer: Option<String> }
```

Curves are included so formats/engines that support splines (FBX, USD, custom engines) can
keep strokes as editable curves instead of baked meshes.

### 4c. Exporter trait and registry

```rust
trait Exporter {
    fn info(&self) -> ExporterInfo;          // id, name, extensions, version, capabilities
    fn options(&self) -> OptionsSchema;      // drives the export dialog
    fn export(&self, scene: &ExportScene, opts: &Options, out: &mut dyn ExportSink)
        -> Result<ExportReport, ExportError>;
}

struct ExporterCaps { meshes, curves, materials, textures, hierarchy, multi_file: bool }
```

`ExportSink` abstracts output (single file, folder, in-memory for share sheet) so exporters
never deal with platform file APIs. Common options (scale, axis conversion, triangulate,
merge by layer, mesh quality, include curves) are handled once in IR building.

### 4d. Built-in formats

| Format            | Implementation                         | Notes                               |
|-------------------|----------------------------------------|-------------------------------------|
| OBJ + MTL ✅      | own writer                             | curves as polylines (`l`)           |
| glTF 2.0 / GLB ✅ | own writer (`serde_json`)              | primary format; curves as LINE_STRIP; linear colors, `KHR_materials_unlit` / `emissive_strength` |
| STL ✅            | own writer                             | binary/ASCII, millimeters by default |
| PLY ✅            | own writer                             | binary/ASCII, per-vertex normals + colors |
| USDZ / USDA ✅    | own USDA writer + aligned stored zip   | UsdPreviewSurface, BasisCurves; AR Quick Look |
| FBX ✅            | own binary FBX 7.4 writer              | meshes, Phong materials, layer hierarchy; no curves yet |
| SVG ✅ / PDF      | orthographic projection of curves      | front/top/side, mm, filled variable-width outlines |
| PNG / turntable   | rendered via Godot (app layer)         | image/video, not an IR exporter     |
| `.airdraw`        | native document                        | lossless (raw samples + brushes)    |

**FBX:** no permissively licensed Rust/C writer exists. Plan: write a minimal binary FBX 7.4
writer (meshes, materials, hierarchy, NURBS curves). Alternative: Autodesk FBX SDK as an
optional plugin on desktop only (proprietary license, not on mobile). Decide at FBX milestone.

### 4d-1. Implemented (step 5)

- `airdraw-export`: `build_scene` re-runs each stroke's brush on its raw input and bakes the
  common `SceneOptions` (scale, Y/Z up, merge per layer+material, centerline curves, hidden
  layers). Node tree: layer → stroke (or merged) nodes.
- `Exporter::options()` returns `ParamSpec`s, the same schema type brushes use; the registry
  validates options and warns when the scene holds data the format can't store.
- `ExportSink` implementations: `FolderSink`, `MemorySink` (tests, future share sheet).
- App: Export button / Cmd+E opens a dialog generated from `SceneOptions::schema()` + the
  format's schema; writes to `user://exports/`. On iPad, `UIFileSharingEnabled` +
  `LSSupportsOpeningDocumentsInPlace` expose `exports/` and `documents/` in the Files app.
- Verified: GLB structure tests in Rust, plus re-import through Godot's glTF importer
  (`app/tests/verify_gltf.gd`).

### 4d-2. Step 9 notes

- **Winding convention:** brush geometry is counter-clockwise with outward normals (the
  convention of glTF, USD, FBX, OBJ, STL, PLY), enforced by a geometry test. Godot treats
  clockwise as front-facing, so `airdraw-godot` flips triangles only when building display
  meshes. (Before step 9 the display was inside-out and ribbon exports were reversed.)
- `ExporterCaps::requires_curves` (SVG): the host turns on centerlines automatically.
- Verified beyond unit tests on macOS: Apple Model I/O loads the OBJ/STL/PLY/USDA/USDZ
  exports, Godot's ufbx-based importer loads the FBX, all with identical triangle counts;
  SceneKit renders the USDZ and ThorVG (Godot) renders the SVG
  (`app/tests/verify_modelio.swift`, `render_usdz.swift`, `verify_gltf.gd`).
- Not yet verified: Autodesk FBX SDK / Maya acceptance of the FBX footer constants, and
  AR Quick Look on device. PDF export is not implemented.

### 4e. Exporter plugins (C ABI) — custom engine formats

Third-party exporters (e.g. the user's C++ game engine asset cooker) plug in through a stable
C ABI, so they can be written in C/C++ without knowing Rust or Godot.

```c
// airdraw_export.h  (stable, versioned)
#define AIRDRAW_EXPORT_ABI_VERSION 1

typedef struct {               // read-only views into the IR, valid during export()
    const AdExportNode*     nodes;     uint32_t node_count;
    const AdExportMesh*     meshes;    uint32_t mesh_count;
    const AdExportMaterial* materials; uint32_t material_count;
    const AdExportCurve*    curves;    uint32_t curve_count;
    AdSceneMeta             meta;
} AdExportScene;

typedef struct {
    uint32_t    abi_version;
    const char* id;            // "com.mystudio.engine"
    const char* name;          // "MyEngine Asset"
    const char* extensions;    // "mea"
    const char* options_json;  // option schema → generated dialog
    int  (*export_scene)(const AdExportScene*, const char* options_json,
                         const AdSink* sink, AdExportReport* report);
    void (*destroy)(void);
} AdExporterPlugin;

// Plugin entry point:
const AdExporterPlugin* airdraw_exporter_entry(uint32_t host_abi_version);
```

Rules:
- Data is plain C structs (float arrays, u32 indices, UTF-8 strings); zero-copy views into
  the Rust IR. No C++ types or allocators cross the boundary.
- ABI is versioned; the host rejects incompatible plugins with a clear message.
- Plugin output goes through `AdSink` (write bytes / open sub-file), never raw file paths.
- Errors/warnings returned via `AdExportReport` and shown in the export dialog.

**Integrating a C++ asset cooker** — two supported paths:

1. **Direct (recommended):** wrap the cooker in a small C++ adapter that converts
   `AdExportScene` into the cooker's in-memory model and emits the engine binary. No lossy
   intermediate step; curves and layers are available.
2. **Via intermediate format:** adapter asks the host for FBX/GLB bytes (`host_export_as`
   callback), feeds them to the cooker's existing importer. Fastest to ship if the cooker
   already handles FBX/OBJ well.

**Loading per platform:**

| Platform              | Plugin loading                                                |
|-----------------------|---------------------------------------------------------------|
| Windows/macOS/Linux   | Dynamic: `.dll/.dylib/.so` from the user plugins folder        |
| iPad / iOS            | Static only (App Store forbids loading downloaded code): plugin compiled into a custom build behind a build flag |
| Android               | Static recommended; dynamic `.so` possible for sideloaded builds |

Dynamic plugins are native code with full access, so they are opt-in, listed in settings, and
loaded only from the user plugins folder.

Rust built-in exporters implement the same trait; in-repo C++ plugins can also be linked
statically via `airdraw-ffi`, so one plugin source works on every platform.

**Implemented (step 8):**

- `rust/crates/airdraw-ffi/include/airdraw_export.h`: the ABI as specified above, plus
  `AdHost.warn`, `AD_CAP_*` flags and `AIRDRAW_PLUGIN_EXPORT`. `export_scene` reports failure
  through its return value and an error buffer; `destroy` is called before unloading.
- `airdraw-ffi`: `abi.rs` mirrors the header; `PluginExporter` adapts a plugin to the
  `Exporter` trait. Scene data is passed zero-copy (`glam::Vec3` arrays as `AdVec3*`);
  options go out as a JSON object after host validation; `options_json` becomes
  `ParamSpec`s, so plugins get the generated export dialog. Host callbacks catch panics;
  `AdSink.write` only accepts bare file names (no `/`, `\`, `..`). `AdHost.export_as`
  runs a built-in exporter in memory (the "via FBX/GLB bytes" integration path).
- Loading: `load_dir` (desktop, `libloading`) for `user://plugins`; `static_plugins.rs` in
  `airdraw-godot` registers linked-in entry points (the iOS path).
- Sample: `plugins/sample_engine_exporter/` (C++17, CMake, no dependencies) writes a compact
  `.sem` engine mesh (16-bit indices when possible, units scale, optional normals) and can
  save the host-generated GLB alongside, demonstrating both cooker integration paths.
  `scripts/build_sample_plugin.sh [dest]` builds and installs it.
- Tests: header/Rust layout check (compiles C against the header and compares every size
  and offset), a Rust plugin through the C ABI (scene, options, `export_as`, warnings,
  errors, path confinement), and the C++ sample compiled with `-Wall -Werror`, loaded with
  `dlopen` and its `.sem` output parsed back.
- Still open: per-plugin opt-in UI (desktop currently loads everything in the plugins
  folder) and a `cc`-crate build step for statically linked C++ plugins on iOS.

---

## 5. App shell: home screen and files

Decisions (2026-10-27):

- **No in-app file manager.** Folders, moving, renaming and deleting belong to the OS (Files
  app, Finder, Explorer, Android's document UI).
- **Home screen = launcher:** `+ New`, `Open…` (system file picker), and **Recents** — a
  history of opened sketches with thumbnails (long-press: Remove from Recents, Show in
  Files/Finder; missing files show "Not found"). Settings.
- **File actions only on the home screen.** The canvas has a Gallery button and autosaves.
- Sketches are `.airdraw` files wherever the user keeps them. New sketches autosave to an
  `AirDraw` folder in Documents until first saved elsewhere. iPad keeps access through
  security-scoped bookmarks; Android through persisted URI permissions; desktop via paths.
- App screens are Godot GUI; OS integration (pickers, bookmarks, share sheet, Show in
  Files) goes through the platform layer's `Storage`/`Share` traits.

### 5-1. Implemented home screen and app flow

- `app/home_screen.gd`: `+ New`, `Open…`, Recents grid (embedded thumbnail, name, when
  opened; "Not found" for moved/deleted files). Tap opens; long-press/right-click: Remove
  from Recents, Show in Files/Finder (`shareddocuments://` on iOS). Same scaled theme as
  the canvas.
- `main.gd` flow: starts on Home (or opens a `.airdraw` passed on the command line, which is
  how Finder launches the Mac app). `+ New` creates `Sketch N.airdraw` in the default folder
  (`Documents/Sketches` on iOS = Files › On My iPad › AirDraw › Sketches; `~/Documents/AirDraw`
  on desktop); a sketch that's never drawn in isn't written. Canvas toolbar has **Gallery**
  (no New/Open/Save). Autosave: every 15 s when dirty, on app pause/focus loss, before quit,
  and when switching documents; each save re-renders the preview.
- `airdraw_doc::Recents` (TOML in user data, max 50, survives corruption). Files inside the
  app's own folder are stored as `user://` paths because the iOS container path changes on
  reinstall; picked files keep a security-scoped bookmark.
- `AppFiles` (Rust Godot node): native iOS picker (`UIDocumentPickerViewController`, opens
  originals in place) with bookmarks and access release (`airdraw-ios/src/files.rs`);
  desktop uses Godot's native file dialog. Recents API for GDScript.
- macOS: templates built from source (`scripts/package_macos_templates.sh`, arm64 for now),
  `macOS` export preset → `build/macos/AirDraw.app`. Verified: macOS resolves `.airdraw` to
  "AirDraw Sketch" and opens it with AirDraw.app.
- Opening sketches from other apps: iOS via an `AirDrawOpenURLService` registered with
  Godot's app delegate, plus launch URLs kept by engine patch `0001`; macOS via engine patch
  `0002`, which queues files opened while running in `ProjectSettings` metadata
  (`airdraw_open_files`) instead of starting a second instance; `main.gd` takes them on
  activation (and polls once a second).
- Known gaps: no Settings screen yet.

### 5-2. Drawing screen layout (implemented)

Decisions (2026-10-27): dark chrome around a light canvas; fixed right panel column for v1
(no free-floating panels); left-handed mode.

- `app/editor_shell.gd`: top bar; tool rail (Brush / Eraser / Select · Plane / Sphere /
  Cylinder · Symmetry / Snap — unbuilt tools disabled with "coming soon"); right column of
  collapsible cards (Brushes, Guide, Layers; shown/hidden from the ⋯ menu); portrait or
  narrow windows (< 760 logical px) get a tabbed bottom sheet instead; view switcher
  (front/side/top/3D, animated); toasts replace the old info line. Left-handed mode swaps
  rail and panel sides; it and panel visibility persist in `user://settings.cfg`.
- `app/ui_theme.gd` (dark palette, scale from viewport, 44 pt touch targets) and
  `app/icons.gd` (inline SVG icons rendered at the UI scale) are shared by the shell, dialogs,
  brush editor and home screen.
- The shell reports its rectangles to the input router (`ui_rects()`), so strokes never
  start under chrome. On desktop, `SketchSession` takes motion of an already-pressed pointer
  in `_input`, so a stroke continues when it passes under a panel.

### 5a. File thumbnails in the OS

| OS | Mechanism | Status |
|----|-----------|--------|
| iPadOS + macOS | Exported UTType `com.venugopal.airdraw.sketch` (permanent once shipped) + document types in Info.plist (✅, `scripts/airdraw_document_types.plist.xml`, in both export presets); Quick Look **Thumbnail Extension** (`QLThumbnailProvider`, Swift, ~40 lines) that reads the embedded PNG. Added to our source-built iOS Xcode template; macOS post-export script embeds and signs the `.appex`. | format ✅, rest planned |
| Windows | `IThumbnailProvider` shell extension (Rust `windows` crate), registered by installer | planned |
| Linux | freedesktop `.thumbnailer` + tiny CLI extracting the PNG + MIME type | planned |
| Android | no system thumbnails for custom types; in-app Recents only | — |

Thumbnail rendering (✅): `app/thumbnail_renderer.gd`, a 512×512 offscreen `SubViewport`
sharing the canvas world; its orthographic camera sees only render layer 20 (stroke nodes
are on layers 1 + 20), so no grid or guide appears. It looks along the user's current view
direction, fitted to the strokes' bounding box. Rendered on every save; empty sketches save
without a preview.

Build order: (1) format 2 ✅ → (2) offscreen thumbnail rendering on save ✅ → (3) file type ✅
declarations → (4) Swift thumbnail extension (iPad + Mac) → (5) verify in Files and Finder.
Note: Files/Finder cache thumbnails; updated previews can take a moment to appear.

### 3d. Later (optional): scripted stages

WASM or Rhai sandboxed stages for behavior beyond composing built-ins, with a per-frame time
budget. Deferred until stage composition proves insufficient.

---

## 6. Implementation order

| Step | Deliverable                                                                  | Proves                    |
|------|------------------------------------------------------------------------------|---------------------------|
| 1 ✅ | Cargo workspace split; input types, platform traits                          | Contract                  |
| 2 ✅ | Godot fallback backend + `InputRouter`; desktop migrated                     | Abstraction w/o native    |
| 3 ✅ | Brush stage traits + registry; tube → stages; Ink/Marker/Ribbon/Dots as TOML  | Brushes as data           |
| 4 ✅ | Stroke model (raw samples), save/load `.airdraw`                             | Re-editable strokes       |
| 5 ✅ | Export IR + registry; OBJ and GLB exporters; export dialog from schema        | Export pipeline           |
| 6 ✅ | iOS backend (Rust/objc2 → queue), predicted tail, hover, squeeze, UI regions | Feather-level latency     |
| 7 ✅ | Brush editor (stage pickers, curves, preview, save, `.brush` import/export)   | Custom brushes            |
| 8 ✅ | C ABI exporter plugins + sample C++ plugin; desktop dynamic loading           | Custom engine export      |
| 9 ✅ | STL, PLY, USDZ, FBX, SVG exporters                                           | Format breadth            |
| 10   | Android backend (Kotlin)                                                     | Second native platform    |
| 11 (proposed) | Incremental live strokes (3e): streaming stages, sealed mesh chunks  | Flat cost on long strokes |

---

## 7. Decisions

| # | Topic                      | Decision (2026-10-26)                                              |
|---|----------------------------|--------------------------------------------------------------------|
| 1 | Brush scope v1             | Geometry brushes only (tube, ribbon, strip, dots). Stamp/textured brushes post-v1. |
| 2 | Custom brush depth v1      | Stage composition only. Scripted stages (WASM/Rhai) post-v1.        |
| 3 | FBX                        | Own minimal binary FBX 7.4 writer (portable, no Autodesk SDK).      |
| 4 | Engine cooker integration  | Direct C++ adapter over `AdExportScene`; FBX/GLB bytes path remains available as fallback. |

## 8. Current state (prototype)

- Godot 4.7.2-stable built from source: macOS arm64 editor, iOS templates (Metal only,
  `vulkan=no`, MoltenVK refs stripped by `scripts/package_ios_templates.sh`).
- Rust workspace `rust/`: `airdraw-input` (stylus model), `airdraw-platform` (contract),
  `airdraw-brush` (stage-based brush engine, 4 built-in brushes, user brush folder),
  `airdraw-godot` (`SketchCanvas`, `SketchSession`, builds `libairdraw_godot`).
- On-screen toolbar (`app/toolbar.gd`): brushes (rebuilt when the library changes),
  undo/redo, guide planes, new/open/save/export, clear. Scales with screen size (44pt targets).
- `airdraw-doc`: document with layers, strokes (raw input + brush snapshot), undoable
  add/remove/clear/re-brush, `.airdraw` save/load. Desktop shortcuts: Cmd+S/O/N to a single
  working file (`user://documents/sketch.airdraw`) until platform file pickers exist.
- iPad build exports, signs and runs via `scripts/deploy_ipad.sh` with the native UIKit
  input backend (confirmed active on device via console log). Latency not yet measured.
- `app/airdraw.gdextension` has a TEMP `macos.debug` → release lib switch for latency tests.
