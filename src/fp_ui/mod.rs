//! New UI: a controller-only, 1920x1080-logical-canvas screen set
//! recreating `freeplay-frontend/FREEPLAY Arcade.dc.html` in SDL2
//! primitives. Lives entirely alongside the legacy `crate::menu` screens —
//! gated behind `Config::new_ui` (default off = byte-for-byte legacy
//! behavior). See `AppState::FpUi` in `crate::menu` for how the two connect
//! in `main.rs`.
//!
//! Only the screens this module actually draws are listed in `FpScreen`;
//! everything else (Lab, Replays, Ghost Select, Profile, Controls, and the
//! non-Quick-Match Online tabs) stays on `crate::menu::MenuScreen` per
//! `freeplay-frontend/uploads/ui-handoff-legacy-screens.md` — reached via
//! `menu::main_menu_state`'s round trip and `FpResult::ActivateMainItem`
//! delegating to the legacy `nav_accept`, not reimplemented here.

pub mod about;
pub mod bandwidth;
pub mod chrome;
mod ghost_select;
pub mod geometry;
pub mod input;
mod lab_menu;
pub mod layout;
mod lobby;
mod main_menu;
mod play_menu;
mod profile;
mod quit;
pub mod rankings;
mod replay_select;
pub mod settings;
pub mod theme;

pub use input::{event_to_fp_nav, FpNav};
pub use layout::Scale;
pub use settings::SettingsFields;

use crate::menu::{LobbyPreview, MAIN_SETTINGS_INDEX as LEGACY_SETTINGS_INDEX};
use crate::font::FpFontCache;
use sdl2::render::Canvas;
use sdl2::video::Window;

/// fp_ui's own Main Menu cursor space — 4 rows (Play/Online/Rankings/
/// Settings), decoupled from `crate::menu::MAIN_ITEMS`'s 9-item legacy
/// ordering. Only used for `FpScreen::Main { cursor }` and "return to Main
/// at row X" transitions; never sent to `main.rs` as an `ActivateMainItem`
/// payload (those still carry *legacy* indices — see `LEGACY_*_INDEX`
/// below).
///
/// The mockup's own 5th row, "Network News", is hidden for now rather than
/// removed outright — it's still fully built (`bandwidth.rs`, reachable via
/// `--test-screen fp:bandwidth`), just unreachable from the live menu, since
/// it's admittedly-fabricated static content (no bulletin backend exists
/// anywhere in this app) that shouldn't sit next to real data-backed rows.
const MAIN_PLAY_INDEX: usize = 0;
const MAIN_ONLINE_INDEX: usize = 1;
const MAIN_RANKINGS_INDEX: usize = 2;
const MAIN_SETTINGS_INDEX: usize = 3;
const MAIN_ITEM_COUNT: usize = 4;
/// Sentinel `cursor` value (one past the last real row) meaning "the YOUR
/// STATS panel is focused" rather than any of the 5 menu rows — reached via
/// `FpNav::Right` from any row, `FpNav::Left` to return. Kept as a sentinel
/// on the existing `cursor: usize` rather than a new `FpScreen::Main` field
/// so every other `FpScreen::Main { cursor: N }` construction site (Quit's
/// restore, the various screens' `Back` targets) keeps working unchanged.
const MAIN_STATS_INDEX: usize = MAIN_ITEM_COUNT;
/// Sentinel `cursor` value meaning "the LAST MATCH card is focused" —
/// reached via `FpNav::Down` from the bottom row (SETTINGS), `Up` to
/// return. One past `MAIN_STATS_INDEX` so the two side-target sentinels
/// never collide.
const MAIN_LAST_MATCH_INDEX: usize = MAIN_ITEM_COUNT + 1;

/// Legacy `crate::menu::MAIN_ITEMS` indices this module delegates real
/// actions to via `FpResult::ActivateMainItem`. Named here (rather than
/// inlined as magic numbers) since fp_ui's own Main Menu no longer mirrors
/// legacy's ordering 1:1 the way it used to.
const LEGACY_ARCADE_INDEX: usize = 1;

/// fp_ui's own PlayMenu cursor space (Arcade/Lab/Replays/Drones).
const PLAY_ARCADE_INDEX: usize = 0;
const PLAY_LAB_INDEX: usize = 1;
const PLAY_REPLAYS_INDEX: usize = 2;
const PLAY_DRONES_INDEX: usize = 3;

