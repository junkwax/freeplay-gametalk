//! Netplay session lifecycle helpers — ghost recording bookends, score-event
//! handling, log-file rotation, ROM fingerprinting, and spectator relay pushes.
//! None of these touch the per-frame inner loop (that lives in `netcore.rs`).

use crate::discord_webhook;
use crate::ghost;
use crate::matchmaking;
use crate::score;
use crate::version;

/// Fire-and-forget: push current match status to the signaling server for
/// spectator clients to poll. Called every ~3s during netplay.
pub fn push_spectator_frame(session_id: &str, p1_wins: u16, p2_wins: u16, frame: u32) {
    let sid = session_id.to_string();
    std::thread::spawn(move || {
        let Some(base_url) = signaling_url() else {
            println!("[spectate] FREEPLAY_SIGNALING_URL is not configured");
            return;
        };
        let url = format!("{base_url}/spectate/push/{sid}");
        let body = format!(
            r#"{{"savestate":"","inputs":"","frame":{frame},"score_p1":{p1_wins},"score_p2":{p2_wins}}}"#
        );
        let _ = http_post_fire(&url, &body);
    });
}

fn signaling_url() -> Option<String> {
    if let Some(v) = crate::config::env_value("FREEPLAY_SIGNALING_URL") {
        return Some(v.trim_end_matches('/').to_string());
    }
    crate::config::signaling_url()
}

fn http_post_fire(url: &str, body: &str) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Write};
    let parsed = url.strip_prefix("https://").ok_or("only HTTPS")?;
    let slash = parsed.find('/').unwrap_or(parsed.len());
    let host = &parsed[..slash];
    let path = &parsed[slash..];
    let addr = format!("{host}:443");
    let tcp = std::net::TcpStream::connect(&addr).map_err(|e| format!("TCP: {e}"))?;
    tcp.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    let connector = native_tls::TlsConnector::new().map_err(|e| format!("TLS: {e}"))?;
    let mut tls = connector
        .connect(host, tcp)
        .map_err(|e| format!("TLS: {e}"))?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    tls.write_all(req.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let mut reader = BufReader::new(tls);
    let mut status = String::new();
    reader
        .read_line(&mut status)
        .map_err(|e| format!("read: {e}"))?;
    if !status.contains("200") {
        return Err(format!("HTTP {}", status.trim()));
    }
    Ok(())
}

/// Hash the contents of the ROM zip (FNV-1a 64-bit) and return `(size, hash)`.
/// Returns `(0, 0)` if the ROM file isn't present — non-netplay launches
/// don't fail, they just log an empty fingerprint. Cheap because the file
/// is ~3 MB and only read once per session.
pub fn rom_fingerprint() -> (u64, u64) {
    let bytes = match crate::rom::read_rom_zip() {
        Some(b) => b,
        None => return (0, 0),
    };
    let mut h: u64 = 0xcbf29ce484222325;
    for chunk in bytes.chunks(8) {
        let mut w = [0u8; 8];
        w[..chunk.len()].copy_from_slice(chunk);
        h ^= u64::from_le_bytes(w);
        h = h.wrapping_mul(0x100000001b3);
    }
    (bytes.len() as u64, h)
}

/// Open `freeplay-net.log` in append mode and write a session-start banner.
/// Returns `None` if the file can't be created (e.g. read-only install dir).
pub fn open_net_log() -> Option<std::fs::File> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("freeplay-net.log")
        .ok()?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (rom_size, rom_hash) = rom_fingerprint();
    let _ = writeln!(
        f,
        "---- session start (unix={}) freeplay=v{} build={} rom=size={} hash=0x{:016x} ----",
        ts,
        version::VERSION,
        version::BUILD_DATE,
        rom_size,
        rom_hash
    );
    Some(f)
}

/// Decide whether to start a new ghost recording for this peer. Honours the
/// per-peer cap stored in the ghost library so popular opponents don't
/// monopolise disk space.
pub fn maybe_start_net_recording(
    lib: &ghost::Library,
    peer: std::net::SocketAddr,
    cap: u32,
) -> Option<ghost::NetRecording> {
    let key = ghost::peer_key(&peer);
    let count = lib.count_for(&key);
    if count >= cap {
        println!("[ghost/net] cap reached for {key} ({count}/{cap}), not recording");
        return None;
    }
    println!("[ghost/net] recording session vs {key} ({count}/{cap})");
    Some(ghost::NetRecording::new(key))
}

