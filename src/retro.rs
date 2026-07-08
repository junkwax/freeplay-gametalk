//! Libretro FFI layer: core loading, callbacks, and the shared
//! framebuffer/input/audio state that the C callbacks write into.
//!
//! Libretro callbacks are plain C function pointers with no user-data arg,
//! so the state they touch must be global. Each domain (frame, audio,
//! input, silent flag) lives behind its own small lock so a callback only
//! ever takes one lock and call sites can't deadlock by nesting domains.
//! This also makes the layer safe if emulation later moves off the main
//! thread. One rule for callers: never call `core.run` (or anything else
//! that re-enters the core) from inside a `with_*` closure — the core's
//! callbacks take these same locks.

use libloading::{Library, Symbol};
use std::ffi::{c_char, c_void, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};

// --- Libretro environment command IDs ---
pub const RETRO_ENVIRONMENT_SET_MESSAGE: u32 = 6;
pub const RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY: u32 = 9;
pub const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: u32 = 10;
pub const RETRO_ENVIRONMENT_GET_VARIABLE: u32 = 15;
pub const RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS: u32 = 11;
pub const RETRO_ENVIRONMENT_GET_LIBRETRO_PATH: u32 = 19;
pub const RETRO_ENVIRONMENT_GET_CONTENT_DIRECTORY: u32 = 30;
pub const RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY: u32 = 31;
pub const RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION: u32 = 52;
pub const RETRO_ENVIRONMENT_EXPERIMENTAL: u32 = 0x10000;
/// Core asks each frame whether it should bother producing video/audio.
/// Answering this is how the frontend makes rollback resim frames cheap:
/// with the video bit cleared FBNeo sets pBurnDraw=NULL and drivers skip
/// rendering entirely; with the audio-enable bit cleared it still emulates
/// sound (required for determinism) but skips mixing/presenting it.
pub const RETRO_ENVIRONMENT_GET_AUDIO_VIDEO_ENABLE: u32 = 47 | RETRO_ENVIRONMENT_EXPERIMENTAL;
/// Core asks what savestates will be used for. Answering ROLLBACK_NETPLAY
/// makes FBNeo use its netplay-optimized scan (ACB_NET_OPT), keep hiscores
/// disabled (hiscore.dat memory writes are a known netplay desync source),
/// and set its internal kNetGame determinism flag.
pub const RETRO_ENVIRONMENT_GET_SAVESTATE_CONTEXT: u32 = 72 | RETRO_ENVIRONMENT_EXPERIMENTAL;
pub const RETRO_SAVESTATE_CONTEXT_ROLLBACK_NETPLAY: u32 = 3;

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

/// Video frame most recently presented by the core, plus its geometry and
/// the pixel format negotiated via `SET_PIXEL_FORMAT`.
pub struct FrameState {
    pub buf: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pitch: usize,
    pub pixel_format: u32,
}

static FRAME: Mutex<FrameState> = Mutex::new(FrameState {
    buf: Vec::new(),
    width: 0,
    height: 0,
    pitch: 0,
    pixel_format: RETRO_PIXEL_FORMAT_0RGB1555,
});

/// Run `f` with the current frame locked. Keep the closure short and never
/// call back into the core from inside it.
pub fn with_frame<R>(f: impl FnOnce(&FrameState) -> R) -> R {
    f(&FRAME.lock().expect("frame lock poisoned"))
}

/// Per-port pad state.
/// `state` is what FBNeo sees through `input_state_cb`: during netplay it
/// holds whatever ggrs's AdvanceFrame supplied, NOT the live user input.
/// `live` is the raw pad/keyboard state written by SDL event handlers — the
/// source of truth for "what is the user currently holding". Netplay
/// snapshots `live` and sends it to ggrs; local play commits `live` into
/// `state` each frame.
#[derive(Default)]
struct Pads {
    state: [[bool; 16]; 2],
    live: [[bool; 16]; 2],
}

static PADS: Mutex<Pads> = Mutex::new(Pads {
    state: [[false; 16]; 2],
    live: [[false; 16]; 2],
});

/// Snapshot the libretro-visible input state (what FBNeo sees).
pub fn input_state_snapshot() -> [[bool; 16]; 2] {
    PADS.lock().expect("pads lock poisoned").state
}

/// Set one libretro-visible input slot.
pub fn set_input(port: usize, id: usize, pressed: bool) {
    if port < 2 && id < 16 {
        PADS.lock().expect("pads lock poisoned").state[port][id] = pressed;
    }
}

