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
mod gl_crt;
mod ghost;
mod incident;
mod input;
mod input_history;
mod lab;
mod log;
mod match_replay;
mod matchmaking;
mod memory;
mod menu;
mod menu_input;
mod mk2_addrs;
mod mk2_perf;
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
use crate::input::{set_action_source, Bindings, InputSource, Player};
use crate::menu::{AppState, MenuScreen, NavResult, LOGICAL_H, LOGICAL_W};
use crate::menu_input::{capture_rebind, event_to_menu_nav, is_cancel, is_clear, MenuNav};
use crate::netcore::{reset_for_netplay, step_netplay_frame, NetRuntime};
use crate::render::{
    build_window_canvas, draw_chat_overlay, draw_emu_frame, draw_fight_overlay,
    draw_lab_assist_overlay, draw_net_stats_overlay, draw_replay_review_overlay,
    draw_render_debug_overlay, ensure_core_loaded, format_probe_result, netplay_safe_filter,
    recommended_profile, renderer_name, route_player, OverlayCache,
};
use crate::retro::*;
use crate::session::{
    finalize_net_recording, handle_score_event, maybe_start_net_recording, open_net_log,
    rom_fingerprint,
};

use sdl2::audio::AudioQueue;
use sdl2::event::Event;
use sdl2::keyboard::{Keycode, Mod, Scancode};
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::render::BlendMode;
use sdl2::surface::Surface;
use sdl2::video::{FullscreenType, Window};
use std::time::{Duration, Instant};

const CHAT_MAX_LINES: usize = 8;

#[cfg(target_os = "windows")]
fn launch_debugger() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let title = format!("FREEPLAY DOCTOR v{}", version::VERSION);
    std::process::Command::new("cmd")
        .arg("/C")
        .arg("start")
        .arg(title)
        .arg("cmd")
        .arg("/K")
        .arg(format!("\"{}\" --doctor", exe.display()))
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

fn app_window_title() -> String {
    format!("FREEPLAY v{}", version::VERSION)
}

fn render_probe_window_title() -> String {
    format!("{} RENDER PROBE", app_window_title())
}

fn set_app_window_icon(window: &mut Window) {
    for path in ["appicon.png", "src/appicon.png"] {
        match set_window_icon_from_png(window, path) {
            Ok(()) => {
                println!("[window] icon loaded from {path}");
                return;
            }
            Err(_) => {}
        }
    }
}

fn set_window_icon_from_png(window: &mut Window, path: &str) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let (rgba, width, height) =
        png::decode_png(&bytes).ok_or_else(|| "unsupported PNG icon format".to_string())?;
    let (mut rgba, width, height) = square_icon_rgba(&rgba, width, height, 256)?;
    let pitch = width
        .checked_mul(4)
        .ok_or_else(|| "PNG icon width is too large".to_string())?;
    let surface = Surface::from_data(&mut rgba, width, height, pitch, PixelFormatEnum::RGBA32)?;
    window.set_icon(surface);
    Ok(())
}

