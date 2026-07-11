//! Claim Username — native redesign of legacy's `MenuScreen::MatchUsername`,
//! the first-time-online screen that lets a player keep or edit their
//! auto-generated name before it's reserved server-side. Matches the new
//! mockup's `isClaim` branch.
//!
//! Typing is real hardware-keyboard input via SDL's `TextInput`/`Backspace`
//! events (`AppState::text_input`/`text_backspace` in `menu.rs`, widened to
//! also cover this screen) — the exact same mechanism legacy's
//! `MatchUsername` already uses, not a re-implementation and not a
//! delegation to anything else. SDL's text-input mode is also what surfaces
//! the OS/controller-overlay's own on-screen keyboard on platforms that have
//! one (Steam Deck, Windows tablet mode, ...), so gamepad-only players get
//! the same affordance here that they already get on every other real-
//! keyboard-entry legacy screen (Join Code, Account Username/Email) — there
//! is no separate "OSK" widget anywhere in this app to delegate to.
//!
//! `status`/`checking` are free-text/bool, not a closed enum — mirrors
//! legacy's own fields exactly, since `main.rs` drives both from the same
//! `matchmaking::check_username_available` round trip regardless of which
//! screen displays them.

use super::chrome::{self, FooterRight};
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const CARD_W: f32 = 720.0;
const TOP_BAR_H: f32 = 5.0;
const PAD_X: f32 = 52.0;
const PAD_TOP: f32 = 44.0;
const PAD_BOTTOM: f32 = 40.0;
const INPUT_H: f32 = 64.0;
const BTN_H: f32 = 62.0;

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    value: &str,
    status: &str,
    checking: bool,
) -> Result<(), String> {
    chrome::draw_background_accents(canvas, scale)?;
    // No confirmed identity yet — this screen exists specifically because
    // one doesn't exist, so the header's profile chip shows the in-progress
    // value rather than a blank/placeholder name.
    let header_name = if value.trim().is_empty() { "PLAYER" } else { value };
    chrome::draw_header(canvas, fonts, scale, header_name, true, None)?;

    let card_h = PAD_TOP + 30.0 + 44.0 + 24.0 + INPUT_H + 22.0 + 30.0 + BTN_H + PAD_BOTTOM;
    let card_x = (theme::VW - CARD_W) / 2.0;
    let card_y = (theme::VH - card_h) / 2.0;

    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.set_draw_color(Color::RGB(0x0d, 0x0d, 0x11));
    canvas.fill_rect(Some(scale.rect(card_x, card_y, CARD_W, card_h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 26));
    canvas.draw_rect(scale.rect(card_x, card_y, CARD_W, card_h))?;
    canvas.set_draw_color(theme::ACCENT);
    canvas.fill_rect(Some(scale.rect(card_x, card_y, CARD_W, TOP_BAR_H)))?;

    let content_x = card_x + PAD_X;
    let mut y = card_y + PAD_TOP;

    let (ex, ey) = scale.point(content_x, y);
    fonts.draw_tracked(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(13.0),
        "ONBOARDING \u{b7} FIRST TIME ONLINE",
        ex,
        ey,
        theme::ACCENT,
        scale.len(4.0).round() as i32,
    )?;
    y += 30.0;
    let (tx, ty) = scale.point(content_x, y);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(34.0), "CLAIM YOUR CALLSIGN", tx, ty, theme::TEXT)?;
    y += 44.0;

    let hint = "This is the name other players see. Edit it, then press Enter to claim it.";
    let (hx, hy) = scale.point(content_x, y);
    fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), hint, hx, hy, Color::RGB(0x8a, 0x8a, 0x92))?;
    y += 24.0;

    // Text input box, blinking caret after the typed value.
    let input_w = CARD_W - PAD_X * 2.0;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 15));
    canvas.fill_rect(Some(scale.rect(content_x, y, input_w, INPUT_H)))?;
    canvas.set_draw_color(if checking { Color::RGBA(255, 255, 255, 40) } else { theme::ACCENT });
    canvas.draw_rect(scale.rect(content_x, y, input_w, INPUT_H))?;
    let (vx, vy) = scale.point(content_x + 20.0, y + INPUT_H / 2.0 - 15.0);
    let (vw, _) = fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(30.0), value, vx, vy, theme::TEXT)?;
    if !checking {
        let caret_x = content_x + 20.0 + (vw as f32 / scale.s) + 4.0;
        canvas.set_draw_color(theme::ACCENT);
        canvas.fill_rect(Some(scale.rect(caret_x, y + 14.0, 3.0, INPUT_H - 28.0)))?;
    }
    y += INPUT_H + 22.0;

    // Status row: dot colored by outcome + the free-text message main.rs sends.
    let dot_color = classify_status(status, checking);
    super::geometry::fill_circle(canvas, scale, content_x + 5.0, y - 6.0, 5.0, dot_color);
    let (sx, sy) = scale.point(content_x + 20.0, y - 14.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(15.0), status, sx, sy, Color::RGB(0xcf, 0xcf, 0xc9))?;
    y += 30.0;

    let enabled = !checking && !value.trim().is_empty();
    draw_submit_button(canvas, fonts, scale, content_x, y, input_w, BTN_H, enabled, checking)?;

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::PROMPT_SELECT],
        FooterRight::Text("TYPE TO EDIT \u{b7} ENTER TO CLAIM"),
    )?;
    Ok(())
}

/// Legacy sends free-text status messages, not a closed enum — this
/// heuristically colors the status dot from the same message content a
/// player reads, rather than inventing a parallel classification that could
/// drift out of sync with `main.rs`'s actual strings.
fn classify_status(status: &str, checking: bool) -> Color {
    if checking {
        return Color::RGB(0x8a, 0x8a, 0x92);
    }
    let lower = status.to_lowercase();
    if lower.contains("taken") || lower.contains("invalid") || lower.contains("timed out") || lower.contains("stopped") {
        theme::WARNING
    } else {
        theme::GREEN
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_submit_button(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    enabled: bool,
    checking: bool,
) -> Result<(), String> {
    let (bg, text_color) = if checking {
        (Color::RGBA(255, 255, 255, 15), Color::RGB(0x8a, 0x8a, 0x92))
    } else if enabled {
        (theme::ACCENT, Color::RGB(255, 255, 255))
    } else {
        (Color::RGBA(255, 255, 255, 15), Color::RGB(0x5a, 0x5a, 0x62))
    };
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.set_draw_color(bg);
    canvas.fill_rect(Some(scale.rect(x, y, w, h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 26));
    canvas.draw_rect(scale.rect(x, y, w, h))?;

    let label = if checking { "CHECKING\u{2026}" } else { "CLAIM CALLSIGN & CONTINUE" };
    let (tw, th) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(22.0), label);
    let (tx, ty) = scale.point(x + w / 2.0 - (tw as f32 / scale.s) / 2.0, y + h / 2.0 - (th as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(22.0), label, tx, ty, text_color)?;
    Ok(())
}
