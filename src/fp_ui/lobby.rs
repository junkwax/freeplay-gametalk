//! Lobby — matches `screenshots/08-lobby-quickmatch.png` (the only one of
//! the three lobby screenshots actually showing tab content; 09/10 are
//! duplicates of 08 in the design package, so Host/Join and Server Browser
//! are built from `FREEPLAY Arcade.dc.html`'s raw markup for those `sc-if`
//! branches instead of a screenshot).
//!
//! Scope, per the fork discussed before building this: Host-Join and
//! Server Browser bind to the real Lobbies data model (`menu::LobbyPreview`,
//! `matchmaking::create_lobby`/`join_lobby`), not the mockup's room-code/
//! ping-sorted shape, which has no real analog — see `mod.rs`'s `FpResult`
//! doc comments for exactly which legacy actions each tab delegates to.
//!
//! Quick Match fidelity gap (flagged, not silently approximated): the
//! mockup auto-starts searching on tab view and shows live elapsed/queue/
//! wait-estimate stats. Actually queueing hands off to the legacy
//! `MenuScreen::Matchmaking` screen instead of staying in this one — see
//! `mod.rs`'s `FpResult::StartFindMatch` doc comment for why re-implementing
//! the match-found/session-start pipeline here was assessed as out of scope
//! for this step. This screen only shows the pre-search state.

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use crate::menu::LobbyPreview;
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

pub const TABS: [&str; 5] = ["QUICK MATCH", "HOST / JOIN", "SERVER BROWSER", "CHAT", "WATCH"];
const SIDE_PAD: f32 = 56.0;
const CONTENT_TOP: f32 = 142.0;
const PANEL_TOP: f32 = CONTENT_TOP + 122.0;
const PANEL_H: f32 = 512.0;
const PANEL_W: f32 = 1808.0;

