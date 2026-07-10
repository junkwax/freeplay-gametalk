//! Lab submenu — reached from the Play submenu's "LAB" row: a 2-item
//! chooser (Start Lab / Load Drones) matching legacy's `MenuScreen::LabMenu`.
//! Same row/eyebrow/watermark visual grammar as `play_menu.rs` (this is one
//! level deeper in the same navigation flow, so it should read as a sibling
//! screen, not a different design). Only this *chooser* is native — picking
//! "Start Lab" still hands off to legacy's `NavResult::StartLocal{lab:true}`
//! via `FpResult::ActivateLabMenuItem(0)` (the actual in-game Lab trainer
//! overlay stays legacy, per explicit user direction: "the in game UI can
//! stay for the Lab/Drones, the UI for the menu system just needs to be
//! updated"). "Load Drones" goes to the native `ghost_select.rs` screen.

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const ROW_H: f32 = 92.0;
const ROW_GAP: f32 = 4.0;
const BAR_W: f32 = 8.0;
const SKEW_DEG: f32 = -9.0;
const LIST_X: f32 = 56.0;
const LIST_TOP: f32 = 158.0;
const ROWS_TOP: f32 = LIST_TOP + 56.0;
const LABEL_GAP: f32 = 26.0;

/// (label, sub-label) — same sub-text `play_menu.rs`'s own LAB/DRONES rows
/// already use, so the copy stays consistent across both entry points.
pub const ITEMS: [(&str, &str); 2] = [
    ("START LAB", "Training tools \u{b7} hitbox \u{b7} dummy \u{b7} punish trainer"),
    ("LOAD DRONES", "Load ghost input streams to train against"),
];

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    cursor: usize,
    username: &str,
) -> Result<(), String> {
    chrome::draw_background_accents(canvas, scale)?;
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
        "LAB \u{b7} SELECT MODE",
        ex,
        ey,
        theme::ACCENT,
        scale.len(7.0).round() as i32,
    )?;

    for (i, (label, sub)) in ITEMS.iter().enumerate() {
        draw_row(canvas, fonts, scale, i, label, sub, i == cursor)?;
    }

    draw_watermark(canvas, fonts, scale)?;

    let bottom = theme::VH - chrome::FOOTER_H - 96.0;
    let mk_size = 32.0;
    let mk_track = scale.len(11.0).round() as i32;
    let (mkw, mkh) = fonts.text_size_tracked(FpFont::SairaCondensedBold, scale.font_px(mk_size), "MORTAL KOMBAT", mk_track);
    let (mkx, mky) = scale.point(theme::VW - 96.0 - (mkw as f32 / scale.s), bottom - (mkh as f32 / scale.s));
    fonts.draw_tracked(canvas, FpFont::SairaCondensedBold, scale.font_px(mk_size), "MORTAL KOMBAT", mkx, mky, Color::RGB(0xde, 0xde, 0xd8), mk_track)?;
    let sub = "ARCADE \u{b7} 1993 \u{b7} LAB MODE";
    let sub_track = scale.len(6.0).round() as i32;
    let (subw, _) = fonts.text_size_tracked(FpFont::ChakraPetchMedium, scale.font_px(14.0), sub, sub_track);
    let (subx, suby) = scale.point(theme::VW - 96.0 - (subw as f32 / scale.s), bottom + 8.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchMedium, scale.font_px(14.0), sub, subx, suby, Color::RGB(0x5e, 0x5e, 0x66), sub_track)?;

    // ROM identity line hidden for now, same as Main Menu's — see its
    // doc comment.

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

/// Giant near-black "LAB" watermark, same treatment as Main Menu's "II" and
/// the Play submenu's "PLAY".
fn draw_watermark(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale) -> Result<(), String> {
    let px = scale.font_px(520.0);
    let (w, h) = fonts.text_size(FpFont::SairaCondensedBlack, px, "LAB");
    let (x, y) = scale.point(
        theme::VW - 30.0 - (w as f32 / scale.s),
        theme::VH / 2.0 - (h as f32 / scale.s) / 2.0,
    );
    let stroke_color = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 82);
    let r = scale.len(1.0).round().max(1.0) as i32;
    for (dx, dy) in [(-r, -r), (0, -r), (r, -r), (-r, 0), (r, 0), (-r, r), (0, r), (r, r)] {
        fonts.draw_italic(canvas, FpFont::SairaCondensedBlack, px, "LAB", x + dx, y + dy, stroke_color)?;
    }
    fonts.draw_italic(canvas, FpFont::SairaCondensedBlack, px, "LAB", x, y, Color::RGB(0x0c, 0x0c, 0x11))?;
    Ok(())
}
