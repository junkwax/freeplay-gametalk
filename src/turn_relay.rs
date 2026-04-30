//! Client-side TURN relay — fallback when direct P2P hole punch fails.
//!
//! Uses ZERO external crates beyond stdlib. All crypto (MD5 + HMAC-SHA1
//! for STUN long-term auth) is hand-rolled below.

use std::fmt;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// STUN message types
const MSG_ALLOCATE_REQUEST: u16 = 0x0003;
const MSG_ALLOCATE_SUCCESS: u16 = 0x0103;
const MSG_ALLOCATE_ERROR: u16 = 0x0113;
const MSG_REFRESH_REQUEST: u16 = 0x0004;
const MSG_REFRESH_SUCCESS: u16 = 0x0104;
const MSG_CREATE_PERMISSION_REQ: u16 = 0x0008;
const MSG_CREATE_PERMISSION_OK: u16 = 0x0108;
const MSG_SEND_INDICATION: u16 = 0x0016;

// STUN attributes
const ATTR_USERNAME: u16 = 0x0006;
const ATTR_MESSAGE_INTEGRITY: u16 = 0x0008;
const ATTR_REALM: u16 = 0x0014;
const ATTR_NONCE: u16 = 0x0015;
const ATTR_XOR_PEER_ADDRESS: u16 = 0x0012;
const ATTR_DATA: u16 = 0x0013;
const ATTR_XOR_RELAYED_ADDRESS: u16 = 0x0016;
const ATTR_REQUESTED_TRANSPORT: u16 = 0x0019;
const ATTR_LIFETIME: u16 = 0x000d;

const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum TurnError {
    Unreachable(String),
    AllocationFailed(String),
    AuthFailed,
    Io(io::Error),
    BadResponse,
}

impl fmt::Display for TurnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TurnError::Unreachable(s) => write!(f, "TURN server unreachable: {s}"),
            TurnError::AllocationFailed(s) => write!(f, "TURN allocation failed: {s}"),
            TurnError::AuthFailed => write!(f, "TURN auth failed"),
            TurnError::Io(e) => write!(f, "TURN IO error: {e}"),
            TurnError::BadResponse => write!(f, "Malformed STUN response"),
        }
    }
}

impl std::error::Error for TurnError {}

impl From<io::Error> for TurnError {
    fn from(e: io::Error) -> Self {
        TurnError::Io(e)
    }
}

// ── TurnRelay struct ──────────────────────────────────────────────────────────

pub struct TurnRelay {
    socket: UdpSocket,
    turn_server: SocketAddr,
    pub relayed_addr: SocketAddr,
    peer_addr: SocketAddr,
    username: String,
    password: String,
    realm: String,
    nonce: Vec<u8>,
}

impl TurnRelay {
    pub fn connect(
        turn_uri: &str,
        username: &str,
        password: &str,
        peer_addr: SocketAddr,
        local_port: u16,
    ) -> Result<Self, TurnError> {
        let turn_server = parse_turn_uri(turn_uri)?;
        let socket = UdpSocket::bind(format!("0.0.0.0:{local_port}"))?;
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;

        // First Allocate — expect 401 with realm + nonce
        let txn1 = random_txn_id();
        let req1 = build_allocate_request(&txn1, None);
        socket.send_to(&req1, turn_server)?;

        let buf1 = recv_matching(&socket, &txn1, Duration::from_secs(5))?;
        let (realm, nonce) = parse_auth_challenge(&buf1)?;

        // Second Allocate — with credentials
        let auth = AuthCreds {
            username,
            realm: &realm,
            nonce: &nonce,
            password,
        };
        let txn2 = random_txn_id();
        let req2 = build_allocate_request(&txn2, Some(&auth));
        socket.send_to(&req2, turn_server)?;

        let buf2 = recv_matching(&socket, &txn2, Duration::from_secs(5))?;
        let relayed_addr = parse_allocation_response(&buf2)?;

        println!("[turn] allocation: relayed={relayed_addr}");

        let mut relay = Self {
            socket,
            turn_server,
            relayed_addr,
            peer_addr,
            username: username.to_string(),
            password: password.to_string(),
            realm,
            nonce,
        };
        relay.add_permission(peer_addr)?;
        Ok(relay)
    }

    pub fn send(&self, data: &[u8]) -> Result<(), TurnError> {
        let msg = build_send_indication(self.peer_addr, data);
        self.socket.send_to(&msg, self.turn_server)?;
        Ok(())
    }

