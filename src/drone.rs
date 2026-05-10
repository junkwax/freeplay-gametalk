//! Drone — smart ghost playback that reacts to the opponent.
#![allow(dead_code)]
// Instead of playing a ghost frame-by-frame sequentially, the drone reads
// the live game state (distance, posture, health) and picks the best-
// matching ghost segment to inject. This creates an AI that plays like the
// recorded player but adapts to the current situation.
//
// ## RAM addresses (MK2 T-Unit 3.0, FBNeo SYSTEM_RAM, little-endian)
//
// These offsets are derived from the MK2 source tree. If addresses change
// between ROM revisions, press Shift+F11 to dump RAM, move char, dump again,
// and diff with `python ram_diff.py before.bin after.bin`.
//
//   P1_X        = 0x253E8  s16   horizontal position
//   P1_Y        = 0x253EA  s16   vertical position (0=ground)
//   P2_X        = 0x25562  s16
//   P2_Y        = 0x25564  s16

macro_rules! dlog {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        println!("{msg}");
        let _ = std::fs::OpenOptions::new()
            .create(true).append(true)
            .open("drone.log")
            .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{msg}\n").as_bytes()));
    }};
}

use crate::ghost::Playback;
use crate::memory;
use crate::retro::{
    Core, RETRO_DEVICE_ID_JOYPAD_A, RETRO_DEVICE_ID_JOYPAD_B, RETRO_DEVICE_ID_JOYPAD_DOWN,
    RETRO_DEVICE_ID_JOYPAD_L, RETRO_DEVICE_ID_JOYPAD_LEFT, RETRO_DEVICE_ID_JOYPAD_RIGHT,
    RETRO_DEVICE_ID_JOYPAD_UP, RETRO_DEVICE_ID_JOYPAD_X, RETRO_DEVICE_ID_JOYPAD_Y,
};

const P1_X_ADDR: usize = 0x253E8;
const P1_Y_ADDR: usize = 0x253EA;
const P2_X_ADDR: usize = 0x25562;
const P2_Y_ADDR: usize = 0x25564;

const CLOSE_RANGE: i16 = 60;
const MID_RANGE: i16 = 140;

// ── Posture ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Posture {
    Idle,
    Crouching,
    Jumping,
    Advancing,
    Retreating,
    Attacking,
    Blocking,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Range {
    Close,
    Mid,
    Far,
}

// ── Game state ─────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug)]
pub struct GameState {
    pub p1_x: i16,
    pub p1_y: i16,
    pub p2_x: i16,
    pub p2_y: i16,
}

impl GameState {
    pub fn read(core: &Core) -> Self {
        Self {
            p1_x: memory::peek_u16(core, P1_X_ADDR, memory::Endian::Little).unwrap_or(0) as i16,
            p1_y: memory::peek_u16(core, P1_Y_ADDR, memory::Endian::Little).unwrap_or(0) as i16,
            p2_x: memory::peek_u16(core, P2_X_ADDR, memory::Endian::Little).unwrap_or(0) as i16,
            p2_y: memory::peek_u16(core, P2_Y_ADDR, memory::Endian::Little).unwrap_or(0) as i16,
        }
    }

    pub fn distance(&self) -> i16 {
        (self.p1_x as i32 - self.p2_x as i32).abs() as i16
    }

    pub fn range(&self) -> Range {
        let d = self.distance();
        if d < CLOSE_RANGE {
            Range::Close
        } else if d < MID_RANGE {
            Range::Mid
        } else {
            Range::Far
        }
    }
}

// ── Drone index ────────────────────────────────────────────────────────────

pub struct DroneIndex {
    tags: Vec<Posture>,
    inputs: Vec<u16>,
    source_port: usize,
}

