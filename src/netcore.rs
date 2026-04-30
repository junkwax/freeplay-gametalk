//! Per-frame netplay engine: drives a GGRS session through one tick of
//! poll → events → advance → save/load callbacks. Self-contained — owns
//! `NetRuntime` (per-session mutable state) and produces `NetStepStats` for
//! the caller to surface in HUD/diag.
//!
//! Lives outside `main.rs` because its dependencies are narrow (retro core,
//! ggrs, ghost recording, log file) and the function is pure: the same
//! inputs always produce the same `NetStepStats`. Callers thread the
//! mutables in by reference.

use crate::dlog;
use crate::ghost;
use crate::input;
use crate::memory;
use crate::netplay;
use crate::retro::{self, SILENT_MODE};

#[derive(Default, Clone, Copy)]
pub struct NetStepStats {
    pub advance_count: usize,
    pub load_count: usize,
    pub peer_disconnected: bool,
}

#[derive(Default)]
pub struct NetRuntime {
    pub skip_frames_remaining: u32,
    pub desync_dumped: bool,
    /// Byte offset of the FBNeo wall-clock u32 inside the libretro savestate.
    /// Detected on the first save_state by scanning for a u32 close to current
    /// unix time; once found, those 4 bytes are excluded from the GGRS cksum so
    /// host/join clock skew never registers as a desync. See
    /// project_frame30_desync_root_cause memory note for full diagnosis.
    pub rtc_mask_offset: Option<usize>,
}

/// Wipe trainer-flag RAM and drop all training-only state before a netplay
/// session starts. Both peers must run this so they begin from canonical
/// state — any non-zero training byte would desync GGRS on the first
/// checksum.
pub fn reset_for_netplay(
    core: &retro::Core,
    trainer: &mut memory::PokeList,
    save_slot: &mut Option<Vec<u8>>,
    ghost_playback: &mut Option<ghost::Playback>,
    ghost_recording: &mut Option<ghost::Recording>,
) {
    use memory::{poke_u16, Endian};
    const ZERO_TARGETS: &[usize] = &[0x250EE, 0x2576E];
    for addr in ZERO_TARGETS {
        poke_u16(core, *addr, 0x0000, Endian::Little);
    }
    trainer.set_enabled("hitboxes", false);
    trainer.set_enabled("p1_health", false);
    trainer.set_enabled("p2_health", false);
    trainer.set_enabled("freeze_timer", false);
    *save_slot = None;
    *ghost_playback = None;
    *ghost_recording = None;
}

/// Find the FBNeo RTC slot in a libretro savestate by scanning for a u32 (LE)
/// that's within ~1 day of the current wall clock. FBNeo serializes the host's
/// `time_t` into the savestate; that one word is the only thing that ever
/// differs between two peers running the same ROM in sync, so the GGRS
/// state-hash always disagrees by exactly that bit-pattern. We zero those 4
/// bytes out of the cksum to suppress the false-positive desync.
///
/// Returns the byte offset of the matching word, or None if nothing in range
/// looks like a wall clock (in which case we leave cksum unmodified — the
/// caller will simply revert to whole-blob hashing).
pub fn detect_rtc_offset(blob: &[u8]) -> Option<usize> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as u32;
    // Search from the END of the blob: FBNeo's RTC device is serialized late,
    // and starting at the back avoids matching random byte patterns elsewhere.
    let window: i64 = 24 * 60 * 60; // accept clocks within 1 day of ours
    let mut best: Option<usize> = None;
    for off in (0..blob.len().saturating_sub(4)).rev() {
        let v = u32::from_le_bytes([blob[off], blob[off + 1], blob[off + 2], blob[off + 3]]);
        let delta = (v as i64) - (now as i64);
        if delta.abs() < window {
            best = Some(off);
            break;
        }
    }
    best
}

/// XOR-fold the savestate into a u128, optionally zeroing 4 bytes at `mask_off`
/// so the FBNeo wall-clock slot doesn't poison the hash.
pub fn cksum_with_mask(blob: &[u8], mask_off: Option<usize>) -> u128 {
    let mut cksum: u128 = 0;
    for (i, chunk) in blob.chunks(16).enumerate() {
        let mut w = [0u8; 16];
        w[..chunk.len()].copy_from_slice(chunk);
        if let Some(off) = mask_off {
            let chunk_start = i * 16;
            let chunk_end = chunk_start + chunk.len();
            // Zero any of the 4 RTC bytes that fall inside this chunk.
            for j in 0..4 {
                let pos = off + j;
                if pos >= chunk_start && pos < chunk_end {
                    w[pos - chunk_start] = 0;
                }
            }
        }
        cksum ^= u128::from_le_bytes(w);
    }
    cksum
}

