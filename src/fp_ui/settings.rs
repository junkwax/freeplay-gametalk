//! Settings — matches `screenshots/04-settings.png`'s row language (sidebar
//! categories, slider/pill rows), bound to real `Config` fields, now with
//! all 4 of the mockup's categories (Controls/Video/Audio/Netplay) — an
//! earlier pass here left Controls out as "an open product question" per
//! the legacy-screens handoff doc; the fuller mockup answers that question
//! itself (`catDefs` lists `controls` first), so it's implemented for real
//! now rather than continuing to flag it.
//!
//! Controls (cat 0) is a real rebinding UI, not a config-field row list like
//! the other 3 categories — see `draw_controls_rows` and
//! `super::FpResult::BeginRebind`/`ClearAllBindings`, which hand off to the
//! exact same `AppState::Rebinding` capture flow and `PlayerBindings`
//! storage the legacy Controls screen (`crate::menu::draw_controls`) uses,
//! rather than reimplementing bind storage/capture here.
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
use crate::input::{Action, Bindings, Player};
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

pub const CONTROLS_CAT_INDEX: usize = 0;
/// Username / Stats Email / Discord connection — the profile-identity
/// fields the legacy `MenuScreen::Settings` used to expose (rows 0-2 there)
/// that had no home anywhere in fp_ui after the redesign into
/// Controls/Video/Audio/Netplay categories. Not in the mockup's own
/// `catDefs` (which has no account/profile category at all), but real user
/// feedback that these were missing and useful outweighs mockup fidelity
/// here — appended last rather than reordered in, to avoid renumbering the
/// other 4 categories' index constants.
pub const ACCOUNT_CAT_INDEX: usize = 4;
/// Diagnostics — a real UDP-probe flow (`netplay::probe_connection`) the
/// mockup's own Settings screen has as its 5th category; this build's own
/// 5th category is Account (see above) instead, so Test Connection is
/// appended as a 6th rather than displacing it — both are real,
/// backend-backed functionality worth keeping.
pub const TEST_CONN_CAT_INDEX: usize = 5;
pub const CATS: [&str; 6] = ["CONTROLS", "VIDEO", "AUDIO", "NETPLAY", "ACCOUNT", "TEST CONNECTION"];
// Controls: 11 actions + a "CLEAR ALL" row. Account: Username/Stats Email/
// Discord. Test Connection: just the one address field.
const ROWS_PER_CAT: [usize; 6] = [Action::ALL.len() + 1, 6, 2, 4, 3, 1];

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

    /// `cat` here is one of the 3 config-row categories (Video=1/Audio=2/
    /// Netplay=3) — Controls (cat 0) has its own row rendering
    /// (`draw_controls_rows`), not a `SettingsFields` row at all.
    fn row_meta(cat: usize, row: usize) -> (&'static str, &'static str) {
        match (cat, row) {
            (1, 0) => ("FULLSCREEN", "Borderless desktop fullscreen"),
            (1, 1) => ("RENDER PROFILE", "SDL renderer backend"),
            (1, 2) => ("VIDEO FILTER", "Gameplay frame presentation"),
            (1, 3) => ("CRT CORNER BEND", "Rounded glass shading on CRT filters"),
            (1, 4) => ("ASPECT MODE", "How the frame fits the window"),
            (1, 5) => ("SCOREBAR STYLE", "Netplay score overlay layout"),
            (2, 0) => ("VOLUME", "Output level"),
            (2, 1) => ("AUDIO BUFFER", "Queue depth vs. latency"),
            (3, 0) => ("INPUT DELAY", "Frames of delay before rollback"),
            (3, 1) => ("RUNAHEAD (OFFLINE)", "One-frame speculative local play"),
            (3, 2) => ("RUNAHEAD (ONLINE)", "Experimental video-only netplay speculation"),
            (3, 3) => ("DISCORD RICH PRESENCE", "Show match status in Discord"),
            _ => ("", ""),
        }
    }

    fn value(&self, cat: usize, row: usize) -> RowValue {
        match (cat, row) {
            (1, 0) => RowValue::Toggle(self.fullscreen),
            (1, 1) => RowValue::Cycle(self.render_profile.label()),
            (1, 2) => RowValue::Cycle(self.video_filter.label()),
            (1, 3) => RowValue::Toggle(self.crt_corner_bend),
            (1, 4) => RowValue::Cycle(self.aspect_mode.label()),
            (1, 5) => RowValue::Cycle(self.scorebar_style.label()),
            (2, 0) => RowValue::Slider { pct: self.volume_percent as f32, text: format!("{}%", self.volume_percent) },
            (2, 1) => RowValue::Cycle(self.audio_buffer.label()),
            (3, 0) => RowValue::Slider {
                pct: self.input_delay as f32 / 8.0 * 100.0,
                text: format!("{}f", self.input_delay),
            },
            (3, 1) => RowValue::Toggle(self.runahead),
            (3, 2) => RowValue::Toggle(self.runahead_online),
            (3, 3) => RowValue::Toggle(self.discord_rpc_enabled),
            _ => RowValue::Toggle(false),
        }
    }

    /// Mutate the field at `(cat, row)` by `delta` (±1). Toggles ignore the
    /// sign and flip.
    pub fn adjust(&mut self, cat: usize, row: usize, delta: i8) {
        match (cat, row) {
            (1, 0) => self.fullscreen = !self.fullscreen,
            (1, 1) => self.render_profile = self.render_profile.cycle(delta),
            (1, 2) => self.video_filter = self.video_filter.cycle(delta),
            (1, 3) => self.crt_corner_bend = !self.crt_corner_bend,
            (1, 4) => self.aspect_mode = self.aspect_mode.cycle(delta),
            (1, 5) => self.scorebar_style = self.scorebar_style.cycle(delta),
            (2, 0) => {
                self.volume_percent = (self.volume_percent as i32 + delta as i32 * 5).clamp(0, 100) as u8
            }
            (2, 1) => self.audio_buffer = self.audio_buffer.cycle(delta),
            (3, 0) => self.input_delay = (self.input_delay as i32 + delta as i32).clamp(0, 8) as u32,
            (3, 1) => self.runahead = !self.runahead,
            (3, 2) => self.runahead_online = !self.runahead_online,
            (3, 3) => self.discord_rpc_enabled = !self.discord_rpc_enabled,
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
    sidebar_focus: bool,
    controls_player: Player,
    bindings: &Bindings,
    username: &str,
    stats_email: &str,
    discord_connected: bool,
    test_conn_address: &str,
    test_conn_lines: &[String],
) -> Result<(), String> {
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    let (ex, ey) = scale.point(SIDE_PAD + 44.0, CONTENT_TOP);
    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, CONTENT_TOP + 8.0, 30.0, 3.0)))?;
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "CONFIGURATION", ex, ey, theme::ACCENT)?;
    let (tx, ty) = scale.point(SIDE_PAD, CONTENT_TOP + 26.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(58.0), "SETTINGS", tx, ty, theme::TEXT)?;

    // The mockup's CSS puts a modest 28px margin-bottom under "SETTINGS"
    // (not a big gap like Main Menu's header->row fix needed) — this was
    // landing about 12px short of that, reading as slightly cramped right
    // under the title; bumped from 70 to 82 to match.
    let list_top = CONTENT_TOP + 26.0 + 82.0;
    for (i, label) in CATS.iter().enumerate() {
        draw_cat_row(canvas, fonts, scale, i, label, list_top, i == cat, sidebar_focus)?;
    }
    draw_cabinet_box(canvas, fonts, scale, list_top + CATS.len() as f32 * (CAT_ROW_H + CAT_ROW_GAP) + 24.0)?;

    let panel_x = SIDE_PAD + SIDEBAR_W + PANEL_GAP;
    let (cnx, cny) = scale.point(panel_x, list_top);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(24.0), CATS[cat], cnx, cny, Color::RGB(0x8a, 0x8a, 0x92))?;

    let rows_top = list_top + 44.0;
    if cat == CONTROLS_CAT_INDEX {
        draw_controls_rows(canvas, fonts, scale, panel_x, rows_top, row, controls_player, bindings, !sidebar_focus)?;
    } else if cat == ACCOUNT_CAT_INDEX {
        draw_account_rows(canvas, fonts, scale, panel_x, rows_top, row, username, stats_email, discord_connected, !sidebar_focus)?;
    } else if cat == TEST_CONN_CAT_INDEX {
        draw_test_conn(canvas, fonts, scale, panel_x, rows_top, test_conn_address, test_conn_lines)?;
    } else {
        for r in 0..rows_in_cat(cat) {
            let (label, hint) = SettingsFields::row_meta(cat, r);
            draw_row(canvas, fonts, scale, panel_x, rows_top + r as f32 * ROW_H, label, hint, fields.value(cat, r), r == row, !sidebar_focus)?;
        }
    }

    let content_prompts: &[chrome::FooterPrompt] = if cat == CONTROLS_CAT_INDEX {
        &[
            chrome::FooterPrompt { glyph: "\u{2195}", label: "Row", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::FooterPrompt { glyph: "L/R", label: "Category", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::FooterPrompt { glyph: "\u{2194}", label: "Switch Player", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::PROMPT_SELECT,
            chrome::PROMPT_BACK,
        ]
    } else if cat == ACCOUNT_CAT_INDEX {
        &[
            chrome::FooterPrompt { glyph: "\u{2195}", label: "Row", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::FooterPrompt { glyph: "L/R", label: "Category", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::PROMPT_SELECT,
            chrome::PROMPT_BACK,
        ]
    } else if cat == TEST_CONN_CAT_INDEX {
        &[
            chrome::FooterPrompt { glyph: "L/R", label: "Category", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::FooterPrompt { glyph: "X", label: "Run Probe", color: theme::BTN_CROSS },
            chrome::PROMPT_BACK,
        ]
    } else {
        &[
            chrome::FooterPrompt { glyph: "\u{2195}", label: "Row", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::FooterPrompt { glyph: "L/R", label: "Category", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::FooterPrompt { glyph: "\u{2194}", label: "Adjust", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::PROMPT_BACK,
        ]
    };
    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        content_prompts,
        FooterRight::Text("CHANGES SAVED AUTOMATICALLY"),
    )?;
    Ok(())
}

/// `current`: this is the active category regardless of input focus.
/// `focused`: Up/Down currently drive the sidebar (`FpScreen::Settings`'s
/// `sidebar_focus`) — only then does the current category get the full
/// bright treatment; otherwise it shows a dimmer "this is where you are,
/// but Up/Down won't move it right now" marker, so it's visually clear
/// which side of the screen will respond to Up/Down.
fn draw_cat_row(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    index: usize,
    label: &str,
    list_top: f32,
    current: bool,
    focused: bool,
) -> Result<(), String> {
    let hot = current && focused;
    let y = list_top + index as f32 * (CAT_ROW_H + CAT_ROW_GAP);
    if hot {
        let tint = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 36);
        let clear = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 0);
        geometry::fill_horizontal_gradient_rect(canvas, scale, SIDE_PAD, y, SIDEBAR_W, CAT_ROW_H, tint, clear);
    }
    let bar_color = if hot {
        theme::ACCENT
    } else if current {
        Color::RGBA(255, 255, 255, 90)
    } else {
        Color::RGBA(255, 255, 255, 18)
    };
    geometry::fill_skewed_rect(canvas, scale, SIDE_PAD, y, 6.0, CAT_ROW_H, -11.0, bar_color);

    let num_color = if hot {
        theme::ACCENT
    } else if current {
        Color::RGB(0x8a, 0x8a, 0x92)
    } else {
        Color::RGB(0x34, 0x34, 0x3a)
    };
    // True visible-pixel centering (not a fixed "half the font size" offset)
    // — the number (Chakra Petch, 13px) and label (Saira Condensed, 26px)
    // are different families at different sizes, so naive offsets tuned for
    // one don't land the other on the same visual center line. Same
    // `visible_span` technique `main_menu.rs`/`play_menu.rs` already use for
    // their own row centering.
    let num_text = format!("{:02}", index + 1);
    let num_px = scale.font_px(13.0);
    let (num_inset, num_vis_h) = fonts.visible_span(FpFont::ChakraPetchSemiBold, num_px, &num_text);
    let num_top = y + CAT_ROW_H / 2.0 - (num_vis_h as f32 / scale.s) / 2.0;
    let (nx, ny) = scale.point(SIDE_PAD + 22.0, num_top - num_inset as f32 / scale.s);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, num_px, &num_text, nx, ny, num_color)?;

    let label_color = if hot {
        theme::TEXT
    } else if current {
        Color::RGB(0x9a, 0x9a, 0xa2)
    } else {
        Color::RGB(0x5a, 0x5a, 0x62)
    };
    let label_px = scale.font_px(26.0);
    let (label_inset, label_vis_h) = fonts.visible_span(FpFont::SairaCondensedBlack, label_px, label);
    let label_top = y + CAT_ROW_H / 2.0 - (label_vis_h as f32 / scale.s) / 2.0;
    let (lx, ly) = scale.point(SIDE_PAD + 22.0 + 30.0, label_top - label_inset as f32 / scale.s);
    fonts.draw(canvas, FpFont::SairaCondensedBlack, label_px, label, lx, ly, label_color)?;
    Ok(())
}

/// The mockup's own version of this box has a third line, "ROM rev L3.1 ·
/// FREE PLAY" — no real MK2 ROM revision is tracked anywhere in this app, so
/// rather than fabricate one this shows freeplay-gametalk's own real build
/// version instead (which "FREE PLAY" is also true of — there's no coin
/// economy in this client, it's always free play).
fn draw_cabinet_box(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, y: f32) -> Result<(), String> {
    let w = SIDEBAR_W;
    let h = 84.0;
    canvas.set_draw_color(Color::RGBA(14, 14, 18, 153));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, w, h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 20));
    canvas.draw_rect(scale.rect(SIDE_PAD, y, w, h))?;
    let (lx, ly) = scale.point(SIDE_PAD + 18.0, y + 10.0);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "CABINET", lx, ly, theme::MUTE)?;
    let (nx, ny) = scale.point(SIDE_PAD + 18.0, y + 27.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(20.0), "MORTAL KOMBAT II", nx, ny, Color::RGB(0xcf, 0xcf, 0xc9))?;
    let build = format!("FREE PLAY \u{b7} v{}", crate::version::VERSION);
    let (fx, fy) = scale.point(SIDE_PAD + 18.0, y + 55.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(12.0), &build, fx, fy, Color::RGB(0x52, 0x52, 0x5a))?;
    Ok(())
}

/// Controls category content: a P1/P2 tab switcher (mockup's own
/// `rebindPlayerTabs`, mouse-only there — `Left`/`Right` drive it here
/// instead, see `FpScreen::Settings::controls_player`'s doc comment) above
/// the 11 real `Action::ALL` bindings plus a "CLEAR ALL" row, reading
/// current bindings via the same `crate::menu::summarize_bindings`/
/// `pretty_binding_name` the legacy Controls screen displays them with.
#[allow(clippy::too_many_arguments)]
fn draw_controls_rows(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    top: f32,
    row: usize,
    controls_player: Player,
    bindings: &Bindings,
    focused: bool,
) -> Result<(), String> {
    let tab_h = 36.0;
    for (i, p) in [Player::P1, Player::P2].iter().enumerate() {
        let tab_w = 70.0;
        let tx = x + i as f32 * (tab_w + 8.0);
        let active = *p == controls_player;
        canvas.set_draw_color(if active {
            Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 46)
        } else {
            Color::RGBA(255, 255, 255, 10)
        });
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        canvas.fill_rect(Some(scale.rect(tx, top, tab_w, tab_h)))?;
        canvas.set_draw_color(if active { theme::ACCENT } else { Color::RGBA(255, 255, 255, 24) });
        canvas.draw_rect(scale.rect(tx, top, tab_w, tab_h))?;
        let label = p.label();
        let (lw, lh) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(13.0), label);
        let (ltx, lty) = scale.point(
            tx + tab_w / 2.0 - (lw as f32 / scale.s) / 2.0,
            top + tab_h / 2.0 - (lh as f32 / scale.s) / 2.0,
        );
        fonts.draw(
            canvas,
            FpFont::ChakraPetchSemiBold,
            scale.font_px(13.0),
            label,
            ltx,
            lty,
            if active { theme::TEXT } else { Color::RGB(0x6a, 0x6a, 0x72) },
        )?;
    }

    let rows_top = top + tab_h + 14.0;
    let row_h = 46.0;
    let pb = bindings.get(controls_player);
    for (i, action) in Action::ALL.iter().enumerate() {
        let y = rows_top + i as f32 * row_h;
        let selected = row == i;
        draw_bind_row(
            canvas,
            fonts,
            scale,
            x,
            y,
            row_h,
            action.label(),
            &crate::menu::summarize_bindings(pb, *action),
            "CROSS TO REBIND",
            selected,
            focused,
        )?;
    }

    let clear_y = rows_top + Action::ALL.len() as f32 * row_h + 10.0;
    let clear_selected = row == Action::ALL.len();
    canvas.set_draw_color(if clear_selected && focused {
        Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 20)
    } else {
        Color::RGBA(255, 255, 255, 8)
    });
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    let clear_w = 260.0;
    let clear_h = 40.0;
    canvas.fill_rect(Some(scale.rect(x, clear_y, clear_w, clear_h)))?;
    canvas.set_draw_color(if clear_selected && focused { theme::ACCENT } else { Color::RGBA(255, 255, 255, 24) });
    canvas.draw_rect(scale.rect(x, clear_y, clear_w, clear_h))?;
    // "X" rather than the Unicode multiplication-X glyph (U+2715) — missing
    // from this font, same tofu-box issue `chrome::PROMPT_SELECT` already
    // works around with a plain letter.
    let clear_label = format!("X CLEAR ALL \u{b7} {}", controls_player.label());
    let (cx, cy) = scale.point(x + 16.0, clear_y + clear_h / 2.0 - 7.0);
    fonts.draw(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(12.0),
        &clear_label,
        cx,
        cy,
        if clear_selected && focused { theme::ACCENT } else { Color::RGB(0x7a, 0x7a, 0x82) },
    )?;
    Ok(())
}

