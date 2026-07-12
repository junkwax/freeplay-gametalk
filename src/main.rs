#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod audio_recovery;
mod cli;
mod clip;
mod config;
mod controllers;
mod diag;
mod discord_webhook;
mod doctor;
mod drone;
mod font;
mod fp_ui;
mod frame_timer;
mod ghost;
mod gl_crt;
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
mod native_titlebar_drag;
mod net_set;
mod net_stats_ui;
mod netcore;
mod netplay;
mod png;
mod protocol;
mod relay_socket;
mod render;
mod replay;
mod runahead;
mod replay_upload;
mod retro;
mod rom;
mod rpc;
mod score;
mod session;
mod version;
mod wuname;

use crate::audio_recovery::prepare_game_audio;
use crate::cli::{parse_args, NetMode};
use crate::controllers::{assign_pad, open_initial_controllers, pad_owner, Pads};
use crate::font::Font;
use crate::input::{set_action_source, Bindings, InputSource, Player};
use crate::menu::{AppState, MenuScreen, NavResult, LOGICAL_H, LOGICAL_W};
use crate::menu_input::{capture_rebind, event_to_menu_nav, is_cancel, is_clear, MenuNav};
use crate::net_set::{
    log_completed_net_match, mark_net_set_complete_pending, pending_net_set_expired,
    sync_completed_net_matches,
};
use crate::net_stats_ui::NetStatsUi;
use crate::netcore::{reset_for_netplay, step_netplay_frame, NetRuntime};
use crate::render::{
    build_window_canvas, draw_chat_overlay, draw_emu_frame, draw_fight_overlay,
    draw_lab_assist_overlay, draw_net_stats_overlay, draw_render_debug_overlay,
    draw_replay_review_overlay, ensure_core_loaded, format_probe_result, netplay_safe_filter,
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
use sdl2::render::{BlendMode, Canvas};
use sdl2::surface::Surface;
use sdl2::video::{FullscreenType, Window};
use std::time::{Duration, Instant};

const CHAT_MAX_LINES: usize = 8;

/// Replay "takeover": from the replay viewer, bookmark the current frame and
/// take control of one fighter while the other plays back its recorded inputs,
/// modern-fighting-game style. Retry reloads the moment to try something else.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TakeoverPhase {
    /// 3-2-1 intro, frozen on the moment.
    Countdown,
    /// Human drives their side; the opponent replays recorded inputs.
    Active,
    /// Window elapsed (or recording ran out) — frozen, waiting for retry/exit.
    Done,
}

struct Takeover {
    /// Savestate captured at the takeover frame; reloaded on retry/exit.
    save: Vec<u8>,
    /// Port the human controls.
    human: input::Player,
    /// Replay cursor at the takeover frame, to rewind on retry.
    start_cursor: usize,
    phase: TakeoverPhase,
    countdown: u32,
    frames_left: u32,
}

/// ~3s of 3-2-1 at MK2's ~55 Hz, then a ~20s control window.
const TAKEOVER_COUNTDOWN_FRAMES: u32 = 165;
const TAKEOVER_ACTIVE_FRAMES: u32 = 20 * 55;

fn adopt_packaged_working_dir() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(exe_dir) = exe.parent() else {
        return;
    };
    let package_markers = [
        "config.toml",
        ".env",
        "media/mk2.ttf",
        "fbneo_libretro.dll",
        "fbneo_libretro.so",
        "fbneo_libretro.dylib",
    ];
    if !package_markers
        .iter()
        .any(|marker| exe_dir.join(marker).exists())
    {
        return;
    }
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    if cwd == exe_dir {
        return;
    }
    match std::env::set_current_dir(exe_dir) {
        Ok(()) => println!("[main] working directory set to {}", exe_dir.display()),
        Err(e) => println!(
            "[main] could not set working directory to {}: {e}",
            exe_dir.display()
        ),
    }
}

/// Run the diagnostics ("doctor") and show the report. The previous approach
/// shelled out to `cmd /C start ... cmd /K "<exe>" --doctor`, whose nested
/// quoting around the exe path never ran (and freeplay is a GUI-subsystem app
/// with no console, so its stdout wasn't visible anyway). Instead we run the
/// doctor in a child process that writes a report file, then open the file in
/// the default text viewer. Done on a background thread so the menu doesn't
/// freeze while diagnostics run.
fn launch_debugger() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let report_path = std::env::temp_dir().join("freeplay-doctor.txt");
    std::thread::spawn(move || {
        let _ = std::process::Command::new(&exe)
            .arg("--doctor-report")
            .arg(&report_path)
            .status();
        let _ = open::that(&report_path);
    });
    Ok(())
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
            let dst_idx = ((y + offset_y) as usize * target as usize + (x + offset_x) as usize) * 4;
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

fn adjust_settings_value(
    cfg: &mut config::Config,
    state: &mut AppState,
    toast: &mut Option<(String, Instant)>,
    cursor: usize,
    delta: i8,
    renderer: &str,
) -> bool {
    match cursor {
        5 => {
            if delta < 0 {
                cfg.volume_percent = cfg.volume_percent.saturating_sub(10);
            } else {
                cfg.volume_percent = cfg.volume_percent.saturating_add(10).min(100);
            }
            config::save(cfg);
            if let AppState::Menu(MenuScreen::Settings {
                ref mut volume_percent,
                ..
            }) = state
            {
                *volume_percent = cfg.volume_percent;
            }
            *toast = Some((
                format!("Volume {}%", cfg.volume_percent),
                Instant::now() + Duration::from_millis(1800),
            ));
            true
        }
        6 => {
            cfg.audio_buffer = cfg.audio_buffer.cycle(delta);
            config::save(cfg);
            if let AppState::Menu(MenuScreen::Settings {
                ref mut audio_buffer,
                ..
            }) = state
            {
                *audio_buffer = cfg.audio_buffer;
            }
            *toast = Some((
                format!("Audio Buffer {}", cfg.audio_buffer.label()),
                Instant::now() + Duration::from_millis(1800),
            ));
            true
        }
        7 => {
            cfg.video_filter = cfg.video_filter.cycle(delta);
            config::save(cfg);
            if let AppState::Menu(MenuScreen::Settings {
                ref mut video_filter,
                ..
            }) = state
            {
                *video_filter = cfg.video_filter;
            }
            *toast = Some((
                video_filter_toast_message(cfg.video_filter, renderer),
                Instant::now() + Duration::from_millis(1800),
            ));
            true
        }
        9 => {
            cfg.aspect_mode = cfg.aspect_mode.cycle(delta);
            config::save(cfg);
            if let AppState::Menu(MenuScreen::Settings {
                ref mut aspect_mode,
                ..
            }) = state
            {
                *aspect_mode = cfg.aspect_mode;
            }
            *toast = Some((
                format!("Aspect {}", cfg.aspect_mode.label()),
                Instant::now() + Duration::from_millis(1800),
            ));
            true
        }
        10 => {
            cfg.scorebar_style = cfg.scorebar_style.cycle(delta);
            config::save(cfg);
            if let AppState::Menu(MenuScreen::Settings {
                ref mut scorebar_style,
                ..
            }) = state
            {
                *scorebar_style = cfg.scorebar_style;
            }
            *toast = Some((
                format!("Scorebar {}", cfg.scorebar_style.label()),
                Instant::now() + Duration::from_millis(1800),
            ));
            true
        }
        11 => {
            if delta < 0 {
                cfg.input_delay = cfg.input_delay.saturating_sub(1);
            } else {
                cfg.input_delay = (cfg.input_delay + 1).min(8);
            }
            config::save(cfg);
            if let AppState::Menu(MenuScreen::Settings {
                ref mut input_delay,
                ..
            }) = state
            {
                *input_delay = cfg.input_delay;
            }
            *toast = Some((
                format!("Input Delay {} frames (next match)", cfg.input_delay),
                Instant::now() + Duration::from_millis(1800),
            ));
            true
        }
        15 => {
            cfg.render_profile = cfg.render_profile.cycle(delta);
            config::save(cfg);
            if let AppState::Menu(MenuScreen::Settings {
                ref mut render_profile,
                ..
            }) = state
            {
                *render_profile = cfg.render_profile;
            }
            *toast = Some((
                format!("Render Profile {} (restart)", cfg.render_profile.label()),
                Instant::now() + Duration::from_millis(2200),
            ));
            true
        }
        _ => false,
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

/// Builds the username-claim screen matching whichever UI is currently
/// showing it — `AppState::FpUi(FpScreen::ClaimUsername)` if that's where
/// `state` already is, otherwise legacy's `AppState::Menu(MenuScreen::MatchUsername)`.
/// Used by every username-check transition (submit / taken / error / retry)
/// so both UIs share one round trip through `matchmaking::check_username_available`
/// without duplicating that logic per screen.
fn set_username_screen(state: &mut AppState, value: String, status: String, checking: bool) {
    if matches!(state, AppState::FpUi(fp_ui::FpScreen::ClaimUsername { .. })) {
        *state = AppState::FpUi(fp_ui::FpScreen::ClaimUsername { value, status, checking });
    } else {
        *state = AppState::Menu(MenuScreen::MatchUsername { value, status, checking });
    }
}

/// Same "stay in whichever UI we're already in" pattern as `set_username_screen`,
/// for the matchmaking search screen. Every place that used to hardcode
/// `AppState::Menu(MenuScreen::Matchmaking { .. })` — including several fp_ui
/// `FpResult` handlers (`SendChallenge`, `AcceptChallenge`, `ToggleDiscordConnect`)
/// that forced a drop to the legacy screen mid-fp_ui-session — now goes through
/// here instead.
fn set_matchmaking_screen(state: &mut AppState, status: String) {
    if matches!(state, AppState::FpUi(_)) {
        *state = AppState::FpUi(fp_ui::FpScreen::Matchmaking { status });
    } else {
        *state = AppState::Menu(MenuScreen::Matchmaking { status });
    }
}

fn is_matchmaking_screen(state: &AppState) -> bool {
    matches!(
        state,
        AppState::Menu(MenuScreen::Matchmaking { .. })
            | AppState::FpUi(fp_ui::FpScreen::Matchmaking { .. })
            | AppState::FpUi(fp_ui::FpScreen::Lobby { quick_match_status: Some(_), .. })
            | AppState::FpUi(fp_ui::FpScreen::DiscordConnect { .. })
    )
}

/// The netplay-failure report screen: the native Connection Failed card
/// when the new UI is on, legacy `TestResult` otherwise. Both failure
/// sites auto-submit an incident report just before calling this, so the
/// appended line is a real fact, not reassurance theater — surfaced on
/// the native card (legacy's fixed layout has no room for it).
fn connection_failed_state(new_ui: bool, mut lines: Vec<String>) -> AppState {
    if new_ui {
        lines.push("OK Incident report submitted automatically".into());
        AppState::FpUi(fp_ui::FpScreen::ConnectionFailed { lines })
    } else {
        AppState::Menu(MenuScreen::TestResult { lines })
    }
}

/// A legacy `TextEdit` capture that was opened *from* an fp_ui screen —
/// rendered with the native on-screen keyboard (`fp_ui::text_entry`) over
/// the dimmed parent screen instead of legacy's full-screen editor, and
/// eligible for controller-driven key-grid input. The state machine itself
/// (value filtering, commit, `came_from` round trip) is identical either
/// way.
fn is_fp_text_edit(state: &AppState) -> bool {
    matches!(
        state,
        AppState::Menu(MenuScreen::TextEdit { came_from, .. })
            if matches!(**came_from, AppState::FpUi(_))
    )
}

/// States that live outside `FpScreen` but get native fp_ui *rendering*:
/// TextEdit/Rebinding captures whose `came_from` is an fp screen (modal over
/// the dimmed parent), and Spectate while still waiting for a first status
/// frame (native "connecting to live match" — the viewer that takes over
/// once frames flow stays legacy).
fn fp_native_overlay(state: &AppState, new_ui: bool) -> bool {
    match state {
        AppState::Menu(MenuScreen::TextEdit { came_from, .. })
        | AppState::Rebinding { came_from, .. } => matches!(**came_from, AppState::FpUi(_)),
        AppState::Menu(MenuScreen::Spectate { status, .. }) => new_ui && status.frame.is_none(),
        _ => false,
    }
}

/// Quick Match specifically stays on `FpScreen::Lobby` through the search
/// (per the mockup's own Quick Match tab, which shows its radar/searching
/// state inline rather than navigating away) instead of using the separate
/// `FpScreen::Matchmaking` screen `set_matchmaking_screen` sends every other
/// matchmaking trigger to. If `state` isn't already `FpScreen::Lobby` (the
/// first-time username-claim path leaves `FpScreen::ClaimUsername` and calls
/// this once the check completes), land on a fresh Quick Match tab rather
/// than losing the search entirely.
fn set_quick_match_searching(state: &mut AppState, status: String) {
    match state {
        AppState::FpUi(fp_ui::FpScreen::Lobby { quick_match_status, .. }) => {
            *quick_match_status = Some(status);
        }
        AppState::FpUi(_) => {
            let mut lobby = fp_ui::FpScreen::lobby();
            if let fp_ui::FpScreen::Lobby { quick_match_status, .. } = &mut lobby {
                *quick_match_status = Some(status);
            }
            *state = AppState::FpUi(lobby);
        }
        _ => {
            *state = AppState::Menu(MenuScreen::Matchmaking { status });
        }
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
        set_quick_match_searching(state, format!("Entering queue as {discord_name}"));
    } else {
        matchmaking::set_guest_profile(
            username.clone(),
            cfg.stats_email.clone(),
            cfg.guest_device_id.clone(),
        );
        matchmaking::start_guest(tx);
        set_quick_match_searching(state, format!("Entering queue as {username}"));
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

/// Enter replay takeover at the current review frame: snapshot the emulator
/// state and arm the 3-2-1 countdown for `human`'s side.
fn start_replay_takeover(
    core: &Option<retro::Core>,
    playback: &Option<match_replay::Playback>,
    takeover: &mut Option<Takeover>,
    paused: &mut bool,
    human: input::Player,
    toast: &mut Option<(String, Instant)>,
) {
    if let (Some(c), Some(pb)) = (core.as_ref(), playback.as_ref()) {
        let Some(save) = c.save_state() else {
            *toast = Some((
                "Couldn't snapshot this moment".into(),
                Instant::now() + Duration::from_millis(1600),
            ));
            return;
        };
        *takeover = Some(Takeover {
            save,
            human,
            start_cursor: pb.current_frame(),
            phase: TakeoverPhase::Countdown,
            countdown: TAKEOVER_COUNTDOWN_FRAMES,
            frames_left: TAKEOVER_ACTIVE_FRAMES,
        });
        *paused = true;
        let side = if human == input::Player::P1 { "P1" } else { "P2" };
        *toast = Some((
            format!("Taking over {side}"),
            Instant::now() + Duration::from_millis(1400),
        ));
    }
}

/// Reload the takeover moment (retry or exit): restore the savestate and rewind
/// the replay cursor to the takeover frame.
fn reload_takeover_moment(
    core: &Option<retro::Core>,
    playback: &mut Option<match_replay::Playback>,
    tk: &Takeover,
) {
    if let (Some(c), Some(pb)) = (core.as_ref(), playback.as_mut()) {
        c.load_state(&tk.save);
        pb.set_cursor(tk.start_cursor);
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
        clear_audio_buffer();
    }
    input::clear_all_inputs();
    {
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
    let pb = match_replay::Playback::load(path).map_err(|e| format!("replay load failed: {e}"))?;
    if !pb.prime(core) {
        return Err("replay state rejected".into());
    }
    input::clear_all_inputs();
    clear_audio_buffer();
    println!(
        "[replay] Reviewing {} frames: {} vs {} (markers build during playback, {} bookmarks)",
        pb.frame_count(),
        pb.p1_name(),
        pb.p2_name(),
        pb.bookmarks().len()
    );
    Ok(pb)
}

fn download_remote_replay(url: &str) -> Result<String, String> {
    const MAX_REMOTE_REPLAY_BYTES: usize = 64 * 1024 * 1024;

    if !url.starts_with("https://") {
        return Err("Replay links must use HTTPS".into());
    }
    let bytes = matchmaking::http_get_bytes(url).map_err(|e| format!("download failed: {e}"))?;
    if bytes.len() > MAX_REMOTE_REPLAY_BYTES {
        return Err(format!(
            "downloaded replay is too large ({} MB)",
            bytes.len() / (1024 * 1024)
        ));
    }

    std::fs::create_dir_all("replays").map_err(|e| format!("create replays folder: {e}"))?;
    let path = std::path::Path::new("replays").join(remote_replay_filename(url));
    std::fs::write(&path, &bytes).map_err(|e| format!("save replay: {e}"))?;

    if let Err(e) = match_replay::Playback::load(&path) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("downloaded replay is invalid: {e}"));
    }

    Ok(path.to_string_lossy().into_owned())
}

fn remote_replay_filename(url: &str) -> String {
    let raw_path = url.split(['?', '#']).next().unwrap_or(url);
    let raw_name = raw_path.rsplit('/').next().unwrap_or("");
    let mut safe_name: String = raw_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '.' | '-' | '_'))
        .take(96)
        .collect();
    if !safe_name.to_ascii_lowercase().ends_with(".ncrp") || safe_name.len() <= ".ncrp".len() {
        safe_name = format!("remote_{:016x}.ncrp", stable_url_hash(url));
    }
    safe_name
}

fn stable_url_hash(url: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in url.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Scan `ghosts/*.ncgh` (newest-first by the timestamp embedded in the
/// filename) plus a legacy bare `ghost.bin`, if present. Shared by both the
/// legacy `MenuScreen::GhostSelect` and the native `fp_ui::FpScreen::
/// GhostSelect` screens — same local data, two different chooser UIs.
fn scan_local_ghost_entries() -> Vec<menu::GhostEntry> {
    let mut entries = Vec::new();
    if let Ok(dir) = std::fs::read_dir("ghosts") {
        let mut files: Vec<(String, String)> = dir
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "ncgh").unwrap_or(false))
            .map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let path = e.path().to_string_lossy().to_string();
                (name, path)
            })
            .collect();
        // Sort by timestamp descending (latest first)
        fn extract_ts(name: &str) -> u64 {
            let base = if name.ends_with(".ncgh") { &name[..name.len() - 5] } else { name };
            base.rsplit('_').next().and_then(|s| s.parse().ok()).unwrap_or(0)
        }
        files.sort_by(|a, b| extract_ts(&b.0).cmp(&extract_ts(&a.0)));
        entries = files
            .into_iter()
            .map(|(name, path)| {
                let frame_count = ghost::read_ncgh_frame_count(std::path::Path::new(&path)).unwrap_or(0);
                menu::GhostEntry::Local { filename: name, path, frame_count }
            })
            .collect();
    }
    if std::path::Path::new("ghost.bin").exists() {
        let frame_count = ghost::read_ncgh_frame_count(std::path::Path::new("ghost.bin")).unwrap_or(0);
        entries.push(menu::GhostEntry::Local { filename: "ghost.bin".into(), path: "ghost.bin".into(), frame_count });
    }
    entries
}

fn write_replay_summary(
    replay_path: &std::path::Path,
    p1_name: &str,
    p2_name: &str,
    p1_score: u32,
    p2_score: u32,
    frames: u32,
    completed_matches: u32,
    session_id: Option<&str>,
    reason: &str,
    completed_set: bool,
) -> std::io::Result<()> {
    let recorded_unix = replay_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.split('_').next())
        .and_then(|raw| raw.parse::<u64>().ok());
    let winner = if p1_score > p2_score {
        p1_name
    } else if p2_score > p1_score {
        p2_name
    } else {
        ""
    };
    let filename = replay_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("replay.ncrp");
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str(&format!("  \"file\": \"{}\",\n", json_escape(filename)));
    out.push_str(&format!("  \"p1\": \"{}\",\n", json_escape(p1_name)));
    out.push_str(&format!("  \"p2\": \"{}\",\n", json_escape(p2_name)));
    out.push_str(&format!("  \"p1_score\": {},\n", p1_score));
    out.push_str(&format!("  \"p2_score\": {},\n", p2_score));
    out.push_str(&format!("  \"winner\": \"{}\",\n", json_escape(winner)));
    out.push_str(&format!("  \"frames\": {},\n", frames));
    out.push_str(&format!(
        "  \"duration\": \"{}\",\n",
        json_escape(&format_duration_frames(frames))
    ));
    if let Some(recorded_unix) = recorded_unix {
        out.push_str(&format!("  \"recorded_unix\": {},\n", recorded_unix));
    }
    out.push_str(&format!(
        "  \"completed_matches\": {},\n",
        completed_matches
    ));
    out.push_str(&format!("  \"completed_set\": {},\n", completed_set));
    out.push_str(&format!("  \"reason\": \"{}\"", json_escape(reason)));
    if let Some(session_id) = session_id {
        out.push_str(&format!(
            ",\n  \"session_id\": \"{}\"",
            json_escape(session_id)
        ));
    }
    out.push_str("\n}\n");
    std::fs::write(replay_path.with_extension("ncrp.json"), out)
}

