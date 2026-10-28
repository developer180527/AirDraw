#!/bin/sh
# Runs a benchmark scene on a connected iPad and copies its results here:
#   live-stroke (default): app/tests/live_stroke_bench.tscn
#   file-format:           app/tests/file_format_bench.tscn (the iPad's own sketches + a 1M-point one)
#   erase:                 app/tests/erase_bench.tscn (eraser picking and partial erase)
#   idle:                  app/tests/idle_bench.gd in the real app (frames and GPU when idle)
# Exports a debug build that includes tests/ (normal exports exclude it), installs it over the
# dev app, launches it with the bench argument, waits for it and copies the CSV it writes
# (Godot's print doesn't reach the devicectl console, and iOS apps can't quit themselves).
# Usage: [BENCH=file-format|erase|idle] [LOOK=com.airdraw.toon] [BENCH_ARGS="…"] scripts/bench_ipad.sh [device-udid-or-name]
#        (default: live-stroke on the first connected iPad)
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH="${BENCH:-live-stroke}"
case "$BENCH" in
  live-stroke) RESULTS="live_stroke_bench.csv live_stroke_bench.txt"; WAIT=90 ;;
  file-format) RESULTS="file_format_bench.txt"; WAIT=30 ;;
  erase) RESULTS="erase_bench.txt"; WAIT=20 ;;
  idle) RESULTS="idle_bench.txt"; WAIT=45 ;;
  *) echo "Unknown BENCH '$BENCH' (live-stroke, file-format, erase, idle)" >&2; exit 1 ;;
esac
GODOT="$ROOT/engine/godot/bin/godot.macos.editor.arm64"
DEVICE="${1:-$(xcrun devicectl list devices 2>/dev/null | awk '/iPad/ && (/available/ || /connected/) && /physical/ {for (i=1;i<=NF;i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$/) {print $i; exit}}')}"
[ -n "$DEVICE" ] || { echo "No connected iPad found" >&2; exit 1; }

PRESETS="$ROOT/app/export_presets.cfg"
cp "$PRESETS" "$PRESETS.bench-backup"
trap 'mv "$PRESETS.bench-backup" "$PRESETS"' EXIT
# Drop tests/* from the exclude filter, whatever else it lists (icons, …).
sed -i '' -E '/^exclude_filter=/ s#tests/\*(, *)?##' "$PRESETS"
grep -q '^exclude_filter=.*tests/' "$PRESETS" && { echo "Couldn't include tests/ in the export" >&2; exit 1; }

"$ROOT/scripts/build_ios_core.sh"
mkdir -p "$ROOT/build/ios"
rm -rf "$ROOT/build/ios/AirDrawBench.ipa" "$ROOT/build/ios/AirDrawBench.xcarchive"
"$GODOT" --headless --path "$ROOT/app" --import >/dev/null 2>&1 || true
"$GODOT" --headless --path "$ROOT/app" --export-debug "iPad" "$ROOT/build/ios/AirDrawBench.ipa" 2>&1 | grep -E "ERROR|SUCCEEDED|FAILED" || true
# Xcode's IPA export can fail on signing parameters (e.g. certificateType
# DEVELOPMENT_MANAGED) after the archive succeeded; the archive's signed .app installs fine.
INSTALL="$ROOT/build/ios/AirDrawBench.ipa"
[ -f "$INSTALL" ] || INSTALL="$ROOT/build/ios/AirDrawBench.xcarchive/Products/Applications/AirDrawBench.app"
[ -e "$INSTALL" ] || { echo "Export failed" >&2; exit 1; }
xcrun devicectl device install app --device "$DEVICE" "$INSTALL" | tail -1
xcrun devicectl device process launch --terminate-existing --device "$DEVICE" \
  com.venugopal.airdraw ++ "bench-$BENCH" ${LOOK:+look "$LOOK"} ${BENCH_ARGS:-} >/dev/null
echo "Benchmark '$BENCH' running on the iPad (keep it unlocked)..."
# The summary file is written last; wait for it (up to 5 minutes) instead of a fixed time.
DONE="${RESULTS##* }"
sleep "$WAIT"
tries=0
until xcrun devicectl device copy from --device "$DEVICE" --domain-type appDataContainer \
    --domain-identifier com.venugopal.airdraw --source "Library/Application Support/$DONE" \
    --destination "$ROOT/build/ios/$DONE" >/dev/null 2>&1; do
  tries=$((tries + 1))
  [ "$tries" -lt 24 ] || { echo "No results after 5 minutes" >&2; exit 1; }
  sleep 10
done
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
if [ "$BENCH" != live-stroke ]; then
  cat "$ROOT/build/ios/$RESULTS"
  exit 0
fi
cat "$ROOT/build/ios/live_stroke_bench.txt"
echo "Per-update data: $ROOT/build/ios/live_stroke_bench.csv"
