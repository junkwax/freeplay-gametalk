//! Rankings — matches the mockup's `isRankings` branch, backed by the real
//! community leaderboard (`crate::matchmaking::fetch_leaderboard`) rather
//! than invented data. `main.rs` already fetches this unconditionally at
//! startup into `main_leaderboard` (used by legacy's own, currently-unused
//! `MenuScreen::Leaderboard`); this screen just reads that same value
//! through `super::draw`'s `leaderboard` parameter instead of opening a
//! second fetch pipeline.
//!
//! Simplification vs. the mockup: no per-row rank-delta arrows or "hot
//! streak" badges, since `LeaderboardRow` (username/rating/wins/losses)
//! carries no trend data to back them — inventing that would be fabricated
//! rather than decorative, unlike Network News's static bulletin text.

use super::chrome::{self, FooterRight};
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use crate::matchmaking::LeaderboardRow;
use crate::menu::LeaderboardState;
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const SIDE_PAD: f32 = 56.0;
const TOP: f32 = 38.0 + 104.0;
const MAX_ROWS: usize = 10;
const SKEW_DEG: f32 = -9.0;

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    username: &str,
    leaderboard: &LeaderboardState,
) -> Result<(), String> {
    // The mockup's radial-glow/vignette/skewed-line background is defined
    // once on the shared stage div, above every per-screen `sc-if` branch —
    // every screen gets it, not just Main Menu/Play submenu (the only two
    // that had picked it up so far). Rankings was missing it entirely,
    // rendering flat black instead.
    chrome::draw_background_accents(canvas, scale, SKEW_DEG)?;
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;
    draw_ghost_watermark(canvas, fonts, scale)?;

    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, TOP + 8.0, 30.0, 3.0)))?;
    let (ex, ey) = scale.point(SIDE_PAD + 44.0, TOP);
    fonts.draw(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(13.0),
        "COMMUNITY LADDER",
        ex,
        ey,
        theme::ACCENT,
    )?;
    let (tx, ty) = scale.point(SIDE_PAD, TOP + 26.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(58.0), "RANKINGS", tx, ty, theme::TEXT)?;

    let body_top = TOP + 26.0 + 70.0;
    let body_h = 620.0;
    let w = theme::VW - SIDE_PAD * 2.0;

    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, body_top, w, body_h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(SIDE_PAD, body_top, w, body_h))?;

    match leaderboard {
        LeaderboardState::Loading => {
            draw_status(canvas, fonts, scale, body_top, body_h, w, "Loading leaderboard\u{2026}")?;
        }
        LeaderboardState::Error(message) => {
            draw_status(canvas, fonts, scale, body_top, body_h, w, &format!("Leaderboard unavailable: {message}"))?;
        }
        LeaderboardState::Loaded(rows) => {
            draw_table(canvas, fonts, scale, body_top, w, rows, username)?;
        }
    }

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::PROMPT_NAVIGATE, chrome::PROMPT_BACK],
        FooterRight::Text("GLICKO-2 RATING SYSTEM"),
    )?;
    Ok(())
}

/// Ghost "#1" watermark, right edge — same poor-man's-stroke treatment as
/// the Main Menu's "II" and the Play submenu's "PLAY" (`main_menu.rs`'s
/// `draw_ghost_watermark`), sized/positioned per the mockup's own
/// `right:-40px;top:56%` + `skewX(-9deg)`.
fn draw_ghost_watermark(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale) -> Result<(), String> {
    let px = scale.font_px(560.0);
    let (w, h) = fonts.text_size(FpFont::SairaCondensedBlack, px, "#1");
    let (x, y) = scale.point(
        theme::VW + 40.0 - (w as f32 / scale.s),
        theme::VH * 0.56 - (h as f32 / scale.s) / 2.0,
    );
    let stroke_color = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 71);
    let r = scale.len(1.0).round().max(1.0) as i32;
    for (dx, dy) in [(-r, -r), (0, -r), (r, -r), (-r, 0), (r, 0), (-r, r), (0, r), (r, r)] {
        fonts.draw_italic(canvas, FpFont::SairaCondensedBlack, px, "#1", x + dx, y + dy, stroke_color)?;
    }
    fonts.draw_italic(canvas, FpFont::SairaCondensedBlack, px, "#1", x, y, Color::RGB(0x0c, 0x0c, 0x11))?;
    Ok(())
}

