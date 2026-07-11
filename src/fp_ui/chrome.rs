//! Header and footer chrome shared by every fp_ui screen. Positions/sizes
//! transcribed from `FREEPLAY Arcade.dc.html`'s header/footer `<div>`s
//! (lines ~43-108 and ~1116-1185), which are identical across all `sc-if`
//! screen branches — only the footer's right-side content and prompt list
//! change per screen.

use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

pub const HEADER_H: f32 = 104.0;
pub const FOOTER_H: f32 = 86.0;
const SIDE_PAD: f32 = 56.0;

/// Decorative background layer, drawn first (behind everything else, before
/// `draw_header`): the mockup's full layered stage background — a 3-stop
/// radial glow off-center toward the upper right, a faint diagonal accent
/// wash, a skewed light bar and red hairline down the content area, and a
/// vignette darkening the edges on top of all of it. Shared across every
/// fp_ui screen that wants this background treatment (currently the Main
/// Menu and Play submenu) rather than duplicated per screen.
///
/// Two earlier passes here under-specified this: a flat top-to-bottom
/// vertical fade (the glow is actually off-center, not vertical), then a
/// 2-stop circular approximation of the radial glow/vignette (both are
/// really CSS two-value-sized *ellipses* against the non-square 1920x1080
/// box — a shared x/y radius distorts the shape — and the glow is a 3-stop
/// gradient, not 2). `geometry::fill_radial_ellipse_gradient` now
/// reproduces both properly (independent rx/ry, arbitrary stop count).
/// Layer order matches the CSS stacking context back-to-front: stage base,
/// diagonal wash, skewed light bar, skewed hairline, vignette on top of all
/// of it. `skew_deg` was dropped as a parameter — the background's own
/// skewed elements are a fixed -11deg per the CSS regardless of whatever
/// skew angle the calling screen's foreground rows use.
pub fn draw_background_accents(canvas: &mut Canvas<Window>, scale: &Scale) -> Result<(), String> {
    draw_stage_glow(canvas, scale)?;
    draw_background_accents_no_glow(canvas, scale)
}

/// The off-center radial "stage glow" ellipse alone — split out of
/// `draw_background_accents` so screens that don't want it can still get
/// the rest of the layered background via `draw_background_accents_no_glow`.
fn draw_stage_glow(canvas: &mut Canvas<Window>, scale: &Scale) -> Result<(), String> {
    // Stage base: radial-gradient(120% 120% at 78% 18%, #121217 0%,
    // #0a0a0d 42%, #060608 100%). Ellipse, not circle — 120% of a
    // 1920x1080 box is not the same in both axes.
    geometry::fill_radial_ellipse_gradient(
        canvas,
        scale,
        theme::VW * 0.78,
        theme::VH * 0.18,
        theme::VW * 1.2,
        theme::VH * 1.2,
        &[
            (0.0, Color::RGB(0x12, 0x12, 0x17)),
            (0.42, Color::RGB(0x0a, 0x0a, 0x0d)),
            (1.0, Color::RGB(0x06, 0x06, 0x08)),
        ],
    );
    Ok(())
}

