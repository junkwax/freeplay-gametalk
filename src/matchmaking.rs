//! Automated matchmaking client for Freeplay.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{mpsc::Sender, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub enum Update {
    Status(String),
    AuthConnected {
        username: String,
        player_id: String,
    },
    Connected {
        peer_endpoint: String,
        is_host: bool,
        transport: MatchTransport,
        session_id: String,
        room_id: Option<String>,
        peer_username: Option<String>,
    },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct SpectateState {
    pub frame: Option<u32>,
    pub p1_score: u32,
    pub p2_score: u32,
    pub updated_at: Option<String>,
}

#[derive(Debug)]
pub enum SpectateUpdate {
    State(SpectateState),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveMatch {
    pub session_id: String,
    pub p1_name: String,
    pub p2_name: String,
    pub p1_score: u32,
    pub p2_score: u32,
}

#[derive(Debug)]
pub enum LiveMatchesUpdate {
    Loaded(Vec<LiveMatch>),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyUser {
    pub player_id: String,
    pub username: String,
    pub status: String,
    /// Glicko rating from the stats service, if the server provided one.
    pub rating: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyChatMessage {
    pub username: String,
    pub message: String,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbySnapshot {
    pub users: Vec<LobbyUser>,
    pub chat: Vec<LobbyChatMessage>,
    pub status: String,
}

#[derive(Debug)]
pub enum LobbyUpdate {
    Loaded(LobbySnapshot),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LobbyMatchFormat {
    UnrankedVs,
    RankedFt3,
    RankedFt5,
    RankedFt10,
}

pub fn lobby_format_label(f: LobbyMatchFormat) -> &'static str {
    match f {
        LobbyMatchFormat::UnrankedVs => "Unranked VS",
        LobbyMatchFormat::RankedFt3 => "Ranked FT3",
        LobbyMatchFormat::RankedFt5 => "Ranked FT5",
        LobbyMatchFormat::RankedFt10 => "Ranked FT10",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyRoom {
    pub id: String,
    pub name: String,
    pub host_username: String,
    pub format: LobbyMatchFormat,
    pub players: u8,
    pub private: bool,
    pub status: String,
}

#[derive(Debug)]
pub enum LobbyListUpdate {
    Loaded(Vec<LobbyRoom>),
    Error(String),
}

#[derive(Debug)]
pub enum LobbyChatPostUpdate {
    Sent,
    Error(String),
}

/// An incoming challenge addressed to us (someone in the lobby challenged us).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingChallenge {
    pub challenge_id: String,
    pub from_username: String,
    pub format: LobbyMatchFormat,
}

#[derive(Debug)]
pub enum ChallengeListUpdate {
    Loaded(Vec<IncomingChallenge>),
    Error(String),
}

// ── King-of-the-hill lobby (client view) ────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyMemberInfo {
    pub username: String,
    pub rating: Option<i32>,
    pub queued: bool,
    pub in_match: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyCurrent {
    pub host_username: String,
    pub join_username: String,
    pub host_session: String,
    pub join_session: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyView {
    pub id: String,
    pub name: String,
    pub ranked: bool,
    pub private: bool,
    pub format: LobbyMatchFormat,
    pub members: Vec<LobbyMemberInfo>,
    /// usernames in queue order (front = next up).
    pub queue: Vec<String>,
    pub current: Option<LobbyCurrent>,
    pub your_position: Option<usize>,
    pub your_queued: bool,
    pub your_session: Option<String>,
    /// True when it's your turn to play (server returned your_match).
    pub your_turn: bool,
}

#[derive(Debug)]
pub enum LobbyViewUpdate {
    /// A lobby was created — navigate to it with this id.
    Created(String),
    /// Latest lobby state.
    Loaded(LobbyView),
    Error(String),
}

#[derive(Debug)]
pub enum UsernameCheckUpdate {
    Available(String),
    Taken(String),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct TurnConnectInfo {
    pub uri: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug)]
pub enum MatchTransport {
    Direct {
        peer_addr: SocketAddr,
    },
    Relay {
        socket: crate::relay_socket::RelaySocket,
    },
}

// Derived from the git tag via build.rs (see version::VERSION) so the
// matchmaking compatibility key matches the version users actually see.
const APP_VERSION: &str = crate::version::VERSION;
const GAME_PORT: u16 = 7000;
const PUNCH_PAYLOAD: &[u8] = b"MK2PUNCH";

static CURRENT_TOKEN: Mutex<Option<String>> = Mutex::new(None);
static GUEST_PROFILE: Mutex<Option<(String, String, String)>> = Mutex::new(None);

fn signaling_url() -> Result<String, String> {
    let from_env = crate::config::env_value("FREEPLAY_SIGNALING_URL");
    if let Some(ref url) = from_env {
        return Ok(url.trim_end_matches('/').to_string());
    }
    crate::config::signaling_url()
        .ok_or_else(|| "FREEPLAY_SIGNALING_URL is not configured".to_string())
}

pub fn current_token() -> Option<String> {
    CURRENT_TOKEN.lock().ok().and_then(|g| g.clone())
}

pub fn set_guest_profile(username: String, email: String, device_id: String) {
    if let Ok(mut g) = GUEST_PROFILE.lock() {
        *g = Some((username, email, device_id));
    }
}

fn set_current_token(token: &str) {
    if let Ok(mut g) = CURRENT_TOKEN.lock() {
        *g = Some(token.to_string());
    }
}

pub fn start_discord_connect(tx: Sender<Update>) {
    std::thread::spawn(move || match discord_login(&tx) {
        Ok(token) => {
            set_current_token(&token);
            write_cached_token(&token);
            let username = username_from_token(&token).unwrap_or_else(|| "Discord".into());
            let player_id = player_id_from_token(&token).unwrap_or_default();
            let _ = tx.send(Update::AuthConnected {
                username,
                player_id,
            });
        }
        Err(e) => {
            let _ = tx.send(Update::Error(e));
        }
    });
}

#[allow(dead_code)]
pub fn start(tx: Sender<Update>) {
    std::thread::spawn(move || {
        if let Err(e) = run(&tx) {
            let _ = tx.send(Update::Error(e));
        }
    });
}

pub fn start_guest(tx: Sender<Update>) {
    std::thread::spawn(move || {
        if let Err(e) = run_guest(&tx) {
            let _ = tx.send(Update::Error(e));
        }
    });
}

pub fn check_username_available(
    stats_url: String,
    username: String,
    tx: Sender<UsernameCheckUpdate>,
) {
    std::thread::spawn(move || {
        let result = check_username_available_inner(&stats_url, &username);
        let update = match result {
            Ok(true) => UsernameCheckUpdate::Available(username),
            Ok(false) => UsernameCheckUpdate::Taken(username),
            Err(e) => UsernameCheckUpdate::Error(e),
        };
        let _ = tx.send(update);
    });
}

/// Start a join-to-spar session. Called when Discord delivers an
/// ACTIVITY_JOIN event containing the xband://join/<room_id> secret.
pub fn start_join_room(tx: Sender<Update>, room_id: String) {
    std::thread::spawn(move || {
        if let Err(e) = run_join_room(&tx, &room_id) {
            let _ = tx.send(Update::Error(e));
        }
    });
}

/// Host a public lobby: create a spar room, then wait for a challenger to join
/// it from their lobby browser and connect like any other match. `format` is
/// the wire format string ("vs"/"ft3"/"ft5"/"ft10").
/// Superseded by king-of-the-hill lobbies (`create_lobby`); kept for the
/// single-use spar-room path / potential Discord-host reuse.
#[allow(dead_code)]
pub fn start_host_room(tx: Sender<Update>, name: String, format: String, private: bool) {
    std::thread::spawn(move || {
        if let Err(e) = run_host_room(&tx, &name, &format, private) {
            let _ = tx.send(Update::Error(e));
        }
    });
}

#[allow(dead_code)]
fn run(tx: &Sender<Update>) -> Result<(), String> {
    let mut maybe_session_id: Option<String> = None;

    let outcome = (|| -> Result<(), String> {
        let token = auth_token(tx)?;
        set_current_token(&token);

        // Best-effort cancel of any prior session before we re-queue.
        // If a previous match crashed or was abandoned without an explicit
        // /match/cancel, the server still has us bound to a matched
        // session — and the OLD partner is still polling that session,
        // about to hole-punch a stale endpoint. Calling /match/cancel
        // lets the server cleanly invalidate the old room (the server's
        // re-queue handler does this too, but doing it here makes the
        // partner's "match cancelled" status appear ~1s earlier).
        // Failure is non-fatal; the server's own re-queue cleanup is the
        // belt to this suspender.
        if let Err(e) = cancel_match(&token, "self-cleanup") {
            // 404 on self-cleanup is the common case: no prior session.
            // Anything else is logged but not fatal.
            if !e.contains("404") {
                println!("[mm] pre-LFG cancel: {e}");
            }
        }

        send(tx, Update::Status("Discovering network...".into()))?;
        let stun_endpoint = stun_discover(GAME_PORT).map_err(|e| format!("STUN failed: {e}"))?;
        println!("[mm] public endpoint: {stun_endpoint}");

        send(tx, Update::Status("Entering queue...".into()))?;
        let (session_id, already_matched) = match lfg(&token, &stun_endpoint) {
            Ok(v) => v,
            Err(e) => {
                if e.contains("401") || e.contains("Unauthorized") || e.contains("Invalid token") {
                    clear_cached_token();
                    return Err(format!("Login expired, please try again: {e}"));
                }
                return Err(e);
            }
        };
        maybe_session_id = Some(session_id.clone());

        let match_info = if already_matched {
            poll_status(&token, &session_id, tx)?
        } else {
            send(tx, Update::Status("Waiting for opponent...".into()))?;
            poll_status(&token, &session_id, tx)?
        };

        if match_info.turn.is_some() {
            println!("[mm] TURN fallback available");
        }

        let session_id_for_update = session_id.clone();
        if let Some(turn) = match_info.turn.clone() {
            send(tx, Update::Status("Connecting via relay...".into()))?;
            println!(
                "[mm] using relay at {} (skipping direct P2P probe)",
                turn.uri
            );
            let transport = connect_relay(&turn)?;
            return send(
                tx,
                Update::Connected {
                    peer_endpoint: match_info.peer_endpoint.clone(),
                    is_host: match_info.role == "host",
                    transport,
                    session_id: session_id_for_update,
                    room_id: match_info.room_id.clone(),
                    peer_username: match_info.peer_username.clone(),
                },
            );
        }

        send(tx, Update::Status("Connecting to opponent...".into()))?;
        match hole_punch(&match_info.peer_endpoint, match_info.punch_at_ms) {
            Ok(peer_addr) => {
                println!("[mm] direct P2P established: {peer_addr}");
                send(
                    tx,
                    Update::Connected {
                        peer_endpoint: peer_addr.to_string(),
                        is_host: match_info.role == "host",
                        transport: MatchTransport::Direct { peer_addr },
                        session_id: session_id_for_update,
                        room_id: match_info.room_id.clone(),
                        peer_username: match_info.peer_username.clone(),
                    },
                )
            }
            Err(punch_err) => Err(format!(
                "Direct P2P failed and no TURN relay configured: {punch_err}"
            )),
        }
    })();

    if outcome.is_err() {
        if let Some(sid) = &maybe_session_id {
            if let Some(tok) = current_token() {
                println!("[mm] cancelling match on server");
                if let Err(e) = cancel_match(&tok, sid) {
                    println!("[mm] cancel failed (queue slot may linger): {e}");
                }
            }
        }
    }
    outcome
}

fn run_guest(tx: &Sender<Update>) -> Result<(), String> {
    let has_profile_email = GUEST_PROFILE
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .and_then(|(_, email, _)| crate::config::normalize_email(&email))
        .is_some();
    let status = if has_profile_email {
        "Signing in with player profile..."
    } else {
        "Signing in as guest..."
    };
    send(tx, Update::Status(status.into()))?;
    let token = guest_login()?;
    set_current_token(&token);
    run_with_token(tx, token)
}

fn run_with_token(tx: &Sender<Update>, token: String) -> Result<(), String> {
    let mut maybe_session_id: Option<String> = None;

    let outcome = (|| -> Result<(), String> {
        if let Err(e) = cancel_match(&token, "self-cleanup") {
            if !e.contains("404") {
                println!("[mm] pre-LFG cancel: {e}");
            }
        }

        send(tx, Update::Status("Discovering network...".into()))?;
        let stun_endpoint = stun_discover(GAME_PORT).map_err(|e| format!("STUN failed: {e}"))?;
        println!("[mm] public endpoint: {stun_endpoint}");

        send(tx, Update::Status("Entering queue...".into()))?;
        let (session_id, already_matched) = match lfg(&token, &stun_endpoint) {
            Ok(v) => v,
            Err(e) => {
                if e.contains("401") || e.contains("Unauthorized") || e.contains("Invalid token") {
                    clear_cached_token();
                    return Err(format!("Login expired, please try again: {e}"));
                }
                return Err(e);
            }
        };
        maybe_session_id = Some(session_id.clone());

        let match_info = if already_matched {
            poll_status(&token, &session_id, tx)?
        } else {
            send(tx, Update::Status("Waiting for opponent...".into()))?;
            poll_status(&token, &session_id, tx)?
        };

        if match_info.turn.is_some() {
            println!("[mm] TURN fallback available");
        }

        let session_id_for_update = session_id.clone();
        if let Some(turn) = match_info.turn.clone() {
            send(tx, Update::Status("Connecting via relay...".into()))?;
            println!(
                "[mm] using relay at {} (skipping direct P2P probe)",
                turn.uri
            );
            let transport = connect_relay(&turn)?;
            return send(
                tx,
                Update::Connected {
                    peer_endpoint: match_info.peer_endpoint.clone(),
                    is_host: match_info.role == "host",
                    transport,
                    session_id: session_id_for_update,
                    room_id: match_info.room_id.clone(),
                    peer_username: match_info.peer_username.clone(),
                },
            );
        }

        send(tx, Update::Status("Connecting to opponent...".into()))?;
        match hole_punch(&match_info.peer_endpoint, match_info.punch_at_ms) {
            Ok(peer_addr) => {
                println!("[mm] direct P2P established: {peer_addr}");
                send(
                    tx,
                    Update::Connected {
                        peer_endpoint: peer_addr.to_string(),
                        is_host: match_info.role == "host",
                        transport: MatchTransport::Direct { peer_addr },
                        session_id: session_id_for_update,
                        room_id: match_info.room_id.clone(),
                        peer_username: match_info.peer_username.clone(),
                    },
                )
            }
            Err(punch_err) => Err(format!(
                "Direct P2P failed and no TURN relay configured: {punch_err}"
            )),
        }
    })();

    if outcome.is_err() {
        if let Some(sid) = &maybe_session_id {
            if let Some(tok) = current_token() {
                println!("[mm] cancelling match on server");
                if let Err(e) = cancel_match(&tok, sid) {
                    println!("[mm] cancel failed (queue slot may linger): {e}");
                }
            }
        }
    }
    outcome
}

fn cancel_match(token: &str, _session_id: &str) -> Result<(), String> {
    // Server resolves the session from the JWT's `sub` via the
    // player_sessions map; no path param needed. Previously this hit
    // /match/cancel/<sid> (a 404 route) so cancels silently failed —
    // which is why prior sessions lingered on the server and caused
    // ghost-match symptoms when players re-queued.
    let url = format!("{}/match/cancel", signaling_url()?);
    http_post_json(&url, token, "{}").map(|_| ())
}

fn connect_relay(turn: &TurnConnectInfo) -> Result<MatchTransport, String> {
    crate::relay_socket::RelaySocket::connect(&turn.uri, &turn.username, &turn.password, GAME_PORT)
        .map(|socket| MatchTransport::Relay { socket })
        .map_err(|e| format!("relay connect failed: {e}"))
}

fn run_join_room(tx: &Sender<Update>, room_id: &str) -> Result<(), String> {
    let token = auth_token(tx)?;
    set_current_token(&token);

    send(tx, Update::Status("Discovering network...".into()))?;
    let stun_endpoint = stun_discover(GAME_PORT).map_err(|e| format!("STUN failed: {e}"))?;
    println!("[mm] public endpoint: {stun_endpoint}");

    send(tx, Update::Status("Joining spar room...".into()))?;
    let match_info = join_room_http(&token, room_id, &stun_endpoint)?;

    if match_info.turn.is_some() {
        println!("[mm] TURN fallback available");
    }

    if let Some(turn) = match_info.turn.clone() {
        send(tx, Update::Status("Connecting via relay...".into()))?;
        println!(
            "[mm] using relay at {} (skipping direct P2P probe)",
            turn.uri
        );
        let transport = connect_relay(&turn)?;
        return send(
            tx,
            Update::Connected {
                peer_endpoint: match_info.peer_endpoint.clone(),
                is_host: match_info.role == "host",
                transport,
                session_id: match_info.session_id,
                room_id: match_info.room_id.clone(),
                peer_username: match_info.peer_username.clone(),
            },
        );
    }

    send(tx, Update::Status("Connecting to opponent...".into()))?;
    match hole_punch(&match_info.peer_endpoint, match_info.punch_at_ms) {
        Ok(peer_addr) => {
            println!("[mm] direct P2P established: {peer_addr}");
            send(
                tx,
                Update::Connected {
                    peer_endpoint: peer_addr.to_string(),
                    is_host: match_info.role == "host",
                    transport: MatchTransport::Direct { peer_addr },
                    session_id: match_info.session_id,
                    room_id: match_info.room_id.clone(),
                    peer_username: match_info.peer_username.clone(),
                },
            )
        }
        Err(punch_err) => Err(format!(
            "Direct P2P failed and no TURN relay configured: {punch_err}"
        )),
    }
}

// ── Challenges ───────────────────────────────────────────────────────────────
// Direct player-vs-player challenges from the lobby presence list. The
// challenger POSTs /challenges and waits on their session like a host; the
// target accepts via /challenges/:id/accept and connects like a joiner.

/// Connect a settled match (relay or hole punch) and report Connected.
fn connect_match(
    tx: &Sender<Update>,
    mi: &MatchInfo,
    session_id: &str,
    is_host: bool,
) -> Result<(), String> {
    if let Some(turn) = mi.turn.clone() {
        send(tx, Update::Status("Connecting via relay...".into()))?;
        let transport = connect_relay(&turn)?;
        return send(
            tx,
            Update::Connected {
                peer_endpoint: mi.peer_endpoint.clone(),
                is_host,
                transport,
                session_id: session_id.to_string(),
                room_id: mi.room_id.clone(),
                peer_username: mi.peer_username.clone(),
            },
        );
    }
    send(tx, Update::Status("Connecting to opponent...".into()))?;
    match hole_punch(&mi.peer_endpoint, mi.punch_at_ms) {
        Ok(peer_addr) => send(
            tx,
            Update::Connected {
                peer_endpoint: peer_addr.to_string(),
                is_host,
                transport: MatchTransport::Direct { peer_addr },
                session_id: session_id.to_string(),
                room_id: mi.room_id.clone(),
                peer_username: mi.peer_username.clone(),
            },
        ),
        Err(punch_err) => Err(format!(
            "Direct P2P failed and no TURN relay configured: {punch_err}"
        )),
    }
}

/// Challenge a specific player: `target_id` is their lobby presence player_id,
/// `format` the wire string ("vs"/"ft3"/"ft5"/"ft10").
pub fn start_send_challenge(tx: Sender<Update>, target_id: String, format: String) {
    std::thread::spawn(move || {
        if let Err(e) = run_send_challenge(&tx, &target_id, &format) {
            let _ = tx.send(Update::Error(e));
        }
    });
}

fn run_send_challenge(tx: &Sender<Update>, target_id: &str, format: &str) -> Result<(), String> {
    let token = auth_token(tx)?;
    set_current_token(&token);

    send(tx, Update::Status("Discovering network...".into()))?;
    let stun_endpoint = stun_discover(GAME_PORT).map_err(|e| format!("STUN failed: {e}"))?;

    send(tx, Update::Status("Sending challenge...".into()))?;
    let challenger_session_id = send_challenge_http(&token, target_id, format, &stun_endpoint)?;

    let outcome = (|| -> Result<(), String> {
        send(tx, Update::Status("Waiting for them to accept...".into()))?;
        let match_info = poll_status(&token, &challenger_session_id, tx)?;
        connect_match(tx, &match_info, &challenger_session_id, true)
    })();

    if outcome.is_err() {
        if let Some(tok) = current_token() {
            let _ = cancel_match(&tok, &challenger_session_id);
        }
    }
    outcome
}

fn send_challenge_http(
    token: &str,
    target_id: &str,
    format: &str,
    stun_endpoint: &str,
) -> Result<String, String> {
    let rom_hash = rom_short_hash();
    let body = format!(
        r#"{{"target_id":"{target_id}","format":"{format}","stun_endpoint":"{stun_endpoint}","app_version":"{APP_VERSION}","rom_hash":"{rom_hash}"}}"#
    );
    let url = format!("{}/challenges", signaling_url()?);
    let resp = http_post_json(&url, token, &body)?;
    json_str(&resp, "challenger_session_id")
        .ok_or_else(|| format!("challenge response missing challenger_session_id: {resp}"))
}

/// Accept an incoming challenge by id and connect (joiner role).
pub fn start_accept_challenge(tx: Sender<Update>, challenge_id: String) {
    std::thread::spawn(move || {
        if let Err(e) = run_accept_challenge(&tx, &challenge_id) {
            let _ = tx.send(Update::Error(e));
        }
    });
}

fn run_accept_challenge(tx: &Sender<Update>, challenge_id: &str) -> Result<(), String> {
    let token = auth_token(tx)?;
    set_current_token(&token);

    send(tx, Update::Status("Discovering network...".into()))?;
    let stun_endpoint = stun_discover(GAME_PORT).map_err(|e| format!("STUN failed: {e}"))?;

    send(tx, Update::Status("Accepting challenge...".into()))?;
    let rom_hash = rom_short_hash();
    let body = format!(
        r#"{{"stun_endpoint":"{stun_endpoint}","app_version":"{APP_VERSION}","rom_hash":"{rom_hash}"}}"#
    );
    let url = format!("{}/challenges/{challenge_id}/accept", signaling_url()?);
    let resp = http_post_json(&url, &token, &body)?;

    let session_id = json_str(&resp, "session_id")
        .ok_or_else(|| format!("accept response missing session_id: {resp}"))?;
    let peer_endpoint = json_nested_str(&resp, "match_info", "peer_endpoint")
        .ok_or_else(|| format!("accept response missing peer_endpoint: {resp}"))?;
    let room_id = json_nested_str(&resp, "match_info", "room_id");
    let punch_at_ms = json_nested_i64(&resp, "match_info", "punch_at_ms")
        .ok_or_else(|| format!("accept response missing punch_at_ms: {resp}"))?;
    let role = json_nested_str(&resp, "match_info", "role").unwrap_or_else(|| "join".to_string());
    let peer_username = json_nested_str(&resp, "match_info", "username");
    let turn = parse_turn_creds(&resp);
    let match_info = MatchInfo {
        session_id: session_id.clone(),
        room_id,
        peer_endpoint,
        punch_at_ms,
        role,
        turn,
        peer_username,
    };
    connect_match(tx, &match_info, &session_id, false)
}

/// Decline an incoming challenge (fire-and-forget).
pub fn decline_challenge(challenge_id: String) {
    std::thread::spawn(move || {
        let Some(token) = current_token() else { return };
        if let Ok(url) = signaling_url().map(|u| format!("{u}/challenges/{challenge_id}/decline")) {
            let _ = http_post_json(&url, &token, "{}");
        }
    });
}

/// Poll the player's incoming challenges.
pub fn fetch_challenges(tx: Sender<ChallengeListUpdate>) {
    std::thread::spawn(move || {
        let update = match fetch_challenges_inner() {
            Ok(list) => ChallengeListUpdate::Loaded(list),
            Err(e) => ChallengeListUpdate::Error(e),
        };
        let _ = tx.send(update);
    });
}

fn fetch_challenges_inner() -> Result<Vec<IncomingChallenge>, String> {
    let token = current_token().ok_or_else(|| "not signed in".to_string())?;
    let url = format!("{}/challenges", signaling_url()?);
    let resp = http_get(&url, &token)?;
    Ok(parse_incoming_challenges(&resp))
}

fn parse_incoming_challenges(json: &str) -> Vec<IncomingChallenge> {
    let mut out = Vec::new();
    let Some(body) = json_array_body(json, "challenges") else {
        return out;
    };
    for chunk in json_object_chunks(body) {
        // Only inbound challenges (someone challenging us).
        if json_str(chunk, "direction").as_deref() == Some("outgoing") {
            continue;
        }
        let Some(challenge_id) = json_str(chunk, "challenge_id").or_else(|| json_str(chunk, "id"))
        else {
            continue;
        };
        let from_username = json_str(chunk, "challenger_username")
            .or_else(|| json_str(chunk, "from_username"))
            .unwrap_or_else(|| "Someone".into());
        let format = json_str(chunk, "format")
            .and_then(|raw| parse_lobby_match_format(&raw))
            .unwrap_or(LobbyMatchFormat::UnrankedVs);
        out.push(IncomingChallenge {
            challenge_id,
            from_username,
            format,
        });
    }
    out
}

/// POST /room/create — register a public spar room and a placeholder queue
/// entry. Returns (room_id, creator_session_id); the host polls
/// /match/status/<creator_session_id> for the join.
#[allow(dead_code)]
fn create_room_http(
    token: &str,
    name: &str,
    format: &str,
    private: bool,
    stun_endpoint: &str,
) -> Result<(String, String), String> {
    let rom_hash = rom_short_hash();
    let body = format!(
        r#"{{"stun_endpoint":"{stun_endpoint}","app_version":"{APP_VERSION}","rom_hash":"{rom_hash}","name":"{name}","format":"{format}","private":{private}}}"#
    );
    let url = format!("{}/room/create", signaling_url()?);
    let resp = http_post_json(&url, token, &body)?;

    let room_id =
        json_str(&resp, "room_id").ok_or_else(|| format!("create missing room_id: {resp}"))?;
    let creator_session_id = json_str(&resp, "creator_session_id")
        .ok_or_else(|| format!("create missing creator_session_id: {resp}"))?;
    Ok((room_id, creator_session_id))
}

#[allow(dead_code)]
fn run_host_room(
    tx: &Sender<Update>,
    name: &str,
    format: &str,
    private: bool,
) -> Result<(), String> {
    let token = auth_token(tx)?;
    set_current_token(&token);

    send(tx, Update::Status("Discovering network...".into()))?;
    let stun_endpoint = stun_discover(GAME_PORT).map_err(|e| format!("STUN failed: {e}"))?;
    println!("[mm] public endpoint: {stun_endpoint}");

    send(tx, Update::Status("Creating lobby...".into()))?;
    let (room_id, creator_session_id) =
        create_room_http(&token, name, format, private, &stun_endpoint)?;
    println!("[mm] hosting room {room_id} (session {creator_session_id})");

    // Wait for a challenger and connect. On any failure, release the room/queue
    // slot so it doesn't linger in the lobby browser.
    let outcome = (|| -> Result<(), String> {
        send(tx, Update::Status("Waiting for a challenger...".into()))?;
        let match_info = poll_status(&token, &creator_session_id, tx)?;

        if let Some(turn) = match_info.turn.clone() {
            send(tx, Update::Status("Connecting via relay...".into()))?;
            let transport = connect_relay(&turn)?;
            return send(
                tx,
                Update::Connected {
                    peer_endpoint: match_info.peer_endpoint.clone(),
                    is_host: true,
                    transport,
                    session_id: creator_session_id.clone(),
                    room_id: match_info.room_id.clone(),
                    peer_username: match_info.peer_username.clone(),
                },
            );
        }

        send(tx, Update::Status("Connecting to challenger...".into()))?;
        match hole_punch(&match_info.peer_endpoint, match_info.punch_at_ms) {
            Ok(peer_addr) => send(
                tx,
                Update::Connected {
                    peer_endpoint: peer_addr.to_string(),
                    is_host: true,
                    transport: MatchTransport::Direct { peer_addr },
                    session_id: creator_session_id.clone(),
                    room_id: match_info.room_id.clone(),
                    peer_username: match_info.peer_username.clone(),
                },
            ),
            Err(punch_err) => Err(format!(
                "Direct P2P failed and no TURN relay configured: {punch_err}"
            )),
        }
    })();

    if outcome.is_err() {
        if let Some(tok) = current_token() {
            if let Err(e) = cancel_match(&tok, &creator_session_id) {
                println!("[mm] host cleanup cancel failed: {e}");
            }
        }
    }
    outcome
}

struct JoinInfo {
    session_id: String,
    room_id: Option<String>,
    peer_endpoint: String,
    punch_at_ms: i64,
    role: String,
    turn: Option<TurnConnectInfo>,
    peer_username: Option<String>,
}

fn join_room_http(token: &str, room_id: &str, stun_endpoint: &str) -> Result<JoinInfo, String> {
    let rom_hash = rom_short_hash();
    let body = format!(
        r#"{{"stun_endpoint":"{stun_endpoint}","app_version":"{APP_VERSION}","rom_hash":"{rom_hash}"}}"#
    );
    let url = format!("{}/room/join/{room_id}", signaling_url()?);
    let resp = http_post_json(&url, token, &body)?;

    let session_id = json_str(&resp, "session_id")
        .ok_or_else(|| format!("join response missing session_id: {resp}"))?;
    let peer_endpoint = json_nested_str(&resp, "match_info", "peer_endpoint")
        .ok_or_else(|| format!("join response missing peer_endpoint: {resp}"))?;
    let room_id = json_nested_str(&resp, "match_info", "room_id");
    let punch_at_ms = json_nested_i64(&resp, "match_info", "punch_at_ms")
        .ok_or_else(|| format!("join response missing punch_at_ms: {resp}"))?;
    let role = json_nested_str(&resp, "match_info", "role").unwrap_or_else(|| "join".to_string());
    let peer_username = json_nested_str(&resp, "match_info", "username");
    let turn = parse_turn_creds(&resp);

    Ok(JoinInfo {
        session_id,
        room_id,
        peer_endpoint,
        punch_at_ms,
        role,
        turn,
        peer_username,
    })
}

fn send(tx: &Sender<Update>, u: Update) -> Result<(), String> {
    tx.send(u)
        .map_err(|_| "main thread disconnected (user cancelled)".to_string())
}

// ── Token caching ─────────────────────────────────────────────────────────────

fn token_cache_path() -> Option<std::path::PathBuf> {
    let base = std::env::var("APPDATA")
        .ok()
        .or_else(|| std::env::var("HOME").ok())?;
    let dir = std::path::PathBuf::from(base).join("freeplay");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("token"))
}

pub fn clear_cached_token() {
    if let Ok(mut guard) = CURRENT_TOKEN.lock() {
        *guard = None;
    }
    if let Some(path) = token_cache_path() {
        let _ = std::fs::remove_file(&path);
        println!("[mm] cleared cached token");
    }
}

fn auth_token(tx: &Sender<Update>) -> Result<String, String> {
    if let Some(token) = read_cached_token() {
        send(tx, Update::Status("Using Discord account...".into()))?;
        return Ok(token);
    }

    send(tx, Update::Status("Signing in as guest...".into()))?;
    guest_login()
}

fn read_cached_token() -> Option<String> {
    let path = token_cache_path()?;
    let token = std::fs::read_to_string(&path).ok()?.trim().to_string();
    if token.is_empty() {
        return None;
    }
    let exp = token_exp(&token)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    if exp <= now + 60 {
        println!("[mm] cached Discord token expired, re-authenticating");
        return None;
    }
    let sub = player_id_from_token(&token)?;
    if sub.starts_with("guest-") {
        return None;
    }
    println!(
        "[mm] using cached Discord token (expires in {}s)",
        exp - now
    );
    Some(token)
}

fn write_cached_token(token: &str) {
    if let Some(path) = token_cache_path() {
        match std::fs::write(&path, token) {
            Ok(_) => println!("[mm] Discord token cached to {}", path.display()),
            Err(e) => println!("[mm] failed to cache token: {e}"),
        }
    }
}

fn guest_login() -> Result<String, String> {
    let (username, email, device_id) = GUEST_PROFILE
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| {
            (
                crate::config::default_username(),
                String::new(),
                String::new(),
            )
        });
    let username = crate::config::sanitize_username(&username).ok_or_else(|| {
        format!(
            "Username must be 2-{} letters/numbers",
            crate::config::MAX_USERNAME_LEN
        )
    })?;
    let email = crate::config::normalize_email(&email).unwrap_or_default();
    let body = if !email.is_empty() {
        format!(
            r#"{{"username":"{}","email":"{}"}}"#,
            json_escape(&username),
            json_escape(&email)
        )
    } else if !device_id.is_empty() {
        format!(
            r#"{{"username":"{}","device_id":"{}"}}"#,
            json_escape(&username),
            json_escape(&device_id)
        )
    } else {
        format!(r#"{{"username":"{}"}}"#, json_escape(&username))
    };
    let url = format!("{}/auth/guest", signaling_url()?);
    let resp = http_post_json_no_auth(&url, &body)?;
    json_str(&resp, "token").ok_or_else(|| format!("guest auth missing token: {resp}"))
}

fn check_username_available_inner(stats_url: &str, username: &str) -> Result<bool, String> {
    let username = crate::config::sanitize_username(username).ok_or_else(|| {
        format!(
            "Username must be 2-{} letters/numbers",
            crate::config::MAX_USERNAME_LEN
        )
    })?;
    let stats_url = stats_url.trim_end_matches('/');
    if stats_url.is_empty() {
        return Err("Stats service is not configured; cannot verify username".into());
    }
    let guest_id = format!("guest-name:{}", sha256_hex(username.as_bytes()));
    let url = format!("{stats_url}/player/{guest_id}");
    let (status, body) = http_get_no_auth_status(&url)?;
    match status {
        200 => Ok(false),
        404 => Ok(true),
        0 => Err("Stats service returned an invalid response".into()),
        500..=599 => Err(format!(
            "Stats service unavailable while checking username: {status}"
        )),
        _ => {
            if body.trim().is_empty() {
                Err(format!("Username check failed with HTTP {status}"))
            } else {
                Err(format!("Username check failed with HTTP {status}: {body}"))
            }
        }
    }
}

fn current_or_cached_token() -> Option<String> {
    current_token().or_else(|| {
        let path = token_cache_path()?;
        let token = std::fs::read_to_string(&path).ok()?.trim().to_string();
        if token.is_empty() {
            None
        } else {
            Some(token)
        }
    })
}

/// Extract the current username from the active JWT, if one is present and not expired.
pub fn username_from_cached_token() -> Option<String> {
    let token = current_or_cached_token()?;
    valid_token_payload(&token).and_then(|payload| json_str(&payload, "username"))
}

/// Extract the active stats/player ID (JWT `sub` claim) from the current token.
pub fn discord_id_from_cached_token() -> Option<String> {
    let token = current_or_cached_token()?;
    player_id_from_token(&token)
}

pub fn guest_player_id(username: &str, email: &str, device_id: &str) -> Option<String> {
    let username = crate::config::sanitize_username(username)?;
    if let Some(email) = crate::config::normalize_email(email) {
        Some(format!("guest-email:{}", sha256_hex(email.as_bytes())))
    } else if !device_id.trim().is_empty() {
        Some(format!(
            "guest-device:{}",
            sha256_hex(device_id.trim().as_bytes())
        ))
    } else {
        Some(format!("guest-name:{}", sha256_hex(username.as_bytes())))
    }
}

pub fn connected_discord_user_from_cached_token() -> Option<String> {
    let token = read_cached_token()?;
    username_from_token(&token)
}

fn username_from_token(token: &str) -> Option<String> {
    valid_token_payload(token).and_then(|payload| json_str(&payload, "username"))
}

fn player_id_from_token(token: &str) -> Option<String> {
    valid_token_payload(token).and_then(|payload| json_str(&payload, "sub"))
}

fn token_exp(token: &str) -> Option<i64> {
    let payload_b64 = token.split('.').nth(1)?;
    let payload_bytes = base64_decode(&pad_base64url(payload_b64))?;
    let payload_str = String::from_utf8(payload_bytes).ok()?;
    json_i64(&payload_str, "exp")
}

fn valid_token_payload(token: &str) -> Option<String> {
    let payload_b64 = token.split('.').nth(1)?;
    let payload_bytes = base64_decode(&pad_base64url(payload_b64))?;
    let payload_str = String::from_utf8(payload_bytes).ok()?;
    let exp = json_i64(&payload_str, "exp")?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    if exp <= now + 60 {
        return None;
    }
    Some(payload_str)
}

fn pad_base64url(s: &str) -> String {
    let mut s = s.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    s
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let bytes: Vec<u8> = s.bytes().filter(|&b| b != b'=').collect();
    for chunk in bytes.chunks(4) {
        let mut v = [0u32; 4];
        for (i, &b) in chunk.iter().enumerate() {
            v[i] = T.iter().position(|&c| c == b)? as u32;
        }
        let combined = (v[0] << 18) | (v[1] << 12) | (v[2] << 6) | v[3];
        if chunk.len() >= 2 {
            out.push((combined >> 16) as u8);
        }
        if chunk.len() >= 3 {
            out.push((combined >> 8) as u8);
        }
        if chunk.len() >= 4 {
            out.push(combined as u8);
        }
    }
    Some(out)
}

// ── ROM hash ──────────────────────────────────────────────────────────────────

fn rom_short_hash() -> String {
    let bytes = match crate::rom::read_rom_zip() {
        Some(b) => b,
        None => return "0".to_string(),
    };
    let mut h: u64 = 0xcbf29ce484222325;
    for chunk in bytes.chunks(8) {
        let mut w = [0u8; 8];
        w[..chunk.len()].copy_from_slice(chunk);
        h ^= u64::from_le_bytes(w);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", (h >> 32) as u32)
}

fn sha256_hex(input: &[u8]) -> String {
    const H0: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h = H0;
    let bit_len = (input.len() as u64) * 8;
    let mut data = input.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in data.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let j = i * 4;
            *word = u32::from_be_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = String::with_capacity(64);
    for word in h {
        use std::fmt::Write;
        let _ = write!(&mut out, "{word:08x}");
    }
    out
}

// ── Optional Discord OAuth ────────────────────────────────────────────────────

fn discord_login(tx: &Sender<Update>) -> Result<String, String> {
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:19420")
        .map_err(|e| format!("Failed to bind local auth server on :19420: {e}"))?;
    listener.set_nonblocking(false).ok();

    let login_url = format!("{}/auth/discord", signaling_url()?);
    open::that(&login_url).map_err(|e| format!("Failed to open browser: {e}"))?;

    send(tx, Update::Status("Waiting for Discord login...".into()))?;

    let (mut stream, _) = listener
        .accept()
        .map_err(|e| format!("Auth server accept failed: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(120))).ok();

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok();
    for l in reader.by_ref().lines() {
        let l = l.map_err(|e| e.to_string())?;
        if l.is_empty() {
            break;
        }
    }

    let html = r#"<!DOCTYPE html>
<html><head><title>Freeplay Login</title></head><body>
<p>Logging you in to Freeplay...</p>
<script>
  const token = new URLSearchParams(window.location.hash.slice(1)).get('token');
  if (token) {
    fetch('/token', { method: 'POST', headers: {'Content-Type': 'text/plain'}, body: token })
      .then(() => { document.body.innerHTML = '<h2>Logged in! You can close this tab.</h2>'; });
  } else {
    document.body.innerHTML = '<h2>Login failed: no token in URL.</h2>';
  }
</script></body></html>"#;

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    stream.write_all(response.as_bytes()).ok();
    drop(stream);

    let (mut stream2, _) = listener
        .accept()
        .map_err(|e| format!("Token POST accept failed: {e}"))?;
    stream2.set_read_timeout(Some(Duration::from_secs(30))).ok();

    let mut reader2 = BufReader::new(&stream2);
    let mut req_line = String::new();
    reader2.read_line(&mut req_line).ok();
    let mut content_length = 0usize;
    for l in reader2.by_ref().lines() {
        let l = l.map_err(|e| e.to_string())?;
        if l.is_empty() {
            break;
        }
        if l.to_lowercase().starts_with("content-length:") {
            content_length = l
                .split(':')
                .nth(1)
                .unwrap_or("0")
                .trim()
                .parse()
                .unwrap_or(0);
        }
    }

    let mut body = vec![0u8; content_length];
    std::io::Read::read_exact(&mut reader2, &mut body)
        .map_err(|e| format!("Token body read failed: {e}"))?;
    let token = String::from_utf8(body)
        .unwrap_or_default()
        .trim()
        .to_string();

    let ack = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    stream2.write_all(ack.as_bytes()).ok();

    if token.is_empty() {
        return Err("Empty token: Discord login may have been cancelled".to_string());
    }
    println!("[mm] Discord OAuth token received");
    Ok(token)
}

// ── STUN discovery ────────────────────────────────────────────────────────────

/// STUN servers tried in order. Hostnames so DNS round-robin can fail us over
/// when a single backend IP is dropped (e.g. Google rotating its STUN pool).
/// Mixing providers protects against a full-provider outage.
const STUN_HOSTS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

fn stun_discover(game_port: u16) -> Result<String, String> {
    let sock = UdpSocket::bind(format!("0.0.0.0:{game_port}")).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            format!(
                "UDP port {game_port} is already in use. Another freeplay.exe \
                 is probably running — close it (Task Manager) and try again."
            )
        } else {
            format!("UDP bind on :{game_port} failed: {e}")
        }
    })?;
    sock.set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;

    let mut req = [0u8; 20];
    req[0] = 0x00;
    req[1] = 0x01;
    req[4] = 0x21;
    req[5] = 0x12;
    req[6] = 0xA4;
    req[7] = 0x42;
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for (i, b) in req[8..20].iter_mut().enumerate() {
        *b = ((t >> (i * 5)) ^ (t >> (i * 3 + 1))) as u8;
    }

    let mut last_err = String::new();
    for host in STUN_HOSTS {
        match try_stun_one(&sock, host, &req) {
            Ok(endpoint) => return Ok(endpoint),
            Err(e) => {
                println!("[mm] STUN via {host} failed: {e}");
                last_err = e;
            }
        }
    }
    Err(format!(
        "All STUN servers unreachable (last: {last_err}). Check internet \
         connectivity and that UDP egress is not firewalled."
    ))
}

fn try_stun_one(sock: &UdpSocket, host: &str, req: &[u8; 20]) -> Result<String, String> {
    let mut addrs = host
        .to_socket_addrs()
        .map_err(|e| format!("DNS lookup for {host}: {e}"))?
        .filter(SocketAddr::is_ipv4);
    let stun = addrs
        .next()
        .ok_or_else(|| format!("{host} resolved to no IPv4 addresses"))?;

    sock.send_to(req, stun)
        .map_err(|e| format!("STUN send to {stun}: {e}"))?;

    let mut buf = [0u8; 512];
    let (n, _) = sock
        .recv_from(&mut buf)
        .map_err(|e| format!("STUN recv from {stun}: {e}"))?;

    parse_stun_xor_mapped(&buf[..n])
        .ok_or_else(|| format!("No XOR-MAPPED-ADDRESS in {host} response"))
}

fn parse_stun_xor_mapped(buf: &[u8]) -> Option<String> {
    if buf.len() < 20 || buf[0] != 0x01 || buf[1] != 0x01 {
        return None;
    }
    let msg_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let magic = [0x21u8, 0x12, 0xA4, 0x42];
    let mut pos = 20;
    while pos + 4 <= 20 + msg_len {
        let attr_type = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let attr_len = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]) as usize;
        pos += 4;
        if (attr_type == 0x0020 || attr_type == 0x0001) && attr_len >= 8 && buf[pos + 1] == 0x01 {
            let pb = [buf[pos + 2], buf[pos + 3]];
            let ab = [buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7]];
            let (port, addr) = if attr_type == 0x0020 {
                (
                    u16::from_be_bytes(pb) ^ 0x2112,
                    [
                        ab[0] ^ magic[0],
                        ab[1] ^ magic[1],
                        ab[2] ^ magic[2],
                        ab[3] ^ magic[3],
                    ],
                )
            } else {
                (u16::from_be_bytes(pb), ab)
            };
            return Some(format!(
                "{}.{}.{}.{}:{}",
                addr[0], addr[1], addr[2], addr[3], port
            ));
        }
        pos += (attr_len + 3) & !3;
    }
    None
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

fn http_post_json(url: &str, token: &str, body: &str) -> Result<String, String> {
    let (host, path) = parse_url(url)?;
    let stream = tls_connect(&host)?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    send_recv_https(stream, &req)
}

fn http_post_json_no_auth(url: &str, body: &str) -> Result<String, String> {
    let (host, path) = parse_url(url)?;
    let stream = tls_connect(&host)?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    send_recv_https(stream, &req)
}

fn http_get(url: &str, token: &str) -> Result<String, String> {
    let (host, path) = parse_url(url)?;
    let stream = tls_connect(&host)?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    send_recv_https(stream, &req)
}

fn http_get_optional_auth(url: &str) -> Result<String, String> {
    if let Some(token) = current_or_cached_token() {
        http_get(url, &token)
    } else {
        http_get_no_auth(url)
    }
}

/// HTTP GET without an Authorization header. Used for the freeplay-stats
/// endpoints that intentionally serve community data unauthenticated
/// (`/player/:id`, `/player/:id/history`, `/leaderboard`, `/ghosts/list`).
fn http_get_no_auth(url: &str) -> Result<String, String> {
    let (host, path) = parse_url(url)?;
    let stream = tls_connect(&host)?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    send_recv_https(stream, &req)
}

fn http_get_no_auth_status(url: &str) -> Result<(u16, String), String> {
    let (host, path) = parse_url(url)?;
    let stream = tls_connect(&host)?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    send_recv_https_status(stream, &req)
}

fn parse_url(url: &str) -> Result<(String, String), String> {
    let url = url.trim_start_matches("https://");
    let slash = url.find('/').unwrap_or(url.len());
    let host = url[..slash].to_string();
    let path = if slash < url.len() {
        url[slash..].to_string()
    } else {
        "/".to_string()
    };
    Ok((host, path))
}

fn send_recv_https(stream: Box<dyn ReadWrite>, req: &str) -> Result<String, String> {
    let (status, response) = send_recv_https_status(stream, req)?;
    if status == 401 || status == 403 {
        return Err(format!("401 Unauthorized: {response}"));
    }
    Ok(response)
}

fn send_recv_https_status(
    mut stream: Box<dyn ReadWrite>,
    req: &str,
) -> Result<(u16, String), String> {
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);

    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| e.to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if trimmed.to_lowercase().starts_with("content-length:") {
            content_length = trimmed
                .split(':')
                .nth(1)
                .unwrap_or("0")
                .trim()
                .parse()
                .unwrap_or(0);
        }
    }

