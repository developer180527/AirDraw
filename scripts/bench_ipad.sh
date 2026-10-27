#!/bin/sh
# Runs a benchmark scene on a connected iPad and copies its results here:
#   live-stroke (default): app/tests/live_stroke_bench.tscn
#   file-format:           app/tests/file_format_bench.tscn (the iPad's own sketches + a 1M-point one)
# Exports a debug build that includes tests/ (normal exports exclude it), installs it over the
# dev app, launches it with the bench argument, waits for it and copies the CSV it writes
# (Godot's print doesn't reach the devicectl console, and iOS apps can't quit themselves).
# Usage: [BENCH=file-format] [LOOK=com.airdraw.toon] scripts/bench_ipad.sh [device-udid-or-name]
#        (default: live-stroke on the first connected iPad)
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH="${BENCH:-live-stroke}"
case "$BENCH" in
  live-stroke) RESULTS="live_stroke_bench.csv live_stroke_bench.txt"; WAIT=90 ;;
  file-format) RESULTS="file_format_bench.txt"; WAIT=60 ;;
  *) echo "Unknown BENCH '$BENCH' (live-stroke, file-format)" >&2; exit 1 ;;
esac
GODOT="$ROOT/engine/godot/bin/godot.macos.editor.arm64"
DEVICE="${1:-$(xcrun devicectl list devices 2>/dev/null | awk '/iPad/ && (/available/ || /connected/) && /physical/ {for (i=1;i<=NF;i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$/) {print $i; exit}}')}"
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
  com.venugopal.airdraw ++ "bench-$BENCH" ${LOOK:+look "$LOOK"} >/dev/null
echo "Benchmark '$BENCH' running on the iPad (keep it unlocked)..."
sleep "$WAIT"
for f in $RESULTS; do
  xcrun devicectl device copy from --device "$DEVICE" --domain-type appDataContainer \
    --domain-identifier com.venugopal.airdraw --source "Library/Application Support/$f" \
    --destination "$ROOT/build/ios/$f" >/dev/null
done
# Images the bench saves when the finished and rebuilt strokes differ.
for f in bench_com.airdraw.ink_finished.png bench_com.airdraw.ink_rebuilt.png bench_com.airdraw.ribbon_finished.png bench_com.airdraw.ribbon_rebuilt.png; do
  xcrun devicectl device copy from --device "$DEVICE" --domain-type appDataContainer \
    --domain-identifier com.venugopal.airdraw --source "Library/Application Support/$f" \
    --destination "$ROOT/build/ios/$f" >/dev/null 2>&1 || true
done
if [ "$BENCH" = file-format ]; then
  cat "$ROOT/build/ios/file_format_bench.txt"
  exit 0
fi
cat "$ROOT/build/ios/live_stroke_bench.txt"
echo "Per-update data: $ROOT/build/ios/live_stroke_bench.csv"
