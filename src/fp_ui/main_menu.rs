//! Main Menu — matches the mockup's `menuDefs`/`isMenu` branch, minus its
//! "Network News" row (hidden for now — see `super::MAIN_ITEM_COUNT`'s doc
//! comment; still fully built in `bandwidth.rs`, just not linked from here),
//! and not the real app's full 9-item `crate::menu::MAIN_ITEMS`. The other 4 legacy items
//! (Arcade/Lab/Replays/Drones) live one level down in `play_menu.rs`'s
//! submenu, matching the mockup's own `playMenuDefs`; Controls folds into
//! Settings' categories (a follow-up item — see `settings.rs`'s module
//! doc); Profile and Quit aren't reachable from here at all in the mockup
//! (Profile is a mouse-only header chip with no documented gamepad
//! binding; Quit is invoked by a hold-Start gesture this app has no
//! hold-duration tracking for yet — see `super::nav`'s `Back` handling for
//! the substitute used instead).
//!
//! "YOUR STATS" and "LAST MATCH" read the real `ProfileData`/`HistoryRow`
//! the caller already fetches at startup for the current player (same
//! `matchmaking::fetch_profile` the dedicated Profile screen uses on
//! demand) — see `main.rs`'s `main_profile`. The mockup's own mock data
//! invents a `profileRank` ("SILVER III"), region ("NA-WEST") and node id
//! ("NODE SF-04") that have no real backend equivalent (no per-player
//! tier/region/node concept exists anywhere in this app), so that subtitle
//! line is replaced with the player's real Glicko rating and match count
//! instead of fabricating those fields.

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use crate::matchmaking::HistoryRow;
use crate::menu::ProfileScreenState;
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const ROW_H: f32 = 92.0;
const ROW_GAP: f32 = 4.0;
const BAR_W: f32 = 8.0;
const SKEW_DEG: f32 = -9.0;
const LIST_X: f32 = 56.0;
// Header (104) + this screen's own container `top:54` offset — the top of
// the eyebrow ("ARCADE * FREEPLAY ONLINE"), not the row list. The row list
// sits well below this: the eyebrow's own row height plus its CSS
// `margin-bottom:30px` before the first item starts — see `ROWS_TOP`.
const LIST_TOP: f32 = 158.0;
// LIST_TOP plus the eyebrow row's height and its margin-bottom, measured
// from a headless render of the reference mockup (the selected row's solid
// accent bar spans logical y~214-306, i.e. row 0 begins at 214, not at
// LIST_TOP) rather than guessed — a previous pass here used LIST_TOP
// directly as the row list's own top and drew the eyebrow *above* it
// (`LIST_TOP - 44.0`), which inverted the real relationship and packed
// everything tight against the header instead of leaving the mockup's
// breathing room between header -> eyebrow -> first row.
const ROWS_TOP: f32 = LIST_TOP + 56.0;
const LABEL_GAP: f32 = 26.0; // bar -> number -> label gap, per rowStyle