    let mut body = vec![0u8; content_length];
    std::io::Read::read_exact(&mut reader, &mut body).map_err(|e| e.to_string())?;
    let response = String::from_utf8_lossy(&body).to_string();

    Ok((status, response))
}

trait ReadWrite: std::io::Read + std::io::Write + Send {}
impl<S: std::io::Read + std::io::Write + Send> ReadWrite for native_tls::TlsStream<S> {}

fn tls_connect(host: &str) -> Result<Box<dyn ReadWrite>, String> {
    use std::net::TcpStream;

    let addr = format!("{host}:443");
    let connector = native_tls::TlsConnector::new().map_err(|e| format!("TLS connector: {e}"))?;
    let addrs: Vec<SocketAddr> = addr
        .to_socket_addrs()
        .map_err(|e| format!("DNS lookup for {addr}: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("DNS lookup for {addr}: no addresses"));
    }

    let mut last_err = String::new();
    for socket_addr in addrs {
        match TcpStream::connect_timeout(&socket_addr, Duration::from_secs(8)) {
            Ok(tcp) => {
                tcp.set_read_timeout(Some(Duration::from_secs(15))).ok();
                tcp.set_write_timeout(Some(Duration::from_secs(10))).ok();
                match connector.connect(host, tcp) {
                    Ok(tls) => return Ok(Box::new(tls)),
                    Err(e) => {
                        last_err = format!("TLS handshake with {host} via {socket_addr}: {e}");
                    }
                }
            }
            Err(e) => {
                last_err = format!("TCP connect to {socket_addr}: {e}");
            }
        }
    }

    Err(last_err)
}

// ── LFG + status polling ──────────────────────────────────────────────────────

struct MatchInfo {
    #[allow(dead_code)]
    session_id: String,
    room_id: Option<String>,
    peer_endpoint: String,
    punch_at_ms: i64,
    role: String,
    turn: Option<TurnConnectInfo>,
    peer_username: Option<String>,
}

fn lfg(token: &str, stun_endpoint: &str) -> Result<(String, bool), String> {
    let rom_hash = rom_short_hash();
    println!("[mm] rom_hash={rom_hash}");

    let body = format!(
        r#"{{"stun_endpoint":"{stun_endpoint}","app_version":"{APP_VERSION}","rom_hash":"{rom_hash}"}}"#
    );
    let url = format!("{}/match/lfg", signaling_url()?);
    let resp = http_post_json(&url, token, &body)?;

    let session_id = json_str(&resp, "session_id")
        .ok_or_else(|| format!("LFG response missing session_id: {resp}"))?;
    let status = json_str(&resp, "status").unwrap_or_default();
    let already_matched = status == "matched";

    Ok((session_id, already_matched))
}

fn poll_status(token: &str, session_id: &str, tx: &Sender<Update>) -> Result<MatchInfo, String> {
    let url = format!("{}/match/status/{session_id}", signaling_url()?);
    let deadline = Instant::now() + Duration::from_secs(120);
    // Allow a handful of consecutive transient HTTP failures before giving up.
    // Cloud Run cold starts and brief network blips routinely produce 502/503
    // or connection resets; a single hiccup shouldn't end a queue session.
    let mut transient_failures = 0u32;
    const MAX_TRANSIENT_FAILURES: u32 = 5;

    loop {
        if Instant::now() > deadline {
            return Err("No opponent found within 2 minutes".to_string());
        }

        let resp = match http_get(&url, token) {
            Ok(r) => {
                transient_failures = 0;
                r
            }
            Err(e) => {
                // 401/403 are auth errors — bubble up immediately so the caller
                // can clear the cached token. Everything else is treated as a
                // transient network/server issue and retried.
                if e.contains("401") || e.contains("403") {
                    return Err(e);
                }
                transient_failures += 1;
                if transient_failures >= MAX_TRANSIENT_FAILURES {
                    return Err(format!(
                        "Signaling server unreachable after {transient_failures} retries: {e}"
                    ));
                }
                println!(
                    "[mm] poll_status transient error ({transient_failures}/{MAX_TRANSIENT_FAILURES}): {e}"
                );
                send(tx, Update::Status("Reconnecting to matchmaking...".into()))?;
                // Linear backoff (1.5s, 3s, 4.5s, 6s, 7.5s) keeps total worst
                // case well inside the 120s window.
                std::thread::sleep(Duration::from_millis(1500 * transient_failures as u64));
                continue;
            }
        };
        let status = json_str(&resp, "status").unwrap_or_default();

        match status.as_str() {
            "matched" => {
                let peer_endpoint = json_nested_str(&resp, "match_info", "peer_endpoint")
                    .ok_or("missing peer_endpoint")?;
                let room_id = json_nested_str(&resp, "match_info", "room_id");
                let punch_at_ms = json_nested_i64(&resp, "match_info", "punch_at_ms")
                    .ok_or("missing punch_at_ms")?;
                let role = json_nested_str(&resp, "match_info", "role").ok_or("missing role")?;
                let peer_username = json_nested_str(&resp, "match_info", "username");
                let turn = parse_turn_creds(&resp);
                return Ok(MatchInfo {
                    session_id: session_id.to_string(),
                    room_id,
                    peer_endpoint,
                    punch_at_ms,
                    role,
                    turn,
                    peer_username,
                });
            }
            "cancelled" => return Err("Match cancelled by server".to_string()),
            _ => {
                send(tx, Update::Status("Waiting for opponent...".into()))?;
                std::thread::sleep(Duration::from_millis(1500));
            }
        }
    }
}

fn parse_turn_creds(json: &str) -> Option<TurnConnectInfo> {
    let outer_pat = "\"match_info\":{";
    let outer_start = json.find(outer_pat)? + outer_pat.len() - 1;
    let outer = &json[outer_start..];

    let turn_pat = "\"turn\":{";
    let turn_start = outer.find(turn_pat)? + turn_pat.len() - 1;
    let turn_section = &outer[turn_start..];

    let uri = json_str(turn_section, "uri")?;
    let username = json_str(turn_section, "username")?;
    let password = json_str(turn_section, "password")?;

    Some(TurnConnectInfo {
        uri,
        username,
        password,
    })
}

// ── freeplay-stats: profile + match history ───────────────────────────────────

/// Minimal flattened view of `freeplay-stats::PlayerProfile` for menu display.
/// We round Glicko mu/RD to integers since two-decimal-place ratings just add
/// visual noise on a low-res surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileData {
    pub username: String,
    pub rating: i32,
    pub deviation: i32,
    pub wins: u64,
    pub losses: u64,
    pub matches_played: u64,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    pub opponent_username: String,
    pub result: String, // "won" or "lost"
    pub our_score: u16,
    pub opponent_score: u16,
    pub played_at: String, // ISO8601 from server, displayed as-is
}

#[derive(Debug)]
pub enum ProfileUpdate {
    Loaded {
        profile: ProfileData,
        history: Vec<HistoryRow>,
    },
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderboardRow {
    pub username: String,
    pub rating: i32,
    pub wins: u64,
    pub losses: u64,
}

#[derive(Debug)]
pub enum LeaderboardUpdate {
    Loaded(Vec<LeaderboardRow>),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteReplayMeta {
    pub filename: String,
    pub url: String,
    pub p1_name: String,
    pub p2_name: String,
    pub p1_score: Option<u16>,
    pub p2_score: Option<u16>,
    pub winner: String,
    pub frame_count: u32,
    pub duration: String,
    pub recorded_at: String,
}

#[derive(Debug)]
pub enum PublicReplayUpdate {
    Loaded(Vec<RemoteReplayMeta>),
    Error(String),
}

/// Fire-and-forget profile fetcher. Spawns a thread, GETs both
/// `/player/:id` and `/player/:id/history`, and pushes a `ProfileUpdate`
/// down `tx`. The main loop polls `tx` like it does for matchmaking.
pub fn fetch_profile(stats_url: String, discord_id: String, tx: Sender<ProfileUpdate>) {
    std::thread::spawn(move || {
        if stats_url.is_empty() {
            let _ = tx.send(ProfileUpdate::Error("stats_url not configured".into()));
            return;
        }
        let profile_url = format!("{stats_url}/player/{discord_id}");
        let history_url = format!("{stats_url}/player/{discord_id}/history?limit=10");

        let profile = match http_get_no_auth(&profile_url) {
            Ok(body) => match parse_profile(&body) {
                Some(p) => p,
                None => {
                    let _ = tx.send(ProfileUpdate::Error(format!(
                        "Couldn't parse profile response: {body}"
                    )));
                    return;
                }
            },
            Err(e) => {
                // 404 is the common case — a freshly-OAuth'd user with no matches.
                // Server returns 404 with no body; we surface a friendlier message.
                let msg = if e.contains("404") {
                    "No matches recorded yet — play one to appear here.".to_string()
                } else {
                    format!("Profile fetch failed: {e}")
                };
                let _ = tx.send(ProfileUpdate::Error(msg));
                return;
            }
        };

        let history = http_get_no_auth(&history_url)
            .ok()
            .and_then(|body| parse_history(&body))
            .unwrap_or_default();

        // Fill in a default Discord avatar if the server didn't provide one.
        let mut profile = profile;
        if profile.avatar_url.is_none() {
            profile.avatar_url = discord_avatar_url_from_cached_token()
                .or_else(|| discord_default_avatar_url(&discord_id));
        }

        let _ = tx.send(ProfileUpdate::Loaded { profile, history });
    });
}

/// Compute the Discord default avatar URL for a given Discord snowflake ID.
/// Discord's default avatars are at `cdn.discordapp.com/embed/avatars/<index>.png`
/// where `index = ((id >> 22) % 6)`.
pub fn discord_default_avatar_url(discord_id: &str) -> Option<String> {
    let id: u64 = discord_id.parse().ok()?;
    let index = ((id >> 22) % 6) as u8;
    Some(format!(
        "https://cdn.discordapp.com/embed/avatars/{}.png",
        index
    ))
}

/// Extract a real Discord avatar URL from the cached JWT when the auth server
/// includes Discord's avatar hash. Falls back elsewhere when the claim is absent.
pub fn discord_avatar_url_from_cached_token() -> Option<String> {
    let path = token_cache_path()?;
    let token = std::fs::read_to_string(&path).ok()?.trim().to_string();
    if token.is_empty() {
        return None;
    }
    let payload_b64 = token.split('.').nth(1)?;
    let payload_bytes = base64_decode(&pad_base64url(payload_b64))?;
    let payload_str = String::from_utf8(payload_bytes).ok()?;
    let exp = json_i64(&payload_str, "exp")?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    if exp <= now + 60 {
        return None;
    }

    let discord_id = json_str(&payload_str, "sub")?;
    let avatar =
        json_str(&payload_str, "avatar").or_else(|| json_str(&payload_str, "avatar_hash"))?;
    if avatar.is_empty() || avatar == "null" {
        return None;
    }
    let ext = if avatar.starts_with("a_") {
        "gif"
    } else {
        "png"
    };
    Some(format!(
        "https://cdn.discordapp.com/avatars/{discord_id}/{avatar}.{ext}?size=128"
    ))
}

fn parse_profile(json: &str) -> Option<ProfileData> {
    Some(ProfileData {
        username: json_str(json, "username")?,
        rating: json_f64(json, "rating")? as i32,
        deviation: json_f64(json, "deviation")? as i32,
        wins: json_u64(json, "wins")?,
        losses: json_u64(json, "losses")?,
        matches_played: json_u64(json, "matches_played")?,
        avatar_url: json_str(json, "avatar_url"),
    })
}

fn parse_history(json: &str) -> Option<Vec<HistoryRow>> {
    // Server returns `{"matches":[{...},{...}]}`. We walk the array
    // tolerantly: split on `}{` boundaries inside the matches list,
    // parse each chunk for the fields we care about, drop whichever
    // entries fail to parse rather than the whole list.
    let arr_start = json.find("\"matches\"")? + "\"matches\"".len();
    let after_colon = json[arr_start..].find('[')? + arr_start + 1;
    let arr_end = json[after_colon..].rfind(']')? + after_colon;
    let body = &json[after_colon..arr_end];

    let mut rows = Vec::new();
    let mut depth = 0i32;
    let mut start: Option<usize> = None;
    for (i, c) in body.char_indices() {
        match c {
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start {
                        let chunk = &body[s..=i];
                        if let Some(row) = parse_history_row(chunk) {
                            rows.push(row);
                        }
                    }
                    start = None;
                }
            }
            _ => {}
        }
    }
    Some(rows)
}

fn parse_history_row(chunk: &str) -> Option<HistoryRow> {
    Some(HistoryRow {
        opponent_username: json_str(chunk, "opponent_username")?,
        result: json_str(chunk, "result")?,
        our_score: json_u64(chunk, "our_score")? as u16,
        opponent_score: json_u64(chunk, "opponent_score")? as u16,
        played_at: json_str(chunk, "played_at").unwrap_or_default(),
    })
}

fn parse_leaderboard(json: &str) -> Option<Vec<LeaderboardRow>> {
    let body = json_array_body(json, "players")
        .or_else(|| json_array_body(json, "leaderboard"))
        .or_else(|| json_array_body(json, "rows"))
        .or_else(|| json_array_body(json, "entries"))
        .or_else(|| json.trim().strip_prefix('[')?.strip_suffix(']'))?;
    let mut rows = Vec::new();
    for chunk in json_object_chunks(body) {
        if let Some(row) = parse_leaderboard_row(chunk) {
            rows.push(row);
        }
    }
    Some(rows)
}

fn parse_leaderboard_row(chunk: &str) -> Option<LeaderboardRow> {
    Some(LeaderboardRow {
        username: json_str(chunk, "username")
            .or_else(|| json_str(chunk, "name"))
            .or_else(|| json_str(chunk, "display_name"))
            .unwrap_or_else(|| "Unknown".into()),
        rating: json_f64(chunk, "rating")
            .or_else(|| json_f64(chunk, "mu"))?
            .round() as i32,
        wins: json_u64(chunk, "wins").unwrap_or(0),
        losses: json_u64(chunk, "losses").unwrap_or(0),
    })
}

fn parse_public_replay_index(json: &str, index_url: &str) -> Option<Vec<RemoteReplayMeta>> {
    let body = json_array_body(json, "replays")?;
    let mut out = Vec::new();
    for chunk in json_object_chunks(body) {
        if let Some(replay) = parse_public_replay_meta(chunk, index_url) {
            out.push(replay);
        }
    }
    Some(out)
}

fn parse_public_replay_meta(chunk: &str, index_url: &str) -> Option<RemoteReplayMeta> {
    let file = json_str(chunk, "file").unwrap_or_default();
    let url = json_str(chunk, "url").unwrap_or_else(|| join_url(index_url, &file));
    if !url.starts_with("https://") {
        return None;
    }
    Some(RemoteReplayMeta {
        filename: if file.is_empty() {
            url.rsplit('/').next().unwrap_or("replay.ncrp").to_string()
        } else {
            file.rsplit('/').next().unwrap_or(&file).to_string()
        },
        url,
        p1_name: json_str(chunk, "p1").unwrap_or_else(|| "P1".into()),
        p2_name: json_str(chunk, "p2").unwrap_or_else(|| "P2".into()),
        p1_score: json_u64(chunk, "p1_score").map(|v| v.min(u16::MAX as u64) as u16),
        p2_score: json_u64(chunk, "p2_score").map(|v| v.min(u16::MAX as u64) as u16),
        winner: json_str(chunk, "winner").unwrap_or_default(),
        frame_count: json_u64(chunk, "frames").unwrap_or(0).min(u32::MAX as u64) as u32,
        duration: json_str(chunk, "duration").unwrap_or_default(),
        recorded_at: json_str(chunk, "recorded_at")
            .or_else(|| json_u64(chunk, "recorded_unix").map(|v| v.to_string()))
            .unwrap_or_default(),
    })
}

fn join_url(index_url: &str, file: &str) -> String {
    if file.starts_with("https://") {
        return file.to_string();
    }
    let Some(slash) = index_url.rfind('/') else {
        return file.to_string();
    };
    format!("{}/{}", &index_url[..slash], file.trim_start_matches('/'))
}

// ── freeplay-stats: ghost browse + download ───────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteGhostMeta {
    pub ghost_id: String,
    pub filename: String,
    pub username: String,
    pub frame_count: u32,
}