impl DroneIndex {
    pub fn build(pb: &Playback, source_port: usize) -> Self {
        let source_port = source_port.min(1);
        let raw = pb.inputs();
        let mut tags = Vec::with_capacity(raw.len());
        let mut counts = [0u32; 7];
        for frame in raw {
            let input = crate::ghost::without_system_buttons(frame[source_port]);
            let p = classify_inputs(input, source_port);
            counts[p as usize] += 1;
            tags.push(p);
        }
        dlog!(
            "[drone] indexed {} frames from P{}: idle={} crouch={} jump={} adv={} retreat={} atk={} block={}",
            raw.len(),
            source_port + 1,
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            counts[4],
            counts[5],
            counts[6]
        );
        Self {
            tags,
            inputs: raw
                .iter()
                .map(|frame| crate::ghost::without_system_buttons(frame[source_port]))
                .collect(),
            source_port,
        }
    }

    pub fn frames_for(&self, posture: Posture) -> Vec<usize> {
        self.tags
            .iter()
            .enumerate()
            .filter(|(_, &t)| t == posture)
            .map(|(i, _)| i)
            .collect()
    }

    pub fn input_at(&self, idx: usize) -> u16 {
        self.inputs.get(idx).copied().unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    pub fn source_port(&self) -> usize {
        self.source_port
    }
}

// ── Drone runner ───────────────────────────────────────────────────────────

pub struct DroneRunner {
    index: DroneIndex,
    target_port: usize,
    cursor: usize,
    active_posture: Posture,
    cycle: usize,
    eval_timer: u32,
}

impl DroneRunner {
    pub fn new(index: DroneIndex, target_port: usize) -> Self {
        Self {
            target_port: target_port.min(1),
            index,
            cursor: 0,
            active_posture: Posture::Idle,
            cycle: 0,
            eval_timer: 0,
        }
    }

    pub fn next_input(&mut self, gs: &GameState) -> u16 {
        self.eval_timer = self.eval_timer.wrapping_add(1);

        if self.index.len() == 0 {
            return 0;
        }

        if self.eval_timer == 1 || self.eval_timer % 30 == 0 {
            let prev = self.active_posture;
            self.active_posture = self.choose_posture(gs);
            self.cycle = 0;
            dlog!(
                "[drone] frame={} dist={} range={:?} posture={prev:?}->{:?}",
                self.eval_timer,
                gs.distance(),
                gs.range(),
                self.active_posture
            );
            dlog!(
                "[drone]   p1=({},{}) p2=({},{})",
                gs.p1_x,
                gs.p1_y,
                gs.p2_x,
                gs.p2_y
            );
        }

        let frames = self.index.frames_for(self.active_posture);
        if frames.is_empty() {
            self.cursor = (self.cursor + 1) % self.index.len();
            let input = adapt_direction(
                self.index.input_at(self.cursor),
                self.active_posture,
                gs,
                self.target_port,
            );
            dlog!(
                "[drone] fallback: no {:?} frames, seq cursor={} input=0x{:04x}",
                self.active_posture,
                self.cursor,
                input
            );
            return input;
        }

        self.cursor = frames[self.cycle % frames.len()];
        self.cycle = self.cycle.wrapping_add(1);
        let input = adapt_direction(
            self.index.input_at(self.cursor),
            self.active_posture,
            gs,
            self.target_port,
        );
        dlog!(
            "[drone] inject frame={} ({}/{}) input=0x{:04x}",
            self.cursor,
            self.cycle,
            frames.len(),
            input
        );
        input
    }

