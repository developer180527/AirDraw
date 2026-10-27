#!/bin/sh
# Packages the source-built Godot iOS libraries into engine/godot/bin/ios.zip (export template).
set -eu
G="$(cd "$(dirname "$0")/../engine/godot" && pwd)"
OUT="$G/bin/ios_xcode"
rm -rf "$OUT" "$G/bin/ios.zip"
cp -R "$G/misc/dist/apple_embedded_xcode" "$OUT"
rm -rf "$OUT"/*visionos*
for t in debug release; do
  cp "$G/bin/libgodot.ios.template_$t.arm64.a" "$OUT/libgodot.ios.$t.xcframework/ios-arm64/libgodot.a"
  cp "$G/bin/libgodot_camera.ios.template_$t.arm64.a" "$OUT/libgodot_camera.ios.$t.xcframework/ios-arm64/libgodot_camera.a"
done
find "$OUT" -name empty -delete
# Metal-only build (vulkan=no): drop MoltenVK references from the Xcode project template.
sed -i "" "/\$moltenvk_/d" "$OUT/godot_apple_embedded.xcodeproj/project.pbxproj"
# AirDraw: Quick Look thumbnail extension target (Files app previews of .airdraw files).
python3 -I "$(dirname "$0")/ios_template_add_thumbnail.py" "$OUT" "$(cd "$(dirname "$0")/.." && pwd)/platform/apple/AirDrawThumbnail"
(cd "$OUT" && zip -qry "$G/bin/ios.zip" .)
echo "Built $G/bin/ios.zip"
