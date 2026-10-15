//! Input abstraction layer.
//!
//! The frontend thinks in terms of **game actions** (HighPunch, Shoot, Coin…).
//! Each game uses its own subset, in its own wire order — see
//! `game::Profile::actions` and `actions()`.
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
    // NBA Jam. Appended, never interleaved: config.toml stores bindings by
    // variant name, and MK2's wire order is `Action::MK2`, not this enum.
    Shoot,
    Pass,
    Turbo,
}

/// The current game's actions, in netplay wire order.
pub fn actions() -> &'static [Action] {
    crate::game::current().actions
}

/// Fingerprint of what `NetInput`'s bits *mean*.
///
/// The netplay packet is a bare `u16` whose bit index is a position in
/// `Action::ALL`, and whose bit is read out of the libretro slot that
/// action's `retro_id` names (see `snapshot_player`/`apply_snapshot`). Both
/// halves are therefore protocol, not preference: two peers that disagree on
/// the list's order or on any slot don't error, they silently read each
/// other's High Punch as Low Kick and walk apart.
///
/// Nothing can disagree today, because the list is one compiled-in constant.
/// It stops being one the moment a game profile supplies it, which is why
/// this exists now — the guard is worth having in place before the thing it
/// guards can vary.
pub fn action_set_fingerprint() -> String {
    action_set_fingerprint_of(actions())
}

