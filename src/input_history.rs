use std::collections::VecDeque;

use crate::input::Action;

const MAX_ENTRIES: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputHistoryEntry {
    pub bits: u16,
    pub frames: u32,
}

#[derive(Clone, Debug)]
pub struct InputHistory {
    entries: VecDeque<InputHistoryEntry>,
    last_bits: Option<u16>,
}

impl InputHistory {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            last_bits: None,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.last_bits = None;
    }

    pub fn step(&mut self, bits: u16) {
        if self.last_bits == Some(bits) {
            if let Some(front) = self.entries.front_mut() {
                front.frames = front.frames.saturating_add(1);
            }
            return;
        }

        self.last_bits = Some(bits);
        self.entries
            .push_front(InputHistoryEntry { bits, frames: 1 });
        while self.entries.len() > MAX_ENTRIES {
            self.entries.pop_back();
        }
    }

    pub fn entries(&self) -> impl Iterator<Item = &InputHistoryEntry> {
        self.entries.iter()
    }
}

impl Default for InputHistory {
    fn default() -> Self {
        Self::new()
    }
}

pub fn format_bits(bits: u16) -> String {
    let mut parts = Vec::new();
    let direction = direction_label(bits);
    if direction != "N" {
        parts.push(direction);
    }
    for (action, label) in [
        (Action::HighPunch, "HP"),
        (Action::LowPunch, "LP"),
        (Action::HighKick, "HK"),
        (Action::LowKick, "LK"),
        (Action::Block, "BL"),
        (Action::Start, "ST"),
        (Action::Coin, "CN"),
    ] {
        if pressed(bits, action) {
            parts.push(label);
        }
    }
    if parts.is_empty() {
        "N".into()
    } else {
        parts.join("+")
    }
}

fn direction_label(bits: u16) -> &'static str {
    let up = pressed(bits, Action::Up);
    let down = pressed(bits, Action::Down);
    let left = pressed(bits, Action::Left);
    let right = pressed(bits, Action::Right);
    match (
        if up {
            Some("U")
        } else if down {
            Some("D")
        } else {
            None
        },
        if left {
            Some("L")
        } else if right {
            Some("R")
        } else {
            None
        },
    ) {
        (Some(v), Some(h)) if v == "U" && h == "L" => "UL",
        (Some(v), Some(h)) if v == "U" && h == "R" => "UR",
        (Some(v), Some(h)) if v == "D" && h == "L" => "DL",
        (Some(v), Some(h)) if v == "D" && h == "R" => "DR",
        (Some("U"), None) => "U",
        (Some("D"), None) => "D",
        (None, Some("L")) => "L",
        (None, Some("R")) => "R",
        _ => "N",
    }
}

fn pressed(bits: u16, action: Action) -> bool {
    let idx = Action::ALL
        .iter()
        .position(|candidate| *candidate == action)
        .unwrap_or(0);
    bits & (1u16 << idx) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bit(action: Action) -> u16 {
        let idx = Action::ALL
            .iter()
            .position(|candidate| *candidate == action)
            .unwrap();
        1u16 << idx
    }

    #[test]
    fn repeated_inputs_accumulate_frames() {
        let mut history = InputHistory::new();
        history.step(bit(Action::Down));
        history.step(bit(Action::Down));
        history.step(bit(Action::Down) | bit(Action::LowPunch));
        let entries: Vec<_> = history.entries().collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].frames, 1);
        assert_eq!(entries[1].frames, 2);
    }

    #[test]
    fn formats_compact_fighting_inputs() {
        assert_eq!(
            format_bits(bit(Action::Down) | bit(Action::Right) | bit(Action::HighPunch)),
            "DR+HP"
        );
        assert_eq!(format_bits(bit(Action::Up)), "U");
        assert_eq!(format_bits(bit(Action::Up) | bit(Action::Down)), "U");
        assert_eq!(format_bits(0), "N");
    }
}
