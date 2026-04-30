//! GGRS NonBlockingSocket implementation that talks through a TURN relay.

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ggrs::{Message, NonBlockingSocket};

use crate::turn_relay::{TurnError, TurnRelay};

const PUNCH_PROBE: &[u8] = b"MK2PUNCH";

pub struct TurnSocket {
    inner: Arc<Mutex<TurnRelay>>,
    peer_addr: Arc<Mutex<SocketAddr>>,
    last_refresh: Arc<Mutex<Instant>>,
}

impl TurnSocket {
    pub fn new(
        turn_uri: &str,
        username: &str,
        password: &str,
        peer_addr: SocketAddr,
        local_port: u16,
    ) -> Result<Self, TurnError> {
        let relay = TurnRelay::connect(turn_uri, username, password, peer_addr, local_port)?;
        relay.raw_socket().set_nonblocking(true).ok();
        Ok(Self {
            inner: Arc::new(Mutex::new(relay)),
            peer_addr: Arc::new(Mutex::new(peer_addr)),
            last_refresh: Arc::new(Mutex::new(Instant::now())),
        })
    }

    /// Address that the TURN server allocated for us.
    /// Pass this to the signaling server so the peer can discover it.
    pub fn relayed_addr(&self) -> SocketAddr {
        self.inner
            .lock()
            .map(|r| r.relayed_addr)
            .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap())
    }

    fn current_peer(&self) -> SocketAddr {
        self.peer_addr
            .lock()
            .map(|p| *p)
            .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap())
    }

    fn maybe_refresh(&self) {
        // try_lock — never block GGRS's poll path. If another caller is mid-refresh
        // we'll retry on the next packet.
        let last = match self.last_refresh.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if last.elapsed() < Duration::from_secs(8 * 60) {
            return;
        }
        drop(last);

        let mut relay = match self.inner.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        match relay.refresh_allocation() {
            Ok(()) => {
                if let Ok(mut last) = self.last_refresh.try_lock() {
                    *last = Instant::now();
                }
            }
            Err(e) => eprintln!("[turn] refresh failed: {e}"),
        }
    }
}

impl NonBlockingSocket<SocketAddr> for TurnSocket {
    fn send_to(&mut self, msg: &Message, _addr: &SocketAddr) {
        let bytes = match bincode::serialize(msg) {
            Ok(b) => b,
            Err(e) => {
                println!("[turn] serialize failed: {e}");
                return;
            }
        };

        if let Ok(relay) = self.inner.lock() {
            if let Err(e) = relay.send(&bytes) {
                println!("[turn] send failed: {e}");
            }
        }
        self.maybe_refresh();
    }

    fn receive_all_messages(&mut self) -> Vec<(SocketAddr, Message)> {
        let mut out = Vec::new();
        let relay = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return out,
        };
        let peer = self.current_peer();

        let mut buf = [0u8; 2048];
        loop {
            match relay.raw_socket().recv_from(&mut buf) {
                Ok((n, _from)) => {
                    if n < 20 {
                        continue;
                    }
                    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
                    if msg_type != 0x0017 {
                        continue;
                    } // not a Data Indication

                    let payload = match crate::turn_relay::extract_data_payload(&buf[..n]) {
                        Some(p) => p,
                        None => continue,
                    };
                    if payload.as_slice() == PUNCH_PROBE {
                        continue;
                    }

                    match bincode::deserialize::<Message>(&payload) {
                        Ok(m) => out.push((peer, m)),
                        Err(_) => {}
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }

        drop(relay);
        self.maybe_refresh();
        out
    }
}
