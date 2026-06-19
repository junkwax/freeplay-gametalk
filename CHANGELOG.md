# Changelog

## 0.7.22 - 2026-06-19

### Fixed

- Replays, ghosts, and shared drones failed to download with a "gzip header"
  error. The stats service serves these binary files with chunked transfer
  encoding, which the client wasn't decoding — it fed the chunk framing
  straight into the gzip decoder. The downloader now de-chunks first.

### Changed

- King-of-the-hill lobby matches are now FT1 (first to one game). The winner of
  a single game stays and the queue rotates every game, instead of playing a
  best-of set before rotating.

## 0.7.21 - 2026-06-19

### Changed

- Name claiming is now a real reservation. When you claim a name it's
  atomically reserved to you in a global registry (case-insensitive), so two
  new players can no longer end up with the same name. Re-claiming your own
  name just re-affirms it, and changing your name frees the old one.

### Fixed

- Crash and disconnect reports now upload even when you're not signed in.
  Incidents are attributed to your Discord account when available and otherwise
  to a stable per-install id, so problems get captured without depending on
  players being logged in or shipping logs.

## 0.7.20 - 2026-06-18

### Added

- First-run name claim. The auto-generated Wu-style name is your identity, but
  the first time you go online you now get a "Choose your name" screen to keep
  or change it — and it's checked for availability before it's claimed. After
  you claim a name once, going online never re-prompts.

### Changed

- The home-screen "FREEPLAY" wordmark is larger and left-aligned.

### Fixed

- Online features (username availability, leaderboards, profiles) now work out
  of the box without a hand-written `.env`: config falls back to the bundled
  `.env.public`, per key, so `FREEPLAY_STATS_URL`/`FREEPLAY_SIGNALING_URL` are
  always resolved. Previously a missing `.env` left the stats service
  unconfigured and the name check couldn't run.

## 0.7.19 - 2026-06-18

### Added

- King-of-the-hill lobbies now play matches automatically. When it's your turn
  the client connects to the paired opponent and starts netplay; when the set
  ends you return to the lobby (winner stays, loser re-queues) instead of the
  normal end-of-session screen.
- Ready check before each lobby match. The next two players pair up visually
  and the challenger gets a 10-second countdown to confirm — press ENTER to
  ready up. If they don't confirm in time they drop to spectating and the next
  player in the queue is pulled up. The champion (the player staying on) is
  auto-ready.

### Fixed

- Lobby queue rotation no longer happens mid-set: the winner-stays hand-off now
  waits for the whole best-of-N set to finish rather than rotating after the
  first game.

## 0.7.18 - 2026-06-18

### Added

- Private king-of-the-hill lobbies. The Lobbies tab now offers "Create Public
  Lobby", "Create Private Lobby", and "Join by Invite Code". A private lobby is
  hidden from the public browser and shows a short invite code; share the code
  and others join with "Join by Invite Code".

## 0.7.17 - 2026-06-18

### Added

- King-of-the-hill lobbies: the Lobbies tab now creates and joins persistent
  lobbies with a play queue. The lobby screen shows the current match, the
  "up next" queue with your position, and your queue/spectate status; you can
  toggle between queuing and spectating, and leaving destroys the lobby when
  it's empty. (The automatic winner-stays match hand-off is the next step.)

## 0.7.16 - 2026-06-18

### Added

- The lobby presence list and Players section now show each player's ranking
  next to their name (e.g. `reptilefan (1403)`) when the server has a rating.

## 0.7.15 - 2026-06-18

### Changed

- Reduced the Online hub font size and title scale for a more readable, less
  oversized layout, and left-aligned the hub title.
- The chat on-screen keyboard now only appears when navigating with a
  controller; keyboard users just type. The chat input bar is clearly
  highlighted (accent border + caret) when focused.

### Added

- Added a quick common-phrase strip above the chat input (GG / GGS / WP /
  one more? / rematch? / nice / lag?) — click to drop it into your message.
