//! Shared native on-screen keyboard — the modal from the updated mockup's
//! `oskOpen` branch, replacing the legacy `MenuScreen::TextEdit` *rendering*
//! (not its state machine) whenever the edit was opened from an fp_ui
//! screen. One design, all TextEdit uses: Settings→Account Username / Stats
//! Email, Host/Join's join code (the six-cell variant), Replays→Edit Note,
//! and Lobby Chat compose.
//!
//! The state stays `AppState::Menu(MenuScreen::TextEdit { .. })` — value
//! filtering (`AppState::text_input`), commit side effects
//! (`NavResult::CommitText`), and the `came_from` round trip are all the
//! proven legacy machinery. This module adds two things on top:
//!
//! 1. `draw_modal`: the mockup's card, drawn *over* the dimmed `came_from`
//!    fp screen (main.rs draws that first via the normal `fp_ui::draw`).
//! 2. A controller-driven key grid (`wants_event`/`apply`): D-pad moves a
//!    grid cursor, Cross presses the highlighted key. Hardware keyboard
//!    keeps working exactly as before (typing, Enter=commit, Esc=cancel) —
//!    the grid is the gamepad path, not the only path, matching the
//!    mockup's own "A HARDWARE KEYBOARD ALSO WORKS" hint. The cursor lives
//!    in a main.rs local (`fp_osk`), not in `TextEdit` itself, so legacy
//!    construction sites stay untouched; only one edit can be active at a
//!    time so a single slot is enough.
//!
//! Pressing Cross on the CONFIRM cell is deliberately *not* intercepted
//! (`wants_event` returns false): the event falls through to the normal
//! `MenuNav::Accept` translation, hitting the same `NavResult::CommitText`
//! path Enter does, so commit behavior can't drift between input methods.

use super::layout::Scale;
use super::theme;
use crate::font::{FpFont, FpFontCache};
use crate::menu::EditField;
use sdl2::event::Event;
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::video::Window;

/// The mockup's `oskKeyDefs`: four rows of ten characters. Used for names,
/// join codes and email — anything alphanumeric.
const ALPHA_ROWS: &[&[char]] = &[
    &['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J'],
    &['K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T'],
    &['U', 'V', 'W', 'X', 'Y', 'Z', '0', '1', '2', '3'],
    &['4', '5', '6', '7', '8', '9', '_', '-', '.', '@'],
];

/// Chat is the one field that accepts arbitrary text, so it gets the
/// punctuation a sentence needs. Without this row a player could only send
/// words — no question marks, no apostrophes.
const CHAT_ROWS: &[&[char]] = &[
    &['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J'],
    &['K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T'],
    &['U', 'V', 'W', 'X', 'Y', 'Z', '0', '1', '2', '3'],
    &['4', '5', '6', '7', '8', '9', '.', ',', '!', '?'],
    &['\'', '"', ':', ';', '-', '_', '(', ')', '@', '/'],
];

/// An address is digits, a dot and a colon — 26 dead letters is a worse way
/// to type one than a pad you can cross in two presses.
const NUMPAD_ROWS: &[&[char]] = &[
    &['1', '2', '3'],
    &['4', '5', '6'],
    &['7', '8', '9'],
    &['.', '0', ':'],
];

/// A control-row cell. The character rows vary per field, and so does what
/// belongs under them: a number pad has nothing to shift and no space to
/// insert.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Ctl {
    Shift,
    Space,
    Backspace,
}

impl Ctl {
    fn label(self, shift: bool) -> &'static str {
        match self {
            // The key states what it will do next, not what it is — "abc"
            // while shifted reads as "press for lowercase".
            Ctl::Shift if shift => "abc",
            Ctl::Shift => "ABC",
            Ctl::Space => "SPACE",
            // "BACKSPACE" as text — the mockup's Unicode glyph isn't in any
            // bundled font.
            Ctl::Backspace => "BACKSPACE",
        }
    }

    /// Share of the control row's width, so SPACE stays the wide one.
    fn weight(self) -> f32 {
        match self {
            Ctl::Space => 3.0,
            _ => 1.0,
        }
    }
}

