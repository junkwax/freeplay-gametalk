//! GGRS NonBlockingSocket implementation that talks to freeplay-relay.
//!
//! Replaces `turn_socket::TurnSocket` and `turn_relay::TurnRelay`. The
//! relay's wire protocol is much simpler than TURN — see the
//! freeplay-relay crate for the spec. From this client's perspective:
//!
//!   - On connect: send REGISTER with `(role, expiry, room_id, hmac)`
//!     parsed out of the signaling server's MatchInfo.turn payload.
//!     Wait briefly for REGISTERED/PEER_READY (when partner is also up).
//!   - To send: prefix payload with `0x04` and send_to(relay_addr).
//!   - To recv: recv_from, verify first byte is `0x04`, strip it.
//!
//! No NAT traversal needed — both clients are sending to a public IP
//! (the relay's), which both can reach.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ggrs::{Message, NonBlockingSocket};

const TAG_REGISTER:   u8 = 0x01;
const TAG_REGISTERED: u8 = 0x02;
const TAG_ERROR:      u8 = 0x03;
const TAG_DATA:       u8 = 0x04;
const TAG_PEER_READY: u8 = 0x05;

#[derive(Debug)]
pub enum RelayError {
    BadUri(String),
    BadCredentialFormat(String),
    Bind(io::Error),
    Send(io::Error),
    Recv(io::Error),
    ServerError { code: u8, msg: String },
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadUri(s) => write!(f, "bad relay URI: {s}"),
            Self::BadCredentialFormat(s) => write!(f, "bad credential format: {s}"),
            Self::Bind(e) => write!(f, "UDP bind: {e}"),
            Self::Send(e) => write!(f, "send: {e}"),
            Self::Recv(e) => write!(f, "recv: {e}"),
            Self::ServerError { code, msg } => write!(f, "relay error {code}: {msg}"),
        }
    }
}

impl std::error::Error for RelayError {}

pub struct RelaySocket {
    sock: UdpSocket,
    relay_addr: SocketAddr,
    /// What we tell GGRS the peer is at. The relay's actual address —
    /// every send goes here, every receive comes from here.
    peer_label: SocketAddr,
    /// `registered` flips true when the relay acknowledges our REGISTER.
    /// Some mobile paths appear to drop the relay's tiny control replies, so
    /// setup can proceed without this after a bounded warmup.
    registered: Arc<Mutex<bool>>,
    /// `peer_ready` flips true when the relay confirms the partner is
    /// also registered. Sent packets BEFORE this flips would be dropped
    /// by the relay (no peer to forward to). We let GGRS retry — its
    /// Synchronizing handshake naturally re-sends.
    peer_ready: Arc<Mutex<bool>>,
}

impl RelaySocket {
    /// Connect to the relay using the credentials from MatchInfo.turn.
    /// `creds.uri` must start with "relay://", username must be
    /// `"<role>:<expiry>:<room_id>"`, password must be hex-encoded
    /// HMAC-SHA256 (32 bytes = 64 hex chars).
    pub fn connect(
        uri: &str,
        username: &str,
        password: &str,
        local_port: u16,
    ) -> Result<Self, RelayError> {
        // Parse "relay://host:port"
        let after_scheme = uri.strip_prefix("relay://")
            .ok_or_else(|| RelayError::BadUri(format!("missing relay:// prefix: {uri}")))?;
        let relay_addr: SocketAddr = after_scheme
            .parse()
            .map_err(|e| RelayError::BadUri(format!("{after_scheme}: {e}")))?;

        // Parse "role:expiry:room_id" (room_id is a UUID, contains hyphens but no colons)
        let parts: Vec<&str> = username.splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(RelayError::BadCredentialFormat(format!(
                "expected 3-part username, got {}: {username}", parts.len()
            )));
        }
        let role: u8 = parts[0]
            .parse()
            .map_err(|e| RelayError::BadCredentialFormat(format!("role parse: {e}")))?;
        let expiry: u64 = parts[1]
            .parse()
            .map_err(|e| RelayError::BadCredentialFormat(format!("expiry parse: {e}")))?;
        let room_id = parts[2];
        if room_id.len() != 36 {
            return Err(RelayError::BadCredentialFormat(format!(
                "room_id must be 36 bytes, got {}: {room_id}", room_id.len()
            )));
        }

        let hmac_bytes = hex_decode(password)
            .ok_or_else(|| RelayError::BadCredentialFormat("hmac not hex".into()))?;
        if hmac_bytes.len() != 32 {
            return Err(RelayError::BadCredentialFormat(format!(
                "hmac must be 32 bytes, got {}", hmac_bytes.len()
            )));
        }

        let bind = format!("0.0.0.0:{local_port}");
        let sock = UdpSocket::bind(&bind).map_err(RelayError::Bind)?;
        sock.set_read_timeout(Some(Duration::from_millis(500))).ok();

