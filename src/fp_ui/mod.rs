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
mod main_menu;
mod quit;
pub mod theme;

pub use input::{event_to_fp_nav, FpNav};
pub use layout::Scale;

use crate::font::FpFontCache;
use sdl2::render::Canvas;
use sdl2::video::Window;

/// All fp_ui screens. Play, Settings, and Lobby land in steps 4-6.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FpScreen {
    Main { cursor: usize },
    /// Quit confirmation, rendered on top of the Main Menu rather than
    /// replacing it. `menu_cursor` is preserved so the dimmed menu behind
    /// the modal (and the screen underneath if Cancel is chosen) still
    /// shows the row the player quit from selected.
    Quit { choice: usize, menu_cursor: usize },
}

impl FpScreen {
    pub fn main() -> Self {
        FpScreen::Main { cursor: 0 }
    }
}

/// What a nav event asks the caller (main.rs) to do, beyond mutating the
/// screen in place.
pub enum FpResult {
    Stay,
    /// Confirm on a Main Menu row (any but Quit). `cursor` is the same
    /// index space as `menu::MAIN_ITEMS` — the caller sets
    /// `state = AppState::Menu(MenuScreen::Main { cursor })` and lets the
    /// existing legacy `nav_accept` dispatch take it from there (ROM-present
    /// checks, screen construction, session/profile/replay side effects),
    /// rather than reimplementing any of that here.
    ActivateMainItem(usize),
    /// EXIT GAME confirmed on the Quit overlay. The caller breaks the main
    /// loop exactly like the legacy `NavResult::Quit`.
    ExitGame,
}

pub fn nav(screen: &mut FpScreen, input: FpNav) -> FpResult {
    match screen {
        FpScreen::Main { cursor } => match input {
            FpNav::Up => {
                *cursor = cursor.saturating_sub(1);
                FpResult::Stay
            }
            FpNav::Down => {
                *cursor = (*cursor + 1).min(crate::menu::MAIN_ITEMS.len() - 1);
                FpResult::Stay
            }
            FpNav::Confirm => {
                if *cursor == crate::menu::MAIN_ITEMS.len() - 1 {
                    // Quit is always the last item — open the overlay
                    // instead of delegating to legacy's instant-exit.
                    *screen = FpScreen::Quit { choice: 0, menu_cursor: *cursor };
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
    }
}

/// Draw the current fp_ui screen. Caller has already set
/// `canvas.set_logical_size(0, 0)` (raw window pixels) — fp_ui owns all its
/// own logical->window scaling via `Scale`, rather than relying on SDL's
/// logical-size stretch (which would blur re-rasterized text).
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
    }

    Ok(())
}