#[allow(static_mut_refs)]
pub fn step_netplay_frame(
    core: &retro::Core,
    sess: &mut netplay::Session,
    local_handle: usize,
    net_recording: &mut Option<ghost::NetRecording>,
    net_log: &mut Option<std::fs::File>,
    runtime: &mut NetRuntime,
) -> NetStepStats {
    use ggrs::{GgrsRequest, PlayerHandle, SessionState};
    use input::{apply_snapshot, snapshot_player, Player};
    use netplay::NetInput;
    use std::io::Write;

    let mut peer_disconnected_this_frame = false;

    sess.poll_remote_clients();

    for ev in sess.events() {
        match ev {
            ggrs::GgrsEvent::WaitRecommendation { skip_frames } => {
                runtime.skip_frames_remaining =
                    runtime.skip_frames_remaining.saturating_add(skip_frames);
                if let Some(f) = net_log.as_mut() {
                    let _ = writeln!(
                        f,
                        "[net/wait] recommend skip {} frames (total pending {})",
                        skip_frames, runtime.skip_frames_remaining
                    );
                }
            }
            ggrs::GgrsEvent::Disconnected { addr } => {
                if let Some(f) = net_log.as_mut() {
                    let _ = writeln!(f, "[net/evt] disconnected: {addr}");
                }
                peer_disconnected_this_frame = true;
            }
            ggrs::GgrsEvent::NetworkInterrupted {
                addr,
                disconnect_timeout,
            } => {
                if let Some(f) = net_log.as_mut() {
                    let _ = writeln!(f, "[net/evt] network interrupted: {addr}  hard-timeout-in-ms={disconnect_timeout}");
                }
            }
            ggrs::GgrsEvent::NetworkResumed { addr } => {
                if let Some(f) = net_log.as_mut() {
                    let _ = writeln!(f, "[net/evt] network resumed: {addr}");
                }
            }
            ggrs::GgrsEvent::Synchronizing { addr, total, count } => {
                if let Some(f) = net_log.as_mut() {
                    let _ = writeln!(f, "[net/evt] sync {count}/{total} with {addr}");
                }
            }
            ggrs::GgrsEvent::Synchronized { addr } => {
                if let Some(f) = net_log.as_mut() {
                    let _ = writeln!(f, "[net/evt] synchronized with {addr}");
                }
            }
            ggrs::GgrsEvent::DesyncDetected {
                frame,
                local_checksum,
                remote_checksum,
                addr,
            } => {
                let line = format!(
                    "[net/err] DESYNC frame={frame} local=0x{local_checksum:x} remote=0x{remote_checksum:x} peer={addr}"
                );
                println!("{line}");
                if let Some(f) = net_log.as_mut() {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
    }

    let session_state = sess.current_state();
    if session_state != SessionState::Running {
        dlog!("net", "session not running, state={:?}", session_state);
        return NetStepStats {
            peer_disconnected: peer_disconnected_this_frame,
            ..NetStepStats::default()
        };
    }

    if runtime.skip_frames_remaining > 0 {
        runtime.skip_frames_remaining -= 1;
        return NetStepStats {
            peer_disconnected: peer_disconnected_this_frame,
            ..NetStepStats::default()
        };
    }

    let local_player = if local_handle == 0 {
        Player::P1
    } else {
        Player::P2
    };
    let local_bits = snapshot_player(local_player);
    let local_input = NetInput { bits: local_bits };

    if let Err(e) = sess.add_local_input(local_handle as PlayerHandle, local_input) {
        dlog!("net", "add_local_input err={e:?}");
        if let Some(f) = net_log.as_mut() {
            let _ = writeln!(f, "[net/err] add_local_input: {e:?}");
        }
        return NetStepStats {
            peer_disconnected: peer_disconnected_this_frame,
            ..NetStepStats::default()
        };
    }

    let requests = match sess.advance_frame() {
        Ok(reqs) => reqs,
        Err(e) => {
            if !matches!(e, ggrs::GgrsError::PredictionThreshold) {
                dlog!("net", "advance_frame err={e:?}");
                if let Some(f) = net_log.as_mut() {
                    let _ = writeln!(f, "[net/err] advance_frame: {e:?}");
                }
            }
            return NetStepStats {
                peer_disconnected: peer_disconnected_this_frame,
                ..NetStepStats::default()
            };
        }
    };

    let advance_count = requests
        .iter()
        .filter(|r| matches!(r, GgrsRequest::AdvanceFrame { .. }))
        .count();
    let save_count = requests
        .iter()
        .filter(|r| matches!(r, GgrsRequest::SaveGameState { .. }))
        .count();
    let load_count = requests
        .iter()
        .filter(|r| matches!(r, GgrsRequest::LoadGameState { .. }))
        .count();
    dlog!(
        "net",
        "frame local_bits=0x{:04x} reqs: advance={} save={} load={}",
        local_bits,
        advance_count,
        save_count,
        load_count
    );

    let mut advances_seen = 0usize;

    for req in requests {
        match req {
            GgrsRequest::SaveGameState { cell, frame } => {
                let blob = core.save_state().unwrap_or_default();
                if runtime.rtc_mask_offset.is_none() {
                    if let Some(off) = detect_rtc_offset(&blob) {
                        runtime.rtc_mask_offset = Some(off);
                        let m = format!("[net] FBNeo RTC slot at savestate offset 0x{off:x} — masking from cksum");
                        println!("{m}");
                        if let Some(f) = net_log.as_mut() {
                            let _ = writeln!(f, "{m}");
                        }
                    }
                }
                let cksum = cksum_with_mask(&blob, runtime.rtc_mask_offset);
                dlog!(
                    "net",
                    "SaveGameState frame={frame} bytes={} cksum=0x{:032x}",
                    blob.len(),
                    cksum
                );
                const DUMP_FRAME: i32 = 30;
                if frame == DUMP_FRAME && !runtime.desync_dumped {
                    runtime.desync_dumped = true;
                    let tag = if local_handle == 0 { "host" } else { "join" };
                    let path = format!("presync_{}_frame{}.state", tag, frame);
                    let _ = std::fs::write(&path, &blob);
                    let m = format!(
                        "[net/err] dumped pre-sync savestate ({} bytes, cksum=0x{:032x}) to {}",
                        blob.len(),
                        cksum,
                        path
                    );
                    println!("{m}");
                    if let Some(f) = net_log.as_mut() {
                        let _ = writeln!(f, "{m}");
                    }
                }
                cell.save(frame, Some(blob), Some(cksum));
            }
            GgrsRequest::LoadGameState { cell, frame } => {
                if let Some(blob) = cell.load() {
                    let ok = core.load_state(&blob);
                    dlog!(
                        "net",
                        "LoadGameState frame={frame} bytes={} ok={}",
                        blob.len(),
                        ok
                    );
                    if !ok {
                        println!("[net] load_state rejected blob during rollback");
                        if let Some(f) = net_log.as_mut() {
                            let _ = writeln!(
                                f,
                                "[net/err] LoadGameState rejected at frame={} size={}",
                                frame,
                                blob.len()
                            );
                        }
                    }
                } else {
                    dlog!("net", "LoadGameState frame={frame} -- cell empty!");
                }
            }
            GgrsRequest::AdvanceFrame { inputs } => {
                advances_seen += 1;
                let is_last = advances_seen == advance_count;
                unsafe {
                    SILENT_MODE = !is_last;
                }
                let p1_bits = inputs[0].0.bits;
                let p2_bits = inputs[1].0.bits;
                dlog!(
                    "net",
                    "AdvanceFrame #{}/{} p1=0x{:04x} p2=0x{:04x} silent={}",
                    advances_seen,
                    advance_count,
                    p1_bits,
                    p2_bits,
                    !is_last
                );
                apply_snapshot(Player::P1, p1_bits);
                apply_snapshot(Player::P2, p2_bits);
                if is_last {
                    if let Some(rec) = net_recording.as_mut() {
                        rec.record_confirmed_frame(core, p1_bits, p2_bits);
                    }
                }
                unsafe {
                    (core.run)();
                }
            }
        }
    }
    unsafe {
        SILENT_MODE = false;
    }
    NetStepStats {
        advance_count,
        load_count,
        peer_disconnected: peer_disconnected_this_frame,
    }
}
