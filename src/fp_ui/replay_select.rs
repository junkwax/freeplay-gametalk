//! Replays — native redesign of legacy's `MenuScreen::ReplaySelect` (reached
//! from the Play submenu's "REPLAYS" row). Real data only: local `.ncrp`
//! recordings scanned from disk, plus community replays from
//! freeplay-stats — both populated/drained by `main.rs` exactly the way it
//! already does for the legacy screen (see `FpResult::OpenReplaySelect`/
//! `LoadReplay`/`LoadRemoteReplay`), just targeting this screen's
//! `entries`/`status` fields instead. Picking an entry still hands off to
//! the same `prepare_replay_review`/`enter_replay_review` pipeline — the
//! actual replay *playback* viewer (scrub controls, bookmarks, clip export)
//! stays legacy; only this chooser list is native.
//!
//! Layout matches the current mockup's `isReplays` branch: a two-pane list
//! + detail sidebar (WATCH / EDIT NOTE / DELETE FILE), not the single flat
//! list an earlier pass here built. Delete and Edit Note are real, not
//! decorative — legacy's own `handle_replay_select_shortcut` (Delete/X to
//! delete, N/R1 to edit the note) already existed for the legacy screen and
//! is now widened in `main.rs` to also recognize this one, rather than
//! re-implemented here.

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
const PANEL_H: f32 = 548.0;
const SIDEBAR_W: f32 = 360.0;
const GAP: f32 = 20.0;

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    cursor: usize,
    entries: &[ReplayEntry],
    status: Option<&str>,
    username: &str,
) -> Result<(), String> {
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, TOP + 8.0, 30.0, 3.0)))?;
    let (ex, ey) = scale.point(SIDE_PAD + 44.0, TOP);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "MATCH RECORDINGS", ex, ey, theme::ACCENT)?;
    let (tx, ty) = scale.point(SIDE_PAD, TOP + 26.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(58.0), "REPLAYS", tx, ty, theme::TEXT)?;

    let local_count = entries.iter().filter(|e| e.remote_url.is_none()).count();
    let count_line = format!("{} FILES \u{b7} {} LOCAL", entries.len(), local_count);
    let (cw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(13.0), &count_line);
    let (cx, cy) = scale.point(theme::VW - SIDE_PAD - (cw as f32 / scale.s), TOP + 40.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(13.0), &count_line, cx, cy, Color::RGB(0x52, 0x52, 0x5a))?;

    let body_top = TOP + 26.0 + 70.0;
    let list_w = 1808.0 - SIDEBAR_W - GAP;

    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, body_top, list_w, PANEL_H)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(SIDE_PAD, body_top, list_w, PANEL_H))?;

    if entries.is_empty() {
        let msg = status.unwrap_or("No replays found");
        draw_status(canvas, fonts, scale, body_top, PANEL_H, list_w, msg)?;
        if status.is_none() {
            let hint = "Completed online matches are recorded and uploaded automatically";
            let (hw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(14.0), hint);
            let (hx, hy) = scale.point(SIDE_PAD + (list_w - hw as f32 / scale.s) / 2.0, body_top + PANEL_H / 2.0 + 20.0);
            fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(14.0), hint, hx, hy, theme::DIM)?;
        }
    } else {
        draw_list(canvas, fonts, scale, body_top, list_w, cursor, entries)?;
    }

    let side_x = SIDE_PAD + list_w + GAP;
    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.fill_rect(Some(scale.rect(side_x, body_top, SIDEBAR_W, PANEL_H)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(side_x, body_top, SIDEBAR_W, PANEL_H))?;
    draw_sidebar(canvas, fonts, scale, side_x, body_top, entries.get(cursor), username)?;

    let right = match status {
        Some(s) if !entries.is_empty() => FooterRight::Text(s),
        _ => FooterRight::Text("MATCH REPLAYS"),
    };
    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[
            chrome::PROMPT_NAVIGATE,
            chrome::PROMPT_SELECT,
            chrome::FooterPrompt { glyph: "R1", label: "Edit Note", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::FooterPrompt { glyph: "DEL", label: "Delete", color: theme::BTN_CIRCLE },
            chrome::PROMPT_BACK,
        ],
        right,
    )?;
    Ok(())
}

fn draw_status(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, top: f32, h: f32, w: f32, text: &str) -> Result<(), String> {
    let (tw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(16.0), text);
    let (x, y) = scale.point(SIDE_PAD + (w - tw as f32 / scale.s) / 2.0, top + h / 2.0 - 8.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(16.0), text, x, y, theme::DIM)?;
    Ok(())
}