    fn choose_posture(&self, gs: &GameState) -> Posture {
        let opponent_y = if self.target_port == 0 {
            gs.p2_y
        } else {
            gs.p1_y
        };
        match gs.range() {
            Range::Close => Posture::Attacking,
            Range::Mid => {
                if opponent_y > 0 {
                    Posture::Jumping
                } else {
                    Posture::Advancing
                }
            }
            Range::Far => Posture::Advancing,
        }
    }
}

// ── Input classification ──────────────────────────────────────────────────

fn classify_inputs(inputs: u16, source_port: usize) -> Posture {
    let up = bit(inputs, RETRO_DEVICE_ID_JOYPAD_UP);
    let down = bit(inputs, RETRO_DEVICE_ID_JOYPAD_DOWN);
    let left = bit(inputs, RETRO_DEVICE_ID_JOYPAD_LEFT);
    let right = bit(inputs, RETRO_DEVICE_ID_JOYPAD_RIGHT);
    let lp = bit(inputs, RETRO_DEVICE_ID_JOYPAD_B);
    let hp = bit(inputs, RETRO_DEVICE_ID_JOYPAD_Y);
    let lk = bit(inputs, RETRO_DEVICE_ID_JOYPAD_A);
    let hk = bit(inputs, RETRO_DEVICE_ID_JOYPAD_X);
    let block = bit(inputs, RETRO_DEVICE_ID_JOYPAD_L);

    if hp || lp || hk || lk {
        return Posture::Attacking;
    }
    if block {
        return Posture::Blocking;
    }
    if up {
        return Posture::Jumping;
    }
    if down {
        return Posture::Crouching;
    }
    if left ^ right {
        let advancing = if source_port == 0 { right } else { left };
        return if advancing {
            Posture::Advancing
        } else {
            Posture::Retreating
        };
    }
    Posture::Idle
}

fn bit(inputs: u16, bit: u32) -> bool {
    inputs & (1u16 << bit) != 0
}

fn set_bit(inputs: &mut u16, bit: u32, pressed: bool) {
    if pressed {
        *inputs |= 1u16 << bit;
    } else {
        *inputs &= !(1u16 << bit);
    }
}

fn adapt_direction(mut input: u16, posture: Posture, gs: &GameState, target_port: usize) -> u16 {
    input = crate::ghost::without_system_buttons(input);
    if !matches!(posture, Posture::Advancing | Posture::Retreating) {
        return input;
    }

    let (target_x, opponent_x) = if target_port == 0 {
        (gs.p1_x, gs.p2_x)
    } else {
        (gs.p2_x, gs.p1_x)
    };
    let toward_left = target_x > opponent_x;
    let press_left = if posture == Posture::Advancing {
        toward_left
    } else {
        !toward_left
    };

    set_bit(&mut input, RETRO_DEVICE_ID_JOYPAD_LEFT, press_left);
    set_bit(&mut input, RETRO_DEVICE_ID_JOYPAD_RIGHT, !press_left);
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p2_left_is_advancing_from_default_side() {
        assert_eq!(
            classify_inputs(1u16 << RETRO_DEVICE_ID_JOYPAD_LEFT, 1),
            Posture::Advancing
        );
        assert_eq!(
            classify_inputs(1u16 << RETRO_DEVICE_ID_JOYPAD_RIGHT, 1),
            Posture::Retreating
        );
    }

    #[test]
    fn p2_advancing_turns_toward_p1() {
        let gs = GameState {
            p1_x: 100,
            p1_y: 0,
            p2_x: 200,
            p2_y: 0,
        };
        let input = adapt_direction(0, Posture::Advancing, &gs, 1);
        assert!(bit(input, RETRO_DEVICE_ID_JOYPAD_LEFT));
        assert!(!bit(input, RETRO_DEVICE_ID_JOYPAD_RIGHT));
    }

    #[test]
    fn combat_logic_strips_coin_and_start() {
        let input = (1u16 << crate::retro::RETRO_DEVICE_ID_JOYPAD_SELECT)
            | (1u16 << crate::retro::RETRO_DEVICE_ID_JOYPAD_START)
            | (1u16 << RETRO_DEVICE_ID_JOYPAD_B);
        let out = adapt_direction(input, Posture::Attacking, &GameState::read_dummy(), 1);
        assert!(!bit(out, crate::retro::RETRO_DEVICE_ID_JOYPAD_SELECT));
        assert!(!bit(out, crate::retro::RETRO_DEVICE_ID_JOYPAD_START));
        assert!(bit(out, RETRO_DEVICE_ID_JOYPAD_B));
    }

    impl GameState {
        fn read_dummy() -> Self {
            Self {
                p1_x: 0,
                p1_y: 0,
                p2_x: 0,
                p2_y: 0,
            }
        }
    }
}
