//! Dynamic rate control between the core's audio and the SDL queue.
//!
//! The core and the sound device run on two clocks that never agree
//! exactly. MK2's core emits a fixed sample count per emulated frame, the
//! frame loop is paced by the host timer, and the device drains at its own
//! crystal's idea of 48 kHz. Any mismatch - a fraction of a percent of drift,
//! a GGRS `WaitRecommendation` skip that produces no audio that tick, a
//! stalled window drag - moves the queue, and nothing used to move it back.
//! A queue that sits near empty is where the chop comes from: SDL pads every
//! callback it cannot fill with silence.
//!
//! The pacer keeps the queue at the configured target by resampling each
//! frame's audio by at most `MAX_DEVIATION` (0.5%, below what anyone hears as
//! pitch), stretching when the queue is short and compressing when it is
//! long. Skipped frames then cost cushion instead of an audible gap, and the
//! cushion refills on its own.
//!
//! Two hard limits stay for what rate control cannot absorb:
//! - an empty queue (a real underrun) is refilled with silence to half the
//!   target, so one gap is followed by clean audio instead of a crackle every
//!   callback while the queue crawls back;
//! - a queue above target + 100 ms still drops the frame, as before.

use std::time::{Duration, Instant};

/// Largest resample deviation, as a fraction. RetroArch's default.
const MAX_DEVIATION: f64 = 0.005;
/// Stereo s16 in SDL's queue.
const BYTES_PER_STEREO_FRAME: u32 = 4;
/// Headroom above target before a frame is dropped outright.
const OVERFLOW_HEADROOM_MS: u32 = 100;
const STATS_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct PacerStats {
    pub calls: u64,
    pub in_frames: u64,
    pub out_frames: u64,
    pub underruns: u64,
    pub drops: u64,
    pub min_fill_ms: u32,
    pub max_fill_ms: u32,
}

pub struct AudioPacer {
    enabled: bool,
    /// Read position into [prev, input...], carried between calls so the
    /// interpolation has no seam at frame boundaries.
    t: f64,
    prev: (i16, i16),
    primed: bool,
    out: Vec<i16>,
    stats: PacerStats,
    stats_since: Instant,
}

impl Default for AudioPacer {
    fn default() -> Self {
        // FREEPLAY_AUDIO_DRC=0 restores the old pass-through path, so the same
        // binary can A/B the two.
        let enabled = std::env::var("FREEPLAY_AUDIO_DRC").map_or(true, |v| v != "0");
        Self::new(enabled)
    }
}

