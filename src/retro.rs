//! Libretro FFI layer: core loading, callbacks, and the shared static
//! framebuffer/input state that the C callbacks write into.
//!
//! Static muts are used because libretro's C callbacks are plain function
//! pointers with no user-data arg — they need somewhere global to put frame
//! data and read input from. The whole thing is single-threaded.
#![allow(static_mut_refs)]

use libloading::{Library, Symbol};
use std::ffi::{c_char, c_void, CString};
use std::ptr;

// --- Libretro environment command IDs ---
pub const RETRO_ENVIRONMENT_SET_MESSAGE: u32 = 6;
pub const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: u32 = 10;
pub const RETRO_ENVIRONMENT_GET_VARIABLE: u32 = 15;
pub const RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS: u32 = 19;
pub const RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION: u32 = 52;

// --- Memory region IDs for retro_get_memory_data/size ---
#[allow(dead_code)]
pub const RETRO_MEMORY_SAVE_RAM: u32 = 0;
#[allow(dead_code)]
pub const RETRO_MEMORY_RTC: u32 = 1;
pub const RETRO_MEMORY_SYSTEM_RAM: u32 = 2;
#[allow(dead_code)]
pub const RETRO_MEMORY_VIDEO_RAM: u32 = 3;

// --- Pixel formats ---
pub const RETRO_PIXEL_FORMAT_0RGB1555: u32 = 0;
pub const RETRO_PIXEL_FORMAT_XRGB8888: u32 = 1;
pub const RETRO_PIXEL_FORMAT_RGB565: u32 = 2;

// --- Device IDs (RetroPad mapping, used as MK2 action slots) ---
pub const RETRO_DEVICE_JOYPAD: u32 = 1;
// Libretro retropad slot IDs. FBNeo's mk2 driver maps each slot to a specific
// MK2 action — see comments. The slot *numbers* are libretro-defined constants
// and do not change. What we pick is which MK2 Action writes into which slot.
pub const RETRO_DEVICE_ID_JOYPAD_B: u32 = 0; // FBNeo mk2: High Punch
pub const RETRO_DEVICE_ID_JOYPAD_Y: u32 = 1; // FBNeo mk2: Block
pub const RETRO_DEVICE_ID_JOYPAD_SELECT: u32 = 2; // FBNeo mk2: Coin
pub const RETRO_DEVICE_ID_JOYPAD_START: u32 = 3; // FBNeo mk2: Start
pub const RETRO_DEVICE_ID_JOYPAD_UP: u32 = 4;
pub const RETRO_DEVICE_ID_JOYPAD_DOWN: u32 = 5;
pub const RETRO_DEVICE_ID_JOYPAD_LEFT: u32 = 6;
pub const RETRO_DEVICE_ID_JOYPAD_RIGHT: u32 = 7;
pub const RETRO_DEVICE_ID_JOYPAD_A: u32 = 8; // FBNeo mk2: Low Punch
pub const RETRO_DEVICE_ID_JOYPAD_X: u32 = 9; // FBNeo mk2: High Kick
pub const RETRO_DEVICE_ID_JOYPAD_L: u32 = 10; // FBNeo mk2: Low Kick
#[allow(dead_code)]
pub const RETRO_DEVICE_ID_JOYPAD_R: u32 = 11; // (unused: Run in some MK titles)

// --- Shared state populated by C callbacks ---
pub static mut FRAME_BUFFER: Vec<u8> = Vec::new();
pub static mut FRAME_WIDTH: u32 = 0;
pub static mut FRAME_HEIGHT: u32 = 0;
pub static mut FRAME_PITCH: usize = 0;
pub static mut PIXEL_FORMAT: u32 = RETRO_PIXEL_FORMAT_0RGB1555;
/// Per-port input state read by `input_state_cb` (what FBNeo sees).
/// [0]=P1, [1]=P2. Bit slots mirror RETRO_DEVICE_ID_JOYPAD_*.
///
/// During netplay this holds whatever ggrs's AdvanceFrame supplied, NOT the
/// live user input — see LIVE_INPUT for that. During local play main.rs
/// copies LIVE_INPUT into INPUT_STATE each frame.
pub static mut INPUT_STATE: [[bool; 16]; 2] = [[false; 16]; 2];