fn square_icon_rgba(
    src: &[u8],
    width: u32,
    height: u32,
    max_size: u32,
) -> Result<(Vec<u8>, u32, u32), String> {
    if width == 0 || height == 0 || max_size == 0 {
        return Err("PNG icon dimensions are empty".to_string());
    }
    let expected_len = width as usize * height as usize * 4;
    if src.len() < expected_len {
        return Err("PNG icon data is truncated".to_string());
    }

    let max_dim = width.max(height);
    let target = max_dim.min(max_size).max(1);
    let draw_w = ((width as u64 * target as u64) / max_dim as u64).max(1) as u32;
    let draw_h = ((height as u64 * target as u64) / max_dim as u64).max(1) as u32;
    let offset_x = (target - draw_w) / 2;
    let offset_y = (target - draw_h) / 2;
    let mut out = vec![0_u8; target as usize * target as usize * 4];

    for y in 0..draw_h {
        let src_y = ((y as u64 * height as u64) / draw_h as u64).min(height as u64 - 1) as u32;
        for x in 0..draw_w {
            let src_x = ((x as u64 * width as u64) / draw_w as u64).min(width as u64 - 1) as u32;
            let src_idx = (src_y as usize * width as usize + src_x as usize) * 4;
            let dst_idx =
                ((y + offset_y) as usize * target as usize + (x + offset_x) as usize) * 4;
            out[dst_idx..dst_idx + 4].copy_from_slice(&src[src_idx..src_idx + 4]);
        }
    }

    Ok((out, target, target))
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

fn video_filter_toast_message(filter: config::VideoFilter, renderer: &str) -> String {
    if filter.uses_opengl_shader() && !renderer.eq_ignore_ascii_case("opengl") {
        format!("Video Filter {} (restart for OpenGL)", filter.label())
    } else {
        format!("Video Filter {}", filter.label())
    }
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
const REPLAY_SEEK_FRAMES: usize = 55 * 5;
const REPLAY_SPEED_LABELS: [&str; 5] = ["0.25X", "0.5X", "1X", "2X", "4X"];
const REPLAY_DEFAULT_SPEED: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalPlayMode {
    Arcade,
    Lab,
}

impl LocalPlayMode {
    fn is_lab(self) -> bool {
        self == Self::Lab
    }
}

fn start_find_match_queue(
    cfg: &config::Config,
    mm_rx: &mut Option<std::sync::mpsc::Receiver<matchmaking::Update>>,
    state: &mut AppState,
    username: String,
    discord_user: &mut Option<String>,
    discord_id: &mut Option<String>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    *mm_rx = Some(rx);
    if let Some(discord_name) = matchmaking::connected_discord_user_from_cached_token() {
        *discord_user = Some(discord_name.clone());
        if discord_id.is_none() {
            *discord_id = matchmaking::discord_id_from_cached_token();
        }
        matchmaking::start(tx);
        *state = AppState::Menu(MenuScreen::Matchmaking {
            status: format!("Entering queue as {discord_name}"),
        });
    } else {
        matchmaking::set_guest_profile(
            username.clone(),
            cfg.stats_email.clone(),
            cfg.guest_device_id.clone(),
        );
        matchmaking::start_guest(tx);
        *state = AppState::Menu(MenuScreen::Matchmaking {
            status: format!("Entering queue as {username}"),
        });
    }
}

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

fn step_replay_frame(core: &retro::Core, playback: &mut match_replay::Playback) -> bool {
    if !playback.inject_next() {
        return false;
    }
    unsafe {
        (core.run)();
    }
    playback.observe_current_frame(core);
    true
}

fn seek_replay_to(
    core: &retro::Core,
    playback: &mut match_replay::Playback,
    target_frame: usize,
) -> bool {
    if !playback.reset_to_anchor(core) {
        return false;
    }
    let target = target_frame.min(playback.frame_count());
    while playback.current_frame() < target {
        if !step_replay_frame(core, playback) {
            break;
        }
        unsafe {
            clear_audio_buffer();
        }
    }
    input::clear_all_inputs();
    unsafe {
        clear_audio_buffer();
    }
    true
}

fn seek_replay_relative(
    core: &retro::Core,
    playback: &mut match_replay::Playback,
    delta_frames: isize,
) -> bool {
    let current = playback.current_frame() as isize;
    let target = current
        .saturating_add(delta_frames)
        .clamp(0, playback.frame_count() as isize) as usize;
    seek_replay_to(core, playback, target)
}

fn replay_frames_for_tick(speed_index: usize, tick: u64) -> usize {
    match speed_index {
        0 => {
            if tick % 4 == 0 {
                1
            } else {
                0
            }
        }
        1 => {
            if tick % 2 == 0 {
                1
            } else {
                0
            }
        }
        3 => 2,
        4 => 4,
        _ => 1,
    }
}

fn adjust_replay_speed(speed_index: &mut usize, delta: isize) {
    let next = (*speed_index as isize)
        .saturating_add(delta)
        .clamp(0, (REPLAY_SPEED_LABELS.len() - 1) as isize) as usize;
    *speed_index = next;
}

fn seek_replay_marker(
    core: &retro::Core,
    playback: &mut match_replay::Playback,
    direction: isize,
    filter: match_replay::ReplayEventFilter,
) -> bool {
    let current = playback.current_frame();
    let target = if direction >= 0 {
        playback.next_event_frame_after(current, filter)
    } else {
        playback.previous_event_frame_before(current, filter)
    };
    if let Some(frame) = target {
        seek_replay_to(core, playback, frame as usize)
    } else {
        false
    }
}

fn prepare_replay_review(core: &retro::Core, path: &str) -> Result<match_replay::Playback, String> {
    let mut pb =
        match_replay::Playback::load(path).map_err(|e| format!("replay load failed: {e}"))?;
    if !pb.prime(core) {
        return Err("replay state rejected".into());
    }
    let total_frames = pb.frame_count();
    let _ = seek_replay_to(core, &mut pb, total_frames);
    let _ = seek_replay_to(core, &mut pb, 0);
    println!(
        "[replay] Reviewing {} frames: {} vs {} ({} markers, {} bookmarks)",
        pb.frame_count(),
        pb.p1_name(),
        pb.p2_name(),
        pb.markers().len(),
        pb.bookmarks().len()
    );
    Ok(pb)
}

fn enter_replay_review(
    pb: match_replay::Playback,
    match_replay_playback: &mut Option<match_replay::Playback>,
    replay_review_paused: &mut bool,
    replay_review_speed: &mut usize,
    replay_review_tick: &mut u64,
    replay_event_filter: &mut match_replay::ReplayEventFilter,
    replay_clip_in: &mut Option<usize>,
    replay_clip_out: &mut Option<usize>,
    ghost_playback: &mut Option<ghost::Playback>,
    ghost_recording: &mut Option<ghost::Recording>,
    drone_runner: &mut Option<drone::DroneRunner>,
    input_history: &mut input_history::InputHistory,
    clip_recorder: &mut Option<clip::ClipRecorder>,
    toast: &mut Option<(String, Instant)>,
    state: &mut AppState,
) {
    *match_replay_playback = Some(pb);
    *replay_review_paused = false;
    *replay_review_speed = REPLAY_DEFAULT_SPEED;
    *replay_review_tick = 0;
    *replay_event_filter = match_replay::ReplayEventFilter::All;
    *replay_clip_in = None;
    *replay_clip_out = None;
    *ghost_playback = None;
    *ghost_recording = None;
    *drone_runner = None;
    input_history.clear();
    input::clear_all_inputs();
    if let Some(recorder) = clip_recorder.take() {
        let message = finish_clip_recording(recorder);
        println!("[clip] {message}");
        *toast = Some((message, Instant::now() + Duration::from_millis(3200)));
    }
    *state = AppState::Playing;
}

fn net_stats_detail_rows(
    rollback_frames: u32,
    load_count: u32,
    kbps_sent: Option<&str>,
    local_frames_behind: Option<&str>,
    remote_frames_behind: Option<&str>,
    ping_ms: Option<i32>,
    mk2_perf: Option<mk2_perf::Mk2PerfSample>,
) -> Vec<String> {
    let mut rows = Vec::new();
    if let Some(ms) = ping_ms {
        let quality = if ms <= 80 {
            "GOOD"
        } else if ms <= 140 {
            "OK"
        } else {
            "HIGH"
        };
        rows.push(format!("QUALITY {quality}"));
    }
    rows.push(format!("ROLL {}F", rollback_frames));
    if load_count > 0 {
        rows.push(format!("LOADS {load_count}"));
    }
    if let (Some(local), Some(remote)) = (local_frames_behind, remote_frames_behind) {
        rows.push(format!("BEHIND L{local} R{remote}"));
    }
    if let Some(kbps) = kbps_sent {
        rows.push(format!("SEND {kbps} KB/S"));
    }
    if let Some(perf) = mk2_perf {
        rows.extend(perf.detail_rows());
    }
    rows
}

fn refresh_replay_select(state: &mut AppState, status: Option<String>) {
    if let AppState::Menu(MenuScreen::ReplaySelect {
        cursor,
        entries,
        status: screen_status,
    }) = state
    {
        *entries = match_replay::list_online_replays()
            .into_iter()
            .map(|meta| menu::ReplayEntry {
                filename: meta.filename,
                path: meta.path,
                p1_name: meta.p1_name,
                p2_name: meta.p2_name,
                frame_count: meta.frame_count,
                note: meta.note,
                bookmark_count: meta.bookmark_count,
            })
            .collect();
        if entries.is_empty() {
            *cursor = 0;
        } else if *cursor >= entries.len() {
            *cursor = entries.len() - 1;
        }
        *screen_status = status.or_else(|| {
            if entries.is_empty() {
                Some("No online replays found".into())
            } else {
                None
            }
        });
    }
}

fn selected_replay_entry(state: &AppState) -> Option<menu::ReplayEntry> {
    if let AppState::Menu(MenuScreen::ReplaySelect {
        cursor, entries, ..
    }) = state
    {
        entries.get(*cursor).cloned()
    } else {
        None
    }
}

fn handle_replay_select_shortcut(event: &Event, state: &mut AppState) -> bool {
    if !matches!(state, AppState::Menu(MenuScreen::ReplaySelect { .. })) {
        return false;
    }

    match event {
        Event::KeyDown {
            keycode: Some(Keycode::Delete),
            repeat: false,
            ..
        }
        | Event::ControllerButtonDown {
            button: sdl2::controller::Button::X,
            ..
        } => {
            let Some(entry) = selected_replay_entry(state) else {
                refresh_replay_select(state, Some("No replay selected".into()));
                return true;
            };
            let status = match std::fs::remove_file(&entry.path) {
                Ok(()) => {
                    let _ = std::fs::remove_file(match_replay::replay_notes_path(&entry.path));
                    format!("Deleted {}", entry.filename)
                }
                Err(e) => format!("Delete failed: {e}"),
            };
            refresh_replay_select(state, Some(status));
            true
        }
        Event::KeyDown {
            keycode: Some(Keycode::N),
            repeat: false,
            ..
        }
        | Event::ControllerButtonDown {
            button: sdl2::controller::Button::RightShoulder,
            ..
        } => {
            let Some(entry) = selected_replay_entry(state) else {
                refresh_replay_select(state, Some("No replay selected".into()));
                return true;
            };
            let cursor = if let AppState::Menu(MenuScreen::ReplaySelect { cursor, .. }) = state {
                *cursor
            } else {
                0
            };
            let came_from = if let AppState::Menu(screen) = state {
                screen.clone()
            } else {
                MenuScreen::ReplaySelect {
                    cursor,
                    entries: vec![],
                    status: None,
                }
            };
            *state = AppState::Menu(MenuScreen::TextEdit {
                title: "REPLAY NOTE".into(),
                label: format!("{} vs {}", entry.p1_name, entry.p2_name),
                value: entry.note,
                field: menu::EditField::ReplayNote {
                    path: entry.path,
                    cursor,
                },
                came_from: Box::new(came_from),
            });
            true
        }
        Event::KeyDown {
            keycode: Some(Keycode::O),
            repeat: false,
            ..
        }
        | Event::ControllerButtonDown {
            button: sdl2::controller::Button::Y,
            ..
        } => {
            let target = std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join("replays");
            let _ = std::fs::create_dir_all(&target);
            let status = match open::that(&target) {
                Ok(()) => "Replay folder opened".into(),
                Err(e) => format!("Open failed: {e}"),
            };
            refresh_replay_select(state, Some(status));
            true
        }
        _ => false,
    }
}

fn apply_volume(samples: &mut [i16], volume_percent: u8) {
    let volume = volume_percent as i32;
    for s in samples.iter_mut() {
        *s = ((*s as i32 * volume) / 100).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
}

fn queue_game_audio(
    q: &AudioQueue<i16>,
    samples: &mut [i16],
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

    // Scale in place: the caller clears the buffer right after queueing, so
    // mutating it avoids a per-frame allocation at sub-100% volume.
    if volume_percent < 100 {
        apply_volume(samples, volume_percent);
    }
    let _ = q.queue_audio(samples);
}

fn finish_clip_recording(recorder: clip::ClipRecorder) -> String {
    match recorder.finish() {
        Ok(result) => result.message,
        Err(e) => format!("Clip save failed: {e}"),
    }
}

fn export_replay_clip(
    core: &retro::Core,
    playback: &mut match_replay::Playback,
    clip_in: usize,
    clip_out: usize,
    sample_rate: u32,
) -> String {
    let start = clip_in.min(clip_out).min(playback.frame_count());
    let end = clip_in.max(clip_out).min(playback.frame_count());
    if end <= start {
        return "Replay clip needs different IN/OUT frames".into();
    }

    let restore_frame = playback.current_frame();
    let mut recorder = match clip::ClipRecorder::start(sample_rate) {
        Ok(recorder) => recorder,
        Err(e) => return format!("Replay clip start failed: {e}"),
    };

    if !seek_replay_to(core, playback, start) {
        return "Replay clip export failed: anchor state rejected".into();
    }

    while playback.current_frame() < end && !recorder.is_at_limit() {
        if !step_replay_frame(core, playback) {
            break;
        }
        let samples = unsafe { drain_audio_buffer() };
        if !samples.is_empty() {
            recorder.record_audio(&samples);
        }
        if let Err(e) = recorder.record_frame() {
            let _ = seek_replay_to(core, playback, restore_frame);
            return format!("Replay clip frame capture failed: {e}");
        }
    }

    let limited = playback.current_frame() < end;
    let message = finish_clip_recording(recorder);
    let _ = seek_replay_to(core, playback, restore_frame);
    if limited {
        format!("{message} (trimmed to 20s)")
    } else {
        message
    }
}

/// Always persist a crash report locally. The incident upload to the
/// signaling server is skipped when the player isn't signed in, which is
/// exactly when first-launch crashes happen — this file is what we ask
/// users to attach to a GitHub issue.
fn write_local_crash_log(summary: &str, location: &str) {
    use std::io::Write;
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = format!("crash_{unix}.log");
    let Ok(mut f) = std::fs::File::create(&path) else {
        return;
    };
    let _ = writeln!(f, "Freeplay crash report");
    let _ = writeln!(f, "version: {}", version::footer_string());
    let _ = writeln!(f, "os: {}", std::env::consts::OS);
    let _ = writeln!(f, "panic: {summary}{location}");
    let _ = writeln!(
        f,
        "\nPlease attach this file (and freeplay-net.log if present) to an issue at"
    );
    let _ = writeln!(f, "https://github.com/junkwax/freeplay-gametalk/issues");
    println!("[crash] wrote {path}");
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
        write_local_crash_log(&summary, &location);
        incident::submit_now(&inc);
    }));
}

fn run_render_probe() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = config::load();
    println!(
        "[render] probe requested profile={}",
        cfg.render_profile.label()
    );
    let sdl_context = sdl2::init()?;
    let video_subsystem = sdl_context.video()?;
    let shader_requested = cfg.video_filter.uses_opengl_shader();
    let render_probe_title = render_probe_window_title();
    let mut window_builder = video_subsystem.window(&render_probe_title, 640, 480);
    window_builder.position_centered().hidden();
    if shader_requested {
        window_builder.opengl();
    }
    let mut window = window_builder.build()?;
    set_app_window_icon(&mut window);
    let canvas = build_window_canvas(window, cfg.render_profile, shader_requested)?;
    if shader_requested {
        match gl_crt::GlCrtRenderer::new(&canvas) {
            Ok(_) => println!("[render] CRT shader probe complete"),
            Err(e) => println!("[render] CRT shader probe failed: {e}"),
        }
    }
    println!("[render] probe complete");
    Ok(())
}