/// (label, sub-label) — verbatim from the mockup's own `menuDefs`, minus
/// its "Network News" row (see this module's doc comment).
pub const ITEMS: [(&str, &str); 4] = [
    ("PLAY", "Start a local freeplay match"),
    ("ONLINE", "Find, host or join a netplay match"),
    ("RANKINGS", "National top 100 \u{b7} season ladder"),
    ("SETTINGS", "Controls \u{b7} video \u{b7} audio \u{b7} netcode"),
];

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    cursor: usize,
    username: &str,
    profile: &ProfileScreenState,
    rom_present: bool,
) -> Result<(), String> {
    chrome::draw_background_accents(canvas, scale)?;
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    draw_ghost_watermark(canvas, fonts, scale, rom_present)?;
    draw_cabinet_title(canvas, fonts, scale, rom_present)?;

    // Eyebrow: accent bar + "ARCADE * FREEPLAY ONLINE".
    let eyebrow_y = LIST_TOP;
    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(LIST_X, eyebrow_y + 8.0, 30.0, 3.0)))?;
    let (ex, ey) = scale.point(LIST_X + 44.0, eyebrow_y);
    fonts.draw_tracked(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(14.0),
        "ARCADE \u{b7} FREEPLAY ONLINE",
        ex,
        ey,
        theme::ACCENT,
        scale.len(7.0).round() as i32,
    )?;

    for (i, (label, sub)) in ITEMS.iter().enumerate() {
        // PLAY (row 0) is the only row gated on a ROM being present —
        // Arcade/Lab/Replays/Drones all end up calling ensure_core_loaded,
        // which hard-fails without one, so there's nothing else under it
        // to individually disable.
        let disabled = i == 0 && !rom_present;
        draw_row(canvas, fonts, scale, i, label, sub, i == cursor, disabled)?;
    }

    draw_last_match_card(canvas, fonts, scale, ITEMS.len() as f32, profile, cursor == ITEMS.len() + 1)?;
    draw_your_stats_panel(canvas, fonts, scale, username, profile, cursor == ITEMS.len())?;
    // The wire ticker is disabled for now along with the Network News row it
    // quotes — see `draw_ticker`'s doc comment.

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[chrome::PROMPT_NAVIGATE, chrome::PROMPT_SELECT],
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
    disabled: bool,
) -> Result<(), String> {
    let y = ROWS_TOP + index as f32 * (ROW_H + ROW_GAP);
    // Disabled (no ROM found) always reads as dim/inactive, even while the
    // cursor sits on it for keyboard nav feedback — Confirm is a no-op
    // there (see `nav`'s Main arm), so it shouldn't look inviting.
    let selected = selected && !disabled;

    if selected {
        let tint = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 36); // ~86% transparent
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
    let label_color = if disabled {
        Color::RGB(0x3a, 0x3a, 0x3e)
    } else if selected {
        theme::TEXT
    } else {
        Color::RGB(0x6a, 0x6a, 0x72)
    };
    let label_font = if selected { FpFont::SairaCondensedBlack } else { FpFont::SairaCondensedBold };
    let label_text = label.to_uppercase();
    let sub_text = if disabled { "NO ROM FOUND".to_string() } else { sub.to_uppercase() };
    let label_x = LIST_X + BAR_W + LABEL_GAP + 30.0 + LABEL_GAP;
    let label_px = scale.font_px(label_size);
    let sub_px = scale.font_px(15.0);

    // Center the (label, sub) pair as a block on their *actual rendered
    // pixels* (`visible_span`), not `size_of`'s full ascent+descent line
    // height — that measurement is noticeably taller than the visible
    // glyphs for a short all-caps label, and centering on it (or offsetting
    // from it with guessed constants, which an earlier pass here did) left
    // the block looking bottom-heavy within the row's selection gradient
    // instead of centered.
    let (label_inset, label_vis_h) = fonts.visible_span(label_font, label_px, &label_text);
    let label_vis_h_l = label_vis_h as f32 / scale.s;
    let gap = 14.0;
    let (sub_inset, sub_vis_h) = if selected || disabled {
        fonts.visible_span(FpFont::SairaSemiBold, sub_px, &sub_text)
    } else {
        (0, 0)
    };
    let sub_vis_h_l = sub_vis_h as f32 / scale.s;
    let block_h = if selected || disabled { label_vis_h_l + gap + sub_vis_h_l } else { label_vis_h_l };
    let block_top = y + (ROW_H - block_h) / 2.0;

    let (lx, ly) = scale.point(label_x, block_top - label_inset as f32 / scale.s);
    fonts.draw_italic(canvas, label_font, label_px, &label_text, lx, ly, label_color)?;

    if disabled {
        let sub_visual_top = block_top + label_vis_h_l + gap;
        let (sx, sy) = scale.point(label_x, sub_visual_top - sub_inset as f32 / scale.s);
        fonts.draw(canvas, FpFont::SairaSemiBold, sub_px, &sub_text, sx, sy, Color::RGB(0x5a, 0x2a, 0x2a))?;
    }

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

        // Selected-row chevron ("&#9656;" in the mockup), right edge of the
        // 730px-wide row, skewed the same -9deg as everything else here.
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

/// "LAST MATCH" card, directly below the item list — reads the most recent
/// entry in the real match-history list the caller already fetched
/// (`main.rs`'s `main_profile`), not the mockup's invented "REMATCH
/// WAITING" flourish (there's no rematch-offer concept in this app).
fn draw_last_match_card(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    item_count: f32,
    profile: &ProfileScreenState,
    focused: bool,
) -> Result<(), String> {
    let y = ROWS_TOP + item_count * (ROW_H + ROW_GAP) + 42.0;
    let h = 78.0;
    let w = 620.0;
    canvas.set_draw_color(Color::RGBA(14, 14, 18, 178));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(LIST_X, y, w, h)))?;
    canvas.set_draw_color(if focused {
        theme::ACCENT
    } else {
        Color::RGBA(255, 255, 255, 20)
    });
    canvas.draw_rect(scale.rect(LIST_X, y, w, h))?;

    geometry::fill_skewed_rect(canvas, scale, LIST_X + 18.0, y + 16.0, 9.0, h - 32.0, SKEW_DEG - 2.0, theme::ACCENT);

    let text_x = LIST_X + 18.0 + 9.0 + LABEL_GAP;
    let (lx, ly) = scale.point(text_x, y + 14.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "LAST MATCH", lx, ly, theme::MUTE, scale.len(4.0).round() as i32)?;

    let (main_text, sub_text) = last_match_text(profile);
    let main_px = scale.font_px(20.0);
    let (mx, my) = scale.point(text_x, y + 32.0);
    let (main_w, _) = fonts.draw(
        canvas,
        FpFont::SairaCondensedBold,
        main_px,
        &main_text,
        mx,
        my,
        Color::RGB(0xf2, 0xf2, 0xee),
    )?;
    if !sub_text.is_empty() {
        // Baseline-align the smaller date/score text against the main
        // line's own visible glyph bottom (`visible_span`, not a flat
        // guessed offset — a fixed +3px gap looked fine for one font-size
        // pairing but drifted noticeably out of line here, same root cause
        // `visible_span` was already introduced for elsewhere in this
        // screen). All-caps text with no descenders on either run, so
        // aligning visible bottoms approximates true baseline alignment.
        let sub_px = scale.font_px(15.0);
        let (main_inset, main_vis_h) = fonts.visible_span(FpFont::SairaCondensedBold, main_px, &main_text);
        let main_bottom = y + 32.0 + (main_inset + main_vis_h) as f32 / scale.s;
        let (sub_inset, sub_vis_h) = fonts.visible_span(FpFont::ChakraPetchMedium, sub_px, &sub_text);
        let sub_top = main_bottom - (sub_inset + sub_vis_h) as f32 / scale.s;
        let (sx, sy) = scale.point(text_x + (main_w as f32 / scale.s) + 10.0, sub_top);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, sub_px, &sub_text, sx, sy, Color::RGB(0x5e, 0x5e, 0x66))?;
    }
    Ok(())
}

