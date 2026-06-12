# Freeplay Gametalk

![Freeplay gameplay screenshot](https://raw.githubusercontent.com/junkwax/freeplay-gametalk/main/screenshot.png)

Freeplay Gametalk is a ready-to-play rollback arcade client for online matches,
local arcade play, training, replays, stats, and community drones.

Bring your own legally obtained compatible ROM zip. ROMs are not included.

## Public Links

- Latest download: https://github.com/junkwax/freeplay-gametalk/releases/latest
- Public replays: https://junkwax.github.io/freeplay-gametalk/replays/
- Source repo: https://github.com/junkwax/freeplay-gametalk

The replay page loads the live community replay feed first, then falls back to
the static replay list in this repo. The **Watch** button opens the replay in
the installed Freeplay app through the `xband://` protocol.

## Quick Start

1. Download the latest release for your system.
2. Extract the archive somewhere simple, like `C:\Games\Freeplay`.
3. Put your compatible ROM zip in the `roms` folder.
4. Run `freeplay.exe`.
5. Open **Controls** if you want to bind a keyboard or controller.
6. Pick **Arcade** for local play, **Lab** for practice, or **Find Match** for
   online play.

On first launch, Freeplay creates a public player name for you. You can keep it
or change it in **Settings**. Discord login is optional; you can play ranked
without it.

## What You Get

- Online matchmaking with rollback netcode.
- Public UDP relay support for players behind home routers or strict NATs.
- Player profile, rating, wins, losses, and recent match history.
- Local and public replay review.
- Automatic upload of completed online match replays when the stats service is
  configured.
- Training Lab with dummy behavior, resets, hitboxes, punish practice, clips,
  and drone playback.
- Arcade mode for normal local play without Lab helpers.
- Controller-friendly menus.
- CRT/video filters and render profiles.
- Discord Rich Presence, join, and spectate support when configured.

## Playing Online

1. Choose **Find Match**.
2. Confirm your public name.
3. Wait for an opponent.
4. Play the set.
5. When the game ends, Freeplay saves the replay and posts the result to the
   stats service.

Useful online buttons:

- `T`: open chat.
- `Enter` or controller `Start`: send chat.
- `Esc`, controller `B`, or controller `Back`: close chat.
- `F1`: leave the online set.
- `F11`: show or hide network stats.
- `R` or controller `Y` after a game: review the replay that was just saved.

## Watching Replays

You have two replay paths:

- In the app: open **Replays** to watch your local replays and public community
  replays.
- On the web: open https://junkwax.github.io/freeplay-gametalk/replays/

The public page shows uploaded community replays. Press **Watch** to launch
Freeplay and review the match in the app, or **Download** to save the replay
file.

Replay review controls:

- `Space`, `Enter`, or controller `Start`: pause or resume.
- `.` or controller `A`: step one frame while paused.
- `Left`/`Right` or D-pad left/right: jump 5 seconds.
- `Up`/`Down` or D-pad up/down: change replay speed.
- `PageUp`/`PageDown` or controller `LB`/`RB`: jump between replay events.
- `F` or controller `Guide`: cycle event filters.
- `Esc`, controller `B`, or controller `Back`: return to Replays.

## Menus And Settings

- `Up`/`Down` or controller D-pad: move.
- `Enter`, numpad `Enter`, controller `A`, or controller `Start`: select.
- `Esc`, controller `B`, or controller `Back`: go back.
- `Left`/`Right` or D-pad left/right: change settings and tabbed options.

Settings lets you change your player name, optional stats email, renderer,
video filter, display mode, controller bindings, and Discord account link.

## Lab And Drones

Use **Lab** when you want to practice instead of queueing online.

- `F2`: hitboxes.
- `F3`: infinite health.
- `F4`: freeze timer.
- `F5`: cycle dummy behavior.
- `Ctrl+F5`: record a short dummy loop.
- `F6`: load reset.
- `F7`: save reset.
- `F9`: start or stop local drone recording.
- `F10`: Punish Trainer, or reactive drone behavior during drone playback.
- `F11`: Lab assist panel.
- `F12`: play against the logic-driven P2 drone.
- `Ctrl+R`: record an MP4 clip.

Drone files are still stored under the `ghosts` folder for compatibility with
older builds, but the app labels them as drones.

## Troubleshooting

Run Doctor from the menu with `Shift+D`, or from a terminal:

```powershell
freeplay.exe --doctor
```

Common fixes:

- If the game does not start, make sure the ROM zip is inside `roms`.
- If online play does not start, check that the release includes the bundled
  `.env` file and that your firewall allows Freeplay.
- If controller input feels wrong, open **Controls** and rebind.
- If the public replay page is empty, there may not be uploaded replays yet.
- If **Watch** on the replay page does nothing, launch Freeplay once so Windows
  registers the `xband://` link handler.

## What Is Not Included

This repo and its release packages do not include ROM files, private service
tokens, OAuth secrets, webhooks, or paid/commercial emulator packages.

## For Developers

Install Rust stable and SDL2 development/runtime libraries.

Windows release packages include the needed SDL runtime DLLs. Linux and macOS
users install SDL2 through their package manager.

Basic commands:

```powershell
cargo check
cargo build --release
cargo run -- --doctor
```

The release executable is written to:

```text
target\release\freeplay.exe
```

Package a Windows release:

```powershell
.\package.ps1
```

Package Linux or macOS:

```bash
./package-linux.sh
./package-macos.sh
```

The package scripts create archives in `dist` and include an empty `roms`
folder. ROMs are intentionally not packaged.

## FBNeo Core

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

The scripts clone or update `vendor/FBNeo`, build the libretro target, and copy
the result into `cores`:

```text
Windows: cores\fbneo_libretro.dll
Linux:   cores/fbneo_libretro.so
macOS:   cores/fbneo_libretro.dylib
```

Both `vendor` and `cores` are ignored by git.

## Configuration

Release builds include public defaults. Local developers can copy
`.env.example` to `.env` and set private service values:

```env
FREEPLAY_SIGNALING_URL=https://your-signaling-service.example.com
FREEPLAY_STATS_URL=https://your-stats-service.example.com
FREEPLAY_USERNAME=Player
FREEPLAY_DISCORD_CLIENT_ID=your-discord-application-id
FREEPLAY_DISCORD_WEBHOOK_URL=
```

`.env` and `config.toml` are ignored by git. Keep private URLs, tokens, Discord
IDs, and webhook values out of commits.

## Public Replay Page

The web replay page lives in `docs/replays/index.html` and is served by GitHub
Pages from the `main` branch's `/docs` folder:

```text
https://junkwax.github.io/freeplay-gametalk/replays/
```

The page loads:

1. Live uploads from `https://freeplay-stats-681135711161.us-central1.run.app/replays/list`.
2. Static fallback entries from `docs/replays/replays.json`.

To add a hand-picked replay to the static fallback, put the `.ncrp` file under
`docs/replays/files` and add its player names, score, outcome, date, and file
path to `docs/replays/replays.json`.

## Online Services

Freeplay uses three public service pieces:

- Signaling service: account/session, matchmaking, spectate, incidents, and
  match result forwarding.
- Relay server: UDP packet forwarding for online play.
- Stats service: leaderboard, profiles, match history, replay uploads, replay
  downloads, and drone uploads.

The relay server is only for live UDP netplay packets. Persistent replay and
drone storage belongs to the stats service.

## Release Automation

GitHub Actions builds release packages for Windows, Linux, and macOS when a
version tag is pushed.

```powershell
git tag v0.7.7
git push github v0.7.7
```

The release workflow checks that the tag matches the version in `Cargo.toml`.
Automated releases do not include ROM files.

## Repo Hygiene

The repository intentionally ignores ROM zips, generated release archives,
runtime binaries in `src`, local logs, save states, drones, replay files,
tokens, `.env`, and local `config.toml`.

See `NOTICE.md` for FBNeo and ROM distribution notes.
