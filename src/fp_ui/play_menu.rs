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

const ROW_H: f32 = 104.0;
const ROW_GAP: f32 = 4.0;
const BAR_W: f32 = 8.0;
const SKEW_DEG: f32 = -11.0;
const LIST_X: f32 = 56.0;
const LIST_TOP: f32 = 158.0; // header (104) + this screen's top:54 offset
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
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    let eyebrow_y = LIST_TOP - 44.0;
    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(LIST_X, eyebrow_y + 8.0, 34.0, 3.0)))?;
    let (ex, ey) = scale.point(LIST_X + 48.0, eyebrow_y);
    fonts.draw(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(14.0),
        "PLAY \u{b7} SELECT MODE",
        ex,
        ey,
        theme::ACCENT,
    )?;

    for (i, (label, sub)) in ITEMS.iter().enumerate() {
        draw_row(canvas, fonts, scale, i, label, sub, i == cursor)?;
    }

    draw_watermark(canvas, fonts, scale)?;

    // Bottom-right "MORTAL KOMBAT / ARCADE · 1993 · PLAY MODES" label.
    let mk_size = 32.0;
    let (mkw, mkh) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(mk_size), "MORTAL KOMBAT");
    let (mkx, mky) = scale.point(theme::VW - 96.0 - (mkw as f32 / scale.s), theme::VH - 96.0 - (mkh as f32 / scale.s) - 8.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(mk_size), "MORTAL KOMBAT", mkx, mky, Color::RGB(0xde, 0xde, 0xd8))?;
    let sub = "ARCADE \u{b7} 1993 \u{b7} PLAY MODES";
    let (subw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(14.0), sub);
    let (subx, suby) = scale.point(theme::VW - 96.0 - (subw as f32 / scale.s), theme::VH - 96.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(14.0), sub, subx, suby, Color::RGB(0x5e, 0x5e, 0x66))?;

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
    let y = LIST_TOP + index as f32 * (ROW_H + ROW_GAP);

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
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(17.0), &num, nx, ny, num_color)?;

    let label_color = if selected { theme::TEXT } else { Color::RGB(0x4a, 0x4a, 0x52) };
    let label_text = label.to_uppercase();
    let sub_text = sub.to_uppercase();
    let label_x = LIST_X + BAR_W + LABEL_GAP + 30.0 + LABEL_GAP;
    let label_size = 62.0;

    let label_h_px = fonts.text_size(FpFont::SairaCondensedBlack, scale.font_px(label_size), &label_text).1 as f32
        / scale.s;
    let sub_h_px = if selected {
        fonts.text_size(FpFont::SairaSemiBold, scale.font_px(15.0), &sub_text).1 as f32 / scale.s
    } else {
        0.0
    };
    let gap = if selected { 5.0 } else { 0.0 };
    let block_h = label_h_px + gap + sub_h_px;
    let block_top = y + (ROW_H - block_h) / 2.0;

    let (lx, ly) = scale.point(label_x, block_top);
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(label_size), &label_text, lx, ly, label_color)?;

    if selected {
        let (sx, sy) = scale.point(label_x, block_top + label_h_px + gap);
        fonts.draw(
            canvas,
            FpFont::SairaSemiBold,
            scale.font_px(15.0),
            &sub_text,
            sx,
            sy,
            Color::RGB(0x8a, 0x8a, 0x92),
        )?;
    }

    Ok(())
}

/// Giant near-black "PLAY" watermark at the right edge — same treatment as
/// Main Menu's "II" watermark (see `main_menu::draw_ghost_watermark`'s doc
/// for the stroke-only-text fidelity gap, which applies here too).
fn draw_watermark(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale) -> Result<(), String> {
    let (w, h) = fonts.text_size(FpFont::SairaCondensedBlack, scale.font_px(520.0), "PLAY");
    let (x, y) = scale.point(
        theme::VW - 30.0 - (w as f32 / scale.s),
        theme::VH / 2.0 - (h as f32 / scale.s) / 2.0,
    );
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(520.0), "PLAY", x, y, Color::RGB(0x0c, 0x0c, 0x11))?;
    Ok(())
}
