//! Play submenu — matches the mockup's `isPlayMenu` branch: Arcade / Lab /
//! Replays / Drones, reached from the Main Menu's "PLAY" row.
//!
//! Arcade boots straight to the ROM (no character-select step — confirmed
//! with the user: "there should be no character select" / "we boot to the
//! rom on play"). Lab, Replays and Drones delegate to their existing legacy
//! screens for now (`FpResult::ActivateMainItem`/`ActivateLabMenuItem` in
//! `super::nav`) — a native fp_ui redesign of those three is follow-up work,
//! same as the rest of the legacy-screens handoff's deferred list.

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

// Same row height/skew as `main_menu.rs` — the mockup's own CSS actually
// specifies 104px/-11deg here (vs Main Menu's 92px/-9deg), but the two
// screens sitting back-to-back in the same navigation flow with visibly
// different item sizes read as inconsistent, so this intentionally departs
// from the mockup to match Main Menu instead, per direct user feedback.
const ROW_H: f32 = 92.0;
const ROW_GAP: f32 = 4.0;
const BAR_W: f32 = 8.0;
const SKEW_DEG: f32 = -9.0;
const LIST_X: f32 = 56.0;
// Header (104) + this screen's own container `top:54` offset — the top of
// the eyebrow, not the row list. See `main_menu.rs`'s identical constants
// (same doc comment there) for how `ROWS_TOP`'s offset was measured.
const LIST_TOP: f32 = 158.0;
const ROWS_TOP: f32 = LIST_TOP + 56.0;
const LABEL_GAP: f32 = 26.0;

