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
use crate::lab;
use crate::match_replay;
use crate::memory;
use crate::mk2_addrs;
use crate::netplay;
use crate::retro;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A pristine just-booted savestate captured once at core load, before any
/// local play. Netplay sessions reload it so every match starts from an
/// identical, canonical state — `retro_reset` alone left a prior Lab/Arcade
/// session bleeding into the match (and broke peer determinism).
static CLEAN_BOOT_STATE: Mutex<Option<Vec<u8>>> = Mutex::new(None);

pub fn set_clean_boot_state(state: Vec<u8>) {
    if let Ok(mut g) = CLEAN_BOOT_STATE.lock() {
        *g = Some(state);
    }
}

pub fn clean_boot_state() -> Option<Vec<u8>> {
    CLEAN_BOOT_STATE.lock().ok().and_then(|g| g.clone())
}

#[derive(Default, Clone, Copy)]
pub struct NetStepStats {
    pub advance_count: usize,
    pub save_count: usize,
    pub load_count: usize,
    pub save_state_micros: u64,
    pub checksum_micros: u64,
    pub load_state_micros: u64,
    pub peer_disconnected: bool,
    pub desync_detected: bool,
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
    /// Scratch buffer for the online-runahead speculative frame (reused).
    spec_scratch: Vec<u8>,
    /// Savestate buffer pool. ggrs holds a clone of each saved state in its
    /// ring (≤ prediction window + a couple); once it drops one, the Arc's
    /// refcount returns to 1 and the buffer is reused for a later save. In
    /// steady state this makes the per-frame save path allocation-free
    /// (previously: a fresh ~2.4 MB zeroed Vec every frame, ~130 MB/s churn).
    state_pool: Vec<std::sync::Arc<Vec<u8>>>,
}

/// Upper bound on pooled savestate buffers. ggrs's ring holds at most the
/// prediction window (8) plus bookkeeping copies; 16 gives comfortable slack.
const STATE_POOL_MAX: usize = 16;

impl NetRuntime {
    /// Serialize the core into a pooled buffer and return an Arc clone for
    /// ggrs to own. The buffer is mutated *while the pool is its only owner*
    /// (Arc::get_mut requires refcount 1), then cloned — so the pool keeps
    /// one reference for reuse and ggrs gets the other. Falls back to a
    /// transient allocation if the pool is saturated, which shouldn't happen
    /// with a correctly-sized pool but must not stall the frame if it does.
    /// Returns (state, serialize_ok).
    fn save_into_pooled(&mut self, core: &retro::Core) -> (std::sync::Arc<Vec<u8>>, bool) {
        use std::sync::Arc;
        let idx = self
            .state_pool
            .iter()
            .position(|a| Arc::strong_count(a) == 1)
            .or_else(|| {
                (self.state_pool.len() < STATE_POOL_MAX).then(|| {
                    self.state_pool.push(Arc::new(Vec::new()));
                    self.state_pool.len() - 1
                })
            });
        match idx {
            Some(i) => {
                let arc = &mut self.state_pool[i];
                let buf = Arc::get_mut(arc).expect("pool entry has refcount 1");
                let ok = core.save_state_into(buf);
                (Arc::clone(arc), ok)
            }
            None => {
                let mut buf = Vec::new();
                let ok = core.save_state_into(&mut buf);
                (Arc::new(buf), ok)
            }
        }
    }
}

/// Wipe trainer-flag RAM and drop all training-only state before a netplay
/// session starts. Both peers must run this so they begin from canonical
/// state — any non-zero training byte would desync GGRS on the first
/// checksum.
pub fn reset_for_netplay(
    core: &retro::Core,
    trainer: &mut memory::PokeList,
    reset_slots: &mut lab::ResetSlots,
    ghost_playback: &mut Option<ghost::Playback>,
    ghost_recording: &mut Option<ghost::Recording>,
) {
    use memory::{poke_u16, Endian};
    for addr in mk2_addrs::ZERO_TARGETS {
        poke_u16(core, *addr, 0x0000, Endian::Little);
    }
    trainer.set_enabled("hitboxes", false);
    trainer.set_enabled("p1_health", false);
    trainer.set_enabled("p2_health", false);
    trainer.set_enabled("freeze_timer", false);
    reset_slots.clear();
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
/// Seconds of clock skew we accept when hunting for the RTC word. The old
/// ±24h window matched too much (any counter-ish u32 near unix time); the
/// two peers' clocks are NTP-synced in practice, so ±10 minutes is plenty
/// and sharply reduces the chance of masking 4 bytes of real gameplay state.
const RTC_WINDOW_SECS: i64 = 10 * 60;
/// FBNeo serializes its device state (including the RTC) at the tail of the
/// blob; only scan the last chunk so a gameplay word can't be mistaken for it.
const RTC_SCAN_TAIL_BYTES: usize = 256 * 1024;

pub fn detect_rtc_offset(blob: &[u8]) -> Option<usize> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as u32;
    // Search from the END of the blob: FBNeo's RTC device is serialized late,
    // and starting at the back avoids matching random byte patterns elsewhere.
    let scan_start = blob.len().saturating_sub(RTC_SCAN_TAIL_BYTES);
    let mut best: Option<usize> = None;
    for off in (scan_start..blob.len().saturating_sub(4)).rev() {
        let v = u32::from_le_bytes([blob[off], blob[off + 1], blob[off + 2], blob[off + 3]]);
        let delta = (v as i64) - (now as i64);
        if delta.abs() < RTC_WINDOW_SECS {
            best = Some(off);
            break;
        }
    }
    best
}

