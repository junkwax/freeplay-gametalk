//! Load Drones — native redesign of legacy's `MenuScreen::GhostSelect`
//! (reached from the Lab submenu's "LOAD DRONES" row, or directly from the
//! Play submenu's "DRONES" row). Real data only: local `.ncgh` recordings
//! scanned from disk, plus community recordings from freeplay-stats — both
//! populated/drained by `main.rs` exactly the way it already does for the
//! legacy screen (see `FpResult::OpenGhostSelect`/`LoadGhost`/
//! `DownloadGhost`), just targeting this screen's `entries`/`status`
//! fields instead. Picking an entry still hands off to the same
//! `ghost::Playback`/`start_logic_ghost_opponent` pipeline — only the
//! chooser UI is native, per explicit user direction (the actual Lab/Drone
//! in-game trainer overlay stays legacy).

use super::chrome::{self, FooterRight};
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use crate::menu::GhostEntry;
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const SIDE_PAD: f32 = 56.0;
const TOP: f32 = 38.0 + 104.0;
const ROW_H: f32 = 64.0;

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    cursor: usize,
    entries: &[GhostEntry],
    status: Option<&str>,
    username: &str,
) -> Result<(), String> {
    chrome::draw_background_accents(canvas, scale)?;
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, TOP + 8.0, 30.0, 3.0)))?;
    let (ex, ey) = scale.point(SIDE_PAD + 44.0, TOP);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "LAB \u{b7} GHOST OPPONENTS", ex, ey, theme::ACCENT)?;
    let (tx, ty) = scale.point(SIDE_PAD, TOP + 26.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(58.0), "LOAD DRONES", tx, ty, theme::TEXT)?;

    let body_top = TOP + 26.0 + 70.0;
    let body_h = 620.0;
    let w = theme::VW - SIDE_PAD * 2.0;

    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, body_top, w, body_h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(SIDE_PAD, body_top, w, body_h))?;

    if entries.is_empty() {
        let msg = status.unwrap_or("No drone recordings found");
        draw_status(canvas, fonts, scale, body_top, body_h, w, msg)?;
        if status.is_none() {
            let hint = "Record during netplay, or press F9 to record locally";
            let (hw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(15.0), hint);
            let (hx, hy) = scale.point(SIDE_PAD + (w - hw as f32 / scale.s) / 2.0, body_top + body_h / 2.0 + 20.0);
            fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(15.0), hint, hx, hy, theme::DIM)?;
        }
    } else {
        draw_list(canvas, fonts, scale, body_top, body_h, w, cursor, entries)?;
    }

    let right = match status {
        Some(s) if !entries.is_empty() => FooterRight::Text(s),
        _ => FooterRight::Text("DRONE = GHOST INPUT STREAM"),
    };
    chrome::draw_footer(canvas, fonts, scale, &[chrome::PROMPT_NAVIGATE, chrome::PROMPT_SELECT, chrome::PROMPT_BACK], right)?;
    Ok(())
}

fn draw_status(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, top: f32, h: f32, w: f32, text: &str) -> Result<(), String> {
    let (tw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(16.0), text);
    let (x, y) = scale.point(SIDE_PAD + (w - tw as f32 / scale.s) / 2.0, top + h / 2.0 - 8.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(16.0), text, x, y, theme::DIM)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_list(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    top: f32,
    h: f32,
    w: f32,
    cursor: usize,
    entries: &[GhostEntry],
) -> Result<(), String> {
    let max_visible = (h / ROW_H).floor().max(1.0) as usize;
    let start = if cursor >= max_visible { cursor - max_visible + 1 } else { 0 };
    let end = (start + max_visible).min(entries.len());

    for (row, i) in (start..end).enumerate() {
        let selected = i == cursor;
        let y = top + row as f32 * ROW_H;
        if selected {
            canvas.set_draw_color(Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 22));
            canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, w, ROW_H)))?;
            canvas.set_draw_color(theme::ACCENT);
            canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, 4.0, ROW_H)))?;
        } else if row > 0 {
            canvas.set_draw_color(Color::RGBA(255, 255, 255, 10));
            canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, w, 1.0)))?;
        }

        let (tag, tag_color, title, subtitle) = match &entries[i] {
            GhostEntry::Local { filename, .. } => (
                "LOCAL",
                theme::GREEN,
                "RECORDED MATCH".to_string(),
                crate::fp_ui::main_menu::short_date(&parse_ghost_time_iso(filename)),
            ),
            GhostEntry::Remote(meta) => {
                let who = if meta.username.trim().is_empty() { "COMMUNITY".to_string() } else { meta.username.to_uppercase() };
                (
                    "REMOTE",
                    theme::ACCENT,
                    "SHARED RECORDING".to_string(),
                    format!("{who} \u{b7} {} frames", meta.frame_count),
                )
            }
        };

        let tag_x = SIDE_PAD + 24.0;
        let (tagw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(11.0), tag);
        let tag_pad = 8.0;
        let tag_w = (tagw as f32 / scale.s) + tag_pad * 2.0;
        canvas.set_draw_color(Color::RGBA(tag_color.r, tag_color.g, tag_color.b, 30));
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        canvas.fill_rect(Some(scale.rect(tag_x, y + ROW_H / 2.0 - 12.0, tag_w, 24.0)))?;
        let (tgx, tgy) = scale.point(tag_x + tag_pad, y + ROW_H / 2.0 - 8.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), tag, tgx, tgy, tag_color)?;

        let title_x = tag_x + tag_w + 20.0;
        let (ttx, tty) = scale.point(title_x, y + ROW_H / 2.0 - 18.0);
        fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(20.0), &title, ttx, tty, theme::TEXT)?;
        let (stx, sty) = scale.point(title_x, y + ROW_H / 2.0 + 4.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(13.0), &subtitle, stx, sty, theme::MUTE)?;
    }

    Ok(())
}

/// `filename` embeds a Unix start-time the same way match replays do
/// (`{unix}_...`), but ghost filenames aren't guaranteed to — falls back to
/// the raw filename if it doesn't parse, same graceful-degradation as the
/// legacy screen's own `parse_ghost_time`.
fn parse_ghost_time_iso(filename: &str) -> String {
    let base = filename.strip_suffix(".ncgh").unwrap_or(filename);
    let Some(unix_str) = base.rsplit('_').next() else {
        return filename.to_string();
    };
    let Ok(unix) = unix_str.parse::<i64>() else {
        return filename.to_string();
    };
    // Reuse match_replay's own unix->ISO-ish conversion isn't public, and
    // main_menu::short_date only needs the "YYYY-MM-DD" prefix — build just
    // that much by hand (no chrono dependency, same reasoning as every
    // other date spot in this app).
    let days = unix.div_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}
