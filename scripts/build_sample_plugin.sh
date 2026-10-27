#!/bin/sh
# Builds the sample C++ exporter plugin. With a folder argument, also copies it there
# (e.g. AirDraw's plugins folder: "$HOME/Library/Application Support/Godot/app_userdata/AirDraw/plugins").
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD="$ROOT/build/plugins/sample_engine_exporter"
cmake -S "$ROOT/plugins/sample_engine_exporter" -B "$BUILD" -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build "$BUILD" --config Release >/dev/null
LIB="$(find "$BUILD" -maxdepth 2 \( -name '*.dylib' -o -name '*.so' -o -name '*.dll' \) | head -1)"
echo "Built $LIB"
if [ "${1:-}" != "" ]; then
  mkdir -p "$1"
  cp "$LIB" "$1/"
  echo "Installed into $1"
fi
