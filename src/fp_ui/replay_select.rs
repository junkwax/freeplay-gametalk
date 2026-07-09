//! Replays — native redesign of legacy's `MenuScreen::ReplaySelect` (reached
//! from the Play submenu's "REPLAYS" row). Real data only: local `.ncrp`
//! recordings scanned from disk, plus community replays from
//! freeplay-stats — both populated/drained by `main.rs` exactly the way it
//! already does for the legacy screen (see `FpResult::OpenReplaySelect`/
//! `LoadReplay`/`LoadRemoteReplay`), just targeting this screen's
//! `entries`/`status` fields instead. Picking an entry still hands off to
//! the same `prepare_replay_review`/`enter_replay_review` pipeline — the
//! actual replay *playback* viewer (scrub controls, bookmarks, clip
//! export) stays legacy; only this chooser list is native. Per-row actions
//! the legacy screen has via keyboard shortcuts (delete, add note, toggle
//! bookmark) aren't reproduced here — same "menu system only" scope as
//! the Lab/Drones screens.

use super::chrome::{self, FooterRight};
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use crate::menu::ReplayEntry;
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const SIDE_PAD: f32 = 56.0;
const TOP: f32 = 38.0 + 104.0;
const SKEW_DEG: f32 = -9.0;
const ROW_H: f32 = 64.0;

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    cursor: usize,
    entries: &[ReplayEntry],
    status: Option<&str>,
    username: &str,
) -> Result<(), String> {
    chrome::draw_background_accents(canvas, scale, SKEW_DEG)?;
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, TOP + 8.0, 30.0, 3.0)))?;
    let (ex, ey) = scale.point(SIDE_PAD + 44.0, TOP);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "PLAY \u{b7} MATCH HISTORY", ex, ey, theme::ACCENT)?;
    let (tx, ty) = scale.point(SIDE_PAD, TOP + 26.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(58.0), "REPLAYS", tx, ty, theme::TEXT)?;

    let body_top = TOP + 26.0 + 70.0;
    let body_h = 620.0;
    let w = theme::VW - SIDE_PAD * 2.0;

    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, body_top, w, body_h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(SIDE_PAD, body_top, w, body_h))?;

    if entries.is_empty() {
        let msg = status.unwrap_or("No replays found");
        draw_status(canvas, fonts, scale, body_top, body_h, w, msg)?;
        if status.is_none() {
            let hint = "Completed online matches are recorded and uploaded automatically";
            let (hw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(15.0), hint);
            let (hx, hy) = scale.point(SIDE_PAD + (w - hw as f32 / scale.s) / 2.0, body_top + body_h / 2.0 + 20.0);
            fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(15.0), hint, hx, hy, theme::DIM)?;
        }
    } else {
        draw_list(canvas, fonts, scale, body_top, body_h, w, cursor, entries)?;
    }

    let right = match status {
        Some(s) if !entries.is_empty() => FooterRight::Text(s),
        _ => FooterRight::Text("MATCH REPLAYS"),
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
    entries: &[ReplayEntry],
) -> Result<(), String> {
    let max_visible = (h / ROW_H).floor().max(1.0) as usize;
    let start = if cursor >= max_visible { cursor - max_visible + 1 } else { 0 };
    let end = (start + max_visible).min(entries.len());

    for (row, i) in (start..end).enumerate() {
        let entry = &entries[i];
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

        let is_remote = entry.remote_url.is_some();
        let (tag, tag_color) = if is_remote { ("ONLINE", theme::ACCENT) } else { ("LOCAL", theme::GREEN) };
        let tag_x = SIDE_PAD + 24.0;
        let (tagw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(11.0), tag);
        let tag_pad = 8.0;
        let tag_w = (tagw as f32 / scale.s) + tag_pad * 2.0;
        canvas.set_draw_color(Color::RGBA(tag_color.r, tag_color.g, tag_color.b, 30));
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        canvas.fill_rect(Some(scale.rect(tag_x, y + ROW_H / 2.0 - 12.0, tag_w, 24.0)))?;
        let (tgx, tgy) = scale.point(tag_x + tag_pad, y + ROW_H / 2.0 - 8.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), tag, tgx, tgy, tag_color)?;

        let title = match (entry.p1_score, entry.p2_score) {
            (Some(p1), Some(p2)) => format!("{} {p1}-{p2} {}", entry.p1_name.to_uppercase(), entry.p2_name.to_uppercase()),
            _ => format!("{} VS {}", entry.p1_name.to_uppercase(), entry.p2_name.to_uppercase()),
        };
        let mut subtitle_parts = vec![entry.duration.clone()];
        if !entry.recorded_at.is_empty() {
            subtitle_parts.push(super::main_menu::short_date(&entry.recorded_at));
        }
        if !entry.note.is_empty() {
            subtitle_parts.push(entry.note.clone());
        }
        let subtitle = subtitle_parts.join(" \u{b7} ");

        let title_x = tag_x + tag_w + 20.0;
        let (ttx, tty) = scale.point(title_x, y + ROW_H / 2.0 - 18.0);
        fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(20.0), &title, ttx, tty, theme::TEXT)?;
        let (stx, sty) = scale.point(title_x, y + ROW_H / 2.0 + 4.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(13.0), &subtitle, stx, sty, theme::MUTE)?;
    }

    Ok(())
}
