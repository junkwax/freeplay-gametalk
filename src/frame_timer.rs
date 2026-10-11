use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
mod platform {
    use std::os::raw::c_uint;

    #[link(name = "winmm")]
    extern "system" {
        fn timeBeginPeriod(period_ms: c_uint) -> c_uint;
        fn timeEndPeriod(period_ms: c_uint) -> c_uint;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn SetPriorityClass(process: *mut std::ffi::c_void, class: u32) -> i32;
    }

    const HIGH_PRIORITY_CLASS: u32 = 0x80;

    /// Same class Fightcade's emulator runs at (`nAppProcessPriority 128`).
    /// A frame is 18 ms with no slack for a background task stealing the
    /// core mid-rollback; the loop sleeps between frames, so this doesn't
    /// starve the rest of the system.
    pub fn raise_process_priority() {
        let ok = unsafe { SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS) } != 0;
        if ok {
            println!("[timer] process priority set to HIGH");
        } else {
            println!("[timer] SetPriorityClass(HIGH) failed");
        }
    }

    pub struct TimerResolution {
        active: bool,
    }

    impl TimerResolution {
        pub fn request_1ms() -> Self {
            let rc = unsafe { timeBeginPeriod(1) };
            let active = rc == 0;
            if active {
                println!("[timer] Windows timer resolution set to 1ms");
            } else {
                println!("[timer] timeBeginPeriod(1) failed with code {rc}");
            }
            Self { active }
        }
    }

    impl Drop for TimerResolution {
        fn drop(&mut self) {
            if self.active {
                let rc = unsafe { timeEndPeriod(1) };
                if rc != 0 {
                    println!("[timer] timeEndPeriod(1) failed with code {rc}");
                }
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    pub struct TimerResolution;

    pub fn raise_process_priority() {}

    impl TimerResolution {
        pub fn request_1ms() -> Self {
            Self
        }
    }
}

pub use self::platform::{raise_process_priority, TimerResolution};

pub fn wait_until_frame_deadline(deadline: Instant) {
    let spin_window = Duration::from_micros(750);
    loop {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        let remaining = deadline.saturating_duration_since(now);
        if remaining <= spin_window {
            break;
        }
        std::thread::sleep(remaining - spin_window);
    }
    while Instant::now() < deadline {
        std::hint::spin_loop();
    }
}
