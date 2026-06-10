//! Wu-style player name generator.
//!
//! The word lists and seed shape mirror the older WuNameAAS generator:
//! sum each lowercased input character's codepoint multiplied by its
//! one-based position, then use that seed to pick one adjective and one noun.

use std::time::{SystemTime, UNIX_EPOCH};

const ADJECTIVES: &[&str] = &[
    "Bittah",
    "Tha_Mad",
    "Master",
    "Dynamic",
    "E-ratic",
    "Wacko",
    "Fearless",
    "Misunderstood",
    "Quiet",
    "Pesty",
    "Gentlemen",
    "Profound",
    "Respected",
    "Amateur",
    "Shriekin",
    "Lucky",
    "Phantom",
    "Smilin",
    "Thunderous",
    "Tuff",
    "Scratchin",
    "Drunken",
    "X-cessive",
    "X-pert",
    "Zexy",
    "Ruff",
    "Intellectual",
    "Unlucky",
    "Vizual",
    "Foolish",
    "Midnight",
    "Mighty",
    "Violent",
    "Vulgar",
    "Crazy",
    "Annoyin",
    "Arrogant",
    "B-loved",
    "Sarkastik",
    "Insane",
    "Irate",
    "Wicked",
    "Lazy-assed",
    "Amazing",
];

const NOUNS: &[&str] = &[
    "Madman",
    "Genius",
    "Hunter",
    "Killah",
    "Professional",
    "Artist",
    "Dreamer",
    "Observer",
    "Bastard",
    "Wizard",
    "Swami",
    "Wanderer",
    "Assassin",
    "Bandit",
    "Leader",
    "Ambassador",
    "Warrior",
    "Menace",
    "Worlock",
    "Conqueror",
    "Lover",
    "Magician",
    "Desperado",
    "Specialist",
    "Mercenary",
    "Ninja",
    "Contender",
    "Mastermind",
    "Demon",
    "Watcher",
    "Destroyer",
    "Beggar",
    "Commander",
    "Dominator",
    "Overlord",
    "Samurai",
    "Knight",
    "Pupil",
    "Prophet",
    "Criminal",
];

pub fn random_username() -> String {
    random_username_nonce(0)
}

/// Like `random_username` but mixes in a caller-supplied nonce so successive
/// calls yield distinct names even when the system clock resolution is too
/// coarse to differ between calls (notably on Windows, where SystemTime can
/// be stuck at the same value across a tight regenerate loop).
pub fn random_username_nonce(nonce: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seed_text = format!("{}-{}-{}-{:p}", now, std::process::id(), nonce, &now);
    name_for_seed_text(&seed_text)
}

fn name_for_seed_text(input: &str) -> String {
    let seed = input
        .to_ascii_lowercase()
        .chars()
        .enumerate()
        .fold(0u64, |acc, (i, ch)| {
            acc.wrapping_add((ch as u64).wrapping_mul(i as u64 + 1))
        })
        .max(1);
    let adj_idx = next_seed(seed) as usize % ADJECTIVES.len();
    let noun_idx = next_seed(seed ^ 0x9e37_79b9_7f4a_7c15) as usize % NOUNS.len();
    sanitize_generated(&format!("{}_{}", ADJECTIVES[adj_idx], NOUNS[noun_idx]))
        .unwrap_or_else(|| "Lucky_Killah".into())
}

fn next_seed(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn sanitize_generated(raw: &str) -> Option<String> {
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else if c.is_whitespace() && !out.ends_with('_') {
            out.push('_');
        }
        if out.len() >= 24 {
            break;
        }
    }
    let out = out.trim_matches('_').to_string();
    (out.len() >= 2).then_some(out)
}
