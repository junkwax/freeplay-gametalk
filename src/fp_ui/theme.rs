//! Design tokens for the fp_ui screen set — colors, the 1920x1080 logical
//! canvas size, and the skew angle used throughout the mockup. Values are
//! transcribed directly from `FREEPLAY Arcade.dc.html`'s computed styles
//! (see the handoff package), not eyeballed from screenshots.
//!
//! Most tokens are unused until later steps consume them (Main Menu, Quit,
//! Settings, Lobby, Play) — allowed here rather than deleted and re-added.
#![allow(dead_code)]

use sdl2::pixels::Color;

/// Logical canvas width/height. All fp_ui layout math happens in this space;
/// `layout::Scale` converts to actual window pixels at draw time.
pub const VW: f32 = 1920.0;
pub const VH: f32 = 1080.0;

/// skewX(-11deg), used for menu accent bars, lobby tabs, and other angled
/// panels throughout the mockup.
pub const SKEW_DEG: f32 = -11.0;

pub const ACCENT: Color = Color::RGB(0xE2, 0x2A, 0x35);
pub const BG: Color = Color::RGB(0x06, 0x06, 0x08);
pub const SURFACE: Color = Color::RGB(0x0e, 0x0e, 0x12);
pub const SURFACE2: Color = Color::RGB(0x14, 0x14, 0x1a);
pub const BORDER: Color = Color::RGBA(0xff, 0xff, 0xff, 23); // rgba(255,255,255,.09)
pub const TEXT: Color = Color::RGB(0xF4, 0xF4, 0xF0);
pub const DIM: Color = Color::RGB(0x9a, 0x9a, 0xa2);
pub const MUTE: Color = Color::RGB(0x62, 0x62, 0x6c);
pub const INACTIVE: Color = Color::RGB(0x4a, 0x4a, 0x52);
pub const GREEN: Color = Color::RGB(0x36, 0xD3, 0x99);
pub const WARNING: Color = Color::RGB(0xE2, 0xB5, 0x3A);
pub const HIGH_PING: Color = Color::RGB(0xE2, 0x60, 0x3A);
pub const PURPLE: Color = Color::RGB(0x7C, 0x4D, 0xFF);

pub const BTN_CROSS: Color = Color::RGB(0x7F, 0xA7, 0xFF);
pub const BTN_CIRCLE: Color = Color::RGB(0xFF, 0x6B, 0x7A);
pub const BTN_TRIANGLE: Color = Color::RGB(0x4C, 0xE0, 0xB3);
pub const BTN_SQUARE: Color = Color::RGB(0xE8, 0x79, 0xC7);

/// Ping color thresholds from the handoff doc (`< 50` green, `50-99` amber,
/// `>= 100` orange-red).
pub fn ping_color(ms: u32) -> Color {
    if ms < 50 {
        GREEN
    } else if ms < 100 {
        WARNING
    } else {
        HIGH_PING
    }
}
