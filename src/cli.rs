//! Command-line argument parsing for direct-IP netplay launches.
//! Used by the legacy `--player N --local PORT --peer IP:PORT` flow that
//! predates the matchmaking server. The menu-driven flow doesn't go through
//! here.

#[derive(Clone, Debug)]
pub enum NetMode {
    Local,
    P2P {
        player: usize,
        local_port: u16,
        peer: std::net::SocketAddr,
    },
}

pub fn parse_args() -> NetMode {
    let args: Vec<String> = std::env::args().collect();
    let mut player: Option<usize> = None;
    let mut local_port: Option<u16> = None;
    let mut peer: Option<std::net::SocketAddr> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--player" => {
                let n: usize = args
                    .get(i + 1)
                    .and_then(|s| s.parse().ok())
                    .expect("--player requires 1 or 2");
                assert!(n == 1 || n == 2, "--player must be 1 or 2");
                player = Some(n - 1);
                i += 2;
            }
            "--local" => {
                let p: u16 = args
                    .get(i + 1)
                    .and_then(|s| s.parse().ok())
                    .expect("--local requires a port, e.g. --local 7000");
                local_port = Some(p);
                i += 2;
            }
            "--peer" => {
                let p: std::net::SocketAddr = args
                    .get(i + 1)
                    .and_then(|s| s.parse().ok())
                    .expect("--peer requires IP:PORT, e.g. --peer 127.0.0.1:7001");
                peer = Some(p);
                i += 2;
            }
            other => {
                println!("Unknown argument: {other}");
                i += 1;
            }
        }
    }
    match (player, local_port, peer) {
        (Some(pl), Some(lp), Some(pr)) => NetMode::P2P {
            player: pl,
            local_port: lp,
            peer: pr,
        },
        (None, None, None) => NetMode::Local,
        _ => panic!("Netplay requires all three: --player <1|2> --local <PORT> --peer <IP:PORT>"),
    }
}
