//! Lab submenu — reached from the Play submenu's "LAB" row: a 2-item
//! chooser (Start Lab / Load Drones) matching legacy's `MenuScreen::LabMenu`.
//! Only this *chooser* is native — picking "Start Lab" still hands off to
//! legacy's `NavResult::StartLocal{lab:true}` via
//! `FpResult::ActivateLabMenuItem(0)` (the actual in-game Lab trainer
//! overlay stays legacy, per explicit user direction: "the in game UI can
//! stay for the Lab/Drones, the UI for the menu system just needs to be
//! updated"). "Load Drones" goes to the native `ghost_select.rs` screen.
//!
//! Layout matches the current mockup's `isLab` branch: a narrow 480px-wide
//! chooser column plus an "IN-LAB SHORTCUT KEYS" reference panel — not the
//! full-width single-column-with-giant-watermark treatment `play_menu.rs`
//! uses one level up. An earlier pass here copied `play_menu.rs`'s
//! full-width/watermark layout verbatim (including its `top:54` offset,
//! rather than this screen's own `padding:38px`), which was a drift from
//! the mockup this rebuild corrects. `labToolDefs` is static reference
//! text (F-key bindings), same "no backend to fetch, so it's fine as
//! static content" reasoning as Network News.

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const SIDE_PAD: f32 = 56.0;
const TOP: f32 = 38.0 + 104.0;
const LIST_W: f32 = 480.0;
const ROW_H: f32 = 92.0;
const ROW_GAP: f32 = 4.0;
const BAR_W: f32 = 8.0;
const SKEW_DEG: f32 = -9.0;
const LABEL_GAP: f32 = 26.0;

/// (label, sub-label) — same sub-text `play_menu.rs`'s own LAB row already
/// uses, so the copy stays consistent across both entry points.
pub const ITEMS: [(&str, &str); 2] = [
    ("START LAB", "Launch training mode with all tools live"),
    ("LOAD DRONES", "Open ghost browser to practice against recorded opponents"),
];

