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
    println!("cargo:rerun-if-changed=src/app.ico");
    println!("cargo:rerun-if-changed=appicon.png");
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
    if std::fs::write(&rc_path, build_rc_contents()).is_err() {
        return;
    }

    let Some(rc_exe) = find_rc_exe() else {
        println!("cargo:warning=rc.exe not found (PATH or Windows SDK); exe icon/version info not embedded");
        return;
    };
    let rc = std::process::Command::new(&rc_exe)
        .args([
            "/nologo",
            "/fo",
            res_path.to_string_lossy().as_ref(),
            rc_path.to_string_lossy().as_ref(),
        ])
        .status();

    if matches!(rc, Ok(status) if status.success()) {
        println!("cargo:rustc-link-arg-bins={}", res_path.display());
    } else {
        println!("cargo:warning=rc.exe failed; exe icon/version info not embedded");
    }
}

/// rc.exe is only on PATH inside a VS developer prompt; plain `cargo build`
/// from a normal shell needs the Windows SDK location resolved by hand.
fn find_rc_exe() -> Option<std::path::PathBuf> {
    let on_path = std::process::Command::new("rc.exe")
        .arg("/?")
        .output()
        .is_ok();
    if on_path {
        return Some(std::path::PathBuf::from("rc.exe"));
    }
    let kits = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<std::path::PathBuf> = std::fs::read_dir(kits)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("x64").join("rc.exe").is_file())
        .collect();
    versions.sort(); // lexicographic works for 10.0.NNNNN.0 names
    versions.pop().map(|p| p.join("x64").join("rc.exe"))
}

/// Icon plus a VERSIONINFO block so the exe's Properties → Details shows
/// product name, version, and the project URL instead of blank fields.
/// (The Explorer/SmartScreen "Publisher" line comes from an Authenticode
/// signature, not from this resource — unsigned builds still say Unknown.)
fn build_rc_contents() -> String {
    let semver = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut parts = semver.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    let (maj, min, pat) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    format!(
        r#"1 ICON "src/app.ico"
1 VERSIONINFO
FILEVERSION {maj},{min},{pat},0
PRODUCTVERSION {maj},{min},{pat},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Freeplay (github.com/junkwax/freeplay-gametalk)"
      VALUE "FileDescription", "Freeplay - Netplay Client"
      VALUE "FileVersion", "{semver}"
      VALUE "ProductName", "Freeplay"
      VALUE "ProductVersion", "{semver}"
      VALUE "OriginalFilename", "freeplay.exe"
      VALUE "LegalCopyright", "Open source - https://github.com/junkwax/freeplay-gametalk"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 0x04B0
  END
END
"#
    )
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