fn draw_list(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    top: f32,
    w: f32,
    cursor: usize,
    entries: &[ReplayEntry],
) -> Result<(), String> {
    let header_h = 34.0;
    let cols = [SIDE_PAD + 20.0, SIDE_PAD + 96.0, SIDE_PAD + w - 234.0, SIDE_PAD + w - 162.0, SIDE_PAD + w - 20.0];
    for (label, x) in [("SOURCE", cols[0]), ("MATCH", cols[1]), ("SCORE", cols[2]), ("DURATION", cols[3])] {
        let (hx, hy) = scale.point(x, top + 12.0);
        fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), label, hx, hy, Color::RGB(0x3a, 0x3a, 0x42), scale.len(2.0).round() as i32)?;
    }
    let (dhw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "DATE");
    let (dhx, dhy) = scale.point(cols[4] - dhw as f32 / scale.s, top + 12.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "DATE", dhx, dhy, Color::RGB(0x3a, 0x3a, 0x42), scale.len(2.0).round() as i32)?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, top + header_h, w, 1.0)))?;

    let row_h = 66.0;
    let max_visible = ((PANEL_H - header_h) / row_h).floor().max(1.0) as usize;
    let start = if cursor >= max_visible { cursor - max_visible + 1 } else { 0 };
    let end = (start + max_visible).min(entries.len());

    for (row, i) in (start..end).enumerate() {
        let entry = &entries[i];
        let selected = i == cursor;
        let y = top + header_h + row as f32 * row_h;
        if selected {
            canvas.set_draw_color(Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 22));
            canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, w, row_h)))?;
            canvas.set_draw_color(theme::ACCENT);
            canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, 4.0, row_h)))?;
        } else if row > 0 {
            canvas.set_draw_color(Color::RGBA(255, 255, 255, 10));
            canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, w, 1.0)))?;
        }

        let is_remote = entry.remote_url.is_some();
        let (tag, tag_color) = if is_remote { ("PUBLIC", theme::ACCENT) } else { ("LOCAL", Color::RGB(0x8a, 0x8a, 0x92)) };
        let (tagw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(11.0), tag);
        let tag_pad = 6.0;
        let tag_w = (tagw as f32 / scale.s) + tag_pad * 2.0;
        canvas.set_draw_color(Color::RGBA(tag_color.r, tag_color.g, tag_color.b, if is_remote { 90 } else { 60 }));
        canvas.draw_rect(scale.rect(cols[0], y + row_h / 2.0 - 11.0, tag_w, 22.0))?;
        let (tgx, tgy) = scale.point(cols[0] + tag_pad, y + row_h / 2.0 - 8.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(11.0), tag, tgx, tgy, tag_color)?;

        let title = format!("{} vs {}", entry.p1_name, entry.p2_name);
        let (mtx, mty) = scale.point(cols[1], y + row_h / 2.0 - 18.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(18.0), &title, mtx, mty, Color::RGB(0xED, 0xED, 0xE8))?;
        if !entry.note.is_empty() {
            let note = format!("\u{201c}{}\u{201d}", entry.note);
            let (ntx, nty) = scale.point(cols[1], y + row_h / 2.0 + 2.0);
            fonts.draw(canvas, FpFont::SairaMedium, scale.font_px(13.0), &note, ntx, nty, Color::RGB(0x52, 0x52, 0x5a))?;
        }

        let score = match (entry.p1_score, entry.p2_score) {
            (Some(p1), Some(p2)) => format!("{p1}-{p2}"),
            _ => "\u{2014}".into(),
        };
        let (scx, scy) = scale.point(cols[2], y + row_h / 2.0 - 8.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(15.0), &score, scx, scy, Color::RGB(0xcf, 0xcf, 0xc9))?;

        let (dx, dy) = scale.point(cols[3], y + row_h / 2.0 - 7.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(14.0), &entry.duration, dx, dy, Color::RGB(0x8a, 0x8a, 0x92))?;

        let date = if entry.recorded_at.is_empty() { String::new() } else { super::main_menu::short_date(&entry.recorded_at) };
        let (dw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(13.0), &date);
        let (datex, datey) = scale.point(cols[4] - dw as f32 / scale.s, y + row_h / 2.0 - 7.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(13.0), &date, datex, datey, Color::RGB(0x52, 0x52, 0x5a))?;
    }

    Ok(())
}