/// The main headline/sub-label pair for the "LAST MATCH" card, derived from
/// real data only — `NotLoggedIn`/`Loading`/`Error`/`Empty` all render a
/// plain status line instead of placeholder win/loss numbers.
fn last_match_text(profile: &ProfileScreenState) -> (String, String) {
    match profile {
        ProfileScreenState::Loaded { history, .. } => match history.first() {
            Some(row) => (format_result_line(row), format!("\u{b7} {}", short_date(&row.played_at))),
            None => ("NO MATCHES RECORDED YET".to_string(), String::new()),
        },
        ProfileScreenState::Empty { .. } => ("NO MATCHES RECORDED YET".to_string(), String::new()),
        ProfileScreenState::Loading => ("LOADING\u{2026}".to_string(), String::new()),
        ProfileScreenState::Error(_) => ("STATS UNAVAILABLE".to_string(), String::new()),
        ProfileScreenState::NotLoggedIn => ("NOT SIGNED IN".to_string(), String::new()),
    }
}

fn format_result_line(row: &HistoryRow) -> String {
    let verb = if row.result == "won" { "DEFEATED" } else { "LOST TO" };
    format!(
        "{verb} {} {}-{}",
        row.opponent_username.to_uppercase(),
        row.our_score,
        row.opponent_score
    )
}

/// "YYYY-MM-DD..." (ISO8601, server-side) -> "JUN 25". Falls back to the raw
/// string if it doesn't start with a recognizable date, rather than a
/// dependency like `chrono` just for this one label (see `chrome.rs`'s
/// clock, which makes the same call for the header time-of-day).
pub(super) fn short_date(iso: &str) -> String {
    let bytes = iso.as_bytes();
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return iso.to_string();
    }
    let Ok(month) = iso[5..7].parse::<usize>() else {
        return iso.to_string();
    };
    let day = &iso[8..10];
    const MONTHS: [&str; 12] = [
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ];
    match MONTHS.get(month.wrapping_sub(1)) {
        Some(name) => format!("{name} {}", day.trim_start_matches('0')),
        None => iso.to_string(),
    }
}

