#!/usr/bin/env bash
# Cut a signed qsketch release and publish it to the in-app update channel:
#
#   1. bump the workspace version (Cargo.toml + Cargo.lock)
#   2. build the Linux tarball (scripts/build-linux.sh) and the Windows NSIS
#      installer + portable zip (scripts/build-windows.sh); both also copy to
#      $QSKETCH_PUBLISH_DIR via publish.sh when it is set
#   3. minisign-sign the two auto-update artifacts with the (offline) updater key
#   4. publish artifacts + qsketch-latest.json to the update directory, which
#      some HTTP server exposes as $QSKETCH_UPDATE_URL/<file>
#   5. verify the served manifest + artifacts byte-exact, commit + tag
#
# Usage: scripts/release.sh [--allow-dirty] [--no-windows] <version> "<release notes>"
#
# Required env (or a gitignored .env at the repo root):
#   QSKETCH_UPDATER_KEY   minisign private key file (tauri signer format)
#   QSKETCH_UPDATE_DIR    local directory the update server serves
#   QSKETCH_UPDATE_URL    public base URL of that directory (no trailing slash)
# Optional:
#   QSKETCH_PUBLISH_DIR   where publish.sh copies manual-download artifacts
#
# The build embeds $QSKETCH_UPDATE_URL/qsketch-latest.json as the legacy
# manifest URL so installs that predate the user-configurable URL keep
# updating; fresh installs must enter a URL in Preferences.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
[ -f "$root/.env" ] && { set -a; . "$root/.env"; set +a; }

allow_dirty=""
build_windows=1
pos=()
for a in "$@"; do
  case "$a" in
    --allow-dirty) allow_dirty=1 ;;
    --no-windows) build_windows="" ;;
    *) pos+=("$a") ;;
  esac
done
set -- "${pos[@]+"${pos[@]}"}"
new="${1:-}"
notes="${2:-}"
if [ -z "$new" ] || [ -z "$notes" ]; then
  echo "usage: scripts/release.sh [--allow-dirty] [--no-windows] <version> \"<release notes>\"" >&2
  exit 2
fi

KEY="${QSKETCH_UPDATER_KEY:-}"
UPDATE_DIR="${QSKETCH_UPDATE_DIR:-}"
UPDATE_URL="${QSKETCH_UPDATE_URL:-}"
for v in QSKETCH_UPDATER_KEY QSKETCH_UPDATE_DIR QSKETCH_UPDATE_URL; do
  [ -n "${!v:-}" ] || { echo "$v is not set (export it or put it in .env)" >&2; exit 1; }
done
UPDATE_URL="${UPDATE_URL%/}"
export QSKETCH_LEGACY_MANIFEST_URL="$UPDATE_URL/qsketch-latest.json"

say() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }

# --- preflight ---------------------------------------------------------------
[ -f "$KEY" ] || { echo "missing updater key: $KEY" >&2; exit 1; }
[ -d "$UPDATE_DIR" ] || { echo "missing update dir: $UPDATE_DIR" >&2; exit 1; }
command -v cargo-tauri >/dev/null || { echo "cargo-tauri not on PATH (used for minisign signing)" >&2; exit 1; }
if [ -z "$allow_dirty" ] && [ -n "$(git status --porcelain)" ]; then
  echo "working tree is dirty (commit first or pass --allow-dirty)" >&2
  exit 1
fi
[[ "$new" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must be MAJOR.MINOR.PATCH" >&2; exit 1; }
cur="$(grep -m1 '^version *= *"' Cargo.toml | sed -E 's/version *= *"([^"]+)".*/\1/')"
if [ "$(printf '%s\n%s\n' "$cur" "$new" | sort -V | tail -1)" != "$new" ] || [ "$cur" = "$new" ]; then
  echo "new version $new must be greater than current $cur" >&2
  exit 1
fi

export TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY")"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

sign() {
  # Writes <file>.sig (base64 of the minisign signature file), Tauri-updater style.
  local f="$1"
  rm -f "$f.sig"
  cargo tauri signer sign "$f" >/dev/null
  [ -f "$f.sig" ] || { echo "signing failed for $f" >&2; exit 1; }
}

# --- 1. bump -----------------------------------------------------------------
say "bump version $cur -> $new"
sed -i -E "0,/^version *= *\"[^\"]+\"/s//version = \"$new\"/" Cargo.toml
cargo update -w -q
grep -q "^version = \"$new\"" Cargo.toml || { echo "version bump failed" >&2; exit 1; }
# Changelog: turn "## Unreleased" into the version heading if present.
if grep -q '^## Unreleased' CHANGELOG.md; then
  sed -i "0,/^## Unreleased/s//## $new — $(date -u +%Y-%m-%d)/" CHANGELOG.md