#[derive(Debug)]
pub enum GhostListUpdate {
    Loaded(Vec<RemoteGhostMeta>),
    Error(String),
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum GhostDownloadUpdate {
    /// Download succeeded; file is at `local_path`. Caller may now load it
    /// the same way they would a local-ghost path.
    Saved {
        ghost_id: String,
        local_path: String,
    },
    Error {
        ghost_id: String,
        message: String,
    },
}

/// Fetch the community ghost catalogue, filtered to ROM-compatible entries.
/// `rom_hash` is the same hex string passed to `/match/lfg` so the server
/// returns only ghosts recorded against the same ROM (loading mismatched
/// .ncgh files would teleport fighters or load junk savestates).
pub fn fetch_ghost_list(stats_url: String, rom_hash: String, tx: Sender<GhostListUpdate>) {
    std::thread::spawn(move || {
        if stats_url.is_empty() {
            let _ = tx.send(GhostListUpdate::Error("stats_url not configured".into()));
            return;
        }
        let url = format!("{stats_url}/ghosts/list?rom_hash={rom_hash}&limit=50");
        match http_get_no_auth(&url) {
            Ok(body) => match parse_ghost_list(&body) {
                Some(ghosts) => {
                    let _ = tx.send(GhostListUpdate::Loaded(ghosts));
                }
                None => {
                    let _ = tx.send(GhostListUpdate::Error(format!(
                        "Couldn't parse ghost list: {body}"
                    )));
                }
            },
            Err(e) => {
                let _ = tx.send(GhostListUpdate::Error(format!("{e}")));
            }
        }
    });
}

pub fn watch_spectate_state(session_id: String, tx: Sender<SpectateUpdate>) {
    std::thread::spawn(move || {
        let base_url = match signaling_url() {
            Ok(url) => url,
            Err(e) => {
                let _ = tx.send(SpectateUpdate::Error(e));
                return;
            }
        };
        let url = format!("{base_url}/spectate/state/{session_id}");
        loop {
            match http_get_no_auth(&url) {
                Ok(body) => match parse_spectate_state(&body) {
                    Some(state) => {
                        if tx.send(SpectateUpdate::State(state)).is_err() {
                            break;
                        }
                    }
                    None => {
                        if tx
                            .send(SpectateUpdate::Error(format!(
                                "Couldn't parse spectator state: {body}"
                            )))
                            .is_err()
                        {
                            break;
                        }
                    }
                },
                Err(e) => {
                    if tx.send(SpectateUpdate::Error(e)).is_err() {
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
    });
}

pub fn fetch_leaderboard(stats_url: String, tx: Sender<LeaderboardUpdate>) {
    std::thread::spawn(move || {
        if stats_url.is_empty() {
            let _ = tx.send(LeaderboardUpdate::Error("stats_url not configured".into()));
            return;
        }

        let url = format!("{stats_url}/leaderboard?limit=20");
        match http_get_no_auth(&url) {
            Ok(body) => match parse_leaderboard(&body) {
                Some(rows) => {
                    let _ = tx.send(LeaderboardUpdate::Loaded(rows));
                }
                None => {
                    let _ = tx.send(LeaderboardUpdate::Error(format!(
                        "Couldn't parse leaderboard: {body}"
                    )));
                }
            },
            Err(e) => {
                let _ = tx.send(LeaderboardUpdate::Error(format!(
                    "Leaderboard fetch failed: {e}"
                )));
            }
        }
    });
}

pub fn fetch_public_replays(stats_url: String, tx: Sender<PublicReplayUpdate>) {
    std::thread::spawn(move || {
        let mut index_urls = Vec::new();
        let stats_url = stats_url.trim_end_matches('/').to_string();
        if !stats_url.is_empty() {
            index_urls.push(format!("{stats_url}/replays/list?limit=50"));
        }
        index_urls.push("https://junkwax.github.io/freeplay-gametalk/replays/replays.json".into());
        index_urls.push(
            "https://raw.githubusercontent.com/junkwax/freeplay-gametalk/main/docs/replays/replays.json"
                .into(),
        );

        let mut last_error = String::new();
        for index_url in index_urls {
            match http_get_no_auth(&index_url) {
                Ok(body) => match parse_public_replay_index(&body, &index_url) {
                    Some(replays) => {
                        let _ = tx.send(PublicReplayUpdate::Loaded(replays));
                        return;
                    }
                    None => {
                        last_error = format!("couldn't parse replay index from {index_url}");
                    }
                },
                Err(e) => {
                    last_error = format!("{index_url}: {e}");
                }
            }
        }
        let _ = tx.send(PublicReplayUpdate::Error(last_error));
    });
}

pub fn fetch_live_matches(tx: Sender<LiveMatchesUpdate>) {
    std::thread::spawn(move || {
        let base_url = match signaling_url() {
            Ok(url) => url,
            Err(e) => {
                let _ = tx.send(LiveMatchesUpdate::Error(e));
                return;
            }
        };
        let url = format!("{base_url}/matches/live");
        match http_get_no_auth(&url) {
            Ok(body) => match parse_live_matches(&body) {
                Some(matches) => {
                    let _ = tx.send(LiveMatchesUpdate::Loaded(matches));
                }
                None => {
                    let _ = tx.send(LiveMatchesUpdate::Error(format!(
                        "Couldn't parse live matches: {body}"
                    )));
                }
            },
            Err(e) => {
                let _ = tx.send(LiveMatchesUpdate::Error(e));
            }
        }
    });
}

pub fn fetch_general_lobby(tx: Sender<LobbyUpdate>) {
    std::thread::spawn(move || {
        let base_url = match signaling_url() {
            Ok(url) => url,
            Err(e) => {
                let _ = tx.send(LobbyUpdate::Error(e));
                return;
            }
        };
        let url = format!("{base_url}/lobby/general");
        match http_get_optional_auth(&url) {
            Ok(body) => match parse_lobby_snapshot(&body) {
                Some(snapshot) => {
                    let _ = tx.send(LobbyUpdate::Loaded(snapshot));
                }
                None => {
                    let _ = tx.send(LobbyUpdate::Error(format!(
                        "Couldn't parse lobby snapshot: {body}"
                    )));
                }
            },
            Err(e) => {
                let _ = tx.send(LobbyUpdate::Error(e));
            }
        }
    });
}

pub fn fetch_lobbies(tx: Sender<LobbyListUpdate>) {
    std::thread::spawn(move || {
        let base_url = match signaling_url() {
            Ok(url) => url,
            Err(e) => {
                let _ = tx.send(LobbyListUpdate::Error(e));
                return;
            }
        };
        let url = format!("{base_url}/koh");
        match http_get_optional_auth(&url) {
            Ok(body) => match parse_lobbies(&body) {
                Some(lobbies) => {
                    let _ = tx.send(LobbyListUpdate::Loaded(lobbies));
                }
                None => {
                    let _ = tx.send(LobbyListUpdate::Error(format!(
                        "Couldn't parse lobby list: {body}"
                    )));
                }
            },
            Err(e) => {
                let _ = tx.send(LobbyListUpdate::Error(e));
            }
        }
    });
}

/// Token from cache (Discord) or a fresh guest login, no status channel.
fn auth_token_quiet() -> Result<String, String> {
    if let Some(t) = read_cached_token() {
        set_current_token(&t);
        return Ok(t);
    }
    let t = guest_login()?;
    set_current_token(&t);
    Ok(t)
}

/// POST /koh — create a king-of-the-hill lobby. `format` is the wire string.
pub fn create_lobby(
    tx: Sender<LobbyViewUpdate>,
    name: String,
    ranked: bool,
    private: bool,
    format: String,
) {
    std::thread::spawn(move || {
        let result = (|| -> Result<String, String> {
            let token = auth_token_quiet()?;
            let stun = stun_discover(GAME_PORT).map_err(|e| format!("STUN failed: {e}"))?;
            let rom_hash = rom_short_hash();
            let body = format!(
                r#"{{"name":"{}","ranked":{ranked},"private":{private},"format":"{format}","stun_endpoint":"{stun}","app_version":"{APP_VERSION}","rom_hash":"{rom_hash}"}}"#,
                json_escape(&name)
            );
            let url = format!("{}/koh", signaling_url()?);
            let resp = http_post_json(&url, &token, &body)?;
            json_str(&resp, "lobby_id")
                .ok_or_else(|| format!("create lobby missing lobby_id: {resp}"))
        })();
        let _ = tx.send(match result {
            Ok(id) => LobbyViewUpdate::Created(id),
            Err(e) => LobbyViewUpdate::Error(e),
        });
    });
}

/// POST /koh/:id/join — join the play queue (or as a spectator).
pub fn join_lobby(tx: Sender<LobbyViewUpdate>, lobby_id: String, spectate: bool) {
    std::thread::spawn(move || {
        let result = (|| -> Result<LobbyView, String> {
            let token = auth_token_quiet()?;
            let stun = stun_discover(GAME_PORT).map_err(|e| format!("STUN failed: {e}"))?;
            let rom_hash = rom_short_hash();
            let body = format!(
                r#"{{"stun_endpoint":"{stun}","spectate":{spectate},"app_version":"{APP_VERSION}","rom_hash":"{rom_hash}"}}"#
            );
            let url = format!("{}/koh/{lobby_id}/join", signaling_url()?);
            let resp = http_post_json(&url, &token, &body)?;
            parse_lobby_view(&resp).ok_or_else(|| format!("join lobby parse failed: {resp}"))
        })();
        let _ = tx.send(match result {
            Ok(v) => LobbyViewUpdate::Loaded(v),
            Err(e) => LobbyViewUpdate::Error(e),
        });
    });
}

/// GET /koh/:id — poll lobby state.
pub fn fetch_lobby(tx: Sender<LobbyViewUpdate>, lobby_id: String) {
    std::thread::spawn(move || {
        let result = (|| -> Result<LobbyView, String> {
            let token = auth_token_quiet()?;
            let url = format!("{}/koh/{lobby_id}", signaling_url()?);
            let resp = http_get(&url, &token)?;
            parse_lobby_view(&resp).ok_or_else(|| format!("lobby parse failed: {resp}"))
        })();
        let _ = tx.send(match result {
            Ok(v) => LobbyViewUpdate::Loaded(v),
            Err(e) => LobbyViewUpdate::Error(e),
        });
    });
}

/// POST /koh/:id/leave — fire-and-forget.
pub fn leave_lobby(lobby_id: String) {
    std::thread::spawn(move || {
        let Some(token) = current_token() else { return };
        if let Ok(url) = signaling_url().map(|u| format!("{u}/koh/{lobby_id}/leave")) {
            let _ = http_post_json(&url, &token, "{}");
        }
    });
}

fn parse_lobby_view(json: &str) -> Option<LobbyView> {
    let id = json_str(json, "id")?;
    let members = json_array_body(json, "members")
        .map(|body| {
            json_object_chunks(body)
                .iter()
                .filter_map(|c| {
                    Some(LobbyMemberInfo {
                        username: json_str(c, "username")?,
                        rating: json_f64(c, "rating").map(|r| r as i32),
                        queued: json_str(c, "role").as_deref() == Some("queued"),
                        in_match: json_bool(c, "in_match").unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let current = if json.contains("\"current\"") {
        match (
            json_nested_str(json, "current", "host_username"),
            json_nested_str(json, "current", "join_username"),
        ) {
            (Some(h), Some(j)) => Some(LobbyCurrent {
                host_username: h,
                join_username: j,
                host_session: json_nested_str(json, "current", "host_session").unwrap_or_default(),
                join_session: json_nested_str(json, "current", "join_session").unwrap_or_default(),
            }),
            _ => None,
        }
    } else {
        None
    };
    Some(LobbyView {
        id,
        name: json_str(json, "name").unwrap_or_else(|| "Lobby".into()),
        ranked: json_bool(json, "ranked").unwrap_or(false),
        private: json_bool(json, "private").unwrap_or(false),
        format: json_str(json, "format")
            .and_then(|f| parse_lobby_match_format(&f))
            .unwrap_or(LobbyMatchFormat::UnrankedVs),
        members,
        queue: json_string_array(json, "queue"),
        current,
        your_position: json_u64(json, "your_position").map(|p| p as usize),
        your_queued: json_str(json, "your_role").as_deref() == Some("queued"),
        your_session: json_str(json, "your_session"),
        your_turn: json.contains("\"your_match\""),
    })
}

pub fn send_lobby_chat(message: String, tx: Sender<LobbyChatPostUpdate>) {
    std::thread::spawn(move || {
        let message = message.trim().to_string();
        if message.is_empty() {
            let _ = tx.send(LobbyChatPostUpdate::Error("Message is empty".into()));
            return;
        }
        if message.chars().count() > 180 {
            let _ = tx.send(LobbyChatPostUpdate::Error("Message is too long".into()));
            return;
        }
        let token = match current_or_cached_token().or_else(|| match guest_login() {
            Ok(token) => {
                set_current_token(&token);
                Some(token)
            }
            Err(_) => None,
        }) {
            Some(token) => token,
            None => {
                let _ = tx.send(LobbyChatPostUpdate::Error(
                    "Couldn't create a lobby session".into(),
                ));
                return;
            }
        };
        let base_url = match signaling_url() {
            Ok(url) => url,
            Err(e) => {
                let _ = tx.send(LobbyChatPostUpdate::Error(e));
                return;
            }
        };
        let body = format!(r#"{{"message":"{}"}}"#, json_escape(&message));
        let url = format!("{base_url}/lobby/chat");
        match http_post_json(&url, &token, &body) {
            Ok(_) => {
                let _ = tx.send(LobbyChatPostUpdate::Sent);
            }
            Err(e) => {
                let _ = tx.send(LobbyChatPostUpdate::Error(e));
            }
        }
    });
}

fn parse_spectate_state(json: &str) -> Option<SpectateState> {
    let frame = json_last_u64(json, "frame").map(|v| v as u32);
    let p1_score = json_last_u64(json, "score_p1")
        .or_else(|| json_last_u64(json, "p1_score"))
        .unwrap_or(0) as u32;
    let p2_score = json_last_u64(json, "score_p2")
        .or_else(|| json_last_u64(json, "p2_score"))
        .unwrap_or(0) as u32;
    let updated_at = json_last_str(json, "updated_at")
        .or_else(|| json_last_str(json, "updatedAt"))
        .or_else(|| json_last_str(json, "timestamp"));

    if frame.is_none() && !json.contains("score_p1") && !json.contains("p1_score") {
        return None;
    }

    Some(SpectateState {
        frame,
        p1_score,
        p2_score,
        updated_at,
    })
}

fn parse_live_matches(json: &str) -> Option<Vec<LiveMatch>> {
    let body = json_array_body(json, "matches").or_else(|| json_array_body(json, "live"))?;
    let mut out = Vec::new();
    for chunk in json_object_chunks(body) {
        if let Some(m) = parse_live_match(chunk) {
            out.push(m);
        }
    }
    Some(out)
}

fn parse_live_match(chunk: &str) -> Option<LiveMatch> {
    let session_id = json_str(chunk, "session_id")
        .or_else(|| json_str(chunk, "room_id"))
        .or_else(|| json_str(chunk, "id"))?;
    let p1_name = json_str(chunk, "p1_name")
        .or_else(|| json_str(chunk, "player1"))
        .or_else(|| json_str(chunk, "host_username"))
        .unwrap_or_else(|| "P1".into());
    let p2_name = json_str(chunk, "p2_name")
        .or_else(|| json_str(chunk, "player2"))
        .or_else(|| json_str(chunk, "join_username"))
        .unwrap_or_else(|| "P2".into());
    let p1_score = json_u64(chunk, "p1_score")
        .or_else(|| json_u64(chunk, "score_p1"))
        .unwrap_or(0) as u32;
    let p2_score = json_u64(chunk, "p2_score")
        .or_else(|| json_u64(chunk, "score_p2"))
        .unwrap_or(0) as u32;

    Some(LiveMatch {
        session_id,
        p1_name,
        p2_name,
        p1_score,
        p2_score,
    })
}

fn parse_lobby_snapshot(json: &str) -> Option<LobbySnapshot> {
    let users = parse_lobby_users(json);
    let chat = parse_lobby_chat(json);
    let status = json_str(json, "message")
        .or_else(|| json_str(json, "status"))
        .unwrap_or_else(|| "General lobby".into());

    if users.is_empty() && chat.is_empty() && !json.contains("status") && !json.contains("message")
    {
        return None;
    }

    Some(LobbySnapshot {
        users,
        chat,
        status,
    })
}

fn parse_lobby_users(json: &str) -> Vec<LobbyUser> {
    let Some(body) = json_array_body(json, "users")
        .or_else(|| json_array_body(json, "players"))
        .or_else(|| json_array_body(json, "online"))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for chunk in json_object_chunks(body) {
        let username = json_str(chunk, "username")
            .or_else(|| json_str(chunk, "name"))
            .or_else(|| json_str(chunk, "display_name"));
        if let Some(username) = username {
            out.push(LobbyUser {
                player_id: json_str(chunk, "player_id")
                    .or_else(|| json_str(chunk, "id"))
                    .or_else(|| json_str(chunk, "discord_id"))
                    .unwrap_or_default(),
                username,
                status: json_str(chunk, "status").unwrap_or_else(|| "online".into()),
                rating: json_f64(chunk, "rating").map(|r| r as i32),
            });
        }
    }
    out
}

fn parse_lobby_chat(json: &str) -> Vec<LobbyChatMessage> {
    let Some(body) = json_array_body(json, "chat").or_else(|| json_array_body(json, "messages"))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for chunk in json_object_chunks(body) {
        let message = json_str(chunk, "message")
            .or_else(|| json_str(chunk, "text"))
            .or_else(|| json_str(chunk, "body"));
        if let Some(message) = message {
            out.push(LobbyChatMessage {
                username: json_str(chunk, "username")
                    .or_else(|| json_str(chunk, "name"))
                    .unwrap_or_else(|| "System".into()),
                message,
                timestamp: json_str(chunk, "timestamp")
                    .or_else(|| json_str(chunk, "created_at"))
                    .or_else(|| json_str(chunk, "updated_at")),
            });
        }
    }
    out
}

fn parse_lobbies(json: &str) -> Option<Vec<LobbyRoom>> {
    let body = json_array_body(json, "lobbies").or_else(|| json_array_body(json, "rooms"))?;
    let mut out = Vec::new();
    for chunk in json_object_chunks(body) {
        if let Some(room) = parse_lobby_room(chunk) {
            out.push(room);
        }
    }
    Some(out)
}

fn parse_lobby_room(chunk: &str) -> Option<LobbyRoom> {
    let id = json_str(chunk, "id")
        .or_else(|| json_str(chunk, "lobby_id"))
        .or_else(|| json_str(chunk, "room_id"))?;
    let format = json_str(chunk, "format")
        .or_else(|| json_str(chunk, "match_format"))
        .and_then(|raw| parse_lobby_match_format(&raw))
        .unwrap_or(LobbyMatchFormat::UnrankedVs);
    Some(LobbyRoom {
        id,
        name: json_str(chunk, "name")
            .or_else(|| json_str(chunk, "title"))
            .unwrap_or_else(|| "Lobby".into()),
        host_username: json_str(chunk, "host_username")
            .or_else(|| json_str(chunk, "host"))
            .or_else(|| json_str(chunk, "owner_username"))
            .unwrap_or_else(|| "Host".into()),
        format,
        players: json_u64(chunk, "players")
            .or_else(|| json_u64(chunk, "player_count"))
            .or_else(|| json_u64(chunk, "count"))
            .unwrap_or(1)
            .min(u8::MAX as u64) as u8,
        private: json_bool(chunk, "private").unwrap_or(false),
        status: json_str(chunk, "status").unwrap_or_else(|| "open".into()),
    })
}

fn parse_lobby_match_format(raw: &str) -> Option<LobbyMatchFormat> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "vs" | "unranked_vs" | "unranked-vs" | "unranked vs" | "casual" => {
            Some(LobbyMatchFormat::UnrankedVs)
        }
        "ft3" | "ranked_ft3" | "ranked-ft3" | "ranked ft3" => Some(LobbyMatchFormat::RankedFt3),
        "ft5" | "ranked_ft5" | "ranked-ft5" | "ranked ft5" => Some(LobbyMatchFormat::RankedFt5),
        "ft10" | "ranked_ft10" | "ranked-ft10" | "ranked ft10" => {
            Some(LobbyMatchFormat::RankedFt10)
        }
        _ => None,
    }
}

fn parse_ghost_list(json: &str) -> Option<Vec<RemoteGhostMeta>> {
    let body = json_array_body(json, "ghosts")?;
    let mut out = Vec::new();
    for chunk in json_object_chunks(body) {
        if let Some(g) = parse_ghost_meta(chunk) {
            out.push(g);
        }
    }
    Some(out)
}

fn json_array_body<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let arr_start = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let after_colon = json[arr_start..].find('[')? + arr_start + 1;
    let mut depth = 1i32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, c) in json[after_colon..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&json[after_colon..after_colon + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Collect the quoted strings in a JSON string array (e.g. `["a","b"]`).
fn json_string_array(json: &str, key: &str) -> Vec<String> {
    let Some(body) = json_array_body(json, key) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut chars = body.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == '"' {
            chars.next();
            let mut s = String::new();
            while let Some(ch) = chars.next() {
                match ch {
                    '\\' => {
                        if let Some(esc) = chars.next() {
                            s.push(esc);
                        }
                    }
                    '"' => break,
                    _ => s.push(ch),
                }
            }
            out.push(s);
        } else {
            chars.next();
        }
    }
    out
}

fn json_object_chunks(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start: Option<usize> = None;
    for (i, c) in body.char_indices() {
        match c {
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start {
                        out.push(&body[s..=i]);
                    }
                    start = None;
                }
            }
            _ => {}
        }
    }
    out
}

fn parse_ghost_meta(chunk: &str) -> Option<RemoteGhostMeta> {
    Some(RemoteGhostMeta {
        ghost_id: json_str(chunk, "ghost_id")?,
        filename: json_str(chunk, "filename")?,
        username: json_str(chunk, "username").unwrap_or_default(),
        frame_count: json_u64(chunk, "frame_count")? as u32,
    })
}

/// Fire-and-forget downloader. Writes the .ncgh bytes to `local_path` and
/// pushes a `GhostDownloadUpdate` to `tx` when done. Caller is responsible
/// for choosing a path that doesn't collide with existing local recordings.
pub fn download_ghost(
    stats_url: String,
    ghost_id: String,
    local_path: String,
    tx: Sender<GhostDownloadUpdate>,
) {
    std::thread::spawn(move || {
        let url = format!("{stats_url}/ghosts/download/{ghost_id}");
        match http_get_bytes(&url) {
            Ok(bytes) => {
                if let Some(parent) = std::path::Path::new(&local_path).parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        let _ = tx.send(GhostDownloadUpdate::Error {
                            ghost_id,
                            message: format!("create {}: {e}", parent.display()),
                        });
                        return;
                    }
                }
                if let Err(e) = std::fs::write(&local_path, &bytes) {
                    let _ = tx.send(GhostDownloadUpdate::Error {
                        ghost_id,
                        message: format!("write {local_path}: {e}"),
                    });
                    return;
                }
                let _ = tx.send(GhostDownloadUpdate::Saved {
                    ghost_id,
                    local_path,
                });
            }
            Err(e) => {
                let _ = tx.send(GhostDownloadUpdate::Error {
                    ghost_id,
                    message: e,
                });
            }
        }
    });
}

/// Binary GET — like `http_get_no_auth` but returns raw bytes. The .ncgh
/// download endpoint serves `application/octet-stream`; treating it as
/// UTF-8 would corrupt the file.
pub(crate) fn http_get_bytes(url: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let (host, path) = parse_url(url)?;
    let mut stream = tls_connect(&host)?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| e.to_string())?;
    if !status_line.contains(" 200 ") {
        return Err(format!("HTTP {}", status_line.trim()));
    }

    // Skip headers, capturing Content-Length/Encoding if present. Stop at the blank
    // line. We don't support chunked encoding here — the stats service
    // returns a finite Content-Length for binary payloads.
    let mut content_length: Option<usize> = None;
    let mut content_encoding = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if trimmed.to_lowercase().starts_with("content-length:") {
            content_length = trimmed
                .split(':')
                .nth(1)
                .and_then(|s| s.trim().parse().ok());
        } else if trimmed.to_lowercase().starts_with("content-encoding:") {
            content_encoding = trimmed
                .split(':')
                .nth(1)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
        }
    }

    let mut bytes = Vec::new();
    if let Some(len) = content_length {
        bytes.resize(len, 0);
        reader.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    } else {
        reader.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    }
    if content_encoding == "gzip" {
        let mut decoder = flate2::read::GzDecoder::new(bytes.as_slice());
        let mut decoded = Vec::new();
        decoder
            .read_to_end(&mut decoded)
            .map_err(|e| format!("gzip decode: {e}"))?;
        Ok(decoded)
    } else {
        Ok(bytes)
    }
}

pub fn post_match_result(
    token: &str,
    session_id: &str,
    match_index: u32,
    p1_score: u16,
    p2_score: u16,
) -> Result<(), String> {
    let body = match_result_body(session_id, match_index, p1_score, p2_score);
    let url = format!("{}/match/result", signaling_url()?);
    http_post_json(&url, token, &body)?;
    Ok(())
}

fn match_result_body(session_id: &str, match_index: u32, p1_score: u16, p2_score: u16) -> String {
    format!(
        r#"{{"session_id":"{}","match_index":{},"p1_score":{},"p2_score":{}}}"#,
        json_escape(session_id),
        match_index,
        p1_score,
        p2_score
    )
}

// ── UDP hole punch ────────────────────────────────────────────────────────────

fn hole_punch(peer_endpoint: &str, punch_at_ms: i64) -> Result<SocketAddr, String> {
    let peer: SocketAddr = peer_endpoint
        .parse()
        .map_err(|e| format!("Bad peer addr '{peer_endpoint}': {e}"))?;

    let sock = UdpSocket::bind(format!("0.0.0.0:{GAME_PORT}"))
        .map_err(|e| format!("UDP bind for punch on :{GAME_PORT}: {e}"))?;
    sock.set_read_timeout(Some(Duration::from_millis(50))).ok();

    wait_until_ms(punch_at_ms);

    println!("[mm] punching UDP hole to {peer}");
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut last_send = Instant::now() - Duration::from_secs(1);

    loop {
        if Instant::now() > deadline {
            return Err(format!(
                "Hole punch timeout — peer {peer} did not respond. \
                 Both players may be behind strict/symmetric NAT."
            ));
        }
        if last_send.elapsed().as_millis() >= 100 {
            sock.send_to(PUNCH_PAYLOAD, peer).ok();
            last_send = Instant::now();
        }
        let mut buf = [0u8; 64];
        match sock.recv_from(&mut buf) {
            Ok((n, from)) if from == peer && &buf[..n] == PUNCH_PAYLOAD => {
                sock.send_to(PUNCH_PAYLOAD, peer).ok();
                return Ok(peer);
            }
            Ok(_) => {}
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => println!("[mm] punch recv: {e}"),
        }
    }
}

fn wait_until_ms(target_ms: i64) {
    let now_ms = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    };
    let remaining = target_ms - now_ms();
    if remaining > 10 {
        std::thread::sleep(Duration::from_millis((remaining - 10) as u64));
    }
    while now_ms() < target_ms {
        std::hint::spin_loop();
    }
}

// ── Minimal JSON parsing ──────────────────────────────────────────────────────

fn json_str(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let start = json.find(&pat)? + pat.len();
    let end = json[start..].find('"')?;
    Some(json[start..start + end].to_string())
}

fn json_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn json_last_str(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let start = json.rfind(&pat)? + pat.len();
    let end = json[start..].find('"')?;
    Some(json[start..start + end].to_string())
}

fn json_nested_str(json: &str, outer: &str, inner: &str) -> Option<String> {
    let pat = format!("\"{outer}\":{{");
    let start = json.find(&pat)? + pat.len() - 1;
    let sub = &json[start..];
    json_str(sub, inner)
}

fn json_nested_i64(json: &str, outer: &str, inner: &str) -> Option<i64> {
    let pat_outer = format!("\"{outer}\":{{");
    let start = json.find(&pat_outer)? + pat_outer.len() - 1;
    let sub = &json[start..];
    let pat = format!("\"{inner}\":");
    let val_start = sub.find(&pat)? + pat.len();
    let val_sub = &sub[val_start..];
    let val_end = val_sub
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(val_sub.len());
    val_sub[..val_end].parse().ok()
}

fn json_i64(json: &str, key: &str) -> Option<i64> {
    let pat = format!("\"{key}\":");
    let start = json.find(&pat)? + pat.len();
    let rest = json[start..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn json_u64(json: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\":");
    let start = json.find(&pat)? + pat.len();
    let rest = json[start..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn json_bool(json: &str, key: &str) -> Option<bool> {
    let pat = format!("\"{key}\":");
    let start = json.find(&pat)? + pat.len();
    let rest = json[start..].trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn json_last_u64(json: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\":");
    let start = json.rfind(&pat)? + pat.len();
    let rest = json[start..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn json_f64(json: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{key}\":");
    let start = json.find(&pat)? + pat.len();
    let rest = json[start..].trim_start();
    let end = rest
        .find(|c: char| {
            !c.is_ascii_digit() && c != '.' && c != '-' && c != 'e' && c != 'E' && c != '+'
        })
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{
        match_result_body, parse_live_matches, parse_lobbies, parse_lobby_snapshot,
        parse_spectate_state, sha256_hex, LobbyMatchFormat,
    };

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn match_result_body_includes_match_index_and_escapes_session_id() {
        let body = match_result_body("room\"\\\nend", 12, 2, 1);

        assert_eq!(
            body,
            r#"{"session_id":"room\"\\\nend","match_index":12,"p1_score":2,"p2_score":1}"#
        );
    }

    #[test]
    fn parse_spectate_state_accepts_current_and_legacy_score_keys() {
        let state = parse_spectate_state(
            r#"{"frame":120,"score_p1":1,"score_p2":0,"updated_at":"2026-06-13T12:00:00Z"}"#,
        )
        .expect("current spectate payload should parse");

        assert_eq!(state.frame, Some(120));
        assert_eq!(state.p1_score, 1);
        assert_eq!(state.p2_score, 0);
        assert_eq!(state.updated_at.as_deref(), Some("2026-06-13T12:00:00Z"));

        let legacy =
            parse_spectate_state(r#"{"frame":121,"p1_score":2,"p2_score":1,"timestamp":"later"}"#)
                .expect("legacy spectate payload should parse");

        assert_eq!(legacy.frame, Some(121));
        assert_eq!(legacy.p1_score, 2);
        assert_eq!(legacy.p2_score, 1);
        assert_eq!(legacy.updated_at.as_deref(), Some("later"));
    }

    #[test]
    fn parse_spectate_state_uses_last_frame_and_update_values() {
        let state = parse_spectate_state(
            r#"{"frame":10,"score_p1":0,"score_p2":0,"state":{"frame":44,"score_p1":2,"score_p2":1,"updatedAt":"fresh"}}"#,
        )
        .expect("nested latest state should parse");

        assert_eq!(state.frame, Some(44));
        assert_eq!(state.p1_score, 2);
        assert_eq!(state.p2_score, 1);
        assert_eq!(state.updated_at.as_deref(), Some("fresh"));
    }

    #[test]
    fn parse_spectate_state_rejects_payload_without_frame_or_score() {
        assert!(parse_spectate_state(r#"{"updated_at":"soon"}"#).is_none());
    }

    #[test]
    fn parse_live_matches_accepts_field_aliases_and_skips_bad_rows() {
        let matches = parse_live_matches(
            r#"{"matches":[
                {"session_id":"s1","p1_name":"Kitana","p2_name":"Mileena","p1_score":1,"p2_score":0},
                {"room_id":"s2","player1":"Liu","player2":"Kung","score_p1":2,"score_p2":1},
                {"host_username":"NoSession","join_username":"Skipped"}
            ]}"#,
        )
        .expect("live matches payload should parse");

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].session_id, "s1");
        assert_eq!(matches[0].p1_name, "Kitana");
        assert_eq!(matches[0].p2_name, "Mileena");
        assert_eq!(matches[0].p1_score, 1);
        assert_eq!(matches[0].p2_score, 0);
        assert_eq!(matches[1].session_id, "s2");
        assert_eq!(matches[1].p1_name, "Liu");
        assert_eq!(matches[1].p2_name, "Kung");
        assert_eq!(matches[1].p1_score, 2);
        assert_eq!(matches[1].p2_score, 1);
    }

    #[test]
    fn parse_live_matches_accepts_live_array_alias_and_default_names() {
        let matches =
            parse_live_matches(r#"{"live":[{"id":"s3"}]}"#).expect("live array alias should parse");

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].session_id, "s3");
        assert_eq!(matches[0].p1_name, "P1");
        assert_eq!(matches[0].p2_name, "P2");
        assert_eq!(matches[0].p1_score, 0);
        assert_eq!(matches[0].p2_score, 0);
    }

    #[test]
    fn parse_lobby_snapshot_accepts_user_and_chat_aliases() {
        let snapshot = parse_lobby_snapshot(
            r#"{
                "message":"General lobby ready",
                "players":[
                    {"id":"p1","username":"Kitana","status":"idle"},
                    {"discord_id":"p2","display_name":"Mileena"}
                ],
                "messages":[
                    {"username":"Kitana","text":"ft5?","created_at":"now"},
                    {"name":"System","body":"Mileena joined"}
                ]
            }"#,
        )
        .expect("lobby snapshot should parse");

        assert_eq!(snapshot.status, "General lobby ready");
        assert_eq!(snapshot.users.len(), 2);
        assert_eq!(snapshot.users[0].player_id, "p1");
        assert_eq!(snapshot.users[0].username, "Kitana");
        assert_eq!(snapshot.users[0].status, "idle");
        assert_eq!(snapshot.users[1].player_id, "p2");
        assert_eq!(snapshot.users[1].username, "Mileena");
        assert_eq!(snapshot.users[1].status, "online");
        assert_eq!(snapshot.chat.len(), 2);
        assert_eq!(snapshot.chat[0].message, "ft5?");
        assert_eq!(snapshot.chat[0].timestamp.as_deref(), Some("now"));
        assert_eq!(snapshot.chat[1].username, "System");
    }

    #[test]
    fn parse_lobbies_accepts_room_aliases_and_formats() {
        let lobbies = parse_lobbies(
            r#"{"rooms":[
                {"room_id":"r1","title":"Long sets","host":"Jax","format":"ranked_ft10","player_count":2,"private":false,"status":"open"},
                {"lobby_id":"r2","name":"Casuals","owner_username":"Sonya","match_format":"vs","players":1,"private":true}
            ]}"#,
        )
        .expect("lobby list should parse");

        assert_eq!(lobbies.len(), 2);
        assert_eq!(lobbies[0].id, "r1");
        assert_eq!(lobbies[0].name, "Long sets");
        assert_eq!(lobbies[0].host_username, "Jax");
        assert_eq!(lobbies[0].format, LobbyMatchFormat::RankedFt10);
        assert_eq!(lobbies[0].players, 2);
        assert!(!lobbies[0].private);
        assert_eq!(lobbies[0].status, "open");

        assert_eq!(lobbies[1].id, "r2");
        assert_eq!(lobbies[1].host_username, "Sonya");
        assert_eq!(lobbies[1].format, LobbyMatchFormat::UnrankedVs);
        assert_eq!(lobbies[1].players, 1);
        assert!(lobbies[1].private);
    }
}
