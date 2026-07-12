//! Rebind capture modal — native rendering for `AppState::Rebinding` when
//! the rebind was started from fp_ui's Settings→Controls category. The
//! capture *state machine* (any-input capture via `capture_rebind`, Delete/
//! Backspace clears, Esc/Circle cancels, `finish_rebind` returning to
//! `came_from`) is untouched in main.rs — this only replaces the brief
//! legacy-styled flash the old handoff showed. Drawn over the dimmed
//! `came_from` Settings screen (main.rs draws that first), matching the
//! updated mockup's in-Controls capture treatment ("PRESS ANY INPUT...",
//! `fp-blink`).

use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use crate::input::{Action, Player};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const CARD_W: f32 = 640.0;
const CARD_H: f32 = 236.0;

pub fn draw_modal(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    action: Action,
    player: Player,
) -> Result<(), String> {
    let card_x = (theme::VW - CARD_W) / 2.0;
    let card_y = (theme::VH - CARD_H) / 2.0;

    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(2, 2, 4, 184));
    canvas.fill_rect(Some(scale.rect(0.0, 0.0, theme::VW, theme::VH)))?;

    canvas.set_draw_color(Color::RGBA(10, 10, 13, 235));
    canvas.fill_rect(Some(scale.rect(card_x, card_y, CARD_W, CARD_H)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 26));
    canvas.draw_rect(scale.rect(card_x, card_y, CARD_W, CARD_H))?;
    canvas.set_draw_color(theme::ACCENT);
    canvas.fill_rect(Some(scale.rect(card_x, card_y, CARD_W, 5.0)))?;

    let content_x = card_x + 44.0;
    let mut y = card_y + 34.0;

    let (ex, ey) = scale.point(content_x, y);
    fonts.draw_tracked(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(13.0),
        "REBIND CONTROL",
        ex,
        ey,
        theme::ACCENT,
        scale.len(5.0).round() as i32,
    )?;
    y += 26.0;

    let title = format!("{} \u{2014} {}", action.label().to_uppercase(), player.label());
    let (tx, ty) = scale.point(content_x, y);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(38.0), &title, tx, ty, theme::TEXT)?;
    y += 62.0;

    // The mockup's `fp-blink 0.8s step-end`: full for the first half of
    // each cycle, dimmed for the second.
    let bright = super::matchmaking::elapsed_ms() % 800 < 400;
    let prompt = "PRESS ANY BUTTON, KEY, OR STICK DIRECTION";
    let (px, py) = scale.point(content_x, y);
    fonts.draw_tracked(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(15.0),
        prompt,
        px,
        py,
        if bright {
            theme::ACCENT
        } else {
            Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 70)
        },
        scale.len(2.0).round() as i32,
    )?;
    y += 44.0;

    // Circle is deliberately not listed as a cancel: during capture every
    // controller button (including Circle) is a *candidate binding* — only
    // Esc escapes `capture_rebind` uncaptured.
    let hint = "DELETE / BACKSPACE CLEARS \u{b7} ESC CANCELS";
    let (hx, hy) = scale.point(content_x, y);
    fonts.draw_tracked(
        canvas,
        FpFont::ChakraPetchMedium,
        scale.font_px(11.0),
        hint,
        hx,
        hy,
        Color::RGB(0x5a, 0x5a, 0x62),
        scale.len(2.0).round() as i32,
    )?;
    Ok(())
}
