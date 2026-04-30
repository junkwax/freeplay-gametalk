//! Ghost recording / playback.
//!
//! A "ghost" is a deterministic replay of a match: an initial savestate
//! plus the per-frame input stream. Because the core is deterministic
//! (see replay::RewindTest), loading the savestate and feeding the inputs
//! back in produces byte-identical game state — you can practice against
//! a recording of a previous match.
//!
//! File format (ghost.bin):
//!   magic       [u8; 4]  = b"NCGH"
//!   version     u16 LE   = 1
//!   frame_count u32 LE
//!   state_size  u32 LE
//!   state       [u8; state_size]
//!   inputs      [[u16; 2]; frame_count]   packed P1/P2 button masks
//!
//! Each frame's input is two u16s, one per port, bit N = button N pressed
//! (matching RETRO_DEVICE_ID_JOYPAD_* slot numbers).
#![allow(static_mut_refs)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::retro::{Core, INPUT_STATE};

/// Persistent counter of how many times we've recorded vs each peer.
/// Used to cap netplay auto-capture at N sessions per opponent.
pub struct Library {
    path: PathBuf,
    pub counts: BTreeMap<String, u32>,
}

const LIBRARY_PATH: &str = "ghost_library.json";

impl Library {
    pub fn load_default() -> Self {
        Self::load(LIBRARY_PATH)
    }

    fn load<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref().to_path_buf();
        let counts = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| parse_library(&s))
            .unwrap_or_default();
        Self { path, counts }
    }

    pub fn count_for(&self, peer_key: &str) -> u32 {
        self.counts.get(peer_key).copied().unwrap_or(0)
    }

    pub fn increment(&mut self, peer_key: &str) {
        *self.counts.entry(peer_key.to_string()).or_insert(0) += 1;
    }

    pub fn save(&self) -> std::io::Result<()> {
        let mut out = String::from("{\n  \"peer_counts\": {\n");
        let mut first = true;
        for (k, v) in &self.counts {
            if !first {
                out.push_str(",\n");
            }
            first = false;
            out.push_str(&format!("    \"{}\": {}", escape_json(k), v));
        }
        out.push_str("\n  }\n}\n");
        std::fs::write(&self.path, out)
    }
}

/// Minimal hand-rolled parser — we only need {"peer_counts":{"k":n,...}}.
fn parse_library(s: &str) -> Option<BTreeMap<String, u32>> {
    let idx = s.find("\"peer_counts\"")?;
    let rest = &s[idx..];
    let brace = rest.find('{')?;
    let rest = &rest[brace + 1..];
    let end = rest.find('}')?;
    let body = &rest[..end];
    let mut out = BTreeMap::new();
    for pair in body.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let colon = pair.find(':')?;
        let k = pair[..colon].trim().trim_matches('"').to_string();
        let v: u32 = pair[colon + 1..].trim().parse().ok()?;
        out.insert(k, v);
    }
    Some(out)
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Sanitize a peer address into a filename-safe key.
/// "192.168.1.5:7000" -> "192-168-1-5_7000"
pub fn peer_key(addr: &std::net::SocketAddr) -> String {
    addr.to_string().replace('.', "-").replace(':', "_")
}

/// Pick a random .ncgh file from `dir`. Returns an error if the directory
/// is missing or has no recordings. Rolls purely from system time — no
/// rand crate needed for this level of randomness.
pub fn pick_random_ghost<P: AsRef<Path>>(dir: P) -> std::io::Result<PathBuf> {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir.as_ref()) {
        Ok(it) => it
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "ncgh").unwrap_or(false))
            .collect(),
        Err(e) => return Err(e),
    };
    if entries.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no .ncgh files in ghosts/ directory",
        ));
    }
    // Sort so the index → file mapping is stable across filesystems (read_dir
    // order is platform-defined; NTFS is alphabetical, ext4 is hash-based).
    entries.sort();
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    Ok(entries[seed % entries.len()].clone())
}

const MAGIC: &[u8; 4] = b"NCGH";
const VERSION: u16 = 1;

