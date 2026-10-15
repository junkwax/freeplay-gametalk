//! `--net-probe`: two headless Freeplay processes play each other over
//! localhost UDP through the real netplay engine, and their confirmed frames
//! are compared - the two-machine sync test, on one machine.
//!
//! `--sync-probe` proves a savestate reloads to the same future inside one
//! process. This goes further: the same path an online match takes - clean
//! boot capture, `reset_for_netplay`, a GGRS P2P session, and
//! `netcore::step_netplay_frame` with its savestate pool, RTC-masked
//! checksums and online run-ahead - with both sides mashing different
//! scripted input so GGRS keeps mispredicting and rolling back. Optional
//! simulated latency, jitter and loss (`LagSocket`) push rollbacks deeper.
//!
//!   freeplay --net-probe [--frames 6000] [--latency 40] [--jitter 10]
//!                        [--loss 2] [--delay 1] [--runahead|--no-runahead]
//!                        [--sabotage TICK]   (negative control: must FAIL;
//!                         poke mid-fight - fight start rewrites health)
//!                        [--keep-going]      (don't stop at GGRS's first desync)
//!
//! spawns `--net-probe-peer 0|1` twice, waits, and compares the logs each peer
//! writes (`net_probe_p1.txt` / `net_probe_p2.txt`: one `frame hash` line per
//! confirmed frame, hash = FNV-64 of SYSTEM_RAM, the same RAM the GGRS
//! checksum covers). Prints IN SYNC / DESYNC and exits 1 on a failure, so it
//! can gate a release.
//!
//! Which frames are compared: after each tick the RAM is hashed and filed
//! under `current_frame`. A tick that rolled back and re-simulated `k`
//! frames also re-ran frames that were filed earlier with predicted input;
//! those entries are dropped, so what is left is only states that were never
//! revised. Of those, everything up to the session's confirmed frame is
//! compared.

use crate::{input, memory, netcore, netplay, retro};
use ggrs::{Message, NonBlockingSocket, UdpNonBlockingSocket};
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// MK2's frame period. Real-time pacing matters: GGRS's time sync and its
/// prediction run against the wall clock, as in a real match.
const FRAME: Duration = Duration::from_micros(18_281);
const PORTS: [u16; 2] = [47_101, 47_102];

#[derive(Clone, Debug)]
struct Opts {
    frames: i32,
    latency_ms: u64,
    jitter_ms: u64,
    loss_pct: u32,
    delay: u32,
    runahead: bool,
    /// Negative control: P2 pokes P1's health at this tick, outside GGRS.
    /// The probe must then FAIL - proof that it can see a desync.
    sabotage: Option<u64>,
}

fn arg_after(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn parse_opts(args: &[String]) -> Opts {
    let num = |name: &str, default: u64| {
        arg_after(args, name).and_then(|v| v.parse().ok()).unwrap_or(default)
    };
    Opts {
        frames: num("--frames", 6000) as i32,
        latency_ms: num("--latency", 0),
        jitter_ms: num("--jitter", 0),
        loss_pct: num("--loss", 0).min(50) as u32,
        delay: num("--delay", 1).min(8) as u32,
        // The app's own default (`Config::runahead_online`), so the probe
        // tests what ships; `--runahead` / `--no-runahead` override it.
        runahead: if args.iter().any(|a| a == "--no-runahead") {
            false
        } else {
            args.iter().any(|a| a == "--runahead")
                || crate::config::Config::default().runahead_online
        },
        sabotage: arg_after(args, "--sabotage").and_then(|v| v.parse().ok()),
    }
}

fn opts_args(o: &Opts) -> Vec<String> {
    let mut v = vec![
        "--frames".into(),
        o.frames.to_string(),
        "--latency".into(),
        o.latency_ms.to_string(),
        "--jitter".into(),
        o.jitter_ms.to_string(),
        "--loss".into(),
        o.loss_pct.to_string(),
        "--delay".into(),
        o.delay.to_string(),
    ];
    v.push(if o.runahead { "--runahead" } else { "--no-runahead" }.into());
    if std::env::args().any(|a| a == "--keep-going") {
        v.push("--keep-going".into());
    }
    if let Some(t) = o.sabotage {
        v.push("--sabotage".into());
        v.push(t.to_string());
    }
    v
}

pub fn pair_requested() -> bool {
    std::env::args().skip(1).any(|a| a == "--net-probe")
}

pub fn peer_requested() -> Option<usize> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    arg_after(&args, "--net-probe-peer").and_then(|v| v.parse().ok()).filter(|h| *h < 2)
}

