//! In-engine menu: list-based screens, pad/keyboard navigation, rebind flow.
use crate::font::Font;
use crate::input::{is_action_active, Action, Binding, Bindings, Player, PlayerBindings};
use crate::matchmaking::{HistoryRow, LeaderboardRow, LiveMatch, ProfileData, RemoteGhostMeta};
use crate::version;
use sdl2::pixels::Color;
use sdl2::rect::Rect;
use sdl2::render::Canvas;
use sdl2::video::Window;

pub const LOGICAL_W: i32 = 400;
pub const LOGICAL_H: i32 = 254;

/// Default netplay port for host/join. Exposed so main.rs can keep in sync.
pub const DEFAULT_NETPLAY_PORT: u16 = 7000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppState {
    Menu(MenuScreen),
    Playing,
    Rebinding {
        action: Action,
        player: Player,
        came_from: MenuScreen,
    },
}

/// All menu screens. Direct-IP Host/Join screens were removed when Find Match
/// (matchmaking server + STUN/TURN) replaced manual IP entry. The CLI
/// `--player/--local/--peer` direct-IP launch is still supported; it bypasses
/// the menu entirely via `cli::NetMode::P2P`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuScreen {
    Main {
        cursor: usize,
    },
    Controls {
        cursor: usize,
        player: Player,
    },
    About,
    /// Connection tester — text editor for an IP:port; Enter triggers a UDP
    /// probe rather than starting ggrs. Once the probe has run, main.rs
    /// transitions to `TestResult`.
    TestIp {
        ip_text: String,
        editing: bool,
    },
    /// Result screen with verdict lines. Owned by the menu so we can render
    /// it between probe runs; main.rs writes into `lines` after each probe.
    TestResult {
        lines: Vec<String>,
    },
    /// Netplay session exit summary. Used for disconnects/timeouts so the user
    /// gets a calm end screen instead of a diagnostics-style failure panel.
    SessionEnded {
        lines: Vec<String>,
    },
    /// Automated matchmaking via the signaling server. `status` is a
    /// human-readable progress string updated by the background thread.
    Matchmaking {
        status: String,
    },
    /// Browse local + remote ghost recordings. Local entries populate
    /// synchronously from the `ghosts/` directory on entry; remote entries
    /// stream in from `freeplay-stats /ghosts/list` as the fetch completes.
    /// `download_status` shows transient feedback during a remote download.
    GhostSelect {
        cursor: usize,
        entries: Vec<GhostEntry>,
        download_status: Option<String>,
    },
    /// Player rating + recent matches fetched from freeplay-stats. Loads
    /// asynchronously — main.rs swaps the inner state as the fetch completes.
    Profile {
        state: ProfileScreenState,
    },
    /// Community rating leaderboard fetched from freeplay-stats.
    Leaderboard {
        state: LeaderboardState,
    },
    /// Small utility/settings panel for runtime toggles and diagnostics.
    Settings {
        cursor: usize,
        discord_rpc_enabled: bool,
        fullscreen: bool,
        volume_percent: u8,
    },
    /// Practice/training helpers backed by RAM pokes already used by F-keys.
    Training {
        cursor: usize,
        hitboxes: bool,
        infinite_health: bool,
        freeze_timer: bool,
    },
    /// Active online matches fetched from the signaling server.
    LiveMatches {
        cursor: usize,
        matches: Vec<LiveMatch>,
        status: String,
    },
    /// Live spectator view for a remote match. The signaling server currently
    /// exposes score/frame status, so this screen updates those values while
    /// full video playback is still future work.
    Spectate {
        session_id: String,
        status: SpectateStatus,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GhostEntry {
    /// On-disk `.ncgh` we recorded locally (or downloaded previously).
    /// `path` is what `ghost::Playback::load` opens.
    Local { filename: String, path: String },
    /// Catalogued on freeplay-stats but not yet downloaded. Pressing Enter
    /// kicks off the download to `ghosts/remote_<id>.ncgh`.
    Remote(RemoteGhostMeta),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileScreenState {
    /// User isn't logged in to Discord — fetcher won't have a discord_id.
    NotLoggedIn,
    /// Background fetch in flight.
    Loading,
    /// `/player/:id` returned 404 (no matches yet) or the network failed.
    Error(String),
    /// Profile + (possibly empty) recent-matches list.
    Loaded {
        profile: ProfileData,
        history: Vec<HistoryRow>,
        avatar_rgba: Option<(Vec<u8>, u32, u32)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaderboardState {
    Loading,
    Error(String),
    Loaded(Vec<LeaderboardRow>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpectateStatus {
    pub message: String,
    pub p1_name: String,
    pub p2_name: String,
    pub frame: Option<u32>,
    pub p1_score: u32,
    pub p2_score: u32,
    pub updated_at: Option<String>,
}

pub struct Toast<'a> {
    pub message: &'a str,
    pub remaining_ms: u128,
}

impl SpectateStatus {
    pub fn waiting() -> Self {
        Self {
            message: "Connecting to spectator relay...".into(),
            p1_name: "P1".into(),
            p2_name: "P2".into(),
            frame: None,
            p1_score: 0,
            p2_score: 0,
            updated_at: None,
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        AppState::Menu(MenuScreen::Main { cursor: 0 })
    }
}

/// Main menu items, in order.
pub const MAIN_ITEMS: [&str; 11] = [
    "Practice",
    "Find Match",
    "Watch Live",
    "Profile",
    "Leaderboard",
    "Load Ghosts",
    "Controls",
    "Training",
    "Settings",
    "About",
    "Quit",
];

const SETTINGS_ITEMS: [&str; 5] = [
    "Discord Rich Presence",
    "Fullscreen",
    "Volume",
    "Run Doctor",
    "Open Logs Folder",
];
const TRAINING_ITEMS: [&str; 3] = ["Hitbox View", "Infinite Health", "Freeze Timer"];

pub enum NavResult {
    Stay,
    StartGame,
    /// Launch automated matchmaking via the signaling server.
    StartMatchmaking,
    /// Open the Profile screen — main.rs kicks off a background fetch from
    /// freeplay-stats and updates the screen state as the response lands.
    OpenProfile,
    /// Open the active match browser.
    OpenLiveMatches,
    /// Open community leaderboard.
    OpenLeaderboard,
    /// Open Settings screen.
    OpenSettings,
    /// Entered GhostSelect — main.rs populates the local list and (if a stats
    /// URL is configured) spawns a background `/ghosts/list` fetch.
    OpenGhostSelect,
    /// Run a UDP probe against `peer`. Main.rs executes the probe (blocking
    /// ~3s) and stashes the output into a `TestResult` screen.
    RunProbe {
        peer: std::net::SocketAddr,
    },
    Quit,
    BeginRebind,
    ClearAllBindings(Player),
    /// Load a ghost file from the selected path.
    LoadGhost(String),
    /// Download a remote ghost by ghost_id, then load it.
    DownloadGhost(String),
    /// Sign out of Discord and clear the cached token.
    #[allow(dead_code)]
    SignOut,
    /// Open the spectator screen for a selected live session.
    WatchSession(String),
    /// Toggle Discord Rich Presence at runtime and persist config.
    ToggleDiscordRpc,
    /// Toggle desktop fullscreen and persist config.
    ToggleFullscreen,
    /// Adjust audio volume by signed percentage points.
    AdjustVolume(i8),
    /// Open Training helper menu.
    OpenTraining,
    /// Toggle named training helper.
    ToggleTraining(&'static str),
    /// Launch the external setup diagnostics window.
    LaunchDoctor,
    /// Open the folder where runtime logs are written.
    OpenLogsFolder,
}

impl AppState {
    pub fn nav_up(&mut self) {
        match self {
            AppState::Menu(MenuScreen::Main { cursor }) => {
                *cursor = cursor.saturating_sub(1);
            }
            AppState::Menu(MenuScreen::Controls { cursor, .. }) => {
                *cursor = cursor.saturating_sub(1);
            }
            AppState::Menu(MenuScreen::GhostSelect { cursor, .. }) => {
                *cursor = cursor.saturating_sub(1);
            }
            AppState::Menu(MenuScreen::LiveMatches { cursor, .. }) => {
                *cursor = cursor.saturating_sub(1);
            }
            AppState::Menu(MenuScreen::Settings { cursor, .. }) => {
                *cursor = cursor.saturating_sub(1);
            }
            AppState::Menu(MenuScreen::Training { cursor, .. }) => {
                *cursor = cursor.saturating_sub(1);
            }
            _ => {}
        }
    }

    pub fn nav_down(&mut self) {
        match self {
            AppState::Menu(MenuScreen::Main { cursor }) => {
                if *cursor + 1 < MAIN_ITEMS.len() {
                    *cursor += 1;
                }
            }
            AppState::Menu(MenuScreen::Controls { cursor, .. }) => {
                if *cursor + 1 < Action::ALL.len() + 1 {
                    *cursor += 1;
                }
            }
            AppState::Menu(MenuScreen::GhostSelect {
                cursor, entries, ..
            }) => {
                if *cursor + 1 < entries.len() {
                    *cursor += 1;
                }
            }
            AppState::Menu(MenuScreen::LiveMatches {
                cursor, matches, ..
            }) => {
                if *cursor + 1 < matches.len() {
                    *cursor += 1;
                }
            }
            AppState::Menu(MenuScreen::Settings { cursor, .. }) => {
                if *cursor + 1 < SETTINGS_ITEMS.len() {
                    *cursor += 1;
                }
            }
            AppState::Menu(MenuScreen::Training { cursor, .. }) => {
                if *cursor + 1 < TRAINING_ITEMS.len() {
                    *cursor += 1;
                }
            }
            _ => {}
        }
    }

    pub fn nav_switch_player(&mut self) {
        if let AppState::Menu(MenuScreen::Controls { player, .. }) = self {
            *player = player.other();
        }
    }

    pub fn nav_accept(&mut self, rom_present: bool) -> NavResult {
        match self.clone() {
            AppState::Menu(MenuScreen::Main { cursor }) => match cursor {
                0 => {
                    // Practice
                    if !rom_present {
                        return NavResult::Stay;
                    }
                    *self = AppState::Playing;
                    NavResult::StartGame
                }
                1 => {
                    // Find Match
                    if !rom_present {
                        return NavResult::Stay;
                    }
                    *self = AppState::Menu(MenuScreen::Matchmaking {
                        status: "Starting...".to_string(),
                    });
                    NavResult::StartMatchmaking
                }
                2 => {
                    // Watch Live
                    *self = AppState::Menu(MenuScreen::LiveMatches {
                        cursor: 0,
                        matches: vec![],
                        status: "Loading active matches...".into(),
                    });
                    NavResult::OpenLiveMatches
                }
                3 => {
                    // Profile
                    *self = AppState::Menu(MenuScreen::Profile {
                        state: ProfileScreenState::Loading,
                    });
                    NavResult::OpenProfile
                }
                4 => {
                    // Leaderboard
                    *self = AppState::Menu(MenuScreen::Leaderboard {
                        state: LeaderboardState::Loading,
                    });
                    NavResult::OpenLeaderboard
                }
                5 => {
                    // Load Ghosts
                    *self = AppState::Menu(MenuScreen::GhostSelect {
                        cursor: 0,
                        entries: vec![],
                        download_status: None,
                    });
                    NavResult::OpenGhostSelect
                }
                6 => {
                    // Controls
                    *self = AppState::Menu(MenuScreen::Controls {
                        cursor: 0,
                        player: Player::P1,
                    });
                    NavResult::Stay
                }
                7 => {
                    // Training
                    *self = AppState::Menu(MenuScreen::Training {
                        cursor: 0,
                        hitboxes: false,
                        infinite_health: false,
                        freeze_timer: false,
                    });
                    NavResult::OpenTraining
                }
                8 => {
                    // Settings
                    *self = AppState::Menu(MenuScreen::Settings {
                        cursor: 0,
                        discord_rpc_enabled: false,
                        fullscreen: false,
                        volume_percent: 100,
                    });
                    NavResult::OpenSettings
                }
                9 => {
                    // About
                    *self = AppState::Menu(MenuScreen::About);
                    NavResult::Stay
                }
                10 => NavResult::Quit,
                _ => NavResult::Stay,
            },
            AppState::Menu(MenuScreen::Controls { cursor, player }) => {
                if cursor < Action::ALL.len() {
                    let action = Action::ALL[cursor];
                    *self = AppState::Rebinding {
                        action,
                        player,
                        came_from: MenuScreen::Controls { cursor, player },
                    };
                    NavResult::BeginRebind
                } else if cursor == Action::ALL.len() {
                    NavResult::ClearAllBindings(player)
                } else {
                    NavResult::Stay
                }
            }
            AppState::Menu(MenuScreen::About) => NavResult::Stay,
            AppState::Menu(MenuScreen::GhostSelect {
                cursor, entries, ..
            }) => {
                if cursor < entries.len() {
                    match &entries[cursor] {
                        GhostEntry::Local { path, .. } => NavResult::LoadGhost(path.clone()),
                        GhostEntry::Remote(meta) => NavResult::DownloadGhost(meta.ghost_id.clone()),
                    }
                } else {
                    NavResult::Stay
                }
            }
            AppState::Menu(MenuScreen::LiveMatches {
                cursor, matches, ..
            }) => {
                if let Some(m) = matches.get(cursor) {
                    *self = AppState::Menu(MenuScreen::Spectate {
                        session_id: m.session_id.clone(),
                        status: SpectateStatus {
                            message: "Opening spectator relay...".into(),
                            p1_name: m.p1_name.clone(),
                            p2_name: m.p2_name.clone(),
                            frame: None,
                            p1_score: m.p1_score,
                            p2_score: m.p2_score,
                            updated_at: None,
                        },
                    });
                    NavResult::WatchSession(m.session_id.clone())
                } else {
                    NavResult::OpenLiveMatches
                }
            }
            AppState::Menu(MenuScreen::TestIp { ip_text, editing }) => {
                if editing {
                    match parse_ip_port(&ip_text) {
                        Some(peer) => NavResult::RunProbe { peer },
                        None => NavResult::Stay,
                    }
                } else {
                    NavResult::Stay
                }
            }
            AppState::Menu(MenuScreen::TestResult { .. }) => {
                // Pressing Enter on the results screen runs another probe.
                *self = AppState::Menu(MenuScreen::TestIp {
                    ip_text: String::new(),
                    editing: true,
                });
                NavResult::Stay
            }
            AppState::Menu(MenuScreen::Settings { cursor, .. }) => match cursor {
                0 => NavResult::ToggleDiscordRpc,
                1 => NavResult::ToggleFullscreen,
                2 => NavResult::AdjustVolume(10),
                3 => NavResult::LaunchDoctor,
                4 => NavResult::OpenLogsFolder,
                _ => NavResult::Stay,
            },
            AppState::Menu(MenuScreen::Training { cursor, .. }) => match cursor {
                0 => NavResult::ToggleTraining("hitboxes"),
                1 => NavResult::ToggleTraining("health"),
                2 => NavResult::ToggleTraining("timer"),
                _ => NavResult::Stay,
            },
            AppState::Menu(MenuScreen::SessionEnded { .. }) => {
                *self = AppState::Menu(MenuScreen::Main { cursor: 0 });
                NavResult::Stay
            }
            _ => NavResult::Stay,
        }
    }

    pub fn nav_back(&mut self) {
        match self {
            AppState::Menu(MenuScreen::Controls { .. })
            | AppState::Menu(MenuScreen::About)
            | AppState::Menu(MenuScreen::TestIp { .. })
            | AppState::Menu(MenuScreen::TestResult { .. })
            | AppState::Menu(MenuScreen::SessionEnded { .. })
            | AppState::Menu(MenuScreen::Matchmaking { .. })
            | AppState::Menu(MenuScreen::GhostSelect { .. })
            | AppState::Menu(MenuScreen::Profile { .. })
            | AppState::Menu(MenuScreen::Leaderboard { .. })
            | AppState::Menu(MenuScreen::Settings { .. })
            | AppState::Menu(MenuScreen::Training { .. })
            | AppState::Menu(MenuScreen::LiveMatches { .. })
            | AppState::Menu(MenuScreen::Spectate { .. }) => {
                *self = AppState::Menu(MenuScreen::Main { cursor: 0 });
            }
            _ => {}
        }
    }

    pub fn finish_rebind(&mut self) {
        if let AppState::Rebinding { came_from, .. } = self.clone() {
            *self = AppState::Menu(came_from);
        }
    }

    /// Append a character to the TestIp IP editor buffer. No-op if not editing.
    pub fn text_input(&mut self, s: &str) {
        let buf: Option<&mut String> = match self {
            AppState::Menu(MenuScreen::TestIp {
                ip_text,
                editing: true,
            }) => Some(ip_text),
            _ => None,
        };
        if let Some(ip_text) = buf {
            for c in s.chars() {
                if c.is_ascii_digit() || c == '.' || c == ':' {
                    if ip_text.len() < 24 {
                        ip_text.push(c);
                    }
                }
            }
        }
    }

    pub fn text_backspace(&mut self) {
        if let AppState::Menu(MenuScreen::TestIp {
            ip_text,
            editing: true,
        }) = self
        {
            ip_text.pop();
        }
    }
}

/// Parse "1.2.3.4:7000" or "1.2.3.4" (default port added) into a SocketAddr.
fn parse_ip_port(s: &str) -> Option<std::net::SocketAddr> {
    let trimmed = s.trim();
    if let Ok(a) = trimmed.parse::<std::net::SocketAddr>() {
        return Some(a);
    }
    // No port — append default.
    if let Ok(ip) = trimmed.parse::<std::net::IpAddr>() {
        return Some(std::net::SocketAddr::new(ip, DEFAULT_NETPLAY_PORT));
    }
    None
}

// --- Rendering ---

pub fn draw(
    state: &AppState,
    bindings: &Bindings,
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    w: i32,
    h: i32,
    rom_present: bool,
    discord_user: Option<&str>,
    toast: Option<Toast<'_>>,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGB(8, 8, 16));
    canvas.clear();

    match state {
        AppState::Menu(MenuScreen::Main { cursor }) => {
            draw_main(canvas, font, *cursor, w, h, rom_present, discord_user)?
        }
        AppState::Menu(MenuScreen::Controls { cursor, player }) => draw_controls(
            canvas,
            font,
            bindings.get(*player),
            *player,
            *cursor,
            None,
            w,
            h,
        )?,
        AppState::Menu(MenuScreen::About) => draw_about(canvas, font, w, h)?,
        AppState::Menu(MenuScreen::TestIp { ip_text, editing }) => {
            draw_test_ip(canvas, font, ip_text, *editing, w, h)?
        }
        AppState::Menu(MenuScreen::TestResult { lines }) => {
            draw_test_result(canvas, font, lines, w, h)?
        }
        AppState::Menu(MenuScreen::SessionEnded { lines }) => {
            draw_session_ended(canvas, font, lines, w, h)?
        }
        AppState::Menu(MenuScreen::Matchmaking { status }) => {
            draw_matchmaking(canvas, font, status, w, h)?
        }
        AppState::Menu(MenuScreen::GhostSelect {
            cursor, entries, ..
        }) => draw_ghost_select(canvas, font, *cursor, entries, w, h)?,
        AppState::Menu(MenuScreen::Profile { state }) => draw_profile(canvas, font, state, w, h)?,
        AppState::Menu(MenuScreen::Leaderboard { state }) => {
            draw_leaderboard(canvas, font, state, w, h)?
        }
        AppState::Menu(MenuScreen::Settings {
            cursor,
            discord_rpc_enabled,
            fullscreen,
            volume_percent,
        }) => draw_settings(
            canvas,
            font,
            *cursor,
            *discord_rpc_enabled,
            *fullscreen,
            *volume_percent,
            w,
            h,
        )?,
        AppState::Menu(MenuScreen::Training {
            cursor,
            hitboxes,
            infinite_health,
            freeze_timer,
        }) => draw_training(
            canvas,
            font,
            *cursor,
            *hitboxes,
            *infinite_health,
            *freeze_timer,
            w,
            h,
        )?,
        AppState::Menu(MenuScreen::LiveMatches {
            cursor,
            matches,
            status,
        }) => draw_live_matches(canvas, font, *cursor, matches, status, w, h)?,
        AppState::Menu(MenuScreen::Spectate { session_id, status }) => {
            draw_spectate(canvas, font, session_id, status, w, h)?
        }
        AppState::Rebinding {
            action,
            player,
            came_from,
        } => {
            let cursor = match came_from {
                MenuScreen::Controls { cursor, .. } => *cursor,
                _ => 0,
            };
            draw_controls(
                canvas,
                font,
                bindings.get(*player),
                *player,
                cursor,
                Some(*action),
                w,
                h,
            )?;
        }
        AppState::Playing => {}
    }

    draw_version_footer(canvas, font, w, h)?;
    if let Some(toast) = toast {
        draw_toast(canvas, font, &toast, w, h)?;
    }
    Ok(())
}

fn draw_toast(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    toast: &Toast<'_>,
    w: i32,
    h: i32,
) -> Result<(), String> {
    let small = small_scale(h);
    let alpha = toast.remaining_ms.min(1800) as u8 / 10 + 70;
    let text_w = font.text_width_exact(toast.message, small);
    let pad_x = 14;
    let box_w = text_w + pad_x * 2;
    let box_h = 28;
    let x = (w - box_w) / 2;
    let y = h - 70;

    canvas.set_draw_color(Color::RGBA(20, 24, 36, alpha));
    canvas.fill_rect(Rect::new(x, y, box_w as u32, box_h as u32))?;
    canvas.set_draw_color(Color::RGBA(90, 130, 210, alpha));
    canvas.draw_rect(Rect::new(x, y, box_w as u32, box_h as u32))?;
    font.draw(
        canvas,
        toast.message,
        x + pad_x,
        y + 7,
        small,
        Color::RGB(225, 235, 255),
    )?;
    Ok(())
}

fn draw_session_ended(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    lines: &[String],
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "MATCH ENDED", w, h)?;
    let scale = body_scale(h).saturating_sub(1).max(1);
    let small = small_scale(h);
    let x = w / 8;
    let mut y = 34 + 28 * title_scale(h) as i32;

    for line in lines {
        let colour = if line.starts_with("OK ") {
            Color::RGB(120, 230, 120)
        } else if line.starts_with("WARN ") {
            Color::RGB(240, 200, 100)
        } else {
            Color::RGB(205, 210, 225)
        };
        font.draw(canvas, line, x, y, scale, colour)?;
        y += 22 * scale as i32 + 4;
    }

    let footer = "ENTER Main Menu   ESC Main Menu";
    let fw = font.text_width_exact(footer, small);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        small,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn draw_spectate(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    session_id: &str,
    status: &SpectateStatus,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "WATCH MATCH", w, h)?;
    let scale = body_scale(h);
    let small = small_scale(h);
    let cx = w / 2;
    let mut y = 42 + 26 * title_scale(h) as i32;

    let score = format!(
        "{} {}  -  {} {}",
        status.p1_name, status.p1_score, status.p2_score, status.p2_name
    );
    let score_w = font.text_width_exact(&score, scale);
    font.draw(
        canvas,
        &score,
        cx - score_w / 2,
        y,
        scale,
        Color::RGB(255, 210, 90),
    )?;
    y += 42 * scale as i32;

    let frame = status
        .frame
        .map(|f| format!("Frame {f}"))
        .unwrap_or_else(|| "Waiting for first frame".into());
    let frame_w = font.text_width_exact(&frame, small);
    font.draw(
        canvas,
        &frame,
        cx - frame_w / 2,
        y,
        small,
        Color::RGB(205, 215, 235),
    )?;
    y += 24 * small as i32;

    let msg_w = font.text_width_exact(&status.message, small);
    font.draw(
        canvas,
        &status.message,
        cx - msg_w / 2,
        y,
        small,
        Color::RGB(155, 175, 210),
    )?;
    y += 24 * small as i32;

    if let Some(updated_at) = &status.updated_at {
        let updated = format!("Updated {updated_at}");
        let updated_w = font.text_width_exact(&updated, small);
        font.draw(
            canvas,
            &updated,
            cx - updated_w / 2,
            y,
            small,
            Color::RGB(120, 130, 155),
        )?;
    }

    let short_id = if session_id.len() > 18 {
        format!("{}...", &session_id[..18])
    } else {
        session_id.to_string()
    };
    let id_line = format!("Session {short_id}");
    let id_w = font.text_width_exact(&id_line, small);
    font.draw(
        canvas,
        &id_line,
        cx - id_w / 2,
        h - 54,
        small,
        Color::RGB(95, 105, 130),
    )?;

    let footer = "C Copy Link   ESC Back";
    let fw = font.text_width_exact(footer, small);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        small,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn draw_live_matches(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    cursor: usize,
    matches: &[LiveMatch],
    status: &str,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "WATCH LIVE", w, h)?;
    let scale = body_scale(h).saturating_sub(1).max(1);
    let small = small_scale(h);
    let x = (w / 10).max(34);
    let y = 34 + 26 * title_scale(h) as i32;

    if matches.is_empty() {
        let tw = font.text_width_exact(status, scale);
        font.draw(
            canvas,
            status,
            (w - tw) / 2,
            y + 30,
            scale,
            Color::RGB(180, 190, 210),
        )?;
    } else {
        let max_rows = ((h - y - 54) / 34).max(1) as usize;
        for (i, m) in matches.iter().take(max_rows).enumerate() {
            let selected = i == cursor;
            let row_y = y + i as i32 * 34;
            if selected {
                canvas.set_draw_color(Color::RGBA(32, 38, 62, 220));
                canvas.fill_rect(Rect::new(x - 12, row_y - 5, (w - 2 * x + 24) as u32, 28))?;
                font.draw(canvas, ">", x - 28, row_y, scale, Color::RGB(255, 235, 180))?;
            }

            let names = format!("{} vs {}", m.p1_name, m.p2_name);
            let score = format!("{}-{}", m.p1_score, m.p2_score);
            let score_w = font.text_width_exact(&score, scale);
            let names = fit_line(font, &names, scale, w - 2 * x - score_w - 28);
            font.draw(
                canvas,
                &names,
                x,
                row_y,
                scale,
                if selected {
                    Color::RGB(245, 245, 250)
                } else {
                    Color::RGB(175, 180, 198)
                },
            )?;
            font.draw(
                canvas,
                &score,
                w - x - score_w,
                row_y,
                scale,
                Color::RGB(255, 210, 90),
            )?;
        }
    }

    let footer = "ENTER Watch   R Refresh   ESC Back";
    let fw = font.text_width_exact(footer, small);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        small,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn fit_line(font: &mut Font, text: &str, scale: u32, max_w: i32) -> String {
    if font.text_width_exact(text, scale) <= max_w {
        return text.to_string();
    }
    let mut out = String::new();
    for ch in text.chars() {
        let candidate = format!("{out}{ch}...");
        if font.text_width_exact(&candidate, scale) > max_w {
            break;
        }
        out.push(ch);
    }
    format!("{out}...")
}

fn title_scale(h: i32) -> u32 {
    ((h / 180).max(3) as u32).min(6)
}
fn body_scale(h: i32) -> u32 {
    ((h / 320).max(2) as u32).min(3)
}
fn small_scale(h: i32) -> u32 {
    ((h / 540).max(1) as u32).min(2)
}

fn draw_version_footer(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    w: i32,
    h: i32,
) -> Result<(), String> {
    let s = version::footer_string();
    let scale = small_scale(h);
    let tw = font.text_width_exact(&s, scale);
    // Bottom-right. Use a generous bottom margin so TTF glyphs (taller than 8px) don't clip.
    font.draw(
        canvas,
        &s,
        w - tw - 8,
        h - 30,
        scale,
        Color::RGB(110, 110, 130),
    )?;
    Ok(())
}

fn draw_title(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    title: &str,
    w: i32,
    h: i32,
) -> Result<(), String> {
    let scale = title_scale(h);
    let tw = font.text_width_exact(title, scale);
    let x = (w - tw) / 2;
    let y = 24;
    font.draw(canvas, title, x, y, scale, Color::RGB(255, 200, 0))?;
    let line_y = y + (24 * scale as i32);
    canvas.set_draw_color(Color::RGB(100, 50, 0));
    canvas.fill_rect(Rect::new(40, line_y, (w - 80) as u32, 2))?;
    Ok(())
}

fn draw_main(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    cursor: usize,
    w: i32,
    h: i32,
    rom_present: bool,
    discord_user: Option<&str>,
) -> Result<(), String> {
    draw_title(canvas, font, "Freeplay", w, h)?;

    let item_scale = body_scale(h).max(2);
    let line_h = (44 * item_scale as i32) / 2;
    let block_h = MAIN_ITEMS.len() as i32 * line_h;
    let start_y = (h - block_h) / 2 + 10;

    let widest = MAIN_ITEMS
        .iter()
        .map(|label| font.text_width_exact(label, item_scale))
        .max()
        .unwrap_or(0);
    let menu_x = (w - widest) / 2;
    for (i, label) in MAIN_ITEMS.iter().enumerate() {
        let x = menu_x;
        let y = start_y + i as i32 * line_h;
        let disabled = i == 0 && !rom_present;
        let base = if disabled {
            Color::RGB(60, 60, 60)
        } else if i == cursor {
            Color::RGB(255, 255, 255)
        } else {
            Color::RGB(120, 120, 120)
        };
        if i == cursor && !disabled {
            let caret_w = font.text_width_exact("> ", item_scale);
            font.draw(canvas, ">", x - caret_w, y, item_scale, base)?;
        }
        font.draw(canvas, label, x, y, item_scale, base)?;
    }

    if !rom_present {
        let msg = "ROM zip not found next to the executable";
        let s = small_scale(h);
        let tw = font.text_width_exact(msg, s);
        font.draw(
            canvas,
            msg,
            (w - tw) / 2,
            start_y - 24,
            s,
            Color::RGB(200, 80, 80),
        )?;
    }

    let footer = "UP/DN or DPAD Select   ENTER Confirm";
    let fs = small_scale(h);
    let fw = font.text_width_exact(footer, fs);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        fs,
        Color::RGB(100, 100, 100),
    )?;

    if let Some(name) = discord_user {
        let label = format!("Logged in as {name}");
        let ls = small_scale(h);
        let lw = font.text_width_exact(&label, ls);
        font.draw(
            canvas,
            &label,
            w - lw - 8,
            h - 54,
            ls,
            Color::RGB(88, 130, 200),
        )?;
    }

    Ok(())
}

fn draw_about(canvas: &mut Canvas<Window>, font: &mut Font, w: i32, h: i32) -> Result<(), String> {
    draw_title(canvas, font, "ABOUT", w, h)?;
    let body = body_scale(h);
    let small = small_scale(h);
    let content_scale = small;
    let line_h = 18 * content_scale as i32;
    let title_rule_y = 24 + (24 * title_scale(h) as i32);
    let content_x = (w / 12).max(42);
    let content_w = w - content_x * 2;
    let mut y = title_rule_y + 22;

    draw_panel(
        canvas,
        content_x,
        y,
        content_w,
        52,
        Color::RGBA(15, 16, 24, 230),
    )?;
    let line = format!(
        "Freeplay  v{}  build {}",
        version::VERSION,
        version::BUILD_DATE
    );
    font.draw(
        canvas,
        &line,
        content_x + 18,
        y + 13,
        body,
        Color::RGB(245, 245, 248),
    )?;
    y += 70;

    let gap = 14;
    let col_w = (content_w - gap) / 2;
    let left_x = content_x;
    let right_x = content_x + col_w + gap;
    let header = Color::RGB(220, 200, 120);
    let body_c = Color::RGB(190, 190, 200);

    draw_panel(canvas, left_x, y, col_w, 224, Color::RGBA(15, 16, 24, 220))?;
    draw_panel(canvas, right_x, y, col_w, 224, Color::RGBA(15, 16, 24, 220))?;
    font.draw(
        canvas,
        "PRACTICE HOTKEYS",
        left_x + 14,
        y + 12,
        body,
        header,
    )?;
    font.draw(canvas, "NETPLAY", right_x + 14, y + 12, body, header)?;
    y += 44;

    let left = [
        "F2   Hitbox overlay",
        "F3   Infinite health",
        "F4   Freeze timer",
        "F5   Save state",
        "F7   Load state",
        "F6   Ghost record",
        "F8   Ghost playback",
        "F12  Play vs ghost",
        "F11  Dump RAM",
    ];
    let right = [
        "Auto-skip attract on both sides",
        "3 matches per session",
        "Auto-record 3 sessions/peer",
        "Disconnect returns to lobby",
        "Esc ends match gracefully",
        "",
        "Logs: freeplay-net.log",
        "Ghosts: ghosts/*.ncgh",
        "",
    ];
    for (l, r) in left.iter().zip(right.iter()) {
        font.draw(canvas, l, left_x + 18, y, content_scale, body_c)?;
        if !r.is_empty() {
            font.draw(canvas, r, right_x + 18, y, content_scale, body_c)?;
        }
        y += line_h;
    }

    let gh = "github.com/junkwax/freeplay-gametalk";
    let ghw = font.text_width_exact(gh, content_scale);
    let btn_x = (w - ghw) / 2 - 14;
    let btn_y = y + 100;
    let btn_w = ghw + 28;
    let btn_h = (24 * content_scale) as i32;
    canvas.set_draw_color(Color::RGBA(25, 30, 55, 220));
    canvas.fill_rect(Rect::new(btn_x, btn_y, btn_w as u32, btn_h as u32))?;
    canvas.set_draw_color(Color::RGB(80, 140, 220));
    canvas.draw_rect(Rect::new(btn_x, btn_y, btn_w as u32, btn_h as u32))?;
    font.draw(
        canvas,
        gh,
        btn_x + 14,
        btn_y + 4,
        content_scale,
        Color::RGB(160, 200, 255),
    )?;

    let hint = "Press ENTER to open in browser";
    let hw = font.text_width_exact(hint, small);
    font.draw(
        canvas,
        hint,
        (w - hw) / 2,
        btn_y + btn_h + 6,
        small,
        Color::RGB(90, 90, 120),
    )?;

    let libs = about_status_line();
    let tw = font.text_width_exact(libs, small);
    font.draw(
        canvas,
        libs,
        (w - tw) / 2,
        h - 56,
        small,
        Color::RGB(140, 140, 160),
    )?;

    let footer = "R Refresh   ESC Back";
    let fs = small;
    let fw = font.text_width_exact(footer, fs);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        fs,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn about_status_line() -> &'static str {
    if crate::rom::find_rom_zip().is_some()
        && platform_core_exists()
        && crate::config::signaling_url()
            .or_else(|| crate::config::env_value("FREEPLAY_SIGNALING_URL"))
            .is_some()
        && crate::config::env_value("FREEPLAY_DISCORD_CLIENT_ID").is_some()
    {
        "Ready: ROM, core, matchmaking, and Discord are configured"
    } else {
        "Run freeplay --doctor for setup diagnostics"
    }
}

fn platform_core_exists() -> bool {
    let core = platform_core_name();
    std::path::Path::new(core).exists() || std::path::Path::new("cores").join(core).exists()
}

fn platform_core_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "fbneo_libretro.dll"
    }
    #[cfg(target_os = "linux")]
    {
        "fbneo_libretro.so"
    }
    #[cfg(target_os = "macos")]
    {
        "fbneo_libretro.dylib"
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        "fbneo_libretro"
    }
}

fn draw_controls(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    pb: &PlayerBindings,
    player: Player,
    cursor: usize,
    rebinding: Option<Action>,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "CONTROLS", w, h)?;

    let tab_scale = small_scale(h).max(1);
    let tab_h = 24 * tab_scale as i32;
    let tab_y = 24 + 24 * title_scale(h) as i32 + 10;
    let tab_w: i32 = font.text_width_exact("  P1  ", tab_scale) + 12;
    let tabs_total = tab_w * 2;
    let tabs_x0 = (w - tabs_total) / 2;
    for (i, p) in [Player::P1, Player::P2].iter().enumerate() {
        let tx = tabs_x0 + i as i32 * tab_w;
        let is_sel = *p == player;
        let bg = if is_sel {
            Color::RGB(190, 145, 35)
        } else {
            Color::RGB(24, 25, 34)
        };
        canvas.set_draw_color(bg);
        canvas.fill_rect(Rect::new(tx, tab_y, tab_w as u32, tab_h as u32))?;
        let fg = if is_sel {
            Color::RGB(0, 0, 0)
        } else {
            Color::RGB(180, 180, 180)
        };
        let lw = font.text_width_exact(p.label(), tab_scale);
        font.draw(
            canvas,
            p.label(),
            tx + (tab_w - lw) / 2,
            tab_y + 2,
            tab_scale,
            fg,
        )?;
    }

    let list_y = tab_y + tab_h + 14;
    let footer_reserve = 40;
    let avail = (h - list_y - footer_reserve).max(0);
    let total_items = Action::ALL.len() + 1;
    let row_h = (avail / total_items as i32).clamp(22, 30);
    let scale = small_scale(h).max(1);

    let content_x = (w / 12).max(42);
    let content_w = w - content_x * 2;
    let name_x = content_x + 18;
    let bind_x = content_x + content_w / 2;
    let live_x = content_x + content_w - 24;

    for (i, action) in Action::ALL.iter().enumerate() {
        let y = list_y + i as i32 * row_h;
        let selected = i == cursor;
        let is_rebinding_row = rebinding == Some(*action);

        canvas.set_draw_color(if selected {
            Color::RGBA(42, 38, 24, 235)
        } else {
            Color::RGBA(15, 16, 24, 205)
        });
        canvas.fill_rect(Rect::new(
            content_x,
            y - 3,
            content_w as u32,
            (row_h - 3) as u32,
        ))?;
        canvas.set_draw_color(if selected {
            Color::RGB(255, 210, 80)
        } else {
            Color::RGBA(70, 72, 88, 220)
        });
        canvas.fill_rect(Rect::new(content_x, y - 3, 3, (row_h - 3) as u32))?;

        let label_color = if selected {
            Color::RGB(255, 230, 120)
        } else {
            Color::RGB(220, 222, 232)
        };
        if selected {
            let cw = font.text_width_exact("> ", scale);
            font.draw(canvas, ">", name_x - cw, y, scale, label_color)?;
        }
        font.draw(canvas, action.label(), name_x, y, scale, label_color)?;

        let summary = if is_rebinding_row {
            "PRESS ANY INPUT...".to_string()
        } else {
            summarize_bindings(pb, *action)
        };
        let sum_color = if is_rebinding_row {
            Color::RGB(255, 100, 100)
        } else {
            Color::RGB(150, 200, 255)
        };
        font.draw(canvas, &summary, bind_x, y, scale, sum_color)?;

        let active = is_action_active(player, *action);
        canvas.set_draw_color(if active {
            Color::RGB(90, 220, 130)
        } else {
            Color::RGB(42, 44, 50)
        });
        let dot_h = 10 * scale as i32;
        canvas.fill_rect(Rect::new(
            live_x,
            y + (row_h - dot_h) / 2 - 1,
            dot_h as u32,
            dot_h as u32,
        ))?;
    }

    let clear_idx = Action::ALL.len();
    let y = list_y + clear_idx as i32 * row_h;
    let selected = cursor == clear_idx;
    canvas.set_draw_color(if selected {
        Color::RGBA(42, 38, 24, 235)
    } else {
        Color::RGBA(15, 16, 24, 205)
    });
    canvas.fill_rect(Rect::new(
        content_x,
        y - 3,
        content_w as u32,
        (row_h - 3) as u32,
    ))?;
    canvas.set_draw_color(if selected {
        Color::RGB(255, 210, 80)
    } else {
        Color::RGBA(70, 72, 88, 220)
    });
    canvas.fill_rect(Rect::new(content_x, y - 3, 3, (row_h - 3) as u32))?;
    let label_color = if selected {
        Color::RGB(255, 230, 120)
    } else {
        Color::RGB(220, 222, 232)
    };
    if selected {
        let cw = font.text_width_exact("> ", scale);
        font.draw(canvas, ">", name_x - cw, y, scale, label_color)?;
    }
    font.draw(canvas, "CLEAR ALL", name_x, y, scale, label_color)?;

    let footer = if rebinding.is_some() {
        "Press Input  DEL Clear  ESC Cancel"
    } else {
        "ENTER Rebind  TAB P1/P2  R Reset  ESC Back"
    };
    let fs = small_scale(h);
    let fw = font.text_width_exact(footer, fs);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        fs,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn summarize_bindings(pb: &PlayerBindings, action: Action) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (a, b) in &pb.entries {
        if *a != action {
            continue;
        }
        let s = match b {
            Binding::Key { key } => format!("KB:{key}"),
            Binding::PadButton { button } => format!("PAD:{}", pretty_binding_name(button)),
            Binding::PadAxis { axis, positive } => {
                format!(
                    "AX:{}{}",
                    pretty_binding_name(axis),
                    if *positive { "+" } else { "-" }
                )
            }
        };
        parts.push(s);
    }
    if parts.is_empty() {
        "(unbound)".to_string()
    } else {
        parts.join(", ")
    }
}

fn pretty_binding_name(s: &str) -> String {
    s.replace("DPAD", "D-PAD ")
        .replace("TRIGGER", "TRIGGER ")
        .replace("SHOULDER", "SHOULDER ")
}

/// IP-entry screen for Test Connection. Same editor shape as Join, but
/// Enter runs a UDP probe instead of starting ggrs.
fn draw_test_ip(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    ip_text: &str,
    editing: bool,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "TEST CONNECTION", w, h)?;
    let scale = body_scale(h);
    let small = small_scale(h);
    let x = w / 8;
    let mut y = 30 + 24 * title_scale(h) as i32;

    let instr = "Enter the host's IP and port  (e.g. 192.168.1.50:7000)";
    font.draw(canvas, instr, x, y, small, Color::RGB(160, 160, 160))?;
    y += 28;

    // Input box (same styling as Join for muscle-memory consistency).
    let box_h = 28 * scale as i32;
    let box_w = w - 2 * x;
    let box_color = if editing {
        Color::RGB(40, 60, 80)
    } else {
        Color::RGB(40, 40, 40)
    };
    canvas.set_draw_color(box_color);
    canvas.fill_rect(Rect::new(x, y, box_w as u32, box_h as u32))?;
    let border = if editing {
        Color::RGB(110, 180, 255)
    } else {
        Color::RGB(80, 80, 80)
    };
    canvas.set_draw_color(border);
    canvas.draw_rect(Rect::new(x, y, box_w as u32, box_h as u32))?;
    let txt = if ip_text.is_empty() && !editing {
        "(empty)"
    } else {
        ip_text
    };
    font.draw(canvas, txt, x + 8, y + 6, scale, Color::RGB(230, 230, 230))?;
    if editing {
        let blink_on = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_millis()
            / 500)
            % 2
            == 0;
        if blink_on {
            let cx = x + 8 + font.text_width_exact(ip_text, scale);
            canvas.set_draw_color(Color::RGB(230, 230, 230));
            canvas.fill_rect(Rect::new(cx, y + 6, 2, 20 * scale as i32 as u32))?;
        }
    }
    y += box_h + 16;

    let help = [
        "This sends a UDP probe to the host and waits for a reply.",
        "Use it BEFORE starting a real match to diagnose NAT / firewall.",
        "The host must be on the Host Match screen (waiting for peer).",
    ];
    for line in help {
        font.draw(canvas, line, x, y, small, Color::RGB(140, 140, 140))?;
        y += 18 * small as i32 + 2;
    }

    let footer = "ENTER Probe   ESC Back";
    let fs = small_scale(h);
    let fw = font.text_width_exact(footer, fs);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        fs,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

/// Result screen: lines pre-formatted by main.rs after the probe runs.
/// Colour is picked per-line based on prefix tags "OK", "FAIL", "WARN".
fn draw_test_result(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    lines: &[String],
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "TEST RESULT", w, h)?;
    let scale = body_scale(h).saturating_sub(1).max(1);
    let small = small_scale(h);
    let x = w / 8;
    let mut y = 30 + 28 * title_scale(h) as i32;

    for line in lines {
        let colour = if line.starts_with("OK ") {
            Color::RGB(120, 230, 120)
        } else if line.starts_with("FAIL ") {
            Color::RGB(240, 100, 100)
        } else if line.starts_with("WARN ") {
            Color::RGB(240, 200, 100)
        } else {
            Color::RGB(200, 200, 210)
        };
        font.draw(canvas, line, x, y, scale, colour)?;
        y += 22 * scale as i32 + 4;
    }

    let footer = "ENTER Retest   ESC Back";
    let fw = font.text_width_exact(footer, small);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        small,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn draw_matchmaking(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    status: &str,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "FIND MATCH", w, h)?;
    let scale = body_scale(h);
    let small = small_scale(h);
    let cx = w / 2;
    let mut y = (h / 2) - 40;

    let dots = match (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        / 500)
        % 4
    {
        0 => "",
        1 => ".",
        2 => "..",
        _ => "...",
    };
    let line = format!("{}{}", status, dots);
    let tw = font.text_width_exact(&line, scale);
    font.draw(
        canvas,
        &line,
        cx - tw / 2,
        y,
        scale,
        Color::RGB(255, 220, 120),
    )?;
    y += 36 * scale as i32;

    let hint = "Discord login will open in your browser";
    let hw = font.text_width_exact(hint, small);
    font.draw(
        canvas,
        hint,
        cx - hw / 2,
        y,
        small,
        Color::RGB(140, 140, 160),
    )?;

    let footer = "ESC Cancel";
    let fw = font.text_width_exact(footer, small);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        small,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn draw_leaderboard(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    state: &LeaderboardState,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "LEADERBOARD", w, h)?;
    let scale = body_scale(h).saturating_sub(1).max(1);
    let small = small_scale(h);
    let x = (w / 10).max(34);
    let mut y = 34 + 26 * title_scale(h) as i32;

    match state {
        LeaderboardState::Loading => {
            let dots = match (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                / 500)
                % 4
            {
                0 => "",
                1 => ".",
                2 => "..",
                _ => "...",
            };
            let msg = format!("Loading{dots}");
            let tw = font.text_width_exact(&msg, scale);
            font.draw(
                canvas,
                &msg,
                (w - tw) / 2,
                y + 30,
                scale,
                Color::RGB(255, 220, 120),
            )?;
        }
        LeaderboardState::Error(message) => {
            let line = fit_line(font, message, small, w - 2 * x);
            let tw = font.text_width_exact(&line, small);
            font.draw(
                canvas,
                &line,
                (w - tw) / 2,
                y + 30,
                small,
                Color::RGB(220, 180, 180),
            )?;
        }
        LeaderboardState::Loaded(rows) => {
            if rows.is_empty() {
                let msg = "No ranked matches yet";
                let tw = font.text_width_exact(msg, scale);
                font.draw(
                    canvas,
                    msg,
                    (w - tw) / 2,
                    y + 30,
                    scale,
                    Color::RGB(180, 190, 210),
                )?;
            } else {
                let header = "RANK PLAYER                 RATING  W-L";
                font.draw(canvas, header, x, y, small, Color::RGB(120, 130, 155))?;
                y += 24 * small as i32;

                let max_rows = ((h - y - 44) / 26).max(1) as usize;
                for (i, row) in rows.iter().take(max_rows).enumerate() {
                    let rank = format!("#{}", i + 1);
                    let rating = row.rating.to_string();
                    let record = format!("{}-{}", row.wins, row.losses);
                    let rank_w = font.text_width_exact(&rank, scale);
                    let rating_w = font.text_width_exact(&rating, scale);
                    let record_w = font.text_width_exact(&record, scale);
                    let name_x = x + 54;
                    let rating_x = w - x - record_w - 98;
                    let row_y = y + i as i32 * 26;
                    let name_max = rating_x - name_x - 18;
                    let name = fit_line(font, &row.username, scale, name_max);
                    let colour = if i == 0 {
                        Color::RGB(255, 220, 120)
                    } else {
                        Color::RGB(205, 210, 225)
                    };

                    font.draw(canvas, &rank, x + 40 - rank_w, row_y, scale, colour)?;
                    font.draw(canvas, &name, name_x, row_y, scale, colour)?;
                    font.draw(
                        canvas,
                        &rating,
                        rating_x + 58 - rating_w,
                        row_y,
                        scale,
                        Color::RGB(180, 205, 255),
                    )?;
                    font.draw(
                        canvas,
                        &record,
                        w - x - record_w,
                        row_y,
                        scale,
                        Color::RGB(160, 180, 170),
                    )?;
                }
            }
        }
    }

    let footer = "ESC Back";
    let fw = font.text_width_exact(footer, small);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        small,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn draw_settings(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    cursor: usize,
    discord_rpc_enabled: bool,
    fullscreen: bool,
    volume_percent: u8,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "SETTINGS", w, h)?;
    let scale = body_scale(h).saturating_sub(1).max(1);
    let small = small_scale(h);
    let x = (w / 8).max(42);
    let mut y = 38 + 28 * title_scale(h) as i32;
    let row_h = 32;

    for (i, label) in SETTINGS_ITEMS.iter().enumerate() {
        let selected = i == cursor;
        let row_y = y + i as i32 * row_h;
        if selected {
            canvas.set_draw_color(Color::RGBA(32, 38, 62, 220));
            canvas.fill_rect(Rect::new(x - 12, row_y - 5, (w - 2 * x + 24) as u32, 28))?;
            font.draw(canvas, ">", x - 28, row_y, scale, Color::RGB(255, 235, 180))?;
        }
        let colour = if selected {
            Color::RGB(245, 245, 250)
        } else {
            Color::RGB(175, 180, 198)
        };
        font.draw(canvas, label, x, row_y, scale, colour)?;

        let value = match i {
            0 => Some(if discord_rpc_enabled { "ON" } else { "OFF" }.to_string()),
            1 => Some(if fullscreen { "ON" } else { "OFF" }.to_string()),
            2 => Some(format!("{volume_percent}%")),
            _ => None,
        };
        if let Some(value) = value {
            let vw = font.text_width_exact(&value, scale);
            let enabled_colour = match i {
                2 => Color::RGB(180, 205, 255),
                _ if value == "ON" => Color::RGB(120, 230, 150),
                _ => Color::RGB(210, 140, 140),
            };
            font.draw(canvas, &value, w - x - vw, row_y, scale, enabled_colour)?;
        }
    }

    y += SETTINGS_ITEMS.len() as i32 * row_h + 20;
    let notes = [
        "Doctor checks local setup in a separate window.",
        "Use LEFT/RIGHT on Volume.",
        "Logs are written next to the app while Freeplay runs.",
    ];
    for note in notes {
        font.draw(canvas, note, x, y, small, Color::RGB(130, 140, 165))?;
        y += 20 * small as i32;
    }

    let footer = "ENTER Select   ESC Back";
    let fw = font.text_width_exact(footer, small);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        small,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn draw_training(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    cursor: usize,
    hitboxes: bool,
    infinite_health: bool,
    freeze_timer: bool,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "TRAINING", w, h)?;
    let scale = body_scale(h).saturating_sub(1).max(1);
    let small = small_scale(h);
    let x = (w / 8).max(42);
    let mut y = 38 + 28 * title_scale(h) as i32;
    let row_h = 32;

    for (i, label) in TRAINING_ITEMS.iter().enumerate() {
        let selected = i == cursor;
        let row_y = y + i as i32 * row_h;
        if selected {
            canvas.set_draw_color(Color::RGBA(32, 38, 62, 220));
            canvas.fill_rect(Rect::new(x - 12, row_y - 5, (w - 2 * x + 24) as u32, 28))?;
            font.draw(canvas, ">", x - 28, row_y, scale, Color::RGB(255, 235, 180))?;
        }
        let enabled = match i {
            0 => hitboxes,
            1 => infinite_health,
            2 => freeze_timer,
            _ => false,
        };
        let colour = if selected {
            Color::RGB(245, 245, 250)
        } else {
            Color::RGB(175, 180, 198)
        };
        font.draw(canvas, label, x, row_y, scale, colour)?;

        let value = if enabled { "ON" } else { "OFF" };
        let vw = font.text_width_exact(value, scale);
        font.draw(
            canvas,
            value,
            w - x - vw,
            row_y,
            scale,
            if enabled {
                Color::RGB(120, 230, 150)
            } else {
                Color::RGB(210, 140, 140)
            },
        )?;
    }

    y += TRAINING_ITEMS.len() as i32 * row_h + 20;
    let notes = [
        "These helpers are disabled during online matches.",
        "F2/F3/F4 still toggle them while playing.",
    ];
    for note in notes {
        font.draw(canvas, note, x, y, small, Color::RGB(130, 140, 165))?;
        y += 20 * small as i32;
    }

    let footer = "ENTER Toggle   ESC Back";
    let fw = font.text_width_exact(footer, small);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        small,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn draw_profile(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    state: &ProfileScreenState,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "PROFILE", w, h)?;
    let body = body_scale(h);
    let small = small_scale(h);
    let cx = w / 2;
    let mut y = 30 + 28 * title_scale(h) as i32;

    match state {
        ProfileScreenState::NotLoggedIn => {
            let msg = "Sign in via Find Match first.";
            let tw = font.text_width_exact(msg, body);
            font.draw(canvas, msg, cx - tw / 2, y, body, Color::RGB(220, 220, 230))?;
        }
        ProfileScreenState::Loading => {
            let dots = match (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                / 500)
                % 4
            {
                0 => "",
                1 => ".",
                2 => "..",
                _ => "...",
            };
            let msg = format!("Loading{dots}");
            let tw = font.text_width_exact(&msg, body);
            font.draw(
                canvas,
                &msg,
                cx - tw / 2,
                y,
                body,
                Color::RGB(255, 220, 120),
            )?;
        }
        ProfileScreenState::Error(e) => {
            let tw = font.text_width_exact(e, small);
            font.draw(canvas, e, cx - tw / 2, y, small, Color::RGB(220, 180, 180))?;
        }
        ProfileScreenState::Loaded {
            profile,
            history,
            avatar_rgba,
        } => {
            let content_x = (w / 12).max(42);
            let content_w = w - content_x * 2;
            let top_h = 84.max(60 * small as i32);
            let avatar_size: i32 = 48;
            let rank = estimate_rank(profile.rating);
            let total = profile.wins + profile.losses;
            let pct = if total > 0 {
                (profile.wins as f64 / total as f64 * 100.0) as u32
            } else {
                0
            };

            draw_panel(
                canvas,
                content_x,
                y,
                content_w,
                top_h,
                Color::RGBA(15, 16, 24, 230),
            )?;

            let avatar_x = content_x + 16;
            let avatar_y = y + (top_h - avatar_size) / 2;
            if let Some((ref rgba, aw, ah)) = avatar_rgba {
                let tc = canvas.texture_creator();
                let mut tex = tc
                    .create_texture_static(sdl2::pixels::PixelFormatEnum::RGBA32, *aw, *ah)
                    .map_err(|e| e.to_string())?;
                tex.update(None, rgba, *aw as usize * 4)
                    .map_err(|e| e.to_string())?;
                canvas.copy(
                    &tex,
                    None,
                    Rect::new(avatar_x, avatar_y, avatar_size as u32, avatar_size as u32),
                )?;
            } else {
                canvas.set_draw_color(Color::RGBA(42, 44, 58, 255));
                canvas.fill_rect(Rect::new(
                    avatar_x,
                    avatar_y,
                    avatar_size as u32,
                    avatar_size as u32,
                ))?;
            }

            let name_x = avatar_x + avatar_size + 18;
            let name_y = y + 18;
            font.draw(
                canvas,
                &profile.username,
                name_x,
                name_y,
                body,
                Color::RGB(245, 245, 248),
            )?;
            let sub = format!("{rank}  |  RD +/-{}", profile.deviation);
            font.draw(
                canvas,
                &sub,
                name_x,
                name_y + 18 * body as i32,
                small,
                Color::RGB(150, 160, 178),
            )?;

            let badge = format!("{}", profile.rating);
            let badge_w = font.text_width_exact(&badge, body);
            let badge_x = content_x + content_w - badge_w - 22;
            font.draw(
                canvas,
                &badge,
                badge_x,
                name_y,
                body,
                Color::RGB(255, 220, 120),
            )?;
            let lbl = "RATING";
            let lbl_w = font.text_width_exact(lbl, small);
            font.draw(
                canvas,
                lbl,
                badge_x + badge_w - lbl_w,
                name_y + 18 * body as i32,
                small,
                Color::RGB(130, 136, 152),
            )?;

            y += top_h + 14;

            let stat_gap = 10;
            let stat_h = 64;
            let stat_w = (content_w - stat_gap * 3) / 4;
            draw_stat_box(
                canvas,
                font,
                content_x,
                y,
                stat_w,
                stat_h,
                "WINS",
                &profile.wins.to_string(),
                small,
            )?;
            draw_stat_box(
                canvas,
                font,
                content_x + (stat_w + stat_gap),
                y,
                stat_w,
                stat_h,
                "LOSSES",
                &profile.losses.to_string(),
                small,
            )?;
            draw_stat_box(
                canvas,
                font,
                content_x + (stat_w + stat_gap) * 2,
                y,
                stat_w,
                stat_h,
                "WIN RATE",
                &format!("{pct}%"),
                small,
            )?;
            draw_stat_box(
                canvas,
                font,
                content_x + (stat_w + stat_gap) * 3,
                y,
                stat_w,
                stat_h,
                "MATCHES",
                &profile.matches_played.to_string(),
                small,
            )?;

            y += stat_h + 16;

            font.draw(
                canvas,
                "RECENT MATCHES",
                content_x,
                y,
                small,
                Color::RGB(190, 194, 210),
            )?;
            y += 18 * small as i32;

            if history.is_empty() {
                let msg = "(none yet)";
                let tw = font.text_width_exact(msg, small);
                font.draw(
                    canvas,
                    msg,
                    cx - tw / 2,
                    y,
                    small,
                    Color::RGB(140, 140, 160),
                )?;
            } else {
                let row_h = 24 * small as i32;
                for row in history.iter().take(8) {
                    let tag = if row.result == "won" { "W" } else { "L" };
                    let tag_color = if row.result == "won" {
                        Color::RGB(120, 230, 120)
                    } else {
                        Color::RGB(240, 100, 100)
                    };
                    canvas.set_draw_color(Color::RGBA(18, 19, 28, 210));
                    canvas.fill_rect(Rect::new(
                        content_x,
                        y - 4,
                        content_w as u32,
                        (row_h - 4) as u32,
                    ))?;
                    font.draw(canvas, tag, content_x + 10, y, small, tag_color)?;

                    let opp = format!("vs {}", row.opponent_username);
                    font.draw(
                        canvas,
                        &opp,
                        content_x + 34,
                        y,
                        small,
                        Color::RGB(220, 222, 230),
                    )?;

                    let score = format!("{}-{}", row.our_score, row.opponent_score);
                    let sw = font.text_width_exact(&score, small);
                    font.draw(
                        canvas,
                        &score,
                        content_x + content_w - sw - 16,
                        y,
                        small,
                        Color::RGB(245, 245, 248),
                    )?;
                    y += row_h;
                }
            }
        }
    }

    let footer = "ESC Back";
    let fw = font.text_width_exact(footer, small);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        small,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn draw_panel(
    canvas: &mut Canvas<Window>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: Color,
) -> Result<(), String> {
    canvas.set_draw_color(color);
    canvas.fill_rect(Rect::new(x, y, w as u32, h as u32))?;
    canvas.set_draw_color(Color::RGBA(80, 82, 96, 210));
    canvas.fill_rect(Rect::new(x, y, w as u32, 1))?;
    canvas.fill_rect(Rect::new(x, y + h - 1, w as u32, 1))?;
    Ok(())
}

fn draw_stat_box(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    label: &str,
    value: &str,
    scale: u32,
) -> Result<(), String> {
    draw_panel(canvas, x, y, w, h, Color::RGBA(18, 19, 28, 220))?;
    let value_w = font.text_width_exact(value, scale + 1);
    font.draw(
        canvas,
        value,
        x + (w - value_w) / 2,
        y + 7,
        scale + 1,
        Color::RGB(245, 245, 248),
    )?;
    let label_w = font.text_width_exact(label, scale);
    font.draw(
        canvas,
        label,
        x + (w - label_w) / 2,
        y + h - 20 * scale as i32,
        scale,
        Color::RGB(130, 136, 152),
    )?;
    Ok(())
}

fn draw_ghost_select(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    cursor: usize,
    entries: &[GhostEntry],
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "LOAD GHOST", w, h)?;
    let small = small_scale(h);
    let cx = w / 2;
    let content_x = (w / 12).max(42);
    let content_w = w - content_x * 2;
    let row_h = 34 * small as i32;
    let mut y = 112;

    if entries.is_empty() {
        draw_panel(
            canvas,
            content_x,
            y - 14,
            content_w,
            92,
            Color::RGBA(15, 16, 24, 225),
        )?;
        let msg = "No ghost recordings found";
        let tw = font.text_width_exact(msg, small);
        font.draw(
            canvas,
            msg,
            cx - tw / 2,
            y,
            small,
            Color::RGB(180, 180, 200),
        )?;
        y += 24 * small as i32;
        let hint = "Record during netplay or use F6 to record locally";
        let hw = font.text_width_exact(hint, small);
        font.draw(
            canvas,
            hint,
            cx - hw / 2,
            y,
            small,
            Color::RGB(140, 140, 160),
        )?;
    } else {
        let header = format!("{} ghosts available", entries.len());
        font.draw(
            canvas,
            &header,
            content_x,
            y - 24,
            small,
            Color::RGB(150, 156, 176),
        )?;

        let max_visible = ((h - y - 62) / row_h).max(4) as usize;
        let start = if cursor >= max_visible {
            cursor - max_visible + 1
        } else {
            0
        };
        let end = (start + max_visible).min(entries.len());

        for i in start..end {
            let selected = i == cursor;
            let (kind, display, subtitle) = match &entries[i] {
                GhostEntry::Local { filename, .. } => {
                    let name = strip_ncgh(filename);
                    let info = parse_ghost_info(filename);
                    ("LOCAL", name, info)
                }
                GhostEntry::Remote(meta) => {
                    let name = strip_ncgh(&meta.filename);
                    let info = format!("by {} \u{2022} {} frames", meta.username, meta.frame_count);
                    ("REMOTE", name, info)
                }
            };
            let bg = if selected {
                Color::RGBA(42, 38, 24, 235)
            } else {
                Color::RGBA(15, 16, 24, 215)
            };
            canvas.set_draw_color(bg);
            canvas.fill_rect(Rect::new(
                content_x,
                y - 4,
                content_w as u32,
                (row_h - 4) as u32,
            ))?;
            canvas.set_draw_color(if selected {
                Color::RGB(255, 210, 80)
            } else {
                Color::RGBA(70, 72, 88, 220)
            });
            canvas.fill_rect(Rect::new(content_x, y - 4, 3, (row_h - 4) as u32))?;

            font.draw(
                canvas,
                kind,
                content_x + 12,
                y,
                small,
                Color::RGB(130, 136, 152),
            )?;
            let name_color = if selected {
                Color::RGB(255, 230, 120)
            } else {
                Color::RGB(220, 222, 232)
            };
            font.draw(canvas, &display, content_x + 86, y, small, name_color)?;

            let sw = font.text_width_exact(&subtitle, small);
            font.draw(
                canvas,
                &subtitle,
                content_x + content_w - sw - 14,
                y,
                small,
                Color::RGB(130, 136, 152),
            )?;
            y += row_h;
        }
    }

    let footer = "ESC Back   Enter Load";
    let fw = font.text_width_exact(footer, small);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        small,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn strip_ncgh(s: &str) -> String {
    if s.ends_with(".ncgh") {
        s[..s.len() - 5].to_string()
    } else {
        s.to_string()
    }
}

fn parse_ghost_info(filename: &str) -> String {
    let base = strip_ncgh(filename);
    let parts: Vec<&str> = base.split('_').collect();
    if parts.len() >= 2 {
        let ip = parts[0].replace('-', ".");
        if parts.len() >= 3 {
            let ts = parts.last().unwrap_or(&"");
            if let Ok(n) = ts.parse::<u64>() {
                let secs = n % 1_000_000_000;
                let t = chrono_prelude(secs as i64);
                return format!("vs {ip} \u{2022} {t}");
            }
        }
        return format!("vs {ip}");
    }
    String::new()
}

fn chrono_prelude(secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let diff = now.saturating_sub(secs);
    if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

fn estimate_rank(rating: i32) -> &'static str {
    if rating < 300 {
        "Beginner"
    } else if rating < 600 {
        "Amateur"
    } else if rating < 900 {
        "Intermediate"
    } else if rating < 1200 {
        "Skilled"
    } else if rating < 1500 {
        "Expert"
    } else if rating < 1800 {
        "Master"
    } else {
        "Grandmaster"
    }
}