impl AudioPacer {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            t: 0.0,
            prev: (0, 0),
            primed: false,
            out: Vec::new(),
            stats: PacerStats {
                min_fill_ms: u32::MAX,
                ..PacerStats::default()
            },
            stats_since: Instant::now(),
        }
    }

    /// Forget the stream, e.g. after the queue was cleared. The next call
    /// primes the cushion again.
    pub fn reset(&mut self) {
        self.t = 0.0;
        self.prev = (0, 0);
        self.primed = false;
    }

    /// Input frames consumed per output frame for a queue at `fill_ms`.
    /// Below target: < 1 (stretch). Above: > 1 (compress).
    fn step_for(fill_ms: u32, target_ms: u32) -> f64 {
        let target = target_ms.max(1) as f64;
        let err = (fill_ms as f64 - target) / target;
        1.0 + MAX_DEVIATION * err.clamp(-1.0, 1.0)
    }

    /// Resample interleaved stereo `input` at `step`, appending to `self.out`.
    fn resample(&mut self, input: &[i16], step: f64) {
        let n = input.len() / 2;
        if n == 0 {
            return;
        }
        let at = |i: usize, prev: (i16, i16)| -> (i16, i16) {
            if i == 0 {
                prev
            } else {
                (input[(i - 1) * 2], input[(i - 1) * 2 + 1])
            }
        };
        let mut t = self.t;
        while t < n as f64 {
            let i = t as usize;
            let frac = t - i as f64;
            let (al, ar) = at(i, self.prev);
            let (bl, br) = at(i + 1, self.prev);
            let l = al as f64 + (bl as f64 - al as f64) * frac;
            let r = ar as f64 + (br as f64 - ar as f64) * frac;
            self.out.push(l.round() as i16);
            self.out.push(r.round() as i16);
            t += step;
        }
        self.t = t - n as f64;
        self.prev = (input[(n - 1) * 2], input[(n - 1) * 2 + 1]);
    }

    /// Decide what to queue for this frame's `input`, given the queue's
    /// current size. Returns the samples to queue (possibly empty).
    pub fn process(&mut self, input: &[i16], queued_bytes: u32, freq: u32, target_ms: u32) -> &[i16] {
        let per_ms = (freq.max(1) * BYTES_PER_STEREO_FRAME) as f64 / 1000.0;
        let fill_ms = (queued_bytes as f64 / per_ms) as u32;
        self.stats.calls += 1;
        self.stats.in_frames += (input.len() / 2) as u64;
        self.stats.min_fill_ms = self.stats.min_fill_ms.min(fill_ms);
        self.stats.max_fill_ms = self.stats.max_fill_ms.max(fill_ms);
        self.out.clear();

        if !self.enabled {
            if fill_ms >= target_ms + OVERFLOW_HEADROOM_MS {
                self.stats.drops += 1;
                return &self.out;
            }
            if queued_bytes == 0 && self.primed {
                self.stats.underruns += 1;
            }
            self.primed = true;
            self.out.extend_from_slice(input);
            self.stats.out_frames += (input.len() / 2) as u64;
            return &self.out;
        }

        if fill_ms >= target_ms + OVERFLOW_HEADROOM_MS {
            self.stats.drops += 1;
            // The stream jumps here, so do not interpolate across it.
            self.reset();
            self.primed = true;
            return &self.out;
        }

        if queued_bytes == 0 {
            if self.primed {
                self.stats.underruns += 1;
            }
            // Rebuild a cushion so the next skipped tick is absorbed.
            let frames = (freq as u64 * (target_ms / 2) as u64 / 1000) as usize;
            self.out.resize(frames * 2, 0);
            self.primed = true;
            let step = Self::step_for(target_ms / 2, target_ms);
            self.resample(input, step);
        } else {
            let step = Self::step_for(fill_ms, target_ms);
            self.resample(input, step);
        }
        self.stats.out_frames += (self.out.len() / 2) as u64;
        &self.out
    }

    /// Stats since the last report, when one is due.
    pub fn take_report(&mut self) -> Option<(PacerStats, Duration)> {
        let elapsed = self.stats_since.elapsed();
        if elapsed < STATS_INTERVAL {
            return None;
        }
        let s = self.stats;
        self.stats = PacerStats {
            min_fill_ms: u32::MAX,
            ..PacerStats::default()
        };
        self.stats_since = Instant::now();
        Some((s, elapsed))
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREQ: u32 = 48_000;
    const TARGET: u32 = 120;

    fn bytes_for_ms(ms: u32) -> u32 {
        FREQ * BYTES_PER_STEREO_FRAME * ms / 1000
    }

    fn tone(frames: usize, phase: &mut f64) -> Vec<i16> {
        let mut v = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            let s = (phase.sin() * 8000.0) as i16;
            v.push(s);
            v.push(s);
            *phase += 0.05;
        }
        v
    }

    #[test]
    fn at_target_the_rate_is_unchanged() {
        let mut p = AudioPacer::new(true);
        p.primed = true;
        let mut phase = 0.0;
        let mut out = 0usize;
        for _ in 0..1000 {
            let input = tone(877, &mut phase);
            out += p.process(&input, bytes_for_ms(TARGET), FREQ, TARGET).len() / 2;
        }
        assert_eq!(out, 877 * 1000);
    }

    #[test]
    fn short_queue_stretches_and_long_queue_compresses_within_half_a_percent() {
        let mut phase = 0.0;
        let mut short = AudioPacer::new(true);
        short.primed = true;
        let mut long = AudioPacer::new(true);
        long.primed = true;
        let (mut s_out, mut l_out) = (0usize, 0usize);
        for _ in 0..1000 {
            let input = tone(877, &mut phase);
            s_out += short.process(&input, bytes_for_ms(TARGET / 2), FREQ, TARGET).len() / 2;
            l_out += long.process(&input, bytes_for_ms(TARGET + 60), FREQ, TARGET).len() / 2;
        }
        let base = 877.0 * 1000.0;
        assert!(s_out as f64 > base && (s_out as f64) < base * 1.005 + 2.0, "{s_out}");
        assert!((l_out as f64) < base && (l_out as f64) > base * 0.995 - 2.0, "{l_out}");
    }

    #[test]
    fn frame_boundaries_leave_no_seam() {
        // A slow ramp resampled across many small frames must stay monotonic:
        // a seam would show up as a step back or a jump.
        let mut p = AudioPacer::new(true);
        p.primed = true;
        let mut all = Vec::new();
        let mut v = 0i16;
        for _ in 0..200 {
            let mut input = Vec::new();
            for _ in 0..37 {
                input.push(v);
                input.push(v);
                v = v.wrapping_add(3);
            }
            if v > 20_000 {
                break;
            }
            all.extend_from_slice(p.process(&input, bytes_for_ms(TARGET / 3), FREQ, TARGET));
        }
        for w in all.chunks(2).collect::<Vec<_>>().windows(2) {
            let d = w[1][0] as i32 - w[0][0] as i32;
            assert!((0..=4).contains(&d), "seam: {} -> {}", w[0][0], w[1][0]);
        }
    }

    #[test]
    fn empty_queue_is_counted_and_refilled_to_half_target() {
        let mut p = AudioPacer::new(true);
        let input = vec![100i16; 877 * 2];
        // First call on an empty queue is the start, not an underrun.
        let first = p.process(&input, 0, FREQ, TARGET).len() / 2;
        assert!(first >= (FREQ * (TARGET / 2) / 1000) as usize);
        assert_eq!(p.stats.underruns, 0);
        p.process(&input, 0, FREQ, TARGET);
        assert_eq!(p.stats.underruns, 1);
    }

    #[test]
    fn overflow_still_drops() {
        let mut p = AudioPacer::new(true);
        p.primed = true;
        let input = vec![1i16; 877 * 2];
        let n = p.process(&input, bytes_for_ms(TARGET + OVERFLOW_HEADROOM_MS), FREQ, TARGET).len();
        assert_eq!(n, 0);
        assert_eq!(p.stats.drops, 1);
    }

    #[test]
    fn disabled_is_pass_through() {
        let mut p = AudioPacer::new(false);
        let input: Vec<i16> = (0..1754).map(|i| i as i16).collect();
        let out = p.process(&input, bytes_for_ms(10), FREQ, TARGET).to_vec();
        assert_eq!(out, input);
    }
}
