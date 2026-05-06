#!/usr/bin/env bash
set -euo pipefail

VERSION="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "([^"]+)"/\1/')"
OUT_DIR="dist/freeplay-gametalk-v${VERSION}-linux"
ZIP_FILE="${OUT_DIR}.tar.gz"
EXE_NAME="freeplay"

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR/media" "$OUT_DIR/roms"

cargo build --release
cp "target/release/$EXE_NAME" "$OUT_DIR/$EXE_NAME"

CORE=""
for candidate in "cores/fbneo_libretro.so" "fbneo_libretro.so"; do
  if [ -f "$candidate" ]; then CORE="$candidate"; break; fi
done
if [ -n "$CORE" ]; then
  cp "$CORE" "$OUT_DIR/fbneo_libretro.so"
else
  echo "warning: fbneo_libretro.so not found; run tools/build-fbneo-linux.sh" >&2
fi

[ -f ".env.example" ] && cp ".env.example" "$OUT_DIR/.env.example"
[ -f "LICENSE" ] && cp "LICENSE" "$OUT_DIR/LICENSE"
[ -f "NOTICE.md" ] && cp "NOTICE.md" "$OUT_DIR/NOTICE.md"

# Bundle a working .env with public defaults (signaling URL, Discord
# client ID, stats URL). Local .env wins if present (dev/self-host
# overrides); otherwise fall back to the tracked .env.public so a fresh
# download just works. Webhook URL line is always blanked — that's
# the one genuinely-private value.
ENV_SOURCE=""
if [ -f ".env" ]; then
  ENV_SOURCE=".env"
elif [ -f ".env.public" ]; then
  ENV_SOURCE=".env.public"
fi
if [ -n "$ENV_SOURCE" ]; then
  grep -v -E '^\s*FREEPLAY_DISCORD_WEBHOOK_URL\s*=' "$ENV_SOURCE" > "$OUT_DIR/.env"
  printf '\n# Optional — your own Discord channel webhook for match notifications.\nFREEPLAY_DISCORD_WEBHOOK_URL=\n' >> "$OUT_DIR/.env"
  echo "Bundled .env from $ENV_SOURCE (public defaults, no secrets)"
fi
[ -f "src/media/mk2.ttf" ] && cp "src/media/mk2.ttf" "$OUT_DIR/media/mk2.ttf"

cat > "$OUT_DIR/roms/README.txt" <<'EOF'
Place your legally-obtained ROM zip here.
ROM files are not distributed with Freeplay.
EOF

cat > "$OUT_DIR/README.txt" <<EOF
freeplay-gametalk v$VERSION (Linux)
===================================

PREREQS (one-time, system-wide):
  Debian/Ubuntu:
    sudo apt install libsdl2-2.0-0 libsdl2-ttf-2.0-0

  Fedora:
    sudo dnf install SDL2 SDL2_ttf

  Arch:
    sudo pacman -S sdl2 sdl2_ttf

  These are runtime libraries the bundled freeplay binary links against.
  They are NOT shipped in this archive.

INSTALL:
  1. Extract this archive anywhere.
  2. Put your legally-obtained ROM zip (mk2.zip) into the roms/ folder.
  3. (Optional) Edit .env to add a Discord webhook URL if you want a
     personal channel pinged on match results. Out of the box .env
     ships with public defaults that point at the official servers —
     nothing to configure for matchmaking to work.
  4. Make the binary executable if needed:
         chmod +x ./freeplay
  5. Run ./freeplay --doctor to verify setup.
  6. Run ./freeplay.

TROUBLESHOOTING:
  - "error while loading shared libraries: libSDL2-2.0.so.0"
        -> install the SDL2 package for your distro (above).
  - "fbneo_libretro.so not found"
        -> ensure it's beside ./freeplay.
  - No audio: PulseAudio/PipeWire not running for your user. SDL falls
    back to silent; the game still runs.
EOF

rm -f "$ZIP_FILE"
tar -czf "$ZIP_FILE" -C "$(dirname "$OUT_DIR")" "$(basename "$OUT_DIR")"
echo "Done: $ZIP_FILE"
