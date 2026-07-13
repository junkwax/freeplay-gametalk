//! Input abstraction layer.
//!
//! The frontend thinks in terms of **MK2 actions** (HighPunch, Block, Coin…).
//! Raw SDL events are translated into actions via a `Bindings` table that the
//! user can edit in the Controls menu and persist to config.toml.

use crate::retro::*;
use sdl2::controller::{Axis, Button};
use sdl2::keyboard::Keycode;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

pub const STICK_DEADZONE: i16 = 8000;

/// A single named game input. Fixed list — order is stable so config files
/// written with an older build still load cleanly.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    HighPunch,
    LowPunch,
    HighKick,
    LowKick,
    Block,
    Start,
    Coin,
}

impl Action {
    pub const ALL: [Action; 11] = [
        Action::Up,
        Action::Down,
        Action::Left,
        Action::Right,
        Action::HighPunch,
        Action::LowPunch,
        Action::HighKick,
        Action::LowKick,
        Action::Block,
        Action::Start,
        Action::Coin,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::Up => "Up",
            Action::Down => "Down",
            Action::Left => "Left",
            Action::Right => "Right",
            Action::HighPunch => "High Punch",
            Action::LowPunch => "Low Punch",
            Action::HighKick => "High Kick",
            Action::LowKick => "Low Kick",
            Action::Block => "Block",
            Action::Start => "Start",
            Action::Coin => "Coin",
        }
    }

    /// Libretro pad-slot index that this MK2 action writes into.
    pub fn retro_id(self) -> usize {
        (match self {
            // Slot assignments empirically verified against FBNeo's mk2 driver.
            // Reverse-deduced from in-game observation: pressing buttons bound
            // to each Action and recording which MK2 move actually fires.
            //   Slot B (id 0)  -> Low Punch
            //   Slot Y (id 1)  -> High Punch
            //   Slot A (id 8)  -> Low Kick
            //   Slot X (id 9)  -> High Kick
            //   Slot L (id 10) -> Block
            Action::Up => RETRO_DEVICE_ID_JOYPAD_UP,
            Action::Down => RETRO_DEVICE_ID_JOYPAD_DOWN,
            Action::Left => RETRO_DEVICE_ID_JOYPAD_LEFT,
            Action::Right => RETRO_DEVICE_ID_JOYPAD_RIGHT,
            Action::LowPunch => RETRO_DEVICE_ID_JOYPAD_B,
            Action::HighPunch => RETRO_DEVICE_ID_JOYPAD_Y,
            Action::LowKick => RETRO_DEVICE_ID_JOYPAD_A,
            Action::HighKick => RETRO_DEVICE_ID_JOYPAD_X,
            Action::Block => RETRO_DEVICE_ID_JOYPAD_L,
            Action::Start => RETRO_DEVICE_ID_JOYPAD_START,
            Action::Coin => RETRO_DEVICE_ID_JOYPAD_SELECT,
        }) as usize
    }
}

/// One physical source that can drive an action. Stored in config.toml.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Binding {
    Key { key: String },                      // SDL keycode name (e.g. "Up", "A")
    PadButton { button: String },             // SDL GameController button name
    PadAxis { axis: String, positive: bool }, // analog stick beyond deadzone
}

/// One concrete physical source currently driving a live action.
///
/// Several sources can map to the same action (D-pad left + analog-left, for
/// example). The live action stays held until every source for it releases.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InputSource {
    Key {
        key: String,
    },
    PadButton {
        which: u32,
        button: String,
    },
    PadAxis {
        which: u32,
        axis: String,
        positive: bool,
    },
}

impl InputSource {
    pub fn key(key: Keycode) -> Self {
        Self::Key { key: key_name(key) }
    }

    pub fn pad_button(which: u32, button: Button) -> Self {
        Self::PadButton {
            which,
            button: button_name(button),
        }
    }

    pub fn pad_axis(which: u32, axis: Axis, positive: bool) -> Self {
        Self::PadAxis {
            which,
            axis: axis_name(axis),
            positive,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AxisUpdate {
    pub action: Action,
    pub positive: bool,
    pub pressed: bool,
}

/// Two-player identifier. Drives selection in the Controls UI and maps to
/// libretro port 0 / 1.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Player {
    P1,
    P2,
}

impl Player {
    pub fn port(self) -> usize {
        match self {
            Player::P1 => 0,
            Player::P2 => 1,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Player::P1 => "P1",
            Player::P2 => "P2",
        }
    }
    pub fn other(self) -> Self {
        match self {
            Player::P1 => Player::P2,
            Player::P2 => Player::P1,
        }
    }
}

/// Per-player bindings table: each action maps to 1+ physical sources.
/// Pad bindings in this table apply **only** when that player's pad is active.
/// See `Bindings` for P1/P2 pad ownership rules.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerBindings {
    pub entries: Vec<(Action, Binding)>,
}

/// Top-level bindings: P1 + P2 + pad-ownership by SDL joystick instance id.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bindings {
    pub p1: PlayerBindings,
    pub p2: PlayerBindings,
}

