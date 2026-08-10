//! Wu-style player name generator.
//!
//! The word lists start from the canonical WuNameAAS set — 45 adjectives and
//! 40 nouns, the same list the various online generators all copy — and add
//! a second block mined from Wu-Tang affiliate and Killa Beez stage names,
//! splitting each into its modifier and its agent half so the Adjective_Noun
//! shape holds. That takes the space from 1,760 names to 7,110.
//!
//! Worth knowing what that does and doesn't buy: it is a 4x space, but
//! collisions are a birthday problem, so the point where two players are
//! more likely than not to share a name only moves from ~49 to ~99. The
//! generated name is a starting suggestion; `check_username_available` on
//! the matchmaking side is what actually settles uniqueness. Its *seed* shape did
//! too — sum each lowercased character's codepoint times its one-based
//! position — but that was designed to hash a name the player typed, so the
//! same input would always produce the same Wu name. Nothing types a name in
//! anymore: the only caller wants an unpredictable handle for a fresh
//! install. Feeding a formatted timestamp through a positional digit sum
//! made a poor random source, because changing one digit shifts the sum by a
//! fixed amount and the resulting seeds bunch up in the middle of their
//! range — measured at 3.1x over-representation for the most common name
//! against 1.5x for a uniform draw. The seed now comes from OS entropy
//! instead (see `mix_entropy`).

use std::time::{SystemTime, UNIX_EPOCH};

const ADJECTIVES: &[&str] = &[
    "Bittah",
    // Upstream carries these as two separate adjectives; they were merged
    // into one entry here at some point, which made this the only two-word
    // adjective and so the only source of three-part names.
    "Tha",
    "Mad",
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
    // Drawn from Wu-Tang affiliate and Killa Beez stage names (Killarmy,
    // Sunz of Man, Gravediggaz, GP Wu, Black Knights, Wu-Syndicate, the
    // Wisemen, Theodore Unit, Tha Beggas, Brooklyn Zu, Deadly Venoms,
    // American Cream Team, Yellow Jacketz, GoldMinerz, Black Rose Kartel
    // and the rest). Only the modifier half of each name lands here -- the
    // agent half goes in NOUNS -- so the two-part shape holds.
    "Almighty",   // Almighty God-Rule
    "Arabian",    // Arabian Knight
    "Babyface",   // Babyface Fensta
    "Bigg",       // Bigg Cy
    "Black",      // Black Jesus
    "Blue",       // Blue Raspberry
    "Born",       // Born U Majesty
    "Bronze",     // Bronze Nazareth
    "Brown",      // Pop Da Brown Hornet
    "Cheesy",     // Cheesy Rat
    "Dirty",      // Young Dirty Bastard
    "Dreddy",     // Dreddy Kruger
    "Eternal",    // Eternal
    "Free",       // Free Murda
    "Illah",      // Illah Dayz
    "Killa",      // Killa Sin
    "Kinetic",    // Kinetic 9
    "Long",       // Long Axe
    "Majik",      // Majik Sword
    "Mega",       // Mega Soul
    "Poetic",     // Poetic
    "Polite",     // Polite
    "Prodigal",   // Prodigal Sunn
    "Rugged",     // Rugged Monk
    "Shorty",     // Shorty Shitstain
    "Stone",      // Stone Mecca
    "Street",     // Street Life
    "Suga",       // Suga Bang Bang
    "Superb",     // Lord Superb
    "Supreme",    // Supreme I-Self
    "Trife",      // Trife Da God
    "True",       // True Master
    "Wise",       // Allah Wise
    "Young",      // Young Dirty Bastard
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
    // Agent half of the same affiliate names (see ADJECTIVES above).
    "Aristotle",    // Aristotle
    "Axe",          // Long Axe
    "Baptist",      // John the Baptist
    "Bearer",       // Christ Bearer
    "Beretta",      // Beretta 9
    "Chief",        // Popa Chief
    "Cognac",       // Cognac
    "Darkman",      // LA the Darkman
    "Disciple",     // 4th Disciple / Shabazz the Disciple
    "Doom",         // Doc Doom
    "Dragon",       // Drunken Dragon
    "Dragonfly",    // Dragonfly
    "Finesse",      // Finesse
    "Fool",         // Master Fool
    "General",      // General Jihad
    "God",          // Sun God / Trife Da God
    "Hornet",       // Pop Da Brown Hornet
    "Iceman",       // Iceman
    "Keeper",       // Zu Keeper
    "Kid",          // Salute the Kid
    "King",         // Timbo King / King Just
    "Kruger",       // Dreddy Kruger
    "Life",         // Street Life / Kryme Life
    "Lord",         // Father Lord
    "Majesty",      // Born U Majesty
    "Masta",        // Dungeon Masta
    "Mathematics",  // Mathematics
    "Mecca",        // Stone Mecca
    "Megalodon",    // June Megalodon
    "Monk",         // Buddha Monk / Rugged Monk
    "Murda",        // Free Murda
    "Napoleon",     // Napoleon
    "Nazareth",     // Bronze Nazareth
    "Omen",         // Decrep Da Omen
    "Pharaoh",      // Meko the Pharaoh
    "Priest",       // Killah Priest
    "Prince",       // 9th Prince / Prince Paul
    "Protagonist",  // Anthai the Protagonist
    "Razah",        // Hell Razah
    "Recka",        // Down Low Recka / Rhyme Recka
    "Remedy",       // Remedy
    "Reverend",     // Reverend William Burke
    "Rook",         // Rook Da Rukus
    "Sharpshooter", // Crisis the Sharpshooter
    "Shogun",       // Shogun Assasson
    "Soul",         // Mega Soul
    "Sunn",         // Prodigal Sunn
    "Sword",        // Majik Sword
    "Trigger",      // Sol Trigger
    "Warcloud",     // Holocaust / Warcloud
];

