//! Lobby Room — native redesign of legacy's `MenuScreen::Lobby` (the
//! post-create/post-join king-of-the-hill room screen), matching the new
//! mockup's `isRoom` branch. Real data only: `view` is the exact same
//! `LobbyView` legacy's own screen renders, polled by `main.rs` the same
//! way (`matchmaking::fetch_lobby`) — see `mod.rs`'s `FpScreen::LobbyRoom`
//! doc comment for the dual-variant poll wiring. `thumb` is the same
//! periodic live-match screenshot legacy's `draw_lobby` shows, uploaded by
//! whichever two players are in `current`.
//!
//! Confirm always does what legacy's own `accept()` does for this screen:
//! confirm-ready if a ready check names you as challenger, otherwise toggle
//! between queued/spectating. Back always leaves the lobby (declining any
//! pending ready check for you in the process) — same as legacy's "ESC
//! Leave", not a separate action.

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use crate::matchmaking::LobbyView;
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const SIDE_PAD: f32 = 56.0;
const CONTENT_TOP: f32 = 142.0;
const PANEL_TOP: f32 = CONTENT_TOP + 96.0;
const PANEL_H: f32 = 570.0;
const MAIN_W: f32 = 1150.0;
const GAP: f32 = 24.0;
const SIDE_W: f32 = 1808.0 - MAIN_W - GAP;

#[allow(clippy::too_many_arguments)]
pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    view: Option<&LobbyView>,
    status: &str,
    thumb: Option<&(Vec<u8>, u32, u32)>,
    username: &str,
) -> Result<(), String> {
    chrome::draw_background_accents(canvas, scale)?;
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, CONTENT_TOP + 8.0, 30.0, 3.0)))?;
    let (ex, ey) = scale.point(SIDE_PAD + 44.0, CONTENT_TOP);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "ONLINE \u{b7} LOBBY ROOM", ex, ey, theme::ACCENT)?;

    let title = view.map(|v| v.name.clone()).unwrap_or_else(|| "LOBBY ROOM".into());
    let (tx, ty) = scale.point(SIDE_PAD, CONTENT_TOP + 26.0);
    let (tw, _) = fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(44.0), &title, tx, ty, theme::TEXT)?;

    if let Some(v) = view {
        let mut badge_x = SIDE_PAD + (tw as f32 / scale.s) + 24.0;
        let badge_y = CONTENT_TOP + 26.0 + 14.0;
        badge_x = draw_badge(canvas, fonts, scale, badge_x, badge_y, crate::matchmaking::lobby_format_label(v.format), theme::ACCENT)?;
        if v.private {
            draw_badge(canvas, fonts, scale, badge_x, badge_y, "PRIVATE", Color::RGB(0x9a, 0x9a, 0xa2))?;
        }
    }

    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, PANEL_TOP - 20.0, 1808.0, 1.0)))?;

    // Main panel.
    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, PANEL_TOP, MAIN_W, PANEL_H)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(SIDE_PAD, PANEL_TOP, MAIN_W, PANEL_H))?;

    match view {
        None => draw_loading(canvas, fonts, scale, status)?,
        Some(v) => draw_room_state(canvas, fonts, scale, v, thumb)?,
    }

    // Side panel: invite code (private lobbies) + member list.
    let side_x = SIDE_PAD + MAIN_W + GAP;
    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.fill_rect(Some(scale.rect(side_x, PANEL_TOP, SIDE_W, PANEL_H)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(side_x, PANEL_TOP, SIDE_W, PANEL_H))?;
    draw_side_panel(canvas, fonts, scale, side_x, view)?;

    let action_label = confirm_label(view);
    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::FooterPrompt { glyph: "X", label: action_label, color: theme::BTN_CROSS }, chrome::PROMPT_BACK],
        FooterRight::Text("KING OF THE HILL"),
    )?;
    Ok(())
}

/// What Confirm currently does, for the footer prompt label — mirrors
/// legacy's own footer text exactly (`draw_lobby`'s `footer` string).
fn confirm_label(view: Option<&LobbyView>) -> &'static str {
    match view {
        None => "Select",
        Some(v) => {
            if v.ready_check.as_ref().is_some_and(|rc| rc.you_are_challenger) {
                "Ready!"
            } else if v.your_queued || v.your_position.is_some() {
                "Spectate"
            } else {
                "Join queue"
            }
        }
    }
}