#[allow(clippy::too_many_arguments)]
pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    tab: usize,
    host_join_focus: usize,
    cursor: usize,
    lobbies: &[LobbyPreview],
    status: &str,
    chat: &[crate::matchmaking::LobbyChatMessage],
    presence: &[crate::matchmaking::LobbyUser],
    live_matches: &[crate::matchmaking::LiveMatch],
    username: &str,
) -> Result<(), String> {
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    let (ex, ey) = scale.point(SIDE_PAD + 44.0, CONTENT_TOP);
    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, CONTENT_TOP + 8.0, 30.0, 3.0)))?;
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "ONLINE \u{b7} NETPLAY", ex, ey, theme::ACCENT)?;
    let (tx, ty) = scale.point(SIDE_PAD, CONTENT_TOP + 26.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(58.0), "LOBBY", tx, ty, theme::TEXT)?;

    let tabs_y = CONTENT_TOP + 96.0;
    let mut x = SIDE_PAD;
    for (i, label) in TABS.iter().enumerate() {
        let w = 20.0 * 2.0 + label.len() as f32 * 11.0;
        draw_tab(canvas, fonts, scale, x, tabs_y, w, label, i == tab)?;
        x += w + 5.0;
    }
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 26));
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, tabs_y + 38.0, PANEL_W, 1.0)))?;

    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, PANEL_TOP, PANEL_W, PANEL_H)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(SIDE_PAD, PANEL_TOP, PANEL_W, PANEL_H))?;

    match tab {
        0 => draw_quick_match(canvas, fonts, scale)?,
        1 => draw_host_join(canvas, fonts, scale, host_join_focus)?,
        2 => draw_server_browser(canvas, fonts, scale, lobbies, cursor, status)?,
        3 => draw_chat(canvas, fonts, scale, chat, presence, cursor)?,
        _ => draw_watch(canvas, fonts, scale, live_matches, cursor)?,
    }

    let prompts: &[chrome::FooterPrompt] = match tab {
        0 => &[chrome::PROMPT_SELECT, chrome::PROMPT_BACK],
        1 => &[
            chrome::FooterPrompt { glyph: "\u{2194}", label: "Choose", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::PROMPT_SELECT,
            chrome::PROMPT_BACK,
        ],
        2 => &[
            chrome::FooterPrompt { glyph: "\u{2195}", label: "Row", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::PROMPT_SELECT,
            chrome::PROMPT_BACK,
        ],
        3 => &[
            chrome::FooterPrompt { glyph: "\u{2195}", label: "Phrase", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::PROMPT_SELECT,
            chrome::PROMPT_BACK,
        ],
        _ => &[
            chrome::FooterPrompt { glyph: "\u{2195}", label: "Match", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::FooterPrompt { glyph: "X", label: "Spectate", color: theme::BTN_CROSS },
            chrome::PROMPT_BACK,
        ],
    };
    chrome::draw_footer(canvas, fonts, scale, prompts, FooterRight::Text("NETPLAY \u{b7} ROLLBACK \u{b7} FREE PLAY"))?;
    Ok(())
}

fn draw_tab(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    y: f32,
    w: f32,
    label: &str,
    active: bool,
) -> Result<(), String> {
    let h = 44.0;
    let color = if active { theme::ACCENT } else { Color::RGBA(255, 255, 255, 8) };
    geometry::fill_skewed_rect(canvas, scale, x, y, w, h, -11.0, color);
    let text_color = if active { Color::RGB(255, 255, 255) } else { Color::RGB(0x7a, 0x7a, 0x82) };
    let (tw, th) = fonts.text_size(FpFont::SairaCondensedBlack, scale.font_px(18.0), label);
    let (lx, ly) = scale.point(x + w / 2.0 - (tw as f32 / scale.s) / 2.0, y + h / 2.0 - (th as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(18.0), label, lx, ly, text_color)?;
    Ok(())
}

fn draw_quick_match(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale) -> Result<(), String> {
    let cx = SIDE_PAD + 60.0 + 170.0;
    let cy = PANEL_TOP + PANEL_H / 2.0;
    for r in [150.0, 95.0] {
        geometry::stroke_circle(canvas, scale, cx, cy, r, 1.0, Color::RGBA(255, 255, 255, 15));
    }
    geometry::stroke_circle(canvas, scale, cx, cy, 150.0, 1.5, Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 90));
    geometry::fill_circle(canvas, scale, cx, cy, 37.0, Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 230));
    let (vw, vh) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(26.0), "VS");
    let (vx, vy) = scale.point(cx - (vw as f32 / scale.s) / 2.0, cy - (vh as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(26.0), "VS", vx, vy, Color::RGB(255, 255, 255))?;

    let text_x = SIDE_PAD + 60.0 + 340.0 + 64.0;
    let (tx, ty) = scale.point(text_x, cy - 90.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(38.0), "READY TO SEARCH", tx, ty, theme::TEXT)?;
    let (sx, sy) = scale.point(text_x, cy - 46.0);
    fonts.draw(
        canvas,
        FpFont::SairaSemiBold,
        scale.font_px(16.0),
        "Matching by connection quality \u{b7} Best of 1 \u{b7} No time limit",
        sx,
        sy,
        Color::RGB(0x8a, 0x8a, 0x92),
    )?;
    let (px, py) = scale.point(text_x, cy);
    fonts.draw(
        canvas,
        FpFont::SairaSemiBold,
        scale.font_px(15.0),
        "Press Cross to search for an opponent",
        px,
        py,
        theme::ACCENT,
    )?;
    Ok(())
}

fn draw_host_join(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    focus: usize,
) -> Result<(), String> {
    let col_w = PANEL_W / 2.0;
    draw_host_join_col(canvas, fonts, scale, SIDE_PAD, col_w, "HOST GAME", "Create a private lobby with an invite code.", "Press Cross to create a private lobby", focus == 0)?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(SIDE_PAD + col_w, PANEL_TOP + 20.0, 1.0, PANEL_H - 40.0)))?;
    draw_host_join_col(canvas, fonts, scale, SIDE_PAD + col_w, col_w, "JOIN GAME", "Enter a host's invite code using your controller.", "Press Cross to enter an invite code", focus == 1)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_host_join_col(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    w: f32,
    title: &str,
    desc: &str,
    action: &str,
    focused: bool,
) -> Result<(), String> {
    let pad = 44.0;
    let (tx, ty) = scale.point(x + pad, PANEL_TOP + 38.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(30.0), title, tx, ty, theme::TEXT)?;
    let (dx, dy) = scale.point(x + pad, PANEL_TOP + 74.0);
    fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(14.0), desc, dx, dy, Color::RGB(0x7a, 0x7a, 0x82))?;

    let btn_y = PANEL_TOP + 130.0;
    let btn_w = w - pad * 2.0;
    let btn_h = 48.0;
    let (border, bg, color) = if focused {
        (theme::ACCENT, Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 36), Color::RGB(255, 255, 255))
    } else {
        (Color::RGBA(255, 255, 255, 31), Color::RGBA(0, 0, 0, 0), Color::RGB(0x8a, 0x8a, 0x92))
    };
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    if bg.a > 0 {
        canvas.set_draw_color(bg);
        canvas.fill_rect(Some(scale.rect(x + pad, btn_y, btn_w, btn_h)))?;
    }
    canvas.set_draw_color(border);
    canvas.draw_rect(scale.rect(x + pad, btn_y, btn_w, btn_h))?;
    let (aw, ah) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(16.0), action);
    let (ax, ay) = scale.point(x + pad + btn_w / 2.0 - (aw as f32 / scale.s) / 2.0, btn_y + btn_h / 2.0 - (ah as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(16.0), action, ax, ay, color)?;
    Ok(())
}

