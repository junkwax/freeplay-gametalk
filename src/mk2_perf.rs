//! Read-only MK2 runtime pressure sampling for the Freeplay overlay.
//!
//! MK2 stores process/object links as TMS34010 bit addresses. FBNeo exposes
//! game RAM as byte offsets from RETRO_MEMORY_SYSTEM_RAM, so list walking has
//! to translate pointer values before following the next link.

use crate::mk2_addrs;
use crate::retro::{Core, RETRO_MEMORY_SYSTEM_RAM};

const FBNEO_SYSTEM_RAM_BASE: usize = 0x20_0000;
const MAX_LINKS: usize = 512;

#[derive(Clone, Copy, Debug, Default)]
pub struct Mk2PerfSample {
    pub active_processes: usize,
    pub free_processes: usize,
    pub object_list_1: usize,
    pub object_list_2: usize,
    pub object_list_3: usize,
    pub free_objects: usize,
    pub overload: u16,
    pub truncated: bool,
}

impl Mk2PerfSample {
    pub fn detail_rows(self) -> Vec<String> {
        let mut rows = vec![
            format!(
                "MK2 PROC A{} F{}",
                self.active_processes, self.free_processes
            ),
            format!(
                "MK2 OBJ {}+{}+{} F{}",
                self.object_list_1, self.object_list_2, self.object_list_3, self.free_objects
            ),
        ];
        if self.overload != 0 {
            rows.push(format!("MK2 OVER {}", self.overload));
        }
        if self.truncated {
            rows.push("MK2 LIST CHECK".to_string());
        }
        rows
    }
}

pub fn sample(core: &Core) -> Option<Mk2PerfSample> {
    let ram = core.memory(RETRO_MEMORY_SYSTEM_RAM)?;
    let (active_processes, active_truncated) = count_list(ram, mk2_addrs::ACTIVE_PROCESS_LIST_ADDR);
    let (free_processes, free_proc_truncated) = count_list(ram, mk2_addrs::FREE_PROCESS_LIST_ADDR);
    let (object_list_1, obj1_truncated) = count_list(ram, mk2_addrs::OBJECT_LIST_1_ADDR);
    let (object_list_2, obj2_truncated) = count_list(ram, mk2_addrs::OBJECT_LIST_2_ADDR);
    let (object_list_3, obj3_truncated) = count_list(ram, mk2_addrs::OBJECT_LIST_3_ADDR);
    let (free_objects, free_obj_truncated) = count_list(ram, mk2_addrs::FREE_OBJECT_LIST_ADDR);

    Some(Mk2PerfSample {
        active_processes,
        free_processes,
        object_list_1,
        object_list_2,
        object_list_3,
        free_objects,
        overload: peek_u16(ram, mk2_addrs::OVERLOAD_ADDR).unwrap_or(0),
        truncated: active_truncated
            || free_proc_truncated
            || obj1_truncated
            || obj2_truncated
            || obj3_truncated
            || free_obj_truncated,
    })
}

fn count_list(ram: &[u8], head_addr: usize) -> (usize, bool) {
    let Some(mut ptr) = peek_u32(ram, head_addr) else {
        return (0, true);
    };
    let mut count = 0usize;
    let mut seen = Vec::new();

    while ptr != 0 {
        if count >= MAX_LINKS {
            return (count, true);
        }
        let Some(offset) = tms_bit_ptr_to_offset(ptr, ram.len()) else {
            return (count, true);
        };
        if seen.contains(&offset) {
            return (count, true);
        }
        seen.push(offset);
        count += 1;
        let Some(next) = peek_u32(ram, offset) else {
            return (count, true);
        };
        ptr = next;
    }

    (count, false)
}

fn tms_bit_ptr_to_offset(ptr: u32, ram_len: usize) -> Option<usize> {
    if ptr & 0x7 != 0 {
        return None;
    }
    let byte_addr = ptr as usize / 8;
    let offset = byte_addr.checked_sub(FBNEO_SYSTEM_RAM_BASE)?;
    if offset + 4 <= ram_len {
        Some(offset)
    } else {
        None
    }
}

fn peek_u16(ram: &[u8], addr: usize) -> Option<u16> {
    let bytes: [u8; 2] = ram.get(addr..addr + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(bytes))
}

fn peek_u32(ram: &[u8], addr: usize) -> Option<u32> {
    let bytes: [u8; 4] = ram.get(addr..addr + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}