pub struct Layout {
    rows: &'static [&'static [char]],
    /// What sits under the character grid. A number pad has nothing to
    /// shift and no space to insert, so it carries neither key.
    controls: &'static [Ctl],
}

const ALPHA: Layout = Layout {
    rows: ALPHA_ROWS,
    controls: &[Ctl::Shift, Ctl::Space, Ctl::Backspace],
};
const CHAT: Layout = Layout {
    rows: CHAT_ROWS,
    controls: &[Ctl::Shift, Ctl::Space, Ctl::Backspace],
};
const NUMPAD: Layout = Layout {
    rows: NUMPAD_ROWS,
    controls: &[Ctl::Backspace],
};

pub fn layout_for(field: &EditField) -> &'static Layout {
    match field {
        EditField::TestConnAddress => &NUMPAD,
        EditField::ChatMessage => &CHAT,
        _ => &ALPHA,
    }
}

impl Layout {
    pub fn char_rows(&self) -> usize {
        self.rows.len()
    }
    /// Derived from `controls` rather than stored, so a layout cannot claim
    /// to shift while offering no key to do it with.
    pub fn shifts(&self) -> bool {
        self.controls.contains(&Ctl::Shift)
    }
    fn control_row(&self) -> usize {
        self.rows.len()
    }
    fn button_row(&self) -> usize {
        self.rows.len() + 1
    }
    fn row_count(&self) -> usize {
        self.rows.len() + 2
    }
    /// The widest character row, which sets the key size for all of them.
    pub fn max_cols(&self) -> usize {
        self.rows.iter().map(|r| r.len()).max().unwrap_or(1)
    }
    pub fn cols_in_row(&self, row: usize) -> usize {
        if row < self.char_rows() {
            self.rows[row].len()
        } else if row == self.control_row() {
            self.controls.len()
        } else {
            2
        }
    }
    /// The character a cell types. Shift applies only to letters, so digits
    /// and punctuation are unaffected by the toggle.
    pub fn char_at(&self, row: usize, col: usize, shift: bool) -> char {
        let r = self.rows[row];
        let c = r[col.min(r.len() - 1)];
        if shift {
            c
        } else {
            c.to_ascii_lowercase()
        }
    }
}

/// Map a column between rows with different cell counts so vertical movement
/// lands on the visually-nearest cell. Done proportionally rather than with
/// hardcoded halves, since row widths now vary per layout.
fn remap_col(layout: &Layout, col: usize, from_row: usize, to_row: usize) -> usize {
    let from = layout.cols_in_row(from_row).max(1);
    let to = layout.cols_in_row(to_row).max(1);
    if from == to {
        return col.min(to - 1);
    }
    // Centre of the source cell, rescaled into the destination row.
    let fraction = (col as f32 + 0.5) / from as f32;
    ((fraction * to as f32) as usize).min(to - 1)
}

/// What a grid press asks main.rs to do. `Moved` covers the D-pad;
/// everything else is a Cross press on the corresponding cell. CONFIRM has
/// no variant — see the module doc for why that press falls through.
pub enum OskAction {
    Moved,
    Char(char),
    Space,
    Backspace,
    Cancel,
}

fn is_confirm_cell(layout: &Layout, cursor: (usize, usize)) -> bool {
    cursor.0 == layout.button_row() && cursor.1 == 1
}

/// Should this controller event be handled by the OSK grid rather than the
/// normal menu-nav translation? Keyboard events are never claimed.
pub fn wants_event(event: &Event, cursor: (usize, usize), field: &EditField) -> bool {
    use sdl2::controller::Button;
    let layout = layout_for(field);
    match event {
        Event::ControllerButtonDown { button, .. } => match button {
            Button::DPadUp | Button::DPadDown | Button::DPadLeft | Button::DPadRight => true,
            Button::A => !is_confirm_cell(layout, cursor),
            _ => false,
        },
        _ => false,
    }
}

