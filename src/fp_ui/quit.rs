//! Quit confirmation overlay — matches `screenshots/06-quit.png`.
//!
//! Rendered on top of the Main Menu (the caller draws the menu first, then
//! this), not as a screen replacement, per the handoff doc: "Rendered on
//! top of the menu (stack above it, not replace)".

use super::chrome;
use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const MODAL_W: f32 = 580.0;
const TOP_BAR_H: f32 = 5.0;
const PAD_X: f32 = 48.0;
const PAD_TOP: f32 = 44.0;
const PAD_BOTTOM: f32 = 40.0;

pub fn draw(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, choice: usize) -> Result<(), String> {
    // Dim backdrop over the whole screen (rgba(4,4,6,.82)).
    canvas.set_draw_color(Color::RGBA(4, 4, 6, 209));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(0.0, 0.0, theme::VW, theme::VH)))?;

    let icon_box = 52.0;
    let title = "EXIT TO SYSTEM?";
    let subtitle = "You will leave FREEPLAY and return to the OS.";

    // Height: top bar + padding + (icon row) + gap + button row + padding.
    let icon_row_h = icon_box;
    let modal_h = PAD_TOP + icon_row_h + 30.0 + 62.0 + PAD_BOTTOM;
    let modal_x = (theme::VW - MODAL_W) / 2.0;
    let modal_y = (theme::VH - modal_h) / 2.0;

    canvas.set_draw_color(Color::RGB(0x0d, 0x0d, 0x11));
    canvas.fill_rect(Some(scale.rect(modal_x, modal_y, MODAL_W, modal_h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 26));
    canvas.draw_rect(scale.rect(modal_x, modal_y, MODAL_W, modal_h))?;
    canvas.set_draw_color(theme::ACCENT);
    canvas.fill_rect(Some(scale.rect(modal_x, modal_y, MODAL_W, TOP_BAR_H)))?;

    let content_x = modal_x + PAD_X;
    let content_y = modal_y + PAD_TOP;

    // Icon box: bordered square with a power glyph. No power icon in either
    // bundled font, so an accent skewed bar stands in for it (flagged below,
    // not silently identical to the mockup's SVG icon).
    canvas.set_draw_color(Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 22));
    canvas.fill_rect(Some(scale.rect(content_x, content_y, icon_box, icon_box)))?;
    canvas.set_draw_color(Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 115));
    canvas.draw_rect(scale.rect(content_x, content_y, icon_box, icon_box))?;
    geometry::fill_skewed_rect(
        canvas,
        scale,
        content_x + icon_box / 2.0 - 3.0,
        content_y + 12.0,
        6.0,
        icon_box - 24.0,
        0.0,
        theme::ACCENT,
    );

    let text_x = content_x + icon_box + 18.0;
    let (tx, ty) = scale.point(text_x, content_y);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(38.0), title, tx, ty, theme::TEXT)?;
    let (sx, sy) = scale.point(text_x, content_y + 44.0);
    fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), subtitle, sx, sy, Color::RGB(0x8a, 0x8a, 0x92))?;

    // Buttons.
    let btn_y = content_y + icon_row_h + 30.0;
    let btn_h = 62.0;
    let gap = 14.0;
    let btn_w = (MODAL_W - PAD_X * 2.0 - gap) / 2.0;
    draw_button(canvas, fonts, scale, content_x, btn_y, btn_w, btn_h, "CANCEL", choice == 0, false)?;
    draw_button(canvas, fonts, scale, content_x + btn_w + gap, btn_y, btn_w, btn_h, "EXIT GAME", choice == 1, true)?;

    // Footer prompt row override: Choose / Confirm / Cancel, per the
    // mockup's quit-specific footer (left/right chooses, cross confirms,
    // circle cancels) rather than the Main Menu's Navigate/Select.
    let row_cy = theme::VH - chrome::FOOTER_H / 2.0;
    canvas.set_draw_color(Color::RGBA(4, 4, 6, 209));
    canvas.fill_rect(Some(scale.rect(0.0, theme::VH - chrome::FOOTER_H, theme::VW, chrome::FOOTER_H)))?;
    chrome::draw_prompt_row(
        canvas,
        fonts,
        scale,
        &[
            chrome::FooterPrompt { glyph: "\u{2194}", label: "Choose", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::PROMPT_SELECT,
            chrome::FooterPrompt { glyph: "O", label: "Cancel", color: theme::BTN_CIRCLE },
        ],
        56.0,
        row_cy,
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_button(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    selected: bool,
    is_exit: bool,
) -> Result<(), String> {
    let (border, bg, text_color) = if selected && is_exit {
        (theme::ACCENT, theme::ACCENT, Color::RGB(255, 255, 255))
    } else if selected {
        (Color::RGBA(255, 255, 255, 128), Color::RGBA(255, 255, 255, 20), theme::TEXT)
    } else {
        (Color::RGBA(255, 255, 255, 31), Color::RGBA(0, 0, 0, 0), Color::RGB(0x8a, 0x8a, 0x92))
    };
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    if bg.a > 0 {
        canvas.set_draw_color(bg);
        canvas.fill_rect(Some(scale.rect(x, y, w, h)))?;
    }
    canvas.set_draw_color(border);
    canvas.draw_rect(scale.rect(x, y, w, h))?;

    let (tw, th) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(23.0), label);
    let (tx, ty) = scale.point(x + w / 2.0 - (tw as f32 / scale.s) / 2.0, y + h / 2.0 - (th as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(23.0), label, tx, ty, text_color)?;
    Ok(())
}
