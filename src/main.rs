#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod cli;
mod clip;
mod config;
mod controllers;
mod diag;
mod discord_webhook;
mod doctor;
mod drone;
mod font;
mod ghost;
mod incident;
mod input;
mod input_history;
mod log;
mod match_replay;
mod matchmaking;
mod memory;
mod menu;
mod menu_input;
mod netcore;
mod netplay;
mod png;
mod protocol;
mod relay_socket;
mod render;
mod replay;
mod retro;
mod rom;
mod rpc;
mod score;
mod session;
mod version;
mod wuname;

use crate::cli::{parse_args, NetMode};
use crate::controllers::{assign_pad, open_initial_controllers, pad_owner, Pads};
use crate::font::Font;
use crate::input::{set_action, Bindings, Player};
use crate::menu::{AppState, MenuScreen, NavResult, LOGICAL_H, LOGICAL_W};
use crate::menu_input::{capture_rebind, event_to_menu_nav, is_cancel, is_clear, MenuNav};
use crate::netcore::{reset_for_netplay, step_netplay_frame, NetRuntime};
use crate::render::{
    draw_chat_overlay, draw_emu_frame, draw_fight_overlay, draw_lab_assist_overlay,
    ensure_core_loaded, format_probe_result, route_player,
};
use crate::retro::*;
use crate::session::{
    finalize_net_recording, handle_score_event, maybe_start_net_recording, open_net_log,
    rom_fingerprint,
};

use sdl2::audio::AudioQueue;
use sdl2::event::Event;
use sdl2::keyboard::{Keycode, Mod};
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::render::BlendMode;
use sdl2::surface::Surface;
use sdl2::video::FullscreenType;
use std::time::{Duration, Instant};

const CHAT_MAX_LINES: usize = 8;

#[cfg(target_os = "windows")]
fn launch_debugger() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new("cmd")
        .args([
            "/C",
            "start",
            "Freeplay Doctor",
            "cmd",
            "/K",
            &format!("\"{}\" --doctor", exe.display()),
        ])
        .spawn()
        .map(|_| ())
}

#[cfg(not(target_os = "windows"))]
fn launch_debugger() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg("--doctor")
        .spawn()
        .map(|_| ())
}

fn toast_payload(toast: &Option<(String, Instant)>) -> Option<menu::Toast<'_>> {
    let (message, until) = toast.as_ref()?;
    if Instant::now() >= *until {
        return None;
    }
    Some(menu::Toast {
        message,
        remaining_ms: until.saturating_duration_since(Instant::now()).as_millis(),
    })
}

fn push_chat_line(lines: &mut Vec<String>, line: String) {
    lines.push(line);
    if lines.len() > CHAT_MAX_LINES {
        let overflow = lines.len() - CHAT_MAX_LINES;
        lines.drain(0..overflow);
    }
}

fn close_chat(chat_open: &mut bool, chat_draft: &mut String) {
    *chat_open = false;
    chat_draft.clear();
    input::clear_all_inputs();
}

fn send_chat_draft(
    relay_chat: Option<&relay_socket::RelayChatHandle>,
    chat_lines: &mut Vec<String>,
    discord_user: Option<&str>,
    chat_draft: &str,
) {
    let msg = chat_draft.trim().to_string();
    if msg.is_empty() {
        return;
    }
    if let Some(chat) = relay_chat {
        match chat.send(&msg) {
            Ok(()) => {
                let who = discord_user.unwrap_or("You");
                push_chat_line(chat_lines, format!("{who}: {msg}"));
            }
            Err(e) => push_chat_line(chat_lines, format!("Chat send failed: {e}")),
        }
    }
}

fn toggle_hitbox_view(trainer: &mut memory::PokeList, toast: &mut Option<(String, Instant)>) {
    let on = !trainer.is_enabled("hitboxes");
    trainer.set_enabled("hitboxes", on);
    println!("[trainer] Hitbox view: {}", if on { "ON" } else { "OFF" });
    *toast = Some((
        format!("Hitbox view {}", if on { "ON" } else { "OFF" }),
        Instant::now() + Duration::from_millis(1800),
    ));
}

const GHOST_P2_MASK: u8 = 0b10;

fn ghost_target_port(mask: u8) -> usize {
    if mask & 0b10 != 0 {
        1
    } else {
        0
    }
}

fn start_logic_ghost_opponent(
    pb: ghost::Playback,
    ghost_port_mask: &mut u8,
    ghost_playback: &mut Option<ghost::Playback>,
    drone_runner: &mut Option<drone::DroneRunner>,
) {
    *ghost_port_mask = GHOST_P2_MASK;
    let target_port = ghost_target_port(*ghost_port_mask);
    let index = drone::DroneIndex::build(&pb, target_port);
    *drone_runner = Some(drone::DroneRunner::new(index, target_port));
    *ghost_playback = Some(pb);
}

fn replay_names(
    local_handle: usize,
    local_name: Option<&str>,
    peer_name: Option<&str>,
) -> (String, String) {
    let local = local_name.unwrap_or("You").to_string();
    let peer = peer_name.unwrap_or("Opponent").to_string();
    if local_handle == 0 {
        (local, peer)
    } else {
        (peer, local)
    }
}

fn refresh_replay_select(state: &mut AppState, status: Option<String>) {
    if let AppState::Menu(MenuScreen::ReplaySelect {
        entries,
        status: screen_status,
        ..
    }) = state
    {
        *entries = match_replay::list_replays()
            .into_iter()
            .map(|meta| menu::ReplayEntry {
                filename: meta.filename,
                path: meta.path,
                p1_name: meta.p1_name,
                p2_name: meta.p2_name,
                frame_count: meta.frame_count,
            })
            .collect();
        *screen_status = status.or_else(|| {
            if entries.is_empty() {
                Some("No saved replays found".into())
            } else {
                None
            }
        });
    }
}

fn apply_volume(samples: &[i16], volume_percent: u8) -> Vec<i16> {
    if volume_percent >= 100 {
        return samples.to_vec();
    }
    let volume = volume_percent as i32;
    samples
        .iter()
        .map(|s| ((*s as i32 * volume) / 100).clamp(i16::MIN as i32, i16::MAX as i32) as i16)
        .collect()
}

fn queue_game_audio(
    q: &AudioQueue<i16>,
    samples: &[i16],
    volume_percent: u8,
    buffer: config::AudioBuffer,
) {
    const BYTES_PER_STEREO_SAMPLE: u32 = 4;
    let freq = q.spec().freq.max(1) as u32;
    let target_bytes = freq * BYTES_PER_STEREO_SAMPLE * buffer.ms() / 1000;
    let max_bytes = target_bytes + freq * BYTES_PER_STEREO_SAMPLE / 10;
    let low_water_bytes = freq * BYTES_PER_STEREO_SAMPLE / 12;
    let queued = q.size();

    if queued >= max_bytes {
        return;
    }

    if queued < low_water_bytes {
        dlog!(
            "audio",
            "low queue: {} ms queued, target={} ms",
            queued * 1000 / (freq * BYTES_PER_STEREO_SAMPLE),
            buffer.ms()
        );
    }

    if volume_percent >= 100 {
        let _ = q.queue_audio(samples);
    } else {
        let scaled = apply_volume(samples, volume_percent);
        let _ = q.queue_audio(&scaled);
    }
}

fn finish_clip_recording(recorder: clip::ClipRecorder) -> String {
    match recorder.finish() {
        Ok(result) => result.message,
        Err(e) => format!("Clip save failed: {e}"),
    }
}

fn install_panic_incident_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        let summary = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "panic".to_string()
        };
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let mut inc = incident::Incident::new(incident::KIND_PANIC, format!("{summary}{location}"));
        inc.net_log_path = Some(std::path::PathBuf::from("freeplay-net.log"));
        let (_rom_size, rom_hash) = rom_fingerprint();
        inc.rom_hash = Some(format!("{rom_hash:016x}"));
        incident::submit_now(&inc);
    }));
}

