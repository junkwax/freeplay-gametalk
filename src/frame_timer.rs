use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
mod platform {
    use std::os::raw::c_uint;

    #[link(name = "winmm")]
    extern "system" {
        fn timeBeginPeriod(period_ms: c_uint) -> c_uint;
        fn timeEndPeriod(period_ms: c_uint) -> c_uint;
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

    impl TimerResolution {
        pub fn request_1ms() -> Self {
            Self
        }
    }
}

pub use self::platform::TimerResolution;

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
