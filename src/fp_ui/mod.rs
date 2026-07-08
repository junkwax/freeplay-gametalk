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
//! `menu::main_menu_state`'s round trip, not reimplemented here.

pub mod input;
pub mod layout;
pub mod theme;

pub use input::{event_to_fp_nav, FpNav};
pub use layout::Scale;

use crate::font::FpFontCache;
use sdl2::render::Canvas;
use sdl2::video::Window;

/// All fp_ui screens. `Main` is the only one drawn for real in the skeleton
/// step — a placeholder proving the scaling/font-cache/legacy-routing
/// pipeline end to end. Steps 2-6 replace the placeholder body per screen
/// without changing this shape much.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FpScreen {
    Main { cursor: usize },
}

impl FpScreen {
    pub fn main() -> Self {
        FpScreen::Main { cursor: 0 }
    }
}

/// What a nav event asks the caller (main.rs) to do, beyond mutating the
/// screen in place. Mirrors `menu::NavResult`'s shape at a much smaller
/// scale — the skeleton only needs "stay" and "drop to a legacy screen".
pub enum FpResult {
    Stay,
    /// Enter a legacy screen. The skeleton's single placeholder item routes
    /// to `MenuScreen::About` as the round-trip proof; later steps route
    /// each of the 7 real menu items appropriately.
    EnterLegacyAbout,
}

/// Placeholder main-menu items for the skeleton step. Step 2 replaces this
/// with the real 7-item set mapped per the legacy-screens doc.
const PLACEHOLDER_ITEMS: usize = 1;

pub fn nav(screen: &mut FpScreen, input: FpNav) -> FpResult {
    match screen {
        FpScreen::Main { cursor } => match input {
            FpNav::Up => {
                *cursor = cursor.saturating_sub(1);
                FpResult::Stay
            }
            FpNav::Down => {
                *cursor = (*cursor + 1).min(PLACEHOLDER_ITEMS - 1);
                FpResult::Stay
            }
            FpNav::Confirm => FpResult::EnterLegacyAbout,
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
) -> Result<(), String> {
    let scale = Scale::compute(win_w, win_h);
    fonts.begin_frame(scale.s);
    canvas.set_draw_color(theme::BG);
    canvas.clear();

    match screen {
        FpScreen::Main { cursor } => draw_main_placeholder(canvas, fonts, &scale, *cursor)?,
    }

    Ok(())
}

fn draw_main_placeholder(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    cursor: usize,
) -> Result<(), String> {
    use crate::font::FpFont;

    let (x, y) = scale.point(56.0, 54.0);
    fonts.draw(
        canvas,
        FpFont::SairaCondensedExtraBold,
        scale.font_px(56.0),
        "FREEPLAY",
        x,
        y,
        theme::TEXT,
    )?;

    let (x, y) = scale.point(56.0, 148.0);
    let label = if cursor == 0 {
        "> ENTER LEGACY (ABOUT) — CROSS TO CONFIRM"
    } else {
        "ENTER LEGACY (ABOUT)"
    };
    fonts.draw(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(18.0),
        label,
        x,
        y,
        if cursor == 0 { theme::ACCENT } else { theme::DIM },
    )?;

    Ok(())
}
