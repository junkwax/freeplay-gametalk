#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VENDOR_DIR="$ROOT/vendor"
FBNEO_DIR="$VENDOR_DIR/FBNeo"
CORES_DIR="$ROOT/cores"
OUT_CORE="$CORES_DIR/fbneo_libretro.dylib"

# Pin the FBNeo commit/branch to keep netplay savestate compatibility predictable
# across builds. Override with FBNEO_REF=<sha-or-branch> to test newer upstream.
FBNEO_REF="${FBNEO_REF:-master}"

mkdir -p "$VENDOR_DIR" "$CORES_DIR"

if [ ! -d "$FBNEO_DIR" ]; then
  git clone https://github.com/finalburnneo/FBNeo.git "$FBNEO_DIR"
fi
git -C "$FBNEO_DIR" fetch --tags origin
git -C "$FBNEO_DIR" checkout "$FBNEO_REF"
git -C "$FBNEO_DIR" pull --ff-only origin "$FBNEO_REF" 2>/dev/null || true

JOBS="$(sysctl -n hw.ncpu 2>/dev/null || echo 4)"
make -C "$FBNEO_DIR/src/burner/libretro" -j"$JOBS"

BUILT="$(find "$FBNEO_DIR/src/burner/libretro" -type f \( -name 'fbneo*_libretro*.dylib' -o -name 'fbneo_libretro.dylib' \) -print0 | xargs -0 ls -t 2>/dev/null | head -n1 || true)"
if [ -z "$BUILT" ]; then
  echo "FBNeo build finished, but fbneo_libretro.dylib was not found." >&2
  exit 1
fi

cp "$BUILT" "$OUT_CORE"
echo "Built $OUT_CORE"