fn log_path(handle: usize) -> String {
    format!("net_probe_p{}.txt", handle + 1)
}

/// Small xorshift: deterministic per seed, no dependency.
struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

/// UDP socket that holds each outgoing packet for latency ± jitter and drops
/// `loss_pct` of them, so a localhost session behaves like a real link.
struct LagSocket {
    inner: UdpNonBlockingSocket,
    queue: VecDeque<(Instant, SocketAddr, Message)>,
    latency: Duration,
    jitter_ms: u64,
    loss_pct: u32,
    rng: Rng,
}

impl LagSocket {
    fn flush(&mut self) {
        let now = Instant::now();
        // Not FIFO once jitter reorders due times: send whatever is due.
        let mut i = 0;
        while i < self.queue.len() {
            if self.queue[i].0 <= now {
                let (_, addr, msg) = self.queue.remove(i).expect("index in range");
                self.inner.send_to(&msg, &addr);
            } else {
                i += 1;
            }
        }
    }
}

impl NonBlockingSocket<SocketAddr> for LagSocket {
    fn send_to(&mut self, msg: &Message, addr: &SocketAddr) {
        if self.loss_pct > 0 && self.rng.next() % 100 < self.loss_pct {
            return;
        }
        let jitter = if self.jitter_ms > 0 {
            Duration::from_millis(u64::from(self.rng.next()) % (self.jitter_ms * 2 + 1))
        } else {
            Duration::ZERO
        };
        let base = self.latency.saturating_sub(Duration::from_millis(self.jitter_ms));
        if base.is_zero() && jitter.is_zero() {
            self.inner.send_to(msg, addr);
        } else {
            self.queue.push_back((Instant::now() + base + jitter, *addr, msg.clone()));
        }
        self.flush();
    }

    fn receive_all_messages(&mut self) -> Vec<(SocketAddr, Message)> {
        self.flush();
        self.inner.receive_all_messages()
    }
}

/// Wait until the other peer process is up (its GGRS socket bound), by
/// trading a one-byte hello on `PORTS + 100`. Keeps answering for a moment
/// after the first hello so the peer is sure to hear one too.
fn meet_peer(handle: usize) -> Result<(), Box<dyn std::error::Error>> {
    let sock = std::net::UdpSocket::bind(("127.0.0.1", PORTS[handle] + 100))?;
    sock.set_read_timeout(Some(Duration::from_millis(50)))?;
    let peer = ("127.0.0.1", PORTS[1 - handle] + 100);
    let start = Instant::now();
    let mut heard: Option<Instant> = None;
    let mut buf = [0u8; 8];
    while start.elapsed() < Duration::from_secs(60) {
        let _ = sock.send_to(b"H", peer);
        if sock.recv_from(&mut buf).is_ok() && heard.is_none() {
            heard = Some(Instant::now());
        }
        if heard.is_some_and(|t| t.elapsed() > Duration::from_millis(500)) {
            return Ok(());
        }
    }
    Err("peer process never answered".into())
}

