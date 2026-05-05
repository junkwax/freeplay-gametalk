# Changelog

## 0.4.4 - 2026-05-05

### Changed

- STUN discovery now tries `stun.l.google.com`, `stun1.l.google.com`, and
  `stun.cloudflare.com` (resolved via DNS) instead of a single hardcoded
  Google IP, so matchmaking survives Google rotating its STUN backends or
  a single provider being down.
- "UDP port already in use" now produces a clearer error pointing at a
  stray freeplay.exe instead of the generic OS message.
- `poll_status` now retries up to 5 transient HTTP failures (502/503,
  connection resets, brief network blips) with linear backoff before
  failing the queue session. Cloud Run cold starts no longer end a queue.

### Fixed

- Workflow now clones `libretro/FBNeo` (which carries `src/burner/libretro/`)
  instead of `finalburnneo/FBNeo` (which does not), so Linux/macOS release
  builds actually find the libretro Makefile.

## 0.4.3 - 2026-05-05

### Added

- Scorebar style option in Settings: PLATES (slanted scoreplates with names +
  win count) or CENTERED (gamertags-only HUD pulled toward the center of the
  screen above the timer).
- Linux release packaging job in `release.yml`. Tagged pushes now produce a
  `freeplay-gametalk-v<version>-linux.tar.gz` and attach it to the GitHub
  Release alongside the Windows build.
- macOS release packaging job in `release.yml` (folder package, unsigned).
  Tagged pushes now produce `freeplay-gametalk-v<version>-macos.tar.gz`.
- `FBNEO_REF` env var support in `tools/build-fbneo-{linux,macos}.sh` so the
  FBNeo libretro core is checked out at a known ref. Defaults to `master`.
  Release workflows cache `vendor/FBNeo` and `cores/` keyed on this ref so
  subsequent builds skip the FBNeo compile step.
- Per-platform README inside Linux/macOS release packages: prereqs (SDL2
  install per distro / Homebrew), Gatekeeper unquarantine note for macOS,
  troubleshooting for the common runtime-library and core-not-found errors.

### Changed

- The fight overlay is now suppressed on attract / VS / character-select
  screens. It only renders when MK2's `round_num` (RAM 0x256D6) is non-zero,
  so the HUD appears with the round intro instead of overlapping menus.
- Default `ScorebarStyle` is now CENTERED.
- Centered scorebar Y position scales with window height (~6.5% of height,
  clamped 36–84 px) to sit just under MK2's timer instead of overlapping the
  life bars.

## 0.4.2 - 2026-05-01

### Added

- Watch Live screen for browsing active online matches and joining spectator mode.
- Spectator view with live score/status polling and copyable watch links.
- Leaderboard screen with manual refresh.
- Settings screen for Discord RPC, fullscreen, volume, doctor launch, and log folder access.
- Training menu toggles for hitboxes, infinite health, and timer control.
- Toast notifications for refreshes, clipboard actions, controller changes, bindings, and settings changes.
- Post-match exit summary screen for completed sets, intentional quits, disconnects, and timeouts.
- Windows CI packaging job that uploads release artifacts after builds.

### Changed

- Debug console stays hidden during normal Windows runtime and only launches from the menu with `Shift+D` or the Settings screen.
- Online matches ignore `Esc` so players do not accidentally leave ranked/session play.
- Release packaging now ships only `mk2.ttf` as the bundled TTF font.
- Scoreboard and fight overlay font resolution now uses `mk2.ttf`.
- Build/release docs now target `v0.4.2`.

### Fixed

- Graceful disconnect handling now routes players to a clearer exit screen.
- CI workflow now produces a packaged Windows artifact instead of only checking the build.
