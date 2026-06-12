//! Upload completed full-match replay files to freeplay-stats.

use std::io::{Read, Write};
use std::path::Path;

use crate::match_replay;

const UPLOAD_QUEUE_PATH: &str = "replays/upload_queue";
const UPLOADED_MARKER_PATH: &str = "replays/uploaded";

macro_rules! rlog {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        println!("{msg}");
        let _ = std::fs::OpenOptions::new()
            .create(true).append(true)
            .open("replays/upload.log")
            .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{msg}\n").as_bytes()));
    }};
}

#[derive(Debug, Clone)]
struct ReplayUploadMeta {
    replay_id: String,
    filename: String,
    p1_name: String,
    p2_name: String,
    p1_score: Option<u16>,
    p2_score: Option<u16>,
    winner: String,
    frame_count: u32,
    duration: String,
    recorded_at: String,
    completed_games: u32,
    completed_set: bool,
    session_id: String,
}

pub fn upload_replay_to_stats(
    stats_url: &str,
    replay_path: &Path,
    _discord_id: &str,
    _username: &str,
    rom_hash: &str,
) {
    let stats_url = stats_url.trim_end_matches('/').to_string();
    if stats_url.is_empty() {
        return;
    }
    let path_buf = replay_path.to_path_buf();
    let rom_hash = rom_hash.to_string();
    let token = crate::matchmaking::current_token();
    std::thread::spawn(move || {
        let Some(token) = token else {
            rlog!("[replay] Not signed in; queued {}", path_buf.display());
            enqueue_upload(&path_buf);
            return;
        };
        if !try_upload_one(&stats_url, &path_buf, &rom_hash, Some(&token)) {
            enqueue_upload(&path_buf);
        }
    });
}

pub fn drain_upload_queue(stats_url: &str) {
    let stats_url = stats_url.trim_end_matches('/').to_string();
    if stats_url.is_empty() {
        return;
    }
    let token = match crate::matchmaking::current_token() {
        Some(t) => t,
        None => return,
    };
    let queue = match std::fs::read_to_string(UPLOAD_QUEUE_PATH) {
        Ok(q) => q,
        Err(_) => return,
    };
    let mut pending: Vec<String> = Vec::new();
    for line in queue.lines() {
        let path_str = line.trim();
        if path_str.is_empty() {
            continue;
        }
        let path = Path::new(path_str);
        if is_uploaded(path) {
            continue;
        }
        rlog!("[replay] Retrying upload: {path_str}");
        if !try_upload_one(&stats_url, path, "", Some(&token)) {
            pending.push(path_str.to_string());
        }
    }
    if pending.is_empty() {
        let _ = std::fs::remove_file(UPLOAD_QUEUE_PATH);
    } else {
        let _ = std::fs::write(UPLOAD_QUEUE_PATH, pending.join("\n"));
    }
}

fn try_upload_one(
    stats_url: &str,
    replay_path: &Path,
    rom_hash: &str,
    token: Option<&str>,
) -> bool {
    let meta = match load_upload_meta(replay_path) {
        Ok(meta) => meta,
        Err(e) => {
            rlog!("[replay] Metadata load {}: {e}", replay_path.display());
            return !replay_path.exists();
        }
    };
    let raw = match std::fs::read(replay_path) {
        Ok(d) => d,
        Err(e) => {
            rlog!("[replay] Read {}: {e}", replay_path.display());
            return !replay_path.exists();
        }
    };
    let compressed = match gzip_compress(&raw) {
        Ok(d) => d,
        Err(e) => {
            rlog!("[replay] gzip: {e}");
            return false;
        }
    };
    let url = format!("{stats_url}/replays/upload");
    match http_post_binary(&url, &compressed, &meta, rom_hash, token) {
        Ok(resp) => {
            rlog!(
                "[replay] Uploaded {} ({} -> {} bytes): {}",
                replay_path.display(),
                raw.len(),
                compressed.len(),
                resp.trim()
            );
            mark_uploaded(replay_path);
            true
        }
        Err(e) => {
            rlog!("[replay] Upload {} failed: {e}", replay_path.display());
            false
        }
    }
}

