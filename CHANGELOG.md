# Changelog

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