- Added a `--test-screen` debug flag (e.g. `--test-screen online:chat`,
  `controls`, `main`; plus `--test-osk`) that jumps straight into a screen with
  sample data for layout testing.

## 0.7.14 - 2026-06-17

### Added

- Added direct player challenges to the Online hub. A new "Players" section
  lists everyone in the lobby; select a player and pick a format
  (Unranked VS / FT3 / FT5 / FT10) to send a challenge. You can also right-click
  a name in chat or the players list to open the format chooser, then left-click
  a format to send.
- Incoming challenges raise an accept/decline prompt (Enter accepts, Esc
  declines).

## 0.7.13 - 2026-06-17

### Added

- Added an on-screen keyboard on the Online → Chat section so controller players
  can type without a physical keyboard: the d-pad moves the key cursor, A
  presses a key, and a SEND key posts the message. Physical keyboard and Enter
  still work.

### Changed

- Finding or creating an online match no longer re-prompts for a name — it uses
  the name you already chat with (set in Settings, or an autogenerated default).

### Fixed

- Local builds and `package.ps1` now report the released version (derived from
  the git tag) instead of the committed Cargo.toml version.

## 0.7.12 - 2026-06-17

### Changed

- Redesigned the Online hub into a left nav rail (Play / Chat / Lobbies / Watch)
  with a content pane and a clear rail-vs-content focus model. Up/Down switch
  section on the rail and move within content; Right/Enter dive in; Left/Esc
  step back out. Footer hints follow the current focus.
- General lobby chat now shows colored sender names, scrollback, and an
  "Online (N)" presence list, and chat typing works on the Chat section.

### Added

- Lobbies tab browses live public rooms and joins the selected one. "Create
  Lobby" now hosts a public room with the chosen Play format and waits for a
  challenger to join from their lobby browser.

### Fixed

- The ROM-missing banner no longer names a specific file; it reads "No valid
  .zip found in the roms folder".
- Release builds now stamp the crate version from the git tag, so a tagged
  release always reports the tag's version.

## 0.7.11 - 2026-06-16

### Added

- Added an Online Hub menu with General, Lobbies, Ranked, and Watch tabs,
  including ranked challenge formats (Unranked VS, Ranked FT3/FT5/FT10) and a
  Spectate/Watch flow.
- Added an in-match network stats overlay (rollback depth, save/load counts,
  and MK2 performance sampling).
- Added persistent net-match settings and completed-match tracking.
- Added audio-recovery ramping to smooth the audio tail after a stall or
  rollback gap.
- Added a dedicated high-resolution frame timer (Windows `winmm` backed) for
  steadier pacing.
- Added native title-bar dragging for the client window on Windows.

### Changed

- Expanded launch CLI, doctor diagnostics, and incident reporting.

## 0.7.6 - 2026-06-12

### Added

- Added hardware render profiles with local renderer probing and automatic
  profile recommendation on first launch.
- Added OpenGL CRT shader filters: balanced CRT Shader, heavier CRT Arcade GL,
  and sharper CRT PVM GL.
- Added a `Ctrl+F10` render debug overlay with FPS, renderer, active filter, and
  online-overlay status.
- Added `appicon.png` as the runtime window icon source and refreshed packaged
  icon assets.

### Changed

- The application window title now shows `FREEPLAY v<version>`.
- The Windows executable description now says `Freeplay - Netplay Client`.
- Netplay player-name overlays appear during the round intro again instead of
  waiting until the fighters have fully spawned.

### Fixed

- Restored OpenGL state after CRT shader rendering so SDL menu and netplay
  overlays continue drawing correctly after leaving arcade or switching views.

## 0.7.5 - 2026-06-12

### Changed

- macOS releases now build Intel packages on GitHub's current
  `macos-15-intel` runner and publish both per-architecture `.tar.gz` archives
  and `.dmg` images.

### Fixed

- Renamed the Wu-name retry variant field so CodeQL no longer flags the
  username generator as using a hard-coded cryptographic nonce.

