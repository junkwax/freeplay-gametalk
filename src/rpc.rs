//! Discord Rich Presence integration.
//!
//! Connects to the local Discord desktop client via IPC and updates the
//! user's activity status based on the current app state (menu, playing,
//! matchmaking, training with ghosts, netplay vs opponent, etc.).
//!
//! ## Interactive features
//!
//!   - "Join" button on the profile card when in Training mode (spar invite)
//!   - Party info during netplay (1/2 or 2/2)
//!   - Elapsed-time timestamps during active play
//!   - State-specific Rich Presence art assets
//!   - Listens for `ACTIVITY_JOIN` events from Discord
//!
//! ## Discord Developer Portal setup
//!
//! 1. Go to https://discord.com/developers/applications → Freeplay app
//! 2. **Rich Presence → Art Assets** (upload these images):
//!    - Key `freeplay` — Freeplay app art
//!    - Key `training` — training mode icon (small, ~128×128)
//!    - Key `netplay` — online match icon
//!    - Key `matchmaking` — searching/queue icon
//!    - Key `ghost` — ghost playback/recording icon
//!    - Key `spectate` — watch/spectate icon
//! 3. **Rich Presence** — enable "Rich Presence" feature
//! 4. **OAuth2** — add Redirect `http://localhost:19420` for token callback
//! 5. **Installation** — set "Discord Public Key" if using Activity Join
//!
//! ## Join-to-spar flow
//!
//! When a player enters training mode, a unique `xband://join/<room>` URL is
//! advertised via the Discord "Join" button. If a friend clicks it:
//!
//!   1. Discord delivers the join secret to this app via IPC
//!   2. `on_activity_join` parses the room ID
//!   3. The room ID is routed into the matchmaking flow so both players connect
//!      through the GGRS session.
//!
//! The discord-presence crate runs its own background thread for the IPC
//! connection. State updates are fire-and-forget — if Discord isn't running
//! the update call is silently ignored.

use discord_presence::client::ClientThread;
use discord_presence::Client as DiscordClient;
use std::sync::{Mutex, OnceLock};

/// Set by the on_activity_join callback when a friend clicks "Join".
/// The main loop reads this each frame and routes into matchmaking.
static JOIN_REQUEST: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static SPECTATE_REQUEST: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static DISCORD_CLIENT_ID: OnceLock<Mutex<Option<String>>> = OnceLock::new();

pub fn set_discord_client_id(id: String) {
    let cell = DISCORD_CLIENT_ID.get_or_init(|| Mutex::new(None));
    *cell.lock().unwrap() = Some(id);
}

fn join_slot() -> &'static Mutex<Option<String>> {
    JOIN_REQUEST.get_or_init(|| Mutex::new(None))
}

fn spectate_slot() -> &'static Mutex<Option<String>> {
    SPECTATE_REQUEST.get_or_init(|| Mutex::new(None))
}

/// Called from the main loop to check if a join request is pending.
/// Returns and consumes the room ID if one was received from Discord.
pub fn take_join_request() -> Option<String> {
    join_slot().lock().ok()?.take()
}

/// Called from the main loop to check if a spectate request is pending.
/// Returns and consumes the session ID if one was received from Discord.
pub fn take_spectate_request() -> Option<String> {
    spectate_slot().lock().ok()?.take()
}

/// Seed the join slot from a non-IPC source (currently: an `xband://join/<id>`
/// URI passed on the command line by the OS shell). The main loop's existing
/// drain in `take_join_request` then routes it into matchmaking the same way
/// a Discord-IPC click would.
pub fn post_join_request(room_id: String) {
    if let Ok(mut slot) = join_slot().lock() {
        *slot = Some(room_id);
    }
}

pub fn post_spectate_request(session_id: String) {
    if let Ok(mut slot) = spectate_slot().lock() {
        *slot = Some(session_id);
    }
}

/// Wraps the discord-presence Client and its background thread.
pub struct RpcClient {
    client: DiscordClient,
    thread: Option<ClientThread>,
    last_update: Option<RpcUpdate>,
    timer_key: Option<String>,
    timer_started_at: Option<u64>,
}

/// What kind of activity the user is engaged in.
#[derive(Clone, PartialEq, Eq)]
pub enum RpcState {
    Menu,
    Playing,
    Training,
    Matchmaking,
    #[allow(dead_code)]
    Hosting,
    Joining,
    Netplay,
    NetplayVs(String),
}

/// Full payload for an activity update. Equality on the entire struct
/// determines whether we actually send an IPC message (dedup).
#[derive(Clone, PartialEq, Eq)]
pub struct RpcUpdate {
    pub state: RpcState,
    pub ghost_recording: bool,
    pub ghost_playback: bool,
    pub join_key: Option<String>,
    pub spectate_key: Option<String>,
    pub party_id: Option<String>,
    pub party: Option<(u32, u32)>,
    pub score: Option<(u16, u16)>,
}

