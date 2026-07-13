//! Lab-mode dummy controls.
//!
//! The arcade game does not have a native training mode. To get a controllable
//! dummy, Lab starts a local two-player match and owns P2's input port.

use crate::mk2_addrs;
use crate::{input::Action, memory, retro::Core};

pub const MAX_DUMMY_RECORDING_FRAMES: usize = 55 * 5;
pub const PUNISH_WINDOW_FRAMES: u32 = 45;
pub const LATE_WINDOW_FRAMES: u32 = 45;
pub const DAMAGE_COMBO_GAP_FRAMES: u32 = 70;
pub const LAB_RESET_SLOT_COUNT: usize = 3;
pub struct ResetSlots {
    active_slot: usize,
    slots: [Option<Vec<u8>>; LAB_RESET_SLOT_COUNT],
}

impl Default for ResetSlots {
    fn default() -> Self {
        Self {
            active_slot: 0,
            slots: std::array::from_fn(|_| None),
        }
    }
}

impl ResetSlots {
    pub fn clear(&mut self) {
        self.active_slot = 0;
        self.slots = std::array::from_fn(|_| None);
    }

    pub fn cycle_next(&mut self) -> usize {
        self.active_slot = (self.active_slot + 1) % LAB_RESET_SLOT_COUNT;
        self.active_number()
    }

    pub fn active_number(&self) -> usize {
        self.active_slot + 1
    }

    pub fn active_status_label(&self) -> String {
        format!(
            "S{}{}",
            self.active_number(),
            if self.active_saved() { "*" } else { "" }
        )
    }

    pub fn active_saved(&self) -> bool {
        self.slots[self.active_slot].is_some()
    }

    pub fn save_active(&mut self, blob: Vec<u8>) -> usize {
        let bytes = blob.len();
        self.slots[self.active_slot] = Some(blob);
        bytes
    }

