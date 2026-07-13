//! In-engine menu: list-based screens, pad/keyboard navigation, rebind flow.
use crate::config::MAX_USERNAME_LEN;
use crate::font::Font;
use crate::input::{Action, Binding, Player, PlayerBindings};
use crate::matchmaking::{
    HistoryRow, IncomingChallenge, LeaderboardRow, LiveMatch, LobbyChatMessage, LobbyCurrent,
    LobbyMatchFormat, LobbyMemberInfo, LobbyReadyCheck, LobbyUser, LobbyView, ProfileData,
    RemoteGhostMeta,
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
        /// Boxed `AppState` rather than `MenuScreen` so rebinding can be
        /// entered from an fp_ui screen (the new native Controls category)
        /// and correctly return there afterward, not just from legacy
        /// `MenuScreen::Controls`.
        came_from: Box<AppState>,
    },
}

/// The app's "main menu" state — the fp_ui Main Menu. Centralizes every
/// "return to the main menu" transition. (`MenuScreen::Main` still exists,
/// but only as the action-dispatch vehicle fp_ui's `ActivateMainItem`
/// fallthrough rides through `nav_accept` — it is never a resting state.)
pub fn main_menu_state() -> AppState {
    AppState::FpUi(crate::fp_ui::FpScreen::main())
}

