#!/bin/sh
# Rebuilds the Rust core for iOS, exports the signed iPad app and installs + launches it.
# Usage: scripts/deploy_ipad.sh [device-udid-or-name]   (default: first connected iPad)
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GODOT="$ROOT/engine/godot/bin/godot.macos.editor.arm64"
DEVICE="${1:-$(xcrun devicectl list devices 2>/dev/null | awk '/iPad/ && (/available/ || /connected/) && /physical/ {for (i=1;i<=NF;i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$/) {print $i; exit}}')}"
[ -n "$DEVICE" ] || { echo "No connected iPad found" >&2; exit 1; }

"$ROOT/scripts/build_ios_core.sh"
mkdir -p "$ROOT/build/ios"
rm -rf "$ROOT/build/ios/AirDraw.ipa" "$ROOT/build/ios/AirDraw.xcarchive"
"$GODOT" --headless --path "$ROOT/app" --import >/dev/null 2>&1 || true
"$GODOT" --headless --path "$ROOT/app" --export-debug "iPad" "$ROOT/build/ios/AirDraw.ipa" 2>&1 | grep -E "ERROR|SUCCEEDED|FAILED" || true
# Xcode's IPA export can fail on signing parameters (e.g. certificateType
# DEVELOPMENT_MANAGED) after the archive succeeded; the archive's signed .app installs fine.
INSTALL="$ROOT/build/ios/AirDraw.ipa"
[ -f "$INSTALL" ] || INSTALL="$ROOT/build/ios/AirDraw.xcarchive/Products/Applications/AirDraw.app"
[ -e "$INSTALL" ] || { echo "Export failed" >&2; exit 1; }
xcrun devicectl device install app --device "$DEVICE" "$INSTALL" | tail -1
xcrun devicectl device process launch --device "$DEVICE" com.venugopal.airdraw | tail -1