/// Same layered background as `draw_background_accents`, minus the
/// off-center radial "stage glow" ellipse (the neutral gray sunburst at
/// 78%/18%) — per direct feedback that the glow specifically wasn't
/// wanted on some screens, while the diagonal red wash, the left-side
/// skewed light bar, the red hairline, and the vignette all still are.
pub fn draw_background_accents_no_glow(canvas: &mut Canvas<Window>, scale: &Scale) -> Result<(), String> {
    const BG_SKEW_DEG: f32 = -11.0;

    // Diagonal accent wash: linear-gradient(115deg, transparent 0%,
    // transparent 60%, rgba(226,42,53,.05) 78%, transparent 80%) — the
    // 0%-60% run is uniformly transparent, so only the 60/78/80 stops are
    // reproduced.
    let wash_clear = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 0);
    let wash_peak = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 13); // .05 * 255
    geometry::fill_linear_gradient_box(
        canvas,
        scale,
        115.0,
        &[(0.60, wash_clear), (0.78, wash_peak), (0.80, wash_clear)],
    );

    // Skewed light bar: rect(-200,-120, 560x1400) skewX(-11deg),
    // linear-gradient(180deg, rgba(255,255,255,.018), rgba(255,255,255,0)).
    geometry::fill_skewed_rect_vgradient(
        canvas,
        scale,
        -200.0,
        -120.0,
        560.0,
        1400.0,
        BG_SKEW_DEG,
        &[(0.0, Color::RGBA(255, 255, 255, 5)), (1.0, Color::RGBA(255, 255, 255, 0))],
    );

    // Skewed red hairline: rect(780,-120, 5x1400) skewX(-11deg),
    // linear-gradient(180deg, transparent, accent@.5 35%, accent@.5 60%,
    // transparent), the whole shape at 0.5 opacity — combined peak alpha
    // 0.5 * 0.5 * 255 ≈ 64.
    let hair_clear = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 0);
    let hair_peak = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 64);
    geometry::fill_skewed_rect_vgradient(
        canvas,
        scale,
        780.0,
        -120.0,
        5.0,
        1400.0,
        BG_SKEW_DEG,
        &[(0.0, hair_clear), (0.35, hair_peak), (0.60, hair_peak), (1.0, hair_clear)],
    );

    // Vignette: radial-gradient(130% 130% at 50% 45%, transparent 55%,
    // rgba(0,0,0,.55) 100%) — drawn last, on top of everything above.
    geometry::fill_radial_ellipse_gradient(
        canvas,
        scale,
        theme::VW * 0.5,
        theme::VH * 0.45,
        theme::VW * 1.3,
        theme::VH * 1.3,
        &[(0.55, Color::RGBA(0, 0, 0, 0)), (1.0, Color::RGBA(0, 0, 0, 140))],
    );

    // Grain (repeating 1px diagonal lines, overlay blend, toggleable in the
    // CSS) is deliberately not reproduced: SDL2's blend modes have no
    // "overlay" compositing mode, so this would need a pre-rendered static
    // noise texture rather than a geometry primitive — not worth it for a
    // toggleable, barely-visible detail. Flag for follow-up if it's ever
    // actually requested.
    Ok(())
}

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

    // Left: wordmark + build tag. Prefers the real logo (rasterized from
    // freeplay-frontend/assets/freeplay-wordmark.svg — a hand-drawn skewed
    // letterform with a layered red chromatic-ghost effect no font can
    // reproduce); falls back to plain text if the asset isn't next to the
    // binary, same graceful-degradation pattern as the SDL_ttf fallback.
    let (x, y) = scale.point(SIDE_PAD, HEADER_H / 2.0 - 28.0);
    let logo_h = scale.len(46.0).round().max(1.0) as u32;
    let word_w = match fonts.draw_logo(canvas, x, y, logo_h) {
        Some(w) => w,
        None => {
            fonts
                .draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(56.0), "FREEPLAY", x, y, theme::TEXT)?
                .0
        }
    };
    let (bx, by) = scale.point(SIDE_PAD + (word_w as f32 / scale.s) + 20.0, HEADER_H / 2.0 - 7.0);
    // `version::footer_string()` — version + build date + short git hash —
    // is what legacy screens show; this used to just be "BUILD {VERSION}",
    // missing the date/hash a dev build actually needs to identify itself.
    fonts.draw(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(13.0),
        &crate::version::footer_string(),
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
    geometry::fill_circle(canvas, scale, cursor_x + 8.0 + avatar_d / 2.0, HEADER_H / 2.0, avatar_d / 2.0, theme::ACCENT);
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
    geometry::fill_circle(canvas, scale, cursor_x, HEADER_H / 2.0, 4.0, dot_color);

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
        geometry::stroke_circle(canvas, scale, x + chip_d / 2.0, row_cy, chip_d / 2.0, 1.5, p.color);
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

            // "CREDIT ∞" and the "SELECT · ABOUT" hint sit to the left of the
            // FREE PLAY badge — present in the mockup's footer but dropped
            // from an earlier pass here, which only ever drew the badge.
            let mut cursor_x = badge_x - 32.0;
            let credit = "CREDIT \u{221e}";
            let credit_px = scale.font_px(13.0);
            let (cw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, credit_px, credit);
            cursor_x -= cw as f32 / scale.s;
            // `visible_span`-based centering, matching SELECT below — a flat
            // `row_cy - 7.0` offset (the old approach) doesn't land on the
            // same center line as SELECT's true glyph-height centering.
            let (c_inset, c_vis_h) = fonts.visible_span(FpFont::ChakraPetchSemiBold, credit_px, credit);
            let c_vis_h_l = c_vis_h as f32 / scale.s;
            let (crx, cry) = scale.point(cursor_x, row_cy - c_vis_h_l / 2.0 - c_inset as f32 / scale.s);
            fonts.draw(canvas, FpFont::ChakraPetchSemiBold, credit_px, credit, crx, cry, theme::DIM)?;

            cursor_x -= 28.0;
            let about = "ABOUT";
            let about_px = scale.font_px(13.0);
            let (aw, _) = fonts.text_size(FpFont::SairaCondensedSemiBold, about_px, about);
            cursor_x -= aw as f32 / scale.s;
            let (a_inset, a_vis_h) = fonts.visible_span(FpFont::SairaCondensedSemiBold, about_px, about);
            let a_vis_h_l = a_vis_h as f32 / scale.s;
            let (ax, ay) = scale.point(cursor_x, row_cy - a_vis_h_l / 2.0 - a_inset as f32 / scale.s);
            fonts.draw(canvas, FpFont::SairaCondensedSemiBold, about_px, about, ax, ay, theme::DIM)?;

            cursor_x -= 10.0;
            let select = "SELECT";
            let select_px = scale.font_px(12.0);
            let (sw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, select_px, select);
            // The pill's height/text position used to be derived from
            // `text_size` (the font's nominal ascent+descent box), which is
            // taller than "SELECT"'s actual glyphs (no descenders) — that
            // extra unused space sat below the visible text within the pill,
            // pushing it visibly lower than "ABOUT"/"CREDIT" alongside it.
            // `visible_span` centers the pill (and the text within it) on
            // the glyphs' real height instead.
            let (s_inset, s_vis_h) = fonts.visible_span(FpFont::ChakraPetchSemiBold, select_px, select);
            let s_vis_h_l = s_vis_h as f32 / scale.s;
            let pill_pad_x = 10.0;
            let pill_pad_y = 4.0;
            let pill_w = (sw as f32 / scale.s) + pill_pad_x * 2.0;
            let pill_h = s_vis_h_l + pill_pad_y * 2.0;
            cursor_x -= pill_w;
            let pill_y = row_cy - pill_h / 2.0;
            canvas.set_draw_color(Color::RGBA(255, 255, 255, 12));
            canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
            canvas.draw_rect(scale.rect(cursor_x, pill_y, pill_w, pill_h))?;
            let (slx, sly) = scale.point(cursor_x + pill_pad_x, row_cy - s_vis_h_l / 2.0 - s_inset as f32 / scale.s);
            fonts.draw(canvas, FpFont::ChakraPetchSemiBold, select_px, select, slx, sly, theme::DIM)?;
        }
        FooterRight::Text(s) => {
            let (tw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(13.0), s);
            let (tx, ty) = scale.point(theme::VW - SIDE_PAD - (tw as f32 / scale.s), row_cy - 7.0);
            fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), s, tx, ty, theme::MUTE)?;
        }
    }

    Ok(())
}
