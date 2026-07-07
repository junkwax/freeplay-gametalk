//! One-frame runahead for offline interactive play.
//!
//! Each displayed tick advances the canonical emulation by exactly one
//! frame, but the frame shown on screen is one frame *further* into the
//! game's future, computed by speculatively re-running with the same input.
//! Since MK2 (like most arcade games) has internal input latency, showing
//! frame T+1's image while the canonical state sits at T removes one frame
//! of that pipeline — the same trick RetroArch runahead uses, driven through
//! the same libretro machinery (GET_AUDIO_VIDEO_ENABLE) our netplay resim
//! path already exercises.
//!
//! Per tick:
//!   1. hidden run    — canonical T -> T+1 with this tick's inputs; video
//!                      skipped (pBurnDraw=NULL via the silent env answer),
//!                      audio emulated but not presented (determinism-safe,
//!                      and its samples would duplicate what the previous
//!                      tick's visible run already played).
//!   2. save          — snapshot canonical T+1 into a reused scratch buffer.
//!   3. visible run   — speculative T+1 -> T+2 with the same inputs; this
//!                      frame's video and audio are what the user sees/hears.
//!   4. load          — restore canonical T+1, so everything the frame loop
//!                      does between ticks (Lab trainer pokes, drone input
//!                      injection, HUD memory peeks, score tracking) operates
//!                      on real state, never speculative state.
//!
//! If the speculation was "wrong" (the user's input changes next tick), the
//! next visible frame simply reflects the corrected future — a one-frame
//! visual revision identical in kind to netplay's one-frame prediction,
//! which is imperceptible.
//!
//! Netplay must never use this (ggrs owns save/load and prediction there),
//! and replay playback gains nothing from it; the call site in main.rs gates
//! on both.

use crate::retro;

pub struct Runahead {
    /// Reused canonical-state buffer; steady state does zero allocations.
    scratch: Vec<u8>,
    /// Set after a serialize/load failure: fall back to plain frame stepping
    /// for the rest of the session rather than failing every tick.
    broken: bool,
    logged_break: bool,
}

impl Default for Runahead {
    fn default() -> Self {
        Self::new()
    }
}

impl Runahead {
    pub fn new() -> Self {
        Self {
            scratch: Vec::new(),
            broken: false,
            logged_break: false,
        }
    }

    /// Advance one canonical frame, displaying one frame ahead.
    /// Falls back to a plain `core.run` if state save/load ever fails.
    pub fn step(&mut self, core: &retro::Core) {
        if self.broken {
            unsafe { (core.run)() };
            return;
        }

        // 1. Hidden canonical advance. silent() makes the env answer clear
        //    the video/audio-present bits and the AV callbacks drop data.
        retro::set_silent(true);
        unsafe { (core.run)() };
        retro::set_silent(false);

        // 2. Snapshot canonical state.
        if !core.save_state_into(&mut self.scratch) {
            // Canonical already advanced; show this tick with a plain
            // visible run (a one-time extra frame of advance) and stop
            // trying to run ahead.
            self.mark_broken("savestate serialize failed");
            unsafe { (core.run)() };
            return;
        }

        // 3. Visible speculative frame.
        unsafe { (core.run)() };

        // 4. Back to canonical for everything between ticks.
        if !core.load_state(&self.scratch) {
            self.mark_broken("savestate load failed");
        }
    }

    fn mark_broken(&mut self, why: &str) {
        self.broken = true;
        if !self.logged_break {
            self.logged_break = true;
            println!("[runahead] disabled for this session: {why}");
            crate::dlog!("runahead", "disabled: {why}");
        }
    }
}
