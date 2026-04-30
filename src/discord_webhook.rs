//! Minimal Discord-webhook poster for live match updates.
//!
//! Posts run on a fire-and-forget background thread so a slow webhook can never
//! stall the netplay loop. Failures are logged and swallowed.
//!
//! Webhook URL format: https://discord.com/api/webhooks/<id>/<token>
//! Body: {"content":"<message>","username":"Freeplay"}

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Fire-and-forget. Spawns a thread, returns immediately.
pub fn post(webhook_url: &str, content: &str) {
    if webhook_url.is_empty() {
        return;
    }
    let url = webhook_url.to_string();
    let msg = content.to_string();
    std::thread::spawn(move || {
        if let Err(e) = post_blocking(&url, &msg) {
            println!("[discord] webhook post failed: {e}");
        }
    });
}

fn post_blocking(url: &str, content: &str) -> Result<(), String> {
    let (host, path) = parse_url(url)?;
    let body = json_payload(content);

    let addr = format!("{host}:443");
    let tcp = TcpStream::connect(&addr).map_err(|e| format!("TCP connect: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(10))).ok();
    let connector = native_tls::TlsConnector::new().map_err(|e| format!("TLS init: {e}"))?;
    let mut tls = connector
        .connect(&host, tcp)
        .map_err(|e| format!("TLS handshake: {e}"))?;

    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nUser-Agent: Freeplay\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    tls.write_all(req.as_bytes())
        .map_err(|e| format!("write: {e}"))?;

    let mut reader = BufReader::new(&mut tls);
    let mut status = String::new();
    reader
        .read_line(&mut status)
        .map_err(|e| format!("read status: {e}"))?;
    // Drain headers + body so the connection can close cleanly.
    let _ = std::io::copy(&mut reader, &mut std::io::sink());

    let s = status.trim();
    if !(s.contains(" 200") || s.contains(" 204")) {
        return Err(format!("non-success status: {s}"));
    }
    Ok(())
}

fn parse_url(url: &str) -> Result<(String, String), String> {
    let url = url
        .strip_prefix("https://")
        .ok_or_else(|| format!("expected https URL, got {url}"))?;
    let slash = url.find('/').unwrap_or(url.len());
    let host = url[..slash].to_string();
    let path = if slash < url.len() {
        url[slash..].to_string()
    } else {
        "/".to_string()
    };
    Ok((host, path))
}

/// Build a `{"content":"..."}` JSON body, escaping the bare minimum required
/// for valid JSON. Discord rejects payloads >2000 chars, so we truncate.
fn json_payload(content: &str) -> String {
    let truncated: String = content.chars().take(1900).collect();
    let mut esc = String::with_capacity(truncated.len() + 32);
    for c in truncated.chars() {
        match c {
            '"' => esc.push_str("\\\""),
            '\\' => esc.push_str("\\\\"),
            '\n' => esc.push_str("\\n"),
            '\r' => esc.push_str("\\r"),
            '\t' => esc.push_str("\\t"),
            c if (c as u32) < 0x20 => { /* drop other control chars */ }
            c => esc.push(c),
        }
    }
    format!(r#"{{"content":"{esc}","username":"Freeplay"}}"#)
}
