#!/usr/bin/env bash
# Build a Linux release of qsketch and package it as a simple tarball, plus a
# freedesktop.org-style install layout (.desktop file + icons) under dist/linux/.
#
# Usage: scripts/build-linux.sh

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

APP_NAME="qsketch"
BIN_NAME="qsketch"

VERSION="$(grep -m1 '^version *= *"' Cargo.toml | sed -E 's/version *= *"([^"]+)".*/\1/')"
if [ -z "$VERSION" ]; then
  echo "error: could not read version from Cargo.toml" >&2
  exit 1
fi
echo "== Building ${APP_NAME} ${VERSION} for Linux (release) =="

cargo build --release -p qsketch

BUILT_BIN="target/release/${BIN_NAME}"
if [ ! -f "$BUILT_BIN" ]; then
  echo "error: expected build output not found at $BUILT_BIN" >&2
  exit 1
fi

PKG_NAME="${BIN_NAME}-${VERSION}-linux-x86_64"
STAGE_DIR="dist/linux/${PKG_NAME}"
rm -rf "$STAGE_DIR"

# Binary
mkdir -p "$STAGE_DIR/bin"
cp "$BUILT_BIN" "$STAGE_DIR/bin/$BIN_NAME"

# Docs/licenses
cp LICENSE-MIT LICENSE-APACHE "$STAGE_DIR/"
if [ -f README.md ]; then
  cp README.md "$STAGE_DIR/"
fi

# freedesktop.org install layout: share/applications/*.desktop + share/icons/hicolor/<size>/apps/*.png
mkdir -p "$STAGE_DIR/share/applications"
cat >"$STAGE_DIR/share/applications/${BIN_NAME}.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=${APP_NAME}
Comment=A fast, professional sketching and raster painting application
Exec=${BIN_NAME} %F
Icon=${BIN_NAME}
Terminal=false
Categories=Graphics;2DGraphics;RasterGraphics;
MimeType=application/x-qsketch;
EOF

for size in 16 32 48 64 128 256 512 1024; do
  src="assets/icon/icon-${size}.png"
  if [ -f "$src" ]; then
    dest_dir="$STAGE_DIR/share/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$dest_dir"
    cp "$src" "$dest_dir/${BIN_NAME}.png"
  fi
done

# A minimal install script for users unpacking the tarball manually.
cat >"$STAGE_DIR/install.sh" <<'EOF'
#!/usr/bin/env bash
# Installs qsketch for the current user (~/.local).
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${1:-$HOME/.local}"
mkdir -p "$PREFIX/bin" "$PREFIX/share"
cp "$DIR/bin/qsketch" "$PREFIX/bin/qsketch"
chmod +x "$PREFIX/bin/qsketch"
cp -r "$DIR/share/applications" "$PREFIX/share/"
cp -r "$DIR/share/icons" "$PREFIX/share/"
echo "Installed qsketch to $PREFIX/bin/qsketch"
echo "Make sure $PREFIX/bin is on your PATH."
EOF
chmod +x "$STAGE_DIR/install.sh"

# --- Tarball -----------------------------------------------------------
mkdir -p dist
TARBALL="dist/${PKG_NAME}.tar.gz"
rm -f "$TARBALL"
tar -C dist/linux -czf "$TARBALL" "$PKG_NAME"
echo "wrote $TARBALL"

"$(dirname "$0")/publish.sh"
echo "== Done =="