/// Live pad/keyboard state, written by SDL event handlers as buttons are
/// pressed/released. This is the source of truth for "what is the user
/// currently holding". Netplay code takes a snapshot of this and sends it
/// to ggrs; local-play code copies it into INPUT_STATE each frame.
pub static mut LIVE_INPUT: [[bool; 16]; 2] = [[false; 16]; 2];

/// Audio samples produced by the core during the most recent `retro_run`.
/// Interleaved stereo s16. Main loop drains this into the SDL audio queue
/// each frame and then clears it.
pub static mut AUDIO_BUFFER: Vec<i16> = Vec::new();

/// When true, video_refresh_cb / audio callbacks discard data. Used during
/// rollback resim frames where we only want to advance state, not present it.
pub static mut SILENT_MODE: bool = false;

#[repr(C)]
pub struct GameInfo {
    pub path: *const c_char,
    pub data: *const c_void,
    pub size: usize,
    pub meta: *const c_char,
}

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct SystemTiming {
    pub fps: f64,
    pub sample_rate: f64,
}

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct GameGeometry {
    pub base_width: u32,
    pub base_height: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub aspect_ratio: f32,
}

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct SystemAvInfo {
    pub geometry: GameGeometry,
    pub timing: SystemTiming,
}

#[repr(C)]
struct RetroVariable {
    key: *const c_char,
    value: *const c_char,
}

#[repr(C)]
struct RetroMessage {
    msg: *const c_char,
    frames: u32,
}

#[repr(C)]
struct RetroInputDescriptor {
    port: u32,
    device: u32,
    index: u32,
    id: u32,
    description: *const c_char,
}

extern "C" fn environment_cb(cmd: u32, data: *mut c_void) -> bool {
    unsafe {
        match cmd {
            RETRO_ENVIRONMENT_SET_PIXEL_FORMAT => {
                let format = *(data as *const u32);
                match format {
                    RETRO_PIXEL_FORMAT_0RGB1555
                    | RETRO_PIXEL_FORMAT_XRGB8888
                    | RETRO_PIXEL_FORMAT_RGB565 => {
                        PIXEL_FORMAT = format;
                        true
                    }
                    _ => false,
                }
            }
            RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION => {
                *(data as *mut u32) = 1;
                true
            }
            RETRO_ENVIRONMENT_GET_VARIABLE => {
                let var = &mut *(data as *mut RetroVariable);
                if !var.key.is_null() {
                    let key_c_str = std::ffi::CStr::from_ptr(var.key);
                    if let Ok(key_str) = key_c_str.to_str() {
                        if key_str == "fbneo-allow-patched-romsets" {
                            var.value = b"enabled\0".as_ptr() as *const c_char;
                            return true;
                        }
                    }
                }
                false
            }
            RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS => {
                // FBNeo sends an array of descriptors terminated by { description = NULL }.
                // Log each one so we learn which retropad slot drives which MK2 action.
                let mut ptr = data as *const RetroInputDescriptor;
                loop {
                    let desc = &*ptr;
                    if desc.description.is_null() {
                        break;
                    }
                    if let Ok(label) = std::ffi::CStr::from_ptr(desc.description).to_str() {
                        println!(
                            "[input-desc] port={} device={} idx={} id={} -> {}",
                            desc.port, desc.device, desc.index, desc.id, label
                        );
                    }
                    ptr = ptr.add(1);
                }
                true
            }
            RETRO_ENVIRONMENT_SET_MESSAGE => {
                let msg_struct = &*(data as *const RetroMessage);
                if !msg_struct.msg.is_null() {
                    let msg_c_str = std::ffi::CStr::from_ptr(msg_struct.msg);
                    if let Ok(msg_str) = msg_c_str.to_str() {
                        println!("CORE MESSAGE: {}", msg_str);
                    }
                }
                true
            }
            _ => false,
        }
    }
}

extern "C" fn video_refresh_cb(data: *const c_void, width: u32, height: u32, pitch: usize) {
    unsafe {
        if SILENT_MODE {
            return;
        }
        if !data.is_null() {
            let size = (height as usize) * pitch;
            if FRAME_BUFFER.len() < size {
                FRAME_BUFFER.resize(size, 0);
            }
            let src = std::slice::from_raw_parts(data as *const u8, size);
            FRAME_BUFFER[..size].copy_from_slice(src);
            FRAME_WIDTH = width;
            FRAME_HEIGHT = height;
            FRAME_PITCH = pitch;
        }
    }
}

