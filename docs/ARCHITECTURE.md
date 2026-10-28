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
- **Format 2: embedded thumbnail.** Right after the 8-byte header:
  `u32 length (0 = none)` + PNG bytes, before the metadata. OS thumbnail extensions read only
  the first bytes of a file (check `ADRW`, version ≥ 2, u32 at offset 8, PNG). Format 1
  files still open (fixture `airdraw-doc/tests/fixtures/format1_smoke.airdraw`) and are
  upgraded on save. `Document::set_thumbnail` isn't an edit (no dirty flag, no undo);
  `read_thumbnail(path)` reads just the preview for Recents and thumbnailers.
- **Format 3: drawn guide.** An optional `[guide]` table in the metadata
  (`kind = "plane"` with `origin`/`normal`, or `kind = "sweep"` with `near`, `far`,
  `profile`, `dirs`), mapped in `airdraw-doc` so `airdraw-guide` stays serde-free. The
  version is bumped (unlike the optional `look` key) because a format 2 app would silently
  drop the guide and re-save without it. A mismatched or non-finite guide is reported as a
  damaged file. Format 1–2 files open with no drawn guide. Loading isn't an edit.
- **Format 4 (current): compressed sections** (§3h): `META` + `STRK` in an `airdraw-pack`
  container, zstd, per-section CRC-32 instead of the whole-file Adler-32. Formats 1–3 still
  open and are upgraded on save; `to_bytes_format3` remains for tests and benchmarks.

### 3e. Incremental live strokes (implemented)

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

**As implemented (step 2):**

- Every built-in stage has a native stream next to its batch `apply`/`build`, and the
  batch code stays as the reference implementation. The exception is `Tube::build`, which
  runs through `TubeStream` because of the vertex reorder. Shared math (`EuroState`,
  `catmull_rom`, `WidthStream::width`, `TaperStream::taper`, `dot`) keeps the two paths
  bit-identical.
- `dots` commits dot *k* only when the committed length already counts it (the count formula
  only grows) **and** a committed point lies past it, so its segment and interpolation can't change.
- Geometry streams keep only a small window of committed points (tube/ribbon: from point
  *i − 1*; dots: from the current segment), so memory and copying don't grow with the stroke.
- Measured (`live_stream_cost`, Mac, 20 s stroke, 2 points per update): a streamed update
  costs 0.003–0.015 ms on average (Ink: 0.007 ms against 0.66 ms for a full rebuild), and the
  provisional tail stays at or below 800 vertices (Ink), 81 (Marker), 272 (Ribbon) and 40 (Dots).
- A full `build` costs the same as before for Ink and about 1.8× for Ribbon (0.19 → 0.35 ms
  for a 20 s stroke). Each stream stage writes into a fresh buffer where the batch stages
  worked in place, which shows on brushes with few vertices per point. That only affects
  load, re-brush and export, since live strokes stop calling `build` in step 3. If it ever
  matters, causal stages can get an in-place fast path for whole-input commits.

#### Godot side (`LiveStroke` in `airdraw-godot/src/live.rs`, implemented)

Godot 4 can append surfaces to an `ArrayMesh` but can't grow or edit a surface's index
buffer in place. So committed geometry is uploaded in **sealed chunks**:

- The committed mesh (in Rust, append-only) is cut into chunks of about **4096 vertices**,
  as contiguous triangle ranges. A chunk copies the vertex range its triangles use, so a tube
  ring at the boundary is duplicated once, and its indices are rebased to zero. Triangles are
  flipped for Godot's winding while copying.
- **Sealed chunks** are appended once with `add_surface_from_arrays` to the stroke node's own
  mesh and never uploaded again.
- **Live mesh:** a child node with one small `ArrayMesh`, cleared and refilled every update,
  holding the open (unfilled) chunk and the provisional tail. The tail's first triangles can
  use committed vertices (the last tube ring), so the copied vertex range starts at the
  lowest one they use.
- **Pen up (decision 2, revised 2026-10-27):** the open chunk plus the end of the stroke
  become one more surface, and the live child is removed. The finished stroke is still **one
  node with one mesh**, so undo, re-brush, thumbnails and export code are unchanged. Short
  strokes (under one chunk, most of them) end as one surface; long ones keep one surface per
  chunk. Full consolidation was measured first and dropped: a 20 s Ink stroke (90k vertices)
  took ~10 ms to re-upload as one surface, which drops a frame at pen-up. Undo/redo and
  loading still rebuild strokes as one surface from their raw points.
- **Surface limit:** at Godot's 256 surfaces per mesh, the sealed chunks are merged into one
  surface (a one-time re-upload) and sealing continues. Reaching it takes a stroke of about
  2.6M triangles (`live_stroke_bench.tscn ++ long`).
- Cancel frees the node with its live child.

Per update, the work is now bounded: Rust processes the new points plus the tail, and Godot
copies and uploads at most one chunk plus the tail (about 4.5k vertices) instead of the
whole stroke.

**Results (steps 3–4).** These come from the same `live_stroke_bench`, 20 s spiral, median ms
per update. The bench also checks that the live stroke, the finished stroke and the stroke
rebuilt from the document (undo + redo) are pixel-identical, and they are, with identical
triangle counts.

| Device | Brush | At | Before (step 0) | After | Uploaded vertices | Pen-up before → after |
|---|---|---|---|---|---|---|
| Mac M4 Pro | Ink | 20 s | 4.36 | **0.38** | ≤ 4 377 (of 89 554) | 4.3 → 0.6 ms |
| Mac M4 Pro | Ribbon | 20 s | 1.39 | **0.39** | ≤ 3 724 (of 20 114) | 2.2 → 0.6 ms |
| iPad Pro M5 | Ink | 20 s | 4.71 | **0.67** | ≤ 4 377 (of 89 554) | not measured → 0.5–0.6 ms |
| iPad Pro M5 | Ribbon | 20 s | 3.48 | **0.61** | ≤ 3 724 (of 20 114) | not measured → 0.5 ms |

On iPad, a 20 s Ink stroke now uses 8% of the 120 Hz frame per update instead of 57%, and
the cost stays flat as the stroke grows (0.42 ms at 5 s and at 10 s).

One renderer quirk: with Godot's **Mobile** renderer (used on iPad), a long Ink stroke
finished as chunk surfaces and the same stroke rebuilt as one surface differ by 1–4/255 in a
single channel at a few scattered pixels (86 of 13 002 stroke pixels on iPad, 3 on the Mac
with `--rendering-method mobile`). With Forward+ they are pixel-identical, and the triangle
data is identical in both cases, so this is the Mobile renderer's shading arithmetic, not
geometry. The visible effect is nil: a long stroke may shift by a barely measurable amount
in a few pixels when the document is reopened.

#### Rollout

| Step | Change | Behavior change |
|---|---|---|
| 0 ✅ | Measure today's cost per update on Mac and iPad (`live_stroke_bench`, results above) | none |
| 1 ✅ | `PointStream`/`MeshStream` traits, batch fallback for every stage, `BrushStream`; `Brush::build` and `centerline` = push + finish | none: build and centerline output bit-identical before/after (64 fingerprints over every stage), all tests and the smoke test pass. Build phase ~10% slower until step 2 (batch fallbacks copy held points) |
| 2 ✅ | Native streams for the built-in stages in the table, one at a time | tube vertex order only: against the pre-stream code, 98 of 126 fingerprints are bit-identical and the other 28 (capped tubes) have identical triangles, vertices and centerlines. Split tests compare streams pushed in pieces (1 point at a time, up to 4, up to 40) against the batch stages, and fail on planted bugs |
| 3 ✅ | `LiveStroke` uses `BrushStream` + sealed/live chunks; pen-up seals the rest (chunks kept, see decision 2) | none visible: live, finished and rebuilt strokes are pixel-identical |
| 4 ✅ | Re-measure (step 0 again) on Mac and iPad | per-update cost flat with stroke length (results above) |

#### Acceptance

- **Split invariance (unit tests):** for every built-in brush and stage, random strokes
  pushed in random splits (including 1 point at a time) give a mesh **bit-identical** to one
  `build` call. With predicted points, `preview` equals `build(real ++ predicted)`.
- **Bounded work:** the `live_rebuild_cost` bench gains an incremental mode that feeds a
  20 s stroke at 240 Hz in 120 Hz frames. The worst per-frame time must not grow with
  length: the last second costs ≤ 1.5× the first.
- **Device:** on iPad, frame time during a 30 s stroke stays within the 120 Hz budget, with
  no upward trend (Instruments).
  *Status:* in the 20 s bench on iPad Pro M5, none of 4 800 updates took a frame over
  8.62 ms, with no upward trend. A 30 s hand-drawn stroke under Instruments hasn't been done yet.
- The existing smoke test (draw, undo, re-brush, export) passes unchanged.

#### Decisions (approved 2026-10-27)

1. Chunk size of 4096 vertices (tunable later).
2. ~~Consolidate to one surface at pen-up.~~ Revised after measuring: keep sealed chunks as
   surfaces of the stroke's mesh, so pen-up uploads at most one chunk (see Godot side).
3. Batch fallback for non-streaming stages.
4. Accept the one-time tube vertex-order change.

### 3f. 3D guides (approved 2026-10-27; steps 1–2 implemented)

**Problem.** A pen gives only a 2D position, so every stroke needs a depth. Today
`SketchSession::project` puts every stroke on **one infinite plane** chosen from four
presets. A drawing made that way is flat: a "cardboard cutout" standing in 3D.

**Decision (2026-10-27): follow Feather's guide workflow.** We copy how it works, not its
name, icons, artwork or documentation text. Behavior below is from Feather's own docs
(support.feather.art/docs/3dguide: interface, draw, loft, primitives).

#### How it works (Feather's model)

- **One active guide.** The guide is a translucent surface with grid section lines and an
  orange line marking where it starts. Pen strokes are projected onto it, on either side,
  from any viewpoint. Orbit to see the surface at an angle, then draw on it; that is
  how strokes get real 3D shape.
- **Draw guide.** In guide-draw mode, a pen stroke creates the guide **perpendicular to the
  current view**: the stroke's screen path is swept along the camera's view direction, so
  from where you drew it the guide looks like the line you drew, and from the side it's a
  wall. The camera's field of view affects the result (perspective rays), and three-finger
  swipe changes FOV. Only the pen draws guides (a "finger-pen" setting allows fingers).
