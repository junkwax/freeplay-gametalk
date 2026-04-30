//! Connectivity diagnostics shown on the Host / Join screens.
//!
//! All probes are non-blocking or short-timeout. The goal is to give the
//! user enough signal to self-diagnose common issues before a real session
//! is attempted: wrong IP, firewall blocking UDP, port already in use.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

/// One diagnostic line, ready to render.
#[allow(dead_code)]
pub struct DiagLine {
    pub label: String,
    pub value: String,
    pub ok: bool,
}

/// Best-effort local LAN IPv4 address. Uses the "connect a UDP socket to an
/// external unreachable address and read local_addr()" trick — no packets
/// actually send, but the OS picks the outbound interface.
#[allow(dead_code)]
pub fn local_lan_ip() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let addr = sock.local_addr().ok()?;
    match addr.ip() {
        IpAddr::V4(v4) => Some(v4),
        _ => None,
    }
}

/// Try binding UDP to the given port on all interfaces. Returns None on
/// success (bind + drop cleanly); Some(msg) on failure.
#[allow(dead_code)]
pub fn test_bind(port: u16) -> Option<String> {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
    match UdpSocket::bind(addr) {
        Ok(_s) => None,
        Err(e) => Some(e.to_string()),
    }
}

/// Send a tiny UDP packet to 127.0.0.1 and try to receive it. Proves the
/// socket stack is alive before blaming the firewall for cross-machine issues.
#[allow(dead_code)]
pub fn loopback_echo() -> bool {
    let sender = match UdpSocket::bind("127.0.0.1:0") {
        Ok(s) => s,
        Err(_) => return false,
    };
    let receiver = match UdpSocket::bind("127.0.0.1:0") {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = receiver.set_read_timeout(Some(Duration::from_millis(500)));
    let Ok(dest) = receiver.local_addr() else {
        return false;
    };
    if sender.send_to(b"freeplay-probe", dest).is_err() {
        return false;
    }
    let mut buf = [0u8; 16];
    matches!(receiver.recv_from(&mut buf), Ok((n, _)) if n > 0)
}

/// Collect the standard diagnostic set. `intended_port` is the UDP port the
/// user will host/bind on; passed in so we can test it specifically.
#[allow(dead_code)]
pub fn run_diagnostics(intended_port: u16) -> Vec<DiagLine> {
    let mut out = Vec::new();

    match local_lan_ip() {
        Some(ip) => out.push(DiagLine {
            label: "Local IP".into(),
            value: ip.to_string(),
            ok: true,
        }),
        None => out.push(DiagLine {
            label: "Local IP".into(),
            value: "(not found — check network adapter)".into(),
            ok: false,
        }),
    }

    match test_bind(intended_port) {
        None => out.push(DiagLine {
            label: format!("Bind UDP :{intended_port}"),
            value: "OK".into(),
            ok: true,
        }),
        Some(e) => out.push(DiagLine {
            label: format!("Bind UDP :{intended_port}"),
            value: format!("FAIL — {e}"),
            ok: false,
        }),
    }

    if loopback_echo() {
        out.push(DiagLine {
            label: "Loopback UDP".into(),
            value: "OK".into(),
            ok: true,
        });
    } else {
        out.push(DiagLine {
            label: "Loopback UDP".into(),
            value: "FAIL (socket stack issue?)".into(),
            ok: false,
        });
    }

    out
}
