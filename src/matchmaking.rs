//! Automated matchmaking client for Freeplay.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{mpsc::Sender, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub enum Update {
    Status(String),
    Connected {
        peer_endpoint: String,
        is_host: bool,
        turn: Option<TurnConnectInfo>,
        session_id: String,
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

#[derive(Debug, Clone)]
pub struct TurnConnectInfo {
    pub uri: String,
    pub username: String,
    pub password: String,
}

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const GAME_PORT: u16 = 7000;
const PUNCH_PAYLOAD: &[u8] = b"MK2PUNCH";

static CURRENT_TOKEN: Mutex<Option<String>> = Mutex::new(None);

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

fn set_current_token(token: &str) {
    if let Ok(mut g) = CURRENT_TOKEN.lock() {
        *g = Some(token.to_string());
    }
}

pub fn start(tx: Sender<Update>) {
    std::thread::spawn(move || {
        if let Err(e) = run(&tx) {
            let _ = tx.send(Update::Error(e));
        }
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

fn run(tx: &Sender<Update>) -> Result<(), String> {
    let mut maybe_session_id: Option<String> = None;

    let outcome = (|| -> Result<(), String> {
        let token = if let Some(cached) = read_cached_token() {
            send(tx, Update::Status("Using saved login...".into()))?;
            cached
        } else {
            send(tx, Update::Status("Opening Discord login...".into()))?;
            let fresh = discord_login(tx)?;
            write_cached_token(&fresh);
            fresh
        };
        set_current_token(&token);

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

        send(tx, Update::Status("Connecting to opponent...".into()))?;
        let session_id_for_update = session_id.clone();
        match hole_punch(&match_info.peer_endpoint, match_info.punch_at_ms) {
            Ok(peer_addr) => {
                println!("[mm] direct P2P established: {peer_addr}");
                send(
                    tx,
                    Update::Connected {
                        peer_endpoint: peer_addr.to_string(),
                        is_host: match_info.role == "host",
                        turn: None,
                        session_id: session_id_for_update,
                        peer_username: match_info.peer_username.clone(),
                    },
                )
            }
            Err(punch_err) => {
                println!("[mm] hole punch failed: {punch_err}");

                let Some(turn) = match_info.turn.clone() else {
                    return Err(format!(
                        "Direct P2P failed and no TURN relay configured: {punch_err}"
                    ));
                };

                send(tx, Update::Status("Connecting via relay...".into()))?;
                println!("[mm] falling back to TURN relay at {}", turn.uri);

                send(
                    tx,
                    Update::Connected {
                        peer_endpoint: match_info.peer_endpoint.clone(),
                        is_host: match_info.role == "host",
                        turn: Some(turn),
                        session_id: session_id_for_update,
                        peer_username: match_info.peer_username.clone(),
                    },
                )
            }
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

fn cancel_match(token: &str, session_id: &str) -> Result<(), String> {
    let url = format!("{}/match/cancel/{session_id}", signaling_url()?);
    http_post_json(&url, token, "{}").map(|_| ())
}

fn run_join_room(tx: &Sender<Update>, room_id: &str) -> Result<(), String> {
    let token = if let Some(cached) = read_cached_token() {
        send(tx, Update::Status("Using saved login...".into()))?;
        cached
    } else {
        send(tx, Update::Status("Opening Discord login...".into()))?;
        let fresh = discord_login(tx)?;
        write_cached_token(&fresh);
        fresh
    };
    set_current_token(&token);

    send(tx, Update::Status("Discovering network...".into()))?;
    let stun_endpoint = stun_discover(GAME_PORT).map_err(|e| format!("STUN failed: {e}"))?;
    println!("[mm] public endpoint: {stun_endpoint}");

    send(tx, Update::Status("Joining spar room...".into()))?;
    let match_info = join_room_http(&token, room_id, &stun_endpoint)?;

    if match_info.turn.is_some() {
        println!("[mm] TURN fallback available");
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
                    turn: None,
                    session_id: match_info.session_id,
                    peer_username: match_info.peer_username.clone(),
                },
            )
        }
        Err(punch_err) => {
            println!("[mm] hole punch failed: {punch_err}");

            let Some(turn) = match_info.turn.clone() else {
                return Err(format!(
                    "Direct P2P failed and no TURN relay configured: {punch_err}"
                ));
            };

            send(tx, Update::Status("Connecting via relay...".into()))?;
            println!("[mm] falling back to TURN relay at {}", turn.uri);

            send(
                tx,
                Update::Connected {
                    peer_endpoint: match_info.peer_endpoint.clone(),
                    is_host: match_info.role == "host",
                    turn: Some(turn),
                    session_id: match_info.session_id,
                    peer_username: match_info.peer_username.clone(),
                },
            )
        }
    }
}

struct JoinInfo {
    session_id: String,
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
    let punch_at_ms = json_nested_i64(&resp, "match_info", "punch_at_ms")
        .ok_or_else(|| format!("join response missing punch_at_ms: {resp}"))?;
    let role = json_nested_str(&resp, "match_info", "role").unwrap_or_else(|| "join".to_string());
    let peer_username = json_nested_str(&resp, "match_info", "username");
    let turn = parse_turn_creds(&resp);

    Ok(JoinInfo {
        session_id,
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

fn read_cached_token() -> Option<String> {
    let path = token_cache_path()?;
    let token = std::fs::read_to_string(&path).ok()?.trim().to_string();
    if token.is_empty() {
        return None;
    }

    let payload_b64 = token.split('.').nth(1)?;
    let padded = pad_base64url(payload_b64);
    let payload_bytes = base64_decode(&padded)?;
    let payload_str = String::from_utf8(payload_bytes).ok()?;

    let exp = json_i64(&payload_str, "exp")?;

    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    if exp <= now + 60 {
        println!("[mm] cached token expired, re-authenticating");
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

pub fn clear_cached_token() {
    if let Some(path) = token_cache_path() {
        let _ = std::fs::remove_file(&path);
        println!("[mm] cleared cached token");
    }
}

/// Extract the discord username from the cached JWT, if one is present and not expired.
pub fn username_from_cached_token() -> Option<String> {
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
    json_str(&payload_str, "username")
}

/// Extract the Discord user ID (JWT `sub` claim) from the cached token.
pub fn discord_id_from_cached_token() -> Option<String> {
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
    json_str(&payload_str, "sub")
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

// ── Discord OAuth ─────────────────────────────────────────────────────────────

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
      .then(() => { document.body.innerHTML = '<h2>✅ Logged in! You can close this tab.</h2>'; });
  } else {
    document.body.innerHTML = '<h2>❌ Login failed — no token in URL.</h2>';
  }
</script></body></html>"#;

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(), html
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
        return Err("Empty token — Discord login may have been cancelled".to_string());
    }
    println!("[mm] OAuth token received");
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

fn http_get(url: &str, token: &str) -> Result<String, String> {
    let (host, path) = parse_url(url)?;
    let stream = tls_connect(&host)?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    send_recv_https(stream, &req)
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

fn send_recv_https(mut stream: Box<dyn ReadWrite>, req: &str) -> Result<String, String> {
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);

    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| e.to_string())?;
    let is_auth_error = status_line.contains("401") || status_line.contains("403");

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

    if is_auth_error {
        return Err(format!("401 Unauthorized: {response}"));
    }
    Ok(response)
}

trait ReadWrite: std::io::Read + std::io::Write + Send {}
impl<S: std::io::Read + std::io::Write + Send> ReadWrite for native_tls::TlsStream<S> {}

fn tls_connect(host: &str) -> Result<Box<dyn ReadWrite>, String> {
    use std::net::TcpStream;
    let addr = format!("{host}:443");
    let tcp = TcpStream::connect(&addr).map_err(|e| format!("TCP connect to {addr}: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(15))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(10))).ok();
    let connector = native_tls::TlsConnector::new().map_err(|e| format!("TLS connector: {e}"))?;
    let tls = connector
        .connect(host, tcp)
        .map_err(|e| format!("TLS handshake with {host}: {e}"))?;
    Ok(Box::new(tls))
}

// ── LFG + status polling ──────────────────────────────────────────────────────

struct MatchInfo {
    #[allow(dead_code)]
    session_id: String,
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
                send(
                    tx,
                    Update::Status("Reconnecting to matchmaking...".into()),
                )?;
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
                let punch_at_ms = json_nested_i64(&resp, "match_info", "punch_at_ms")
                    .ok_or("missing punch_at_ms")?;
                let role = json_nested_str(&resp, "match_info", "role").ok_or("missing role")?;
                let peer_username = json_nested_str(&resp, "match_info", "username");
                let turn = parse_turn_creds(&resp);
                return Ok(MatchInfo {
                    session_id: session_id.to_string(),
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
    let arr_end = json[after_colon..].rfind(']')? + after_colon;
    Some(&json[after_colon..arr_end])
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

    // Skip headers, capturing Content-Length if present. Stop at the blank
    // line. We don't support chunked encoding here — the stats service
    // returns a finite Content-Length for binary payloads.
    let mut content_length: Option<usize> = None;
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
        }
    }

    let mut bytes = Vec::new();
    if let Some(len) = content_length {
        bytes.resize(len, 0);
        reader.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    } else {
        reader.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    }
    Ok(bytes)
}

pub fn post_match_result(
    token: &str,
    session_id: &str,
    p1_score: u16,
    p2_score: u16,
) -> Result<(), String> {
    let body =
        format!(r#"{{"session_id":"{session_id}","p1_score":{p1_score},"p2_score":{p2_score}}}"#);
    let url = format!("{}/match/result", signaling_url()?);
    http_post_json(&url, token, &body)?;
    Ok(())
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
