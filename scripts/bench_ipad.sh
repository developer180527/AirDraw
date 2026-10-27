#!/bin/sh
# Runs app/tests/live_stroke_bench.tscn on a connected iPad and copies its results here.
# Exports a debug build that includes tests/ (normal exports exclude it), installs it over the
# dev app, launches it with the bench argument, waits for it and copies the CSV it writes
# (Godot's print doesn't reach the devicectl console, and iOS apps can't quit themselves).
# Usage: scripts/bench_ipad.sh [device-udid-or-name]   (default: first connected iPad)
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GODOT="$ROOT/engine/godot/bin/godot.macos.editor.arm64"
DEVICE="${1:-$(xcrun devicectl list devices 2>/dev/null | awk '/iPad/ && /available/ && /physical/ {for (i=1;i<=NF;i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$/) {print $i; exit}}')}"
[ -n "$DEVICE" ] || { echo "No connected iPad found" >&2; exit 1; }

PRESETS="$ROOT/app/export_presets.cfg"
cp "$PRESETS" "$PRESETS.bench-backup"
trap 'mv "$PRESETS.bench-backup" "$PRESETS"' EXIT
sed -i '' 's|^exclude_filter="tests/\*"|exclude_filter=""|' "$PRESETS"

"$ROOT/scripts/build_ios_core.sh"
mkdir -p "$ROOT/build/ios"
rm -f "$ROOT/build/ios/AirDrawBench.ipa"
"$GODOT" --headless --path "$ROOT/app" --import >/dev/null 2>&1 || true
"$GODOT" --headless --path "$ROOT/app" --export-debug "iPad" "$ROOT/build/ios/AirDrawBench.ipa" 2>&1 | grep -E "ERROR|SUCCEEDED|FAILED" || true
[ -f "$ROOT/build/ios/AirDrawBench.ipa" ] || { echo "Export failed" >&2; exit 1; }
xcrun devicectl device install app --device "$DEVICE" "$ROOT/build/ios/AirDrawBench.ipa" | tail -1
xcrun devicectl device process launch --terminate-existing --device "$DEVICE" \
  com.venugopal.airdraw ++ bench-live-stroke >/dev/null
echo "Benchmark running on the iPad (keep it unlocked)..."
sleep 90
OUT="$ROOT/build/ios/live_stroke_bench.csv"
xcrun devicectl device copy from --device "$DEVICE" --domain-type appDataContainer \
  --domain-identifier com.venugopal.airdraw --source "Library/Application Support/live_stroke_bench.csv" \
  --destination "$OUT" >/dev/null
echo "Results: $OUT"
