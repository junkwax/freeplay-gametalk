//! Connection Failed — native replacement for the legacy `TestResult`
//! screen's role as the netplay failure report (relay handshake timeouts,
//! matchmaking errors, sessions that never start). Matches the updated
//! mockup's `isFailure` branch: the same centered-card skeleton as Session
//! Ended, an error-leaning header, verdict lines colored by their
//! `OK `/`WARN `/`FAIL ` prefixes, and a single RETURN TO MENU action.
//! `lines` is the exact same list main.rs already builds for the legacy
//! screen (an incident report has always been auto-submitted before this
//! appears — the construction site appends a line saying so, since that's
//! a real fact worth surfacing rather than fabricated reassurance).
//!
//! Legacy `MenuScreen::TestResult` itself is untouched — it still serves
//! the legacy-UI path and legacy probe results.

use super::chrome;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const CARD_W: f32 = 760.0;
const PAD_X: f32 = 52.0;
const PAD_TOP: f32 = 40.0;
const PAD_BOTTOM: f32 = 36.0;
const LINE_H: f32 = 30.0;
const BTN_H: f32 = 62.0;

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    lines: &[String],
    username: &str,
) -> Result<(), String> {
    chrome::draw_background_accents_no_glow(canvas, scale)?;
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    // Legacy lines include blank spacers and a literal "ESC to go back"
    // hint — the card's own spacing and footer already cover both.
    let shown: Vec<&String> = lines
        .iter()
        .filter(|l| !l.trim().is_empty() && !l.trim().eq_ignore_ascii_case("ESC to go back"))
        .collect();

    let lines_h = (shown.len().max(1)) as f32 * LINE_H;
    let card_h = PAD_TOP + 30.0 + 44.0 + lines_h + 30.0 + BTN_H + PAD_BOTTOM;
    let card_x = (theme::VW - CARD_W) / 2.0;
    let card_y = (theme::VH - card_h) / 2.0;

    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(4, 4, 6, 190));
    canvas.fill_rect(Some(scale.rect(0.0, 0.0, theme::VW, theme::VH)))?;

    canvas.set_draw_color(Color::RGB(0x0d, 0x0d, 0x11));
    canvas.fill_rect(Some(scale.rect(card_x, card_y, CARD_W, card_h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 26));
    canvas.draw_rect(scale.rect(card_x, card_y, CARD_W, card_h))?;
    canvas.set_draw_color(theme::ACCENT);
    canvas.fill_rect(Some(scale.rect(card_x, card_y, CARD_W, 5.0)))?;

    let content_x = card_x + PAD_X;
    let mut y = card_y + PAD_TOP;

    let (ex, ey) = scale.point(content_x, y);
    fonts.draw_tracked(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(13.0),
        "CONNECTION FAILED",
        ex,
        ey,
        theme::ACCENT,
        scale.len(4.0).round() as i32,
    )?;
    y += 30.0;
    let (tx, ty) = scale.point(content_x, y);
    fonts.draw(
        canvas,
        FpFont::SairaCondensedBold,
        scale.font_px(34.0),
        "NETPLAY SESSION COULD NOT START",
        tx,
        ty,
        theme::TEXT,
    )?;
    y += 44.0;

    for line in shown {
        let (dot_color, text) = if let Some(rest) = line.strip_prefix("FAIL ") {
            (theme::ACCENT, rest)
        } else if let Some(rest) = line.strip_prefix("WARN ") {
            (theme::WARNING, rest)
        } else if let Some(rest) = line.strip_prefix("OK ") {
            (theme::GREEN, rest)
        } else {
            (Color::RGB(0x62, 0x62, 0x6c), line.as_str())
        };
        super::geometry::fill_circle(canvas, scale, content_x + 4.0, y + LINE_H / 2.0, 4.0, dot_color);
        let (lx, ly) = scale.point(content_x + 20.0, y + LINE_H / 2.0 - 9.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(16.0), text, lx, ly, Color::RGB(0xcf, 0xcf, 0xc9))?;
        y += LINE_H;
    }
    y += 30.0;

    // Single primary action, always selected.
    let btn_w = CARD_W - PAD_X * 2.0;
    canvas.set_draw_color(theme::ACCENT);
    canvas.fill_rect(Some(scale.rect(content_x, y, btn_w, BTN_H)))?;
    let (bw, bh) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(21.0), "RETURN TO MENU");
    let (bx, by) = scale.point(content_x + btn_w / 2.0 - (bw as f32 / scale.s) / 2.0, y + BTN_H / 2.0 - (bh as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(21.0), "RETURN TO MENU", bx, by, Color::RGB(255, 255, 255))?;

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::PROMPT_SELECT],
        chrome::FooterRight::Text("INCIDENT REPORT \u{b7} freeplay-net.log"),
    )?;
    Ok(())
}