/// Scripted local input: Start taps keep the game moving out of attract and
/// select, and every 6 ticks a fresh random set of held buttons and
/// directions. Seeded per player, so the two sides never press the same thing
/// and every remote input change is a misprediction GGRS must roll back.
fn script_input(player: input::Player, tick: u64, rng: &mut Rng, held: &mut u16) {
    let actions = input::actions();
    if tick % 6 == 0 {
        // About a third of the buttons down at once.
        let a = rng.next();
        let b = rng.next();
        *held = (a & b & 0xffff) as u16;
    }
    for (i, action) in actions.iter().enumerate() {
        let pressed = match action {
            input::Action::Start => tick % 150 < 4,
            input::Action::Coin => false,
            _ => tick > 300 && (*held >> i) & 1 == 1,
        };
        input::set_action(player, *action, pressed);
    }
}

fn ram_hash(core: &retro::Core) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    if let Some(ram) = memory::snapshot(core) {
        for b in ram {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// One side of the pair.
pub fn run_peer(handle: usize) -> Result<(), Box<dyn std::error::Error>> {
    crate::log::init(if handle == 0 { "net_probe_p1" } else { "net_probe_p2" });
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = parse_opts(&args);
    let tag = format!("[net-probe p{}]", handle + 1);
    let rom_path = crate::rom::find_rom_zip_string()
        .ok_or_else(|| "ROM zip not found next to the executable or in roms\\".to_string())?;
    let core_path = crate::render::fbneo_core_path()
        .ok_or_else(|| "FBNeo core not found next to the executable or in cores\\".to_string())?;
    retro::set_silent(true);
    let core = unsafe { retro::load(&core_path, &rom_path)? };

    // The app's own boot capture (render.rs) and match start (main.rs).
    for _ in 0..90 {
        unsafe { (core.run)() };
    }
    let boot = core.save_state().ok_or("could not capture clean boot state")?;
    netcore::set_clean_boot_state(boot.clone());
    let boot_hash = netcore::clean_boot_hash();
    if !core.load_state(&boot) {
        return Err("could not reload the clean boot state".into());
    }
    netcore::reset_for_netplay(
        &core,
        &mut memory::PokeList::new(),
        &mut crate::lab::ResetSlots::default(),
        &mut None,
        &mut None,
    );
    retro::clear_audio_buffer();

    let peer: SocketAddr = format!("127.0.0.1:{}", PORTS[1 - handle]).parse()?;
    // Bind the GGRS socket BEFORE meeting the peer, then meet on a side port.
    // GGRS 0.10 re-sends a sync request only after 200 ms with nothing sent
    // at all; if one side's first requests hit a port not yet bound, the
    // other side syncs off its replies, starts sending inputs, and every
    // reply to those resets the timer - the first side then waits in
    // Synchronizing forever. The app's hole punch has the same effect as
    // this hello: both sockets are live before GGRS sends anything.
    let inner = UdpNonBlockingSocket::bind_to_port(PORTS[handle])?;
    meet_peer(handle)?;
    let socket = LagSocket {
        inner,
        queue: VecDeque::new(),
        latency: Duration::from_millis(o.latency_ms),
        jitter_ms: o.jitter_ms,
        loss_pct: o.loss_pct,
        rng: Rng(0x2545_f491 ^ (handle as u32 + 1)),
    };
    let mut sess = netplay::start_session_with_socket(handle, peer, socket, o.delay, |l| {
        println!("{tag} {l}")
    })?;
    println!(
        "{tag} boot {boot_hash:016x}, delay {}f, window {}, latency {}±{} ms, loss {}%, runahead {}",
        o.delay,
        netplay::MAX_PREDICTION_WINDOW,
        o.latency_ms,
        o.jitter_ms,
        o.loss_pct,
        o.runahead
    );

    let player = if handle == 0 { input::Player::P1 } else { input::Player::P2 };
    let mut rng = Rng(0x9e37_79b9 ^ ((handle as u32 + 1) * 0x85eb_ca6b));
    let mut held = 0u16;
    let mut runtime = netcore::NetRuntime::default();
    // GGRS events (sync progress, interruptions, desyncs) land here.
    let mut net_log = std::fs::File::create(format!("net_probe_p{}.net.log", handle + 1)).ok();
    let mut hashes: BTreeMap<i32, u64> = BTreeMap::new();
    let (mut rollbacks, mut deepest, mut desyncs) = (0u64, 0usize, 0u64);
    let mut disconnected = false;
    // Lowest health each fighter reached: proof the script got into fights
    // and hits landed (MK2 only - other games have no HP address here).
    let mut hp_min = [u16::MAX; 2];
    let mut fight_frames = 0u64;
    let mk2 = crate::game::current().mk2_tooling;
    let started = Instant::now();
    let deadline = FRAME * (o.frames as u32 * 3) + Duration::from_secs(30);
    let mut next = Instant::now();
    let mut tick = 0u64;
    // `--keep-going`: don't stop at GGRS's first desync report, so the RAM
    // comparison shows whether the two sides really diverged after it.
    let keep_going = args.iter().any(|a| a == "--keep-going");

    loop {
        if handle == 1 && o.sabotage == Some(tick) && mk2 {
            memory::poke_u16(&core, crate::mk2_addrs::a().P1_HP_ADDR, 1, memory::Endian::Little);
            println!("{tag} SABOTAGE: poked P1 health at tick {tick} (expect FAIL)");
        }
        script_input(player, tick, &mut rng, &mut held);
        let stats = netcore::step_netplay_frame(
            &core,
            &mut sess,
            o.runahead,
            handle,
            &mut None,
            &mut None,
            &mut net_log,
            &mut runtime,
        );
        retro::clear_audio_buffer();
        tick += 1;

        if stats.desync_detected {
            desyncs += 1;
        }
        if stats.peer_disconnected {
            disconnected = true;
        }
        if stats.advance_count > 0 {
            let cur = sess.current_frame();
            if stats.load_count > 0 {
                rollbacks += 1;
                deepest = deepest.max(stats.advance_count.saturating_sub(1));
            }
            // Frames re-run inside this tick were filed earlier with predicted
            // input: drop them. Only the tick's final frame is observed.
            let first = cur - stats.advance_count as i32 + 1;
            for f in first..cur {
                hashes.remove(&f);
            }
            hashes.insert(cur, ram_hash(&core));
            // gstate 0x02 = fighting (main.rs GS_FIGHTING); outside a fight
            // the health words hold other data.
            let fighting = mk2
                && memory::peek_u16(&core, crate::mk2_addrs::a().GSTATE_ADDR, memory::Endian::Little)
                    == Some(0x02);
            if fighting {
                fight_frames += 1;
                let a = crate::mk2_addrs::a();
                for (i, addr) in [a.P1_HP_ADDR, a.P2_HP_ADDR].into_iter().enumerate() {
                    if let Some(hp) = memory::peek_u16(&core, addr, memory::Endian::Little) {
                        if hp > 0 {
                            hp_min[i] = hp_min[i].min(hp);
                        }
                    }
                }
            }
        }

        if sess.confirmed_frame() >= o.frames
            || disconnected
            || (desyncs > 0 && !keep_going)
            || started.elapsed() > deadline
        {
            break;
        }
        next += FRAME;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }

    let confirmed = sess.confirmed_frame();
    let mut f = std::fs::File::create(log_path(handle))?;
    writeln!(f, "# boot {boot_hash:016x} confirmed {confirmed} rollbacks {rollbacks} deepest {deepest} desyncs {desyncs}")?;
    for (frame, h) in hashes.range(..=confirmed) {
        writeln!(f, "{frame} {h:016x}")?;
    }
    println!(
        "{tag} done: confirmed {confirmed}, {rollbacks} rollbacks (deepest {deepest}), {desyncs} GGRS desync events{}",
        if disconnected { ", peer disconnected" } else { "" }
    );
    if mk2 {
        let show = |v: u16| if v == u16::MAX { "-".to_string() } else { v.to_string() };
        println!(
            "{tag} fighting: {fight_frames} ticks in a fight, lowest health p1 {} p2 {} (full is 161)",
            show(hp_min[0]),
            show(hp_min[1])
        );
    }
    Ok(())
}

struct PeerLog {
    boot: String,
    confirmed: i32,
    rollbacks: u64,
    deepest: u64,
    desyncs: u64,
    frames: BTreeMap<i32, String>,
}

fn read_log(path: &str) -> Result<PeerLog, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    let mut log = PeerLog {
        boot: String::new(),
        confirmed: -1,
        rollbacks: 0,
        deepest: 0,
        desyncs: 0,
        frames: BTreeMap::new(),
    };
    for line in text.lines() {
        let p: Vec<&str> = line.split_whitespace().collect();
        if p.first() == Some(&"#") {
            let get = |k: &str| p.iter().position(|x| *x == k).and_then(|i| p.get(i + 1)).copied();
            log.boot = get("boot").unwrap_or("?").to_string();
            log.confirmed = get("confirmed").and_then(|v| v.parse().ok()).unwrap_or(-1);
            log.rollbacks = get("rollbacks").and_then(|v| v.parse().ok()).unwrap_or(0);
            log.deepest = get("deepest").and_then(|v| v.parse().ok()).unwrap_or(0);
            log.desyncs = get("desyncs").and_then(|v| v.parse().ok()).unwrap_or(0);
        } else if p.len() == 2 {
            if let Ok(frame) = p[0].parse() {
                log.frames.insert(frame, p[1].to_string());
            }
        }
    }
    Ok(log)
}

/// `--net-probe`: spawn both peers, wait, compare.
pub fn run_pair() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = parse_opts(&args);
    let exe = std::env::current_exe()?;
    for h in 0..2 {
        let _ = std::fs::remove_file(log_path(h));
    }
    println!(
        "[net-probe] two peers on 127.0.0.1:{}/{} for {} frames (~{} s)",
        PORTS[0],
        PORTS[1],
        o.frames,
        (o.frames as f64 * FRAME.as_secs_f64()).round()
    );
    let mut children = Vec::new();
    for h in 0..2usize {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--net-probe-peer").arg(h.to_string()).args(opts_args(&o));
        if let Some(rom) = arg_after(&args, "--rom") {
            cmd.arg("--rom").arg(rom);
        }
        children.push(cmd.spawn()?);
    }
    let mut child_ok = true;
    for mut c in children {
        child_ok &= c.wait()?.success();
    }

    let (a, b) = (read_log(&log_path(0))?, read_log(&log_path(1))?);
    let shared: Vec<i32> = a.frames.keys().filter(|f| b.frames.contains_key(f)).copied().collect();
    let first_bad = shared.iter().find(|f| a.frames[f] != b.frames[f]);
    let bad_count = shared.iter().filter(|f| a.frames[*f] != b.frames[*f]).count();
    println!(
        "[net-probe] p1: confirmed {}, {} rollbacks (deepest {}), {} desync events",
        a.confirmed, a.rollbacks, a.deepest, a.desyncs
    );
    println!(
        "[net-probe] p2: confirmed {}, {} rollbacks (deepest {}), {} desync events",
        b.confirmed, b.rollbacks, b.deepest, b.desyncs
    );
    let boot_ok = a.boot == b.boot;
    println!(
        "[net-probe] boot hash {} ({} / {})",
        if boot_ok { "matches" } else { "DIFFERS" },
        a.boot,
        b.boot
    );
    let mut ok = child_ok && boot_ok && a.desyncs == 0 && b.desyncs == 0 && !shared.is_empty();
    match first_bad {
        Some(f) => {
            ok = false;
            println!(
                "[net-probe] DESYNC at frame {f}: p1 {} vs p2 {} ({bad_count} of {} compared frames differ)",
                a.frames[f], b.frames[f], shared.len()
            );
        }
        None if !shared.is_empty() => println!(
            "[net-probe] IN SYNC: {} confirmed frames identical ({}..{})",
            shared.len(),
            shared[0],
            shared[shared.len() - 1]
        ),
        None => println!("[net-probe] no frames to compare"),
    }
    println!("[net-probe] {}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
