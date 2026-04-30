//! Game RAM access for Training Mode and address archaeology.
//!
//! Wraps `retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM)` with typed
//! peek/poke, a frame-level `PokeList` for "every frame write X to Y",
//! and a tiny diff helper for finding unknown addresses.
//!
//! Endianness: FBNeo exposes the 68000's RAM with its native big-endian
//! layout. Peek/poke helpers here take an explicit `Endian` to make the
//! caller's intent obvious — once MK2 addresses are characterized the
//! module can grow a `Word::<BE>` convenience wrapper.

use crate::retro::{Core, RETRO_MEMORY_SYSTEM_RAM};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    #[allow(dead_code)]
    Big,
    Little,
}

/// Read a single byte at `addr`. Returns None if out of bounds or the
/// core doesn't expose system RAM.
#[allow(dead_code)]
pub fn peek_u8(core: &Core, addr: usize) -> Option<u8> {
    let ram = core.memory(RETRO_MEMORY_SYSTEM_RAM)?;
    ram.get(addr).copied()
}

/// Read a 16-bit word at `addr` with the given endianness.
pub fn peek_u16(core: &Core, addr: usize, endian: Endian) -> Option<u16> {
    let ram = core.memory(RETRO_MEMORY_SYSTEM_RAM)?;
    let bytes: [u8; 2] = ram.get(addr..addr + 2)?.try_into().ok()?;
    Some(match endian {
        Endian::Big => u16::from_be_bytes(bytes),
        Endian::Little => u16::from_le_bytes(bytes),
    })
}

/// Write a single byte at `addr`. Silently no-ops if out of bounds so
/// a bad address doesn't panic the emulator thread.
pub fn poke_u8(core: &Core, addr: usize, value: u8) {
    if let Some(ram) = core.memory(RETRO_MEMORY_SYSTEM_RAM) {
        if let Some(slot) = ram.get_mut(addr) {
            *slot = value;
        }
    }
}

/// Write a 16-bit word at `addr` with the given endianness.
pub fn poke_u16(core: &Core, addr: usize, value: u16, endian: Endian) {
    if let Some(ram) = core.memory(RETRO_MEMORY_SYSTEM_RAM) {
        if let Some(slot) = ram.get_mut(addr..addr + 2) {
            let bytes = match endian {
                Endian::Big => value.to_be_bytes(),
                Endian::Little => value.to_le_bytes(),
            };
            slot.copy_from_slice(&bytes);
        }
    }
}

/// A scheduled per-frame RAM write. Used for things like "infinite health":
/// attach a Poke { addr, value, ... } to a PokeList and apply() each frame
/// after retro_run so the game's next read sees the forced value.
#[derive(Clone, Debug)]
pub enum Poke {
    #[allow(dead_code)]
    U8 { addr: usize, value: u8 },
    U16 {
        addr: usize,
        value: u16,
        endian: Endian,
    },
}

impl Poke {
    pub fn apply(&self, core: &Core) {
        match *self {
            Poke::U8 { addr, value } => poke_u8(core, addr, value),
            Poke::U16 {
                addr,
                value,
                endian,
            } => poke_u16(core, addr, value, endian),
        }
    }
}

struct Entry {
    name: String,
    enabled: bool,
    on: Poke,
    /// If Some, fires once when transitioning from enabled to disabled
    /// (e.g. write 0 to a "training mode" flag so the game resumes normal
    /// behavior instead of staying pinned to the on-value). Features where
    /// the game naturally overwrites the address each frame (like health
    /// damage) don't need this — leave None.
    off: Option<Poke>,
    /// Tracks previous `enabled` state so the release poke fires exactly once.
    was_enabled: bool,
}

/// A set of pokes applied once per frame. Enabled/disabled independently
/// so you can toggle infinite health without losing the timer poke, etc.
#[derive(Default)]
pub struct PokeList {
    entries: Vec<Entry>,
}

impl PokeList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, name: impl Into<String>, poke: Poke) {
        self.entries.push(Entry {
            name: name.into(),
            enabled: false,
            on: poke,
            off: None,
            was_enabled: false,
        });
    }

    /// Register a poke with an explicit release value that fires once on disable.
    pub fn add_with_release(&mut self, name: impl Into<String>, on: Poke, off: Poke) {
        self.entries.push(Entry {
            name: name.into(),
            enabled: false,
            on,
            off: Some(off),
            was_enabled: false,
        });
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.name == name) {
            entry.enabled = enabled;
        }
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.enabled)
            .unwrap_or(false)
    }

    pub fn apply(&mut self, core: &Core) {
        for e in &mut self.entries {
            if e.enabled {
                e.on.apply(core);
            } else if e.was_enabled {
                // Transition enabled -> disabled: fire the release poke once.
                if let Some(off) = &e.off {
                    off.apply(core);
                }
            }
            e.was_enabled = e.enabled;
        }
    }
}

/// Snapshot of RAM for address archaeology. Take one before an event,
/// one after, then call `diff` to see which bytes changed.
pub fn snapshot(core: &Core) -> Option<Vec<u8>> {
    core.memory(RETRO_MEMORY_SYSTEM_RAM).map(|r| r.to_vec())
}

/// Return addresses whose value differs between two snapshots.
/// Use for narrowing candidates: take N snapshots before/after the thing
/// you care about, intersect the diff sets.
#[allow(dead_code)]
pub fn diff(before: &[u8], after: &[u8]) -> Vec<usize> {
    let len = before.len().min(after.len());
    let mut out = Vec::new();
    for i in 0..len {
        if before[i] != after[i] {
            out.push(i);
        }
    }
    out
}
