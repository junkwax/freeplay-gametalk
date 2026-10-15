//! Which cabinet is running, and everything about it that isn't the same
//! for every game.
//!
//! The netcode never sees the game: rollback, savestates, ghosts and replays
//! move opaque bytes. What does differ lives here — the ROM names, the input
//! set (which is wire protocol, see `input::action_set_fingerprint`), which
//! core port the second player drives, the DIP switches the core must be
//! told, whether the MK2 RAM tooling applies, and the key that keeps each
//! game's matchmaking pool and ratings apart.
//!
//! One game per process. Switching games relaunches the client
//! (`relaunch_with_game`) rather than swapping cores under the main loop:
//! the clean boot state, audio rate and runahead buffers all hang off the
//! loaded core, and a fresh process is the only way to guarantee none of
//! one game's state survives into the other.

use crate::input::Action;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GameId {
    Mk2,
    NbaJam,
    NbaHangtime,
}

pub struct Profile {
    pub id: GameId,
    /// Stable short key: config.toml `game`, the stats service's rating
    /// pool, and the matchmaking key prefix. Never rename one.
    pub key: &'static str,
    pub title: &'static str,
    pub caption: &'static str,
    /// Short name for tight labels (Discord presence).
    pub short: &'static str,
    /// The giant outlined mark behind the main menu.
    pub watermark: &'static str,
    /// The netplay input set, in wire order. Bit `i` of `NetInput` is
    /// `actions[i]`.
    pub actions: &'static [Action],
    /// ROM zip names this game boots, preferred first. FBNeo picks its
    /// driver from the file name, so these are driver names too.
    pub rom_names: &'static [&'static str],
    /// Whether a misnamed zip ("mk2 (1).zip") still counts, by prefix of a
    /// `rom_names` stem. Only for MK2, whose players get one-off builds; NBA
    /// Jam's revisions have different RAM layouts and its prefixes overlap
    /// ("nbajam" is a prefix of "nbajamte"), so a guess would be worse than
    /// no match.
    pub fuzzy_names: bool,
    /// libretro port the app's second player drives. NBA Jam is a
    /// four-player cabinet where P1+P2 are one team and P3+P4 the other
    /// (Pugsy's cheat file labels the score bytes "P1+P2 Team 1" /
    /// "P3+P4 Team 2"), so a head-to-head opponent sits on port 2.
    pub second_player_port: u32,
    /// FBNeo DIP core options as (key suffix, value). Keys are
    /// `fbneo-dipswitch-<driver>-<Name>`, so matching on the suffix covers
    /// every revision of the game. Both peers answer from this same table,
    /// which is what keeps a DIP from becoming a desync.
    pub dip_options: &'static [(&'static str, &'static str)],
    /// The Lab, trainer pokes, drones, hitbox view and the netplay RAM flag
    /// all run off the MK2 address table. On any other game those addresses
    /// are some unrelated variable, so the tooling must stay off entirely.
    pub mk2_tooling: bool,
}

pub const MK2: Profile = Profile {
    id: GameId::Mk2,
    key: "mk2",
    title: "MORTAL KOMBAT II",
    caption: "ARCADE \u{b7} 1993 \u{b7} NETPLAY FREEPLAY",
    watermark: "II",
    short: "MK2",
    actions: &Action::MK2,
    rom_names: &["mk2.zip", "umk3.zip"],
    fuzzy_names: true,
    second_player_port: 1,
    dip_options: &[],
    mk2_tooling: true,
};

pub const NBA_JAM: Profile = Profile {
    id: GameId::NbaJam,
    key: "nbajam",
    title: "NBA JAM",
    caption: "ARCADE \u{b7} 1993 \u{b7} NETPLAY FREEPLAY",
    watermark: "JAM",
    short: "NBA Jam",
    actions: &Action::NBA_JAM,
    rom_names: &[
        "nbajam.zip",
        "nbajamte.zip",
        "nbajamr2.zip",
        "nbajamte4.zip",
        "nbajamte3.zip",
        "nbajamte3a.zip",
        "nbajamte2.zip",
        "nbajamte2a.zip",
        "nbajamte1.zip",
        "nbajamr1.zip",
        "nbajamp2.zip",
        "nbajamp1.zip",
        "nbajamtep2.zip",
        "nbajamten.zip",
    ],
    fuzzy_names: false,
    second_player_port: 2,
    // The cabinet is coin-op and, by default, takes its coinage from CMOS
    // rather than the DIPs — so both are needed for free play. MK2 gets the
    // same effect from its own ROM's netplay flag; NBA Jam has no such flag.
    dip_options: &[("-Coinage_Source", "Dipswitch"), ("-Coinage", "Free Play")],
    mk2_tooling: false,
};

/// Wolf unit, whose driver the shipped core already has for MK2-on-Wolf.
/// Same three buttons on the same bits as NBA Jam (MAME midwunit.cpp: 0x10
/// Shoot, 0x20 Pass, 0x40 Turbo), so it shares that action set. Maximum
/// Hangtime is a roster update of the same game and shares this profile;
/// its revisions never pair with Hangtime's, since the matchmaking key
/// carries the ROM hash.
pub const NBA_HANGTIME: Profile = Profile {
    id: GameId::NbaHangtime,
    key: "nbahangt",
    title: "NBA HANGTIME",
    caption: "ARCADE \u{b7} 1996 \u{b7} NETPLAY FREEPLAY",
    watermark: "HT",
    short: "NBA Hangtime",
    actions: &Action::NBA_JAM,
    rom_names: &[
        "nbahangt.zip",
        "nbahangtm13.zip",
        "nbahangtl12.zip",
        "nbahangtm12.zip",
        "nbahangtl11.zip",
        "nbahangtm11.zip",
        "nbamht.zip",
        "nbamhtm10.zip",
        "nbamhtl10.zip",
        "nbamhtp.zip",
    ],
    fuzzy_names: false,
    // Unlike NBA Jam, a DIP picks the seating: on the 2-player cabinet P2
    // is the other team, so a head-to-head opponent is plain P2.
    second_player_port: 1,
    // Coinage comes from CMOS unless told otherwise, as on NBA Jam. 2-player
    // is FBNeo's default cabinet already; pinned so the seating above can't
    // move under us.
    dip_options: &[
        ("-Coinage_Source", "Dipswitch"),
        ("-Coinage", "Free Play"),
        ("-Cabinet", "2-player"),
    ],
    mk2_tooling: false,
};