    pub fn load_active(&self, core: &Core) -> Option<bool> {
        self.slots[self.active_slot]
            .as_ref()
            .map(|slot| core.load_state(slot))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionPreset {
    Midscreen,
    P2Corner,
    P1Corner,
}

impl PositionPreset {
    pub fn next(self) -> Self {
        match self {
            Self::Midscreen => Self::P2Corner,
            Self::P2Corner => Self::P1Corner,
            Self::P1Corner => Self::Midscreen,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Midscreen => "MID",
            Self::P2Corner => "P2 CORNER",
            Self::P1Corner => "P1 CORNER",
        }
    }

    fn coords(self) -> (u16, u16) {
        match self {
            Self::Midscreen => (120, 280),
            Self::P2Corner => (248, 342),
            Self::P1Corner => (58, 152),
        }
    }
}

impl Default for PositionPreset {
    fn default() -> Self {
        Self::Midscreen
    }
}

pub fn apply_position_preset(core: &Core, preset: PositionPreset) {
    let (p1_x, p2_x) = preset.coords();
    memory::poke_u16(core, mk2_addrs::P1_X_ADDR, p1_x, memory::Endian::Little);
    memory::poke_u16(core, mk2_addrs::P2_X_ADDR, p2_x, memory::Endian::Little);
    memory::poke_u16(core, mk2_addrs::P1_Y_ADDR, 0, memory::Endian::Little);
    memory::poke_u16(core, mk2_addrs::P2_Y_ADDR, 0, memory::Endian::Little);
}

/// Value MKSEL.ASM's `player_select` writes into `p1_char`/`p2_char` on
/// select-screen entry ("null the characters"). Each side's real fighter id
/// replaces it when that side locks in, and attract-mode fights write real
/// ids too — so this value unambiguously means "on the select screen, not
/// picked yet".
pub const CHAR_NULLED: u16 = 0xFFFF;

/// Where the game is between "Lab launched" and "fight running", derived
/// from RAM once per tick by main.rs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabPhase {
    /// Attract, VS screen, continue countdowns — anything that isn't the
    /// select screen or a live fight.
    PreFight,
    /// Character select. Each flag = that side has locked a character in.
    Select { p1_picked: bool, p2_picked: bool },
    Fight,
}

pub fn phase_from_ram(fight_loaded: bool, p1_char: u16, p2_char: u16) -> LabPhase {
    if fight_loaded {
        LabPhase::Fight
    } else if p1_char == CHAR_NULLED || p2_char == CHAR_NULLED {
        LabPhase::Select {
            p1_picked: p1_char != CHAR_NULLED,
            p2_picked: p2_char != CHAR_NULLED,
        }
    } else {
        LabPhase::PreFight
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DummyMode {
    Stand,
    Crouch,
    Block,
    CrouchBlock,
    Jump,
    JumpIn,
    ReversalMash,
    ThrowTech,
    WakeBlock,
    Off,
}

impl DummyMode {
    pub fn next(self) -> Self {
        match self {
            Self::Stand => Self::Crouch,
            Self::Crouch => Self::Block,
            Self::Block => Self::CrouchBlock,
            Self::CrouchBlock => Self::Jump,
            Self::Jump => Self::JumpIn,
            Self::JumpIn => Self::ReversalMash,
            Self::ReversalMash => Self::ThrowTech,
            Self::ThrowTech => Self::WakeBlock,
            Self::WakeBlock => Self::Off,
            Self::Off => Self::Stand,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stand => "STAND",
            Self::Crouch => "CROUCH",
            Self::Block => "BLOCK",
            Self::CrouchBlock => "CROUCH BLOCK",
            Self::Jump => "JUMP",
            Self::JumpIn => "JUMP IN",
            Self::ReversalMash => "REV MASH",
            Self::ThrowTech => "THROW TECH",
            Self::WakeBlock => "WAKE BLOCK",
            Self::Off => "OFF",
        }
    }

    pub fn active(self) -> bool {
        self != Self::Off
    }
}

pub struct DummyController {
    mode: DummyMode,
    frame: u32,
    recording: bool,
    recorded_frames: Vec<u16>,
    loop_frames: Vec<u16>,
    loop_cursor: usize,
    loop_completed: Option<usize>,
    auto_finished_loop: Option<usize>,
    /// P1's confirm button is usually still held on the frames right after
    /// their pick lands in `p1_char`; mirroring it to P2 immediately would
    /// insta-confirm whatever square P2's cursor starts on. Buttons only
    /// mirror once P1 has fully released them at least one frame.
    select_mirror_armed: bool,
}

impl Default for DummyController {
    fn default() -> Self {
        Self {
            mode: DummyMode::Stand,
            frame: 0,
            recording: false,
            recorded_frames: Vec::new(),
            loop_frames: Vec::new(),
            loop_cursor: 0,
            loop_completed: None,
            auto_finished_loop: None,
            select_mirror_armed: false,
        }
    }
}

impl DummyController {
    #[cfg(test)]
    pub fn mode(&self) -> DummyMode {
        self.mode
    }

    pub fn cycle_mode(&mut self) -> DummyMode {
        self.mode = self.mode.next();
        self.frame = 0;
        self.recording = false;
        self.recorded_frames.clear();
        self.loop_frames.clear();
        self.loop_cursor = 0;
        self.loop_completed = None;
        self.auto_finished_loop = None;
        self.mode
    }

    pub fn active(&self) -> bool {
        self.recording || !self.loop_frames.is_empty() || self.mode.active()
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    pub fn start_recording(&mut self) {
        self.recording = true;
        self.recorded_frames.clear();
        self.loop_frames.clear();
        self.loop_cursor = 0;
        self.loop_completed = None;
        self.auto_finished_loop = None;
    }

    pub fn stop_recording(&mut self) -> usize {
        self.recording = false;
        let len = self.recorded_frames.len();
        if len > 0 {
            self.loop_frames = self.recorded_frames.clone();
            self.loop_cursor = 0;
            self.loop_completed = None;
        }
        self.recorded_frames.clear();
        len
    }

    pub fn clear_loop(&mut self) {
        self.recording = false;
        self.recorded_frames.clear();
        self.loop_frames.clear();
        self.loop_cursor = 0;
        self.loop_completed = None;
        self.auto_finished_loop = None;
    }

    /// True when a recorded loop is armed for playback — the punish
    /// trainer's arming signal only fires on loop wrap, so without one the
    /// trainer never scores.
    pub fn has_loop(&self) -> bool {
        !self.loop_frames.is_empty()
    }

    pub fn take_auto_finished_loop(&mut self) -> Option<usize> {
        self.auto_finished_loop.take()
    }

    pub fn take_loop_completed(&mut self) -> Option<usize> {
        self.loop_completed.take()
    }

    pub fn status_label(&self) -> String {
        if self.recording {
            format!(
                "REC {}/{}F",
                self.recorded_frames.len(),
                MAX_DUMMY_RECORDING_FRAMES
            )
        } else if !self.loop_frames.is_empty() {
            format!("LOOP {}", format_frames(self.loop_frames.len()))
        } else {
            self.mode.label().to_string()
        }
    }

    /// `p2_right_of_p1` orients side-relative presets (Jump In) toward the
    /// player — hardcoding a direction sends the dummy the wrong way after
    /// a side switch.
    pub fn next_bits(
        &mut self,
        phase: LabPhase,
        p2_right_of_p1: bool,
        live_p1_bits: u16,
        live_p2_bits: u16,
    ) -> Option<u16> {
        if !self.active() {
            return None;
        }
        let fight_loaded = phase == LabPhase::Fight;
        self.frame = self.frame.wrapping_add(1);
        if self.recording {
            if fight_loaded {
                self.recorded_frames.push(live_p2_bits);
                if self.recorded_frames.len() >= MAX_DUMMY_RECORDING_FRAMES {
                    let len = self.stop_recording();
                    self.auto_finished_loop = Some(len);
                }
                return Some(live_p2_bits);
            }
            return Some(self.pre_fight_bits(phase, live_p1_bits, live_p2_bits));
        }
        if fight_loaded && !self.loop_frames.is_empty() {
            let bits = self.loop_frames[self.loop_cursor % self.loop_frames.len()];
            self.loop_cursor = (self.loop_cursor + 1) % self.loop_frames.len();
            if self.loop_cursor == 0 {
                self.loop_completed = Some(self.loop_frames.len());
            }
            return Some(bits);
        }
        if fight_loaded {
            Some(self.fight_bits(p2_right_of_p1))
        } else {
            Some(self.pre_fight_bits(phase, live_p1_bits, live_p2_bits))
        }
    }

    /// Pre-fight P2 port bits. The dummy's only pre-fight jobs are getting
    /// P2 joined in (Start pulses — the select screen picks with the attack
    /// buttons, so stray Starts can't lock a character) and, once P1 has
    /// locked in on the select screen, handing the P2 cursor to the player
    /// by mirroring P1's controls. A physical P2 controller keeps working
    /// throughout: its steer bits are OR'd in unconditionally.
    fn pre_fight_bits(&mut self, phase: LabPhase, live_p1_bits: u16, live_p2_bits: u16) -> u16 {
        let live_p2_steer = live_p2_bits & steer_mask();
        match phase {
            LabPhase::Select {
                p2_picked: true, ..
            } => {
                // P2 locked in — press nothing; the game runs the VS intro
                // on its own.
                self.select_mirror_armed = false;
                0
            }
            LabPhase::Select {
                p1_picked: true,
                p2_picked: false,
            } => {
                let mut bits = live_p1_bits & direction_mask();
                let p1_buttons = live_p1_bits & button_mask();
                if self.select_mirror_armed {
                    bits |= p1_buttons;
                } else if p1_buttons == 0 {
                    self.select_mirror_armed = true;
                }
                bits | live_p2_steer
            }
            _ => {
                // Attract / select-before-P1-picks / transitions: keep P2
                // joining in so select never ends with the dummy absent.
                self.select_mirror_armed = false;
                let mut bits = 0;
                if self.frame % 24 < 5 {
                    set_action(&mut bits, Action::Start);
                }
                bits | live_p2_steer
            }
        }
    }

    fn fight_bits(&self, p2_right_of_p1: bool) -> u16 {
        let mut bits = 0;
        match self.mode {
            DummyMode::Stand | DummyMode::Off => {}
            DummyMode::Crouch => set_action(&mut bits, Action::Down),
            DummyMode::Block => set_action(&mut bits, Action::Block),
            DummyMode::CrouchBlock => {
                set_action(&mut bits, Action::Down);
                set_action(&mut bits, Action::Block);
            }
            DummyMode::Jump => {
                if self.frame % 58 < 8 {
                    set_action(&mut bits, Action::Up);
                }
            }
            DummyMode::JumpIn => {
                let phase = self.frame % 72;
                let toward_p1 = if p2_right_of_p1 {
                    Action::Left
                } else {
                    Action::Right
                };
                if phase < 14 {
                    set_action(&mut bits, Action::Up);
                    set_action(&mut bits, toward_p1);
                } else if (24..31).contains(&phase) {
                    set_action(&mut bits, Action::HighKick);
                }
            }
            // MK2 ignores attack buttons while Block is held, so both mash
            // modes must *release* block for the press frames — Block+LP on
            // the same frame just blocks and the "mash" never comes out.
            DummyMode::ReversalMash => {
                if self.frame % 16 < 4 {
                    set_action(&mut bits, Action::LowPunch);
                } else {
                    set_action(&mut bits, Action::Block);
                }
            }
            DummyMode::ThrowTech => {
                // Guarded most of the cycle (blocking also denies throws in
                // MK2), with periodic counter-throw attempts (close LP).
                if self.frame % 24 < 4 {
                    set_action(&mut bits, Action::LowPunch);
                } else {
                    set_action(&mut bits, Action::Block);
                }
            }
            DummyMode::WakeBlock => {
                if self.frame % 90 < 58 {
                    set_action(&mut bits, Action::Down);
                    set_action(&mut bits, Action::Block);
                }
            }
        }
        bits
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PunishEvent {
    Punish { frame: u32 },
    Late { frame: u32 },
    Blocked,
    Missed,
}

impl PunishEvent {
    pub fn label(self) -> String {
        match self {
            Self::Punish { frame } => format!("PUNISH {frame}F"),
            Self::Late { frame } => format!("LATE {frame}F"),
            Self::Blocked => "BLOCKED".into(),
            Self::Missed => "MISSED".into(),
        }
    }
}

#[derive(Default)]
pub struct PunishTrainer {
    enabled: bool,
    window_remaining: u32,
    late_remaining: u32,
    baseline_hp: Option<u16>,
    saw_attack: bool,
    attempts: u32,
    punishes: u32,
    late_hits: u32,
    blocked: u32,
    missed: u32,
    best_frame: Option<u32>,
    last_event: Option<PunishEvent>,
}

#[derive(Default)]
pub struct DamageTracker {
    last_hp: Option<u16>,
    combo_damage: u16,
    combo_hits: u32,
    combo_gap: u32,
    last_damage: u16,
    last_hits: u32,
    attempts: u32,
    best_damage: u16,
}

impl DamageTracker {
    pub fn reset_stats(&mut self) {
        *self = Self::default();
    }

    pub fn observe(&mut self, fight_loaded: bool, p2_hp: u16) {
        if !fight_loaded || p2_hp == 0 {
            self.last_hp = None;
            self.combo_damage = 0;
            self.combo_hits = 0;
            self.combo_gap = 0;
            return;
        }

        if let Some(prev_hp) = self.last_hp {
            if p2_hp < prev_hp {
                let damage = prev_hp - p2_hp;
                if self.combo_gap == 0 || self.combo_damage == 0 {
                    self.combo_damage = 0;
                    self.combo_hits = 0;
                    self.attempts = self.attempts.saturating_add(1);
                }
                self.combo_damage = self.combo_damage.saturating_add(damage);
                self.combo_hits = self.combo_hits.saturating_add(1);
                self.combo_gap = DAMAGE_COMBO_GAP_FRAMES;
                self.last_damage = self.combo_damage;
                self.last_hits = self.combo_hits;
                self.best_damage = self.best_damage.max(self.combo_damage);
            } else if p2_hp > prev_hp {
                self.combo_damage = 0;
                self.combo_hits = 0;
                self.combo_gap = 0;
            } else if self.combo_gap > 0 {
                self.combo_gap -= 1;
                if self.combo_gap == 0 {
                    self.combo_damage = 0;
                    self.combo_hits = 0;
                }
            }
        }
        self.last_hp = Some(p2_hp);
    }

    #[cfg(test)]
    pub fn status_label(&self) -> String {
        if self.combo_damage > 0 && self.combo_gap > 0 {
            format!(
                "{} H{} TRY {} BEST {}",
                self.combo_damage, self.combo_hits, self.attempts, self.best_damage
            )
        } else if self.last_damage > 0 {
            format!(
                "LAST {} H{} TRY {} BEST {}",
                self.last_damage, self.last_hits, self.attempts, self.best_damage
            )
        } else {
            "--".into()
        }
    }
}

impl PunishTrainer {
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn toggle(&mut self) -> bool {
        self.enabled = !self.enabled;
        self.window_remaining = 0;
        self.late_remaining = 0;
        self.baseline_hp = None;
        self.saw_attack = false;
        self.enabled
    }

    pub fn reset_stats(&mut self) {
        let enabled = self.enabled;
        *self = Self::default();
        self.enabled = enabled;
    }

    pub fn arm(&mut self, p2_hp: u16) {
        if !self.enabled || p2_hp == 0 {
            return;
        }
        self.window_remaining = PUNISH_WINDOW_FRAMES;
        self.late_remaining = 0;
        self.baseline_hp = Some(p2_hp);
        self.saw_attack = false;
    }

    pub fn observe(&mut self, p2_hp: u16, p1_bits: u16) -> Option<PunishEvent> {
        if !self.enabled {
            return None;
        }
        let attack = is_attack(p1_bits);
        if self.window_remaining > 0 {
            self.saw_attack |= attack;
            let elapsed = PUNISH_WINDOW_FRAMES - self.window_remaining + 1;
            if let Some(base) = self.baseline_hp {
                if p2_hp > 0 && p2_hp < base {
                    self.window_remaining = 0;
                    self.baseline_hp = Some(p2_hp);
                    self.attempts = self.attempts.saturating_add(1);
                    self.punishes = self.punishes.saturating_add(1);
                    self.best_frame =
                        Some(self.best_frame.map_or(elapsed, |best| best.min(elapsed)));
                    let event = PunishEvent::Punish { frame: elapsed };
                    self.last_event = Some(event);
                    return Some(event);
                }
            }
            self.window_remaining -= 1;
            if self.window_remaining == 0 {
                self.late_remaining = LATE_WINDOW_FRAMES;
                let event = if self.saw_attack {
                    self.blocked = self.blocked.saturating_add(1);
                    PunishEvent::Blocked
                } else {
                    self.missed = self.missed.saturating_add(1);
                    PunishEvent::Missed
                };
                self.attempts = self.attempts.saturating_add(1);
                self.last_event = Some(event);
                return Some(event);
            }
            return None;
        }

        if self.late_remaining > 0 {
            let elapsed = PUNISH_WINDOW_FRAMES + (LATE_WINDOW_FRAMES - self.late_remaining + 1);
            if let Some(base) = self.baseline_hp {
                if p2_hp > 0 && p2_hp < base {
                    self.late_remaining = 0;
                    self.baseline_hp = Some(p2_hp);
                    self.late_hits = self.late_hits.saturating_add(1);
                    let event = PunishEvent::Late { frame: elapsed };
                    self.last_event = Some(event);
                    return Some(event);
                }
            }
            self.late_remaining -= 1;
        }
        None
    }

    pub fn status_label(&self) -> String {
        if !self.enabled {
            return "OFF".into();
        }
        if self.window_remaining > 0 {
            return format!("ARMED {}F", self.window_remaining);
        }
        let result = self
            .last_event
            .map(PunishEvent::label)
            .unwrap_or_else(|| "READY".into());
        let best = self
            .best_frame
            .map(|frame| format!(" BEST {frame}F"))
            .unwrap_or_default();
        format!(
            "{result} {}/{}{}",
            self.punishes,
            self.attempts.max(1),
            best
        )
    }
}

pub fn format_frames(frames: usize) -> String {
    format!("{:.1}s", frames as f32 / 55.0)
}

fn direction_mask() -> u16 {
    let mut bits = 0;
    for action in [Action::Up, Action::Down, Action::Left, Action::Right] {
        set_action(&mut bits, action);
    }
    bits
}

/// The five buttons the select screen treats as "choice made"
/// (MKSEL.ASM `psel_cursor_loop`). Start is deliberately absent — on the
/// select screen it's only the Scorpion start+coin palette cheat, and
/// mirroring it from P1 could trigger that by accident.
fn button_mask() -> u16 {
    let mut bits = 0;
    for action in [
        Action::HighPunch,
        Action::LowPunch,
        Action::HighKick,
        Action::LowKick,
        Action::Block,
    ] {
        set_action(&mut bits, action);
    }
    bits
}

fn steer_mask() -> u16 {
    direction_mask() | button_mask()
}

fn set_action(bits: &mut u16, action: Action) {
    if let Some(index) = Action::ALL
        .iter()
        .position(|candidate| *candidate == action)
    {
        *bits |= 1u16 << index;
    }
}

fn is_attack(bits: u16) -> bool {
    has_action(bits, Action::HighPunch)
        || has_action(bits, Action::LowPunch)
        || has_action(bits, Action::HighKick)
        || has_action(bits, Action::LowKick)
}

fn has_action(bits: u16, action: Action) -> bool {
    Action::ALL
        .iter()
        .position(|candidate| *candidate == action)
        .map(|index| bits & (1u16 << index) != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(bits: u16, action: Action) -> bool {
        let index = Action::ALL
            .iter()
            .position(|candidate| *candidate == action)
            .unwrap();
        bits & (1u16 << index) != 0
    }

    #[test]
    fn position_presets_cycle_back_to_midscreen() {
        assert_eq!(PositionPreset::Midscreen.next(), PositionPreset::P2Corner);
        assert_eq!(PositionPreset::P2Corner.next(), PositionPreset::P1Corner);
        assert_eq!(PositionPreset::P1Corner.next(), PositionPreset::Midscreen);
    }

    #[test]
    fn reset_slots_cycle_and_track_saved_slot() {
        let mut slots = ResetSlots::default();
        assert_eq!(slots.active_number(), 1);
        assert_eq!(slots.active_status_label(), "S1");
        assert!(!slots.active_saved());
        assert_eq!(slots.save_active(vec![1, 2, 3]), 3);
        assert_eq!(slots.active_status_label(), "S1*");
        assert_eq!(slots.cycle_next(), 2);
        assert_eq!(slots.active_status_label(), "S2");
        assert!(!slots.active_saved());
        assert_eq!(slots.cycle_next(), 3);
        assert_eq!(slots.cycle_next(), 1);
        assert_eq!(slots.active_status_label(), "S1*");
    }

    #[test]
    fn reset_slots_clear_returns_to_empty_slot_one() {
        let mut slots = ResetSlots::default();
        slots.save_active(vec![1]);
        slots.cycle_next();
        slots.save_active(vec![2]);
        slots.clear();
        assert_eq!(slots.active_number(), 1);
        assert_eq!(slots.active_status_label(), "S1");
        assert!(!slots.active_saved());
    }

    #[test]
    fn dummy_modes_cycle_back_to_stand() {
        let mut dummy = DummyController::default();
        assert_eq!(dummy.mode(), DummyMode::Stand);
        assert_eq!(dummy.cycle_mode(), DummyMode::Crouch);
        assert_eq!(dummy.cycle_mode(), DummyMode::Block);
        assert_eq!(dummy.cycle_mode(), DummyMode::CrouchBlock);
        assert_eq!(dummy.cycle_mode(), DummyMode::Jump);
        assert_eq!(dummy.cycle_mode(), DummyMode::JumpIn);
        assert_eq!(dummy.cycle_mode(), DummyMode::ReversalMash);
        assert_eq!(dummy.cycle_mode(), DummyMode::ThrowTech);
        assert_eq!(dummy.cycle_mode(), DummyMode::WakeBlock);
        assert_eq!(dummy.cycle_mode(), DummyMode::Off);
        assert_eq!(dummy.cycle_mode(), DummyMode::Stand);
    }

    #[test]
    fn crouch_block_holds_down_and_block() {
        let mut dummy = DummyController {
            mode: DummyMode::CrouchBlock,
            frame: 0,
            ..DummyController::default()
        };
        let bits = dummy.next_bits(LabPhase::Fight, true, 0, 0).unwrap();
        assert!(has(bits, Action::Down));
        assert!(has(bits, Action::Block));
    }

    #[test]
    fn off_mode_does_not_own_p2() {
        let mut dummy = DummyController {
            mode: DummyMode::Off,
            frame: 0,
            ..DummyController::default()
        };
        assert_eq!(dummy.next_bits(LabPhase::Fight, true, 0, 0), None);
    }

    #[test]
    fn jump_in_jumps_toward_p1_from_either_side() {
        let mut from_right = DummyController {
            mode: DummyMode::JumpIn,
            frame: 0,
            ..DummyController::default()
        };
        let bits = from_right.next_bits(LabPhase::Fight, true, 0, 0).unwrap();
        assert!(has(bits, Action::Up));
        assert!(has(bits, Action::Left));

        let mut from_left = DummyController {
            mode: DummyMode::JumpIn,
            frame: 0,
            ..DummyController::default()
        };
        let bits = from_left.next_bits(LabPhase::Fight, false, 0, 0).unwrap();
        assert!(has(bits, Action::Up));
        assert!(has(bits, Action::Right));
    }

    #[test]
    fn mash_modes_release_block_for_the_press_frames() {
        // MK2 ignores attacks while Block is held — the press frames must
        // carry LP *without* block or the mash never comes out.
        for mode in [DummyMode::ReversalMash, DummyMode::ThrowTech] {
            let mut dummy = DummyController {
                mode,
                frame: 0,
                ..DummyController::default()
            };
            let mut saw_clean_press = false;
            let mut saw_guard = false;
            for _ in 0..24 {
                let bits = dummy.next_bits(LabPhase::Fight, true, 0, 0).unwrap();
                if has(bits, Action::LowPunch) {
                    assert!(!has(bits, Action::Block), "{mode:?} pressed LP under block");
                    saw_clean_press = true;
                } else if has(bits, Action::Block) {
                    saw_guard = true;
                }
            }
            assert!(saw_clean_press, "{mode:?} never attempted its press");
            assert!(saw_guard, "{mode:?} never guarded between presses");
        }
    }

    #[test]
    fn wake_block_crouch_blocks() {
        let mut wake_block = DummyController {
            mode: DummyMode::WakeBlock,
            frame: 0,
            ..DummyController::default()
        };
        let bits = wake_block.next_bits(LabPhase::Fight, true, 0, 0).unwrap();
        assert!(has(bits, Action::Down));
        assert!(has(bits, Action::Block));
    }

    #[test]
    fn recording_becomes_looping_dummy() {
        let mut dummy = DummyController::default();
        dummy.start_recording();
        assert!(dummy.is_recording());
        assert!(!dummy.has_loop());
        assert_eq!(
            dummy.next_bits(LabPhase::Fight, true, 0, 0x0001),
            Some(0x0001)
        );
        assert_eq!(
            dummy.next_bits(LabPhase::Fight, true, 0, 0x0002),
            Some(0x0002)
        );
        assert_eq!(dummy.stop_recording(), 2);
        assert!(!dummy.is_recording());
        assert!(dummy.has_loop());
        assert_eq!(dummy.next_bits(LabPhase::Fight, true, 0, 0), Some(0x0001));
        assert_eq!(dummy.next_bits(LabPhase::Fight, true, 0, 0), Some(0x0002));
        assert_eq!(dummy.next_bits(LabPhase::Fight, true, 0, 0), Some(0x0001));
    }

    #[test]
    fn loop_completion_is_reported_once() {
        let mut dummy = DummyController::default();
        dummy.start_recording();
        dummy.next_bits(LabPhase::Fight, true, 0, 0x0001);
        dummy.next_bits(LabPhase::Fight, true, 0, 0x0002);
        dummy.stop_recording();
        dummy.next_bits(LabPhase::Fight, true, 0, 0);
        assert_eq!(dummy.take_loop_completed(), None);
        dummy.next_bits(LabPhase::Fight, true, 0, 0);
        assert_eq!(dummy.take_loop_completed(), Some(2));
        assert_eq!(dummy.take_loop_completed(), None);
    }

    fn bit(action: Action) -> u16 {
        let mut bits = 0;
        set_action(&mut bits, action);
        bits
    }

    #[test]
    fn phase_from_ram_classifies_select_states() {
        assert_eq!(phase_from_ram(true, 0, 0), LabPhase::Fight);
        assert_eq!(
            phase_from_ram(false, CHAR_NULLED, CHAR_NULLED),
            LabPhase::Select {
                p1_picked: false,
                p2_picked: false
            }
        );
        assert_eq!(
            phase_from_ram(false, 3, CHAR_NULLED),
            LabPhase::Select {
                p1_picked: true,
                p2_picked: false
            }
        );
        assert_eq!(
            phase_from_ram(false, 3, 7),
            LabPhase::PreFight,
            "both ids real outside a fight = attract or a transition screen"
        );
    }

    #[test]
    fn select_pulses_start_but_never_picks_before_p1_locks_in() {
        let mut dummy = DummyController::default();
        let waiting = LabPhase::Select {
            p1_picked: false,
            p2_picked: false,
        };
        let mut saw_start = false;
        for _ in 0..24 {
            let bits = dummy.next_bits(waiting, true, 0, 0).unwrap();
            saw_start |= has(bits, Action::Start);
            assert_eq!(bits & button_mask(), 0, "no pick button before P1 locks in");
        }
        assert!(saw_start, "P2 must keep joining in during select");
    }

    #[test]
    fn select_mirrors_p1_to_p2_after_p1_locks_in() {
        let mut dummy = DummyController::default();
        let mirror = LabPhase::Select {
            p1_picked: true,
            p2_picked: false,
        };
        // P1's confirm press is still held: directions pass, buttons don't.
        let held = bit(Action::LowPunch) | bit(Action::Left);
        let bits = dummy.next_bits(mirror, true, held, 0).unwrap();
        assert!(has(bits, Action::Left));
        assert!(!has(bits, Action::LowPunch));
        // Release arms the mirror; the next press confirms P2's pick.
        let bits = dummy.next_bits(mirror, true, 0, 0).unwrap();
        assert_eq!(bits, 0);
        let bits = dummy.next_bits(mirror, true, held, 0).unwrap();
        assert!(has(bits, Action::Left));
        assert!(has(bits, Action::LowPunch));
        // Start never mirrors (select-screen palette cheat).
        let bits = dummy.next_bits(mirror, true, bit(Action::Start), 0).unwrap();
        assert!(!has(bits, Action::Start));
    }

    #[test]
    fn select_goes_quiet_once_p2_is_picked() {
        let mut dummy = DummyController::default();
        let done = LabPhase::Select {
            p1_picked: true,
            p2_picked: true,
        };
        assert_eq!(dummy.next_bits(done, true, bit(Action::LowPunch), 0), Some(0));
    }

    #[test]
    fn physical_p2_controls_steer_during_select() {
        let mut dummy = DummyController::default();
        let waiting = LabPhase::Select {
            p1_picked: false,
            p2_picked: false,
        };
        let bits = dummy
            .next_bits(waiting, true, 0, bit(Action::Right) | bit(Action::HighKick))
            .unwrap();
        assert!(has(bits, Action::Right));
        assert!(has(bits, Action::HighKick));
    }

    #[test]
    fn punish_trainer_scores_fast_damage() {
        let mut trainer = PunishTrainer::default();
        trainer.toggle();
        trainer.arm(161);
        let event = trainer.observe(140, 0);
        assert_eq!(event, Some(PunishEvent::Punish { frame: 1 }));
        assert!(trainer.status_label().contains("PUNISH"));
    }

    #[test]
    fn punish_trainer_scores_blocked_attack() {
        let mut trainer = PunishTrainer::default();
        trainer.toggle();
        trainer.arm(161);
        let attack = 1u16
            << Action::ALL
                .iter()
                .position(|candidate| *candidate == Action::HighPunch)
                .unwrap();
        let mut event = None;
        for _ in 0..PUNISH_WINDOW_FRAMES {
            event = trainer.observe(161, attack);
        }
        assert_eq!(event, Some(PunishEvent::Blocked));
    }

    #[test]
    fn punish_trainer_reports_late_damage_after_miss() {
        let mut trainer = PunishTrainer::default();
        trainer.toggle();
        trainer.arm(161);
        let mut event = None;
        for _ in 0..PUNISH_WINDOW_FRAMES {
            event = trainer.observe(161, 0);
        }
        assert_eq!(event, Some(PunishEvent::Missed));
        assert_eq!(
            trainer.observe(150, 0),
            Some(PunishEvent::Late {
                frame: PUNISH_WINDOW_FRAMES + 1
            })
        );
    }

    #[test]
    fn damage_tracker_counts_combo_damage() {
        let mut tracker = DamageTracker::default();
        tracker.observe(true, 161);
        tracker.observe(true, 150);
        tracker.observe(true, 140);
        assert!(tracker.status_label().contains("21 H2"));
    }

    #[test]
    fn damage_tracker_splits_attempts_after_gap() {
        let mut tracker = DamageTracker::default();
        tracker.observe(true, 161);
        tracker.observe(true, 150);
        for _ in 0..DAMAGE_COMBO_GAP_FRAMES {
            tracker.observe(true, 150);
        }
        tracker.observe(true, 140);
        let status = tracker.status_label();
        assert!(status.contains("10 H1"));
        assert!(status.contains("TRY 2"));
    }
}