/// Pack `[[bool; 16]; 2]` into two u16 bitmasks.
fn pack(state: &[[bool; 16]; 2]) -> [u16; 2] {
    let mut out = [0u16; 2];
    for port in 0..2 {
        let mut m = 0u16;
        for bit in 0..16 {
            if state[port][bit] {
                m |= 1 << bit;
            }
        }
        out[port] = m;
    }
    out
}

/// Unpack two u16 bitmasks back into `[[bool; 16]; 2]`.
fn unpack(bits: [u16; 2]) -> [[bool; 16]; 2] {
    let mut out = [[false; 16]; 2];
    for port in 0..2 {
        for bit in 0..16 {
            out[port][bit] = bits[port] & (1 << bit) != 0;
        }
    }
    out
}

/// Active recording session. Captures an initial savestate on creation and
/// accumulates per-frame inputs until the user stops and saves.
pub struct Recording {
    initial_state: Vec<u8>,
    inputs: Vec<[u16; 2]>,
}

impl Recording {
    /// Start recording. Captures the current savestate as the replay anchor.
    pub fn start(core: &Core) -> Option<Self> {
        let state = core.save_state()?;
        println!(
            "[ghost] Recording started ({} bytes anchor state)",
            state.len()
        );
        Some(Self {
            initial_state: state,
            inputs: Vec::new(),
        })
    }

    /// Call once per frame BEFORE core.run, after INPUT_STATE has been
    /// populated. Captures what the core is about to see.
    pub fn record_frame(&mut self) {
        let s = unsafe { INPUT_STATE };
        self.inputs.push(pack(&s));
    }

    pub fn frame_count(&self) -> usize {
        self.inputs.len()
    }

    /// Write the recording to disk in the NCGH format.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let mut f = std::fs::File::create(path.as_ref())?;
        f.write_all(MAGIC)?;
        f.write_all(&VERSION.to_le_bytes())?;
        f.write_all(&(self.inputs.len() as u32).to_le_bytes())?;
        f.write_all(&(self.initial_state.len() as u32).to_le_bytes())?;
        f.write_all(&self.initial_state)?;
        for bits in &self.inputs {
            f.write_all(&bits[0].to_le_bytes())?;
            f.write_all(&bits[1].to_le_bytes())?;
        }
        Ok(())
    }
}

/// Netplay auto-recording. Defers anchor-state capture to the first
/// confirmed frame (ggrs `is_last` in AdvanceFrame), because the
/// session takes a moment to stabilize after connection.
pub struct NetRecording {
    initial_state: Option<Vec<u8>>,
    inputs: Vec<[u16; 2]>,
    pub peer_key: String,
}

impl NetRecording {
    pub fn new(peer_key: String) -> Self {
        Self {
            initial_state: None,
            inputs: Vec::new(),
            peer_key,
        }
    }

    /// Capture the first confirmed-frame savestate; append per-frame inputs.
    /// Call with the ggrs `inputs[p].bits` values as `(p1_bits, p2_bits)`.
    pub fn record_confirmed_frame(&mut self, core: &Core, p1_bits: u16, p2_bits: u16) {
        if self.initial_state.is_none() {
            if let Some(blob) = core.save_state() {
                println!("[ghost/net] Anchor state captured ({} bytes)", blob.len());
                self.initial_state = Some(blob);
            } else {
                return; // try again next confirmed frame
            }
        }
        self.inputs.push([p1_bits, p2_bits]);
    }

    pub fn frame_count(&self) -> usize {
        self.inputs.len()
    }

