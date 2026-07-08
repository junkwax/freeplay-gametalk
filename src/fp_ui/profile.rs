//! Profile — matches the mockup's `isProfile` branch (found in the fuller
//! "FREEPLAY Arcade (standalone).html" reference, not the earlier
//! `freeplay-frontend` copy this module set was originally built against —
//! that's why CLAUDE.md's history says no native Profile screen exists yet).
//! Reached from the Main Menu's "YOUR STATS" panel (`Confirm` while
//! focused) rather than delegating to the legacy bitmap-font Profile
//! screen the way Lab/Replays/Drones still do — this one reuses the exact
//! same `ProfileScreenState` the caller already fetches for the "YOUR
//! STATS"/"LAST MATCH" panels, so no new fetch pipeline either.
//!
//! Two fields in the mockup have no real backend equivalent and are
//! adjusted rather than fabricated, same policy as the Main Menu's stats
//! panel: `profileRank` becomes `menu::estimate_rank(profile.rating)` (a
//! real derivation from the real rating, already used by the legacy
//! Profile screen) instead of an invented tier name, and the mockup's
//! "NA-WEST" region text is dropped entirely (no per-player region concept
//! exists anywhere in this app).

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::main_menu::short_date;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use crate::matchmaking::HistoryRow;
use crate::menu::{estimate_rank, ProfileScreenState};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const SIDE_PAD: f32 = 56.0;
const TOP: f32 = 38.0 + 104.0;
const LEFT_W: f32 = 370.0;
const COL_GAP: f32 = 48.0;

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    username: &str,
    profile: &ProfileScreenState,
) -> Result<(), String> {
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, TOP + 8.0, 30.0, 3.0)))?;
    let (ex, ey) = scale.point(SIDE_PAD + 44.0, TOP);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "PLAYER RECORD", ex, ey, theme::ACCENT, scale.len(7.0).round() as i32)?;
    let (tx, ty) = scale.point(SIDE_PAD, TOP + 26.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(58.0), "PROFILE", tx, ty, theme::TEXT)?;

    let body_top = TOP + 26.0 + 70.0;
    let body_h = 620.0;

    match profile {
        ProfileScreenState::Loaded { profile, history, .. } => {
            draw_left_column(canvas, fonts, scale, body_top, username, profile, history)?;
            draw_right_column(canvas, fonts, scale, body_top, body_h, profile, history)?;
        }
        other => {
            let msg = match other {
                ProfileScreenState::Loading => "Loading\u{2026}".to_string(),
                ProfileScreenState::Error(e) => format!("Profile unavailable: {e}"),
                ProfileScreenState::Empty { .. } => "No ranked matches yet — play an online match to build a record.".to_string(),
                ProfileScreenState::NotLoggedIn => "Not signed in.".to_string(),
                ProfileScreenState::Loaded { .. } => unreachable!(),
            };
            let (mw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(16.0), &msg);
            let (mx, my) = scale.point((theme::VW - mw as f32 / scale.s) / 2.0, body_top + body_h / 2.0 - 8.0);
            fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(16.0), &msg, mx, my, theme::DIM)?;
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

fn draw_left_column(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    top: f32,
    username: &str,
    profile: &crate::matchmaking::ProfileData,
    history: &[HistoryRow],
) -> Result<(), String> {
    let x = SIDE_PAD;
    let avatar_d = 114.0;

    geometry::fill_circle(canvas, scale, x + avatar_d / 2.0, top + avatar_d / 2.0, avatar_d / 2.0, Color::RGB(0x8a, 0x14, 0x1a));
    let initial = username.chars().next().unwrap_or('?').to_uppercase().to_string();
    let (iw, ih) = fonts.text_size(FpFont::SairaCondensedBlack, scale.font_px(48.0), &initial);
    let (ix, iy) = scale.point(x + avatar_d / 2.0 - (iw as f32 / scale.s) / 2.0, top + avatar_d / 2.0 - (ih as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(48.0), &initial, ix, iy, Color::RGB(255, 255, 255))?;

    let name_x = x + avatar_d + 22.0;
    let (nx, ny) = scale.point(name_x, top + 6.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(30.0), &username.to_uppercase(), nx, ny, theme::TEXT)?;

    let rank = estimate_rank(profile.rating);
    let (rw, rh) = fonts.text_size(FpFont::ChakraPetchBold, scale.font_px(12.0), rank);
    let pad_x = 13.0;
    let pad_y = 5.0;
    let badge_y = top + 44.0;
    canvas.set_draw_color(Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 40));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(name_x, badge_y, (rw as f32 / scale.s) + pad_x * 2.0, (rh as f32 / scale.s) + pad_y * 2.0)))?;
    let (bx, by) = scale.point(name_x + pad_x, badge_y + pad_y);
    fonts.draw(canvas, FpFont::ChakraPetchBold, scale.font_px(12.0), rank, bx, by, theme::ACCENT)?;

    let dot_y = badge_y + 34.0;
    geometry::fill_circle(canvas, scale, name_x + 4.0, dot_y, 3.5, theme::GREEN);
    let (onx, ony) = scale.point(name_x + 14.0, dot_y - 6.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchMedium, scale.font_px(11.0), "ONLINE", onx, ony, theme::GREEN, scale.len(2.0).round() as i32)?;

    let card_y = top + avatar_d + 16.0;
    let card_h = 150.0;
    canvas.set_draw_color(Color::RGBA(8, 8, 11, 178));
    canvas.fill_rect(Some(scale.rect(x, card_y, LEFT_W, card_h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 20));
    canvas.draw_rect(scale.rect(x, card_y, LEFT_W, card_h))?;
    let (glx, gly) = scale.point(x + 22.0, card_y + 20.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "GLICKO RATING", glx, gly, Color::RGB(0x52, 0x52, 0x5a), scale.len(4.0).round() as i32)?;

    let rating_text = profile.rating.to_string();
    let (rgx, rgy) = scale.point(x + 22.0, card_y + 40.0);
    let (rating_w, _) = fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(48.0), &rating_text, rgx, rgy, theme::TEXT)?;
    let (rdx, rdy) = scale.point(x + 22.0 + (rating_w as f32 / scale.s) + 14.0, card_y + 46.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(13.0), &format!("\u{b1}{}", profile.deviation), rdx, rdy, Color::RGB(0x8a, 0x8a, 0x92))?;
    let (devx, devy) = scale.point(x + 22.0 + (rating_w as f32 / scale.s) + 14.0, card_y + 64.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchMedium, scale.font_px(11.0), "DEVIATION", devx, devy, Color::RGB(0x3a, 0x3a, 0x42), scale.len(2.0).round() as i32)?;

    let bar_y = card_y + 100.0;
    let bar_w = LEFT_W - 44.0;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 15));
    canvas.fill_rect(Some(scale.rect(x + 22.0, bar_y, bar_w, 4.0)))?;
    let pct = (profile.rating as f32 / 3000.0).clamp(0.0, 1.0);
    canvas.set_draw_color(theme::ACCENT);
    canvas.fill_rect(Some(scale.rect(x + 22.0, bar_y, bar_w * pct, 4.0)))?;
    for (i, label) in ["1000", "2000", "3000+"].iter().enumerate() {
        let lx = x + 22.0 + bar_w * (i as f32 / 2.0);
        let (px, py) = scale.point(lx, bar_y + 10.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(10.0), label, px, py, Color::RGB(0x2e, 0x2e, 0x36))?;
    }

    let last_y = card_y + card_h + 16.0;
    let last_h = 60.0;
    canvas.set_draw_color(Color::RGBA(14, 14, 18, 128));
    canvas.fill_rect(Some(scale.rect(x, last_y, LEFT_W, last_h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(x, last_y, LEFT_W, last_h))?;
    let (llx, lly) = scale.point(x + 18.0, last_y + last_h / 2.0 - 6.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "LAST MATCH", llx, lly, Color::RGB(0x3a, 0x3a, 0x42), scale.len(3.0).round() as i32)?;
    let last_played = history.first().map(|r| short_date(&r.played_at)).unwrap_or_else(|| "\u{2014}".to_string());
    let (lw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(13.0), &last_played);
    let (lpx, lpy) = scale.point(x + LEFT_W - 18.0 - (lw as f32 / scale.s), last_y + last_h / 2.0 - 7.0);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), &last_played, lpx, lpy, Color::RGB(0x8a, 0x8a, 0x92))?;

    Ok(())
}

