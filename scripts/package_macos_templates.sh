#!/bin/sh
# Packages the source-built Godot macOS templates into the export template folder.
# Apple Silicon only for now; a Universal build (lipo arm64 + x86_64) comes with desktop releases.
set -eu
G="$(cd "$(dirname "$0")/../engine/godot" && pwd)"
WORK="$G/bin/macos_template_pkg"
rm -rf "$WORK" && mkdir -p "$WORK"
cp -R "$G/misc/dist/macos_template.app" "$WORK/"
mkdir -p "$WORK/macos_template.app/Contents/MacOS"
cp "$G/bin/godot.macos.template_release.arm64" "$WORK/macos_template.app/Contents/MacOS/godot_macos_release.arm64"
cp "$G/bin/godot.macos.template_debug.arm64" "$WORK/macos_template.app/Contents/MacOS/godot_macos_debug.arm64"
rm -f "$G/bin/macos.zip"
(cd "$WORK" && zip -qry "$G/bin/macos.zip" macos_template.app)
T="$HOME/Library/Application Support/Godot/export_templates/4.7.2.stable"
mkdir -p "$T"
ln -sf "$G/bin/macos.zip" "$T/macos.zip"
echo "Built $G/bin/macos.zip"
