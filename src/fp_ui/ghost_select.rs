//! Load Drones — native redesign of legacy's `MenuScreen::GhostSelect`
//! (reached from the Lab submenu's "LOAD DRONES" row, or directly from the
//! Play submenu's "DRONES" row). Real data only: local `.ncgh` recordings
//! scanned from disk, plus community recordings from freeplay-stats — both
//! populated/drained by `main.rs` exactly the way it already does for the
//! legacy screen (see `FpResult::OpenGhostSelect`/`LoadGhost`/
//! `DownloadGhost`), just targeting this screen's `entries`/`status` fields
//! instead. Picking an entry still hands off to the same
//! `ghost::Playback`/`start_logic_ghost_opponent` pipeline — only the
//! chooser UI is native, per explicit user direction (the actual Lab/Drone
//! in-game trainer overlay stays legacy).
//!
//! Layout matches the current mockup's `isDrones` branch: LOCAL/REMOTE
//! section tabs over a two-pane list + detail sidebar (LOAD DRONE button),
//! not the single flat list an earlier pass here built. The mockup's
//! FIGHTER column is dropped — neither `GhostEntry::Local` nor
//! `RemoteGhostMeta` carries which fighter a recording used, and inventing
//! it would be fabricated rather than derived (unlike LENGTH below). LENGTH
//! *is* shown for both sections: computed from each entry's real
//! `frame_count` at MK2's fixed ~54.7Hz frame rate, not fabricated —
//! `GhostEntry::Local::frame_count` is read straight from the file's own
//! header (`ghost::read_ncgh_frame_count`) at scan time, the same real data
//! `RemoteGhostMeta::frame_count` already had.

use super::chrome::{self, FooterRight};
use super::geometry;
use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use crate::menu::GhostEntry;
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

const SIDE_PAD: f32 = 56.0;
const TOP: f32 = 38.0 + 104.0;
const PANEL_H: f32 = 470.0;
const SIDEBAR_W: f32 = 330.0;
const GAP: f32 = 20.0;
/// MK2's fixed native frame rate (18281us/frame, see `main.rs`'s frame
/// timing doc) — used to derive a real mm:ss length from a real frame count.
const FPS: f32 = 1_000_000.0 / 18281.0;