/// Longest current streak from the front of `history` (most recent first).
/// Returns `(count, is_win_streak)`; `None` with no matches played yet.
fn compute_streak(history: &[HistoryRow]) -> Option<(u32, bool)> {
    let first = history.first()?;
    let is_win = first.result == "won";
    let count = history.iter().take_while(|r| (r.result == "won") == is_win).count() as u32;
    Some((count, is_win))
}

/// "YOUR STATS" card, top-right — real wins/losses (`ProfileData`) and a
/// real computed streak (`HistoryRow` list), not the mockup's fabricated
/// `profileRank`/region/node subtitle (see this module's doc comment).
fn draw_your_stats_panel(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    username: &str,
    profile: &ProfileScreenState,
    focused: bool,
) -> Result<(), String> {
    let x = theme::VW - 56.0 - 520.0;
    // header height (104) + this panel's own top:54 offset, same reasoning
    // as `LIST_TOP` — a bare `54.0` here (this module's own earlier mistake)
    // rendered the panel almost flush with the very top of the window,
    // overlapping the header instead of sitting below it.
    let y = chrome::HEADER_H + 54.0;
    let w = 520.0;
    let header_h = 60.0;
    let identity_h = 128.0;
    let stats_h = 90.0;
    let h = header_h + identity_h + stats_h;

    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(x, y, w, h)))?;
    canvas.set_draw_color(if focused {
        theme::ACCENT
    } else {
        Color::RGBA(255, 255, 255, 18)
    });
    canvas.draw_rect(scale.rect(x, y, w, h))?;

    // Header row: accent tick + "YOUR STATS".
    canvas.set_draw_color(theme::ACCENT);
    canvas.fill_rect(Some(scale.rect(x + 26.0, y + 28.0, 30.0, 3.0)))?;
    let (hx, hy) = scale.point(x + 26.0 + 30.0 + 13.0, y + 20.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "YOUR STATS", hx, hy, theme::ACCENT, scale.len(6.0).round() as i32)?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(x, y + header_h, w, 1.0)))?;

    // Identity row: avatar square + username + real rating/match-count line.
    let avatar_d = 84.0;
    let avatar_x = x + 26.0;
    let avatar_y = y + header_h + (identity_h - avatar_d) / 2.0;
    canvas.set_draw_color(Color::RGB(0x5a, 0x12, 0x17));
    canvas.fill_rect(Some(scale.rect(avatar_x, avatar_y, avatar_d, avatar_d)))?;
    let initial = username.chars().next().unwrap_or('?').to_uppercase().to_string();
    let (iw, ih) = fonts.text_size(FpFont::SairaCondensedBlack, scale.font_px(36.0), &initial);
    let (ix, iy) = scale.point(
        avatar_x + avatar_d / 2.0 - (iw as f32 / scale.s) / 2.0,
        avatar_y + avatar_d / 2.0 - (ih as f32 / scale.s) / 2.0,
    );
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(36.0), &initial, ix, iy, Color::RGB(255, 255, 255))?;

    // Name + subtitle centered as a block against the avatar's height, using
    // real rendered pixel spans rather than `size_of`'s full line height —
    // same reasoning as `draw_row`'s label/sub pair.
    let name_x = avatar_x + avatar_d + 20.0;
    let name_px = scale.font_px(34.0);
    let sub_px = scale.font_px(13.0);
    let username_text = username.to_uppercase();
    let sub_text = identity_subtitle(profile);
    let (name_inset, name_vis_h) = fonts.visible_span(FpFont::SairaCondensedBlack, name_px, &username_text);
    let (sub_inset, sub_vis_h) = fonts.visible_span(FpFont::ChakraPetchMedium, sub_px, &sub_text);
    let name_vis_h_l = name_vis_h as f32 / scale.s;
    let sub_vis_h_l = sub_vis_h as f32 / scale.s;
    let gap = 10.0;
    let block_h = name_vis_h_l + gap + sub_vis_h_l;
    let block_top = avatar_y + (avatar_d - block_h) / 2.0;

    let (nx, ny) = scale.point(name_x, block_top - name_inset as f32 / scale.s);
    fonts.draw(canvas, FpFont::SairaCondensedBlack, name_px, &username_text, nx, ny, Color::RGB(0xf4, 0xf4, 0xf0))?;
    let sub_top = block_top + name_vis_h_l + gap;
    let (sx, sy) = scale.point(name_x, sub_top - sub_inset as f32 / scale.s);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, sub_px, &sub_text, sx, sy, Color::RGB(0x7a, 0x7a, 0x82))?;

    // Stats row: WINS / LOSSES / STREAK, three equal columns.
    let stats_y = y + header_h + identity_h;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(x, stats_y, w, 1.0)))?;
    let col_w = w / 3.0;
    let (wins, losses, streak) = stat_values(profile);
    draw_stat_col(canvas, fonts, scale, x, stats_y, "WINS", &wins, Color::RGB(0x36, 0xd3, 0x99))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(x + col_w, stats_y, 1.0, stats_h)))?;
    draw_stat_col(canvas, fonts, scale, x + col_w, stats_y, "LOSSES", &losses, Color::RGB(0xf2, 0xf2, 0xee))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(x + col_w * 2.0, stats_y, 1.0, stats_h)))?;
    draw_stat_col(canvas, fonts, scale, x + col_w * 2.0, stats_y, "STREAK", &streak, theme::ACCENT)?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_stat_col(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    y: f32,
    label: &str,
    value: &str,
    value_color: Color,
) -> Result<(), String> {
    let pad = 26.0;
    let (lx, ly) = scale.point(x + pad, y + 18.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchMedium, scale.font_px(11.0), label, lx, ly, Color::RGB(0x62, 0x62, 0x6c), scale.len(3.0).round() as i32)?;
    let (vx, vy) = scale.point(x + pad, y + 36.0);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(34.0), value, vx, vy, value_color)?;
    Ok(())
}

