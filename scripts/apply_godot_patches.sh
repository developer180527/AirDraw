#!/bin/sh
# Applies AirDraw's engine patches (patches/godot/*.patch) to the Godot checkout.
# Run after cloning or updating engine/godot; already-applied patches are skipped.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/engine/godot"
for p in "$ROOT"/patches/godot/*.patch; do
  if git apply --reverse --check "$p" 2>/dev/null; then
    echo "already applied: $(basename "$p")"
  else
    git apply "$p" && echo "applied: $(basename "$p")"
  fi
done