fn draw_right_column(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    top: f32,
    body_h: f32,
    profile: &crate::matchmaking::ProfileData,
    history: &[HistoryRow],
) -> Result<(), String> {
    let x = SIDE_PAD + LEFT_W + COL_GAP;
    let w = theme::VW - SIDE_PAD - x;

    let total = profile.wins + profile.losses;
    let win_rate = if total > 0 { (profile.wins as f64 / total as f64 * 100.0) as u32 } else { 0 };
    let stats: [(&str, String, Color); 4] = [
        ("WINS", profile.wins.to_string(), theme::GREEN),
        ("LOSSES", profile.losses.to_string(), Color::RGB(0xf2, 0xf2, 0xee)),
        ("WIN RATE", format!("{win_rate}%"), theme::TEXT),
        ("MATCHES", profile.matches_played.to_string(), theme::TEXT),
    ];
    let stat_h = 100.0;
    let gap = 12.0;
    let stat_w = (w - gap * 3.0) / 4.0;
    for (i, (label, value, color)) in stats.iter().enumerate() {
        let sx = x + i as f32 * (stat_w + gap);
        canvas.set_draw_color(Color::RGBA(8, 8, 11, 153));
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        canvas.fill_rect(Some(scale.rect(sx, top, stat_w, stat_h)))?;
        canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
        canvas.draw_rect(scale.rect(sx, top, stat_w, stat_h))?;
        let (lx, ly) = scale.point(sx + 18.0, top + 18.0);
        fonts.draw_tracked(canvas, FpFont::ChakraPetchMedium, scale.font_px(11.0), label, lx, ly, Color::RGB(0x52, 0x52, 0x5a), scale.len(2.0).round() as i32)?;
        let (vx, vy) = scale.point(sx + 18.0, top + 38.0);
        fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(30.0), value, vx, vy, *color)?;
    }

    let panel_y = top + stat_h + 18.0;
    let panel_h = body_h - stat_h - 18.0;
    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.fill_rect(Some(scale.rect(x, panel_y, w, panel_h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(x, panel_y, w, panel_h))?;

    let (rmx, rmy) = scale.point(x + 22.0, panel_y + 14.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "RECENT MATCHES", rmx, rmy, Color::RGB(0x8a, 0x8a, 0x92), scale.len(5.0).round() as i32)?;
    let count_text = format!("LAST {}", history.len().min(7));
    let (cw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(11.0), &count_text);
    let (cx, cy) = scale.point(x + w - 22.0 - (cw as f32 / scale.s), panel_y + 15.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(11.0), &count_text, cx, cy, Color::RGB(0x3a, 0x3a, 0x42))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(x, panel_y + 44.0, w, 1.0)))?;

    let header_y = panel_y + 44.0;
    let cols = [x + 22.0, x + 110.0, w - 90.0 + x - 200.0, x + w - 100.0];
    for (i, h) in ["RESULT", "OPPONENT", "SCORE", "DATE"].iter().enumerate() {
        let (hx, hy) = scale.point(cols[i], header_y + 12.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), h, hx, hy, Color::RGB(0x3a, 0x3a, 0x42))?;
    }
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 10));
    canvas.fill_rect(Some(scale.rect(x, header_y + 34.0, w, 1.0)))?;

    if history.is_empty() {
        let msg = "No matches recorded yet";
        let (mw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(14.0), msg);
        let (mx, my) = scale.point(x + (w - mw as f32 / scale.s) / 2.0, header_y + 60.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(14.0), msg, mx, my, theme::DIM)?;
        return Ok(());
    }

    let row_h = (panel_h - 44.0 - 34.0) / 7.0;
    for (i, m) in history.iter().take(7).enumerate() {
        let ry = header_y + 34.0 + i as f32 * row_h;
        let won = m.result == "won";
        let (badge_text, badge_color) = if won { ("WON", theme::GREEN) } else { ("LOST", Color::RGB(0xe2, 0x60, 0x3a)) };
        let (bx, by) = scale.point(cols[0], ry + row_h / 2.0 - 8.0);
        fonts.draw(canvas, FpFont::ChakraPetchBold, scale.font_px(13.0), badge_text, bx, by, badge_color)?;

        let (ox, oy) = scale.point(cols[1], ry + row_h / 2.0 - 10.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(18.0), &m.opponent_username, ox, oy, Color::RGB(0xed, 0xed, 0xe8))?;

        let score = format!("{}-{}", m.our_score, m.opponent_score);
        let (scx, scy) = scale.point(cols[2], ry + row_h / 2.0 - 9.0);
        fonts.draw(canvas, FpFont::ChakraPetchBold, scale.font_px(16.0), &score, scx, scy, Color::RGB(0xcf, 0xcf, 0xc9))?;

        let date = short_date(&m.played_at);
        let (dw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(13.0), &date);
        let (dx, dy) = scale.point(x + w - 22.0 - (dw as f32 / scale.s), ry + row_h / 2.0 - 8.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(13.0), &date, dx, dy, Color::RGB(0x52, 0x52, 0x5a))?;

        if i > 0 {
            canvas.set_draw_color(Color::RGBA(255, 255, 255, 8));
            canvas.fill_rect(Some(scale.rect(x, ry, w, 1.0)))?;
        }
    }

    Ok(())
}
