use std::path::{Path, PathBuf};

use crate::game::{self, GameId, Profile};
use std::sync::Mutex;

// ROM names live on each game's profile (`game::Profile::rom_names`). For
// MK2: `mk2` is the T-unit build, and `umk3` is the same game rebuilt for the
// Wolf unit, which ships inside a UMK3 donor set (mk2-main `makewolf.py`).

/// The game to look for first (config.toml `game`). The others are still
/// found if it isn't installed, so a player who only has NBA Jam boots NBA
/// Jam whatever the config says.
static PREFERRED: Mutex<Option<GameId>> = Mutex::new(None);

pub fn set_preferred_game(id: Option<GameId>) {
    *PREFERRED.lock().unwrap_or_else(|e| e.into_inner()) = id;
}

/// The ROM file to boot (config.toml `rom`, a bare file name like
/// "umk3.zip"), set by the main menu's ROM switcher. Wins over `PREFERRED`
/// while it is installed; once it's gone the game preference takes over.
static PREFERRED_ROM: Mutex<String> = Mutex::new(String::new());

pub fn set_preferred_rom(name: &str) {
    *PREFERRED_ROM.lock().unwrap_or_else(|e| e.into_inner()) = name.trim().to_ascii_lowercase();
}

fn file_name_lower(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn search_order() -> Vec<&'static Profile> {
    let preferred = *PREFERRED.lock().unwrap_or_else(|e| e.into_inner());
    let mut order: Vec<&'static Profile> = game::ALL.to_vec();
    if let Some(id) = preferred {
        order.sort_by_key(|p| p.id != id);
    }
    order
}

/// `--rom <path>`: the one ROM zip to use, instead of looking for one.
static OVERRIDE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

/// Read `--rom` off the command line. Call before anything changes the
/// working directory, so a relative path means what the user typed.
pub fn init_override() {
    OVERRIDE.get_or_init(|| {
        let path = crate::cli::rom_override()?;
        let path = std::fs::canonicalize(&path)
            .map(strip_verbatim)
            .unwrap_or(path);
        if path.is_file() {
            println!("[rom] --rom {}", path.display());
        } else {
            println!("[rom] --rom {} does not exist", path.display());
        }
        Some(path)
    });
}

/// `canonicalize` answers in `\\?\C:\...` form on Windows. The core builds
/// its own paths from the one it is handed and does not take that form.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}

pub fn find_rom_zip() -> Option<PathBuf> {
    // A named ROM that is missing is an error to report, not a reason to
    // boot a different game.
    if let Some(Some(path)) = OVERRIDE.get() {
        return path.is_file().then(|| path.clone());
    }
    let wanted = PREFERRED_ROM
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if !wanted.is_empty() {
        if let Some(path) = installed_roms()
            .into_iter()
            .find(|p| file_name_lower(p) == wanted)
        {
            return Some(path);
        }
    }
    search_order().into_iter().find_map(find_game_rom)
}

/// Every installed ROM zip, one per file name: each game in `game::ALL`
/// order, its exact `rom_names` in preference order, then (MK2 only) a
/// misnamed copy if no exact name is present. This is what the main menu's
/// LB/RB switcher cycles through, so `mk2.zip` and `umk3.zip` are both
/// reachable even though only one of them is ever the default.
pub fn installed_roms() -> Vec<PathBuf> {
    if let Some(Some(path)) = OVERRIDE.get() {
        return path
            .is_file()
            .then(|| vec![path.clone()])
            .unwrap_or_default();
    }
    collect_roms(
        |name| name_candidates(name).into_iter().find(|p| p.is_file()),
        find_game_rom,
    )
}

/// `installed_roms` minus the filesystem: `exact` finds a ROM by file name,
/// `fallback` finds a game's ROM any way it can (MK2's misnamed copies).
fn collect_roms(
    exact: impl Fn(&str) -> Option<PathBuf>,
    fallback: impl Fn(&Profile) -> Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for game in game::ALL {
        let before = out.len();
        out.extend(game.rom_names.iter().filter_map(|name| exact(name)));
        if out.len() == before {
            out.extend(fallback(game));
        }
    }
    out
}

/// `installed_roms`, rechecked at most once a second (the main menu asks
/// every frame).
pub fn installed_roms_cached() -> Vec<PathBuf> {
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Vec<PathBuf>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let stale = cache
        .as_ref()
        .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(1));
    if stale {
        *cache = Some((Instant::now(), installed_roms()));
    }
    cache
        .as_ref()
        .map(|(_, roms)| roms.clone())
        .unwrap_or_default()
}

/// The installed ROM `step` places after (1) or before (-1) the one this
/// process boots, wrapping. None when there's nothing else to switch to.
pub fn neighbouring_rom(step: i32) -> Option<PathBuf> {
    let roms = installed_roms_cached();
    if roms.len() < 2 {
        return None;
    }
    // The running ROM, not a fresh `find_rom_zip`: the preference may have
    // been saved already by a switch whose relaunch is still pending.
    let here_name = loaded_rom_name();
    let here = roms
        .iter()
        .position(|p| file_name_lower(p) == here_name)
        .unwrap_or(0) as i32;
    let n = roms.len() as i32;
    Some(roms[((here + step).rem_euclid(n)) as usize].clone())
}

