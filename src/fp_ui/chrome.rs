//! Header and footer chrome shared by every fp_ui screen. Positions/sizes
//! transcribed from `FREEPLAY Arcade.dc.html`'s header/footer `<div>`s
//! (lines ~43-108 and ~1116-1185), which are identical across all `sc-if`
//! screen branches — only the footer's right-side content and prompt list
//! change per screen.

use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

pub const HEADER_H: f32 = 104.0;
pub const FOOTER_H: f32 = 86.0;
const SIDE_PAD: f32 = 56.0;

pub struct FooterPrompt {
    pub glyph: &'static str,
    pub label: &'static str,
    pub color: Color,
}

/// D-pad / Cross / Circle prompt chips, colors from the handoff doc's
/// controller mapping table.
pub const PROMPT_NAVIGATE: FooterPrompt = FooterPrompt {
    glyph: "\u{2195}",
    label: "Navigate",
    color: Color::RGB(0xcf, 0xcf, 0xc9),
};
pub const PROMPT_SELECT: FooterPrompt = FooterPrompt {
    // "X" rather than the Unicode multiplication-X glyph (U+2715) — also
    // missing from Saira Condensed Bold, same issue as PROMPT_BACK's U+25CB.
    glyph: "X",
    label: "Select",
    color: theme::BTN_CROSS,
};
#[allow(dead_code)] // used starting with the Play/Settings/Lobby steps
pub const PROMPT_BACK: FooterPrompt = FooterPrompt {
    // "O" rather than the Unicode circle glyph (U+25CB) — missing from
    // Saira Condensed Bold, rendering as a tofu box. The chip's own
    // stroked-circle outline already reads as "Circle button", so a plain
    // letter inside it isn't a legibility loss.
    glyph: "O",
    label: "Back",
    color: theme::BTN_CIRCLE,
};

/// Get the local wall-clock hour/minute via libc (already a dependency) —
/// no `chrono`, since portable local-time formatting is the only thing that
/// crate would be for here.
#[cfg(windows)]
fn local_hour_minute() -> (i32, i32) {
    unsafe {
        let mut t: libc::time_t = 0;
        libc::time(&mut t);
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_s(&mut tm, &t);
        (tm.tm_hour, tm.tm_min)
    }
}

#[cfg(not(windows))]
fn local_hour_minute() -> (i32, i32) {
    unsafe {
        let mut t: libc::time_t = 0;
        libc::time(&mut t);
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_hour, tm.tm_min)
    }
}

/// "6:47 PM" — matches the header clock's format in the mockup.
pub fn clock_string() -> String {
    let (h24, m) = local_hour_minute();
    let period = if h24 >= 12 { "PM" } else { "AM" };
    let h12 = match h24 % 12 {
        0 => 12,
        h => h,
    };
    format!("{h12}:{m:02} {period}")
}

