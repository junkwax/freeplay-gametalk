#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VENDOR_DIR="$ROOT/vendor"
FBNEO_DIR="$VENDOR_DIR/FBNeo"
CORES_DIR="$ROOT/cores"
OUT_CORE="$CORES_DIR/fbneo_libretro.dylib"

mkdir -p "$VENDOR_DIR" "$CORES_DIR"

if [ ! -d "$FBNEO_DIR" ]; then
  git clone https://github.com/finalburnneo/FBNeo.git "$FBNEO_DIR"
else
  git -C "$FBNEO_DIR" pull --ff-only
fi

JOBS="$(sysctl -n hw.ncpu 2>/dev/null || echo 4)"
make -C "$FBNEO_DIR/src/burner/libretro" -j"$JOBS"

BUILT="$(find "$FBNEO_DIR/src/burner/libretro" -type f \( -name 'fbneo*_libretro*.dylib' -o -name 'fbneo_libretro.dylib' \) -print0 | xargs -0 ls -t 2>/dev/null | head -n1 || true)"
if [ -z "$BUILT" ]; then
  echo "FBNeo build finished, but fbneo_libretro.dylib was not found." >&2
  exit 1
fi

cp "$BUILT" "$OUT_CORE"
echo "Built $OUT_CORE"
