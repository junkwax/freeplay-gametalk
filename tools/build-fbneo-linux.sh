#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VENDOR_DIR="$ROOT/vendor"
FBNEO_DIR="$VENDOR_DIR/FBNeo"
CORES_DIR="$ROOT/cores"
OUT_CORE="$CORES_DIR/fbneo_libretro.so"

# Pin the FBNeo commit/branch to keep netplay savestate compatibility predictable
# across builds. Override with FBNEO_REF=<sha-or-branch> to test newer upstream.
FBNEO_REF="${FBNEO_REF:-master}"

mkdir -p "$VENDOR_DIR" "$CORES_DIR"

if [ ! -d "$FBNEO_DIR" ]; then
  # Clone the libretro/FBNeo fork — it carries the libretro target under
  # src/burner/libretro/, which finalburnneo/FBNeo does not.
  git clone https://github.com/libretro/FBNeo.git "$FBNEO_DIR"
fi
git -C "$FBNEO_DIR" fetch --tags origin
git -C "$FBNEO_DIR" checkout "$FBNEO_REF"
# If FBNEO_REF is a branch (e.g. "master"), pull latest. Detached HEAD on a SHA
# will fail this pull silently — that's fine, we already have the tree we want.
git -C "$FBNEO_DIR" pull --ff-only origin "$FBNEO_REF" 2>/dev/null || true

make -C "$FBNEO_DIR/src/burner/libretro" -j"$(nproc 2>/dev/null || echo 4)"

BUILT="$(find "$FBNEO_DIR/src/burner/libretro" -type f \( -name 'fbneo*_libretro*.so' -o -name 'fbneo_libretro.so' \) -printf '%T@ %p\n' 2>/dev/null | sort -nr | head -n1 | cut -d' ' -f2-)"
if [ -z "$BUILT" ]; then
  echo "FBNeo build finished, but fbneo_libretro.so was not found." >&2
  exit 1
fi

cp "$BUILT" "$OUT_CORE"
echo "Built $OUT_CORE"
