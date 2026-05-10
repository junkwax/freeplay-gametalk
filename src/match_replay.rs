use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::input::{self, Player};
use crate::retro::Core;

const MAGIC: &[u8; 4] = b"NCRP";
const VERSION: u16 = 1;
const REPLAY_DIR: &str = "replays";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayMeta {
    pub filename: String,
    pub path: String,
    pub p1_name: String,
    pub p2_name: String,
    pub frame_count: u32,
}

pub struct Recording {
    initial_state: Option<Vec<u8>>,
    inputs: Vec<[u16; 2]>,
    p1_name: String,
    p2_name: String,
    started_unix: u64,
}

impl Recording {
    pub fn new(p1_name: impl Into<String>, p2_name: impl Into<String>) -> Self {
        Self {
            initial_state: None,
            inputs: Vec::new(),
            p1_name: clean_name(&p1_name.into()),
            p2_name: clean_name(&p2_name.into()),
            started_unix: unix_now(),
        }
    }

    pub fn record_confirmed_frame(&mut self, core: &Core, p1_bits: u16, p2_bits: u16) {
        if self.initial_state.is_none() {
            self.initial_state = core.save_state();
            if let Some(state) = &self.initial_state {
                println!("[replay] Anchor state captured ({} bytes)", state.len());
            }
        }
        if self.initial_state.is_some() {
            self.inputs.push([p1_bits, p2_bits]);
        }
    }

    pub fn frame_count(&self) -> usize {
        self.inputs.len()
    }

    pub fn save_default(&self) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(REPLAY_DIR)?;
        let filename = format!(
            "{}_{}_vs_{}.ncrp",
            self.started_unix,
            filename_part(&self.p1_name),
            filename_part(&self.p2_name)
        );
        let path = Path::new(REPLAY_DIR).join(filename);
        self.save(&path)?;
        Ok(path)
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        let Some(state) = &self.initial_state else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "no anchor state captured",
            ));
        };
        let mut f = std::fs::File::create(path)?;
        write_header(
            &mut f,
            self.inputs.len() as u32,
            state.len() as u32,
            &self.p1_name,
            &self.p2_name,
        )?;
        f.write_all(state)?;
        for frame in &self.inputs {
            f.write_all(&frame[0].to_le_bytes())?;
            f.write_all(&frame[1].to_le_bytes())?;
        }
        Ok(())
    }
}

pub struct Playback {
    state: Vec<u8>,
    inputs: Vec<[u16; 2]>,
    cursor: usize,
    p1_name: String,
    p2_name: String,
}

impl Playback {
    pub fn load<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let mut f = std::fs::File::open(path.as_ref())?;
        let header = read_header(&mut f)?;
        let mut state = vec![0u8; header.state_size as usize];
        f.read_exact(&mut state)?;
        let mut inputs = Vec::with_capacity(header.frame_count as usize);
        for _ in 0..header.frame_count {
            let mut buf = [0u8; 4];
            f.read_exact(&mut buf)?;
            inputs.push([
                u16::from_le_bytes([buf[0], buf[1]]),
                u16::from_le_bytes([buf[2], buf[3]]),
            ]);
        }
        Ok(Self {
            state,
            inputs,
            cursor: 0,
            p1_name: header.p1_name,
            p2_name: header.p2_name,
        })
    }

    pub fn prime(&self, core: &Core) -> bool {
        let ok = core.load_state(&self.state);
        if ok {
            println!(
                "[replay] Loaded anchor state ({} bytes), {} frames queued",
                self.state.len(),
                self.inputs.len()
            );
        }
        ok
    }

    pub fn inject_next(&mut self) -> bool {
        let Some(frame) = self.inputs.get(self.cursor).copied() else {
            return false;
        };
        input::apply_snapshot(Player::P1, frame[0]);
        input::apply_snapshot(Player::P2, frame[1]);
        self.cursor += 1;
        true
    }

    pub fn frame_count(&self) -> usize {
        self.inputs.len()
    }

    pub fn p1_name(&self) -> &str {
        &self.p1_name
    }

    pub fn p2_name(&self) -> &str {
        &self.p2_name
    }
}

