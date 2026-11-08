#!/usr/bin/env bash
# Build KeptCraft for Apple silicon and Intel and package it as one app:
#
#   $DIST/keptcraft-<version>-macos-universal.dmg  KeptCraft.app on a drag-to-Applications disk image
#
# Usage: packaging/macos/package.sh [--skip-build]
#
# Needs Xcode's command line tools and the aarch64-apple-darwin and x86_64-apple-darwin Rust targets.
# The app is signed ad hoc (no Developer ID), so Gatekeeper asks users to approve it on first launch.
set -euo pipefail
# shellcheck source=packaging/env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/macos"
APP_ID=io.github.jamesbuckley-source.keptcraft
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)

SKIP_BUILD=0
case "${1:-}" in
  "") ;;
  --skip-build) SKIP_BUILD=1 ;;
  *) echo "usage: $0 [--skip-build]" >&2; exit 2 ;;
esac

# Keep in sync with LSMinimumSystemVersion in Info.plist.in.
export MACOSX_DEPLOYMENT_TARGET=11.0
WORK="$CARGO_TARGET_DIR/macos-package"
APP="$WORK/KeptCraft.app"
DMG="$DIST/keptcraft-$VERSION-macos-universal.dmg"
echo "==> KeptCraft $VERSION for macOS (universal)"

if [ "$SKIP_BUILD" = 0 ]; then
  (cd "$ROOT" && cargo build --release --locked -p keptcraft --target "${TARGETS[0]}" --target "${TARGETS[1]}")
fi
rm -rf "$WORK"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/Licenses"

# ---- KeptCraft.app --------------------------------------------------------------------------------
lipo -create -output "$APP/Contents/MacOS/KeptCraft" \
  "$CARGO_TARGET_DIR/${TARGETS[0]}/release/keptcraft" "$CARGO_TARGET_DIR/${TARGETS[1]}/release/keptcraft"
lipo "$APP/Contents/MacOS/KeptCraft" -verify_arch arm64 x86_64
# The icon set from the hicolor PNGs; iconutil leaves out the sizes that have no file.
ICONSET="$WORK/KeptCraft.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  cp "$ROOT/assets/app-icon/hicolor/${s}x${s}/apps/$APP_ID.png" "$ICONSET/icon_${s}x${s}.png"
  if [ "$s" != 512 ]; then
    cp "$ROOT/assets/app-icon/hicolor/$((s * 2))x$((s * 2))/apps/$APP_ID.png" "$ICONSET/icon_${s}x${s}@2x.png"
  fi
done
iconutil -c icns -o "$APP/Contents/Resources/KeptCraft.icns" "$ICONSET"

sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/${VERSION%%-*}/g" "$HERE/Info.plist.in" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"
copy_docs "$APP/Contents/Resources/Licenses"

# Ad hoc: Apple silicon runs only signed code, and a bundle signature gives macOS a stable identity
# for the Screen Recording permission. Sign the executable, then the bundle.
codesign --force --sign - --identifier "$APP_ID" "$APP/Contents/MacOS/KeptCraft"
codesign --force --sign - --identifier "$APP_ID" "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"
"$APP/Contents/MacOS/KeptCraft" --version

# ---- DMG ------------------------------------------------------------------------------------------
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/KeptCraft.app"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
# makehybrid and convert build the image without attaching a device, which `hdiutil create
# -srcfolder` needs and which fails intermittently on CI runners ("Resource busy").
hdiutil makehybrid -hfs -hfs-volume-name "KeptCraft" -hfs-openfolder "$STAGE" -o "$WORK/raw.dmg" "$STAGE"
hdiutil convert "$WORK/raw.dmg" -format UDZO -imagekey zlib-level=9 -o "$DMG"
rm -f "$WORK/raw.dmg"
hdiutil verify "$DMG"

echo "==> done"
ls -lh "$DMG"