/// All offsets whose 4-byte LE u32 looks like a wall clock (within ~1 day of
/// now). Diagnostic only: if more than one exists, the single-slot mask in
/// `detect_rtc_offset` may be leaving the *real* RTC in the cksum — the most
/// likely cause of a recurring frame-30 desync. Logged at session start so a
/// 2-PC capture shows whether both peers see the same candidate set.
pub fn detect_rtc_candidates(blob: &[u8]) -> Vec<usize> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let Ok(dur) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return Vec::new();
    };
    let now = dur.as_secs() as u32;
    let window: i64 = RTC_WINDOW_SECS;
    let mut offs = Vec::new();
    let mut i = 0usize;
    while i + 4 <= blob.len() {
        let v = u32::from_le_bytes([blob[i], blob[i + 1], blob[i + 2], blob[i + 3]]);
        if ((v as i64) - (now as i64)).abs() < window {
            offs.push(i);
            i += 4;
        } else {
            i += 1;
        }
    }
    offs
}

const HASH_LANE_INIT: [u64; 4] = [
    0x243f_6a88_85a3_08d3,
    0x1319_8a2e_0370_7344,
    0xa409_3822_299f_31d0,
    0x082e_fa98_ec4e_6c89,
];
const HASH_LANE_MUL: [u64; 4] = [
    0x9e37_79b1_85eb_ca87,
    0xc2b2_ae3d_27d4_eb4f,
    0x1656_67b1_9e37_79f9,
    0x85eb_ca77_c2b2_ae63,
];
const HASH_POS_MUL: u64 = 0xd6e8_feb8_6659_fd93;
const HASH_FINAL_A: u64 = 0xff51_afd7_ed55_8ccd;
const HASH_FINAL_B: u64 = 0xc4ce_b9fe_1a85_ec53;

/// Hash the savestate into a u128, optionally zeroing 4 bytes at `mask_off`
/// so the FBNeo wall-clock slot doesn't poison the hash.
pub fn cksum_with_mask(blob: &[u8], mask_off: Option<usize>) -> u128 {
    let mask = mask_off.and_then(|off| {
        let end = off.saturating_add(4).min(blob.len());
        (off < end).then_some((off, end))
    });
    let mut lanes = [
        HASH_LANE_INIT[0] ^ blob.len() as u64,
        HASH_LANE_INIT[1] ^ (blob.len() as u64).rotate_left(17),
        HASH_LANE_INIT[2] ^ (blob.len() as u64).rotate_left(31),
        HASH_LANE_INIT[3] ^ (blob.len() as u64).rotate_left(47),
    ];

    let mut block_index = 0usize;
    let mut chunks = blob.chunks_exact(32);
    for block in chunks.by_ref() {
        let start = block_index * 32;
        let words = if range_overlaps(start, start + 32, mask) {
            [
                masked_word_at(blob, start, mask),
                masked_word_at(blob, start + 8, mask),
                masked_word_at(blob, start + 16, mask),
                masked_word_at(blob, start + 24, mask),
            ]
        } else {
            [
                read_exact_word(&block[0..8]),
                read_exact_word(&block[8..16]),
                read_exact_word(&block[16..24]),
                read_exact_word(&block[24..32]),
            ]
        };
        mix_hash_block(&mut lanes, block_index, words);
        block_index += 1;
    }

    let remainder_start = block_index * 32;
    if !chunks.remainder().is_empty() {
        let words = [
            masked_word_at(blob, remainder_start, mask),
            masked_word_at(blob, remainder_start + 8, mask),
            masked_word_at(blob, remainder_start + 16, mask),
            masked_word_at(blob, remainder_start + 24, mask),
        ];
        mix_hash_block(&mut lanes, block_index, words);
    }

    let len = blob.len() as u64;
    let lo = avalanche64(
        lanes[0]
            ^ lanes[1].rotate_left(13)
            ^ lanes[2].rotate_left(29)
            ^ lanes[3].rotate_left(43)
            ^ len,
    );
    let hi = avalanche64(
        lanes[2]
            ^ lanes[3].rotate_left(19)
            ^ lanes[0].rotate_left(37)
            ^ lanes[1].rotate_left(51)
            ^ len.rotate_left(7),
    );
    (u128::from(hi) << 64) | u128::from(lo)
}

