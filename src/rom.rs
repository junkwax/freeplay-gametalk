use std::path::{Path, PathBuf};

const ROM_NAME: &str = "mk2.zip";

pub fn find_rom_zip() -> Option<PathBuf> {
    rom_candidates()
        .into_iter()
        .find(|p| p.exists())
        .or_else(|| first_zip_in("roms"))
        .or_else(|| first_zip_in("."))
}

pub fn find_rom_zip_string() -> Option<String> {
    find_rom_zip().map(|p| p.to_string_lossy().into_owned())
}

pub fn read_rom_zip() -> Option<Vec<u8>> {
    let path = find_rom_zip()?;
    std::fs::read(path).ok()
}

fn rom_candidates() -> Vec<PathBuf> {
    vec![
        Path::new(ROM_NAME).to_path_buf(),
        Path::new("roms").join(ROM_NAME),
    ]
}

fn first_zip_in(dir: &str) -> Option<PathBuf> {
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
