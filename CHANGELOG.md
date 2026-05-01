# Changelog

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
