//! Per-game score tracking from FBNeo SYSTEM_RAM.
//!
//! Every game reduces to the same two numbers per side and the same three
//! events, so the HUD, the Discord webhook, set tracking and the result
//! report to the server stay game-blind:
//!
//! - **MK2**: the counters are round wins in the current match (best of 3).
//!   Addresses come from `mk2_addrs`, generated from mk2-main's `mk2.map`.
//! - **NBA Jam**: the counters are team points; P1 is team 1 (P1+P2 seats)
//!   and P2 is team 2 (P3+P4), which is where `retro.rs` seats the second
//!   player. Addresses are per ROM revision — see `nba_jam_addrs`.
//! - **NBA Hangtime**: the same as NBA Jam, with its own addresses
//!   (`nba_hangtime_addrs`); the second player is P2 on its 2-player cabinet.

use crate::game::{self, GameId};
use crate::memory::{peek_u16, peek_u8, Endian};
use crate::mk2_addrs;
use crate::retro::Core;

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Score {
    /// Side 1's counter: MK2 round wins, NBA Jam team-1 points.
    pub p1_match_wins: u16,
    /// Side 2's counter: MK2 round wins, NBA Jam team-2 points.
    pub p2_match_wins: u16,
    /// MK2's round number. Always 0 on the basketball games.
    pub round_num: u16,
    /// MK2's winner status. Always 0 on the basketball games.
    pub winner_status: u16,
    /// The basketball games' quarter clock as its three digits packed (m, s10, s1 ->
    /// 0x0m_s10_s1-ish); only "zero or not" is meaningful. Always 0 on MK2.
    pub clock: u32,
}

/// Where NBA Jam keeps its score and quarter clock, as byte offsets into
/// FBNeo's SYSTEM_RAM.
///
/// Source: Pugsy's MAME cheat collection (`nbajam*.xml`), whose addresses
/// are TMS34010 bit addresses: `maincpu.pb@10F10C0` is team 1's score.
/// Offset = bit / 8 - 0x200000, the same conversion `mk2_addrs` uses. The
/// clock is three single-digit bytes, minutes then tens then ones; the
/// cheat that ends a quarter zeroes all three, so 0:00 is the period end.
///
/// Only revisions the cheat file covers get an entry. The prototypes and
/// the hacks have no known layout, and reading the wrong one would invent
/// scores, so they play without scoring rather than with bad scoring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NbaJamAddrs {
    pub team1_score: usize,
    pub team2_score: usize,
    pub clock: [usize; 3],
}

const fn ram(bit: usize) -> usize {
    bit / 8 - mk2_addrs::FBNEO_SYSTEM_RAM_BASE
}

pub fn nba_jam_addrs(driver: &str) -> Option<NbaJamAddrs> {
    let (t1, t2, clock) = match driver {
        // rev 3.01 (04/07/93)
        "nbajam" => (0x10F10C0, 0x10F10D0, [0x10F0F58, 0x10F0F50, 0x10F0F48]),
        // rev 2.00 (02/10/93)
        "nbajamr2" => (0x10F1060, 0x10F1070, [0x10F0EF8, 0x10F0EF0, 0x10F0EE8]),
        // Tournament Edition, every revision the cheat file lists
        "nbajamte" | "nbajamte1" | "nbajamte2" | "nbajamte2a" | "nbajamte3" | "nbajamte3a"
        | "nbajamte4" => (0x10EAFD0, 0x10EAFE0, [0x10EAE68, 0x10EAE60, 0x10EAE58]),
        _ => return None,
    };
    Some(NbaJamAddrs {
        team1_score: ram(t1),
        team2_score: ram(t2),
        clock: [ram(clock[0]), ram(clock[1]), ram(clock[2])],
    })
}