    /// Install a TURN permission for `peer`. Required before the TURN server
    /// will forward Send Indications to that peer or deliver Data Indications
    /// from it. Can be called multiple times for different peers.
    pub fn add_permission(&mut self, peer: SocketAddr) -> Result<(), TurnError> {
        // First attempt with current nonce
        match self.try_add_permission(peer) {
            Ok(()) => Ok(()),
            Err(TurnError::AllocationFailed(ref s))
                if s.contains("438") || s.contains("0x0118") =>
            {
                // Stale nonce — re-read it from the error response and retry once
                println!("[turn] stale nonce, refreshing and retrying permission");
                self.ensure_have_nonce()?;
                self.try_add_permission(peer)
            }
            Err(e) => Err(e),
        }
    }

    /// Single-attempt CreatePermission. On 0x0118 (error response), read the
    /// fresh REALM and NONCE from the error body so the caller can retry.
    fn try_add_permission(&mut self, peer: SocketAddr) -> Result<(), TurnError> {
        let auth = AuthCreds {
            username: &self.username,
            realm: &self.realm,
            nonce: &self.nonce,
            password: &self.password,
        };
        let txn = random_txn_id();
        let req = build_create_permission(&txn, peer, &auth);

        let was_nonblocking = self.socket.set_nonblocking(false).is_ok();
        self.socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .ok();

        self.socket.send_to(&req, self.turn_server)?;
        let result = recv_matching(&self.socket, &txn, Duration::from_secs(3));

        if was_nonblocking {
            let _ = self.socket.set_nonblocking(true);
        }

        let buf = result?;
        let msg_type = u16::from_be_bytes([buf[0], buf[1]]);

        // DIAGNOSTIC — dump first 64 bytes hex
        let preview: String = buf
            .iter()
            .take(64.min(buf.len()))
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "[turn] CreatePermission response: msg_type=0x{:04x} len={} bytes:\n  {}",
            msg_type,
            buf.len(),
            preview
        );

        if msg_type == MSG_CREATE_PERMISSION_OK {
            return Ok(());
        }

