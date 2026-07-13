//! About — matches the mockup's `isAbout` branch: build info + a
//! keybindings reference table. Content is real (`crate::version`, and the
//! actual fp_ui/lab bindings) rather than the mockup's placeholder
//! "0.9.4-alpha" text.

use super::chrome::{self, FooterRight};
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const SIDE_PAD: f32 = 56.0;
const TOP: f32 = 42.0 + 104.0;

const MENU_KEYS: [(&str, &str); 6] = [
    ("Navigate", "D-PAD"),
    ("Select / Confirm", "CROSS"),
    ("Back / Cancel", "CIRCLE"),
    ("Previous tab", "L1"),
    ("Next tab", "R1"),
    ("About", "SELECT"),
];

// Must stay in step with the `Event::KeyDown` lab arms in main.rs and the
// in-lab hotkey overlay (`render::draw_lab_assist_overlay`).
const LAB_KEYS: [(&str, &str); 11] = [
    ("Hitbox overlay", "F2"),
    ("Infinite health (both)", "F3"),
    ("Freeze timer", "F4"),
    ("Dummy behavior / record", "F5"),
    ("Load reset slot", "F6"),
    ("Save reset slot", "F7"),
    ("Load drone", "F8"),
    ("Save drone", "F9"),
    ("Punish trainer", "F10"),
    ("Hide lab overlay", "F11"),
    ("Play vs. drone", "F12"),
];

pub fn draw(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, username: &str) -> Result<(), String> {
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    let (ex, ey) = scale.point(SIDE_PAD + 44.0, TOP);
    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, TOP + 8.0, 30.0, 3.0)))?;
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "ABOUT", ex, ey, theme::ACCENT)?;

    let title_y = TOP + 26.0;
    let (title_x, title_yw) = scale.point(SIDE_PAD, title_y);
    let (_, wordmark_h) = fonts.draw(
        canvas,
        FpFont::SairaCondensedBlack,
        scale.font_px(72.0),
        "FREE",
        title_x,
        title_yw,
        theme::TEXT,
    )?;
    let (fx, fy) = scale.point(SIDE_PAD, title_y + (wordmark_h as f32 / scale.s) + 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(72.0), "PLAY", fx, fy, theme::TEXT)?;

    let sub_y = title_y + (wordmark_h as f32 / scale.s) * 2.0 + 14.0;
    let (subx, suby) = scale.point(SIDE_PAD, sub_y);
    fonts.draw(
        canvas,
        FpFont::ChakraPetchMedium,
        scale.font_px(13.0),
        "ARCADE NETPLAY",
        subx,
        suby,
        Color::RGB(0x52, 0x52, 0x5a),
    )?;

    let build_rows: [(&str, String); 4] = [
        ("VERSION", crate::version::VERSION.to_string()),
        ("BUILD", crate::version::BUILD_DATE.to_string()),
        ("ENGINE", "SDL2 / Rust".to_string()),
        ("NETPLAY", "GGRS ROLLBACK \u{b7} P2P".to_string()),
    ];
    let mut y = sub_y + 46.0;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, 400.0, 1.0)))?;
    for (key, val) in &build_rows {
        y += 14.0;
        let (kx, ky) = scale.point(SIDE_PAD, y);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), key, kx, ky, Color::RGB(0x3a, 0x3a, 0x42))?;
        let (vw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(14.0), val);
        let (vx, vy) = scale.point(SIDE_PAD + 400.0 - (vw as f32 / scale.s), y - 2.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(14.0), val, vx, vy, Color::RGB(0xcf, 0xcf, 0xc9))?;
        y += 20.0;
        canvas.set_draw_color(Color::RGBA(255, 255, 255, 10));
        canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, 400.0, 1.0)))?;
    }

    let col_x = SIDE_PAD + 456.0;
    draw_key_column(canvas, fonts, scale, col_x, TOP + 20.0, "MENU NAVIGATION", &MENU_KEYS)?;
    draw_key_column(canvas, fonts, scale, col_x + 460.0, TOP + 20.0, "IN-LAB SHORTCUTS", &LAB_KEYS)?;

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::PROMPT_BACK],
        FooterRight::Text(&format!("v{} \u{b7} {}", crate::version::VERSION, crate::version::BUILD_DATE)),
    )?;
    Ok(())
}

fn draw_key_column(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    top: f32,
    heading: &str,
    rows: &[(&str, &str)],
) -> Result<(), String> {
    let (hx, hy) = scale.point(x, top);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), heading, hx, hy, Color::RGB(0x5a, 0x5a, 0x62))?;

    let mut y = top + 30.0;
    let row_h = 40.0;
    for (action, key) in rows {
        let (ax, ay) = scale.point(x, y);
        fonts.draw(canvas, FpFont::SairaMedium, scale.font_px(16.0), action, ax, ay, Color::RGB(0x9a, 0x9a, 0xa2))?;

        let (kw, kh) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(12.0), key);
        let pad_x = 10.0;
        let pad_y = 4.0;
        let chip_w = (kw as f32 / scale.s) + pad_x * 2.0;
        let chip_h = (kh as f32 / scale.s) + pad_y * 2.0;
        let chip_x = x + 300.0 - chip_w;
        canvas.set_draw_color(Color::RGBA(255, 255, 255, 12));
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        canvas.draw_rect(scale.rect(chip_x, y - pad_y, chip_w, chip_h))?;
        let (cx, cy) = scale.point(chip_x + pad_x, y - pad_y + pad_y - 1.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(12.0), key, cx, cy, Color::RGB(0xcf, 0xcf, 0xc9))?;

        canvas.set_draw_color(Color::RGBA(255, 255, 255, 10));
        canvas.fill_rect(Some(scale.rect(x, y + row_h - 10.0, 300.0, 1.0)))?;
        y += row_h;
    }
    Ok(())
}
