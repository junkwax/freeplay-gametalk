//! Command-line argument parsing for direct-IP netplay launches.
//! Used by the legacy `--player N --local PORT --peer IP:PORT` flow that
//! predates the matchmaking server. The menu-driven flow doesn't go through
//! here.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetMode {
    Local,
    P2P {
        player: usize,
        local_port: u16,
        peer: std::net::SocketAddr,
    },
}

pub fn doctor_requested() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    doctor_requested_from(&args)
}

pub fn doctor_report_path() -> Option<std::path::PathBuf> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    doctor_report_path_from(&args)
}

pub fn render_probe_requested() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    render_probe_requested_from(&args)
}

pub fn core_probe_requested() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    core_probe_requested_from(&args)
}

fn doctor_requested_from(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--doctor")
}

fn doctor_report_path_from(args: &[String]) -> Option<std::path::PathBuf> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if let Some(path) = arg.strip_prefix("--doctor-report=") {
            return Some(std::path::PathBuf::from(path));
        }
        if arg == "--doctor-report" {
            return Some(
                args.next()
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| std::path::PathBuf::from("doctor.txt")),
            );
        }
    }
    None
}

fn render_probe_requested_from(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--render-probe")
}

fn core_probe_requested_from(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--core-probe")
}

pub fn parse_args() -> NetMode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    parse_net_mode_from(&args)
}

fn parse_net_mode_from(args: &[String]) -> NetMode {
    let mut player: Option<usize> = None;
    let mut local_port: Option<u16> = None;
    let mut peer: Option<std::net::SocketAddr> = None;
    let mut i = 0;
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
            "--doctor" => {
                i += 1;
            }
            "--doctor-report" => {
                i += 2;
            }
            arg if arg.starts_with("--doctor-report=") => {
                i += 1;
            }
            "--render-probe" => {
                i += 1;
            }
            "--core-probe" => {
                i += 1;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn net_mode_defaults_to_local() {
        assert_eq!(parse_net_mode_from(&args(&[])), NetMode::Local);
    }

    #[test]
    fn net_mode_parses_direct_p2p_args() {
        let mode = parse_net_mode_from(&args(&[
            "--player",
            "2",
            "--local",
            "7001",
            "--peer",
            "127.0.0.1:7000",
        ]));

        assert_eq!(
            mode,
            NetMode::P2P {
                player: 1,
                local_port: 7001,
                peer: "127.0.0.1:7000".parse().unwrap(),
            }
        );
    }

    #[test]
    fn net_mode_ignores_doctor_and_render_probe_flags() {
        assert_eq!(
            parse_net_mode_from(&args(&[
                "--doctor",
                "--doctor-report",
                "doctor.txt",
                "--doctor-report=other.txt",
                "--render-probe",
                "--core-probe",
            ])),
            NetMode::Local
        );
    }

    #[test]
    fn doctor_report_path_supports_split_equals_and_default_forms() {
        assert_eq!(
            doctor_report_path_from(&args(&["--doctor-report", "doctor.txt"])),
            Some(PathBuf::from("doctor.txt"))
        );
        assert_eq!(
            doctor_report_path_from(&args(&["--doctor-report=out.txt"])),
            Some(PathBuf::from("out.txt"))
        );
        assert_eq!(
            doctor_report_path_from(&args(&["--doctor-report"])),
            Some(PathBuf::from("doctor.txt"))
        );
    }

    #[test]
    fn diagnostic_flag_helpers_detect_their_flags() {
        let a = args(&["--render-probe", "--core-probe", "--doctor"]);
        assert!(render_probe_requested_from(&a));
        assert!(core_probe_requested_from(&a));
        assert!(doctor_requested_from(&a));
        assert!(!doctor_requested_from(&args(&[
            "--doctor-report=doctor.txt"
        ])));
    }
}