        // 0x0118 = CreatePermission Error — extract attributes for diagnosis
        if msg_type == 0x0118 {
            match iter_attrs(&buf) {
                Ok(attrs) => {
                    for (t, v) in attrs {
                        match t {
                            0x0009 => {
                                // ERROR-CODE attribute: bytes 0-1 reserved, byte 2 = class, byte 3 = number
                                if v.len() >= 4 {
                                    let class = v[2];
                                    let number = v[3];
                                    let code = (class as u16) * 100 + number as u16;
                                    let reason = String::from_utf8_lossy(&v[4..]);
                                    println!("[turn] ERROR-CODE: {} ({})", code, reason);
                                }
                            }
                            ATTR_REALM => {
                                let r = String::from_utf8_lossy(v).into_owned();
                                println!("[turn] new REALM: {}", r);
                                self.realm = r;
                            }
                            ATTR_NONCE => {
                                println!("[turn] new NONCE: {} bytes", v.len());
                                self.nonce = v.to_vec();
                            }
                            _ => {
                                println!("[turn] error attr: type=0x{:04x} len={}", t, v.len());
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("[turn] failed to parse error response attrs: {:?}", e);
                }
            }
        }

        Err(TurnError::AllocationFailed(format!(
            "permission rejected (msg_type=0x{:04x})",
            msg_type
        )))
    }

    /// Check that self.nonce is populated (set by try_add_permission during a
    /// prior 438 Stale Nonce response). Returns an error if nonce is missing —
    /// this shouldn't happen in normal flow but guards against a refactor that
    /// skips the 438 retry path.
    fn ensure_have_nonce(&self) -> Result<(), TurnError> {
        // The nonce was already extracted into self.nonce by try_add_permission
        // when it hit the 438 Stale Nonce response. If somehow it wasn't, we
        // have no recovery path — return error.
        if self.nonce.is_empty() {
            return Err(TurnError::AllocationFailed(
                "no nonce available after refresh".into(),
            ));
        }
        Ok(())
    }

    /// Send a STUN Refresh request to extend the allocation lifetime.
    /// Called periodically by GGRS's socket wrapper before the 10-minute
    /// allocation expires. On success the timestamp is bumped by the caller.
    pub fn refresh_allocation(&mut self) -> Result<(), TurnError> {
        let auth = AuthCreds {
            username: &self.username,
            realm: &self.realm,
            nonce: &self.nonce,
            password: &self.password,
        };
        let txn = random_txn_id();
        let req = build_refresh_request(&txn, &auth);

        let was_nonblocking = self.socket.set_nonblocking(false).is_ok();
        self.socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .ok();

        self.socket.send_to(&req, self.turn_server)?;
        let result = recv_matching(&self.socket, &txn, Duration::from_secs(3));

        if was_nonblocking {
            let _ = self.socket.set_nonblocking(true);
        }

        let buf = result?;
        let msg_type = u16::from_be_bytes([buf[0], buf[1]]);

        if msg_type == MSG_REFRESH_SUCCESS {
            println!("[turn] allocation refreshed");
            return Ok(());
        }

        if msg_type == MSG_ALLOCATE_ERROR || msg_type == 0x0114 {
            if let Ok(attrs) = iter_attrs(&buf) {
                let mut new_nonce = None;
                let mut error_code: Option<u16> = None;
                for (t, v) in attrs {
                    match t {
                        0x0009 if v.len() >= 4 => {
                            error_code = Some(v[2] as u16 * 100 + v[3] as u16);
                            let reason = String::from_utf8_lossy(&v[4..]);
                            println!(
                                "[turn] refresh error-code: {} ({})",
                                error_code.unwrap_or(0),
                                reason
                            );
                        }
                        ATTR_NONCE => {
                            new_nonce = Some(v.to_vec());
                            println!("[turn] refresh: got fresh nonce ({} bytes)", v.len());
                        }
                        _ => {}
                    }
                }
                if let Some(n) = new_nonce {
                    self.nonce = n;
                }
                if error_code == Some(437) {
                    return Err(TurnError::AllocationFailed(
                        "allocation may have expired".into(),
                    ));
                }
            }
            return Err(TurnError::AllocationFailed(format!(
                "refresh rejected (msg_type=0x{:04x})",
                msg_type
            )));
        }

        Err(TurnError::BadResponse)
    }

    pub fn raw_socket(&self) -> &UdpSocket {
        &self.socket
    }
}

// ── recv_matching is OUTSIDE the impl block as a free function ──
fn recv_matching(
    socket: &UdpSocket,
    expected_txn: &[u8; 12],
    timeout: Duration,
) -> Result<Vec<u8>, TurnError> {
    let deadline = std::time::Instant::now() + timeout;
    let mut buf = [0u8; 2048];
    loop {
        if std::time::Instant::now() > deadline {
            return Err(TurnError::AllocationFailed("response timeout".into()));
        }
        match socket.recv_from(&mut buf) {
            Ok((n, _)) => {
                if n < 20 {
                    continue;
                }
                let resp_txn = &buf[8..20];
                if resp_txn == expected_txn {
                    return Ok(buf[..n].to_vec());
                }
                let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
                println!(
                    "[turn] discarding non-matching packet (msg_type=0x{:04x})",
                    msg_type
                );
            }
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                return Err(TurnError::AllocationFailed(
                    "response timeout (recv)".into(),
                ));
            }
            Err(e) => return Err(TurnError::Io(e)),
        }
    }
}

// ── Auth credential helper ────────────────────────────────────────────────────

struct AuthCreds<'a> {
    username: &'a str,
    realm: &'a str,
    nonce: &'a [u8],
    password: &'a str,
}

// ── STUN message building ─────────────────────────────────────────────────────

fn build_allocate_request(txn: &[u8; 12], auth: Option<&AuthCreds>) -> Vec<u8> {
    let mut body = Vec::new();
    append_attr(&mut body, ATTR_REQUESTED_TRANSPORT, &[17, 0, 0, 0]);
    append_attr(&mut body, ATTR_LIFETIME, &600u32.to_be_bytes());

    if let Some(a) = auth {
        append_attr(&mut body, ATTR_USERNAME, a.username.as_bytes());
        append_attr(&mut body, ATTR_REALM, a.realm.as_bytes());
        append_attr(&mut body, ATTR_NONCE, a.nonce);
    }

    let mut msg = build_header(MSG_ALLOCATE_REQUEST, txn, body.len() as u16);
    msg.extend_from_slice(&body);

    if let Some(a) = auth {
        append_message_integrity(&mut msg, a);
    }
    msg
}

fn build_refresh_request(txn: &[u8; 12], auth: &AuthCreds) -> Vec<u8> {
    let mut body = Vec::new();
    append_attr(&mut body, ATTR_LIFETIME, &600u32.to_be_bytes());
    append_attr(&mut body, ATTR_USERNAME, auth.username.as_bytes());
    append_attr(&mut body, ATTR_REALM, auth.realm.as_bytes());
    append_attr(&mut body, ATTR_NONCE, auth.nonce);

    let mut msg = build_header(MSG_REFRESH_REQUEST, txn, body.len() as u16);
    msg.extend_from_slice(&body);
    append_message_integrity(&mut msg, auth);
    msg
}