fn identity_subtitle(profile: &ProfileScreenState) -> String {
    match profile {
        ProfileScreenState::Loaded { profile, .. } => {
            format!("RATING {} \u{b7} {} PLAYED", profile.rating, profile.matches_played)
        }
        ProfileScreenState::Empty { .. } => "NO RANKED MATCHES YET".to_string(),
        ProfileScreenState::Loading => "LOADING\u{2026}".to_string(),
        ProfileScreenState::Error(_) => "STATS UNAVAILABLE".to_string(),
        ProfileScreenState::NotLoggedIn => "NOT SIGNED IN".to_string(),
    }
}

fn stat_values(profile: &ProfileScreenState) -> (String, String, String) {
    match profile {
        ProfileScreenState::Loaded { profile, history, .. } => {
            let streak = match compute_streak(history) {
                Some((n, true)) => format!("W{n}"),
                Some((n, false)) => format!("L{n}"),
                None => "\u{2014}".to_string(),
            };
            (profile.wins.to_string(), profile.losses.to_string(), streak)
        }
        ProfileScreenState::Empty { .. } => ("0".to_string(), "0".to_string(), "\u{2014}".to_string()),
        _ => ("\u{2014}".to_string(), "\u{2014}".to_string(), "\u{2014}".to_string()),
    }
}

/// "MORTAL KOMBAT II / ARCADE · 1993 · NETPLAY FREEPLAY" caption,
/// bottom-right of the content area, in front of the giant "II" ghost
/// watermark — the mockup's own markup just says "MORTAL KOMBAT" here, but
/// this app only ever runs MK2, so the caption says so explicitly rather
/// than the franchise-level name. An earlier pass here instead drew an
/// invented "SELECTED CABINET / MORTAL KOMBAT II / ARCADE" info box that
/// doesn't exist in the current mockup; that box has been replaced by the
/// real "LAST MATCH" card (`draw_last_match_card`), which is what actually
/// sits below the item list in this version of the markup.
fn draw_cabinet_title(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, rom_present: bool) -> Result<(), String> {
    let bottom = theme::VH - chrome::FOOTER_H - 96.0;
    // No ROM found: don't claim MK2 is loaded and ready to play — real
    // data only, same reasoning as the "?" watermark next to this.
    let title = if rom_present { "MORTAL KOMBAT II" } else { "NO ROM FOUND" };
    let title_track = scale.len(11.0).round() as i32;
    let (tw, th) = fonts.text_size_tracked(FpFont::SairaCondensedBold, scale.font_px(32.0), title, title_track);
    let (tx, ty) = scale.point(theme::VW - 96.0 - (tw as f32 / scale.s), bottom - (th as f32 / scale.s));
    fonts.draw_tracked(canvas, FpFont::SairaCondensedBold, scale.font_px(32.0), title, tx, ty, Color::RGB(0xde, 0xde, 0xd8), title_track)?;

    let sub = if rom_present {
        "ARCADE \u{b7} 1993 \u{b7} NETPLAY FREEPLAY"
    } else {
"PLACE mk2.zip NEXT TO THE EXE OR IN roms\\"
    };
    let sub_track = scale.len(6.0).round() as i32;
    let (sw, _) = fonts.text_size_tracked(FpFont::ChakraPetchMedium, scale.font_px(14.0), sub, sub_track);
    let (sx, sy) = scale.point(theme::VW - 96.0 - (sw as f32 / scale.s), bottom + 8.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchMedium, scale.font_px(14.0), sub, sx, sy, Color::RGB(0x5e, 0x5e, 0x66), sub_track)?;

    // ROM identity line (FNV hash + core build tag) hidden for now per
    // direct user feedback ("not needed right now") — kept as a function,
    // not deleted, in case it comes back; same "hidden not deleted"
    // treatment as Network News elsewhere in fp_ui.
    Ok(())
}

