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

pub mod chrome;
pub mod geometry;
pub mod input;
pub mod layout;
mod lobby;
mod main_menu;
mod quit;
pub mod settings;
pub mod theme;

pub use input::{event_to_fp_nav, FpNav};
pub use layout::Scale;
pub use settings::SettingsFields;

use crate::font::FpFontCache;
use crate::menu::{LobbyPreview, MAIN_ITEMS, MAIN_SETTINGS_INDEX};
use sdl2::render::Canvas;
use sdl2::video::Window;

/// Main Menu index for "Online" — `crate::fp_ui` special-cases it (opens the
/// new Lobby screen rather than delegating to legacy's OnlineHub) the same
/// way it special-cases Settings and Quit.
const MAIN_ONLINE_INDEX: usize = 0;

/// All fp_ui screens. Play lands in step 6.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FpScreen {
    Main { cursor: usize },
    /// Quit confirmation, rendered on top of the Main Menu rather than
    /// replacing it. `menu_cursor` is preserved so the dimmed menu behind
    /// the modal (and the screen underneath if Cancel is chosen) still
    /// shows the row the player quit from selected.
    Quit { choice: usize, menu_cursor: usize },
    /// `fields` mirrors the relevant `Config` fields directly; every
    /// adjustment writes straight into this copy, and `FpResult::SettingsChanged`
    /// tells the caller to sync it into the real `Config` and persist
    /// (`"changes saved automatically"`, per the mockup's footer).
    Settings { cat: usize, row: usize, fields: SettingsFields },
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
        FpScreen::Main { cursor: 0 }
    }

    pub fn settings_from_cfg(cfg: &crate::config::Config) -> Self {
        FpScreen::Settings {
            cat: 0,
            row: 0,
            fields: SettingsFields::from_cfg(cfg),
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
}

pub fn nav(screen: &mut FpScreen, input: FpNav) -> FpResult {
    match screen {
        FpScreen::Main { cursor } => match input {
            FpNav::Up => {
                *cursor = cursor.saturating_sub(1);
                FpResult::Stay
            }
            FpNav::Down => {
                *cursor = (*cursor + 1).min(MAIN_ITEMS.len() - 1);
                FpResult::Stay
            }
            FpNav::Confirm => {
                if *cursor == MAIN_ITEMS.len() - 1 {
                    // Quit is always the last item — open the overlay
                    // instead of delegating to legacy's instant-exit.
                    *screen = FpScreen::Quit { choice: 0, menu_cursor: *cursor };
                    FpResult::Stay
                } else if *cursor == MAIN_ONLINE_INDEX {
                    *screen = FpScreen::lobby();
                    FpResult::Stay
                } else {
                    FpResult::ActivateMainItem(*cursor)
                }
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
        FpScreen::Settings { cat, row, fields } => match input {
            FpNav::Up => {
                *row = row.saturating_sub(1);
                FpResult::Stay
            }
            FpNav::Down => {
                *row = (*row + 1).min(settings::rows_in_cat(*cat) - 1);
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
            FpNav::Left => {
                fields.adjust(*cat, *row, -1);
                FpResult::SettingsChanged
            }
            FpNav::Right => {
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
) -> Result<(), String> {
    let scale = Scale::compute(win_w, win_h);
    fonts.begin_frame(scale.s);
    canvas.set_draw_color(theme::BG);
    canvas.clear();

    match screen {
        FpScreen::Main { cursor } => main_menu::draw(canvas, fonts, &scale, *cursor, username)?,
        FpScreen::Quit { choice, menu_cursor } => {
            main_menu::draw(canvas, fonts, &scale, *menu_cursor, username)?;
            quit::draw(canvas, fonts, &scale, *choice)?;
        }
        FpScreen::Settings { cat, row, fields } => {
            settings::draw(canvas, fonts, &scale, fields, *cat, *row, username)?
        }
        FpScreen::Lobby { tab, host_join_focus, cursor, lobbies, status } => {
            lobby::draw(canvas, fonts, &scale, *tab, *host_join_focus, *cursor, lobbies, status, username)?
        }
    }

    Ok(())
}
