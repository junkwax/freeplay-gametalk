#[cfg(target_os = "windows")]
use sdl2::video::Window;

#[cfg(target_os = "windows")]
use std::ffi::c_void;
#[cfg(target_os = "windows")]
use std::ptr::null_mut;
#[cfg(target_os = "windows")]
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "windows")]
type Hwnd = *mut c_void;
#[cfg(target_os = "windows")]
type Wparam = usize;
#[cfg(target_os = "windows")]
type Lparam = isize;
#[cfg(target_os = "windows")]
type Lresult = isize;
#[cfg(target_os = "windows")]
type WndProc = Option<unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult>;

#[cfg(target_os = "windows")]
const GWLP_WNDPROC: i32 = -4;
#[cfg(target_os = "windows")]
const HTCAPTION: Wparam = 2;
#[cfg(target_os = "windows")]
const WM_NCLBUTTONDOWN: u32 = 0x00A1;
#[cfg(target_os = "windows")]
const WM_NCDESTROY: u32 = 0x0082;
#[cfg(target_os = "windows")]
const WM_MOUSEMOVE: u32 = 0x0200;
#[cfg(target_os = "windows")]
const WM_LBUTTONUP: u32 = 0x0202;
#[cfg(target_os = "windows")]
const WM_CAPTURECHANGED: u32 = 0x0215;
#[cfg(target_os = "windows")]
const SWP_NOSIZE: u32 = 0x0001;
#[cfg(target_os = "windows")]
const SWP_NOZORDER: u32 = 0x0004;
#[cfg(target_os = "windows")]
const SDL_SYSWM_WINDOWS: u32 = 1;

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct SdlWinInfo {
    window: Hwnd,
    hdc: Hwnd,
    hinstance: Hwnd,
}

#[cfg(target_os = "windows")]
#[repr(C)]
union SdlWmInfoData {
    win: SdlWinInfo,
    dummy: [u8; 64],
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct SdlWmInfo {
    version: sdl2::sys::SDL_version,
    subsystem: u32,
    info: SdlWmInfoData,
}

#[cfg(target_os = "windows")]
#[derive(Copy, Clone)]
struct DragState {
    hwnd: Hwnd,
    previous_wndproc: isize,
    enabled: bool,
    dragging: bool,
    cursor_start: Point,
    window_start: Point,
}

#[cfg(target_os = "windows")]
unsafe impl Send for DragState {}

#[cfg(target_os = "windows")]
static STATE: OnceLock<Mutex<Option<DragState>>> = OnceLock::new();

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, new_long: isize) -> isize;
    fn CallWindowProcW(
        previous: WndProc,
        hwnd: Hwnd,
        msg: u32,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: Wparam, lparam: Lparam) -> Lresult;
    fn SetCapture(hwnd: Hwnd) -> Hwnd;
    fn ReleaseCapture() -> i32;
    fn GetCapture() -> Hwnd;
    fn GetCursorPos(point: *mut Point) -> i32;
    fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn SetWindowPos(
        hwnd: Hwnd,
        insert_after: Hwnd,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
}

#[cfg(target_os = "windows")]
#[link(name = "SDL2")]
extern "C" {
    fn SDL_GetWindowWMInfo(
        window: *mut sdl2::sys::SDL_Window,
        info: *mut SdlWmInfo,
    ) -> sdl2::sys::SDL_bool;
}

#[cfg(target_os = "windows")]
pub struct NativeTitlebarDrag;

#[cfg(target_os = "windows")]
pub fn install(window: &Window) -> Result<NativeTitlebarDrag, String> {
    let hwnd = sdl_hwnd(window)?;
    let shim = titlebar_drag_wndproc as *const () as usize as isize;
    let previous_wndproc = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, shim) };
    if previous_wndproc == 0 {
        return Err("SetWindowLongPtrW returned null".into());
    }

    let mut guard = state().lock().map_err(|_| "titlebar drag state poisoned")?;
    *guard = Some(DragState {
        hwnd,
        previous_wndproc,
        enabled: false,
        dragging: false,
        cursor_start: Point::default(),
        window_start: Point::default(),
    });
    Ok(NativeTitlebarDrag)
}

#[cfg(target_os = "windows")]
pub fn set_enabled(enabled: bool) {
    let Ok(mut guard) = state().lock() else {
        return;
    };
    let Some(state) = guard.as_mut() else {
        return;
    };
    state.enabled = enabled;
    if !enabled {
        stop_dragging(state);
    }
}