pub fn action_set_fingerprint_of(set: &[Action]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for (i, a) in set.iter().enumerate() {
        for byte in (i as u64)
            .to_le_bytes()
            .iter()
            .chain((a.retro_id() as u64).to_le_bytes().iter())
        {
            h ^= *byte as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    format!("{:08x}", (h >> 32) as u32)
}

/// The fingerprint every client through v0.8.7 shipped.
///
/// Matchmaking omits the fingerprint from its key when it equals this, so a
/// build carrying the original action set produces byte-identical keys to
/// those releases and keeps pairing with them. Absence means "the original
/// set" — only a profile that actually changes the wire meaning gets its own
/// pool, which is the only case where splitting players is the correct
/// outcome.
pub const LEGACY_ACTION_SET: &str = "8b52357c";

impl Action {
    /// MK2's set. Its order is the wire format every MK2 client has ever
    /// shipped; never reorder it.
    pub const MK2: [Action; 11] = [
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

    /// NBA Jam's set: the stick, the cabinet's three buttons, start, coin.
    pub const NBA_JAM: [Action; 9] = [
        Action::Up,
        Action::Down,
        Action::Left,
        Action::Right,
        Action::Shoot,
        Action::Pass,
        Action::Turbo,
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
            Action::Shoot => "Shoot / Block",
            Action::Pass => "Pass / Steal",
            Action::Turbo => "Turbo",
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
            // NBA Jam has three plain buttons, so FBNeo gives it the generic
            // layout: fire 1 -> B, fire 2 -> A, fire 3 -> Y. Its input list
            // (d_tunit.cpp) wires fire 1 to the Turbo bit (0x40), fire 2 to
            // Shoot (0x10) and fire 3 to Pass (0x20), matching MAME's names.
            Action::Shoot => RETRO_DEVICE_ID_JOYPAD_A,
            Action::Pass => RETRO_DEVICE_ID_JOYPAD_Y,
            Action::Turbo => RETRO_DEVICE_ID_JOYPAD_B,
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
    /// Give every action with no binding at all its default ones. A
    /// config.toml written before NBA Jam existed has nothing for Shoot, Pass
    /// or Turbo; without this an upgrading player couldn't play it until they
    /// found the rebind screen. Never touches an action that has a binding.
    pub fn fill_missing_defaults(&mut self) -> bool {
        let defaults = Bindings::default();
        let mut changed = false;
        for (mine, theirs) in [(&mut self.p1, &defaults.p1), (&mut self.p2, &defaults.p2)] {
            let unbound: Vec<Action> = theirs
                .entries
                .iter()
                .map(|(a, _)| *a)
                .filter(|a| !mine.entries.iter().any(|(m, _)| m == a))
                .collect();
            for (action, binding) in &theirs.entries {
                if unbound.contains(action) {
                    mine.entries.push((*action, binding.clone()));
                    changed = true;
                }
            }
        }
        changed
    }

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
        // Left stick too: plenty of players move with it, and many arcade
        // sticks report their lever as the left stick in "LS" mode. Axis and
        // button bindings for a direction coexist (see `replace_binding`).
        for (action, axis, positive) in [
            (Up, "LeftY", false),
            (Down, "LeftY", true),
            (Left, "LeftX", false),
            (Right, "LeftX", true),
        ] {
            e.push((
                action,
                Binding::PadAxis {
                    axis: axis.into(),
                    positive,
                },
            ));
        }
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
        e.extend(Self::p1_nba_jam_defaults());

        Self { entries: e }
    }

    fn p1_nba_jam_defaults() -> Vec<(Action, Binding)> {
        use Action::*;
        vec![
            (Shoot, Binding::PadButton { button: "A".into() }),
            (Pass, Binding::PadButton { button: "X".into() }),
            (
                Turbo,
                Binding::PadAxis {
                    axis: "TriggerRight".into(),
                    positive: true,
                },
            ),
            (
                Turbo,
                Binding::PadButton {
                    button: "RightShoulder".into(),
                },
            ),
        ]
    }

    fn p2_nba_jam_defaults() -> Vec<(Action, Binding)> {
        use Action::*;
        vec![
            (Shoot, Binding::Key { key: "Kp1".into() }),
            (Pass, Binding::Key { key: "Kp7".into() }),
            (Turbo, Binding::Key { key: "Kp0".into() }),
        ]
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
        e.extend(Self::p2_nba_jam_defaults());

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
///
/// Actions outside the current game's set are dropped here. Bindings for
/// every game live side by side and share retropad slots (MK2's High Punch
/// and NBA Jam's Pass are both slot Y), so without this an MK2 binding would
/// press an NBA Jam button.
pub fn set_action(player: Player, action: Action, pressed: bool) {
    if !actions().contains(&action) {
        return;
    }
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
/// network transmission. Bit index = position in `actions()`.
pub fn snapshot_player(player: Player) -> u16 {
    // One lock for the whole row instead of one per action.
    let row = crate::retro::live_input_port(player.port());
    let mut bits: u16 = 0;
    for (i, a) in actions().iter().enumerate() {
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
    for (i, a) in actions().iter().enumerate() {
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

    #[test]
    fn p1_moves_with_the_dpad_and_the_left_stick() {
        let p1 = Bindings::default().p1;
        let bound = |action: Action| {
            p1.entries
                .iter()
                .filter(move |(a, _)| *a == action)
                .map(|(_, b)| b)
        };
        for (action, dpad, axis, positive) in [
            (Action::Up, "DPadUp", "LeftY", false),
            (Action::Down, "DPadDown", "LeftY", true),
            (Action::Left, "DPadLeft", "LeftX", false),
            (Action::Right, "DPadRight", "LeftX", true),
        ] {
            assert!(
                bound(action).any(|b| matches!(b, Binding::PadButton { button } if button == dpad)),
                "{action:?} d-pad"
            );
            assert!(
                bound(action).any(|b| matches!(b, Binding::PadAxis { axis: a, positive: p } if a == axis && *p == positive)),
                "{action:?} stick"
            );
        }
        // Up on the stick must not also press Down.
        let ups = p1.axis_updates(Axis::LeftY, i16::MIN);
        assert!(ups.iter().any(|u| u.action == Action::Up && u.pressed));
        assert!(!ups.iter().any(|u| u.action == Action::Down && u.pressed));
    }

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

#[cfg(test)]
mod action_set_tests {
    use super::*;

    /// If this fails, the wire meaning of `NetInput` changed. That is allowed
    /// — but it must be deliberate, and `LEGACY_ACTION_SET` must NOT be
    /// updated to match: leaving it alone is what makes matchmaking give the
    /// new set its own pool instead of pairing it against clients that read
    /// the bits differently.
    #[test]
    fn the_shipped_action_set_still_matches_the_recorded_fingerprint() {
        assert_eq!(
            action_set_fingerprint_of(&Action::MK2),
            LEGACY_ACTION_SET,
            "action list order or a retro_id changed - this alters what every \
             netplay input bit means"
        );
    }

    /// The fingerprint has to react to both halves of the wire contract, or
    /// it would wave through exactly the changes it exists to catch.
    #[test]
    fn the_fingerprint_reacts_to_order_and_to_slots() {
        fn fingerprint_of(pairs: &[(usize, usize)]) -> String {
            let mut h: u64 = 0xcbf29ce484222325;
            for (i, slot) in pairs {
                for byte in (*i as u64)
                    .to_le_bytes()
                    .iter()
                    .chain((*slot as u64).to_le_bytes().iter())
                {
                    h ^= *byte as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
            }
            format!("{:08x}", (h >> 32) as u32)
        }

        let actual: Vec<(usize, usize)> = Action::MK2
            .iter()
            .enumerate()
            .map(|(i, a)| (i, a.retro_id()))
            .collect();
        assert_eq!(
            fingerprint_of(&actual),
            action_set_fingerprint_of(&Action::MK2)
        );

        // Same actions, two of them swapped: every bit past the swap means
        // something else on the wire.
        let mut reordered = actual.clone();
        reordered.swap(4, 7);
        let reordered: Vec<(usize, usize)> = reordered
            .into_iter()
            .enumerate()
            .map(|(i, (_, slot))| (i, slot))
            .collect();
        assert_ne!(
            fingerprint_of(&reordered),
            action_set_fingerprint_of(&Action::MK2)
        );

        // Same order, one action rebound to a different libretro slot.
        let mut reslotted = actual.clone();
        reslotted[4].1 = 15;
        assert_ne!(
            fingerprint_of(&reslotted),
            action_set_fingerprint_of(&Action::MK2)
        );
    }

    #[test]
    fn nba_jam_gets_its_own_wire_pool() {
        assert_ne!(
            action_set_fingerprint_of(&Action::NBA_JAM),
            LEGACY_ACTION_SET
        );
    }

    #[test]
    fn old_configs_gain_nba_jam_defaults_without_losing_their_own() {
        let mut b = Bindings::default();
        b.p1.entries
            .retain(|(a, _)| !matches!(a, Action::Shoot | Action::Pass | Action::Turbo));
        b.p1.entries.retain(|(a, _)| *a != Action::HighPunch);
        b.p1.entries
            .push((Action::HighPunch, Binding::Key { key: "Q".into() }));
        assert!(b.fill_missing_defaults());
        assert!(b.p1.entries.iter().any(|(a, _)| *a == Action::Shoot));
        let hp: Vec<_> =
            b.p1.entries
                .iter()
                .filter(|(a, _)| *a == Action::HighPunch)
                .collect();
        assert_eq!(
            hp.len(),
            1,
            "a bound action must keep only the player's binding"
        );
        assert!(!b.fill_missing_defaults());
    }

    #[test]
    fn every_game_fits_the_u16_wire_packet_without_slot_collisions() {
        for set in [&Action::MK2[..], &Action::NBA_JAM[..]] {
            assert!(set.len() <= 16);
            let mut slots: Vec<usize> = set.iter().map(|a| a.retro_id()).collect();
            slots.sort();
            slots.dedup();
            assert_eq!(slots.len(), set.len());
        }
    }
}