/// NBA Hangtime's score and quarter clock, in the same form as NBA Jam's.
///
/// Source: Pugsy's cheat collection again (`nbahangt*.xml`, `nbamht*.xml`).
/// Every Hangtime revision shares one file, and every Maximum Hangtime
/// revision another.
pub fn nba_hangtime_addrs(driver: &str) -> Option<NbaJamAddrs> {
    let (t1, t2, clock) = match driver {
        "nbahangt" | "nbahangtm13" | "nbahangtl12" | "nbahangtm12" | "nbahangtl11"
        | "nbahangtm11" => (0x1137B40, 0x1137B50, [0x1137A18, 0x1137A10, 0x1137A08]),
        "nbamht" | "nbamhtm10" | "nbamhtl10" | "nbamhtp" => {
            (0x1137BE0, 0x1137BF0, [0x1137AB8, 0x1137AB0, 0x1137AA8])
        }
        _ => return None,
    };
    Some(NbaJamAddrs {
        team1_score: ram(t1),
        team2_score: ram(t2),
        clock: [ram(clock[0]), ram(clock[1]), ram(clock[2])],
    })
}

/// The running basketball game's addresses, if it is one we can score.
fn hoops_addrs() -> Option<NbaJamAddrs> {
    let driver = game::loaded_driver();
    match game::current().id {
        GameId::Mk2 => None,
        GameId::NbaJam => nba_jam_addrs(&driver),
        GameId::NbaHangtime => nba_hangtime_addrs(&driver),
    }
}

/// Can this session produce results at all? False for a basketball revision
/// with no known RAM layout: it still plays online, it just reports nothing.
pub fn supported() -> bool {
    game::current().id == GameId::Mk2 || hoops_addrs().is_some()
}

impl Score {
    pub fn read(core: &Core) -> Score {
        match game::current().id {
            GameId::Mk2 => Score {
                p1_match_wins: peek_u16(core, mk2_addrs::a().P1_MATCHW, Endian::Little)
                    .unwrap_or(0),
                p2_match_wins: peek_u16(core, mk2_addrs::a().P2_MATCHW, Endian::Little)
                    .unwrap_or(0),
                round_num: peek_u16(core, mk2_addrs::a().ROUND_NUM, Endian::Little).unwrap_or(0),
                winner_status: peek_u16(core, mk2_addrs::a().WINNER_STATUS, Endian::Little)
                    .unwrap_or(0),
                clock: 0,
            },
            GameId::NbaJam | GameId::NbaHangtime => match hoops_addrs() {
                Some(a) => {
                    let byte = |addr| u32::from(peek_u8(core, addr).unwrap_or(0));
                    Score {
                        p1_match_wins: u16::from(peek_u8(core, a.team1_score).unwrap_or(0)),
                        p2_match_wins: u16::from(peek_u8(core, a.team2_score).unwrap_or(0)),
                        clock: byte(a.clock[0]) << 16 | byte(a.clock[1]) << 8 | byte(a.clock[2]),
                        ..Score::default()
                    }
                }
                None => Score::default(),
            },
        }
    }
}

/// Frame-to-frame change tracker: turns RAM polls into discrete events.
#[derive(Default, Clone, Debug)]
pub struct ScoreTracker {
    last: Option<Score>,
    /// NBA Jam: quarter (and overtime) clocks that have run out this game.
    periods_ended: u8,
    /// NBA Jam: MatchOver already sent for this game.
    decided: bool,
    /// NBA Jam: frames the clock has sat at 0:00 since a period ended, while
    /// a shot released before the buzzer can still land.
    buzzer_frames: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreEvent {
    /// MK2 only. `winner` is 1 for P1, 2 for P2. Carries the new running score.
    RoundWon {
        winner: u8,
        p1_wins: u16,
        p2_wins: u16,
    },
    /// A match (MK2 best-of-3, NBA Jam full game) completed. `winner` 1 or
    /// 2; the counters are the final score (round wins / points).
    MatchOver {
        winner: u8,
        p1_wins: u16,
        p2_wins: u16,
    },
    /// Counters wrapped back to zero — a fresh match has begun.
    NewMatch,
}

/// Regulation length; anything after is overtime, decided at the first
/// period end with the score untied.
const NBA_JAM_QUARTERS: u8 = 4;

/// How long after 0:00 a period's score is final. A shot released before the
/// buzzer counts when it lands: on Hangtime one scored 49 frames after the
/// clock ran out (`--score-probe`). Two seconds covers any shot's flight and
/// is still long before the next tip-off.
const BUZZER_SETTLE_FRAMES: u16 = 110;

impl ScoreTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compare the new RAM read against last frame's; emit any state-change events.
    pub fn step(&mut self, now: Score) -> Vec<ScoreEvent> {
        match game::current().id {
            GameId::Mk2 => self.step_mk2(now),
            GameId::NbaJam | GameId::NbaHangtime => self.step_nba_jam(now),
        }
    }