#[cfg(target_os = "windows")]
impl Drop for NativeTitlebarDrag {
    fn drop(&mut self) {
        let Ok(mut guard) = state().lock() else {
            return;
        };
        if let Some(state) = guard.take() {
            unsafe {
                if GetCapture() == state.hwnd {
                    let _ = ReleaseCapture();
                }
                SetWindowLongPtrW(state.hwnd, GWLP_WNDPROC, state.previous_wndproc);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn state() -> &'static Mutex<Option<DragState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

#[cfg(target_os = "windows")]
fn sdl_hwnd(window: &Window) -> Result<Hwnd, String> {
    let mut info = SdlWmInfo {
        version: sdl2::sys::SDL_version {
            major: 0,
            minor: 0,
            patch: 0,
        },
        subsystem: 0,
        info: SdlWmInfoData { dummy: [0; 64] },
    };
    unsafe {
        sdl2::sys::SDL_GetVersion(&mut info.version);
        if SDL_GetWindowWMInfo(window.raw(), &mut info) != sdl2::sys::SDL_bool::SDL_TRUE {
            return Err("SDL_GetWindowWMInfo failed".into());
        }
        if info.subsystem != SDL_SYSWM_WINDOWS {
            return Err(format!(
                "SDL window subsystem {} is not Win32",
                info.subsystem
            ));
        }
        let hwnd = info.info.win.window;
        if hwnd.is_null() {
            return Err("SDL returned a null Win32 HWND".into());
        }
        Ok(hwnd)
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn titlebar_drag_wndproc(
    hwnd: Hwnd,
    msg: u32,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    if msg == WM_NCDESTROY {
        let previous = take_previous_wndproc(hwnd);
        if previous != 0 {
            SetWindowLongPtrW(hwnd, GWLP_WNDPROC, previous);
            return call_wndproc(previous, hwnd, msg, wparam, lparam);
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }

    if handle_drag_message(hwnd, msg, wparam) {
        return 0;
    }

    let previous = previous_wndproc(hwnd);
    if previous != 0 {
        call_wndproc(previous, hwnd, msg, wparam, lparam)
    } else {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

#[cfg(target_os = "windows")]
unsafe fn call_wndproc(
    previous: isize,
    hwnd: Hwnd,
    msg: u32,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    let proc: WndProc = std::mem::transmute(previous);
    CallWindowProcW(proc, hwnd, msg, wparam, lparam)
}

#[cfg(target_os = "windows")]
fn handle_drag_message(hwnd: Hwnd, msg: u32, wparam: Wparam) -> bool {
    let Ok(mut guard) = state().lock() else {
        return false;
    };
    let Some(state) = guard.as_mut() else {
        return false;
    };
    if state.hwnd != hwnd {
        return false;
    }

    match msg {
        WM_NCLBUTTONDOWN if state.enabled && wparam == HTCAPTION => {
            let mut cursor = Point::default();
            let mut rect = Rect::default();
            unsafe {
                if GetCursorPos(&mut cursor) == 0 || GetWindowRect(hwnd, &mut rect) == 0 {
                    return false;
                }
                SetCapture(hwnd);
            }
            state.dragging = true;
            state.cursor_start = cursor;
            state.window_start = Point {
                x: rect.left,
                y: rect.top,
            };
            true
        }
        WM_MOUSEMOVE if state.dragging => {
            let mut cursor = Point::default();
            unsafe {
                if GetCursorPos(&mut cursor) != 0 {
                    let dx = cursor.x.saturating_sub(state.cursor_start.x);
                    let dy = cursor.y.saturating_sub(state.cursor_start.y);
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        state.window_start.x.saturating_add(dx),
                        state.window_start.y.saturating_add(dy),
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER,
                    );
                }
            }
            true
        }
        WM_LBUTTONUP if state.dragging => {
            stop_dragging(state);
            true
        }
        WM_CAPTURECHANGED if state.dragging => {
            state.dragging = false;
            true
        }
        _ => false,
    }
}

#[cfg(target_os = "windows")]
fn stop_dragging(state: &mut DragState) {
    state.dragging = false;
    unsafe {
        if GetCapture() == state.hwnd {
            let _ = ReleaseCapture();
        }
    }
}

#[cfg(target_os = "windows")]
fn previous_wndproc(hwnd: Hwnd) -> isize {
    let Ok(guard) = state().lock() else {
        return 0;
    };
    guard
        .as_ref()
        .filter(|state| state.hwnd == hwnd)
        .map(|state| state.previous_wndproc)
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
fn take_previous_wndproc(hwnd: Hwnd) -> isize {
    let Ok(mut guard) = state().lock() else {
        return 0;
    };
    if guard.as_ref().map(|state| state.hwnd) == Some(hwnd) {
        guard
            .take()
            .map(|state| state.previous_wndproc)
            .unwrap_or(0)
    } else {
        0
    }
}

#[cfg(not(target_os = "windows"))]
use sdl2::video::Window;

#[cfg(not(target_os = "windows"))]
pub struct NativeTitlebarDrag;

#[cfg(not(target_os = "windows"))]
pub fn install(_window: &Window) -> Result<NativeTitlebarDrag, String> {
    Ok(NativeTitlebarDrag)
}

#[cfg(not(target_os = "windows"))]
pub fn set_enabled(_enabled: bool) {}
