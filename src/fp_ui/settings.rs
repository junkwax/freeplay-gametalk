//! Settings — matches `screenshots/04-settings.png`'s row language (sidebar
//! categories, slider/pill rows), bound to real `Config` fields.
//!
//! The mockup's sidebar has 4 categories (Controls/Video/Audio/Netplay);
//! this implements 3 (Video/Audio/Netplay), all bound to real, persisted
//! settings. Controls is left out deliberately: the legacy-screens handoff
//! doc flags "does Controls become a Settings category, or stay standalone"
//! as an open product question, and Controls already has its own working
//! destination from the Main Menu — folding it in here would be answering
//! that question by fiat rather than flagging it.
//!
//! Category switching uses L1/R1 (`FpNav::PrevTab`/`NextTab`), matching the
//! convention the handoff doc specifies for Lobby tabs — the doc's own
//! summary table for Settings ("up/down: cat cycle, left/right: adjust")
//! doesn't leave room for navigating *between* individual rows within a
//! category, which the mockup's actual per-row `selected` state clearly
//! needs. Up/Down moves the row cursor within the category instead, which
//! is the reading that makes the mockup's own interaction model consistent.

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::config::{AspectMode, AudioBuffer, RenderProfile, ScorebarStyle, VideoFilter};
use crate::font::{FpFont, FpFontCache};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

pub const CATS: [&str; 3] = ["VIDEO", "AUDIO", "NETPLAY"];
const ROWS_PER_CAT: [usize; 3] = [6, 2, 4];

const SIDE_PAD: f32 = 56.0;
const CONTENT_TOP: f32 = 142.0;
const CAT_ROW_H: f32 = 60.0;
const CAT_ROW_GAP: f32 = 4.0;
const SIDEBAR_W: f32 = 268.0;
const PANEL_GAP: f32 = 44.0;
const ROW_H: f32 = 72.0;

pub fn rows_in_cat(cat: usize) -> usize {
    ROWS_PER_CAT.get(cat).copied().unwrap_or(1)
}

