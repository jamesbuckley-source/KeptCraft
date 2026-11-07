#!/bin/sh
# Install ShotCraft4Kept with its desktop entry, icons, AppStream metadata and license files.
#
#   cargo build --release && packaging/linux/install.sh    # for the current user, into ~/.local
#   sudo PREFIX=/usr/local packaging/linux/install.sh      # system-wide
#   packaging/linux/install.sh --uninstall                 # same PREFIX as the install
#
# Settings and capture history stay in ~/.config/shotcraft4kept and ~/.local/share/shotcraft4kept.
set -eu
APP_ID=io.github.jamesbuckley-source.shotcraft4kept
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
PREFIX=${PREFIX:-$HOME/.local}
BIN=${BIN:-$ROOT/target/release/shotcraft4kept}
SHARE=$PREFIX/share
DOC=$SHARE/doc/shotcraft4kept
SIZES="16 24 32 48 64 128 256 512"

# Desktop entries need no cache (there is no MimeType). Only refresh an icon cache that a theme
# index already owns: a new cache in ~/.local/share/icons/hicolor could hide other apps' icons.
refresh() {
  if [ -f "$SHARE/icons/hicolor/index.theme" ] && command -v gtk-update-icon-cache >/dev/null; then
    gtk-update-icon-cache -q "$SHARE/icons/hicolor" || true
  fi
}

if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$PREFIX/bin/shotcraft4kept" "$SHARE/applications/$APP_ID.desktop" "$SHARE/metainfo/$APP_ID.metainfo.xml" \
    "$SHARE/icons/hicolor/scalable/apps/$APP_ID.svg"
  for s in $SIZES; do rm -f "$SHARE/icons/hicolor/${s}x${s}/apps/$APP_ID.png"; done
  rm -rf "$DOC"
  refresh
  echo "ShotCraft4Kept removed from $PREFIX"
  exit 0
fi

[ -x "$BIN" ] || { echo "error: $BIN not found; run cargo build --release first" >&2; exit 1; }
# The desktop entry's Exec key can quote spaces but treats these as escapes or field codes.
case $PREFIX in
  *[\"\`\$\\%]*) echo "error: PREFIX must not contain \" \` \$ \\ or %" >&2; exit 1 ;;
esac

install -Dm755 "$BIN" "$PREFIX/bin/shotcraft4kept"
# Absolute paths, because ~/.local/bin is not on every desktop session's PATH.
exe=$(printf '%s' "$PREFIX/bin/shotcraft4kept" | sed 's/[&|]/\\&/g')
mkdir -p "$SHARE/applications"
sed -e "s|^Exec=shotcraft4kept|Exec=\"$exe\"|" -e "s|^TryExec=shotcraft4kept|TryExec=$exe|" \
  "$ROOT/packaging/linux/$APP_ID.desktop" >"$SHARE/applications/$APP_ID.desktop"
install -Dm644 "$ROOT/packaging/linux/$APP_ID.metainfo.xml" "$SHARE/metainfo/$APP_ID.metainfo.xml"
install -Dm644 "$ROOT/assets/app-icon/shotcraft4kept.svg" "$SHARE/icons/hicolor/scalable/apps/$APP_ID.svg"
for s in $SIZES; do
  install -Dm644 "$ROOT/assets/app-icon/hicolor/${s}x${s}/apps/$APP_ID.png" "$SHARE/icons/hicolor/${s}x${s}/apps/$APP_ID.png"
done
for f in LICENSE-MIT LICENSE-APACHE NOTICE THIRD-PARTY-LICENSES.md ATTRIBUTION.md; do
  install -Dm644 "$ROOT/$f" "$DOC/$f"
done
install -Dm644 "$ROOT/crates/doc/assets/fonts/OFL.txt" "$DOC/OFL-Inter.txt"
refresh
echo "ShotCraft4Kept installed into $PREFIX"