    /// Write to disk as a normal `.ncgh` file. Returns the filename used.
    pub fn save<P: AsRef<Path>>(&self, dir: P) -> std::io::Result<PathBuf> {
        let Some(state) = &self.initial_state else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "no anchor state captured",
            ));
        };
        std::fs::create_dir_all(dir.as_ref())?;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let fname = format!("{}_{}.ncgh", self.peer_key, ts);
        let path = dir.as_ref().join(&fname);
        let mut f = std::fs::File::create(&path)?;
        f.write_all(MAGIC)?;
        f.write_all(&VERSION.to_le_bytes())?;
        f.write_all(&(self.inputs.len() as u32).to_le_bytes())?;
        f.write_all(&(state.len() as u32).to_le_bytes())?;
        f.write_all(state)?;
        for bits in &self.inputs {
            f.write_all(&bits[0].to_le_bytes())?;
            f.write_all(&bits[1].to_le_bytes())?;
        }
        Ok(path)
    }
}

/// Loaded ghost ready for playback.
pub struct Playback {
    state: Vec<u8>,
    inputs: Vec<[u16; 2]>,
    cursor: usize,
}

impl Playback {
    /// Load a ghost file. Returns an error if the file is malformed.
    pub fn load<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let mut f = std::fs::File::open(path.as_ref())?;

        let mut magic = [0u8; 4];
        f.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("bad magic: expected NCGH, got {:?}", magic),
            ));
        }
        let mut version = [0u8; 2];
        f.read_exact(&mut version)?;
        let v = u16::from_le_bytes(version);
        if v != VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported version: {}", v),
            ));
        }
        let mut frame_count_buf = [0u8; 4];
        f.read_exact(&mut frame_count_buf)?;
        let frame_count = u32::from_le_bytes(frame_count_buf) as usize;

        let mut state_size_buf = [0u8; 4];
        f.read_exact(&mut state_size_buf)?;
        let state_size = u32::from_le_bytes(state_size_buf) as usize;

        let mut state = vec![0u8; state_size];
        f.read_exact(&mut state)?;

        let mut inputs = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            let mut buf = [0u8; 4];
            f.read_exact(&mut buf)?;
            let p1 = u16::from_le_bytes([buf[0], buf[1]]);
            let p2 = u16::from_le_bytes([buf[2], buf[3]]);
            inputs.push([p1, p2]);
        }
        Ok(Self {
            state,
            inputs,
            cursor: 0,
        })
    }

    /// Restore the anchor savestate so the emulator is positioned at the
    /// exact frame where recording began.
    pub fn prime(&self, core: &Core) -> bool {
        let ok = core.load_state(&self.state);
        if ok {
            println!(
                "[ghost] Loaded anchor state ({} bytes), {} frames queued",
                self.state.len(),
                self.inputs.len()
            );
        }
        ok
    }

    /// Overwrite INPUT_STATE with the next recorded frame's inputs.
    /// Returns false when the recording is exhausted (caller should stop).
    ///
    /// `ports` controls which ports get overwritten:
    ///   0b01 -> P1 only, 0b10 -> P2 only, 0b11 -> both (full playback).
    /// Ports not in the mask keep whatever INPUT_STATE already holds,
    /// which is normally the live SDL input for that port.
    pub fn inject_next(&mut self, ports: u8) -> bool {
        if self.cursor >= self.inputs.len() {
            return false;
        }
        let s = unpack(self.inputs[self.cursor]);
        unsafe {
            if ports & 0b01 != 0 {
                INPUT_STATE[0] = s[0];
            }
            if ports & 0b10 != 0 {
                INPUT_STATE[1] = s[1];
            }
        }
        self.cursor += 1;
        true
    }

    #[allow(dead_code)]
    pub fn is_done(&self) -> bool {
        self.cursor >= self.inputs.len()
    }
    pub fn frame_count(&self) -> usize {
        self.inputs.len()
    }
    #[allow(dead_code)]
    pub fn inputs(&self) -> &[[u16; 2]] {
        &self.inputs
    }

    /// Reset the input cursor without touching the core's savestate. Used
    /// by Ghost Opponent mode to keep P2 behavior active across rounds:
    /// when the recording runs out we just replay its inputs from frame 0
    /// rather than re-priming (which would teleport both fighters).
    pub fn rewind_inputs(&mut self) {
        self.cursor = 0;
    }
}

// ── Stats service upload ──────────────────────────────────────────────────────