/// (label, sub-label) — verbatim from the mockup's `playMenuDefs`.
const ITEMS: [(&str, &str); 4] = [
    ("ARCADE", "Local versus \u{b7} 12 fighters \u{b7} 2-player"),
    ("LAB", "Training tools \u{b7} hitbox \u{b7} dummy \u{b7} punish trainer"),
    ("REPLAYS", "Watch and manage recorded match files"),
    ("DRONES", "Load ghost input streams to train against"),
];

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    cursor: usize,
    username: &str,
) -> Result<(), String> {
    chrome::draw_background_accents(canvas, scale, SKEW_DEG)?;
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    let eyebrow_y = LIST_TOP;
    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(LIST_X, eyebrow_y + 8.0, 34.0, 3.0)))?;
    let (ex, ey) = scale.point(LIST_X + 48.0, eyebrow_y);
    fonts.draw_tracked(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(14.0),
        "PLAY \u{b7} SELECT MODE",
        ex,
        ey,
        theme::ACCENT,
        scale.len(7.0).round() as i32,
    )?;

    for (i, (label, sub)) in ITEMS.iter().enumerate() {
        draw_row(canvas, fonts, scale, i, label, sub, i == cursor)?;
    }

    draw_watermark(canvas, fonts, scale)?;

    // Bottom-right "MORTAL KOMBAT / ARCADE · 1993 · PLAY MODES" label —
    // same tracked treatment as Main Menu's equivalent caption. `bottom`
    // subtracts FOOTER_H the same way Main Menu's does — this used to omit
    // it, positioning the caption low enough to collide with the footer.
    let bottom = theme::VH - chrome::FOOTER_H - 96.0;
    let mk_size = 32.0;
    let mk_track = scale.len(11.0).round() as i32;
    let (mkw, mkh) = fonts.text_size_tracked(FpFont::SairaCondensedBold, scale.font_px(mk_size), "MORTAL KOMBAT", mk_track);
    let (mkx, mky) = scale.point(theme::VW - 96.0 - (mkw as f32 / scale.s), bottom - (mkh as f32 / scale.s));
    fonts.draw_tracked(canvas, FpFont::SairaCondensedBold, scale.font_px(mk_size), "MORTAL KOMBAT", mkx, mky, Color::RGB(0xde, 0xde, 0xd8), mk_track)?;
    let sub = "ARCADE \u{b7} 1993 \u{b7} PLAY MODES";
    let sub_track = scale.len(6.0).round() as i32;
    let (subw, _) = fonts.text_size_tracked(FpFont::ChakraPetchMedium, scale.font_px(14.0), sub, sub_track);
    let (subx, suby) = scale.point(theme::VW - 96.0 - (subw as f32 / scale.s), bottom + 8.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchMedium, scale.font_px(14.0), sub, subx, suby, Color::RGB(0x5e, 0x5e, 0x66), sub_track)?;

    // Same real ROM-identity line as Main Menu's equivalent caption (ROM
    // hash + core build tag — no real arcade board revision is tracked or
    // detectable in this app, so this is real data rather than a fabricated
    // "rev" string).
    let rom_line = format!("ROM {} \u{b7} CORE {}", crate::matchmaking::rom_fnv_hash(), crate::retro::core_compat_tag());
    let (rw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(12.0), &rom_line);
    let (rx, ry) = scale.point(theme::VW - 96.0 - (rw as f32 / scale.s), bottom + 28.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(12.0), &rom_line, rx, ry, Color::RGB(0x3a, 0x3a, 0x42))?;

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::PROMPT_NAVIGATE, chrome::PROMPT_SELECT, chrome::PROMPT_BACK],
        FooterRight::Menu,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    index: usize,
    label: &str,
    sub: &str,
    selected: bool,
) -> Result<(), String> {
    let y = ROWS_TOP + index as f32 * (ROW_H + ROW_GAP);

    if selected {
        let tint = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 36);
        let clear = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 0);
        geometry::fill_horizontal_gradient_rect(canvas, scale, LIST_X, y, 730.0 * 0.62, ROW_H, tint, clear);
    }

    let bar_color = if selected {
        theme::ACCENT
    } else {
        Color::RGBA(255, 255, 255, 15)
    };
    geometry::fill_skewed_rect(canvas, scale, LIST_X, y, BAR_W, ROW_H, SKEW_DEG, bar_color);

    let num_color = if selected { theme::ACCENT } else { Color::RGB(0x34, 0x34, 0x3a) };
    let num = format!("{:02}", index + 1);
    let (nx, ny) = scale.point(LIST_X + BAR_W + LABEL_GAP, y + ROW_H / 2.0 - 8.0);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(16.0), &num, nx, ny, num_color)?;

    // Same label sizes/font/italic treatment and true visible-pixel
    // centering as `main_menu::draw_row` (see its doc comment on why
    // `visible_span`, not `size_of`, drives the vertical layout) — this
    // screen used to run a fixed 62pt non-italic label with its own
    // measured-glyph-height centering, which both looked and behaved
    // differently from the Main Menu row directly above it in the
    // navigation flow.
    let label_size = if selected { 52.0 } else { 42.0 };
    let label_color = if selected { theme::TEXT } else { Color::RGB(0x6a, 0x6a, 0x72) };
    let label_font = if selected { FpFont::SairaCondensedBlack } else { FpFont::SairaCondensedBold };
    let label_text = label.to_uppercase();
    let sub_text = sub.to_uppercase();
    let label_x = LIST_X + BAR_W + LABEL_GAP + 30.0 + LABEL_GAP;
    let label_px = scale.font_px(label_size);
    let sub_px = scale.font_px(15.0);

    let (label_inset, label_vis_h) = fonts.visible_span(label_font, label_px, &label_text);
    let label_vis_h_l = label_vis_h as f32 / scale.s;
    let gap = 14.0;
    let (sub_inset, sub_vis_h) = if selected {
        fonts.visible_span(FpFont::SairaSemiBold, sub_px, &sub_text)
    } else {
        (0, 0)
    };
    let sub_vis_h_l = sub_vis_h as f32 / scale.s;
    let block_h = if selected { label_vis_h_l + gap + sub_vis_h_l } else { label_vis_h_l };
    let block_top = y + (ROW_H - block_h) / 2.0;

    let (lx, ly) = scale.point(label_x, block_top - label_inset as f32 / scale.s);
    fonts.draw_italic(canvas, label_font, label_px, &label_text, lx, ly, label_color)?;

    if selected {
        let sub_visual_top = block_top + label_vis_h_l + gap;
        let (sx, sy) = scale.point(label_x, sub_visual_top - sub_inset as f32 / scale.s);
        fonts.draw(
            canvas,
            FpFont::SairaSemiBold,
            sub_px,
            &sub_text,
            sx,
            sy,
            Color::RGB(0x8a, 0x8a, 0x92),
        )?;

        // Selected-row chevron, same treatment as `main_menu::draw_row`'s.
        let chev_cx = LIST_X + 730.0 - 20.0;
        let chev_cy = y + ROW_H / 2.0;
        let skew = SKEW_DEG.to_radians().tan();
        let half_w = 9.0;
        let half_h = 13.0;
        let shift = |dy: f32| skew * dy;
        geometry::fill_triangle(
            canvas,
            scale,
            [
                (chev_cx - half_w + shift(-half_h), chev_cy - half_h),
                (chev_cx - half_w + shift(half_h), chev_cy + half_h),
                (chev_cx + half_w, chev_cy),
            ],
            theme::ACCENT,
        );
    }

    Ok(())
}

/// Giant near-black "PLAY" watermark at the right edge — same treatment as
/// Main Menu's "II" watermark (see `main_menu::draw_ghost_watermark`'s doc
/// for the stroke-only-text fidelity gap, which applies here too).
/// Same poor-man's-stroke treatment as `main_menu.rs`'s "II" watermark and
/// `rankings.rs`'s "#1" — this one was missing it entirely (flat fill,
/// no accent outline), the only one of the three ghost watermarks that was.
fn draw_watermark(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale) -> Result<(), String> {
    let px = scale.font_px(520.0);
    let (w, h) = fonts.text_size(FpFont::SairaCondensedBlack, px, "PLAY");
    let (x, y) = scale.point(
        theme::VW - 30.0 - (w as f32 / scale.s),
        theme::VH / 2.0 - (h as f32 / scale.s) / 2.0,
    );
    let stroke_color = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 82);
    let r = scale.len(1.0).round().max(1.0) as i32;
    for (dx, dy) in [(-r, -r), (0, -r), (r, -r), (-r, 0), (r, 0), (-r, r), (0, r), (r, r)] {
        fonts.draw(canvas, FpFont::SairaCondensedBlack, px, "PLAY", x + dx, y + dy, stroke_color)?;
    }
    fonts.draw(canvas, FpFont::SairaCondensedBlack, px, "PLAY", x, y, Color::RGB(0x0c, 0x0c, 0x11))?;
    Ok(())
}