## 0.7.4 - 2026-06-12

### Added

- Added an Input Delay setting to the Options menu, backed by `config.toml`.
  Netplay now reads the persisted delay value when starting direct or relay
  matches, making rollback/input-latency tuning available without editing files.

## 0.7.2 - 2026-06-10

### Fixed

- Guest players without a registered email now accumulate stats persistently.
  A stable device ID is generated on first run and stored in `config.toml`;
  it is sent to the matchmaking server so every player gets a consistent profile
  across sessions without needing to register an account.

## 0.7.1 - 2026-06-10

### Fixed

- Fixed the Find Match name flow getting stuck on a "Checking name" screen for
  fresh installs. The bundled environment no longer seeds a fixed username, so
  a new install again auto-generates a Wu-style name and shows the confirm
  screen before the first match instead of silently checking a shared default.

### Added

- Find Match now picks an auto-generated name the stats service reports as
  available before offering it for confirmation, and still re-checks
  availability when you set a custom name.

### Changed

- macOS releases now ship both Apple Silicon (arm64) and Intel (x86_64)
  builds as separate archives.

## 0.7.0 - 2026-06-10

### Added

- Added a configurable GGRS input delay (`input_delay` in `config.toml`,
  default 3, range 0–8). Higher values trade input latency for fewer rollbacks
  on high-latency connections; the value is now threaded through direct UDP and
  TURN-relay netplay sessions.

### Fixed

- Hardened netplay desync detection by mixing an MK2 sync word into the GGRS
  savestate checksum, so divergent states are caught earlier instead of
  surfacing as a late desync.
- Corrected the MK2 SYSTEM_RAM addresses (game state, player/match win counts,
  round number, winner status, player health, hitbox-overlay flag, freeze
  timer, and player positions) to match the current ROM map. This restores
  accurate live scoring, the hitbox overlay, health/freeze trainer pokes, and
  Lab/drone positioning.

## 0.6.1 - 2026-05-21

### Fixed

- Fixed controller direction merging so D-pad and analog stick bindings no
  longer cancel each other when both are mapped to the same movement action.

## 0.6.0 - 2026-05-10

### Added

- Added Replay Review Mode for saved full-match replays with pause/resume,
  one-frame stepping, 5-second seek jumps, live P1/P2 input display, frame/time
  readout, and a timeline with round, hit, and match-end markers.
- Added replay review speed controls and previous/next marker jumps for quickly
  navigating long sets.
- Added a Replay Review event sidebar that lists round starts, hits, and match
  end markers with the current event highlighted.
- Added replay clip export: set in/out marks during review and press `Ctrl+R`
  to export that replay segment through the MP4 clip pipeline.
- Added Replays browser actions for deleting selected online replays, opening the
  replay folder, and showing readable replay duration/date metadata.
- Added persistent replay bookmarks and replay notes. Bookmarks appear on the
  review timeline/sidebar, marker jumps include them, and notes show in the
  Replays browser.
- Added Replay Review event filters for narrowing the timeline, sidebar, and
  previous/next event jumps to hits, bookmarks, rounds, or match-end markers.
- Added Replay Review learning auto-markers for first hit, big damage,
  low-health scrambles, and P1/P2 round wins, plus a `LEARN` event filter.
- Added a separate main-menu `Arcade` entry under Find Match for normal local
  coin/start play without Lab dummy input, Lab assist, or training pokes.
- Added a compact Lab submenu for Start Lab and Load Ghosts.
- Added online match replay capture so completed Find Match sets can save a
  reviewable `.ncrp` replay.
- Added an opt-in `F11` network stats overlay for matchmaking and online play,
  showing FPS and live ping once connected.
- Added the `F11` stats hint directly to the Find Match queue screen.
- Added a post-online-match replay shortcut on the match-ended screen (`R` or
  controller `Y`) when an online replay was saved.
- Added richer online diagnostics to the `F11` stats panel: quality, rollback
  frames, load count, frames behind, and send rate.