extern "C" fn audio_sample_cb(left: i16, right: i16) {
    unsafe {
        if SILENT_MODE {
            return;
        }
        AUDIO_BUFFER.push(left);
        AUDIO_BUFFER.push(right);
    }
}
extern "C" fn audio_sample_batch_cb(data: *const i16, frames: usize) -> usize {
    unsafe {
        if SILENT_MODE {
            return frames;
        }
        if !data.is_null() && frames > 0 {
            let samples = std::slice::from_raw_parts(data, frames * 2);
            AUDIO_BUFFER.extend_from_slice(samples);
        }
    }
    frames
}

extern "C" fn input_poll_cb() {}

extern "C" fn input_state_cb(port: u32, device: u32, index: u32, id: u32) -> i16 {
    unsafe {
        if device == RETRO_DEVICE_JOYPAD
            && index == 0
            && port < 2
            && id < 16
            && INPUT_STATE[port as usize][id as usize]
        {
            return 1;
        }
    }
    0
}

/// Loaded core handle. Holds the `Library` so it stays alive, plus the
/// function pointers we actually call from the main loop.
pub struct Core {
    _lib: Library, // kept alive so the other symbols stay valid
    pub run: unsafe extern "C" fn(),
    pub av_info: SystemAvInfo,
    serialize_size_fn: unsafe extern "C" fn() -> usize,
    serialize_fn: unsafe extern "C" fn(*mut c_void, usize) -> bool,
    unserialize_fn: unsafe extern "C" fn(*const c_void, usize) -> bool,
    get_memory_data_fn: unsafe extern "C" fn(u32) -> *mut c_void,
    get_memory_size_fn: unsafe extern "C" fn(u32) -> usize,
}

impl Core {
    /// Maximum serialized state size in bytes (upper bound; actual writes fit in this).
    pub fn serialize_size(&self) -> usize {
        unsafe { (self.serialize_size_fn)() }
    }

    /// Snapshot the current emulation state into a fresh Vec.
    /// Returns None if the core refuses to serialize.
    pub fn save_state(&self) -> Option<Vec<u8>> {
        let size = self.serialize_size();
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size];
        let ok = unsafe { (self.serialize_fn)(buf.as_mut_ptr() as *mut c_void, size) };
        if ok {
            Some(buf)
        } else {
            None
        }
    }

    /// Restore a previously-saved state. Returns false if the core rejects it
    /// (size mismatch after a core upgrade, corrupt data, etc).
    pub fn load_state(&self, data: &[u8]) -> bool {
        unsafe { (self.unserialize_fn)(data.as_ptr() as *const c_void, data.len()) }
    }

    /// Borrow a libretro memory region (e.g. `RETRO_MEMORY_SYSTEM_RAM`).
    /// Returns None if the core doesn't expose that region.
    ///
    /// The slice is valid for the lifetime of the core — the pointer is
    /// core-owned and stable across frames.
    pub fn memory(&self, region: u32) -> Option<&mut [u8]> {
        let size = unsafe { (self.get_memory_size_fn)(region) };
        if size == 0 {
            return None;
        }
        let ptr = unsafe { (self.get_memory_data_fn)(region) };
        if ptr.is_null() {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts_mut(ptr as *mut u8, size) })
    }
}