/// Apply a claimed controller event (see `wants_event`) to the grid cursor,
/// returning the resulting action for main.rs to perform on the TextEdit
/// state (via the same `text_input`/`text_backspace`/`nav_back` methods the
/// hardware keyboard path uses).
pub fn apply(
    event: &Event,
    cursor: &mut (usize, usize),
    shift: &mut bool,
    field: &EditField,
) -> OskAction {
    use sdl2::controller::Button;
    let layout = layout_for(field);
    // A cursor carried over from a different layout could sit outside this
    // one; clamp before it indexes anything.
    cursor.0 = cursor.0.min(layout.row_count() - 1);
    cursor.1 = cursor.1.min(layout.cols_in_row(cursor.0).saturating_sub(1));

    let Event::ControllerButtonDown { button, .. } = event else {
        return OskAction::Moved;
    };
    match button {
        Button::DPadUp => {
            if cursor.0 > 0 {
                cursor.1 = remap_col(layout, cursor.1, cursor.0, cursor.0 - 1);
                cursor.0 -= 1;
            }
            OskAction::Moved
        }
        Button::DPadDown => {
            if cursor.0 + 1 < layout.row_count() {
                cursor.1 = remap_col(layout, cursor.1, cursor.0, cursor.0 + 1);
                cursor.0 += 1;
            }
            OskAction::Moved
        }
        Button::DPadLeft => {
            cursor.1 = cursor.1.saturating_sub(1);
            OskAction::Moved
        }
        Button::DPadRight => {
            cursor.1 = (cursor.1 + 1).min(layout.cols_in_row(cursor.0) - 1);
            OskAction::Moved
        }
        Button::A => {
            if cursor.0 < layout.char_rows() {
                OskAction::Char(layout.char_at(cursor.0, cursor.1, *shift))
            } else if cursor.0 == layout.control_row() {
                match layout.controls.get(cursor.1) {
                    Some(Ctl::Shift) => {
                        *shift = !*shift;
                        OskAction::Moved
                    }
                    Some(Ctl::Space) => OskAction::Space,
                    Some(Ctl::Backspace) => OskAction::Backspace,
                    None => OskAction::Moved,
                }
            } else {
                // Button row col 0; col 1 is CONFIRM and never reaches here
                // (`wants_event` leaves it to the normal Accept path).
                OskAction::Cancel
            }
        }
        _ => OskAction::Moved,
    }
}

/// Would this character survive the field's own `text_input` filter?
/// Drawing mirror of `AppState::text_input`'s per-field rules, used to dim
/// keys that would be no-ops (e.g. `@` while entering a join code).
fn char_allowed(field: &EditField, c: char) -> bool {
    match field {
        EditField::Username | EditField::ClaimName => {
            c.is_ascii_alphanumeric() || c == '_' || c == '-'
        }
        EditField::JoinCode => c.is_ascii_alphanumeric(),
        EditField::TestConnAddress => c.is_ascii_digit() || c == '.' || c == ':',
        EditField::StatsEmail | EditField::ReplayNote { .. } | EditField::ChatMessage => true,
    }
}

const CARD_W: f32 = 820.0;
const PAD_X: f32 = 38.0;
const KEY_GAP: f32 = 6.0;
const KEY_H: f32 = 44.0;
const BTN_H: f32 = 48.0;
const CELL_W: f32 = 64.0;
const CELL_H: f32 = 78.0;

