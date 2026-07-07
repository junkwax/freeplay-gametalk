#!/usr/bin/env bash
# Build the MK2-only (Midway T-Unit subset) FBNeo libretro core for Linux,
# pinned to a known commit, and install it to ./cores/fbneo_mk2_libretro.so.
#
# Why pinned: FBNeo savestate layout and simulation behavior drift between
# commits. Two clients whose cores were built from different commits can
# desync even with identical ROMs, so every Freeplay release must ship a
# core from the SAME ref on every platform. The client reads the GIT tag
# from the core's version string and folds it into the matchmaking
# compatibility hash, so mismatched cores simply never match.
#
# Why subset: `make SUBSET=mk2` (via tools/Makefile.mk2) builds only the
# T-Unit driver + its CPUs/sound (TMS34010, ADSP-2105 DCS, M6809/YM2151/OKI),
# shrinking the core ~35MB -> ~7MB and making it impossible to boot anything
# but T-Unit games.
#
# Bumping the pin: change FBNEO_REF below (and in the other two platform
# scripts — all three MUST match), rebuild on all platforms, and validate a
# cross-platform netplay match before releasing.

set -euo pipefail

# All three build-fbneo-* scripts must pin the same ref.
FBNEO_REF="${FBNEO_REF:-cf53523f844b59d48748248a28f15b04d97f08d4}"
FBNEO_REPO="${FBNEO_REPO:-https://github.com/libretro/FBNeo.git}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FBNEO_DIR="$ROOT/vendor/FBNeo"
CORES_DIR="$ROOT/cores"
OUT="$CORES_DIR/fbneo_mk2_libretro.so"
JOBS="$(nproc 2>/dev/null || echo 4)"

mkdir -p "$CORES_DIR"

# --- Fetch / pin ------------------------------------------------------------
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

# --- Inject the mk2 subset makefile ------------------------------------------
cp "$ROOT/tools/Makefile.mk2" "$FBNEO_DIR/src/burner/libretro/Makefile.mk2"

# --- Build --------------------------------------------------------------------
LIBRETRO_DIR="$FBNEO_DIR/src/burner/libretro"
# driverlist.h must be regenerated for the subset, otherwise the checked-in
# full list references every driver in the tree and the link fails.
make -C "$LIBRETRO_DIR" SUBSET=mk2 generate-files
make -C "$LIBRETRO_DIR" -j"$JOBS" SUBSET=mk2

strip -s -o "$OUT" "$LIBRETRO_DIR/fbneo_mk2_libretro.so"
echo "Installed $OUT ($(du -h "$OUT" | cut -f1))"