/// All menu screens. Direct-IP Host/Join screens were removed when Find Match
/// (matchmaking server + STUN/TURN) replaced manual IP entry. The CLI
/// `--player/--local/--peer` direct-IP launch is still supported; it bypasses
/// the menu entirely via `cli::NetMode::P2P`.
/// What's left of the legacy menu screen set now that fp_ui draws every
/// menu-level screen natively. `Main` and `LabMenu` survive only as the
/// transient action-dispatch vehicles fp_ui's `ActivateMainItem`/
/// `ActivateLabMenuItem` fallthrough rides through `nav_accept` — they are
/// never a resting state and never drawn. `Spectate` and `TextEdit` are
/// real resting states whose *rendering* fp_ui provides (the live-match
/// viewer once frames flow is the one in-game legacy surface left here).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuScreen {
    Main {
        cursor: usize,
    },
    LabMenu {
        cursor: usize,
    },
    /// Live spectator view for a remote match. The signaling server currently
    /// exposes score/frame status, so this screen updates those values while
    /// full video playback is still future work.
    Spectate {
        session_id: String,
        status: SpectateStatus,
    },
    /// `came_from` is `Box<AppState>` (not `Box<MenuScreen>`) so this can
    /// return to a native `AppState::FpUi` screen exactly the way
    /// `AppState::Rebinding::came_from` already does, rather than always
    /// falling back to a legacy `MenuScreen` regardless of where the edit
    /// was triggered from.
    TextEdit {
        title: String,
        label: String,
        value: String,
        field: EditField,
        came_from: Box<AppState>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditField {
    Username,
    StatsEmail,
    ReplayNote { path: String },
    /// Entering a lobby invite code to join a private lobby.
    JoinCode,
    /// Composing a free-text lobby chat message from fp_ui's Chat tab —
    /// commit sends it (`matchmaking::send_lobby_chat`) and returns to
    /// `came_from`, replacing the old whole-screen handoff to legacy's
    /// OnlineHub just to reach an on-screen keyboard.
    ChatMessage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GhostEntry {
    /// On-disk `.ncgh` we recorded locally (or downloaded previously).
    /// `path` is what `ghost::Playback::load` opens. `frame_count` is read
    /// straight from the file's own header (`ghost::read_ncgh_frame_count`)
    /// at scan time — real data, not fabricated, matching what
    /// `RemoteGhostMeta` already carries for remote entries.
    Local { filename: String, path: String, frame_count: u32 },
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChallengeFormat {
    UnrankedVs,
    RankedFt3,
    RankedFt5,
    RankedFt10,
}

impl ChallengeFormat {
    pub const ALL: [ChallengeFormat; 4] = [
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

    pub fn ranked(self) -> bool {
        !matches!(self, ChallengeFormat::UnrankedVs)
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

/// Index of "Settings" within the old legacy main-menu ordering —
/// `ActivateMainItem` payloads still carry these indices (see `nav_accept`).
pub const MAIN_SETTINGS_INDEX: usize = 6;

/// Common chat phrases offered as one-click inserts.
pub const QUICK_PHRASES: [&str; 7] =
    ["gg", "ggs", "wp", "one more?", "rematch?", "nice", "lag?"];

/// The quick phrase at an index (for the click handler).
pub fn quick_phrase(idx: usize) -> &'static str {
    QUICK_PHRASES.get(idx).copied().unwrap_or("")
}

/// Build a screen pre-populated with sample data for layout/font testing
/// via `--test-screen` (all fixtures are `fp:*` names now that fp_ui is the
/// only menu UI).
pub fn test_state(name: &str) -> Option<AppState> {
    match name {
        "fp:main" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::Main { cursor: 0 })),
        "fp:quit" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Quit {
                choice: 0,
                menu_cursor: 0,
            }))
        }
        "fp:playmenu" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::PlayMenu { cursor: 0 })),
        "fp:labmenu" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::LabMenu { cursor: 0 })),
        "fp:ghostselect" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::GhostSelect {
                cursor: 0,
                section: 0,
                entries: vec![
                    GhostEntry::Local { filename: "ghost_1720600000.ncgh".into(), path: "ghosts/ghost_1720600000.ncgh".into(), frame_count: 8420 },
                    GhostEntry::Local { filename: "ghost_1720700000.ncgh".into(), path: "ghosts/ghost_1720700000.ncgh".into(), frame_count: 11340 },
                    GhostEntry::Remote(RemoteGhostMeta {
                        ghost_id: "g1".into(),
                        filename: "scorpion_pit.ncgh".into(),
                        username: "SCORPION_PIT".into(),
                        frame_count: 23400,
                    }),
                    GhostEntry::Remote(RemoteGhostMeta {
                        ghost_id: "g2".into(),
                        filename: "raidenbolt.ncgh".into(),
                        username: "RaidenBolt99".into(),
                        frame_count: 18200,
                    }),
                ],
                status: None,
            }))
        }
        "fp:replayselect" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::ReplaySelect {
                cursor: 0,
                entries: vec![
                    ReplayEntry {
                        filename: "1719index_a.ncrp".into(),
                        path: "replays/1719index_a.ncrp".into(),
                        remote_url: None,
                        p1_name: "Respected_Hunter".into(),
                        p2_name: "Scorpion_Pit".into(),
                        p1_score: Some(3),
                        p2_score: Some(1),
                        winner: "Respected_Hunter".into(),
                        frame_count: 28400,
                        duration: "8:42".into(),
                        recorded_at: "2026-06-25T00:00:00Z".into(),
                        note: "Good block strings".into(),
                        bookmark_count: 3,
                    },
                    ReplayEntry {
                        filename: "1719index_b.ncrp".into(),
                        path: "replays/1719index_b.ncrp".into(),
                        remote_url: None,
                        p1_name: "Respected_Hunter".into(),
                        p2_name: "KungLaoFan".into(),
                        p1_score: Some(1),
                        p2_score: Some(3),
                        winner: "KungLaoFan".into(),
                        frame_count: 36900,
                        duration: "11:15".into(),
                        recorded_at: "2026-06-25T00:00:00Z".into(),
                        note: String::new(),
                        bookmark_count: 0,
                    },
                    ReplayEntry {
                        filename: String::new(),
                        path: String::new(),
                        remote_url: Some("https://stats.example/replays/r3".into()),
                        p1_name: "Sub__Zero".into(),
                        p2_name: "MileenaX".into(),
                        p1_score: Some(0),
                        p2_score: Some(3),
                        winner: "MileenaX".into(),
                        frame_count: 20500,
                        duration: "6:21".into(),
                        recorded_at: "2026-06-23T00:00:00Z".into(),
                        note: String::new(),
                        bookmark_count: 12,
                    },
                ],
                status: None,
            }))
        }
        "fp:bandwidth" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::Bandwidth)),
        "fp:matchmaking" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Matchmaking {
                status: "Entering queue as SUDDEN_RECLINE".into(),
            }))
        }
        "fp:failure" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::ConnectionFailed {
                lines: vec![
                    "FAIL Match connect failed: relay handshake timed out after 12s".into(),
                    "WARN Peer NAT type: symmetric (TURN relay required)".into(),
                    "OK Local network and ROM checksum verified".into(),
                    "Log: freeplay-net.log".into(),
                    "OK Incident report submitted automatically".into(),
                ],
            }))
        }
        "fp:discord" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::DiscordConnect {
                status: "Waiting for authorization".into(),
            }))
        }
        "fp:osk" => {
            return Some(AppState::Menu(MenuScreen::TextEdit {
                title: "USERNAME".into(),
                label: "2-16 characters - letters, digits, _ or -".into(),
                value: "SUDDEN_REC".into(),
                field: EditField::Username,
                came_from: Box::new(AppState::FpUi(crate::fp_ui::FpScreen::settings_account(
                    &crate::config::load(),
                ))),
            }))
        }
        "fp:osk:joincode" => {
            return Some(AppState::Menu(MenuScreen::TextEdit {
                title: "JOIN LOBBY".into(),
                label: "Enter the 6-character invite code".into(),
                value: "X9K".into(),
                field: EditField::JoinCode,
                came_from: Box::new(AppState::FpUi(crate::fp_ui::FpScreen::lobby())),
            }))
        }
        "fp:rebind" => {
            return Some(AppState::Rebinding {
                action: Action::HighPunch,
                player: Player::P1,
                came_from: Box::new(AppState::FpUi(crate::fp_ui::FpScreen::settings_from_cfg(
                    &crate::config::load(),
                ))),
            })
        }
        "fp:spectate" => {
            let mut status = SpectateStatus::waiting();
            status.p1_name = "SUDDEN_RECLINE".into();
            status.p2_name = "IRON_MONGER".into();
            status.p1_score = 1;
            status.p2_score = 1;
            return Some(AppState::Menu(MenuScreen::Spectate {
                session_id: "test".into(),
                status,
            }));
        }
        "fp:rankings" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::Rankings)),
        "fp:about" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::About)),
        "fp:profile" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::Profile)),
        "fp:settings" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::settings_from_cfg(
                &crate::config::load(),
            )))
        }
        "fp:settings:account" => {
            let crate::fp_ui::FpScreen::Settings { fields, sidebar_focus, controls_player, test_conn_address, test_conn_lines, .. } =
                crate::fp_ui::FpScreen::settings_from_cfg(&crate::config::load())
            else {
                unreachable!()
            };
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Settings {
                cat: crate::fp_ui::settings::ACCOUNT_CAT_INDEX,
                row: 0,
                fields,
                sidebar_focus,
                controls_player,
                test_conn_address,
                test_conn_lines,
            }));
        }
        "fp:settings:testconn" => {
            let crate::fp_ui::FpScreen::Settings { fields, sidebar_focus, controls_player, .. } =
                crate::fp_ui::FpScreen::settings_from_cfg(&crate::config::load())
            else {
                unreachable!()
            };
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Settings {
                cat: crate::fp_ui::settings::TEST_CONN_CAT_INDEX,
                row: 0,
                fields,
                sidebar_focus,
                controls_player,
                test_conn_address: "127.0.0.1:7000".into(),
                test_conn_lines: Vec::new(),
            }));
        }
        "fp:lobby" => return Some(AppState::FpUi(crate::fp_ui::FpScreen::lobby())),
        "fp:lobby:searching" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Lobby {
                tab: 0,
                host_join_focus: 0,
                cursor: 0,
                lobbies: Vec::new(),
                status: String::new(),
                chat: Vec::new(),
                presence: Vec::new(),
                live_matches: Vec::new(),
                challenge_pick: None,
                incoming: None,
                quick_match_status: Some("Entering queue as SUDDEN_RECLINE".into()),
            }))
        }
        "fp:lobby:host" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Lobby {
                tab: 1,
                host_join_focus: 0,
                cursor: 0,
                lobbies: Vec::new(),
                status: String::new(),
                chat: Vec::new(),
                presence: Vec::new(),
                live_matches: Vec::new(),
                challenge_pick: None,
                incoming: None,
                quick_match_status: None,
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
                chat: Vec::new(),
                presence: Vec::new(),
                live_matches: Vec::new(),
                challenge_pick: None,
                incoming: None,
                quick_match_status: None,
            }))
        }
        "fp:lobby:chat" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Lobby {
                tab: 3,
                host_join_focus: 0,
                cursor: 0,
                lobbies: Vec::new(),
                status: String::new(),
                chat: vec![
                    LobbyChatMessage {
                        username: "ScorpionKiller".into(),
                        message: "gg wp that was close".into(),
                        timestamp: Some("21:04".into()),
                    },
                    LobbyChatMessage {
                        username: "SubZeroFan".into(),
                        message: "anyone up for an ft5?".into(),
                        timestamp: Some("21:05".into()),
                    },
                ],
                presence: vec![
                    LobbyUser {
                        player_id: "p1".into(),
                        username: "ScorpionKiller".into(),
                        status: "online".into(),
                        rating: Some(1403),
                    },
                    LobbyUser {
                        player_id: "p2".into(),
                        username: "SubZeroFan".into(),
                        status: "in lobby".into(),
                        rating: Some(1521),
                    },
                ],
                live_matches: Vec::new(),
                challenge_pick: None,
                incoming: None,
                quick_match_status: None,
            }))
        }
        "fp:lobby:watch" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Lobby {
                tab: 4,
                host_join_focus: 0,
                cursor: 0,
                lobbies: Vec::new(),
                status: String::new(),
                chat: Vec::new(),
                presence: Vec::new(),
                live_matches: vec![
                    LiveMatch {
                        session_id: "s1".into(),
                        p1_name: "Liu Kang".into(),
                        p2_name: "Kung Lao".into(),
                        p1_score: 2,
                        p2_score: 1,
                    },
                    LiveMatch {
                        session_id: "s2".into(),
                        p1_name: "Mileena".into(),
                        p2_name: "Kitana".into(),
                        p1_score: 0,
                        p2_score: 3,
                    },
                ],
                challenge_pick: None,
                incoming: None,
                quick_match_status: None,
            }))
        }
        "fp:lobby:players" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Lobby {
                tab: 5,
                host_join_focus: 0,
                cursor: 0,
                lobbies: Vec::new(),
                status: String::new(),
                chat: Vec::new(),
                presence: vec![
                    LobbyUser { player_id: "p1".into(), username: "ScorpionKiller".into(), status: "online".into(), rating: Some(1403) },
                    LobbyUser { player_id: "p2".into(), username: "SubZeroFan".into(), status: "in lobby".into(), rating: Some(1521) },
                    LobbyUser { player_id: "p3".into(), username: "Kano_Main".into(), status: "online".into(), rating: Some(1288) },
                ],
                live_matches: Vec::new(),
                challenge_pick: None,
                incoming: None,
                quick_match_status: None,
            }))
        }
        "fp:lobby:incoming" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::Lobby {
                tab: 5,
                host_join_focus: 0,
                cursor: 0,
                lobbies: Vec::new(),
                status: String::new(),
                chat: Vec::new(),
                presence: vec![
                    LobbyUser { player_id: "p1".into(), username: "ScorpionKiller".into(), status: "online".into(), rating: Some(1403) },
                ],
                live_matches: Vec::new(),
                challenge_pick: None,
                incoming: Some(IncomingChallenge {
                    challenge_id: "ch1".into(),
                    from_username: "ScorpionKiller".into(),
                    format: LobbyMatchFormat::RankedFt5,
                }),
                quick_match_status: None,
            }))
        }
        "fp:sessionended" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::SessionEnded {
                lines: vec![
                    "OK Match completed \u{b7} FT3 \u{b7} 3-1".into(),
                    "WARN 2 rollback resyncs (avg 4 frames)".into(),
                    "Replay saved locally".into(),
                ],
                replay_path: Some("dummy.ncrp".into()),
                choice: 0,
            }))
        }
        "fp:claimusername" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::ClaimUsername {
                value: crate::config::default_username(),
                status: "This is your name — edit it or press Enter to claim it".into(),
                checking: false,
            }))
        }
        "fp:lobbyroom:empty" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::LobbyRoom {
                id: "AB12CD".into(),
                view: Some(LobbyView {
                    id: "AB12CD".into(),
                    name: "Respected_Hunter's lobby".into(),
                    ranked: false,
                    private: true,
                    format: LobbyMatchFormat::UnrankedVs,
                    members: vec![
                        LobbyMemberInfo { username: "Respected_Hunter".into(), rating: Some(1502), queued: false, in_match: false },
                    ],
                    queue: vec![],
                    current: None,
                    ready_check: None,
                    your_position: None,
                    your_queued: false,
                    your_session: None,
                    your_turn: false,
                }),
                status: String::new(),
                thumb: None,
            }))
        }
        "fp:lobbyroom:queued" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::LobbyRoom {
                id: "AB12CD".into(),
                view: Some(LobbyView {
                    id: "AB12CD".into(),
                    name: "Respected_Hunter's lobby".into(),
                    ranked: false,
                    private: true,
                    format: LobbyMatchFormat::UnrankedVs,
                    members: vec![
                        LobbyMemberInfo { username: "Respected_Hunter".into(), rating: Some(1502), queued: true, in_match: false },
                        LobbyMemberInfo { username: "Sub_Zero_Fan".into(), rating: Some(1610), queued: false, in_match: true },
                        LobbyMemberInfo { username: "Kano_Main".into(), rating: Some(1480), queued: false, in_match: true },
                    ],
                    queue: vec!["Respected_Hunter".into()],
                    current: Some(LobbyCurrent {
                        host_username: "Sub_Zero_Fan".into(),
                        join_username: "Kano_Main".into(),
                        host_session: String::new(),
                        join_session: String::new(),
                    }),
                    ready_check: None,
                    your_position: Some(0),
                    your_queued: true,
                    your_session: None,
                    your_turn: false,
                }),
                status: String::new(),
                thumb: None,
            }))
        }
        "fp:lobbyroom:ready" => {
            return Some(AppState::FpUi(crate::fp_ui::FpScreen::LobbyRoom {
                id: "AB12CD".into(),
                view: Some(LobbyView {
                    id: "AB12CD".into(),
                    name: "Respected_Hunter's lobby".into(),
                    ranked: false,
                    private: true,
                    format: LobbyMatchFormat::UnrankedVs,
                    members: vec![
                        LobbyMemberInfo { username: "Respected_Hunter".into(), rating: Some(1502), queued: false, in_match: false },
                        LobbyMemberInfo { username: "Sub_Zero_Fan".into(), rating: Some(1610), queued: false, in_match: false },
                    ],
                    queue: vec![],
                    current: None,
                    ready_check: Some(LobbyReadyCheck {
                        champion_username: "Sub_Zero_Fan".into(),
                        challenger_username: "Respected_Hunter".into(),
                        seconds_left: 8,
                        you_are_challenger: true,
                    }),
                    your_position: None,
                    your_queued: false,
                    your_session: None,
                    your_turn: false,
                }),
                status: String::new(),
                thumb: None,
            }))
        }
        _ => {}
    }
    None
}

