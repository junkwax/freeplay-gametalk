//! SDL event -> fp_ui navigation intent. Mirrors `crate::menu_input`'s
//! event-to-intent style but adds the L1/R1 tab-cycle intents the new
//! screens need (lobby tabs, settings categories) that `MenuNav` doesn't
//! have. Keyboard arrows/Enter/Escape are kept as a dev-only fallback per
//! the handoff doc; real navigation is controller D-pad/face-buttons.

use sdl2::controller::Button;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FpNav {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Back,
    PrevTab,
    NextTab,
}

pub fn event_to_fp_nav(ev: &Event) -> Option<FpNav> {
    match ev {
        Event::KeyDown {
            keycode: Some(k), ..
        } => match k {
            Keycode::Up => Some(FpNav::Up),
            Keycode::Down => Some(FpNav::Down),
            Keycode::Left => Some(FpNav::Left),
            Keycode::Right => Some(FpNav::Right),
            Keycode::Return | Keycode::KpEnter => Some(FpNav::Confirm),
            Keycode::Escape => Some(FpNav::Back),
            Keycode::PageUp => Some(FpNav::PrevTab),
            Keycode::PageDown => Some(FpNav::NextTab),
            _ => None,
        },
        Event::ControllerButtonDown { button, .. } => match button {
            Button::DPadUp => Some(FpNav::Up),
            Button::DPadDown => Some(FpNav::Down),
            Button::DPadLeft => Some(FpNav::Left),
            Button::DPadRight => Some(FpNav::Right),
            Button::A => Some(FpNav::Confirm),
            Button::B => Some(FpNav::Back),
            Button::LeftShoulder => Some(FpNav::PrevTab),
            Button::RightShoulder => Some(FpNav::NextTab),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_confirm_and_back() {
        let confirm = Event::ControllerButtonDown {
            timestamp: 0,
            which: 0,
            button: Button::A,
        };
        let back = Event::ControllerButtonDown {
            timestamp: 0,
            which: 0,
            button: Button::B,
        };
        assert_eq!(event_to_fp_nav(&confirm), Some(FpNav::Confirm));
        assert_eq!(event_to_fp_nav(&back), Some(FpNav::Back));
    }

    #[test]
    fn maps_shoulders_to_tab_cycle() {
        let l1 = Event::ControllerButtonDown {
            timestamp: 0,
            which: 0,
            button: Button::LeftShoulder,
        };
        let r1 = Event::ControllerButtonDown {
            timestamp: 0,
            which: 0,
            button: Button::RightShoulder,
        };
        assert_eq!(event_to_fp_nav(&l1), Some(FpNav::PrevTab));
        assert_eq!(event_to_fp_nav(&r1), Some(FpNav::NextTab));
    }

    #[test]
    fn ignores_unmapped_buttons() {
        let y = Event::ControllerButtonDown {
            timestamp: 0,
            which: 0,
            button: Button::Y,
        };
        assert_eq!(event_to_fp_nav(&y), None);
    }
}
