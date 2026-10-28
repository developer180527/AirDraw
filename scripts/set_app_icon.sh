#!/bin/sh
# Sets AirDraw's app icon from an Icon Composer document (.icon) or a 1024×1024 PNG.
#   scripts/set_app_icon.sh design/app-icon-v2/AirDraw-Icon.icon
#   scripts/set_app_icon.sh some-icon.png
# Writes:
#   app/icon.png         light icon: Windows, Linux, Android, the home screen mark, iOS default
#   app/icon_dark.png    iOS/iPadOS dark appearance
#   app/icon_tinted.png  iOS/iPadOS tinted appearance (grayscale; the system colours it)
#   app/splash.png       boot splash and iOS launch screen
#   platform/apple/AppIcon.icon  (.icon input only) compiled into the macOS app by
#                        scripts/build_macos.sh, so macOS 26 shows its dark, tinted and
#                        glass looks; older macOS gets the .icns made from it.
# A PNG has no dark version: dark and tinted are made from the light one.
# Then rebuild: scripts/deploy_ipad.sh, scripts/build_macos.sh.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${1:?usage: set_app_icon.sh <icon.icon | icon.png>}"
APP="$ROOT/app"
APPLE_ICON="$ROOT/platform/apple/AppIcon.icon"
ICTOOL="$(xcode-select -p)/../Applications/Icon Composer.app/Contents/Executables/ictool"
GRAY="/System/Library/ColorSync/Profiles/Generic Gray Gamma 2.2 Profile.icc"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

render() { # rendition, output
  "$ICTOOL" "$SRC" --export-image --output-file "$2" --platform iOS --rendition "$1" \
    --width 1024 --height 1024 --scale 1 >/dev/null
  [ -s "$2" ] || { echo "ictool couldn't render the $1 icon" >&2; exit 1; }
}

case "$SRC" in
  *.icon|*.icon/)
    [ -x "$ICTOOL" ] || { echo "Icon Composer's ictool not found (needs Xcode 26+)" >&2; exit 1; }
    render Default "$TMP/light.png"
    render Dark "$TMP/dark.png"
    rm -rf "$APPLE_ICON"
    cp -R "$SRC" "$APPLE_ICON"
    ;;
  *.png)
    sips -z 1024 1024 "$SRC" --out "$TMP/light.png" >/dev/null
    cp "$TMP/light.png" "$TMP/dark.png"
    rm -rf "$APPLE_ICON"
    ;;
  *) echo "Expected a .icon or .png, got $SRC" >&2; exit 1 ;;
esac

# Tinted: the dark icon in grayscale, as Apple asks for tintable icons.
sips -m "$GRAY" "$TMP/dark.png" --out "$TMP/tinted.png" >/dev/null
cp "$TMP/light.png" "$APP/icon.png"
cp "$TMP/dark.png" "$APP/icon_dark.png"
cp "$TMP/tinted.png" "$APP/icon_tinted.png"
sips -Z 320 "$APP/icon.png" --out "$APP/splash.png" >/dev/null
echo "Icon set from $SRC"
[ -d "$APPLE_ICON" ] && echo "  macOS: $APPLE_ICON (compiled by build_macos.sh)"
echo "  iOS launch screens are cached: if the old splash still shows, restart the iPad."
