//! Rewind-test harness — a GGPO prerequisite check.
//!
//! Records N frames of (input_state snapshot, save_state blob). Then rewinds
//! to frame 0 and replays frames 1..N under silent mode, comparing the
//! final save state byte-for-byte with the recorded frame-N blob.
//!
//! If this test passes for a given ROM, the core is deterministic under
//! save/load/replay — the essential property for rollback netcode.
//! If it fails, rollback would desync on that ROM.
#![allow(static_mut_refs)]

use crate::retro::{Core, INPUT_STATE, SILENT_MODE};

/// How many frames of history the test captures before rewinding.
pub const REWIND_FRAMES: usize = 60;

pub struct RewindTest {
    /// One snapshot per recorded frame. snapshots[0] is the state *before*
    /// frame 0 advances (the rewind target); snapshots[i] for i>0 is the
    /// state *after* frame i-1 ran.
    snapshots: Vec<Vec<u8>>,
    /// inputs[i] = INPUT_STATE captured before frame i ran.
    inputs: Vec<[[bool; 16]; 2]>,
    /// Count of frames recorded so far. When this reaches REWIND_FRAMES+1
    /// we have snapshots[0..=N] and inputs[0..N]; time to verify.
    frames_recorded: usize,
}

impl RewindTest {
    pub fn new() -> Self {
        Self {
            snapshots: Vec::with_capacity(REWIND_FRAMES + 1),
            inputs: Vec::with_capacity(REWIND_FRAMES),
            frames_recorded: 0,
        }
    }

    /// Call **before** each `retro_run` while the test is active.
    /// Returns true when recording is complete and `verify()` should be called.
    pub fn record_pre_frame(&mut self, core: &Core) -> bool {
        // Snapshot the state that *will* be stepped by the next retro_run.
        if let Some(buf) = core.save_state() {
            self.snapshots.push(buf);
        } else {
            println!(
                "[rewind] save_state failed at frame {}",
                self.frames_recorded
            );
            return true; // abort the test
        }
        // Capture the input that will drive the step.
        let inputs = unsafe { INPUT_STATE };
        self.inputs.push(inputs);
        self.frames_recorded += 1;
        if self.frames_recorded % 20 == 0 {
            println!(
                "[rewind] recorded {}/{}",
                self.frames_recorded,
                REWIND_FRAMES + 1
            );
        }
        self.frames_recorded > REWIND_FRAMES
    }

    /// Run the rewind + replay check. Reports result to stdout.
    pub fn verify(&self, core: &Core) {
        if self.snapshots.len() < REWIND_FRAMES + 1 {
            println!(
                "[rewind] not enough frames recorded ({} / {})",
                self.snapshots.len(),
                REWIND_FRAMES + 1
            );
            return;
        }

        println!("[rewind] Verifying {} frames of replay...", REWIND_FRAMES);
        let initial = &self.snapshots[0];
        let expected_final = &self.snapshots[REWIND_FRAMES];

        if !core.load_state(initial) {
            println!("[rewind] FAIL: load_state rejected the frame-0 snapshot");
            return;
        }

        // Replay frames 1..=N under silent mode using recorded inputs.
        unsafe {
            SILENT_MODE = true;
        }
        for i in 0..REWIND_FRAMES {
            // Drive the step with the input that was live at frame i.
            unsafe {
                INPUT_STATE = self.inputs[i];
            }
            unsafe {
                (core.run)();
            }

            // Check this frame's state against the recording (catches
            // early divergences).
            let Some(actual) = core.save_state() else {
                unsafe {
                    SILENT_MODE = false;
                }
                println!(
                    "[rewind] FAIL: save_state failed during replay at frame {}",
                    i + 1
                );
                return;
            };
            let recorded = &self.snapshots[i + 1];
            if actual.len() != recorded.len() {
                unsafe {
                    SILENT_MODE = false;
                }
                println!(
                    "[rewind] FAIL: size mismatch at frame {} ({} vs {})",
                    i + 1,
                    actual.len(),
                    recorded.len()
                );
                return;
            }
            if actual != *recorded {
                let first_diff = actual
                    .iter()
                    .zip(recorded.iter())
                    .position(|(a, b)| a != b)
                    .unwrap_or(0);
                unsafe {
                    SILENT_MODE = false;
                }
                println!(
                    "[rewind] FAIL: divergence at frame {} (first byte differs at offset 0x{:x})",
                    i + 1,
                    first_diff
                );
                // Do NOT restore — leaves the divergent state visible, but
                // that's OK: we've already corrupted continuity by rewinding.
                // Load the "live" final state so the player isn't stuck.
                core.load_state(expected_final);
                return;
            }
        }
        unsafe {
            SILENT_MODE = false;
        }

        // Final sanity check
        let Some(final_state) = core.save_state() else {
            println!("[rewind] FAIL: save_state failed on final frame");
            return;
        };
        if &final_state == expected_final {
            println!(
                "[rewind] PASS: {} frames replayed deterministically ({} bytes/state)",
                REWIND_FRAMES,
                final_state.len()
            );
        } else {
            println!("[rewind] FAIL: final state differs from recording despite per-frame match");
        }

        // Restore to the recorded final state so the user's live game resumes
        // from exactly where it left off, not from the replayed re-run.
        core.load_state(expected_final);
    }
}