#[allow(dead_code)]
fn rom_identity_line() -> String {
    format!("ROM {} \u{b7} CORE {}", crate::matchmaking::rom_fnv_hash(), crate::retro::core_compat_tag())
}

/// Bottom wire ticker, directly above the footer. Not called right now —
/// disabled along with the Network News row it quotes (see
/// `super::MAIN_ITEM_COUNT`'s doc comment) — kept rather than deleted since
/// it's meant to come back once/if a real bulletin backend exists. The
/// mockup scrolls this via a CSS `animation: fp-ticker 30s linear infinite`
/// marquee; reproducing that in SDL would need a per-frame time input this
/// draw call doesn't take (every other fp_ui screen is a pure function of
/// its `FpScreen` state, not wall-clock time), so this renders one static
/// line instead of a scrolling one — same text, no motion.
#[allow(dead_code)]
fn draw_ticker(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale) -> Result<(), String> {
    let h = 46.0;
    let y = theme::VH - chrome::FOOTER_H - h;
    canvas.set_draw_color(Color::RGBA(6, 6, 8, 217));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(0.0, y, theme::VW, h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(0.0, y, theme::VW, 1.0)))?;

    let text_y = y + h / 2.0 - 9.0;
    let (px, py) = scale.point(LIST_X, text_y);
    let (prefix_w, _) = fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(15.0), "WIRE //", px, py, theme::ACCENT)?;
    let rest = super::bandwidth::TICKER_TEXT.trim_start_matches("WIRE //");
    let (rx, ry) = scale.point(LIST_X + (prefix_w as f32 / scale.s), text_y);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(15.0), rest, rx, ry, Color::RGB(0x7a, 0x7a, 0x82))?;
    Ok(())
}

/// Giant near-black "II" watermark at the right edge, per the mockup's
/// ghost-text treatment: `skewX(-9deg)` (approximated here with Freetype's
/// synthetic italic slant — see `font::FpFontCache::ensure_font`'s doc
/// comment) plus a `-webkit-text-stroke` red outline. SDL2/SDL_ttf has no
/// real stroke-only text mode, so the outline is approximated by stamping
/// the same glyph in accent color at several small offsets behind the
/// near-black fill — a cheap poor-man's outline, not a true vector stroke,
/// but it reads as a colored rim at this glyph's size.
fn draw_ghost_watermark(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, rom_present: bool) -> Result<(), String> {
    // No ROM found: "?" instead of the "II" mark, matching legacy's own
    // "hard-fail rather than pretend the game is there" stance — this
    // screen otherwise implies MK2 is loaded and ready (the "II" mark, the
    // "MORTAL KOMBAT" caption below it) when it may not actually be.
    let text = if rom_present { "II" } else { "?" };
    let px = scale.font_px(720.0);
    let (w, h) = fonts.text_size(FpFont::SairaCondensedBlack, px, text);
    let (x, y) = scale.point(
        theme::VW + 30.0 - (w as f32 / scale.s),
        theme::VH / 2.0 - (h as f32 / scale.s) / 2.0,
    );
    let stroke_color = Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 82);
    let r = scale.len(1.0).round().max(1.0) as i32;
    for (dx, dy) in [(-r, -r), (0, -r), (r, -r), (-r, 0), (r, 0), (-r, r), (0, r), (r, r)] {
        fonts.draw_italic(canvas, FpFont::SairaCondensedBlack, px, text, x + dx, y + dy, stroke_color)?;
    }
    fonts.draw_italic(canvas, FpFont::SairaCondensedBlack, px, text, x, y, Color::RGB(0x0c, 0x0c, 0x11))?;
    Ok(())
}