fn draw_badge(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    y: f32,
    text: &str,
    color: Color,
) -> Result<f32, String> {
    let (tw, th) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(12.0), text);
    let pad_x = 10.0;
    let pad_y = 5.0;
    let w = (tw as f32 / scale.s) + pad_x * 2.0;
    let h = (th as f32 / scale.s) + pad_y * 2.0;
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(color.r, color.g, color.b, 36));
    canvas.fill_rect(Some(scale.rect(x, y, w, h)))?;
    canvas.set_draw_color(Color::RGBA(color.r, color.g, color.b, 130));
    canvas.draw_rect(scale.rect(x, y, w, h))?;
    let (lx, ly) = scale.point(x + pad_x, y + pad_y);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(12.0), text, lx, ly, color)?;
    Ok(x + w + 10.0)
}

fn draw_loading(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, status: &str) -> Result<(), String> {
    let cx = SIDE_PAD + MAIN_W / 2.0;
    let cy = PANEL_TOP + PANEL_H / 2.0;
    geometry::stroke_circle(canvas, scale, cx, cy - 40.0, 34.0, 3.0, Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 140));
    let title = "CONNECTING TO ROOM";
    let (tw, _) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(26.0), title);
    let (tx, ty) = scale.point(cx - (tw as f32 / scale.s) / 2.0, cy + 12.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(26.0), title, tx, ty, theme::TEXT)?;
    let sub = if status.is_empty() { "Syncing room state from server\u{2026}" } else { status };
    let (sw, _) = fonts.text_size(FpFont::SairaSemiBold, scale.font_px(15.0), sub);
    let (sx, sy) = scale.point(cx - (sw as f32 / scale.s) / 2.0, cy + 46.0);
    fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), sub, sx, sy, Color::RGB(0x8a, 0x8a, 0x92))?;
    Ok(())
}

fn draw_room_state(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    v: &LobbyView,
    thumb: Option<&(Vec<u8>, u32, u32)>,
) -> Result<(), String> {
    if let Some(rc) = &v.ready_check {
        return draw_ready_check(canvas, fonts, scale, rc);
    }
    if let Some(cur) = &v.current {
        return draw_now_playing(canvas, fonts, scale, v, cur, thumb);
    }
    if v.queue.is_empty() && !v.your_queued && v.your_position.is_none() {
        return draw_empty(canvas, fonts, scale);
    }
    draw_queued(canvas, fonts, scale, v)
}

fn draw_ready_check(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    rc: &crate::matchmaking::LobbyReadyCheck,
) -> Result<(), String> {
    let cx = SIDE_PAD + MAIN_W / 2.0;
    let cy = PANEL_TOP + PANEL_H / 2.0;
    geometry::stroke_circle(canvas, scale, cx, cy - 70.0, 48.0, 3.0, theme::ACCENT);
    let secs = rc.seconds_left.max(0).to_string();
    let (nw, nh) = fonts.text_size(FpFont::SairaCondensedBlack, scale.font_px(38.0), &secs);
    let (nx, ny) = scale.point(cx - (nw as f32 / scale.s) / 2.0, cy - 70.0 - (nh as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(38.0), &secs, nx, ny, theme::TEXT)?;

    let heading = if rc.you_are_challenger { "YOU ARE UP NEXT" } else { "NEXT MATCH PAIRING UP" };
    let (hw, _) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(28.0), heading);
    let (hx, hy) = scale.point(cx - (hw as f32 / scale.s) / 2.0, cy - 4.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(28.0), heading, hx, hy, theme::ACCENT)?;

    let line = format!("{}  VS  {}", rc.challenger_username.to_uppercase(), rc.champion_username.to_uppercase());
    let (lw, _) = fonts.text_size(FpFont::SairaSemiBold, scale.font_px(18.0), &line);
    let (lx, ly) = scale.point(cx - (lw as f32 / scale.s) / 2.0, cy + 36.0);
    fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(18.0), &line, lx, ly, Color::RGB(0xcf, 0xcf, 0xc9))?;
    Ok(())
}