fn build_create_permission(txn: &[u8; 12], peer: SocketAddr, auth: &AuthCreds) -> Vec<u8> {
    let mut body = Vec::new();
    append_xor_peer_address(&mut body, peer);
    append_attr(&mut body, ATTR_USERNAME, auth.username.as_bytes());
    append_attr(&mut body, ATTR_REALM, auth.realm.as_bytes());
    append_attr(&mut body, ATTR_NONCE, auth.nonce);

    let mut msg = build_header(MSG_CREATE_PERMISSION_REQ, txn, body.len() as u16);
    msg.extend_from_slice(&body);
    append_message_integrity(&mut msg, auth);
    msg
}

fn build_send_indication(peer: SocketAddr, data: &[u8]) -> Vec<u8> {
    let txn = random_txn_id();
    let mut body = Vec::new();
    append_xor_peer_address(&mut body, peer);
    append_attr(&mut body, ATTR_DATA, data);
    let mut msg = build_header(MSG_SEND_INDICATION, &txn, body.len() as u16);
    msg.extend_from_slice(&body);
    msg
}

fn build_header(msg_type: u16, txn: &[u8; 12], body_len: u16) -> Vec<u8> {
    let mut h = Vec::with_capacity(20);
    h.extend_from_slice(&msg_type.to_be_bytes());
    h.extend_from_slice(&body_len.to_be_bytes());
    h.extend_from_slice(&MAGIC_COOKIE);
    h.extend_from_slice(txn);
    h
}