    fn step_mk2(&mut self, now: Score) -> Vec<ScoreEvent> {
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

    /// NBA Jam has no "game over" byte we know of, so the end is derived:
    /// count clock expiries (running -> 0:00) since the scores were last
    /// reset, and the game is over at the first expiry from the fourth on
    /// where the score isn't tied — a tie means overtime, and the next
    /// expiry decides it. Boot RAM reads 0:00 too, which is why only a
    /// running clock reaching zero counts.
    fn step_nba_jam(&mut self, now: Score) -> Vec<ScoreEvent> {
        let mut out = Vec::new();
        let prev = match self.last {
            Some(p) => p,
            None => {
                self.last = Some(now);
                return out;
            }
        };

        let was_nonzero = prev.p1_match_wins != 0 || prev.p2_match_wins != 0;
        let is_zero = now.p1_match_wins == 0 && now.p2_match_wins == 0;
        if was_nonzero && is_zero {
            self.periods_ended = 0;
            self.decided = false;
            out.push(ScoreEvent::NewMatch);
        }

        // Tip-off: the clock starts with nobody on the board. Clears periods
        // counted by anything that never scored (an attract-mode demo), which
        // the score-reset rule above can't see.
        if prev.clock == 0 && now.clock != 0 && is_zero {
            self.periods_ended = 0;
            self.decided = false;
        }

        if prev.clock != 0 && now.clock == 0 {
            self.periods_ended = self.periods_ended.saturating_add(1);
            self.buzzer_frames = Some(0);
        }
        if let Some(n) = self.buzzer_frames {
            // The clock restarting early (a new period, a reset) settles it
            // on the last score seen at 0:00.
            let settled = if now.clock != 0 {
                Some(prev)
            } else if n + 1 >= BUZZER_SETTLE_FRAMES {
                Some(now)
            } else {
                None
            };
            match settled {
                Some(end) => {
                    self.buzzer_frames = None;
                    crate::dlog!(
                        "net",
                        "[score] basketball period {} ended {}-{}",
                        self.periods_ended,
                        end.p1_match_wins,
                        end.p2_match_wins
                    );
                    if !self.decided
                        && self.periods_ended >= NBA_JAM_QUARTERS
                        && end.p1_match_wins != end.p2_match_wins
                    {
                        self.decided = true;
                        out.push(ScoreEvent::MatchOver {
                            winner: if end.p1_match_wins > end.p2_match_wins {
                                1
                            } else {
                                2
                            },
                            p1_wins: end.p1_match_wins,
                            p2_wins: end.p2_match_wins,
                        });
                    }
                }
                None => self.buzzer_frames = Some(n + 1),
            }
        }

        self.last = Some(now);
        out
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(p1: u16, p2: u16, clock: u32) -> Score {
        Score {
            p1_match_wins: p1,
            p2_match_wins: p2,
            clock,
            ..Score::default()
        }
    }

    /// Run a game: each quarter the clock starts, scores move, the clock
    /// runs out. Returns every event seen.
    fn play(t: &mut ScoreTracker, quarters: &[(u16, u16)]) -> Vec<ScoreEvent> {
        let mut out = Vec::new();
        for &(p1, p2) in quarters {
            out.extend(t.step_nba_jam(s(p1, p2, 0x030000)));
            out.extend(t.step_nba_jam(s(p1, p2, 0x000001)));
            // Between quarters the clock sits at 0:00 for a while.
            for _ in 0..BUZZER_SETTLE_FRAMES + 20 {
                out.extend(t.step_nba_jam(s(p1, p2, 0)));
            }
        }
        out
    }

    #[test]
    fn buzzer_beater_counts() {
        let mut t = ScoreTracker::new();
        t.step_nba_jam(s(0, 0, 0));
        play(&mut t, &[(10, 8), (22, 25), (40, 39)]);
        t.step_nba_jam(s(60, 61, 0x000001));
        let mut ev = t.step_nba_jam(s(60, 61, 0));
        for _ in 0..49 {
            ev.extend(t.step_nba_jam(s(60, 61, 0)));
        }
        for _ in 0..BUZZER_SETTLE_FRAMES {
            ev.extend(t.step_nba_jam(s(63, 61, 0)));
        }
        assert_eq!(
            ev,
            vec![ScoreEvent::MatchOver {
                winner: 1,
                p1_wins: 63,
                p2_wins: 61
            }]
        );
    }

    #[test]
    fn nba_jam_game_ends_after_the_fourth_quarter() {
        let mut t = ScoreTracker::new();
        t.step_nba_jam(s(0, 0, 0)); // boot: 0:00 must not count
        let ev = play(&mut t, &[(10, 8), (22, 25), (40, 39), (61, 57)]);
        assert_eq!(
            ev,
            vec![ScoreEvent::MatchOver {
                winner: 1,
                p1_wins: 61,
                p2_wins: 57
            }]
        );
    }

    #[test]
    fn nba_jam_tie_goes_to_overtime() {
        let mut t = ScoreTracker::new();
        t.step_nba_jam(s(0, 0, 0));
        let ev = play(&mut t, &[(10, 8), (22, 25), (40, 39), (60, 60), (66, 71)]);
        assert_eq!(
            ev,
            vec![ScoreEvent::MatchOver {
                winner: 2,
                p1_wins: 66,
                p2_wins: 71
            }]
        );
    }

    #[test]
    fn nba_jam_next_game_starts_fresh() {
        let mut t = ScoreTracker::new();
        t.step_nba_jam(s(0, 0, 0));
        play(&mut t, &[(1, 0), (2, 0), (3, 0), (4, 0)]);
        assert_eq!(t.step_nba_jam(s(0, 0, 0)), vec![ScoreEvent::NewMatch]);
        // Two quarters into the next game: not over, despite the earlier four.
        assert!(play(&mut t, &[(5, 2), (9, 4)]).is_empty());
    }

    #[test]
    fn nba_jam_addresses_convert_like_the_mk2_table() {
        let a = nba_jam_addrs("nbajam").unwrap();
        assert_eq!(a.team1_score, 0x1E218);
        assert_eq!(a.team2_score, 0x1E21A);
        assert_eq!(a.clock, [0x1E1EB, 0x1E1EA, 0x1E1E9]);
        assert!(nba_jam_addrs("nbajamtep2").is_none());
        assert_eq!(nba_jam_addrs("nbajamte3"), nba_jam_addrs("nbajamte"));
    }

    #[test]
    fn hangtime_addresses_cover_every_revision() {
        let a = nba_hangtime_addrs("nbahangtl11").unwrap();
        assert_eq!(a.team1_score, 0x26F68);
        assert_eq!(a.team2_score, 0x26F6A);
        assert_eq!(nba_hangtime_addrs("nbahangt"), Some(a));
        assert_ne!(nba_hangtime_addrs("nbamht"), Some(a));
        assert!(nba_hangtime_addrs("nbajam").is_none());
    }
}