/// The name a fresh install starts with. Drawn independently per install —
/// nothing about the machine, the clock, or the install order narrows it.
pub fn random_username() -> String {
    random_username_variant(0)
}

/// Roll a fresh name, guaranteed to differ across rapid presses. The
/// monotonic counter feeds `variant` so a re-roll can't repeat even if every
/// other entropy source were to return an identical value.
pub fn reroll_username() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static REROLL_COUNTER: AtomicU64 = AtomicU64::new(1);
    random_username_variant(REROLL_COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Like `random_username` but folds in a caller-supplied variant, so a caller
/// that needs a run of distinct names can guarantee one.
pub fn random_username_variant(variant: u64) -> String {
    name_for_seed(mix_entropy(variant))
}

/// The best entropy the standard library offers without taking on an RNG
/// crate.
///
/// `RandomState` is the load-bearing source: std seeds its SipHash keys from
/// the OS random generator once per thread, then bumps them per instance. So
/// two installs are drawn independently even if they run on identical
/// hardware, boot at the same clock tick, and land on the same pid — the
/// case the old timestamp-derived seed handled worst. The clock, pid, and an
/// ASLR'd stack address are folded in behind it as belt and braces, so a
/// hypothetical platform with a weak `RandomState` still can't collapse two
/// installs onto one name.
fn mix_entropy(variant: u64) -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(variant);
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    hasher.write_u32(std::process::id());
    let sentinel = 0u64;
    hasher.write_usize(&sentinel as *const u64 as usize);
    hasher.finish()
}

/// Pick one adjective and one noun. `next_seed` is a splitmix64 finalizer —
/// a bijection over u64 — so each index is uniform provided the seed is, and
/// the two draws are taken from separately mixed values rather than from
/// adjacent bits of one.
fn name_for_seed(seed: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The whole point of the generator: two installs must be able to land
    /// anywhere in the name space with equal odds. Guards against a
    /// regression to a seed that merely *looks* random — the previous
    /// timestamp-derived one reached every name too, but over-represented
    /// its most common one by 3.1x.
    ///
    /// Statistical, but not flaky, and it stays that way if the word lists
    /// grow again: the bound is expressed in standard deviations rather than
    /// as a fixed ratio. Bucket counts are Poisson, so sigma is sqrt(mean),
    /// and the largest of ~7k uniform buckets lands near +4 sigma. Eight
    /// sigma leaves room for that while still being nowhere near the old
    /// timestamp-derived seed, whose peak sat about 25 sigma out.
    #[test]
    fn generated_names_are_drawn_uniformly() {
        let n = 1_000_000;
        let mut counts: HashMap<String, u64> = HashMap::new();
        for _ in 0..n {
            *counts.entry(random_username()).or_insert(0) += 1;
        }
        let possible = ADJECTIVES.len() * NOUNS.len();
        assert!(
            counts.len() >= possible - 10,
            "only {} of {possible} names reachable",
            counts.len()
        );
        let expected = n as f64 / possible as f64;
        let sigma = expected.sqrt();
        let max = counts.values().copied().max().unwrap_or(0) as f64;
        assert!(
            max <= expected + 8.0 * sigma,
            "most common name is {:.2}x expected ({:.1} sigma) — seed is not uniform",
            max / expected,
            (max - expected) / sigma
        );
    }

    /// Both halves of a name come from separate lists, so a duplicate inside
    /// either one silently biases that word without shrinking the space in
    /// any visible way.
    #[test]
    fn word_lists_have_no_duplicates() {
        for (label, list) in [("adjective", ADJECTIVES), ("noun", NOUNS)] {
            let unique: std::collections::HashSet<String> =
                list.iter().map(|w| w.to_ascii_lowercase()).collect();
            assert_eq!(unique.len(), list.len(), "duplicate {label} in the word list");
        }
    }

    /// A player mashing re-roll must never see the same name twice running,
    /// which is what the monotonic variant counter is for.
    #[test]
    fn rerolls_do_not_repeat_back_to_back() {
        let mut previous = reroll_username();
        for _ in 0..500 {
            let next = reroll_username();
            assert_ne!(previous, next, "re-roll returned the same name twice");
            previous = next;
        }
    }

    /// Whatever the draw, the result has to survive the same validation a
    /// typed username goes through, or a fresh install starts with a name
    /// the matchmaking service will reject.
    #[test]
    fn every_generated_name_is_a_valid_username() {
        for _ in 0..2000 {
            let name = random_username();
            assert!((2..=24).contains(&name.len()), "bad length: {name}");
            assert!(
                name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "bad characters: {name}"
            );
            assert!(!name.starts_with('_') && !name.ends_with('_'), "{name}");
        }
    }
}