/// Everything a settings row needs to render, resolved for the current
/// field values — kept separate from the `FpScreen::Settings` storage
/// fields so drawing doesn't need to match on `(cat, row)` twice.
enum RowValue {
    Toggle(bool),
    Cycle(&'static str),
    Slider { pct: f32, text: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsFields {
    pub fullscreen: bool,
    pub render_profile: RenderProfile,
    pub video_filter: VideoFilter,
    pub crt_corner_bend: bool,
    pub aspect_mode: AspectMode,
    pub scorebar_style: ScorebarStyle,
    pub volume_percent: u8,
    pub audio_buffer: AudioBuffer,
    pub input_delay: u32,
    pub runahead: bool,
    pub runahead_online: bool,
    pub discord_rpc_enabled: bool,
}

impl SettingsFields {
    pub fn from_cfg(cfg: &crate::config::Config) -> Self {
        Self {
            fullscreen: cfg.fullscreen,
            render_profile: cfg.render_profile,
            video_filter: cfg.video_filter,
            crt_corner_bend: cfg.crt_corner_bend,
            aspect_mode: cfg.aspect_mode,
            scorebar_style: cfg.scorebar_style,
            volume_percent: cfg.volume_percent,
            audio_buffer: cfg.audio_buffer,
            input_delay: cfg.input_delay,
            runahead: cfg.runahead,
            runahead_online: cfg.runahead_online,
            discord_rpc_enabled: cfg.discord_rpc_enabled,
        }
    }

    fn row_meta(cat: usize, row: usize) -> (&'static str, &'static str) {
        match (cat, row) {
            (0, 0) => ("FULLSCREEN", "Borderless desktop fullscreen"),
            (0, 1) => ("RENDER PROFILE", "SDL renderer backend"),
            (0, 2) => ("VIDEO FILTER", "Gameplay frame presentation"),
            (0, 3) => ("CRT CORNER BEND", "Rounded glass shading on CRT filters"),
            (0, 4) => ("ASPECT MODE", "How the frame fits the window"),
            (0, 5) => ("SCOREBAR STYLE", "Netplay score overlay layout"),
            (1, 0) => ("VOLUME", "Output level"),
            (1, 1) => ("AUDIO BUFFER", "Queue depth vs. latency"),
            (2, 0) => ("INPUT DELAY", "Frames of delay before rollback"),
            (2, 1) => ("RUNAHEAD (OFFLINE)", "One-frame speculative local play"),
            (2, 2) => ("RUNAHEAD (ONLINE)", "Experimental video-only netplay speculation"),
            (2, 3) => ("DISCORD RICH PRESENCE", "Show match status in Discord"),
            _ => ("", ""),
        }
    }

    fn value(&self, cat: usize, row: usize) -> RowValue {
        match (cat, row) {
            (0, 0) => RowValue::Toggle(self.fullscreen),
            (0, 1) => RowValue::Cycle(self.render_profile.label()),
            (0, 2) => RowValue::Cycle(self.video_filter.label()),
            (0, 3) => RowValue::Toggle(self.crt_corner_bend),
            (0, 4) => RowValue::Cycle(self.aspect_mode.label()),
            (0, 5) => RowValue::Cycle(self.scorebar_style.label()),
            (1, 0) => RowValue::Slider { pct: self.volume_percent as f32, text: format!("{}%", self.volume_percent) },
            (1, 1) => RowValue::Cycle(self.audio_buffer.label()),
            (2, 0) => RowValue::Slider {
                pct: self.input_delay as f32 / 8.0 * 100.0,
                text: format!("{}f", self.input_delay),
            },
            (2, 1) => RowValue::Toggle(self.runahead),
            (2, 2) => RowValue::Toggle(self.runahead_online),
            (2, 3) => RowValue::Toggle(self.discord_rpc_enabled),
            _ => RowValue::Toggle(false),
        }
    }

    /// Mutate the field at `(cat, row)` by `delta` (±1). Toggles ignore the
    /// sign and flip.
    pub fn adjust(&mut self, cat: usize, row: usize, delta: i8) {
        match (cat, row) {
            (0, 0) => self.fullscreen = !self.fullscreen,
            (0, 1) => self.render_profile = self.render_profile.cycle(delta),
            (0, 2) => self.video_filter = self.video_filter.cycle(delta),
            (0, 3) => self.crt_corner_bend = !self.crt_corner_bend,
            (0, 4) => self.aspect_mode = self.aspect_mode.cycle(delta),
            (0, 5) => self.scorebar_style = self.scorebar_style.cycle(delta),
            (1, 0) => {
                self.volume_percent = (self.volume_percent as i32 + delta as i32 * 5).clamp(0, 100) as u8
            }
            (1, 1) => self.audio_buffer = self.audio_buffer.cycle(delta),
            (2, 0) => self.input_delay = (self.input_delay as i32 + delta as i32).clamp(0, 8) as u32,
            (2, 1) => self.runahead = !self.runahead,
            (2, 2) => self.runahead_online = !self.runahead_online,
            (2, 3) => self.discord_rpc_enabled = !self.discord_rpc_enabled,
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    fields: &SettingsFields,
    cat: usize,
    row: usize,
    username: &str,
) -> Result<(), String> {
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    let (ex, ey) = scale.point(SIDE_PAD + 44.0, CONTENT_TOP);
    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, CONTENT_TOP + 8.0, 30.0, 3.0)))?;
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "CONFIGURATION", ex, ey, theme::ACCENT)?;
    let (tx, ty) = scale.point(SIDE_PAD, CONTENT_TOP + 26.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(58.0), "SETTINGS", tx, ty, theme::TEXT)?;

    let list_top = CONTENT_TOP + 26.0 + 70.0;
    for (i, label) in CATS.iter().enumerate() {
        draw_cat_row(canvas, fonts, scale, i, label, list_top, i == cat)?;
    }
    draw_cabinet_box(canvas, fonts, scale, list_top + CATS.len() as f32 * (CAT_ROW_H + CAT_ROW_GAP) + 24.0)?;

    let panel_x = SIDE_PAD + SIDEBAR_W + PANEL_GAP;
    let (cnx, cny) = scale.point(panel_x, list_top);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(24.0), CATS[cat], cnx, cny, Color::RGB(0x8a, 0x8a, 0x92))?;

    let rows_top = list_top + 44.0;
    for r in 0..rows_in_cat(cat) {
        let (label, hint) = SettingsFields::row_meta(cat, r);
        draw_row(canvas, fonts, scale, panel_x, rows_top + r as f32 * ROW_H, label, hint, fields.value(cat, r), r == row)?;
    }

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[
            chrome::FooterPrompt { glyph: "\u{2195}", label: "Row", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::FooterPrompt { glyph: "L/R", label: "Category", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::FooterPrompt { glyph: "\u{2194}", label: "Adjust", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::PROMPT_BACK,
        ],
        FooterRight::Text("CHANGES SAVED AUTOMATICALLY"),
    )?;
    Ok(())
}

fn draw_cat_row(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    index: usize,
    label: &str,
    list_top: f32,
    selected: bool,
) -> Result<(), String> {
    let y = list_top + index as f32 * (CAT_ROW_H + CAT_ROW_GAP);
    if selected {
        let tint = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 36);
        let clear = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 0);
        geometry::fill_horizontal_gradient_rect(canvas, scale, SIDE_PAD, y, SIDEBAR_W, CAT_ROW_H, tint, clear);
    }
    let bar_color = if selected { theme::ACCENT } else { Color::RGBA(255, 255, 255, 18) };
    geometry::fill_skewed_rect(canvas, scale, SIDE_PAD, y, 6.0, CAT_ROW_H, -11.0, bar_color);

    let num_color = if selected { theme::ACCENT } else { Color::RGB(0x34, 0x34, 0x3a) };
    let (nx, ny) = scale.point(SIDE_PAD + 22.0, y + CAT_ROW_H / 2.0 - 7.0);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), &format!("{:02}", index + 1), nx, ny, num_color)?;

    let label_color = if selected { theme::TEXT } else { Color::RGB(0x5a, 0x5a, 0x62) };
    let (lx, ly) = scale.point(SIDE_PAD + 22.0 + 30.0, y + CAT_ROW_H / 2.0 - 13.0);
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(26.0), label, lx, ly, label_color)?;
    Ok(())
}