fn load_upload_meta(path: &Path) -> Result<ReplayUploadMeta, String> {
    let playback = match_replay::Playback::load(path).map_err(|e| e.to_string())?;
    let sidecar = std::fs::read_to_string(path.with_extension("ncrp.json")).unwrap_or_default();
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("replay.ncrp")
        .to_string();
    let frame_count = json_u64(&sidecar, "frames")
        .unwrap_or(playback.frame_count() as u64)
        .min(u32::MAX as u64) as u32;
    Ok(ReplayUploadMeta {
        replay_id: replay_id_for(path),
        filename,
        p1_name: json_str(&sidecar, "p1").unwrap_or_else(|| playback.p1_name().to_string()),
        p2_name: json_str(&sidecar, "p2").unwrap_or_else(|| playback.p2_name().to_string()),
        p1_score: json_u64(&sidecar, "p1_score").map(|v| v.min(u16::MAX as u64) as u16),
        p2_score: json_u64(&sidecar, "p2_score").map(|v| v.min(u16::MAX as u64) as u16),
        winner: json_str(&sidecar, "winner").unwrap_or_default(),
        frame_count,
        duration: json_str(&sidecar, "duration").unwrap_or_else(|| format_duration(frame_count)),
        recorded_at: json_str(&sidecar, "recorded_at")
            .or_else(|| json_u64(&sidecar, "recorded_unix").map(|v| v.to_string()))
            .unwrap_or_default(),
        completed_games: json_u64(&sidecar, "completed_matches")
            .or_else(|| json_u64(&sidecar, "completed_games"))
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32,
        completed_set: json_bool(&sidecar, "completed_set").unwrap_or(false),
        session_id: json_str(&sidecar, "session_id").unwrap_or_default(),
    })
}

fn replay_id_for(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let mut safe: String = stem
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    if safe.len() >= 8 {
        return safe;
    }
    safe = format!("{:016x}", stable_path_hash(path));
    safe
}

fn stable_path_hash(path: &Path) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in path.display().to_string().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if let Ok(meta) = std::fs::metadata(path) {
        for byte in meta.len().to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

fn enqueue_upload(path: &Path) {
    if is_uploaded(path) {
        return;
    }
    let _ = std::fs::create_dir_all("replays");
    let entry = path.display().to_string();
    let mut existing = std::fs::read_to_string(UPLOAD_QUEUE_PATH).unwrap_or_default();
    if !existing.lines().any(|l| l.trim() == entry) {
        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(&entry);
        existing.push('\n');
        let _ = std::fs::write(UPLOAD_QUEUE_PATH, existing);
        rlog!("[replay] Queued for retry: {}", entry);
    }
}

fn upload_key(path: &Path) -> String {
    let meta = std::fs::metadata(path).ok();
    let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let modified = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}|{}|{}", path.display(), len, modified)
}

fn is_uploaded(path: &Path) -> bool {
    let key = upload_key(path);
    std::fs::read_to_string(UPLOADED_MARKER_PATH)
        .map(|s| s.lines().any(|line| line.trim() == key))
        .unwrap_or(false)
}

fn mark_uploaded(path: &Path) {
    let _ = std::fs::create_dir_all("replays");
    let key = upload_key(path);
    let mut existing = std::fs::read_to_string(UPLOADED_MARKER_PATH).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == key) {
        return;
    }
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(&key);
    existing.push('\n');
    let _ = std::fs::write(UPLOADED_MARKER_PATH, existing);
}

fn gzip_compress(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(data).map_err(|e| e.to_string())?;
    encoder.finish().map_err(|e| e.to_string())
}