/// What's left of the legacy action-result enum: only what the surviving
/// `nav_accept` arms can still return. Everything else moved to
/// `fp_ui::FpResult`, whose handlers in main.rs carry the real side
/// effects now.
pub enum NavResult {
    Stay,
    StartLocal {
        lab: bool,
    },
    /// Open Settings screen (fp_ui's — legacy has no Settings anymore).
    OpenSettings,
    /// `Box<AppState>` is the screen editing began from — see
    /// `MenuScreen::TextEdit::came_from`'s doc comment.
    CommitText(EditField, String, Box<AppState>),
}

impl AppState {
    /// Directional nav for the residual legacy states. None of them has a
    /// cursor to move anymore — the surviving resting states (Spectate,
    /// TextEdit) are cursor-less and the `Main`/`LabMenu` dispatch vehicles
    /// are never navigated, only Accept-ed — so these are no-ops kept for
    /// main.rs's uniform `MenuNav` dispatch. fp_ui screens handle their own
    /// navigation in `fp_ui::nav`.
    pub fn nav_up(&mut self) {}

    pub fn nav_down(&mut self) {}

    pub fn nav_left(&mut self) {}

    pub fn nav_right(&mut self) {}

    /// The legacy `nav_accept` survives as the action dispatcher fp_ui's
    /// `FpResult::ActivateMainItem`/`ActivateLabMenuItem` fallthrough rides:
    /// fp_ui parks the state on a `Main`/`LabMenu` vehicle with a legacy
    /// cursor index and lets the unmodified `NavResult` handling in main.rs
    /// run the real side effects. Only the indices fp_ui actually sends
    /// remain — Arcade (`LEGACY_ARCADE_INDEX` = 1), Settings
    /// (`MAIN_SETTINGS_INDEX` = 6), Start Lab (LabMenu 0) — plus TextEdit's
    /// commit, which is a real resting state's Accept.
    pub fn nav_accept(&mut self, rom_present: bool) -> NavResult {
        match self.clone() {
            AppState::Menu(MenuScreen::Main { cursor }) => match cursor {
                1 => {
                    // Arcade. On a failed ROM check, return to the fp main
                    // menu rather than staying parked on this invisible
                    // dispatch vehicle (fp_ui gates entry on rom_present, but
                    // the ROM can disappear between that check and this one).
                    if !rom_present {
                        *self = main_menu_state();
                        return NavResult::Stay;
                    }
                    *self = AppState::Playing;
                    NavResult::StartLocal { lab: false }
                }
                6 => NavResult::OpenSettings,
                _ => {
                    *self = main_menu_state();
                    NavResult::Stay
                }
            },
            AppState::Menu(MenuScreen::LabMenu { cursor }) => match cursor {
                0 => {
                    if !rom_present {
                        *self = main_menu_state();
                        return NavResult::Stay;
                    }
                    *self = AppState::Playing;
                    NavResult::StartLocal { lab: true }
                }
                _ => {
                    *self = main_menu_state();
                    NavResult::Stay
                }
            },
            AppState::Menu(MenuScreen::TextEdit { field, value, came_from, .. }) => {
                NavResult::CommitText(field, value, came_from)
            }
            _ => NavResult::Stay,
        }
    }

