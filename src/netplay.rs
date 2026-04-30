//! ggrs rollback integration.
//!
//! Phase 1: two-player P2P sessions over UDP. Defines the ggrs `Config`
//! (input type, state type, address type) and provides helpers to build
//! host/join sessions.
//!
//! State type = `Vec<u8>` holding a `retro_serialize` blob. ~2.4 MB each;
//! ggrs keeps a ring of these (default ~8 frames) so ~20 MB RAM for the
//! rollback buffer — acceptable.
//!
//! Input type = `NetInput { p1: u16, p2: u16 }` but ggrs treats each
//! player's input independently, so per-player = `u16` wrapped in a Pod
//! struct (ggrs requires Pod).

use bytemuck::{Pod, Zeroable};
use ggrs::{
    Config, NonBlockingSocket, P2PSession, PlayerType, SessionBuilder, UdpNonBlockingSocket,
};
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

/// One player's compact input snapshot, sent over the wire each frame.
/// 2 bytes. Bit layout matches `input::snapshot_player` / `apply_snapshot`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct NetInput {
    pub bits: u16,
}

/// ggrs configuration: what types the session operates on.
pub struct GgrsConfig;
impl Config for GgrsConfig {
    type Input = NetInput;
    type State = Vec<u8>; // retro_serialize blob
    type Address = SocketAddr;
}

pub type Session = P2PSession<GgrsConfig>;

/// Build a 2-player session.
///   `local_port` — UDP port to bind locally (any free port works for the
///   client; the host needs a known port so the client can connect).
///   `local_handle` — which player index (0 or 1) we are.
///   `remote_addr` — where the other player lives.
pub fn start_session(
    local_port: u16,
    local_handle: usize,
    remote_addr: SocketAddr,
) -> Result<Session, Box<dyn std::error::Error>> {
    start_session_verbose(local_port, local_handle, remote_addr, |_| {})
}

/// Same as `start_session` but each step reports progress through `log_fn`.
/// Lets the caller stream step-by-step progress into the net log so failures
/// are diagnosable instead of silent. The closure receives human-readable
/// lines (no trailing newline).
pub fn start_session_verbose<F: FnMut(&str)>(
    local_port: u16,
    local_handle: usize,
    remote_addr: SocketAddr,
    mut log_fn: F,
) -> Result<Session, Box<dyn std::error::Error>> {
    assert!(local_handle < 2, "local_handle must be 0 or 1");
    let remote_handle = 1 - local_handle;

    log_fn(&format!(
        "[session] starting: local_handle={} local_port={} remote={}",
        local_handle, local_port, remote_addr
    ));

    // MK2 runs ~54.7 Hz. ggrs wants the logical tick rate so its timing
    // calculations (timesync, drift detection) are correct.
    // Tighter disconnect timers. Defaults are 500ms notify / 2000ms hard-kill.
    // We additionally tear the session down on the Disconnected event in
    // main.rs, so the ggrs-side timeout mostly serves as a backstop. 1500ms
    // feels snappy to users but still survives a normal LAN hiccup.
    let mut builder = SessionBuilder::<GgrsConfig>::new()
        .with_num_players(2)
        .with_fps(55)
        .map_err(|e| {
            log_fn(&format!("[session] with_fps err: {e}"));
            e
        })?
        .with_input_delay(3)
        .with_max_prediction_window(8)
        .map_err(|e| {
            log_fn(&format!("[session] with_max_prediction_window err: {e}"));
            e
        })?
        .with_disconnect_notify_delay(std::time::Duration::from_millis(400))
        .with_disconnect_timeout(std::time::Duration::from_millis(1500))
        .with_desync_detection_mode(ggrs::DesyncDetection::On { interval: 30 });
    log_fn("[session] builder configured (fps=55 delay=3 window=8 desync=on, drop=1.5s)");

    builder = builder
        .add_player(PlayerType::Local, local_handle)
        .map_err(|e| {
            log_fn(&format!("[session] add_local_player err: {e}"));
            e
        })?
        .add_player(PlayerType::Remote(remote_addr), remote_handle)
        .map_err(|e| {
            log_fn(&format!("[session] add_remote_player err: {e}"));
            e
        })?;
    log_fn("[session] players added");

    let socket = UdpNonBlockingSocket::bind_to_port(local_port)
        .map_err(|e| {
            log_fn(&format!(
                "[session] UDP bind on port {} FAILED: {}  (is another client running? another app using this port?)",
                local_port, e));
            e
        })?;
    log_fn(&format!("[session] UDP bound on port {}", local_port));

    let session = builder.start_p2p_session(socket).map_err(|e| {
        log_fn(&format!("[session] start_p2p_session err: {e}"));
        e
    })?;
    log_fn(&format!(
        "[session] ready: we are P{} @ :{}, remote P{} @ {}",
        local_handle + 1,
        local_port,
        remote_handle + 1,
        remote_addr
    ));
    Ok(session)
}

