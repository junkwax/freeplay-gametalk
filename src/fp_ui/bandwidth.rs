//! Network News ("the wire") — matches the mockup's `isBandwidth` branch.
//!
//! Unlike every other new screen added alongside this one, there is no
//! backend anywhere in this app for bulletins/line notices — no `bulletin`
//! or `news` concept exists in `matchmaking.rs` or anywhere else. The
//! mockup's own content is 100% static mocked data too (`aboutBuildDefs`-
//! style literal arrays, no fetch), so this screen matches that: decorative
//! content for visual parity, not a real feed. If a real bulletin backend
//! ever exists, swap these constants for whatever it returns.

use super::chrome::{self, FooterRight};
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const SIDE_PAD: f32 = 56.0;
const TOP: f32 = 38.0 + 104.0; // this screen's own top:38 + header height

struct Bulletin {
    time: &'static str,
    title: &'static str,
    sub: &'static str,
    highlighted: bool,
}

/// The Main Menu's scrolling wire ticker reuses this same flavor text
/// (`main_menu.rs`'s `draw_ticker`) rather than inventing a second static
/// string — still no real bulletin backend, see this module's doc comment.
pub const TICKER_TEXT: &str = "WIRE // WEST REGION FINALS SATURDAY 20:00 \u{b7} PHANTOM_9847 ENTERS TOP 50 \u{b7} NETCODE PATCH 2.4.1 LIVE ON ALL NODES \u{b7} NODE MAINTENANCE SUNDAY 03:00\u{2013}05:00 \u{b7} SEASON LADDER RESETS IN 9 DAYS";

const BULLETINS: [Bulletin; 6] = [
    Bulletin {
        time: "21:15",
        title: "PHANTOM_9847 ENTERS TOP 50 NATIONAL",
        sub: "A four-win streak lifts the challenger into ladder contention.",
        highlighted: true,
    },
    Bulletin {
        time: "19:48",
        title: "SEASON LADDER RESETS IN 9 DAYS",
        sub: "",
        highlighted: false,
    },
    Bulletin {
        time: "17:30",
        title: "NODE MAINTENANCE \u{2014} SUNDAY 03:00\u{2013}05:00",
        sub: "",
        highlighted: false,
    },
    Bulletin {
        time: "14:12",
        title: "NETCODE PATCH 2.4.1 LIVE ON ALL NODES",
        sub: "",
        highlighted: false,
    },
    Bulletin {
        time: "11:05",
        title: "EAST VS WEST CROSS-REGION NIGHT RETURNS",
        sub: "",
        highlighted: false,
    },
    Bulletin {
        time: "FRI",
        title: "NEW NODE ONLINE: SEATTLE SEA-02",
        sub: "",
        highlighted: false,
    },
];

pub fn draw(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, username: &str) -> Result<(), String> {
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, TOP + 8.0, 30.0, 3.0)))?;
    let (ex, ey) = scale.point(SIDE_PAD + 44.0, TOP);
    fonts.draw(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(13.0),
        "THE WIRE \u{b7} LINE NOTICES",
        ex,
        ey,
        theme::ACCENT,
    )?;
    let (tx, ty) = scale.point(SIDE_PAD, TOP + 26.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(58.0), "NETWORK NEWS", tx, ty, theme::TEXT)?;

    let issue = "ISSUE 214 \u{b7} SAT JUL 04 \u{b7} UPDATED 21:40";
    let (iw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(13.0), issue);
    let (ix, iy) = scale.point(theme::VW - SIDE_PAD - (iw as f32 / scale.s), TOP - 8.0);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), issue, ix, iy, theme::MUTE)?;

    let body_top = TOP + 26.0 + 70.0;
    draw_headline_card(canvas, fonts, scale, body_top)?;
    draw_bulletin_list(canvas, fonts, scale, body_top)?;

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::PROMPT_NAVIGATE, chrome::PROMPT_BACK],
        FooterRight::Text("6 OF 23 BULLETINS"),
    )?;
    Ok(())
}

