//! In-engine menu: list-based screens, pad/keyboard navigation, rebind flow.
use crate::config::{
    AspectMode, AudioBuffer, RenderProfile, ScorebarStyle, VideoFilter, MAX_USERNAME_LEN,
};
use crate::font::Font;
use crate::input::{is_action_active, Action, Binding, Bindings, Player, PlayerBindings};
use crate::matchmaking::{
    HistoryRow, IncomingChallenge, LeaderboardRow, LiveMatch, LobbyChatMessage, LobbyCurrent,
    LobbyMemberInfo, LobbyUser, LobbyView, ProfileData, RemoteGhostMeta,
};
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
    /// New angular/red UI rebuild (`crate::fp_ui`) — Main/Play/Settings/
    /// Lobby/Quit only; every other screen routes to `Menu(MenuScreen::_)`
    /// via `main_menu_state` and `MenuScreen`'s own navigation, never
    /// replacing them; see `crate::fp_ui` module docs.
    FpUi(crate::fp_ui::FpScreen),
    Playing,
    Rebinding {
        action: Action,
        player: Player,
        came_from: MenuScreen,
    },
}

/// The app's "main menu" state: the new fp_ui Main Menu when `new_ui` is on,
/// otherwise the legacy `Menu(MenuScreen::Main)`. Centralizes every "return
/// to the main menu" transition so flipping `new_ui` doesn't require hunting
/// down each call site individually.
pub fn main_menu_state(new_ui: bool) -> AppState {
    if new_ui {
        AppState::FpUi(crate::fp_ui::FpScreen::main())
    } else {
        AppState::Menu(MenuScreen::Main { cursor: 0 })
    }
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
    /// Online hub: a left nav rail (Play / Chat / Lobbies / Watch) with a
    /// content pane. `focus` decides whether d-pad navigation drives the rail
    /// (switching section) or the content (rows / chat scroll). `chat` and
    /// `presence` keep the server's structured lobby snapshot so chat can show
    /// colored sender nicks and an online list.
    OnlineHub {
        tab: OnlineTab,
        focus: HubFocus,
        cursor: usize,
        challenge_format: ChallengeFormat,
        chat_draft: String,
        chat: Vec<LobbyChatMessage>,
        presence: Vec<LobbyUser>,
        chat_scroll: usize,
        /// On-screen keyboard cursor (row, col) used on the Chat section.
        osk_row: usize,
        osk_col: usize,
        /// When set, the format chooser is open for the selected player on the
        /// Players section (value is the highlighted format index 0..4).
        challenge_pick: Option<usize>,
        /// A challenge someone sent us — shown as a modal accept/decline prompt.
        incoming: Option<IncomingChallenge>,
        lobbies: Vec<LobbyPreview>,
        live_matches: Vec<LiveMatch>,
        status: String,
    },
    LabMenu {
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
        replay_path: Option<String>,
    },
    /// Automated matchmaking via the signaling server. `status` is a
    /// human-readable progress string updated by the background thread.
    Matchmaking {
        status: String,
    },
    /// Pre-match username gate. The player must confirm a unique guest name
    /// before entering the public matchmaking queue.
    MatchUsername {
        value: String,
        status: String,
        checking: bool,
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
    /// Browse locally saved full match replays. These are deterministic set
    /// replays and are intentionally separate from ghost opponent files.
    ReplaySelect {
        cursor: usize,
        entries: Vec<ReplayEntry>,
        status: Option<String>,
    },
    /// Player rating + recent matches fetched from freeplay-stats. Loads
    /// asynchronously — main.rs swaps the inner state as the fetch completes.
    Profile {
        state: ProfileScreenState,
    },
    /// Community rating leaderboard fetched from freeplay-stats.
    #[allow(dead_code)]
    Leaderboard {
        state: LeaderboardState,
    },
    /// Small utility/settings panel for runtime toggles and diagnostics.
    Settings {
        cursor: usize,
        player_username: String,
        stats_email: String,
        discord_connected: bool,
        discord_rpc_enabled: bool,
        fullscreen: bool,
        volume_percent: u8,
        audio_buffer: AudioBuffer,
        video_filter: VideoFilter,
        crt_corner_bend: bool,
        aspect_mode: AspectMode,
        scorebar_style: ScorebarStyle,
        input_delay: u32,
        render_profile: RenderProfile,
        runahead: bool,
        runahead_online: bool,
    },
    /// Lab helpers backed by RAM pokes already used by F-keys.
    Training {
        cursor: usize,
        hitboxes: bool,
        infinite_health: bool,
        freeze_timer: bool,
    },
    /// Active online matches fetched from the signaling server.
    #[allow(dead_code)]
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
    TextEdit {
        title: String,
        label: String,
        value: String,
        field: EditField,
        came_from: Box<MenuScreen>,
    },
    /// A king-of-the-hill lobby room: current match, play queue, spectators.
    /// `view` is `None` until the first poll lands.
    Lobby {
        id: String,
        view: Option<LobbyView>,
        status: String,
        /// Latest match thumbnail (RGBA, width, height), streamed from the two
        /// active players. `None` until the first frame arrives.
        thumb: Option<(Vec<u8>, u32, u32)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditField {
    Username,
    StatsEmail,
    ReplayNote { path: String, cursor: usize },
    /// Entering a lobby invite code to join a private lobby.
    JoinCode,
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
pub struct ReplayEntry {
    pub filename: String,
    pub path: String,
    pub remote_url: Option<String>,
    pub p1_name: String,
    pub p2_name: String,
    pub p1_score: Option<u16>,
    pub p2_score: Option<u16>,
    pub winner: String,
    pub frame_count: u32,
    pub duration: String,
    pub recorded_at: String,
    pub note: String,
    pub bookmark_count: usize,
}

/// Which column of the Online hub the d-pad currently drives.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HubFocus {
    /// Up/Down switch section; Right/Enter dive into content; Left/Back exit.
    Rail,
    /// Up/Down move within the section; Left/Back returns to the rail.
    Content,
}

/// Sections of the Online hub, shown as a vertical nav rail.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OnlineTab {
    /// Pick a challenge format and find a match.
    Play,
    /// General lobby chat + online presence.
    Chat,
    /// Online players you can challenge directly.
    Players,
    /// Browse / create / join public lobbies.
    Lobbies,
    /// Watch live matches.
    Watch,
}

impl OnlineTab {
    const ALL: [OnlineTab; 5] = [
        OnlineTab::Play,
        OnlineTab::Chat,
        OnlineTab::Players,
        OnlineTab::Lobbies,
        OnlineTab::Watch,
    ];

    fn label(self) -> &'static str {
        match self {
            OnlineTab::Play => "Play",
            OnlineTab::Chat => "Chat",
            OnlineTab::Players => "Players",
            OnlineTab::Lobbies => "Lobbies",
            OnlineTab::Watch => "Watch",
        }
    }

    fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChallengeFormat {
    UnrankedVs,
    RankedFt3,
    RankedFt5,
    RankedFt10,
}

impl ChallengeFormat {
    const ALL: [ChallengeFormat; 4] = [
        ChallengeFormat::UnrankedVs,
        ChallengeFormat::RankedFt3,
        ChallengeFormat::RankedFt5,
        ChallengeFormat::RankedFt10,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ChallengeFormat::UnrankedVs => "Unranked VS",
            ChallengeFormat::RankedFt3 => "Ranked FT3",
            ChallengeFormat::RankedFt5 => "Ranked FT5",
            ChallengeFormat::RankedFt10 => "Ranked FT10",
        }
    }

    fn next(self) -> Self {
        let idx = Self::ALL
            .iter()
            .position(|format| *format == self)
            .unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    pub fn ranked(self) -> bool {
        !matches!(self, ChallengeFormat::UnrankedVs)
    }

    pub fn match_limit(self) -> u32 {
        match self {
            ChallengeFormat::UnrankedVs => 1,
            ChallengeFormat::RankedFt3 => 3,
            ChallengeFormat::RankedFt5 => 5,
            ChallengeFormat::RankedFt10 => 10,
        }
    }

    /// Format at a chooser index (clamped).
    pub fn at_index(idx: usize) -> ChallengeFormat {
        ChallengeFormat::ALL[idx.min(ChallengeFormat::ALL.len() - 1)]
    }

    /// Index of this format in the chooser order.
    pub fn index(self) -> usize {
        ChallengeFormat::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }

    /// Wire string sent to the signaling server's room/lobby format field.
    pub fn wire(self) -> &'static str {
        match self {
            ChallengeFormat::UnrankedVs => "vs",
            ChallengeFormat::RankedFt3 => "ft3",
            ChallengeFormat::RankedFt5 => "ft5",
            ChallengeFormat::RankedFt10 => "ft10",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LobbyPreview {
    /// Server room id — passed to `start_join_room` when the row is selected.
    pub id: String,
    pub name: String,
    pub host: String,
    pub format: ChallengeFormat,
    pub players: u8,
    pub private: bool,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileScreenState {
    /// No active player identity is available yet.
    NotLoggedIn,
    /// Background fetch in flight.
    Loading,
    /// Identity exists but no ranked matches yet (404). Shows an empty profile
    /// for `username` instead of a bare error line.
    Empty { username: String },
    /// The network failed (non-404).
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
///
/// Watch Live, Leaderboard, and Training are still implemented, but are hidden
/// from the public menu for now to keep the first-run footprint focused.
pub const MAIN_ITEMS: [&str; 9] = [
    "Online", "Arcade", "Lab", "Replays", "Profile", "Controls", "Settings", "About", "Quit",
];
/// Index of "Settings" within `MAIN_ITEMS` — `crate::fp_ui` special-cases it
/// (opens its own Settings screen rather than delegating to legacy) the same
/// way it special-cases the last index for Quit.
pub const MAIN_SETTINGS_INDEX: usize = 6;
const LAB_MENU_ITEMS: [&str; 2] = ["Start Lab", "Load Drones"];

const SETTINGS_ITEMS: [&str; 18] = [
    "Username",
    "Stats Email",
    "Discord Account",
    "Discord Rich Presence",
    "Fullscreen",
    "Volume",
    "Audio Buffer",
    "Video Filter",
    "CRT Glass",
    "Aspect",
    "Scorebar",
    "Input Delay",
    "Run Doctor",
    "Open Clips Folder",
    "Open Logs Folder",
    "Render Profile",
    "Runahead (offline)",
    "Runahead (online, experimental)",
];
const TRAINING_ITEMS: [&str; 3] = ["Hitbox View", "Infinite Health", "Freeze Timer"];

// ── Online hub palette ───────────────────────────────────────────────────────
// Shared colors so every section reads as one screen. RGBA panels sit over the
// menu's dark clear color.
const HUB_PANEL: Color = Color::RGBA(16, 19, 30, 230);
const HUB_PANEL_SEL: Color = Color::RGBA(40, 48, 78, 242);
const HUB_RAIL_BG: Color = Color::RGBA(12, 14, 22, 235);
const HUB_ACCENT: Color = Color::RGB(255, 205, 90);
const HUB_TEXT: Color = Color::RGB(214, 222, 238);
const HUB_DIM: Color = Color::RGB(120, 134, 162);
const HUB_FAINT: Color = Color::RGB(92, 104, 130);
/// Deterministic per-nick colors for chat senders + presence.
const NICK_PALETTE: [Color; 8] = [
    Color::RGB(245, 170, 120),
    Color::RGB(130, 205, 170),
    Color::RGB(150, 185, 255),
    Color::RGB(235, 150, 190),
    Color::RGB(205, 195, 120),
    Color::RGB(170, 165, 245),
    Color::RGB(120, 210, 220),
    Color::RGB(225, 130, 130),
];

// ── Mouse hit regions ────────────────────────────────────────────────────────
// The hub records clickable rectangles for challenge targets (presence names)
// and the format chooser each time it draws, so the event loop can resolve a
// right/left click to a player index or format index. Single-threaded UI, so a
// thread-local is fine. Rects are in window-pixel space (menus draw with
// logical scaling disabled, see main.rs).
thread_local! {
    static PRESENCE_HITS: std::cell::RefCell<Vec<(i32, i32, i32, i32, usize)>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static FORMAT_HITS: std::cell::RefCell<Vec<(i32, i32, i32, i32, usize)>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static PHRASE_HITS: std::cell::RefCell<Vec<(i32, i32, i32, i32, usize)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Common chat phrases offered as one-click inserts.
pub const QUICK_PHRASES: [&str; 7] =
    ["gg", "ggs", "wp", "one more?", "rematch?", "nice", "lag?"];

/// The quick phrase at an index (for the click handler).
pub fn quick_phrase(idx: usize) -> &'static str {
    QUICK_PHRASES.get(idx).copied().unwrap_or("")
}

fn hits_reset() {
    PRESENCE_HITS.with(|h| h.borrow_mut().clear());
    FORMAT_HITS.with(|h| h.borrow_mut().clear());
    PHRASE_HITS.with(|h| h.borrow_mut().clear());
}

fn record_phrase_hit(x: i32, y: i32, w: i32, h: i32, idx: usize) {
    PHRASE_HITS.with(|hits| hits.borrow_mut().push((x, y, w, h, idx)));
}

/// Quick-phrase chip index at a window-pixel point, if any.
pub fn phrase_hit_at(px: i32, py: i32) -> Option<usize> {
    PHRASE_HITS.with(|h| hit_lookup(h, px, py))
}

fn record_presence_hit(x: i32, y: i32, w: i32, h: i32, idx: usize) {
    PRESENCE_HITS.with(|hits| hits.borrow_mut().push((x, y, w, h, idx)));
}

fn record_format_hit(x: i32, y: i32, w: i32, h: i32, idx: usize) {
    FORMAT_HITS.with(|hits| hits.borrow_mut().push((x, y, w, h, idx)));
}

fn hit_lookup(hits: &std::cell::RefCell<Vec<(i32, i32, i32, i32, usize)>>, px: i32, py: i32) -> Option<usize> {
    hits.borrow()
        .iter()
        .find(|(x, y, w, h, _)| px >= *x && px < *x + *w && py >= *y && py < *y + *h)
        .map(|t| t.4)
}

/// Player presence index at a window-pixel point, if any (for right-click).
pub fn presence_hit_at(px: i32, py: i32) -> Option<usize> {
    PRESENCE_HITS.with(|h| hit_lookup(h, px, py))
}

/// Format-chooser row index at a window-pixel point, if any (for left-click).
pub fn format_hit_at(px: i32, py: i32) -> Option<usize> {
    FORMAT_HITS.with(|h| hit_lookup(h, px, py))
}

/// A presence/player label showing the rating in parentheses, e.g.
/// `reptilefan (1403)`, when the server provided one.
fn user_label(u: &LobbyUser) -> String {
    match u.rating {
        Some(r) => format!("{} ({r})", u.username),
        None => u.username.clone(),
    }
}

/// FNV-1a hash → stable color for a username.
fn nick_color(name: &str) -> Color {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    NICK_PALETTE[(h as usize) % NICK_PALETTE.len()]
}

// ── On-screen keyboard ───────────────────────────────────────────────────────
// A d-pad-navigable keyboard so controller players can type in chat without a
// physical keyboard. Physical-keyboard input still works in parallel.
const OSK_CHAR_ROWS: [&str; 4] = ["1234567890", "qwertyuiop", "asdfghjkl", "zxcvbnm"];
const OSK_ROWS: usize = 5; // 4 character rows + 1 action row

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OskKey {
    Char(char),
    Space,
    Backspace,
    Send,
}

fn osk_row_len(row: usize) -> usize {
    if row < 4 {
        OSK_CHAR_ROWS[row].chars().count()
    } else {
        3 // Space, Del, Send
    }
}

fn osk_key_at(row: usize, col: usize) -> OskKey {
    if row < 4 {
        OskKey::Char(OSK_CHAR_ROWS[row].chars().nth(col).unwrap_or(' '))
    } else {
        match col {
            0 => OskKey::Space,
            1 => OskKey::Backspace,
            _ => OskKey::Send,
        }
    }
}

/// Number of selectable rows in a section's content pane (for cursor clamping).
fn content_row_count(tab: OnlineTab, lobbies: &[LobbyPreview], live_matches: &[LiveMatch]) -> usize {
    match tab {
        OnlineTab::Play => 2,             // format selector + Find Match
        OnlineTab::Chat => 1,             // the input line (scroll is separate)
        OnlineTab::Players => 1,          // presence selection (clamped separately)
        OnlineTab::Lobbies => 3 + lobbies.len(), // Create public/private + Join code + lobbies
        OnlineTab::Watch => live_matches.len().max(1),
    }
}

/// Build an Online hub pre-populated with sample data and jumped to a section,
/// for layout/font testing via `--test-screen`. Accepts names like
/// `online:chat`, `online:players`, `online:lobbies`, `online:watch`,
/// `online:play` (or just the section name).
pub fn test_state(name: &str) -> Option<AppState> {
    // Non-online dense screens, for checking readability at the current font.
    match name {
        "controls" => {
            return Some(AppState::Menu(MenuScreen::Controls {
                cursor: 0,
                player: Player::P1,
            }))
        }
        "main" => return Some(AppState::Menu(MenuScreen::Main { cursor: 0 })),
        "fp:main" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::Main { cursor: 0 })),
        "fp:quit" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Quit {
                choice: 0,
                menu_cursor: 0,
            }))
        }
        "fp:playmenu" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::PlayMenu { cursor: 0 })),
        "fp:bandwidth" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::Bandwidth)),
        "fp:rankings" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::Rankings)),
        "fp:about" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::About)),
        "fp:profile" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::Profile)),
        "fp:settings" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::settings_from_cfg(
                &crate::config::load(),
            )))
        }
        "fp:lobby" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::lobby())),
        "fp:lobby:host" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Lobby {
                tab: 1,
                host_join_focus: 0,
                cursor: 0,
                lobbies: Vec::new(),
                status: String::new(),
            }))
        }
        "fp:lobby:browser" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Lobby {
                tab: 2,
                host_join_focus: 0,
                cursor: 0,
                lobbies: vec![
                    LobbyPreview {
                        id: "abc123".into(),
                        name: "Test Lobby".into(),
                        host: "Phantom_9847".into(),
                        format: ChallengeFormat::UnrankedVs,
                        players: 1,
                        private: false,
                        status: "OPEN".into(),
                    },
                    LobbyPreview {
                        id: "def456".into(),
                        name: "Ranked Room".into(),
                        host: "ScorpionPit".into(),
                        format: ChallengeFormat::RankedFt5,
                        players: 2,
                        private: false,
                        status: "IN GAME".into(),
                    },
                ],
                status: String::new(),
            }))
        }
        "profile" => {
            return Some(AppState::Menu(MenuScreen::Profile {
                state: ProfileScreenState::Empty {
                    username: crate::config::default_username(),
                },
            }))
        }
        "name" => {
            return Some(AppState::Menu(MenuScreen::MatchUsername {
                value: crate::config::default_username(),
                status: "This is your name — edit it or press Enter to claim it".into(),
                checking: false,
            }))
        }
        "lobby" => {
            return Some(AppState::Menu(MenuScreen::Lobby {
                id: "demo".into(),
                status: String::new(),
                view: Some(LobbyView {
                    id: "AB3K9X".into(),
                    name: "Long sets only".into(),
                    ranked: true,
                    private: true,
                    format: crate::matchmaking::LobbyMatchFormat::RankedFt5,
                    members: vec![
                        LobbyMemberInfo { username: "ScorpionKiller".into(), rating: Some(1403), queued: true, in_match: true },
                        LobbyMemberInfo { username: "SubZeroFan".into(), rating: Some(1521), queued: true, in_match: true },
                        LobbyMemberInfo { username: "Reptile99".into(), rating: Some(1198), queued: true, in_match: false },
                        LobbyMemberInfo { username: "kunglao".into(), rating: None, queued: false, in_match: false },
                    ],
                    queue: vec!["Reptile99".into(), "You".into()],
                    current: Some(LobbyCurrent {
                        host_username: "ScorpionKiller".into(),
                        join_username: "SubZeroFan".into(),
                        host_session: "s1".into(),
                        join_session: "s2".into(),
                    }),
                    ready_check: None,
                    your_position: Some(0),
                    your_queued: true,
                    your_session: None,
                    your_turn: false,
                }),
                thumb: {
                    // Gradient stand-in so the thumbnail panel can be checked.
                    let (tw, th) = (crate::render::LOBBY_THUMB_W, crate::render::LOBBY_THUMB_H);
                    let mut rgba = Vec::with_capacity((tw * th * 4) as usize);
                    for yy in 0..th {
                        for xx in 0..tw {
                            rgba.push((xx * 255 / tw) as u8);
                            rgba.push((yy * 255 / th) as u8);
                            rgba.push(150);
                            rgba.push(255);
                        }
                    }
                    Some((rgba, tw, th))
                },
            }));
        }
        _ => {}
    }
    let section = name
        .strip_prefix("online:")
        .or_else(|| name.strip_prefix("online"))
        .unwrap_or(name)
        .trim_start_matches(':');
    let tab = match section {
        "chat" => OnlineTab::Chat,
        "players" => OnlineTab::Players,
        "lobbies" => OnlineTab::Lobbies,
        "watch" => OnlineTab::Watch,
        "play" | "" => OnlineTab::Play,
        _ => return None,
    };
    let presence = vec![
        LobbyUser { player_id: "p1".into(), username: "ScorpionKiller".into(), status: "online".into(), rating: Some(1403) },
        LobbyUser { player_id: "p2".into(), username: "SubZeroFan".into(), status: "in lobby".into(), rating: Some(1521) },
        LobbyUser { player_id: "p3".into(), username: "Reptile99".into(), status: "online".into(), rating: Some(1198) },
        LobbyUser { player_id: "p4".into(), username: "kunglao".into(), status: "looking".into(), rating: None },
    ];
    let chat = vec![
        LobbyChatMessage { username: "ScorpionKiller".into(), message: "gg wp that was close".into(), timestamp: None },
        LobbyChatMessage { username: "SubZeroFan".into(), message: "anyone up for an ft5?".into(), timestamp: None },
        LobbyChatMessage { username: "Reptile99".into(), message: "lobby is open, hop in".into(), timestamp: None },
    ];
    let lobbies = vec![
        LobbyPreview { id: "r1".into(), name: "Long sets only".into(), host: "Jax".into(), format: ChallengeFormat::RankedFt10, players: 2, private: false, status: "open".into() },
        LobbyPreview { id: "r2".into(), name: "casual fun".into(), host: "Sonya".into(), format: ChallengeFormat::UnrankedVs, players: 1, private: true, status: "open".into() },
    ];
    let live_matches = vec![
        LiveMatch { session_id: "s1".into(), p1_name: "Liu Kang".into(), p2_name: "Kung Lao".into(), p1_score: 2, p2_score: 1 },
        LiveMatch { session_id: "s2".into(), p1_name: "Mileena".into(), p2_name: "Kitana".into(), p1_score: 0, p2_score: 3 },
    ];
    Some(AppState::Menu(MenuScreen::OnlineHub {
        tab,
        focus: HubFocus::Content,
        cursor: 0,
        challenge_format: ChallengeFormat::RankedFt5,
        chat_draft: String::new(),
        chat,
        presence,
        chat_scroll: 0,
        osk_row: 0,
        osk_col: 0,
        challenge_pick: None,
        incoming: None,
        lobbies,
        live_matches,
        status: "Sample data — test screen".into(),
    }))
}