/// File name of the ROM this process booted (or will boot), lowercase.
fn loaded_rom_name() -> String {
    static BOOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BOOT.get_or_init(|| {
        find_rom_zip()
            .map(|p| file_name_lower(&p))
            .unwrap_or_default()
    })
    .clone()
}

/// How the switcher names a ROM: the game, plus the file when that game has
/// more than one ROM installed (MK2's T-unit and Wolf builds share a title).
pub fn rom_label(path: &Path) -> String {
    let game = game::by_id(game::detect(path));
    let siblings = installed_roms_cached()
        .iter()
        .filter(|p| game::detect(p) == game.id)
        .count();
    if siblings > 1 {
        format!(
            "{} ({})",
            game.title,
            file_name_lower(path).to_ascii_uppercase()
        )
    } else {
        game.title.to_string()
    }
}

/// This game's ROM, if installed. Exact names first, then (MK2 only) a
/// misnamed copy.
pub fn find_game_rom(game: &Profile) -> Option<PathBuf> {
    rom_candidates(game)
        .into_iter()
        .find(|p| p.exists())
        .or_else(|| {
            if !game.fuzzy_names {
                return None;
            }
            first_zip_in("roms", game)
                .or_else(|| first_zip_in(".", game))
                .or_else(|| exe_dir().and_then(|dir| first_zip_in_path(&dir.join("roms"), game)))
                .or_else(|| exe_dir().and_then(|dir| first_zip_in_path(&dir, game)))
        })
}

/// Cached ROM presence with a periodic recheck. `find_rom_zip` stats several
/// paths and scans up to four directories; the menu asks every frame, so an
/// uncached check is filesystem I/O at ~55 Hz. A 1-second recheck keeps the
/// "drop mk2.zip in while the app is running" detection.
pub struct PresenceCache {
    present: bool,
    next_check: std::time::Instant,
}

impl PresenceCache {
    pub fn new() -> Self {
        Self {
            present: find_rom_zip().is_some(),
            next_check: std::time::Instant::now() + std::time::Duration::from_secs(1),
        }
    }

    pub fn check(&mut self) -> bool {
        let now = std::time::Instant::now();
        if now >= self.next_check {
            self.present = find_rom_zip().is_some();
            self.next_check = now + std::time::Duration::from_secs(1);
        }
        self.present
    }
}

pub fn find_rom_zip_string() -> Option<String> {
    find_rom_zip().map(|p| p.to_string_lossy().into_owned())
}

pub fn read_rom_zip() -> Option<Vec<u8>> {
    let path = find_rom_zip()?;
    std::fs::read(path).ok()
}

fn rom_candidates(game: &Profile) -> Vec<PathBuf> {
    game.rom_names
        .iter()
        .flat_map(|name| name_candidates(name))
        .collect()
}

fn name_candidates(name: &str) -> Vec<PathBuf> {
    let mut candidates = vec![Path::new("roms").join(name), Path::new(name).to_path_buf()];
    if let Some(exe_dir) = exe_dir() {
        candidates.push(exe_dir.join("roms").join(name));
        candidates.push(exe_dir.join(name));
    }
    candidates
}

fn first_zip_in(dir: &str, game: &Profile) -> Option<PathBuf> {
    first_zip_in_path(Path::new(dir), game)
}

/// Fallback scan for a ROM zip that isn't named exactly `mk2.zip` (or
/// `umk3.zip`). Only accepts filenames that still look like one of those sets
/// ("mk2 (1).zip", "MK2.zip", "mk2-l31.zip", ...). An unrestricted "first zip in the folder"
/// scan used to run here — with a `kof98.zip` sorting first, the client would
/// silently boot the wrong game (or hand FBNeo garbage) and desync online.
fn first_zip_in_path(dir: &Path, game: &Profile) -> Option<PathBuf> {
    let mut zips: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| {
                        let stem = stem.to_ascii_lowercase();
                        game.rom_names
                            .iter()
                            .any(|name| stem.starts_with(name.trim_end_matches(".zip")))
                    })
        })
        .collect();
    zips.sort();
    zips.into_iter().next()
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roms_from(present: &[&str], fuzzy: Option<&str>) -> Vec<String> {
        collect_roms(
            |name| {
                present
                    .contains(&name)
                    .then(|| PathBuf::from("roms").join(name))
            },
            |g| (g.fuzzy_names).then(|| fuzzy.map(PathBuf::from)).flatten(),
        )
        .iter()
        .map(|p| file_name_lower(p))
        .collect()
    }

    #[test]
    fn switcher_cycles_every_rom_of_every_game() {
        // The v0.9.1 package's roms\ folder.
        assert_eq!(
            roms_from(&["nbajam.zip", "umk3.zip", "mk2.zip"], None),
            ["mk2.zip", "umk3.zip", "nbajam.zip"]
        );
    }

    #[test]
    fn misnamed_mk2_only_counts_without_an_exact_name() {
        assert_eq!(roms_from(&["mk2.zip"], Some("mk2 (1).zip")), ["mk2.zip"]);
        assert_eq!(
            roms_from(&["nbajamte.zip"], Some("mk2 (1).zip")),
            ["mk2 (1).zip", "nbajamte.zip"]
        );
    }
}
