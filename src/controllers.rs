//! Two-slot SDL game controller management. `pads[0]` is P1's pad,
//! `pads[1]` is P2's. Hot-plug is handled via `assign_pad`; ownership of an
//! already-assigned pad is looked up by SDL instance ID.
//!
//! Everything downstream of this module — bindings, rebind capture, menu
//! navigation, gameplay input — speaks only in SDL `GameController` events.
//! A pad SDL has no mapping for is therefore not merely mis-bound, it is
//! invisible to the entire app: it can't even be captured in Settings ->
//! Controls to fix itself. That is the normal state of a DualShock 3 on
//! Windows, where the pad reaches SDL through one of several third-party
//! drivers (DsHidMini, ScpToolkit, libusb) and SDL's built-in database has
//! no entry for the GUID any of them present.
//!
//! `ensure_mapping` closes that gap by synthesizing a mapping from the
//! device's own reported capabilities. The synthesized button order is a
//! best guess, but it is a guess that keeps every physical control
//! *reachable* — so if it lands buttons in the wrong places, the rebind UI
//! can correct it, which is not true of a pad SDL never surfaces at all.

use crate::input::Player;
use sdl2::controller::GameController;
use sdl2::{GameControllerSubsystem, JoystickSubsystem};
use std::path::PathBuf;

/// Up to two open controllers: pads[0] = P1's pad, pads[1] = P2's pad.
pub type Pads = [Option<GameController>; 2];

/// What `ensure_mapping` had to do to make a device usable, which decides
/// whether the caller is responsible for opening it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MappingOutcome {
    /// SDL already knew this device, so it emits `ControllerDeviceAdded` on
    /// its own — opening it here too would assign one pad to both slots.
    AlreadyMapped,
    /// We just registered a mapping for it. SDL does not retroactively
    /// announce a device as a controller once a mapping shows up, so the
    /// caller owns opening this one.
    Synthesized,
    /// Still unusable as a game controller.
    Unmapped,
}

/// SDL reads these once when the joystick subsystem starts, so this must run
/// before `sdl2::init()` rather than per-device.
///
/// The PS3 hint is the load-bearing one: SDL's HIDAPI DualShock 3 driver is
/// compiled in but defaults to off on Windows, so without this a DS3 is only
/// visible at all when a third-party driver happens to be installed.
pub fn set_joystick_hints() {
    let _ = sdl2::hint::set("SDL_JOYSTICK_HIDAPI", "1");
    let _ = sdl2::hint::set("SDL_JOYSTICK_HIDAPI_PS3", "1");
    let _ = sdl2::hint::set("SDL_JOYSTICK_HIDAPI_PS4", "1");
    let _ = sdl2::hint::set("SDL_JOYSTICK_HIDAPI_PS5", "1");
}

/// Candidate locations for a community `gamecontrollerdb.txt`, in priority
/// order: alongside the binary first, then the bundled assets folder.
fn mapping_db_paths() -> Vec<PathBuf> {
    let mut paths = vec![
        PathBuf::from("gamecontrollerdb.txt"),
        PathBuf::from("assets").join("gamecontrollerdb.txt"),
    ];
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)) {
        paths.push(dir.join("gamecontrollerdb.txt"));
    }
    paths
}

/// Load a community mapping database if the user dropped one in. A real
/// entry always beats anything `synthesize_mapping` can infer, so this runs
/// before any device is inspected — it is the supported escape hatch for a
/// pad whose synthesized layout comes out wrong.
pub fn load_mapping_db(gc: &GameControllerSubsystem) {
    for path in mapping_db_paths() {
        if !path.exists() {
            continue;
        }
        match gc.load_mappings(&path) {
            Ok(n) => println!("Loaded {n} controller mappings from {}", path.display()),
            Err(e) => println!("Failed to load {}: {e}", path.display()),
        }
        return;
    }
}

