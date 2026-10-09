use std::path::{Path, PathBuf};

/// ROM sets this client boots, in the order they are preferred when more
/// than one is present. FBNeo picks its driver from the file name, so these
/// are driver names too: `mk2` is the T-unit build, and `umk3` is the same
/// game rebuilt for the Wolf unit, which ships inside a UMK3 donor set
/// (mk2-main `makewolf.py`).
const ROM_NAMES: [&str; 2] = ["mk2.zip", "umk3.zip"];

/// `--rom <path>`: the one ROM zip to use, instead of looking for one.
static OVERRIDE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

/// Read `--rom` off the command line. Call before anything changes the
/// working directory, so a relative path means what the user typed.
pub fn init_override() {
    OVERRIDE.get_or_init(|| {
        let path = crate::cli::rom_override()?;
        let path = std::fs::canonicalize(&path).map(strip_verbatim).unwrap_or(path);
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
    rom_candidates()
        .into_iter()
        .find(|p| p.exists())
        .or_else(|| first_zip_in("roms"))
        .or_else(|| first_zip_in("."))
        .or_else(|| exe_dir().and_then(|dir| first_zip_in_path(&dir.join("roms"))))
        .or_else(|| exe_dir().and_then(|dir| first_zip_in_path(&dir)))
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

fn rom_candidates() -> Vec<PathBuf> {
    let exe_dir = exe_dir();
    let mut candidates = Vec::new();
    for name in ROM_NAMES {
        candidates.push(Path::new("roms").join(name));
        candidates.push(Path::new(name).to_path_buf());
        if let Some(exe_dir) = &exe_dir {
            candidates.push(exe_dir.join("roms").join(name));
            candidates.push(exe_dir.join(name));
        }
    }
    candidates
}

fn first_zip_in(dir: &str) -> Option<PathBuf> {
    first_zip_in_path(Path::new(dir))
}

/// Fallback scan for a ROM zip that isn't named exactly `mk2.zip` (or
/// `umk3.zip`). Only accepts filenames that still look like one of those sets
/// ("mk2 (1).zip", "MK2.zip", "mk2-l31.zip", ...). An unrestricted "first zip in the folder"
/// scan used to run here — with a `kof98.zip` sorting first, the client would
/// silently boot the wrong game (or hand FBNeo garbage) and desync online.
fn first_zip_in_path(dir: &Path) -> Option<PathBuf> {
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
                        ROM_NAMES
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