fn format_duration_frames(frames: u32) -> String {
    let total_seconds = (frames as u64 + 27) / 55;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if minutes >= 60 {
        let hours = minutes / 60;
        let minutes = minutes % 60;
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn inferred_match_over(now: score::Score, target: u16) -> Option<score::ScoreEvent> {
    if now.p1_match_wins >= target && now.p1_match_wins > now.p2_match_wins {
        Some(score::ScoreEvent::MatchOver {
            winner: 1,
            p1_wins: now.p1_match_wins,
            p2_wins: now.p2_match_wins,
        })
    } else if now.p2_match_wins >= target && now.p2_match_wins > now.p1_match_wins {
        Some(score::ScoreEvent::MatchOver {
            winner: 2,
            p1_wins: now.p1_match_wins,
            p2_wins: now.p2_match_wins,
        })
    } else {
        None
    }
}

fn set_netplay_window_chrome(canvas: &mut Canvas<Window>, active: bool) {
    if canvas.window().fullscreen_state() == FullscreenType::Off {
        canvas.window_mut().set_bordered(true);
    }
    native_titlebar_drag::set_enabled(active);
}

#[allow(clippy::too_many_arguments)]
fn shutdown_local_runtime_for_netplay(
    core: Option<&retro::Core>,
    audio_queue: Option<&AudioQueue<i16>>,
    trainer: &mut memory::PokeList,
    lab_reset_slots: &mut lab::ResetSlots,
    lab_dummy: &mut lab::DummyController,
    punish_trainer: &mut lab::PunishTrainer,
    damage_tracker: &mut lab::DamageTracker,
    ghost_playback: &mut Option<ghost::Playback>,
    ghost_recording: &mut Option<ghost::Recording>,
    drone_runner: &mut Option<drone::DroneRunner>,
    ghost_port_mask: &mut u8,
    match_replay_playback: &mut Option<match_replay::Playback>,
    match_replay_recording: &mut Option<match_replay::Recording>,
    replay_review_paused: &mut bool,
    replay_review_tick: &mut u64,
    replay_clip_in: &mut Option<usize>,
    replay_clip_out: &mut Option<usize>,
    clip_recorder: &mut Option<clip::ClipRecorder>,
    input_history: &mut input_history::InputHistory,
    score_tracker: &mut score::ScoreTracker,
    local_play_mode: &mut LocalPlayMode,
    session_p1_wins: &mut u32,
    session_p2_wins: &mut u32,
    auto_start_done: &mut bool,
    auto_start_frame: &mut u32,
    audio_tail_sample: &mut Option<(i16, i16)>,
    mut net_log: Option<&mut std::fs::File>,
    reason: &str,
) {
    let line = format!(
        "[net] shutting down local {:?} runtime before online start: {reason}",
        *local_play_mode
    );
    println!("{line}");
    if let Some(f) = net_log.as_mut() {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }

    *local_play_mode = LocalPlayMode::Arcade;
    *match_replay_playback = None;
    *match_replay_recording = None;
    *replay_review_paused = false;
    *replay_review_tick = 0;
    *replay_clip_in = None;
    *replay_clip_out = None;
    *ghost_playback = None;
    *ghost_recording = None;
    *drone_runner = None;
    *ghost_port_mask = 0b11;
    lab_reset_slots.clear();
    lab_dummy.clear_loop();
    punish_trainer.reset_stats();
    damage_tracker.reset_stats();
    score_tracker.reset();
    *session_p1_wins = 0;
    *session_p2_wins = 0;
    *auto_start_done = true;
    *auto_start_frame = 0;
    input_history.clear();
    input::clear_all_inputs();

    if let Some(recorder) = clip_recorder.take() {
        let message = finish_clip_recording(recorder);
        println!("[clip] {message}");
    }
    if let Some(q) = audio_queue {
        q.clear();
    }
    retro::clear_audio_buffer();
    *audio_tail_sample = None;

    if let Some(c) = core {
        // Prefer reloading the pristine boot savestate over retro_reset: a soft
        // reset leaves a prior Lab/Arcade session bleeding into the match (and
        // desynced the two peers, since each started from different leftover
        // state). The boot state is the same canonical attract-mode frame on
        // both clients. Fall back to reset if it was never captured.
        let how = match netcore::clean_boot_state() {
            Some(boot) if c.load_state(&boot) => "loaded clean boot state",
            _ => {
                c.reset();
                "reset (no boot state)"
            }
        };
        reset_for_netplay(c, trainer, lab_reset_slots, ghost_playback, ghost_recording);
        if let Some(f) = net_log.as_mut() {
            use std::io::Write;
            let _ = writeln!(f, "[net] core {how} for canonical online start");
        }
        println!("[net] core {how} for canonical online start");
    }
}

fn attach_relay_diagnostics(
    inc: &mut incident::Incident,
    relay_chat: Option<&relay_socket::RelayChatHandle>,
) {
    if let Some(chat) = relay_chat {
        let diag = chat.diagnostics();
        inc.relay_registered = Some(diag.registered);
        inc.relay_peer_ready = Some(diag.peer_ready);
        inc.relay_data_received = Some(diag.data_received);
    }
}

fn lobby_format_to_menu(format: matchmaking::LobbyMatchFormat) -> menu::ChallengeFormat {
    match format {
        matchmaking::LobbyMatchFormat::UnrankedVs => menu::ChallengeFormat::UnrankedVs,
        matchmaking::LobbyMatchFormat::RankedFt3 => menu::ChallengeFormat::RankedFt3,
        matchmaking::LobbyMatchFormat::RankedFt5 => menu::ChallengeFormat::RankedFt5,
        matchmaking::LobbyMatchFormat::RankedFt10 => menu::ChallengeFormat::RankedFt10,
    }
}

fn lobby_room_to_preview(room: matchmaking::LobbyRoom) -> menu::LobbyPreview {
    menu::LobbyPreview {
        id: room.id,
        name: room.name,
        host: room.host_username,
        format: lobby_format_to_menu(room.format),
        players: room.players,
        private: room.private,
        status: room.status,
    }
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

/// Shared by `refresh_replay_select` (legacy) and the native
/// `fp_ui::FpScreen::ReplaySelect` — same local `.ncrp` scan, two different
/// chooser UIs.
fn scan_local_replay_entries() -> Vec<menu::ReplayEntry> {
    match_replay::list_online_replays()
        .into_iter()
        .map(|meta| menu::ReplayEntry {
            filename: meta.filename,
            path: meta.path,
            remote_url: None,
            p1_name: meta.p1_name,
            p2_name: meta.p2_name,
            p1_score: meta.p1_score,
            p2_score: meta.p2_score,
            winner: meta.winner,
            frame_count: meta.frame_count,
            duration: meta.duration,
            recorded_at: String::new(),
            note: meta.note,
            bookmark_count: meta.bookmark_count,
        })
        .collect()
}

/// Exiting replay review (Escape/Back, controller B, or natural playback
/// completion) used to always land on the *legacy* ReplaySelect regardless
/// of `cfg.new_ui` — the fp_ui Replays screen has no `came_from` to return
/// to (review reuses `AppState::Playing`, not a dedicated state), so this
/// picks the right screen shape to return to instead.
fn replay_select_exit_state(new_ui: bool, status: impl Into<String>) -> AppState {
    let status = Some(status.into());
    if new_ui {
        AppState::FpUi(fp_ui::FpScreen::ReplaySelect { cursor: 0, entries: Vec::new(), status })
    } else {
        AppState::Menu(MenuScreen::ReplaySelect { cursor: 0, entries: Vec::new(), status })
    }
}

fn refresh_replay_select(state: &mut AppState, status: Option<String>) {
    let cursor_entries_status = match state {
        AppState::Menu(MenuScreen::ReplaySelect { cursor, entries, status: screen_status }) => {
            Some((cursor, entries, screen_status))
        }
        AppState::FpUi(fp_ui::FpScreen::ReplaySelect { cursor, entries, status: screen_status }) => {
            Some((cursor, entries, screen_status))
        }
        _ => None,
    };
    if let Some((cursor, entries, screen_status)) = cursor_entries_status {
        *entries = scan_local_replay_entries();
        if entries.is_empty() {
            *cursor = 0;
        } else if *cursor >= entries.len() {
            *cursor = entries.len() - 1;
        }
        *screen_status = status.or_else(|| {
            if entries.is_empty() {
                Some("No local replays found".into())
            } else {
                None
            }
        });
    }
}

/// True while the native Test Connection category is showing and actively
/// focused (not sidebar-driven category switching) — real hardware-keyboard
/// text input should be active into `test_conn_address`, same mechanism
/// legacy's `MenuScreen::TestIp { editing: true, .. }` already uses for the
/// same field.
fn is_fp_test_conn_editing(state: &AppState) -> bool {
    matches!(
        state,
        AppState::FpUi(fp_ui::FpScreen::Settings { cat, sidebar_focus: false, .. })
            if *cat == fp_ui::settings::TEST_CONN_CAT_INDEX
    )
}

fn set_replay_select_status(state: &mut AppState, status: impl Into<String>) {
    let status = status.into();
    if let AppState::FpUi(fp_ui::FpScreen::ReplaySelect { status: screen_status, .. }) = state {
        *screen_status = Some(status.clone());
    }
    if let AppState::Menu(MenuScreen::ReplaySelect {
        status: screen_status,
        ..
    }) = state
    {
        *screen_status = Some(status.into());
    }
}

fn selected_replay_entry(state: &AppState) -> Option<menu::ReplayEntry> {
    if let AppState::Menu(MenuScreen::ReplaySelect { cursor, entries, .. })
    | AppState::FpUi(fp_ui::FpScreen::ReplaySelect { cursor, entries, .. }) = state
    {
        entries.get(*cursor).cloned()
    } else {
        None
    }
}

fn handle_replay_select_shortcut(event: &Event, state: &mut AppState) -> bool {
    if !matches!(
        state,
        AppState::Menu(MenuScreen::ReplaySelect { .. }) | AppState::FpUi(fp_ui::FpScreen::ReplaySelect { .. })
    ) {
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
            if entry.remote_url.is_some() {
                set_replay_select_status(state, "Public replays stay in the archive");
                return true;
            }
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
            if entry.remote_url.is_some() {
                set_replay_select_status(state, "Notes are local replays only");
                return true;
            }
            // `came_from` is the exact screen (native or legacy) editing
            // began from — `came_from: Box<AppState>` (see
            // `MenuScreen::TextEdit`'s doc comment) means this returns to
            // whichever one it actually was on commit/cancel, not always a
            // legacy fallback.
            let came_from = state.clone();
            *state = AppState::Menu(MenuScreen::TextEdit {
                title: "REPLAY NOTE".into(),
                label: format!("{} vs {}", entry.p1_name, entry.p2_name),
                value: entry.note,
                field: menu::EditField::ReplayNote { path: entry.path },
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
        let samples = drain_audio_buffer();
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

/// Catches native crashes (access violations, stack overflows, ...) that
/// bypass Rust's panic machinery entirely — an `unwrap`/`panic!` always
/// leaves a trace via `install_panic_incident_hook`'s hook, but a bad
/// pointer dereference inside `unsafe` FFI code (the libretro core, GL
/// calls in `gl_crt.rs`, ...) just terminates the process with nothing
/// printed and no `crash_*.log` written — exactly what made an earlier
/// "settings crash" report (traced to changing the CRT shader filter)
/// impossible to diagnose from the logs alone. `SetUnhandledExceptionFilter`
/// is the OS-level catch-all below any Rust-level handling; it runs on the
/// crashing thread just before Windows tears the process down, so a plain
/// synchronous file write here is safe (this is not a POSIX signal handler
/// with async-signal-safety constraints).
#[cfg(windows)]
fn install_native_crash_handler() {
    use std::ffi::c_void;

    #[repr(C)]
    struct ExceptionRecord {
        exception_code: u32,
        _exception_flags: u32,
        _exception_record: *mut c_void,
        exception_address: *mut c_void,
        _number_parameters: u32,
        _exception_information: [usize; 15],
    }

    #[repr(C)]
    struct ExceptionPointers {
        exception_record: *mut ExceptionRecord,
        _context_record: *mut c_void,
    }

    const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x2;
    const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x4;

    extern "system" {
        fn SetUnhandledExceptionFilter(
            filter: unsafe extern "system" fn(*mut ExceptionPointers) -> i32,
        ) -> *mut c_void;
        fn GetModuleHandleExW(flags: u32, module_name: *const u16, module: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, filename: *mut u16, size: u32) -> u32;
    }

    fn module_at(addr: *mut c_void) -> String {
        unsafe {
            let mut handle: *mut c_void = std::ptr::null_mut();
            let flags = GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS;
            if GetModuleHandleExW(flags, addr as *const u16, &mut handle) == 0 || handle.is_null() {
                return "<unknown module>".to_string();
            }
            let mut buf = [0u16; 512];
            let len = GetModuleFileNameW(handle, buf.as_mut_ptr(), buf.len() as u32);
            if len == 0 {
                return "<unknown module>".to_string();
            }
            String::from_utf16_lossy(&buf[..len as usize])
        }
    }

    fn exception_name(code: u32) -> &'static str {
        match code {
            0xC0000005 => "ACCESS_VIOLATION",
            0xC00000FD => "STACK_OVERFLOW",
            0xC0000094 => "INT_DIVIDE_BY_ZERO",
            0x80000003 => "BREAKPOINT",
            0xC000001D => "ILLEGAL_INSTRUCTION",
            _ => "UNKNOWN",
        }
    }

    unsafe extern "system" fn handler(info: *mut ExceptionPointers) -> i32 {
        const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
        if let Some(info) = info.as_ref() {
            if let Some(rec) = info.exception_record.as_ref() {
                let code = rec.exception_code;
                let addr = rec.exception_address;
                let module = module_at(addr);
                let unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let path = format!("crash_native_{unix}.log");
                use std::io::Write;
                if let Ok(mut f) = std::fs::File::create(&path) {
                    let _ = writeln!(f, "Freeplay native crash report");
                    let _ = writeln!(f, "version: {}", version::footer_string());
                    let _ = writeln!(
                        f,
                        "exception: {} (0x{code:08X}) at address {addr:?}",
                        exception_name(code)
                    );
                    let _ = writeln!(f, "faulting module: {module}");
                    let _ = writeln!(
                        f,
                        "\nPlease attach this file (and freeplay-net.log if present) to an issue at"
                    );
                    let _ = writeln!(f, "https://github.com/junkwax/freeplay-gametalk/issues");
                }
            }
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    unsafe {
        SetUnhandledExceptionFilter(handler);
    }
}

#[cfg(not(windows))]
fn install_native_crash_handler() {}

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

/// `gl_crt.rs`'s CRT shader mixes a GLSL `#version 120` program with legacy
/// immediate-mode calls (`glBegin`/`glVertex2f`/`glTexCoord2f`/`glEnd`) and
/// never requested a specific GL context profile — left to the driver's
/// default, which on some GPU/driver stacks is (or negotiates to) a core
/// profile that has *removed* those immediate-mode entry points. Calling
/// through them there is undefined behavior at the driver level: an access
/// violation with no Rust panic and no `crash_*.log`, since it happens
/// entirely inside the FFI call — this was the root cause behind a
/// "changing shader settings crashes the game" report that left no trace
/// to investigate. Explicitly requesting a compatibility profile (and the
/// 2.1 version matching `#version 120`) keeps those calls legal; if the
/// driver can't honor it, window/context creation fails immediately and
/// visibly instead of crashing on the first shader-rendered frame.
fn request_compat_gl_profile(video_subsystem: &sdl2::VideoSubsystem) {
    let gl_attr = video_subsystem.gl_attr();
    gl_attr.set_context_profile(sdl2::video::GLProfile::Compatibility);
    gl_attr.set_context_version(2, 1);
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
        request_compat_gl_profile(&video_subsystem);
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

fn run_core_probe() -> Result<(), Box<dyn std::error::Error>> {
    log::init("core_probe");
    dlog!("boot", "core_probe");

    let rom_path = rom::find_rom_zip_string()
        .ok_or_else(|| "ROM zip not found next to the executable or in roms\\".to_string())?;
    let core_path = render::fbneo_core_path()
        .ok_or_else(|| "FBNeo core not found next to the executable or in cores\\".to_string())?;

    println!("[core-probe] rom={rom_path}");
    println!("[core-probe] core={core_path}");
    dlog!("retro", "core_probe resolved rom zip={rom_path}");
    dlog!("retro", "core_probe resolved fbneo core={core_path}");

    retro::set_silent(true);
    let probe = (|| -> Result<(), Box<dyn std::error::Error>> {
        let core = unsafe { retro::load(&core_path, &rom_path)? };
        for _ in 0..12 {
            unsafe { (core.run)() };
        }
        Ok(())
    })();
    retro::set_silent(false);
    probe?;

    println!("[core-probe] ok");
    dlog!("retro", "core_probe ok");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    adopt_packaged_working_dir();

    if cli::render_probe_requested() {
        return run_render_probe();
    }

    if cli::core_probe_requested() {
        return run_core_probe();
    }

    if let Some(report_path) = cli::doctor_report_path() {
        let cfg = config::load();
        config::set_signaling_url(cfg.signaling_url.clone());
        std::process::exit(doctor::run_report(&report_path));
    }

    if cli::doctor_requested() {
        let cfg = config::load();
        config::set_signaling_url(cfg.signaling_url.clone());
        std::process::exit(doctor::run());
    }

    let _timer_resolution = frame_timer::TimerResolution::request_1ms();

    let net_mode = parse_args();
    let log_tag = match &net_mode {
        NetMode::Local => "local".to_string(),
        NetMode::P2P { player, .. } => format!("p{}", player + 1),
    };
    log::init(&log_tag);
    dlog!("boot", "net_mode={net_mode:?}");
    println!("Net mode: {net_mode:?}");

    protocol::register_uri_scheme();

    let mut startup_replay_url: Option<String> = None;
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
                protocol::XbandUri::Replay { url } => {
                    println!("[main] xband:// deep link: replay");
                    startup_replay_url = Some(url);
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
        request_compat_gl_profile(&video_subsystem);
    }
    let mut window = window_builder.build()?;
    set_app_window_icon(&mut window);
    let requested_render_profile = cfg.render_profile;
    let mut canvas = build_window_canvas(
        window,
        cfg.render_profile,
        cfg.video_filter.uses_opengl_shader(),
    )?;
    let _native_titlebar_drag = match native_titlebar_drag::install(canvas.window()) {
        Ok(guard) => Some(guard),
        Err(e) => {
            println!("[window] native titlebar drag shim unavailable: {e}");
            None
        }
    };
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
    let mut fp_fonts = ttf_ctx
        .as_ref()
        .map(|ctx| font::FpFontCache::new(&texture_creator, ctx));
    if cfg.new_ui && fp_fonts.is_none() {
        println!("[fp_ui] SDL2_ttf unavailable; falling back to the legacy UI despite new_ui=true");
    }

    let mut event_pump = sdl_context.event_pump()?;

    if cfg.fullscreen {
        let _ = canvas.window_mut().set_fullscreen(FullscreenType::Desktop);
    }
    config::set_signaling_url(cfg.signaling_url.clone());
    crate::rpc::set_discord_client_id(cfg.discord_client_id.clone());
    incident::set_guest_device_id(cfg.guest_device_id.clone());
    install_panic_incident_hook();
    install_native_crash_handler();
    let mut state = menu::main_menu_state(cfg.new_ui);
    // Debug: `--test-screen online:chat` (or :players/:lobbies/:watch/:play)
    // jumps straight into a hub section with sample data so layout/fonts can be
    // checked without the live server. `--test-osk` also shows the chat keyboard.
    let test_args: Vec<String> = std::env::args().collect();
    let mut test_force_pad = false;
    for i in 0..test_args.len() {
        let val = if test_args[i] == "--test-screen" {
            test_args.get(i + 1).cloned()
        } else {
            test_args[i]
                .strip_prefix("--test-screen=")
                .map(str::to_string)
        };
        if let Some(name) = val {
            if let Some(s) = menu::test_state(&name) {
                state = s;
                println!("[test] jumped to screen: {name}");
            }
        }
        if test_args[i] == "--test-osk" {
            test_force_pad = true;
        }
    }
    let mut rom_present = rom::PresenceCache::new();

    let mut discord_user: Option<String> = matchmaking::username_from_cached_token();
    let mut discord_id: Option<String> = matchmaking::discord_id_from_cached_token();
    let mut score_tracker = score::ScoreTracker::new();
    let mut local_runahead = runahead::Runahead::new();

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
    let mut replay_takeover: Option<Takeover> = None;
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
    const NETPLAY_SET_COMPLETE_GRACE_FRAMES: u32 = 55 * 25;
    let mut net_match_count: u32 = 0;
    // Games needed to complete the current set. Default best-of for find-match /
    // challenges; king-of-the-hill lobbies override this to 1 (FT1 — winner of a
    // single game stays) so the queue rotates after every game.
    let mut net_match_limit: u32 = NETPLAY_MATCH_LIMIT;
    let mut ranked_match_index: u32 = 0;
    let mut net_in_fight: bool = false;
    let mut net_set_complete_pending_frame: Option<u32> = None;
    let mut net_teardown_reason: Option<String> = None;
    // When a netplay session was launched from a king-of-the-hill lobby, this
    // holds the lobby id so we return to the lobby screen (winner stays / loser
    // re-queues) instead of the normal SessionEnded screen.
    let mut lobby_return: Option<String> = None;
    let mut net_frames_since_progress: u32 = 0;
    let mut net_log: Option<std::fs::File> = None;
    let mut net_runtime = NetRuntime::default();
    let mut net_stats_visible = false;
    let mut net_stats = NetStatsUi::default();
    // Grid cursor for the native on-screen keyboard (`fp_ui::text_entry`) —
    // a loop-local rather than a `MenuScreen::TextEdit` field since only one
    // edit can be active at a time; persists across edits, which is
    // harmless (the cursor just stays where the player left it).
    let mut fp_osk: (usize, usize) = (0, 0);
    let mut audio_tail_sample: Option<(i16, i16)> = None;
    let mut render_debug_visible = false;
    let mut net_spectate_next: u32 = 165; // ~3s
    let mut net_frame_counter: u32 = 0;
    // One-shot per netplay session: logs the RAM values the score-bar overlay
    // depends on, so a missing bar can be traced to gstate/hp/RAM availability.
    let mut net_overlay_diagnosed = false;
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
    let mut net_transport_path: Option<&'static str> = None;
    let mut chat_open = false;
    let mut chat_draft = String::new();
    let mut chat_lines: Vec<String> = Vec::new();

    let mut mm_rx: Option<std::sync::mpsc::Receiver<matchmaking::Update>> = None;
    let mut username_check_rx: Option<std::sync::mpsc::Receiver<matchmaking::UsernameCheckUpdate>> =
        None;
    let mut username_check_silent = false;
    let mut username_check_started_at: Option<Instant> = None;
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
    // Fetched unconditionally at startup, same reasoning as `main_leaderboard`
    // above — fp_ui's Main Menu "YOUR STATS"/"LAST MATCH" panels need the
    // current player's own record regardless of which screen is open, unlike
    // the on-demand fetch `NavResult::OpenProfile` triggers only while the
    // dedicated Profile screen is active (polled separately below).
    let mut main_profile_rx: Option<std::sync::mpsc::Receiver<matchmaking::ProfileUpdate>> = None;
    let mut main_profile = {
        let profile_id = discord_id
            .clone()
            .or_else(matchmaking::discord_id_from_cached_token)
            .or_else(|| {
                matchmaking::guest_player_id(&cfg.player_username, &cfg.stats_email, &cfg.guest_device_id)
            });
        match profile_id {
            Some(did) if !cfg.stats_url.is_empty() => {
                let display_name = discord_user.clone().unwrap_or_else(|| cfg.player_username.clone());
                let (tx, rx) = std::sync::mpsc::channel();
                main_profile_rx = Some(rx);
                matchmaking::fetch_profile(cfg.stats_url.clone(), did, display_name, tx);
                menu::ProfileScreenState::Loading
            }
            Some(_) => menu::ProfileScreenState::Error("stats_url not configured".into()),
            None => menu::ProfileScreenState::NotLoggedIn,
        }
    };
    let mut avatar_rx: Option<std::sync::mpsc::Receiver<Vec<u8>>> = None;
    let mut ghost_list_rx: Option<std::sync::mpsc::Receiver<matchmaking::GhostListUpdate>> = None;
    let mut ghost_download_rx: Option<std::sync::mpsc::Receiver<matchmaking::GhostDownloadUpdate>> =
        None;
    let mut public_replay_rx: Option<std::sync::mpsc::Receiver<matchmaking::PublicReplayUpdate>> =
        None;
    // Dedicated to the Main Menu's LAST MATCH card falling back to the
    // public replay index when no local `.ncrp` matches — kept separate
    // from `public_replay_rx` above (the legacy Replays screen's online
    // tab) so the two don't interpret each other's fetch results.
    let mut last_match_remote_rx: Option<(
        matchmaking::HistoryRow,
        std::sync::mpsc::Receiver<matchmaking::PublicReplayUpdate>,
    )> = None;
    let mut spectate_rx: Option<std::sync::mpsc::Receiver<matchmaking::SpectateUpdate>> = None;
    let mut spectate_last_update: Option<Instant> = None;
    let mut lobby_rx: Option<std::sync::mpsc::Receiver<matchmaking::LobbyUpdate>> = None;
    let mut lobby_next_refresh = Instant::now();
    let mut lobby_list_rx: Option<std::sync::mpsc::Receiver<matchmaking::LobbyListUpdate>> = None;
    let mut lobby_list_next_refresh = Instant::now();
    let mut challenge_rx: Option<std::sync::mpsc::Receiver<matchmaking::ChallengeListUpdate>> = None;
    let mut challenge_next_refresh = Instant::now();
    let mut lobby_view_rx: Option<std::sync::mpsc::Receiver<matchmaking::LobbyViewUpdate>> = None;
    let mut lobby_view_next_refresh = Instant::now();
    // King-of-the-hill match thumbnail: active players push a screenshot every
    // ~25s; lobby viewers fetch the latest every ~12s.
    let mut lobby_thumb_rx: Option<std::sync::mpsc::Receiver<Vec<u8>>> = None;
    let mut lobby_thumb_next_fetch = Instant::now();
    let mut lobby_thumb_next_push = Instant::now() + Duration::from_secs(8);
    // Tracks whether the player is driving menus with a controller, so the chat
    // on-screen keyboard only appears for pad users (keyboard users just type).
    let mut menu_input_pad = test_force_pad;
    let mut lobby_chat_post_rx: Option<
        std::sync::mpsc::Receiver<matchmaking::LobbyChatPostUpdate>,
    > = None;
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
    let mut next_slow_perf_log_at = Instant::now();
    let mut rpc_pulse: u32 = 0;

    if let Some(url) = startup_replay_url.take() {
        println!("[replay] Downloading remote replay: {url}");
        state = replay_select_exit_state(cfg.new_ui, "Downloading replay...");
        match download_remote_replay(&url) {
            Ok(path) => match ensure_core_loaded(&mut core, &mut audio_queue, &audio_subsystem) {
                Ok(()) => {
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
                                println!("[replay] Remote replay load failed: {e}");
                                refresh_replay_select(&mut state, Some(format!("Error: {e}")));
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("[replay] Core load failed for remote replay: {e}");
                    refresh_replay_select(&mut state, Some(format!("Error: {e}")));
                }
            },
            Err(e) => {
                println!("[replay] Remote replay download failed: {e}");
                refresh_replay_select(&mut state, Some(format!("Error: {e}")));
            }
        }
    }

    ghost::drain_upload_queue(&cfg.stats_url);
    replay_upload::drain_upload_queue(&cfg.stats_url);
    ghost::queue_all_local_ghosts(
        discord_id.as_deref(),
        discord_user.as_deref(),
        &format!("{:016x}", rom_fingerprint().1),
        &cfg.stats_url,
    );

    macro_rules! shutdown_for_online_start {
        ($reason:expr) => {
            shutdown_local_runtime_for_netplay(
                core.as_ref(),
                audio_queue.as_ref(),
                &mut trainer,
                &mut lab_reset_slots,
                &mut lab_dummy,
                &mut punish_trainer,
                &mut damage_tracker,
                &mut ghost_playback,
                &mut ghost_recording,
                &mut drone_runner,
                &mut ghost_port_mask,
                &mut match_replay_playback,
                &mut match_replay_recording,
                &mut replay_review_paused,
                &mut replay_review_tick,
                &mut replay_clip_in,
                &mut replay_clip_out,
                &mut clip_recorder,
                &mut input_history,
                &mut score_tracker,
                &mut local_play_mode,
                &mut session_p1_wins,
                &mut session_p2_wins,
                &mut auto_start_done,
                &mut auto_start_frame,
                &mut audio_tail_sample,
                None,
                $reason,
            )
        };
    }

    'running: loop {
        if chat_open
            || matches!(
                state,
                AppState::Menu(menu::MenuScreen::TestIp { editing: true, .. })
                    | AppState::Menu(menu::MenuScreen::TextEdit { .. })
                    | AppState::Menu(menu::MenuScreen::OnlineHub {
                        tab: menu::OnlineTab::Chat,
                        focus: menu::HubFocus::Content,
                        ..
                    })
                    | AppState::Menu(menu::MenuScreen::MatchUsername {
                        checking: false,
                        ..
                    })
                    | AppState::FpUi(fp_ui::FpScreen::ClaimUsername {
                        checking: false,
                        ..
                    })
            )
            || is_fp_test_conn_editing(&state)
        {
            video_subsystem.text_input().start();
        } else {
            video_subsystem.text_input().stop();
        }

        // Check for Discord ACTIVITY_JOIN (friend clicked "Join to Spar")
        if mm_rx.is_none() {
            if let Some(room_id) = rpc::take_join_request() {
                println!("[main] Join-to-spar request received");
                shutdown_for_online_start!("Discord join request");
                let (tx, rx) = std::sync::mpsc::channel();
                mm_rx = Some(rx);
                matchmaking::start_join_room(tx, room_id);
                set_matchmaking_screen(&mut state, "Joining spar room...".into());
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

        // Menus draw at window resolution, so disable SDL logical scaling while
        // a menu is up — otherwise mouse-event coordinates would arrive in the
        // 400×254 logical space and not line up with the drawn hit regions.
        if matches!(state, AppState::Menu(_)) {
            let _ = canvas.set_logical_size(0, 0);
        }

        for event in event_pump.poll_iter() {
            // Remember the last input device so the chat keyboard shows for pad
            // users only. A controller button flips to pad mode; any key press
            // flips back to keyboard mode.
            match &event {
                Event::ControllerButtonDown { .. } => menu_input_pad = true,
                Event::KeyDown { .. } => menu_input_pad = false,
                _ => {}
            }
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

                Event::ControllerButtonDown {
                    button: sdl2::controller::Button::DPadLeft | sdl2::controller::Button::DPadRight,
                    ..
                } if matches!(state, AppState::Menu(MenuScreen::Settings { .. })) => {
                    let delta = match event {
                        Event::ControllerButtonDown {
                            button: sdl2::controller::Button::DPadLeft,
                            ..
                        } => -1,
                        _ => 1,
                    };
                    let cursor = match &state {
                        AppState::Menu(MenuScreen::Settings { cursor, .. }) => *cursor,
                        _ => 0,
                    };
                    let _ = adjust_settings_value(
                        &mut cfg,
                        &mut state,
                        &mut toast,
                        cursor,
                        delta,
                        renderer_name(&canvas),
                    );
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
                            | AppState::Menu(menu::MenuScreen::OnlineHub {
                                tab: menu::OnlineTab::Chat,
                                focus: menu::HubFocus::Content,
                                ..
                            })
                            | AppState::Menu(menu::MenuScreen::TextEdit { .. })
                            | AppState::Menu(menu::MenuScreen::MatchUsername {
                                checking: false,
                                ..
                            })
                            | AppState::FpUi(fp_ui::FpScreen::ClaimUsername {
                                checking: false,
                                ..
                            })
                    ) || is_fp_test_conn_editing(&state) =>
                {
                    state.text_input(&text);
                }
                Event::KeyDown {
                    keycode: Some(Keycode::Backspace),
                    ..
                } if matches!(
                    state,
                    AppState::Menu(menu::MenuScreen::TestIp { editing: true, .. })
                        | AppState::Menu(menu::MenuScreen::OnlineHub {
                            tab: menu::OnlineTab::Chat,
                            focus: menu::HubFocus::Content,
                            ..
                        })
                        | AppState::Menu(menu::MenuScreen::TextEdit { .. })
                        | AppState::Menu(menu::MenuScreen::MatchUsername {
                            checking: false,
                            ..
                        })
                        | AppState::FpUi(fp_ui::FpScreen::ClaimUsername {
                            checking: false,
                            ..
                        })
                ) || is_fp_test_conn_editing(&state) =>
                {
                    state.text_backspace();
                }

                // Physical Enter sends a lobby chat message. Handled here, ahead
                // of the generic menu-nav dispatch, because in the Chat section
                // the gamepad A button drives the on-screen keyboard instead of
                // sending — so keyboard users still send with Enter.
                Event::KeyDown {
                    keycode: Some(Keycode::Return | Keycode::KpEnter),
                    ..
                } if matches!(
                    state,
                    AppState::Menu(menu::MenuScreen::OnlineHub {
                        tab: menu::OnlineTab::Chat,
                        focus: menu::HubFocus::Content,
                        ..
                    })
                ) =>
                {
                    let message = if let AppState::Menu(menu::MenuScreen::OnlineHub {
                        ref mut chat_draft,
                        ref mut status,
                        ..
                    }) = state
                    {
                        let m = chat_draft.trim().to_string();
                        if m.is_empty() {
                            None
                        } else {
                            chat_draft.clear();
                            *status = "Sending chat...".into();
                            Some(m)
                        }
                    } else {
                        None
                    };
                    if let Some(message) = message {
                        matchmaking::set_guest_profile(
                            cfg.player_username.clone(),
                            cfg.stats_email.clone(),
                            cfg.guest_device_id.clone(),
                        );
                        let (tx, rx) = std::sync::mpsc::channel();
                        lobby_chat_post_rx = Some(rx);
                        matchmaking::send_lobby_chat(message, tx);
                    }
                }

                Event::KeyDown {
                    keycode: Some(Keycode::F11),
                    repeat: false,
                    ..
                } if is_matchmaking_screen(&state) => {
                    net_stats_visible = !net_stats_visible;
                    net_stats.on_overlay_toggle(net_stats_visible);
                    toast = Some((
                        format!(
                            "Network stats {}",
                            if net_stats_visible { "ON" } else { "OFF" }
                        ),
                        Instant::now() + Duration::from_millis(1600),
                    ));
                }

                _ if is_matchmaking_screen(&state) => {
                    if is_cancel(&event) {
                        println!("[mm] matchmaking canceled by user");
                        mm_rx = None;
                        username_check_rx = None;
                        username_check_silent = false;
                        username_check_started_at = None;
                        // Quick Match stays on the Lobby's tab bar/chrome
                        // throughout the search (see `set_quick_match_searching`)
                        // — canceling just clears the searching state and
                        // lands back on the pre-search prompt, rather than
                        // leaving the Lobby entirely the way every other
                        // matchmaking trigger's cancel does. A Discord
                        // connect similarly returns to the Settings→Account
                        // row it started from, not the main menu.
                        if let AppState::FpUi(fp_ui::FpScreen::Lobby { quick_match_status, .. }) = &mut state {
                            *quick_match_status = None;
                        } else if matches!(state, AppState::FpUi(fp_ui::FpScreen::DiscordConnect { .. })) {
                            state = AppState::FpUi(fp_ui::FpScreen::settings_account(&cfg));
                        } else {
                            state = menu::main_menu_state(cfg.new_ui);
                        }
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
                    match launch_debugger() {
                        Ok(()) => {
                            toast = Some((
                                "Running diagnostics — report will open shortly…".into(),
                                Instant::now() + Duration::from_millis(3000),
                            ));
                        }
                        Err(e) => {
                            println!("[doctor] failed to launch: {e}");
                            toast = Some((
                                format!("Diagnostics failed to launch: {e}"),
                                Instant::now() + Duration::from_millis(3000),
                            ));
                        }
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

                // Mouse challenges in the Online hub: right-click a player name
                // to open the format chooser, left-click a format to send.
                Event::MouseButtonDown {
                    mouse_btn, x, y, ..
                } if matches!(state, AppState::Menu(menu::MenuScreen::OnlineHub { .. })) => {
                    match mouse_btn {
                        sdl2::mouse::MouseButton::Right => {
                            if let Some(idx) = menu::presence_hit_at(x, y) {
                                if let AppState::Menu(menu::MenuScreen::OnlineHub {
                                    tab,
                                    focus,
                                    cursor,
                                    challenge_pick,
                                    challenge_format,
                                    presence,
                                    ..
                                }) = &mut state
                                {
                                    if idx < presence.len() {
                                        *tab = menu::OnlineTab::Players;
                                        *focus = menu::HubFocus::Content;
                                        *cursor = idx;
                                        *challenge_pick = Some(challenge_format.index());
                                    }
                                }
                            }
                        }
                        sdl2::mouse::MouseButton::Left => {
                            if let Some(pi) = menu::phrase_hit_at(x, y) {
                                if let AppState::Menu(menu::MenuScreen::OnlineHub {
                                    chat_draft,
                                    ..
                                }) = &mut state
                                {
                                    let ph = menu::quick_phrase(pi);
                                    if chat_draft.chars().count() + ph.chars().count() + 1 <= 180 {
                                        if !chat_draft.is_empty() && !chat_draft.ends_with(' ') {
                                            chat_draft.push(' ');
                                        }
                                        chat_draft.push_str(ph);
                                        chat_draft.push(' ');
                                    }
                                }
                            } else if let Some(fmt_idx) = menu::format_hit_at(x, y) {
                                let target = if let AppState::Menu(
                                    menu::MenuScreen::OnlineHub {
                                        challenge_pick: Some(_),
                                        cursor,
                                        presence,
                                        ..
                                    },
                                ) = &state
                                {
                                    presence.get(*cursor).map(|u| u.player_id.clone())
                                } else {
                                    None
                                };
                                if let Some(target_id) = target {
                                    if let AppState::Menu(menu::MenuScreen::OnlineHub {
                                        challenge_pick,
                                        ..
                                    }) = &mut state
                                    {
                                        *challenge_pick = None;
                                    }
                                    let fmt = menu::ChallengeFormat::at_index(fmt_idx);
                                    matchmaking::set_guest_profile(
                                        cfg.player_username.clone(),
                                        cfg.stats_email.clone(),
                                        cfg.guest_device_id.clone(),
                                    );
                                    shutdown_for_online_start!("Send challenge");
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    mm_rx = Some(rx);
                                    matchmaking::start_send_challenge(
                                        tx,
                                        target_id,
                                        fmt.wire().to_string(),
                                    );
                                    set_matchmaking_screen(&mut state, "Challenging player...".into());
                                }
                            }
                        }
                        _ => {}
                    }
                }

                // Controller input driving the native on-screen keyboard —
                // only for TextEdit captures opened from fp_ui, and only for
                // the events the grid claims (`wants_event`): D-pad moves the
                // key cursor, Cross presses the highlighted cell. A Cross on
                // the CONFIRM cell is deliberately *not* claimed, so it falls
                // into the normal Accept translation below and commits
                // through the same `NavResult::CommitText` path Enter does.
                // Keyboard events are never claimed — typing/Enter/Esc keep
                // working exactly as before.
                _ if is_fp_text_edit(&state) && fp_ui::text_entry::wants_event(&event, fp_osk) => {
                    match fp_ui::text_entry::apply(&event, &mut fp_osk) {
                        fp_ui::text_entry::OskAction::Moved => {}
                        fp_ui::text_entry::OskAction::Char(c) => state.text_input(&c.to_string()),
                        fp_ui::text_entry::OskAction::Space => state.text_input(" "),
                        fp_ui::text_entry::OskAction::Backspace => state.text_backspace(),
                        fp_ui::text_entry::OskAction::Cancel => state.nav_back(cfg.new_ui),
                    }
                }

                // Combined so a Confirm on the fp_ui Main Menu can convert
                // `state` to the legacy `Menu(MenuScreen::Main)` and, in the
                // same event, immediately fall into the unmodified legacy
                // Accept handling below — reusing its ROM-present checks and
                // NavResult side effects (session start, profile fetch,
                // replay listing, ...) instead of re-implementing them here.
                _ if matches!(state, AppState::FpUi(_)) || matches!(state, AppState::Menu(_)) => {
                    if let AppState::FpUi(screen) = &mut state {
                        if let Some(nav) = fp_ui::event_to_fp_nav(&event) {
                            match fp_ui::nav(screen, nav, rom_present.check()) {
                                fp_ui::FpResult::Stay => {}
                                fp_ui::FpResult::ActivateMainItem(cursor) => {
                                    state = AppState::Menu(MenuScreen::Main { cursor });
                                }
                                fp_ui::FpResult::ActivateLabMenuItem(cursor) => {
                                    state = AppState::Menu(MenuScreen::LabMenu { cursor });
                                }
                                fp_ui::FpResult::ExitGame => break 'running,
                                fp_ui::FpResult::SettingsChanged => {
                                    if let AppState::FpUi(fp_ui::FpScreen::Settings { fields, .. }) = &state {
                                        if cfg.fullscreen != fields.fullscreen {
                                            let mode = if fields.fullscreen {
                                                FullscreenType::Desktop
                                            } else {
                                                FullscreenType::Off
                                            };
                                            let _ = canvas.window_mut().set_fullscreen(mode);
                                        }
                                        cfg.fullscreen = fields.fullscreen;
                                        cfg.render_profile = fields.render_profile;
                                        cfg.video_filter = fields.video_filter;
                                        cfg.crt_corner_bend = fields.crt_corner_bend;
                                        cfg.aspect_mode = fields.aspect_mode;
                                        cfg.scorebar_style = fields.scorebar_style;
                                        cfg.volume_percent = fields.volume_percent;
                                        cfg.audio_buffer = fields.audio_buffer;
                                        cfg.input_delay = fields.input_delay;
                                        cfg.runahead = fields.runahead;
                                        cfg.runahead_online = fields.runahead_online;
                                        cfg.discord_rpc_enabled = fields.discord_rpc_enabled;
                                        config::save(&cfg);
                                    }
                                }
                                fp_ui::FpResult::StartFindMatch => {
                                    let value = config::sanitize_username(&cfg.player_username)
                                        .unwrap_or_else(config::default_username);
                                    cfg.player_username = value.clone();
                                    if cfg.player_username_confirmed {
                                        // Already confirmed once — mirrors legacy's
                                        // NavResult::OpenUsernameEntry exactly.
                                        config::save(&cfg);
                                        shutdown_for_online_start!("find-match queue");
                                        start_find_match_queue(
                                            &cfg,
                                            &mut mm_rx,
                                            &mut state,
                                            value,
                                            &mut discord_user,
                                            &mut discord_id,
                                        );
                                    } else {
                                        // First time online: show the auto-generated name
                                        // and let them keep or change it before queueing —
                                        // native counterpart of legacy's own first-time path
                                        // just below (`NavResult::OpenUsernameEntry`).
                                        config::save(&cfg);
                                        state = AppState::FpUi(fp_ui::FpScreen::ClaimUsername {
                                            value,
                                            status: "This is your name — edit it or press Enter to claim it".into(),
                                            checking: false,
                                        });
                                    }
                                }
                                fp_ui::FpResult::CreatePrivateLobby => {
                                    // Mirrors legacy's NavResult::CreateLobby(format, true)
                                    // with a fixed default format — fp_ui's Host/Join tab
                                    // doesn't expose a format picker.
                                    matchmaking::set_guest_profile(
                                        cfg.player_username.clone(),
                                        cfg.stats_email.clone(),
                                        cfg.guest_device_id.clone(),
                                    );
                                    let host_name = if cfg.player_username.trim().is_empty() {
                                        "Player".to_string()
                                    } else {
                                        format!("{}'s lobby", cfg.player_username)
                                    };
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    lobby_view_rx = Some(rx);
                                    let format = menu::ChallengeFormat::UnrankedVs;
                                    matchmaking::create_lobby(
                                        tx,
                                        host_name,
                                        format.ranked(),
                                        true,
                                        format.wire().to_string(),
                                    );
                                    state = AppState::FpUi(fp_ui::FpScreen::LobbyRoom {
                                        id: String::new(),
                                        view: None,
                                        status: "Creating private lobby...".into(),
                                        thumb: None,
                                    });
                                }
                                fp_ui::FpResult::OpenJoinCode => {
                                    // Mirrors legacy's NavResult::OpenJoinCode, returning to
                                    // the actual fp_ui Lobby screen on cancel now that
                                    // `came_from` is `Box<AppState>`.
                                    let came_from = state.clone();
                                    state = AppState::Menu(MenuScreen::TextEdit {
                                        title: "JOIN LOBBY".into(),
                                        label: "Enter the 6-character invite code".into(),
                                        value: String::new(),
                                        field: menu::EditField::JoinCode,
                                        came_from: Box::new(came_from),
                                    });
                                }
                                fp_ui::FpResult::JoinLobby(lobby_id) => {
                                    // Mirrors legacy's NavResult::JoinLobby(id).
                                    matchmaking::set_guest_profile(
                                        cfg.player_username.clone(),
                                        cfg.stats_email.clone(),
                                        cfg.guest_device_id.clone(),
                                    );
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    lobby_view_rx = Some(rx);
                                    matchmaking::join_lobby(tx, lobby_id.clone(), false);
                                    state = AppState::FpUi(fp_ui::FpScreen::LobbyRoom {
                                        id: lobby_id,
                                        view: None,
                                        status: "Joining lobby...".into(),
                                        thumb: None,
                                    });
                                }
                                fp_ui::FpResult::WatchLastMatchReplay => {
                                    let row = if let menu::ProfileScreenState::Loaded { history, .. } = &main_profile {
                                        history.first().cloned()
                                    } else {
                                        None
                                    };
                                    let path = row.as_ref().and_then(match_replay::find_matching_local_replay);
                                    match path {
                                        Some(path) => {
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
                                                        println!("[replay] Last-match replay load failed: {e}");
                                                        toast = Some((
                                                            format!("Replay unavailable: {e}"),
                                                            Instant::now() + Duration::from_millis(3200),
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                        None => {
                                            // No local file — every completed online match
                                            // gets uploaded to freeplay-stats
                                            // (`replay_upload.rs`), so check the public
                                            // replay index before giving up; drained in
                                            // the `last_match_remote_rx` poll below.
                                            if let Some(row) = row {
                                                toast = Some((
                                                    "Checking remote replays\u{2026}".into(),
                                                    Instant::now() + Duration::from_millis(2000),
                                                ));
                                                let (tx, rx) = std::sync::mpsc::channel();
                                                matchmaking::fetch_public_replays(cfg.stats_url.clone(), tx);
                                                last_match_remote_rx = Some((row, rx));
                                            } else {
                                                toast = Some((
                                                    "No replay found for this match".into(),
                                                    Instant::now() + Duration::from_millis(2400),
                                                ));
                                            }
                                        }
                                    }
                                }
                                fp_ui::FpResult::SubmitUsername(value) => {
                                    // Mirrors legacy's NavResult::SubmitUsername exactly,
                                    // targeting FpScreen::ClaimUsername's fields instead of
                                    // the legacy screen's (see `set_username_screen`).
                                    match config::sanitize_username(&value) {
                                        Some(username) => {
                                            let (tx, rx) = std::sync::mpsc::channel();
                                            username_check_rx = Some(rx);
                                            username_check_silent = false;
                                            username_check_started_at = Some(Instant::now());
                                            let owner_id = discord_id
                                                .clone()
                                                .unwrap_or_else(|| cfg.guest_device_id.clone());
                                            matchmaking::check_username_available(
                                                cfg.stats_url.clone(),
                                                username.clone(),
                                                owner_id,
                                                tx,
                                            );
                                            set_username_screen(&mut state, username, "Checking username".into(), true);
                                        }
                                        None => {
                                            set_username_screen(
                                                &mut state,
                                                value,
                                                format!(
                                                    "Invalid name: use 2-{} letters, numbers, _ or -",
                                                    config::MAX_USERNAME_LEN
                                                ),
                                                false,
                                            );
                                        }
                                    }
                                }
                                fp_ui::FpResult::SetLobbyQueue(lobby_id, queued) => {
                                    // Mirrors legacy's NavResult::SetLobbyQueue exactly.
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    lobby_view_rx = Some(rx);
                                    matchmaking::join_lobby(tx, lobby_id, queued);
                                }
                                fp_ui::FpResult::ReadyLobby(lobby_id) => {
                                    // Mirrors legacy's NavResult::ReadyLobby exactly.
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    lobby_view_rx = Some(rx);
                                    matchmaking::ready_lobby(tx, lobby_id);
                                }
                                fp_ui::FpResult::LeaveLobby(lobby_id) => {
                                    // Mirrors legacy's own Back handling for
                                    // MenuScreen::Lobby (declines any pending ready
                                    // check for you by simply leaving).
                                    if !lobby_id.is_empty() {
                                        matchmaking::leave_lobby(lobby_id);
                                    }
                                    lobby_view_rx = None;
                                    state = menu::main_menu_state(cfg.new_ui);
                                }
                                fp_ui::FpResult::SendChallenge(target_id, format) => {
                                    // Mirrors legacy's NavResult::SendChallenge exactly —
                                    // same "connecting" handoff to the shared legacy
                                    // Matchmaking screen as StartFindMatch above.
                                    matchmaking::set_guest_profile(
                                        cfg.player_username.clone(),
                                        cfg.stats_email.clone(),
                                        cfg.guest_device_id.clone(),
                                    );
                                    shutdown_for_online_start!("Send challenge");
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    mm_rx = Some(rx);
                                    matchmaking::start_send_challenge(
                                        tx,
                                        target_id,
                                        format.wire().to_string(),
                                    );
                                    set_matchmaking_screen(&mut state, "Challenging player...".into());
                                }
                                fp_ui::FpResult::AcceptChallenge(challenge_id) => {
                                    // Mirrors legacy's NavResult::AcceptChallenge exactly.
                                    matchmaking::set_guest_profile(
                                        cfg.player_username.clone(),
                                        cfg.stats_email.clone(),
                                        cfg.guest_device_id.clone(),
                                    );
                                    shutdown_for_online_start!("Accept challenge");
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    mm_rx = Some(rx);
                                    matchmaking::start_accept_challenge(tx, challenge_id);
                                    set_matchmaking_screen(&mut state, "Accepting challenge...".into());
                                }
                                fp_ui::FpResult::DeclineChallenge(challenge_id) => {
                                    matchmaking::decline_challenge(challenge_id);
                                }
                                fp_ui::FpResult::RunConnectionProbe(address) => {
                                    // Mirrors legacy's NavResult::RunProbe exactly (same
                                    // netplay::probe_connection/format_probe_result call),
                                    // targeting this screen's test_conn_lines instead of a
                                    // separate MenuScreen::TestResult.
                                    let lines = match menu::parse_ip_port(&address) {
                                        Some(peer) => {
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
                                            lines
                                        }
                                        None => vec!["FAIL Invalid address \u{2014} use IP or IP:PORT".into()],
                                    };
                                    if let AppState::FpUi(fp_ui::FpScreen::Settings {
                                        ref mut test_conn_lines,
                                        ..
                                    }) = state
                                    {
                                        *test_conn_lines = lines;
                                    }
                                }
                                fp_ui::FpResult::WatchEndedReplay(path) => {
                                    // Same playback pipeline as WatchLastMatchReplay above,
                                    // just fed an explicit path instead of looking one up.
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
                                                println!("[replay] Session-end replay load failed: {e}");
                                                toast = Some((
                                                    format!("Replay unavailable: {e}"),
                                                    Instant::now() + Duration::from_millis(3200),
                                                ));
                                            }
                                        }
                                    }
                                }
                                // Same `AppState::Rebinding` capture the legacy Controls
                                // screen uses — `came_from` is the current fp_ui Settings
                                // screen (not a legacy `MenuScreen`), so `finish_rebind`
                                // returns here instead of to legacy once a button is
                                // pressed or the capture is canceled.
                                fp_ui::FpResult::BeginRebind(action, player) => {
                                    let came_from = state.clone();
                                    state = AppState::Rebinding {
                                        action,
                                        player,
                                        came_from: Box::new(came_from),
                                    };
                                }
                                fp_ui::FpResult::ClearAllBindings(player) => {
                                    cfg.bindings.get_mut(player).clear_all();
                                    config::save(&cfg);
                                }
                                fp_ui::FpResult::BeginAccountEdit(field) => {
                                    // Mirrors legacy's NavResult::EditText for just the
                                    // two fields the Account category exposes, returning to
                                    // the actual fp_ui Settings screen on cancel/commit now
                                    // that `came_from` is `Box<AppState>`.
                                    let (title, label, value) = match &field {
                                        menu::EditField::Username => (
                                            "Username",
                                            "Choose the name other players see",
                                            cfg.player_username.clone(),
                                        ),
                                        menu::EditField::StatsEmail => (
                                            "Stats Email",
                                            "Optional email for portable stats",
                                            cfg.stats_email.clone(),
                                        ),
                                        _ => ("", "", String::new()),
                                    };
                                    let came_from = state.clone();
                                    state = AppState::Menu(MenuScreen::TextEdit {
                                        title: title.into(),
                                        label: label.into(),
                                        value,
                                        field,
                                        came_from: Box::new(came_from),
                                    });
                                }
                                fp_ui::FpResult::ToggleDiscordConnect => {
                                    // Mirrors legacy's NavResult::ConnectDiscord, but
                                    // both directions land back on the native
                                    // Settings→Account category they started from
                                    // (disconnect immediately; connect via the
                                    // DiscordConnect waiting screen and the
                                    // AuthConnected/cancel handlers).
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
                                        state = AppState::FpUi(fp_ui::FpScreen::settings_account(&cfg));
                                    } else {
                                        let (tx, rx) = std::sync::mpsc::channel();
                                        mm_rx = Some(rx);
                                        matchmaking::start_discord_connect(tx);
                                        state = AppState::FpUi(fp_ui::FpScreen::DiscordConnect {
                                            status: "Opening Discord login...".into(),
                                        });
                                    }
                                }
                                fp_ui::FpResult::OpenGhostSelect => {
                                    // Mirrors legacy's NavResult::OpenGhostSelect exactly,
                                    // targeting the native FpScreen::GhostSelect's status
                                    // field instead of the legacy screen's.
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    ghost_list_rx = Some(rx);
                                    if let AppState::FpUi(fp_ui::FpScreen::GhostSelect {
                                        ref mut status, ..
                                    }) = state
                                    {
                                        *status = if cfg.stats_url.trim().is_empty() {
                                            None
                                        } else {
                                            Some("Loading shared drones...".into())
                                        };
                                    }
                                    let rh = rom_fingerprint().1;
                                    let rom_hash = format!("{:016x}", rh);
                                    matchmaking::fetch_ghost_list(cfg.stats_url.clone(), rom_hash, tx);
                                }
                                fp_ui::FpResult::LoadGhost(path) => {
                                    // Mirrors legacy's NavResult::LoadGhost(path) exactly.
                                    ensure_core_loaded(&mut core, &mut audio_queue, &audio_subsystem)?;
                                    if let Some(c) = &core {
                                        match ghost::Playback::load(&path) {
                                            Ok(pb) => {
                                                if pb.prime(c) {
                                                    println!(
                                                        "[ghost] Loaded drone opponent: {} frames",
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
                                                    println!("[ghost] Anchor state rejected by core.");
                                                    state = AppState::FpUi(fp_ui::FpScreen::LabMenu { cursor: 1 });
                                                }
                                            }
                                            Err(e) => {
                                                println!("[ghost] Load failed: {e}");
                                                state = AppState::FpUi(fp_ui::FpScreen::LabMenu { cursor: 1 });
                                            }
                                        }
                                    } else {
                                        state = AppState::FpUi(fp_ui::FpScreen::LabMenu { cursor: 1 });
                                    }
                                }
                                fp_ui::FpResult::DownloadGhost(ghost_id) => {
                                    // Mirrors legacy's NavResult::DownloadGhost(ghost_id).
                                    let local_path = format!("ghosts/remote_{ghost_id}.ncgh");
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    ghost_download_rx = Some(rx);
                                    if let AppState::FpUi(fp_ui::FpScreen::GhostSelect {
                                        ref mut status, ..
                                    }) = state
                                    {
                                        *status = Some(format!("Downloading {ghost_id}..."));
                                    }
                                    matchmaking::download_ghost(cfg.stats_url.clone(), ghost_id, local_path, tx);
                                }
                                fp_ui::FpResult::OpenReplaySelect => {
                                    // Mirrors legacy's NavResult::OpenReplaySelect exactly;
                                    // `refresh_replay_select` already branches on either
                                    // screen shape (see its own definition).
                                    refresh_replay_select(&mut state, Some("Loading public replays...".into()));
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    public_replay_rx = Some(rx);
                                    matchmaking::fetch_public_replays(cfg.stats_url.clone(), tx);
                                }
                                fp_ui::FpResult::LoadReplay(path) => {
                                    // Mirrors legacy's NavResult::LoadReplay(path) exactly.
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
                                                println!("[replay] Load failed: {e}");
                                                refresh_replay_select(&mut state, Some(format!("Error: {e}")));
                                            }
                                        }
                                    }
                                }
                                fp_ui::FpResult::LoadRemoteReplay(url) => {
                                    // Mirrors legacy's NavResult::LoadRemoteReplay(url).
                                    set_replay_select_status(&mut state, "Downloading public replay...");
                                    match download_remote_replay(&url) {
                                        Ok(path) => {
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
                                                        println!("[replay] Load failed: {e}");
                                                        set_replay_select_status(&mut state, format!("Error: {e}"));
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            println!("[replay] Remote download failed: {e}");
                                            set_replay_select_status(&mut state, format!("Download failed: {e}"));
                                        }
                                    }
                                }
                                fp_ui::FpResult::SendLobbyChat(message) => {
                                    // Mirrors legacy's NavResult::SendLobbyChat(message).
                                    matchmaking::set_guest_profile(
                                        cfg.player_username.clone(),
                                        cfg.stats_email.clone(),
                                        cfg.guest_device_id.clone(),
                                    );
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    lobby_chat_post_rx = Some(rx);
                                    matchmaking::send_lobby_chat(message, tx);
                                }
                                fp_ui::FpResult::ComposeChat => {
                                    // Native text-entry capture (shared on-screen
                                    // keyboard) — commit sends the message and lands
                                    // back on this Chat tab via `came_from`, replacing
                                    // the old whole-screen handoff to legacy's
                                    // OnlineHub just to reach a keyboard.
                                    let came_from = state.clone();
                                    state = AppState::Menu(MenuScreen::TextEdit {
                                        title: "LOBBY CHAT".into(),
                                        label: "Send a message to everyone online".into(),
                                        value: String::new(),
                                        field: menu::EditField::ChatMessage,
                                        came_from: Box::new(came_from),
                                    });
                                }
                                fp_ui::FpResult::WatchSession(session_id) => {
                                    // Mirrors legacy's NavResult::WatchSession(id) — the
                                    // spectator view itself stays legacy.
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    spectate_rx = Some(rx);
                                    spectate_last_update = Some(Instant::now());
                                    matchmaking::watch_spectate_state(session_id.clone(), tx);
                                    state = AppState::Menu(MenuScreen::Spectate {
                                        session_id,
                                        status: menu::SpectateStatus::waiting(),
                                    });
                                }
                            }
                        }
                    }
                    // The native Replays screen has no `FpNav` equivalent for
                    // Delete/Edit-Note (they're raw shortcut keys, not part of
                    // the D-pad/face-button navigation grammar), so it's let
                    // through here too — `handle_replay_select_shortcut` (and
                    // the legacy nav_* methods it falls through to on a miss)
                    // already no-op safely for any other `AppState::FpUi`.
                    if !matches!(state, AppState::Menu(_))
                        && !matches!(state, AppState::FpUi(fp_ui::FpScreen::ReplaySelect { .. }))
                    {
                        continue;
                    }
                    if handle_replay_select_shortcut(&event, &mut state) {
                        continue;
                    }
                    if !matches!(state, AppState::Menu(_)) {
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
                                            if let Some(c) = &core {
                                                input::clear_all_inputs();
                                                c.reset();
                                                reset_for_netplay(
                                                    c,
                                                    &mut trainer,
                                                    &mut lab_reset_slots,
                                                    &mut ghost_playback,
                                                    &mut ghost_recording,
                                                );
                                            }
                                            net_runtime = NetRuntime::default();
                                            net_match_count = 0;
                                            // Non-lobby session (direct IP / join
                                            // room) — always the default best-of.
                                            net_match_limit = NETPLAY_MATCH_LIMIT;
                                            ranked_match_index = 0;
                                            net_in_fight = false;
                                            net_set_complete_pending_frame = None;
                                            net_frames_since_progress = 0;
                                            match netplay::start_session(
                                                *local_port,
                                                *player,
                                                *peer,
                                                cfg.input_delay,
                                            ) {
                                                Ok(s) => {
                                                    net_session = Some(s);
                                                    set_netplay_window_chrome(&mut canvas, true);
                                                    relay_chat = None;
                                                    net_transport_path = Some("direct");
                                                    net_stats.reset();
                                                    audio_tail_sample = None;
                                                    net_frame_counter = 0;
                                                    net_overlay_diagnosed = false;
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
                                                    net_transport_path = None;
                                                    println!("[net] session start failed: {e}")
                                                }
                                            }
                                        }
                                    }
                                }
                                NavResult::OpenUsernameEntry => {
                                    let value = config::sanitize_username(&cfg.player_username)
                                        .unwrap_or_else(config::default_username);
                                    cfg.player_username = value.clone();
                                    if cfg.player_username_confirmed {
                                        // Already confirmed once — the name is the same
                                        // identity used in lobby chat, so going online
                                        // never re-prompts. Players rename via Settings.
                                        config::save(&cfg);
                                        shutdown_for_online_start!("find-match queue");
                                        start_find_match_queue(
                                            &cfg,
                                            &mut mm_rx,
                                            &mut state,
                                            value,
                                            &mut discord_user,
                                            &mut discord_id,
                                        );
                                    } else {
                                        // First time online: show their auto-generated
                                        // name and let them keep or change it, then
                                        // verify it isn't already taken before queueing.
                                        config::save(&cfg);
                                        state = AppState::Menu(MenuScreen::MatchUsername {
                                            value,
                                            status: "This is your name — edit it or press Enter to claim it".into(),
                                            checking: false,
                                        });
                                    }
                                }
                                NavResult::SubmitUsername(value) => {
                                    match config::sanitize_username(&value) {
                                        Some(username) => {
                                            let (tx, rx) = std::sync::mpsc::channel();
                                            username_check_rx = Some(rx);
                                            username_check_silent = false;
                                            username_check_started_at = Some(Instant::now());
                                            // Reserve under the player's stable
                                            // identity: their Discord id if
                                            // signed in, otherwise the per-
                                            // install guest_device_id.
                                            let owner_id = discord_id
                                                .clone()
                                                .unwrap_or_else(|| cfg.guest_device_id.clone());
                                            matchmaking::check_username_available(
                                                cfg.stats_url.clone(),
                                                username.clone(),
                                                owner_id,
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
                                                status: format!(
                                                    "Invalid name: use 2-{} letters, numbers, _ or -",
                                                    config::MAX_USERNAME_LEN
                                                ),
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
                                    shutdown_for_online_start!("find-match queue");
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
                                                Some("Loading shared drones...".into());
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
                                    refresh_replay_select(
                                        &mut state,
                                        Some("Loading public replays...".into()),
                                    );
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    public_replay_rx = Some(rx);
                                    matchmaking::fetch_public_replays(cfg.stats_url.clone(), tx);
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
                                    let profile_id = discord_id
                                        .clone()
                                        .or_else(matchmaking::discord_id_from_cached_token)
                                        .or_else(|| {
                                            matchmaking::guest_player_id(
                                                &cfg.player_username,
                                                &cfg.stats_email,
                                                &cfg.guest_device_id,
                                            )
                                        });
                                    if let Some(did) = profile_id {
                                        let display_name = discord_user
                                            .clone()
                                            .unwrap_or_else(|| cfg.player_username.clone());
                                        let (tx, rx) = std::sync::mpsc::channel();
                                        profile_rx = Some(rx);
                                        matchmaking::fetch_profile(
                                            cfg.stats_url.clone(),
                                            did,
                                            display_name,
                                            tx,
                                        );
                                    } else {
                                        state = AppState::Menu(MenuScreen::Profile {
                                            state: menu::ProfileScreenState::NotLoggedIn,
                                        });
                                    }
                                }
                                NavResult::OpenLiveMatches => {
                                    if let AppState::Menu(menu::MenuScreen::OnlineHub {
                                        ref mut tab,
                                        ref mut status,
                                        ..
                                    }) = state
                                    {
                                        *tab = menu::OnlineTab::Watch;
                                        *status = "Loading live matches...".into();
                                    }
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
                                        runahead: cfg.runahead,
                                        runahead_online: cfg.runahead_online,
                                    });
                                    if cfg.new_ui {
                                        state = AppState::FpUi(fp_ui::FpScreen::settings_from_cfg(&cfg));
                                    }
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
                                        menu::EditField::ReplayNote { .. }
                                        | menu::EditField::JoinCode
                                        | menu::EditField::ChatMessage => String::new(),
                                    };
                                    let label = match &field {
                                        menu::EditField::Username => {
                                            "Choose the name other players see"
                                        }
                                        menu::EditField::StatsEmail => {
                                            "Optional email for portable stats"
                                        }
                                        menu::EditField::ReplayNote { .. } => "Replay note",
                                        menu::EditField::JoinCode => "Enter invite code",
                                        menu::EditField::ChatMessage => {
                                            "Send a message to everyone online"
                                        }
                                    };
                                    // `state` is already `AppState::Menu(MenuScreen::Settings
                                    // { .. })` here (this only fires from that screen's own
                                    // `accept()` arm) — clone it directly instead of
                                    // rebuilding the same struct field-by-field.
                                    let came_from = state.clone();
                                    state = AppState::Menu(MenuScreen::TextEdit {
                                        title,
                                        label: label.into(),
                                        value,
                                        field,
                                        came_from: Box::new(came_from),
                                    });
                                }
                                NavResult::CommitText(field, value, came_from) => match field {
                                    menu::EditField::JoinCode => {
                                        let code = value.trim().to_uppercase();
                                        if code.is_empty() {
                                            state =
                                                menu::main_menu_state(cfg.new_ui);
                                        } else {
                                            matchmaking::set_guest_profile(
                                                cfg.player_username.clone(),
                                                cfg.stats_email.clone(),
                                                cfg.guest_device_id.clone(),
                                            );
                                            let (tx, rx) = std::sync::mpsc::channel();
                                            lobby_view_rx = Some(rx);
                                            matchmaking::join_lobby(tx, code.clone(), false);
                                            state = if cfg.new_ui {
                                                AppState::FpUi(fp_ui::FpScreen::LobbyRoom {
                                                    id: code,
                                                    view: None,
                                                    status: "Joining lobby...".into(),
                                                    thumb: None,
                                                })
                                            } else {
                                                AppState::Menu(MenuScreen::Lobby {
                                                    id: code,
                                                    view: None,
                                                    status: "Joining lobby...".into(),
                                                    thumb: None,
                                                })
                                            };
                                        }
                                    }
                                    menu::EditField::ReplayNote { path } => {
                                        let status =
                                            match match_replay::save_replay_note(&path, &value) {
                                                Ok(()) => "Replay note saved".to_string(),
                                                Err(e) => format!("Replay note failed: {e}"),
                                            };
                                        // Return to whichever screen (native or legacy)
                                        // editing began from, then refresh its entries —
                                        // `refresh_replay_select` already handles both.
                                        state = *came_from;
                                        refresh_replay_select(&mut state, Some(status));
                                    }
                                    menu::EditField::ChatMessage => {
                                        // Same send path as FpResult::SendLobbyChat /
                                        // legacy's NavResult::SendLobbyChat; an empty
                                        // commit just returns without sending.
                                        let message = value.trim().to_string();
                                        if !message.is_empty() {
                                            matchmaking::set_guest_profile(
                                                cfg.player_username.clone(),
                                                cfg.stats_email.clone(),
                                                cfg.guest_device_id.clone(),
                                            );
                                            let (tx, rx) = std::sync::mpsc::channel();
                                            lobby_chat_post_rx = Some(rx);
                                            matchmaking::send_lobby_chat(message, tx);
                                        }
                                        state = *came_from;
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
                                            menu::EditField::ReplayNote { .. }
                                            | menu::EditField::JoinCode
                                            | menu::EditField::ChatMessage => {}
                                        }
                                        config::save(&cfg);
                                        matchmaking::clear_cached_token();
                                        // Return to whichever Settings screen (native or
                                        // legacy) editing began from. fp_ui's own Settings
                                        // reads `player_username`/`stats_email` live from
                                        // `cfg` every frame (passed as separate `draw()`
                                        // params, not baked into `FpScreen::Settings`), so
                                        // it needs no refresh here — only legacy's own
                                        // `MenuScreen::Settings`, which snapshots them, does.
                                        state = *came_from;
                                        if let AppState::Menu(MenuScreen::Settings {
                                            player_username,
                                            stats_email,
                                            discord_connected,
                                            ..
                                        }) = &mut state
                                        {
                                            *player_username = cfg.player_username.clone();
                                            *stats_email = cfg.stats_email.clone();
                                            *discord_connected =
                                                matchmaking::connected_discord_user_from_cached_token()
                                                    .is_some();
                                        }
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
                                            runahead: cfg.runahead,
                                            runahead_online: cfg.runahead_online,
                                        });
                                    } else {
                                        let (tx, rx) = std::sync::mpsc::channel();
                                        mm_rx = Some(rx);
                                        matchmaking::start_discord_connect(tx);
                                        set_matchmaking_screen(&mut state, "Opening Discord login...".into());
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
                                NavResult::SendLobbyChat(message) => {
                                    matchmaking::set_guest_profile(
                                        cfg.player_username.clone(),
                                        cfg.stats_email.clone(),
                                        cfg.guest_device_id.clone(),
                                    );
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    lobby_chat_post_rx = Some(rx);
                                    matchmaking::send_lobby_chat(message, tx);
                                    if let AppState::Menu(menu::MenuScreen::OnlineHub {
                                        ref mut status,
                                        ..
                                    }) = state
                                    {
                                        *status = "Sending chat...".into();
                                    }
                                }
                                NavResult::JoinLobby(lobby_id) => {
                                    // Join a king-of-the-hill lobby and open its
                                    // room screen.
                                    matchmaking::set_guest_profile(
                                        cfg.player_username.clone(),
                                        cfg.stats_email.clone(),
                                        cfg.guest_device_id.clone(),
                                    );
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    lobby_view_rx = Some(rx);
                                    matchmaking::join_lobby(tx, lobby_id.clone(), false);
                                    state = AppState::Menu(MenuScreen::Lobby {
                                        id: lobby_id,
                                        view: None,
                                        status: "Joining lobby...".into(),
                                        thumb: None,
                                    });
                                }
                                NavResult::CreateLobby(format, private) => {
                                    // Create a king-of-the-hill lobby named after
                                    // the player; navigate to it once it exists.
                                    matchmaking::set_guest_profile(
                                        cfg.player_username.clone(),
                                        cfg.stats_email.clone(),
                                        cfg.guest_device_id.clone(),
                                    );
                                    let host_name = if cfg.player_username.trim().is_empty() {
                                        "Player".to_string()
                                    } else {
                                        format!("{}'s lobby", cfg.player_username)
                                    };
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    lobby_view_rx = Some(rx);
                                    matchmaking::create_lobby(
                                        tx,
                                        host_name,
                                        format.ranked(),
                                        private,
                                        format.wire().to_string(),
                                    );
                                    state = AppState::Menu(MenuScreen::Lobby {
                                        id: String::new(),
                                        view: None,
                                        status: if private {
                                            "Creating private lobby...".into()
                                        } else {
                                            "Creating lobby...".into()
                                        },
                                        thumb: None,
                                    });
                                }
                                NavResult::OpenJoinCode => {
                                    // Was hardcoded to legacy Main regardless of where this
                                    // was triggered from — now returns to the actual
                                    // OnlineHub screen on cancel.
                                    let came_from = state.clone();
                                    state = AppState::Menu(MenuScreen::TextEdit {
                                        title: "JOIN LOBBY".into(),
                                        label: "Enter the 6-character invite code".into(),
                                        value: String::new(),
                                        field: menu::EditField::JoinCode,
                                        came_from: Box::new(came_from),
                                    });
                                }
                                NavResult::SetLobbyQueue(lobby_id, spectate) => {
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    lobby_view_rx = Some(rx);
                                    matchmaking::join_lobby(tx, lobby_id, spectate);
                                }
                                NavResult::ReadyLobby(lobby_id) => {
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    lobby_view_rx = Some(rx);
                                    matchmaking::ready_lobby(tx, lobby_id);
                                }
                                NavResult::SendChallenge(target_id, format) => {
                                    matchmaking::set_guest_profile(
                                        cfg.player_username.clone(),
                                        cfg.stats_email.clone(),
                                        cfg.guest_device_id.clone(),
                                    );
                                    shutdown_for_online_start!("Send challenge");
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    mm_rx = Some(rx);
                                    matchmaking::start_send_challenge(
                                        tx,
                                        target_id,
                                        format.wire().to_string(),
                                    );
                                    set_matchmaking_screen(&mut state, "Challenging player...".into());
                                }
                                NavResult::AcceptChallenge(challenge_id) => {
                                    matchmaking::set_guest_profile(
                                        cfg.player_username.clone(),
                                        cfg.stats_email.clone(),
                                        cfg.guest_device_id.clone(),
                                    );
                                    shutdown_for_online_start!("Accept challenge");
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    mm_rx = Some(rx);
                                    matchmaking::start_accept_challenge(tx, challenge_id);
                                    set_matchmaking_screen(&mut state, "Accepting challenge...".into());
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
                                NavResult::ToggleRunahead => {
                                    cfg.runahead = !cfg.runahead;
                                    config::save(&cfg);
                                    if let AppState::Menu(MenuScreen::Settings {
                                        ref mut runahead,
                                        ..
                                    }) = state
                                    {
                                        *runahead = cfg.runahead;
                                    }
                                    toast = Some((
                                        format!(
                                            "Runahead (offline) {}",
                                            if cfg.runahead { "ON" } else { "OFF" }
                                        ),
                                        Instant::now() + Duration::from_millis(1800),
                                    ));
                                }
                                NavResult::ToggleRunaheadOnline => {
                                    cfg.runahead_online = !cfg.runahead_online;
                                    config::save(&cfg);
                                    if let AppState::Menu(MenuScreen::Settings {
                                        ref mut runahead_online,
                                        ..
                                    }) = state
                                    {
                                        *runahead_online = cfg.runahead_online;
                                    }
                                    toast = Some((
                                        format!(
                                            "Runahead (online, experimental) {}",
                                            if cfg.runahead_online { "ON" } else { "OFF" }
                                        ),
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
                                NavResult::Stay => {
                                    // "Online" (Main Menu item 0) landed in legacy's
                                    // OnlineHub/Play tab via ActivateMainItem's
                                    // delegation — hand off to fp_ui's own Lobby
                                    // screen instead when the new UI is on, same
                                    // pattern as OpenSettings just above.
                                    if cfg.new_ui {
                                        if let AppState::Menu(MenuScreen::OnlineHub {
                                            tab: menu::OnlineTab::Play,
                                            ..
                                        }) = &state
                                        {
                                            state = AppState::FpUi(fp_ui::FpScreen::lobby());
                                        }
                                    }
                                }
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
                                                        "[ghost] Loaded drone opponent: {} frames",
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
                                NavResult::LoadRemoteReplay(url) => {
                                    set_replay_select_status(
                                        &mut state,
                                        "Downloading public replay...",
                                    );
                                    match download_remote_replay(&url) {
                                        Ok(path) => {
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
                                                        set_replay_select_status(
                                                            &mut state,
                                                            format!("Error: {e}"),
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            println!("[replay] Remote load failed: {e}");
                                            set_replay_select_status(
                                                &mut state,
                                                format!("Error: {e}"),
                                            );
                                        }
                                    }
                                }
                            },
                            MenuNav::Left => state.nav_left(),
                            MenuNav::Right => state.nav_right(),
                            MenuNav::Back => {
                                // Back on the incoming-challenge modal declines it.
                                let declined = if let AppState::Menu(
                                    menu::MenuScreen::OnlineHub { incoming, .. },
                                ) = &mut state
                                {
                                    incoming.take().map(|c| c.challenge_id)
                                } else {
                                    None
                                };
                                // Leaving a lobby tells the server (auto-destroys
                                // when empty) and returns to the hub.
                                let left_lobby = if let AppState::Menu(
                                    menu::MenuScreen::Lobby { id, .. },
                                ) = &state
                                {
                                    Some(id.clone())
                                } else {
                                    None
                                };
                                if let Some(id) = declined {
                                    matchmaking::decline_challenge(id);
                                } else if let Some(id) = left_lobby {
                                    if !id.is_empty() {
                                        matchmaking::leave_lobby(id);
                                    }
                                    lobby_view_rx = None;
                                    state = menu::main_menu_state(cfg.new_ui);
                                } else {
                                    state.nav_back(cfg.new_ui);
                                }
                            }
                            MenuNav::ToggleMenu => {}
                            MenuNav::SwitchPlayer => state.nav_switch_player(),
                        }
                    }
                }

                _ if state == AppState::Playing => match event {
                    // ── Replay takeover ─────────────────────────────────────
                    // Exit takeover → back to the paused replay at the moment.
                    Event::KeyDown {
                        keycode: Some(Keycode::Escape),
                        repeat: false,
                        ..
                    } if replay_takeover.is_some() => {
                        if let Some(tk) = replay_takeover.take() {
                            reload_takeover_moment(&core, &mut match_replay_playback, &tk);
                        }
                        replay_review_paused = true;
                        input::clear_all_inputs();
                        toast = Some((
                            "Exited takeover".into(),
                            Instant::now() + Duration::from_millis(1400),
                        ));
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::B | sdl2::controller::Button::Back,
                        ..
                    } if replay_takeover.is_some() => {
                        if let Some(tk) = replay_takeover.take() {
                            reload_takeover_moment(&core, &mut match_replay_playback, &tk);
                        }
                        replay_review_paused = true;
                        input::clear_all_inputs();
                    }
                    // Retry the same moment.
                    Event::KeyDown {
                        keycode: Some(Keycode::R),
                        repeat: false,
                        ..
                    } if replay_takeover.is_some() => {
                        if let Some(tk) = replay_takeover.as_ref() {
                            reload_takeover_moment(&core, &mut match_replay_playback, tk);
                        }
                        if let Some(tk) = replay_takeover.as_mut() {
                            tk.phase = TakeoverPhase::Countdown;
                            tk.countdown = TAKEOVER_COUNTDOWN_FRAMES;
                            tk.frames_left = TAKEOVER_ACTIVE_FRAMES;
                        }
                        input::clear_all_inputs();
                    }
                    Event::ControllerButtonDown {
                        button: sdl2::controller::Button::Start | sdl2::controller::Button::A,
                        ..
                    } if replay_takeover.is_some() => {
                        if let Some(tk) = replay_takeover.as_ref() {
                            reload_takeover_moment(&core, &mut match_replay_playback, tk);
                        }
                        if let Some(tk) = replay_takeover.as_mut() {
                            tk.phase = TakeoverPhase::Countdown;
                            tk.countdown = TAKEOVER_COUNTDOWN_FRAMES;
                            tk.frames_left = TAKEOVER_ACTIVE_FRAMES;
                        }
                        input::clear_all_inputs();
                    }
                    // Enter takeover from the replay viewer: 1 = P1, 2 = P2.
                    Event::KeyDown {
                        keycode: Some(Keycode::Num1 | Keycode::Kp1),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() && replay_takeover.is_none() => {
                        start_replay_takeover(
                            &core,
                            &match_replay_playback,
                            &mut replay_takeover,
                            &mut replay_review_paused,
                            input::Player::P1,
                            &mut toast,
                        );
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::Num2 | Keycode::Kp2),
                        repeat: false,
                        ..
                    } if match_replay_playback.is_some() && replay_takeover.is_none() => {
                        start_replay_takeover(
                            &core,
                            &match_replay_playback,
                            &mut replay_takeover,
                            &mut replay_review_paused,
                            input::Player::P2,
                            &mut toast,
                        );
                    }
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
                        state = replay_select_exit_state(cfg.new_ui, "Replay stopped");
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
                        state = replay_select_exit_state(cfg.new_ui, "Replay stopped");
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
                        state = menu::main_menu_state(cfg.new_ui);
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
                            println!("[drone] Disabled - sequential drone playback");
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
                            println!("[replay] Drone recording disabled during replay playback.");
                        } else if net_session.is_some() {
                            println!("[ghost] Drone recording disabled in netplay mode.");
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
                            println!("[ghost] Can't record during drone playback.");
                        } else if let Some(c) = &core {
                            match ghost::Recording::start(c) {
                                Some(rec) => ghost_recording = Some(rec),
                                None => println!("[ghost] Couldn't capture drone anchor state."),
                            }
                        }
                    }
                    Event::KeyDown {
                        keycode: Some(Keycode::F8),
                        repeat: false,
                        ..
                    } if local_play_mode.is_lab() => {
                        if match_replay_playback.is_some() {
                            println!("[replay] Drone playback disabled during replay playback.");
                        } else if net_session.is_some() {
                            println!("[ghost] Drone playback disabled in netplay mode.");
                        } else if ghost_recording.is_some() {
                            println!("[ghost] Can't play drone while recording.");
                        } else if ghost_playback.is_some() {
                            ghost_playback = None;
                            println!("[ghost] Drone playback stopped.");
                        } else if let Some(c) = &core {
                            match ghost::Playback::load(&ghost_path) {
                                Ok(pb) => {
                                    if pb.prime(c) {
                                        println!(
                                            "[ghost] Playing drone back {} frames (full)...",
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
                            println!("[replay] Drone opponent disabled during replay playback.");
                        } else if net_session.is_some() {
                            println!("[ghost] Drone playback disabled in netplay mode.");
                        } else if ghost_recording.is_some() {
                            println!("[ghost] Can't play drone while recording.");
                        } else if ghost_playback.is_some() {
                            ghost_playback = None;
                            println!("[ghost] Drone playback stopped.");
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
                                            "[ghost] Logic drone loaded: {} frames, you are P1...",
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
                        net_stats.on_overlay_toggle(net_stats_visible);
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
                        let mut reached_gameover_this_frame = false;
                        if gstate == GS_FIGHTING {
                            net_in_fight = true;
                        } else if net_in_fight && gstate == GS_GAMEOVER {
                            net_in_fight = false;
                            reached_gameover_this_frame = true;
                        }

                        // ── Live score tracking ──────────────────────────────────────
                        let now_score = score::Score::read(c);
                        let mut emitted_match_over = false;
                        for ev in score_tracker.step(now_score) {
                            let mut result_match_index = None;
                            // `set_over` tells the server this game completed the
                            // whole best-of-N set, so a KoH lobby rotates only now.
                            let mut set_over = false;
                            if let score::ScoreEvent::MatchOver { winner, .. } = ev {
                                emitted_match_over = true;
                                ranked_match_index = ranked_match_index.saturating_add(1);
                                result_match_index = Some(ranked_match_index);
                                if winner == 1 {
                                    session_p1_wins += 1;
                                } else {
                                    session_p2_wins += 1;
                                }
                                if sync_completed_net_matches(
                                    &mut net_match_count,
                                    session_p1_wins,
                                    session_p2_wins,
                                ) && log_completed_net_match(
                                    net_match_count,
                                    &mut net_log,
                                    net_match_limit,
                                ) {
                                    set_over = true;
                                    mark_net_set_complete_pending(
                                        &mut net_set_complete_pending_frame,
                                        net_frame_counter,
                                        &mut net_log,
                                        net_match_limit,
                                    );
                                }
                            } else if ev == score::ScoreEvent::NewMatch
                                && net_set_complete_pending_frame.is_some()
                                && net_teardown_reason.is_none()
                            {
                                net_teardown_reason = Some(format!(
                                    "game limit reached ({net_match_limit} games)"
                                ));
                            }
                            handle_score_event(
                                ev,
                                local_handle,
                                discord_user.as_deref(),
                                &cfg.discord_webhook_url,
                                &mut net_log,
                                mm_session_id.as_deref(),
                                result_match_index,
                                set_over,
                            );
                        }
                        if reached_gameover_this_frame && !emitted_match_over {
                            if let Some(ev) = inferred_match_over(now_score, MATCH_WIN_TARGET) {
                                println!("[score] inferred match result at gameover");
                                let mut result_match_index = None;
                                let mut set_over = false;
                                if let score::ScoreEvent::MatchOver { winner, .. } = ev {
                                    ranked_match_index = ranked_match_index.saturating_add(1);
                                    result_match_index = Some(ranked_match_index);
                                    if winner == 1 {
                                        session_p1_wins += 1;
                                    } else {
                                        session_p2_wins += 1;
                                    }
                                    if sync_completed_net_matches(
                                        &mut net_match_count,
                                        session_p1_wins,
                                        session_p2_wins,
                                    ) && log_completed_net_match(
                                        net_match_count,
                                        &mut net_log,
                                        net_match_limit,
                                    ) {
                                        set_over = true;
                                        mark_net_set_complete_pending(
                                            &mut net_set_complete_pending_frame,
                                            net_frame_counter,
                                            &mut net_log,
                                            net_match_limit,
                                        );
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
                                    set_over,
                                );
                            } else {
                                println!(
                                    "[score] gameover reached without final round counters: P1 {} - {} P2",
                                    now_score.p1_match_wins, now_score.p2_match_wins
                                );
                            }
                        }

                        let pre_confirmed = sess.confirmed_frame();
                        let pre_ready = matches!(sess.current_state(), ggrs::SessionState::Running);

                        let step_stats = step_netplay_frame(
                            c,
                            sess,
                            cfg.runahead_online,
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

                        // King-of-the-hill: stream a periodic screenshot to the
                        // lobby so spectators see the live match. Capture here on
                        // the main thread (the core just advanced FRAME_BUFFER);
                        // compression + upload happen off-thread.
                        if let Some(lobby_id) = lobby_return.as_ref() {
                            if Instant::now() >= lobby_thumb_next_push {
                                lobby_thumb_next_push =
                                    Instant::now() + Duration::from_secs(25);
                                if let Some(rgba) = render::capture_frame_thumbnail() {
                                    matchmaking::push_lobby_thumbnail(lobby_id.clone(), rgba);
                                }
                            }
                        }

                        if net_frame_counter < 10 {
                            if let Some(f) = net_log.as_mut() {
                                use std::io::Write;
                                let _ = writeln!(f,
                                    "[net/early] frame={} sess_state={:?} advance_count={} saves={} loads={} timing_us=S{} H{} L{} confirmed={}",
                                    net_frame_counter, sess.current_state(),
                                    step_stats.advance_count, step_stats.save_count,
                                    step_stats.load_count, step_stats.save_state_micros,
                                    step_stats.checksum_micros, step_stats.load_state_micros,
                                    sess.confirmed_frame());
                            }
                        }

                        if step_stats.advance_count > 1 {
                            if let Some(f) = net_log.as_mut() {
                                use std::io::Write;
                                let _ = writeln!(
                                    f,
                                    "[net/rollback] frame={} resim_depth={} saves={} loads={} timing_us=S{} H{} L{}",
                                    net_frame_counter,
                                    step_stats.advance_count - 1,
                                    step_stats.save_count,
                                    step_stats.load_count,
                                    step_stats.save_state_micros,
                                    step_stats.checksum_micros,
                                    step_stats.load_state_micros
                                );
                            }
                        }
                        net_stats.record_step(step_stats);

                        let post_confirmed = sess.confirmed_frame();
                        if pre_ready && post_confirmed > pre_confirmed {
                            net_frames_since_progress = 0;
                        } else {
                            net_frames_since_progress = net_frames_since_progress.saturating_add(1);
                        }
                        if step_stats.peer_disconnected && net_teardown_reason.is_none() {
                            net_teardown_reason = Some("peer disconnected".into());
                        } else if step_stats.desync_detected && net_teardown_reason.is_none() {
                            net_teardown_reason = Some("desync detected".into());
                        } else if net_frames_since_progress > 120 && net_teardown_reason.is_none() {
                            net_teardown_reason = Some("peer timed out (no progress)".into());
                        }
                        if pending_net_set_expired(
                            net_set_complete_pending_frame,
                            net_frame_counter,
                            NETPLAY_SET_COMPLETE_GRACE_FRAMES,
                        ) && net_teardown_reason.is_none()
                        {
                            net_teardown_reason =
                                Some(format!("game limit reached ({net_match_limit} games)"));
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
                        if net_frame_counter >= net_stats.next_network_sample_frame {
                            net_stats.next_network_sample_frame =
                                net_frame_counter.wrapping_add(275);
                            let remote_handle = 1 - local_handle;
                            if let Ok(stats) = sess.network_stats(remote_handle) {
                                net_stats.ping_ms = Some(stats.ping as i32);
                                net_stats.kbps_sent = Some(stats.kbps_sent.to_string());
                                net_stats.local_frames_behind =
                                    Some(stats.local_frames_behind.to_string());
                                net_stats.remote_frames_behind =
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
                    } else if replay_takeover.is_some() {
                        if match_replay_playback.is_none() {
                            // The underlying replay went away — bail out cleanly.
                            replay_takeover = None;
                            input::clear_all_inputs();
                        } else if let (Some(tk), Some(pb)) =
                            (replay_takeover.as_mut(), match_replay_playback.as_mut())
                        {
                            match tk.phase {
                                TakeoverPhase::Countdown => {
                                    // Frozen on the moment — neutral inputs, no
                                    // core advance — while 3-2-1 counts down.
                                    input::apply_snapshot(input::Player::P1, 0);
                                    input::apply_snapshot(input::Player::P2, 0);
                                    if tk.countdown > 0 {
                                        tk.countdown -= 1;
                                    } else {
                                        tk.phase = TakeoverPhase::Active;
                                    }
                                }
                                TakeoverPhase::Active => {
                                    // Human drives their port from live input;
                                    // the opponent replays its recorded inputs.
                                    let human_bits = input::snapshot_player(input::Player::P1);
                                    input::apply_snapshot(tk.human, human_bits);
                                    if pb.inject_ai_side(tk.human) {
                                        unsafe {
                                            (c.run)();
                                        }
                                        if tk.frames_left > 0 {
                                            tk.frames_left -= 1;
                                        }
                                        if tk.frames_left == 0 {
                                            tk.phase = TakeoverPhase::Done;
                                        }
                                    } else {
                                        tk.phase = TakeoverPhase::Done;
                                    }
                                }
                                TakeoverPhase::Done => {
                                    input::apply_snapshot(input::Player::P1, 0);
                                    input::apply_snapshot(input::Player::P2, 0);
                                    // Frozen — waiting for R (retry) or Esc.
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
                                clear_audio_buffer();
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
                                state = replay_select_exit_state(cfg.new_ui, "Replay complete");
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
                                retro::set_input(0, RETRO_DEVICE_ID_JOYPAD_START as usize, pulse);
                                auto_start_frame = auto_start_frame.wrapping_add(1);
                            } else if gstate != 0 {
                                retro::set_input(0, RETRO_DEVICE_ID_JOYPAD_START as usize, false);
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
                                    let mut row = [false; 16];
                                    for (b, slot) in row.iter_mut().enumerate() {
                                        *slot = (ghost_input >> b) & 1 != 0;
                                    }
                                    retro::set_input_port(target_port, row);
                                } else if !pb.inject_next(ghost_port_mask) {
                                    pb.rewind_inputs();
                                    let _ = pb.inject_next(ghost_port_mask);
                                }
                            } else {
                                // Normal drone playback through the legacy ghost format.
                                if !pb.inject_next(ghost_port_mask) {
                                    if ghost_port_mask == 0b10 {
                                        pb.rewind_inputs();
                                        let _ = pb.inject_next(ghost_port_mask);
                                    } else {
                                        println!(
                                            "[ghost] Drone playback complete ({} frames).",
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
                        // Shared score tracking for local/drone play
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
                        // Offline interactive play gets one-frame runahead:
                        // the screen shows a frame further into MK2's input
                        // pipeline while all logic below still reads the
                        // canonical state the runahead step restores.
                        if cfg.runahead && net_session.is_none() && match_replay_playback.is_none()
                        {
                            local_runahead.step(c);
                        } else {
                            unsafe {
                                (c.run)();
                            }
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
                        // Drone match vs detection: track fight start/end for webhook
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
                                    "Drone Match Result - Player {} (P1 HP: 0x{p1_hp:04X} | P2 HP: 0x{p2_hp:04X})",
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
                        let rollback_recovery_audio =
                            net_session.is_some() && net_stats.rollback_frames > 0;
                        retro::with_audio_mut(|audio| {
                            if audio.is_empty() {
                                return;
                            }
                            prepare_game_audio(
                                audio,
                                rollback_recovery_audio,
                                &mut audio_tail_sample,
                            );
                            if let Some(recorder) = clip_recorder.as_mut() {
                                recorder.record_audio(audio);
                            }
                            queue_game_audio(q, audio, cfg.volume_percent, cfg.audio_buffer);
                            audio.clear();
                        });
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
                        let completed_set = reason.starts_with("game limit reached")
                            || reason.starts_with("match limit reached");

                        // Auto-incident on abnormal teardowns. We skip
                        // intentional_quit (user pressed back) and
                        // completed_set (clean BO3 finish) because those
                        // aren't failures. Everything else — disconnect,
                        // timeout, GGRS desync — gets uploaded.
                        if !intentional_quit && !completed_set {
                            let kind = if reason.contains("disconnected") {
                                incident::KIND_GGRS_DISCONNECTED
                            } else if reason.contains("desync") {
                                incident::KIND_GGRS_DESYNC
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
                            inc.transport_path = net_transport_path;
                            inc.ggrs_state = net_session
                                .as_ref()
                                .map(|s| format!("{:?}", s.current_state()));
                            attach_relay_diagnostics(&mut inc, relay_chat.as_ref());
                            inc.net_log_path = Some(std::path::PathBuf::from("freeplay-net.log"));
                            let (_size, hash) = rom_fingerprint();
                            inc.rom_hash = Some(format!("{:016x}", hash));
                            incident::submit(inc);
                        }
                        let player_role = if local_handle == 0 { "P1" } else { "P2" };
                        let opponent = peer_name.as_deref().unwrap_or("Opponent");
                        let final_score =
                            format!("Set score: P1 {session_p1_wins} - {session_p2_wins} P2");
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
                                format!("Games completed: {}", net_match_count),
                                String::new(),
                                "Results and drones finalized where available.".into(),
                                "ENTER returns to the main menu.".into(),
                            ]
                        } else if intentional_quit {
                            vec![
                                "You left the match.".into(),
                                final_score,
                                format!("You were {player_role} vs {opponent}."),
                                duration,
                                format!("Games completed: {}", net_match_count),
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
                                format!("Games completed: {}", net_match_count),
                                String::new(),
                                "WARN Match data was saved where possible.".into(),
                                "You can return to the menu and queue again.".into(),
                                "Log: freeplay-net.log".into(),
                            ]
                        };
                        let (_rom_size, rom_hash_u64) = rom_fingerprint();
                        let rom_hash = format!("{:016x}", rom_hash_u64);
                        finalize_net_recording(
                            &mut net_recording,
                            &mut ghost_library,
                            &cfg.stats_url,
                            discord_user.as_deref(),
                            discord_id.as_deref(),
                            &rom_hash,
                        );
                        let (replay_p1, replay_p2) = replay_names(
                            local_handle,
                            discord_user.as_deref(),
                            peer_name.as_deref(),
                        );
                        let replay_path = match_replay::finalize_recording(
                            &mut match_replay_recording,
                        )
                        .map(|path| {
                            if let Err(e) = write_replay_summary(
                                &path,
                                &replay_p1,
                                &replay_p2,
                                session_p1_wins,
                                session_p2_wins,
                                net_frame_counter,
                                net_match_count,
                                mm_session_id.as_deref(),
                                &reason,
                                completed_set,
                            ) {
                                println!("[replay] Summary save failed: {e}");
                            }
                            if !cfg.stats_url.is_empty() && discord_user.is_some() {
                                replay_upload::upload_replay_to_stats(
                                    &cfg.stats_url,
                                    &path,
                                    discord_id.as_deref().unwrap_or(""),
                                    discord_user.as_deref().unwrap_or(""),
                                    &rom_hash,
                                );
                            }
                            path.to_string_lossy().into_owned()
                        });
                        net_session = None;
                        set_netplay_window_chrome(&mut canvas, false);
                        net_match_count = 0;
                        net_in_fight = false;
                        net_set_complete_pending_frame = None;
                        net_frames_since_progress = 0;
                        ranked_match_index = 0;
                        net_spectate_next = 165;
                        net_frame_counter = 0;
                        net_overlay_diagnosed = false;
                        net_runtime = NetRuntime::default();
                        net_log = None;
                        net_stats.reset();
                        audio_tail_sample = None;
                        relay_chat = None;
                        net_transport_path = None;
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
                        replay_upload::drain_upload_queue(&cfg.stats_url);
                        if !cfg.stats_url.trim().is_empty() {
                            let (tx, rx) = std::sync::mpsc::channel();
                            leaderboard_rx = Some(rx);
                            matchmaking::fetch_leaderboard(cfg.stats_url.clone(), tx);
                            main_leaderboard = menu::LeaderboardState::Loading;
                        }
                        score_tracker.reset();
                        session_p1_wins = 0;
                        session_p2_wins = 0;
                        trainer.set_enabled("hitboxes", false);
                        trainer.set_enabled("p1_health", false);
                        trainer.set_enabled("p2_health", false);
                        trainer.set_enabled("freeze_timer", false);
                        input::clear_all_inputs();
                        if let Some(lobby_id) = lobby_return.take() {
                            // Came from a KoH lobby — go back to it. The server
                            // rotates the queue from the match result (winner
                            // stays, loser re-queues); we just resume polling.
                            lobby_view_rx = None;
                            lobby_view_next_refresh = Instant::now();
                            state = if cfg.new_ui {
                                AppState::FpUi(fp_ui::FpScreen::LobbyRoom {
                                    id: lobby_id,
                                    view: None,
                                    status: "Returning to lobby...".into(),
                                    thumb: None,
                                })
                            } else {
                                AppState::Menu(MenuScreen::Lobby {
                                    id: lobby_id,
                                    view: None,
                                    status: "Returning to lobby...".into(),
                                    thumb: None,
                                })
                            };
                        } else if cfg.new_ui {
                            state = AppState::FpUi(fp_ui::FpScreen::SessionEnded {
                                lines: teardown_lines,
                                replay_path,
                                choice: 0,
                            });
                        } else {
                            state = AppState::Menu(MenuScreen::SessionEnded {
                                lines: teardown_lines,
                                replay_path,
                            });
                        }
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
                // Fight overlays wait for spawned fighters so they do not cover
                // the VS portrait/loading screen.
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
                        let fighters_spawned = matches!(gstate, GS_FIGHTING | 0x03) && p1_hp > 0;
                        !match_decided && fighters_spawned
                    })
                    .unwrap_or(false);
                // Diagnostic: log the score-bar inputs the first time fighters
                // spawn in a netplay match (or a forced late log if they never
                // seem to), so a missing bar can be traced to gstate/hp/RAM.
                if net_session.is_some()
                    && !net_overlay_diagnosed
                    && (overlay_screen || net_frame_counter > 900)
                {
                    net_overlay_diagnosed = true;
                    if let Some(c) = core.as_ref() {
                        let gstate =
                            memory::peek_u16(c, GSTATE_ADDR, memory::Endian::Little);
                        let p1_hp = memory::peek_u16(c, P1_HP_ADDR, memory::Endian::Little);
                        let sram = c
                            .memory(retro::RETRO_MEMORY_SYSTEM_RAM)
                            .map(|r| r.len())
                            .unwrap_or(0);
                        let s = score::Score::read(c);
                        let line = format!(
                            "[net/overlay] sysram={sram} gstate={gstate:?} p1_hp={p1_hp:?} match_wins={}/{} overlay_screen={overlay_screen} scorebar={:?}",
                            s.p1_match_wins, s.p2_match_wins, cfg.scorebar_style
                        );
                        println!("{line}");
                        if let Some(f) = net_log.as_mut() {
                            use std::io::Write;
                            let _ = writeln!(f, "{line}");
                        }
                    }
                }
                if overlay_screen
                    && (net_session.is_some()
                        || local_play_mode.is_lab()
                        || match_replay_playback.is_some())
                {
                    if discord_user.is_none() {
                        discord_user = matchmaking::username_from_cached_token();
                    }
                    // Local display name: Discord name when signed in, otherwise
                    // the claimed guest name from config (most online players are
                    // guests now). Without this fallback the namebar showed
                    // "P1"/"P2" for guests instead of their name.
                    let local_name = discord_user
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                        .or_else(|| {
                            let n = cfg.player_username.trim();
                            (!n.is_empty()).then_some(n)
                        })
                        .unwrap_or("You");
                    let ghost_name = "Drone";
                    let p1 = if net_session.is_some() {
                        if local_handle == 0 {
                            Some(local_name)
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
                            Some(local_name)
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
                if let Some(tk) = replay_takeover.as_ref() {
                    canvas.set_logical_size(0, 0)?;
                    let (win_w, win_h) = canvas.output_size().unwrap_or((1200, 762));
                    let countdown_num = if tk.phase == TakeoverPhase::Countdown {
                        Some(tk.countdown / 55 + 1)
                    } else {
                        None
                    };
                    let secs_left = if tk.phase == TakeoverPhase::Active {
                        Some((tk.frames_left + 54) / 55)
                    } else {
                        None
                    };
                    render::draw_takeover_overlay(
                        &mut canvas,
                        &mut font,
                        win_w as i32,
                        win_h as i32,
                        tk.human == input::Player::P1,
                        countdown_num,
                        secs_left,
                        tk.phase == TakeoverPhase::Done,
                    )
                    .map_err(|e| format!("takeover overlay: {e}"))?;
                    canvas.set_logical_size(LOGICAL_W as u32, LOGICAL_H as u32)?;
                } else if let Some(pb) = match_replay_playback.as_ref() {
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
                    let ping_label = net_stats.ping_label();
                    let mk2_perf = net_stats.sample_mk2_perf(core.as_ref(), net_frame_counter);
                    let detail_rows = net_stats.detail_rows(mk2_perf);
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
            AppState::Menu(MenuScreen::Matchmaking { .. })
            | AppState::FpUi(fp_ui::FpScreen::Matchmaking { .. })
            | AppState::FpUi(fp_ui::FpScreen::Lobby { quick_match_status: Some(_), .. })
            | AppState::FpUi(fp_ui::FpScreen::DiscordConnect { .. }) => {
                if let Some(rx) = &mm_rx {
                    loop {
                        match rx.try_recv() {
                            Ok(matchmaking::Update::Status(s)) => {
                                match &mut state {
                                    AppState::Menu(MenuScreen::Matchmaking { status }) => *status = s,
                                    AppState::FpUi(fp_ui::FpScreen::Matchmaking { status }) => *status = s,
                                    AppState::FpUi(fp_ui::FpScreen::DiscordConnect { status }) => *status = s,
                                    AppState::FpUi(fp_ui::FpScreen::Lobby { quick_match_status, .. }) => {
                                        *quick_match_status = Some(s);
                                    }
                                    _ => {}
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
                                // Land back on the Settings→Account row the connect
                                // started from — the *native* one when the new UI is
                                // on (this used to always drop to legacy Settings,
                                // the one real leak in the fp_ui Account flow).
                                state = if cfg.new_ui {
                                    AppState::FpUi(fp_ui::FpScreen::settings_account(&cfg))
                                } else {
                                    AppState::Menu(MenuScreen::Settings {
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
                                        runahead: cfg.runahead,
                                        runahead_online: cfg.runahead_online,
                                    })
                                };
                                break;
                            }
                            Ok(matchmaking::Update::Connected {
                                peer_endpoint,
                                is_host,
                                transport,
                                session_id,
                                room_id,
                                peer_username,
                            }) => {
                                let stun_peer: std::net::SocketAddr = match peer_endpoint.parse() {
                                    Ok(a) => a,
                                    Err(e) => {
                                        println!("[mm] bad peer addr from matchmaking: {e}");
                                        mm_rx = None;
                                        state = menu::main_menu_state(cfg.new_ui);
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
                                let transport_path = match &transport {
                                    matchmaking::MatchTransport::Relay { .. } => "relay",
                                    matchmaking::MatchTransport::Direct { .. } => "direct",
                                };

                                ensure_core_loaded(&mut core, &mut audio_queue, &audio_subsystem)?;
                                let mut log = open_net_log();
                                shutdown_local_runtime_for_netplay(
                                    core.as_ref(),
                                    audio_queue.as_ref(),
                                    &mut trainer,
                                    &mut lab_reset_slots,
                                    &mut lab_dummy,
                                    &mut punish_trainer,
                                    &mut damage_tracker,
                                    &mut ghost_playback,
                                    &mut ghost_recording,
                                    &mut drone_runner,
                                    &mut ghost_port_mask,
                                    &mut match_replay_playback,
                                    &mut match_replay_recording,
                                    &mut replay_review_paused,
                                    &mut replay_review_tick,
                                    &mut replay_clip_in,
                                    &mut replay_clip_out,
                                    &mut clip_recorder,
                                    &mut input_history,
                                    &mut score_tracker,
                                    &mut local_play_mode,
                                    &mut session_p1_wins,
                                    &mut session_p2_wins,
                                    &mut auto_start_done,
                                    &mut auto_start_frame,
                                    &mut audio_tail_sample,
                                    log.as_mut(),
                                    "matchmaking connected",
                                );
                                net_runtime = NetRuntime::default();
                                net_match_count = 0;
                                // King-of-the-hill lobby matches are FT1 (winner
                                // of one game stays); everything else uses the
                                // default best-of. lobby_return is set before we
                                // route into this Connected handler.
                                net_match_limit = if lobby_return.is_some() {
                                    1
                                } else {
                                    NETPLAY_MATCH_LIMIT
                                };
                                ranked_match_index = 0;
                                net_in_fight = false;
                                net_set_complete_pending_frame = None;
                                net_frames_since_progress = 0;

                                local_handle = if is_host { 0 } else { 1 };
                                net_transport_path = None;
                                let mut lines: Vec<String> = Vec::new();

                                // Branch: relay vs direct UDP
                                let result: Result<netplay::Session, Box<dyn std::error::Error>> =
                                    match transport {
                                        matchmaking::MatchTransport::Relay { socket } => {
                                            let peer_label = socket.peer_label();
                                            let relay_diag = socket.diagnostics();
                                            relay_chat = match socket.chat_handle() {
                                                Ok(handle) => Some(handle),
                                                Err(e) => {
                                                    println!("[chat] relay chat unavailable: {e}");
                                                    None
                                                }
                                            };
                                            if let Some(f) = log.as_mut() {
                                                use std::io::Write;
                                                let _ = writeln!(f,
                                                    "[net] relay session ready, routing through {peer_label} (registered={}, peer_ready={}, data_received={})",
                                                    socket.is_registered(),
                                                    socket.is_peer_ready(),
                                                    relay_diag.data_received);
                                            }
                                            println!(
                                                "[net] relay session ready (registered={}, peer_ready={}, data_received={})",
                                                socket.is_registered(),
                                                socket.is_peer_ready(),
                                                relay_diag.data_received
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
                                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
                                        }
                                        matchmaking::MatchTransport::Direct { peer_addr } => {
                                            // ── DIRECT UDP PATH ──
                                            relay_chat = None;
                                            let log_ref = &mut log;
                                            let lines_ref = &mut lines;
                                            netplay::start_session_verbose(
                                                menu::DEFAULT_NETPLAY_PORT,
                                                local_handle,
                                                peer_addr,
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
                                        }
                                    };

                                match result {
                                    Ok(s) => {
                                        net_session = Some(s);
                                        set_netplay_window_chrome(&mut canvas, true);
                                        net_transport_path = Some(transport_path);
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
                                        net_stats.reset();
                                        audio_tail_sample = None;
                                        net_frame_counter = 0;
                                        net_overlay_diagnosed = false;
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
                                        inc.transport_path = Some(transport_path);
                                        inc.ggrs_state = Some("start_session_failed".into());
                                        attach_relay_diagnostics(&mut inc, relay_chat.as_ref());
                                        inc.net_log_path =
                                            Some(std::path::PathBuf::from("freeplay-net.log"));
                                        let (_size, hash) = rom_fingerprint();
                                        inc.rom_hash = Some(format!("{:016x}", hash));
                                        incident::submit(inc);

                                        state = connection_failed_state(cfg.new_ui, lines);
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
                                inc.transport_path = if kind == incident::KIND_TURN_FALLBACK_FAILED
                                {
                                    Some("relay")
                                } else if kind == incident::KIND_HOLE_PUNCH_FAILED {
                                    Some("direct")
                                } else {
                                    None
                                };
                                inc.ggrs_state = Some("not_started".into());
                                inc.net_log_path =
                                    Some(std::path::PathBuf::from("freeplay-net.log"));
                                let (_size, hash) = rom_fingerprint();
                                inc.rom_hash = Some(format!("{:016x}", hash));
                                incident::submit(inc);

                                mm_rx = None;
                                net_transport_path = None;
                                relay_chat = None;
                                state = connection_failed_state(
                                    cfg.new_ui,
                                    vec![
                                        String::new(),
                                        format!("FAIL {e}"),
                                        String::new(),
                                        "ESC to go back".into(),
                                    ],
                                );
                                break;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                mm_rx = None;
                                state = menu::main_menu_state(cfg.new_ui);
                                break;
                            }
                        }
                    }
                }

                canvas.set_logical_size(0, 0)?;
                let (win_w, win_h) = canvas.output_size().unwrap_or((1200, 762));
                if let (AppState::FpUi(screen), Some(fpf)) = (&state, fp_fonts.as_mut()) {
                    let fp_username = discord_user.as_deref().unwrap_or(&cfg.player_username);
                    fp_ui::draw(
                        screen,
                        &mut canvas,
                        fpf,
                        win_w as i32,
                        win_h as i32,
                        fp_username,
                        &main_leaderboard,
                        &main_profile,
                        &cfg.bindings,
                        &cfg.stats_email,
                        discord_user.is_some(),
                        rom_present.check(),
                    )
                    .map_err(|e| format!("fp_ui draw: {e}"))?;
                    // fp_ui screens have no toast param of their own (unlike
                    // legacy's `menu::draw`, which takes one directly) — a
                    // toast set while in an FpUi state (e.g. "No local
                    // replay found for this match" from
                    // WatchLastMatchReplay) was silently never drawn,
                    // reading as "nothing happened" to the user. Draw it
                    // as an overlay on top with the same legacy bitmap font
                    // legacy screens use for it.
                    if let Some(toast) = toast_payload(&toast) {
                        menu::draw_toast(&mut canvas, &mut font, &toast, win_w as i32, win_h as i32)
                            .map_err(|e| format!("fp_ui toast overlay: {e}"))?;
                    }
                } else {
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
                        menu_input_pad,
                    )
                    .map_err(|e| format!("menu draw: {e}"))?;
                }
                if net_stats_visible && is_matchmaking_screen(&state) {
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

            AppState::Menu(_) | AppState::Rebinding { .. } | AppState::FpUi(_) => {
                if matches!(
                    state,
                    AppState::Menu(menu::MenuScreen::TestIp { editing: true, .. })
                        | AppState::Menu(menu::MenuScreen::TextEdit { .. })
                        | AppState::Menu(menu::MenuScreen::OnlineHub {
                            tab: menu::OnlineTab::Chat,
                            focus: menu::HubFocus::Content,
                            ..
                        })
                        | AppState::Menu(menu::MenuScreen::MatchUsername {
                            checking: false,
                            ..
                        })
                        | AppState::FpUi(fp_ui::FpScreen::ClaimUsername {
                            checking: false,
                            ..
                        })
                ) || is_fp_test_conn_editing(&state) {
                    video_subsystem.text_input().start();
                } else {
                    video_subsystem.text_input().stop();
                }

                if let Some(rx) = &username_check_rx {
                    let waiting_for_username = matches!(
                        state,
                        AppState::Menu(menu::MenuScreen::MatchUsername { .. })
                            | AppState::FpUi(fp_ui::FpScreen::ClaimUsername { .. })
                    ) || (username_check_silent && is_matchmaking_screen(&state));
                    if waiting_for_username {
                        let timed_out = username_check_started_at
                            .map(|started| started.elapsed() >= Duration::from_secs(12))
                            .unwrap_or(false);
                        if timed_out {
                            let value = match &state {
                                AppState::Menu(MenuScreen::MatchUsername { value, .. }) => value.clone(),
                                AppState::FpUi(fp_ui::FpScreen::ClaimUsername { value, .. }) => value.clone(),
                                _ => cfg.player_username.clone(),
                            };
                            set_username_screen(
                                &mut state,
                                value,
                                "Username check timed out. Press Enter to retry.".into(),
                                false,
                            );
                            username_check_rx = None;
                            username_check_silent = false;
                            username_check_started_at = None;
                        } else {
                            match rx.try_recv() {
                                Ok(matchmaking::UsernameCheckUpdate::Available(username)) => {
                                    cfg.player_username = username.clone();
                                    cfg.player_username_confirmed = true;
                                    cfg.player_username_autogenerated = false;
                                    config::save(&cfg);
                                    shutdown_for_online_start!("find-match queue");
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
                                    username_check_started_at = None;
                                }
                                Ok(matchmaking::UsernameCheckUpdate::Taken(username)) => {
                                    set_username_screen(
                                        &mut state,
                                        username,
                                        "That name is already taken".into(),
                                        false,
                                    );
                                    username_check_rx = None;
                                    username_check_silent = false;
                                    username_check_started_at = None;
                                }
                                Ok(matchmaking::UsernameCheckUpdate::Error(message)) => {
                                    let value = match &state {
                                        AppState::Menu(MenuScreen::MatchUsername { value, .. }) => value.clone(),
                                        AppState::FpUi(fp_ui::FpScreen::ClaimUsername { value, .. }) => value.clone(),
                                        _ => cfg.player_username.clone(),
                                    };
                                    set_username_screen(&mut state, value, message, false);
                                    username_check_rx = None;
                                    username_check_silent = false;
                                    username_check_started_at = None;
                                }
                                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                    username_check_rx = None;
                                    username_check_silent = false;
                                    username_check_started_at = None;
                                    set_username_screen(
                                        &mut state,
                                        cfg.player_username.clone(),
                                        "Username check stopped".into(),
                                        false,
                                    );
                                }
                            }
                        }
                    } else {
                        username_check_rx = None;
                        username_check_silent = false;
                        username_check_started_at = None;
                    }
                }

                if (matches!(state, AppState::Menu(menu::MenuScreen::LiveMatches { .. }))
                    || matches!(
                        state,
                        AppState::Menu(menu::MenuScreen::OnlineHub {
                            tab: menu::OnlineTab::Watch,
                            ..
                        })
                    )
                    || matches!(
                        state,
                        AppState::FpUi(fp_ui::FpScreen::Lobby { tab: 4, .. })
                    ))
                    && live_matches_rx.is_none()
                    && Instant::now() >= live_matches_next_refresh
                {
                    let (tx, rx) = std::sync::mpsc::channel();
                    live_matches_rx = Some(rx);
                    matchmaking::fetch_live_matches(tx);
                    live_matches_next_refresh = Instant::now() + Duration::from_secs(7);
                }

                // Refresh general-lobby presence/chat on Chat and Players (both
                // need the online roster; this also registers our presence so
                // others can challenge us). fp_ui's Chat tab (tab: 3) mirrors
                // the same fetch rather than a second pipeline.
                if (matches!(
                    state,
                    AppState::Menu(menu::MenuScreen::OnlineHub {
                        tab: menu::OnlineTab::Chat | menu::OnlineTab::Players,
                        ..
                    })
                ) || matches!(
                    state,
                    AppState::FpUi(fp_ui::FpScreen::Lobby { tab: 3, .. })
                )) && lobby_rx.is_none()
                    && Instant::now() >= lobby_next_refresh
                {
                    let (tx, rx) = std::sync::mpsc::channel();
                    lobby_rx = Some(rx);
                    matchmaking::fetch_general_lobby(tx);
                    lobby_next_refresh = Instant::now() + Duration::from_secs(7);
                }

                if let Some(rx) = &lobby_rx {
                    // fp_ui's Chat tab (tab: 3) mirrors legacy OnlineHub's
                    // chat/presence/status fields exactly, so the same fetch
                    // populates either shape.
                    let target = match &mut state {
                        AppState::Menu(menu::MenuScreen::OnlineHub {
                            status,
                            chat,
                            presence,
                            ..
                        }) => Some((status, chat, presence)),
                        AppState::FpUi(fp_ui::FpScreen::Lobby {
                            status,
                            chat,
                            presence,
                            ..
                        }) => Some((status, chat, presence)),
                        _ => None,
                    };
                    if let Some((status, chat, presence)) = target {
                        match rx.try_recv() {
                            Ok(matchmaking::LobbyUpdate::Loaded(snapshot)) => {
                                *status = snapshot.status.clone();
                                *chat = snapshot.chat;
                                *presence = snapshot.users;
                                lobby_rx = None;
                            }
                            Ok(matchmaking::LobbyUpdate::Error(message)) => {
                                *status = format!("Lobby unreachable: {message}");
                                lobby_rx = None;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                lobby_rx = None;
                            }
                        }
                    } else {
                        lobby_rx = None;
                    }
                }

                if let Some(rx) = &lobby_chat_post_rx {
                    let target = match &mut state {
                        AppState::Menu(menu::MenuScreen::OnlineHub { status, .. }) => {
                            Some(status)
                        }
                        AppState::FpUi(fp_ui::FpScreen::Lobby { status, .. }) => Some(status),
                        _ => None,
                    };
                    if let Some(status) = target {
                        match rx.try_recv() {
                            Ok(matchmaking::LobbyChatPostUpdate::Sent) => {
                                *status = "Message sent".into();
                                lobby_next_refresh = Instant::now();
                                lobby_chat_post_rx = None;
                            }
                            Ok(matchmaking::LobbyChatPostUpdate::Error(message)) => {
                                *status = format!("Send failed: {message}");
                                lobby_chat_post_rx = None;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                lobby_chat_post_rx = None;
                            }
                        }
                    } else {
                        lobby_chat_post_rx = None;
                    }
                }

                if (matches!(
                    state,
                    AppState::Menu(menu::MenuScreen::OnlineHub {
                        tab: menu::OnlineTab::Lobbies,
                        ..
                    })
                ) || matches!(
                    state,
                    AppState::FpUi(fp_ui::FpScreen::Lobby { tab: 2, .. })
                )) && lobby_list_rx.is_none()
                    && Instant::now() >= lobby_list_next_refresh
                {
                    let (tx, rx) = std::sync::mpsc::channel();
                    lobby_list_rx = Some(rx);
                    matchmaking::fetch_lobbies(tx);
                    lobby_list_next_refresh = Instant::now() + Duration::from_secs(10);
                }

                if let Some(rx) = &lobby_list_rx {
                    // Same arrival handling either way — fp_ui's Server
                    // Browser tab mirrors legacy OnlineHub's Lobbies tab
                    // fields exactly (status/lobbies/cursor) rather than a
                    // second fetch pipeline.
                    let target = match &mut state {
                        AppState::Menu(menu::MenuScreen::OnlineHub {
                            status,
                            lobbies,
                            cursor,
                            ..
                        }) => Some((status, lobbies, cursor)),
                        AppState::FpUi(fp_ui::FpScreen::Lobby {
                            status,
                            lobbies,
                            cursor,
                            ..
                        }) => Some((status, lobbies, cursor)),
                        _ => None,
                    };
                    if let Some((status, lobbies, cursor)) = target {
                        match rx.try_recv() {
                            Ok(matchmaking::LobbyListUpdate::Loaded(list)) => {
                                *lobbies = list.into_iter().map(lobby_room_to_preview).collect();
                                *cursor = 0;
                                *status = if lobbies.is_empty() {
                                    "No public lobbies right now".into()
                                } else {
                                    "Select a lobby to inspect".into()
                                };
                                lobby_list_rx = None;
                            }
                            Ok(matchmaking::LobbyListUpdate::Error(message)) => {
                                lobbies.clear();
                                *cursor = 0;
                                *status = format!("Lobbies unavailable: {message}");
                                lobby_list_rx = None;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                lobby_list_rx = None;
                            }
                        }
                    } else {
                        lobby_list_rx = None;
                    }
                }

                // Poll incoming challenges anywhere in the Online hub (legacy or
                // native) and raise a modal prompt when one arrives — a
                // challenge can land while any tab is showing, not just Players.
                if matches!(
                    state,
                    AppState::Menu(menu::MenuScreen::OnlineHub { .. })
                        | AppState::FpUi(fp_ui::FpScreen::Lobby { .. })
                ) && challenge_rx.is_none()
                    && Instant::now() >= challenge_next_refresh
                {
                    let (tx, rx) = std::sync::mpsc::channel();
                    challenge_rx = Some(rx);
                    matchmaking::fetch_challenges(tx);
                    challenge_next_refresh = Instant::now() + Duration::from_secs(4);
                }

                if let Some(rx) = &challenge_rx {
                    match rx.try_recv() {
                        Ok(matchmaking::ChallengeListUpdate::Loaded(list)) => {
                            if let AppState::Menu(menu::MenuScreen::OnlineHub {
                                ref mut incoming,
                                ..
                            })
                            | AppState::FpUi(fp_ui::FpScreen::Lobby {
                                ref mut incoming, ..
                            }) = state
                            {
                                if incoming.is_none() {
                                    *incoming = list.into_iter().next();
                                }
                            }
                            challenge_rx = None;
                        }
                        Ok(matchmaking::ChallengeListUpdate::Error(e)) => {
                            // Not signed in / transient — quiet, just retry later.
                            let _ = e;
                            challenge_rx = None;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            challenge_rx = None;
                        }
                    }
                }

                // King-of-the-hill lobby: poll state while the lobby room screen
                // is up — legacy or native, both carry the exact same fields
                // (`FpScreen::LobbyRoom` mirrors `MenuScreen::Lobby`'s shape), so
                // the same or-pattern binds either one.
                if let AppState::Menu(menu::MenuScreen::Lobby { id, .. })
                | AppState::FpUi(fp_ui::FpScreen::LobbyRoom { id, .. }) = &state
                {
                    if !id.is_empty()
                        && lobby_view_rx.is_none()
                        && Instant::now() >= lobby_view_next_refresh
                    {
                        let (tx, rx) = std::sync::mpsc::channel();
                        lobby_view_rx = Some(rx);
                        matchmaking::fetch_lobby(tx, id.clone());
                        lobby_view_next_refresh = Instant::now() + Duration::from_millis(2000);
                    }
                }
                if let Some(rx) = &lobby_view_rx {
                    match rx.try_recv() {
                        Ok(matchmaking::LobbyViewUpdate::Created(new_id)) => {
                            if let AppState::Menu(menu::MenuScreen::Lobby { id, status, .. })
                            | AppState::FpUi(fp_ui::FpScreen::LobbyRoom { id, status, .. }) =
                                &mut state
                            {
                                *id = new_id;
                                *status = "Loading lobby...".into();
                            }
                            lobby_view_next_refresh = Instant::now();
                            lobby_view_rx = None;
                        }
                        Ok(matchmaking::LobbyViewUpdate::Loaded(v)) => {
                            if v.your_turn && lobby_return.is_none() && net_session.is_none() {
                                // It's our turn — connect to the opponent the
                                // server paired us with and start netplay. We
                                // route through the Matchmaking screen so the
                                // shared mm_rx Connected->netplay path applies,
                                // and remember the lobby so we return to it.
                                let lobby_id = v.id.clone();
                                shutdown_for_online_start!("lobby match");
                                let (tx, rx) = std::sync::mpsc::channel();
                                mm_rx = Some(rx);
                                matchmaking::start_lobby_match(tx, lobby_id.clone());
                                lobby_return = Some(lobby_id);
                                // First thumbnail a few seconds in (past the
                                // round intro), then every ~25s.
                                lobby_thumb_next_push = Instant::now() + Duration::from_secs(8);
                                set_matchmaking_screen(&mut state, "Match starting — connecting...".into());
                            } else if let AppState::Menu(menu::MenuScreen::Lobby {
                                id,
                                view,
                                status,
                                thumb,
                                ..
                            })
                            | AppState::FpUi(fp_ui::FpScreen::LobbyRoom {
                                id,
                                view,
                                status,
                                thumb,
                            }) = &mut state
                            {
                                *id = v.id.clone();
                                // No live match → drop any stale thumbnail.
                                if v.current.is_none() {
                                    *thumb = None;
                                }
                                *view = Some(v);
                                *status = String::new();
                            }
                            lobby_view_rx = None;
                        }
                        Ok(matchmaking::LobbyViewUpdate::Error(e)) => {
                            if let AppState::Menu(menu::MenuScreen::Lobby { status, .. })
                            | AppState::FpUi(fp_ui::FpScreen::LobbyRoom { status, .. }) =
                                &mut state
                            {
                                *status = format!("Lobby unavailable: {e}");
                            }
                            lobby_view_rx = None;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            lobby_view_rx = None;
                        }
                    }
                }

                // King-of-the-hill: fetch the live match thumbnail while a match
                // is in progress in the lobby we're viewing.
                if let AppState::Menu(menu::MenuScreen::Lobby { id, view, .. })
                | AppState::FpUi(fp_ui::FpScreen::LobbyRoom { id, view, .. }) = &state
                {
                    let has_match = view.as_ref().map_or(false, |v| v.current.is_some());
                    if has_match
                        && !id.is_empty()
                        && lobby_thumb_rx.is_none()
                        && Instant::now() >= lobby_thumb_next_fetch
                    {
                        let (tx, rx) = std::sync::mpsc::channel();
                        lobby_thumb_rx = Some(rx);
                        matchmaking::fetch_lobby_thumbnail(tx, id.clone());
                        lobby_thumb_next_fetch = Instant::now() + Duration::from_millis(12000);
                    }
                }
                if let Some(rx) = &lobby_thumb_rx {
                    match rx.try_recv() {
                        Ok(rgba) => {
                            let expected =
                                (render::LOBBY_THUMB_W * render::LOBBY_THUMB_H * 4) as usize;
                            if rgba.len() == expected {
                                if let AppState::Menu(menu::MenuScreen::Lobby { thumb, .. })
                                | AppState::FpUi(fp_ui::FpScreen::LobbyRoom { thumb, .. }) =
                                    &mut state
                                {
                                    *thumb = Some((
                                        rgba,
                                        render::LOBBY_THUMB_W,
                                        render::LOBBY_THUMB_H,
                                    ));
                                }
                            }
                            lobby_thumb_rx = None;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            lobby_thumb_rx = None;
                        }
                    }
                }

                // Populate the Drone loader when entering the screen (both the
                // legacy screen and the native fp_ui one — see
                // `scan_local_ghost_entries`).
                if let AppState::Menu(menu::MenuScreen::GhostSelect {
                    ref mut entries, ..
                }) = state
                {
                    if entries.is_empty() {
                        *entries = scan_local_ghost_entries();
                    }
                }
                if let AppState::FpUi(fp_ui::FpScreen::GhostSelect {
                    ref mut entries, ..
                }) = state
                {
                    if entries.is_empty() {
                        *entries = scan_local_ghost_entries();
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
                            Ok(matchmaking::ProfileUpdate::Empty { username }) => {
                                *state = menu::ProfileScreenState::Empty { username };
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

                // Drain the Main Menu's own profile fetch (separate from
                // `profile_rx` above, which only updates while the dedicated
                // Profile screen is open).
                if let Some(rx) = &main_profile_rx {
                    match rx.try_recv() {
                        Ok(matchmaking::ProfileUpdate::Loaded { profile, history }) => {
                            main_profile = menu::ProfileScreenState::Loaded {
                                profile,
                                history,
                                avatar_rgba: None,
                            };
                            main_profile_rx = None;
                        }
                        Ok(matchmaking::ProfileUpdate::Empty { username }) => {
                            main_profile = menu::ProfileScreenState::Empty { username };
                            main_profile_rx = None;
                        }
                        Ok(matchmaking::ProfileUpdate::Error(msg)) => {
                            main_profile = menu::ProfileScreenState::Error(msg);
                            main_profile_rx = None;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            main_profile_rx = None;
                        }
                    }
                }

                // Drain the LAST MATCH card's remote-replay fallback lookup
                // (kicked off by `WatchLastMatchReplay` when no local file
                // matched) — search the public replay index for the same
                // opponent/time-window match `find_matching_local_replay`
                // looks for locally, and download+play it if found.
                let mut remote_replay_loaded: Option<(matchmaking::HistoryRow, Vec<matchmaking::RemoteReplayMeta>)> = None;
                let mut remote_replay_error: Option<String> = None;
                let mut remote_replay_disconnected = false;
                if let Some((row, rx)) = &last_match_remote_rx {
                    match rx.try_recv() {
                        Ok(matchmaking::PublicReplayUpdate::Loaded(replays)) => {
                            remote_replay_loaded = Some((row.clone(), replays));
                        }
                        Ok(matchmaking::PublicReplayUpdate::Error(e)) => remote_replay_error = Some(e),
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => remote_replay_disconnected = true,
                    }
                }
                if remote_replay_loaded.is_some() || remote_replay_error.is_some() || remote_replay_disconnected {
                    last_match_remote_rx = None;
                }
                if let Some((row, replays)) = remote_replay_loaded {
                    match match_replay::find_matching_remote_replay(&row, &replays) {
                        Some(meta) => match download_remote_replay(&meta.url) {
                            Ok(path) => {
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
                                            println!("[replay] Remote last-match replay load failed: {e}");
                                            toast = Some((
                                                format!("Replay unavailable: {e}"),
                                                Instant::now() + Duration::from_millis(3200),
                                            ));
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                toast = Some((
                                    format!("Remote replay download failed: {e}"),
                                    Instant::now() + Duration::from_millis(3200),
                                ));
                            }
                        },
                        None => {
                            toast = Some((
                                "No local or remote replay found for this match".into(),
                                Instant::now() + Duration::from_millis(2400),
                            ));
                        }
                    }
                }
                if let Some(e) = remote_replay_error {
                    toast = Some((
                        format!("Remote replay check failed: {e}"),
                        Instant::now() + Duration::from_millis(3200),
                    ));
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
                    match &mut state {
                        AppState::Menu(menu::MenuScreen::LiveMatches {
                            cursor,
                            matches,
                            status,
                        })
                        | AppState::Menu(menu::MenuScreen::OnlineHub {
                            cursor,
                            live_matches: matches,
                            status,
                            ..
                        })
                        | AppState::FpUi(fp_ui::FpScreen::Lobby {
                            cursor,
                            live_matches: matches,
                            status,
                            ..
                        }) => match rx.try_recv() {
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
                        },
                        _ => {
                            live_matches_rx = None;
                        }
                    }
                }

                // Drain the public replay index fetcher channel.
                if let Some(rx) = &public_replay_rx {
                    let entries_status = match &mut state {
                        AppState::Menu(menu::MenuScreen::ReplaySelect { entries, status, .. }) => {
                            Some((entries, status))
                        }
                        AppState::FpUi(fp_ui::FpScreen::ReplaySelect { entries, status, .. }) => {
                            Some((entries, status))
                        }
                        _ => None,
                    };
                    if let Some((entries, status)) = entries_status {
                        match rx.try_recv() {
                            Ok(matchmaking::PublicReplayUpdate::Loaded(replays)) => {
                                let loaded_count = replays.len();
                                let mut existing = std::collections::HashSet::new();
                                for entry in entries.iter() {
                                    if let Some(url) = &entry.remote_url {
                                        existing.insert(url.clone());
                                    }
                                }
                                for meta in replays {
                                    if existing.insert(meta.url.clone()) {
                                        entries.push(menu::ReplayEntry {
                                            filename: meta.filename,
                                            path: String::new(),
                                            remote_url: Some(meta.url),
                                            p1_name: meta.p1_name,
                                            p2_name: meta.p2_name,
                                            p1_score: meta.p1_score,
                                            p2_score: meta.p2_score,
                                            winner: meta.winner,
                                            frame_count: meta.frame_count,
                                            duration: meta.duration,
                                            recorded_at: meta.recorded_at,
                                            note: String::new(),
                                            bookmark_count: 0,
                                        });
                                    }
                                }
                                if loaded_count == 0 && entries.is_empty() {
                                    *status = Some("No local or public replays found".into());
                                } else if loaded_count == 0 {
                                    *status = None;
                                } else {
                                    *status = Some(format!(
                                        "Loaded {loaded_count} public replay{}",
                                        if loaded_count == 1 { "" } else { "s" }
                                    ));
                                }
                                public_replay_rx = None;
                            }
                            Ok(matchmaking::PublicReplayUpdate::Error(e)) => {
                                println!("[replay] public index fetch failed: {e}");
                                if entries.is_empty() {
                                    *status =
                                        Some(format!("Error: public replays unavailable: {e}"));
                                } else {
                                    *status = Some("Public replays unavailable".into());
                                }
                                public_replay_rx = None;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                public_replay_rx = None;
                            }
                        }
                    } else {
                        public_replay_rx = None;
                    }
                }

                // Drain the ghost-list fetcher channel.
                if let Some(rx) = &ghost_list_rx {
                    let entries_status = match &mut state {
                        AppState::Menu(menu::MenuScreen::GhostSelect { entries, download_status, .. }) => {
                            Some((entries, download_status))
                        }
                        AppState::FpUi(fp_ui::FpScreen::GhostSelect { entries, status, .. }) => {
                            Some((entries, status))
                        }
                        _ => None,
                    };
                    if let Some((entries, download_status)) = entries_status {
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
                                    *download_status = Some("No shared drones found".into());
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
                    let cursor_entries_status = match &mut state {
                        AppState::Menu(menu::MenuScreen::GhostSelect { cursor, entries, download_status, .. }) => {
                            Some((cursor, entries, download_status))
                        }
                        AppState::FpUi(fp_ui::FpScreen::GhostSelect { cursor, entries, status, .. }) => {
                            Some((cursor, entries, status))
                        }
                        _ => None,
                    };
                    if let Some((cursor, entries, download_status)) = cursor_entries_status {
                        match rx.try_recv() {
                            Ok(matchmaking::GhostDownloadUpdate::Saved { local_path, .. }) => {
                                ghost_download_rx = None;
                                *download_status = Some("Loading drone...".into());
                                ensure_core_loaded(&mut core, &mut audio_queue, &audio_subsystem)?;
                                if let Some(c) = &core {
                                    match ghost::Playback::load(&local_path) {
                                        Ok(pb) => {
                                            if pb.prime(c) {
                                                println!(
                                                    "[ghost] Loaded remote drone opponent: {} frames",
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
                                                Some(format!("Error: drone load failed: {e}"));
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
                                        Some("Shared drone is no longer available".into());
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
                if let (AppState::FpUi(screen), Some(fpf)) = (&state, fp_fonts.as_mut()) {
                    let fp_username = discord_user.as_deref().unwrap_or(&cfg.player_username);
                    fp_ui::draw(
                        screen,
                        &mut canvas,
                        fpf,
                        win_w as i32,
                        win_h as i32,
                        fp_username,
                        &main_leaderboard,
                        &main_profile,
                        &cfg.bindings,
                        &cfg.stats_email,
                        discord_user.is_some(),
                        rom_present.check(),
                    )
                    .map_err(|e| format!("fp_ui draw: {e}"))?;
                    // fp_ui screens have no toast param of their own (unlike
                    // legacy's `menu::draw`, which takes one directly) — a
                    // toast set while in an FpUi state (e.g. "No local
                    // replay found for this match" from
                    // WatchLastMatchReplay) was silently never drawn,
                    // reading as "nothing happened" to the user. Draw it
                    // as an overlay on top with the same legacy bitmap font
                    // legacy screens use for it.
                    if let Some(toast) = toast_payload(&toast) {
                        menu::draw_toast(&mut canvas, &mut font, &toast, win_w as i32, win_h as i32)
                            .map_err(|e| format!("fp_ui toast overlay: {e}"))?;
                    }
                } else if let Some(fpf) = fp_fonts.as_mut().filter(|_| fp_native_overlay(&state, cfg.new_ui)) {
                    // Non-FpScreen states rendered natively: TextEdit and
                    // Rebinding captures opened from fp_ui draw their parent
                    // screen dimmed under a modal; the Spectate connecting
                    // state is its own full native frame. State machines are
                    // unchanged — only the rendering swaps.
                    let fp_username = discord_user.as_deref().unwrap_or(&cfg.player_username);
                    match &state {
                        AppState::Menu(menu::MenuScreen::TextEdit {
                            title,
                            label,
                            value,
                            field,
                            came_from,
                        }) => {
                            if let AppState::FpUi(under) = &**came_from {
                                fp_ui::draw(
                                    under,
                                    &mut canvas,
                                    fpf,
                                    win_w as i32,
                                    win_h as i32,
                                    fp_username,
                                    &main_leaderboard,
                                    &main_profile,
                                    &cfg.bindings,
                                    &cfg.stats_email,
                                    discord_user.is_some(),
                                    rom_present.check(),
                                )
                                .map_err(|e| format!("fp_ui under draw: {e}"))?;
                            }
                            fp_ui::draw_text_entry_modal(
                                &mut canvas,
                                fpf,
                                win_w as i32,
                                win_h as i32,
                                title,
                                label,
                                value,
                                field,
                                fp_osk,
                            )
                            .map_err(|e| format!("fp_ui text entry: {e}"))?;
                        }
                        AppState::Rebinding {
                            action,
                            player,
                            came_from,
                        } => {
                            if let AppState::FpUi(under) = &**came_from {
                                fp_ui::draw(
                                    under,
                                    &mut canvas,
                                    fpf,
                                    win_w as i32,
                                    win_h as i32,
                                    fp_username,
                                    &main_leaderboard,
                                    &main_profile,
                                    &cfg.bindings,
                                    &cfg.stats_email,
                                    discord_user.is_some(),
                                    rom_present.check(),
                                )
                                .map_err(|e| format!("fp_ui under draw: {e}"))?;
                            }
                            fp_ui::draw_rebind_capture_modal(
                                &mut canvas,
                                fpf,
                                win_w as i32,
                                win_h as i32,
                                *action,
                                *player,
                            )
                            .map_err(|e| format!("fp_ui rebind capture: {e}"))?;
                        }
                        AppState::Menu(menu::MenuScreen::Spectate { status, .. }) => {
                            fp_ui::draw_spectate_connecting(
                                &mut canvas,
                                fpf,
                                win_w as i32,
                                win_h as i32,
                                status,
                                fp_username,
                            )
                            .map_err(|e| format!("fp_ui spectate connecting: {e}"))?;
                        }
                        _ => {}
                    }
                    if let Some(toast) = toast_payload(&toast) {
                        menu::draw_toast(&mut canvas, &mut font, &toast, win_w as i32, win_h as i32)
                            .map_err(|e| format!("fp_ui toast overlay: {e}"))?;
                    }
                } else {
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
                        menu_input_pad,
                    )
                    .map_err(|e| format!("menu draw: {e}"))?;
                }
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
            } else if is_matchmaking_screen(&state) {
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
            frame_timer::wait_until_frame_deadline(next_frame_deadline);
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
            let sampled_fps = fps_sample_frames as f32 / fps_elapsed.as_secs_f32();
            current_fps = Some(sampled_fps);
            fps_sample_frames = 0;
            fps_sample_started = Instant::now();

            if net_session.is_some()
                && sampled_fps < 53.5
                && Instant::now() >= next_slow_perf_log_at
            {
                next_slow_perf_log_at = Instant::now() + Duration::from_secs(5);
                let mk2_rows = core
                    .as_ref()
                    .and_then(mk2_perf::sample)
                    .map(|sample| sample.detail_rows().join(" | "))
                    .unwrap_or_else(|| "MK2 PERF unavailable".to_string());
                let line = format!(
                    "[perf/slow] fps={sampled_fps:.1} renderer={} filter={} ping={} rollback={} saves={} loads={} timing_us=S{} H{} L{} behind=L{} R{} {mk2_rows}",
                    renderer_name(&canvas),
                    cfg.video_filter.label(),
                    net_stats
                        .ping_ms
                        .map(|ms| format!("{ms}ms"))
                        .unwrap_or_else(|| "--".to_string()),
                    net_stats.rollback_frames,
                    net_stats.save_count,
                    net_stats.load_count,
                    net_stats.save_state_micros,
                    net_stats.checksum_micros,
                    net_stats.load_state_micros,
                    net_stats.local_frames_behind.as_deref().unwrap_or("-"),
                    net_stats.remote_frames_behind.as_deref().unwrap_or("-"),
                );
                println!("{line}");
                if let Some(f) = net_log.as_mut() {
                    use std::io::Write;
                    let _ = writeln!(f, "{line}");
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{shutdown_local_runtime_for_netplay, LocalPlayMode};

    #[test]
    fn shutdown_local_runtime_for_netplay_clears_local_state() {
        let mut trainer = crate::memory::PokeList::new();
        let mut reset_slots = crate::lab::ResetSlots::default();
        let mut dummy = crate::lab::DummyController::default();
        let mut punish = crate::lab::PunishTrainer::default();
        let mut damage = crate::lab::DamageTracker::default();
        let mut ghost_playback = None;
        let mut ghost_recording = None;
        let mut drone_runner = None;
        let mut ghost_port_mask = 0;
        let mut replay_playback = None;
        let mut replay_recording = None;
        let mut replay_paused = true;
        let mut replay_tick = 123;
        let mut replay_clip_in = Some(10);
        let mut replay_clip_out = Some(20);
        let mut clip_recorder = None;
        let mut input_history = crate::input_history::InputHistory::new();
        let mut score_tracker = crate::score::ScoreTracker::new();
        let mut local_mode = LocalPlayMode::Lab;
        let mut p1_wins = 2;
        let mut p2_wins = 1;
        let mut auto_start_done = false;
        let mut auto_start_frame = 77;
        let mut audio_tail = Some((1, -1));

        shutdown_local_runtime_for_netplay(
            None,
            None,
            &mut trainer,
            &mut reset_slots,
            &mut dummy,
            &mut punish,
            &mut damage,
            &mut ghost_playback,
            &mut ghost_recording,
            &mut drone_runner,
            &mut ghost_port_mask,
            &mut replay_playback,
            &mut replay_recording,
            &mut replay_paused,
            &mut replay_tick,
            &mut replay_clip_in,
            &mut replay_clip_out,
            &mut clip_recorder,
            &mut input_history,
            &mut score_tracker,
            &mut local_mode,
            &mut p1_wins,
            &mut p2_wins,
            &mut auto_start_done,
            &mut auto_start_frame,
            &mut audio_tail,
            None,
            "test",
        );

        assert_eq!(local_mode, LocalPlayMode::Arcade);
        assert_eq!(ghost_port_mask, 0b11);
        assert!(!replay_paused);
        assert_eq!(replay_tick, 0);
        assert!(replay_clip_in.is_none());
        assert!(replay_clip_out.is_none());
        assert_eq!(p1_wins, 0);
        assert_eq!(p2_wins, 0);
        assert!(auto_start_done);
        assert_eq!(auto_start_frame, 0);
        assert!(audio_tail.is_none());
    }
}