pub fn draw(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    cursor: usize,
    section: usize,
    entries: &[GhostEntry],
    status: Option<&str>,
    username: &str,
) -> Result<(), String> {
    chrome::draw_header(canvas, fonts, scale, username, true, None)?;

    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, TOP + 8.0, 30.0, 3.0)))?;
    let (ex, ey) = scale.point(SIDE_PAD + 44.0, TOP);
    fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(13.0), "GHOST INPUTS", ex, ey, theme::ACCENT)?;
    let (tx, ty) = scale.point(SIDE_PAD, TOP + 26.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(58.0), "DRONES", tx, ty, theme::TEXT)?;

    let hint = "F9 DURING MATCH TO RECORD";
    let (hw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(12.0), hint);
    let (hx, hy) = scale.point(theme::VW - SIDE_PAD - (hw as f32 / scale.s), TOP + 40.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(12.0), hint, hx, hy, Color::RGB(0x3a, 0x3a, 0x42))?;

    let tabs_y = TOP + 26.0 + 70.0;
    let tab_w = 160.0;
    let tab_h = 44.0;
    for (i, label) in ["LOCAL", "REMOTE"].iter().enumerate() {
        draw_tab(canvas, fonts, scale, SIDE_PAD + i as f32 * (tab_w + 5.0), tabs_y, tab_w, tab_h, label, i == section)?;
    }
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 26));
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, tabs_y + tab_h + 6.0, 1808.0, 1.0)))?;

    let body_top = tabs_y + tab_h + 24.0;
    let list_w = 1808.0 - SIDEBAR_W - GAP;

    let idxs: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!((section, e), (0, GhostEntry::Local { .. }) | (1, GhostEntry::Remote(_))))
        .map(|(i, _)| i)
        .collect();

    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, body_top, list_w, PANEL_H)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(SIDE_PAD, body_top, list_w, PANEL_H))?;

    if idxs.is_empty() {
        let msg = status.unwrap_or(if section == 0 { "No local drone recordings found" } else { "No shared drones available" });
        draw_status(canvas, fonts, scale, body_top, PANEL_H, list_w, msg)?;
    } else {
        draw_list(canvas, fonts, scale, body_top, list_w, section, entries, &idxs, cursor)?;
    }

    // `cursor` is only a valid selection within the *current* section once
    // nav() has scoped it there (see `FpScreen::GhostSelect`'s doc comment)
    // — on first entering the screen, before any Up/Down/section-switch
    // event, it can still be pointing at an index from the other section
    // (e.g. section is LOCAL but there are zero local files, so `cursor: 0`
    // is actually the first REMOTE entry). Guard against showing that
    // mismatched entry mislabeled with this section's source text.
    let side_x = SIDE_PAD + list_w + GAP;
    canvas.set_draw_color(Color::RGBA(8, 8, 11, 140));
    canvas.fill_rect(Some(scale.rect(side_x, body_top, SIDEBAR_W, PANEL_H)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.draw_rect(scale.rect(side_x, body_top, SIDEBAR_W, PANEL_H))?;
    let selected = if idxs.contains(&cursor) { entries.get(cursor) } else { None };
    draw_sidebar(canvas, fonts, scale, side_x, body_top, section, selected)?;

    chrome::draw_footer(
        canvas,
        fonts,
        scale,
        &[
            chrome::FooterPrompt { glyph: "\u{2195}", label: "Drone", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::FooterPrompt { glyph: "L1/R1", label: "Section", color: Color::RGB(0xcf, 0xcf, 0xc9) },
            chrome::PROMPT_SELECT,
            chrome::PROMPT_BACK,
        ],
        FooterRight::Text("GHOST TRAINING \u{b7} F9 TO RECORD DURING MATCH"),
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_tab(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    active: bool,
) -> Result<(), String> {
    let color = if active { theme::ACCENT } else { Color::RGBA(255, 255, 255, 8) };
    geometry::fill_skewed_rect(canvas, scale, x, y, w, h, -11.0, color);
    let text_color = if active { Color::RGB(255, 255, 255) } else { Color::RGB(0x7a, 0x7a, 0x82) };
    let (tw, th) = fonts.text_size(FpFont::SairaCondensedBlack, scale.font_px(18.0), label);
    let (lx, ly) = scale.point(x + w / 2.0 - (tw as f32 / scale.s) / 2.0, y + h / 2.0 - (th as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBlack, scale.font_px(18.0), label, lx, ly, text_color)?;
    Ok(())
}

fn draw_status(canvas: &mut Canvas<Window>, fonts: &mut FpFontCache, scale: &Scale, top: f32, h: f32, w: f32, text: &str) -> Result<(), String> {
    let (tw, _) = fonts.text_size(FpFont::SairaSemiBold, scale.font_px(15.0), text);
    let (x, y) = scale.point(SIDE_PAD + (w - tw as f32 / scale.s) / 2.0, top + h / 2.0 - 8.0);
    fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), text, x, y, theme::DIM)?;
    Ok(())
}

fn fmt_length(frame_count: u32) -> String {
    let secs = (frame_count as f32 / FPS).round() as u32;
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[allow(clippy::too_many_arguments)]
fn draw_list(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    top: f32,
    w: f32,
    section: usize,
    entries: &[GhostEntry],
    idxs: &[usize],
    cursor: usize,
) -> Result<(), String> {
    let header_h = 40.0;
    let col_a_label = if section == 0 { "FILENAME" } else { "CREATOR" };
    let cols = [SIDE_PAD + 20.0, SIDE_PAD + w - 260.0, SIDE_PAD + w - 140.0];
    for (label, x) in [(col_a_label, cols[0]), ("FRAMES", cols[1]), ("LENGTH", cols[2])] {
        let (hx, hy) = scale.point(x, top + 14.0);
        fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), label, hx, hy, Color::RGB(0x3a, 0x3a, 0x42), scale.len(2.0).round() as i32)?;
    }
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
    canvas.fill_rect(Some(scale.rect(SIDE_PAD, top + header_h, w, 1.0)))?;

    let row_h = 60.0;
    for (row, &i) in idxs.iter().enumerate() {
        let y = top + header_h + row as f32 * row_h;
        let selected = i == cursor;
        if selected {
            canvas.set_draw_color(Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 22));
            canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, w, row_h)))?;
            canvas.set_draw_color(theme::ACCENT);
            canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, 4.0, row_h)))?;
        } else if row > 0 {
            canvas.set_draw_color(Color::RGBA(255, 255, 255, 10));
            canvas.fill_rect(Some(scale.rect(SIDE_PAD, y, w, 1.0)))?;
        }

        let (name, frames, length) = match &entries[i] {
            GhostEntry::Local { filename, frame_count, .. } => (filename.clone(), *frame_count, fmt_length(*frame_count)),
            GhostEntry::Remote(meta) => (meta.username.clone(), meta.frame_count, fmt_length(meta.frame_count)),
        };
        let (nx, ny) = scale.point(cols[0], y + row_h / 2.0 - 9.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(18.0), &name, nx, ny, Color::RGB(0xED, 0xED, 0xE8))?;
        let (fx, fy) = scale.point(cols[1], y + row_h / 2.0 - 7.0);
        fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(14.0), &frames.to_string(), fx, fy, Color::RGB(0x8a, 0x8a, 0x92))?;
        let (lx, ly) = scale.point(cols[2], y + row_h / 2.0 - 8.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(15.0), &length, lx, ly, Color::RGB(0xcf, 0xcf, 0xc9))?;
    }
    Ok(())
}