/// All fp_ui screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FpScreen {
    Main { cursor: usize },
    /// Quit confirmation, rendered on top of the Main Menu rather than
    /// replacing it. `menu_cursor` is preserved so the dimmed menu behind
    /// the modal (and the screen underneath if Cancel is chosen) still
    /// shows the row the player quit from selected.
    Quit { choice: usize, menu_cursor: usize },
    /// Play's submenu: Arcade / Lab / Replays / Drones. Arcade boots the ROM
    /// directly (no character-select step — see `play_menu.rs`); Replays
    /// still delegates to its legacy screen (follow-up work). Lab and
    /// Drones now have native *chooser/list* screens (`LabMenu`/
    /// `GhostSelect` below) — the actual in-game Lab trainer overlay and
    /// drone gameplay stay legacy, per explicit user direction: only the
    /// menu system needed updating, not the in-game UI.
    PlayMenu { cursor: usize },
    /// Lab's own 2-item chooser (Start Lab / Load Drones) — native
    /// redesign of legacy's `MenuScreen::LabMenu`. See `lab_menu.rs`.
    LabMenu { cursor: usize },
    /// Load Drones — native redesign of legacy's `MenuScreen::GhostSelect`.
    /// `entries`/`status` are populated/drained by `main.rs` exactly like
    /// the legacy screen's own fields (`FpResult::OpenGhostSelect` kicks off
    /// the same local-scan + remote-fetch this screen just displays). See
    /// `ghost_select.rs`.
    GhostSelect {
        cursor: usize,
        entries: Vec<crate::menu::GhostEntry>,
        status: Option<String>,
    },
    /// Replays — native redesign of legacy's `MenuScreen::ReplaySelect`.
    /// Same populate/drain pattern as `GhostSelect` above (see
    /// `FpResult::OpenReplaySelect`/`LoadReplay`/`LoadRemoteReplay`); the
    /// replay *playback* viewer itself stays legacy, only this chooser
    /// list is native. See `replay_select.rs`.
    ReplaySelect {
        cursor: usize,
        entries: Vec<crate::menu::ReplayEntry>,
        status: Option<String>,
    },
    /// Static bulletin board ("the wire") — no backend exists for this
    /// content anywhere in the app; see `bandwidth.rs` module doc.
    Bandwidth,
    /// Community leaderboard. Reads real data the caller already fetches
    /// unconditionally at startup (`main_leaderboard` in `main.rs`) rather
    /// than opening a second fetch pipeline — see `rankings.rs`.
    Rankings,
    /// Native Profile screen (rating card, win/loss/streak stats, recent
    /// match history) — reached from the Main Menu's "YOUR STATS" panel.
    /// Reads the same `ProfileScreenState` the caller already fetches for
    /// that panel (`main.rs`'s `main_profile`), same reasoning as
    /// `Rankings`. Unlike Lab/Replays/Drones/Controls, this does *not*
    /// delegate to the legacy bitmap-font Profile screen — see
    /// `profile.rs`'s module doc for why a native one exists here now.
    Profile,
    /// Static build info + keybindings, reachable from any fp_ui screen via
    /// the Info gesture (Y / Triangle) as well as the Main Menu footer icon.
    About,
    /// `fields` mirrors the relevant `Config` fields directly; every
    /// adjustment writes straight into this copy, and `FpResult::SettingsChanged`
    /// tells the caller to sync it into the real `Config` and persist
    /// (`"changes saved automatically"`, per the mockup's footer).
    /// `sidebar_focus`: true while Up/Down drive the category sidebar
    /// (`cat`) instead of the row cursor (`row`) — L1/R1 (`PrevTab`/
    /// `NextTab`) can still switch categories either way, but without this
    /// there was no way to reach the sidebar with Up/Down at all, which
    /// read as "stuck" since both a category and a row are drawn as
    /// selected at once with nothing showing which one input applies to.
    /// `controls_player`: which player's bindings the Controls category
    /// (cat 0) is showing/rebinding — `FpNav::Left`/`Right` switch it while
    /// that category is active instead of adjusting a value (Controls rows
    /// have none to adjust; the mockup's own `rebindPlayerTabs` are a
    /// mouse-only affordance we don't have, so repurposing the otherwise-
    /// idle Left/Right for this category is the closest controller-native
    /// equivalent).
    Settings {
        cat: usize,
        row: usize,
        fields: SettingsFields,
        sidebar_focus: bool,
        controls_player: crate::input::Player,
    },
    /// `tab`: 0=Quick Match, 1=Host/Join, 2=Server Browser, 3=Chat, 4=Watch.
    /// `host_join_focus`: 0=Host column, 1=Join column (tab 1 only).
    /// `lobbies`/`cursor`/`status`: the real public-lobby list (tab 2),
    /// kept in sync by main.rs the same way it syncs
    /// `MenuScreen::OnlineHub`'s fields from `lobby_list_rx`.
    /// `chat`/`presence` (tab 3) and `live_matches` (tab 4) are kept in sync
    /// the same way — see `mod.rs`'s `FpResult::SendLobbyChat`/
    /// `WatchSession`/`OpenLegacyChat` doc comments. `cursor` doubles as the
    /// quick-phrase index on tab 3 and the live-match index on tab 4 (same
    /// per-tab reuse `Settings`' `row` already does across categories).
    Lobby {
        tab: usize,
        host_join_focus: usize,
        cursor: usize,
        lobbies: Vec<LobbyPreview>,
        status: String,
        chat: Vec<crate::matchmaking::LobbyChatMessage>,
        presence: Vec<crate::matchmaking::LobbyUser>,
        live_matches: Vec<crate::matchmaking::LiveMatch>,
    },
}