#[allow(static_mut_refs)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if cli::render_probe_requested() {
        return run_render_probe();
    }

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
                    println!("[main] xband:// deep link: join room");
                    rpc::post_join_request(room_id);
                }
                protocol::XbandUri::Watch { session_id } => {
                    println!("[main] xband:// deep link: watch session");
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
    let mut cfg = config::load();

    let main_window_title = app_window_title();
    let mut window_builder = video_subsystem.window(&main_window_title, 1200, 762);
    window_builder.position_centered().resizable();
    if cfg.video_filter.uses_opengl_shader() {
        window_builder.opengl();
    }
    let mut window = window_builder.build()?;
    set_app_window_icon(&mut window);
    let requested_render_profile = cfg.render_profile;
    let mut canvas = build_window_canvas(
        window,
        cfg.render_profile,
        cfg.video_filter.uses_opengl_shader(),
    )?;
    let mut render_startup_toast: Option<String> = None;
    if requested_render_profile == config::RenderProfile::Auto {
        let recommended = recommended_profile(&canvas);
        if recommended != config::RenderProfile::Auto {
            let driver = renderer_name(&canvas);
            println!(
                "[render] AUTO recommended {} via {}",
                recommended.label(),
                driver
            );
            cfg.render_profile = recommended;
            config::save(&cfg);
            render_startup_toast =
                Some(format!("Render Auto -> {} ({driver})", recommended.label()));
        }
    }
    let mut crt_shader = if cfg.video_filter.uses_opengl_shader()
        || renderer_name(&canvas).eq_ignore_ascii_case("opengl")
    {
        match gl_crt::GlCrtRenderer::new(&canvas) {
            Ok(shader) => Some(shader),
            Err(e) => {
                println!("[render] CRT shader unavailable: {e}");
                if cfg.video_filter.uses_opengl_shader() {
                    render_startup_toast = Some("CRT Shader unavailable; using CRT Deluxe".into());
                }
                None
            }
        }
    } else {
        None
    };
    canvas.set_blend_mode(BlendMode::Blend);
    canvas.set_logical_size(LOGICAL_W as u32, LOGICAL_H as u32)?;
    let texture_creator = canvas.texture_creator();

    let mut emu_texture =
        texture_creator.create_texture_streaming(PixelFormatEnum::ARGB8888, 400, 254)?;
    let mut overlay_cache: OverlayCache = None;

    let ttf_ctx = match sdl2::ttf::init() {
        Ok(c) => Some(c),
        Err(e) => {
            println!("SDL2_ttf init failed ({e}); using bitmap font");
            None
        }
    };
    let mut font = Font::new(&texture_creator, ttf_ctx.as_ref())?;

    let mut event_pump = sdl_context.event_pump()?;

    if cfg.fullscreen {
        let _ = canvas.window_mut().set_fullscreen(FullscreenType::Desktop);
    }
    config::set_signaling_url(cfg.signaling_url.clone());
    crate::rpc::set_discord_client_id(cfg.discord_client_id.clone());
    install_panic_incident_hook();
    let mut state = AppState::default();
    let mut rom_present = rom::PresenceCache::new();

    let mut discord_user: Option<String> = matchmaking::username_from_cached_token();
    let mut discord_id: Option<String> = matchmaking::discord_id_from_cached_token();
    let mut score_tracker = score::ScoreTracker::new();

    let mut core: Option<retro::Core> = None;
    let mut audio_queue: Option<AudioQueue<i16>> = None;
    let mut lab_reset_slots = lab::ResetSlots::default();
    let mut rewind_test: Option<replay::RewindTest> = None;
    let mut input_history = input_history::InputHistory::new();
    let mut lab_assist_visible = true;
    let mut local_play_mode = LocalPlayMode::Arcade;
    let mut lab_dummy = lab::DummyController::default();
    let mut lab_position_preset = lab::PositionPreset::default();
    let mut punish_trainer = lab::PunishTrainer::default();
    let mut damage_tracker = lab::DamageTracker::default();
    let mut ghost_recording: Option<ghost::Recording> = None;
    let mut ghost_playback: Option<ghost::Playback> = None;
    let mut match_replay_recording: Option<match_replay::Recording> = None;
    let mut match_replay_playback: Option<match_replay::Playback> = None;
    let mut replay_review_paused = false;
    let mut replay_review_speed = REPLAY_DEFAULT_SPEED;
    let mut replay_review_tick: u64 = 0;
    let mut replay_event_filter = match_replay::ReplayEventFilter::All;
    let mut replay_clip_in: Option<usize> = None;
    let mut replay_clip_out: Option<usize> = None;
    let mut clip_recorder: Option<clip::ClipRecorder> = None;
    let mut ghost_port_mask: u8 = 0b11;
    let mut drone_runner: Option<drone::DroneRunner> = None;
    let ghost_path = std::path::Path::new("ghost.bin").to_path_buf();
    const GHOST_CAP_PER_PEER: u32 = 3;
    let mut ghost_library = ghost::Library::load_default();
    let mut net_recording: Option<ghost::NetRecording> = None;
    const HITBOX_FLAG_ADDR: usize = mk2_addrs::HITBOX_FLAG_ADDR;

    let mut trainer = memory::PokeList::new();
    trainer.add(
        "p1_health",
        memory::Poke::U16 {
            addr: mk2_addrs::P1_HP_ADDR,
            value: 0x00A1,
            endian: memory::Endian::Little,
        },
    );
    trainer.add(
        "p2_health",
        memory::Poke::U16 {
            addr: mk2_addrs::P2_HP_ADDR,
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
            addr: mk2_addrs::FREEZE_TIMER_ADDR,
            value: 0x0001,
            endian: memory::Endian::Little,
        },
        memory::Poke::U16 {
            addr: mk2_addrs::FREEZE_TIMER_ADDR,
            value: 0x0000,
            endian: memory::Endian::Little,
        },
    );

    const GSTATE_ADDR: usize = mk2_addrs::GSTATE_ADDR;
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
    let mut net_stats_visible = false;
    let mut render_debug_visible = false;
    let mut latest_net_ping_ms: Option<i32> = None;
    let mut latest_net_kbps_sent: Option<String> = None;
    let mut latest_net_local_frames_behind: Option<String> = None;
    let mut latest_net_remote_frames_behind: Option<String> = None;
    let mut latest_net_rollback_frames: u32 = 0;
    let mut latest_net_load_count: u32 = 0;
    let mut net_spectate_next: u32 = 165; // ~3s
    let mut net_frame_counter: u32 = 0;
    const GS_FIGHTING: u16 = 0x02;
    const GS_GAMEOVER: u16 = 0x0b;
    const MATCH_WIN_TARGET: u16 = 2;
    let mut session_p1_wins: u32 = 0;
    let mut session_p2_wins: u32 = 0;
    const P1_HP_ADDR: usize = mk2_addrs::P1_HP_ADDR;
    const P2_HP_ADDR: usize = mk2_addrs::P2_HP_ADDR;
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
    let mut username_check_silent = false;
    // Separate channel for "generate an available Wu name to confirm". Kept
    // apart from username_check_rx because its Available result opens the
    // confirm screen rather than proceeding straight into the queue.
    let mut username_gen_rx: Option<std::sync::mpsc::Receiver<matchmaking::UsernameCheckUpdate>> =
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
    let mut toast: Option<(String, Instant)> =
        render_startup_toast.map(|message| (message, Instant::now() + Duration::from_millis(2600)));
    let frame_duration = Duration::from_micros(18281);
    let mut next_frame_deadline = Instant::now() + frame_duration;
    let mut fps_sample_started = Instant::now();
    let mut fps_sample_frames: u32 = 0;
    let mut current_fps: Option<f32> = None;
    let mut rpc_pulse: u32 = 0;

    ghost::drain_upload_queue(&cfg.stats_url);
    ghost::queue_all_local_ghosts(
        discord_id.as_deref(),
        discord_user.as_deref(),
        &format!("{:016x}", rom_fingerprint().1),
        &cfg.stats_url,
    );

    'running: loop {
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
                println!("[main] Join-to-spar request received");
                let (tx, rx) = std::sync::mpsc::channel();
                mm_rx = Some(rx);
                matchmaking::start_join_room(tx, room_id);
                state = AppState::Menu(MenuScreen::Matchmaking {
                    status: "Joining spar room...".into(),
                });
            }
        }
        if let Some(session_id) = rpc::take_spectate_request() {
            println!("[main] Spectate request received");
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
                        video_filter_toast_message(cfg.video_filter, renderer_name(&canvas)),
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
                        video_filter_toast_message(cfg.video_filter, renderer_name(&canvas)),
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
                    keycode: Some(Keycode::Left),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 11, .. })
                ) =>
                {
                    cfg.input_delay = cfg.input_delay.saturating_sub(1);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut input_delay,
                        ..
                    }) = state
                    {
                        *input_delay = cfg.input_delay;
                    }
                    toast = Some((
                        format!("Input Delay {} frames (next match)", cfg.input_delay),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Right),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 11, .. })
                ) =>
                {
                    cfg.input_delay = (cfg.input_delay + 1).min(8);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut input_delay,
                        ..
                    }) = state
                    {
                        *input_delay = cfg.input_delay;
                    }
                    toast = Some((
                        format!("Input Delay {} frames (next match)", cfg.input_delay),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Left),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 15, .. })
                ) =>
                {
                    cfg.render_profile = cfg.render_profile.cycle(-1);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut render_profile,
                        ..
                    }) = state
                    {
                        *render_profile = cfg.render_profile;
                    }
                    toast = Some((
                        format!("Render Profile {} (restart)", cfg.render_profile.label()),
                        Instant::now() + Duration::from_millis(2200),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::Right),
                    repeat: false,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::Settings { cursor: 15, .. })
                ) =>
                {
                    cfg.render_profile = cfg.render_profile.cycle(1);
                    config::save(&cfg);
                    if let AppState::Menu(MenuScreen::Settings {
                        ref mut render_profile,
                        ..
                    }) = state
                    {
                        *render_profile = cfg.render_profile;
                    }
                    toast = Some((
                        format!("Render Profile {} (restart)", cfg.render_profile.label()),
                        Instant::now() + Duration::from_millis(2200),
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
                        video_filter_toast_message(cfg.video_filter, renderer_name(&canvas)),
                        Instant::now() + Duration::from_millis(1800),
                    ));
                }

                Event::KeyDown {
                    keycode: Some(Keycode::F10),
                    keymod,
                    repeat: false,
                    ..
                } if state == AppState::Playing
                    && keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) =>
                {
                    render_debug_visible = !render_debug_visible;
                    toast = Some((
                        format!(
                            "Render debug {}",
                            if render_debug_visible { "ON" } else { "OFF" }
                        ),
                        Instant::now() + Duration::from_millis(1600),
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
                    if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                        match (replay_clip_in, replay_clip_out) {
                            (Some(clip_in), Some(clip_out)) => {
                                replay_review_paused = true;
                                replay_review_tick = 0;
                                let sample_rate = audio_queue
                                    .as_ref()
                                    .map(|q| q.spec().freq.max(1) as u32)
                                    .unwrap_or(48_000);
                                let message =
                                    export_replay_clip(c, pb, clip_in, clip_out, sample_rate);
                                println!("[clip] {message}");
                                toast =
                                    Some((message, Instant::now() + Duration::from_millis(3600)));
                            }
                            _ => {
                                toast = Some((
                                    "Set replay clip IN and OUT first".into(),
                                    Instant::now() + Duration::from_millis(2400),
                                ));
                            }
                        }
                    } else if let Some(recorder) = clip_recorder.take() {
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

                Event::KeyDown {
                    keycode: Some(Keycode::F11),
                    repeat: false,
                    ..
                } if matches!(state, AppState::Menu(MenuScreen::Matchmaking { .. })) => {
                    net_stats_visible = !net_stats_visible;
                    toast = Some((
                        format!(
                            "Network stats {}",
                            if net_stats_visible { "ON" } else { "OFF" }
                        ),
                        Instant::now() + Duration::from_millis(1600),
                    ));
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

                Event::KeyDown {
                    keycode: Some(Keycode::R),
                    repeat: false,
                    ..
                }
                | Event::ControllerButtonDown {
                    button: sdl2::controller::Button::Y,
                    ..
                } if matches!(
                    state,
                    AppState::Menu(MenuScreen::SessionEnded {
                        replay_path: Some(_),
                        ..
                    })
                ) =>
                {
                    let path = if let AppState::Menu(MenuScreen::SessionEnded {
                        replay_path: Some(path),
                        ..
                    }) = &state
                    {
                        path.clone()
                    } else {
                        String::new()
                    };
                    ensure_core_loaded(&mut core, &mut audio_queue, &audio_subsystem)?;
                    if let Some(c) = &core {
                        match prepare_replay_review(c, &path) {
                            Ok(pb) => enter_replay_review(
                                pb,
                                &mut match_replay_playback,
                                &mut replay_review_paused,
                                &mut replay_review_speed,
                                &mut replay_review_tick,
                                &mut replay_event_filter,
                                &mut replay_clip_in,
                                &mut replay_clip_out,
                                &mut ghost_playback,
                                &mut ghost_recording,
                                &mut drone_runner,
                                &mut input_history,
                                &mut clip_recorder,
                                &mut toast,
                                &mut state,
                            ),
                            Err(e) => {
                                println!("[replay] Session replay load failed: {e}");
                                toast = Some((
                                    format!("Replay unavailable: {e}"),
                                    Instant::now() + Duration::from_millis(3200),
                                ));
                            }
                        }
                    }
                }

                _ if matches!(state, AppState::Menu(_)) => {
                    if handle_replay_select_shortcut(&event, &mut state) {
                        continue;
                    }
                    if let Some(nav) = event_to_menu_nav(&event) {
                        match nav {
                            MenuNav::Up => state.nav_up(),
                            MenuNav::Down => state.nav_down(),
                            MenuNav::Accept => match state.nav_accept(rom_present.check()) {
                                NavResult::StartLocal { lab } => {
                                    let previous_local_mode = local_play_mode;
                                    local_play_mode = if lab {
                                        LocalPlayMode::Lab
                                    } else {
                                        LocalPlayMode::Arcade
                                    };
                                    ensure_core_loaded(
                                        &mut core,
                                        &mut audio_queue,
                                        &audio_subsystem,
                                    )?;
                                    input_history.clear();
                                    match_replay_playback = None;
                                    match_replay_recording = None;
                                    ghost_playback = None;
                                    drone_runner = None;
                                    score_tracker.reset();
                                    session_p1_wins = 0;
                                    session_p2_wins = 0;
                                    punish_trainer.reset_stats();
                                    damage_tracker.reset_stats();
                                    if lab {
                                        auto_start_done = false;
                                        auto_start_frame = 0;
                                    } else {
                                        if previous_local_mode.is_lab() {
                                            if let Some(c) = &core {
                                                c.reset();
                                                println!("[arcade] Core reset after leaving Lab.");
                                            }
                                        }
                                        lab_reset_slots.clear();
                                        lab_dummy.clear_loop();
                                        trainer.set_enabled("hitboxes", false);
                                        trainer.set_enabled("p1_health", false);
                                        trainer.set_enabled("p2_health", false);
                                        trainer.set_enabled("freeze_timer", false);
                                        auto_start_done = true;
                                        auto_start_frame = 0;
                                    }
                                    input::clear_all_inputs();
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
                                                cfg.input_delay,
                                            ) {
                                                Ok(s) => {
                                                    net_session = Some(s);
                                                    latest_net_ping_ms = None;
                                                    latest_net_kbps_sent = None;
                                                    latest_net_local_frames_behind = None;
                                                    latest_net_remote_frames_behind = None;
                                                    latest_net_rollback_frames = 0;
                                                    latest_net_load_count = 0;
                                                    net_stats_next_frame = 0;
                                                    net_frame_counter = 0;
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
                                    if cfg.player_username_confirmed {
                                        cfg.player_username = value.clone();
                                        config::save(&cfg);
                                        start_find_match_queue(
                                            &cfg,
                                            &mut mm_rx,
                                            &mut state,
                                            value,
                                            &mut discord_user,
                                            &mut discord_id,
                                        );
                                    } else if !cfg.player_username_autogenerated {
                                        let (tx, rx) = std::sync::mpsc::channel();
                                        username_check_rx = Some(rx);
                                        username_check_silent = true;
                                        matchmaking::check_username_available(
                                            cfg.stats_url.clone(),
                                            value.clone(),
                                            tx,
                                        );
                                        state = AppState::Menu(MenuScreen::Matchmaking {
                                            status: format!("Checking name {value}"),
                                        });
                                    } else {
                                        // Autogenerated name, not yet confirmed: ask the stats
                                        // service for a name that's actually free before showing
                                        // the confirm screen (don't offer one that's taken).
                                        let (tx, rx) = std::sync::mpsc::channel();
                                        username_gen_rx = Some(rx);
                                        matchmaking::generate_available_username(
                                            cfg.stats_url.clone(),
                                            tx,
                                        );
                                        state = AppState::Menu(MenuScreen::MatchUsername {
                                            value,
                                            status: "Finding an available name".into(),
                                            checking: true,
                                        });
                                    }
                                }
                                NavResult::SubmitUsername(value) => {
                                    match config::sanitize_username(&value) {
                                        Some(username) => {
                                            let (tx, rx) = std::sync::mpsc::channel();
                                            username_check_rx = Some(rx);
                                            username_check_silent = false;
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
                                    start_find_match_queue(
                                        &cfg,
                                        &mut mm_rx,
                                        &mut state,
                                        cfg.player_username.clone(),
                                        &mut discord_user,
                                        &mut discord_id,
                                    );
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
                                        input_delay: cfg.input_delay,
                                        render_profile: cfg.render_profile,
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
                                    let value = match &field {
                                        menu::EditField::Username => cfg.player_username.clone(),
                                        menu::EditField::StatsEmail => cfg.stats_email.clone(),
                                        menu::EditField::ReplayNote { .. } => String::new(),
                                    };
                                    let label = match &field {
                                        menu::EditField::Username => {
                                            "Choose the name other players see"
                                        }
                                        menu::EditField::StatsEmail => {
                                            "Optional email for portable stats"
                                        }
                                        menu::EditField::ReplayNote { .. } => "Replay note",
                                    };
                                    let settings_cursor = match &field {
                                        menu::EditField::Username => 0,
                                        menu::EditField::StatsEmail => 1,
                                        menu::EditField::ReplayNote { .. } => 0,
                                    };
                                    state = AppState::Menu(MenuScreen::TextEdit {
                                        title,
                                        label: label.into(),
                                        value,
                                        field,
                                        came_from: Box::new(MenuScreen::Settings {
                                            cursor: settings_cursor,
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
                                            input_delay: cfg.input_delay,
                                            render_profile: cfg.render_profile,
                                        }),
                                    });
                                }
                                NavResult::CommitText(field, value) => match field {
                                    menu::EditField::ReplayNote { path, cursor } => {
                                        let status =
                                            match match_replay::save_replay_note(&path, &value) {
                                                Ok(()) => "Replay note saved".to_string(),
                                                Err(e) => format!("Replay note failed: {e}"),
                                            };
                                        state = AppState::Menu(MenuScreen::ReplaySelect {
                                            cursor,
                                            entries: vec![],
                                            status: Some(status.clone()),
                                        });
                                        refresh_replay_select(&mut state, Some(status));
                                    }
                                    field => {
                                        match field {
                                            menu::EditField::Username => {
                                                cfg.player_username =
                                                    config::sanitize_username(&value)
                                                        .unwrap_or_else(config::default_username);
                                                cfg.player_username_confirmed = false;
                                                cfg.player_username_autogenerated = false;
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
                                                        Instant::now()
                                                            + Duration::from_millis(2200),
                                                    ));
                                                } else if let Some(email) =
                                                    config::normalize_email(trimmed)
                                                {
                                                    cfg.stats_email = email;
                                                    toast = Some((
                                                        "Stats email saved".into(),
                                                        Instant::now()
                                                            + Duration::from_millis(2200),
                                                    ));
                                                } else {
                                                    toast = Some((
                                                        "Enter a valid email address".into(),
                                                        Instant::now()
                                                            + Duration::from_millis(2600),
                                                    ));
                                                }
                                            }
                                            menu::EditField::ReplayNote { .. } => {}
                                        }
                                        config::save(&cfg);
                                        matchmaking::clear_cached_token();
                                        state = AppState::Menu(MenuScreen::Settings {
                                            cursor: match field {
                                                menu::EditField::Username => 0,
                                                menu::EditField::StatsEmail => 1,
                                                menu::EditField::ReplayNote { .. } => 0,
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
                                            input_delay: cfg.input_delay,
                                            render_profile: cfg.render_profile,
                                        });
                                    }
                                },
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
                                            input_delay: cfg.input_delay,
                                            render_profile: cfg.render_profile,
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
                                        video_filter_toast_message(
                                            cfg.video_filter,
                                            renderer_name(&canvas),
                                        ),
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
                                NavResult::AdjustInputDelay(delta) => {
                                    // ENTER steps through 0..=8 and wraps; LEFT/RIGHT
                                    // key handlers clamp at the ends instead.
                                    cfg.input_delay = if delta < 0 {
                                        cfg.input_delay.saturating_sub(delta.unsigned_abs() as u32)
                                    } else if cfg.input_delay >= 8 {
                                        0
                                    } else {
                                        (cfg.input_delay + delta as u32).min(8)
                                    };
                                    config::save(&cfg);
                                    if let AppState::Menu(MenuScreen::Settings {
                                        ref mut input_delay,
                                        ..
                                    }) = state
                                    {
                                        *input_delay = cfg.input_delay;
                                    }
                                    toast = Some((
                                        format!(
                                            "Input Delay {} frames (next match)",
                                            cfg.input_delay
                                        ),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                NavResult::CycleRenderProfile(delta) => {
                                    cfg.render_profile = cfg.render_profile.cycle(delta);
                                    config::save(&cfg);
                                    if let AppState::Menu(MenuScreen::Settings {
                                        ref mut render_profile,
                                        ..
                                    }) = state
                                    {
                                        *render_profile = cfg.render_profile;
                                    }
                                    toast = Some((
                                        format!(
                                            "Render Profile {} (restart)",
                                            cfg.render_profile.label()
                                        ),
                                        Instant::now() + Duration::from_millis(2200),
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
                                                    local_play_mode = LocalPlayMode::Lab;
                                                    input::clear_all_inputs();
                                                    auto_start_done = false;
                                                    auto_start_frame = 0;
                                                    state = AppState::Playing;
                                                } else {
                                                    println!(
                                                        "[ghost] Anchor state rejected by core."
                                                    );
                                                    state = AppState::Menu(MenuScreen::LabMenu {
                                                        cursor: 1,
                                                    });
                                                }
                                            }
                                            Err(e) => {
                                                println!("[ghost] Load failed: {e}");
                                                state = AppState::Menu(MenuScreen::LabMenu {
                                                    cursor: 1,
                                                });
                                            }
                                        }
                                    } else {
                                        state = AppState::Menu(MenuScreen::LabMenu { cursor: 1 });
                                    }
                                }
                                NavResult::LoadReplay(path) => {
                                    ensure_core_loaded(
                                        &mut core,
                                        &mut audio_queue,
                                        &audio_subsystem,
                                    )?;
                                    if let Some(c) = &core {
                                        match prepare_replay_review(c, &path) {
                                            Ok(pb) => enter_replay_review(
                                                pb,
                                                &mut match_replay_playback,
                                                &mut replay_review_paused,
                                                &mut replay_review_speed,
                                                &mut replay_review_tick,
                                                &mut replay_event_filter,
                                                &mut replay_clip_in,
                                                &mut replay_clip_out,
                                                &mut ghost_playback,
                                                &mut ghost_recording,
                                                &mut drone_runner,
                                                &mut input_history,
                                                &mut clip_recorder,
                                                &mut toast,
                                                &mut state,
                                            ),
                                            Err(e) => {
                                                println!("[replay] Load failed: {e}");
                                                refresh_replay_select(
                                                    &mut state,
                                                    Some(format!("Error: {e}")),
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
                        replay_review_paused = false;
                        replay_review_speed = REPLAY_DEFAULT_SPEED;
                        replay_review_tick = 0;
                        replay_event_filter = match_replay::ReplayEventFilter::All;
                        replay_clip_in = None;
                        replay_clip_out = None;
                        input::clear_all_inputs();
                        state = AppState::Menu(MenuScreen::ReplaySelect {
                            cursor: 0,
                            entries: vec![],
                            status: Some("Replay stopped".into()),
                        });
                        refresh_replay_select(&mut state, Some("Replay stopped".into()));
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::B | sdl2::controller::Button::Back,
                        ..
                    } if match_replay_playback.is_some() => {
                        match_replay_playback = None;
                        replay_review_paused = false;
                        replay_review_speed = REPLAY_DEFAULT_SPEED;
                        replay_review_tick = 0;
                        replay_event_filter = match_replay::ReplayEventFilter::All;
                        replay_clip_in = None;
                        replay_clip_out = None;
                        input::clear_all_inputs();
                        state = AppState::Menu(MenuScreen::ReplaySelect {
                            cursor: 0,
                            entries: vec![],
                            status: Some("Replay stopped".into()),
                        });
                        refresh_replay_select(&mut state, Some("Replay stopped".into()));
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Space | Keycode::Return | Keycode::KpEnter),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        replay_review_paused = !replay_review_paused;
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::Start,
                        ..
                    } if match_replay_playback.is_some() => {
                        replay_review_paused = !replay_review_paused;
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Period),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                            replay_review_paused = true;
                            if !step_replay_frame(c, pb) {
                                toast = Some((
                                    "Replay complete".into(),
                                    Instant::now() + Duration::from_millis(1800),
                                ));
                            }
                        }
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::A,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                            replay_review_paused = true;
                            if !step_replay_frame(c, pb) {
                                toast = Some((
                                    "Replay complete".into(),
                                    Instant::now() + Duration::from_millis(1800),
                                ));
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::I),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let Some(pb) = match_replay_playback.as_ref() {
                            let frame = pb.current_frame();
                            replay_clip_in = Some(frame);
                            toast = Some((
                                format!("Replay clip IN set: {frame}"),
                                Instant::now() + Duration::from_millis(1600),
                            ));
                        }
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::X,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let Some(pb) = match_replay_playback.as_ref() {
                            let frame = pb.current_frame();
                            replay_clip_in = Some(frame);
                            toast = Some((
                                format!("Replay clip IN set: {frame}"),
                                Instant::now() + Duration::from_millis(1600),
                            ));
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::O),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let Some(pb) = match_replay_playback.as_ref() {
                            let frame = pb.current_frame();
                            replay_clip_out = Some(frame);
                            toast = Some((
                                format!("Replay clip OUT set: {frame}"),
                                Instant::now() + Duration::from_millis(1600),
                            ));
                        }
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::Y,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let Some(pb) = match_replay_playback.as_ref() {
                            let frame = pb.current_frame();
                            replay_clip_out = Some(frame);
                            toast = Some((
                                format!("Replay clip OUT set: {frame}"),
                                Instant::now() + Duration::from_millis(1600),
                            ));
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::C),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        replay_clip_in = None;
                        replay_clip_out = None;
                        toast = Some((
                            "Replay clip marks cleared".into(),
                            Instant::now() + Duration::from_millis(1600),
                        ));
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::M),
                        repeat: false,
                        ..
                    }
                    | Event::ControllerButtonDown {
                        button: sdl2::controller::Button::RightStick,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let Some(pb) = match_replay_playback.as_mut() {
                            replay_review_paused = true;
                            let frame = pb.current_frame();
                            match pb.toggle_bookmark_at_current() {
                                Ok(true) => {
                                    toast = Some((
                                        format!("Bookmark added: {frame}"),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                Ok(false) => {
                                    toast = Some((
                                        format!("Bookmark removed: {frame}"),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                Err(e) => {
                                    toast = Some((
                                        format!("Bookmark failed: {e}"),
                                        Instant::now() + Duration::from_millis(2600),
                                    ));
                                }
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Delete),
                        repeat: false,
                        ..
                    }
                    | Event::ControllerButtonDown {
                        button: sdl2::controller::Button::LeftStick,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let Some(pb) = match_replay_playback.as_mut() {
                            replay_review_paused = true;
                            match pb.remove_bookmark_near_current(90) {
                                Ok(Some(bookmark)) => {
                                    toast = Some((
                                        format!("Bookmark removed: {}", bookmark.frame),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                Ok(None) => {
                                    toast = Some((
                                        "No nearby bookmark".into(),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                Err(e) => {
                                    toast = Some((
                                        format!("Bookmark failed: {e}"),
                                        Instant::now() + Duration::from_millis(2600),
                                    ));
                                }
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Left),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                            let _ = seek_replay_relative(c, pb, -(REPLAY_SEEK_FRAMES as isize));
                            replay_review_tick = 0;
                        }
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::DPadLeft,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                            let _ = seek_replay_relative(c, pb, -(REPLAY_SEEK_FRAMES as isize));
                            replay_review_tick = 0;
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Right),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                            let _ = seek_replay_relative(c, pb, REPLAY_SEEK_FRAMES as isize);
                            replay_review_tick = 0;
                        }
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::DPadRight,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                            let _ = seek_replay_relative(c, pb, REPLAY_SEEK_FRAMES as isize);
                            replay_review_tick = 0;
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::PageUp),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                            if seek_replay_marker(c, pb, -1, replay_event_filter) {
                                replay_review_paused = true;
                                replay_review_tick = 0;
                            }
                        }
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::LeftShoulder,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                            if seek_replay_marker(c, pb, -1, replay_event_filter) {
                                replay_review_paused = true;
                                replay_review_tick = 0;
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::PageDown),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                            if seek_replay_marker(c, pb, 1, replay_event_filter) {
                                replay_review_paused = true;
                                replay_review_tick = 0;
                            }
                        }
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::RightShoulder,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                            if seek_replay_marker(c, pb, 1, replay_event_filter) {
                                replay_review_paused = true;
                                replay_review_tick = 0;
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Up),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        adjust_replay_speed(&mut replay_review_speed, 1);
                        replay_review_tick = 0;
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::DPadUp,
                        ..
                    } if match_replay_playback.is_some() => {
                        adjust_replay_speed(&mut replay_review_speed, 1);
                        replay_review_tick = 0;
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Down),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        adjust_replay_speed(&mut replay_review_speed, -1);
                        replay_review_tick = 0;
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::DPadDown,
                        ..
                    } if match_replay_playback.is_some() => {
                        adjust_replay_speed(&mut replay_review_speed, -1);
                        replay_review_tick = 0;
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F),
                        keymod,
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some()
                        && !keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) =>
                    {
                        replay_event_filter = replay_event_filter.next();
                        toast = Some((
                            format!("Replay events: {}", replay_event_filter.label()),
                            Instant::now() + Duration::from_millis(1800),
                        ));
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::Guide,
                        ..
                    } if match_replay_playback.is_some() => {
                        replay_event_filter = replay_event_filter.next();
                        toast = Some((
                            format!("Replay events: {}", replay_event_filter.label()),
                            Instant::now() + Duration::from_millis(1800),
                        ));
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Home),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                            let _ = seek_replay_to(c, pb, 0);
                            replay_review_tick = 0;
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::End),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() => {
                        if let (Some(c), Some(pb)) = (&core, match_replay_playback.as_mut()) {
                            let end_frame = pb.frame_count();
                            let _ = seek_replay_to(c, pb, end_frame);
                            replay_review_paused = true;
                            replay_review_tick = 0;
                        }
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
                    } if net_session.is_some() => {
                        net_teardown_reason = Some("you quit the match".into());
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
                        keymod,
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD) =>
                    {
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
                        keymod,
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && ghost_playback.is_none()
                        && local_play_mode.is_lab()
                        && keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD) =>
                    {
                        punish_trainer.reset_stats();
                        damage_tracker.reset_stats();
                        toast = Some((
                            "Lab stats reset".into(),
                            Instant::now() + Duration::from_millis(1800),
                        ));
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F10),
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && local_play_mode.is_lab()
                        && ghost_playback.is_none() =>
                    {
                        let on = punish_trainer.toggle();
                        toast = Some((
                            format!("Punish trainer {}", if on { "ON" } else { "OFF" }),
                            Instant::now() + Duration::from_millis(1800),
                        ));
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F10),
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && ghost_playback.is_some()
                        && local_play_mode.is_lab() =>
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
                        keycode,
                        scancode,
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && local_play_mode.is_lab()
                        && (keycode == Some(Keycode::F2) || scancode == Some(Scancode::F2)) =>
                    {
                        toggle_hitbox_view(&mut trainer, &mut toast);
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F3),
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && local_play_mode.is_lab() =>
                    {
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
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && local_play_mode.is_lab() =>
                    {
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
                        keymod,
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && ghost_playback.is_none()
                        && local_play_mode.is_lab()
                        && keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) =>
                    {
                        if lab_dummy.is_recording() {
                            let frames = lab_dummy.stop_recording();
                            let message = if frames > 0 {
                                format!("Dummy loop saved {}", lab::format_frames(frames))
                            } else {
                                "Dummy loop empty".into()
                            };
                            toast = Some((message, Instant::now() + Duration::from_millis(2200)));
                        } else if let Some(c) = &core {
                            let gstate = memory::peek_u16(c, GSTATE_ADDR, memory::Endian::Little)
                                .unwrap_or(0);
                            let p1_hp = memory::peek_u16(c, P1_HP_ADDR, memory::Endian::Little)
                                .unwrap_or(0);
                            let fight_loaded = matches!(gstate, GS_FIGHTING | 0x03) && p1_hp > 0;
                            if fight_loaded {
                                punish_trainer.reset_stats();
                                damage_tracker.reset_stats();
                                lab_dummy.start_recording();
                                toast = Some((
                                    "Recording P2 dummy... Ctrl+F5 stops".into(),
                                    Instant::now() + Duration::from_millis(2400),
                                ));
                            } else {
                                toast = Some((
                                    "Start dummy record after fight loads".into(),
                                    Instant::now() + Duration::from_millis(2200),
                                ));
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F5),
                        keymod,
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && ghost_playback.is_none()
                        && local_play_mode.is_lab()
                        && keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD) =>
                    {
                        lab_dummy.clear_loop();
                        punish_trainer.reset_stats();
                        damage_tracker.reset_stats();
                        input::apply_snapshot(Player::P2, 0);
                        toast = Some((
                            "Dummy loop cleared".into(),
                            Instant::now() + Duration::from_millis(1800),
                        ));
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F5),
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && ghost_playback.is_none()
                        && local_play_mode.is_lab() =>
                    {
                        let mode = lab_dummy.cycle_mode();
                        punish_trainer.reset_stats();
                        damage_tracker.reset_stats();
                        if mode.active() {
                            auto_start_done = false;
                            auto_start_frame = 0;
                        } else {
                            input::apply_snapshot(Player::P2, 0);
                        }
                        toast = Some((
                            format!("Dummy {}", mode.label()),
                            Instant::now() + Duration::from_millis(1800),
                        ));
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F6),
                        keymod,
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && local_play_mode.is_lab()
                        && keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) =>
                    {
                        if let Some(c) = &core {
                            let gstate = memory::peek_u16(c, GSTATE_ADDR, memory::Endian::Little)
                                .unwrap_or(0);
                            let p1_hp = memory::peek_u16(c, P1_HP_ADDR, memory::Endian::Little)
                                .unwrap_or(0);
                            let fight_loaded = matches!(gstate, GS_FIGHTING | 0x03) && p1_hp > 0;
                            if fight_loaded {
                                let preset = lab_position_preset;
                                lab::apply_position_preset(c, preset);
                                lab_position_preset = lab_position_preset.next();
                                toast = Some((
                                    format!("Position {}", preset.label()),
                                    Instant::now() + Duration::from_millis(1800),
                                ));
                            } else {
                                toast = Some((
                                    "Position reset after fight loads".into(),
                                    Instant::now() + Duration::from_millis(2200),
                                ));
                            }
                        } else {
                            toast = Some((
                                "Position reset after fight loads".into(),
                                Instant::now() + Duration::from_millis(2200),
                            ));
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F7),
                        keymod,
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && local_play_mode.is_lab()
                        && keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) =>
                    {
                        let slot = lab_reset_slots.cycle_next();
                        toast = Some((
                            format!("Lab reset slot {slot}"),
                            Instant::now() + Duration::from_millis(1600),
                        ));
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F7),
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && local_play_mode.is_lab() =>
                    {
                        if let Some(c) = &core {
                            match c.save_state() {
                                Some(blob) => {
                                    let slot = lab_reset_slots.active_number();
                                    let bytes = lab_reset_slots.save_active(blob);
                                    println!("[lab] Saved reset point slot {slot} ({bytes} bytes)");
                                    toast = Some((
                                        format!("Lab reset slot {slot} saved"),
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
                        keycode: Some(Keycode::F6),
                        repeat: false,
                        ..
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && local_play_mode.is_lab() =>
                    {
                        let slot = lab_reset_slots.active_number();
                        if let Some(c) = &core {
                            if let Some(loaded) = lab_reset_slots.load_active(c) {
                                if loaded {
                                    input::clear_all_inputs();
                                    input_history.clear();
                                    println!("[lab] Reset to saved slot {slot}");
                                    toast = Some((
                                        format!("Lab reset slot {slot} loaded"),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                } else {
                                    toast = Some((
                                        format!("Lab reset slot {slot} failed"),
                                        Instant::now() + Duration::from_millis(2200),
                                    ));
                                }
                            } else {
                                toast = Some((
                                    format!("No lab reset in slot {slot}"),
                                    Instant::now() + Duration::from_millis(1800),
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
                        keycode: Some(Keycode::F9),
                        repeat: false,
                        ..
                    } if local_play_mode.is_lab() => {
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
                    } if local_play_mode.is_lab() => {
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
                    } if local_play_mode.is_lab() => {
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
                        repeat: false,
                        ..
                    } if net_session.is_some() && match_replay_playback.is_none() => {
                        net_stats_visible = !net_stats_visible;
                        toast = Some((
                            format!(
                                "Network stats {}",
                                if net_stats_visible { "ON" } else { "OFF" }
                            ),
                            Instant::now() + Duration::from_millis(1600),
                        ));
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
                    } if net_session.is_none()
                        && match_replay_playback.is_none()
                        && local_play_mode.is_lab() =>
                    {
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
                    } if match_replay_playback.is_some() => {
                        let _ = k;
                    }
                    Event::KeyUp { .. } if match_replay_playback.is_some() => {}
                    Event::ControllerButtonDown { .. } if match_replay_playback.is_some() => {}
                    Event::ControllerButtonUp { .. } if match_replay_playback.is_some() => {}
                    Event::ControllerAxisMotion { .. } if match_replay_playback.is_some() => {}
                    Event::KeyDown {
                        keycode: Some(k),
                        repeat: false,
                        ..
                    } if !chat_open => {
                        for p in [Player::P1, Player::P2] {
                            let source = InputSource::key(k);
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
                                set_action_source(dest, a, source.clone(), true);
                            }
                        }
                    }
                    Event::KeyUp {
                        keycode: Some(k), ..
                    } if !chat_open => {
                        for p in [Player::P1, Player::P2] {
                            let source = InputSource::key(k);
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
                                set_action_source(dest, a, source.clone(), false);
                            }
                        }
                    }
                    Event::ControllerButtonDown { which, button, .. } if !chat_open => {
                        if let Some(p) = pad_owner(&pads, which) {
                            let source = InputSource::pad_button(which, button);
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
                                set_action_source(dest, a, source.clone(), true);
                            }
                        } else {
                            dlog!("input", "PadDown pad={} {:?} -- no owner", which, button);
                        }
                    }
                    Event::ControllerButtonUp { which, button, .. } if !chat_open => {
                        if let Some(p) = pad_owner(&pads, which) {
                            let source = InputSource::pad_button(which, button);
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
                                set_action_source(dest, a, source.clone(), false);
                            }
                        }
                    }
                    Event::ControllerAxisMotion {
                        which, axis, value, ..
                    } if !chat_open => {
                        if let Some(p) = pad_owner(&pads, which) {
                            for update in cfg.bindings.get(p).axis_updates(axis, value) {
                                let dest = route_player(p, &net_session, local_handle);
                                dlog!("input", "PadAxis pad={} {:?}={} bound={:?} dest={:?} action={:?} pressed={}",
                                          which, axis, value, p, dest, update.action, update.pressed);
                                set_action_source(
                                    dest,
                                    update.action,
                                    InputSource::pad_axis(which, axis, update.positive),
                                    update.pressed,
                                );
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
                        latest_net_rollback_frames =
                            step_stats.advance_count.saturating_sub(1) as u32;
                        latest_net_load_count = step_stats.load_count as u32;

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
                                latest_net_ping_ms = Some(stats.ping as i32);
                                latest_net_kbps_sent = Some(stats.kbps_sent.to_string());
                                latest_net_local_frames_behind =
                                    Some(stats.local_frames_behind.to_string());
                                latest_net_remote_frames_behind =
                                    Some(stats.remote_frames_behind.to_string());
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
                        if replay_review_paused {
                            input::clear_all_inputs();
                        } else {
                            replay_review_tick = replay_review_tick.wrapping_add(1);
                            let frames_this_tick =
                                replay_frames_for_tick(replay_review_speed, replay_review_tick);
                            let mut complete = false;
                            for _ in 0..frames_this_tick {
                                if !step_replay_frame(c, pb) {
                                    complete = true;
                                    break;
                                }
                            }
                            if replay_review_speed != REPLAY_DEFAULT_SPEED {
                                unsafe {
                                    clear_audio_buffer();
                                }
                            }
                            if complete {
                                println!(
                                    "[replay] Playback complete ({} frames).",
                                    pb.frame_count()
                                );
                                match_replay_playback = None;
                                replay_review_paused = false;
                                replay_review_speed = REPLAY_DEFAULT_SPEED;
                                replay_review_tick = 0;
                                replay_event_filter = match_replay::ReplayEventFilter::All;
                                replay_clip_in = None;
                                replay_clip_out = None;
                                input::clear_all_inputs();
                                state = AppState::Menu(MenuScreen::ReplaySelect {
                                    cursor: 0,
                                    entries: vec![],
                                    status: Some("Replay complete".into()),
                                });
                                refresh_replay_select(&mut state, Some("Replay complete".into()));
                            }
                        }
                    } else {
                        input::commit_live_to_state();
                        input_history.step(input::snapshot_player(Player::P1));
                        if local_play_mode.is_lab() && !auto_start_done {
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
                        if local_play_mode.is_lab() && ghost_playback.is_none() {
                            let gstate = memory::peek_u16(c, GSTATE_ADDR, memory::Endian::Little)
                                .unwrap_or(0);
                            let p1_hp = memory::peek_u16(c, P1_HP_ADDR, memory::Endian::Little)
                                .unwrap_or(0);
                            let fight_loaded = matches!(gstate, GS_FIGHTING | 0x03) && p1_hp > 0;
                            let live_p2_bits = input::snapshot_player(Player::P2);
                            if let Some(bits) = lab_dummy.next_bits(fight_loaded, live_p2_bits) {
                                input::apply_snapshot(Player::P2, bits);
                            }
                            if let Some(frames) = lab_dummy.take_auto_finished_loop() {
                                toast = Some((
                                    format!("Dummy loop saved {}", lab::format_frames(frames)),
                                    Instant::now() + Duration::from_millis(2200),
                                ));
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
                        if local_play_mode.is_lab() {
                            trainer.apply(c);
                        }
                        unsafe {
                            (c.run)();
                        }
                        if net_session.is_none()
                            && match_replay_playback.is_none()
                            && ghost_playback.is_none()
                            && local_play_mode.is_lab()
                        {
                            let gstate = memory::peek_u16(c, GSTATE_ADDR, memory::Endian::Little)
                                .unwrap_or(0);
                            let p1_hp = memory::peek_u16(c, P1_HP_ADDR, memory::Endian::Little)
                                .unwrap_or(0);
                            let fight_loaded = matches!(gstate, GS_FIGHTING | 0x03) && p1_hp > 0;
                            let p2_hp = memory::peek_u16(c, P2_HP_ADDR, memory::Endian::Little)
                                .unwrap_or(0);
                            damage_tracker.observe(fight_loaded, p2_hp);
                            let p1_bits = input::snapshot_player(Player::P1);
                            if let Some(event) = punish_trainer.observe(p2_hp, p1_bits) {
                                toast = Some((
                                    event.label(),
                                    Instant::now() + Duration::from_millis(1400),
                                ));
                            }
                            if lab_dummy.take_loop_completed().is_some() {
                                punish_trainer.arm(p2_hp);
                            }
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
                        && local_play_mode.is_lab()
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
                                    &mut AUDIO_BUFFER,
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
                        let replay_path =
                            match_replay::finalize_recording(&mut match_replay_recording)
                                .map(|path| path.to_string_lossy().into_owned());
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
                        latest_net_ping_ms = None;
                        latest_net_kbps_sent = None;
                        latest_net_local_frames_behind = None;
                        latest_net_remote_frames_behind = None;
                        latest_net_rollback_frames = 0;
                        latest_net_load_count = 0;
                        relay_chat = None;
                        chat_open = false;
                        chat_draft.clear();
                        chat_lines.clear();
                        mm_session_id = None;
                        peer_name = None;
                        auto_start_done = false;
                        auto_start_frame = 0;
                        lab_reset_slots.clear();
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
                            replay_path,
                        });
                    }
                }
                canvas.set_logical_size(0, 0)?;
                canvas.set_draw_color(Color::RGB(0, 0, 0));
                canvas.clear();
                let frame_filter =
                    netplay_safe_filter(&canvas, cfg.video_filter, net_session.is_some());
                draw_emu_frame(
                    &mut canvas,
                    &mut emu_texture,
                    &texture_creator,
                    &mut overlay_cache,
                    crt_shader.as_mut(),
                    frame_filter,
                    cfg.aspect_mode,
                    cfg.crt_corner_bend,
                )?;
                // Netplay names should come up as soon as the round intro starts;
                // lab/replay overlays still wait for spawned fighters so they do
                // not cover the VS screen.
                let overlay_screen = core
                    .as_ref()
                    .map(|c| {
                        let gstate =
                            memory::peek_u16(c, GSTATE_ADDR, memory::Endian::Little).unwrap_or(0);
                        let p1_hp =
                            memory::peek_u16(c, P1_HP_ADDR, memory::Endian::Little).unwrap_or(0);
                        let s = score::Score::read(c);
                        let match_decided = s.p1_match_wins >= MATCH_WIN_TARGET
                            || s.p2_match_wins >= MATCH_WIN_TARGET;
                        let round_intro_started = gstate != 0
                            && gstate != GS_AMODE
                            && gstate != GS_GAMEOVER
                            && s.round_num > 0;
                        let fighters_spawned = matches!(gstate, GS_FIGHTING | 0x03) && p1_hp > 0;
                        !match_decided
                            && if net_session.is_some() {
                                round_intro_started
                            } else {
                                fighters_spawned
                            }
                    })
                    .unwrap_or(false);
                if overlay_screen
                    && (net_session.is_some()
                        || local_play_mode.is_lab()
                        || match_replay_playback.is_some())
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
                    } else if local_play_mode.is_lab() {
                        Some("Lab")
                    } else {
                        Some("CPU")
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
                if let Some(pb) = match_replay_playback.as_ref() {
                    canvas.set_logical_size(0, 0)?;
                    let (win_w, win_h) = canvas.output_size().unwrap_or((1200, 762));
                    draw_replay_review_overlay(
                        &mut canvas,
                        &mut font,
                        win_w as i32,
                        win_h as i32,
                        pb,
                        replay_review_paused,
                        REPLAY_SPEED_LABELS[replay_review_speed],
                        replay_event_filter,
                        replay_clip_in,
                        replay_clip_out,
                    )
                    .map_err(|e| format!("replay review overlay: {e}"))?;
                    canvas.set_logical_size(LOGICAL_W as u32, LOGICAL_H as u32)?;
                }
                if net_session.is_none()
                    && match_replay_playback.is_none()
                    && local_play_mode.is_lab()
                    && lab_assist_visible
                {
                    canvas.set_logical_size(0, 0)?;
                    let (win_w, win_h) = canvas.output_size().unwrap_or((1200, 762));
                    draw_lab_assist_overlay(
                        &mut canvas,
                        &mut font,
                        win_w as i32,
                        win_h as i32,
                        &input_history,
                        trainer.is_enabled("hitboxes"),
                        trainer.is_enabled("p1_health"),
                        trainer.is_enabled("freeze_timer"),
                        &lab_dummy.status_label(),
                        &lab_reset_slots.active_status_label(),
                        &punish_trainer.status_label(),
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
                if net_stats_visible && net_session.is_some() {
                    canvas.set_logical_size(0, 0)?;
                    let (win_w, win_h) = canvas.output_size().unwrap_or((1200, 762));
                    let ping_label = latest_net_ping_ms.map(|ms| format!("{ms} ms"));
                    let mk2_perf = core.as_ref().and_then(mk2_perf::sample);
                    let detail_rows = net_stats_detail_rows(
                        latest_net_rollback_frames,
                        latest_net_load_count,
                        latest_net_kbps_sent.as_deref(),
                        latest_net_local_frames_behind.as_deref(),
                        latest_net_remote_frames_behind.as_deref(),
                        latest_net_ping_ms,
                        mk2_perf,
                    );
                    draw_net_stats_overlay(
                        &mut canvas,
                        &mut font,
                        win_w as i32,
                        win_h as i32,
                        current_fps,
                        ping_label.as_deref(),
                        "ONLINE",
                        &detail_rows,
                    )
                    .map_err(|e| format!("network stats overlay: {e}"))?;
                    canvas.set_logical_size(LOGICAL_W as u32, LOGICAL_H as u32)?;
                }
                if render_debug_visible {
                    canvas.set_logical_size(0, 0)?;
                    let renderer = renderer_name(&canvas).to_string();
                    draw_render_debug_overlay(
                        &mut canvas,
                        &mut font,
                        current_fps,
                        &renderer,
                        frame_filter,
                        net_session.is_some(),
                    )
                    .map_err(|e| format!("render debug overlay: {e}"))?;
                    canvas.set_logical_size(LOGICAL_W as u32, LOGICAL_H as u32)?;
                }
                if let Some(toast) = toast_payload(&toast) {
                    canvas.set_logical_size(0, 0)?;
                    let (win_w, win_h) = canvas.output_size().unwrap_or((1200, 762));
                    menu::draw_toast(&mut canvas, &mut font, &toast, win_w as i32, win_h as i32)
                        .map_err(|e| format!("toast overlay: {e}"))?;
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
                                    input_delay: cfg.input_delay,
                                    render_profile: cfg.render_profile,
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
                                        &mut lab_reset_slots,
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
                                                    cfg.input_delay,
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
                                            cfg.input_delay,
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
                                        latest_net_ping_ms = None;
                                        latest_net_kbps_sent = None;
                                        latest_net_local_frames_behind = None;
                                        latest_net_remote_frames_behind = None;
                                        latest_net_rollback_frames = 0;
                                        latest_net_load_count = 0;
                                        net_stats_next_frame = 0;
                                        net_frame_counter = 0;
                                        let who = discord_user.as_deref().unwrap_or("Anonymous");
                                        let role = if local_handle == 0 { "P1" } else { "P2" };
                                        discord_webhook::post(
                                            &cfg.discord_webhook_url,
                                            &format!(":crossed_swords: **{who}** ({role}) is in a match - MK2"),
                                        );
                                        state = AppState::Playing;
                                        local_play_mode = LocalPlayMode::Arcade;
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
                    rom_present.check(),
                    discord_user.as_deref(),
                    &main_leaderboard,
                    toast_payload(&toast),
                )
                .map_err(|e| format!("menu draw: {e}"))?;
                if net_stats_visible
                    && matches!(state, AppState::Menu(MenuScreen::Matchmaking { .. }))
                {
                    draw_net_stats_overlay(
                        &mut canvas,
                        &mut font,
                        win_w as i32,
                        win_h as i32,
                        current_fps,
                        None,
                        "QUEUE",
                        &[],
                    )
                    .map_err(|e| format!("network stats overlay: {e}"))?;
                }
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
                    let waiting_for_username = matches!(
                        state,
                        AppState::Menu(menu::MenuScreen::MatchUsername { .. })
                    ) || (username_check_silent
                        && matches!(state, AppState::Menu(menu::MenuScreen::Matchmaking { .. })));
                    if waiting_for_username {
                        match rx.try_recv() {
                            Ok(matchmaking::UsernameCheckUpdate::Available(username)) => {
                                cfg.player_username = username.clone();
                                cfg.player_username_confirmed = true;
                                cfg.player_username_autogenerated = false;
                                config::save(&cfg);
                                start_find_match_queue(
                                    &cfg,
                                    &mut mm_rx,
                                    &mut state,
                                    username,
                                    &mut discord_user,
                                    &mut discord_id,
                                );
                                username_check_rx = None;
                                username_check_silent = false;
                            }
                            Ok(matchmaking::UsernameCheckUpdate::Taken(username)) => {
                                state = AppState::Menu(MenuScreen::MatchUsername {
                                    value: username,
                                    status: "That name is already taken".into(),
                                    checking: false,
                                });
                                username_check_rx = None;
                                username_check_silent = false;
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
                                username_check_silent = false;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                username_check_rx = None;
                                username_check_silent = false;
                                state = AppState::Menu(MenuScreen::MatchUsername {
                                    value: cfg.player_username.clone(),
                                    status: "Username check stopped".into(),
                                    checking: false,
                                });
                            }
                        }
                    } else {
                        username_check_rx = None;
                        username_check_silent = false;
                    }
                }

                if let Some(rx) = &username_gen_rx {
                    match rx.try_recv() {
                        // generate_available_username only ever sends Available — the
                        // chosen name to confirm. Open the confirm screen with it.
                        Ok(matchmaking::UsernameCheckUpdate::Available(name)) => {
                            cfg.player_username = name.clone();
                            cfg.player_username_autogenerated = true;
                            cfg.player_username_confirmed = false;
                            state = AppState::Menu(MenuScreen::MatchUsername {
                                value: name,
                                status: "Confirm this name or type your own".into(),
                                checking: false,
                            });
                            username_gen_rx = None;
                        }
                        Ok(_) => {
                            username_gen_rx = None;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            // Worker ended without a name: fall back to a local one
                            // so the player still reaches the confirm screen.
                            let name = config::default_username();
                            cfg.player_username = name.clone();
                            cfg.player_username_autogenerated = true;
                            cfg.player_username_confirmed = false;
                            state = AppState::Menu(MenuScreen::MatchUsername {
                                value: name,
                                status: "Confirm this name or type your own".into(),
                                checking: false,
                            });
                            username_gen_rx = None;
                        }
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
                                                local_play_mode = LocalPlayMode::Lab;
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
                    rom_present.check(),
                    discord_user.as_deref(),
                    &main_leaderboard,
                    toast_payload(&toast),
                )
                .map_err(|e| format!("menu draw: {e}"))?;
                canvas.present();
                canvas.set_logical_size(menu::LOGICAL_W as u32, menu::LOGICAL_H as u32)?;
            }
        }

        // Discord rate-limits presence updates to a tiny fraction of frame
        // rate; building the snapshot (string formats + RAM reads) 55×/s is
        // wasted work. ~2 Hz keeps presence fresh.
        rpc_pulse = rpc_pulse.wrapping_add(1);
        let rpc_due = rpc_pulse % 28 == 0;
        if let Some(rc) = rpc_client.as_mut().filter(|_| rpc_due) {
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
        // Absolute-deadline pacing: sleep until a fixed deadline and advance it
        // by exactly one frame, so sleep overshoot is reclaimed on the next
        // frame instead of accumulating (relative sleeps ran the game ~2% slow).
        let now = Instant::now();
        if now < next_frame_deadline {
            std::thread::sleep(next_frame_deadline - now);
            next_frame_deadline += frame_duration;
        } else if now - next_frame_deadline > frame_duration * 4 {
            // Fell far behind (window drag, stall) — resync instead of
            // fast-forwarding through the backlog.
            next_frame_deadline = now + frame_duration;
        } else {
            next_frame_deadline += frame_duration;
        }
        fps_sample_frames = fps_sample_frames.saturating_add(1);
        let fps_elapsed = fps_sample_started.elapsed();
        if fps_elapsed >= Duration::from_millis(500) {
            current_fps = Some(fps_sample_frames as f32 / fps_elapsed.as_secs_f32());
            fps_sample_frames = 0;
            fps_sample_started = Instant::now();
        }
    }

    Ok(())
}
