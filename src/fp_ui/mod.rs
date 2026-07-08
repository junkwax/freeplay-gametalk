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
pub mod geometry;
pub mod input;
pub mod layout;
mod lobby;
mod main_menu;
mod play_menu;
mod profile;
mod quit;
pub mod rankings;
pub mod settings;
pub mod theme;

pub use input::{event_to_fp_nav, FpNav};
pub use layout::Scale;
pub use settings::SettingsFields;

use crate::menu::{LobbyPreview, MAIN_SETTINGS_INDEX as LEGACY_SETTINGS_INDEX};
use crate::font::FpFontCache;
use sdl2::render::Canvas;
use sdl2::video::Window;

/// fp_ui's own Main Menu cursor space — 5 rows matching the mockup's
/// `menuDefs` (Play/Online/Network News/Rankings/Settings), decoupled from
/// `crate::menu::MAIN_ITEMS`'s 9-item legacy ordering. Only used for
/// `FpScreen::Main { cursor }` and "return to Main at row X" transitions;
/// never sent to `main.rs` as an `ActivateMainItem` payload (those still
/// carry *legacy* indices — see `LEGACY_*_INDEX` below).
const MAIN_PLAY_INDEX: usize = 0;
const MAIN_ONLINE_INDEX: usize = 1;
const MAIN_NETWORK_NEWS_INDEX: usize = 2;
const MAIN_RANKINGS_INDEX: usize = 3;
const MAIN_SETTINGS_INDEX: usize = 4;
const MAIN_ITEM_COUNT: usize = 5;
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
const LEGACY_LAB_INDEX: usize = 2;
const LEGACY_REPLAYS_INDEX: usize = 3;

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
    /// directly (no character-select step — see `play_menu.rs`); Lab,
    /// Replays and Drones delegate to their legacy screens for now (own
    /// fp_ui visual language is follow-up work).
    PlayMenu { cursor: usize },
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
    Settings { cat: usize, row: usize, fields: SettingsFields, sidebar_focus: bool },
    /// `tab`: 0=Quick Match, 1=Host/Join, 2=Server Browser.
    /// `host_join_focus`: 0=Host column, 1=Join column (tab 1 only).
    /// `lobbies`/`cursor`/`status`: the real public-lobby list (tab 2),
    /// kept in sync by main.rs the same way it syncs
    /// `MenuScreen::OnlineHub`'s fields from `lobby_list_rx`.
    Lobby {
        tab: usize,
        host_join_focus: usize,
        cursor: usize,
        lobbies: Vec<LobbyPreview>,
        status: String,
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
        }
    }

    pub fn lobby() -> Self {
        FpScreen::Lobby {
            tab: 0,
            host_join_focus: 0,
            cursor: 0,
            lobbies: Vec::new(),
            status: String::new(),
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
}

pub fn nav(screen: &mut FpScreen, input: FpNav) -> FpResult {
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
                    *screen = FpScreen::PlayMenu { cursor: 0 };
                    FpResult::Stay
                }
                c if c == MAIN_ONLINE_INDEX => {
                    *screen = FpScreen::lobby();
                    FpResult::Stay
                }
                c if c == MAIN_NETWORK_NEWS_INDEX => {
                    *screen = FpScreen::Bandwidth;
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
                c if c == PLAY_LAB_INDEX => FpResult::ActivateMainItem(LEGACY_LAB_INDEX),
                c if c == PLAY_REPLAYS_INDEX => FpResult::ActivateMainItem(LEGACY_REPLAYS_INDEX),
                _ => FpResult::ActivateLabMenuItem(1), // Drones = LabMenu row 1 ("Load Drones")
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
        FpScreen::Bandwidth => match input {
            FpNav::Back => {
                *screen = FpScreen::Main { cursor: MAIN_NETWORK_NEWS_INDEX };
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
        FpScreen::Settings { cat, row, fields, sidebar_focus } => match input {
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
            FpNav::Left if !*sidebar_focus => {
                fields.adjust(*cat, *row, -1);
                FpResult::SettingsChanged
            }
            FpNav::Right if !*sidebar_focus => {
                fields.adjust(*cat, *row, 1);
                FpResult::SettingsChanged
            }
            FpNav::Back => {
                *screen = FpScreen::Main { cursor: MAIN_SETTINGS_INDEX };
                FpResult::Stay
            }
            _ => FpResult::Stay,
        },
        FpScreen::Lobby { tab, host_join_focus, cursor, lobbies, .. } => match input {
            FpNav::PrevTab => {
                *tab = (*tab + 2) % 3;
                FpResult::Stay
            }
            FpNav::NextTab => {
                *tab = (*tab + 1) % 3;
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
            FpNav::Left | FpNav::Right if *tab == 1 => {
                *host_join_focus = 1 - *host_join_focus;
                FpResult::Stay
            }
            FpNav::Confirm => match *tab {
                0 => FpResult::StartFindMatch,
                1 if *host_join_focus == 0 => FpResult::CreatePrivateLobby,
                1 => FpResult::OpenJoinCode,
                _ => match lobbies.get(*cursor) {
                    Some(l) => FpResult::JoinLobby(l.id.clone()),
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
) -> Result<(), String> {
    let scale = Scale::compute(win_w, win_h);
    fonts.begin_frame(scale.s);
    canvas.set_draw_color(theme::BG);
    canvas.clear();

    match screen {
        FpScreen::Main { cursor } => main_menu::draw(canvas, fonts, &scale, *cursor, username, profile)?,
        FpScreen::Quit { choice, menu_cursor } => {
            main_menu::draw(canvas, fonts, &scale, *menu_cursor, username, profile)?;
            quit::draw(canvas, fonts, &scale, *choice)?;
        }
        FpScreen::PlayMenu { cursor } => play_menu::draw(canvas, fonts, &scale, *cursor, username)?,
        FpScreen::Bandwidth => bandwidth::draw(canvas, fonts, &scale, username)?,
        FpScreen::Rankings => rankings::draw(canvas, fonts, &scale, username, leaderboard)?,
        FpScreen::Profile => profile::draw(canvas, fonts, &scale, username, profile)?,
        FpScreen::About => about::draw(canvas, fonts, &scale, username)?,
        FpScreen::Settings { cat, row, fields, sidebar_focus } => {
            settings::draw(canvas, fonts, &scale, fields, *cat, *row, *sidebar_focus, username)?
        }
        FpScreen::Lobby { tab, host_join_focus, cursor, lobbies, status } => {
            lobby::draw(canvas, fonts, &scale, *tab, *host_join_focus, *cursor, lobbies, status, username)?
        }
    }

    Ok(())
}