/// Make `index` usable as a game controller, synthesizing a mapping if SDL
/// has none. See `MappingOutcome` for who owns opening the device after.
pub fn ensure_mapping(
    gc: &GameControllerSubsystem,
    js: &JoystickSubsystem,
    index: u32,
) -> MappingOutcome {
    if gc.is_game_controller(index) {
        return MappingOutcome::AlreadyMapped;
    }

    let joystick = match js.open(index) {
        Ok(j) => j,
        Err(e) => {
            println!("Joystick {index}: cannot open to inspect capabilities: {e}");
            return MappingOutcome::Unmapped;
        }
    };
    let name = joystick.name();
    let mapping = synthesize_mapping(
        &joystick.guid().string(),
        &name,
        joystick.num_buttons(),
        joystick.num_axes(),
        joystick.num_hats(),
    );
    // Close before the caller re-opens the same device as a controller.
    drop(joystick);

    if let Err(e) = gc.add_mapping(&mapping) {
        println!("Joystick {index} ({name}): mapping rejected by SDL: {e}");
        return MappingOutcome::Unmapped;
    }
    if gc.is_game_controller(index) {
        println!("Joystick {index} ({name}): no SDL mapping, using synthesized layout");
        println!("  {mapping}");
        MappingOutcome::Synthesized
    } else {
        println!("Joystick {index} ({name}): still not a game controller after mapping");
        MappingOutcome::Unmapped
    }
}

/// Build an SDL mapping string for a device SDL doesn't recognize.
fn synthesize_mapping(guid: &str, name: &str, buttons: u32, axes: u32, hats: u32) -> String {
    // Mapping fields are comma-separated, so a comma in the device name
    // would split the entry mid-record.
    let safe_name = name.replace(',', " ");
    let mut out = format!("{guid},{safe_name}");
    for (field, control) in layout_for(name, buttons, axes, hats) {
        out.push_str(&format!(",{field}:{control}"));
    }
    // Deliberately no `platform:` field — SDL applies a platform-less
    // mapping everywhere, and these are derived from the live device rather
    // than from a per-OS table that would need one.
    out.push(',');
    out
}

fn is_dualshock3(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("ps3")
        || n.contains("playstation(r)3")
        || n.contains("playstation 3")
        || n.contains("dualshock 3")
        || n.contains("sixaxis")
}

fn layout_for(name: &str, buttons: u32, axes: u32, hats: u32) -> Vec<(&'static str, String)> {
    if is_dualshock3(name) && buttons >= 17 && axes >= 4 {
        dualshock3_layout()
    } else {
        generic_layout(buttons, axes, hats)
    }
}

/// DualShock 3 in its HID/DirectInput report order, which every DS3 driver
/// that doesn't emulate an Xbox pad presents: 0 Select, 1 L3, 2 R3, 3 Start,
/// 4-7 D-pad (up/right/down/left), 8 L2, 9 R2, 10 L1, 11 R1, 12 Triangle,
/// 13 Circle, 14 Cross, 15 Square, 16 PS.
///
/// Face buttons map by position, not by letter: SDL's `a` is the bottom face
/// button, which on a DS3 is Cross.
///
/// L2/R2 are mapped as triggers even though DirectInput reports them as
/// plain buttons — SDL synthesizes the 0..32767 axis motion from a
/// button-backed trigger, so the default Block binding (TriggerRight) works
/// without special-casing.
fn dualshock3_layout() -> Vec<(&'static str, String)> {
    [
        ("a", "b14"),
        ("b", "b13"),
        ("x", "b15"),
        ("y", "b12"),
        ("back", "b0"),
        ("start", "b3"),
        ("guide", "b16"),
        ("leftstick", "b1"),
        ("rightstick", "b2"),
        ("leftshoulder", "b10"),
        ("rightshoulder", "b11"),
        ("lefttrigger", "b8"),
        ("righttrigger", "b9"),
        ("dpup", "b4"),
        ("dpright", "b5"),
        ("dpdown", "b6"),
        ("dpleft", "b7"),
        ("leftx", "a0"),
        ("lefty", "a1"),
        ("rightx", "a2"),
        ("righty", "a3"),
    ]
    .into_iter()
    .map(|(field, control)| (field, control.to_string()))
    .collect()
}