fn draw_headline_card(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, top: f32) -> Result<(), String> {
    let x = SIDE_PAD;
    let w = 660.0;
    let h = 480.0;

    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(x, top, w, h)))?;
    canvas.set_draw_color(Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 90));
    canvas.draw_rect(scale.rect(x, top, w, h))?;

    let badge_text = "HEADLINE";
    let (bw, bh) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(16.0), badge_text);
    let badge_pad_x = 16.0;
    let badge_pad_y = 5.0;
    let badge_w = (bw as f32 / scale.s) + badge_pad_x * 2.0;
    let badge_h = (bh as f32 / scale.s) + badge_pad_y * 2.0;
    canvas.set_draw_color(theme::ACCENT);
    canvas.fill_rect(Some(scale.rect(x + 24.0, top + 24.0, badge_w, badge_h)))?;
    let (btx, bty) = scale.point(x + 24.0 + badge_pad_x, top + 24.0 + badge_pad_y);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(16.0), badge_text, btx, bty, Color::RGB(255, 255, 255))?;

    let posted = "POSTED 18:02";
    let (px, py) = scale.point(x + 24.0 + badge_w + 12.0, top + 24.0 + 5.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(12.0), posted, px, py, theme::MUTE)?;

    let headline_y = top + 24.0 + badge_h + 20.0;
    let line1 = "WEST REGION FINALS";
    let line2 = "LOCK IN SATURDAY 20:00";
    let (l1x, l1y) = scale.point(x + 24.0, headline_y);
    let (_, l1h) = fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(52.0), line1, l1x, l1y, theme::TEXT)?;
    let (l2x, l2y) = scale.point(x + 24.0, headline_y + (l1h as f32 / scale.s) + 4.0);
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(52.0), line2, l2x, l2y, theme::TEXT)?;

    let body_y = headline_y + (l1h as f32 / scale.s) * 2.0 + 40.0;
    let body = "Sixteen challengers survive the qualifier gauntlet. Winner takes the";
    let body2 = "regional crown, a guaranteed national seed \u{2014} and one month of free line time.";
    let (bxp, byp) = scale.point(x + 24.0, body_y);
    fonts.draw(canvas, FpFont::SairaMedium, scale.font_px(18.0), body, bxp, byp, Color::RGB(0x8a, 0x8a, 0x92))?;
    let (bxp2, byp2) = scale.point(x + 24.0, body_y + 30.0);
    fonts.draw(canvas, FpFont::SairaMedium, scale.font_px(18.0), body2, bxp2, byp2, Color::RGB(0x8a, 0x8a, 0x92))?;

    let footer_y = top + h - 20.0 - 24.0;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(x + 24.0, footer_y, w - 48.0, 1.0)))?;
    let (fx, fy) = scale.point(x + 24.0, footer_y + 20.0);
    fonts.draw(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(13.0),
        "READ FULL BULLETIN",
        fx,
        fy,
        theme::ACCENT,
    )?;

    Ok(())
}

fn draw_bulletin_list(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, top: f32) -> Result<(), String> {
    let x = SIDE_PAD + 660.0 + 26.0;
    let w = theme::VW - SIDE_PAD - x;
    let h = 480.0;

    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(x, top, w, h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(x, top, w, h))?;

    let row_h = h / BULLETINS.len() as f32;
    for (i, b) in BULLETINS.iter().enumerate() {
        let ry = top + i as f32 * row_h;
        if b.highlighted {
            canvas.set_draw_color(Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 15));
            canvas.fill_rect(Some(scale.rect(x, ry, w, row_h)))?;
            canvas.set_draw_color(theme::ACCENT);
            canvas.fill_rect(Some(scale.rect(x, ry, 4.0, row_h)))?;
        }
        if i > 0 {
            canvas.set_draw_color(Color::RGBA(255, 255, 255, 12));
            canvas.fill_rect(Some(scale.rect(x, ry, w, 1.0)))?;
        }
        let time_color = if b.highlighted { theme::ACCENT } else { Color::RGB(0x4a, 0x4a, 0x52) };
        let (ttx, tty) = scale.point(x + 24.0, ry + row_h / 2.0 - 8.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), b.time, ttx, tty, time_color)?;

        let title_x = x + 24.0 + 70.0;
        let title_color = if b.highlighted { theme::TEXT } else { Color::RGB(0x9a, 0x9a, 0xa2) };
        let title_size = if b.highlighted { 26.0 } else { 24.0 };
        if b.sub.is_empty() {
            let (tx2, ty2) = scale.point(title_x, ry + row_h / 2.0 - 12.0);
            fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(title_size), b.title, tx2, ty2, title_color)?;
        } else {
            let (tx2, ty2) = scale.point(title_x, ry + row_h / 2.0 - 20.0);
            fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(title_size), b.title, tx2, ty2, title_color)?;
            let (sx2, sy2) = scale.point(title_x, ry + row_h / 2.0 + 8.0);
            fonts.draw(canvas, FpFont::SairaMedium, scale.font_px(15.0), b.sub, sx2, sy2, Color::RGB(0x8a, 0x8a, 0x92))?;
        }
    }

    Ok(())
}
