//! "Keep running with the lid closed" — for playing a laptop docked to a
//! monitor and a pad with the lid shut.
//!
//! Closing a laptop's lid is an OS policy decision, not something an app can
//! veto the way it can veto idle sleep (SDL already does that while we run).
//! So this changes the policy for as long as Freeplay is open, and puts it
//! back afterwards:
//!
//! - **Windows:** sets the active power plan's *plugged-in* lid action to
//!   "Do nothing" via `powercfg`. Battery is deliberately left alone: a laptop
//!   that ignores its lid on battery is the one that cooks itself in a bag.
//!   The original value is written to `LID_RESTORE_FILE` before it is changed,
//!   so a crash or a killed process is undone on the next launch.
//! - **Linux:** holds a logind `handle-lid-switch` inhibitor through
//!   `systemd-inhibit`. The lock lives in a child process whose stdin is a pipe
//!   from us; when we exit, however we exit, the pipe closes and the lock goes.
//! - **macOS:** nothing to do. A Mac with power, an external display and an
//!   input device already keeps running closed (clamshell mode), and changing
//!   that otherwise needs `sudo pmset`.

#[cfg(windows)]
const LID_RESTORE_FILE: &str = "lid_restore.txt";

pub struct LidGuard {
    #[cfg(windows)]
    restore: Option<(String, u32)>,
    #[cfg(target_os = "linux")]
    inhibitor: Option<std::process::Child>,
}

impl LidGuard {
    pub fn inactive() -> Self {
        Self {
            #[cfg(windows)]
            restore: None,
            #[cfg(target_os = "linux")]
            inhibitor: None,
        }
    }

    pub fn is_active(&self) -> bool {
        #[cfg(windows)]
        {
            self.restore.is_some()
        }
        #[cfg(target_os = "linux")]
        {
            self.inhibitor.is_some()
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            false
        }
    }

    /// Turn the guard on or off to match the setting. Returns a short message
    /// worth showing the player when something didn't work.
    pub fn set_enabled(&mut self, enabled: bool) -> Option<String> {
        if enabled == self.is_active() {
            return None;
        }
        if enabled {
            self.engage()
        } else {
            self.release();
            None
        }
    }

    #[cfg(windows)]
    fn engage(&mut self) -> Option<String> {
        let scheme = match windows::active_scheme() {
            Some(s) => s,
            None => return Some("Lid setting unavailable: no active power plan".into()),
        };
        let original = match windows::ac_lid_action(&scheme) {
            Some(v) => v,
            // Desktops hide the lid setting entirely.
            None => return Some("This PC has no lid setting to change".into()),
        };
        if original == windows::LID_DO_NOTHING {
            return None;
        }
        // Recorded before the change, so there is never a window where the
        // setting is changed and nothing knows how to undo it.
        if let Err(e) = std::fs::write(LID_RESTORE_FILE, format!("{scheme} {original}")) {
            return Some(format!("Lid setting not changed: {e}"));
        }
        if let Err(e) = windows::set_ac_lid_action(&scheme, windows::LID_DO_NOTHING) {
            let _ = std::fs::remove_file(LID_RESTORE_FILE);
            return Some(format!("Lid setting not changed: {e}"));
        }
        println!("[lid] plugged-in lid action set to do nothing (was {original})");
        self.restore = Some((scheme, original));
        None
    }

