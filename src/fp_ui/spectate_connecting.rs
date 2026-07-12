//! Spectate connecting — native rendering for the waiting phase of
//! `MenuScreen::Spectate` (picked from the Watch tab or a spectate deep
//! link) before the first status frame arrives, matching the updated
//! mockup's `isWatchConnecting` branch: the shared radar animation with a
//! play-badge center, the two player names and score, and the live status
//! message with legacy's own trailing-dots cycle. Once real frames start
//! flowing (`status.frame.is_some()`), main.rs falls back to the legacy
//! spectate viewer — the *viewer* stays legacy per the agreed scope, only
//! this connecting state is native. Real data only: everything shown comes
//! straight from `SpectateStatus`.

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::matchmaking;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use crate::menu::SpectateStatus;
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const RADAR_R: f32 = 140.0;

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    status: &SpectateStatus,
    username: &str,
) -> Result<(), String> {
    chrome::draw_background_accents_no_glow(canvas, scale)?;
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    // Radar on the left, text block on the right — the mockup's
    // side-by-side `isWatchConnecting` layout.
    let cx = theme::VW * 0.30;
    let cy = theme::VH / 2.0 - 20.0;
    matchmaking::draw_radar(canvas, scale, cx, cy, RADAR_R)?;
    // "LIVE" as text — the mockup's ▶ glyph isn't in any bundled font.
    geometry::fill_circle(canvas, scale, cx, cy, 32.0, Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 230));
    let (pw, ph) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(17.0), "LIVE");
    let (px, py) = scale.point(cx - (pw as f32 / scale.s) / 2.0, cy - (ph as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(17.0), "LIVE", px, py, Color::RGB(255, 255, 255))?;

    let text_x = cx + RADAR_R + 110.0;
    let mut y = cy - 92.0;

    let (tx, ty) = scale.point(text_x, y);
    fonts.draw(
        canvas,
        FpFont::SairaCondensedBold,
        scale.font_px(40.0),
        "CONNECTING TO LIVE MATCH",
        tx,
        ty,
        theme::TEXT,
    )?;
    y += 66.0;

    // P1  score  P2 — laid out left to right at mixed sizes.
    let name_px = scale.font_px(30.0);
    let score_px = scale.font_px(22.0);
    let mut x = text_x;
    let (w1, _) = fonts.text_size(FpFont::SairaCondensedBold, name_px, &status.p1_name);
    let (nx, ny) = scale.point(x, y);
    fonts.draw(canvas, FpFont::SairaCondensedBold, name_px, &status.p1_name, nx, ny, theme::TEXT)?;
    x += w1 as f32 / scale.s + 22.0;
    let score = format!("{}-{}", status.p1_score, status.p2_score);
    let (ws, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, score_px, &score);
    let (sx, sy) = scale.point(x, y + 6.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, score_px, &score, sx, sy, theme::ACCENT, scale.len(3.0).round() as i32)?;
    x += ws as f32 / scale.s + 22.0 + 9.0;
    let (n2x, n2y) = scale.point(x, y);
    fonts.draw(canvas, FpFont::SairaCondensedBold, name_px, &status.p2_name, n2x, n2y, theme::TEXT)?;
    let _ = (ny, sy, n2y);
    y += 56.0;

    let line = format!("{}{}", status.message.trim_end_matches('.'), matchmaking::dots());
    let (mx, my) = scale.point(text_x, y);
    fonts.draw(canvas, FpFont::SairaMedium, scale.font_px(16.0), &line, mx, my, theme::DIM)?;
    y += 44.0;

    let (hx, hy) = scale.point(text_x, y);
    fonts.draw_tracked(
        canvas,
        FpFont::ChakraPetchMedium,
        scale.font_px(12.0),
        "CIRCLE TO CANCEL",
        hx,
        hy,
        Color::RGB(0x7a, 0x7a, 0x82),
        scale.len(2.0).round() as i32,
    )?;

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::PROMPT_BACK],
        FooterRight::Text("SPECTATE \u{b7} LIVE"),
    )?;
    Ok(())
}