fn draw_sidebar(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    top: f32,
    selected: Option<&ReplayEntry>,
    username: &str,
) -> Result<(), String> {
    let pad = 22.0;
    let (hx, hy) = scale.point(x + pad, top + 20.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "SELECTED", hx, hy, Color::RGB(0x3a, 0x3a, 0x42), scale.len(4.0).round() as i32)?;

    let Some(entry) = selected else {
        let (mx, my) = scale.point(x + pad, top + 60.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), "No replay selected", mx, my, theme::DIM)?;
        return Ok(());
    };

    let title = format!("{} vs {}", entry.p1_name, entry.p2_name);
    let (tx, ty) = scale.point(x + pad, top + 42.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(22.0), &title, tx, ty, theme::TEXT)?;

    let you_won = !entry.winner.is_empty() && entry.winner.eq_ignore_ascii_case(username);
    let score = match (entry.p1_score, entry.p2_score) {
        (Some(p1), Some(p2)) => format!("{p1}-{p2}"),
        _ => String::new(),
    };
    if !entry.winner.is_empty() {
        let badge = format!("{} WINS", entry.winner.to_uppercase());
        let badge_color = if you_won { theme::GREEN } else { theme::ACCENT };
        let (bw, bh) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(12.0), &badge);
        let pad_x = 10.0;
        let pad_y = 4.0;
        let bx = x + pad;
        let by = top + 72.0;
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        canvas.set_draw_color(Color::RGBA(badge_color.r, badge_color.g, badge_color.b, 20));
        canvas.fill_rect(Some(scale.rect(bx, by, (bw as f32 / scale.s) + pad_x * 2.0, (bh as f32 / scale.s) + pad_y * 2.0)))?;
        canvas.set_draw_color(Color::RGBA(badge_color.r, badge_color.g, badge_color.b, 110));
        canvas.draw_rect(scale.rect(bx, by, (bw as f32 / scale.s) + pad_x * 2.0, (bh as f32 / scale.s) + pad_y * 2.0))?;
        let (btx, bty) = scale.point(bx + pad_x, by + pad_y);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(12.0), &badge, btx, bty, badge_color)?;

        if !score.is_empty() {
            let score_x = bx + (bw as f32 / scale.s) + pad_x * 2.0 + 12.0;
            let (scx, scy) = scale.point(score_x, by + pad_y);
            fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(13.0), &score, scx, scy, Color::RGB(0x52, 0x52, 0x5a))?;
        }
    }

    let rows: [(&str, String); 3] = [
        ("DURATION", entry.duration.clone()),
        ("RECORDED", if entry.recorded_at.is_empty() { "\u{2014}".into() } else { super::main_menu::short_date(&entry.recorded_at) }),
        ("BOOKMARKS", entry.bookmark_count.to_string()),
    ];
    let mut ry = top + 128.0;
    for (label, value) in rows {
        let (lx, ly) = scale.point(x + pad, ry);
        fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), label, lx, ly, Color::RGB(0x3a, 0x3a, 0x42), scale.len(2.0).round() as i32)?;
        let (vw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(14.0), &value);
        let (vx, vy) = scale.point(x + SIDEBAR_W - pad - (vw as f32 / scale.s), ry - 2.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(14.0), &value, vx, vy, Color::RGB(0xcf, 0xcf, 0xc9))?;
        ry += 30.0;
    }

    ry += 6.0;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 15));
    canvas.fill_rect(Some(scale.rect(x + pad, ry, SIDEBAR_W - pad * 2.0, 1.0)))?;
    ry += 14.0;
    let (nlx, nly) = scale.point(x + pad, ry);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "NOTE", nlx, nly, Color::RGB(0x3a, 0x3a, 0x42), scale.len(2.0).round() as i32)?;
    let note = if entry.note.is_empty() { "\u{2014}" } else { &entry.note };
    let (nx, ny) = scale.point(x + pad, ry + 22.0);
    fonts.draw(canvas, FpFont::SairaMedium, scale.font_px(14.0), note, nx, ny, Color::RGB(0x7a, 0x7a, 0x82))?;

    let btn_y = top + PANEL_H - 92.0;
    let btn_h = 42.0;
    let gap = 8.0;
    let watch_w = (SIDEBAR_W - pad * 2.0 - gap) / 2.0;
    draw_action_button(canvas, fonts, scale, x + pad, btn_y, watch_w, btn_h, "\u{25b6} WATCH", true)?;
    draw_action_button(canvas, fonts, scale, x + pad + watch_w + gap, btn_y, watch_w, btn_h, "EDIT NOTE", false)?;

    let is_local = entry.remote_url.is_none();
    let del_y = btn_y + btn_h + 8.0;
    let del_h = 34.0;
    let del_color = if is_local { Color::RGB(0x7a, 0x7a, 0x82) } else { Color::RGB(0x2e, 0x2e, 0x36) };
    canvas.set_draw_color(if is_local { Color::RGBA(255, 255, 255, 26) } else { Color::RGBA(255, 255, 255, 8) });
    canvas.draw_rect(scale.rect(x + pad, del_y, SIDEBAR_W - pad * 2.0, del_h))?;
    let label = "DELETE FILE";
    let (dlw, dlh) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(13.0), label);
    let (dlx, dly) = scale.point(x + pad + (SIDEBAR_W - pad * 2.0) / 2.0 - (dlw as f32 / scale.s) / 2.0, del_y + del_h / 2.0 - (dlh as f32 / scale.s) / 2.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), label, dlx, dly, del_color, scale.len(2.0).round() as i32)?;
    Ok(())
}

fn draw_action_button(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    is_primary: bool,
) -> Result<(), String> {
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    if is_primary {
        canvas.set_draw_color(theme::ACCENT);
        canvas.fill_rect(Some(scale.rect(x, y, w, h)))?;
    } else {
        canvas.set_draw_color(Color::RGBA(255, 255, 255, 31));
        canvas.draw_rect(scale.rect(x, y, w, h))?;
    }
    let color = if is_primary { Color::RGB(255, 255, 255) } else { Color::RGB(0x9a, 0x9a, 0xa2) };
    let (tw, th) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(16.0), label);
    let (tx, ty) = scale.point(x + w / 2.0 - (tw as f32 / scale.s) / 2.0, y + h / 2.0 - (th as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(16.0), label, tx, ty, color)?;
    Ok(())
}