    #[cfg(windows)]
    fn release(&mut self) {
        if let Some((scheme, original)) = self.restore.take() {
            match windows::set_ac_lid_action(&scheme, original) {
                Ok(()) => {
                    let _ = std::fs::remove_file(LID_RESTORE_FILE);
                    println!("[lid] plugged-in lid action restored to {original}");
                }
                // Leave the file so the next launch retries.
                Err(e) => println!("[lid] could not restore lid action: {e}"),
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn engage(&mut self) -> Option<String> {
        use std::process::{Command, Stdio};
        let child = Command::new("systemd-inhibit")
            .args([
                "--what=handle-lid-switch",
                "--who=Freeplay",
                "--why=Playing with the lid closed",
                "--mode=block",
                "cat",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(mut c) => {
                // A refused lock (polkit) exits at once instead of blocking.
                std::thread::sleep(std::time::Duration::from_millis(150));
                if let Ok(Some(status)) = c.try_wait() {
                    return Some(format!("Lid lock refused by the system ({status})"));
                }
                println!("[lid] holding handle-lid-switch inhibitor");
                self.inhibitor = Some(c);
                None
            }
            Err(e) => Some(format!("Lid lock unavailable: {e}")),
        }
    }

    #[cfg(target_os = "linux")]
    fn release(&mut self) {
        if let Some(mut c) = self.inhibitor.take() {
            drop(c.stdin.take());
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn engage(&mut self) -> Option<String> {
        Some("Mac: lid-closed play works with power + external display".into())
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn release(&mut self) {}
}

impl Drop for LidGuard {
    fn drop(&mut self) {
        self.release();
    }
}

/// Undo a lid change left behind by a session that never reached its own
/// cleanup. Must run before any `LidGuard` engages.
pub fn recover_from_crash() {
    #[cfg(windows)]
    {
        let Ok(text) = std::fs::read_to_string(LID_RESTORE_FILE) else {
            return;
        };
        let mut parts = text.split_whitespace();
        let (Some(scheme), Some(Ok(original))) =
            (parts.next(), parts.next().map(str::parse::<u32>))
        else {
            let _ = std::fs::remove_file(LID_RESTORE_FILE);
            return;
        };
        match windows::set_ac_lid_action(scheme, original) {
            Ok(()) => {
                let _ = std::fs::remove_file(LID_RESTORE_FILE);
                println!("[lid] restored lid action {original} left by a previous session");
            }
            Err(e) => println!("[lid] could not restore previous lid action: {e}"),
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    // powercfg is a console program; without this every call flashes a
    // console window over the game.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    pub const LID_DO_NOTHING: u32 = 0;

    fn powercfg(args: &[&str]) -> Result<String, String> {
        let out = Command::new("powercfg")
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let err = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Err(if err.is_empty() { format!("powercfg {}", out.status) } else { err })
        }
    }

    pub fn active_scheme() -> Option<String> {
        parse_scheme_guid(&powercfg(&["/getactivescheme"]).ok()?)
    }

    pub fn ac_lid_action(scheme: &str) -> Option<u32> {
        parse_ac_index(&powercfg(&["/q", scheme, "SUB_BUTTONS", "LIDACTION"]).ok()?)
    }

    pub fn set_ac_lid_action(scheme: &str, value: u32) -> Result<(), String> {
        powercfg(&["/setacvalueindex", scheme, "SUB_BUTTONS", "LIDACTION", &value.to_string()])?;
        // Writing an index doesn't apply it until the plan is re-activated.
        powercfg(&["/setactive", "SCHEME_CURRENT"])?;
        Ok(())
    }

    /// First GUID-shaped token. The surrounding text is localized.
    pub(super) fn parse_scheme_guid(text: &str) -> Option<String> {
        text.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ':')
            .find(|t| {
                t.len() == 36
                    && t.chars().filter(|&c| c == '-').count() == 4
                    && t.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
            })
            .map(str::to_string)
    }

    /// The AC index is the first `0x` value in the query output (DC is the
    /// second). Matching the label instead would break on non-English
    /// Windows, where "Current AC Power Setting Index" is translated.
    pub(super) fn parse_ac_index(text: &str) -> Option<u32> {
        text.split_whitespace()
            .find_map(|t| t.strip_prefix("0x"))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::windows::{parse_ac_index, parse_scheme_guid};

    #[test]
    fn reads_the_active_scheme_guid() {
        let out = "Power Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)\r\n";
        assert_eq!(
            parse_scheme_guid(out).as_deref(),
            Some("381b4222-f694-41f0-9685-ff5bb260df2e")
        );
    }

    #[test]
    fn reads_the_ac_lid_index_not_the_dc_one() {
        let out = "\
Power Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)
  GUID Alias: SCHEME_BALANCED
  Subgroup GUID: 4f971e89-eebd-4455-a8de-9e59040e7347  (Power buttons and lid)
    GUID Alias: SUB_BUTTONS
    Power Setting GUID: 5ca83367-6e45-459f-a27b-476b1d01c936  (Lid close action)
      GUID Alias: LIDACTION
      Possible Setting Index: 000
      Possible Setting Friendly Name: Do nothing
      Possible Setting Index: 001
      Possible Setting Friendly Name: Sleep
    Current AC Power Setting Index: 0x00000001
    Current DC Power Setting Index: 0x00000002
";
        assert_eq!(parse_ac_index(out), Some(1));
    }

    #[test]
    fn a_desktop_with_no_lid_setting_reads_as_none() {
        let out = "Power Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)\n  GUID Alias: SCHEME_BALANCED\n";
        assert_eq!(parse_ac_index(out), None);
    }
}