#[allow(static_mut_refs)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if cli::doctor_requested() {
        let cfg = config::load();
        config::set_signaling_url(cfg.signaling_url.clone());
        std::process::exit(doctor::run());
    }

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
        if let Some(uri) = protocol::parse_uri(&arg) {
            match uri {
                protocol::XbandUri::Join { room_id } => {
                    println!("[main] xband:// deep link: join room {room_id}");
                    rpc::post_join_request(room_id);
                }
                protocol::XbandUri::Watch { session_id } => {
                    println!("[main] xband:// deep link: watch session {session_id}");
                    rpc::post_spectate_request(session_id);
                }
            }
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
    if cfg.fullscreen {
        let _ = canvas.window_mut().set_fullscreen(FullscreenType::Desktop);
    }
    config::set_signaling_url(cfg.signaling_url.clone());
    crate::rpc::set_discord_client_id(cfg.discord_client_id.clone());
    install_panic_incident_hook();
    let mut state = AppState::default();
    let rom_present = || rom::find_rom_zip().is_some();

    let mut discord_user: Option<String> = matchmaking::username_from_cached_token();
    let mut discord_id: Option<String> = matchmaking::discord_id_from_cached_token();
    let mut score_tracker = score::ScoreTracker::new();

    let mut core: Option<retro::Core> = None;
    let mut audio_queue: Option<AudioQueue<i16>> = None;
    let mut lab_save_slot: Option<Vec<u8>> = None;
    let mut rewind_test: Option<replay::RewindTest> = None;
    let mut input_history = input_history::InputHistory::new();
    let mut lab_assist_visible = true;
    let mut ghost_recording: Option<ghost::Recording> = None;
    let mut ghost_playback: Option<ghost::Playback> = None;
    let mut match_replay_recording: Option<match_replay::Recording> = None;
    let mut match_replay_playback: Option<match_replay::Playback> = None;
    let mut clip_recorder: Option<clip::ClipRecorder> = None;
    let mut ghost_port_mask: u8 = 0b11;
    let mut drone_runner: Option<drone::DroneRunner> = None;
    let ghost_path = std::path::Path::new("ghost.bin").to_path_buf();
    const GHOST_CAP_PER_PEER: u32 = 3;
    let mut ghost_library = ghost::Library::load_default();
    let mut net_recording: Option<ghost::NetRecording> = None;
    // MK2 `f_colbox` lives at 0x22576c in the 68000 map, which is 0x2576c
    // in FBNeo SYSTEM_RAM. Poking this before retro_run enables the boxes
    // for the frame being drawn.
    const HITBOX_FLAG_ADDR: usize = 0x2576C;

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
            addr: HITBOX_FLAG_ADDR,
            value: 0x0001,
            endian: memory::Endian::Little,
        },
        memory::Poke::U16 {
            addr: HITBOX_FLAG_ADDR,
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
    let mut ranked_match_index: u32 = 0;
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
    const MATCH_WIN_TARGET: u16 = 2;
    let mut session_p1_wins: u32 = 0;
    let mut session_p2_wins: u32 = 0;
    const P1_HP_ADDR: usize = 0x253D6;
    const P2_HP_ADDR: usize = 0x25550;
    let mut ghost_in_fight: bool = false;

    let mut net_session: Option<netplay::Session> = None;
    let mut local_handle: usize = 0;
    let mut relay_chat: Option<relay_socket::RelayChatHandle> = None;
    let mut chat_open = false;
    let mut chat_draft = String::new();
    let mut chat_lines: Vec<String> = Vec::new();

    let mut mm_rx: Option<std::sync::mpsc::Receiver<matchmaking::Update>> = None;
    let mut username_check_rx: Option<std::sync::mpsc::Receiver<matchmaking::UsernameCheckUpdate>> =
        None;
    let mut mm_session_id: Option<String> = None;
    let mut rpc_client = if cfg.discord_rpc_enabled {
        rpc::RpcClient::init()
    } else {
        None
    };
    if let Some(ref mut rc) = rpc_client {
        rc.update(rpc::RpcUpdate::default());
    }
    let mut spar_room_id: Option<String> = None;
    let mut peer_name: Option<String> = None;
    let mut profile_rx: Option<std::sync::mpsc::Receiver<matchmaking::ProfileUpdate>> = None;
    let mut leaderboard_rx: Option<std::sync::mpsc::Receiver<matchmaking::LeaderboardUpdate>> =
        None;
    let mut main_leaderboard = if cfg.stats_url.is_empty() {
        menu::LeaderboardState::Error("stats_url not configured".into())
    } else {
        let (tx, rx) = std::sync::mpsc::channel();
        leaderboard_rx = Some(rx);
        matchmaking::fetch_leaderboard(cfg.stats_url.clone(), tx);
        menu::LeaderboardState::Loading
    };
    let mut avatar_rx: Option<std::sync::mpsc::Receiver<Vec<u8>>> = None;
    let mut ghost_list_rx: Option<std::sync::mpsc::Receiver<matchmaking::GhostListUpdate>> = None;
    let mut ghost_download_rx: Option<std::sync::mpsc::Receiver<matchmaking::GhostDownloadUpdate>> =
        None;
    let mut spectate_rx: Option<std::sync::mpsc::Receiver<matchmaking::SpectateUpdate>> = None;
    let mut spectate_last_update: Option<Instant> = None;
    let mut live_matches_rx: Option<std::sync::mpsc::Receiver<matchmaking::LiveMatchesUpdate>> =
        None;
    let mut live_matches_next_refresh = Instant::now();
    let mut toast: Option<(String, Instant)> = None;
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

        if chat_open
            || matches!(
                state,
                AppState::Menu(menu::MenuScreen::TestIp { editing: true, .. })
                    | AppState::Menu(menu::MenuScreen::TextEdit { .. })
                    | AppState::Menu(menu::MenuScreen::MatchUsername {
                        checking: false,
                        ..
                    })
            )
        {
            video_subsystem.text_input().start();
        } else {
            video_subsystem.text_input().stop();
        }

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
        if let Some(session_id) = rpc::take_spectate_request() {
            println!("[main] Spectate request received: session_id={session_id}");
            let (tx, rx) = std::sync::mpsc::channel();
            spectate_rx = Some(rx);
            spectate_last_update = Some(Instant::now());
            matchmaking::watch_spectate_state(session_id.clone(), tx);
            state = AppState::Menu(MenuScreen::Spectate {
                session_id,
                status: menu::SpectateStatus::waiting(),
            });
        }

        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'running,

                Event::ControllerDeviceAdded { which, .. } => {
                    match controller_subsystem.open(which) {
                        Ok(c) => {
                            let name = c.name();
                            assign_pad(&mut pads, c);
                            toast = Some((
                                format!("Controller connected: {name}"),
                                Instant::now() + Duration::from_millis(2200),
                            ));
                        }
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
                                toast = Some((
                                    format!("P{} controller disconnected", i + 1),
                                    Instant::now() + Duration::from_millis(2200),
                                ));
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
                    toast = Some((
                        "Bindings reset to defaults".into(),
                        Instant::now() + Duration::from_millis(2200),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::R),
                    repeat: false,
                    ..
                } if matches!(state, AppState::Menu(MenuScreen::LiveMatches { .. })) => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    live_matches_rx = Some(rx);
                    matchmaking::fetch_live_matches(tx);
                    live_matches_next_refresh = Instant::now() + Duration::from_secs(7);
                    if let AppState::Menu(MenuScreen::LiveMatches { ref mut status, .. }) = state {
                        *status = "Refreshing active matches...".into();
                    }
                    toast = Some((
                        "Refreshing active matches".into(),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::R),
                    repeat: false,
                    ..
                } if matches!(state, AppState::Menu(MenuScreen::Leaderboard { .. })) => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    leaderboard_rx = Some(rx);
                    matchmaking::fetch_leaderboard(cfg.stats_url.clone(), tx);
                    main_leaderboard = menu::LeaderboardState::Loading;
                    if let AppState::Menu(MenuScreen::Leaderboard { ref mut state }) = state {
                        *state = menu::LeaderboardState::Loading;
                    }
                    toast = Some((
                        "Refreshing leaderboard".into(),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::C),
                    repeat: false,
                    ..
                } if matches!(state, AppState::Menu(MenuScreen::Spectate { .. })) => {
                    if let AppState::Menu(MenuScreen::Spectate {
                        ref session_id,
                        ref mut status,
                    }) = state
                    {
                        let link = format!("xband://watch/{session_id}");
                        match video_subsystem.clipboard().set_clipboard_text(&link) {
                            Ok(()) => {
                                status.message = "Watch link copied to clipboard.".into();
                                toast = Some((
                                    "Watch link copied".into(),
                                    Instant::now() + Duration::from_millis(2200),
                                ));
                            }
                            Err(e) => {
                                status.message = format!("Copy failed: {e}");
                                toast = Some((
                                    "Copy failed".into(),
                                    Instant::now() + Duration::from_millis(2200),
                                ));
                            }
                        }
                    }
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Left),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 5, .. })
                ) =>
                {
                    cfg.volume_percent = cfg.volume_percent.saturating_sub(10);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut volume_percent,
                        ..
                    }) = state
                    {
                        *volume_percent = cfg.volume_percent;
                    }
                    toast = Some((
                        format!("Volume {}%", cfg.volume_percent),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Right),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 5, .. })
                ) =>
                {
                    cfg.volume_percent = cfg.volume_percent.saturating_add(10).min(100);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut volume_percent,
                        ..
                    }) = state
                    {
                        *volume_percent = cfg.volume_percent;
                    }
                    toast = Some((
                        format!("Volume {}%", cfg.volume_percent),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Left),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 6, .. })
                ) =>
                {
                    cfg.audio_buffer = cfg.audio_buffer.cycle(-1);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut audio_buffer,
                        ..
                    }) = state
                    {
                        *audio_buffer = cfg.audio_buffer;
                    }
                    toast = Some((
                        format!("Audio Buffer {}", cfg.audio_buffer.label()),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Right),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 6, .. })
                ) =>
                {
                    cfg.audio_buffer = cfg.audio_buffer.cycle(1);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut audio_buffer,
                        ..
                    }) = state
                    {
                        *audio_buffer = cfg.audio_buffer;
                    }
                    toast = Some((
                        format!("Audio Buffer {}", cfg.audio_buffer.label()),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Left),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 7, .. })
                ) =>
                {
                    cfg.video_filter = cfg.video_filter.cycle(-1);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut video_filter,
                        ..
                    }) = state
                    {
                        *video_filter = cfg.video_filter;
                    }
                    toast = Some((
                        format!("Video Filter {}", cfg.video_filter.label()),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Right),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 7, .. })
                ) =>
                {
                    cfg.video_filter = cfg.video_filter.cycle(1);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut video_filter,
                        ..
                    }) = state
                    {
                        *video_filter = cfg.video_filter;
                    }
                    toast = Some((
                        format!("Video Filter {}", cfg.video_filter.label()),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Left),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 9, .. })
                ) =>
                {
                    cfg.aspect_mode = cfg.aspect_mode.cycle(-1);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut aspect_mode,
                        ..
                    }) = state
                    {
                        *aspect_mode = cfg.aspect_mode;
                    }
                    toast = Some((
                        format!("Aspect {}", cfg.aspect_mode.label()),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Right),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 9, .. })
                ) =>
                {
                    cfg.aspect_mode = cfg.aspect_mode.cycle(1);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut aspect_mode,
                        ..
                    }) = state
                    {
                        *aspect_mode = cfg.aspect_mode;
                    }
                    toast = Some((
                        format!("Aspect {}", cfg.aspect_mode.label()),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Left),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 10, .. })
                ) =>
                {
                    cfg.scorebar_style = cfg.scorebar_style.cycle(-1);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut scorebar_style,
                        ..
                    }) = state
                    {
                        *scorebar_style = cfg.scorebar_style;
                    }
                    toast = Some((
                        format!("Scorebar {}", cfg.scorebar_style.label()),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Right),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 10, .. })
                ) =>
                {
                    cfg.scorebar_style = cfg.scorebar_style.cycle(1);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut scorebar_style,
                        ..
                    }) = state
                    {
                        *scorebar_style = cfg.scorebar_style;
                    }
                    toast = Some((
                        format!("Scorebar {}", cfg.scorebar_style.label()),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::F),
                    keymod,
                    repeat: false,
                    ..
                } if state == AppState::Playing
                    && keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) =>
                {
                    cfg.video_filter = cfg.video_filter.cycle(1);
                    config::save(&cfg);
                    toast = Some((
                        format!("Video Filter {}", cfg.video_filter.label()),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::A),
                    keymod,
                    repeat: false,
                    ..
                } if state == AppState::Playing
                    && keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) =>
                {
                    cfg.aspect_mode = cfg.aspect_mode.cycle(1);
                    config::save(&cfg);
                    toast = Some((
                        format!("Aspect {}", cfg.aspect_mode.label()),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::R),
                    keymod,
                    repeat: false,
                    ..
                } if state == AppState::Playing
                    && keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) =>
                {
                    if let Some(recorder) = clip_recorder.take() {
                        let message = finish_clip_recording(recorder);
                        println!("[clip] {message}");
                        toast = Some((message, Instant::now() + Duration::from_millis(3200)));
                    } else {
                        let sample_rate = audio_queue
                            .as_ref()
                            .map(|q| q.spec().freq.max(1) as u32)
                            .unwrap_or(48_000);
                        match clip::ClipRecorder::start(sample_rate) {
                            Ok(recorder) => {
                                clip_recorder = Some(recorder);
                                toast = Some((
                                    "Recording clip... Ctrl+R to stop".into(),
                                    Instant::now() + Duration::from_millis(2200),
                                ));
                            }
                            Err(e) => {
                                toast = Some((
                                    format!("Clip start failed: {e}"),
                                    Instant::now() + Duration::from_millis(3200),
                                ));
                            }
                        }
                    }
                }

                Event::TextInput { text, .. }
                    if state == AppState::Playing && net_session.is_some() && chat_open =>
                {
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        if chat_draft.chars().count() < 180 {
                            chat_draft.push(ch);
                        }
                    }
                }

                Event::TextInput { text, .. }
                    if matches!(
                        state,
                        AppState::Menu(menu::MenuScreen::TestIp { editing: true, .. })
                            | AppState::Menu(menu::MenuScreen::TextEdit { .. })
                            | AppState::Menu(menu::MenuScreen::MatchUsername {
                                checking: false,
                                ..
                            })
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
                        | AppState::Menu(menu::MenuScreen::TextEdit { .. })
                        | AppState::Menu(menu::MenuScreen::MatchUsername {
                            checking: false,
                            ..
                        })
                ) =>
                {
                    state.text_backspace();
                }

                _ if matches!(state, AppState::Menu(MenuScreen::Matchmaking { .. })) => {
                    if is_cancel(&event) {
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

                // Launch diagnostics on demand from menu screens.
                Event::KeyDown {
                    keycode: Some(Keycode::D),
                    keymod,
                    repeat: false,
                    ..
                } if matches!(state, AppState::Menu(_))
                    && keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD) =>
                {
                    if let Err(e) = launch_debugger() {
                        println!("[doctor] failed to launch: {e}");
                    }
                }

                _ if matches!(state, AppState::Menu(MenuScreen::About))
                    && matches!(event_to_menu_nav(&event), Some(MenuNav::Accept)) =>
                {
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
                                    input_history.clear();
                                    match_replay_playback = None;
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
                                                    let (p1, p2) = replay_names(
                                                        local_handle,
                                                        discord_user.as_deref(),
                                                        peer_name.as_deref(),
                                                    );
                                                    match_replay_recording =
                                                        Some(match_replay::Recording::new(p1, p2));
                                                    net_log = open_net_log();
                                                }
                                                Err(e) => {
                                                    println!("[net] session start failed: {e}")
                                                }
                                            }
                                        }
                                    }
                                }
                                NavResult::OpenUsernameEntry => {
                                    let value = config::sanitize_username(&cfg.player_username)
                                        .unwrap_or_else(config::default_username);
                                    state = AppState::Menu(MenuScreen::MatchUsername {
                                        value,
                                        status: "Choose a public player name".into(),
                                        checking: false,
                                    });
                                }
                                NavResult::SubmitUsername(value) => {
                                    match config::sanitize_username(&value) {
                                        Some(username) => {
                                            let (tx, rx) = std::sync::mpsc::channel();
                                            username_check_rx = Some(rx);
                                            matchmaking::check_username_available(
                                                cfg.stats_url.clone(),
                                                username.clone(),
                                                tx,
                                            );
                                            state = AppState::Menu(MenuScreen::MatchUsername {
                                                value: username,
                                                status: "Checking username".into(),
                                                checking: true,
                                            });
                                        }
                                        None => {
                                            state = AppState::Menu(MenuScreen::MatchUsername {
                                                value,
                                                status:
                                                    "Invalid name: use 2-24 letters, numbers, _ or -"
                                                        .into(),
                                                checking: false,
                                            });
                                        }
                                    }
                                }
                                NavResult::StartMatchmaking => {
                                    cfg.player_username =
                                        config::sanitize_username(&cfg.player_username)
                                            .unwrap_or_else(config::default_username);
                                    config::save(&cfg);
                                    matchmaking::set_guest_profile(
                                        cfg.player_username.clone(),
                                        cfg.stats_email.clone(),
                                    );
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    mm_rx = Some(rx);
                                    matchmaking::start_guest(tx);
                                }
                                NavResult::OpenGhostSelect => {
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    ghost_list_rx = Some(rx);
                                    if let AppState::Menu(menu::MenuScreen::GhostSelect {
                                        ref mut download_status,
                                        ..
                                    }) = state
                                    {
                                        if cfg.stats_url.trim().is_empty() {
                                            *download_status = None;
                                        } else {
                                            *download_status =
                                                Some("Loading shared ghosts...".into());
                                        }
                                    }
                                    let rh = rom_fingerprint().1;
                                    let rom_hash = format!("{:016x}", rh);
                                    matchmaking::fetch_ghost_list(
                                        cfg.stats_url.clone(),
                                        rom_hash,
                                        tx,
                                    );
                                }
                                NavResult::OpenReplaySelect => {
                                    refresh_replay_select(&mut state, None);
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
                                NavResult::OpenLiveMatches => {
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    live_matches_rx = Some(rx);
                                    matchmaking::fetch_live_matches(tx);
                                    live_matches_next_refresh =
                                        Instant::now() + Duration::from_secs(7);
                                }
                                NavResult::OpenLeaderboard => {
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    leaderboard_rx = Some(rx);
                                    matchmaking::fetch_leaderboard(cfg.stats_url.clone(), tx);
                                }
                                NavResult::OpenSettings => {
                                    state = AppState::Menu(MenuScreen::Settings {
                                        cursor: 0,
                                        player_username: cfg.player_username.clone(),
                                        stats_email: cfg.stats_email.clone(),
                                        discord_connected:
                                            matchmaking::connected_discord_user_from_cached_token()
                                                .is_some(),
                                        discord_rpc_enabled: cfg.discord_rpc_enabled,
                                        fullscreen: cfg.fullscreen,
                                        volume_percent: cfg.volume_percent,
                                        audio_buffer: cfg.audio_buffer,
                                        video_filter: cfg.video_filter,
                                        crt_corner_bend: cfg.crt_corner_bend,
                                        aspect_mode: cfg.aspect_mode,
                                        scorebar_style: cfg.scorebar_style,
                                    });
                                }
                                NavResult::OpenTraining => {
                                    state = AppState::Menu(MenuScreen::Training {
                                        cursor: 0,
                                        hitboxes: trainer.is_enabled("hitboxes"),
                                        infinite_health: trainer.is_enabled("p1_health"),
                                        freeze_timer: trainer.is_enabled("freeze_timer"),
                                    });
                                }
                                NavResult::WatchSession(session_id) => {
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    spectate_rx = Some(rx);
                                    spectate_last_update = Some(Instant::now());
                                    matchmaking::watch_spectate_state(session_id, tx);
                                }
                                NavResult::SignOut => {
                                    matchmaking::clear_cached_token();
                                    discord_user = None;
                                    discord_id = None;
                                    println!("[auth] Signed out");
                                }
                                NavResult::EditText(field, title) => {
                                    let value = match field {
                                        menu::EditField::Username => cfg.player_username.clone(),
                                        menu::EditField::StatsEmail => cfg.stats_email.clone(),
                                    };
                                    let label = match field {
                                        menu::EditField::Username => {
                                            "Choose the name other players see"
                                        }
                                        menu::EditField::StatsEmail => {
                                            "Optional email for portable stats"
                                        }
                                    };
                                    state = AppState::Menu(MenuScreen::TextEdit {
                                        title,
                                        label: label.into(),
                                        value,
                                        field,
                                        came_from: Box::new(MenuScreen::Settings {
                                            cursor: match field {
                                                menu::EditField::Username => 0,
                                                menu::EditField::StatsEmail => 1,
                                            },
                                            player_username: cfg.player_username.clone(),
                                            stats_email: cfg.stats_email.clone(),
                                            discord_connected: matchmaking::connected_discord_user_from_cached_token()
                                                .is_some(),
                                            discord_rpc_enabled: cfg.discord_rpc_enabled,
                                            fullscreen: cfg.fullscreen,
                                            volume_percent: cfg.volume_percent,
                                            audio_buffer: cfg.audio_buffer,
                                            video_filter: cfg.video_filter,
                                            crt_corner_bend: cfg.crt_corner_bend,
                                            aspect_mode: cfg.aspect_mode,
                                            scorebar_style: cfg.scorebar_style,
                                        }),
                                    });
                                }
                                NavResult::CommitText(field, value) => {
                                    match field {
                                        menu::EditField::Username => {
                                            cfg.player_username = config::sanitize_username(&value)
                                                .unwrap_or_else(config::default_username);
                                            toast = Some((
                                                format!("Username {}", cfg.player_username),
                                                Instant::now() + Duration::from_millis(2200),
                                            ));
                                        }
                                        menu::EditField::StatsEmail => {
                                            let trimmed = value.trim();
                                            if trimmed.is_empty() {
                                                cfg.stats_email.clear();
                                                toast = Some((
                                                    "Stats email cleared".into(),
                                                    Instant::now() + Duration::from_millis(2200),
                                                ));
                                            } else if let Some(email) =
                                                config::normalize_email(trimmed)
                                            {
                                                cfg.stats_email = email;
                                                toast = Some((
                                                    "Stats email saved".into(),
                                                    Instant::now() + Duration::from_millis(2200),
                                                ));
                                            } else {
                                                toast = Some((
                                                    "Enter a valid email address".into(),
                                                    Instant::now() + Duration::from_millis(2600),
                                                ));
                                            }
                                        }
                                    }
                                    config::save(&cfg);
                                    matchmaking::clear_cached_token();
                                    state = AppState::Menu(MenuScreen::Settings {
                                        cursor: match field {
                                            menu::EditField::Username => 0,
                                            menu::EditField::StatsEmail => 1,
                                        },
                                        player_username: cfg.player_username.clone(),
                                        stats_email: cfg.stats_email.clone(),
                                        discord_connected:
                                            matchmaking::connected_discord_user_from_cached_token()
                                                .is_some(),
                                        discord_rpc_enabled: cfg.discord_rpc_enabled,
                                        fullscreen: cfg.fullscreen,
                                        volume_percent: cfg.volume_percent,
                                        audio_buffer: cfg.audio_buffer,
                                        video_filter: cfg.video_filter,
                                        crt_corner_bend: cfg.crt_corner_bend,
                                        aspect_mode: cfg.aspect_mode,
                                        scorebar_style: cfg.scorebar_style,
                                    });
                                }
                                NavResult::ConnectDiscord => {
                                    if matchmaking::connected_discord_user_from_cached_token()
                                        .is_some()
                                    {
                                        matchmaking::clear_cached_token();
                                        discord_user = None;
                                        discord_id = None;
                                        toast = Some((
                                            "Discord disconnected".into(),
                                            Instant::now() + Duration::from_millis(2200),
                                        ));
                                        state = AppState::Menu(MenuScreen::Settings {
                                            cursor: 2,
                                            player_username: cfg.player_username.clone(),
                                            stats_email: cfg.stats_email.clone(),
                                            discord_connected: false,
                                            discord_rpc_enabled: cfg.discord_rpc_enabled,
                                            fullscreen: cfg.fullscreen,
                                            volume_percent: cfg.volume_percent,
                                            audio_buffer: cfg.audio_buffer,
                                            video_filter: cfg.video_filter,
                                            crt_corner_bend: cfg.crt_corner_bend,
                                            aspect_mode: cfg.aspect_mode,
                                            scorebar_style: cfg.scorebar_style,
                                        });
                                    } else {
                                        let (tx, rx) = std::sync::mpsc::channel();
                                        mm_rx = Some(rx);
                                        matchmaking::start_discord_connect(tx);
                                        state = AppState::Menu(MenuScreen::Matchmaking {
                                            status: "Opening Discord login...".into(),
                                        });
                                    }
                                }
                                NavResult::ToggleDiscordRpc => {
                                    cfg.discord_rpc_enabled = !cfg.discord_rpc_enabled;
                                    config::save(&cfg);
                                    if cfg.discord_rpc_enabled {
                                        crate::rpc::set_discord_client_id(
                                            cfg.discord_client_id.clone(),
                                        );
                                        rpc_client = rpc::RpcClient::init();
                                    } else {
                                        rpc_client = None;
                                    }
                                    if let AppState::Menu(MenuScreen::Settings {
                                        ref mut discord_rpc_enabled,
                                        ..
                                    }) = state
                                    {
                                        *discord_rpc_enabled = cfg.discord_rpc_enabled;
                                    }
                                    toast = Some((
                                        format!(
                                            "Discord Rich Presence {}",
                                            if cfg.discord_rpc_enabled {
                                                "enabled"
                                            } else {
                                                "disabled"
                                            }
                                        ),
                                        Instant::now() + Duration::from_millis(2200),
                                    ));
                                }
                                NavResult::ToggleFullscreen => {
                                    cfg.fullscreen = !cfg.fullscreen;
                                    config::save(&cfg);
                                    let result = if cfg.fullscreen {
                                        canvas.window_mut().set_fullscreen(FullscreenType::Desktop)
                                    } else {
                                        canvas.window_mut().set_fullscreen(FullscreenType::Off)
                                    };
                                    if let AppState::Menu(MenuScreen::Settings {
                                        ref mut fullscreen,
                                        ..
                                    }) = state
                                    {
                                        *fullscreen = cfg.fullscreen;
                                    }
                                    toast = Some((
                                        match result {
                                            Ok(()) => format!(
                                                "Fullscreen {}",
                                                if cfg.fullscreen {
                                                    "enabled"
                                                } else {
                                                    "disabled"
                                                }
                                            ),
                                            Err(e) => format!("Fullscreen failed: {e}"),
                                        },
                                        Instant::now() + Duration::from_millis(2200),
                                    ));
                                }
                                NavResult::AdjustVolume(delta) => {
                                    if delta < 0 {
                                        cfg.volume_percent =
                                            cfg.volume_percent.saturating_sub(delta.unsigned_abs());
                                    } else {
                                        cfg.volume_percent =
                                            cfg.volume_percent.saturating_add(delta as u8).min(100);
                                    }
                                    config::save(&cfg);
                                    if let AppState::Menu(MenuScreen::Settings {
                                        ref mut volume_percent,
                                        ..
                                    }) = state
                                    {
                                        *volume_percent = cfg.volume_percent;
                                    }
                                    toast = Some((
                                        format!("Volume {}%", cfg.volume_percent),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                NavResult::CycleAudioBuffer(delta) => {
                                    cfg.audio_buffer = cfg.audio_buffer.cycle(delta);
                                    config::save(&cfg);
                                    if let AppState::Menu(MenuScreen::Settings {
                                        ref mut audio_buffer,
                                        ..
                                    }) = state
                                    {
                                        *audio_buffer = cfg.audio_buffer;
                                    }
                                    toast = Some((
                                        format!("Audio Buffer {}", cfg.audio_buffer.label()),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                NavResult::CycleVideoFilter(delta) => {
                                    cfg.video_filter = cfg.video_filter.cycle(delta);
                                    config::save(&cfg);
                                    if let AppState::Menu(MenuScreen::Settings {
                                        ref mut video_filter,
                                        ..
                                    }) = state
                                    {
                                        *video_filter = cfg.video_filter;
                                    }
                                    toast = Some((
                                        format!("Video Filter {}", cfg.video_filter.label()),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                NavResult::ToggleCrtGlass => {
                                    cfg.crt_corner_bend = !cfg.crt_corner_bend;
                                    config::save(&cfg);
                                    if let AppState::Menu(MenuScreen::Settings {
                                        ref mut crt_corner_bend,
                                        ..
                                    }) = state
                                    {
                                        *crt_corner_bend = cfg.crt_corner_bend;
                                    }
                                    toast = Some((
                                        format!(
                                            "CRT Glass {}",
                                            if cfg.crt_corner_bend { "ON" } else { "OFF" }
                                        ),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                NavResult::CycleAspectMode(delta) => {
                                    cfg.aspect_mode = cfg.aspect_mode.cycle(delta);
                                    config::save(&cfg);
                                    if let AppState::Menu(MenuScreen::Settings {
                                        ref mut aspect_mode,
                                        ..
                                    }) = state
                                    {
                                        *aspect_mode = cfg.aspect_mode;
                                    }
                                    toast = Some((
                                        format!("Aspect {}", cfg.aspect_mode.label()),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                NavResult::CycleScorebarStyle(delta) => {
                                    cfg.scorebar_style = cfg.scorebar_style.cycle(delta);
                                    config::save(&cfg);
                                    if let AppState::Menu(MenuScreen::Settings {
                                        ref mut scorebar_style,
                                        ..
                                    }) = state
                                    {
                                        *scorebar_style = cfg.scorebar_style;
                                    }
                                    toast = Some((
                                        format!("Scorebar {}", cfg.scorebar_style.label()),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                NavResult::ToggleTraining(kind) => {
                                    match kind {
                                        "hitboxes" => {
                                            let on = !trainer.is_enabled("hitboxes");
                                            trainer.set_enabled("hitboxes", on);
                                        }
                                        "health" => {
                                            let on = !trainer.is_enabled("p1_health");
                                            trainer.set_enabled("p1_health", on);
                                            trainer.set_enabled("p2_health", on);
                                        }
                                        "timer" => {
                                            let on = !trainer.is_enabled("freeze_timer");
                                            trainer.set_enabled("freeze_timer", on);
                                        }
                                        _ => {}
                                    }
                                    if let AppState::Menu(MenuScreen::Training {
                                        ref mut hitboxes,
                                        ref mut infinite_health,
                                        ref mut freeze_timer,
                                        ..
                                    }) = state
                                    {
                                        *hitboxes = trainer.is_enabled("hitboxes");
                                        *infinite_health = trainer.is_enabled("p1_health");
                                        *freeze_timer = trainer.is_enabled("freeze_timer");
                                    }
                                    let label = match kind {
                                        "hitboxes" => "Hitbox view",
                                        "health" => "Infinite health",
                                        "timer" => "Freeze timer",
                                        _ => "Training helper",
                                    };
                                    let on = match kind {
                                        "hitboxes" => trainer.is_enabled("hitboxes"),
                                        "health" => trainer.is_enabled("p1_health"),
                                        "timer" => trainer.is_enabled("freeze_timer"),
                                        _ => false,
                                    };
                                    toast = Some((
                                        format!("{label} {}", if on { "ON" } else { "OFF" }),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                NavResult::LaunchDoctor => match launch_debugger() {
                                    Ok(()) => {
                                        toast = Some((
                                            "Doctor launched".into(),
                                            Instant::now() + Duration::from_millis(2200),
                                        ));
                                    }
                                    Err(e) => {
                                        toast = Some((
                                            format!("Doctor failed: {e}"),
                                            Instant::now() + Duration::from_millis(2600),
                                        ));
                                    }
                                },
                                NavResult::OpenClipsFolder => {
                                    let target = std::env::current_dir()
                                        .unwrap_or_else(|_| std::path::PathBuf::from("."))
                                        .join("clips");
                                    let _ = std::fs::create_dir_all(&target);
                                    match open::that(&target) {
                                        Ok(()) => {
                                            toast = Some((
                                                "Clips folder opened".into(),
                                                Instant::now() + Duration::from_millis(2200),
                                            ));
                                        }
                                        Err(e) => {
                                            toast = Some((
                                                format!("Open failed: {e}"),
                                                Instant::now() + Duration::from_millis(2600),
                                            ));
                                        }
                                    }
                                }
                                NavResult::OpenLogsFolder => {
                                    let target = std::env::current_dir()
                                        .unwrap_or_else(|_| std::path::PathBuf::from("."));
                                    match open::that(&target) {
                                        Ok(()) => {
                                            toast = Some((
                                                "Logs folder opened".into(),
                                                Instant::now() + Duration::from_millis(2200),
                                            ));
                                        }
                                        Err(e) => {
                                            toast = Some((
                                                format!("Open failed: {e}"),
                                                Instant::now() + Duration::from_millis(2600),
                                            ));
                                        }
                                    }
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
                                    toast = Some((
                                        format!("Cleared bindings for {}", player.label()),
                                        Instant::now() + Duration::from_millis(2200),
                                    ));
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
                                                        "[ghost] Loaded logic opponent: {} frames",
                                                        pb.frame_count()
                                                    );
                                                    start_logic_ghost_opponent(
                                                        pb,
                                                        &mut ghost_port_mask,
                                                        &mut ghost_playback,
                                                        &mut drone_runner,
                                                    );
                                                    match_replay_playback = None;
                                                    input::clear_all_inputs();
                                                    auto_start_done = false;
                                                    auto_start_frame = 0;
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
                                NavResult::LoadReplay(path) => {
                                    ensure_core_loaded(
                                        &mut core,
                                        &mut audio_queue,
                                        &audio_subsystem,
                                    )?;
                                    if let Some(c) = &core {
                                        match match_replay::Playback::load(&path) {
                                            Ok(pb) => {
                                                if pb.prime(c) {
                                                    println!(
                                                        "[replay] Playing {} frames: {} vs {}",
                                                        pb.frame_count(),
                                                        pb.p1_name(),
                                                        pb.p2_name()
                                                    );
                                                    match_replay_playback = Some(pb);
                                                    ghost_playback = None;
                                                    ghost_recording = None;
                                                    drone_runner = None;
                                                    input_history.clear();
                                                    input::clear_all_inputs();
                                                    state = AppState::Playing;
                                                } else {
                                                    println!(
                                                        "[replay] Anchor state rejected by core."
                                                    );
                                                    refresh_replay_select(
                                                        &mut state,
                                                        Some("Error: replay state rejected".into()),
                                                    );
                                                }
                                            }
                                            Err(e) => {
                                                println!("[replay] Load failed: {e}");
                                                refresh_replay_select(
                                                    &mut state,
                                                    Some(format!("Error: replay load failed: {e}")),
                                                );
                                            }
                                        }
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
                        keycode: Some(Keycode::Escape),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        match_replay_playback = None;
                        input::clear_all_inputs();
                        state = AppState::Menu(MenuScreen::ReplaySelect {
                            cursor: 0,
                            entries: vec![],
                            status: Some("Replay stopped".into()),
                        });
                        refresh_replay_select(&mut state, Some("Replay stopped".into()));
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::T),
                        repeat: false,
                        ..
                    } if net_session.is_some() && !chat_open => {
                        input::clear_all_inputs();
                        chat_open = true;
                        chat_draft.clear();
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Backspace),
                        repeat: false,
                        ..
                    } if chat_open => {
                        chat_draft.pop();
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Escape),
                        repeat: false,
                        ..
                    } if chat_open => {
                        close_chat(&mut chat_open, &mut chat_draft);
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Return),
                        repeat: false,
                        ..
                    } if chat_open => {
                        send_chat_draft(
                            relay_chat.as_ref(),
                            &mut chat_lines,
                            discord_user.as_deref(),
                            &chat_draft,
                        );
                        close_chat(&mut chat_open, &mut chat_draft);
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::Start,
                        ..
                    } if chat_open => {
                        send_chat_draft(
                            relay_chat.as_ref(),
                            &mut chat_lines,
                            discord_user.as_deref(),
                            &chat_draft,
                        );
                        close_chat(&mut chat_open, &mut chat_draft);
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::B | sdl2::controller::Button::Back,
                        ..
                    } if chat_open => {
                        close_chat(&mut chat_open, &mut chat_draft);
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F1),
                        repeat: false,
                        ..
                    } => {
                        if net_session.is_some() {
                            net_teardown_reason = Some("you quit the match".into());
                        } else {
                            toggle_hitbox_view(&mut trainer, &mut toast);
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Escape),
                        ..
                    } if net_session.is_none() && match_replay_playback.is_none() => {
                        input::clear_all_inputs();
                        state = AppState::Menu(MenuScreen::Main { cursor: 0 });
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F9),
                        repeat: false,
                        ..
                    } if net_session.is_none() && match_replay_playback.is_none() => {
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
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && ghost_playback.is_some() =>
                    {
                        if drone_runner.is_some() {
                            drone_runner = None;
                            println!("[drone] Disabled — sequential ghost playback");
                        } else {
                            let target_port = ghost_target_port(ghost_port_mask);
                            let index = drone::DroneIndex::build(
                                ghost_playback.as_ref().unwrap(),
                                target_port,
                            );
                            drone_runner = Some(drone::DroneRunner::new(index, target_port));
                            println!("[drone] Enabled — posture-reactive playback");
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F2),
                        repeat: false,
                        ..
                    } if net_session.is_none() && match_replay_playback.is_none() => {
                        toggle_hitbox_view(&mut trainer, &mut toast);
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F3),
                        repeat: false,
                        ..
                    } if net_session.is_none() && match_replay_playback.is_none() => {
                        let on = !trainer.is_enabled("p1_health");
                        trainer.set_enabled("p1_health", on);
                        trainer.set_enabled("p2_health", on);
                        println!(
                            "[trainer] Infinite health: {}",
                            if on { "ON" } else { "OFF" }
                        );
                        toast = Some((
                            format!("Infinite health {}", if on { "ON" } else { "OFF" }),
                            Instant::now() + Duration::from_millis(1800),
                        ));
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F4),
                        repeat: false,
                        ..
                    } if net_session.is_none() && match_replay_playback.is_none() => {
                        let on = !trainer.is_enabled("freeze_timer");
                        trainer.set_enabled("freeze_timer", on);
                        println!("[trainer] Freeze timer: {}", if on { "ON" } else { "OFF" });
                        toast = Some((
                            format!("Freeze timer {}", if on { "ON" } else { "OFF" }),
                            Instant::now() + Duration::from_millis(1800),
                        ));
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F5),
                        repeat: false,
                        ..
                    } if net_session.is_none() && match_replay_playback.is_none() => {
                        if let Some(c) = &core {
                            match c.save_state() {
                                Some(blob) => {
                                    let bytes = blob.len();
                                    lab_save_slot = Some(blob);
                                    println!("[lab] Saved reset point ({bytes} bytes)");
                                    toast = Some((
                                        "Lab reset point saved".into(),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                None => {
                                    toast = Some((
                                        "Lab save failed".into(),
                                        Instant::now() + Duration::from_millis(2200),
                                    ));
                                }
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F7),
                        repeat: false,
                        ..
                    } if net_session.is_none() && match_replay_playback.is_none() => {
                        if let (Some(c), Some(slot)) = (&core, lab_save_slot.as_ref()) {
                            if c.load_state(slot) {
                                input::clear_all_inputs();
                                input_history.clear();
                                println!("[lab] Reset to saved point");
                                toast = Some((
                                    "Lab reset loaded".into(),
                                    Instant::now() + Duration::from_millis(1800),
                                ));
                            } else {
                                toast = Some((
                                    "Lab reset failed".into(),
                                    Instant::now() + Duration::from_millis(2200),
                                ));
                            }
                        } else {
                            toast = Some((
                                "No lab reset point saved".into(),
                                Instant::now() + Duration::from_millis(1800),
                            ));
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F6),
                        repeat: false,
                        ..
                    } => {
                        if match_replay_playback.is_some() {
                            println!("[replay] Ghost recording disabled during replay playback.");
                        } else if net_session.is_some() {
                            println!("[ghost] Recording disabled in netplay mode.");
                        } else if let Some(rec) = ghost_recording.take() {
                            match rec.save(&ghost_path) {
                                Ok(_) => {
                                    println!(
                                        "[ghost] Saved {} frames to {}",
                                        rec.frame_count(),
                                        ghost_path.display()
                                    );
                                    let _ = std::fs::create_dir_all("ghosts");
                                    let ts = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    let library_path = std::path::Path::new("ghosts")
                                        .join(format!("local_{ts}.ncgh"));
                                    match rec.save(&library_path) {
                                        Ok(()) => println!(
                                            "[ghost] Added local recording to {}",
                                            library_path.display()
                                        ),
                                        Err(e) => println!(
                                            "[ghost] Library save failed {}: {e}",
                                            library_path.display()
                                        ),
                                    }
                                }
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
                        if match_replay_playback.is_some() {
                            println!("[replay] Ghost playback disabled during replay playback.");
                        } else if net_session.is_some() {
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
                                        drone_runner = None;
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
                        if match_replay_playback.is_some() {
                            println!("[replay] Ghost opponent disabled during replay playback.");
                        } else if net_session.is_some() {
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
                                            "[ghost] Logic ghost loaded: {} frames, you are P1...",
                                            pb.frame_count()
                                        );
                                        start_logic_ghost_opponent(
                                            pb,
                                            &mut ghost_port_mask,
                                            &mut ghost_playback,
                                            &mut drone_runner,
                                        );
                                        match_replay_playback = None;
                                        input::clear_all_inputs();
                                        auto_start_done = false;
                                        auto_start_frame = 0;
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
                        keymod,
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD) =>
                    {
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
                        keycode: Some(Keycode::F11),
                        repeat: false,
                        ..
                    } if net_session.is_none() && match_replay_playback.is_none() => {
                        lab_assist_visible = !lab_assist_visible;
                        toast = Some((
                            format!(
                                "Lab assist {}",
                                if lab_assist_visible { "ON" } else { "OFF" }
                            ),
                            Instant::now() + Duration::from_millis(1800),
                        ));
                    }
                    Event::KeyDown {
                        keycode: Some(k),
                        repeat: false,
                        ..
                    } if !chat_open => {
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
                    } if !chat_open => {
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
                    Event::ControllerButtonDown { which, button, .. } if !chat_open => {
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
                    Event::ControllerButtonUp { which, button, .. } if !chat_open => {
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
                    } if !chat_open => {
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
                            let mut result_match_index = None;
                            if let score::ScoreEvent::MatchOver { winner, .. } = ev {
                                ranked_match_index = ranked_match_index.saturating_add(1);
                                result_match_index = Some(ranked_match_index);
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
                                result_match_index,
                            );
                        }

                        let pre_confirmed = sess.confirmed_frame();
                        let pre_ready = matches!(sess.current_state(), ggrs::SessionState::Running);

                        let step_stats = step_netplay_frame(
                            c,
                            sess,
                            local_handle,
                            &mut net_recording,
                            &mut match_replay_recording,
                            &mut net_log,
                            &mut net_runtime,
                        );
                        if let Some(chat) = relay_chat.as_ref() {
                            let who = peer_name.as_deref().unwrap_or("Opponent");
                            for msg in chat.drain() {
                                push_chat_line(&mut chat_lines, format!("{who}: {msg}"));
                            }
                        }

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
                        if net_frame_counter >= net_spectate_next {
                            net_spectate_next = net_frame_counter.wrapping_add(165);
                            if let Some(ref sid) = mm_session_id {
                                session::push_spectator_frame(
                                    sid,
                                    session_p1_wins.min(u16::MAX as u32) as u16,
                                    session_p2_wins.min(u16::MAX as u32) as u16,
                                    net_frame_counter,
                                );
                            }
                        }
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
                    } else if let Some(pb) = match_replay_playback.as_mut() {
                        if !pb.inject_next() {
                            println!("[replay] Playback complete ({} frames).", pb.frame_count());
                            match_replay_playback = None;
                            input::clear_all_inputs();
                            state = AppState::Menu(MenuScreen::ReplaySelect {
                                cursor: 0,
                                entries: vec![],
                                status: Some("Replay complete".into()),
                            });
                            refresh_replay_select(&mut state, Some("Replay complete".into()));
                        } else {
                            unsafe {
                                (c.run)();
                            }
                        }
                    } else {
                        input::commit_live_to_state();
                        input_history.step(input::snapshot_player(Player::P1));
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
                                let gstate =
                                    memory::peek_u16(c, GSTATE_ADDR, memory::Endian::Little)
                                        .unwrap_or(0);
                                let p1_hp = memory::peek_u16(c, P1_HP_ADDR, memory::Endian::Little)
                                    .unwrap_or(0);
                                let fight_loaded =
                                    matches!(gstate, GS_FIGHTING | 0x03) && p1_hp > 0;
                                if fight_loaded {
                                    let gs = drone::GameState::read(c);
                                    let ghost_input = drone.next_input(&gs);
                                    let target_port = ghost_target_port(ghost_port_mask);
                                    unsafe {
                                        for b in 0..16 {
                                            INPUT_STATE[target_port][b] =
                                                (ghost_input >> b) & 1 != 0;
                                        }
                                    }
                                } else if !pb.inject_next(ghost_port_mask) {
                                    pb.rewind_inputs();
                                    let _ = pb.inject_next(ghost_port_mask);
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
                        trainer.apply(c);
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
                                    "Ghost Match Result - Player {} (P1 HP: 0x{p1_hp:04X} | P2 HP: 0x{p2_hp:04X})",
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
                    if matches!(state, AppState::Playing)
                        && net_session.is_none()
                        && match_replay_playback.is_none()
                    {
                        trainer.apply(c);
                    }
                    if let Some(q) = &audio_queue {
                        unsafe {
                            if !AUDIO_BUFFER.is_empty() {
                                if let Some(recorder) = clip_recorder.as_mut() {
                                    recorder.record_audio(&AUDIO_BUFFER);
                                }
                                queue_game_audio(
                                    q,
                                    &AUDIO_BUFFER,
                                    cfg.volume_percent,
                                    cfg.audio_buffer,
                                );
                                AUDIO_BUFFER.clear();
                            }
                        }
                    }
                    if let Some(recorder) = clip_recorder.as_mut() {
                        match recorder.record_frame() {
                            Ok(()) => {
                                if recorder.is_at_limit() {
                                    if let Some(done) = clip_recorder.take() {
                                        let message = finish_clip_recording(done);
                                        println!("[clip] {message}");
                                        toast = Some((
                                            message,
                                            Instant::now() + Duration::from_millis(3200),
                                        ));
                                    }
                                }
                            }
                            Err(e) => {
                                clip_recorder = None;
                                toast = Some((
                                    format!("Clip recording stopped: {e}"),
                                    Instant::now() + Duration::from_millis(3200),
                                ));
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
                        let intentional_quit = reason == "you quit the match";
                        let completed_set = reason.starts_with("match limit reached");

                        // Auto-incident on abnormal teardowns. We skip
                        // intentional_quit (user pressed back) and
                        // completed_set (clean BO3 finish) because those
                        // aren't failures. Everything else — disconnect,
                        // timeout, GGRS desync — gets uploaded.
                        if !intentional_quit && !completed_set {
                            let kind = if reason.contains("disconnected") {
                                incident::KIND_GGRS_DISCONNECTED
                            } else if reason.contains("timed out") || reason.contains("no progress")
                            {
                                if net_frame_counter < 60 {
                                    incident::KIND_GGRS_NEVER_SYNCED
                                } else {
                                    incident::KIND_MATCH_ENDED_EARLY
                                }
                            } else {
                                incident::KIND_MATCH_ENDED_EARLY
                            };
                            let mut inc = incident::Incident::new(kind, reason.clone());
                            inc.session_id = mm_session_id.clone();
                            inc.role = Some(if local_handle == 0 { "host" } else { "join" });
                            inc.frames_advanced = net_frame_counter;
                            inc.p1_score = Some(session_p1_wins as u16);
                            inc.p2_score = Some(session_p2_wins as u16);
                            inc.net_log_path = Some(std::path::PathBuf::from("freeplay-net.log"));
                            let (_size, hash) = rom_fingerprint();
                            inc.rom_hash = Some(format!("{:016x}", hash));
                            incident::submit(inc);
                        }
                        let player_role = if local_handle == 0 { "P1" } else { "P2" };
                        let opponent = peer_name.as_deref().unwrap_or("Opponent");
                        let final_score =
                            format!("Final score: P1 {session_p1_wins} - {session_p2_wins} P2");
                        let duration = format!(
                            "Duration: {} frames (~{:.1}s)",
                            net_frame_counter,
                            net_frame_counter as f32 / 55.0
                        );
                        let teardown_lines: Vec<String> = if completed_set {
                            vec![
                                "OK Set complete.".into(),
                                final_score,
                                format!("You were {player_role} vs {opponent}."),
                                duration,
                                format!("Matches completed: {}", net_match_count),
                                String::new(),
                                "Results and ghosts finalized where available.".into(),
                                "ENTER returns to the main menu.".into(),
                            ]
                        } else if intentional_quit {
                            vec![
                                "You left the match.".into(),
                                final_score,
                                format!("You were {player_role} vs {opponent}."),
                                duration,
                                format!("Matches completed: {}", net_match_count),
                                String::new(),
                                "Partial match data was saved where possible.".into(),
                                "ENTER returns to the main menu.".into(),
                            ]
                        } else {
                            let headline = if reason.contains("disconnected") {
                                "Your opponent disconnected."
                            } else if reason.contains("timed out") {
                                "The connection stopped responding."
                            } else {
                                "The online match ended early."
                            };
                            vec![
                                headline.into(),
                                String::new(),
                                format!("Details: {reason}"),
                                format!(
                                    "Session ran for {} frames (~{:.1}s)",
                                    net_frame_counter,
                                    net_frame_counter as f32 / 55.0
                                ),
                                format!("Matches completed: {}", net_match_count),
                                String::new(),
                                "WARN Match data was saved where possible.".into(),
                                "You can return to the menu and queue again.".into(),
                                "Log: freeplay-net.log".into(),
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
                        match_replay::finalize_recording(&mut match_replay_recording);
                        net_session = None;
                        net_match_count = 0;
                        net_in_fight = false;
                        net_frames_since_progress = 0;
                        net_stats_next_frame = 0;
                        ranked_match_index = 0;
                        net_spectate_next = 165;
                        net_frame_counter = 0;
                        net_runtime = NetRuntime::default();
                        net_log = None;
                        relay_chat = None;
                        chat_open = false;
                        chat_draft.clear();
                        chat_lines.clear();
                        mm_session_id = None;
                        peer_name = None;
                        auto_start_done = false;
                        auto_start_frame = 0;
                        lab_save_slot = None;
                        ghost_recording = None;
                        ghost_playback = None;
                        if let Some(recorder) = clip_recorder.take() {
                            let message = finish_clip_recording(recorder);
                            println!("[clip] {message}");
                            toast = Some((message, Instant::now() + Duration::from_millis(3200)));
                        }
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
                        state = AppState::Menu(MenuScreen::SessionEnded {
                            lines: teardown_lines,
                        });
                    }
                }
                canvas.set_logical_size(0, 0)?;
                canvas.set_draw_color(Color::RGB(0, 0, 0));
                canvas.clear();
                draw_emu_frame(
                    &mut canvas,
                    &mut emu_texture,
                    &texture_creator,
                    cfg.video_filter,
                    cfg.aspect_mode,
                    cfg.crt_corner_bend,
                )?;
                // Draw the fight overlay once the stage/round has loaded. round_num
                // flips on earlier than HP, so names appear during the intro instead
                // of waiting until the "FIGHT" callout. Hide it once a match is
                // decided so arcade endings after Shao Kahn do not keep the plates.
                let in_fight_screen = core
                    .as_ref()
                    .map(|c| {
                        let gstate =
                            memory::peek_u16(c, GSTATE_ADDR, memory::Endian::Little).unwrap_or(0);
                        let s = score::Score::read(c);
                        let match_decided = s.p1_match_wins >= MATCH_WIN_TARGET
                            || s.p2_match_wins >= MATCH_WIN_TARGET;
                        gstate != 0
                            && gstate != GS_AMODE
                            && gstate != GS_GAMEOVER
                            && s.round_num > 0
                            && !match_decided
                    })
                    .unwrap_or(false);
                if in_fight_screen {
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
                    } else if let Some(pb) = match_replay_playback.as_ref() {
                        Some(pb.p1_name())
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
                    } else if let Some(pb) = match_replay_playback.as_ref() {
                        Some(pb.p2_name())
                    } else if ghost_playback.is_some() {
                        if (ghost_port_mask & 0b10) != 0 {
                            Some(ghost_name)
                        } else {
                            Some(local_name)
                        }
                    } else {
                        Some("Lab")
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
                        cfg.scorebar_style,
                    )
                    .map_err(|e| format!("overlay: {e}"))?;
                    if let Some(recorder) = clip_recorder.as_ref() {
                        let label = format!(
                            "REC {:02}:{:02}",
                            recorder.elapsed_seconds() / 60,
                            recorder.elapsed_seconds() % 60
                        );
                        let scale = 1;
                        let text_w = font.text_width_exact(&label, scale);
                        let x = win_w as i32 - text_w - 18;
                        let y = 44;
                        canvas.set_draw_color(Color::RGBA(80, 0, 0, 210));
                        canvas.fill_rect(sdl2::rect::Rect::new(
                            x - 10,
                            y - 5,
                            (text_w + 18) as u32,
                            22,
                        ))?;
                        canvas.set_draw_color(Color::RGBA(235, 55, 55, 240));
                        canvas.fill_rect(sdl2::rect::Rect::new(x - 4, y + 5, 5, 5))?;
                        font.draw(
                            &mut canvas,
                            &label,
                            x + 8,
                            y,
                            scale,
                            Color::RGB(255, 215, 215),
                        )
                        .map_err(|e| format!("recording indicator: {e}"))?;
                    }
                    canvas.set_logical_size(LOGICAL_W as u32, LOGICAL_H as u32)?;
                }
                if net_session.is_none() && match_replay_playback.is_none() && lab_assist_visible {
                    canvas.set_logical_size(0, 0)?;
                    let (win_w, win_h) = canvas.output_size().unwrap_or((1200, 762));
                    draw_lab_assist_overlay(
                        &mut canvas,
                        &mut font,
                        win_w as i32,
                        win_h as i32,
                        &input_history,
                    )
                    .map_err(|e| format!("lab assist overlay: {e}"))?;
                    canvas.set_logical_size(LOGICAL_W as u32, LOGICAL_H as u32)?;
                }
                if net_session.is_some() && (chat_open || !chat_lines.is_empty()) {
                    canvas.set_logical_size(0, 0)?;
                    let (win_w, win_h) = canvas.output_size().unwrap_or((1200, 762));
                    draw_chat_overlay(
                        &mut canvas,
                        &mut font,
                        win_w as i32,
                        win_h as i32,
                        &chat_lines,
                        if chat_open { Some(&chat_draft) } else { None },
                    )
                    .map_err(|e| format!("chat overlay: {e}"))?;
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
                            Ok(matchmaking::Update::AuthConnected {
                                username,
                                player_id,
                            }) => {
                                mm_rx = None;
                                discord_user = Some(username.clone());
                                discord_id = Some(player_id);
                                toast = Some((
                                    format!("Discord connected as {username}"),
                                    Instant::now() + Duration::from_millis(2600),
                                ));
                                state = AppState::Menu(MenuScreen::Settings {
                                    cursor: 2,
                                    player_username: cfg.player_username.clone(),
                                    stats_email: cfg.stats_email.clone(),
                                    discord_connected: true,
                                    discord_rpc_enabled: cfg.discord_rpc_enabled,
                                    fullscreen: cfg.fullscreen,
                                    volume_percent: cfg.volume_percent,
                                    audio_buffer: cfg.audio_buffer,
                                    video_filter: cfg.video_filter,
                                    crt_corner_bend: cfg.crt_corner_bend,
                                    aspect_mode: cfg.aspect_mode,
                                    scorebar_style: cfg.scorebar_style,
                                });
                                break;
                            }
                            Ok(matchmaking::Update::Connected {
                                peer_endpoint,
                                is_host,
                                turn,
                                session_id,
                                room_id,
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
                                        &mut lab_save_slot,
                                        &mut ghost_playback,
                                        &mut ghost_recording,
                                    );
                                }
                                match_replay_playback = None;
                                input_history.clear();

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
                                        println!("[net] using freeplay-relay: {}", creds.uri);

                                        match relay_socket::RelaySocket::connect(
                                            &creds.uri,
                                            &creds.username,
                                            &creds.password,
                                            menu::DEFAULT_NETPLAY_PORT,
                                        ) {
                                            Ok(socket) => {
                                                let peer_label = socket.peer_label();
                                                relay_chat = match socket.chat_handle() {
                                                    Ok(handle) => Some(handle),
                                                    Err(e) => {
                                                        println!(
                                                            "[chat] relay chat unavailable: {e}"
                                                        );
                                                        None
                                                    }
                                                };
                                                if let Some(f) = log.as_mut() {
                                                    use std::io::Write;
                                                    let _ = writeln!(f,
                                                        "[net] relay session ready, routing through {peer_label} (registered={}, peer_ready={})",
                                                        socket.is_registered(),
                                                        socket.is_peer_ready());
                                                }
                                                println!(
                                                    "[net] relay session ready (registered={}, peer_ready={})",
                                                    socket.is_registered(),
                                                    socket.is_peer_ready()
                                                );

                                                let log_ref = &mut log;
                                                let lines_ref = &mut lines;
                                                netplay::start_session_with_socket(
                                                    local_handle,
                                                    peer_label,
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
                                        relay_chat = None;
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
                                        chat_open = false;
                                        chat_draft.clear();
                                        chat_lines.clear();
                                        net_recording = maybe_start_net_recording(
                                            &ghost_library,
                                            stun_peer,
                                            GHOST_CAP_PER_PEER,
                                        );
                                        let (p1, p2) = replay_names(
                                            local_handle,
                                            discord_user.as_deref(),
                                            peer_name.as_deref(),
                                        );
                                        match_replay_recording =
                                            Some(match_replay::Recording::new(p1, p2));
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

                                        let mut inc = incident::Incident::new(
                                            if tail.contains("relay") || tail.contains("Relay") {
                                                incident::KIND_TURN_FALLBACK_FAILED
                                            } else {
                                                incident::KIND_MATCH_ENDED_EARLY
                                            },
                                            tail,
                                        );
                                        inc.session_id = mm_session_id.clone();
                                        inc.room_id = room_id.clone();
                                        inc.peer_endpoint = Some(peer_endpoint.clone());
                                        inc.role = Some(if is_host { "host" } else { "join" });
                                        inc.net_log_path =
                                            Some(std::path::PathBuf::from("freeplay-net.log"));
                                        let (_size, hash) = rom_fingerprint();
                                        inc.rom_hash = Some(format!("{:016x}", hash));
                                        incident::submit(inc);

                                        state = AppState::Menu(MenuScreen::TestResult { lines });
                                    }
                                }
                                break;
                            }

                            Ok(matchmaking::Update::Error(e)) => {
                                println!("[mm] matchmaking error: {e}");
                                // Auto-incident on the "match never happened"
                                // failure modes. Classification is by error
                                // string because the matchmaking thread's
                                // error type collapses everything to String.
                                let kind = if e.contains("Hole punch") {
                                    incident::KIND_HOLE_PUNCH_FAILED
                                } else if e.contains("TURN") || e.contains("relay") {
                                    incident::KIND_TURN_FALLBACK_FAILED
                                } else {
                                    incident::KIND_MATCH_ENDED_EARLY
                                };
                                let mut inc = incident::Incident::new(kind, e.clone());
                                inc.session_id = mm_session_id.clone();
                                // peer_endpoint is None at this point — the
                                // matchmaking thread didn't survive long enough
                                // to publish a Connected event. Server-side
                                // queue state has it.
                                inc.role = Some(if local_handle == 0 { "host" } else { "join" });
                                inc.net_log_path =
                                    Some(std::path::PathBuf::from("freeplay-net.log"));
                                let (_size, hash) = rom_fingerprint();
                                inc.rom_hash = Some(format!("{:016x}", hash));
                                incident::submit(inc);

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
                    &main_leaderboard,
                    toast_payload(&toast),
                )
                .map_err(|e| format!("menu draw: {e}"))?;
                canvas.present();
                canvas.set_logical_size(menu::LOGICAL_W as u32, menu::LOGICAL_H as u32)?;
            }

            AppState::Menu(_) | AppState::Rebinding { .. } => {
                if matches!(
                    state,
                    AppState::Menu(menu::MenuScreen::TestIp { editing: true, .. })
                        | AppState::Menu(menu::MenuScreen::TextEdit { .. })
                        | AppState::Menu(menu::MenuScreen::MatchUsername {
                            checking: false,
                            ..
                        })
                ) {
                    video_subsystem.text_input().start();
                } else {
                    video_subsystem.text_input().stop();
                }

                if let Some(rx) = &username_check_rx {
                    if matches!(
                        state,
                        AppState::Menu(menu::MenuScreen::MatchUsername { .. })
                    ) {
                        match rx.try_recv() {
                            Ok(matchmaking::UsernameCheckUpdate::Available(username)) => {
                                cfg.player_username = username.clone();
                                config::save(&cfg);
                                matchmaking::set_guest_profile(
                                    username.clone(),
                                    cfg.stats_email.clone(),
                                );
                                let (tx, rx) = std::sync::mpsc::channel();
                                mm_rx = Some(rx);
                                matchmaking::start_guest(tx);
                                state = AppState::Menu(MenuScreen::Matchmaking {
                                    status: format!("Entering queue as {username}"),
                                });
                                username_check_rx = None;
                            }
                            Ok(matchmaking::UsernameCheckUpdate::Taken(username)) => {
                                state = AppState::Menu(MenuScreen::MatchUsername {
                                    value: username,
                                    status: "That name is already taken".into(),
                                    checking: false,
                                });
                                username_check_rx = None;
                            }
                            Ok(matchmaking::UsernameCheckUpdate::Error(message)) => {
                                let value = if let AppState::Menu(MenuScreen::MatchUsername {
                                    value,
                                    ..
                                }) = &state
                                {
                                    value.clone()
                                } else {
                                    cfg.player_username.clone()
                                };
                                state = AppState::Menu(MenuScreen::MatchUsername {
                                    value,
                                    status: message,
                                    checking: false,
                                });
                                username_check_rx = None;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                username_check_rx = None;
                                state = AppState::Menu(MenuScreen::MatchUsername {
                                    value: cfg.player_username.clone(),
                                    status: "Username check stopped".into(),
                                    checking: false,
                                });
                            }
                        }
                    } else {
                        username_check_rx = None;
                    }
                }

                if matches!(state, AppState::Menu(menu::MenuScreen::LiveMatches { .. }))
                    && live_matches_rx.is_none()
                    && Instant::now() >= live_matches_next_refresh
                {
                    let (tx, rx) = std::sync::mpsc::channel();
                    live_matches_rx = Some(rx);
                    matchmaking::fetch_live_matches(tx);
                    live_matches_next_refresh = Instant::now() + Duration::from_secs(7);
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
                        if std::path::Path::new("ghost.bin").exists() {
                            entries.push(menu::GhostEntry::Local {
                                filename: "ghost.bin".into(),
                                path: "ghost.bin".into(),
                            });
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

                // Drain the leaderboard fetcher channel.
                if let Some(rx) = &leaderboard_rx {
                    match rx.try_recv() {
                        Ok(matchmaking::LeaderboardUpdate::Loaded(rows)) => {
                            main_leaderboard = menu::LeaderboardState::Loaded(rows.clone());
                            if let AppState::Menu(menu::MenuScreen::Leaderboard { ref mut state }) =
                                state
                            {
                                *state = menu::LeaderboardState::Loaded(rows);
                            }
                            leaderboard_rx = None;
                        }
                        Ok(matchmaking::LeaderboardUpdate::Error(message)) => {
                            main_leaderboard = menu::LeaderboardState::Error(message.clone());
                            if let AppState::Menu(menu::MenuScreen::Leaderboard { ref mut state }) =
                                state
                            {
                                *state = menu::LeaderboardState::Error(message);
                            }
                            leaderboard_rx = None;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            leaderboard_rx = None;
                        }
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

                // Drain the spectator relay poller.
                if let Some(rx) = &spectate_rx {
                    if let AppState::Menu(menu::MenuScreen::Spectate { ref mut status, .. }) = state
                    {
                        loop {
                            match rx.try_recv() {
                                Ok(matchmaking::SpectateUpdate::State(update)) => {
                                    spectate_last_update = Some(Instant::now());
                                    status.message = "Live spectator relay connected".into();
                                    status.frame = update.frame;
                                    status.p1_score = update.p1_score;
                                    status.p2_score = update.p2_score;
                                    status.updated_at = update.updated_at;
                                }
                                Ok(matchmaking::SpectateUpdate::Error(message)) => {
                                    status.message = format!("Relay error: {message}");
                                }
                                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                    spectate_rx = None;
                                    break;
                                }
                            }
                        }
                    } else {
                        spectate_rx = None;
                        spectate_last_update = None;
                    }
                }

                if let AppState::Menu(menu::MenuScreen::Spectate { ref mut status, .. }) = state {
                    if let Some(last) = spectate_last_update {
                        if last.elapsed() > Duration::from_secs(15) {
                            status.message =
                                "No live updates recently. Match may have ended.".into();
                        }
                    }
                }

                // Drain the live-match browser fetcher.
                if let Some(rx) = &live_matches_rx {
                    if let AppState::Menu(menu::MenuScreen::LiveMatches {
                        ref mut cursor,
                        ref mut matches,
                        ref mut status,
                    }) = state
                    {
                        match rx.try_recv() {
                            Ok(matchmaking::LiveMatchesUpdate::Loaded(list)) => {
                                *matches = list;
                                *cursor = 0;
                                *status = if matches.is_empty() {
                                    "No active matches right now".into()
                                } else {
                                    "Select a match to watch".into()
                                };
                                live_matches_rx = None;
                            }
                            Ok(matchmaking::LiveMatchesUpdate::Error(message)) => {
                                matches.clear();
                                *cursor = 0;
                                *status = format!("Error: {message}");
                                live_matches_rx = None;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                live_matches_rx = None;
                            }
                        }
                    } else {
                        live_matches_rx = None;
                    }
                }

                // Drain the ghost-list fetcher channel.
                if let Some(rx) = &ghost_list_rx {
                    if let AppState::Menu(menu::MenuScreen::GhostSelect {
                        ref mut entries,
                        ref mut download_status,
                        ..
                    }) = state
                    {
                        match rx.try_recv() {
                            Ok(matchmaking::GhostListUpdate::Loaded(ghosts)) => {
                                let loaded_count = ghosts.len();
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
                                if loaded_count == 0 && entries.is_empty() {
                                    *download_status = Some("No shared ghosts found".into());
                                } else {
                                    *download_status = None;
                                }
                                ghost_list_rx = None;
                            }
                            Ok(matchmaking::GhostListUpdate::Error(e)) => {
                                println!("[ghost] list fetch failed: {e}");
                                *download_status = Some(format!("Error: {e}"));
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
                        ref mut entries,
                        ref mut download_status,
                        ..
                    }) = state
                    {
                        match rx.try_recv() {
                            Ok(matchmaking::GhostDownloadUpdate::Saved { local_path, .. }) => {
                                ghost_download_rx = None;
                                *download_status = Some("Loading ghost...".into());
                                ensure_core_loaded(&mut core, &mut audio_queue, &audio_subsystem)?;
                                if let Some(c) = &core {
                                    match ghost::Playback::load(&local_path) {
                                        Ok(pb) => {
                                            if pb.prime(c) {
                                                println!(
                                                    "[ghost] Loaded remote logic opponent: {} frames",
                                                    pb.frame_count()
                                                );
                                                start_logic_ghost_opponent(
                                                    pb,
                                                    &mut ghost_port_mask,
                                                    &mut ghost_playback,
                                                    &mut drone_runner,
                                                );
                                                match_replay_playback = None;
                                                input::clear_all_inputs();
                                                auto_start_done = false;
                                                auto_start_frame = 0;
                                                state = AppState::Playing;
                                            } else {
                                                println!("[ghost] Anchor state rejected.");
                                                *download_status =
                                                    Some("Error: anchor state rejected".into());
                                            }
                                        }
                                        Err(e) => {
                                            println!("[ghost] Load failed: {e}");
                                            *download_status =
                                                Some(format!("Error: ghost load failed: {e}"));
                                        }
                                    }
                                } else {
                                    *download_status =
                                        Some("Error: emulator core is not loaded".into());
                                }
                            }
                            Ok(matchmaking::GhostDownloadUpdate::Error { ghost_id, message }) => {
                                if message.contains("404") {
                                    entries.retain(|entry| match entry {
                                        menu::GhostEntry::Remote(meta) => meta.ghost_id != ghost_id,
                                        _ => true,
                                    });
                                    if entries.is_empty() {
                                        *cursor = 0;
                                    } else if *cursor >= entries.len() {
                                        *cursor = entries.len() - 1;
                                    }
                                    *download_status =
                                        Some("Shared ghost is no longer available".into());
                                } else {
                                    *download_status = Some(format!("Error: {message}"));
                                }
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
                    &main_leaderboard,
                    toast_payload(&toast),
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
                Some((2, 2))
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
                    mm_session_id
                        .as_deref()
                        .filter(|sid| !sid.is_empty())
                        .map(|sid| format!("xband://watch/{sid}"))
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