fn draw_cabinet_box(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, y: f32) -> Result<(), String> {
    let w = SIDEBAR_W;
    let h = 60.0;
    canvas.set_draw_color(Color::RGBA(14, 14, 18, 153));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, w, h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 20));
    canvas.draw_rect(scale.rect(SIDE_PAD, y, w, h))?;
    let (lx, ly) = scale.point(SIDE_PAD + 18.0, y + 10.0);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "CABINET", lx, ly, theme::MUTE)?;
    let (nx, ny) = scale.point(SIDE_PAD + 18.0, y + 27.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(20.0), "MORTAL KOMBAT II", nx, ny, Color::RGB(0xcf, 0xcf, 0xc9))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    y: f32,
    label: &str,
    hint: &str,
    value: RowValue,
    selected: bool,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 15));
    canvas.fill_rect(Some(scale.rect(x, y + ROW_H - 1.0, 1360.0, 1.0)))?;
    if selected {
        canvas.set_draw_color(theme::ACCENT);
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        canvas.fill_rect(Some(scale.rect(x, y, 3.0, ROW_H)))?;
    }

    let text_x = x + 12.0;
    let (lx, ly) = scale.point(text_x, y + 12.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(24.0), label, lx, ly, Color::RGB(0xed, 0xed, 0xe8))?;
    let (hx, hy) = scale.point(text_x, y + 42.0);
    fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(13.0), hint, hx, hy, theme::MUTE)?;

    let right_edge = x + 1360.0;
    match value {
        RowValue::Toggle(on) => {
            let text = if on { "ON" } else { "OFF" };
            let (tw, th) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(15.0), text);
            let pad_x = 16.0;
            let pad_y = 8.0;
            let pill_w = (tw as f32 / scale.s) + pad_x * 2.0;
            let pill_h = (th as f32 / scale.s) + pad_y * 2.0;
            let pill_x = right_edge - pill_w;
            let pill_y = y + ROW_H / 2.0 - pill_h / 2.0;
            let (border, bg, color) = if on {
                (
                    Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 165),
                    Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 36),
                    Color::RGB(255, 255, 255),
                )
            } else {
                (Color::RGBA(255, 255, 255, 36), Color::RGBA(255, 255, 255, 8), Color::RGB(0xcf, 0xcf, 0xc9))
            };
            canvas.set_draw_color(bg);
            canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
            canvas.fill_rect(Some(scale.rect(pill_x, pill_y, pill_w, pill_h)))?;
            canvas.set_draw_color(border);
            canvas.draw_rect(scale.rect(pill_x, pill_y, pill_w, pill_h))?;
            let (ttx, tty) = scale.point(pill_x + pad_x, pill_y + pad_y);
            fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(15.0), text, ttx, tty, color)?;
        }
        RowValue::Cycle(text) => {
            let (tw, th) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(15.0), text);
            let pad_x = 16.0;
            let pad_y = 8.0;
            let pill_w = (tw as f32 / scale.s) + pad_x * 2.0;
            let pill_h = (th as f32 / scale.s) + pad_y * 2.0;
            let pill_x = right_edge - pill_w;
            let pill_y = y + ROW_H / 2.0 - pill_h / 2.0;
            canvas.set_draw_color(Color::RGBA(255, 255, 255, 8));
            canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
            canvas.fill_rect(Some(scale.rect(pill_x, pill_y, pill_w, pill_h)))?;
            canvas.set_draw_color(Color::RGBA(255, 255, 255, 36));
            canvas.draw_rect(scale.rect(pill_x, pill_y, pill_w, pill_h))?;
            let (ttx, tty) = scale.point(pill_x + pad_x, pill_y + pad_y);
            fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(15.0), text, ttx, tty, Color::RGB(0xcf, 0xcf, 0xc9))?;
        }
        RowValue::Slider { pct, text } => {
            let (tw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(18.0), &text);
            let value_w = (tw as f32 / scale.s).max(46.0);
            let slider_w = 240.0;
            let gap = 16.0;
            let slider_x = right_edge - value_w - gap - slider_w;
            let slider_y = y + ROW_H / 2.0 - 3.5;
            canvas.set_draw_color(Color::RGBA(255, 255, 255, 20));
            canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
            canvas.fill_rect(Some(scale.rect(slider_x, slider_y, slider_w, 7.0)))?;
            let fill_w = slider_w * (pct.clamp(0.0, 100.0) / 100.0);
            canvas.set_draw_color(theme::ACCENT);
            canvas.fill_rect(Some(scale.rect(slider_x, slider_y, fill_w, 7.0)))?;
            let (vx, vy) = scale.point(right_edge - value_w, y + ROW_H / 2.0 - 9.0);
            fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(18.0), &text, vx, vy, Color::RGB(0xf2, 0xf2, 0xee))?;
        }
    }
    Ok(())
}