/// Save the in-progress ghost recording (if any), bump the per-peer counter,
/// and fire-and-forget upload to `freeplay-stats` when both a stats URL and
/// a Discord login are available. Anonymous sessions skip upload because
/// empty `discord_id`/`username` pollute the public library.
pub fn finalize_net_recording(
    rec_slot: &mut Option<ghost::NetRecording>,
    library: &mut ghost::Library,
    stats_url: &str,
    discord_user: Option<&str>,
    discord_id: Option<&str>,
    rom_hash: &str,
) {
    let Some(rec) = rec_slot.take() else {
        return;
    };
    let frame_count = rec.frame_count();
    if frame_count == 0 {
        println!("[ghost/net] Session produced 0 frames, nothing to save.");
        return;
    }
    let peer_key = rec.peer_key.clone();
    match rec.save("ghosts") {
        Ok(path) => {
            println!("[ghost/net] Saved {frame_count} to {}", path.display());
            library.increment(&peer_key);
            if let Err(e) = library.save() {
                println!("[ghost/net] Library save failed: {e}");
            }
            if !stats_url.is_empty() && discord_user.is_some() {
                ghost::upload_ghost_to_stats(
                    stats_url,
                    &path,
                    discord_id.unwrap_or(""),
                    discord_user.unwrap_or(""),
                    rom_hash,
                    frame_count as u32,
                );
            }
        }
        Err(e) => println!("[ghost/net] Save failed: {e}"),
    }
}

/// React to a score-tracker event: log it, post to Discord, print to console.
/// Also forwards `MatchOver` to the signaling server's `/match/result`
/// endpoint for Glicko rating updates (server dedups by room_id, so calling
/// from both peers is safe).
pub fn handle_score_event(
    ev: score::ScoreEvent,
    local_handle: usize,
    discord_user: Option<&str>,
    webhook_url: &str,
    net_log: &mut Option<std::fs::File>,
    session_id: Option<&str>,
) {
    use std::io::Write;
    let local_tag = match (discord_user, local_handle) {
        (Some(u), 0) => format!("P1 ({u})"),
        (Some(u), 1) => format!("P2 ({u})"),
        (None, 0) => "P1 (you)".into(),
        (None, 1) => "P2 (you)".into(),
        _ => "?".into(),
    };
    let me_won =
        |winner: u8| (local_handle == 0 && winner == 1) || (local_handle == 1 && winner == 2);

    let (log_line, webhook_msg) = match ev {
        score::ScoreEvent::RoundWon {
            winner,
            p1_wins,
            p2_wins,
        } => {
            let who = if winner == 1 { "P1" } else { "P2" };
            let mine = if me_won(winner) { " (you)" } else { " (peer)" };
            (
                format!("[score] Round won by {who}{mine}. Score: P1 {p1_wins} - {p2_wins} P2"),
                format!(
                    ":boxing_glove: {local_tag} — {who} wins a round. **{p1_wins} - {p2_wins}**"
                ),
            )
        }
        score::ScoreEvent::MatchOver {
            winner,
            p1_wins,
            p2_wins,
        } => {
            let who = if winner == 1 { "P1" } else { "P2" };
            let mine = if me_won(winner) { " (you)" } else { " (peer)" };

            if let (Some(sid), Some(token)) = (session_id, matchmaking::current_token()) {
                if let Err(e) = matchmaking::post_match_result(&token, sid, p1_wins, p2_wins) {
                    println!("[score] failed to post match result to server: {e}");
                } else {
                    println!("[score] posted match result to server");
                }
            }

            (
                format!("[score] Match over. Winner: {who}{mine}. Final: P1 {p1_wins} - {p2_wins} P2"),
                format!(":trophy: **{who} wins the match!** Final: **{p1_wins} - {p2_wins}**  ({local_tag})"),
            )
        }
        score::ScoreEvent::NewMatch => (
            "[score] New match starting (counters reset)".to_string(),
            String::new(), // don't spam Discord on every reset
        ),
    };

    println!("{log_line}");
    if let Some(f) = net_log.as_mut() {
        let _ = writeln!(f, "{log_line}");
    }
    if !webhook_msg.is_empty() {
        discord_webhook::post(webhook_url, &webhook_msg);
    }
}
