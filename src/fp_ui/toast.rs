//! Native toast notification — replaces the legacy bitmap-font
//! `menu::draw_toast` overlay whenever the frame underneath is an fp_ui
//! screen (legacy screens keep the legacy toast). Same payload and timing
//! as legacy (`menu::Toast`'s message + remaining_ms, set by the exact
//! same `toast` variable in main.rs); only the rendering differs: a dark
//! card with the skewed accent edge bar from the design language,
//! bottom-centered above the footer, fading out over its final ~300ms the
//! way the legacy one does.

use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const CARD_H: f32 = 54.0;
const PAD_X: f32 = 26.0;
const BAR_W: f32 = 8.0;
const FADE_MS: u128 = 300;
/// Sits just above the footer chrome (86 logical px tall).
const BOTTOM_MARGIN: f32 = 86.0 + 22.0;

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    message: &str,
    remaining_ms: u128,
) -> Result<(), String> {
    let fade = (remaining_ms.min(FADE_MS) as f32 / FADE_MS as f32).clamp(0.0, 1.0);
    let a = |base: u8| (base as f32 * fade) as u8;

    let px = scale.font_px(19.0);
    let (tw, th) = fonts.text_size(FpFont::SairaCondensedSemiBold, px, message);
    let text_w = tw as f32 / scale.s;
    let card_w = BAR_W + PAD_X * 2.0 + text_w;
    let card_x = (theme::VW - card_w) / 2.0;
    let card_y = theme::VH - BOTTOM_MARGIN - CARD_H;

    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(14, 14, 18, a(240)));
    canvas.fill_rect(Some(scale.rect(card_x, card_y, card_w, CARD_H)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, a(26)));
    canvas.draw_rect(scale.rect(card_x, card_y, card_w, CARD_H))?;
    geometry::fill_skewed_rect(
        canvas,
        scale,
        card_x + 6.0,
        card_y + 8.0,
        BAR_W,
        CARD_H - 16.0,
        -11.0,
        Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, a(255)),
    );

    let (tx, ty) = scale.point(
        card_x + BAR_W + PAD_X,
        card_y + CARD_H / 2.0 - (th as f32 / scale.s) / 2.0,
    );
    fonts.draw(
        canvas,
        FpFont::SairaCondensedSemiBold,
        px,
        message,
        tx,
        ty,
        Color::RGBA(0xf2, 0xf2, 0xee, a(255)),
    )?;
    Ok(())
}
