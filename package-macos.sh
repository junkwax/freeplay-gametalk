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

cat > "$OUT_DIR/roms/README.txt" <<'EOF'
Place your legally-obtained ROM zip here.
ROM files are not distributed with Freeplay.
EOF

cat > "$OUT_DIR/README.txt" <<EOF
freeplay-gametalk v$VERSION (macOS)
===================================

PREREQS (one-time, system-wide):
  Install Homebrew if you don't have it (https://brew.sh), then:

    brew install sdl2 sdl2_ttf

  These are runtime libraries the bundled freeplay binary links against.
  They are NOT shipped in this archive.

INSTALL:
  1. Extract this archive anywhere.
  2. Put your legally-obtained ROM zip (mk2.zip) into the roms/ folder.
  3. Copy .env.example to .env and fill in online service values
     (signaling URL, Discord client id, stats URL — optional).
  4. macOS Gatekeeper will refuse to launch the unsigned binary on
     first run. Either:
         xattr -d com.apple.quarantine ./freeplay
     or right-click freeplay in Finder > Open > Open anyway.
  5. Make the binary executable if needed:
         chmod +x ./freeplay
  6. Run ./freeplay --doctor to verify setup.
  7. Run ./freeplay.

TROUBLESHOOTING:
  - "image not found: libSDL2-2.0.0.dylib" -> brew install sdl2 sdl2_ttf
  - "fbneo_libretro.dylib not found"       -> ensure it's beside ./freeplay
  - Apple Silicon: build was produced for the host arch of the CI runner.
    If your Mac arch differs, build locally with package-macos.sh.

NOTE:
  This is a folder package, not a signed .app bundle yet. Code-signing and
  notarization are tracked as future work.
EOF

rm -f "$ZIP_FILE"
tar -czf "$ZIP_FILE" -C "$(dirname "$OUT_DIR")" "$(basename "$OUT_DIR")"
echo "Done: $ZIP_FILE"