fn draw_status(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    top: f32,
    h: f32,
    w: f32,
    text: &str,
) -> Result<(), String> {
    let (tw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(16.0), text);
    let (x, y) = scale.point(SIDE_PAD + (w - tw as f32 / scale.s) / 2.0, top + h / 2.0 - 8.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(16.0), text, x, y, theme::DIM)?;
    Ok(())
}

fn draw_table(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    top: f32,
    w: f32,
    rows: &[LeaderboardRow],
    username: &str,
) -> Result<(), String> {
    let header_h = 44.0;
    let cols = [SIDE_PAD + 24.0, SIDE_PAD + 140.0, SIDE_PAD + w - 280.0, SIDE_PAD + w - 190.0, SIDE_PAD + w - 100.0];
    let headers = ["RANK", "CODENAME", "W", "L", "RATING"];
    for (i, h) in headers.iter().enumerate() {
        let (hx, hy) = scale.point(cols[i], top + 14.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), h, hx, hy, Color::RGB(0x5a, 0x5a, 0x62))?;
    }
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, top + header_h, w, 1.0)))?;

    if rows.is_empty() {
        draw_status(canvas, fonts, scale, top + header_h, 620.0 - header_h, w, "No ranked players yet")?;
        return Ok(());
    }

    let row_h = 56.0;
    for (i, row) in rows.iter().take(MAX_ROWS).enumerate() {
        let ry = top + header_h + i as f32 * row_h;
        let is_you = row.username.eq_ignore_ascii_case(username);
        if is_you {
            canvas.set_draw_color(Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 22));
            canvas.fill_rect(Some(scale.rect(SIDE_PAD, ry, w, row_h)))?;
            canvas.set_draw_color(theme::ACCENT);
            canvas.fill_rect(Some(scale.rect(SIDE_PAD, ry, 4.0, row_h)))?;
        } else if i > 0 {
            canvas.set_draw_color(Color::RGBA(255, 255, 255, 10));
            canvas.fill_rect(Some(scale.rect(SIDE_PAD, ry, w, 1.0)))?;
        }

        let rank_color = match i {
            0 => Color::RGB(0xe2, 0xb5, 0x3a),
            1 => Color::RGB(0xb8, 0xb8, 0xb2),
            2 => Color::RGB(0x8a, 0x6a, 0x3a),
            _ if is_you => theme::ACCENT,
            _ => Color::RGB(0x6a, 0x6a, 0x72),
        };
        let (rx, ry2) = scale.point(cols[0], ry + row_h / 2.0 - 13.0);
        fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(27.0), &format!("{:02}", i + 1), rx, ry2, rank_color)?;

        let name_color = if is_you { theme::TEXT } else { Color::RGB(0xf2, 0xf2, 0xee) };
        let (nx, ny) = scale.point(cols[1], ry + row_h / 2.0 - 12.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(20.0), &row.username, nx, ny, name_color)?;
        if is_you {
            let (yx, yy) = scale.point(cols[1] + 220.0, ry + row_h / 2.0 - 10.0);
            fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "YOU", yx, yy, theme::ACCENT)?;
        }

        let (wx, wy) = scale.point(cols[2], ry + row_h / 2.0 - 9.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(16.0), &row.wins.to_string(), wx, wy, theme::GREEN)?;
        let (lx, ly) = scale.point(cols[3], ry + row_h / 2.0 - 9.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(16.0), &row.losses.to_string(), lx, ly, Color::RGB(0x7a, 0x7a, 0x82))?;

        let rating_color = if is_you { theme::TEXT } else { Color::RGB(0xcf, 0xcf, 0xc9) };
        let (gx, gy) = scale.point(cols[4], ry + row_h / 2.0 - 10.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(17.0), &row.rating.to_string(), gx, gy, rating_color)?;
    }

    Ok(())
}