- **Bend.** While a guide is active, Bend mode reshapes it: draw a line and the guide bends
  along it, starting from the orange start line. This can be repeated.
- **Loft.** Select two or more existing curves; a preview guide connects them in order. A
  tension slider runs from sharp to smooth; Done or Cancel.
- **Primitives.** Cube, pyramid, sphere and tube, created at the origin. A segment slider
  turns them into cylinders, cones and so on. You place them with the joystick (move,
  rotate, scale), then Done or Cancel.
- **Guide controls.** A close button removes the active guide (undoable). Sliding sideways on
  it sets opacity (0% to nearly opaque); sliding up saves the guide to a reusable **Resources**
  library. Long-pressing a guide with the Select tool selects it (tinted green) so the
  joystick can move, rotate or scale it. Curves behind the guide can't be selected.

#### Model

New pure-Rust crate `airdraw-guide` (no Godot), unit-testable like the brush engine:

```rust
trait Surface {
    /// Nearest hit of a world-space ray: point + surface normal (either side counts).
    fn intersect(&self, ray: Ray) -> Option<Hit>;
    /// Triangles for display (translucent, section lines) and for picking.
    fn mesh(&self) -> MeshBuffers;
}

enum GuideShape {
    /// Default before any guide is drawn: today's plane presets.
    Plane { origin, normal },
    /// A profile curve swept along a direction (draw: the view direction; bend reshapes the profile).
    Sweep { profile: Vec<Vec3>, dir: Vec3, extent: (f32, f32) },
    /// Surface through two or more curves, with tension.
    Loft { curves: Vec<Vec<Vec3>>, tension: f32 },
    /// Cube, pyramid, sphere, tube with a segment count, placed by a transform.
    Primitive { kind, segments: u32, transform: Mat4 },
}

struct Guide { shape: GuideShape, opacity: f32 }
```

- **Draw guide → Sweep:** the guide stroke's smoothed screen path is unprojected at a depth
  (the hit point on the current guide or, if none, a fixed distance in front of the camera);
  each point becomes a line along the camera's view direction (through that point, for
  perspective). Wide extents both ways let you draw anywhere along it.
- **Bend:** the bend stroke is projected onto a plane through the guide's start line and
  replaces the profile's shape from that line on.
- Every projected sample carries the hit **normal** (`InputPoint.normal` exists already), so
  ribbon brushes lie flat on curved guides with no brush changes.

#### Interaction in AirDraw

- **Tool rail:** a **Guide** tool with a bottom context bar: Draw · Bend · Loft · Shapes.
  Sphere and cylinder buttons fold into Shapes. Brush strokes always go onto the active guide.
- **Active-guide controls:** a floating pill with Close (remove, undoable), an opacity slider
  and Save.
- **Joystick:** the Select tool and guide placement share one move/rotate/scale gizmo (the
  same control later moves strokes).
- **Resources:** saved guides are stored in a user library folder (like user brushes), and a
  panel lists them to reuse.

#### Storage and undo

- The active guide is part of the document: `airdraw-doc` gains a `SetGuide` op (create,
  bend, transform, remove), undoable like strokes. `.airdraw` **format 3** stores it in the
  TOML metadata. Format 1–2 files open with the default plane.
- Saved guides are `.guide.toml` files in the user library, shareable like brushes.
- Exports ignore guides.

#### Rollout

| Step | Deliverable |
|---|---|
| 1 ✅ | `airdraw-guide`: `Guide` (plane, sweep), `Sweep::from_view`, ray tests (hits, normals, both sides, edge-on from the drawing view, depth from the side, nearest layer, degenerate input) |
| 2 ✅ | Session projects onto the active guide (a stroke keeps the guide it started on; misses draw nothing; hit normals go into the stroke); rail **Draw guide** button (one-shot, `G` on desktop) with a live orange path; drawn guide shown with section lines and an orange start edge; Remove in the Guide panel; choosing a preset plane replaces the drawn guide; undo/redo via `SetGuide`. The smoke test drives it end to end. |
| 3 ✅ | `.airdraw` format 3 saves the drawn guide; it's shown again on open (smoke test: save, new, reopen). Display (translucent, section lines, orange start line) shipped in step 2. **Opacity:** an Opacity slider in the Guide bar, 0 (hidden, still drawable) to 0.9; default 0.1. A **device setting** (`user://settings.cfg`, `[guide] opacity`), not part of the sketch or `Guide` as sketched above: it's how you like to see guides, and a file must not change because someone viewed it differently (§3h). Applies to preset planes and drawn guides. The Feather-style slide-on-the-close-button gesture waits for the active-guide pill |
| 4 | **Bend** |
| 5 | **Shapes** (cube, pyramid, sphere, tube + segments) with the joystick gizmo |
| 6 | **Loft** (needs curve selection), saved guides + Resources panel |

#### Open details (Feather's docs don't say; proposed defaults)

1. Whether a stroke that misses the guide draws nothing (proposed: nothing, plus a light
   haptic/flash) or falls back to a plane.
2. Whether guide-draw mode returns to the brush after one guide (proposed: yes).
3. Sweep extent (proposed: ±3 m, shown fading at the edges).

### 3g. Looks: how strokes are shaded (approved 2026-10-27 with the proposed defaults; steps 1–3 implemented)

**Goal.** Switch a sketch between cartoon, line-art, clay or realistic rendering. This is
not a colour template: each Look changes how the GPU computes every pixel. The strokes and
their brushes stay the same.

#### How a frame is drawn, and where a Look plugs in

```
mesh ──▶ VERTEX ──▶ raster ──▶ FRAGMENT ──▶ LIGHT (per light) ──▶ framebuffer ──▶ POST ──▶ screen
         position              surface:      how much each light   colour, depth,   whole-image
         (+ outline push)      albedo,       adds: the shading     normals          edges, AO,
                               normal,       model                                  tone mapping
                               roughness
```

| Stage | Realistic (PBR) | Toon / cel | Ink / line art | Clay |
|---|---|---|---|---|
| Vertex | project | project; outline pass pushes along the normal | same as toon | project |
| Fragment | albedo, roughness, metallic (+ texture maps via UVs) | flat albedo; optional halftone/hatch from light level | white fill | grey, matte |
| Light | Cook-Torrance GGX: smooth diffuse, roughness-shaped specular, Fresnel, energy conserving | N·L quantized into 2–3 bands, hard specular blob, rim light on the silhouette | none (unlit) | soft wrap diffuse, no specular |
| Environment | HDR sky lighting (IBL), sun shadows, AO, GI on desktop | one key light, flat ambient, no/soft shadows | none | sky light, strong AO, soft shadows |
| Post | filmic tone map, exposure, light bloom | none, or posterize | none | tone map |
| Outlines | — | inverted hull (black, back faces) | inverted hull, thicker | — |