impl FpScreen {
    pub fn main() -> Self {
        FpScreen::Main { cursor: MAIN_PLAY_INDEX }
    }

    pub fn settings_from_cfg(cfg: &crate::config::Config) -> Self {
        FpScreen::Settings {
            cat: 0,
            row: 0,
            fields: SettingsFields::from_cfg(cfg),
            sidebar_focus: false,
            controls_player: crate::input::Player::P1,
        }
    }

    pub fn lobby() -> Self {
        FpScreen::Lobby {
            tab: 0,
            host_join_focus: 0,
            cursor: 0,
            lobbies: Vec::new(),
            status: String::new(),
            chat: Vec::new(),
            presence: Vec::new(),
            live_matches: Vec::new(),
        }
    }
}

/// What a nav event asks the caller (main.rs) to do, beyond mutating the
/// screen in place.
pub enum FpResult {
    Stay,
    /// Confirm on a Main Menu row (any but Online/Settings/Quit, which this
    /// module handles itself). `cursor` is the same index space as
    /// `menu::MAIN_ITEMS` — the caller sets
    /// `state = AppState::Menu(MenuScreen::Main { cursor })` and lets the
    /// existing legacy `nav_accept` dispatch take it from there (ROM-present
    /// checks, screen construction, session/profile/replay side effects),
    /// rather than reimplementing any of that here.
    ActivateMainItem(usize),
    /// Same idea as `ActivateMainItem`, but the caller sets
    /// `state = AppState::Menu(MenuScreen::LabMenu { cursor })` instead —
    /// `nav_accept`'s `LabMenu` arm handles both of its rows (Start Lab /
    /// Load Drones) the same way its `Main` arm handles the top-level menu,
    /// so PlayMenu's "Drones" row can jump straight to `GhostSelect` without
    /// fp_ui needing to reimplement Lab's own 2-item chooser.
    ActivateLabMenuItem(usize),
    /// EXIT GAME confirmed on the Quit overlay. The caller breaks the main
    /// loop exactly like the legacy `NavResult::Quit`.
    ExitGame,
    /// A Settings row changed. The caller reads `FpScreen::Settings`'s
    /// current `fields` out of `state`, copies them into `Config`, applies
    /// any that need a live side effect (fullscreen), and calls
    /// `config::save`.
    SettingsChanged,
    /// Confirm on the Quick Match tab. The caller runs the same
    /// username-confirmed check as legacy's `NavResult::OpenUsernameEntry`
    /// and, once queued, hands off to the legacy `MenuScreen::Matchmaking`
    /// screen — see `lobby.rs`'s module doc for why the search itself isn't
    /// re-implemented in the new visual language for this step.
    StartFindMatch,
    /// Confirm on Host/Join's Host column. Caller does what legacy's
    /// `NavResult::CreateLobby(format, true)` does (a real private lobby,
    /// default format), landing on the real `MenuScreen::Lobby`.
    CreatePrivateLobby,
    /// Confirm on Host/Join's Join column. Caller opens the same legacy
    /// join-code text-entry screen `NavResult::OpenJoinCode` does.
    OpenJoinCode,
    /// Confirm on a Server Browser row. Caller does what legacy's
    /// `NavResult::JoinLobby` does with this id.
    JoinLobby(String),
    /// Confirm while the "LAST MATCH" card is focused (`MAIN_LAST_MATCH_INDEX`).
    /// The caller looks up a local replay file matching the most recent
    /// `HistoryRow` (same opponent, score, and date) and, if one exists,
    /// starts reviewing it the same way picking it from the legacy
    /// `ReplaySelect` screen would; otherwise it's a no-op (no replay to
    /// show — not every server-recorded match has a local `.rep` file, e.g.
    /// if it predates this install or was played on another device).
    WatchLastMatchReplay,
    /// Confirm on a Controls-category row (Settings' new 1st category).
    /// The caller enters the exact same `AppState::Rebinding` capture the
    /// legacy Controls screen uses (`came_from` set to the current
    /// `AppState::FpUi(FpScreen::Settings{..})` so it returns here, not to
    /// legacy, once a button is pressed or the rebind is canceled).
    BeginRebind(crate::input::Action, crate::input::Player),
    /// Confirm on Controls' "CLEAR ALL" row. Caller does what legacy's
    /// `NavResult::ClearAllBindings` does: `Bindings::get_mut(player).clear_all()`
    /// + save.
    ClearAllBindings(crate::input::Player),
    /// Confirm on the Account category's Username or Stats Email row.
    /// Caller opens the same legacy `MenuScreen::TextEdit` capture
    /// `NavResult::EditText` does, same known rough edge as
    /// `OpenJoinCode` — lands back on legacy Main (not fp_ui Settings) once
    /// submitted/cancelled, rather than threading a `came_from` back to
    /// fp_ui for what's a one-off action.
    BeginAccountEdit(crate::menu::EditField),
    /// Confirm on the Account category's Discord row. Caller does exactly
    /// what legacy's `NavResult::ConnectDiscord` does (same rough edge as
    /// above: lands on legacy Settings/Matchmaking, not fp_ui).
    ToggleDiscordConnect,
    /// Entered the (native) Load Drones screen. Caller does exactly what
    /// legacy's `NavResult::OpenGhostSelect` does — scans `ghosts/` for
    /// local `.ncgh` files and kicks off a `fetch_ghost_list` for community
    /// recordings — targeting `FpScreen::GhostSelect`'s fields instead of
    /// the legacy screen's.
    OpenGhostSelect,
    /// Confirm on a local drone entry. Caller does exactly what legacy's
    /// `NavResult::LoadGhost(path)` does.
    LoadGhost(String),
    /// Confirm on a community drone entry. Caller does exactly what
    /// legacy's `NavResult::DownloadGhost(ghost_id)` does.
    DownloadGhost(String),
    /// Entered the (native) Replays screen. Caller does exactly what
    /// legacy's `NavResult::OpenReplaySelect` does — scans local `.ncrp`
    /// files and kicks off a `fetch_public_replays` for community replays —
    /// targeting `FpScreen::ReplaySelect`'s fields instead of the legacy
    /// screen's.
    OpenReplaySelect,
    /// Confirm on a local replay entry. Caller does exactly what legacy's
    /// `NavResult::LoadReplay(path)` does.
    LoadReplay(String),
    /// Confirm on a community replay entry. Caller does exactly what
    /// legacy's `NavResult::LoadRemoteReplay(url)` does.
    LoadRemoteReplay(String),
    /// Confirm on a Chat quick-phrase chip. Caller does exactly what
    /// legacy's `NavResult::SendLobbyChat(message)` does.
    SendLobbyChat(String),
    /// Confirm on the Chat tab's "compose a message" slot (one past the
    /// last quick phrase). fp_ui has no on-screen keyboard of its own —
    /// same as the mockup itself, whose Chat tab shows a "△ TO OPEN
    /// KEYBOARD" hint rather than an inline keyboard — so this hands off
    /// to the real legacy `MenuScreen::OnlineHub` (tab Chat, focus
    /// Content), which has the actual OSK, seeded with the chat/presence
    /// already fetched here rather than re-fetching from empty. Known
    /// rough edge, same shape as `BeginAccountEdit`/`OpenJoinCode`: lands
    /// back on legacy once the player backs out, not on this screen.
    OpenLegacyChat {
        chat: Vec<crate::matchmaking::LobbyChatMessage>,
        presence: Vec<crate::matchmaking::LobbyUser>,
    },
    /// Confirm on a Watch tab live-match row. Caller does exactly what
    /// legacy's `NavResult::WatchSession(session_id)` does — the spectator
    /// *view* itself stays legacy (`MenuScreen::Spectate`), same as replay/
    /// ghost playback; only this list is native.
    WatchSession(String),
}

