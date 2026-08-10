//! Lab-mode frame trace — a readable, per-frame CSV of inputs and game state.
//!
//! The ghost recorder (`ghost.rs`) already captures everything needed to
//! *replay* a sequence: a savestate anchor plus per-frame input bits. That is
//! the right format for a machine and a useless one to read, because nothing
//! in it records what the game did in response.
//!
//! This writes the other half: one row per frame with the decoded inputs
//! alongside each fighter's action id, health, position and the gap between
//! them. From that a move's real frame data falls straight out — the action
//! id appears on startup, the opponent's health drops on the active frame,
//! the id clears on recovery — which is data MK2 has never had published,
//! because it is normally eyeballed off video.
//!
//! Written alongside the ghost recording on the same lab hotkey, to
//! `lab_traces/trace_<unix>.csv`.

use crate::memory::{peek_u16, Endian};
use crate::mk2_addrs as addr;
use crate::retro::Core;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

/// p_action within a process block: pdata + 0f0h in ROM bit terms, which is
/// 0x3E bytes into the block once converted.
const PROC_ACTION_OFF: usize = 0x3E;

/// ft_* order from MAINEQU.ASM. A trace that does not say who was fighting is
/// one somebody has to annotate by hand later, and nobody does.
const CHAR_NAMES: [&str; 17] = [
    "KungLao", "LiuKang", "Cage", "Baraka", "Kitana", "Mileena", "ShangTsung", "Raiden", "SubZero",
    "Reptile", "Scorpion", "Jax", "Kintaro", "ShaoKahn", "Smoke", "Noob", "Jade",
];

fn char_name(id: u16) -> String {
    CHAR_NAMES
        .get(id as usize)
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("char{id}"))
}

pub struct Trace {
    out: BufWriter<File>,
    path: PathBuf,
    frame: u32,
    rows: u32,
}

fn i16s(v: u16) -> i32 {
    if v >= 0x8000 {
        v as i32 - 0x10000
    } else {
        v as i32
    }
}

/// Follow a fighter's process pointer to its current action id.
/// None when the pointer is null — between rounds, in menus.
fn action_of(core: &Core, proc_ptr_addr: usize) -> Option<u16> {
    let lo = peek_u16(core, proc_ptr_addr, Endian::Little)? as usize;
    let hi = peek_u16(core, proc_ptr_addr + 2, Endian::Little)? as usize;
    let proc = (hi << 16) | lo;
    if proc == 0 {
        return None;
    }
    // The stored value is a raw TMS34010 BIT address. Every constant in
    // mk2_addrs went through (bit / 8) - FBNEO_SYSTEM_RAM_BASE on its way out
    // of the exporter, so a pointer followed at runtime needs the identical
    // conversion. Dividing by 8 alone — which this did first — reads 0x200000
    // bytes past the block and returns noise that looks like a valid id.
    let byte = (proc / 8).checked_sub(addr::FBNEO_SYSTEM_RAM_BASE)?;
    peek_u16(core, byte + PROC_ACTION_OFF, Endian::Little)
}

/// Decode an input word. Bit order matches `ghost::pack`, so a trace lines up
/// with the `.ncgh` of the same session frame for frame.
fn decode(bits: u16) -> String {
    const NAMES: [(u16, &str); 10] = [
        (1 << 4, "U"),
        (1 << 5, "D"),
        (1 << 6, "L"),
        (1 << 7, "R"),
        (1 << 8, "HP"),
        (1 << 9, "LP"),
        (1 << 10, "HK"),
        (1 << 11, "LK"),
        (1 << 12, "BLK"),
        (1 << 3, "START"),
    ];
    let mut s = String::new();
    for (mask, name) in NAMES {
        if bits & mask != 0 {
            if !s.is_empty() {
                s.push('+');
            }
            s.push_str(name);
        }
    }
    if s.is_empty() {
        s.push('-');
    }
    s
}

impl Trace {
    pub fn start(core: &Core) -> std::io::Result<Self> {
        std::fs::create_dir_all("lab_traces")?;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = PathBuf::from("lab_traces").join(format!("trace_{ts}.csv"));
        let mut out = BufWriter::new(File::create(&path)?);
        let g = |a: usize| peek_u16(core, a, Endian::Little).unwrap_or(0);
        writeln!(
            out,
            "# mk2 lab trace  p1={}  p2={}  round={}",
            char_name(g(addr::P1_CHAR_ADDR)),
            char_name(g(addr::P2_CHAR_ADDR)),
            g(addr::ROUND_NUM)
        )?;
        writeln!(
            out,
            "# act = p_action (MAINEQU.ASM): 0303 stance, 0107 uppercut, 0200 flykick, ---- = no proc"
        )?;
        writeln!(
            out,
            "frame,p1_in,p2_in,p1_act,p2_act,p1_hp,p2_hp,p1_x,p2_x,gap,p1_y,p2_y"
        )?;
        println!("[lab] trace started -> {}", path.display());
        Ok(Self {
            out,
            path,
            frame: 0,
            rows: 0,
        })
    }

    /// Once per frame, after the core has run, so the state on a row is the
    /// result of the inputs on that same row.
    pub fn record(&mut self, core: &Core, p1_bits: u16, p2_bits: u16) {
        let g = |a: usize| peek_u16(core, a, Endian::Little).unwrap_or(0);
        let p1x = i16s(g(addr::P1_X_ADDR));
        let p2x = i16s(g(addr::P2_X_ADDR));
        let act = |p: usize| {
            action_of(core, p)
                .map(|v| format!("{v:04x}"))
                .unwrap_or_else(|| "----".into())
        };
        let _ = writeln!(
            self.out,
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            self.frame,
            decode(p1_bits),
            decode(p2_bits),
            act(addr::P1_PROC_ADDR),
            act(addr::P2_PROC_ADDR),
            g(addr::P1_HP_ADDR),
            g(addr::P2_HP_ADDR),
            p1x,
            p2x,
            (p1x - p2x).abs(),
            i16s(g(addr::P1_Y_ADDR)),
            i16s(g(addr::P2_Y_ADDR)),
        );
        self.frame += 1;
        self.rows += 1;
    }

    pub fn finish(mut self) -> PathBuf {
        let _ = self.out.flush();
        println!(
            "[lab] trace saved: {} rows -> {}",
            self.rows,
            self.path.display()
        );
        self.path
    }
}