impl Default for RpcUpdate {
    fn default() -> Self {
        Self {
            state: RpcState::Menu,
            ghost_recording: false,
            ghost_playback: false,
            join_key: None,
            spectate_key: None,
            party_id: None,
            party: None,
            score: None,
        }
    }
}

impl RpcClient {
    /// Initialize the Rich Presence client. Returns None if the Discord
    /// desktop client is not running (IPC pipe unavailable).
    pub fn init() -> Option<Self> {
        let Some(client_id) = client_id() else {
            println!("[rpc] FREEPLAY_DISCORD_CLIENT_ID not configured; Rich Presence disabled");
            return None;
        };
        let mut client = DiscordClient::new(client_id);
        let thread = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| client.start()))
        {
            Ok(t) => t,
            Err(_) => {
                eprintln!("[rpc] Failed to start Discord IPC — Discord may not be running");
                return None;
            }
        };
        println!("[rpc] Discord Rich Presence started");

        // Listen for activity join (friend clicks "Join" or "Ask to Join")
        client.on_activity_join(|ctx| {
            let secret = ctx
                .event
                .get("secret")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            println!("[rpc] Activity join requested — secret: {secret}");
            if let Some(room_id) = secret.strip_prefix("xband://join/") {
                println!("[rpc] → Spar room join: room_id={room_id}");
                if let Ok(mut slot) = spectate_slot().lock() {
                    *slot = Some(room_id.to_string());
                }
            }
        });

        // Listen for activity spectate (friend clicks "Spectate")
        client.on_activity_spectate(|ctx| {
            let secret = ctx
                .event
                .get("secret")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            println!("[rpc] Spectate requested — secret: {secret}");
            if let Some(room_id) = secret.strip_prefix("xband://watch/") {
                println!("[rpc] → Watch match: session_id={room_id}");
                if let Ok(mut slot) = join_slot().lock() {
                    *slot = Some(room_id.to_string());
                }
            }
        });

        Some(Self {
            client,
            thread: Some(thread),
            last_update: None,
            timer_key: None,
            timer_started_at: None,
        })
    }

    /// Update the activity shown on Discord. Only sends a network message
    /// if the payload actually changed to avoid spamming the IPC pipe.
    /// The actual IPC call happens on a fire-and-forget thread so it never
    /// blocks the main loop.
    pub fn update(&mut self, u: RpcUpdate) {
        if !DiscordClient::is_ready() {
            return;
        }
        if self.last_update.as_ref() == Some(&u) {
            return;
        }
        let (details, state_str) = self.details_for(&u);
        let timestamps = self.timestamps_for(&u);
        let small_asset = self.small_asset_for(&u);
        let join_key = u.join_key.clone();
        let spectate_key = u.spectate_key.clone();
        let party_id = u.party_id.clone();
        let party = u.party;
        let is_training = u.state == RpcState::Training;

        self.last_update = Some(u);

        let mut client = self.client.clone();
        std::thread::spawn(move || {
            use discord_presence::models::rich_presence::Activity;

            let mut act = Activity::new().details(&details).instance(true);

            if !state_str.is_empty() {
                act = act.state(&state_str);
            }

            if let Some((start, _end)) = timestamps {
                act = act.timestamps(|t| t.start(start));
            }

            if is_training || small_asset.is_some() {
                act = act.assets(|a| {
                    let mut a = a.large_image("freeplay").large_text("Freeplay");
                    if let Some((key, text)) = small_asset {
                        a = a.small_image(key).small_text(text);
                    }
                    a
                });
            } else {
                act = act.assets(|a| a.large_image("freeplay").large_text("Freeplay"));
            }

            // Join: during training, let friends spar vs you
            // Spectate: during netplay, let friends watch the match
            if join_key.is_some() || spectate_key.is_some() {
                let mut s = discord_presence::models::rich_presence::ActivitySecrets::new();
                if let Some(ref k) = join_key {
                    s = s.join(k.as_str());
                }
                if let Some(ref k) = spectate_key {
                    s = s.spectate(k.as_str());
                }
                act = act.secrets(|_| s);
            }

            // Party: group matches together, use session ID as party ID.
            // Discord is happier with a concrete party id when join/spectate
            // secrets are present, so training uses the spar key as the party id.
            if let Some(ref pid) = party_id {
                let (cur, max) = party.unwrap_or((1, 2));
                act = act.party(|p| p.id(pid.as_str()).size((cur, max)));
            } else if let Some(ref key) = join_key {
                act = act.party(|p| p.id(key.as_str()).size((1, 2)));
            } else if let Some((cur, max)) = party {
                act = act.party(|p| p.size((cur, max)));
            }

            match client.set_activity(|_| act) {
                Ok(_) => println!("[rpc] State updated: {state_str}"),
                Err(e) => eprintln!("[rpc] Failed to set activity: {e}"),
            }
        });
    }

    fn details_for(&self, u: &RpcUpdate) -> (String, String) {
        let state = &u.state;

        let details = match state {
            RpcState::Menu => "In Lobby",
            RpcState::Playing => "Practice Mode",
            RpcState::Training => "Practice Mode",
            RpcState::Matchmaking => "Searching for opponent",
            RpcState::Hosting => "Hosting a match",
            RpcState::Joining => "Joining a match",
            RpcState::Netplay => "Netplay Match",
            RpcState::NetplayVs(_) => "Netplay Match",
        };

        let mut state_str = match state {
            RpcState::Menu => String::new(),
            RpcState::Playing => "Offline practice".into(),
            RpcState::Training => "Ghost training".into(),
            RpcState::Matchmaking => "In queue".into(),
            RpcState::Hosting => "Waiting for opponent".into(),
            RpcState::Joining => "Connecting...".into(),
            RpcState::Netplay => {
                if let Some((p1, p2)) = u.score {
                    format!("Online | {p1}-{p2}")
                } else {
                    "Online".into()
                }
            }
            RpcState::NetplayVs(n) => {
                if let Some((p1, p2)) = u.score {
                    format!("vs {n} | {p1}-{p2}")
                } else {
                    format!("vs {n}")
                }
            }
        };

        // Append ghost status for training log
        if u.ghost_recording {
            state_str.push_str(" | Recording");
        }
        if u.ghost_playback {
            state_str.push_str(" | Ghost playback");
        }

        (details.to_string(), state_str)
    }

    fn timestamps_for(&mut self, u: &RpcUpdate) -> Option<(u64, u64)> {
        match &u.state {
            RpcState::Playing | RpcState::Training | RpcState::Netplay | RpcState::NetplayVs(_) => {
                let key = timer_key(u);
                if self.timer_key.as_deref() != Some(key.as_str()) {
                    self.timer_key = Some(key);
                    self.timer_started_at = Some(now_secs());
                }
                self.timer_started_at.map(|start| (start, 0))
            }
            _ => {
                self.timer_key = None;
                self.timer_started_at = None;
                None
            }
        }
    }

    fn small_asset_for(&self, u: &RpcUpdate) -> Option<(&'static str, &'static str)> {
        if u.ghost_playback || u.ghost_recording {
            return Some(("ghost", "Ghost Training"));
        }
        match &u.state {
            RpcState::Training => Some(("training", "Training Mode")),
            RpcState::Matchmaking => Some(("matchmaking", "Finding Match")),
            RpcState::Netplay | RpcState::NetplayVs(_) => Some(("netplay", "Online Match")),
            RpcState::Joining => Some(("matchmaking", "Connecting")),
            _ => None,
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn timer_key(u: &RpcUpdate) -> String {
    match &u.state {
        RpcState::NetplayVs(name) => {
            format!("netplay:{name}:{}", u.party_id.as_deref().unwrap_or(""))
        }
        RpcState::Netplay => format!("netplay:{}", u.party_id.as_deref().unwrap_or("")),
        RpcState::Training => format!("training:{}", u.join_key.as_deref().unwrap_or("")),
        RpcState::Playing => "playing".into(),
        RpcState::Matchmaking => "matchmaking".into(),
        RpcState::Hosting => "hosting".into(),
        RpcState::Joining => "joining".into(),
        RpcState::Menu => "menu".into(),
    }
}

fn client_id() -> Option<u64> {
    if let Some(v) = crate::config::env_value("FREEPLAY_DISCORD_CLIENT_ID") {
        return v.parse::<u64>().ok();
    }
    let cell = DISCORD_CLIENT_ID.get()?;
    cell.lock().ok()?.as_ref()?.parse::<u64>().ok()
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.stop();
            println!("[rpc] Discord Rich Presence stopped");
        }
    }
}

/// Generate a unique spar key for the "Join" button. Format: `xband://join/<hex>`.
/// Combines system-time nanos with two `RandomState` hashes (seeded from the OS
/// CSPRNG on construction) so the key isn't predictable to anyone watching the
/// clock. The key is broadcast via Rich Presence anyway, so this is about
/// preventing guess-and-claim, not secrecy.
pub fn make_spar_key() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    // Each RandomState pulls fresh OS entropy. Hashing a fixed input gives us
    // a deterministic-per-state-but-OS-random u64.
    let mut h1 = RandomState::new().build_hasher();
    h1.write_u64(nanos);
    let r1 = h1.finish();

    let mut h2 = RandomState::new().build_hasher();
    h2.write_u64(r1);
    let r2 = h2.finish();

    format!("xband://join/{:016x}{:016x}", r1, r2)
}
