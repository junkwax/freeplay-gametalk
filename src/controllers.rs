//! Two-slot SDL game controller management. `pads[0]` is P1's pad,
//! `pads[1]` is P2's. Hot-plug is handled via `assign_pad`; ownership of an
//! already-assigned pad is looked up by SDL instance ID.

use crate::input::Player;
use sdl2::controller::GameController;

/// Up to two open controllers: pads[0] = P1's pad, pads[1] = P2's pad.
pub type Pads = [Option<GameController>; 2];

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

pub fn assign_pad(pads: &mut Pads, c: GameController) {
    let slot_idx = if pads[0].is_none() {
        0
    } else if pads[1].is_none() {
        1
    } else {
        println!("Both pad slots full, ignoring new pad: {}", c.name());
        return;
    };
    let name = c.name();
    pads[slot_idx] = Some(c);
    println!("Controller assigned to P{}: {}", slot_idx + 1, name);
}

pub fn open_initial_controllers(subsystem: &sdl2::GameControllerSubsystem) -> Pads {
    let mut pads: Pads = [None, None];
    let n = match subsystem.num_joysticks() {
        Ok(n) => n,
        Err(_) => return pads,
    };
    for i in 0..n {
        if subsystem.is_game_controller(i) {
            match subsystem.open(i) {
                Ok(c) => assign_pad(&mut pads, c),
                Err(e) => println!("Failed to open controller {i}: {e}"),
            }
        } else {
            println!("Joystick {i} has no SDL GameController mapping — skipping");
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