pub const ALL: [&Profile; 3] = [&MK2, &NBA_JAM, &NBA_HANGTIME];

static CURRENT: AtomicU8 = AtomicU8::new(0);

fn index_of(id: GameId) -> u8 {
    ALL.iter().position(|p| p.id == id).unwrap_or(0) as u8
}

/// The game this process is running (or will run once the core loads).
pub fn current() -> &'static Profile {
    ALL[CURRENT.load(Ordering::Relaxed) as usize % ALL.len()]
}

pub fn set_current(id: GameId) {
    let prev = CURRENT.swap(index_of(id), Ordering::Relaxed);
    if prev != index_of(id) {
        crate::dlog!("boot", "game -> {}", by_id(id).key);
    }
}

pub fn by_id(id: GameId) -> &'static Profile {
    ALL[index_of(id) as usize]
}

pub fn by_key(key: &str) -> Option<&'static Profile> {
    ALL.iter()
        .copied()
        .find(|p| p.key.eq_ignore_ascii_case(key.trim()))
}

/// The driver a ROM zip boots, which is its lowercase file stem.
pub fn driver_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Which game a ROM zip is. Exact driver names first; MK2's fuzzy prefix
/// last. Anything unrecognised is MK2, which is what this client always
/// assumed before it knew about other games.
pub fn detect(path: &Path) -> GameId {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    for p in ALL {
        if p.rom_names.iter().any(|n| *n == name) {
            return p.id;
        }
    }
    GameId::Mk2
}

/// The driver of the ROM the core was loaded with, for game logic that
/// differs between revisions (NBA Jam's RAM layout moves between them).
static DRIVER: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Called by `retro::load` with the ROM it is about to boot, so every load
/// path — app, probes, tests — agrees on the game before the core asks for
/// its options or reads input.
pub fn note_loaded_rom(rom_path: &str) {
    let path = Path::new(rom_path);
    set_current(detect(path));
    *DRIVER.lock().unwrap_or_else(|e| e.into_inner()) = driver_of(path);
}

pub fn loaded_driver() -> String {
    DRIVER.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Answer for an FBNeo `GET_VARIABLE` query, if the current game sets it.
pub fn core_option(key: &str) -> Option<&'static str> {
    option_for(current(), key)
}

fn option_for(game: &'static Profile, key: &str) -> Option<&'static str> {
    if !key.starts_with("fbneo-dipswitch-") {
        return None;
    }
    game.dip_options
        .iter()
        .find(|(suffix, _)| key.ends_with(suffix))
        .map(|(_, value)| *value)
}

/// Restart the client on another game. Replaces any `--rom` with the new
/// game's preference; every other argument (`--player`, logging, ...) is
/// kept. The caller has already saved `config.game`.
pub fn relaunch_with_game(game: &Profile) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut args: Vec<String> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        if arg == "--rom" {
            it.next();
            continue;
        }
        if arg.starts_with("--rom=") || arg.starts_with("xband://") {
            continue;
        }
        args.push(arg);
    }
    println!("[game] relaunching on {}", game.key);
    std::process::Command::new(exe).args(args).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rom_names_pick_the_game() {
        assert_eq!(detect(Path::new("roms/mk2.zip")), GameId::Mk2);
        assert_eq!(detect(Path::new("roms/umk3.zip")), GameId::Mk2);
        assert_eq!(detect(Path::new("C:/x/NBAJAM.zip")), GameId::NbaJam);
        assert_eq!(detect(Path::new("nbajamte3.zip")), GameId::NbaJam);
        assert_eq!(detect(Path::new("mk2 (1).zip")), GameId::Mk2);
        assert_eq!(
            detect(Path::new("roms/nbahangtl11.zip")),
            GameId::NbaHangtime
        );
        assert_eq!(detect(Path::new("nbamht.zip")), GameId::NbaHangtime);
    }

    #[test]
    fn dip_options_match_every_revision_and_nothing_else() {
        let jam = &NBA_JAM;
        assert_eq!(
            option_for(jam, "fbneo-dipswitch-nbajam-Coinage"),
            Some("Free Play")
        );
        assert_eq!(
            option_for(jam, "fbneo-dipswitch-nbajamte-Coinage_Source"),
            Some("Dipswitch")
        );
        assert_eq!(option_for(jam, "fbneo-dipswitch-nbajam-Players"), None);
        assert_eq!(option_for(jam, "fbneo-allow-patched-romsets"), None);
        assert_eq!(option_for(&MK2, "fbneo-dipswitch-mk2-Coinage"), None);
        let ht = &NBA_HANGTIME;
        assert_eq!(
            option_for(ht, "fbneo-dipswitch-nbahangtl11-Coinage"),
            Some("Free Play")
        );
        assert_eq!(
            option_for(ht, "fbneo-dipswitch-nbamht-Cabinet"),
            Some("2-player")
        );
    }

    #[test]
    fn keys_are_unique_and_round_trip() {
        for p in ALL {
            assert_eq!(by_key(p.key).map(|q| q.id), Some(p.id));
        }
    }
}