/// Positional guess for an unrecognized pad, matching the order most
/// DirectInput devices enumerate in. Entries are emitted only for controls
/// the device actually reports, so a mapping never references a button or
/// axis that isn't there.
fn generic_layout(buttons: u32, axes: u32, hats: u32) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for (field, idx) in [
        ("a", 0u32),
        ("b", 1),
        ("x", 2),
        ("y", 3),
        ("leftshoulder", 4),
        ("rightshoulder", 5),
        ("back", 6),
        ("start", 7),
        ("leftstick", 8),
        ("rightstick", 9),
    ] {
        if idx < buttons {
            out.push((field, format!("b{idx}")));
        }
    }

    if hats > 0 {
        for (field, mask) in [("dpup", 1), ("dpright", 2), ("dpdown", 4), ("dpleft", 8)] {
            out.push((field, format!("h0.{mask}")));
        }
    } else {
        // No hat switch: the D-pad is usually the block of buttons right
        // after the stick clicks.
        for (field, idx) in [("dpup", 10u32), ("dpdown", 11), ("dpleft", 12), ("dpright", 13)] {
            if idx < buttons {
                out.push((field, format!("b{idx}")));
            }
        }
    }

    for (field, idx) in [("leftx", 0u32), ("lefty", 1), ("rightx", 2), ("righty", 3)] {
        if idx < axes {
            out.push((field, format!("a{idx}")));
        }
    }
    if axes >= 6 {
        out.push(("lefttrigger", "a4".to_string()));
        out.push(("righttrigger", "a5".to_string()));
    }
    out
}

/// One line per attached joystick describing exactly how SDL sees it.
/// Printed by `--pad-probe`: this is what separates "SDL never saw the pad"
/// from "SDL saw it and put the buttons somewhere unexpected", which need
/// completely different fixes.
pub fn describe_devices(gc: &GameControllerSubsystem, js: &JoystickSubsystem) -> Vec<String> {
    let mut lines = Vec::new();
    let n = match js.num_joysticks() {
        Ok(n) => n,
        Err(e) => return vec![format!("Could not enumerate joysticks: {e}")],
    };
    lines.push(format!("SDL sees {n} joystick(s)"));
    for i in 0..n {
        let name = js.name_for_index(i).unwrap_or_else(|_| "<unnamed>".into());
        lines.push(format!("[{i}] {name}"));
        match js.open(i) {
            Ok(j) => {
                let guid = j.guid();
                lines.push(format!("    guid    {}", guid.string()));
                lines.push(format!(
                    "    axes {}  buttons {}  hats {}",
                    j.num_axes(),
                    j.num_buttons(),
                    j.num_hats()
                ));
                match gc.mapping_for_guid(guid) {
                    Ok(m) => {
                        lines.push("    SDL mapping: yes".into());
                        lines.push(format!("      {m}"));
                    }
                    Err(_) => {
                        lines.push("    SDL mapping: NONE — would synthesize:".into());
                        lines.push(format!(
                            "      {}",
                            synthesize_mapping(
                                &guid.string(),
                                &name,
                                j.num_buttons(),
                                j.num_axes(),
                                j.num_hats()
                            )
                        ));
                    }
                }
            }
            Err(e) => lines.push(format!("    could not open: {e}")),
        }
        lines.push(format!(
            "    usable as game controller: {}",
            if gc.is_game_controller(i) { "yes" } else { "no" }
        ));
    }
    lines
}

pub fn pad_owner(pads: &Pads, which: u32) -> Option<Player> {
    for (i, slot) in pads.iter().enumerate() {
        if let Some(c) = slot {
            if c.instance_id() == which {
                return Some(if i == 0 { Player::P1 } else { Player::P2 });
            }
        }
    }
    None
}

/// Returns whether the pad actually took a slot, so callers can skip the
/// "controller connected" toast for a duplicate.
pub fn assign_pad(pads: &mut Pads, c: GameController) -> bool {
    // SDL announces a device present at startup through both the initial
    // enumeration and a queued ControllerDeviceAdded event. Without this
    // guard the same physical pad lands in P1 *and* P2 and drives both.
    if pads.iter().flatten().any(|p| p.instance_id() == c.instance_id()) {
        return false;
    }
    let slot_idx = if pads[0].is_none() {
        0
    } else if pads[1].is_none() {
        1
    } else {
        println!("Both pad slots full, ignoring new pad: {}", c.name());
        return false;
    };
    let name = c.name();
    pads[slot_idx] = Some(c);
    println!("Controller assigned to P{}: {}", slot_idx + 1, name);
    true
}

