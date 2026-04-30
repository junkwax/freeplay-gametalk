mod cli;
mod config;
mod controllers;
mod diag;
mod discord_webhook;
mod drone;
mod font;
mod ghost;
mod input;
mod log;
mod matchmaking;
mod memory;
mod menu;
mod menu_input;
mod netcore;
mod netplay;
mod png;
mod protocol;
mod render;
mod replay;
mod retro;
mod rom;
mod rpc;
mod score;
mod session;
mod turn_relay;
mod turn_socket;
mod version;

use crate::cli::{parse_args, NetMode};
use crate::controllers::{assign_pad, open_initial_controllers, pad_owner, Pads};
use crate::font::Font;
use crate::input::{set_action, Bindings, Player};
use crate::menu::{AppState, MenuScreen, NavResult, LOGICAL_H, LOGICAL_W};
use crate::menu_input::{capture_rebind, event_to_menu_nav, is_cancel, is_clear, MenuNav};
use crate::netcore::{reset_for_netplay, step_netplay_frame, NetRuntime};
use crate::render::{
    draw_emu_frame, draw_fight_overlay, ensure_core_loaded, format_probe_result, route_player,
};
use crate::retro::*;
use crate::session::{
    finalize_net_recording, handle_score_event, maybe_start_net_recording, open_net_log,
    rom_fingerprint,
};

use sdl2::audio::AudioQueue;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::BlendMode;
use sdl2::surface::Surface;
use std::time::{Duration, Instant};

