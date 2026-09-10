#!/usr/bin/env bash
# Build a Windows release of qsketch by cross-compiling from Linux with mingw-w64, then
# package it as an NSIS installer and a portable zip under dist/.
#
# Requires: rustup target x86_64-pc-windows-gnu, x86_64-w64-mingw32-gcc/windres, makensis.
#
# Usage: scripts/build-windows.sh

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

TARGET="x86_64-pc-windows-gnu"
APP_NAME="qsketch"
EXE_NAME="qsketch.exe"

# --- Version -----------------------------------------------------------------
# Read from [workspace.package] in the root Cargo.toml (crate Cargo.tomls just say
# `version.workspace = true`, so the root manifest is the single source of truth).
VERSION="$(grep -m1 '^version *= *"' Cargo.toml | sed -E 's/version *= *"([^"]+)".*/\1/')"
if [ -z "$VERSION" ]; then
  echo "error: could not read version from Cargo.toml" >&2
  exit 1
fi
echo "== Building ${APP_NAME} ${VERSION} for ${TARGET} =="

# --- Toolchain sanity check ----------------------------------------------------
for bin in x86_64-w64-mingw32-gcc x86_64-w64-mingw32-windres makensis; do
  if ! command -v "$bin" >/dev/null 2>&1; then
    echo "error: required tool '$bin' not found on PATH" >&2
    exit 1
  fi
done

# --- Build ----------------------------------------------------------------
cargo build --release --target "$TARGET" -p qsketch

BUILT_EXE="target/${TARGET}/release/${EXE_NAME}"
if [ ! -f "$BUILT_EXE" ]; then
  echo "error: expected build output not found at $BUILT_EXE" >&2
  exit 1
fi

# --- Stage the "windows" dist dir (this is also what installer/qsketch.nsi packages) ---
DIST_DIR="dist/windows"
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"
cp "$BUILT_EXE" "$DIST_DIR/$EXE_NAME"
cp LICENSE-MIT LICENSE-APACHE "$DIST_DIR/"
if [ -f README.md ]; then
  cp README.md "$DIST_DIR/"
fi

# --- Copy MinGW runtime DLLs, but only the ones actually dynamically imported ---
# Static linking these (-C target-feature=+crt-static) is deliberately avoided for this
# target -- see .cargo/config.toml -- so we ship whichever of the runtime DLLs the exe
# actually needs, resolved via the same gcc that built it (so we get the matching
# thread-model variant, e.g. -win32 vs -posix) instead of hardcoding a path.
CANDIDATE_DLLS=(libgcc_s_seh-1.dll libstdc++-6.dll libwinpthread-1.dll)
IMPORTED_DLLS="$(x86_64-w64-mingw32-objdump -p "$DIST_DIR/$EXE_NAME" | grep -i 'DLL Name:' | awk '{print $NF}')"

for dll in "${CANDIDATE_DLLS[@]}"; do
  if grep -qix "$dll" <<<"$IMPORTED_DLLS"; then
    src="$(x86_64-w64-mingw32-gcc -print-file-name="$dll")"
    if [ -f "$src" ]; then
      echo "bundling runtime DLL: $dll (from $src)"
      cp "$src" "$DIST_DIR/"
    else
      echo "warning: $EXE_NAME imports $dll but it could not be located via gcc -print-file-name" >&2
    fi
  fi
done

# --- Portable zip -----------------------------------------------------------
mkdir -p dist
PORTABLE_ZIP="$ROOT_DIR/dist/${APP_NAME}-${VERSION}-windows-portable.zip"
rm -f "$PORTABLE_ZIP"
(cd "$DIST_DIR" && zip -r -9 -q "$PORTABLE_ZIP" .)
echo "wrote dist/${APP_NAME}-${VERSION}-windows-portable.zip"

# --- NSIS installer -----------------------------------------------------------
makensis -DVERSION="$VERSION" installer/qsketch.nsi
echo "wrote dist/${APP_NAME}-${VERSION}-setup.exe"

"$(dirname "$0")/publish.sh"
echo "== Done =="