pub fn start_session_with_socket<S>(
    local_handle: usize,
    remote_addr: SocketAddr,
    socket: S,
    mut log_fn: impl FnMut(&str),
) -> Result<P2PSession<GgrsConfig>, ggrs::GgrsError>
where
    S: NonBlockingSocket<SocketAddr> + 'static,
{
    let remote_handle = 1 - local_handle;

    let mut builder = SessionBuilder::<GgrsConfig>::new()
        .with_num_players(2)
        .with_fps(55)
        .map_err(|e| {
            log_fn(&format!("[session] with_fps err: {e}"));
            e
        })?
        .with_input_delay(3)
        .with_max_prediction_window(8)
        .map_err(|e| {
            log_fn(&format!("[session] with_max_prediction_window err: {e}"));
            e
        })?
        .with_disconnect_notify_delay(std::time::Duration::from_millis(400))
        .with_disconnect_timeout(std::time::Duration::from_millis(1500))
        .with_desync_detection_mode(ggrs::DesyncDetection::On { interval: 30 });
    log_fn("[session] (turn) builder configured");

    builder = builder
        .add_player(PlayerType::Local, local_handle)
        .map_err(|e| {
            log_fn(&format!("[session] add_local_player err: {e}"));
            e
        })?
        .add_player(PlayerType::Remote(remote_addr), remote_handle)
        .map_err(|e| {
            log_fn(&format!("[session] add_remote_player err: {e}"));
            e
        })?;

    let session = builder.start_p2p_session(socket).map_err(|e| {
        log_fn(&format!("[session] start_p2p_session (turn) err: {e}"));
        e
    })?;
    log_fn(&format!(
        "[session] ready (TURN): we are P{} → P{} @ {}",
        local_handle + 1,
        remote_handle + 1,
        remote_addr
    ));
    Ok(session)
}

/// A listener that binds a UDP port and waits (non-blocking) for any packet.
/// Used by Host Match when we don't yet know the client's address. Poll
/// `try_recv_peer()` every frame until it returns Some(addr), then drop the
/// listener and start a normal ggrs session on that port.
#[allow(dead_code)]
pub struct PeerListener {
    socket: UdpSocket,
    pub port: u16,
    /// Our client version, sent in NCAK replies so the probing peer can
    /// sanity-check that both sides run the same build.
    pub self_version: String,
    /// Our mk2.zip hash for the same reason. 0 if we have no ROM.
    pub self_rom_hash: u64,
}

#[allow(dead_code)]
impl PeerListener {
    pub fn bind(port: u16, self_version: &str, self_rom_hash: u64) -> std::io::Result<Self> {
        let sock = UdpSocket::bind(("0.0.0.0", port))?;
        sock.set_nonblocking(true)?;
        // Tiny read timeout redundant with nonblocking, but harmless.
        let _ = sock.set_read_timeout(Some(Duration::from_millis(1)));
        Ok(Self {
            socket: sock,
            port,
            self_version: self_version.to_string(),
            self_rom_hash,
        })
    }