/// Header: FREEPLAY wordmark + build tag (left), server/ping/clock/profile
/// chip (right). `username` is the player's display name; `online` and
/// `ping_ms` reflect the signaling connection (both `None`/`false` render
/// the offline-safe variant — dim dot, no ping figure — rather than
/// fabricating a fake connection state).
pub fn draw_header(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    username: &str,
    online: bool,
    ping_ms: Option<u32>,
) -> Result<(), String> {
    let border_y = scale.point(0.0, HEADER_H).1;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(sdl2::rect::Rect::new(
        0,
        border_y,
        scale.rect(0.0, 0.0, theme::VW, 1.0).width().max(1),
        1,
    )))?;

    // Left: wordmark + build tag.
    let (x, y) = scale.point(SIDE_PAD, HEADER_H / 2.0 - 28.0);
    let (word_w, _) = fonts.draw(
        canvas,
        FpFont::SairaCondensedBlack,
        scale.font_px(56.0),
        "FREEPLAY",
        x,
        y,
        theme::TEXT,
    )?;
    let (bx, by) = scale.point(SIDE_PAD + (word_w as f32 / scale.s) + 20.0, HEADER_H / 2.0 - 7.0);
    fonts.draw(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(13.0),
        &format!("BUILD {}", crate::version::VERSION),
        bx,
        by,
        theme::MUTE,
    )?;

    // Right side is laid out right-to-left: profile chip, clock, divider,
    // status dot + label + ping — each block's width is measured before the
    // cursor steps left past it, so blocks never overlap regardless of
    // username/ping-string length.
    let dot_color = if online { theme::GREEN } else { theme::MUTE };
    let status_label = if online { "SERVERS ONLINE" } else { "OFFLINE" };
    let ping_label = ping_ms.map(|p| format!("{p} ms")).unwrap_or_default();
    let mut cursor_x = theme::VW - SIDE_PAD;

    // Profile chip (rightmost): avatar circle + username.
    let chip_text_w = fonts
        .text_size(FpFont::SairaCondensedBold, scale.font_px(17.0), username)
        .0 as f32
        / scale.s;
    let avatar_d = 32.0;
    let chip_w = avatar_d + 10.0 + chip_text_w + 14.0 + 8.0;
    cursor_x -= chip_w;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 8));
    canvas.fill_rect(Some(scale.rect(cursor_x, HEADER_H / 2.0 - 16.0, chip_w, 32.0)))?;
    fill_circle(canvas, scale, cursor_x + 8.0 + avatar_d / 2.0, HEADER_H / 2.0, avatar_d / 2.0, theme::ACCENT);
    let initial = username.chars().next().unwrap_or('?').to_uppercase().to_string();
    let (iw, ih) = fonts.text_size(FpFont::SairaCondensedBlack, scale.font_px(15.0), &initial);
    let (ix, iy) = scale.point(
        cursor_x + 8.0 + avatar_d / 2.0 - (iw as f32 / scale.s) / 2.0,
        HEADER_H / 2.0 - (ih as f32 / scale.s) / 2.0,
    );
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(15.0), &initial, ix, iy, Color::RGB(255, 255, 255))?;
    let (nx, ny) = scale.point(cursor_x + 8.0 + avatar_d + 10.0, HEADER_H / 2.0 - 10.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(17.0), username, nx, ny, theme::TEXT)?;

    // Clock.
    cursor_x -= 24.0; // gap before the chip
    let clock = clock_string();
    let clock_w = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(16.0), &clock).0 as f32 / scale.s;
    cursor_x -= clock_w;
    let (cx, cy) = scale.point(cursor_x, HEADER_H / 2.0 - 8.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(16.0), &clock, cx, cy, theme::TEXT)?;

    // Divider.
    cursor_x -= 24.0;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 26));
    canvas.fill_rect(Some(scale.rect(cursor_x, HEADER_H / 2.0 - 15.0, 1.0, 30.0)))?;

    // Status dot + label + ping.
    cursor_x -= 18.0;
    let status_w = fonts
        .text_size(FpFont::ChakraPetchSemiBold, scale.font_px(14.0), status_label)
        .0 as f32
        / scale.s;
    let ping_w = if ping_label.is_empty() {
        0.0
    } else {
        fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(14.0), &ping_label).0 as f32 / scale.s + 12.0
    };
    cursor_x -= status_w + ping_w;
    let (sx, sy) = scale.point(cursor_x, HEADER_H / 2.0 - 7.0);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(14.0), status_label, sx, sy, theme::DIM)?;
    if !ping_label.is_empty() {
        let (px, py) = scale.point(cursor_x + status_w + 12.0, HEADER_H / 2.0 - 7.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(14.0), &ping_label, px, py, theme::MUTE)?;
    }
    cursor_x -= 9.0;
    fill_circle(canvas, scale, cursor_x, HEADER_H / 2.0, 4.0, dot_color);

    Ok(())
}

/// What the footer's right side shows. Each fp_ui screen picks the variant
/// matching its `sc-if` branch in the mockup (`isMenu`, `isSettings`, ...).
pub enum FooterRight<'a> {
    /// Main Menu: "SELECT · About" + "CREDIT ∞" + blinking FREE PLAY badge.
    Menu,
    #[allow(dead_code)] // used starting with the Settings step
    Text(&'a str),
}