fn append_attr(out: &mut Vec<u8>, attr_type: u16, value: &[u8]) {
    out.extend_from_slice(&attr_type.to_be_bytes());
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn append_xor_peer_address(out: &mut Vec<u8>, addr: SocketAddr) {
    let mut v = Vec::with_capacity(20);
    v.push(0);
    v.push(0x01);
    let port_xor = addr.port() ^ 0x2112;
    v.extend_from_slice(&port_xor.to_be_bytes());
    match addr.ip() {
        std::net::IpAddr::V4(ip) => {
            let octets = ip.octets();
            for i in 0..4 {
                v.push(octets[i] ^ MAGIC_COOKIE[i]);
            }
        }
        std::net::IpAddr::V6(ip) => {
            let octets = ip.octets();
            for i in 0..16 {
                v.push(octets[i] ^ MAGIC_COOKIE[i % 4]);
            }
        }
    }
    append_attr(out, ATTR_XOR_PEER_ADDRESS, &v);
}

fn append_message_integrity(msg: &mut Vec<u8>, auth: &AuthCreds) {
    let mut hash_input = Vec::new();
    hash_input.extend_from_slice(auth.username.as_bytes());
    hash_input.push(b':');
    hash_input.extend_from_slice(auth.realm.as_bytes());
    hash_input.push(b':');
    hash_input.extend_from_slice(auth.password.as_bytes());
    let key = md5(&hash_input);

    let new_body_len = (msg.len() - 20 + 24) as u16;
    msg[2..4].copy_from_slice(&new_body_len.to_be_bytes());

    let hmac = hmac_sha1(&key, msg);

    msg.extend_from_slice(&ATTR_MESSAGE_INTEGRITY.to_be_bytes());
    msg.extend_from_slice(&20u16.to_be_bytes());
    msg.extend_from_slice(&hmac);
}

// ── STUN response parsing ─────────────────────────────────────────────────────

fn parse_auth_challenge(buf: &[u8]) -> Result<(String, Vec<u8>), TurnError> {
    if buf.len() < 20 {
        return Err(TurnError::BadResponse);
    }
    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    if msg_type != MSG_ALLOCATE_ERROR {
        return Err(TurnError::BadResponse);
    }

    let mut realm = String::new();
    let mut nonce = Vec::new();
    for (t, v) in iter_attrs(buf)? {
        match t {
            ATTR_REALM => realm = String::from_utf8_lossy(v).into_owned(),
            ATTR_NONCE => nonce = v.to_vec(),
            _ => {}
        }
    }
    if realm.is_empty() || nonce.is_empty() {
        return Err(TurnError::BadResponse);
    }
    Ok((realm, nonce))
}

fn parse_allocation_response(buf: &[u8]) -> Result<SocketAddr, TurnError> {
    if buf.len() < 20 {
        return Err(TurnError::BadResponse);
    }
    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    if msg_type == MSG_ALLOCATE_ERROR {
        return Err(TurnError::AuthFailed);
    }
    if msg_type != MSG_ALLOCATE_SUCCESS {
        return Err(TurnError::BadResponse);
    }

    for (t, v) in iter_attrs(buf)? {
        if t == ATTR_XOR_RELAYED_ADDRESS && v.len() >= 8 && v[1] == 0x01 {
            let port = u16::from_be_bytes([v[2], v[3]]) ^ 0x2112;
            let ip = std::net::Ipv4Addr::new(
                v[4] ^ MAGIC_COOKIE[0],
                v[5] ^ MAGIC_COOKIE[1],
                v[6] ^ MAGIC_COOKIE[2],
                v[7] ^ MAGIC_COOKIE[3],
            );
            return Ok(SocketAddr::new(ip.into(), port));
        }
    }
    Err(TurnError::BadResponse)
}

fn parse_data_indication(buf: &[u8]) -> Result<(SocketAddr, Vec<u8>), TurnError> {
    let mut peer: Option<SocketAddr> = None;
    let mut data: Option<Vec<u8>> = None;
    for (t, v) in iter_attrs(buf)? {
        if t == ATTR_XOR_PEER_ADDRESS && v.len() >= 8 && v[1] == 0x01 {
            let port = u16::from_be_bytes([v[2], v[3]]) ^ 0x2112;
            let ip = std::net::Ipv4Addr::new(
                v[4] ^ MAGIC_COOKIE[0],
                v[5] ^ MAGIC_COOKIE[1],
                v[6] ^ MAGIC_COOKIE[2],
                v[7] ^ MAGIC_COOKIE[3],
            );
            peer = Some(SocketAddr::new(ip.into(), port));
        } else if t == ATTR_DATA {
            data = Some(v.to_vec());
        }
    }
    match (peer, data) {
        (Some(p), Some(d)) => Ok((p, d)),
        _ => Err(TurnError::BadResponse),
    }
}

pub fn extract_data_payload(buf: &[u8]) -> Option<Vec<u8>> {
    parse_data_indication(buf)
        .ok()
        .map(|(_peer, payload)| payload)
}

fn iter_attrs(buf: &[u8]) -> Result<Vec<(u16, &[u8])>, TurnError> {
    if buf.len() < 20 {
        return Err(TurnError::BadResponse);
    }
    let body_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    if buf.len() < 20 + body_len {
        return Err(TurnError::BadResponse);
    }

    let mut out = Vec::new();
    let mut pos = 20;
    while pos + 4 <= 20 + body_len {
        let t = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let l = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]) as usize;
        pos += 4;
        if pos + l > 20 + body_len {
            break;
        }
        out.push((t, &buf[pos..pos + l]));
        pos += l;
        pos = (pos + 3) & !3;
    }
    Ok(out)
}

fn parse_turn_uri(uri: &str) -> Result<SocketAddr, TurnError> {
    let rest = uri
        .strip_prefix("turn:")
        .or_else(|| uri.strip_prefix("turns:"))
        .ok_or_else(|| TurnError::Unreachable(format!("bad URI: {uri}")))?;
    let host_port = rest.split('?').next().unwrap_or(rest);
    host_port
        .parse()
        .map_err(|e| TurnError::Unreachable(format!("bad URI {uri}: {e}")))
}

fn random_txn_id() -> [u8; 12] {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut b = [0u8; 12];
    for (i, byte) in b.iter_mut().enumerate() {
        *byte = ((t >> (i * 5)) ^ (t >> (i * 3 + 1))) as u8;
    }
    b
}

// ── Hand-rolled MD5 (RFC 1321) ────────────────────────────────────────────────

fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in padded.chunks(64) {
        let mut m = [0u32; 16];
        for (i, w) in chunk.chunks(4).enumerate() {
            m[i] = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(K[i])
                    .wrapping_add(m[g])
                    .rotate_left(S[i]),
            );
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

// ── Hand-rolled SHA-1 (RFC 3174) ──────────────────────────────────────────────

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let h = sha1(key);
        k[..20].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0u8; BLOCK];
    let mut opad = [0u8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] = k[i] ^ 0x36;
        opad[i] = k[i] ^ 0x5c;
    }
    let mut inner = Vec::with_capacity(BLOCK + msg.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(msg);
    let inner_hash = sha1(&inner);

    let mut outer = Vec::with_capacity(BLOCK + 20);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner_hash);
    sha1(&outer)
}