- Added Lab dummy controls. Lab now defaults to a local 2P dummy, `F5` cycles
  stand/crouch/block/crouch-block/jump/jump-in/reversal/throw-tech/wakeup-block/off
  modes, and `OFF` preserves the old single-player CPU behavior.
- Added Lab dummy presets for jump-in, reversal mash, throw-tech, and wakeup
  block practice.
- Added Lab dummy loop recording with `Ctrl+F5`: record P2 live controls for up
  to five seconds, loop them as the dummy, and clear the loop with `Shift+F5`.
- Added Lab quick position reset on `Ctrl+F6`, cycling midscreen, P2-corner,
  and P1-corner placements without leaving the match.
- Added three Lab reset slots. `F6/F7` load/save the active slot, `Ctrl+F7`
  cycles slots, and the Lab assist panel marks saved slots with `*`.
- Added Punish Trainer with `F10`: recorded dummy loops arm a punish window and
  score `PUNISH`, `LATE`, `BLOCKED`, or `MISSED` from P2 health and P1 attacks.
- Added passive Lab damage tracking for last damage, hit count, attempts, and
  best damage in the Lab assist panel.
- Changed the Find Match name gate so generated names only need first-run
  confirmation; accepted/custom names skip the blinking name box on later
  queues unless the name check fails.
- Changed the replay browser back to a top-level `Replays` main-menu item and
  scoped it to online match replays.
- Changed Arcade/Lab/Ghost local play to stop creating Replays entries.
- Changed Arcade to suppress the extra scorebar overlay so it feels like the
  normal arcade path.
- Changed the in-game Lab hotkeys panel to a compact F-key-only quick reference;
  modifier shortcuts remain documented in About/README.
- Changed the About and Settings screens to use tighter reference text instead
  of large bottom note blocks.
- Changed input display direction formatting so Up displays as `U`, including
  controller conflict cases that previously collapsed to neutral.

### Fixed

- Fixed Arcade starts after Lab so the core resets back to a clean arcade boot
  instead of inheriting the last Lab match state.
- Fixed the lower-right `Logged in as` badge disappearing on the Find Match
  matchmaking screen.

## 0.5.21 - 2026-05-10

### Fixed

- Updated the Lab hitbox overlay flag to the current `mk2.map` derived
  `f_colbox` RAM offset (`0x25772`), matching `w@0x225772=1` from
  `hitbox_info.py`.

## 0.5.20 - 2026-05-10

### Fixed

- Added proper header-to-body padding in the in-game Lab hotkeys and P1 input
  panels so the first row no longer overlaps the yellow headers.
- Restored the Lab hitbox flag to the old working `0x2576E` RAM slot used by
  earlier builds and the netplay reset path.

## 0.5.19 - 2026-05-10

### Fixed

- Fixed the Lab hitbox overlay poke to match the 68000 word write used by the
  debugger, restoring the green MK2 collision boxes when `F2` is enabled.
- Changed the Lab assist body rows under `LAB HOTKEYS` and `P1 INPUTS` to use
  the cleaner menu font style from the About screen.

## 0.5.18 - 2026-05-10

### Fixed

- Anchored the Lab hotkeys and P1 input-history panels together in the lower
  right corner and prevented hotkey labels or frame counts from overlapping.
- Rendered in-game toast feedback during play so `F2` and other Lab toggles
  visibly report their new state.

### Changed

- Added live `ON`/`OFF` state text for Lab hitboxes, health, and timer in the
  Lab assist hotkey panel.

## 0.5.17 - 2026-05-10

### Changed

- Reassigned Lab hotkeys into ordered pairs: `F6/F7` load/save Lab reset
  points and `F8/F9` load/save local ghosts.
- Reserved `F1` for online match quit only; `F2` is now the single hitbox
  toggle.
- Moved the rewind determinism diagnostic from `F9` to `Shift+F9`.

## 0.5.16 - 2026-05-10

### Fixed

