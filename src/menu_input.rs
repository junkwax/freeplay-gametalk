//! Translation layer between SDL events and menu navigation intent.
//! Pure transformation — no state, no side effects. Lives in its own module
//! so the main event loop can stay focused on dispatch rather than key
//! detection.

use crate::input::{self, Binding};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;

pub enum MenuNav {
    Up,
    Down,
    Accept,
    Back,
    ToggleMenu,
    SwitchPlayer,
}

pub fn event_to_menu_nav(ev: &Event) -> Option<MenuNav> {
    use sdl2::controller::Button;
    match ev {
        Event::KeyDown {
            keycode: Some(k), ..
        } => match k {
            Keycode::Up => Some(MenuNav::Up),
            Keycode::Down => Some(MenuNav::Down),
            Keycode::Return | Keycode::KpEnter => Some(MenuNav::Accept),
            Keycode::Escape => Some(MenuNav::Back),
            Keycode::F1 => Some(MenuNav::ToggleMenu),
            Keycode::Tab | Keycode::Left | Keycode::Right => Some(MenuNav::SwitchPlayer),
            _ => None,
        },
        Event::ControllerButtonDown { button, .. } => match button {
            Button::DPadUp => Some(MenuNav::Up),
            Button::DPadDown => Some(MenuNav::Down),
            Button::DPadLeft | Button::DPadRight => Some(MenuNav::SwitchPlayer),
            Button::A | Button::Start => Some(MenuNav::Accept),
            Button::B | Button::Back => Some(MenuNav::Back),
            _ => None,
        },
        _ => None,
    }
}

pub fn capture_rebind(ev: &Event) -> Option<Binding> {
    match ev {
        Event::KeyDown {
            keycode: Some(k),
            repeat: false,
            ..
        } => {
            if *k == Keycode::Escape {
                return None;
            }
            Some(Binding::Key {
                key: input::key_name(*k),
            })
        }
        Event::ControllerButtonDown { button, .. } => Some(Binding::PadButton {
            button: input::button_name(*button),
        }),
        Event::ControllerAxisMotion { axis, value, .. } => {
            let threshold = input::STICK_DEADZONE * 2;
            if *value > threshold {
                Some(Binding::PadAxis {
                    axis: input::axis_name(*axis),
                    positive: true,
                })
            } else if *value < -threshold {
                Some(Binding::PadAxis {
                    axis: input::axis_name(*axis),
                    positive: false,
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn is_cancel(ev: &Event) -> bool {
    matches!(
        ev,
        Event::KeyDown {
            keycode: Some(Keycode::Escape),
            ..
        }
    )
}

pub fn is_clear(ev: &Event) -> bool {
    matches!(
        ev,
        Event::KeyDown {
            keycode: Some(Keycode::Delete),
            ..
        } | Event::KeyDown {
            keycode: Some(Keycode::Backspace),
            ..
        }
    )
}
