#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VENDOR_DIR="$ROOT/vendor"
FBNEO_DIR="$VENDOR_DIR/FBNeo"
CORES_DIR="$ROOT/cores"
OUT_CORE="$CORES_DIR/fbneo_libretro.so"

mkdir -p "$VENDOR_DIR" "$CORES_DIR"

if [ ! -d "$FBNEO_DIR" ]; then
  git clone https://github.com/finalburnneo/FBNeo.git "$FBNEO_DIR"
else
  git -C "$FBNEO_DIR" pull --ff-only
fi

make -C "$FBNEO_DIR/src/burner/libretro" -j"$(nproc 2>/dev/null || echo 4)"

BUILT="$(find "$FBNEO_DIR/src/burner/libretro" -type f \( -name 'fbneo*_libretro*.so' -o -name 'fbneo_libretro.so' \) -printf '%T@ %p\n' 2>/dev/null | sort -nr | head -n1 | cut -d' ' -f2-)"
if [ -z "$BUILT" ]; then
  echo "FBNeo build finished, but fbneo_libretro.so was not found." >&2
  exit 1
fi

cp "$BUILT" "$OUT_CORE"
echo "Built $OUT_CORE"