fn draw_now_playing(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    v: &LobbyView,
    cur: &crate::matchmaking::LobbyCurrent,
    thumb: Option<&(Vec<u8>, u32, u32)>,
) -> Result<(), String> {
    let pad = 32.0;
    let x = SIDE_PAD + pad;
    let mut y = PANEL_TOP + pad;

    geometry::fill_circle(canvas, scale, x + 5.0, y + 5.0, 5.0, theme::ACCENT);
    let heading = if v.your_queued || v.your_position.is_some() { "MATCH IN PROGRESS \u{b7} YOU ARE QUEUED" } else { "MATCH IN PROGRESS \u{b7} SPECTATING" };
    let (hx, hy) = scale.point(x + 18.0, y - 6.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), heading, hx, hy, theme::ACCENT, scale.len(3.0).round() as i32)?;
    y += 40.0;

    let thumb_w = MAIN_W - pad * 2.0;
    let thumb_h = 340.0;
    canvas.set_draw_color(Color::RGB(0x14, 0x14, 0x1a));
    canvas.fill_rect(Some(scale.rect(x, y, thumb_w, thumb_h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 26));
    canvas.draw_rect(scale.rect(x, y, thumb_w, thumb_h))?;

    if let Some((rgba, tw, th)) = thumb {
        let tc = canvas.texture_creator();
        let made = tc.create_texture_static(sdl2::pixels::PixelFormatEnum::RGBA32, *tw, *th);
        if let Ok(mut tex) = made {
            if tex.update(None, rgba, *tw as usize * 4).is_ok() {
                let (px, py) = scale.point(x, y);
                let (pw, ph) = scale.point(thumb_w, thumb_h);
                canvas.copy(&tex, None, sdl2::rect::Rect::new(px, py, pw as u32, ph as u32))?;
            }
        }
    } else {
        let wait = "live feed loading\u{2026}";
        let (ww, _) = fonts.text_size(FpFont::SairaSemiBold, scale.font_px(15.0), wait);
        let (wx, wy) = scale.point(x + (thumb_w - ww as f32 / scale.s) / 2.0, y + thumb_h / 2.0 - 8.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), wait, wx, wy, theme::DIM)?;
    }

    let names = format!("{}  VS  {}", cur.host_username.to_uppercase(), cur.join_username.to_uppercase());
    let (nw, _) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(22.0), &names);
    let (nx, ny) = scale.point(x + (thumb_w - nw as f32 / scale.s) / 2.0, y + thumb_h - 40.0);
    canvas.set_draw_color(Color::RGBA(0, 0, 0, 140));
    canvas.fill_rect(Some(scale.rect(x, y + thumb_h - 46.0, thumb_w, 46.0)))?;
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(22.0), &names, nx, ny, Color::RGB(255, 255, 255))?;
    Ok(())
}

fn draw_empty(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale) -> Result<(), String> {
    let cx = SIDE_PAD + MAIN_W / 2.0;
    let cy = PANEL_TOP + PANEL_H / 2.0;
    geometry::stroke_circle(canvas, scale, cx, cy - 70.0, 40.0, 1.5, Color::RGBA(255, 255, 255, 30));
    geometry::fill_circle(canvas, scale, cx, cy - 70.0, 20.0, Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 200));
    let (vw, vh) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(16.0), "VS");
    let (vx, vy) = scale.point(cx - (vw as f32 / scale.s) / 2.0, cy - 70.0 - (vh as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(16.0), "VS", vx, vy, Color::RGB(255, 255, 255))?;

    let title = "WAITING ON PLAYERS";
    let (tw, _) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(28.0), title);
    let (tx, ty) = scale.point(cx - (tw as f32 / scale.s) / 2.0, cy - 6.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(28.0), title, tx, ty, theme::TEXT)?;

    let desc = "Queue up to challenge the room \u{b7} winner stays, loser re-queues";
    let (dw, _) = fonts.text_size(FpFont::SairaSemiBold, scale.font_px(15.0), desc);
    let (dx, dy) = scale.point(cx - (dw as f32 / scale.s) / 2.0, cy + 30.0);
    fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), desc, dx, dy, Color::RGB(0x8a, 0x8a, 0x92))?;
    Ok(())
}