- Restored Lab hitbox display by poking the correct MK2 `f_colbox` RAM offset
  and applying trainer flags before the emulated frame runs.
- Made broken shared ghost downloads fail cleanly by removing unavailable
  remote entries from Load Ghosts instead of leaving a raw HTTP 404 on screen.

### Changed

- Tightened the in-game Lab assist panel with lower placement, two-column
  hotkeys, grouped save/load actions, grouped ghost actions, and capped
  input-history frame labels.

## 0.5.12 - 2026-05-07

### Added

- Added optional Discord account linking back into Settings. Choose
  `Discord Account` to open the Discord OAuth flow, cache the token locally,
  and use that identity for matchmaking/stats/profile features.

### Changed

- Matchmaking now prefers a connected Discord account when one is available,
  otherwise it falls back to username-based guest sign-in. Discord remains
  optional and is no longer required for Find Match.

## 0.5.11 - 2026-05-07

### Added

- Added username-based guest sign-in for online matchmaking. Players can set
  the public name shown to opponents in Settings and match without Discord
  OAuth.
- Added an optional Stats Email field in Settings. When filled in, guest stats
  use the email-derived identity so ratings and history can follow the same
  player across machines.

### Changed

- Discord is no longer required for Find Match or Join/Spectate links. Discord
  remains available as optional Rich Presence when configured.

## 0.5.10 - 2026-05-07

### Fixed

- Discord Rich Presence join and spectate callbacks now route to the
  correct in-app flows. Join opens the spar/join path; Spectate opens the
  watch-match screen.

### Changed

- Discord Rich Presence text now calls out MK2 ranked queue/match state,
  opponent name, and set score more clearly.
- README onboarding now documents the public online flow and Discord watch
  behavior for desktop users.

## 0.5.9 - 2026-05-07

### Added

- Added a small relay-backed in-match chat overlay. Press `T` during
  online play, type a short message, then press Enter to send.

### Changed

- Moved Find Match to the top of the main menu, with Practice directly
  underneath it.
- Load Ghosts now shows privacy-safe recording labels and timestamps
  instead of exposing peer endpoint details from local filenames.

## 0.5.8 - 2026-05-07

### Fixed

- Ranked results now include a per-session `match_index`, so multiple
  completed games in one netplay set can each count on the leaderboard
  instead of only the first completed game being recorded.
- Ghost uploads now mark successfully uploaded local recordings and skip
  them during later backfill/retry scans. This prevents an already-uploaded
  ghost from being retried and requeued after a later auth failure.

## 0.5.7 - 2026-05-07

### Added

- Installed a process panic hook that uploads a `panic` incident when a
  logged-in client crashes from a Rust panic. The signaling server then
  stores the private log bundle in GCS and opens/comments on the
  matching GitHub issue.

## 0.5.6 - 2026-05-07

### Fixed

- Windows CI now downloads the official libretro buildbot
  `fbneo_libretro.dll` artifact instead of building from upstream source.
  The v0.5.5 Windows release job failed because the cloned source tree no
  longer had the expected `src/burner/libretro` build path.

## 0.5.5 - 2026-05-07

### Fixed

- Windows release archives now fail packaging if `fbneo_libretro.dll` is
  missing, and GitHub Actions builds the Windows FBNeo core before
  packaging. This prevents public zips from crashing or exiting when
  Find Match hands off into the emulator path.
- Relay sessions now use a 10s GGRS disconnect timeout instead of the
  old 1.5s direct-UDP timeout, giving internet relay setup time to
  synchronize.
- Relay setup no longer aborts solely because the relay control ACKs are
  missing. Incidents showed one client reaching the relay repeatedly
  while not receiving the relay's tiny control packets; the client now
  proceeds to GGRS after the bounded relay warmup and logs whether
  `REGISTERED`/`PEER_READY` were observed.
- Added `tools/test-relay-local.ps1`, a one-machine relay smoke test
  that starts the sibling relay, simulates both roles, and verifies
  control packets plus bidirectional DATA forwarding.

