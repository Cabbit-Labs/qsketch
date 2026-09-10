#!/usr/bin/env bash
# Copy built distributables from dist/ to the manual-download directory named
# by QSKETCH_PUBLISH_DIR (also read from a gitignored .env at the repo root).
# Skipped silently when it is unset or its parent directory does not exist.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -f .env ] && { set -a; . ./.env; set +a; }
DEST="${QSKETCH_PUBLISH_DIR:-}"
if [ -z "$DEST" ]; then
    echo "publish: QSKETCH_PUBLISH_DIR not set; skipping"
    exit 0
fi
if [ ! -d "$(dirname "$DEST")" ]; then
    echo "publish: $(dirname "$DEST") not available; skipping"
    exit 0
fi
mkdir -p "$DEST"
shopt -s nullglob
files=(dist/*-setup.exe dist/*-portable.zip dist/*.tar.gz)
if [ ${#files[@]} -eq 0 ]; then
    echo "publish: nothing in dist/ to publish"
    exit 0
fi
cp -v "${files[@]}" "$DEST/"
echo "published to $DEST"