/// (key, label, hint) — F-key shortcuts available inside the in-game Lab
/// trainer overlay (legacy-rendered; see this module's doc comment). Static
/// reference text, not read from any live binding table — the Lab overlay's
/// F-key assignments are hardcoded in `main.rs`, not user-rebindable.
const TOOLS: [(&str, &str, &str); 8] = [
    ("F2", "Hitbox Overlay", "Show collision boxes"),
    ("F3", "Infinite Health P1", "Toggle on / off"),
    ("F4", "Infinite Health P2", "Toggle on / off"),
    ("F5", "Freeze Timer", "Pause match clock"),
    ("F6", "Dummy Behavior", "CPU / crouch / jump / block"),
    ("F7", "Punish Trainer", "Mark unsafe moves on block"),
    ("F8", "Save RAM State", "Snapshot current frame"),
    ("F9", "Load RAM State / Record Ghost", "Restore snapshot or record ghost"),
];

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    cursor: usize,
    username: &str,
) -> Result<(), String> {
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, TOP + 8.0, 30.0, 3.0)))?;
    let (ex, ey) = scale.point(SIDE_PAD + 44.0, TOP);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "TRAINING MODE", ex, ey, theme::ACCENT)?;
    let (tx, ty) = scale.point(SIDE_PAD, TOP + 26.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(58.0), "LAB", tx, ty, theme::TEXT)?;

    let body_top = TOP + 26.0 + 70.0;
    for (i, (label, sub)) in ITEMS.iter().enumerate() {
        draw_row(canvas, fonts, scale, body_top, i, label, sub, i == cursor)?;
    }

    let panel_x = SIDE_PAD + LIST_W + 48.0;
    let panel_w = theme::VW - SIDE_PAD - panel_x;
    let panel_h = 620.0;
    draw_shortcut_panel(canvas, fonts, scale, panel_x, body_top, panel_w, panel_h)?;

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::PROMPT_NAVIGATE, chrome::PROMPT_SELECT, chrome::PROMPT_BACK],
        FooterRight::Text("TRAINING MODE \u{b7} ALL TOOLS ACTIVE"),
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    body_top: f32,
    index: usize,
    label: &str,
    sub: &str,
    selected: bool,
) -> Result<(), String> {
    let y = body_top + index as f32 * (ROW_H + ROW_GAP);

    if selected {
        let tint = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 36);
        let clear = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 0);
        geometry::fill_horizontal_gradient_rect(canvas, scale, SIDE_PAD, y, LIST_W, ROW_H, tint, clear);
    }

    let bar_color = if selected {
        theme::ACCENT
    } else {
        Color::RGBA(255, 255, 255, 15)
    };
    geometry::fill_skewed_rect(canvas, scale, SIDE_PAD, y, BAR_W, ROW_H, SKEW_DEG, bar_color);

    let num_color = if selected { theme::ACCENT } else { Color::RGB(0x34, 0x34, 0x3a) };
    let num = format!("{:02}", index + 1);
    let num_px = scale.font_px(16.0);
    // True visible-glyph centering (`visible_span`), not a hand-tuned
    // "-8.0" offset — matches the same technique the label below already
    // uses, so the number lands on the same center line as the label
    // instead of independently guessed.
    let (num_inset, num_vis_h) = fonts.visible_span(FpFont::ChakraPetchSemiBold, num_px, &num);
    let num_top = y + ROW_H / 2.0 - (num_vis_h as f32 / scale.s) / 2.0;
    let (nx, ny) = scale.point(SIDE_PAD + BAR_W + LABEL_GAP, num_top - num_inset as f32 / scale.s);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, num_px, &num, nx, ny, num_color)?;

    let label_color = if selected { theme::TEXT } else { Color::RGB(0x6a, 0x6a, 0x72) };
    let label_font = if selected { FpFont::SairaCondensedBlack } else { FpFont::SairaCondensedBold };
    let label_text = label.to_uppercase();
    let label_x = SIDE_PAD + BAR_W + LABEL_GAP + 30.0 + LABEL_GAP;
    let label_px = scale.font_px(46.0);
    let sub_px = scale.font_px(14.0);

    let (label_inset, label_vis_h) = fonts.visible_span(label_font, label_px, &label_text);
    let label_vis_h_l = label_vis_h as f32 / scale.s;
    let gap = 8.0;
    let (sub_inset, sub_vis_h) = fonts.visible_span(FpFont::SairaMedium, sub_px, sub);
    let sub_vis_h_l = sub_vis_h as f32 / scale.s;
    let block_h = label_vis_h_l + gap + sub_vis_h_l;
    let block_top = y + (ROW_H - block_h) / 2.0;

    let (lx, ly) = scale.point(label_x, block_top - label_inset as f32 / scale.s);
    fonts.draw_italic(canvas, label_font, label_px, &label_text, lx, ly, label_color)?;

    let sub_visual_top = block_top + label_vis_h_l + gap;
    let (sx, sy) = scale.point(label_x, sub_visual_top - sub_inset as f32 / scale.s);
    let sub_color = if selected { Color::RGB(0x8a, 0x8a, 0x92) } else { Color::RGB(0x52, 0x52, 0x5a) };
    fonts.draw(canvas, FpFont::SairaMedium, sub_px, sub, sx, sy, sub_color)?;

    if selected {
        let chev_cx = SIDE_PAD + LIST_W - 20.0;
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

fn draw_shortcut_panel(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(x, y, w, h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(x, y, w, h))?;

    let pad = 30.0;
    let (hx, hy) = scale.point(x + pad, y + 22.0);
    fonts.draw_tracked(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(11.0),
        "IN-LAB SHORTCUT KEYS",
        hx,
        hy,
        Color::RGB(0x3a, 0x3a, 0x42),
        scale.len(4.0).round() as i32,
    )?;

    let grid_top = y + 22.0 + 34.0;
    let col_w = (w - pad * 2.0) / 2.0;
    let row_h = 60.0;
    for (i, (key, label, hint)) in TOOLS.iter().enumerate() {
        let col = i / 4;
        let row = i % 4;
        let cx = x + pad + col as f32 * col_w;
        let cy = grid_top + row as f32 * row_h;

        canvas.set_draw_color(Color::RGBA(255, 255, 255, 10));
        canvas.fill_rect(Some(scale.rect(cx, cy + row_h - 1.0, col_w - 14.0, 1.0)))?;

        let badge_w = 46.0;
        let badge_h = 26.0;
        canvas.set_draw_color(Color::RGBA(255, 255, 255, 36));
        canvas.draw_rect(scale.rect(cx, cy + row_h / 2.0 - badge_h / 2.0, badge_w, badge_h))?;
        let (kw, kh) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(12.0), key);
        let (kx, ky) = scale.point(
            cx + badge_w / 2.0 - (kw as f32 / scale.s) / 2.0,
            cy + row_h / 2.0 - (kh as f32 / scale.s) / 2.0,
        );
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(12.0), key, kx, ky, Color::RGB(0x8a, 0x8a, 0x92))?;

        let text_x = cx + badge_w + 14.0;
        let (labx, laby) = scale.point(text_x, cy + row_h / 2.0 - 18.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(16.0), label, labx, laby, Color::RGB(0xED, 0xED, 0xE8))?;
        let (hintx, hinty) = scale.point(text_x, cy + row_h / 2.0 + 2.0);
        fonts.draw(canvas, FpFont::SairaMedium, scale.font_px(12.0), hint, hintx, hinty, Color::RGB(0x52, 0x52, 0x5a))?;
    }
    Ok(())
}
