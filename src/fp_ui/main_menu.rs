//! Main Menu — matches the mockup's `menuDefs`/`isMenu` branch exactly: 5
//! rows (Play, Online, Network News, Rankings, Settings), not the real
//! app's full 9-item `crate::menu::MAIN_ITEMS`. The other 4 legacy items
//! (Arcade/Lab/Replays/Drones) live one level down in `play_menu.rs`'s
//! submenu, matching the mockup's own `playMenuDefs`; Controls folds into
//! Settings' categories (a follow-up item — see `settings.rs`'s module
//! doc); Profile and Quit aren't reachable from here at all in the mockup
//! (Profile is a mouse-only header chip with no documented gamepad
//! binding; Quit is invoked by a hold-Start gesture this app has no
//! hold-duration tracking for yet — see `super::nav`'s `Back` handling for
//! the substitute used instead).

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const ROW_H: f32 = 92.0;
const ROW_GAP: f32 = 4.0;
const BAR_W: f32 = 8.0;
const SKEW_DEG: f32 = -9.0;
const LIST_X: f32 = 56.0;
const LIST_TOP: f32 = 158.0; // header (104) + this screen's top:54 offset
const LABEL_GAP: f32 = 26.0; // bar -> number -> label gap, per rowStyle

/// (label, sub-label) — verbatim from the mockup's own `menuDefs`.
pub const ITEMS: [(&str, &str); 5] = [
    ("PLAY", "Start a local freeplay match"),
    ("ONLINE", "Find, host or join a netplay match"),
    ("NETWORK NEWS", "The wire \u{b7} bulletins \u{b7} line notices"),
    ("RANKINGS", "National top 100 \u{b7} season ladder"),
    ("SETTINGS", "Controls \u{b7} video \u{b7} audio \u{b7} netcode"),
];

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    cursor: usize,
    username: &str,
) -> Result<(), String> {
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    // Eyebrow: accent bar + "ARCADE * FREEPLAY ONLINE".
    let eyebrow_y = LIST_TOP - 44.0;
    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(LIST_X, eyebrow_y + 8.0, 30.0, 3.0)))?;
    let (ex, ey) = scale.point(LIST_X + 44.0, eyebrow_y);
    fonts.draw(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(14.0),
        "ARCADE \u{b7} FREEPLAY ONLINE",
        ex,
        ey,
        theme::ACCENT,
    )?;

    for (i, (label, sub)) in ITEMS.iter().enumerate() {
        draw_row(canvas, fonts, scale, i, label, sub, i == cursor)?;
    }

    draw_cabinet_box(canvas, fonts, scale, ITEMS.len() as f32)?;
    draw_ghost_watermark(canvas, fonts, scale)?;

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::PROMPT_NAVIGATE, chrome::PROMPT_SELECT],
        FooterRight::Menu,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    index: usize,
    label: &str,
    sub: &str,
    selected: bool,
) -> Result<(), String> {
    let y = LIST_TOP + index as f32 * (ROW_H + ROW_GAP);

    if selected {
        let tint = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 36); // ~86% transparent
        let clear = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 0);
        geometry::fill_horizontal_gradient_rect(canvas, scale, LIST_X, y, 730.0 * 0.62, ROW_H, tint, clear);
    }

    let bar_color = if selected {
        theme::ACCENT
    } else {
        Color::RGBA(255, 255, 255, 15)
    };
    geometry::fill_skewed_rect(canvas, scale, LIST_X, y, BAR_W, ROW_H, SKEW_DEG, bar_color);

    let num_color = if selected { theme::ACCENT } else { Color::RGB(0x34, 0x34, 0x3a) };
    let num = format!("{:02}", index + 1);
    let (nx, ny) = scale.point(LIST_X + BAR_W + LABEL_GAP, y + ROW_H / 2.0 - 8.0);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(16.0), &num, nx, ny, num_color)?;

    let label_size = if selected { 52.0 } else { 42.0 };
    let label_color = if selected { theme::TEXT } else { Color::RGB(0x6a, 0x6a, 0x72) };
    let label_font = if selected { FpFont::SairaCondensedBlack } else { FpFont::SairaCondensedBold };
    let label_text = label.to_uppercase();
    let sub_text = sub.to_uppercase();
    let label_x = LIST_X + BAR_W + LABEL_GAP + 30.0 + LABEL_GAP;

    // The (label, sub) pair is a `flex-direction:column;gap:5px` block
    // vertically centered within the row — measure real glyph-box heights
    // rather than guessing with a ratio of the point size, which is what
    // produced overlapping label/sub text at first pass.
    let label_h_px = fonts.text_size(label_font, scale.font_px(label_size), &label_text).1 as f32 / scale.s;
    let sub_h_px = if selected {
        fonts.text_size(FpFont::SairaSemiBold, scale.font_px(15.0), &sub_text).1 as f32 / scale.s
    } else {
        0.0
    };
    let gap = if selected { 5.0 } else { 0.0 };
    let block_h = label_h_px + gap + sub_h_px;
    let block_top = y + (ROW_H - block_h) / 2.0;

    let (lx, ly) = scale.point(label_x, block_top);
    fonts.draw(canvas, label_font, scale.font_px(label_size), &label_text, lx, ly, label_color)?;

    if selected {
        let (sx, sy) = scale.point(label_x, block_top + label_h_px + gap);
        fonts.draw(
            canvas,
            FpFont::SairaSemiBold,
            scale.font_px(15.0),
            &sub_text,
            sx,
            sy,
            Color::RGB(0x8a, 0x8a, 0x92),
        )?;
    }

    Ok(())
}

