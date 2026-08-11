//! Does the loaded ROM match the address table compiled into this build?
//!
//! `mk2_addrs` is generated from one mk2-main build's `mk2.map` and is only
//! meaningful against the ROM that build produced. Run a client whose table
//! came from a different build than the player's `mk2.zip` and every Lab RAM
//! feature reads the wrong part of memory: the trainer shows nothing,
//! position presets poke nowhere, the frame trace logs noise. None of it
//! errors. Releases ship no ROM, so this is the *expected* state for anyone
//! who updates the client without updating their ROM.
//!
//! The check is an exact identity — the FNV of the ROM zip, the same hash
//! `matchmaking` already computes for pairing — recorded into the table when
//! it is exported. Both halves come out of one mk2-main build, so recording
//! it there is what keeps them honest.
//!
//! An earlier attempt inferred the answer instead, by reading values with
//! hard bounds (boolean flags, fighter ids indexing a 17-entry roster) and
//! asking whether they were possible. It does not work, and the failure is
//! worth recording so nobody rebuilds it: this whole RAM region reads zero
//! through boot and all of attract — verified over 3,600 frames with
//! `--addr-probe` — and zero satisfies every bound. A completely wrong table
//! pointing at any of the many zeroed regions passes just as cleanly as a
//! correct one. The heuristic could only ever have produced false
//! reassurance, which is worse than no check.

use crate::mk2_addrs as addr;

/// Every entry `--addr-probe` dumps. Named so the output can be read against
/// `mk2.map` by hand when a mismatch needs diagnosing rather than detecting.
pub const TABLE_SAMPLE: &[(&str, usize)] = &[
    ("p1_char", addr::P1_CHAR_ADDR),
    ("p2_char", addr::P2_CHAR_ADDR),
    ("round_num", addr::ROUND_NUM),
    ("p1_matchw", addr::P1_MATCHW),
    ("p2_matchw", addr::P2_MATCHW),
    ("winner_status", addr::WINNER_STATUS),
    ("p1_hp", addr::P1_HP_ADDR),
    ("p2_hp", addr::P2_HP_ADDR),
    ("p1_proc", addr::P1_PROC_ADDR),
    ("p2_proc", addr::P2_PROC_ADDR),
    ("gstate", addr::GSTATE_ADDR),
    ("timer", addr::MPROC_TIMER_ADDR),
    ("f_colbox", addr::HITBOX_FLAG_ADDR),
    ("f_shadows", addr::SHADOWS_FLAG_ADDR),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pairing {
    /// The table records no source ROM, so there is nothing to compare and
    /// no claim to make. Tables exported before the exporter recorded one.
    Unrecorded,
    /// No ROM is loaded or readable — not evidence either way.
    NoRom,
    Matches,
    Mismatch { expected: String, found: String },
}

/// Compare the ROM on disk against the one this build's table came from.
pub fn check() -> Pairing {
    let expected = addr::SOURCE_ROM_FNV.trim();
    if expected.is_empty() {
        return Pairing::Unrecorded;
    }
    let found = crate::matchmaking::rom_fnv_hash();
    if found == "0" {
        return Pairing::NoRom;
    }
    if found.eq_ignore_ascii_case(expected) {
        Pairing::Matches
    } else {
        Pairing::Mismatch {
            expected: expected.to_string(),
            found,
        }
    }
}

impl Pairing {
    /// The message a player should see, or `None` when there is nothing
    /// worth saying. Deliberately advisory: the mismatch makes Lab's RAM
    /// features useless, not the game unplayable, and refusing to launch
    /// over it would be a worse trade than saying so.
    pub fn lab_warning(&self) -> Option<String> {
        match self {
            Pairing::Mismatch { .. } => Some(
                "Lab RAM tools need the mk2.zip this build was made for - yours is different"
                    .to_string(),
            ),
            _ => None,
        }
    }

    /// One line for the console and debug log at startup.
    pub fn log_line(&self) -> String {
        match self {
            Pairing::Unrecorded => {
                "MK2 address table records no source ROM - pairing unchecked".into()
            }
            Pairing::NoRom => "MK2 address table pairing unchecked - no ROM readable".into(),
            Pairing::Matches => "MK2 address table matches the loaded ROM".into(),
            Pairing::Mismatch { expected, found } => format!(
                "MK2 address table was built for ROM {expected} but {found} is loaded - \
                 Lab RAM features will read the wrong memory"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_mismatch_warns_the_player() {
        assert!(Pairing::Matches.lab_warning().is_none());
        assert!(Pairing::Unrecorded.lab_warning().is_none());
        // No ROM means Lab is not reachable anyway; warning would be noise.
        assert!(Pairing::NoRom.lab_warning().is_none());
        assert!(Pairing::Mismatch {
            expected: "aaaaaaaa".into(),
            found: "bbbbbbbb".into(),
        }
        .lab_warning()
        .is_some());
    }

    /// The mismatch line has to carry both hashes: "your ROM is wrong" is
    /// not actionable, "expected X, found Y" tells you which build to get.
    #[test]
    fn the_mismatch_log_names_both_roms() {
        let line = Pairing::Mismatch {
            expected: "24f627cd".into(),
            found: "deadbeef".into(),
        }
        .log_line();
        assert!(line.contains("24f627cd"), "{line}");
        assert!(line.contains("deadbeef"), "{line}");
    }

    /// An exported table should carry its ROM. If this fails the exporter
    /// was run without `--rom`, and the pairing silently stops being checked.
    #[test]
    fn the_shipped_table_records_its_source_rom() {
        let recorded = addr::SOURCE_ROM_FNV.trim();
        assert!(
            !recorded.is_empty(),
            "mk2_addrs::SOURCE_ROM_FNV is empty - re-export with --rom"
        );
        assert_eq!(recorded.len(), 8, "expected an 8-hex-digit FNV: {recorded}");
        assert!(recorded.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