#[allow(dead_code)]
const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[allow(dead_code)]
fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(((data.len() + 2) / 3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(BASE64_TABLE[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            BASE64_TABLE[((triple >> 6) & 0x3F) as usize]
        } else {
            b'='
        } as char);
        out.push(if chunk.len() > 2 {
            BASE64_TABLE[(triple & 0x3F) as usize]
        } else {
            b'='
        } as char);
    }
    out
}

/// Minimal JSON string escape — handles the cases that actually appear in our
/// fields (`"`, `\`, control chars). Keeps us off serde_json without breaking
/// on Discord usernames containing quotes or backslashes.
#[allow(dead_code)]
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Upload a ghost file to the freeplay-stats service. Fire-and-forget:
/// spawns a thread, sends the POST, and logs the result.
///
/// `discord_id` should be the JWT `sub` claim (Discord snowflake). Empty when
/// the user hasn't logged in — server stores it as anonymous.
macro_rules! ulog {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        println!("{msg}");
        let _ = std::fs::OpenOptions::new()
            .create(true).append(true)
            .open("ghosts/upload.log")
            .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{msg}\n").as_bytes()));
    }};
}

const UPLOAD_QUEUE_PATH: &str = "ghosts/upload_queue";

/// Upload a saved NetRecording .ncgh to freeplay-stats. The file is gzip-
/// compressed and sent as a binary POST with metadata in HTTP headers.
/// On failure the path is queued for retry next time an upload is attempted.
pub fn upload_ghost_to_stats(
    stats_url: &str,
    ghost_path: &std::path::Path,
    discord_id: &str,
    username: &str,
    rom_hash: &str,
    frame_count: u32,
) {
    if stats_url.is_empty() {
        return;
    }
    let path_buf = ghost_path.to_path_buf();
    let filename = ghost_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.ncgh")
        .to_string();
    let ghost_id = format!(
        "{:016x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let url = format!("{stats_url}/ghosts/upload");
    let did = discord_id.to_string();
    let uname = username.to_string();
    let rh = rom_hash.to_string();

    std::thread::spawn(move || {
        if !try_upload_one(
            &url,
            &path_buf,
            &ghost_id,
            &did,
            &uname,
            &rh,
            &filename,
            frame_count,
        ) {
            enqueue_upload(&path_buf);
        }
    });
}

/// Attempt a single compressed binary upload. Returns true on success.
fn try_upload_one(
    url: &str,
    ghost_path: &std::path::Path,
    ghost_id: &str,
    discord_id: &str,
    username: &str,
    rom_hash: &str,
    filename: &str,
    frame_count: u32,
) -> bool {
    let raw = match std::fs::read(ghost_path) {
        Ok(d) => d,
        Err(e) => {
            ulog!("[ghost] upload read {}: {e}", ghost_path.display());
            return false;
        }
    };
    let compressed = match gzip_compress(&raw) {
        Ok(d) => d,
        Err(e) => {
            ulog!("[ghost] gzip: {e}");
            return false;
        }
    };

    match http_post_binary(
        url,
        &compressed,
        ghost_id,
        discord_id,
        username,
        rom_hash,
        filename,
        frame_count,
    ) {
        Ok(resp) => {
            ulog!(
                "[ghost] Uploaded {} ({} -> {} bytes): {}",
                ghost_path.display(),
                raw.len(),
                compressed.len(),
                resp.trim()
            );
            true
        }
        Err(e) => {
            ulog!("[ghost] Upload {} failed: {e}", ghost_path.display());
            false
        }
    }
}

/// Drain the upload queue. Called at startup and after each match.
pub fn drain_upload_queue(stats_url: &str) {
    if stats_url.is_empty() {
        return;
    }
    let url = format!("{stats_url}/ghosts/upload");
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
        let p = std::path::Path::new(path_str);
        let filename = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown.ncgh");
        let ghost_id = format!(
            "{:016x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        ulog!("[ghost] Retrying upload: {path_str}");
        if try_upload_one(&url, p, &ghost_id, "", "", "", filename, 0) {
            // Success — removed from queue
        } else {
            pending.push(path_str.to_string());
        }
    }
    if pending.is_empty() {
        let _ = std::fs::remove_file(UPLOAD_QUEUE_PATH);
    } else {
        let _ = std::fs::write(UPLOAD_QUEUE_PATH, pending.join("\n"));
    }
}

fn enqueue_upload(path: &std::path::Path) {
    let _ = std::fs::create_dir_all("ghosts");
    let entry = path.display().to_string();
    let mut existing = std::fs::read_to_string(UPLOAD_QUEUE_PATH).unwrap_or_default();
    if !existing.lines().any(|l| l.trim() == entry) {
        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(&entry);
        existing.push('\n');
        let _ = std::fs::write(UPLOAD_QUEUE_PATH, existing);
        ulog!("[ghost] Queued for retry: {}", entry);
    }
}

/// Read just the frame_count from an .ncgh header (skipping the savestate).
fn read_ncgh_frame_count(path: &std::path::Path) -> Option<u32> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).ok()?;
    if &magic != MAGIC {
        return None;
    }
    let mut _ver = [0u8; 2];
    f.read_exact(&mut _ver).ok()?;
    let mut fc = [0u8; 4];
    f.read_exact(&mut fc).ok()?;
    Some(u32::from_le_bytes(fc))
}