    /// Poll once. Returns Some(sender) if a packet arrived this call; None if
    /// the socket is idle. Discards the packet contents — we only want the
    /// sender address. ggrs on the client side will retry its handshake.
    pub fn try_recv_peer(&self) -> Option<SocketAddr> {
        let mut buf = [0u8; 1500];
        let mut reads = 0u32;
        loop {
            reads = reads.wrapping_add(1);
            if reads > 32 {
                return None;
            }
            match self.socket.recv_from(&mut buf) {
                Ok((n, addr)) => {
                    // If this looks like a Test Connection probe, reply with
                    // an ack and keep waiting for the real peer. Probes aren't
                    // from our intended ggrs partner.
                    if is_probe(&buf[..n]) {
                        let ack = make_ack(addr, &self.self_version, self.self_rom_hash);
                        let _ = self.socket.send_to(&ack, addr);
                        continue;
                    }
                    return Some(addr);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => return None,
                Err(e) => {
                    println!("[net] listener recv err: {e}");
                    return None;
                }
            }
        }
    }
}

// --- Test Connection probe protocol (v2) ---
//
// A probe is one UDP datagram carrying a payload that identifies this build.
// The host replies with its own identity + how it observed the sender. We use
// fixed-offset LE encoding and carry enough context to diagnose:
//   - L3 reachability        (any reply at all)
//   - L4 bidirectional flow  (our reply gets back through NAT)
//   - Protocol compatibility (same client version + same ROM hash)
//   - NAT symmetry           (host saw us on a different port than we bound)
//   - Round-trip latency + jitter (send N probes)
//
// Wire format:
//   NCPB (4 bytes)                magic
//   version u16 LE                probe protocol version (current: 2)
//   client_ver_len u8  + bytes    (UTF-8, max 31)
//   rom_hash u64 LE               sender's mk2.zip hash (0 if missing)
//
//   NCAK (4 bytes)                magic
//   version u16 LE                protocol version (current: 2)
//   family u8                     4 or 6
//   ip bytes                      4 or 16
//   port u16 LE                   observed sender port
//   host_ver_len u8 + bytes       host's client version (UTF-8, max 31)
//   host_rom_hash u64 LE          host's ROM hash
//
// Older v1 packets (4-byte magic only) are still accepted for forward compat.

const PROBE_MAGIC: &[u8; 4] = b"NCPB";
const ACK_MAGIC: &[u8; 4] = b"NCAK";
const PROBE_PROTO_VERSION: u16 = 2;

/// Context a host includes in its NCAK reply so the client can sanity-check
/// version / ROM compatibility without starting a real session.
#[derive(Clone, Debug, Default)]
pub struct HostIdentity {
    pub client_version: String,
    pub rom_hash: u64,
}

#[allow(dead_code)]
fn is_probe(buf: &[u8]) -> bool {
    buf.len() >= 4 && &buf[..4] == PROBE_MAGIC
}

fn encode_string(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(31);
    out.push(n as u8);
    out.extend_from_slice(&bytes[..n]);
}

#[allow(dead_code)]
fn make_ack(sender: SocketAddr, self_ver: &str, self_rom_hash: u64) -> Vec<u8> {
    let mut v = Vec::with_capacity(64);
    v.extend_from_slice(ACK_MAGIC);
    v.extend_from_slice(&PROBE_PROTO_VERSION.to_le_bytes());
    match sender.ip() {
        std::net::IpAddr::V4(ip) => {
            v.push(4);
            v.extend_from_slice(&ip.octets());
        }
        std::net::IpAddr::V6(ip) => {
            v.push(6);
            v.extend_from_slice(&ip.octets());
        }
    }
    v.extend_from_slice(&sender.port().to_le_bytes());
    encode_string(&mut v, self_ver);
    v.extend_from_slice(&self_rom_hash.to_le_bytes());
    v
}

/// Parse an NCAK. Returns observed-self addr + host identity, where identity
/// is empty for v1 packets that lacked those fields.
fn parse_ack(buf: &[u8]) -> Option<(SocketAddr, HostIdentity)> {
    if buf.len() < 5 || &buf[..4] != ACK_MAGIC {
        return None;
    }
    // v2 packets carry a protocol version after the magic. Peek the next 2
    // bytes; if they're 0x01 or 0x04/0x06 (family byte of v1), treat as v1.
    let (mut cursor, has_version) = if buf.len() >= 7 {
        let maybe_ver = u16::from_le_bytes([buf[4], buf[5]]);
        if (2..=99).contains(&maybe_ver) {
            (6, true)
        } else {
            (4, false)
        }
    } else {
        (4, false)
    };

    if buf.len() <= cursor {
        return None;
    }
    let family = buf[cursor];
    cursor += 1;
    let ip_addr: SocketAddr = match family {
        4 => {
            if buf.len() < cursor + 4 + 2 {
                return None;
            }
            let ip = std::net::Ipv4Addr::new(
                buf[cursor],
                buf[cursor + 1],
                buf[cursor + 2],
                buf[cursor + 3],
            );
            cursor += 4;
            let port = u16::from_le_bytes([buf[cursor], buf[cursor + 1]]);
            cursor += 2;
            SocketAddr::from((ip, port))
        }
        6 => {
            if buf.len() < cursor + 16 + 2 {
                return None;
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&buf[cursor..cursor + 16]);
            cursor += 16;
            let ip = std::net::Ipv6Addr::from(octets);
            let port = u16::from_le_bytes([buf[cursor], buf[cursor + 1]]);
            cursor += 2;
            SocketAddr::from((ip, port))
        }
        _ => return None,
    };

    let mut identity = HostIdentity::default();
    if has_version && cursor < buf.len() {
        let n = buf[cursor] as usize;
        cursor += 1;
        if buf.len() >= cursor + n {
            if let Ok(s) = std::str::from_utf8(&buf[cursor..cursor + n]) {
                identity.client_version = s.to_string();
            }
            cursor += n;
        }
        if buf.len() >= cursor + 8 {
            let mut h = [0u8; 8];
            h.copy_from_slice(&buf[cursor..cursor + 8]);
            identity.rom_hash = u64::from_le_bytes(h);
        }
    }
    Some((ip_addr, identity))
}

/// Per-layer outcome of an extended probe.
#[derive(Debug, Default)]
pub struct ProbeReport {
    /// Set if we couldn't even bind a local UDP socket (L2/L3 local).
    pub local_bind_error: Option<String>,
    /// Our ephemeral port after bind (0 if bind failed).
    pub local_port: u16,
    /// Set if the first send_to() immediately errored (no route, host down,
    /// unreachable network). The kernel only reports this on connected sockets
    /// or via ICMP — usually None even when reachability is broken.
    pub send_error: Option<String>,
    /// Count of datagrams sent.
    pub sent: u32,
    /// Count of valid NCAK replies received.
    pub received: u32,
    /// Round-trip times in ms, one per successful reply (for jitter calc).
    pub rtts_ms: Vec<u128>,
    /// What the host observed as our address, if we got a v2 reply.
    pub observed_self: Option<SocketAddr>,
    /// True when `observed_self.port() != local_port`. Indicates symmetric NAT.
    pub nat_rewrote_port: bool,
    /// Host's client version string, if the reply was v2.
    pub host_version: Option<String>,
    /// Host's ROM hash, if the reply was v2. 0 means the host had no ROM.
    pub host_rom_hash: u64,
    /// Total wall-clock duration the probe ran (ms).
    pub duration_ms: u128,
}

impl ProbeReport {
    #[allow(dead_code)]
    pub fn reachable(&self) -> bool {
        self.received > 0
    }

