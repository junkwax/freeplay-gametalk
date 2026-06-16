//! Auto-incident reporter for failed netplay matches.
//!
//! Whenever a match ends abnormally (hole-punch failure, GGRS desync,
//! peer disconnect, score-mismatch rejection from the server, panic),
//! we POST a JSON blob to the signaling server's `/match/incident`
//! endpoint. The server stores it in a GCS bucket so we can investigate
//! offline without relying on players to ship logs.
//!
//! Fire-and-forget by design: this runs on the cleanup path, where the
//! user is already seeing an error screen. A failure to upload an
//! incident is logged to the console but doesn't surface anywhere
//! user-visible.

use crate::matchmaking;
use crate::version;

/// Cap each log payload at 256 KB. Matches the server-side MAX_LOG_BYTES.
/// We keep the most recent slice — tails of logs are where the failure
/// usually shows up.
const MAX_LOG_BYTES: usize = 256 * 1024;

/// Free-form incident category. Matches the server's `kind` field.
/// We use plain strings rather than an enum so the kinds catalog can
/// grow without server-side enum upgrades.
pub const KIND_HOLE_PUNCH_FAILED: &str = "hole_punch_failed";
pub const KIND_TURN_FALLBACK_FAILED: &str = "turn_fallback_failed";
pub const KIND_GGRS_DISCONNECTED: &str = "ggrs_disconnected";
pub const KIND_GGRS_NEVER_SYNCED: &str = "ggrs_never_synced";
pub const KIND_MATCH_ENDED_EARLY: &str = "match_ended_early";
pub const KIND_SCORE_REJECTED: &str = "score_rejected";
pub const KIND_PANIC: &str = "panic";

/// All the state we capture for one incident. Most fields are optional
/// because the early-failure cases don't have all the context yet
/// (e.g. hole-punch failure has no GGRS event log because GGRS never
/// started).
#[derive(Default, Debug, Clone)]
pub struct Incident {
    pub kind: &'static str,
    pub summary: String,
    pub session_id: Option<String>,
    pub room_id: Option<String>,
    pub peer_endpoint: Option<String>,
    pub role: Option<&'static str>,           // "host" or "join"
    pub transport_path: Option<&'static str>, // "direct" or "relay"
    pub relay_registered: Option<bool>,
    pub relay_peer_ready: Option<bool>,
    pub relay_data_received: Option<bool>,
    pub ggrs_state: Option<String>,
    pub rom_hash: Option<String>,
    pub p1_score: Option<u16>,
    pub p2_score: Option<u16>,
    pub frames_advanced: u32,
    pub net_log_path: Option<std::path::PathBuf>,
    pub ggrs_event_tail: Option<String>,
}

impl Incident {
    pub fn new(kind: &'static str, summary: impl Into<String>) -> Self {
        Self {
            kind,
            summary: summary.into(),
            ..Default::default()
        }
    }
}

/// Serialize and POST in a background thread. Pulls the cached JWT at
/// call time so a sign-out can't race the upload.
///
/// We run on a thread rather than blocking because the failure path is
/// already showing the user an error screen — we don't want to add a
/// signaling-server round-trip to the latency of "click ENTER to return
/// to menu". If the upload fails (or there's no JWT cached because the
/// failure happened pre-login), we log it to the console and move on.
pub fn submit(incident: Incident) {
    let token = match matchmaking::current_token() {
        Some(t) => t,
        None => {
            println!(
                "[incident] not signed in, skipping upload of {:?}",
                incident.kind
            );
            return;
        }
    };
    std::thread::spawn(move || {
        if let Err(e) = submit_blocking(&incident, &token) {
            println!("[incident] upload failed for kind={}: {e}", incident.kind);
        } else {
            println!("[incident] uploaded kind={}", incident.kind);
        }
    });
}

/// Synchronous upload for process panic hooks. Panics usually terminate the
/// process immediately after the hook returns, so the normal background
/// thread path may not get CPU time to finish.
pub fn submit_now(incident: &Incident) {
    let Some(token) = matchmaking::current_token() else {
        println!("[incident] not signed in, skipping panic upload");
        return;
    };
    if let Err(e) = submit_blocking(incident, &token) {
        println!("[incident] panic upload failed: {e}");
    } else {
        println!("[incident] uploaded panic");
    }
}

fn submit_blocking(incident: &Incident, token: &str) -> Result<(), String> {
    let base_url = match crate::config::env_value("FREEPLAY_SIGNALING_URL")
        .or_else(crate::config::signaling_url)
    {
        Some(u) => u.trim_end_matches('/').to_string(),
        None => return Err("FREEPLAY_SIGNALING_URL not configured".into()),
    };
    let url = format!("{base_url}/match/incident");
    let body = build_body(incident);
    http_post_json_with_auth(&url, &body, token)
}