pub enum NavResult {
    Stay,
    StartLocal {
        lab: bool,
    },
    /// Launch automated matchmaking via the signaling server.
    #[allow(dead_code)]
    StartMatchmaking,
    /// Ask main.rs to open the username gate with the current config value.
    OpenUsernameEntry,
    /// Validate the typed name before matchmaking starts.
    SubmitUsername(String),
    /// Open the Profile screen — main.rs kicks off a background fetch from
    /// freeplay-stats and updates the screen state as the response lands.
    OpenProfile,
    /// Open the active match browser.
    OpenLiveMatches,
    /// Open community leaderboard.
    #[allow(dead_code)]
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
    EditText(EditField, String),
    CommitText(EditField, String),
    ConnectDiscord,
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
    /// Send a server-backed General lobby chat message.
    SendLobbyChat(String),
    /// Join a public lobby by its server room id (reuses the deep-link path).
    JoinLobby(String),
    /// Host a new lobby with the given challenge format; bool = private.
    CreateLobby(ChallengeFormat, bool),
    /// Open the "join by invite code" text entry.
    OpenJoinCode,
    /// Challenge a player (presence player_id) to a match in the given format.
    SendChallenge(String, ChallengeFormat),
    /// Accept an incoming challenge by its id.
    AcceptChallenge(String),
    /// Toggle the caller between the play queue and spectating in a lobby.
    SetLobbyQueue(String, bool),
    /// Challenger confirms ready for the pending match in a lobby.
    ReadyLobby(String),
    /// Toggle desktop fullscreen and persist config.
    ToggleFullscreen,
    /// Adjust audio volume by signed percentage points.
    AdjustVolume(i8),
    /// Cycle audio queue stability setting.
    CycleAudioBuffer(i8),
    /// Cycle gameplay video presentation filter.
    CycleVideoFilter(i8),
    /// Toggle rounded CRT glass/corner shading.
    ToggleCrtGlass,
    /// Cycle gameplay frame aspect handling.
    CycleAspectMode(i8),
    /// Cycle the in-game scorebar layout.
    CycleScorebarStyle(i8),
    /// Adjust GGRS input delay by signed frames (clamped 0–8 in main.rs).
    AdjustInputDelay(i8),
    /// Cycle SDL renderer backend preference for hardware tests.
    CycleRenderProfile(i8),
    /// Toggle offline one-frame runahead and persist config.
    ToggleRunahead,
    /// Toggle experimental online (netplay) video-only runahead and persist config.
    ToggleRunaheadOnline,
    /// Open Training helper menu.
    #[allow(dead_code)]
    OpenTraining,
    /// Enter the local full-replay browser.
    OpenReplaySelect,
    /// Load and play a full deterministic match replay.
    LoadReplay(String),
    /// Download and play a public deterministic match replay.
    LoadRemoteReplay(String),
    /// Toggle named training helper.
    ToggleTraining(&'static str),
    /// Launch the external setup diagnostics window.
    LaunchDoctor,
    /// Open the folder where Ctrl+R clips are written.
    OpenClipsFolder,
    /// Open the folder where runtime logs are written.
    OpenLogsFolder,
}

impl AppState {
    pub fn nav_up(&mut self) {
        match self {
            AppState::Menu(MenuScreen::Main { cursor }) => {
                *cursor = cursor.saturating_sub(1);
            }
            AppState::Menu(MenuScreen::OnlineHub {
                tab,
                focus,
                cursor,
                osk_row,
                osk_col,
                challenge_pick,
                incoming,
                ..
            }) => {
                if incoming.is_some() {
                    return; // modal prompt: only Accept/Back respond
                }
                match focus {
                    HubFocus::Rail => {
                        *tab = tab.prev();
                        *cursor = 0;
                        *challenge_pick = None;
                    }
                    HubFocus::Content => match tab {
                        OnlineTab::Chat => {
                            *osk_row = osk_row.saturating_sub(1);
                            *osk_col = (*osk_col).min(osk_row_len(*osk_row).saturating_sub(1));
                        }
                        OnlineTab::Players => {
                            if let Some(f) = challenge_pick {
                                *challenge_pick = Some(f.saturating_sub(1));
                            } else {
                                *cursor = cursor.saturating_sub(1);
                            }
                        }
                        _ => *cursor = cursor.saturating_sub(1),
                    },
                }
            }
            AppState::Menu(MenuScreen::LabMenu { cursor }) => {
                *cursor = cursor.saturating_sub(1);
            }
            AppState::Menu(MenuScreen::Controls { cursor, .. }) => {
                *cursor = cursor.saturating_sub(1);
            }
            AppState::Menu(MenuScreen::GhostSelect { cursor, .. }) => {
                *cursor = cursor.saturating_sub(1);
            }
            AppState::Menu(MenuScreen::ReplaySelect { cursor, .. }) => {
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
            AppState::Menu(MenuScreen::OnlineHub {
                tab,
                focus,
                cursor,
                osk_row,
                osk_col,
                challenge_pick,
                incoming,
                presence,
                lobbies,
                live_matches,
                ..
            }) => {
                if incoming.is_some() {
                    return;
                }
                match focus {
                    HubFocus::Rail => {
                        *tab = tab.next();
                        *cursor = 0;
                        *challenge_pick = None;
                    }
                    HubFocus::Content => match tab {
                        OnlineTab::Chat => {
                            *osk_row = (*osk_row + 1).min(OSK_ROWS - 1);
                            *osk_col = (*osk_col).min(osk_row_len(*osk_row).saturating_sub(1));
                        }
                        OnlineTab::Players => {
                            if let Some(f) = challenge_pick {
                                *challenge_pick = Some((*f + 1).min(ChallengeFormat::ALL.len() - 1));
                            } else if *cursor + 1 < presence.len() {
                                *cursor += 1;
                            }
                        }
                        _ => {
                            if *cursor + 1 < content_row_count(*tab, lobbies, live_matches) {
                                *cursor += 1;
                            }
                        }
                    },
                }
            }
            AppState::Menu(MenuScreen::LabMenu { cursor }) => {
                if *cursor + 1 < LAB_MENU_ITEMS.len() {
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
            AppState::Menu(MenuScreen::ReplaySelect {
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
        match self {
            AppState::Menu(MenuScreen::Controls { player, .. }) => {
                *player = player.other();
            }
            AppState::Menu(MenuScreen::OnlineHub {
                tab,
                cursor,
                chat_scroll,
                ..
            }) => {
                // Tab cycles section regardless of focus.
                *tab = tab.next();
                *cursor = 0;
                *chat_scroll = 0;
            }
            _ => {}
        }
    }

    /// D-pad/arrow Left. On the Chat keyboard it moves the key cursor; in other
    /// hub content it returns focus to the rail; elsewhere it swaps player.
    pub fn nav_left(&mut self) {
        if let AppState::Menu(MenuScreen::OnlineHub {
            tab,
            focus,
            osk_col,
            challenge_pick,
            incoming,
            ..
        }) = self
        {
            if incoming.is_some() {
                return;
            }
            if *focus == HubFocus::Content && *tab == OnlineTab::Chat {
                *osk_col = osk_col.saturating_sub(1);
            } else {
                *focus = HubFocus::Rail;
                *challenge_pick = None;
            }
            return;
        }
        self.nav_switch_player();
    }

    /// D-pad/arrow Right. On the Chat keyboard it moves the key cursor; in the
    /// rail it dives into content; elsewhere it swaps player.
    pub fn nav_right(&mut self) {
        if let AppState::Menu(MenuScreen::OnlineHub {
            tab,
            focus,
            osk_row,
            osk_col,
            incoming,
            ..
        }) = self
        {
            if incoming.is_some() {
                return;
            }
            if *focus == HubFocus::Content && *tab == OnlineTab::Chat {
                let max_col = osk_row_len(*osk_row).saturating_sub(1);
                *osk_col = (*osk_col + 1).min(max_col);
            } else {
                *focus = HubFocus::Content;
            }
            return;
        }
        self.nav_switch_player();
    }

    /// Send the current chat draft (trimmed). Clears the draft on send.
    fn send_chat_draft(&mut self) -> NavResult {
        if let AppState::Menu(MenuScreen::OnlineHub { chat_draft, .. }) = self {
            let message = chat_draft.trim().to_string();
            if !message.is_empty() {
                chat_draft.clear();
                return NavResult::SendLobbyChat(message);
            }
        }
        NavResult::Stay
    }

    /// Edit the chat draft from the on-screen keyboard: `Some(c)` appends a
    /// character (capped), `None` is backspace.
    fn osk_edit_draft(&mut self, ch: Option<char>) {
        if let AppState::Menu(MenuScreen::OnlineHub { chat_draft, .. }) = self {
            match ch {
                Some(c) => {
                    if chat_draft.chars().count() < 180 {
                        chat_draft.push(c);
                    }
                }
                None => {
                    chat_draft.pop();
                }
            }
        }
    }

    pub fn nav_accept(&mut self, rom_present: bool) -> NavResult {
        match self.clone() {
            AppState::Menu(MenuScreen::Main { cursor }) => match cursor {
                0 => {
                    // Online
                    if !rom_present {
                        return NavResult::Stay;
                    }
                    *self = AppState::Menu(MenuScreen::OnlineHub {
                        tab: OnlineTab::Play,
                        focus: HubFocus::Rail,
                        cursor: 0,
                        challenge_format: ChallengeFormat::UnrankedVs,
                        chat_draft: String::new(),
                        chat: Vec::new(),
                        presence: Vec::new(),
                        chat_scroll: 0,
                        osk_row: 0,
                        osk_col: 0,
                        challenge_pick: None,
                        incoming: None,
                        lobbies: Vec::new(),
                        live_matches: Vec::new(),
                        status: "Choose a section".into(),
                    });
                    NavResult::Stay
                }
                1 => {
                    // Arcade
                    if !rom_present {
                        return NavResult::Stay;
                    }
                    *self = AppState::Playing;
                    NavResult::StartLocal { lab: false }
                }
                2 => {
                    // Lab
                    if !rom_present {
                        return NavResult::Stay;
                    }
                    *self = AppState::Menu(MenuScreen::LabMenu { cursor: 0 });
                    NavResult::Stay
                }
                3 => {
                    // Replays
                    if !rom_present {
                        return NavResult::Stay;
                    }
                    *self = AppState::Menu(MenuScreen::ReplaySelect {
                        cursor: 0,
                        entries: vec![],
                        status: None,
                    });
                    NavResult::OpenReplaySelect
                }
                4 => {
                    // Profile
                    *self = AppState::Menu(MenuScreen::Profile {
                        state: ProfileScreenState::Loading,
                    });
                    NavResult::OpenProfile
                }
                5 => {
                    // Controls
                    *self = AppState::Menu(MenuScreen::Controls {
                        cursor: 0,
                        player: Player::P1,
                    });
                    NavResult::Stay
                }
                6 => {
                    // Settings
                    *self = AppState::Menu(MenuScreen::Settings {
                        cursor: 0,
                        player_username: String::new(),
                        stats_email: String::new(),
                        discord_connected: false,
                        discord_rpc_enabled: false,
                        fullscreen: false,
                        volume_percent: 100,
                        audio_buffer: AudioBuffer::Stable,
                        video_filter: VideoFilter::Sharp,
                        crt_corner_bend: true,
                        aspect_mode: AspectMode::Fit,
                        scorebar_style: ScorebarStyle::Centered,
                        input_delay: 3,
                        render_profile: RenderProfile::Auto,
                        runahead: true,
                        runahead_online: false,
                    });
                    NavResult::OpenSettings
                }
                7 => {
                    // About
                    *self = AppState::Menu(MenuScreen::About);
                    NavResult::Stay
                }
                8 => NavResult::Quit,
                _ => NavResult::Stay,
            },
            AppState::Menu(MenuScreen::OnlineHub {
                tab,
                focus,
                cursor,
                mut challenge_format,
                osk_row,
                osk_col,
                challenge_pick,
                incoming,
                presence,
                lobbies,
                live_matches,
                ..
            }) => {
                // An incoming challenge prompt takes priority: Accept it.
                if let Some(ch) = incoming {
                    if let AppState::Menu(MenuScreen::OnlineHub { incoming, .. }) = self {
                        *incoming = None;
                    }
                    return NavResult::AcceptChallenge(ch.challenge_id);
                }
                // From the rail, Enter dives into the section content.
                if focus == HubFocus::Rail {
                    if let AppState::Menu(MenuScreen::OnlineHub { focus, .. }) = self {
                        *focus = HubFocus::Content;
                    }
                    return NavResult::Stay;
                }
                match tab {
                    OnlineTab::Players => {
                        if presence.is_empty() {
                            return NavResult::Stay;
                        }
                        match challenge_pick {
                            None => {
                                // Open the format chooser at the global format.
                                let idx = ChallengeFormat::ALL
                                    .iter()
                                    .position(|f| *f == challenge_format)
                                    .unwrap_or(0);
                                if let AppState::Menu(MenuScreen::OnlineHub {
                                    challenge_pick,
                                    ..
                                }) = self
                                {
                                    *challenge_pick = Some(idx);
                                }
                                NavResult::Stay
                            }
                            Some(fmt_idx) => {
                                let fmt = ChallengeFormat::ALL
                                    [fmt_idx.min(ChallengeFormat::ALL.len() - 1)];
                                let target = presence.get(cursor).map(|u| u.player_id.clone());
                                if let AppState::Menu(MenuScreen::OnlineHub {
                                    challenge_pick,
                                    ..
                                }) = self
                                {
                                    *challenge_pick = None;
                                }
                                match target {
                                    Some(id) => NavResult::SendChallenge(id, fmt),
                                    None => NavResult::Stay,
                                }
                            }
                        }
                    }
                    OnlineTab::Play => {
                        if cursor == 0 {
                            challenge_format = challenge_format.next();
                            if let AppState::Menu(MenuScreen::OnlineHub {
                                challenge_format: format,
                                ..
                            }) = self
                            {
                                *format = challenge_format;
                            }
                            NavResult::Stay
                        } else {
                            NavResult::OpenUsernameEntry
                        }
                    }
                    OnlineTab::Chat => {
                        // Accept presses the selected on-screen-keyboard key.
                        // (Physical Enter sends via a dedicated handler in
                        // main.rs, so keyboard users aren't affected.)
                        match osk_key_at(osk_row, osk_col) {
                            OskKey::Send => {
                                return self.send_chat_draft();
                            }
                            OskKey::Backspace => self.osk_edit_draft(None),
                            OskKey::Space => self.osk_edit_draft(Some(' ')),
                            OskKey::Char(c) => self.osk_edit_draft(Some(c)),
                        }
                        NavResult::Stay
                    }
                    OnlineTab::Lobbies => match cursor {
                        0 => NavResult::CreateLobby(challenge_format, false),
                        1 => NavResult::CreateLobby(challenge_format, true),
                        2 => NavResult::OpenJoinCode,
                        n => {
                            if let Some(lobby) = lobbies.get(n - 3) {
                                NavResult::JoinLobby(lobby.id.clone())
                            } else {
                                NavResult::Stay
                            }
                        }
                    },
                    OnlineTab::Watch => {
                        if let Some(m) = live_matches.get(cursor) {
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
                }
            }
            AppState::Menu(MenuScreen::LabMenu { cursor }) => match cursor {
                0 => {
                    if !rom_present {
                        return NavResult::Stay;
                    }
                    *self = AppState::Playing;
                    NavResult::StartLocal { lab: true }
                }
                1 => {
                    if !rom_present {
                        return NavResult::Stay;
                    }
                    *self = AppState::Menu(MenuScreen::GhostSelect {
                        cursor: 0,
                        entries: vec![],
                        download_status: None,
                    });
                    NavResult::OpenGhostSelect
                }
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
            AppState::Menu(MenuScreen::Lobby { id, view, .. }) => match view {
                Some(v) => {
                    if v.ready_check.as_ref().map_or(false, |rc| rc.you_are_challenger) {
                        // You're the challenger in a pending match — confirm ready.
                        NavResult::ReadyLobby(id)
                    } else {
                        let queued = v.your_queued || v.your_position.is_some();
                        // Toggle: queued -> spectate, spectating -> join queue.
                        NavResult::SetLobbyQueue(id, queued)
                    }
                }
                None => NavResult::Stay,
            },
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
            AppState::Menu(MenuScreen::ReplaySelect {
                cursor, entries, ..
            }) => {
                if let Some(entry) = entries.get(cursor) {
                    if let Some(url) = &entry.remote_url {
                        NavResult::LoadRemoteReplay(url.clone())
                    } else {
                        NavResult::LoadReplay(entry.path.clone())
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
            AppState::Menu(MenuScreen::MatchUsername {
                value, checking, ..
            }) => {
                if checking {
                    NavResult::Stay
                } else {
                    NavResult::SubmitUsername(value)
                }
            }
            AppState::Menu(MenuScreen::Settings { cursor, .. }) => match cursor {
                0 => NavResult::EditText(EditField::Username, "Username".into()),
                1 => NavResult::EditText(EditField::StatsEmail, "Stats Email".into()),
                2 => NavResult::ConnectDiscord,
                3 => NavResult::ToggleDiscordRpc,
                4 => NavResult::ToggleFullscreen,
                5 => NavResult::AdjustVolume(10),
                6 => NavResult::CycleAudioBuffer(1),
                7 => NavResult::CycleVideoFilter(1),
                8 => NavResult::ToggleCrtGlass,
                9 => NavResult::CycleAspectMode(1),
                10 => NavResult::CycleScorebarStyle(1),
                11 => NavResult::AdjustInputDelay(1),
                12 => NavResult::LaunchDoctor,
                13 => NavResult::OpenClipsFolder,
                14 => NavResult::OpenLogsFolder,
                15 => NavResult::CycleRenderProfile(1),
                16 => NavResult::ToggleRunahead,
                17 => NavResult::ToggleRunaheadOnline,
                _ => NavResult::Stay,
            },
            AppState::Menu(MenuScreen::TextEdit { field, value, .. }) => {
                NavResult::CommitText(field, value)
            }
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

    pub fn nav_back(&mut self, new_ui: bool) {
        // In the Online hub, Back closes the format chooser, then returns from
        // content to the rail, then from the rail out to the main menu. (The
        // incoming-challenge modal's decline is handled in main.rs, before
        // nav_back, because it needs to fire a network call.)
        if let AppState::Menu(MenuScreen::OnlineHub {
            focus,
            challenge_pick,
            ..
        }) = self
        {
            if challenge_pick.is_some() {
                *challenge_pick = None;
                return;
            }
            if *focus == HubFocus::Content {
                *focus = HubFocus::Rail;
                return;
            }
            *self = main_menu_state(new_ui);
            return;
        }
        match self {
            AppState::Menu(MenuScreen::Controls { .. })
            | AppState::Menu(MenuScreen::LabMenu { .. })
            | AppState::Menu(MenuScreen::About)
            | AppState::Menu(MenuScreen::TestIp { .. })
            | AppState::Menu(MenuScreen::TestResult { .. })
            | AppState::Menu(MenuScreen::SessionEnded { .. })
            | AppState::Menu(MenuScreen::Matchmaking { .. })
            | AppState::Menu(MenuScreen::MatchUsername { .. })
            | AppState::Menu(MenuScreen::GhostSelect { .. })
            | AppState::Menu(MenuScreen::ReplaySelect { .. })
            | AppState::Menu(MenuScreen::Profile { .. })
            | AppState::Menu(MenuScreen::Leaderboard { .. })
            | AppState::Menu(MenuScreen::Settings { .. })
            | AppState::Menu(MenuScreen::Training { .. })
            | AppState::Menu(MenuScreen::LiveMatches { .. })
            | AppState::Menu(MenuScreen::Spectate { .. }) => {
                *self = main_menu_state(new_ui);
            }
            AppState::Menu(MenuScreen::TextEdit { came_from, .. }) => {
                *self = AppState::Menu((**came_from).clone());
            }
            _ => {}
        }
    }

    pub fn finish_rebind(&mut self) {
        if let AppState::Rebinding { came_from, .. } = self.clone() {
            *self = AppState::Menu(came_from);
        }
    }

    /// Append typed text to the active menu editor. No-op if nothing is editing.
    pub fn text_input(&mut self, s: &str) {
        if let AppState::Menu(MenuScreen::OnlineHub {
            tab: OnlineTab::Chat,
            focus: HubFocus::Content,
            chat_draft,
            ..
        }) = self
        {
            for c in s.chars() {
                if !c.is_control() && chat_draft.chars().count() < 180 {
                    chat_draft.push(c);
                }
            }
        } else if let AppState::Menu(MenuScreen::TestIp {
            ip_text,
            editing: true,
        }) = self
        {
            for c in s.chars() {
                if (c.is_ascii_digit() || c == '.' || c == ':') && ip_text.len() < 24 {
                    ip_text.push(c);
                }
            }
            return;
        }

        if let AppState::Menu(MenuScreen::TextEdit { field, value, .. }) = self {
            for c in s.chars() {
                match field {
                    EditField::Username => {
                        if (c.is_ascii_alphanumeric() || c == '_' || c == '-' || c.is_whitespace())
                            && value.len() < MAX_USERNAME_LEN
                        {
                            value.push(c);
                        }
                    }
                    EditField::JoinCode => {
                        if c.is_ascii_alphanumeric() && value.len() < 6 {
                            value.extend(c.to_uppercase());
                        }
                    }
                    EditField::StatsEmail | EditField::ReplayNote { .. } => {
                        if !c.is_control() && value.len() < 96 {
                            value.push(c);
                        }
                    }
                }
            }
        } else if let AppState::Menu(MenuScreen::MatchUsername { value, .. }) = self {
            for c in s.chars() {
                if (c.is_ascii_alphanumeric() || c == '_' || c == '-' || c.is_whitespace())
                    && value.len() < MAX_USERNAME_LEN
                {
                    value.push(c);
                }
            }
        }
    }

    pub fn text_backspace(&mut self) {
        if let AppState::Menu(MenuScreen::OnlineHub {
            tab: OnlineTab::Chat,
            focus: HubFocus::Content,
            chat_draft,
            ..
        }) = self
        {
            chat_draft.pop();
        } else if let AppState::Menu(MenuScreen::TestIp {
            ip_text,
            editing: true,
        }) = self
        {
            ip_text.pop();
        } else if let AppState::Menu(MenuScreen::TextEdit { value, .. }) = self {
            value.pop();
        } else if let AppState::Menu(MenuScreen::MatchUsername { value, .. }) = self {
            value.pop();
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

#[allow(clippy::too_many_arguments)]
pub fn draw(
    state: &AppState,
    bindings: &Bindings,
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    w: i32,
    h: i32,
    rom_present: bool,
    discord_user: Option<&str>,
    main_leaderboard: &LeaderboardState,
    toast: Option<Toast<'_>>,
    osk_visible: bool,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGB(8, 8, 16));
    canvas.clear();

    match state {
        AppState::Menu(MenuScreen::Main { cursor }) => draw_main(
            canvas,
            font,
            *cursor,
            w,
            h,
            rom_present,
            discord_user,
            main_leaderboard,
        )?,
        AppState::Menu(MenuScreen::OnlineHub {
            tab,
            focus,
            cursor,
            challenge_format,
            chat_draft,
            chat,
            presence,
            chat_scroll,
            osk_row,
            osk_col,
            challenge_pick,
            incoming,
            lobbies,
            live_matches,
            status,
        }) => draw_online_hub(
            canvas,
            font,
            *tab,
            *focus,
            *cursor,
            *challenge_format,
            chat_draft,
            chat,
            presence,
            *chat_scroll,
            (*osk_row, *osk_col),
            osk_visible,
            *challenge_pick,
            incoming.as_ref(),
            lobbies,
            live_matches,
            status,
            w,
            h,
        )?,
        AppState::Menu(MenuScreen::LabMenu { cursor }) => {
            draw_lab_menu(canvas, font, *cursor, rom_present, w, h)?
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
        AppState::Menu(MenuScreen::SessionEnded { lines, replay_path }) => {
            draw_session_ended(canvas, font, lines, replay_path.is_some(), w, h)?
        }
        AppState::Menu(MenuScreen::Matchmaking { status }) => {
            draw_matchmaking(canvas, font, status, discord_user, w, h)?
        }
        AppState::Menu(MenuScreen::MatchUsername {
            value,
            status,
            checking,
        }) => draw_match_username(canvas, font, value, status, *checking, w, h)?,
        AppState::Menu(MenuScreen::GhostSelect {
            cursor,
            entries,
            download_status,
        }) => draw_ghost_select(
            canvas,
            font,
            *cursor,
            entries,
            download_status.as_deref(),
            w,
            h,
        )?,
        AppState::Menu(MenuScreen::ReplaySelect {
            cursor,
            entries,
            status,
        }) => draw_replay_select(canvas, font, *cursor, entries, status.as_deref(), w, h)?,
        AppState::Menu(MenuScreen::Profile { state }) => draw_profile(canvas, font, state, w, h)?,
        AppState::Menu(MenuScreen::Leaderboard { state }) => {
            draw_leaderboard(canvas, font, state, w, h)?
        }
        AppState::Menu(MenuScreen::Settings {
            cursor,
            player_username,
            stats_email,
            discord_connected,
            discord_rpc_enabled,
            fullscreen,
            volume_percent,
            audio_buffer,
            video_filter,
            crt_corner_bend,
            aspect_mode,
            scorebar_style,
            input_delay,
            render_profile,
            runahead,
            runahead_online,
        }) => draw_settings(
            canvas,
            font,
            *cursor,
            player_username,
            stats_email,
            *discord_connected,
            *discord_rpc_enabled,
            *fullscreen,
            *volume_percent,
            *audio_buffer,
            *video_filter,
            *crt_corner_bend,
            *aspect_mode,
            *scorebar_style,
            *input_delay,
            *render_profile,
            *runahead,
            *runahead_online,
            w,
            h,
        )?,
        AppState::Menu(MenuScreen::TextEdit {
            title,
            label,
            value,
            ..
        }) => draw_text_edit(canvas, font, title, label, value, w, h)?,
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
        AppState::Menu(MenuScreen::Lobby {
            view, status, thumb, ..
        }) => draw_lobby(canvas, font, view.as_ref(), status, thumb.as_ref(), w, h)?,
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
        // Drawn by `crate::fp_ui::draw` from main.rs instead — the caller
        // branches on `AppState::FpUi` before ever reaching this function.
        AppState::FpUi(_) => {}
    }

    draw_version_footer(canvas, font, w, h)?;
    if let Some(toast) = toast {
        draw_toast(canvas, font, &toast, w, h)?;
    }
    Ok(())
}

pub(crate) fn draw_toast(
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
    has_replay: bool,
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

    let footer = if has_replay {
        "R/Y Replay   ENTER Main Menu   ESC Main Menu"
    } else {
        "ENTER Main Menu   ESC Main Menu"
    };
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

fn draw_lobby(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    view: Option<&LobbyView>,
    status: &str,
    thumb: Option<&(Vec<u8>, u32, u32)>,
    w: i32,
    h: i32,
) -> Result<(), String> {
    let body = body_scale(h);
    let small = small_scale(h);
    let pad = (w / 24).clamp(16, 40);
    let tscale = title_scale(h);
    let title = view.map(|v| v.name.clone()).unwrap_or_else(|| "LOBBY".into());

    font.draw(canvas, &title, pad, 22, tscale, HUB_ACCENT)?;
    let line_y = 22 + 22 * tscale as i32;
    canvas.set_draw_color(Color::RGB(120, 70, 10));
    canvas.fill_rect(Rect::new(pad, line_y, (w - pad * 2) as u32, 2))?;
    let panel_w = w - pad * 2;
    let mut y = line_y + 16;

    let footer_y = h - (16 * small as i32 + 26);
    let Some(v) = view else {
        font.draw(
            canvas,
            if status.is_empty() { "Loading lobby…" } else { status },
            pad,
            y,
            small,
            HUB_DIM,
        )?;
        let footer = "ESC Leave";
        let fw = font.text_width_exact(footer, small);
        font.draw(canvas, footer, (w - fw) / 2, footer_y, small, HUB_FAINT)?;
        return Ok(());
    };

    let sub = format!(
        "{} · FT1 · {} player{}",
        if v.ranked { "Ranked" } else { "Unranked" },
        v.members.len(),
        if v.members.len() == 1 { "" } else { "s" },
    );
    font.draw(canvas, &sub, pad, y, small, HUB_DIM)?;
    y += 16 * small as i32 + 8;

    if v.private {
        let code = format!("PRIVATE — invite code:  {}", v.id);
        font.draw(canvas, &code, pad, y, body, HUB_ACCENT)?;
        y += 16 * body as i32 + 10;
    }

    // Current match panel.
    let mp_h = 22 * body as i32 + 24;
    if let Some(rc) = &v.ready_check {
        // A ready check pre-empts the now-playing panel: the next two players
        // are pairing up and the challenger must confirm within the countdown.
        let panel_col = if rc.you_are_challenger { HUB_PANEL_SEL } else { HUB_PANEL };
        draw_panel(canvas, pad, y, panel_w, mp_h, panel_col)?;
        let head = format!("READY CHECK · {}s", rc.seconds_left.max(0));
        font.draw(canvas, &head, pad + 12, y + 8, small, HUB_ACCENT)?;
        let line = format!("{}  vs  {}", rc.champion_username, rc.challenger_username);
        font.draw(canvas, &line, pad + 12, y + 10 + 14 * small as i32, body, HUB_TEXT)?;
        y += mp_h + 14;
    } else if let Some(c) = &v.current {
        // NOW PLAYING — the panel grows to fit a live match thumbnail on the
        // right (a periodic screenshot streamed from the two active players).
        let thumb_h = (h / 6).clamp(72, 132);
        let thumb_w =
            thumb_h * crate::render::LOBBY_THUMB_W as i32 / crate::render::LOBBY_THUMB_H as i32;
        let panel_h = mp_h.max(thumb_h + 16);
        draw_panel(canvas, pad, y, panel_w, panel_h, HUB_PANEL)?;
        font.draw(canvas, "NOW PLAYING", pad + 12, y + 8, small, HUB_ACCENT)?;
        let line = format!("{}  vs  {}", c.host_username, c.join_username);
        font.draw(canvas, &line, pad + 12, y + 10 + 14 * small as i32, body, HUB_TEXT)?;

        let tx_x = pad + panel_w - thumb_w - 12;
        let tx_y = y + (panel_h - thumb_h) / 2;
        canvas.set_draw_color(Color::RGB(60, 66, 90));
        canvas.fill_rect(Rect::new(
            tx_x - 2,
            tx_y - 2,
            (thumb_w + 4) as u32,
            (thumb_h + 4) as u32,
        ))?;
        if let Some((rgba, tw, th)) = thumb {
            let tc = canvas.texture_creator();
            let made = tc.create_texture_static(sdl2::pixels::PixelFormatEnum::RGBA32, *tw, *th);
            if let Ok(mut tex) = made {
                if tex.update(None, rgba, *tw as usize * 4).is_ok() {
                    canvas.copy(
                        &tex,
                        None,
                        Rect::new(tx_x, tx_y, thumb_w as u32, thumb_h as u32),
                    )?;
                }
            }
        } else {
            canvas.set_draw_color(Color::RGB(20, 22, 32));
            canvas.fill_rect(Rect::new(tx_x, tx_y, thumb_w as u32, thumb_h as u32))?;
            let wait = "live…";
            let ww = font.text_width_exact(wait, small);
            font.draw(
                canvas,
                wait,
                tx_x + (thumb_w - ww) / 2,
                tx_y + thumb_h / 2 - 6,
                small,
                HUB_FAINT,
            )?;
        }
        y += panel_h + 14;
    } else {
        draw_panel(canvas, pad, y, panel_w, mp_h, HUB_PANEL)?;
        font.draw(
            canvas,
            "Waiting for two players to queue up…",
            pad + 12,
            y + mp_h / 2 - 6,
            small,
            HUB_DIM,
        )?;
        y += mp_h + 14;
    }

    // Up-next queue.
    font.draw(canvas, "UP NEXT", pad, y, small, HUB_ACCENT)?;
    y += 16 * small as i32 + 6;
    let row_h = 16 * small as i32 + 4;
    if v.queue.is_empty() {
        font.draw(canvas, "(queue is empty)", pad + 8, y, small, HUB_FAINT)?;
    } else {
        let max_rows = ((footer_y - y - 40) / row_h).max(1) as usize;
        for (i, name) in v.queue.iter().take(max_rows).enumerate() {
            let yours = v.your_position == Some(i);
            let label = format!("{}.  {}{}", i + 1, name, if yours { "  (you)" } else { "" });
            font.draw(
                canvas,
                &label,
                pad + 8,
                y + i as i32 * row_h,
                small,
                if yours { HUB_ACCENT } else { HUB_TEXT },
            )?;
        }
    }

    // Your status line above the footer.
    let challenger_ready = v
        .ready_check
        .as_ref()
        .map_or(false, |rc| rc.you_are_challenger);
    let your = if challenger_ready {
        let secs = v.ready_check.as_ref().map_or(0, |rc| rc.seconds_left.max(0));
        format!("You're up! Confirm ready — {}s", secs)
    } else if v.your_turn {
        "Your turn — get ready!".to_string()
    } else if v.ready_check.is_some() {
        "Next match pairing up…".to_string()
    } else if let Some(p) = v.your_position {
        format!("You're #{} in the queue", p + 1)
    } else if v.your_queued {
        "You're queued to play".to_string()
    } else {
        "You're spectating".to_string()
    };
    let your_col = if challenger_ready { HUB_ACCENT } else { HUB_DIM };
    font.draw(canvas, &your, pad, footer_y - 18 * small as i32 - 6, small, your_col)?;

    let footer = if challenger_ready {
        "ENTER Ready!    ESC Leave"
    } else if v.your_queued || v.your_position.is_some() {
        "ENTER Spectate    ESC Leave"
    } else {
        "ENTER Join queue    ESC Leave"
    };
    let fw = font.text_width_exact(footer, small);
    font.draw(canvas, footer, (w - fw) / 2, footer_y, small, HUB_FAINT)?;
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
    ((h / 260).max(2) as u32).min(4)
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

fn draw_logged_in_as(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    discord_user: Option<&str>,
    w: i32,
    h: i32,
) -> Result<(), String> {
    if let Some(name) = discord_user {
        let label = format!("Logged in as {name}");
        let scale = small_scale(h);
        let tw = font.text_width_exact(&label, scale);
        font.draw(
            canvas,
            &label,
            w - tw - 8,
            h - 54,
            scale,
            Color::RGB(88, 130, 200),
        )?;
    }
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
    leaderboard: &LeaderboardState,
) -> Result<(), String> {
    // Left-aligned, oversized wordmark (bigger than the shared centered
    // draw_title used elsewhere) to anchor the home screen.
    let pad = (w / 24).clamp(16, 40);
    let title_sc = (title_scale(h) + 1).min(5);
    let title_y = 22;
    font.draw(canvas, "FREEPLAY", pad, title_y, title_sc, Color::RGB(255, 200, 0))?;
    let line_y = title_y + 24 * title_sc as i32;
    canvas.set_draw_color(Color::RGB(100, 50, 0));
    canvas.fill_rect(Rect::new(pad, line_y, (w - pad * 2) as u32, 2))?;

    let item_scale = body_scale(h).max(2);
    let line_h = (44 * item_scale as i32) / 2;
    let block_h = MAIN_ITEMS.len() as i32 * line_h;
    let start_y = (h - block_h) / 2 + 10;
    let show_sidebar = w >= 760;
    let sidebar_w = if show_sidebar {
        ((w * 42) / 100).clamp(320, 520)
    } else {
        0
    };
    let sidebar_gap = if show_sidebar { 42 } else { 0 };
    let sidebar_x = if show_sidebar { w - sidebar_w - 56 } else { w };
    let menu_area_w = if show_sidebar {
        sidebar_x - sidebar_gap
    } else {
        w
    };

    let widest = MAIN_ITEMS
        .iter()
        .map(|label| font.text_width_exact(label, item_scale))
        .max()
        .unwrap_or(0);
    let menu_x = if show_sidebar {
        ((menu_area_w - widest) / 2).max(28)
    } else {
        ((w - widest) / 2).max(24)
    };
    for (i, label) in MAIN_ITEMS.iter().enumerate() {
        let x = menu_x;
        let y = start_y + i as i32 * line_h;
        let disabled = matches!(i, 0 | 1 | 2 | 3) && !rom_present;
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
        let ready_line =
            "No valid .zip found in the roms folder (detected automatically) - Shift+D runs Doctor";
        let s = small_scale(h);
        let ready_w = font.text_width_exact(ready_line, s);
        let panel_w = (ready_w + 18).min(w - 56).max(ready_w + 8);
        let panel_h = 18 * s as i32;
        let panel_x = (w - panel_w) / 2;
        let panel_y = (start_y + block_h + 12).min(h - 78);
        draw_panel(
            canvas,
            panel_x,
            panel_y,
            panel_w,
            panel_h,
            Color::RGBA(14, 16, 24, 210),
        )?;
        font.draw(
            canvas,
            ready_line,
            panel_x + (panel_w - ready_w) / 2,
            panel_y + 3,
            s,
            Color::RGB(235, 110, 90),
        )?;
    }

    let footer = "UP/DN Select   ENTER Confirm   Shift+D Doctor";
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

    draw_logged_in_as(canvas, font, discord_user, w, h)?;

    if show_sidebar {
        let sidebar_y = (start_y - 16).max(84);
        draw_main_leaderboard(
            canvas,
            font,
            leaderboard,
            sidebar_x,
            sidebar_y,
            sidebar_w,
            h,
        )?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_online_hub(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    tab: OnlineTab,
    focus: HubFocus,
    cursor: usize,
    challenge_format: ChallengeFormat,
    chat_draft: &str,
    chat: &[LobbyChatMessage],
    presence: &[LobbyUser],
    chat_scroll: usize,
    osk: (usize, usize),
    osk_visible: bool,
    challenge_pick: Option<usize>,
    incoming: Option<&IncomingChallenge>,
    lobbies: &[LobbyPreview],
    live_matches: &[LiveMatch],
    status: &str,
    w: i32,
    h: i32,
) -> Result<(), String> {
    hits_reset();
    let body = body_scale(h);
    let small = small_scale(h);
    let content_focus = focus == HubFocus::Content;
    let pad = (w / 24).clamp(16, 40);

    // Left-aligned title with an underline spanning the content width.
    let tscale = title_scale(h);
    font.draw(canvas, "ONLINE", pad, 22, tscale, HUB_ACCENT)?;
    let line_y = 22 + 22 * tscale as i32;
    canvas.set_draw_color(Color::RGB(120, 70, 10));
    canvas.fill_rect(Rect::new(pad, line_y, (w - pad * 2) as u32, 2))?;
    let top = line_y + 16;
    let footer_h = 16 * small as i32 + 28;
    let bottom = h - footer_h;
    let area_h = bottom - top;

    // ── Left nav rail ──────────────────────────────────────────────────────
    let rail_w = (w / 5).clamp(120, 220);
    draw_rail(canvas, font, tab, focus, pad, top, rail_w, area_h, body)?;

    // ── Content panel ──────────────────────────────────────────────────────
    let cx = pad + rail_w + 14;
    let cw = w - cx - pad;
    draw_panel(canvas, cx, top, cw, area_h, HUB_PANEL)?;
    let x = cx + 16; // inner left
    let content_w = cw - 32; // inner width

    // Section header + status line.
    font.draw(canvas, tab.label(), x, top + 14, body, HUB_ACCENT)?;
    let status_line = fit_line(font, status, small, content_w);
    font.draw(
        canvas,
        &status_line,
        x,
        top + 16 + 16 * body as i32,
        small,
        HUB_DIM,
    )?;
    let y = top + 30 + 20 * body as i32; // content start

    match tab {
        OnlineTab::Players => {
            if presence.is_empty() {
                font.draw(canvas, "No players online right now.", x, y, body, HUB_TEXT)?;
                font.draw(
                    canvas,
                    "Open the Chat section to join the lobby and appear here.",
                    x + 10,
                    y + 16 * body as i32,
                    small,
                    HUB_DIM,
                )?;
            } else {
                font.draw(canvas, "Select a player, then pick a format to challenge.", x, y, small, HUB_DIM)?;
                let row_gap = 30;
                let list_y = y + 22;
                let max_rows = ((bottom - list_y) / row_gap).max(1) as usize;
                for (i, u) in presence.iter().take(max_rows).enumerate() {
                    let row_y = list_y + i as i32 * row_gap;
                    let sel = content_focus && cursor == i;
                    draw_online_row(canvas, font, &user_label(u), sel, x, row_y, content_w, body)?;
                    record_presence_hit(x - 8, row_y - 5, content_w + 12, row_gap, i);
                    if !u.status.is_empty() {
                        let sw = font.text_width_exact(&u.status, small);
                        font.draw(canvas, &u.status, x + content_w - sw - 6, row_y + 2, small, HUB_FAINT)?;
                    }
                }
            }
        }
        OnlineTab::Play => {
            let fmt_line = format!("Format:  {}", challenge_format.label());
            draw_online_row(canvas, font, &fmt_line, content_focus && cursor == 0, x, y, content_w, body)?;
            let sub = format!(
                "{}  —  {} game{}   (Left/Right or Enter to change)",
                if challenge_format.ranked() { "Ranked" } else { "Unranked" },
                challenge_format.match_limit(),
                if challenge_format.match_limit() == 1 { "" } else { "s" },
            );
            font.draw(canvas, &sub, x + 10, y + 14 * body as i32, small, HUB_DIM)?;

            let find_y = y + 56;
            draw_online_row(canvas, font, "Find Match", content_focus && cursor == 1, x, find_y, content_w, body)?;
            font.draw(
                canvas,
                "Queues for an opponent on the same ROM and version.",
                x + 10,
                find_y + 14 * body as i32,
                small,
                HUB_DIM,
            )?;
        }
        OnlineTab::Chat => {
            let line_h = (16 * small as i32).max(15);
            let input_h = 14 * small as i32 + 14;
            let key_h = 14 * small as i32 + 8;
            let osk_gap = 4;
            // The on-screen keyboard only appears when the player is driving the
            // UI with a controller (osk_visible); keyboard users just type.
            let show_osk = content_focus && osk_visible;
            let osk_h = if show_osk {
                OSK_ROWS as i32 * (key_h + osk_gap) + 4
            } else {
                0
            };
            let strip_h = 12 * small as i32 + 8;
            let log_top = y;
            let input_y = bottom - osk_h - if show_osk { 8 } else { 4 } - input_h;
            let strip_y = input_y - strip_h - 6;
            let log_bottom = strip_y - 6;
            let log_h = (log_bottom - log_top).max(line_h);

            let presence_w = (content_w / 3).clamp(96, 240);
            let log_w = content_w - presence_w - 14;

            // Message log (newest at the bottom, honoring chat_scroll).
            if chat.is_empty() {
                font.draw(canvas, "No messages yet — say hi!", x, log_top + 4, small, HUB_FAINT)?;
            } else {
                let max_lines = (log_h / line_h).max(1) as usize;
                let total = chat.len();
                let scroll = chat_scroll.min(total.saturating_sub(1));
                let end = total - scroll;
                let start = end.saturating_sub(max_lines);
                let mut ly = log_top + 4;
                for msg in &chat[start..end] {
                    let nick = format!("{}:", msg.username);
                    font.draw(canvas, &nick, x, ly, small, nick_color(&msg.username))?;
                    let nw = font.text_width_exact(&nick, small) + 6;
                    let text = fit_line(font, &msg.message, small, log_w - nw);
                    font.draw(canvas, &text, x + nw, ly, small, HUB_TEXT)?;
                    ly += line_h;
                }
                if scroll > 0 {
                    font.draw(canvas, "\u{25B2} more", x + log_w - 60, log_bottom - line_h, small, HUB_FAINT)?;
                }
            }

            // Presence panel.
            let px = x + log_w + 14;
            draw_panel(canvas, px, log_top, presence_w, log_h, HUB_RAIL_BG)?;
            font.draw(canvas, &format!("Online ({})", presence.len()), px + 8, log_top + 6, small, HUB_ACCENT)?;
            let mut uy = log_top + 6 + line_h + 2;
            if presence.is_empty() {
                font.draw(canvas, "(nobody yet)", px + 8, uy, small, HUB_FAINT)?;
            } else {
                let max_users = ((log_h - line_h - 8) / line_h).max(1) as usize;
                for (i, u) in presence.iter().take(max_users).enumerate() {
                    let name = fit_line(font, &user_label(u), small, presence_w - 16);
                    font.draw(canvas, &name, px + 8, uy, small, nick_color(&u.username))?;
                    record_presence_hit(px, uy - 2, presence_w, line_h, i);
                    uy += line_h;
                }
            }

            // Quick-phrase chips — click to drop a common message into the bar.
            font.draw(canvas, "QUICK:", x, strip_y + 3, small, HUB_FAINT)?;
            let mut chip_x = x + font.text_width_exact("QUICK:", small) + 10;
            for (i, ph) in QUICK_PHRASES.iter().enumerate() {
                let cw = font.text_width_exact(ph, small) + 16;
                if chip_x + cw > x + content_w {
                    break;
                }
                draw_panel(canvas, chip_x, strip_y, cw, strip_h, HUB_PANEL)?;
                font.draw(canvas, ph, chip_x + 8, strip_y + 3, small, HUB_TEXT)?;
                record_phrase_hit(chip_x, strip_y, cw, strip_h, i);
                chip_x += cw + 6;
            }

            // Input box. When focused (you're "in" the chat bar) make it clearly
            // active: brighter fill, an accent border + left bar, and a caret.
            let box_color = if content_focus {
                Color::RGBA(44, 52, 84, 250)
            } else {
                HUB_RAIL_BG
            };
            draw_panel(canvas, x, input_y, content_w, input_h, box_color)?;
            if content_focus {
                canvas.set_draw_color(HUB_ACCENT);
                canvas.draw_rect(Rect::new(x, input_y, content_w as u32, input_h as u32))?;
                canvas.fill_rect(Rect::new(x, input_y, 3, input_h as u32))?;
            }
            let (draft, draft_color) = if chat_draft.is_empty() {
                ("Type a message…", HUB_FAINT)
            } else {
                (chat_draft, Color::RGB(236, 240, 255))
            };
            let draft = fit_line(font, draft, small, content_w - 24);
            font.draw(canvas, &draft, x + 10, input_y + 4, small, draft_color)?;
            if content_focus {
                // Blinking caret after the current text.
                let caret_x = x + 10 + font.text_width_exact(&draft, small) + 2;
                let blink = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_millis() < 500)
                    .unwrap_or(true);
                if blink {
                    canvas.set_draw_color(HUB_ACCENT);
                    canvas.fill_rect(Rect::new(caret_x, input_y + 4, 2, (12 * small) as u32))?;
                }
            }

            // On-screen keyboard for controller players.
            if show_osk {
                draw_osk(
                    canvas,
                    font,
                    osk,
                    x,
                    input_y + input_h + 6,
                    content_w,
                    small,
                    key_h,
                    osk_gap,
                )?;
            }
        }
        OnlineTab::Lobbies => {
            let act_gap = 26;
            // King-of-the-hill lobbies are always FT1 (winner stays). The shared
            // format chooser only decides ranked vs unranked here.
            let rank = if challenge_format.ranked() { "Ranked" } else { "Unranked" };
            let pub_label = format!("+  Create Public Lobby  ({rank} · FT1)");
            draw_online_row(canvas, font, &pub_label, content_focus && cursor == 0, x, y, content_w, body)?;
            let priv_label = format!("+  Create Private Lobby  ({rank} · FT1)");
            draw_online_row(canvas, font, &priv_label, content_focus && cursor == 1, x, y + act_gap, content_w, body)?;
            draw_online_row(canvas, font, "Join by Invite Code", content_focus && cursor == 2, x, y + act_gap * 2, content_w, body)?;
            let list_y = y + act_gap * 3 + 12;
            if lobbies.is_empty() {
                font.draw(
                    canvas,
                    "No public lobbies right now. Create one to get started.",
                    x + 10,
                    list_y,
                    small,
                    HUB_DIM,
                )?;
            } else {
                let row_gap = 30;
                let max_rows = ((bottom - list_y) / row_gap).max(1) as usize;
                for (i, lobby) in lobbies.iter().take(max_rows).enumerate() {
                    let row_y = list_y + i as i32 * row_gap;
                    let sel = content_focus && cursor == i + 3;
                    let label = format!("{}   FT1   {}P", lobby.name, lobby.players);
                    draw_online_row(canvas, font, &label, sel, x, row_y, content_w, body)?;
                    let meta = format!("{} · {}", lobby.host, lobby.status);
                    let mw = font.text_width_exact(&meta, small);
                    font.draw(canvas, &meta, x + content_w - mw - 6, row_y + 2, small, HUB_FAINT)?;
                }
            }
        }
        OnlineTab::Watch => {
            if live_matches.is_empty() {
                font.draw(canvas, "No live matches right now.", x, y, body, HUB_TEXT)?;
                font.draw(
                    canvas,
                    "Refreshes automatically — Enter to refresh now.",
                    x + 10,
                    y + 16 * body as i32,
                    small,
                    HUB_DIM,
                )?;
            } else {
                let row_gap = 34;
                let max_rows = ((bottom - y) / row_gap).max(1) as usize;
                for (i, m) in live_matches.iter().take(max_rows).enumerate() {
                    let row_y = y + i as i32 * row_gap;
                    let score = format!("{}-{}", m.p1_score, m.p2_score);
                    let score_w = font.text_width_exact(&score, body);
                    let names = fit_line(
                        font,
                        &format!("{} vs {}", m.p1_name, m.p2_name),
                        body,
                        content_w - score_w - 42,
                    );
                    draw_online_row(canvas, font, &names, content_focus && cursor == i, x, row_y, content_w, body)?;
                    font.draw(canvas, &score, x + content_w - score_w - 6, row_y, body, HUB_ACCENT)?;
                }
            }
        }
    }

    // Format chooser overlay (challenging a selected player).
    if let Some(pick) = challenge_pick {
        if let Some(target) = presence.get(cursor) {
            draw_format_chooser(canvas, font, &target.username, pick, body, w, h)?;
        }
    }

    // Incoming-challenge modal takes the whole screen's attention.
    if let Some(ch) = incoming {
        draw_incoming_challenge(canvas, font, ch, body, small, w, h)?;
    }

    // Focus-aware footer hints.
    let footer = if incoming.is_some() {
        "ENTER Accept    ESC Decline"
    } else if challenge_pick.is_some() {
        "UP/DOWN Format    ENTER Challenge    ESC Cancel"
    } else {
        match focus {
            HubFocus::Rail => "UP/DOWN Section    RIGHT/ENTER Open    ESC Back",
            HubFocus::Content => match tab {
                OnlineTab::Chat => {
                    if osk_visible {
                        "D-pad keys   A press key   ENTER send   ESC back"
                    } else {
                        "Type a message    ENTER Send    ESC Back"
                    }
                }
                OnlineTab::Play => "UP/DOWN Move    ENTER Select/Change    LEFT Back",
                OnlineTab::Players => "UP/DOWN Player    ENTER Challenge    LEFT Back",
                _ => "UP/DOWN Move    ENTER Select    LEFT Back",
            },
        }
    };
    let fw = font.text_width_exact(footer, small);
    font.draw(canvas, footer, (w - fw) / 2, h - footer_h + 8, small, HUB_FAINT)?;
    Ok(())
}

/// Small centered popup listing the four challenge formats.
fn draw_format_chooser(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    target: &str,
    pick: usize,
    body: u32,
    w: i32,
    h: i32,
) -> Result<(), String> {
    let bw = (w / 3).clamp(220, 360);
    let row_h = 14 * body as i32 + 10;
    let bh = row_h * ChallengeFormat::ALL.len() as i32 + 40;
    let bx = (w - bw) / 2;
    let by = (h - bh) / 2;
    draw_panel(canvas, bx, by, bw, bh, Color::RGBA(20, 24, 38, 248))?;
    let title = format!("Challenge {target}");
    let title = fit_line(font, &title, body, bw - 24);
    font.draw(canvas, &title, bx + 14, by + 12, body, HUB_ACCENT)?;
    for (i, fmt) in ChallengeFormat::ALL.iter().enumerate() {
        let ry = by + 36 + i as i32 * row_h;
        let selected = i == pick;
        record_format_hit(bx + 8, ry - 4, bw - 16, row_h, i);
        if selected {
            canvas.set_draw_color(HUB_PANEL_SEL);
            canvas.fill_rect(Rect::new(bx + 8, ry - 4, (bw - 16) as u32, row_h as u32))?;
            canvas.set_draw_color(HUB_ACCENT);
            canvas.fill_rect(Rect::new(bx + 8, ry - 4, 3, row_h as u32))?;
        }
        font.draw(
            canvas,
            fmt.label(),
            bx + 18,
            ry,
            body,
            if selected { HUB_ACCENT } else { HUB_TEXT },
        )?;
    }
    Ok(())
}

/// Centered modal prompting to accept or decline an incoming challenge.
fn draw_incoming_challenge(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    ch: &IncomingChallenge,
    body: u32,
    small: u32,
    w: i32,
    h: i32,
) -> Result<(), String> {
    // Dim the whole screen.
    canvas.set_draw_color(Color::RGBA(0, 0, 0, 170));
    canvas.fill_rect(Rect::new(0, 0, w as u32, h as u32))?;
    let bw = (w / 3).clamp(260, 420);
    let bh = 30 * body as i32 + 60;
    let bx = (w - bw) / 2;
    let by = (h - bh) / 2;
    draw_panel(canvas, bx, by, bw, bh, Color::RGBA(22, 26, 42, 252))?;
    font.draw(canvas, "Challenge!", bx + 16, by + 14, body, HUB_ACCENT)?;
    let fmt = crate::matchmaking::lobby_format_label(ch.format);
    let line = format!("{} wants to play  ({})", ch.from_username, fmt);
    let line = fit_line(font, &line, small, bw - 32);
    font.draw(canvas, &line, bx + 16, by + 16 + 18 * body as i32, small, HUB_TEXT)?;
    font.draw(
        canvas,
        "ENTER Accept      ESC Decline",
        bx + 16,
        by + bh - 14 * small as i32 - 12,
        small,
        HUB_DIM,
    )?;
    Ok(())
}

/// Vertical section rail down the left edge of the Online hub.
fn draw_rail(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    tab: OnlineTab,
    focus: HubFocus,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    body: u32,
) -> Result<(), String> {
    draw_panel(canvas, x, y, w, h, HUB_RAIL_BG)?;
    let item_h = 40;
    let mut iy = y + 16;
    for item in OnlineTab::ALL {
        let selected = item == tab;
        if selected {
            let bg = if focus == HubFocus::Rail {
                HUB_PANEL_SEL
            } else {
                Color::RGBA(26, 31, 50, 235)
            };
            canvas.set_draw_color(bg);
            canvas.fill_rect(Rect::new(x + 6, iy - 8, (w - 12) as u32, (item_h - 8) as u32))?;
            canvas.set_draw_color(HUB_ACCENT);
            canvas.fill_rect(Rect::new(x + 6, iy - 8, 3, (item_h - 8) as u32))?;
        }
        let color = if selected {
            if focus == HubFocus::Rail {
                HUB_ACCENT
            } else {
                Color::RGB(222, 212, 182)
            }
        } else {
            HUB_DIM
        };
        font.draw(canvas, item.label(), x + 18, iy, body, color)?;
        iy += item_h;
    }
    Ok(())
}

fn draw_online_row(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    label: &str,
    selected: bool,
    x: i32,
    y: i32,
    w: i32,
    scale: u32,
) -> Result<(), String> {
    if selected {
        let row_h = 12 * scale as i32 + 10;
        canvas.set_draw_color(HUB_PANEL_SEL);
        canvas.fill_rect(Rect::new(x - 8, y - 5, (w + 12) as u32, row_h as u32))?;
        canvas.set_draw_color(HUB_ACCENT);
        canvas.fill_rect(Rect::new(x - 8, y - 5, 3, row_h as u32))?;
    }
    let label = fit_line(font, label, scale, w - 16);
    font.draw(
        canvas,
        &label,
        x,
        y,
        scale,
        if selected {
            HUB_TEXT
        } else {
            Color::RGB(170, 178, 196)
        },
    )?;
    Ok(())
}

/// Display label for an on-screen-keyboard key.
fn osk_key_label(row: usize, col: usize) -> String {
    match osk_key_at(row, col) {
        OskKey::Char(c) => c.to_string(),
        OskKey::Space => "SPACE".into(),
        OskKey::Backspace => "DEL".into(),
        OskKey::Send => "SEND".into(),
    }
}

/// Draw the d-pad-navigable on-screen keyboard. `sel` is the highlighted
/// (row, col).
#[allow(clippy::too_many_arguments)]
fn draw_osk(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    sel: (usize, usize),
    x: i32,
    y: i32,
    w: i32,
    small: u32,
    key_h: i32,
    gap: i32,
) -> Result<(), String> {
    // Character keys sized to fit the widest row (10 keys); the action row
    // (SPACE/DEL/SEND) spans the same width in three wide keys.
    let kw = ((w - 9 * gap) / 10).clamp(16, 56);
    for row in 0..OSK_ROWS {
        let row_y = y + row as i32 * (key_h + gap);
        let len = osk_row_len(row);
        if row < 4 {
            for col in 0..len {
                let kx = x + col as i32 * (kw + gap);
                draw_osk_key(canvas, font, &osk_key_label(row, col), sel == (row, col), kx, row_y, kw, key_h, small)?;
            }
        } else {
            let aw = (w - 2 * gap) / 3;
            for col in 0..len {
                let kx = x + col as i32 * (aw + gap);
                draw_osk_key(canvas, font, &osk_key_label(row, col), sel == (row, col), kx, row_y, aw, key_h, small)?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_osk_key(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    label: &str,
    selected: bool,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    small: u32,
) -> Result<(), String> {
    draw_panel(
        canvas,
        x,
        y,
        w,
        h,
        if selected { HUB_PANEL_SEL } else { HUB_PANEL },
    )?;
    if selected {
        canvas.set_draw_color(HUB_ACCENT);
        canvas.fill_rect(Rect::new(x, y, w as u32, 2))?;
    }
    let tw = font.text_width_exact(label, small);
    font.draw(
        canvas,
        label,
        x + (w - tw) / 2,
        y + (h - 8 * small as i32) / 2,
        small,
        if selected { HUB_ACCENT } else { HUB_TEXT },
    )?;
    Ok(())
}

fn draw_lab_menu(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    cursor: usize,
    rom_present: bool,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "LAB", w, h)?;

    let scale = body_scale(h).max(2);
    let small = small_scale(h);
    let line_h = (44 * scale as i32) / 2;
    let block_h = LAB_MENU_ITEMS.len() as i32 * line_h;
    let start_y = (h - block_h) / 2 + 8;
    let widest = LAB_MENU_ITEMS
        .iter()
        .map(|label| font.text_width_exact(label, scale))
        .max()
        .unwrap_or(0);
    let x = ((w - widest) / 2).max(24);

    for (i, label) in LAB_MENU_ITEMS.iter().enumerate() {
        let y = start_y + i as i32 * line_h;
        let colour = if !rom_present {
            Color::RGB(60, 60, 60)
        } else if i == cursor {
            Color::RGB(255, 255, 255)
        } else {
            Color::RGB(135, 140, 160)
        };
        if i == cursor && rom_present {
            let caret_w = font.text_width_exact("> ", scale);
            font.draw(canvas, ">", x - caret_w, y, scale, colour)?;
        }
        font.draw(canvas, label, x, y, scale, colour)?;
    }

    let note = "Lab tools, drones, and replay review";
    let note_w = font.text_width_exact(note, small);
    font.draw(
        canvas,
        note,
        (w - note_w) / 2,
        start_y + block_h + 18,
        small,
        Color::RGB(130, 140, 165),
    )?;

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

fn draw_main_leaderboard(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    state: &LeaderboardState,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<(), String> {
    let small = small_scale(h);
    let heading_scale = (small + 1).min(3);
    let row_h = 18 * small as i32;
    let visible_rows = match state {
        LeaderboardState::Loaded(rows) => rows.len().clamp(4, 10),
        _ => 4,
    } as i32;
    let panel_h = (58 + row_h * visible_rows).min(h - y - 58).max(150);
    draw_panel(canvas, x, y, w, panel_h, Color::RGBA(14, 16, 24, 225))?;
    draw_stroked_rect(canvas, x, y, w, panel_h, Color::RGB(80, 140, 220), 3)?;

    font.draw(
        canvas,
        "Leaderboards",
        x + 12,
        y + 10,
        heading_scale,
        Color::RGB(255, 200, 80),
    )?;

    match state {
        LeaderboardState::Loading => {
            draw_sidebar_note(canvas, font, "Loading stats...", x, y + 48, w, small)?;
        }
        LeaderboardState::Error(_) => {
            draw_sidebar_note(canvas, font, "Stats warming up", x, y + 48, w, small)?;
        }
        LeaderboardState::Loaded(rows) if rows.is_empty() => {
            draw_sidebar_note(canvas, font, "No ranked sets yet", x, y + 48, w, small)?;
        }
        LeaderboardState::Loaded(rows) => {
            let rating_header = "RATING";
            let record_header = "W-L";
            let rhw = font.text_width_exact(rating_header, small);
            font.draw(
                canvas,
                record_header,
                x + w - 118,
                y + 36,
                small,
                Color::RGB(95, 115, 145),
            )?;
            font.draw(
                canvas,
                rating_header,
                x + w - rhw - 12,
                y + 36,
                small,
                Color::RGB(95, 115, 145),
            )?;

            let max_name_w = w - 170;
            for (idx, row) in rows.iter().take(10).enumerate() {
                let row_y = y + 56 + idx as i32 * row_h;
                let rank = format!("{}.", idx + 1);
                font.draw(
                    canvas,
                    &rank,
                    x + 12,
                    row_y,
                    small,
                    Color::RGB(150, 165, 190),
                )?;
                let name = fit_text(font, &row.username.to_uppercase(), small, max_name_w);
                font.draw(
                    canvas,
                    &name,
                    x + 34,
                    row_y,
                    small,
                    Color::RGB(225, 230, 240),
                )?;
                let record = format!("{}-{}", row.wins, row.losses);
                font.draw(
                    canvas,
                    &record,
                    x + w - 118,
                    row_y,
                    small,
                    Color::RGB(155, 170, 195),
                )?;
                let rating = row.rating.to_string();
                let rw = font.text_width_exact(&rating, small);
                font.draw(
                    canvas,
                    &rating,
                    x + w - rw - 12,
                    row_y,
                    small,
                    Color::RGB(120, 210, 150),
                )?;
            }
        }
    }

    Ok(())
}

fn draw_stroked_rect(
    canvas: &mut Canvas<Window>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: Color,
    stroke: i32,
) -> Result<(), String> {
    canvas.set_draw_color(color);
    for inset in 0..stroke {
        let rw = w - inset * 2;
        let rh = h - inset * 2;
        if rw > 0 && rh > 0 {
            canvas.draw_rect(Rect::new(x + inset, y + inset, rw as u32, rh as u32))?;
        }
    }
    Ok(())
}

fn draw_sidebar_note(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    note: &str,
    x: i32,
    y: i32,
    w: i32,
    scale: u32,
) -> Result<(), String> {
    let text = fit_text(font, note, scale, w - 24);
    let tw = font.text_width_exact(&text, scale);
    font.draw(
        canvas,
        &text,
        x + (w - tw) / 2,
        y,
        scale,
        Color::RGB(150, 165, 190),
    )
}

fn fit_text(font: &mut Font, text: &str, scale: u32, max_w: i32) -> String {
    if font.text_width_exact(text, scale) <= max_w {
        return text.to_string();
    }
    let mut out = text.to_string();
    while !out.is_empty() && font.text_width_exact(&format!("{out}..."), scale) > max_w {
        out.pop();
    }
    format!("{out}...")
}

fn draw_about(canvas: &mut Canvas<Window>, font: &mut Font, w: i32, h: i32) -> Result<(), String> {
    draw_title(canvas, font, "ABOUT", w, h)?;
    let body = body_scale(h);
    let small = small_scale(h);
    let content_scale = 1;
    let line_h = 18;
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
        "Freeplay v{}  build {}",
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

    let left = [
        "F2  Hitboxes",
        "F3  Health",
        "F4  Timer",
        "F5  Dummy mode",
        "F6/F7 Reset load/save",
        "F8/F9 Drone load/save",
        "F10 Punish trainer",
        "F11 Hide help",
        "F12 Play vs drone",
    ];
    let right = [
        "Online saves replays",
        "Replays opens online sets",
        "F1 leaves online",
        "F11 toggles ping/FPS",
        "T opens online chat",
        "Enter/Start sends chat",
        "Esc/B/Back closes chat",
        "Logs: freeplay-net.log",
        "Shift+F11 dumps RAM",
    ];
    let panel_h = 54 + left.len().max(right.len()) as i32 * line_h;
    draw_panel(
        canvas,
        left_x,
        y,
        col_w,
        panel_h,
        Color::RGBA(15, 16, 24, 220),
    )?;
    draw_panel(
        canvas,
        right_x,
        y,
        col_w,
        panel_h,
        Color::RGBA(15, 16, 24, 220),
    )?;
    font.draw(canvas, "LAB F KEYS", left_x + 14, y + 12, body, header)?;
    font.draw(canvas, "ONLINE", right_x + 14, y + 12, body, header)?;
    y += 44;

    for i in 0..left.len().max(right.len()) {
        if let Some(l) = left.get(i) {
            font.draw(canvas, l, left_x + 18, y, content_scale, body_c)?;
        }
        if let Some(r) = right.get(i) {
            if !r.is_empty() {
                font.draw(canvas, r, right_x + 18, y, content_scale, body_c)?;
            }
        }
        y += line_h;
    }

    let gh = "github.com/junkwax/freeplay-gametalk";
    let ghw = font.text_width_exact(gh, content_scale);
    font.draw(
        canvas,
        gh,
        (w - ghw) / 2,
        (h - 78).max(y + 18),
        content_scale,
        Color::RGB(160, 200, 255),
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

    let footer = "ENTER GitHub   R Refresh   ESC Back";
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
    discord_user: Option<&str>,
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

    let status_lower = status.to_ascii_lowercase();
    let hint = if status_lower.starts_with("checking name") {
        "Verifying your player name before entering the queue"
    } else {
        "Using your confirmed player name for online play"
    };
    let hw = font.text_width_exact(hint, small);
    font.draw(
        canvas,
        hint,
        cx - hw / 2,
        y,
        small,
        Color::RGB(140, 140, 160),
    )?;

    let footer = "F11 Stats   ESC Cancel";
    let fw = font.text_width_exact(footer, small);
    font.draw(
        canvas,
        footer,
        (w - fw) / 2,
        h - 28,
        small,
        Color::RGB(100, 100, 100),
    )?;
    draw_logged_in_as(canvas, font, discord_user, w, h)?;
    Ok(())
}

fn draw_match_username(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    value: &str,
    status: &str,
    checking: bool,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "CHOOSE YOUR NAME", w, h)?;
    let scale = body_scale(h);
    let small = small_scale(h);
    let panel_w = (w - 72).clamp(260, 520);
    let panel_x = (w - panel_w) / 2;
    let mut y = (h / 2) - 56;

    let label = "PLAYER NAME";
    font.draw(canvas, label, panel_x, y, small, Color::RGB(155, 165, 195))?;
    y += 20 * small as i32;

    let box_h = 34 * scale as i32;
    draw_panel(
        canvas,
        panel_x,
        y,
        panel_w,
        box_h,
        Color::RGBA(15, 18, 30, 230),
    )?;
    canvas.set_draw_color(if checking {
        Color::RGB(90, 105, 145)
    } else {
        Color::RGB(150, 180, 255)
    });
    canvas.draw_rect(Rect::new(panel_x, y, panel_w as u32, box_h as u32))?;

    let blink_on = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        / 400)
        % 2
        == 0;
    let shown = if blink_on && !checking {
        format!("{value}_")
    } else {
        value.to_string()
    };
    let clipped = fit_line(font, &shown, scale, panel_w - 24);
    font.draw(
        canvas,
        &clipped,
        panel_x + 12,
        y + 8,
        scale,
        Color::RGB(245, 245, 250),
    )?;
    y += box_h + 16;

    let dots = if checking {
        match (std::time::SystemTime::now()
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
        }
    } else {
        ""
    };
    let status = fit_line(font, &format!("{status}{dots}"), small, w - 40);
    let status_w = font.text_width_exact(&status, small);
    font.draw(
        canvas,
        &status,
        (w - status_w) / 2,
        y,
        small,
        if status.to_ascii_lowercase().contains("taken")
            || status.to_ascii_lowercase().contains("invalid")
            || status.to_ascii_lowercase().contains("verify")
            || status.to_ascii_lowercase().contains("timed out")
            || status.to_ascii_lowercase().contains("failed")
        {
            Color::RGB(235, 120, 105)
        } else {
            Color::RGB(180, 190, 215)
        },
    )?;

    let footer = if checking {
        "Checking...   ESC Back"
    } else {
        "ENTER Claim name   ESC Back"
    };
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
    player_username: &str,
    stats_email: &str,
    discord_connected: bool,
    discord_rpc_enabled: bool,
    fullscreen: bool,
    volume_percent: u8,
    audio_buffer: AudioBuffer,
    video_filter: VideoFilter,
    crt_corner_bend: bool,
    aspect_mode: AspectMode,
    scorebar_style: ScorebarStyle,
    input_delay: u32,
    render_profile: RenderProfile,
    runahead: bool,
    runahead_online: bool,
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
            0 => Some(
                if player_username.trim().is_empty() {
                    "Player"
                } else {
                    player_username
                }
                .to_string(),
            ),
            1 => Some(
                if stats_email.trim().is_empty() {
                    "optional"
                } else {
                    stats_email
                }
                .to_string(),
            ),
            2 => Some(
                if discord_connected {
                    "CONNECTED"
                } else {
                    "CONNECT"
                }
                .to_string(),
            ),
            3 => Some(if discord_rpc_enabled { "ON" } else { "OFF" }.to_string()),
            4 => Some(if fullscreen { "ON" } else { "OFF" }.to_string()),
            5 => Some(format!("{volume_percent}%")),
            6 => Some(audio_buffer.label().to_string()),
            7 => Some(video_filter.label().to_string()),
            8 => Some(if crt_corner_bend { "ON" } else { "OFF" }.to_string()),
            9 => Some(aspect_mode.label().to_string()),
            10 => Some(scorebar_style.label().to_string()),
            11 => Some(format!("{input_delay} FRAMES")),
            15 => Some(render_profile.label().to_string()),
            16 => Some(if runahead { "ON" } else { "OFF" }.to_string()),
            17 => Some(if runahead_online { "ON" } else { "OFF" }.to_string()),
            _ => None,
        };
        if let Some(value) = value {
            let value = fit_line(font, &value, scale, (w / 2).max(120));
            let vw = font.text_width_exact(&value, scale);
            let enabled_colour = match i {
                0 | 1 | 5 | 6 | 7 | 9 | 10 | 11 | 15 => Color::RGB(180, 205, 255),
                _ if value == "ON" || value == "CONNECTED" => Color::RGB(120, 230, 150),
                _ => Color::RGB(210, 140, 140),
            };
            font.draw(canvas, &value, w - x - vw, row_y, scale, enabled_colour)?;
        }
    }

    y += SETTINGS_ITEMS.len() as i32 * row_h + 18;
    let (hint_line, extra_hint_line) = settings_hint_lines(cursor);
    for (line_idx, line) in [Some(hint_line), extra_hint_line]
        .into_iter()
        .flatten()
        .enumerate()
    {
        let hint = fit_line(font, line, small, w - x * 2);
        font.draw(
            canvas,
            &hint,
            x,
            y + line_idx as i32 * ((small as i32 * 8) + 4),
            small,
            Color::RGB(130, 140, 165),
        )?;
    }

    let footer = "ENTER Select   LEFT/RIGHT Adjust   ESC Back";
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

fn settings_hint(cursor: usize) -> &'static str {
    match cursor {
        0 => "Public name used for Online and replays.",
        1 => "Optional email keeps stats portable across machines.",
        2 => "Connect Discord for account display and profile lookup.",
        3 => "Discord Rich Presence shows menu, queue, and match state.",
        4 => "Fullscreen toggles desktop fullscreen.",
        5 => "Volume adjusts game audio.",
        6 => "Stable audio adds a little buffer to reduce crackle.",
        7 => "Video filter applies during gameplay.",
        8 => "CRT glass controls rounded screen corners.",
        9 => "Aspect controls how the game fits the window.",
        10 => "Scorebar changes the online/Lab overlay style.",
        11 => "Netplay frames of input delay. Lower = snappier, higher = fewer rollbacks. Applies next match.",
        12 => "Doctor checks ROM, core, networking, and config.",
        13 => "Open the folder where Ctrl+R clips are written.",
        14 => "Open runtime logs next to the app.",
        15 => "Renderer backend for local hardware output tests. Restart applies.",
        16 => "One-frame runahead for offline play (Arcade/Lab/drones). Cuts input latency by a frame.",
        17 => "Experimental video-only runahead prediction during netplay. Cannot desync; off by default.",
        _ => "",
    }
}

fn settings_hint_lines(cursor: usize) -> (&'static str, Option<&'static str>) {
    match cursor {
        11 => (
            "Input delay 3 is the default; try 4-5 for Wi-Fi, moderate ping, or weaker PCs.",
            Some(
                "Use 6 as a fallback. Avoid 0-1 online; delay helps rollback, not CPU starvation.",
            ),
        ),
        _ => (settings_hint(cursor), None),
    }
}

fn draw_text_edit(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    title: &str,
    label: &str,
    value: &str,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, title, w, h)?;
    let scale = body_scale(h).saturating_sub(1).max(1);
    let small = small_scale(h);
    let x = (w / 8).max(42);
    let y = h / 3;
    font.draw(canvas, label, x, y, small, Color::RGB(145, 155, 180))?;
    let box_y = y + 24;
    let box_h = 34;
    canvas.set_draw_color(Color::RGBA(18, 22, 34, 230));
    canvas.fill_rect(Rect::new(x, box_y, (w - x * 2) as u32, box_h))?;
    canvas.set_draw_color(Color::RGBA(95, 130, 210, 210));
    canvas.draw_rect(Rect::new(x, box_y, (w - x * 2) as u32, box_h))?;
    let shown = fit_line(font, &format!("{value}_"), scale, w - x * 2 - 22);
    font.draw(
        canvas,
        &shown,
        x + 10,
        box_y + 9,
        scale,
        Color::RGB(235, 240, 255),
    )?;
    let footer = "ENTER Save   ESC Cancel";
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
    draw_title(canvas, font, "LAB", w, h)?;
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
        "F2/F3/F4 toggle training overlays while playing.",
        "F6/F7 load/save Lab reset slots; Ctrl+F7 changes slot.",
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
            let msg = "Sign in via Online first.";
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
        ProfileScreenState::Empty { username } => {
            let content_x = (w / 12).max(42);
            let content_w = w - content_x * 2;
            let top_h = 84.max(60 * small as i32);
            draw_panel(canvas, content_x, y, content_w, top_h, HUB_PANEL)?;
            font.draw(
                canvas,
                username,
                content_x + 18,
                y + 14,
                body,
                Color::RGB(255, 230, 120),
            )?;
            font.draw(
                canvas,
                "Unranked  ·  0 - 0",
                content_x + 18,
                y + 16 + 18 * body as i32,
                small,
                Color::RGB(170, 176, 196),
            )?;
            y += top_h + 26;
            let lines = [
                "No ranked matches yet.",
                "Play your first online match and your rating,",
                "record, and recent games show up here.",
            ];
            for line in lines {
                let tw = font.text_width_exact(line, small);
                font.draw(canvas, line, cx - tw / 2, y, small, Color::RGB(200, 205, 220))?;
                y += 22 * small as i32;
            }
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
    download_status: Option<&str>,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "LOAD DRONE", w, h)?;
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
        let msg = "No drone recordings found";
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
        let hint = "Record during netplay or use F9 to record locally";
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
        let header = format!("{} drones available", entries.len());
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
                    let time = parse_ghost_time(filename);
                    ("LOCAL", "Recorded match".to_string(), time)
                }
                GhostEntry::Remote(meta) => {
                    let who = if meta.username.trim().is_empty() {
                        "Community".to_string()
                    } else {
                        meta.username.clone()
                    };
                    let time = parse_ghost_time(&meta.filename);
                    let info = if time.is_empty() {
                        format!("{} \u{2022} {} frames", who, meta.frame_count)
                    } else {
                        format!(
                            "{} \u{2022} {} frames \u{2022} {}",
                            who, meta.frame_count, time
                        )
                    };
                    ("REMOTE", "Shared recording".to_string(), info)
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

    if let Some(status) = download_status {
        let status = fit_line(font, status, small, w - 48);
        let sw = font.text_width_exact(&status, small);
        font.draw(
            canvas,
            &status,
            (w - sw) / 2,
            h - 54,
            small,
            if status.starts_with("Error:") {
                Color::RGB(235, 120, 105)
            } else {
                Color::RGB(190, 200, 235)
            },
        )?;
    }

    let footer = "B/ESC Back   A/ENTER Load";
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

fn draw_replay_select(
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    cursor: usize,
    entries: &[ReplayEntry],
    status: Option<&str>,
    w: i32,
    h: i32,
) -> Result<(), String> {
    draw_title(canvas, font, "REPLAYS", w, h)?;
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
        let msg = status.unwrap_or("No local or public replays found");
        let msg = fit_line(font, msg, small, content_w - 24);
        let tw = font.text_width_exact(&msg, small);
        font.draw(
            canvas,
            &msg,
            cx - tw / 2,
            y,
            small,
            Color::RGB(180, 180, 200),
        )?;
        y += 24 * small as i32;
        let hint = "Completed sets save here; public replays load from GitHub";
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
        let header = format!("{} replays available", entries.len());
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
            let entry = &entries[i];
            let subtitle = {
                let duration = if entry.duration.is_empty() {
                    format_replay_duration(entry.frame_count)
                } else {
                    entry.duration.clone()
                };
                let time = replay_recorded_time(entry);
                let score = replay_score_line(entry);
                let marks = match entry.bookmark_count {
                    0 => String::new(),
                    1 => " - 1 mark".to_string(),
                    n => format!(" - {n} marks"),
                };
                if time.is_empty() {
                    format!("{duration} - {}f{score}{marks}", entry.frame_count)
                } else {
                    format!("{duration} - {time}{score}{marks}")
                }
            };
            let subtitle = fit_line(font, &subtitle, small, (content_w / 2).max(120));
            let subtitle_w = font.text_width_exact(&subtitle, small);
            let display = fit_line(
                font,
                &format!("{} vs {}", entry.p1_name, entry.p2_name),
                small,
                (content_w - subtitle_w - 124).max(72),
            );
            let bg = if selected {
                Color::RGBA(30, 44, 56, 235)
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
                Color::RGB(100, 210, 255)
            } else {
                Color::RGBA(70, 72, 88, 220)
            });
            canvas.fill_rect(Rect::new(content_x, y - 4, 3, (row_h - 4) as u32))?;
            let kind = if entry.remote_url.is_some() {
                "PUBLIC"
            } else {
                "LOCAL"
            };
            font.draw(
                canvas,
                kind,
                content_x + 12,
                y,
                small,
                Color::RGB(130, 136, 152),
            )?;
            font.draw(
                canvas,
                &display,
                content_x + 86,
                y,
                small,
                if selected {
                    Color::RGB(190, 235, 255)
                } else {
                    Color::RGB(220, 222, 232)
                },
            )?;
            font.draw(
                canvas,
                &subtitle,
                content_x + content_w - subtitle_w - 14,
                y,
                small,
                Color::RGB(130, 136, 152),
            )?;
            y += row_h;
        }
    }

    let selected_note = entries
        .get(cursor)
        .and_then(|entry| (!entry.note.is_empty()).then_some(format!("NOTE: {}", entry.note)));
    let bottom_status = status
        .filter(|_| !entries.is_empty())
        .map(str::to_string)
        .or(selected_note);
    if let Some(status) = bottom_status {
        let status = fit_line(font, &status, small, w - 48);
        let sw = font.text_width_exact(&status, small);
        font.draw(
            canvas,
            &status,
            (w - sw) / 2,
            h - 54,
            small,
            Color::RGB(190, 200, 235),
        )?;
    }

    let footer = fit_line(
        font,
        "B/ESC Back   A/ENTER Watch   N/RB Note   DEL/X Delete   O/Y Folder",
        small,
        w - 48,
    );
    let fw = font.text_width_exact(&footer, small);
    font.draw(
        canvas,
        &footer,
        (w - fw) / 2,
        h - 28,
        small,
        Color::RGB(100, 100, 100),
    )?;
    Ok(())
}

fn replay_score_line(entry: &ReplayEntry) -> String {
    match (entry.p1_score, entry.p2_score) {
        (Some(p1), Some(p2)) => {
            let winner = entry.winner.trim();
            if winner.is_empty() {
                format!(" - {p1}-{p2}")
            } else {
                format!(" - {p1}-{p2} {winner}")
            }
        }
        _ => String::new(),
    }
}

fn replay_recorded_time(entry: &ReplayEntry) -> String {
    let recorded = entry.recorded_at.trim();
    if recorded.is_empty() {
        return parse_replay_time(&entry.filename);
    }
    if let Ok(secs) = recorded.parse::<i64>() {
        format!("Recorded {}", chrono_prelude(secs))
    } else {
        format!("Recorded {recorded}")
    }
}

fn strip_ncgh(s: &str) -> String {
    if s.ends_with(".ncgh") {
        s[..s.len() - 5].to_string()
    } else {
        s.to_string()
    }
}

fn strip_ncrp(s: &str) -> String {
    if s.ends_with(".ncrp") {
        s[..s.len() - 5].to_string()
    } else {
        s.to_string()
    }
}

fn parse_replay_time(filename: &str) -> String {
    let base = strip_ncrp(filename);
    let ts = base.split('_').next().unwrap_or("");
    if let Ok(n) = ts.parse::<u64>() {
        return format!("Recorded {}", chrono_prelude(n as i64));
    }
    String::new()
}

fn format_replay_duration(frames: u32) -> String {
    let total_seconds = (frames as u64 + 30) / 60;
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

fn parse_ghost_time(filename: &str) -> String {
    let base = strip_ncgh(filename);
    let parts: Vec<&str> = base.split('_').collect();
    if parts.len() >= 2 {
        let ts = parts.last().unwrap_or(&"");
        if let Ok(n) = ts.parse::<u64>() {
            let secs = normalize_ghost_timestamp(n);
            return format!("Recorded {}", chrono_prelude(secs as i64));
        }
    }
    String::new()
}

fn normalize_ghost_timestamp(n: u64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if n > now + 365 * 24 * 60 * 60 && n > 1_000_000_000 {
        n % 1_000_000_000
    } else {
        n
    }
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

pub(crate) fn estimate_rank(rating: i32) -> &'static str {
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