#[allow(static_mut_refs)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let net_mode = parse_args();
    let log_tag = match &net_mode {
        NetMode::Local => "local".to_string(),
        NetMode::P2P { player, .. } => format!("p{}", player + 1),
    };
    log::init(&log_tag);
    dlog!("boot", "net_mode={net_mode:?}");
    println!("Net mode: {net_mode:?}");

    protocol::register_uri_scheme();

    for arg in std::env::args().skip(1) {
        if let Some(protocol::XbandUri::Join { room_id }) = protocol::parse_uri(&arg) {
            println!("[main] xband:// deep link: join room {room_id}");
            rpc::post_join_request(room_id);
            break;
        }
    }

    let sdl_context = sdl2::init()?;
    let video_subsystem = sdl_context.video()?;
    let audio_subsystem = sdl_context.audio()?;
    let controller_subsystem = sdl_context.game_controller()?;
    let mut pads: Pads = open_initial_controllers(&controller_subsystem);

    let mut window = video_subsystem
        .window("Freeplay", 1200, 762)
        .position_centered()
        .resizable()
        .build()?;
    if let Ok(icon) =
        Surface::load_bmp("src/app_icon.bmp").or_else(|_| Surface::load_bmp("app_icon.bmp"))
    {
        window.set_icon(icon);
    }
    let mut canvas = window.into_canvas().build()?;
    canvas.set_blend_mode(BlendMode::Blend);
    canvas.set_logical_size(LOGICAL_W as u32, LOGICAL_H as u32)?;
    let texture_creator = canvas.texture_creator();

    let mut emu_texture =
        texture_creator.create_texture_streaming(PixelFormatEnum::ARGB8888, 400, 254)?;

    let ttf_ctx = match sdl2::ttf::init() {
        Ok(c) => Some(c),
        Err(e) => {
            println!("SDL2_ttf init failed ({e}); using bitmap font");
            None
        }
    };
    let mut font = Font::new(&texture_creator, ttf_ctx.as_ref())?;

    let mut event_pump = sdl_context.event_pump()?;

    let mut cfg = config::load();
    let mut state = AppState::default();
    let rom_present = || rom::find_rom_zip().is_some();

    let mut discord_user: Option<String> = matchmaking::username_from_cached_token();
    let mut discord_id: Option<String> = matchmaking::discord_id_from_cached_token();
    let mut score_tracker = score::ScoreTracker::new();

    let mut core: Option<retro::Core> = None;
    let mut audio_queue: Option<AudioQueue<i16>> = None;
    let mut save_slot: Option<Vec<u8>> = None;
    let mut rewind_test: Option<replay::RewindTest> = None;
    let mut ghost_recording: Option<ghost::Recording> = None;
    let mut ghost_playback: Option<ghost::Playback> = None;
    let mut ghost_port_mask: u8 = 0b11;
    let mut drone_runner: Option<drone::DroneRunner> = None;
    let ghost_path = std::path::Path::new("ghost.bin").to_path_buf();
    const GHOST_CAP_PER_PEER: u32 = 3;
    let mut ghost_library = ghost::Library::load_default();
    let mut net_recording: Option<ghost::NetRecording> = None;

    let mut trainer = memory::PokeList::new();
    trainer.add(
        "p1_health",
        memory::Poke::U16 {
            addr: 0x253D6,
            value: 0x00A1,
            endian: memory::Endian::Little,
        },
    );
    trainer.add(
        "p2_health",
        memory::Poke::U16 {
            addr: 0x25550,
            value: 0x00A1,
            endian: memory::Endian::Little,
        },
    );
    trainer.add_with_release(
        "hitboxes",
        memory::Poke::U16 {
            addr: 0x2576E,
            value: 0x0001,
            endian: memory::Endian::Little,
        },
        memory::Poke::U16 {
            addr: 0x2576E,
            value: 0x0000,
            endian: memory::Endian::Little,
        },
    );
    trainer.add_with_release(
        "freeze_timer",
        memory::Poke::U16 {
            addr: 0x250EE,
            value: 0x0001,
            endian: memory::Endian::Little,
        },
        memory::Poke::U16 {
            addr: 0x250EE,
            value: 0x0000,
            endian: memory::Endian::Little,
        },
    );

    const GSTATE_ADDR: usize = 0x253B2;
    const GS_AMODE: u16 = 0x01;
    let mut auto_start_frame: u32 = 0;
    let mut auto_start_done = false;

    const NETPLAY_MATCH_LIMIT: u32 = 3;
    let mut net_match_count: u32 = 0;
    let mut net_in_fight: bool = false;
    let mut net_teardown_reason: Option<String> = None;
    let mut net_frames_since_progress: u32 = 0;
    let mut net_log: Option<std::fs::File> = None;
    let mut net_runtime = NetRuntime::default();
    let mut net_stats_next_frame: u32 = 0;
    let mut net_spectate_next: u32 = 165; // ~3s
    let mut net_frame_counter: u32 = 0;
    const GS_FIGHTING: u16 = 0x02;
    const GS_GAMEOVER: u16 = 0x0b;
    let mut session_p1_wins: u32 = 0;
    let mut session_p2_wins: u32 = 0;
    const P1_HP_ADDR: usize = 0x253D6;
    const P2_HP_ADDR: usize = 0x25550;
    let mut ghost_in_fight: bool = false;

    let mut net_session: Option<netplay::Session> = None;
    let mut local_handle: usize = 0;

    let mut mm_rx: Option<std::sync::mpsc::Receiver<matchmaking::Update>> = None;
    let mut mm_session_id: Option<String> = None;
    let mut rpc_client = rpc::RpcClient::init();
    if let Some(ref mut rc) = rpc_client {
        rc.update(rpc::RpcUpdate::default());
    }
    let mut spar_room_id: Option<String> = None;
    let mut peer_name: Option<String> = None;
    let mut profile_rx: Option<std::sync::mpsc::Receiver<matchmaking::ProfileUpdate>> = None;
    let mut avatar_rx: Option<std::sync::mpsc::Receiver<Vec<u8>>> = None;
    let mut ghost_list_rx: Option<std::sync::mpsc::Receiver<matchmaking::GhostListUpdate>> = None;
    let mut ghost_download_rx: Option<std::sync::mpsc::Receiver<matchmaking::GhostDownloadUpdate>> =
        None;
    let frame_duration = Duration::from_micros(18281);

    ghost::drain_upload_queue(&cfg.stats_url);
    ghost::queue_all_local_ghosts(
        discord_id.as_deref(),
        discord_user.as_deref(),
        &format!("{:016x}", rom_fingerprint().1),
        &cfg.stats_url,
    );

    'running: loop {
        let frame_start = Instant::now();

        // Check for Discord ACTIVITY_JOIN (friend clicked "Join to Spar")
        if mm_rx.is_none() {
            if let Some(room_id) = rpc::take_join_request() {
                println!("[main] Join-to-spar request: room_id={room_id}");
                let (tx, rx) = std::sync::mpsc::channel();
                mm_rx = Some(rx);
                matchmaking::start_join_room(tx, room_id);
                state = AppState::Menu(MenuScreen::Matchmaking {
                    status: "Joining spar room...".into(),
                });
            }
        }

        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'running,

                Event::ControllerDeviceAdded { which, .. } => {
                    match controller_subsystem.open(which) {
                        Ok(c) => assign_pad(&mut pads, c),
                        Err(e) => println!("Failed to open controller {which}: {e}"),
                    }
                }
                Event::ControllerDeviceRemoved { which, .. } => {
                    for (i, slot) in pads.iter_mut().enumerate() {
                        if let Some(c) = slot {
                            if c.instance_id() == which {
                                println!("P{} controller disconnected", i + 1);
                                *slot = None;
                                input::clear_all_inputs();
                            }
                        }
                    }
                }

                _ if matches!(state, AppState::Rebinding { .. }) => {
                    if is_clear(&event) {
                        if let AppState::Rebinding { action, player, .. } = state.clone() {
                            cfg.bindings.get_mut(player).clear_action(action);
                            config::save(&cfg);
                            state.finish_rebind();
                        }
                    } else if let Some(new_binding) = capture_rebind(&event) {
                        if let AppState::Rebinding { action, player, .. } = state.clone() {
                            cfg.bindings
                                .get_mut(player)
                                .replace_binding(action, new_binding);
                            config::save(&cfg);
                            state.finish_rebind();
                        }
                    } else if is_cancel(&event) {
                        state.finish_rebind();
                    }
                }

                Event::KeyDown {
                    keycode: Some(Keycode::R),
                    repeat: false,
                    ..
                } if matches!(state, AppState::Menu(MenuScreen::Controls { .. })) => {
                    cfg.bindings = Bindings::default();
                    config::save(&cfg);
                    println!("Bindings reset to defaults");
                }

                Event::TextInput { text, .. }
                    if matches!(
                        state,
                        AppState::Menu(menu::MenuScreen::TestIp { editing: true, .. })
                    ) =>
                {
                    state.text_input(&text);
                }
                Event::KeyDown {
                    keycode: Some(Keycode::Backspace),
                    ..
                } if matches!(
                    state,
                    AppState::Menu(menu::MenuScreen::TestIp { editing: true, .. })
                ) =>
                {
                    state.text_backspace();
                }

                _ if matches!(state, AppState::Menu(MenuScreen::Matchmaking { .. })) => {
                    if let Event::KeyDown {
                        keycode: Some(Keycode::Escape),
                        ..
                    } = event
                    {
                        println!("[mm] matchmaking canceled by user");
                        mm_rx = None;
                        state = AppState::Menu(MenuScreen::Main { cursor: 0 });
                    }
                }

                // Sign Out via 'S' key on main menu
                Event::KeyDown {
                    keycode: Some(Keycode::S),
                    ..
                } if matches!(state, AppState::Menu(MenuScreen::Main { .. })) => {
                    if discord_user.is_some() {
                        matchmaking::clear_cached_token();
                        discord_user = None;
                        discord_id = None;
                        println!("[auth] Signed out");
                    }
                }

                // Open GitHub on Enter from About screen
                Event::KeyDown {
                    keycode: Some(Keycode::Return),
                    ..
                } if matches!(state, AppState::Menu(MenuScreen::About)) => {
                    let _ = open::that("https://github.com/junkwax/freeplay-gametalk");
                }

                _ if matches!(state, AppState::Menu(_)) => {
                    if let Some(nav) = event_to_menu_nav(&event) {
                        match nav {
                            MenuNav::Up => state.nav_up(),
                            MenuNav::Down => state.nav_down(),
                            MenuNav::Accept => match state.nav_accept(rom_present()) {
                                NavResult::StartGame => {
                                    ensure_core_loaded(
                                        &mut core,
                                        &mut audio_queue,
                                        &audio_subsystem,
                                    )?;
                                    if net_session.is_none() {
                                        if let NetMode::P2P {
                                            player,
                                            local_port,
                                            peer,
                                        } = &net_mode
                                        {
                                            local_handle = *player;
                                            match netplay::start_session(
                                                *local_port,
                                                *player,
                                                *peer,
                                            ) {
                                                Ok(s) => {
                                                    net_session = Some(s);
                                                    net_recording = maybe_start_net_recording(
                                                        &ghost_library,
                                                        *peer,
                                                        GHOST_CAP_PER_PEER,
                                                    );
                                                    net_log = open_net_log();
                                                }
                                                Err(e) => {
                                                    println!("[net] session start failed: {e}")
                                                }
                                            }
                                        }
                                    }
                                }
                                NavResult::StartMatchmaking => {
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    mm_rx = Some(rx);
                                    matchmaking::start(tx);
                                }
                                NavResult::OpenGhostSelect => {
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    ghost_list_rx = Some(rx);
                                    let rh = rom_fingerprint().1;
                                    let rom_hash = format!("{:016x}", rh);
                                    matchmaking::fetch_ghost_list(
                                        cfg.stats_url.clone(),
                                        rom_hash,
                                        tx,
                                    );
                                }
                                NavResult::DownloadGhost(ghost_id) => {
                                    let local_path = format!("ghosts/remote_{ghost_id}.ncgh");
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    ghost_download_rx = Some(rx);
                                    if let AppState::Menu(menu::MenuScreen::GhostSelect {
                                        ref mut download_status,
                                        ..
                                    }) = state
                                    {
                                        *download_status =
                                            Some(format!("Downloading {ghost_id}..."));
                                    }
                                    matchmaking::download_ghost(
                                        cfg.stats_url.clone(),
                                        ghost_id,
                                        local_path,
                                        tx,
                                    );
                                }
                                NavResult::OpenProfile => {
                                    if let Some(did) = discord_id.clone() {
                                        let (tx, rx) = std::sync::mpsc::channel();
                                        profile_rx = Some(rx);
                                        matchmaking::fetch_profile(cfg.stats_url.clone(), did, tx);
                                    } else {
                                        state = AppState::Menu(MenuScreen::Profile {
                                            state: menu::ProfileScreenState::NotLoggedIn,
                                        });
                                    }
                                }
                                NavResult::SignOut => {
                                    matchmaking::clear_cached_token();
                                    discord_user = None;
                                    discord_id = None;
                                    println!("[auth] Signed out");
                                }
                                NavResult::RunProbe { peer } => {
                                    let (_rom_size, rom_hash_u64) = rom_fingerprint();
                                    let report = netplay::probe_connection(
                                        peer,
                                        5,
                                        version::VERSION,
                                        rom_hash_u64,
                                    );
                                    let lines = format_probe_result(peer, rom_hash_u64, &report);
                                    for l in &lines {
                                        println!("[probe] {}", l);
                                    }
                                    state = AppState::Menu(MenuScreen::TestResult { lines });
                                }
                                NavResult::Quit => break 'running,
                                NavResult::BeginRebind => {}
                                NavResult::ClearAllBindings(player) => {
                                    cfg.bindings.get_mut(player).clear_all();
                                    config::save(&cfg);
                                    println!("Cleared all bindings for {}", player.label());
                                }
                                NavResult::Stay => {}
                                NavResult::LoadGhost(path) => {
                                    ensure_core_loaded(
                                        &mut core,
                                        &mut audio_queue,
                                        &audio_subsystem,
                                    )?;
                                    if let Some(c) = &core {
                                        match ghost::Playback::load(&path) {
                                            Ok(pb) => {
                                                if pb.prime(c) {
                                                    println!(
                                                        "[ghost] Loaded: {} frames",
                                                        pb.frame_count()
                                                    );
                                                    ghost_port_mask = 0b10;
                                                    ghost_playback = Some(pb);
                                                    drone_runner = None;
                                                    state = AppState::Playing;
                                                } else {
                                                    println!(
                                                        "[ghost] Anchor state rejected by core."
                                                    );
                                                    state = AppState::Menu(MenuScreen::Main {
                                                        cursor: 3,
                                                    });
                                                }
                                            }
                                            Err(e) => {
                                                println!("[ghost] Load failed: {e}");
                                                state =
                                                    AppState::Menu(MenuScreen::Main { cursor: 3 });
                                            }
                                        }
                                    } else {
                                        state = AppState::Menu(MenuScreen::Main { cursor: 3 });
                                    }
                                }
                            },
                            MenuNav::Back => state.nav_back(),
                            MenuNav::ToggleMenu => {}
                            MenuNav::SwitchPlayer => state.nav_switch_player(),
                        }
                    }
                }

                _ if state == AppState::Playing => match event {
                    Event::KeyDown {
                        keycode: Some(Keycode::F1),
                        ..
                    }
                    | Event::KeyDown {
                        keycode: Some(Keycode::Escape),
                        ..
                    } => {
                        if net_session.is_some() {
                            net_teardown_reason = Some("you quit the match".into());
                        } else {
                            input::clear_all_inputs();
                            state = AppState::Menu(MenuScreen::Main { cursor: 0 });
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F5),
                        repeat: false,
                        ..
                    } if net_session.is_none() => {
                        if let Some(c) = &core {
                            match c.save_state() {
                                Some(buf) => {
                                    println!("State saved ({} bytes)", buf.len());
                                    save_slot = Some(buf);
                                }
                                None => println!("Save failed: core refused to serialize"),
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F9),
                        repeat: false,
                        ..
                    } if net_session.is_none() => {
                        if core.is_some() && rewind_test.is_none() {
                            println!(
                                "[rewind] Starting rewind test — recording {} frames...",
                                replay::REWIND_FRAMES
                            );
                            rewind_test = Some(replay::RewindTest::new());
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F10),
                        repeat: false,
                        ..
                    } if net_session.is_none() && ghost_playback.is_some() => {
                        if drone_runner.is_some() {
                            drone_runner = None;
                            println!("[drone] Disabled — sequential ghost playback");
                        } else {
                            let index = drone::DroneIndex::build(ghost_playback.as_ref().unwrap());
                            drone_runner = Some(drone::DroneRunner::new(index));
                            println!("[drone] Enabled — posture-reactive playback");
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F7),
                        repeat: false,
                        ..
                    } if net_session.is_none() => match (&core, &save_slot) {
                        (Some(c), Some(buf)) => {
                            if c.load_state(buf) {
                                println!("State loaded ({} bytes)", buf.len());
                            } else {
                                println!("Load failed: core rejected state");
                            }
                        }
                        (_, None) => println!("No save slot to load"),
                        _ => {}
                    },
                    Event::KeyDown {
                        keycode: Some(Keycode::F2),
                        repeat: false,
                        ..
                    } if net_session.is_none() => {
                        let on = !trainer.is_enabled("hitboxes");
                        trainer.set_enabled("hitboxes", on);
                        println!("[trainer] Hitbox view: {}", if on { "ON" } else { "OFF" });
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F3),
                        repeat: false,
                        ..
                    } if net_session.is_none() => {
                        let on = !trainer.is_enabled("p1_health");
                        trainer.set_enabled("p1_health", on);
                        trainer.set_enabled("p2_health", on);
                        println!(
                            "[trainer] Infinite health: {}",
                            if on { "ON" } else { "OFF" }
                        );
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F4),
                        repeat: false,
                        ..
                    } if net_session.is_none() => {
                        let on = !trainer.is_enabled("freeze_timer");
                        trainer.set_enabled("freeze_timer", on);
                        println!("[trainer] Freeze timer: {}", if on { "ON" } else { "OFF" });
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F6),
                        repeat: false,
                        ..
                    } => {
                        if net_session.is_some() {
                            println!("[ghost] Recording disabled in netplay mode.");
                        } else if let Some(rec) = ghost_recording.take() {
                            match rec.save(&ghost_path) {
                                Ok(_) => println!(
                                    "[ghost] Saved {} frames to {}",
                                    rec.frame_count(),
                                    ghost_path.display()
                                ),
                                Err(e) => println!("[ghost] Save failed: {}", e),
                            }
                        } else if ghost_playback.is_some() {
                            println!("[ghost] Can't record during playback.");
                        } else if let Some(c) = &core {
                            match ghost::Recording::start(c) {
                                Some(rec) => ghost_recording = Some(rec),
                                None => println!("[ghost] Couldn't capture anchor state."),
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F8),
                        repeat: false,
                        ..
                    } => {
                        if net_session.is_some() {
                            println!("[ghost] Playback disabled in netplay mode.");
                        } else if ghost_recording.is_some() {
                            println!("[ghost] Can't play back while recording.");
                        } else if ghost_playback.is_some() {
                            ghost_playback = None;
                            println!("[ghost] Playback stopped.");
                        } else if let Some(c) = &core {
                            match ghost::Playback::load(&ghost_path) {
                                Ok(pb) => {
                                    if pb.prime(c) {
                                        println!(
                                            "[ghost] Playing back {} frames (full)...",
                                            pb.frame_count()
                                        );
                                        ghost_port_mask = 0b11;
                                        ghost_playback = Some(pb);
                                    } else {
                                        println!("[ghost] Anchor state rejected by core.");
                                    }
                                }
                                Err(e) => println!("[ghost] Load failed: {}", e),
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F12),
                        repeat: false,
                        ..
                    } => {
                        if net_session.is_some() {
                            println!("[ghost] Playback disabled in netplay mode.");
                        } else if ghost_recording.is_some() {
                            println!("[ghost] Can't play back while recording.");
                        } else if ghost_playback.is_some() {
                            ghost_playback = None;
                            println!("[ghost] Playback stopped.");
                        } else if let Some(c) = &core {
                            let mut load_result = ghost::Playback::load(&ghost_path);
                            if load_result.is_err() {
                                if let Ok(p) = ghost::pick_random_ghost("ghosts") {
                                    load_result = ghost::Playback::load(&p);
                                }
                            }
                            match load_result {
                                Ok(pb) => {
                                    if pb.prime(c) {
                                        println!(
                                            "[ghost] Play vs ghost: {} frames, you are P1...",
                                            pb.frame_count()
                                        );
                                        ghost_port_mask = 0b10;
                                        ghost_playback = Some(pb);
                                    } else {
                                        println!("[ghost] Anchor state rejected by core.");
                                    }
                                }
                                Err(e) => println!("[ghost] Load failed: {}", e),
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F11),
                        repeat: false,
                        ..
                    } if net_session.is_none() => {
                        if let Some(c) = &core {
                            if let Some(snap) = memory::snapshot(c) {
                                let ts = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0);
                                let path = format!("ram_{}.bin", ts);
                                match std::fs::write(&path, &snap) {
                                    Ok(_) => {
                                        println!("[F11] Dumped {} bytes to {}", snap.len(), path)
                                    }
                                    Err(e) => println!("[F11] Dump failed: {}", e),
                                }
                            } else {
                                println!("[F11] Core exposed no SYSTEM_RAM");
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(k),
                        repeat: false,
                        ..
                    } => {
                        for p in [Player::P1, Player::P2] {
                            for a in cfg.bindings.get(p).actions_for_key(k) {
                                let dest = route_player(p, &net_session, local_handle);
                                dlog!(
                                    "input",
                                    "KeyDown {:?} bound={:?} dest={:?} action={:?}",
                                    k,
                                    p,
                                    dest,
                                    a
                                );
                                set_action(dest, a, true);
                            }
                        }
                    }
                    Event::KeyUp {
                        keycode: Some(k), ..
                    } => {
                        for p in [Player::P1, Player::P2] {
                            for a in cfg.bindings.get(p).actions_for_key(k) {
                                let dest = route_player(p, &net_session, local_handle);
                                dlog!(
                                    "input",
                                    "KeyUp   {:?} bound={:?} dest={:?} action={:?}",
                                    k,
                                    p,
                                    dest,
                                    a
                                );
                                set_action(dest, a, false);
                            }
                        }
                    }
                    Event::ControllerButtonDown { which, button, .. } => {
                        if let Some(p) = pad_owner(&pads, which) {
                            for a in cfg.bindings.get(p).actions_for_button(button) {
                                let dest = route_player(p, &net_session, local_handle);
                                dlog!(
                                    "input",
                                    "PadDown pad={} {:?} bound={:?} dest={:?} action={:?}",
                                    which,
                                    button,
                                    p,
                                    dest,
                                    a
                                );
                                set_action(dest, a, true);
                            }
                        } else {
                            dlog!("input", "PadDown pad={} {:?} -- no owner", which, button);
                        }
                    }
                    Event::ControllerButtonUp { which, button, .. } => {
                        if let Some(p) = pad_owner(&pads, which) {
                            for a in cfg.bindings.get(p).actions_for_button(button) {
                                let dest = route_player(p, &net_session, local_handle);
                                dlog!(
                                    "input",
                                    "PadUp   pad={} {:?} bound={:?} dest={:?} action={:?}",
                                    which,
                                    button,
                                    p,
                                    dest,
                                    a
                                );
                                set_action(dest, a, false);
                            }
                        }
                    }
                    Event::ControllerAxisMotion {
                        which, axis, value, ..
                    } => {
                        if let Some(p) = pad_owner(&pads, which) {
                            for (a, pressed) in cfg.bindings.get(p).axis_updates(axis, value) {
                                let dest = route_player(p, &net_session, local_handle);
                                dlog!("input", "PadAxis pad={} {:?}={} bound={:?} dest={:?} action={:?} pressed={}",
                                          which, axis, value, p, dest, a, pressed);
                                set_action(dest, a, pressed);
                            }
                        }
                    }
                    _ => {}
                },

                _ => {}
            }
        }

        match &state {
            AppState::Playing => {
                if let Some(c) = &core {
                    if let Some(sess) = net_session.as_mut() {
                        let gstate =
                            memory::peek_u16(c, GSTATE_ADDR, memory::Endian::Little).unwrap_or(0);
                        if gstate == GS_FIGHTING {
                            net_in_fight = true;
                        } else if net_in_fight && gstate == GS_GAMEOVER {
                            net_in_fight = false;
                            net_match_count += 1;
                            let line = format!(
                                "[net] Match {}/{} complete.",
                                net_match_count, NETPLAY_MATCH_LIMIT
                            );
                            println!("{}", line);
                            if let Some(f) = net_log.as_mut() {
                                use std::io::Write;
                                let _ = writeln!(f, "{}", line);
                            }
                            if net_match_count >= NETPLAY_MATCH_LIMIT {
                                net_teardown_reason = Some(format!(
                                    "match limit reached ({} matches)",
                                    NETPLAY_MATCH_LIMIT
                                ));
                            }
                        }

                        // ── Live score tracking ──────────────────────────────────────
                        let now_score = score::Score::read(c);
                        for ev in score_tracker.step(now_score) {
                            if let score::ScoreEvent::MatchOver { winner, .. } = ev {
                                if winner == 1 {
                                    session_p1_wins += 1;
                                } else {
                                    session_p2_wins += 1;
                                }
                            }
                            handle_score_event(
                                ev,
                                local_handle,
                                discord_user.as_deref(),
                                &cfg.discord_webhook_url,
                                &mut net_log,
                                mm_session_id.as_deref(),
                            );
                        }

                        let pre_confirmed = sess.confirmed_frame();
                        let pre_ready = matches!(sess.current_state(), ggrs::SessionState::Running);

                        let step_stats = step_netplay_frame(
                            c,
                            sess,
                            local_handle,
                            &mut net_recording,
                            &mut net_log,
                            &mut net_runtime,
                        );

                        if net_frame_counter < 10 {
                            if let Some(f) = net_log.as_mut() {
                                use std::io::Write;
                                let _ = writeln!(f,
                                    "[net/early] frame={} sess_state={:?} advance_count={} loads={} confirmed={}",
                                    net_frame_counter, sess.current_state(),
                                    step_stats.advance_count, step_stats.load_count,
                                    sess.confirmed_frame());
                            }
                        }

                        if step_stats.advance_count > 1 {
                            if let Some(f) = net_log.as_mut() {
                                use std::io::Write;
                                let _ = writeln!(
                                    f,
                                    "[net/rollback] frame={} resim_depth={} loads={}",
                                    net_frame_counter,
                                    step_stats.advance_count - 1,
                                    step_stats.load_count
                                );
                            }
                        }

                        let post_confirmed = sess.confirmed_frame();
                        if pre_ready && post_confirmed > pre_confirmed {
                            net_frames_since_progress = 0;
                        } else {
                            net_frames_since_progress = net_frames_since_progress.saturating_add(1);
                        }
                        if step_stats.peer_disconnected && net_teardown_reason.is_none() {
                            net_teardown_reason = Some("peer disconnected".into());
                        } else if net_frames_since_progress > 120 && net_teardown_reason.is_none() {
                            net_teardown_reason = Some("peer timed out (no progress)".into());
                        }

                        net_frame_counter = net_frame_counter.wrapping_add(1);
                        if net_frame_counter >= net_stats_next_frame {
                            net_stats_next_frame = net_frame_counter.wrapping_add(275);
                            let remote_handle = 1 - local_handle;
                            if let Ok(stats) = sess.network_stats(remote_handle) {
                                let line = format!(
                                    "[net/diag] frame={} confirmed={} ping={}ms kbps_sent={} local_frames_ahead={} remote_frames_ahead={}",
                                    net_frame_counter,
                                    sess.confirmed_frame(),
                                    stats.ping,
                                    stats.kbps_sent,
                                    stats.local_frames_behind,
                                    stats.remote_frames_behind,
                                );
                                println!("{}", line);
                                if let Some(f) = net_log.as_mut() {
                                    use std::io::Write;
                                    let _ = writeln!(f, "{}", line);
                                }
                            }
                        }
                    } else {
                        input::commit_live_to_state();
                        if !auto_start_done {
                            let gstate = memory::peek_u16(c, GSTATE_ADDR, memory::Endian::Little)
                                .unwrap_or(0);
                            if gstate == GS_AMODE {
                                let pulse = (auto_start_frame % 24) < 4;
                                unsafe {
                                    INPUT_STATE[0][RETRO_DEVICE_ID_JOYPAD_START as usize] = pulse;
                                }
                                auto_start_frame = auto_start_frame.wrapping_add(1);
                            } else if gstate != 0 {
                                unsafe {
                                    INPUT_STATE[0][RETRO_DEVICE_ID_JOYPAD_START as usize] = false;
                                }
                                auto_start_done = true;
                            }
                        }
                        if let Some(pb) = ghost_playback.as_mut() {
                            if let Some(drone) = drone_runner.as_mut() {
                                // Drone mode: pick posture-matched inputs
                                let gs = drone::GameState::read(c);
                                let p1_input = drone.next_input(&gs);
                                unsafe {
                                    for b in 0..16 {
                                        // Inject drone's P1 inputs, keep human P2 inputs
                                        if (ghost_port_mask & 0b01) != 0 {
                                            INPUT_STATE[0][b] = (p1_input >> b) & 1 != 0;
                                        }
                                    }
                                }

                                // Push spectator frame to signaling server every ~3s
                                if net_frame_counter >= net_spectate_next {
                                    net_spectate_next = net_frame_counter.wrapping_add(165);
                                    if let Some(ref sid) = mm_session_id {
                                        let now_score = score::Score::read(c);
                                        session::push_spectator_frame(
                                            sid,
                                            now_score.p1_match_wins,
                                            now_score.p2_match_wins,
                                            net_frame_counter,
                                        );
                                    }
                                }
                            } else {
                                // Normal ghost playback
                                if !pb.inject_next(ghost_port_mask) {
                                    if ghost_port_mask == 0b10 {
                                        pb.rewind_inputs();
                                        let _ = pb.inject_next(ghost_port_mask);
                                    } else {
                                        println!(
                                            "[ghost] Playback complete ({} frames).",
                                            pb.frame_count()
                                        );
                                        ghost_playback = None;
                                        drone_runner = None;
                                    }
                                }
                            }
                        }
                        if let Some(rec) = ghost_recording.as_mut() {
                            rec.record_frame();
                        }
                        // Shared score tracking for local/ghost play
                        if net_session.is_none() {
                            let now_score = score::Score::read(c);
                            for ev in score_tracker.step(now_score) {
                                if let score::ScoreEvent::MatchOver { winner, .. } = ev {
                                    if winner == 1 {
                                        session_p1_wins += 1;
                                    } else {
                                        session_p2_wins += 1;
                                    }
                                }
                            }
                        }
                        if let Some(rt) = rewind_test.as_mut() {
                            let done = rt.record_pre_frame(c);
                            if done {
                                rt.verify(c);
                                rewind_test = None;
                            }
                        }
                        unsafe {
                            (c.run)();
                        }
                        // Ghost match vs detection: track fight start/end for webhook
                        if ghost_playback.is_some() && ghost_port_mask == 0b10 {
                            let gstate = memory::peek_u16(c, GSTATE_ADDR, memory::Endian::Little)
                                .unwrap_or(0);
                            if !ghost_in_fight && gstate == GS_FIGHTING {
                                ghost_in_fight = true;
                            } else if ghost_in_fight && gstate == GS_GAMEOVER {
                                ghost_in_fight = false;
                                let p1_hp = memory::peek_u16(c, P1_HP_ADDR, memory::Endian::Little)
                                    .unwrap_or(0);
                                let p2_hp = memory::peek_u16(c, P2_HP_ADDR, memory::Endian::Little)
                                    .unwrap_or(0);
                                let outcome = if p1_hp > p2_hp { "won" } else { "lost" };
                                let msg = format!(
                                    "🎮 **Ghost Match Result** — Player {} (P1 HP: 0x{p1_hp:04X} | P2 HP: 0x{p2_hp:04X})",
                                    outcome
                                );
                                println!("[ghost] Match end: {msg}");
                                if !cfg.discord_webhook_url.is_empty() {
                                    discord_webhook::post(&cfg.discord_webhook_url, &msg);
                                }
                            }
                        } else if ghost_in_fight {
                            ghost_in_fight = false;
                        }
                    }
                    if net_session.is_none() {
                        trainer.apply(c);
                    }
                    if let Some(q) = &audio_queue {
                        unsafe {
                            if !AUDIO_BUFFER.is_empty() {
                                let max_queued_bytes = (q.spec().freq as u32) * 2 * 2 / 5;
                                if q.size() < max_queued_bytes {
                                    let _ = q.queue_audio(&AUDIO_BUFFER);
                                }
                                AUDIO_BUFFER.clear();
                            }
                        }
                    }

                    if let Some(reason) = net_teardown_reason.take() {
                        let line = format!("[net] Session ended: {reason}");
                        println!("{line}");
                        if let Some(f) = net_log.as_mut() {
                            use std::io::Write;
                            let _ = writeln!(f, "{line}");
                        }
                        let user_quit = reason == "you quit the match"
                            || reason.starts_with("match limit reached");
                        let teardown_lines: Vec<String> = if user_quit {
                            Vec::new()
                        } else {
                            vec![
                                format!("Reason: {reason}"),
                                format!(
                                    "Session ran for {} frames (~{:.1}s)",
                                    net_frame_counter,
                                    net_frame_counter as f32 / 55.0
                                ),
                                format!("Matches completed: {}", net_match_count),
                                String::new(),
                                "FAIL Session dropped unexpectedly.".into(),
                                "".into(),
                                "Common causes:".into(),
                                "  - peer closed Freeplay / lost network".into(),
                                "  - ROM or Freeplay build mismatch between peers".into(),
                                "  - desync detected (check log for DESYNC line)".into(),
                                "".into(),
                                "Full trace: freeplay-net.log".into(),
                            ]
                        };
                        let (_rom_size, rom_hash_u64) = rom_fingerprint();
                        finalize_net_recording(
                            &mut net_recording,
                            &mut ghost_library,
                            &cfg.stats_url,
                            discord_user.as_deref(),
                            discord_id.as_deref(),
                            &format!("{:016x}", rom_hash_u64),
                        );
                        net_session = None;
                        net_match_count = 0;
                        net_in_fight = false;
                        net_frames_since_progress = 0;
                        net_stats_next_frame = 0;
                        net_frame_counter = 0;
                        net_runtime = NetRuntime::default();
                        net_log = None;
                        mm_session_id = None;
                        peer_name = None;
                        auto_start_done = false;
                        auto_start_frame = 0;
                        save_slot = None;
                        ghost_recording = None;
                        ghost_playback = None;
                        drone_runner = None;
                        ghost::drain_upload_queue(&cfg.stats_url);
                        score_tracker.reset();
                        session_p1_wins = 0;
                        session_p2_wins = 0;
                        trainer.set_enabled("hitboxes", false);
                        trainer.set_enabled("p1_health", false);
                        trainer.set_enabled("p2_health", false);
                        trainer.set_enabled("freeze_timer", false);
                        input::clear_all_inputs();
                        state = if teardown_lines.is_empty() {
                            AppState::Menu(MenuScreen::Main { cursor: 0 })
                        } else {
                            AppState::Menu(MenuScreen::TestResult {
                                lines: teardown_lines,
                            })
                        };
                    }
                }
                draw_emu_frame(&mut canvas, &mut emu_texture, &texture_creator)?;
                // Draw modern fight overlay for all play modes
                {
                    if discord_user.is_none() {
                        discord_user = matchmaking::username_from_cached_token();
                    }
                    let local_name = discord_user.as_deref().unwrap_or("You");
                    let ghost_name = if drone_runner.is_some() {
                        "Drone"
                    } else {
                        "Ghost"
                    };
                    let p1 = if net_session.is_some() {
                        if local_handle == 0 {
                            discord_user.as_deref()
                        } else {
                            peer_name.as_deref()
                        }
                    } else if ghost_playback.is_some()
                        && (ghost_port_mask & 0b01) != 0
                        && (ghost_port_mask & 0b10) == 0
                    {
                        Some(ghost_name)
                    } else {
                        Some(local_name)
                    };
                    let p2 = if net_session.is_some() {
                        if local_handle == 1 {
                            discord_user.as_deref()
                        } else {
                            peer_name.as_deref()
                        }
                    } else if ghost_playback.is_some() {
                        if (ghost_port_mask & 0b10) != 0 {
                            Some(ghost_name)
                        } else {
                            Some(local_name)
                        }
                    } else {
                        Some("Practice")
                    };
                    canvas.set_logical_size(0, 0)?;
                    let (win_w, win_h) = canvas.output_size().unwrap_or((1200, 762));
                    draw_fight_overlay(
                        &mut canvas,
                        &mut font,
                        win_w as i32,
                        win_h as i32,
                        p1.unwrap_or("P1"),
                        p2.unwrap_or("P2"),
                        session_p1_wins,
                        session_p2_wins,
                        if net_session.is_some() {
                            Some("ONLINE")
                        } else {
                            None
                        },
                    )
                    .map_err(|e| format!("overlay: {e}"))?;
                    canvas.set_logical_size(LOGICAL_W as u32, LOGICAL_H as u32)?;
                }
                canvas.present();
            }
            AppState::Menu(MenuScreen::Matchmaking { .. }) => {
                if let Some(rx) = &mm_rx {
                    loop {
                        match rx.try_recv() {
                            Ok(matchmaking::Update::Status(s)) => {
                                if let AppState::Menu(MenuScreen::Matchmaking { ref mut status }) =
                                    state
                                {
                                    *status = s;
                                }
                                if discord_user.is_none() {
                                    discord_user = matchmaking::username_from_cached_token();
                                }
                                if discord_id.is_none() {
                                    discord_id = matchmaking::discord_id_from_cached_token();
                                }
                            }
                            Ok(matchmaking::Update::Connected {
                                peer_endpoint,
                                is_host,
                                turn,
                                session_id,
                                peer_username,
                            }) => {
                                let stun_peer: std::net::SocketAddr = match peer_endpoint.parse() {
                                    Ok(a) => a,
                                    Err(e) => {
                                        println!("[mm] bad peer addr from matchmaking: {e}");
                                        mm_rx = None;
                                        state = AppState::Menu(MenuScreen::Main { cursor: 0 });
                                        break;
                                    }
                                };
                                mm_rx = None;
                                mm_session_id = Some(session_id);
                                peer_name = peer_username;
                                discord_user =
                                    matchmaking::username_from_cached_token().or(discord_user);
                                discord_id =
                                    matchmaking::discord_id_from_cached_token().or(discord_id);

                                ensure_core_loaded(&mut core, &mut audio_queue, &audio_subsystem)?;
                                if let Some(c) = &core {
                                    reset_for_netplay(
                                        c,
                                        &mut trainer,
                                        &mut save_slot,
                                        &mut ghost_playback,
                                        &mut ghost_recording,
                                    );
                                }

                                local_handle = if is_host { 0 } else { 1 };
                                let mut log = open_net_log();
                                let mut lines: Vec<String> = Vec::new();

                                // Branch: TURN relay vs direct UDP
                                let result: Result<netplay::Session, Box<dyn std::error::Error>> =
                                    if let Some(creds) = turn {
                                        // ── TURN PATH ──
                                        // Both clients open their own TURN allocation. Each side installs
                                        // a permission for the OTHER's STUN address (their NAT-mapped
                                        // public IP). When we Send to that peer address through TURN,
                                        // coturn finds the matching allocation internally and forwards
                                        // the packet — the two relay sockets never have to talk to each
                                        // other on the public network.
                                        //
                                        // We do NOT install a permission for the peer's TURN-relayed
                                        // address. coturn 4.6 hardcodes a "no peer = own external-ip"
                                        // rule and rejects it. The peer's STUN IP is what we use as
                                        // the GGRS peer label, AND it's what TURN routes by.
                                        println!("[net] using TURN relay: {}", creds.uri);

                                        match turn_socket::TurnSocket::new(
                                            &creds.uri,
                                            &creds.username,
                                            &creds.password,
                                            stun_peer, // permission for peer's STUN addr
                                            menu::DEFAULT_NETPLAY_PORT,
                                        ) {
                                            Ok(socket) => {
                                                let our_relayed = socket.relayed_addr();
                                                println!("[net] our relayed addr: {our_relayed}");
                                                println!("[net] routing through TURN to peer at STUN addr: {stun_peer}");

                                                // Hand to GGRS using the peer's STUN address as the
                                                // GGRS peer label. The TurnSocket internally routes
                                                // every send through the TURN server.
                                                let log_ref = &mut log;
                                                let lines_ref = &mut lines;
                                                netplay::start_session_with_socket(
                                                    local_handle,
                                                    stun_peer,
                                                    socket,
                                                    |line: &str| {
                                                        println!("[net] {}", line);
                                                        if let Some(f) = log_ref.as_mut() {
                                                            use std::io::Write;
                                                            let _ = writeln!(f, "{}", line);
                                                        }
                                                        lines_ref.push(line.to_string());
                                                    },
                                                )
                                                .map_err(|e| {
                                                    Box::new(e) as Box<dyn std::error::Error>
                                                })
                                            }
                                            Err(e) => {
                                                Err(Box::new(e) as Box<dyn std::error::Error>)
                                            }
                                        }
                                    } else {
                                        // ── DIRECT UDP PATH (unchanged) ──
                                        let log_ref = &mut log;
                                        let lines_ref = &mut lines;
                                        netplay::start_session_verbose(
                                            menu::DEFAULT_NETPLAY_PORT,
                                            local_handle,
                                            stun_peer,
                                            |line: &str| {
                                                println!("[net] {}", line);
                                                if let Some(f) = log_ref.as_mut() {
                                                    use std::io::Write;
                                                    let _ = writeln!(f, "{}", line);
                                                }
                                                lines_ref.push(line.to_string());
                                            },
                                        )
                                    };

                                match result {
                                    Ok(s) => {
                                        net_session = Some(s);
                                        net_recording = maybe_start_net_recording(
                                            &ghost_library,
                                            stun_peer,
                                            GHOST_CAP_PER_PEER,
                                        );
                                        net_log = log;
                                        let who = discord_user.as_deref().unwrap_or("Anonymous");
                                        let role = if local_handle == 0 { "P1" } else { "P2" };
                                        discord_webhook::post(
                                            &cfg.discord_webhook_url,
                                            &format!(":crossed_swords: **{who}** ({role}) is in a match - MK2"),
                                        );
                                        state = AppState::Playing;
                                    }
                                    Err(e) => {
                                        let tail = format!("Match connect failed: {e}");
                                        println!("[net] {tail}");
                                        lines.push(String::new());
                                        lines.push(format!("FAIL {tail}"));
                                        lines.push(String::new());
                                        lines.push("Log: freeplay-net.log".into());
                                        state = AppState::Menu(MenuScreen::TestResult { lines });
                                    }
                                }
                                break;
                            }

                            Ok(matchmaking::Update::Error(e)) => {
                                println!("[mm] matchmaking error: {e}");
                                mm_rx = None;
                                state = AppState::Menu(MenuScreen::TestResult {
                                    lines: vec![
                                        String::new(),
                                        format!("FAIL {e}"),
                                        String::new(),
                                        "ESC to go back".into(),
                                    ],
                                });
                                break;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                mm_rx = None;
                                state = AppState::Menu(MenuScreen::Main { cursor: 0 });
                                break;
                            }
                        }
                    }
                }

                canvas.set_logical_size(0, 0)?;
                let (win_w, win_h) = canvas.output_size().unwrap_or((1200, 762));
                menu::draw(
                    &state,
                    &cfg.bindings,
                    &mut canvas,
                    &mut font,
                    win_w as i32,
                    win_h as i32,
                    rom_present(),
                    discord_user.as_deref(),
                )
                .map_err(|e| format!("menu draw: {e}"))?;
                canvas.present();
                canvas.set_logical_size(menu::LOGICAL_W as u32, menu::LOGICAL_H as u32)?;
            }

            AppState::Menu(_) | AppState::Rebinding { .. } => {
                if matches!(
                    state,
                    AppState::Menu(menu::MenuScreen::TestIp { editing: true, .. })
                ) {
                    video_subsystem.text_input().start();
                } else {
                    video_subsystem.text_input().stop();
                }

                // Populate GhostSelect entries when entering the screen.
                // Dedup: for recordings against the same peer (same IP prefix),
                // keep only the most recent (highest timestamp filename).
                if let AppState::Menu(menu::MenuScreen::GhostSelect {
                    ref mut entries, ..
                }) = state
                {
                    if entries.is_empty() {
                        if let Ok(dir) = std::fs::read_dir("ghosts") {
                            let mut files: Vec<(String, String)> = dir
                                .filter_map(|e| e.ok())
                                .filter(|e| {
                                    e.path().extension().map(|x| x == "ncgh").unwrap_or(false)
                                })
                                .map(|e| {
                                    let name = e.file_name().to_string_lossy().to_string();
                                    let path = e.path().to_string_lossy().to_string();
                                    (name, path)
                                })
                                .collect();
                            // Sort by timestamp descending (latest first)
                            fn extract_ts(name: &str) -> u64 {
                                let base = if name.ends_with(".ncgh") {
                                    &name[..name.len() - 5]
                                } else {
                                    name
                                };
                                base.rsplit('_')
                                    .next()
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0)
                            }
                            files.sort_by(|a, b| extract_ts(&b.0).cmp(&extract_ts(&a.0)));
                            *entries = files
                                .into_iter()
                                .map(|(name, path)| menu::GhostEntry::Local {
                                    filename: name,
                                    path,
                                })
                                .collect();
                        }
                    }
                }

                // Drain the profile fetcher channel and update the screen.
                // Runs every menu frame; if the user navigated away mid-fetch
                // we drop the result on Disconnected.
                if let Some(rx) = &profile_rx {
                    if let AppState::Menu(menu::MenuScreen::Profile { ref mut state }) = state {
                        match rx.try_recv() {
                            Ok(matchmaking::ProfileUpdate::Loaded { profile, history }) => {
                                // Spawn avatar download if the profile has an avatar URL
                                if let Some(ref url) = profile.avatar_url {
                                    let url_clone = url.clone();
                                    let (atx, arx) = std::sync::mpsc::channel();
                                    avatar_rx = Some(arx);
                                    std::thread::spawn(move || {
                                        match matchmaking::http_get_bytes(&url_clone) {
                                            Ok(bytes) => {
                                                let _ = atx.send(bytes);
                                            }
                                            Err(e) => {
                                                println!("[avatar] download failed: {e}");
                                            }
                                        }
                                    });
                                }
                                *state = menu::ProfileScreenState::Loaded {
                                    profile,
                                    history,
                                    avatar_rgba: None,
                                };
                                profile_rx = None;
                            }
                            Ok(matchmaking::ProfileUpdate::Error(msg)) => {
                                *state = menu::ProfileScreenState::Error(msg);
                                profile_rx = None;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                profile_rx = None;
                            }
                        }
                    } else {
                        // User left the Profile screen — drop the channel; the
                        // background thread will finish and push to a closed rx.
                        profile_rx = None;
                    }
                }

                // Drain the avatar download channel — decode and cache.
                if let Some(rx) = &avatar_rx {
                    if let AppState::Menu(menu::MenuScreen::Profile {
                        state:
                            menu::ProfileScreenState::Loaded {
                                ref mut avatar_rgba,
                                ..
                            },
                        ..
                    }) = state
                    {
                        match rx.try_recv() {
                            Ok(bytes) => {
                                if let Some((rgba, w, h)) = png::decode_png(&bytes) {
                                    *avatar_rgba = Some((rgba, w, h));
                                }
                                avatar_rx = None;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                avatar_rx = None;
                            }
                        }
                    } else {
                        avatar_rx = None;
                    }
                }

                // Drain the ghost-list fetcher channel.
                if let Some(rx) = &ghost_list_rx {
                    if let AppState::Menu(menu::MenuScreen::GhostSelect {
                        ref mut entries, ..
                    }) = state
                    {
                        match rx.try_recv() {
                            Ok(matchmaking::GhostListUpdate::Loaded(ghosts)) => {
                                let existing: std::collections::HashSet<String> = entries
                                    .iter()
                                    .filter_map(|e| match e {
                                        menu::GhostEntry::Remote(m) => Some(m.ghost_id.clone()),
                                        _ => None,
                                    })
                                    .collect();
                                for meta in ghosts {
                                    if !existing.contains(&meta.ghost_id) {
                                        entries.push(menu::GhostEntry::Remote(meta));
                                    }
                                }
                                ghost_list_rx = None;
                            }
                            Ok(matchmaking::GhostListUpdate::Error(e)) => {
                                println!("[ghost] list fetch failed: {e}");
                                ghost_list_rx = None;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                ghost_list_rx = None;
                            }
                        }
                    } else {
                        ghost_list_rx = None;
                    }
                }

                // Drain the ghost download channel.
                if let Some(rx) = &ghost_download_rx {
                    if let AppState::Menu(menu::MenuScreen::GhostSelect {
                        ref mut cursor,
                        ref mut download_status,
                        ..
                    }) = state
                    {
                        match rx.try_recv() {
                            Ok(matchmaking::GhostDownloadUpdate::Saved { local_path, .. }) => {
                                *download_status = None;
                                ghost_download_rx = None;
                                // Load the downloaded ghost
                                if let Some(c) = &core {
                                    match ghost::Playback::load(&local_path) {
                                        Ok(pb) => {
                                            if pb.prime(c) {
                                                println!(
                                                    "[ghost] Loaded remote: {} frames",
                                                    pb.frame_count()
                                                );
                                                ghost_port_mask = 0b10;
                                                ghost_playback = Some(pb);
                                                *cursor = 3;
                                                state =
                                                    AppState::Menu(MenuScreen::Main { cursor: 3 });
                                            } else {
                                                println!("[ghost] Anchor state rejected.");
                                            }
                                        }
                                        Err(e) => println!("[ghost] Load failed: {e}"),
                                    }
                                }
                            }
                            Ok(matchmaking::GhostDownloadUpdate::Error { message, .. }) => {
                                *download_status = Some(format!("Error: {message}"));
                                ghost_download_rx = None;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                ghost_download_rx = None;
                            }
                        }
                    } else {
                        ghost_download_rx = None;
                    }
                }

                canvas.set_logical_size(0, 0)?;
                let (win_w, win_h) = canvas.output_size().unwrap_or((1200, 762));
                menu::draw(
                    &state,
                    &cfg.bindings,
                    &mut canvas,
                    &mut font,
                    win_w as i32,
                    win_h as i32,
                    rom_present(),
                    discord_user.as_deref(),
                )
                .map_err(|e| format!("menu draw: {e}"))?;
                canvas.present();
                canvas.set_logical_size(menu::LOGICAL_W as u32, menu::LOGICAL_H as u32)?;
            }
        }

        let elapsed = frame_start.elapsed();
        if let Some(ref mut rc) = rpc_client {
            let is_training = ghost_recording.is_some() || ghost_playback.is_some();
            let rpc_state = if let Some(ref _sess) = net_session {
                if let Some(ref name) = peer_name {
                    rpc::RpcState::NetplayVs(name.clone())
                } else {
                    rpc::RpcState::Netplay
                }
            } else if matches!(state, AppState::Menu(menu::MenuScreen::Matchmaking { .. })) {
                rpc::RpcState::Matchmaking
            } else if matches!(state, AppState::Menu(menu::MenuScreen::TestIp { .. })) {
                rpc::RpcState::Joining
            } else if state == AppState::Playing && is_training {
                if spar_room_id.is_none() {
                    spar_room_id = Some(rpc::make_spar_key());
                }
                rpc::RpcState::Training
            } else if state == AppState::Playing {
                spar_room_id = None;
                rpc::RpcState::Playing
            } else {
                spar_room_id = None;
                rpc::RpcState::Menu
            };
            let party = if net_session.is_some() {
                Some((1, 2))
            } else {
                None
            };
            let score = if net_session.is_some() {
                core.as_ref().map(|c| {
                    let s = score::Score::read(c);
                    (s.p1_match_wins, s.p2_match_wins)
                })
            } else {
                None
            };
            let update = rpc::RpcUpdate {
                state: rpc_state,
                ghost_recording: ghost_recording.is_some(),
                ghost_playback: ghost_playback.is_some(),
                join_key: spar_room_id.clone(),
                spectate_key: if net_session.is_some() {
                    Some(format!(
                        "xband://watch/{}",
                        mm_session_id.as_deref().unwrap_or("")
                    ))
                } else {
                    None
                },
                party_id: mm_session_id.clone(),
                party,
                score,
            };
            rc.update(update);
        }
        if elapsed < frame_duration {
            std::thread::sleep(frame_duration - elapsed);
        }
    }

    Ok(())
}