impl Default for Bindings {
    fn default() -> Self {
        Self {
            p1: PlayerBindings::p1_default(),
            p2: PlayerBindings::p2_default(),
        }
    }
}

impl Bindings {
    pub fn get(&self, p: Player) -> &PlayerBindings {
        match p {
            Player::P1 => &self.p1,
            Player::P2 => &self.p2,
        }
    }
    pub fn get_mut(&mut self, p: Player) -> &mut PlayerBindings {
        match p {
            Player::P1 => &mut self.p1,
            Player::P2 => &mut self.p2,
        }
    }
}

impl PlayerBindings {
    fn p1_default() -> Self {
        use Action::*;
        let mut e: Vec<(Action, Binding)> = Vec::new();

        // P1 pad: Xbox layout with right trigger for block
        e.push((
            Coin,
            Binding::PadButton {
                button: "Back".into(),
            },
        ));
        e.push((
            Up,
            Binding::PadButton {
                button: "DPadUp".into(),
            },
        ));
        e.push((
            Down,
            Binding::PadButton {
                button: "DPadDown".into(),
            },
        ));
        e.push((
            Left,
            Binding::PadButton {
                button: "DPadLeft".into(),
            },
        ));
        e.push((
            Right,
            Binding::PadButton {
                button: "DPadRight".into(),
            },
        ));
        e.push((HighPunch, Binding::PadButton { button: "X".into() }));
        e.push((LowPunch, Binding::PadButton { button: "A".into() }));
        e.push((HighKick, Binding::PadButton { button: "Y".into() }));
        e.push((LowKick, Binding::PadButton { button: "B".into() }));
        e.push((
            Block,
            Binding::PadAxis {
                axis: "TriggerRight".into(),
                positive: true,
            },
        ));
        e.push((
            Start,
            Binding::PadButton {
                button: "Start".into(),
            },
        ));

        Self { entries: e }
    }

    fn p2_default() -> Self {
        use Action::*;
        let mut e: Vec<(Action, Binding)> = Vec::new();

        // P2 keyboard: numpad layout
        e.push((Up, Binding::Key { key: "Kp8".into() }));
        e.push((Down, Binding::Key { key: "Kp2".into() }));
        e.push((Left, Binding::Key { key: "Kp4".into() }));
        e.push((Right, Binding::Key { key: "Kp6".into() }));
        e.push((HighPunch, Binding::Key { key: "Kp7".into() }));
        e.push((HighKick, Binding::Key { key: "Kp9".into() }));
        e.push((LowPunch, Binding::Key { key: "Kp1".into() }));
        e.push((LowKick, Binding::Key { key: "Kp3".into() }));
        e.push((Block, Binding::Key { key: "Kp0".into() }));
        e.push((
            Start,
            Binding::Key {
                key: "KpEnter".into(),
            },
        ));
        e.push((
            Coin,
            Binding::Key {
                key: "KpPlus".into(),
            },
        ));

        Self { entries: e }
    }
}

impl PlayerBindings {
    pub fn clear_all(&mut self) {
        self.entries.clear();
    }

    /// Look up every action bound to this physical source.
    pub fn actions_for_key(&self, key: Keycode) -> Vec<Action> {
        let name = key_name(key);
        self.entries
            .iter()
            .filter_map(|(a, b)| match b {
                Binding::Key { key: k } if k == &name => Some(*a),
                _ => None,
            })
            .collect()
    }

    pub fn actions_for_button(&self, btn: Button) -> Vec<Action> {
        let name = button_name(btn);
        self.entries
            .iter()
            .filter_map(|(a, b)| match b {
                Binding::PadButton { button: k } if k == &name => Some(*a),
                _ => None,
            })
            .collect()
    }

    /// For axis motion, return (action, pressed) for every binding that either
    /// triggers or releases based on this axis value.
    pub fn axis_updates(&self, axis: Axis, value: i16) -> Vec<AxisUpdate> {
        let name = axis_name(axis);
        let mut out = Vec::new();
        for (a, b) in &self.entries {
            if let Binding::PadAxis { axis: k, positive } = b {
                if k == &name {
                    let pressed = if *positive {
                        value > STICK_DEADZONE
                    } else {
                        value < -STICK_DEADZONE
                    };
                    out.push(AxisUpdate {
                        action: *a,
                        positive: *positive,
                        pressed,
                    });
                }
            }
        }
        out
    }

    /// Remove every binding (kbd, pad, axis) for this action.
    pub fn clear_action(&mut self, action: Action) {
        self.entries.retain(|(a, _)| *a != action);
    }