/// "SELECTED CABINET / MORTAL KOMBAT II / ARCADE" info box, directly below
/// the item list.
fn draw_cabinet_box(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    item_count: f32,
) -> Result<(), String> {
    let y = LIST_TOP + item_count * (ROW_H + ROW_GAP) + 42.0;
    let w = 340.0;
    let h = 76.0;
    canvas.set_draw_color(Color::RGBA(14, 14, 18, 178));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(LIST_X, y, w, h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 20));
    canvas.draw_rect(scale.rect(LIST_X, y, w, h))?;

    geometry::fill_skewed_rect(canvas, scale, LIST_X + 18.0, y + 15.0, 9.0, h - 30.0, SKEW_DEG, theme::ACCENT);

    let text_x = LIST_X + 18.0 + 9.0 + LABEL_GAP;
    let (lx, ly) = scale.point(text_x, y + 12.0);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "SELECTED CABINET", lx, ly, theme::MUTE)?;
    let (nx, ny) = scale.point(text_x, y + 30.0);
    fonts.draw(
        canvas,
        FpFont::SairaCondensedBold,
        scale.font_px(20.0),
        "MORTAL KOMBAT II",
        nx,
        ny,
        Color::RGB(0xcf, 0xcf, 0xc9),
    )?;
    let (ax, ay) = scale.point(text_x, y + 54.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(12.0), "ARCADE", ax, ay, Color::RGB(0x52, 0x52, 0x5a))?;
    Ok(())
}

/// Giant near-black "II" watermark at the right edge, per the mockup's
/// ghost-text treatment. Fidelity gap: the mockup layers a second copy of
/// the glyph with `-webkit-text-stroke` (transparent fill, accent-tinted
/// outline only) on top, giving the letters a faint red rim. SDL2/SDL_ttf
/// has no stroke-only text mode without hand-rolling glyph outlining, so
/// only the solid near-black fill layer is reproduced here — the dominant
/// part of the effect, but the red rim is missing.
fn draw_ghost_watermark(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale) -> Result<(), String> {
    let (w, h) = fonts.text_size(FpFont::SairaCondensedBlack, scale.font_px(720.0), "II");
    let (x, y) = scale.point(
        theme::VW + 30.0 - (w as f32 / scale.s),
        theme::VH / 2.0 - (h as f32 / scale.s) / 2.0,
    );
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(720.0), "II", x, y, Color::RGB(0x0c, 0x0c, 0x11))?;
    Ok(())
}
