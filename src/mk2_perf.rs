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
    pub foreground_objects: [usize; 3],
    pub background_objects: [usize; 8],
    pub free_objects: usize,
    pub overload: u16,
    pub list_warnings: ListWarnings,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ListWarnings {
    pub active: bool,
    pub free_processes: bool,
    pub foreground: [bool; 3],
    pub background: [bool; 8],
    pub free_objects: bool,
}

impl ListWarnings {
    fn any(self) -> bool {
        self.active
            || self.free_processes
            || self.free_objects
            || self.foreground.iter().any(|v| *v)
            || self.background.iter().any(|v| *v)
    }

    fn labels(self) -> Vec<&'static str> {
        let mut labels = Vec::new();
        if self.active {
            labels.push("ACTIVE");
        }
        if self.free_processes {
            labels.push("PFREE");
        }
        for (i, warn) in self.foreground.iter().enumerate() {
            if *warn {
                labels.push(["OBJ1", "OBJ2", "OBJ3"][i]);
            }
        }
        for (i, warn) in self.background.iter().enumerate() {
            if *warn {
                labels.push(
                    [
                        "BAK1", "BAK2", "BAK3", "BAK4", "BAK5", "BAK6", "BAK7", "BAK8",
                    ][i],
                );
            }
        }
        if self.free_objects {
            labels.push("OFREE");
        }
        labels
    }
}

impl Mk2PerfSample {
    pub fn detail_rows(self) -> Vec<String> {
        let foreground_total: usize = self.foreground_objects.iter().sum();
        let background_total: usize = self.background_objects.iter().sum();
        let mut rows = vec![
            format!(
                "MK2 PROC A{} F{}",
                self.active_processes, self.free_processes
            ),
            format!(
                "MK2 OBJ FG{} BG{} F{}",
                foreground_total, background_total, self.free_objects
            ),
        ];
        if self.overload != 0 {
            rows.push(format!("MK2 OVER {}", self.overload));
        }
        if self.list_warnings.any() {
            rows.push(format!(
                "MK2 LIST {}",
                self.list_warnings.labels().join(",")
            ));
        }
        rows
    }
}

pub fn sample(core: &Core) -> Option<Mk2PerfSample> {
    let ram = core.memory(RETRO_MEMORY_SYSTEM_RAM)?;
    let (active_processes, active_warn) = count_list(ram, mk2_addrs::ACTIVE_PROCESS_LIST_ADDR);
    let (free_processes, free_proc_warn) = count_list(ram, mk2_addrs::FREE_PROCESS_LIST_ADDR);
    let (obj1, obj1_warn) = count_list(ram, mk2_addrs::OBJECT_LIST_1_ADDR);
    let (obj2, obj2_warn) = count_list(ram, mk2_addrs::OBJECT_LIST_2_ADDR);
    let (obj3, obj3_warn) = count_list(ram, mk2_addrs::OBJECT_LIST_3_ADDR);
    let (bak1, bak1_warn) = count_list(ram, mk2_addrs::BACKGROUND_LIST_1_ADDR);
    let (bak2, bak2_warn) = count_list(ram, mk2_addrs::BACKGROUND_LIST_2_ADDR);
    let (bak3, bak3_warn) = count_list(ram, mk2_addrs::BACKGROUND_LIST_3_ADDR);
    let (bak4, bak4_warn) = count_list(ram, mk2_addrs::BACKGROUND_LIST_4_ADDR);
    let (bak5, bak5_warn) = count_list(ram, mk2_addrs::BACKGROUND_LIST_5_ADDR);
    let (bak6, bak6_warn) = count_list(ram, mk2_addrs::BACKGROUND_LIST_6_ADDR);
    let (bak7, bak7_warn) = count_list(ram, mk2_addrs::BACKGROUND_LIST_7_ADDR);
    let (bak8, bak8_warn) = count_list(ram, mk2_addrs::BACKGROUND_LIST_8_ADDR);
    let (free_objects, free_obj_warn) = count_list(ram, mk2_addrs::FREE_OBJECT_LIST_ADDR);

    Some(Mk2PerfSample {
        active_processes,
        free_processes,
        foreground_objects: [obj1, obj2, obj3],
        background_objects: [bak1, bak2, bak3, bak4, bak5, bak6, bak7, bak8],
        free_objects,
        overload: peek_u16(ram, mk2_addrs::OVERLOAD_ADDR).unwrap_or(0),
        list_warnings: ListWarnings {
            active: active_warn,
            free_processes: free_proc_warn,
            foreground: [obj1_warn, obj2_warn, obj3_warn],
            background: [
                bak1_warn, bak2_warn, bak3_warn, bak4_warn, bak5_warn, bak6_warn, bak7_warn,
                bak8_warn,
            ],
            free_objects: free_obj_warn,
        },
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
