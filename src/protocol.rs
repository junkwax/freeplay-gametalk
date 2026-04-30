//! xband:// custom URI scheme — registration and deep link parsing.
//!
//! ## URI scheme
//!
//!   xband://join/<room_id>          Deep link from Discord "click to join" webhook
//!   xband://auth/callback#token=… OAuth redirect target (caught by local HTTP
//!                                   server in matchmaking.rs, not the exe directly)
//!
//! ## Registration
//!
//! On Windows the scheme is written to HKCU\Software\Classes\xband so it
//! doesn't require admin rights and survives alongside other users' installs.
//! Call `register_uri_scheme()` once at startup — it's idempotent and fast
//! (a few registry reads/writes, no disk I/O).
//!
//! The .reg file in the repo root is for manual/installer use and writes to
//! HKCR (machine-wide). The runtime registration here writes to HKCU
//! (user-scope) which takes precedence on the same machine and needs no UAC.

/// Parse a raw xband:// URI the OS handed us on the command line.
///
/// Windows passes the URI as a single argument: `freeplay.exe "xband://join/abc123"`
/// Returns `None` for unrecognised or malformed URIs.
pub fn parse_uri(arg: &str) -> Option<XbandUri> {
    let rest = arg.strip_prefix("xband://")?;
    if let Some(room_id) = rest.strip_prefix("join/") {
        if !room_id.is_empty() {
            return Some(XbandUri::Join {
                room_id: room_id.trim_end_matches('/').to_string(),
            });
        }
    }
    None
}

#[derive(Debug, Clone)]
pub enum XbandUri {
    /// xband://join/<room_id> — connect directly to an in-progress match lobby
    Join { room_id: String },
}

// ── Windows registry registration ─────────────────────────────────────────────

#[cfg(target_os = "windows")]
pub fn register_uri_scheme() {
    // Get the path to the running exe
    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            println!("[protocol] could not get exe path: {e}");
            return;
        }
    };
    let exe_str = exe_path.to_string_lossy();

    // Write to HKCU\Software\Classes\xband (no admin required)
    match write_registry_scheme(&exe_str) {
        Ok(_) => println!("[protocol] xband:// scheme registered for current user"),
        Err(e) => println!("[protocol] xband:// registration failed (non-fatal): {e}"),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn register_uri_scheme() {
    // macOS: requires a CFBundleURLTypes entry in Info.plist — handled at bundle time.
    // Linux: requires a .desktop file in ~/.local/share/applications/ — out of scope for now.
    println!("[protocol] URI scheme registration not implemented on this platform");
}

#[cfg(target_os = "windows")]
fn write_registry_scheme(exe_path: &str) -> Result<(), String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    // HKCU\Software\Classes\xband
    let (key, _) = hkcu
        .create_subkey(r"Software\Classes\xband")
        .map_err(|e| e.to_string())?;
    key.set_value("", &"Freeplay xband Protocol")
        .map_err(|e| e.to_string())?;
    key.set_value("URL Protocol", &"")
        .map_err(|e| e.to_string())?;

    // HKCU\Software\Classes\xband\DefaultIcon
    let (icon_key, _) = hkcu
        .create_subkey(r"Software\Classes\xband\DefaultIcon")
        .map_err(|e| e.to_string())?;
    icon_key
        .set_value("", &format!("{exe_path},0"))
        .map_err(|e| e.to_string())?;

    // HKCU\Software\Classes\xband\shell\open\command
    let (cmd_key, _) = hkcu
        .create_subkey(r"Software\Classes\xband\shell\open\command")
        .map_err(|e| e.to_string())?;
    cmd_key
        .set_value("", &format!("\"{exe_path}\" \"%1\""))
        .map_err(|e| e.to_string())?;

    Ok(())
}
