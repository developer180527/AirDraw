#!/bin/sh
# Builds the Rust core for iOS and wraps it as a .framework for Godot's iOS export.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${1:-release}"
cd "$ROOT/rust"
cargo build -p airdraw-godot --profile "$PROFILE" --target aarch64-apple-ios

DIR=$([ "$PROFILE" = dev ] && echo debug || echo "$PROFILE")
FW="$ROOT/app/bin/ios/AirDrawCore.framework"
rm -rf "$FW" && mkdir -p "$FW"
cp "target/aarch64-apple-ios/$DIR/libairdraw_godot.dylib" "$FW/AirDrawCore"
install_name_tool -id @rpath/AirDrawCore.framework/AirDrawCore "$FW/AirDrawCore"
cat > "$FW/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>AirDrawCore</string>
  <key>CFBundleIdentifier</key><string>com.airdraw.core</string>
  <key>CFBundleName</key><string>AirDrawCore</string>
  <key>CFBundlePackageType</key><string>FMWK</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleSupportedPlatforms</key><array><string>iPhoneOS</string></array>
  <key>MinimumOSVersion</key><string>16.0</string>
</dict></plist>
PLIST
echo "Built $FW"