    pub fn replace_binding(&mut self, action: Action, new_b: Binding) {
        // Remove any existing keyboard/pad bindings of the SAME kind for this action,
        // so "rebind HighPunch to button Z" replaces the old button rather than
        // stacking. Axis bindings for dirs can coexist with their button bindings.
        let same_kind = |existing: &Binding, new: &Binding| {
            matches!(
                (existing, new),
                (Binding::Key { .. }, Binding::Key { .. })
                    | (Binding::PadButton { .. }, Binding::PadButton { .. })
                    | (Binding::PadAxis { .. }, Binding::PadAxis { .. })
            )
        };
        self.entries
            .retain(|(a, b)| !(*a == action && same_kind(b, &new_b)));
        self.entries.push((action, new_b));
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct HeldInputSource {
    player: Player,
    action: Action,
    source: InputSource,
}

#[derive(Default)]
struct LiveInputSources {
    held: HashSet<HeldInputSource>,
}

impl LiveInputSources {
    fn set_source(
        &mut self,
        player: Player,
        action: Action,
        source: InputSource,
        pressed: bool,
    ) -> bool {
        let held = HeldInputSource {
            player,
            action,
            source,
        };
        if pressed {
            self.held.insert(held);
        } else {
            self.held.remove(&held);
        }
        self.is_action_held(player, action)
    }

    fn is_action_held(&self, player: Player, action: Action) -> bool {
        self.held
            .iter()
            .any(|held| held.player == player && held.action == action)
    }

    fn clear(&mut self) {
        self.held.clear();
    }
}

static LIVE_INPUT_SOURCES: OnceLock<Mutex<LiveInputSources>> = OnceLock::new();

fn live_input_sources() -> &'static Mutex<LiveInputSources> {
    LIVE_INPUT_SOURCES.get_or_init(|| Mutex::new(LiveInputSources::default()))
}

/// Record an action press/release from the live input layer (SDL events).
/// This does NOT write directly to the libretro-visible input state; the
/// main loop decides when to commit live input into it (every frame for
/// local play; never for netplay — ggrs owns the visible state there).
pub fn set_action(player: Player, action: Action, pressed: bool) {
    crate::retro::set_live_input(player.port(), action.retro_id(), pressed);
}

/// Record one physical source for an action and resolve the action as the OR
/// of every currently-held source bound to it.
pub fn set_action_source(player: Player, action: Action, source: InputSource, pressed: bool) {
    let pressed = {
        let mut sources = live_input_sources()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        sources.set_source(player, action, source, pressed)
    };
    set_action(player, action, pressed);
}

/// Serialize one player's live pad state into a compact 16-bit packet for
/// network transmission. Bit index = position in Action::ALL.
pub fn snapshot_player(player: Player) -> u16 {
    // One lock for the whole row instead of one per action.
    let row = crate::retro::live_input_port(player.port());
    let mut bits: u16 = 0;
    for (i, a) in Action::ALL.iter().enumerate() {
        if row[a.retro_id()] {
            bits |= 1 << i;
        }
    }
    bits
}

/// Apply a compact packet to the libretro-visible input state for `player`.
/// Called by netplay during AdvanceFrame, or by the local-play path each
/// frame via `commit_live_to_state`.
pub fn apply_snapshot(player: Player, bits: u16) {
    let mut row = [false; 16];
    for (i, a) in Action::ALL.iter().enumerate() {
        row[a.retro_id()] = (bits >> i) & 1 == 1;
    }
    crate::retro::set_input_port(player.port(), row);
}

/// Copy live input directly into the libretro-visible state for both
/// players. Used by the local-play path each frame so the emulator sees
/// the user's current pad state without going through ggrs.
pub fn commit_live_to_state() {
    crate::retro::commit_live_to_state();
}

pub fn clear_all_inputs() {
    live_input_sources()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    crate::retro::clear_all_inputs();
}

// --- SDL enum <-> string helpers ---
// We store binding identifiers as strings so config.toml is human-readable and
// survives SDL enum ordering changes between crate versions.

pub fn key_name(k: Keycode) -> String {
    format!("{:?}", k)
}

pub fn button_name(b: Button) -> String {
    format!("{:?}", b)
}

pub fn axis_name(a: Axis) -> String {
    format!("{:?}", a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(name: &str) -> InputSource {
        InputSource::Key { key: name.into() }
    }

    #[test]
    fn action_stays_held_until_all_sources_release() {
        let mut sources = LiveInputSources::default();

        assert!(sources.set_source(Player::P1, Action::Left, source("DPadLeft"), true));
        assert!(sources.set_source(Player::P1, Action::Left, source("LeftX-"), true));
        assert!(sources.set_source(Player::P1, Action::Left, source("LeftX-"), false));
        assert!(sources.is_action_held(Player::P1, Action::Left));

        assert!(!sources.set_source(Player::P1, Action::Left, source("DPadLeft"), false));
        assert!(!sources.is_action_held(Player::P1, Action::Left));
    }

    #[test]
    fn source_releases_only_its_player_and_action() {
        let mut sources = LiveInputSources::default();

        assert!(sources.set_source(Player::P1, Action::Left, source("Shared"), true));
        assert!(sources.set_source(Player::P1, Action::Right, source("Shared"), true));
        assert!(sources.set_source(Player::P2, Action::Left, source("Shared"), true));

        assert!(!sources.set_source(Player::P1, Action::Left, source("Shared"), false));
        assert!(sources.is_action_held(Player::P1, Action::Right));
        assert!(sources.is_action_held(Player::P2, Action::Left));
    }
}
