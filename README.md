# freeplay-gametalk


![freeplay-image](https://raw.githubusercontent.com/junkwax/freeplay-gametalk/main/screenshot.png)

freeplay-gametalk is the Freeplay client package: a Rust/SDL2 arcade rollback
client with Discord login, matchmaking, ghost recording, profile stats, and a
compact in-game overlay. It wraps an FBNeo libretro core, drives the emulator
frame by frame, and synchronizes two players with GGRS rollback netcode over
direct UDP or TURN relay fallback.

This repository does not include ROM files, emulator DLLs, private service
URLs, OAuth client IDs, tokens, or webhooks.

## Current Build

- Modern SDL menu with Profile, Load Ghosts, Controls, About, and Sign Out via
  hotkey.
- Discord OAuth login with cached local token.
- Discord Rich Presence with join/spectate support when configured.
- Online matchmaking through a signaling service.
- Direct UDP hole punching with TURN fallback for stricter NATs.
- Best-of-session scoring and a high-resolution fight overlay rendered over
  the emulator view.
- Profile page with rating, wins, losses, win rate, match history, and Discord
  avatar support when available.
- Ghost recording, upload/download support, local ghost playback, and practice
  drone mode.
- Package script that creates a distributable zip without ROMs or secrets.

## Requirements

- Windows
- Rust stable toolchain
- SDL2 runtime DLLs available next to the executable when running packaged
  builds
- FBNeo libretro core available next to the executable
- A legally obtained compatible ROM zip supplied by the user
- Optional Discord application and backend services for online features

## Configuration

Copy `.env.example` to `.env` for local development and fill in your private
values:

```env
FREEPLAY_SIGNALING_URL=https://your-signaling-service.example.com
FREEPLAY_STATS_URL=https://your-stats-service.example.com
FREEPLAY_DISCORD_CLIENT_ID=your-discord-application-id
FREEPLAY_DISCORD_WEBHOOK_URL=

# Optional scoreboard font override. Relative paths are resolved from the app cwd.
FREEPLAY_SCOREBOARD_FONT=media/N27-Regular.otf
```

`.env` is ignored by git. Keep real service URLs, Discord IDs, and webhook URLs
out of commits.

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

GitHub Actions includes `.github/workflows/release.yml`.

Create and publish a release by pushing a version tag:

```powershell
git tag v0.4.1
git push origin v0.4.1
```

You can also run the **Release** workflow manually from GitHub Actions and
provide a tag. The workflow builds the Windows package, uploads it as an
artifact, and creates a GitHub Release with the zip attached.

The automated release does not include ROM files. If an FBNeo core is not
available in the CI workspace, the package is still produced as a client
package and users can provide/build the core locally.

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

1. User selects Find Match.
2. Discord login opens if no cached token exists.
3. The client discovers its public UDP endpoint.
4. The signaling service pairs compatible players.
5. Peers attempt direct UDP punching.
6. If direct UDP fails, TURN relay is used when credentials are available.
7. Match results and spectator state are posted through the configured backend.

Stats and ghost uploads are disabled when `FREEPLAY_STATS_URL` is missing.
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

- Stable elapsed timers for active play/training/netplay sessions
- State-specific small art
- Live opponent and score text during netplay
- Join secrets for training spar invites
- Spectate secrets for active online matches

Spectate requests are routed separately from join requests. The full spectator
viewer UI is still a follow-up feature.

## Ghosts

Ghost recordings are written under `ghosts\` next to the executable. When stats
upload is configured and the user is logged in, completed netplay recordings
can be uploaded for community ghost playback. Local ghost files and upload
queues are ignored by git.

## Repo Hygiene

The repository intentionally ignores:

- ROM zips and generated release zips
- SDL/FBNeo runtime binaries in `src\`
- local logs, save states, ghosts, and tokens
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
backend values, ROM zip, FBNeo core, and scoreboard font.

## Notes For Contributors

Keep public commits free of ROMs, third-party runtime DLLs, private URLs,
Discord application secrets, webhook URLs, OAuth tokens, logs, ghost recordings,
and generated packages. Prefer configuration through `.env` or local ignored
files.

See `NOTICE.md` for FBNeo and ROM distribution notes.