fn draw_server_browser(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    lobbies: &[LobbyPreview],
    cursor: usize,
    status: &str,
) -> Result<(), String> {
    let header_y = PANEL_TOP + 16.0;
    let cols = [("STATUS", 0.0), ("HOST", 140.0), ("FORMAT", 900.0), ("PLAYERS", 1200.0)];
    for (label, off) in cols {
        let (hx, hy) = scale.point(SIDE_PAD + 24.0 + off, header_y);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), label, hx, hy, theme::MUTE)?;
    }
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, header_y + 26.0, PANEL_W, 1.0)))?;

    if lobbies.is_empty() {
        let (sx, sy) = scale.point(SIDE_PAD + 24.0, header_y + 50.0);
        let text = if status.is_empty() { "Fetching public lobbies..." } else { status };
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), text, sx, sy, Color::RGB(0x7a, 0x7a, 0x82))?;
        return Ok(());
    }

    let row_h = 52.0;
    for (i, lobby) in lobbies.iter().enumerate() {
        let y = header_y + 40.0 + i as f32 * row_h;
        if y + row_h > PANEL_TOP + PANEL_H {
            break;
        }
        if i == cursor {
            let tint = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 30);
            let clear = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 4);
            geometry::fill_horizontal_gradient_rect(canvas, scale, SIDE_PAD, y, PANEL_W, row_h - 2.0, tint, clear);
        }
        let status_color = if lobby.status.eq_ignore_ascii_case("open") { theme::GREEN } else { theme::MUTE };
        let (stx, sty) = scale.point(SIDE_PAD + 24.0, y + row_h / 2.0 - 7.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), &lobby.status.to_uppercase(), stx, sty, status_color)?;
        let (hx, hy) = scale.point(SIDE_PAD + 24.0 + 140.0, y + row_h / 2.0 - 9.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(18.0), &lobby.host, hx, hy, Color::RGB(0xf2, 0xf2, 0xee))?;
        let (fx, fy) = scale.point(SIDE_PAD + 24.0 + 900.0, y + row_h / 2.0 - 7.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), lobby.format.label(), fx, fy, Color::RGB(0x9a, 0x9a, 0xa2))?;
        let (px, py) = scale.point(SIDE_PAD + 24.0 + 1200.0, y + row_h / 2.0 - 8.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(16.0), &format!("{}", lobby.players), px, py, Color::RGB(0xcf, 0xcf, 0xc9))?;
    }
    Ok(())
}