/// Scan the `ghosts/` directory and enqueue any .ncgh files not already
/// in the upload queue. Call once at startup (after Discord login is
/// available) to backfill old recordings.
pub fn queue_all_local_ghosts(
    discord_id: Option<&str>,
    username: Option<&str>,
    rom_hash: &str,
    stats_url: &str,
) {
    if stats_url.is_empty() {
        return;
    }
    let dir = match std::fs::read_dir("ghosts") {
        Ok(d) => d,
        Err(_) => return,
    };
    let did = discord_id.unwrap_or("");
    let uname = username.unwrap_or("");

    for entry in dir.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().map(|x| x == "ncgh").unwrap_or(false) {
            let frame_count = read_ncgh_frame_count(&path).unwrap_or(0);
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown.ncgh");
            let ghost_id = format!(
                "{:016x}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d
                        .as_nanos()
                        .wrapping_add(path.display().to_string().len() as u128))
                    .unwrap_or(0)
            );
            let url = format!("{stats_url}/ghosts/upload");
            let did_str = did.to_string();
            let uname_str = uname.to_string();
            let rh = rom_hash.to_string();
            let fn_str = filename.to_string();
            let pb = path.clone();
            std::thread::spawn(move || {
                if !try_upload_one(
                    &url,
                    &pb,
                    &ghost_id,
                    &did_str,
                    &uname_str,
                    &rh,
                    &fn_str,
                    frame_count,
                ) {
                    enqueue_upload(&pb);
                }
            });
        }
    }
}

fn gzip_compress(data: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(data).map_err(|e| e.to_string())?;
    encoder.finish().map_err(|e| e.to_string())
}

/// Binary POST with metadata in headers. Sends raw bytes (already gzip'd
/// by caller) as `application/octet-stream`.
fn http_post_binary(
    url: &str,
    body: &[u8],
    ghost_id: &str,
    discord_id: &str,
    username: &str,
    rom_hash: &str,
    filename: &str,
    frame_count: u32,
) -> Result<String, String> {
    use std::io::{BufRead, BufReader, Write};
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

    let headers = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Encoding: gzip\r\n\
         Content-Length: {}\r\n\
         X-Freeplay-Ghost-Id: {ghost_id}\r\n\
         X-Freeplay-Discord-Id: {discord_id}\r\n\
         X-Freeplay-Username: {username}\r\n\
         X-Freeplay-Rom-Hash: {rom_hash}\r\n\
         X-Freeplay-Filename: {filename}\r\n\
         X-Freeplay-Frame-Count: {frame_count}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    // Write headers and body in two parts so large binaries don't blow the
    // format! string buffer.
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
    // skip headers
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