## 0.5.4 - 2026-05-06

### Fixed

- Matchmaking now uses the relay whenever relay credentials are present,
  instead of letting each client independently decide whether direct UDP
  succeeded. The old flow could split-brain: one peer would start GGRS
  direct while the other peer routed through `freeplay-relay`, so both
  sat in Synchronizing until timeout.
- `RelaySocket::connect` now waits for the relay's `PEER_READY` signal
  before starting GGRS, with a 20s setup window. This prevents one peer
  from starting GGRS while the partner is still reaching relay setup.

## 0.5.3 - 2026-05-06

### Fixed

- Relay/GGRS setup failures after matchmaking now upload an incident to
  the signaling server instead of only showing the local error screen.
  The incident includes the room id, session id, peer endpoint, role,
  ROM hash, and `freeplay-net.log` tail.

## 0.5.2 - 2026-05-06

### Fixed

- Actually track `.env.public` in git. v0.5.1 updated the packagers to
  fall back to `.env.public`, but `.gitignore` still excluded `.env.*`
  except `.env.example`, so CI never received the public defaults file
  and the Windows archive still shipped without a runtime `.env`.
- `.env.example` now contains the public production defaults instead of
  placeholder URLs, and packagers can use it as a final fallback.
- Windows package README now reflects the current fresh-download flow:
  ROM first, then run the app; no manual env setup is needed for public
  matchmaking.

## 0.5.1 - 2026-05-06

### Fixed

- Release archives now bundle a working `.env` next to the binary with
  public defaults baked in (signaling URL, Discord client ID, stats URL).
  Previously a clean download of v0.5.0 hit "FREEPLAY_SIGNALING_URL is
  not configured" — users had to know to create `.env` manually with
  values copied out of band. New users can now download, drop their
  ROM in `roms/`, and click Find Match.
- Added tracked `.env.public` with the public values. Local `.env`
  still wins if present (so dev/self-host overrides are unchanged);
  CI builds fall back to `.env.public`. Discord webhook URL is always
  blank in the bundled `.env` — that's the one private value.

## 0.5.0 - 2026-05-06

### Changed

- Replaced TURN/coturn relay path with a custom UDP relay
  (`freeplay-relay`). coturn refused to install permissions for its own
  external IP (`is_my_address` hardcoded check), so the TURN-to-TURN
  routing the v0.4.7-v0.4.9 design needed was unworkable without
  forking coturn. The new relay is a 250-line UDP forwarder that just
  pairs clients on a room ID — no STUN, no permissions, no NAT
  traversal at all (both clients send to the relay, relay forwards to
  partner).

- Removed `src/turn_relay.rs` and `src/turn_socket.rs` (~960 lines).
  Replaced with `src/relay_socket.rs` (~250 lines) speaking the new
  protocol. Same `NonBlockingSocket<SocketAddr>` trait so GGRS plumbing
  is unchanged.

- Signaling server's MatchInfo.turn payload still has uri/username/
  password/ttl_secs (wire format unchanged), but contents now describe
  relay credentials: uri = `relay://...`, username =
  `<role>:<expiry>:<room_id>`, password = hex HMAC-SHA256.

### Fixed

- Cross-NAT matches between two real machines now connect. Previously
  hole-punch + TURN both failed for residential ISP NAT pairs (Cox/
  Comcast etc.). The relay sidesteps NAT entirely.

## 0.4.9 - 2026-05-06

### Changed