/// Account category content: Username / Stats Email / Discord connection —
/// see `super::FpResult::BeginAccountEdit`/`ToggleDiscordConnect`, which hand
/// off to the exact legacy `MenuScreen::TextEdit`/Discord-OAuth flows the
/// old `MenuScreen::Settings` rows 0-2 used, rather than reimplementing
/// text capture or OAuth here.
#[allow(clippy::too_many_arguments)]
fn draw_account_rows(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    top: f32,
    row: usize,
    username: &str,
    stats_email: &str,
    discord_connected: bool,
    focused: bool,
) -> Result<(), String> {
    let row_h = 46.0;
    let rows: [(&str, String, &str); 3] = [
        ("USERNAME", username.to_string(), "CROSS TO EDIT"),
        (
            "STATS EMAIL",
            if stats_email.is_empty() { "NOT SET".to_string() } else { stats_email.to_string() },
            "CROSS TO EDIT",
        ),
        (
            "DISCORD",
            if discord_connected { "CONNECTED".to_string() } else { "NOT CONNECTED".to_string() },
            if discord_connected { "CROSS TO DISCONNECT" } else { "CROSS TO CONNECT" },
        ),
    ];
    for (i, (label, value, hint)) in rows.iter().enumerate() {
        let y = top + i as f32 * row_h;
        draw_bind_row(canvas, fonts, scale, x, y, row_h, label, value, hint, row == i, focused)?;
    }
    Ok(())
}

