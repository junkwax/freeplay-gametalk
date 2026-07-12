//! Discord connect — native waiting screen for the browser OAuth round
//! trip, per the updated mockup's `discordOpen` modal: a spinner, the live
//! status string ("Opening Discord login..." → "Waiting for
//! authorization..."), and a cancel hint. Replaces the previous fp-side
//! treatment of this flow, which borrowed the Find Match radar screen — a
//! match-search radar was an odd fit for an OAuth wait (the design brief's
//! open question, answered by the design agent with this minimal card).
//! `status` is updated by the same `matchmaking::Update::Status` round trip
//! the Matchmaking screen uses; cancellation is the same raw
//! `is_cancel(&event)` check in main.rs (`is_matchmaking_screen`), landing
//! back on Settings→Account rather than the main menu.

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::matchmaking;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const CARD_W: f32 = 520.0;
const CARD_H: f32 = 320.0;

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    status: &str,
    username: &str,
) -> Result<(), String> {
    chrome::draw_background_accents_no_glow(canvas, scale)?;
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    let card_x = (theme::VW - CARD_W) / 2.0;
    let card_y = (theme::VH - CARD_H) / 2.0 - 20.0;

    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(10, 10, 13, 235));
    canvas.fill_rect(Some(scale.rect(card_x, card_y, CARD_W, CARD_H)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 26));
    canvas.draw_rect(scale.rect(card_x, card_y, CARD_W, CARD_H))?;
    canvas.set_draw_color(theme::ACCENT);
    canvas.fill_rect(Some(scale.rect(card_x, card_y, CARD_W, 5.0)))?;

    let center_x = card_x + CARD_W / 2.0;
    let mut y = card_y + 38.0;

    let eyebrow = "DISCORD";
    let epx = scale.font_px(13.0);
    // `text_size` doesn't know about tracking — add it in ((n-1) gaps of 6
    // logical px) so the centered position accounts for the full run.
    let (ew, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, epx, eyebrow);
    let etw = ew as f32 / scale.s + (eyebrow.len().saturating_sub(1)) as f32 * 6.0;
    let (ex, ey) = scale.point(center_x - etw / 2.0, y);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, epx, eyebrow, ex, ey, theme::ACCENT, scale.len(6.0).round() as i32)?;
    y += 44.0;

    // Spinner: a faint full ring plus an accent "comet" — a head dot with a
    // fading tail sweeping one revolution per 0.9s, standing in for the
    // mockup's border-top rotating circle (SDL has no arc primitive).
    let spin_cy = y + 28.0;
    let spin_r = 26.0;
    geometry::stroke_circle(canvas, scale, center_x, spin_cy, spin_r, 2.0, Color::RGBA(255, 255, 255, 30));
    let t = (matchmaking::elapsed_ms() % 900) as f32 / 900.0;
    let head = t * std::f32::consts::TAU;
    for i in 0..12 {
        let a = head - i as f32 * 0.16;
        let alpha = (200.0 * (1.0 - i as f32 / 12.0)) as u8;
        geometry::fill_circle(
            canvas,
            scale,
            center_x + a.cos() * spin_r,
            spin_cy + a.sin() * spin_r,
            2.6,
            Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, alpha),
        );
    }
    y += 84.0;

    let line = format!("{}{}", status.trim_end_matches('.'), matchmaking::dots());
    let spx = scale.font_px(24.0);
    let (sw, _) = fonts.text_size(FpFont::SairaCondensedBold, spx, &line);
    let (sx, sy) = scale.point(center_x - (sw as f32 / scale.s) / 2.0, y);
    fonts.draw(canvas, FpFont::SairaCondensedBold, spx, &line, sx, sy, theme::TEXT)?;
    y += 40.0;

    let sub = "Complete login in your browser window.";
    let bpx = scale.font_px(14.0);
    let (bw, _) = fonts.text_size(FpFont::SairaMedium, bpx, sub);
    let (bx, by) = scale.point(center_x - (bw as f32 / scale.s) / 2.0, y);
    fonts.draw(canvas, FpFont::SairaMedium, bpx, sub, bx, by, Color::RGB(0x8a, 0x8a, 0x92))?;
    y += 44.0;

    let hint = "CIRCLE TO CANCEL";
    let hpx = scale.font_px(12.0);
    let (hw, _) = fonts.text_size(FpFont::ChakraPetchMedium, hpx, hint);
    let htw = hw as f32 / scale.s + (hint.len().saturating_sub(1)) as f32 * 2.0;
    let (hx, hy) = scale.point(center_x - htw / 2.0, y);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchMedium, hpx, hint, hx, hy, Color::RGB(0x7a, 0x7a, 0x82), scale.len(2.0).round() as i32)?;

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::PROMPT_BACK],
        FooterRight::Text("ACCOUNT \u{b7} DISCORD LINK"),
    )?;
    Ok(())
}