/// Overwrite one port's full libretro-visible input row.
pub fn set_input_port(port: usize, row: [bool; 16]) {
    if port < 2 {
        PADS.lock().expect("pads lock poisoned").state[port] = row;
    }
}

/// Overwrite the whole libretro-visible input state.
pub fn set_input_all(state: [[bool; 16]; 2]) {
    PADS.lock().expect("pads lock poisoned").state = state;
}

/// Set one live (user-held) input slot. Written by SDL event handlers.
pub fn set_live_input(port: usize, id: usize, pressed: bool) {
    if port < 2 && id < 16 {
        PADS.lock().expect("pads lock poisoned").live[port][id] = pressed;
    }
}

/// Read one live (user-held) input slot.
pub fn live_input(port: usize, id: usize) -> bool {
    port < 2 && id < 16 && PADS.lock().expect("pads lock poisoned").live[port][id]
}

/// Snapshot one port's live row.
pub fn live_input_port(port: usize) -> [bool; 16] {
    if port < 2 {
        PADS.lock().expect("pads lock poisoned").live[port]
    } else {
        [false; 16]
    }
}

/// Copy live input into the libretro-visible state for both players.
/// Used by local (non-netplay) play each frame.
pub fn commit_live_to_state() {
    let mut pads = PADS.lock().expect("pads lock poisoned");
    pads.state = pads.live;
}

/// Zero both live and libretro-visible input for both players.
pub fn clear_all_inputs() {
    let mut pads = PADS.lock().expect("pads lock poisoned");
    pads.state = [[false; 16]; 2];
    pads.live = [[false; 16]; 2];
}

/// Audio samples produced by the core during the most recent `retro_run`.
/// Interleaved stereo s16. The main loop drains this into the SDL audio
/// queue each frame.
static AUDIO: Mutex<Vec<i16>> = Mutex::new(Vec::new());

pub fn clear_audio_buffer() {
    AUDIO.lock().expect("audio lock poisoned").clear();
}

pub fn drain_audio_buffer() -> Vec<i16> {
    std::mem::take(&mut *AUDIO.lock().expect("audio lock poisoned"))
}

/// Run `f` with the pending audio locked (e.g. to filter in place and hand
/// it to a recorder before clearing). Never call `core.run` inside.
pub fn with_audio_mut<R>(f: impl FnOnce(&mut Vec<i16>) -> R) -> R {
    f(&mut AUDIO.lock().expect("audio lock poisoned"))
}

/// What the core should present this frame. Backed by an atomic u8.
///
/// - `Normal`: video and audio both emulated and presented.
/// - `Silent`: neither presented (rollback resim frames, fast-forward);
///   FBNeo also skips rendering entirely via the env answer.
/// - `VideoOnly`: video presented, audio emulated but NOT presented. Used
///   for speculative runahead frames layered on top of netplay: the shown
///   frame is one ahead of canonical, but the audio timeline stays
///   canonical so the SDL queue never receives duplicate samples.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AvMode {
    Normal = 0,
    Silent = 1,
    VideoOnly = 2,
}

static AV_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_av_mode(mode: AvMode) {
    AV_MODE.store(mode as u8, Ordering::Relaxed);
}

fn av_mode() -> AvMode {
    match AV_MODE.load(Ordering::Relaxed) {
        1 => AvMode::Silent,
        2 => AvMode::VideoOnly,
        _ => AvMode::Normal,
    }
}

/// True when video should be produced/presented this frame.
pub fn video_enabled() -> bool {
    av_mode() != AvMode::Silent
}

/// True when audio should be presented this frame (it is always emulated).
pub fn audio_enabled() -> bool {
    av_mode() == AvMode::Normal
}

/// Diagnostic kill switch (env `FREEPLAY_NO_AV_SKIP=1`): make the
/// GET_AUDIO_VIDEO_ENABLE answer always report video+audio enabled,
/// regardless of `AvMode`. This is an A/B lever for the desync
/// investigation — it never lets FBNeo see the video-disabled bit clear, so
/// `pBurnDraw` is never NULLed on resim/fast-forward frames. If desyncs stop
/// reproducing with this set, the T-Unit driver's null-draw path is
/// implicated. Debug-only: forcing audio presentation on resim frames will
/// produce audible glitches, which is expected and irrelevant to the test.
fn no_av_skip() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| crate::config::env_value("FREEPLAY_NO_AV_SKIP").is_some())
}