/// Chat tab — message list + presence sidebar, matching the mockup's own
/// two-column layout. Quick-phrase chips are real, native, and Confirm-able
/// (`FpResult::SendLobbyChat`); the mockup's own "△ TO OPEN KEYBOARD" hint
/// (there's no inline keyboard drawn in its own markup either) becomes the
/// "COMPOSE MESSAGE" row here — see `mod.rs`'s `FpResult::OpenLegacyChat`
/// doc comment for why that hands off to legacy instead of a native OSK.
fn draw_chat(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    chat: &[crate::matchmaking::LobbyChatMessage],
    presence: &[crate::matchmaking::LobbyUser],
    cursor: usize,
) -> Result<(), String> {
    let sidebar_w = 320.0;
    let messages_w = PANEL_W - sidebar_w - 1.0;

    // Message list, newest at the bottom (matching the mockup's own
    // `justify-content:flex-end` chat log), most recent MAX_MESSAGES shown.
    const MAX_MESSAGES: usize = 9;
    let row_h = 30.0;
    let list_top = PANEL_TOP + 16.0;
    let list_h = 300.0;
    let shown: Vec<&crate::matchmaking::LobbyChatMessage> = chat.iter().rev().take(MAX_MESSAGES).rev().collect();
    if shown.is_empty() {
        let msg = "No messages yet \u{2014} say hi";
        let (mw, _) = fonts.text_size(FpFont::SairaSemiBold, scale.font_px(15.0), msg);
        let (mx, my) = scale.point(SIDE_PAD + (messages_w - mw as f32 / scale.s) / 2.0, list_top + list_h / 2.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), msg, mx, my, theme::DIM)?;
    } else {
        let start_y = list_top + list_h - shown.len() as f32 * row_h;
        for (i, msg) in shown.iter().enumerate() {
            let y = start_y + i as f32 * row_h;
            if let Some(time) = &msg.timestamp {
                let (ttx, tty) = scale.point(SIDE_PAD + 24.0, y);
                fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(11.0), time, ttx, tty, Color::RGB(0x3a, 0x3a, 0x42))?;
            }
            let (ux, uy) = scale.point(SIDE_PAD + 24.0 + 52.0, y);
            let (uw, _) = fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), &msg.username, ux, uy, theme::ACCENT)?;
            let (mx, my) = scale.point(SIDE_PAD + 24.0 + 52.0 + (uw as f32 / scale.s) + 10.0, y);
            fonts.draw(canvas, FpFont::SairaMedium, scale.font_px(15.0), &msg.message, mx, my, Color::RGB(0xcf, 0xcf, 0xc9))?;
        }
    }

    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, list_top + list_h + 12.0, messages_w, 1.0)))?;

    // Quick phrases + compose row, one shared vertical cursor.
    let quick_y = list_top + list_h + 28.0;
    let mut px = SIDE_PAD + 24.0;
    let phrase_y_h = 34.0;
    for (i, phrase) in crate::menu::QUICK_PHRASES.iter().enumerate() {
        let selected = i == cursor;
        let (pw, ph) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(13.0), phrase);
        let chip_w = (pw as f32 / scale.s) + 22.0;
        let chip_h = (ph as f32 / scale.s) + 14.0;
        if px + chip_w > SIDE_PAD + messages_w - 24.0 {
            break;
        }
        canvas.set_draw_color(if selected {
            Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 46)
        } else {
            Color::RGBA(255, 255, 255, 10)
        });
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        canvas.fill_rect(Some(scale.rect(px, quick_y, chip_w, chip_h)))?;
        canvas.set_draw_color(if selected { theme::ACCENT } else { Color::RGBA(255, 255, 255, 24) });
        canvas.draw_rect(scale.rect(px, quick_y, chip_w, chip_h))?;
        let (tx2, ty2) = scale.point(px + 11.0, quick_y + 7.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), phrase, tx2, ty2, if selected { theme::TEXT } else { Color::RGB(0x9a, 0x9a, 0xa2) })?;
        px += chip_w + 8.0;
    }

    let compose_selected = cursor == crate::menu::QUICK_PHRASES.len();
    let compose_y = quick_y + phrase_y_h + 14.0;
    canvas.set_draw_color(if compose_selected {
        Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 30)
    } else {
        Color::RGBA(255, 255, 255, 8)
    });
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD + 24.0, compose_y, messages_w - 48.0, 40.0)))?;
    canvas.set_draw_color(if compose_selected { theme::ACCENT } else { Color::RGBA(255, 255, 255, 20) });
    canvas.draw_rect(scale.rect(SIDE_PAD + 24.0, compose_y, messages_w - 48.0, 40.0))?;
    let hint = "SELECT TO OPEN KEYBOARD \u{b7} TYPE A MESSAGE";
    let (hx, hy) = scale.point(SIDE_PAD + 24.0 + 14.0, compose_y + 13.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(13.0), hint, hx, hy, if compose_selected { theme::TEXT } else { theme::MUTE })?;

    // Presence sidebar.
    let sidebar_x = SIDE_PAD + messages_w + 1.0;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 12));
    canvas.fill_rect(Some(scale.rect(sidebar_x, PANEL_TOP, 1.0, PANEL_H)))?;
    let header = format!("{} ONLINE", presence.len());
    let (phx, phy) = scale.point(sidebar_x + 20.0, PANEL_TOP + 16.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), &header, phx, phy, theme::MUTE, scale.len(3.0).round() as i32)?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 12));
    canvas.fill_rect(Some(scale.rect(sidebar_x, PANEL_TOP + 40.0, sidebar_w, 1.0)))?;

    let prow_h = 46.0;
    for (i, user) in presence.iter().enumerate() {
        let y = PANEL_TOP + 40.0 + i as f32 * prow_h;
        if y + prow_h > PANEL_TOP + PANEL_H {
            break;
        }
        let dot_color = if user.status.eq_ignore_ascii_case("online") { theme::GREEN } else { theme::MUTE };
        geometry::fill_circle(canvas, scale, sidebar_x + 20.0 + 4.0, y + prow_h / 2.0, 4.0, dot_color);
        let (nx, ny) = scale.point(sidebar_x + 20.0 + 18.0, y + prow_h / 2.0 - 15.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(14.0), &user.username, nx, ny, Color::RGB(0xed, 0xed, 0xe8))?;
        let (stx, sty) = scale.point(sidebar_x + 20.0 + 18.0, y + prow_h / 2.0 + 2.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(10.0), &user.status, stx, sty, Color::RGB(0x52, 0x52, 0x5a))?;
        if let Some(rating) = user.rating {
            let rtext = rating.to_string();
            let (rw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(12.0), &rtext);
            let (rx, ry) = scale.point(sidebar_x + sidebar_w - 20.0 - (rw as f32 / scale.s), y + prow_h / 2.0 - 7.0);
            fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(12.0), &rtext, rx, ry, Color::RGB(0x4a, 0x4a, 0x52))?;
        }
    }
    Ok(())
}