fn draw_sidebar(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    top: f32,
    section: usize,
    selected: Option<&GhostEntry>,
) -> Result<(), String> {
    let pad = 22.0;
    let (hx, hy) = scale.point(x + pad, top + 22.0);
    fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), "SELECTED DRONE", hx, hy, Color::RGB(0x3a, 0x3a, 0x42), scale.len(4.0).round() as i32)?;

    let Some(entry) = selected else {
        let (mx, my) = scale.point(x + pad, top + 60.0);
        fonts.draw(canvas, FpFont::SairaSemiBold, scale.font_px(15.0), "No drone selected", mx, my, theme::DIM)?;
        return Ok(());
    };

    let (name, frame_count) = match entry {
        GhostEntry::Local { filename, frame_count, .. } => (filename.clone(), *frame_count),
        GhostEntry::Remote(meta) => (meta.username.clone(), meta.frame_count),
    };
    let (nx, ny) = scale.point(x + pad, top + 56.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(24.0), &name, nx, ny, theme::TEXT)?;

    let source_label = if section == 0 { "LOCAL RECORDING" } else { "SHARED RECORDING" };
    let (sx, sy) = scale.point(x + pad, top + 92.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(12.0), source_label, sx, sy, Color::RGB(0x52, 0x52, 0x5a))?;

    let rows: [(&str, String); 2] = [
        ("FRAMES", frame_count.to_string()),
        ("RECORDING TIME", fmt_length(frame_count)),
    ];
    let mut ry = top + 132.0;
    for (label, value) in rows {
        let (lx, ly) = scale.point(x + pad, ry);
        fonts.draw_tracked(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(11.0), label, lx, ly, Color::RGB(0x3a, 0x3a, 0x42), scale.len(2.0).round() as i32)?;
        let (vw, _) = fonts.text_size(FpFont::ChakraPetchSemiBold, scale.font_px(14.0), &value);
        let (vx, vy) = scale.point(x + SIDEBAR_W - pad - (vw as f32 / scale.s), ry - 2.0);
        fonts.draw(canvas, FpFont::ChakraPetchSemiBold, scale.font_px(14.0), &value, vx, vy, Color::RGB(0xcf, 0xcf, 0xc9))?;
        ry += 30.0;
    }

    let btn_y = top + PANEL_H - 78.0;
    let btn_w = SIDEBAR_W - pad * 2.0;
    let btn_h = 48.0;
    canvas.set_draw_color(theme::ACCENT);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.fill_rect(Some(scale.rect(x + pad, btn_y, btn_w, btn_h)))?;
    let label = "LOAD DRONE";
    let (lw, lh) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(20.0), label);
    let (ltx, lty) = scale.point(x + pad + btn_w / 2.0 - (lw as f32 / scale.s) / 2.0, btn_y + btn_h / 2.0 - (lh as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(20.0), label, ltx, lty, Color::RGB(255, 255, 255))?;

    // Not the mockup's literal "Starts character select" hint — this app
    // has no character-select step anywhere (Arcade boots straight to the
    // ROM too, see `play_menu.rs`'s doc comment); loading a drone does the
    // same, straight into gameplay with the drone as P2.
    let sub = "Boots straight into gameplay \u{b7} drone plays as P2";
    let (sw, _) = fonts.text_size(FpFont::ChakraPetchMedium, scale.font_px(11.0), sub);
    let (subx, suby) = scale.point(x + pad + btn_w / 2.0 - (sw as f32 / scale.s) / 2.0, btn_y + btn_h + 10.0);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, scale.font_px(11.0), sub, subx, suby, Color::RGB(0x3a, 0x3a, 0x42))?;
    Ok(())
}