/// Compatibility wrappers: most call sites only toggle between fully
/// presenting and fully silent.
pub fn set_silent(on: bool) {
    set_av_mode(if on { AvMode::Silent } else { AvMode::Normal });
}

pub fn silent() -> bool {
    av_mode() == AvMode::Silent
}

static LIBRETRO_PATH: OnceLock<CString> = OnceLock::new();
static SYSTEM_DIRECTORY: OnceLock<CString> = OnceLock::new();
static CONTENT_DIRECTORY: OnceLock<CString> = OnceLock::new();
static SAVE_DIRECTORY: OnceLock<CString> = OnceLock::new();

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

fn cstring_from_path(path: PathBuf) -> CString {
    CString::new(path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| CString::new(".").expect("literal has no NUL"))
}

fn fallback_directory() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn set_env_path(cell: &'static OnceLock<CString>, path: PathBuf) {
    let _ = cell.set(cstring_from_path(path));
}

fn configure_environment_paths(dll_path: &str, rom_path: &str) {
    let fallback = fallback_directory();
    let content_dir = Path::new(rom_path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| fallback.clone());

    set_env_path(&LIBRETRO_PATH, PathBuf::from(dll_path));
    set_env_path(&SYSTEM_DIRECTORY, fallback.clone());
    set_env_path(&CONTENT_DIRECTORY, content_dir);
    set_env_path(&SAVE_DIRECTORY, fallback);
}

unsafe fn write_env_path(cell: &'static OnceLock<CString>, data: *mut c_void, label: &str) -> bool {
    if data.is_null() {
        return false;
    }
    let Some(path) = cell.get() else {
        return false;
    };
    *(data as *mut *const c_char) = path.as_ptr();
    crate::dlog!("retro", "env {label}={}", path.as_c_str().to_string_lossy());
    true
}

