//! Automated matchmaking search — native redesign of legacy's
//! `MenuScreen::Matchmaking` (`crate::menu::draw_matchmaking`), used by every
//! matchmaking trigger *except* Quick Match (Send/Accept Challenge, Discord
//! connect, joining a spar room, a lobby match starting) — those hand off
//! here the same way legacy hands off to its own Matchmaking screen, since
//! there's no natural "tab to stay on" for them the way there is for Quick
//! Match. Quick Match itself now stays on `FpScreen::Lobby` and shows this
//! same radar animation inline — see `lobby.rs`'s `draw_quick_match`, which
//! calls `draw_radar` (below) directly rather than duplicating it.
//!
//! `draw_radar`'s animation is transcribed from `FREEPLAY Arcade.dc.html`'s
//! `isQuick` branch: two static rings, a double pulse ring expanding
//! outward and fading (`@keyframes fp-radar`, 2.8s, second ring offset
//! 1.4s), and a ~20deg wedge rotating linearly once per cycle
//! (`@keyframes fp-sweep`, conic-gradient in CSS, drawn here as a thin fan
//! of triangles since SDL2 has no conic-gradient primitive). `status`
//! carries the same human-readable progress string the legacy screen
//! shows, updated by the same background-thread round trip in `main.rs`.
//! Cancellation is a raw `is_cancel(&event)` check in `main.rs`
//! (`is_matchmaking_screen`), same as legacy — this screen's own `nav()`
//! arm in `mod.rs` is a no-op.

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const RADAR_R: f32 = 170.0;

pub(super) fn elapsed_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Same 4-frame "..."/"."/".."/"" cycle legacy's `draw_matchmaking` uses, so
/// the search doesn't read as stalled when the status string itself hasn't
/// changed in a while.
pub(super) fn dots() -> &'static str {
    match (elapsed_ms() / 500) % 4 {
        0 => "",
        1 => ".",
        2 => "..",
        _ => "...",
    }
}

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    status: &str,
    username: &str,
) -> Result<(), String> {
    chrome::draw_background_accents_no_glow(canvas, scale)?;
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    let cx = theme::VW / 2.0;
    let cy = theme::VH / 2.0 - 60.0;
    draw_radar(canvas, scale, cx, cy, RADAR_R)?;
    geometry::fill_circle(canvas, scale, cx, cy, 37.0, Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 230));
    let (vw, vh) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(26.0), "VS");
    let (vx, vy) = scale.point(cx - (vw as f32 / scale.s) / 2.0, cy - (vh as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(26.0), "VS", vx, vy, Color::RGB(255, 255, 255))?;

    let title_px = scale.font_px(15.0);
    let (tw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, title_px, "FIND MATCH");
    let (ttx, tty) = scale.point(cx - (tw as f32 / scale.s) / 2.0, 96.0);
    fonts.draw_tracked(
        canvas,
        FpFont::ChakraPetchSemiBold,
        title_px,
        "FIND MATCH",
        ttx,
        tty,
        theme::ACCENT,
        scale.len(4.0).round() as i32,
    )?;

    let line = format!("{status}{}", dots());
    let status_px = scale.font_px(28.0);
    let (lw, _) = fonts.text_size(FpFont::SairaCondensedBold, status_px, &line);
    let status_y = cy + RADAR_R + 46.0;
    let (lx, ly) = scale.point(cx - (lw as f32 / scale.s) / 2.0, status_y);
    fonts.draw(canvas, FpFont::SairaCondensedBold, status_px, &line, lx, ly, theme::WARNING)?;

    let status_lower = status.to_ascii_lowercase();
    let hint = if status_lower.starts_with("checking name") {
        "Verifying your player name before entering the queue"
    } else {
        "Using your confirmed player name for online play"
    };
    let hint_px = scale.font_px(15.0);
    let (hw, _) = fonts.text_size(FpFont::SairaMedium, hint_px, hint);
    let (hx, hy) = scale.point(cx - (hw as f32 / scale.s) / 2.0, status_y + 40.0);
    fonts.draw(canvas, FpFont::SairaMedium, hint_px, hint, hx, hy, theme::DIM)?;

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::PROMPT_BACK],
        FooterRight::Text("F11 NETWORK STATS"),
    )?;
    Ok(())
}

/// The radar rings + sweep alone, with no center badge — shared by this
/// screen's own `draw` (above) and `lobby.rs`'s inline Quick Match "searching"
/// state, each of which draws its own center content on top.
pub(super) fn draw_radar(canvas: &mut Canvas<Window>, scale: &Scale, cx: f32, cy: f32, radius: f32) -> Result<(), String> {
    // Two static rings: outer at the full radius, inner at 190/300 of it —
    // matches the CSS's 300px/190px pair.
    geometry::stroke_circle(canvas, scale, cx, cy, radius, 1.0, Color::RGBA(255, 255, 255, 15));
    geometry::stroke_circle(canvas, scale, cx, cy, radius * 190.0 / 300.0, 1.0, Color::RGBA(255, 255, 255, 15));

    // Double pulse ring: each expands from 0.25x to 1.55x the radius while
    // fading from ~65% to 0% opacity over a 2.8s cycle, the second offset by
    // half a cycle (1.4s) so a new pulse starts as the first fades out —
    // `@keyframes fp-radar` in the mockup.
    //
    // The modulo must happen in *integer* space before the f32 conversion:
    // `elapsed_ms()` is Unix-epoch millis (~1.8e12), far past f32's 24-bit
    // mantissa — `as f32` first quantizes it to ~131s steps, freezing the
    // animation on any human timescale (the bug this comment replaces).
    let cycle_ms: u128 = 2800;
    let cycle_t = (elapsed_ms() % cycle_ms) as f32 / cycle_ms as f32;
    for phase in [0.0f32, 0.5] {
        let t = (cycle_t + phase) % 1.0;
        let scale_factor = 0.25 + t * (1.55 - 0.25);
        let alpha = (0.65 * (1.0 - t) * 255.0) as u8;
        geometry::stroke_circle(
            canvas,
            scale,
            cx,
            cy,
            radius * scale_factor,
            1.5,
            Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, alpha),
        );
    }

    // Outer boundary ring, brighter than the static grid rings.
    geometry::stroke_circle(canvas, scale, cx, cy, radius, 2.0, Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 90));

    // Rotating ~20deg wedge (conic-gradient's 55deg-75deg visible band in
    // the CSS) — `@keyframes fp-sweep`, one linear revolution per 2.8s.
    let angle = cycle_t * std::f32::consts::TAU;
    let wedge_span = 20.0_f32.to_radians();
    let steps = 10;
    for i in 0..steps {
        let a = angle + (i as f32 / steps as f32) * wedge_span;
        let a_next = angle + ((i + 1) as f32 / steps as f32) * wedge_span;
        geometry::fill_triangle(
            canvas,
            scale,
            [
                (cx, cy),
                (cx + a.cos() * radius, cy + a.sin() * radius),
                (cx + a_next.cos() * radius, cy + a_next.sin() * radius),
            ],
            Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 130),
        );
    }
    Ok(())
}