        // Build REGISTER: [01][role:1][expiry:8][room_id:36][hmac:32] = 78 bytes
        let mut pkt = Vec::with_capacity(78);
        pkt.push(TAG_REGISTER);
        pkt.push(role);
        pkt.extend_from_slice(&(expiry as i64).to_be_bytes());
        pkt.extend_from_slice(room_id.as_bytes());
        pkt.extend_from_slice(&hmac_bytes);
        debug_assert_eq!(pkt.len(), 78);

        // Send REGISTER for a bounded warmup. The relay may answer with
        // REGISTERED and PEER_READY, but at least one real mobile path has
        // shown repeated REGISTERs arriving at the relay while its one-byte
        // control replies never make it back to the client. DATA packets are
        // larger and flow through the same relay mapping, so don't fail the
        // whole match just because this optional control-plane ACK is missing.
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut next_send = Instant::now();
        let mut buf = [0u8; 2048];
        let mut registered = false;
        let mut peer_ready = false;
        while Instant::now() < deadline {
            if Instant::now() >= next_send {
                sock.send_to(&pkt, relay_addr).map_err(RelayError::Send)?;
                next_send = Instant::now() + Duration::from_millis(500);
            }
            match sock.recv_from(&mut buf) {
                Ok((n, _from)) if n >= 1 => {
                    match buf[0] {
                        TAG_REGISTERED => registered = true,
                        TAG_PEER_READY => peer_ready = true,
                        TAG_ERROR if n >= 3 => {
                            let code = buf[1];
                            let msg_len = buf[2] as usize;
                            let msg = if n >= 3 + msg_len {
                                String::from_utf8_lossy(&buf[3..3 + msg_len]).to_string()
                            } else {
                                String::new()
                            };
                            return Err(RelayError::ServerError { code, msg });
                        }
                        _ => {}
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {}
                Err(e) => return Err(RelayError::Recv(e)),
            }
            if registered && peer_ready {
                break;
            }
        }
        // Expired or bad credentials are still surfaced by TAG_ERROR above.
        // If no control reply arrived, proceed anyway and let GGRS exchange
        // real DATA through the relay; the caller logs the observed state.

        sock.set_nonblocking(true).ok();

        Ok(Self {
            sock,
            relay_addr,
            // Use the relay address itself as the GGRS peer label. Every
            // send goes to the relay; the relay forwards to the partner.
            // Receives come from the relay too. GGRS doesn't care about
            // routing details — it just calls send_to(addr) and gets
            // packets back from the same addr.
            peer_label: relay_addr,
            registered: Arc::new(Mutex::new(registered)),
            peer_ready: Arc::new(Mutex::new(peer_ready)),
        })
    }

    /// Address of the relay. GGRS uses this as the peer identifier.
    pub fn peer_label(&self) -> SocketAddr {
        self.peer_label
    }

    /// Whether the relay has signaled the partner is also registered.
    /// Useful for the caller to log diagnostic state; GGRS just retries
    /// regardless.
    pub fn is_peer_ready(&self) -> bool {
        self.peer_ready.lock().map(|g| *g).unwrap_or(false)
    }

    /// Whether the relay acknowledged our REGISTER during setup.
    pub fn is_registered(&self) -> bool {
        self.registered.lock().map(|g| *g).unwrap_or(false)
    }
}

impl NonBlockingSocket<SocketAddr> for RelaySocket {
    fn send_to(&mut self, msg: &Message, _addr: &SocketAddr) {
        let bytes = match bincode::serialize(msg) {
            Ok(b) => b,
            Err(e) => {
                println!("[relay] serialize: {e}");
                return;
            }
        };
        // [04][payload]
        let mut pkt = Vec::with_capacity(1 + bytes.len());
        pkt.push(TAG_DATA);
        pkt.extend_from_slice(&bytes);
        if let Err(e) = self.sock.send_to(&pkt, self.relay_addr) {
            // EAGAIN/WouldBlock here is harmless; GGRS retries on its own.
            if e.kind() != io::ErrorKind::WouldBlock {
                println!("[relay] send: {e}");
            }
        }
    }

    fn receive_all_messages(&mut self) -> Vec<(SocketAddr, Message)> {
        let mut out = Vec::new();
        let mut buf = [0u8; 2048];
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((n, _from)) if n >= 1 => {
                    match buf[0] {
                        TAG_DATA => {
                            let payload = &buf[1..n];
                            if let Ok(m) = bincode::deserialize::<Message>(payload) {
                                out.push((self.peer_label, m));
                            }
                        }
                        TAG_PEER_READY => {
                            if let Ok(mut g) = self.peer_ready.lock() {
                                *g = true;
                            }
                        }
                        TAG_ERROR if n >= 3 => {
                            let code = buf[1];
                            let msg_len = buf[2] as usize;
                            let msg = if n >= 3 + msg_len {
                                String::from_utf8_lossy(&buf[3..3 + msg_len]).to_string()
                            } else {
                                String::new()
                            };
                            println!("[relay] error {code}: {msg}");
                        }
                        _ => {}
                    }
                }
                Ok(_) => {}
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        out
    }
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for chunk in s.as_bytes().chunks(2) {
        let hi = nibble(chunk[0])?;
        let lo = nibble(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
