use std::path::{Path, PathBuf};

const ROM_NAME: &str = "mk2.zip";

pub fn find_rom_zip() -> Option<PathBuf> {
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
    let mut candidates = vec![
        Path::new(ROM_NAME).to_path_buf(),
        Path::new("roms").join(ROM_NAME),
    ];
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        candidates.push(exe_dir.join(ROM_NAME));
        candidates.push(exe_dir.join("roms").join(ROM_NAME));
    }
    candidates
}

fn first_zip_in(dir: &str) -> Option<PathBuf> {
    first_zip_in_path(Path::new(dir))
}

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
