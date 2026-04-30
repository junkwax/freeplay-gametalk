#!/usr/bin/env bash
set -euo pipefail

VERSION="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "([^"]+)"/\1/')"
OUT_DIR="dist/freeplay-gametalk-v${VERSION}-macos"
ZIP_FILE="${OUT_DIR}.tar.gz"
EXE_NAME="freeplay"

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR/media" "$OUT_DIR/roms"

cargo build --release
cp "target/release/$EXE_NAME" "$OUT_DIR/$EXE_NAME"

CORE=""
for candidate in "cores/fbneo_libretro.dylib" "fbneo_libretro.dylib"; do
  if [ -f "$candidate" ]; then CORE="$candidate"; break; fi
done
if [ -n "$CORE" ]; then
  cp "$CORE" "$OUT_DIR/fbneo_libretro.dylib"
else
  echo "warning: fbneo_libretro.dylib not found; run tools/build-fbneo-macos.sh" >&2
fi

[ -f ".env.example" ] && cp ".env.example" "$OUT_DIR/.env.example"
[ -f "LICENSE" ] && cp "LICENSE" "$OUT_DIR/LICENSE"
[ -f "NOTICE.md" ] && cp "NOTICE.md" "$OUT_DIR/NOTICE.md"
[ -f "src/media/mk2.ttf" ] && cp "src/media/mk2.ttf" "$OUT_DIR/media/mk2.ttf"
for font in "src/media/N27-Regular.otf" "src/media/N27-Regular.ttf" "src/media/regular.ttf"; do
  [ -f "$font" ] && cp "$font" "$OUT_DIR/media/"
done

cat > "$OUT_DIR/roms/README.txt" <<'EOF'
Place your legally-obtained ROM zip here.
ROM files are not distributed with Freeplay.
EOF

cat > "$OUT_DIR/README.txt" <<EOF
freeplay-gametalk v$VERSION
===========================

INSTALL:
  1. Extract this archive anywhere.
  2. Put your legally-obtained ROM zip in the roms/ folder.
  3. Copy .env.example to .env and fill in online service values.
  4. Run ./freeplay --doctor to verify setup.
  5. Run ./freeplay.

NOTE:
  This is a folder package, not a signed .app bundle yet.
EOF

rm -f "$ZIP_FILE"
tar -czf "$ZIP_FILE" -C "$(dirname "$OUT_DIR")" "$(basename "$OUT_DIR")"
echo "Done: $ZIP_FILE"