pub fn open_initial_controllers(gc: &GameControllerSubsystem, js: &JoystickSubsystem) -> Pads {
    let mut pads: Pads = [None, None];
    let n = match gc.num_joysticks() {
        Ok(n) => n,
        Err(_) => return pads,
    };
    for i in 0..n {
        match ensure_mapping(gc, js, i) {
            MappingOutcome::AlreadyMapped | MappingOutcome::Synthesized => match gc.open(i) {
                Ok(c) => {
                    assign_pad(&mut pads, c);
                }
                Err(e) => println!("Failed to open controller {i}: {e}"),
            },
            MappingOutcome::Unmapped => println!(
                "Joystick {i} ({}) is not usable as a game controller — skipping",
                js.name_for_index(i).unwrap_or_else(|_| "<unnamed>".into())
            ),
        }
        if pads[1].is_some() {
            break;
        }
    }
    if pads[0].is_none() {
        println!("No compatible controller at startup (hot-plug still supported)");
    }
    pads
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dualshock3_is_detected_by_the_names_its_drivers_report() {
        for name in [
            "PLAYSTATION(R)3 Controller",
            "Sony PLAYSTATION(R)3 Controller",
            "PS3 Controller",
            "DualShock 3",
            "SIXAXIS",
        ] {
            assert!(is_dualshock3(name), "{name} should be detected as a DS3");
        }
        for name in ["Xbox 360 Controller", "PS4 Controller", "8BitDo Pro 2"] {
            assert!(!is_dualshock3(name), "{name} should not match DS3");
        }
    }

    #[test]
    fn ds3_layout_puts_face_buttons_where_the_hid_report_has_them() {
        let m = synthesize_mapping("0300abcd", "PLAYSTATION(R)3 Controller", 19, 4, 1);
        // Cross is the bottom face button, so it is SDL's `a`.
        assert!(m.contains(",a:b14,"), "{m}");
        assert!(m.contains(",b:b13,"), "{m}");
        assert!(m.contains(",x:b15,"), "{m}");
        assert!(m.contains(",y:b12,"), "{m}");
        // Block's default binding rides TriggerRight, so R2 must be a
        // trigger and not a shoulder button.
        assert!(m.contains(",righttrigger:b9,"), "{m}");
        assert!(m.contains(",dpup:b4,"), "{m}");
    }

    #[test]
    fn a_ds3_reporting_too_few_controls_falls_back_to_the_generic_layout() {
        // A DS3 behind an XInput-emulating driver reports 11 buttons, and
        // guessing the 17-button HID order at it would be wrong.
        let m = synthesize_mapping("0300abcd", "PS3 Controller", 11, 6, 1);
        assert!(m.contains(",a:b0,"), "{m}");
        assert!(m.contains(",righttrigger:a5,"), "{m}");
    }

    #[test]
    fn generic_layout_never_references_controls_the_device_lacks() {
        let m = synthesize_mapping("0300abcd", "Cheap Arcade Stick", 6, 2, 0);
        assert!(!m.contains("b6"), "{m}");
        assert!(!m.contains("a2"), "{m}");
        assert!(!m.contains("trigger"), "{m}");
        // 6 buttons, no hat, no spare buttons for a D-pad — the sticks are
        // all the directional input there is.
        assert!(!m.contains("dpup"), "{m}");
    }

    #[test]
    fn hat_dpad_is_used_when_the_device_has_one() {
        let m = synthesize_mapping("0300abcd", "Generic Pad", 12, 4, 1);
        assert!(m.contains(",dpup:h0.1,"), "{m}");
        assert!(m.contains(",dpleft:h0.8,"), "{m}");
    }

    #[test]
    fn device_names_with_commas_cannot_split_the_mapping_record() {
        let m = synthesize_mapping("0300abcd", "Pad, Wireless", 12, 4, 1);
        assert!(m.starts_with("0300abcd,Pad  Wireless,"), "{m}");
    }
}