/// Load fbneo_libretro.dll, wire callbacks, init the core, and load the ROM.
/// Returns a `Core` whose `run` is callable once per frame.
pub unsafe fn load(dll_path: &str, rom_path: &str) -> Result<Core, Box<dyn std::error::Error>> {
    println!("Loading FBNeo Libretro Core...");
    let lib = Library::new(dll_path)?;

    let retro_set_environment: Symbol<
        unsafe extern "C" fn(extern "C" fn(u32, *mut c_void) -> bool),
    > = lib.get(b"retro_set_environment\0")?;
    let retro_init: Symbol<unsafe extern "C" fn()> = lib.get(b"retro_init\0")?;
    let retro_set_video_refresh: Symbol<
        unsafe extern "C" fn(extern "C" fn(*const c_void, u32, u32, usize)),
    > = lib.get(b"retro_set_video_refresh\0")?;
    let retro_set_audio_sample: Symbol<unsafe extern "C" fn(extern "C" fn(i16, i16))> =
        lib.get(b"retro_set_audio_sample\0")?;
    let retro_set_audio_sample_batch: Symbol<
        unsafe extern "C" fn(extern "C" fn(*const i16, usize) -> usize),
    > = lib.get(b"retro_set_audio_sample_batch\0")?;
    let retro_set_input_poll: Symbol<unsafe extern "C" fn(extern "C" fn())> =
        lib.get(b"retro_set_input_poll\0")?;
    let retro_set_input_state: Symbol<
        unsafe extern "C" fn(extern "C" fn(u32, u32, u32, u32) -> i16),
    > = lib.get(b"retro_set_input_state\0")?;
    let retro_load_game: Symbol<unsafe extern "C" fn(*const GameInfo) -> bool> =
        lib.get(b"retro_load_game\0")?;
    let retro_run: Symbol<unsafe extern "C" fn()> = lib.get(b"retro_run\0")?;
    let retro_get_system_av_info: Symbol<unsafe extern "C" fn(*mut SystemAvInfo)> =
        lib.get(b"retro_get_system_av_info\0")?;
    let retro_serialize_size: Symbol<unsafe extern "C" fn() -> usize> =
        lib.get(b"retro_serialize_size\0")?;
    let retro_serialize: Symbol<unsafe extern "C" fn(*mut c_void, usize) -> bool> =
        lib.get(b"retro_serialize\0")?;
    let retro_unserialize: Symbol<unsafe extern "C" fn(*const c_void, usize) -> bool> =
        lib.get(b"retro_unserialize\0")?;
    let retro_get_memory_data: Symbol<unsafe extern "C" fn(u32) -> *mut c_void> =
        lib.get(b"retro_get_memory_data\0")?;
    let retro_get_memory_size: Symbol<unsafe extern "C" fn(u32) -> usize> =
        lib.get(b"retro_get_memory_size\0")?;

    retro_set_environment(environment_cb);
    retro_set_video_refresh(video_refresh_cb);
    retro_set_audio_sample(audio_sample_cb);
    retro_set_audio_sample_batch(audio_sample_batch_cb);
    retro_set_input_poll(input_poll_cb);
    retro_set_input_state(input_state_cb);

    retro_init();

    println!("Loading {rom_path}...");
    let rom_c = CString::new(rom_path)?;
    let game_info = GameInfo {
        path: rom_c.as_ptr(),
        data: ptr::null(),
        size: 0,
        meta: ptr::null(),
    };
    if !retro_load_game(&game_info) {
        println!("WARNING: retro_load_game returned false (CRC mismatch?); booting anyway.");
    }
    println!("ROM Loaded. Booting System...");

    // Query native AV info now that a game is loaded (per libretro spec,
    // it must be called after load_game, not before).
    let mut av_info = SystemAvInfo::default();
    retro_get_system_av_info(&mut av_info);
    println!(
        "Core AV: {:.3} fps, {:.0} Hz audio, {}x{}",
        av_info.timing.fps,
        av_info.timing.sample_rate,
        av_info.geometry.base_width,
        av_info.geometry.base_height
    );

    // Capture function pointers before their `Symbol` guards drop.
    let run_fn: unsafe extern "C" fn() = *retro_run;
    let ss_fn: unsafe extern "C" fn() -> usize = *retro_serialize_size;
    let s_fn: unsafe extern "C" fn(*mut c_void, usize) -> bool = *retro_serialize;
    let u_fn: unsafe extern "C" fn(*const c_void, usize) -> bool = *retro_unserialize;
    let gmd_fn: unsafe extern "C" fn(u32) -> *mut c_void = *retro_get_memory_data;
    let gms_fn: unsafe extern "C" fn(u32) -> usize = *retro_get_memory_size;

    let state_size = ss_fn();
    println!("Save-state size: {} bytes", state_size);
    let sysram_size = gms_fn(RETRO_MEMORY_SYSTEM_RAM);
    println!("System RAM size: {} bytes", sysram_size);

    Ok(Core {
        _lib: lib,
        run: run_fn,
        av_info,
        serialize_size_fn: ss_fn,
        serialize_fn: s_fn,
        unserialize_fn: u_fn,
        get_memory_data_fn: gmd_fn,
        get_memory_size_fn: gms_fn,
    })
}