fi

# --- 2. build ------------------------------------------------------------------
say "build Linux tarball"
scripts/build-linux.sh
LINUX="dist/qsketch-${new}-linux-x86_64.tar.gz"
[ -f "$LINUX" ] || { echo "missing $LINUX" >&2; exit 1; }

SETUP=""
if [ -n "$build_windows" ]; then
  say "build Windows installer"
  scripts/build-windows.sh
  SETUP="dist/qsketch-${new}-setup.exe"
  [ -f "$SETUP" ] || { echo "missing $SETUP" >&2; exit 1; }
fi

# --- 3. sign -------------------------------------------------------------------
say "sign auto-update artifacts"
sign "$LINUX"
[ -n "$SETUP" ] && sign "$SETUP"

# --- 4. publish ----------------------------------------------------------------
say "publish to $UPDATE_DIR"
linux_name="$(basename "$LINUX")"
cp "$LINUX" "$UPDATE_DIR/$linux_name"
find "$UPDATE_DIR" -maxdepth 1 -name 'qsketch-*-linux-x86_64.tar.gz' ! -name "$linux_name" -delete
setup_name=""
if [ -n "$SETUP" ]; then
  setup_name="$(basename "$SETUP")"
  cp "$SETUP" "$UPDATE_DIR/$setup_name"
  find "$UPDATE_DIR" -maxdepth 1 -iname 'qsketch-*-setup.exe' ! -name "$setup_name" -delete
fi
# Manifest. Artifact URLs are RELATIVE to the manifest so the same files work
# behind any hostname or tunnel.
LINUX_SIG="$(cat "$LINUX.sig")" SETUP_SIG="$([ -n "$SETUP" ] && cat "$SETUP.sig" || true)" \
VER="$new" NOTES="$notes" LINUX_NAME="$linux_name" SETUP_NAME="$setup_name" \
python3 - "$UPDATE_DIR/qsketch-latest.json" <<'PY'
import json, os, sys, datetime
out = sys.argv[1]
prev = {}
try:
    prev = json.load(open(out))
except Exception:
    pass
platforms = dict(prev.get("platforms", {})) if prev.get("version") == os.environ["VER"] else {}
platforms["linux-x86_64"] = {"url": os.environ["LINUX_NAME"], "signature": os.environ["LINUX_SIG"]}
if os.environ.get("SETUP_NAME"):
    platforms["windows-x86_64"] = {"url": os.environ["SETUP_NAME"], "signature": os.environ["SETUP_SIG"]}
elif "windows-x86_64" in platforms:
    pass  # keep a same-version Windows entry from a previous --no-windows run
json.dump({
    "version": os.environ["VER"],
    "notes": os.environ["NOTES"],
    "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "platforms": platforms,
}, open(out, "w"), indent=2)
open(out, "a").write("\n")
PY
chmod -R a+rX "$UPDATE_DIR"

# --- 5. verify + commit + tag ---------------------------------------------------
say "verify through $UPDATE_URL"
served="$(curl -fsS -m 20 "$UPDATE_URL/qsketch-latest.json" | python3 -c 'import json,sys;print(json.load(sys.stdin)["version"])' || true)"
if [ "$served" = "$new" ]; then
  echo "manifest OK ($served)"
  for art in "$linux_name" $setup_name; do
    tmp="$(mktemp)"
    curl -fsS -m 600 -o "$tmp" "$UPDATE_URL/$art"
    if cmp -s "$tmp" "$UPDATE_DIR/$art"; then echo "served byte-exact: $art"; else echo "MISMATCH serving $art" >&2; exit 1; fi
    rm -f "$tmp"
  done
else
  echo "WARNING: could not confirm the served manifest (got '${served:-nothing}'); check the update server" >&2
fi

say "commit + tag v$new"
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -q -m "Release $new

$notes"
git tag -a "v$new" -m "qsketch $new"
echo
echo "Released qsketch $new"
echo "  • update dir:  $UPDATE_DIR ($linux_name${setup_name:+, $setup_name}, qsketch-latest.json)"
echo "  • manifest:    $UPDATE_URL/qsketch-latest.json"
[ -n "${QSKETCH_PUBLISH_DIR:-}" ] && echo "  • downloads:   $QSKETCH_PUBLISH_DIR (via publish.sh)"
echo "  • push:        git push && git push --tags"
echo "  • NOTE: back up the updater key ($KEY); losing it ends auto-update."
