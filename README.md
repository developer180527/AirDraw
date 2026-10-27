# AirDraw

A 3D sketching app in the spirit of Feather 3D — draw strokes in 3D space with a pen, finger
or mouse — built to run on **iPad first**, then macOS, Windows, Linux (x64 + ARM64) and
Android tablets, at a low price.

**Status: early prototype.** Drawing, brushes, documents, export, a home screen and the iPad
Pencil backend work and are tested on an iPad Pro (M5) and an Apple Silicon Mac. Many
drawing features users expect from Feather are not built yet, and only iPad and macOS
builds exist. See [Current state](#current-state).

Design and decisions: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## How it's built

| Layer | Technology | Where |
|---|---|---|
| Engine | Godot **4.7.2-stable**, built from source with two small AirDraw patches | `engine/godot` (not in git), `patches/godot/` |
| Core (input model, brushes, documents, export, plugin ABI) | Rust workspace, exposed to Godot via godot-rust (GDExtension) | `rust/crates/` |
| iPad input, file picker, open-from-Files | Rust with `objc2` (UIKit), inside the same library | `rust/crates/airdraw-ios` |
| App UI (home screen, editor shell, dialogs, brush editor) | Godot GUI in GDScript (dark theme, inline SVG icons) | `app/` |
| Quick Look thumbnails (Files / Finder) | Swift app extension | `platform/apple/AirDrawThumbnail` |
| Exporter plugins | C ABI (`airdraw_export.h`), sample in C++ | `rust/crates/airdraw-ffi/include`, `plugins/` |

Rust crates:

- `airdraw-input` — unified pen/touch/mouse events and the input router (pen draws, fingers
  move the camera, palm rejection, 2/3-finger tap undo/redo, UI regions).
- `airdraw-platform` — platform traits (input, haptics, storage, share, display) and defaults.
- `airdraw-ios` — UIKit backend: real tool type, coalesced + predicted Pencil samples,
  pressure/tilt/azimuth/roll, hover, squeeze/double-tap, document picker, bookmarks.
- `airdraw-brush` — brushes as data (`.brush.toml`): filters → dynamics → geometry → material,
  built-in Ink / Marker / Ribbon / Dots, user brushes, validation.
- `airdraw-doc` — document model (raw input + brush snapshot per stroke), undo/redo,
  `.airdraw` file format (v2, embedded PNG preview), Recents list.
- `airdraw-export` — export scene + exporter registry; GLB, glTF, FBX (binary 7.4, own
  writer), OBJ, USDZ, USDA, STL, PLY, SVG.
- `airdraw-ffi` — exporter plugin ABI and loader (desktop dynamic, iOS static).
- `airdraw-godot` — the GDExtension: `SketchCanvas`, `SketchSession`, `AppFiles`.

## Current state

### Works (tested)

- **Drawing screen:** dark floating chrome over a full-screen light canvas — two pills in
  the top corners (Gallery + sketch name; undo/redo, export, ⋯ menu), tool rail, fixed right column of collapsible panels (Brushes, Guide,
  Layers) that becomes a tabbed bottom sheet in portrait, view switcher (front/side/top/3D),
  toasts, and a left-handed mode that mirrors the layout. Tools not built yet (eraser,
  select, sphere/cylinder guides, symmetry, snap) are in the rail, disabled.
- **Drawing:** strokes projected onto a guide plane (front / side / top / view-aligned,
  movable), pressure-sensitive, smoothed; live preview of predicted Pencil points on iPad.
- **iPad:** native UIKit input (Pencil vs finger by tool type, not guessing), toolbar,
  home screen, system file picker, open sketches from the Files app, Files thumbnails.
- **Brushes:** 4 built-ins, brush editor with live preview and pressure-curve editor,
  save / save as new / delete / share / import (`.brush.toml`).
- **Documents:** home screen with Recents (thumbnails), + New, Open…, autosave (15 s, on
  background, before quit, when switching sketches), undo/redo, re-brush strokes.
  Sketches are ordinary files the OS manages: `Files › On My iPad › AirDraw › Sketches` on
  iPad, `~/Documents/AirDraw/Sketches` on macOS.
- **Export:** 9 built-in formats plus plugins; outputs verified by Apple Model I/O (OBJ,
  STL, PLY, USD/USDZ), Godot's ufbx importer (FBX) and glTF importer (GLB/glTF), with matching
  triangle counts; USDZ renders in SceneKit, SVG renders in ThorVG.
- **Plugins:** C/C++ exporter plugins with generated settings UI; sample `.sem` exporter
  showing both integration paths for a game-engine asset cooker.
- **macOS:** app export (Apple Silicon), `.airdraw` registered as "AirDraw Sketch", Finder
  thumbnails via the Quick Look extension, app category Graphics & Design. Opening a sketch
  from Finder works whether or not AirDraw is running (engine patch `0002` hands files to the
  running app instead of starting a second instance).

### Not working / known issues

- **Not built yet:** eraser, selection/move/duplicate, symmetry, sphere/cylinder guides,
  layer UI (the file format supports layers), hover cursor, Look presets (toon, realistic…),
  settings screen, share sheet.
- **Platforms:** no Windows, Linux or Android builds yet; macOS is arm64-only (no Universal
  build), development-signed (no Developer ID / notarization).
- **Unverified:** Pencil latency vs. Procreate/Feather (not measured), FBX in Autodesk
  tools, AR Quick Look on iPad.
- **Dev leftovers:** `app/airdraw.gdextension` points `macos.debug` at the *release* Rust
  library (marked `TEMP`); exporter plugins load from `user://plugins` with no opt-in UI.

## Repository layout

```
app/          Godot project (GDScript UI, export presets, tests)
docs/         ARCHITECTURE.md — design, decisions, roadmap
engine/       Godot source checkout (not committed; see Setup)
patches/godot AirDraw's engine patches
platform/     Apple Quick Look thumbnail extension (Swift)
plugins/      Sample C++ exporter plugin
rust/         Rust workspace (core + GDExtension)
scripts/      Build, packaging and deploy scripts
build/        Build outputs (not committed)
```

## Setup (macOS host)

Requirements: Xcode (with iOS SDK), Rust (stable ≥ 1.94, edition 2024) with targets
`aarch64-apple-ios`, Homebrew `scons`, Python 3, CMake (sample plugin only). An Apple
Development certificate is needed for iPad deploys and for the macOS Quick Look extension.

```bash
# 1. Engine source + AirDraw patches
git clone --depth 1 --branch 4.7.2-stable https://github.com/godotengine/godot.git engine/godot
scripts/apply_godot_patches.sh

# 2. Godot editor (Apple Silicon)
(cd engine/godot && scons platform=macos arch=arm64 target=editor)

# 3. Export templates (iOS is Metal-only: vulkan=no)
(cd engine/godot && scons platform=ios arch=arm64 target=template_debug vulkan=no \
  && scons platform=ios arch=arm64 target=template_release vulkan=no \
  && scons platform=macos arch=arm64 target=template_release \
  && scons platform=macos arch=arm64 target=template_debug)
scripts/package_ios_templates.sh     # adds the thumbnail extension target
scripts/package_macos_templates.sh

# 4. Rust core
rustup target add aarch64-apple-ios
(cd rust && cargo build --release)
```

## Build and run

```bash
# Desktop (from the Godot editor binary)
engine/godot/bin/godot.macos.editor.arm64 --path app

# iPad: builds the Rust core for iOS, exports, signs, installs and launches
# (team ID and bundle ID are in app/export_presets.cfg)
scripts/deploy_ipad.sh

# macOS app with the Quick Look extension → build/macos/AirDraw.app
scripts/build_macos.sh

# Sample C++ exporter plugin (optionally install into a plugins folder)
scripts/build_sample_plugin.sh "$HOME/Library/Application Support/Godot/app_userdata/AirDraw/plugins"
```

## Tests

```bash
# Rust unit + integration tests (includes building/loading the C++ sample plugin and a
# header/ABI layout check)
(cd rust && cargo test --workspace)

# App smoke test: drives the real app with synthetic mouse/touch input — drawing, undo,
# brushes, brush editor, documents, every export format, home screen — and saves
# screenshots to the Godot user data folder
engine/godot/bin/godot.macos.editor.arm64 --path app res://tests/draw_smoke_test.tscn

# Brush rebuild timing (prints, doesn't assert)
(cd rust && cargo test --release -p airdraw-brush perf -- --ignored --nocapture)
```

iOS-specific code (Pencil input, picker, open-from-Files) has no automated tests yet; it's
verified by hand on device.

## File format

`.airdraw` files are versioned binary: `ADRW`, format version, an embedded PNG preview right
after the header (so OS thumbnailers read only the first bytes), TOML metadata (layers,
brush snapshots), then per-stroke binary point data and a checksum. Format 1 files still
open. Details in `rust/crates/airdraw-doc/src/file.rs`.

## License

Not chosen yet. Godot is MIT-licensed; godot-rust is MPL-2.0.
