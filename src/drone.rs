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
// between ROM revisions, press F11 to dump RAM, move char, dump again,
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
use crate::retro::Core;

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
    inputs: Vec<[u16; 2]>,
}

impl DroneIndex {
    pub fn build(pb: &Playback) -> Self {
        let raw = pb.inputs();
        let mut tags = Vec::with_capacity(raw.len());
        let mut counts = [0u32; 7];
        for frame in raw {
            let p1_input = frame[0];
            let p = classify_inputs(p1_input);
            counts[p as usize] += 1;
            tags.push(p);
        }
        dlog!(
            "[drone] indexed {} frames: idle={} crouch={} jump={} adv={} atk={} block={}",
            raw.len(),
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            counts[4],
            counts[5]
        );
        Self {
            tags,
            inputs: raw.to_vec(),
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

    pub fn input_at(&self, idx: usize) -> [u16; 2] {
        self.inputs.get(idx).copied().unwrap_or([0, 0])
    }

    pub fn len(&self) -> usize {
        self.inputs.len()
    }
}

// ── Drone runner ───────────────────────────────────────────────────────────

pub struct DroneRunner {
    index: DroneIndex,
    cursor: usize,
    active_posture: Posture,
    cycle: usize,
    eval_timer: u32,
}

impl DroneRunner {
    pub fn new(index: DroneIndex) -> Self {
        Self {
            index,
            cursor: 0,
            active_posture: Posture::Idle,
            cycle: 0,
            eval_timer: 0,
        }
    }

    pub fn next_input(&mut self, gs: &GameState) -> u16 {
        self.eval_timer = self.eval_timer.wrapping_add(1);

        if self.eval_timer % 30 == 0 {
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
            let input = self.index.input_at(self.cursor)[0];
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
        let input = self.index.input_at(self.cursor)[0];
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
        match gs.range() {
            Range::Close => Posture::Attacking,
            Range::Mid => {
                if gs.p1_y > 0 {
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

fn classify_inputs(inputs: u16) -> Posture {
    let up = (inputs >> 0) & 1 != 0;
    let down = (inputs >> 1) & 1 != 0;
    let left = (inputs >> 2) & 1 != 0;
    let right = (inputs >> 3) & 1 != 0;
    let hp = (inputs >> 4) & 1 != 0;
    let lp = (inputs >> 5) & 1 != 0;
    let hk = (inputs >> 6) & 1 != 0;
    let lk = (inputs >> 7) & 1 != 0;
    let block = (inputs >> 8) & 1 != 0;

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
    if left || right {
        return Posture::Advancing;
    }
    Posture::Idle
}