pub fn nav(screen: &mut FpScreen, input: FpNav, rom_present: bool) -> FpResult {
    match screen {
        FpScreen::Main { cursor } => match input {
            FpNav::Up => {
                if *cursor == MAIN_LAST_MATCH_INDEX {
                    *cursor = MAIN_SETTINGS_INDEX;
                } else if *cursor < MAIN_ITEM_COUNT {
                    *cursor = cursor.saturating_sub(1);
                }
                FpResult::Stay
            }
            FpNav::Down => {
                if *cursor < MAIN_ITEM_COUNT {
                    *cursor = if *cursor == MAIN_ITEM_COUNT - 1 {
                        MAIN_LAST_MATCH_INDEX
                    } else {
                        *cursor + 1
                    };
                }
                FpResult::Stay
            }
            // The "YOUR STATS" panel sits to the right of the row list —
            // Right focuses it (Confirm there opens the real Profile
            // screen), Left returns focus to the rows.
            FpNav::Right => {
                *cursor = MAIN_STATS_INDEX;
                FpResult::Stay
            }
            FpNav::Left => {
                if *cursor == MAIN_STATS_INDEX {
                    *cursor = MAIN_PLAY_INDEX;
                }
                FpResult::Stay
            }
            FpNav::Confirm if *cursor == MAIN_STATS_INDEX => {
                *screen = FpScreen::Profile;
                FpResult::Stay
            }
            FpNav::Confirm if *cursor == MAIN_LAST_MATCH_INDEX => FpResult::WatchLastMatchReplay,
            FpNav::Confirm => match *cursor {
                c if c == MAIN_PLAY_INDEX => {
                    // Nothing under Play works without the ROM (Arcade/Lab/
                    // Replays/Drones all end up calling ensure_core_loaded,
                    // which hard-fails without it) — block entry here
                    // rather than letting the player pick a row that can
                    // only silently fail deeper in, matching the dimmed
                    // row `main_menu::draw_row` shows for the same reason.
                    if rom_present {
                        *screen = FpScreen::PlayMenu { cursor: 0 };
                    }
                    FpResult::Stay
                }
                c if c == MAIN_ONLINE_INDEX => {
                    *screen = FpScreen::lobby();
                    FpResult::Stay
                }
                c if c == MAIN_RANKINGS_INDEX => {
                    *screen = FpScreen::Rankings;
                    FpResult::Stay
                }
                _ => FpResult::ActivateMainItem(LEGACY_SETTINGS_INDEX),
            },
            // Not a menu row in the new mockup (no Quit entry in `menuDefs`
            // — its own keybindings table documents "Quit to desktop: HOLD
            // START" instead, a gesture this app has no hold-duration
            // tracking for yet). Back-from-root is the next best fit: the
            // conventional "press Back at the top level to exit" pattern,
            // and it costs no new input plumbing.
            FpNav::Back => {
                *screen = FpScreen::Quit { choice: 0, menu_cursor: *cursor };
                FpResult::Stay
            }
            FpNav::Info => {
                *screen = FpScreen::About;
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
        FpScreen::PlayMenu { cursor } => match input {
            FpNav::Up => {
                *cursor = cursor.saturating_sub(1);
                FpResult::Stay
            }
            FpNav::Down => {
                *cursor = (*cursor + 1).min(PLAY_DRONES_INDEX);
                FpResult::Stay
            }
            FpNav::Confirm => match *cursor {
                c if c == PLAY_ARCADE_INDEX => FpResult::ActivateMainItem(LEGACY_ARCADE_INDEX),
                c if c == PLAY_LAB_INDEX => {
                    *screen = FpScreen::LabMenu { cursor: 0 };
                    FpResult::Stay
                }
                c if c == PLAY_REPLAYS_INDEX => {
                    *screen = FpScreen::ReplaySelect { cursor: 0, entries: Vec::new(), status: None };
                    FpResult::OpenReplaySelect
                }
                _ => {
                    // Drones — jumps straight to the drone list, same shortcut
                    // legacy's own Play submenu gives this row (skipping the
                    // Lab chooser, since there's nothing to choose here).
                    *screen = FpScreen::GhostSelect { cursor: 0, entries: Vec::new(), status: None };
                    FpResult::OpenGhostSelect
                }
            },
            FpNav::Back => {
                *screen = FpScreen::Main { cursor: MAIN_PLAY_INDEX };
                FpResult::Stay
            }
            FpNav::Info => {
                *screen = FpScreen::About;
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
        FpScreen::LabMenu { cursor } => match input {
            FpNav::Up => {
                *cursor = cursor.saturating_sub(1);
                FpResult::Stay
            }
            FpNav::Down => {
                *cursor = (*cursor + 1).min(lab_menu::ITEMS.len() - 1);
                FpResult::Stay
            }
            FpNav::Confirm if *cursor == 0 => FpResult::ActivateLabMenuItem(0), // Start Lab
            FpNav::Confirm => {
                // Load Drones — same OpenGhostSelect side effect the Play
                // submenu's Drones row triggers directly.
                *screen = FpScreen::GhostSelect { cursor: 0, entries: Vec::new(), status: None };
                FpResult::OpenGhostSelect
            }
            FpNav::Back => {
                *screen = FpScreen::PlayMenu { cursor: PLAY_LAB_INDEX };
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
        FpScreen::GhostSelect { cursor, entries, .. } => match input {
            FpNav::Up => {
                *cursor = cursor.saturating_sub(1);
                FpResult::Stay
            }
            FpNav::Down => {
                *cursor = (*cursor + 1).min(entries.len().saturating_sub(1));
                FpResult::Stay
            }
            FpNav::Confirm => match entries.get(*cursor) {
                Some(crate::menu::GhostEntry::Local { path, .. }) => FpResult::LoadGhost(path.clone()),
                Some(crate::menu::GhostEntry::Remote(meta)) => FpResult::DownloadGhost(meta.ghost_id.clone()),
                None => FpResult::Stay,
            },
            FpNav::Back => {
                *screen = FpScreen::LabMenu { cursor: 1 };
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
        FpScreen::ReplaySelect { cursor, entries, .. } => match input {
            FpNav::Up => {
                *cursor = cursor.saturating_sub(1);
                FpResult::Stay
            }
            FpNav::Down => {
                *cursor = (*cursor + 1).min(entries.len().saturating_sub(1));
                FpResult::Stay
            }
            FpNav::Confirm => match entries.get(*cursor) {
                Some(entry) => match &entry.remote_url {
                    Some(url) => FpResult::LoadRemoteReplay(url.clone()),
                    None => FpResult::LoadReplay(entry.path.clone()),
                },
                None => FpResult::Stay,
            },
            FpNav::Back => {
                *screen = FpScreen::PlayMenu { cursor: PLAY_REPLAYS_INDEX };
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
        FpScreen::Bandwidth => match input {
            // Not reachable from the live Main Menu right now (see
            // `MAIN_ITEM_COUNT`'s doc comment) — only via `--test-screen
            // fp:bandwidth` — so there's no "row it came from" to return
            // to; Back just goes to the top of the menu.
            FpNav::Back => {
                *screen = FpScreen::main();
                FpResult::Stay
            }
            FpNav::Info => {
                *screen = FpScreen::About;
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
        FpScreen::Rankings => match input {
            FpNav::Back => {
                *screen = FpScreen::Main { cursor: MAIN_RANKINGS_INDEX };
                FpResult::Stay
            }
            FpNav::Info => {
                *screen = FpScreen::About;
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
        FpScreen::About => match input {
            FpNav::Back => {
                *screen = FpScreen::main();
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
        FpScreen::Profile => match input {
            FpNav::Back => {
                *screen = FpScreen::Main { cursor: MAIN_STATS_INDEX };
                FpResult::Stay
            }
            FpNav::Info => {
                *screen = FpScreen::About;
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
        FpScreen::Quit { choice, menu_cursor } => match input {
            FpNav::Left | FpNav::Right => {
                *choice = 1 - *choice;
                FpResult::Stay
            }
            FpNav::Confirm => {
                if *choice == 1 {
                    FpResult::ExitGame
                } else {
                    *screen = FpScreen::Main { cursor: *menu_cursor };
                    FpResult::Stay
                }
            }
            FpNav::Back => {
                *screen = FpScreen::Main { cursor: *menu_cursor };
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
        FpScreen::Settings { cat, row, fields, sidebar_focus, controls_player } => match input {
            FpNav::Up => {
                if *sidebar_focus {
                    *cat = cat.saturating_sub(1);
                } else if *row == 0 {
                    // Nothing above the top row — hand Up/Down over to the
                    // sidebar rather than doing nothing, so a controller
                    // without working shoulder buttons can still reach the
                    // other categories.
                    *sidebar_focus = true;
                } else {
                    *row -= 1;
                }
                FpResult::Stay
            }
            FpNav::Down => {
                if *sidebar_focus {
                    *cat = (*cat + 1).min(settings::CATS.len() - 1);
                } else {
                    *row = (*row + 1).min(settings::rows_in_cat(*cat) - 1);
                }
                FpResult::Stay
            }
            FpNav::Confirm if *sidebar_focus => {
                *sidebar_focus = false;
                *row = 0;
                FpResult::Stay
            }
            FpNav::Confirm if *cat == settings::CONTROLS_CAT_INDEX => {
                if *row < crate::input::Action::ALL.len() {
                    FpResult::BeginRebind(crate::input::Action::ALL[*row], *controls_player)
                } else {
                    FpResult::ClearAllBindings(*controls_player)
                }
            }
            FpNav::Confirm if *cat == settings::ACCOUNT_CAT_INDEX => match *row {
                0 => FpResult::BeginAccountEdit(crate::menu::EditField::Username),
                1 => FpResult::BeginAccountEdit(crate::menu::EditField::StatsEmail),
                _ => FpResult::ToggleDiscordConnect,
            },
            FpNav::PrevTab => {
                *cat = (*cat + settings::CATS.len() - 1) % settings::CATS.len();
                *row = 0;
                FpResult::Stay
            }
            FpNav::NextTab => {
                *cat = (*cat + 1) % settings::CATS.len();
                *row = 0;
                FpResult::Stay
            }
            // Controls rows have nothing to adjust with Left/Right (no
            // toggle/cycle/slider value) — repurposed to switch which
            // player's bindings are shown/rebound instead (see
            // `FpScreen::Settings::controls_player`'s doc comment).
            FpNav::Left | FpNav::Right if !*sidebar_focus && *cat == settings::CONTROLS_CAT_INDEX => {
                *controls_player = controls_player.other();
                FpResult::Stay
            }
            FpNav::Left if !*sidebar_focus => {
                fields.adjust(*cat, *row, -1);
                FpResult::SettingsChanged
            }
            FpNav::Right if !*sidebar_focus => {
                fields.adjust(*cat, *row, 1);
                FpResult::SettingsChanged
            }
            // Back drills up one level at a time: from a category's content,
            // it hands focus to the sidebar (same place Up-at-top-row and
            // L1/R1 already land you) rather than leaving Settings outright;
            // only a second Back — now with the sidebar already focused —
            // exits to the Main Menu.
            FpNav::Back if !*sidebar_focus => {
                *sidebar_focus = true;
                FpResult::Stay
            }
            FpNav::Back => {
                *screen = FpScreen::Main { cursor: MAIN_SETTINGS_INDEX };
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
        FpScreen::Lobby { tab, host_join_focus, cursor, lobbies, chat, presence, live_matches, .. } => match input {
            FpNav::PrevTab => {
                *tab = (*tab + lobby::TABS.len() - 1) % lobby::TABS.len();
                *cursor = 0;
                FpResult::Stay
            }
            FpNav::NextTab => {
                *tab = (*tab + 1) % lobby::TABS.len();
                *cursor = 0;
                FpResult::Stay
            }
            FpNav::Up if *tab == 2 => {
                *cursor = cursor.saturating_sub(1);
                FpResult::Stay
            }
            FpNav::Down if *tab == 2 => {
                *cursor = (*cursor + 1).min(lobbies.len().saturating_sub(1));
                FpResult::Stay
            }
            // Chat: cursor walks the quick-phrase row, plus one sentinel
            // slot past the last phrase for "compose a message" (see
            // `FpResult::OpenLegacyChat`'s doc comment for why that's a
            // screen swap rather than native text entry).
            FpNav::Up if *tab == 3 => {
                *cursor = cursor.saturating_sub(1);
                FpResult::Stay
            }
            FpNav::Down if *tab == 3 => {
                *cursor = (*cursor + 1).min(crate::menu::QUICK_PHRASES.len());
                FpResult::Stay
            }
            FpNav::Up if *tab == 4 => {
                *cursor = cursor.saturating_sub(1);
                FpResult::Stay
            }
            FpNav::Down if *tab == 4 => {
                *cursor = (*cursor + 1).min(live_matches.len().saturating_sub(1));
                FpResult::Stay
            }
            FpNav::Left | FpNav::Right if *tab == 1 => {
                *host_join_focus = 1 - *host_join_focus;
                FpResult::Stay
            }
            FpNav::Confirm => match *tab {
                0 => FpResult::StartFindMatch,
                1 if *host_join_focus == 0 => FpResult::CreatePrivateLobby,
                1 => FpResult::OpenJoinCode,
                2 => match lobbies.get(*cursor) {
                    Some(l) => FpResult::JoinLobby(l.id.clone()),
                    None => FpResult::Stay,
                },
                3 if *cursor < crate::menu::QUICK_PHRASES.len() => {
                    FpResult::SendLobbyChat(crate::menu::quick_phrase(*cursor).to_string())
                }
                3 => FpResult::OpenLegacyChat { chat: chat.clone(), presence: presence.clone() },
                _ => match live_matches.get(*cursor) {
                    Some(m) => FpResult::WatchSession(m.session_id.clone()),
                    None => FpResult::Stay,
                },
            },
            FpNav::Back => {
                *screen = FpScreen::Main { cursor: MAIN_ONLINE_INDEX };
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
    }
}

/// Draw the current fp_ui screen. Caller has already set
/// `canvas.set_logical_size(0, 0)` (raw window pixels) — fp_ui owns all its
/// own logical->window scaling via `Scale`, rather than relying on SDL's
/// logical-size stretch (which would blur re-rasterized text).
#[allow(clippy::too_many_arguments)]
pub fn draw(
    screen: &FpScreen,
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    win_w: i32,
    win_h: i32,
    username: &str,
    leaderboard: &crate::menu::LeaderboardState,
    profile: &crate::menu::ProfileScreenState,
    bindings: &crate::input::Bindings,
    stats_email: &str,
    discord_connected: bool,
    rom_present: bool,
) -> Result<(), String> {
    let scale = Scale::compute(win_w, win_h);
    fonts.begin_frame(scale.s);
    canvas.set_draw_color(theme::BG);
    canvas.clear();

    match screen {
        FpScreen::Main { cursor } => main_menu::draw(canvas, fonts, &scale, *cursor, username, profile, rom_present)?,
        FpScreen::Quit { choice, menu_cursor } => {
            main_menu::draw(canvas, fonts, &scale, *menu_cursor, username, profile, rom_present)?;
            quit::draw(canvas, fonts, &scale, *choice)?;
        }
        FpScreen::PlayMenu { cursor } => play_menu::draw(canvas, fonts, &scale, *cursor, username)?,
        FpScreen::LabMenu { cursor } => lab_menu::draw(canvas, fonts, &scale, *cursor, username)?,
        FpScreen::GhostSelect { cursor, entries, status } => {
            ghost_select::draw(canvas, fonts, &scale, *cursor, entries, status.as_deref(), username)?
        }
        FpScreen::ReplaySelect { cursor, entries, status } => {
            replay_select::draw(canvas, fonts, &scale, *cursor, entries, status.as_deref(), username)?
        }
        FpScreen::Bandwidth => bandwidth::draw(canvas, fonts, &scale, username)?,
        FpScreen::Rankings => rankings::draw(canvas, fonts, &scale, username, leaderboard)?,
        FpScreen::Profile => profile::draw(canvas, fonts, &scale, username, profile)?,
        FpScreen::About => about::draw(canvas, fonts, &scale, username)?,
        FpScreen::Settings { cat, row, fields, sidebar_focus, controls_player } => settings::draw(
            canvas,
            fonts,
            &scale,
            fields,
            *cat,
            *row,
            *sidebar_focus,
            *controls_player,
            bindings,
            username,
            stats_email,
            discord_connected,
        )?,
        FpScreen::Lobby { tab, host_join_focus, cursor, lobbies, status, chat, presence, live_matches } => lobby::draw(
            canvas,
            fonts,
            &scale,
            *tab,
            *host_join_focus,
            *cursor,
            lobbies,
            status,
            chat,
            presence,
            live_matches,
            username,
        )?,
    }

    Ok(())
}