- TURN address-exchange diagnostics now go to `freeplay-net.log` (and
  therefore the incident bucket) instead of stdout-only. Without this
  the bucket couldn't tell us whether the exchange even ran on a
  failure — we'd see `[session] ready (TURN): peer @ <STUN endpoint>`
  with no clue whether that was a successful exchange routing through
  coturn (impossible — STUN endpoints aren't on coturn) or a silent
  fallback (correct interpretation).

- TURN exchange deadline raised from 5s to 15s, polled every 200ms.
  TURN allocation alone can take 1-3s per side; the old 5s budget
  raced both peers' handshakes and timed out before either had time
  to publish.

## 0.4.8 - 2026-05-06

### Fixed

- Windows release archive now bundles `freetype.dll` and the rest of
  vcpkg's SDL2_ttf transitive dependencies (zlib, bzip2, libpng,
  brotli). v0.4.7 only shipped `SDL2.dll` and `SDL2_ttf.dll`, so a
  clean Windows install failed at launch with "freetype.dll was not
  found". Dev machines didn't see this because they had freetype on
  PATH from another install.

  No client-code changes — same v0.4.7 binary, just a complete
  Windows DLL bundle.

## 0.4.7 - 2026-05-06

### Fixed

- TURN sessions now route through the partner's TURN-relayed address
  rather than the partner's STUN endpoint. The previous design sent
  packets to the peer's NAT-mapped public IP via TURN's Send Indication;
  coturn forwarded them as raw UDP, but the receiving NAT dropped the
  unsolicited packets. GGRS sat at `Synchronizing` for 10 frames then
  timed out — the failure mode that the v0.4.5 / v0.4.6 incident bucket
  captured.
  
  The new flow uses the signaling server's `/match/turn-ready` and
  `/match/peer-relay` endpoints (which were always there, just unused
  by the client). After both sides publish their relayed addresses,
  each TurnSocket re-targets itself at the partner's relayed addr.
  coturn then routes between its own two allocations internally,
  bypassing the receiving side's NAT entirely. 5-second deadline; if
  the partner doesn't publish in time, falls back to the legacy STUN-
  endpoint path (better than aborting).

## 0.4.6 - 2026-05-06

### Fixed

- `/match/cancel` now hits the correct route. The client was POSTing to
  `/match/cancel/<session_id>` (a 404), so cancels silently failed. The
  server resolves the session from the JWT, no path param needed. Stale
  prior sessions on the server were causing "match doesn't happen" /
  ghost-match symptoms when players re-queued.

### Added

- Auto-incident upload on failed matches. When a match fails to start
  (hole-punch failure, TURN fallback failure) or ends abnormally
  (peer disconnect, GGRS timeout, score-mismatch rejection), the client
  now POSTs an incident JSON to the signaling server with the last
  256 KB of `freeplay-net.log`, role, frames advanced, ROM hash, and a
  short kind tag. The server stores these in
  `gs://quarterframe-freeplay-incidents/YYYY/MM/DD/<id>.json` for
  offline investigation.
- Pre-LFG self-cleanup: every Find Match starts by calling
  `/match/cancel` (best effort) so a previously-matched session that
  ended unexpectedly doesn't linger and trip up the next pairing.

## 0.4.5 - 2026-05-06

### Fixed

- Spectator frame push (`/spectate/push/:sid`) now sends the cached JWT
  in `Authorization: Bearer ...`. The signaling server was updated in the
  same release window to require auth on this endpoint; v0.4.4 silently
  failed every push with 401, so the live-match scoreboard never
  reflected in-progress matches.
- Ghost upload (`/ghosts/upload`) now sends the JWT. Same root cause —
  the stats server now verifies the uploader's identity from the JWT
  `sub` rather than the trusted-but-spoofable `X-Freeplay-Discord-Id`
  header. v0.4.4 ghost uploads were 401-bouncing into the retry queue
  forever after the server upgrade.

### Changed

- `drain_upload_queue` now skips entirely when no JWT is cached, instead
  of attempting unauthenticated retries that would just re-enqueue. The
  queue is drained again as soon as Discord login completes, so nothing
  is lost.

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
- macOS workflow exports `LIBRARY_PATH`/`CPATH`/`PKG_CONFIG_PATH` from the
  Homebrew prefix (`/opt/homebrew` on Apple Silicon runners) so the
  release link step actually finds `-lSDL2`/`-lSDL2_ttf`. Previously the
  cargo build failed with `library 'SDL2' not found` even though brew
  install had succeeded.

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