extern "C" fn environment_cb(cmd: u32, data: *mut c_void) -> bool {
    unsafe {
        match cmd {
            RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY => {
                write_env_path(&SYSTEM_DIRECTORY, data, "system_directory")
            }
            RETRO_ENVIRONMENT_GET_AUDIO_VIDEO_ENABLE => {
                // Bit 0: enable video. Cleared during silent (resim/fast-
                //        forward) frames -> FBNeo sets pBurnDraw=NULL and the
                //        driver skips all rendering for that frame.
                // Bit 1: enable audio (present to frontend). Cleared during
                //        silent frames; FBNeo still *emulates* sound, which
                //        is required for determinism — the remote peer ran
                //        this frame with sound emulated, so skipping the
                //        sound CPU here would diverge the state.
                // Bit 2: netplay context hint for FBNeo's fallback paths.
                // Bit 3: hard-disable audio. NEVER set — it skips sound
                //        emulation entirely, which is only safe for the
                //        throwaway instance of 2-instance runahead.
                if !data.is_null() {
                    let mut flags: i32 = 0b100;
                    if video_enabled() || no_av_skip() {
                        flags |= 0b001;
                    }
                    if audio_enabled() || no_av_skip() {
                        flags |= 0b010;
                    }
                    *(data as *mut i32) = flags;
                }
                true
            }
            RETRO_ENVIRONMENT_GET_SAVESTATE_CONTEXT => {
                // Always report the rollback-netplay context, even for local
                // play. Two reasons: (1) serialize and unserialize layouts
                // must agree, and states cross the local/netplay boundary
                // (clean-boot reload, replays), so the context must never
                // change at runtime; (2) this exact layout (ACB_NET_OPT,
                // hiscores off, kNetGame=1) is what the core already used
                // when this query went unanswered — FBNeo's fallback reads
                // an untouched -1 sentinel from GET_AUDIO_VIDEO_ENABLE and
                // lands on the netplay path — so existing savestates and
                // replays keep their layout.
                if !data.is_null() {
                    *(data as *mut u32) = RETRO_SAVESTATE_CONTEXT_ROLLBACK_NETPLAY;
                }
                true
            }
            RETRO_ENVIRONMENT_SET_PIXEL_FORMAT => {
                let format = *(data as *const u32);
                match format {
                    RETRO_PIXEL_FORMAT_0RGB1555
                    | RETRO_PIXEL_FORMAT_XRGB8888
                    | RETRO_PIXEL_FORMAT_RGB565 => {
                        FRAME.lock().expect("frame lock poisoned").pixel_format = format;
                        true
                    }
                    _ => false,
                }
            }
            RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION => {
                *(data as *mut u32) = 1;
                true
            }
            RETRO_ENVIRONMENT_GET_LIBRETRO_PATH => {
                write_env_path(&LIBRETRO_PATH, data, "libretro_path")
            }
            RETRO_ENVIRONMENT_GET_CONTENT_DIRECTORY => {
                write_env_path(&CONTENT_DIRECTORY, data, "content_directory")
            }
            RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY => {
                write_env_path(&SAVE_DIRECTORY, data, "save_directory")
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
    if !video_enabled() || data.is_null() {
        return;
    }
    let size = (height as usize) * pitch;
    // SAFETY: libretro guarantees `data` points at `height * pitch` readable
    // bytes for the duration of this callback.
    let src = unsafe { std::slice::from_raw_parts(data as *const u8, size) };
    let mut frame = FRAME.lock().expect("frame lock poisoned");
    if frame.buf.len() < size {
        frame.buf.resize(size, 0);
    }
    frame.buf[..size].copy_from_slice(src);
    frame.width = width;
    frame.height = height;
    frame.pitch = pitch;
}

extern "C" fn audio_sample_cb(left: i16, right: i16) {
    if !audio_enabled() {
        return;
    }
    let mut audio = AUDIO.lock().expect("audio lock poisoned");
    audio.push(left);
    audio.push(right);
}
extern "C" fn audio_sample_batch_cb(data: *const i16, frames: usize) -> usize {
    if audio_enabled() && !data.is_null() && frames > 0 {
        // SAFETY: libretro guarantees `data` points at `frames` interleaved
        // stereo sample pairs for the duration of this callback.
        let samples = unsafe { std::slice::from_raw_parts(data, frames * 2) };
        AUDIO
            .lock()
            .expect("audio lock poisoned")
            .extend_from_slice(samples);
    }
    frames
}

extern "C" fn input_poll_cb() {}

extern "C" fn input_state_cb(port: u32, device: u32, index: u32, id: u32) -> i16 {
    if device == RETRO_DEVICE_JOYPAD
        && index == 0
        && port < 2
        && id < 16
        && PADS.lock().expect("pads lock poisoned").state[port as usize][id as usize]
    {
        return 1;
    }
    0
}

/// Loaded core handle. Holds the `Library` so it stays alive, plus the
/// function pointers we actually call from the main loop.
pub struct Core {
    _lib: Library, // kept alive so the other symbols stay valid
    pub run: unsafe extern "C" fn(),
    pub av_info: SystemAvInfo,
    reset_fn: unsafe extern "C" fn(),
    serialize_size_fn: unsafe extern "C" fn() -> usize,
    serialize_fn: unsafe extern "C" fn(*mut c_void, usize) -> bool,
    unserialize_fn: unsafe extern "C" fn(*const c_void, usize) -> bool,
    get_memory_data_fn: unsafe extern "C" fn(u32) -> *mut c_void,
    get_memory_size_fn: unsafe extern "C" fn(u32) -> usize,
}

impl Core {
    /// Reset the loaded game through libretro. This keeps the core loaded but
    /// returns emulation to the same clean state as a fresh arcade boot.
    pub fn reset(&self) {
        unsafe {
            (self.reset_fn)();
            clear_audio_buffer();
        }
    }

    /// Maximum serialized state size in bytes (upper bound; actual writes fit in this).
    pub fn serialize_size(&self) -> usize {
        unsafe { (self.serialize_size_fn)() }
    }

    /// Serialize the current emulation state into `buf`, reusing its
    /// allocation. `buf` is resized to exactly `serialize_size()`; when the
    /// buffer is already that size (the steady state during netplay, where
    /// this runs every frame) no allocation and no zero-fill happens.
    /// Returns false if the core refuses to serialize.
    pub fn save_state_into(&self, buf: &mut Vec<u8>) -> bool {
        let size = self.serialize_size();
        if size == 0 {
            return false;
        }
        buf.resize(size, 0);
        unsafe { (self.serialize_fn)(buf.as_mut_ptr() as *mut c_void, size) }
    }

    /// Snapshot the current emulation state into a fresh Vec.
    /// Returns None if the core refuses to serialize. Prefer
    /// `save_state_into` in per-frame paths to avoid the allocation.
    pub fn save_state(&self) -> Option<Vec<u8>> {
        let mut buf = Vec::new();
        if self.save_state_into(&mut buf) {
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

/// Load the FBNeo libretro core, wire callbacks, init the core, and load the ROM.
/// Returns a `Core` whose `run` is callable once per frame.
/// The core's `library_version` string, captured at load (e.g.
/// "v1.0.0.03 260705 GITcf53523"). Two peers can only stay in sync if their
/// cores were built from the same FBNeo commit — savestate layout and
/// simulation behavior drift between commits — so the GIT tag from this
/// string is folded into the matchmaking compatibility hash. The git ref
/// (not a hash of the core file) is the right key: the same ref built by
/// different compilers on different OSes yields different binaries that
/// are nonetheless sync-compatible.
static CORE_VERSION: OnceLock<String> = OnceLock::new();

/// Short sync-compatibility tag for the loaded core: the `GIT<hash>` token
/// from the core's version string if present, otherwise the whole version
/// string with spaces collapsed, otherwise "unknown" (no core loaded yet).
pub fn core_compat_tag() -> String {
    match CORE_VERSION.get() {
        Some(v) => parse_compat_tag(v),
        None => "unknown".to_string(),
    }
}

fn parse_compat_tag(version: &str) -> String {
    version
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("GIT"))
        .map(|h| h.to_ascii_lowercase())
        .unwrap_or_else(|| version.split_whitespace().collect::<Vec<_>>().join("_"))
}

#[cfg(test)]
mod compat_tag_tests {
    use super::parse_compat_tag;

    #[test]
    fn extracts_git_ref_from_fbneo_version_string() {
        assert_eq!(parse_compat_tag("v1.0.0.03 260705 GITcf53523"), "cf53523");
    }

    #[test]
    fn falls_back_to_collapsed_version_without_git_tag() {
        assert_eq!(parse_compat_tag("v1.0.0.03 260705"), "v1.0.0.03_260705");
    }
}

#[repr(C)]
struct RetroSystemInfo {
    library_name: *const c_char,
    library_version: *const c_char,
    valid_extensions: *const c_char,
    need_fullpath: bool,
    block_extract: bool,
}

pub unsafe fn load(dll_path: &str, rom_path: &str) -> Result<Core, Box<dyn std::error::Error>> {
    println!("Loading FBNeo Libretro Core...");
    crate::dlog!("retro", "core dll={dll_path}");
    crate::dlog!("retro", "rom zip={rom_path}");
    configure_environment_paths(dll_path, rom_path);
    let lib = Library::new(dll_path)?;

    if let Ok(get_system_info) = lib.get::<Symbol<unsafe extern "C" fn(*mut RetroSystemInfo)>>(
        b"retro_get_system_info\0",
    ) {
        let mut info = RetroSystemInfo {
            library_name: ptr::null(),
            library_version: ptr::null(),
            valid_extensions: ptr::null(),
            need_fullpath: false,
            block_extract: false,
        };
        get_system_info(&mut info);
        if !info.library_version.is_null() {
            let ver = std::ffi::CStr::from_ptr(info.library_version)
                .to_string_lossy()
                .into_owned();
            let name = if info.library_name.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(info.library_name)
                    .to_string_lossy()
                    .into_owned()
            };
            println!("Core: {name} {ver}");
            crate::dlog!("retro", "core system_info name={name} version={ver}");
            let _ = CORE_VERSION.set(ver);
        }
    }

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
    let retro_reset: Symbol<unsafe extern "C" fn()> = lib.get(b"retro_reset\0")?;
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
    let load_ok = retro_load_game(&game_info);
    if !load_ok {
        // Booting anyway used to be allowed here, but a rejected romset means
        // FBNeo's driver-level CRC/layout validation failed — the machine is
        // either unbootable or, worse, bootable-but-divergent, which shows up
        // online as an unexplainable instant desync. Fail loudly instead.
        crate::dlog!("retro", "retro_load_game returned false");
        return Err(format!(
            "FBNeo rejected the romset at {rom_path}. This usually means the zip is the \
             wrong MK2 revision or an incomplete/renamed set. Freeplay needs the arcade \
             'mk2' romset (rev L3.1) matching the bundled FBNeo core."
        )
        .into());
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
    let reset_fn: unsafe extern "C" fn() = *retro_reset;
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
        reset_fn,
        serialize_size_fn: ss_fn,
        serialize_fn: s_fn,
        unserialize_fn: u_fn,
        get_memory_data_fn: gmd_fn,
        get_memory_size_fn: gms_fn,
    })
}