    pub fn rtt_min(&self) -> Option<u128> {
        self.rtts_ms.iter().copied().min()
    }
    pub fn rtt_max(&self) -> Option<u128> {
        self.rtts_ms.iter().copied().max()
    }
    pub fn rtt_avg(&self) -> Option<u128> {
        if self.rtts_ms.is_empty() {
            None
        } else {
            Some(self.rtts_ms.iter().sum::<u128>() / self.rtts_ms.len() as u128)
        }
    }
    pub fn loss_percent(&self) -> u32 {
        if self.sent == 0 {
            return 0;
        }
        (((self.sent - self.received) as f32 / self.sent as f32) * 100.0).round() as u32
    }
}

/// Run N probes toward `target` with per-layer reporting. Blocks for ~N seconds.
///
/// `self_version` and `self_rom_hash` are embedded in our probe so the host
/// could (in a future protocol rev) echo back its view of us. Currently the
/// host only includes its own identity in the ACK.
pub fn probe_connection(
    target: SocketAddr,
    probes: u32,
    self_version: &str,
    self_rom_hash: u64,
) -> ProbeReport {
    let mut report = ProbeReport::default();
    let start = std::time::Instant::now();

    let sock = match UdpSocket::bind(("0.0.0.0", 0)) {
        Ok(s) => s,
        Err(e) => {
            report.local_bind_error = Some(e.to_string());
            return report;
        }
    };
    if let Err(e) = sock.set_read_timeout(Some(Duration::from_millis(900))) {
        report.local_bind_error = Some(e.to_string());
        return report;
    }
    report.local_port = sock.local_addr().ok().map(|a| a.port()).unwrap_or(0);

    // Build the probe payload once; target-agnostic.
    let mut packet = Vec::with_capacity(48);
    packet.extend_from_slice(PROBE_MAGIC);
    packet.extend_from_slice(&PROBE_PROTO_VERSION.to_le_bytes());
    encode_string(&mut packet, self_version);
    packet.extend_from_slice(&self_rom_hash.to_le_bytes());

    let mut buf = [0u8; 256];
    for _ in 0..probes {
        let send_start = std::time::Instant::now();
        match sock.send_to(&packet, target) {
            Ok(_) => {
                report.sent += 1;
            }
            Err(e) => {
                report.send_error = Some(e.to_string());
                break;
            }
        }

        // Wait up to 900ms for a reply matching our target.
        let deadline = send_start + Duration::from_millis(900);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let _ = sock.set_read_timeout(Some(remaining));
            match sock.recv_from(&mut buf) {
                Ok((n, from)) if from == target => {
                    if let Some((observed, identity)) = parse_ack(&buf[..n]) {
                        report.received += 1;
                        report.rtts_ms.push(send_start.elapsed().as_millis());
                        report.observed_self = Some(observed);
                        report.nat_rewrote_port = observed.port() != report.local_port;
                        if !identity.client_version.is_empty() {
                            report.host_version = Some(identity.client_version);
                        }
                        if identity.rom_hash != 0 {
                            report.host_rom_hash = identity.rom_hash;
                        }
                        break;
                    }
                }
                Ok(_) => { /* stranger, keep waiting */ }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(e) => {
                    report.send_error = Some(e.to_string());
                    break;
                }
            }
        }
    }
    report.duration_ms = start.elapsed().as_millis();
    report
}