fn read_exact_word(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("hash word slices are 8 bytes"))
}

fn range_overlaps(start: usize, end: usize, mask: Option<(usize, usize)>) -> bool {
    mask.is_some_and(|(mask_start, mask_end)| start < mask_end && end > mask_start)
}

fn masked_word_at(blob: &[u8], start: usize, mask: Option<(usize, usize)>) -> u64 {
    let mut word = [0u8; 8];
    if start < blob.len() {
        let len = (blob.len() - start).min(8);
        word[..len].copy_from_slice(&blob[start..start + len]);
        if let Some((mask_start, mask_end)) = mask {
            let end = start + len;
            if start < mask_end && end > mask_start {
                let from = mask_start.saturating_sub(start).min(len);
                let to = mask_end.saturating_sub(start).min(len);
                for byte in &mut word[from..to] {
                    *byte = 0;
                }
            }
        }
    }
    u64::from_le_bytes(word)
}

fn mix_hash_block(lanes: &mut [u64; 4], block_index: usize, words: [u64; 4]) {
    let base_pos = (block_index as u64).wrapping_mul(4);
    for lane in 0..4 {
        let pos = base_pos.wrapping_add(lane as u64);
        let salted = words[lane]
            ^ pos
                .wrapping_mul(HASH_POS_MUL)
                .rotate_left((lane as u32 * 11) + 7);
        lanes[lane] ^= salted;
        lanes[lane] = lanes[lane]
            .rotate_left((lane as u32 * 9) + 13)
            .wrapping_mul(HASH_LANE_MUL[lane])
            .wrapping_add(HASH_LANE_INIT[3 - lane] ^ pos.rotate_left(23));
    }
}

fn avalanche64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(HASH_FINAL_A);
    x ^= x >> 33;
    x = x.wrapping_mul(HASH_FINAL_B);
    x ^ (x >> 33)
}

fn duration_micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

