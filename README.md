# freeplay-gametalk


![freeplay-image](https://raw.githubusercontent.com/junkwax/freeplay-gametalk/main/screenshot.png)

freeplay-gametalk is the Freeplay client package: a Rust/SDL2 arcade rollback
client with username-based matchmaking, ghost recording, profile stats, and a
compact in-game overlay. It wraps an FBNeo libretro core, drives the emulator
frame by frame, and synchronizes two players with GGRS rollback netcode over
the public `freeplay-relay` UDP path.

This repository does not include ROM files, emulator DLLs, private service
URLs, OAuth client IDs, tokens, or webhooks.

## Current Build

- Modern SDL menu with Profile, Load Ghosts, Watch Replays, Controls,
  Settings, and About.
- Username-based online sign-in; Discord OAuth is not required to match.
- Optional Discord account linking from Settings for profile lookup and
  account display.
- Optional Stats Email setting for portable ratings/history across machines.
- Discord Rich Presence with join/spectate support when configured.
- Online matchmaking through a signaling service.
- Public UDP relay path for cross-NAT online play.
- Best-of-session scoring and a high-resolution fight overlay rendered over
  the emulator view.
- Profile page with rating, wins, losses, win rate, match history, and Discord
  avatar support when available.
- Ghost recording, upload/download support, local ghost playback, full match
  replay viewing, and lab play drone mode.
- Package script that creates a distributable zip without ROMs or secrets.

## Hotkeys

### Menus

- `Up`/`Down` or controller D-pad: move selection.
- `Enter`, numpad `Enter`, controller `A`, or controller `Start`: confirm.
- `Esc`, controller `B`, or controller `Back`: go back/cancel.
- `Tab`, `Left`/`Right`, or controller D-pad left/right: switch P1/P2 on
  screens that support it.
- `Shift+D`: open Doctor from menu screens.

### Lab And Local Play

- `Esc`: return to the main menu.
- `F1` or `F2`: toggle hitbox overlay.
- `F3`: toggle infinite health.
- `F4`: toggle freeze timer.
- `F5`: save the current Lab reset point.
- `F7`: load the saved Lab reset point.
- `F6`: start/stop local ghost recording.
- `F8`: full ghost playback from `ghost.bin`.
- `F10`: toggle reactive drone behavior while a ghost playback is active.
- `F12`: play against a logic-driven P2 ghost opponent.
- `F11`: show/hide the Lab assist panel, including P1 input history.
- `Ctrl+R`: start/stop MP4 clip recording.
- `Ctrl+F`: cycle video filter.
- `Ctrl+A`: cycle aspect mode.

### Online And Replay

- `T`: open online chat.
- `Enter` or controller `Start`: send chat.
- `Esc`, controller `B`, or controller `Back`: close chat.
- `F1`: leave an online set gracefully.
- `Esc`: stop full replay playback and return to Watch Replays.
- `Shift+F11`: dump SYSTEM_RAM for diagnostics.
- `F9`: run the rewind determinism test.

## Requirements

- Windows, Linux, or macOS
- Rust stable toolchain
- SDL2 runtime libraries:
  - **Windows**: SDL2.dll + SDL2_ttf.dll next to the executable (bundled in
    the packaged Windows release)
  - **Linux**: `libsdl2-2.0-0` and `libsdl2-ttf-2.0-0` from your distro
    (Debian/Ubuntu); equivalents on Fedora/Arch
  - **macOS**: `brew install sdl2 sdl2_ttf`
- FBNeo libretro core available next to the executable
- A legally obtained compatible ROM zip supplied by the user
- Optional backend services for online features
- Optional Discord application for Rich Presence

## Configuration

Copy `.env.example` to `.env` for local development and fill in your private
values:

```env
FREEPLAY_SIGNALING_URL=https://your-signaling-service.example.com
FREEPLAY_STATS_URL=https://your-stats-service.example.com
FREEPLAY_USERNAME=Player
FREEPLAY_DISCORD_CLIENT_ID=your-discord-application-id
FREEPLAY_DISCORD_WEBHOOK_URL=
```

`.env` is ignored by git. Keep real service URLs, Discord IDs, and webhook URLs
out of commits. The in-app Settings screen also stores the saved player name and
optional stats email in `config.toml`.

`config.toml` is also ignored because it can contain local controller bindings
and private webhook settings. The app can still run with defaults, and user
configuration should stay local.

## Build

```powershell
cargo check
cargo build --release
cargo run -- --doctor
```

The release executable is written to:

```text
target\release\freeplay.exe
```

The Windows icon resource is embedded when the Windows SDK resource compiler is
available. The app also sets the runtime SDL window icon from `app_icon.bmp`.

## Build FBNeo Core

FBNeo is open source, but its license is non-commercial. Do not sell packages
containing FBNeo, and do not commit compiled cores to this repo.

Build the libretro core locally:

```powershell
.\tools\build-fbneo-windows.ps1
```

```bash
./tools/build-fbneo-linux.sh
./tools/build-fbneo-macos.sh
```

The scripts clone/update `vendor/FBNeo`, build the libretro target, and copy
the result into `cores/`:

```text
Windows: cores\fbneo_libretro.dll
Linux:   cores/fbneo_libretro.so
macOS:   cores/fbneo_libretro.dylib
```

Both `vendor/` and `cores/` are ignored by git.

## Package

```powershell
.\package.ps1
```

```bash
./package-linux.sh
./package-macos.sh
```

The package script creates `dist\freeplay-gametalk-v<version>.zip` and copies the
executable, runtime DLLs, media assets, app icon, registry helper, and an empty
`roms\` folder. ROM files are intentionally not included.

## Release Automation

GitHub Actions includes `.github/workflows/release.yml`. It now produces
release packages for **Windows, Linux, and macOS** in parallel:

- Windows: `freeplay-gametalk-v<version>.zip`
- Linux: `freeplay-gametalk-v<version>-linux.tar.gz`
- macOS: `freeplay-gametalk-v<version>-macos.tar.gz`

Regular pushes and pull requests also run CI and upload a Windows package
artifact from the **CI** workflow. Use a version tag when you want builds
published as a GitHub Release.

Create and publish a release by pushing a version tag:

```powershell
git tag v0.5.16
git push origin v0.5.16
```

The release workflow fails if the pushed tag does not match the version in
`Cargo.toml`, so every released build gets a new version and every commit build
gets a distinct git revision in the app footer/logged metadata.

You can also run the **Release** workflow manually from GitHub Actions and
provide a tag. The workflow builds packages for all three platforms, uploads
them as artifacts, and attaches the archives to a GitHub Release.

### FBNeo pin

The Linux/macOS jobs build the FBNeo libretro core from
`finalburnneo/FBNeo`. The commit/branch is controlled by the `FBNEO_REF`
env var at the top of `release.yml` (default: `master`). Both build scripts
(`tools/build-fbneo-{linux,macos}.sh`) honor the same `FBNEO_REF` env var
when run locally, so a release build can be reproduced bit-for-bit. CI also
caches `vendor/FBNeo` and `cores/` keyed on this ref so repeat builds skip
the ~10–20 minute FBNeo compile.

Bump `FBNEO_REF` to a specific commit SHA when you want to lock netplay
savestate compatibility across releases.

The automated release does not include ROM files. SDL2 runtime libraries
are NOT bundled in the Linux/macOS archives — users install them via their
package manager (see the per-platform README inside each archive).

## Runtime Files

Expected next to the executable for a packaged build:

```text
freeplay.exe
fbneo_libretro.dll        Windows
fbneo_libretro.so         Linux
fbneo_libretro.dylib      macOS
SDL2.dll
SDL2_ttf.dll
media\
roms\
app_icon.bmp
```

Development builds can also resolve media from `src\media`.

The only tracked TTF is `src\media\mk2.ttf`. Its upstream source is:
https://www.mortalkombatwarehouse.com/site/fonts/mortalkombat2.ttf

## Online Flow

For public release builds:

1. Download the latest `freeplay-gametalk-v<version>.zip` from GitHub Releases.
2. Extract it, then put your legally obtained ROM zip in `roms\`.
3. Run `freeplay.exe`.
4. Open Settings if you want to change your public username or add an optional
   stats email.
5. Optional: choose `Discord Account` in Settings to connect Discord. Your
   browser opens; after authorization, close the browser tab and return to
   Freeplay.
6. Select Find Match, confirm a player name, then enter the queue.
7. During an online match, press `T` to chat, Enter/Start to send, or Esc/B/Back to close.
8. Completed sets save local full-match replays; use Watch Replays to review.
9. Press `F1` to leave the set.

Under the hood:

1. The client discovers its public UDP endpoint.
2. The signaling service pairs compatible players.
3. Relay credentials are minted for the shared room.
4. Both clients send netplay packets to `freeplay-relay`, which forwards
   traffic between the two registered peers.
5. Match results, ghost uploads, and spectator state are posted through the
   configured backend.

Stats and ghost uploads are disabled when `FREEPLAY_STATS_URL` is missing.
Without a Stats Email, ratings are tied to the confirmed player-name identity.
Add the same email on another machine to keep using the same stats identity there.
Discord presence is disabled when `FREEPLAY_DISCORD_CLIENT_ID` is missing.

## Discord Rich Presence

The client publishes activity through the local Discord desktop app when
`FREEPLAY_DISCORD_CLIENT_ID` is configured.

Recommended Rich Presence art asset keys in the Discord Developer Portal:

```text
freeplay
training
netplay
matchmaking
ghost
spectate
```

Asset keys are lowercased by Discord. The client uses:

- Stable elapsed timers for active play/lab/netplay sessions
- State-specific small art
- Live opponent and score text during netplay
- Join secrets for lab spar invites
- Spectate secrets for active online matches

When a player is in an online match, Discord desktop can show a Watch/Spectate
action on their profile card. Clicking it opens Freeplay through
`xband://watch/<session>` and lands on the watch-match screen, which follows
the live score/frame state exposed by the signaling service. This requires the
viewer to have launched Freeplay at least once so the `xband://` protocol is
registered locally, and it depends on Discord desktop Rich Presence being
enabled for both users.

Spectate requests are routed separately from join requests so Discord profile
actions do not accidentally queue the viewer into the match.

## Ghosts

Ghost recordings are written under `ghosts\` next to the executable. When stats
upload is configured and the client has a username identity, completed netplay
recordings can be uploaded for community ghost playback. Local ghost files and
upload queues are ignored by git.

## Repo Hygiene

The repository intentionally ignores:

- ROM zips and generated release zips
- SDL/FBNeo runtime binaries in `src\`
- local logs, save states, ghosts, and tokens
- local match replays
- `.env` and local `config.toml`
- local agent notes and scratch scripts
- `Cargo.lock`

`README.md` is the only markdown file intended to remain tracked.

## Useful Commands

```powershell
cargo check
cargo build --release
cargo run -- --doctor
.\package.ps1
git status --short
git ls-files
```

`freeplay --doctor` checks local setup without opening the SDL window: `.env`,
backend values, ROM zip, FBNeo core, and the mk2 scoreboard font.

## Notes For Contributors

Keep public commits free of ROMs, third-party runtime DLLs, private URLs,
Discord application secrets, webhook URLs, OAuth tokens, logs, ghost recordings,
and generated packages. Prefer configuration through `.env` or local ignored
files.

See `NOTICE.md` for FBNeo and ROM distribution notes.