fn draw_queued(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, v: &LobbyView) -> Result<(), String> {
    let cx = SIDE_PAD + MAIN_W / 2.0;
    let cy = PANEL_TOP + PANEL_H / 2.0;

    let heading = "YOUR QUEUE POSITION";
    let (hw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(14.0), heading);
    let (hx, hy) = scale.point(cx - (hw as f32 / scale.s) / 2.0, cy - 110.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(14.0), heading, hx, hy, theme::MUTE, scale.len(3.0).round() as i32)?;

    let pos_text = v.your_position.map(|p| format!("#{}", p + 1)).unwrap_or_else(|| "\u{2014}".into());
    let (pw, ph) = fonts.text_size(FpFont::SairaCondensedBlack, scale.font_px(72.0), &pos_text);
    let (px, py) = scale.point(cx - (pw as f32 / scale.s) / 2.0, cy - 90.0 + (110.0 - ph as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(72.0), &pos_text, px, py, theme::ACCENT)?;

    let next = v.queue.first().map(|n| format!("Next up: {n}")).unwrap_or_else(|| "You're next up".into());
    let (nw, _) = fonts.text_size(FpFont::SairaSemiBold, scale.font_px(16.0), &next);
    let (nx, ny) = scale.point(cx - (nw as f32 / scale.s) / 2.0, cy + 40.0);
    fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(16.0), &next, nx, ny, Color::RGB(0x8a, 0x8a, 0x92))?;
    Ok(())
}

fn draw_side_panel(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    view: Option<&LobbyView>,
) -> Result<(), String> {
    let pad = 24.0;
    let mut y = PANEL_TOP + pad;

    let Some(v) = view else {
        return Ok(());
    };

    if v.private {
        let (lx, ly) = scale.point(x + pad, y);
        fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(12.0), "INVITE CODE", lx, ly, theme::MUTE, scale.len(3.0).round() as i32)?;
        y += 26.0;
        canvas.set_draw_color(Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 20));
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        let code_h = 48.0;
        canvas.fill_rect(Some(scale.rect(x + pad, y, SIDE_W - pad * 2.0, code_h)))?;
        canvas.set_draw_color(Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 110));
        canvas.draw_rect(scale.rect(x + pad, y, SIDE_W - pad * 2.0, code_h))?;
        let (cx, cy) = scale.point(x + pad + 14.0, y + 11.0);
        fonts.draw_tracked(canvas, FpFont::SairaCondensedBold, scale.font_px(24.0), &v.id, cx, cy, theme::TEXT, scale.len(4.0).round() as i32)?;
        y += code_h + 24.0;
    }

    let header = format!("{} IN ROOM", v.members.len());
    let (hx, hy) = scale.point(x + pad, y);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(12.0), &header, hx, hy, theme::MUTE, scale.len(3.0).round() as i32)?;
    y += 22.0;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(x + pad, y, SIDE_W - pad * 2.0, 1.0)))?;
    y += 10.0;

    let row_h = 46.0;
    for m in &v.members {
        if y + row_h > PANEL_TOP + PANEL_H - pad {
            break;
        }
        let (dot_color, tag) = if m.in_match {
            (theme::ACCENT, "IN MATCH".to_string())
        } else if m.queued {
            let pos = v.queue.iter().position(|u| u.eq_ignore_ascii_case(&m.username));
            (theme::GREEN, pos.map(|p| format!("QUEUE #{}", p + 1)).unwrap_or_else(|| "QUEUED".into()))
        } else {
            (theme::INACTIVE, "IDLE".to_string())
        };
        geometry::fill_circle(canvas, scale, x + pad + 4.0, y + row_h / 2.0, 4.0, dot_color);
        let (nx, ny) = scale.point(x + pad + 18.0, y + row_h / 2.0 - 15.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), &m.username, nx, ny, Color::RGB(0xed, 0xed, 0xe8))?;
        let (tx, ty) = scale.point(x + pad + 18.0, y + row_h / 2.0 + 2.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(10.0), &tag, tx, ty, Color::RGB(0x62, 0x62, 0x6c))?;
        if let Some(rating) = m.rating {
            let rtext = rating.to_string();
            let (rw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(12.0), &rtext);
            let (rx, ry) = scale.point(x + SIDE_W - pad - (rw as f32 / scale.s), y + row_h / 2.0 - 7.0);
            fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(12.0), &rtext, rx, ry, Color::RGB(0x4a, 0x4a, 0x52))?;
        }
        y += row_h;
    }
    Ok(())
}