/// Draw a left-to-right row of button-prompt chips starting at logical
/// `(x, row_cy)` (`row_cy` is the row's vertical center). Shared by the
/// normal footer and the Quit overlay, which redraws this row with
/// different prompts on top of the dim backdrop.
pub fn draw_prompt_row(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    prompts: &[FooterPrompt],
    x: f32,
    row_cy: f32,
) -> Result<(), String> {
    let mut x = x;
    let chip_d = 34.0;
    for p in prompts {
        stroke_circle(canvas, scale, x + chip_d / 2.0, row_cy, chip_d / 2.0, p.color);
        let (gw, gh) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(15.0), p.glyph);
        let (gx, gy) = scale.point(
            x + chip_d / 2.0 - (gw as f32 / scale.s) / 2.0,
            row_cy - (gh as f32 / scale.s) / 2.0,
        );
        fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(15.0), p.glyph, gx, gy, p.color)?;
        let (lx, ly) = scale.point(x + chip_d + 10.0, row_cy - 8.0);
        let (lw, _) = fonts.draw(
            canvas,
            FpFont::SairaCondensedSemiBold,
            scale.font_px(13.0),
            &p.label.to_uppercase(),
            lx,
            ly,
            theme::DIM,
        )?;
        x += chip_d + 10.0 + (lw as f32 / scale.s) + 26.0;
    }
    Ok(())
}

pub fn draw_footer(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    prompts: &[FooterPrompt],
    right: FooterRight,
) -> Result<(), String> {
    let top_y = theme::VH - FOOTER_H;
    let (_, border_y) = scale.point(0.0, top_y);
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(sdl2::rect::Rect::new(
        0,
        border_y,
        scale.rect(0.0, 0.0, theme::VW, 1.0).width().max(1),
        1,
    )))?;

    let row_cy = top_y + FOOTER_H / 2.0;
    draw_prompt_row(canvas, fonts, scale, prompts, SIDE_PAD, row_cy)?;

    match right {
        FooterRight::Menu => {
            let text = "FREE PLAY";
            let (tw, th) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(14.0), text);
            let pad_x = 12.0;
            let pad_y = 5.0;
            let badge_w = (tw as f32 / scale.s) + pad_x * 2.0;
            let badge_h = (th as f32 / scale.s) + pad_y * 2.0;
            let badge_x = theme::VW - SIDE_PAD - badge_w;
            let badge_y = row_cy - badge_h / 2.0;
            canvas.set_draw_color(theme::ACCENT);
            canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
            canvas.fill_rect(Some(scale.rect(badge_x, badge_y, badge_w, badge_h)))?;
            let (tx, ty) = scale.point(badge_x + pad_x, badge_y + pad_y);
            fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(14.0), text, tx, ty, Color::RGB(255, 255, 255))?;
        }
        FooterRight::Text(s) => {
            let (tw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(13.0), s);
            let (tx, ty) = scale.point(theme::VW - SIDE_PAD - (tw as f32 / scale.s), row_cy - 7.0);
            fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), s, tx, ty, theme::MUTE)?;
        }
    }

    Ok(())
}

/// Cheap filled-circle approximation: enough horizontal strips to look round
/// at menu-chrome sizes (~8-34px diameter) without needing a texture asset.
fn fill_circle(canvas: &mut Canvas<Window>, scale: &Scale, cx: f32, cy: f32, r: f32, color: Color) {
    if color.a == 0 {
        return;
    }
    canvas.set_draw_color(color);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    let steps = 24;
    for i in 0..steps {
        let t = -1.0 + 2.0 * (i as f32) / (steps as f32 - 1.0);
        let y = cy + t * r;
        let half_w = (r * r - (t * r) * (t * r)).max(0.0).sqrt();
        let rect = scale.rect(cx - half_w, y, half_w * 2.0, (r * 2.0 / steps as f32).max(1.0));
        let _ = canvas.fill_rect(Some(rect));
    }
}

fn stroke_circle(canvas: &mut Canvas<Window>, scale: &Scale, cx: f32, cy: f32, r: f32, color: Color) {
    canvas.set_draw_color(color);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    let segments = 28;
    for i in 0..segments {
        let a0 = std::f32::consts::TAU * (i as f32) / segments as f32;
        let a1 = std::f32::consts::TAU * (i as f32 + 1.0) / segments as f32;
        let p0 = scale.point(cx + a0.cos() * r, cy + a0.sin() * r);
        let p1 = scale.point(cx + a1.cos() * r, cy + a1.sin() * r);
        let _ = canvas.draw_line(p0, p1);
    }
}