fn http_post_binary(
    url: &str,
    body: &[u8],
    meta: &ReplayUploadMeta,
    rom_hash: &str,
    token: Option<&str>,
) -> Result<String, String> {
    use std::io::{BufRead, BufReader};
    let parsed = match url.strip_prefix("https://") {
        Some(rest) => rest,
        None => return Err("only HTTPS supported".into()),
    };
    let slash = parsed.find('/').unwrap_or(parsed.len());
    let host = &parsed[..slash];
    let path = &parsed[slash..];

    let addr = format!("{host}:443");
    let tcp = std::net::TcpStream::connect(&addr).map_err(|e| format!("TCP: {e}"))?;
    tcp.set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .ok();
    tcp.set_write_timeout(Some(std::time::Duration::from_secs(30)))
        .ok();
    let connector = native_tls::TlsConnector::new().map_err(|e| format!("TLS: {e}"))?;
    let mut tls = connector
        .connect(host, tcp)
        .map_err(|e| format!("TLS: {e}"))?;

    let auth_line = match token {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    let headers = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         {auth_line}\
         Content-Type: application/octet-stream\r\n\
         Content-Encoding: gzip\r\n\
         Content-Length: {}\r\n\
         X-Freeplay-Replay-Id: {}\r\n\
         X-Freeplay-Rom-Hash: {}\r\n\
         X-Freeplay-Filename: {}\r\n\
         X-Freeplay-P1-Name: {}\r\n\
         X-Freeplay-P2-Name: {}\r\n\
         X-Freeplay-P1-Score: {}\r\n\
         X-Freeplay-P2-Score: {}\r\n\
         X-Freeplay-Winner: {}\r\n\
         X-Freeplay-Frame-Count: {}\r\n\
         X-Freeplay-Duration: {}\r\n\
         X-Freeplay-Recorded-At: {}\r\n\
         X-Freeplay-Completed-Games: {}\r\n\
         X-Freeplay-Completed-Set: {}\r\n\
         X-Freeplay-Session-Id: {}\r\n\
         Connection: close\r\n\r\n",
        body.len(),
        header_value(&meta.replay_id, 96),
        header_value(rom_hash, 64),
        header_value(&meta.filename, 96),
        header_value(&meta.p1_name, 48),
        header_value(&meta.p2_name, 48),
        meta.p1_score.map(|v| v.to_string()).unwrap_or_default(),
        meta.p2_score.map(|v| v.to_string()).unwrap_or_default(),
        header_value(&meta.winner, 48),
        meta.frame_count,
        header_value(&meta.duration, 32),
        header_value(&meta.recorded_at, 64),
        meta.completed_games,
        if meta.completed_set { "true" } else { "false" },
        header_value(&meta.session_id, 96),
    );
    tls.write_all(headers.as_bytes())
        .map_err(|e| format!("write headers: {e}"))?;
    tls.write_all(body)
        .map_err(|e| format!("write body: {e}"))?;

    let mut reader = BufReader::new(tls);
    let mut status = String::new();
    reader
        .read_line(&mut status)
        .map_err(|e| format!("read: {e}"))?;
    if !status.contains("200") && !status.contains("201") {
        return Err(format!("HTTP {}", status.trim()));
    }
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("read: {e}"))?;
        if line.trim().is_empty() {
            break;
        }
    }
    let mut resp = String::new();
    reader.read_to_string(&mut resp).ok();
    Ok(resp)
}

fn header_value(value: &str, max_len: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() && *c != '\r' && *c != '\n')
        .take(max_len)
        .collect()
}

fn format_duration(frames: u32) -> String {
    let secs = (frames as f32 / 55.0).max(0.0);
    if secs >= 60.0 {
        format!("{:.0}m {:02.0}s", (secs / 60.0).floor(), secs % 60.0)
    } else {
        format!("{secs:.1}s")
    }
}

fn json_str(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let start = json.find(&pat)? + pat.len();
    let end = json[start..].find('"')?;
    Some(json[start..start + end].to_string())
}

fn json_u64(json: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\":");
    let start = json.find(&pat)? + pat.len();
    let rest = json[start..].trim_start();
    let mut end = 0usize;
    for c in rest.chars() {
        if c.is_ascii_digit() {
            end += c.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        return None;
    }
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