/// Draw the OSK card over whatever the caller already drew (the dimmed
/// `came_from` screen). Does *not* clear the canvas or reset the font
/// cache — `fp_ui::draw` for the underlying screen already did both this
/// frame at the same scale.
#[allow(clippy::too_many_arguments)]
pub fn draw_modal(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    title: &str,
    label: &str,
    value: &str,
    field: &EditField,
    cursor: (usize, usize),
    shift: bool,
) -> Result<(), String> {
    let is_cells = matches!(field, EditField::JoinCode);
    let layout = layout_for(field);

    let grid_w = CARD_W - PAD_X * 2.0;
    let input_h = if is_cells { CELL_H } else { 58.0 };
    let grid_h = layout.char_rows() as f32 * (KEY_H + KEY_GAP) + KEY_H;
    let card_h = 30.0 + 22.0 + 24.0 + input_h + 20.0 + grid_h + 18.0 + BTN_H + 16.0 + 16.0 + 30.0;
    let card_x = (theme::VW - CARD_W) / 2.0;
    let card_y = (theme::VH - card_h) / 2.0;

    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(2, 2, 4, 184));
    canvas.fill_rect(Some(scale.rect(0.0, 0.0, theme::VW, theme::VH)))?;

    canvas.set_draw_color(Color::RGBA(10, 10, 13, 235));
    canvas.fill_rect(Some(scale.rect(card_x, card_y, CARD_W, card_h)))?;
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 26));
    canvas.draw_rect(scale.rect(card_x, card_y, CARD_W, card_h))?;
    canvas.set_draw_color(theme::ACCENT);
    canvas.fill_rect(Some(scale.rect(card_x, card_y, CARD_W, 5.0)))?;

    let content_x = card_x + PAD_X;
    let mut y = card_y + 30.0;

    let (ex, ey) = scale.point(content_x, y);
    fonts.draw_tracked(
        canvas,
        FpFont::ChakraPetchSemiBold,
        scale.font_px(13.0),
        title,
        ex,
        ey,
        theme::ACCENT,
        scale.len(5.0).round() as i32,
    )?;
    y += 22.0;
    let (lx, ly) = scale.point(content_x, y);
    fonts.draw(canvas, FpFont::SairaMedium, scale.font_px(15.0), label, lx, ly, Color::RGB(0x9a, 0x9a, 0xa2))?;
    y += 24.0;

    // Caret blink: visible for the first half of each 1s cycle, matching
    // the mockup's `fp-caret 1s step-end`.
    let caret_on = super::matchmaking::elapsed_ms() % 1000 < 500;

    if is_cells {
        // Six large character cells (join code).
        let filled = value.chars().count();
        for i in 0..6usize {
            let cx = content_x + i as f32 * (CELL_W + 10.0);
            let is_caret_cell = i == filled;
            canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
            canvas.set_draw_color(if is_caret_cell {
                Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 26)
            } else {
                Color::RGBA(255, 255, 255, 6)
            });
            canvas.fill_rect(Some(scale.rect(cx, y, CELL_W, CELL_H)))?;
            canvas.set_draw_color(if is_caret_cell {
                Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 166)
            } else if i < filled {
                Color::RGBA(255, 255, 255, 46)
            } else {
                Color::RGBA(255, 255, 255, 23)
            });
            canvas.draw_rect(scale.rect(cx, y, CELL_W, CELL_H))?;
            if let Some(c) = value.chars().nth(i) {
                let s = c.to_string();
                let px = scale.font_px(38.0);
                let (cw, ch) = fonts.text_size(FpFont::ChakraPetchSemiBold, px, &s);
                let (tx, ty) = scale.point(
                    cx + CELL_W / 2.0 - (cw as f32 / scale.s) / 2.0,
                    y + CELL_H / 2.0 - (ch as f32 / scale.s) / 2.0,
                );
                fonts.draw(canvas, FpFont::ChakraPetchSemiBold, px, &s, tx, ty, theme::TEXT)?;
            } else if is_caret_cell && caret_on {
                canvas.set_draw_color(theme::ACCENT);
                canvas.fill_rect(Some(scale.rect(cx + CELL_W / 2.0 - 1.5, y + CELL_H / 2.0 - 17.0, 3.0, 34.0)))?;
            }
        }
    } else {
        // Single-line value box with a trailing caret.
        canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
        canvas.set_draw_color(Color::RGBA(255, 255, 255, 8));
        canvas.fill_rect(Some(scale.rect(content_x, y, grid_w, input_h)))?;
        canvas.set_draw_color(Color::RGBA(255, 255, 255, 36));
        canvas.draw_rect(scale.rect(content_x, y, grid_w, input_h))?;
        let px = scale.font_px(24.0);
        let (vw, vh) = fonts.text_size(FpFont::ChakraPetchSemiBold, px, value);
        let text_y = y + input_h / 2.0 - (vh as f32 / scale.s).max(24.0) / 2.0;
        let (vx, vy) = scale.point(content_x + 14.0, text_y);
        if !value.is_empty() {
            fonts.draw(canvas, FpFont::ChakraPetchSemiBold, px, value, vx, vy, theme::TEXT)?;
        }
        if caret_on {
            let caret_x = content_x + 14.0 + if value.is_empty() { 0.0 } else { vw as f32 / scale.s + 4.0 };
            canvas.set_draw_color(theme::ACCENT);
            canvas.fill_rect(Some(scale.rect(caret_x, y + input_h / 2.0 - 13.0, 3.0, 26.0)))?;
        }
        let _ = vy;
    }
    y += input_h + 20.0;

    // Character grid. Key width comes from the layout's widest row and is
    // capped, so a 3-wide number pad stays pad-sized instead of stretching
    // three keys across the whole card. Narrower rows centre under it.
    let cols = layout.max_cols() as f32;
    let key_w = ((grid_w - (cols - 1.0) * KEY_GAP) / cols).min(96.0);
    let grid_span = cols * key_w + (cols - 1.0) * KEY_GAP;
    let grid_x = content_x + (grid_w - grid_span) / 2.0;
    for (r, row) in layout.rows.iter().enumerate() {
        let row_span = row.len() as f32 * key_w + (row.len() as f32 - 1.0) * KEY_GAP;
        let row_x = grid_x + (grid_span - row_span) / 2.0;
        for c in 0..row.len() {
            // Via the layout, not the raw row, so a cell always shows the
            // character it actually types — including its case.
            let ch = layout.char_at(r, c, shift);
            let kx = row_x + c as f32 * (key_w + KEY_GAP);
            let selected = cursor == (r, c);
            let allowed = char_allowed(field, ch);
            draw_key(
                canvas,
                fonts,
                scale,
                kx,
                y,
                key_w,
                KEY_H,
                &ch.to_string(),
                FpFont::SairaCondensedBold,
                19.0,
                selected,
                allowed,
            )?;
        }
        y += KEY_H + KEY_GAP;
    }
    // Control row, laid out by weight so SPACE stays the wide one whatever
    // else sits beside it. Spans the character grid, so a number pad's lone
    // BACKSPACE sits under the pad rather than across the card.
    let space_allowed = !matches!(
        field,
        EditField::JoinCode
            | EditField::Username
            | EditField::ClaimName
            | EditField::TestConnAddress
    );
    let total_weight: f32 = layout.controls.iter().map(|c| c.weight()).sum();
    let gaps = (layout.controls.len() as f32 - 1.0) * KEY_GAP;
    let usable = grid_span - gaps;
    let mut cx = grid_x;
    for (i, ctl) in layout.controls.iter().enumerate() {
        let w = usable * (ctl.weight() / total_weight);
        let allowed = match ctl {
            Ctl::Space => space_allowed,
            _ => true,
        };
        draw_key(
            canvas,
            fonts,
            scale,
            cx,
            y,
            w,
            KEY_H,
            ctl.label(shift),
            FpFont::ChakraPetchSemiBold,
            12.0,
            cursor == (layout.control_row(), i),
            allowed,
        )?;
        cx += w + KEY_GAP;
    }
    y += KEY_H + 18.0;

    // CANCEL / CONFIRM.
    let btn_w = (grid_w - 14.0) / 2.0;
    let cancel_sel = cursor == (layout.button_row(), 0);
    let confirm_sel = cursor == (layout.button_row(), 1);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.set_draw_color(if cancel_sel { Color::RGBA(255, 255, 255, 20) } else { Color::RGBA(0, 0, 0, 0) });
    if cancel_sel {
        canvas.fill_rect(Some(scale.rect(content_x, y, btn_w, BTN_H)))?;
    }
    canvas.set_draw_color(if cancel_sel { Color::RGBA(255, 255, 255, 128) } else { Color::RGBA(255, 255, 255, 36) });
    canvas.draw_rect(scale.rect(content_x, y, btn_w, BTN_H))?;
    let (cw, chh) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(17.0), "CANCEL");
    let (ctx, cty) = scale.point(content_x + btn_w / 2.0 - (cw as f32 / scale.s) / 2.0, y + BTN_H / 2.0 - (chh as f32 / scale.s) / 2.0);
    fonts.draw(canvas, FpFont::SairaCondensedBold, scale.font_px(17.0), "CANCEL", ctx, cty, if cancel_sel { theme::TEXT } else { Color::RGB(0x9a, 0x9a, 0xa2) })?;

    let confirm_x = content_x + btn_w + 14.0;
    canvas.set_draw_color(if confirm_sel {
        theme::ACCENT
    } else {
        Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 90)
    });
    if confirm_sel {
        canvas.fill_rect(Some(scale.rect(confirm_x, y, btn_w, BTN_H)))?;
    }
    canvas.draw_rect(scale.rect(confirm_x, y, btn_w, BTN_H))?;
    let (fw, fh) = fonts.text_size(FpFont::SairaCondensedBold, scale.font_px(17.0), "CONFIRM");
    let (ftx, fty) = scale.point(confirm_x + btn_w / 2.0 - (fw as f32 / scale.s) / 2.0, y + BTN_H / 2.0 - (fh as f32 / scale.s) / 2.0);
    fonts.draw(
        canvas,
        FpFont::SairaCondensedBold,
        scale.font_px(17.0),
        "CONFIRM",
        ftx,
        fty,
        if confirm_sel { Color::RGB(255, 255, 255) } else { Color::RGBA(255, 255, 255, 200) },
    )?;
    y += BTN_H + 16.0;

    // "ABC"/"abc" on a key is not self-evidently a case toggle, so say so
    // where one exists.
    let hint = if layout.shifts() {
        "CIRCLE TO CANCEL \u{b7} ABC SWITCHES CASE \u{b7} A HARDWARE KEYBOARD ALSO WORKS"
    } else {
        "CIRCLE TO CANCEL \u{b7} A HARDWARE KEYBOARD ALSO WORKS"
    };
    let hint_px = scale.font_px(11.0);
    let (hw, _) = fonts.text_size(FpFont::ChakraPetchMedium, hint_px, hint);
    let (hx, hy) = scale.point(card_x + CARD_W / 2.0 - (hw as f32 / scale.s) / 2.0, y);
    fonts.draw(canvas, FpFont::ChakraPetchMedium, hint_px, hint, hx, hy, Color::RGB(0x3a, 0x3a, 0x42))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_key(
    canvas: &mut Canvas<Window>,
    fonts: &mut FpFontCache,
    scale: &Scale,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    font: FpFont,
    px: f32,
    selected: bool,
    allowed: bool,
) -> Result<(), String> {
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    canvas.set_draw_color(if selected {
        Color::RGBA(theme::ACCENT.r, theme::ACCENT.g, theme::ACCENT.b, 46)
    } else {
        Color::RGBA(255, 255, 255, 9)
    });
    canvas.fill_rect(Some(scale.rect(x, y, w, h)))?;
    canvas.set_draw_color(if selected {
        theme::ACCENT
    } else {
        Color::RGBA(255, 255, 255, 20)
    });
    canvas.draw_rect(scale.rect(x, y, w, h))?;
    let fpx = scale.font_px(px);
    let (tw, th) = fonts.text_size(font, fpx, label);
    let (tx, ty) = scale.point(x + w / 2.0 - (tw as f32 / scale.s) / 2.0, y + h / 2.0 - (th as f32 / scale.s) / 2.0);
    let color = if selected {
        Color::RGB(255, 255, 255)
    } else if allowed {
        Color::RGB(0xcf, 0xcf, 0xc9)
    } else {
        Color::RGB(0x4a, 0x4a, 0x52)
    };
    fonts.draw(canvas, font, fpx, label, tx, ty, color)?;
    Ok(())
}