/// Watch tab — live-match list, matching the mockup's own card layout minus
/// the round/duration fields (`matchmaking::LiveMatch` has no real data to
/// back those — session id, names, and score only).
fn draw_watch(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    live_matches: &[crate::matchmaking::LiveMatch],
    cursor: usize,
) -> Result<(), String> {
    let header_y = PANEL_TOP + 20.0;
    let header = format!("{} LIVE MATCHES \u{2014} SELECT TO SPECTATE", live_matches.len());
    let (hx, hy) = scale.point(SIDE_PAD + 24.0, header_y);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), &header, hx, hy, theme::MUTE, scale.len(3.0).round() as i32)?;

    geometry::fill_circle(canvas, scale, SIDE_PAD + PANEL_W - 70.0, header_y + 6.0, 3.5, theme::ACCENT);
    let (lx, ly) = scale.point(SIDE_PAD + PANEL_W - 58.0, header_y - 2.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(12.0), "LIVE", lx, ly, theme::ACCENT, scale.len(3.0).round() as i32)?;

    if live_matches.is_empty() {
        let msg = "No live matches right now";
        let (mw, _) = fonts.text_size(FpFont::SairaSemiBold, scale.font_px(15.0), msg);
        let (mx, my) = scale.point(SIDE_PAD + (PANEL_W - mw as f32 / scale.s) / 2.0, PANEL_TOP + PANEL_H / 2.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), msg, mx, my, theme::DIM)?;
        return Ok(());
    }

    let row_h = 84.0;
    let list_top = header_y + 32.0;
    for (i, m) in live_matches.iter().enumerate() {
        let y = list_top + i as f32 * (row_h + 10.0);
        if y + row_h > PANEL_TOP + PANEL_H {
            break;
        }
        let selected = i == cursor;
        canvas.set_draw_color(if selected {
            Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 26)
        } else {
            Color::RGBA(8, 8, 11, 140)
        });
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, PANEL_W, row_h)))?;
        canvas.set_draw_color(if selected { theme::ACCENT } else { Color::RGBA(255, 255, 255, 18) });
        canvas.draw_rect(scale.rect(SIDE_PAD, y, PANEL_W, row_h))?;

        let score = format!("{}-{}", m.p1_score, m.p2_score);
        let center_x = SIDE_PAD + PANEL_W / 2.0;
        let (p1w, _) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(28.0), &m.p1_name.to_uppercase());
        let (scw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(22.0), &score);
        let gap = 24.0;
        let p1_x = center_x - (p1w as f32 / scale.s) - gap / 2.0 - (scw as f32 / scale.s) / 2.0;
        let (p1x, p1y) = scale.point(p1_x, y + row_h / 2.0 - 16.0);
        fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(28.0), &m.p1_name.to_uppercase(), p1x, p1y, theme::TEXT)?;
        let (scx, scy) = scale.point(center_x - (scw as f32 / scale.s) / 2.0, y + row_h / 2.0 - 13.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(22.0), &score, scx, scy, theme::ACCENT)?;
        let (p2x, p2y) = scale.point(center_x + (scw as f32 / scale.s) / 2.0 + gap / 2.0, y + row_h / 2.0 - 16.0);
        fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(28.0), &m.p2_name.to_uppercase(), p2x, p2y, theme::TEXT)?;

        let watch_label = "WATCH";
        let (ww, wh) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(15.0), watch_label);
        let btn_w = (ww as f32 / scale.s) + 32.0;
        let btn_h = (wh as f32 / scale.s) + 18.0;
        let btn_x = SIDE_PAD + PANEL_W - 24.0 - btn_w;
        let btn_y = y + row_h / 2.0 - btn_h / 2.0;
        canvas.set_draw_color(if selected { theme::ACCENT } else { Color::RGBA(255, 255, 255, 24) });
        canvas.draw_rect(scale.rect(btn_x, btn_y, btn_w, btn_h))?;
        let (wtx, wty) = scale.point(btn_x + 16.0, btn_y + 9.0);
        fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(15.0), watch_label, wtx, wty, if selected { theme::TEXT } else { Color::RGB(0x9a, 0x9a, 0xa2) })?;
    }
    Ok(())
}
