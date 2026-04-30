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
```

The release executable is written to:

```text
target\release\freeplay.exe
```

If `rc.exe` is not on your PATH, the Windows icon resource will not be embedded
at compile time. The app still sets the runtime SDL window icon from
`app_icon.bmp` when available. To embed the icon, build from a Visual Studio
Developer PowerShell or install the Windows SDK resource compiler.

## Package

```powershell
.\package.ps1
```

The package script creates `dist\freeplay-gametalk-v<version>.zip` and copies the
executable, runtime DLLs, media assets, app icon, registry helper, and an empty
`roms\` folder. ROM files are intentionally not included.

## Runtime Files

Expected next to the executable for a packaged build:

```text
freeplay.exe
fbneo_libretro.dll
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
.\package.ps1
git status --short
git ls-files
```

## Notes For Contributors

Keep public commits free of ROMs, third-party runtime DLLs, private URLs,
Discord application secrets, webhook URLs, OAuth tokens, logs, ghost recordings,
and generated packages. Prefer configuration through `.env` or local ignored
files.
