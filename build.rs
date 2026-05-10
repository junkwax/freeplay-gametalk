//! Bake a build date into an env var so the frontend can show it in About.
//! Runs every time cargo rebuilds (we explicitly rerun on source changes).

use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let (y, m, d) = civil_date(secs);
    println!("cargo:rustc-env=FREEPLAY_BUILD_DATE={y:04}-{m:02}-{d:02}");

    // Best-effort git revision. Silently absent if git isn't available.
    if let Some(mut hash) = git_output(["rev-parse", "--short", "HEAD"]) {
        if git_is_dirty() {
            hash.push_str("-dirty");
        }
        println!("cargo:rustc-env=FREEPLAY_GIT_HASH={hash}");
    }

    // Tell the linker where to find SDL2.lib / SDL2_ttf.lib on Windows.
    // The sdl2 crate emits `rustc-link-search=native=lib` (project root),
    // but our pre-built libs live in src/lib/.
    println!("cargo:rustc-link-search=native=src/lib");
    embed_windows_icon();

    // Rerun when the build script itself or src/ changes. Cargo already tracks
    // src changes for the main build, but build scripts need explicit hints.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    print_git_rerun_hints();
}

fn git_output<const N: usize>(args: [&str; N]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn git_is_dirty() -> bool {
    !git_status_ok(["diff", "--quiet"]) || !git_status_ok(["diff", "--cached", "--quiet"])
}

fn git_status_ok<const N: usize>(args: [&str; N]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(true)
}

fn print_git_rerun_hints() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    let head = match std::fs::read_to_string(".git/HEAD") {
        Ok(head) => head,
        Err(_) => return,
    };
    if let Some(reference) = head.trim().strip_prefix("ref: ") {
        println!("cargo:rerun-if-changed=.git/{reference}");
    }
}

fn embed_windows_icon() {
    if std::env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }

    let out_dir = match std::env::var("OUT_DIR") {
        Ok(v) => std::path::PathBuf::from(v),
        Err(_) => return,
    };
    let rc_path = out_dir.join("freeplay_icon.rc");
    let res_path = out_dir.join("freeplay_icon.res");
    if std::fs::write(&rc_path, r#"1 ICON "src/app.ico""#).is_err() {
        return;
    }

    let rc = std::process::Command::new("rc.exe")
        .args([
            "/nologo",
            "/fo",
            res_path.to_string_lossy().as_ref(),
            rc_path.to_string_lossy().as_ref(),
        ])
        .status();

    if matches!(rc, Ok(status) if status.success()) {
        println!("cargo:rustc-link-arg-bins={}", res_path.display());
    }
}

/// Convert a unix timestamp (UTC) to (year, month, day). Avoids a chrono dep.
/// Good from 1970 through 2099-ish.
fn civil_date(mut secs: i64) -> (i32, u32, u32) {
    let days_from_epoch = secs.div_euclid(86400);
    secs = days_from_epoch;
    // Algorithm: Howard Hinnant's days-from-civil, inverted.
    let z = secs + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
