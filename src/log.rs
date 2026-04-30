//! Simple file logger for diagnosing netplay issues.
//!
//! One file per process (filename tagged with the player index so two
//! loopback processes don't clobber each other). Synchronous append,
//! auto-flushed. Negligible overhead — only called from already-expensive
//! frame/input paths, not from hot loops like callbacks.
#![allow(static_mut_refs)]

use std::fs::File;
use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;

static mut LOGGER: Option<Mutex<Logger>> = None;

struct Logger {
    file: File,
    start: Instant,
}

/// Initialize the global logger. Call once from main, early. `tag` goes into
/// the filename, e.g. "p1" or "p2" or "local".
pub fn init(tag: &str) {
    let filename = format!("debug_{tag}.log");
    match File::create(&filename) {
        Ok(mut f) => {
            let _ = writeln!(
                f,
                "=== freeplay-gametalk debug log ({tag}) opened at wall clock {:?} ===",
                std::time::SystemTime::now()
            );
            let _ = f.flush();
            unsafe {
                LOGGER = Some(Mutex::new(Logger {
                    file: f,
                    start: Instant::now(),
                }));
            }
            println!("[log] writing to {filename}");
        }
        Err(e) => eprintln!("[log] failed to open {filename}: {e}"),
    }
}

pub fn write(category: &str, msg: &str) {
    unsafe {
        if let Some(lock) = LOGGER.as_ref() {
            if let Ok(mut l) = lock.lock() {
                let t = l.start.elapsed();
                let _ = writeln!(
                    l.file,
                    "[{:>6}.{:03}] {} {}",
                    t.as_secs(),
                    t.subsec_millis(),
                    category,
                    msg
                );
                let _ = l.file.flush();
            }
        }
    }
}

/// Convenience macro so call sites read clean:  log!("input", "P1 Up pressed");
#[macro_export]
macro_rules! dlog {
    ($cat:literal, $($arg:tt)*) => {{
        $crate::log::write($cat, &format!($($arg)*));
    }};
}