    pub fn nav_back(&mut self) {
        match self {
            AppState::Menu(MenuScreen::Main { .. })
            | AppState::Menu(MenuScreen::LabMenu { .. })
            | AppState::Menu(MenuScreen::Spectate { .. }) => {
                *self = main_menu_state();
            }
            AppState::Menu(MenuScreen::TextEdit { came_from, .. }) => {
                *self = (**came_from).clone();
            }
            _ => {}
        }
    }

    pub fn finish_rebind(&mut self) {
        if let AppState::Rebinding { came_from, .. } = self.clone() {
            *self = *came_from;
        }
    }

    /// Append typed text to the active menu editor. No-op if nothing is editing.
    pub fn text_input(&mut self, s: &str) {
        if let AppState::FpUi(crate::fp_ui::FpScreen::Settings {
            cat,
            test_conn_address,
            ..
        }) = self
        {
            if *cat == crate::fp_ui::settings::TEST_CONN_CAT_INDEX {
                for c in s.chars() {
                    if (c.is_ascii_digit() || c == '.' || c == ':') && test_conn_address.len() < 24 {
                        test_conn_address.push(c);
                    }
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
                    // Same 180-char cap as legacy OnlineHub's `chat_draft`.
                    EditField::ChatMessage => {
                        if !c.is_control() && value.chars().count() < 180 {
                            value.push(c);
                        }
                    }
                }
            }
        } else if let AppState::FpUi(crate::fp_ui::FpScreen::ClaimUsername { value, .. }) = self {
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
        if let AppState::FpUi(crate::fp_ui::FpScreen::Settings {
            cat,
            test_conn_address,
            ..
        }) = self
        {
            if *cat == crate::fp_ui::settings::TEST_CONN_CAT_INDEX {
                test_conn_address.pop();
            }
        } else if let AppState::Menu(MenuScreen::TextEdit { value, .. }) = self {
            value.pop();
        } else if let AppState::FpUi(crate::fp_ui::FpScreen::ClaimUsername { value, .. }) = self {
            value.pop();
        }
    }
}

/// Parse "1.2.3.4:7000" or "1.2.3.4" (default port added) into a SocketAddr.
pub fn parse_ip_port(s: &str) -> Option<std::net::SocketAddr> {
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
    canvas: &mut Canvas<Window>,
    font: &mut Font,
    w: i32,
    h: i32,
    toast: Option<Toast<'_>>,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGB(8, 8, 16));
    canvas.clear();

    match state {
        // The live-match spectator viewer once frames flow — the one
        // legacy-rendered menu-side state left (its "connecting" phase is
        // fp_ui's `spectate_connecting`). Everything else that could reach
        // this function is either a transient dispatch vehicle
        // (Main/LabMenu), rendered natively by fp_ui before this is called
        // (FpUi screens, TextEdit/Rebinding overlays), or in-game.
        AppState::Menu(MenuScreen::Spectate { session_id, status }) => {
            draw_spectate(canvas, font, session_id, status, w, h)?
        }
        _ => {}
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

pub(crate) fn summarize_bindings(pb: &PlayerBindings, action: Action) -> String {
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

pub(crate) fn pretty_binding_name(s: &str) -> String {
    s.replace("DPAD", "D-PAD ")
        .replace("TRIGGER", "TRIGGER ")
        .replace("SHOULDER", "SHOULDER ")
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
