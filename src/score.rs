//! MK2 round/match score tracking from FBNeo SYSTEM_RAM.
//!
//! Symbols (verified by anchoring on `gstate=0x253B2` and walking RAM.ASM):
//!   p1_matchw  = 0x253DA  (P1 wins this match, u16)
//!   p2_matchw  = 0x25554  (P2 wins this match, u16)
//!   round_num  = 0x256D6  (current round, u16)
//!   winner_status = 0x256D8  (1=P1, 2=P2, 3=finish him, u16)

use crate::memory::{peek_u16, Endian};
use crate::retro::Core;

const P1_MATCHW: usize = 0x253DA;
const P2_MATCHW: usize = 0x25554;
const ROUND_NUM: usize = 0x256D6;
const WINNER_STATUS: usize = 0x256D8;

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Score {
    pub p1_match_wins: u16,
    pub p2_match_wins: u16,
    pub round_num: u16,
    pub winner_status: u16,
}

impl Score {
    pub fn read(core: &Core) -> Score {
        Score {
            p1_match_wins: peek_u16(core, P1_MATCHW, Endian::Little).unwrap_or(0),
            p2_match_wins: peek_u16(core, P2_MATCHW, Endian::Little).unwrap_or(0),
            round_num: peek_u16(core, ROUND_NUM, Endian::Little).unwrap_or(0),
            winner_status: peek_u16(core, WINNER_STATUS, Endian::Little).unwrap_or(0),
        }
    }
}

/// Frame-to-frame change tracker: turns RAM polls into discrete events.
#[derive(Default, Clone, Debug)]
pub struct ScoreTracker {
    last: Option<Score>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreEvent {
    /// `winner` is 1 for P1, 2 for P2. Carries the new running score.
    RoundWon {
        winner: u8,
        p1_wins: u16,
        p2_wins: u16,
    },
    /// A match (best-of-N rounds) completed. `winner` 1 or 2.
    MatchOver {
        winner: u8,
        p1_wins: u16,
        p2_wins: u16,
    },
    /// Counters wrapped back to zero — a fresh match has begun.
    NewMatch,
}

impl ScoreTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compare the new RAM read against last frame's; emit any state-change events.
    pub fn step(&mut self, now: Score) -> Vec<ScoreEvent> {
        let mut out = Vec::new();
        let prev = match self.last {
            Some(p) => p,
            None => {
                self.last = Some(now);
                return out;
            }
        };

        // Round won: a match-win counter ticked up by exactly 1.
        if now.p1_match_wins == prev.p1_match_wins + 1 {
            out.push(ScoreEvent::RoundWon {
                winner: 1,
                p1_wins: now.p1_match_wins,
                p2_wins: now.p2_match_wins,
            });
        }
        if now.p2_match_wins == prev.p2_match_wins + 1 {
            out.push(ScoreEvent::RoundWon {
                winner: 2,
                p1_wins: now.p1_match_wins,
                p2_wins: now.p2_match_wins,
            });
        }

        // MK2 is best-of-3 rounds. When either side hits 2, the match is decided.
        // We emit MatchOver exactly once on the transition to 2.
        let was_decided = prev.p1_match_wins >= 2 || prev.p2_match_wins >= 2;
        let is_decided = now.p1_match_wins >= 2 || now.p2_match_wins >= 2;
        if !was_decided && is_decided {
            let winner = if now.p1_match_wins >= 2 { 1 } else { 2 };
            out.push(ScoreEvent::MatchOver {
                winner,
                p1_wins: now.p1_match_wins,
                p2_wins: now.p2_match_wins,
            });
        }

        // Both counters dropped to zero from non-zero -> new match starting.
        let was_nonzero = prev.p1_match_wins != 0 || prev.p2_match_wins != 0;
        let is_zero = now.p1_match_wins == 0 && now.p2_match_wins == 0;
        if was_nonzero && is_zero {
            out.push(ScoreEvent::NewMatch);
        }

        self.last = Some(now);
        out
    }

    pub fn reset(&mut self) {
        self.last = None;
    }
}
