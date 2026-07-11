//! Session Ended — native redesign of legacy's `MenuScreen::SessionEnded`
//! (shown after a netplay disconnect/timeout so the player gets a calm
//! summary instead of a diagnostics-style failure panel). Matches the new
//! mockup's `isSessionEnd` branch: a centered card over the ambient
//! background, `lines` colored by the same "OK "/"WARN "/plain prefix
//! convention the legacy screen already uses (`crate::menu::draw_session_ended`),
//! plus a two-button row (WATCH REPLAY / RETURN TO MENU) navigated the same
//! way Quit's Cancel/Exit row is (`FpScreen::Quit`'s `choice`) rather than a
//! mockup-only "R button" shortcut, since fp_ui has no free face-button
//! mapping for that. Real data only: `lines`/`replay_path` are populated by
//! `main.rs` exactly the way it already builds the legacy screen's fields.

use super::chrome;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const CARD_W: f32 = 760.0;
const TOP_BAR_H: f32 = 5.0;
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
    replay_path: Option<&str>,
    choice: usize,
    username: &str,
) -> Result<(), String> {
    chrome::draw_background_accents_no_glow(canvas, scale)?;
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    let has_replay = replay_path.is_some();
    let lines_h = (lines.len().max(1)) as f32 * LINE_H;
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
    canvas.fill_rect(Some(scale.rect(card_x, card_y, CARD_W, TOP_BAR_H)))?;

    let content_x = card_x + PAD_X;
    let mut y = card_y + PAD_TOP;

    let (ex, ey) = scale.point(content_x, y);
    fonts.draw_tracked(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(13.0),
        "SESSION ENDED",
        ex,
        ey,
        theme::ACCENT,
        scale.len(4.0).round() as i32,
    )?;
    y += 30.0;
    let (tx, ty) = scale.point(content_x, y);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(34.0), "NETPLAY SESSION SUMMARY", tx, ty, theme::TEXT)?;
    y += 44.0;

    for line in lines {
        let (dot_color, text) = if let Some(rest) = line.strip_prefix("OK ") {
            (theme::GREEN, rest)
        } else if let Some(rest) = line.strip_prefix("WARN ") {
            (theme::WARNING, rest)
        } else {
            (Color::RGB(0x62, 0x62, 0x6c), line.as_str())
        };
        super::geometry::fill_circle(canvas, scale, content_x + 4.0, y + LINE_H / 2.0, 4.0, dot_color);
        let (lx, ly) = scale.point(content_x + 20.0, y + LINE_H / 2.0 - 9.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(16.0), text, lx, ly, Color::RGB(0xcf, 0xcf, 0xc9))?;
        y += LINE_H;
    }
    y += 30.0;

    let btn_gap = 14.0;
    let btn_w = if has_replay { (CARD_W - PAD_X * 2.0 - btn_gap) / 2.0 } else { CARD_W - PAD_X * 2.0 };
    let mut bx = content_x;
    if has_replay {
        draw_button(canvas, fonts, scale, bx, y, btn_w, BTN_H, "WATCH REPLAY", choice == 0, false)?;
        bx += btn_w + btn_gap;
    }
    let return_selected = if has_replay { choice == 1 } else { true };
    draw_button(canvas, fonts, scale, bx, y, btn_w, BTN_H, "RETURN TO MENU", return_selected, true)?;

    let prompts: &[chrome::FooterPrompt] = if has_replay {
        &[
            chrome::FooterPrompt { glyph: "\u{2194}", label: "Choose", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::PROMPT_SELECT,
        ]
    } else {
        &[chrome::PROMPT_SELECT]
    };
    chrome::draw_footer(canvas, fonts, scale, prompts, chrome::FooterRight::Text("MATCH SUMMARY"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_button(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    selected: bool,
    is_primary: bool,
) -> Result<(), String> {
    let (border, bg, text_color) = if selected && is_primary {
        (theme::ACCENT, theme::ACCENT, Color::RGB(255, 255, 255))
    } else if selected {
        (Color::RGBA(255, 255, 255, 128), Color::RGBA(255, 255, 255, 20), theme::TEXT)
    } else {
        (Color::RGBA(255, 255, 255, 31), Color::RGBA(0, 0, 0, 0), Color::RGB(0x8a, 0x8a, 0x92))
    };
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    if bg.a > 0 {
        canvas.set_draw_color(bg);
        canvas.fill_rect(Some(scale.rect(x, y, w, h)))?;
    }
    canvas.set_draw_color(border);
    canvas.draw_rect(scale.rect(x, y, w, h))?;

    let (tw, th) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(21.0), label);
    let (tx, ty) = scale.point(x + w / 2.0 - (tw as f32 / scale.s) / 2.0, y + h / 2.0 - (th as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(21.0), label, tx, ty, text_color)?;
    Ok(())
}