pub fn step_netplay_frame(
    core: &retro::Core,
    sess: &mut netplay::Session,
    speculate: bool,
    local_handle: usize,
    net_recording: &mut Option<ghost::NetRecording>,
    replay_recording: &mut Option<match_replay::Recording>,
    net_log: &mut Option<std::fs::File>,
    runtime: &mut NetRuntime,
) -> NetStepStats {
    use ggrs::{GgrsRequest, PlayerHandle, SessionState};
    use input::{apply_snapshot, snapshot_player, Player};
    use netplay::NetInput;
    use std::io::Write;

    let mut peer_disconnected_this_frame = false;
    let mut desync_detected_this_frame = false;

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
                desync_detected_this_frame = true;
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
    let mut save_state_time = Duration::ZERO;
    let mut checksum_time = Duration::ZERO;
    let mut load_state_time = Duration::ZERO;
    let mut sim_frame = sess
        .current_frame()
        .saturating_sub(advance_count.min(i32::MAX as usize) as i32);
    let confirmed_frame = sess.confirmed_frame();

    for req in requests {
        match req {
            GgrsRequest::SaveGameState { cell, frame } => {
                let save_started = Instant::now();
                let (blob, save_ok) = runtime.save_into_pooled(core);
                save_state_time += save_started.elapsed();
                if !save_ok {
                    dlog!("net", "SaveGameState frame={frame} serialize FAILED");
                    if let Some(f) = net_log.as_mut() {
                        let _ = writeln!(f, "[net/err] serialize failed at frame={frame}");
                    }
                }

                // The checksum only exists for ggrs desync detection, which
                // compares once every DESYNC_INTERVAL frames — computing it on
                // the other frames is pure waste, so gate it to the frames
                // ggrs will actually look at.
                let checksum_frame = frame >= 0 && (frame as u32) % netplay::DESYNC_INTERVAL == 0;
                let cksum = if checksum_frame {
                    let checksum_started = Instant::now();
                    if runtime.rtc_mask_offset.is_none() {
                        if let Some(off) = detect_rtc_offset(&blob) {
                            runtime.rtc_mask_offset = Some(off);
                            // Log every wall-clock-like slot, not just the masked one.
                            // If a 2-PC capture shows >1 candidate (or differing sets
                            // between peers), the single-slot mask is the desync cause.
                            let candidates = detect_rtc_candidates(&blob);
                            let cand_str: Vec<String> =
                                candidates.iter().map(|o| format!("0x{o:x}")).collect();
                            let m = format!(
                                "[net] FBNeo RTC slot at savestate offset 0x{off:x} — masking from cksum (candidates: [{}])",
                                cand_str.join(", ")
                            );
                            println!("{m}");
                            if let Some(f) = net_log.as_mut() {
                                let _ = writeln!(f, "{m}");
                            }
                        }
                    }
                    // Desync checksum is computed over the game's SYSTEM_RAM (the
                    // 68000 work RAM that drives gameplay), NOT the full emulator
                    // savestate. The savestate also serializes the sound CPU RAM,
                    // the audio chips (YM2151/OKI), and the host RTC — all of which
                    // differ between two peers even when gameplay is in perfect
                    // lockstep. Hashing the whole blob made every such difference a
                    // "desync"; a 2-PC capture showed only ~0.1% of the savestate
                    // differing, none of it in gameplay RAM. If SYSTEM_RAM is
                    // unavailable for some reason, fall back to the old whole-blob
                    // hash with the RTC slot masked.
                    let mut cksum = match memory::snapshot(core) {
                        Some(ram) => cksum_with_mask(&ram, None),
                        None => cksum_with_mask(&blob, runtime.rtc_mask_offset),
                    };
                    if let Some(sync) = memory::peek_u16(
                        core,
                        mk2_addrs::NETPLAY_SYNC_ADDR,
                        memory::Endian::Little,
                    ) {
                        cksum ^= u128::from(sync) << 112;
                    }
                    checksum_time += checksum_started.elapsed();
                    Some(cksum)
                } else {
                    None
                };
                dlog!(
                    "net",
                    "SaveGameState frame={frame} bytes={} cksum={}",
                    blob.len(),
                    cksum
                        .map(|c| format!("0x{c:032x}"))
                        .unwrap_or_else(|| "-".into())
                );
                // Diagnostic pre-sync dump: a synchronous multi-MB write mid-
                // match, so it is opt-in (set FREEPLAY_DUMP_PRESYNC=1) rather
                // than firing on frame 30 of every session as it used to.
                const DUMP_FRAME: i32 = 30;
                if frame == DUMP_FRAME
                    && !runtime.desync_dumped
                    && crate::config::env_value("FREEPLAY_DUMP_PRESYNC").is_some()
                {
                    runtime.desync_dumped = true;
                    let tag = if local_handle == 0 { "host" } else { "join" };
                    let path = format!("presync_{}_frame{}.state", tag, frame);
                    let _ = std::fs::write(&path, blob.as_slice());
                    let m = format!(
                        "[net] dumped pre-sync savestate ({} bytes) to {}",
                        blob.len(),
                        path
                    );
                    println!("{m}");
                    if let Some(f) = net_log.as_mut() {
                        let _ = writeln!(f, "{m}");
                    }
                }
                cell.save(frame, Some(blob), cksum);
            }
            GgrsRequest::LoadGameState { cell, frame } => {
                if let Some(blob) = cell.load() {
                    let load_started = Instant::now();
                    let ok = core.load_state(&blob);
                    load_state_time += load_started.elapsed();
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
                retro::set_silent(!is_last);
                let p1_bits = inputs[0].0.bits;
                let p2_bits = inputs[1].0.bits;
                dlog!(
                    "net",
                    "AdvanceFrame frame={} #{}/{} p1=0x{:04x} p2=0x{:04x} silent={}",
                    sim_frame,
                    advances_seen,
                    advance_count,
                    p1_bits,
                    p2_bits,
                    !is_last
                );
                apply_snapshot(Player::P1, p1_bits);
                apply_snapshot(Player::P2, p2_bits);
                if let Some(rec) = replay_recording.as_mut() {
                    rec.record_frame(core, sim_frame, p1_bits, p2_bits);
                }
                if is_last {
                    if let Some(rec) = net_recording.as_mut() {
                        rec.record_confirmed_frame(core, p1_bits, p2_bits);
                    }
                }
                unsafe {
                    (core.run)();
                }
                sim_frame = sim_frame.saturating_add(1);
            }
        }
    }
    if let Some(rec) = replay_recording.as_mut() {
        rec.set_confirmed_frame(confirmed_frame);
    }
    retro::set_silent(false);

    // Online video-only runahead: show one frame beyond the ggrs frontier.
    // Runs the core once more from canonical state with the freshest LOCAL
    // input (the remote port keeps its last ggrs-applied input, i.e. the
    // same prediction ggrs itself would make), presents only its VIDEO,
    // then restores canonical. It never enters ggrs state or the desync
    // checksum path, so it cannot cause desyncs — it only deepens the
    // displayed prediction by one frame. Audio stays canonical: presenting
    // speculative audio would enqueue duplicate samples every tick.
    if speculate && advance_count > 0 && !peer_disconnected_this_frame {
        if core.save_state_into(&mut runtime.spec_scratch) {
            apply_snapshot(local_player, snapshot_player(local_player));
            retro::set_av_mode(retro::AvMode::VideoOnly);
            unsafe { (core.run)() };
            retro::set_av_mode(retro::AvMode::Normal);
            if !core.load_state(&runtime.spec_scratch) {
                dlog!("net", "speculative frame: canonical restore FAILED");
                if let Some(f) = net_log.as_mut() {
                    let _ = writeln!(f, "[net/err] speculative restore failed");
                }
            }
        }
    }

    NetStepStats {
        advance_count,
        save_count,
        load_count,
        save_state_micros: duration_micros(save_state_time),
        checksum_micros: duration_micros(checksum_time),
        load_state_micros: duration_micros(load_state_time),
        peer_disconnected: peer_disconnected_this_frame,
        desync_detected: desync_detected_this_frame,
    }
}

#[cfg(test)]
mod tests {
    use super::cksum_with_mask;
    use std::hint::black_box;
    use std::time::Instant;

    #[test]
    fn checksum_mask_ignores_rtc_word() {
        let mut a = vec![0x42; 48];
        let mut b = a.clone();
        a[19..23].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        b[19..23].copy_from_slice(&0x90ab_cdefu32.to_le_bytes());

        assert_ne!(cksum_with_mask(&a, None), cksum_with_mask(&b, None));
        assert_eq!(cksum_with_mask(&a, Some(19)), cksum_with_mask(&b, Some(19)));
    }

    #[test]
    fn checksum_detects_paired_word_changes() {
        let a = vec![0u8; 64];
        let mut b = a.clone();
        b[0] = 1;
        b[16] = 1;

        assert_ne!(cksum_with_mask(&a, None), cksum_with_mask(&b, None));
    }

    #[test]
    fn checksum_mask_ignores_rtc_word_across_word_boundary() {
        let mut a = vec![0x24; 64];
        let mut b = a.clone();
        a[6..10].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        b[6..10].copy_from_slice(&0x90ab_cdefu32.to_le_bytes());

        assert_ne!(cksum_with_mask(&a, None), cksum_with_mask(&b, None));
        assert_eq!(cksum_with_mask(&a, Some(6)), cksum_with_mask(&b, Some(6)));
    }

    #[test]
    fn checksum_detects_same_lane_paired_word_changes() {
        let a = vec![0u8; 96];
        let mut b = a.clone();
        b[0] = 1;
        b[32] = 1;

        assert_ne!(cksum_with_mask(&a, None), cksum_with_mask(&b, None));
    }

    #[test]
    fn checksum_distinguishes_padded_lengths() {
        assert_ne!(cksum_with_mask(&[0], None), cksum_with_mask(&[0, 0], None));
        assert_ne!(
            cksum_with_mask(&[0; 31], None),
            cksum_with_mask(&[0; 32], None)
        );
    }

    #[test]
    #[ignore]
    fn checksum_large_blob_smoke_timing() {
        let mut blob = vec![0u8; 2_447_284];
        for (i, byte) in blob.iter_mut().enumerate() {
            *byte = (i as u8).wrapping_mul(37).wrapping_add((i >> 8) as u8);
        }

        let started = Instant::now();
        let mut sum = 0u128;
        const ITERS: u32 = 201;
        for _ in 0..ITERS {
            sum ^= black_box(cksum_with_mask(black_box(&blob), Some(2_100_003)));
        }
        let elapsed = started.elapsed();
        eprintln!(
            "checksum_large_blob_smoke_timing: {:?} total, {:.1} us/hash, acc=0x{sum:032x}",
            elapsed,
            elapsed.as_secs_f64() * 1_000_000.0 / f64::from(ITERS)
        );
        assert_ne!(sum, 0);
    }
}