fn build_body(i: &Incident) -> String {
    // Minimal JSON construction by hand — matches the rest of the
    // crate's avoidance of serde_json (the only consumer of serde_json
    // here would be this module). Strings are escaped for JSON safety.
    let mut s = String::with_capacity(2048);
    s.push('{');
    push_str_field(&mut s, "kind", i.kind, true);
    push_str_field(&mut s, "summary", &i.summary, false);
    push_opt_str(&mut s, "session_id", i.session_id.as_deref());
    push_opt_str(&mut s, "room_id", i.room_id.as_deref());
    push_opt_str(&mut s, "peer_endpoint", i.peer_endpoint.as_deref());
    push_opt_str(&mut s, "role", i.role);
    push_opt_str(&mut s, "transport_path", i.transport_path);
    push_opt_bool(&mut s, "relay_registered", i.relay_registered);
    push_opt_bool(&mut s, "relay_peer_ready", i.relay_peer_ready);
    push_opt_bool(&mut s, "relay_data_received", i.relay_data_received);
    push_opt_str(&mut s, "ggrs_state", i.ggrs_state.as_deref());
    push_str_field(&mut s, "app_version", version::VERSION, false);
    push_str_field(&mut s, "build_date", version::BUILD_DATE, false);
    push_str_field(&mut s, "git_hash", version::GIT_HASH, false);
    push_opt_str(&mut s, "rom_hash", i.rom_hash.as_deref());
    if let Some(p1) = i.p1_score {
        s.push_str(&format!(",\"p1_score\":{}", p1));
    }
    if let Some(p2) = i.p2_score {
        s.push_str(&format!(",\"p2_score\":{}", p2));
    }
    s.push_str(&format!(",\"frames_advanced\":{}", i.frames_advanced));

    let net_log_tail = i
        .net_log_path
        .as_deref()
        .and_then(read_log_tail)
        .unwrap_or_default();
    s.push_str(",\"net_log_tail\":\"");
    json_escape_into(&net_log_tail, &mut s);
    s.push('"');

    if let Some(gtail) = &i.ggrs_event_tail {
        s.push_str(",\"ggrs_event_tail\":\"");
        json_escape_into(gtail, &mut s);
        s.push('"');
    }
    s.push('}');
    s
}

/// Read up to MAX_LOG_BYTES from the end of `path`. If the file is
/// larger, the front is dropped — the most recent content is what
/// matters for incident triage.
fn read_log_tail(path: &std::path::Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let cap = MAX_LOG_BYTES as u64;
    let start = if len > cap { len - cap } else { 0 };
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = String::new();
    f.read_to_string(&mut buf).ok()?;
    Some(buf)
}

fn push_str_field(out: &mut String, key: &str, value: &str, first: bool) {
    if !first {
        out.push(',');
    }
    out.push('"');
    out.push_str(key);
    out.push_str("\":\"");
    json_escape_into(value, out);
    out.push('"');
}

fn push_opt_str(out: &mut String, key: &str, value: Option<&str>) {
    if let Some(v) = value {
        push_str_field(out, key, v, false);
    }
}

fn push_opt_bool(out: &mut String, key: &str, value: Option<bool>) {
    if let Some(v) = value {
        out.push_str(&format!(",\"{key}\":{v}"));
    }
}

/// Minimal JSON string escape. Backslash, double-quote, and control
/// characters get the standard \\uXXXX or short-form treatment. All
/// other bytes pass through — we accept that legitimately weird
/// utf-8 in log tails won't be re-escaped, but it WILL be valid JSON.
fn json_escape_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_body_includes_transport_and_relay_diagnostics() {
        let mut inc = Incident::new(KIND_GGRS_NEVER_SYNCED, "relay \"startup\"\nfailed");
        inc.session_id = Some("session-123".into());
        inc.room_id = Some("room-456".into());
        inc.role = Some("join");
        inc.transport_path = Some("relay");
        inc.relay_registered = Some(true);
        inc.relay_peer_ready = Some(false);
        inc.relay_data_received = Some(true);
        inc.ggrs_state = Some("Synchronizing".into());
        inc.frames_advanced = 42;

        let body = build_body(&inc);

        assert!(body.contains("\"kind\":\"ggrs_never_synced\""));
        assert!(body.contains("\"summary\":\"relay \\\"startup\\\"\\nfailed\""));
        assert!(body.contains("\"session_id\":\"session-123\""));
        assert!(body.contains("\"room_id\":\"room-456\""));
        assert!(body.contains("\"role\":\"join\""));
        assert!(body.contains("\"transport_path\":\"relay\""));
        assert!(body.contains("\"relay_registered\":true"));
        assert!(body.contains("\"relay_peer_ready\":false"));
        assert!(body.contains("\"relay_data_received\":true"));
        assert!(body.contains("\"ggrs_state\":\"Synchronizing\""));
        assert!(body.contains("\"frames_advanced\":42"));
    }

    #[test]
    fn build_body_omits_unset_optional_relay_fields() {
        let inc = Incident::new(KIND_MATCH_ENDED_EARLY, "direct failure");
        let body = build_body(&inc);

        assert!(!body.contains("transport_path"));
        assert!(!body.contains("relay_registered"));
        assert!(!body.contains("relay_peer_ready"));
        assert!(!body.contains("relay_data_received"));
        assert!(!body.contains("ggrs_state"));
        assert!(body.contains("\"net_log_tail\":\"\""));
    }
}

fn http_post_json_with_auth(url: &str, body: &str, token: &str) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Write};
    let parsed = url.strip_prefix("https://").ok_or("only HTTPS")?;
    let slash = parsed.find('/').unwrap_or(parsed.len());
    let host = &parsed[..slash];
    let path = &parsed[slash..];
    let addr = format!("{host}:443");
    let tcp = std::net::TcpStream::connect(&addr).map_err(|e| format!("TCP: {e}"))?;
    tcp.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .ok();
    tcp.set_write_timeout(Some(std::time::Duration::from_secs(10)))
        .ok();
    let connector = native_tls::TlsConnector::new().map_err(|e| format!("TLS: {e}"))?;
    let mut tls = connector
        .connect(host, tcp)
        .map_err(|e| format!("TLS: {e}"))?;

    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tls.write_all(req.as_bytes())
        .map_err(|e| format!("write headers: {e}"))?;
    tls.write_all(body.as_bytes())
        .map_err(|e| format!("write body: {e}"))?;
    let mut reader = BufReader::new(tls);
    let mut status = String::new();
    reader
        .read_line(&mut status)
        .map_err(|e| format!("read: {e}"))?;
    if !status.contains(" 200") {
        return Err(format!("HTTP {}", status.trim()));
    }
    Ok(())
}