pub fn list_replays() -> Vec<ReplayMeta> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir(REPLAY_DIR) else {
        return out;
    };
    for entry in dir.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ncrp") {
            continue;
        }
        if let Ok(meta) = read_meta(&path) {
            out.push(meta);
        }
    }
    out.sort_by(|a, b| b.filename.cmp(&a.filename));
    out
}

pub fn finalize_recording(rec: &mut Option<Recording>) -> Option<PathBuf> {
    let rec = rec.take()?;
    if rec.frame_count() == 0 {
        println!("[replay] Session produced 0 frames, nothing to save.");
        return None;
    }
    match rec.save_default() {
        Ok(path) => {
            println!(
                "[replay] Saved {} frames to {}",
                rec.frame_count(),
                path.display()
            );
            Some(path)
        }
        Err(e) => {
            println!("[replay] Save failed: {e}");
            None
        }
    }
}

fn read_meta(path: &Path) -> std::io::Result<ReplayMeta> {
    let mut f = std::fs::File::open(path)?;
    let header = read_header(&mut f)?;
    Ok(ReplayMeta {
        filename: path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown.ncrp")
            .to_string(),
        path: path.to_string_lossy().into_owned(),
        p1_name: header.p1_name,
        p2_name: header.p2_name,
        frame_count: header.frame_count,
    })
}

struct Header {
    frame_count: u32,
    state_size: u32,
    p1_name: String,
    p2_name: String,
}

fn write_header<W: Write>(
    w: &mut W,
    frame_count: u32,
    state_size: u32,
    p1_name: &str,
    p2_name: &str,
) -> std::io::Result<()> {
    let p1 = p1_name.as_bytes();
    let p2 = p2_name.as_bytes();
    if p1.len() > u16::MAX as usize || p2.len() > u16::MAX as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "player name too long",
        ));
    }
    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    w.write_all(&frame_count.to_le_bytes())?;
    w.write_all(&state_size.to_le_bytes())?;
    w.write_all(&(p1.len() as u16).to_le_bytes())?;
    w.write_all(&(p2.len() as u16).to_le_bytes())?;
    w.write_all(p1)?;
    w.write_all(p2)?;
    Ok(())
}

fn read_header<R: Read>(r: &mut R) -> std::io::Result<Header> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("bad replay magic: expected NCRP, got {:?}", magic),
        ));
    }
    let version = read_u16(r)?;
    if version != VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unsupported replay version: {version}"),
        ));
    }
    let frame_count = read_u32(r)?;
    let state_size = read_u32(r)?;
    let p1_len = read_u16(r)? as usize;
    let p2_len = read_u16(r)? as usize;
    let mut p1 = vec![0u8; p1_len];
    let mut p2 = vec![0u8; p2_len];
    r.read_exact(&mut p1)?;
    r.read_exact(&mut p2)?;
    Ok(Header {
        frame_count,
        state_size,
        p1_name: String::from_utf8_lossy(&p1).into_owned(),
        p2_name: String::from_utf8_lossy(&p2).into_owned(),
    })
}

fn read_u16<R: Read>(r: &mut R) -> std::io::Result<u16> {
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32<R: Read>(r: &mut R) -> std::io::Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn clean_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "Player".into()
    } else {
        trimmed.chars().take(32).collect()
    }
}

fn filename_part(name: &str) -> String {
    let mut out: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(24)
        .collect();
    if out.is_empty() {
        out = "Player".into();
    }
    out
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_parts_are_safe() {
        assert_eq!(filename_part("Liu Kang!"), "LiuKang");
        assert_eq!(filename_part(""), "Player");
    }

    #[test]
    fn header_roundtrips_metadata() {
        let mut bytes = Vec::new();
        write_header(&mut bytes, 123, 456, "P1", "Opponent").unwrap();
        let header = read_header(&mut bytes.as_slice()).unwrap();
        assert_eq!(header.frame_count, 123);
        assert_eq!(header.state_size, 456);
        assert_eq!(header.p1_name, "P1");
        assert_eq!(header.p2_name, "Opponent");
    }
}