Planned Looks: **Shaded** (today's look), **Flat** (unlit colour, like marker on paper),
**Toon**, **Ink**, **Clay**, **Realistic**.

#### Data: what strokes need

- Today stroke meshes have positions and normals only: enough for Shaded, Flat, Toon, Ink
  and Clay.
- **UVs** for textures (hatching, paper grain, textured brushes, PBR maps): `u` = distance
  along the stroke (`StrokePoint.dist` already exists), `v` = position around the tube or
  across the ribbon (0..1). Added in each geometry stage and its stream, so live and rebuilt
  strokes stay identical. `MeshBuffers` gains `uvs`; exporters write them (glTF, OBJ, FBX, USD
  all support UVs).
- **Tangents** for normal maps, derived from the stroke direction (the tube's frame already
  has them).

#### Model

```
Brush MaterialDesc (what):  colour, roughness, metallic, emission, shading      ← per stroke, unchanged
Look (how):                 shader family + environment preset + passes          ← per sketch
```

- A Look is a small data file like a brush (`.look.toml`): shader name and parameters
  (ramp bands, rim, outline width/colour, hatch density), environment preset (sky, sun angle,
  shadow, AO, tone map, exposure) and post options. The built-in Looks ship as files;
  user Looks come later.
- `airdraw-godot` builds one Godot `ShaderMaterial` per (Look, brush snapshot), feeding the brush
  values in as uniforms, so a red marker stays red in every Look. Toon and Ink add a second
  material pass (`next_pass`) for the outline. The canvas swaps materials on every stroke
  node when the Look changes; the live stroke uses the same material.
- Shaders live in `app/looks/*.gdshader` and use Godot's `vertex()`, `fragment()` and
  `light()` functions. The toon light function, for example:

```glsl
void light() {
    float ndl = dot(NORMAL, LIGHT);
    float band = smoothstep(0.0, 0.02, ndl - 0.1) * 0.6 + smoothstep(0.0, 0.02, ndl - 0.6) * 0.4;
    DIFFUSE_LIGHT += ALBEDO * LIGHT_COLOR * ATTENUATION * band;
}
```

- The Look is saved in the document (`.airdraw` metadata). It affects the screen, thumbnails
  and PNG/turntable renders; 3D exports keep the brush's PBR material (+ UVs).

#### Shader warm-up (found in step 3)

Custom shaders compile the first time a material variant is drawn, and on iPad that stalled
the first stroke by **0.5–0.8 s** (measured: 840 ms Toon, 441 ms Shaded). Since step 2 moved
every Look, Shaded included, off Godot's precompiled `StandardMaterial3D`, this also affected
the first stroke after opening the app. `SketchCanvas.warm_up_look(camera)` draws one
sub-pixel triangle per variant (lit/unlit × one/two-sided, with outline passes) 0.5 m ahead of
the camera for three frames, with the stroke vertex layout; `main.gd` calls it whenever a Look
is applied (startup, sketch open, switch). On iPad, a never-compiled Look (Clay) now pauses
once at warm-up (837 ms; ~100–140 ms when Godot's shader cache already has it) and the first
stroke frame is 8.6 ms. The triangles are invisible (frames captured during warm-up are
pixel-identical). A cold switch to a new Look therefore still pauses once, at the switch.

#### Platforms

- iPad uses Godot's **Mobile** renderer: custom `light()` functions, outlines, IBL and shadows
  work; screen-space AO, SSIL and SDFGI don't exist there. So on iPad, Realistic = PBR + IBL +
  shadows, and Clay fakes AO with a darker ambient and sky occlusion from the normal.
- Desktop (**Forward+**) gets the full stack: SSAO, SSIL, optional SDFGI, better shadows.
- Cost: outline passes draw each stroke twice (fine at today's sizes; batching finished
  strokes later halves draw calls). Realistic on iPad stays within budget at default shadow
  quality; it's measured with `live_stroke_bench` before shipping.

#### Rollout

| Step | Deliverable |
|---|---|
| 1 ✅ | UVs + tangents in tube, ribbon and dots geometry and streams; Godot meshes carry them; exporters write them. Tube rings and sphere rows gain a seam vertex (u/v wrap cleanly), so tube strokes have ~12% more vertices; the triangles are unchanged (fingerprinted before/after). Tests: tangent unit, ⟂ normal, w = ±1 and agreeing with the UV directions on every triangle (fails 0/624 when the handedness is flipped); tube u = distance, seam closes; split tests now include UVs/tangents. Re-imported: GLB/glTF/FBX (Godot) with UVs + tangents, OBJ/USDA/USDZ (Model I/O) with UVs; PLY writes `s`/`t` (Blender's names) but Model I/O ignores PLY UVs under any name. The C plugin ABI (`AdExportMesh`) is unchanged: plugin UVs need ABI v2. Per-update cost unchanged within noise |
| 2 ✅ | New crate `airdraw-look`: `LookDef` (`.look.toml`: shader, optional unlit-brush shader, float params, environment), validated (known shader, `#RRGGBB` colours, schema); built-in Shaded, Flat, Clay and their shaders embedded like built-in brushes (`airdraw-look/looks`, `airdraw-look/shaders` instead of `app/looks`, so they need no file access on device). Each shader has one `render_mode` line starting `cull_back`; double-sided brushes get a `cull_disabled` variant. The canvas builds one `ShaderMaterial` per brush snapshot with the brush's colour, roughness, metallic and emission as uniforms, compiling each (shader, double-sided) once; changing the Look re-applies materials to every stroke, including the one being drawn. `main.gd` applies the Look's lighting (SSAO only on Forward+). The document saves the Look as an optional metadata key (older apps ignore it; no format bump); it's dirty but not undoable. Picker: the ⋯ menu's Look section. Verified: Shaded through the new pipeline is pixel-identical to the old `StandardMaterial3D` (0 of 1.3M pixels, all four brushes); all Looks render under the Mobile renderer; smoke test picks Clay by touch, checks every stroke's shader and the lighting, and save/reload |
| 3 ✅ | **Toon**: `toon` shader with a custom `light()`: two flat bands with a hard step (`threshold`, `shadow_tone`), a hard-edged highlight, a rim light on the lit side; hard shadows via `ATTENUATION`. **Ink**: `fill` shader (paper white) so the outline draws. **Outline pass** (`outline` shader, a Look's optional `[outline]` colour + width), attached as `next_pass`: tubes and dots are an inverted hull (front faces culled) pushed out along the normal; flat ribbons (double-sided) are pushed sideways along ±binormal (from step 1's tangents) and slightly back in depth. The push is in screen space, so lines keep their width at any distance and scale with the view. Verified on Forward+ and Mobile renderers, all brushes; smoke test picks Toon and Ink by touch and checks every stroke has the outline pass, and Shaded has none |
| 4 | Realistic: PBR + sky lighting + shadows; desktop extras (SSAO, SSIL) |
| 5 | Textured brushes (paper grain, hatching) using the UVs |

#### Decisions (approved 2026-10-27)

1. Looks are per sketch.
2. Exports always use the brush's PBR material (+ UVs).
3. Realistic is desktop-only until measured on iPad.

### 3h. The sketch file: history, compression, nothing view-dependent (proposed 2026-10-27, partly decided)

**Goal.** A `.airdraw` file opens to the same artwork on any device: an artist draws on an
iPad, pushes the file to the team repo, and a teammate on an Android tablet, a Windows ARM
laptop or a Linux x86 box opens the same strokes, guide, Look and undo history. Viewing a
file never changes it. The file stays small enough to autosave every 15 s and to keep in git.

#### What a file holds today (format 3) and what changes

| Kept today | Change |
|---|---|
| Strokes: raw input + layer + brush snapshot (full `BrushDef`, so brushes travel with the file) | keep |
| Layers (name, visible, locked) | keep |
| Look **id** | add the Look's definition, so a device without that Look still renders it |
| Drawn guide | keep |
| Guide opacity | moved to device settings ✅ (a view preference) |
| Embedded PNG thumbnail | fixed angle, only re-rendered when content changes |
| (none) | add undo/redo history |

Not in the file: camera, preset plane, current tool/brush/tray, export settings, and device
preferences (left-handed mode, UI scale, pen and gesture settings, hints). The file never
holds absolute paths, screen pixels or DPI.

#### Container (format 4): sections

Every format bump so far (thumbnail, guide) was needed because the layout was fixed. Format 4
switches to a **section container** so later additions don't need a bump:

```text
"ADRW"  u16 format version (4)  u16 flags
u32 thumbnail length + PNG            (unchanged position: OS thumbnailers keep working)
u32 section count, then per section:
  [u8;4] id  u8 flags (1 = required)  u8 codec (0 = stored, 1 = zstd)
  u64 raw length  u64 stored length  u32 CRC-32 of raw bytes  stored bytes
u32 Adler-32 of everything above
```

| Id | Required | Contents |
|---|---|---|
| `META` | yes | TOML: app version, content id, layers, brush snapshots (including ones only history uses), Look snapshot, guide |
| `STRK` | yes | live strokes, point data (encoding below) |
| `HIST` | no | undo + redo history (below) |

- A reader **skips unknown optional sections** and refuses unknown required ones ("saved by a
  newer AirDraw"). Like PNG's critical/ancillary chunks.
- A damaged optional section (bad CRC, parse error) is dropped with a warning; the sketch
  still opens. A damaged required section is a damaged file, as now.
- Formats 1–3 are still read; saving writes format 4. Quick Look and Files thumbnails don't
  change, since the PNG stays at offset 8.

#### Compression: swappable codecs (`airdraw-pack` + one crate per codec)

`airdraw-pack` owns the container (section framing, checksums) and a **codec interface**; it
depends on no compression library. Each codec is its own crate. `airdraw-doc` only says
*what* goes in each section.

```rust
// airdraw-pack
pub trait Codec: Send + Sync {
    /// Stable id written in each section header. Never reused for a different algorithm.
    fn id(&self) -> CodecId;
    fn name(&self) -> &str;
    fn compress(&self, raw: &[u8], level: Level) -> Result<Vec<u8>, PackError>;
    /// `raw_len` comes from the section header; output longer than it is an error.
    fn decompress(&self, stored: &[u8], raw_len: usize) -> Result<Vec<u8>, PackError>;
}
pub enum Level { Fast, Default, Max }          // each codec maps these to its own settings
pub struct Codecs { /* registry by CodecId */ } // the app registers what it links in
pub struct WriteOptions { pub codec: CodecId, pub level: Level }
```

- **Codec ids:** `0` stored (built into `airdraw-pack`), `1` zstd (`airdraw-codec-zstd`,
  wrapping the `zstd` crate / libzstd). New codecs get new ids and crates; nothing else
  changes.
- **Reading never depends on the writer's choice:** every section names its codec, so a file
  can mix codecs, and switching the default codec later keeps every old file readable as long
  as the old codec crate stays linked. A file using a codec this build lacks gives a clear
  error ("needs codec N, update the app").
- `Level::Fast` for autosave, `Level::Max` for "Save a copy". Levels change only the size,
  never the decoded bytes.
- **Implemented (step 1):** `write`/`read` in `airdraw-pack`; a section that doesn't shrink is
  stored instead. Reads are bounded (`ReadLimits`: 2 GiB per section, 1024 sections), and a
  codec must produce exactly the header's length. The limit alone didn't stop huge
  allocations: zstd's bulk decoder reserved the claimed length (up to 2 GiB) for a frame that
  doesn't record its size, or whose recorded size lies, which on iPad can fail and abort the
  app (a crafted file shared via Files/AirDrop). Since 2026-10-28 the zstd codec checks the
  claim against the frame's recorded size first and decodes into a buffer that grows only
  with real output, stopping one byte past the claim; a test with a counting allocator
  checks a 2 GiB claim reserves under 64 MB (the old decoder reserved all 2 GiB). Open
  speed unchanged (16.1 vs 15.8 ms, file format benchmark). zstd levels: Fast 3, Default 9, Max 19; default features off (no legacy
  decoders). Built for macOS, iOS and Android (NDK clang via `CC_aarch64_linux_android`);
  Windows and Linux builds unverified until those toolchains are set up.
- Tests run the container against the stored codec and a test codec, so the container is
  proven independent of zstd; the zstd crate has its own round-trip and corruption tests.
- **Point encoding before compression (lossless; exact round-trip stays a rule),**
  `airdraw-doc/src/strokes.rs`:
  - One stream per channel across all strokes: x, y, z, normal x/y/z, pressure, tilt, time.
  - Positions, pressure and time: residual from a straight-line prediction
    (`2·prev − prev2`) on the bit patterns, zigzag-coded. Normals and tilt: XOR with the
    previous value. Then byte planes (all lowest bytes, then the next…).
  - Measured per stream on real iPad sketches (zstd fast): the prediction beats plain XOR by
    20% on positions and 45% on time. Pressure is the largest stream and barely compresses
    (12,681 distinct values in 13,465 points): it's sensor noise, and lossless means it
    stays. Shared normals and absent tilt stay omitted, as before.
- **Thumbnail** is already PNG and stays stored.
- **Target**, measured on a large fixture (≥1M points) and on the iPad: file ≥3× smaller
  than format 3; autosave of that sketch under 50 ms on iPad. Fixtures and a benchmark come
  with the crate.

#### Step 2 results (2026-10-27)

`file_format_benchmark` (desktop) and `tests/file_format_bench.tscn` (device:
`BENCH=file-format scripts/bench_ipad.sh`) print the same table from `airdraw_doc::bench`.
Mac (M4 Pro), real sketches copied off the iPad:

| Sketch | Points | Thumbnail | Format 3 | Format 4 fast | Ratio | Without thumbnail |
|---|---|---|---|---|---|---|
| Sketch 1 | 1,746 | 29.7 KB | 73.8 KB | 45.6 KB | 1.6× | 2.8× |
| Sketch 2 | 507 | 7.6 KB | 20.4 KB | 12.2 KB | 1.7× | 2.7× |
| Sketch 3 | 4,204 | 5.3 KB | 108.4 KB | 35.4 KB | 3.1× | 3.4× |
| Sketch 4 | 5,290 | 9.3 KB | 137.5 KB | 44.1 KB | 3.1× | 3.7× |
| Sketch 5 | 1,718 | 3.5 KB | 59.5 KB | 30.4 KB | 2.0× | 2.1× |
| All | | 63 KB | 435 KB | 186 KB | 2.3× | **3.0×** |

- Stroke data meets the 3× target overall (3.4–3.7× on the larger sketches). Whole files
  reach 2.3×: the 512 px PNG thumbnail doesn't compress and is now about a third of a
  format 4 file. Shrinking it (size, palette) is the next lever, separately.
- `Max` gains only 2–6% over `Fast` on real sketches.
- **Times, 1M points** (sample strokes tiled with per-point jitter so no copy repeats; this
  compresses worse than real data, 2.1×, so it's a pessimistic workload), median of 5:
  save format 3 12 ms, **format 4 fast (autosave) 28 ms**, format 4 max 1.2 s, open
  format 3 17 ms, **open format 4 15 ms**.
- **iPad (M5)**, its own sketches, same table: sizes identical; 1M points: save format 3
  12 ms, **format 4 fast 25 ms**, max 0.9 s, open format 3 16 ms, **open format 4 15 ms**.
  Autosave is well under the 50 ms budget on both. An empty sketch grows from 125 to 171
  bytes (section headers), which doesn't matter.

#### Step 3 results (2026-10-27)

- **Look copy:** `META` keeps `look = "<id>"` and adds `[look_def]`, the full definition
  (`airdraw-look`'s `LookDef` is now serializable, with `validate()`). On open: the copy if
  this build can render it (known schema and shaders, valid colours), else the built-in Look
  with that id (a copy from a newer app), else the default. Format 3 files (id only) resolve
  through the library as before. A sketch's Look from another device is listed last in the
  Look menu. Picking a Look stores the library's current version, so re-picking refreshes it.
- **Content id:** 128 random bits (hex) from `RandomState` (OS-seeded, no new dependency),
  created with the sketch and kept across saves and edits. Files without one (formats
  1–3) get one derived from their bytes, stable until they're saved. `canvas.content_id()`.
- **Content hash:** `Document::content_hash()`, BLAKE3 over a versioned prefix, the content
  TOML (layers, used brush snapshots, Look copy, guide) and the encoded strokes. Excludes the
  thumbnail, app version, content id, history and compression level. Written to `META` as
  `content_hash`, so tools can compare sketches by reading only that section. Costs about
  10 ms per 1M points (NEON enabled): **autosave of 1M points is now 38 ms on the Mac and
  36 ms on the iPad**, still under the 50 ms budget.
- **Fixed-angle thumbnail:** rendered from the angle a new sketch opens at (yaw 0.6, pitch
  −0.45), never the user's camera. Smoke test: renders from the top and side views are
  byte-identical. With step 0 (saves only when edited), a viewed file never changes.
- No format bump: everything added is optional `META` keys in format 4.

#### Undo history (`HIST`)

- **Stored:** the full undo **and** redo stacks (≤ 1000 ops), so reopening is the same as
  never having closed the app. Ops: `Add`, `Remove`, `SetBrush`, `SetGuide`, and `Replace`
  (added for Erase and Select, §3i).
- **No duplicated strokes:** strokes in the history refer to strokes by id. A stroke that's
  still in the sketch is stored once (in `STRK`); only strokes that history alone holds
  (deleted ones, undone adds) go into a `HIST` stroke pool, with the same point encoding.
- **Deleted strokes stay in the file** (decided). Anyone given the file can undo back to them.
- **Size cap (revised in step 4):** history's own data (pool + ops, before compression) is
  kept under the **sketch's peak size anywhere in its history**, or 1 MB if larger. The
  first rule (current size) failed its benchmark: after Clear the sketch is empty, so the
  Clear itself was dropped and couldn't be undone after reopening. With the peak, history
  holds as much as the sketch ever did; the cap only bites on long draw-and-delete cycles.
  The newest undo step and nearest redo step are always kept; beyond them, the oldest undo
  steps go first, then the farthest redo steps.
- Loading history isn't an edit: the file opens clean, Undo enabled.
- Brush snapshots used only by history are kept in `META` (saving now drops unused ones).

#### Step 4 results (2026-10-27)

- `airdraw-doc/src/history.rs`: `HIST` is an optional section: TOML for the steps
  (`add`, `remove`, `set_brush`, `set_guide`; strokes referenced as id + brush, since
  re-brushing changes a stroke's brush over time), then a pool of strokes only history holds,
  in the `STRK` encoding. Brushes only history uses follow the live ones in `META`, so live
  numbering and the content hash don't depend on history.
- **Checked on open:** the undo steps are rewound and the redo steps replayed on stroke ids
  (an added stroke must be exactly where it was added, removed ones where they were removed),
  so a damaged history can never make undo misbehave. Anything wrong drops the history and
  opens the sketch. New stroke ids start above every id the history knows.
- `SaveOptions { history: false }` writes without it (step 5 wires the menu).
- **Times, 1M points** (Mac / iPad): autosave 37 / 36 ms; after Clear (history holds every
  stroke) save 28 / 26 ms, open including the history check 17 / 16 ms. The check uses hash
  sets, so it stays linear for a Clear of any size.
- Tests: every undo/redo step after reopening matches the original session; a deleted stroke
  comes back from the file; undoing Clear survives reopening a 3 MB sketch; the cap keeps
  the newest steps; five kinds of bad history (garbage, truncated pool, wrong indices,
  missing stroke, missing brush) open the sketch without history; saving is deterministic.
  App smoke test: clear, save, reopen, undo restores, redo clears.

#### Viewing must never change the file (audit 2026-10-27)

**Rule:** the file holds the artwork and the choices made about it, never how someone looked
at it. No camera is saved (no home view either). A file that is only viewed is never
rewritten.

Audit of everything a file holds or was planned to hold:

| Item | Kind | Verdict |
|---|---|---|
| Strokes, layers (name, visible, locked), brush snapshots | content | keep |
| Undo/redo history | content | keep (format 4) |
| Drawn guide | content (construction geometry, undoable) | keep |
| Look (+ planned snapshot of its definition) | authored appearance, changed on purpose | keep |
| `app_version` | written only with a content save | keep |
| Thumbnail | **rendered from the viewer's camera today**, on every save | keep, but render from a fixed angle (front-left-above, framing all strokes), only when content changed; excluded from the content hash (GPUs differ in a PNG's last bits) |
| Guide opacity | a view preference (dirtied the file) | moved to device settings ✅ |
| Preset plane + offset | tool state | per-device, not in the file |
| Camera, tool, brush, tray | view / tool state | not in the file (planned earlier; dropped) |
| Last export settings | pipeline config | per-device for now; revisit with the export work (a team may want shared engine export settings, as an explicit choice) |

**Bug found by the audit (fixed, step 0):** `save_current()` ran on open-another, + New, going
home and quit **without checking dirty**, re-rendering the thumbnail from the current camera,
so opening a file, orbiting and closing it rewrote it. Now it saves only when dirty, so the
thumbnail changes only with content. The smoke test opens a file, orbits, changes guide
opacity and leaves: same bytes, same modification time.

**Deterministic writer:** the same content always produces the same bytes: no timestamps,
stable key and section order, fixed codec settings per level.

**Content id and hash:** `META` holds a random id created with the sketch; per-device state
(last camera, tool) can be keyed by it, outside the file. A content hash (BLAKE3 of the
decompressed `META` + `STRK`, excluding thumbnail and history) tells "same model" even when
history, thumbnail or codec differ. Copies made with "Save a Copy Without History" get a new
id (`SaveOptions::new_content_id`), so they don't share per-device state with the original;
their content hash still matches. (A file duplicated in the Files app keeps the id: the OS
copies bytes.)

**Forward compatibility of `META`:** the embedded Look (`look_def`) and the drawn guide are
read leniently: a Look with fields this build doesn't know, or a guide kind from a newer app,
reads as absent (the Look falls back to the built-in one with its id, else the default; the
guide is dropped) instead of failing the whole section, which made such sketches unopenable
(found 2026-10-28; `LookDef` is `deny_unknown_fields`). Damage that parses (non-finite guide
numbers) still reports the file as corrupt.

#### "Save a copy without history"

- ⋯ menu → **Save a Copy Without History…**. Writes the same file without `HIST`, so
  deleted strokes are gone for good. The open sketch keeps its history and isn't marked
  edited.
- **Implemented (step 5):** `Document::save_with(path, SaveOptions { history: false, .. })`
  (same temp-file + fsync + rename as saves), `canvas.save_copy_without_history(path)`.
  - iOS: the copy is written to `user://copies/`, then the system **export picker**
    ("Save to Files": choose a folder, `UIDocumentPickerViewController` for exporting, as a
    copy) via `AppFiles.export_file`; `file_exported` / `export_cancelled` delete the
    temporary file. Chosen over the share sheet, which on iPad needs a popover anchor in
    view points (the pixel/point mapping that caused the stroke offset bug); a Share…
    item can come later.
  - Desktop: the native save dialog (Godot's own dialog where there's none), suggesting
    "<name> copy.airdraw" in the Sketches folder.
  - Compression level stays `Fast`: `Max` saves only 2–6% and takes ~1 s per 1M points.
- Also fixed: the ⋯ menu's Look rows were built once at startup, so a Look that came with a
  sketch from another device never appeared there. They're rebuilt each time the menu opens.
- **Exports never carry history:** OBJ, FBX, STL, GLB… are final outputs
  built from the live strokes only.

#### Cross-device sameness

- Rebuilding a stroke must give the same mesh on ARM and x86. Brush stages are plain `f32`
  maths, but `sin`/`cos`/`pow` can differ by an ulp between platform maths libraries.
  Visually identical; meshes may differ in the last bit. Test: build a fixture's meshes on
  macOS ARM and Linux x86 in CI, and compare with a tolerance.
- Teams keeping sketches in git should track `*.airdraw` with Git LFS (binary, compressed).
  Merging two people's edits to one sketch isn't supported.

#### Rollout

| Step | Deliverable |
|---|---|
| 0 ✅ | Fix today's bug: save only when dirty (open-another, + New, home, quit), so no thumbnail re-render without a content change; guide opacity moved to device settings |
| 1 ✅ | `airdraw-pack`: section container, `Codec` trait + registry, stored codec, CRC-32, required/optional sections; `airdraw-codec-zstd`; tests for round-trip, unknown sections and codecs, corruption, truncation, determinism |
| 2 ✅ | Format 4 in `airdraw-doc` on top of it: point encoding (XOR + byte planes); formats 1–3 still read; large-sketch fixture + size/time benchmark (Mac and iPad); Quick Look still works |
| 3 ✅ | Look snapshot and content id in `META`; content hash; guide opacity moved to device settings; fixed-angle thumbnail; smoke test: open, orbit, close leaves the file byte-identical |
| 4 ✅ | `HIST`: history encoding with a shared stroke pool, size cap, damaged history → open without it; smoke test (draw, delete, save, reopen, undo, redo) |
| 5 ✅ | Save a Copy Without History (⋯ menu, system save/share) |

**Decided (2026-10-27):** zstd, behind a swappable codec interface. No camera in the file at
all, and viewing never rewrites a file.
History cap: see the revised rule under Undo history.

### 3i. Erase and Select (approved 2026-10-28 with the proposed defaults; steps 1–5 implemented; scale waits for per-stroke size)

Two tools are still missing from the rail (§5-3): **Erase** and **Select**. Both need the
same thing first: knowing which strokes, and which parts of them, are under the pen. Then
Erase removes them and Select lets you move, rotate, duplicate or delete them.

#### What exists today

- Strokes are world-space raw input (`InputPoint`: position, normal, pressure, tilt, time)
  plus a brush snapshot. There is no per-stroke transform. Meshes are always rebuilt from the
  points, so changing the points is how a stroke changes.
- Undo ops are `Add`, `Remove`, `SetBrush`, `SetGuide`. `HIST` stores stroke points once,
  keyed by stroke id: from `STRK` if the stroke is in the sketch, otherwise from the pool.
- The session already gets `PointerTool::Eraser` (a Wacom eraser end, for example) and
  `PenAction::DoubleTap`/`Squeeze`. Both are ignored for now ("Eraser behaves like the brush
  until erasing is implemented").

#### Picking (`airdraw-pick`, a new crate)

Picking is pure Rust with no Godot dependency, so it can be unit-tested. It is its own crate
because it is its own subsystem (one crate per subsystem).

- **Input:** the camera (view-projection matrix and viewport size), plus each stroke's points
  and its *half width*: the brush's largest half width from its geometry stage. Raw points are
  used instead of the filtered centreline. Filters move points by much less than a stroke's
  width, and raw points need no brush run.
- **Index:** a world AABB per stroke, cached and updated from `Change` events. Per query,
  only strokes whose projected box touches the query region are projected point by point.
  There is no BVH for now; add one only if the benchmark below needs it.
- **Queries, all in screen pixels:**
  - `under_capsule(a, b, radius)`: strokes touched by the eraser moving from sample `a` to
    `b`. Using capsules, not circles, means a fast stroke can't skip a line.
  - `nearest(p, tolerance)`: the frontmost stroke at a tap.
  - `in_lasso(polygon)`: strokes with at least half their points inside.
  - `cut(stroke, capsules)`: the kept parts of a stroke as point ranges with fractional ends.
- **Depth:**
  - The eraser and taps act on the **front** strokes only. At each eraser position, find the
    nearest depth among the strokes under the eraser; only strokes within a small band behind
    it count (the band is a few stroke widths). Otherwise erasing a line on the front of a
    shape would also delete the one behind it. A stroke behind is still erased where the
    eraser passes over it with nothing in front (see step 1 results).
  - Lasso selects **through**, at all depths. You lasso what you see outlined, and you can
    orbit first.

#### Undo: one new op

```rust
Op::Replace { removed: Vec<(usize, Stroke)>, added: Vec<(usize, Stroke)> }
```

One op covers partial erase, transform and duplicate:

| Edit | removed | added |
|---|---|---|
| Partial erase | the cut strokes | their kept pieces |
| Move / rotate / scale | the strokes as they were | the same strokes, moved |
| Duplicate | — | copies |

- Every changed stroke gets a **new id**. Ids are never reused, so `HIST`'s pool-by-id stays
  valid and the file format needs only one new op kind (`op = "replace"`). It is not a new
  format version.
- An older build that meets `replace` drops the history and still opens the sketch; history
  is already optional data.
- Undo of `Replace` is exact: it swaps the stored strokes back. No inverse matrix is
  involved, so repeated undo and redo never drift.
- Whole-stroke erase and Delete keep using `Remove`.
- The selection follows the new ids: the op returns the old → new mapping.
- The history size cap counts both sides of a `Replace`.

#### Erase

- **Whole stroke (default):**
  - Any stroke the eraser touches is hidden at once, so it feels like erasing.
  - Pen-up commits one `Remove` for everything touched by that drag: one undo step.
  - Cancel (a palm or a gesture taking over) shows them again.
- **Partial:**
  - Cuts strokes where the eraser passes. The cut ends are interpolated points at the
    eraser's edge, with position, normal, pressure and time interpolated too.
  - Pieces shorter than 2 points or 1 mm are dropped.
  - While dragging, the original is hidden and its pieces are rebuilt live. Only strokes
    touched by the latest capsule are recomputed. The kept ranges accumulate per stroke.
  - Pen-up commits one `Replace`.
  - Pieces are rebuilt from their own points, so new ends get the brush's taper, like a
    freshly drawn stroke.
- **Size:** in screen points (24 pt by default), so it stays the same on screen when zooming.
  There is a slider in the context bar.
- **Cursor:** a ring at the pen while erasing. On iPads with Pencil hover it is shown before
  touching too.
- **Ways to erase:**
  - Rail button or `E`.
  - Pencil double-tap: Draw ⇄ Erase, following the system's Pencil setting.
  - A pen's eraser end always erases, whatever the tool.
  - Holding `Shift` while erasing switches between whole and partial.
- Hidden and locked layers are never touched. Guides aren't strokes, so the eraser leaves them
  alone (Remove is in the Guide bar).

#### Select

- **Making a selection:**
  - Tap a stroke to select it. Tapping empty space clears the selection.
  - Drag a lasso.
  - `Shift` or a second finger held adds to the selection, `Alt` removes from it.
  - `⌘A` selects all, `Esc` clears.
  - A finger long-press on a stroke selects it and opens its menu, with any tool active.
- **Highlight:** selected strokes get an accent outline, drawn as a `next_pass` with the Ink
  outline shader in the accent colour. It works with every Look and costs no rebuild.
- **Not part of the document:** the selection isn't saved and isn't an undo step. Undo keeps
  whichever selected strokes still exist.
- **Context bar and menu:**
  - Move / Rotate / Scale
  - Duplicate (`⌘D`, the copy is offset a little toward the camera's right)
  - Delete (`⌫`)
  - Re-brush with the current brush: the existing `SetBrush` op, which stands in for Recolour
    until per-stroke colour exists
  - Select All / Deselect

#### Transform ("joystick")

A gizmo sits at the selection's centre, sized in screen points:

| Handle | Drag does |
|---|---|
| Centre pad | Move in the view plane, at the selection's depth |
| Depth bar | Push or pull along the view direction |
| Ring | Rotate about the line of sight to the selection (see step 5 results) |
| Two smaller arcs | Rotate about the world up axis and the view's right axis |
| Corner dot | Uniform scale about the centre |

- **During a drag:** only the stroke nodes' `transform` changes, which is O(1) per stroke with
  no rebuild.
- **On release:**
  - The matrix is baked into each selected stroke's points. Positions are transformed and
    normals rotated.
  - One `Replace` is committed, and the affected meshes are rebuilt.
  - The rebuild is needed because brush geometry isn't guaranteed to be equivariant under
    rotation. For example, a tube's start frame uses a fixed reference axis.
- **Scale and width:** strokes have no size of their own yet, so scaling would change a
  sketch's proportions (lengths grow, line widths don't). Scale should ship with per-stroke
  size, the next roadmap item: scaling then multiplies it. Until then the gizmo has Move and
  Rotate only.
- **Snapping:** with `Shift`, rotation snaps to 15° and moves snap to the grid.

#### Desktop and keyboard

| Input | Action |
|---|---|
| `E` / `V` | Erase / Select |
| Click / drag (Select) | Pick / lasso, with `Shift` to add and `Alt` to remove |
| `⌘A`, `Esc`, `⌘D`, `⌫` | Select all, deselect, duplicate, delete |
| `Shift` while erasing | Whole ⇄ partial |
| Right-click a stroke | Select it and open its menu |

#### Performance targets (iPad, measured with a new `BENCH=erase` scene)

| Case | Target |
|---|---|
| Pick per eraser move, 100k-point sketch | < 1 ms |
| Pick per eraser move, 1M-point sketch | < 4 ms |
| Partial-erase move cutting a 5k-point stroke | < 4 ms including the piece rebuild |
| Transform drag | no rebuilds, 120 fps |
| Release after transforming 50k points | < 50 ms |

#### Rollout

| Step | Deliverable |
|---|---|
| 1 ✅ | `airdraw-pick` (index, capsule, nearest, lasso, cut, front-only depth) with tests; `Op::Replace` in `airdraw-doc` and the `replace` op in `HIST`, with round-trip and older-build tests |
| 2 ✅ | Whole-stroke Erase: rail button, context bar, `E`, eraser end, Pencil double-tap, ring cursor, one undo step per drag; smoke checks |
| 3 ✅ | Partial erase with live pieces; `BENCH=erase` on the iPad |
| 4 ✅ | Select: tap, lasso, accent outline, Delete, Duplicate, Re-brush, Select All, long-press and right-click menus |
| 5 ✅ | Transform gizmo: move and rotate, baked on release; scale follows per-stroke size |

#### Step 1 results (2026-10-28)

- **`airdraw-pick`** (pure Rust, 16 tests, every one checked to fail on a deliberately broken
  version of the code): `PickIndex` with `insert`/`remove`/`set_skipped_layers`,
  `under_capsule`, `nearest`, `in_lasso`, and `cut`. Cut ends land on the eraser's edge
  within 0.02 px, and untouched points are bit-identical to the originals.
- **Front-only, made exact:** the eraser is checked at positions every quarter radius along
  each move, and each stroke also at the position nearest where it touches, so nothing
  between positions is missed. A first version grouped contacts into bins along the move
  instead, and it got one case wrong: swiping across a front line and a visible line just
  behind it spared the second one. Now a line behind is spared only while the front line is
  under the eraser too.
- **`Op::Replace`** with `Document::replace_strokes` (pieces take their stroke's place, one
  undo step) and `Document::duplicate_strokes` (copies on top). `HIST` stores it as
  `op = "replace"` with `removed` and `added` lists. The file checks, round trip,
  determinism, the size cap in both directions, and damaged or unknown ops (what an older
  build sees) all have tests. A bad `replace` drops only the history.
- **Timing (Mac, release build, 1M points in 1,000 strokes):** eraser move 0.41 ms with front
  only, tap 0.19 ms, lasso over 40% of the view 3.5 ms. `cargo test --release -p
  airdraw-pick -- --ignored --nocapture` reruns it. The iPad figures come with `BENCH=erase`
  in step 3.
- Nothing in the app uses it yet (step 2).

#### Step 2 results (2026-10-28)

- **Tool:** Erase sits between Draw and Guide (`E`). Its bar has the eraser size (8–96 pt,
  24 pt by default, a device setting like left-handed mode); dragging the slider previews the
  ring mid-screen.
- **Ways in:**
  - Pencil double-tap: Draw ⇄ Erase. From Erase it goes back to the tool before it.
  - Settings › Apple Pencil "Ignore" turns double-tap off; the backend reads
    `UIPencilInteraction.preferredTapAction`.
  - A tablet pen's eraser end erases with any tool. Godot reports it as `pen_inverted` on
    mouse motion, and the press becomes `PointerTool::Eraser`.
- **Canvas:** the pick index follows every document change: adds, removals, re-brushes and
  loads.
  - A stroke's half width comes from its brush: the widest point of a slow, full-pressure
    probe stroke, cached per snapshot.
  - Touched strokes hide at once and leave the index, so they no longer hide what's behind
    them.
  - On lift, one `Remove` commits everything the drag touched. Cancel, undo, redo or Clear
    mid-drag shows them again.
- **Session:**
  - Eraser drags go to the canvas as capsules between samples, through a pick camera built
    from the `Camera3D`. Its projection matches Godot's `unproject_position` to within 0.5 px
    (smoke check).
  - The ring follows the pen while erasing and while a Pencil hovers. With a mouse it follows
    the pointer over the canvas.
- **Checks:**
  - Smoke test: 3 new lines, 85 in all; the 82 older ones are unchanged. Thumbnail bytes and
    the SVG render's height vary from run to run with or without this change, since mouse
    strokes are timed by the real clock.
  - The router test covers eraser-end strokes.
  - The iPad build is still to be hand-tested.

#### Step 3 results (2026-10-28)

- **Partial erase:** Whole / Partial in the Erase bar, a device setting. Holding Shift when a
  drag starts does the other one.
- **The first time the eraser crosses a stroke:**
  - The stroke is hidden.
  - `airdraw_pick::cut` splits it into pieces. Each piece gets its own node and goes into the
    pick index under a temporary id (from 2⁶²).
- **Later moves:** they cut pieces like any stroke, rebuilding only the piece that was cut.
  Each original remembers its pieces in order along it.
- **On lift:** one `replace_strokes` turns each original into its pieces. All pieces gone
  means the stroke is deleted. It is one undo step.
- **Cancel, undo, redo or Clear mid-drag:** the pieces are dropped and the originals are shown
  again.
- **`BENCH=erase`** (`app/tests/erase_bench.tscn`; `scripts/bench_ipad.sh` runs it on the
  iPad): it measures picking on 100k- and 1M-point sketches, plus partial and whole erase of a
  5,000-point Ink stroke on a real canvas, 20 cuts.
  - Mac (M-series, 1440×900):

| Measure | Mac | iPad Pro M5 (1668×2420) | Target (iPad) |
|---|---|---|---|
| Eraser move, 100k points | 0.03 ms mean, 0.08 ms p95 | 0.03 ms mean, 0.04 ms p95 | < 1 ms ✅ |
| Eraser move, 1M points | 0.40 ms mean, 1.03 ms p95 | 0.28 ms mean, 0.38 ms p95 | < 4 ms ✅ |
| Partial-erase move with the piece rebuild, 5k-point stroke | 1.69 ms mean, 2.80 ms max | 1.89 ms mean, 3.59 ms max | < 4 ms ✅ |
| Lift after 20 cuts (21 strokes built) | 1.97 ms | 3.03 ms | |
| Lasso over 40% of the view, 1M points | 3.5–4.6 ms | 3.1 ms | |

  - Run on the iPad 2026-10-28. Xcode's IPA export step now fails on its own signing
    parameters (`certificateType DEVELOPMENT_MANAGED`, and it picks the Mac as the selected
    device) after the archive succeeds. `deploy_ipad.sh` and `bench_ipad.sh` install the
    archive's signed `.app` instead.
- **Smoke test, 4 new lines:** the line is cut into live pieces with nothing committed, splits
  on lift, is one undo step, and Shift erases whole. The pieces check fails if the pieces are
  never added to the scene.

#### Step 4 results (2026-10-28)

- **Tool:** Select (`V`) sits between Erase and Guide.
  - A pen or mouse stroke under 10 px of travel is a tap: it picks the frontmost stroke within
    14 pt. A tap on nothing clears the selection.
  - A longer stroke is a lasso, drawn in the accent colour while you draw it and closed back
    to its start.
  - Shift adds to the selection and Alt removes, checked when the pen lifts.
  - A pen's eraser end still erases.
- **Highlight:** a `material_overlay` on each selected node. It is a pale tint over the
  stroke, with the outline shader in the accent colour as its next pass. Tinting was added
  after the first try: an accent outline alone vanished on the Ribbon brush, whose blue
  (`#2F6FDE`) is almost the accent.
- **Select bar:** "N selected", Delete, Duplicate, Re-brush, Select All, Deselect.
- **Keys:** `⌫`, `⌘D`, `⇧R`, `⌘A`, `Esc`.
- **Duplicate:** copies go on top, 5% of the distance to the camera towards its right, and
  become the selection. `duplicate_strokes` now takes an offset.
- **Stroke menu:** a right-click without dragging, or a finger held still for 0.5 s, opens
  it, with any tool active.
  - It selects the stroke there, keeping a selection the stroke is already part of, then
    switches to Select and opens the menu at the pointer.
  - Nothing opens while a stroke, erase or lasso is in progress.
  - It shares the ⋯ menu's scrim.
- **In step with the document:**
  - Removed strokes leave the selection.
  - A re-brushed stroke gets its highlight again (it may change between one- and two-sided).
  - Loading clears the selection.
  - The selection isn't saved and isn't an undo step.
- **Smoke test, 3 new lines** (92 in all, the older 82 unchanged):
  - tap, outline, bar and Esc
  - lasso (shown, then cleared), duplicate as one undo step, select all
  - the right-click and long-press menus, Delete from the menu, `⌫`, Re-brush
- **Learned while testing:** a mouse click 4 ms after a touch lifts is dropped by the router
  as the OS's copy of that touch. That is correct, so the test waits.

#### Step 5 results (2026-10-28)

- **Gizmo:** `app/transform_gizmo.gd` is a Control drawn at the selection's centre.
  - It shows with the Select tool while something is selected.
  - Only its handles take input (`_has_point`): the centre pad moves, a bar beside the ring
    pushes and pulls, the ring turns, the top arc turns about world up, and the left arc
    turns about the view's right axis.
  - Shift snaps turns to 15°. Snapping moves to a grid isn't done: the app has no grid step
    yet.
- **The ring turns about the line of sight from the eye to the selection,** not the camera's
  forward axis. About the forward axis, a selection away from the middle of the view leaned
  about 15° under perspective (seen in a screenshot). About the line of sight it turns flat
  around the gizmo.
- **Moving** uses `Camera3D.project_position` at the selection's depth, so the selection
  stays under the pointer. In the smoke test an 80 px drag lands exactly 80 px over.
- **Dragging:** only the selected nodes' transforms change
  (`preview_selection_transform`), so nothing is rebuilt or recorded.
- **On release:** `commit_selection_transform` → `Document::transform_strokes`. Positions are
  transformed and normals rotated, as one `Replace` with new ids. The meshes are rebuilt and
  the moved strokes stay selected.
- **Fixed on the way:**
  - The session claimed every mouse drag with a button held, including ones the GUI started,
    so the gizmo never saw motion. `GodotInput` now tracks the buttons whose press reached
    it (`held`).
  - The gizmo first reported its whole bounding square as UI. That blocked taps and lassos
    near the selection, and since regions update a frame late, the test caught it. It now
    reports small rects per handle (pad, bar, squares along the ring and arcs).
  - A `layout_changed` emitted while the session was dispatching a tap re-entered it. It is
    now deferred.
- **Smoke test:** 1 new line, 93 in all; the older 82 are unchanged. It covers:
  - the handles are UI while the middle of the ring isn't
  - nothing is baked mid-drag and no lasso starts
  - moving lands under the pointer
  - the turn keeps the centre, the line stands upright, the depth bar pushes away, the arc
    turns
  - the gizmo hides without a selection, and undo restores

#### Open details (proposed defaults)

1. **Eraser depth:** front strokes only (proposed) vs everything under the pen.
2. **Lasso rule:** at least half of a stroke's points inside (proposed) vs touching vs fully
   inside.
3. **Default eraser mode:** whole stroke (proposed: fewer surprises, and the cheapest to
   undo) vs partial.
4. **Partial-erase ends:** taper like new strokes (proposed) vs cut flat.
5. **Scale:** wait for per-stroke size (proposed) vs ship now with line widths unchanged.


---

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

- `app/home_screen.gd`: header (app mark, title, outlined **Open**, accent **New Sketch**
  pill), then a **Recent** grid led by a New Sketch card. Cards: square preview with
  rounded corners on paper, name, when opened ("Not found" for moved/deleted files), and a
  ⋯ menu (also long-press/right-click): Open, Show in Files/Finder (`shareddocuments://`
  on iOS), Remove from Recents. Columns follow the width (min card 210 pt: 3 in portrait,
  4–5 in landscape on iPad) and cards stretch so rows fill it; content stops widening at
  1680 pt. Same scaled theme as the canvas.
- Type: Inter (variable, SIL OFL, `app/fonts/`) app-wide via `UiTheme.font(weight,
  display)`; titles use the large optical size. Boot splash and the iOS launch screen show
  `app/splash.png` (the app icon) on the UI background instead of Godot's logo.
- `main.gd` flow: starts on Home (or opens a `.airdraw` passed on the command line, which is
  how Finder launches the Mac app). `+ New` creates `Sketch N.airdraw` in the default folder
  (`Documents/Sketches` on iOS = Files › On My iPad › AirDraw › Sketches; `~/Documents/AirDraw`
  on desktop); a sketch that's never drawn in isn't written. Canvas toolbar has **Gallery**
  (no New/Open/Save). Autosave: every 15 s when dirty, on app pause/focus loss, before quit,
  and when switching documents; each save re-renders the preview.
- `airdraw_doc::Recents` (TOML in user data, max 50, survives corruption). Files inside the
  app's Documents are stored relative (`airdraw-docs://…`) because the iOS container path
  changes on reinstall, and without a bookmark; `AppPaths.store` recognises them however
  the path is spelled (`/private/var/…` from the Files picker, or an earlier install's
  container). Other picked files keep a security-scoped bookmark. Older builds stored
  absolute paths, so each reinstall + reopen added a duplicate; at startup
  `DocumentController.tidy_recents` rewrites those (`Recents::retarget`, newest wins), and
  the home screen shows one card per resolved file.
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
(no free-floating panels); left-handed mode. Later the same day the full-width top bar was
replaced by floating pills so the canvas runs edge to edge, and the app follows the
device's orientation (`display/window/handheld/orientation = sensor`; before this the
iOS export allowed only one landscape orientation).

- `app/editor_shell.gd`: floating pills in the top corners (Gallery + sketch name on the
  left; undo, redo, export, ⋯ menu on the right, both fixed in left-handed mode); tool rail (Brush / Eraser / Select · Plane / Sphere /
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

### 5-3. Drawing screen v2: tools, context bar, sheets (approved 2026-10-27; steps 1–4 implemented)

**Problems with 5-2.** Controls are spread over five places (rail, right panels, view
switcher, two pills), so one task often needs two (a guide is drawn from the rail but removed
in the Guide panel). Six of the rail's buttons are disabled placeholders. The panel column
covers ~20% of the canvas while drawing. F/S/T/3D doesn't show which way you face. Desktop
has hidden shortcuts and no hover, tooltips or right-click.

**Principles.**
1. The canvas is the app: chrome is small, floating, and gets out of the way while drawing.
2. One tool, one place: each tool's options live in one context bar that follows the tool.
3. Same layout on tablet and desktop; input differs (touch + Pencil vs. mouse, keys, hover).
4. Show only what works: unbuilt tools are hidden, not disabled.

#### Layout (all platforms)

```
┌───────────────────────────────────────────────────────────────┐
│ [‹ Sketch 4]                             [↶ ↷  ⤴  ⋯]         │  pills
│                                                     [ cube ]  │  view cube
│ ┌──┐                                                          │
│ │✎ │ Draw     B                                               │
│ │◌ │ Erase    E                                               │
│ │⬚ │ Select   V                                               │
│ │◇ │ Guide    G                                               │
│ └──┘                                                          │
│ ┃ size                                                        │
│ ●●●● recent brushes          ┌──────────────────────────┐     │
│                              │ context bar (tool options)│     │
│                              └──────────────────────────┘     │
└───────────────────────────────────────────────────────────────┘
```

- **Tool rail, four tools:** Draw, Erase, Select, Guide. Erase and Select appear when they're
  built (design: §3i). Left-handed mode mirrors the rail, brush strip and context bar.
- **Context bar** (bottom center, one row, scrolls if narrow):

| Tool | Context bar |
|---|---|
| Draw | current brush (tap: library sheet), colour, size, opacity; later Symmetry, Snap toggles |
| Guide | **Draw · Bend · Shapes · Loft** · presets (Front, Side, Top, View) · opacity · Remove · Save |
| Select | Move / Rotate / Scale (joystick), Duplicate, Delete, Recolour |
| Erase | Whole stroke / Partial, size |

  The Guide card and the plane buttons move here; the Guide panel goes away. A drawn guide
  stays active when you switch back to Draw (you draw on it), and its row stays reachable from
  the Guide tool.
- **Brush quick strip:** the last 4–6 brushes as swatches above the context bar, on the hand
  side, with a vertical size slider. Tap to switch; long-press or right-click to edit or
  duplicate.
- **View cube** (top right, under the action pill), replacing F/S/T/3D. Tap a face or edge
  to snap to that view; drag to orbit; shows orientation at all times. Double-tap = reset view.
- **Sheets instead of fixed panels:** brush library, brush editor, layers, export open as a
  sheet: from the side in landscape (≥ 760 logical px wide), from the bottom in portrait. A
  sheet closes on the next pen-down on the canvas. Desktop windows ≥ 1400 px may pin a sheet
  as a docked column.
- **Toasts** stay where they are, above the context bar.

#### Tablet (touch + Pencil)

| Input | Action |
|---|---|
| Pencil | Use the current tool |
| 1 finger drag | Orbit |
| 2 finger drag / pinch | Pan / zoom |
| 2 finger tap / 3 finger tap | Undo / redo (existing) |
| 4 finger tap | Hide / show all UI |
| Pencil double-tap | Draw ⇄ Erase (Apple's convention; configurable) |
| Pencil squeeze (Pro) | Radial palette at the pen tip: recent brushes, Guide, Erase, Undo |
| Long-press (finger) on stroke / brush / guide | Context menu (select, recolour, edit, remove) |
| Pencil hover (M-series iPad Pro) | Preview dot where the stroke will land on the guide |

- 44 pt minimum targets (existing); context bar and brush strip within thumb reach on the hand side.
- First launch: a short dismissible hint ("Pencil draws, fingers move the view").

#### Desktop (mouse, trackpad, keyboard, pen tablet)

| Input | Action |
|---|---|
| Left drag | Use the current tool |
| Right drag | Orbit |
| Middle drag, or Space + left drag | Pan |
| Wheel / trackpad pinch | Zoom at cursor |
| Alt + wheel | Slide the guide along its normal |
| Right click (no drag) on stroke / guide / canvas | Context menu |
| Pen tablet | Same as Pencil (already routed as a pen) |

| Shortcut | Action |
|---|---|
| B / E / V / G | Draw / Erase / Select / Guide |
| [ / ] | Brush size |
| 1–9 | Recent brushes |
| Cmd/Ctrl + Z, Shift + Cmd/Ctrl + Z | Undo / redo |
| F, S, T, 0 | Front / side / top / reset view |
| Tab | Hide / show UI |
| Cmd/Ctrl + E, Cmd/Ctrl + S | Export / save |

- Every button shows a tooltip with its shortcut ("Guide (G)"). Hover highlights on all
  controls; the cursor reflects the tool. Existing keys that conflict move: guide presets
  leave 1/2/3 (now in the Guide context bar), guide nudge moves from [ ] to Alt + wheel.

#### Implementation notes

- `editor_shell.gd` splits into small pieces: `tool_rail.gd`, `context_bar.gd` (one builder
  per tool), `brush_strip.gd`, `view_cube.gd`, `sheet.gd`. The shell keeps layout and
  `ui_rects()`.
- Tool state moves to one place (`current_tool` in main/session) instead of the rail's button
  group plus `session.guide_drawing`; the context bar and rail both read it.
- Shortcuts and gestures live in one table (`shortcuts.gd`) that also feeds tooltips, so they
  can't drift apart.
- The smoke test finds controls by name, so names stay stable; layout screenshots cover
  landscape, portrait, left-handed and a 1920 px desktop window.

#### Rollout

| Step | Deliverable |
|---|---|
| 1 ✅ | Context bar + tool rail (Draw, Guide); Guide panel folded into the Guide context bar; placeholders hidden |
| 2 ✅ | Brush quick strip; brush library as a sheet (size slider waits for per-stroke size; brush editor stays its own dialog) |
| 3 ✅ | View cube replacing F/S/T/3D |
| 4 ✅ | Desktop: shortcut table + tooltips, mouse map (+ Space-pan), Tab hides UI. Right-click menus wait for Select |
| 5 | Tablet: Pencil double-tap Draw ⇄ Erase and squeeze palette, hover preview (4-finger hide UI and first-run hint done) |

#### Decisions (2026-10-27)

1. Pencil double-tap → Draw ⇄ Erase once the eraser exists; until then it stays undo.
2. Right-drag orbits on desktop.
3. Sheets close on pen-down.
4. Colour (and size) stay part of the brush for now; per-stroke colour/size needs a
   document change and comes as its own step.

#### As built (steps 1–4)

- `editor_shell.gd` (rail, strip, context bars, sheet, toast, menu), `view_cube.gd`,
  `shortcuts.gd`. The planned split of the shell into more files waits until the Select and
  Erase bars exist; today the bars are two short builders.
- Rail: **Draw** and **Guide** only (Erase/Select hidden until built). Guide arms one guide
  stroke, then returns to Draw. Tool state lives in `main.gd` (`tool`), mirrored to
  `session.guide_drawing` and the shell.
- Draw bar: current brush (opens the library sheet) and Edit; while a drawn guide is active,
  "On guide · Remove". Guide bar: Draw, Front/Side/Top/View, position slider + Reset (preset
  planes) or Remove (drawn guide). The old panel column and Layers placeholder are gone.
- Brush strip: 5 most recent brushes (kept in `settings.cfg`), keys 1–5, plus "…" for the
  library. Library sheet fits its content, scrolls past a cap, closes on pen-down
  (`SketchSession.pen_down`).
- View cube: faces Front/Back/Right/Left/Top/Bottom (camera gained back, left and bottom
  views and turns the short way); tap = view, drag = orbit, double-tap on the same face = 3D.
  Straight on, only one face is visible, so arrows mark the hidden neighbours and a tap beside
  the cube turns to the face lying that way (faces turned < 15% toward the camera count as
  hidden).
- **Visual polish (2026-10-27):** light round buttons float along the top (Gallery + sketch
  name left, tools centered, undo/redo/export/menu right); a slim dark tray docked to the
  hand-side edge holds collapse, the recent brushes as nib icons (pen, marker, flat, dots —
  from the brush's geometry and width) with colour dots, and "…"; the brush list opens beside
  it with a nib, name, colour and a live stroke preview per brush (`brush_previews.gd`
  renders each brush's sample stroke offscreen once, cached per definition); the current
  brush row is accent-filled. The context bar, toasts and menu share one slim dark style; the
  view cube is light to match the buttons. `UiTheme` adds `RoundButton`, `TrayButton`,
  `BrushRow`, `TrayLeft/Right`, `Bar`, `Title`, `Small`.
- Bug found by the previews: `dict_to_def` read the filter/dynamics lists only as untyped
  arrays, but `def_to_dict` produces typed ones, so they came back empty. Brush editor
  previews and brushes saved from the editor lost all smoothing and width settings. It now
  accepts both and errors on anything else; the smoke test checks a saved brush keeps them.
- The ⋯ menu is an in-canvas panel with a full-screen scrim, not a `PopupMenu`: Godot popups
  are separate windows that close only on mouse clicks outside, so on iPad (no mouse
  emulation) one stayed open invisibly and blocked all GUI input.
- Desktop: `Shortcuts.KEYS` drives key handling and tooltips (e.g. "Guide (G)", "Undo (⌘Z)");
  right-drag orbits, middle or Space + left drag pans (`Modifiers.space`), wheel zooms,
  Alt + wheel slides the plane. Tab hides the interface.
- Tablet: four-finger tap hides the interface (`Command::ToggleUi`); one-time first-run hint.
- Not yet: right-click / long-press context menus (need stroke picking from Select), squeeze
  palette, hover preview, per-stroke colour/size, a desktop-docked sheet.

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
| 5 | Exporter structure (2026-10-27) | Every exporter is its own crate (`airdraw-export-obj`, `-gltf`, `-fbx`, `-stl`, `-ply`, `-usd`, `-svg`; shared helpers such as the zip writer also separate), wrapping an existing library or handwritten. `airdraw-export` keeps only the scene IR, the `Exporter` trait and the registry. Today's formats are modules in one crate and get split when export work resumes. |
| 6 | Exports and history (2026-10-27) | Exported files (OBJ, FBX, STL, GLB…) are final builds: no undo history. Only `.airdraw` carries it. |

## 9. Continuous integration (approved 2026-10-29; stage 1 written)

GitHub Actions on the public repo (`developer180527/AirDraw`): hosted runners are free,
ARM ones included. One workflow, `.github/workflows/ci.yml`, on pushes to `main` and on
pull requests; a newer push cancels the older run of the same branch.

### 9a. Engine in CI

`engine/` is not in the repo. AirDraw's two engine patches touch only Apple code (iOS app
delegate and app-support folder, macOS open-files), so **desktop and Android jobs use the
official Godot 4.7.2-stable editor and export templates**, downloaded from GitHub
releases and cached. Only the iOS job needs the patched engine: it clones
`4.7.2-stable`, applies `patches/godot/*.patch`, builds the iOS templates
(`scripts/package_ios_templates.sh`) and caches them keyed on the Godot tag plus the
patches' hash, so the 30–40 min build runs only when the patches change.

### 9b. Jobs

| Job | Runner | Steps |
|---|---|---|
| Rust | `ubuntu-24.04` | `cargo fmt --check`, `clippy -D warnings`, workspace tests |
| Linux x86_64 | `ubuntu-24.04` | build GDExtension, Godot import + script check, **smoke test** |
| Linux arm64 | `ubuntu-24.04-arm` | same |
| Windows x86_64 | `windows-2025` | same (tests + build + script check; smoke see 9c) |
| Windows arm64 | `windows-11-arm` | same |
| macOS arm64 | `macos-15` | same, plus the Quick Look extension compiles |
| Android arm64 | `ubuntu-24.04` | `cargo-ndk` build for `arm64-v8a`, export a debug APK (CI-made debug keystore) |
| iOS | `macos-15` | `build_ios_core.sh`, export the Xcode project, `xcodebuild` for a generic iOS device with signing off |

Every platform also runs the workspace tests natively (the Rust job alone wouldn't catch
Windows path or ARM-only bugs). APKs and the unsigned iOS `.app` are kept as run
artifacts for 7 days; no desktop packages yet.

### 9c. The smoke test without a GPU

`draw_smoke_test.tscn` renders (Forward+), so it needs a GPU driver:
- **Linux:** Mesa's software Vulkan (lavapipe) under `xvfb-run`. Known to work with Godot.
- **macOS:** GitHub's Apple-silicon runners expose a paravirtual Metal device; expected to work.
- **Windows:** no GPU and no software Vulkan by default. First try Mesa's prebuilt
  lavapipe for Windows (x64 and arm64 builds exist); if it doesn't hold, the job runs a
  headless subset (load/save, Recents, document checks) instead.
Screenshots (`user://*.png`) upload as artifacts, so layout regressions can be eyeballed
from any run. A smoke line with `=false` that isn't on an allow-list fails the job.

### 9d. Changes needed in the repo

- `app/airdraw.gdextension`: add `linux.arm64`, `windows.arm64`, `android.arm64` entries;
  drop the TEMP debug→release line (CI builds release everywhere).
- Android: an `Android` export preset, and `airdraw-ios`-style platform gating so the
  Android build has no iOS/macOS-only crates (already `cfg`-gated in `airdraw-ffi`).
- Smoke test verdict: `scripts/check_smoke.py LOG` fails on script errors, an unexpected
  `key=false`, or a run that stopped early (done in stage 1).
- `rust/rustfmt.toml` (width 140, compact heuristics) matches the code as written, so
  `cargo fmt --check` can gate CI (done in stage 1).
- Android runs on an emulator and iOS on a simulator need x86_64/simulator builds of the
  engine and core; **build-only for now**, simulator runs later.

### 9e. Order

1. Rust + Linux x86_64 (incl. smoke). 2. macOS, Linux arm64. 3. Windows x64/arm64.
4. Android APK. 5. iOS build with cached templates. Each lands green before the next.

## 10. Efficiency: race to idle (proposed, 2026-10-29)

Goal: when nothing changes on screen, AirDraw does (almost) no work, so the CPU, GPU and
display pipeline can drop to their low-power states; when something does change, it runs at
full rate and finishes fast.

### 10a. Baseline (iPad, M5, 120 Hz; `BENCH=idle` + Instruments Activity Monitor)

`app/tests/idle_bench.gd` runs inside the real app (`bench-idle` launch argument): home
screen, an open sketch left alone, a slow orbit, the sketch left alone again; it logs frames
drawn and loop ticks per second. Instruments gives CPU per process.

| Phase | Frames/s | AirDraw CPU | backboardd (compositor) | audiomxd |
|---|---|---|---|---|
| Home, untouched | **120** | **22 %** | 16 % | 12 % |
| Canvas, untouched | **120** | **22 %** | 16 % | 12 % |
| Canvas, orbiting | 120 | 36 % | 17 % | 12 % |

The app redraws every display refresh forever; the compositor pays for each frame, and an
audio output runs although AirDraw plays no sound. Memory footprint: ~455 MB.

Experiment, low-processor mode on (Godot draws only when the scene changed):

| Phase | Frames/s | AirDraw CPU | backboardd | audiomxd |
|---|---|---|---|---|
| Home, untouched | **0** | **2 %** | 4–5 % | 10 % |
| Canvas, untouched | **0** | **2 %** | 4–5 % | 10 % |
| Canvas, orbiting | 120 | 37 % | 17 % | 12 % |

Nothing in the app forces redraws while idle (Mac shows the same: 0 frames/s idle). The 2 %
left is the loop ticking 120×/s with nothing to do (iOS drives it from CADisplayLink).

### 10b. Plan

1. **Low-processor mode** (`application/run/low_processor_mode`): draw only on change.
   Desktop keeps Godot's 6.9 ms idle sleep (≤ 7 ms extra latency for the first event after
   idle, then full rate).
2. **No audio driver** (`audio/driver/driver = "Dummy"`): frees the audio daemon (~10 %).
   Revisit if AirDraw ever plays sounds.
3. **Redraw only on change** in our own per-frame code: the transform gizmo redraws every
   frame while a selection exists (`_follow` → `queue_redraw` unconditionally); audit the
   eraser ring, hover cursor, view cube and brush previews the same way, and the 1 s macOS
   open-files poll (event-driven where the OS allows).
4. **Engine patch 0003 (iOS): sleep the display link when idle.** After N ticks with no
   input, no scene change and no running tween, pause CADisplayLink; touches, Pencil hover
   and squeeze/tap, keyboard, app-lifecycle and file events resume it in the same run-loop
   turn, so the first frame after idle is no later than today. Expected: 2 % → ~0 %.
5. **Guards:**
   - Pencil latency: `BENCH=live-stroke` before/after must match (drawing changes the
     scene every frame, so it always renders at full rate).
   - The smoke test checks that an untouched home screen and canvas draw 0 frames over a
     second (CI catches anything that starts redrawing forever).
   - `BENCH=idle` stays as the on-device measurement.
6. **Later, measured separately:** the cost of a frame while orbiting/drawing (36 %: finish
   each frame faster: GPU timing isn't exposed on Metal here, so Instruments' Metal trace),
   and the 455 MB footprint.

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
