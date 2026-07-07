#!/usr/bin/env bash
# Build the MK2-only (Midway T-Unit subset) FBNeo libretro core for macOS,
# pinned to a known commit, installed to ./cores/fbneo_mk2_libretro.dylib.
# See build-fbneo-linux.sh for the full rationale (pin + subset). All three
# build-fbneo-* scripts must pin the SAME ref.

set -euo pipefail

FBNEO_REF="${FBNEO_REF:-cf53523f844b59d48748248a28f15b04d97f08d4}"
FBNEO_REPO="${FBNEO_REPO:-https://github.com/libretro/FBNeo.git}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FBNEO_DIR="$ROOT/vendor/FBNeo"
CORES_DIR="$ROOT/cores"
OUT="$CORES_DIR/fbneo_mk2_libretro.dylib"
JOBS="$(sysctl -n hw.ncpu 2>/dev/null || echo 4)"

mkdir -p "$CORES_DIR"

if [ ! -d "$FBNEO_DIR/.git" ]; then
    echo "Cloning libretro/FBNeo @ $FBNEO_REF"
    git init -q "$FBNEO_DIR"
    git -C "$FBNEO_DIR" remote add origin "$FBNEO_REPO"
fi
CURRENT="$(git -C "$FBNEO_DIR" rev-parse HEAD 2>/dev/null || echo none)"
if [ "$CURRENT" != "$FBNEO_REF" ]; then
    git -C "$FBNEO_DIR" fetch --depth 1 origin "$FBNEO_REF"
    git -C "$FBNEO_DIR" checkout -q --force "$FBNEO_REF"
    git -C "$FBNEO_DIR" clean -qfdx src/burner/libretro || true
fi
echo "FBNeo pinned at: $(git -C "$FBNEO_DIR" rev-parse HEAD)"

cp "$ROOT/tools/Makefile.mk2" "$FBNEO_DIR/src/burner/libretro/Makefile.mk2"

LIBRETRO_DIR="$FBNEO_DIR/src/burner/libretro"
make -C "$LIBRETRO_DIR" SUBSET=mk2 generate-files
make -C "$LIBRETRO_DIR" -j"$JOBS" SUBSET=mk2

# macOS strip: -x keeps exported symbols (the libretro API) intact.
strip -x -o "$OUT" "$LIBRETRO_DIR/fbneo_mk2_libretro.dylib"
echo "Installed $OUT ($(du -h "$OUT" | cut -f1))"