/// Test Connection category content: a real hardware-keyboard address
/// field (typed into directly, same mechanism `claim_username.rs` uses —
/// see `FpResult::RunConnectionProbe`'s doc comment) plus the exact
/// `render::format_probe_result` output a real probe produces. Lines are
/// colored by the same "OK "/"WARN "/"FAIL " prefix convention that
/// function already uses; unprefixed lines are either section headers
/// (`"L3 LOCAL STACK"`, no leading space) or indented follow-up detail
/// (leading spaces) — distinguished here by that same leading-whitespace
/// check rather than a second real/fabricated classification field.
fn draw_test_conn(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    top: f32,
    address: &str,
    lines: &[String],
) -> Result<(), String> {
    let field_w = 460.0;
    let field_h = 46.0;
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 15));
    canvas.fill_rect(Some(scale.rect(x, top, field_w, field_h)))?;
    canvas.set_draw_color(theme::ACCENT);
    canvas.draw_rect(scale.rect(x, top, field_w, field_h))?;
    let (vx, vy) = scale.point(x + 14.0, top + field_h / 2.0 - 10.0);
    let (vw, _) = fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(17.0), address, vx, vy, Color::RGB(0xF4, 0xF4, 0xF0))?;
    let caret_x = x + 14.0 + (vw as f32 / scale.s) + 3.0;
    canvas.set_draw_color(theme::ACCENT);
    canvas.fill_rect(Some(scale.rect(caret_x, top + 10.0, 2.0, field_h - 20.0)))?;

    let hint_y = top + field_h + 8.0;
    let (hx, hy) = scale.point(x, hint_y);
    fonts.draw(
        canvas,
        FpFont::SairaMedium,
        scale.font_px(12.0),
        "Type an address, then Cross to run a UDP probe",
        hx,
        hy,
        Color::RGB(0x52, 0x52, 0x5a),
    )?;

    let log_top = hint_y + 30.0;
    let log_w = 1360.0f32.max(field_w);
    let log_h = 420.0;
    canvas.set_draw_color(Color::RGBA(4, 4, 7, 179));
    canvas.fill_rect(Some(scale.rect(x, log_top, log_w, log_h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(x, log_top, log_w, log_h))?;
    let (plx, ply) = scale.point(x + 16.0, log_top + 12.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "PROBE OUTPUT", plx, ply, Color::RGB(0x3a, 0x3a, 0x42), scale.len(4.0).round() as i32)?;

    if lines.is_empty() {
        let msg = "ENTER AN ADDRESS AND RUN PROBE";
        let (mw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(12.0), msg);
        let (mx, my) = scale.point(x + (log_w - mw as f32 / scale.s) / 2.0, log_top + log_h / 2.0 - 6.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(12.0), msg, mx, my, Color::RGB(0x1e, 0x1e, 0x24))?;
    } else {
        let row_h = 22.0;
        let max_visible = ((log_h - 40.0) / row_h).floor().max(1.0) as usize;
        let mut y = log_top + 40.0;
        for line in lines.iter().take(max_visible) {
            if line.is_empty() {
                y += row_h * 0.4;
                continue;
            }
            let indented = line.starts_with(' ');
            let (color, text) = if let Some(rest) = line.strip_prefix("OK ") {
                (theme::GREEN, rest)
            } else if let Some(rest) = line.strip_prefix("WARN ") {
                (theme::WARNING, rest)
            } else if let Some(rest) = line.strip_prefix("FAIL ") {
                (theme::HIGH_PING, rest)
            } else if indented {
                (Color::RGB(0x52, 0x52, 0x5a), line.trim_start())
            } else {
                (Color::RGB(0xcf, 0xcf, 0xc9), line.as_str())
            };
            let (lx, ly) = scale.point(x + 16.0 + if indented { 14.0 } else { 0.0 }, y);
            fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(13.0), text, lx, ly, color)?;
            y += row_h;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_bind_row(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    y: f32,
    row_h: f32,
    action_label: &str,
    binding_text: &str,
    select_hint: &str,
    selected: bool,
    focused: bool,
) -> Result<(), String> {
    if selected {
        canvas.set_draw_color(if focused {
            theme::ACCENT
        } else {
            Color::RGBA(255, 255, 255, 90)
        });
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        canvas.fill_rect(Some(scale.rect(x, y, 3.0, row_h - 4.0)))?;
    }
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 10));
    canvas.fill_rect(Some(scale.rect(x, y + row_h - 4.0, 620.0, 1.0)))?;

    let (lx, ly) = scale.point(x + 14.0, y + row_h / 2.0 - 11.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(20.0), action_label, lx, ly, Color::RGB(0xed, 0xed, 0xe8))?;

    if selected && focused {
        let hint = select_hint;
        let (hw, hh) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(11.0), hint);
        let pad_x = 9.0;
        let pad_y = 3.0;
        let hint_w = (hw as f32 / scale.s) + pad_x * 2.0;
        let hint_h = (hh as f32 / scale.s) + pad_y * 2.0;
        let hint_x = x + 620.0 - hint_w;
        let hint_y = y + row_h / 2.0 - hint_h / 2.0 - 2.0;
        canvas.set_draw_color(Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 140));
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        canvas.draw_rect(scale.rect(hint_x, hint_y, hint_w, hint_h))?;
        let (htx, hty) = scale.point(hint_x + pad_x, hint_y + pad_y);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), hint, htx, hty, theme::ACCENT)?;
    } else {
        let (bw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(14.0), binding_text);
        let (bx, by) = scale.point(x + 620.0 - (bw as f32 / scale.s), y + row_h / 2.0 - 9.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(14.0), binding_text, bx, by, Color::RGB(0xcf, 0xcf, 0xc9))?;
    }
    Ok(())
}

/// `focused`: mirrors `draw_cat_row`'s — true when Up/Down currently drive
/// this content panel rather than the category sidebar.
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
    focused: bool,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 15));
    canvas.fill_rect(Some(scale.rect(x, y + ROW_H - 1.0, 1360.0, 1.0)))?;
    if selected {
        canvas.set_draw_color(if focused {
            theme::ACCENT
        } else {
            Color::RGBA(255, 255, 255, 90)
        });
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
